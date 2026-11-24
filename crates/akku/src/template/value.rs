//! Template values and the conversions into them.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::fmt;

use crate::html::Html;
use crate::json;

use super::context::Context;

/// How deep nested JSON is followed when converting. Anything deeper becomes
/// [`Value::Null`], so hostile input cannot overflow the stack.
const MAX_CONVERT_DEPTH: usize = 128;

/// A value a template can read, print, loop over and filter.
///
/// [`Value::Str`] is untrusted text and is escaped when printed under
/// [`AutoEscape::Html`](crate::template::AutoEscape::Html). [`Value::Safe`] is
/// trusted markup and is printed as is: create it deliberately with
/// [`Value::safe`], the `safe` filter or [`Html`].
///
/// ```
/// use akku::template::Value;
///
/// assert_eq!(Value::from(3), Value::Int(3));
/// assert_eq!(Value::from("a"), Value::Str("a".into()));
/// assert_eq!(Value::from(vec![1, 2]).as_list().map(Vec::len), Some(2));
/// assert!(!Value::from("").is_truthy());
/// ```
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Value {
    /// No value. Prints as nothing and is falsy.
    #[default]
    Null,
    /// A boolean.
    Bool(bool),
    /// An integer.
    Int(i64),
    /// A floating-point number.
    Float(f64),
    /// Untrusted text: escaped on output under `AutoEscape::Html`.
    Str(String),
    /// Trusted markup: never escaped.
    Safe(String),
    /// A list of values.
    List(Vec<Value>),
    /// A map from string keys to values, iterated in key order.
    Map(BTreeMap<String, Value>),
}

impl Value {
    /// Trusted markup that is printed without escaping.
    ///
    /// Only use this for text you control; see [`Html`] for the same idea at
    /// the HTTP layer.
    pub fn safe(markup: impl Into<String>) -> Value {
        Value::Safe(markup.into())
    }

    /// The name of the value's type, as used in error messages.
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Null => "none",
            Value::Bool(_) => "boolean",
            Value::Int(_) => "integer",
            Value::Float(_) => "float",
            Value::Str(_) | Value::Safe(_) => "string",
            Value::List(_) => "list",
            Value::Map(_) => "map",
        }
    }

    /// Whether this is [`Value::Null`].
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    /// Template truthiness: `none`, `false`, `0`, `0.0`, empty strings, empty
    /// lists and empty maps are falsy; everything else is truthy.
    pub fn is_truthy(&self) -> bool {
        match self {
            Value::Null => false,
            Value::Bool(value) => *value,
            Value::Int(value) => *value != 0,
            Value::Float(value) => *value != 0.0,
            Value::Str(text) | Value::Safe(text) => !text.is_empty(),
            Value::List(items) => !items.is_empty(),
            Value::Map(entries) => !entries.is_empty(),
        }
    }

    /// The boolean, if this is one.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(value) => Some(*value),
            _ => None,
        }
    }

    /// The integer, if this is one.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int(value) => Some(*value),
            _ => None,
        }
    }

    /// The number as `f64`, if this is an integer or a float.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Int(value) => Some(*value as f64),
            Value::Float(value) => Some(*value),
            _ => None,
        }
    }

    /// The text, if this is [`Value::Str`] or [`Value::Safe`].
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(text) | Value::Safe(text) => Some(text),
            _ => None,
        }
    }

    /// The items, if this is a list.
    pub fn as_list(&self) -> Option<&Vec<Value>> {
        match self {
            Value::List(items) => Some(items),
            _ => None,
        }
    }

    /// The entries, if this is a map.
    pub fn as_map(&self) -> Option<&BTreeMap<String, Value>> {
        match self {
            Value::Map(entries) => Some(entries),
            _ => None,
        }
    }

    /// The text of a scalar, or `None` for lists and maps.
    pub(crate) fn scalar_text(&self) -> Option<Cow<'_, str>> {
        match self {
            Value::Null => Some(Cow::Borrowed("")),
            Value::Str(text) | Value::Safe(text) => Some(Cow::Borrowed(text)),
            Value::Bool(value) => Some(Cow::Borrowed(if *value { "true" } else { "false" })),
            Value::Int(value) => Some(Cow::Owned(value.to_string())),
            Value::Float(value) => Some(Cow::Owned(value.to_string())),
            Value::List(_) | Value::Map(_) => None,
        }
    }
}

/// Prints the value as text. This is total: lists and maps print as compact
/// JSON-like text. (Templates refuse to *print* them; use `join` or `json`.)
impl fmt::Display for Value {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.scalar_text() {
            Some(text) => formatter.write_str(&text),
            None => formatter.write_str(&json::to_string(&json::Value::from(self.clone()))),
        }
    }
}

impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Value::Bool(value)
    }
}

macro_rules! from_int {
    ($($type:ty),*) => {
        $(impl From<$type> for Value {
            fn from(value: $type) -> Self {
                Value::Int(i64::from(value))
            }
        })*
    };
}

from_int!(i8, i16, i32, i64, u8, u16, u32);

impl From<isize> for Value {
    fn from(value: isize) -> Self {
        Value::Int(value as i64)
    }
}

/// Integers above `i64::MAX` become floats.
impl From<u64> for Value {
    fn from(value: u64) -> Self {
        match i64::try_from(value) {
            Ok(value) => Value::Int(value),
            Err(_) => Value::Float(value as f64),
        }
    }
}

/// Integers above `i64::MAX` become floats.
impl From<usize> for Value {
    fn from(value: usize) -> Self {
        Value::from(value as u64)
    }
}

impl From<f32> for Value {
    fn from(value: f32) -> Self {
        Value::Float(f64::from(value))
    }
}

impl From<f64> for Value {
    fn from(value: f64) -> Self {
        Value::Float(value)
    }
}

impl From<()> for Value {
    fn from((): ()) -> Self {
        Value::Null
    }
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Value::Str(value.to_owned())
    }
}

impl From<String> for Value {
    fn from(value: String) -> Self {
        Value::Str(value)
    }
}

impl From<Cow<'_, str>> for Value {
    fn from(value: Cow<'_, str>) -> Self {
        Value::Str(value.into_owned())
    }
}

/// Trusted markup: printed without escaping.
impl From<Html> for Value {
    fn from(value: Html) -> Self {
        Value::Safe(value.into_string())
    }
}

impl<T: Into<Value>> From<Vec<T>> for Value {
    fn from(items: Vec<T>) -> Self {
        Value::List(items.into_iter().map(Into::into).collect())
    }
}

impl<T: Into<Value>> From<Option<T>> for Value {
    fn from(value: Option<T>) -> Self {
        value.map_or(Value::Null, Into::into)
    }
}

impl<K: Into<String>, V: Into<Value>> From<BTreeMap<K, V>> for Value {
    fn from(map: BTreeMap<K, V>) -> Self {
        Value::Map(map.into_iter().map(|(k, v)| (k.into(), v.into())).collect())
    }
}

impl<K: Into<String>, V: Into<Value>> From<HashMap<K, V>> for Value {
    fn from(map: HashMap<K, V>) -> Self {
        Value::Map(map.into_iter().map(|(k, v)| (k.into(), v.into())).collect())
    }
}

impl From<Context> for Value {
    fn from(context: Context) -> Self {
        Value::Map(context.into_vars())
    }
}

fn json_number(number: json::Number) -> Value {
    match number.as_i64() {
        Some(value) => Value::Int(value),
        None => Value::Float(number.as_f64()),
    }
}

/// Convert an owned JSON value, moving strings instead of copying them.
///
/// `json::Value` implements `Drop`, so its variants cannot be destructured by
/// move; the contents are taken out through `&mut` instead.
fn from_json_owned(mut value: json::Value, depth: usize) -> Value {
    if depth > MAX_CONVERT_DEPTH {
        return Value::Null;
    }
    match &mut value {
        json::Value::Null => Value::Null,
        json::Value::Bool(flag) => Value::Bool(*flag),
        json::Value::Number(number) => json_number(*number),
        json::Value::String(text) => Value::Str(std::mem::take(text)),
        json::Value::Array(items) => Value::List(
            std::mem::take(items)
                .into_iter()
                .map(|item| from_json_owned(item, depth + 1))
                .collect(),
        ),
        json::Value::Object(object) => Value::Map(
            std::mem::take(object)
                .into_iter()
                .map(|(key, item)| (key, from_json_owned(item, depth + 1)))
                .collect(),
        ),
    }
}

fn from_json_ref(value: &json::Value, depth: usize) -> Value {
    if depth > MAX_CONVERT_DEPTH {
        return Value::Null;
    }
    match value {
        json::Value::Null => Value::Null,
        json::Value::Bool(flag) => Value::Bool(*flag),
        json::Value::Number(number) => json_number(*number),
        json::Value::String(text) => Value::Str(text.clone()),
        json::Value::Array(items) => Value::List(
            items
                .iter()
                .map(|item| from_json_ref(item, depth + 1))
                .collect(),
        ),
        json::Value::Object(object) => Value::Map(
            object
                .iter()
                .map(|(key, item)| (key.to_owned(), from_json_ref(item, depth + 1)))
                .collect(),
        ),
    }
}

/// JSON `null` becomes `none`, numbers that fit `i64` become integers and all
/// other numbers floats. Nesting beyond 128 levels becomes `none`.
impl From<json::Value> for Value {
    fn from(value: json::Value) -> Self {
        from_json_owned(value, 0)
    }
}

impl From<&json::Value> for Value {
    fn from(value: &json::Value) -> Self {
        from_json_ref(value, 0)
    }
}

fn to_json(value: &Value, depth: usize) -> json::Value {
    if depth > MAX_CONVERT_DEPTH {
        return json::Value::Null;
    }
    match value {
        Value::Null => json::Value::Null,
        Value::Bool(flag) => json::Value::Bool(*flag),
        Value::Int(number) => json::Value::from(*number),
        Value::Float(number) => json::Value::from(*number),
        Value::Str(text) | Value::Safe(text) => json::Value::String(text.clone()),
        Value::List(items) => {
            json::Value::Array(items.iter().map(|item| to_json(item, depth + 1)).collect())
        }
        Value::Map(entries) => json::Value::Object(
            entries
                .iter()
                .map(|(key, item)| (key.clone(), to_json(item, depth + 1)))
                .collect(),
        ),
    }
}

/// Non-finite floats become `null`; safe markup becomes a plain string.
impl From<Value> for json::Value {
    fn from(value: Value) -> Self {
        to_json(&value, 0)
    }
}
