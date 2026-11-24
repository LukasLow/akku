//! Configparser-style INI parsing, manipulation, and serialization.
//!
//! Insertion order is preserved for both sections and keys within each section.
//! Values are stored as strings with typed helpers for integers, floats,
//! booleans, and comma-separated lists.
//!
//! # Examples
//!
//! ```
//! use akku::ini;
//!
//! let doc = ini::parse("[server]\nhost = 127.0.0.1\nport = 8080\n").unwrap();
//! assert_eq!(doc.get("server.host"), Some("127.0.0.1"));
//! assert_eq!(doc.get_int("server.port").unwrap(), 8080);
//! ```

use std::fmt;
use std::io::{self, Write};
use std::ops::Index;
use std::str::FromStr;

/// Result type used by `akku::ini`.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Policy applied when a duplicate key is encountered in the same section.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DuplicatePolicy {
    /// Later definitions overwrite earlier values in-place.
    #[default]
    LastWins,
    /// Earlier definitions are kept and later duplicates are ignored.
    FirstWins,
    /// Duplicate keys cause a parse error ([`ErrorKind::DuplicateKey`]).
    Error,
}

/// Configuration options controlling INI parsing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IniOptions {
    /// Characters that start a full-line comment (and inline comments when enabled).
    pub comment_chars: [char; 2],
    /// Characters recognized as key-value delimiters.
    pub delimiters: &'static [char],
    /// Whether section names and keys are compared case-sensitively.
    pub case_sensitive: bool,
    /// Whether key-value pairs before the first `[section]` header are permitted.
    pub allow_global: bool,
    /// Whether whitespace is trimmed around keys, section names, and unquoted values.
    pub trim: bool,
    /// Whether comment characters terminate unquoted values mid-line.
    pub inline_comments: bool,
    /// Policy for handling duplicate keys in the same section.
    pub duplicate: DuplicatePolicy,
}

impl Default for IniOptions {
    fn default() -> Self {
        Self {
            comment_chars: [';', '#'],
            delimiters: &['='],
            case_sensitive: true,
            allow_global: true,
            trim: true,
            inline_comments: false,
            duplicate: DuplicatePolicy::LastWins,
        }
    }
}

/// Specific kind of INI error.
#[derive(Debug)]
#[non_exhaustive]
pub enum ErrorKind {
    /// Underlying I/O error.
    Io(io::Error),
    /// A non-empty, non-comment line lacked a key-value delimiter.
    MissingDelimiter {
        /// One-based line number where the delimiter was missing.
        line: usize,
    },
    /// A section header was empty (`[]` or whitespace only).
    EmptySectionName,
    /// A section header opened with `[` lacked a closing `]`.
    UnclosedSection,
    /// A section header contained nested brackets or a dot (`[a.b]`).
    NestedSection,
    /// A key name was empty or invalid (or a global key was disallowed).
    InvalidKey {
        /// The offending key text.
        key: String,
    },
    /// A quoted value lacked a matching closing quote on the line.
    UnterminatedQuote,
    /// A double-quoted value contained an unrecognized escape sequence.
    InvalidEscape {
        /// The offending escape sequence.
        escape: String,
    },
    /// A key appeared more than once when [`DuplicatePolicy::Error`] was active.
    DuplicateKey {
        /// The section name (`None` for the global section).
        section: Option<String>,
        /// The duplicated key name.
        key: String,
    },
    /// The requested key or path was not present.
    MissingKey {
        /// The requested lookup path.
        path: String,
    },
    /// A value could not be parsed as the requested type.
    WrongType {
        /// The requested lookup path.
        path: String,
        /// Human-readable expected type name.
        expected: &'static str,
        /// The raw value found.
        found: String,
    },
}

/// Error returned by INI parsing, writing, and typed lookups.
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
            ErrorKind::MissingDelimiter { line } => {
                write!(f, "missing key-value delimiter on line {line}")
            }
            ErrorKind::EmptySectionName => f.write_str("empty section name"),
            ErrorKind::UnclosedSection => f.write_str("unclosed section header"),
            ErrorKind::NestedSection => f.write_str("nested or dotted section name is not allowed"),
            ErrorKind::InvalidKey { key } => write!(f, "invalid key '{key}'"),
            ErrorKind::UnterminatedQuote => f.write_str("unterminated quoted value"),
            ErrorKind::InvalidEscape { escape } => {
                write!(f, "invalid escape sequence '{escape}'")
            }
            ErrorKind::DuplicateKey { section, key } => match section {
                Some(sec) => write!(f, "duplicate key '{key}' in section [{sec}]"),
                None => write!(f, "duplicate global key '{key}'"),
            },
            ErrorKind::MissingKey { path } => write!(f, "missing key '{path}'"),
            ErrorKind::WrongType {
                path,
                expected,
                found,
            } => write!(f, "expected {expected} for '{path}', found '{found}'"),
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

/// An insertion-ordered section of key-value pairs in an [`Ini`] document.
#[derive(Debug, Clone, Eq)]
pub struct Section {
    name: String,
    entries: Vec<(String, String)>,
    case_sensitive: bool,
}

impl PartialEq for Section {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.entries == other.entries
    }
}

impl Section {
    fn new_with_case(name: impl Into<String>, case_sensitive: bool) -> Self {
        Self {
            name: name.into(),
            entries: Vec::new(),
            case_sensitive,
        }
    }

    fn matches(&self, a: &str, b: &str) -> bool {
        if self.case_sensitive {
            a == b
        } else {
            a.eq_ignore_ascii_case(b)
        }
    }

    fn find_index(&self, key: &str) -> Option<usize> {
        self.entries.iter().position(|(k, _)| self.matches(k, key))
    }

    fn format_path(&self, key: &str) -> String {
        if self.name.is_empty() {
            key.to_string()
        } else {
            format!("{}.{key}", self.name)
        }
    }

    /// Return the section name (empty string for the global section).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Look up the string value for `key`.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.find_index(key).map(|i| self.entries[i].1.as_str())
    }

    /// Look up a mutable reference to the value for `key`.
    pub fn get_mut(&mut self, key: &str) -> Option<&mut String> {
        let idx = self.find_index(key)?;
        Some(&mut self.entries[idx].1)
    }

    /// Return `true` if `key` is present in this section.
    pub fn contains_key(&self, key: &str) -> bool {
        self.find_index(key).is_some()
    }

    /// Insert or update `key` with `value`, preserving existing key order when updating.
    pub fn set(&mut self, key: impl Into<String>, value: impl Into<String>) {
        let key = key.into();
        let value = value.into();
        if let Some(idx) = self.find_index(&key) {
            self.entries[idx].1 = value;
        } else {
            self.entries.push((key, value));
        }
    }

    /// Remove `key` from this section, returning its value if present.
    pub fn remove(&mut self, key: &str) -> Option<String> {
        let idx = self.find_index(key)?;
        Some(self.entries.remove(idx).1)
    }

    /// Return the number of entries in this section.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Return `true` if this section has no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Iterate over `(key, value)` pairs in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    /// Iterate over keys in insertion order.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|(k, _)| k.as_str())
    }

    /// Look up `key` and parse its value as an `i64`.
    pub fn get_int(&self, key: &str) -> Result<i64> {
        let val = self.get(key).ok_or_else(|| {
            Error::new(ErrorKind::MissingKey {
                path: self.format_path(key),
            })
        })?;
        val.trim().parse::<i64>().map_err(|_| {
            Error::new(ErrorKind::WrongType {
                path: self.format_path(key),
                expected: "integer",
                found: val.to_string(),
            })
        })
    }

    /// Look up `key` and parse its value as an `f64`.
    pub fn get_float(&self, key: &str) -> Result<f64> {
        let val = self.get(key).ok_or_else(|| {
            Error::new(ErrorKind::MissingKey {
                path: self.format_path(key),
            })
        })?;
        val.trim().parse::<f64>().map_err(|_| {
            Error::new(ErrorKind::WrongType {
                path: self.format_path(key),
                expected: "float",
                found: val.to_string(),
            })
        })
    }

    /// Look up `key` and parse its value as a boolean (`true`/`yes`/`on`/`1` or
    /// `false`/`no`/`off`/`0`, case-insensitive).
    pub fn get_bool(&self, key: &str) -> Result<bool> {
        let val = self.get(key).ok_or_else(|| {
            Error::new(ErrorKind::MissingKey {
                path: self.format_path(key),
            })
        })?;
        parse_bool_value(val).ok_or_else(|| {
            Error::new(ErrorKind::WrongType {
                path: self.format_path(key),
                expected: "boolean",
                found: val.to_string(),
            })
        })
    }

    /// Look up `key` and split its value as a comma-separated list of trimmed items.
    pub fn get_list(&self, key: &str) -> Result<Vec<String>> {
        let val = self.get(key).ok_or_else(|| {
            Error::new(ErrorKind::MissingKey {
                path: self.format_path(key),
            })
        })?;
        Ok(parse_list_value(val))
    }
}

impl Index<&str> for Section {
    type Output = str;

    fn index(&self, key: &str) -> &Self::Output {
        self.get(key)
            .unwrap_or_else(|| panic!("no entry found for key '{key}'"))
    }
}

/// An insertion-ordered INI document consisting of a global section and named sections.
#[derive(Debug, Clone, Eq)]
pub struct Ini {
    global: Section,
    sections: Vec<Section>,
    case_sensitive: bool,
}

impl Default for Ini {
    fn default() -> Self {
        Self::new()
    }
}

impl PartialEq for Ini {
    fn eq(&self, other: &Self) -> bool {
        self.global == other.global && self.sections == other.sections
    }
}

impl AsRef<Ini> for Ini {
    fn as_ref(&self) -> &Ini {
        self
    }
}

impl Ini {
    /// Create an empty, case-sensitive [`Ini`] document.
    pub fn new() -> Self {
        Self::with_case_sensitivity(true)
    }

    fn with_case_sensitivity(case_sensitive: bool) -> Self {
        Self {
            global: Section::new_with_case("", case_sensitive),
            sections: Vec::new(),
            case_sensitive,
        }
    }

    fn matches(&self, a: &str, b: &str) -> bool {
        if self.case_sensitive {
            a == b
        } else {
            a.eq_ignore_ascii_case(b)
        }
    }

    fn find_section_index(&self, name: &str) -> Option<usize> {
        self.sections
            .iter()
            .position(|s| self.matches(&s.name, name))
    }

    /// Borrow the global (unnamed) section.
    pub fn global(&self) -> &Section {
        &self.global
    }

    /// Mutably borrow the global (unnamed) section.
    pub fn global_mut(&mut self) -> &mut Section {
        &mut self.global
    }

    /// Look up a named section.
    pub fn section(&self, name: &str) -> Option<&Section> {
        self.find_section_index(name).map(|i| &self.sections[i])
    }

    /// Mutably look up a named section.
    pub fn section_mut(&mut self, name: &str) -> Option<&mut Section> {
        let idx = self.find_section_index(name)?;
        Some(&mut self.sections[idx])
    }

    /// Look up a named section, inserting a new empty section if it does not exist.
    pub fn section_or_insert(&mut self, name: &str) -> &mut Section {
        if let Some(idx) = self.find_section_index(name) {
            &mut self.sections[idx]
        } else {
            self.sections
                .push(Section::new_with_case(name, self.case_sensitive));
            let last = self.sections.len() - 1;
            &mut self.sections[last]
        }
    }

    /// Remove a named section, returning it if it existed.
    pub fn remove_section(&mut self, name: &str) -> Option<Section> {
        let idx = self.find_section_index(name)?;
        Some(self.sections.remove(idx))
    }

    /// Iterate over all named sections in insertion order.
    pub fn sections(&self) -> impl Iterator<Item = &Section> {
        self.sections.iter()
    }

    /// Iterate over all named section names in insertion order.
    pub fn section_names(&self) -> impl Iterator<Item = &str> {
        self.sections.iter().map(|s| s.name.as_str())
    }

    /// Return the number of named sections in this document.
    pub fn len(&self) -> usize {
        self.sections.len()
    }

    /// Return `true` if there are neither global keys nor named sections.
    pub fn is_empty(&self) -> bool {
        self.global.is_empty() && self.sections.is_empty()
    }

    /// Look up a value by path.
    ///
    /// If `path` contains `.`, it is split on the **first** `.` only so keys
    /// containing dots (such as `section.a.b`) resolve inside `section`.
    /// Otherwise `path` is looked up in the global section.
    pub fn get(&self, path: &str) -> Option<&str> {
        if let Some((sec, key)) = path.split_once('.') {
            self.section(sec).and_then(|s| s.get(key))
        } else {
            self.global.get(path)
        }
    }

    /// Look up `path` and parse its value as an `i64`.
    pub fn get_int(&self, path: &str) -> Result<i64> {
        let val = self.get(path).ok_or_else(|| {
            Error::new(ErrorKind::MissingKey {
                path: path.to_string(),
            })
        })?;
        val.trim().parse::<i64>().map_err(|_| {
            Error::new(ErrorKind::WrongType {
                path: path.to_string(),
                expected: "integer",
                found: val.to_string(),
            })
        })
    }

    /// Look up `path` and parse its value as an `f64`.
    pub fn get_float(&self, path: &str) -> Result<f64> {
        let val = self.get(path).ok_or_else(|| {
            Error::new(ErrorKind::MissingKey {
                path: path.to_string(),
            })
        })?;
        val.trim().parse::<f64>().map_err(|_| {
            Error::new(ErrorKind::WrongType {
                path: path.to_string(),
                expected: "float",
                found: val.to_string(),
            })
        })
    }

    /// Look up `path` and parse its value as a boolean (`true`/`yes`/`on`/`1` or
    /// `false`/`no`/`off`/`0`, case-insensitive).
    pub fn get_bool(&self, path: &str) -> Result<bool> {
        let val = self.get(path).ok_or_else(|| {
            Error::new(ErrorKind::MissingKey {
                path: path.to_string(),
            })
        })?;
        parse_bool_value(val).ok_or_else(|| {
            Error::new(ErrorKind::WrongType {
                path: path.to_string(),
                expected: "boolean",
                found: val.to_string(),
            })
        })
    }

    /// Look up `path` and split its value as a comma-separated list of trimmed items.
    pub fn get_list(&self, path: &str) -> Result<Vec<String>> {
        let val = self.get(path).ok_or_else(|| {
            Error::new(ErrorKind::MissingKey {
                path: path.to_string(),
            })
        })?;
        Ok(parse_list_value(val))
    }

    /// Merge another [`Ini`] document into `self`, overwriting matching keys in-place
    /// and appending new keys and sections in order.
    pub fn merge(&mut self, other: impl AsRef<Ini>) {
        let other = other.as_ref();
        for (k, v) in other.global.iter() {
            self.global.set(k, v);
        }
        for sec in other.sections() {
            let target = self.section_or_insert(sec.name());
            for (k, v) in sec.iter() {
                target.set(k, v);
            }
        }
    }
}

impl FromStr for Ini {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        parse(s)
    }
}

impl fmt::Display for Ini {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut buf = Vec::new();
        write_to(self, &mut buf).map_err(|_| fmt::Error)?;
        let s = String::from_utf8(buf).map_err(|_| fmt::Error)?;
        f.write_str(&s)
    }
}

/// Parse an INI document using [`IniOptions::default()`].
pub fn parse(input: &str) -> Result<Ini> {
    parse_with(input, IniOptions::default())
}

/// Parse an INI document using the given [`IniOptions`].
pub fn parse_with(input: &str, options: IniOptions) -> Result<Ini> {
    let input = input.strip_prefix('\u{FEFF}').unwrap_or(input);
    let mut ini = Ini::with_case_sensitivity(options.case_sensitive);
    let mut current_section: Option<String> = None;

    for (idx, raw_line) in input.lines().enumerate() {
        let line_num = idx + 1;
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        let trimmed_start = line.trim_start();

        if trimmed_start.is_empty() {
            continue;
        }
        if let Some(first) = trimmed_start.chars().next() {
            if options.comment_chars.contains(&first) {
                continue;
            }
            if first == '[' {
                let name = parse_section_header(trimmed_start, line_num, &options)?;
                ini.section_or_insert(&name);
                current_section = Some(name);
                continue;
            }
        }

        let (key, value) = parse_key_value(line, line_num, &options)?;
        if current_section.is_none() && !options.allow_global {
            return Err(Error::with_line(ErrorKind::InvalidKey { key }, line_num));
        }

        let target_sec = match &current_section {
            Some(name) => ini.section_or_insert(name),
            None => ini.global_mut(),
        };

        if target_sec.contains_key(&key) {
            match options.duplicate {
                DuplicatePolicy::LastWins => target_sec.set(key, value),
                DuplicatePolicy::FirstWins => {}
                DuplicatePolicy::Error => {
                    return Err(Error::with_line(
                        ErrorKind::DuplicateKey {
                            section: current_section,
                            key,
                        },
                        line_num,
                    ));
                }
            }
        } else {
            target_sec.set(key, value);
        }
    }

    Ok(ini)
}

/// Serialize `ini` into a formatted INI [`String`].
pub fn to_string(ini: &Ini) -> String {
    let mut buf = Vec::new();
    write_to(ini, &mut buf).expect("writing to Vec<u8> cannot fail");
    String::from_utf8(buf).expect("INI serialization produces valid UTF-8")
}

/// Write `ini` in INI format to `writer`.
pub fn write_to<W: Write>(ini: &Ini, mut writer: W) -> Result<()> {
    let mut wrote_block = false;

    if !ini.global.is_empty() {
        for (k, v) in ini.global.iter() {
            writeln!(writer, "{k} = {}", format_value(v))?;
        }
        wrote_block = true;
    }

    for sec in ini.sections() {
        if wrote_block {
            writeln!(writer)?;
        }
        writeln!(writer, "[{}]", sec.name())?;
        for (k, v) in sec.iter() {
            writeln!(writer, "{k} = {}", format_value(v))?;
        }
        wrote_block = true;
    }

    Ok(())
}

fn parse_section_header(
    trimmed_start: &str,
    line_num: usize,
    options: &IniOptions,
) -> Result<String> {
    let rest = &trimmed_start[1..];
    let Some(close_pos) = rest.find(']') else {
        return Err(Error::with_line(ErrorKind::UnclosedSection, line_num));
    };

    let raw_name = &rest[..close_pos];
    let after = rest[close_pos + 1..].trim_start();
    if !after.is_empty() {
        let is_comment = after
            .chars()
            .next()
            .is_some_and(|c| options.comment_chars.contains(&c));
        if !is_comment {
            if after.contains('[') || after.contains(']') {
                return Err(Error::with_line(ErrorKind::NestedSection, line_num));
            }
            return Err(Error::with_line(ErrorKind::UnclosedSection, line_num));
        }
    }

    if raw_name.contains('[') || raw_name.contains('.') {
        return Err(Error::with_line(ErrorKind::NestedSection, line_num));
    }

    let name = if options.trim {
        raw_name.trim()
    } else {
        raw_name
    };

    if name.is_empty() {
        return Err(Error::with_line(ErrorKind::EmptySectionName, line_num));
    }

    Ok(name.to_string())
}

fn parse_key_value(line: &str, line_num: usize, options: &IniOptions) -> Result<(String, String)> {
    let delim_pos = line.find(|c: char| options.delimiters.contains(&c));
    let Some(pos) = delim_pos else {
        return Err(Error::with_line(
            ErrorKind::MissingDelimiter { line: line_num },
            line_num,
        ));
    };

    let raw_key = &line[..pos];
    let key = if options.trim {
        raw_key.trim()
    } else {
        raw_key
    };

    if key.is_empty() || key.chars().any(char::is_control) {
        return Err(Error::with_line(
            ErrorKind::InvalidKey {
                key: key.to_string(),
            },
            line_num,
        ));
    }

    let delim_len = line[pos..].chars().next().map_or(1, char::len_utf8);
    let raw_val = &line[pos + delim_len..];
    let val_start = if options.trim {
        raw_val.trim_start()
    } else {
        raw_val
    };

    let value = if let Some(after_quote) = val_start.strip_prefix('"') {
        parse_quoted_double(after_quote, line_num, options)?
    } else if let Some(after_quote) = val_start.strip_prefix('\'') {
        parse_quoted_single(after_quote, line_num, options)?
    } else {
        let mut s = val_start;
        if options.inline_comments {
            if let Some(comment_idx) = s.find(|c: char| options.comment_chars.contains(&c)) {
                s = &s[..comment_idx];
            }
        }
        if options.trim {
            s.trim_end().to_string()
        } else {
            s.to_string()
        }
    };

    Ok((key.to_string(), value))
}

fn parse_quoted_double(input: &str, line_num: usize, options: &IniOptions) -> Result<String> {
    let mut out = String::new();
    let mut chars = input.char_indices();

    while let Some((idx, ch)) = chars.next() {
        match ch {
            '"' => {
                let rest = input[idx + 1..].trim_start();
                if !rest.is_empty() {
                    let is_comment = options.inline_comments
                        && rest
                            .chars()
                            .next()
                            .is_some_and(|c| options.comment_chars.contains(&c));
                    if !is_comment {
                        return Err(Error::with_line(ErrorKind::UnterminatedQuote, line_num));
                    }
                }
                return Ok(out);
            }
            '\\' => {
                let Some((_, esc)) = chars.next() else {
                    return Err(Error::with_line(ErrorKind::UnterminatedQuote, line_num));
                };
                match esc {
                    'n' => out.push('\n'),
                    'r' => out.push('\r'),
                    't' => out.push('\t'),
                    '\\' => out.push('\\'),
                    '"' => out.push('"'),
                    '\'' => out.push('\''),
                    ';' => out.push(';'),
                    '#' => out.push('#'),
                    other => {
                        return Err(Error::with_line(
                            ErrorKind::InvalidEscape {
                                escape: format!("\\{other}"),
                            },
                            line_num,
                        ));
                    }
                }
            }
            other => out.push(other),
        }
    }

    Err(Error::with_line(ErrorKind::UnterminatedQuote, line_num))
}

fn parse_quoted_single(input: &str, line_num: usize, options: &IniOptions) -> Result<String> {
    let mut out = String::new();
    for (idx, ch) in input.char_indices() {
        if ch == '\'' {
            let rest = input[idx + 1..].trim_start();
            if !rest.is_empty() {
                let is_comment = options.inline_comments
                    && rest
                        .chars()
                        .next()
                        .is_some_and(|c| options.comment_chars.contains(&c));
                if !is_comment {
                    return Err(Error::with_line(ErrorKind::UnterminatedQuote, line_num));
                }
            }
            return Ok(out);
        }
        out.push(ch);
    }

    Err(Error::with_line(ErrorKind::UnterminatedQuote, line_num))
}

fn parse_bool_value(val: &str) -> Option<bool> {
    match val.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Some(true),
        "false" | "no" | "off" | "0" => Some(false),
        _ => None,
    }
}

fn parse_list_value(val: &str) -> Vec<String> {
    if val.trim().is_empty() {
        return Vec::new();
    }
    val.split(',')
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
        .collect()
}

fn format_value(val: &str) -> String {
    if val.is_empty() {
        return String::new();
    }
    let needs_quotes = val.starts_with(char::is_whitespace)
        || val.ends_with(char::is_whitespace)
        || val.starts_with('"')
        || val.starts_with('\'')
        || val.contains(['\n', '\r', '\t', '\\', '"', ';', '#']);

    if !needs_quotes {
        return val.to_string();
    }

    let mut out = String::with_capacity(val.len() + 2);
    out.push('"');
    for ch in val.chars() {
        match ch {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}
