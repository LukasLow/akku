# akku Namespace

This document is akku's **naming constitution**. It defines how public paths are
named, when families form, and which name collisions are forbidden. Read it
before adding any public name ([AGENTS.md](AGENTS.md) rule 4).

## Core rules

1. **One library crate:** all public modules live under `akku::*`. Exactly three
   crates are published: `akku`, the proc-macro crate `akku-drive` (required by
   Rust, re-exported by `akku`), and the optional binary `akku-cli`. Everything
   else, including platform code, stays inside `akku`.
2. **No global symbol pool:** `akku::get`, `akku::Request`, `akku::Server`, and
   `akku::Value` do not exist. Every public type and macro lives in its module
   (`akku::http::get`, `akku::json::Value`).
3. **Flat by default:** every standalone library starts directly at level 1
   (`akku::<name>`). No artificial taxonomy such as `akku::web::http` or
   `akku::encoding::json`.
4. **Dynamic families:** when several closely related modules grow
   organically, they may be bundled into a family:
   - before `1.0` directly,
   - from `1.0` on through `#[deprecated] pub use family::module;` transition
     aliases until the next major version.
5. **Short, crisp names:** short canonical names are preferred (`ws`, `sse`,
   `pic`, `vid`, `phy`, `cli`, `fs`, `kv`, `url`, `tls`; storage as single
   modules `sql`/`kv` rather than a `db` family).
6. **No Rust keywords, no shadowing:** reserved keywords are never module
   names (`async`, `gen`, `try`, `box`, `mod`, `type`, `move`, `ref`, `impl`).
   The async module is `akku::task`, not `akku::async`. There is no
   `akku::core`, so `::core` is never shadowed.
7. **Discovery through tags, not deep trees:** cross-cutting assignments
   (`#web`, `#cli`, `#wasm32`, `#native-0dep`, `#adapter`, `#fork`,
   `#maintainer`) are kept in module metadata (`meta.toml`) and sorted in the
   catalog (e.g. GitHub Pages).

---

## Families (grouped modules)

The following areas are deliberately grouped as families (level 1 = family,
level 2 = individual modules):

| Family | Contents / examples | Reason |
|---|---|---|
| `akku::archive` | `gzip`, `zstd`, `zip`, `tar` | Shared stream/archive family |
| `akku::auth` | `password`, `jwt`, `oauth2`, `totp`, `webauthn`, `session`, `acl` | Authentication, identity, and access control |
| `akku::audio` | `pcm`, `wav`, `flac`, `dsp`, `midi`, `device` | Separate from image/video (e.g. for DAWs) |
| `akku::vid` | `frame`, `codec`, `mux`, `stream`, `device` | Separate from audio/image |
| `akku::pic` | `png`, `jpeg`, `webp`, `bmp`, `canvas`, `filter` | Separate from audio/video (e.g. image editors) |
| `akku::crypto` | `hash`, `aead`, `sign`, `kem`, `kdf`, `rand` | Avoids collision with `std::hash`; clear audit boundary |
| `akku::time` | `clock`, `datetime`, `duration`, `timezone`, `sleep`, `sleep_until`, `interval`, `timeout` | One coherent time, calendar, and async timer model |
| `akku::units` | `distance`, `mass`, `time`, `temp`, `energy`, `pressure`, `speed` | Dimension system (`geo` stays separate and flat) |
| `akku::math` | `decimal`, `linear`, `special`, `stat` | Numerics family |
| `akku::phy` | `mechanics`, `relativity`, `quantum`, `thermo`, `cosmology` | Physics calculations |
| `akku::actor` | `supervisor`, `mailbox`, `registry` | Erlang-style actor system (`cluster` and `broker` stay flat) |
| `akku::gpu` | `compute`, `render` | GPU compute and graphics (including WebGPU on `wasm32`) |
| `akku::wasm` | `host`, `guest` | **Only** the WASM runtime (host, plugins, guest helpers) |
| `akku::os` | `unix`, `linux`, `macos`, `ios`, `android`, `windows` | Platform modules (`std::os` superset), see [OS layer](#os-layer) |

Networking is deliberately **not** a family: `net`, `dns`, and `quic` are flat
modules (below).

---

## Flat modules (level 1)

Everything outside the families above lives directly under `akku::`.

### Network, transport & storage protocols

| Module | Contents / rule |
|---|---|
| `akku::net` | `std::net` superset: IP addresses, async TCP/UDP/Unix sockets on the akku runtime, socket options (Wave 2) |
| `akku::dns` | DNS resolver and message encoding (Wave 2) |
| `akku::quic` | QUIC transport (Wave 2; the base for `http3`) |
| `akku::xdr` | XDR encoding and decoding (RFC 4506) (Wave 2, megaghost-driven) |
| `akku::vfs` | Virtual filesystem trait layer that backends and servers plug into (Wave 2, megaghost-driven) |
| `akku::nfs` | NFS `server` and `client` over `vfs`; ONC-RPC stays an internal detail for now (Wave 2, megaghost-driven; read-only `server` first) |

### Web & protocols

| Module | Contents / rule |
|---|---|
| `akku::http` | `App`/`server`, `client`, `Request`, `Response`, `Status`, `Headers`, `Cookie`, `Path`, `Result`; `http2`/`http3` are protocol features inside `http` |
| `akku::ws` | `server`, `client` (native + `wasm32`), `Message`, `Frame` |
| `akku::sse` | `server`, `client` (native + `wasm32`), `Event`, `Stream` |
| `akku::webpush` | WebPush client/server (not a vague `push`) |
| `akku::url` | URL/URI parsing and query builder |
| `akku::mime` | MIME types and content-type detection |
| `akku::tls` | TLS configuration, streams, certificates (`#[doc(alias = "ssl")]`; there is **no** `akku::ssl` module) |
| `akku::rpc` | `server`, `client` |
| `akku::mail` | SMTP client and MIME mail builder |
| `akku::mqtt` | `client`, `broker` |
| `akku::ssh` | `server`, `client` |

### Markup & text

| Module | Contents / rule |
|---|---|
| `akku::html` | Kept separate from `akku::http`; `Html`, `escape`, later parser/sanitizer |
| `akku::xml` | XML reading/writing |
| `akku::markdown` | CommonMark/GFM → AST/HTML |
| `akku::template` | Safe text/HTML templates |
| `akku::regex` | Regular expressions |
| `akku::text` | Unicode, graphemes, normalization, string helpers |
| `akku::diff` | Text/structure diff |
| `akku::i18n` | Localization and translations |

### Data formats, serialization & encoding

| Module | Contents / rule |
|---|---|
| `akku::json` | `Value`, `from_str`, `to_string`, typed mapping |
| `akku::csv` | Streaming reader/writer, headers, typed rows |
| `akku::toml` | TOML reading/writing |
| `akku::yaml` | YAML reading/writing |
| `akku::ini` | INI files |
| `akku::dotenv` | `.env` files |
| `akku::config` | Layered app configuration (defaults → file → env) |
| `akku::protobuf` | Protocol Buffers (runtime + codegen via `build-dependencies`) |
| `akku::flatbuffers` | Zero-copy FlatBuffers |
| `akku::msgpack` | MessagePack |
| `akku::cbor` | CBOR |
| `akku::base64` | Base64 and Base64URL |
| `akku::hex` | Hex encoding/decoding |

### Databases, storage & data

| Module | Contents / rule |
|---|---|
| `akku::sql` | Unified SQL API, pool, `migrate`, drivers (`sql::sqlite`, `sql::postgres`; no product names at level 1) |
| `akku::kv` | Key-value store API (embedded + e.g. `kv::redis`) |
| `akku::cache` | In-memory LRU/TTL cache |
| `akku::dataframe` | Column-oriented table processing |

### Async, system, CLI & development

| Module | Contents / rule |
|---|---|
| `akku::task` | Async tasks, runtime, cancellation (not `async`; timers and `timeout` live in `akku::time`) |
| `akku::io` | `std::io` superset with async `AsyncRead`, `AsyncWrite`, `AsyncBufRead` traits |
| `akku::sync` | `std::sync` superset + `AsyncMutex`, `AsyncRwLock`, `AsyncOnceCell`, `Semaphore`, `Notify`; channel modules `channel` (async MPMC), `oneshot`, `broadcast`, `watch` |
| `akku::schedule` | Cron, retry/backoff |
| `akku::cluster` | Node membership, gossip, discovery |
| `akku::broker` | Pub/sub message broker |
| `akku::fs` | `std::fs` superset + `walk`, `glob`, `tempfile`, `watch`, atomic writes |
| `akku::cli` | Argument parser, subcommands, auto help |
| `akku::term` | ANSI colors, tables, progress bars, prompts |
| `akku::tui` | Terminal UI components |
| `akku::process` | `std::process` superset + pipes, signals |
| `akku::env` | `std::env` superset + typed vars, OS standard paths |
| `akku::log` | Structured logging (human + JSON) |
| `akku::trace` | Spans, tracing, metrics |
| `akku::test` | HTTP test client, tempdir, snapshots |
| `akku::bench` | Micro-benchmarks |
| `akku::error` | Unified `Error`/`Result` with context and error chain |
| `akku::bytes` | Byte buffers, endianness, bit operations |
| `akku::id` | UUID (v4/v7), ULID, NanoID |

### Unified OS APIs

Cross-platform APIs over the [OS layer](#os-layer); each is a flat module.

| Module | Contents / rule |
|---|---|
| `akku::clipboard` | System clipboard |
| `akku::notify` | Desktop/system notifications |
| `akku::secret` | OS secret stores (Keychain, Credential Manager, Secret Service) |
| `akku::power` | Battery, power state, sleep inhibition |

Media capture is not a separate module: it lives in `audio::device` and
`vid::device`.

### Science, media helpers & UI

| Module | Contents / rule |
|---|---|
| `akku::algo` | Graphs, pathfinding, sort/search algorithms |
| `akku::geo` | Geo coordinates, distances, projections (separate from `units`) |
| `akku::chem` | Chemistry domain (may become a family as it grows) |
| `akku::bio` | Biology domain (may become a family as it grows) |
| `akku::econ` | Economics/finance domain (may become a family as it grows) |
| `akku::color` | RGB, HSL, OKLCH, hex, contrast |
| `akku::pdf` | PDF generation/reading |
| `akku::ui` | UI components (native + `wasm32`) |
| `akku::webview` | System WebView applications |

---

## Cross-cutting conventions

### 1. `server` and `client` live *inside*

Protocol modules with server and client roles always use the same inner
structure:

```rust
akku::http::server   // or akku::http::App
akku::http::client   // native + wasm32 (fetch)

akku::ws::server
akku::ws::client     // native + wasm32 (WebSocket)

akku::sse::server
akku::sse::client    // native + wasm32 (EventSource)

akku::nfs::server
akku::nfs::client
```

Shared types (`Request`, `Response`, `Message`, `Event`, `Status`) live directly
on the protocol module (`akku::http::Request`, `akku::ws::Message`).

### 2. `wasm32` target vs. `akku::wasm`

- **No `forwasm::` module.** Client and pure data modules support `wasm32`
  transparently behind the same API via `#[cfg(target_arch = "wasm32")]`.
- **`akku::wasm`** is exclusively the WASM runtime (`wasm::host`, `wasm::guest`).

### 3. `std` name supersets

Modules named like a `std` module (`io`, `fs`, `net`, `sync`, `time`, `env`,
`process`, `os`) are **supersets**, so `use akku::fs` works ergonomically next
to or instead of `std::fs`.

In a `std`-superset module (`io`, `fs`, `net`, `sync`, `process`, …), every name
`std` defines in the matching module keeps `std`'s exact item and meaning. It is
re-exported by explicit list, never by glob, and never redefined or shadowed. An
async counterpart never takes that name:

- **(a)** async traits and types get an `Async` prefix (`AsyncRead`,
  `AsyncTcpStream`, `AsyncFile`, `AsyncMutex`);
- **(b)** async operations on a reader or writer become methods on the matching
  `Async*Ext` trait (`copy_to`, `read_to_end`);
- **(c)** other async free functions may only use names `std` lacks in that
  module (`time::sleep`, `time::timeout`). Otherwise the operation goes on an
  `Ext` trait. Never use an `_async` or `async_` suffix or prefix;
- **(d)** a blocking twin of an async method on an akku type takes a
  `blocking_` prefix (`blocking_lock`, `blocking_recv`); never a `_blocking`
  suffix or `sync_`;
- **(e)** an akku item never takes the name of a `std` item in the matching
  module, stable or unstable at current stable Rust. Hence the async MPMC
  channel is `akku::sync::channel`, never `mpsc` or `mpmc`. Recorded
  exception: `akku::sync::oneshot` (std's is unstable). Once
  `std::sync::oneshot` is stable and within the MSRV, `akku::sync::oneshot`
  becomes a `std` superset and its async halves take the `Async` prefix (a
  `0.0.x` breaking change).

### 4. No product names at level 1

Vendor and product names live under their domain:

- `akku::kv::redis` (not `akku::redis`)
- `akku::sql::postgres`, `akku::sql::sqlite` (not `akku::postgres`)

### 5. Reserved generic type names

These type names never appear directly under `akku::`, always in their module:

```text
Server, Client, Request, Response, Error, Result, Value, Map, Set,
List, Node, Element, Document, Route, Handler, Context, Config
```

### 6. Macros

Macros follow the same module rule:

```rust
#[akku::http::get("/users/{id}")]
#[akku::http::post("/users")]
#[akku::http::main]
#[akku::task::main]
```

Not `#[akku::get]` or `#[akku::main]`. `akku-drive` stays an internal
implementation detail; users import only `akku`.

### 7. Prelude

An optional `akku::prelude` may offer convenience but never owns names. The full
module path is always the canonical source.

---

## OS layer

- **Platform modules:** `akku::os::{unix, linux, macos, ios, android, windows}`
  are a `std::os` superset, each gated with `#[cfg(target_os = …)]` (or
  `cfg(unix)`) inside `akku`. Names follow Rust's `std::os` (`macos`, not
  `darwin`). There are no `akku-<platform>` crates; platform-only dependencies
  go in `[target.'cfg(…)'.dependencies]`.
- **Unified APIs** (`clipboard`, `notify`, `secret`, `power`, …) are flat
  domain modules built on the platform modules. On unsupported platforms they
  return typed `Unsupported` / `NotAvailable` / `PermissionDenied` errors.
- Platform code that needs `unsafe` keeps it in a `sys` submodule
  ([akku-unsafe](.agents/skills/akku-unsafe/SKILL.md)).
- The detailed design (support matrix in `meta.toml`, first slices) is an
  RFC PR (TODO 0.7).
