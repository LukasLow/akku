# akku — Concept

Status: 2026-11-24  
Repo/workspace: `akku`  
Product/crate: `akku`

Agent and maintainer rules live in [`AGENTS.md`](AGENTS.md); this document explains the product idea behind them.

## 1. Vision

akku is an **expansive standard library next to `std`** for Rust.

The target feeling:

> I start a real Rust project, my dependencies are `akku` and `std`, and everything I need for web, CLI, data formats, systems, media, or concurrency is there, consistent and pleasant.

```toml
[dependencies]
akku = "0.0.1"
```

### Long-term goal: zero external dependencies

akku's ultimate goal is **zero external dependencies**:

- External crates (`adapter`) are only a **temporary speedup** to make akku usable early (as `axum`/`tokio` are today in `akku::http`).
- Behind the same stable akku API, adapters are replaced step by step with **own code (`native`)** or **license-clean, slimmed-down forks (`fork`)**, with no breaking change for users.
- Security-critical cores (`crypto`, `tls`) get extra care: audited forks (MIT/Apache-2.0/BSD/ISC/MPL-2.0 with clean attribution) or pinned adapters until a fully audited state exists, never a quick rewrite.

## 2. Non-goals

- akku builds on `std` and extends it; it does not replace Rust's `core`/`alloc`/`std`.
- akku avoids crate sprawl (no 20+ internal or 300 external crates): there is **one library crate, `akku`**.
- The public contract is akku's own API, never a 1:1 re-export of foreign APIs (`axum`, `serde`, `tokio`).
- crates.io gets real crates only, never placeholders.
- Forks contain no GPL/AGPL code (akku is MPL-2.0).

## 3. Product shape: one library crate

```text
akku/
  crates/
    akku/        # the one library crate (all modules live under src/)
    akku-drive/  # proc-macro = true (required by Rust; re-exported by akku)
    akku-cli/    # optional binary: `akku new <name>`, `akku catalog gen`
```

- Users depend only on `akku`, including for codegen in `[build-dependencies]` (e.g. `protobuf`, `flatbuffers`) instead of extra crates.
- **Exactly three crates are published** to crates.io: `akku`, `akku-drive`, `akku-cli`. Everything else, including platform code, stays inside `akku`. A crate that published `akku` depends on must itself be on crates.io, so new crates would grow the published set.
- **Releases stay on `0.0.x` until at least 2027-Q2**, always with an explicit `0.0.N` version ([Release workflow](.agents/workflows/Release.md)).
- **Ownership without extra crates:** every module lives in its own folder `crates/akku/src/<module>/` with its own metadata file (`meta.toml`) and folder-based `CODEOWNERS` ([akku-module-layout](.agents/skills/akku-module-layout/SKILL.md)).
- **Few merge conflicts:** a generator checked in CI (`akku catalog gen`) derives shared files such as the module catalog ([`docs/CATALOG.md`](docs/CATALOG.md), later a GitHub Pages tag catalog) from the module metadata.

## 4. Namespace principle: flat by default, dynamic families

The full rules are in [`Namespace.md`](Namespace.md). In short:

1. **Flat by default** (`akku::http`, `akku::ws`, `akku::json`, `akku::csv`, `akku::tls`, `akku::sql`, `akku::fs`, `akku::net`, `akku::cli`, `akku::base64`, `akku::protobuf`, `akku::flatbuffers`, …).
2. **Dynamic families:** when related modules grow organically, they are grouped into a family (directly before `1.0`; from `1.0` on through `#[deprecated] pub use` transition aliases).
3. **Short, crisp names** for families and modules:
   - `archive/`, `auth/`, `audio/`, `vid/`, `pic/` (media types strictly separate: a DAW needs no `pic`/`vid`, an image editor no `audio`)
   - `crypto/`, `time/`, `units/`, `math/`, `phy/`, `actor/`, `gpu/`, `wasm/`, `os/`
4. **Server/client always inside** (`akku::http::{server, client}`, `akku::ws::{server, client}`, `akku::sse::{server, client}`).
5. **Discovery through tags, not a deep taxonomy:** cross-cutting views (`#web`, `#cli`, `#wasm32`, `#native-0dep`, `#adapter`, `#no_std`, `#maintainer:...`) come from a catalog (GitHub Pages), not from a four-level module tree.

## 5. Minimal features and compile times

The Rust linker plus LTO removes unused code from the final binary. Features are therefore used **sparingly**: not per tiny module, but as a guard rail for compile time, platform builds (`wasm32`), and transitional adapters.

- **Zero-dep everyday modules** (`error`, `bytes`, `id`, `base64`, `hex`, `url`, `html`, `json`, `csv`, `toml`, `units`, `math` core, …) work directly, with no feature juggling.
- **Heavy, OS-dependent, or still-`adapter` domains** (`http`, `ws`, `sql`, `tls`, `crypto`, `auth`, `archive`, `audio`, `vid`, `pic`, `gpu`, `wasm`, …) sit behind coarse domain features (about 30–60 in total, far below the crates.io limit of 300).
- **Convenience bundles** (`web`, `cli`, `data`, `sci`, `full`) group typical project setups.
- `cargo tree -e features -p akku` remains the check for the actual dependency graph.

## 6. WASM model (strictly two-part)

1. **Code that runs *in* WASM (`wasm32` target):**
   - There is no separate `forwasm::` or `async::wasm` module.
   - Client and compute modules (`http::client`, `ws::client`, `sse::client`, `gpu`, `json`, `url`, `time`, `units`, `math`, `pic`, …) support `wasm32` transparently behind the same API via `#[cfg(target_arch = "wasm32")]`.
   - Native-only server and OS parts (`http::server`, `ws::server`, `net` sockets, `process`, `fs::watch`, `os`) are hidden automatically on `wasm32`.
2. **Code that runs WASM modules itself (host/guest runtime):**
   - Lives in its own module `akku::wasm` (`wasm::host`, `wasm::guest`).

## 7. Backend lifecycle and license policy

Every module carries an explicit backend status:

| Status | Meaning | Example |
|---|---|---|
| `native` | 0 external crates, pure akku code on `core`/`alloc`/`std` | `html`, `bytes`, `error`, `base64`, `csv`, `json`, `toml`, `units`, `math` |
| `fork` | 0 external crates, deliberately adopted and maintained code in the repo | `crypto`, `tls`, `regex`, `archive`, `pic` |
| `adapter` | temporary external crate (exactly pinned) as a speedup until replaced | `http` (v0 on `axum`/`tokio`), early DB/audio/GPU bindings |

Rules:

- Temporary adapter dependencies are **exactly pinned** (`axum = "=0.8.9"`), checked with `cargo deny`/`cargo audit`, and hidden behind akku's own API.
- Forks come only from MPL-2.0-compatible licenses (MIT, Apache-2.0, BSD, ISC, MPL-2.0), with documented origin and the license text in `THIRD_PARTY_LICENSES`.
- Security-critical cores (`crypto`, `tls`) are not rewritten untested for prestige; audited forks and adapters take precedence.

### `unsafe`

`unsafe` is allowed only where no safe way exists (FFI, OS syscalls, sockets, platform APIs), never for convenience or speed, and only inside designated `sys` submodules with a `// SAFETY:` comment on every block. Today no module needs it and the crate is `#![forbid(unsafe_code)]`. Policy and mechanics: [AGENTS.md](AGENTS.md) rule 9 and [akku-unsafe](.agents/skills/akku-unsafe/SKILL.md).

## 8. API principles

1. **User-first.** Design the user code first, then the implementation.
2. **One obvious way.** No trait maze for the common case.
3. **Swappable backends.** The public akku API stays stable when a module moves from `adapter` to `native` or `fork`.
4. **Escape hatch where needed.** While an adapter sits underneath (e.g. `App::into_axum`), the way out stays explicitly documented.
5. **Macros are sugar, not a requirement.** The manual API is pleasant without macros.
6. **Safe, fast defaults.** No insecure legacy module at the top level (e.g. no `akku::ssl`).
7. **No Rust keywords.** Module names are never reserved keywords (`task` instead of `async`; no `gen`, `try`, `box`).

## 9. Success criteria

akku succeeds when:

- a real project needs only `akku` + `std` in its `Cargo.toml`,
- Wave 1 everyday modules (`html`, `json`, `csv`, `toml`, `url`, `base64`, `hex`, `cli`, `log`, `time`, `units`, `math`, …) run with **0 external dependencies**,
- the public API stays stable across `adapter` → `native`/`fork` switches,
- client code (`http::client`, `ws::client`, `json`, `units`, …) compiles on `wasm32` without special imports,
- `cargo test`, `cargo clippy`, `cargo doc`, and `cargo deny`/`audit` are green,
- the library can grow cleanly from ~58 core modules to 300+ along a clear roadmap ([`docs/ROADMAP.md`](docs/ROADMAP.md)).
