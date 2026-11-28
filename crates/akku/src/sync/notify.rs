//! Single-permit and broadcast async task notification.

// G3 stub allows — remove in G4.
#![allow(dead_code, unused_variables)]

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex as StdMutex;
use std::task::{Context, Poll};

/// Wakes a single waiting task (storing at most one permit when nobody waits)
/// or all currently waiting tasks.
///
/// [`notify_one`](Self::notify_one) hands a permit to the oldest queued waiter
/// in FIFO order, or stores a single permit if no waiter is queued (repeated
/// calls coalesce to one stored permit). [`notify_waiters`](Self::notify_waiters)
/// completes every [`Notified`] future created before that call and stores no
/// permit.
///
/// Cancellation-safe: dropping a [`Notified`] future that already received a
/// `notify_one` permit forwards that permit to the next queued waiter (or
/// stores it if none remains). A `notify_waiters` wakeup is never forwarded.
pub struct Notify {
    inner: StdMutex<()>,
}

impl Notify {
    /// Creates a new `Notify` with no stored permit.
    ///
    /// This constructor is `const` and performs no heap allocation.
    pub const fn new() -> Self {
        Self {
            inner: StdMutex::new(()),
        }
    }

    /// Creates a [`Notified`] future that completes after receiving a
    /// [`notify_one`](Self::notify_one) permit or a subsequent
    /// [`notify_waiters`](Self::notify_waiters) call.
    ///
    /// The future captures the `notify_waiters` epoch at creation time, before
    /// it is ever polled.
    pub fn notified(&self) -> Notified<'_> {
        todo!()
    }

    /// Hands a notification permit to the oldest queued waiter, or stores one
    /// permit if no waiter is currently queued.
    ///
    /// At most one permit is stored; additional calls while a permit is already
    /// stored have no further effect.
    pub fn notify_one(&self) {
        todo!()
    }

    /// Completes every [`Notified`] future created before this call.
    ///
    /// Unlike [`notify_one`](Self::notify_one), this method does not store a
    /// permit for futures created afterwards.
    pub fn notify_waiters(&self) {
        todo!()
    }
}

impl Default for Notify {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for Notify {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        todo!()
    }
}

/// Future returned by [`Notify::notified`].
///
/// This future is [`Unpin`], [`Send`], and [`Sync`]. Dropping it before
/// observing a [`Notify::notify_one`] permit forwards that permit to the next
/// waiter (or stores it if no waiter remains).
#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct Notified<'a> {
    notify: &'a Notify,
}

impl Notified<'_> {
    /// Registers this future in the waiter queue without a waker so a
    /// subsequent [`Notify::notify_one`] is handed to it even before its first
    /// poll.
    ///
    /// Returns `true` if the future is already notified (either by a stored
    /// permit or a `notify_waiters` call after this future was created).
    pub fn enable(&mut self) -> bool {
        todo!()
    }
}

impl Future for Notified<'_> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        todo!()
    }
}

impl fmt::Debug for Notified<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        todo!()
    }
}

impl Drop for Notified<'_> {
    fn drop(&mut self) {
        todo!()
    }
}
