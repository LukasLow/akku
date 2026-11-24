//! The TOML data model: [`Table`] and [`Value`].

use std::fmt;
use std::ops::Index;
use std::str::FromStr;

use super::datetime::Datetime;
use super::de::parse;
use super::error::Result;
use super::ser;

/// An insertion-ordered TOML table.
///
/// A table keeps its keys in the order they were inserted, which makes
/// serialization stable. Lookups are linear; TOML tables are small and the
/// order is part of the output.
///
/// ```
/// use akku::toml::{Table, Value};
///
/// let mut table = Table::new();
/// table.insert("port", Value::Integer(8080));
/// assert_eq!(table.get_int("port"), Some(8080));
/// assert_eq!(table["port"], Value::Integer(8080));
/// ```
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Table(pub Vec<(String, Value)>);

impl Table {
    /// Create an empty table.
    pub fn new() -> Self {
        Self(Vec::new())
    }

    /// Look up a key.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.0
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value)
    }

    /// Look up a key for mutation.
    pub fn get_mut(&mut self, key: &str) -> Option<&mut Value> {
        self.0
            .iter_mut()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value)
    }

    /// Whether the table has `key`.
    pub fn contains_key(&self, key: &str) -> bool {
        self.0.iter().any(|(name, _)| name == key)
    }

    /// Insert a value, replacing an existing key in place.
    ///
    /// Replacing keeps the position of the key, so the order of the table does
    /// not change. The previous value is returned.
    pub fn insert(&mut self, key: impl Into<String>, value: Value) -> Option<Value> {
        let key = key.into();
        for (name, current) in &mut self.0 {
            if name == &key {
                return Some(std::mem::replace(current, value));
            }
        }
        self.0.push((key, value));
        None
    }

    /// Remove a key and return its value.
    pub fn remove(&mut self, key: &str) -> Option<Value> {
        let index = self.0.iter().position(|(name, _)| name == key)?;
        Some(self.0.remove(index).1)
    }

    /// The number of entries.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the table has no entries.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Iterate over key and value pairs in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Value)> + '_ {
        self.0.iter().map(|(key, value)| (key.as_str(), value))
    }

    /// Iterate over the keys in insertion order.
    pub fn keys(&self) -> impl Iterator<Item = &str> + '_ {
        self.0.iter().map(|(key, _)| key.as_str())
    }

    /// Look up a dotted path such as `server.tls."cert.file"` or
    /// `products.0.name`.
    pub fn get_path(&self, path: &str) -> Option<&Value> {
        let segments = split_path(path)?;
        let (first, rest) = segments.split_first()?;
        let value = self.get(first)?;
        walk(value, rest)
    }

    /// Look up a dotted path for mutation.
    pub fn get_path_mut(&mut self, path: &str) -> Option<&mut Value> {
        let segments = split_path(path)?;
        let (first, rest) = segments.split_first()?;
        let value = self.get_mut(first)?;
        walk_mut(value, rest)
    }

    /// Shorthand for a string value.
    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.get(key)?.as_str()
    }

    /// Shorthand for an integer value.
    pub fn get_int(&self, key: &str) -> Option<i64> {
        self.get(key)?.as_integer()
    }

    /// Shorthand for a float value.
    pub fn get_float(&self, key: &str) -> Option<f64> {
        self.get(key)?.as_float()
    }

    /// Shorthand for a boolean value.
    pub fn get_bool(&self, key: &str) -> Option<bool> {
        self.get(key)?.as_bool()
    }

    /// Shorthand for an array value.
    pub fn get_array(&self, key: &str) -> Option<&[Value]> {
        self.get(key)?.as_array()
    }

    /// Shorthand for a table value.
    pub fn get_table(&self, key: &str) -> Option<&Table> {
        self.get(key)?.as_table()
    }

    /// Shorthand for a string value at a dotted path.
    pub fn get_str_path(&self, path: &str) -> Option<&str> {
        self.get_path(path)?.as_str()
    }

    /// Shorthand for an integer value at a dotted path.
    pub fn get_int_path(&self, path: &str) -> Option<i64> {
        self.get_path(path)?.as_integer()
    }

    /// Shorthand for a boolean value at a dotted path.
    pub fn get_bool_path(&self, path: &str) -> Option<bool> {
        self.get_path(path)?.as_bool()
    }
}

impl Index<&str> for Table {
    type Output = Value;

    fn index(&self, key: &str) -> &Value {
        self.get(key)
            .unwrap_or_else(|| panic!("table has no key {key:?}"))
    }
}

impl FromIterator<(String, Value)> for Table {
    fn from_iter<I: IntoIterator<Item = (String, Value)>>(iter: I) -> Self {
        let mut table = Self::new();
        for (key, value) in iter {
            table.insert(key, value);
        }
        table
    }
}

impl IntoIterator for Table {
    type Item = (String, Value);
    type IntoIter = std::vec::IntoIter<(String, Value)>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a> IntoIterator for &'a Table {
    type Item = (&'a str, &'a Value);
    type IntoIter = std::iter::Map<
        std::slice::Iter<'a, (String, Value)>,
        fn(&'a (String, Value)) -> (&'a str, &'a Value),
    >;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter().map(|(key, value)| (key.as_str(), value))
    }
}

impl FromStr for Table {
    type Err = super::error::Error;

    fn from_str(input: &str) -> Result<Self> {
        parse(input)
    }
}

impl fmt::Display for Table {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&ser::to_string(self))
    }
}

/// A TOML value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// A UTF-8 string.
    String(String),
    /// A signed 64-bit integer.
    Integer(i64),
    /// A 64-bit float.
    Float(f64),
    /// A boolean.
    Boolean(bool),
    /// A date, time or date-time.
    Datetime(Datetime),
    /// An array; elements may have different types.
    Array(Vec<Value>),
    /// A table.
    Table(Table),
}

impl Value {
    /// The TOML name of the value's type.
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::String(_) => "string",
            Self::Integer(_) => "integer",
            Self::Float(_) => "float",
            Self::Boolean(_) => "boolean",
            Self::Datetime(_) => "datetime",
            Self::Array(_) => "array",
            Self::Table(_) => "table",
        }
    }

    /// The value as a string slice.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            _ => None,
        }
    }

    /// The value as an integer.
    pub fn as_integer(&self) -> Option<i64> {
        match self {
            Self::Integer(value) => Some(*value),
            _ => None,
        }
    }

    /// The value as a float.
    pub fn as_float(&self) -> Option<f64> {
        match self {
            Self::Float(value) => Some(*value),
            _ => None,
        }
    }

    /// The value as a boolean.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Boolean(value) => Some(*value),
            _ => None,
        }
    }

    /// The value as a date, time or date-time.
    pub fn as_datetime(&self) -> Option<Datetime> {
        match self {
            Self::Datetime(value) => Some(*value),
            _ => None,
        }
    }

    /// The value as an array.
    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Self::Array(values) => Some(values),
            _ => None,
        }
    }

    /// The value as an array for mutation.
    pub fn as_array_mut(&mut self) -> Option<&mut Vec<Value>> {
        match self {
            Self::Array(values) => Some(values),
            _ => None,
        }
    }

    /// The value as a table.
    pub fn as_table(&self) -> Option<&Table> {
        match self {
            Self::Table(value) => Some(value),
            _ => None,
        }
    }

    /// The value as a table for mutation.
    pub fn as_table_mut(&mut self) -> Option<&mut Table> {
        match self {
            Self::Table(value) => Some(value),
            _ => None,
        }
    }

    /// Consume the value and return the table inside it.
    pub fn into_table(self) -> Option<Table> {
        match self {
            Self::Table(value) => Some(value),
            _ => None,
        }
    }

    /// Look up a key if this value is a table.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.as_table()?.get(key)
    }

    /// Look up a dotted path if this value is a table.
    pub fn get_path(&self, path: &str) -> Option<&Value> {
        self.as_table()?.get_path(path)
    }

    /// The array as a list of tables, if every element is a table.
    ///
    /// An empty array yields an empty list.
    pub fn array_of_tables(&self) -> Option<Vec<&Table>> {
        let values = self.as_array()?;
        values.iter().map(Value::as_table).collect()
    }

    /// Structural equality where `nan` equals `nan`.
    ///
    /// Deriving [`PartialEq`] keeps IEEE semantics, so `nan != nan`. Use this
    /// method when comparing a parsed document with an expected one.
    pub fn deep_eq(&self, other: &Value) -> bool {
        match (self, other) {
            (Self::Float(left), Self::Float(right)) => {
                (left.is_nan() && right.is_nan()) || left == right
            }
            (Self::Array(left), Self::Array(right)) => {
                left.len() == right.len()
                    && left
                        .iter()
                        .zip(right.iter())
                        .all(|(left, right)| left.deep_eq(right))
            }
            (Self::Table(left), Self::Table(right)) => {
                left.len() == right.len()
                    && left.iter().all(|(key, value)| {
                        right.get(key).is_some_and(|other| value.deep_eq(other))
                    })
            }
            (left, right) => left == right,
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&ser::value_to_string(self))
    }
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Self::String(value.to_string())
    }
}

impl From<String> for Value {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl From<i64> for Value {
    fn from(value: i64) -> Self {
        Self::Integer(value)
    }
}

impl From<f64> for Value {
    fn from(value: f64) -> Self {
        Self::Float(value)
    }
}

impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Self::Boolean(value)
    }
}

impl From<Datetime> for Value {
    fn from(value: Datetime) -> Self {
        Self::Datetime(value)
    }
}

impl From<Table> for Value {
    fn from(value: Table) -> Self {
        Self::Table(value)
    }
}

impl From<Vec<Value>> for Value {
    fn from(value: Vec<Value>) -> Self {
        Self::Array(value)
    }
}

fn walk<'a>(mut value: &'a Value, segments: &[String]) -> Option<&'a Value> {
    for segment in segments {
        value = match value {
            Value::Table(table) => table.get(segment)?,
            Value::Array(values) => values.get(segment.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(value)
}

fn walk_mut<'a>(mut value: &'a mut Value, segments: &[String]) -> Option<&'a mut Value> {
    for segment in segments {
        value = match value {
            Value::Table(table) => table.get_mut(segment)?,
            Value::Array(values) => values.get_mut(segment.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(value)
}

/// Split a dotted path into its segments, honouring quoted segments.
fn split_path(path: &str) -> Option<Vec<String>> {
    let mut segments = Vec::new();
    let mut chars = path.chars().peekable();
    loop {
        let quote = match chars.peek() {
            Some('"') => Some('"'),
            Some('\'') => Some('\''),
            _ => None,
        };
        let segment = if let Some(quote) = quote {
            chars.next();
            let mut segment = String::new();
            loop {
                match chars.next() {
                    Some(character) if character == quote => break,
                    Some('\\') if quote == '"' => match chars.next() {
                        Some('"') => segment.push('"'),
                        Some('\\') => segment.push('\\'),
                        Some('n') => segment.push('\n'),
                        Some('t') => segment.push('\t'),
                        _ => return None,
                    },
                    Some(character) => segment.push(character),
                    None => return None,
                }
            }
            segment
        } else {
            let mut segment = String::new();
            while let Some(&character) = chars.peek() {
                if character == '.' {
                    break;
                }
                segment.push(character);
                chars.next();
            }
            segment
        };
        if segment.is_empty() {
            return None;
        }
        segments.push(segment);
        match chars.next() {
            Some('.') => {}
            None => break,
            _ => return None,
        }
    }
    if segments.is_empty() {
        None
    } else {
        Some(segments)
    }
}
