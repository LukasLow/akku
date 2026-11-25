# io — design record

## Purpose

`akku::io` is akku's `std::io` superset providing synchronous `std::io` re-exports (including `copy`) alongside akku's zero-dependency, 100%-safe async I/O traits (`AsyncRead`, `AsyncWrite`, `AsyncBufRead`) and extension methods (`read`, `read_exact`, `copy_to`, `write`, `write_all`, `flush`, `shutdown`, `fill_buf`).

Rust `std` has no stable async I/O traits, while the ecosystem is split between `tokio::io` (`ReadBuf`, which requires `unsafe` and `MaybeUninit<u8>`) and `futures-io` (`&mut [u8]`). Every async transport and protocol in `akku` — `akku::net::AsyncTcpStream` (Phase 1), `akku::tls`, `akku::http` (Phases 5–8), `akku::ws`, `akku::sse`, `akku::nfs`, and `akku::archive` — needs one coherent, zero-dependency, `#![deny(unsafe_code)]`-clean async I/O contract.

**Phase 1 scope (human decision, 2026-11-25):**
1. Re-export `std::io` core error, slice, and synchronous I/O items (including `copy`) by explicit list so `use akku::io` works ergonomically as a full `std::io` superset (`Namespace.md` §3).
2. Define `AsyncRead`, `AsyncWrite`, and `AsyncBufRead` using the 100%-safe `&mut [u8]` buffer model (`0` `unsafe` blocks).
3. Provide blanket extension traits (`AsyncReadExt`, `AsyncWriteExt`, `AsyncBufReadExt`) for `read`, `read_exact`, `copy_to`, `write`, `write_all`, `flush`, `shutdown`, and `fill_buf`.
4. Implement `AsyncRead` / `AsyncWrite` / `AsyncBufRead` for core in-memory types (`&[u8]`, `Cursor<T>`, `Vec<u8>`, `Empty`, `Sink`, `&mut T`, `Box<T>`, `Pin<P>`) and `AsyncRead` / `AsyncWrite` for `akku::net::{AsyncTcpStream, AsyncTcpReadHalf, AsyncTcpWriteHalf, AsyncTcpOwnedReadHalf, AsyncTcpOwnedWriteHalf}`.

## Non-goals

- **No `unsafe` anywhere in `akku::io`.** Per `AGENTS.md` rule 9, `akku::io` has no `sys/` submodule and stays strictly `#![deny(unsafe_code)]` (`0` `unsafe` blocks). No `ReadBuf`/`MaybeUninit<u8>` raw-slice transmutation.
- **No async buffered wrapper structs (`AsyncBufReader`, `AsyncBufWriter`) or unbounded string readers (`read_to_end`, `read_to_string`, `read_line`) in Phase 1.** Tracked in `_dev/TODO.md`; when added, every growing reader will require an explicit byte limit (no unbounded OOM readers).
- **No `AsyncSeek` or vectored I/O (`poll_read_vectored` / `poll_write_vectored`) in Phase 1.** Tracked in `_dev/TODO.md` for when async file I/O or HTTP/NFS scatter-gather framing needs them.
- **No `copy_bidirectional` in Phase 1.** Tracked in `_dev/TODO.md`.
- **No async `stdin`/`stdout`/`stderr` wrappers in Phase 1.** Synchronous `std::io::{stdin, stdout, stderr}` are re-exported directly; async stdio requires blocking-pool or OS handle integration (Phase 4 / `akku::process`).
- **No external dependencies (`tokio`, `futures-io`, `futures-util`, `pin-project-lite`).** Pure `std` only.

## Namespace

`akku::io` — flat level-1 module, already listed in [`Namespace.md`](../../../../../Namespace.md) under *Async, system, CLI & development* (line 141) and *`std` name supersets* (line 222):

> `akku::io` | `std::io` superset with async `AsyncRead`, `AsyncWrite`, `AsyncBufRead` traits

Per `Namespace.md` §3 (`std` name supersets), every name `std::io` defines keeps `std::io`'s exact item and meaning (re-exported by explicit list, never by glob, never redefined or shadowed), while async traits/types take the `Async` prefix and async reader/writer operations live on `Async*Ext`.

Proposed Phase 1 public items under `akku::io`:
- Re-exported from `std::io` (explicit list): `BufRead`, `BufReader`, `BufWriter`, `Bytes`, `Chain`, `Cursor`, `Empty`, `Error`, `ErrorKind`, `IntoInnerError`, `IoSlice`, `IoSliceMut`, `LineWriter`, `Lines`, `Read`, `Repeat`, `Result`, `Seek`, `SeekFrom`, `Sink`, `Split`, `Stderr`, `StderrLock`, `Stdin`, `StdinLock`, `Stdout`, `StdoutLock`, `Take`, `Write`, `WriterPanicked`, `copy`, `empty`, `read_to_string`, `repeat`, `sink`, `stderr`, `stdin`, `stdout`.
  - `akku::io::copy` is the re-exported synchronous `std::io::copy`. Async stream copying is solely `AsyncReadExt::copy_to(&mut self, writer: &mut W)` (no `async_copy` free function).
- Async traits: `AsyncRead`, `AsyncWrite`, `AsyncBufRead`
- Extension traits (blanket-implemented): `AsyncReadExt`, `AsyncWriteExt`, `AsyncBufReadExt`
- Named `Future` types (so `Send` bounds work cleanly on Rust 1.85): `ReadFuture<'a, R>`, `ReadExactFuture<'a, R>`, `CopyToFuture<'a, R, W>`, `WriteFuture<'a, W>`, `WriteAllFuture<'a, W>`, `FlushFuture<'a, W>`, `ShutdownFuture<'a, W>`, `FillBufFuture<'a, R>` (named with `Future` suffix so they never collide with re-exported `std::io::Read` / `std::io::Write`!)

No name clashes: `io` is not a Rust keyword (`AGENTS.md` rule 6), does not shadow `::core`, and naming the future structs `*Future` prevents collisions with `std::io::Read` and `std::io::Write` in the same flat namespace (`Namespace.md` rule 2).

## Equivalents

| Ecosystem | Item |
|---|---|
| Go stdlib | `io.Reader` (`Read(p []byte) (n int, err error)`), `io.Writer`, `io.ReadFull`, `io.Copy`, `bufio.Reader` |
| Rust `std::io` | `Read`, `Write`, `BufRead`, `copy`, `Error`, `ErrorKind`, `Result`, `Cursor`, `Empty`, `Sink`, nightly-only `core::io::BorrowedBuf` |
| `futures-io` / `futures-util` | `futures_io::{AsyncRead, AsyncWrite, AsyncBufRead}` (`&mut [u8]`), `futures_util::io::{AsyncReadExt, AsyncWriteExt, AsyncBufReadExt, copy}` |
| `tokio::io` | `tokio::io::{AsyncRead, AsyncWrite, AsyncBufRead, ReadBuf}`, `AsyncReadExt`, `AsyncWriteExt`, `AsyncBufReadExt`, `copy` |
| `async-std::io` | `async_std::io::{Read, Write, BufRead, copy}` (re-exports `futures-io` traits) |

## Dependencies

`native`: zero external dependencies (`std` only, `#![deny(unsafe_code)]`, **0 `unsafe` blocks**).

- **Tier 1 (macOS, Windows, Linux, iOS, Android):** 100% pure Rust, identical behavior across all five targets.
- **Tier 2 (`wasm32`):** compiles cleanly with zero platform-specific code (`wasm32 = true` in `meta.toml`).
- **Module dependency direction:** `akku::io` depends only on `std` (it does **not** depend on `akku::task` or `akku::net`). `akku::net` depends on `akku::io` to implement `AsyncRead` and `AsyncWrite` for `AsyncTcpStream` and its halves.

## References

Pinned to the commits read on 2026-11-25.

- **`futures-io` & `futures-util` (`rust-lang/futures-rs`)** `rust-lang/futures-rs@92c78c2b985e9b713c658d4d65c84e2688788b87`
  - `futures-io/src/lib.rs:50-69`: `AsyncRead::poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<Result<usize>>`; specifies that implementations must translate `WouldBlock` to `Poll::Pending` and internally retry or map `Interrupted` (`:61-64`).
  - `futures-io/src/lib.rs:115-215`: `AsyncWrite` (`poll_write` at `:134`, `poll_flush` at `:195`, `poll_close` at `:212`).
  - `futures-io/src/lib.rs:256-303`: `AsyncBufRead: AsyncRead` (`poll_fill_buf` at `:285`, `consume` at `:302`) and delegation impls for `&[u8]` (`:546-559`).
  - `futures-util/src/io/mod.rs:137,378,401,409,588`: `AsyncReadExt`, `AsyncWriteExt`, `AsyncBufReadExt` blanket extension traits returning concrete `Future` types.
- **Tokio (`tokio-rs/tokio`)** `tokio-rs/tokio@854013a8cb7d1397e9d037315592dc1d2d55187d`
  - `tokio/src/io/read_buf.rs:23-40`: `ReadBuf<'a>` wraps `&'a mut [MaybeUninit<u8>]` and requires `unsafe { slice_to_uninit_mut(buf) }` (`:34`) even when wrapping an initialized `&mut [u8]`.
  - `tokio/src/io/async_read.rs:44-60`: `AsyncRead::poll_read` taking `&mut ReadBuf<'_>` and returning `Poll<io::Result<()>>`.
  - `tokio/src/io/async_write.rs:67,89-93,127`: `AsyncWrite::{poll_write, poll_flush, poll_shutdown}`; `poll_shutdown` implies flushing pending buffered data before shutting down the write half.
  - `tokio/src/io/async_buf_read.rs:45-63`: `AsyncBufRead::{poll_fill_buf, consume}` and deref delegation for `Box<T>`, `&mut T`, `Pin<P>` (`:65-85`).
  - `tokio/src/io/util/async_read_ext.rs:65,112,271` and `tokio/src/io/util/async_write_ext.rs:72,79,349,1286,1328`: ergonomic `read`, `read_exact`, `write`, `write_all`, `flush`, `shutdown` extension methods.
  - `tokio/src/io/util/copy.rs:9-81,138-162`: `CopyBuffer` allocates a single fixed-size `Box<[u8]>`, retries `ErrorKind::Interrupted` (`:44,69`), and flushes the writer when the buffer is drained and the reader returns `Poll::Pending` (`:141-158`) to prevent buffered request/response deadlocks.
- **Rust `core` / `std` (`rust-lang/rust`)** `rust-lang/rust@bc9132daa5c0be572ebb81304f6039d947298e67`
  - `library/core/src/io/borrowed_buf.rs:1,29-36,61-65,164,206`: `BorrowedBuf<'data, T>` and `BorrowedCursor<'a, T>` remain unstable nightly-only (`#![unstable(feature = "core_io_borrowed_buf", issue = "117693")]`), store `&'data mut [MaybeUninit<T>]`, and rely on internal `unsafe`.
  - `library/std/src/io/mod.rs:319`: `std::io` `DEFAULT_BUF_SIZE` (8 KiB) and standard sync I/O surface.
- **Go (`golang/go`)** `golang/go@d695060f5be23f2dd86d21d861a292dc2d183cab`
  - `src/io/io.go:86,99,353,387,407`: `io.Reader` (`Read(p []byte) (n int, err error)`), `io.Writer`, `io.ReadFull`, and `io.Copy`/`copyBuffer` using a single reusable buffer slice.

## Public API

Signatures and contracts for Phase 1 (no implementation). All public items live under `akku::io`.

### `std::io` re-exports

```rust
pub use std::io::{
    BufRead, BufReader, BufWriter, Bytes, Chain, Cursor, Empty, Error, ErrorKind,
    IntoInnerError, IoSlice, IoSliceMut, LineWriter, Lines, Read, Repeat, Result, Seek,
    SeekFrom, Sink, Split, Stderr, StderrLock, Stdin, StdinLock, Stdout, StdoutLock, Take,
    Write, WriterPanicked, copy, empty, read_to_string, repeat, sink, stderr, stdin, stdout,
};
```

### Core async traits

```rust
use std::pin::Pin;
use std::task::{Context, Poll};

/// Asynchronous byte reader (`&mut [u8]`, 100% safe, dyn-compatible).
pub trait AsyncRead {
    /// Attempts to read bytes into `buf`.
    ///
    /// - `Poll::Ready(Ok(0))` indicates EOF (unless `buf.is_empty()`).
    /// - `Poll::Ready(Ok(n))` (`n <= buf.len()`) indicates `n` bytes were read into `buf[..n]`.
    /// - `Poll::Pending` indicates no bytes are ready yet and `cx.waker()` was registered.
    ///
    /// Implementations must translate OS `WouldBlock` into `Poll::Pending` and should
    /// retry `ErrorKind::Interrupted` internally.
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<Result<usize>>;
}

/// Asynchronous byte writer (100% safe, dyn-compatible).
pub trait AsyncWrite {
    /// Attempts to write bytes from `buf`.
    ///
    /// - `Poll::Ready(Ok(n))` (`n <= buf.len()`) indicates `n` bytes were written.
    /// - `Poll::Pending` indicates the sink cannot accept bytes yet and `cx.waker()` was registered.
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize>>;

    /// Flushes buffered output, ensuring all intermediately buffered bytes reach the destination.
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>>;

    /// Gracefully shuts down the output stream (implying a flush of buffered bytes first).
    /// For full-duplex transports such as TCP, shuts down the write half (`Shutdown::Write`).
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>>;
}

/// Asynchronous buffered reader (`AsyncRead` with an internal byte buffer, dyn-compatible).
pub trait AsyncBufRead: AsyncRead {
    /// Returns the contents of the internal buffer, filling it from the underlying reader
    /// if empty. An empty slice (`&[]`) indicates EOF.
    fn poll_fill_buf(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<&[u8]>>;

    /// Marks `amt` bytes as consumed from the buffer returned by `poll_fill_buf`.
    /// Callers must pass `amt <= buf.len()`; implementations clamp or panic only on contract violation.
    fn consume(self: Pin<&mut Self>, amt: usize);
}
```

### Standard blanket & in-memory implementations

- Pointer delegation (zero `unsafe` via `Pin::new(&mut **self)` when `Unpin`, or `self.as_mut()` for `Pin<P>`):
  - `impl<T: AsyncRead + Unpin + ?Sized> AsyncRead for &mut T` and `Box<T>`
  - `impl<P: DerefMut + Unpin> AsyncRead for Pin<P> where P::Target: AsyncRead`
  - Equivalent delegation impls for `AsyncWrite` and `AsyncBufRead`.
- In-memory & trivial types (making generic protocol code testable without sockets):
  - `impl AsyncRead for &[u8]` and `impl AsyncBufRead for &[u8]`
  - `impl<T: AsRef<[u8]> + Unpin> AsyncRead for Cursor<T>` and `impl<T: AsRef<[u8]> + Unpin> AsyncBufRead for Cursor<T>`
  - `impl AsyncWrite for Vec<u8>`
  - `impl AsyncRead for Empty`, `impl AsyncBufRead for Empty`
  - `impl AsyncWrite for Sink`

### Extension traits

```rust
/// Extension methods for any [`AsyncRead`] value.
pub trait AsyncReadExt: AsyncRead {
    /// Reads up to `buf.len()` bytes into `buf`, returning `Ok(0)` on EOF (or if `buf` is empty).
    fn read<'a>(&'a mut self, buf: &'a mut [u8]) -> ReadFuture<'a, Self>
    where
        Self: Unpin;

    /// Reads the exact number of bytes required to fill `buf`.
    ///
    /// Retries `ErrorKind::Interrupted`. Returns `ErrorKind::UnexpectedEof` if EOF is reached
    /// before `buf` is full. Returns `Ok(())` immediately if `buf.is_empty()`.
    fn read_exact<'a>(&'a mut self, buf: &'a mut [u8]) -> ReadExactFuture<'a, Self>
    where
        Self: Unpin;

    /// Copies all bytes from `self` into `writer` using a fixed 8 KiB internal buffer,
    /// returning the total number of bytes copied (`Result<u64>`).
    ///
    /// Retries `ErrorKind::Interrupted`, flushes `writer` on EOF (and when the buffer drains
    /// while `self` returns `Poll::Pending`), and returns `ErrorKind::WriteZero` if `writer`
    /// returns `Ok(0)` with pending bytes. Does NOT call `shutdown` on `writer`.
    fn copy_to<'a, W>(&'a mut self, writer: &'a mut W) -> CopyToFuture<'a, Self, W>
    where
        Self: Unpin,
        W: AsyncWrite + Unpin + ?Sized;
}
impl<R: AsyncRead + ?Sized> AsyncReadExt for R {}

/// Extension methods for any [`AsyncWrite`] value.
pub trait AsyncWriteExt: AsyncWrite {
    /// Writes up to `buf.len()` bytes from `buf`, returning how many bytes were written.
    fn write<'a>(&'a mut self, buf: &'a [u8]) -> WriteFuture<'a, Self>
    where
        Self: Unpin;

    /// Writes the entire `buf`, retrying `ErrorKind::Interrupted` and short writes.
    ///
    /// Returns `ErrorKind::WriteZero` if `poll_write` returns `Ok(0)` while bytes remain.
    /// Returns `Ok(())` immediately if `buf.is_empty()`.
    fn write_all<'a>(&'a mut self, buf: &'a [u8]) -> WriteAllFuture<'a, Self>
    where
        Self: Unpin;

    /// Flushes this writer, ensuring all buffered data reaches the underlying sink.
    fn flush(&mut self) -> FlushFuture<'_, Self>
    where
        Self: Unpin;

    /// Gracefully shuts down the writer (flushing buffered data first).
    fn shutdown(&mut self) -> ShutdownFuture<'_, Self>
    where
        Self: Unpin;
}
impl<W: AsyncWrite + ?Sized> AsyncWriteExt for W {}

/// Extension methods for any [`AsyncBufRead`] value.
pub trait AsyncBufReadExt: AsyncBufRead {
    /// Awaits the internal buffer contents (filling from the underlying reader if empty).
    fn fill_buf(&mut self) -> FillBufFuture<'_, Self>
    where
        Self: Unpin;
}
impl<R: AsyncBufRead + ?Sized> AsyncBufReadExt for R {}
```

*(There is no `async_copy` free function: `akku::io::copy` is synchronous `std::io::copy`, and async copying is solely `AsyncReadExt::copy_to`.)*

### DoS limits & defensive contracts

- **Fixed `copy_to` buffer size:** `DEFAULT_COPY_BUF_SIZE = 8_192` bytes (8 KiB, matching `std::io::DEFAULT_BUF_SIZE`). Allocated once per `copy_to` call (`vec![0u8; 8_192].into_boxed_slice()`), never grows dynamically, so `copy_to` uses O(1) memory regardless of stream length.
- **Zero-length slice guard:** `read(&mut [])` / `write(&[])` return `Ok(0)` and `read_exact(&mut [])` / `write_all(&[])` return `Ok(())` immediately without polling the underlying stream (prevents mistaking 0-byte requests for EOF or `WriteZero`).
- **Buggy `n > buf.len()` guard:** `read_exact`, `write_all`, and `copy_to` validate `n <= slice.len()` on the count returned by `poll_read`/`poll_write`; if a broken `AsyncRead`/`AsyncWrite` implementation reports `n > slice.len()`, they return `Err(Error::new(ErrorKind::InvalidData, ...))` instead of panicking on out-of-bounds slice indexing.
- **`WriteZero` loop guard:** `write_all` and `copy_to` treat `Ok(0)` on a non-empty write slice as `Err(ErrorKind::WriteZero)` rather than spinning forever.
- **Integer overflow guard in `copy_to`:** total copied byte count uses `saturating_add` (or `u64` accumulation) without panicking on debug overflow.
- **`Interrupted` (`EINTR`) retry:** `read_exact`, `write_all`, and `copy_to` automatically retry `ErrorKind::Interrupted`.

## API rationale

### 1. Buffer model: `&mut [u8]` (chosen) vs `tokio::io::ReadBuf` vs `std::io::BorrowedBuf`

| Criterion | `&mut [u8]` (`futures-io` / `std::io::Read`) **(Chosen)** | `tokio::io::ReadBuf<'_>` | `std::io::BorrowedBuf<'_>` |
|---|---|---|---|
| `unsafe` required | **0 `unsafe`** | Requires `unsafe` (`MaybeUninit<u8>` slice cast in `ReadBuf::new`, `assume_init`, `tokio/src/io/read_buf.rs:34`) | Uses internal `unsafe` (`library/core/src/io/borrowed_buf.rs:64`) |
| Rust 1.85 MSRV stable | **Yes** | Yes (if implemented in crate, violating `AGENTS.md` rule 9) | **No** — nightly-only `#![unstable(feature = "core_io_borrowed_buf", issue = "117693")]` (`borrowed_buf.rs:1`) |
| Policy fit (`AGENTS.md` rule 9) | **Exact fit** (no `sys/` needed; speed-only `unsafe` is forbidden) | **Violates rule 9** (uninitialized read buffers are a speed-only optimization) | Unavailable on stable |
| Ergonomics & interop | Matches `std::io::Read::read` and `AsyncTcpStream::try_read` 1:1; zero wrapper boilerplate | Callers must wrap `&mut [u8]` in `ReadBuf::new`, inspect `.filled().len()` | Requires `BorrowedBuf` + `BorrowedCursor` double indirection |
| Steady-state throughput | **0% overhead** in real servers: HTTP/NFS/TLS parsers allocate one zeroed `Vec<u8>` per connection and reuse it across every read (`docs/forks/tokio-axum.md` §5B) | Avoids one-time zero-fill of fresh stack/heap buffers | Avoids one-time zero-fill of fresh buffers |

**Decision:** **`&mut [u8]`** is the simplest 100%-safe buffer model, aligns with `std::io::Read` and `futures-io` (`futures-io/src/lib.rs:65-69`), complies strictly with `#![deny(unsafe_code)]`, and incurs zero steady-state cost on reused connection buffers.

### 2. `poll_shutdown` vs `poll_close`

`AsyncWrite` names the third method `poll_shutdown` (following Tokio `tokio/src/io/async_write.rs:127`) rather than `futures-io`'s `poll_close` (`futures-io/src/lib.rs:212`). On full-duplex transports like `akku::net::AsyncTcpStream`, shutting down the write half sends TCP `FIN` while the read half remains open for the peer's response; `shutdown` accurately describes half-close whereas `close` misleads callers into expecting full socket teardown.

### 3. Extension traits with concrete `Future` types (instead of `async fn` in traits)

- Keeping `AsyncRead`, `AsyncWrite`, and `AsyncBufRead` strictly poll-based makes all three traits **100% dyn-compatible** (`Pin<&mut dyn AsyncRead>`, `Box<dyn AsyncWrite + Unpin>`).
- Providing `read`, `read_exact`, `copy_to`, `write`, `write_all`, `flush`, `shutdown`, and `fill_buf` on blanket extension traits (`AsyncReadExt`, `AsyncWriteExt`, `AsyncBufReadExt`) returning named `Future` structs (`ReadFuture`, `CopyToFuture`, etc.) guarantees that every future is automatically `Send` whenever the underlying `R`/`W` is `Send` — avoiding Rust 1.85 RPITIT `Send`-bound friction in `akku::task::spawn`.
- Future struct names use the `*Future` suffix (`ReadFuture`, `WriteFuture`, etc.) so they never clash with `std::io::Read` and `std::io::Write` re-exported in the same `akku::io` module.

### 4. `akku::io::copy` stays sync; async copy is `AsyncReadExt::copy_to`

Per the `std`-superset naming rule (`Namespace.md` §3), `akku::io::copy` is the re-exported synchronous `std::io::copy` — never redefined or shadowed — and async reader/writer operations live on the matching `Async*Ext` trait rather than introducing an `async_copy` free function. Async stream copying is therefore exclusively `AsyncReadExt::copy_to(&mut self, writer: &mut W) -> CopyToFuture<'_, Self, W>` (returning `Result<u64>`). Flushing `writer` when the internal buffer drains and `self` returns `Poll::Pending` (matching `tokio/src/io/util/copy.rs:141-158`) prevents subtle request/response deadlocks when copying between buffered streams.

### 5. `E0034` method-resolution hazard on dual-impl types

In-memory types (`&[u8]`, `Cursor<T>`, `Empty`, `Vec<u8>`, `Sink`) implement both the synchronous `std::io` traits (`Read`, `BufRead`, `Write`) and akku's async traits (`AsyncRead`, `AsyncBufRead`, `AsyncWrite`). If a module brings both a sync trait and its async extension trait into scope at once (e.g. `use akku::io::{Read, AsyncReadExt};` or a glob `use akku::io::*;`), calling `x.read(&mut buf)` on `&[u8]`, `Cursor`, or `Empty` — or `x.write(buf)` / `x.flush()` on `Vec<u8>` or `Sink` — fails to compile with Rust `E0034` ("multiple applicable items in scope").

Mitigation and guidance (documented in `akku::io`'s module rustdoc):
- Import `AsyncReadExt`, `AsyncWriteExt`, and `AsyncBufReadExt` explicitly only where async I/O is used (never glob-import `akku::io::*`, and never re-export `Async*Ext` in `akku::prelude`).
- When a single scope genuinely needs both sync and async calls on a dual-impl type, disambiguate explicitly via UFCS (`AsyncReadExt::read(&mut src, &mut buf).await`, `Read::read(&mut src, &mut buf)`).
- Native async socket types such as `akku::net::AsyncTcpStream` implement only `AsyncRead`/`AsyncWrite` (not sync `Read`/`Write`), so they never trigger `E0034`.

## Seams

Public boundaries targeted by `crates/akku/src/io/tests.rs` (G3):

| Seam | Catches | Misses |
|---|---|---|
| `std::io` re-exports (`copy`, `Error`, `ErrorKind`, `Result`, `Read`, `Write`, `BufRead`, `Cursor`, `Empty`, `Sink`, etc.) | missing re-export, accidentally async `copy`, name collision with `*Future` or `Async*` | — |
| `AsyncRead` + `AsyncReadExt::{read, read_exact}` on `&[u8]`, `Cursor`, `Empty`, custom scripted pollers | partial-read reassembly bugs, `UnexpectedEof`, `Interrupted` retry, zero-length guard, `n > len` panic | OS socket readiness (covered by `net/tests.rs`) |
| `AsyncWrite` + `AsyncWriteExt::{write, write_all, flush, shutdown}` on `Vec<u8>`, `Sink`, custom scripted pollers | short-write loop bugs, `WriteZero` infinite loop, `Interrupted` retry, missed flush/shutdown propagation | — |
| `AsyncBufRead` + `AsyncBufReadExt::fill_buf` + `consume` on `&[u8]`, `Cursor`, `Empty` | stale slice after `consume`, EOF empty-slice handling | `AsyncBufReader` (deferred to TODO) |
| `AsyncReadExt::copy_to` across in-memory streams, chunked/pending mock streams, and `akku::net::AsyncTcpStream` | lost tail bytes, missed flush on `Pending`, `WriteZero` hang, large multi-buffer (>8 KiB) copy bugs | — |
| `E0034` dual-impl disambiguation (`&[u8]`, `Cursor`, `Vec<u8>`, `Empty` with both sync `Read`/`Write`/`BufRead` and `Async*Ext` in scope) | broken UFCS disambiguation (`Read::read` vs `AsyncReadExt::read`, `Write::write` vs `AsyncWriteExt::write`), missing rustdoc import guidance | compile-fail `E0034` harness (not possible without external `trybuild` dep; covered by documented rustdoc note + explicit-disambiguation test) |
| Trait object (`&mut dyn AsyncRead`, `Box<dyn AsyncWrite + Unpin>`) and `Send` future bounds inside `akku::task::spawn` | accidental loss of dyn-compatibility or `!Send` futures | — |

### Adversarial test list for G3

1. **Short reads & writes (1-byte trickle + `Pending` interleaving):**
   - Scripted `AsyncRead` that yields `Poll::Pending` (after scheduling a wake) before every 1-byte chunk; verify `read_exact` reassembles a 4 KiB payload accurately and `copy_to` transfers >32 KiB (crossing multiple 8 KiB buffer fills) with exact byte count and content.
   - Scripted `AsyncWrite` that accepts 1–7 bytes per poll with `Poll::Pending` in between; verify `write_all` and `copy_to` deliver every byte in order without data loss or duplication.
2. **EOF, `UnexpectedEof`, and `WriteZero`:**
   - `read_exact` when the reader hits EOF after 3 of 10 bytes returns `Err(e)` with `e.kind() == ErrorKind::UnexpectedEof`.
   - `write_all` and `copy_to` against a writer returning `Ok(0)` on non-empty input return `Err(e)` with `e.kind() == ErrorKind::WriteZero` immediately (no infinite loop).
3. **`ErrorKind::Interrupted` (`EINTR`) storms:**
   - Scripted reader/writer injecting `ErrorKind::Interrupted` before every valid chunk; verify `read_exact`, `write_all`, and `copy_to` retry cleanly and succeed.
4. **Zero-length buffers:**
   - `read(&mut [])` and `write(&[])` return `Ok(0)` immediately; `read_exact(&mut [])` and `write_all(&[])` return `Ok(())` immediately even on an already-EOF reader or `WriteZero` writer.
5. **Buggy `n > buf.len()` implementor defense:**
   - Malicious/buggy `AsyncRead` and `AsyncWrite` returning `Ok(buf.len() + 1)`; verify `read_exact`, `write_all`, and `copy_to` return `Err(ErrorKind::InvalidData)` instead of panicking on slice indexing.
6. **Buffered-writer deadlock prevention in `copy_to`:**
   - Writer buffers bytes until `poll_flush`, and reader blocks (`Pending`) waiting for the flushed bytes to arrive before sending more; verify `copy_to` flushes when its buffer drains and `self` returns `Poll::Pending`, completing without deadlock.
7. **Drop-cancellation:**
   - Drop `read_exact`, `write_all`, and `copy_to` futures mid-transfer after partial progress; verify no panic, no resource leak, and subsequent operations on the underlying stream continue cleanly.
8. **Sync `akku::io::copy` & `E0034` explicit-disambiguation test (plus rustdoc note):**
   - Verify `akku::io::copy(&mut src, &mut dst)` executes synchronously as `std::io::copy` outside any async runtime.
   - Import both `akku::io::{Read, Write, BufRead}` and `akku::io::{AsyncReadExt, AsyncWriteExt, AsyncBufReadExt}` in the same test scope and exercise explicit UFCS disambiguation (`Read::read` / `AsyncReadExt::read`, `Write::write` / `AsyncWriteExt::write`, `BufRead::fill_buf` / `AsyncBufReadExt::fill_buf`) on `&[u8]`, `Cursor`, `Vec<u8>`, and `Empty`.
   - Include a documented rustdoc note (and doctest) on `akku::io` warning about the `E0034` ambiguity and advising explicit `Async*Ext` imports (a compile-fail test is omitted to preserve zero dependencies).
9. **End-to-end `akku::net::AsyncTcpStream` integration:**
   - Run `read_half.copy_to(&mut write_half).await` as a full-duplex TCP echo handler, exercising `AsyncTcpStream` `AsyncRead`/`AsyncWrite` impls, `write_all`, `shutdown` (half-close `FIN`), and `read_exact` across all Tier-1 OS CI runners (Linux, macOS, Windows).

## Decisions (human, 2026-11-25)

1. **Bundle `akku::io` into Phase 1:** flat `std::io` superset module with `AsyncRead`, `AsyncWrite`, `AsyncBufRead`, and extension methods (`read`, `read_exact`, `copy_to`, `write`, `write_all`, `flush`, `shutdown`, `fill_buf`), implemented by `akku::net::AsyncTcpStream` (and its split halves).
2. **Buffer model:** `&mut [u8]` (100% safe, zero `unsafe`, Rust 1.85 stable, matching `futures-io` and `std::io::Read`).
3. **`std`-superset naming rule (`Namespace.md` §3):** `akku::io::copy` is the re-exported synchronous `std::io::copy` (never redefined or shadowed); async copying is solely `AsyncReadExt::copy_to(&mut self, writer: &mut W)` returning `Result<u64>` (no `async_copy` free function). Document the `E0034` dual-impl method-resolution hazard on `&[u8]`/`Cursor`/`Vec<u8>`/`Empty` in rustdoc (advising explicit `Async*Ext` imports) and cover it with an explicit-disambiguation G3 test.
