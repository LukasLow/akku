//! Deterministic module catalog generator and checker (`akku catalog gen` / `akku catalog check`).
//!
//! Pure `std`, zero external dependencies.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

const RUST_KEYWORDS: &[&str] = &[
    "async", "gen", "match", "type", "mod", "fn", "let", "struct", "enum", "trait", "impl", "use",
    "pub", "ref", "move", "loop", "while", "for", "if", "else", "return", "break", "continue",
    "const", "static", "unsafe", "extern", "crate", "self", "super", "where", "as", "in", "dyn",
    "try", "yield", "await", "box", "macro", "abstract", "become", "do", "final", "override",
    "priv", "typeof", "unsized", "virtual",
];

const MAX_SUMMARY_BYTES: usize = 140;
const MAX_TAGS: usize = 8;

/// Module implementation status in the Akku catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Native,
    Fork,
    Adapter,
}

impl Status {
    fn as_str(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Fork => "fork",
            Self::Adapter => "adapter",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "native" => Some(Self::Native),
            "fork" => Some(Self::Fork),
            "adapter" => Some(Self::Adapter),
            _ => None,
        }
    }
}

/// Validated metadata for a single `crates/akku/src/<name>` module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleMeta {
    pub name: String,
    pub family: Option<String>,
    pub status: Status,
    pub wasm32: bool,
    pub feature: Option<String>,
    pub wave: u8,
    pub summary: String,
    pub owners: Vec<String>,
    pub tags: Vec<String>,
    pub source: Option<String>,
}

impl ModuleMeta {
    /// Returns the relative repository path for this module.
    pub fn path(&self) -> String {
        format!("crates/akku/src/{}", self.name)
    }

    /// Computes deterministic derived catalog tags for this module.
    pub fn derived_tags(&self) -> Vec<String> {
        let mut out = Vec::new();
        match self.status {
            Status::Native if self.feature.is_none() => out.push("native-0dep".to_string()),
            Status::Native => out.push("native".to_string()),
            Status::Fork => out.push("fork".to_string()),
            Status::Adapter => out.push("adapter".to_string()),
        }
        if self.wasm32 {
            out.push("wasm32".to_string());
        }
        if self.feature.is_some() {
            out.push("optional-feature".to_string());
        }
        if let Some(ref family) = self.family {
            out.push(format!("family:{family}"));
        }
        out
    }
}

/// Generated file bundle produced by [`build_outputs`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedFiles {
    pub catalog_json: String,
    pub catalog_md: String,
    pub codeowners: String,
}

/// Generates `docs/catalog.json`, `docs/CATALOG.md`, and `.github/CODEOWNERS` under `root`.
pub fn generate(root: &Path) -> Result<usize, String> {
    let modules = discover_and_validate(root)?;
    let outputs = build_outputs(&modules);

    let targets = [
        (root.join("docs/catalog.json"), &outputs.catalog_json),
        (root.join("docs/CATALOG.md"), &outputs.catalog_md),
        (root.join(".github/CODEOWNERS"), &outputs.codeowners),
    ];

    for (path, content) in targets {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|err| format!("cannot create directory `{}`: {err}", parent.display()))?;
        }
        fs::write(&path, content.as_bytes())
            .map_err(|err| format!("cannot write `{}`: {err}", path.display()))?;
    }

    Ok(modules.len())
}

/// Validates all `meta.toml` + `lib.rs` sync and verifies generated files match disk.
pub fn check(root: &Path) -> Result<usize, String> {
    let modules = discover_and_validate(root)?;
    let outputs = build_outputs(&modules);

    let targets = [
        ("docs/catalog.json", &outputs.catalog_json),
        ("docs/CATALOG.md", &outputs.catalog_md),
        (".github/CODEOWNERS", &outputs.codeowners),
    ];

    let mut problems = Vec::new();
    for (rel, expected) in targets {
        let full = root.join(rel);
        match fs::read_to_string(&full) {
            Ok(actual) => {
                if actual != *expected {
                    problems.push(format!(
                        "{rel}: out of date ({})",
                        first_diff_hint(&actual, expected)
                    ));
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                problems.push(format!("{rel}: missing"));
            }
            Err(err) => {
                problems.push(format!("{rel}: cannot read ({err})"));
            }
        }
    }

    if !problems.is_empty() {
        return Err(format!(
            "catalog check failed; run `akku catalog gen` to update:\n  - {}",
            problems.join("\n  - ")
        ));
    }

    Ok(modules.len())
}

fn first_diff_hint(actual: &str, expected: &str) -> String {
    let mut actual_lines = actual.lines();
    let mut expected_lines = expected.lines();
    let mut line_no = 1usize;
    loop {
        match (actual_lines.next(), expected_lines.next()) {
            (Some(a), Some(e)) if a == e => line_no += 1,
            (Some(_), Some(_)) => return format!("first difference at line {line_no}"),
            (Some(_), None) => return format!("extra trailing content at line {line_no}"),
            (None, Some(_)) => return format!("missing content at line {line_no}"),
            (None, None) => return "trailing newline mismatch".to_string(),
        }
    }
}

/// Discovers and validates all `meta.toml` files and checks sync with `crates/akku/src/lib.rs`.
pub fn discover_and_validate(root: &Path) -> Result<Vec<ModuleMeta>, String> {
    let src_dir = root.join("crates/akku/src");
    let lib_rs_path = src_dir.join("lib.rs");

    if !src_dir.is_dir() {
        return Err(format!(
            "module source directory not found: `{}`",
            src_dir.display()
        ));
    }

    let lib_rs_content = fs::read_to_string(&lib_rs_path)
        .map_err(|err| format!("cannot read `{}`: {err}", lib_rs_path.display()))?;
    let declared_mods = extract_pub_mods(&lib_rs_content);

    let mut meta_entries: Vec<(String, PathBuf)> = Vec::new();
    let dir_iter = fs::read_dir(&src_dir)
        .map_err(|err| format!("cannot read `{}`: {err}", src_dir.display()))?;

    for entry in dir_iter {
        let entry = entry.map_err(|err| format!("error reading `{}`: {err}", src_dir.display()))?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let meta_path = path.join("meta.toml");
        if meta_path.is_file() {
            let folder = entry.file_name().to_string_lossy().into_owned();
            meta_entries.push((folder, meta_path));
        }
    }
    meta_entries.sort_by(|a, b| a.0.cmp(&b.0));

    let meta_names: BTreeSet<String> = meta_entries.iter().map(|(n, _)| n.clone()).collect();

    let mut sync_errors = Vec::new();
    for declared in &declared_mods {
        if !meta_names.contains(declared) {
            sync_errors.push(format!(
                "`pub mod {declared};` declared in `{}` is missing `crates/akku/src/{declared}/meta.toml`",
                lib_rs_path.display()
            ));
        }
    }
    for found in &meta_names {
        if !declared_mods.contains(found) {
            sync_errors.push(format!(
                "`crates/akku/src/{found}/meta.toml` exists but `pub mod {found};` is not declared in `{}`",
                lib_rs_path.display()
            ));
        }
    }
    if !sync_errors.is_empty() {
        return Err(format!(
            "lib.rs and meta.toml are out of sync:\n  - {}",
            sync_errors.join("\n  - ")
        ));
    }

    let mut modules = Vec::with_capacity(meta_entries.len());
    for (folder, meta_path) in meta_entries {
        let raw = fs::read_to_string(&meta_path)
            .map_err(|err| format!("cannot read `{}`: {err}", meta_path.display()))?;
        let meta = parse_and_validate_meta(&raw, &folder, &meta_path)?;
        modules.push(meta);
    }

    modules.sort_by(|a, b| a.wave.cmp(&b.wave).then_with(|| a.name.cmp(&b.name)));
    Ok(modules)
}

/// Extracts all top-level `pub mod <ident>;` declarations from `lib.rs` (excluding inline
/// `pub mod <ident> { ... }` blocks, comments, and string literals).
fn extract_pub_mods(src: &str) -> BTreeSet<String> {
    let bytes = src.as_bytes();
    let len = bytes.len();
    let mut i = 0usize;
    let mut brace_depth = 0usize;
    let mut tokens: Vec<(String, usize)> = Vec::new();

    while i < len {
        let b = bytes[i];
        // Whitespace
        if b.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        // Line comment
        if b == b'/' && i + 1 < len && bytes[i + 1] == b'/' {
            i += 2;
            while i < len && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        // Nested block comment
        if b == b'/' && i + 1 < len && bytes[i + 1] == b'*' {
            i += 2;
            let mut depth = 1usize;
            while i < len && depth > 0 {
                if bytes[i] == b'/' && i + 1 < len && bytes[i + 1] == b'*' {
                    depth += 1;
                    i += 2;
                } else if bytes[i] == b'*' && i + 1 < len && bytes[i + 1] == b'/' {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            continue;
        }
        // Raw string literal r#"..."# or br#"..."#
        if (b == b'r' || (b == b'b' && i + 1 < len && bytes[i + 1] == b'r'))
            && let_raw_string_end(bytes, i).is_some()
        {
            if let Some(end) = let_raw_string_end(bytes, i) {
                i = end;
                continue;
            }
        }
        // Normal string literal
        if b == b'"' {
            i += 1;
            while i < len {
                if bytes[i] == b'\\' && i + 1 < len {
                    i += 2;
                } else if bytes[i] == b'"' {
                    i += 1;
                    break;
                } else {
                    i += 1;
                }
            }
            continue;
        }
        // Char literal (avoid breaking on lifetimes like `'a`)
        if b == b'\'' {
            if i + 2 < len && bytes[i + 1] == b'\\' {
                // Escaped char literal such as `'\n'` or `'\''`
                let mut j = i + 2;
                while j < len && bytes[j] != b'\'' && bytes[j] != b'\n' {
                    j += 1;
                }
                if j < len && bytes[j] == b'\'' {
                    i = j + 1;
                    continue;
                }
            } else if i + 2 < len && bytes[i + 2] == b'\'' {
                i += 3;
                continue;
            }
            i += 1;
            continue;
        }
        if b == b'{' {
            tokens.push(("{".to_string(), brace_depth));
            brace_depth += 1;
            i += 1;
            continue;
        }
        if b == b'}' {
            brace_depth = brace_depth.saturating_sub(1);
            tokens.push(("}".to_string(), brace_depth));
            i += 1;
            continue;
        }
        if b == b';' {
            tokens.push((";".to_string(), brace_depth));
            i += 1;
            continue;
        }
        if b.is_ascii_alphabetic() || b == b'_' {
            let start = i;
            i += 1;
            while i < len && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            tokens.push((src[start..i].to_string(), brace_depth));
            continue;
        }
        // Other punctuation token
        tokens.push(((b as char).to_string(), brace_depth));
        i += 1;
    }

    let mut out = BTreeSet::new();
    let mut idx = 0usize;
    while idx + 3 < tokens.len() {
        let (ref t0, d0) = tokens[idx];
        let (ref t1, d1) = tokens[idx + 1];
        let (ref t2, d2) = tokens[idx + 2];
        let (ref t3, d3) = tokens[idx + 3];
        if d0 == 0
            && d1 == 0
            && d2 == 0
            && d3 == 0
            && t0 == "pub"
            && t1 == "mod"
            && t3 == ";"
            && is_ident(t2)
        {
            out.insert(t2.clone());
            idx += 4;
        } else {
            idx += 1;
        }
    }
    out
}

fn let_raw_string_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut i = start;
    if bytes.get(i) == Some(&b'b') {
        i += 1;
    }
    if bytes.get(i) != Some(&b'r') {
        return None;
    }
    i += 1;
    let mut hashes = 0usize;
    while bytes.get(i) == Some(&b'#') {
        hashes += 1;
        i += 1;
    }
    if bytes.get(i) != Some(&b'"') {
        return None;
    }
    i += 1;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            let mut matched = 0usize;
            while matched < hashes && bytes.get(i + 1 + matched) == Some(&b'#') {
                matched += 1;
            }
            if matched == hashes {
                return Some(i + 1 + hashes);
            }
        }
        i += 1;
    }
    Some(bytes.len())
}

fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

// ---------------------------------------------------------------------------
// Strict flat-TOML parser for meta.toml
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum TomlValue {
    Str(String),
    Int(i64),
    Bool(bool),
    StrArray(Vec<String>),
}

struct TomlParser<'a> {
    src: &'a str,
    bytes: &'a [u8],
    pos: usize,
    line: usize,
    path: &'a Path,
}

impl<'a> TomlParser<'a> {
    fn new(src: &'a str, path: &'a Path) -> Self {
        Self {
            src,
            bytes: src.as_bytes(),
            pos: 0,
            line: 1,
            path,
        }
    }

    fn err(&self, line: usize, msg: impl Into<String>) -> String {
        format!("{}:{line}: {}", self.path.display(), msg.into())
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let b = self.peek()?;
        self.pos += 1;
        if b == b'\n' {
            self.line += 1;
        }
        Some(b)
    }

    fn skip_horizontal_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t')) {
            self.pos += 1;
        }
    }

    fn skip_ws_and_comments(&mut self) {
        loop {
            while let Some(b) = self.peek() {
                if b == b' ' || b == b'\t' || b == b'\r' || b == b'\n' {
                    self.bump();
                } else {
                    break;
                }
            }
            if self.peek() == Some(b'#') {
                while let Some(b) = self.bump() {
                    if b == b'\n' {
                        break;
                    }
                }
            } else {
                break;
            }
        }
    }

    fn expect_line_end(&mut self) -> Result<(), String> {
        self.skip_horizontal_ws();
        match self.peek() {
            None => Ok(()),
            Some(b'\n') => {
                self.bump();
                Ok(())
            }
            Some(b'\r') => {
                self.bump();
                if self.peek() == Some(b'\n') {
                    self.bump();
                }
                Ok(())
            }
            Some(b'#') => {
                while let Some(b) = self.bump() {
                    if b == b'\n' {
                        break;
                    }
                }
                Ok(())
            }
            Some(other) => Err(self.err(
                self.line,
                format!("unexpected trailing character '{}'", other as char),
            )),
        }
    }

    fn parse_table(&mut self) -> Result<BTreeMap<String, (usize, TomlValue)>, String> {
        let mut map: BTreeMap<String, (usize, TomlValue)> = BTreeMap::new();
        loop {
            self.skip_ws_and_comments();
            let Some(b) = self.peek() else {
                break;
            };
            let key_line = self.line;
            if b == b'[' {
                return Err(self.err(
                    key_line,
                    "TOML table sections `[...]` are not allowed in flat meta.toml",
                ));
            }
            let key = self.parse_bare_key()?;
            self.skip_horizontal_ws();
            if self.peek() == Some(b'.') {
                return Err(self.err(
                    key_line,
                    format!("dotted keys like `{key}.*` are not allowed in flat meta.toml"),
                ));
            }
            if self.bump() != Some(b'=') {
                return Err(self.err(key_line, format!("expected `=` after key `{key}`")));
            }
            self.skip_horizontal_ws();
            let val = self.parse_value()?;
            self.expect_line_end()?;

            if map.contains_key(&key) {
                return Err(self.err(key_line, format!("duplicate key `{key}`")));
            }
            map.insert(key, (key_line, val));
        }
        Ok(map)
    }

    fn parse_bare_key(&mut self) -> Result<String, String> {
        let start = self.pos;
        let line = self.line;
        while let Some(b) = self.peek() {
            if b.is_ascii_alphanumeric() || b == b'_' || b == b'-' {
                self.pos += 1;
            } else {
                break;
            }
        }
        if self.pos == start {
            return Err(self.err(line, "expected key name"));
        }
        Ok(self.src[start..self.pos].to_string())
    }

    fn parse_value(&mut self) -> Result<TomlValue, String> {
        let line = self.line;
        let Some(b) = self.peek() else {
            return Err(self.err(line, "expected value"));
        };
        match b {
            b'"' => Ok(TomlValue::Str(self.parse_basic_string()?)),
            b'\'' => Ok(TomlValue::Str(self.parse_literal_string()?)),
            b'[' => Ok(TomlValue::StrArray(self.parse_string_array()?)),
            b't' | b'f' => self.parse_bool(),
            b'-' | b'+' | b'0'..=b'9' => self.parse_int(),
            other => Err(self.err(
                line,
                format!("unexpected start of value '{}'", other as char),
            )),
        }
    }

    fn parse_basic_string(&mut self) -> Result<String, String> {
        let start_line = self.line;
        self.bump(); // opening '"'
        let mut out = String::new();
        loop {
            let Some(b) = self.bump() else {
                return Err(self.err(start_line, "unterminated string literal"));
            };
            match b {
                b'"' => return Ok(out),
                b'\n' | b'\r' => {
                    return Err(self.err(start_line, "unterminated single-line string literal"));
                }
                b'\\' => {
                    let Some(esc) = self.bump() else {
                        return Err(self.err(self.line, "unterminated escape sequence"));
                    };
                    match esc {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'b' => out.push('\u{0008}'),
                        b'f' => out.push('\u{000C}'),
                        b'u' => out.push(self.parse_unicode_escape(4)?),
                        b'U' => out.push(self.parse_unicode_escape(8)?),
                        other => {
                            return Err(self.err(
                                self.line,
                                format!("unknown escape sequence '\\{}'", other as char),
                            ));
                        }
                    }
                }
                c if c < 0x20 && c != b'\t' => {
                    return Err(self.err(self.line, "unescaped control character in string"));
                }
                _ => {
                    // Re-decode UTF-8 scalar starting at `self.pos - 1`.
                    let start = self.pos - 1;
                    let ch = self.src[start..]
                        .chars()
                        .next()
                        .ok_or_else(|| self.err(self.line, "invalid UTF-8 in string"))?;
                    self.pos = start + ch.len_utf8();
                    out.push(ch);
                }
            }
        }
    }

    fn parse_unicode_escape(&mut self, digits: usize) -> Result<char, String> {
        let line = self.line;
        if self.pos + digits > self.bytes.len() {
            return Err(self.err(line, "truncated unicode escape"));
        }
        let hex = &self.src[self.pos..self.pos + digits];
        self.pos += digits;
        let code = u32::from_str_radix(hex, 16)
            .map_err(|_| self.err(line, format!("invalid unicode escape '{hex}'")))?;
        char::from_u32(code)
            .ok_or_else(|| self.err(line, format!("invalid unicode scalar U+{code:04X}")))
    }

    fn parse_literal_string(&mut self) -> Result<String, String> {
        let start_line = self.line;
        self.bump(); // opening '\''
        let start = self.pos;
        while let Some(b) = self.bump() {
            if b == b'\'' {
                return Ok(self.src[start..self.pos - 1].to_string());
            }
            if b == b'\n' || b == b'\r' {
                return Err(self.err(start_line, "unterminated literal string"));
            }
        }
        Err(self.err(start_line, "unterminated literal string"))
    }

    fn parse_string_array(&mut self) -> Result<Vec<String>, String> {
        let start_line = self.line;
        self.bump(); // '['
        let mut items = Vec::new();
        loop {
            self.skip_ws_and_comments();
            let Some(b) = self.peek() else {
                return Err(self.err(start_line, "unterminated array"));
            };
            if b == b']' {
                self.bump();
                return Ok(items);
            }
            let item_line = self.line;
            let item = match b {
                b'"' => self.parse_basic_string()?,
                b'\'' => self.parse_literal_string()?,
                _ => {
                    return Err(self.err(item_line, "array elements must be strings"));
                }
            };
            items.push(item);

            self.skip_ws_and_comments();
            match self.peek() {
                Some(b',') => {
                    self.bump();
                }
                Some(b']') => {
                    self.bump();
                    return Ok(items);
                }
                _ => {
                    return Err(self.err(self.line, "expected `,` or `]` in array"));
                }
            }
        }
    }

    fn parse_bool(&mut self) -> Result<TomlValue, String> {
        let line = self.line;
        let rest = &self.src[self.pos..];
        if rest.starts_with("true") && is_value_boundary(self.bytes.get(self.pos + 4).copied()) {
            self.pos += 4;
            Ok(TomlValue::Bool(true))
        } else if rest.starts_with("false")
            && is_value_boundary(self.bytes.get(self.pos + 5).copied())
        {
            self.pos += 5;
            Ok(TomlValue::Bool(false))
        } else {
            Err(self.err(line, "invalid boolean literal"))
        }
    }

    fn parse_int(&mut self) -> Result<TomlValue, String> {
        let line = self.line;
        let start = self.pos;
        if matches!(self.peek(), Some(b'+' | b'-')) {
            self.pos += 1;
        }
        while let Some(b) = self.peek() {
            if b.is_ascii_digit() {
                self.pos += 1;
            } else {
                break;
            }
        }
        let raw = &self.src[start..self.pos];
        if !is_value_boundary(self.peek()) {
            return Err(self.err(
                line,
                format!("invalid integer literal starting with `{raw}`"),
            ));
        }
        let n = raw
            .parse::<i64>()
            .map_err(|_| self.err(line, format!("invalid integer literal `{raw}`")))?;
        Ok(TomlValue::Int(n))
    }
}

fn is_value_boundary(next: Option<u8>) -> bool {
    matches!(next, None | Some(b' ' | b'\t' | b'\r' | b'\n' | b'#'))
}

// ---------------------------------------------------------------------------
// Schema Validation
// ---------------------------------------------------------------------------

/// Parses and strictly validates a `meta.toml` document for module folder `folder`.
pub fn parse_and_validate_meta(raw: &str, folder: &str, path: &Path) -> Result<ModuleMeta, String> {
    let mut parser = TomlParser::new(raw, path);
    let mut table = parser.parse_table()?;

    let err_at = |line: usize, msg: String| format!("{}:{line}: {msg}", path.display());
    let err_file = |msg: String| format!("{}: {msg}", path.display());

    // Reject unknown keys first with exact line numbers.
    const ALLOWED_KEYS: &[&str] = &[
        "name", "status", "wave", "summary", "owners", "family", "wasm32", "feature", "tags",
        "source",
    ];
    for (key, (line, _)) in &table {
        if !ALLOWED_KEYS.contains(&key.as_str()) {
            return Err(err_at(*line, format!("unknown key `{key}`")));
        }
    }

    // Required: name
    let (name_line, name) = take_required_str(&mut table, "name", path)?;
    if !is_valid_module_name(&name) {
        return Err(err_at(
            name_line,
            format!("invalid module name `{name}`: must match `[a-z][a-z0-9_]*`"),
        ));
    }
    if RUST_KEYWORDS.contains(&name.as_str()) {
        return Err(err_at(
            name_line,
            format!("module name `{name}` is a reserved Rust keyword"),
        ));
    }
    if name != folder {
        return Err(err_at(
            name_line,
            format!("module name `{name}` must match directory name `{folder}`"),
        ));
    }

    // Required: status
    let (status_line, status_str) = take_required_str(&mut table, "status", path)?;
    let status = Status::parse(&status_str).ok_or_else(|| {
        err_at(
            status_line,
            format!("invalid status `{status_str}`: expected `native`, `fork`, or `adapter`"),
        )
    })?;

    // Required: wave
    let (wave_line, wave_i64) = take_required_int(&mut table, "wave", path)?;
    if !(1..=9).contains(&wave_i64) {
        return Err(err_at(
            wave_line,
            format!("invalid wave `{wave_i64}`: must be an integer in 1..=9"),
        ));
    }
    let wave = wave_i64 as u8;

    // Required: summary
    let (summary_line, summary) = take_required_str(&mut table, "summary", path)?;
    if summary.trim().is_empty() {
        return Err(err_at(
            summary_line,
            "summary must be non-empty".to_string(),
        ));
    }
    if summary.contains('\n') || summary.contains('\r') {
        return Err(err_at(
            summary_line,
            "summary must be a single line".to_string(),
        ));
    }
    if summary.len() > MAX_SUMMARY_BYTES {
        return Err(err_at(
            summary_line,
            format!(
                "summary is {} bytes, exceeding maximum of {MAX_SUMMARY_BYTES} bytes",
                summary.len()
            ),
        ));
    }

    // Required: owners
    let (owners_line, owners) = take_required_str_array(&mut table, "owners", path)?;
    if owners.is_empty() {
        return Err(err_at(
            owners_line,
            "owners must contain at least one GitHub handle or team".to_string(),
        ));
    }
    for owner in &owners {
        if !is_valid_owner(owner) {
            return Err(err_at(
                owners_line,
                format!(
                    "invalid owner `{owner}`: expected GitHub handle `[A-Za-z0-9-]+` or team `org/team`"
                ),
            ));
        }
    }

    // Optional: family
    let family = match take_optional_str(&mut table, "family", path)? {
        Some((line, f)) if !f.is_empty() => {
            if !is_valid_kebab_or_ident(&f) {
                return Err(err_at(
                    line,
                    format!("invalid family `{f}`: must match `[a-z][a-z0-9_-]*`"),
                ));
            }
            Some(f)
        }
        _ => None,
    };

    // Optional: wasm32
    let wasm32 = match table.remove("wasm32") {
        Some((_, TomlValue::Bool(b))) => b,
        Some((line, _)) => return Err(err_at(line, "`wasm32` must be a boolean".to_string())),
        None => false,
    };

    // Optional: feature
    let feature = match take_optional_str(&mut table, "feature", path)? {
        Some((line, f)) if !f.is_empty() => {
            if !is_valid_kebab_case(&f) {
                return Err(err_at(
                    line,
                    format!("invalid feature `{f}`: must be kebab-case (`[a-z][a-z0-9-]*`)"),
                ));
            }
            Some(f)
        }
        _ => None,
    };

    // Optional: tags
    let tags = match table.remove("tags") {
        Some((line, TomlValue::StrArray(arr))) => {
            if arr.len() > MAX_TAGS {
                return Err(err_at(
                    line,
                    format!("too many tags ({}): maximum is {MAX_TAGS}", arr.len()),
                ));
            }
            let mut seen = BTreeSet::new();
            for tag in &arr {
                if !is_valid_kebab_case(tag) {
                    return Err(err_at(
                        line,
                        format!("invalid tag `{tag}`: must be kebab-case (`[a-z][a-z0-9-]*`)"),
                    ));
                }
                if !seen.insert(tag.as_str()) {
                    return Err(err_at(line, format!("duplicate tag `{tag}`")));
                }
            }
            arr
        }
        Some((line, _)) => {
            return Err(err_at(
                line,
                "`tags` must be an array of strings".to_string(),
            ));
        }
        None => Vec::new(),
    };

    // Optional/conditional: source
    let source_opt = take_optional_str(&mut table, "source", path)?;
    let source = match status {
        Status::Native => {
            if let Some((line, s)) = source_opt {
                if !s.is_empty() {
                    return Err(err_at(
                        line,
                        "`source` is forbidden when `status = \"native\"`".to_string(),
                    ));
                }
            }
            None
        }
        Status::Fork | Status::Adapter => match source_opt {
            Some((line, s)) => {
                if s.trim().is_empty() {
                    return Err(err_at(
                        line,
                        format!(
                            "`source` must be non-empty when `status = \"{}\"`",
                            status.as_str()
                        ),
                    ));
                }
                Some(s)
            }
            None => {
                return Err(err_file(format!(
                    "missing required key `source` for `status = \"{}\"`",
                    status.as_str()
                )));
            }
        },
    };

    Ok(ModuleMeta {
        name,
        family,
        status,
        wasm32,
        feature,
        wave,
        summary,
        owners,
        tags,
        source,
    })
}

fn take_required_str(
    table: &mut BTreeMap<String, (usize, TomlValue)>,
    key: &str,
    path: &Path,
) -> Result<(usize, String), String> {
    match table.remove(key) {
        Some((line, TomlValue::Str(s))) => Ok((line, s)),
        Some((line, _)) => Err(format!(
            "{}:{line}: `{key}` must be a string",
            path.display()
        )),
        None => Err(format!("{}: missing required key `{key}`", path.display())),
    }
}

fn take_optional_str(
    table: &mut BTreeMap<String, (usize, TomlValue)>,
    key: &str,
    path: &Path,
) -> Result<Option<(usize, String)>, String> {
    match table.remove(key) {
        Some((line, TomlValue::Str(s))) => Ok(Some((line, s))),
        Some((line, _)) => Err(format!(
            "{}:{line}: `{key}` must be a string",
            path.display()
        )),
        None => Ok(None),
    }
}

fn take_required_int(
    table: &mut BTreeMap<String, (usize, TomlValue)>,
    key: &str,
    path: &Path,
) -> Result<(usize, i64), String> {
    match table.remove(key) {
        Some((line, TomlValue::Int(n))) => Ok((line, n)),
        Some((line, _)) => Err(format!(
            "{}:{line}: `{key}` must be an integer",
            path.display()
        )),
        None => Err(format!("{}: missing required key `{key}`", path.display())),
    }
}

fn take_required_str_array(
    table: &mut BTreeMap<String, (usize, TomlValue)>,
    key: &str,
    path: &Path,
) -> Result<(usize, Vec<String>), String> {
    match table.remove(key) {
        Some((line, TomlValue::StrArray(arr))) => Ok((line, arr)),
        Some((line, _)) => Err(format!(
            "{}:{line}: `{key}` must be an array of strings",
            path.display()
        )),
        None => Err(format!("{}: missing required key `{key}`", path.display())),
    }
}

fn is_valid_module_name(s: &str) -> bool {
    let mut bytes = s.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    first.is_ascii_lowercase()
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

fn is_valid_kebab_case(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.is_empty() || !bytes[0].is_ascii_lowercase() || bytes[bytes.len() - 1] == b'-' {
        return false;
    }
    let mut prev_hyphen = false;
    for &b in bytes {
        if b == b'-' {
            if prev_hyphen {
                return false;
            }
            prev_hyphen = true;
        } else if b.is_ascii_lowercase() || b.is_ascii_digit() {
            prev_hyphen = false;
        } else {
            return false;
        }
    }
    true
}

fn is_valid_kebab_or_ident(s: &str) -> bool {
    let mut bytes = s.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    first.is_ascii_lowercase()
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

fn is_valid_owner(raw: &str) -> bool {
    let body = raw.strip_prefix('@').unwrap_or(raw);
    if body.is_empty() {
        return false;
    }
    let valid_handle = |seg: &str| {
        !seg.is_empty()
            && !seg.starts_with('-')
            && !seg.ends_with('-')
            && seg.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    };
    let valid_team = |seg: &str| {
        !seg.is_empty()
            && !seg.starts_with('-')
            && !seg.ends_with('-')
            && seg
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    };
    match body.split_once('/') {
        Some((org, team)) => !team.contains('/') && valid_handle(org) && valid_team(team),
        None => valid_handle(body),
    }
}

fn format_owner_handle(owner: &str) -> String {
    if owner.starts_with('@') {
        owner.to_string()
    } else {
        format!("@{owner}")
    }
}

// ---------------------------------------------------------------------------
// Deterministic output renderers
// ---------------------------------------------------------------------------

/// Renders `docs/catalog.json`, `docs/CATALOG.md`, and `.github/CODEOWNERS` deterministically.
pub fn build_outputs(modules: &[ModuleMeta]) -> GeneratedFiles {
    let mut sorted = modules.to_vec();
    sorted.sort_by(|a, b| a.wave.cmp(&b.wave).then_with(|| a.name.cmp(&b.name)));

    GeneratedFiles {
        catalog_json: render_catalog_json(&sorted),
        catalog_md: render_catalog_md(&sorted),
        codeowners: render_codeowners(&sorted),
    }
}

fn render_catalog_json(modules: &[ModuleMeta]) -> String {
    let mut out = String::new();
    out.push_str("{\n");
    out.push_str("  \"version\": 1,\n");
    if modules.is_empty() {
        out.push_str("  \"modules\": []\n");
    } else {
        out.push_str("  \"modules\": [\n");
        for (idx, m) in modules.iter().enumerate() {
            out.push_str("    {\n");
            push_json_kv_str(&mut out, "name", &m.name, true);
            push_json_kv_opt_str(&mut out, "family", m.family.as_deref(), true);
            push_json_kv_str(&mut out, "status", m.status.as_str(), true);
            let _ = writeln!(out, "      \"wasm32\": {},", m.wasm32);
            push_json_kv_opt_str(&mut out, "feature", m.feature.as_deref(), true);
            let _ = writeln!(out, "      \"wave\": {},", m.wave);
            push_json_kv_str(&mut out, "summary", &m.summary, true);
            push_json_kv_str_array(&mut out, "owners", &m.owners, true);
            push_json_kv_str_array(&mut out, "tags", &m.tags, true);
            push_json_kv_str_array(&mut out, "derived_tags", &m.derived_tags(), true);
            push_json_kv_opt_str(&mut out, "source", m.source.as_deref(), true);
            push_json_kv_str(&mut out, "path", &m.path(), false);
            if idx + 1 < modules.len() {
                out.push_str("    },\n");
            } else {
                out.push_str("    }\n");
            }
        }
        out.push_str("  ]\n");
    }
    out.push_str("}\n");
    out
}

fn push_json_kv_str(out: &mut String, key: &str, val: &str, comma: bool) {
    out.push_str("      \"");
    out.push_str(key);
    out.push_str("\": ");
    push_json_escaped(out, val);
    if comma {
        out.push(',');
    }
    out.push('\n');
}

fn push_json_kv_opt_str(out: &mut String, key: &str, val: Option<&str>, comma: bool) {
    out.push_str("      \"");
    out.push_str(key);
    out.push_str("\": ");
    match val {
        Some(s) => push_json_escaped(out, s),
        None => out.push_str("null"),
    }
    if comma {
        out.push(',');
    }
    out.push('\n');
}

fn push_json_kv_str_array(out: &mut String, key: &str, items: &[String], comma: bool) {
    out.push_str("      \"");
    out.push_str(key);
    out.push_str("\": [");
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        push_json_escaped(out, item);
    }
    out.push(']');
    if comma {
        out.push(',');
    }
    out.push('\n');
}

fn push_json_escaped(out: &mut String, s: &str) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{0008}' => out.push_str("\\b"),
            '\u{000C}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn render_catalog_md(modules: &[ModuleMeta]) -> String {
    let mut out = String::new();
    out.push_str("<!-- GENERATED by akku catalog gen — do not edit manually -->\n\n");
    out.push_str("# Akku Module Catalog\n");

    let mut by_wave: BTreeMap<u8, Vec<&ModuleMeta>> = BTreeMap::new();
    for m in modules {
        by_wave.entry(m.wave).or_default().push(m);
    }

    for (wave, mods) in by_wave {
        let _ = writeln!(out, "\n## Wave {wave}\n");
        out.push_str("| Module | Status | WASM32 | Feature | Owners | Summary |\n");
        out.push_str("| --- | --- | --- | --- | --- | --- |\n");
        for m in mods {
            let wasm = if m.wasm32 { "yes" } else { "no" };
            let feature = match &m.feature {
                Some(f) => format!("`{f}`"),
                None => "-".to_string(),
            };
            let owners = m
                .owners
                .iter()
                .map(|o| format!("`{}`", format_owner_handle(o)))
                .collect::<Vec<_>>()
                .join(", ");
            let summary = m.summary.replace('|', "\\|");
            let _ = writeln!(
                out,
                "| `akku::{}` | `{}` | {} | {} | {} | {} |",
                m.name,
                m.status.as_str(),
                wasm,
                feature,
                owners,
                summary
            );
        }
    }

    out
}

fn render_codeowners(modules: &[ModuleMeta]) -> String {
    let mut by_name: Vec<&ModuleMeta> = modules.iter().collect();
    by_name.sort_by(|a, b| a.name.cmp(&b.name));

    let mut out = String::new();
    out.push_str("# GENERATED by akku catalog gen — do not edit manually\n");
    for m in by_name {
        let owners = m
            .owners
            .iter()
            .map(|o| format_owner_handle(o))
            .collect::<Vec<_>>()
            .join(" ");
        let _ = writeln!(out, "crates/akku/src/{}/** {owners}", m.name);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TestRoot {
        path: PathBuf,
    }

    impl TestRoot {
        fn new(label: &str) -> Self {
            let id = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "akku_cli_catalog_test_{}_{}_{}",
                std::process::id(),
                label,
                id
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(path.join("crates/akku/src")).unwrap();
            Self { path }
        }

        fn write_lib_rs(&self, content: &str) {
            fs::write(self.path.join("crates/akku/src/lib.rs"), content).unwrap();
        }

        fn write_meta(&self, module: &str, content: &str) {
            let dir = self.path.join("crates/akku/src").join(module);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("meta.toml"), content).unwrap();
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn parses_full_and_minimal_meta_toml() {
        let full = r#"
# Full metadata example
name = "http"
family = "net"
status = "adapter"
wasm32 = false
feature = "http"
wave = 1
summary = "HTTP server boundary & zero-dep HTTP/1.1 client"
owners = [
  "alice",
  "akku-rs/core-team",
]
tags = ["http", "web-server", "client"]
source = "axum 0.8 (server boundary)"
"#;
        let meta = parse_and_validate_meta(full, "http", Path::new("http/meta.toml")).unwrap();
        assert_eq!(meta.name, "http");
        assert_eq!(meta.family.as_deref(), Some("net"));
        assert_eq!(meta.status, Status::Adapter);
        assert!(!meta.wasm32);
        assert_eq!(meta.feature.as_deref(), Some("http"));
        assert_eq!(meta.wave, 1);
        assert_eq!(meta.owners, vec!["alice", "akku-rs/core-team"]);
        assert_eq!(meta.tags, vec!["http", "web-server", "client"]);
        assert_eq!(meta.source.as_deref(), Some("axum 0.8 (server boundary)"));
        assert_eq!(
            meta.derived_tags(),
            vec!["adapter", "optional-feature", "family:net"]
        );

        let minimal = r#"
name = "time"
status = "native"
wave = 1
summary = "Civil dates, durations, RFC 3339, and mock clocks"
owners = ["bob"]
wasm32 = true
"#;
        let meta2 = parse_and_validate_meta(minimal, "time", Path::new("time/meta.toml")).unwrap();
        assert_eq!(meta2.family, None);
        assert_eq!(meta2.feature, None);
        assert_eq!(meta2.source, None);
        assert!(meta2.tags.is_empty());
        assert!(meta2.wasm32);
        assert_eq!(meta2.derived_tags(), vec!["native-0dep", "wasm32"]);
    }

    #[test]
    fn rejects_invalid_meta_toml_cases() {
        let p = Path::new("mod/meta.toml");

        // Unknown key reports line number.
        let err = parse_and_validate_meta(
            "name = \"cli\"\nstatus = \"native\"\nwave = 1\nsummary = \"ok\"\nowners = [\"a\"]\nbogus = 1\n",
            "cli",
            p,
        )
        .unwrap_err();
        assert!(
            err.contains("unknown key `bogus`") && err.contains(":6:"),
            "{err}"
        );

        // Duplicate key.
        let err = parse_and_validate_meta(
            "name = \"cli\"\nname = \"cli\"\nstatus = \"native\"\nwave = 1\nsummary = \"ok\"\nowners = [\"a\"]\n",
            "cli",
            p,
        )
        .unwrap_err();
        assert!(err.contains("duplicate key `name`"), "{err}");

        // Missing required key.
        let err = parse_and_validate_meta(
            "name = \"cli\"\nstatus = \"native\"\nwave = 1\nowners = [\"a\"]\n",
            "cli",
            p,
        )
        .unwrap_err();
        assert!(err.contains("missing required key `summary`"), "{err}");

        // Rust keyword as module name.
        let err = parse_and_validate_meta(
            "name = \"async\"\nstatus = \"native\"\nwave = 1\nsummary = \"ok\"\nowners = [\"a\"]\n",
            "async",
            p,
        )
        .unwrap_err();
        assert!(err.contains("reserved Rust keyword"), "{err}");

        // Folder mismatch.
        let err = parse_and_validate_meta(
            "name = \"cli\"\nstatus = \"native\"\nwave = 1\nsummary = \"ok\"\nowners = [\"a\"]\n",
            "term",
            p,
        )
        .unwrap_err();
        assert!(err.contains("must match directory name"), "{err}");

        // Fork/adapter missing source.
        let err = parse_and_validate_meta(
            "name = \"http\"\nstatus = \"fork\"\nwave = 1\nsummary = \"ok\"\nowners = [\"a\"]\n",
            "http",
            p,
        )
        .unwrap_err();
        assert!(err.contains("missing required key `source`"), "{err}");

        // Native with non-empty source.
        let err = parse_and_validate_meta(
            "name = \"cli\"\nstatus = \"native\"\nwave = 1\nsummary = \"ok\"\nowners = [\"a\"]\nsource = \"clap\"\n",
            "cli",
            p,
        )
        .unwrap_err();
        assert!(
            err.contains("forbidden when `status = \"native\"`"),
            "{err}"
        );

        // Invalid status.
        let err = parse_and_validate_meta(
            "name = \"cli\"\nstatus = \"wrapper\"\nwave = 1\nsummary = \"ok\"\nowners = [\"a\"]\n",
            "cli",
            p,
        )
        .unwrap_err();
        assert!(err.contains("invalid status"), "{err}");

        // Invalid owner.
        let err = parse_and_validate_meta(
            "name = \"cli\"\nstatus = \"native\"\nwave = 1\nsummary = \"ok\"\nowners = [\"bad owner\"]\n",
            "cli",
            p,
        )
        .unwrap_err();
        assert!(err.contains("invalid owner"), "{err}");

        // Duplicate tag.
        let err = parse_and_validate_meta(
            "name = \"cli\"\nstatus = \"native\"\nwave = 1\nsummary = \"ok\"\nowners = [\"a\"]\ntags = [\"x\", \"x\"]\n",
            "cli",
            p,
        )
        .unwrap_err();
        assert!(err.contains("duplicate tag `x`"), "{err}");

        // Summary > 140 bytes.
        let long = "a".repeat(141);
        let err = parse_and_validate_meta(
            &format!(
                "name = \"cli\"\nstatus = \"native\"\nwave = 1\nsummary = \"{long}\"\nowners = [\"a\"]\n"
            ),
            "cli",
            p,
        )
        .unwrap_err();
        assert!(err.contains("exceeding maximum of 140 bytes"), "{err}");
    }

    #[test]
    fn detects_lib_rs_and_meta_toml_drift() {
        let root = TestRoot::new("drift_missing_meta");
        root.write_lib_rs(
            r#"
// pub mod ignored_comment;
pub mod cli;
pub mod ghost;
pub mod prelude {
    pub mod nested_ignored;
}
"#,
        );
        root.write_meta(
            "cli",
            "name = \"cli\"\nstatus = \"native\"\nwave = 1\nsummary = \"CLI parser\"\nowners = [\"alice\"]\n",
        );

        let err = discover_and_validate(&root.path).unwrap_err();
        assert!(err.contains("ghost") && err.contains("missing"), "{err}");

        let root2 = TestRoot::new("drift_undeclared");
        root2.write_lib_rs("pub mod cli;\n");
        root2.write_meta(
            "cli",
            "name = \"cli\"\nstatus = \"native\"\nwave = 1\nsummary = \"CLI parser\"\nowners = [\"alice\"]\n",
        );
        root2.write_meta(
            "orphan",
            "name = \"orphan\"\nstatus = \"native\"\nwave = 2\nsummary = \"Orphan\"\nowners = [\"bob\"]\n",
        );

        let err2 = discover_and_validate(&root2.path).unwrap_err();
        assert!(
            err2.contains("orphan") && err2.contains("not declared"),
            "{err2}"
        );
    }

    #[test]
    fn gen_check_determinism_and_drift_detection_end_to_end() {
        let root = TestRoot::new("e2e");
        root.write_lib_rs(
            r#"
//! Crate docs
pub mod time;
#[cfg(feature = "http")]
pub mod http;
pub mod prelude {
    pub use crate::time::Duration;
}
"#,
        );
        root.write_meta(
            "time",
            r#"
name = "time"
status = "native"
wasm32 = true
wave = 1
summary = "Dates & \"durations\" | RFC 3339"
owners = ["alice", "@org/time-team"]
tags = ["time", "date"]
"#,
        );
        root.write_meta(
            "http",
            r#"
name = "http"
family = "net"
status = "adapter"
wasm32 = false
feature = "http"
wave = 2
summary = "HTTP server & client"
owners = ["bob"]
source = "axum 0.8"
"#,
        );

        // Before gen, check must fail reporting missing files.
        let before = check(&root.path).unwrap_err();
        assert!(before.contains("docs/catalog.json: missing"), "{before}");
        assert!(before.contains("docs/CATALOG.md: missing"), "{before}");
        assert!(before.contains(".github/CODEOWNERS: missing"), "{before}");

        // Run gen -> check passes.
        let count = generate(&root.path).unwrap();
        assert_eq!(count, 2);
        assert_eq!(check(&root.path).unwrap(), 2);

        let json1 = fs::read_to_string(root.path.join("docs/catalog.json")).unwrap();
        let md1 = fs::read_to_string(root.path.join("docs/CATALOG.md")).unwrap();
        let co1 = fs::read_to_string(root.path.join(".github/CODEOWNERS")).unwrap();

        // Spot-check JSON escaping, ordering (wave 1 time before wave 2 http), and CODEOWNERS.
        assert!(json1.contains("\"version\": 1"));
        assert!(json1.contains(r#"Dates & \"durations\" | RFC 3339"#));
        assert!(
            json1.find("\"name\": \"time\"").unwrap() < json1.find("\"name\": \"http\"").unwrap()
        );
        assert!(md1.contains("<!-- GENERATED by akku catalog gen — do not edit manually -->"));
        assert!(md1.contains("Dates & \"durations\" \\| RFC 3339"));
        assert!(co1.contains("# GENERATED by akku catalog gen — do not edit manually\n"));
        assert!(co1.contains("crates/akku/src/http/** @bob\n"));
        assert!(co1.contains("crates/akku/src/time/** @alice @org/time-team\n"));
        assert!(
            co1.find("crates/akku/src/http/**").unwrap()
                < co1.find("crates/akku/src/time/**").unwrap()
        );

        // Determinism: running gen a second time yields byte-identical output.
        generate(&root.path).unwrap();
        assert_eq!(
            fs::read_to_string(root.path.join("docs/catalog.json")).unwrap(),
            json1
        );
        assert_eq!(
            fs::read_to_string(root.path.join("docs/CATALOG.md")).unwrap(),
            md1
        );
        assert_eq!(
            fs::read_to_string(root.path.join(".github/CODEOWNERS")).unwrap(),
            co1
        );

        // Tampering with a generated file causes check to fail cleanly.
        fs::write(
            root.path.join("docs/CATALOG.md"),
            format!("{md1}manual edit\n"),
        )
        .unwrap();
        let err = check(&root.path).unwrap_err();
        assert!(err.contains("docs/CATALOG.md: out of date"), "{err}");
    }
}
