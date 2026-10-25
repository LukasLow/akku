# Architecture

## One dependency, many domains

Applications add `akku`. Domains are modules, not separate public crates:

```text
akku::html
akku::http
akku::json     (planned)
akku::time     (planned)
akku::units    (planned)
```

The proc-macro implementation lives in `akku-drive`; `akku` re-exports it under
`akku::http`. `akku-cli` is a small scaffolder and is not required at runtime.

## Crates

| Crate | Kind | Responsibility |
|---|---|---|
| `akku` | library | HTML, HTTP facade, public namespace |
| `akku-drive` | proc-macro | `#[akku::http::get]`, `#[akku::http::post]`, `#[akku::http::main]` |
| `akku-cli` | binary | `akku new <name>` |

## HTTP layer

v0.0.1 is an axum 0.8 facade:

- `akku::http::App` owns an axum `Router`.
- `#[akku::http::get]` generates a hidden handler and a public unit type
  implementing `akku::http::Mount`.
- `App::mount` adds that route.
- `App::serve` binds a listener and runs axum.
- `App::into_axum()` is the escape hatch.

The public contract is Akku's types and mount boundary. The axum router is an
implementation detail, except at the documented escape hatch.

## Feature and dependency policy

- `http` is the only feature in v0.0.1 and is enabled by default.
- `default-features = false` gives the HTML-only core without axum/tokio.
- Direct dependency versions are pinned in the workspace.
- Crypto and TLS are never reimplemented in Akku. Use audited crates when those
  domains arrive.
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
