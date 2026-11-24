//! Zero-dependency synchronous HTTP/1.1 client.
//!
//! Built directly on [`std::net::TcpStream`] in pure safe Rust, this client
//! works in both synchronous and asynchronous code without extra dependencies.
//! Plain `http://` is supported in Wave 1; `https://` returns a clear
//! [`ClientError::UnsupportedScheme`] error until TLS arrives in Wave 2.

use std::fmt;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::str::Utf8Error;
use std::time::Duration;

use crate::http::types::{is_http_tchar, Headers, Method, StatusCode};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_MAX_REDIRECTS: usize = 5;
const DEFAULT_USER_AGENT: &str = "akku-http/0.1";
const DEFAULT_MAX_RESPONSE_BODY_BYTES: usize = 64 * 1024 * 1024;
const MAX_HEADER_BYTES: usize = 256 * 1024;
const MAX_HEADER_LINES: usize = 1024;
const MAX_INFORMATIONAL_RESPONSES: usize = 16;

/// Errors returned by [`Client`] and [`RequestBuilder::send`].
#[non_exhaustive]
#[derive(Debug)]
pub enum ClientError {
    /// The request URL or a redirect `Location` could not be parsed.
    InvalidUrl(String),
    /// The URL scheme is not supported (for example `https` in Wave 1).
    UnsupportedScheme(String),
    /// An underlying socket or stream I/O error occurred.
    Io(io::Error),
    /// The server returned a malformed HTTP response.
    InvalidResponse(String),
    /// The response exceeded the configured redirect limit.
    TooManyRedirects(usize),
    /// [`ClientResponse::ensure_success`] observed a non-2xx HTTP status.
    HttpStatus {
        /// The returned non-2xx HTTP status code.
        status: StatusCode,
        /// The response body decoded lossily as UTF-8.
        body: String,
    },
    /// Connecting, writing, or reading timed out.
    TimedOut,
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUrl(msg) => write!(f, "invalid URL: {msg}"),
            Self::UnsupportedScheme(scheme) if scheme.eq_ignore_ascii_case("https") => {
                write!(
                    f,
                    "unsupported URL scheme '{scheme}': HTTPS/TLS support arrives in Akku Wave 2"
                )
            }
            Self::UnsupportedScheme(scheme) => {
                write!(
                    f,
                    "unsupported URL scheme '{scheme}': only 'http' is supported"
                )
            }
            Self::Io(err) => write!(f, "HTTP I/O error: {err}"),
            Self::InvalidResponse(msg) => write!(f, "invalid HTTP response: {msg}"),
            Self::TooManyRedirects(limit) => {
                write!(f, "exceeded maximum redirect limit ({limit})")
            }
            Self::HttpStatus { status, body } => {
                if body.is_empty() {
                    write!(f, "HTTP request failed with status {status}")
                } else {
                    write!(f, "HTTP request failed with status {status}: {body}")
                }
            }
            Self::TimedOut => write!(f, "HTTP request timed out"),
        }
    }
}

impl std::error::Error for ClientError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            _ => None,
        }
    }
}

/// A configurable HTTP/1.1 client.
#[derive(Debug, Clone)]
pub struct Client {
    timeout: Option<Duration>,
    max_redirects: usize,
    max_response_body_bytes: usize,
    default_headers: Headers,
}

impl Client {
    /// Creates a new [`Client`] with default settings (30s timeout, up to 5 redirects).
    pub fn new() -> Self {
        Self::builder().build()
    }

    /// Returns a [`ClientBuilder`] for customizing timeouts, redirects, and default headers.
    pub fn builder() -> ClientBuilder {
        ClientBuilder::new()
    }

    /// Starts building a request with an arbitrary [`Method`] and target `url`.
    pub fn request(&self, method: Method, url: impl Into<String>) -> RequestBuilder<'_> {
        RequestBuilder {
            client: self,
            method,
            url: url.into(),
            headers: Headers::new(),
            query: Vec::new(),
            body: Vec::new(),
            timeout: self.timeout,
        }
    }

    /// Starts a `GET` request to `url`.
    pub fn get(&self, url: impl Into<String>) -> RequestBuilder<'_> {
        self.request(Method::Get, url)
    }

    /// Starts a `POST` request to `url`.
    pub fn post(&self, url: impl Into<String>) -> RequestBuilder<'_> {
        self.request(Method::Post, url)
    }

    /// Starts a `PUT` request to `url`.
    pub fn put(&self, url: impl Into<String>) -> RequestBuilder<'_> {
        self.request(Method::Put, url)
    }

    /// Starts a `PATCH` request to `url`.
    pub fn patch(&self, url: impl Into<String>) -> RequestBuilder<'_> {
        self.request(Method::Patch, url)
    }

    /// Starts a `DELETE` request to `url`.
    pub fn delete(&self, url: impl Into<String>) -> RequestBuilder<'_> {
        self.request(Method::Delete, url)
    }

    /// Starts a `HEAD` request to `url`.
    pub fn head(&self, url: impl Into<String>) -> RequestBuilder<'_> {
        self.request(Method::Head, url)
    }
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

/// Builder for configuring a [`Client`].
#[derive(Debug, Clone)]
pub struct ClientBuilder {
    timeout: Option<Duration>,
    max_redirects: usize,
    max_response_body_bytes: usize,
    default_headers: Headers,
}

impl ClientBuilder {
    /// Creates a [`ClientBuilder`] with default settings.
    pub fn new() -> Self {
        let mut default_headers = Headers::new();
        default_headers.insert("user-agent", DEFAULT_USER_AGENT);
        Self {
            timeout: Some(DEFAULT_TIMEOUT),
            max_redirects: DEFAULT_MAX_REDIRECTS,
            max_response_body_bytes: DEFAULT_MAX_RESPONSE_BODY_BYTES,
            default_headers,
        }
    }

    /// Sets the connect, read, and write timeout applied to requests.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Disables the default socket timeout.
    pub fn no_timeout(mut self) -> Self {
        self.timeout = None;
        self
    }

    /// Sets the maximum number of HTTP redirects to follow (default: `5`).
    pub fn max_redirects(mut self, max_redirects: usize) -> Self {
        self.max_redirects = max_redirects;
        self
    }

    /// Sets the maximum allowed response body size in bytes (default: 64 MiB).
    pub fn max_response_body_bytes(mut self, max_bytes: usize) -> Self {
        self.max_response_body_bytes = max_bytes;
        self
    }

    /// Adds a default header sent on every request unless overridden on the request.
    pub fn default_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.default_headers.insert(name, value);
        self
    }

    /// Sets the default `User-Agent` header.
    pub fn user_agent(mut self, user_agent: impl Into<String>) -> Self {
        self.default_headers.insert("user-agent", user_agent);
        self
    }

    /// Builds the configured [`Client`].
    pub fn build(self) -> Client {
        Client {
            timeout: self.timeout,
            max_redirects: self.max_redirects,
            max_response_body_bytes: self.max_response_body_bytes,
            default_headers: self.default_headers,
        }
    }
}

impl Default for ClientBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Builder for a single outgoing HTTP/1.1 request.
#[derive(Debug)]
pub struct RequestBuilder<'a> {
    client: &'a Client,
    method: Method,
    url: String,
    headers: Headers,
    query: Vec<(String, String)>,
    body: Vec<u8>,
    timeout: Option<Duration>,
}

impl<'a> RequestBuilder<'a> {
    /// Appends a request header (preserves multiple values for the same name).
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.append(name, value);
        self
    }

    /// Appends all entries from `headers` to this request.
    pub fn headers(mut self, headers: Headers) -> Self {
        for (name, value) in headers {
            self.headers.append(name, value);
        }
        self
    }

    /// Appends a percent-encoded query parameter `key=value` to the request URL.
    pub fn query(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.query.push((key.into(), value.into()));
        self
    }

    /// Sets the raw request body bytes.
    pub fn body(mut self, body: impl Into<Vec<u8>>) -> Self {
        self.body = body.into();
        self
    }

    /// Sets a UTF-8 plain-text request body and sets `content-type: text/plain; charset=utf-8`
    /// if no `content-type` header is already set.
    pub fn text(mut self, body: impl Into<String>) -> Self {
        if !self.headers.contains("content-type") {
            self.headers
                .insert("content-type", "text/plain; charset=utf-8");
        }
        self.body = body.into().into_bytes();
        self
    }

    /// Sets a raw JSON request body and sets `content-type: application/json`.
    pub fn json_raw(mut self, body: impl Into<String>) -> Self {
        self.headers.insert("content-type", "application/json");
        self.body = body.into().into_bytes();
        self
    }

    /// Overrides the timeout for this request.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Executes the HTTP/1.1 request, following redirects up to the client's limit.
    pub fn send(self) -> std::result::Result<ClientResponse, ClientError> {
        let mut current_url = ParsedUrl::parse(&self.url)?;
        if !self.query.is_empty() {
            current_url.append_query_pairs(&self.query);
        }

        let mut method = self.method;
        let mut body = self.body;
        let mut req_headers = self.headers;

        validate_method(&method)?;

        let mut redirects = 0usize;
        loop {
            let response = execute_once(
                &current_url,
                &method,
                &self.client.default_headers,
                &req_headers,
                &body,
                self.timeout,
                self.client.max_response_body_bytes,
            )?;

            if is_redirect_status(response.status) {
                if let Some(location) = response.header("location") {
                    if redirects >= self.client.max_redirects {
                        return Err(ClientError::TooManyRedirects(self.client.max_redirects));
                    }
                    redirects += 1;

                    let next_url = current_url.resolve_redirect(location)?;
                    let authority_changed =
                        next_url.host != current_url.host || next_url.port != current_url.port;
                    if authority_changed {
                        req_headers.remove("authorization");
                    }
                    req_headers.remove("host");

                    match response.status {
                        StatusCode::SEE_OTHER => {
                            if method != Method::Head {
                                method = Method::Get;
                            }
                            body.clear();
                            req_headers.remove("content-type");
                            req_headers.remove("content-length");
                        }
                        StatusCode::MOVED_PERMANENTLY | StatusCode::FOUND => {
                            if method == Method::Post {
                                method = Method::Get;
                                body.clear();
                                req_headers.remove("content-type");
                                req_headers.remove("content-length");
                            }
                        }
                        StatusCode::TEMPORARY_REDIRECT | StatusCode::PERMANENT_REDIRECT => {}
                        _ => {}
                    }

                    current_url = next_url;
                    continue;
                }
            }

            return Ok(response);
        }
    }
}

/// An HTTP response returned by [`RequestBuilder::send`].
#[derive(Debug, Clone)]
pub struct ClientResponse {
    status: StatusCode,
    headers: Headers,
    body: Vec<u8>,
}

impl ClientResponse {
    /// Returns the HTTP [`StatusCode`].
    pub fn status(&self) -> StatusCode {
        self.status
    }

    /// Returns all response [`Headers`].
    pub fn headers(&self) -> &Headers {
        &self.headers
    }

    /// Returns the first value of header `name` (ASCII case-insensitive).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name)
    }

    /// Returns the raw response body bytes.
    pub fn bytes(&self) -> &[u8] {
        &self.body
    }

    /// Consumes the response and returns the owned body bytes.
    pub fn into_bytes(self) -> Vec<u8> {
        self.body
    }

    /// Returns the response body interpreted as UTF-8 text.
    pub fn text(&self) -> std::result::Result<&str, Utf8Error> {
        std::str::from_utf8(&self.body)
    }

    /// Returns `true` if the status code is in `200..=299`.
    pub fn is_success(&self) -> bool {
        self.status.is_success()
    }

    /// Returns `Ok(self)` if the status code is 2xx, or `Err(ClientError::HttpStatus { .. })`.
    pub fn ensure_success(self) -> std::result::Result<Self, ClientError> {
        if self.is_success() {
            Ok(self)
        } else {
            let body = String::from_utf8_lossy(&self.body).into_owned();
            Err(ClientError::HttpStatus {
                status: self.status,
                body,
            })
        }
    }
}

#[derive(Debug, Clone)]
struct ParsedUrl {
    host: String,
    port: u16,
    path: String,
    query: Option<String>,
}

impl ParsedUrl {
    fn parse(raw: &str) -> Result<Self, ClientError> {
        if raw.bytes().any(|b| b == b'\r' || b == b'\n' || b == 0) {
            return Err(ClientError::InvalidUrl(
                "URL must not contain control characters".to_string(),
            ));
        }
        let without_fragment = raw.split('#').next().unwrap_or(raw);
        if without_fragment.bytes().any(is_invalid_target_byte) {
            return Err(ClientError::InvalidUrl(
                "URL contains invalid whitespace, control, or non-ASCII characters".to_string(),
            ));
        }
        let (scheme, rest) = without_fragment
            .split_once("://")
            .ok_or_else(|| ClientError::InvalidUrl(format!("missing scheme in '{raw}'")))?;

        if scheme.is_empty() {
            return Err(ClientError::InvalidUrl(format!("empty scheme in '{raw}'")));
        }
        if !scheme.eq_ignore_ascii_case("http") {
            return Err(ClientError::UnsupportedScheme(scheme.to_ascii_lowercase()));
        }

        let authority_end = rest.find(['/', '?']).unwrap_or(rest.len());
        let authority = &rest[..authority_end];
        let path_and_query = &rest[authority_end..];

        if authority.is_empty() {
            return Err(ClientError::InvalidUrl(format!("missing host in '{raw}'")));
        }

        let (host, port) = parse_authority(authority, raw)?;

        let (raw_path, query) = match path_and_query.split_once('?') {
            Some((p, q)) => (p, Some(q.to_string())),
            None => (path_and_query, None),
        };
        let path = if raw_path.is_empty() {
            "/".to_string()
        } else {
            raw_path.to_string()
        };

        Ok(Self {
            host,
            port,
            path,
            query,
        })
    }

    fn append_query_pairs(&mut self, pairs: &[(String, String)]) {
        let mut encoded = self.query.take().unwrap_or_default();
        for (k, v) in pairs {
            if !encoded.is_empty() {
                encoded.push('&');
            }
            percent_encode_into(k, &mut encoded);
            encoded.push('=');
            percent_encode_into(v, &mut encoded);
        }
        self.query = Some(encoded);
    }

    fn host_header(&self) -> String {
        if self.port == 80 {
            self.host.clone()
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }

    fn connect_addr(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    fn target(&self) -> String {
        match &self.query {
            Some(q) => format!("{}?{q}", self.path),
            None => self.path.clone(),
        }
    }

    fn resolve_redirect(&self, location: &str) -> Result<Self, ClientError> {
        let loc = location.trim();
        if loc.is_empty() || loc.bytes().any(|b| b == b'\r' || b == b'\n' || b == 0) {
            return Err(ClientError::InvalidUrl(format!(
                "invalid redirect Location: {location:?}"
            )));
        }

        if let Some((scheme, _)) = loc.split_once("://") {
            if is_valid_scheme(scheme) {
                return Self::parse(loc);
            }
        }

        if let Some(after_slashes) = loc.strip_prefix("//") {
            return Self::parse(&format!("http://{after_slashes}"));
        }

        let without_fragment = loc.split('#').next().unwrap_or(loc);
        if without_fragment.bytes().any(is_invalid_target_byte) {
            return Err(ClientError::InvalidUrl(format!(
                "invalid redirect Location: {location:?}"
            )));
        }

        if let Some(q) = without_fragment.strip_prefix('?') {
            return Ok(Self {
                host: self.host.clone(),
                port: self.port,
                path: self.path.clone(),
                query: Some(q.to_string()),
            });
        }

        let (raw_path, query) = match without_fragment.split_once('?') {
            Some((p, q)) => (p, Some(q.to_string())),
            None => (without_fragment, None),
        };

        let resolved_path = if raw_path.starts_with('/') {
            normalize_path(raw_path)
        } else {
            let base_dir = match self.path.rfind('/') {
                Some(idx) => &self.path[..=idx],
                None => "/",
            };
            normalize_path(&format!("{base_dir}{raw_path}"))
        };

        Ok(Self {
            host: self.host.clone(),
            port: self.port,
            path: resolved_path,
            query,
        })
    }
}

fn parse_authority(authority: &str, raw: &str) -> Result<(String, u16), ClientError> {
    if authority.contains('@') {
        return Err(ClientError::InvalidUrl(format!(
            "userinfo is not supported in '{raw}'"
        )));
    }
    if authority.starts_with('[') {
        let close = authority
            .find(']')
            .ok_or_else(|| ClientError::InvalidUrl(format!("unclosed IPv6 host in '{raw}'")))?;
        let host = authority[..=close].to_string();
        let rest = &authority[close + 1..];
        let port = if rest.is_empty() {
            80
        } else if let Some(p) = rest.strip_prefix(':') {
            p.parse::<u16>()
                .map_err(|_| ClientError::InvalidUrl(format!("invalid port in '{raw}'")))?
        } else {
            return Err(ClientError::InvalidUrl(format!(
                "invalid authority in '{raw}'"
            )));
        };
        Ok((host, port))
    } else if let Some((h, p)) = authority.rsplit_once(':') {
        if h.is_empty() {
            return Err(ClientError::InvalidUrl(format!("missing host in '{raw}'")));
        }
        let port = p
            .parse::<u16>()
            .map_err(|_| ClientError::InvalidUrl(format!("invalid port in '{raw}'")))?;
        Ok((h.to_string(), port))
    } else {
        Ok((authority.to_string(), 80))
    }
}

fn normalize_path(path: &str) -> String {
    let mut stack: Vec<&str> = Vec::new();
    let trailing_slash = path.ends_with('/');
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                stack.pop();
            }
            other => stack.push(other),
        }
    }
    if stack.is_empty() {
        "/".to_string()
    } else {
        let mut out = String::from("/");
        out.push_str(&stack.join("/"));
        if trailing_slash {
            out.push('/');
        }
        out
    }
}

fn percent_encode_into(input: &str, out: &mut String) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for b in input.bytes() {
        let unreserved = b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~');
        if unreserved {
            out.push(b as char);
        } else {
            out.push('%');
            out.push(HEX[(b >> 4) as usize] as char);
            out.push(HEX[(b & 0x0f) as usize] as char);
        }
    }
}

fn is_redirect_status(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::MOVED_PERMANENTLY
            | StatusCode::FOUND
            | StatusCode::SEE_OTHER
            | StatusCode::TEMPORARY_REDIRECT
            | StatusCode::PERMANENT_REDIRECT
    )
}

fn is_invalid_target_byte(b: u8) -> bool {
    !(0x21..0x7F).contains(&b)
}

fn is_valid_scheme(s: &str) -> bool {
    let mut bytes = s.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    first.is_ascii_alphabetic()
        && bytes.all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'))
}

fn validate_request_target(target: &str) -> Result<(), ClientError> {
    if target.is_empty() || target.bytes().any(is_invalid_target_byte) {
        return Err(ClientError::InvalidUrl(format!(
            "invalid HTTP request-target: {target:?}"
        )));
    }
    Ok(())
}

fn validate_method(method: &Method) -> Result<(), ClientError> {
    let s = method.as_str();
    if s.is_empty() || !s.bytes().all(is_http_tchar) {
        return Err(ClientError::InvalidUrl(format!(
            "invalid HTTP method: {s:?}"
        )));
    }
    Ok(())
}

fn validate_header(name: &str, value: &str) -> Result<(), ClientError> {
    if name.is_empty() || !name.bytes().all(is_http_tchar) {
        return Err(ClientError::InvalidUrl(format!(
            "invalid header name: {name:?}"
        )));
    }
    if value.bytes().any(|b| b == b'\r' || b == b'\n' || b == 0) {
        return Err(ClientError::InvalidUrl(format!(
            "invalid header value for '{name}'"
        )));
    }
    Ok(())
}

fn map_io_err(err: io::Error, has_timeout: bool) -> ClientError {
    if has_timeout
        && matches!(
            err.kind(),
            io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
        )
    {
        ClientError::TimedOut
    } else {
        ClientError::Io(err)
    }
}

fn connect_stream(addr: &str, timeout: Option<Duration>) -> Result<TcpStream, ClientError> {
    let has_timeout = timeout.is_some();
    let addrs: Vec<_> = addr
        .to_socket_addrs()
        .map_err(|e| map_io_err(e, has_timeout))?
        .collect();
    if addrs.is_empty() {
        return Err(ClientError::InvalidUrl(format!(
            "failed to resolve address '{addr}'"
        )));
    }

    let mut last_err = None;
    for sock_addr in addrs {
        let res = match timeout {
            Some(dur) => TcpStream::connect_timeout(&sock_addr, dur),
            None => TcpStream::connect(sock_addr),
        };
        match res {
            Ok(stream) => {
                stream
                    .set_read_timeout(timeout)
                    .map_err(|e| map_io_err(e, has_timeout))?;
                stream
                    .set_write_timeout(timeout)
                    .map_err(|e| map_io_err(e, has_timeout))?;
                return Ok(stream);
            }
            Err(err) => last_err = Some(err),
        }
    }

    Err(map_io_err(
        last_err.unwrap_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "connect failed")),
        has_timeout,
    ))
}

fn execute_once(
    url: &ParsedUrl,
    method: &Method,
    default_headers: &Headers,
    req_headers: &Headers,
    body: &[u8],
    timeout: Option<Duration>,
    max_body_bytes: usize,
) -> Result<ClientResponse, ClientError> {
    let has_timeout = timeout.is_some();

    let mut wire = Vec::with_capacity(256 + body.len());
    let target = url.target();
    validate_request_target(&target)?;
    let _ = write!(wire, "{} {} HTTP/1.1\r\n", method.as_str(), target);

    if !req_headers.contains("host") {
        let _ = write!(wire, "Host: {}\r\n", url.host_header());
    }
    if !req_headers.contains("connection") {
        wire.extend_from_slice(b"Connection: close\r\n");
    }

    let needs_content_length =
        !body.is_empty() || matches!(method, Method::Post | Method::Put | Method::Patch);
    if needs_content_length && !req_headers.contains("content-length") {
        let _ = write!(wire, "Content-Length: {}\r\n", body.len());
    }

    for (name, value) in default_headers.iter() {
        if !req_headers.contains(name)
            && !name.eq_ignore_ascii_case("host")
            && !name.eq_ignore_ascii_case("connection")
            && !name.eq_ignore_ascii_case("content-length")
        {
            validate_header(name, value)?;
            let _ = write!(wire, "{name}: {value}\r\n");
        }
    }
    for (name, value) in req_headers.iter() {
        validate_header(name, value)?;
        let _ = write!(wire, "{name}: {value}\r\n");
    }
    wire.extend_from_slice(b"\r\n");
    wire.extend_from_slice(body);

    let mut stream = connect_stream(&url.connect_addr(), timeout)?;
    stream
        .write_all(&wire)
        .map_err(|e| map_io_err(e, has_timeout))?;
    stream.flush().map_err(|e| map_io_err(e, has_timeout))?;

    let mut reader = BufReader::new(stream);
    let mut info_count = 0usize;
    let (status, headers) = loop {
        let (status, headers) = read_response_head(&mut reader, has_timeout)?;
        if status.is_informational() {
            info_count += 1;
            if info_count > MAX_INFORMATIONAL_RESPONSES {
                return Err(ClientError::InvalidResponse(format!(
                    "exceeded maximum number of informational 1xx responses ({MAX_INFORMATIONAL_RESPONSES})"
                )));
            }
            continue;
        }
        break (status, headers);
    };

    let response_body = if *method == Method::Head
        || status == StatusCode::NO_CONTENT
        || status.as_u16() == 304
    {
        Vec::new()
    } else if is_chunked(&headers) {
        read_chunked_body(&mut reader, max_body_bytes, has_timeout)?
    } else if let Some(len) = parse_content_length(&headers)? {
        if len > max_body_bytes {
            return Err(ClientError::InvalidResponse(format!(
                    "response body Content-Length {len} exceeded maximum size of {max_body_bytes} bytes"
                )));
        }
        let mut buf = vec![0u8; len];
        reader
            .read_exact(&mut buf)
            .map_err(|e| map_io_err(e, has_timeout))?;
        buf
    } else {
        let cap = u64::try_from(max_body_bytes)
            .unwrap_or(u64::MAX)
            .saturating_add(1);
        let mut buf = Vec::new();
        reader
            .by_ref()
            .take(cap)
            .read_to_end(&mut buf)
            .map_err(|e| map_io_err(e, has_timeout))?;
        if buf.len() > max_body_bytes {
            return Err(ClientError::InvalidResponse(format!(
                "response body exceeded maximum size of {max_body_bytes} bytes"
            )));
        }
        buf
    };

    Ok(ClientResponse {
        status,
        headers,
        body: response_body,
    })
}

fn read_crlf_line<R: BufRead>(
    reader: &mut R,
    buf: &mut Vec<u8>,
    max_len: usize,
    has_timeout: bool,
) -> Result<usize, ClientError> {
    buf.clear();
    let cap = u64::try_from(max_len).unwrap_or(u64::MAX).saturating_add(1);
    let n = reader
        .by_ref()
        .take(cap)
        .read_until(b'\n', buf)
        .map_err(|e| map_io_err(e, has_timeout))?;
    if n == 0 {
        return Err(ClientError::InvalidResponse(
            "unexpected EOF while reading HTTP headers".to_string(),
        ));
    }
    if buf.len() > max_len {
        return Err(ClientError::InvalidResponse(
            "HTTP header line exceeded size limit".to_string(),
        ));
    }
    if buf.ends_with(b"\n") {
        buf.pop();
        if buf.ends_with(b"\r") {
            buf.pop();
        }
    }
    Ok(n)
}

fn read_response_head<R: BufRead>(
    reader: &mut R,
    has_timeout: bool,
) -> Result<(StatusCode, Headers), ClientError> {
    let mut line = Vec::new();
    let mut total_bytes = read_crlf_line(reader, &mut line, MAX_HEADER_BYTES, has_timeout)?;

    let status_line = std::str::from_utf8(&line)
        .map_err(|_| ClientError::InvalidResponse("status line is not valid UTF-8".to_string()))?;
    let mut parts = status_line.splitn(3, ' ');
    let version = parts
        .next()
        .ok_or_else(|| ClientError::InvalidResponse("missing HTTP version".to_string()))?;
    if !version.starts_with("HTTP/1.") {
        return Err(ClientError::InvalidResponse(format!(
            "unsupported HTTP version '{version}'"
        )));
    }
    let code_str = parts
        .next()
        .ok_or_else(|| ClientError::InvalidResponse("missing HTTP status code".to_string()))?;
    let code_num = code_str.parse::<u16>().map_err(|_| {
        ClientError::InvalidResponse(format!("invalid HTTP status code '{code_str}'"))
    })?;
    let status = StatusCode::from_u16(code_num).ok_or_else(|| {
        ClientError::InvalidResponse(format!("out-of-range HTTP status code {code_num}"))
    })?;

    let mut headers = Headers::new();
    let mut header_count = 0usize;
    loop {
        let remaining = MAX_HEADER_BYTES.saturating_sub(total_bytes);
        if remaining == 0 {
            return Err(ClientError::InvalidResponse(
                "HTTP response headers exceeded size limit".to_string(),
            ));
        }
        total_bytes += read_crlf_line(reader, &mut line, remaining, has_timeout)?;
        if line.is_empty() {
            break;
        }
        header_count += 1;
        if header_count > MAX_HEADER_LINES {
            return Err(ClientError::InvalidResponse(
                "too many HTTP response headers".to_string(),
            ));
        }
        let line_str = std::str::from_utf8(&line).map_err(|_| {
            ClientError::InvalidResponse("header line is not valid UTF-8".to_string())
        })?;
        let (name, value) = line_str.split_once(':').ok_or_else(|| {
            ClientError::InvalidResponse(format!("malformed header line '{line_str}'"))
        })?;
        headers.append(name.trim(), value.trim());
    }

    Ok((status, headers))
}

fn is_chunked(headers: &Headers) -> bool {
    headers.get_all("transfer-encoding").into_iter().any(|val| {
        val.split(',')
            .any(|token| token.trim().eq_ignore_ascii_case("chunked"))
    })
}

fn parse_content_length(headers: &Headers) -> Result<Option<usize>, ClientError> {
    let values = headers.get_all("content-length");
    if values.is_empty() {
        return Ok(None);
    }
    let mut parsed: Option<usize> = None;
    for raw in values {
        for part in raw.split(',') {
            let trimmed = part.trim();
            let n = trimmed.parse::<usize>().map_err(|_| {
                ClientError::InvalidResponse(format!("invalid Content-Length '{trimmed}'"))
            })?;
            match parsed {
                Some(prev) if prev != n => {
                    return Err(ClientError::InvalidResponse(
                        "conflicting Content-Length headers".to_string(),
                    ));
                }
                _ => parsed = Some(n),
            }
        }
    }
    Ok(parsed)
}

fn read_chunked_body<R: BufRead>(
    reader: &mut R,
    max_body_bytes: usize,
    has_timeout: bool,
) -> Result<Vec<u8>, ClientError> {
    let mut out = Vec::new();
    let mut line = Vec::new();
    loop {
        read_crlf_line(reader, &mut line, 16 * 1024, has_timeout)?;
        let size_line = std::str::from_utf8(&line).map_err(|_| {
            ClientError::InvalidResponse("chunk size line is not valid UTF-8".to_string())
        })?;
        let size_hex = size_line.split(';').next().unwrap_or("").trim();
        let chunk_len = usize::from_str_radix(size_hex, 16).map_err(|_| {
            ClientError::InvalidResponse(format!("invalid chunk size '{size_hex}'"))
        })?;

        if chunk_len == 0 {
            // Consume optional trailer headers until terminating empty line.
            loop {
                read_crlf_line(reader, &mut line, 16 * 1024, has_timeout)?;
                if line.is_empty() {
                    break;
                }
            }
            break;
        }

        let start = out.len();
        let Some(new_len) = start.checked_add(chunk_len) else {
            return Err(ClientError::InvalidResponse(
                "chunked response body size overflowed".to_string(),
            ));
        };
        if new_len > max_body_bytes {
            return Err(ClientError::InvalidResponse(format!(
                "response body exceeded maximum size of {max_body_bytes} bytes"
            )));
        }
        out.resize(new_len, 0);
        reader
            .read_exact(&mut out[start..])
            .map_err(|e| map_io_err(e, has_timeout))?;

        // Consume trailing CRLF after chunk payload.
        let mut crlf = [0u8; 2];
        reader
            .read_exact(&mut crlf)
            .map_err(|e| map_io_err(e, has_timeout))?;
        if &crlf != b"\r\n" {
            return Err(ClientError::InvalidResponse(
                "missing CRLF after chunk payload".to_string(),
            ));
        }
    }
    Ok(out)
}
