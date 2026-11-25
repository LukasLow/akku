//! Akku is a pragmatic battery pack for Rust.
//!
//! The public surface is intentionally flat and domain-namespaced — every module
//! owns its names (`akku::json::Value`, `akku::toml::Value`, `akku::http::App`).
//! There is no global `akku::get` or `akku::Value`. See the repository's
//! `Namespace.md` and `docs/CATALOG.md` for the full rules and module catalog.
//!
//! # Wave 1 modules
//!
//! - **Formats & encoding:** [`base64`], [`csv`], [`dotenv`], [`hex`], [`html`], [`ini`], [`json`], [`mime`], [`toml`], [`url`]
//! - **Config & core primitives:** [`bytes`], [`config`], [`error`], [`id`], [`math`], [`time`], [`units`]
//! - **OS, CLI & observability:** [`cli`], [`env`](mod@env), [`fs`], [`log`], [`process`], [`term`], [`test`](mod@test)
//! - **Web (`http` feature, enabled by default):** `http`
//!
//! # Quick start
//!
//! ```ignore
//! use akku::html::Html;
//! use akku::http::{App, Path, Result};
//!
//! #[akku::http::get("/")]
//! async fn home() -> Html {
//!     Html::new("<h1>Hello from Akku</h1>")
//! }
//!
//! #[akku::http::get("/users/{id}")]
//! async fn user(Path(id): Path<u64>) -> Html {
//!     Html::new(format!("<h1>User {id}</h1>"))
//! }
//!
//! #[akku::http::main]
//! async fn main() -> Result<()> {
//!     App::new()
//!         .mount(home)
//!         .mount(user)
//!         .serve("127.0.0.1:3000")
//!         .await
//! }
//! ```
//!
//! The `http` feature is enabled by default. Disable it for the zero-dependency
//! core (all 24 native Wave 1 modules):
//!
//! ```toml
//! akku = { version = "0.0.1", default-features = false }
//! ```

// `unsafe` is only allowed in allowlisted `sys` submodules (see
// `.github/unsafe-allowlist.txt`), which opt in with `#![allow(unsafe_code)]`.
#![deny(unsafe_code)]
#![deny(unsafe_op_in_unsafe_fn)]
#![deny(clippy::undocumented_unsafe_blocks)]
#![deny(missing_docs)]

pub mod algo;
pub mod archive;
pub mod base64;
pub mod bytes;
pub mod cache;
pub mod cli;
pub mod config;
pub mod csv;
pub mod dotenv;
pub mod env;
pub mod error;
pub mod fs;
pub mod hex;
pub mod html;

#[cfg(feature = "http")]
pub mod http;

pub mod id;
pub mod ini;
pub mod json;
pub mod kv;
pub mod log;
pub mod markdown;
pub mod math;
pub mod mime;
pub mod process;
pub mod regex;
pub mod sse;
pub mod template;
pub mod term;
pub mod test;
pub mod text;
pub mod time;
pub mod toml;
pub mod units;
pub mod url;
pub mod ws;
pub mod xdr;
pub mod xml;
pub mod yaml;

pub use crate::text::diff;

/// The small, curated set of names most applications need.
///
/// The prelude is convenience, not the namespace. Anything not here stays
/// available at its full domain path.
pub mod prelude {
    pub use crate::html::Html;

    #[cfg(feature = "http")]
    pub use crate::http::{get, main, post, App, Path, Result};
}
