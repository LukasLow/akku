//! Synchronous zero-dependency HTTP/1.1 Server-Sent Events client over [`std::net::TcpStream`].

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use crate::sse::error::{map_io_err, Error, Result};
use crate::sse::event::Event;
use crate::sse::parser::{EventStream, DEFAULT_MAX_EVENT_BYTES};
use crate::url::Url;

const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_HEADER_BYTES: usize = 64 * 1024;
const MAX_HEADER_LINES: usize = 128;
const MAX_LINE_BYTES: usize = 8 * 1024;
const MAX_ERROR_BODY_BYTES: usize = 4 * 1024;

/// Connects to an `http://` Server-Sent Events endpoint with default settings.
pub fn connect(url: &str) -> Result<ClientStream> {
    ClientBuilder::new().connect(url)
}

/// Builder for configuring outgoing SSE client connections.
#[derive(Debug, Clone)]
pub struct ClientBuilder {
    connect_timeout: Option<Duration>,
    read_timeout: Option<Duration>,
    write_timeout: Option<Duration>,
    last_event_id: Option<String>,
    headers: Vec<(String, String)>,
    max_event_bytes: usize,
}

impl ClientBuilder {
    /// Creates a [`ClientBuilder`] with default settings (30s connect timeout,
    /// no idle read timeout, 1 MiB `max_event_bytes`).
    pub fn new() -> Self {
        Self {
            connect_timeout: Some(DEFAULT_CONNECT_TIMEOUT),
            read_timeout: None,
            write_timeout: Some(DEFAULT_CONNECT_TIMEOUT),
            last_event_id: None,
            headers: Vec::new(),
            max_event_bytes: DEFAULT_MAX_EVENT_BYTES,
        }
    }

    /// Sets the `Last-Event-ID` header sent on the initial SSE request.
    pub fn last_event_id(mut self, id: impl Into<String>) -> Self {
        self.last_event_id = Some(id.into());
        self
    }

    /// Adds a custom HTTP request header.
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// Sets the connect, read, and write timeouts together.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = Some(timeout);
        self.read_timeout = Some(timeout);
        self.write_timeout = Some(timeout);
        self
    }

    /// Sets the TCP connect timeout.
    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = Some(timeout);
        self
    }

    /// Sets or clears the socket read timeout applied while streaming events.
    pub fn read_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.read_timeout = timeout;
        self
    }

    /// Sets the maximum byte budget per SSE line / buffered event.
    pub fn max_event_bytes(mut self, max_event_bytes: usize) -> Self {
        self.max_event_bytes = max_event_bytes;
        self
    }

    /// Opens the HTTP/1.1 connection, validates the `200 OK` and `text/event-stream`
    /// response headers, and returns a live [`ClientStream`].
    pub fn connect(self, url: &str) -> Result<ClientStream> {
        let parsed = Url::parse(url).map_err(|e| Error::InvalidUrl(e.to_string()))?;

        match parsed.scheme() {
            "http" => {}
            other => return Err(Error::UnsupportedScheme(other.to_string())),
        }

        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err(Error::InvalidUrl(
                "userinfo in SSE URL is not supported".to_string(),
            ));
        }

        let host = parsed
            .host()
            .ok_or_else(|| Error::InvalidUrl(format!("missing host in '{url}'")))?
            .to_string();
        let port = parsed
            .port_or_default()
            .ok_or_else(|| Error::InvalidUrl(format!("missing port in '{url}'")))?;

        let connect_addr = format!("{host}:{port}");
        let host_header = if port == 80 {
            host
        } else {
            format!("{host}:{port}")
        };

        let path = if parsed.path().is_empty() {
            "/"
        } else {
            parsed.path()
        };
        let target = match parsed.query() {
            Some(q) => format!("{path}?{q}"),
            None => path.to_string(),
        };
        if !target.bytes().all(|b| (0x21..0x7F).contains(&b)) {
            return Err(Error::InvalidUrl(format!(
                "invalid HTTP request target: {target:?}"
            )));
        }

        let mut req_wire = Vec::with_capacity(256);
        let _ = write!(req_wire, "GET {target} HTTP/1.1\r\n");
        let _ = write!(req_wire, "Host: {host_header}\r\n");
        req_wire.extend_from_slice(b"Accept: text/event-stream\r\n");
        req_wire.extend_from_slice(b"Cache-Control: no-cache\r\n");

        if let Some(ref last_id) = self.last_event_id {
            validate_header_value("Last-Event-ID", last_id)?;
            let _ = write!(req_wire, "Last-Event-ID: {last_id}\r\n");
        }

        for (name, value) in &self.headers {
            validate_header_name(name)?;
            validate_header_value("header", value)?;
            let _ = write!(req_wire, "{name}: {value}\r\n");
        }
        req_wire.extend_from_slice(b"\r\n");

        let mut stream = connect_tcp(&connect_addr, self.connect_timeout, self.write_timeout)?;
        // Apply handshake read timeout while reading response headers.
        let header_read_timeout = self.read_timeout.or(self.connect_timeout);
        stream
            .set_read_timeout(header_read_timeout)
            .map_err(|e| map_io_err(e, header_read_timeout.is_some()))?;

        stream
            .write_all(&req_wire)
            .map_err(|e| map_io_err(e, self.write_timeout.is_some()))?;
        stream
            .flush()
            .map_err(|e| map_io_err(e, self.write_timeout.is_some()))?;

        let mut reader = BufReader::new(stream);
        let (status, headers) = read_response_head(&mut reader, header_read_timeout.is_some())?;

        if status != 200 {
            let mut body_buf = Vec::new();
            let _ = reader
                .by_ref()
                .take(MAX_ERROR_BODY_BYTES as u64)
                .read_to_end(&mut body_buf);
            let body = String::from_utf8_lossy(&body_buf).into_owned();
            return Err(Error::HttpStatus { status, body });
        }

        let content_type = headers
            .iter()
            .find(|(k, _)| k == "content-type")
            .map(|(_, v)| v.as_str())
            .unwrap_or("");
        let media_type = content_type.split(';').next().unwrap_or("").trim();
        if !media_type.eq_ignore_ascii_case("text/event-stream") {
            return Err(Error::InvalidContentType(content_type.to_string()));
        }

        // Switch socket to the configured streaming read timeout.
        reader
            .get_ref()
            .set_read_timeout(self.read_timeout)
            .map_err(|e| map_io_err(e, self.read_timeout.is_some()))?;

        let is_chunked = headers.iter().any(|(k, v)| {
            k == "transfer-encoding"
                && v.split(',')
                    .map(str::trim)
                    .any(|t| t.eq_ignore_ascii_case("chunked"))
        });

        let content_length = if is_chunked {
            None
        } else {
            headers
                .iter()
                .find(|(k, _)| k == "content-length")
                .and_then(|(_, v)| v.parse::<u64>().ok())
        };

        let body_reader = if is_chunked {
            HttpBodyReader::chunked(reader)
        } else if let Some(len) = content_length {
            HttpBodyReader::content_length(reader, len)
        } else {
            HttpBodyReader::close_delimited(reader)
        };

        let event_stream =
            EventStream::with_max_event_bytes(BufReader::new(body_reader), self.max_event_bytes);

        Ok(ClientStream {
            status,
            headers,
            stream: event_stream,
            has_read_timeout: self.read_timeout.is_some(),
        })
    }
}

impl Default for ClientBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Live blocking SSE client stream over HTTP/1.1.
#[derive(Debug)]
pub struct ClientStream {
    status: u16,
    headers: Vec<(String, String)>,
    stream: EventStream<BufReader<HttpBodyReader<BufReader<TcpStream>>>>,
    has_read_timeout: bool,
}

impl ClientStream {
    /// Reads and returns the next [`Event`], or `Ok(None)` when the server closes the stream.
    pub fn next_event(&mut self) -> Result<Option<Event>> {
        self.stream.next_event().map_err(|err| match err {
            Error::Io(io_err) => map_io_err(io_err, self.has_read_timeout),
            other => other,
        })
    }

    /// Returns the most recently observed valid `Last-Event-ID`.
    pub fn last_event_id(&self) -> &str {
        self.stream.last_event_id()
    }

    /// Returns the most recently parsed `retry:` duration, if any.
    pub const fn retry(&self) -> Option<Duration> {
        self.stream.retry()
    }

    /// Returns the HTTP response status code (always `200` on an established stream).
    pub const fn status(&self) -> u16 {
        self.status
    }

    /// Returns the first HTTP response header matching `name` (ASCII case-insensitive).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

impl Iterator for ClientStream {
    type Item = Result<Event>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.next_event() {
            Ok(Some(ev)) => Some(Ok(ev)),
            Ok(None) => None,
            Err(err) => Some(Err(err)),
        }
    }
}

/// HTTP/1.1 body reader supporting `Transfer-Encoding: chunked`, `Content-Length`,
/// and close-delimited responses while preserving real-time incremental delivery.
#[derive(Debug)]
enum BodyMode {
    CloseDelimited,
    ContentLength(u64),
    Chunked { remaining: usize, done: bool },
}

#[derive(Debug)]
struct HttpBodyReader<R> {
    inner: R,
    mode: BodyMode,
}

impl<R: BufRead> HttpBodyReader<R> {
    fn close_delimited(inner: R) -> Self {
        Self {
            inner,
            mode: BodyMode::CloseDelimited,
        }
    }

    fn content_length(inner: R, len: u64) -> Self {
        Self {
            inner,
            mode: BodyMode::ContentLength(len),
        }
    }

    fn chunked(inner: R) -> Self {
        Self {
            inner,
            mode: BodyMode::Chunked {
                remaining: 0,
                done: false,
            },
        }
    }
}

impl<R: BufRead> Read for HttpBodyReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }

        match &mut self.mode {
            BodyMode::CloseDelimited => self.inner.read(buf),
            BodyMode::ContentLength(remaining) => {
                if *remaining == 0 {
                    return Ok(0);
                }
                let max_take = (buf.len() as u64).min(*remaining) as usize;
                let n = self.inner.read(&mut buf[..max_take])?;
                *remaining -= n as u64;
                Ok(n)
            }
            BodyMode::Chunked { remaining, done } => {
                if *done {
                    return Ok(0);
                }
                if *remaining == 0 {
                    let mut size_line = String::new();
                    let read = read_crlf_line_io(&mut self.inner, &mut size_line)?;
                    if read == 0 {
                        return Err(io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            "unexpected EOF reading chunk size",
                        ));
                    }
                    let hex_part = size_line.split(';').next().unwrap_or("").trim();
                    let chunk_size = usize::from_str_radix(hex_part, 16).map_err(|_| {
                        io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!("invalid HTTP chunk size: {size_line:?}"),
                        )
                    })?;

                    if chunk_size == 0 {
                        // Consume optional trailer headers until blank line.
                        let mut trailer = String::new();
                        loop {
                            trailer.clear();
                            let n = read_crlf_line_io(&mut self.inner, &mut trailer)?;
                            if n == 0 || trailer.is_empty() {
                                break;
                            }
                        }
                        *done = true;
                        return Ok(0);
                    }
                    *remaining = chunk_size;
                }

                let max_take = buf.len().min(*remaining);
                let n = self.inner.read(&mut buf[..max_take])?;
                if n == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "unexpected EOF inside HTTP chunk body",
                    ));
                }
                *remaining -= n;
                if *remaining == 0 {
                    // Consume trailing CRLF after chunk payload.
                    let mut crlf = [0u8; 2];
                    self.inner.read_exact(&mut crlf)?;
                    if crlf != *b"\r\n" {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "missing CRLF after HTTP chunk payload",
                        ));
                    }
                }
                Ok(n)
            }
        }
    }
}

fn read_crlf_line_io<R: BufRead>(reader: &mut R, out: &mut String) -> io::Result<usize> {
    out.clear();
    let mut raw = Vec::new();
    let mut total = 0usize;
    loop {
        let buf = reader.fill_buf()?;
        if buf.is_empty() {
            break;
        }
        let (take, found_lf) = match buf.iter().position(|&b| b == b'\n') {
            Some(pos) => (pos + 1, true),
            None => (buf.len(), false),
        };
        if raw.len().saturating_add(take) > MAX_LINE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "HTTP chunk header line too long",
            ));
        }
        raw.extend_from_slice(&buf[..take]);
        reader.consume(take);
        total += take;
        if found_lf {
            break;
        }
    }
    if raw.ends_with(b"\r\n") {
        raw.truncate(raw.len() - 2);
    } else if raw.ends_with(b"\n") {
        raw.truncate(raw.len() - 1);
    }
    let s = String::from_utf8(raw)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "non-UTF-8 chunk header line"))?;
    out.push_str(&s);
    Ok(total)
}

fn read_response_head<R: BufRead>(
    reader: &mut R,
    has_timeout: bool,
) -> Result<(u16, Vec<(String, String)>)> {
    let mut total_bytes = 0usize;
    let status_line = read_http_line(reader, &mut total_bytes, has_timeout)?;
    let mut parts = status_line.splitn(3, ' ');
    let version = parts
        .next()
        .ok_or_else(|| Error::InvalidResponse("empty HTTP status line".to_string()))?;
    let status_str = parts
        .next()
        .ok_or_else(|| Error::InvalidResponse(format!("malformed status line: {status_line:?}")))?;
    if !version.starts_with("HTTP/1.") {
        return Err(Error::InvalidResponse(format!(
            "unsupported HTTP version: {version:?}"
        )));
    }
    let status: u16 = status_str
        .parse()
        .map_err(|_| Error::InvalidResponse(format!("invalid HTTP status: {status_str:?}")))?;

    let mut headers = Vec::new();
    loop {
        let line = read_http_line(reader, &mut total_bytes, has_timeout)?;
        if line.is_empty() {
            break;
        }
        if headers.len() >= MAX_HEADER_LINES {
            return Err(Error::InvalidResponse(format!(
                "response exceeds maximum of {MAX_HEADER_LINES} headers"
            )));
        }
        let Some((name, val)) = line.split_once(':') else {
            return Err(Error::InvalidResponse(format!(
                "malformed HTTP header line: {line:?}"
            )));
        };
        headers.push((name.trim().to_ascii_lowercase(), val.trim().to_string()));
    }

    Ok((status, headers))
}

fn read_http_line<R: BufRead>(
    reader: &mut R,
    total_bytes: &mut usize,
    has_timeout: bool,
) -> Result<String> {
    let mut raw = Vec::new();
    loop {
        let buf = reader.fill_buf().map_err(|e| map_io_err(e, has_timeout))?;
        if buf.is_empty() {
            return Err(Error::InvalidResponse(
                "unexpected EOF while reading HTTP response headers".to_string(),
            ));
        }
        let (take, found_lf) = match buf.iter().position(|&b| b == b'\n') {
            Some(pos) => (pos + 1, true),
            None => (buf.len(), false),
        };
        if raw.len().saturating_add(take) > MAX_LINE_BYTES {
            return Err(Error::InvalidResponse(format!(
                "HTTP response header line exceeds {MAX_LINE_BYTES} bytes"
            )));
        }
        *total_bytes = total_bytes.saturating_add(take);
        if *total_bytes > MAX_HEADER_BYTES {
            return Err(Error::InvalidResponse(format!(
                "HTTP response headers exceed {MAX_HEADER_BYTES} bytes"
            )));
        }
        raw.extend_from_slice(&buf[..take]);
        reader.consume(take);
        if found_lf {
            break;
        }
    }
    if raw.ends_with(b"\r\n") {
        raw.truncate(raw.len() - 2);
    } else if raw.ends_with(b"\n") {
        raw.truncate(raw.len() - 1);
    }
    String::from_utf8(raw)
        .map_err(|_| Error::InvalidResponse("HTTP headers must be valid UTF-8".to_string()))
}

fn validate_header_name(name: &str) -> Result<()> {
    let valid = !name.is_empty()
        && name.bytes().all(|b| {
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
        });
    if !valid {
        return Err(Error::InvalidField {
            field: "header",
            message: format!("invalid HTTP header name: {name:?}"),
        });
    }
    Ok(())
}

fn validate_header_value(field: &'static str, value: &str) -> Result<()> {
    if value.bytes().any(|b| b == b'\r' || b == b'\n' || b == 0) {
        return Err(Error::InvalidField {
            field,
            message: "value must not contain CR, LF, or NUL".to_string(),
        });
    }
    Ok(())
}

fn connect_tcp(
    addr: &str,
    connect_timeout: Option<Duration>,
    write_timeout: Option<Duration>,
) -> Result<TcpStream> {
    let has_timeout = connect_timeout.is_some();
    let addrs: Vec<_> = addr
        .to_socket_addrs()
        .map_err(|e| map_io_err(e, has_timeout))?
        .collect();
    if addrs.is_empty() {
        return Err(Error::InvalidUrl(format!(
            "failed to resolve SSE host '{addr}'"
        )));
    }

    let mut last_err = None;
    for sock_addr in addrs {
        let res = match connect_timeout {
            Some(dur) => TcpStream::connect_timeout(&sock_addr, dur),
            None => TcpStream::connect(sock_addr),
        };
        match res {
            Ok(stream) => {
                stream
                    .set_write_timeout(write_timeout)
                    .map_err(|e| map_io_err(e, write_timeout.is_some()))?;
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
