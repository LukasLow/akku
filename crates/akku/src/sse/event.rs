//! Server-Sent Events [`Event`] builder and CR/LF-safe wire encoder.

use std::fmt::Write as _;
use std::io::Write;
use std::time::Duration;

use crate::sse::error::{Error, Result};

/// A single WHATWG Server-Sent Event.
///
/// Use the builder methods ([`Event::data`], [`Event::event`], [`Event::id`],
/// [`Event::retry`], [`Event::comment`]) to construct events and
/// [`Event::try_to_wire_string`] or [`Event::to_wire_string`] to serialize them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Event {
    event: Option<String>,
    data: String,
    has_data: bool,
    id: Option<String>,
    retry: Option<Duration>,
    comments: Vec<String>,
}

impl Event {
    /// Creates an empty [`Event`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates an [`Event`] containing only `data`.
    pub fn data_only(data: impl Into<String>) -> Self {
        Self::new().data(data)
    }

    /// Sets the `data:` payload of this event (multi-line strings are split into
    /// separate `data:` lines during wire serialization).
    pub fn data(mut self, data: impl Into<String>) -> Self {
        self.data = data.into();
        self.has_data = true;
        self
    }

    /// Sets the `event:` name of this event.
    pub fn event(mut self, name: impl Into<String>) -> Self {
        self.event = Some(name.into());
        self
    }

    /// Sets the `id:` field of this event.
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Sets the `retry:` reconnection delay of this event.
    pub fn retry(mut self, retry: Duration) -> Self {
        self.retry = Some(retry);
        self
    }

    /// Sets the `retry:` reconnection delay in milliseconds.
    pub fn retry_ms(self, ms: u64) -> Self {
        self.retry(Duration::from_millis(ms))
    }

    /// Appends a `: comment` line emitted before the event fields.
    pub fn comment(mut self, comment: impl Into<String>) -> Self {
        self.comments.push(comment.into());
        self
    }

    /// Internal constructor used by [`crate::sse::Parser`] when dispatching a parsed event.
    pub(crate) fn from_parsed(
        event: Option<String>,
        data: String,
        id: Option<String>,
        retry: Option<Duration>,
    ) -> Self {
        Self {
            event,
            data,
            has_data: true,
            id,
            retry,
            comments: Vec::new(),
        }
    }

    /// Returns the event type, defaulting to `"message"` per WHATWG HTML §9.2.6.
    pub fn event_type(&self) -> &str {
        match self.event.as_deref() {
            Some(name) if !name.is_empty() => name,
            _ => "message",
        }
    }

    /// Returns the explicitly configured `event:` name, if any.
    pub fn event_name(&self) -> Option<&str> {
        self.event.as_deref()
    }

    /// Returns the event's `data:` payload.
    pub fn data_str(&self) -> &str {
        &self.data
    }

    /// Returns the event's `id:` value, if present.
    pub fn id_str(&self) -> Option<&str> {
        self.id.as_deref()
    }

    /// Returns the event's `retry:` reconnection delay, if present.
    pub fn retry_duration(&self) -> Option<Duration> {
        self.retry
    }

    /// Returns any comment lines attached to this event.
    pub fn comments(&self) -> &[String] {
        &self.comments
    }

    /// Strictly validates that single-line fields cannot inject wire frame breaks.
    ///
    /// Rejects `\r` or `\n` in `event` or `id`, and rejects `\0` (NUL) in `id`
    /// (per WHATWG HTML §9.2.6).
    pub fn validate(&self) -> Result<()> {
        if let Some(name) = &self.event {
            if name.contains('\r') || name.contains('\n') {
                return Err(Error::InvalidField {
                    field: "event",
                    message: "event name must not contain CR or LF".to_string(),
                });
            }
        }
        if let Some(id) = &self.id {
            if id.contains('\r') || id.contains('\n') {
                return Err(Error::InvalidField {
                    field: "id",
                    message: "event id must not contain CR or LF".to_string(),
                });
            }
            if id.contains('\0') {
                return Err(Error::InvalidField {
                    field: "id",
                    message: "event id must not contain NUL (U+0000)".to_string(),
                });
            }
        }
        Ok(())
    }

    /// Validates this event and encodes it into WHATWG SSE wire format terminated by `\n\n`.
    pub fn try_to_wire_string(&self) -> Result<String> {
        self.validate()?;
        Ok(self.encode_wire(false))
    }

    /// Validates this event and writes its wire representation to `writer`.
    pub fn write_to<W: Write>(&self, writer: &mut W) -> Result<()> {
        let wire = self.try_to_wire_string()?;
        writer.write_all(wire.as_bytes())?;
        Ok(())
    }

    /// Encodes this event into WHATWG SSE wire format, sanitizing any forbidden
    /// `\r`, `\n`, or `\0` characters in `event` / `id` so CR/LF field injection
    /// cannot occur.
    pub fn to_wire_string(&self) -> String {
        self.encode_wire(true)
    }

    fn encode_wire(&self, sanitize: bool) -> String {
        let mut out = String::with_capacity(self.data.len() + 32);

        for comment in &self.comments {
            for line in split_sse_lines(comment) {
                if line.is_empty() {
                    out.push_str(":\n");
                } else {
                    out.push_str(": ");
                    out.push_str(line);
                    out.push('\n');
                }
            }
        }

        if let Some(name) = &self.event {
            if sanitize {
                let clean: String = name.chars().filter(|&c| c != '\r' && c != '\n').collect();
                if !clean.is_empty() {
                    out.push_str("event: ");
                    out.push_str(&clean);
                    out.push('\n');
                }
            } else if !name.is_empty() {
                out.push_str("event: ");
                out.push_str(name);
                out.push('\n');
            }
        }

        if let Some(id) = &self.id {
            let clean: String = if sanitize {
                id.chars()
                    .filter(|&c| c != '\r' && c != '\n' && c != '\0')
                    .collect()
            } else {
                id.clone()
            };
            if clean.is_empty() {
                out.push_str("id:\n");
            } else {
                out.push_str("id: ");
                out.push_str(&clean);
                out.push('\n');
            }
        }

        if let Some(retry) = self.retry {
            let _ = writeln!(out, "retry: {}", retry.as_millis());
        }

        if self.has_data {
            for line in split_sse_lines(&self.data) {
                if line.is_empty() {
                    out.push_str("data:\n");
                } else {
                    out.push_str("data: ");
                    out.push_str(line);
                    out.push('\n');
                }
            }
        }

        out.push('\n');
        out
    }
}

/// Splits `s` on `\r\n`, `\n`, or `\r`, preserving empty lines (including trailing empty lines).
pub(crate) fn split_sse_lines(s: &str) -> Vec<&str> {
    let bytes = s.as_bytes();
    let mut lines = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'\r' => {
                lines.push(&s[start..i]);
                if i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
                    i += 2;
                } else {
                    i += 1;
                }
                start = i;
            }
            b'\n' => {
                lines.push(&s[start..i]);
                i += 1;
                start = i;
            }
            _ => {
                i += 1;
            }
        }
    }
    lines.push(&s[start..]);
    lines
}

#[cfg(feature = "http")]
impl axum::response::IntoResponse for Event {
    fn into_response(self) -> axum::response::Response {
        (
            [
                (
                    axum::http::header::CONTENT_TYPE,
                    "text/event-stream; charset=utf-8",
                ),
                (axum::http::header::CACHE_CONTROL, "no-cache"),
            ],
            self.to_wire_string(),
        )
            .into_response()
    }
}
