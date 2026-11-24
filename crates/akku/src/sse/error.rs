//! Error types for [`crate::sse`].

use std::fmt;
use std::io;

/// Errors returned by Server-Sent Events parsing, encoding, client, and server operations.
#[non_exhaustive]
#[derive(Debug)]
pub enum Error {
    /// An underlying stream or socket I/O error occurred.
    Io(io::Error),
    /// The target SSE URL could not be parsed or is invalid.
    InvalidUrl(String),
    /// The URL scheme is not supported (for example `https` without TLS).
    UnsupportedScheme(String),
    /// An SSE event field or HTTP header contained forbidden characters (such as CR, LF, or NUL).
    InvalidField {
        /// The field or header name that failed validation.
        field: &'static str,
        /// A human-readable description of why the value was rejected.
        message: String,
    },
    /// An incoming line or accumulated event exceeded the configured `max_event_bytes` cap.
    EventTooLarge {
        /// The configured maximum event size in bytes.
        max: usize,
    },
    /// The server returned a malformed HTTP response or invalid chunked transfer encoding.
    InvalidResponse(String),
    /// The server responded with a non-200 HTTP status code.
    HttpStatus {
        /// The HTTP status code returned by the server.
        status: u16,
        /// A snippet of the HTTP response body decoded lossily as UTF-8.
        body: String,
    },
    /// The server responded with a `Content-Type` other than `text/event-stream`.
    InvalidContentType(String),
    /// Connecting, reading, or writing timed out.
    TimedOut,
}

/// Convenience result alias for [`crate::sse`] operations.
pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "SSE I/O error: {err}"),
            Self::InvalidUrl(msg) => write!(f, "invalid SSE URL: {msg}"),
            Self::UnsupportedScheme(scheme) if scheme.eq_ignore_ascii_case("https") => {
                write!(
                    f,
                    "unsupported SSE scheme '{scheme}': HTTPS/TLS requires akku::tls"
                )
            }
            Self::UnsupportedScheme(scheme) => {
                write!(
                    f,
                    "unsupported SSE scheme '{scheme}': only 'http' is supported"
                )
            }
            Self::InvalidField { field, message } => {
                write!(f, "invalid SSE field '{field}': {message}")
            }
            Self::EventTooLarge { max } => {
                write!(f, "SSE event exceeded maximum size of {max} bytes")
            }
            Self::InvalidResponse(msg) => write!(f, "invalid SSE HTTP response: {msg}"),
            Self::HttpStatus { status, body } => {
                if body.is_empty() {
                    write!(f, "SSE request failed with HTTP status {status}")
                } else {
                    write!(f, "SSE request failed with HTTP status {status}: {body}")
                }
            }
            Self::InvalidContentType(ct) => {
                write!(
                    f,
                    "invalid SSE Content-Type '{ct}': expected 'text/event-stream'"
                )
            }
            Self::TimedOut => f.write_str("SSE operation timed out"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

/// Maps an [`io::Error`] into [`Error::TimedOut`] when timeouts are enabled.
pub(crate) fn map_io_err(err: io::Error, has_timeout: bool) -> Error {
    if has_timeout
        && matches!(
            err.kind(),
            io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
        )
    {
        Error::TimedOut
    } else {
        Error::Io(err)
    }
}
