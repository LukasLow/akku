//! HTTP routing, server boundary, core types, and HTTP/1.1 client.
//!
//! The v0.1 implementation keeps axum 0.8 behind a small, stable Akku API for
//! serving ([`App`], [`Path`], [`Request`], [`Response`], [`ResponseBuilder`],
//! and the attribute macros [`get`], [`post`], [`put`], [`patch`], [`delete`],
//! and [`main`]), defines Akku-owned HTTP vocabulary types ([`Method`],
//! [`StatusCode`], [`Headers`]), and ships a zero-dependency HTTP/1.1 client
//! under [`client`]. The raw axum router remains available through
//! [`App::into_axum`] as an explicit escape hatch.
//!
//! ```no_run
//! use akku::html::Html;
//! use akku::http::{App, Path, Result};
//!
//! #[akku::http::get("/users/{id}")]
//! async fn user(Path(id): Path<u64>) -> Html {
//!     Html::new(format!("<h1>User {id}</h1>"))
//! }
//!
//! #[akku::http::main]
//! async fn main() -> Result<()> {
//!     App::new().mount(user).serve("127.0.0.1:3000").await
//! }
//! ```

pub mod client;
pub mod server;
pub mod types;

pub use server::{
    __private, delete, get, main, ok_html, ok_json_raw, ok_text, patch, post, put, redirect,
    status_text, App, Error, Mount, Path, Request, Response, ResponseBuilder, Result, Router,
};
pub use types::{Headers, InvalidMethodError, Method, StatusCode};
