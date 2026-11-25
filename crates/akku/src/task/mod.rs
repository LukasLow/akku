//! Zero-dependency async task executor and runtime.
//!
//! `akku::task` drives Rust [`Future`]s on a single-threaded
//! runtime integrated with the native OS I/O reactor (`akku::net`).
//!
//! # Overview
//!
//! | Task | API |
//! |---|---|
//! | Run a future on a fresh runtime | [`block_on`], [`try_block_on`] |
//! | Manage a reusable runtime with bounds | [`Runtime`], [`Limits`] |
//! | Spawn concurrent background tasks | [`spawn`], [`try_spawn`], [`Runtime::spawn`], [`Runtime::try_spawn`] |
//! | Await, detach, or cancel a task | [`JoinHandle`], [`JoinError`], [`SpawnError`] |
//! | Yield cooperatively to sibling tasks & I/O | [`yield_now`], [`YieldNow`] |
//!
//! # Task lifecycle and panic isolation
//!
//! - Dropping a [`JoinHandle`] **detaches** the task; it continues running until
//!   it finishes or the owning [`Runtime`] is dropped. Call [`JoinHandle::abort`]
//!   for explicit cancellation.
//! - Panics inside spawned tasks (and inside their destructors) are caught and
//!   returned as [`JoinError`] (`JoinError::is_panic() == true`) rather than
//!   unwinding the runtime thread.
//! - Dropping a [`Runtime`] cancels and drops all remaining unfinished tasks and
//!   shuts down its I/O reactor.
//!
//! # Limits and defaults
//!
//! Resource and fairness bounds are configured via [`Limits`]:
//!
//! - [`Limits::max_tasks`]: `65_536` concurrent unfinished spawned tasks.
//! - [`Limits::max_polls_per_tick`]: `64` task polls before yielding to I/O and
//!   re-checking the root `block_on` future (values of `0` are clamped to `1`).
//! - [`Limits::max_io_events_per_tick`]: `1_024` OS I/O events drained per
//!   reactor wait turn (values of `0` are clamped to `1`).
//!
//! # Examples
//!
//! ```
//! use akku::task;
//!
//! let total = task::block_on(async {
//!     let a = task::spawn(async { 20usize });
//!     let b = task::spawn(async { 22usize });
//!     a.await.unwrap() + b.await.unwrap()
//! });
//! assert_eq!(total, 42);
//! ```
//!
//! Enforcing explicit task limits with [`Runtime`] and [`Limits`]:
//!
//! ```
//! use akku::task::{Limits, Runtime, SpawnError};
//!
//! let rt = Runtime::with_limits(Limits::new().with_max_tasks(1))?;
//! rt.block_on(async {
//!     let first = akku::task::try_spawn(async { 1u8 })?;
//!     assert_eq!(
//!         akku::task::try_spawn(async { 2u8 }).unwrap_err(),
//!         SpawnError::TaskLimitExceeded { limit: 1 },
//!     );
//!     assert_eq!(first.await.unwrap(), 1);
//!     Ok::<(), SpawnError>(())
//! })?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use std::any::Any;
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::error::Error;
use std::fmt;
use std::future::Future;
use std::io;
use std::panic::{self, AssertUnwindSafe};
use std::pin::{pin, Pin};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::task::{Context, Poll, Wake, Waker};
use std::time::Duration;

/// Resource and fairness bounds for a [`Runtime`].
///
/// Values of `0` for [`max_polls_per_tick`](Self::max_polls_per_tick) or
/// [`max_io_events_per_tick`](Self::max_io_events_per_tick) are clamped
/// internally to `1` so a runtime can never livelock. Setting
/// [`max_tasks`](Self::max_tasks) to `0` forbids all `spawn` calls while still
/// allowing [`Runtime::block_on`] to execute the root future.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Maximum number of live (unfinished) spawned tasks per runtime.
    ///
    /// Default: `65_536`.
    pub max_tasks: usize,
    /// Maximum spawned-task polls executed before yielding to the I/O reactor
    /// and checking the root `block_on` future.
    ///
    /// Default: `64`.
    pub max_polls_per_tick: usize,
    /// Maximum OS I/O events drained per reactor wait turn.
    ///
    /// Default: `1_024`.
    pub max_io_events_per_tick: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self::new()
    }
}

impl Limits {
    /// Returns a [`Limits`] value initialized with the default bounds.
    pub const fn new() -> Self {
        Self {
            max_tasks: 65_536,
            max_polls_per_tick: 64,
            max_io_events_per_tick: 1_024,
        }
    }

    /// Sets [`Limits::max_tasks`] and returns the updated configuration.
    pub const fn with_max_tasks(mut self, max: usize) -> Self {
        self.max_tasks = max;
        self
    }

    /// Sets [`Limits::max_polls_per_tick`] and returns the updated configuration.
    pub const fn with_max_polls_per_tick(mut self, max: usize) -> Self {
        self.max_polls_per_tick = max;
        self
    }

    /// Sets [`Limits::max_io_events_per_tick`] and returns the updated configuration.
    pub const fn with_max_io_events_per_tick(mut self, max: usize) -> Self {
        self.max_io_events_per_tick = max;
        self
    }
}

// --- Errors -------------------------------------------------------------------

/// Locks `mutex`, ignoring poisoning: every critical section in this module
/// leaves its data consistent, and user code never runs under these locks.
fn lock<T: ?Sized>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

enum JoinErrorRepr {
    Cancelled,
    Panic(Mutex<Option<Box<dyn Any + Send + 'static>>>),
}

/// Error returned when awaiting a [`JoinHandle`] whose task did not complete normally.
///
/// A task can fail to produce its output either because it was cancelled
/// ([`JoinError::is_cancelled`]) or because its future or destructor panicked
/// ([`JoinError::is_panic`]).
pub struct JoinError {
    repr: JoinErrorRepr,
}

impl JoinError {
    fn cancelled() -> Self {
        Self {
            repr: JoinErrorRepr::Cancelled,
        }
    }

    fn panic(payload: Box<dyn Any + Send + 'static>) -> Self {
        Self {
            repr: JoinErrorRepr::Panic(Mutex::new(Some(payload))),
        }
    }

    /// Returns `true` if the task was cancelled via [`JoinHandle::abort`] or
    /// [`Runtime`] shutdown.
    pub fn is_cancelled(&self) -> bool {
        matches!(self.repr, JoinErrorRepr::Cancelled)
    }

    /// Returns `true` if the task terminated because its future (or destructor)
    /// panicked.
    pub fn is_panic(&self) -> bool {
        matches!(self.repr, JoinErrorRepr::Panic(_))
    }

    /// Consumes the error and returns the panic payload.
    ///
    /// # Panics
    ///
    /// Panics if `!self.is_panic()`.
    #[track_caller]
    pub fn into_panic(self) -> Box<dyn Any + Send + 'static> {
        match self.try_into_panic() {
            Ok(payload) => payload,
            Err(err) => panic!("JoinError::into_panic called on a non-panic error: {err}"),
        }
    }

    /// Consumes the error and returns the panic payload if `self.is_panic()`,
    /// or `Err(self)` otherwise.
    pub fn try_into_panic(self) -> Result<Box<dyn Any + Send + 'static>, Self> {
        match self.repr {
            JoinErrorRepr::Panic(payload) => {
                let payload = payload.into_inner().unwrap_or_else(|e| e.into_inner());
                // The payload is only taken by consuming `self`, so it is present.
                Ok(payload.unwrap_or_else(|| Box::new("<panic payload unavailable>")))
            }
            repr => Err(Self { repr }),
        }
    }

    fn panic_message(&self) -> Option<String> {
        let JoinErrorRepr::Panic(payload) = &self.repr else {
            return None;
        };
        let payload = lock(payload);
        let payload = payload.as_deref()?;
        if let Some(s) = payload.downcast_ref::<&'static str>() {
            Some((*s).to_string())
        } else {
            payload.downcast_ref::<String>().cloned()
        }
    }
}

impl fmt::Debug for JoinError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.repr {
            JoinErrorRepr::Cancelled => f.write_str("JoinError::Cancelled"),
            JoinErrorRepr::Panic(_) => match self.panic_message() {
                Some(msg) => write!(f, "JoinError::Panic({msg:?})"),
                None => f.write_str("JoinError::Panic(..)"),
            },
        }
    }
}

impl fmt::Display for JoinError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.repr {
            JoinErrorRepr::Cancelled => f.write_str("task was cancelled"),
            JoinErrorRepr::Panic(_) => match self.panic_message() {
                Some(msg) => write!(f, "task panicked: {msg}"),
                None => f.write_str("task panicked"),
            },
        }
    }
}

impl Error for JoinError {}

impl From<JoinError> for io::Error {
    fn from(err: JoinError) -> Self {
        io::Error::other(err)
    }
}

/// Error returned by [`try_spawn`] or [`Runtime::try_spawn`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SpawnError {
    /// Called outside an active [`Runtime`] / [`block_on`] context.
    NoRuntime,
    /// The runtime is shutting down and no longer accepts new tasks.
    ShuttingDown,
    /// The number of live tasks reached [`Limits::max_tasks`].
    TaskLimitExceeded {
        /// The configured maximum live task limit that was reached.
        limit: usize,
    },
}

impl fmt::Display for SpawnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoRuntime => f.write_str("no active akku::task runtime on this thread"),
            Self::ShuttingDown => f.write_str("akku::task runtime is shutting down"),
            Self::TaskLimitExceeded { limit } => {
                write!(f, "task limit exceeded: at most {limit} live tasks")
            }
        }
    }
}

impl Error for SpawnError {}

impl From<SpawnError> for io::Error {
    fn from(err: SpawnError) -> Self {
        io::Error::other(err)
    }
}

// --- Driver (I/O reactor or plain parker) -------------------------------------

/// The native OS I/O reactor; `block_on` parks inside its wait.
#[cfg(not(target_arch = "wasm32"))]
type Driver = Arc<crate::net::reactor::Reactor>;

#[cfg(not(target_arch = "wasm32"))]
fn new_driver(max_events: usize) -> io::Result<Driver> {
    crate::net::reactor::Reactor::new(max_events).map(Arc::new)
}

/// `wasm32` has no OS reactor; a plain parker stands in.
#[cfg(target_arch = "wasm32")]
type Driver = Parker;

#[cfg(target_arch = "wasm32")]
fn new_driver(_max_events: usize) -> io::Result<Driver> {
    Ok(Parker::new())
}

/// A safe `Mutex` + `Condvar` parker, used where no native reactor exists
/// (`wasm32` here, and the non-Tier-1 fallback in `net::sys`).
#[cfg_attr(not(target_arch = "wasm32"), allow(unused_imports))]
pub(crate) use parker::Parker;

mod parker {
    use std::io;
    use std::sync::{Condvar, Mutex};
    use std::time::Duration;

    use super::lock;

    /// Blocks the driving thread until [`Parker::unpark`] or a timeout.
    ///
    /// Unused on Tier-1 native targets, where the OS reactor parks instead.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    pub(crate) struct Parker {
        notified: Mutex<bool>,
        cv: Condvar,
    }

    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    impl Parker {
        pub(crate) fn new() -> Self {
            Self {
                notified: Mutex::new(false),
                cv: Condvar::new(),
            }
        }

        pub(crate) fn turn(&self, timeout: Option<Duration>) -> io::Result<()> {
            let mut notified = lock(&self.notified);
            match timeout {
                Some(d) if d.is_zero() => {}
                Some(d) => {
                    if !*notified {
                        notified = self
                            .cv
                            .wait_timeout(notified, d)
                            .unwrap_or_else(|e| e.into_inner())
                            .0;
                    }
                }
                None => {
                    while !*notified {
                        notified = self.cv.wait(notified).unwrap_or_else(|e| e.into_inner());
                    }
                }
            }
            *notified = false;
            Ok(())
        }

        pub(crate) fn unpark(&self) {
            *lock(&self.notified) = true;
            self.cv.notify_one();
        }

        pub(crate) fn shutdown(&self) {}
    }
}

// --- Runtime internals ----------------------------------------------------------

/// Live-task registry; also the gate that enforces `max_tasks` and shutdown.
struct Registry {
    live: HashMap<u64, Arc<Task>>,
    next_id: u64,
    shutting_down: bool,
}

/// State shared by a [`Runtime`], its tasks' wakers, and the thread-local
/// context of an active `block_on`.
pub(crate) struct Shared {
    limits: Limits,
    poll_budget: usize,
    queue: Mutex<VecDeque<Arc<Task>>>,
    registry: Mutex<Registry>,
    /// `yield_now` wakers, woken only after the next reactor turn.
    deferred: Mutex<Vec<Waker>>,
    /// Set while the driving thread may block in the driver (lost-wakeup guard).
    parked: AtomicBool,
    shutting_down: AtomicBool,
    /// Serializes `block_on` calls from different threads.
    drive: Mutex<()>,
    driver: Driver,
}

impl Shared {
    fn new(limits: Limits) -> io::Result<Arc<Self>> {
        let driver = new_driver(limits.max_io_events_per_tick.max(1))?;
        Ok(Arc::new(Self {
            limits,
            poll_budget: limits.max_polls_per_tick.max(1),
            queue: Mutex::new(VecDeque::new()),
            registry: Mutex::new(Registry {
                live: HashMap::new(),
                next_id: 0,
                shutting_down: false,
            }),
            deferred: Mutex::new(Vec::new()),
            parked: AtomicBool::new(false),
            shutting_down: AtomicBool::new(false),
            drive: Mutex::new(()),
            driver,
        }))
    }

    /// Wakes the driving thread if it may be blocked in the driver.
    ///
    /// Callers publish their work (queue push under the queue lock, or a
    /// `SeqCst` flag store) before calling this; the driver stores `parked`
    /// before re-checking that work, so one side always sees the other.
    fn unpark(&self) {
        if self.parked.load(Ordering::SeqCst) {
            self.driver.unpark();
        }
    }

    fn schedule(&self, task: Arc<Task>) {
        lock(&self.queue).push_back(task);
        self.unpark();
    }

    fn spawn<F>(self: &Arc<Self>, future: F) -> Result<JoinHandle<F::Output>, SpawnError>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let join = Arc::new(JoinState {
            inner: Mutex::new(JoinInner {
                result: None,
                finished: false,
                waker: None,
            }),
        });
        let cell = Cell {
            future: Some(Box::pin(future)),
            staged: None,
            join: Arc::clone(&join),
        };
        let task = {
            let mut registry = lock(&self.registry);
            if registry.shutting_down {
                return Err(SpawnError::ShuttingDown);
            }
            if registry.live.len() >= self.limits.max_tasks {
                return Err(SpawnError::TaskLimitExceeded {
                    limit: self.limits.max_tasks,
                });
            }
            let id = registry.next_id;
            registry.next_id = registry.next_id.wrapping_add(1);
            let task = Arc::new(Task {
                id,
                shared: Arc::downgrade(self),
                state: AtomicU8::new(SCHEDULED),
                harness: Mutex::new(Some(Box::new(cell))),
            });
            registry.live.insert(id, Arc::clone(&task));
            task
        };
        self.schedule(Arc::clone(&task));
        Ok(JoinHandle { join, task })
    }

    /// Polls (or cancels) one scheduled task.
    fn run_task(&self, task: Arc<Task>) {
        let mut state = task.state.load(Ordering::Acquire);
        let cancelled = loop {
            if state & PHASE_MASK != SCHEDULED {
                return; // Finished or already claimed while queued.
            }
            let is_cancelled = state & CANCELLED != 0;
            let next = RUNNING | (state & CANCELLED);
            match task
                .state
                .compare_exchange_weak(state, next, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => break is_cancelled,
                Err(actual) => state = actual,
            }
        };
        let Some(mut harness) = lock(&task.harness).take() else {
            task.state.store(DONE, Ordering::Release);
            return;
        };
        if cancelled {
            harness.cancel();
            self.finish(&task, harness);
            return;
        }
        let waker = Waker::from(Arc::clone(&task));
        let mut cx = Context::from_waker(&waker);
        match harness.poll(&mut cx) {
            Poll::Ready(()) => self.finish(&task, harness),
            Poll::Pending => {
                *lock(&task.harness) = Some(harness);
                let mut state = RUNNING;
                loop {
                    if state & PHASE_MASK == DONE {
                        break;
                    }
                    let next = if state == RUNNING {
                        IDLE
                    } else {
                        // Woken (NOTIFIED) and/or aborted (CANCELLED) while
                        // running: preserve CANCELLED and re-queue.
                        SCHEDULED | (state & CANCELLED)
                    };
                    match task.state.compare_exchange_weak(
                        state,
                        next,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    ) {
                        Ok(_) => {
                            if next != IDLE {
                                self.schedule(Arc::clone(&task));
                            }
                            break;
                        }
                        Err(actual) => state = actual,
                    }
                }
            }
        }
    }

    /// Releases the task slot, then publishes the staged result.
    fn finish(&self, task: &Task, mut harness: Box<dyn Harness>) {
        task.state.store(DONE, Ordering::Release);
        lock(&self.registry).live.remove(&task.id);
        harness.publish();
        drop_harness_guarded(harness);
    }

    /// One scheduler tick: run up to `poll_budget` tasks, then turn the driver.
    fn tick(&self, root: &RootWaker) {
        for _ in 0..self.poll_budget {
            let next = lock(&self.queue).pop_front();
            match next {
                Some(task) => self.run_task(task),
                None => break,
            }
        }

        let has_work = |s: &Self| {
            root.woken.load(Ordering::SeqCst)
                || !lock(&s.queue).is_empty()
                || !lock(&s.deferred).is_empty()
        };
        let timeout = if has_work(self) {
            Some(Duration::ZERO)
        } else {
            self.parked.store(true, Ordering::SeqCst);
            if has_work(self) {
                Some(Duration::ZERO)
            } else {
                None
            }
        };
        let turned = self.driver.turn(timeout);
        self.parked.store(false, Ordering::SeqCst);
        if let Err(e) = turned {
            panic!("akku::task reactor wait failed: {e}");
        }

        let deferred = std::mem::take(&mut *lock(&self.deferred));
        for waker in deferred {
            waker.wake();
        }
    }

    /// Cancels every unfinished task and shuts the driver down.
    fn shutdown(&self) {
        self.shutting_down.store(true, Ordering::SeqCst);
        lock(&self.registry).shutting_down = true;
        loop {
            let live: Vec<Arc<Task>> = lock(&self.registry)
                .live
                .drain()
                .map(|(_, task)| task)
                .collect();
            let queued: Vec<Arc<Task>> = lock(&self.queue).drain(..).collect();
            if live.is_empty() && queued.is_empty() {
                break;
            }
            for task in live {
                task.state.store(DONE, Ordering::Release);
                let harness = lock(&task.harness).take();
                if let Some(mut harness) = harness {
                    // Dropping the future may run arbitrary destructors (even
                    // ones that wake or spawn); no runtime lock is held here.
                    harness.cancel();
                    harness.publish();
                    drop_harness_guarded(harness);
                }
            }
            drop(queued);
        }
        let deferred = std::mem::take(&mut *lock(&self.deferred));
        drop(deferred);
        self.driver.shutdown();
    }
}

// --- Tasks ------------------------------------------------------------------------

const IDLE: u8 = 0;
const SCHEDULED: u8 = 1;
const RUNNING: u8 = 2;
const NOTIFIED: u8 = 3; // Running, and woken or aborted again during the poll.
const DONE: u8 = 4;
const PHASE_MASK: u8 = 0x07;
/// Sticky cancellation bit folded into `Task::state` so `abort()` sets it and
/// transitions `IDLE -> SCHEDULED` / `RUNNING -> NOTIFIED` in one atomic RMW.
const CANCELLED: u8 = 1 << 3;

struct Task {
    id: u64,
    shared: Weak<Shared>,
    state: AtomicU8,
    harness: Mutex<Option<Box<dyn Harness>>>,
}

impl Task {
    fn schedule_if_idle(self: &Arc<Self>) {
        let mut state = self.state.load(Ordering::Acquire);
        loop {
            let phase = state & PHASE_MASK;
            let cancelled = state & CANCELLED;
            let next = match phase {
                IDLE => SCHEDULED | cancelled,
                RUNNING => NOTIFIED | cancelled,
                // Already queued, already re-poll pending, or finished.
                _ => return,
            };
            match self
                .state
                .compare_exchange_weak(state, next, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => {
                    if phase == IDLE {
                        if let Some(shared) = self.shared.upgrade() {
                            shared.schedule(Arc::clone(self));
                        }
                    }
                    return;
                }
                Err(actual) => state = actual,
            }
        }
    }

    fn abort(self: &Arc<Self>) {
        let mut state = self.state.load(Ordering::Acquire);
        loop {
            let phase = state & PHASE_MASK;
            if phase == DONE {
                return;
            }
            let next_phase = match phase {
                IDLE => SCHEDULED,
                RUNNING => NOTIFIED,
                // SCHEDULED or NOTIFIED: already queued or will be re-queued after poll.
                _ => phase,
            };
            let next = next_phase | CANCELLED;
            if next == state {
                return;
            }
            match self
                .state
                .compare_exchange_weak(state, next, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => {
                    if phase == IDLE {
                        if let Some(shared) = self.shared.upgrade() {
                            shared.schedule(Arc::clone(self));
                        }
                    }
                    return;
                }
                Err(actual) => state = actual,
            }
        }
    }
}

/// Drops a finished harness inside `catch_unwind`: if the task was detached,
/// this drops the final `Arc<JoinState<T>>` and therefore the task's output
/// (or panic payload), whose `Drop` must not unwind the runtime thread.
fn drop_harness_guarded(harness: Box<dyn Harness>) {
    if let Err(payload) = panic::catch_unwind(AssertUnwindSafe(move || drop(harness))) {
        let _ = panic::catch_unwind(AssertUnwindSafe(move || drop(payload)));
    }
}

impl Wake for Task {
    fn wake(self: Arc<Self>) {
        self.schedule_if_idle();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.schedule_if_idle();
    }
}

/// Type-erased task body. Every method catches panics from user code.
trait Harness: Send {
    /// Polls the future; on `Ready` the future is dropped and the result staged.
    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<()>;
    /// Drops the future and stages `Cancelled` (or `Panic` if its drop panics).
    fn cancel(&mut self);
    /// Hands the staged result to the `JoinHandle` and wakes its waiter.
    fn publish(&mut self);
}

struct Cell<F: Future> {
    future: Option<Pin<Box<F>>>,
    staged: Option<Result<F::Output, JoinError>>,
    join: Arc<JoinState<F::Output>>,
}

impl<F> Harness for Cell<F>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        let Some(future) = self.future.as_mut() else {
            return Poll::Ready(());
        };
        match panic::catch_unwind(AssertUnwindSafe(|| future.as_mut().poll(cx))) {
            Ok(Poll::Pending) => Poll::Pending,
            Ok(Poll::Ready(output)) => {
                let future = self.future.take();
                self.staged = Some(
                    match panic::catch_unwind(AssertUnwindSafe(|| drop(future))) {
                        Ok(()) => Ok(output),
                        Err(payload) => {
                            let _ = panic::catch_unwind(AssertUnwindSafe(|| drop(output)));
                            Err(JoinError::panic(payload))
                        }
                    },
                );
                Poll::Ready(())
            }
            Err(payload) => {
                // The future's state survived the unwind; drop it here, and
                // keep the first panic if its destructor panics too.
                let future = self.future.take();
                if let Err(second) = panic::catch_unwind(AssertUnwindSafe(|| drop(future))) {
                    let _ = panic::catch_unwind(AssertUnwindSafe(|| drop(second)));
                }
                self.staged = Some(Err(JoinError::panic(payload)));
                Poll::Ready(())
            }
        }
    }

    fn cancel(&mut self) {
        if let Some(future) = self.future.take() {
            self.staged = Some(
                match panic::catch_unwind(AssertUnwindSafe(|| drop(future))) {
                    Ok(()) => Err(JoinError::cancelled()),
                    Err(payload) => Err(JoinError::panic(payload)),
                },
            );
        }
    }

    fn publish(&mut self) {
        let result = self
            .staged
            .take()
            .unwrap_or_else(|| Err(JoinError::cancelled()));
        let waker = {
            let mut inner = lock(&self.join.inner);
            inner.result = Some(result);
            inner.finished = true;
            inner.waker.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

struct JoinState<T> {
    inner: Mutex<JoinInner<T>>,
}

struct JoinInner<T> {
    result: Option<Result<T, JoinError>>,
    finished: bool,
    waker: Option<Waker>,
}

// --- Thread-local context -------------------------------------------------------------

thread_local! {
    static CURRENT: RefCell<Option<Arc<Shared>>> = const { RefCell::new(None) };
}

fn current() -> Option<Arc<Shared>> {
    CURRENT
        .try_with(|current| current.borrow().clone())
        .ok()
        .flatten()
}

/// Restores the previous (empty) thread-local context, even on unwind.
struct EnterGuard;

impl Drop for EnterGuard {
    fn drop(&mut self) {
        let _ = CURRENT.try_with(|current| current.borrow_mut().take());
    }
}

#[track_caller]
fn enter(shared: &Arc<Shared>) -> EnterGuard {
    let nested = CURRENT.with(|current| {
        let mut current = current.borrow_mut();
        if current.is_some() {
            true
        } else {
            *current = Some(Arc::clone(shared));
            false
        }
    });
    if nested {
        panic!(
            "akku::task::block_on cannot be called from inside another block_on on the same thread"
        );
    }
    EnterGuard
}

/// Returns the I/O reactor of the runtime driving the current thread.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn current_reactor() -> io::Result<Arc<crate::net::reactor::Reactor>> {
    match current() {
        None => Err(io::Error::other(
            "no active akku::task runtime on this thread",
        )),
        Some(shared) if shared.shutting_down.load(Ordering::SeqCst) => {
            Err(io::Error::other("akku::task runtime is shutting down"))
        }
        Some(shared) => Ok(Arc::clone(&shared.driver)),
    }
}

/// Waker of the root `block_on` future.
struct RootWaker {
    woken: AtomicBool,
    shared: Weak<Shared>,
}

impl Wake for RootWaker {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        if !self.woken.swap(true, Ordering::SeqCst) {
            if let Some(shared) = self.shared.upgrade() {
                shared.unpark();
            }
        }
    }
}

// --- Public runtime API ------------------------------------------------------------------

/// A single-threaded async runtime driving spawned tasks and the I/O reactor.
///
/// Dropping a `Runtime` cancels and drops all remaining unfinished spawned tasks
/// and shuts down its I/O reactor.
pub struct Runtime {
    shared: Arc<Shared>,
}

impl Runtime {
    /// Creates a new runtime with [`Limits::default()`].
    ///
    /// Returns an `io::Error` if the underlying OS I/O reactor cannot be
    /// initialized (for example on `EMFILE`/`ENFILE`).
    pub fn new() -> io::Result<Self> {
        Self::with_limits(Limits::default())
    }

    /// Creates a new runtime with explicit [`Limits`].
    ///
    /// Returns an `io::Error` if the underlying OS I/O reactor cannot be
    /// initialized.
    pub fn with_limits(limits: Limits) -> io::Result<Self> {
        Ok(Self {
            shared: Shared::new(limits)?,
        })
    }

    /// Returns the active [`Limits`] for this runtime.
    pub fn limits(&self) -> Limits {
        self.shared.limits
    }

    /// Runs `future` to completion on the current thread, driving all spawned
    /// tasks and I/O events until `future` resolves.
    ///
    /// # Panics
    ///
    /// Panics if called from inside another active `block_on` on the same thread.
    #[track_caller]
    pub fn block_on<F: Future>(&self, future: F) -> F::Output {
        let _enter = enter(&self.shared);
        let _drive = lock(&self.shared.drive);
        let root = Arc::new(RootWaker {
            woken: AtomicBool::new(true),
            shared: Arc::downgrade(&self.shared),
        });
        let waker = Waker::from(Arc::clone(&root));
        let mut cx = Context::from_waker(&waker);
        let mut future = pin!(future);
        loop {
            if root.woken.swap(false, Ordering::SeqCst) {
                if let Poll::Ready(output) = future.as_mut().poll(&mut cx) {
                    return output;
                }
            }
            self.shared.tick(&root);
        }
    }

    /// Spawns a task onto this runtime.
    ///
    /// # Panics
    ///
    /// Panics if [`Limits::max_tasks`] is exceeded or the runtime is shutting down.
    /// Use [`Runtime::try_spawn`] for fallible spawning.
    #[track_caller]
    pub fn spawn<F>(&self, future: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        match self.try_spawn(future) {
            Ok(handle) => handle,
            Err(e) => panic!("akku::task::Runtime::spawn failed: {e}"),
        }
    }

    /// Fallible version of [`Runtime::spawn`], returning [`SpawnError`] instead
    /// of panicking.
    pub fn try_spawn<F>(&self, future: F) -> Result<JoinHandle<F::Output>, SpawnError>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.shared.spawn(future)
    }
}

impl fmt::Debug for Runtime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Runtime")
            .field("limits", &self.shared.limits)
            .finish_non_exhaustive()
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.shared.shutdown();
    }
}

/// Owned handle to a spawned task.
///
/// Awaiting a `JoinHandle<T>` resolves to `Result<T, JoinError>`. Dropping a
/// `JoinHandle<T>` **detaches** the task so it continues running until
/// completion or runtime shutdown.
#[must_use = "dropping a JoinHandle detaches the task; await it or call abort()"]
pub struct JoinHandle<T> {
    join: Arc<JoinState<T>>,
    task: Arc<Task>,
}

impl<T> JoinHandle<T> {
    /// Requests cancellation of the task.
    ///
    /// Safe to call multiple times or after the task has already completed (if
    /// the task already finished normally, awaiting the handle still returns
    /// `Ok(output)`).
    pub fn abort(&self) {
        self.task.abort();
    }

    /// Returns `true` once the task has finished (completed, panicked, or
    /// cancelled) and its future has been dropped.
    pub fn is_finished(&self) -> bool {
        lock(&self.join.inner).finished
    }
}

impl<T> Future for JoinHandle<T> {
    type Output = Result<T, JoinError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut inner = lock(&self.join.inner);
        if let Some(result) = inner.result.take() {
            return Poll::Ready(result);
        }
        assert!(
            !inner.finished,
            "JoinHandle polled again after it returned its result"
        );
        match &inner.waker {
            Some(w) if w.will_wake(cx.waker()) => {}
            _ => inner.waker = Some(cx.waker().clone()),
        }
        Poll::Pending
    }
}

impl<T> fmt::Debug for JoinHandle<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JoinHandle")
            .field("finished", &self.is_finished())
            .finish_non_exhaustive()
    }
}

/// Runs `future` to completion on a fresh single-threaded [`Runtime`] with
/// default limits.
///
/// # Panics
///
/// Panics if called nested inside another `block_on` on the same thread or if
/// the OS reactor cannot be created.
#[track_caller]
pub fn block_on<F: Future>(future: F) -> F::Output {
    match try_block_on(future) {
        Ok(output) => output,
        Err(e) => panic!("akku::task::block_on could not create a runtime: {e}"),
    }
}

/// Fallible counterpart to [`block_on`], returning `io::Error` if runtime or
/// reactor initialization fails.
///
/// # Panics
///
/// Panics if called nested inside another `block_on` on the same thread.
#[track_caller]
pub fn try_block_on<F: Future>(future: F) -> io::Result<F::Output> {
    if current().is_some() {
        panic!(
            "akku::task::block_on cannot be called from inside another block_on on the same thread"
        );
    }
    let runtime = Runtime::new()?;
    Ok(runtime.block_on(future))
}

/// Spawns `future` onto the current thread's active [`Runtime`].
///
/// # Panics
///
/// Panics if called outside an active runtime, if the runtime is shutting down,
/// or if [`Limits::max_tasks`] is exceeded.
#[track_caller]
pub fn spawn<F>(future: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    match try_spawn(future) {
        Ok(handle) => handle,
        Err(SpawnError::NoRuntime) => {
            panic!("akku::task::spawn called outside of an akku::task runtime (use block_on)")
        }
        Err(e) => panic!("akku::task::spawn failed: {e}"),
    }
}

/// Fallible counterpart to [`spawn`], returning [`SpawnError`] instead of
/// panicking.
pub fn try_spawn<F>(future: F) -> Result<JoinHandle<F::Output>, SpawnError>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    current().ok_or(SpawnError::NoRuntime)?.spawn(future)
}

/// Cooperatively yields execution back to the runtime so other ready tasks and
/// I/O events can make progress before this task is polled again.
pub fn yield_now() -> YieldNow {
    YieldNow { yielded: false }
}

/// Future returned by [`yield_now`].
///
/// Yields `Poll::Pending` once (deferring its wakeup past the current reactor
/// turn when running inside a [`Runtime`]) and then resolves to `()`.
#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct YieldNow {
    yielded: bool,
}

impl Future for YieldNow {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.yielded {
            return Poll::Ready(());
        }
        self.yielded = true;
        match current() {
            Some(shared) => lock(&shared.deferred).push(cx.waker().clone()),
            None => cx.waker().wake_by_ref(),
        }
        Poll::Pending
    }
}

impl fmt::Debug for YieldNow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("YieldNow").finish_non_exhaustive()
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod unit_tests {
    use super::*;
    use std::time::Instant;

    struct Signal {
        fired: AtomicBool,
        waker: Mutex<Option<Waker>>,
    }

    impl Signal {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                fired: AtomicBool::new(false),
                waker: Mutex::new(None),
            })
        }

        fn fire(&self) {
            self.fired.store(true, Ordering::SeqCst);
            if let Some(waker) = lock(&self.waker).take() {
                waker.wake();
            }
        }
    }

    struct WaitSignal(Arc<Signal>);

    impl Future for WaitSignal {
        type Output = ();

        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
            if self.0.fired.load(Ordering::SeqCst) {
                return Poll::Ready(());
            }
            *lock(&self.0.waker) = Some(cx.waker().clone());
            if self.0.fired.load(Ordering::SeqCst) {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        }
    }

    struct PendingWithDrop {
        polled: Arc<AtomicBool>,
        dropped: Arc<AtomicBool>,
    }

    impl Future for PendingWithDrop {
        type Output = ();

        fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<()> {
            self.polled.store(true, Ordering::Release);
            Poll::Pending
        }
    }

    impl Drop for PendingWithDrop {
        fn drop(&mut self) {
            self.dropped.store(true, Ordering::Release);
        }
    }

    #[test]
    fn cross_thread_abort_never_loses_cancellation() {
        const ITERATIONS: usize = 1_000;
        let rt = Runtime::with_limits(Limits::new().with_max_tasks(1)).unwrap();
        let stop = Signal::new();
        let stop_driver = Arc::clone(&stop);

        std::thread::scope(|scope| {
            let aborter = scope.spawn(|| {
                for i in 0..ITERATIONS {
                    let polled = Arc::new(AtomicBool::new(false));
                    let dropped = Arc::new(AtomicBool::new(false));
                    let mut handle = match rt.try_spawn(PendingWithDrop {
                        polled: Arc::clone(&polled),
                        dropped: Arc::clone(&dropped),
                    }) {
                        Ok(h) => h,
                        Err(e) => {
                            stop.fire();
                            panic!("iteration {i}: try_spawn failed unexpectedly: {e}");
                        }
                    };

                    if i % 2 == 1 {
                        let spin_deadline = Instant::now() + Duration::from_secs(5);
                        while !polled.load(Ordering::Acquire) {
                            if Instant::now() >= spin_deadline {
                                stop.fire();
                                panic!("iteration {i}: task was never polled");
                            }
                            std::thread::yield_now();
                        }
                    }

                    handle.abort();

                    let deadline = Instant::now() + Duration::from_secs(5);
                    while !handle.is_finished() {
                        if Instant::now() >= deadline {
                            stop.fire();
                            panic!("iteration {i}: cross-thread abort lost (timed out)");
                        }
                        std::thread::yield_now();
                    }

                    assert!(
                        dropped.load(Ordering::Acquire),
                        "iteration {i}: cancelled future was not dropped before finish"
                    );

                    let mut cx = Context::from_waker(Waker::noop());
                    match Pin::new(&mut handle).poll(&mut cx) {
                        Poll::Ready(Err(err)) => assert!(
                            err.is_cancelled(),
                            "iteration {i}: expected cancelled JoinError, got {err}"
                        ),
                        other => {
                            stop.fire();
                            panic!("iteration {i}: unexpected JoinHandle poll result: {other:?}");
                        }
                    }
                }
                stop.fire();
            });

            rt.block_on(WaitSignal(stop_driver));
            aborter.join().unwrap();
        });
    }

    #[test]
    fn detached_output_drop_panic_does_not_unwind_runtime() {
        struct PanicOutput(Arc<AtomicBool>);

        impl Drop for PanicOutput {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
                panic!("detached output drop boom");
            }
        }

        let dropped = Arc::new(AtomicBool::new(false));
        let dropped_in_task = Arc::clone(&dropped);
        let rt = Runtime::new().unwrap();

        let answer = rt.block_on(async move {
            // Drop the JoinHandle immediately so the output drops on the runtime thread.
            drop(spawn(async move { PanicOutput(dropped_in_task) }));
            // Wait until the detached task has finished and run its output's Drop.
            let follow_up = spawn(async { 42usize });
            follow_up.await.unwrap()
        });

        assert_eq!(answer, 42);
        assert!(dropped.load(Ordering::SeqCst));

        // Runtime must still be healthy for subsequent work and shutdown.
        let after = rt.block_on(async { spawn(async { 7usize }).await.unwrap() });
        assert_eq!(after, 7);
    }

    #[test]
    fn shutdown_detached_panicking_payload_drop_does_not_unwind() {
        struct PanicPayload;

        impl Drop for PanicPayload {
            fn drop(&mut self) {
                if !std::thread::panicking() {
                    panic!("panic payload drop boom");
                }
            }
        }

        struct PanicFutureOnDrop;

        impl Future for PanicFutureOnDrop {
            type Output = ();

            fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<()> {
                Poll::Pending
            }
        }

        impl Drop for PanicFutureOnDrop {
            fn drop(&mut self) {
                panic::panic_any(PanicPayload);
            }
        }

        let rt = Runtime::new().unwrap();
        drop(rt.spawn(PanicFutureOnDrop));
        drop(rt);
    }
}
