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

## Phase 2 (G2 designed: `DESIGN.md` §Phase 2 G2: API)

The Phase 2 items above are designed in `DESIGN.md` §Phase 2 G2 and decided in §Phase 2 Decisions (G2) (Orcha + independent Opus review, 2026-11-26). They are `AsyncUdpSocket`, `AsyncUnix*` (streams also on Windows 10 1803+ via `afunix.sys`, decision Q2), `SocketOpts`, and `AsyncTcpStream::peek`. `AsyncTcpBuilder` is replaced by `SocketOpts` + `bind_with`/`connect_with` (Q1). Vectored I/O and `reunite` stay open.

### In-PR gate obligations (not deferred)
- G3 step 1: `WSAPoll` + AF_UNIX probe on `windows-latest`. On failure, stop and report to Orcha; the human decides an exception. (origin: decision Q2)
- G4 step 1: consolidate the Phase 1 `connect_nonblocking`/`SockAddrIn{,6}`/`WSASocketW`/`SetHandleInformation` into `sys/sock_{unix,win}.rs`, with Phase 1 green before any Phase 2 code. (origin: decision Q10)
- Phase 1 follow-up within this PR: `SO_NOSIGPIPE` on accepted and `from_std` TCP streams on Darwin, and the child-process SIGPIPE probe in place of the current "never SIGPIPE" check. (origin: decision Q7)

### Phase 2c (future)
- `AsyncUnixStream::peer_cred`: Linux `SO_PEERCRED`, Darwin `getpeereid` + `LOCAL_PEERPID`, Windows `SIO_AF_UNIX_GETPEERPID`. This is the first item. (origin: decision Q5)
- Ancillary data (`SCM_RIGHTS`, `SCM_CREDENTIALS`), Unix only. (origin: decision Q5)

### Deferred from Phase 2
- `peek` / `peek_from` on `AsyncUnixStream` / `AsyncUnixDatagram` via `recv`/`recvfrom` with `MSG_PEEK` (std `unix_socket_peek` is unstable in 1.85). (origin: open question 6)
- Truncation reporting for datagrams (`recvmsg` + `MSG_TRUNC`), plus `recvmmsg`/`sendmmsg`, GSO/GRO, `IP_PKTINFO`. (origin: open question 9)
- `bind_with` for Unix sockets (buffer sizes, backlog) and Linux autobind. (origin: `DESIGN.md` §P2.1)
- Bind-before-connect (local source address in `SocketOpts` for `connect_with`). (origin: `DESIGN.md` §P2.1)
- `IP_TOS` / `IPV6_TCLASS`, `SO_BINDTODEVICE`, source-specific multicast (`IP_ADD_SOURCE_MEMBERSHIP`), UDP disconnect (`AF_UNSPEC`), optional Windows `SO_EXCLUSIVEADDRUSE`. (origin: socket2 0.6.5 / tokio 1.54.1 surface)
- Unix connect retry with a timeout when the listener backlog is full (needs Phase 3 `akku::time`). (origin: decision Q14)
- Abstract AF_UNIX names on Windows (`afunix.sys` accepts them; kept Linux/Android only by decision). (origin: decision Q2)
- Rate-limited `akku::log` line when an accepted socket is dropped because re-applying an option failed. (origin: missing item a)
