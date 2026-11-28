//! Safe fallback for native targets without a Tier-1 readiness backend.
//!
//! The runtime parks on a `Condvar` (so `akku::task` works everywhere), while
//! socket registration and non-blocking connect return
//! `io::ErrorKind::Unsupported`.

use std::io;
use std::net::SocketAddr;
use std::time::Duration;

use super::{Handle, Interest, ReadyEvent};
use crate::task::Parker;

fn unsupported() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "akku::net has no async socket backend for this target",
    )
}

pub(crate) struct Poller {
    parker: Parker,
}

impl Poller {
    pub(crate) fn new(_max_events: usize) -> io::Result<Self> {
        Ok(Self {
            parker: Parker::new(),
        })
    }

    pub(crate) fn register(&self, _handle: Handle<'_>, _token: u64) -> io::Result<()> {
        Err(unsupported())
    }

    pub(crate) fn deregister(&self, _handle: Handle<'_>, _token: u64) -> io::Result<()> {
        Ok(())
    }

    pub(crate) fn arm(&self, _token: u64, _interest: Interest) {}

    pub(crate) fn wait(
        &self,
        _events: &mut Vec<ReadyEvent>,
        timeout: Option<Duration>,
    ) -> io::Result<()> {
        self.parker.turn(timeout)
    }

    pub(crate) fn unpark(&self) {
        self.parker.unpark();
    }
}

pub(crate) fn connect_nonblocking(_addr: SocketAddr) -> io::Result<(std::net::TcpStream, bool)> {
    Err(unsupported())
}

#[cfg(all(test, unix))]
pub(crate) fn reset_sigpipe_to_default() {}
