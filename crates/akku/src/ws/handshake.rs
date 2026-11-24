//! RFC 6455 §4 HTTP/1.1 opening handshake helpers and validation.

use std::io::{BufRead, BufReader, Read, Write};

use crate::base64;
use crate::ws::error::{Error, Result};
use crate::ws::sha1;

/// RFC 6455 §1.3 magic GUID concatenated with `Sec-WebSocket-Key`.
pub const WEBSOCKET_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

const MAX_HEADER_BYTES: usize = 64 * 1024;
const MAX_HEADER_LINES: usize = 128;
const MAX_LINE_BYTES: usize = 8 * 1024;

/// Computes the RFC 6455 §1.3 `Sec-WebSocket-Accept` header value:
/// `base64(SHA-1(sec_websocket_key || WEBSOCKET_GUID))`.
pub fn compute_accept_key(sec_websocket_key: &str) -> String {
    let mut input = Vec::with_capacity(sec_websocket_key.len() + WEBSOCKET_GUID.len());
    input.extend_from_slice(sec_websocket_key.as_bytes());
    input.extend_from_slice(WEBSOCKET_GUID.as_bytes());
    let hash = sha1::digest(&input);
    base64::encode(hash)
}

/// Generates a random 16-byte `Sec-WebSocket-Key` encoded as standard padded Base64 (24 chars).
pub fn generate_key() -> String {
    let mut nonce = [0u8; 16];
    sha1::fill_random(&mut nonce);
    base64::encode(nonce)
}

/// Returns `true` if `key` is a canonical 24-character padded Base64 encoding of 16 bytes.
pub fn is_valid_key(key: &str) -> bool {
    key.len() == 24
        && base64::decode(key)
            .map(|decoded| decoded.len() == 16)
            .unwrap_or(false)
}

/// Parsed HTTP/1.1 client WebSocket upgrade request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandshakeRequest {
    /// Request target path (and optional query string).
    pub path: String,
    /// Value of the `Host` header.
    pub host: String,
    /// Validated `Sec-WebSocket-Key` value.
    pub key: String,
    /// Optional `Origin` header value.
    pub origin: Option<String>,
    /// Subprotocols requested in `Sec-WebSocket-Protocol` headers.
    pub protocols: Vec<String>,
    /// All request headers with lowercased ASCII names.
    pub headers: Vec<(String, String)>,
}

impl HandshakeRequest {
    /// Returns the first header matching `name` (ASCII case-insensitive).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// Parsed HTTP/1.1 server WebSocket upgrade response (`101 Switching Protocols`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandshakeResponse {
    /// Verified `Sec-WebSocket-Accept` value.
    pub accept_key: String,
    /// Optional negotiated `Sec-WebSocket-Protocol`.
    pub protocol: Option<String>,
    /// All response headers with lowercased ASCII names.
    pub headers: Vec<(String, String)>,
}

impl HandshakeResponse {
    /// Returns the first header matching `name` (ASCII case-insensitive).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// Returns `true` if `b` is a valid HTTP token character (`tchar`, RFC 9110 §5.6.2).
pub(crate) fn is_http_tchar(b: u8) -> bool {
    b.is_ascii_alphanumeric()
        || matches!(
            b,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
}

/// Validates an outgoing HTTP header name and value against CRLF/NUL injection.
pub(crate) fn validate_header(name: &str, value: &str) -> Result<()> {
    if name.is_empty() || !name.bytes().all(is_http_tchar) {
        return Err(Error::Handshake(format!(
            "invalid HTTP header name: {name:?}"
        )));
    }
    if value.bytes().any(|b| b == b'\r' || b == b'\n' || b == 0) {
        return Err(Error::Handshake(format!(
            "invalid HTTP header value for '{name}': contains CR, LF, or NUL"
        )));
    }
    Ok(())
}

/// Validates an HTTP request-target contains only visible ASCII (`0x21..=0x7E`).
pub(crate) fn validate_request_target(target: &str) -> Result<()> {
    if target.is_empty() || !target.bytes().all(|b| (0x21..0x7F).contains(&b)) {
        return Err(Error::InvalidUrl(format!(
            "invalid WebSocket request target: {target:?}"
        )));
    }
    Ok(())
}

/// Checks whether a comma-separated HTTP header value contains `token` (ASCII case-insensitive).
fn header_contains_token(value: &str, token: &str) -> bool {
    value
        .split(',')
        .map(str::trim)
        .any(|part| part.eq_ignore_ascii_case(token))
}

/// Reads a single CRLF-terminated HTTP line from `reader` without consuming past `\n`,
/// enforcing both per-line and cumulative header byte caps.
fn read_http_line<R: BufRead>(reader: &mut R, total_bytes: &mut usize) -> Result<String> {
    let mut line = Vec::new();
    loop {
        let buf = reader.fill_buf()?;
        if buf.is_empty() {
            return Err(Error::Handshake(
                "unexpected EOF while reading HTTP handshake headers".to_string(),
            ));
        }

        let (take, found_lf) = match buf.iter().position(|&b| b == b'\n') {
            Some(pos) => (pos + 1, true),
            None => (buf.len(), false),
        };

        if line.len().saturating_add(take) > MAX_LINE_BYTES {
            return Err(Error::Handshake(format!(
                "HTTP handshake header line exceeds {MAX_LINE_BYTES} bytes"
            )));
        }
        *total_bytes = total_bytes.saturating_add(take);
        if *total_bytes > MAX_HEADER_BYTES {
            return Err(Error::Handshake(format!(
                "HTTP handshake headers exceed {MAX_HEADER_BYTES} bytes"
            )));
        }

        line.extend_from_slice(&buf[..take]);
        reader.consume(take);

        if found_lf {
            break;
        }
    }

    if line.ends_with(b"\r\n") {
        line.truncate(line.len() - 2);
    } else if line.ends_with(b"\n") {
        line.truncate(line.len() - 1);
    }

    if line.contains(&b'\r') || line.contains(&0) {
        return Err(Error::Handshake(
            "HTTP handshake line contains bare CR or NUL byte".to_string(),
        ));
    }

    String::from_utf8(line)
        .map_err(|_| Error::Handshake("HTTP handshake headers must be valid UTF-8".to_string()))
}

/// Reads all header lines following the start line up to the terminating blank line.
fn read_headers<R: BufRead>(
    reader: &mut R,
    total_bytes: &mut usize,
) -> Result<Vec<(String, String)>> {
    let mut headers = Vec::new();
    loop {
        let line = read_http_line(reader, total_bytes)?;
        if line.is_empty() {
            break;
        }
        if headers.len() >= MAX_HEADER_LINES {
            return Err(Error::Handshake(format!(
                "HTTP handshake exceeds maximum of {MAX_HEADER_LINES} headers"
            )));
        }
        let Some((raw_name, raw_val)) = line.split_once(':') else {
            return Err(Error::Handshake(format!(
                "malformed HTTP header line: {line:?}"
            )));
        };
        if raw_name.is_empty() || !raw_name.bytes().all(is_http_tchar) {
            return Err(Error::Handshake(format!(
                "invalid HTTP header name: {raw_name:?}"
            )));
        }
        let name = raw_name.to_ascii_lowercase();
        let value = raw_val.trim().to_string();
        headers.push((name, value));
    }
    Ok(headers)
}

/// Writes an HTTP/1.1 WebSocket client opening handshake request.
pub(crate) fn write_client_request<W: Write>(
    writer: &mut W,
    target: &str,
    host: &str,
    key: &str,
    protocols: &[String],
    extra_headers: &[(String, String)],
) -> Result<()> {
    validate_request_target(target)?;
    validate_header("Host", host)?;
    validate_header("Sec-WebSocket-Key", key)?;

    let mut wire = Vec::with_capacity(256);
    let _ = write!(wire, "GET {target} HTTP/1.1\r\n");
    let _ = write!(wire, "Host: {host}\r\n");
    wire.extend_from_slice(b"Upgrade: websocket\r\n");
    wire.extend_from_slice(b"Connection: Upgrade\r\n");
    let _ = write!(wire, "Sec-WebSocket-Key: {key}\r\n");
    wire.extend_from_slice(b"Sec-WebSocket-Version: 13\r\n");

    if !protocols.is_empty() {
        let joined = protocols.join(", ");
        validate_header("Sec-WebSocket-Protocol", &joined)?;
        let _ = write!(wire, "Sec-WebSocket-Protocol: {joined}\r\n");
    }

    for (name, value) in extra_headers {
        validate_header(name, value)?;
        if name.eq_ignore_ascii_case("host")
            || name.eq_ignore_ascii_case("upgrade")
            || name.eq_ignore_ascii_case("connection")
            || name.eq_ignore_ascii_case("sec-websocket-key")
            || name.eq_ignore_ascii_case("sec-websocket-version")
            || name.eq_ignore_ascii_case("sec-websocket-protocol")
        {
            return Err(Error::Handshake(format!(
                "cannot override reserved WebSocket handshake header '{name}'"
            )));
        }
        let _ = write!(wire, "{name}: {value}\r\n");
    }

    wire.extend_from_slice(b"\r\n");
    writer.write_all(&wire)?;
    writer.flush()?;
    Ok(())
}

/// Reads and validates a server's `101 Switching Protocols` response against `expected_key`
/// and the client's offered `protocols`.
pub(crate) fn read_server_response<R: Read>(
    reader: &mut BufReader<R>,
    expected_key: &str,
    offered_protocols: &[String],
) -> Result<HandshakeResponse> {
    let mut total_bytes = 0usize;
    let status_line = read_http_line(reader, &mut total_bytes)?;
    let mut parts = status_line.splitn(3, ' ');
    let version = parts
        .next()
        .ok_or_else(|| Error::Handshake("empty HTTP status line".to_string()))?;
    let status_str = parts
        .next()
        .ok_or_else(|| Error::Handshake(format!("malformed HTTP status line: {status_line:?}")))?;
    let reason = parts.next().unwrap_or("").to_string();

    if version != "HTTP/1.1" {
        return Err(Error::Handshake(format!(
            "unsupported HTTP version in handshake response: {version:?}"
        )));
    }
    let status: u16 = status_str
        .parse()
        .map_err(|_| Error::Handshake(format!("invalid HTTP status code: {status_str:?}")))?;

    let headers = read_headers(reader, &mut total_bytes)?;

    if status != 101 {
        return Err(Error::HandshakeStatus { status, reason });
    }

    let has_upgrade = headers
        .iter()
        .any(|(k, v)| k == "upgrade" && header_contains_token(v, "websocket"));
    if !has_upgrade {
        return Err(Error::Handshake(
            "missing or invalid 'Upgrade: websocket' header in server response".to_string(),
        ));
    }

    let has_connection = headers
        .iter()
        .any(|(k, v)| k == "connection" && header_contains_token(v, "upgrade"));
    if !has_connection {
        return Err(Error::Handshake(
            "missing or invalid 'Connection: Upgrade' header in server response".to_string(),
        ));
    }

    let mut accept_values = headers
        .iter()
        .filter(|(k, _)| k == "sec-websocket-accept")
        .map(|(_, v)| v.as_str());
    let accept_key = accept_values.next().ok_or_else(|| {
        Error::Handshake("missing 'Sec-WebSocket-Accept' header in server response".to_string())
    })?;
    if accept_values.next().is_some() {
        return Err(Error::Handshake(
            "duplicate 'Sec-WebSocket-Accept' header in server response".to_string(),
        ));
    }

    let expected_accept = compute_accept_key(expected_key);
    if accept_key != expected_accept {
        return Err(Error::Handshake(format!(
            "Sec-WebSocket-Accept mismatch: expected '{expected_accept}', got '{accept_key}'"
        )));
    }

    let mut protocol_values = headers
        .iter()
        .filter(|(k, _)| k == "sec-websocket-protocol")
        .map(|(_, v)| v.as_str());
    let protocol = match protocol_values.next() {
        Some(proto) => {
            if protocol_values.next().is_some() {
                return Err(Error::Handshake(
                    "duplicate 'Sec-WebSocket-Protocol' header in server response".to_string(),
                ));
            }
            if !offered_protocols.iter().any(|p| p == proto) {
                return Err(Error::Handshake(format!(
                    "server selected unoffered subprotocol '{proto}'"
                )));
            }
            Some(proto.to_string())
        }
        None => None,
    };

    Ok(HandshakeResponse {
        accept_key: accept_key.to_string(),
        protocol,
        headers,
    })
}

/// Error kind returned internally when validating a client upgrade request so the server
/// can write the RFC 6455 HTTP error response before closing.
pub(crate) enum ClientRequestError {
    BadRequest(Error),
    UpgradeRequired(Error),
}

/// Reads and validates a client's HTTP/1.1 WebSocket upgrade request.
pub(crate) fn read_client_request<R: Read>(
    reader: &mut BufReader<R>,
) -> std::result::Result<HandshakeRequest, ClientRequestError> {
    let mut total_bytes = 0usize;
    let request_line =
        read_http_line(reader, &mut total_bytes).map_err(ClientRequestError::BadRequest)?;
    let mut parts = request_line.split(' ');
    let method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("");
    let version = parts.next().unwrap_or("");
    if parts.next().is_some() || method.is_empty() || target.is_empty() || version.is_empty() {
        return Err(ClientRequestError::BadRequest(Error::Handshake(format!(
            "malformed HTTP request line: {request_line:?}"
        ))));
    }
    if method != "GET" {
        return Err(ClientRequestError::BadRequest(Error::Handshake(format!(
            "WebSocket upgrade requires GET method, got '{method}'"
        ))));
    }
    if version != "HTTP/1.1" {
        return Err(ClientRequestError::BadRequest(Error::Handshake(format!(
            "WebSocket upgrade requires HTTP/1.1, got '{version}'"
        ))));
    }
    validate_request_target(target).map_err(ClientRequestError::BadRequest)?;

    let headers = read_headers(reader, &mut total_bytes).map_err(ClientRequestError::BadRequest)?;

    // Reject request bodies / chunked encoding to prevent HTTP request smuggling.
    for (k, v) in &headers {
        if k == "transfer-encoding" {
            return Err(ClientRequestError::BadRequest(Error::Handshake(
                "WebSocket upgrade request must not contain Transfer-Encoding".to_string(),
            )));
        }
        if k == "content-length" && v != "0" {
            return Err(ClientRequestError::BadRequest(Error::Handshake(
                "WebSocket upgrade request must not contain non-zero Content-Length".to_string(),
            )));
        }
    }

    let mut host_iter = headers
        .iter()
        .filter(|(k, _)| k == "host")
        .map(|(_, v)| v.as_str());
    let host = match host_iter.next() {
        Some(h) if !h.is_empty() => {
            if host_iter.next().is_some() {
                return Err(ClientRequestError::BadRequest(Error::Handshake(
                    "duplicate 'Host' header in WebSocket request".to_string(),
                )));
            }
            h.to_string()
        }
        _ => {
            return Err(ClientRequestError::BadRequest(Error::Handshake(
                "missing or empty 'Host' header in WebSocket request".to_string(),
            )));
        }
    };

    let has_upgrade = headers
        .iter()
        .any(|(k, v)| k == "upgrade" && header_contains_token(v, "websocket"));
    if !has_upgrade {
        return Err(ClientRequestError::BadRequest(Error::Handshake(
            "missing or invalid 'Upgrade: websocket' header".to_string(),
        )));
    }

    let has_connection = headers
        .iter()
        .any(|(k, v)| k == "connection" && header_contains_token(v, "upgrade"));
    if !has_connection {
        return Err(ClientRequestError::BadRequest(Error::Handshake(
            "missing or invalid 'Connection: Upgrade' header".to_string(),
        )));
    }

    let mut version_iter = headers
        .iter()
        .filter(|(k, _)| k == "sec-websocket-version")
        .map(|(_, v)| v.as_str());
    match version_iter.next() {
        Some("13") => {
            if version_iter.next().is_some() {
                return Err(ClientRequestError::BadRequest(Error::Handshake(
                    "duplicate 'Sec-WebSocket-Version' header".to_string(),
                )));
            }
        }
        Some(other) => {
            return Err(ClientRequestError::UpgradeRequired(Error::Handshake(
                format!("unsupported Sec-WebSocket-Version '{other}' (expected 13)"),
            )));
        }
        None => {
            return Err(ClientRequestError::UpgradeRequired(Error::Handshake(
                "missing 'Sec-WebSocket-Version' header".to_string(),
            )));
        }
    }

    let mut key_iter = headers
        .iter()
        .filter(|(k, _)| k == "sec-websocket-key")
        .map(|(_, v)| v.as_str());
    let key = match key_iter.next() {
        Some(k) if is_valid_key(k) => {
            if key_iter.next().is_some() {
                return Err(ClientRequestError::BadRequest(Error::Handshake(
                    "duplicate 'Sec-WebSocket-Key' header".to_string(),
                )));
            }
            k.to_string()
        }
        Some(_) => {
            return Err(ClientRequestError::BadRequest(Error::Handshake(
                "invalid 'Sec-WebSocket-Key' header (must be 16-byte Base64)".to_string(),
            )));
        }
        None => {
            return Err(ClientRequestError::BadRequest(Error::Handshake(
                "missing 'Sec-WebSocket-Key' header".to_string(),
            )));
        }
    };

    let origin = headers
        .iter()
        .find(|(k, _)| k == "origin")
        .map(|(_, v)| v.clone());

    let mut protocols = Vec::new();
    for (k, v) in &headers {
        if k == "sec-websocket-protocol" {
            for proto in v.split(',').map(str::trim).filter(|p| !p.is_empty()) {
                protocols.push(proto.to_string());
            }
        }
    }

    Ok(HandshakeRequest {
        path: target.to_string(),
        host,
        key,
        origin,
        protocols,
        headers,
    })
}

/// Writes the `101 Switching Protocols` server handshake response.
pub(crate) fn write_server_response<W: Write>(
    writer: &mut W,
    client_key: &str,
    protocol: Option<&str>,
    extra_headers: &[(String, String)],
) -> Result<()> {
    let accept = compute_accept_key(client_key);
    let mut wire = Vec::with_capacity(256);
    wire.extend_from_slice(b"HTTP/1.1 101 Switching Protocols\r\n");
    wire.extend_from_slice(b"Upgrade: websocket\r\n");
    wire.extend_from_slice(b"Connection: Upgrade\r\n");
    let _ = write!(wire, "Sec-WebSocket-Accept: {accept}\r\n");

    if let Some(proto) = protocol {
        validate_header("Sec-WebSocket-Protocol", proto)?;
        let _ = write!(wire, "Sec-WebSocket-Protocol: {proto}\r\n");
    }
    for (name, value) in extra_headers {
        validate_header(name, value)?;
        let _ = write!(wire, "{name}: {value}\r\n");
    }
    wire.extend_from_slice(b"\r\n");
    writer.write_all(&wire)?;
    writer.flush()?;
    Ok(())
}

/// Writes an HTTP/1.1 error response when rejecting a WebSocket upgrade.
pub(crate) fn write_error_response<W: Write>(
    writer: &mut W,
    status: u16,
    reason: &str,
    extra_headers: &[(&str, &str)],
    body: &str,
) {
    let clean_reason: String = reason
        .chars()
        .filter(|&c| c != '\r' && c != '\n' && c != '\0')
        .collect();
    let mut wire = Vec::with_capacity(128 + body.len());
    let _ = write!(wire, "HTTP/1.1 {status} {clean_reason}\r\n");
    for (k, v) in extra_headers {
        let _ = write!(wire, "{k}: {v}\r\n");
    }
    let _ = write!(wire, "Content-Length: {}\r\n", body.len());
    wire.extend_from_slice(b"Connection: close\r\n\r\n");
    wire.extend_from_slice(body.as_bytes());
    let _ = writer.write_all(&wire);
    let _ = writer.flush();
}
