//! Windows `WSAPoll` + loopback TCP wakeup socketpair readiness backend.

#![allow(unsafe_code)]
#![allow(dead_code, unused_variables)]

use std::ffi::c_void;
use std::io;
use std::net::SocketAddr;
use std::os::windows::io::BorrowedSocket;
use std::time::Duration;

use super::{Interest, ReadyEvent};

pub(crate) const POLLERR: i16 = 0x0001;
pub(crate) const POLLHUP: i16 = 0x0002;
pub(crate) const POLLNVAL: i16 = 0x0004;
pub(crate) const POLLWRNORM: i16 = 0x0010;
pub(crate) const POLLOUT: i16 = 0x0010;
pub(crate) const POLLRDNORM: i16 = 0x0100;
pub(crate) const POLLRDBAND: i16 = 0x0200;
pub(crate) const POLLIN: i16 = 0x0300;

pub(crate) const AF_INET: i32 = 2;
pub(crate) const AF_INET6: i32 = 23;
pub(crate) const SOCK_STREAM: i32 = 1;
pub(crate) const IPPROTO_TCP: i32 = 6;
pub(crate) const FIONBIO: i32 = 0x8004_667eu32 as i32;
pub(crate) const INVALID_SOCKET: usize = usize::MAX;
pub(crate) const SOCKET_ERROR: i32 = -1;
pub(crate) const WSAEWOULDBLOCK: i32 = 10035;
pub(crate) const WSAEINPROGRESS: i32 = 10036;
pub(crate) const WSAEINTR: i32 = 10004;

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct WsaPollFd {
    pub(crate) fd: usize,
    pub(crate) events: i16,
    pub(crate) revents: i16,
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

// 64-bit Windows (x86_64, aarch64): SOCKET is 8 bytes + 2+2 + 4 bytes tail pad = 16 bytes.
#[cfg(target_pointer_width = "64")]
const _: () = {
    assert!(core::mem::size_of::<WsaPollFd>() == 16);
    assert!(core::mem::align_of::<WsaPollFd>() == 8);
    assert!(core::mem::offset_of!(WsaPollFd, fd) == 0);
    assert!(core::mem::offset_of!(WsaPollFd, events) == 8);
    assert!(core::mem::offset_of!(WsaPollFd, revents) == 10);
};

// 32-bit Windows (x86): SOCKET is 4 bytes + 2+2 = 8 bytes.
#[cfg(target_pointer_width = "32")]
const _: () = {
    assert!(core::mem::size_of::<WsaPollFd>() == 8);
    assert!(core::mem::align_of::<WsaPollFd>() == 4);
    assert!(core::mem::offset_of!(WsaPollFd, fd) == 0);
    assert!(core::mem::offset_of!(WsaPollFd, events) == 4);
    assert!(core::mem::offset_of!(WsaPollFd, revents) == 6);
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
#[link(name = "ws2_32")]
unsafe extern "system" {
    fn WSAPoll(fdarray: *mut WsaPollFd, fds: u32, timeout: i32) -> i32;
    fn WSAGetLastError() -> i32;
    fn socket(af: i32, ty: i32, protocol: i32) -> usize;
    fn ioctlsocket(s: usize, cmd: i32, argp: *mut u32) -> i32;
    fn connect(s: usize, name: *const c_void, namelen: i32) -> i32;
}

pub(crate) struct Poller {
    _priv: (),
}

impl Poller {
    pub(crate) fn new(max_events: usize) -> io::Result<Self> {
        todo!()
    }

    pub(crate) fn register(&self, sock: BorrowedSocket<'_>, token: u64) -> io::Result<()> {
        todo!()
    }

    pub(crate) fn deregister(&self, sock: BorrowedSocket<'_>, token: u64) -> io::Result<()> {
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
