//! Zero-dependency XML 1.0 pull parser, DOM, writer, and XXE-safe entity handling.
//!
//! `akku::xml` provides:
//! - A streaming pull parser ([`Reader`]) yielding [`Event`]s without recursion.
//! - A lightweight owned DOM ([`Document`], [`Element`], [`Node`], [`Attribute`]) with
//!   namespace-aware helpers and an iterative `Drop` implementation that cannot overflow
//!   the stack on deeply nested documents.
//! - A streaming and DOM serializer ([`Writer`], [`to_string`], [`to_string_pretty`]).
//!
//! By default, `<!DOCTYPE ...>` and DTD entity declarations are rejected immediately
//! ([`DtdPolicy::Reject`]), preventing XXE and billion-laughs entity-expansion attacks.
//!
//! # Quick start (DOM)
//!
//! ```
//! use akku::xml::{parse, to_string};
//!
//! let doc = parse(r#"<feed xml:lang="en"><title>Akku &amp; XML</title></feed>"#).unwrap();
//! let title = doc.root().child("title").unwrap();
//! assert_eq!(title.text(), "Akku & XML");
//! assert_eq!(
//!     to_string(&doc).unwrap(),
//!     r#"<feed xml:lang="en"><title>Akku &amp; XML</title></feed>"#
//! );
//! ```
//!
//! # Pull parsing
//!
//! ```
//! use akku::xml::{Event, Reader};
//!
//! let mut reader = Reader::new("<item id=\"42\">hello</item>");
//! assert!(matches!(reader.next_event().unwrap(), Event::Start(tag) if tag.attr("id") == Some("42")));
//! assert!(matches!(reader.next_event().unwrap(), Event::Text(t) if t == "hello"));
//! assert_eq!(reader.next_event().unwrap(), Event::End("item"));
//! assert_eq!(reader.next_event().unwrap(), Event::Eof);
//! ```

mod dom;
mod error;
mod reader;
mod writer;

pub use self::dom::{Attribute, Document, Element, Node};
pub use self::error::{Error, ErrorKind, Position, Result};
pub use self::reader::{DtdPolicy, Event, ParseOptions, RawAttr, Reader, StartTag, XmlDecl};
pub use self::writer::{
    to_string, to_string_pretty, to_string_with, write_to, write_to_pretty, write_to_with,
    SerializeOptions, Writer,
};

/// Parse a UTF-8 XML string into a [`Document`] with strict default [`ParseOptions`].
pub fn parse(input: &str) -> Result<Document> {
    Document::parse(input)
}

/// Parse a UTF-8 XML string into a [`Document`] with custom [`ParseOptions`].
pub fn parse_with(input: &str, options: ParseOptions) -> Result<Document> {
    Document::parse_with(input, options)
}

/// Parse UTF-8 XML bytes (stripping an optional UTF-8 BOM) into a [`Document`].
pub fn from_bytes(bytes: &[u8]) -> Result<Document> {
    Document::from_bytes(bytes)
}

/// Parse UTF-8 XML bytes into a [`Document`] with custom [`ParseOptions`].
pub fn from_bytes_with(bytes: &[u8], options: ParseOptions) -> Result<Document> {
    Document::from_bytes_with(bytes, options)
}
