//! Akku is a pragmatic battery pack for Rust.
//!
//! The public surface is intentionally namespaced:
//!
//! ```text
//! akku::html
//! akku::http
//! ```
//!
//! A domain owns its names. There is no global `akku::get` or `akku::Value`.
//! See the repository's `Namespace.md` for the naming rules.
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
//! The `http` feature is enabled by default. Disable it for the HTML-only
//! core:
//!
//! ```toml
//! akku = { version = "0.0.1", default-features = false }
//! ```

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod html;

#[cfg(feature = "http")]
pub mod http;

/// The small, curated set of names most applications need.
///
/// The prelude is convenience, not the namespace. Anything not here stays
/// available at its full domain path.
pub mod prelude {
    pub use crate::html::Html;

    #[cfg(feature = "http")]
    pub use crate::http::{get, main, post, App, Path, Result};
}
