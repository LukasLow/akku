//! Structured, level-filtered logging with human and JSON output formats,
//! atomic line writes, in-memory test capture, and namespace-clean macros.
//!
//! # Examples
//!
//! ```
//! use akku::log::{Format, Level, Logger};
//!
//! let (logger, buf) = Logger::memory(Level::Info, Format::Json);
//! akku::log::debug!(logger, "filtered out"; expensive = 1);
//! akku::log::info!(logger, "server started on port {}", 8080; env = "prod", port = 8080);
//!
//! let out = buf.to_string_lossy();
//! assert!(!out.contains("filtered out"));
//! assert!(out.contains("server started on port 8080"));
//! assert!(out.contains("\"port\":8080"));
//! ```

use std::error::Error;
use std::fmt::{self, Write as FmtWrite};
use std::io::{self, Write};
use std::str::FromStr;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use crate::term::{resolve_color, Color, ColorMode, Stream, Style};
use crate::time::OffsetDateTime;

/// Log severity level, ordered from most verbose ([`Level::Trace`]) to most severe ([`Level::Error`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum Level {
    /// Fine-grained diagnostic tracing.
    Trace = 0,
    /// Debugging information.
    Debug = 1,
    /// Informational lifecycle events.
    Info = 2,
    /// Recoverable warnings.
    Warn = 3,
    /// Failures requiring attention.
    Error = 4,
}

impl Level {
    /// Returns the uppercase canonical name of this level (`"TRACE"`..`"ERROR"`).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Trace => "TRACE",
            Self::Debug => "DEBUG",
            Self::Info => "INFO",
            Self::Warn => "WARN",
            Self::Error => "ERROR",
        }
    }

    const fn from_u8(raw: u8) -> Self {
        match raw {
            0 => Self::Trace,
            1 => Self::Debug,
            2 => Self::Info,
            3 => Self::Warn,
            _ => Self::Error,
        }
    }
}

impl fmt::Display for Level {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Error returned when parsing a [`Level`] from a string fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseLevelError(String);

impl fmt::Display for ParseLevelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown log level: {:?}", self.0)
    }
}

impl Error for ParseLevelError {}

impl FromStr for Level {
    type Err = ParseLevelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "trace" => Ok(Self::Trace),
            "debug" => Ok(Self::Debug),
            "info" => Ok(Self::Info),
            "warn" | "warning" => Ok(Self::Warn),
            "error" => Ok(Self::Error),
            _ => Err(ParseLevelError(s.to_string())),
        }
    }
}

/// A typed structured log field value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Value<'a> {
    /// Borrowed UTF-8 string.
    Str(&'a str),
    /// Signed 64-bit integer.
    Int(i64),
    /// Unsigned 64-bit integer.
    Uint(u64),
    /// 64-bit floating-point number.
    Float(f64),
    /// Boolean flag.
    Bool(bool),
    /// Null / absent value.
    Null,
}

impl<'a> From<&'a str> for Value<'a> {
    fn from(v: &'a str) -> Self {
        Self::Str(v)
    }
}

impl<'a> From<&'a String> for Value<'a> {
    fn from(v: &'a String) -> Self {
        Self::Str(v.as_str())
    }
}

impl From<i8> for Value<'_> {
    fn from(v: i8) -> Self {
        Self::Int(i64::from(v))
    }
}

impl From<i16> for Value<'_> {
    fn from(v: i16) -> Self {
        Self::Int(i64::from(v))
    }
}

impl From<i32> for Value<'_> {
    fn from(v: i32) -> Self {
        Self::Int(i64::from(v))
    }
}

impl From<i64> for Value<'_> {
    fn from(v: i64) -> Self {
        Self::Int(v)
    }
}

impl From<isize> for Value<'_> {
    fn from(v: isize) -> Self {
        Self::Int(v as i64)
    }
}

impl From<u8> for Value<'_> {
    fn from(v: u8) -> Self {
        Self::Uint(u64::from(v))
    }
}

impl From<u16> for Value<'_> {
    fn from(v: u16) -> Self {
        Self::Uint(u64::from(v))
    }
}

impl From<u32> for Value<'_> {
    fn from(v: u32) -> Self {
        Self::Uint(u64::from(v))
    }
}

impl From<u64> for Value<'_> {
    fn from(v: u64) -> Self {
        Self::Uint(v)
    }
}

impl From<usize> for Value<'_> {
    fn from(v: usize) -> Self {
        Self::Uint(v as u64)
    }
}

impl From<f32> for Value<'_> {
    fn from(v: f32) -> Self {
        Self::Float(f64::from(v))
    }
}

impl From<f64> for Value<'_> {
    fn from(v: f64) -> Self {
        Self::Float(v)
    }
}

impl From<bool> for Value<'_> {
    fn from(v: bool) -> Self {
        Self::Bool(v)
    }
}

impl<'a, T: Into<Value<'a>>> From<Option<T>> for Value<'a> {
    fn from(v: Option<T>) -> Self {
        match v {
            Some(inner) => inner.into(),
            None => Self::Null,
        }
    }
}

/// A single structured `key = value` log attribute.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Field<'a> {
    key: &'a str,
    value: Value<'a>,
}

impl<'a> Field<'a> {
    /// Creates a new `Field` from a key and any value convertible into [`Value`].
    pub fn new(key: &'a str, value: impl Into<Value<'a>>) -> Self {
        Self {
            key,
            value: value.into(),
        }
    }

    /// Returns the field's key.
    pub const fn key(&self) -> &'a str {
        self.key
    }

    /// Returns a reference to the field's [`Value`].
    pub const fn value(&self) -> &Value<'a> {
        &self.value
    }
}

enum MessageRef<'a> {
    Str(&'a str),
    Args(fmt::Arguments<'a>),
}

enum FieldsStorage<'a> {
    Borrowed(&'a [Field<'a>]),
    Owned(Vec<Field<'a>>),
}

/// A single structured log record passed to [`Logger::log`].
pub struct Record<'a> {
    level: Level,
    target: &'a str,
    message: MessageRef<'a>,
    timestamp: Option<OffsetDateTime>,
    fields: FieldsStorage<'a>,
}

impl<'a> Record<'a> {
    /// Creates a new `Record` with a plain string message.
    pub const fn new(level: Level, target: &'a str, message: &'a str) -> Self {
        Self {
            level,
            target,
            message: MessageRef::Str(message),
            timestamp: None,
            fields: FieldsStorage::Borrowed(&[]),
        }
    }

    /// Creates a new `Record` from lazily formatted [`fmt::Arguments`].
    pub const fn with_args(level: Level, target: &'a str, args: fmt::Arguments<'a>) -> Self {
        Self {
            level,
            target,
            message: MessageRef::Args(args),
            timestamp: None,
            fields: FieldsStorage::Borrowed(&[]),
        }
    }

    /// Attaches an explicit UTC/offset timestamp to this record.
    pub const fn timestamp(mut self, ts: OffsetDateTime) -> Self {
        self.timestamp = Some(ts);
        self
    }

    /// Appends a single structured field to this record.
    pub fn field(mut self, key: &'a str, value: impl Into<Value<'a>>) -> Self {
        let f = Field::new(key, value);
        match &mut self.fields {
            FieldsStorage::Borrowed(slice) => {
                let mut vec = Vec::with_capacity(slice.len() + 1);
                vec.extend_from_slice(slice);
                vec.push(f);
                self.fields = FieldsStorage::Owned(vec);
            }
            FieldsStorage::Owned(vec) => vec.push(f),
        }
        self
    }

    /// Sets the record's structured fields from a borrowed slice (zero allocation).
    pub fn fields(mut self, fields: &'a [Field<'a>]) -> Self {
        self.fields = FieldsStorage::Borrowed(fields);
        self
    }

    /// Returns this record's [`Level`].
    pub const fn level(&self) -> Level {
        self.level
    }

    /// Returns this record's target (typically a module path).
    pub const fn target(&self) -> &'a str {
        self.target
    }

    /// Returns the explicit timestamp attached to this record, if any.
    pub const fn get_timestamp(&self) -> Option<OffsetDateTime> {
        self.timestamp
    }

    /// Returns all structured fields attached to this record.
    pub fn fields_slice(&self) -> &[Field<'a>] {
        match &self.fields {
            FieldsStorage::Borrowed(s) => s,
            FieldsStorage::Owned(v) => v.as_slice(),
        }
    }

    fn write_message(&self, out: &mut String) {
        match self.message {
            MessageRef::Str(s) => out.push_str(s),
            MessageRef::Args(args) => {
                let _ = out.write_fmt(args);
            }
        }
    }
}

/// Output wire format for [`Logger`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Format {
    /// Human-readable single-line log format with optional ANSI colors.
    #[default]
    Human,
    /// Structured newline-delimited JSON (NDJSON) format.
    Json,
}

/// Thread-safe in-memory byte buffer implementing [`io::Write`] for capturing log output in tests.
#[derive(Debug, Clone, Default)]
pub struct Buffer {
    inner: Arc<Mutex<Vec<u8>>>,
}

impl Buffer {
    /// Creates an empty `Buffer`.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Returns the buffered bytes decoded lossily as UTF-8.
    pub fn to_string_lossy(&self) -> String {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        String::from_utf8_lossy(&guard).into_owned()
    }

    /// Returns a cloned snapshot of the raw buffered bytes.
    pub fn bytes(&self) -> Vec<u8> {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.clone()
    }

    /// Clears all buffered bytes.
    pub fn clear(&self) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.clear();
    }
}

impl Write for Buffer {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Builder for configuring and constructing a [`Logger`].
pub struct LoggerBuilder {
    level: Level,
    format: Format,
    color: ColorMode,
    writer: Option<Box<dyn Write + Send>>,
    writer_is_stderr: bool,
}

impl Default for LoggerBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl LoggerBuilder {
    /// Creates a `LoggerBuilder` defaulting to `Level::Info`, `Format::Human`,
    /// `ColorMode::Auto`, writing to `stderr`.
    pub fn new() -> Self {
        Self {
            level: Level::Info,
            format: Format::Human,
            color: ColorMode::Auto,
            writer: None,
            writer_is_stderr: true,
        }
    }

    /// Sets the initial minimum log [`Level`].
    pub fn level(mut self, level: Level) -> Self {
        self.level = level;
        self
    }

    /// Sets the output [`Format`] (`Human` or `Json`).
    pub fn format(mut self, format: Format) -> Self {
        self.format = format;
        self
    }

    /// Sets the ANSI [`ColorMode`] policy (applies to [`Format::Human`]).
    pub fn color(mut self, color: ColorMode) -> Self {
        self.color = color;
        self
    }

    /// Sets a custom destination writer (replacing the default `stderr` sink).
    pub fn writer<W: Write + Send + 'static>(mut self, writer: W) -> Self {
        self.writer = Some(Box::new(writer));
        self.writer_is_stderr = false;
        self
    }

    /// Builds the configured [`Logger`].
    pub fn build(self) -> Logger {
        let use_color = match self.format {
            Format::Json => false,
            Format::Human => {
                let is_tty = if self.writer_is_stderr {
                    crate::term::is_tty(Stream::Stderr)
                } else {
                    false
                };
                let no_color = std::env::var_os("NO_COLOR");
                let term = std::env::var("TERM").ok();
                resolve_color(self.color, is_tty, no_color.as_deref(), term.as_deref())
            }
        };
        let writer: Box<dyn Write + Send> = self.writer.unwrap_or_else(|| Box::new(io::stderr()));
        Logger {
            min_level: AtomicU8::new(self.level as u8),
            format: self.format,
            use_color,
            writer: Mutex::new(writer),
        }
    }
}

/// Thread-safe structured logger with lock-free level filtering and atomic line output.
pub struct Logger {
    min_level: AtomicU8,
    format: Format,
    use_color: bool,
    writer: Mutex<Box<dyn Write + Send>>,
}

impl Logger {
    /// Returns a new [`LoggerBuilder`].
    pub fn builder() -> LoggerBuilder {
        LoggerBuilder::new()
    }

    /// Creates a human-formatted logger writing to `stderr` at `level`.
    pub fn stderr(level: Level) -> Self {
        Self::builder().level(level).build()
    }

    /// Creates an in-memory logger paired with a [`Buffer`] for deterministic tests.
    pub fn memory(level: Level, format: Format) -> (Self, Buffer) {
        let buf = Buffer::new();
        let logger = Self::builder()
            .level(level)
            .format(format)
            .color(ColorMode::Never)
            .writer(buf.clone())
            .build();
        (logger, buf)
    }

    /// Returns the current minimum enabled [`Level`].
    pub fn min_level(&self) -> Level {
        Level::from_u8(self.min_level.load(Ordering::Relaxed))
    }

    /// Dynamically updates the minimum enabled [`Level`].
    pub fn set_level(&self, level: Level) {
        self.min_level.store(level as u8, Ordering::Relaxed);
    }

    /// Returns `true` if records at `level` will be emitted.
    pub fn enabled(&self, level: Level) -> bool {
        (level as u8) >= self.min_level.load(Ordering::Relaxed)
    }

    /// Formats and writes `record` atomically if its level is enabled.
    pub fn log(&self, record: &Record<'_>) {
        if !self.enabled(record.level()) {
            return;
        }
        let ts = record
            .get_timestamp()
            .unwrap_or_else(OffsetDateTime::now_utc);

        // Format complete line BEFORE acquiring the writer lock so concurrent
        // threads never interleave partial lines.
        let mut line = String::with_capacity(128);
        match self.format {
            Format::Human => format_human(&mut line, record, ts, self.use_color),
            Format::Json => format_json(&mut line, record, ts),
        }
        line.push('\n');

        let mut writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        let _ = writer.write_all(line.as_bytes());
    }

    /// Flushes the underlying writer.
    pub fn flush(&self) {
        let mut writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        let _ = writer.flush();
    }
}

fn format_human(out: &mut String, record: &Record<'_>, ts: OffsetDateTime, use_color: bool) {
    let _ = write!(out, "{ts} ");
    let level_str = record.level().as_str();
    if use_color {
        let style = match record.level() {
            Level::Trace => Style::new().dim(),
            Level::Debug => Style::new().cyan(),
            Level::Info => Style::new().green(),
            Level::Warn => Style::new().yellow().bold(),
            Level::Error => Style::new().fg(Color::Red).bold(),
        };
        let _ = write!(out, "{}", style.paint(level_str));
    } else {
        out.push_str(level_str);
    }

    if !record.target().is_empty() {
        out.push(' ');
        out.push_str(record.target());
        out.push(':');
    }
    out.push(' ');
    record.write_message(out);

    for field in record.fields_slice() {
        out.push(' ');
        out.push_str(field.key());
        out.push('=');
        match field.value() {
            Value::Str(s) => {
                if s.is_empty() || s.bytes().any(|b| b.is_ascii_whitespace() || b == b'"') {
                    push_json_string(out, s);
                } else {
                    out.push_str(s);
                }
            }
            Value::Int(n) => {
                let _ = write!(out, "{n}");
            }
            Value::Uint(n) => {
                let _ = write!(out, "{n}");
            }
            Value::Float(v) => {
                let _ = write!(out, "{v}");
            }
            Value::Bool(b) => {
                let _ = write!(out, "{b}");
            }
            Value::Null => out.push_str("null"),
        }
    }
}

fn format_json(out: &mut String, record: &Record<'_>, ts: OffsetDateTime) {
    let mut msg = String::new();
    record.write_message(&mut msg);

    out.push_str("{\"timestamp\":");
    push_json_string(out, &ts.format_rfc3339());
    out.push_str(",\"level\":");
    push_json_string(out, record.level().as_str());
    out.push_str(",\"target\":");
    push_json_string(out, record.target());
    out.push_str(",\"message\":");
    push_json_string(out, &msg);

    let fields = record.fields_slice();
    if !fields.is_empty() {
        out.push_str(",\"fields\":{");
        for (idx, field) in fields.iter().enumerate() {
            if idx > 0 {
                out.push(',');
            }
            push_json_string(out, field.key());
            out.push(':');
            match field.value() {
                Value::Str(s) => push_json_string(out, s),
                Value::Int(n) => {
                    let _ = write!(out, "{n}");
                }
                Value::Uint(n) => {
                    let _ = write!(out, "{n}");
                }
                Value::Float(v) => {
                    if v.is_finite() {
                        let _ = write!(out, "{v}");
                    } else {
                        out.push_str("null");
                    }
                }
                Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
                Value::Null => out.push_str("null"),
            }
        }
        out.push('}');
    }
    out.push('}');
}

fn push_json_string(out: &mut String, s: &str) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

// ---------------------------------------------------------------------------
// Global logger
// ---------------------------------------------------------------------------

static GLOBAL_LOGGER: OnceLock<Logger> = OnceLock::new();

/// Error returned by [`init_global`] if a global logger was already installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InitError;

impl fmt::Display for InitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("global logger has already been initialized")
    }
}

impl Error for InitError {}

/// Installs `logger` as the process-wide global logger.
///
/// Returns `Err(`[`InitError`]`)` if called more than once.
pub fn init_global(logger: Logger) -> Result<(), InitError> {
    GLOBAL_LOGGER.set(logger).map_err(|_| InitError)
}

/// Returns a reference to the global [`Logger`], if initialized.
pub fn global() -> Option<&'static Logger> {
    GLOBAL_LOGGER.get()
}

/// Emits `record` to the global [`Logger`], if one has been initialized.
pub fn log(record: &Record<'_>) {
    if let Some(logger) = global() {
        logger.log(record);
    }
}

// ---------------------------------------------------------------------------
// Namespace-clean logging macros
// ---------------------------------------------------------------------------

#[doc(hidden)]
#[macro_export]
macro_rules! __akku_log_emit {
    ($level:expr, $fmt:literal $(, $arg:expr)* ; $($key:ident = $val:expr),+ $(,)?) => {{
        if let ::core::option::Option::Some(__akku_logger) = $crate::log::global() {
            if __akku_logger.enabled($level) {
                __akku_logger.log(
                    &$crate::log::Record::with_args(
                        $level,
                        ::core::module_path!(),
                        ::core::format_args!($fmt $(, $arg)*),
                    )
                    .fields(&[
                        $($crate::log::Field::new(::core::stringify!($key), $val)),+
                    ]),
                );
            }
        }
    }};
    ($level:expr, $fmt:literal $(, $arg:expr)* $(,)?) => {{
        if let ::core::option::Option::Some(__akku_logger) = $crate::log::global() {
            if __akku_logger.enabled($level) {
                __akku_logger.log(
                    &$crate::log::Record::with_args(
                        $level,
                        ::core::module_path!(),
                        ::core::format_args!($fmt $(, $arg)*),
                    ),
                );
            }
        }
    }};
    ($level:expr, $logger:expr, $fmt:literal $(, $arg:expr)* ; $($key:ident = $val:expr),+ $(,)?) => {{
        let __akku_logger = &$logger;
        if __akku_logger.enabled($level) {
            __akku_logger.log(
                &$crate::log::Record::with_args(
                    $level,
                    ::core::module_path!(),
                    ::core::format_args!($fmt $(, $arg)*),
                )
                .fields(&[
                    $($crate::log::Field::new(::core::stringify!($key), $val)),+
                ]),
            );
        }
    }};
    ($level:expr, $logger:expr, $fmt:literal $(, $arg:expr)* $(,)?) => {{
        let __akku_logger = &$logger;
        if __akku_logger.enabled($level) {
            __akku_logger.log(
                &$crate::log::Record::with_args(
                    $level,
                    ::core::module_path!(),
                    ::core::format_args!($fmt $(, $arg)*),
                ),
            );
        }
    }};
}

#[doc(hidden)]
#[macro_export]
macro_rules! __akku_log_trace {
    ($($tt:tt)*) => {
        $crate::__akku_log_emit!($crate::log::Level::Trace, $($tt)*)
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __akku_log_debug {
    ($($tt:tt)*) => {
        $crate::__akku_log_emit!($crate::log::Level::Debug, $($tt)*)
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __akku_log_info {
    ($($tt:tt)*) => {
        $crate::__akku_log_emit!($crate::log::Level::Info, $($tt)*)
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __akku_log_warn {
    ($($tt:tt)*) => {
        $crate::__akku_log_emit!($crate::log::Level::Warn, $($tt)*)
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __akku_log_error {
    ($($tt:tt)*) => {
        $crate::__akku_log_emit!($crate::log::Level::Error, $($tt)*)
    };
}

/// Logs a structured message at [`Level::Trace`] to either the global logger or an explicit [`Logger`].
pub use crate::__akku_log_trace as trace;

/// Logs a structured message at [`Level::Debug`] to either the global logger or an explicit [`Logger`].
pub use crate::__akku_log_debug as debug;

/// Logs a structured message at [`Level::Info`] to either the global logger or an explicit [`Logger`].
pub use crate::__akku_log_info as info;

/// Logs a structured message at [`Level::Warn`] to either the global logger or an explicit [`Logger`].
pub use crate::__akku_log_warn as warn;

/// Logs a structured message at [`Level::Error`] to either the global logger or an explicit [`Logger`].
pub use crate::__akku_log_error as error;
