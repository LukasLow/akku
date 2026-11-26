# sync — design record

Status: G1 proposal + G2 API (2026-11-26), decided by Orcha with an independent Opus review (§Decisions (G2)). Nothing is implemented. G1 and G2 land together on branch `propose/sync`. G3/G4 are split into two PRs on `lib/sync` (§G2 "Delivery: PR-A and PR-B").

## Purpose

`akku::sync` is the `std::sync` superset with akku's async synchronization primitives. It is Phase 3 of the runtime port (`docs/forks/tokio-axum.md` §3 "Sync primitives" row, §8 Phase 3). It is needed because:

- the shipped runtime (`akku::task`, `akku::io`, `akku::net`) has no way for tasks to coordinate: no async lock, no permit pool, no channel. Every server built on `akku::net` (the native `akku::http` rewrite in Phase 5/6, `akku::nfs`, `akku::dns`) needs bounded connection pools (`Semaphore`), shared mutable state across `.await` (`AsyncMutex`, `AsyncRwLock`), request/response hand-off (`oneshot`), work queues (`channel`), shutdown fan-out (`broadcast`, `Notify`), and config reload (`watch`);
- `akku::http` still depends on `tokio`. Phase 6 deletes `tokio` from `Cargo.toml`, and `tokio::sync` is part of what has to be replaced (`docs/forks/tokio-axum.md` §8, Phase 3 row "Replaces `tokio::time`, `tokio::sync`, `parking_lot*`, `once_cell`");
- users should find `Arc`, `Mutex`, `OnceLock`, `mpsc`, and the async primitives in one place: `use akku::sync::{Arc, AsyncMutex, Semaphore};` (`Namespace.md` §3).

Hard constraints from the human (`docs/forks/tokio-axum.md` §9 decisions 1 and 9, `AGENTS.md` rules 8, 9, 11):

- zero dependencies and **zero `unsafe`**. Tokio's intrusive waiter lists need `unsafe` (`tokio/src/sync/batch_semaphore.rs:772`, `tokio/src/sync/notify.rs:1354`), so akku needs a safe waiter queue with the same cancellation guarantees (§G2 "Safe waiter queue");
- every primitive is `Send + Sync` (when its payload allows) and correct under the future Phase 4 multi-threaded executor, not only under today's single-threaded `akku::task` runtime (`crates/akku/src/task/_dev/DESIGN.md` §Non-goals);
- runtime-agnostic: the primitives use only `std::task::Waker`, so they work under `akku::task`, under a foreign executor, and from plain threads through `blocking_*` methods.

## Non-goals

- **No `unsafe`, no intrusive pointer lists, no lock-free queues.** All shared state sits behind one `std::sync::Mutex` per primitive, as `docs/forks/tokio-axum.md` §5B row "`tokio/src/sync/{mpsc,oneshot,mutex,rwlock,broadcast,once_cell}`" plans. Atomic fast paths (safe `AtomicUsize` CAS for an uncontended `try_acquire`) are a later, benchmark-driven optimization (`_dev/TODO.md`).
- **No poisoning on async locks.** A panic while an `AsyncMutexGuard` is held unlocks the lock (matches `tokio/src/sync/mutex.rs:122-126`). The re-exported `std::sync::Mutex` keeps its poisoning unchanged.
- **No `?Sized` payloads in `AsyncMutex<T>` / `AsyncRwLock<T>`.** `const fn new` stores `T` inline, which needs `T: Sized` (§G2 "Guards without `unsafe`"). Use `AsyncMutex<Box<dyn Trait>>`.
- **No mapped guards** (`MappedMutexGuard`, `RwLockMappedWriteGuard`), no `downgrade`, no upgradable reads in v1. Mapped guards remain possible later as a guard plus a stored `fn(&mut T) -> &mut U` projection re-applied on every deref (`_dev/TODO.md`).
- **No timers.** `send_timeout`, `recv_timeout`, and `acquire_timeout` are not methods. Compose with `akku::time::timeout(d, rx.recv())` (Phase 3, `Namespace.md` family `akku::time`). Blocking `recv_timeout` is in `_dev/TODO.md`.
- **No `select!`/`join!` macros.** They belong to `akku::task` (`crates/akku/src/task/_dev/TODO.md` "Cancellation & structured concurrency").
- **No actor mailboxes, pub/sub brokers, or cross-process primitives.** Those are `akku::actor`, `akku::broker`, and `akku::cluster` (`Namespace.md`).
- **No re-export of unstable `std::sync` items** (`Exclusive`, `ReentrantLock`, `ReentrantLockGuard`, `MappedMutexGuard`, `MappedRwLockReadGuard`, `MappedRwLockWriteGuard`, `mpmc`, `poison`, `nonpoison`, `oneshot`, `UniqueArc`). They are re-exported once they are stable and the MSRV covers them (`_dev/TODO.md`).
- **No `AsyncBarrier`, `AsyncCondvar`, `WaitGroup`, or cancellation token in v1** (`_dev/TODO.md`, Decisions Q11). §G2 "Patterns" shows how latched shutdown and connection draining work with v1 primitives until those exist.

## Namespace

`akku::sync` is a flat level-1 module, already listed in `Namespace.md` §"Async, system, CLI & development":

> `akku::sync` | `std::sync` superset + `AsyncMutex`, `AsyncRwLock`, `AsyncOnceCell`, `Semaphore`, `Notify`; channel modules `channel` (async MPMC), `oneshot`, `broadcast`, `watch`

(Row updated in this PR, together with the new `Namespace.md` §3 rules (d) and (e).)

`Namespace.md` §3 rules applied:

1. **`std::sync` names are kept exactly.** Every stable item of `std::sync` at the MSRV (Rust 1.85.0) is re-exported by explicit list (§G2 "`std::sync` re-exports"). No akku item uses any of these names.
2. **Async types get the `Async` prefix when `std::sync` has the base name:** `AsyncMutex` (std `Mutex`), `AsyncRwLock` (std `RwLock`), `AsyncOnceCell` (std has `OnceLock` and `core::cell::OnceCell`; the prefix keeps the async cell visibly different from both). Their guards follow the `akku::net` split-half pattern (`AsyncTcpStream` → `AsyncTcpOwnedWriteHalf`): `AsyncMutexGuard`, `AsyncMutexOwnedGuard`, `AsyncRwLockReadGuard`, `AsyncRwLockWriteGuard`, `AsyncRwLockOwnedReadGuard`, `AsyncRwLockOwnedWriteGuard`.
3. **Names `std::sync` lacks stay plain:** `Semaphore`, `SemaphorePermit`, `SemaphoreOwnedPermit`, `AcquireError`, `TryAcquireError`, `Notify`, `Notified`, and the modules `oneshot`, `broadcast`, `watch` (human decision `docs/forks/tokio-axum.md` §9.9).
4. **Async MPSC never shadows `std::sync::mpsc`.** Proposed name: **`akku::sync::channel`**, an async **MPMC** channel (`channel::bounded`, `channel::unbounded`, `channel::Sender`, `channel::Receiver`). It also avoids `mpmc`, which is an unstable `std::sync` module in current stable Rust (`rust-lang/rust@0ef1bc386059596dada8fb65b50425aab2ea637a` `library/std/src/sync/mod.rs:192-194`). `std::sync` has no item named `channel` (only `std::sync::mpsc::channel`, one level down). Alternatives considered: `chan` (Go-flavoured, less discoverable), `queue` (suggests a non-blocking data structure), `mpsc`/`mpmc` (forbidden by `Namespace.md` §3 (e)). The module carries `#[doc(alias = "mpsc")]` so rustdoc search for `mpsc` finds it. Note on namespaces: the **module** `akku::sync::channel` lives in the type/module namespace, while the **functions** `oneshot::channel` and `broadcast::channel` (and `watch::channel`) live in the value namespace of their own modules, so `use akku::sync::{channel, oneshot};` followed by `channel::bounded(8)` and `oneshot::channel()` never conflicts. Decided (Q1).
5. **Blocking counterparts use a `blocking_` prefix** (`blocking_lock`, `blocking_recv`, `blocking_send`, `blocking_acquire`), never a `_blocking` suffix or `sync_` (tokio naming, `tokio/src/sync/mutex.rs:520`). This is the new `Namespace.md` §3 rule (d), added in this PR (Q9).
6. **Recorded exception: `oneshot`.** Current stable Rust has an *unstable* `std::sync::oneshot` (`#[unstable(feature = "oneshot_channel", issue = "143674")]`, `rust-lang/rust@0ef1bc38…` `library/std/src/sync/mod.rs:195-196`, `library/std/src/sync/oneshot.rs:36-37`). The new `Namespace.md` §3 rule (e) forbids taking names of `std` items even when they are unstable, and records `akku::sync::oneshot` as the one exception: once `std::sync::oneshot` is stable and within the MSRV, `akku::sync::oneshot` becomes a `std` superset and its async halves take the `Async` prefix (a `0.0.x` breaking change). Decided (Q2).

No `Namespace.md` §5 reserved generic names are used at level 1. `sync` is not a Rust keyword and does not shadow `::core`.

## Equivalents

| akku | Rust `std` | tokio 1.54.1 | async-lock 3.4.2 / event-listener 5.4.2 | futures-channel 0.3.34 | flume 0.12 | async-channel 2.5.0 | Go (`sync`, `chan`) | Python `asyncio` |
|---|---|---|---|---|---|---|---|---|
| `std::sync` re-exports | `std::sync::*` | — | — | — | — | — | `sync.Mutex`, `sync.RWMutex`, `sync.Once`, `sync.Cond`, `sync/atomic` | — |
| `AsyncMutex` | `Mutex` (blocking) | `sync::Mutex` | `Mutex` | `lock::BiLock` (2 owners only) | — | — | `sync.Mutex` (blocking) | `Lock` |
| `AsyncRwLock` | `RwLock` (blocking) | `sync::RwLock` | `RwLock` | — | — | — | `sync.RWMutex` | — |
| `Semaphore` | — | `sync::Semaphore` | `Semaphore` | — | — | — | `x/sync/semaphore.Weighted`, buffered `chan struct{}` | `Semaphore` |
| `Notify` | `Condvar` (blocking) | `sync::Notify` | `event_listener::Event` | — | — | — | `sync.Cond` | `Event`, `Condition` |
| `AsyncOnceCell` | `OnceLock` (blocking init) | `sync::OnceCell` | `OnceCell` | — | — | — | `sync.Once`, `sync.OnceValue` | — |
| `oneshot` | (unstable `oneshot`) | `sync::oneshot` | — | `oneshot` | `bounded(1)` | `bounded(1)` | `make(chan T, 1)` | `Future` |
| `channel` (MPMC) | `mpsc::{channel, sync_channel}` (MPSC, blocking) | `sync::mpsc` (MPSC) | — | `mpsc` (MPSC) | `bounded`, `unbounded` (MPMC) | `bounded`, `unbounded` (MPMC) | `chan T` (MPMC) | `Queue` |
| `broadcast` | — | `sync::broadcast` | `async-broadcast` 0.7.2 (separate crate) | — | — | — | — | — |
| `watch` | — | `sync::watch` | — | — | — | — | — | — |

### References

Pinned to the sources read on 2026-11-26. crates.io sources are pinned to the commit recorded in each crate's `.cargo_vcs_info.json`.

- **Rust 1.85.0 (`rust-lang/rust@4d91de4e48198da2e33413efdcd9cd2cc0c46688`, tag `1.85.0`)**
  - `library/std/src/sync/mod.rs:174-224`: the complete `std::sync` export list at the MSRV. Stable: `atomic` (`:177`), `Arc`, `Weak` (`:180`), `Barrier`, `BarrierWaitResult` (`:186`), `LazyLock` (`:188`, since 1.80), `OnceLock` (`:190`, since 1.70), `LockResult`, `PoisonError` (`:197`), `Mutex`, `MutexGuard`, `TryLockError`, `TryLockResult`, `Condvar`, `WaitTimeoutResult`, `Once`, `OnceState`, `RwLock`, `RwLockReadGuard`, `RwLockWriteGuard` (`:208-213`), deprecated `ONCE_INIT` (`:214-217`), `mpsc` (`:224`). Unstable: `Exclusive` (`:174-175`), `ReentrantLock`, `ReentrantLockGuard` (`:191-192`), mapped guards (`:218-220`), `mpmc` (`:222-223`), `poison` (`:226-227`).
  - `library/std/src/sync/mpsc/mod.rs:525,575`: `mpsc::channel` and `mpsc::sync_channel(bound)`; `sync_channel(0)` is a "rendezvous channel" (`:540`), the precedent for `channel::bounded(0)`.
  - `library/core/src/task/wake.rs:567-569`: `Waker::noop()` is stable since 1.85.0 (used by G3 tests to poll futures by hand). `:480`: `Waker::will_wake`.
  - `library/alloc/src/sync.rs:1111-1112`: `Arc::into_inner` is stable since 1.70.0 (used by `AsyncRwLock::into_inner`; the write guard uses `Arc::get_mut`, stable since 1.4.0).
- **Rust current stable 1.100.0 (`rust-lang/rust@0ef1bc386059596dada8fb65b50425aab2ea637a`)**
  - `library/std/src/sync/mod.rs:182-252`: the stable list is unchanged since 1.85; new unstable items are `SyncView` (`:182-183`), `UniqueArc` (`:187-188`), `mpmc` (`:192-193`), `oneshot` (`:195-196`), `nonpoison` (`:227-228`).
  - `library/std/src/sync/oneshot.rs:36-37,119,209,250,283,341,389`: unstable blocking `oneshot::{channel, Sender::send, Receiver::recv, try_recv, recv_timeout, TryRecvError<T>, RecvTimeoutError<T>}`.
- **tokio 1.54.1 (`tokio-rs/tokio@b53b2001ed2ccfd9b87e32a0c6da1a1eebdc80b8`, path `tokio/`)**
  - `src/sync/mod.rs:440-446`: sync primitives are runtime-agnostic and take part in cooperative scheduling only inside a tokio runtime. akku copies this split.
  - `src/sync/batch_semaphore.rs:1-13`: one intrusive FIFO list of waiters; permits go to the front waiter first, which makes the semaphore fair. `:137`: `MAX_PERMITS = usize::MAX >> 3`. `:334`: `add_permits_locked` hands released permits to queued waiters. `:609-612`: `assign_permits` gives the head waiter *partial* permits, so a large `acquire_many` at the head blocks later small requests (no barging). `:659-667`: `poll_acquire` calls `coop::poll_proceed` and `made_progress`. `:758-779`: `Drop for Acquire` unlinks the node (`unsafe { waiters.queue.remove(node) }` at `:772`) and returns already-assigned permits through `add_permits_locked`. This is the cancellation contract akku reproduces without `unsafe`.
  - `src/sync/semaphore.rs:19-24`: fairness across `acquire_many`. `:479`: public `MAX_PERMITS`. `:586-589`: cancel safety ("lose your place in the queue"). `:614,661,709,802,884,1161,1193`: `acquire`, `acquire_many`, `try_acquire`, `blocking_acquire`, `acquire_owned`, `close`, `SemaphorePermit::forget`.
  - `src/sync/mutex.rs:18-21,112-115`: FIFO lock order. `:122-126`: no poisoning; a panic unlocks. `:415-420`: cancel safety. `:434,520,618,682,750`: `lock`, `blocking_lock`, `lock_owned`, `try_lock`, `try_lock_owned`. `:474-476`: `blocking_lock` panics inside an async execution context.
  - `src/sync/rwlock.rs:24`: `MAX_READS = u32::MAX >> 3`. `:41-46`: write-preferring FIFO: a read lock is not handed out while an earlier write request waits.
  - `src/sync/notify.rs:30-46`: one stored permit, `notify_one` coalesces. `:381-389,562-566`: `Notified` records the `notify_waiters` call count when it is *created*, so a future created before `notify_waiters` completes even if it was never polled. `:536-540`: cancel safety. `:740,776-811`: `notify_waiters` wakes in batches of `WakeList` and releases the lock before waking. `:1003`: `Notified::enable`. `:1330-1370`: dropping a `Notified` unlinks it (`unsafe` at `:1354`) and, if it had received a `notify_one` it never observed, forwards that notification to the next waiter (`:1364`).
  - `src/util/wake_list.rs:5`: `NUM_WAKERS = 32` wakers per batch.
  - `src/sync/oneshot.rs:246-250`: awaiting `&mut Receiver` is cancel safe. `:622,727,820,947,1171,1240`: `Sender::send`, `Sender::closed`, `poll_closed`, `Receiver::close`, `try_recv`, `blocking_recv`. `:823-862`: coop budget in `Receiver::poll`.
  - `src/sync/mpsc/bounded.rs:128-131,159-160`: capacity `0` panics. `:776-788`: a cancelled `send` loses the message and its queue position; `reserve` is the lossless alternative. `:1046,1116,1265,1356`: `blocking_send`, `reserve`, `reserve_owned`, `try_reserve`. `:243,364,424,478`: `recv`, `try_recv`, `blocking_recv`, `close`.
  - `src/sync/mpsc/chan.rs:310-348`: coop budget in `recv`.
  - `src/sync/broadcast.rs:44-49`: a lagging receiver gets `RecvError::Lagged(n)` and stays connected. `:412-421`: a slot's value drops once every receiver has read it (`rem`). `:479`: `MAX_RECEIVERS = usize::MAX >> 2`. `:575-586`: capacity `0` panics, capacity rounds up to a power of two, and the whole ring is **preallocated** (`(0..capacity).map(...)`), so `channel(usize::MAX >> 1)` aborts on allocation. akku grows lazily instead. `:1424,1502`: `resubscribe`, `recv`.
  - `src/sync/watch.rs:433-474`: version counter with a closed bit. `:629,676,738,776,786,836-837,907`: `borrow`, `borrow_and_update`, `has_changed`, `mark_changed`, `mark_unchanged`, `changed` (wrapped in `cooperative`), `wait_for`. `:1082,1122,1189,1247,1331,1405`: `send`, `send_modify`, `send_if_modified`, `send_replace`, `closed`, `subscribe`.
  - `src/sync/barrier.rs:122-125`: `Barrier::wait` is **not** cancel safe.
  - `src/sync/once_cell.rs:310,350,400`: `set`, `get_or_init`, `get_or_try_init`.
- **async-lock 3.4.2 (`smol-rs/async-lock@f07ba65a02ec3eca32fa95e6d13dc754d619bb35`)**
  - `src/mutex.rs:22-24,578-594`: "eventual fairness": a lock operation that waits longer than 0.5 ms switches to a fair, queued strategy. `:143,164`: `lock_blocking`, and `try_lock` returning `Option<MutexGuard>` (no error type).
  - `src/semaphore.rs:60,124,237`: `try_acquire -> Option`, `acquire_blocking`, `add_permits`. `semaphore.rs` and `barrier.rs` contain 0 `unsafe` tokens; `mutex.rs` has 17, `rwlock.rs` 48, `once_cell.rs` 16 (counted with `grep -c unsafe`), all for `UnsafeCell` data access.
- **event-listener 5.4.2 (`smol-rs/event-listener@77004ac77dcef7884bf71a99c276a50f83a7fa4e`)**
  - `src/intrusive.rs:278-343`: `remove(listener, propagate)` unlinks through raw pointers (`unsafe` at `:284,292,300,311`) and, with `propagate = true`, re-sends a notification the removed listener had received but not consumed (`:330-339`).
  - `src/lib.rs:1075-1078`: a dropped listener always calls `remove(.., true)`, so notifications are never lost on cancellation.
- **futures-channel 0.3.34 (`rust-lang/futures-rs@705e6b5c0f06535b1aac1cb1989a172b3d45be8c`, path `futures-channel/`)**
  - `src/mpsc/mod.rs:47,379-381`: effective capacity is `buffer + num_senders` (one guaranteed slot per sender), a known surprise. akku's capacity is exact. `:348-353`: `MAX_BUFFER`.
  - `src/oneshot.rs:357,366,450`: `Sender::poll_canceled`, `cancellation()`, `Receiver::try_recv`.
- **flume 0.12 (`zesterer/flume@58e0c09b7af2c135d13017c1fd56b8ed9d0c76a5`)**
  - `src/lib.rs:11`: "No `unsafe` code anywhere in the codebase". `:440-445`: the channel is `Mutex<Chan>` with `VecDeque<T>` and `VecDeque<Arc<Hook>>` waiter queues.
  - `src/async.rs:385-407`: `reset_hook` removes a cancelled waiter with `VecDeque::retain` (**O(n)** per cancellation) and passes an already-fired signal on to another receiver. This is the closest safe prior art; akku keeps its hand-off rule but makes removal O(1).
- **async-channel 2.5.0 (`smol-rs/async-channel@35a63c456aaa1906015f5a825e7e35505a749afa`)**
  - `src/lib.rs:135-136,179`: `bounded(cap)` panics on `0`; `unbounded()`. `:324,710`: `send_blocking`, `recv_blocking`. `:389,734`: `Sender::close`/`Receiver::close -> bool` (true if this call closed it). `:1070-1173`: `SendError<T>(pub T)`, `TrySendError::{Full, Closed}`, `RecvError`, `TryRecvError::{Empty, Closed}`, the error shape akku adopts.
- **async-broadcast 0.7.2 (`smol-rs/async-broadcast@f7a99132b7e12cb03945013df78b93a35ceafbc1`)**
  - `src/lib.rs:150,367,394,411`: `broadcast(cap)`, optional overflow mode, "await active receivers". It blocks senders by default; tokio and akku drop the oldest value instead.
- **crossbeam-channel 0.5.17 (`crossbeam-rs/crossbeam@2920c984290229ab4e0ca0452ef09e48a82063f3`, path `crossbeam-channel/`)**
  - `src/channel.rs:60,88,109-110`: `bounded(0)` is a zero-capacity rendezvous channel. `:386`: `try_send` on it succeeds only if a receiver is waiting at that moment.
- **Go (`golang/go@d695060f5be23f2dd86d21d861a292dc2d183cab`)**
  - `src/runtime/chan.go:10-18,34-45`: `hchan` with FIFO `recvq`/`sendq`; at most one of them is non-empty; an unbuffered channel is a rendezvous. `:218`: send on a closed channel panics. `:406-417`: closing a nil or closed channel panics. akku returns errors instead.
  - `src/internal/sync/mutex.go:33-55`: `sync.Mutex` normal mode (woken waiters compete with newcomers) and starvation mode (direct hand-off after 1 ms, `starvationThresholdNs = 1e6`).
  - `src/sync/rwmutex.go:16-25,47`: a pending writer blocks new readers; `rwmutexMaxReaders = 1 << 30`.
  - `src/sync/waitgroup.go:77,160,236`: `Add`, `Wait`, `Go`. `src/sync/once.go:52`: `Once.Do`.
- **Go `x/sync` (`golang/sync@1f920df1e8fcf601d1aaf0f2ea500e1d2ccce105`)**
  - `semaphore/semaphore.go:41,80-98`: `Weighted.Acquire` under context cancellation: if the waiter was granted after cancellation it puts the tokens back and re-notifies; if it was at the front and tokens remain it notifies the next waiters. `:145-172`: `notifyWaiters` stops at the first waiter that does not fit, so large requests are not starved.
- **CPython (`python/cpython@87fc280d3129ff521d082026278fe9bc8e6aeec8`)**
  - `Lib/asyncio/locks.py:96-121`: `Lock.acquire` is FIFO; on cancellation it removes itself (`deque.remove`, O(n)) and wakes the first waiter if the lock is free. `:400-416`: `Semaphore.acquire` undoes a grant it received but will not use (`self._value += 1`) and wakes the next waiter.
- **akku (this repo, `0165b5d`)**
  - `crates/akku/src/task/mod.rs:160-164`: internal `lock()` ignores poisoning because no user code runs under runtime locks. `akku::sync` adopts the same rule (§G2 "Locking discipline").
  - `crates/akku/src/task/mod.rs:887-947`: the cooperative budget. `COOP_BUDGET = 128` (`:892`), thread-local `BUDGET` (`:894-899`), `with_budget` around every task and root poll (`:903-919`), `pub(crate) poll_budget(cx)` (`:926-937`) and `pub(crate) consume_budget()` (`:941-948`). Both are `#[cfg(not(target_arch = "wasm32"))]` today because only `akku::net` uses them (`crates/akku/src/net/reactor.rs:399-401,471-483`, `crates/akku/src/net/mod.rs:133-134`).
  - `crates/akku/src/task/mod.rs:954-964,969-983`: `defer_wake` and the panic-isolating `pub(crate) wake_all`.
  - `crates/akku/src/task/mod.rs:834-842`: thread-local `CURRENT` runtime context, the basis for the "`blocking_*` inside a runtime panics" check.

## Dependencies

`native`: zero external crates, `std` only, **0 `unsafe` blocks**. No `sys/` submodule, so no `.github/unsafe-allowlist.txt` entry. No Cargo feature: `akku::sync` is always compiled (like `akku::io`).

Crate-internal coupling (no new public API outside `akku::sync`):

- `akku::sync` → `akku::task` only:
  - `crate::task::poll_budget(cx)` / `crate::task::consume_budget()`: G4 removes their `#[cfg(not(target_arch = "wasm32"))]` gates (they are pure thread-local code) so the sync primitives budget on every target (Q10).
  - `crate::task::wake_all(wakers)` for batch wakeups (exists, not gated).
  - a new `pub(crate) fn in_runtime() -> bool` (`current().is_some()`) for the `blocking_*` deadlock guard. G3 adds the stub, G4 the body.
- `akku::task` does not depend on `akku::sync`; `akku::sync` does not depend on `akku::net` or `akku::io`.
- Tier 1 (macOS, Windows, Linux, iOS, Android): identical pure-Rust code on all five; no OS-specific behaviour.
- Tier 2 (`wasm32`): everything compiles; `blocking_*` methods are `#[cfg(not(target_arch = "wasm32"))]` because a blocked thread on `wasm32-unknown-unknown` can never be woken (`meta.toml`: `wasm32 = true`).

Planned `meta.toml` (G3): `name = "sync"`, `status = "native"`, `wasm32 = true`, `wave = 2`, `summary = "std::sync superset with safe async Mutex, RwLock, Semaphore, Notify, OnceCell, and oneshot, MPMC, broadcast, and watch channels."`, `tags = ["sync", "async", "concurrency", "channels", "std-superset"]`.

Size estimate: ~1,800–2,300 LOC of implementation (more than the ~1,200–1,500 that `docs/forks/tokio-axum.md` §8 Phase 3 budgets for `time` + `sync` together), plus ~1,500 lines of tests, split over PR-A (~1,100–1,400 LOC) and PR-B (~700–900 LOC).

## G2: API

All items live under `akku::sync`. Signatures are exact; bodies are not part of G2.

### Delivery: PR-A and PR-B

G3 (tests first) and G4 (implementation) run twice on `lib/sync`, once per PR, each with its own Checker pass (Q11):

| PR | Contents | Why first / second |
|---|---|---|
| **PR-A** | `std::sync` re-exports, the private `WaitList`, the `blocking_*` thread parker, `AsyncMutex`, `Semaphore`, `Notify`, `oneshot`, `channel` (bounded incl. rendezvous, unbounded, `reserve`/`Permit`, `poll_reserve`), the two `akku::task` crate-internal edits | everything the Phase 5 native `akku::http` h1 server needs: connection limits (`Semaphore`), request/response hand-off (`oneshot`), body backpressure (`channel` + `poll_reserve`), shutdown (`Notify`) |
| **PR-B** | `AsyncRwLock`, `AsyncOnceCell`, `broadcast`, `watch`, `channel::Receiver::poll_recv` | builds on the PR-A `WaitList`; no Phase 5 dependency |

### `std::sync` re-exports

Pinned against Rust 1.85.0 (`library/std/src/sync/mod.rs:174-224`); order as `rustfmt` emits it. Verified to compile with `#![deny(warnings)]`; without `#[allow(deprecated)]` the `ONCE_INIT` re-export fails with `use of deprecated constant`.

```rust
#[allow(deprecated)] // ONCE_INIT is deprecated in std; re-exported so the superset stays exact.
pub use std::sync::{
    atomic, mpsc, Arc, Barrier, BarrierWaitResult, Condvar, LazyLock, LockResult, Mutex,
    MutexGuard, Once, OnceLock, OnceState, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard,
    TryLockError, TryLockResult, WaitTimeoutResult, Weak, ONCE_INIT,
};
```

A user who writes `akku::sync::ONCE_INIT` still gets std's deprecation warning (the attribute travels with the item).

### Public surface overview

| Item | Kind |
|---|---|
| `AsyncMutex<T>`, `AsyncMutexGuard<'a, T>`, `AsyncMutexOwnedGuard<T>` | async FIFO mutex |
| `AsyncRwLock<T>`, `AsyncRwLockReadGuard<'a, T>`, `AsyncRwLockWriteGuard<'a, T>`, `AsyncRwLockOwnedReadGuard<T>`, `AsyncRwLockOwnedWriteGuard<T>` | async write-preferring FIFO reader-writer lock |
| `Semaphore`, `SemaphorePermit<'a>`, `SemaphoreOwnedPermit`, `AcquireError`, `TryAcquireError` | async FIFO counting semaphore |
| `Notify`, `Notified<'a>` | wake one or all waiters, with one stored permit |
| `AsyncOnceCell<T>` | async lazy one-time initialization |
| `oneshot::{channel, Sender, Receiver, SendError, RecvError, TryRecvError}` | single value, single use |
| `channel::{bounded, unbounded, Sender, Receiver, Permit, SendError, TrySendError, RecvError, TryRecvError}` | async MPMC queue |
| `broadcast::{channel, Sender, Receiver, SendError, RecvError, TryRecvError}` | every receiver sees every value (lossy for laggards) |
| `watch::{channel, Sender, Receiver, Ref, SendError, RecvError}` | latest value, change notification |

### Shared conventions (all primitives)

1. **Async methods are `async fn`**, as in `akku::net` (`AsyncTcpListener::accept`). The returned futures are `Send` whenever the primitive is `Sync` and are cancellation-safe as the table below states. Two exceptions are named types because users need methods or `Unpin` on them: `Notified<'a>` (has `enable`) and `oneshot::Receiver<T>` (implements `Future` itself, as in tokio and futures-channel). Hand-written poll code uses `channel::Sender::poll_reserve` and `channel::Receiver::poll_recv` instead of storing futures.
2. **Fairness: strict FIFO with direct hand-off.** A released resource (lock ownership, permits, a channel slot, a message, a `notify_one` token) is assigned under the primitive's mutex to the waiter at the head of the queue, before any other caller can see it. Newcomers and `try_*` calls cannot barge past queued waiters. Matches tokio (`mutex.rs:18-21`, `semaphore.rs:19-24`, `rwlock.rs:41-46`), unlike Go's normal mode and async-lock's eventual fairness (Q4). **Lock convoys:** strict hand-off means a released lock is owned by the head waiter until that task is scheduled and polled, even if another task on a running thread could take it immediately. Under heavy contention throughput is therefore bounded by scheduler latency (one wake → poll round-trip per hand-off), and a task that re-locks in a loop always goes to the back of the queue. Users keep critical sections short and never hold a guard across slow `.await`s; the rustdoc of `AsyncMutex` and `Semaphore` states this. Go's starvation mode (`mutex.go:33-55`) and async-lock's 0.5 ms rule exist to trade this fairness for throughput; akku can add an opt-in later without changing the default.
3. **Cancellation is lossless.** Dropping any future at any point never loses a permit, lock ownership, notification, or message that the channel accepted. See the cancellation table.
4. **No poisoning** for `AsyncMutex`, `AsyncRwLock`, `Semaphore`, channels, and cells. A guard dropped during unwinding releases normally. Internal `std::sync::Mutex`es are locked with a poison-ignoring helper (`crates/akku/src/task/mod.rs:160-164`).
5. **`try_*` methods never wait and never touch the cooperative budget.** Lock `try_*` methods return `Option` (no failure reason exists without poisoning, and `TryLockError` is a `std::sync` name). Semaphore and channel `try_*` methods return a typed error because they can fail for two reasons.
6. **`blocking_*` methods** (not on `wasm32`) block the calling OS thread until the operation completes, outside any runtime. They park through a private thread-unpark `Waker` (`Arc<ThreadWaker>: Wake` + `std::thread::park`), so they need no runtime and no `unsafe`. **They panic if called on a thread that is driving an `akku::task` runtime** (`block_on` or a task poll), because blocking the only executor thread deadlocks: the panic message contains `"blocking"` (tokio precedent `mutex.rs:474-476`). Under Phase 4, `spawn_blocking` threads are not runtime threads, so `blocking_*` is allowed there. Three cases the guard does **not** prevent, stated in the rustdoc:
   1. **No background workers.** `akku::task` has no worker threads: tasks run only while some thread is inside `block_on`. `blocking_recv` on a plain thread therefore hangs if the sender is a spawned task and no thread drives that runtime's `block_on`.
   2. **Foreign executors are not detected.** `in_runtime()` sees only `akku::task` runtimes. Calling `blocking_*` from inside another executor's poll blocks that executor's thread and may deadlock it.
   3. **Panic during unwinding aborts.** The guard panics; if `blocking_*` is called from a `Drop` impl that runs while the thread is already unwinding, that second panic aborts the process. Destructors must not call `blocking_*` on runtime threads.
7. **Cooperative budget.** Every async operation that can wait spends the `akku::task` budget like `akku::net` does: on entry it calls `crate::task::poll_budget(cx)` (returns `Pending` with a deferred wake once the poll's 128 units are spent), and when it returns `Ready` it calls `crate::task::consume_budget()`. **Exception:** a future that already holds a grant (a hand-off happened while it was queued) completes without the entry check, because yielding while holding a lock or permit would block every other waiter for a whole scheduler tick; it still spends one unit. Outside an `akku::task` poll the budget is `None` and nothing is limited (`crates/akku/src/task/mod.rs:894-899`).
8. **Send/Sync bounds** follow automatically from `Arc`, `Box`, and `std::sync::Mutex`; there are no manual `unsafe impl`. `AsyncMutex<T>: Send + Sync` iff `T: Send` (same as std `Mutex`). **`AsyncRwLock<T>` is `Send` and `Sync` only if `T: Send + Sync`**, which differs from std (`RwLock<T>: Send` iff `T: Send`), because the value lives in an `Arc<T>` and `Arc<T>: Send` needs `T: Sync` (Q6b; documented in rustdoc, asserted in G3). Channel and `oneshot` handles are `Send + Sync` iff `T: Send`; `broadcast` needs `T: Send + Sync` (values are shared as `Arc<T>`) and `T: Clone`; `watch` needs `T: Send + Sync`.
9. **Constructors are `const fn` where the storage allows it** (`AsyncMutex::new`, `Semaphore::new`, `Notify::new`, `AsyncOnceCell::new`), so `static POOL: Semaphore = Semaphore::new(64);` works without `LazyLock`. `AsyncRwLock::new` is a plain `fn` because it allocates the `Arc<T>`; a static `AsyncRwLock` goes through `LazyLock`.
10. **Common trait impls:** `Debug` for every public type (guards forward to `T: Debug`, locks print `<locked>` when `try_*` fails, like std), `Default` where std has it (`AsyncMutex<T: Default>`, `AsyncRwLock<T: Default>`, `AsyncOnceCell<T>`, `Notify`), `From<T>` for `AsyncMutex`, `AsyncRwLock`, `AsyncOnceCell`. Guards implement `Deref` (+ `DerefMut` for exclusive guards) and `Display` when `T: Display`. Error types implement `Debug`, `Display`, `std::error::Error`, `Clone`, `Copy` (when payload-free), `PartialEq`, `Eq`.

### `AsyncMutex`

```rust
/// An async mutual-exclusion lock with FIFO hand-off and no poisoning.
pub struct AsyncMutex<T> { /* private */ }

impl<T> AsyncMutex<T> {
    pub const fn new(value: T) -> Self;
    /// Waits in FIFO order until the lock is free, then returns a guard.
    pub async fn lock(&self) -> AsyncMutexGuard<'_, T>;
    /// Like `lock`, but the guard owns an `Arc` and is `'static`.
    pub async fn lock_owned(self: Arc<Self>) -> AsyncMutexOwnedGuard<T>;
    /// Returns a guard if the lock is free and no task is queued; never waits.
    pub fn try_lock(&self) -> Option<AsyncMutexGuard<'_, T>>;
    pub fn try_lock_owned(self: Arc<Self>) -> Option<AsyncMutexOwnedGuard<T>>;
    /// Blocks the current thread. Panics inside an `akku::task` runtime thread.
    #[cfg(not(target_arch = "wasm32"))]
    #[track_caller]
    pub fn blocking_lock(&self) -> AsyncMutexGuard<'_, T>;
    #[cfg(not(target_arch = "wasm32"))]
    #[track_caller]
    pub fn blocking_lock_owned(self: Arc<Self>) -> AsyncMutexOwnedGuard<T>;
    /// Panics if a guard was leaked with `mem::forget`.
    #[track_caller]
    pub fn get_mut(&mut self) -> &mut T;
    /// Panics if a guard was leaked with `mem::forget`.
    #[track_caller]
    pub fn into_inner(self) -> T;
}

pub struct AsyncMutexGuard<'a, T> { /* private: &'a AsyncMutex<T>, value: Box<T> (~16 B) */ }
pub struct AsyncMutexOwnedGuard<T> { /* private: Arc<AsyncMutex<T>>, value: Box<T> (~16 B) */ }
// Both: Deref<Target = T>, DerefMut, Debug (T: Debug), Display (T: Display),
// Send iff T: Send, Sync iff T: Send + Sync. Drop unlocks and hands off.
```

The value's address is stable from the first `lock` on (it lives in one `Box` that only moves as a pointer); see §G2 "Guards without `unsafe`".

### `AsyncRwLock`

```rust
/// An async reader-writer lock: FIFO, write-preferring, no poisoning.
pub struct AsyncRwLock<T> { /* private */ }

impl<T> AsyncRwLock<T> {
    /// Allocates the `Arc<T>` that holds the value for the lock's whole life (not `const`).
    pub fn new(value: T) -> Self;
    pub async fn read(&self) -> AsyncRwLockReadGuard<'_, T>;
    pub async fn write(&self) -> AsyncRwLockWriteGuard<'_, T>;
    pub async fn read_owned(self: Arc<Self>) -> AsyncRwLockOwnedReadGuard<T>;
    pub async fn write_owned(self: Arc<Self>) -> AsyncRwLockOwnedWriteGuard<T>;
    /// Fails if a writer holds the lock or any task (reader or writer) is queued.
    pub fn try_read(&self) -> Option<AsyncRwLockReadGuard<'_, T>>;
    pub fn try_write(&self) -> Option<AsyncRwLockWriteGuard<'_, T>>;
    pub fn try_read_owned(self: Arc<Self>) -> Option<AsyncRwLockOwnedReadGuard<T>>;
    pub fn try_write_owned(self: Arc<Self>) -> Option<AsyncRwLockOwnedWriteGuard<T>>;
    #[cfg(not(target_arch = "wasm32"))]
    #[track_caller]
    pub fn blocking_read(&self) -> AsyncRwLockReadGuard<'_, T>;
    #[cfg(not(target_arch = "wasm32"))]
    #[track_caller]
    pub fn blocking_write(&self) -> AsyncRwLockWriteGuard<'_, T>;
    /// Panics if a read or write guard was leaked with `mem::forget`.
    #[track_caller]
    pub fn get_mut(&mut self) -> &mut T;
    /// Panics if a read or write guard was leaked with `mem::forget`.
    #[track_caller]
    pub fn into_inner(self) -> T;
}
// AsyncRwLock<T>: Send iff T: Send + Sync, Sync iff T: Send + Sync (differs from std's Send bound).
// Read guards (&'a lock or Arc<lock>, plus an Arc<T> clone): Deref.
// Write guards (&'a lock or Arc<lock>, plus the moved-out Arc<T>): Deref + DerefMut via Arc::get_mut.
// All guards Send + Sync iff T: Send + Sync. Debug/Display as for AsyncMutexGuard.
```

Queue policy: one FIFO queue of readers and writers. When the lock becomes free, the head is granted; if the head is a reader, every reader directly behind it is granted too (one batch), up to the first queued writer. While a writer waits, new readers queue behind it (no reader can starve a writer). No reader limit: the reader count is a `usize` bounded by live guards (tokio's `MAX_READS`, `rwlock.rs:24`, exists only because of its semaphore encoding).

### `Semaphore`

```rust
/// An async counting semaphore with FIFO hand-off and close.
pub struct Semaphore { /* private */ }

impl Semaphore {
    /// Largest permit count; `new`, `add_permits`, and `acquire_many` panic above it.
    pub const MAX_PERMITS: usize = usize::MAX >> 3;
    /// Panics if `permits > MAX_PERMITS`.
    #[track_caller]
    pub const fn new(permits: usize) -> Self;
    pub fn available_permits(&self) -> usize;
    /// Adds permits, handing them to queued waiters first.
    /// Panics if the total would exceed `MAX_PERMITS`.
    #[track_caller]
    pub fn add_permits(&self, n: usize);
    /// Removes up to `n` currently available permits; returns how many were removed.
    pub fn forget_permits(&self, n: usize) -> usize;
    pub async fn acquire(&self) -> Result<SemaphorePermit<'_>, AcquireError>;
    /// `n == 0` returns a zero-permit immediately (unless closed).
    /// Panics if `n > MAX_PERMITS` (such a request could never complete).
    pub async fn acquire_many(&self, n: usize) -> Result<SemaphorePermit<'_>, AcquireError>;
    pub async fn acquire_owned(self: Arc<Self>) -> Result<SemaphoreOwnedPermit, AcquireError>;
    pub async fn acquire_many_owned(self: Arc<Self>, n: usize) -> Result<SemaphoreOwnedPermit, AcquireError>;
    pub fn try_acquire(&self) -> Result<SemaphorePermit<'_>, TryAcquireError>;
    #[track_caller]
    pub fn try_acquire_many(&self, n: usize) -> Result<SemaphorePermit<'_>, TryAcquireError>;
    pub fn try_acquire_owned(self: Arc<Self>) -> Result<SemaphoreOwnedPermit, TryAcquireError>;
    #[track_caller]
    pub fn try_acquire_many_owned(self: Arc<Self>, n: usize) -> Result<SemaphoreOwnedPermit, TryAcquireError>;
    #[cfg(not(target_arch = "wasm32"))]
    #[track_caller]
    pub fn blocking_acquire(&self) -> Result<SemaphorePermit<'_>, AcquireError>;
    #[cfg(not(target_arch = "wasm32"))]
    #[track_caller]
    pub fn blocking_acquire_many(&self, n: usize) -> Result<SemaphorePermit<'_>, AcquireError>;
    /// Closes the semaphore: every queued and future `acquire*` returns `AcquireError`,
    /// `try_acquire*` returns `TryAcquireError::Closed`. Held permits stay valid and are
    /// returned on drop. Idempotent.
    pub fn close(&self);
    pub fn is_closed(&self) -> bool;
}

pub struct SemaphorePermit<'a> { /* private */ }
pub struct SemaphoreOwnedPermit { /* private */ }
// Both: num_permits(&self) -> usize; forget(self) (permits are not returned);
// Drop returns the permits and hands them to queued waiters. Send + Sync, Debug.
impl SemaphoreOwnedPermit { pub fn semaphore(&self) -> &Arc<Semaphore>; }

/// The semaphore was closed.
pub struct AcquireError(/* private */);
pub enum TryAcquireError {
    /// The semaphore was closed.
    Closed,
    /// Not enough permits are available, or other tasks are queued ahead.
    NoPermits,
}
```

Head-of-line rule (tokio `batch_semaphore.rs:609-612`, Go `x/sync` `semaphore.go:145-172`): if the head waiter asks for more permits than are available, it receives the available ones as a partial grant and everyone behind it waits. `available_permits()` reports only unassigned permits, so `try_acquire` cannot barge past a partially served head.

Two consequences, both documented in rustdoc and tested in G3:

- **An unsatisfiable head stalls the queue.** `acquire_many(n)` with `n` larger than every permit that will ever exist (for example `Semaphore::new(4)` and `acquire_many(5)`, with no later `add_permits`) never completes, and every waiter behind it waits too, until `add_permits` makes the request satisfiable, the head future is dropped (its partial grant then flows to the next waiters), or `close()` is called. `MAX_PERMITS` only rejects requests that can never fit at all; it cannot detect this case.
- **`close()` returns partial grants.** When `close()` runs while the head waiter holds a partial grant, those permits go back to the pool in the same critical section (so `available_permits()` counts them again), and the head waiter's future returns `Err(AcquireError)`.

### `Notify`

```rust
/// Wakes one waiter (storing one permit if nobody waits) or all current waiters.
pub struct Notify { /* private */ }

impl Notify {
    pub const fn new() -> Self;
    /// Creates a future that completes after a `notify_one` permit or a later `notify_waiters`.
    /// The future records the `notify_waiters` epoch at creation, before it is polled.
    pub fn notified(&self) -> Notified<'_>;
    /// Hands a permit to the oldest waiter, or stores it (at most one) if none waits.
    pub fn notify_one(&self);
    /// Completes every `Notified` created before this call; stores no permit.
    pub fn notify_waiters(&self);
}

#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct Notified<'a> { /* private */ }
// Future<Output = ()> + Unpin + Send + Sync + Debug.
impl Notified<'_> {
    /// Joins the waiter queue without a waker, so a following `notify_one` is handed to
    /// this future. Returns `true` if the future is already notified.
    pub fn enable(&mut self) -> bool;
}
```

### `AsyncOnceCell`

```rust
/// A cell initialized at most once by an async initializer.
pub struct AsyncOnceCell<T> { /* private: OnceLock<T> + internal FIFO init gate */ }

impl<T> AsyncOnceCell<T> {
    pub const fn new() -> Self;
    pub fn get(&self) -> Option<&T>;
    pub fn get_mut(&mut self) -> Option<&mut T>;
    /// Stores `value` if the cell is empty; otherwise returns it back.
    pub fn set(&self, value: T) -> Result<(), T>;
    /// Runs `init` in at most one caller at a time (FIFO); others wait. If the running
    /// initializer is cancelled or panics, the next waiter runs its own `init`.
    pub async fn get_or_init<F, Fut>(&self, init: F) -> &T
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = T>;
    /// Like `get_or_init`; an `Err` leaves the cell empty and lets the next waiter try.
    pub async fn get_or_try_init<F, Fut, E>(&self, init: F) -> Result<&T, E>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T, E>>;
    pub fn take(&mut self) -> Option<T>;
    pub fn into_inner(self) -> Option<T>;
}
// Send iff T: Send; Sync iff T: Send + Sync (as OnceLock). Default, From<T>, Debug.
```

A `set` that races a running initializer wins if it lands first; the initializer's value is then dropped and `get_or_init` returns the stored one (the `OnceLock::set` semantics).

**Re-entrancy deadlocks.** An `init` future that calls `get_or_init`/`get_or_try_init` on the **same** cell (directly or through a task it awaits) waits behind its own init gate forever. Like `OnceLock::get_or_init` (which deadlocks or panics in this case), this is documented in the rustdoc and not detected.

### `oneshot`

```rust
pub mod oneshot {
    pub fn channel<T>() -> (Sender<T>, Receiver<T>);

    pub struct Sender<T> { /* private */ }
    impl<T> Sender<T> {
        /// Fails with the value if the receiver was dropped or closed.
        pub fn send(self, value: T) -> Result<(), SendError<T>>;
        /// Completes when the receiver is dropped or closed.
        pub async fn closed(&mut self);
        pub fn is_closed(&self) -> bool;
    }

    /// Awaiting the receiver (or `&mut receiver`) yields the value.
    pub struct Receiver<T> { /* private */ }
    // impl<T> Future for Receiver<T> { type Output = Result<T, RecvError>; } + Unpin.
    // Polling again after `Ready` panics ("polled after completion"), as `task::JoinHandle`.
    impl<T> Receiver<T> {
        pub fn try_recv(&mut self) -> Result<T, TryRecvError>;
        /// Prevents further sends; a value sent before `close` can still be received.
        pub fn close(&mut self);
        #[cfg(not(target_arch = "wasm32"))]
        #[track_caller]
        pub fn blocking_recv(self) -> Result<T, RecvError>;
    }

    pub struct SendError<T>(pub T);
    /// The sender was dropped without sending.
    pub struct RecvError;
    pub enum TryRecvError { Empty, Closed }
}
// Sender<T>, Receiver<T>: Send + Sync iff T: Send. Debug.
```

### `channel` (async MPMC)

```rust
#[doc(alias = "mpsc")]
pub mod channel {
    /// A channel holding at most `capacity` messages. `capacity == 0` is a rendezvous
    /// channel: `send` completes only when a receiver has taken the message. No memory is
    /// preallocated, so any `capacity` up to `usize::MAX` is valid.
    pub fn bounded<T>(capacity: usize) -> (Sender<T>, Receiver<T>);
    /// A channel without a capacity limit; `send` never waits.
    pub fn unbounded<T>() -> (Sender<T>, Receiver<T>);

    /// Cloneable sending half.
    pub struct Sender<T> { /* private */ }
    impl<T> Sender<T> {
        /// Waits (FIFO among senders) for capacity, then enqueues `value`. Fails with the
        /// value if the channel closes first. On `bounded(0)` it completes only once a
        /// receiver has returned the value from its own `recv`.
        pub async fn send(&self, value: T) -> Result<(), SendError<T>>;
        /// Never waits. On `bounded(0)` always `Full` (or `Closed`): see "Rendezvous".
        pub fn try_send(&self, value: T) -> Result<(), TrySendError<T>>;
        #[cfg(not(target_arch = "wasm32"))]
        #[track_caller]
        pub fn blocking_send(&self, value: T) -> Result<(), SendError<T>>;
        /// Waits for capacity and reserves one slot; the lossless way to send under
        /// cancellation (`time::timeout`, task abort).
        /// Panics on `bounded(0)` (there is no slot to reserve).
        #[track_caller]
        pub async fn reserve(&self) -> Result<Permit<'_, T>, SendError<()>>;
        /// On `bounded(0)` always `Full(())` (or `Closed(())`).
        pub fn try_reserve(&self) -> Result<Permit<'_, T>, TrySendError<()>>;
        /// Poll-based `reserve` for hand-written futures (Phase 5 h1 body backpressure).
        /// The registration lives in this handle and is wake-only (see below).
        /// Panics on `bounded(0)`.
        #[track_caller]
        pub fn poll_reserve(&mut self, cx: &mut Context<'_>) -> Poll<Result<Permit<'_, T>, SendError<()>>>;
        /// Completes when the channel is closed or every receiver is dropped.
        pub async fn closed(&self);
        /// Closes the channel for all handles; returns `true` if this call closed it.
        pub fn close(&self) -> bool;
        pub fn is_closed(&self) -> bool;
        pub fn len(&self) -> usize;
        pub fn is_empty(&self) -> bool;
        /// `None` for an unbounded channel.
        pub fn capacity(&self) -> Option<usize>;
        pub fn sender_count(&self) -> usize;
        pub fn receiver_count(&self) -> usize;
        pub fn same_channel(&self, other: &Sender<T>) -> bool;
    }

    /// Cloneable receiving half; each message goes to exactly one receiver.
    pub struct Receiver<T> { /* private */ }
    impl<T> Receiver<T> {
        /// Waits (FIFO among receivers) for a message. After close or after every sender
        /// is dropped, buffered messages are still delivered, then `Err(RecvError)`.
        pub async fn recv(&self) -> Result<T, RecvError>;
        pub fn try_recv(&self) -> Result<T, TryRecvError>;
        #[cfg(not(target_arch = "wasm32"))]
        #[track_caller]
        pub fn blocking_recv(&self) -> Result<T, RecvError>;
        /// Poll-based receive for hand-written futures (PR-B). The registration lives in
        /// this handle and is wake-only: it never receives a grant or a message hand-off.
        pub fn poll_recv(&mut self, cx: &mut Context<'_>) -> Poll<Result<T, RecvError>>;
        pub fn close(&self) -> bool;
        pub fn is_closed(&self) -> bool;
        pub fn len(&self) -> usize;
        pub fn is_empty(&self) -> bool;
        pub fn capacity(&self) -> Option<usize>;
        pub fn sender_count(&self) -> usize;
        pub fn receiver_count(&self) -> usize;
        pub fn same_channel(&self, other: &Receiver<T>) -> bool;
    }

    /// One reserved slot. Dropping it unused releases the slot to the next sender.
    pub struct Permit<'a, T> { /* private */ }
    impl<T> Permit<'_, T> { pub fn send(self, value: T); }

    pub struct SendError<T>(pub T);
    pub enum TrySendError<T> {
        /// The channel is full (always on `bounded(0)`).
        Full(T),
        Closed(T),
    }
    /// The channel is closed and empty.
    pub struct RecvError;
    pub enum TryRecvError { Empty, Closed }
}
// Sender<T>, Receiver<T>, Permit<'_, T>: Send + Sync iff T: Send. Clone for Sender/Receiver. Debug.
```

Close and disconnect:

- `close()` (from either side) or the last `Sender` dropping closes the channel: queued senders wake with `Err(SendError(value))` and get **their own value back**; new sends fail the same way; receivers drain the buffer, then get `RecvError`; `closed()` futures complete.
- The last `Receiver` dropping also closes the channel, and the buffered messages are dropped **after** the internal lock is released (a message may own a `Sender` of the same channel).
Receiver grants (all capacities). Messages never move into a receiver's waiter slot. A queued `recv` is granted a **claim**: a count against the messages that are available (buffered messages, plus at `bounded(0)` the values of queued senders). The invariant `claims <= available` holds after every critical section, and claims go to the oldest queued receivers first. A granted receiver takes the front message itself when it is polled. A newcomer `recv`, `try_recv`, or `poll_recv` may take a message only if `available > claims`, so it cannot barge past granted receivers. If a granted receiver is dropped before it is polled, its claim passes to the next queued receiver or lapses. No message moved, so no message is lost, put back, or duplicated, and the buffer never exceeds `capacity`.

Rendezvous (`bounded(0)`), defined exactly:

1. `send(v)` puts `v` in its own sender slot, queued FIFO among senders, and grants one claim to the oldest queued receiver if there is one. The value stays in the sender slot.
2. A receiver that holds a claim or finds an unclaimed queued sender takes the value from the **oldest** queued sender slot when it is polled. In the same critical section that sender is marked complete and woken. **`send` completes only at this point**: the value has been returned from a `recv`.
3. **(i) A receiver is granted a claim and then dropped unpolled.** The value never left the sender slot. The claim passes to the next queued receiver (which is woken) or lapses. The sender stays queued at its position and its `send` is still pending. No buffered item appears, and `len()` still counts the value as a queued send (`len()` on `bounded(0)` counts the values of queued senders).
4. A queued `send` cancelled before step 2 removes its slot and drops `v` (not sent). If that leaves `claims > available`, the newest claim is revoked and that receiver goes back to waiting at the head of the receiver queue (a spurious wake).
5. `try_send` returns `Full(v)` (or `Closed(v)`) on `bounded(0)`, even while a receiver waits. A non-waiting send cannot confirm step 2, and accepting the value would create exactly the buffered item that `bounded(0)` must not have. This deliberately differs from crossbeam (`channel.rs:386`) and Go, whose receivers cannot be cancelled after they are chosen.
6. **(ii) `reserve`, `try_reserve`, and `poll_reserve` on `bounded(0)`.** A `Permit` reserves a buffer slot, and `bounded(0)` has none. `try_reserve` returns `Full(())` (or `Closed(())`); `reserve` and `poll_reserve` panic with a message containing `"rendezvous"`. They are `#[track_caller]` and the capacity is known when the channel is built, so the misuse shows up on first use. Code that needs lossless sends under cancellation uses `bounded(1)`.
7. `close()` wakes queued senders with `Err(SendError(v))` (their own values back) and receivers with `Err(RecvError)` once no queued sender is left. On `bounded(0)` a send whose value is not yet taken is not delivered after close.

Wake-only registrations (`poll_recv`, `poll_reserve`). These live in the handle (one per `Sender`/`Receiver` clone). They sit in a separate wake list, never in the FIFO grant queue, and never receive a claim, a reserved slot, or a message. When a message or slot becomes available that no FIFO waiter claims, every wake-only registration of that kind is woken, and on its next poll it competes like `try_recv`/`try_reserve`. A handle that stops polling therefore never holds a claim or slot that blocks others. The trade-off is no FIFO among poll-based callers and an O(k) wake for `k` registrations. The registration is removed when the handle drops or completes.

### `broadcast`

```rust
pub mod broadcast {
    /// Keeps the newest `capacity` values. Panics if `capacity == 0`.
    /// Storage grows lazily, so a huge capacity does not allocate up front.
    #[track_caller]
    pub fn channel<T: Clone>(capacity: usize) -> (Sender<T>, Receiver<T>);

    pub struct Sender<T> { /* private */ }   // Clone
    impl<T: Clone> Sender<T> {
        /// Never waits. Returns the number of receivers that will see the value,
        /// or the value back if there are none. Overwrites the oldest value when full.
        pub fn send(&self, value: T) -> Result<usize, SendError<T>>;
        pub fn subscribe(&self) -> Receiver<T>;
        pub fn receiver_count(&self) -> usize;
        pub fn len(&self) -> usize;
        pub fn is_empty(&self) -> bool;
        pub fn same_channel(&self, other: &Sender<T>) -> bool;
        /// Completes when every receiver is dropped.
        pub async fn closed(&self);
    }

    pub struct Receiver<T> { /* private */ }   // not Clone; see resubscribe
    impl<T: Clone> Receiver<T> {
        /// Next value in send order. `Lagged(n)` reports `n` overwritten values and moves
        /// this receiver to the oldest value still stored; the receiver stays usable.
        pub async fn recv(&mut self) -> Result<T, RecvError>;
        pub fn try_recv(&mut self) -> Result<T, TryRecvError>;
        #[cfg(not(target_arch = "wasm32"))]
        #[track_caller]
        pub fn blocking_recv(&mut self) -> Result<T, RecvError>;
        /// A new receiver that starts at the next value sent.
        pub fn resubscribe(&self) -> Self;
        pub fn len(&self) -> usize;
        pub fn is_empty(&self) -> bool;
        pub fn is_closed(&self) -> bool;
        pub fn same_channel(&self, other: &Receiver<T>) -> bool;
    }

    /// No receiver exists.
    pub struct SendError<T>(pub T);
    pub enum RecvError { Closed, Lagged(u64) }
    pub enum TryRecvError { Empty, Closed, Lagged(u64) }
}
// Send + Sync iff T: Send + Sync. Debug.
```

The ring stores **`Arc<T>` per slot** (Q15). `send` wraps the value once (one allocation per `send`). `recv` clones the `Arc` and advances the receiver's position under the internal lock, then calls `T::clone` on it **after the lock is released**, so no user code runs under the lock: a `Clone` impl may even use the same channel. If `T::clone` panics, the panic propagates and that value counts as received; the receiver's position has already moved past it. Each slot also carries a "still unread" counter set to the receiver count at send time (tokio `broadcast.rs:412-421`). The slot drops its `Arc` once every such receiver has read it or once it is overwritten, and the `T` drops when the last clone of the `Arc` (possibly a receiver's in-flight clone) drops, which is always outside the lock.

### `watch`

```rust
pub mod watch {
    pub fn channel<T>(init: T) -> (Sender<T>, Receiver<T>);

    pub struct Sender<T> { /* private */ }
    impl<T> Sender<T> {
        /// Stores `value` and notifies receivers; fails with the value if no receiver exists.
        pub fn send(&self, value: T) -> Result<(), SendError<T>>;
        /// Always stores `value`, even without receivers; returns the previous value.
        pub fn send_replace(&self, value: T) -> T;
        /// Modifies the value in place and always notifies.
        pub fn send_modify<F: FnOnce(&mut T)>(&self, modify: F);
        /// Notifies only if `modify` returns `true`.
        pub fn send_if_modified<F: FnOnce(&mut T) -> bool>(&self, modify: F) -> bool;
        pub fn borrow(&self) -> Ref<'_, T>;
        pub fn subscribe(&self) -> Receiver<T>;
        pub fn receiver_count(&self) -> usize;
        pub fn is_closed(&self) -> bool;
        pub async fn closed(&self);
    }

    pub struct Receiver<T> { /* private */ }   // Clone
    impl<T> Receiver<T> {
        pub fn borrow(&self) -> Ref<'_, T>;
        /// Borrows and marks the current version as seen.
        pub fn borrow_and_update(&mut self) -> Ref<'_, T>;
        pub fn has_changed(&self) -> Result<bool, RecvError>;
        pub fn mark_changed(&mut self);
        pub fn mark_unchanged(&mut self);
        /// Completes when a version newer than the last seen one exists; marks it seen.
        /// Returns `Err` only if the sender is dropped and no unseen version remains.
        pub async fn changed(&mut self) -> Result<(), RecvError>;
        /// Waits until `f(&value)` is true (checking the current value first).
        pub async fn wait_for<F: FnMut(&T) -> bool>(&mut self, f: F) -> Result<Ref<'_, T>, RecvError>;
        pub fn same_channel(&self, other: &Receiver<T>) -> bool;
    }

    /// A read borrow of the current value. Holds an internal read lock: keep it short and
    /// never across `.await` (it is `!Send`, so spawned tasks cannot hold it there).
    pub struct Ref<'a, T> { /* private: std::sync::RwLockReadGuard<'a, T> + changed flag */ }
    impl<T> Ref<'_, T> { pub fn has_changed(&self) -> bool; }
    // Deref<Target = T>, Debug.

    /// No receiver exists.
    pub struct SendError<T>(pub T);
    /// The sender was dropped.
    pub struct RecvError;
}
// Sender<T>, Receiver<T>: Send + Sync iff T: Send + Sync.
```

If the closure of `send_modify`/`send_if_modified` panics, the partial modification stays visible, the version is bumped so receivers see it, and the panic propagates (the internal `RwLock` poison is ignored). Calling `send*` on a thread that holds a `Ref` of the same channel deadlocks or panics, as `std::sync::RwLock` documents; this is stated in the rustdoc.

### Patterns (documented in the module rustdoc)

Until a cancellation token and a `WaitGroup` exist (`_dev/TODO.md`), two server patterns use v1 primitives:

- **Latched shutdown via `watch::channel(false)`.** The server keeps the `Sender<bool>`; every task holds a `Receiver<bool>` clone and waits on `rx.wait_for(|stop| *stop).await` (or checks `*rx.borrow()`). `tx.send_replace(true)` latches: unlike `Notify::notify_waiters`, a task that subscribes or starts waiting **after** the signal still sees it at once, and unlike `broadcast` nothing can lag. Dropping the sender also wakes every waiter with `Err(RecvError)`, which tasks treat as "stop".
- **Connection draining via a `Sender` clone per connection plus `closed()`.** The server creates `let (drain_tx, drain_rx) = channel::bounded::<()>(1)`, hands every accepted connection a clone of `drain_tx` (it never sends), drops its own copy at shutdown, and awaits `drain_rx`'s `recv()`. That returns `Err(RecvError)` once the last connection's clone drops, meaning every connection has finished. The mirror form, holding a `Receiver` clone per connection and awaiting `Sender::closed()`, works the same way. `sender_count()` reports how many connections are still open.

### Guards without `unsafe`

Rust gives `&T`/`&mut T` from a shared reference only through a scoped lock guard (`std::sync::MutexGuard`, `!Send`) or through `UnsafeCell`. A `!Send` guard cannot be held across `.await` in a spawned task, and `UnsafeCell` needs `unsafe`. akku's guards therefore **own** the data while they exist:

- **`AsyncMutex<T>`** keeps `enum Slot<T> { Inline(T), Boxed(Box<T>), Lent }` in its internal state (Q6).
  - `const fn new` stores `Inline(value)`, with no allocation.
  - The first admitted `lock()` takes `Inline(t)` out under the internal lock, then boxes it **outside** the lock (`Box::new(t)`, once per mutex lifetime). From then on the state holds `Boxed(b)`, and every later `lock()` moves only the `Box` pointer into the guard. A guard is about 16 B (a reference or `Arc` plus a `Box`). `Deref`/`DerefMut` go through the `Box`, so the value's **address is stable** from the first lock on.
  - `Drop` puts `Boxed(b)` back and hands the lock to the next waiter in one critical section. While a guard exists the state is `Lent`.
  - `get_mut`/`into_inner` read `Inline` or `Boxed` directly.
- **`AsyncRwLock<T>`** keeps the value **permanently in one `Arc<T>`** (`Option<Arc<T>>` in the state; `None` while a writer holds it). `new` allocates the `Arc`, so it is not `const`.
  - A reader clones the `Arc` under the internal lock; `Deref` is `&*arc`.
  - A writer is admitted only when the reader count is `0`. It moves the `Arc` out of the state and gets `&mut T` through `Arc::get_mut`. Admission guarantees uniqueness, so a failing `get_mut` is an invariant violation and panics with `"akku::sync::AsyncRwLock invariant violated: a read guard's Arc clone outlived its admission"`. `DerefMut` calls `Arc::get_mut` (a cheap uniqueness check) on each use. `Drop` puts the `Arc` back and hands over.
  - **Read-guard drop order:** a read guard drops its `Arc<T>` clone **before** it takes the internal lock to decrement the reader count. In the reverse order, a writer could be admitted between the decrement and the clone's drop, and `get_mut` would fail. G3 tests this (§G3 item 14).
  - Consequence: `AsyncRwLock<T>: Send + Sync` needs `T: Send + Sync` (§Shared conventions 8).
- **`mem::forget(guard)`** keeps the lock locked forever, as in every lock implementation:
  - A forgotten `AsyncMutex` guard takes the `Box` with it, so the state stays `Lent`. A forgotten `AsyncRwLock` write guard takes the `Arc`. A forgotten read guard leaks one `Arc` clone and never decrements the reader count, so writers wait forever.
  - In every case, `get_mut`/`into_inner` then panic with a message containing `"forgotten"`. The value itself leaks (it is never dropped). `try_lock`/`try_write` keep returning `None`.
- **Mapped guards later:** a mapped guard can be built without `unsafe` as the original guard plus a stored `fn(&mut T) -> &mut U` projection that is re-applied on each `Deref`/`DerefMut` (closures would need boxing; a fn pointer does not). This is deferred to `_dev/TODO.md`.

### Safe waiter queue (private `sync/wait.rs`)

One generic `WaitList<W>` serves every primitive. It lives inside the primitive's single `std::sync::Mutex<State>`:

```rust
pub(crate) struct WaitList<W> {
    slots: Vec<Slot<W>>,       // slab
    free: Option<usize>,       // head of the vacant-slot free list
    head: Option<usize>,       // oldest queued waiter
    tail: Option<usize>,       // newest queued waiter
    queued: usize,
    next_id: u64,              // unique per push, never reused
}
enum Slot<W> {
    Vacant { next_free: Option<usize> },
    Occupied { id: u64, waker: Option<Waker>, prev: Option<usize>, next: Option<usize>, phase: Phase, payload: W },
}
enum Phase { Queued, Granted, Closed }
pub(crate) struct WaitKey { index: usize, id: u64 }   // stored in the future
```

| Operation | Cost | Notes |
|---|---|---|
| `push_back(waker, payload) -> WaitKey` | O(1) amortized | reuses a vacant slot, else `Vec::push` |
| `set_waker(&key, &Waker)` | O(1) | replaces only if `!will_wake`; the old waker is returned and dropped after unlock |
| `grant_front() -> Option<Waker>` | O(1) | unlinks the head, sets `Granted`; caller wakes after unlock |
| `remove(&key) -> Option<(Phase, W)>` | O(1) | unlinks if `Queued`, frees the slot; `None` if `key.id` does not match (stale key) |
| `drain(Phase) -> Vec<Waker>` | O(n) | `close`, `notify_waiters`, broadcast `send` |

- Unique `id`s (a `u64` that never wraps in practice) stop a stale key from touching a slot that a newer waiter reuses (ABA). Because ids are unique, the list may **shrink when it becomes empty** (`queued == 0` and `slots.capacity() > 64` → `clear` + `shrink_to(64)`), so a burst of a million waiters does not pin memory forever.
- **Peak size while non-empty (Q12).** Live keys are slab indices, so slots cannot be compacted while any waiter is queued. A slab keeps its peak size (vacant slots are reused, not freed) until the queue becomes empty once. One long-lived waiter therefore pins the slab of a past burst.
- **Slots hold payloads inline.** A blocked `channel::Sender::send` keeps its `T` in its slot (`payload: W` holds `T`), so a sender queue's memory is about `peak queued senders × (slot header + size_of::<T>())`. Large messages should be boxed by the caller; the rustdoc of `bounded` says so.
- Cost against tokio's intrusive list: same O(1) enqueue, cancel, and grant; one contiguous `Vec` per primitive instead of nodes inside futures (≈56–80 bytes per waiter plus the payload, retained until the queue empties); futures can be `Unpin`; 0 instead of ~176 `unsafe` blocks (`docs/forks/tokio-axum.md` §5B); no atomic fast path in v1, so every operation takes one uncontended `std::sync::Mutex` (~15 ns).
- Rejected: `VecDeque<Arc<Waiter>>` with `retain` on cancel (flume `src/async.rs:385-407`, CPython `deque.remove`): O(n) per cancellation, so `n` cancellations (a `timeout` storm) cost O(n²) under the lock, a DoS vector. Also rejected: `VecDeque<Weak<Waiter>>` with lazy skipping: O(1) amortized, but one `Arc` allocation per wait and unbounded tombstones under cancel storms until compaction.

### Locking discipline

1. One `std::sync::Mutex` per primitive guards all of its state (counts, buffer, every `WaitList`); every check-then-register happens in one critical section, so no wakeup can be lost between "condition false" and "waker stored". This holds unchanged under a multi-threaded executor (Phase 4).
2. **No user code under the lock**, except `Waker::clone` while registering (cheap for `Arc<impl Wake>` wakers). `broadcast` clones `T` after unlocking (values are `Arc<T>`), and `AsyncMutex` boxes its value outside the lock. Wakers are woken, replaced wakers dropped, and user values (`T`) dropped only after the guard is released. Batch wakeups use `crate::task::wake_all`, which wakes every waker even if one panics and then resumes the first panic.
3. Internal mutexes ignore poisoning (`unwrap_or_else(PoisonError::into_inner)`). Every critical section leaves the state consistent before any operation that could panic.

### Cancellation-safety table

"Granted" means a hand-off reached the future while it was queued, but the future was dropped before it observed the hand-off.

| Primitive / operation | Dropped while queued | Dropped after grant | Cancel-safe? |
|---|---|---|---|
| `AsyncMutex::lock`, `lock_owned` | unlinked, O(1); position lost | ownership passes to the next waiter, or the lock is freed | yes |
| `AsyncRwLock::read`, `read_owned` | unlinked; if this unblocks readers behind a removed writer, they are granted | read count released; a writer behind may be granted | yes |
| `AsyncRwLock::write`, `write_owned` | unlinked; readers that queued directly behind it are granted if no writer holds the lock | ownership passes to the next waiter(s) | yes |
| `Semaphore::acquire*` | unlinked; partially assigned permits return to the pool and go to the next waiter (tokio `batch_semaphore.rs:758-779`) | all assigned permits go to the next waiter(s) or the pool | yes |
| `Notify::notified` / `Notified` | unlinked | a `notify_one` token passes to the next waiter or becomes the stored permit (tokio `notify.rs:1364`); a `notify_waiters` wake is not forwarded (it was a broadcast) | yes |
| `AsyncOnceCell::get_or_init` (initializer running) | — | the init gate passes to the next waiter, which runs its own initializer; the dropped initializer's partial work is dropped | yes |
| `AsyncOnceCell::get_or_init` (waiting) | unlinked | gate passes on | yes |
| `oneshot::Receiver` (`(&mut rx).await`) | waker cleared; value stays in the channel | — | yes |
| `oneshot::Sender::closed` | waker cleared | — | yes |
| `channel::Sender::send` (capacity ≥ 1) | unlinked; **the value is dropped with the future and was not sent** (same contract as tokio `bounded.rs:776-788`) | send already completed (the value moved into the buffer): the message is delivered | partial: never half-sent, but the value is lost; use `reserve` |
| `channel::Sender::send` (`bounded(0)`) | unlinked, value dropped, not sent; a surplus receiver claim is revoked | — (completion happens only when a receiver takes the value in its own poll) | partial, as above; `reserve` is unavailable on `bounded(0)` |
| `channel::Sender::reserve` | unlinked | the reserved slot passes to the next sender | yes |
| `channel::Permit` dropped unused | — | slot released to the next sender | yes |
| `channel::Receiver::recv` | unlinked | the claim passes to the next queued receiver or lapses; no message moved, so nothing is put back | yes |
| `channel::Receiver::poll_recv`, `Sender::poll_reserve` | wake-only registration; removed with the handle | — (never granted) | yes |
| `channel::Sender::closed` | unlinked | — | yes |
| `broadcast::Receiver::recv` | unlinked; position unchanged | — (values are cloned on poll, not handed off) | yes |
| `watch::Receiver::changed`, `wait_for` | unlinked; seen version unchanged | — | yes |
| `blocking_*` | not futures; cannot be cancelled | — | n/a |

Task-level consequences: `JoinHandle::abort` and `Runtime` drop both drop the task's future, so they run the same paths. A panicking task unwinds its guards and releases them (no poisoning).

### DoS limits and defaults

| Limit | Default / rule | Reason |
|---|---|---|
| Waiter queues | no hard cap; memory is O(peak live futures the caller created), kept at peak while the queue is non-empty, released when it empties; blocked senders' `T` sits inline in slots | a cap would add an error path to `lock()`; tasks are already capped by `task::Limits::max_tasks` (65,536) |
| `channel::bounded(capacity)` | any `usize`, `0` = rendezvous; no preallocation | `bounded(usize::MAX)` must not abort |
| `channel::unbounded()` | unbounded by definition; rustdoc warns that a slow consumer means unbounded memory | parity with tokio, async-channel, flume |
| `broadcast::channel(capacity)` | `capacity >= 1` (panics on `0`); lazy growth up to `capacity` | tokio preallocates (`broadcast.rs:575-586`) |
| `Semaphore::MAX_PERMITS` | `usize::MAX >> 3` (tokio parity); `new`, `add_permits`, `acquire_many` panic above it | overflow-free arithmetic and a clear contract |
| Batch wakes | collected into one `Vec<Waker>` under the lock, woken after unlock | one allocation per `close`/`notify_waiters`/broadcast `send`, no lock held while user wakers run |
| Cooperative budget | 128 ready operations per task poll (shared with `akku::net`) | a receiver fed faster than it consumes cannot starve siblings or I/O |

### API rationale

1. **`channel` is MPMC, not MPSC.** With a mutex-protected queue a cloneable receiver costs one extra `WaitList`, and it serves worker pools without an extra `AsyncMutex<Receiver>` (Go channels, async-channel, flume, crossbeam are all MPMC). The name avoids `mpsc` and `mpmc` (§Namespace 4).
2. **`bounded(0)` is a rendezvous channel** instead of a panic (tokio `bounded.rs:128-131`, async-channel `lib.rs:135-136`), because `std::sync::mpsc::sync_channel(0)`, Go unbuffered channels (`chan.go:10-18`), and crossbeam (`channel.rs:109-110`) all define it that way, and the design supports it directly: a blocked sender keeps its value in its waiter slot until a receiver takes it. `try_send`/`reserve` differ from crossbeam on `bounded(0)` for the reason in "Rendezvous" step 5.
3. **Blocked senders keep their value in the waiter slot; receivers get claims, not values.** This gives FIFO among senders, an exact rendezvous at capacity 0, and on close each queued sender gets its own value back in `SendError`. Because a message never moves into a receiver slot, a receiver dropped unpolled has nothing to put back: no phantom item, no `capacity + 1` overshoot.
4. **Direct hand-off everywhere** (lock ownership, permits, `notify_one` tokens, channel claims) gives predictable FIFO and makes `try_*` unable to barge. Go's normal mode (`mutex.go:33-55`) and async-lock (`mutex.rs:22-24`) trade fairness for throughput under contention; akku prefers the simpler, testable guarantee. Atomic fast paths can be added later without changing semantics.
5. **`recv` returns `Result<T, RecvError>`** like `std::sync::mpsc::Receiver::recv` and async-channel, not tokio's `Option<T>`.
6. **Lock `try_*` returns `Option`** (async-lock `mutex.rs:164`): without poisoning there is exactly one failure reason, and it avoids an akku type named like `std::sync::TryLockError`.
7. **Owned guards and permits are first-class** (`lock_owned`, `acquire_owned`): `spawn` requires `'static`, so a permit moved into a spawned connection handler must own an `Arc`.
8. **Strict budget participation** mirrors tokio (`sync/mod.rs:440-446`, `chan.rs:310-348`, `batch_semaphore.rs:659-667`) and reuses the exact mechanism `akku::net` already ships, so all ready operations share one 128-unit budget per poll.
9. **`watch::Ref` wraps a `std::sync::RwLockReadGuard`** so `send_modify` mutates in place without `T: Clone`, at the cost of `Ref: !Send` (which conveniently stops spawned tasks from holding it across `.await`).

### Seams

| Seam | Catches | Misses |
|---|---|---|
| `std::sync` re-exports (type identity: `akku::sync::Mutex<u8>` is `std::sync::Mutex<u8>`) | missing or redefined std item | items stabilized after 1.85 |
| `AsyncMutex` / `AsyncRwLock` lock, owned, try, blocking, `get_mut`, `into_inner` | lost ownership, FIFO violations, writer starvation, barging `try_*`, leaked value on panic/abort | performance under heavy contention |
| `Semaphore` acquire, many, owned, try, blocking, `add_permits`, `forget_permits`, `close` | permit leaks, head-of-line violations, overflow, close races | — |
| `Notify` + `Notified::enable` | lost `notify_one`, wrong epoch semantics, forwarding on drop | — |
| `AsyncOnceCell` | double initialization, stuck gate after cancel/panic, `set` races | — |
| `oneshot` | lost value on cancel, close/drop semantics, poll-after-completion panic | — |
| `channel` bounded(0/1/N/MAX), unbounded, reserve/Permit, `poll_reserve`, close, disconnect, `poll_recv` | lost or duplicated messages, order, capacity off-by-one, rendezvous phantom items or early `send` completion, stuck wake-only registrations, lost wakeups, deadlock on drop of `T` | — |
| `broadcast` | lag accounting, ordering, value drop timing, receiver churn, user `Clone` under the lock | — |
| `watch` | missed or spurious changes, version after close, `send_modify` panic | — |
| Futures under `akku::task` (`block_on`, `spawn`, `abort`, `Runtime` drop) and under a hand-driven poll with counting wakers | cancellation paths, budget yields, cross-thread wakes | real multi-threaded executor (Phase 4) |
| `Send`/`Sync` assertions, `const fn` statics | auto-trait regressions | — |

### G3 test plan (adversarial)

Tests live in `crates/akku/src/sync/tests.rs` and use only the public API. PR-A carries the items for its primitives (re-exports, `AsyncMutex`, `Semaphore`, `Notify`, `oneshot`, `channel` except `poll_recv`); PR-B adds the `AsyncRwLock`, `AsyncOnceCell`, `broadcast`, `watch`, and `poll_recv` parts of each item. Hand-driven interleavings poll futures with `Waker::noop()` or a test `CountingWaker` (`Arc<AtomicUsize>` behind `std::task::Wake`) and `std::pin::pin!`. This is a "loom without loom": each test fixes one exact schedule and asserts the state after every step.

1. **Drop while waiting (every primitive in the cancellation table).** Queue A then B, drop A before it is granted, release, assert B is granted (A's slot leaves no hole). Queue A then B, grant A, drop A unpolled, assert B is now woken and granted (forwarding). `Semaphore`: head asks 5 of 3 available (partial), drop head, assert the 3 permits reach the next waiter and `available_permits()` is exact. `Notify`: `notify_one` handed to A, drop A, assert B completes; with no B, assert the permit is stored (a later `notified()` is ready at once). `channel` recv: grant receiver A a claim, drop A unpolled, assert receiver B (or, with no B, the next `try_recv`) gets the **same** front message, order is preserved, and `len()` never exceeded `capacity`.
2. **Completed future dropped after slot reuse (ABA).** A completes, its slot is reused by B, then A's future is dropped: B is unaffected.
3. **Lost wakeups.** Register then release in every order: release before first poll, between polls, from another `std::thread` while the runtime is parked; 1,000 cross-thread ping-pong rounds over `channel`, `oneshot`, `Notify`, and `AsyncMutex` with a watchdog (`std::sync::mpsc::Receiver::recv_timeout`) so a hang fails instead of blocking CI.
4. **Wake storms and spurious polls.** Poll one queued future 10,000 times with changing wakers: exactly one grant later, no duplicate queue entries (a second waiter is still granted next). `notify_waiters` and broadcast `send` with 10,000 waiters: every one woken exactly once. A waker that panics in `wake` during `notify_waiters`: every other waiter is still woken, the panic surfaces to the caller, and the primitive stays usable.
5. **Fairness.** N lock waiters granted in queue order; `try_lock` fails while anyone is queued; RwLock: reader holds, writer queues, a new `read()` queues behind the writer and `try_read` fails, release grants the writer first, then the reader batch; Semaphore head-of-line blocking of small requests behind a large one.
6. **Close semantics.** `channel::close` returns `true` once, then `false`; queued senders get `SendError` with their exact values; receivers drain then get `RecvError`; `send` after close returns the value; last sender dropped vs explicit close vs last receiver dropped (buffered values dropped exactly once, counted with a drop counter, after the lock: a message that owns a `Sender` of the same channel must not deadlock). `Semaphore::close` wakes all waiters with `AcquireError`, held permits still drop cleanly, `try_acquire` returns `Closed` even with permits available; **`close()` while the head waiter holds a partial grant** (`new(3)`, `acquire_many(5)` polled once) → the head returns `Err(AcquireError)` and `available_permits() == 3` again. `oneshot`: sender dropped → `RecvError`; `close` → `send` fails with the value and `closed()` completes; value sent before `close` is still received. `watch`: sender dropped → `changed()` returns `Ok` for an unseen version, then `Err`. `broadcast`: all senders dropped → receivers drain, then `Closed`.
7. **Capacity bounds.** `bounded(0)` per "Rendezvous" steps 1–7: `send` stays `Pending` until a receiver's `recv` returns its value, then completes; **(i)** a receiver granted a claim and dropped unpolled → the sender is still pending, `len() == 1` (the queued send), no `try_recv` succeeds without a queued sender, and the next receiver gets the value; a queued `send` cancelled after a claim was granted → the claimed receiver re-waits (spurious wake) and receives nothing; `try_send` is `Full` even while a `recv` is queued; **(ii)** `try_reserve` is `Full(())`, `reserve`/`poll_reserve` panic (`expected = "rendezvous"`); close returns queued senders' values. `bounded(1)`: second `try_send` is `Full`. `bounded(usize::MAX)`: thousands of sends without abort, `capacity() == Some(usize::MAX)`. `Semaphore::new(0)`, `new(MAX_PERMITS)` ok, `new(MAX_PERMITS + 1)`, `add_permits` overflow, `acquire_many(MAX_PERMITS + 1)` panic (`#[should_panic(expected = "MAX_PERMITS")]`); `acquire_many(0)` completes at once and fails once closed. **Unsatisfiable head:** `new(4)`, `acquire_many(5)` queued, then `acquire()` queued behind it → both stay `Pending` (even though 4 permits exist and `try_acquire` is `NoPermits`) until `close()` (both `Err(AcquireError)`, permits back to 4) or `add_permits(1)` (head completes, then the second waiter after release). `broadcast::channel(0)` panics (`expected = "capacity"`); `channel(1)` lag: two sends → `Lagged(1)` then the newest value; `channel(usize::MAX)` does not preallocate.
8. **Budget.** A task in `loop { rx.recv().await }` fed by an always-ahead producer thread over `unbounded()`, and a task in `loop { m.lock().await; }` on an uncontended mutex, both let a sibling task set a flag within 4,096 operations (the `akku::net` G5 bound). `try_*` never yields.
9. **Blocking variants.** `blocking_recv`/`blocking_send`/`blocking_lock`/`blocking_acquire` from `std::thread`s against async tasks; calling any of them inside `task::block_on` panics (`expected = "blocking"`). The three documented non-detected cases (no driver thread, foreign executor, panic-in-unwinding abort) are rustdoc-only; G3 does not test the hang or abort.
10. **Task integration.** A task holding an `AsyncMutexGuard` is aborted → the lock is free; a panicking task holding a guard → `JoinError::is_panic` and the lock is free (no poisoning); `Runtime` dropped while tasks wait on a lock held by the root → afterwards a new runtime can lock it; `AsyncOnceCell` initializer cancelled (abort) and panicking → the next caller initializes; concurrent `get_or_init` from 64 tasks runs `init` once.
11. **Multi-thread stress (no loom).** 8 `std::thread`s × 10,000 operations, each thread driving its own `task::block_on`: semaphore conservation (`available + held == initial` at quiescence), mutex counter exact, MPMC every message received exactly once (sum and multiset check), broadcast every receiver sees an increasing sequence with `Lagged` gaps that add up, watch last value observed.
12. **Types.** `Send`/`Sync` assertions for every public type and for the futures of `lock`, `acquire`, `send`, `recv`, `changed`; `static` initialization via `const fn new` (`AsyncMutex`, `Semaphore`, `Notify`, `AsyncOnceCell`); `akku::sync::Arc` is `std::sync::Arc`. **`AsyncRwLock` bounds (Q6b):** static assertions that `AsyncRwLock<T>` is `Send + Sync` for `T: Send + Sync`, plus a negative check that `AsyncRwLock<Cell<u8>>` is **not** `Send` (unlike `std::sync::RwLock<Cell<u8>>`, which is), asserted at runtime through the stable autoref-specialization probe (an inherent `is_send()` on `Probe<T>` gated by `T: Send` shadows a fallback trait method returning `false`), so no compile-fail harness is needed; and `AsyncMutex<Cell<u8>>` is `Send + Sync`.
13. **Guard leaks.** `mem::forget` on an `AsyncMutexGuard`, an `AsyncRwLock` read guard, and a write guard: `try_lock`/`try_write` stay `None`, `get_mut`/`into_inner` panic with `expected = "forgotten"`. `AsyncMutex` address stability: `&*guard as *const T` is equal across two consecutive `lock()`s.
14. **`AsyncRwLock` read-guard drop order.** A writer queued behind N readers on 8 threads; readers drop concurrently while the writer is granted; 10,000 iterations: the writer's `DerefMut` never hits the invariant panic. Deterministic variant: one reader, one queued writer; drop the reader; the writer's first `DerefMut` succeeds.
15. **Wake-only registrations.** `poll_recv` registered with `Pending`, then a FIFO `recv` is queued, then one message is sent: the FIFO `recv` gets it, `poll_recv`'s handle is woken only when an unclaimed message exists, and a handle that never polls again does not block a later `recv`. Same for `poll_reserve` versus `reserve` on `bounded(1)`.
16. **`broadcast` clone outside the lock.** A `T` whose `Clone` impl calls `send`/`subscribe` on the same channel does not deadlock; a `Clone` that panics propagates and the next `recv` returns the following value.
17. **`AsyncOnceCell` re-entrancy** is rustdoc-only (a deadlock is not testable without a watchdog thread); G3 adds one watchdog test that `init` re-entering `get_or_init` on a **different** cell works.
18. **Phase 4 (added to the Phase 4 PR, listed here so it is not lost).** `in_runtime()` is `true` on every worker thread and `false` on `spawn_blocking` threads (so `blocking_recv` works there); a guard or permit dropped during `Runtime::drop` (task cancelled at shutdown while holding an `AsyncMutexGuard` and a `SemaphoreOwnedPermit`) does not panic and leaves both primitives usable from another runtime.

### Open questions (as proposed; all resolved in §Decisions (G2))

1. **Async MPSC name.** Recommend `akku::sync::channel` as an **MPMC** channel (`bounded`, `unbounded`, `Sender`, `Receiver`). Alternatives: `chan`, `queue`. `mpsc`/`mpmc` are forbidden.
2. **`oneshot` vs the unstable `std::sync::oneshot`.** Recommend keeping `akku::sync::oneshot` (human plan, tokio familiarity) and recording a rule in `_dev/TODO.md`: when `std::sync::oneshot` is stable *and* the MSRV reaches it, `akku::sync::oneshot` becomes a std superset (re-exporting std's blocking items) and akku's async halves move to `AsyncSender`/`AsyncReceiver` with a non-std constructor name, a `0.0.x` breaking change. Alternative: avoid the risk now by folding oneshot into `channel` (`channel::oneshot()` → `OneshotSender`/`OneshotReceiver`), which is collision-proof but less familiar.
3. **Capacity 0.** Recommend rendezvous semantics (std `sync_channel(0)`, Go, crossbeam) over tokio's and async-channel's panic.
4. **Fairness.** Recommend strict FIFO hand-off for every primitive (tokio) over Go's starvation mode or async-lock's 0.5 ms eventual fairness.
5. **Poisoning.** Recommend none for async locks; the re-exported `std::sync::Mutex` keeps std's poisoning.
6. **Guard storage.** Recommend inline value-moving guards (`T: Sized`, two moves per lock cycle, `const fn new`, no allocation) over `Box<T>` storage (pointer moves only, but an allocation per mutex and no `const fn new`).
7. **Owned guard and permit names.** Recommend the `akku::net` pattern (`AsyncMutexOwnedGuard`, `AsyncRwLockOwnedReadGuard`, `SemaphoreOwnedPermit`) over tokio's (`OwnedMutexGuard`, `OwnedSemaphorePermit`).
8. **`try_lock` return type.** Recommend `Option<Guard>` (no clash with `std::sync::TryLockError`, one failure reason).
9. **Blocking variants.** Recommend shipping `blocking_*` (prefix, not suffix) for locks, semaphore, and channel receive/send, hidden on `wasm32`, panicking inside a runtime thread; add the `blocking_` rule to `Namespace.md` §3.
10. **Cooperative budget.** Recommend that every waitable sync operation spends the shared budget, which needs two crate-internal edits in `akku::task`: drop the `wasm32` gates on `poll_budget`/`consume_budget` and add `pub(crate) fn in_runtime()`. Both land in the `lib/sync` PR, reviewed with `task` as owner.
11. **v1 scope.** Recommend v1 = everything in §G2 including `AsyncOnceCell`, `channel::reserve`/`Permit`, and `channel::Receiver::poll_recv`. Defer `AsyncBarrier` (tokio's is not cancel safe, `barrier.rs:122-125`; a cancel-safe one would withdraw the arrival on drop), `WaitGroup`, mapped guards, `downgrade`, weak senders, `recv_many`, `reserve_many`, owned channel permits, and blocking `recv_timeout` to `_dev/TODO.md`.
12. **DoS policy.** Recommend no hard waiter cap, shrink-on-empty queues, lazy buffer growth, and `MAX_PERMITS = usize::MAX >> 3`.
13. **Future types.** Recommend `async fn` methods (as `akku::net`), with `Notified` and `oneshot::Receiver` as the only named futures. Alternative: named `*Future` types everywhere (as `akku::io`) for `Unpin` storage in structs; `poll_recv` covers the main need.
14. **Unified bounded/unbounded types.** Recommend one `Sender`/`Receiver` pair for both (async-channel) over tokio's separate `UnboundedSender`; on an unbounded channel `try_send` is the synchronous send.
15. **`broadcast` clone under lock.** Recommend cloning `T` under the internal lock (documented restriction: `Clone` must not use the same channel) over storing `Arc<T>` per value (an extra allocation per `send`).
16. **`ONCE_INIT`.** Recommend re-exporting it with `#[allow(deprecated)]` so the superset stays exact (verified against `#![deny(warnings)]`).

## Decisions (G2)

Orcha + independent Opus review, 2026-11-26 (sketches compiled on Rust 1.85.1). These decisions supersede the recommendations above where they differ, and §G2 already reflects them.

| # | Topic | Decision | Where applied |
|---|---|---|---|
| Q1 | Async MPSC name | **Accepted:** `akku::sync::channel`, MPMC. Add `#[doc(alias = "mpsc")]` on the module and document that the module `channel` and the functions `oneshot::channel`/`broadcast::channel` live in different namespaces. | §Namespace 4, §G2 `channel` |
| Q2 | `oneshot` vs unstable `std::sync::oneshot` | **Accepted:** keep `akku::sync::oneshot`. The migration rule lives in `Namespace.md` §3 (e) as a recorded exception. | `Namespace.md` §3 (e), §Namespace 6 |
| Q3 | Capacity 0 | **Accepted with specification:** rendezvous. (i) A receiver handed a claim and dropped unpolled passes the claim on; the value never leaves the sender slot, so there is no phantom buffered item and `send` does not complete. (ii) `reserve`/`poll_reserve` panic and `try_reserve`/`try_send` return `Full` on `bounded(0)`. | §G2 `channel` "Rendezvous", "Receiver grants" |
| Q4 | Fairness | **Accepted:** strict FIFO hand-off; lock convoys are documented. | §Shared conventions 2 |
| Q5 | Poisoning | **Accepted:** none for async locks. | §Non-goals, §Shared conventions 4 |
| Q6 | Guard storage | **Changed:** `AsyncMutex` state `Inline(T) \| Boxed(Box<T>) \| Lent`. `const fn new` stores `Inline`, the first lock boxes once outside the internal lock, and after that only the `Box` moves (~16 B guards, stable address). `AsyncRwLock` keeps the value permanently in one `Arc<T>`: readers clone it, the writer moves it out and uses `Arc::get_mut`, and a failure is an invariant panic. Read guards drop their `Arc` clone before taking the state lock, and this is tested. The `mem::forget` behaviour is documented, and mapped guards remain possible later as a fn-pointer projection. | §G2 "Guards without `unsafe`", `AsyncMutex`, `AsyncRwLock`, G3 items 13–14 |
| Q6b | `AsyncRwLock` auto traits | **Corrected:** `AsyncRwLock<T>` is `Send`/`Sync` only if `T: Send + Sync` (differs from std); documented and statically asserted. | §Shared conventions 8, G3 item 12 |
| Q7 | Owned guard/permit names | **Accepted:** `AsyncMutexOwnedGuard`, `AsyncRwLockOwnedReadGuard`, `SemaphoreOwnedPermit`. | §Namespace 2 |
| Q8 | `try_lock` return type | **Accepted:** `Option<Guard>`. | §Shared conventions 5 |
| Q9 | Blocking variants | **Accepted** (`blocking_` prefix, now `Namespace.md` §3 (d)). The three deadlock/abort cases are documented: no background workers, foreign executors not detected, and a guard panic inside a `Drop` during unwinding aborts. | §Shared conventions 6, `Namespace.md` §3 (d) |
| Q10 | Cooperative budget | **Accepted:** un-gate `poll_budget`/`consume_budget`, add `pub(crate) fn in_runtime()`. | §Dependencies |
| Q11 | Scope and delivery | **Changed:** G3/G4 split into **PR-A** (std re-exports, `WaitList`, `AsyncMutex`, `Semaphore`, `Notify`, `oneshot`, `channel` incl. `Sender::poll_reserve` for Phase 5 h1 backpressure) and **PR-B** (`AsyncRwLock`, `AsyncOnceCell`, `broadcast`, `watch`, `poll_recv`). Deferred items as recommended. | §G2 "Delivery: PR-A and PR-B" |
| Q12 | DoS policy | **Accepted, plus notes:** the slab keeps its peak size while non-empty, and slots hold blocked senders' `T` inline. | §G2 "Safe waiter queue", DoS table |
| Q13 | Future types | **Accepted:** `async fn`, plus named `Notified` and `oneshot::Receiver`; `poll_reserve`/`poll_recv` for poll-based code. | §Shared conventions 1 |
| Q14 | Unified bounded/unbounded types | **Accepted.** | §G2 `channel` |
| Q15 | `broadcast` clone | **Changed:** slots store `Arc<T>`; `T::clone` runs after unlocking (no user code under the lock). | §G2 `broadcast`, §Locking discipline 2, G3 item 16 |
| Q16 | `ONCE_INIT` | **Accepted:** re-export with `#[allow(deprecated)]`. | §G2 re-exports |
| M1 | `poll_recv` (must-fix) | Registrations are wake-only and never receive message hand-offs or claims; the same applies to `poll_reserve`. | §G2 `channel` "Wake-only registrations", G3 item 15 |
| M2 | Semaphore edge cases | Documented and tested: (a) `acquire_many(n)` larger than all permits ever available stalls the waiters behind it until `close()`; (b) `close()` with a partially granted head returns those permits. | §G2 `Semaphore`, G3 items 6–7 |
| M3 | Server patterns | Latched shutdown via `watch::channel(false)`; connection draining via a `Sender` clone per connection + `closed()`/`recv` until `WaitGroup` exists; cancellation token added to `_dev/TODO.md`. | §G2 "Patterns", `_dev/TODO.md` |
| M4 | Phase 4 tests | `in_runtime()` true on workers and false on `spawn_blocking` threads; a guard dropped during `Runtime::drop` does not panic. | G3 item 18 |
| M5 | `AsyncOnceCell` re-entrancy | An `init` re-entering `get_or_init` on the same cell deadlocks; documented. | §G2 `AsyncOnceCell` |
