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
//! ```no_run
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

#![allow(dead_code, unused_variables)]

pub use std::net::{
    AddrParseError, Incoming, IpAddr, Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, SocketAddrV4,
    SocketAddrV6, TcpListener, TcpStream, ToSocketAddrs, UdpSocket,
};

#[cfg(not(target_arch = "wasm32"))]
mod sys;

#[cfg(not(target_arch = "wasm32"))]
use std::fmt;
#[cfg(not(target_arch = "wasm32"))]
use std::io;
#[cfg(not(target_arch = "wasm32"))]
use std::marker::PhantomData;
#[cfg(all(unix, not(target_arch = "wasm32")))]
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, RawFd};
#[cfg(all(windows, not(target_arch = "wasm32")))]
use std::os::windows::io::{AsRawSocket, AsSocket, BorrowedSocket, RawSocket};
#[cfg(not(target_arch = "wasm32"))]
use std::pin::Pin;
#[cfg(not(target_arch = "wasm32"))]
use std::task::{Context, Poll};

#[cfg(not(target_arch = "wasm32"))]
use crate::io::{AsyncRead, AsyncWrite};

/// An async TCP socket server listening for incoming connections on the current
/// [`crate::task::Runtime`].
///
/// Dropping an `AsyncTcpListener` deregisters it from the OS reactor before
/// closing the underlying socket.
#[cfg(not(target_arch = "wasm32"))]
pub struct AsyncTcpListener {
    _priv: (),
}

#[cfg(not(target_arch = "wasm32"))]
impl AsyncTcpListener {
    /// Creates a new `AsyncTcpListener` bound to `addr`.
    ///
    /// Returns `Err(io::Error)` if called outside an active
    /// [`crate::task::Runtime`], if the runtime is shutting down, or if binding
    /// fails.
    pub async fn bind<A: ToSocketAddrs>(addr: A) -> io::Result<Self> {
        todo!()
    }

    /// Accepts a new incoming TCP connection, returning the non-blocking
    /// [`AsyncTcpStream`] and the remote peer address.
    ///
    /// This future is cancel-safe: dropping it before completion does not drop
    /// or lose an already-queued incoming connection.
    pub async fn accept(&self) -> io::Result<(AsyncTcpStream, SocketAddr)> {
        todo!()
    }

    /// Polls for an incoming TCP connection.
    pub fn poll_accept(
        &self,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<(AsyncTcpStream, SocketAddr)>> {
        todo!()
    }

    /// Wraps an existing [`std::net::TcpListener`], switching it to
    /// non-blocking mode and registering it with the current
    /// [`crate::task::Runtime`].
    ///
    /// Returns `Err(io::Error)` if called outside an active runtime.
    pub fn from_std(listener: std::net::TcpListener) -> io::Result<Self> {
        todo!()
    }

    /// Deregisters this listener from the reactor and returns the underlying
    /// [`std::net::TcpListener`] (still in non-blocking mode).
    pub fn into_std(self) -> io::Result<std::net::TcpListener> {
        todo!()
    }

    /// Returns the local socket address of this listener.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        todo!()
    }

    /// Gets the value of the `IP_TTL` option for this socket.
    pub fn ttl(&self) -> io::Result<u32> {
        todo!()
    }

    /// Sets the value of the `IP_TTL` option for this socket.
    pub fn set_ttl(&self, ttl: u32) -> io::Result<()> {
        todo!()
    }

    /// Gets the value of the `SO_ERROR` option on this socket.
    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        todo!()
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl fmt::Debug for AsyncTcpListener {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AsyncTcpListener").finish_non_exhaustive()
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for AsyncTcpListener {
    fn drop(&mut self) {
        // G3 stub: G4 deregisters from the reactor before the socket closes.
        // Deliberately not `todo!()`: a panicking `Drop` aborts during unwinding.
    }
}

#[cfg(all(unix, not(target_arch = "wasm32")))]
impl AsFd for AsyncTcpListener {
    fn as_fd(&self) -> BorrowedFd<'_> {
        todo!()
    }
}

#[cfg(all(unix, not(target_arch = "wasm32")))]
impl AsRawFd for AsyncTcpListener {
    fn as_raw_fd(&self) -> RawFd {
        todo!()
    }
}

#[cfg(all(windows, not(target_arch = "wasm32")))]
impl AsSocket for AsyncTcpListener {
    fn as_socket(&self) -> BorrowedSocket<'_> {
        todo!()
    }
}

#[cfg(all(windows, not(target_arch = "wasm32")))]
impl AsRawSocket for AsyncTcpListener {
    fn as_raw_socket(&self) -> RawSocket {
        todo!()
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
    _priv: (),
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
        todo!()
    }

    /// Wraps an existing [`std::net::TcpStream`], switching it to non-blocking
    /// mode and registering it with the current [`crate::task::Runtime`].
    ///
    /// Returns `Err(io::Error)` if called outside an active runtime.
    pub fn from_std(stream: std::net::TcpStream) -> io::Result<Self> {
        todo!()
    }

    /// Deregisters this stream from the reactor and returns the underlying
    /// [`std::net::TcpStream`] (still in non-blocking mode).
    pub fn into_std(self) -> io::Result<std::net::TcpStream> {
        todo!()
    }

    /// Polls for reading bytes into `buf`.
    ///
    /// Returns `Poll::Ready(Ok(0))` immediately when `buf` is empty or on EOF.
    pub fn poll_read(&mut self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        todo!()
    }

    /// Polls for writing bytes from `buf`.
    ///
    /// Returns `Poll::Ready(Ok(0))` immediately when `buf` is empty.
    pub fn poll_write(&mut self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        todo!()
    }

    /// Polls for shutting down the write half (`Shutdown::Write`) of this stream.
    pub fn poll_shutdown(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        todo!()
    }

    /// Attempts a non-blocking read without awaiting readiness.
    ///
    /// Returns `Err(io::ErrorKind::WouldBlock)` if no data is immediately
    /// available, clearing/re-arming readiness for a subsequent `.await`.
    pub fn try_read(&self, buf: &mut [u8]) -> io::Result<usize> {
        todo!()
    }

    /// Attempts a non-blocking write without awaiting readiness.
    ///
    /// Returns `Err(io::ErrorKind::WouldBlock)` if the socket send buffer is
    /// currently full, clearing/re-arming readiness for a subsequent `.await`.
    pub fn try_write(&self, buf: &[u8]) -> io::Result<usize> {
        todo!()
    }

    /// Shuts down the read, write, or both halves of this connection
    /// immediately.
    ///
    /// Named `shutdown_how` so it does not shadow 0-argument
    /// [`crate::io::AsyncWriteExt::shutdown`].
    pub fn shutdown_how(&self, how: Shutdown) -> io::Result<()> {
        todo!()
    }

    /// Splits this stream into borrowed read and write halves for concurrent
    /// full-duplex I/O without heap allocation.
    pub fn split(&mut self) -> (AsyncTcpReadHalf<'_>, AsyncTcpWriteHalf<'_>) {
        todo!()
    }

    /// Splits this stream into owned read and write halves that can be moved
    /// into separate tasks.
    pub fn into_split(self) -> (AsyncTcpOwnedReadHalf, AsyncTcpOwnedWriteHalf) {
        todo!()
    }

    /// Returns the local socket address of this stream.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        todo!()
    }

    /// Returns the remote peer socket address of this stream.
    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        todo!()
    }

    /// Gets the value of the `TCP_NODELAY` option on this socket.
    pub fn nodelay(&self) -> io::Result<bool> {
        todo!()
    }

    /// Sets the value of the `TCP_NODELAY` option on this socket.
    pub fn set_nodelay(&self, nodelay: bool) -> io::Result<()> {
        todo!()
    }

    /// Gets the value of the `IP_TTL` option on this socket.
    pub fn ttl(&self) -> io::Result<u32> {
        todo!()
    }

    /// Sets the value of the `IP_TTL` option on this socket.
    pub fn set_ttl(&self, ttl: u32) -> io::Result<()> {
        todo!()
    }

    /// Gets the value of the `SO_ERROR` option on this socket.
    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
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

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        todo!()
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        AsyncTcpStream::poll_shutdown(&mut self, cx)
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl fmt::Debug for AsyncTcpStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AsyncTcpStream").finish_non_exhaustive()
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for AsyncTcpStream {
    fn drop(&mut self) {
        // G3 stub: G4 deregisters from the reactor before the socket closes.
        // Deliberately not `todo!()`: a panicking `Drop` aborts during unwinding.
    }
}

#[cfg(all(unix, not(target_arch = "wasm32")))]
impl AsFd for AsyncTcpStream {
    fn as_fd(&self) -> BorrowedFd<'_> {
        todo!()
    }
}

#[cfg(all(unix, not(target_arch = "wasm32")))]
impl AsRawFd for AsyncTcpStream {
    fn as_raw_fd(&self) -> RawFd {
        todo!()
    }
}

#[cfg(all(windows, not(target_arch = "wasm32")))]
impl AsSocket for AsyncTcpStream {
    fn as_socket(&self) -> BorrowedSocket<'_> {
        todo!()
    }
}

#[cfg(all(windows, not(target_arch = "wasm32")))]
impl AsRawSocket for AsyncTcpStream {
    fn as_raw_socket(&self) -> RawSocket {
        todo!()
    }
}

/// Borrowed read half of an [`AsyncTcpStream`] (zero allocation).
#[cfg(not(target_arch = "wasm32"))]
pub struct AsyncTcpReadHalf<'a> {
    _stream: PhantomData<&'a AsyncTcpStream>,
}

#[cfg(not(target_arch = "wasm32"))]
impl AsyncTcpReadHalf<'_> {
    /// Polls for reading bytes into `buf`.
    pub fn poll_read(&mut self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        todo!()
    }

    /// Attempts a non-blocking read without awaiting readiness.
    pub fn try_read(&self, buf: &mut [u8]) -> io::Result<usize> {
        todo!()
    }

    /// Returns the local socket address of the underlying stream.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        todo!()
    }

    /// Returns the remote peer socket address of the underlying stream.
    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        todo!()
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
        f.debug_struct("AsyncTcpReadHalf").finish_non_exhaustive()
    }
}

/// Borrowed write half of an [`AsyncTcpStream`] (zero allocation).
#[cfg(not(target_arch = "wasm32"))]
pub struct AsyncTcpWriteHalf<'a> {
    _stream: PhantomData<&'a AsyncTcpStream>,
}

#[cfg(not(target_arch = "wasm32"))]
impl AsyncTcpWriteHalf<'_> {
    /// Polls for writing bytes from `buf`.
    pub fn poll_write(&mut self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        todo!()
    }

    /// Polls for shutting down the write half (`Shutdown::Write`).
    pub fn poll_shutdown(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        todo!()
    }

    /// Attempts a non-blocking write without awaiting readiness.
    pub fn try_write(&self, buf: &[u8]) -> io::Result<usize> {
        todo!()
    }

    /// Returns the local socket address of the underlying stream.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        todo!()
    }

    /// Returns the remote peer socket address of the underlying stream.
    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        todo!()
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

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        todo!()
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        AsyncTcpWriteHalf::poll_shutdown(&mut self, cx)
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl fmt::Debug for AsyncTcpWriteHalf<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AsyncTcpWriteHalf").finish_non_exhaustive()
    }
}

/// Owned read half of an [`AsyncTcpStream`], movable into a separate spawned task.
#[cfg(not(target_arch = "wasm32"))]
pub struct AsyncTcpOwnedReadHalf {
    _priv: (),
}

#[cfg(not(target_arch = "wasm32"))]
impl AsyncTcpOwnedReadHalf {
    /// Polls for reading bytes into `buf`.
    pub fn poll_read(&mut self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        todo!()
    }

    /// Attempts a non-blocking read without awaiting readiness.
    pub fn try_read(&self, buf: &mut [u8]) -> io::Result<usize> {
        todo!()
    }

    /// Returns the local socket address of the underlying stream.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        todo!()
    }

    /// Returns the remote peer socket address of the underlying stream.
    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        todo!()
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
            .finish_non_exhaustive()
    }
}

/// Owned write half of an [`AsyncTcpStream`], movable into a separate spawned task.
///
/// Dropping an `AsyncTcpOwnedWriteHalf` calls `shutdown(Shutdown::Write)`
/// (ignoring errors) so the remote peer observes EOF promptly.
#[cfg(not(target_arch = "wasm32"))]
pub struct AsyncTcpOwnedWriteHalf {
    _priv: (),
}

#[cfg(not(target_arch = "wasm32"))]
impl AsyncTcpOwnedWriteHalf {
    /// Polls for writing bytes from `buf`.
    pub fn poll_write(&mut self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        todo!()
    }

    /// Polls for shutting down the write half (`Shutdown::Write`).
    pub fn poll_shutdown(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        todo!()
    }

    /// Attempts a non-blocking write without awaiting readiness.
    pub fn try_write(&self, buf: &[u8]) -> io::Result<usize> {
        todo!()
    }

    /// Returns the local socket address of the underlying stream.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        todo!()
    }

    /// Returns the remote peer socket address of the underlying stream.
    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        todo!()
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

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        todo!()
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        AsyncTcpOwnedWriteHalf::poll_shutdown(&mut self, cx)
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl fmt::Debug for AsyncTcpOwnedWriteHalf {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AsyncTcpOwnedWriteHalf")
            .finish_non_exhaustive()
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for AsyncTcpOwnedWriteHalf {
    fn drop(&mut self) {
        // G3 stub: G4 calls `shutdown(Shutdown::Write)` and ignores errors.
        // Deliberately not `todo!()`: a panicking `Drop` aborts during unwinding.
    }
}
