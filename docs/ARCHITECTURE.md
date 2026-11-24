# Architecture

## One library crate, flat modules + dynamic families

Applications add `akku`. There is **only one library crate** (`akku`) — no
20+ internal crates and no crate-per-domain zoo. Modules start flat by default
(`akku::<name>`) and group into dynamic families when closely related modules
grow together:

```text
# Flat modules (examples)
akku::html      akku::http      akku::ws        akku::sse
akku::json      akku::csv       akku::toml      akku::sql
akku::tls       akku::fs        akku::cli       akku::task

# Families (examples)
akku::archive   akku::auth      akku::audio     akku::vid       akku::pic
akku::net       akku::crypto    akku::time      akku::units     akku::math
akku::phy       akku::actor     akku::gpu       akku::wasm
```

See [`Namespace.md`](../Namespace.md) for the complete naming rules.

The proc-macro implementation lives in `akku-drive` (required by Rust); `akku`
re-exports it under `akku::http`. `akku-cli` is a small scaffolder and is not
required at runtime.

## Crates

| Crate | Kind | Responsibility |
|---|---|---|
| `akku` | library | The single library crate: all modules, families, and public namespace |
| `akku-drive` | proc-macro | `#[akku::http::get]`, `#[akku::http::post]`, `#[akku::http::main]` |
| `akku-cli` | binary | `akku new <name>` |

## HTTP layer

v0.0.1 is a temporary `adapter` over axum 0.8:

- `akku::http::App` owns an axum `Router`.
- `#[akku::http::get]` generates a hidden handler and a public unit type
  implementing `akku::http::Mount`.
- `App::mount` adds that route.
- `App::serve` binds a listener and runs axum.
- `App::into_axum()` is the escape hatch while the axum adapter is active.

The public contract is Akku's types and mount boundary. The axum router is an
implementation detail so a `native` HTTP core can replace it without breaking
user code.

## Feature, dependency, and WASM policy

- **Zero-dep end goal:** External dependencies (`adapter`) are temporary
  speedups, pinned in the workspace and gradually replaced by `native` Akku code
  or license-clean, audited `fork`s (MPL-2.0-compatible licenses only).
- **Crypto and TLS:** Never blindly reimplemented from scratch; use audited
  forks or pinned adapters. No `akku::ssl` module.
- **Minimal features:** Zero-dep everyday modules stay lightweight and
  accessible without feature friction; coarse domain features gate heavy,
  OS-dependent, or adapter-backed modules (`http`, `sql`, `audio`, `vid`, `pic`,
  `gpu`, `wasm`, …) to keep compile times fast and `wasm32` builds clean.
- **WASM model:** `wasm32` target support lives transparently behind
  `#[cfg(target_arch = "wasm32")]` inside client/compute modules (never a
  `forwasm::` namespace). `akku::wasm` is exclusively the host/guest runtime.
- **Ownership without extra crates:** Folder-level `CODEOWNERS` and per-module
  `meta.toml` files drive module registration and the tag-based catalog.
- Macro code must stay a thin layer over a pleasant manual API.

## dree compatibility

dree can build on Akku once the HTTP boundary exposes:

- host, scheme, query, headers, cookies and body on requests,
- path parameters and method matching,
- status, multiple `Set-Cookie` headers and streaming bodies,
- fallback routes,
- server configuration and graceful shutdown.

v0.0.1 covers the route/mount/server path. Richer request/response and server
configuration are the next compatibility step.
