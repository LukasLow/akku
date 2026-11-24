//! Conversions between Rust types and [`Value`].
//!
//! [`ToJson`] builds a value from a Rust value, [`FromJson`] reads a Rust
//! value back. Both are deliberately strict about JSON types: an integer
//! conversion never accepts a float, so data loss is always visible.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};

use crate::json::error::{Error, ErrorKind, Result};
use crate::json::value::{Number, Object, Value};

/// Build a [`Value`] from a Rust value.
///
/// Floats that are not finite (`NaN`, infinities) have no JSON
/// representation; their conversion yields [`Value::Null`] rather than an
/// invalid number.
///
/// ```
/// use akku::json::ToJson;
///
/// assert_eq!(1i32.to_json().as_i64(), Some(1));
/// assert_eq!(f64::NAN.to_json(), akku::json::Value::Null);
/// ```
pub trait ToJson {
    /// Convert `self` into a JSON value.
    ///
    /// # Panics
    ///
    /// Implementations in this crate never panic.
    fn to_json(&self) -> Value;
}

/// Read a Rust value out of a [`Value`].
///
/// Conversions are strict: integer types accept only integer numbers, so
/// `1.5` fails with [`ErrorKind::TypeMismatch`]. Float types accept any
/// number. Values out of the target's range fail with
/// [`ErrorKind::NumberOutOfRange`].
///
/// ```
/// use akku::json::{parse, FromJson};
///
/// let value = parse("[1, 2, 3]").unwrap();
/// assert_eq!(Vec::<u8>::from_json(&value).unwrap(), vec![1, 2, 3]);
/// ```
pub trait FromJson: Sized {
    /// Convert `value` into `Self`.
    fn from_json(value: &Value) -> Result<Self>;
}

/// Build the standard type mismatch error for `expected` versus `value`.
fn mismatch(expected: &'static str, value: &Value) -> Error {
    Error::new(ErrorKind::TypeMismatch {
        expected,
        found: value.type_name(),
    })
}

/// Read an integer, rejecting floats.
fn integer_from_json(value: &Value) -> Result<Number> {
    match value.as_number() {
        Some(number) if !number.is_f64() => Ok(number),
        _ => Err(mismatch("an integer", value)),
    }
}

impl ToJson for Value {
    fn to_json(&self) -> Value {
        self.clone()
    }
}

impl<T: ToJson + ?Sized> ToJson for &T {
    fn to_json(&self) -> Value {
        (*self).to_json()
    }
}

impl ToJson for str {
    fn to_json(&self) -> Value {
        Value::String(self.to_owned())
    }
}

impl ToJson for String {
    fn to_json(&self) -> Value {
        Value::String(self.clone())
    }
}

impl ToJson for Cow<'_, str> {
    fn to_json(&self) -> Value {
        Value::String(self.clone().into_owned())
    }
}

impl ToJson for Object {
    fn to_json(&self) -> Value {
        Value::Object(self.clone())
    }
}

impl ToJson for Number {
    fn to_json(&self) -> Value {
        Value::Number(*self)
    }
}

/// Booleans become `true`/`false`.
impl ToJson for bool {
    fn to_json(&self) -> Value {
        Value::Bool(*self)
    }
}

/// The unit type becomes `null`.
impl ToJson for () {
    fn to_json(&self) -> Value {
        Value::Null
    }
}

impl<T: ToJson> ToJson for Option<T> {
    fn to_json(&self) -> Value {
        match self {
            Some(value) => value.to_json(),
            None => Value::Null,
        }
    }
}

impl<T: ToJson> ToJson for Vec<T> {
    fn to_json(&self) -> Value {
        Value::Array(self.iter().map(ToJson::to_json).collect())
    }
}

impl<T: ToJson> ToJson for [T] {
    fn to_json(&self) -> Value {
        Value::Array(self.iter().map(ToJson::to_json).collect())
    }
}

impl<T: ToJson, const N: usize> ToJson for [T; N] {
    fn to_json(&self) -> Value {
        Value::Array(self.iter().map(ToJson::to_json).collect())
    }
}

impl<V: ToJson> ToJson for BTreeMap<String, V> {
    fn to_json(&self) -> Value {
        Value::Object(
            self.iter()
                .map(|(key, value)| (key.clone(), value.to_json()))
                .collect(),
        )
    }
}

impl<V: ToJson> ToJson for HashMap<String, V> {
    fn to_json(&self) -> Value {
        Value::Object(
            self.iter()
                .map(|(key, value)| (key.clone(), value.to_json()))
                .collect(),
        )
    }
}

macro_rules! impl_to_json_integer {
    ($($type:ty),* $(,)?) => {
        $(
            /// Integers become exact number values.
            impl ToJson for $type {
                fn to_json(&self) -> Value {
                    Value::from(*self)
                }
            }
        )*
    };
}

impl_to_json_integer!(i8, i16, i32, i64, isize, u8, u16, u32, u64, usize);

macro_rules! impl_to_json_float {
    ($($type:ty),* $(,)?) => {
        $(
            /// Finite floats become number values, non-finite ones `null`.
            impl ToJson for $type {
                fn to_json(&self) -> Value {
                    Value::from(*self)
                }
            }
        )*
    };
}

impl_to_json_float!(f32, f64);

impl<A: ToJson, B: ToJson> ToJson for (A, B) {
    fn to_json(&self) -> Value {
        Value::Array(vec![self.0.to_json(), self.1.to_json()])
    }
}

impl<A: ToJson, B: ToJson, C: ToJson> ToJson for (A, B, C) {
    fn to_json(&self) -> Value {
        Value::Array(vec![self.0.to_json(), self.1.to_json(), self.2.to_json()])
    }
}

impl<A: ToJson, B: ToJson, C: ToJson, D: ToJson> ToJson for (A, B, C, D) {
    fn to_json(&self) -> Value {
        Value::Array(vec![
            self.0.to_json(),
            self.1.to_json(),
            self.2.to_json(),
            self.3.to_json(),
        ])
    }
}

impl<A: ToJson, B: ToJson, C: ToJson, D: ToJson, E: ToJson> ToJson for (A, B, C, D, E) {
    fn to_json(&self) -> Value {
        Value::Array(vec![
            self.0.to_json(),
            self.1.to_json(),
            self.2.to_json(),
            self.3.to_json(),
            self.4.to_json(),
        ])
    }
}

impl<A: ToJson, B: ToJson, C: ToJson, D: ToJson, E: ToJson, F: ToJson> ToJson
    for (A, B, C, D, E, F)
{
    fn to_json(&self) -> Value {
        Value::Array(vec![
            self.0.to_json(),
            self.1.to_json(),
            self.2.to_json(),
            self.3.to_json(),
            self.4.to_json(),
            self.5.to_json(),
        ])
    }
}

impl FromJson for Value {
    fn from_json(value: &Value) -> Result<Self> {
        Ok(value.clone())
    }
}

impl FromJson for Object {
    fn from_json(value: &Value) -> Result<Self> {
        match value {
            Value::Object(object) => Ok(object.clone()),
            other => Err(mismatch("an object", other)),
        }
    }
}

impl FromJson for Number {
    fn from_json(value: &Value) -> Result<Self> {
        value.as_number().ok_or_else(|| mismatch("a number", value))
    }
}

/// Only `true` and `false` convert to a boolean.
impl FromJson for bool {
    fn from_json(value: &Value) -> Result<Self> {
        match value {
            Value::Bool(boolean) => Ok(*boolean),
            other => Err(mismatch("a boolean", other)),
        }
    }
}

/// Only a JSON string converts to a `String`.
impl FromJson for String {
    fn from_json(value: &Value) -> Result<Self> {
        match value {
            Value::String(text) => Ok(text.clone()),
            other => Err(mismatch("a string", other)),
        }
    }
}

/// Only `null` converts to the unit type.
impl FromJson for () {
    fn from_json(value: &Value) -> Result<Self> {
        match value {
            Value::Null => Ok(()),
            other => Err(mismatch("null", other)),
        }
    }
}

impl<T: FromJson> FromJson for Option<T> {
    fn from_json(value: &Value) -> Result<Self> {
        match value {
            Value::Null => Ok(None),
            other => Ok(Some(T::from_json(other)?)),
        }
    }
}

impl<T: FromJson> FromJson for Vec<T> {
    fn from_json(value: &Value) -> Result<Self> {
        match value {
            Value::Array(items) => items.iter().map(T::from_json).collect(),
            other => Err(mismatch("an array", other)),
        }
    }
}

impl<V: FromJson> FromJson for BTreeMap<String, V> {
    fn from_json(value: &Value) -> Result<Self> {
        match value {
            Value::Object(object) => object
                .iter()
                .map(|(key, value)| Ok((key.to_owned(), V::from_json(value)?)))
                .collect(),
            other => Err(mismatch("an object", other)),
        }
    }
}

impl<V: FromJson> FromJson for HashMap<String, V> {
    fn from_json(value: &Value) -> Result<Self> {
        match value {
            Value::Object(object) => object
                .iter()
                .map(|(key, value)| Ok((key.to_owned(), V::from_json(value)?)))
                .collect(),
            other => Err(mismatch("an object", other)),
        }
    }
}

macro_rules! impl_from_json_signed {
    ($($type:ty),* $(,)?) => {
        $(
            /// Convert a JSON integer, rejecting floats and out-of-range values.
            impl FromJson for $type {
                fn from_json(value: &Value) -> Result<Self> {
                    let number = integer_from_json(value)?;
                    let integer = number
                        .as_i64()
                        .ok_or_else(|| Error::new(ErrorKind::NumberOutOfRange))?;
                    <$type>::try_from(integer).map_err(|_| Error::new(ErrorKind::NumberOutOfRange))
                }
            }
        )*
    };
}

macro_rules! impl_from_json_unsigned {
    ($($type:ty),* $(,)?) => {
        $(
            /// Convert a JSON integer, rejecting floats, negatives and
            /// out-of-range values.
            impl FromJson for $type {
                fn from_json(value: &Value) -> Result<Self> {
                    let number = integer_from_json(value)?;
                    let integer = number
                        .as_u64()
                        .ok_or_else(|| Error::new(ErrorKind::NumberOutOfRange))?;
                    <$type>::try_from(integer).map_err(|_| Error::new(ErrorKind::NumberOutOfRange))
                }
            }
        )*
    };
}

impl_from_json_signed!(i8, i16, i32, i64, isize);
impl_from_json_unsigned!(u8, u16, u32, u64, usize);

/// Convert any JSON number to an `f64`.
impl FromJson for f64 {
    fn from_json(value: &Value) -> Result<Self> {
        match value.as_number() {
            Some(number) => Ok(number.as_f64()),
            None => Err(mismatch("a number", value)),
        }
    }
}

/// Convert any JSON number to an `f32`, failing when it does not fit.
impl FromJson for f32 {
    fn from_json(value: &Value) -> Result<Self> {
        let number = <f64 as FromJson>::from_json(value)?;
        let narrowed = number as f32;
        if narrowed.is_finite() {
            Ok(narrowed)
        } else {
            Err(Error::new(ErrorKind::NumberOutOfRange))
        }
    }
}

macro_rules! impl_from_json_tuple {
    ($length:literal, $($name:ident : $index:tt),+ $(,)?) => {
        impl<$($name: FromJson),+> FromJson for ($($name,)+) {
            fn from_json(value: &Value) -> Result<Self> {
                let items = match value {
                    Value::Array(items) => items,
                    other => return Err(mismatch("an array", other)),
                };
                if items.len() != $length {
                    return Err(Error::new(ErrorKind::TypeMismatch {
                        expected: concat!("an array of ", $length),
                        found: "an array of a different length",
                    }));
                }
                Ok(($($name::from_json(&items[$index])?,)+))
            }
        }
    };
}

impl_from_json_tuple!(2, A: 0, B: 1);
impl_from_json_tuple!(3, A: 0, B: 1, C: 2);
impl_from_json_tuple!(4, A: 0, B: 1, C: 2, D: 3);
impl_from_json_tuple!(5, A: 0, B: 1, C: 2, D: 3, E: 4);
impl_from_json_tuple!(6, A: 0, B: 1, C: 2, D: 3, E: 4, F: 5);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_conversion_rejects_floats() {
        let value = Value::from(1.5);
        let error = i64::from_json(&value).unwrap_err();
        assert!(matches!(
            error.kind(),
            ErrorKind::TypeMismatch {
                expected: "an integer",
                found: "number"
            }
        ));
    }

    #[test]
    fn roundtrip_through_json_text() {
        let value = vec![1u8, 2, 3].to_json();
        assert_eq!(Vec::<u8>::from_json(&value).unwrap(), vec![1, 2, 3]);
    }
}
