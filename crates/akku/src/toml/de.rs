//! The TOML parser.
//!
//! The parser is a hand-written recursive descent parser over the input bytes.
//! Nesting is bounded before every recursive step, so a deeply nested document
//! returns an error instead of overflowing the stack.

use std::collections::BTreeMap;

use super::datetime::Datetime;
use super::error::{position_at, Error, ErrorKind, Result};
use super::value::{Table, Value};

/// Limits applied while parsing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseOptions {
    /// The maximum nesting depth of arrays and inline tables.
    pub max_depth: usize,
    /// The maximum number of entries across all tables and arrays.
    pub max_table_entries: usize,
}

impl Default for ParseOptions {
    fn default() -> Self {
        Self {
            max_depth: 64,
            max_table_entries: 100_000,
        }
    }
}

/// Parse a TOML document with the default [`ParseOptions`].
pub fn parse(input: &str) -> Result<Table> {
    parse_with(input, ParseOptions::default())
}

/// Parse a TOML document with explicit [`ParseOptions`].
pub fn parse_with(input: &str, options: ParseOptions) -> Result<Table> {
    Parser::new(input, options).parse()
}

/// Parse a TOML document.
///
/// This is a named alternative to [`str::parse`] for callers that prefer a
/// free function.
pub fn from_str(input: &str) -> Result<Table> {
    parse(input)
}

/// How a table came into existence, used to enforce the TOML conflict rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Def {
    /// Defined by a `[name]` header.
    Header,
    /// Created implicitly by a deeper header such as `[a.b]`.
    ImplicitHeader,
    /// Created by a dotted key such as `a.b = 1`.
    Dotted,
    /// An array of tables created by `[[name]]`.
    ArrayOfTables,
    /// An inline table, which is closed for further extension.
    Inline,
}

/// One resolved step of a table path.
#[derive(Debug, Clone)]
struct Seg {
    key: String,
    index: Option<usize>,
}

/// The shape of an existing key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slot {
    Table,
    Array { len: usize, ends_with_table: bool },
    Other,
}

enum NumberError {
    Invalid(&'static str),
    Overflow,
}

struct Parser<'a> {
    src: &'a [u8],
    pos: usize,
    options: ParseOptions,
    root: Table,
    defs: BTreeMap<Vec<u8>, Def>,
    entries: usize,
    current: Vec<Seg>,
}

impl<'a> Parser<'a> {
    fn new(input: &'a str, options: ParseOptions) -> Self {
        Self {
            src: input.as_bytes(),
            pos: 0,
            options,
            root: Table::new(),
            defs: BTreeMap::new(),
            entries: 0,
            current: Vec::new(),
        }
    }

    fn parse(mut self) -> Result<Table> {
        loop {
            self.skip_trivia()?;
            if self.eof() {
                break;
            }
            if self.peek() == Some(b'[') {
                self.parse_table_header()?;
            } else {
                self.parse_key_value()?;
            }
            self.skip_line_tail()?;
        }
        Ok(self.root)
    }

    // ----------------------------------------------------------------- bytes

    fn eof(&self) -> bool {
        self.pos >= self.src.len()
    }

    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    fn byte_at(&self, index: usize) -> Option<u8> {
        self.src.get(index).copied()
    }

    fn err(&self, kind: ErrorKind) -> Error {
        self.err_at(kind, self.pos)
    }

    fn err_at(&self, kind: ErrorKind, at: usize) -> Error {
        Error::new(kind, position_at(self.src, at))
    }

    fn unexpected_eof(&self) -> Error {
        self.err(ErrorKind::UnexpectedEof)
    }

    fn expected(&self, expected: &'static str) -> Error {
        let found = match self.peek() {
            Some(byte) if byte.is_ascii_graphic() => format!("{:?}", char::from(byte)),
            Some(byte) => format!("byte {byte:#04x}"),
            None => "end of input".to_string(),
        };
        self.err(ErrorKind::Expected { expected, found })
    }

    fn text(&self, start: usize, end: usize) -> Result<&'a str> {
        match std::str::from_utf8(&self.src[start..end]) {
            Ok(text) => Ok(text),
            Err(_) => Err(self.err_at(ErrorKind::InvalidKey, start)),
        }
    }

    fn next_char(&mut self) -> Result<char> {
        let character = match std::str::from_utf8(&self.src[self.pos..]) {
            Ok(rest) => match rest.chars().next() {
                Some(character) => character,
                None => return Err(self.unexpected_eof()),
            },
            Err(_) => return Err(self.err(ErrorKind::InvalidKey)),
        };
        self.pos += character.len_utf8();
        Ok(character)
    }

    // -------------------------------------------------------------- skipping

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t')) {
            self.pos += 1;
        }
    }

    fn skip_comment(&mut self) {
        while let Some(byte) = self.peek() {
            if byte == b'\n' {
                break;
            }
            self.pos += 1;
        }
    }

    fn skip_trivia(&mut self) -> Result<()> {
        loop {
            match self.peek() {
                Some(b' ' | b'\t' | b'\n') => self.pos += 1,
                Some(b'\r') => {
                    if self.byte_at(self.pos + 1) == Some(b'\n') {
                        self.pos += 2;
                    } else {
                        self.pos += 1;
                    }
                }
                Some(b'#') => self.skip_comment(),
                _ => return Ok(()),
            }
        }
    }

    fn skip_line_tail(&mut self) -> Result<()> {
        self.skip_whitespace();
        if self.peek() == Some(b'#') {
            self.skip_comment();
        }
        match self.peek() {
            None => Ok(()),
            Some(b'\n') => {
                self.pos += 1;
                Ok(())
            }
            Some(b'\r') if self.byte_at(self.pos + 1) == Some(b'\n') => {
                self.pos += 2;
                Ok(())
            }
            Some(_) => Err(self.expected("newline")),
        }
    }

    // --------------------------------------------------------------- entries

    fn bump_entries(&mut self, at: usize) -> Result<()> {
        self.entries += 1;
        let limit = self.options.max_table_entries;
        if self.entries > limit {
            return Err(self.err_at(ErrorKind::MaxEntriesExceeded { limit }, at));
        }
        Ok(())
    }

    // --------------------------------------------------------------- headers

    fn parse_table_header(&mut self) -> Result<()> {
        let at = self.pos;
        self.pos += 1;
        let is_array = self.peek() == Some(b'[');
        if is_array {
            self.pos += 1;
        }
        self.skip_whitespace();
        let mut keys = Vec::new();
        loop {
            let key = self.parse_simple_key()?;
            keys.push(key);
            self.skip_whitespace();
            match self.peek() {
                Some(b'.') => {
                    self.pos += 1;
                    self.skip_whitespace();
                }
                Some(b']') => {
                    self.pos += 1;
                    if is_array {
                        if self.peek() != Some(b']') {
                            return Err(self.expected("']'"));
                        }
                        self.pos += 1;
                    }
                    break;
                }
                None => return Err(self.unexpected_eof()),
                Some(_) => return Err(self.expected("'.' or ']'")),
            }
        }
        self.define_table(&keys, is_array, at)
    }

    fn define_table(&mut self, keys: &[String], is_array: bool, at: usize) -> Result<()> {
        let mut root = std::mem::take(&mut self.root);
        let outcome = self.define_table_in(&mut root, keys, is_array, at);
        self.root = root;
        match outcome {
            Ok(segments) => {
                self.current = segments;
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    fn define_table_in(
        &mut self,
        root: &mut Table,
        keys: &[String],
        is_array: bool,
        at: usize,
    ) -> Result<Vec<Seg>> {
        let mut segments: Vec<Seg> = Vec::new();
        let mut path: Vec<u8> = Vec::new();
        let mut table: &mut Table = root;

        for (index, key) in keys.iter().enumerate() {
            let last = index + 1 == keys.len();
            let mut segment_key = path.clone();
            push_segment(&mut segment_key, key, None);
            let definition = self.defs.get(&segment_key).copied();
            let existing = slot(table, key);

            if last {
                if is_array {
                    match existing {
                        None => {
                            let values = vec![Value::Table(Table::new())];
                            table.insert(key.clone(), Value::Array(values));
                            self.bump_entries(at)?;
                            self.defs.insert(segment_key, Def::ArrayOfTables);
                            segments.push(Seg {
                                key: key.clone(),
                                index: Some(0),
                            });
                        }
                        Some(Slot::Array { len, .. }) if definition == Some(Def::ArrayOfTables) => {
                            let array = table
                                .get_mut(key)
                                .ok_or_else(|| self.expected("array of tables"))?;
                            let values = array
                                .as_array_mut()
                                .ok_or_else(|| self.expected("array of tables"))?;
                            values.push(Value::Table(Table::new()));
                            self.bump_entries(at)?;
                            segments.push(Seg {
                                key: key.clone(),
                                index: Some(len),
                            });
                        }
                        Some(_) => {
                            return Err(
                                self.err_at(ErrorKind::DuplicateTable { name: key.clone() }, at)
                            )
                        }
                    }
                } else {
                    match existing {
                        None => {
                            table.insert(key.clone(), Value::Table(Table::new()));
                            self.bump_entries(at)?;
                            self.defs.insert(segment_key, Def::Header);
                            segments.push(Seg {
                                key: key.clone(),
                                index: None,
                            });
                        }
                        Some(Slot::Table) => match definition {
                            Some(Def::ImplicitHeader) | None => {
                                self.defs.insert(segment_key, Def::Header);
                                segments.push(Seg {
                                    key: key.clone(),
                                    index: None,
                                });
                            }
                            Some(Def::Dotted) | Some(Def::Inline) => {
                                return Err(self
                                    .err_at(ErrorKind::TableRedefined { name: key.clone() }, at))
                            }
                            Some(_) => {
                                return Err(self
                                    .err_at(ErrorKind::DuplicateTable { name: key.clone() }, at))
                            }
                        },
                        Some(_) => {
                            return Err(
                                self.err_at(ErrorKind::DuplicateTable { name: key.clone() }, at)
                            )
                        }
                    }
                }
                break;
            }

            match existing {
                Some(Slot::Table) => {
                    if definition == Some(Def::Inline) {
                        return Err(
                            self.err_at(ErrorKind::TableRedefined { name: key.clone() }, at)
                        );
                    }
                    if definition.is_none() {
                        self.defs.insert(segment_key.clone(), Def::ImplicitHeader);
                    }
                    segments.push(Seg {
                        key: key.clone(),
                        index: None,
                    });
                    path = segment_key;
                    let next = table.get_mut(key).ok_or_else(|| self.expected("table"))?;
                    table = next.as_table_mut().ok_or_else(|| self.expected("table"))?;
                }
                Some(Slot::Array {
                    len,
                    ends_with_table,
                }) => {
                    if definition == Some(Def::Inline) || !ends_with_table || len == 0 {
                        return Err(
                            self.err_at(ErrorKind::TableRedefined { name: key.clone() }, at)
                        );
                    }
                    let last_index = len - 1;
                    segments.push(Seg {
                        key: key.clone(),
                        index: Some(last_index),
                    });
                    let mut array_key = path.clone();
                    push_segment(&mut array_key, key, Some(last_index));
                    path = array_key;
                    let next = table.get_mut(key).ok_or_else(|| self.expected("array"))?;
                    let values = next.as_array_mut().ok_or_else(|| self.expected("array"))?;
                    table = values[last_index]
                        .as_table_mut()
                        .ok_or_else(|| self.expected("table"))?;
                }
                Some(Slot::Other) => {
                    return Err(self.err_at(ErrorKind::TableRedefined { name: key.clone() }, at))
                }
                None => {
                    table.insert(key.clone(), Value::Table(Table::new()));
                    self.bump_entries(at)?;
                    self.defs.insert(segment_key.clone(), Def::ImplicitHeader);
                    segments.push(Seg {
                        key: key.clone(),
                        index: None,
                    });
                    path = segment_key;
                    let next = table.get_mut(key).ok_or_else(|| self.expected("table"))?;
                    table = next.as_table_mut().ok_or_else(|| self.expected("table"))?;
                }
            }
        }

        Ok(segments)
    }

    // ------------------------------------------------------------ key values

    fn parse_key_value(&mut self) -> Result<()> {
        let at = self.pos;
        let keys = self.parse_dotted_key()?;
        self.skip_whitespace();
        if self.peek() != Some(b'=') {
            return Err(self.expected("'='"));
        }
        self.pos += 1;
        self.skip_whitespace();
        let value = self.parse_value(0)?;

        let mut root = std::mem::take(&mut self.root);
        let outcome = self.insert_value(&mut root, &keys, value, at);
        self.root = root;
        outcome
    }

    fn insert_value(
        &mut self,
        root: &mut Table,
        keys: &[String],
        value: Value,
        at: usize,
    ) -> Result<()> {
        let current = self.current.clone();
        let mut path = path_key(&current);
        let mut table = resolve_table(root, &current).ok_or_else(|| {
            self.err_at(
                ErrorKind::TableRedefined {
                    name: current_name(&current),
                },
                at,
            )
        })?;

        for (index, key) in keys.iter().enumerate() {
            let last = index + 1 == keys.len();
            push_segment(&mut path, key, None);

            if last {
                if table.contains_key(key) {
                    return Err(self.err_at(ErrorKind::DuplicateKey { key: key.clone() }, at));
                }
                if matches!(value, Value::Table(_)) {
                    self.defs.insert(path.clone(), Def::Inline);
                }
                self.bump_entries(at)?;
                table.insert(key.clone(), value);
                return Ok(());
            }

            match slot(table, key) {
                None => {
                    table.insert(key.clone(), Value::Table(Table::new()));
                    self.bump_entries(at)?;
                    self.defs.insert(path.clone(), Def::Dotted);
                }
                Some(Slot::Table) => {
                    if self.defs.get(&path).is_some_and(|def| *def == Def::Inline) {
                        return Err(
                            self.err_at(ErrorKind::DottedKeyConflict { key: key.clone() }, at)
                        );
                    }
                }
                Some(_) => {
                    return Err(self.err_at(ErrorKind::DottedKeyConflict { key: key.clone() }, at))
                }
            }

            let next = table.get_mut(key).ok_or_else(|| {
                self.err_at(ErrorKind::DottedKeyConflict { key: key.clone() }, at)
            })?;
            table = next.as_table_mut().ok_or_else(|| {
                self.err_at(ErrorKind::DottedKeyConflict { key: key.clone() }, at)
            })?;
        }
        Ok(())
    }

    fn parse_dotted_key(&mut self) -> Result<Vec<String>> {
        let mut keys = Vec::new();
        loop {
            keys.push(self.parse_simple_key()?);
            self.skip_whitespace();
            if self.peek() == Some(b'.') {
                self.pos += 1;
                self.skip_whitespace();
            } else {
                break;
            }
        }
        Ok(keys)
    }

    fn parse_simple_key(&mut self) -> Result<String> {
        match self.peek() {
            Some(b'"') => self.parse_basic_string(),
            Some(b'\'') => self.parse_literal_string(),
            Some(byte) if is_bare_key_char(byte) => {
                let start = self.pos;
                while matches!(self.peek(), Some(byte) if is_bare_key_char(byte)) {
                    self.pos += 1;
                }
                let end = self.pos;
                Ok(self.text(start, end)?.to_string())
            }
            None => Err(self.unexpected_eof()),
            Some(_) => Err(self.err(ErrorKind::InvalidKey)),
        }
    }

    // ---------------------------------------------------------------- values

    fn parse_value(&mut self, depth: usize) -> Result<Value> {
        match self.peek() {
            None => Err(self.unexpected_eof()),
            Some(b'"') => self.parse_basic_string().map(Value::String),
            Some(b'\'') => self.parse_literal_string().map(Value::String),
            Some(b'[') => self.parse_array(depth),
            Some(b'{') => self.parse_inline_table(depth),
            Some(_) => {
                if self.starts_with(b"true") {
                    self.pos += 4;
                    return Ok(Value::Boolean(true));
                }
                if self.starts_with(b"false") {
                    self.pos += 5;
                    return Ok(Value::Boolean(false));
                }
                self.parse_number_or_datetime()
            }
        }
    }

    fn starts_with(&self, prefix: &[u8]) -> bool {
        self.src[self.pos..].starts_with(prefix)
    }

    /// Parse an array without recursing per nesting level.
    ///
    /// Nested arrays are tracked on an explicit stack, so a document with
    /// 100_000 opening brackets fails with [`ErrorKind::MaxDepthExceeded`]
    /// instead of overflowing the stack.
    fn parse_array(&mut self, depth: usize) -> Result<Value> {
        let mut stack: Vec<Vec<Value>> = Vec::new();
        let at = self.pos;
        self.push_array_frame(depth, at, &mut stack)?;
        loop {
            self.skip_trivia()?;
            match self.peek() {
                None => return Err(self.unexpected_eof()),
                Some(b']') => {
                    self.pos += 1;
                    let items = match stack.pop() {
                        Some(items) => items,
                        None => {
                            return Err(self.err(ErrorKind::UnexpectedByte { byte: b']' }));
                        }
                    };
                    match stack.last_mut() {
                        None => return Ok(Value::Array(items)),
                        Some(parent) => {
                            let at = self.pos;
                            self.bump_entries(at)?;
                            parent.push(Value::Array(items));
                        }
                    }
                    self.skip_trivia()?;
                    self.expect_array_separator()?;
                }
                Some(b'[') => {
                    let level = depth + stack.len();
                    let at = self.pos;
                    self.push_array_frame(level, at, &mut stack)?;
                }
                Some(_) => {
                    let value = self.parse_value(depth + stack.len())?;
                    let at = self.pos;
                    self.bump_entries(at)?;
                    if let Some(parent) = stack.last_mut() {
                        parent.push(value);
                    }
                    self.skip_trivia()?;
                    self.expect_array_separator()?;
                }
            }
        }
    }

    fn push_array_frame(
        &mut self,
        level: usize,
        at: usize,
        stack: &mut Vec<Vec<Value>>,
    ) -> Result<()> {
        if level >= self.options.max_depth {
            let limit = self.options.max_depth;
            return Err(self.err_at(ErrorKind::MaxDepthExceeded { limit }, at));
        }
        self.pos += 1;
        stack.push(Vec::new());
        Ok(())
    }

    fn expect_array_separator(&mut self) -> Result<()> {
        match self.peek() {
            Some(b',') => {
                self.pos += 1;
                Ok(())
            }
            Some(b']') => Ok(()),
            None => Err(self.unexpected_eof()),
            Some(_) => Err(self.expected("',' or ']'")),
        }
    }

    fn parse_inline_table(&mut self, depth: usize) -> Result<Value> {
        if depth >= self.options.max_depth {
            let limit = self.options.max_depth;
            return Err(self.err(ErrorKind::MaxDepthExceeded { limit }));
        }
        let at = self.pos;
        self.pos += 1;
        let mut table = Table::new();
        self.skip_whitespace();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(Value::Table(table));
        }
        loop {
            self.skip_whitespace();
            if matches!(self.peek(), Some(b'\n') | Some(b'\r')) {
                return Err(self.err(ErrorKind::NestedInlineTable));
            }
            let keys = self.parse_dotted_key()?;
            self.skip_whitespace();
            if self.peek() != Some(b'=') {
                return Err(self.expected("'='"));
            }
            self.pos += 1;
            self.skip_whitespace();
            let value = self.parse_value(depth + 1)?;
            insert_inline(&mut table, &keys, value).map_err(|kind| self.err(kind))?;
            self.bump_entries(at)?;
            self.skip_whitespace();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                    self.skip_whitespace();
                    if self.peek() == Some(b'}') {
                        return Err(self.err_at(ErrorKind::InlineTableNotClosed, at));
                    }
                }
                Some(b'}') => {
                    self.pos += 1;
                    break;
                }
                Some(b'\n' | b'\r') => return Err(self.err(ErrorKind::NestedInlineTable)),
                None => return Err(self.err_at(ErrorKind::InlineTableNotClosed, at)),
                Some(_) => return Err(self.expected("',' or '}'")),
            }
        }
        Ok(Value::Table(table))
    }

    // -------------------------------------------------------------- numbers

    fn parse_number_or_datetime(&mut self) -> Result<Value> {
        let start = self.pos;
        if looks_like_date(self.src, start) || looks_like_time(self.src, start) {
            let end = scan_datetime(self.src, start);
            self.pos = end;
            let text = self.text(start, end)?;
            let datetime = match Datetime::parse(text) {
                Ok(datetime) => datetime,
                Err(error) => {
                    let reason = match error.kind() {
                        ErrorKind::InvalidDatetime { reason } => *reason,
                        _ => "invalid date or time",
                    };
                    return Err(self.err_at(ErrorKind::InvalidDatetime { reason }, start));
                }
            };
            return Ok(Value::Datetime(datetime));
        }

        let end = scan_number(self.src, start);
        if end == start {
            return Err(self.err(ErrorKind::UnexpectedByte {
                byte: self.peek().unwrap_or_default(),
            }));
        }
        self.pos = end;
        let text = self.text(start, end)?;
        match number_from_str(text) {
            Ok(value) => Ok(value),
            Err(NumberError::Invalid(reason)) => {
                Err(self.err_at(ErrorKind::InvalidNumber { reason }, start))
            }
            Err(NumberError::Overflow) => Err(self.err_at(ErrorKind::IntegerOutOfRange, start)),
        }
    }

    // -------------------------------------------------------------- strings

    fn parse_basic_string(&mut self) -> Result<String> {
        let at = self.pos;
        if self.byte_at(self.pos + 1) == Some(b'"') && self.byte_at(self.pos + 2) == Some(b'"') {
            return self.parse_multiline_basic(at);
        }
        self.pos += 1;
        let mut output = String::new();
        loop {
            let byte = match self.peek() {
                Some(byte) => byte,
                None => return Err(self.err_at(ErrorKind::UnterminatedString, at)),
            };
            match byte {
                b'"' => {
                    self.pos += 1;
                    return Ok(output);
                }
                b'\\' => {
                    self.pos += 1;
                    self.parse_escape(&mut output)?;
                }
                b'\n' | b'\r' => return Err(self.err_at(ErrorKind::UnterminatedString, at)),
                byte if (byte < 0x20 && byte != b'\t') || byte == 0x7f => {
                    return Err(self.err(ErrorKind::ControlCharacterInString))
                }
                _ => {
                    let character = self.next_char()?;
                    output.push(character);
                }
            }
        }
    }

    fn parse_multiline_basic(&mut self, at: usize) -> Result<String> {
        self.pos += 3;
        self.skip_opening_newline();
        let mut output = String::new();
        loop {
            let byte = match self.peek() {
                Some(byte) => byte,
                None => return Err(self.err_at(ErrorKind::UnterminatedString, at)),
            };
            match byte {
                b'"' => {
                    let run = self.count_run(b'"');
                    if run < 3 {
                        for _ in 0..run {
                            output.push('"');
                        }
                        self.pos += run;
                        continue;
                    }
                    if run > 5 {
                        return Err(self.err(ErrorKind::UnexpectedByte { byte: b'"' }));
                    }
                    for _ in 0..run - 3 {
                        output.push('"');
                    }
                    self.pos += run;
                    return Ok(output);
                }
                b'\\' => {
                    self.pos += 1;
                    if self.is_line_ending_backslash() {
                        self.trim_line_ending();
                    } else {
                        self.parse_escape(&mut output)?;
                    }
                }
                b'\r' if self.byte_at(self.pos + 1) == Some(b'\n') => {
                    self.pos += 2;
                    output.push('\n');
                }
                b'\n' => {
                    self.pos += 1;
                    output.push('\n');
                }
                byte if (byte < 0x20 && byte != b'\t') || byte == 0x7f => {
                    return Err(self.err(ErrorKind::ControlCharacterInString))
                }
                _ => {
                    let character = self.next_char()?;
                    output.push(character);
                }
            }
        }
    }

    fn parse_literal_string(&mut self) -> Result<String> {
        let at = self.pos;
        if self.byte_at(self.pos + 1) == Some(b'\'') && self.byte_at(self.pos + 2) == Some(b'\'') {
            return self.parse_multiline_literal(at);
        }
        self.pos += 1;
        let mut output = String::new();
        loop {
            let byte = match self.peek() {
                Some(byte) => byte,
                None => return Err(self.err_at(ErrorKind::UnterminatedString, at)),
            };
            match byte {
                b'\'' => {
                    self.pos += 1;
                    return Ok(output);
                }
                b'\n' | b'\r' => return Err(self.err_at(ErrorKind::UnterminatedString, at)),
                byte if (byte < 0x20 && byte != b'\t') || byte == 0x7f => {
                    return Err(self.err(ErrorKind::ControlCharacterInString))
                }
                _ => {
                    let character = self.next_char()?;
                    output.push(character);
                }
            }
        }
    }

    fn parse_multiline_literal(&mut self, at: usize) -> Result<String> {
        self.pos += 3;
        self.skip_opening_newline();
        let mut output = String::new();
        loop {
            let byte = match self.peek() {
                Some(byte) => byte,
                None => return Err(self.err_at(ErrorKind::UnterminatedString, at)),
            };
            match byte {
                b'\'' => {
                    let run = self.count_run(b'\'');
                    if run < 3 {
                        for _ in 0..run {
                            output.push('\'');
                        }
                        self.pos += run;
                        continue;
                    }
                    if run > 5 {
                        return Err(self.err(ErrorKind::UnexpectedByte { byte: b'\'' }));
                    }
                    for _ in 0..run - 3 {
                        output.push('\'');
                    }
                    self.pos += run;
                    return Ok(output);
                }
                b'\r' if self.byte_at(self.pos + 1) == Some(b'\n') => {
                    self.pos += 2;
                    output.push('\n');
                }
                b'\n' => {
                    self.pos += 1;
                    output.push('\n');
                }
                byte if (byte < 0x20 && byte != b'\t') || byte == 0x7f => {
                    return Err(self.err(ErrorKind::ControlCharacterInString))
                }
                _ => {
                    let character = self.next_char()?;
                    output.push(character);
                }
            }
        }
    }

    fn skip_opening_newline(&mut self) {
        if self.peek() == Some(b'\r') && self.byte_at(self.pos + 1) == Some(b'\n') {
            self.pos += 2;
        } else if self.peek() == Some(b'\n') {
            self.pos += 1;
        }
    }

    fn count_run(&self, byte: u8) -> usize {
        let mut run = 0;
        while self.byte_at(self.pos + run) == Some(byte) {
            run += 1;
        }
        run
    }

    fn is_line_ending_backslash(&self) -> bool {
        // After `\`, only optional spaces and tabs followed by a line ending
        // (`\n` or `\r\n`) form a line-ending backslash. A backslash followed
        // by more content must start a real escape sequence instead.
        let mut index = self.pos;
        while matches!(self.byte_at(index), Some(b' ' | b'\t')) {
            index += 1;
        }
        match self.byte_at(index) {
            Some(b'\n') => true,
            Some(b'\r') => self.byte_at(index + 1) == Some(b'\n'),
            _ => false,
        }
    }

    fn trim_line_ending(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    fn parse_escape(&mut self, output: &mut String) -> Result<()> {
        let byte = match self.peek() {
            Some(byte) => byte,
            None => return Err(self.unexpected_eof()),
        };
        let simple = match byte {
            b'b' => Some('\u{8}'),
            b't' => Some('\t'),
            b'n' => Some('\n'),
            b'f' => Some('\u{c}'),
            b'r' => Some('\r'),
            b'"' => Some('"'),
            b'\\' => Some('\\'),
            _ => None,
        };
        if let Some(character) = simple {
            self.pos += 1;
            output.push(character);
            return Ok(());
        }
        match byte {
            b'u' => {
                self.pos += 1;
                let value = self.parse_hex(4)?;
                match char::from_u32(value) {
                    Some(character) => {
                        output.push(character);
                        Ok(())
                    }
                    None => Err(self.err(ErrorKind::InvalidUnicodeEscape)),
                }
            }
            b'U' => {
                self.pos += 1;
                let value = self.parse_hex(8)?;
                match char::from_u32(value) {
                    Some(character) => {
                        output.push(character);
                        Ok(())
                    }
                    None => Err(self.err(ErrorKind::InvalidUnicodeEscape)),
                }
            }
            _ => Err(self.err(ErrorKind::InvalidEscape {
                escape: char::from(byte),
            })),
        }
    }

    fn parse_hex(&mut self, count: usize) -> Result<u32> {
        let at = self.pos;
        let mut value = 0u32;
        for _ in 0..count {
            let byte = match self.peek() {
                Some(byte) => byte,
                None => return Err(self.unexpected_eof()),
            };
            let digit = match byte {
                b'0'..=b'9' => u32::from(byte - b'0'),
                b'a'..=b'f' => u32::from(byte - b'a') + 10,
                b'A'..=b'F' => u32::from(byte - b'A') + 10,
                _ => return Err(self.err_at(ErrorKind::InvalidUnicodeEscape, at)),
            };
            value = value * 16 + digit;
            self.pos += 1;
        }
        Ok(value)
    }
}

fn is_bare_key_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'
}

fn slot(table: &Table, key: &str) -> Option<Slot> {
    table.get(key).map(|value| match value {
        Value::Table(_) => Slot::Table,
        Value::Array(values) => Slot::Array {
            len: values.len(),
            ends_with_table: matches!(values.last(), Some(Value::Table(_))),
        },
        _ => Slot::Other,
    })
}

fn resolve_table<'t>(root: &'t mut Table, segments: &[Seg]) -> Option<&'t mut Table> {
    let mut table = root;
    for segment in segments {
        let value = table.get_mut(&segment.key)?;
        let value = match segment.index {
            Some(index) => value.as_array_mut()?.get_mut(index)?,
            None => value,
        };
        table = value.as_table_mut()?;
    }
    Some(table)
}

fn push_segment(path: &mut Vec<u8>, key: &str, index: Option<usize>) {
    // Length-prefix every segment so key bytes can never make two different
    // key sequences produce the same path. With plain separators the quoted
    // key `["a\u001Fb"]` collided with the dotted header path `[a.b]`.
    let key_bytes = key.as_bytes();
    path.extend_from_slice(&(key_bytes.len() as u64).to_be_bytes());
    path.extend_from_slice(key_bytes);
    if let Some(index) = index {
        path.extend_from_slice(&(index as u64).to_be_bytes());
    }
    path.push(0x1f);
}

fn path_key(segments: &[Seg]) -> Vec<u8> {
    let mut path = Vec::new();
    for segment in segments {
        push_segment(&mut path, &segment.key, segment.index);
    }
    path
}

fn current_name(segments: &[Seg]) -> String {
    segments
        .iter()
        .map(|segment| segment.key.as_str())
        .collect::<Vec<_>>()
        .join(".")
}

fn insert_inline(
    table: &mut Table,
    keys: &[String],
    value: Value,
) -> std::result::Result<(), ErrorKind> {
    let (last, parents) = keys.split_last().ok_or(ErrorKind::InvalidKey)?;
    let mut current = table;
    for key in parents {
        if !current.contains_key(key) {
            current.insert(key.clone(), Value::Table(Table::new()));
        }
        let next = current.get_mut(key).ok_or(ErrorKind::InvalidKey)?;
        current = next.as_table_mut().ok_or(ErrorKind::NestedInlineTable)?;
    }
    if current.contains_key(last) {
        return Err(ErrorKind::DuplicateKey { key: last.clone() });
    }
    current.insert(last.clone(), value);
    Ok(())
}

fn looks_like_date(source: &[u8], at: usize) -> bool {
    let end = at + 5;
    end <= source.len()
        && source[at..at + 4].iter().all(u8::is_ascii_digit)
        && source[at + 4] == b'-'
}

fn looks_like_time(source: &[u8], at: usize) -> bool {
    let end = at + 3;
    end <= source.len()
        && source[at].is_ascii_digit()
        && source[at + 1].is_ascii_digit()
        && source[at + 2] == b':'
}

fn scan_datetime(source: &[u8], start: usize) -> usize {
    let mut index = start;
    if looks_like_date(source, start) {
        index += 10;
        match source.get(index).copied() {
            Some(b'T') | Some(b't') => index += 1,
            Some(b' ') if source.get(index + 1).is_some_and(u8::is_ascii_digit) => index += 1,
            _ => return index,
        }
    }
    while matches!(source.get(index), Some(byte) if byte.is_ascii_digit() || *byte == b':') {
        index += 1;
    }
    if source.get(index) == Some(&b'.') {
        index += 1;
        while matches!(source.get(index), Some(byte) if byte.is_ascii_digit()) {
            index += 1;
        }
    }
    match source.get(index).copied() {
        Some(b'Z') | Some(b'z') => index += 1,
        Some(b'+') | Some(b'-') => {
            index += 1;
            while matches!(source.get(index), Some(byte) if byte.is_ascii_digit() || *byte == b':')
            {
                index += 1;
            }
        }
        _ => {}
    }
    index
}

fn scan_number(source: &[u8], start: usize) -> usize {
    let mut index = start;
    while let Some(byte) = source.get(index) {
        if byte.is_ascii_alphanumeric() || matches!(*byte, b'_' | b'+' | b'-' | b'.') {
            index += 1;
        } else {
            break;
        }
    }
    index
}

fn number_from_str(text: &str) -> std::result::Result<Value, NumberError> {
    let negative = text.starts_with('-');
    // At most one leading sign is allowed; strip only that one and reject any
    // `+-inf`, `--inf`, or `++nan` combination.
    let unsigned = text.strip_prefix(['+', '-']).unwrap_or(text);
    if unsigned.starts_with(['+', '-']) {
        return Err(NumberError::Invalid("multiple signs"));
    }
    match unsigned {
        "inf" => {
            return Ok(Value::Float(if negative {
                f64::NEG_INFINITY
            } else {
                f64::INFINITY
            }))
        }
        "nan" => return Ok(Value::Float(f64::NAN)),
        _ => {}
    }

    if unsigned.starts_with("0x") || unsigned.starts_with("0o") || unsigned.starts_with("0b") {
        if text.len() != unsigned.len() {
            return Err(NumberError::Invalid(
                "a prefix cannot be combined with a sign",
            ));
        }
        let (radix, digits) = match &unsigned[..2] {
            "0x" => (16u32, &unsigned[2..]),
            "0o" => (8, &unsigned[2..]),
            _ => (2, &unsigned[2..]),
        };
        if digits.is_empty() || !valid_underscores(digits) {
            return Err(NumberError::Invalid("invalid prefixed integer"));
        }
        let cleaned: String = digits
            .chars()
            .filter(|character| *character != '_')
            .collect();
        let invalid = !cleaned.chars().all(|character| character.is_digit(radix));
        if invalid {
            return Err(NumberError::Invalid("invalid digit for radix"));
        }
        return match i64::from_str_radix(&cleaned, radix) {
            Ok(value) => Ok(Value::Integer(value)),
            Err(_) => Err(NumberError::Overflow),
        };
    }

    if !valid_underscores(text) || !valid_decimal_underscores(text) {
        return Err(NumberError::Invalid("misplaced underscore"));
    }
    let cleaned: String = text.chars().filter(|character| *character != '_').collect();
    if cleaned.contains(['.', 'e', 'E']) {
        if !valid_float(&cleaned) {
            return Err(NumberError::Invalid("invalid float"));
        }
        return match cleaned.parse::<f64>() {
            Ok(value) => Ok(Value::Float(value)),
            Err(_) => Err(NumberError::Invalid("invalid float")),
        };
    }

    if !valid_integer(&cleaned) {
        return Err(NumberError::Invalid("invalid integer"));
    }
    match cleaned.parse::<i64>() {
        Ok(value) => Ok(Value::Integer(value)),
        Err(_) => Err(NumberError::Overflow),
    }
}

fn valid_underscores(text: &str) -> bool {
    if text.starts_with('_') || text.ends_with('_') || text.contains("__") {
        return false;
    }
    // An underscore must sit between two digits, or between a digit and the
    // prefix character of a radix.
    text.as_bytes().windows(3).all(|window| {
        window[1] != b'_' || (window[0].is_ascii_alphanumeric() && window[2].is_ascii_hexdigit())
    })
}

/// In decimal numbers an underscore must sit between two ASCII digits, so
/// underscores beside the exponent marker (`5_e3`, `5e_3`) are rejected.
fn valid_decimal_underscores(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.iter().enumerate().all(|(index, &byte)| {
        byte != b'_'
            || (index > 0
                && bytes[index - 1].is_ascii_digit()
                && index + 1 < bytes.len()
                && bytes[index + 1].is_ascii_digit())
    })
}

fn valid_integer(text: &str) -> bool {
    let body = text.trim_start_matches(['+', '-']);
    if body.is_empty() || !body.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    if body.len() > 1 && body.starts_with('0') {
        return false;
    }
    true
}

fn valid_float(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut index = 0;
    if matches!(bytes.first(), Some(b'+') | Some(b'-')) {
        index += 1;
    }
    let start = index;
    while bytes.get(index).is_some_and(u8::is_ascii_digit) {
        index += 1;
    }
    let integer_digits = index - start;
    if integer_digits == 0 {
        return false;
    }
    if integer_digits > 1 && bytes[start] == b'0' {
        return false;
    }
    if bytes.get(index) == Some(&b'.') {
        index += 1;
        let fraction_start = index;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            index += 1;
        }
        if index == fraction_start {
            return false;
        }
    }
    if matches!(bytes.get(index), Some(b'e') | Some(b'E')) {
        index += 1;
        if matches!(bytes.get(index), Some(b'+') | Some(b'-')) {
            index += 1;
        }
        let exponent_start = index;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            index += 1;
        }
        if index == exponent_start {
            return false;
        }
    }
    index == bytes.len()
}
