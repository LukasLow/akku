//! Error and result types for the CSV domain.

use std::fmt;
use std::io;

/// The kind of a CSV [`Error`].
#[non_exhaustive]
#[derive(Debug)]
pub enum ErrorKind {
    /// An error from the underlying reader or writer.
    Io(io::Error),
    /// A record has a different number of fields than the first record.
    UnequalLengths {
        /// The number of fields the reader expects.
        expected: usize,
        /// The number of fields the record actually has.
        got: usize,
    },
    /// A quoted field was not closed before the end of the input.
    UnterminatedQuote,
    /// A field was not valid UTF-8.
    InvalidUtf8,
    /// The configured delimiter or quote byte is not usable.
    InvalidDelimiter {
        /// The rejected byte.
        byte: u8,
    },
    /// Content followed a closing quote that was neither the field delimiter
    /// nor a line break.
    UnexpectedAfterQuote {
        /// The first unexpected byte after the closing quote.
        byte: u8,
    },
    /// A field could not be parsed into the requested type.
    ParseField {
        /// The name of the type that was expected.
        expected: &'static str,
        /// The field value that failed to parse.
        field: String,
    },
    /// A record does not have a field at the requested index.
    MissingField {
        /// The requested field index.
        index: usize,
    },
    /// A record has no value for the requested column name.
    MissingColumn {
        /// The requested column name.
        name: String,
    },
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "I/O error: {error}"),
            Self::UnequalLengths { expected, got } => {
                write!(formatter, "expected {expected} fields, found {got}")
            }
            Self::UnterminatedQuote => formatter.write_str("unterminated quoted field"),
            Self::InvalidUtf8 => formatter.write_str("field is not valid UTF-8"),
            Self::InvalidDelimiter { byte } => {
                write!(
                    formatter,
                    "byte {byte:#04x} cannot be used as delimiter or quote"
                )
            }
            Self::UnexpectedAfterQuote { byte } => {
                write!(
                    formatter,
                    "unexpected content {byte:#04x} after a closing quote"
                )
            }
            Self::ParseField { expected, field } => {
                write!(formatter, "cannot parse {field:?} as {expected}")
            }
            Self::MissingField { index } => write!(formatter, "record has no field {index}"),
            Self::MissingColumn { name } => write!(formatter, "record has no column {name:?}"),
        }
    }
}

/// A CSV error with the line it was detected on.
///
/// The line is `0` when the error did not originate from parsing input, for
/// example when a [`super::Record`] value is converted to another type.
#[derive(Debug)]
pub struct Error {
    kind: ErrorKind,
    line: u64,
}

impl Error {
    /// Create an error with an explicit kind and line.
    pub(crate) fn new(kind: ErrorKind, line: u64) -> Self {
        Self { kind, line }
    }

    /// The kind of this error.
    pub fn kind(&self) -> &ErrorKind {
        &self.kind
    }

    /// The one-based input line the error was detected on, or `0` if unknown.
    pub fn line(&self) -> u64 {
        self.line
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = &self.kind;
        write!(formatter, "{kind}")?;
        let line = self.line;
        if line > 0 {
            write!(formatter, " at line {line}")?;
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

impl From<io::Error> for Error {
    fn from(value: io::Error) -> Self {
        Self {
            kind: ErrorKind::Io(value),
            line: 0,
        }
    }
}

/// Convenience alias for results in the CSV domain.
pub type Result<T, E = Error> = std::result::Result<T, E>;
