# sync — backlog

Deferred candidates from the G1/G2 proposal, as decided in DESIGN.md §Decisions (G2). v1 ships in PR-A and PR-B (DESIGN.md §G2 "Delivery").

## `std::sync` parity (MSRV-gated)
- `ReentrantLock`, `ReentrantLockGuard`, `MappedMutexGuard`, `MappedRwLockReadGuard`, `MappedRwLockWriteGuard`, `Exclusive`/`SyncView`, `UniqueArc`, `mpmc`, `poison`, `nonpoison` — re-export once stable and covered by the MSRV. (origin: DESIGN.md §References / `library/std/src/sync/mod.rs`)
- `std::sync::oneshot` — on stabilization past the MSRV, turn `akku::sync::oneshot` into a std superset and rename the async halves. (origin: DESIGN.md Decisions Q2 / `Namespace.md` §3 (e))

## More primitives
- `AsyncBarrier` + `AsyncBarrierWaitResult` — cancel-safe barrier that withdraws an arrival when its `wait` future drops. (origin: DESIGN.md Decisions Q11 / tokio `sync/barrier.rs:122-125`)
- `WaitGroup` — Go-style counter with `add`, `done`, `wait`; replaces the "`Sender` clone per connection + `closed()`" draining pattern. (origin: Go `sync/waitgroup.go:77,160,236`, DESIGN.md §G2 "Patterns")
- Cancellation token — cloneable, hierarchical, latched shutdown signal (`cancel`, `is_cancelled`, `cancelled().await`, child tokens); replaces the `watch::channel(false)` pattern. Home to be decided between `akku::sync` and `akku::task` (`task/_dev/TODO.md` lists `CancelToken`). (origin: DESIGN.md Decisions M3 / Go `context.WithCancel`)
- `AsyncCondvar` — async condition variable over `AsyncMutexGuard`. (origin: Go `sync/cond.go`, Python `asyncio.Condition`)
- `AsyncLazy` — async counterpart of `LazyLock` over `AsyncOnceCell`. (origin: DESIGN.md §G2 `AsyncOnceCell`)

## Locks and permits
- Mapped guards (`AsyncMutexGuard::map`, `try_map`) as guard + stored `fn(&mut T) -> &mut U` projection, and `AsyncRwLockWriteGuard::downgrade`. (origin: tokio `sync/mutex.rs:869,918`, DESIGN.md §G2 "Guards without `unsafe`")
- Opt-in throughput mode for `AsyncMutex` (Go starvation-mode style: barging allowed until a waiter waits > 1 ms) if lock convoys show up in benchmarks. (origin: DESIGN.md Decisions Q4 / Go `internal/sync/mutex.go:33-55`)
- `SemaphorePermit::{merge, split}`. (origin: tokio `sync/semaphore.rs:1230,1260`)
- `Notify::notify_last` (LIFO) and `Notify::notified_owned`. (origin: tokio `sync/notify.rs:610,670`)
- Atomic fast paths (`AtomicUsize` CAS for uncontended `try_acquire`/`try_lock`), benchmark-driven, still 0 `unsafe`. (origin: DESIGN.md §Non-goals)

## Channels
- `Sender::downgrade` / `WeakSender`. (origin: tokio `sync/mpsc/bounded.rs:56`, async-channel `lib.rs:528`)
- `Receiver::recv_many(&mut Vec<T>, limit)` and `Sender::reserve_many(n)`. (origin: tokio `sync/mpsc/bounded.rs:319,1177`)
- `Sender::reserve_owned` / `OwnedPermit`. (origin: tokio `sync/mpsc/bounded.rs:1265`)
- `Sender::force_send` (drop oldest when full). (origin: async-channel `lib.rs:352`)
- Blocking `recv_timeout` / `recv_deadline` on channel and oneshot receivers. (origin: std `mpsc/mod.rs:909,970`)
- `Stream`-style adapter once akku has a `Stream` trait. (origin: `docs/forks/tokio-axum.md` §4 "minimal `Stream`")
- `watch::Sender: Clone` and `broadcast::Receiver::poll_recv` (wake-only, like `channel::Receiver::poll_recv`). (origin: DESIGN.md §G2)
- Lossless send on `bounded(0)` (a `reserve`-like API whose completion waits for the receiver's take). (origin: DESIGN.md Decisions Q3)
