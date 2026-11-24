//! Synchronous zero-dependency RFC 6455 WebSocket server acceptor over any `Read + Write`.

use std::io::{BufReader, Read, Write};

use crate::ws::error::{Error, Result};
use crate::ws::handshake::{
    read_client_request, write_error_response, write_server_response, ClientRequestError,
    HandshakeRequest,
};
use crate::ws::socket::{Config, Role, WebSocket};

/// Approval returned by an [`accept_hdr`] callback when accepting a client upgrade.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AcceptResponse {
    /// Optional `Sec-WebSocket-Protocol` selected from the client's offered list.
    pub protocol: Option<String>,
    /// Additional HTTP headers to include in the `101 Switching Protocols` response.
    pub extra_headers: Vec<(String, String)>,
}

impl AcceptResponse {
    /// Creates a default approval with no subprotocol or extra headers.
    pub fn new() -> Self {
        Self::default()
    }

    /// Selects a `Sec-WebSocket-Protocol` to echo back to the client.
    pub fn with_protocol(mut self, protocol: impl Into<String>) -> Self {
        self.protocol = Some(protocol.into());
        self
    }

    /// Appends an extra HTTP header to the `101` handshake response.
    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.extra_headers.push((name.into(), value.into()));
        self
    }
}

/// Rejection returned by an [`accept_hdr`] callback to deny a WebSocket upgrade.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RejectResponse {
    /// HTTP status code (e.g. `401`, `403`, `404`).
    pub status: u16,
    /// HTTP reason phrase (e.g. `"Forbidden"`).
    pub reason: String,
    /// Optional plain-text response body.
    pub body: String,
}

impl RejectResponse {
    /// Creates a rejection response with the given `status`, `reason`, and `body`.
    pub fn new(status: u16, reason: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            status,
            reason: reason.into(),
            body: body.into(),
        }
    }
}

/// Accepts an incoming RFC 6455 WebSocket handshake on `stream` with default [`Config`].
pub fn accept<S: Read + Write>(stream: S) -> Result<WebSocket<S>> {
    accept_with_config(stream, Config::default())
}

/// Accepts an incoming RFC 6455 WebSocket handshake on `stream` with a custom [`Config`].
pub fn accept_with_config<S: Read + Write>(stream: S, config: Config) -> Result<WebSocket<S>> {
    accept_hdr(stream, config, |_| Ok(AcceptResponse::default()))
}

/// Accepts an incoming RFC 6455 WebSocket handshake while inspecting the client's
/// [`HandshakeRequest`] (path, `Origin`, custom headers, `Sec-WebSocket-Protocol`).
///
/// If `on_request` returns `Err(RejectResponse)`, the server writes the HTTP error response
/// and returns [`Error::Handshake`]. Any pipelined WebSocket frames sent immediately behind
/// the upgrade request are preserved inside the returned [`WebSocket`].
pub fn accept_hdr<S, F>(stream: S, config: Config, on_request: F) -> Result<WebSocket<S>>
where
    S: Read + Write,
    F: FnOnce(&HandshakeRequest) -> std::result::Result<AcceptResponse, RejectResponse>,
{
    let mut reader = BufReader::new(stream);
    let request = match read_client_request(&mut reader) {
        Ok(req) => req,
        Err(ClientRequestError::UpgradeRequired(err)) => {
            write_error_response(
                reader.get_mut(),
                426,
                "Upgrade Required",
                &[("Sec-WebSocket-Version", "13")],
                "Sec-WebSocket-Version: 13 required",
            );
            return Err(err);
        }
        Err(ClientRequestError::BadRequest(err)) => {
            write_error_response(
                reader.get_mut(),
                400,
                "Bad Request",
                &[],
                "Invalid WebSocket upgrade request",
            );
            return Err(err);
        }
    };

    let accept_resp = match on_request(&request) {
        Ok(resp) => resp,
        Err(reject) => {
            write_error_response(
                reader.get_mut(),
                reject.status,
                &reject.reason,
                &[],
                &reject.body,
            );
            return Err(Error::Handshake(format!(
                "server rejected WebSocket handshake with HTTP {} {}",
                reject.status, reject.reason
            )));
        }
    };

    write_server_response(
        reader.get_mut(),
        &request.key,
        accept_resp.protocol.as_deref(),
        &accept_resp.extra_headers,
    )?;

    Ok(WebSocket::from_buffered(
        reader,
        Role::Server,
        config,
        accept_resp.protocol,
    ))
}
