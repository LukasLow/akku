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
| `crates/akku/src/net/sys/wsapoll.rs` | Windows | `WSAPoll` (armed level-triggered) + loopback TCP pair | 5 (`WSAPoll`, `WSAGetLastError`, `WSASocketW`, `connect`, `SetHandleInformation`; G5) | ~8–10 (9 after G5) |
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
| `WSASocketW(.., WSA_FLAG_OVERLAPPED \| WSA_FLAG_NO_HANDLE_INHERIT)` + `OwnedSocket::from_raw_socket` (G5; was `socket`) | 2 | Return checked `!= INVALID_SOCKET` before adopting into `OwnedSocket`; safely converted via `TcpStream::from(owned_socket)` |
| `SetHandleInformation(HANDLE_FLAG_INHERIT, 0)` (G5, fallback only) | 1 | Socket is open and borrowed; std's `set_no_inherit` does the same |
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
- **`max_io_events_per_tick`**: default `1_024` (from `akku::task::Limits`; bounds the pre-allocated event buffer on `epoll`/`kqueue` to ~12–32 KiB per runtime). Clamped to `1..=65_536` (`MAX_REGISTRATIONS`) in `Reactor::new`, since one turn can never report more ready sources than are registered, which caps the buffer at about 2 MiB.
- **Cooperative budget**: `128` ready socket operations per task poll (and per root poll); see §G5 fixes 1.
- **Shared-listener waiters**: at most `max_tasks + 1` (clamped to `65_537`) distinct wakers per listener direction; see §G5 fixes 2.
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
- **`&mut self` on `AsyncRead`/`AsyncWrite` + `split()`/`into_split()`.** Compile-time enforcement of single reader + single writer per **stream**, so each stream direction keeps one waker slot (`tokio/src/runtime/io/registration.rs:28-33`). This does **not** hold for `AsyncTcpListener`: it is `Sync` and `accept`/`poll_accept` take `&self` (matching std and tokio), so any number of tasks may wait on its read direction. The listener therefore keeps a bounded, deduplicated waker set and wakes all of them on readiness (§G5 fixes 2).
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

## G5 fixes (2026-11-26)

Fixes for the G5 audit (`_dev/AUDIT.md`). No public name was added.

1. **Cooperative budget (CRITICAL).** `Registration::poll_io` and `Registration::poll_ready` first call `crate::task::poll_budget(cx)` and return `Pending` (with a deferred self-wake) once the current poll has spent its `128`-operation budget; every `Ready` they return (data, EOF, `EMFILE`, `EINVAL`, a shutdown error) spends one unit through `crate::task::consume_budget()`. The `WouldBlock` retry inside `poll_io` does not spend budget, and a budget yield never clears readiness, so the next poll still sees it. `AsyncTcpListener::bind` and `AsyncTcpStream::connect` spend one unit up front (`coop_step`), so loops of constructors that fail at once (`EMFILE` from `socket()`, `EADDRINUSE`, an empty address list) also yield. `copy_to` between ready sockets inherits the budget through `poll_read`/`poll_write`. An `accept` loop under `EMFILE` still retries, but fairly: other connections, the root future, and the reactor keep running, so closing handlers can free descriptors.
2. **Shared-listener waiters (MEDIUM).** Each direction keeps a `WakerSet` (one inline slot plus a spill `Vec` that never allocates for streams). Streams register with `Waiters::Single` (a new waker replaces the old one, as before). Listeners register with `Waiters::Shared`: a waker that `will_wake`-matches a parked one is not added again, so each task contributes at most one waker; all parked wakers are woken on readiness; the woken tasks race for the backlog and the losers park again after `WouldBlock`. The set is bounded by `max_tasks + 1` (clamped to `1..=65_537`, passed to `Reactor::new` by the runtime). When a new distinct waker arrives at a full set, all parked wakers are woken early (a spurious wakeup is always safe) instead of being dropped. Parking is `O(waiters)` because of the dedupe scan.
3. **Non-inheritable Windows sockets (MEDIUM).** `connect_nonblocking` creates its socket like std's `Socket::new`: `WSASocketW(af, SOCK_STREAM, IPPROTO_TCP, null, 0, WSA_FLAG_OVERLAPPED | WSA_FLAG_NO_HANDLE_INHERIT)`, and if that fails with `WSAEINVAL` or `WSAEPROTOTYPE` (systems without the flag), `WSASocketW` without it followed by `SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0)`. FFI signatures and constants were checked against std's `library/std/src/sys/pal/windows/c/windows_sys.rs` and `microsoft/windows-rs` (`WSA_FLAG_OVERLAPPED = 0x01`, `WSA_FLAG_NO_HANDLE_INHERIT = 0x80`, `HANDLE_FLAG_INHERIT = 0x01`, `WSAEINVAL = 10022`, `WSAEPROTOTYPE = 10041`, `GROUP`/`DWORD` = `u32`, `BOOL` = `i32`). A `#[cfg(test)]` module in `wsapoll.rs` checks the inherit flag with `GetHandleInformation` (test-only FFI). `wsapoll.rs` now has 9 non-test `unsafe` blocks (1 more in its tests).
4. **LOWs.** `sin6_flowinfo` keeps std's host-order behaviour, with a comment in all three backends. Rustdoc now covers blocking DNS in `bind`/`connect` (`# Blocking`), the Windows 10 2004 / Server 2022 minimum for refused-connect detection (`# Platform support`), fairness (`# Fairness`), and shared accepting (`AsyncTcpListener`, `poll_accept`). The reactor wakes every batch through `crate::task::wake_all`, so one panicking waker cannot drop the others' wakeups.

## Phase 2 G2: API

Research and API design for Phase 2 (`docs/forks/tokio-axum.md` §8 row 2): `AsyncUdpSocket`, async Unix domain sockets, and pre-bind socket options. No code lands at G2. Everything below follows `Namespace.md` §3: the `Async` prefix for async types, inherent async methods on those types (as Phase 1's `accept`/`connect`), `Async*Ext` for byte-stream operations, and no `_async` affix. New non-async names (`SocketOpts`, `TcpKeepalive`, `UnixSocketAddr`) are not defined by `std::net`, so they do not shadow anything.

### P2.1 Scope and non-goals

In scope:

1. `AsyncUdpSocket` on all five Tier-1 OSes. All of its I/O and options go through safe `std::net::UdpSocket`, except on Windows, where every UDP socket also gets `SIO_UDP_CONNRESET`/`SIO_UDP_NETRESET` turned off (P2.4.4).
2. Unix domain sockets on all five Tier-1 OSes (decision Q2, `AGENTS.md` rule 11):
   - `#[cfg(all(any(unix, windows), not(target_arch = "wasm32")))]`: `AsyncUnixListener`, `AsyncUnixStream` (stream subset) with the halves `AsyncUnixReadHalf<'a>`, `AsyncUnixWriteHalf<'a>`, `AsyncUnixOwnedReadHalf`, and `AsyncUnixOwnedWriteHalf`, and `UnixSocketAddr` (pathname and unnamed). Linux, Android, macOS, and iOS use std's `std::os::unix::net`; Windows 10 1803+ uses native `afunix.sys` through `sys/sock_win.rs` (P2.10).
   - `#[cfg(all(unix, not(target_arch = "wasm32")))]` only, because Windows lacks the capability (same precedent as `reuse_port`): `AsyncUnixDatagram`, `AsyncUnixStream::pair`, `from_std`/`into_std` on the Unix types (std has no Windows AF_UNIX type to convert from or to), and the `AsFd`/`AsRawFd` impls. On Windows the stream types implement `AsSocket`/`AsRawSocket` instead.
   - Linux/Android only: abstract names (`UnixSocketAddr::{from_abstract_name, as_abstract_name}`).
3. `SocketOpts` and `TcpKeepalive`, applied through the new constructors `AsyncTcpListener::bind_with`, `AsyncTcpStream::connect_with`, and `AsyncUdpSocket::bind_with`. `AsyncTcpStream` also gets post-creation setters for the options that `std` lacks.
4. `AsyncTcpStream::{peek, poll_peek, try_peek}`, which pull a Phase 1 `_dev/TODO.md` item forward. They use the stable `std::net::TcpStream::peek` (std 1.85 `library/std/src/net/tcp.rs:419`) and need 0 `unsafe`.
5. Two new allowlisted `sys` files: `net/sys/sock_unix.rs` and `net/sys/sock_win.rs`. Orcha approved both as allowlist entries (G2 decisions, 2026-11-26). Each is added with its file: `sock_win.rs` at G3 step 1 for the AF_UNIX probe, `sock_unix.rs` at G4 (P2.4.5). Socket FFI is within hard rule 9. There is no third file: Windows AF_UNIX reuses `sock_win.rs`.

Non-goals (tracked in `_dev/TODO.md` §Phase 2 deferred):

- Ancillary data: fd passing (`SCM_RIGHTS`), `SCM_CREDENTIALS`, and `peer_cred` (`SO_PEERCRED`, `LOCAL_PEERCRED`). `peer_cred` is the first item of a future Phase 2c (decision Q5).
- `peek` on Unix sockets. `std::os::unix::net::UnixStream::peek` and `UnixDatagram::{peek, peek_from}` are unstable in std 1.85 (`unix_socket_peek`, `stream.rs:476`, `datagram.rs:902,934`). See open question 6.
- Truncation reporting through `recvmsg`/`MSG_TRUNC`, `recvmmsg`/`sendmmsg`, GSO/GRO, `IP_PKTINFO`, `SO_BINDTODEVICE`, `IP_TOS`/`IPV6_TCLASS`, source-specific multicast, and UDP "disconnect" (`AF_UNSPEC` connect).
- Linux autobind (binding an empty address), a `bind_with` for Unix sockets (buffer sizes, backlog), and non-zero `SO_LINGER` (P2.5.3).
- Bind-before-connect (choosing a local source address for `connect_with`).
- The Windows `IOCP`+`AFD` reactor, which stays the Phase 1 follow-up.

### P2.2 Public API

All fallible methods return `std::io::Result<T>`. Construction, registration, and `from_std` outside an active `akku::task::Runtime` return `Err(io::Error)` (`ErrorKind::Other`, with a message that contains `"runtime"`), exactly as in Phase 1. Every new type implements `fmt::Debug`, `Send`, and `Sync`, plus `AsFd`/`AsRawFd` on Unix and `AsSocket`/`AsRawSocket` on Windows. `Drop` deregisters from the reactor before the socket closes. Every async method below is cancel-safe: dropping the future loses no datagram, connection, or byte that the OS had already queued.

#### P2.2.1 `SocketOpts` and `TcpKeepalive` (all native targets)

```rust
/// Socket options applied when akku creates a socket, before `bind`/`connect`
/// (and before `listen`). Every option is unset by default; unset options keep
/// the exact behaviour of the plain constructor, so
/// `bind_with(addr, &SocketOpts::new())` == `bind(addr)`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SocketOpts { /* private: Option<_> per option */ }

impl SocketOpts {
    /// All options unset.
    pub const fn new() -> Self;
    /// `SO_REUSEADDR`. Unset: TCP listeners on Unix enable it (std parity,
    /// `sys_common/net.rs:404-412`), everything else leaves it off.
    /// Windows TCP: never set (port-hijack risk); `true` is a documented no-op
    /// because Windows already allows rebinding over `TIME_WAIT`.
    /// Windows UDP: sets `SO_REUSEADDR` (multicast port sharing).
    pub fn reuse_address(self, on: bool) -> Self;
    /// `SO_REUSEPORT` (Linux/Android: load-balanced listeners and UDP;
    /// Darwin: BSD port sharing). Absent on Windows at compile time.
    #[cfg(unix)]
    pub fn reuse_port(self, on: bool) -> Self;
    /// `IPV6_V6ONLY`; applied only to IPv6 addresses, skipped for IPv4 ones.
    /// Unset keeps the OS default (Linux/Darwin dual-stack, Windows v6-only).
    pub fn only_v6(self, on: bool) -> Self;
    /// `TCP_NODELAY` (TCP only).
    pub fn nodelay(self, on: bool) -> Self;
    /// `SO_KEEPALIVE` + idle/interval/retries (TCP only). `None` = off.
    pub fn keepalive(self, keepalive: Option<TcpKeepalive>) -> Self;
    /// `SO_LINGER` with a zero timeout: closing sends `RST` and drops unsent
    /// data (TCP only). Non-zero linger is never offered (P2.5.3).
    pub fn zero_linger(self, on: bool) -> Self;
    /// `SO_RCVBUF` in bytes (`1..=i32::MAX`). Set on the listener before
    /// `listen`, so accepted sockets inherit it and TCP window scaling uses it.
    pub fn recv_buffer_size(self, bytes: usize) -> Self;
    /// `SO_SNDBUF` in bytes (`1..=i32::MAX`).
    pub fn send_buffer_size(self, bytes: usize) -> Self;
    /// `IP_TTL` / unicast hops (`1..=255`), set through safe std `set_ttl`.
    pub fn ttl(self, ttl: u32) -> Self;
    /// `SO_BROADCAST` (UDP only), set through safe std `set_broadcast`.
    pub fn broadcast(self, on: bool) -> Self;
    /// `listen` backlog (TCP listeners only). Unset: `128` (std parity).
    /// Values above `i32::MAX` are clamped; the OS may cap further
    /// (Linux `somaxconn`, Darwin `kern.ipc.somaxconn`).
    pub fn backlog(self, backlog: u32) -> Self;
}

/// TCP keepalive parameters. Unset fields keep the OS default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TcpKeepalive { /* private: Option<Duration>, Option<Duration>, Option<u32> */ }

impl TcpKeepalive {
    pub const fn new() -> Self;
    /// Idle time before the first probe (`TCP_KEEPIDLE`; Darwin `TCP_KEEPALIVE`;
    /// Windows `SIO_KEEPALIVE_VALS`). `1 s..=32_767 s`, rounded up to whole seconds.
    pub fn idle(self, idle: Duration) -> Self;
    /// Interval between probes (`TCP_KEEPINTVL`). `1 s..=32_767 s`.
    pub fn interval(self, interval: Duration) -> Self;
    /// Unanswered probes before the connection drops (`TCP_KEEPCNT`). `1..=127`.
    pub fn retries(self, retries: u32) -> Self;
}
```

New constructors and setters on Phase 1 types:

```rust
impl AsyncTcpListener {
    /// Like `bind`, but creates each socket with `opts` (per resolved address,
    /// first success wins, last error returned). `nodelay`, `keepalive`, and
    /// `zero_linger` are also applied to every accepted stream (P2.3.5).
    pub async fn bind_with<A: ToSocketAddrs>(addr: A, opts: &SocketOpts) -> io::Result<Self>;
}

impl AsyncTcpStream {
    /// Like `connect`, but creates each socket with `opts` before the
    /// non-blocking connect.
    pub async fn connect_with<A: ToSocketAddrs>(addr: A, opts: &SocketOpts) -> io::Result<Self>;

    /// Reads without consuming. `&mut self` keeps the single-reader rule
    /// (one read waker slot, `Waiters::Single`).
    pub async fn peek(&mut self, buf: &mut [u8]) -> io::Result<usize>;
    pub fn poll_peek(&mut self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>>;
    pub fn try_peek(&self, buf: &mut [u8]) -> io::Result<usize>;

    pub fn keepalive(&self) -> io::Result<bool>;
    pub fn set_keepalive(&self, keepalive: Option<TcpKeepalive>) -> io::Result<()>;
    /// `Some(Duration::ZERO)` after `set_zero_linger(true)`, `None` when off.
    pub fn linger(&self) -> io::Result<Option<Duration>>;
    pub fn set_zero_linger(&self, on: bool) -> io::Result<()>;
    pub fn recv_buffer_size(&self) -> io::Result<usize>;
    pub fn set_recv_buffer_size(&self, bytes: usize) -> io::Result<()>;
    pub fn send_buffer_size(&self) -> io::Result<usize>;
    pub fn set_send_buffer_size(&self, bytes: usize) -> io::Result<()>;
}
```

`peek`/`poll_peek`/`try_peek` are also added to `AsyncTcpReadHalf<'_>` and `AsyncTcpOwnedReadHalf`, with `&mut self` for the waiting forms.

#### P2.2.2 `AsyncUdpSocket` (all native targets)

```rust
/// An async UDP socket on the current `akku::task::Runtime`.
///
/// All methods take `&self`: datagram operations are atomic, so one socket
/// may be shared (`Arc`) between any number of sending and receiving tasks.
/// Both directions use shared waiter sets (`Waiters::Shared`, Phase 1 §G5 fix 2).
pub struct AsyncUdpSocket { /* private: Option<std::net::UdpSocket> + Registration */ }

impl AsyncUdpSocket {
    /// Binds to `addr` (each resolved address in order, last error returned).
    pub async fn bind<A: ToSocketAddrs>(addr: A) -> io::Result<Self>;
    pub async fn bind_with<A: ToSocketAddrs>(addr: A, opts: &SocketOpts) -> io::Result<Self>;
    /// Modifies the caller's socket (decision Q8): sets non-blocking mode and,
    /// on Windows, turns off `SIO_UDP_CONNRESET`/`SIO_UDP_NETRESET`. Both
    /// changes stay in effect after `into_std`, and the rustdoc says so.
    pub fn from_std(socket: std::net::UdpSocket) -> io::Result<Self>;
    pub fn into_std(self) -> io::Result<std::net::UdpSocket>;

    /// Sets the default peer and filters incoming datagrams to it. Async only
    /// because `addr` may need DNS (blocking today, async with `akku::dns`).
    pub async fn connect<A: ToSocketAddrs>(&self, addr: A) -> io::Result<()>;

    pub async fn send(&self, buf: &[u8]) -> io::Result<usize>;
    pub async fn recv(&self, buf: &mut [u8]) -> io::Result<usize>;
    /// Sends to the first resolved address (std semantics, `udp.rs:203-208`).
    /// `target` is resolved once, before the poll loop, never per retry.
    /// `# Blocking`: a host name means a blocking `getaddrinfo` for every
    /// datagram on the runtime thread; resolve once and pass a `SocketAddr`.
    pub async fn send_to<A: ToSocketAddrs>(&self, buf: &[u8], target: A) -> io::Result<usize>;
    pub async fn recv_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)>;
    pub async fn peek(&self, buf: &mut [u8]) -> io::Result<usize>;
    pub async fn peek_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)>;

    pub fn poll_send(&self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>>;
    pub fn poll_recv(&self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>>;
    pub fn poll_send_to(&self, cx: &mut Context<'_>, buf: &[u8], target: SocketAddr) -> Poll<io::Result<usize>>;
    pub fn poll_recv_from(&self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<(usize, SocketAddr)>>;
    pub fn poll_peek(&self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>>;
    pub fn poll_peek_from(&self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<(usize, SocketAddr)>>;

    pub fn try_send(&self, buf: &[u8]) -> io::Result<usize>;
    pub fn try_recv(&self, buf: &mut [u8]) -> io::Result<usize>;
    pub fn try_send_to(&self, buf: &[u8], target: SocketAddr) -> io::Result<usize>;
    pub fn try_recv_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)>;
    pub fn try_peek(&self, buf: &mut [u8]) -> io::Result<usize>;
    pub fn try_peek_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)>;

    pub fn local_addr(&self) -> io::Result<SocketAddr>;
    pub fn peer_addr(&self) -> io::Result<SocketAddr>;
    pub fn take_error(&self) -> io::Result<Option<io::Error>>;

    // Delegated 1:1 to std (same signatures as `std::net::UdpSocket`, 0 unsafe).
    pub fn ttl(&self) -> io::Result<u32>;
    pub fn set_ttl(&self, ttl: u32) -> io::Result<()>;
    pub fn broadcast(&self) -> io::Result<bool>;
    pub fn set_broadcast(&self, on: bool) -> io::Result<()>;
    pub fn multicast_loop_v4(&self) -> io::Result<bool>;
    pub fn set_multicast_loop_v4(&self, on: bool) -> io::Result<()>;
    pub fn multicast_ttl_v4(&self) -> io::Result<u32>;
    pub fn set_multicast_ttl_v4(&self, ttl: u32) -> io::Result<()>;
    pub fn multicast_loop_v6(&self) -> io::Result<bool>;
    pub fn set_multicast_loop_v6(&self, on: bool) -> io::Result<()>;
    pub fn join_multicast_v4(&self, multiaddr: &Ipv4Addr, interface: &Ipv4Addr) -> io::Result<()>;
    pub fn join_multicast_v6(&self, multiaddr: &Ipv6Addr, interface: u32) -> io::Result<()>;
    pub fn leave_multicast_v4(&self, multiaddr: &Ipv4Addr, interface: &Ipv4Addr) -> io::Result<()>;
    pub fn leave_multicast_v6(&self, multiaddr: &Ipv6Addr, interface: u32) -> io::Result<()>;

    // Options std lacks (FFI through `sys::sock`).
    pub fn recv_buffer_size(&self) -> io::Result<usize>;
    pub fn set_recv_buffer_size(&self, bytes: usize) -> io::Result<()>;
    pub fn send_buffer_size(&self) -> io::Result<usize>;
    pub fn set_send_buffer_size(&self, bytes: usize) -> io::Result<()>;
}
```

#### P2.2.3 Unix domain sockets

The gates follow decision Q2. `UnixSocketAddr`, `AsyncUnixListener`, `AsyncUnixStream`, and the four halves use `#[cfg(all(any(unix, windows), not(target_arch = "wasm32")))]`. Items marked `#[cfg(unix)]` below exist only on Linux, Android, macOS, and iOS, because Windows AF_UNIX has no such capability (P2.10).

```rust
/// Address of a Unix domain socket: pathname, abstract (Linux/Android), or
/// unnamed. On Unix a thin newtype over `std::os::unix::net::SocketAddr`; on
/// Windows it owns its path bytes (std has no Windows AF_UNIX address type).
/// The public API is the same on both.
#[derive(Clone)]
pub struct UnixSocketAddr { /* private: Unix: std::os::unix::net::SocketAddr; Windows: kind + Vec<u8> */ }

impl UnixSocketAddr {
    /// `InvalidInput` for an empty path, a path with any NUL byte, or a path
    /// of `sun_path` length or more (107 usable bytes on Linux/Android and
    /// Windows, 103 on Darwin). On Windows the path must be valid UTF-8
    /// (`afunix` takes a UTF-8 `sun_path`), else `InvalidInput`.
    pub fn from_pathname<P: AsRef<Path>>(path: P) -> io::Result<Self>;
    /// Abstract namespace name (no filesystem entry, no permission bits).
    /// `InvalidInput` above 107 bytes. NUL bytes inside the name are allowed.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    pub fn from_abstract_name<N: AsRef<[u8]>>(name: N) -> io::Result<Self>;
    pub fn as_pathname(&self) -> Option<&Path>;
    #[cfg(any(target_os = "linux", target_os = "android"))]
    pub fn as_abstract_name(&self) -> Option<&[u8]>;
    pub fn is_unnamed(&self) -> bool;
}
// Also: fmt::Debug (std's format), PartialEq + Eq (by kind and bytes).
// #[cfg(unix)] only (decision Q3): From<std::os::unix::net::SocketAddr>,
// From<UnixSocketAddr> for std::os::unix::net::SocketAddr.

/// Async Unix stream listener. `accept`/`poll_accept` take `&self` with a
/// shared waiter set, exactly like `AsyncTcpListener`.
pub struct AsyncUnixListener { /* private */ }

impl AsyncUnixListener {
    /// Binds and listens (Unix: std's backlog, `-1` = `somaxconn` on Linux and
    /// 128 on Darwin, `listener.rs:72-108`; Windows: `listen(128)`).
    /// Synchronous: no DNS, no blocking.
    /// Unlinks the socket file on drop by default (P2.3.4).
    pub fn bind<P: AsRef<Path>>(path: P) -> io::Result<Self>;
    pub fn bind_addr(addr: &UnixSocketAddr) -> io::Result<Self>;
    pub async fn accept(&self) -> io::Result<(AsyncUnixStream, UnixSocketAddr)>;
    pub fn poll_accept(&self, cx: &mut Context<'_>) -> Poll<io::Result<(AsyncUnixStream, UnixSocketAddr)>>;
    /// Unlink-on-drop starts disabled for adopted sockets. Modifies the caller's
    /// socket (non-blocking; Darwin `SO_NOSIGPIPE`; decision Q8).
    #[cfg(unix)]
    pub fn from_std(listener: std::os::unix::net::UnixListener) -> io::Result<Self>;
    /// Disarms unlink-on-drop: the file now belongs to the std listener.
    #[cfg(unix)]
    pub fn into_std(self) -> io::Result<std::os::unix::net::UnixListener>;
    pub fn local_addr(&self) -> io::Result<UnixSocketAddr>;
    pub fn take_error(&self) -> io::Result<Option<io::Error>>;
    /// Enabling records the current file identity; `InvalidInput` when the
    /// address is not a pathname.
    pub fn set_unlink_on_drop(&self, unlink: bool) -> io::Result<()>;
}

/// Async Unix stream; implements `akku::io::{AsyncRead, AsyncWrite}`.
pub struct AsyncUnixStream { /* private */ }

impl AsyncUnixStream {
    /// Connect that never blocks the runtime (P2.3.2); waits for writability
    /// when the OS reports in-progress, then checks `take_error`.
    /// `# Errors`: `WouldBlock` means "listener backlog full" (Linux `EAGAIN`,
    /// decision Q14); Darwin reports `ConnectionRefused` for the same case.
    pub async fn connect<P: AsRef<Path>>(path: P) -> io::Result<Self>;
    pub async fn connect_addr(addr: &UnixSocketAddr) -> io::Result<Self>;
    /// Connected unnamed pair (std `UnixStream::pair` + `set_nonblocking`).
    /// No `socketpair` on Windows.
    #[cfg(unix)]
    pub fn pair() -> io::Result<(Self, Self)>;
    /// Modifies the caller's socket (non-blocking; Darwin `SO_NOSIGPIPE`; decision Q8).
    #[cfg(unix)]
    pub fn from_std(stream: std::os::unix::net::UnixStream) -> io::Result<Self>;
    #[cfg(unix)]
    pub fn into_std(self) -> io::Result<std::os::unix::net::UnixStream>;

    pub fn poll_read(&mut self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>>;
    pub fn poll_write(&mut self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>>;
    pub fn poll_shutdown(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>>;
    pub fn try_read(&self, buf: &mut [u8]) -> io::Result<usize>;
    pub fn try_write(&self, buf: &[u8]) -> io::Result<usize>;
    /// Same reason for the name as `AsyncTcpStream::shutdown_how`.
    pub fn shutdown_how(&self, how: Shutdown) -> io::Result<()>;

    pub fn split(&mut self) -> (AsyncUnixReadHalf<'_>, AsyncUnixWriteHalf<'_>);
    pub fn into_split(self) -> (AsyncUnixOwnedReadHalf, AsyncUnixOwnedWriteHalf);

    pub fn local_addr(&self) -> io::Result<UnixSocketAddr>;
    pub fn peer_addr(&self) -> io::Result<UnixSocketAddr>;
    pub fn take_error(&self) -> io::Result<Option<io::Error>>;
}

// Halves: same method sets and trait impls as the `AsyncTcp*Half` types
// (`poll_read`/`try_read`/`local_addr`/`peer_addr` on read halves;
// `poll_write`/`poll_shutdown`/`try_write`/`local_addr`/`peer_addr` on write halves).
// Dropping `AsyncUnixOwnedWriteHalf` calls `shutdown(Shutdown::Write)`, ignoring errors.
pub struct AsyncUnixReadHalf<'a> { /* &'a AsyncUnixStream */ }
pub struct AsyncUnixWriteHalf<'a> { /* &'a AsyncUnixStream */ }
pub struct AsyncUnixOwnedReadHalf { /* Arc<AsyncUnixStream> */ }
pub struct AsyncUnixOwnedWriteHalf { /* Arc<AsyncUnixStream> */ }

/// Async Unix datagram socket. All methods take `&self` (shared waiters).
/// `#[cfg(unix)]` only: Windows `afunix.sys` has no `SOCK_DGRAM`.
#[cfg(unix)]
pub struct AsyncUnixDatagram { /* private */ }

impl AsyncUnixDatagram {
    /// Unlinks the socket file on drop by default (P2.3.4).
    pub fn bind<P: AsRef<Path>>(path: P) -> io::Result<Self>;
    pub fn bind_addr(addr: &UnixSocketAddr) -> io::Result<Self>;
    pub fn unbound() -> io::Result<Self>;
    pub fn pair() -> io::Result<(Self, Self)>;
    /// Synchronous: a Unix datagram connect never blocks and needs no DNS.
    pub fn connect<P: AsRef<Path>>(&self, path: P) -> io::Result<()>;
    pub fn connect_addr(&self, addr: &UnixSocketAddr) -> io::Result<()>;
    /// Modifies the caller's socket (non-blocking; Darwin `SO_NOSIGPIPE`; decision Q8).
    pub fn from_std(socket: std::os::unix::net::UnixDatagram) -> io::Result<Self>;
    pub fn into_std(self) -> io::Result<std::os::unix::net::UnixDatagram>;

    pub async fn send(&self, buf: &[u8]) -> io::Result<usize>;
    pub async fn recv(&self, buf: &mut [u8]) -> io::Result<usize>;
    pub async fn send_to<P: AsRef<Path>>(&self, buf: &[u8], path: P) -> io::Result<usize>;
    pub async fn send_to_addr(&self, buf: &[u8], addr: &UnixSocketAddr) -> io::Result<usize>;
    pub async fn recv_from(&self, buf: &mut [u8]) -> io::Result<(usize, UnixSocketAddr)>;

    pub fn poll_send(&self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>>;
    pub fn poll_recv(&self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>>;
    pub fn poll_send_to_addr(&self, cx: &mut Context<'_>, buf: &[u8], addr: &UnixSocketAddr) -> Poll<io::Result<usize>>;
    pub fn poll_recv_from(&self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<(usize, UnixSocketAddr)>>;
    pub fn try_send(&self, buf: &[u8]) -> io::Result<usize>;
    pub fn try_recv(&self, buf: &mut [u8]) -> io::Result<usize>;
    pub fn try_send_to_addr(&self, buf: &[u8], addr: &UnixSocketAddr) -> io::Result<usize>;
    pub fn try_recv_from(&self, buf: &mut [u8]) -> io::Result<(usize, UnixSocketAddr)>;

    pub fn local_addr(&self) -> io::Result<UnixSocketAddr>;
    pub fn peer_addr(&self) -> io::Result<UnixSocketAddr>;
    pub fn take_error(&self) -> io::Result<Option<io::Error>>;
    pub fn shutdown_how(&self, how: Shutdown) -> io::Result<()>;
    pub fn set_unlink_on_drop(&self, unlink: bool) -> io::Result<()>;
}
```

The module rustdoc overview table gets these rows: "Async UDP", "Async Unix stream server/connection (Unix, Windows 10 1803+)", "Async Unix datagram (Unix)", and "Socket options".

### P2.3 Semantics and edge cases

#### P2.3.1 Datagrams (UDP and Unix datagram)

- **One operation is one datagram.** `send` and `send_to` either send the whole buffer as one datagram or fail (`EMSGSIZE` when it is larger than the protocol or OS limit). Datagram sends are never clamped to `MAX_IO_LEN`, because clamping would silently truncate the datagram. Receive buffers are still clamped to `MAX_IO_LEN` (1 GiB), which is harmless because no datagram is that large.
- **No zero-length shortcut.** The Phase 1 guard that returns `Ok(0)` for `&[]` / `&mut []` applies to byte streams only. For datagram sockets, `send(&[])` sends an empty datagram, and `Ok(0)` from `recv` or `recv_from` never means EOF. `recv(&mut [])` passes the OS result through, which differs per OS (decision Q9):
  - Linux/Android: `Ok(0)`, and one datagram is consumed (empty or not).
  - Windows: an empty datagram gives `Ok(0)`; a non-empty one gives `Err` with `raw_os_error() == Some(10040)` (`WSAEMSGSIZE`), and the datagram is discarded.
  - Darwin: GUESS `Ok(0)` with the datagram consumed, as on Linux. Tests 2 and 25 pin it.
- **Truncated datagrams follow std and the OS, with no normalization.** On Linux, Android, macOS, and iOS, a datagram longer than the buffer fills the buffer, returns `Ok(buf.len())`, and the rest is discarded. On Windows, the buffer is filled, the call returns `Err` with `raw_os_error() == Some(10040)` (`WSAEMSGSIZE`), and the rest is discarded. std returns that error unchanged, and tokio documents the same behaviour (`tokio/src/net/udp.rs:1530-1534`). In both cases the next receive gets the next datagram. The reactor keeps readiness on that error, and the level-triggered `WSAPoll` re-arms. The docs recommend 65,536-byte buffers. See open question 9.
- **`peek`/`peek_from` (UDP only)** return the next datagram without consuming it. They do not clear readiness on success, so a `recv` after a `peek` never waits for a new edge. The same truncation rules apply. On Windows, peeking an oversized datagram also returns `WSAEMSGSIZE`, but the datagram stays queued.
- **Connected UDP.** `connect` fixes the default peer, and the OS then drops datagrams from other sources. `send`/`recv` require a prior `connect` (otherwise `NotConnected`, or the OS's `EDESTADDRREQ`). On Linux and Darwin, an ICMP port-unreachable makes the next `send`/`recv` on a connected socket fail with `ConnectionRefused`. On Windows this report is turned off (P2.4.4), so a connected socket just keeps waiting there. Tests accept both outcomes, and the rustdoc says that the refused report is best-effort. `send_to` to another address on a connected socket is passed through to the OS. GUESS: Linux and Windows accept it, and Darwin returns `EISCONN`.
- **Unconnected UDP servers on Windows** no longer see `WSAECONNRESET`/`WSAENETRESET` on `recv_from` after one of their earlier `send_to`s triggered an ICMP error. Otherwise a remote host could stop a `while let Ok(..) = sock.recv_from(..)` loop at will (Go issues 5834 and 68614, `golang/go@d695060f` `src/net/fd_windows.go:58-75`).
- **OS datagram limits** are passed through: UDP over IPv4 carries at most 65,507 payload bytes and IPv6 65,527. macOS defaults to `net.inet.udp.maxdgram = 9216`, so larger UDP sends fail with `EMSGSIZE` there, and Unix datagrams are limited to `net.local.dgram.maxdgram = 2048`. These defaults were read with `sysctl` on the macOS 27.2 dev host.
- **Unix datagram backpressure (missing item b).** The OSes differ when the receiver's queue is full, and the rustdoc documents both behaviours:
  - Darwin fails at once with `ENOBUFS`. akku passes it through unchanged, so callers see a real `Err`, never a retry.
  - Linux returns `EAGAIN`. akku treats that as `WouldBlock` and waits, both for connected `send` and for unconnected `send_to`.
  - GUESS: on Linux, an unconnected `send_to` to a full receiver may never get an `EPOLLOUT` edge when the receiver drains, because the sender is not on the receiver's peer-wait queue.

  P2.8 test 28 pins this with a watchdog. If it hangs, G4 handles it as follows: `poll_send_to`/`send_to_addr` on an **unconnected** socket return `EAGAIN` to the caller as `Err(WouldBlock)` instead of waiting. The rustdoc then says "receiver queue full; retry later". Connected `send` keeps waiting, because Linux does wake the sender through `peer_wait` for connected sockets.
- **Unnamed senders.** `recv_from` from an `unbound()` or `pair()` peer returns an address with `is_unnamed() == true`. On Linux the kernel reports length 0, and std maps that to unnamed (`addr.rs:103-128`).

#### P2.3.2 Unix streams

- **The connect never blocks the runtime, so std's blocking connect is not used on Linux/Android.** `std::os::unix::net::UnixStream::connect` blocks while the listener's backlog is full. On Linux that lasts until an `accept` or `SO_SNDTIMEO` (infinite by default), so a local peer that stops accepting would freeze the whole single-threaded runtime. Instead, akku creates `socket(AF_UNIX, SOCK_STREAM | SOCK_NONBLOCK | SOCK_CLOEXEC)` itself and calls `connect`. `0` means connected. `EINPROGRESS` means wait for writability and then call `take_error` (mio accepts it, `mio/src/sys/unix/uds/stream.rs:8-21`). Any other error is returned unchanged. This is the tokio/mio behaviour; retrying with a timeout needs Phase 3 timers.
- **The full-backlog rule (decision Q14).** On Linux, `EAGAIN` from an AF_UNIX `connect` is returned to the caller directly, as `Err` with `ErrorKind::WouldBlock` and `raw_os_error() == Some(11)`. It never enters `Registration::poll_io`/`try_io`, which would treat `WouldBlock` as "wait for readiness" and could wait forever. The rustdoc on `connect`/`connect_addr` says `WouldBlock` means "listener backlog full". Darwin reports the same condition as `ECONNREFUSED`.
- **Darwin connect (optional, missing item g).** GUESS: XNU never blocks a Unix stream connect, because a full backlog returns `ECONNREFUSED` at once. G4 may then use the safe std `UnixStream::connect` followed by `set_nonblocking` and `SO_NOSIGPIPE` on Darwin, instead of the FFI path (`socket` + `fcntl(FD_CLOEXEC)` + `SO_NOSIGPIPE` + `connect`). Test 19 decides: if a full-backlog std connect returns promptly on `macos-latest`, the safe path is allowed.
- **Windows connect** uses `WSASocketW(AF_UNIX, SOCK_STREAM, 0, .., WSA_FLAG_OVERLAPPED | WSA_FLAG_NO_HANDLE_INHERIT)`, then non-blocking mode, then `connect` with `RawSockAddr::Unix`. `WSAEWOULDBLOCK` means wait for writability and then check `take_error`; any other error is returned (P2.10).
- **SIGPIPE never fires, and the hardening is mandatory (decision Q7).** std writes Unix streams with plain `write(2)` (`stream.rs:632-635` → `sys/pal/unix/net.rs:354-356`) and connected datagrams with `write(2)` (`datagram.rs:579-581`). Without `MSG_NOSIGNAL`, a write to a closed peer raises `SIGPIPE`. Rust's startup code sets `SIGPIPE` to ignored only in Rust binaries. **Android JNI `cdylib`s and iOS `staticlib`s never run that startup code**, and neither does akku inside any C host, or a binary built with `-Zon-broken-pipe=kill`. There, the first write to a closed peer kills the app.
  - Linux and Android: stream writes and connected-datagram `send` use `send(fd, buf, len, MSG_NOSIGNAL)`.
  - Darwin: every Unix socket akku registers gets `SO_NOSIGPIPE = 1`, which covers accepted sockets, `pair()`, `from_std`, and akku's connect. std sets `SO_NOSIGPIPE` only in `Socket::new_raw` (`sys/pal/unix/net.rs:104-108`), not in `new_pair` or `accept` (`:116-145`, `:245-270`).
  - std's `send_to`/`send_to_addr` already pass `MSG_NOSIGNAL` (`datagram.rs:508-519,545-553`).
  - GUESS: Linux `unix_dgram` sends may never raise `SIGPIPE` at all, because they return `ECONNREFUSED` or `EPIPE` without the signal. Test 27 pins this; `MSG_NOSIGNAL` stays either way.
  - Windows has no `SIGPIPE`.

  The same decision covers the Phase 1 TCP gap: on Darwin, accepted and `from_std` TCP streams also get `SO_NOSIGPIPE`.
- **Mobile pass-through notes (rustdoc, missing item d).** On iOS, sockets become *defunct* when the app is suspended, and the next operation fails with `ENOTCONN`/`EBADF`-class errors (GUESS on the exact errno). akku passes these through, and the rustdoc tells apps to reconnect on resume. On Android, SELinux may deny `bind`/`connect` on filesystem paths outside the app's data directory with `EACCES` (`PermissionDenied`). akku passes that through, and the rustdoc suggests the app's private directory or an abstract name.
- **Half-close, `into_split`, drop shutdown, and EOF** match Phase 1 TCP exactly, on Windows too (`shutdown(SD_SEND)` through std).

#### P2.3.3 Unix addresses

- **Pathname length.** `sun_path` is `[u8; 108]` on Linux and Android and `[u8; 104]` on Darwin (`libc/src/unix/linux_like/mod.rs:216-219`, `libc/src/unix/bsd/mod.rs:165-170`). std rejects a path of `sun_path.len()` bytes or more with `InvalidInput` (`addr.rs:39-44`), so 107 and 103 bytes are usable. The limit counts the bytes as passed, so a relative path counts relative to the current directory. On iOS the app container paths are long, and the rustdoc says so. Windows `SOCKADDR_UN` also has `sun_path: [i8; 108]` (`afunix.h`, `UNIX_PATH_MAX = 108`). akku applies the same 107-byte rule there to the path's UTF-8 bytes, and rejects non-UTF-8 paths with `InvalidInput`.
- **Empty path, interior or leading NUL.** akku rejects these with `InvalidInput` before any syscall. Empty is rejected because Linux would autobind to a random abstract name (`len == sizeof(sa_family_t)`) and Darwin would return `EINVAL`. NUL bytes are rejected because std rejects them (`addr.rs:32-37`), and akku has no tokio-style leading-NUL shortcut (`tokio/src/net/unix/listener.rs:80-91`); abstract names go through `UnixSocketAddr::from_abstract_name` explicitly.
- **Abstract names (Linux/Android only, `#[cfg]`, no runtime stub).** They have no filesystem entry, are never unlinked, and are not covered by permissions: any process in the same network namespace can connect. The rustdoc says this under `# Security`. Android apps use them widely.
- **Abstract names on Windows.** `afunix.sys` accepts abstract addresses, but akku keeps them Linux/Android only by decision, so they are not exposed on Windows.
- **Unnamed** comes from `pair()`, `unbound()`, and the peer address of accepted connections from unbound clients. On Windows it is the peer address of accepted connections, and of a client socket that was never bound.
- **Permissions.** A socket file is created with the process umask. akku does not chmod after bind, because that would race. The docs suggest a private parent directory.

#### P2.3.4 Unlink-on-drop policy (listener and bound datagram)

The default is **on** for sockets that akku binds to a pathname (`bind`, `bind_addr`). This follows Go, which unlinks only listeners it created (`golang/go@d695060f` `src/net/unixsock_posix.go:190-196,207-216,230`), and it removes the most common footgun, a stale file that makes the next start fail with `EADDRINUSE`. std and tokio never unlink. The rules:

1. At bind time, akku stores `std::path::absolute(path)` (stable since 1.79), so a later `chdir` cannot redirect the unlink. Right after `bind`, it also stores the file identity from `std::fs::symlink_metadata`. All of this is safe std:
   - Unix: `(dev, ino)` from `std::os::unix::fs::MetadataExt`.
   - Windows: `std::os::windows::fs::MetadataExt::creation_time()`, plus the requirement that `file_attributes()` contains `FILE_ATTRIBUTE_REPARSE_POINT` (`0x400`). The AF_UNIX socket file is an NTFS reparse point.
2. On `Drop`, the order is **deregister, unlink, close**. Unlinking before close is Go's order (decision Q4): a new server may bind the path the moment the old socket closes, and its file must never be deleted. akku looks up `symlink_metadata(path)`. It calls `std::fs::remove_file` only if the entry still matches the recorded identity, and it ignores errors:
   - Unix: `FileTypeExt::is_socket` and the same `(dev, ino)`.
   - Windows: still a reparse point with the same `creation_time`.

   A path that another process has since replaced or rebound is never deleted. The window between the check and the removal is a documented TOCTOU of microseconds.
3. Abstract and unnamed addresses are never unlinked. `from_std` starts with unlink off, because akku does not know who owns the file. `set_unlink_on_drop(true)` on such a socket records the identity at that point.
4. `into_std` (Unix only) disarms the unlink. Unlinking also happens if a panic unwinds through the owner (the drop runs), but not on `process::exit` or abort.
5. `bind` never deletes an existing file. Removing a stale file is left to the caller, and the documented pattern is "connect → `ConnectionRefused` → `remove_file` → `bind`".

See open question 4.

#### P2.3.5 Socket options

- `bind_with`/`connect_with` validate every option first, without touching any socket, and return `InvalidInput` for:
  - out-of-range values: keepalive idle or interval outside `1 s..=32_767 s` (Linux `MAX_TCP_KEEPIDLE`/`MAX_TCP_KEEPINTVL`, applied on every OS for uniform behaviour), retries outside `1..=127` (Linux `MAX_TCP_KEEPCNT`), buffer sizes outside `1..=i32::MAX`, and TTL outside `1..=255`;
  - options that do not apply to the protocol: `nodelay`/`keepalive`/`zero_linger`/`backlog` on UDP, `broadcast` on TCP, and `backlog` on `connect_with`. Failing loudly is chosen over silently ignoring them (open question 15).
- Per resolved address: `socket()` (non-inheritable: `SOCK_CLOEXEC`, Darwin `FD_CLOEXEC`, Windows `WSA_FLAG_NO_HANDLE_INHERIT`), then pre-bind options via FFI (`SO_REUSEADDR`, `SO_REUSEPORT`, `IPV6_V6ONLY` for V6 only, `SO_RCVBUF`, `SO_SNDBUF`), then options that also work through std after the fd is wrapped (`set_ttl`, `set_nodelay`, `set_broadcast`), then keepalive and linger via FFI, then `bind` and, for listeners, `listen(backlog)` or, for streams, the non-blocking `connect`. The first address that succeeds wins, and otherwise the last error is returned (std `each_addr` semantics).
- **How listener options reach accepted streams (decision Q15).**
  - Re-applied explicitly to every accepted stream: `nodelay`, `keepalive` (all fields), `zero_linger`, and, on Darwin, `SO_NOSIGPIPE`. Linux inherits these, but Darwin inheritance is a GUESS, so akku does not rely on it.
  - Inherited, never re-applied: `recv_buffer_size` and `send_buffer_size`. They must be set before `listen` to affect window scaling, and setting them again after the handshake changes nothing useful.
  - `ttl` is inherited on Linux, Darwin, and Windows; std's `set_ttl` on the listener sets `IP_TTL` on the listening socket, and accepted sockets copy it. GUESS on Darwin, which test 35 pins. If test 35 shows Darwin does not inherit it, `ttl` moves to the re-applied list.
- **Accept-path failures (missing item a).** If re-applying an option to an accepted socket fails, akku closes that connection and keeps accepting. Examples: `EINVAL` on macOS when the peer already reset the socket, for `TCP_NODELAY`, `SO_KEEPALIVE`, `SO_LINGER`, or `SO_NOSIGPIPE`. `poll_accept` **never** returns that error: it drops the socket, spends one budget unit, and continues with the next queued connection, or waits. A rate-limited `log` line is deferred until `akku::log` integration exists. Test 35b covers this where it is feasible.
- Getters read back what the OS reports. Linux doubles `SO_RCVBUF`/`SO_SNDBUF` and caps them at `rmem_max`/`wmem_max`; Darwin returns `ENOBUFS` above `kern.ipc.maxsockbuf` (8 MiB on the dev host). These are passed through.

### P2.4 Platform behaviour and syscall table

#### P2.4.1 Operation → mechanism

| Operation | Linux / Android | macOS / iOS | Windows |
|---|---|---|---|
| UDP `bind`, `connect`, `send*`, `recv*`, `peek*`, multicast, broadcast, TTL | safe `std::net::UdpSocket` | safe std | safe std |
| UDP: disable ICMP reset reporting | — | — | `WSAIoctl(SIO_UDP_CONNRESET, FALSE)` + `WSAIoctl(SIO_UDP_NETRESET, FALSE)` |
| TCP `peek` | safe std `TcpStream::peek` | safe std | safe std |
| Unix listener `bind` | safe std `UnixListener` | safe std | `WSASocketW(AF_UNIX)` + `bind` + `listen` (existing `sock_win.rs` sites, `RawSockAddr::Unix`) |
| Unix listener `accept` | safe std | safe std | `accept` with `SOCKADDR_UN` out (**new site**), then `OwnedSocket` adoption (existing site) |
| Unix `local_addr` / `peer_addr` | safe std | safe std | `getsockname` / `getpeername` with `SOCKADDR_UN` out (**2 new sites**) |
| Unix stream `connect` | `socket` + `connect` (FFI) | FFI `socket` + `fcntl` + `setsockopt(SO_NOSIGPIPE)` + `connect`, or safe std `connect` + `set_nonblocking` if test 19 allows (missing item g) | `WSASocketW(AF_UNIX)` + `connect` (existing sites) |
| Unix stream `read`, `shutdown`, `set_nonblocking`, `take_error` | safe std | safe std | safe std through an internal `std::net::TcpStream::from(OwnedSocket)`, which is only an I/O carrier; its IP address methods are never called |
| Unix stream `write` | `send(MSG_NOSIGNAL)` (FFI) | safe std `write` + `SO_NOSIGPIPE` at registration | safe std (`TcpStream` carrier; no `SIGPIPE` on Windows) |
| Unix stream `pair` | safe std | safe std | — (`#[cfg(unix)]`, no `socketpair`) |
| Unix datagram (all) | safe std `UnixDatagram`; connected `send` via `send(MSG_NOSIGNAL)` | safe std | — (`#[cfg(unix)]`, no `SOCK_DGRAM`) |
| Unlink-on-drop | safe `std::fs` (`dev`/`ino`) | safe `std::fs` (`dev`/`ino`) | safe `std::fs` (reparse point + `creation_time`) |
| `SocketOpts` socket creation | `socket(.. \| SOCK_NONBLOCK \| SOCK_CLOEXEC)` (FFI) | `socket` + `fcntl(F_SETFD)` + `SO_NOSIGPIPE` (FFI) | `WSASocketW(.., WSA_FLAG_OVERLAPPED \| WSA_FLAG_NO_HANDLE_INHERIT)` (+ `SetHandleInformation` fallback) |
| `SO_REUSEADDR`, `SO_REUSEPORT`, `IPV6_V6ONLY`, `SO_RCVBUF`, `SO_SNDBUF`, `SO_KEEPALIVE`, `SO_LINGER` | `setsockopt`/`getsockopt` | `setsockopt`/`getsockopt` (`SO_LINGER_SEC`) | `setsockopt`/`getsockopt` (`SO_REUSEPORT` absent) |
| Keepalive idle / interval / retries | `TCP_KEEPIDLE` / `TCP_KEEPINTVL` / `TCP_KEEPCNT` | `TCP_KEEPALIVE` / `TCP_KEEPINTVL` / `TCP_KEEPCNT` | `WSAIoctl(SIO_KEEPALIVE_VALS)` (ms) + `setsockopt(TCP_KEEPCNT)` (Windows 10 1703+) |
| `bind`, `listen` for `bind_with` | FFI | FFI | FFI |
| Non-blocking TCP connect (Phase 1, consolidated, open question 10) | FFI | FFI | FFI |
| `nodelay`, `ttl`, `broadcast` in `SocketOpts` | safe std on the wrapped fd | safe std | safe std |

#### P2.4.2 Constants (verified against `rust-lang/libc@1e106cd95539f5faf40687bc9c380b1185d52715`, `windows-sys 0.61.2`, and std `windows_sys.rs` at `rust-lang/rust@bc9132daa5c0be572ebb81304f6039d947298e67`)

| Constant | Linux / Android¹ | Darwin (macOS / iOS) | Windows |
|---|---:|---:|---:|
| `AF_UNIX` | 1 | 1 (`AF_LOCAL`) | 1 |
| `SOCK_STREAM` / `SOCK_DGRAM` | 1 / 2 | 1 / 2 | 1 / 2 |
| `SOL_SOCKET` | 1 | `0xffff` | `0xffff` |
| `SO_REUSEADDR` | 2 | `0x0004` | 4 |
| `SO_REUSEPORT` | 15 | `0x0200` | — |
| `SO_KEEPALIVE` | 9 | `0x0008` | 8 |
| `SO_BROADCAST` | 6 | `0x0020` | 32 (std path) |
| `SO_LINGER` | 13 | `SO_LINGER_SEC = 0x1080` (socket2 `sys/unix.rs:226-233`) | 128 |
| `SO_SNDBUF` / `SO_RCVBUF` | 7 / 8 | `0x1001` / `0x1002` | `0x1001` / `0x1002` |
| `SO_NOSIGPIPE` | — | `0x1022` | — |
| `IPPROTO_TCP` / `IPPROTO_UDP` / `IPPROTO_IPV6` | 6 / 17 / 41 | 6 / 17 / 41 | 6 / 17 / 41 |
| `IPV6_V6ONLY` | 26 | 27 | 27 |
| `TCP_KEEPIDLE` (Darwin `TCP_KEEPALIVE`) | 4 | `0x10` | `3` (unused; `SIO_KEEPALIVE_VALS` instead) |
| `TCP_KEEPINTVL` / `TCP_KEEPCNT` | 5 / 6 | `0x101` / `0x102` | 17 / 16 |
| `MSG_NOSIGNAL` | `0x4000` | (unused) | — |
| `F_SETFD` / `FD_CLOEXEC` | — | 2 / 1 | — |
| `SOCK_NONBLOCK` / `SOCK_CLOEXEC` | `0x800` / `0x80000` | — | — |
| `EINPROGRESS` / `EAGAIN` / `EMSGSIZE` / `ECONNREFUSED` | 115 / 11 / 90 / 111 | 36 / 35 / 40 / 61 | `WSAEWOULDBLOCK` 10035 / `WSAEMSGSIZE` 10040 / `WSAECONNRESET` 10054 / `WSAENETRESET` 10052 |
| `SIO_KEEPALIVE_VALS` | — | — | `0x9800_0004` |
| `SIO_UDP_CONNRESET` / `SIO_UDP_NETRESET` | — | — | `0x9800_000C` / `0x9800_000F` |
| `WSA_FLAG_OVERLAPPED` / `WSA_FLAG_NO_HANDLE_INHERIT` / `HANDLE_FLAG_INHERIT` | — | — | `0x01` / `0x80` / `0x01` |

¹ These are the asm-generic values, used by every architecture that `sys/epoll.rs` gates on (`x86_64`, `aarch64`, `x86`, `arm`, `riscv64`). MIPS, SPARC, and PowerPC differ and stay on the `Unsupported` fallback (Phase 1 §Dependencies). Sources: `src/unix/linux_like/android/mod.rs:1115-1176`, `src/unix/linux_like/mod.rs:716-1015`, `src/unix/mod.rs:389-392`, `src/unix/bsd/apple/mod.rs:2345-3036`, `src/unix/bsd/mod.rs:279,304,423`, `windows-sys-0.61.2/src/Windows/Win32/Networking/WinSock/mod.rs:4074,4115-4116,4704-4707`.

#### P2.4.3 FFI declarations and structs: `net/sys/sock_unix.rs`

Target gate: the union of the Phase 1 `epoll` and `kqueue` gates. Inside the file, `#[cfg(any(target_os = "linux", target_os = "android"))]` and `#[cfg(any(target_os = "macos", target_os = "ios"))]` select the constants, structs, and the two OS-only symbols.

```rust
// SAFETY: every declaration matches the C ABI on all gated targets
// (`socklen_t` is a 32-bit integer on glibc, musl, bionic, and Darwin; bionic
// declares it `int` on 32-bit, same width); callers uphold pointer and
// descriptor preconditions at each call site.
unsafe extern "C" {
    fn socket(domain: c_int, ty: c_int, protocol: c_int) -> c_int;
    fn bind(fd: c_int, addr: *const c_void, len: u32) -> c_int;
    fn listen(fd: c_int, backlog: c_int) -> c_int;
    fn connect(fd: c_int, addr: *const c_void, len: u32) -> c_int;
    fn setsockopt(fd: c_int, level: c_int, name: c_int, val: *const c_void, len: u32) -> c_int;
    fn getsockopt(fd: c_int, level: c_int, name: c_int, val: *mut c_void, len: *mut u32) -> c_int;
    #[cfg(any(target_os = "linux", target_os = "android"))]
    fn send(fd: c_int, buf: *const c_void, len: usize, flags: c_int) -> isize;
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    fn fcntl(fd: c_int, cmd: c_int, ...) -> c_int;
}

// Linux / Android
#[repr(C)] struct SockAddrUn { sun_family: u16, sun_path: [u8; 108] }      // size 110, sun_path @ 2
// Darwin
#[repr(C)] struct SockAddrUn { sun_len: u8, sun_family: u8, sun_path: [u8; 104] } // size 106, sun_path @ 2
#[repr(C)] struct Linger { l_onoff: c_int, l_linger: c_int }               // size 8 (both)
// SockAddrIn / SockAddrIn6 move here unchanged from epoll.rs / kqueue.rs (open question 10).
```

Compile-time `size_of`/`offset_of` asserts cover every struct per OS, as in Phase 1. `SockAddrUn` is built with safe code (`sun_path[..n].copy_from_slice(bytes)` from `UnixSocketAddr::as_pathname`/`as_abstract_name`, with an offset of 1 for abstract names). The length is `2 + n + 1` for a pathname and `2 + 1 + n` for an abstract name (mio `uds/mod.rs:33-86`, std `addr.rs:53-57,266-282`); Darwin also sets `sun_len`. A sealed private trait `SockOptValue` (implemented only for `c_int` and `Linger`) gives one generic `setsockopt` site and one generic `getsockopt` site. The `getsockopt` output is a zeroed `T` (no `MaybeUninit`), and a returned `len` other than `size_of::<T>()` is rejected with `InvalidData`. One private `RawSockAddr` enum (`V4`, `V6`, `Unix`) has a safe `as_ptr_len()`, so `bind` and `connect` are one `unsafe` site each.

**`unsafe` budget: 9 sites** (8 compiled per OS):

| Site | Count | Invariant |
|---|---:|---|
| `socket` | 1 | integer arguments only; return checked `>= 0` |
| `OwnedFd::from_raw_fd` | 1 | fresh descriptor, sole owner |
| `fcntl(F_SETFD, FD_CLOEXEC)` (Darwin) | 1 | fd borrowed and open; `c_int` vararg |
| `setsockopt::<T: SockOptValue>` | 1 | `&T` valid for `size_of::<T>()` |
| `getsockopt::<T: SockOptValue>` | 1 | zeroed `T`, writable, `len` in/out initialized |
| `bind` | 1 | `RawSockAddr` initialized, exact length |
| `listen` | 1 | integer arguments only |
| `connect` | 1 | same as `bind` |
| `send(MSG_NOSIGNAL)` (Linux/Android) | 1 | slice pointer and length from a valid `&[u8]` (≤ `MAX_IO_LEN`) |
| *Test-only* (`#[cfg(test)]`): `signal(SIGPIPE, SIG_DFL)` in the SIGPIPE child probe | 1 | test helper, like Phase 1's `GetHandleInformation` |

The test-only `fcntl(F_GETFD)` from the first draft is dropped (decision Q7). The CLOEXEC check is safe instead: the test spawns a child via `std::process::Command` and has it test `/dev/fd/N` for the parent's descriptor number (Darwin, Linux). Linux can additionally read `/proc/self/fdinfo/N` (`flags:` field, `O_CLOEXEC` = `02000000`).

#### P2.4.4 FFI declarations and structs: `net/sys/sock_win.rs`

Target gate: the Phase 1 `wsapoll` gate. WSAStartup has already run, because every path first calls `current_reactor()`, whose `WSAPoll` wake pair is built with std sockets (Phase 1 Backend 3).

```rust
// SAFETY: signatures match `winsock2.h` / `mstcpip.h` as declared by std's
// `windows_sys.rs` and windows-sys 0.61.2 (`SOCKET` = `usize`, `int` = `i32`,
// `DWORD` = `u32`; the nullable completion-routine pointer is declared as an
// untyped pointer and always null).
#[link(name = "ws2_32")]
unsafe extern "system" {
    fn WSASocketW(af: i32, ty: i32, protocol: i32, protocol_info: *const c_void, group: u32, flags: u32) -> usize;
    fn bind(s: usize, name: *const c_void, namelen: i32) -> i32;
    fn listen(s: usize, backlog: i32) -> i32;
    fn connect(s: usize, name: *const c_void, namelen: i32) -> i32;
    fn setsockopt(s: usize, level: i32, optname: i32, optval: *const c_void, optlen: i32) -> i32;
    fn getsockopt(s: usize, level: i32, optname: i32, optval: *mut c_void, optlen: *mut i32) -> i32;
    fn WSAIoctl(s: usize, code: u32, in_buf: *const c_void, in_len: u32, out_buf: *mut c_void,
                out_len: u32, bytes_returned: *mut u32, overlapped: *mut c_void, completion: *const c_void) -> i32;
    fn WSAGetLastError() -> i32;
    // Windows AF_UNIX only (decision Q2): the three new sites.
    fn accept(s: usize, addr: *mut c_void, addrlen: *mut i32) -> usize;
    fn getsockname(s: usize, name: *mut c_void, namelen: *mut i32) -> i32;
    fn getpeername(s: usize, name: *mut c_void, namelen: *mut i32) -> i32;
}
#[link(name = "kernel32")]
unsafe extern "system" {
    fn SetHandleInformation(handle: *mut c_void, mask: u32, flags: u32) -> i32;
}

#[repr(C)] struct Linger { l_onoff: u16, l_linger: u16 }                          // size 4 (`LINGER`, windows-sys :2403-2406)
#[repr(C)] struct TcpKeepaliveVals { onoff: u32, keepalivetime: u32, keepaliveinterval: u32 } // size 12 (mstcpip `tcp_keepalive`)
#[repr(C)] struct SockAddrUn { sun_family: u16, sun_path: [u8; 108] }            // size 110 (`SOCKADDR_UN`, windows-sys :4329; afunix.h)
```

**Windows AF_UNIX on `sock_win.rs` (decision Q2).** The AF_UNIX stream types reuse the existing sites:
- `WSASocketW(AF_UNIX = 1, SOCK_STREAM, 0, ..)` with the same non-inheritable flags and fallback;
- `bind`, `listen`, and `connect` through a new `RawSockAddr::Unix(SockAddrUn, i32)` variant. Its length is `2 + n + 1` for a pathname, as on Linux;
- `OwnedSocket::from_raw_socket` adoption.

After adoption, the socket is wrapped in an internal `std::net::TcpStream::from(OwnedSocket)`, used only as a family-agnostic I/O carrier. `read`, `write`, `shutdown`, `set_nonblocking`, and `take_error` are plain Winsock `recv`/`send`/`shutdown`/`ioctlsocket(FIONBIO)`/`getsockopt(SO_ERROR)` calls in std, so they are safe and need no FFI. The carrier's `local_addr`/`peer_addr`/`nodelay`/`ttl` are never called: they would parse IP addresses or set TCP-level options. Only three sites are new:
- `accept` (output `SockAddrUn`, length in/out);
- `getsockname` and `getpeername` (output `SockAddrUn`).

The returned length is validated: it must be ≥ 2 and ≤ `size_of::<SockAddrUn>()`, the family must be `AF_UNIX`, and the path bytes are copied out by safe slicing. There is no third allowlist file.

`getsockopt` on Windows may write only 1 byte for `BOOL`-typed options (socket2 `sys/windows.rs:112-140`). The wrapper therefore reads into a zeroed `i32` and accepts a returned length of 1 or 4. UDP sockets get `SIO_UDP_CONNRESET` and `SIO_UDP_NETRESET` set to `FALSE` (`u32` 0) in `AsyncUdpSocket::{bind, bind_with, from_std}`, and any error there is returned (Go returns it as well). Keepalive follows socket2 `sys/windows.rs:811-842`: `SIO_KEEPALIVE_VALS` with milliseconds (`onoff = 1`; an unset idle or interval keeps the Windows defaults of 2 h and 1 s), then `TCP_KEEPCNT` if retries are set.

**`unsafe` budget: 13–14 sites** (10–11 for sockets and options, plus 3 for AF_UNIX)

| Site | Count | Invariant |
|---|---:|---|
| `WSASocketW` (with the flag; retry without it on `WSAEINVAL`/`WSAEPROTOTYPE`; shared by TCP, UDP, and AF_UNIX) | 1–2 | integer arguments and a null protocol-info pointer; return checked `!= INVALID_SOCKET` |
| `accept` (AF_UNIX) | 1 | listener borrowed and open; `&mut SockAddrUn` zeroed and writable for the in/out `i32` length; result checked `!= INVALID_SOCKET` and adopted at once |
| `getsockname` / `getpeername` (AF_UNIX) | 2 | socket borrowed and open; zeroed `&mut SockAddrUn` with an exact in/out length |
| `OwnedSocket::from_raw_socket` | 1 | fresh socket, sole owner |
| `SetHandleInformation` (fallback only) | 1 | open socket handle |
| `setsockopt::<T>` / `getsockopt::<T>` | 2 | as on Unix; `getsockopt` accepts 1- or 4-byte `BOOL` results |
| `bind` / `listen` / `connect` | 3 | as on Unix |
| `WSAIoctl::<T>` (input-only) | 1 | `&T` valid for `size_of::<T>()`; no output buffer; overlapped and completion null |
| `WSAGetLastError` | 1 | no preconditions |
| *Test-only*: `GetHandleInformation` (moved from `wsapoll.rs`) | 1 | test helper |

#### P2.4.5 Module-wide `unsafe` totals and allowlist

Phase 1 today has 30 sites: `epoll.rs` 11, `kqueue.rs` 10, `wsapoll.rs` 9. If the TCP `connect_nonblocking` moves into `sock_*` (open question 10), `epoll.rs` drops to about 7 (removing `socket`, `from_raw_fd`, and 2 × `connect`), `kqueue.rs` to about 5 (removing `socket`, `from_raw_fd`, `SO_NOSIGPIPE`, and 2 × `connect`), and `wsapoll.rs` to about 2 (`WSAPoll` and `WSAGetLastError`).

Recount after the G2 decisions:

| File | Sites |
|---|---:|
| `epoll.rs` | ~7 |
| `kqueue.rs` | ~5 |
| `wsapoll.rs` | ~2 |
| `sock_unix.rs` | 9 (8 per OS) |
| `sock_win.rs` | 13–14 |
| **Total** | **~36–37** |

That is about 6–7 more than today, within the plan's estimate of 10–15 extra. Without the consolidation it would be about 52. Test-only sites (`signal`, `GetHandleInformation`) are not counted.

`.github/unsafe-allowlist.txt` gains exactly these two paths. **Orcha approved both entries at G2** (2026-11-26, Orcha + independent Opus review): socket FFI is within hard rule 9. Each entry is added in the commit that creates its file: `sock_win.rs` at G3 step 1 (for the probe), `sock_unix.rs` at G4. The Checker still reviews every site:

```text
crates/akku/src/net/sys/sock_unix.rs
crates/akku/src/net/sys/sock_win.rs
```

The fallback (`net/sys/fallback.rs`) gains safe `sock` stubs returning `ErrorKind::Unsupported` for non-Tier-1 native targets. It stays off the allowlist.

### P2.5 API rationale

1. **`SocketOpts` is a value, not a socket handle (tokio `TcpSocket`/socket2 `Socket` style).** A value holds no OS resource, it is `Clone + Eq + Default`, and it can be reused for every resolved address. That keeps `ToSocketAddrs` multi-address fallback working: a `TcpSocket::new_v4()` handle fixes the family before resolution, so it cannot. It is also one obvious way (`CONCEPT.md` §8): there are only the constructors `bind`/`bind_with` and `connect`/`connect_with`. **`AsyncTcpBuilder` is therefore not added** (open question 1).
2. **`bind_with(addr, &SocketOpts::new())` behaves exactly like `bind(addr)`**, including Unix `SO_REUSEADDR` on TCP listeners, backlog 128, and the OS default for `IPV6_V6ONLY`. That invariant is directly testable.
3. **Only zero linger.** A non-zero `SO_LINGER` makes `close()` block the thread for up to the linger time, and that thread is the single-threaded runtime thread. tokio 1.54 deprecated `set_linger` for this reason (`tokio/src/net/tcp/stream.rs:1334`) and added `set_zero_linger` (`tokio/src/net/tcp/socket.rs:462`).
4. **`reuse_port` is `#[cfg(unix)]`.** Windows has no `SO_REUSEPORT`, and a compile-time absence (like std's `std::os::*` extensions) is not a runtime `Unsupported` stub. tokio (`tcp/socket.rs:301`) and socket2 (`sys/unix.rs:2247`) gate it the same way. On Windows, TCP `reuse_address` never sets `SO_REUSEADDR`, which would allow port hijacking (std `sys_common/net.rs:404-412`, Go `src/net/sockopt_windows.go:27-35`). UDP does set it, for multicast port sharing (Go `sockopt_windows.go:37-41`; BSD also needs `SO_REUSEPORT`, Go `sockopt_bsd.go:46-56`).
5. **UDP methods take `&self` with shared waiters; `AsyncTcpStream::peek` takes `&mut self`.** A UDP socket is normally shared between tasks. TCP keeps its single-reader read slot (`Waiters::Single`), so a waiting `peek(&self)` from two tasks would overwrite each other's waker and lose a wakeup, the same bug class as the Phase 1 G5 listener finding. `try_peek(&self)` never waits, so it stays `&self`.
6. **The UDP `connect`/`send_to` are async but the Unix datagram `connect` is sync.** UDP resolves `ToSocketAddrs`, which will be async DNS later, so the async signature avoids a breaking change. A Unix path needs no resolution, and a Unix datagram connect never blocks (tokio uses the same split: `udp.rs:348`, `unix/datagram/socket.rs:611`). Unix `bind` is sync for the same reason, while a Unix stream connect is async because it may wait for writability (P2.3.2).
7. **`UnixSocketAddr` newtype rather than `std::os::unix::net::SocketAddr`.** std's type cannot be re-exported in `akku::net` (it would clash with `std::net::SocketAddr`). Returning it fully qualified would tie the signature to `cfg(unix)` for good. A newtype costs nothing, and with Windows AF_UNIX in this PR it gives one type on all five OSes (tokio has the same newtype, `tokio/src/net/unix/socketaddr.rs`). On Windows it owns its path bytes. The `From` conversions to and from std's type are `#[cfg(unix)]` (decision Q3).
8. **Blocking Unix types are not re-exported in `akku::net`.** `akku::net` is the `std::net` superset. `std::os::unix::net::*` belongs to the future `akku::os::unix` (`Namespace.md` §OS layer), and `from_std`/`into_std` name the std paths (open question 13).
9. **Same reactor, no reactor change.** Every new type uses the Phase 1 `Registration` (`poll_io`, `try_io`, cooperative budget, shared and single waiters, lazy poller add, deregister before close). Datagram `poll_*` methods are `poll_io` with the zero-length shortcut and the send clamp skipped.

### P2.6 Seams

| Seam | Catches | Misses |
|---|---|---|
| `AsyncUdpSocket` send/recv/peek (connected and not), on IPv4/IPv6 loopback | readiness loss with shared waiters, wrong zero-length handling, truncation mishandling, Windows ICMP-reset DoS | WAN loss, fragmentation |
| `AsyncUnix{Listener,Stream}` + halves (+ `pair` on Unix), on all five OSes | blocking connect, SIGPIPE, unlink bugs, `sockaddr_un` length bugs (`sun_len`, abstract offset), Windows `afunix` + `WSAPoll` readiness, `SOCKADDR_UN` parsing in `accept`/`getsockname`/`getpeername` | Windows versions before 1803 (unsupported) |
| `AsyncUnixDatagram` bind/unbound/pair/send_to/recv_from | unnamed-address mapping, macOS `maxdgram`, backpressure hang | ancillary data |
| `UnixSocketAddr` constructors | path length, NUL, empty-path autobind, abstract name round-trip | — |
| `SocketOpts` via `bind_with`/`connect_with` + `AsyncTcpStream` setters + getters | wrong constant or level per OS, pre-bind ordering, Windows hijack, accepted-stream application, range validation | kernel sysctl caps (asserted loosely) |
| `sys/sock_{unix,win}.rs` layout asserts + test-only FFI probes | struct ABI drift, inheritable handles | mobile runtime (compile-checked only, as in Phase 1) |

### P2.7 DoS and limits

| Limit | Default / value | Enforcement |
|---|---|---|
| Registrations per reactor (all socket kinds together) | `65_536` | Phase 1 slab cap |
| Waiters per direction on shared sockets (UDP, Unix datagram, listeners) | `max_tasks + 1`, clamped to `65_537` | Phase 1 §G5 fix 2 |
| Cooperative budget | `128` ready operations per task poll, each datagram counting as one | inherited from `poll_io`; a datagram flood cannot starve sibling tasks |
| Datagram send size | not clamped; OS `EMSGSIZE` (UDP 65,507/65,527; macOS UDP 9,216 and Unix datagram 2,048) | passed through |
| Receive slice | `≤ 1 << 30` | Phase 1 `MAX_IO_LEN` |
| Unix path | 107 (Linux/Android/Windows) / 103 (Darwin) bytes; no NUL; not empty; UTF-8 on Windows | `InvalidInput` before any syscall |
| Abstract name | ≤ 107 bytes | `InvalidInput` (std `addr.rs:266-282`) |
| Keepalive idle / interval | `1 s..=32_767 s` (rounded up to whole seconds; Windows milliseconds) | `InvalidInput` |
| Keepalive retries | `1..=127` | `InvalidInput` |
| `SO_RCVBUF` / `SO_SNDBUF` | `1..=i32::MAX`; OS caps passed through | `InvalidInput` / OS error |
| TTL in `SocketOpts` | `1..=255` | `InvalidInput` (std `set_ttl` stays pass-through) |
| TCP backlog | `128` default; `u32` clamped to `i32::MAX`; OS caps silently | clamp |
| Unix listener backlog | std's (`-1`/`somaxconn` on Linux, 128 on Darwin); Windows `128` | std / `listen(128)` |
| Windows UDP ICMP resets | disabled | `SIO_UDP_CONNRESET`/`SIO_UDP_NETRESET` |
| Linux multicast memberships | OS `igmp_max_memberships` (default 20) | passed through |
| Unix listener backlog full (client) | fails at once (`EAGAIN`/`ECONNREFUSED`), never blocks the runtime | non-blocking connect |
| Unlink-on-drop | removes only the same socket file (Unix `(dev, ino)`, Windows reparse point + `creation_time`), before close | identity check |
| Accept-path option failure | drops that one connection, keeps accepting | never surfaced from `accept` (missing item a) |

### P2.8 Test plan (adversarial, G3)

All tests run on `smd` (musl), `ubuntu-latest`, `macos-latest`, and `windows-latest`. The Unix-only ones are `#[cfg(unix)]`, and the Windows AF_UNIX ones are `#[cfg(windows)]`. Tests that may hang use a watchdog thread (10 s), as in the Phase 1 G5 probes. Datagram payloads stay ≤ 2,048 bytes unless a test targets the limits.

**G3 step 1: the Windows `WSAPoll` + AF_UNIX probe (decision Q2, a stop condition).** Before any other G3 test is written, a probe test runs on `windows-latest`. It creates a listener and a client with the planned `sock_win.rs` FFI calls, switches both to non-blocking mode, and checks three things. The probe lives in `sys/sock_win.rs` (`#[cfg(test)]`), so that file and its approved allowlist entry are created at G3 step 1 rather than at G4:
- `WSAPoll` reports `POLLRDNORM` on the listener when a client connects;
- it reports `POLLWRNORM` on the client after a non-blocking `connect`;
- it reports `POLLRDNORM` on a stream with pending data.

**If any check fails, the Worker stops and reports to Orcha.** The human then decides whether an exception from rule 11 applies. The Worker does not decide this or work around it.

**UDP**
1. IPv4 and IPv6 loopback `send_to`/`recv_from` echo; `connect` + `send`/`recv`; `peer_addr` before `connect` is `NotConnected`; `send` before `connect` is an `Err`.
2. Zero-length datagrams (per OS, decision Q9). `send_to(&[])` arrives as `Ok((0, sender))` on every OS, is not EOF, and the next datagram still arrives. `recv(&mut [])` on a **non-empty** datagram behaves per OS:
   - Linux/Android: `Ok(0)`, and the datagram is consumed (the next `recv` gets the following one).
   - Windows: `Err` with `raw_os_error() == Some(10040)`, and the datagram is discarded.
   - Darwin: GUESS `Ok(0)` with the datagram consumed; the test records the actual behaviour and the rustdoc is fixed to match.
3. Truncation: send 1,000 bytes, receive into 10 bytes. Unix: `Ok(10)` with the correct prefix. Windows: `Err` with `raw_os_error() == Some(10040)` and the prefix filled. On every OS, the following `recv` returns the next datagram, and nothing hangs.
4. `peek` and `peek_from` do not consume; the following `recv` returns the same bytes without waiting; `peek` followed by an awaited `recv` does not hang (edge-triggered readiness is kept).
5. Connected filtering: after `connect(A)`, a datagram from B is never returned by `recv`.
6. Connected refused: `connect` to a closed port, then `send` and `recv`. Linux/macOS return `ConnectionRefused`; Windows keeps waiting (the watchdog cancels via `try_recv` → `WouldBlock`). No OS spins the CPU.
7. ICMP-reset immunity: an unconnected server `send_to`s a closed port, and its next `recv_from` still gets a live peer's datagram (no `ConnectionReset`). This is the main check on Windows.
8. Eight tasks share an `Arc<AsyncUdpSocket>`, all wait in `recv_from`, and eight datagrams arrive: all eight tasks finish (shared waiters, no lost wakeup).
9. Flood fairness: 10,000 datagrams while a sibling task counts `yield_now` turns; the sibling makes progress (budget).
10. Size limits: 70,000 bytes gives `EMSGSIZE` everywhere (never truncated); 9,217 bytes gives `EMSGSIZE` on macOS only.
11. Multicast and broadcast setters round-trip through getters; joining a unicast address as a group is an `Err`; `leave` without `join` is an `Err`.
12. `from_std`/`into_std` keep the socket usable; outside a runtime the constructors return `Err`; a 500-socket bind/drop loop shows no slab leak.

**Unix streams**
13. Pathname bind/connect/accept echo, `split`, `into_split`, half-close, and EOF after dropping the owned write half.
14. `pair()` echo; `local_addr().is_unnamed()`.
15. Linux/Android abstract names: bind and connect through `from_abstract_name`; no file is created; a name with embedded NUL bytes round-trips through `as_abstract_name`.
16. Path limits: the maximum (107/103 bytes) succeeds, one more byte is `InvalidInput`; an interior NUL, a leading NUL, and an empty path are `InvalidInput`, and no socket is created (fd count unchanged).
17. Unlink-on-drop: drop removes the file; `set_unlink_on_drop(false)` keeps it; rename the file away and rebind the path with a second listener, then drop the first: the second listener's file survives (identity check); `into_std` keeps the file; abstract addresses touch nothing; binding over an existing file is `AddrInUse` and the file is not deleted.
18. Connecting to a missing path is `NotFound`; connecting to a stale socket file is `ConnectionRefused`.
19. Full backlog: a listener that never accepts, plus clients connecting in a loop, must produce an `Err` within the watchdog time and never block the runtime:
    - Linux: `WouldBlock`, with `raw_os_error() == Some(11)`. The test also asserts that the error comes back without the future ever returning `Pending` (decision Q14).
    - Darwin: `ConnectionRefused`.

    A sibling task keeps running. On Darwin the test also runs std `UnixStream::connect` against the full backlog: if it returns promptly, the safe Darwin connect path (missing item g) is allowed.
20. SIGPIPE: the test binary re-executes itself (`current_exe` + an env var) as a child that sets `SIGPIPE` to `SIG_DFL` (test-only `signal()`, kept by decision Q7) and writes to a stream whose peer was dropped. The child must see `BrokenPipe` and exit 0, not be killed by signal 13. The same check covers `pair()` and accepted streams, and on Darwin also accepted TCP streams. On Darwin the test also asserts `SO_NOSIGPIPE == 1` through the production `getsockopt` wrapper on accepted, `pair()`, and `from_std` Unix sockets, and on accepted and `from_std` TCP streams.
21. CLOEXEC, checked safely without `fcntl` (decision Q7). On Unix, a `std::process::Command` child is spawned and must not see the parent's descriptor via `/dev/fd/N`; Linux also reads `/proc/self/fdinfo/N` for `O_CLOEXEC`. On Windows the test uses the test-only `GetHandleInformation`. The test covers:
    - sockets from Unix `connect`;
    - `AsyncTcpStream::connect_with` and `AsyncTcpListener::bind_with`;
    - `AsyncUdpSocket::bind_with` (missing item f);
    - streams accepted by `AsyncUnixListener` (missing item f; std `accept4(SOCK_CLOEXEC)` on Linux, `FD_CLOEXEC` on Darwin).
22. Backpressure: a 4–16 MiB `write_all` over a Unix stream (macOS buffers are only 8 KiB) arrives byte-exact.
23. Drop-cancelling `connect` and `accept` futures, and a loop of 300 connect/accept/drop cycles, leak nothing.
23b. Unlink order: drop a listener while a second thread loops on `bind(path)`. Once the second bind succeeds, its socket file must exist and stay (decision Q4: akku unlinks before close).

**Windows AF_UNIX streams (`#[cfg(windows)]`, after the G3 step 1 probe)**
W1. Echo: `AsyncUnixListener::bind` on a temporary path, `AsyncUnixStream::connect`, round trip with `write_all`/`read_exact`, then `split` and `into_split`. `local_addr().as_pathname()` equals the bound path, and the accepted stream's `peer_addr().is_unnamed()`.
W2. Half-close: the client `shutdown()`s its write side, the server reads to EOF (`Ok(0)`), and then the server writes a reply that the client reads in full. Dropping `AsyncUnixOwnedWriteHalf` gives the peer EOF.
W3. Unlink-on-drop: drop removes the reparse-point file; `set_unlink_on_drop(false)` keeps it; a replaced path (different `creation_time`) is not removed; binding over an existing file is `AddrInUse` and never deletes it.
W4. Full backlog: a listener that never accepts, plus clients connecting in a loop. Each connect must finish (`Ok` while the backlog has room, then an `Err`) within the watchdog time, and never block the runtime. A sibling task keeps running. The exact error is recorded (GUESS: `WSAECONNREFUSED`).
W5. Path validation: 107 UTF-8 bytes succeed, 108 bytes, a NUL byte, an empty path, and non-UTF-8 input (`OsString` from unpaired UTF-16) are `InvalidInput`; no socket is created.
W6. A 4 MiB `write_all` arrives byte-exact (no lost `POLLWRNORM` wakeup), and an idle connected pair parks without spinning `WSAPoll`.

**Unix datagrams**
24. `bind`, `unbound`, and `pair`; `send_to` a path; `recv_from` from an unbound sender is unnamed; `connect` + `send`/`recv`.
25. Zero-length and truncated datagrams (per OS, decision Q9). `send(&[])` arrives as `Ok(0)`. `recv(&mut [])` on a non-empty datagram: Linux/Android `Ok(0)` with the datagram consumed; Darwin GUESS, same as Linux and recorded by the test. Truncation follows the Unix semantics of test 3.
26. Datagrams above 2,048 bytes give `EMSGSIZE` on macOS and are delivered on Linux.
27. Connected send after the peer was dropped is an `Err` (`ConnectionRefused` or `NotConnected`) and never a SIGPIPE (child probe). The test also records whether Linux `unix_dgram` raises `SIGPIPE` without `MSG_NOSIGNAL` (GUESS: never; decision Q7). `MSG_NOSIGNAL` stays in either case.
28. Receiver-full backpressure (missing item b). Darwin: an unconnected `send_to` returns `Err` (`ENOBUFS`) at once. Linux: an unconnected `send_to` to a full receiver either finishes after the receiver drains or, if the edge never comes, the G4 rule from P2.3.1 applies, and the call returns `Err(WouldBlock)` instead of waiting. Linux connected `send` finishes after the drain. Nothing hangs past the watchdog.
29. Eight tasks share an `Arc<AsyncUnixDatagram>` and all `recv` (shared waiters).

**`SocketOpts`**
30. `bind_with(addr, &SocketOpts::new())` and `bind(addr)` report the same `reuse_address`, backlog behaviour, and `only_v6`.
31. Unix `reuse_port`, checked per OS (decision Q11). Without the option, the second bind is `AddrInUse` everywhere.
    - Linux/Android: two listeners with `reuse_port` on the same port both bind, and both accept connections, since the kernel load-balances. The test connects until each listener has accepted at least one connection; it does not assert how they are spread.
    - Darwin: only that both binds succeed. `SO_REUSEPORT` does not load-balance there, so which listener accepts is not asserted.
32. Hijack guard: a second bind to an active listener's port with `reuse_address(true)` is `AddrInUse` on every OS, including Windows.
33. A server whose closed connections leave `TIME_WAIT` can rebind with default options (all OSes).
34. `only_v6(true)` on `[::]:0` refuses IPv4 connections; `only_v6(false)` accepts IPv4-mapped connections. The test is skipped when IPv6 is unavailable.
35. `nodelay`, `keepalive` (all three fields), and `zero_linger` round-trip through getters on a `connect_with` stream and on streams accepted from a `bind_with` listener. The test also asserts which options are inherited versus re-applied (decision Q15): `ttl` on accepted streams equals the listener's, and `send_buffer_size` on accepted streams is at least the listener's value (pins the Darwin GUESS).
35b. Accept-path failure (missing item a, where feasible): a client connects to a `bind_with(nodelay, keepalive, zero_linger)` listener and resets at once (`zero_linger` + drop) before the server accepts. Then a second, healthy client connects. `accept` must return the healthy connection and never surface the `EINVAL`. The test cannot force the error on every OS, so it asserts only that `accept` never fails because of the reset peer.
36. `zero_linger`: after the stream drops, the peer's read returns `ConnectionReset`, not `Ok(0)`.
37. Range validation: idle 0 s or 32,768 s, retries 0 or 128, buffer size 0 or `i32::MAX as usize + 1`, TTL 0 or 256 are all `InvalidInput`, and no socket is created. `backlog(u32::MAX)` works.
38. Options that do not apply: UDP with `nodelay`, TCP with `broadcast`, and `connect_with` with `backlog` are `InvalidInput`.
39. Buffer sizes: 64 KiB reads back as at least 64 KiB (Linux doubles it); a size above `kern.ipc.maxsockbuf` on macOS is an `Err`, never a panic.
40. `connect_with` falls back across addresses: a closed IPv6 address followed by an open IPv4 address connects.
41. Windows UDP `reuse_address(true)`: two sockets share one multicast port; Unix uses `reuse_address` + `reuse_port`.

**TCP `peek` (missing item e)**
42. `peek` then an awaited `read` returns the same bytes and does not hang. A successful `peek` must not clear edge-triggered readiness (epoll `EPOLLET`, kqueue `EV_CLEAR`) or the `WSAPoll` arm state.
43. `peek` after the peer's `shutdown(Write)` with nothing buffered returns `Ok(0)` (EOF), and stays `Ok(0)` on repeated calls; `try_peek` on an idle stream is `WouldBlock` and clears readiness so a later `peek` waits.
44. `peek`/`poll_peek`/`try_peek` on `AsyncTcpReadHalf` and `AsyncTcpOwnedReadHalf`: peek through the half while the write half sends concurrently from another task; no lost wakeup.

### P2.9 References (pinned)

- **std 1.85.0** (`rust-lang/rust` tag `1.85.0`): `library/std/src/os/unix/net/addr.rs:25-59,103-128,197,233-252,266-282`; `listener.rs:72-108,137-152`; `stream.rs:82,116,476,632-635`; `datagram.rs:98,214,508-519,545-553,579-581,902,934`; `library/std/src/net/udp.rs:172,203-208,677,699,767`; `library/std/src/net/tcp.rs:419`; `library/std/src/sys_common/net.rs:36-46,404-435,555-563`; `library/std/src/sys/pal/unix/net.rs:71-145,245-270,354-356`.
- **tokio 1.54.1**: `src/net/udp.rs:150,227,348,570,790,1185,1326,1530-1534,1569,1711,2251-2278`; `src/net/unix/socketaddr.rs` (newtype); `src/net/unix/listener.rs:76-112`; `src/net/unix/stream.rs:81-127,913,1017-1032`; `src/net/unix/datagram/socket.rs:395,433,568,611`; `src/net/unix/split_owned.rs:107,261,268`; `src/net/tcp/socket.rs:126-906`; `src/net/tcp/stream.rs:1334-1339`.
- **mio 1.2.5**: `src/sys/unix/uds/mod.rs:33-86,89-141`; `src/sys/unix/uds/stream.rs:8-21`; `src/sys/unix/net.rs:15-60`; `src/sys/unix/udp.rs:13-37`.
- **socket2 0.6.5**: `src/socket.rs:913-967,1065-1160,2060-2090,2234-2356`; `src/sys/unix.rs:220-245,315-348,1276-1330,2210-2281`; `src/sys/windows.rs:26,71,87-140,337-418,811-842`.
- **libc** `rust-lang/libc@1e106cd95539f5faf40687bc9c380b1185d52715`: as in P2.4.2, plus `src/unix/mod.rs:198-201` (`linger`) and `src/unix/bsd/apple/mod.rs:711-716,2971-2973` (`xucred`, `LOCAL_PEERCRED`/`LOCAL_PEERPID`, deferred).
- **Windows**: `windows-sys 0.61.2` `src/Windows/Win32/Networking/WinSock/mod.rs:116,590,2403-2406,4074,4115-4116,4329,4540,4704-4707,5240`; `microsoft/windows-rs@0a952b0ef7e37801ff624b91d314768c727fd276` `crates/libs/sys/src/Windows/Win32/winsock2/mod.rs:80,144-178,612-635`, `.../mstcpip/mod.rs:185,481-485`, `.../afunix/mod.rs` (`SOCKADDR_UN`, `sun_path: [i8; 108]`, `UNIX_PATH_MAX = 108`); Microsoft Command Line blog "AF_UNIX comes to Windows" (2017-12-19, <https://devblogs.microsoft.com/commandline/af_unix-comes-to-windows/>): stream sockets only, pathname, abstract, and unnamed addresses, no autobind, no `socketpair`, no ancillary data.
- **Go** `golang/go@d695060f5be23f2dd86d21d861a292dc2d183cab`: `src/net/sockopt_windows.go:12-41`, `src/net/sockopt_bsd.go:15-57`, `src/net/fd_windows.go:54-75`, `src/net/unixsock_posix.go:190-196,207-216,230`, `src/internal/syscall/windows/types_windows.go:19`, `src/syscall/types_windows.go:626-632`.
- **macOS 27.2 dev host** (`sysctl`): `net.inet.udp.maxdgram: 9216`, `net.local.dgram.maxdgram: 2048`, `net.local.stream.{send,recv}space: 8192`, `kern.ipc.somaxconn: 128`, `kern.ipc.maxsockbuf: 8388608`.

### P2.10 Windows AF_UNIX (rule 11): decided, ships in this PR

**Decision Q2 (Orcha + independent Opus review, 2026-11-26).** Windows AF_UNIX streams ship in Phase 2, not in a Phase 2b. The first draft's recommendation and assessment below are kept for the record; the decision replaces them as follows:

- `#[cfg(all(any(unix, windows), not(target_arch = "wasm32")))]`: `AsyncUnixListener`, `AsyncUnixStream` (stream subset), the four halves, and `UnixSocketAddr` (pathname and unnamed).
- `#[cfg(unix)]` only, because Windows lacks the capability (the `reuse_port` precedent): `AsyncUnixDatagram`, `pair()`, `from_std`/`into_std`, and the `AsFd`/`AsRawFd` impls. Abstract names stay Linux/Android only.
- Implementation, in `sys/sock_win.rs` only, with no third allowlist file:
  - Reused sites: `WSASocketW(AF_UNIX)`, `bind`, `listen`, `connect` via `RawSockAddr::Unix`, and `OwnedSocket` adoption.
  - Safe std, through an internal `std::net::TcpStream::from(OwnedSocket)` carrier: `read`/`write`/`shutdown`/`set_nonblocking`/`take_error`.
  - New sites: `accept`, `getsockname`, and `getpeername` (3 sites). The recount is in P2.4.5.
- Minimum Windows version: 10 1803 (build 17134) / Server 2019. This is above Phase 1's refused-connect floor of 2004 / Server 2022, so the Phase 1 floor still governs. On a system without `afunix.sys`, `WSASocketW(AF_UNIX)` fails with `WSAEAFNOSUPPORT`, which is passed through as an OS error, not as an akku `Unsupported` stub.
- **G3 step 1 is the `WSAPoll` + AF_UNIX probe on `windows-latest` (P2.8).** If it fails, the Worker stops and reports to Orcha. The human decides whether an exception from rule 11 applies, and the Worker does not decide it.
- Windows AF_UNIX tests W1–W6 (P2.8): echo, half-close, unlink, backlog full, path validation, and backpressure/idle.

First draft (superseded):

Facts: Windows 10 build 17063 and later (generally available from 1803, build 17134) provides `AF_UNIX` with `SOCK_STREAM` only, through `afunix.sys`. It accepts pathname, abstract, and unnamed addresses. It has no `SOCK_DGRAM`/`SOCK_SEQPACKET`, no `socketpair`, no autobind, and no ancillary data (MS blog above). The socket file is an NTFS reparse point that `std::fs::remove_file` deletes. std 1.85 has no Windows AF_UNIX type, and neither tokio nor mio supports it (tokio offers named pipes instead).

Assessment: `AsyncUnixDatagram` and `pair()` can never exist on Windows, so full parity cannot be reached. Rule 11 requires native backends for OS-dependent behaviour of cross-platform APIs and forbids `Unsupported` stubs. `AsyncUnix*` is an OS-family API (std puts it in `std::os::unix`), and with `#[cfg(unix)]` there is no stub. Supporting Windows stream sockets would cost a `sys/uds_win.rs` of about 10–12 more `unsafe` sites: `WSASocketW(AF_UNIX)`, `bind`, `listen`, `accept` with a `SOCKADDR_UN` output (std's `TcpListener::accept` cannot be used because it parses IP addresses), `connect`, `getsockname`, `getpeername`, `send`, and `recv`, plus `OwnedSocket` adoption. It would also need Windows-only `UnixSocketAddr` internals, and its compatibility with `WSAPoll` on the `afunix` provider is unverified (GUESS).

*Superseded recommendation:* ship Phase 2 with `#[cfg(unix)]`, which covers 4 of the 5 Tier-1 OSes natively and matches std, tokio, and the plan's text (`docs/forks/tokio-axum.md` §3, §8). Keep the API Windows-ready through the `UnixSocketAddr` newtype. Track Windows `AsyncUnixListener`/`AsyncUnixStream` as a Phase 2b follow-up whose G2 starts with a `WSAPoll` + `afunix` probe on `windows-latest`. If the human reads rule 11 as requiring it now, Phase 2 grows by `sys/uds_win.rs` (a third allowlist entry) and that probe becomes a G3 prerequisite. See open question 2.

### P2.11 Open questions (with recommendations)

All 15 are resolved in §Phase 2 Decisions (G2) below. The list is kept as the record of what was asked.

1. **`AsyncTcpBuilder`.** Drop it in favour of the value type `SocketOpts` with `bind_with`/`connect_with`. *Recommend: drop* (P2.5.1).
2. **Windows AF_UNIX.** *Recommend: `#[cfg(unix)]` in Phase 2, with Windows stream sockets in Phase 2b after a `WSAPoll` probe* (P2.10). The alternative is `sys/uds_win.rs` now (+10–12 `unsafe` sites, a third allowlist file, and no datagram or `pair` support on Windows).
3. **`UnixSocketAddr` newtype vs std's `SocketAddr`.** *Recommend: the newtype* (P2.5.7).
4. **Unlink-on-drop defaults on** for pathname sockets akku binds (listener and datagram), with an identity check and `set_unlink_on_drop`. *Recommend: yes* (P2.3.4). The alternative is std/tokio parity (never unlink).
5. **Credentials and fd passing** (`peer_cred`, `SCM_RIGHTS`, `SCM_CREDENTIALS`). *Recommend: out of scope for Phase 2; TODO.* `peer_cred` is the most useful of these (local daemon authorization) and costs about 2–3 sites: Linux `SO_PEERCRED`, Darwin `getpeereid` + `LOCAL_PEERPID`.
6. **`peek` on Unix sockets** (std unstable, needs `recv`/`recvfrom` with `MSG_PEEK` via FFI, +2 sites). *Recommend: defer.* TCP and UDP `peek` ship (0 `unsafe`).
7. **SIGPIPE hardening** with `MSG_NOSIGNAL` on Linux/Android and `SO_NOSIGPIPE` at registration on Darwin, for every Unix stream and connected datagram (+1 site). *Recommend: yes.* Also, on Darwin, set `SO_NOSIGPIPE` on accepted and `from_std` TCP streams (Phase 1 relies on inheritance, GUESS), and replace the Phase 1 "never SIGPIPE" check with the child-process probe (test 20): Rust test binaries ignore `SIGPIPE`, so the current check cannot fail.
8. **Disable Windows UDP ICMP resets on every `AsyncUdpSocket`**, connected or not, as Go does. *Recommend: yes.* This makes connected-UDP refused reporting Unix-only; it is documented.
9. **Truncation semantics.** *Recommend: pass std/OS behaviour through and document it* (Unix truncates silently, Windows returns `WSAEMSGSIZE`). Uniform truncation reporting (`recvmsg`/`MSG_TRUNC`) would be a later explicit API.
10. **Consolidate the Phase 1 TCP `connect_nonblocking`** (and `SockAddrIn{,6}`, `WSASocketW`, `SetHandleInformation`) from `epoll.rs`/`kqueue.rs`/`wsapoll.rs` into `sock_unix.rs`/`sock_win.rs` at G4, so poller files hold only poller FFI and `connect_with` reuses the same path. *Recommend: yes* (about 33–35 total sites instead of about 49).
11. **`reuse_port` as `#[cfg(unix)]`**; Windows TCP `reuse_address(true)` as a no-op, and Windows UDP `reuse_address(true)` setting `SO_REUSEADDR`. *Recommend: yes* (P2.5.4).
12. **Only zero linger**, no `set_linger(Duration)`. *Recommend: yes* (P2.5.3).
13. **Re-exporting the blocking `std::os::unix::net::{UnixListener, UnixStream, UnixDatagram}` in `akku::net`.** *Recommend: no*; they belong to `akku::os::unix` (P2.5.8).
14. **Unix connect with a full listener backlog.** *Recommend: pass the raw error through* (Linux `EAGAIN` → `WouldBlock`, Darwin `ECONNREFUSED`), documented under `# Errors`; consider a retry with timeout after Phase 3 timers exist.
15. **`SocketOpts` options that do not apply to the protocol.** *Recommend: fail loudly with `InvalidInput`* rather than ignore them. `only_v6` on IPv4 addresses is skipped, not rejected, so resolved address lists keep working.

## Phase 2 Decisions (G2)

Decided by **Orcha + independent Opus review, 2026-11-26** (the human delegated G2 approval to Orcha). All 15 recommendations in P2.11 are accepted except Q2, which changed. The changes and additions below are binding for G3–G6, and the sections above have been updated to match.

| # | Question | Decision | Where applied |
|---:|---|---|---|
| Q1 | `AsyncTcpBuilder` | Accepted: dropped; `SocketOpts` + `bind_with`/`connect_with`. | P2.2.1, P2.5.1 |
| Q2 | Windows AF_UNIX | **Changed (hard rule 11):** Windows AF_UNIX streams ship in this PR, not as Phase 2b. `#[cfg(any(unix, windows))]` gates `AsyncUnixListener`, `AsyncUnixStream` (stream subset) with the four halves, and `UnixSocketAddr` (pathname and unnamed). `#[cfg(unix)]` gates `AsyncUnixDatagram`, `pair()`, `from_std`/`into_std`, and the `AsFd` impls (the OS lacks them, as with `reuse_port`). Abstract names stay Linux/Android only. Windows reuses the existing `sock_win.rs` sites: `WSASocketW(AF_UNIX)`, `bind`, `listen`, `connect` via `RawSockAddr::Unix`, and `OwnedSocket` adoption. `read`/`write`/`shutdown`/`set_nonblocking`/`take_error` go through an internal `std::net::TcpStream::from(OwnedSocket)` (safe). Only `accept`, `getsockname`, and `getpeername` are new (3 sites); there is no third allowlist file. **G3 step 1 is a `WSAPoll` + AF_UNIX probe on `windows-latest`; if it fails, stop and report to Orcha, and the human decides an exception.** Windows tests: echo, half-close, unlink, backlog full. | P2.1, P2.2.3, P2.3.2–P2.3.4, P2.4.1, P2.4.4, P2.4.5, P2.8 (G3 step 1, W1–W6), P2.10 |
| Q3 | `UnixSocketAddr` | Accepted, with a change: the `From<std::os::unix::net::SocketAddr>` impls (both directions) are `#[cfg(unix)]`. On Windows the type owns its path bytes, and `as_pathname() -> Option<&Path>` still works. | P2.2.3, P2.5.7 |
| Q4 | Unlink-on-drop | Accepted, with a change: unlink **before** close (Go order), with the same identity check. Windows identity uses safe std: `FILE_ATTRIBUTE_REPARSE_POINT` plus `MetadataExt::creation_time`. | P2.3.4, P2.8 tests 17, 23b, W3 |
| Q5 | Credentials and fd passing | Accepted: out of scope. `peer_cred` is the first item of a future Phase 2c in TODO. | P2.1, `_dev/TODO.md` |
| Q6 | Unix `peek` | Accepted: deferred. | P2.1, `_dev/TODO.md` |
| Q7 | SIGPIPE hardening | Accepted as **mandatory**: Android JNI `cdylib`s and iOS `staticlib`s never run Rust's startup `SIGPIPE` ignore. Linux `unix_dgram` sends may not raise `SIGPIPE` at all (GUESS, pinned by test 27). The SIGPIPE child probe keeps its test-only `signal()`. The test-only `fcntl(F_GETFD)` CLOEXEC check is replaced by a safe check (a `Command` child testing `/dev/fd/N`; `/proc/self/fdinfo` on Linux). On Darwin, `SO_NOSIGPIPE` is also asserted via the production `getsockopt` on accepted, `pair`, and `from_std` sockets. | P2.3.2, P2.4.3, P2.8 tests 20, 21, 27 |
| Q8 | Windows UDP ICMP resets | Accepted. The docs state that `from_std` modifies the user's socket (non-blocking mode, Windows ICMP-reset ioctls, Darwin `SO_NOSIGPIPE`). | P2.2.2, P2.2.3 |
| Q9 | Truncation semantics | Accepted (pass-through). Tests 2 and 25 get per-OS zero-length expectations: Windows `recv(&mut [])` on a non-empty datagram gives `Err(10040)`; Darwin is a GUESS. | P2.3.1, P2.8 tests 2, 25 |
| Q10 | Consolidate Phase 1 connect into `sock_*` | Accepted, as **its own first G4 step**: move `connect_nonblocking`, `SockAddrIn{,6}`, `WSASocketW`, and `SetHandleInformation` into `sock_unix.rs`/`sock_win.rs` with no behaviour change, and get the full Phase 1 test suite green on all CI OSes before any Phase 2 code lands. | P2.4.5; G4 order below |
| Q11 | `reuse_port` / Windows `reuse_address` | Accepted. Test 31 is OS-specific: on Linux/Android both sockets accept; on Darwin only both binds succeeding is asserted (no load-balancing). | P2.5.4, P2.8 test 31 |
| Q12 | Zero linger only | Accepted. | P2.2.1, P2.5.3 |
| Q13 | No blocking Unix re-exports | Accepted. | P2.5.8 |
| Q14 | Unix connect with a full backlog | Accepted, with the rule written down: on Linux, `EAGAIN` from an AF_UNIX `connect` is returned directly and never enters `poll_io`/`try_io`. The rustdoc says `WouldBlock` means "backlog full". | P2.2.3, P2.3.2, P2.8 test 19 |
| Q15 | Inapplicable options fail loudly | Accepted. The design states how each listener option reaches accepted streams: `nodelay`/`keepalive`/`zero_linger`/`SO_NOSIGPIPE` are re-applied; buffer sizes and `ttl` are inherited (Darwin `ttl` is a GUESS that test 35 pins). | P2.3.5, P2.8 test 35 |

Missing items added:

| Item | Decision | Where applied |
|---|---|---|
| (a) | Accept-path `setsockopt` failures (macOS `EINVAL` on an already-reset socket, for `nodelay`/`keepalive`/`zero_linger`/`SO_NOSIGPIPE`) close that connection and keep accepting; they are never returned from `accept`. Tested where feasible. | P2.3.5, P2.7, P2.8 test 35b |
| (b) | Darwin returns `ENOBUFS` at once on a full Unix datagram receiver, while Linux waits; both are documented. If test 28 shows a Linux hang, an unconnected `send_to` returns `Err(WouldBlock)` ("receiver queue full") instead of waiting. | P2.3.1, P2.8 test 28 |
| (c) | UDP `send_to` resolves the target once, before the poll loop. The docs warn that a host-name target means blocking DNS for every datagram. | P2.2.2 |
| (d) | Rustdoc notes for iOS sockets that go defunct on suspension, and Android SELinux `EACCES` (both passed through). | P2.3.2 |
| (e) | TCP `peek` tests: `peek` then an awaited `read` must not hang (edge-triggered readiness); `peek` at EOF gives `Ok(0)`; `peek` on the halves. | P2.8 tests 42–44 |
| (f) | CLOEXEC tests on accepted Unix streams and on `AsyncUdpSocket::bind_with`. | P2.8 test 21 |
| (g) | Optional: the Darwin AF_UNIX connect may use safe std `UnixStream::connect` + `set_nonblocking`; test 19 decides. | P2.3.2, P2.4.1, P2.8 test 19 |
| Allowlist | Orcha approves `crates/akku/src/net/sys/sock_unix.rs` and `crates/akku/src/net/sys/sock_win.rs` as allowlist entries (socket FFI is within hard rule 9). Each is added with its file: `sock_win.rs` at G3 step 1 (probe), `sock_unix.rs` at G4. The Checker still reviews every site. | P2.1, P2.4.5 |

**Gate order that follows from these decisions:**
- **G3:** (1) the Windows `WSAPoll` + AF_UNIX probe, which is a stop condition; (2) stubs and adversarial tests 1–44 plus W1–W6, red against the stubs.
- **G4:** (1) the Q10 consolidation alone, with Phase 1 green on `smd`, `ubuntu-latest`, `macos-latest`, and `windows-latest`; (2) `sock_unix.rs`/`sock_win.rs` socket options; (3) `AsyncUdpSocket`; (4) Unix-domain streams (Unix and Windows); (5) `AsyncUnixDatagram`; (6) TCP `peek` and the `AsyncTcpStream` option setters.
