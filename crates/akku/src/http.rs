//! HTTP routing and serving.
//!
//! The v0.0.1 implementation uses axum 0.8 behind a small, stable Akku API.
//! Users normally see [`App`], [`Path`], [`Request`], [`Response`] and the
//! attribute macros [`get`], [`post`] and [`main`]. The raw axum router remains
//! available through [`App::into_axum`] as an escape hatch.
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

pub use akku_drive::{get, main, post};
pub use axum::extract::{Path, Request};
pub use axum::response::Response;
pub use axum::Router;

/// The error type used by the convenience server entry points.
///
/// It is deliberately the standard boxed error: `?` works with `std::io`,
/// axum and user errors without an Akku-specific conversion layer.
pub type Error = Box<dyn std::error::Error + Send + Sync + 'static>;

/// A result with [`Error`] as the error type.
pub type Result<T> = std::result::Result<T, Error>;

/// A route that can be added to an [`App`].
///
/// The attribute macros generate an implementation. Hand-written routes can
/// implement this trait directly, which keeps the router extensible without
/// exposing axum in the normal API.
pub trait Mount {
    /// Add this route to an axum router.
    fn mount(self, router: Router) -> Router;
}

/// A small HTTP application.
///
/// `App` owns the route table. It is consumed by [`App::serve`], or converted
/// with [`App::into_axum`] when an adapter needs the underlying router.
pub struct App {
    router: Router,
}

impl App {
    /// Create an empty application.
    pub fn new() -> Self {
        Self {
            router: Router::new(),
        }
    }

    /// Add a route produced by an attribute macro or a custom [`Mount`].
    pub fn mount(mut self, route: impl Mount) -> Self {
        self.router = route.mount(self.router);
        self
    }

    /// Return the underlying axum router.
    ///
    /// This is the explicit escape hatch for middleware, state or adapters that
    /// need the full axum surface.
    pub fn into_axum(self) -> Router {
        self.router
    }

    /// Serve the application on `address` until the process ends.
    pub async fn serve(self, address: &str) -> Result<()> {
        let listener = tokio::net::TcpListener::bind(address).await?;
        axum::serve(listener, self.router).await?;
        Ok(())
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

/// Implementation details used by the generated macro code.
///
/// This module is public because proc-macro expansions run in the caller's
/// crate. It is not part of the stable API; do not use it directly.
#[doc(hidden)]
#[allow(missing_docs)]
pub mod __private {
    use std::future::Future;

    pub use axum::routing::{get, post};
    pub use axum::Router;

    /// Run an async entry point on a multi-thread Tokio runtime.
    pub fn run<F: Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Akku: failed to build the Tokio runtime")
            .block_on(future)
    }
}
