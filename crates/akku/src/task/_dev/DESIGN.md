# task — design record

## Purpose

`akku::task` is akku's zero-dependency async task executor and runtime. Today `crates/akku/Cargo.toml` pulls `tokio` (and its transitive tree) only to drive `akku::http`, while Wave 2 networking (`akku::net`, `akku::dns`, `akku::nfs`, `akku::quic`) and the planned zero-dependency `akku::http` rewrite all need a single native runtime (`docs/forks/tokio-axum.md` §3, §8).

**Phase 1 scope (approved in `docs/forks/tokio-axum.md` §8–§9):** ship the minimal single-threaded, **100% safe** (`0` `unsafe` blocks) executor — `Runtime`, `block_on`, `try_block_on`, `spawn`, `try_spawn`, `JoinHandle`, `JoinError`, `SpawnError`, `Limits`, and `yield_now` — built on `Arc<Task>: std::task::Wake` and integrated with the internal `akku::net` I/O reactor across all five Tier-1 OSes (`epoll` on Linux/Android, `kqueue` on macOS/iOS, `WSAPoll` on Windows; safe `Condvar` parker only on non-Tier-1 targets without a native OS reactor). Alongside bundled `akku::io`, this immediately unblocks native async TCP in `akku::net` on every Tier-1 platform and lays a forward-compatible public surface for Phase 3 timers (`akku::time`), Phase 3 async channels (`akku::sync`), and Phase 4 multi-threaded work-stealing + `spawn_blocking` + `#[akku::task::main]`.

## Non-goals

- **No `unsafe` anywhere in `akku::task`.** Human decision (`docs/forks/tokio-axum.md` §9.1) and `AGENTS.md` rule 9: no manual `RawTask` vtables, no intrusive `NonNull` linked lists, no lock-free `UnsafeCell` Chase-Lev deques. `std::task::Wake` over `Arc<Task>` + `std::sync::Mutex` is 100% safe and fast enough for I/O workloads.
- **No multi-threaded scheduler or `spawn_blocking` pool in Phase 1.** Deferred to Phase 4 (`_dev/TODO.md`), which adds per-worker `Mutex<VecDeque<Arc<Task>>>` + `try_lock` half-batch stealing and a bounded blocking pool behind the same `Runtime` / `spawn` / `block_on` API.
- **No timers (`sleep`, `sleep_until`, `interval`, `timeout`) in `akku::task`.** Human decision (`docs/forks/tokio-axum.md` §9.5): timers live in `akku::time` (Phase 3), driven by an internal timer wheel hook inside the runtime parker.
- **No `#[akku::task::main]` proc macro in Phase 1.** Deferred to Phase 4 (`akku-drive` addition) once multi-threaded `Runtime` lands; `akku::task::block_on(async { ... })` is pleasant without macros (`AGENTS.md` rule 7).
- **No `LocalSet` / `!Send` `spawn_local` in Phase 1.** Requiring `F: Future + Send + 'static` on `spawn` from day one avoids a breaking signature change when Phase 4 introduces multi-threaded execution. Non-`Send` local spawning is tracked in `_dev/TODO.md`.
- **No task-local storage, runtime metrics/tracing hooks, or cooperative poll-budget macros (`tokio::task::coop`) in Phase 1.** Fairness is enforced by `Limits::max_polls_per_tick` and deferred `yield_now()`; instrumentation belongs to `akku::trace` (Wave 3).
- **No external runtime dependencies (`tokio`, `async-task`, `concurrent-queue`, `parking_lot`).** Pure `std` only.
- **No `wasm32` browser executor bridge in Phase 1.** `wasm32` `spawn` via microtasks/promises is planned for Phase 7 (`docs/forks/tokio-axum.md` §8); in Phase 1 `akku::task` compiles on `wasm32` (with pure `block_on` only or `Unsupported` for OS-blocking park) without compromising native paths (`CONCEPT.md` §6).

## Namespace

`akku::task` — flat level-1 module, already listed in [`Namespace.md`](../../../../../Namespace.md) under *Async, system, CLI & development*:

> `akku::task` | Async tasks, runtime, cancellation (not `async`; timers and `timeout` live in `akku::time`)

The `Namespace.md` entries for `task`, `time`, `io`, and `#[akku::task::main]` already match the human's 2026-11-25 decisions (`docs/forks/tokio-axum.md` §9.5); no edit to `Namespace.md` is needed.

Proposed Phase 1 public items under `akku::task`:
- `Runtime`, `Limits`
- `block_on`, `try_block_on`, `spawn`, `try_spawn`, `yield_now`, `YieldNow`
- `JoinHandle<T>`, `JoinError`, `SpawnError`

No name clashes: `task` is not a Rust keyword (`AGENTS.md` rule 6), does not shadow `::core`, and all types stay inside `akku::task::*` (`Namespace.md` rule 2).

## Equivalents

| Ecosystem | Item |
|---|---|
| Go runtime | Goroutines (`go f()` -> `runtime.newproc`), cooperative/preemptive scheduler (`runtime/proc.go`), `runtime.Gosched()` (yield), `context.WithCancel` (cancellation); no explicit `block_on` or `JoinHandle` (`sync.WaitGroup` + channels instead) |
| Rust std | `std::future::Future`, `std::task::{Context, Poll, Wake, Waker}`, `std::thread::{spawn, JoinHandle, yield_now}` (OS threads, no async executor) |
| `tokio` | `tokio::runtime::Runtime` (`Builder::new_current_thread`), `Runtime::block_on`, `tokio::spawn`, `tokio::task::{JoinHandle, JoinError, yield_now}` |
| `async-std` | `async_std::task::{block_on, spawn, yield_now, JoinHandle}` (note: `async-std` is discontinued; panics propagate rather than returning `Result<T, JoinError>`) |
| `smol` | `smol::block_on`, `smol::spawn`, `async_executor::{Executor, LocalExecutor}`, `async_task::Task` (cancels on drop unless `.detach()`ed), `futures_lite::future::yield_now` |
| `mio` | None (pure I/O readiness layer; no task executor) |
| `polling` | None (pure OS poller; `async-io` pairs it with `async-executor`) |

## Dependencies

`native`: zero external dependencies (`std` only, `#![deny(unsafe_code)]`, **0 `unsafe` blocks**).

**Internal coupling with `akku::net` (Phase 1):**
- `akku::task::Runtime` owns an internal `pub(crate)` I/O reactor driver (`crate::net::reactor::Driver` / `ReactorHandle`) so `block_on` parks directly in the native OS reactor on all five Tier-1 platforms (`epoll_wait` on Linux/Android, `kevent` on macOS/iOS, `WSAPoll` on Windows) without spawning a hidden background reactor thread.
- On non-Tier-1 targets where no native OS reactor is compiled in (exotic Unix or `wasm32`), `Runtime` falls back to a safe `std::sync::{Mutex, Condvar}` parker (`0` `unsafe`).

## References

Pinned to the commits read on 2026-11-25.

- **Rust `std` (`rust-lang/rust`)** `rust-lang/rust@bc9132daa5c0be572ebb81304f6039d947298e67`
  - `library/alloc/src/task.rs:95-123`: `std::task::Wake` trait (stabilized in Rust 1.51) and `impl<W: Wake + Send + Sync + 'static> From<Arc<W>> for Waker`, constructing a `Waker` from `Arc<W>` with zero extra heap allocations and zero caller `unsafe`.
  - `library/alloc/src/task.rs:108-110`: default `Wake::wake_by_ref` clones the `Arc` and calls `wake`; overriding `wake_by_ref` avoids redundant `Arc` refcount traffic when a task is already scheduled (waker-storm coalescing).
- **Tokio (`tokio-rs/tokio`)** `tokio-rs/tokio@854013a8cb7d1397e9d037315592dc1d2d55187d`
  - `tokio/src/runtime/scheduler/current_thread/mod.rs:423-461,689-693`: `parked: AtomicBool` RMW `swap(true, AcqRel)` / `swap(false, AcqRel)` protocol between `Context::park` and `Handle::unpark` to prevent lost wakeups while coalescing redundant driver unparks.
  - `tokio/src/runtime/scheduler/current_thread/mod.rs:484-507`: `has_pending_work` checks local queue, deferred (`yield_now`) list, root `woken` flag, and remote `inject` queue before deciding whether to block in `driver.park` or do a zero-timeout `park_timeout(0)` turn; `self.defer.wake()` runs after the driver turn.
  - `tokio/src/runtime/scheduler/current_thread/mod.rs:866-923`: `CoreGuard::block_on` event loop polls the root `future` when `reset_woken()` is true, runs at most `event_interval` (default 61) spawned tasks per tick, then yields to `park_yield` so I/O readiness is never starved by CPU-bound or self-waking tasks.
  - `tokio/src/runtime/scheduler/current_thread/mod.rs:293-324`: `shutdown2` cancels/drops all remaining owned tasks, drains local and remote queues, and shuts down the resource drivers cleanly on runtime drop.
  - `tokio/src/task/yield_now.rs:38-60`: `yield_now()` registers `cx.waker()` into `context::defer` on first poll (waking immediately if polled outside a Tokio runtime) so the task does not immediately re-enter the front/back of the current turn before I/O runs.
  - `tokio/src/runtime/task/join.rs:18-36,227-236`: `JoinHandle<T>` detaches on drop (task keeps running), exposes `abort()` and `is_finished()`, and returns `Result<T, JoinError>` on `.await`.
  - `tokio/src/runtime/task/error.rs:9-133,171-178`: `JoinError` distinguishes `Cancelled` vs `Panic(Box<dyn Any + Send + 'static>)`, implements `Display`/`Error`/`From<JoinError> for io::Error`, and exposes `is_cancelled`, `is_panic`, `into_panic`, `try_into_panic`. Tokio uses `SyncWrapper` (`unsafe impl Sync`) at `:17` to make `JoinError: Sync`; akku achieves `Sync` safely via `Mutex<Option<Box<dyn Any + Send + 'static>>>`.
  - `tokio/src/runtime/task/harness.rs:502-569`: wraps both `future.poll(cx)` and `drop(future)` (on poll panic or cancellation) inside `std::panic::catch_unwind(AssertUnwindSafe(...))` so a panic in a spawned task or its `Drop` never corrupts or unwinds the executor thread.
- **smol / `async-executor` (`smol-rs/async-executor`)** `smol-rs/async-executor@a75cad623bd5c6c80ab19a5befb3d9b93a79bfcd`
  - `src/lib.rs:169-174,251-285`: tracks active tasks in a `Mutex<Slab<Waker>>`, removes each task on completion via `AsyncCallOnDrop`, and drains/wakes all active tasks in `Executor::drop` (`:412-431`).
  - `src/lib.rs:280`: sets `.propagate_panic(true)` on `async-task`, contrasting with Tokio's `Result<T, JoinError>` containment; `async_task::Task` also cancels on drop unless `.detach()` is called.
  - `src/lib.rs:708-723`: coalesces sleeper notifications with an `AtomicBool` `notified` CAS before acquiring the sleepers mutex.
  - `src/lib.rs:750-758`: caps consecutive task polls (`200`) before yielding in `State::run` to avoid starving the root future.
- **`async-std` (`async-rs/async-std`)** `async-rs/async-std@844b552531521f517a2b02c4ed685ae176608247`
  - `src/task/mod.rs:19-23,36-57`: documents `block_on`, `spawn`, `yield_now`, and `JoinHandle<T>` (whose `.await` yields `T` directly and propagates panics rather than returning `Result<T, JoinError>`).
- **Go runtime (`golang/go`)** `golang/go@d695060f5be23f2dd86d21d861a292dc2d183cab`
  - `src/runtime/proc.go:400-407`: `runtime.Gosched()` yields the current goroutine back to the global run queue so other goroutines can progress.
  - `src/runtime/proc.go:5381-5386`: `newproc` spawns a goroutine onto the scheduler run queue (detached by default; panics inside a goroutine crash the process unless `recover()`d).
  - `src/runtime/netpoll_epoll.go:81-103`: `netpollBreak` uses an `atomic.Uint32` CAS (`netpollWakeSig`) so concurrent wakes coalesce into a single `eventfd` write.

## Public API

Signatures and contracts for Phase 1 (no implementation). All public items live under `akku::task`.

### DoS limits (`Limits`)

```rust
/// Resource and fairness bounds for an [`akku::task::Runtime`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Maximum number of live (unfinished) spawned tasks per runtime.
    /// Default: `65_536`.
    pub max_tasks: usize,
    /// Maximum spawned-task polls executed before yielding to the I/O reactor
    /// and checking the root `block_on` future.
    /// Default: `64`.
    pub max_polls_per_tick: usize,
    /// Maximum OS I/O events drained per reactor wait turn.
    /// Default: `1_024`.
    pub max_io_events_per_tick: usize,
}

impl Default for Limits { /* defaults above */ }

impl Limits {
    pub const fn new() -> Self;
    pub const fn with_max_tasks(self, max: usize) -> Self;
    pub const fn with_max_polls_per_tick(self, max: usize) -> Self;
    pub const fn with_max_io_events_per_tick(self, max: usize) -> Self;
}
```

Values of `0` for `max_polls_per_tick` or `max_io_events_per_tick` are clamped internally to `1` so a runtime can never livelock on a misconfigured limit (`max_tasks = 0` validly forbids all `spawn` calls while still allowing `block_on`).

### Error types (`JoinError`, `SpawnError`)

```rust
/// Error returned when awaiting a [`JoinHandle`] whose task did not complete normally.
///
/// Implements `fmt::Display`, `fmt::Debug`, `std::error::Error`, `Send`, `Sync`,
/// and `From<JoinError> for std::io::Error`.
pub struct JoinError { /* private: Repr::Cancelled | Repr::Panic(Mutex<Option<Box<dyn Any + Send + 'static>>>) */ }

impl JoinError {
    /// Returns `true` if the task was cancelled via [`JoinHandle::abort`] or runtime shutdown.
    pub fn is_cancelled(&self) -> bool;
    /// Returns `true` if the task terminated because its future (or destructor) panicked.
    pub fn is_panic(&self) -> bool;
    /// Consumes the error and returns the panic payload.
    ///
    /// # Panics
    /// Panics if `!self.is_panic()`.
    #[track_caller]
    pub fn into_panic(self) -> Box<dyn std::any::Any + Send + 'static>;
    /// Consumes the error and returns the panic payload if `self.is_panic()`, or `Err(self)` otherwise.
    pub fn try_into_panic(self) -> Result<Box<dyn std::any::Any + Send + 'static>, Self>;
}

/// Error returned by [`try_spawn`] or [`Runtime::try_spawn`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SpawnError {
    /// Called outside an active [`Runtime`] / [`block_on`] context.
    NoRuntime,
    /// The runtime is shutting down and no longer accepts new tasks.
    ShuttingDown,
    /// Live task count reached [`Limits::max_tasks`].
    TaskLimitExceeded { limit: usize },
}
// Implements `fmt::Display`, `std::error::Error`, and `From<SpawnError> for std::io::Error`.
```

### Runtime, task handles, and free functions

```rust
/// A single-threaded (Phase 1) async runtime driving spawned tasks and the I/O reactor.
pub struct Runtime { /* private */ }

impl Runtime {
    /// Creates a new runtime with [`Limits::default()`].
    /// Fails with `io::Error` if the underlying OS I/O reactor (`epoll_create1`/`eventfd` on Linux/Android,
    /// `kqueue` on macOS/iOS, loopback wake socketpair on Windows) cannot be initialized
    /// (for example on `EMFILE`/`ENFILE`).
    pub fn new() -> std::io::Result<Self>;

    /// Creates a new runtime with explicit [`Limits`].
    pub fn with_limits(limits: Limits) -> std::io::Result<Self>;

    /// Returns the active [`Limits`] for this runtime.
    pub fn limits(&self) -> Limits;

    /// Runs `future` to completion on the current thread, driving all spawned tasks
    /// and I/O events until `future` resolves.
    ///
    /// # Panics
    /// Panics if called from inside another active `block_on` on the same thread.
    #[track_caller]
    pub fn block_on<F: std::future::Future>(&self, future: F) -> F::Output;

    /// Spawns a task onto this runtime.
    ///
    /// # Panics
    /// Panics if `Limits::max_tasks` is exceeded. Use [`Runtime::try_spawn`] for fallible spawning.
    #[track_caller]
    pub fn spawn<F>(&self, future: F) -> JoinHandle<F::Output>
    where
        F: std::future::Future + Send + 'static,
        F::Output: Send + 'static;

    /// Fallible version of [`Runtime::spawn`], returning [`SpawnError`] instead of panicking.
    pub fn try_spawn<F>(&self, future: F) -> Result<JoinHandle<F::Output>, SpawnError>
    where
        F: std::future::Future + Send + 'static,
        F::Output: Send + 'static;
}
// `Drop` cancels and drops all remaining spawned tasks and shuts down the I/O reactor.

/// Owned handle to a spawned task.
///
/// Awaiting a `JoinHandle<T>` resolves to `Result<T, JoinError>`.
/// Dropping a `JoinHandle<T>` detaches the task (it continues running until completion
/// or runtime shutdown).
pub struct JoinHandle<T> { /* private */ }
// Implements `Future<Output = Result<T, JoinError>>`, `Unpin`, `fmt::Debug`,
// `Send` (when `T: Send`), `Sync` (when `T: Send`).

impl<T> JoinHandle<T> {
    /// Requests cancellation of the task. Safe to call multiple times or after completion.
    pub fn abort(&self);

    /// Returns `true` once the task has finished (completed, panicked, or cancelled)
    /// and its future has been dropped.
    pub fn is_finished(&self) -> bool;
}

/// Runs `future` to completion on a fresh single-threaded [`Runtime`] with default limits.
///
/// # Panics
/// Panics if called nested inside another `block_on` or if the OS reactor cannot be created.
#[track_caller]
pub fn block_on<F: std::future::Future>(future: F) -> F::Output;

/// Fallible counterpart to [`block_on`], returning `io::Error` if runtime/reactor init fails.
///
/// # Panics
/// Panics if called nested inside another `block_on`.
#[track_caller]
pub fn try_block_on<F: std::future::Future>(future: F) -> std::io::Result<F::Output>;

/// Spawns `future` onto the current thread's active [`Runtime`].
///
/// # Panics
/// Panics if called outside a runtime or if [`Limits::max_tasks`] is exceeded.
#[track_caller]
pub fn spawn<F>(future: F) -> JoinHandle<F::Output>
where
    F: std::future::Future + Send + 'static,
    F::Output: Send + 'static;

/// Fallible counterpart to [`spawn`].
pub fn try_spawn<F>(future: F) -> Result<JoinHandle<F::Output>, SpawnError>
where
    F: std::future::Future + Send + 'static,
    F::Output: Send + 'static;

/// Cooperatively yields execution back to the runtime so other ready tasks and I/O
/// events can make progress before this task is polled again.
pub fn yield_now() -> YieldNow;

/// Future returned by [`yield_now`]. Implements `Future<Output = ()> + Unpin + Send + Sync`.
pub struct YieldNow { /* private */ }
```

### Safe internal executor sketch (0 `unsafe`)

1. **`Arc<Task>: std::task::Wake`:**
   - Each task holds `state: AtomicU8` (bits: `SCHEDULED`, `RUNNING`, `CANCELLED`, `COMPLETED`), a `Weak<RuntimeInner>` (or `Arc<RuntimeInner>` dropped on completion so runtime drop breaks cycles), and `future: Mutex<Option<Box<dyn TaskFuture + Send>>>` where `TaskFuture` is a small private trait exposing `poll_and_store(&mut self, cx: &mut Context<'_>) -> Poll<()>` and `cancel_and_store(&mut self)`.
   - `Task::wake_by_ref` atomically inspects `state`: if `COMPLETED`, returns immediately; if already `SCHEDULED`, returns immediately (O(1) waker-storm coalescing); if `RUNNING`, sets `SCHEDULED` so the worker re-enqueues after `poll()` returns `Pending`; if `IDLE`, transitions `IDLE -> SCHEDULED`, pushes `Arc::clone(self)` onto the run queue, and calls `runtime.unpark()` (coalesced by `parked: AtomicBool`).
   - **Never hold locks across `poll` or `drop`:** the executor takes the `Box<dyn TaskFuture>` out of `task.future` before polling or dropping it (preventing re-entrant `Mutex` deadlocks if a future wakes itself, spawns a child, or drops its own `JoinHandle` during `poll`/`drop`), wraps both `poll` and `drop` in `panic::catch_unwind(AssertUnwindSafe(...))`, and only puts the future back if `Poll::Pending` and not `CANCELLED`.
2. **Event loop (`Runtime::block_on`):**
   - Stores an `Arc<RuntimeInner>` in a thread-local guard (`Drop` restores previous state even if the root future panics).
   - Loop:
     1. If the root future's `woken` `AtomicBool` is set, poll the root future (`Pin<&mut F>`). If `Ready(out)`, return `out`.
     2. Drain up to `limits.max_polls_per_tick` tasks from the FIFO `VecDeque<Arc<Task>>`.
     3. Flush/check I/O: if there is still ready work, deferred `yield_now` wakers, or `root_woken`, do a **non-blocking** reactor turn (`timeout = Some(Duration::ZERO)`), wake deferred `yield_now` wakers, and continue; otherwise set `parked = true`, re-check pending work (preventing lost wakeups per Tokio `current_thread/mod.rs:423-444`), and park in the reactor (`timeout = None`) until an I/O event or OS wakeup (`eventfd` on Linux/Android, `EVFILT_USER` on macOS/iOS, loopback wake socket on Windows, or `Condvar` on non-Tier-1 targets) arrives, then clear `parked = false` and wake any deferred wakers.
   - When `Runtime` is dropped, mark `shutting_down = true`, drain the live-task registry, cancel and drop every unfinished task (inside `catch_unwind`), drain the run queue, and shut down the I/O reactor.

## API rationale

- **100% safe `Arc<Task>: Wake` + `Mutex` task cell.** Follows `docs/forks/tokio-axum.md` §5B and §9.1 (`0` `unsafe`). Rust 1.51 stabilized `From<Arc<W>> for Waker` (`library/alloc/src/task.rs:114-123`), giving zero-`unsafe` task wakers with only one extra `Box` per `spawn` (~20 ns), negligible next to socket I/O.
- **`JoinHandle<T>` returns `Result<T, JoinError>` and detaches on drop.** Matches `tokio::task::JoinHandle` (`tokio/src/runtime/task/join.rs:18-36,90-108`) rather than `smol`'s cancel-on-drop or `async-std`'s panic-propagation:
  - Catching panics in spawned tasks isolates one misbehaving client connection from tearing down an entire `akku::http` or `akku::nfs` server, while letting callers inspect/resume the panic via `JoinError::into_panic()`.
  - Detaching on drop matches the ubiquitous `akku::task::spawn(handle_conn(stream));` server loop without requiring `.detach()` on every spawn. (Decided 2026-11-25, see §Decisions.)
- **`JoinError: Send + Sync` via `Mutex<Option<Box<dyn Any + Send>>>` instead of `unsafe SyncWrapper`.** Achieves the exact same `Send + Sync + Error` bounds as Tokio (`tokio/src/runtime/task/error.rs:17`) with zero `unsafe`.
- **`spawn` requires `Send + 'static` in Phase 1.** Even though Phase 1 is single-threaded, Phase 4 promotes `Runtime` to multi-threaded work-stealing. Keeping `Send + 'static` from Phase 1 guarantees zero breaking changes in Phase 4 (`spawn_local` is a separate additive API in `_dev/TODO.md`).
- **Both `spawn` and `try_spawn` (plus `block_on` and `try_block_on`).** Ergonomic panicking defaults (`block_on`, `spawn`) match Rust ecosystem muscle memory, while fallible `Runtime::new`, `try_block_on`, and `try_spawn` let servers enforce `Limits::max_tasks` backpressure and let G3 adversarial tests verify `EMFILE` / task-limit behavior without unwinding.
- **Deferred `yield_now()` + `max_polls_per_tick = 64`.** Mirrors Tokio (`tokio/src/task/yield_now.rs:49-55`, `current_thread/mod.rs:881`). A naive `yield_now` that calls `cx.waker().wake_by_ref()` immediately causes a hot `loop { yield_now().await }` task to spin the run queue and starve I/O; deferring until after a reactor turn guarantees fairness.
- **Per-`Runtime` reactor rather than a hidden global thread.** Unlike `smol`/`async-io`'s global `OnceLock<Reactor>` + background `"async-io"` thread (`smol-rs/async-io` `src/reactor.rs:93-107`), tying the reactor to `Runtime` means `block_on` parks directly in the native OS reactor (`epoll_wait` / `kevent` / `WSAPoll`) with zero extra OS threads, deterministic cleanup on `Runtime::drop`, and zero cross-test interference in `tests.rs`.
- **`akku::io` bundled into Phase 1 alongside `akku::task` and `akku::net`.** Per human decision (2026-11-25), `akku::io` ships in Phase 1 with 100%-safe `&mut [u8]` async traits (`crates/akku/src/io/_dev/DESIGN.md`). `akku::task` itself stays decoupled from `akku::io` (executor only), while `akku::net::AsyncTcpStream` implements `akku::io::{AsyncRead, AsyncWrite}`.
- **All five Tier-1 OSes run natively in Phase 1 (`AGENTS.md` rule 11).** Linux/Android (`epoll`), macOS/iOS (`kqueue`), and Windows (`WSAPoll`) all ship native reactor backends in Phase 1 with no Tier-1 `Unsupported` stubs, so `ubuntu-latest`, `macos-latest`, and `windows-latest` CI all run the complete `akku::task` + `akku::net` + `akku::io` test suites.

## Seams

Public boundaries targeted by `crates/akku/src/task/tests.rs` (G3):

| Seam | Catches | Misses |
|---|---|---|
| `block_on` / `try_block_on` with immediate, multi-poll, and cross-thread woken futures | missed wakeups, `parked` flag race, root future not re-polled | multi-thread work-stealing (Phase 4) |
| `spawn` / `try_spawn` + `JoinHandle::await` (single, fan-out, nested spawns, return values) | lost tasks, FIFO ordering bugs, return-value drop leaks | — |
| `JoinHandle::drop` (detach) vs `JoinHandle::abort` (cancel) + `is_finished` | accidental cancel-on-drop, zombie tasks after `abort`, future `Drop` not run on cancel | — |
| `yield_now` across competing tasks and outside a runtime | starvation of sibling tasks, missing deferred wake, panic when polled outside `akku::task` | — |
| `Runtime::drop` with unfinished/blocked spawned tasks | leaked tasks, `Arc` reference cycles, hanging destructors | — |
| `Limits` (`max_tasks`, `max_polls_per_tick`, `max_io_events_per_tick`) | off-by-one cap, zero-limit livelock, task counter leak after completion/panic/cancel | — |
| `JoinError` & `SpawnError` (`Display`, `Debug`, `Error`, `Send + Sync`, `From<JoinError> for io::Error`) | broken trait bounds, lost panic message, wrong `is_panic`/`is_cancelled` | — |

### Adversarial test list for G3

Covering every applicable class in `akku-tdd` and the Phase 1 prompt requirements:

1. **Panic inside a spawned task:**
   - Spawn a task that panics with `&'static str`, formatted `String`, and a non-string custom payload (`panic_any(42u32)`). Verify the runtime does **not** unwind, sibling tasks continue to completion, `JoinHandle::await` returns `Err(e)` with `e.is_panic() == true`, `e.is_cancelled() == false`, `e.try_into_panic()` recovers the payload, and `live_tasks` decrements cleanly.
   - Detached panicking task (dropped `JoinHandle`): must not crash the runtime or poison internal mutexes; subsequent spawns succeed.
   - Panic inside a future's `Drop` impl during both poll-panic unwinding and `JoinHandle::abort()`: must be caught, reported as `JoinError::is_panic()`, and leave the runtime healthy.
2. **Drop-cancellation & `abort()`:**
   - Drop a future awaiting a `JoinHandle` mid-flight (e.g., via custom `poll` race) — the spawned task stays detached and finishes; polling `&mut JoinHandle` again later still observes the result (cancel-safe).
   - Call `handle.abort()` before first poll, while `Pending`, after completion (must still return `Ok(val)`), and multiple times idempotently; verify an `Arc<AtomicBool>` drop guard inside the cancelled future runs promptly and `is_finished()` becomes `true`.
   - Drop `Runtime` while spawned tasks are still pending — verify drop guards inside all pending tasks run before `Runtime::drop` returns (no `Arc` cycle leak).
3. **Waker storms & starvation:**
   - Custom future whose `poll` clones `cx.waker()` and calls `wake_by_ref()` 10,000 times from the same thread and from 4 background `std::thread`s concurrently before returning `Pending`. Verify bounded memory (no 10,000 duplicate queue entries — `SCHEDULED` bit coalesces to 1 entry) and timely completion.
   - Self-waking spin task (`loop { yield_now().await }`) running alongside a sibling task and a cross-thread woken task: verify `max_polls_per_tick` + deferred `yield_now` guarantees the sibling and root future still complete promptly (no starvation).
   - Re-entrant wake during `poll()`: a task calls `cx.waker().wake_by_ref()` while executing `poll()` and returns `Pending` — must not deadlock on `Mutex` and must be polled again on a subsequent turn.
   - Spurious wakeups after task completion or cancellation: holding a cloned `Waker` and calling `.wake()` after the task finished/aborted must be a safe no-op and not leak memory or re-enqueue.
4. **Limits & DoS bounds:**
   - `Limits::new().with_max_tasks(4)`: spawning 4 live tasks succeeds; 5th `try_spawn` returns `Err(SpawnError::TaskLimitExceeded { limit: 4 })`; once 1 finishes/cancels/panics, spawning another succeeds (verifies exact slot accounting).
   - `Limits::new().with_max_tasks(0)`: `try_spawn` returns `TaskLimitExceeded { limit: 0 }`; `block_on(async { 42 })` still succeeds.
   - `Limits::new().with_max_polls_per_tick(0)` / `with_max_io_events_per_tick(0)`: clamped to `1`, does not hang.
5. **Re-entrancy & lifecycle misuse:**
   - Calling `block_on` nested inside `block_on` panics cleanly (and the outer `block_on` thread-local guard unwinds cleanly so a subsequent top-level `block_on` on the same thread works).
   - Calling `try_spawn` outside any runtime returns `Err(SpawnError::NoRuntime)`; calling `spawn` outside any runtime panics clearly.
   - Cross-thread `Runtime::spawn` / `Waker::wake` from a `std::thread` while the runtime thread is parked in `block_on` wakes the runtime without lost-wakeup hangs (stress-tested over 500 iterations to expose `parked` RMW races).

## Decisions (human, 2026-11-25)

1. **Bundle `akku::io` into Phase 1:** ship flat `akku::io` (full `std::io` superset including sync `copy` + safe `&mut [u8]` `AsyncRead`, `AsyncWrite`, `AsyncBufRead`, and `Async*Ext` methods `read`/`read_exact`/`copy_to`/`write`/`write_all`/`flush`/`shutdown`) in Phase 1; `akku::net::AsyncTcpStream` and its split halves implement these traits (`crates/akku/src/io/_dev/DESIGN.md`).
2. **Platform tiers apply to Phase 1 (`AGENTS.md` rule 11):** `akku::net` async TCP and the `akku::task` reactor integration ship native backends for all five Tier-1 targets in Phase 1 with no `Unsupported` stubs — `epoll` on Linux/Android (`net/sys/epoll.rs`), `kqueue` on macOS/iOS (`net/sys/kqueue.rs`), and `WSAPoll` on Windows (`net/sys/wsapoll.rs`, with `IOCP`+`AFD` as a later performance follow-up).
3. **`JoinHandle` semantics:** detach on drop, plus explicit `abort()`.
4. **API surface:** expose `Runtime`, `Limits`, `try_block_on`, and `try_spawn`; `spawn` requires `Send + 'static`; runtime-dependent fallible functions (`akku::net` async constructors, `try_spawn` via `SpawnError::NoRuntime` / `From<SpawnError> for io::Error`) return `Err(io::Error)` / `Err(...)` with no active runtime instead of panicking; dropping `AsyncTcpOwnedWriteHalf` calls `shutdown(Shutdown::Write)` and ignores errors.
