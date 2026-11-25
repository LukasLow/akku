//! `std::net` superset with native asynchronous TCP sockets on `akku::task`.
//!
//! Every blocking type and address primitive from `std::net` is re-exported
//! under its original name (`TcpListener`, `TcpStream`, `UdpSocket`,
//! `SocketAddr`, `IpAddr`, `Shutdown`, and friends) on all targets, including
//! `wasm32`.
//!
//! On native (non-`wasm32`) targets, asynchronous TCP networking is provided by
//! [`AsyncTcpListener`], [`AsyncTcpStream`], and its borrowed and owned split
//! halves, integrated directly with [`crate::io::AsyncRead`] and
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
//!
//! # Platform support
//!
//! - **Native Tier-1 targets (Linux, Android, macOS, iOS, Windows):** full
//!   native readiness reactor support (`epoll`, `kqueue`, `WSAPoll`).
//! - **`wasm32`:** `std::net` address and blocking re-exports compile cleanly;
//!   native async socket types (`AsyncTcp*`) are `#[cfg(not(target_arch = "wasm32"))]`
//!   because raw OS TCP sockets are unavailable in `wasm32`.
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
pub(crate) mod reactor;
#[cfg(not(target_arch = "wasm32"))]
mod sys;

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
use crate::io::{AsyncRead, AsyncWrite};
#[cfg(not(target_arch = "wasm32"))]
use reactor::{Direction, Reactor, Registration};

/// Single-syscall read/write slice cap (`1 << 30`, as in Go's `internal/poll`).
#[cfg(not(target_arch = "wasm32"))]
const MAX_IO_LEN: usize = 1 << 30;

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
    pub async fn bind<A: ToSocketAddrs>(addr: A) -> io::Result<Self> {
        let reactor = crate::task::current_reactor()?;
        let listener = std::net::TcpListener::bind(addr)?;
        Self::register(listener, reactor)
    }

    fn register(listener: std::net::TcpListener, reactor: Arc<Reactor>) -> io::Result<Self> {
        listener.set_nonblocking(true)?;
        let reg = Registration::new(reactor, true)?;
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
    /// or lose an already-queued incoming connection.
    pub async fn accept(&self) -> io::Result<(AsyncTcpStream, SocketAddr)> {
        poll_fn(|cx| self.poll_accept(cx)).await
    }

    /// Polls for an incoming TCP connection.
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
    pub async fn connect<A: ToSocketAddrs>(addr: A) -> io::Result<Self> {
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
        let reg = Registration::new(reactor, initial_ready)?;
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
