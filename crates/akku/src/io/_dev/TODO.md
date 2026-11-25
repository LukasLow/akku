# io — backlog

## Buffered async adapters
- `AsyncBufReader<R>` — buffered wrapper implementing `AsyncBufRead` over any `AsyncRead` with configurable fixed capacity (default 8 KiB). (origin: `tokio::io::BufReader`, `futures_util::io::BufReader`)
- `AsyncBufWriter<W>` — buffered wrapper implementing `AsyncWrite` over any `AsyncWrite` with flush-before-shutdown semantics. (origin: `tokio::io::BufWriter`, `futures_util::io::BufWriter`)

## Bounded collection readers (with mandatory DoS byte caps)
- `AsyncReadExt::read_to_end_limit(&mut self, buf: &mut Vec<u8>, max_bytes: usize)` and `read_to_string_limit` — bounded readers preventing OOM bombs from untrusted peers. (origin: `DESIGN.md` §Non-goals / `tokio::io::AsyncReadExt::read_to_end`)
- `AsyncBufReadExt::{read_until_limit, read_line_limit}` — delimiter/line readers with explicit line-length cap for HTTP/text protocols. (origin: `tokio::io::AsyncBufReadExt::read_line`)
- `Take<R>` / `Chain<R1, R2>` async combinators (`AsyncReadExt::{take, chain}`). (origin: `tokio::io::AsyncReadExt::{take, chain}`)

## Stream copy & framing helpers
- `AsyncBufReadExt::copy_buf_to(&mut self, writer: &mut W)` — zero-extra-buffer copy from an `AsyncBufRead` source into an `AsyncWrite` sink (follows `Namespace.md` §3(b) alongside `AsyncReadExt::copy_to`). (origin: `tokio::io::copy_buf`)
- `copy_bidirectional<A, B>` — concurrent bidirectional relay with half-close propagation for proxies and tunnels. (origin: `tokio::io::copy_bidirectional`)
- `poll_read_vectored` / `poll_write_vectored` (default methods on `AsyncRead`/`AsyncWrite` using `IoSliceMut`/`IoSlice`) — scatter-gather framing for `akku::http` and `akku::nfs`. (origin: `futures-io/src/lib.rs:93,163`, `tokio/src/io/async_write.rs:129-145`)

## Async seeking
- `AsyncSeek` (`start_seek`, `poll_complete`) — if/when async file I/O in `akku::fs` needs seeking beyond `spawn_blocking`. (origin: `tokio::io::AsyncSeek`, `futures-io/src/lib.rs:222-250`)
