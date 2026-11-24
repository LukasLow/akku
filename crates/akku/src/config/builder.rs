//! The [`Builder`] that stacks configuration layers into a [`Config`].

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;

use crate::config::error::{Error, ErrorKind, Result};
use crate::config::path::{self, purge_sources};
use crate::config::value::ConfigValue;
use crate::config::{Config, Source};

/// Builder that stacks configuration layers from lowest to highest priority.
///
/// Every `add_*` call merges one more layer on top: values added later win over
/// values added earlier, leaf by leaf. Tables are merged key by key, lists are
/// replaced as a whole.
///
/// ```
/// use akku::config::builder;
///
/// let config = builder()
///     .add_toml("[db]\nhost = \"localhost\"\nport = 5432\n").unwrap()
///     .add_env_iter("APP", [("APP_DB__PORT", "6543")])
///     .build();
///
/// assert_eq!(config.require_str("db.host").unwrap(), "localhost");
/// assert_eq!(config.require_int("db.port").unwrap(), 6543);
/// ```
#[derive(Debug, Clone, Default)]
pub struct Builder {
    map: BTreeMap<String, ConfigValue>,
    sources: BTreeMap<Vec<String>, Source>,
}

impl Builder {
    /// Create an empty builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Declare the empty defaults layer.
    ///
    /// This is a readable marker for "the stack starts at defaults"; use
    /// [`Builder::set_default`] or [`Config::with_default`] to give that layer
    /// actual values. It adds no keys by itself.
    pub fn add_defaults(self) -> Self {
        self
    }

    /// Merge a JSON object as the next layer.
    ///
    /// The top-level JSON value must be an object.
    pub fn add_json(mut self, text: &str) -> Result<Self> {
        let value = crate::json::parse(text).map_err(json_error)?;
        let crate::json::Value::Object(object) = &value else {
            return Err(Error::new(ErrorKind::Parse {
                path: String::new(),
                reason: "the top-level JSON value must be an object".to_string(),
            }));
        };
        self.merge_layer(json_table(object), Source::Json);
        Ok(self)
    }

    /// Merge the JSON object read from `path` as the next layer.
    pub fn add_json_file<P: AsRef<Path>>(self, path: P) -> Result<Self> {
        let text = read_required(path.as_ref())?;
        self.add_json(&text)
    }

    /// Merge the JSON object read from `path`, ignoring a missing file.
    ///
    /// Any other I/O error and any parse error is still returned.
    pub fn add_json_file_optional<P: AsRef<Path>>(self, path: P) -> Result<Self> {
        match read_optional(path.as_ref())? {
            Some(text) => self.add_json(&text),
            None => Ok(self),
        }
    }

    /// Merge an already parsed [`crate::json::Value`] as the next layer.
    ///
    /// Only an object contributes keys; any other top-level value is ignored.
    pub fn add_json_value(mut self, value: &crate::json::Value) -> Self {
        if let crate::json::Value::Object(object) = value {
            self.merge_layer(json_table(object), Source::Json);
        }
        self
    }

    /// Merge a TOML document as the next layer.
    pub fn add_toml(mut self, text: &str) -> Result<Self> {
        let table = crate::toml::parse(text).map_err(toml_error)?;
        self.merge_layer(toml_table(&table), Source::Toml);
        Ok(self)
    }

    /// Merge the TOML document read from `path` as the next layer.
    pub fn add_toml_file<P: AsRef<Path>>(self, path: P) -> Result<Self> {
        let text = read_required(path.as_ref())?;
        self.add_toml(&text)
    }

    /// Merge the TOML document read from `path`, ignoring a missing file.
    ///
    /// Any other I/O error and any parse error is still returned.
    pub fn add_toml_file_optional<P: AsRef<Path>>(self, path: P) -> Result<Self> {
        match read_optional(path.as_ref())? {
            Some(text) => self.add_toml(&text),
            None => Ok(self),
        }
    }

    /// Merge an already parsed [`crate::toml::Table`] as the next layer.
    pub fn add_toml_table(mut self, table: &crate::toml::Table) -> Self {
        self.merge_layer(toml_table(table), Source::Toml);
        self
    }

    /// Merge an INI document as the next layer.
    ///
    /// Keys of the global section become top-level keys; keys of a named
    /// section `[name]` become `name.<key>`. All INI values are strings.
    pub fn add_ini(mut self, text: &str) -> Result<Self> {
        let document = crate::ini::parse(text).map_err(ini_error)?;
        self.merge_layer(ini_table(&document), Source::Ini);
        Ok(self)
    }

    /// Merge the INI document read from `path` as the next layer.
    pub fn add_ini_file<P: AsRef<Path>>(self, path: P) -> Result<Self> {
        let text = read_required(path.as_ref())?;
        self.add_ini(&text)
    }

    /// Merge a `.env` document as the next layer.
    ///
    /// Keys are used verbatim, values stay unexpanded strings.
    pub fn add_dotenv(mut self, text: &str) -> Result<Self> {
        let document = crate::dotenv::parse_str(text).map_err(dotenv_error)?;
        let mut table = BTreeMap::new();
        for (key, value) in document.iter() {
            table.insert(key.to_string(), ConfigValue::String(value.to_string()));
        }
        self.merge_layer(table, Source::DotEnv);
        Ok(self)
    }

    /// Merge the `.env` document read from `path` as the next layer.
    pub fn add_dotenv_file<P: AsRef<Path>>(self, path: P) -> Result<Self> {
        let text = read_required(path.as_ref())?;
        self.add_dotenv(&text)
    }

    /// Merge a snapshot of the process environment as the next layer.
    ///
    /// Every variable that starts with `prefix` contributes one key: the
    /// prefix and one optional following `_` are removed, the rest is
    /// lowercased, and `__` becomes `.`. `APP_SERVER__PORT=8080` therefore sets
    /// `server.port`, while `APP_SERVER_PORT` sets `server_port`.
    pub fn add_env_prefix(self, prefix: &str) -> Self {
        let mut vars = std::env::vars_os()
            .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)))
            .collect::<Vec<_>>();
        vars.sort_by(|(left, _), (right, _)| left.cmp(right));
        self.add_env_iter(prefix, vars)
    }

    /// Merge the given `(name, value)` pairs as the next layer.
    ///
    /// This is the pure form of [`Builder::add_env_prefix`] and is convenient
    /// for tests because it never touches the process environment.
    pub fn add_env_iter<I, K, V>(mut self, prefix: &str, vars: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<str>,
        V: AsRef<str>,
    {
        for (name, value) in vars {
            let Some(key) = env_key(prefix, name.as_ref()) else {
                continue;
            };
            self.insert_at(
                &key,
                ConfigValue::String(value.as_ref().to_string()),
                Source::Env,
            );
        }
        self
    }

    /// Set a single value as the highest priority layer.
    ///
    /// The dotted path creates intermediate tables as needed.
    pub fn set(mut self, path: &str, value: impl Into<ConfigValue>) -> Self {
        self.insert_at(path, value.into(), Source::Explicit);
        self
    }

    /// Set a default value, but only if the path is still unset.
    ///
    /// Unlike [`Builder::set`], this never overrides an existing value, which
    /// makes it a safe way to seed the defaults layer.
    pub fn set_default(mut self, path: &str, value: impl Into<ConfigValue>) -> Self {
        let segments: Vec<&str> = path::split(path).collect();
        insert_default_path(&mut self.map, &mut self.sources, &segments, value.into());
        self
    }

    /// Finish the stack and return the [`Config`].
    pub fn build(self) -> Config {
        Config::from_parts(self.map, self.sources)
    }

    /// Finish the stack and return the [`Config`].
    ///
    /// Alias for [`Builder::build`].
    pub fn into_config(self) -> Config {
        self.build()
    }

    fn merge_layer(&mut self, table: BTreeMap<String, ConfigValue>, source: Source) {
        merge_tree(
            &mut self.map,
            &mut self.sources,
            &[],
            table,
            &|_: &[String]| source,
        );
    }

    fn insert_at(&mut self, path: &str, value: ConfigValue, source: Source) {
        let segments: Vec<&str> = path::split(path).collect();
        insert_path(&mut self.map, &mut self.sources, &segments, value, source);
    }
}

/// Read a configuration file that must exist.
fn read_required(path: &Path) -> Result<String> {
    fs::read_to_string(path).map_err(Error::from)
}

/// Read a configuration file, treating a missing file as absent.
fn read_optional(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(Error::from(error)),
    }
}

/// Turn an environment variable name into a configuration path.
fn env_key(prefix: &str, name: &str) -> Option<String> {
    let rest = if prefix.is_empty() {
        name
    } else {
        let rest = name.strip_prefix(prefix)?;
        if rest.is_empty() || prefix.ends_with('_') {
            rest
        } else {
            // A prefix without a trailing underscore only matches on a `_`
            // boundary, so `APP` does not swallow `APPLE`.
            rest.strip_prefix('_')?
        }
    };
    if rest.is_empty() {
        return None;
    }
    Some(rest.to_ascii_lowercase().replace("__", "."))
}

/// Convert a JSON object into a configuration table.
fn json_table(object: &crate::json::Object) -> BTreeMap<String, ConfigValue> {
    object
        .iter()
        .map(|(key, value)| (key.to_string(), json_value(value)))
        .collect()
}

/// Convert a single JSON value into a configuration value.
fn json_value(value: &crate::json::Value) -> ConfigValue {
    match value {
        crate::json::Value::Null => ConfigValue::Null,
        crate::json::Value::Bool(flag) => ConfigValue::Bool(*flag),
        crate::json::Value::Number(number) => match number.as_i64() {
            Some(integer) => ConfigValue::Int(integer),
            None => ConfigValue::Float(number.as_f64()),
        },
        crate::json::Value::String(text) => ConfigValue::String(text.clone()),
        crate::json::Value::Array(items) => {
            ConfigValue::List(items.iter().map(json_value).collect())
        }
        crate::json::Value::Object(object) => ConfigValue::Table(json_table(object)),
    }
}

/// Convert a TOML table into a configuration table.
fn toml_table(table: &crate::toml::Table) -> BTreeMap<String, ConfigValue> {
    table
        .iter()
        .map(|(key, value)| (key.to_string(), toml_value(value)))
        .collect()
}

/// Convert a single TOML value into a configuration value.
fn toml_value(value: &crate::toml::Value) -> ConfigValue {
    match value {
        crate::toml::Value::String(text) => ConfigValue::String(text.clone()),
        crate::toml::Value::Integer(number) => ConfigValue::Int(*number),
        crate::toml::Value::Float(number) => ConfigValue::Float(*number),
        crate::toml::Value::Boolean(flag) => ConfigValue::Bool(*flag),
        // The configuration model has no calendar type; a date or time keeps
        // its RFC 3339 spelling as a plain string.
        crate::toml::Value::Datetime(datetime) => ConfigValue::String(datetime.to_string()),
        crate::toml::Value::Array(items) => {
            ConfigValue::List(items.iter().map(toml_value).collect())
        }
        crate::toml::Value::Table(table) => ConfigValue::Table(toml_table(table)),
    }
}

/// Flatten an INI document into a configuration table.
fn ini_table(document: &crate::ini::Ini) -> BTreeMap<String, ConfigValue> {
    let mut table = BTreeMap::new();
    for (key, value) in document.global().iter() {
        table.insert(key.to_string(), ConfigValue::String(value.to_string()));
    }
    for section in document.sections() {
        let entries = section
            .iter()
            .map(|(key, value)| (key.to_string(), ConfigValue::String(value.to_string())))
            .collect();
        table.insert(section.name().to_string(), ConfigValue::Table(entries));
    }
    table
}

fn json_error(error: crate::json::Error) -> Error {
    Error::new(ErrorKind::Format {
        source: Source::Json,
        line: error.position().map(|position| position.line as usize),
        reason: error.kind().to_string(),
    })
}

fn toml_error(error: crate::toml::Error) -> Error {
    Error::new(ErrorKind::Format {
        source: Source::Toml,
        line: Some(error.position().line as usize),
        reason: error.kind().to_string(),
    })
}

fn ini_error(error: crate::ini::Error) -> Error {
    let line = error.line();
    let reason = format_without_line(&error.to_string(), line);
    Error::new(ErrorKind::Format {
        source: Source::Ini,
        line,
        reason,
    })
}

fn dotenv_error(error: crate::dotenv::Error) -> Error {
    let line = error.line();
    let reason = format_without_line(&error.to_string(), line);
    Error::new(ErrorKind::Format {
        source: Source::DotEnv,
        line,
        reason,
    })
}

/// Format an INI or `.env` error without its redundant `line N: ` prefix.
///
/// Both parsers report the line twice: in [`ErrorKind::Format::line`] and in
/// the beginning of their own message. The line is kept once, in the field.
fn format_without_line(message: &str, line: Option<usize>) -> String {
    match line {
        Some(line) => {
            let prefix = format!("line {line}: ");
            match message.strip_prefix(&prefix) {
                Some(rest) => rest.to_string(),
                None => message.to_string(),
            }
        }
        None => message.to_string(),
    }
}

/// Merge `src` into `dst` leaf by leaf.
///
/// Tables are descended into so that unrelated keys survive; every other value
/// replaces whatever was stored at its path. `source_for` reports the origin of
/// a merged path.
pub(crate) fn merge_tree(
    dst: &mut BTreeMap<String, ConfigValue>,
    dst_sources: &mut BTreeMap<Vec<String>, Source>,
    base_prefix: &[String],
    src: BTreeMap<String, ConfigValue>,
    source_for: &dyn Fn(&[String]) -> Source,
) {
    let mut work: Vec<(Vec<String>, BTreeMap<String, ConfigValue>)> = vec![(Vec::new(), src)];
    while let Some((rel_path, table)) = work.pop() {
        let mut target = &mut *dst;
        for segment in &rel_path {
            let Some(ConfigValue::Table(next)) = target.get_mut(segment) else {
                unreachable!("merge_tree worklist only tracks table paths");
            };
            target = next;
        }
        for (key, mut value) in table {
            let mut child_rel = Vec::with_capacity(rel_path.len() + 1);
            child_rel.extend_from_slice(&rel_path);
            child_rel.push(key.clone());

            let mut full = Vec::with_capacity(base_prefix.len() + child_rel.len());
            full.extend_from_slice(base_prefix);
            full.extend(child_rel.iter().cloned());

            if let ConfigValue::Table(subtable) = &mut value {
                let subtable = std::mem::take(subtable);
                let source = source_for(&full);
                if !matches!(target.get(&key), Some(ConfigValue::Table(_))) {
                    purge_sources(dst_sources, &full);
                    target.insert(key, ConfigValue::Table(BTreeMap::new()));
                }
                dst_sources.insert(full, source);
                if !subtable.is_empty() {
                    work.push((child_rel, subtable));
                }
            } else {
                purge_sources(dst_sources, &full);
                let source = source_for(&full);
                target.insert(key, value);
                dst_sources.insert(full, source);
            }
        }
    }
}

/// Merge `src` into `dst` leaf by leaf with default semantics.
///
/// Existing scalar, list, or null values are never overwritten, and existing
/// sources are preserved; only missing keys (and missing sub-keys inside
/// existing tables) are populated with [`Source::Defaults`].
fn merge_defaults(
    dst: &mut BTreeMap<String, ConfigValue>,
    dst_sources: &mut BTreeMap<Vec<String>, Source>,
    base_prefix: &[String],
    src: BTreeMap<String, ConfigValue>,
) {
    let mut work: Vec<(Vec<String>, BTreeMap<String, ConfigValue>)> = vec![(Vec::new(), src)];
    while let Some((rel_path, table)) = work.pop() {
        let mut target = &mut *dst;
        for segment in &rel_path {
            let Some(ConfigValue::Table(next)) = target.get_mut(segment) else {
                unreachable!("merge_defaults worklist only tracks table paths");
            };
            target = next;
        }
        for (key, mut value) in table {
            let mut child_rel = Vec::with_capacity(rel_path.len() + 1);
            child_rel.extend_from_slice(&rel_path);
            child_rel.push(key.clone());

            let mut full = Vec::with_capacity(base_prefix.len() + child_rel.len());
            full.extend_from_slice(base_prefix);
            full.extend(child_rel.iter().cloned());

            if let ConfigValue::Table(subtable) = &mut value {
                let subtable = std::mem::take(subtable);
                match target.get(&key) {
                    Some(ConfigValue::Table(_)) => {
                        dst_sources.entry(full).or_insert(Source::Defaults);
                        if !subtable.is_empty() {
                            work.push((child_rel, subtable));
                        }
                    }
                    Some(_) => {}
                    None => {
                        target.insert(key, ConfigValue::Table(BTreeMap::new()));
                        dst_sources.insert(full, Source::Defaults);
                        if !subtable.is_empty() {
                            work.push((child_rel, subtable));
                        }
                    }
                }
            } else if let std::collections::btree_map::Entry::Vacant(entry) = target.entry(key) {
                entry.insert(value);
                dst_sources.insert(full, Source::Defaults);
            }
        }
    }
}

/// Insert `value` at the dotted `segments`, creating intermediate tables.
pub(crate) fn insert_path(
    map: &mut BTreeMap<String, ConfigValue>,
    sources: &mut BTreeMap<Vec<String>, Source>,
    segments: &[&str],
    value: ConfigValue,
    source: Source,
) {
    let Some((&last, prefix)) = segments.split_last() else {
        return;
    };
    let mut current = map;
    let mut path = Vec::with_capacity(segments.len());
    for &segment in prefix {
        path.push(segment.to_string());
        if !matches!(current.get(segment), Some(ConfigValue::Table(_))) {
            purge_sources(sources, &path);
            current.insert(segment.to_string(), ConfigValue::Table(BTreeMap::new()));
        }
        sources.entry(path.clone()).or_insert(source);
        let Some(ConfigValue::Table(next)) = current.get_mut(segment) else {
            return;
        };
        current = next;
    }
    let mut single = BTreeMap::new();
    single.insert(last.to_string(), value);
    merge_tree(current, sources, &path, single, &|_| source);
}

/// Insert default `value` at `segments` without overwriting any existing
/// non-table ancestor or existing leaf value.
pub(crate) fn insert_default_path(
    map: &mut BTreeMap<String, ConfigValue>,
    sources: &mut BTreeMap<Vec<String>, Source>,
    segments: &[&str],
    value: ConfigValue,
) {
    let Some((&last, prefix)) = segments.split_last() else {
        return;
    };

    // Abort without touching `map` or `sources` if any ancestor is non-table,
    // or if the target already exists and cannot be leaf-wise merged.
    let mut check: &BTreeMap<String, ConfigValue> = map;
    let mut prefix_exists = true;
    for &segment in prefix {
        match check.get(segment) {
            Some(ConfigValue::Table(next)) => check = next,
            Some(_) => return,
            None => {
                prefix_exists = false;
                break;
            }
        }
    }
    if prefix_exists {
        match check.get(last) {
            Some(ConfigValue::Table(_)) => {
                if !matches!(value, ConfigValue::Table(_)) {
                    return;
                }
            }
            Some(_) => return,
            None => {}
        }
    }

    let mut current = map;
    let mut path = Vec::with_capacity(segments.len());
    for &segment in prefix {
        path.push(segment.to_string());
        if !current.contains_key(segment) {
            current.insert(segment.to_string(), ConfigValue::Table(BTreeMap::new()));
        }
        sources.entry(path.clone()).or_insert(Source::Defaults);
        let Some(ConfigValue::Table(next)) = current.get_mut(segment) else {
            return;
        };
        current = next;
    }

    let mut single = BTreeMap::new();
    single.insert(last.to_string(), value);
    merge_defaults(current, sources, &path, single);
}
