//! Single-value, single-use async channel.
//!
//! Create a pair with [`channel`]: the [`Sender`] sends at most one value, and
//! the [`Receiver`] implements [`Future`] directly to receive it.
//!
//! Note that the function [`oneshot::channel`](channel) lives in this module's
//! value namespace and does not conflict with the sibling module
//! [`akku::sync::channel`](crate::sync::channel).

// G3 stub allows — remove in G4.
#![allow(dead_code, unused_variables)]

use std::error::Error;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex as StdMutex};
use std::task::{Context, Poll};

/// Creates a new one-shot channel, returning the sending and receiving halves.
pub fn channel<T>() -> (Sender<T>, Receiver<T>) {
    todo!()
}

/// Sending half of a [`oneshot`](self) channel.
///
/// Can send at most one value via [`send`](Self::send). Dropping the sender
/// without sending causes the [`Receiver`] to resolve with [`RecvError`].
pub struct Sender<T> {
    inner: Arc<StdMutex<Option<T>>>,
}

impl<T> Sender<T> {
    /// Sends `value` to the associated [`Receiver`].
    ///
    /// Fails with [`SendError`] containing `value` if the receiver was already
    /// dropped or closed via [`Receiver::close`].
    pub fn send(self, value: T) -> Result<(), SendError<T>> {
        todo!()
    }

    /// Completes when the associated [`Receiver`] is dropped or closed.
    ///
    /// Cancellation-safe.
    pub async fn closed(&mut self) {
        todo!()
    }

    /// Returns `true` if the associated [`Receiver`] has been dropped or closed.
    pub fn is_closed(&self) -> bool {
        todo!()
    }
}

impl<T> fmt::Debug for Sender<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        todo!()
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        todo!()
    }
}

/// Receiving half of a [`oneshot`](self) channel.
///
/// Implements [`Future`] directly (and is [`Unpin`]): awaiting `rx` or
/// `&mut rx` resolves to `Ok(value)` when the [`Sender`] sends, or
/// `Err(RecvError)` if the sender is dropped without sending. Awaiting
/// `&mut rx` is cancellation-safe; dropping the borrow leaves any sent value
/// intact in the channel.
///
/// # Panics
///
/// Polling a `Receiver` again after it has returned `Poll::Ready` panics
/// (`"polled after completion"`).
#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct Receiver<T> {
    inner: Arc<StdMutex<Option<T>>>,
}

impl<T> Receiver<T> {
    /// Attempts to receive the value immediately without waiting.
    ///
    /// Returns [`TryRecvError::Empty`] if the sender is still alive and has not
    /// sent yet, or [`TryRecvError::Closed`] if the sender dropped without
    /// sending (or the value was already received).
    pub fn try_recv(&mut self) -> Result<T, TryRecvError> {
        todo!()
    }

    /// Prevents any future [`Sender::send`] from succeeding and wakes any task
    /// awaiting [`Sender::closed`].
    ///
    /// A value already sent before `close` was called can still be received via
    /// [`try_recv`](Self::try_recv) or `.await`.
    pub fn close(&mut self) {
        todo!()
    }

    /// Blocks the current OS thread until a value is received or the [`Sender`]
    /// is dropped.
    ///
    /// # Panics
    ///
    /// Panics if called from a thread currently driving an `akku::task`
    /// runtime (`block_on` or a spawned task poll). Note that `akku::task` has
    /// no background worker threads, foreign executors are not detected, and
    /// calling this from a `Drop` impl during unwinding on a runtime thread
    /// aborts the process.
    #[cfg(not(target_arch = "wasm32"))]
    #[track_caller]
    pub fn blocking_recv(self) -> Result<T, RecvError> {
        todo!()
    }
}

impl<T> Future for Receiver<T> {
    type Output = Result<T, RecvError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        todo!()
    }
}

impl<T> fmt::Debug for Receiver<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        todo!()
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        todo!()
    }
}

/// Error returned by [`Sender::send`] when the [`Receiver`] was closed or dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendError<T>(
    /// The unsent value returned to the caller.
    pub T,
);

impl<T> fmt::Display for SendError<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        todo!()
    }
}

impl<T: fmt::Debug> Error for SendError<T> {}

/// Error returned when the [`Sender`] dropped without sending a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecvError;

impl fmt::Display for RecvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        todo!()
    }
}

impl Error for RecvError {}

/// Error returned by [`Receiver::try_recv`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TryRecvError {
    /// The [`Sender`] is still alive and has not sent a value yet.
    Empty,
    /// The [`Sender`] was dropped without sending, or the value was already received.
    Closed,
}

impl fmt::Display for TryRecvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        todo!()
    }
}

impl Error for TryRecvError {}
