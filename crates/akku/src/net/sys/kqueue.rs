//! macOS and iOS (64-bit `aarch64` / `x86_64`) `kqueue` + `EVFILT_USER` backend.

#![allow(unsafe_code)]

use std::ffi::{c_int, c_void};
use std::io;
use std::net::SocketAddr;
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use super::{Interest, ReadyEvent};

pub(crate) const EVFILT_READ: i16 = -1;
pub(crate) const EVFILT_WRITE: i16 = -2;
pub(crate) const EVFILT_USER: i16 = -10;

pub(crate) const EV_ADD: u16 = 0x1;
pub(crate) const EV_DELETE: u16 = 0x2;
pub(crate) const EV_CLEAR: u16 = 0x20;
pub(crate) const EV_RECEIPT: u16 = 0x40;
pub(crate) const EV_ERROR: u16 = 0x4000;
pub(crate) const EV_EOF: u16 = 0x8000;

pub(crate) const NOTE_TRIGGER: u32 = 0x0100_0000;

pub(crate) const F_SETFD: c_int = 2;
pub(crate) const FD_CLOEXEC: c_int = 1;

pub(crate) const AF_INET: c_int = 2;
pub(crate) const AF_INET6: c_int = 30;
pub(crate) const SOCK_STREAM: c_int = 1;
pub(crate) const SOL_SOCKET: c_int = 0xffff;
pub(crate) const SO_NOSIGPIPE: c_int = 0x1022;
pub(crate) const EINPROGRESS: c_int = 36;
pub(crate) const EINTR: c_int = 4;
pub(crate) const ENOENT: c_int = 2;
pub(crate) const EPIPE: c_int = 32;

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

/// `ident` and `udata` of the `EVFILT_USER` wakeup event.
const WAKE_IDENT: usize = 0;
const TOKEN_WAKEUP: usize = 0;

/// Darwin rejects `tv_sec > 1e8` with `EINVAL`; Go clamps to `1e6`.
const MAX_TIMEOUT_SECS: u64 = 1_000_000;

const ZERO_KEVENT: Kevent = Kevent {
    ident: 0,
    filter: 0,
    flags: 0,
    fflags: 0,
    data: 0,
    udata: 0,
};

/// Converts a `-1` syscall return into the current `errno` error.
fn cvt(ret: c_int) -> io::Result<c_int> {
    if ret < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(ret)
    }
}

pub(crate) struct Poller {
    kq: OwnedFd,
    /// Coalesces `unpark` calls until the wakeup is received in `wait`.
    notified: AtomicBool,
    /// Pre-zeroed kernel event buffer (`max_events` entries, never resized).
    buf: Mutex<Vec<Kevent>>,
}

impl Poller {
    pub(crate) fn new(max_events: usize) -> io::Result<Self> {
        // SAFETY: `kqueue` takes no arguments.
        let raw = cvt(unsafe { kqueue() })?;
        // SAFETY: `raw` is a fresh, valid descriptor (checked `>= 0` above)
        // that nothing else owns; `OwnedFd` takes sole ownership and closes it.
        let kq = unsafe { OwnedFd::from_raw_fd(raw) };
        set_cloexec(kq.as_raw_fd())?;
        let poller = Self {
            kq,
            notified: AtomicBool::new(false),
            buf: Mutex::new(vec![ZERO_KEVENT; max_events.clamp(1, c_int::MAX as usize)]),
        };
        let wake = Kevent {
            ident: WAKE_IDENT,
            filter: EVFILT_USER,
            flags: EV_ADD | EV_CLEAR | EV_RECEIPT,
            fflags: 0,
            data: 0,
            udata: TOKEN_WAKEUP,
        };
        poller.submit(&mut [wake], &[])?;
        Ok(poller)
    }

    /// Applies `changes` with `EV_RECEIPT` and checks every receipt, ignoring
    /// the errno values listed in `ignore`.
    fn submit(&self, changes: &mut [Kevent], ignore: &[i32]) -> io::Result<()> {
        let n = c_int::try_from(changes.len())
            .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
        let list = changes.as_mut_ptr();
        loop {
            // SAFETY: `self.kq` is an open kqueue owned by `self`; `list`
            // points to `n` initialized `Kevent`s used both as the changelist
            // (read) and, with `EV_RECEIPT`, as the eventlist for exactly `n`
            // receipts (written); a null timeout is allowed because receipts
            // return immediately.
            let ret = unsafe { kevent(self.kq.as_raw_fd(), list, n, list, n, std::ptr::null()) };
            match cvt(ret) {
                Ok(_) => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
        for receipt in changes.iter() {
            if receipt.flags & EV_ERROR != 0 && receipt.data != 0 {
                let errno = receipt.data as i32;
                if !ignore.contains(&errno) {
                    return Err(io::Error::from_raw_os_error(errno));
                }
            }
        }
        Ok(())
    }

    pub(crate) fn register(&self, fd: BorrowedFd<'_>, token: u64) -> io::Result<()> {
        let ident = fd.as_raw_fd() as usize;
        let flags = EV_ADD | EV_CLEAR | EV_RECEIPT;
        let event = |filter| Kevent {
            ident,
            filter,
            flags,
            fflags: 0,
            data: 0,
            udata: token as usize,
        };
        // EPIPE: the peer already closed the write side (mio does the same).
        self.submit(&mut [event(EVFILT_READ), event(EVFILT_WRITE)], &[EPIPE])
    }

    pub(crate) fn deregister(&self, fd: BorrowedFd<'_>, _token: u64) -> io::Result<()> {
        let ident = fd.as_raw_fd() as usize;
        let event = |filter| Kevent {
            ident,
            filter,
            flags: EV_DELETE | EV_RECEIPT,
            fflags: 0,
            data: 0,
            udata: 0,
        };
        self.submit(&mut [event(EVFILT_READ), event(EVFILT_WRITE)], &[ENOENT])
    }

    /// `EV_CLEAR` registrations stay armed for both directions.
    pub(crate) fn arm(&self, _token: u64, _interest: Interest) {}

    pub(crate) fn wait(
        &self,
        events: &mut Vec<ReadyEvent>,
        timeout: Option<Duration>,
    ) -> io::Result<()> {
        let spec = timeout.map(|d| Timespec {
            tv_sec: d.as_secs().min(MAX_TIMEOUT_SECS) as i64,
            tv_nsec: i64::from(d.subsec_nanos()),
        });
        let spec_ptr = spec
            .as_ref()
            .map_or(std::ptr::null(), |s| s as *const Timespec);
        let mut buf = self.buf.lock().unwrap_or_else(|e| e.into_inner());
        let capacity = c_int::try_from(buf.len()).unwrap_or(c_int::MAX);
        // SAFETY: `self.kq` is an open kqueue; no changes are submitted; `buf`
        // is an initialized, exclusively borrowed slice of at least `capacity`
        // `Kevent`s, so the kernel writes only inside it; `spec_ptr` is null
        // or points to `spec`, which outlives the call.
        let n = unsafe {
            kevent(
                self.kq.as_raw_fd(),
                std::ptr::null(),
                0,
                buf.as_mut_ptr(),
                capacity,
                spec_ptr,
            )
        };
        let n = match cvt(n) {
            Ok(n) => n as usize,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => return Ok(()),
            Err(e) => return Err(e),
        };
        let mut woken = false;
        for event in &buf[..n.min(buf.len())] {
            if event.filter == EVFILT_USER {
                woken = true;
                continue;
            }
            let eof = event.flags & EV_EOF != 0;
            let error = event.flags & EV_ERROR != 0;
            let read = event.filter == EVFILT_READ;
            let write = event.filter == EVFILT_WRITE;
            events.push(ReadyEvent {
                token: event.udata as u64,
                readable: read,
                writable: write,
                read_closed: read && eof,
                write_closed: write && eof,
                error,
            });
        }
        if woken {
            // `EV_CLEAR` resets the trigger on retrieval; clear the flag after
            // it so a racing `unpark` either triggers again or is covered by
            // the caller's post-turn work check.
            self.notified.store(false, Ordering::SeqCst);
        }
        Ok(())
    }

    pub(crate) fn unpark(&self) {
        if self.notified.swap(true, Ordering::SeqCst) {
            return;
        }
        let trigger = Kevent {
            ident: WAKE_IDENT,
            filter: EVFILT_USER,
            // Keep the `EV_ADD | EV_CLEAR` registration; only fire it.
            flags: EV_RECEIPT,
            fflags: NOTE_TRIGGER,
            data: 0,
            udata: TOKEN_WAKEUP,
        };
        if self.submit(&mut [trigger], &[]).is_err() {
            // Let a later `unpark` try again instead of losing every wakeup.
            self.notified.store(false, Ordering::SeqCst);
        }
    }
}

/// Sets `FD_CLOEXEC` on `fd` (Darwin has no `SOCK_CLOEXEC` / `kqueue1`).
fn set_cloexec(fd: c_int) -> io::Result<()> {
    // SAFETY: `fd` is an open descriptor owned by the caller for the duration
    // of the call; `F_SETFD` takes one `c_int` argument.
    cvt(unsafe { fcntl(fd, F_SETFD, FD_CLOEXEC) })?;
    Ok(())
}

/// Starts a non-blocking TCP connect; returns `(stream, connected_now)`.
pub(crate) fn connect_nonblocking(addr: SocketAddr) -> io::Result<(std::net::TcpStream, bool)> {
    let domain = match addr {
        SocketAddr::V4(_) => AF_INET,
        SocketAddr::V6(_) => AF_INET6,
    };
    // SAFETY: `socket` takes plain integer arguments and has no pointers.
    let raw = cvt(unsafe { socket(domain, SOCK_STREAM, 0) })?;
    // SAFETY: `raw` is a fresh, valid descriptor (checked `>= 0` above) that
    // nothing else owns; `OwnedFd` takes sole ownership and closes it on error.
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    set_cloexec(fd.as_raw_fd())?;
    let one: c_int = 1;
    // SAFETY: `fd` is an open socket owned above; `&one` points to an
    // initialized `c_int` and `len` is exactly its size.
    cvt(unsafe {
        setsockopt(
            fd.as_raw_fd(),
            SOL_SOCKET,
            SO_NOSIGPIPE,
            (&one as *const c_int).cast::<c_void>(),
            std::mem::size_of::<c_int>() as u32,
        )
    })?;
    // `O_NONBLOCK` through the safe std API (ioctl `FIONBIO`).
    let stream = std::net::TcpStream::from(fd);
    stream.set_nonblocking(true)?;

    let ret = match addr {
        SocketAddr::V4(a) => {
            let sa = SockAddrIn {
                sin_len: std::mem::size_of::<SockAddrIn>() as u8,
                sin_family: AF_INET as u8,
                sin_port: a.port().to_be(),
                sin_addr: a.ip().octets(),
                sin_zero: [0; 8],
            };
            // SAFETY: `stream` owns an open socket; the pointer refers to an
            // initialized `SockAddrIn` on this stack frame, and `len` is its
            // exact size, so the kernel reads only inside it.
            unsafe {
                connect(
                    stream.as_raw_fd(),
                    (&sa as *const SockAddrIn).cast::<c_void>(),
                    std::mem::size_of::<SockAddrIn>() as u32,
                )
            }
        }
        SocketAddr::V6(a) => {
            let sa = SockAddrIn6 {
                sin6_len: std::mem::size_of::<SockAddrIn6>() as u8,
                sin6_family: AF_INET6 as u8,
                sin6_port: a.port().to_be(),
                // Host byte order, exactly as std's `socket_addr_to_c` does,
                // so `peer_addr()` round-trips this value unchanged.
                sin6_flowinfo: a.flowinfo(),
                sin6_addr: a.ip().octets(),
                sin6_scope_id: a.scope_id(),
            };
            // SAFETY: `stream` owns an open socket; the pointer refers to an
            // initialized `SockAddrIn6` on this stack frame, and `len` is its
            // exact size, so the kernel reads only inside it.
            unsafe {
                connect(
                    stream.as_raw_fd(),
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
    Ok((stream, connected))
}
