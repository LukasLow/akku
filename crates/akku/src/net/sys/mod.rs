//! Platform readiness multiplexer backends for `akku::net`.
//!
//! Each Tier-1 backend lives in its own allowlisted submodule and is gated to
//! its target OS and ABI-verified architectures (`_dev/DESIGN.md`
//! §Dependencies). Every backend exposes the same crate-internal surface:
//!
//! - `Poller::{new, register, deregister, arm, wait, unpark}`, all `&self`;
//! - `connect_nonblocking(addr) -> (std::net::TcpStream, connected_now)`.
//!
//! Other native targets get the safe [`fallback`] below: the runtime still
//! parks on a `Condvar`, and socket registration returns
//! `io::ErrorKind::Unsupported`.

#[cfg(all(
    any(target_os = "linux", target_os = "android"),
    any(
        target_arch = "x86_64",
        target_arch = "aarch64",
        target_arch = "x86",
        target_arch = "arm",
        target_arch = "riscv64",
    )
))]
mod epoll;
#[cfg(all(
    any(target_os = "linux", target_os = "android"),
    any(
        target_arch = "x86_64",
        target_arch = "aarch64",
        target_arch = "x86",
        target_arch = "arm",
        target_arch = "riscv64",
    )
))]
pub(crate) use epoll::{connect_nonblocking, Poller};

#[cfg(all(
    any(target_os = "macos", target_os = "ios"),
    any(target_arch = "aarch64", target_arch = "x86_64")
))]
mod kqueue;
#[cfg(all(
    any(target_os = "macos", target_os = "ios"),
    any(target_arch = "aarch64", target_arch = "x86_64")
))]
pub(crate) use kqueue::{connect_nonblocking, Poller};

#[cfg(all(
    target_os = "windows",
    any(target_arch = "x86_64", target_arch = "aarch64", target_arch = "x86")
))]
mod wsapoll;
#[cfg(all(
    target_os = "windows",
    any(target_arch = "x86_64", target_arch = "aarch64", target_arch = "x86")
))]
pub(crate) use wsapoll::{connect_nonblocking, Poller};

#[cfg(not(any(
    all(
        any(target_os = "linux", target_os = "android"),
        any(
            target_arch = "x86_64",
            target_arch = "aarch64",
            target_arch = "x86",
            target_arch = "arm",
            target_arch = "riscv64",
        )
    ),
    all(
        any(target_os = "macos", target_os = "ios"),
        any(target_arch = "aarch64", target_arch = "x86_64")
    ),
    all(
        target_os = "windows",
        any(target_arch = "x86_64", target_arch = "aarch64", target_arch = "x86")
    ),
)))]
mod fallback;
#[cfg(not(any(
    all(
        any(target_os = "linux", target_os = "android"),
        any(
            target_arch = "x86_64",
            target_arch = "aarch64",
            target_arch = "x86",
            target_arch = "arm",
            target_arch = "riscv64",
        )
    ),
    all(
        any(target_os = "macos", target_os = "ios"),
        any(target_arch = "aarch64", target_arch = "x86_64")
    ),
    all(
        target_os = "windows",
        any(target_arch = "x86_64", target_arch = "aarch64", target_arch = "x86")
    ),
)))]
pub(crate) use fallback::{connect_nonblocking, Poller};

/// Borrowed OS socket handle passed to `register` / `deregister`.
#[cfg(unix)]
pub(crate) type Handle<'a> = std::os::fd::BorrowedFd<'a>;
/// Borrowed OS socket handle passed to `register` / `deregister`.
#[cfg(windows)]
pub(crate) type Handle<'a> = std::os::windows::io::BorrowedSocket<'a>;

/// Readiness interest direction for reactor arming.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Interest {
    pub(crate) readable: bool,
    pub(crate) writable: bool,
}

/// Readiness event reported by an OS poller turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReadyEvent {
    pub(crate) token: u64,
    pub(crate) readable: bool,
    pub(crate) writable: bool,
    pub(crate) read_closed: bool,
    pub(crate) write_closed: bool,
    pub(crate) error: bool,
}

/// Converts a `Duration` timeout to whole milliseconds, rounding up so a short
/// non-zero timeout never becomes a busy `0`, and capping at `i32::MAX`.
#[cfg_attr(
    not(any(target_os = "linux", target_os = "android", target_os = "windows")),
    allow(dead_code)
)]
pub(crate) fn timeout_ms(timeout: Option<std::time::Duration>) -> i32 {
    match timeout {
        None => -1,
        Some(d) => {
            let ms = d.as_nanos().div_ceil(1_000_000);
            i32::try_from(ms).unwrap_or(i32::MAX)
        }
    }
}
