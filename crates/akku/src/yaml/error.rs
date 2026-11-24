//! Errors and positions for the YAML domain.
//!
//! Every fallible YAML operation returns [`Result`] with an [`Error`] that
//! carries an [`ErrorKind`], the exact [`Position`] in the input and the path
//! to the offending value (`server.port`, `items[2].name`).

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

/// What went wrong while parsing or writing YAML.
///
/// The enum is `#[non_exhaustive]`: more precise variants may be added without
/// a breaking change. Match with a `_` arm.
#[non_exhaustive]
#[derive(Debug)]
pub enum ErrorKind {
    /// The input ended in the middle of a node.
    UnexpectedEof,
    /// A character appeared where the grammar does not allow it.
    UnexpectedChar {
        /// The offending character.
        ch: char,
    },
    /// A specific token was required at this point.
    Expected {
        /// What the parser wanted, for example `"',' or ']'"`.
        expected: &'static str,
        /// What it found, for example `"a plain scalar"`.
        found: &'static str,
    },
    /// A backslash escape is not part of YAML.
    InvalidEscape {
        /// The character that followed the backslash.
        escape: char,
    },
    /// A `\x`, `\u` or `\U` escape did not carry the right number of hex digits.
    InvalidUnicodeEscape,
    /// A number literal does not follow the YAML 1.2 core schema.
    InvalidNumber {
        /// A short explanation, for example `"integer out of range"`.
        reason: &'static str,
    },
    /// A tag is not one of the supported core schema tags.
    InvalidTag {
        /// The tag as it appeared in the input.
        tag: String,
    },
    /// An explicit tag does not fit the node it is applied to.
    TagMismatch {
        /// The tag, for example `"!!int"`.
        tag: &'static str,
        /// The kind of node it was applied to, for example `"string"`.
        found: &'static str,
    },
    /// Indentation is inconsistent with the enclosing block.
    BadIndentation {
        /// A short explanation, for example `"no matching block node"`.
        reason: &'static str,
    },
    /// A tab was used for indentation.
    TabIndentation,
    /// A block scalar appeared inside a flow collection.
    BlockScalarInFlow,
    /// A block mapping key was not followed by `:`.
    MissingValueSeparator,
    /// The same key appeared twice in one mapping.
    DuplicateKey {
        /// The repeated key, best-effort as text.
        key: String,
    },
    /// A sequence or mapping was used as a key while complex keys are disabled.
    UnsupportedKeyKind {
        /// The kind that was used, for example `"sequence"`.
        kind: &'static str,
    },
    /// An alias refers to an anchor that was not defined before it.
    AnchorNotFound {
        /// The anchor name without the `*`.
        name: String,
    },
    /// The document contains more aliases than allowed.
    AliasLimitExceeded {
        /// The configured limit.
        limit: usize,
    },
    /// Alias expansion would create more nodes than allowed.
    ///
    /// This is the billion-laughs guard: an alias bomb is rejected before the
    /// nodes are cloned.
    AliasExpansionLimitExceeded {
        /// The configured limit.
        limit: usize,
    },
    /// The configured nesting limit was exceeded.
    MaxDepthExceeded {
        /// The configured limit.
        limit: usize,
    },
    /// The configured node budget was exceeded.
    MaxNodesExceeded {
        /// The configured limit.
        limit: usize,
    },
    /// A merge key holds something that is not a mapping or a list of mappings.
    MergeNotMapping {
        /// The kind that was found, for example `"int"`.
        found: &'static str,
    },
    /// A `%` directive other than `%YAML` is not supported.
    UnsupportedDirective {
        /// The directive name.
        name: String,
    },
    /// The `%YAML` directive asks for a version this parser does not speak.
    UnsupportedVersion {
        /// Major version.
        major: u8,
        /// Minor version.
        minor: u8,
    },
    /// The input contains more than one document and one was expected.
    MultipleDocuments,
    /// Content followed the value where it cannot stand.
    TrailingContent,
    /// Writing to the target failed.
    Io(std::io::Error),
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedEof => f.write_str("unexpected end of input"),
            Self::UnexpectedChar { ch } => write!(f, "unexpected character {ch:?}"),
            Self::Expected { expected, found } => {
                write!(f, "expected {expected}, found {found}")
            }
            Self::InvalidEscape { escape } => {
                write!(f, "invalid escape sequence \\{escape}")
            }
            Self::InvalidUnicodeEscape => f.write_str("invalid unicode escape"),
            Self::InvalidNumber { reason } => write!(f, "invalid number: {reason}"),
            Self::InvalidTag { tag } => write!(f, "unsupported tag {tag}"),
            Self::TagMismatch { tag, found } => {
                write!(f, "tag {tag} does not apply to a {found}")
            }
            Self::BadIndentation { reason } => write!(f, "bad indentation: {reason}"),
            Self::TabIndentation => f.write_str("tabs cannot be used for indentation"),
            Self::BlockScalarInFlow => {
                f.write_str("block scalars are not allowed in a flow collection")
            }
            Self::MissingValueSeparator => f.write_str("expected ':' after a mapping key"),
            Self::DuplicateKey { key } => write!(f, "duplicate mapping key {key:?}"),
            Self::UnsupportedKeyKind { kind } => {
                write!(f, "a {kind} cannot be used as a key")
            }
            Self::AnchorNotFound { name } => write!(f, "unknown anchor {name:?}"),
            Self::AliasLimitExceeded { limit } => {
                write!(f, "more than {limit} aliases in one document")
            }
            Self::AliasExpansionLimitExceeded { limit } => {
                write!(f, "alias expansion would create more than {limit} nodes")
            }
            Self::MaxDepthExceeded { limit } => {
                write!(f, "nesting deeper than {limit} levels")
            }
            Self::MaxNodesExceeded { limit } => write!(f, "more than {limit} nodes"),
            Self::MergeNotMapping { found } => {
                write!(
                    f,
                    "merge key needs a mapping or a sequence of mappings, found {found}"
                )
            }
            Self::UnsupportedDirective { name } => {
                write!(f, "unsupported directive %{name}")
            }
            Self::UnsupportedVersion { major, minor } => {
                write!(f, "unsupported YAML version {major}.{minor}")
            }
            Self::MultipleDocuments => {
                f.write_str("input contains more than one document; use parse_all")
            }
            Self::TrailingContent => f.write_str("unexpected content after the value"),
            Self::Io(error) => write!(f, "write failed: {error}"),
        }
    }
}

/// A YAML error: a kind, a position and the path to the offending value.
#[derive(Debug)]
pub struct Error {
    kind: ErrorKind,
    position: Position,
    path: String,
}

impl Error {
    /// Build an error from its parts.
    pub(crate) fn new(kind: ErrorKind, position: Position) -> Self {
        Self {
            kind,
            position,
            path: String::new(),
        }
    }

    /// Attach the path to the offending value.
    pub(crate) fn with_path(mut self, path: String) -> Self {
        self.path = path;
        self
    }

    /// What went wrong.
    pub fn kind(&self) -> &ErrorKind {
        &self.kind
    }

    /// Where it went wrong.
    pub fn position(&self) -> Position {
        self.position
    }

    /// The one-based line of the offending token.
    pub fn line(&self) -> u32 {
        self.position.line
    }

    /// The one-based column of the offending token, counted in characters.
    pub fn column(&self) -> u32 {
        self.position.column
    }

    /// The zero-based byte offset of the offending token.
    pub fn offset(&self) -> usize {
        self.position.offset
    }

    /// The path to the offending value, for example `server.port` or `items[2]`.
    ///
    /// The empty string means the document root.
    pub fn path(&self) -> &str {
        &self.path
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "yaml error at line {}, column {}",
            self.position.line, self.position.column
        )?;
        if !self.path.is_empty() {
            write!(f, " (path `{}`)", self.path)?;
        }
        write!(f, ": {}", self.kind)
    }
}

impl std::error::Error for Error {}

/// The result type for the YAML domain.
pub type Result<T> = std::result::Result<T, Error>;
