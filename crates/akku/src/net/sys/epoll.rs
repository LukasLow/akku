//! Linux and Android `epoll` + `eventfd` readiness backend.

#![allow(unsafe_code)]
#![allow(dead_code, unused_variables)]

use std::ffi::{c_int, c_uint, c_void};
use std::io;
use std::net::SocketAddr;
use std::os::fd::BorrowedFd;
use std::time::Duration;

use super::{Interest, ReadyEvent};

pub(crate) const EPOLL_CLOEXEC: c_int = 0x80000;
pub(crate) const EPOLL_CTL_ADD: c_int = 1;
pub(crate) const EPOLL_CTL_DEL: c_int = 2;
pub(crate) const EPOLL_CTL_MOD: c_int = 3;

pub(crate) const EPOLLIN: u32 = 0x1;
pub(crate) const EPOLLOUT: u32 = 0x4;
pub(crate) const EPOLLERR: u32 = 0x8;
pub(crate) const EPOLLHUP: u32 = 0x10;
pub(crate) const EPOLLRDHUP: u32 = 0x2000;
pub(crate) const EPOLLET: u32 = 0x8000_0000;

pub(crate) const EFD_CLOEXEC: c_int = 0x80000;
pub(crate) const EFD_NONBLOCK: c_int = 0x800;

pub(crate) const AF_INET: c_int = 2;
pub(crate) const AF_INET6: c_int = 10;
pub(crate) const SOCK_STREAM: c_int = 1;
pub(crate) const SOCK_NONBLOCK: c_int = 0x800;
pub(crate) const SOCK_CLOEXEC: c_int = 0x80000;
pub(crate) const EINPROGRESS: c_int = 115;

#[cfg_attr(target_arch = "x86_64", repr(packed))]
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct EpollEvent {
    pub(crate) events: u32,
    pub(crate) data: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct SockAddrIn {
    pub(crate) sin_family: u16,
    pub(crate) sin_port: u16,
    pub(crate) sin_addr: [u8; 4],
    pub(crate) sin_zero: [u8; 8],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct SockAddrIn6 {
    pub(crate) sin6_family: u16,
    pub(crate) sin6_port: u16,
    pub(crate) sin6_flowinfo: u32,
    pub(crate) sin6_addr: [u8; 16],
    pub(crate) sin6_scope_id: u32,
}

// Linux & Android x86_64 — packed: 12 bytes, no padding.
#[cfg(target_arch = "x86_64")]
const _: () = {
    assert!(core::mem::size_of::<EpollEvent>() == 12);
    assert!(core::mem::align_of::<EpollEvent>() == 1);
    assert!(core::mem::offset_of!(EpollEvent, events) == 0);
    assert!(core::mem::offset_of!(EpollEvent, data) == 4);
};

// Linux & Android 32-bit x86 (glibc, musl, Android) — natural i386 alignment: 12 bytes, align 4.
#[cfg(target_arch = "x86")]
const _: () = {
    assert!(core::mem::size_of::<EpollEvent>() == 12);
    assert!(core::mem::align_of::<EpollEvent>() == 4);
    assert!(core::mem::offset_of!(EpollEvent, events) == 0);
    assert!(core::mem::offset_of!(EpollEvent, data) == 4);
};

// Linux & Android aarch64, arm, riscv64 — natural 8-byte u64 alignment: 16 bytes.
#[cfg(any(target_arch = "aarch64", target_arch = "arm", target_arch = "riscv64"))]
const _: () = {
    assert!(core::mem::size_of::<EpollEvent>() == 16);
    assert!(core::mem::align_of::<EpollEvent>() == 8);
    assert!(core::mem::offset_of!(EpollEvent, events) == 0);
    assert!(core::mem::offset_of!(EpollEvent, data) == 8);
};

const _: () = {
    assert!(core::mem::size_of::<SockAddrIn>() == 16);
    assert!(core::mem::offset_of!(SockAddrIn, sin_family) == 0);
    assert!(core::mem::offset_of!(SockAddrIn, sin_port) == 2);
    assert!(core::mem::offset_of!(SockAddrIn, sin_addr) == 4);

    assert!(core::mem::size_of::<SockAddrIn6>() == 28);
    assert!(core::mem::offset_of!(SockAddrIn6, sin6_family) == 0);
    assert!(core::mem::offset_of!(SockAddrIn6, sin6_port) == 2);
    assert!(core::mem::offset_of!(SockAddrIn6, sin6_flowinfo) == 4);
    assert!(core::mem::offset_of!(SockAddrIn6, sin6_addr) == 8);
    assert!(core::mem::offset_of!(SockAddrIn6, sin6_scope_id) == 24);
};

// SAFETY: every declaration matches the platform C ABI signature (argument and
// return widths per DESIGN.md §Dependencies); callers uphold each function's
// pointer and descriptor preconditions at the call site.
unsafe extern "C" {
    fn epoll_create1(flags: c_int) -> c_int;
    fn epoll_ctl(epfd: c_int, op: c_int, fd: c_int, event: *mut EpollEvent) -> c_int;
    fn epoll_wait(epfd: c_int, events: *mut EpollEvent, maxevents: c_int, timeout: c_int) -> c_int;
    fn eventfd(initval: c_uint, flags: c_int) -> c_int;
    fn socket(domain: c_int, ty: c_int, protocol: c_int) -> c_int;
    fn connect(sockfd: c_int, addr: *const c_void, addrlen: u32) -> c_int;
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
