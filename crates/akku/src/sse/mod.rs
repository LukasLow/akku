//! Zero-dependency WHATWG Server-Sent Events parser, encoder, and stream helpers.
//!
//! `akku::sse` works without any external dependencies (including under
//! `--no-default-features`) and provides:
//!
//! - CR/LF-safe [`Event`] builder and wire serializer supporting multi-line `data:`,
//!   `event:`, `id:`, `retry:`, and `: comment` lines.
//! - Incremental, chunk-boundary-safe WHATWG HTML §9.2.6 [`Parser`] and pull-based
//!   [`EventStream`] with UTF-8 BOM stripping, `\r\n` / `\r` / `\n` line endings,
//!   `Last-Event-ID` tracking (rejecting NUL), `retry:` parsing, and `max_event_bytes`
//!   DoS caps.
//! - Blocking HTTP/1.1 [`client`] supporting raw and `Transfer-Encoding: chunked`
//!   responses with `Last-Event-ID`, plus [`server::SseWriter`] and
//!   [`server::SseResponse`] (which implements `IntoResponse` under `feature = "http"`).
//!
//! # Examples
//!
//! ```
//! use akku::sse::{Event, Parser};
//!
//! let wire = Event::new()
//!     .event("update")
//!     .id("42")
//!     .data("line 1\nline 2")
//!     .try_to_wire_string()
//!     .unwrap();
//!
//! let mut parser = Parser::new();
//! let events = parser.push(wire.as_bytes()).unwrap();
//! assert_eq!(events.len(), 1);
//! assert_eq!(events[0].event_type(), "update");
//! assert_eq!(events[0].id_str(), Some("42"));
//! assert_eq!(events[0].data_str(), "line 1\nline 2");
//! assert_eq!(parser.last_event_id(), "42");
//! ```

pub mod client;
mod error;
mod event;
mod parser;
pub mod server;

pub use error::{Error, Result};
pub use event::Event;
pub use parser::{EventStream, Parser, DEFAULT_MAX_EVENT_BYTES};
