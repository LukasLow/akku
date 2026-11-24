//! YAML 1.2 core schema: the type of an untagged plain scalar.
//!
//! Only plain scalars are resolved. Quoted scalars are always strings, and an
//! explicit tag forces the type regardless of the literal's shape.
//!
//! The rules deliberately follow the YAML 1.2 core schema and nothing else:
//! `yes`, `no`, `on`, `off` and `NO` stay strings (the "Norway problem"),
//! `1_000` stays a string (no underscore separators), `0x` and `0o` are
//! integers but `0b` is not, and a leading zero never means octal.

use super::value::Value;

/// A tag from the YAML core schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tag {
    /// `!!str`
    Str,
    /// `!!int`
    Int,
    /// `!!float`
    Float,
    /// `!!bool`
    Bool,
    /// `!!null`
    Null,
    /// `!!seq`
    Seq,
    /// `!!map`
    Map,
}

impl Tag {
    /// Look up a tag by its short core schema name.
    pub(crate) fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "str" => Self::Str,
            "int" => Self::Int,
            "float" => Self::Float,
            "bool" => Self::Bool,
            "null" => Self::Null,
            "seq" => Self::Seq,
            "map" => Self::Map,
            _ => return None,
        })
    }

    /// The name as it is written in a document, for error messages.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Str => "!!str",
            Self::Int => "!!int",
            Self::Float => "!!float",
            Self::Bool => "!!bool",
            Self::Null => "!!null",
            Self::Seq => "!!seq",
            Self::Map => "!!map",
        }
    }
}

/// Resolve an untagged plain scalar.
pub(crate) fn resolve_plain(text: &str) -> Value {
    match text {
        "" | "~" | "null" | "Null" | "NULL" => Value::Null,
        "true" | "True" | "TRUE" => Value::Bool(true),
        "false" | "False" | "FALSE" => Value::Bool(false),
        _ => {
            if let Some(value) = parse_int(text) {
                Value::Int(value)
            } else if let Some(value) = parse_float(text) {
                Value::Float(value)
            } else {
                Value::String(text.to_string())
            }
        }
    }
}

/// Coerce a scalar's raw text to the type an explicit tag demands.
///
/// Returns `None` when the text does not fit the tag; the caller turns that
/// into a [`super::error::ErrorKind::TagMismatch`].
pub(crate) fn coerce(tag: Tag, raw: &str) -> Option<Value> {
    match tag {
        Tag::Str => Some(Value::String(raw.to_string())),
        Tag::Null => match raw {
            "" | "~" | "null" | "Null" | "NULL" => Some(Value::Null),
            _ => None,
        },
        Tag::Bool => match raw {
            "true" | "True" | "TRUE" => Some(Value::Bool(true)),
            "false" | "False" | "FALSE" => Some(Value::Bool(false)),
            _ => None,
        },
        Tag::Int => parse_int(raw).map(Value::Int),
        Tag::Float => parse_float(raw).map(Value::Float),
        Tag::Seq | Tag::Map => None,
    }
}

/// Whether text is a YAML 1.2 core schema integer.
///
/// Accepts `[-+]?[0-9]+`, `[-+]?0o[0-7]+` and `[-+]?0x[0-9a-fA-F]+`.
/// A literal whose magnitude does not fit `i64` is not an integer here; the
/// caller falls back to a float, matching the core schema's promotion.
pub(crate) fn parse_int(text: &str) -> Option<i64> {
    let (negative, body) = match text.as_bytes().first() {
        Some(b'-') => (true, &text[1..]),
        Some(b'+') => (false, &text[1..]),
        _ => (false, text),
    };
    if body.is_empty() {
        return None;
    }
    // `from_str_radix` would accept a sign after the prefix (`0x+1F`), which
    // the core schema does not: the digits must follow the prefix directly.
    let magnitude = if let Some(hex) = body.strip_prefix("0x") {
        if hex.is_empty() || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }
        u64::from_str_radix(hex, 16).ok()?
    } else if let Some(octal) = body.strip_prefix("0o") {
        if octal.is_empty() || !octal.bytes().all(|byte| matches!(byte, b'0'..=b'7')) {
            return None;
        }
        u64::from_str_radix(octal, 8).ok()?
    } else {
        if !body.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        body.parse::<u64>().ok()?
    };
    if negative {
        if magnitude > i64::MAX as u64 + 1 {
            return None;
        }
        if magnitude == i64::MAX as u64 + 1 {
            return Some(i64::MIN);
        }
        Some(-(magnitude as i64))
    } else {
        i64::try_from(magnitude).ok()
    }
}

/// Whether text is a YAML 1.2 core schema float.
///
/// Accepts `[-+]?(\.[0-9]+|[0-9]+(\.[0-9]*)?)([eE][-+]?[0-9]+)?` and the
/// special values `.inf`, `.Inf`, `.INF`, `-.inf` and `.nan`.
pub(crate) fn parse_float(text: &str) -> Option<f64> {
    match text {
        ".inf" | ".Inf" | ".INF" | "+.inf" | "+.Inf" | "+.INF" => return Some(f64::INFINITY),
        "-.inf" | "-.Inf" | "-.INF" => return Some(f64::NEG_INFINITY),
        ".nan" | ".NaN" | ".NAN" => return Some(f64::NAN),
        _ => {}
    }
    let bytes = text.as_bytes();
    let mut index = 0;
    if matches!(bytes.first(), Some(b'-') | Some(b'+')) {
        index += 1;
    }
    let mut digits = 0;
    while matches!(bytes.get(index), Some(byte) if byte.is_ascii_digit()) {
        index += 1;
        digits += 1;
    }
    if matches!(bytes.get(index), Some(b'.')) {
        index += 1;
        let mut fraction = 0;
        while matches!(bytes.get(index), Some(byte) if byte.is_ascii_digit()) {
            index += 1;
            fraction += 1;
        }
        if digits == 0 && fraction == 0 {
            return None;
        }
        digits += fraction;
    }
    if matches!(bytes.get(index), Some(b'e') | Some(b'E')) {
        index += 1;
        if matches!(bytes.get(index), Some(b'-') | Some(b'+')) {
            index += 1;
        }
        let mut exponent = 0;
        while matches!(bytes.get(index), Some(byte) if byte.is_ascii_digit()) {
            index += 1;
            exponent += 1;
        }
        if exponent == 0 {
            return None;
        }
    }
    if digits == 0 || index != bytes.len() {
        return None;
    }
    text.parse::<f64>().ok()
}
