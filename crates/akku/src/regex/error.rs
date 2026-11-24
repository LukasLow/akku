//! Errors for the regex domain.
//!
//! Compiling a pattern returns [`Result`] with an [`Error`] that carries an
//! [`ErrorKind`], the original pattern and — for syntax errors — the exact
//! byte [`Span`] of the offending part of the pattern.

use std::fmt;

/// `Result` specialised to regex [`Error`]s.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// A half-open byte range `start..end` inside the pattern text.
///
/// Both offsets always lie on `char` boundaries of the pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Span {
    /// Zero-based byte offset of the first byte.
    pub start: usize,
    /// Zero-based byte offset one past the last byte.
    pub end: usize,
}

/// What went wrong while compiling a pattern.
///
/// The enum is `#[non_exhaustive]`: new variants may be added without a
/// breaking change. Match with a `_` arm.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorKind {
    /// A `(` was never closed.
    UnclosedGroup,
    /// A `)` has no matching `(`.
    UnopenedGroup,
    /// A `[` was never closed.
    UnclosedClass,
    /// A class range whose start is greater than its end, such as `[z-a]`.
    InvalidClassRange {
        /// Range start.
        start: char,
        /// Range end.
        end: char,
    },
    /// A repetition operator with nothing to repeat, such as `*a` or `(+)`.
    RepetitionMissing,
    /// Two repetition operators in a row, such as `a**` or `a{2}*`.
    RepetitionStacked,
    /// A `{` that does not start a valid `{m}`, `{m,}` or `{m,n}`.
    ///
    /// Write a literal brace as `\{`.
    RepetitionCountUnclosed,
    /// A counted repetition whose minimum exceeds its maximum.
    RepetitionCountInvalid {
        /// The minimum.
        min: u32,
        /// The maximum.
        max: u32,
    },
    /// A counted repetition bound exceeds the hard limit.
    RepetitionCountTooLarge {
        /// The largest permitted bound.
        limit: u32,
    },
    /// A backslash escape that has no meaning.
    InvalidEscape {
        /// The character after the backslash.
        c: char,
    },
    /// The pattern ends with a lone backslash.
    TrailingBackslash,
    /// A malformed `\x` or `\u` escape.
    InvalidHexEscape,
    /// A `\x`/`\u` escape that names a surrogate or a value above `U+10FFFF`.
    InvalidCodepoint {
        /// The numeric value written in the escape.
        value: u32,
    },
    /// A group name that is empty, unterminated or not `[A-Za-z_][A-Za-z0-9_]*`.
    InvalidGroupName,
    /// The same group name was used twice.
    DuplicateGroupName {
        /// The repeated name.
        name: String,
    },
    /// An unknown inline flag.
    UnknownFlag {
        /// The flag character.
        flag: char,
    },
    /// A malformed flag group such as `(?)`, `(?i-)`, `(?--i)` or `(?ii)`.
    InvalidFlags,
    /// Valid syntax in other engines that this engine deliberately rejects
    /// (backreferences, look-around, possessive quantifiers, ...).
    Unsupported {
        /// A short name of the feature.
        feature: &'static str,
    },
    /// Groups or repetitions are nested deeper than the configured limit.
    NestLimitExceeded {
        /// The configured nesting limit.
        limit: u32,
    },
    /// The compiled program would exceed the configured size limit.
    SizeLimitExceeded {
        /// The configured limit in bytes.
        limit: usize,
    },
    /// The pattern text is longer than the configured limit.
    PatternTooLong {
        /// The configured limit in bytes.
        limit: usize,
    },
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ErrorKind::UnclosedGroup => f.write_str("unclosed group"),
            ErrorKind::UnopenedGroup => f.write_str("unopened group: `)` without matching `(`"),
            ErrorKind::UnclosedClass => f.write_str("unclosed character class"),
            ErrorKind::InvalidClassRange { start, end } => {
                write!(
                    f,
                    "invalid class range {start:?}-{end:?}: start is greater than end"
                )
            }
            ErrorKind::RepetitionMissing => {
                f.write_str("repetition operator has nothing to repeat")
            }
            ErrorKind::RepetitionStacked => f.write_str("repetition operators cannot be stacked"),
            ErrorKind::RepetitionCountUnclosed => {
                f.write_str("malformed counted repetition (write a literal brace as `\\{`)")
            }
            ErrorKind::RepetitionCountInvalid { min, max } => {
                write!(
                    f,
                    "invalid counted repetition {{{min},{max}}}: min is greater than max"
                )
            }
            ErrorKind::RepetitionCountTooLarge { limit } => {
                write!(f, "counted repetition exceeds the limit of {limit}")
            }
            ErrorKind::InvalidEscape { c } => write!(f, "invalid escape `\\{c}`"),
            ErrorKind::TrailingBackslash => f.write_str("pattern ends with a lone backslash"),
            ErrorKind::InvalidHexEscape => f.write_str("malformed hexadecimal escape"),
            ErrorKind::InvalidCodepoint { value } => {
                write!(
                    f,
                    "escape names an invalid Unicode scalar value 0x{value:X}"
                )
            }
            ErrorKind::InvalidGroupName => f.write_str("invalid capture group name"),
            ErrorKind::DuplicateGroupName { name } => {
                write!(f, "duplicate capture group name `{name}`")
            }
            ErrorKind::UnknownFlag { flag } => write!(f, "unknown flag `{flag}`"),
            ErrorKind::InvalidFlags => f.write_str("malformed flag group"),
            ErrorKind::Unsupported { feature } => write!(f, "unsupported feature: {feature}"),
            ErrorKind::NestLimitExceeded { limit } => {
                write!(f, "pattern nesting exceeds the limit of {limit}")
            }
            ErrorKind::SizeLimitExceeded { limit } => {
                write!(
                    f,
                    "compiled pattern exceeds the size limit of {limit} bytes"
                )
            }
            ErrorKind::PatternTooLong { limit } => {
                write!(f, "pattern exceeds the length limit of {limit} bytes")
            }
        }
    }
}

/// A pattern compilation error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    kind: ErrorKind,
    span: Option<Span>,
    pattern: String,
}

impl Error {
    pub(crate) fn new(kind: ErrorKind, span: Option<Span>, pattern: &str) -> Error {
        Error {
            kind,
            span,
            pattern: pattern.to_owned(),
        }
    }

    /// What went wrong.
    pub fn kind(&self) -> &ErrorKind {
        &self.kind
    }

    /// Where in the pattern it went wrong (`None` for limit errors).
    pub fn span(&self) -> Option<Span> {
        self.span
    }

    /// The pattern that failed to compile.
    pub fn pattern(&self) -> &str {
        &self.pattern
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Some(span) = self.span else {
            return write!(f, "regex compile error: {}", self.kind);
        };
        write!(
            f,
            "regex parse error at {}..{}: {}",
            span.start, span.end, self.kind
        )?;
        if self.pattern.contains('\n') {
            return Ok(());
        }
        let col = self
            .pattern
            .get(..span.start)
            .map_or(0, |s| s.chars().count());
        let width = self
            .pattern
            .get(span.start..span.end)
            .map_or(1, |s| s.chars().count())
            .max(1);
        write!(
            f,
            "\n    {}\n    {}{}",
            self.pattern,
            " ".repeat(col),
            "^".repeat(width)
        )
    }
}

impl std::error::Error for Error {}
