//! Zero-dependency SSE server streaming writer ([`SseWriter`]) and buffered
//! [`SseResponse`] helper (with optional `#[cfg(feature = "http")]` integration).

use std::fmt::Write as _;
use std::io::Write;
use std::time::Duration;

use crate::sse::error::{Error, Result};
use crate::sse::event::{split_sse_lines, Event};

/// Synchronous streaming Server-Sent Events writer over any `W: Write`.
///
/// Works over [`std::net::TcpStream`], in-memory buffers, or pipes with zero
/// external dependencies. Supports raw SSE framing ([`SseWriter::new`]),
/// HTTP/1.1 streaming ([`SseWriter::begin_http`]), and HTTP/1.1 chunked
/// transfer encoding ([`SseWriter::begin_http_chunked`]).
#[derive(Debug)]
pub struct SseWriter<W> {
    writer: W,
    chunked: bool,
    finished: bool,
}

impl<W: Write> SseWriter<W> {
    /// Wraps `writer` for raw SSE wire output without writing an HTTP response head.
    pub fn new(writer: W) -> Self {
        Self {
            writer,
            chunked: false,
            finished: false,
        }
    }

    /// Writes `HTTP/1.1 200 OK` SSE response headers (`Content-Type: text/event-stream; charset=utf-8`,
    /// `Cache-Control: no-cache`, `Connection: keep-alive`), flushes, and returns an [`SseWriter`].
    pub fn begin_http(mut writer: W) -> Result<Self> {
        writer.write_all(
            b"HTTP/1.1 200 OK\r\n\
              Content-Type: text/event-stream; charset=utf-8\r\n\
              Cache-Control: no-cache\r\n\
              Connection: keep-alive\r\n\r\n",
        )?;
        writer.flush()?;
        Ok(Self {
            writer,
            chunked: false,
            finished: false,
        })
    }

    /// Same as [`SseWriter::begin_http`] but adds `Transfer-Encoding: chunked` and frames
    /// every write as an RFC 9112 HTTP/1.1 chunk. Call [`SseWriter::finish`] to emit the
    /// terminating `0\r\n\r\n` chunk.
    pub fn begin_http_chunked(mut writer: W) -> Result<Self> {
        writer.write_all(
            b"HTTP/1.1 200 OK\r\n\
              Content-Type: text/event-stream; charset=utf-8\r\n\
              Cache-Control: no-cache\r\n\
              Transfer-Encoding: chunked\r\n\
              Connection: keep-alive\r\n\r\n",
        )?;
        writer.flush()?;
        Ok(Self {
            writer,
            chunked: true,
            finished: false,
        })
    }

    /// Validates, writes, and flushes one [`Event`].
    pub fn send(&mut self, event: &Event) -> Result<()> {
        let wire = event.try_to_wire_string()?;
        self.write_payload(wire.as_bytes())
    }

    /// Writes and flushes a simple `data:`-only event.
    pub fn send_data(&mut self, data: &str) -> Result<()> {
        self.send(&Event::data_only(data))
    }

    /// Writes and flushes a keep-alive comment block (`: <comment>\n\n`).
    pub fn send_comment(&mut self, comment: &str) -> Result<()> {
        let mut out = String::with_capacity(comment.len() + 8);
        for line in split_sse_lines(comment) {
            if line.is_empty() {
                out.push_str(":\n");
            } else {
                out.push_str(": ");
                out.push_str(line);
                out.push('\n');
            }
        }
        out.push('\n');
        self.write_payload(out.as_bytes())
    }

    /// Writes and flushes a `retry:` directive (`retry: <ms>\n\n`).
    pub fn send_retry(&mut self, retry: Duration) -> Result<()> {
        let wire = format!("retry: {}\n\n", retry.as_millis());
        self.write_payload(wire.as_bytes())
    }

    /// Flushes the underlying writer.
    pub fn flush(&mut self) -> Result<()> {
        self.writer.flush()?;
        Ok(())
    }

    /// Finalizes the stream (emitting `0\r\n\r\n` when chunked), flushes, and returns `W`.
    pub fn finish(mut self) -> Result<W> {
        if self.chunked && !self.finished {
            self.writer.write_all(b"0\r\n\r\n")?;
            self.finished = true;
        }
        self.writer.flush()?;
        Ok(self.writer)
    }

    /// Consumes the [`SseWriter`] and returns the inner writer without writing a final chunk.
    pub fn into_inner(self) -> W {
        self.writer
    }

    fn write_payload(&mut self, bytes: &[u8]) -> Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        if self.chunked {
            let mut header = String::new();
            let _ = write!(header, "{:X}\r\n", bytes.len());
            self.writer.write_all(header.as_bytes())?;
            self.writer.write_all(bytes)?;
            self.writer.write_all(b"\r\n")?;
        } else {
            self.writer.write_all(bytes)?;
        }
        self.writer.flush()?;
        Ok(())
    }
}

/// Buffered SSE response builder that works under `--no-default-features` and integrates
/// with [`crate::http`] (`axum::response::IntoResponse`) when the `http` feature is enabled.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SseResponse {
    wire: String,
    headers: Vec<(String, String)>,
}

impl SseResponse {
    /// Creates an empty [`SseResponse`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Validates and appends one [`Event`] to the response buffer.
    pub fn event(mut self, event: &Event) -> Result<Self> {
        let encoded = event.try_to_wire_string()?;
        self.wire.push_str(&encoded);
        Ok(self)
    }

    /// Validates and appends multiple [`Event`]s to the response buffer.
    pub fn events<'a>(mut self, events: impl IntoIterator<Item = &'a Event>) -> Result<Self> {
        for ev in events {
            let encoded = ev.try_to_wire_string()?;
            self.wire.push_str(&encoded);
        }
        Ok(self)
    }

    /// Appends a comment block to the response buffer.
    pub fn comment(mut self, comment: &str) -> Self {
        for line in split_sse_lines(comment) {
            if line.is_empty() {
                self.wire.push_str(":\n");
            } else {
                self.wire.push_str(": ");
                self.wire.push_str(line);
                self.wire.push('\n');
            }
        }
        self.wire.push('\n');
        self
    }

    /// Appends a `retry:` directive to the response buffer.
    pub fn retry(mut self, retry: Duration) -> Self {
        let _ = write!(self.wire, "retry: {}\n\n", retry.as_millis());
        self
    }

    /// Adds an extra HTTP response header (validates against CR/LF/NUL injection).
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Result<Self> {
        let name = name.into();
        let value = value.into();
        if name.is_empty()
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(Error::InvalidField {
                field: "header",
                message: format!("invalid HTTP header name: {name:?}"),
            });
        }
        if value.bytes().any(|b| b == b'\r' || b == b'\n' || b == 0) {
            return Err(Error::InvalidField {
                field: "header",
                message: "header value must not contain CR, LF, or NUL".to_string(),
            });
        }
        self.headers.push((name, value));
        Ok(self)
    }

    /// Borrows the accumulated SSE wire text.
    pub fn as_wire_str(&self) -> &str {
        &self.wire
    }

    /// Consumes the builder and returns the accumulated SSE wire text.
    pub fn into_wire_string(self) -> String {
        self.wire
    }

    /// Converts this [`SseResponse`] into an [`crate::http::Response`] with
    /// `Content-Type: text/event-stream; charset=utf-8` and `Cache-Control: no-cache`.
    #[cfg(feature = "http")]
    pub fn into_http_response(self) -> crate::http::Response {
        let mut builder = crate::http::ResponseBuilder::new(crate::http::StatusCode::OK)
            .content_type("text/event-stream; charset=utf-8")
            .header("cache-control", "no-cache");
        for (k, v) in self.headers {
            builder = builder.header(k, v);
        }
        builder.body(self.wire.into_bytes())
    }
}

#[cfg(feature = "http")]
impl axum::response::IntoResponse for SseResponse {
    fn into_response(self) -> axum::response::Response {
        self.into_http_response()
    }
}
