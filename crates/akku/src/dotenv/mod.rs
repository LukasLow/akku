//! Pure `.env` parser, variable expander, and environment loader.
//!
//! Parsing (`parse_str`, `parse_str_with`) is completely pure and **never**
//! reads `std::env`. Variable interpolation (`${VAR}` and `${VAR:-default}`)
//! happens in a separate expansion pass via [`DotEnv::expand`],
//! [`DotEnv::expand_with`], [`DotEnv::expanded`], or [`DotEnv::expand_value`].
//!
//! # Examples
//!
//! ```
//! use akku::dotenv;
//!
//! let env = dotenv::parse_str("HOST=localhost\nURL=http://${HOST}:8080\n").unwrap();
//! assert_eq!(env.get("URL"), Some("http://${HOST}:8080"));
//!
//! let expanded = env.expand().unwrap();
//! assert_eq!(expanded.get("URL").map(String::as_str), Some("http://localhost:8080"));
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::ops::Index;
use std::path::Path;

/// Result type used by `akku::dotenv`.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Options controlling `.env` parsing and variable expansion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseOptions {
    /// Whether an optional leading `export ` keyword before a key is stripped.
    pub allow_export: bool,
    /// Whether `${VAR}` and `${VAR:-default}` interpolation is performed during expansion.
    pub interpolation: bool,
    /// Whether unresolved `${VAR}` references (without `:-default`) fail expansion.
    pub strict_interpolation: bool,
    /// Maximum recursive expansion depth allowed before returning [`ErrorKind::MaxExpansionDepth`].
    pub max_expansion_depth: usize,
}

impl Default for ParseOptions {
    fn default() -> Self {
        Self {
            allow_export: true,
            interpolation: true,
            strict_interpolation: false,
            max_expansion_depth: 16,
        }
    }
}

/// Specific kind of `.env` error.
#[derive(Debug)]
#[non_exhaustive]
pub enum ErrorKind {
    /// Underlying I/O error.
    Io(io::Error),
    /// A non-empty, non-comment line lacked `=`.
    MissingEquals {
        /// Text encountered before the end of the line.
        key: String,
    },
    /// A key name was empty or contained invalid characters.
    InvalidKey {
        /// The offending key text.
        key: String,
    },
    /// A single- or double-quoted value reached EOF without a closing quote.
    UnterminatedQuote,
    /// A double-quoted value contained an unrecognized escape sequence.
    InvalidEscape {
        /// The offending escape sequence.
        escape: String,
    },
    /// An un-defaulted `${VAR}` could not be resolved in strict mode.
    UnresolvedVariable {
        /// Name of the missing variable.
        name: String,
    },
    /// Variable expansion encountered a cycle.
    CyclicExpansion {
        /// Human-readable cycle chain (e.g. `"A -> B -> A"`).
        chain: String,
    },
    /// Recursive variable expansion exceeded [`ParseOptions::max_expansion_depth`].
    MaxExpansionDepth {
        /// Configured depth limit that was exceeded.
        limit: usize,
    },
}

/// Error returned by `.env` parsing, expansion, loading, and serialization.
#[derive(Debug)]
pub struct Error {
    kind: ErrorKind,
    line: Option<usize>,
}

impl Error {
    fn new(kind: ErrorKind) -> Self {
        Self { kind, line: None }
    }

    fn with_line(kind: ErrorKind, line: usize) -> Self {
        Self {
            kind,
            line: Some(line),
        }
    }

    /// Return the specific [`ErrorKind`].
    pub fn kind(&self) -> &ErrorKind {
        &self.kind
    }

    /// Return the 1-based line number associated with a parse error, if any.
    pub fn line(&self) -> Option<usize> {
        self.line
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(line) = self.line {
            write!(f, "line {line}: ")?;
        }
        match &self.kind {
            ErrorKind::Io(err) => write!(f, "I/O error: {err}"),
            ErrorKind::MissingEquals { key } => write!(f, "missing '=' after '{key}'"),
            ErrorKind::InvalidKey { key } => write!(f, "invalid environment key '{key}'"),
            ErrorKind::UnterminatedQuote => f.write_str("unterminated quoted value"),
            ErrorKind::InvalidEscape { escape } => {
                write!(f, "invalid escape sequence '{escape}'")
            }
            ErrorKind::UnresolvedVariable { name } => {
                write!(f, "unresolved variable '{name}'")
            }
            ErrorKind::CyclicExpansion { chain } => {
                write!(f, "cyclic variable expansion: {chain}")
            }
            ErrorKind::MaxExpansionDepth { limit } => {
                write!(f, "maximum variable expansion depth ({limit}) exceeded")
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.kind {
            ErrorKind::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Self {
        Self::new(ErrorKind::Io(err))
    }
}

#[derive(Debug, Clone, Eq)]
struct Entry {
    key: String,
    value: String,
    allow_interpolation: bool,
    escaped_dollars: Vec<usize>,
}

impl PartialEq for Entry {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key && self.value == other.value
    }
}

/// An insertion-ordered collection of raw `.env` entries.
#[derive(Debug, Clone, Eq, Default)]
pub struct DotEnv {
    entries: Vec<Entry>,
    default_options: ParseOptions,
}

impl PartialEq for DotEnv {
    fn eq(&self, other: &Self) -> bool {
        self.entries == other.entries
    }
}

impl AsRef<DotEnv> for DotEnv {
    fn as_ref(&self) -> &DotEnv {
        self
    }
}

impl DotEnv {
    /// Create an empty [`DotEnv`] collection.
    pub fn new() -> Self {
        Self::default()
    }

    fn find_index(&self, key: &str) -> Option<usize> {
        self.entries.iter().position(|e| e.key == key)
    }

    /// Look up the raw value for `key`.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.find_index(key).map(|i| self.entries[i].value.as_str())
    }

    /// Return `true` if `key` is present.
    pub fn contains_key(&self, key: &str) -> bool {
        self.find_index(key).is_some()
    }

    /// Insert or update `key` with raw `value`, preserving insertion order on update.
    ///
    /// Returns the previous raw value if `key` was already present.
    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<String>) -> Option<String> {
        self.insert_entry(Entry {
            key: key.into(),
            value: value.into(),
            allow_interpolation: true,
            escaped_dollars: Vec::new(),
        })
    }

    fn insert_entry(&mut self, entry: Entry) -> Option<String> {
        if let Some(idx) = self.find_index(&entry.key) {
            let prev = std::mem::replace(&mut self.entries[idx], entry);
            Some(prev.value)
        } else {
            self.entries.push(entry);
            None
        }
    }

    /// Remove `key` in order-preserving fashion, returning its raw value if present.
    pub fn remove(&mut self, key: &str) -> Option<String> {
        let idx = self.find_index(key)?;
        Some(self.entries.remove(idx).value)
    }

    /// Return the number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Return `true` if there are no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Iterate over raw `(key, value)` pairs in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.entries
            .iter()
            .map(|e| (e.key.as_str(), e.value.as_str()))
    }

    /// Iterate over keys in insertion order.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|e| e.key.as_str())
    }

    /// Expand all entries into a [`BTreeMap`] using this instance's options
    /// (defaulting to [`ParseOptions::default()`]).
    pub fn expand(&self) -> Result<BTreeMap<String, String>> {
        self.expand_with(&self.default_options)
    }

    /// Expand all entries into a [`BTreeMap`] using explicit [`ParseOptions`].
    pub fn expand_with(&self, options: &ParseOptions) -> Result<BTreeMap<String, String>> {
        let mut resolved = BTreeMap::new();
        for entry in &self.entries {
            if !resolved.contains_key(&entry.key) {
                let mut visiting = Vec::new();
                let val =
                    self.expand_key_inner(&entry.key, options, 0, &mut visiting, &mut resolved)?;
                resolved.insert(entry.key.clone(), val);
            }
        }
        Ok(resolved)
    }

    /// Return a new [`DotEnv`] with all values expanded in insertion order.
    pub fn expanded(&self) -> Result<DotEnv> {
        self.expanded_with(&self.default_options)
    }

    /// Return a new [`DotEnv`] with all values expanded using explicit [`ParseOptions`].
    pub fn expanded_with(&self, options: &ParseOptions) -> Result<DotEnv> {
        let map = self.expand_with(options)?;
        let mut out = DotEnv::new();
        for entry in &self.entries {
            let val = map.get(&entry.key).cloned().unwrap_or_default();
            out.insert_entry(Entry {
                key: entry.key.clone(),
                value: val,
                allow_interpolation: false,
                escaped_dollars: Vec::new(),
            });
        }
        Ok(out)
    }

    /// Expand a single key from `self`, returning `Ok(None)` if `key` is absent.
    pub fn expand_value(&self, key: &str) -> Result<Option<String>> {
        self.expand_value_with(key, &self.default_options)
    }

    /// Expand a single key from `self` using explicit [`ParseOptions`].
    pub fn expand_value_with(&self, key: &str, options: &ParseOptions) -> Result<Option<String>> {
        if !self.contains_key(key) {
            return Ok(None);
        }
        let mut resolved = BTreeMap::new();
        let mut visiting = Vec::new();
        let val = self.expand_key_inner(key, options, 0, &mut visiting, &mut resolved)?;
        Ok(Some(val))
    }

    /// Expand entries and insert those not already set in the process environment.
    ///
    /// Returns the number of environment variables written.
    pub fn load_into_env(&self) -> Result<usize> {
        let expanded = self.expand()?;
        let mut set_count = 0;
        for entry in &self.entries {
            if std::env::var_os(&entry.key).is_none() {
                if let Some(val) = expanded.get(&entry.key) {
                    std::env::set_var(&entry.key, val);
                    set_count += 1;
                }
            }
        }
        Ok(set_count)
    }

    /// Expand entries and write all of them into the process environment,
    /// overwriting any pre-existing values.
    ///
    /// Returns the number of environment variables written.
    pub fn load_into_env_overwrite(&self) -> Result<usize> {
        let expanded = self.expand()?;
        let mut set_count = 0;
        for entry in &self.entries {
            if let Some(val) = expanded.get(&entry.key) {
                std::env::set_var(&entry.key, val);
                set_count += 1;
            }
        }
        Ok(set_count)
    }

    /// Merge entries from `other` into `self`, updating existing keys in-place
    /// and appending new keys in order.
    pub fn merge(&mut self, other: impl AsRef<DotEnv>) {
        for entry in &other.as_ref().entries {
            self.insert_entry(entry.clone());
        }
    }

    fn expand_key_inner(
        &self,
        key: &str,
        options: &ParseOptions,
        depth: usize,
        visiting: &mut Vec<String>,
        resolved: &mut BTreeMap<String, String>,
    ) -> Result<String> {
        if let Some(cached) = resolved.get(key) {
            return Ok(cached.clone());
        }

        if let Some(pos) = visiting.iter().position(|k| k == key) {
            let mut cycle = visiting[pos..].to_vec();
            cycle.push(key.to_string());
            return Err(Error::new(ErrorKind::CyclicExpansion {
                chain: cycle.join(" -> "),
            }));
        }

        if depth > options.max_expansion_depth {
            return Err(Error::new(ErrorKind::MaxExpansionDepth {
                limit: options.max_expansion_depth,
            }));
        }

        let Some(idx) = self.find_index(key) else {
            return Err(Error::new(ErrorKind::UnresolvedVariable {
                name: key.to_string(),
            }));
        };
        let entry = &self.entries[idx];

        if !options.interpolation || !entry.allow_interpolation {
            return Ok(entry.value.clone());
        }

        visiting.push(key.to_string());
        let val = self.expand_template(
            &entry.value,
            &entry.escaped_dollars,
            options,
            depth,
            visiting,
            resolved,
        )?;
        visiting.pop();

        resolved.insert(key.to_string(), val.clone());
        Ok(val)
    }

    fn expand_template(
        &self,
        template: &str,
        escaped_dollars: &[usize],
        options: &ParseOptions,
        depth: usize,
        visiting: &mut Vec<String>,
        resolved: &mut BTreeMap<String, String>,
    ) -> Result<String> {
        let mut out = String::with_capacity(template.len());
        let bytes = template.as_bytes();
        let mut i = 0;

        while i < bytes.len() {
            if bytes[i] == b'$'
                && !escaped_dollars.contains(&i)
                && i + 1 < bytes.len()
                && bytes[i + 1] == b'{'
            {
                if let Some((expr_end, var_name, default_expr)) = parse_braced_expr(template, i + 2)
                {
                    let var_val = if self.contains_key(var_name) {
                        Some(self.expand_key_inner(
                            var_name,
                            options,
                            depth + 1,
                            visiting,
                            resolved,
                        )?)
                    } else {
                        None
                    };

                    match (var_val, default_expr) {
                        (Some(v), None) => out.push_str(&v),
                        (Some(v), Some(_)) if !v.is_empty() => out.push_str(&v),
                        (_, Some(def)) => {
                            // The default expression expands one level deeper.
                            // Check the limit before recursing so deeply nested
                            // defaults such as `${Z:-${Z:-...}}` fail with an
                            // error instead of overflowing the stack.
                            if depth + 1 > options.max_expansion_depth {
                                return Err(Error::new(ErrorKind::MaxExpansionDepth {
                                    limit: options.max_expansion_depth,
                                }));
                            }
                            let expanded_def = self.expand_template(
                                def,
                                &[],
                                options,
                                depth + 1,
                                visiting,
                                resolved,
                            )?;
                            out.push_str(&expanded_def);
                        }
                        (None, None) => {
                            if options.strict_interpolation {
                                return Err(Error::new(ErrorKind::UnresolvedVariable {
                                    name: var_name.to_string(),
                                }));
                            }
                        }
                    }

                    i = expr_end + 1;
                    continue;
                }
            }

            let ch = template[i..]
                .chars()
                .next()
                .expect("i points to a valid UTF-8 boundary");
            out.push(ch);
            i += ch.len_utf8();
        }

        Ok(out)
    }
}

impl Index<&str> for DotEnv {
    type Output = str;

    fn index(&self, key: &str) -> &Self::Output {
        self.get(key)
            .unwrap_or_else(|| panic!("no entry found for key '{key}'"))
    }
}

impl FromIterator<(String, String)> for DotEnv {
    fn from_iter<T: IntoIterator<Item = (String, String)>>(iter: T) -> Self {
        let mut env = Self::new();
        for (k, v) in iter {
            env.insert(k, v);
        }
        env
    }
}

impl fmt::Display for DotEnv {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&write_to_string(self))
    }
}

/// Parse a `.env` string using [`ParseOptions::default()`].
///
/// This function is pure and never reads `std::env`.
pub fn parse_str(input: &str) -> Result<DotEnv> {
    parse_str_with(input, ParseOptions::default())
}

/// Parse a `.env` string using explicit [`ParseOptions`].
///
/// This function is pure and never reads `std::env`.
pub fn parse_str_with(input: &str, options: ParseOptions) -> Result<DotEnv> {
    let input = input.strip_prefix('\u{FEFF}').unwrap_or(input);
    let mut env = DotEnv {
        entries: Vec::new(),
        default_options: options,
    };

    let bytes = input.as_bytes();
    let mut pos = 0;
    let mut line = 1usize;

    while pos < bytes.len() {
        // Skip horizontal whitespace
        while pos < bytes.len() && (bytes[pos] == b' ' || bytes[pos] == b'\t') {
            pos += 1;
        }
        if pos >= bytes.len() {
            break;
        }

        // Empty line
        if bytes[pos] == b'\n' {
            pos += 1;
            line += 1;
            continue;
        }
        if bytes[pos] == b'\r' {
            pos += 1;
            if pos < bytes.len() && bytes[pos] == b'\n' {
                pos += 1;
            }
            line += 1;
            continue;
        }

        // Comment line
        if bytes[pos] == b'#' {
            while pos < bytes.len() && bytes[pos] != b'\n' {
                pos += 1;
            }
            if pos < bytes.len() && bytes[pos] == b'\n' {
                pos += 1;
                line += 1;
            }
            continue;
        }

        let entry_line = line;

        // Optional `export ` prefix
        if options.allow_export && input[pos..].starts_with("export") {
            let after_export = pos + "export".len();
            if after_export < bytes.len()
                && (bytes[after_export] == b' ' || bytes[after_export] == b'\t')
            {
                pos = after_export;
                while pos < bytes.len() && (bytes[pos] == b' ' || bytes[pos] == b'\t') {
                    pos += 1;
                }
            }
        }

        // Scan key up to '=' or newline
        let key_start = pos;
        while pos < bytes.len() && bytes[pos] != b'=' && bytes[pos] != b'\n' && bytes[pos] != b'\r'
        {
            pos += 1;
        }

        if pos >= bytes.len() || bytes[pos] == b'\n' || bytes[pos] == b'\r' {
            let key_text = input[key_start..pos].trim().to_string();
            return Err(Error::with_line(
                ErrorKind::MissingEquals { key: key_text },
                entry_line,
            ));
        }

        let raw_key = input[key_start..pos].trim_end_matches([' ', '\t']);
        if !is_valid_key(raw_key) {
            return Err(Error::with_line(
                ErrorKind::InvalidKey {
                    key: raw_key.to_string(),
                },
                entry_line,
            ));
        }
        let key = raw_key.to_string();

        // Consume '='
        pos += 1;

        // Skip leading horizontal whitespace before value
        while pos < bytes.len() && (bytes[pos] == b' ' || bytes[pos] == b'\t') {
            pos += 1;
        }

        let (value, allow_interpolation, escaped_dollars) = if pos < bytes.len()
            && bytes[pos] == b'\''
        {
            pos += 1;
            let val = parse_single_quoted(input, &mut pos, &mut line, entry_line)?;
            skip_trailing_comment(input, &mut pos, &mut line, entry_line)?;
            (val, false, Vec::new())
        } else if pos < bytes.len() && bytes[pos] == b'"' {
            pos += 1;
            let (val, esc_dollars) = parse_double_quoted(input, &mut pos, &mut line, entry_line)?;
            skip_trailing_comment(input, &mut pos, &mut line, entry_line)?;
            (val, true, esc_dollars)
        } else {
            let val = parse_unquoted(input, &mut pos, &mut line);
            (val, true, Vec::new())
        };

        env.insert_entry(Entry {
            key,
            value,
            allow_interpolation,
            escaped_dollars,
        });
    }

    Ok(env)
}

/// Read and parse a `.env` file using [`ParseOptions::default()`].
pub fn from_file(path: impl AsRef<Path>) -> Result<DotEnv> {
    from_file_with(path, ParseOptions::default())
}

/// Read and parse a `.env` file using explicit [`ParseOptions`].
pub fn from_file_with(path: impl AsRef<Path>, options: ParseOptions) -> Result<DotEnv> {
    let contents = fs::read_to_string(path)?;
    parse_str_with(&contents, options)
}

/// Search `candidates` in order, parse the first existing file, load its expanded
/// variables into `std::env` without overwriting existing variables, and return it.
///
/// Returns `Ok(None)` if none of the candidate paths exist.
pub fn find_and_load(candidates: &[&str]) -> Result<Option<DotEnv>> {
    for candidate in candidates {
        match fs::read_to_string(candidate) {
            Ok(contents) => {
                let env = parse_str(&contents)?;
                env.load_into_env()?;
                return Ok(Some(env));
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
            Err(err) => return Err(Error::from(err)),
        }
    }
    Ok(None)
}

/// Serialize `env` into `.env` format.
pub fn write_to_string(env: &DotEnv) -> String {
    let mut buf = Vec::new();
    write_to(env, &mut buf).expect("writing to Vec<u8> cannot fail");
    String::from_utf8(buf).expect(".env serialization produces valid UTF-8")
}

/// Write `env` in `.env` format to `writer`.
pub fn write_to<W: Write>(env: &DotEnv, mut writer: W) -> Result<()> {
    for entry in &env.entries {
        let formatted = format_entry_value(entry);
        writeln!(writer, "{}={}", entry.key, formatted)?;
    }
    Ok(())
}

fn is_valid_key(key: &str) -> bool {
    let mut chars = key.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn parse_single_quoted(
    input: &str,
    pos: &mut usize,
    line: &mut usize,
    start_line: usize,
) -> Result<String> {
    let bytes = input.as_bytes();
    let mut out = String::new();

    while *pos < bytes.len() {
        let b = bytes[*pos];
        if b == b'\'' {
            *pos += 1;
            return Ok(out);
        }
        if b == b'\r' {
            *pos += 1;
            if *pos < bytes.len() && bytes[*pos] == b'\n' {
                *pos += 1;
                *line += 1;
                out.push('\n');
            } else {
                out.push('\r');
            }
            continue;
        }
        if b == b'\n' {
            *pos += 1;
            *line += 1;
            out.push('\n');
            continue;
        }
        let ch = input[*pos..].chars().next().expect("valid UTF-8 boundary");
        out.push(ch);
        *pos += ch.len_utf8();
    }

    Err(Error::with_line(ErrorKind::UnterminatedQuote, start_line))
}

fn parse_double_quoted(
    input: &str,
    pos: &mut usize,
    line: &mut usize,
    start_line: usize,
) -> Result<(String, Vec<usize>)> {
    let bytes = input.as_bytes();
    let mut out = String::new();
    let mut escaped_dollars = Vec::new();

    while *pos < bytes.len() {
        let b = bytes[*pos];
        if b == b'"' {
            *pos += 1;
            return Ok((out, escaped_dollars));
        }
        if b == b'\\' {
            *pos += 1;
            if *pos >= bytes.len() {
                return Err(Error::with_line(ErrorKind::UnterminatedQuote, start_line));
            }
            let esc = input[*pos..].chars().next().expect("valid UTF-8 boundary");
            *pos += esc.len_utf8();
            match esc {
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                '\\' => out.push('\\'),
                '"' => out.push('"'),
                '$' => {
                    escaped_dollars.push(out.len());
                    out.push('$');
                }
                other => {
                    return Err(Error::with_line(
                        ErrorKind::InvalidEscape {
                            escape: format!("\\{other}"),
                        },
                        *line,
                    ));
                }
            }
            continue;
        }
        if b == b'\r' {
            *pos += 1;
            if *pos < bytes.len() && bytes[*pos] == b'\n' {
                *pos += 1;
                *line += 1;
                out.push('\n');
            } else {
                out.push('\r');
            }
            continue;
        }
        if b == b'\n' {
            *pos += 1;
            *line += 1;
            out.push('\n');
            continue;
        }
        let ch = input[*pos..].chars().next().expect("valid UTF-8 boundary");
        out.push(ch);
        *pos += ch.len_utf8();
    }

    Err(Error::with_line(ErrorKind::UnterminatedQuote, start_line))
}

fn skip_trailing_comment(
    input: &str,
    pos: &mut usize,
    line: &mut usize,
    entry_line: usize,
) -> Result<()> {
    let bytes = input.as_bytes();
    while *pos < bytes.len() && (bytes[*pos] == b' ' || bytes[*pos] == b'\t') {
        *pos += 1;
    }
    if *pos < bytes.len() && bytes[*pos] == b'#' {
        while *pos < bytes.len() && bytes[*pos] != b'\n' {
            *pos += 1;
        }
    } else if *pos < bytes.len() && bytes[*pos] != b'\n' && bytes[*pos] != b'\r' {
        return Err(Error::with_line(ErrorKind::UnterminatedQuote, entry_line));
    }
    if *pos < bytes.len() && bytes[*pos] == b'\r' {
        *pos += 1;
    }
    if *pos < bytes.len() && bytes[*pos] == b'\n' {
        *pos += 1;
        *line += 1;
    }
    Ok(())
}

fn parse_unquoted(input: &str, pos: &mut usize, line: &mut usize) -> String {
    let bytes = input.as_bytes();
    let start = *pos;
    let mut end = start;

    while *pos < bytes.len() {
        let b = bytes[*pos];
        if b == b'\n' {
            end = *pos;
            *pos += 1;
            *line += 1;
            break;
        }
        if b == b'\r' {
            end = *pos;
            *pos += 1;
            if *pos < bytes.len() && bytes[*pos] == b'\n' {
                *pos += 1;
                *line += 1;
            }
            break;
        }
        if b == b'#' && *pos > 0 && (bytes[*pos - 1] == b' ' || bytes[*pos - 1] == b'\t') {
            end = *pos;
            while *pos < bytes.len() && bytes[*pos] != b'\n' {
                *pos += 1;
            }
            if *pos < bytes.len() && bytes[*pos] == b'\n' {
                *pos += 1;
                *line += 1;
            }
            break;
        }
        *pos += 1;
        end = *pos;
    }

    input[start..end].trim_end_matches([' ', '\t']).to_string()
}

/// Parse `${VAR}` or `${VAR:-default}` starting at `start` (just after `${`).
/// Returns `(closing_brace_byte_index, var_name, optional_default)`.
fn parse_braced_expr(template: &str, start: usize) -> Option<(usize, &str, Option<&str>)> {
    let bytes = template.as_bytes();
    let mut i = start;
    let mut depth = 1usize;
    let mut split_pos: Option<usize> = None;

    while i < bytes.len() {
        if bytes[i] == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
            depth += 1;
            i += 2;
            continue;
        }
        if bytes[i] == b'}' {
            depth -= 1;
            if depth == 0 {
                return match split_pos {
                    Some(colon) => {
                        let name = &template[start..colon];
                        let def = &template[colon + 2..i];
                        Some((i, name, Some(def)))
                    }
                    None => Some((i, &template[start..i], None)),
                };
            }
        } else if depth == 1
            && split_pos.is_none()
            && bytes[i] == b':'
            && i + 1 < bytes.len()
            && bytes[i + 1] == b'-'
        {
            split_pos = Some(i);
            i += 2;
            continue;
        }
        i += 1;
    }

    None
}

fn format_entry_value(entry: &Entry) -> String {
    let val = &entry.value;
    if !entry.allow_interpolation && !val.contains('\'') {
        return format!("'{val}'");
    }

    let needs_double_quotes = !entry.allow_interpolation
        || !entry.escaped_dollars.is_empty()
        || val.starts_with(char::is_whitespace)
        || val.ends_with(char::is_whitespace)
        || val.starts_with('"')
        || val.starts_with('\'')
        || val.starts_with('#')
        || val.contains(" #")
        || val.contains("\t#")
        || val.contains(['\n', '\r', '\t', '\\']);

    if !needs_double_quotes {
        return val.clone();
    }

    let mut out = String::with_capacity(val.len() + 2);
    out.push('"');
    for (idx, ch) in val.char_indices() {
        match ch {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '$' if !entry.allow_interpolation || entry.escaped_dollars.contains(&idx) => {
                out.push_str("\\$");
            }
            other => out.push(other),
        }
    }
    out.push('"');
    out
}
