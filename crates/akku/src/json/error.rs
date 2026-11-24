//! Errors and positions for the JSON domain.
//!
//! Every fallible JSON operation returns [`Result`] with an [`Error`] that
//! carries an [`ErrorKind`], and — where the information exists — the exact
//! [`Position`] in the input and the path to the offending value.

use std::fmt;

/// A position inside the parsed input.
///
/// `offset` is a zero-based byte offset from the start of the input, while
/// `line` and `column` are one-based. The column counts characters, not bytes,
/// so a multi-byte character advances it by exactly one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Position {
    /// Zero-based byte offset from the start of the input.
    pub offset: usize,
    /// One-based line number.
    pub line: u32,
    /// One-based column number, counted in characters.
    pub column: u32,
}

/// What went wrong while parsing, converting or writing JSON.
///
/// The enum is `#[non_exhaustive]`: more precise variants may be added without
/// a breaking change. Match with a `_` arm.
#[non_exhaustive]
#[derive(Debug)]
pub enum ErrorKind {
    /// The input ended in the middle of a value.
    UnexpectedEof,
    /// A byte appeared where the grammar does not allow it.
    UnexpectedByte {
        /// The offending byte.
        byte: u8,
    },
    /// A specific token was required at this point.
    Expected {
        /// What the parser wanted, for example `"a value"`.
        expected: &'static str,
        /// What it found, for example `"']'"`.
        found: &'static str,
    },
    /// A raw control character (below `U+0020`) appeared inside a string.
    ///
    /// Such characters must be escaped.
    ControlCharacterInString,
    /// A backslash escape is not part of JSON.
    InvalidEscape {
        /// The character that followed the backslash.
        escape: char,
    },
    /// A `\u` escape was not followed by four hexadecimal digits.
    InvalidUnicodeEscape,
    /// A UTF-16 surrogate appeared without its partner.
    LoneSurrogate,
    /// A number literal does not follow the JSON number grammar.
    InvalidNumber {
        /// A short explanation, for example `"leading zero"`.
        reason: &'static str,
    },
    /// A number is outside the range representable as `f64`.
    NumberOutOfRange,
    /// The same key appeared twice in one object.
    DuplicateKey {
        /// The repeated key.
        key: String,
    },
    /// A comma appeared after the last element of an array or object.
    TrailingComma,
    /// The configured nesting limit was exceeded.
    MaxDepthExceeded {
        /// The configured limit.
        limit: usize,
    },
    /// Content followed the top-level value.
    TrailingCharacters,
    /// A decoded string exceeded the configured length limit.
    StringTooLong {
        /// The configured limit in bytes of the decoded string.
        limit: usize,
    },
    /// Byte input was not valid UTF-8.
    InvalidUtf8,
    /// A conversion found a different JSON type than expected.
    TypeMismatch {
        /// The expected JSON type name.
        expected: &'static str,
        /// The actual JSON type name.
        found: &'static str,
    },
    /// Reading or writing failed.
    Io(std::io::Error),
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedEof => formatter.write_str("unexpected end of input"),
            Self::UnexpectedByte { byte } => write!(formatter, "unexpected byte 0x{byte:02x}"),
            Self::Expected { expected, found } => {
                write!(formatter, "expected {expected}, found {found}")
            }
            Self::ControlCharacterInString => {
                formatter.write_str("control character in string; escape it")
            }
            Self::InvalidEscape { escape } => {
                write!(formatter, "invalid escape sequence '\\{escape}'")
            }
            Self::InvalidUnicodeEscape => {
                formatter.write_str("invalid unicode escape; expected four hex digits")
            }
            Self::LoneSurrogate => formatter.write_str("lone UTF-16 surrogate in string"),
            Self::InvalidNumber { reason } => write!(formatter, "invalid number: {reason}"),
            Self::NumberOutOfRange => formatter.write_str("number out of range"),
            Self::DuplicateKey { key } => write!(formatter, "duplicate key \"{key}\""),
            Self::TrailingComma => formatter.write_str("trailing comma"),
            Self::MaxDepthExceeded { limit } => {
                write!(formatter, "maximum nesting depth of {limit} exceeded")
            }
            Self::TrailingCharacters => {
                formatter.write_str("trailing characters after the top-level value")
            }
            Self::StringTooLong { limit } => {
                write!(formatter, "string longer than the limit of {limit} bytes")
            }
            Self::InvalidUtf8 => formatter.write_str("input is not valid UTF-8"),
            Self::TypeMismatch { expected, found } => {
                write!(formatter, "expected {expected}, found {found}")
            }
            Self::Io(error) => fmt::Display::fmt(error, formatter),
        }
    }
}

/// An error from the JSON domain.
///
/// Besides the [`ErrorKind`], a parser error carries the [`Position`] of the
/// offending byte and, when inside a container, the path of the value it was
/// parsing (for example `a.b[1].c`).
#[derive(Debug)]
pub struct Error {
    kind: ErrorKind,
    position: Option<Position>,
    path: Option<String>,
}

impl Error {
    /// Create an error from a kind, without position or path.
    pub(crate) fn new(kind: ErrorKind) -> Self {
        Self {
            kind,
            position: None,
            path: None,
        }
    }

    /// Attach the input position at which the error occurred.
    pub(crate) fn at(mut self, position: Position) -> Self {
        self.position = Some(position);
        self
    }

    /// Attach the path to the value that failed to parse.
    pub(crate) fn with_path(mut self, path: Option<String>) -> Self {
        self.path = path;
        self
    }

    /// What went wrong.
    pub fn kind(&self) -> &ErrorKind {
        &self.kind
    }

    /// Where in the input it went wrong, if the error has a position.
    pub fn position(&self) -> Option<Position> {
        self.position
    }

    /// The path to the offending value, if it was nested.
    ///
    /// Segments are joined with dots, array elements appear as `[index]`.
    pub fn path(&self) -> Option<&str> {
        self.path.as_deref()
    }

    /// Drop position and path and return just the kind.
    pub fn into_kind(self) -> ErrorKind {
        self.kind
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.kind, formatter)?;
        if let Some(position) = self.position {
            write!(
                formatter,
                " at line {} column {}",
                position.line, position.column
            )?;
        }
        if let Some(path) = &self.path {
            write!(formatter, " (at \"{path}\")")?;
        }
        Ok(())
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.kind {
            ErrorKind::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::new(ErrorKind::Io(error))
    }
}

impl From<Error> for std::io::Error {
    fn from(error: Error) -> Self {
        match error.into_kind() {
            ErrorKind::Io(error) => error,
            other => std::io::Error::new(std::io::ErrorKind::InvalidData, other.to_string()),
        }
    }
}

/// The result type of the JSON domain.
pub type Result<T, E = Error> = std::result::Result<T, E>;
