//! Incremental WHATWG HTML §9.2.6 Server-Sent Events parser and [`EventStream`].

use std::collections::VecDeque;
use std::io::BufRead;
use std::time::Duration;

use crate::sse::error::{Error, Result};
use crate::sse::event::Event;

/// Default maximum byte budget per SSE line / buffered event (1 MiB).
pub const DEFAULT_MAX_EVENT_BYTES: usize = 1024 * 1024;

const UTF8_BOM: [u8; 3] = [0xEF, 0xBB, 0xBF];

/// Incremental, chunk-boundary-safe WHATWG Server-Sent Events parser.
///
/// Accepts arbitrary byte chunks via [`Parser::push`] (including 1 byte at a time)
/// or pre-split lines via [`Parser::push_line`]. Handles leading UTF-8 BOM stripping,
/// `\r\n` / `\r` / `\n` line endings, comment lines, multi-line `data:` concatenation,
/// persistent `Last-Event-ID` tracking (rejecting NUL), `retry:` milliseconds parsing,
/// and `max_event_bytes` DoS caps.
///
/// # Oversized events
///
/// When a line or accumulated event exceeds `max_event_bytes`, the parser drops
/// the **entire offending event**: every remaining byte up to and including the
/// next blank line (event terminator) is discarded, so the tail of an oversized
/// line can never be reinterpreted as new fields (field injection). Parsing then
/// resumes normally with the following event.
///
/// Consecutive oversized events (with no valid event completed in between) are
/// coalesced into a **single** queued [`Error::EventTooLarge`], so a flood of
/// oversized events in one chunk cannot grow the result queue without bound.
///
/// # Last event ID
///
/// An `id:` field only updates the *pending* last-event-ID buffer.
/// [`Parser::last_event_id`] is committed when the event is dispatched by a
/// blank line (WHATWG HTML §9.2.6). An `id:` belonging to an event that is
/// dropped (oversized, or incomplete at [`Parser::finish`]) never advances it.
///
/// [`Parser::push`] always consumes the whole chunk. Results are delivered in
/// stream order: events completed *before* an overflow are returned first as
/// `Ok(events)`; the [`Error::EventTooLarge`] is returned by the next call; events
/// completed after it follow on the call after that. Call `push(&[])` (or check
/// [`Parser::has_pending`]) to drain queued results without feeding new bytes.
///
/// ```
/// use akku::sse::{Error, Parser};
///
/// let mut parser = Parser::with_max_event_bytes(16);
/// let wire = b"data: a\n\ndata: xxxxxxxxxxxxxxxxxxxxxxxxxx\n\ndata: b\n\n";
/// let first = parser.push(wire).unwrap();
/// assert_eq!(first[0].data_str(), "a");
/// assert!(matches!(parser.push(&[]), Err(Error::EventTooLarge { max: 16 })));
/// let rest = parser.push(&[]).unwrap();
/// assert_eq!(rest[0].data_str(), "b");
/// assert!(!parser.has_pending());
/// ```
#[derive(Debug, Clone)]
pub struct Parser {
    bom_buf: [u8; 3],
    bom_len: usize,
    bom_done: bool,
    pending_cr: bool,
    line_buf: Vec<u8>,
    data_buf: String,
    has_data: bool,
    event_buf: String,
    /// Committed last event ID (updated only on dispatch).
    last_event_id: String,
    /// WHATWG "last event ID buffer" for the event currently being parsed.
    id_buf: String,
    retry: Option<Duration>,
    pending_retry: Option<Duration>,
    max_event_bytes: usize,
    /// `true` while discarding the remainder of an oversized event.
    discarding: bool,
    /// `true` when the line currently being discarded has at least one byte.
    discard_line_nonempty: bool,
    /// Completed events and deferred overflow errors in stream order.
    queue: VecDeque<Item>,
}

/// One queued parser result (kept `Clone`-able, unlike [`Error`]).
#[derive(Debug, Clone)]
enum Item {
    Event(Event),
    TooLarge,
}

impl Parser {
    /// Creates a [`Parser`] with [`DEFAULT_MAX_EVENT_BYTES`] (1 MiB).
    pub fn new() -> Self {
        Self::with_max_event_bytes(DEFAULT_MAX_EVENT_BYTES)
    }

    /// Creates a [`Parser`] with a custom `max_event_bytes` DoS cap.
    pub fn with_max_event_bytes(max_event_bytes: usize) -> Self {
        Self {
            bom_buf: [0; 3],
            bom_len: 0,
            bom_done: false,
            pending_cr: false,
            line_buf: Vec::new(),
            data_buf: String::new(),
            has_data: false,
            event_buf: String::new(),
            last_event_id: String::new(),
            id_buf: String::new(),
            retry: None,
            pending_retry: None,
            max_event_bytes,
            discarding: false,
            discard_line_nonempty: false,
            queue: VecDeque::new(),
        }
    }

    /// Feeds an arbitrary chunk of bytes and returns any [`Event`]s completed by blank lines.
    ///
    /// The whole chunk is always consumed. If an event overflows `max_event_bytes`,
    /// events completed earlier are returned first and the
    /// [`Error::EventTooLarge`] is surfaced by the next call (see the
    /// [type-level docs](Parser#oversized-events)); `push(&[])` drains queued results.
    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<Event>> {
        for &byte in chunk {
            if !self.bom_done {
                if byte == UTF8_BOM[self.bom_len] {
                    self.bom_buf[self.bom_len] = byte;
                    self.bom_len += 1;
                    if self.bom_len == 3 {
                        self.bom_done = true;
                        self.bom_len = 0;
                    }
                    continue;
                } else {
                    self.bom_done = true;
                    let prefix_len = self.bom_len;
                    let prefix = self.bom_buf;
                    self.bom_len = 0;
                    for &prev in &prefix[..prefix_len] {
                        self.feed_byte(prev);
                    }
                }
            }

            self.feed_byte(byte);
        }
        self.drain_queue()
    }

    /// Returns `true` when completed events or a deferred error are queued and
    /// can be retrieved with `push(&[])`.
    pub fn has_pending(&self) -> bool {
        !self.queue.is_empty()
    }

    fn drain_queue(&mut self) -> Result<Vec<Event>> {
        if matches!(self.queue.front(), Some(Item::TooLarge)) {
            self.queue.pop_front();
            return Err(Error::EventTooLarge {
                max: self.max_event_bytes,
            });
        }
        let mut events = Vec::new();
        while let Some(Item::Event(_)) = self.queue.front() {
            if let Some(Item::Event(ev)) = self.queue.pop_front() {
                events.push(ev);
            }
        }
        Ok(events)
    }

    /// Feeds a single already-delimited line (without trailing `\r` or `\n`).
    ///
    /// Returns `Ok(Some(event))` when an empty line dispatches a buffered event.
    ///
    /// While the remainder of an oversized event is being discarded, non-empty
    /// lines are ignored and the next empty line ends the discarded event.
    pub fn push_line(&mut self, line: &str) -> Result<Option<Event>> {
        if self.discarding {
            self.bom_done = true;
            if line.is_empty() {
                self.discarding = false;
            }
            return Ok(None);
        }
        if !self.bom_done {
            self.bom_done = true;
            if let Some(stripped) = line.strip_prefix('\u{FEFF}') {
                return self.process_line_guarded(stripped);
            }
        }
        self.process_line_guarded(line)
    }

    /// Runs [`Self::process_line`] and enters discard mode on overflow.
    fn process_line_guarded(&mut self, line: &str) -> Result<Option<Event>> {
        let result = self.process_line(line);
        if result.is_err() {
            // The offending line has already ended; discard until the blank line.
            self.start_discard(false);
        }
        result
    }

    fn start_discard(&mut self, line_nonempty: bool) {
        self.line_buf.clear();
        self.reset_event_buffers();
        self.discarding = true;
        self.discard_line_nonempty = line_nonempty;
    }

    /// Signals end-of-stream: discards any incomplete buffered event per WHATWG HTML §9.2.6.
    pub fn finish(&mut self) {
        self.bom_done = true;
        self.bom_len = 0;
        self.pending_cr = false;
        self.line_buf.clear();
        self.data_buf.clear();
        self.has_data = false;
        self.event_buf.clear();
        self.pending_retry = None;
        self.id_buf.clone_from(&self.last_event_id);
        self.discarding = false;
        self.discard_line_nonempty = false;
    }

    /// Returns the last event ID committed by a dispatched event (empty string if unset or reset).
    ///
    /// Pending `id:` fields of events not yet terminated by a blank line are not reflected.
    pub fn last_event_id(&self) -> &str {
        &self.last_event_id
    }

    /// Returns the most recently parsed valid `retry:` duration, if any.
    pub const fn retry(&self) -> Option<Duration> {
        self.retry
    }

    /// Returns the configured maximum byte size per event / line.
    pub const fn max_event_bytes(&self) -> usize {
        self.max_event_bytes
    }

    fn feed_byte(&mut self, byte: u8) {
        if self.pending_cr {
            self.pending_cr = false;
            if byte == b'\n' {
                return;
            }
        }

        if self.discarding {
            match byte {
                b'\r' | b'\n' => {
                    self.pending_cr = byte == b'\r';
                    if self.discard_line_nonempty {
                        self.discard_line_nonempty = false;
                    } else {
                        // Blank line: the discarded event is over; do not dispatch.
                        self.discarding = false;
                    }
                }
                _ => self.discard_line_nonempty = true,
            }
            return;
        }

        match byte {
            b'\r' => {
                self.pending_cr = true;
                self.flush_line_buf();
            }
            b'\n' => self.flush_line_buf(),
            other => {
                if self.line_buf.len() >= self.max_event_bytes {
                    self.queue_too_large();
                    self.start_discard(true);
                    return;
                }
                self.line_buf.push(other);
            }
        }
    }

    /// Queues a deferred overflow error, coalescing it with an already-queued one.
    fn queue_too_large(&mut self) {
        if !matches!(self.queue.back(), Some(Item::TooLarge)) {
            self.queue.push_back(Item::TooLarge);
        }
    }

    fn flush_line_buf(&mut self) {
        let raw = std::mem::take(&mut self.line_buf);
        let line = String::from_utf8_lossy(&raw);
        match self.process_line_guarded(&line) {
            Ok(Some(ev)) => self.queue.push_back(Item::Event(ev)),
            Ok(None) => {}
            Err(_) => self.queue_too_large(),
        }
        // Reuse the line allocation.
        drop(line);
        self.line_buf = raw;
        self.line_buf.clear();
    }

    fn process_line(&mut self, line: &str) -> Result<Option<Event>> {
        if line.len() > self.max_event_bytes {
            self.reset_event_buffers();
            return Err(Error::EventTooLarge {
                max: self.max_event_bytes,
            });
        }

        if line.is_empty() {
            return Ok(self.dispatch_event());
        }

        // Comment line (starts with ':').
        if line.starts_with(':') {
            return Ok(None);
        }

        let (field, value) = match line.split_once(':') {
            Some((name, rest)) => (name, rest.strip_prefix(' ').unwrap_or(rest)),
            None => (line, ""),
        };

        match field {
            "event" => {
                self.check_event_budget(value.len().saturating_sub(self.event_buf.len()))?;
                self.event_buf.clear();
                self.event_buf.push_str(value);
            }
            "data" => {
                self.check_event_budget(value.len().saturating_add(1))?;
                self.data_buf.push_str(value);
                self.data_buf.push('\n');
                self.has_data = true;
            }
            "id" => {
                // WHATWG HTML §9.2.6: ignore id field if value contains U+0000 NULL.
                if !value.contains('\0') {
                    self.check_event_budget(value.len().saturating_sub(self.id_buf.len()))?;
                    self.id_buf.clear();
                    self.id_buf.push_str(value);
                }
            }
            "retry" if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) => {
                if let Ok(ms) = value.parse::<u64>() {
                    let dur = Duration::from_millis(ms);
                    self.retry = Some(dur);
                    self.pending_retry = Some(dur);
                }
            }
            _ => {
                // Unknown fields are ignored per WHATWG HTML §9.2.6.
            }
        }

        Ok(None)
    }

    fn check_event_budget(&mut self, additional: usize) -> Result<()> {
        let current = self
            .data_buf
            .len()
            .saturating_add(self.event_buf.len())
            .saturating_add(self.id_buf.len());
        if current.saturating_add(additional) > self.max_event_bytes {
            self.reset_event_buffers();
            return Err(Error::EventTooLarge {
                max: self.max_event_bytes,
            });
        }
        Ok(())
    }

    fn dispatch_event(&mut self) -> Option<Event> {
        // WHATWG HTML §9.2.6 dispatch step 1: commit the last event ID buffer,
        // even when the data buffer is empty.
        self.last_event_id.clone_from(&self.id_buf);
        if !self.has_data {
            self.event_buf.clear();
            self.pending_retry = None;
            return None;
        }

        // Pop the single trailing U+000A appended after the last `data:` field.
        if self.data_buf.ends_with('\n') {
            self.data_buf.pop();
        }

        let data = std::mem::take(&mut self.data_buf);
        self.has_data = false;

        let event_name = std::mem::take(&mut self.event_buf);
        let event = if event_name.is_empty() {
            None
        } else {
            Some(event_name)
        };

        let id = if self.last_event_id.is_empty() {
            None
        } else {
            Some(self.last_event_id.clone())
        };

        let retry = self.pending_retry.take();

        Some(Event::from_parsed(event, data, id, retry))
    }

    fn reset_event_buffers(&mut self) {
        // The dropped event's `id:` must not leak into later events.
        self.id_buf.clone_from(&self.last_event_id);
        self.data_buf.clear();
        self.has_data = false;
        self.event_buf.clear();
        self.pending_retry = None;
    }
}

impl Default for Parser {
    fn default() -> Self {
        Self::new()
    }
}

/// Pull-based streaming Server-Sent Events reader over any `R: BufRead`.
#[derive(Debug)]
pub struct EventStream<R> {
    reader: R,
    parser: Parser,
    queue: VecDeque<Event>,
    eof: bool,
}

impl<R: BufRead> EventStream<R> {
    /// Creates an [`EventStream`] wrapping `reader` with [`DEFAULT_MAX_EVENT_BYTES`].
    pub fn new(reader: R) -> Self {
        Self::with_max_event_bytes(reader, DEFAULT_MAX_EVENT_BYTES)
    }

    /// Creates an [`EventStream`] wrapping `reader` with a custom `max_event_bytes` cap.
    pub fn with_max_event_bytes(reader: R, max_event_bytes: usize) -> Self {
        Self {
            reader,
            parser: Parser::with_max_event_bytes(max_event_bytes),
            queue: VecDeque::new(),
            eof: false,
        }
    }

    /// Reads from the underlying stream until the next [`Event`] is dispatched or EOF is reached.
    pub fn next_event(&mut self) -> Result<Option<Event>> {
        loop {
            if let Some(ev) = self.queue.pop_front() {
                return Ok(Some(ev));
            }
            if self.parser.has_pending() {
                let events = self.parser.push(&[])?;
                self.queue.extend(events);
                continue;
            }
            if self.eof {
                return Ok(None);
            }

            let available = self.reader.fill_buf()?;
            if available.is_empty() {
                self.parser.finish();
                self.eof = true;
                return Ok(None);
            }

            let n = available.len();
            let parsed = self.parser.push(available);
            self.reader.consume(n);

            let events = parsed?;
            self.queue.extend(events);
        }
    }

    /// Returns the most recently observed valid `Last-Event-ID`.
    pub fn last_event_id(&self) -> &str {
        self.parser.last_event_id()
    }

    /// Returns the most recently parsed `retry:` duration, if any.
    pub const fn retry(&self) -> Option<Duration> {
        self.parser.retry()
    }

    /// Consumes the [`EventStream`] and returns the underlying reader.
    pub fn into_inner(self) -> R {
        self.reader
    }
}

impl<R: BufRead> Iterator for EventStream<R> {
    type Item = Result<Event>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.next_event() {
            Ok(Some(event)) => Some(Ok(event)),
            Ok(None) => None,
            Err(err) => Some(Err(err)),
        }
    }
}
