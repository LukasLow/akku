//! Platform readiness multiplexer backends for `akku::net`.
//!
//! Each Tier-1 backend lives in its own allowlisted submodule and is gated to
//! its target OS and ABI-verified architectures (`_dev/DESIGN.md`
//! §Dependencies). Targets outside these gates get no backend yet; the G4
//! fallback returns `io::ErrorKind::Unsupported`.

#![allow(dead_code, unused_variables)]

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
pub(crate) mod epoll;

#[cfg(all(
    any(target_os = "macos", target_os = "ios"),
    any(target_arch = "aarch64", target_arch = "x86_64")
))]
pub(crate) mod kqueue;

#[cfg(all(
    target_os = "windows",
    any(target_arch = "x86_64", target_arch = "aarch64", target_arch = "x86")
))]
pub(crate) mod wsapoll;

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
