//! JSON values: [`Value`], [`Number`] and [`Object`].
//!
//! The model is deliberately small and allocation-light: an object keeps its
//! insertion order in a `Vec` of pairs, and a deeply nested value is dropped
//! iteratively so hostile input cannot overflow the stack.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::ops::{Index, IndexMut};

use crate::json::error::{Error, ErrorKind, Result};

/// A JSON number.
///
/// Integers keep their exact value; `I64` and `U64` are never silently
/// widened to `F64` by the parser. [`Number::F64`] is only used when the
/// literal has a fraction or an exponent, or when it does not fit either
/// integer type.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Number {
    /// A negative or explicitly signed integer that fits `i64`.
    I64(i64),
    /// A non-negative integer that fits `u64`.
    U64(u64),
    /// A floating-point number.
    F64(f64),
}

impl Number {
    /// Wrap a float, rejecting `NaN` and the infinities.
    ///
    /// `-0.0` is preserved bit-for-bit, so it survives a roundtrip.
    pub fn from_f64(value: f64) -> Option<Self> {
        if value.is_finite() {
            Some(Self::F64(value))
        } else {
            None
        }
    }

    /// The value as `i64`, if it is an integer that fits.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Self::I64(value) => Some(*value),
            Self::U64(value) => i64::try_from(*value).ok(),
            Self::F64(_) => None,
        }
    }

    /// The value as `u64`, if it is a non-negative integer that fits.
    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Self::U64(value) => Some(*value),
            Self::I64(value) => u64::try_from(*value).ok(),
            Self::F64(_) => None,
        }
    }

    /// The value as `f64`.
    ///
    /// Integers are converted; precision may be lost for values beyond 2^53.
    pub fn as_f64(&self) -> f64 {
        match self {
            Self::I64(value) => *value as f64,
            Self::U64(value) => *value as f64,
            Self::F64(value) => *value,
        }
    }

    /// Whether this is the `I64` variant.
    pub fn is_i64(&self) -> bool {
        matches!(self, Self::I64(_))
    }

    /// Whether this is the `U64` variant.
    pub fn is_u64(&self) -> bool {
        matches!(self, Self::U64(_))
    }

    /// Whether this is the `F64` variant.
    pub fn is_f64(&self) -> bool {
        matches!(self, Self::F64(_))
    }
}

impl From<i64> for Number {
    fn from(value: i64) -> Self {
        Self::I64(value)
    }
}

impl From<u64> for Number {
    fn from(value: u64) -> Self {
        Self::U64(value)
    }
}

impl TryFrom<f64> for Number {
    type Error = Error;

    fn try_from(value: f64) -> Result<Self> {
        Self::from_f64(value).ok_or_else(|| Error::new(ErrorKind::NumberOutOfRange))
    }
}

impl fmt::Display for Number {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::I64(value) => fmt::Display::fmt(value, formatter),
            Self::U64(value) => fmt::Display::fmt(value, formatter),
            // `Debug` prints the shortest representation that round-trips,
            // including the `.0` of integral floats and the sign of `-0.0`.
            Self::F64(value) => fmt::Debug::fmt(value, formatter),
        }
    }
}

/// JSON `null`, `true`, `false`, a number, a string, an array or an object.
///
/// Values compare structurally. Numbers compare by variant **and** value, so
/// `I64(1)` is not equal to `U64(1)`. Use [`Value::as_i64`] when the exact
/// integer representation does not matter.
///
/// Dropping a value never recurses: nested arrays and objects are flattened
/// into an explicit worklist first.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Value {
    /// JSON `null`.
    #[default]
    Null,
    /// JSON `true` or `false`.
    Bool(bool),
    /// A JSON number.
    Number(Number),
    /// A JSON string.
    String(String),
    /// A JSON array.
    Array(Vec<Value>),
    /// A JSON object that preserves insertion order.
    Object(Object),
}

impl Drop for Value {
    fn drop(&mut self) {
        // A recursive drop would overflow the stack on deeply nested input.
        // Take the children out and unlink them level by level instead.
        let mut worklist: Vec<Value> = match self {
            Self::Array(items) => std::mem::take(items),
            Self::Object(object) => std::mem::take(&mut object.0)
                .into_iter()
                .map(|(_, value)| value)
                .collect(),
            _ => return,
        };
        while let Some(mut value) = worklist.pop() {
            match &mut value {
                Self::Array(items) => worklist.append(items),
                Self::Object(object) => {
                    for (_, child) in object.0.drain(..) {
                        worklist.push(child);
                    }
                }
                _ => {}
            }
        }
    }
}

impl Value {
    /// The JSON type name, as used in [`ErrorKind::TypeMismatch`].
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Null => "null",
            Self::Bool(_) => "boolean",
            Self::Number(_) => "number",
            Self::String(_) => "string",
            Self::Array(_) => "array",
            Self::Object(_) => "object",
        }
    }

    /// Whether this is `null`.
    pub fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }

    /// Whether this is a boolean.
    pub fn is_bool(&self) -> bool {
        matches!(self, Self::Bool(_))
    }

    /// Whether this is a number.
    pub fn is_number(&self) -> bool {
        matches!(self, Self::Number(_))
    }

    /// Whether this is a string.
    pub fn is_string(&self) -> bool {
        matches!(self, Self::String(_))
    }

    /// Whether this is an array.
    pub fn is_array(&self) -> bool {
        matches!(self, Self::Array(_))
    }

    /// Whether this is an object.
    pub fn is_object(&self) -> bool {
        matches!(self, Self::Object(_))
    }

    /// The boolean, if this is one.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            _ => None,
        }
    }

    /// The number as `i64`, if it is an integer in range.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Number(number) => number.as_i64(),
            _ => None,
        }
    }

    /// The number as `u64`, if it is a non-negative integer in range.
    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Self::Number(number) => number.as_u64(),
            _ => None,
        }
    }

    /// The number as `f64`, if this is a number.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Number(number) => Some(number.as_f64()),
            _ => None,
        }
    }

    /// The number, if this is one.
    pub fn as_number(&self) -> Option<Number> {
        match self {
            Self::Number(number) => Some(*number),
            _ => None,
        }
    }

    /// The string, if this is one.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            _ => None,
        }
    }

    /// The array, if this is one.
    pub fn as_array(&self) -> Option<&Vec<Value>> {
        match self {
            Self::Array(items) => Some(items),
            _ => None,
        }
    }

    /// The array mutably, if this is one.
    pub fn as_array_mut(&mut self) -> Option<&mut Vec<Value>> {
        match self {
            Self::Array(items) => Some(items),
            _ => None,
        }
    }

    /// The object, if this is one.
    pub fn as_object(&self) -> Option<&Object> {
        match self {
            Self::Object(object) => Some(object),
            _ => None,
        }
    }

    /// The object mutably, if this is one.
    pub fn as_object_mut(&mut self) -> Option<&mut Object> {
        match self {
            Self::Object(object) => Some(object),
            _ => None,
        }
    }

    /// Consume the value and return its array, if it is one.
    pub fn into_array(mut self) -> Option<Vec<Value>> {
        match &mut self {
            Self::Array(items) => Some(std::mem::take(items)),
            _ => None,
        }
    }

    /// Consume the value and return its object, if it is one.
    pub fn into_object(mut self) -> Option<Object> {
        match &mut self {
            Self::Object(object) => Some(std::mem::take(object)),
            _ => None,
        }
    }

    /// Consume the value and return its string, if it is one.
    pub fn into_string(mut self) -> Option<String> {
        match &mut self {
            Self::String(value) => Some(std::mem::take(value)),
            _ => None,
        }
    }

    /// Consume the value and return its number, if it is one.
    pub fn into_number(self) -> Option<Number> {
        self.as_number()
    }

    /// Replace the value with `null` and return the previous value.
    pub fn take(&mut self) -> Value {
        std::mem::take(self)
    }

    /// Look up an object key.
    ///
    /// Returns `None` for every non-object value.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.as_object()?.get(key)
    }

    /// Look up an object key mutably.
    pub fn get_mut(&mut self, key: &str) -> Option<&mut Value> {
        self.as_object_mut()?.get_mut(key)
    }

    /// Look up an array index.
    pub fn get_index(&self, index: usize) -> Option<&Value> {
        self.as_array()?.get(index)
    }

    /// Look up an array index mutably.
    pub fn get_index_mut(&mut self, index: usize) -> Option<&mut Value> {
        self.as_array_mut()?.get_mut(index)
    }

    /// Look up a dotted path such as `a.b.0`.
    ///
    /// A dot separates segments; `\.` is a literal dot inside a key and `\\`
    /// a literal backslash. Numeric segments index into arrays. An empty path
    /// returns the value itself.
    ///
    /// ```
    /// use akku::json::parse;
    ///
    /// let value = parse(r#"{"a": {"b": [1, 2]}}"#).unwrap();
    /// assert_eq!(value.get_path("a.b.1").and_then(|v| v.as_i64()), Some(2));
    /// ```
    pub fn get_path(&self, path: &str) -> Option<&Value> {
        if path.is_empty() {
            return Some(self);
        }
        let mut current = self;
        for segment in split_path(path) {
            current = match current {
                Self::Object(object) => object.get(&segment)?,
                Self::Array(items) => items.get(segment.parse::<usize>().ok()?)?,
                _ => return None,
            };
        }
        Some(current)
    }

    /// Look up a dotted path mutably, see [`Value::get_path`].
    pub fn get_path_mut(&mut self, path: &str) -> Option<&mut Value> {
        if path.is_empty() {
            return Some(self);
        }
        let segments = split_path(path);
        let mut current: Option<&mut Value> = Some(self);
        for segment in &segments {
            let slot = current.take()?;
            current = Some(match slot {
                Self::Object(object) => object.get_mut(segment)?,
                Self::Array(items) => items.get_mut(segment.parse::<usize>().ok()?)?,
                _ => return None,
            });
        }
        current
    }

    /// Look up an [RFC 6901](https://www.rfc-editor.org/rfc/rfc6901) JSON
    /// Pointer such as `/users/0/name`.
    ///
    /// `~1` decodes to `/` and `~0` to `~`. The empty pointer returns the
    /// value itself. Array indices are read with plain decimal parsing, so
    /// leading zeros are tolerated even though RFC 6901 forbids them.
    ///
    /// ```
    /// use akku::json::parse;
    ///
    /// let value = parse(r#"{"a/b": {"c~d": 1}}"#).unwrap();
    /// assert_eq!(value.pointer("/a~1b/c~0d").and_then(|v| v.as_i64()), Some(1));
    /// ```
    pub fn pointer(&self, pointer: &str) -> Option<&Value> {
        let mut current = self;
        for segment in pointer_segments(pointer)? {
            current = match current {
                Self::Object(object) => object.get(&segment)?,
                Self::Array(items) => items.get(segment.parse::<usize>().ok()?)?,
                _ => return None,
            };
        }
        Some(current)
    }

    /// Look up an RFC 6901 JSON Pointer mutably, see [`Value::pointer`].
    pub fn pointer_mut(&mut self, pointer: &str) -> Option<&mut Value> {
        let segments = pointer_segments(pointer)?;
        let mut current: Option<&mut Value> = Some(self);
        for segment in &segments {
            let slot = current.take()?;
            current = Some(match slot {
                Self::Object(object) => object.get_mut(segment)?,
                Self::Array(items) => items.get_mut(segment.parse::<usize>().ok()?)?,
                _ => return None,
            });
        }
        current
    }

    /// Insert into an object, returning the replaced value.
    ///
    /// A `null` value becomes an empty object first. Any other non-object
    /// value is left untouched and `None` is returned.
    pub fn insert(&mut self, key: impl Into<String>, value: Value) -> Option<Value> {
        match self {
            Self::Object(object) => object.insert(key, value),
            Self::Null => {
                let mut object = Object::new();
                let previous = object.insert(key, value);
                *self = Self::Object(object);
                previous
            }
            _ => None,
        }
    }

    /// Append to an array.
    ///
    /// A `null` value becomes an array first. Any other non-array value is
    /// left untouched.
    pub fn push(&mut self, value: Value) {
        match self {
            Self::Array(items) => items.push(value),
            Self::Null => *self = Self::Array(vec![value]),
            _ => {}
        }
    }

    /// Remove an object key, returning its value.
    pub fn remove(&mut self, key: &str) -> Option<Value> {
        self.as_object_mut()?.remove(key)
    }
}

/// Split a dotted path into segments, honouring `\.` and `\\`.
fn split_path(path: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut characters = path.chars();
    while let Some(character) = characters.next() {
        match character {
            '\\' => match characters.next() {
                Some(escaped) => current.push(escaped),
                None => current.push('\\'),
            },
            '.' => segments.push(std::mem::take(&mut current)),
            _ => current.push(character),
        }
    }
    segments.push(current);
    segments
}

/// Decode an RFC 6901 pointer into path segments.
fn pointer_segments(pointer: &str) -> Option<Vec<String>> {
    if pointer.is_empty() {
        return Some(Vec::new());
    }
    let rest = pointer.strip_prefix('/')?;
    let mut segments = Vec::new();
    for raw in rest.split('/') {
        let mut decoded = String::with_capacity(raw.len());
        let mut characters = raw.chars();
        while let Some(character) = characters.next() {
            if character == '~' {
                match characters.next() {
                    Some('0') => decoded.push('~'),
                    Some('1') => decoded.push('/'),
                    _ => return None,
                }
            } else {
                decoded.push(character);
            }
        }
        segments.push(decoded);
    }
    Some(segments)
}

impl Index<&str> for Value {
    type Output = Value;

    /// # Panics
    ///
    /// Panics if the value is not an object or the key is missing.
    fn index(&self, key: &str) -> &Value {
        self.get(key).expect("no entry found for key")
    }
}

impl IndexMut<&str> for Value {
    /// # Panics
    ///
    /// Panics if the value is not an object or the key is missing.
    fn index_mut(&mut self, key: &str) -> &mut Value {
        self.get_mut(key).expect("no entry found for key")
    }
}

impl Index<usize> for Value {
    type Output = Value;

    /// # Panics
    ///
    /// Panics if the value is not an array or the index is out of bounds.
    fn index(&self, index: usize) -> &Value {
        self.get_index(index).expect("index out of bounds")
    }
}

impl IndexMut<usize> for Value {
    /// # Panics
    ///
    /// Panics if the value is not an array or the index is out of bounds.
    fn index_mut(&mut self, index: usize) -> &mut Value {
        self.get_index_mut(index).expect("index out of bounds")
    }
}

impl PartialEq<bool> for Value {
    fn eq(&self, other: &bool) -> bool {
        self.as_bool() == Some(*other)
    }
}

impl PartialEq<Value> for bool {
    fn eq(&self, other: &Value) -> bool {
        other.as_bool() == Some(*self)
    }
}

impl PartialEq<i64> for Value {
    fn eq(&self, other: &i64) -> bool {
        self.as_i64() == Some(*other)
    }
}

impl PartialEq<Value> for i64 {
    fn eq(&self, other: &Value) -> bool {
        other.as_i64() == Some(*self)
    }
}

impl PartialEq<u64> for Value {
    fn eq(&self, other: &u64) -> bool {
        self.as_u64() == Some(*other)
    }
}

impl PartialEq<Value> for u64 {
    fn eq(&self, other: &Value) -> bool {
        other.as_u64() == Some(*self)
    }
}

impl PartialEq<&str> for Value {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == Some(*other)
    }
}

impl PartialEq<str> for Value {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == Some(other)
    }
}

impl PartialEq<Value> for &str {
    fn eq(&self, other: &Value) -> bool {
        other.as_str() == Some(*self)
    }
}

impl PartialEq<Value> for str {
    fn eq(&self, other: &Value) -> bool {
        other.as_str() == Some(self)
    }
}

impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

/// Create a number value from a signed integer.
///
/// Non-negative values are stored as [`Number::U64`] so that a value built in
/// Rust compares equal to the same literal parsed from text.
impl From<i64> for Value {
    fn from(value: i64) -> Self {
        Self::Number(if value >= 0 {
            Number::U64(value as u64)
        } else {
            Number::I64(value)
        })
    }
}

/// Create a number value from an unsigned integer.
impl From<u64> for Value {
    fn from(value: u64) -> Self {
        Self::Number(Number::U64(value))
    }
}

/// Create a number value from a float.
///
/// `NaN` and the infinities have no JSON representation and become
/// `Value::Null`.
impl From<f64> for Value {
    fn from(value: f64) -> Self {
        match Number::from_f64(value) {
            Some(number) => Self::Number(number),
            None => Self::Null,
        }
    }
}

/// Create a number value from a float, `null` when it is not finite.
impl From<f32> for Value {
    fn from(value: f32) -> Self {
        Self::from(f64::from(value))
    }
}

macro_rules! impl_from_signed {
    ($($type:ty),* $(,)?) => {
        $(
            /// Create a number value from a signed integer.
            impl From<$type> for Value {
                fn from(value: $type) -> Self {
                    Self::from(i64::from(value))
                }
            }
        )*
    };
}

macro_rules! impl_from_unsigned {
    ($($type:ty),* $(,)?) => {
        $(
            /// Create a number value from an unsigned integer.
            impl From<$type> for Value {
                fn from(value: $type) -> Self {
                    Self::from(u64::from(value))
                }
            }
        )*
    };
}

impl_from_signed!(i8, i16, i32);
impl_from_unsigned!(u8, u16, u32);

/// Create a number value from `isize`.
impl From<isize> for Value {
    fn from(value: isize) -> Self {
        Self::from(value as i64)
    }
}

/// Create a number value from `usize`.
impl From<usize> for Value {
    fn from(value: usize) -> Self {
        Self::from(value as u64)
    }
}

/// Create a string value.
impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Self::String(value.to_owned())
    }
}

/// Create a string value.
impl From<String> for Value {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

/// Create a string value.
impl From<std::borrow::Cow<'_, str>> for Value {
    fn from(value: std::borrow::Cow<'_, str>) -> Self {
        Self::String(value.into_owned())
    }
}

/// Create a number value.
impl From<Number> for Value {
    fn from(value: Number) -> Self {
        Self::Number(value)
    }
}

/// Create an object value.
impl From<Object> for Value {
    fn from(value: Object) -> Self {
        Self::Object(value)
    }
}

/// Create an array value.
impl<T: Into<Value>> From<Vec<T>> for Value {
    fn from(items: Vec<T>) -> Self {
        Self::Array(items.into_iter().map(Into::into).collect())
    }
}

/// Create `null` from the unit type.
impl From<()> for Value {
    fn from((): ()) -> Self {
        Self::Null
    }
}

/// Create `null` from `None` and the inner value from `Some`.
impl<T: Into<Value>> From<Option<T>> for Value {
    fn from(value: Option<T>) -> Self {
        match value {
            Some(value) => value.into(),
            None => Self::Null,
        }
    }
}

/// Create an object value from a sorted map.
impl<K, V> From<BTreeMap<K, V>> for Value
where
    K: Into<String>,
    V: Into<Value>,
{
    fn from(map: BTreeMap<K, V>) -> Self {
        Self::Object(map.into_iter().collect())
    }
}

/// Create an object value from a hash map.
///
/// The iteration order of a `HashMap` is unspecified, so the resulting object
/// keeps an arbitrary order.
impl<K, V> From<HashMap<K, V>> for Value
where
    K: Into<String>,
    V: Into<Value>,
{
    fn from(map: HashMap<K, V>) -> Self {
        Self::Object(map.into_iter().collect())
    }
}

/// A JSON object that preserves insertion order.
///
/// Equality is **order-insensitive**: two objects are equal when they have
/// the same length and every key maps to an equal value. Inserting an existing
/// key replaces the value and keeps the original position.
#[derive(Debug, Clone, Default)]
pub struct Object(Vec<(String, Value)>);

impl Object {
    /// Create an empty object.
    pub fn new() -> Self {
        Self(Vec::new())
    }

    /// Create an empty object with room for `capacity` entries.
    pub fn with_capacity(capacity: usize) -> Self {
        Self(Vec::with_capacity(capacity))
    }

    /// The number of entries.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the object has no entries.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Look up a key.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.0
            .iter()
            .find(|(existing, _)| existing == key)
            .map(|(_, value)| value)
    }

    /// Look up a key mutably.
    pub fn get_mut(&mut self, key: &str) -> Option<&mut Value> {
        self.0
            .iter_mut()
            .find(|(existing, _)| existing == key)
            .map(|(_, value)| value)
    }

    /// Whether the key exists.
    pub fn contains_key(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    /// Insert a key, returning the replaced value.
    ///
    /// An existing key keeps its position, only the value is replaced.
    pub fn insert(&mut self, key: impl Into<String>, value: Value) -> Option<Value> {
        let key = key.into();
        match self.0.iter_mut().find(|(existing, _)| *existing == key) {
            Some((_, slot)) => Some(std::mem::replace(slot, value)),
            None => {
                self.0.push((key, value));
                None
            }
        }
    }

    /// Remove a key, returning its value.
    pub fn remove(&mut self, key: &str) -> Option<Value> {
        let index = self.0.iter().position(|(existing, _)| existing == key)?;
        Some(self.0.remove(index).1)
    }

    /// Remove all entries.
    pub fn clear(&mut self) {
        self.0.clear();
    }

    /// Iterate over key/value pairs in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Value)> + '_ {
        self.0.iter().map(|(key, value)| (key.as_str(), value))
    }

    /// Iterate over key/value pairs mutably, in insertion order.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&str, &mut Value)> + '_ {
        self.0.iter_mut().map(|(key, value)| (key.as_str(), value))
    }

    /// Iterate over the keys in insertion order.
    pub fn keys(&self) -> impl Iterator<Item = &str> + '_ {
        self.0.iter().map(|(key, _)| key.as_str())
    }

    /// Iterate over the values in insertion order.
    pub fn values(&self) -> impl Iterator<Item = &Value> + '_ {
        self.0.iter().map(|(_, value)| value)
    }

    /// Sort the entries by key.
    pub fn sort_keys(&mut self) {
        self.0.sort_by(|left, right| left.0.cmp(&right.0));
    }

    /// Get the entry for `key`, for in-place inserts and updates.
    pub fn entry(&mut self, key: impl Into<String>) -> Entry<'_> {
        let key = key.into();
        match self.0.iter().position(|(existing, _)| *existing == key) {
            Some(index) => Entry::Occupied(OccupiedEntry {
                map: &mut self.0,
                index,
            }),
            None => Entry::Vacant(VacantEntry {
                map: &mut self.0,
                key,
            }),
        }
    }

    /// Borrow the entries as a slice of pairs.
    pub(crate) fn as_slice(&self) -> &[(String, Value)] {
        &self.0
    }
}

fn pair_ref((key, value): &(String, Value)) -> (&str, &Value) {
    (key.as_str(), value)
}

fn pair_mut((key, value): &mut (String, Value)) -> (&str, &mut Value) {
    (key.as_str(), value)
}

impl PartialEq for Object {
    fn eq(&self, other: &Self) -> bool {
        if self.len() != other.len() {
            return false;
        }
        self.0
            .iter()
            .all(|(key, value)| other.get(key).is_some_and(|other| other == value))
    }
}

impl<K, V> FromIterator<(K, V)> for Object
where
    K: Into<String>,
    V: Into<Value>,
{
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        let mut object = Self::new();
        object.extend(iter);
        object
    }
}

impl<K, V> Extend<(K, V)> for Object
where
    K: Into<String>,
    V: Into<Value>,
{
    fn extend<I: IntoIterator<Item = (K, V)>>(&mut self, iter: I) {
        for (key, value) in iter {
            self.insert(key, value.into());
        }
    }
}

impl IntoIterator for Object {
    type Item = (String, Value);
    type IntoIter = std::vec::IntoIter<(String, Value)>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a> IntoIterator for &'a Object {
    type Item = (&'a str, &'a Value);
    type IntoIter = std::iter::Map<
        std::slice::Iter<'a, (String, Value)>,
        fn(&'a (String, Value)) -> (&'a str, &'a Value),
    >;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter().map(pair_ref as fn(_) -> _)
    }
}

impl<'a> IntoIterator for &'a mut Object {
    type Item = (&'a str, &'a mut Value);
    type IntoIter = std::iter::Map<
        std::slice::IterMut<'a, (String, Value)>,
        fn(&'a mut (String, Value)) -> (&'a str, &'a mut Value),
    >;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter_mut().map(pair_mut as fn(_) -> _)
    }
}

impl Index<&str> for Object {
    type Output = Value;

    /// # Panics
    ///
    /// Panics if the key is missing.
    fn index(&self, key: &str) -> &Value {
        self.get(key).expect("no entry found for key")
    }
}

impl IndexMut<&str> for Object {
    /// # Panics
    ///
    /// Panics if the key is missing.
    fn index_mut(&mut self, key: &str) -> &mut Value {
        self.get_mut(key).expect("no entry found for key")
    }
}

/// A view into one entry of an [`Object`], returned by [`Object::entry`].
pub enum Entry<'a> {
    /// The key exists.
    Occupied(OccupiedEntry<'a>),
    /// The key does not exist.
    Vacant(VacantEntry<'a>),
}

impl<'a> Entry<'a> {
    /// The key of this entry.
    pub fn key(&self) -> &str {
        match self {
            Self::Occupied(entry) => entry.key(),
            Self::Vacant(entry) => entry.key(),
        }
    }

    /// The value, inserting `default` when the key is missing.
    pub fn or_insert(self, default: Value) -> &'a mut Value {
        match self {
            Self::Occupied(entry) => entry.into_mut(),
            Self::Vacant(entry) => entry.insert(default),
        }
    }

    /// The value, inserting the result of `default` when the key is missing.
    pub fn or_insert_with(self, default: impl FnOnce() -> Value) -> &'a mut Value {
        match self {
            Self::Occupied(entry) => entry.into_mut(),
            Self::Vacant(entry) => entry.insert(default()),
        }
    }

    /// The value, inserting a value derived from the key when it is missing.
    pub fn or_insert_with_key(self, default: impl FnOnce(&str) -> Value) -> &'a mut Value {
        match self {
            Self::Occupied(entry) => entry.into_mut(),
            Self::Vacant(entry) => {
                let value = default(&entry.key);
                entry.insert(value)
            }
        }
    }
}

/// An [`Entry`] whose key exists in the object.
pub struct OccupiedEntry<'a> {
    map: &'a mut Vec<(String, Value)>,
    index: usize,
}

impl<'a> OccupiedEntry<'a> {
    /// The key of this entry.
    pub fn key(&self) -> &str {
        &self.map[self.index].0
    }

    /// The value of this entry.
    pub fn get(&self) -> &Value {
        &self.map[self.index].1
    }

    /// The value of this entry mutably.
    pub fn get_mut(&mut self) -> &mut Value {
        &mut self.map[self.index].1
    }

    /// Consume the entry and borrow the value.
    pub fn into_mut(self) -> &'a mut Value {
        let map = self.map;
        &mut map[self.index].1
    }

    /// Replace the value, returning the old one.
    pub fn insert(&mut self, value: Value) -> Value {
        std::mem::replace(&mut self.map[self.index].1, value)
    }

    /// Remove the entry, returning its value.
    pub fn remove(self) -> Value {
        self.map.remove(self.index).1
    }
}

/// An [`Entry`] whose key does not exist in the object.
pub struct VacantEntry<'a> {
    map: &'a mut Vec<(String, Value)>,
    key: String,
}

impl<'a> VacantEntry<'a> {
    /// The key that is missing.
    pub fn key(&self) -> &str {
        &self.key
    }

    /// Insert a value for the key and borrow it.
    pub fn insert(self, value: Value) -> &'a mut Value {
        let Self { map, key } = self;
        map.push((key, value));
        &mut map.last_mut().expect("the entry was just pushed").1
    }

    /// Consume the entry and return the missing key.
    pub fn into_key(self) -> String {
        self.key
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_insert_keeps_position_and_replaces_value() {
        let mut object = Object::new();
        object.insert("a", Value::from(1));
        object.insert("b", Value::from(2));
        object.insert("a", Value::from(3));
        assert_eq!(object.len(), 2);
        assert_eq!(object.keys().collect::<Vec<_>>(), vec!["a", "b"]);
        assert_eq!(object["a"], 3i64);
    }

    #[test]
    fn object_equality_ignores_order() {
        let mut left = Object::new();
        left.insert("a", Value::from(1));
        left.insert("b", Value::from(2));

        let mut right = Object::new();
        right.insert("b", Value::from(2));
        right.insert("a", Value::from(1));

        assert_eq!(left, right);
    }

    #[test]
    fn deep_value_drops_without_recursion() {
        let mut value = Value::Null;
        for _ in 0..50_000 {
            value = Value::Array(vec![value]);
        }
        drop(value);
    }
}
