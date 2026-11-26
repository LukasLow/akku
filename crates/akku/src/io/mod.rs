//! `std::io` superset with safe asynchronous byte stream traits.
//!
//! Every synchronous item from `std::io` is re-exported under its original name
//! and semantics (including synchronous [`copy`], [`Read`], [`Write`], and
//! [`BufRead`]). Asynchronous byte streams use the poll-based [`AsyncRead`],
//! [`AsyncWrite`], and [`AsyncBufRead`] traits together with the blanket
//! extension traits [`AsyncReadExt`], [`AsyncWriteExt`], and
//! [`AsyncBufReadExt`].
//!
//! # Overview
//!
//! | Task | API |
//! |---|---|
//! | Synchronous I/O (`std::io` re-exports) | [`Read`], [`Write`], [`BufRead`], [`copy`], [`Error`], [`ErrorKind`], [`Result`] |
//! | Poll-based async stream traits | [`AsyncRead`], [`AsyncWrite`], [`AsyncBufRead`] |
//! | Async read & copy helpers | [`AsyncReadExt::read`], [`AsyncReadExt::read_exact`], [`AsyncReadExt::copy_to`] |
//! | Async write & shutdown helpers | [`AsyncWriteExt::write`], [`AsyncWriteExt::write_all`], [`AsyncWriteExt::flush`], [`AsyncWriteExt::shutdown`] |
//! | Async buffered read helpers | [`AsyncBufReadExt::fill_buf`] |
//!
//! # Avoiding `E0034` ambiguity on in-memory types
//!
//! Standard in-memory types (`&[u8]`, [`Cursor`], [`Empty`], `Vec<u8>`, and
//! [`Sink`]) implement both synchronous `std::io` traits ([`Read`], [`BufRead`],
//! [`Write`]) and akku's async traits ([`AsyncRead`], [`AsyncBufRead`],
//! [`AsyncWrite`]).
//!
//! Importing both a synchronous trait and the matching async extension trait in
//! the same scope (for example `use akku::io::{Read, AsyncReadExt};` or a glob
//! `use akku::io::*;`) causes Rust `E0034` ("multiple applicable items in
//! scope") when calling `.read(...)`, `.write(...)`, `.flush()`, or
//! `.fill_buf()` on those in-memory types.
//!
//! - Prefer importing [`AsyncReadExt`], [`AsyncWriteExt`], and
//!   [`AsyncBufReadExt`] explicitly only in modules that perform async I/O, and
//!   avoid glob-importing `akku::io::*`.
//! - When a single function genuinely needs both synchronous and asynchronous
//!   methods on a dual-impl type, disambiguate with UFCS:
//!   `AsyncReadExt::read(&mut src, &mut buf).await` vs
//!   `Read::read(&mut src, &mut buf)`.
//! - Native async socket types such as `akku::net::AsyncTcpStream` implement
//!   only [`AsyncRead`] and [`AsyncWrite`], so they never trigger `E0034`.
//!
//! # Limits and defensive contracts
//!
//! - [`AsyncReadExt::copy_to`] allocates a single fixed 8 KiB (`8_192`-byte)
//!   buffer per call and flushes the writer on EOF and whenever the internal
//!   buffer drains while the reader returns `Poll::Pending`. It does not call
//!   `shutdown` on the writer.
//! - Zero-length slice operations (`read(&mut [])`, `write(&[])`,
//!   `read_exact(&mut [])`, `write_all(&[])`) return immediately without polling
//!   the underlying stream.
//! - [`AsyncReadExt::read_exact`], [`AsyncWriteExt::write_all`], and
//!   [`AsyncReadExt::copy_to`] retry [`ErrorKind::Interrupted`] automatically,
//!   return [`ErrorKind::WriteZero`] if a writer reports `Ok(0)` on non-empty
//!   input, and return [`ErrorKind::InvalidData`] if a buggy stream reports more
//!   bytes read or written than the slice length.
//!
//! # Examples
//!
//! Copying between async streams with [`AsyncReadExt::copy_to`]:
//!
//! ```
//! use akku::io::AsyncReadExt;
//!
//! # akku::task::block_on(async {
//! let mut src: &[u8] = b"hello akku";
//! let mut dst = Vec::new();
//! let copied = src.copy_to(&mut dst).await?;
//! assert_eq!(copied, 10);
//! assert_eq!(dst, b"hello akku");
//! # Ok::<(), akku::io::Error>(())
//! # }).unwrap();
//! ```
//!
//! Disambiguating sync and async calls on a dual-impl type when both traits are
//! in scope:
//!
//! ```
//! use akku::io::{AsyncReadExt, Read};
//!
//! # akku::task::block_on(async {
//! let mut src: &[u8] = b"abcdef";
//! let mut a = [0u8; 3];
//! let mut b = [0u8; 3];
//! assert_eq!(Read::read(&mut src, &mut a)?, 3);
//! assert_eq!(AsyncReadExt::read(&mut src, &mut b).await?, 3);
//! assert_eq!(&a, b"abc");
//! assert_eq!(&b, b"def");
//! # Ok::<(), akku::io::Error>(())
//! # }).unwrap();
//! ```

use std::fmt;
use std::future::Future;
use std::ops::DerefMut;
use std::pin::Pin;
use std::task::{ready, Context, Poll};

pub use std::io::{
    copy, empty, read_to_string, repeat, sink, stderr, stdin, stdout, BufRead, BufReader,
    BufWriter, Bytes, Chain, Cursor, Empty, Error, ErrorKind, IntoInnerError, IoSlice, IoSliceMut,
    LineWriter, Lines, Read, Repeat, Result, Seek, SeekFrom, Sink, Split, Stderr, StderrLock,
    Stdin, StdinLock, Stdout, StdoutLock, Take, Write, WriterPanicked,
};

/// Fixed size of the single buffer allocated by [`AsyncReadExt::copy_to`].
const COPY_BUF_SIZE: usize = 8_192;

fn over_reported(what: &str) -> Error {
    Error::new(
        ErrorKind::InvalidData,
        format!("{what} reported more bytes than the buffer holds"),
    )
}

fn write_zero() -> Error {
    Error::new(ErrorKind::WriteZero, "writer accepted zero bytes")
}

/// Asynchronous byte reader operating on initialized `&mut [u8]` buffers.
///
/// This trait is dyn-compatible (`dyn AsyncRead`).
pub trait AsyncRead {
    /// Attempts to read bytes into `buf`.
    ///
    /// - `Poll::Ready(Ok(0))` indicates EOF (unless `buf.is_empty()`).
    /// - `Poll::Ready(Ok(n))` (`n <= buf.len()`) indicates `n` bytes were read
    ///   into `buf[..n]`.
    /// - `Poll::Pending` indicates no bytes are ready yet and `cx.waker()` was
    ///   registered for wakeup.
    ///
    /// Implementations must translate OS `WouldBlock` into `Poll::Pending` and
    /// should retry [`ErrorKind::Interrupted`] internally.
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut [u8])
        -> Poll<Result<usize>>;
}

/// Asynchronous byte writer.
///
/// This trait is dyn-compatible (`dyn AsyncWrite`).
pub trait AsyncWrite {
    /// Attempts to write bytes from `buf`.
    ///
    /// - `Poll::Ready(Ok(n))` (`n <= buf.len()`) indicates `n` bytes were
    ///   written.
    /// - `Poll::Pending` indicates the sink cannot accept bytes yet and
    ///   `cx.waker()` was registered for wakeup.
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<Result<usize>>;

    /// Flushes buffered output, ensuring all intermediately buffered bytes
    /// reach the destination.
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>>;

    /// Gracefully shuts down the output stream (flushing buffered bytes first).
    ///
    /// For full-duplex transports such as TCP, this shuts down the write half
    /// (`Shutdown::Write`) while leaving the read half open.
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>>;
}

/// Asynchronous buffered reader (`AsyncRead` with an internal byte buffer).
///
/// This trait is dyn-compatible (`dyn AsyncBufRead`).
pub trait AsyncBufRead: AsyncRead {
    /// Returns the contents of the internal buffer, filling it from the
    /// underlying reader if empty.
    ///
    /// An empty slice (`&[]`) indicates EOF. Calling `poll_fill_buf` repeatedly
    /// without calling [`consume`](Self::consume) returns the same unconsumed
    /// buffer slice.
    fn poll_fill_buf(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<&[u8]>>;

    /// Marks `amt` bytes as consumed from the buffer returned by
    /// [`poll_fill_buf`](Self::poll_fill_buf).
    ///
    /// Callers must pass `amt <= buf.len()`.
    fn consume(self: Pin<&mut Self>, amt: usize);
}

impl<T: AsyncRead + Unpin + ?Sized> AsyncRead for &mut T {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<Result<usize>> {
        Pin::new(&mut **self.get_mut()).poll_read(cx, buf)
    }
}

impl<T: AsyncRead + Unpin + ?Sized> AsyncRead for Box<T> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<Result<usize>> {
        Pin::new(&mut **self.get_mut()).poll_read(cx, buf)
    }
}

impl<P> AsyncRead for Pin<P>
where
    P: DerefMut + Unpin,
    P::Target: AsyncRead,
{
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<Result<usize>> {
        self.get_mut().as_mut().poll_read(cx, buf)
    }
}

impl<T: AsyncWrite + Unpin + ?Sized> AsyncWrite for &mut T {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<Result<usize>> {
        Pin::new(&mut **self.get_mut()).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        Pin::new(&mut **self.get_mut()).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        Pin::new(&mut **self.get_mut()).poll_shutdown(cx)
    }
}

impl<T: AsyncWrite + Unpin + ?Sized> AsyncWrite for Box<T> {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<Result<usize>> {
        Pin::new(&mut **self.get_mut()).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        Pin::new(&mut **self.get_mut()).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        Pin::new(&mut **self.get_mut()).poll_shutdown(cx)
    }
}

impl<P> AsyncWrite for Pin<P>
where
    P: DerefMut + Unpin,
    P::Target: AsyncWrite,
{
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<Result<usize>> {
        self.get_mut().as_mut().poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        self.get_mut().as_mut().poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        self.get_mut().as_mut().poll_shutdown(cx)
    }
}

impl<T: AsyncBufRead + Unpin + ?Sized> AsyncBufRead for &mut T {
    fn poll_fill_buf(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<&[u8]>> {
        Pin::new(&mut **self.get_mut()).poll_fill_buf(cx)
    }

    fn consume(self: Pin<&mut Self>, amt: usize) {
        Pin::new(&mut **self.get_mut()).consume(amt);
    }
}

impl<T: AsyncBufRead + Unpin + ?Sized> AsyncBufRead for Box<T> {
    fn poll_fill_buf(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<&[u8]>> {
        Pin::new(&mut **self.get_mut()).poll_fill_buf(cx)
    }

    fn consume(self: Pin<&mut Self>, amt: usize) {
        Pin::new(&mut **self.get_mut()).consume(amt);
    }
}

impl<P> AsyncBufRead for Pin<P>
where
    P: DerefMut + Unpin,
    P::Target: AsyncBufRead,
{
    fn poll_fill_buf(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<&[u8]>> {
        self.get_mut().as_mut().poll_fill_buf(cx)
    }

    fn consume(self: Pin<&mut Self>, amt: usize) {
        self.get_mut().as_mut().consume(amt);
    }
}

impl AsyncRead for &[u8] {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<Result<usize>> {
        Poll::Ready(Read::read(self.get_mut(), buf))
    }
}

impl AsyncBufRead for &[u8] {
    fn poll_fill_buf(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<&[u8]>> {
        Poll::Ready(Ok(&**self.get_mut()))
    }

    fn consume(self: Pin<&mut Self>, amt: usize) {
        let this = self.get_mut();
        *this = &this[amt.min(this.len())..];
    }
}

impl<T: AsRef<[u8]> + Unpin> AsyncRead for Cursor<T> {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<Result<usize>> {
        Poll::Ready(Read::read(self.get_mut(), buf))
    }
}

impl<T: AsRef<[u8]> + Unpin> AsyncBufRead for Cursor<T> {
    fn poll_fill_buf(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<&[u8]>> {
        Poll::Ready(BufRead::fill_buf(self.get_mut()))
    }

    fn consume(self: Pin<&mut Self>, amt: usize) {
        BufRead::consume(self.get_mut(), amt);
    }
}

impl AsyncWrite for Vec<u8> {
    fn poll_write(self: Pin<&mut Self>, _cx: &mut Context<'_>, buf: &[u8]) -> Poll<Result<usize>> {
        self.get_mut().extend_from_slice(buf);
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<()>> {
        Poll::Ready(Ok(()))
    }
}

impl AsyncRead for Empty {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _buf: &mut [u8],
    ) -> Poll<Result<usize>> {
        Poll::Ready(Ok(0))
    }
}

impl AsyncBufRead for Empty {
    fn poll_fill_buf(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<&[u8]>> {
        Poll::Ready(Ok(&[]))
    }

    fn consume(self: Pin<&mut Self>, _amt: usize) {}
}

impl AsyncWrite for Sink {
    fn poll_write(self: Pin<&mut Self>, _cx: &mut Context<'_>, buf: &[u8]) -> Poll<Result<usize>> {
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<()>> {
        Poll::Ready(Ok(()))
    }
}

/// Extension methods for any [`AsyncRead`] value.
pub trait AsyncReadExt: AsyncRead {
    /// Reads up to `buf.len()` bytes into `buf`, returning `Ok(0)` on EOF (or
    /// immediately if `buf` is empty).
    fn read<'a>(&'a mut self, buf: &'a mut [u8]) -> ReadFuture<'a, Self>
    where
        Self: Unpin,
    {
        ReadFuture { reader: self, buf }
    }

    /// Reads the exact number of bytes required to fill `buf`.
    ///
    /// Retries [`ErrorKind::Interrupted`]. Returns [`ErrorKind::UnexpectedEof`]
    /// if EOF is reached before `buf` is full, and [`ErrorKind::InvalidData`] if
    /// the underlying reader reports more bytes than requested. Returns `Ok(())`
    /// immediately if `buf.is_empty()`.
    ///
    /// Dropping the future keeps the progress already made: bytes read so far
    /// stay in the front of `buf` and are consumed from the stream.
    fn read_exact<'a>(&'a mut self, buf: &'a mut [u8]) -> ReadExactFuture<'a, Self>
    where
        Self: Unpin,
    {
        ReadExactFuture {
            reader: self,
            buf,
            filled: 0,
        }
    }

    /// Copies all bytes from `self` into `writer` using a fixed 8 KiB internal
    /// buffer, returning the total number of bytes copied.
    ///
    /// Retries [`ErrorKind::Interrupted`], flushes `writer` on EOF and whenever
    /// the buffer drains while `self` returns `Poll::Pending`, and returns
    /// [`ErrorKind::WriteZero`] if `writer` returns `Ok(0)` with pending bytes.
    /// Does not call `shutdown` on `writer`.
    ///
    /// Every chunk is written completely before the next read, so dropping the
    /// future never loses bytes already handed to a ready writer.
    fn copy_to<'a, W>(&'a mut self, writer: &'a mut W) -> CopyToFuture<'a, Self, W>
    where
        Self: Unpin,
        W: AsyncWrite + Unpin + ?Sized,
    {
        CopyToFuture {
            reader: self,
            writer,
            buf: vec![0u8; COPY_BUF_SIZE].into_boxed_slice(),
            pos: 0,
            cap: 0,
            total: 0,
            read_done: false,
            need_flush: false,
        }
    }
}

impl<R: AsyncRead + ?Sized> AsyncReadExt for R {}

/// Extension methods for any [`AsyncWrite`] value.
pub trait AsyncWriteExt: AsyncWrite {
    /// Writes up to `buf.len()` bytes from `buf`, returning how many bytes were
    /// written (or `Ok(0)` immediately if `buf` is empty).
    fn write<'a>(&'a mut self, buf: &'a [u8]) -> WriteFuture<'a, Self>
    where
        Self: Unpin,
    {
        WriteFuture { writer: self, buf }
    }

    /// Writes the entire `buf`, retrying [`ErrorKind::Interrupted`] and short
    /// writes.
    ///
    /// Returns [`ErrorKind::WriteZero`] if `poll_write` returns `Ok(0)` while
    /// bytes remain, and [`ErrorKind::InvalidData`] if the writer reports more
    /// bytes written than provided. Returns `Ok(())` immediately if
    /// `buf.is_empty()`.
    ///
    /// Dropping the future keeps the progress already made: bytes accepted by
    /// the writer so far stay written.
    fn write_all<'a>(&'a mut self, buf: &'a [u8]) -> WriteAllFuture<'a, Self>
    where
        Self: Unpin,
    {
        WriteAllFuture { writer: self, buf }
    }

    /// Flushes this writer, ensuring all buffered data reaches the underlying
    /// sink.
    fn flush(&mut self) -> FlushFuture<'_, Self>
    where
        Self: Unpin,
    {
        FlushFuture { writer: self }
    }

    /// Gracefully shuts down the writer (flushing buffered data first).
    fn shutdown(&mut self) -> ShutdownFuture<'_, Self>
    where
        Self: Unpin,
    {
        ShutdownFuture { writer: self }
    }
}

impl<W: AsyncWrite + ?Sized> AsyncWriteExt for W {}

/// Extension methods for any [`AsyncBufRead`] value.
pub trait AsyncBufReadExt: AsyncBufRead {
    /// Awaits the internal buffer contents, filling from the underlying reader
    /// if empty.
    fn fill_buf(&mut self) -> FillBufFuture<'_, Self>
    where
        Self: Unpin,
    {
        FillBufFuture { reader: Some(self) }
    }
}

impl<R: AsyncBufRead + ?Sized> AsyncBufReadExt for R {}

/// Future returned by [`AsyncReadExt::read`].
#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct ReadFuture<'a, R: ?Sized> {
    reader: &'a mut R,
    buf: &'a mut [u8],
}

impl<R: AsyncRead + Unpin + ?Sized> Future for ReadFuture<'_, R> {
    type Output = Result<usize>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        if this.buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let n = ready!(Pin::new(&mut *this.reader).poll_read(cx, this.buf))?;
        if n > this.buf.len() {
            return Poll::Ready(Err(over_reported("reader")));
        }
        Poll::Ready(Ok(n))
    }
}

impl<R: ?Sized> fmt::Debug for ReadFuture<'_, R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReadFuture").finish_non_exhaustive()
    }
}

/// Future returned by [`AsyncReadExt::read_exact`].
#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct ReadExactFuture<'a, R: ?Sized> {
    reader: &'a mut R,
    buf: &'a mut [u8],
    filled: usize,
}

impl<R: AsyncRead + Unpin + ?Sized> Future for ReadExactFuture<'_, R> {
    type Output = Result<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        while this.filled < this.buf.len() {
            let rest = &mut this.buf[this.filled..];
            let remaining = rest.len();
            match ready!(Pin::new(&mut *this.reader).poll_read(cx, rest)) {
                Ok(0) => {
                    return Poll::Ready(Err(Error::new(
                        ErrorKind::UnexpectedEof,
                        "stream ended before the buffer was filled",
                    )))
                }
                Ok(n) if n > remaining => return Poll::Ready(Err(over_reported("reader"))),
                Ok(n) => this.filled += n,
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => return Poll::Ready(Err(e)),
            }
        }
        Poll::Ready(Ok(()))
    }
}

impl<R: ?Sized> fmt::Debug for ReadExactFuture<'_, R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReadExactFuture")
            .field("filled", &self.filled)
            .field("len", &self.buf.len())
            .finish_non_exhaustive()
    }
}

/// Future returned by [`AsyncReadExt::copy_to`].
#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct CopyToFuture<'a, R: ?Sized, W: ?Sized> {
    reader: &'a mut R,
    writer: &'a mut W,
    buf: Box<[u8]>,
    pos: usize,
    cap: usize,
    total: u64,
    read_done: bool,
    need_flush: bool,
}

impl<R: AsyncRead + Unpin + ?Sized, W: AsyncWrite + Unpin + ?Sized> Future
    for CopyToFuture<'_, R, W>
{
    type Output = Result<u64>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        loop {
            // Refill only once the previous chunk is fully written.
            if this.pos == this.cap && !this.read_done {
                match Pin::new(&mut *this.reader).poll_read(cx, &mut this.buf) {
                    Poll::Pending => {
                        // Push buffered bytes out before waiting on the reader:
                        // the peer may be waiting for them (request/response).
                        if this.need_flush {
                            ready!(Pin::new(&mut *this.writer).poll_flush(cx))?;
                            this.need_flush = false;
                        }
                        return Poll::Pending;
                    }
                    Poll::Ready(Ok(0)) => this.read_done = true,
                    Poll::Ready(Ok(n)) if n > this.buf.len() => {
                        return Poll::Ready(Err(over_reported("reader")))
                    }
                    Poll::Ready(Ok(n)) => {
                        this.pos = 0;
                        this.cap = n;
                    }
                    Poll::Ready(Err(e)) if e.kind() == ErrorKind::Interrupted => continue,
                    Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                }
            }

            while this.pos < this.cap {
                let chunk = &this.buf[this.pos..this.cap];
                match ready!(Pin::new(&mut *this.writer).poll_write(cx, chunk)) {
                    Ok(0) => return Poll::Ready(Err(write_zero())),
                    Ok(n) if n > chunk.len() => return Poll::Ready(Err(over_reported("writer"))),
                    Ok(n) => {
                        this.pos += n;
                        this.total = this.total.saturating_add(n as u64);
                        this.need_flush = true;
                    }
                    Err(e) if e.kind() == ErrorKind::Interrupted => {}
                    Err(e) => return Poll::Ready(Err(e)),
                }
            }

            if this.read_done {
                ready!(Pin::new(&mut *this.writer).poll_flush(cx))?;
                this.need_flush = false;
                return Poll::Ready(Ok(this.total));
            }
        }
    }
}

impl<R: ?Sized, W: ?Sized> fmt::Debug for CopyToFuture<'_, R, W> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CopyToFuture")
            .field("copied", &self.total)
            .finish_non_exhaustive()
    }
}

/// Future returned by [`AsyncWriteExt::write`].
#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct WriteFuture<'a, W: ?Sized> {
    writer: &'a mut W,
    buf: &'a [u8],
}

impl<W: AsyncWrite + Unpin + ?Sized> Future for WriteFuture<'_, W> {
    type Output = Result<usize>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        if this.buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let n = ready!(Pin::new(&mut *this.writer).poll_write(cx, this.buf))?;
        if n > this.buf.len() {
            return Poll::Ready(Err(over_reported("writer")));
        }
        Poll::Ready(Ok(n))
    }
}

impl<W: ?Sized> fmt::Debug for WriteFuture<'_, W> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WriteFuture").finish_non_exhaustive()
    }
}

/// Future returned by [`AsyncWriteExt::write_all`].
#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct WriteAllFuture<'a, W: ?Sized> {
    writer: &'a mut W,
    buf: &'a [u8],
}

impl<W: AsyncWrite + Unpin + ?Sized> Future for WriteAllFuture<'_, W> {
    type Output = Result<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        while !this.buf.is_empty() {
            match ready!(Pin::new(&mut *this.writer).poll_write(cx, this.buf)) {
                Ok(0) => return Poll::Ready(Err(write_zero())),
                Ok(n) if n > this.buf.len() => return Poll::Ready(Err(over_reported("writer"))),
                Ok(n) => this.buf = &this.buf[n..],
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => return Poll::Ready(Err(e)),
            }
        }
        Poll::Ready(Ok(()))
    }
}

impl<W: ?Sized> fmt::Debug for WriteAllFuture<'_, W> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WriteAllFuture")
            .field("remaining", &self.buf.len())
            .finish_non_exhaustive()
    }
}

/// Future returned by [`AsyncWriteExt::flush`].
#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct FlushFuture<'a, W: ?Sized> {
    writer: &'a mut W,
}

impl<W: AsyncWrite + Unpin + ?Sized> Future for FlushFuture<'_, W> {
    type Output = Result<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut *self.get_mut().writer).poll_flush(cx)
    }
}

impl<W: ?Sized> fmt::Debug for FlushFuture<'_, W> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FlushFuture").finish_non_exhaustive()
    }
}

/// Future returned by [`AsyncWriteExt::shutdown`].
#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct ShutdownFuture<'a, W: ?Sized> {
    writer: &'a mut W,
}

impl<W: AsyncWrite + Unpin + ?Sized> Future for ShutdownFuture<'_, W> {
    type Output = Result<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut *self.get_mut().writer).poll_shutdown(cx)
    }
}

impl<W: ?Sized> fmt::Debug for ShutdownFuture<'_, W> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ShutdownFuture").finish_non_exhaustive()
    }
}

/// Future returned by [`AsyncBufReadExt::fill_buf`].
#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct FillBufFuture<'a, R: ?Sized> {
    reader: Option<&'a mut R>,
}

impl<'a, R: AsyncBufRead + Unpin + ?Sized> Future for FillBufFuture<'a, R> {
    type Output = Result<&'a [u8]>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let reader = this
            .reader
            .take()
            .expect("FillBufFuture polled after completion");
        // The first poll borrows `reader` only for this call, so it can be put
        // back on `Pending`. Once it is ready, poll again with the full `'a`
        // borrow: `poll_fill_buf` without `consume` returns the same bytes.
        match Pin::new(&mut *reader).poll_fill_buf(cx) {
            Poll::Pending => {
                this.reader = Some(reader);
                Poll::Pending
            }
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
            Poll::Ready(Ok(_)) => match Pin::new(reader).poll_fill_buf(cx) {
                Poll::Ready(result) => Poll::Ready(result),
                Poll::Pending => Poll::Ready(Err(Error::other(
                    "AsyncBufRead::poll_fill_buf returned Pending right after Ready",
                ))),
            },
        }
    }
}

impl<R: ?Sized> fmt::Debug for FillBufFuture<'_, R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FillBufFuture").finish_non_exhaustive()
    }
}
