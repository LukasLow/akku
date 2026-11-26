//! `std::net` superset with native asynchronous TCP, UDP, and Unix sockets on `akku::task`.
//!
//! Every blocking type and address primitive from `std::net` is re-exported
//! under its original name (`TcpListener`, `TcpStream`, `UdpSocket`,
//! `SocketAddr`, `IpAddr`, `Shutdown`, and friends) on all targets, including
//! `wasm32`.
//!
//! On native (non-`wasm32`) targets, asynchronous networking is provided by
//! [`AsyncTcpListener`], [`AsyncTcpStream`] (and its split halves),
//! [`AsyncUdpSocket`], `AsyncUnixListener`, `AsyncUnixStream` (Unix and
//! Windows 10 1803+), `AsyncUnixDatagram` (Unix), and [`SocketOpts`],
//! integrated directly with [`crate::io::AsyncRead`] and
//! [`crate::io::AsyncWrite`].
//!
//! # Overview
//!
//! | Task | API |
//! |---|---|
//! | Blocking sockets & addresses (`std::net`) | [`TcpListener`], [`TcpStream`], [`UdpSocket`], [`SocketAddr`], [`IpAddr`], [`Shutdown`] |
//! | Async TCP server (native) | [`AsyncTcpListener`] |
//! | Async TCP connection (native) | [`AsyncTcpStream`] |
//! | Borrowed full-duplex split (native) | [`AsyncTcpReadHalf`], [`AsyncTcpWriteHalf`] |
//! | Owned full-duplex split (native) | [`AsyncTcpOwnedReadHalf`], [`AsyncTcpOwnedWriteHalf`] |
//! | Async UDP | [`AsyncUdpSocket`] |
//! | Async Unix stream server/connection (Unix, Windows 10 1803+) | `AsyncUnixListener`, `AsyncUnixStream`, `UnixSocketAddr` |
//! | Async Unix datagram (Unix) | `AsyncUnixDatagram` |
//! | Socket options | [`SocketOpts`], [`TcpKeepalive`] |
//!
//! # Platform support
//!
//! - **Native Tier-1 targets (Linux, Android, macOS, iOS, Windows):** full
//!   native readiness reactor support (`epoll`, `kqueue`, `WSAPoll`).
//! - **Windows version:** Windows 10 version 2004 (build 19041) or Windows
//!   Server 2022 and later. Older versions (including Windows Server 2019)
//!   have a `WSAPoll` that never reports a failed non-blocking connect, so
//!   [`AsyncTcpStream::connect`] to a refused port would wait forever there.
//!   Like `std::net` sockets, the sockets [`AsyncTcpStream::connect`] creates
//!   are not inherited by child processes (`SOCK_CLOEXEC` on Linux and
//!   Android, `FD_CLOEXEC` on Darwin, `WSA_FLAG_NO_HANDLE_INHERIT` on
//!   Windows).
//! - **`wasm32`:** `std::net` address and blocking re-exports compile cleanly;
//!   native async socket types (`AsyncTcp*`, `AsyncUdpSocket`, `AsyncUnix*`,
//!   `SocketOpts`) are `#[cfg(not(target_arch = "wasm32"))]`
//!   because raw OS sockets are unavailable in `wasm32`.
//!
//! # Errors and runtime requirement
//!
//! - All fallible methods return `std::io::Result<T>`.
//! - Constructing or registering an async socket (`bind`, `connect`,
//!   `from_std`) outside an active [`crate::task::Runtime`] returns
//!   `Err(io::Error)` rather than panicking.
//! - Dropping an [`AsyncTcpOwnedWriteHalf`] calls `shutdown(Shutdown::Write)`
//!   (ignoring errors) so the remote peer observes EOF promptly.
//! - Each reactor enforces an internal cap of `65_536` concurrent registered
//!   sockets and caps single-syscall read/write slices at `1 << 30` bytes.
//!
//! # Fairness
//!
//! Every ready socket operation (including one that fails at once, such as
//! `accept` under `EMFILE`) spends the cooperative budget of the current
//! [`crate::task`] poll. After `128` of them the next operation returns
//! `Poll::Pending` and the task is re-polled after the other ready tasks and
//! the next reactor turn, so a fast peer or a persistent error cannot starve
//! other connections. A server `accept` loop that hits `EMFILE` still spins
//! (fairly) until a descriptor frees up; back off with
//! [`crate::task::yield_now`] or drop idle connections in that case.
//!
//! # Examples
//!
//! ```
//! use akku::io::{AsyncReadExt, AsyncWriteExt};
//! use akku::net::{AsyncTcpListener, AsyncTcpStream};
//! use akku::task;
//!
//! task::block_on(async {
//!     let listener = AsyncTcpListener::bind("127.0.0.1:0").await?;
//!     let addr = listener.local_addr()?;
//!
//!     let server = task::spawn(async move {
//!         let (mut conn, _peer) = listener.accept().await?;
//!         let (mut rx, mut tx) = conn.split();
//!         rx.copy_to(&mut tx).await
//!     });
//!
//!     let mut client = AsyncTcpStream::connect(addr).await?;
//!     client.write_all(b"ping").await?;
//!     client.shutdown().await?;
//!
//!     let mut buf = [0u8; 4];
//!     client.read_exact(&mut buf).await?;
//!     assert_eq!(&buf, b"ping");
//!     assert_eq!(server.await.unwrap()?, 4);
//!     Ok::<(), std::io::Error>(())
//! })?;
//! # Ok::<(), std::io::Error>(())
//! ```

pub use std::net::{
    AddrParseError, Incoming, IpAddr, Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, SocketAddrV4,
    SocketAddrV6, TcpListener, TcpStream, ToSocketAddrs, UdpSocket,
};

#[cfg(not(target_arch = "wasm32"))]
mod opts;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod reactor;
#[cfg(not(target_arch = "wasm32"))]
mod sys;
#[cfg(not(target_arch = "wasm32"))]
mod udp;
#[cfg(all(any(unix, windows), not(target_arch = "wasm32")))]
mod unix;

#[cfg(not(target_arch = "wasm32"))]
pub use opts::{SocketOpts, TcpKeepalive};
#[cfg(not(target_arch = "wasm32"))]
pub use udp::AsyncUdpSocket;
#[cfg(all(unix, not(target_arch = "wasm32")))]
pub use unix::AsyncUnixDatagram;
#[cfg(all(any(unix, windows), not(target_arch = "wasm32")))]
pub use unix::{
    AsyncUnixListener, AsyncUnixOwnedReadHalf, AsyncUnixOwnedWriteHalf, AsyncUnixReadHalf,
    AsyncUnixStream, AsyncUnixWriteHalf, UnixSocketAddr,
};

#[cfg(not(target_arch = "wasm32"))]
use std::fmt;
#[cfg(not(target_arch = "wasm32"))]
use std::future::poll_fn;
#[cfg(not(target_arch = "wasm32"))]
use std::io::{self, Read, Write};
#[cfg(all(unix, not(target_arch = "wasm32")))]
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, RawFd};
#[cfg(all(windows, not(target_arch = "wasm32")))]
use std::os::windows::io::{AsRawSocket, AsSocket, BorrowedSocket, RawSocket};
#[cfg(not(target_arch = "wasm32"))]
use std::pin::Pin;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::Arc;
#[cfg(not(target_arch = "wasm32"))]
use std::task::{ready, Context, Poll};
#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;

#[cfg(not(target_arch = "wasm32"))]
use crate::io::{AsyncRead, AsyncWrite};
#[cfg(not(target_arch = "wasm32"))]
use reactor::{Direction, Reactor, Registration, Waiters};

/// Single-syscall read/write slice cap (`1 << 30`, as in Go's `internal/poll`).
#[cfg(not(target_arch = "wasm32"))]
const MAX_IO_LEN: usize = 1 << 30;

/// Spends one unit of the cooperative budget, first yielding if it is spent,
/// so a loop of constructor calls that fail at once (`EMFILE`,
/// `EADDRINUSE`, unresolvable input) cannot starve sibling tasks.
#[cfg(not(target_arch = "wasm32"))]
async fn coop_step() {
    poll_fn(|cx| {
        ready!(crate::task::poll_budget(cx));
        crate::task::consume_budget();
        Poll::Ready(())
    })
    .await
}

/// Borrows the OS handle of a std socket in the form `sys` expects.
#[cfg(all(unix, not(target_arch = "wasm32")))]
fn handle<T: AsFd>(socket: &T) -> sys::Handle<'_> {
    socket.as_fd()
}

/// Borrows the OS handle of a std socket in the form `sys` expects.
#[cfg(all(windows, not(target_arch = "wasm32")))]
fn handle<T: AsSocket>(socket: &T) -> sys::Handle<'_> {
    socket.as_socket()
}

/// An async TCP socket server listening for incoming connections on the current
/// [`crate::task::Runtime`].
///
/// Dropping an `AsyncTcpListener` deregisters it from the OS reactor before
/// closing the underlying socket.
///
/// The listener is `Send + Sync`, and [`accept`](Self::accept) takes `&self`,
/// so several tasks may accept concurrently from one `Arc<AsyncTcpListener>`:
/// every waiting task is woken when a connection arrives, and each
/// connection is handed to exactly one of them.
#[cfg(not(target_arch = "wasm32"))]
pub struct AsyncTcpListener {
    /// `None` only after [`AsyncTcpListener::into_std`] moved it out.
    inner: Option<std::net::TcpListener>,
    reg: Registration,
}

#[cfg(not(target_arch = "wasm32"))]
impl AsyncTcpListener {
    /// Creates a new `AsyncTcpListener` bound to `addr`.
    ///
    /// Returns `Err(io::Error)` if called outside an active
    /// [`crate::task::Runtime`], if the runtime is shutting down, or if binding
    /// fails.
    ///
    /// # Blocking
    ///
    /// A host name in `addr` is resolved with the blocking system resolver
    /// (`getaddrinfo`) on the runtime thread, which stalls every task until it
    /// returns. Pass a [`SocketAddr`] (or an IP literal) to avoid that.
    pub async fn bind<A: ToSocketAddrs>(addr: A) -> io::Result<Self> {
        coop_step().await;
        let reactor = crate::task::current_reactor()?;
        let listener = std::net::TcpListener::bind(addr)?;
        Self::register(listener, reactor)
    }

    /// Creates a new `AsyncTcpListener` bound to `addr`, applying `opts`
    /// before `bind` and `listen`.
    ///
    /// Tries each resolved address in order until one succeeds, returning the
    /// last error if all fail. `nodelay`, `keepalive`, and `zero_linger` from
    /// `opts` are also applied to every accepted [`AsyncTcpStream`]. Calling
    /// `bind_with(addr, &SocketOpts::new())` behaves identically to
    /// [`bind`](Self::bind).
    ///
    /// # Blocking
    ///
    /// A host name in `addr` is resolved with the blocking system resolver
    /// (`getaddrinfo`) on the runtime thread.
    #[allow(unused_variables)]
    pub async fn bind_with<A: ToSocketAddrs>(addr: A, opts: &SocketOpts) -> io::Result<Self> {
        todo!()
    }

    fn register(listener: std::net::TcpListener, reactor: Arc<Reactor>) -> io::Result<Self> {
        listener.set_nonblocking(true)?;
        let reg = Registration::new(reactor, true, Waiters::Shared)?;
        Ok(Self {
            inner: Some(listener),
            reg,
        })
    }

    fn io(&self) -> &std::net::TcpListener {
        self.inner
            .as_ref()
            .expect("AsyncTcpListener used after into_std")
    }

    /// Accepts a new incoming TCP connection, returning the non-blocking
    /// [`AsyncTcpStream`] and the remote peer address.
    ///
    /// This future is cancel-safe: dropping it before completion does not drop
    /// or lose an already-queued incoming connection. Any number of tasks may
    /// await `accept` on the same listener at once.
    pub async fn accept(&self) -> io::Result<(AsyncTcpStream, SocketAddr)> {
        poll_fn(|cx| self.poll_accept(cx)).await
    }

    /// Polls for an incoming TCP connection.
    ///
    /// On `Poll::Pending`, the waker of `cx` is kept in addition to wakers of
    /// other pending callers (deduplicated with [`std::task::Waker::will_wake`])
    /// and all of them are woken when the listener becomes ready. The set is
    /// bounded by the runtime's [`crate::task::Limits::max_tasks`] plus one;
    /// past that bound, the parked wakers are woken early (spuriously) rather
    /// than dropped.
    pub fn poll_accept(
        &self,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<(AsyncTcpStream, SocketAddr)>> {
        let accept = || self.io().accept();
        let (stream, peer) =
            ready!(self
                .reg
                .poll_io(cx, Direction::Read, handle(self.io()), accept))?;
        // Accepted sockets may already hold data: seed readiness optimistically.
        let stream = AsyncTcpStream::register(stream, Arc::clone(self.reg.reactor()), true);
        Poll::Ready(stream.map(|s| (s, peer)))
    }

    /// Wraps an existing [`std::net::TcpListener`], switching it to
    /// non-blocking mode and registering it with the current
    /// [`crate::task::Runtime`].
    ///
    /// Returns `Err(io::Error)` if called outside an active runtime.
    pub fn from_std(listener: std::net::TcpListener) -> io::Result<Self> {
        let reactor = crate::task::current_reactor()?;
        Self::register(listener, reactor)
    }

    /// Deregisters this listener from the reactor and returns the underlying
    /// [`std::net::TcpListener`] (still in non-blocking mode).
    pub fn into_std(mut self) -> io::Result<std::net::TcpListener> {
        let listener = self
            .inner
            .take()
            .expect("AsyncTcpListener used after into_std");
        self.reg.deregister(handle(&listener))?;
        Ok(listener)
    }

    /// Returns the local socket address of this listener.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.io().local_addr()
    }

    /// Gets the value of the `IP_TTL` option for this socket.
    pub fn ttl(&self) -> io::Result<u32> {
        self.io().ttl()
    }

    /// Sets the value of the `IP_TTL` option for this socket.
    pub fn set_ttl(&self, ttl: u32) -> io::Result<()> {
        self.io().set_ttl(ttl)
    }

    /// Gets the value of the `SO_ERROR` option on this socket.
    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        self.io().take_error()
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl fmt::Debug for AsyncTcpListener {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut s = f.debug_struct("AsyncTcpListener");
        if let Some(listener) = &self.inner {
            s.field("local_addr", &listener.local_addr().ok());
        }
        s.finish_non_exhaustive()
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for AsyncTcpListener {
    fn drop(&mut self) {
        // Deregister while the socket is still open; it closes afterwards
        // when `inner` drops.
        if let Some(listener) = &self.inner {
            let _ = self.reg.deregister(handle(listener));
        }
    }
}

#[cfg(all(unix, not(target_arch = "wasm32")))]
impl AsFd for AsyncTcpListener {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.io().as_fd()
    }
}

#[cfg(all(unix, not(target_arch = "wasm32")))]
impl AsRawFd for AsyncTcpListener {
    fn as_raw_fd(&self) -> RawFd {
        self.io().as_raw_fd()
    }
}

#[cfg(all(windows, not(target_arch = "wasm32")))]
impl AsSocket for AsyncTcpListener {
    fn as_socket(&self) -> BorrowedSocket<'_> {
        self.io().as_socket()
    }
}

#[cfg(all(windows, not(target_arch = "wasm32")))]
impl AsRawSocket for AsyncTcpListener {
    fn as_raw_socket(&self) -> RawSocket {
        self.io().as_raw_socket()
    }
}

/// An async TCP stream between a local and a remote socket on the current
/// [`crate::task::Runtime`].
///
/// Implements [`AsyncRead`] and [`AsyncWrite`], so ergonomic async methods
/// (`read`, `read_exact`, `copy_to`, `write`, `write_all`, `flush`, `shutdown`)
/// are available through [`crate::io::AsyncReadExt`] and
/// [`crate::io::AsyncWriteExt`].
///
/// Dropping an `AsyncTcpStream` deregisters it from the OS reactor before
/// closing the underlying socket.
#[cfg(not(target_arch = "wasm32"))]
pub struct AsyncTcpStream {
    /// `None` only after [`AsyncTcpStream::into_std`] moved it out.
    inner: Option<std::net::TcpStream>,
    reg: Registration,
}

#[cfg(not(target_arch = "wasm32"))]
impl AsyncTcpStream {
    /// Opens a non-blocking async TCP connection to `addr`.
    ///
    /// Tries each resolved address in order until one succeeds, returning the
    /// last error if all fail. Returns `Err(io::Error)` if called outside an
    /// active [`crate::task::Runtime`]. Cancel-safe: dropping the future closes
    /// any in-progress socket cleanly.
    ///
    /// # Blocking
    ///
    /// A host name in `addr` is resolved with the blocking system resolver
    /// (`getaddrinfo`) on the runtime thread, which stalls every task for up
    /// to the resolver's timeout. Pass a [`SocketAddr`] (or an IP literal) to
    /// avoid that. The connect itself has no timeout of its own beyond the
    /// operating system's TCP handshake timeout.
    pub async fn connect<A: ToSocketAddrs>(addr: A) -> io::Result<Self> {
        coop_step().await;
        let reactor = crate::task::current_reactor()?;
        let addrs: Vec<SocketAddr> = addr.to_socket_addrs()?.collect();
        let mut last_err = None;
        for addr in addrs {
            match Self::connect_one(addr, &reactor).await {
                Ok(stream) => return Ok(stream),
                Err(e) => last_err = Some(e),
            }
        }
        Err(last_err.unwrap_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "could not resolve to any addresses",
            )
        }))
    }

    /// Opens a non-blocking async TCP connection to `addr`, applying `opts`
    /// before `connect`.
    ///
    /// Tries each resolved address in order until one succeeds, returning the
    /// last error if all fail. Validates `opts` before creating any socket and
    /// returns `ErrorKind::InvalidInput` for out-of-range values or
    /// inapplicable options (`broadcast`, `backlog`).
    ///
    /// # Blocking
    ///
    /// A host name in `addr` is resolved with the blocking system resolver
    /// (`getaddrinfo`) on the runtime thread.
    #[allow(unused_variables)]
    pub async fn connect_with<A: ToSocketAddrs>(addr: A, opts: &SocketOpts) -> io::Result<Self> {
        todo!()
    }

    async fn connect_one(addr: SocketAddr, reactor: &Arc<Reactor>) -> io::Result<Self> {
        let (stream, connected) = sys::connect_nonblocking(addr)?;
        // An in-progress connect is not ready until the OS reports it.
        let stream = Self::register(stream, Arc::clone(reactor), connected)?;
        if !connected {
            poll_fn(|cx| stream.poll_connected(cx)).await?;
            // Both directions are usable now; reads still wait for data.
        }
        Ok(stream)
    }

    /// Waits until a non-blocking connect has finished, reporting its error.
    fn poll_connected(&self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        loop {
            let tick = ready!(self.reg.poll_ready(cx, Direction::Write, handle(self.io())))?;
            if let Some(e) = self.io().take_error()? {
                return Poll::Ready(Err(e));
            }
            match self.io().peer_addr() {
                Ok(_) => return Poll::Ready(Ok(())),
                // Spurious writability before the handshake finished.
                Err(e) if e.kind() == io::ErrorKind::NotConnected => {
                    self.reg.clear_ready(Direction::Write, tick);
                }
                Err(e) => return Poll::Ready(Err(e)),
            }
        }
    }

    fn register(
        stream: std::net::TcpStream,
        reactor: Arc<Reactor>,
        initial_ready: bool,
    ) -> io::Result<Self> {
        stream.set_nonblocking(true)?;
        let reg = Registration::new(reactor, initial_ready, Waiters::Single)?;
        Ok(Self {
            inner: Some(stream),
            reg,
        })
    }

    fn io(&self) -> &std::net::TcpStream {
        self.inner
            .as_ref()
            .expect("AsyncTcpStream used after into_std")
    }

    /// Wraps an existing [`std::net::TcpStream`], switching it to non-blocking
    /// mode and registering it with the current [`crate::task::Runtime`].
    ///
    /// Returns `Err(io::Error)` if called outside an active runtime.
    pub fn from_std(stream: std::net::TcpStream) -> io::Result<Self> {
        let reactor = crate::task::current_reactor()?;
        Self::register(stream, reactor, true)
    }

    /// Deregisters this stream from the reactor and returns the underlying
    /// [`std::net::TcpStream`] (still in non-blocking mode).
    pub fn into_std(mut self) -> io::Result<std::net::TcpStream> {
        let stream = self
            .inner
            .take()
            .expect("AsyncTcpStream used after into_std");
        self.reg.deregister(handle(&stream))?;
        Ok(stream)
    }

    fn poll_read_ref(&self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let len = buf.len().min(MAX_IO_LEN);
        let buf = &mut buf[..len];
        self.reg
            .poll_io(cx, Direction::Read, handle(self.io()), || {
                self.io().read(buf)
            })
    }

    fn poll_write_ref(&self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let buf = &buf[..buf.len().min(MAX_IO_LEN)];
        self.reg
            .poll_io(cx, Direction::Write, handle(self.io()), || {
                self.io().write(buf)
            })
    }

    fn try_read_ref(&self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let len = buf.len().min(MAX_IO_LEN);
        let buf = &mut buf[..len];
        self.reg.try_io(Direction::Read, || self.io().read(buf))
    }

    fn try_write_ref(&self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let buf = &buf[..buf.len().min(MAX_IO_LEN)];
        self.reg.try_io(Direction::Write, || self.io().write(buf))
    }

    fn shutdown_write(&self) -> Poll<io::Result<()>> {
        Poll::Ready(self.io().shutdown(Shutdown::Write))
    }

    /// Polls for reading bytes into `buf`.
    ///
    /// Returns `Poll::Ready(Ok(0))` immediately when `buf` is empty or on EOF.
    pub fn poll_read(&mut self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        self.poll_read_ref(cx, buf)
    }

    /// Polls for writing bytes from `buf`.
    ///
    /// Returns `Poll::Ready(Ok(0))` immediately when `buf` is empty.
    pub fn poll_write(&mut self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        self.poll_write_ref(cx, buf)
    }

    /// Polls for shutting down the write half (`Shutdown::Write`) of this stream.
    pub fn poll_shutdown(&mut self, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.shutdown_write()
    }

    /// Attempts a non-blocking read without awaiting readiness.
    ///
    /// Returns `Err(io::ErrorKind::WouldBlock)` if no data is immediately
    /// available, clearing/re-arming readiness for a subsequent `.await`.
    pub fn try_read(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.try_read_ref(buf)
    }

    /// Attempts a non-blocking write without awaiting readiness.
    ///
    /// Returns `Err(io::ErrorKind::WouldBlock)` if the socket send buffer is
    /// currently full, clearing/re-arming readiness for a subsequent `.await`.
    pub fn try_write(&self, buf: &[u8]) -> io::Result<usize> {
        self.try_write_ref(buf)
    }

    /// Shuts down the read, write, or both halves of this connection
    /// immediately.
    ///
    /// Named `shutdown_how` so it does not shadow 0-argument
    /// [`crate::io::AsyncWriteExt::shutdown`].
    pub fn shutdown_how(&self, how: Shutdown) -> io::Result<()> {
        self.io().shutdown(how)
    }

    /// Splits this stream into borrowed read and write halves for concurrent
    /// full-duplex I/O without heap allocation.
    pub fn split(&mut self) -> (AsyncTcpReadHalf<'_>, AsyncTcpWriteHalf<'_>) {
        let stream: &Self = self;
        (AsyncTcpReadHalf { stream }, AsyncTcpWriteHalf { stream })
    }

    /// Splits this stream into owned read and write halves that can be moved
    /// into separate tasks.
    pub fn into_split(self) -> (AsyncTcpOwnedReadHalf, AsyncTcpOwnedWriteHalf) {
        let stream = Arc::new(self);
        (
            AsyncTcpOwnedReadHalf {
                stream: Arc::clone(&stream),
            },
            AsyncTcpOwnedWriteHalf { stream },
        )
    }

    /// Returns the local socket address of this stream.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.io().local_addr()
    }

    /// Returns the remote peer socket address of this stream.
    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.io().peer_addr()
    }

    /// Gets the value of the `TCP_NODELAY` option on this socket.
    pub fn nodelay(&self) -> io::Result<bool> {
        self.io().nodelay()
    }

    /// Sets the value of the `TCP_NODELAY` option on this socket.
    pub fn set_nodelay(&self, nodelay: bool) -> io::Result<()> {
        self.io().set_nodelay(nodelay)
    }

    /// Gets the value of the `IP_TTL` option on this socket.
    pub fn ttl(&self) -> io::Result<u32> {
        self.io().ttl()
    }

    /// Sets the value of the `IP_TTL` option on this socket.
    pub fn set_ttl(&self, ttl: u32) -> io::Result<()> {
        self.io().set_ttl(ttl)
    }

    /// Gets the value of the `SO_ERROR` option on this socket.
    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        self.io().take_error()
    }

    /// Receives data from the stream without removing it from the receive
    /// queue.
    ///
    /// Takes `&mut self` to uphold the single-reader waker invariant.
    /// Cancel-safe.
    #[allow(unused_variables)]
    pub async fn peek(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        todo!()
    }

    /// Polls for peeking bytes into `buf` without consuming them.
    #[allow(unused_variables)]
    pub fn poll_peek(&mut self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        todo!()
    }

    /// Attempts a non-blocking peek without awaiting readiness.
    #[allow(unused_variables)]
    pub fn try_peek(&self, buf: &mut [u8]) -> io::Result<usize> {
        todo!()
    }

    /// Gets whether `SO_KEEPALIVE` is enabled on this socket.
    pub fn keepalive(&self) -> io::Result<bool> {
        todo!()
    }

    /// Enables or disables `SO_KEEPALIVE` and optional probe timers on this
    /// socket.
    #[allow(unused_variables)]
    pub fn set_keepalive(&self, keepalive: Option<TcpKeepalive>) -> io::Result<()> {
        todo!()
    }

    /// Gets the `SO_LINGER` state of this socket: `Some(Duration::ZERO)` after
    /// [`set_zero_linger(true)`](Self::set_zero_linger), or `None` when off.
    pub fn linger(&self) -> io::Result<Option<Duration>> {
        todo!()
    }

    /// Enables or disables `SO_LINGER` with a zero timeout (abortive `RST` on
    /// close).
    #[allow(unused_variables)]
    pub fn set_zero_linger(&self, on: bool) -> io::Result<()> {
        todo!()
    }

    /// Gets the value of the `SO_RCVBUF` option in bytes.
    pub fn recv_buffer_size(&self) -> io::Result<usize> {
        todo!()
    }

    /// Sets the value of the `SO_RCVBUF` option in bytes (`1..=i32::MAX`).
    #[allow(unused_variables)]
    pub fn set_recv_buffer_size(&self, bytes: usize) -> io::Result<()> {
        todo!()
    }

    /// Gets the value of the `SO_SNDBUF` option in bytes.
    pub fn send_buffer_size(&self) -> io::Result<usize> {
        todo!()
    }

    /// Sets the value of the `SO_SNDBUF` option in bytes (`1..=i32::MAX`).
    #[allow(unused_variables)]
    pub fn set_send_buffer_size(&self, bytes: usize) -> io::Result<()> {
        todo!()
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl AsyncRead for AsyncTcpStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        AsyncTcpStream::poll_read(&mut self, cx, buf)
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl AsyncWrite for AsyncTcpStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        AsyncTcpStream::poll_write(&mut self, cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        // TCP writes go straight to the kernel; there is nothing to flush.
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        AsyncTcpStream::poll_shutdown(&mut self, cx)
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl fmt::Debug for AsyncTcpStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut s = f.debug_struct("AsyncTcpStream");
        if let Some(stream) = &self.inner {
            s.field("local_addr", &stream.local_addr().ok())
                .field("peer_addr", &stream.peer_addr().ok());
        }
        s.finish_non_exhaustive()
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for AsyncTcpStream {
    fn drop(&mut self) {
        // Deregister while the socket is still open; it closes afterwards
        // when `inner` drops.
        if let Some(stream) = &self.inner {
            let _ = self.reg.deregister(handle(stream));
        }
    }
}

#[cfg(all(unix, not(target_arch = "wasm32")))]
impl AsFd for AsyncTcpStream {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.io().as_fd()
    }
}

#[cfg(all(unix, not(target_arch = "wasm32")))]
impl AsRawFd for AsyncTcpStream {
    fn as_raw_fd(&self) -> RawFd {
        self.io().as_raw_fd()
    }
}

#[cfg(all(windows, not(target_arch = "wasm32")))]
impl AsSocket for AsyncTcpStream {
    fn as_socket(&self) -> BorrowedSocket<'_> {
        self.io().as_socket()
    }
}

#[cfg(all(windows, not(target_arch = "wasm32")))]
impl AsRawSocket for AsyncTcpStream {
    fn as_raw_socket(&self) -> RawSocket {
        self.io().as_raw_socket()
    }
}

/// Borrowed read half of an [`AsyncTcpStream`] (zero allocation).
#[cfg(not(target_arch = "wasm32"))]
pub struct AsyncTcpReadHalf<'a> {
    stream: &'a AsyncTcpStream,
}

#[cfg(not(target_arch = "wasm32"))]
impl AsyncTcpReadHalf<'_> {
    /// Polls for reading bytes into `buf`.
    pub fn poll_read(&mut self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        self.stream.poll_read_ref(cx, buf)
    }

    /// Attempts a non-blocking read without awaiting readiness.
    pub fn try_read(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.stream.try_read_ref(buf)
    }

    /// Receives data from the stream without removing it from the receive
    /// queue. Cancel-safe.
    #[allow(unused_variables)]
    pub async fn peek(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        todo!()
    }

    /// Polls for peeking bytes into `buf` without consuming them.
    #[allow(unused_variables)]
    pub fn poll_peek(&mut self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        todo!()
    }

    /// Attempts a non-blocking peek without awaiting readiness.
    #[allow(unused_variables)]
    pub fn try_peek(&self, buf: &mut [u8]) -> io::Result<usize> {
        todo!()
    }

    /// Returns the local socket address of the underlying stream.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.stream.local_addr()
    }

    /// Returns the remote peer socket address of the underlying stream.
    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.stream.peer_addr()
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl AsyncRead for AsyncTcpReadHalf<'_> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        AsyncTcpReadHalf::poll_read(&mut self, cx, buf)
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl fmt::Debug for AsyncTcpReadHalf<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AsyncTcpReadHalf")
            .field("stream", self.stream)
            .finish()
    }
}

/// Borrowed write half of an [`AsyncTcpStream`] (zero allocation).
#[cfg(not(target_arch = "wasm32"))]
pub struct AsyncTcpWriteHalf<'a> {
    stream: &'a AsyncTcpStream,
}

#[cfg(not(target_arch = "wasm32"))]
impl AsyncTcpWriteHalf<'_> {
    /// Polls for writing bytes from `buf`.
    pub fn poll_write(&mut self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        self.stream.poll_write_ref(cx, buf)
    }

    /// Polls for shutting down the write half (`Shutdown::Write`).
    pub fn poll_shutdown(&mut self, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.stream.shutdown_write()
    }

    /// Attempts a non-blocking write without awaiting readiness.
    pub fn try_write(&self, buf: &[u8]) -> io::Result<usize> {
        self.stream.try_write_ref(buf)
    }

    /// Returns the local socket address of the underlying stream.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.stream.local_addr()
    }

    /// Returns the remote peer socket address of the underlying stream.
    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.stream.peer_addr()
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl AsyncWrite for AsyncTcpWriteHalf<'_> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        AsyncTcpWriteHalf::poll_write(&mut self, cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        AsyncTcpWriteHalf::poll_shutdown(&mut self, cx)
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl fmt::Debug for AsyncTcpWriteHalf<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AsyncTcpWriteHalf")
            .field("stream", self.stream)
            .finish()
    }
}

/// Owned read half of an [`AsyncTcpStream`], movable into a separate spawned task.
#[cfg(not(target_arch = "wasm32"))]
pub struct AsyncTcpOwnedReadHalf {
    stream: Arc<AsyncTcpStream>,
}

#[cfg(not(target_arch = "wasm32"))]
impl AsyncTcpOwnedReadHalf {
    /// Polls for reading bytes into `buf`.
    pub fn poll_read(&mut self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        self.stream.poll_read_ref(cx, buf)
    }

    /// Attempts a non-blocking read without awaiting readiness.
    pub fn try_read(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.stream.try_read_ref(buf)
    }

    /// Receives data from the stream without removing it from the receive
    /// queue. Cancel-safe.
    #[allow(unused_variables)]
    pub async fn peek(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        todo!()
    }

    /// Polls for peeking bytes into `buf` without consuming them.
    #[allow(unused_variables)]
    pub fn poll_peek(&mut self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        todo!()
    }

    /// Attempts a non-blocking peek without awaiting readiness.
    #[allow(unused_variables)]
    pub fn try_peek(&self, buf: &mut [u8]) -> io::Result<usize> {
        todo!()
    }

    /// Returns the local socket address of the underlying stream.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.stream.local_addr()
    }

    /// Returns the remote peer socket address of the underlying stream.
    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.stream.peer_addr()
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl AsyncRead for AsyncTcpOwnedReadHalf {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        AsyncTcpOwnedReadHalf::poll_read(&mut self, cx, buf)
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl fmt::Debug for AsyncTcpOwnedReadHalf {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AsyncTcpOwnedReadHalf")
            .field("stream", &*self.stream)
            .finish()
    }
}

/// Owned write half of an [`AsyncTcpStream`], movable into a separate spawned task.
///
/// Dropping an `AsyncTcpOwnedWriteHalf` calls `shutdown(Shutdown::Write)`
/// (ignoring errors) so the remote peer observes EOF promptly.
#[cfg(not(target_arch = "wasm32"))]
pub struct AsyncTcpOwnedWriteHalf {
    stream: Arc<AsyncTcpStream>,
}

#[cfg(not(target_arch = "wasm32"))]
impl AsyncTcpOwnedWriteHalf {
    /// Polls for writing bytes from `buf`.
    pub fn poll_write(&mut self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        self.stream.poll_write_ref(cx, buf)
    }

    /// Polls for shutting down the write half (`Shutdown::Write`).
    pub fn poll_shutdown(&mut self, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.stream.shutdown_write()
    }

    /// Attempts a non-blocking write without awaiting readiness.
    pub fn try_write(&self, buf: &[u8]) -> io::Result<usize> {
        self.stream.try_write_ref(buf)
    }

    /// Returns the local socket address of the underlying stream.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.stream.local_addr()
    }

    /// Returns the remote peer socket address of the underlying stream.
    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.stream.peer_addr()
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl AsyncWrite for AsyncTcpOwnedWriteHalf {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        AsyncTcpOwnedWriteHalf::poll_write(&mut self, cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        AsyncTcpOwnedWriteHalf::poll_shutdown(&mut self, cx)
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl fmt::Debug for AsyncTcpOwnedWriteHalf {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AsyncTcpOwnedWriteHalf")
            .field("stream", &*self.stream)
            .finish()
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for AsyncTcpOwnedWriteHalf {
    fn drop(&mut self) {
        let _ = self.stream.shutdown_how(Shutdown::Write);
    }
}

#[cfg(all(test, unix, not(target_arch = "wasm32")))]
mod tests_sigpipe {
    //! Child-process `SIGPIPE` probes for tests 20 and 27 (DESIGN.md §P2.8,
    //! decision Q7). Lives in `--lib` tests because test-only `signal()` FFI
    //! must stay inside allowlisted `sys/*.rs` files (`check-unsafe.sh`).

    use std::io::ErrorKind;
    use std::os::unix::process::ExitStatusExt;
    use std::path::PathBuf;
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;
    use crate::io::AsyncWriteExt;
    use crate::task::{self, Runtime};

    const ENV_CASE: &str = "AKKU_SIGPIPE_PROBE_CASE";

    struct RemoveOnDrop(PathBuf);
    impl Drop for RemoveOnDrop {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn temp_path(tag: &str) -> RemoveOnDrop {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "akku-sigpipe-{tag}-{}-{}.sock",
            std::process::id(),
            nanos
        ));
        let _ = std::fs::remove_file(&path);
        RemoveOnDrop(path)
    }

    fn run_child(test_name: &str, case: &str) -> std::process::Output {
        let exe = std::env::current_exe().expect("current_exe");
        Command::new(exe)
            .args(["--exact", test_name, "--nocapture", "--test-threads=1"])
            .env(ENV_CASE, case)
            .output()
            .expect("spawn sigpipe probe child")
    }

    fn assert_child_ok(case: &str, out: &std::process::Output) {
        assert_ne!(
            out.status.signal(),
            Some(13),
            "SIGPIPE probe ({case}) child was killed by SIGPIPE (signal 13)\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );
        assert!(
            out.status.success(),
            "SIGPIPE probe ({case}) child failed ({:?})\nstdout: {}\nstderr: {}",
            out.status,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );
    }

    /// Test 20: re-executes this test binary with `SIGPIPE` restored to
    /// `SIG_DFL` and verifies writes to closed peers return `BrokenPipe`
    /// (never signal 13) for connected, accepted, and `pair()` Unix streams
    /// as well as accepted and `from_std` TCP streams.
    #[test]
    fn p2_20_sigpipe_child_probe_streams() {
        if let Ok(case) = std::env::var(ENV_CASE) {
            sys::reset_sigpipe_to_default();
            let rt = Runtime::new().expect("runtime");
            rt.block_on(async move {
                match case.as_str() {
                    "unix_connect" => {
                        let path = temp_path("uconn");
                        let listener = AsyncUnixListener::bind(&path.0).expect("unix bind");
                        let p = path.0.clone();
                        let c = task::spawn(async move { AsyncUnixStream::connect(&p).await });
                        let (server, _) = listener.accept().await.expect("accept");
                        let mut client = c.await.unwrap().expect("connect");
                        drop(server);
                        let mut saw_err = None;
                        for _ in 0..64 {
                            if let Err(e) = client.write_all(&[b'x'; 4096]).await {
                                saw_err = Some(e.kind());
                                break;
                            }
                            task::yield_now().await;
                        }
                        assert!(
                            matches!(
                                saw_err,
                                Some(ErrorKind::BrokenPipe | ErrorKind::ConnectionReset)
                            ),
                            "expected BrokenPipe/ConnectionReset, got {saw_err:?}"
                        );
                    }
                    "unix_accept" => {
                        let path = temp_path("uacc");
                        let listener = AsyncUnixListener::bind(&path.0).expect("unix bind");
                        let p = path.0.clone();
                        let c = task::spawn(async move { AsyncUnixStream::connect(&p).await });
                        let (mut server, _) = listener.accept().await.expect("accept");
                        let client = c.await.unwrap().expect("connect");
                        drop(client);
                        let mut saw_err = None;
                        for _ in 0..64 {
                            if let Err(e) = server.write_all(&[b'x'; 4096]).await {
                                saw_err = Some(e.kind());
                                break;
                            }
                            task::yield_now().await;
                        }
                        assert!(
                            matches!(
                                saw_err,
                                Some(ErrorKind::BrokenPipe | ErrorKind::ConnectionReset)
                            ),
                            "expected BrokenPipe/ConnectionReset, got {saw_err:?}"
                        );
                    }
                    "unix_pair" => {
                        let (mut a, b) = AsyncUnixStream::pair().expect("pair");
                        drop(b);
                        let err = a.write_all(b"boom").await.unwrap_err();
                        assert!(
                            matches!(
                                err.kind(),
                                ErrorKind::BrokenPipe | ErrorKind::ConnectionReset
                            ),
                            "expected BrokenPipe/ConnectionReset, got {err:?}"
                        );
                    }
                    "tcp_accept" => {
                        let listener = AsyncTcpListener::bind("127.0.0.1:0").await.expect("bind");
                        let addr = listener.local_addr().expect("local_addr");
                        let c = task::spawn(async move { AsyncTcpStream::connect(addr).await });
                        let (mut server, _) = listener.accept().await.expect("accept");
                        let client = c.await.unwrap().expect("connect");
                        drop(client);
                        let mut saw_err = None;
                        for _ in 0..200 {
                            if let Err(e) = server.write_all(&[b'x'; 4096]).await {
                                saw_err = Some(e.kind());
                                break;
                            }
                            task::yield_now().await;
                        }
                        assert!(
                            matches!(
                                saw_err,
                                Some(ErrorKind::BrokenPipe | ErrorKind::ConnectionReset)
                            ),
                            "expected BrokenPipe/ConnectionReset, got {saw_err:?}"
                        );
                    }
                    "tcp_from_std" => {
                        let std_l = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
                        let addr = std_l.local_addr().expect("addr");
                        let std_c = std::net::TcpStream::connect(addr).expect("connect");
                        let (std_s, _) = std_l.accept().expect("accept");
                        let mut server = AsyncTcpStream::from_std(std_s).expect("from_std");
                        drop(std_c);
                        let mut saw_err = None;
                        for _ in 0..200 {
                            if let Err(e) = server.write_all(&[b'x'; 4096]).await {
                                saw_err = Some(e.kind());
                                break;
                            }
                            task::yield_now().await;
                        }
                        assert!(
                            matches!(
                                saw_err,
                                Some(ErrorKind::BrokenPipe | ErrorKind::ConnectionReset)
                            ),
                            "expected BrokenPipe/ConnectionReset, got {saw_err:?}"
                        );
                    }
                    other => panic!("unknown SIGPIPE probe case: {other}"),
                }
            });
            return;
        }

        let test_name = "net::tests_sigpipe::p2_20_sigpipe_child_probe_streams";
        for case in [
            "unix_connect",
            "unix_accept",
            "unix_pair",
            "tcp_accept",
            "tcp_from_std",
        ] {
            let out = run_child(test_name, case);
            assert_child_ok(case, &out);
        }
    }

    /// Test 27 (child probe): connected `AsyncUnixDatagram::send` after the
    /// peer is dropped returns an error (`ConnectionRefused`, `NotConnected`,
    /// or `BrokenPipe`) and never raises `SIGPIPE` when `SIGPIPE` is `SIG_DFL`.
    #[test]
    fn p2_27_sigpipe_child_probe_unix_datagram() {
        if let Ok(case) = std::env::var(ENV_CASE) {
            sys::reset_sigpipe_to_default();
            match case.as_str() {
                "akku_dgram" => {
                    let rt = Runtime::new().expect("runtime");
                    rt.block_on(async {
                        let (a, b) = AsyncUnixDatagram::pair().expect("dgram pair");
                        drop(b);
                        let err = a.send(b"hello").await.unwrap_err();
                        assert!(
                            matches!(
                                err.kind(),
                                ErrorKind::ConnectionRefused
                                    | ErrorKind::NotConnected
                                    | ErrorKind::BrokenPipe
                                    | ErrorKind::ConnectionReset
                            ),
                            "unexpected error on dropped dgram peer: {err:?}"
                        );
                    });
                }
                #[cfg(any(target_os = "linux", target_os = "android"))]
                "std_dgram_probe" => {
                    let (a, b) = std::os::unix::net::UnixDatagram::pair().expect("std pair");
                    drop(b);
                    let res = a.send(b"hello");
                    eprintln!("std UnixDatagram::send without MSG_NOSIGNAL returned: {res:?}");
                }
                other => panic!("unknown SIGPIPE dgram case: {other}"),
            }
            return;
        }

        let test_name = "net::tests_sigpipe::p2_27_sigpipe_child_probe_unix_datagram";
        let out = run_child(test_name, "akku_dgram");
        assert_child_ok("akku_dgram", &out);

        #[cfg(any(target_os = "linux", target_os = "android"))]
        {
            // Diagnostic probe per decision Q7: record whether Linux
            // `unix_dgram` raises SIGPIPE without MSG_NOSIGNAL (never fails
            // the test either way; MSG_NOSIGNAL stays).
            let probe = run_child(test_name, "std_dgram_probe");
            eprintln!(
                "Linux std unix_dgram probe status: {:?} (signal={:?})",
                probe.status,
                probe.status.signal()
            );
        }
    }
}
