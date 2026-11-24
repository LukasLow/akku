//! Errors raised while building or reading a configuration.

use std::fmt;
use std::io;

use crate::config::Source;

/// What went wrong in the configuration domain.
///
/// The enum is `#[non_exhaustive]`: more precise variants may be added without
/// a breaking change. Match with a `_` arm.
#[non_exhaustive]
#[derive(Debug)]
pub enum ErrorKind {
    /// The requested key is not present.
    Missing {
        /// The dotted path that was requested.
        path: String,
    },
    /// The key is present but cannot be read as the requested type.
    WrongType {
        /// The dotted path that was requested.
        path: String,
        /// The type name that was requested, for example `"int"`.
        expected: &'static str,
        /// The [`ConfigValue::type_name`](crate::config::ConfigValue::type_name) that was found.
        found: &'static str,
    },
    /// A layer could not be converted into the configuration model.
    Parse {
        /// The dotted path that failed, empty for the whole document.
        path: String,
        /// A human-readable explanation.
        reason: String,
    },
    /// A layer's own parser rejected the input.
    Format {
        /// The layer that failed to parse.
        source: Source,
        /// The one-based line of the offending input, if the parser reported one.
        line: Option<usize>,
        /// The underlying parser message.
        reason: String,
    },
    /// Reading a configuration file failed.
    Io(io::Error),
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing { path } => {
                write!(formatter, "missing configuration key \"{path}\"")
            }
            Self::WrongType {
                path,
                expected,
                found,
            } => write!(
                formatter,
                "configuration key \"{path}\" expects {expected}, found {found}"
            ),
            Self::Parse { path, reason } if path.is_empty() => {
                write!(formatter, "invalid configuration value: {reason}")
            }
            Self::Parse { path, reason } => {
                write!(
                    formatter,
                    "invalid configuration value at \"{path}\": {reason}"
                )
            }
            Self::Format {
                source,
                line: Some(line),
                reason,
            } => write!(
                formatter,
                "failed to parse {source} at line {line}: {reason}"
            ),
            Self::Format {
                source,
                line: None,
                reason,
            } => write!(formatter, "failed to parse {source}: {reason}"),
            Self::Io(error) => write!(formatter, "configuration I/O error: {error}"),
        }
    }
}

/// An error from the configuration domain.
#[derive(Debug)]
pub struct Error {
    kind: ErrorKind,
}

impl Error {
    /// Create an error from its kind.
    pub(crate) fn new(kind: ErrorKind) -> Self {
        Self { kind }
    }

    /// What went wrong.
    pub fn kind(&self) -> &ErrorKind {
        &self.kind
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.kind, formatter)
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
    fn from(error: io::Error) -> Self {
        Self::new(ErrorKind::Io(error))
    }
}

/// The result type of the configuration domain.
pub type Result<T, E = Error> = std::result::Result<T, E>;
