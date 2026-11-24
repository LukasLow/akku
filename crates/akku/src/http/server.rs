//! HTTP routing, response helpers, and server entry points.
//!
//! The v0.1 server boundary keeps axum 0.8 behind a small, stable Akku API.
//! Users normally see [`App`], [`Path`], [`Request`], [`Response`],
//! [`ResponseBuilder`], and the attribute macros [`get`], [`post`], [`put`],
//! [`patch`], [`delete`], and [`main`]. The raw axum router remains available
//! through [`App::into_axum`] as an explicit escape hatch.

pub use akku_drive::{delete, get, main, patch, post, put};
pub use axum::extract::{Path, Request};
pub use axum::response::Response;
pub use axum::Router;

use axum::body::Body;
use axum::handler::Handler;
use axum::http::header::{HeaderName, HeaderValue};

use crate::http::types::{Headers, StatusCode};

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
/// `App` owns the route table. It is consumed by [`App::serve`] or
/// [`App::serve_listener`], or converted with [`App::into_axum`] when an
/// adapter needs the underlying router.
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

    /// Register a `GET` handler directly on `path`.
    pub fn get<H, T>(mut self, path: &str, handler: H) -> Self
    where
        H: Handler<T, ()>,
        T: 'static,
    {
        self.router = self.router.route(path, axum::routing::get(handler));
        self
    }

    /// Register a `POST` handler directly on `path`.
    pub fn post<H, T>(mut self, path: &str, handler: H) -> Self
    where
        H: Handler<T, ()>,
        T: 'static,
    {
        self.router = self.router.route(path, axum::routing::post(handler));
        self
    }

    /// Register a `PUT` handler directly on `path`.
    pub fn put<H, T>(mut self, path: &str, handler: H) -> Self
    where
        H: Handler<T, ()>,
        T: 'static,
    {
        self.router = self.router.route(path, axum::routing::put(handler));
        self
    }

    /// Register a `PATCH` handler directly on `path`.
    pub fn patch<H, T>(mut self, path: &str, handler: H) -> Self
    where
        H: Handler<T, ()>,
        T: 'static,
    {
        self.router = self.router.route(path, axum::routing::patch(handler));
        self
    }

    /// Register a `DELETE` handler directly on `path`.
    pub fn delete<H, T>(mut self, path: &str, handler: H) -> Self
    where
        H: Handler<T, ()>,
        T: 'static,
    {
        self.router = self.router.route(path, axum::routing::delete(handler));
        self
    }

    /// Register a `HEAD` handler directly on `path`.
    pub fn head<H, T>(mut self, path: &str, handler: H) -> Self
    where
        H: Handler<T, ()>,
        T: 'static,
    {
        self.router = self.router.route(path, axum::routing::head(handler));
        self
    }

    /// Nest another [`App`] under `prefix` (e.g. `"/api"`).
    pub fn nest(mut self, prefix: &str, other: App) -> Self {
        self.router = self.router.nest(prefix, other.router);
        self
    }

    /// Merge all routes from `other` into this [`App`].
    pub fn merge(mut self, other: App) -> Self {
        self.router = self.router.merge(other.router);
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
        self.serve_listener(listener).await
    }

    /// Serve the application on an already-bound [`tokio::net::TcpListener`].
    pub async fn serve_listener(self, listener: tokio::net::TcpListener) -> Result<()> {
        axum::serve(listener, self.router).await?;
        Ok(())
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

/// Builder for constructing a [`Response`] without depending on axum builder types.
#[derive(Debug, Clone)]
pub struct ResponseBuilder {
    status: StatusCode,
    headers: Headers,
}

impl ResponseBuilder {
    /// Starts a new response with the given [`StatusCode`].
    pub fn new(status: StatusCode) -> Self {
        Self {
            status,
            headers: Headers::new(),
        }
    }

    /// Appends a header to the response.
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.append(name, value);
        self
    }

    /// Appends all entries from `headers` to the response.
    pub fn headers(mut self, headers: Headers) -> Self {
        for (name, value) in headers {
            self.headers.append(name, value);
        }
        self
    }

    /// Sets the `content-type` header (replacing any existing `content-type`).
    pub fn content_type(mut self, content_type: impl Into<String>) -> Self {
        self.headers.insert("content-type", content_type);
        self
    }

    /// Finishes the response with raw bytes as the body.
    pub fn body(self, body: impl Into<Vec<u8>>) -> Response {
        build_axum_response(self.status, &self.headers, body.into())
    }

    /// Finishes the response with a UTF-8 `text/plain; charset=utf-8` body.
    pub fn text(mut self, body: impl Into<String>) -> Response {
        if !self.headers.contains("content-type") {
            self.headers
                .insert("content-type", "text/plain; charset=utf-8");
        }
        build_axum_response(self.status, &self.headers, body.into().into_bytes())
    }

    /// Finishes the response with a UTF-8 `text/html; charset=utf-8` body.
    pub fn html(mut self, body: impl Into<String>) -> Response {
        if !self.headers.contains("content-type") {
            self.headers
                .insert("content-type", "text/html; charset=utf-8");
        }
        build_axum_response(self.status, &self.headers, body.into().into_bytes())
    }

    /// Finishes the response with a raw JSON string (`application/json`).
    pub fn json_raw(mut self, body: impl Into<String>) -> Response {
        if !self.headers.contains("content-type") {
            self.headers.insert("content-type", "application/json");
        }
        build_axum_response(self.status, &self.headers, body.into().into_bytes())
    }

    /// Finishes the response with an empty body.
    pub fn empty(self) -> Response {
        build_axum_response(self.status, &self.headers, Vec::new())
    }
}

fn build_axum_response(status: StatusCode, headers: &Headers, body: Vec<u8>) -> Response {
    let mut builder = Response::builder();
    let axum_status = axum::http::StatusCode::from_u16(status.as_u16())
        .unwrap_or(axum::http::StatusCode::INTERNAL_SERVER_ERROR);
    builder = builder.status(axum_status);
    for (name, value) in headers.iter() {
        if let (Ok(hn), Ok(hv)) = (
            HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_str(value),
        ) {
            builder = builder.header(hn, hv);
        }
    }
    builder
        .body(Body::from(body))
        .unwrap_or_else(|_| Response::new(Body::empty()))
}

/// Builds a `200 OK` response with a `text/plain; charset=utf-8` body.
pub fn ok_text(body: impl Into<String>) -> Response {
    ResponseBuilder::new(StatusCode::OK).text(body)
}

/// Builds a `200 OK` response with a `text/html; charset=utf-8` body.
pub fn ok_html(body: impl Into<String>) -> Response {
    ResponseBuilder::new(StatusCode::OK).html(body)
}

/// Builds a `200 OK` response with a raw `application/json` body.
pub fn ok_json_raw(body: impl Into<String>) -> Response {
    ResponseBuilder::new(StatusCode::OK).json_raw(body)
}

/// Builds a plain-text response with an arbitrary [`StatusCode`].
pub fn status_text(status: StatusCode, body: impl Into<String>) -> Response {
    ResponseBuilder::new(status).text(body)
}

/// Builds a redirect response with the given [`StatusCode`] and `Location` header.
pub fn redirect(status: StatusCode, location: &str) -> Response {
    ResponseBuilder::new(status)
        .header("location", location)
        .empty()
}

/// Implementation details used by the generated macro code.
///
/// This module is public because proc-macro expansions run in the caller's
/// crate. It is not part of the stable API; do not use it directly.
#[doc(hidden)]
#[allow(missing_docs)]
pub mod __private {
    use std::future::Future;

    pub use axum::routing::{delete, get, patch, post, put};
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
