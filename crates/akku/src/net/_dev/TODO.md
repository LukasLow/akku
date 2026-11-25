# net — backlog

## Reactor performance & portability follow-ups
- `sys/iocp.rs` + `sys/afd.rs` — Windows high-scale readiness follow-up (`IOCP` + `AFD`) once benchmarked against `WSAPoll` past ~1k concurrent sockets. (origin: `docs/forks/tokio-axum.md` §5A, §9.2)
- `pipe2` waker fallback in `sys/epoll.rs` — fallback when container seccomp profiles block `eventfd2` (e.g. Gramine). (origin: `DESIGN.md` §References / `smol-rs/polling` `src/epoll.rs:419-478`)
- Non-Tier-1 BSD `kqueue` support (`FreeBSD`, `OpenBSD`, `NetBSD`, `DragonFly`) — gate divergent `kevent` field types (`Filter`/`Flags`/`Count`) per `mio` `src/sys/unix/selector/kqueue.rs:19-50`. (origin: `docs/forks/tokio-axum.md` §6)

## Additional socket types & options (Phase 2)
- `AsyncUdpSocket` — async UDP socket (`bind`, `connect`, `send`, `recv`, `send_to`, `recv_from`, multicast/broadcast options; blocking `UdpSocket` stays re-exported from `std::net`). (origin: `Namespace.md` §3 / `docs/forks/tokio-axum.md` §8 Phase 2)
- `AsyncUnixListener`, `AsyncUnixStream` (with `AsyncUnixReadHalf`/`AsyncUnixWriteHalf`/`AsyncUnixOwnedReadHalf`/`AsyncUnixOwnedWriteHalf`), `AsyncUnixDatagram` (`#[cfg(unix)]`) — async Unix domain sockets. (origin: `Namespace.md` §3 / `docs/forks/tokio-axum.md` §8 Phase 2)
- `AsyncTcpBuilder` / `SocketOpts` (`sys/sock_unix.rs`, `sys/sock_win.rs`) — pre-bind socket options (`SO_REUSEADDR`, `SO_REUSEPORT`, `TCP_KEEPALIVE`/`TCP_KEEPIDLE`/`TCP_KEEPINTVL`/`TCP_KEEPCNT`, `IPV6_V6ONLY`, `SO_NOSIGPIPE`, listen backlog, send/recv buffer sizes). (origin: `docs/forks/tokio-axum.md` §3, §5A, §6, §8 Phase 2)
- `AsyncTcpStream::peek` / `poll_peek` and vectored I/O (`write_vectored` / `poll_write_vectored`) — zero-copy header+body framing for `akku::http` and `akku::nfs`. (origin: `tokio::net::TcpStream::peek`, `tokio/src/net/tcp/stream.rs:1510-1520`)
- `AsyncTcpOwnedReadHalf::reunite(AsyncTcpOwnedWriteHalf)` — reassemble a split `AsyncTcpStream` safely without closing. (origin: `tokio::net::tcp::OwnedReadHalf::reunite`)

## DNS & higher-level stream helpers
- Non-blocking hostname resolution in `AsyncTcpListener::bind` / `AsyncTcpStream::connect` — offload `ToSocketAddrs` onto `akku::task::spawn_blocking` (Phase 4) and later native `akku::dns` (Wave 2). (origin: `DESIGN.md` §Non-goals / `docs/forks/tokio-axum.md` §8 Phase 4)
- `AsyncTcpStream::ready(Interest)` / `readable()` / `writable()` — explicit readiness waiting API for custom zero-copy protocol codecs. (origin: `tokio::net::TcpStream::ready`, `smol-rs/async-io` `Async::readable`/`writable`)
- `AsyncTcpListener::incoming()` — async stream adapter once `akku::task` / `akku::io` defines a minimal `Stream` trait. (origin: `smol-rs/async-net` `src/tcp.rs:169-173`)

## G4 implementation cleanup
- Remove `no_run` from all rustdoc examples in `src/net/mod.rs` so `cargo test -p akku --doc net` runs every example. (origin: G3 stub scaffold)
- Remove `#![allow(dead_code, unused_variables)]` from `src/net/mod.rs` and `src/net/sys/{mod,epoll,kqueue,wsapoll}.rs` once stub bodies are replaced with the real implementation, plus the `#[allow(clippy::ptr_arg)]` on each backend's `Poller::wait`. (origin: G3 stub scaffold)
- Fill the empty G3 `Drop` stubs for `AsyncTcpListener` / `AsyncTcpStream` (deregister before close) and `AsyncTcpOwnedWriteHalf` (`shutdown(Shutdown::Write)`, errors ignored); `into_std` / `into_split` must then move fields out of a `Drop` type (e.g. `Option` fields). (origin: G3 stub scaffold)
- Add the non-Tier-1 `sys` fallback (safe, no allowlist entry) returning `io::ErrorKind::Unsupported`; G3 compiles the three backends only behind their DESIGN.md target gates. (origin: DESIGN.md §Non-goals / G3 scaffold)
- Android (`aarch64-linux-android`, `armv7-linux-androideabi`, `x86_64-linux-android`) and iOS (`aarch64-apple-ios`, `aarch64-apple-ios-sim`, `x86_64-apple-ios`) are not cross-checked yet; the `epoll.rs` / `kqueue.rs` layout asserts are verified only on Linux aarch64 (smd on an Apple-silicon host; CI `ubuntu-latest` adds x86_64), macOS aarch64, and Windows x86_64 (`cargo check`) so far. (origin: DESIGN.md §Tier-1 CI & mobile cross-check strategy)
