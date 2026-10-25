# Akku HTTP API — v0.0.1

The first API is deliberately small and attribute-first:

```rust
use akku::html::Html;
use akku::http::{App, Path, Result};

#[akku::http::get("/")]
async fn home() -> Html {
    Html::new("<h1>Hello</h1>")
}

#[akku::http::get("/users/{id}")]
async fn user(Path(id): Path<u64>) -> Html {
    Html::new(format!("<h1>User {id}</h1>"))
}

#[akku::http::main]
async fn main() -> Result<()> {
    App::new()
        .mount(home)
        .mount(user)
        .serve("127.0.0.1:3000")
        .await
}
```

## Names

Every public name has a domain:

```
akku::html::Html
akku::html::escape
akku::http::App
akku::http::Mount
akku::http::Request
akku::http::Response
akku::http::Path
akku::http::Result
akku::http::get
akku::http::post
akku::http::main
```

There is no `akku::get` or `akku::Html`. See [`../Namespace.md`](../Namespace.md).

## The route attribute

```rust
#[akku::http::get("/users/{id}")]
async fn user(Path(id): Path<u64>) -> Html { /* … */ }
```

The macro:

1. renames the async function to a hidden handler,
2. creates a unit type with the function's name,
3. implements `akku::http::Mount` for that type,
4. registers `GET /users/{id}` with axum.

`#[akku::http::post("/path")]` is the same for POST.

The generated type is what `App::mount` accepts:

```rust
App::new().mount(user)
```

The handler body stays ordinary Rust and can use axum extractors re-exported by
`akku::http`, such as `Path` and `Request`.

## The main attribute

```rust
#[akku::http::main]
async fn main() -> Result<()> {
    App::new().serve("127.0.0.1:3000").await
}
```

It runs the async body on a multi-thread Tokio runtime. Use a normal
`#[tokio::main]` and `App::into_axum()` when you need a custom runtime.

## App

```rust
pub struct App { /* … */ }

impl App {
    pub fn new() -> Self;
    pub fn mount(self, route: impl Mount) -> Self;
    pub fn into_axum(self) -> axum::Router;
    pub async fn serve(self, address: &str) -> Result<()>;
}
```

- `new` starts empty.
- `mount` adds a generated route or a hand-written `Mount`.
- `into_axum` is the escape hatch to the full axum router.
- `serve` binds a `TcpListener` and runs axum.

## Request and Response

`Request`, `Response` and `Path` are re-exported axum types in v0.0.1:

```rust
use akku::html::{escape, Html};
use akku::http::Request;

async fn inspect(request: Request) -> Html {
    let host = request
        .headers()
        .get("host")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("unknown");
    Html::new(format!("host: {}", escape(host)))
}
```

This keeps the first release honest: the HTTP engine is axum, but applications
do not import axum directly for the normal path. The types are part of Akku's
public namespace and can be replaced by Akku-owned types in a later major
version without changing the import path.

## Html

```rust
use akku::html::{escape, Html};

let safe = escape("<script>alert(1)</script>");
let trusted = Html::new("<h1>Hello</h1>");
```

`Html` is trusted markup and is not escaped again. `escape` returns escaped
text. `Html` implements axum's `IntoResponse` when the `http` feature is on.

## Result and Error

```rust
pub type Error = Box<dyn std::error::Error + Send + Sync + 'static>;
pub type Result<T> = std::result::Result<T, Error>;
```

The boxed error keeps `?` usable with `std`, axum and user errors in the first
release. A richer error enum can come later without changing the macro surface.

## Features

```toml
[dependencies]
akku = "0.0.1" # http is default

# HTML only, no axum/tokio:
akku = { version = "0.0.1", default-features = false }
```

The only feature in v0.0.1 is `http`. Future domains get their own features.

## dree compatibility requirements

For dree to build on Akku, the HTTP layer must eventually expose:

- host, scheme, query, headers, cookies and body on `Request`,
- path parameters and method matching,
- status, multiple `Set-Cookie` headers and streaming bodies on `Response`,
- fallback routes,
- a stable `Mount`/handler boundary,
- graceful shutdown and server configuration.

The v0.0.1 API covers the route/mount/server path. Request/response streaming
and richer server configuration are the next compatibility step.

## Non-goals for v0.0.1

- Own HTTP parser, TLS or async runtime.
- Middleware layers.
- JSON/form extractors.
- A second server implementation.
- Macros beyond `get`, `post` and `main`.

Those are deliberate next steps, not forgotten features.
