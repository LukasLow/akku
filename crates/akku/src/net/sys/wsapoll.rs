//! Windows `WSAPoll` + loopback TCP wakeup socketpair readiness backend.

#![allow(unsafe_code)]

use std::collections::hash_map::RandomState;
use std::collections::HashMap;
use std::ffi::c_void;
use std::hash::{BuildHasher, Hasher};
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::os::windows::io::{AsRawSocket, BorrowedSocket, FromRawSocket, OwnedSocket, RawSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use super::{Interest, ReadyEvent};

pub(crate) const POLLERR: i16 = 0x0001;
pub(crate) const POLLHUP: i16 = 0x0002;
pub(crate) const POLLNVAL: i16 = 0x0004;
pub(crate) const POLLWRNORM: i16 = 0x0010;
pub(crate) const POLLRDNORM: i16 = 0x0100;
pub(crate) const POLLRDBAND: i16 = 0x0200;

pub(crate) const AF_INET: i32 = 2;
pub(crate) const AF_INET6: i32 = 23;
pub(crate) const SOCK_STREAM: i32 = 1;
pub(crate) const IPPROTO_TCP: i32 = 6;
pub(crate) const INVALID_SOCKET: usize = usize::MAX;
pub(crate) const SOCKET_ERROR: i32 = -1;
pub(crate) const WSAEWOULDBLOCK: i32 = 10035;
pub(crate) const WSAEINPROGRESS: i32 = 10036;
pub(crate) const WSAEINTR: i32 = 10004;
pub(crate) const WSAEINVAL: i32 = 10022;
pub(crate) const WSAEPROTOTYPE: i32 = 10041;
/// `WSASocketW` flags (`winsock2.h`); std uses the same pair.
pub(crate) const WSA_FLAG_OVERLAPPED: u32 = 0x01;
pub(crate) const WSA_FLAG_NO_HANDLE_INHERIT: u32 = 0x80;
/// `SetHandleInformation` mask/flag (`handleapi.h`).
pub(crate) const HANDLE_FLAG_INHERIT: u32 = 0x01;

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
    // `SOCKET WSASocketW(int, int, int, LPWSAPROTOCOL_INFOW, GROUP, DWORD)`;
    // `GROUP` and `DWORD` are `u32`. The protocol-info pointer is always null
    // here, so it is declared as an untyped pointer.
    fn WSASocketW(
        af: i32,
        ty: i32,
        protocol: i32,
        protocol_info: *const c_void,
        group: u32,
        flags: u32,
    ) -> usize;
    fn connect(s: usize, name: *const c_void, namelen: i32) -> i32;
}

// SAFETY: matches `BOOL SetHandleInformation(HANDLE, DWORD, DWORD)` from
// `kernel32` (`BOOL` is `i32`, `HANDLE` a pointer, `DWORD` `u32`), as declared
// by std and `windows-sys`; callers pass an open socket handle.
#[link(name = "kernel32")]
unsafe extern "system" {
    fn SetHandleInformation(handle: *mut c_void, mask: u32, flags: u32) -> i32;
}

const ARM_READ: u8 = 0b01;
const ARM_WRITE: u8 = 0b10;

/// A registered socket and the directions it currently waits for.
struct Entry {
    socket: usize,
    armed: u8,
}

#[derive(Default)]
struct Table {
    entries: HashMap<u64, Entry>,
}

/// Scratch arrays rebuilt on every wait (slot `0` is the wake socket).
#[derive(Default)]
struct Scratch {
    fds: Vec<WsaPollFd>,
    tokens: Vec<u64>,
}

pub(crate) struct Poller {
    wake_reader: TcpStream,
    wake_writer: TcpStream,
    /// Coalesces `unpark` calls until the wake socket is drained in `wait`.
    notified: AtomicBool,
    /// Set while a `wait` may block, so cross-thread `arm` calls interrupt it.
    polling: AtomicBool,
    max_events: usize,
    table: Mutex<Table>,
    scratch: Mutex<Scratch>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// Returns 8 unpredictable bytes without an RNG dependency.
fn nonce() -> [u8; 8] {
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u64(std::process::id().into());
    hasher.finish().to_ne_bytes()
}

/// Builds a connected loopback TCP pair (curl `socketpair.c` approach): the
/// accepted peer must be our own connecting socket, proven by its address and
/// an 8-byte nonce, so another local process cannot hijack the listener.
fn wake_pair() -> io::Result<(TcpStream, TcpStream)> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let addr = listener.local_addr()?;
    let mut writer = TcpStream::connect(addr)?;
    let expected_peer = writer.local_addr()?;
    let token = nonce();
    writer.write_all(&token)?;
    for _ in 0..8 {
        let (mut reader, peer) = listener.accept()?;
        if peer != expected_peer {
            continue; // Somebody else connected first; drop them.
        }
        let mut got = [0u8; 8];
        reader.read_exact(&mut got)?;
        if got != token {
            break;
        }
        reader.set_nonblocking(true)?;
        writer.set_nonblocking(true)?;
        writer.set_nodelay(true)?;
        return Ok((reader, writer));
    }
    Err(io::Error::other(
        "could not establish the reactor wakeup socket pair",
    ))
}

impl Poller {
    pub(crate) fn new(max_events: usize) -> io::Result<Self> {
        // Creating std sockets first also runs std's one-time `WSAStartup`,
        // which every raw Winsock call below relies on.
        let (wake_reader, wake_writer) = wake_pair()?;
        Ok(Self {
            wake_reader,
            wake_writer,
            notified: AtomicBool::new(false),
            polling: AtomicBool::new(false),
            max_events: max_events.max(1),
            table: Mutex::new(Table::default()),
            scratch: Mutex::new(Scratch::default()),
        })
    }

    pub(crate) fn register(&self, sock: BorrowedSocket<'_>, token: u64) -> io::Result<()> {
        let socket = usize::try_from(sock.as_raw_socket())
            .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
        lock(&self.table)
            .entries
            .insert(token, Entry { socket, armed: 0 });
        Ok(())
    }

    pub(crate) fn deregister(&self, _sock: BorrowedSocket<'_>, token: u64) -> io::Result<()> {
        lock(&self.table).entries.remove(&token);
        Ok(())
    }

    /// Adds `interest` to the socket's armed set. `WSAPoll` is
    /// level-triggered, so only directions a task waits for are polled, and
    /// each is disarmed again once delivered (no idle `POLLWRNORM` spin).
    pub(crate) fn arm(&self, token: u64, interest: Interest) {
        let bits = if interest.readable { ARM_READ } else { 0 }
            | if interest.writable { ARM_WRITE } else { 0 };
        let changed = match lock(&self.table).entries.get_mut(&token) {
            Some(entry) if entry.armed & bits != bits => {
                entry.armed |= bits;
                true
            }
            _ => false,
        };
        // A wait already in progress polls a stale set; interrupt it.
        if changed && self.polling.load(Ordering::SeqCst) {
            self.unpark();
        }
    }

    pub(crate) fn wait(
        &self,
        events: &mut Vec<ReadyEvent>,
        timeout: Option<Duration>,
    ) -> io::Result<()> {
        let mut scratch = lock(&self.scratch);
        let Scratch { fds, tokens } = &mut *scratch;
        fds.clear();
        tokens.clear();
        fds.push(WsaPollFd {
            fd: usize::try_from(self.wake_reader.as_raw_socket()).unwrap_or(INVALID_SOCKET),
            events: POLLRDNORM,
            revents: 0,
        });
        tokens.push(0);

        self.polling.store(true, Ordering::SeqCst);
        {
            let table = lock(&self.table);
            for (&token, entry) in &table.entries {
                if entry.armed == 0 {
                    continue;
                }
                let mut mask = 0;
                if entry.armed & ARM_READ != 0 {
                    mask |= POLLRDNORM;
                }
                if entry.armed & ARM_WRITE != 0 {
                    mask |= POLLWRNORM;
                }
                fds.push(WsaPollFd {
                    fd: entry.socket,
                    events: mask,
                    revents: 0,
                });
                tokens.push(token);
            }
        }

        let count = u32::try_from(fds.len()).unwrap_or(u32::MAX);
        // SAFETY: `fds` is an initialized, exclusively borrowed slice of
        // `count >= 1` `WsaPollFd`s (slot 0 is always the wake socket, so
        // `WSAPoll` never sees an empty array); Winsock writes only the
        // `revents` fields inside it.
        let ret = unsafe { WSAPoll(fds.as_mut_ptr(), count, super::timeout_ms(timeout)) };
        self.polling.store(false, Ordering::SeqCst);
        if ret == SOCKET_ERROR {
            // SAFETY: `WSAGetLastError` has no preconditions; it reads the
            // calling thread's last Winsock error.
            let code = unsafe { WSAGetLastError() };
            return if code == WSAEINTR {
                Ok(())
            } else {
                Err(io::Error::from_raw_os_error(code))
            };
        }

        if fds[0].revents != 0 {
            let mut sink = [0u8; 64];
            loop {
                match (&self.wake_reader).read(&mut sink) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
            }
            self.notified.store(false, Ordering::SeqCst);
        }

        let mut table = lock(&self.table);
        let mut delivered = 0;
        for (fd, &token) in fds.iter().zip(tokens.iter()).skip(1) {
            let revents = fd.revents;
            if revents == 0 {
                continue;
            }
            if delivered == self.max_events {
                break; // The rest stay armed and are reported next turn.
            }
            delivered += 1;
            let readable = revents & (POLLRDNORM | POLLRDBAND) != 0;
            let writable = revents & POLLWRNORM != 0;
            let hangup = revents & POLLHUP != 0;
            let error = revents & (POLLERR | POLLNVAL) != 0;
            // `POLLHUP`/`POLLERR`/`POLLNVAL` are reported regardless of the
            // requested events, so they disarm (and wake) both directions.
            let mut disarm = 0;
            if readable || hangup || error {
                disarm |= ARM_READ;
            }
            if writable || hangup || error {
                disarm |= ARM_WRITE;
            }
            if let Some(entry) = table.entries.get_mut(&token) {
                entry.armed &= !disarm;
            }
            events.push(ReadyEvent {
                token,
                readable,
                writable,
                read_closed: hangup,
                write_closed: hangup,
                error,
            });
        }
        Ok(())
    }

    pub(crate) fn unpark(&self) {
        if self.notified.swap(true, Ordering::SeqCst) {
            return;
        }
        // `WouldBlock` means unread wake bytes are pending: already readable.
        if let Err(e) = (&self.wake_writer).write(&[1]) {
            if e.kind() != io::ErrorKind::WouldBlock {
                self.notified.store(false, Ordering::SeqCst);
            }
        }
    }
}

/// Calls `WSASocketW` for a TCP socket with `flags`, adopting the result.
fn wsa_socket(family: i32, flags: u32) -> Result<OwnedSocket, i32> {
    // SAFETY: plain integer arguments plus a null protocol-info pointer,
    // which `WSASocketW` documents as "use the first matching provider";
    // Winsock is initialized because a runtime (and thus the std wake socket
    // pair) exists.
    let raw = unsafe { WSASocketW(family, SOCK_STREAM, IPPROTO_TCP, std::ptr::null(), 0, flags) };
    if raw == INVALID_SOCKET {
        // SAFETY: `WSAGetLastError` has no preconditions.
        return Err(unsafe { WSAGetLastError() });
    }
    // SAFETY: `raw` is a fresh, valid socket (checked `!= INVALID_SOCKET`)
    // that nothing else owns; `OwnedSocket` takes sole ownership and closes
    // it on every later error path.
    Ok(unsafe { OwnedSocket::from_raw_socket(raw as RawSocket) })
}

/// Creates a TCP socket that child processes do not inherit, exactly as std
/// does (`library/std/src/sys/net/connection/socket/windows.rs`, `Socket::new`):
/// `WSA_FLAG_NO_HANDLE_INHERIT` first, and on systems that reject the flag
/// (`WSAEINVAL` / `WSAEPROTOTYPE`) a socket without it whose handle
/// inheritance is cleared with `SetHandleInformation`.
fn new_tcp_socket(family: i32) -> io::Result<OwnedSocket> {
    match wsa_socket(family, WSA_FLAG_OVERLAPPED | WSA_FLAG_NO_HANDLE_INHERIT) {
        Ok(socket) => Ok(socket),
        Err(code) if code == WSAEINVAL || code == WSAEPROTOTYPE => {
            let socket =
                wsa_socket(family, WSA_FLAG_OVERLAPPED).map_err(io::Error::from_raw_os_error)?;
            clear_inherit(&socket)?;
            Ok(socket)
        }
        Err(code) => Err(io::Error::from_raw_os_error(code)),
    }
}

/// Clears `HANDLE_FLAG_INHERIT` on `socket` (std's `set_no_inherit`).
fn clear_inherit(socket: &OwnedSocket) -> io::Result<()> {
    // SAFETY: `socket` is open and borrowed for the duration of the call; a
    // Winsock `SOCKET` is a kernel handle that `SetHandleInformation` accepts
    // (std does the same in `OwnedSocket::set_no_inherit`).
    let ok = unsafe {
        SetHandleInformation(
            socket.as_raw_socket() as usize as *mut c_void,
            HANDLE_FLAG_INHERIT,
            0,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Starts a non-blocking TCP connect; returns `(stream, connected_now)`.
pub(crate) fn connect_nonblocking(addr: SocketAddr) -> io::Result<(TcpStream, bool)> {
    let family = match addr {
        SocketAddr::V4(_) => AF_INET,
        SocketAddr::V6(_) => AF_INET6,
    };
    let stream = TcpStream::from(new_tcp_socket(family)?);
    // `FIONBIO` through the safe std API.
    stream.set_nonblocking(true)?;
    let sock = usize::try_from(stream.as_raw_socket())
        .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;

    let ret = match addr {
        SocketAddr::V4(a) => {
            let sa = SockAddrIn {
                sin_family: AF_INET as u16,
                sin_port: a.port().to_be(),
                sin_addr: a.ip().octets(),
                sin_zero: [0; 8],
            };
            // SAFETY: `sock` is an open socket owned by `stream`; the pointer
            // refers to an initialized `SockAddrIn` on this stack frame, and
            // `namelen` is its exact size, so Winsock reads only inside it.
            unsafe {
                connect(
                    sock,
                    (&sa as *const SockAddrIn).cast::<c_void>(),
                    std::mem::size_of::<SockAddrIn>() as i32,
                )
            }
        }
        SocketAddr::V6(a) => {
            let sa = SockAddrIn6 {
                sin6_family: AF_INET6 as u16,
                sin6_port: a.port().to_be(),
                // Host byte order, exactly as std's `socket_addr_to_c` does,
                // so `peer_addr()` round-trips this value unchanged.
                sin6_flowinfo: a.flowinfo(),
                sin6_addr: a.ip().octets(),
                sin6_scope_id: a.scope_id(),
            };
            // SAFETY: `sock` is an open socket owned by `stream`; the pointer
            // refers to an initialized `SockAddrIn6` on this stack frame, and
            // `namelen` is its exact size, so Winsock reads only inside it.
            unsafe {
                connect(
                    sock,
                    (&sa as *const SockAddrIn6).cast::<c_void>(),
                    std::mem::size_of::<SockAddrIn6>() as i32,
                )
            }
        }
    };
    if ret == 0 {
        return Ok((stream, true));
    }
    // SAFETY: `WSAGetLastError` has no preconditions.
    let code = unsafe { WSAGetLastError() };
    match code {
        WSAEWOULDBLOCK | WSAEINPROGRESS | WSAEINTR => Ok((stream, false)),
        _ => Err(io::Error::from_raw_os_error(code)),
    }
}

#[cfg(test)]
mod tests {
    //! Windows-only checks of handle inheritance, which the public API cannot
    //! observe without spawning child processes.

    use super::*;

    // SAFETY: matches `BOOL GetHandleInformation(HANDLE, LPDWORD)` from
    // `kernel32` (`BOOL` is `i32`, `LPDWORD` a `*mut u32`), as declared by
    // `windows-sys`; the test passes an open handle and a valid out pointer.
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetHandleInformation(handle: *mut c_void, flags: *mut u32) -> i32;
    }

    fn inherit_flag(socket: &impl AsRawSocket) -> u32 {
        let mut flags = 0u32;
        // SAFETY: `socket` is open for the duration of the call, and `flags`
        // is a valid, writable `u32` on this stack frame.
        let ok = unsafe {
            GetHandleInformation(socket.as_raw_socket() as usize as *mut c_void, &mut flags)
        };
        assert_ne!(
            ok,
            0,
            "GetHandleInformation failed: {}",
            io::Error::last_os_error()
        );
        flags & HANDLE_FLAG_INHERIT
    }

    /// A std listener: initializes Winsock and gives `connect` a target.
    fn listener() -> TcpListener {
        TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap()
    }

    #[test]
    fn connect_socket_is_not_inheritable() {
        let listener = listener();
        let (stream, _) = connect_nonblocking(listener.local_addr().unwrap()).unwrap();
        assert_eq!(inherit_flag(&stream), 0);
    }

    #[test]
    fn fallback_clears_inheritance_of_a_plain_socket() {
        let _winsock = listener();
        // Without `WSA_FLAG_NO_HANDLE_INHERIT` a socket is inheritable, which
        // is what this check would catch in `connect_nonblocking`.
        let socket = wsa_socket(AF_INET, WSA_FLAG_OVERLAPPED).unwrap();
        assert_eq!(inherit_flag(&socket), HANDLE_FLAG_INHERIT);
        clear_inherit(&socket).unwrap();
        assert_eq!(inherit_flag(&socket), 0);
    }
}
