//! Errors and source positions for the XML domain.

use std::fmt;

/// Convenience result alias for XML operations.
pub type Result<T> = std::result::Result<T, Error>;

/// A position inside the parsed XML input.
///
/// `offset` is a zero-based byte offset, while `line` and `column` are
/// one-based (counting Unicode scalar values for `column`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Position {
    /// Zero-based byte offset from the start of the input.
    pub offset: usize,
    /// One-based line number.
    pub line: u32,
    /// One-based column number in Unicode scalar values.
    pub column: u32,
}

impl Position {
    /// Create a position at the start of input (`offset = 0`, `line = 1`, `column = 1`).
    pub const fn start() -> Self {
        Self {
            offset: 0,
            line: 1,
            column: 1,
        }
    }
}

/// What went wrong while parsing or serializing XML.
#[non_exhaustive]
#[derive(Debug)]
pub enum ErrorKind {
    /// Input ended before the document or current construct completed.
    UnexpectedEof,
    /// An unexpected token or character was encountered.
    UnexpectedToken {
        /// What the parser or writer expected.
        expected: &'static str,
        /// What was encountered instead.
        found: String,
    },
    /// A character outside the XML 1.0 `Char` production appeared.
    InvalidXmlChar {
        /// The offending Unicode code point.
        codepoint: u32,
    },
    /// An element, attribute, or processing instruction name was invalid.
    InvalidName {
        /// The invalid name.
        name: String,
    },
    /// A namespace prefix or declaration violated XML Namespaces 1.0 rules.
    InvalidNamespace {
        /// Reason the namespace usage is invalid.
        reason: String,
    },
    /// A closing tag did not match the currently open start tag.
    MismatchedTag {
        /// Expected closing tag name.
        expected: String,
        /// Actual closing tag name found.
        found: String,
    },
    /// One or more start tags were never closed before EOF.
    UnclosedTag {
        /// Name of the innermost unclosed tag.
        name: String,
    },
    /// A closing tag appeared with no matching open element.
    UnexpectedEndTag {
        /// Name of the stray closing tag.
        name: String,
    },
    /// Two attributes on the same element shared the same raw name.
    DuplicateAttribute {
        /// Repeated attribute name.
        name: String,
    },
    /// Two attributes on the same element resolved to the same `(namespace, local_name)`.
    DuplicateNamespaceAttribute {
        /// Shared namespace URI.
        namespace: String,
        /// Shared local attribute name.
        local_name: String,
    },
    /// An unrecognised named entity reference such as `&custom;` was encountered.
    UnknownEntity {
        /// Entity name without `&` and `;`.
        name: String,
    },
    /// A numeric character reference (`&#...;` or `&#x...;`) was malformed or invalid.
    InvalidCharRef {
        /// Raw reference text.
        raw: String,
    },
    /// The sequence `]]>` appeared in character data outside a CDATA section.
    UnescapedCdataEndInText,
    /// A comment contained `--` or ended with `-`.
    InvalidComment,
    /// A processing instruction used the reserved `xml` target or invalid data.
    InvalidPiTarget {
        /// The offending target.
        target: String,
    },
    /// The `<?xml ...?>` declaration was malformed or appeared after content.
    InvalidDecl,
    /// The document contained no root element.
    MissingRootElement,
    /// Markup or non-whitespace character data appeared outside the root element.
    TrailingContentAfterRoot,
    /// A `<!DOCTYPE` or DTD declaration (`<!ENTITY`, etc.) was rejected.
    DtdNotAllowed,
    /// An external DTD or entity reference (`SYSTEM`/`PUBLIC`) was rejected.
    ExternalEntityNotAllowed,
    /// The configured element nesting limit was exceeded.
    MaxDepthExceeded {
        /// Configured limit.
        limit: usize,
    },
    /// The configured node count limit was exceeded.
    MaxNodesExceeded {
        /// Configured limit.
        limit: usize,
    },
    /// The configured per-element attribute count limit was exceeded.
    MaxAttributesExceeded {
        /// Configured limit.
        limit: usize,
    },
    /// A single token (text, attribute, comment, CDATA, or PI) exceeded the byte cap.
    MaxTokenLenExceeded {
        /// Configured byte limit.
        limit: usize,
    },
    /// Input bytes were not valid UTF-8.
    InvalidUtf8,
    /// The [`Reader`](crate::xml::Reader) already returned an error and cannot resume.
    ReaderPoisoned,
    /// An underlying I/O operation failed.
    Io(std::io::Error),
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedEof => f.write_str("unexpected end of XML input"),
            Self::UnexpectedToken { expected, found } => {
                write!(f, "expected {expected}, found {found}")
            }
            Self::InvalidXmlChar { codepoint } => {
                write!(f, "invalid XML 1.0 character U+{codepoint:04X}")
            }
            Self::InvalidName { name } => write!(f, "invalid XML name '{name}'"),
            Self::InvalidNamespace { reason } => write!(f, "invalid XML namespace: {reason}"),
            Self::MismatchedTag { expected, found } => {
                write!(
                    f,
                    "mismatched closing tag: expected </{expected}>, found </{found}>"
                )
            }
            Self::UnclosedTag { name } => write!(f, "unclosed XML element <{name}>"),
            Self::UnexpectedEndTag { name } => write!(f, "unexpected closing tag </{name}>"),
            Self::DuplicateAttribute { name } => write!(f, "duplicate attribute '{name}'"),
            Self::DuplicateNamespaceAttribute {
                namespace,
                local_name,
            } => write!(
                f,
                "duplicate namespaced attribute '{local_name}' in namespace '{namespace}'"
            ),
            Self::UnknownEntity { name } => write!(f, "unknown entity reference '&{name};'"),
            Self::InvalidCharRef { raw } => write!(f, "invalid character reference '{raw}'"),
            Self::UnescapedCdataEndInText => {
                f.write_str("unescaped ']]>' sequence in text content")
            }
            Self::InvalidComment => {
                f.write_str("invalid XML comment (contains '--' or ends with '-')")
            }
            Self::InvalidPiTarget { target } => {
                write!(f, "invalid processing instruction target '{target}'")
            }
            Self::InvalidDecl => f.write_str("invalid XML declaration"),
            Self::MissingRootElement => f.write_str("XML document has no root element"),
            Self::TrailingContentAfterRoot => {
                f.write_str("unexpected content outside the root element")
            }
            Self::DtdNotAllowed => f.write_str("DTD declarations and entities are not allowed"),
            Self::ExternalEntityNotAllowed => {
                f.write_str("external entities and external DTD subsets are not allowed")
            }
            Self::MaxDepthExceeded { limit } => {
                write!(f, "maximum XML nesting depth of {limit} exceeded")
            }
            Self::MaxNodesExceeded { limit } => {
                write!(f, "maximum XML node count of {limit} exceeded")
            }
            Self::MaxAttributesExceeded { limit } => {
                write!(f, "maximum attribute count of {limit} exceeded")
            }
            Self::MaxTokenLenExceeded { limit } => {
                write!(f, "maximum token length of {limit} bytes exceeded")
            }
            Self::InvalidUtf8 => f.write_str("XML input is not valid UTF-8"),
            Self::ReaderPoisoned => {
                f.write_str("XML reader cannot continue after a previous error")
            }
            Self::Io(err) => write!(f, "I/O error: {err}"),
        }
    }
}

/// An error returned while parsing or writing XML.
#[derive(Debug)]
pub struct Error {
    kind: ErrorKind,
    position: Option<Position>,
}

impl Error {
    /// Construct an error without a source position.
    pub fn new(kind: ErrorKind) -> Self {
        Self {
            kind,
            position: None,
        }
    }

    /// Construct an error with a source position.
    pub fn at(kind: ErrorKind, position: Position) -> Self {
        Self {
            kind,
            position: Some(position),
        }
    }

    /// The structured error kind.
    pub fn kind(&self) -> &ErrorKind {
        &self.kind
    }

    /// The source position where the error occurred, if known.
    pub fn position(&self) -> Option<Position> {
        self.position
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.position {
            Some(pos) => write!(f, "{} at {}:{}", self.kind, pos.line, pos.column),
            None => write!(f, "{}", self.kind),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.kind {
            ErrorKind::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Self {
        Self::new(ErrorKind::Io(err))
    }
}
