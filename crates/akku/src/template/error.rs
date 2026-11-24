//! Template errors with exact source positions.

use std::fmt;

/// A position in a template's source.
///
/// Both numbers are one-based; the column counts characters, not bytes, and a
/// `\r\n` line ending counts as a single newline.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Position {
    /// One-based line number.
    pub line: u32,
    /// One-based column number, counted in characters.
    pub column: u32,
}

impl Position {
    /// The position of byte `offset` in `source`.
    pub(crate) fn of(source: &str, offset: usize) -> Position {
        let offset = offset.min(source.len());
        let mut line = 1u32;
        let mut line_start = 0usize;
        for (index, byte) in source.as_bytes()[..offset].iter().enumerate() {
            if *byte == b'\n' {
                line += 1;
                line_start = index + 1;
            }
        }
        let column = source[line_start..]
            .char_indices()
            .take_while(|(index, _)| line_start + index < offset)
            .count();
        Position {
            line,
            column: column as u32 + 1,
        }
    }
}

/// What went wrong while compiling or rendering a template.
///
/// The enum is `#[non_exhaustive]`: more variants may be added without a
/// breaking change. Match with a `_` arm.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub enum ErrorKind {
    /// A `{{`, `{%` or `{#` was never closed.
    UnclosedTag {
        /// The opening delimiter.
        open: &'static str,
    },
    /// A block tag was never closed.
    UnclosedBlock {
        /// The tag that was left open, for example `"if"`.
        tag: &'static str,
    },
    /// An end tag closed a different block than the open one.
    MismatchedEndTag {
        /// The end tag that was needed.
        expected: &'static str,
        /// The end tag that was found.
        found: String,
    },
    /// A tag appeared where it does not belong (`endif` without `if`, …).
    UnexpectedTag {
        /// The tag name.
        name: String,
    },
    /// There is no such tag.
    UnknownTag {
        /// The tag name.
        name: String,
    },
    /// A token appeared where another was required.
    UnexpectedToken {
        /// What was needed.
        expected: &'static str,
        /// What was found.
        found: String,
    },
    /// A string literal is missing its closing quote.
    UnterminatedString,
    /// A number literal is malformed or out of range.
    InvalidNumber,
    /// Tags or expressions are nested deeper than the limit.
    NestingTooDeep {
        /// The configured limit.
        limit: usize,
    },
    /// `{% extends %}` is not the first tag of the template.
    ExtendsNotFirst,
    /// Two blocks of one template share a name.
    DuplicateBlock {
        /// The block name.
        name: String,
    },
    /// There is no such filter.
    UnknownFilter {
        /// The filter name.
        name: String,
    },
    /// A filter returned an error.
    FilterFailed {
        /// The filter name.
        name: String,
        /// The filter's message.
        message: String,
    },
    /// A variable or attribute does not exist.
    UndefinedVariable {
        /// The dotted path up to the part that is missing.
        name: String,
    },
    /// A `for` loop was given something that is not a list or a map.
    NotIterable {
        /// The type that was found.
        type_name: &'static str,
    },
    /// A list or map cannot be printed; use `join` or `json`.
    NotPrintable {
        /// The type that was found.
        type_name: &'static str,
    },
    /// An operator was applied to unsuitable operands.
    InvalidOperation {
        /// What was wrong.
        message: String,
    },
    /// An included or extended template does not exist.
    TemplateNotFound {
        /// The template name.
        name: String,
    },
    /// Includes or `extends` chains are nested deeper than the limit.
    IncludeDepthExceeded {
        /// The configured limit.
        limit: usize,
    },
    /// More includes were rendered than the limit allows.
    IncludeLimitExceeded {
        /// The configured limit.
        limit: usize,
    },
    /// More loop iterations ran than the limit allows.
    LoopLimitExceeded {
        /// The configured limit.
        limit: usize,
    },
    /// The output (or an intermediate string) is longer than the limit.
    OutputTooLarge {
        /// The configured limit in bytes.
        limit: usize,
    },
    /// The template copied more value data (list literals and filter
    /// arguments duplicating existing values) than the per-render budget of
    /// 16 times the output limit allows.
    CopyLimitExceeded {
        /// The budget in bytes.
        limit: usize,
    },
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnclosedTag { open } => write!(formatter, "unclosed `{open}`"),
            Self::UnclosedBlock { tag } => write!(formatter, "unclosed block `{tag}`"),
            Self::MismatchedEndTag { expected, found } => {
                write!(formatter, "expected `{expected}`, found `{found}`")
            }
            Self::UnexpectedTag { name } => write!(formatter, "unexpected tag `{name}`"),
            Self::UnknownTag { name } => write!(formatter, "unknown tag `{name}`"),
            Self::UnexpectedToken { expected, found } => {
                write!(formatter, "expected {expected}, found {found}")
            }
            Self::UnterminatedString => formatter.write_str("unterminated string"),
            Self::InvalidNumber => formatter.write_str("invalid number"),
            Self::NestingTooDeep { limit } => {
                write!(formatter, "nesting deeper than the limit of {limit}")
            }
            Self::ExtendsNotFirst => formatter.write_str("`extends` must be the first tag"),
            Self::DuplicateBlock { name } => write!(formatter, "duplicate block `{name}`"),
            Self::UnknownFilter { name } => write!(formatter, "unknown filter `{name}`"),
            Self::FilterFailed { name, message } => {
                write!(formatter, "filter `{name}` failed: {message}")
            }
            Self::UndefinedVariable { name } => write!(formatter, "undefined variable `{name}`"),
            Self::NotIterable { type_name } => {
                write!(formatter, "cannot loop over a value of type {type_name}")
            }
            Self::NotPrintable { type_name } => write!(
                formatter,
                "cannot print a value of type {type_name}; use `join` or `json`"
            ),
            Self::InvalidOperation { message } => formatter.write_str(message),
            Self::TemplateNotFound { name } => write!(formatter, "template `{name}` not found"),
            Self::IncludeDepthExceeded { limit } => {
                write!(
                    formatter,
                    "includes nested deeper than the limit of {limit}"
                )
            }
            Self::IncludeLimitExceeded { limit } => {
                write!(formatter, "more than {limit} includes rendered")
            }
            Self::LoopLimitExceeded { limit } => {
                write!(formatter, "more than {limit} loop iterations")
            }
            Self::OutputTooLarge { limit } => {
                write!(formatter, "output larger than the limit of {limit} bytes")
            }
            Self::CopyLimitExceeded { limit } => {
                write!(
                    formatter,
                    "copied values larger than the budget of {limit} bytes"
                )
            }
        }
    }
}

/// An error from compiling or rendering a template.
///
/// ```
/// use akku::template::{render_str, Context, ErrorKind};
///
/// let error = render_str("Hello\n  {{ user.name }}", &Context::new()).unwrap_err();
/// assert_eq!(error.position().map(|p| (p.line, p.column)), Some((2, 6)));
/// assert!(matches!(error.kind(), ErrorKind::UndefinedVariable { name } if name == "user"));
/// assert_eq!(error.to_string(), "undefined variable `user` at <string>:2:6");
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct Error {
    kind: ErrorKind,
    template: Option<String>,
    position: Option<Position>,
}

impl Error {
    pub(crate) fn new(kind: ErrorKind) -> Self {
        Self {
            kind,
            template: None,
            position: None,
        }
    }

    /// An error at byte `offset` of `source`, which belongs to `template`.
    pub(crate) fn at(kind: ErrorKind, template: Option<&str>, source: &str, offset: usize) -> Self {
        Self {
            kind,
            template: template.map(str::to_owned),
            position: Some(Position::of(source, offset)),
        }
    }

    /// What went wrong.
    pub fn kind(&self) -> &ErrorKind {
        &self.kind
    }

    /// The name of the template whose source is at fault.
    ///
    /// For errors inside an included or extended template this is the inner
    /// template. `None` for anonymous templates.
    pub fn template(&self) -> Option<&str> {
        self.template.as_deref()
    }

    /// The line and column in the template's source, if the error has one.
    ///
    /// Only a failed lookup by name (`Engine::render("missing", …)`) has no
    /// position.
    pub fn position(&self) -> Option<Position> {
        self.position
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.kind, formatter)?;
        if let Some(position) = self.position {
            write!(
                formatter,
                " at {}:{}:{}",
                self.template.as_deref().unwrap_or("<string>"),
                position.line,
                position.column
            )?;
        }
        Ok(())
    }
}

impl std::error::Error for Error {}

/// The result type of the template module.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// A compile-time error that has an offset but is not yet bound to a source.
///
/// It is passed around boxed: the compilers return `Result<_, RawError>` all
/// over the place, and a small error keeps every stack frame small.
#[derive(Debug)]
pub(crate) struct Fail {
    pub(crate) kind: ErrorKind,
    pub(crate) offset: usize,
}

pub(crate) type RawError = Box<Fail>;

impl Fail {
    pub(crate) fn new(kind: ErrorKind, offset: usize) -> RawError {
        Box::new(Self { kind, offset })
    }

    pub(crate) fn bind(self, template: Option<&str>, source: &str) -> Error {
        Error::at(self.kind, template, source, self.offset)
    }
}
