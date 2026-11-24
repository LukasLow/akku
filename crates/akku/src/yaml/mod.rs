//! YAML 1.2 (core schema) parsing, values and serialization without dependencies.
//!
//! The domain stays on `std`: no `serde`, no external parser. It is always
//! compiled — it does not depend on any feature.
//!
//! # Parse
//!
//! ```
//! use akku::yaml::parse;
//!
//! let value = parse("server:\n  host: localhost\n  port: 8080\n").unwrap();
//! assert_eq!(value.get_path("server.port").and_then(|v| v.as_i64()), Some(8080));
//! ```
//!
//! Only the YAML 1.2 core schema is resolved, so `NO`, `yes` and `on` are
//! strings, not booleans:
//!
//! ```
//! use akku::yaml::parse;
//!
//! let value = parse("country: NO\n").unwrap();
//! assert_eq!(value.get("country").and_then(|v| v.as_str()), Some("NO"));
//! ```
//!
//! # Build and serialize
//!
//! ```
//! use akku::yaml::{parse, to_string};
//!
//! let value = parse("items:\n  - 1\n  - 2\n").unwrap();
//! assert_eq!(parse(&to_string(&value)).unwrap(), value);
//! ```
//!
//! # Multi-document streams
//!
//! [`parse`] reads exactly one document and reports [`ErrorKind::MultipleDocuments`]
//! when a second one follows. [`parse_all`] reads every document, and
//! [`Documents`] yields them lazily.
//!
//! ```
//! use akku::yaml::parse_all;
//!
//! let documents = parse_all("---\na: 1\n---\nb: 2\n").unwrap();
//! assert_eq!(documents.len(), 2);
//! ```
//!
//! # What this module covers
//!
//! Block mappings and sequences, flow collections, plain and quoted scalars,
//! literal and folded block scalars with chomping and an explicit indentation
//! indicator, comments, anchors and aliases, the `<<` merge key, explicit
//! `!!str`/`!!int`/`!!float`/`!!bool`/`!!null` tags, the `%YAML` directive and
//! `---`/`...` document markers.
//!
//! # What it deliberately does not do
//!
//! YAML 1.1 types (`yes`, `no`, `on`, `off`, sexagesimals, `0b` binary),
//! `!!timestamp`, `!!binary`, local tags (`!foo`), `%TAG` handles, and custom
//! tag URIs. Block scalars are written as quoted single-line scalars, so the
//! writer never folds.
//!
//! # Limits
//!
//! Hostile input is contained by [`ParseOptions`]: nesting depth, alias count,
//! total nodes and the number of nodes an alias may expand to. Values drop,
//! clone and compare iteratively, so deeply nested input cannot overflow the
//! stack.

mod error;
mod parser;
mod scanner;
mod schema;
mod ser;
mod value;

pub use self::error::{Error, ErrorKind, Position, Result};
pub use self::parser::{Documents, ParseOptions};
pub use self::ser::{QuoteStyle, WriteOptions};
pub use self::value::{Key, Mapping, Value};

/// Parse exactly one document with the default [`ParseOptions`].
///
/// An empty input yields [`Value::Null`]. Input that holds more than one
/// document fails with [`ErrorKind::MultipleDocuments`]; use [`parse_all`] for
/// streams.
pub fn parse(input: &str) -> Result<Value> {
    parser::parse_one(input, ParseOptions::new())
}

/// Parse exactly one document with explicit [`ParseOptions`].
pub fn parse_with(input: &str, options: ParseOptions) -> Result<Value> {
    parser::parse_one(input, options)
}

/// Parse every document of a stream with the default [`ParseOptions`].
pub fn parse_all(input: &str) -> Result<Vec<Value>> {
    parser::parse_many(input, ParseOptions::new())
}

/// Parse every document of a stream with explicit [`ParseOptions`].
pub fn parse_all_with(input: &str, options: ParseOptions) -> Result<Vec<Value>> {
    parser::parse_many(input, options)
}

/// Serialize a value with the default [`WriteOptions`].
///
/// The output is deterministic and re-parses to an equal value. It always ends
/// with exactly one line break.
pub fn to_string(value: &Value) -> String {
    ser::to_string(value)
}

/// Serialize a value with explicit [`WriteOptions`].
pub fn to_string_with(value: &Value, options: WriteOptions) -> String {
    ser::to_string_with(value, options)
}

/// Write a value to any [`std::io::Write`].
pub fn write_to<W: std::io::Write>(value: &Value, writer: W) -> Result<()> {
    ser::write_to(value, writer)
}

/// Write a value to any [`std::io::Write`] with explicit options.
pub fn write_to_with<W: std::io::Write>(
    value: &Value,
    options: WriteOptions,
    writer: W,
) -> Result<()> {
    ser::write_to_with(value, options, writer)
}

/// Write a multi-document stream, each document behind an explicit `---`.
pub fn write_all_to<W: std::io::Write>(documents: &[Value], writer: W) -> Result<()> {
    ser::write_all_to(documents, writer)
}
