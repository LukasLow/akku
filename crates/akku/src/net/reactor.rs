//! Safe, platform-independent I/O reactor shared by `akku::task` and the
//! async socket types.
//!
//! The reactor owns one OS poller (`sys::Poller`) and a generational slab of
//! registered sources. Each source tracks per-direction readiness, a readiness
//! tick (so a `WouldBlock` never clears readiness that a newer event set), and
//! the wakers waiting on each direction: one slot for streams (`&mut self`
//! I/O and the split halves allow one reader and one writer), or a bounded,
//! deduplicated set for listeners, which accept through `&self` and may be
//! shared by several tasks.
//!
//! Every readiness wait spends the current poll's cooperative budget
//! (`crate::task::poll_budget`), so an always-ready socket cannot keep one
//! task inside a single poll forever.
//!
//! Sockets join the OS poller lazily, on their first `Pending` wait: `epoll`
//! `EPOLL_CTL_ADD` and `kqueue` `EV_ADD` both report readiness that already
//! exists at registration time, so no edge is lost, and sockets that never
//! block cost no poller syscalls (this also sidesteps XNU's per-socket knote
//! list, whose removal is linear in the knotes attached to that socket).

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{ready, Context, Poll, Waker};
use std::time::Duration;

use super::sys::{self, Interest, ReadyEvent};

/// Internal cap on concurrently registered sources per reactor.
pub(crate) const MAX_REGISTRATIONS: usize = 65_536;

/// Upper clamp of `Limits::max_io_events_per_tick`: no turn can report more
/// ready sources than there are registrations.
pub(crate) const MAX_EVENTS_PER_TURN: usize = MAX_REGISTRATIONS;

/// Upper clamp of the per-direction waiter set of a shared source (the
/// default `max_tasks` plus the root future).
const MAX_WAITERS: usize = 65_537;

const READ: usize = 0;
const WRITE: usize = 1;

/// Readiness direction of a registered source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Direction {
    Read,
    Write,
}

impl Direction {
    fn index(self) -> usize {
        match self {
            Self::Read => READ,
            Self::Write => WRITE,
        }
    }

    fn interest(self) -> Interest {
        Interest {
            readable: self == Self::Read,
            writable: self == Self::Write,
        }
    }
}

fn lock<T: ?Sized>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) fn shutting_down_error() -> io::Error {
    io::Error::other("akku::task runtime is shutting down")
}

/// Who may wait on one direction of a source at the same time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Waiters {
    /// One poller per direction (streams): a new waker replaces the old one.
    Single,
    /// Any number of tasks through `&self` (listeners): every distinct waker
    /// is kept and all are woken on readiness.
    Shared,
}

/// The wakers parked on one direction. The first lives inline, so a stream
/// never allocates; further distinct wakers (shared sources only) spill into
/// `rest`.
#[derive(Default)]
struct WakerSet {
    first: Option<Waker>,
    rest: Vec<Waker>,
}

impl WakerSet {
    fn len(&self) -> usize {
        usize::from(self.first.is_some()) + self.rest.len()
    }

    fn contains(&self, waker: &Waker) -> bool {
        self.first
            .iter()
            .chain(&self.rest)
            .any(|w| w.will_wake(waker))
    }

    /// Parks `waker`. In [`Waiters::Shared`] mode a waker that is already
    /// parked (by `Waker::will_wake`) is not added again, so the set holds at
    /// most one waker per polling task. If the set is full (`cap`), every
    /// parked waker moves to `overflow` to be woken spuriously, which is
    /// always safe: each re-polls and parks again.
    fn park(&mut self, waker: &Waker, mode: Waiters, cap: usize, overflow: &mut Vec<Waker>) {
        match mode {
            Waiters::Single => match &self.first {
                Some(w) if w.will_wake(waker) => {}
                _ => self.first = Some(waker.clone()),
            },
            Waiters::Shared => {
                if self.contains(waker) {
                    return;
                }
                if self.len() >= cap {
                    self.drain_into(overflow);
                }
                if self.first.is_none() {
                    self.first = Some(waker.clone());
                } else {
                    self.rest.push(waker.clone());
                }
            }
        }
    }

    /// Removes `waker` again (it is waking up with an error instead).
    fn remove(&mut self, waker: &Waker) {
        if self.first.as_ref().is_some_and(|w| w.will_wake(waker)) {
            self.first = self.rest.pop();
        }
        self.rest.retain(|w| !w.will_wake(waker));
    }

    /// Moves every parked waker into `out` (keeping `rest`'s capacity).
    fn drain_into(&mut self, out: &mut Vec<Waker>) {
        out.extend(self.first.take());
        out.append(&mut self.rest);
    }
}

/// Per-registration readiness state.
struct ScheduledIo {
    token: u64,
    waiters: Waiters,
    state: Mutex<IoState>,
}

struct IoState {
    ready: [bool; 2],
    tick: [u64; 2],
    wakers: [WakerSet; 2],
    shutdown: bool,
    /// Whether the socket has been added to the OS poller yet.
    in_poller: bool,
}

impl ScheduledIo {
    /// Marks the given directions ready and returns the wakers to wake.
    fn set_ready(&self, read: bool, write: bool, wake: &mut Vec<Waker>) {
        let mut state = lock(&self.state);
        for (dir, hit) in [(READ, read), (WRITE, write)] {
            if hit {
                state.ready[dir] = true;
                state.tick[dir] = state.tick[dir].wrapping_add(1);
                state.wakers[dir].drain_into(wake);
            }
        }
    }

    fn shutdown(&self, wake: &mut Vec<Waker>) {
        let mut state = lock(&self.state);
        state.shutdown = true;
        for set in &mut state.wakers {
            set.drain_into(wake);
        }
    }
}

struct SlabEntry {
    generation: u32,
    io: Option<Arc<ScheduledIo>>,
}

/// Generational slab: token = `(generation << 32) | index`, `generation >= 1`,
/// so token `0` (the poller wakeup) is never a source.
#[derive(Default)]
struct Slab {
    entries: Vec<SlabEntry>,
    free: Vec<usize>,
    len: usize,
}

impl Slab {
    fn insert(&mut self, initial_ready: bool, waiters: Waiters) -> io::Result<Arc<ScheduledIo>> {
        if self.len >= MAX_REGISTRATIONS {
            return Err(io::Error::other(format!(
                "reactor registration limit reached ({MAX_REGISTRATIONS} sockets)"
            )));
        }
        let index = match self.free.pop() {
            Some(index) => index,
            None => {
                self.entries.push(SlabEntry {
                    generation: 1,
                    io: None,
                });
                self.entries.len() - 1
            }
        };
        let entry = &mut self.entries[index];
        let token = (u64::from(entry.generation) << 32) | index as u64;
        let io = Arc::new(ScheduledIo {
            token,
            waiters,
            state: Mutex::new(IoState {
                ready: [initial_ready; 2],
                tick: [0; 2],
                wakers: [WakerSet::default(), WakerSet::default()],
                shutdown: false,
                in_poller: false,
            }),
        });
        entry.io = Some(Arc::clone(&io));
        self.len += 1;
        Ok(io)
    }

    fn get(&self, token: u64) -> Option<Arc<ScheduledIo>> {
        let index = usize::try_from(token & u64::from(u32::MAX)).ok()?;
        let generation = (token >> 32) as u32;
        let entry = self.entries.get(index)?;
        if entry.generation != generation {
            return None; // Stale event for a closed (and maybe reused) slot.
        }
        entry.io.clone()
    }

    fn remove(&mut self, token: u64) {
        let Ok(index) = usize::try_from(token & u64::from(u32::MAX)) else {
            return;
        };
        let generation = (token >> 32) as u32;
        if let Some(entry) = self.entries.get_mut(index) {
            if entry.generation == generation && entry.io.take().is_some() {
                entry.generation = entry.generation.wrapping_add(1).max(1);
                self.free.push(index);
                self.len -= 1;
            }
        }
    }
}

/// One runtime's I/O reactor.
pub(crate) struct Reactor {
    poller: sys::Poller,
    events: Mutex<Vec<ReadyEvent>>,
    slab: Mutex<Slab>,
    shutdown: AtomicBool,
    /// Bound of a [`Waiters::Shared`] direction's waker set.
    max_waiters: usize,
}

impl Reactor {
    /// Creates the OS poller; `max_events` bounds events drained per turn
    /// (clamped to `1..=MAX_EVENTS_PER_TURN`), and `max_waiters` bounds the
    /// waker set of each shared source direction (clamped to
    /// `1..=MAX_WAITERS`).
    pub(crate) fn new(max_events: usize, max_waiters: usize) -> io::Result<Self> {
        let max_events = max_events.clamp(1, MAX_EVENTS_PER_TURN);
        Ok(Self {
            poller: sys::Poller::new(max_events)?,
            events: Mutex::new(Vec::with_capacity(max_events)),
            slab: Mutex::new(Slab::default()),
            shutdown: AtomicBool::new(false),
            max_waiters: max_waiters.clamp(1, MAX_WAITERS),
        })
    }

    /// Waits for OS events (at most `timeout`; `None` blocks until an event or
    /// [`Reactor::unpark`]) and wakes the affected tasks.
    pub(crate) fn turn(&self, timeout: Option<Duration>) -> io::Result<()> {
        let mut events = std::mem::take(&mut *lock(&self.events));
        events.clear();
        let waited = self.poller.wait(&mut events, timeout);
        let mut wake = Vec::new();
        for event in &events {
            let io = lock(&self.slab).get(event.token);
            if let Some(io) = io {
                let read = event.readable || event.read_closed || event.error;
                let write = event.writable || event.write_closed || event.error;
                io.set_ready(read, write, &mut wake);
            }
        }
        events.clear();
        *lock(&self.events) = events;
        crate::task::wake_all(wake);
        waited
    }

    /// Interrupts a blocking [`Reactor::turn`] from any thread.
    pub(crate) fn unpark(&self) {
        self.poller.unpark();
    }

    /// Fails every pending and future I/O wait with a shutdown error.
    pub(crate) fn shutdown(&self) {
        let sources: Vec<Arc<ScheduledIo>> = {
            let slab = lock(&self.slab);
            self.shutdown.store(true, Ordering::SeqCst);
            slab.entries
                .iter()
                .filter_map(|entry| entry.io.clone())
                .collect()
        };
        let mut wake = Vec::new();
        for io in sources {
            io.shutdown(&mut wake);
        }
        crate::task::wake_all(wake);
    }
}

/// A socket's registration with a [`Reactor`].
///
/// Every method that may add the socket to the OS poller takes the socket's
/// borrowed handle, so the poller only ever sees open sockets. The owner must
/// call [`Registration::deregister`] before closing the socket; dropping the
/// registration only releases the slab slot.
pub(crate) struct Registration {
    reactor: Arc<Reactor>,
    io: Arc<ScheduledIo>,
    registered: AtomicBool,
}

impl Registration {
    /// Reserves a reactor slot for a socket (counted against the
    /// registration limit); the OS poller sees it on the first wait.
    ///
    /// `initial_ready` seeds both directions as ready (optimistic, for sockets
    /// that may already have data); `false` waits for the first OS event.
    /// `waiters` says whether several tasks may wait on one direction.
    pub(crate) fn new(
        reactor: Arc<Reactor>,
        initial_ready: bool,
        waiters: Waiters,
    ) -> io::Result<Self> {
        let io = {
            let mut slab = lock(&reactor.slab);
            if reactor.shutdown.load(Ordering::SeqCst) {
                return Err(shutting_down_error());
            }
            slab.insert(initial_ready, waiters)?
        };
        Ok(Self {
            reactor,
            io,
            registered: AtomicBool::new(true),
        })
    }

    pub(crate) fn reactor(&self) -> &Arc<Reactor> {
        &self.reactor
    }

    /// Removes the socket from the OS poller and frees its slot (idempotent).
    pub(crate) fn deregister(&self, handle: sys::Handle<'_>) -> io::Result<()> {
        if !self.registered.swap(false, Ordering::AcqRel) {
            return Ok(());
        }
        let in_poller = std::mem::replace(&mut lock(&self.io.state).in_poller, false);
        let result = if in_poller {
            self.reactor.poller.deregister(handle, self.io.token)
        } else {
            Ok(())
        };
        lock(&self.reactor.slab).remove(self.io.token);
        result
    }

    /// Returns the current readiness tick once `dir` is ready, or registers
    /// `cx`'s waker, adds `handle` to the OS poller if needed, and arms it.
    ///
    /// Spends one unit of the cooperative budget when it returns `Ready`
    /// (readiness or error); with the budget exhausted it returns `Pending`
    /// and schedules a deferred wakeup instead, keeping readiness intact.
    pub(crate) fn poll_ready(
        &self,
        cx: &mut Context<'_>,
        dir: Direction,
        handle: sys::Handle<'_>,
    ) -> Poll<io::Result<u64>> {
        ready!(crate::task::poll_budget(cx));
        let result = ready!(self.poll_ready_unbudgeted(cx, dir, handle));
        crate::task::consume_budget();
        Poll::Ready(result)
    }

    fn poll_ready_unbudgeted(
        &self,
        cx: &mut Context<'_>,
        dir: Direction,
        handle: sys::Handle<'_>,
    ) -> Poll<io::Result<u64>> {
        let d = dir.index();
        let mut overflow = Vec::new();
        {
            let mut state = lock(&self.io.state);
            if state.shutdown || self.reactor.shutdown.load(Ordering::SeqCst) {
                return Poll::Ready(Err(shutting_down_error()));
            }
            if state.ready[d] {
                return Poll::Ready(Ok(state.tick[d]));
            }
            state.wakers[d].park(
                cx.waker(),
                self.io.waiters,
                self.reactor.max_waiters,
                &mut overflow,
            );
            if !state.in_poller {
                // Under the state lock, so the read and write halves never
                // add the socket twice.
                if let Err(e) = self.reactor.poller.register(handle, self.io.token) {
                    state.wakers[d].remove(cx.waker());
                    drop(state);
                    crate::task::wake_all(overflow);
                    return Poll::Ready(Err(e));
                }
                state.in_poller = true;
            }
        }
        // Wakers evicted from a full shared set re-poll and park again.
        crate::task::wake_all(overflow);
        self.reactor.poller.arm(self.io.token, dir.interest());
        Poll::Pending
    }

    /// Clears readiness of `dir` unless an event arrived after `tick`.
    pub(crate) fn clear_ready(&self, dir: Direction, tick: u64) {
        let d = dir.index();
        let mut state = lock(&self.io.state);
        if state.tick[d] == tick {
            state.ready[d] = false;
        }
    }

    fn tick(&self, dir: Direction) -> u64 {
        lock(&self.io.state).tick[dir.index()]
    }

    /// Runs `op` once `dir` is ready, retrying `Interrupted` and turning
    /// `WouldBlock` into a readiness wait.
    ///
    /// Every `Ready` result (data, EOF, or any error such as `EMFILE`) spends
    /// one unit of the cooperative budget; with the budget exhausted this
    /// returns `Pending` without running `op` (see [`Registration::poll_ready`]).
    pub(crate) fn poll_io<R>(
        &self,
        cx: &mut Context<'_>,
        dir: Direction,
        handle: sys::Handle<'_>,
        mut op: impl FnMut() -> io::Result<R>,
    ) -> Poll<io::Result<R>> {
        ready!(crate::task::poll_budget(cx));
        let result = loop {
            let tick = match ready!(self.poll_ready_unbudgeted(cx, dir, handle)) {
                Ok(tick) => tick,
                Err(e) => break Err(e),
            };
            match op() {
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => self.clear_ready(dir, tick),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                result => break result,
            }
        };
        crate::task::consume_budget();
        Poll::Ready(result)
    }

    /// Runs `op` immediately; on `WouldBlock` clears readiness so the next
    /// poll waits for a fresh OS event.
    pub(crate) fn try_io<R>(
        &self,
        dir: Direction,
        mut op: impl FnMut() -> io::Result<R>,
    ) -> io::Result<R> {
        loop {
            let tick = self.tick(dir);
            match op() {
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    self.clear_ready(dir, tick);
                    return Err(e);
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                result => return result,
            }
        }
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        if self.registered.swap(false, Ordering::AcqRel) {
            lock(&self.reactor.slab).remove(self.io.token);
        }
    }
}
