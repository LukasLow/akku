# akku-drive

The proc-macro crate behind Akku's attribute-first HTTP API.

Do not add this crate directly unless you are building tooling around Akku.
`akku` re-exports the macros under `akku::http`:

```rust
#[akku::http::get("/users/{id}")]
async fn user(Path(id): Path<u64>) -> Html { /* … */ }

#[akku::http::main]
async fn main() -> akku::http::Result<()> { /* … */ }
```

The macro re-export keeps one dependency for applications.
