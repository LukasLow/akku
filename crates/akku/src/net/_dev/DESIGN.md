# net — design record

## Purpose

`akku::net` is akku's `std::net` superset providing IP/socket address types and zero-dependency async networking on top of `akku::task` and `akku::io`. Today `akku` has no native async socket layer: `akku::http::server` depends on `tokio::net`, `akku::http::client` is synchronous over `std::net`, and Wave 2 protocols (`akku::dns`, `akku::nfs`, `akku::quic`, and the native `akku::http` rewrite) all require native async sockets (`docs/forks/tokio-axum.md` §3, §8).

**Phase 1 scope (human decisions, 2026-11-25; `AGENTS.md` rule 11):**
1. Re-export the full blocking `std::net` surface by explicit list (`AddrParseError`, `Incoming`, `IpAddr`, `Ipv4Addr`, `Ipv6Addr`, `Shutdown`, `SocketAddr`, `SocketAddrV4`, `SocketAddrV6`, `TcpListener`, `TcpStream`, `ToSocketAddrs`, `UdpSocket`), per `Namespace.md` §3.
2. Ship native OS readiness reactors for **all five Tier-1 platforms** in the same PR (no Tier-1 `Unsupported` stubs):
   - `crates/akku/src/net/sys/epoll.rs` — Linux and Android (`epoll` + `eventfd`);
   - `crates/akku/src/net/sys/kqueue.rs` — macOS and iOS (`kqueue` + `EVFILT_USER`);
   - `crates/akku/src/net/sys/wsapoll.rs` — Windows (`WSAPoll` + loopback TCP wake socketpair; `IOCP`+`AFD` stays a later benchmarked performance follow-up).
   These three `sys/` files are the **only** files allowed to contain `unsafe` in Phase 1 (allowlisted in `.github/unsafe-allowlist.txt`, `// SAFETY:` on every block).
3. Ship `AsyncTcpListener` and `AsyncTcpStream` (plus borrowed `AsyncTcpReadHalf`/`AsyncTcpWriteHalf` and owned `AsyncTcpOwnedReadHalf`/`AsyncTcpOwnedWriteHalf` splits) wrapping `std::net::{TcpListener, TcpStream}` and implementing `akku::io::{AsyncRead, AsyncWrite}` directly.

## Non-goals

- **No Windows `IOCP`+`AFD` or non-Tier-1 BSD (`FreeBSD`/`OpenBSD`/`NetBSD`) tuning in Phase 1.** Windows ships native `WSAPoll` in Phase 1; `IOCP`+`AFD` (`net/sys/iocp.rs`, `net/sys/afd.rs`) is tracked in `_dev/TODO.md` as a benchmarked performance follow-up for >1k concurrent sockets (`docs/forks/tokio-axum.md` §9.2). Only non-Tier-1 / exotic OS targets without a native backend compile through the fallback returning `io::ErrorKind::Unsupported`.
- **No `AsyncUdpSocket`, async Unix domain sockets, or custom pre-bind `SocketOpts`/`AsyncTcpBuilder` in Phase 1.** Deferred to Phase 2 (`_dev/TODO.md`, `docs/forks/tokio-axum.md` §8). (Blocking `std::net::UdpSocket` is re-exported in Phase 1 as part of the `std::net` superset.) Phase 1 exposes only the socket options `std::net` already provides safely (`nodelay`, `ttl`, `take_error`, `local_addr`, `peer_addr`, `shutdown`).
- **No `libc`, `windows-sys`, `mio`, or `socket2` external crate dependencies.** Minimal hand-written FFI declarations across `net/sys/{epoll,kqueue,wsapoll}.rs` (~550–650 LOC total) avoid ~450k SLOC of `libc` + `windows-sys` and stay well within crates.io's 10 MiB package limit (`docs/forks/tokio-axum.md` §6).
- **No Linux `io_uring`.** Deferred indefinitely: readiness-based `epoll` matches Rust's borrow-based `Future` model cleanly, whereas completion-based `io_uring` requires owned buffers to avoid drop-cancellation UAF and is blocked by default in many container seccomp profiles (`docs/forks/tokio-axum.md` §9.4).
- **No async DNS resolution in Phase 1.** `ToSocketAddrs::to_socket_addrs` runs synchronously via `std::net` (zero network I/O for IP literals like `"127.0.0.1:0"` or `SocketAddr`; synchronous `getaddrinfo` for hostnames until Phase 4 `spawn_blocking` and Wave 2 `akku::dns`).
- **No `wasm32` async TCP sockets.** Per `CONCEPT.md` §6 and `AGENTS.md` rule 11 (Tier 2), native async socket types (`AsyncTcpListener`, `AsyncTcpStream`, splits) are `#[cfg(not(target_arch = "wasm32"))]`, while the explicit `std::net` re-exports stay available on all targets including `wasm32`.
- **No TLS in `akku::net`.** TLS belongs strictly to `akku::tls` (`AGENTS.md` rule 10).

## Namespace

`akku::net` — flat level-1 module, already listed in [`Namespace.md`](../../../../../Namespace.md) under *Network, transport & storage protocols*:

> `akku::net` | `std::net` superset: IP addresses, async TCP/UDP/Unix sockets on the akku runtime, socket options (Wave 2)

Networking is deliberately flat (`akku::net`, `akku::dns`, `akku::quic` — `Namespace.md` lines 60–61). Per `Namespace.md` §3 (`std` name supersets), `akku::net` is a full `std::net` superset: every name `std::net` defines keeps its exact blocking `std::net` item and meaning (re-exported by explicit list, never glob, never redefined or shadowed), while async socket types use an `Async` prefix.

Proposed Phase 1 public items under `akku::net`:
- Re-exported from `std::net` by explicit list (all targets including `wasm32`): `AddrParseError`, `Incoming`, `IpAddr`, `Ipv4Addr`, `Ipv6Addr`, `Shutdown`, `SocketAddr`, `SocketAddrV4`, `SocketAddrV6`, `TcpListener`, `TcpStream`, `ToSocketAddrs`, `UdpSocket`
- Native async TCP (`#[cfg(not(target_arch = "wasm32"))]`): `AsyncTcpListener`, `AsyncTcpStream`, `AsyncTcpReadHalf<'a>`, `AsyncTcpWriteHalf<'a>`, `AsyncTcpOwnedReadHalf`, `AsyncTcpOwnedWriteHalf`

Blocking `TcpListener`, `TcpStream`, and `UdpSocket` are re-exported directly from `std::net`; interop with `AsyncTcpListener` / `AsyncTcpStream` goes through `from_std` / `into_std`. Phase 2 adds `AsyncUdpSocket` and `AsyncUnix{Listener,Stream,Datagram}` under the same `Async` prefix.

## Equivalents

| Ecosystem | Item |
|---|---|
| Go `net` + `runtime` | `net.Listen` / `net.ListenTCP` (`TCPListener`), `net.Dial` / `net.DialTCP` (`TCPConn` with `CloseRead`/`CloseWrite`), `internal/poll.FD`, `runtime/netpoll_epoll.go` (`epoll` + `eventfd`), `runtime/netpoll_kqueue.go` + `netpoll_kqueue_event.go` (`kqueue` + `EVFILT_USER`) |
| Rust `std::net` | `IpAddr`, `Ipv4Addr`, `Ipv6Addr`, `SocketAddr`, `Shutdown`, `ToSocketAddrs`, blocking `TcpListener`, `TcpStream` (wrapped internally by `akku::net` with `set_nonblocking(true)`) |
| `tokio::net` | `tokio::net::{TcpListener, TcpStream}`, `tcp::{ReadHalf, WriteHalf, OwnedReadHalf, OwnedWriteHalf}`, `runtime::io::{Driver, Registration, RegistrationSet}` |
| `async-std::net` | `async_std::net::{TcpListener, TcpStream}` + `std::net` address re-exports |
| `smol` (`async-net` + `async-io`) | `async_net::{TcpListener, TcpStream}` over `async_io::Async<std::net::*>` + `async_io::Reactor` (`Slab<Arc<Source>>`) |
| `mio` | `mio::{Poll, Registry, Events, Interest, Token, Waker}`, `mio::net::{TcpListener, TcpStream}`, `sys/unix/selector/{epoll,kqueue}.rs`, `sys/unix/waker/{eventfd,selector}.rs` |
| `polling` | `polling::{Poller, Events, Event, PollMode}`, `src/epoll.rs`, `src/kqueue.rs` (`EVFILT_USER` notify) |
| `libcurl` | `lib/socketpair.c` (Windows loopback TCP wake socketpair with nonce verification), `lib/select.c` (`WSAPoll` event flag mapping) |

## Dependencies

`native`: zero external dependencies (`std` only + `akku::task` + `akku::io`).

- **All code outside `crates/akku/src/net/sys/{epoll,kqueue,wsapoll}.rs` is 100% safe (`#![deny(unsafe_code)]`).**
- Why no safe path exists in `sys/`: `std` exposes neither an OS readiness multiplexer (`epoll`, `kqueue`, `WSAPoll`), nor an OS cross-thread reactor wakeup (`eventfd`, `EVFILT_USER`), nor a non-blocking TCP `connect()` before handshake completion (`std::net::TcpStream::connect` blocks the calling thread).
- All established socket I/O (`bind`, `accept`, `read`, `write`, `shutdown`, `local_addr`, `peer_addr`, `set_nonblocking`, `set_nodelay`, `set_ttl`, `take_error`) delegates directly to safe `std::net` methods.

### `.github/unsafe-allowlist.txt` entries for Phase 1 (3 files, ~31–36 `unsafe` blocks total)

| Allowlist path | Tier-1 targets | Backend & wakeup | FFI symbols | Est. `unsafe` blocks |
|---|---|---|---:|---:|
| `crates/akku/src/net/sys/epoll.rs` | Linux, Android | `epoll` (`EPOLLET`) + `eventfd` | 6 | ~11–12 |
| `crates/akku/src/net/sys/kqueue.rs` | macOS, iOS | `kqueue` (`EV_CLEAR`) + `EVFILT_USER` | 6 | ~12–14 |
| `crates/akku/src/net/sys/wsapoll.rs` | Windows | `WSAPoll` (armed level-triggered) + loopback TCP pair | 5 | ~8–10 |
| **Total (Phase 1)** | **All 5 Tier-1 OSes** | **0 `unsafe impl Send/Sync`, 0 `Vec::set_len`, 0 `MaybeUninit`** | **17** | **~31–36** |

---

### Backend 1: Linux & Android (`crates/akku/src/net/sys/epoll.rs`)

Target gate: `#[cfg(all(any(target_os = "linux", target_os = "android"), any(target_arch = "x86_64", target_arch = "aarch64", target_arch = "x86", target_arch = "arm", target_arch = "riscv64")))]`. Exotic Linux architectures (e.g. `mips`, `sparc`, `powerpc`, where `O_NONBLOCK`/`SOCK_STREAM`/`EINPROGRESS` diverge) fall through to the non-Tier-1 `Unsupported` stub rather than silently miscompiling (`docs/forks/tokio-axum.md` §6).

#### FFI symbols (`extern "C"` — 6 symbols)

| Symbol | Signature | Why needed (no safe `std` equivalent) |
|---|---|---|
| `epoll_create1` | `fn(flags: c_int) -> c_int` | Create `EPOLL_CLOEXEC` epoll instance |
| `epoll_ctl` | `fn(epfd: c_int, op: c_int, fd: c_int, event: *mut EpollEvent) -> c_int` | Register (`ADD`), update (`MOD`), or remove (`DEL`) fds |
| `epoll_wait` | `fn(epfd: c_int, events: *mut EpollEvent, maxevents: c_int, timeout: c_int) -> c_int` | Block/poll for readiness events |
| `eventfd` | `fn(initval: c_uint, flags: c_int) -> c_int` | Cross-thread wakeup descriptor (`EFD_CLOEXEC \| EFD_NONBLOCK`) |
| `socket` | `fn(domain: c_int, ty: c_int, protocol: c_int) -> c_int` | Create `SOCK_STREAM \| SOCK_NONBLOCK \| SOCK_CLOEXEC` socket for `AsyncTcpStream::connect` |
| `connect` | `fn(sockfd: c_int, addr: *const c_void, addrlen: u32) -> c_int` | Initiate non-blocking TCP handshake (`EINPROGRESS`), avoiding blocking the single-threaded executor |

**Deliberately omitted (handled safely by `std`):** `close` (via `OwnedFd::from_raw_fd` / `File::from_raw_fd`), `read`/`write` on `eventfd` (via `std::fs::File`), `getsockopt(SO_ERROR)` (via `TcpStream::take_error()`), `bind`/`listen`/`accept`/`shutdown` (via `std::net`).

#### Constants & structs

Constants (verified against Linux & Android constants in `golang/go` and `rust-lang/libc`, pinned in §References):
- `EPOLL_CLOEXEC = 0x80000`, `EPOLL_CTL_ADD = 1`, `EPOLL_CTL_DEL = 2`, `EPOLL_CTL_MOD = 3`
- `EPOLLIN = 0x1`, `EPOLLOUT = 0x4`, `EPOLLERR = 0x8`, `EPOLLHUP = 0x10`, `EPOLLRDHUP = 0x2000`, `EPOLLET = 0x8000_0000`
- `EFD_CLOEXEC = 0x80000`, `EFD_NONBLOCK = 0x800`
- `AF_INET = 2`, `AF_INET6 = 10`, `SOCK_STREAM = 1`, `SOCK_NONBLOCK = 0x800`, `SOCK_CLOEXEC = 0x80000`, `EINPROGRESS = 115` (`0x73`)

Structs & compile-time `size_of` / `offset_of` assertions (Rust 1.85 `core::mem`):
- `EpollEvent`: `#[cfg_attr(target_arch = "x86_64", repr(packed))]` `#[repr(C)]` (`events: u32`, `data: u64`; read `ev.data` by value so no unaligned reference is ever formed). Packed on `x86_64` only (matching Linux UAPI `EPOLL_PACKED`); on 32-bit `x86` (glibc, musl, Android) `u64` already has 4-byte alignment so `#[repr(C)]` has the exact 12-byte kernel stride (`offset_of!(data) == 4`, `align_of == 4`, strictly over-aligned for glibc's 1-byte `packed`).
- `SockAddrIn` (`#[repr(C)]`): `sin_family: u16`, `sin_port: u16`, `sin_addr: [u8; 4]`, `sin_zero: [u8; 8]`.
- `SockAddrIn6` (`#[repr(C)]`): `sin6_family: u16`, `sin6_port: u16`, `sin6_flowinfo: u32`, `sin6_addr: [u8; 16]`, `sin6_scope_id: u32`.

```rust
// Linux & Android x86_64 — packed: 12 bytes, no padding
#[cfg(target_arch = "x86_64")]
const _: () = {
    assert!(core::mem::size_of::<EpollEvent>() == 12);
    assert!(core::mem::align_of::<EpollEvent>() == 1);
    assert!(core::mem::offset_of!(EpollEvent, events) == 0);
    assert!(core::mem::offset_of!(EpollEvent, data) == 4);
};

// Linux & Android 32-bit x86 (glibc, musl, Android) — natural i386 alignment: 12 bytes, align 4
#[cfg(target_arch = "x86")]
const _: () = {
    assert!(core::mem::size_of::<EpollEvent>() == 12);
    assert!(core::mem::align_of::<EpollEvent>() == 4);
    assert!(core::mem::offset_of!(EpollEvent, events) == 0);
    assert!(core::mem::offset_of!(EpollEvent, data) == 4);
};

// Linux & Android aarch64, arm, riscv64 — natural 8-byte u64 alignment: 16 bytes
#[cfg(any(target_arch = "aarch64", target_arch = "arm", target_arch = "riscv64"))]
const _: () = {
    assert!(core::mem::size_of::<EpollEvent>() == 16);
    assert!(core::mem::align_of::<EpollEvent>() == 8);
    assert!(core::mem::offset_of!(EpollEvent, events) == 0);
    assert!(core::mem::offset_of!(EpollEvent, data) == 8);
};

const _: () = {
    assert!(core::mem::size_of::<SockAddrIn>() == 16);
    assert!(core::mem::offset_of!(SockAddrIn, sin_family) == 0);
    assert!(core::mem::offset_of!(SockAddrIn, sin_port) == 2);
    assert!(core::mem::offset_of!(SockAddrIn, sin_addr) == 4);

    assert!(core::mem::size_of::<SockAddrIn6>() == 28);
    assert!(core::mem::offset_of!(SockAddrIn6, sin6_family) == 0);
    assert!(core::mem::offset_of!(SockAddrIn6, sin6_port) == 2);
    assert!(core::mem::offset_of!(SockAddrIn6, sin6_flowinfo) == 4);
    assert!(core::mem::offset_of!(SockAddrIn6, sin6_addr) == 8);
    assert!(core::mem::offset_of!(SockAddrIn6, sin6_scope_id) == 24);
};
```

#### Self-wakeup mechanism (`eventfd`)

Created via `eventfd(0, EFD_CLOEXEC | EFD_NONBLOCK)` and adopted into `std::fs::File`, registered once in `epfd` with reserved `TOKEN_WAKEUP = 0` (`EPOLLIN | EPOLLET`). `unpark()` coalesces concurrent calls via `AtomicBool::swap(true, AcqRel)` and performs a safe `(&self.wake_file).write(&1u64.to_ne_bytes())`, handling `ErrorKind::WouldBlock` (u64 counter saturation) by draining with an 8-byte `read` and retrying (`mio` `src/sys/unix/waker/eventfd.rs:55-60`, Go `src/runtime/netpoll_epoll.go:81-103`). On wakeup receipt in `epoll_wait`, clear the `AtomicBool` and drain with an 8-byte `read` ignoring `WouldBlock`.

#### Estimated `unsafe` block budget: **11–12 blocks**

| Operation in `net/sys/epoll.rs` | `unsafe` blocks | Safety invariant summary |
|---|---:|---|
| `epoll_create1` + `OwnedFd::from_raw_fd` | 2 | Return checked `>= 0` before adopting sole ownership into `OwnedFd` |
| `eventfd` + `File::from_raw_fd` | 2 | Return checked `>= 0` before adopting sole ownership into `File` |
| `epoll_ctl` (`ADD`, `MOD`, `DEL`) | 3 | `epfd` is open; target `fd` is `BorrowedFd<'_>`; `&mut ev` is valid for call (`null_mut()` for `DEL`) |
| `epoll_wait` | 1 | `events` is a pre-initialized `&mut [EpollEvent]` (zeroed once on init — no `MaybeUninit`, no `Vec::set_len`); `maxevents` clamped `>= 1` |
| `socket` + `OwnedFd::from_raw_fd` | 2 | Return checked `>= 0` before adopting into `OwnedFd`; safely converted via `TcpStream::from(owned_fd)` |
| `connect` (IPv4 / IPv6) | 1–2 | Pointer refers to initialized `SockAddrIn`/`SockAddrIn6` with exact `size_of` as `addrlen`; `fd` is open |
| **Total** | **11–12** | **0 `unsafe impl`, 0 `Vec::set_len`, 0 `MaybeUninit`** |

---

### Backend 2: macOS & iOS (`crates/akku/src/net/sys/kqueue.rs`)

Target gate: `#[cfg(all(any(target_os = "macos", target_os = "ios"), any(target_arch = "aarch64", target_arch = "x86_64")))]`.

#### FFI symbols (`extern "C"` — 6 symbols)

| Symbol | Signature | Why needed (no safe `std` equivalent) |
|---|---|---|
| `kqueue` | `fn() -> c_int` | Create kqueue descriptor |
| `kevent` | `fn(kq: c_int, changelist: *const Kevent, nchanges: c_int, eventlist: *mut Kevent, nevents: c_int, timeout: *const Timespec) -> c_int` | Register/deregister filters, trigger `EVFILT_USER` wakeup, and wait for readiness events |
| `fcntl` | `fn(fd: c_int, cmd: c_int, ...) -> c_int` | Set `FD_CLOEXEC` on `kq` and newly created sockets, and `O_NONBLOCK` on sockets (Darwin has no `SOCK_NONBLOCK`/`SOCK_CLOEXEC` or `kqueue1`) |
| `socket` | `fn(domain: c_int, ty: c_int, protocol: c_int) -> c_int` | Create TCP socket for non-blocking `AsyncTcpStream::connect` |
| `setsockopt` | `fn(fd: c_int, level: c_int, name: c_int, val: *const c_void, len: u32) -> c_int` | Set `SO_NOSIGPIPE = 1` on raw connected sockets (matching `std::net` and `mio` `src/sys/unix/net.rs:40-57` so peer disconnect yields `EPIPE`, never `SIGPIPE`) |
| `connect` | `fn(fd: c_int, addr: *const c_void, len: u32) -> c_int` | Initiate non-blocking TCP handshake (`EINPROGRESS = 36` on Darwin) |

**Deliberately omitted (handled safely by `std`):** `close` (immediately adopt `kqueue()` and `socket()` into `OwnedFd::from_raw_fd`), `getsockopt(SO_ERROR)` (via `TcpStream::take_error()`), `bind`/`listen`/`accept`/`shutdown` (via `std::net`).

#### Constants & structs

Constants (verified from `rust-lang/libc@1e106cd95539f5faf40687bc9c380b1185d52715` `src/unix/bsd/apple/mod.rs`, `src/unix/bsd/mod.rs`, `src/unix/mod.rs`):
- `EVFILT_READ = -1i16`, `EVFILT_WRITE = -2i16`, `EVFILT_USER = -10i16`
- `EV_ADD = 0x1u16`, `EV_DELETE = 0x2u16`, `EV_ENABLE = 0x4u16`, `EV_ONESHOT = 0x10u16`, `EV_CLEAR = 0x20u16`, `EV_RECEIPT = 0x40u16`, `EV_ERROR = 0x4000u16`, `EV_EOF = 0x8000u16`
- `NOTE_TRIGGER = 0x0100_0000u32`
- `F_SETFD = 2`, `F_GETFL = 3`, `F_SETFL = 4`, `FD_CLOEXEC = 1`, `O_NONBLOCK = 0x4`
- `AF_INET = 2`, `AF_INET6 = 30`, `SOCK_STREAM = 1`, `SOL_SOCKET = 0xffff`, `SO_NOSIGPIPE = 0x1022`, `EINPROGRESS = 36`

Structs & compile-time `size_of` / `offset_of` assertions (Apple 64-bit LP64 — identical across `aarch64-apple-darwin`, `x86_64-apple-darwin`, `aarch64-apple-ios`, `aarch64-apple-ios-sim`, `x86_64-apple-ios`):
- `Kevent` (`#[repr(C)]`): `ident: usize` (`uintptr_t`), `filter: i16`, `flags: u16`, `fflags: u32`, `data: isize` (`intptr_t`), `udata: usize` (pointer-sized integer storing the 64-bit generational slab token; identical ABI to `*mut c_void`, zero raw-pointer provenance casts). Apple's `<sys/event.h>` wraps `struct kevent` in `#pragma pack(4)` (`rust-lang/libc` `#[repr(packed(4))]`), and on 64-bit LP64 every field sits at its natural offset (`0, 8, 10, 12, 16, 24`, size `32`), so `#[repr(C)]` (align `8`, strictly over-aligned for `pack(4)`) has the exact same array stride and field offsets without unaligned references.
- `Timespec` (`#[repr(C)]`): `tv_sec: i64` (`time_t`), `tv_nsec: i64` (`c_long`). Clamp `tv_sec <= 1_000_000` on Darwin per Go `netpoll_kqueue.go:102-105` to avoid `EINVAL`.
- `SockAddrIn` (`#[repr(C)]`): `sin_len: u8` (set to `16`), `sin_family: u8` (`AF_INET as u8 = 2`), `sin_port: u16`, `sin_addr: [u8; 4]`, `sin_zero: [u8; 8]`.
- `SockAddrIn6` (`#[repr(C)]`): `sin6_len: u8` (set to `28`), `sin6_family: u8` (`AF_INET6 as u8 = 30`), `sin6_port: u16`, `sin6_flowinfo: u32`, `sin6_addr: [u8; 16]`, `sin6_scope_id: u32`.

```rust
// Apple 64-bit (macOS & iOS on aarch64 and x86_64)
const _: () = {
    assert!(core::mem::size_of::<Kevent>() == 32);
    assert!(core::mem::align_of::<Kevent>() == 8);
    assert!(core::mem::offset_of!(Kevent, ident) == 0);
    assert!(core::mem::offset_of!(Kevent, filter) == 8);
    assert!(core::mem::offset_of!(Kevent, flags) == 10);
    assert!(core::mem::offset_of!(Kevent, fflags) == 12);
    assert!(core::mem::offset_of!(Kevent, data) == 16);
    assert!(core::mem::offset_of!(Kevent, udata) == 24);

    assert!(core::mem::size_of::<Timespec>() == 16);
    assert!(core::mem::offset_of!(Timespec, tv_sec) == 0);
    assert!(core::mem::offset_of!(Timespec, tv_nsec) == 8);

    assert!(core::mem::size_of::<SockAddrIn>() == 16);
    assert!(core::mem::offset_of!(SockAddrIn, sin_len) == 0);
    assert!(core::mem::offset_of!(SockAddrIn, sin_family) == 1);
    assert!(core::mem::offset_of!(SockAddrIn, sin_port) == 2);
    assert!(core::mem::offset_of!(SockAddrIn, sin_addr) == 4);

    assert!(core::mem::size_of::<SockAddrIn6>() == 28);
    assert!(core::mem::offset_of!(SockAddrIn6, sin6_len) == 0);
    assert!(core::mem::offset_of!(SockAddrIn6, sin6_family) == 1);
    assert!(core::mem::offset_of!(SockAddrIn6, sin6_port) == 2);
    assert!(core::mem::offset_of!(SockAddrIn6, sin6_flowinfo) == 4);
    assert!(core::mem::offset_of!(SockAddrIn6, sin6_addr) == 8);
    assert!(core::mem::offset_of!(SockAddrIn6, sin6_scope_id) == 24);
};
```

#### Self-wakeup mechanism (`EVFILT_USER` + `NOTE_TRIGGER`)

On reactor creation, register a single user event with `ident = WAKE_IDENT` (`0`), `filter = EVFILT_USER`, `flags = EV_ADD | EV_CLEAR | EV_RECEIPT`, `udata = TOKEN_WAKEUP` (`0`) — zero extra file descriptors needed! `unpark()` coalesces concurrent calls via `AtomicBool::swap(true, AcqRel)` and submits one changelist entry with `ident = WAKE_IDENT`, `filter = EVFILT_USER`, `fflags = NOTE_TRIGGER` via `kevent(kq, &ev, 1, null, 0, null)`, retrying `EINTR` (matching Go `src/runtime/netpoll_kqueue_event.go:16-56`, `mio` `src/sys/unix/selector/kqueue.rs:241-265`, and `polling` `src/kqueue.rs:427-465`).

Socket registration submits both `EVFILT_READ` and `EVFILT_WRITE` once with `EV_ADD | EV_CLEAR | EV_RECEIPT` (edge-triggered, matching Go `netpoll_kqueue.go:36-59` and `mio` `kqueue.rs:127-163`), ignoring `EV_ERROR` with `data == EPIPE` on register and `ENOENT` on deregister (`EV_DELETE | EV_RECEIPT`). Events with `EV_EOF` or `EV_ERROR` wake the corresponding direction (and error wakes both).

#### Estimated `unsafe` block budget: **12–14 blocks**

| Operation in `net/sys/kqueue.rs` | `unsafe` blocks | Safety invariant summary |
|---|---:|---|
| `kqueue` + `OwnedFd::from_raw_fd` | 2 | Return checked `>= 0` before adopting into `OwnedFd` |
| `fcntl(kq, F_SETFD, FD_CLOEXEC)` | 1 | `kq` is open (`OwnedFd`); valid `F_SETFD` command with `c_int` arg |
| `kevent` (register user wakeup, register/deregister fd, trigger wakeup, wait) | 4 | Changelist/eventlist pointers refer to pre-initialized stack arrays or pre-zeroed `Vec<Kevent>`; lengths fit `c_int`; `timeout` is `null` or valid `&Timespec` |
| `socket` + `OwnedFd::from_raw_fd` | 2 | Return checked `>= 0` before adopting into `OwnedFd`; safely converted via `TcpStream::from(owned_fd)` |
| `setsockopt(SO_NOSIGPIPE)` | 1 | `fd` is open; `&1i32` valid with `size_of::<c_int>()` |
| `fcntl` (`O_NONBLOCK`, `FD_CLOEXEC`) on new socket | 2 | `fd` is open; valid `F_SETFL`/`F_SETFD` args |
| `connect` (IPv4 / IPv6) | 1–2 | Pointer refers to initialized `SockAddrIn`/`SockAddrIn6` with exact `size_of` as `addrlen`; `fd` is open |
| **Total** | **12–14** | **0 `unsafe impl`, 0 `Vec::set_len`, 0 `MaybeUninit`** |

---

### Backend 3: Windows (`crates/akku/src/net/sys/wsapoll.rs`)

Target gate: `#[cfg(all(target_os = "windows", any(target_arch = "x86_64", target_arch = "aarch64", target_arch = "x86")))]`.

#### FFI symbols (`#[link(name = "ws2_32")] extern "system"` — 5 symbols)

| Symbol | Signature | Why needed (no safe `std` equivalent) |
|---|---|---|
| `WSAPoll` | `fn(fdarray: *mut WsaPollFd, fds: u32, timeout: i32) -> i32` | Poll registered sockets + loopback wake socket for readiness |
| `WSAGetLastError` | `fn() -> i32` | Read Winsock error code on `SOCKET_ERROR` / `INVALID_SOCKET` |
| `socket` | `fn(af: i32, ty: i32, protocol: i32) -> usize` | Create TCP socket for non-blocking `AsyncTcpStream::connect` |
| `ioctlsocket` | `fn(s: usize, cmd: i32, argp: *mut u32) -> i32` | Set `FIONBIO = 1` before calling non-blocking `connect` |
| `connect` | `fn(s: usize, name: *const c_void, namelen: i32) -> i32` | Initiate non-blocking TCP handshake (`WSAEWOULDBLOCK = 10035`) |

**Deliberately omitted (handled safely by `std`):**
- `WSAStartup`: creating the safe `std::net` loopback wakeup socketpair during reactor init automatically runs `std`'s once-initialized `WSAStartup` before any `ws2_32` FFI call occurs.
- `closesocket`: raw `SOCKET` from `socket()` is checked `!= INVALID_SOCKET` and immediately adopted via `std::os::windows::io::OwnedSocket::from_raw_socket(raw as RawSocket)` (stabilized Rust 1.63), then safely converted via `std::net::TcpStream::from(owned_socket)`.
- `getsockopt(SO_ERROR)`, `bind`, `listen`, `accept`, `shutdown`: handled safely by `std::net`.

#### Constants & structs

Constants (verified from `microsoft/windows-rs@0a952b0ef7e37801ff624b91d314768c727fd276` `crates/libs/sys/src/Windows/Win32/winsock2/mod.rs` and `rust-lang/rust@bc9132daa5c0be572ebb81304f6039d947298e67` `library/std/src/sys/pal/windows/c/windows_sys.rs`):
- `POLLERR = 0x0001i16`, `POLLHUP = 0x0002i16`, `POLLNVAL = 0x0004i16`
- `POLLWRNORM = 0x0010i16`, `POLLOUT = 0x0010i16`, `POLLRDNORM = 0x0100i16`, `POLLRDBAND = 0x0200i16`, `POLLIN = 0x0300i16`
  - **CRITICAL:** Never set `POLLPRI` (`0x0400`) in `WSAPOLLFD::events` — the Microsoft Winsock provider rejects `POLLPRI` with `WSAEINVAL` (Microsoft Learn `WSAPoll`).
- `AF_INET = 2`, `AF_INET6 = 23`, `SOCK_STREAM = 1`, `IPPROTO_TCP = 6`
- `FIONBIO = 0x8004_667eu32 as i32` (`2_147_772_030u32`), `INVALID_SOCKET = usize::MAX`, `SOCKET_ERROR = -1`
- `WSAEWOULDBLOCK = 10035`, `WSAEINPROGRESS = 10036`, `WSAEINTR = 10004`

Structs & compile-time `size_of` / `offset_of` assertions (`x86_64`, `aarch64`, and `x86` Windows):
- `WsaPollFd` (`#[repr(C)]`): `fd: usize` (`SOCKET` = `UINT_PTR`), `events: i16` (`SHORT`), `revents: i16` (`SHORT`).
- `SockAddrIn` (`#[repr(C)]`): `sin_family: u16` (`AF_INET = 2`), `sin_port: u16`, `sin_addr: [u8; 4]`, `sin_zero: [u8; 8]`.
- `SockAddrIn6` (`#[repr(C)]`): `sin6_family: u16` (`AF_INET6 = 23`), `sin6_port: u16`, `sin6_flowinfo: u32`, `sin6_addr: [u8; 16]`, `sin6_scope_id: u32`.

```rust
// 64-bit Windows (x86_64, aarch64): SOCKET is 8 bytes + 2+2 + 4 bytes tail pad = 16 bytes
#[cfg(target_pointer_width = "64")]
const _: () = {
    assert!(core::mem::size_of::<WsaPollFd>() == 16);
    assert!(core::mem::align_of::<WsaPollFd>() == 8);
    assert!(core::mem::offset_of!(WsaPollFd, fd) == 0);
    assert!(core::mem::offset_of!(WsaPollFd, events) == 8);
    assert!(core::mem::offset_of!(WsaPollFd, revents) == 10);
};

// 32-bit Windows (x86): SOCKET is 4 bytes + 2+2 = 8 bytes
#[cfg(target_pointer_width = "32")]
const _: () = {
    assert!(core::mem::size_of::<WsaPollFd>() == 8);
    assert!(core::mem::align_of::<WsaPollFd>() == 4);
    assert!(core::mem::offset_of!(WsaPollFd, fd) == 0);
    assert!(core::mem::offset_of!(WsaPollFd, events) == 4);
    assert!(core::mem::offset_of!(WsaPollFd, revents) == 6);
};

const _: () = {
    assert!(core::mem::size_of::<SockAddrIn>() == 16);
    assert!(core::mem::offset_of!(SockAddrIn, sin_family) == 0);
    assert!(core::mem::offset_of!(SockAddrIn, sin_port) == 2);
    assert!(core::mem::offset_of!(SockAddrIn, sin_addr) == 4);

    assert!(core::mem::size_of::<SockAddrIn6>() == 28);
    assert!(core::mem::offset_of!(SockAddrIn6, sin6_family) == 0);
    assert!(core::mem::offset_of!(SockAddrIn6, sin6_port) == 2);
    assert!(core::mem::offset_of!(SockAddrIn6, sin6_flowinfo) == 4);
    assert!(core::mem::offset_of!(SockAddrIn6, sin6_addr) == 8);
    assert!(core::mem::offset_of!(SockAddrIn6, sin6_scope_id) == 24);
};
```

#### Self-wakeup mechanism (loopback TCP socketpair) & `WSAPoll` quirks

1. **Loopback TCP socketpair (100% safe `std::net` setup):**
   - Bind `std::net::TcpListener::bind("127.0.0.1:0")` on an ephemeral loopback port, connect `writer = std::net::TcpStream::connect(listener.local_addr()?)`, call `listener.accept()`, and verify **both** `peer_addr == writer.local_addr()` **and** an 8-byte nonce written on `writer` matches the 8 bytes read on `reader` (following `curl/curl/lib/socketpair.c:182-253` so another local process cannot hijack the ephemeral loopback listener during init). Drop the listener immediately, and set `set_nonblocking(true)` + `set_nodelay(true)` on both halves.
   - `wake_reader` is permanently placed at index `0` of `fdarray` with `events = POLLRDNORM`. This guarantees `fds >= 1` on every `WSAPoll` call (avoiding `WSAEINVAL` when no user sockets are registered, per Microsoft Learn `WSAPoll`).
   - `unpark()` coalesces via `AtomicBool::swap(true, AcqRel)` and writes `[1u8]` to `wake_writer` (ignoring `WouldBlock`); when `fdarray[0].revents != 0`, clear the flag and drain `wake_reader` into a small stack buffer until `WouldBlock`.
2. **Level-triggered interest discipline (no 100% CPU busy-spin):**
   - Unlike `EPOLLET` / `EV_CLEAR`, `WSAPoll` is **level-triggered**: passing `POLLWRNORM` for an idle TCP socket causes `WSAPoll` to return immediately every tick!
   - Therefore `sys/wsapoll.rs` tracks a per-registration `armed: AtomicU8` bitmask (`READ | WRITE`), populates `fdarray` only with entries that have `armed != 0` (`POLLRDNORM` when `READ` is armed, `POLLWRNORM` when `WRITE` is armed), and disarms delivered directions (one-shot semantics) until the next `WouldBlock` re-arms them.
3. **Failed non-blocking `connect()` on Windows 10 2004+ (build 19041) / Windows Server 2022:**
   - Per Microsoft Learn (`WSAPoll`), as of Windows 10 version 2004 (build 19041) — which includes all currently supported Windows 10/11 and Windows Server 2022/2025 CI runners (`windows-latest`) — a failed non-blocking TCP `connect()` reports `(POLLHUP | POLLERR | POLLWRNORM)` in `revents`, waking the connect future so `TcpStream::take_error()` retrieves `WSAECONNREFUSED` (`ErrorKind::ConnectionRefused`).
   - `IOCP` + `AFD` (`net/sys/iocp.rs`, `net/sys/afd.rs`) remains the planned benchmarked performance follow-up for scaling past ~1k concurrent sockets (`_dev/TODO.md`, `docs/forks/tokio-axum.md` §9.2).

#### Estimated `unsafe` block budget: **8–10 blocks**

| Operation in `net/sys/wsapoll.rs` | `unsafe` blocks | Safety invariant summary |
|---|---:|---|
| `WSAPoll` + `WSAGetLastError` | 2 | `fdarray` is a valid `&mut [WsaPollFd]` with `len >= 1` (slot 0 is `wake_reader`) and `len <= u32::MAX`; `WSAGetLastError` has no preconditions |
| `socket` + `OwnedSocket::from_raw_socket` | 2 | Return checked `!= INVALID_SOCKET` before adopting into `OwnedSocket`; safely converted via `TcpStream::from(owned_socket)` |
| `ioctlsocket(FIONBIO)` | 1 | `socket` is open; `&mut 1u32` is valid writable `u_long` |
| `connect` (IPv4 / IPv6) + `WSAGetLastError` | 3–5 | Pointer refers to initialized `SockAddrIn`/`SockAddrIn6` with exact `size_of` as `namelen`; `socket` is open |
| **Total** | **8–10** | **0 `unsafe impl`, 0 `Vec::set_len`, 0 `MaybeUninit`** |

---

### Tier-1 CI & mobile cross-check strategy

- **Desktop Tier-1 (executed on every PR in `.github/workflows/ci.yml`):**
  - Linux (`test` on `ubuntu-latest` + `smd` Alpine/musl): runs full `epoll` + `akku::task` + `akku::io` + `akku::net` test suite.
  - macOS (`os-test (macos-latest)`): runs full `kqueue` + `akku::task` + `akku::io` + `akku::net` test suite.
  - Windows (`os-test (windows-latest)`): runs full `WSAPoll` + `akku::task` + `akku::io` + `akku::net` test suite.
- **Mobile Tier-1 (compile-checked only — no emulator/simulator runtime in CI):**
  - **Android** (shares `net/sys/epoll.rs` via `target_os = "android"`):
    - `aarch64-linux-android`
    - `armv7-linux-androideabi`
    - `x86_64-linux-android`
  - **iOS** (shares `net/sys/kqueue.rs` via `target_os = "ios"`):
    - `aarch64-apple-ios`
    - `aarch64-apple-ios-sim`
    - `x86_64-apple-ios`
  - **How CI and `flake.nix` can cross-check mobile targets cleanly without an NDK or Xcode SDK:**
    - `cargo check --lib -p akku --no-default-features --target <triple> --locked` does **not** invoke a platform linker, yet fully evaluates every `#[cfg]` gate, FFI signature, and compile-time `const _: () = assert!(size_of / offset_of)` check!
    - In `.github/workflows/ci.yml`: add a lightweight `mobile-check` job (on `ubuntu-latest` using `dtolnay/rust-toolchain@stable` with `targets: aarch64-linux-android,x86_64-linux-android,aarch64-apple-ios,aarch64-apple-ios-sim`) running `cargo check --lib` for each target.
    - In `flake.nix`: extend `toolchain`'s `targets = [...]` list (currently `["wasm32-unknown-unknown" "x86_64-pc-windows-gnu"]`) with `aarch64-linux-android`, `x86_64-linux-android`, `aarch64-apple-ios`, and `aarch64-apple-ios-sim` so contributors can verify mobile compilation locally via `nix shell`.

## References

Pinned to the commits read on 2026-11-25.

- **Go (`golang/go`)** `golang/go@d695060f5be23f2dd86d21d861a292dc2d183cab`
  - `src/runtime/netpoll_epoll.go:25-34,36-58,64-74,81-103,131-186`: `EPOLLIN | EPOLLOUT | EPOLLRDHUP | EPOLLET` single registration (`:66`), generation tag (`fdseq`) in `ev.Data` (`:25-34,182`), `epoll_create1` + `eventfd`, `atomic.Uint32` CAS wakeup coalescing (`:83`), `EINTR` retry (`:135-145`).
  - `src/runtime/netpoll_kqueue.go:22-60,72-140`: `kqueue` + `closeonexec`, dual `EVFILT_READ`/`EVFILT_WRITE` registration with `EV_ADD | EV_CLEAR` (`:36-55`), Darwin `timespec.tv_sec <= 1e6` clamp (`:102-105`), `EINTR`/`ETIMEDOUT` handling (`:115`).
  - `src/runtime/netpoll_kqueue_event.go:13-74`: `EVFILT_USER` + `NOTE_TRIGGER` zero-fd kqueue wakeup event (`addWakeupEvent`, `wakeNetpoll`, `isWakeup`).
  - `src/internal/poll/fd_poll_runtime.go:38-129` and `src/internal/poll/fd_unix.go:142-182,369-410`: `maxRW = 1 << 30`, zero-length read guard (`:150-162`), `EINTR` retry (`:170,383`), `EAGAIN` wait (`:173,398`).
  - `src/internal/runtime/syscall/linux/defs_linux.go:12-23`, `defs_linux_amd64.go:22-30`, `defs_linux_arm64.go:22-31`: Linux `EpollEvent` ABI (12 bytes packed on `amd64`, 16 bytes aligned on `arm64`).
- **Tokio (`tokio-rs/tokio`)** `tokio-rs/tokio@854013a8cb7d1397e9d037315592dc1d2d55187d`
  - `tokio/src/runtime/io/driver.rs:108-135,185-244`: `TOKEN_WAKEUP = Token(0)`, reusable `Events` buffer (`nevents = 1024`), `ErrorKind::Interrupted` handling (`:215`).
  - `tokio/src/runtime/io/registration.rs:28-33,107-116,173-192,225-263`: single-reader/single-writer waker discipline, `assume_ready` on accepted sockets, `WouldBlock` -> `clear_readiness` loop, waker cleanup on `Drop`.
  - `tokio/src/net/tcp/listener.rs:102-194,243-249` and `tokio/src/net/tcp/stream.rs:121-181,225-231,1426-1443,1491-1532`: `TcpListener`/`TcpStream`, non-blocking `connect` + `take_error()` (`:156-160`), `split`/`into_split`, `OwnedWriteHalf::drop` calling `shutdown(Shutdown::Write)` (`:1436-1437`), `poll_flush` no-op (`:1523-1526`), `poll_shutdown` (`:1528-1531`).
- **Mio (`tokio-rs/mio`)** `tokio-rs/mio@101e5b7c92b1b5f8abc0ba3eb81b264943e13e37`
  - `src/sys/unix/selector/epoll.rs:27-106,129-189,231-236` and `src/sys/unix/waker/eventfd.rs:20-92`: `epoll` flags, packed `event.u64` copy, `eventfd` in `std::fs::File` with overflow drain+retry.
  - `src/sys/unix/selector/kqueue.rs:27-53,75-118,126-260`: Apple `kevent` field types (`filter: i16`, `flags: u16`), `FD_CLOEXEC` on `kq`, `EV_ADD | EV_CLEAR | EV_RECEIPT`, `EPIPE`/`ENOENT` handling, and `EVFILT_USER` + `NOTE_TRIGGER` wake (`:241-260`).
  - `src/sys/unix/net.rs:15-85` and `src/sys/unix/tcp.rs:13-38,57-87`: non-blocking `socket` + `connect` (`EINPROGRESS`), Apple `SO_NOSIGPIPE` (`:40-57`) + `fcntl(O_NONBLOCK/FD_CLOEXEC)` (`:59-82`), and Android x86 `accept4` seccomp caveat (`tcp.rs:64-66`).
- **`polling`, `async-io`, & `async-net`**
  - `smol-rs/polling@35ba7a4fa8bf1e9f33711cb140a015d6bfe84c1b` `src/epoll.rs:41-87,297-323,417-518` and `src/kqueue.rs:51-90,427-465`: `epoll` and Apple `EVFILT_USER` notification.
  - `smol-rs/async-io@e94f31c472d61101dac0aaa67c4fb42068bd8071` `src/reactor.rs:70,115-144` and `src/lib.rs:1523-1544`: safe `Mutex<Slab<Arc<Source>>>` token dispatch and non-blocking `connect` + `take_error()`.
  - `smol-rs/async-net@b2fea5f9294be19abe108a44a93c4e1b4c07bf9f` `src/tcp.rs:48-145,221-227`: `set_nonblocking(true)` inside `from_std`.
- **`libc` (`rust-lang/libc`)** `rust-lang/libc@1e106cd95539f5faf40687bc9c380b1185d52715`
  - Linux & Android: `src/unix/linux_like/mod.rs:62-75,356-364,718,726,1085-1103`, `src/unix/linux_like/android/mod.rs:697,705,724,728-729,1096,1223,1878`, `src/unix/linux_like/linux/gnu/b64/{x86_64,aarch64}/mod.rs`, `src/unix/linux_like/linux/musl/mod.rs:515-576`.
  - Apple (macOS & iOS): `src/unix/bsd/apple/mod.rs:423-429,1392-1400,2347,2439-2441,2834,2863,2928,2983,3012,3224-3260` (`sockaddr_in`, `kevent`, `EINPROGRESS`, `F_*`, `AF_INET6=30`, `SO_NOSIGPIPE`, `EVFILT_*`, `EV_*`, `NOTE_TRIGGER`), `src/unix/bsd/mod.rs:43-50,304` (`sockaddr_in6`, `O_NONBLOCK`), `src/unix/mod.rs:81,308` (`timespec`, `FD_CLOEXEC`).
- **Windows ABI & `WSAPoll` (`microsoft/windows-rs`, `rust-lang/rust`, `curl/curl`, Microsoft Learn)**
  - `microsoft/windows-rs@0a952b0ef7e37801ff624b91d314768c727fd276` `crates/libs/sys/src/Windows/Win32/winsock2/mod.rs:60,99,149,167,180,222,234-236,462-471,569,611,890-894`: exact `WSAPOLLFD`, `WSAPoll`, `WSAGetLastError`, `ioctlsocket`, `socket`, `connect`, `POLL*`, `FIONBIO`, `INVALID_SOCKET`, `SOCKET_ERROR`.
  - `rust-lang/rust@bc9132daa5c0be572ebb81304f6039d947298e67` `library/std/src/sys/pal/windows/c/windows_sys.rs:123,141,147,172-173,2696,2809,3198,3203,3435-3436,3468` and `library/std/src/sys/net/connection/socket/windows.rs:72-89`: Windows `AF_INET6 = 23`, `WSAEWOULDBLOCK = 10035`, `WSAEINPROGRESS = 10036`, `WSAEINTR = 10004`, `sockaddr_in`, `sockaddr_in6`.
  - `curl/curl@f26b3ac90bb343fe426cab40ed775dd661248502` `lib/socketpair.c:144-255` and `lib/select.c:133-166,239-243`: Windows loopback TCP wakeup socketpair with peer verification & random nonce check, and `POLLERR`/`POLLHUP` mapping.
  - Microsoft Learn `WSAPoll` (`https://learn.microsoft.com/en-us/windows/win32/api/winsock2/nf-winsock2-wsapoll`) and `WSAPOLLFD` (`https://learn.microsoft.com/en-us/windows/win32/api/winsock2/ns-winsock2-wsapollfd`): `fds >= 1` requirement, `POLLPRI` -> `WSAEINVAL`, and Windows 10 version 2004 (build 19041) `(POLLHUP | POLLERR | POLLWRNORM)` failed-connect behavior.

## Public API

Signatures and contracts for Phase 1 (no implementation). All fallible operations return `std::io::Result<T>`.

### `std::net` re-exports (explicit list, all targets including `wasm32`)

```rust
pub use std::net::{
    AddrParseError, Incoming, IpAddr, Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, SocketAddrV4,
    SocketAddrV6, TcpListener, TcpStream, ToSocketAddrs, UdpSocket,
};
```

### DoS limits & defaults

`akku::net` itself does not allocate unbounded per-connection buffers (callers supply `&mut [u8]` / `&[u8]`); reactor-level bounds are owned by `akku::task::Limits` and internal reactor invariants:
- **`max_io_events_per_tick`**: default `1_024` (from `akku::task::Limits`; bounds the pre-allocated event buffer on `epoll`/`kqueue` to ~12–32 KiB per runtime).
- **`max_registrations`**: internal cap of `65_536` concurrent registered sockets per reactor (prevents unbounded slab growth under fd/socket leaks; returns `io::Error::other("reactor registration limit reached")` if exceeded, though OS `RLIMIT_NOFILE` typically triggers `EMFILE` earlier).
- **Single syscall read/write slice cap**: `1 << 30` bytes (1 GiB, matching Go `fd_unix.go:142` and preventing signed `isize`/`c_int` truncation edge cases on Unix and Windows).
- **Zero-length I/O guard**: `poll_read` with `&mut []` and `poll_write` with `&[]` return `Poll::Ready(Ok(0))` immediately without waiting or misreporting EOF.
- **Generational token layout (`u64`)**: token `0` is reserved for the reactor wakeup; registered sources use `((generation as u64) << 32) | (slot_index as u64)` (with `generation >= 1`, wrapping `u32::MAX -> 1`), rejecting stale events after fd/socket close and reuse.

### `AsyncTcpListener` (`#[cfg(not(target_arch = "wasm32"))]`)

```rust
/// An async TCP socket server listening for incoming connections on the current `akku::task` runtime.
pub struct AsyncTcpListener { /* private: std::net::TcpListener + reactor Registration */ }
// Implements `fmt::Debug`, `Send`, `Sync`, `#[cfg(unix)] AsFd / AsRawFd`,
// and `#[cfg(windows)] AsSocket / AsRawSocket`.
// `Drop` deregisters from the OS reactor BEFORE closing the underlying `std::net::TcpListener`.

impl AsyncTcpListener {
    /// Creates a new `AsyncTcpListener` bound to `addr`.
    /// Must be called from inside an active `akku::task::Runtime` (returns `Err(io::Error)` otherwise).
    /// Works natively on all Tier-1 platforms (Linux, Android, macOS, iOS, Windows).
    pub async fn bind<A: ToSocketAddrs>(addr: A) -> std::io::Result<Self>;

    /// Accepts a new incoming TCP connection, returning the non-blocking [`AsyncTcpStream`]
    /// and the remote peer address. Cancel-safe.
    pub async fn accept(&self) -> std::io::Result<(AsyncTcpStream, SocketAddr)>;

    /// Polls for an incoming connection.
    pub fn poll_accept(
        &self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<(AsyncTcpStream, SocketAddr)>>;

    /// Wraps an existing `std::net::TcpListener`, switching it to non-blocking mode
    /// and registering it with the current `akku::task` runtime.
    pub fn from_std(listener: std::net::TcpListener) -> std::io::Result<Self>;

    /// Deregisters from the reactor and returns the underlying `std::net::TcpListener`
    /// (still in non-blocking mode).
    pub fn into_std(self) -> std::io::Result<std::net::TcpListener>;

    pub fn local_addr(&self) -> std::io::Result<SocketAddr>;
    pub fn ttl(&self) -> std::io::Result<u32>;
    pub fn set_ttl(&self, ttl: u32) -> std::io::Result<()>;
    pub fn take_error(&self) -> std::io::Result<Option<std::io::Error>>;
}
```

### `AsyncTcpStream` and halves (`#[cfg(not(target_arch = "wasm32"))]`)

```rust
/// An async TCP stream between a local and a remote socket on the current `akku::task` runtime.
pub struct AsyncTcpStream { /* private: std::net::TcpStream + reactor Registration */ }
// Implements `akku::io::AsyncRead`, `akku::io::AsyncWrite`, `fmt::Debug`, `Send`, `Sync`,
// `#[cfg(unix)] AsFd / AsRawFd`, and `#[cfg(windows)] AsSocket / AsRawSocket`.
// `Drop` deregisters from the OS reactor BEFORE closing the underlying `std::net::TcpStream`.

impl AsyncTcpStream {
    /// Opens a non-blocking async TCP connection to `addr`.
    /// Tries each resolved address in order until one succeeds (returning the last error if all fail).
    /// Cancel-safe: dropping the future closes the in-progress socket cleanly.
    /// Works natively on all Tier-1 platforms (Linux, Android, macOS, iOS, Windows).
    pub async fn connect<A: ToSocketAddrs>(addr: A) -> std::io::Result<Self>;

    /// Wraps an existing `std::net::TcpStream`, switching it to non-blocking mode
    /// and registering it with the current `akku::task` runtime.
    pub fn from_std(stream: std::net::TcpStream) -> std::io::Result<Self>;

    /// Deregisters from the reactor and returns the underlying `std::net::TcpStream`
    /// (still in non-blocking mode).
    pub fn into_std(self) -> std::io::Result<std::net::TcpStream>;

    // Low-level poll & non-blocking try hooks (`akku::io::AsyncRead` / `AsyncWrite` delegate here;
    // async `read`, `read_exact`, `copy_to`, `write`, `write_all`, `flush`, and `shutdown` come
    // from `akku::io::{AsyncReadExt, AsyncWriteExt}`).
    pub fn poll_read(
        &mut self,
        cx: &mut std::task::Context<'_>,
        buf: &mut [u8],
    ) -> std::task::Poll<std::io::Result<usize>>;
    pub fn poll_write(
        &mut self,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>>;
    pub fn poll_shutdown(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>>;
    pub fn try_read(&self, buf: &mut [u8]) -> std::io::Result<usize>;
    pub fn try_write(&self, buf: &[u8]) -> std::io::Result<usize>;

    /// Shuts down the read, write, or both halves of this connection immediately.
    /// Named `shutdown_how` so it does not shadow 0-arg `akku::io::AsyncWriteExt::shutdown(&mut self)`.
    pub fn shutdown_how(&self, how: Shutdown) -> std::io::Result<()>;

    // Concurrent full-duplex splitting
    pub fn split(&mut self) -> (AsyncTcpReadHalf<'_>, AsyncTcpWriteHalf<'_>);
    pub fn into_split(self) -> (AsyncTcpOwnedReadHalf, AsyncTcpOwnedWriteHalf);

    // Metadata & std-safe options
    pub fn local_addr(&self) -> std::io::Result<SocketAddr>;
    pub fn peer_addr(&self) -> std::io::Result<SocketAddr>;
    pub fn nodelay(&self) -> std::io::Result<bool>;
    pub fn set_nodelay(&self, nodelay: bool) -> std::io::Result<()>;
    pub fn ttl(&self) -> std::io::Result<u32>;
    pub fn set_ttl(&self, ttl: u32) -> std::io::Result<()>;
    pub fn take_error(&self) -> std::io::Result<Option<std::io::Error>>;
}

/// Borrowed read half of an [`AsyncTcpStream`] (zero allocation).
/// Implements `akku::io::AsyncRead`, `fmt::Debug`, `Send`, `Sync`.
pub struct AsyncTcpReadHalf<'a> { /* private: &'a AsyncTcpStream */ }
impl AsyncTcpReadHalf<'_> {
    pub fn poll_read(&mut self, cx: &mut std::task::Context<'_>, buf: &mut [u8]) -> std::task::Poll<std::io::Result<usize>>;
    pub fn try_read(&self, buf: &mut [u8]) -> std::io::Result<usize>;
    pub fn local_addr(&self) -> std::io::Result<SocketAddr>;
    pub fn peer_addr(&self) -> std::io::Result<SocketAddr>;
}

/// Borrowed write half of an [`AsyncTcpStream`] (zero allocation).
/// Implements `akku::io::AsyncWrite`, `fmt::Debug`, `Send`, `Sync`.
pub struct AsyncTcpWriteHalf<'a> { /* private: &'a AsyncTcpStream */ }
impl AsyncTcpWriteHalf<'_> {
    pub fn poll_write(&mut self, cx: &mut std::task::Context<'_>, buf: &[u8]) -> std::task::Poll<std::io::Result<usize>>;
    pub fn poll_shutdown(&mut self, cx: &mut std::task::Context<'_>) -> std::task::Poll<std::io::Result<()>>;
    pub fn try_write(&self, buf: &[u8]) -> std::io::Result<usize>;
    pub fn local_addr(&self) -> std::io::Result<SocketAddr>;
    pub fn peer_addr(&self) -> std::io::Result<SocketAddr>;
}

/// Owned read half of an [`AsyncTcpStream`] (movable into a separate `akku::task::spawn`ed task).
/// Implements `akku::io::AsyncRead`, `fmt::Debug`, `Send`, `Sync`.
pub struct AsyncTcpOwnedReadHalf { /* private: Arc<AsyncTcpStream> */ }
// Same `poll_read` / `try_read` / `local_addr` / `peer_addr` methods as `AsyncTcpReadHalf`.

/// Owned write half of an [`AsyncTcpStream`] (movable into a separate `akku::task::spawn`ed task).
/// Implements `akku::io::AsyncWrite`, `fmt::Debug`, `Send`, `Sync`.
/// Dropping `AsyncTcpOwnedWriteHalf` calls `shutdown(Shutdown::Write)` (ignoring errors) so the peer
/// observes EOF promptly (decided 2026-11-25).
pub struct AsyncTcpOwnedWriteHalf { /* private: Arc<AsyncTcpStream> */ }
// Same `poll_write` / `poll_shutdown` / `try_write` / `local_addr` / `peer_addr` methods as `AsyncTcpWriteHalf`.
```

### Error contracts

- Every fallible method returns `std::io::Result<T>` (no custom wrapper error type, matching `std::net`, `tokio::net`, and `async-net`).
- Calling `bind`, `connect`, or `from_std` outside an active `akku::task::Runtime` returns `Err(io::Error::other("no active akku::task runtime"))` rather than panicking (decided 2026-11-25).
- Calling `bind`, `connect`, or `from_std` after runtime shutdown returns `Err(io::Error::other("akku::task runtime is shutting down"))`, and pending I/O operations wake and fail with the same error (matching `tokio/src/runtime/io/registration.rs:265-267`).
- All five Tier-1 platforms (Linux, Android, macOS, iOS, Windows) run native backends in Phase 1; only non-Tier-1 / exotic OS targets return `Err(io::Error::new(io::ErrorKind::Unsupported, ...))`.

## API rationale

### 1. Direct `akku::io::{AsyncRead, AsyncWrite}` integration ("one obvious way")

With `akku::io` bundled into Phase 1 (human decision, 2026-11-25), `AsyncTcpStream`, `AsyncTcpReadHalf`, `AsyncTcpWriteHalf`, `AsyncTcpOwnedReadHalf`, and `AsyncTcpOwnedWriteHalf` implement `akku::io::AsyncRead` and/or `akku::io::AsyncWrite` directly. Async operations (`read`, `read_exact`, `copy_to`, `write`, `write_all`, `flush`, `shutdown`) live in one place (`akku::io::{AsyncReadExt, AsyncWriteExt}`), following `CONCEPT.md` §8 ("one obvious way"). Naming the explicit `Shutdown`-argument method `AsyncTcpStream::shutdown_how(&self, how: Shutdown)` avoids shadowing 0-arg `AsyncWriteExt::shutdown(&mut self)` (which shuts down `Shutdown::Write`).

### 2. Full `std::net` superset, `Async` prefix, and split-half naming (`Namespace.md` §3)

Per `Namespace.md` §3, `akku::net` re-exports all blocking `std::net` items (`TcpListener`, `TcpStream`, `UdpSocket`, `Incoming`, plus all address/shutdown types) by explicit list with unchanged semantics, so `use akku::net` is a drop-in superset of `std::net`. Async counterparts use the `Async` prefix (`AsyncTcpListener`, `AsyncTcpStream`, and `AsyncUdpSocket` / `AsyncUnix*` in Phase 2).

For `AsyncTcpStream`'s split halves, unprefixed names (`ReadHalf`/`WriteHalf`/`OwnedReadHalf`/`OwnedWriteHalf`) would not clash with `std::net`, so either prefixed or unprefixed names are legal. We choose **`AsyncTcpReadHalf<'a>`, `AsyncTcpWriteHalf<'a>`, `AsyncTcpOwnedReadHalf`, and `AsyncTcpOwnedWriteHalf`** for three reasons:
- **Explicit async identity:** in a flat module exporting both blocking `TcpStream` and async `AsyncTcpStream`, the `Async` prefix makes clear at every function signature that these halves implement `AsyncRead`/`AsyncWrite` (not blocking `std::io::Read`/`Write`), matching `Namespace.md` §3(a).
- **Flat-module future-proofing (Phase 2):** `akku::net` is flat (`Namespace.md` Core rule 3); when Phase 2 adds `AsyncUnixStream::split()` / `into_split()`, `AsyncUnix*Half` sits beside `AsyncTcp*Half` cleanly without colliding on `ReadHalf` or forcing nested `net::tcp`/`net::unix` submodules.
- **Zero day-to-day ergonomic cost:** callers typically destructure `let (mut rx, mut tx) = stream.into_split();` with type inference or accept `impl AsyncRead`/`impl AsyncWrite`, while docs and autocomplete group every async TCP type together under `AsyncTcp*`.

### 3. Unified internal `sys::Poller` across edge-triggered (`epoll`/`kqueue`) and level-triggered (`WSAPoll`) backends

All three Tier-1 backends sit behind one small `pub(crate)` `sys::Poller` trait/facade:
- `new(max_events: usize) -> io::Result<Self>`
- `register(&self, raw: RawHandle, token: u64) -> io::Result<()>`
- `deregister(&self, raw: RawHandle, token: u64) -> io::Result<()>`
- `arm(&self, token: u64, interest: Interest)` — no-op on edge-triggered `epoll` (`EPOLLET`) and `kqueue` (`EV_CLEAR`), which stay permanently registered for `READ | WRITE`; sets the interest bit on level-triggered `WSAPoll` when a socket hits `WouldBlock` so idle writable sockets never busy-spin `WSAPoll`.
- `wait(&mut self, events: &mut Vec<ReadyEvent>, timeout: Option<Duration>) -> io::Result<()>`
- `unpark(&self)` (safe to call from any thread; coalesced with an `AtomicBool` CAS).
- `connect_nonblocking(addr: SocketAddr) -> io::Result<(std::net::TcpStream, bool)>` (returns `connected_immediately: bool`).

This keeps the safe reactor (`Slab<Arc<Source>>` + generational `u64` tokens + per-direction readiness generation counters) 100% shared across all five Tier-1 OSes while confining platform syscalls strictly to `net/sys/{epoll,kqueue,wsapoll}.rs`.

### 4. Other key design decisions

- **Generational `Slab` tokens (`u64`) instead of raw-pointer tokens.** Discards stale OS events after close/reuse with zero `unsafe` provenance casts and no deferred-free list (contrasting with `tokio/src/runtime/io/registration_set.rs:25-30`).
- **Pre-initialized event buffers.** Allocating zeroed `Vec<EpollEvent>` / `Vec<Kevent>` / `Vec<WsaPollFd>` once per `Runtime` eliminates `MaybeUninit` and `unsafe { Vec::set_len }` across all three `sys/` backends.
- **True non-blocking `connect` on all three backends.** Avoids freezing the single-threaded executor or leaking helper threads on drop-cancellation; `std::net::TcpStream::take_error()` checks `SO_ERROR` safely on all OSes once writability/error arrives.
- **`&mut self` on `AsyncRead`/`AsyncWrite` + `split()`/`into_split()`.** Compile-time enforcement of single reader + single writer per stream, avoiding lost-wakeup waker clobbering (`tokio/src/runtime/io/registration.rs:28-33`).
- **`from_std` calls `set_nonblocking(true)` automatically.** Matches `smol-rs/async-net` (`src/tcp.rs:224-226`) rather than rejecting blocking sockets.
- **Deregister before close in `Drop` and `into_std`.** Prevents fd/socket reuse races before reactor removal.
- **Optimistic readiness on `accept()`.** Newly accepted sockets pre-seed `READABLE | WRITABLE` (`tokio/src/net/tcp/stream.rs:174-181`) to save an extra reactor wait turn.
- **Ignore `EINTR` (`WSAEINTR`) transparently everywhere.** Both reactor `wait` and socket ops retry on `io::ErrorKind::Interrupted`.

## Seams

Public boundaries targeted by `crates/akku/src/net/tests.rs` (G3) — all async TCP seams run natively on Linux (`ubuntu-latest` + `smd` musl), macOS (`macos-latest`), and Windows (`windows-latest`):

| Seam | Catches | Misses |
|---|---|---|
| Full `std::net` re-exports (`TcpListener`, `TcpStream`, `UdpSocket`, `Incoming`, `IpAddr`, `SocketAddr`, `Shutdown`, `AddrParseError`, `ToSocketAddrs`) on all OSes + `wasm32` | missing re-export, accidentally shadowed blocking type, broken `wasm32` compile | — |
| `AsyncTcpListener::bind` (IPv4 `127.0.0.1:0`, IPv6 `[::1]:0`, port collision `AddrInUse`) + `local_addr` + `accept` | bind/listen bugs, IPv6 `SockAddrIn6` layout bugs (`sin6_len` on Apple, `AF_INET6 = 23` on Windows), wrong peer addr | hostname DNS concurrency (Phase 4) |
| `AsyncTcpStream::connect` (loopback IPv4/IPv6, connection refused, multi-address fallback) | `EINPROGRESS`/`WSAEWOULDBLOCK` mishandling, missed `take_error()` (`SO_ERROR`), Windows `WSAPoll` failed-connect error reporting, byte-order bugs | WAN packet loss |
| `akku::io::{AsyncRead, AsyncWrite, AsyncReadExt, AsyncWriteExt}` (including `copy_to`) on `AsyncTcpStream`, `split`, `into_split` | partial write truncation, `UnexpectedEof`, lost edge/level wakeup, read/write waker clobbering, `AsyncTcpOwnedWriteHalf::drop` auto-shutdown | TLS framing |
| `from_std` / `into_std` + `Drop` deregistration + no-runtime `Err(io::Error)` | blocking socket left blocking, leaked slab slots, stale fd/socket-reuse events, unexpected panic outside runtime | — |
| `sys/{epoll,kqueue,wsapoll}.rs` compile-time `size_of`/`offset_of` assertions + cross-thread wakeup (`eventfd` / `EVFILT_USER` / loopback pair) | per-arch ABI mismatch, wakeup coalescing/drain bugs, Windows `WSAPoll` idle busy-spin or `WSAEINVAL` | mobile runtime execution (iOS/Android compile-checked) |

### Adversarial test list for G3

Covering every applicable class in `akku-tdd` across Linux, macOS, and Windows CI:

1. **Half-close (TCP FIN in one direction):**
   - Client sends payload, calls `stream.shutdown().await?` (or drops `AsyncTcpOwnedWriteHalf`), then continues reading the server's response until EOF (`read` returns `Ok(0)`). Server reads until EOF, then writes a full response and closes. Verify zero bytes lost and clean `Ok(0)` on both sides (catches `EPOLLRDHUP` on Linux, `EV_EOF` on macOS, `POLLHUP`/`POLLRDNORM` on Windows).
   - Reverse half-close (`shutdown_how(Shutdown::Write)` from server while still reading client bytes).
   - Writing after peer closed read end returns `BrokenPipe` / `ConnectionReset` cleanly (never terminates via `SIGPIPE` on macOS — verifies `SO_NOSIGPIPE` — and never panics or spins on `EPOLLERR`/`EV_ERROR`/`POLLERR`).
2. **Partial writes & backpressure (buffer saturation):**
   - Server accepts an `AsyncTcpStream` and delays reading while the client writes 4–16 MiB via `write_all` (exceeding kernel `SO_SNDBUF` + `SO_RCVBUF`), forcing short writes and `WouldBlock` repeatedly. Server drains all bytes; verify exact byte-for-byte delivery with no lost `EPOLLOUT` / `EVFILT_WRITE` / `POLLWRNORM` wakeup.
   - Verify `read(&mut [])` / `write(&[])` return `Ok(0)` immediately; verify `read_exact` on mid-frame peer close returns `ErrorKind::UnexpectedEof`.
3. **Drop-cancellation:**
   - Drop an in-flight `listener.accept()` future, then call `accept()` again and connect — accepts cleanly with no lost connection or stale waker.
   - Drop an in-flight `stream.read(...)` future while idle, then read again after peer sends — receives all data with no lost wakeup.
   - Rapid bind/connect/accept/drop loop (300–500 short-lived connections) — verifies deregister-before-close, slab slot reclamation, and generational token rejection of stale events across all three OSes.
   - Drop an in-flight `AsyncTcpStream::connect(...)` future after a single poll — closes the socket cleanly with no fd/handle leak.
4. **Waker storms, `AsyncReadExt::copy_to`, & concurrent streams:**
   - 64 concurrent client/server echo streams over one single-threaded `Runtime` using `into_split()` + `rx.copy_to(&mut tx).await`, verifying fairness, no read/write waker clobbering, and no Windows `WSAPoll` busy-spin.
   - Stress cross-thread reactor wakeups: 4 background threads wake the parked runtime 2,000 times while an echo server runs concurrently — exercises `eventfd` (Linux), `EVFILT_USER` (macOS), and loopback wake socketpair (Windows) with zero lost wakeups.
5. **`EINTR` / `EAGAIN` (`WouldBlock`) & idle sleep:**
   - Repeated `try_read` / `try_write` on idle/saturated sockets return `WouldBlock` cleanly and clear/re-arm readiness so subsequent `.await` wakes properly.
   - Idle runtime with open connected sockets parks cleanly without spinning CPU (specifically catches level-triggered `WSAPoll` `POLLWRNORM` spin regressions on Windows).
6. **FD / registration limits:**
   - Verify hitting the reactor registration limit (or `EMFILE` in an isolated test) returns clean `Err(io::Error)`, never panics, and recovers once sockets drop.
7. **Panic inside a spawned connection handler:**
   - Connection #1 handler panics mid-read/write while holding its `AsyncTcpStream`; verify clean deregistration and close, healthy runtime, and connection #2 completes normally.
8. **Connection refusal, no-runtime error, blocking `std::net` identity, & `from_std`/`into_std`:**
   - `AsyncTcpStream::connect` to an unbound ephemeral port returns `Err(e)` with `e.kind() == ErrorKind::ConnectionRefused` on Linux, macOS, **and Windows** (directly verifying Windows 10 2004+ `WSAPoll` failed-connect `(POLLHUP | POLLERR | POLLWRNORM)` handling!).
   - `AsyncTcpListener::bind` to an already-bound address returns `ErrorKind::AddrInUse`.
   - Calling `AsyncTcpListener::bind`, `AsyncTcpStream::connect`, `AsyncTcpListener::from_std`, `AsyncTcpStream::from_std` outside any `akku::task::Runtime` returns `Err(io::Error)` without panicking.
   - Verify `akku::net::{TcpListener, TcpStream, UdpSocket}` are type-identical to `std::net::{TcpListener, TcpStream, UdpSocket}` and usable synchronously outside any runtime.
   - `from_std` on a blocking `akku::net::TcpListener` / `TcpStream` (`std::net`) switches it to non-blocking automatically; `into_std` deregisters cleanly and returns a usable `std::net` socket.

## Decisions (human, 2026-11-25)

1. **Bundle `akku::io` into Phase 1:** ship flat `akku::io` (full `std::io` superset including sync `copy` + safe `&mut [u8]` `AsyncRead`, `AsyncWrite`, `AsyncBufRead`, and `Async*Ext` methods `read`/`read_exact`/`copy_to`/`write`/`write_all`/`flush`/`shutdown`) in Phase 1; `AsyncTcpStream` and its split halves implement `AsyncRead` / `AsyncWrite` (`crates/akku/src/io/_dev/DESIGN.md`).
2. **Platform tiers apply to Phase 1 (`AGENTS.md` rule 11):** `akku::net` async TCP ships native backends for all five Tier-1 OSes in Phase 1 with no `Unsupported` stubs — `epoll` on Linux/Android (`net/sys/epoll.rs`), `kqueue` on macOS/iOS (`net/sys/kqueue.rs`), and `WSAPoll` on Windows (`net/sys/wsapoll.rs`, with `IOCP`+`AFD` as a later benchmarked performance follow-up).
3. **`JoinHandle` semantics:** detach on drop, plus explicit `abort()`.
4. **API surface:** expose `Runtime`, `Limits`, `try_block_on`, and `try_spawn`; `spawn` requires `Send + 'static`; calling runtime-dependent functions with no active runtime returns `Err(io::Error)` instead of panicking; dropping `AsyncTcpOwnedWriteHalf` calls `shutdown(Shutdown::Write)` and ignores errors.
5. **Full `std::net` superset & `Async` naming rule (`Namespace.md` §3):** `akku::net` re-exports blocking `std::net::{TcpListener, TcpStream, UdpSocket, Incoming}` and all address/shutdown types by explicit list (never glob). Async socket types take the `Async` prefix (`AsyncTcpListener`, `AsyncTcpStream`, and `AsyncUdpSocket` / `AsyncUnix*` in Phase 2), and TCP split halves take matching prefixed names (`AsyncTcpReadHalf<'a>`, `AsyncTcpWriteHalf<'a>`, `AsyncTcpOwnedReadHalf`, `AsyncTcpOwnedWriteHalf`) for explicit async identity and flat-namespace symmetry with Phase 2 Unix halves.

## G3 clarifications (2026-11-25)

These are clarifications of points §Public API leaves open, pinned by `tests.rs` at G3. They do not change any decision above.

1. The futures returned by `AsyncTcpListener::bind` and `AsyncTcpStream::connect` are `Send` whenever `A: Send`, so they can be passed to `akku::task::spawn`.
2. The no-runtime error has `ErrorKind::Other` and a message that contains `"runtime"`, matching the §Error contracts text.
3. The internal `max_registrations` cap (`65_536`) is not configurable, so the test cannot lower it. The test therefore accepts either of two outcomes: the cap error (`ErrorKind::Other`, with a message that contains `"registration limit"`) at exactly `65_536` registrations, or an earlier OS error (`raw_os_error().is_some()`, for example `EMFILE`). In both cases, registering works again once the streams are dropped. The test takes `FD_GATE` exclusively, so its fd exhaustion cannot fail tests that run in parallel.
4. A connection reset by the peer surfaces as `ConnectionReset` or `ConnectionAborted` on read. On write it surfaces as `BrokenPipe`, `ConnectionReset`, or `ConnectionAborted`, and never as a `SIGPIPE`.

## G4 implementation notes (2026-11-26)

Implementation details that refine §Dependencies and §API rationale 3. They change no decision above.

1. **Layout.** The safe reactor is `net/reactor.rs` (`Reactor`, `Registration`, generational slab, per-direction readiness tick). `net/sys/mod.rs` selects one backend per target. `net/sys/fallback.rs` is safe and has no allowlist entry: on other native targets it parks on `task::Parker` (`Mutex` + `Condvar`), and registering a socket or connecting returns `ErrorKind::Unsupported`.
2. **`sys::Poller` methods all take `&self`.** `wait` locks its own pre-zeroed event buffer, so `Reactor` is `Sync` and one `Arc<Reactor>` serves both the runtime and every socket.
3. **Lazy poller registration.** `Registration::new` only reserves a slab slot, which counts toward the `65_536` cap. The socket joins the OS poller on its first `Pending` wait. `epoll` `EPOLL_CTL_ADD` and `kqueue` `EV_ADD` both report readiness that already exists when the socket is added, so no edge is lost. Sockets that never block make no poller syscalls. This also avoids an XNU cost found on macOS: removing a knote (`EV_DELETE`, or `close`) walks the socket's knote list. With the 65k `try_clone` duplicates of one socket in the registration-limit test, that walk made teardown take more than 60 s. Deregistration still happens before close, but only for sockets that were added to the poller.
4. **Fewer FFI symbols than budgeted.** Non-blocking mode uses safe `std` `set_nonblocking` (`FIONBIO`) on every backend. Windows therefore has no `ioctlsocket` declaration, and Darwin uses `fcntl` only for `F_SETFD FD_CLOEXEC`. The `unsafe` block counts are `epoll.rs` 11, `kqueue.rs` 10, and `wsapoll.rs` 8, each block with a `// SAFETY:` comment.
5. **Wakeup coalescing.** The poller-level `notified` flag is cleared only after the wakeup is drained (`eventfd` read, `EVFILT_USER` retrieval, or the loopback socket drained to `WouldBlock`). On top of that flag, the runtime calls `unpark` only while its driver is parked (`parked` flag, `SeqCst` Dekker check against the run queue and the root `woken` flag).
6. **`WSAPoll` masks.** `POLLHUP`, `POLLERR`, and `POLLNVAL` are reported whatever the requested events are. Each one therefore wakes and disarms both directions, so a hung-up socket cannot spin the poller. A direction is re-armed only when a task waits on it again. `arm` called from another thread during a blocking `WSAPoll` interrupts the wait (`polling` flag).
7. **Connect.** An in-progress connect is registered not-ready. It completes when the socket becomes writable and then checks `take_error()` followed by `peer_addr()`; a `NotConnected` result counts as spurious writability, so readiness is cleared and the connect waits again. Accepted sockets and sockets passed to `from_std` start optimistically ready.
