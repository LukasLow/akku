# Tokio & Axum Fork / Port Plan

Status: Proposal (Step 1 — Inventory & Port Plan)  
Date: 2026-11-24  
Scope: Vendor `tokio` + `axum` and all transitive dependencies for reference, audit licenses and `unsafe` usage, and plan a phased, zero-dependency, MPL-2.0-compatible port into `akku`.

---

## 1. Context & Methodology

### Current `akku` baseline

Today `crates/akku/Cargo.toml` pulls external crates **only** when the default `http` feature is enabled:

- `axum = "=0.8.9"` (default features)
- `tokio = { version = "=1.54.1", features = ["rt-multi-thread", "net"] }`

External crate usage is tightly contained to three places:

1. `crates/akku/src/http/server.rs` — wraps `axum::Router`, re-exports `axum::extract::{Path, Request}` and `axum::response::Response`, serves via `tokio::net::TcpListener` + `axum::serve`, and exposes `__private::run` on `tokio::runtime::Builder::new_multi_thread()`.
2. `crates/akku/src/html/mod.rs` — `#[cfg(feature = "http")] impl axum::response::IntoResponse for Html`.
3. `crates/akku-drive/src/lib.rs` — proc macros expanding to `::akku::http::__private::{run, Router, get, post, put, patch, delete}`.

Meanwhile, `akku` **already ships** zero-dependency native implementations of `akku::http::types` (`Method`, `StatusCode`, `Headers`), `akku::http::client` (synchronous HTTP/1.1 client over `std::net::TcpStream`), `akku::bytes`, `akku::json`, `akku::url`, `akku::mime`, `akku::log`, `akku::time`, `akku::fs`, `akku::process`, `akku::archive`, `akku::ws`, and `akku::sse`.

### Reference vendor setup

A scratch project was created in the git-ignored path `target/fork-src/` depending on the latest crates.io releases:

- `tokio` (`1.54.1`, `features = ["full"]`)
- `axum` (`0.8.9`, default features)
- `hyper` (`1.13.0`), `hyper-util` (`0.1.21`), `tower` (`0.5.3`), `tower-http` (`0.7.1`), `http` (`1.5.0`), `http-body` (`1.1.0`), `bytes` (`1.12.2`), `mio` (`1.2.5`)

Vendored inside `smd` (`smd '{cd target/fork-src && cargo vendor}'`), yielding **61 vendored crates** (verified 1:1 against `cargo metadata --format-version 1`).

### Measurement rules

- **Non-test SLOC:** Non-blank, non-comment Rust lines across all `.rs` files in each vendored crate, excluding `tests/`, `benches/`, `examples/`, `fuzz/`, `*_test.rs`/`tests.rs` files, and `#[cfg(test)]` items. (Total non-test physical lines across all 61 crates: **925,438**; non-test SLOC: **726,303**.)
- **`unsafe` count:** Count of lexical `unsafe` keyword tokens (`unsafe { ... }`, `unsafe fn`, `unsafe impl`, `unsafe trait`, `unsafe extern`, `#[unsafe(...)]`) in non-test `.rs` files after stripping line/block comments, string/char literals, and `r#unsafe` — using the same lexical rules as `.github/scripts/check-unsafe.sh`.

---

## 2. License & Size Inventory (All 61 Vendored Crates)

### Executive verdict

- **Total crates:** **61**
- **License verdict:** **61 / 61 MPL-2.0-compatible** (**0 GPL, 0 LGPL, 0 AGPL, 0 unknown**).
  - **Notable attribution details:**
    - `matchit 0.8.4` is **conjunctive** `MIT AND BSD-3-Clause` (its radix tree descends from Go's `httprouter`; both notices must be retained).
    - `sync_wrapper 1.0.2` is `Apache-2.0` only (no MIT option; requires Apache NOTICE/modification notice if ported — not planned for porting).
    - `unicode-ident 1.0.27` is `(MIT OR Apache-2.0) AND Unicode-3.0` (proc-macro transitive dep only; not ported).
    - `serde_urlencoded 0.7.1` uses legacy slash syntax `MIT/Apache-2.0` (verified via vendored `LICENSE-MIT` and `LICENSE-APACHE`; not ported).
- **Total non-test SLOC:** **726,303** (925,438 physical lines)
- **Total non-test `unsafe` tokens:** **16,815**
- **Key structural finding:** Just **7 platform-binding crates** (`windows-sys`, `libc`, `redox_syscall`, `wasi`, `errno`, `signal-hook-registry`, `windows-link`) account for **62.8% of all SLOC (455,873)** and **80.9% of all `unsafe` (13,598)**. Another **~128,000 SLOC** comes from `serde*`, `syn`/`quote`/`proc-macro2`, `tracing*`, `futures-util`, and `tower*` that `akku` does not need to port.

### Rollup by role group

| Role group | Crates | Non-test SLOC | `unsafe` count | Share of SLOC | Share of `unsafe` |
|---|---:|---:|---:|---:|---:|
| Runtime core | 8 | 64,339 | 1,245 | 8.9% | 7.4% |
| I/O driver / reactor | 1 | 8,210 | 146 | 1.1% | 0.9% |
| Sync primitives | 5 | 8,454 | 561 | 1.2% | 3.3% |
| Time | 1 | 394 | 0 | 0.1% | 0.0% |
| Net | 1 | 5,471 | 230 | 0.8% | 1.4% |
| HTTP types | 5 | 13,043 | 207 | 1.8% | 1.2% |
| HTTP/1+2 protocol | 3 | 22,293 | 142 | 3.1% | 0.8% |
| Service / middleware | 4 | 20,286 | 0 | 2.8% | 0.0% |
| Router / extractors | 11 | 53,698 | 29 | 7.4% | 0.2% |
| Utilities | 15 | 74,242 | 657 | 10.2% | 3.9% |
| Platform bindings | 7 | 455,873 | 13,598 | 62.8% | 80.9% |
| **Total** | **61** | **726,303** | **16,815** | **100.0%** | **100.0%** |

> **Note on `tokio` multi-role span:** `tokio 1.54.1` (45,228 SLOC, 1,023 `unsafe`) is listed under **Runtime core** to avoid double-counting, but spans several domains internally: `runtime/{task,scheduler,blocking,driver,context,metrics}` (~10.5k SLOC, 223 `unsafe`), `runtime/io` (~1.3k SLOC, 40 `unsafe`), `sync/` (~6.2k SLOC, 229 `unsafe`), `runtime/{time,time_alt}` (~2.0k SLOC, 104 `unsafe`), `net/` (~4.3k SLOC, 125 `unsafe`), `io/` (~4.8k SLOC, 72 `unsafe`), `fs/` (~1.2k SLOC, 11 `unsafe`), `process/` (~1.9k SLOC, 18 `unsafe`), and `signal/` (~0.6k SLOC, 22 `unsafe`).
>
> **Note on HTTP/2 (`h2`):** `axum` default features enable `http1` only, so the optional `h2` crate (MIT) is not in the 61-crate graph. However, `hyper 1.13.0` includes its `proto::h2` glue layer (2,069 SLOC, 0 `unsafe`) inside the vendored tarball.

### Full crate inventory by role

#### Runtime core (8 crates — 64,339 SLOC, 1,245 `unsafe`)

| Crate | Version | Upstream repo | License (SPDX) | MPL-2.0 | Non-test SLOC | `unsafe` | Platform-specific |
|---|---|---|---|---|---:|---:|---|
| `tokio` | `1.54.1` | [gh:tokio-rs/tokio](https://github.com/tokio-rs/tokio) | `MIT` | Yes | 45,228 | 1,023 | Mixed (Unix/Windows/WASM `sys` adapters) |
| `tokio-macros` | `2.7.2` | [gh:tokio-rs/tokio](https://github.com/tokio-rs/tokio) | `MIT` | Yes | 791 | 0 | No |
| `futures-core` | `0.3.34` | [gh:rust-lang/futures-rs](https://github.com/rust-lang/futures-rs) | `MIT OR Apache-2.0` | Yes | 306 | 5 | No |
| `futures-task` | `0.3.34` | [gh:rust-lang/futures-rs](https://github.com/rust-lang/futures-rs) | `MIT OR Apache-2.0` | Yes | 500 | 64 | No |
| `futures-util` | `0.3.34` | [gh:rust-lang/futures-rs](https://github.com/rust-lang/futures-rs) | `MIT OR Apache-2.0` | Yes | 16,083 | 136 | No |
| `atomic-waker` | `1.1.2` | [gh:smol-rs/atomic-waker](https://github.com/smol-rs/atomic-waker) | `Apache-2.0 OR MIT` | Yes | 99 | 4 | No |
| `pin-project-lite` | `0.2.18` | [gh:taiki-e/pin-project-lite](https://github.com/taiki-e/pin-project-lite) | `Apache-2.0 OR MIT` | Yes | 1,241 | 9 | No |
| `sync_wrapper` | `1.0.2` | [gh:Actyx/sync_wrapper](https://github.com/Actyx/sync_wrapper) | `Apache-2.0` | Yes (Apache NOTICE) | 91 | 4 | No |

#### I/O driver / reactor (1 crate — 8,210 SLOC, 146 `unsafe`)

| Crate | Version | Upstream repo | License (SPDX) | MPL-2.0 | Non-test SLOC | `unsafe` | Platform-specific |
|---|---|---|---|---|---:|---:|---|
| `mio` | `1.2.5` | [gh:tokio-rs/mio](https://github.com/tokio-rs/mio) | `MIT` | Yes | 8,210 | 146 | Mixed (cross-platform API + OS `sys/`) |

#### Sync primitives (5 crates — 8,454 SLOC, 561 `unsafe`)

| Crate | Version | Upstream repo | License (SPDX) | MPL-2.0 | Non-test SLOC | `unsafe` | Platform-specific |
|---|---|---|---|---|---:|---:|---|
| `parking_lot` | `0.12.5` | [gh:Amanieu/parking_lot](https://github.com/Amanieu/parking_lot) | `MIT OR Apache-2.0` | Yes | 1,816 | 82 | No |
| `parking_lot_core` | `0.9.12` | [gh:Amanieu/parking_lot](https://github.com/Amanieu/parking_lot) | `MIT OR Apache-2.0` | Yes | 2,290 | 134 | Mixed (OS thread parkers) |
| `lock_api` | `0.4.14` | [gh:Amanieu/parking_lot](https://github.com/Amanieu/parking_lot) | `MIT OR Apache-2.0` | Yes | 3,148 | 297 | No |
| `once_cell` | `1.21.4` | [gh:matklad/once_cell](https://github.com/matklad/once_cell) | `MIT OR Apache-2.0` | Yes | 1,031 | 45 | No |
| `scopeguard` | `1.2.0` | [gh:bluss/scopeguard](https://github.com/bluss/scopeguard) | `MIT OR Apache-2.0` | Yes | 169 | 3 | No |

#### Time (1 crate — 394 SLOC, 0 `unsafe`)

| Crate | Version | Upstream repo | License (SPDX) | MPL-2.0 | Non-test SLOC | `unsafe` | Platform-specific |
|---|---|---|---|---|---:|---:|---|
| `httpdate` | `1.0.3` | [gh:pyfisch/httpdate](https://github.com/pyfisch/httpdate) | `MIT OR Apache-2.0` | Yes | 394 | 0 | No |

*(Plus `tokio::time` timer wheel inside `tokio`: ~2,015 SLOC, 104 `unsafe`.)*

#### Net (1 crate — 5,471 SLOC, 230 `unsafe`)

| Crate | Version | Upstream repo | License (SPDX) | MPL-2.0 | Non-test SLOC | `unsafe` | Platform-specific |
|---|---|---|---|---|---:|---:|---|
| `socket2` | `0.6.5` | [gh:rust-lang/socket2](https://github.com/rust-lang/socket2) | `MIT OR Apache-2.0` | Yes | 5,471 | 230 | Mixed (Unix + Windows `sys/`) |

*(Plus `tokio::net` inside `tokio`: ~4,337 SLOC, 125 `unsafe`.)*

#### HTTP types (5 crates — 13,043 SLOC, 207 `unsafe`)

| Crate | Version | Upstream repo | License (SPDX) | MPL-2.0 | Non-test SLOC | `unsafe` | Platform-specific |
|---|---|---|---|---|---:|---:|---|
| `http` | `1.5.0` | [gh:hyperium/http](https://github.com/hyperium/http) | `MIT OR Apache-2.0` | Yes | 6,304 | 63 | No |
| `http-body` | `1.1.0` | [gh:hyperium/http-body](https://github.com/hyperium/http-body) | `MIT` | Yes | 343 | 2 | No |
| `http-body-util` | `0.1.5` | [gh:hyperium/http-body](https://github.com/hyperium/http-body) | `MIT` | Yes | 1,463 | 1 | No |
| `bytes` | `1.12.2` | [gh:tokio-rs/bytes](https://github.com/tokio-rs/bytes) | `MIT` | Yes | 4,078 | 141 | No |
| `mime` | `0.3.18` | [gh:hyperium/mime](https://github.com/hyperium/mime) | `MIT OR Apache-2.0` | Yes | 855 | 0 | No |

#### HTTP/1+2 protocol (3 crates — 22,293 SLOC, 142 `unsafe`)

| Crate | Version | Upstream repo | License (SPDX) | MPL-2.0 | Non-test SLOC | `unsafe` | Platform-specific |
|---|---|---|---|---|---:|---:|---|
| `hyper` | `1.13.0` | [gh:hyperium/hyper](https://github.com/hyperium/hyper) | `MIT` | Yes | 12,389 | 54 | No |
| `hyper-util` | `0.1.21` | [gh:hyperium/hyper-util](https://github.com/hyperium/hyper-util) | `MIT` | Yes | 8,074 | 17 | No |
| `httparse` | `1.10.1` | [gh:seanmonstar/httparse](https://github.com/seanmonstar/httparse) | `MIT OR Apache-2.0` | Yes | 1,830 | 71 | No |

#### Service / middleware (4 crates — 20,286 SLOC, 0 `unsafe`)

| Crate | Version | Upstream repo | License (SPDX) | MPL-2.0 | Non-test SLOC | `unsafe` | Platform-specific |
|---|---|---|---|---|---:|---:|---|
| `tower-service` | `0.3.3` | [gh:tower-rs/tower](https://github.com/tower-rs/tower) | `MIT` | Yes | 45 | 0 | No |
| `tower-layer` | `0.3.3` | [gh:tower-rs/tower](https://github.com/tower-rs/tower) | `MIT` | Yes | 408 | 0 | No |
| `tower` | `0.5.3` | [gh:tower-rs/tower](https://github.com/tower-rs/tower) | `MIT` | Yes | 6,251 | 0 | No |
| `tower-http` | `0.7.1` | [gh:tower-rs/tower-http](https://github.com/tower-rs/tower-http) | `MIT` | Yes | 13,582 | 0 | No |

#### Router / extractors (11 crates — 53,698 SLOC, 29 `unsafe`)

| Crate | Version | Upstream repo | License (SPDX) | MPL-2.0 | Non-test SLOC | `unsafe` | Platform-specific |
|---|---|---|---|---|---:|---:|---|
| `axum` | `0.8.9` | [gh:tokio-rs/axum](https://github.com/tokio-rs/axum) | `MIT` | Yes | 7,920 | 0 | No |
| `axum-core` | `0.5.6` | [gh:tokio-rs/axum](https://github.com/tokio-rs/axum) | `MIT` | Yes | 1,662 | 0 | No |
| `matchit` | `0.8.4` | [gh:ibraheemdev/matchit](https://github.com/ibraheemdev/matchit) | `MIT AND BSD-3-Clause` | Yes (both notices) | 969 | 6 | No |
| `serde` | `1.0.229` | [gh:serde-rs/serde](https://github.com/serde-rs/serde) | `MIT OR Apache-2.0` | Yes | 11,559 | 2 | No |
| `serde_core` | `1.0.229` | [gh:serde-rs/serde](https://github.com/serde-rs/serde) | `MIT OR Apache-2.0` | Yes | 7,281 | 2 | No |
| `serde_derive` | `1.0.229` | [gh:serde-rs/serde](https://github.com/serde-rs/serde) | `MIT OR Apache-2.0` | Yes | 7,472 | 0 | No |
| `serde_json` | `1.0.154` | [gh:serde-rs/json](https://github.com/serde-rs/json) | `MIT OR Apache-2.0` | Yes | 12,618 | 13 | No |
| `serde_path_to_error` | `0.1.20` | [gh:dtolnay/path-to-error](https://github.com/dtolnay/path-to-error) | `MIT OR Apache-2.0` | Yes | 2,420 | 0 | No |
| `serde_urlencoded` | `0.7.1` | [gh:nox/serde_urlencoded](https://github.com/nox/serde_urlencoded) | `MIT/Apache-2.0` | Yes (legacy `/`) | 1,208 | 0 | No |
| `form_urlencoded` | `1.2.3` | [gh:servo/rust-url](https://github.com/servo/rust-url) | `MIT OR Apache-2.0` | Yes | 285 | 2 | No |
| `percent-encoding` | `2.3.2` | [gh:servo/rust-url](https://github.com/servo/rust-url) | `MIT OR Apache-2.0` | Yes | 304 | 4 | No |

#### Utilities (15 crates — 74,242 SLOC, 657 `unsafe`)

| Crate | Version | Upstream repo | License (SPDX) | MPL-2.0 | Non-test SLOC | `unsafe` | Platform-specific |
|---|---|---|---|---|---:|---:|---|
| `slab` | `0.4.12` | [gh:tokio-rs/slab](https://github.com/tokio-rs/slab) | `MIT` | Yes | 752 | 6 | No |
| `smallvec` | `1.16.6` | [gh:smallvec/smallvec](https://github.com/smallvec/smallvec) | `MIT OR Apache-2.0` | Yes | 1,748 | 79 | No |
| `bitflags` | `2.13.2` | [gh:bitflags/bitflags](https://github.com/bitflags/bitflags) | `MIT OR Apache-2.0` | Yes | 1,979 | 2 | No |
| `cfg-if` | `1.0.5` | [gh:rust-lang/cfg-if](https://github.com/rust-lang/cfg-if) | `MIT OR Apache-2.0` | Yes | 46 | 0 | No |
| `memchr` | `2.8.3` | [gh:BurntSushi/memchr](https://github.com/BurntSushi/memchr) | `Unlicense OR MIT` | Yes | 6,567 | 333 | No |
| `itoa` | `1.0.19` | [gh:dtolnay/itoa](https://github.com/dtolnay/itoa) | `MIT OR Apache-2.0` | Yes | 320 | 16 | No |
| `ryu` | `1.0.23` | [gh:dtolnay/ryu](https://github.com/dtolnay/ryu) | `Apache-2.0 OR BSL-1.0` | Yes | 2,025 | 27 | No |
| `zmij` | `1.0.23` | [gh:dtolnay/zmij](https://github.com/dtolnay/zmij) | `MIT` | Yes | 1,585 | 69 | No |
| `log` | `0.4.35` | [gh:rust-lang/log](https://github.com/rust-lang/log) | `MIT OR Apache-2.0` | Yes | 2,573 | 6 | No |
| `tracing` | `0.1.44` | [gh:tokio-rs/tracing](https://github.com/tokio-rs/tracing) | `MIT` | Yes | 3,564 | 7 | No |
| `tracing-core` | `0.1.36` | [gh:tokio-rs/tracing](https://github.com/tokio-rs/tracing) | `MIT` | Yes | 2,873 | 27 | No |
| `proc-macro2` | `1.0.107` | [gh:dtolnay/proc-macro2](https://github.com/dtolnay/proc-macro2) | `MIT OR Apache-2.0` | Yes | 4,661 | 6 | No |
| `quote` | `1.0.47` | [gh:dtolnay/quote](https://github.com/dtolnay/quote) | `MIT OR Apache-2.0` | Yes | 1,547 | 0 | No |
| `syn` | `3.0.10` | [gh:dtolnay/syn](https://github.com/dtolnay/syn) | `MIT OR Apache-2.0` | Yes | 43,299 | 77 | No |
| `unicode-ident` | `1.0.27` | [gh:dtolnay/unicode-ident](https://github.com/dtolnay/unicode-ident) | `(MIT OR Apache-2.0) AND Unicode-3.0` | Yes (both notices) | 703 | 2 | No |

#### Platform bindings (7 crates — 455,873 SLOC, 13,598 `unsafe`)

| Crate | Version | Upstream repo | License (SPDX) | MPL-2.0 | Non-test SLOC | `unsafe` | Platform-specific |
|---|---|---|---|---|---:|---:|---|
| `libc` | `0.2.191` | [gh:rust-lang/libc](https://github.com/rust-lang/libc) | `MIT OR Apache-2.0` | Yes | 115,491 | 788 | Yes (Unix / Solaris / WASI / etc.) |
| `windows-sys` | `0.61.2` | [gh:microsoft/windows-rs](https://github.com/microsoft/windows-rs) | `MIT OR Apache-2.0` | Yes | 334,260 | 12,530 | Yes (Windows) |
| `windows-link` | `0.2.1` | [gh:microsoft/windows-rs](https://github.com/microsoft/windows-rs) | `MIT OR Apache-2.0` | Yes | 33 | 0 | Yes (Windows) |
| `redox_syscall` | `0.5.18` | [gl:redox-os/syscall](https://gitlab.redox-os.org/redox-os/syscall) | `MIT` | Yes | 3,817 | 178 | Yes (Redox OS) |
| `wasi` | `0.11.1+wasi-snapshot-preview1` | [gh:bytecodealliance/wasi](https://github.com/bytecodealliance/wasi) | `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT` | Yes | 1,453 | 62 | Yes (WASI preview1) |
| `errno` | `0.3.15` | [gh:lambda-fairy/rust-errno](https://github.com/lambda-fairy/rust-errno) | `MIT OR Apache-2.0` | Yes | 318 | 12 | Yes (OS-specific errno/GetLastError) |
| `signal-hook-registry` | `1.4.8` | [gh:vorner/signal-hook](https://github.com/vorner/signal-hook) | `MIT OR Apache-2.0` | Yes | 501 | 28 | Yes (Unix signals) |

---

## 3. Mapping to `akku` Flat Modules (per `Namespace.md`)

All target modules follow `Namespace.md` (flat by default, `std` supersets where named like `std`, `server`/`client` inside protocol modules, no foreign crate names exposed).

| Role group | Target `akku` module | In `Namespace.md`? | Mapped surface & notes |
|---|---|---|---|
| Runtime core | `akku::task` | **Yes** | `Runtime`, `block_on`, `spawn`, `spawn_blocking`, `JoinHandle`, `AbortHandle`, `yield_now`, `CancelToken`, `timeout`, `join!`/`select!` helpers. On `wasm32`: transparent `#[cfg(target_arch = "wasm32")]` spawn bridge. Macro `#[akku::task::main]` via `akku-drive` (see flags below). |
| I/O driver / reactor | Internal `net::sys` / `task::reactor` (private) | N/A (internal) | **No public `akku::mio` or `akku::reactor` module.** The readiness reactor (`epoll`/`kqueue`/`WSAPoll`, with `IOCP+AFD` later on Windows) is an internal engine shared by `akku::task` and `akku::net`, isolating all syscall `unsafe` inside `sys/`. |
| Sync primitives | `akku::sync` | **Yes** | `std::sync` superset (explicit re-exports: `Arc`, `Mutex`, `RwLock`, `OnceLock`, `Condvar`, `Barrier`, `mpsc`, atomics) + async primitives per `Namespace.md` §3: `AsyncMutex`, `AsyncRwLock` (never shadowing `std::sync::{Mutex, RwLock}`), while `oneshot`, `broadcast`, `watch`, `Notify`, and `Semaphore` keep plain names (and async MPSC avoids shadowing `std::sync::mpsc`; exact channel name resolved at Phase 3 G2). |
| Time | `akku::time` (additions) | **Yes** (family exists) | Add async timers (`sleep`, `sleep_until`, `interval`, `timeout`) backed by the `akku::task` timer driver, plus IMF-fixdate (RFC 9110) HTTP date formatting/parsing (replacing `httpdate`, ~150 LOC). |
| Net | `akku::net` | **Yes** | Full `std::net` superset (explicit re-exports of IP/SocketAddr + blocking `TcpListener`, `TcpStream`, `UdpSocket`, `Incoming`) **plus** `AsyncTcpListener`, `AsyncTcpStream` (with `AsyncTcp*Half` splits), `AsyncUdpSocket`, `AsyncUnixListener`, `AsyncUnixStream`, `AsyncUnixDatagram` (`#[cfg(unix)]`), and `AsyncTcpBuilder`/`SocketOpts` (`SO_REUSEADDR`, `SO_REUSEPORT`, `TCP_NODELAY`, `TCP_KEEPALIVE`, `IPV6_V6ONLY`, backlog) replacing `socket2` + `tokio::net`. |
| HTTP types | `akku::http` + `akku::bytes` + `akku::mime` | **Yes** (all exist) | Extend `akku::http::types` with akku-owned `Request`, `Response`, `Body` (fixed bytes + streaming), `Version`, `Cookie`. Reuse existing native `akku::bytes` and `akku::mime` directly. |
| HTTP/1+2 protocol | `akku::http::{server, client}` (internal `h1` codec) | **Yes** | Safe HTTP/1.1 parser & encoder (`Content-Length`, chunked transfer, keep-alive, `Expect: 100-continue`, strict RFC 9112 smuggling guards) powering both `http::server` and an async mode on `http::client`. HTTP/2 (gated behind `http2` per `Namespace.md`) ships in two steps: h2c client/server in Phase 7, then h2 over `akku::tls` with ALPN in Phase 8. |
| Service / middleware | `akku::http` (thin middleware hook) | **Yes** (inside `http`) | **No `akku::tower` or `akku::service` module.** Expose a small `Middleware` / `around` hook on `akku::http::App` plus essential built-in middlewares (CORS, gzip/deflate compression via `akku::archive`, body-size limit, request timeout, structured logging via `akku::log`). |
| Router / extractors | `akku::http` | **Yes** | Internal safe radix-tree router (inspired by `matchit`), `App`, `Mount`, `FromRequest`, `IntoResponse`, `Path<T>`, `Query<T>` (over `akku::url`), `Json<T>` (over `akku::json`), `Form<T>`, `State<T>`, fallback routes, graceful shutdown. |
| Utilities | Existing `akku::*` + private helpers | **Yes** | `akku::log`, `akku::url`, `akku::json`, `akku::error`, `akku::bytes` already exist. Small internal helpers (e.g., ~60-line safe `Slab<T>`) stay `pub(crate)`. |
| Platform bindings | `sys/` submodules (`net/sys/`, `task/sys/`, later `akku::os::*/sys`) | **Yes** (`akku::os` + `sys` rule) | Minimal hand-written `extern` declarations per OS inside allowlisted `sys/` submodules; shares conventions with the planned `akku::os::{unix, linux, macos, ios, android, windows}` layer (TODO 0.7). |
| Async fs / process / signals (from `tokio/full`) | `akku::fs`, `akku::process` | **Yes** (both exist) | Later extensions: `fs` async helpers via `task::spawn_blocking`; `process` async child pipes & OS signals (already tracked in TODO 0.10). |

### Proposed public names NOT currently in `Namespace.md` (flagged for decision)

1. **`akku::io`** — Neither `AsyncRead`/`AsyncWrite` nor a `std::io` superset module appears in `Namespace.md`. Options:
   - *(a)* Add flat `akku::io` (`std::io` superset including sync `BufReader` & `copy` + `AsyncRead`, `AsyncWrite`, `AsyncBufRead`, `AsyncReadExt::copy_to`, later `AsyncBufReader`), or
   - *(b)* Keep `AsyncRead`/`AsyncWrite` inside `akku::task::io` (or `akku::net`) and avoid a new level-1 module.
2. **`#[akku::task::main]`** — `Namespace.md` §Macros lists `#[akku::http::main]` only. Adding `#[akku::task::main]` lets non-HTTP async programs (`net`, `nfs`, `kv`, CLI tools) start the runtime cleanly while keeping `#[akku::http::main]` as an alias.
3. **Timer/interval/timeout boundary across `akku::time`, `akku::task`, and `akku::schedule`** — `Namespace.md` currently lists `timeout` under `akku::task`, `intervals` under `akku::schedule`, and `clock`/`datetime`/`duration`/`timezone` under `akku::time`. Recommend canonicalizing low-level async timers (`sleep`, `sleep_until`, `interval`, `timeout`) in one place (`akku::time`, re-exporting `timeout` in `akku::task` if desired) and keeping `akku::schedule` for high-level cron/retry/backoff.
4. **`akku::http::Middleware` (and `akku::http::Body`, `akku::http::State`, `akku::http::Query`, `akku::http::Json`, `akku::http::Form`)** — Internal to `akku::http`, so no new level-1 module, but should be confirmed in the `http` G2 API review.

---

## 4. What NOT to Port

Dropping unnecessary crates cuts the footprint from **726,303 SLOC / 16,815 `unsafe`** down to an estimated **~8,440–11,240 SLOC / ~41–51 `unsafe`** (Phases 0–6, with `WSAPoll` on Windows).

| Crates skipped | SLOC saved | `unsafe` saved | Reason not to port |
|---|---:|---:|---|
| `windows-sys`, `libc`, `windows-link`, `redox_syscall`, `wasi`, `errno` | 455,372 | 13,570 | 99.9% unused FFI symbols; Redox and WASI preview1 sockets are out of scope (`wasm32` uses browser APIs via `#[cfg(target_arch = "wasm32")]`). Replaced by ~700–900 LOC of minimal hand-written `sys/` bindings (see §6). |
| `serde`, `serde_core`, `serde_derive`, `serde_json`, `serde_path_to_error`, `serde_urlencoded`, `ryu`, `zmij`, `itoa` | 46,488 | 129 | `akku` already ships native `akku::json` and `akku::url`; `std` formatting suffices for headers/status codes (`itoa` is micro-optimization). Forking `serde` would duplicate a huge framework. |
| `syn`, `proc-macro2`, `quote`, `unicode-ident`, `tokio-macros` | 51,001 | 85 | `akku-drive` is already the dedicated proc-macro crate; `#[akku::task::main]` is ~40 LOC inside `akku-drive` (no need for `tokio-macros`'s complex flavor/worker attribute matrix). |
| `bytes`, `mime`, `form_urlencoded`, `percent-encoding`, `log` | 8,095 | 153 | Already implemented natively with zero dependencies in `akku::bytes`, `akku::mime`, `akku::url`, and `akku::log`. |
| `tracing`, `tracing-core` (+ `tokio` `runtime/metrics`, `runtime/dump`, `loom`) | 8,582+ | 53+ | `akku::trace` is a separate Wave-3 domain. Wiring `tracing` spans into every hot poll/HTTP request bloats binary size and compile time; emit structured logs via `akku::log` at boundaries instead. |
| `once_cell`, `parking_lot`, `parking_lot_core`, `lock_api`, `scopeguard`, `cfg-if`, `pin-project-lite`, `sync_wrapper`, `atomic-waker` | 9,931 | 578 | Superseded by Rust 1.85 `std`: `std::sync::{OnceLock, LazyLock}` replaces `once_cell`; modern `std::sync::{Mutex, RwLock, Condvar}` is already futex-backed on Linux/macOS/Windows; `std::task::Wake` + `#[cfg]` modules replace the rest. |
| `tower`, `tower-service`, `tower-layer`, `tower-http` (trait hierarchy) | 20,286 | 0 | Generic `Service<R>` + `Layer` + `Ready`/`Buffer`/`LoadShed`/`Balance` creates the exact trait maze `CONCEPT.md` §8 rejects. Port only the concrete HTTP middleware behaviors needed behind one simple `akku::http` middleware hook. |
| `futures-util`, `futures-core`, `futures-task` (kitchen sink) | 16,889 | 205 | Vast combinator surface (`FuturesOrdered`, v0.3 `AsyncRead` compat, `Sink`, `BiLock`). Write only the few combinators `akku` needs (`join`, `select`, minimal `Stream`) cleanly in safe Rust. |
| `memchr`, `httparse` SIMD, `slab`, `smallvec`, `bitflags` | 11,753 | 455 | SIMD byte scanners (`memchr`, `httparse::simd`) trade 368 `unsafe` blocks for marginal HTTP server gains; safe slice scanning / safe SWAR is fast and `#![deny(unsafe_code)]`-clean. A 60-line safe `Slab<T>` and typed structs replace `slab`/`bitflags`/`smallvec`. |
| `hyper::ffi`, `hyper-util::client::legacy`, `mio` Solaris/WASI/named-pipe, `tokio::{io::uring, runtime::time_alt, runtime::jspi, net::windows::named_pipe}` | ~8,000+ | ~90+ | C ABI for libcurl, deprecated Hyper 0.14 compat, exotic OSes, experimental/unstable Tokio features not needed for `akku`. |

---

## 5. `unsafe` Strategy

Policy baseline (`AGENTS.md` rule 9, `.agents/skills/akku-unsafe/SKILL.md`, `.github/scripts/check-unsafe.sh`):

> **`unsafe` is allowed ONLY where no safe way exists (FFI, OS syscalls, socket options, platform APIs) — never for convenience or speed — and ONLY inside allowlisted `sys/` submodules with a `// SAFETY:` comment on every block.**

### Prerequisite mechanics PR (Phase 0)

Today `crates/akku/src/lib.rs` sets `#![forbid(unsafe_code)]`. Before any `sys/` submodule lands, one small governance PR (with Checker review + human approval) must:

1. Change `#![forbid(unsafe_code)]` → `#![deny(unsafe_code)]` in `crates/akku/src/lib.rs`.
2. Add `#![deny(clippy::undocumented_unsafe_blocks)]` and `#![deny(unsafe_op_in_unsafe_fn)]`.
3. Add each new `crates/akku/src/<module>/sys/<file>.rs` to `.github/unsafe-allowlist.txt` only in the PR that introduces it (enforced by `.github/scripts/check-unsafe.sh`).

### A. Unavoidable `unsafe` (confined strictly to `sys/` submodules)

These operations cross the OS kernel ABI and have **no safe `std` equivalent**:

| Subsystem | Platforms | Syscalls / FFI APIs | `sys/` location | Est. `unsafe` blocks |
|---|---|---|---|---:|
| Readiness reactor (`epoll`) | Linux, Android | `epoll_create1`, `epoll_ctl`, `epoll_wait`, `eventfd`, `socket`, `connect` (`close`/`read`/`write` handled safely via `OwnedFd`/`File`) | `net/sys/epoll.rs` | ~11–12 |
| Readiness reactor (`kqueue`) | macOS, iOS (later non-Tier-1 BSDs) | `kqueue`, `kevent`, `fcntl`, `socket`, `setsockopt(SO_NOSIGPIPE)`, `connect` (`EVFILT_USER` + `NOTE_TRIGGER` waker; `close` via `OwnedFd`) | `net/sys/kqueue.rs` | ~12–14 |
| Readiness reactor (`WSAPoll` first; `IOCP` + `AFD` later) | Windows | Initial: `WSAPoll`, `WSAGetLastError`, `socket`, `ioctlsocket(FIONBIO)`, `connect`, safe `std::net` loopback wake socketpair. Later follow-up: `CreateIoCompletionPort`, `GetQueuedCompletionStatusEx`, `PostQueuedCompletionStatus`, `SetFileCompletionNotificationModes`, `CloseHandle`, `NtCreateFile`, `NtDeviceIoControlFile`, `NtCancelIoFileEx`, `RtlNtStatusToDosError`, `WSAIoctl` | `net/sys/wsapoll.rs` (later `iocp.rs`, `afd.rs`) | ~8–10 (~30–40 with `IOCP`+`AFD`) |
| Socket options not in `std::net` | Unix + Windows | `setsockopt`, `getsockopt` (`SO_REUSEPORT`, `TCP_KEEPALIVE`, `IPV6_V6ONLY`, `SO_NOSIGPIPE`, `TCP_NODELAY`), plus `socket`/`bind`/`listen` only when options must be set pre-`bind` | `net/sys/sock_unix.rs`, `net/sys/sock_win.rs` | ~10–15 |
| Process signals (deferred, TODO 0.10) | Unix + Windows | `sigaction` + self-pipe / `SetConsoleCtrlHandler` | `process/sys/` | ~10 |

- **Native-first mobile coverage:** Android shares Linux's `epoll` (`target_os = "android"`), and iOS shares macOS's `kqueue` (`target_os = "ios"`), so desktop + mobile Tier-1 platforms are covered by just three backends (`epoll`, `kqueue`, `WSAPoll` — with `IOCP/AFD` as a later Windows follow-up).
- **Unsupported targets** compile cleanly and return a typed `Unsupported` error (per `akku-unsafe`).
- **Basic socket I/O stays safe:** `std::net::{TcpListener, TcpStream, UdpSocket}` and `std::os::unix::net::*` already provide safe `bind`, `connect`, `accept`, `read`, `write`, `shutdown`, and `set_nonblocking(true)`. Wrapping `std::net` types and passing their `AsRawFd`/`AsRawSocket` to the `sys` reactor avoids reimplementing socket syscalls in `unsafe`.

### B. Speed-only `unsafe` in upstream crates → Safe `akku` designs

Almost all of the 1,500+ non-FFI `unsafe` tokens in `tokio`, `hyper`, `http`, `httparse`, `bytes`, and `matchit` exist solely to squeeze out allocations, bounds checks, or zero-initialization. Under akku's principle (*"fast AND absolutely safe"*), every one of these has a clean safe replacement at comparable real-world performance:

| Upstream hotspot | Vendored `unsafe` | Why upstream used `unsafe` | Safe `akku` design (0 `unsafe`) | Performance comparison |
|---|---:|---|---|---|
| `tokio/src/runtime/task/` | 143 | Single-allocation `RawTask` (manual vtable, header/future/output union, raw `Waker`) | `Arc<Task>` implementing `std::task::Wake` (stabilized Rust 1.51) + `Mutex<TaskState>` (or `RefCell` on single-thread) holding `Pin<Box<F>>` | +1 `Box` alloc per `spawn` (~20 ns); negligible for I/O tasks lasting µs–ms. |
| `tokio/src/util/linked_list.rs`, `runtime/time*`, `sync/{notify,batch_semaphore}` | ~176 | Intrusive doubly-linked lists using `NonNull` pointers into pinned futures to avoid waiter heap allocations | **Slab-indexed** intrusive list (`Vec<Slot>` with `usize` next/prev indices) + slab-indexed timing wheel or `BinaryHeap<Reverse<(Instant, u64)>>` | O(1) wheel / O(log N) heap; contiguous `Vec` slots often have *better* CPU cache locality than scattered `Pin<&mut>` pointers. |
| `tokio/src/sync/{mpsc,oneshot,mutex,rwlock,broadcast,once_cell}` | 173 | Lock-free linked blocks with `MaybeUninit`/`UnsafeCell` and manual `ptr::read`/`ptr::write` | `std::sync::Mutex<VecDeque<T>>` + waiter `VecDeque<Waker>`; `std::sync::OnceLock` | Futex-backed `std::sync::Mutex` is ~15 ns uncontended; lock held only for a pointer push/pop. |
| `tokio/src/runtime/scheduler/` | 53 | Lock-free Chase-Lev work-stealing deque (`UnsafeCell<[MaybeUninit<Notified>; 256]>`) | Single-thread: `VecDeque<Arc<Task>>`. Multi-thread: per-worker `Mutex<VecDeque<Arc<Task>>>` + global queue + `try_lock` half-batch stealing | Local push/pop is uncontended; stealing half a victim's queue amortizes one `try_lock` across N/2 tasks. |
| `tokio/src/io/read_buf.rs`, `hyper/src/rt/io.rs`, `hyper/src/proto/h1/io.rs` | 37 | `&mut [MaybeUninit<u8>]` to avoid zero-initializing fresh read buffers | Reuse one pre-zeroed `Vec<u8>` / `akku::bytes::BytesMut` buffer per connection across reads | Zero-fill paid once on connection accept; **0% overhead** in steady state. |
| `bytes` | 141 | Custom raw-pointer vtable & uninit `BytesMut` | **Already solved:** `akku::bytes` uses `Arc<[u8]>` + `start..end` window (`#![forbid(unsafe_code)]`) | O(1) clone and O(1) zero-copy slicing, identical asymptotics. |
| `httparse` (35 SIMD + 36 ptrs) & `hyper/src/proto/h1/role.rs` (6) | 77 | SSE4.2/AVX2/NEON header scanning, unchecked slice indexing, `MaybeUninit` header arrays | Safe slice parser + optional safe SWAR (`u64` word scan) + `Vec` reused per connection | HTTP/1.1 parsing is <2% of total server request time; safe Rust parses >500k req/s per core. |
| `http` (`HeaderMap`, `Uri`, `Method`) | 63 | Open-addressed `HeaderMap` with `get_unchecked` and `from_utf8_unchecked` | Extend safe `akku::http::Headers` (`Vec<(String, String)>` for typical <32 headers, optional safe index map if large) | Linear scan over 8–16 headers beats hashing due to L1 cache locality; 0 `unsafe`. |
| `matchit` | 6 | `UnsafeCell<T>` so `at_mut` can yield `&mut T` during radix-tree descent | Return `RouteId(usize)` from `&self` radix tree, index into `Vec<Handler>` | **Identical** O(path length) lookup, 0 `unsafe`. |
| `axum`, `axum-core`, `tower*` | 0 | Already 100% safe | Keep 100% safe | Identical. |

---

## 6. System Bindings: Minimal Hand-Written `sys/` vs Vendoring `libc` / `windows-sys`

| Criterion | Option A: Minimal hand-written `sys/` **(Recommended)** | Option B: Vendor/fork `libc` + `windows-sys` |
|---|---|---|
| Code size | **~700–900 LOC** total across Linux/Android, macOS/iOS/BSD, Windows | **449,751 SLOC** (`libc` 115,491 + `windows-sys` 334,260) |
| `unsafe` surface | **~41–51 audited blocks** with `WSAPoll` (~60–75 with `IOCP`+`AFD`) | **13,318 `unsafe` tokens** |
| Compile time | Negligible (<50 ms) | Multi-second cold compile + massive doc/IDE indexing |
| External deps | **0** | 0 if vendored, but bloats the `akku` crate tarball to tens of MB (exceeding crates.io 10 MiB limit!) |
| ABI maintenance risk | Moderate — must get C struct layouts/constants right per target OS & arch | Low — upstream tracks every target |

> **Decisive constraint:** crates.io enforces a **10 MiB package limit** per crate. Vendoring `windows-sys` + `libc` inside `crates/akku` is practically infeasible in a single-crate design (`CONCEPT.md` §3).

### Concrete syscall budget for Option A

1. **Linux & Android (`sys/epoll.rs` + `sys/sock_unix.rs`, ~220 LOC):**
   - `extern "C"`: `epoll_create1`, `epoll_ctl`, `epoll_wait`, `eventfd`, `read`, `write`, `close`, `fcntl`, `setsockopt`, `getsockopt`, `socket`, `bind`, `listen`.
   - Structs & constants: `epoll_event` (**ABI note:** `#[repr(C, packed)]` on `x86_64`, `#[repr(C)]` on `aarch64`/`arm`/`riscv64`!), `EPOLL_CLOEXEC`, `EPOLL_CTL_{ADD,MOD,DEL}`, `EPOLL{IN,OUT,ERR,HUP,RDHUP,ET}`, `EFD_{CLOEXEC,NONBLOCK}`, `SOL_SOCKET`, `SO_REUSEPORT`, `IPPROTO_TCP`, `TCP_KEEPIDLE`/`TCP_KEEPINTVL`/`TCP_KEEPCNT`.
2. **macOS, iOS & BSD (`sys/kqueue.rs` + `sys/sock_unix.rs`, ~180 LOC):**
   - `extern "C"`: `kqueue`, `kevent`, `fcntl`, `close`, `setsockopt`, `getsockopt`.
   - Structs & constants: `kevent` (field types differ slightly between macOS/FreeBSD/NetBSD/OpenBSD — gate explicitly), `timespec`, `EVFILT_{READ,WRITE,USER}`, `EV_{ADD,DELETE,CLEAR,ENABLE,EOF,ERROR,RECEIPT}`, `NOTE_TRIGGER`, `SO_NOSIGPIPE`, `SO_REUSEPORT`, `TCP_KEEPALIVE`.
3. **Windows (`sys/wsapoll.rs` + `sys/sock_win.rs`, ~180 LOC first; optional `sys/iocp.rs` + `sys/afd.rs` later, ~380 LOC):**
   - Initial (`WSAPoll` backend): `#[link(name = "ws2_32")] extern "system"`: `WSAPoll`, `WSAGetLastError`, `setsockopt`, `getsockopt`; struct `WSAPOLLFD`.
   - Later performance follow-up (`IOCP` + `AFD`): `kernel32` (`CreateIoCompletionPort`, `GetQueuedCompletionStatusEx`, `PostQueuedCompletionStatus`, `SetFileCompletionNotificationModes`, `CloseHandle`), `ntdll` (`NtCreateFile`, `NtDeviceIoControlFile`, `NtCancelIoFileEx`, `RtlNtStatusToDosError`), `ws2_32` (`WSAIoctl`), and structs `OVERLAPPED`, `OVERLAPPED_ENTRY`, `IO_STATUS_BLOCK`, `OBJECT_ATTRIBUTES`, `UNICODE_STRING`, `AFD_POLL_INFO`, `AFD_POLL_HANDLE_INFO`.

### ABI safety guardrails for Option A

- Compile-time `const _: () = assert!(size_of::<...>() == ...)` checks per `(target_os, target_arch)`.
- Cross-OS CI matrix (`linux-musl` in `smd`, `ubuntu-latest`, `macos-latest`, `windows-latest` — aligning with TODO 0.6) executing real socket/reactor integration tests on every PR.
- If an exotic Unix target (e.g., `s390x`/`sparc64` with divergent `O_NONBLOCK`/`SOL_SOCKET`) is ever requested, return `Unsupported` rather than silently miscompiling with wrong constants.

---

## 7. Attribution & MPL-2.0 License Interaction

### Legal mechanics (MPL-2.0 + MIT / BSD-3-Clause / Apache-2.0)

`akku` is licensed under **MPL-2.0** (file-level weak copyleft). All 61 vendored crates use permissive licenses compatible with MPL-2.0:

- **Clean-room / design-inspired files** (e.g., safe `VecDeque` executor, safe `akku::bytes`, safe HTTP/1.1 parser written from RFC 9110/9112): original `akku` code under MPL-2.0. Credit prior art in `<module>/_dev/DESIGN.md` §References.
- **Derived / ported files** (any file substantially copied or adapted from upstream MIT, BSD-3-Clause, or Apache-2.0 sources — e.g., `mio`'s Windows AFD polling logic, `matchit`'s radix-tree split/merge algorithm, or `hyper`'s HTTP/1.1 corner-case state machine):
  1. **MIT & BSD-3-Clause:** Require preserving the upstream copyright notice and license text/permission notice in all copies or substantial portions. When adapted inside an MPL-2.0 file, the upstream portions remain subject to MIT/BSD-3-Clause notices while `akku`'s modifications and the combined Larger Work are distributed under MPL-2.0 (§3.3). Retaining both notices satisfies both licenses simultaneously.
  2. **Apache-2.0 (if ever adapted):** Also requires including a copy of Apache-2.0, preserving any upstream `NOTICE` file, and adding a prominent modification notice in changed files (Apache-2.0 §4(b)).
  3. **`matchit` (`MIT AND BSD-3-Clause`):** Because the expression is conjunctive (`AND`), any code adapted from `matchit` must carry **both** the `matchit` MIT notice **and** the Go `httprouter` BSD-3-Clause notice (`LICENSE.httprouter`).

### Required repo artifacts before the first ported file lands

1. **Root `THIRD_PARTY_LICENSES` file** (referenced by `CONCEPT.md` §7 and `docs/ROADMAP.md`), containing for each adapted upstream crate:
   - Crate name, exact version (and commit/tag if known), upstream repository URL, SPDX license identifier;
   - Exact list of `akku` files derived from it;
   - Full verbatim upstream copyright line(s) and license text (plus `NOTICE` text if Apache-2.0).
2. **Package inclusion:** Update `crates/akku/Cargo.toml`'s `include = [...]` list to ship `LICENSE` and `THIRD_PARTY_LICENSES` (or symlink/copy into `crates/akku/`) inside the published crates.io tarball so binary/source redistributors automatically receive all notices.
3. **Standard file header for every forked/adapted `.rs` file:**

```rust
// SPDX-License-Identifier: MPL-2.0 AND MIT
//
// Portions adapted from `<crate>` v<version> (<upstream repo URL>):
//   Copyright (c) <Year> <Upstream Authors>
//   Licensed under the MIT License (see THIRD_PARTY_LICENSES).
//
// Modifications for akku are licensed under the Mozilla Public License 2.0.
```

*(Adjust the `SPDX-License-Identifier` to `MPL-2.0 AND MIT AND BSD-3-Clause` if adapting `matchit`, or `MPL-2.0` alone for clean-room native files.)*

---

## 8. Phased Implementation Plan (Vertical Slices)

Each phase is scoped as **1 small PR** (or 2 tightly focused PRs where noted) with public-API and adversarial tests in `tests.rs`, following the `NewLib` / `BugFix` workflows.

| Phase | Slice / PR scope | Est. LOC | `unsafe` blocks (in `sys/`) | Replaces in existing `akku::http` adapter |
|---|---|---:|---:|---|
| **0** | `unsafe` mechanics (`#![deny(unsafe_code)]`, clippy lints, `THIRD_PARTY_LICENSES` scaffold, `Cargo.toml` `include`, `Namespace.md` updates) | ~40 | 0 | Governance prerequisite for `sys/` |
| **1** | **First vertical slice (all Tier-1 OSes + `akku::io`):** single-threaded `akku::task` (`Runtime`, `Limits`, `block_on`, `try_block_on`, `spawn`, `try_spawn`, `JoinHandle` detach + `abort()`, `yield_now`) + **`akku::io`** (full `std::io` superset including sync `copy` + safe `&mut [u8]` `AsyncRead`/`AsyncWrite`/`AsyncBufRead`, `Async*Ext` methods including `AsyncReadExt::copy_to`) + **`akku::net`** (full `std::net` re-exports including blocking `TcpListener`/`TcpStream`/`UdpSocket` + `AsyncTcpListener`/`AsyncTcpStream` & `AsyncTcp*Half` splits implementing `akku::io` + native reactors for all Tier-1 OSes: Linux/Android `net/sys/epoll.rs`, macOS/iOS `net/sys/kqueue.rs`, Windows `net/sys/wsapoll.rs`) | ~2,100–2,800 | ~31–36 | Replaces `mio` TCP readiness on all Tier-1 OSes; immediately unblocks `akku::net`, `akku::io`, & `akku::nfs` across Linux, macOS, Windows, iOS, and Android |
| **2** | `AsyncUdpSocket` + Unix domain sockets (`AsyncUnixListener`/`AsyncUnixStream`/`AsyncUnixDatagram`, `#[cfg(unix)]`) + pre-bind `SocketOpts`/`AsyncTcpBuilder` (`net/sys/sock_unix.rs`, `net/sys/sock_win.rs`); Windows `IOCP` + `AFD` stays a later benchmarked performance follow-up | ~800–1,100 | ~10–15 | Replaces `socket2` (Unix + Windows) + remaining `tokio::net` (UDP/Unix) |
| **3** | Async timers in `akku::time` (`sleep`, `sleep_until`, `timeout`, `interval` — no `std::time` clash) + `akku::sync` (`std::sync` superset + `AsyncMutex`/`AsyncRwLock`, plain-named `oneshot`/`broadcast`/`watch`/`Notify`/`Semaphore`, and non-shadowing async MPSC) | ~1,200–1,500 | 0 | Replaces `tokio::time`, `tokio::sync`, `parking_lot*`, `once_cell` |
| **4** | Multi-threaded `akku::task` runtime (per-worker `Mutex<VecDeque>` + `try_lock` batch stealing), `spawn_blocking` pool (for DNS & `fs`), `#[akku::task::main]` in `akku-drive` | ~900–1,200 | 0 | Replaces `tokio::runtime::Builder::new_multi_thread()` in `crates/akku/src/http/server.rs` (`__private::run`) and `tokio-macros` |
| **5** | Native HTTP/1.1 server codec (`akku::http::h1`: parser, encoder, chunked, keep-alive, smuggling/DoS guards, IMF-fixdate) + akku-owned `Request`, `Response`, `Body` (bytes + streaming), `Cookie`, graceful shutdown | ~1,800–2,400 | 0 | Replaces `hyper`, `hyper-util`, `httparse`, `http`, `http-body*`, `httpdate`; removes `axum::extract::Request` & `axum::response::Response` re-exports in `http/server.rs` and `axum::response::IntoResponse` in `html/mod.rs` |
| **6** | Native safe radix-tree router (`http::router`), `FromRequest`/`IntoResponse`/`Handler`, extractors (`Path`, `Query`, `Json`, `Form`, `State`), thin `Middleware` hook (CORS, compression, limits, timeout), update `akku-drive` route macros & `App::serve` | ~1,600–2,200 | 0 | Replaces `axum`, `axum-core`, `matchit`, `tower*`, `serde*`; **deletes `axum` and `tokio` from `Cargo.toml` — `akku` reaches 0 external runtime dependencies** |
| **7** | *(Follow-up)* Async mode + connection pooling in `akku::http::client` + `wasm32` `fetch` client & `task::spawn`; HTTP/2 cleartext (h2c: HPACK, streams, flow control, Rapid-Reset guards) for client and server | ~1,200 (client) / ~6,000+ (h2) | 0 | Brings `http::client` & `task` to full async + `wasm32` parity; adds h2c client & server |
| **8** | *(Follow-up)* HTTP/2 over `akku::tls` with ALPN — Go `net/http` parity (HTTP/1.1 + h2c + h2, client and server) | ~800–1,200 | 0 | Completes Go `net/http` parity (HTTP/1.1 + h2c + h2 over TLS) |

**Total for Phases 0–6 (zero-dep async runtime + async I/O + cross-platform networking + HTTP/1.1 server):** **~8,440–11,240 LOC**, **~41–51 `unsafe` blocks** (with `WSAPoll` on Windows; all confined to `sys/`).

### Why Phase 1 is the right first slice

1. **Native Tier-1 parity from day one (`AGENTS.md` rule 11):** Linux/Android (`epoll` + `eventfd`), macOS/iOS (`kqueue` + `EVFILT_USER`), and Windows (`WSAPoll` + loopback wake pair) all ship together with zero Tier-1 `Unsupported` stubs, verified end-to-end across `smd` (Alpine/musl) and required CI (`ubuntu-latest`, `macos-latest`, `windows-latest`).
2. **Complete async I/O + TCP foundation:** Bundling `akku::io` (full `std::io` superset + `&mut [u8]` `AsyncRead`/`AsyncWrite`/`AsyncBufRead` + `Async*Ext` including `copy_to`) with `akku::task` and `akku::net` (`AsyncTcpListener`/`AsyncTcpStream`) immediately unblocks Wave 2 protocols (`akku::dns`, `akku::nfs`, `akku::quic`, and the native `akku::http` rewrite) behind the final trait boundary.
3. **Bounded, auditable risk:** Proves the 3-file `sys/` allowlist mechanics (~31–36 `unsafe` blocks total), safe `Arc<Task>: Wake` executor, and unified edge/level reactor abstraction in ~2,100–2,800 LOC before adding UDP/Unix sockets (Phase 2) or touching `akku::http`.

---

## 9. Risks & Open Questions for the Human

### Key technical risks

1. **HTTP security & CVE maintenance burden:** Owning the HTTP/1.1 server parser means `akku` owns defense against request smuggling (RFC 9112 §6.3 ambiguous `Content-Length` / `Transfer-Encoding`, obs-fold, bare LF, invalid chunk extensions), slowloris header trickling, and OOM header/body bombs. *Mitigation:* strict-by-default parser, hard header/body/idle-timeout limits on every connection, adversarial tests in `tests.rs`, fuzz targets, and monitoring RustSec advisories for `hyper`/`tokio`/`mio`/`axum`.
2. **Windows `AFD` undocumented NT APIs:** `mio`, `wepoll`, and `polling` rely on `NtDeviceIoControlFile` against `\Device\Afd\Mio` to emulate readiness polling over IOCP. While de-facto stable (used by Node.js/libuv, Python, and Rust's async ecosystem), it is technically undocumented and has subtle handle-unwrapping edge cases (`SIO_BASE_HANDLE` / layered service providers).
3. **Multi-threaded scheduler scalability:** A safe per-worker `Mutex<VecDeque>` + `try_lock` batch-stealing scheduler is fast and zero-`unsafe` for typical web/service workloads (tens of thousands of req/s), but under extreme micro-task ping-pong contention on 32+ cores it may trail Tokio's lock-free Chase-Lev deque. Benchmark in `--release` during Phase 4 before considering any more complex design.
4. **Linux `io_uring`:** Readiness-based `epoll` matches Rust's borrow-based `Future` model cleanly. Completion-based `io_uring` requires owned buffers (kernel holds pointers across polls — dropping a `Future` cannot cancel synchronously without risking UAF), has had recurring kernel security vulnerabilities, and is blocked by default in many container seccomp profiles (including Docker/OrbStack defaults). Recommend **deferring `io_uring`** indefinitely unless a specialized buffer-owning I/O API is designed later.
5. **HTTP/2 complexity & TLS dependency:** Full HTTP/2 (`h2` is ~12k LOC: HPACK, stream state machine, flow control, priority/settings frames, plus high-severity CVE history like Rapid Reset CVE-2023-44487) is larger than the entire Phases 1–4 runtime+net stack, and browsers only negotiate HTTP/2 over TLS (ALPN). Decision: ship HTTP/1.1 first, then h2c, then h2 over `akku::tls` (Go `net/http` parity); HTTP/3 is revisited alongside `akku::quic`.
6. **Cross-architecture C ABI drift in hand-written `sys/`:** Struct layout differences (e.g., `epoll_event` packing on `x86_64` vs `aarch64`, BSD `kevent` field widths) can cause subtle memory corruption if wrong. Mitigated by compile-time `size_of`/`offset_of` assertions and running Linux (x86_64 + aarch64), macOS, and Windows CI runners.

### Open questions for the human

*(All questions below are resolved in `### Decisions (human, 2026-11-25)`.)*

1. **Multi-threaded executor & `unsafe`:** Confirm that a **100% safe** per-worker `Mutex<VecDeque>` + `try_lock` batch-stealing scheduler is the right tradeoff (never allowing `unsafe` Chase-Lev deques outside `sys/`), rather than relaxing Hard Rule 9 for scheduler hot paths.
2. **Windows reactor backend:** Prefer `IOCP + AFD` (same battle-tested model as `mio`/`wepoll`, O(1) readiness scaling, uses NT `NtDeviceIoControlFile`), or a simpler documented `WSAPoll` backend first (zero undocumented NT APIs, simpler `sys/`, but O(N) per poll and slower past ~1k concurrent sockets)?
3. **HTTP/2 timeline:** Confirm shipping **HTTP/1.1 only** through Phase 6 (dropping `axum`/`tokio` immediately once HTTP/1.1 parity holds), and deferring HTTP/2 until `akku::tls` lands (or evaluating an audited fork of `h2` at that point).
4. **Breaking `App::into_axum()` & `akku::http::Router` re-export:** Confirm that retiring `App::into_axum()` and switching `akku::http::{Request, Response, Router}` from axum re-exports to akku-owned types is acceptable during `0.0.x` (keeping `App`, `Mount`, `Path`, `ResponseBuilder`, `ok_*`, and `#[akku::http::{get,post,put,patch,delete,main}]` source-compatible).
5. **Namespace decisions (not yet settled in `Namespace.md`):**
   - Should async `AsyncRead`/`AsyncWrite` live in a new flat **`akku::io`** (`std::io` superset), or inside `akku::task` / `akku::net`?
   - Should **`#[akku::task::main]`** be added alongside `#[akku::http::main]`?
   - Where is the canonical home for `sleep` / `interval` / `timeout` (`akku::time` vs `akku::task` vs `akku::schedule`)?
6. **Extractors without `serde`:** Confirm whether `akku::http::Json<T>` / `Query<T>` / `Form<T>` should target `akku::json` and `akku::url` natively (zero external deps), and whether an optional `serde` feature flag should also be offered later.
7. **Proceed with Phase 0 + Phase 1?** Ready to kick off the `unsafe` mechanics PR (Phase 0) followed by the Phase 1 vertical slice (single-threaded `akku::task` + `akku::io` + native Tier-1 OS backends in `akku::net`)?

### Decisions (human, 2026-11-25)

1. **Scheduler:** 100% safe per-worker `Mutex<VecDeque>` + `try_lock` batch stealing; no `unsafe` outside `sys/`.
2. **Windows backend:** `WSAPoll` first; `IOCP` + `AFD` later as a benchmarked performance follow-up.
3. **HTTP:** target is parity with Go `net/http` — HTTP/1.1, HTTP/2 cleartext (h2c), and HTTP/2 over TLS, client and server. Delivery order: HTTP/1.1 (Phase 5–6) → h2c (Phase 7) → h2 + TLS/ALPN once `akku::tls` exists (Phase 8). HTTP/2 is part of the plan, not deferred indefinitely.
4. **`App::into_axum()`:** dropped; `akku::http::{Request, Response, Router}` become akku-owned types during `0.0.x`.
5. **Namespace:** async `AsyncRead`/`AsyncWrite` live in a new flat `akku::io` (`std::io` superset); `sleep`/`sleep_until`/`interval`/`timeout` live in `akku::time`; the runtime is `akku::task`; `akku::schedule` keeps cron/retry/backoff. `#[akku::task::main]` is added next to `#[akku::http::main]`. These names go into `Namespace.md` in the Phase 0 PR.
6. **Extractors:** `Json`/`Query`/`Form` target `akku::json` and `akku::url` natively; no `serde`.
7. **Go:** Phase 0 and Phase 1 start now.
8. **Phase 1 G2 revision (2026-11-25):** Bundle `akku::io` (full `std::io` superset + safe `&mut [u8]` `AsyncRead`/`AsyncWrite`/`AsyncBufRead`, `Async*Ext` methods `read`/`read_exact`/`copy_to`/`write`/`write_all`/`flush`/`shutdown`) into Phase 1 with `AsyncTcpStream` implementing these traits. Apply platform tiers (`AGENTS.md` rule 11) to Phase 1: ship native `epoll` (Linux/Android), `kqueue` (macOS/iOS), and `WSAPoll` (Windows; `IOCP`+`AFD` later) in Phase 1 with no Tier-1 `Unsupported` stubs, leaving `AsyncUdpSocket`, Unix sockets, and `SocketOpts` in Phase 2. `JoinHandle` detaches on drop and provides `abort()`. Expose `Runtime`, `Limits`, `try_block_on`, and `try_spawn`; `spawn` requires `Send + 'static`; calling runtime-dependent functions with no active runtime returns `Err(io::Error)` instead of panicking; dropping `AsyncTcpOwnedWriteHalf` calls `shutdown(Shutdown::Write)` ignoring errors.
9. **`std`-superset async naming rule (2026-11-25, `Namespace.md` §3):** In every `std`-superset module (`io`, `fs`, `net`, `sync`, `process`, …), every name `std` defines keeps `std`'s exact item and meaning (re-exported by explicit list, never glob, never redefined or shadowed). Async traits and types take an `Async` prefix (`AsyncRead`, `AsyncTcpStream`, `AsyncTcpListener`, `AsyncUdpSocket`, `AsyncFile`, `AsyncMutex`, `AsyncRwLock`); async reader/writer operations live on `Async*Ext` (`AsyncReadExt::copy_to`, never `async_copy`); non-clashing names stay plain (`akku::time::{sleep, sleep_until, timeout, interval}` and `akku::sync::{broadcast, watch, Notify, Semaphore}`). Never use `_async` or `async_` affixes.
