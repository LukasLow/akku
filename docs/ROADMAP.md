# Roadmap

Akku's goal is an **expansive Rust standard library alongside `std`** inside a
**single library crate (`akku`)** with an ultimate target of **zero external
dependencies**. External crates (`adapter`) are used only as a temporary speedup
and are replaced over time by native Akku implementations (`native`) or
license-clean, audited code forks (`fork`) behind the same stable public API.

See [`Namespace.md`](../Namespace.md) for the flat-by-default naming rules and
dynamic families, and [`CONCEPT.md`](../CONCEPT.md) for the full vision.

### Backend Legend

- **`native`** — 0 external crates, pure Akku code on `core`/`alloc`/`std`.
- **`fork`** — 0 external crates, trimmed & audited code maintained in-tree
  (MPL-2.0-compatible licenses only, attributed in `THIRD_PARTY_LICENSES`).
- **`adapter → native/fork`** — temporary pinned external crate as speedup,
  scheduled to be replaced behind the identical Akku API.

---

## v0.0.1 — Shipped

- `akku::html::{Html, escape}` (`native`)
- `akku::http::{App, Mount, Request, Response, Path, Result}` (`adapter` over axum 0.8)
- attribute macros `get`, `post`, `main` (`akku-drive`)
- `akku new <name>` scaffolder (`akku-cli`)
- namespace constitution, concept paper, API docs
- published to crates.io

---

## Wave 1 — Everyday Batteries & Server Boundary (~25 modules)

Goal: build real CLI tools, services, and `dree` using only `akku` + `std`, with
most modules already **`native` (0 deps)**.

### Server Boundary (v0.1 — `dree` compatibility)

- [ ] `http` (`adapter → native`): own `Request` and `Response` types instead of
  re-exporting axum types; host, scheme, cookies, raw query, body accessors;
  streaming response bodies; fallback routes; server config and graceful
  shutdown; thin middleware/around hook; `dree` runs on `akku::http` without
  importing axum.
- [ ] `http::client` (`adapter → native`, + `wasm32` `fetch` backend).

### Zero-Dep Core & Data Formats (`native`)

- [x] `error` (`native`) — unified `Error`/`Result` with context & chains
- [x] `bytes` (`native`) — byte buffers, endianness, bit helpers
- [x] `id` (`native`) — UUID (v4/v7), ULID, NanoID
- [x] `base64` (`native`) — standard & URL-safe Base64
- [x] `hex` (`native`) — hex encoding/decoding
- [x] `url` (`native`) — URL/URI parsing and query builder
- [x] `mime` (`native`) — MIME types and detection
- [x] `json` (`native`) — `Value`, parse/stringify, typed mapping
- [x] `csv` (`native`) — streaming reader/writer, headers, typed rows
- [x] `toml` (`native`) — TOML reader/writer
- [x] `ini` (`native`) — INI reader/writer
- [x] `dotenv` (`native`) — `.env` parser
- [x] `config` (`native`) — layered config (defaults → file → env)

### System, CLI, Time & Math (`native`)

- [x] `cli` (`native`) — argument parser, subcommands, auto-help
- [x] `term` (`native`) — ANSI colors, tables, progress bars, prompts
- [x] `log` (`native`) — structured logging (human + JSON)
- [x] `fs` (`native`) — `std::fs` superset + `walk`, `glob`, `tempfile`, atomic write
- [x] `env` (`native`) — typed env vars & OS standard dirs (config/cache/data)
- [x] `process` (`native`) — ergonomic commands, pipes (signals still open)
- [x] `test` (`native`) — HTTP test client, tempdir, snapshot assertions
- [x] `time` (`native`) — `clock`, `datetime`, `duration`, `timezone`
- [x] `units` (`native`) — dimensional units (`distance`, `mass`, `time`, `temp`, `energy`, `speed`, `pressure`)
- [x] `math` (`native`) — `decimal`, `linear`, `special`, `stat`

### Repo & Catalog Infrastructure

- [x] Per-module `meta.toml` (tags, backend status, `wasm32` support, owner)
- [ ] Generated `src/modules.rs`, `CODEOWNERS`, and GitHub Pages tag catalog
- [ ] Feature matrix in CI: minimal, default, domain features, bundles (`web`, `cli`, `data`, `sci`)

---

## Wave 2 — Web Protocols, Storage, Async & Security (~17 modules/families)

Goal: production web backends, data pipelines, and WASM-ready clients.

- [x] `ws` (`adapter → native`, + `wasm32` `WebSocket` client) — `server`, `client`, `Message`
- [x] `sse` (`native`, + `wasm32` `EventSource` client) — `server`, `client`, `Event`
- [x] `template` (`native`) — safe HTML/text templates
- [x] `markdown` (`native`) — CommonMark/GFM → AST/HTML
- [x] `xml` (`native`) — XML reader/writer
- [x] `yaml` (`native`) — YAML reader/writer
- [x] `text` (`native`) — Unicode helpers, graphemes, normalization, `diff`
- [x] `regex` (`fork` or `native`) — linear-time regular expressions
- [x] `archive` (`fork` / `native`) — `gzip`, `zstd`, `zip`, `tar`
- [ ] `crypto` (`fork` / `adapter`) — `hash`, `rand`, `aead`, `sign`, `kem`, `kdf` (audited only)
- [ ] `auth` (`native` over `crypto`) — `password` (Argon2id), `jwt`, `oauth2`, `totp`, `webauthn`, `session`, `acl`
- [ ] `tls` (`adapter → fork`) — TLS config, streams, certificates (`#[doc(alias = "ssl")]`; never an `ssl` module)
- [ ] `sql` (`adapter → native/fork`) — unified SQL API, pool, `migrate`, `sql::sqlite`, `sql::postgres`
- [x] `kv` (`native` / `adapter`) — embedded KV store + `kv::redis`
- [x] `cache` (`native`) — in-memory LRU/TTL cache
- [ ] `task` & `sync` (`adapter → native`, + `wasm32` spawn) — async tasks, channels, semaphore, `schedule`
- [ ] `net`, `dns`, `quic` (`native` / `adapter`) — flat modules: `net` (`std::net` superset: IP, TCP/UDP/Unix sockets, async), `dns`, `quic`
- [ ] `xdr`, `vfs`, `nfs` (megaghost-driven) — `xdr` (XDR, RFC 4506; shipped in PR #18), `vfs` (virtual filesystem trait layer), `nfs` (`server`/`client` over `vfs`; ONC-RPC internal; read-only `server` first)

---

## Wave 3 — Media, Erlang-Style Distribution & Rust Power (~20 modules/families)

Goal: cover creative tools (DAW, image/video editors), distributed systems,
science, GPU, and WebAssembly hosts.

### Separated Media Families (DAW / Image / Video)

- [ ] `audio` (`native` / OS-bindings) — `pcm`, `wav`, `flac`, `dsp`, `midi`, `device`
- [ ] `pic` (`native` / `fork`) — `png`, `jpeg`, `webp`, `bmp`, `canvas`, `filter`
- [ ] `vid` (`adapter → fork/native`) — `frame`, `codec`, `mux`, `stream`
- [ ] `color` (`native`) — RGB, HSL, OKLCH, hex, contrast
- [ ] `pdf` (`native`) — PDF generation/reading

### Erlang-Style Concurrency & Wire Formats

- [ ] `actor` (`native`) — `supervisor`, `mailbox`, `registry`
- [ ] `cluster` (`native`) — node membership, gossip, discovery
- [ ] `broker` (`native`) — pub/sub messaging
- [ ] `protobuf` (`native`, + `build-dependencies` codegen)
- [ ] `flatbuffers` (`native`, zero-copy)
- [ ] `msgpack` & `cbor` (`native`)

### Protocols, Science, GPU, WASM & UI

- [ ] `webpush`, `rpc`, `mail`, `mqtt`, `ssh`
- [ ] `gpu` (`adapter → native`, + `wasm32` WebGPU) — `compute`, `render`
- [ ] `wasm` (`adapter → fork/native`) — `host` (plugin runtime) & `guest` helpers
- [ ] `ui` & `webview` — native + `wasm32` UI and WebView apps
- [ ] `phy` (`native`) — `mechanics`, `relativity`, `quantum`, `thermo`, `cosmology`
- [ ] `algo` ✅, `geo`, `dataframe`, `chem`, `bio`, `econ`, `i18n`, `tui`, `trace`, `bench`

### Native HTTP Milestone

- [ ] Native HTTP/1.1 core behind the existing `akku::http` API, benchmarked
  against direct axum; drop `axum`/`tokio` default dependency once parity holds.

---

## Non-Goals

- Replacing Rust's `core`/`alloc`/`std`.
- Splitting into 20+ or 300 separate library crates.
- Global top-level items (`akku::get`, `akku::Request`, `akku::Error`).
- An insecure `akku::ssl` module or a `forwasm::` parallel namespace.
- Unaudited, from-scratch cryptography or GPL-licensed code forks.
