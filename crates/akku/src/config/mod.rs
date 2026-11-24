//! Layered application configuration.
//!
//! `akku::config` turns the crate's own [`json`](crate::json),
//! [`toml`](crate::toml), [`ini`](crate::ini) and [`dotenv`](crate::dotenv)
//! domains into one configuration tree with a single, explicit rule: the layer
//! added last wins, leaf by leaf.
//!
//! ```
//! use akku::config::{builder, ConfigValue, Source};
//!
//! let config = builder()
//!     .add_defaults()
//!     .add_toml("[server]\nhost = \"localhost\"\nport = 8080\n").unwrap()
//!     .add_env_iter("APP", [("APP_SERVER__PORT", "9090")])
//!     .set("server.tls", true)
//!     .build();
//!
//! assert_eq!(config.require_str("server.host").unwrap(), "localhost");
//! assert_eq!(config.require_int("server.port").unwrap(), 9090);
//! assert_eq!(config.get("server.tls"), Some(&ConfigValue::Bool(true)));
//! assert_eq!(config.source_of("server.port"), Some(&Source::Env));
//! ```
//!
//! # Precedence
//!
//! Typical stacks go `defaults < toml < json < ini < dotenv < env < explicit`,
//! but the order is entirely up to the caller. Tables are merged key by key,
//! so a higher layer can override one field without dropping its siblings;
//! lists are replaced as a whole.

use std::collections::BTreeMap;
use std::fmt;
use std::ops::Index;

mod builder;
mod error;
mod path;
mod value;

pub use self::builder::Builder;
pub use self::error::{Error, ErrorKind, Result};
pub use self::value::ConfigValue;

use self::builder::insert_default_path;
use self::value::scalar_text;

/// The layer a value came from.
///
/// Sources are ordered only by the caller's `add_*` calls; the enum itself
/// carries no priority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Source {
    /// A built-in or [`Config::with_default`] default.
    Defaults,
    /// A JSON document.
    Json,
    /// A TOML document.
    Toml,
    /// An INI document.
    Ini,
    /// A `.env` document.
    DotEnv,
    /// The process environment.
    Env,
    /// A value set directly through [`Builder::set`].
    Explicit,
}

impl Source {
    /// The short lower-case name of the source.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Defaults => "defaults",
            Self::Json => "json",
            Self::Toml => "toml",
            Self::Ini => "ini",
            Self::DotEnv => "dotenv",
            Self::Env => "env",
            Self::Explicit => "explicit",
        }
    }
}

impl fmt::Display for Source {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Create an empty [`Builder`].
pub fn builder() -> Builder {
    Builder::new()
}

/// A merged configuration tree.
///
/// Values are read with the coercing getters ([`Config::get_int`] and friends)
/// or with the strict `require_*` getters that return an [`Error`] naming the
/// path. [`Config::source_of`] reports which layer produced a value.
///
/// ```
/// use akku::config::builder;
///
/// let config = builder().add_json(r#"{"port": 8080}"#).unwrap().build();
/// assert_eq!(config.require_int("port").unwrap(), 8080);
/// assert!(config.require_str("host").is_err());
/// ```
#[derive(Debug, Clone, Default)]
pub struct Config {
    map: BTreeMap<String, ConfigValue>,
    sources: BTreeMap<Vec<String>, Source>,
}

impl Config {
    /// Create an empty configuration.
    pub fn empty() -> Self {
        Self::default()
    }

    pub(crate) fn from_parts(
        map: BTreeMap<String, ConfigValue>,
        sources: BTreeMap<Vec<String>, Source>,
    ) -> Self {
        Self { map, sources }
    }

    /// Look up a dotted path.
    pub fn get(&self, path: &str) -> Option<&ConfigValue> {
        path::lookup(&self.map, path)
    }

    /// Look up a value by its exact path segments without splitting on dots.
    ///
    /// This reaches literal dotted or empty keys loaded from formats such as
    /// JSON, TOML, or INI.
    pub fn get_segments(&self, segments: &[&str]) -> Option<&ConfigValue> {
        path::lookup_segments(&self.map, segments)
    }

    /// The layer that last wrote `path`, if any.
    pub fn source_of(&self, path: &str) -> Option<&Source> {
        let segments: Vec<String> = path::split(path).map(str::to_string).collect();
        if segments.is_empty() {
            return None;
        }
        self.sources.get(&segments)
    }

    /// Iterate over the top-level keys in sorted order.
    pub fn keys(&self) -> impl Iterator<Item = &str> + '_ {
        self.map.keys().map(String::as_str)
    }

    /// Clone the whole tree as a map of top-level entries.
    pub fn to_map(&self) -> BTreeMap<String, ConfigValue> {
        self.map.clone()
    }

    /// Consume the configuration and return its map of top-level entries.
    pub fn into_map(self) -> BTreeMap<String, ConfigValue> {
        self.map
    }

    /// Merge another configuration on top of this one, leaf by leaf.
    ///
    /// The other configuration wins where both define a value; each merged
    /// value keeps the source it had there.
    pub fn merge(&mut self, other: Self) {
        let Self {
            map,
            sources: other_sources,
        } = other;
        let source_of =
            |path: &[String]| other_sources.get(path).copied().unwrap_or(Source::Explicit);
        builder::merge_tree(&mut self.map, &mut self.sources, &[], map, &source_of);
    }

    /// Add `value` at `path` only if nothing is set there yet.
    ///
    /// The value is recorded with [`Source::Defaults`].
    pub fn with_default(mut self, path: &str, value: impl Into<ConfigValue>) -> Self {
        let segments: Vec<&str> = path::split(path).collect();
        insert_default_path(&mut self.map, &mut self.sources, &segments, value.into());
        self
    }

    /// Read a string, without any coercion.
    pub fn get_str(&self, path: &str) -> Option<&str> {
        self.get(path)?.as_str()
    }

    /// Read an integer.
    ///
    /// Accepts an `Int` or a string holding a trimmed integer. Floats,
    /// booleans and strings such as `"4.2"` are rejected.
    pub fn get_int(&self, path: &str) -> Option<i64> {
        match self.get(path)? {
            ConfigValue::Int(number) => Some(*number),
            ConfigValue::String(text) => text.trim().parse().ok(),
            _ => None,
        }
    }

    /// Read a float.
    ///
    /// Accepts a `Float`, an `Int`, or a string holding a finite numeric value.
    pub fn get_float(&self, path: &str) -> Option<f64> {
        match self.get(path)? {
            ConfigValue::Float(number) => Some(*number),
            ConfigValue::Int(number) => Some(*number as f64),
            ConfigValue::String(text) => text
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|value| value.is_finite()),
            _ => None,
        }
    }

    /// Read a boolean.
    ///
    /// Accepts a `Bool` or a case-insensitive string: `true`/`false`, `1`/`0`,
    /// `yes`/`no`, `on`/`off` and `y`/`n`.
    pub fn get_bool(&self, path: &str) -> Option<bool> {
        match self.get(path)? {
            ConfigValue::Bool(flag) => Some(*flag),
            ConfigValue::String(text) => parse_bool(text),
            _ => None,
        }
    }

    /// Read a list of strings.
    ///
    /// Accepts a `List` whose items are strings or scalars, or a single string
    /// of comma-separated items. Blank items are skipped.
    pub fn get_list(&self, path: &str) -> Option<Vec<String>> {
        match self.get(path)? {
            ConfigValue::List(items) => items.iter().map(scalar_text).collect(),
            ConfigValue::String(text) => Some(split_list(text)),
            _ => None,
        }
    }

    /// Read a required string.
    pub fn require_str(&self, path: &str) -> Result<&str> {
        let value = self.get(path).ok_or_else(|| missing(path))?;
        value
            .as_str()
            .ok_or_else(|| wrong_type(path, "string", value))
    }

    /// Read a required integer, with the same coercion as [`Config::get_int`].
    pub fn require_int(&self, path: &str) -> Result<i64> {
        let value = self.get(path).ok_or_else(|| missing(path))?;
        self.get_int(path)
            .ok_or_else(|| wrong_type(path, "int", value))
    }

    /// Read a required float, with the same coercion as [`Config::get_float`].
    pub fn require_float(&self, path: &str) -> Result<f64> {
        let value = self.get(path).ok_or_else(|| missing(path))?;
        self.get_float(path)
            .ok_or_else(|| wrong_type(path, "float", value))
    }

    /// Read a required boolean, with the same coercion as [`Config::get_bool`].
    pub fn require_bool(&self, path: &str) -> Result<bool> {
        let value = self.get(path).ok_or_else(|| missing(path))?;
        self.get_bool(path)
            .ok_or_else(|| wrong_type(path, "bool", value))
    }

    /// Read a required list, with the same coercion as [`Config::get_list`].
    pub fn require_list(&self, path: &str) -> Result<Vec<String>> {
        let value = self.get(path).ok_or_else(|| missing(path))?;
        self.get_list(path)
            .ok_or_else(|| wrong_type(path, "list of strings", value))
    }

    /// Read a required table.
    pub fn require_table(&self, path: &str) -> Result<&BTreeMap<String, ConfigValue>> {
        let value = self.get(path).ok_or_else(|| missing(path))?;
        value
            .as_table()
            .ok_or_else(|| wrong_type(path, "table", value))
    }
}

impl Index<&str> for Config {
    type Output = ConfigValue;

    fn index(&self, path: &str) -> &Self::Output {
        self.get(path)
            .unwrap_or_else(|| panic!("no configuration value at \"{path}\""))
    }
}

fn missing(path: &str) -> Error {
    Error::new(ErrorKind::Missing {
        path: path.to_string(),
    })
}

fn wrong_type(path: &str, expected: &'static str, value: &ConfigValue) -> Error {
    Error::new(ErrorKind::WrongType {
        path: path.to_string(),
        expected,
        found: value.type_name(),
    })
}

fn parse_bool(text: &str) -> Option<bool> {
    let trimmed = text.trim();
    if trimmed.eq_ignore_ascii_case("true")
        || trimmed == "1"
        || trimmed.eq_ignore_ascii_case("yes")
        || trimmed.eq_ignore_ascii_case("on")
        || trimmed.eq_ignore_ascii_case("y")
    {
        Some(true)
    } else if trimmed.eq_ignore_ascii_case("false")
        || trimmed == "0"
        || trimmed.eq_ignore_ascii_case("no")
        || trimmed.eq_ignore_ascii_case("off")
        || trimmed.eq_ignore_ascii_case("n")
    {
        Some(false)
    } else {
        None
    }
}

fn split_list(text: &str) -> Vec<String> {
    text.split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect()
}
