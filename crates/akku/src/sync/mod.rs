//! `std::sync` superset with safe async synchronization primitives and channels.
//!
//! Every stable item from [`std::sync`] is re-exported under its exact name and
//! semantics, alongside zero-`unsafe` async primitives that work under
//! `akku::task`, any foreign async executor, and plain OS threads (via
//! `blocking_*` methods).
//!
//! # Overview
//!
//! | Task | API |
//! |---|---|
//! | Standard synchronous primitives (`std::sync`) | [`Arc`], [`Weak`], [`Mutex`], [`RwLock`], [`Condvar`], [`Barrier`], [`Once`], [`OnceLock`], [`LazyLock`], [`atomic`], [`mpsc`] |
//! | Async FIFO mutual-exclusion lock | [`AsyncMutex`], [`AsyncMutexGuard`], [`AsyncMutexOwnedGuard`] |
//! | Async FIFO counting semaphore | [`Semaphore`], [`SemaphorePermit`], [`SemaphoreOwnedPermit`], [`AcquireError`], [`TryAcquireError`] |
//! | Single-permit / broadcast task notification | [`Notify`], [`Notified`] |
//! | Single-value, single-use async channel | [`oneshot::channel`], [`oneshot::Sender`], [`oneshot::Receiver`] |
//! | Async MPMC channel (bounded, rendezvous, unbounded) | [`channel::bounded`], [`channel::unbounded`], [`channel::Sender`], [`channel::Receiver`], [`channel::Permit`] |
//!
//! # Shared contracts
//!
//! - **Strict FIFO hand-off:** released locks, permits, `notify_one` tokens,
//!   and channel slots/claims are handed directly to the oldest queued waiter;
//!   `try_*` calls cannot barge ahead of queued waiters.
//! - **Lossless cancellation:** dropping a waiting future never loses a lock,
//!   permit, `notify_one` token, or accepted channel message. Dropping a queued
//!   [`channel::Sender::send`] drops its unsent value; use
//!   [`channel::Sender::reserve`] for lossless sends under cancellation.
//! - **No poisoning:** async locks do not poison when a task panics while
//!   holding a guard (the re-exported [`Mutex`] and [`RwLock`] keep `std`'s
//!   poisoning unchanged).
//! - **Cooperative budget:** every waitable async operation participates in the
//!   shared `akku::task` cooperative budget (128 ready operations per poll);
//!   `try_*` calls never wait and never touch the budget.
//! - **`blocking_*` methods:** available on non-`wasm32` targets for plain OS
//!   threads. Calling them on a thread currently driving an `akku::task`
//!   runtime panics to prevent self-deadlock. Note that `akku::task` has no
//!   background worker threads, foreign executors are not detected, and calling
//!   a `blocking_*` method inside a `Drop` impl while already unwinding on a
//!   runtime thread aborts the process.
//!
//! # Connection-draining pattern
//!
//! To wait until every active connection task finishes during shutdown, hand
//! each task a clone of a [`channel::Sender`] (which never sends), drop the
//! server's own copy, and await [`channel::Receiver::recv`]:
//!
//! ```no_run
//! use akku::sync::channel;
//! use akku::task;
//!
//! task::block_on(async {
//!     let (drain_tx, drain_rx) = channel::bounded::<()>(1);
//!     let conn_tx = drain_tx.clone();
//!     let _conn = task::spawn(async move {
//!         let _guard = conn_tx;
//!         // Connection work runs here; `_guard` drops when done.
//!     });
//!     drop(drain_tx);
//!     assert!(drain_rx.recv().await.is_err());
//! });
//! ```

#[allow(deprecated)]
// ONCE_INIT is deprecated in std; re-exported so the superset stays exact.
pub use std::sync::{
    atomic, mpsc, Arc, Barrier, BarrierWaitResult, Condvar, LazyLock, LockResult, Mutex,
    MutexGuard, Once, OnceLock, OnceState, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard,
    TryLockError, TryLockResult, WaitTimeoutResult, Weak, ONCE_INIT,
};

#[doc(alias = "mpsc")]
pub mod channel;
mod mutex;
mod notify;
pub mod oneshot;
mod semaphore;
mod wait;

pub use mutex::{AsyncMutex, AsyncMutexGuard, AsyncMutexOwnedGuard};
pub use notify::{Notified, Notify};
pub use semaphore::{
    AcquireError, Semaphore, SemaphoreOwnedPermit, SemaphorePermit, TryAcquireError,
};
