//! Asynchronous Unix domain sockets (`UnixSocketAddr`, `AsyncUnixListener`,
//! `AsyncUnixStream` with halves, and Unix-only `AsyncUnixDatagram`).

use std::fmt;
use std::io;
use std::net::Shutdown;
#[cfg(unix)]
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, RawFd};
#[cfg(windows)]
use std::os::windows::io::{AsRawSocket, AsSocket, BorrowedSocket, RawSocket};
use std::path::Path;
#[cfg(windows)]
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use super::reactor::Registration;
use crate::io::{AsyncRead, AsyncWrite};

/// Address of a Unix domain socket: pathname, abstract (Linux/Android), or
/// unnamed.
///
/// Available on Unix (Linux, Android, macOS, iOS) and Windows 10 1803+.
#[derive(Clone)]
pub struct UnixSocketAddr {
    #[cfg(unix)]
    #[allow(dead_code)]
    inner: std::os::unix::net::SocketAddr,
    #[cfg(windows)]
    #[allow(dead_code)]
    inner: Option<PathBuf>,
}

#[allow(unused_variables)]
impl UnixSocketAddr {
    /// Creates a pathname Unix socket address.
    ///
    /// Returns `ErrorKind::InvalidInput` for an empty path, a path containing
    /// any NUL byte, or a path of `sun_path` length or more (`107` usable bytes
    /// on Linux, Android, and Windows; `103` on macOS and iOS). On Windows the
    /// path must also be valid UTF-8, otherwise `ErrorKind::InvalidInput` is
    /// returned.
    ///
    /// Note that on iOS, app container paths are long and count toward the
    /// `103`-byte limit; prefer a short relative path or temporary directory
    /// subpath there.
    pub fn from_pathname<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        todo!()
    }

    /// Creates an abstract-namespace Unix socket address (Linux and Android
    /// only).
    ///
    /// Returns `ErrorKind::InvalidInput` if `name` exceeds `107` bytes.
    /// Embedded NUL bytes inside `name` are permitted.
    ///
    /// # Security
    ///
    /// Abstract names have no filesystem entry and are not protected by file
    /// permission bits: any process in the same network namespace may connect.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    pub fn from_abstract_name<N: AsRef<[u8]>>(name: N) -> io::Result<Self> {
        todo!()
    }

    /// Returns the filesystem path if this is a pathname address.
    pub fn as_pathname(&self) -> Option<&Path> {
        todo!()
    }

    /// Returns the abstract namespace name if this is an abstract address
    /// (Linux and Android only).
    #[cfg(any(target_os = "linux", target_os = "android"))]
    pub fn as_abstract_name(&self) -> Option<&[u8]> {
        todo!()
    }

    /// Returns `true` if this address is unnamed (for example from `pair()`,
    /// `unbound()`, or an unbound connecting client).
    pub fn is_unnamed(&self) -> bool {
        todo!()
    }
}

impl fmt::Debug for UnixSocketAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UnixSocketAddr").finish_non_exhaustive()
    }
}

impl PartialEq for UnixSocketAddr {
    fn eq(&self, _other: &Self) -> bool {
        todo!()
    }
}

impl Eq for UnixSocketAddr {}

#[cfg(unix)]
impl From<std::os::unix::net::SocketAddr> for UnixSocketAddr {
    fn from(addr: std::os::unix::net::SocketAddr) -> Self {
        let _ = addr;
        todo!()
    }
}

#[cfg(unix)]
impl From<UnixSocketAddr> for std::os::unix::net::SocketAddr {
    fn from(addr: UnixSocketAddr) -> Self {
        let _ = addr;
        todo!()
    }
}

/// An async Unix domain stream listener on the current [`crate::task::Runtime`].
///
/// Available on Unix (Linux, Android, macOS, iOS) and Windows 10 1803+.
/// `accept` and `poll_accept` take `&self` with a shared waiter set, matching
/// [`super::AsyncTcpListener`].
///
/// # Unlink on drop
///
/// Listeners bound to a filesystem pathname via [`bind`](Self::bind) or
/// [`bind_addr`](Self::bind_addr) unlink their socket file on drop by default
/// (before closing the socket, and only if the path still refers to the same
/// socket file identity). `bind` never removes an existing file. Create socket
/// files inside a private parent directory if access control is needed, as
/// socket files inherit the process umask.
///
/// # Platform notes
///
/// - **Android:** SELinux may deny `bind` outside the app's private data
///   directory with `ErrorKind::PermissionDenied`; use the app's private
///   directory or an abstract address.
/// - **iOS:** sockets may become defunct when the app is suspended and return
///   errors on resume; recreate the listener on resume.
pub struct AsyncUnixListener {
    #[cfg(unix)]
    #[allow(dead_code)]
    inner: Option<std::os::unix::net::UnixListener>,
    #[cfg(windows)]
    #[allow(dead_code)]
    inner: Option<std::net::TcpStream>,
    #[allow(dead_code)]
    reg: Registration,
}

#[allow(unused_variables)]
impl AsyncUnixListener {
    /// Binds a new `AsyncUnixListener` to `path` and starts listening.
    ///
    /// Synchronous: no DNS resolution and no blocking wait. Unlink-on-drop is
    /// enabled by default for pathname sockets. Returns `Err(io::Error)` if
    /// called outside an active [`crate::task::Runtime`].
    pub fn bind<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        todo!()
    }

    /// Binds a new `AsyncUnixListener` to `addr` and starts listening.
    ///
    /// Unlink-on-drop is enabled by default for pathname addresses and disabled
    /// for abstract addresses.
    pub fn bind_addr(addr: &UnixSocketAddr) -> io::Result<Self> {
        todo!()
    }

    /// Accepts a new incoming Unix stream connection and its peer address.
    /// Cancel-safe.
    pub async fn accept(&self) -> io::Result<(AsyncUnixStream, UnixSocketAddr)> {
        todo!()
    }

    /// Polls for an incoming Unix stream connection.
    pub fn poll_accept(
        &self,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<(AsyncUnixStream, UnixSocketAddr)>> {
        todo!()
    }

    /// Wraps an existing [`std::os::unix::net::UnixListener`] and registers it
    /// with the current [`crate::task::Runtime`].
    ///
    /// Unlink-on-drop starts disabled for adopted sockets. This modifies
    /// `listener` by switching it to non-blocking mode (and on Darwin setting
    /// `SO_NOSIGPIPE`).
    #[cfg(unix)]
    pub fn from_std(listener: std::os::unix::net::UnixListener) -> io::Result<Self> {
        todo!()
    }

    /// Deregisters this listener from the reactor, disarms unlink-on-drop, and
    /// returns the underlying [`std::os::unix::net::UnixListener`].
    #[cfg(unix)]
    pub fn into_std(self) -> io::Result<std::os::unix::net::UnixListener> {
        todo!()
    }

    /// Returns the local address of this listener.
    pub fn local_addr(&self) -> io::Result<UnixSocketAddr> {
        todo!()
    }

    /// Gets the value of the `SO_ERROR` option on this socket.
    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        todo!()
    }

    /// Enables or disables automatic unlinking of the bound socket file on
    /// drop.
    ///
    /// Enabling records the current file identity. Returns
    /// `ErrorKind::InvalidInput` if this listener is not bound to a pathname.
    pub fn set_unlink_on_drop(&self, unlink: bool) -> io::Result<()> {
        todo!()
    }
}

impl fmt::Debug for AsyncUnixListener {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AsyncUnixListener").finish_non_exhaustive()
    }
}

impl Drop for AsyncUnixListener {
    fn drop(&mut self) {}
}

#[cfg(unix)]
impl AsFd for AsyncUnixListener {
    fn as_fd(&self) -> BorrowedFd<'_> {
        todo!()
    }
}

#[cfg(unix)]
impl AsRawFd for AsyncUnixListener {
    fn as_raw_fd(&self) -> RawFd {
        todo!()
    }
}

#[cfg(windows)]
impl AsSocket for AsyncUnixListener {
    fn as_socket(&self) -> BorrowedSocket<'_> {
        todo!()
    }
}

#[cfg(windows)]
impl AsRawSocket for AsyncUnixListener {
    fn as_raw_socket(&self) -> RawSocket {
        todo!()
    }
}

/// An async Unix domain byte stream on the current [`crate::task::Runtime`].
///
/// Available on Unix (Linux, Android, macOS, iOS) and Windows 10 1803+.
/// Implements [`AsyncRead`] and [`AsyncWrite`], so ergonomic async methods come
/// from [`crate::io::AsyncReadExt`] and [`crate::io::AsyncWriteExt`].
///
/// Writes never raise `SIGPIPE` on Unix (`MSG_NOSIGNAL` on Linux/Android,
/// `SO_NOSIGPIPE` on Darwin).
///
/// # Platform notes
///
/// - **Android:** SELinux may deny `connect` on filesystem paths outside the
///   app's private data directory with `ErrorKind::PermissionDenied`; use the
///   app's private directory or an abstract address.
/// - **iOS:** sockets become defunct when the app is suspended and return
///   errors on subsequent operations; reconnect on resume.
pub struct AsyncUnixStream {
    #[cfg(unix)]
    #[allow(dead_code)]
    inner: Option<std::os::unix::net::UnixStream>,
    #[cfg(windows)]
    #[allow(dead_code)]
    inner: Option<std::net::TcpStream>,
    #[allow(dead_code)]
    reg: Registration,
}

#[allow(unused_variables)]
impl AsyncUnixStream {
    /// Connects asynchronously to the Unix stream listener at `path` without
    /// blocking the runtime thread.
    ///
    /// Cancel-safe: dropping the future closes any in-progress socket cleanly.
    ///
    /// # Errors
    ///
    /// Returns `ErrorKind::WouldBlock` on Linux (`EAGAIN`) when the listener's
    /// backlog is full; Darwin reports `ErrorKind::ConnectionRefused` for the
    /// same condition.
    pub async fn connect<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        todo!()
    }

    /// Connects asynchronously to the Unix stream listener at `addr` without
    /// blocking the runtime thread.
    ///
    /// Cancel-safe.
    ///
    /// # Errors
    ///
    /// Returns `ErrorKind::WouldBlock` on Linux (`EAGAIN`) when the listener's
    /// backlog is full; Darwin reports `ErrorKind::ConnectionRefused` for the
    /// same condition.
    pub async fn connect_addr(addr: &UnixSocketAddr) -> io::Result<Self> {
        todo!()
    }

    /// Creates an unnamed connected pair of `AsyncUnixStream`s (Unix only).
    #[cfg(unix)]
    pub fn pair() -> io::Result<(Self, Self)> {
        todo!()
    }

    /// Wraps an existing [`std::os::unix::net::UnixStream`] and registers it
    /// with the current [`crate::task::Runtime`] (Unix only).
    ///
    /// This modifies `stream` by switching it to non-blocking mode and, on
    /// Darwin, setting `SO_NOSIGPIPE`.
    #[cfg(unix)]
    pub fn from_std(stream: std::os::unix::net::UnixStream) -> io::Result<Self> {
        todo!()
    }

    /// Deregisters this stream from the reactor and returns the underlying
    /// [`std::os::unix::net::UnixStream`] (still non-blocking; Unix only).
    #[cfg(unix)]
    pub fn into_std(self) -> io::Result<std::os::unix::net::UnixStream> {
        todo!()
    }

    /// Polls for reading bytes into `buf`.
    pub fn poll_read(&mut self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        todo!()
    }

    /// Polls for writing bytes from `buf`.
    pub fn poll_write(&mut self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        todo!()
    }

    /// Polls for shutting down the write half (`Shutdown::Write`).
    pub fn poll_shutdown(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        todo!()
    }

    /// Attempts a non-blocking read without awaiting readiness.
    pub fn try_read(&self, buf: &mut [u8]) -> io::Result<usize> {
        todo!()
    }

    /// Attempts a non-blocking write without awaiting readiness.
    pub fn try_write(&self, buf: &[u8]) -> io::Result<usize> {
        todo!()
    }

    /// Shuts down the read, write, or both halves of this stream immediately.
    ///
    /// Named `shutdown_how` so it does not shadow 0-argument
    /// [`crate::io::AsyncWriteExt::shutdown`].
    pub fn shutdown_how(&self, how: Shutdown) -> io::Result<()> {
        todo!()
    }

    /// Splits this stream into borrowed read and write halves without
    /// allocation.
    pub fn split(&mut self) -> (AsyncUnixReadHalf<'_>, AsyncUnixWriteHalf<'_>) {
        todo!()
    }

    /// Splits this stream into owned read and write halves movable into
    /// separate tasks.
    pub fn into_split(self) -> (AsyncUnixOwnedReadHalf, AsyncUnixOwnedWriteHalf) {
        todo!()
    }

    /// Returns the local address of this stream.
    pub fn local_addr(&self) -> io::Result<UnixSocketAddr> {
        todo!()
    }

    /// Returns the remote peer address of this stream.
    pub fn peer_addr(&self) -> io::Result<UnixSocketAddr> {
        todo!()
    }

    /// Gets the value of the `SO_ERROR` option on this socket.
    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        todo!()
    }
}

impl AsyncRead for AsyncUnixStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        AsyncUnixStream::poll_read(&mut self, cx, buf)
    }
}

impl AsyncWrite for AsyncUnixStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        AsyncUnixStream::poll_write(&mut self, cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        AsyncUnixStream::poll_shutdown(&mut self, cx)
    }
}

impl fmt::Debug for AsyncUnixStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AsyncUnixStream").finish_non_exhaustive()
    }
}

impl Drop for AsyncUnixStream {
    fn drop(&mut self) {}
}

#[cfg(unix)]
impl AsFd for AsyncUnixStream {
    fn as_fd(&self) -> BorrowedFd<'_> {
        todo!()
    }
}

#[cfg(unix)]
impl AsRawFd for AsyncUnixStream {
    fn as_raw_fd(&self) -> RawFd {
        todo!()
    }
}

#[cfg(windows)]
impl AsSocket for AsyncUnixStream {
    fn as_socket(&self) -> BorrowedSocket<'_> {
        todo!()
    }
}

#[cfg(windows)]
impl AsRawSocket for AsyncUnixStream {
    fn as_raw_socket(&self) -> RawSocket {
        todo!()
    }
}

/// Borrowed read half of an [`AsyncUnixStream`] (zero allocation).
pub struct AsyncUnixReadHalf<'a> {
    #[allow(dead_code)]
    stream: &'a AsyncUnixStream,
}

#[allow(unused_variables)]
impl AsyncUnixReadHalf<'_> {
    /// Polls for reading bytes into `buf`.
    pub fn poll_read(&mut self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        todo!()
    }

    /// Attempts a non-blocking read without awaiting readiness.
    pub fn try_read(&self, buf: &mut [u8]) -> io::Result<usize> {
        todo!()
    }

    /// Returns the local address of the underlying stream.
    pub fn local_addr(&self) -> io::Result<UnixSocketAddr> {
        todo!()
    }

    /// Returns the remote peer address of the underlying stream.
    pub fn peer_addr(&self) -> io::Result<UnixSocketAddr> {
        todo!()
    }
}

impl AsyncRead for AsyncUnixReadHalf<'_> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        AsyncUnixReadHalf::poll_read(&mut self, cx, buf)
    }
}

impl fmt::Debug for AsyncUnixReadHalf<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AsyncUnixReadHalf").finish_non_exhaustive()
    }
}

/// Borrowed write half of an [`AsyncUnixStream`] (zero allocation).
pub struct AsyncUnixWriteHalf<'a> {
    #[allow(dead_code)]
    stream: &'a AsyncUnixStream,
}

#[allow(unused_variables)]
impl AsyncUnixWriteHalf<'_> {
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

    /// Returns the local address of the underlying stream.
    pub fn local_addr(&self) -> io::Result<UnixSocketAddr> {
        todo!()
    }

    /// Returns the remote peer address of the underlying stream.
    pub fn peer_addr(&self) -> io::Result<UnixSocketAddr> {
        todo!()
    }
}

impl AsyncWrite for AsyncUnixWriteHalf<'_> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        AsyncUnixWriteHalf::poll_write(&mut self, cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        AsyncUnixWriteHalf::poll_shutdown(&mut self, cx)
    }
}

impl fmt::Debug for AsyncUnixWriteHalf<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AsyncUnixWriteHalf").finish_non_exhaustive()
    }
}

/// Owned read half of an [`AsyncUnixStream`], movable into a separate spawned
/// task.
pub struct AsyncUnixOwnedReadHalf {
    #[allow(dead_code)]
    stream: Arc<AsyncUnixStream>,
}

#[allow(unused_variables)]
impl AsyncUnixOwnedReadHalf {
    /// Polls for reading bytes into `buf`.
    pub fn poll_read(&mut self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        todo!()
    }

    /// Attempts a non-blocking read without awaiting readiness.
    pub fn try_read(&self, buf: &mut [u8]) -> io::Result<usize> {
        todo!()
    }

    /// Returns the local address of the underlying stream.
    pub fn local_addr(&self) -> io::Result<UnixSocketAddr> {
        todo!()
    }

    /// Returns the remote peer address of the underlying stream.
    pub fn peer_addr(&self) -> io::Result<UnixSocketAddr> {
        todo!()
    }
}

impl AsyncRead for AsyncUnixOwnedReadHalf {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        AsyncUnixOwnedReadHalf::poll_read(&mut self, cx, buf)
    }
}

impl fmt::Debug for AsyncUnixOwnedReadHalf {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AsyncUnixOwnedReadHalf")
            .finish_non_exhaustive()
    }
}

/// Owned write half of an [`AsyncUnixStream`], movable into a separate spawned
/// task.
///
/// Dropping an `AsyncUnixOwnedWriteHalf` calls `shutdown(Shutdown::Write)`
/// (ignoring errors) so the peer observes EOF promptly.
pub struct AsyncUnixOwnedWriteHalf {
    #[allow(dead_code)]
    stream: Arc<AsyncUnixStream>,
}

#[allow(unused_variables)]
impl AsyncUnixOwnedWriteHalf {
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

    /// Returns the local address of the underlying stream.
    pub fn local_addr(&self) -> io::Result<UnixSocketAddr> {
        todo!()
    }

    /// Returns the remote peer address of the underlying stream.
    pub fn peer_addr(&self) -> io::Result<UnixSocketAddr> {
        todo!()
    }
}

impl AsyncWrite for AsyncUnixOwnedWriteHalf {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        AsyncUnixOwnedWriteHalf::poll_write(&mut self, cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        AsyncUnixOwnedWriteHalf::poll_shutdown(&mut self, cx)
    }
}

impl fmt::Debug for AsyncUnixOwnedWriteHalf {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AsyncUnixOwnedWriteHalf")
            .finish_non_exhaustive()
    }
}

impl Drop for AsyncUnixOwnedWriteHalf {
    fn drop(&mut self) {}
}

/// An async Unix domain datagram socket on the current [`crate::task::Runtime`]
/// (Unix only).
///
/// All I/O methods take `&self` with shared waiter sets. Pathname sockets
/// created by [`bind`](Self::bind) or [`bind_addr`](Self::bind_addr) unlink
/// their file on drop by default (before closing, identity-checked).
///
/// # Backpressure
///
/// When the receiver's kernel queue is full, Darwin returns `ENOBUFS`
/// immediately (passed through as an `Err`), whereas Linux returns `EAGAIN`
/// (`WouldBlock`).
#[cfg(unix)]
pub struct AsyncUnixDatagram {
    #[allow(dead_code)]
    inner: Option<std::os::unix::net::UnixDatagram>,
    #[allow(dead_code)]
    reg: Registration,
}

#[cfg(unix)]
#[allow(unused_variables)]
impl AsyncUnixDatagram {
    /// Creates a new `AsyncUnixDatagram` bound to `path`.
    ///
    /// Unlink-on-drop is enabled by default. Returns `Err(io::Error)` if called
    /// outside an active [`crate::task::Runtime`].
    pub fn bind<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        todo!()
    }

    /// Creates a new `AsyncUnixDatagram` bound to `addr`.
    ///
    /// Unlink-on-drop is enabled by default for pathname addresses.
    pub fn bind_addr(addr: &UnixSocketAddr) -> io::Result<Self> {
        todo!()
    }

    /// Creates an unbound `AsyncUnixDatagram`.
    pub fn unbound() -> io::Result<Self> {
        todo!()
    }

    /// Creates an unnamed connected pair of `AsyncUnixDatagram` sockets.
    pub fn pair() -> io::Result<(Self, Self)> {
        todo!()
    }

    /// Connects this datagram socket to `path` as its default peer.
    ///
    /// Synchronous: a Unix datagram connect never blocks and needs no DNS.
    pub fn connect<P: AsRef<Path>>(&self, path: P) -> io::Result<()> {
        todo!()
    }

    /// Connects this datagram socket to `addr` as its default peer.
    pub fn connect_addr(&self, addr: &UnixSocketAddr) -> io::Result<()> {
        todo!()
    }

    /// Wraps an existing [`std::os::unix::net::UnixDatagram`] and registers it
    /// with the current [`crate::task::Runtime`].
    ///
    /// Unlink-on-drop starts disabled for adopted sockets. This modifies
    /// `socket` by switching it to non-blocking mode and, on Darwin, setting
    /// `SO_NOSIGPIPE`.
    pub fn from_std(socket: std::os::unix::net::UnixDatagram) -> io::Result<Self> {
        todo!()
    }

    /// Deregisters this socket from the reactor, disarms unlink-on-drop, and
    /// returns the underlying [`std::os::unix::net::UnixDatagram`].
    pub fn into_std(self) -> io::Result<std::os::unix::net::UnixDatagram> {
        todo!()
    }

    /// Sends a single datagram to the connected peer. Cancel-safe.
    pub async fn send(&self, buf: &[u8]) -> io::Result<usize> {
        todo!()
    }

    /// Receives a single datagram from the connected peer. Cancel-safe.
    pub async fn recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        todo!()
    }

    /// Sends a single datagram to `path`. Cancel-safe.
    pub async fn send_to<P: AsRef<Path>>(&self, buf: &[u8], path: P) -> io::Result<usize> {
        todo!()
    }

    /// Sends a single datagram to `addr`. Cancel-safe.
    pub async fn send_to_addr(&self, buf: &[u8], addr: &UnixSocketAddr) -> io::Result<usize> {
        todo!()
    }

    /// Receives a single datagram and returns the byte count and sender
    /// address. Cancel-safe.
    pub async fn recv_from(&self, buf: &mut [u8]) -> io::Result<(usize, UnixSocketAddr)> {
        todo!()
    }

    /// Polls for sending a datagram to the connected peer.
    pub fn poll_send(&self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        todo!()
    }

    /// Polls for receiving a datagram from the connected peer.
    pub fn poll_recv(&self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        todo!()
    }

    /// Polls for sending a datagram to `addr`.
    pub fn poll_send_to_addr(
        &self,
        cx: &mut Context<'_>,
        buf: &[u8],
        addr: &UnixSocketAddr,
    ) -> Poll<io::Result<usize>> {
        todo!()
    }

    /// Polls for receiving a datagram and its sender address.
    pub fn poll_recv_from(
        &self,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<(usize, UnixSocketAddr)>> {
        todo!()
    }

    /// Attempts a non-blocking send to the connected peer without awaiting.
    pub fn try_send(&self, buf: &[u8]) -> io::Result<usize> {
        todo!()
    }

    /// Attempts a non-blocking receive from the connected peer without
    /// awaiting.
    pub fn try_recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        todo!()
    }

    /// Attempts a non-blocking send to `addr` without awaiting.
    pub fn try_send_to_addr(&self, buf: &[u8], addr: &UnixSocketAddr) -> io::Result<usize> {
        todo!()
    }

    /// Attempts a non-blocking receive with sender address without awaiting.
    pub fn try_recv_from(&self, buf: &mut [u8]) -> io::Result<(usize, UnixSocketAddr)> {
        todo!()
    }

    /// Returns the local address of this datagram socket.
    pub fn local_addr(&self) -> io::Result<UnixSocketAddr> {
        todo!()
    }

    /// Returns the remote peer address of this datagram socket.
    pub fn peer_addr(&self) -> io::Result<UnixSocketAddr> {
        todo!()
    }

    /// Gets the value of the `SO_ERROR` option on this socket.
    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        todo!()
    }

    /// Shuts down the read, write, or both halves of this socket immediately.
    pub fn shutdown_how(&self, how: Shutdown) -> io::Result<()> {
        todo!()
    }

    /// Enables or disables automatic unlinking of the bound socket file on
    /// drop.
    ///
    /// Enabling records the current file identity. Returns
    /// `ErrorKind::InvalidInput` if this socket is not bound to a pathname.
    pub fn set_unlink_on_drop(&self, unlink: bool) -> io::Result<()> {
        todo!()
    }
}

#[cfg(unix)]
impl fmt::Debug for AsyncUnixDatagram {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AsyncUnixDatagram").finish_non_exhaustive()
    }
}

#[cfg(unix)]
impl Drop for AsyncUnixDatagram {
    fn drop(&mut self) {}
}

#[cfg(unix)]
impl AsFd for AsyncUnixDatagram {
    fn as_fd(&self) -> BorrowedFd<'_> {
        todo!()
    }
}

#[cfg(unix)]
impl AsRawFd for AsyncUnixDatagram {
    fn as_raw_fd(&self) -> RawFd {
        todo!()
    }
}
