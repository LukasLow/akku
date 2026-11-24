//! Error types for [`crate::ws`].

use std::fmt;
use std::io;
use std::str::Utf8Error;

/// Errors returned by WebSocket framing, handshake, and transport operations.
#[non_exhaustive]
#[derive(Debug)]
pub enum Error {
    /// An underlying stream or socket I/O error occurred.
    Io(io::Error),
    /// The target WebSocket URL could not be parsed or is invalid.
    InvalidUrl(String),
    /// The URL scheme is not supported (for example `wss` without TLS).
    UnsupportedScheme(String),
    /// A frame carried an unknown or reserved 4-bit opcode.
    InvalidOpcode(u8),
    /// A Close frame carried a status code forbidden on the wire by RFC 6455 §7.4.
    InvalidCloseCode(u16),
    /// A Text frame or Close reason contained invalid UTF-8 bytes.
    InvalidUtf8(Utf8Error),
    /// A single frame declared a payload length exceeding [`crate::ws::Config::max_frame_size`].
    FrameTooLarge {
        /// The payload length declared on the wire.
        len: u64,
        /// The configured maximum frame size in bytes.
        max: usize,
    },
    /// Reassembled message bytes exceeded [`crate::ws::Config::max_message_size`].
    MessageTooLarge {
        /// The accumulated message size in bytes.
        len: usize,
        /// The configured maximum message size in bytes.
        max: usize,
    },
    /// A peer violated RFC 6455 framing or state-machine rules.
    Protocol(String),
    /// The HTTP/1.1 WebSocket opening handshake was malformed or rejected.
    Handshake(String),
    /// The server responded to the opening handshake with a non-101 HTTP status.
    HandshakeStatus {
        /// The HTTP status code returned by the server.
        status: u16,
        /// The HTTP reason phrase returned by the server.
        reason: String,
    },
    /// An operation was attempted on a WebSocket connection that has already closed.
    ConnectionClosed,
    /// Connecting, reading, or writing timed out.
    TimedOut,
}

/// Convenience result alias for [`crate::ws`] operations.
pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "WebSocket I/O error: {err}"),
            Self::InvalidUrl(msg) => write!(f, "invalid WebSocket URL: {msg}"),
            Self::UnsupportedScheme(scheme) if scheme.eq_ignore_ascii_case("wss") => {
                write!(
                    f,
                    "unsupported WebSocket scheme '{scheme}': TLS/wss requires akku::tls"
                )
            }
            Self::UnsupportedScheme(scheme) => {
                write!(
                    f,
                    "unsupported WebSocket scheme '{scheme}': only 'ws' is supported"
                )
            }
            Self::InvalidOpcode(op) => write!(f, "invalid WebSocket opcode: 0x{op:x}"),
            Self::InvalidCloseCode(code) => {
                write!(f, "invalid WebSocket close status code on wire: {code}")
            }
            Self::InvalidUtf8(err) => write!(f, "invalid UTF-8 in WebSocket payload: {err}"),
            Self::FrameTooLarge { len, max } => {
                write!(
                    f,
                    "WebSocket frame payload length {len} exceeds maximum {max} bytes"
                )
            }
            Self::MessageTooLarge { len, max } => {
                write!(
                    f,
                    "WebSocket message length {len} exceeds maximum {max} bytes"
                )
            }
            Self::Protocol(msg) => write!(f, "WebSocket protocol violation: {msg}"),
            Self::Handshake(msg) => write!(f, "WebSocket handshake error: {msg}"),
            Self::HandshakeStatus { status, reason } => {
                if reason.is_empty() {
                    write!(f, "WebSocket handshake rejected with HTTP status {status}")
                } else {
                    write!(
                        f,
                        "WebSocket handshake rejected with HTTP status {status} {reason}"
                    )
                }
            }
            Self::ConnectionClosed => f.write_str("WebSocket connection is closed"),
            Self::TimedOut => f.write_str("WebSocket operation timed out"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::InvalidUtf8(err) => Some(err),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<Utf8Error> for Error {
    fn from(err: Utf8Error) -> Self {
        Self::InvalidUtf8(err)
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
