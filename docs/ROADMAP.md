# Roadmap

## v0.0.1 — shipped

- `akku::html::{Html, escape}`
- `akku::http::{App, Mount, Request, Response, Path, Result}`
- attribute macros `get`, `post`, `main`
- `akku new <name>` scaffolder
- namespace constitution, concept paper, API docs
- published to crates.io

## v0.1 — server boundary

- [ ] Own `Request` and `Response` types instead of re-exporting axum types.
- [ ] Host, scheme, cookies, raw query and body accessors.
- [ ] Streaming response bodies for SSE.
- [ ] Fallback routes.
- [ ] Server configuration and graceful shutdown.
- [ ] Middleware/around hook with a small, documented surface.
- [ ] dree can use `akku::http` without importing axum.

## v0.2 — first non-HTTP domains

- [ ] `akku::json` over `serde_json`.
- [ ] `akku::units` as a small zero-dependency domain.
- [ ] `akku::math` as a small zero-dependency domain.
- [ ] Feature matrix in CI: minimal, default, each domain, full.
- [ ] Compile-time examples for every domain.

## Later

- [ ] `akku::time`, `akku::xml`, `akku::tls` after a concrete need.
- [ ] An experimental native HTTP/1.1 core behind the same API.
- [ ] More macros only where the manual API is demonstrably too repetitive.
- [ ] A performance comparison against direct axum and Svelte/Bun.

## Non-goals

- Replacing the Rust standard library.
- Reimplementing TLS or cryptography.
- A crate per domain.
- A global `akku::get` namespace.
