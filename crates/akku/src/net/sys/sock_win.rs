//! Windows socket FFI (Phase 2 G3 step 1: `WSAPoll` + `AF_UNIX` probe).
//!
//! At G3 this file holds only the minimal `AF_UNIX` + `WSAPoll` FFI needed by
//! `tests::w0_probe_wsapoll_af_unix`. Full `SocketOpts` and Windows `AF_UNIX`
//! implementations land at G4.

#![allow(unsafe_code)]

use std::ffi::c_void;
use std::io;
use std::net::TcpStream;
use std::os::windows::io::{AsRawSocket, FromRawSocket, OwnedSocket, RawSocket};
use std::path::Path;

use super::wsapoll::{
    WsaPollFd, HANDLE_FLAG_INHERIT, INVALID_SOCKET, SOCKET_ERROR, SOCK_STREAM, WSAEINPROGRESS,
    WSAEINTR, WSAEINVAL, WSAEPROTOTYPE, WSAEWOULDBLOCK, WSA_FLAG_NO_HANDLE_INHERIT,
    WSA_FLAG_OVERLAPPED,
};

/// `AF_UNIX` address family (`winsock2.h` / `afunix.h`).
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) const AF_UNIX: i32 = 1;

/// Windows `SOCKADDR_UN` (`afunix.h`, `UNIX_PATH_MAX = 108`).
#[repr(C)]
#[derive(Clone, Copy)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct SockAddrUn {
    pub(crate) sun_family: u16,
    pub(crate) sun_path: [u8; 108],
}

const _: () = {
    assert!(core::mem::size_of::<SockAddrUn>() == 110);
    assert!(core::mem::align_of::<SockAddrUn>() == 2);
    assert!(core::mem::offset_of!(SockAddrUn, sun_family) == 0);
    assert!(core::mem::offset_of!(SockAddrUn, sun_path) == 2);
};

// SAFETY: signatures match `winsock2.h` as declared by std's `windows_sys.rs`
// and `windows-sys` (`SOCKET` = `usize`, `int` = `i32`, `DWORD`/`GROUP` =
// `u32`; the protocol-info pointer is always null). Callers uphold pointer and
// socket preconditions at every call site.
#[link(name = "ws2_32")]
unsafe extern "system" {
    fn WSASocketW(
        af: i32,
        ty: i32,
        protocol: i32,
        protocol_info: *const c_void,
        group: u32,
        flags: u32,
    ) -> usize;
    fn bind(s: usize, name: *const c_void, namelen: i32) -> i32;
    fn listen(s: usize, backlog: i32) -> i32;
    fn accept(s: usize, addr: *mut c_void, addrlen: *mut i32) -> usize;
    fn connect(s: usize, name: *const c_void, namelen: i32) -> i32;
    fn WSAPoll(fdarray: *mut WsaPollFd, fds: u32, timeout: i32) -> i32;
    fn WSAGetLastError() -> i32;
}

// SAFETY: matches `BOOL SetHandleInformation(HANDLE, DWORD, DWORD)` from
// `kernel32` (`BOOL` is `i32`, `HANDLE` a pointer, `DWORD` `u32`). Callers pass
// an open socket handle.
#[link(name = "kernel32")]
unsafe extern "system" {
    fn SetHandleInformation(handle: *mut c_void, mask: u32, flags: u32) -> i32;
}

#[cfg_attr(not(test), allow(dead_code))]
fn last_wsa_error() -> i32 {
    // SAFETY: `WSAGetLastError` has no preconditions; it reads the calling
    // thread's last Winsock error code.
    unsafe { WSAGetLastError() }
}

#[cfg_attr(not(test), allow(dead_code))]
fn sockaddr_un_from_path(path: &Path) -> io::Result<(SockAddrUn, i32)> {
    let s = path
        .to_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "non-UTF-8 Unix socket path"))?;
    let bytes = s.as_bytes();
    if bytes.is_empty() || bytes.len() >= 108 || bytes.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid Unix socket path",
        ));
    }
    let mut addr = SockAddrUn {
        sun_family: AF_UNIX as u16,
        sun_path: [0u8; 108],
    };
    addr.sun_path[..bytes.len()].copy_from_slice(bytes);
    let len = (2 + bytes.len() + 1) as i32;
    Ok((addr, len))
}

#[cfg_attr(not(test), allow(dead_code))]
fn wsa_unix_socket(flags: u32) -> Result<OwnedSocket, i32> {
    // SAFETY: integer arguments plus a null protocol-info pointer (`WSASocketW`
    // uses the first matching provider for `AF_UNIX` / `SOCK_STREAM`); Winsock
    // has already been initialized by the caller.
    let raw = unsafe { WSASocketW(AF_UNIX, SOCK_STREAM, 0, std::ptr::null(), 0, flags) };
    if raw == INVALID_SOCKET {
        return Err(last_wsa_error());
    }
    // SAFETY: `raw` is a fresh, valid socket (`!= INVALID_SOCKET`) that nothing
    // else owns; `OwnedSocket` takes sole ownership and closes it on drop.
    Ok(unsafe { OwnedSocket::from_raw_socket(raw as RawSocket) })
}

#[cfg_attr(not(test), allow(dead_code))]
fn new_unix_stream_socket() -> io::Result<OwnedSocket> {
    match wsa_unix_socket(WSA_FLAG_OVERLAPPED | WSA_FLAG_NO_HANDLE_INHERIT) {
        Ok(socket) => Ok(socket),
        Err(code) if code == WSAEINVAL || code == WSAEPROTOTYPE => {
            let socket =
                wsa_unix_socket(WSA_FLAG_OVERLAPPED).map_err(io::Error::from_raw_os_error)?;
            // SAFETY: `socket` is open and borrowed for the duration of the
            // call; a Winsock `SOCKET` is a kernel handle accepted by
            // `SetHandleInformation`.
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
            Ok(socket)
        }
        Err(code) => Err(io::Error::from_raw_os_error(code)),
    }
}

#[cfg_attr(not(test), allow(dead_code))]
fn bind_and_listen_unix(stream: &TcpStream, addr: &SockAddrUn, namelen: i32) -> io::Result<()> {
    let sock = usize::try_from(stream.as_raw_socket())
        .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    // SAFETY: `sock` is an open socket owned by `stream`; `addr` is an
    // initialized `SockAddrUn` and `namelen <= size_of::<SockAddrUn>()`.
    let rc = unsafe { bind(sock, (addr as *const SockAddrUn).cast::<c_void>(), namelen) };
    if rc == SOCKET_ERROR {
        return Err(io::Error::from_raw_os_error(last_wsa_error()));
    }
    // SAFETY: `sock` is an open, bound socket owned by `stream`.
    let rc = unsafe { listen(sock, 128) };
    if rc == SOCKET_ERROR {
        return Err(io::Error::from_raw_os_error(last_wsa_error()));
    }
    Ok(())
}

#[cfg_attr(not(test), allow(dead_code))]
fn accept_unix(listener: &TcpStream) -> io::Result<TcpStream> {
    let sock = usize::try_from(listener.as_raw_socket())
        .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    let mut peer = SockAddrUn {
        sun_family: 0,
        sun_path: [0u8; 108],
    };
    let mut peer_len = core::mem::size_of::<SockAddrUn>() as i32;
    // SAFETY: `sock` is an open listening socket owned by `listener`; `peer`
    // is a valid, writable `SockAddrUn` and `peer_len` holds its byte size.
    let raw = unsafe {
        accept(
            sock,
            (&mut peer as *mut SockAddrUn).cast::<c_void>(),
            &mut peer_len,
        )
    };
    if raw == INVALID_SOCKET {
        return Err(io::Error::from_raw_os_error(last_wsa_error()));
    }
    // SAFETY: `raw` is a newly accepted socket (`!= INVALID_SOCKET`) that
    // nothing else owns; `OwnedSocket` takes sole ownership.
    let owned = unsafe { OwnedSocket::from_raw_socket(raw as RawSocket) };
    Ok(TcpStream::from(owned))
}

#[cfg_attr(not(test), allow(dead_code))]
fn connect_unix_nonblocking(
    stream: &TcpStream,
    addr: &SockAddrUn,
    namelen: i32,
) -> io::Result<bool> {
    let sock = usize::try_from(stream.as_raw_socket())
        .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    // SAFETY: `sock` is an open socket owned by `stream`; `addr` is an
    // initialized `SockAddrUn` and `namelen <= size_of::<SockAddrUn>()`.
    let rc = unsafe { connect(sock, (addr as *const SockAddrUn).cast::<c_void>(), namelen) };
    if rc == 0 {
        return Ok(true);
    }
    let code = last_wsa_error();
    match code {
        WSAEWOULDBLOCK | WSAEINPROGRESS | WSAEINTR => Ok(false),
        _ => Err(io::Error::from_raw_os_error(code)),
    }
}

#[cfg_attr(not(test), allow(dead_code))]
fn wsa_poll_fds(fds: &mut [WsaPollFd], timeout_ms: i32) -> io::Result<usize> {
    let count = u32::try_from(fds.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "too many poll descriptors"))?;
    if count == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "WSAPoll requires at least one descriptor",
        ));
    }
    // SAFETY: `fds` is an exclusively borrowed, initialized slice of `count >= 1`
    // `WsaPollFd` structs; `WSAPoll` writes only their `revents` fields.
    let rc = unsafe { WSAPoll(fds.as_mut_ptr(), count, timeout_ms) };
    if rc == SOCKET_ERROR {
        return Err(io::Error::from_raw_os_error(last_wsa_error()));
    }
    Ok(rc as usize)
}

#[cfg(test)]
mod tests {
    use std::io::{Read as _, Write as _};
    use std::net::{Ipv4Addr, TcpListener};
    use std::path::PathBuf;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use super::*;
    use crate::net::sys::wsapoll::{POLLERR, POLLHUP, POLLNVAL, POLLRDNORM, POLLWRNORM};

    struct RemoveOnDrop(PathBuf);

    impl Drop for RemoveOnDrop {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn temp_sock_path(tag: &str) -> RemoveOnDrop {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("akku-{tag}-{}-{}.sock", std::process::id(), nanos));
        let _ = std::fs::remove_file(&path);
        RemoveOnDrop(path)
    }

    /// G3 step 1 stop-condition probe (DESIGN.md §P2.8 / decision Q2):
    /// proves that `WSAPoll` reports readiness on Windows `AF_UNIX` stream
    /// sockets:
    /// 1. `POLLRDNORM` on the listener when a client connects;
    /// 2. `POLLWRNORM` on the client after a non-blocking `connect`;
    /// 3. `POLLRDNORM` on a stream after the peer writes data;
    /// 4. `POLLHUP` / `POLLRDNORM` + `Ok(0)` EOF after the peer closes.
    #[test]
    fn w0_probe_wsapoll_af_unix() {
        // Initialize Winsock safely via std before any raw `ws2_32` calls.
        let _winsock = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("init winsock");

        let guard = temp_sock_path("w0-probe");
        let (addr, namelen) = sockaddr_un_from_path(&guard.0).expect("build SOCKADDR_UN");

        let listener = TcpStream::from(new_unix_stream_socket().expect("create AF_UNIX listener"));
        bind_and_listen_unix(&listener, &addr, namelen).expect("bind + listen AF_UNIX");
        listener
            .set_nonblocking(true)
            .expect("listener set_nonblocking");

        let listener_sock = listener.as_raw_socket() as usize;

        // Idle listener must not report readable before any client connects.
        let mut idle = [WsaPollFd {
            fd: listener_sock,
            events: POLLRDNORM,
            revents: 0,
        }];
        assert_eq!(
            wsa_poll_fds(&mut idle, 0).expect("idle listener WSAPoll"),
            0,
            "idle AF_UNIX listener reported unexpected revents: {:#x}",
            idle[0].revents
        );

        let mut client = TcpStream::from(new_unix_stream_socket().expect("create AF_UNIX client"));
        client
            .set_nonblocking(true)
            .expect("client set_nonblocking");
        let _connected_now = connect_unix_nonblocking(&client, &addr, namelen)
            .expect("non-blocking AF_UNIX connect");
        let client_sock = client.as_raw_socket() as usize;

        // Probe 1 & 2: wait for POLLRDNORM on the listener and POLLWRNORM on
        // the client (polling both handles either connect-completion order).
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut listener_ready = false;
        let mut client_writable = false;
        let mut server: Option<TcpStream> = None;

        while !(listener_ready && client_writable) {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for AF_UNIX WSAPoll readiness \
                 (listener_ready={listener_ready}, client_writable={client_writable})"
            );
            let mut pfds = Vec::with_capacity(2);
            if !listener_ready {
                pfds.push(WsaPollFd {
                    fd: listener_sock,
                    events: POLLRDNORM,
                    revents: 0,
                });
            }
            if !client_writable {
                pfds.push(WsaPollFd {
                    fd: client_sock,
                    events: POLLWRNORM,
                    revents: 0,
                });
            }
            let n = wsa_poll_fds(&mut pfds, 200).expect("WSAPoll listener/client");
            if n == 0 {
                continue;
            }
            for pfd in &pfds {
                if pfd.fd == listener_sock && pfd.revents != 0 {
                    assert_eq!(
                        pfd.revents & (POLLERR | POLLNVAL),
                        0,
                        "listener WSAPoll error revents: {:#x}",
                        pfd.revents
                    );
                    assert_ne!(
                        pfd.revents & POLLRDNORM,
                        0,
                        "listener expected POLLRDNORM, got {:#x}",
                        pfd.revents
                    );
                    listener_ready = true;
                    let accepted = accept_unix(&listener).expect("accept AF_UNIX");
                    accepted
                        .set_nonblocking(true)
                        .expect("accepted set_nonblocking");
                    server = Some(accepted);
                } else if pfd.fd == client_sock && pfd.revents != 0 {
                    assert_eq!(
                        pfd.revents & (POLLERR | POLLHUP | POLLNVAL),
                        0,
                        "client WSAPoll error/hangup revents: {:#x}",
                        pfd.revents
                    );
                    assert_ne!(
                        pfd.revents & POLLWRNORM,
                        0,
                        "client expected POLLWRNORM, got {:#x}",
                        pfd.revents
                    );
                    assert!(
                        client.take_error().expect("client take_error").is_none(),
                        "client connect reported SO_ERROR"
                    );
                    client_writable = true;
                }
            }
        }

        let mut server = server.expect("server accepted");
        let server_sock = server.as_raw_socket() as usize;

        // Idle server stream must not report readable yet.
        let mut server_idle = [WsaPollFd {
            fd: server_sock,
            events: POLLRDNORM,
            revents: 0,
        }];
        assert_eq!(
            wsa_poll_fds(&mut server_idle, 0).expect("idle server WSAPoll"),
            0,
            "idle AF_UNIX stream reported unexpected revents: {:#x}",
            server_idle[0].revents
        );

        // Probe 3: stream readable after peer writes.
        client
            .write_all(b"probe-afunix-ping")
            .expect("client write_all");
        let mut server_rd = [WsaPollFd {
            fd: server_sock,
            events: POLLRDNORM,
            revents: 0,
        }];
        let n = wsa_poll_fds(&mut server_rd, 5_000).expect("WSAPoll server readable");
        assert_eq!(n, 1, "expected server readable event");
        assert_eq!(
            server_rd[0].revents & (POLLERR | POLLNVAL),
            0,
            "server read error revents: {:#x}",
            server_rd[0].revents
        );
        assert_ne!(
            server_rd[0].revents & POLLRDNORM,
            0,
            "expected POLLRDNORM on server after client write, got {:#x}",
            server_rd[0].revents
        );
        let mut buf = [0u8; 32];
        let got = server.read(&mut buf).expect("server read");
        assert_eq!(&buf[..got], b"probe-afunix-ping");

        // Reverse direction check as well.
        server
            .write_all(b"probe-afunix-pong")
            .expect("server write_all");
        let mut client_rd = [WsaPollFd {
            fd: client_sock,
            events: POLLRDNORM,
            revents: 0,
        }];
        let n = wsa_poll_fds(&mut client_rd, 5_000).expect("WSAPoll client readable");
        assert_eq!(n, 1, "expected client readable event");
        assert_ne!(
            client_rd[0].revents & POLLRDNORM,
            0,
            "expected POLLRDNORM on client after server write, got {:#x}",
            client_rd[0].revents
        );
        let got = client.read(&mut buf).expect("client read");
        assert_eq!(&buf[..got], b"probe-afunix-pong");

        // Probe 4: hangup / readable-EOF after peer closes.
        drop(client);
        let mut server_eof = [WsaPollFd {
            fd: server_sock,
            events: POLLRDNORM,
            revents: 0,
        }];
        let n = wsa_poll_fds(&mut server_eof, 5_000).expect("WSAPoll server EOF");
        assert_eq!(n, 1, "expected server hangup/EOF event after peer close");
        assert_eq!(
            server_eof[0].revents & POLLNVAL,
            0,
            "server EOF reported POLLNVAL: {:#x}",
            server_eof[0].revents
        );
        assert_ne!(
            server_eof[0].revents & (POLLHUP | POLLRDNORM),
            0,
            "expected POLLHUP or POLLRDNORM after peer close, got {:#x}",
            server_eof[0].revents
        );
        let eof = server.read(&mut buf).expect("server read EOF");
        assert_eq!(eof, 0, "expected clean EOF after peer close");
    }

    // SAFETY: matches `BOOL GetHandleInformation(HANDLE, LPDWORD)` from
    // `kernel32` (`BOOL` is `i32`, `LPDWORD` a `*mut u32`); callers pass an
    // open socket handle and a valid writable `u32`.
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetHandleInformation(handle: *mut c_void, flags: *mut u32) -> i32;
    }

    fn inherit_flag(socket: &impl AsRawSocket) -> u32 {
        let mut flags = 0u32;
        // SAFETY: `socket` is open for the duration of the call, and `flags`
        // is a valid writable `u32` on this stack frame.
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

    /// Test 21 (Windows side): every Phase 2 socket handle must be
    /// non-inheritable (`HANDLE_FLAG_INHERIT == 0`). Fails against G3 stubs.
    #[test]
    fn p2_21_windows_phase2_sockets_are_not_inheritable() {
        use crate::net::{
            AsyncTcpListener, AsyncTcpStream, AsyncUdpSocket, AsyncUnixListener, AsyncUnixStream,
            SocketOpts,
        };
        use crate::task::Runtime;

        let rt = Runtime::new().expect("create runtime");
        rt.block_on(async {
            let tcp_listener = AsyncTcpListener::bind_with("127.0.0.1:0", &SocketOpts::new())
                .await
                .expect("tcp bind_with");
            assert_eq!(inherit_flag(&tcp_listener), 0);

            let tcp_addr = tcp_listener.local_addr().expect("tcp local_addr");
            let tcp_client = AsyncTcpStream::connect_with(tcp_addr, &SocketOpts::new())
                .await
                .expect("tcp connect_with");
            assert_eq!(inherit_flag(&tcp_client), 0);

            let udp = AsyncUdpSocket::bind_with("127.0.0.1:0", &SocketOpts::new())
                .await
                .expect("udp bind_with");
            assert_eq!(inherit_flag(&udp), 0);

            let guard = temp_sock_path("p2-21-win");
            let unix_listener = AsyncUnixListener::bind(&guard.0).expect("unix bind");
            assert_eq!(inherit_flag(&unix_listener), 0);
            let path = guard.0.clone();
            let connect = crate::task::spawn(async move { AsyncUnixStream::connect(&path).await });
            let (unix_accepted, _) = unix_listener.accept().await.expect("unix accept");
            let unix_client = connect.await.unwrap().expect("unix connect");
            assert_eq!(inherit_flag(&unix_accepted), 0);
            assert_eq!(inherit_flag(&unix_client), 0);
        });
    }
}
