# task — backlog

## Multi-threaded scheduler & blocking pool (Phase 4)
- `RuntimeBuilder` (`new_current_thread`, `new_multi_thread`, `worker_threads`, `thread_name`) — configurable runtime construction with per-worker `Mutex<VecDeque<Arc<Task>>>` + `try_lock` half-batch stealing (100% safe). (origin: `docs/forks/tokio-axum.md` §5B, §8 Phase 4)
- `spawn_blocking` / `try_spawn_blocking` — bounded OS thread pool for blocking DNS resolution, `akku::fs` async helpers, and CPU-heavy work. (origin: `docs/forks/tokio-axum.md` §8 Phase 4 / `tokio::task::spawn_blocking`)
- `#[akku::task::main]` — proc macro in `crates/akku-drive` expanding to `akku::task::block_on`. (origin: `Namespace.md` §Macros / `docs/forks/tokio-axum.md` §9.5)
- `Handle` (`Runtime::handle`, `Handle::current`, `Handle::spawn`, `Handle::enter`) — cloneable handle for spawning tasks from non-runtime threads. (origin: `tokio::runtime::Handle`)

## Cancellation & structured concurrency
- `AbortHandle` — cloneable handle allowing cancellation without owning `JoinHandle<T>`. (origin: `tokio::task::AbortHandle`)
- `JoinSet<T>` — dynamic set of spawned tasks that cancels remaining tasks on drop and yields results as they finish. (origin: `tokio::task::JoinSet`)
- `CancelToken` — hierarchical cooperative cancellation token for graceful server shutdown. (origin: `docs/forks/tokio-axum.md` §3 / Go `context.WithCancel`)
- `join!` / `try_join!` / `select!` — minimal safe combinators for concurrent polling on a single task. (origin: `docs/forks/tokio-axum.md` §3–§4)

## Local tasks & ergonomics
- `spawn_local` / `LocalSet` — run `!Send` futures pinned to a single thread. (origin: `DESIGN.md` §Non-goals / `tokio::task::LocalSet`, `async_executor::LocalExecutor`)
- `TaskId` (`JoinError::id`, `task::try_id`) — opaque monotonic task identifier for diagnostics and logging. (origin: `tokio::task::Id`, `async_std::task::TaskId`)
- `task_local!` — task-scoped context storage (e.g. request IDs for `akku::log` / `akku::trace`). (origin: `tokio::task_local!`, `async_std::task_local!`)

## Platform extensions
- `wasm32` `spawn` bridge (`#[cfg(target_arch = "wasm32")]`) — schedule futures onto the JS microtask queue without affecting native code paths. (origin: `CONCEPT.md` §6 / `docs/forks/tokio-axum.md` §8 Phase 7)
- Timer driver hook for `akku::time` (`sleep`, `sleep_until`, `interval`, `timeout`) — internal park timeout integration with the `akku::time` timer wheel. (origin: `docs/forks/tokio-axum.md` §8 Phase 3)
