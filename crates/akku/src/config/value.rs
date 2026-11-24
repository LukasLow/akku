//! The [`ConfigValue`] tree shared by every configuration layer.

use std::collections::BTreeMap;

/// A value in a configuration tree.
///
/// Layers are parsed into this one model, so a TOML table, a JSON object and
/// a nested environment variable all look the same to the reader.
///
/// ```
/// use akku::config::ConfigValue;
///
/// let value = ConfigValue::from("8080");
/// assert_eq!(value.type_name(), "string");
/// assert_eq!(ConfigValue::from(8080i64).as_int(), Some(8080));
/// ```
#[derive(Debug, Clone, PartialEq)]
pub enum ConfigValue {
    /// An explicit null or absent value.
    Null,
    /// A boolean.
    Bool(bool),
    /// A signed 64-bit integer.
    Int(i64),
    /// A 64-bit floating-point number.
    Float(f64),
    /// A UTF-8 string.
    String(String),
    /// A list of values.
    List(Vec<ConfigValue>),
    /// A table of named values.
    Table(BTreeMap<String, ConfigValue>),
}

impl Drop for ConfigValue {
    fn drop(&mut self) {
        // A recursive drop would overflow the stack on deeply nested trees.
        // Drain children into an explicit worklist instead.
        let mut stack: Vec<ConfigValue> = match self {
            Self::List(items) => std::mem::take(items),
            Self::Table(table) => std::mem::take(table).into_values().collect(),
            _ => return,
        };
        while let Some(mut value) = stack.pop() {
            match &mut value {
                Self::List(items) => stack.append(items),
                Self::Table(table) => {
                    for (_, child) in std::mem::take(table) {
                        stack.push(child);
                    }
                }
                _ => {}
            }
        }
    }
}

impl ConfigValue {
    /// The short type name used in error messages.
    ///
    /// One of `"null"`, `"bool"`, `"int"`, `"float"`, `"string"`, `"list"` or
    /// `"table"`.
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Null => "null",
            Self::Bool(_) => "bool",
            Self::Int(_) => "int",
            Self::Float(_) => "float",
            Self::String(_) => "string",
            Self::List(_) => "list",
            Self::Table(_) => "table",
        }
    }

    /// The value as a boolean, if it is one.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            _ => None,
        }
    }

    /// The value as an integer, if it is one.
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Self::Int(value) => Some(*value),
            _ => None,
        }
    }

    /// The value as a float, if it is one.
    pub fn as_float(&self) -> Option<f64> {
        match self {
            Self::Float(value) => Some(*value),
            _ => None,
        }
    }

    /// The value as a string slice, if it is a string.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            _ => None,
        }
    }

    /// The value as a list, if it is one.
    pub fn as_list(&self) -> Option<&[Self]> {
        match self {
            Self::List(values) => Some(values),
            _ => None,
        }
    }

    /// The value as a table, if it is one.
    pub fn as_table(&self) -> Option<&BTreeMap<String, Self>> {
        match self {
            Self::Table(table) => Some(table),
            _ => None,
        }
    }
}

/// Render a scalar value the way [`Config::get_list`](crate::config::Config::get_list)
/// wants it, returning `None` for `null`, lists and tables.
pub(crate) fn scalar_text(value: &ConfigValue) -> Option<String> {
    match value {
        ConfigValue::String(text) => Some(text.clone()),
        ConfigValue::Int(number) => Some(number.to_string()),
        ConfigValue::Float(number) => Some(number.to_string()),
        ConfigValue::Bool(flag) => Some(flag.to_string()),
        ConfigValue::Null | ConfigValue::List(_) | ConfigValue::Table(_) => None,
    }
}

impl From<bool> for ConfigValue {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl From<i64> for ConfigValue {
    fn from(value: i64) -> Self {
        Self::Int(value)
    }
}

impl From<f64> for ConfigValue {
    fn from(value: f64) -> Self {
        Self::Float(value)
    }
}

impl From<&str> for ConfigValue {
    fn from(value: &str) -> Self {
        Self::String(value.to_string())
    }
}

impl From<String> for ConfigValue {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl<T: Into<ConfigValue>> From<Vec<T>> for ConfigValue {
    fn from(values: Vec<T>) -> Self {
        Self::List(values.into_iter().map(Into::into).collect())
    }
}

impl<K: Into<String>, V: Into<ConfigValue>> From<BTreeMap<K, V>> for ConfigValue {
    fn from(values: BTreeMap<K, V>) -> Self {
        Self::Table(
            values
                .into_iter()
                .map(|(key, value)| (key.into(), value.into()))
                .collect(),
        )
    }
}
