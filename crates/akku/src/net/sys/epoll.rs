//! Linux and Android `epoll` + `eventfd` readiness backend.

#![allow(unsafe_code)]

use std::ffi::{c_int, c_uint, c_void};
use std::fs::File;
use std::io::{self, Read, Write};
use std::net::SocketAddr;
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use super::{Interest, ReadyEvent};

pub(crate) const EPOLL_CLOEXEC: c_int = 0x80000;
pub(crate) const EPOLL_CTL_ADD: c_int = 1;
pub(crate) const EPOLL_CTL_DEL: c_int = 2;

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
pub(crate) const EINTR: c_int = 4;

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

/// Reserved token of the `eventfd` wakeup registration.
const TOKEN_WAKEUP: u64 = 0;

/// Converts a `-1` syscall return into the current `errno` error.
fn cvt(ret: c_int) -> io::Result<c_int> {
    if ret < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(ret)
    }
}

pub(crate) struct Poller {
    epfd: OwnedFd,
    wake: File,
    /// Coalesces `unpark` calls until the wakeup is drained in `wait`.
    notified: AtomicBool,
    /// Pre-zeroed kernel event buffer (`max_events` entries, never resized).
    buf: Mutex<Vec<EpollEvent>>,
}

impl Poller {
    pub(crate) fn new(max_events: usize) -> io::Result<Self> {
        // SAFETY: `epoll_create1` takes a plain flag word and has no pointer
        // arguments.
        let raw = cvt(unsafe { epoll_create1(EPOLL_CLOEXEC) })?;
        // SAFETY: `raw` is a fresh, valid descriptor (checked `>= 0` above)
        // that nothing else owns; `OwnedFd` takes sole ownership and closes it.
        let epfd = unsafe { OwnedFd::from_raw_fd(raw) };

        // SAFETY: `eventfd` takes an initial counter and flags; no pointers.
        let raw = cvt(unsafe { eventfd(0, EFD_CLOEXEC | EFD_NONBLOCK) })?;
        // SAFETY: `raw` is a fresh, valid descriptor (checked `>= 0` above)
        // that nothing else owns; `File` takes sole ownership and closes it.
        let wake = unsafe { File::from_raw_fd(raw) };

        let poller = Self {
            epfd,
            wake,
            notified: AtomicBool::new(false),
            buf: Mutex::new(vec![
                EpollEvent { events: 0, data: 0 };
                max_events.clamp(1, c_int::MAX as usize)
            ]),
        };
        poller.ctl(
            EPOLL_CTL_ADD,
            poller.wake.as_raw_fd(),
            EPOLLIN | EPOLLET,
            TOKEN_WAKEUP,
        )?;
        Ok(poller)
    }

    /// `epoll_ctl(ADD | MOD)` for `fd` with `events` and `token`.
    fn ctl(&self, op: c_int, fd: c_int, events: u32, token: u64) -> io::Result<()> {
        let mut event = EpollEvent {
            events,
            data: token,
        };
        // SAFETY: `self.epfd` is an open epoll descriptor owned by `self`; the
        // caller passes an fd that is open for the duration of the call
        // (borrowed from a live socket or our own eventfd); `event` is a valid,
        // initialized `EpollEvent` that outlives the call.
        cvt(unsafe { epoll_ctl(self.epfd.as_raw_fd(), op, fd, &mut event) })?;
        Ok(())
    }

    pub(crate) fn register(&self, fd: BorrowedFd<'_>, token: u64) -> io::Result<()> {
        self.ctl(
            EPOLL_CTL_ADD,
            fd.as_raw_fd(),
            EPOLLIN | EPOLLOUT | EPOLLRDHUP | EPOLLET,
            token,
        )
    }

    pub(crate) fn deregister(&self, fd: BorrowedFd<'_>, _token: u64) -> io::Result<()> {
        // SAFETY: `self.epfd` is an open epoll descriptor owned by `self`;
        // `fd` is borrowed from a live socket, so it is open for the call;
        // `EPOLL_CTL_DEL` ignores the event pointer, so null is allowed
        // (Linux >= 2.6.9).
        cvt(unsafe {
            epoll_ctl(
                self.epfd.as_raw_fd(),
                EPOLL_CTL_DEL,
                fd.as_raw_fd(),
                std::ptr::null_mut(),
            )
        })?;
        Ok(())
    }

    /// Edge-triggered registrations stay armed for both directions.
    pub(crate) fn arm(&self, _token: u64, _interest: Interest) {}

    pub(crate) fn wait(
        &self,
        events: &mut Vec<ReadyEvent>,
        timeout: Option<Duration>,
    ) -> io::Result<()> {
        let mut buf = self.buf.lock().unwrap_or_else(|e| e.into_inner());
        let capacity = c_int::try_from(buf.len()).unwrap_or(c_int::MAX);
        // SAFETY: `self.epfd` is an open epoll descriptor; `buf` is an
        // initialized, exclusively borrowed slice of at least `capacity`
        // `EpollEvent`s, so the kernel writes only inside it.
        let n = unsafe {
            epoll_wait(
                self.epfd.as_raw_fd(),
                buf.as_mut_ptr(),
                capacity,
                super::timeout_ms(timeout),
            )
        };
        let n = match cvt(n) {
            Ok(n) => n as usize,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => return Ok(()),
            Err(e) => return Err(e),
        };
        let mut woken = false;
        for event in &buf[..n.min(buf.len())] {
            // Copy the fields out by value: `EpollEvent` is packed on x86_64.
            let (flags, token) = (event.events, event.data);
            if token == TOKEN_WAKEUP {
                woken = true;
                continue;
            }
            events.push(ReadyEvent {
                token,
                readable: flags & EPOLLIN != 0,
                writable: flags & EPOLLOUT != 0,
                read_closed: flags & (EPOLLRDHUP | EPOLLHUP) != 0,
                write_closed: flags & EPOLLHUP != 0,
                error: flags & EPOLLERR != 0,
            });
        }
        if woken {
            // Drain first, then clear: an `unpark` racing with this either
            // sees `notified == true` (and its work is checked by the caller
            // after this turn) or writes a fresh wakeup.
            self.drain_wake();
            self.notified.store(false, Ordering::SeqCst);
        }
        Ok(())
    }

    fn drain_wake(&self) {
        let mut counter = [0u8; 8];
        loop {
            match (&self.wake).read(&mut counter) {
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                _ => return,
            }
        }
    }

    pub(crate) fn unpark(&self) {
        if self.notified.swap(true, Ordering::SeqCst) {
            return;
        }
        let one = 1u64.to_ne_bytes();
        loop {
            match (&self.wake).write(&one) {
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    // Counter saturated: drain it, then retry.
                    self.drain_wake();
                }
                Err(_) => {
                    // Let a later `unpark` try again instead of losing every wakeup.
                    self.notified.store(false, Ordering::SeqCst);
                    return;
                }
                Ok(_) => return,
            }
        }
    }
}

/// Starts a non-blocking TCP connect; returns `(stream, connected_now)`.
pub(crate) fn connect_nonblocking(addr: SocketAddr) -> io::Result<(std::net::TcpStream, bool)> {
    let domain = match addr {
        SocketAddr::V4(_) => AF_INET,
        SocketAddr::V6(_) => AF_INET6,
    };
    // SAFETY: `socket` takes plain integer arguments and has no pointers.
    let raw = cvt(unsafe { socket(domain, SOCK_STREAM | SOCK_NONBLOCK | SOCK_CLOEXEC, 0) })?;
    // SAFETY: `raw` is a fresh, valid descriptor (checked `>= 0` above) that
    // nothing else owns; `OwnedFd` takes sole ownership and closes it on error.
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };

    let ret = match addr {
        SocketAddr::V4(a) => {
            let sa = SockAddrIn {
                sin_family: AF_INET as u16,
                sin_port: a.port().to_be(),
                sin_addr: a.ip().octets(),
                sin_zero: [0; 8],
            };
            // SAFETY: `fd` is an open socket owned above; the pointer refers to
            // an initialized `SockAddrIn` on this stack frame, and `addrlen` is
            // its exact size, so the kernel reads only inside it.
            unsafe {
                connect(
                    fd.as_raw_fd(),
                    (&sa as *const SockAddrIn).cast::<c_void>(),
                    std::mem::size_of::<SockAddrIn>() as u32,
                )
            }
        }
        SocketAddr::V6(a) => {
            let sa = SockAddrIn6 {
                sin6_family: AF_INET6 as u16,
                sin6_port: a.port().to_be(),
                sin6_flowinfo: a.flowinfo(),
                sin6_addr: a.ip().octets(),
                sin6_scope_id: a.scope_id(),
            };
            // SAFETY: `fd` is an open socket owned above; the pointer refers to
            // an initialized `SockAddrIn6` on this stack frame, and `addrlen`
            // is its exact size, so the kernel reads only inside it.
            unsafe {
                connect(
                    fd.as_raw_fd(),
                    (&sa as *const SockAddrIn6).cast::<c_void>(),
                    std::mem::size_of::<SockAddrIn6>() as u32,
                )
            }
        }
    };
    let connected = if ret == 0 {
        true
    } else {
        let err = io::Error::last_os_error();
        match err.raw_os_error() {
            // EINTR on a non-blocking connect: the handshake continues.
            Some(EINPROGRESS) | Some(EINTR) => false,
            _ => return Err(err),
        }
    };
    Ok((std::net::TcpStream::from(fd), connected))
}
