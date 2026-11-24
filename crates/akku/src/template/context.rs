//! The variables a template renders with.

use std::collections::BTreeMap;

use crate::json;

use super::value::Value;

/// The variables available to a template.
///
/// ```
/// use akku::template::Context;
///
/// let mut context = Context::new();
/// context.insert("name", "Ada").insert("age", 36);
/// let context = context.with("admin", true).with("tags", vec!["a", "b"]);
/// assert_eq!(context.len(), 4);
/// assert_eq!(context.get("age").and_then(|v| v.as_i64()), Some(36));
/// ```
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Context {
    vars: BTreeMap<String, Value>,
}

impl Context {
    /// An empty context.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a variable, replacing an existing one of the same name.
    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<Value>) -> &mut Self {
        self.vars.insert(key.into(), value.into());
        self
    }

    /// Builder form of [`Context::insert`].
    #[must_use]
    pub fn with(mut self, key: impl Into<String>, value: impl Into<Value>) -> Self {
        self.insert(key, value);
        self
    }

    /// Look up a variable.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.vars.get(key)
    }

    /// Remove a variable, returning it.
    pub fn remove(&mut self, key: &str) -> Option<Value> {
        self.vars.remove(key)
    }

    /// Whether the variable exists.
    pub fn contains_key(&self, key: &str) -> bool {
        self.vars.contains_key(key)
    }

    /// The number of variables.
    pub fn len(&self) -> usize {
        self.vars.len()
    }

    /// Whether the context has no variables.
    pub fn is_empty(&self) -> bool {
        self.vars.is_empty()
    }

    /// Move every variable of `other` into this context.
    pub fn extend(&mut self, other: Context) {
        self.vars.extend(other.vars);
    }

    /// Iterate over the variables in name order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.vars.iter().map(|(key, value)| (key.as_str(), value))
    }

    pub(crate) fn into_vars(self) -> BTreeMap<String, Value> {
        self.vars
    }
}

/// A JSON object becomes one variable per key. Any other JSON value has no
/// names to offer and gives an empty context.
impl From<json::Value> for Context {
    fn from(value: json::Value) -> Self {
        match Value::from(value) {
            Value::Map(vars) => Self { vars },
            _ => Self::default(),
        }
    }
}

/// See [`From<json::Value>`](Context#impl-From<Value>-for-Context).
impl From<&json::Value> for Context {
    fn from(value: &json::Value) -> Self {
        match Value::from(value) {
            Value::Map(vars) => Self { vars },
            _ => Self::default(),
        }
    }
}

impl<K: Into<String>, V: Into<Value>> FromIterator<(K, V)> for Context {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        Self {
            vars: iter
                .into_iter()
                .map(|(key, value)| (key.into(), value.into()))
                .collect(),
        }
    }
}
