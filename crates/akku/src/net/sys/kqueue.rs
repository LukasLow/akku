//! macOS and iOS (64-bit `aarch64` / `x86_64`) `kqueue` + `EVFILT_USER` backend.

#![allow(unsafe_code)]
#![allow(dead_code, unused_variables)]

use std::ffi::{c_int, c_void};
use std::io;
use std::net::SocketAddr;
use std::os::fd::BorrowedFd;
use std::time::Duration;

use super::{Interest, ReadyEvent};

pub(crate) const EVFILT_READ: i16 = -1;
pub(crate) const EVFILT_WRITE: i16 = -2;
pub(crate) const EVFILT_USER: i16 = -10;

pub(crate) const EV_ADD: u16 = 0x1;
pub(crate) const EV_DELETE: u16 = 0x2;
pub(crate) const EV_ENABLE: u16 = 0x4;
pub(crate) const EV_ONESHOT: u16 = 0x10;
pub(crate) const EV_CLEAR: u16 = 0x20;
pub(crate) const EV_RECEIPT: u16 = 0x40;
pub(crate) const EV_ERROR: u16 = 0x4000;
pub(crate) const EV_EOF: u16 = 0x8000;

pub(crate) const NOTE_TRIGGER: u32 = 0x0100_0000;

pub(crate) const F_SETFD: c_int = 2;
pub(crate) const F_GETFL: c_int = 3;
pub(crate) const F_SETFL: c_int = 4;
pub(crate) const FD_CLOEXEC: c_int = 1;
pub(crate) const O_NONBLOCK: c_int = 0x4;

pub(crate) const AF_INET: c_int = 2;
pub(crate) const AF_INET6: c_int = 30;
pub(crate) const SOCK_STREAM: c_int = 1;
pub(crate) const SOL_SOCKET: c_int = 0xffff;
pub(crate) const SO_NOSIGPIPE: c_int = 0x1022;
pub(crate) const EINPROGRESS: c_int = 36;

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct Kevent {
    pub(crate) ident: usize,
    pub(crate) filter: i16,
    pub(crate) flags: u16,
    pub(crate) fflags: u32,
    pub(crate) data: isize,
    pub(crate) udata: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct Timespec {
    pub(crate) tv_sec: i64,
    pub(crate) tv_nsec: i64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct SockAddrIn {
    pub(crate) sin_len: u8,
    pub(crate) sin_family: u8,
    pub(crate) sin_port: u16,
    pub(crate) sin_addr: [u8; 4],
    pub(crate) sin_zero: [u8; 8],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct SockAddrIn6 {
    pub(crate) sin6_len: u8,
    pub(crate) sin6_family: u8,
    pub(crate) sin6_port: u16,
    pub(crate) sin6_flowinfo: u32,
    pub(crate) sin6_addr: [u8; 16],
    pub(crate) sin6_scope_id: u32,
}

// Apple 64-bit LP64 (macOS and iOS on aarch64 and x86_64).
const _: () = {
    assert!(core::mem::size_of::<Kevent>() == 32);
    assert!(core::mem::align_of::<Kevent>() == 8);
    assert!(core::mem::offset_of!(Kevent, ident) == 0);
    assert!(core::mem::offset_of!(Kevent, filter) == 8);
    assert!(core::mem::offset_of!(Kevent, flags) == 10);
    assert!(core::mem::offset_of!(Kevent, fflags) == 12);
    assert!(core::mem::offset_of!(Kevent, data) == 16);
    assert!(core::mem::offset_of!(Kevent, udata) == 24);

    assert!(core::mem::size_of::<Timespec>() == 16);
    assert!(core::mem::offset_of!(Timespec, tv_sec) == 0);
    assert!(core::mem::offset_of!(Timespec, tv_nsec) == 8);

    assert!(core::mem::size_of::<SockAddrIn>() == 16);
    assert!(core::mem::offset_of!(SockAddrIn, sin_len) == 0);
    assert!(core::mem::offset_of!(SockAddrIn, sin_family) == 1);
    assert!(core::mem::offset_of!(SockAddrIn, sin_port) == 2);
    assert!(core::mem::offset_of!(SockAddrIn, sin_addr) == 4);

    assert!(core::mem::size_of::<SockAddrIn6>() == 28);
    assert!(core::mem::offset_of!(SockAddrIn6, sin6_len) == 0);
    assert!(core::mem::offset_of!(SockAddrIn6, sin6_family) == 1);
    assert!(core::mem::offset_of!(SockAddrIn6, sin6_port) == 2);
    assert!(core::mem::offset_of!(SockAddrIn6, sin6_flowinfo) == 4);
    assert!(core::mem::offset_of!(SockAddrIn6, sin6_addr) == 8);
    assert!(core::mem::offset_of!(SockAddrIn6, sin6_scope_id) == 24);
};

// SAFETY: every declaration matches the platform C ABI signature (argument and
// return widths per DESIGN.md §Dependencies); callers uphold each function's
// pointer and descriptor preconditions at the call site.
unsafe extern "C" {
    fn kqueue() -> c_int;
    fn kevent(
        kq: c_int,
        changelist: *const Kevent,
        nchanges: c_int,
        eventlist: *mut Kevent,
        nevents: c_int,
        timeout: *const Timespec,
    ) -> c_int;
    fn fcntl(fd: c_int, cmd: c_int, ...) -> c_int;
    fn socket(domain: c_int, ty: c_int, protocol: c_int) -> c_int;
    fn setsockopt(fd: c_int, level: c_int, name: c_int, val: *const c_void, len: u32) -> c_int;
    fn connect(fd: c_int, addr: *const c_void, len: u32) -> c_int;
}

pub(crate) struct Poller {
    _priv: (),
}

impl Poller {
    pub(crate) fn new(max_events: usize) -> io::Result<Self> {
        todo!()
    }

    pub(crate) fn register(&self, fd: BorrowedFd<'_>, token: u64) -> io::Result<()> {
        todo!()
    }

    pub(crate) fn deregister(&self, fd: BorrowedFd<'_>, token: u64) -> io::Result<()> {
        todo!()
    }

    pub(crate) fn arm(&self, token: u64, interest: Interest) {
        todo!()
    }

    // G3 stub: the G4 body pushes into `events`, which silences `ptr_arg`.
    #[allow(clippy::ptr_arg)]
    pub(crate) fn wait(
        &mut self,
        events: &mut Vec<ReadyEvent>,
        timeout: Option<Duration>,
    ) -> io::Result<()> {
        todo!()
    }

    pub(crate) fn unpark(&self) {
        todo!()
    }
}

pub(crate) fn connect_nonblocking(addr: SocketAddr) -> io::Result<(std::net::TcpStream, bool)> {
    todo!()
}
