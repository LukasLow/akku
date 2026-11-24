//! Error and result types for the TOML domain.

use std::fmt;
use std::io;

/// A location inside a TOML document.
///
/// `offset` is a byte offset, `line` and `column` are one based.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Position {
    /// The byte offset of the position.
    pub offset: usize,
    /// The one-based line number.
    pub line: u32,
    /// The one-based column number, counted in bytes.
    pub column: u32,
}

/// The kind of a TOML [`Error`].
#[non_exhaustive]
#[derive(Debug)]
pub enum ErrorKind {
    /// The document ended where more input was required.
    UnexpectedEof,
    /// A byte appeared where the grammar does not allow it.
    UnexpectedByte {
        /// The offending byte.
        byte: u8,
    },
    /// Something else was expected.
    Expected {
        /// A short description of what was expected.
        expected: &'static str,
        /// A short description of what was found.
        found: String,
    },
    /// An escape sequence in a string was not recognized.
    InvalidEscape {
        /// The character after the backslash.
        escape: char,
    },
    /// A `\u` or `\U` escape did not encode a valid scalar value.
    InvalidUnicodeEscape,
    /// A string contained a raw control character.
    ControlCharacterInString,
    /// A string was not closed before the end of the input.
    UnterminatedString,
    /// A number was malformed.
    InvalidNumber {
        /// Why the number was rejected.
        reason: &'static str,
    },
    /// An integer did not fit into an `i64`.
    IntegerOutOfRange,
    /// A date, time or date-time was malformed.
    InvalidDatetime {
        /// Why the value was rejected.
        reason: &'static str,
    },
    /// A key was empty or contained characters that are not allowed.
    InvalidKey,
    /// A key was defined twice in the same table.
    DuplicateKey {
        /// The duplicated key.
        key: String,
    },
    /// A table header appeared twice.
    DuplicateTable {
        /// The table name.
        name: String,
    },
    /// A table that cannot be reopened was defined again.
    TableRedefined {
        /// The table name.
        name: String,
    },
    /// An inline table was missing its closing brace.
    InlineTableNotClosed,
    /// An inline table cannot be extended or nested across a line break.
    NestedInlineTable,
    /// A dotted key tried to extend a value that is not a table.
    DottedKeyConflict {
        /// The conflicting key.
        key: String,
    },
    /// The nesting depth limit was exceeded.
    MaxDepthExceeded {
        /// The configured limit.
        limit: usize,
    },
    /// The entry limit was exceeded.
    MaxEntriesExceeded {
        /// The configured limit.
        limit: usize,
    },
    /// An error from the underlying writer.
    Io(io::Error),
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedEof => formatter.write_str("unexpected end of input"),
            Self::UnexpectedByte { byte } => write!(formatter, "unexpected byte {byte:#04x}"),
            Self::Expected { expected, found } => {
                write!(formatter, "expected {expected}, found {found}")
            }
            Self::InvalidEscape { escape } => write!(formatter, "invalid escape \\{escape}"),
            Self::InvalidUnicodeEscape => formatter.write_str("invalid unicode escape"),
            Self::ControlCharacterInString => formatter.write_str("control character in string"),
            Self::UnterminatedString => formatter.write_str("unterminated string"),
            Self::InvalidNumber { reason } => write!(formatter, "invalid number: {reason}"),
            Self::IntegerOutOfRange => formatter.write_str("integer out of range"),
            Self::InvalidDatetime { reason } => write!(formatter, "invalid date or time: {reason}"),
            Self::InvalidKey => formatter.write_str("invalid key"),
            Self::DuplicateKey { key } => write!(formatter, "duplicate key {key:?}"),
            Self::DuplicateTable { name } => write!(formatter, "duplicate table {name:?}"),
            Self::TableRedefined { name } => {
                write!(formatter, "table {name:?} cannot be redefined")
            }
            Self::InlineTableNotClosed => formatter.write_str("inline table is not closed"),
            Self::NestedInlineTable => {
                formatter.write_str("inline table cannot be extended or span lines")
            }
            Self::DottedKeyConflict { key } => {
                write!(
                    formatter,
                    "dotted key {key:?} conflicts with an existing value"
                )
            }
            Self::MaxDepthExceeded { limit } => {
                write!(formatter, "maximum nesting depth of {limit} exceeded")
            }
            Self::MaxEntriesExceeded { limit } => {
                write!(formatter, "maximum of {limit} entries exceeded")
            }
            Self::Io(error) => write!(formatter, "I/O error: {error}"),
        }
    }
}

/// A TOML error with the position it was detected at.
#[derive(Debug)]
pub struct Error {
    kind: ErrorKind,
    position: Position,
}

impl Error {
    /// Create an error from a kind and a position.
    pub(crate) fn new(kind: ErrorKind, position: Position) -> Self {
        Self { kind, position }
    }

    /// The kind of this error.
    pub fn kind(&self) -> &ErrorKind {
        &self.kind
    }

    /// Where the error was detected.
    pub fn position(&self) -> Position {
        self.position
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = &self.kind;
        let position = self.position;
        write!(
            formatter,
            "{kind} at line {} column {}",
            position.line, position.column
        )
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

impl From<io::Error> for Error {
    fn from(value: io::Error) -> Self {
        Self {
            kind: ErrorKind::Io(value),
            position: Position::default(),
        }
    }
}

/// Convenience alias for results in the TOML domain.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Compute the one-based line and column of a byte offset.
pub(crate) fn position_at(source: &[u8], offset: usize) -> Position {
    let end = offset.min(source.len());
    let mut line = 1u32;
    let mut column = 1u32;
    for &byte in &source[..end] {
        if byte == b'\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    Position {
        offset,
        line,
        column,
    }
}
