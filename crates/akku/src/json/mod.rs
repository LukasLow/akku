//! JSON: values, parsing and serialization without dependencies.
//!
//! The domain follows RFC 8259 and stays on `std`: no `serde`, no external
//! parser. It is always compiled — it does not depend on any feature.
//!
//! # Parse
//!
//! ```
//! use akku::json::parse;
//!
//! let value = parse(r#"{"a": [1, 2, 3]}"#).unwrap();
//! assert_eq!(value.get_path("a.1").and_then(|v| v.as_i64()), Some(2));
//! ```
//!
//! # Build and serialize
//!
//! ```
//! use akku::json::{json, to_string};
//!
//! let value = json!({ "ok": true, "items": [1, "two", null] });
//! assert_eq!(to_string(&value), r#"{"ok":true,"items":[1,"two",null]}"#);
//! ```
//!
//! # Stream tokens
//!
//! [`Decoder`] yields one [`Token`] at a time and never recurses, so nesting
//! depth is limited by [`ParseOptions::max_depth`], not by the stack.
//!
//! ```
//! use akku::json::{Decoder, Token};
//!
//! let mut decoder = Decoder::new(r#"[1, "two"]"#);
//! assert_eq!(decoder.next().unwrap(), Some(Token::ArrayStart));
//! assert!(matches!(
//!     decoder.next().unwrap(),
//!     Some(Token::Number(_))
//! ));
//! ```
//!
//! # Values
//!
//! [`Value`] is the data model: `null`, booleans, [`Number`], strings, arrays
//! and [`Object`]. Objects keep insertion order and compare without regard to
//! it. Lookup helpers cover dotted paths ([`Value::get_path`]) and RFC 6901
//! pointers ([`Value::pointer`]).
//!
//! # Errors
//!
//! Every error is an [`Error`] with an [`ErrorKind`], and parser errors also
//! carry the [`Position`] of the offending byte plus the path to the value
//! that failed (`a.b[1].c`).

mod convert;
mod de;
mod error;
mod macros;
mod ser;
mod value;

pub use self::convert::{FromJson, ToJson};
pub use self::de::{
    from_bytes, from_bytes_with, from_reader, from_reader_with, from_str, parse, parse_with,
    Decoder, LoneSurrogate, ParseOptions, Token,
};
pub use self::error::{Error, ErrorKind, Position, Result};
pub use self::ser::{
    to_string, to_string_pretty, to_string_with, write_to, write_to_pretty, write_to_with,
    SerializeOptions,
};
pub use self::value::{Entry, Number, Object, OccupiedEntry, VacantEntry, Value};

pub use self::macros::json;
