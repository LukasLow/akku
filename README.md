# Akku

Akku is a pragmatic Rust battery pack: one dependency, one namespace, clear
APIs, opt-in power. It is **not** a replacement for the Rust standard library.

```toml
[dependencies]
akku = "0.0.1"
```

```rust
use akku::html::Html;
use akku::http::{App, Path, Result};

#[akku::http::get("/")]
async fn home() -> Html {
    Html::new("<h1>Hello from Akku</h1>")
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

Run it:

```sh
cargo run
# http://127.0.0.1:3000/
# http://127.0.0.1:3000/users/7
```

## Status

`0.0.1` is published:

- [akku](https://crates.io/crates/akku)
- [akku-drive](https://crates.io/crates/akku-drive)
- [akku-cli](https://crates.io/crates/akku-cli)

It is deliberately small:

- `akku::html` — trusted HTML and escaping
- `akku::http` — attribute routes, `App`, `Request`, `Response`, `Path`, `Result`
- `akku-drive` — the attribute macros behind `akku::http`
- `akku-cli` — `akku new <name>`

The HTTP backend is axum 0.8 behind the Akku API. `App::into_axum` is the
explicit escape hatch; the public Akku API is the stable surface.

## Namespace

Akku uses flat domain modules, not one crate per concept:

```text
akku::html
akku::http
akku::json     (planned)
akku::time     (planned)
akku::units    (planned)
```

There is no global `akku::get`, `akku::Value` or `akku::Server`. The rules and
the reserved names are in [`Namespace.md`](Namespace.md).

## Crates

| Crate | Role |
|---|---|
| `akku` | The only dependency users normally add |
| `akku-drive` | Proc-macros; re-exported by `akku::http` |
| `akku-cli` | `akku new <name>` project scaffolder |

## Documentation

- [Concept](CONCEPT.md)
- [Namespace](Namespace.md)
- [HTTP API](docs/API.md)
- [Security](SECURITY.md)

## License

Apache-2.0.
