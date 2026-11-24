//! An iterative, streaming JSON parser.
//!
//! [`Decoder`] turns text into [`Token`]s without recursion, so the nesting
//! depth is bounded by [`ParseOptions::max_depth`] rather than by the stack.
//! [`parse`] builds a [`Value`] from the same token stream.

use std::borrow::Cow;
use std::io::Read;
use std::str::FromStr;

use crate::json::error::{Error, ErrorKind, Position, Result};
use crate::json::value::{Number, Object, Value};

/// How to treat a lone UTF-16 surrogate in a `\u` escape.
///
/// RFC 8259 allows the escape sequence but a lone surrogate is not a Unicode
/// scalar value, so it cannot be represented in a Rust `String`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LoneSurrogate {
    /// Reject the input with [`ErrorKind::LoneSurrogate`].
    #[default]
    Reject,
    /// Replace the surrogate with `U+FFFD REPLACEMENT CHARACTER`.
    Replace,
}

/// Limits and relaxations for parsing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseOptions {
    /// Maximum nesting depth of arrays and objects.
    ///
    /// The default is `128`. Deeper input fails with
    /// [`ErrorKind::MaxDepthExceeded`] instead of growing the stack.
    pub max_depth: usize,
    /// Fail when the same key appears twice in one object.
    ///
    /// The default is `true`. When disabled, the last value wins and the key
    /// keeps its first position.
    pub reject_duplicate_keys: bool,
    /// Accept `[1, 2,]` and `{"a": 1,}`.
    ///
    /// The default is `false`.
    pub allow_trailing_commas: bool,
    /// What to do with a lone surrogate. Default: reject.
    pub lone_surrogate: LoneSurrogate,
    /// Maximum length of a decoded string in bytes.
    ///
    /// The default is `None` (no limit).
    pub max_string_len: Option<usize>,
}

impl Default for ParseOptions {
    fn default() -> Self {
        Self::new()
    }
}

impl ParseOptions {
    /// Strict RFC 8259 options: depth `128`, duplicate keys rejected, no
    /// trailing commas, lone surrogates rejected, no string length limit.
    pub const fn new() -> Self {
        Self {
            max_depth: 128,
            reject_duplicate_keys: true,
            allow_trailing_commas: false,
            lone_surrogate: LoneSurrogate::Reject,
            max_string_len: None,
        }
    }

    /// Tolerant options: trailing commas and duplicate keys are allowed, lone
    /// surrogates are replaced, and the depth limit is raised to `512`.
    pub const fn lenient() -> Self {
        Self {
            max_depth: 512,
            reject_duplicate_keys: false,
            allow_trailing_commas: true,
            lone_surrogate: LoneSurrogate::Replace,
            max_string_len: None,
        }
    }
}

/// One token of a JSON document.
///
/// Strings borrow from the input whenever they contain no escape sequence, so
/// the common case does not allocate.
#[derive(Debug, Clone, PartialEq)]
pub enum Token<'a> {
    /// `null`.
    Null,
    /// `true` or `false`.
    Bool(bool),
    /// A number.
    Number(Number),
    /// A string. Borrowed when it had no escapes.
    String(Cow<'a, str>),
    /// `[`.
    ArrayStart,
    /// `]`.
    ArrayEnd,
    /// `{`.
    ObjectStart,
    /// A key inside an object, including the opening and closing quote.
    ObjectKey(Cow<'a, str>),
    /// `}`.
    ObjectEnd,
}

/// Where the parser is inside the grammar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// A value is expected.
    Value,
    /// An object key or `}` is expected.
    Key,
    /// `:` is expected.
    Colon,
    /// `,` or the end of the container is expected.
    CommaOrEnd,
    /// The top-level value is complete.
    End,
}

/// One open container.
#[derive(Debug)]
enum Frame {
    Array {
        index: usize,
        after_comma: bool,
    },
    Object {
        key: Option<String>,
        seen: Vec<String>,
        after_comma: bool,
    },
}

/// A streaming JSON decoder over a borrowed string.
///
/// ```
/// use akku::json::{Decoder, Token};
///
/// let mut decoder = Decoder::new(r#"{"a": [1, true]}"#);
/// assert_eq!(decoder.next().unwrap(), Some(Token::ObjectStart));
/// assert_eq!(
///     decoder.next().unwrap(),
///     Some(Token::ObjectKey("a".into()))
/// );
/// assert_eq!(decoder.next().unwrap(), Some(Token::ArrayStart));
/// assert!(decoder.next().unwrap().is_some());
/// ```
pub struct Decoder<'a> {
    source: &'a str,
    bytes: &'a [u8],
    index: usize,
    line: u32,
    column: u32,
    options: ParseOptions,
    stack: Vec<Frame>,
    state: State,
}

impl<'a> Decoder<'a> {
    /// Create a decoder with [`ParseOptions::new`].
    pub fn new(source: &'a str) -> Self {
        Self::with_options(source, ParseOptions::new())
    }

    /// Create a decoder with explicit options.
    pub fn with_options(source: &'a str, options: ParseOptions) -> Self {
        Self {
            source,
            bytes: source.as_bytes(),
            index: 0,
            line: 1,
            column: 1,
            options,
            stack: Vec::new(),
            state: State::Value,
        }
    }

    /// The options this decoder runs with.
    pub fn options(&self) -> &ParseOptions {
        &self.options
    }

    /// The number of currently open containers.
    pub fn depth(&self) -> usize {
        self.stack.len()
    }

    /// The current position, pointing at the next unread byte.
    pub fn position(&self) -> Position {
        Position {
            offset: self.index,
            line: self.line,
            column: self.column,
        }
    }

    /// Produce the next token, or `None` once the whole document is consumed.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Result<Option<Token<'a>>> {
        loop {
            match self.state {
                State::End => {
                    self.skip_whitespace();
                    if self.index < self.bytes.len() {
                        return Err(self.err(ErrorKind::TrailingCharacters));
                    }
                    return Ok(None);
                }
                State::Value => {
                    self.skip_whitespace();
                    let Some(byte) = self.peek() else {
                        return Err(self.err(ErrorKind::UnexpectedEof));
                    };
                    if byte == b']' {
                        if !self.close_array()? {
                            return Err(self.err(ErrorKind::Expected {
                                expected: "a value",
                                found: "']'",
                            }));
                        }
                        return Ok(Some(Token::ArrayEnd));
                    }
                    match byte {
                        b'[' => {
                            self.open_container(Frame::Array {
                                index: 0,
                                after_comma: false,
                            })?;
                            self.state = State::Value;
                            return Ok(Some(Token::ArrayStart));
                        }
                        b'{' => {
                            self.open_container(Frame::Object {
                                key: None,
                                seen: Vec::new(),
                                after_comma: false,
                            })?;
                            self.state = State::Key;
                            return Ok(Some(Token::ObjectStart));
                        }
                        b'"' => {
                            let text = self.parse_string()?;
                            self.complete_value();
                            return Ok(Some(Token::String(text)));
                        }
                        b'n' => {
                            self.expect_literal("null")?;
                            self.complete_value();
                            return Ok(Some(Token::Null));
                        }
                        b't' => {
                            self.expect_literal("true")?;
                            self.complete_value();
                            return Ok(Some(Token::Bool(true)));
                        }
                        b'f' => {
                            self.expect_literal("false")?;
                            self.complete_value();
                            return Ok(Some(Token::Bool(false)));
                        }
                        b'-' | b'0'..=b'9' => {
                            let number = self.parse_number()?;
                            self.complete_value();
                            return Ok(Some(Token::Number(number)));
                        }
                        other => return Err(self.err(ErrorKind::UnexpectedByte { byte: other })),
                    }
                }
                State::Key => {
                    self.skip_whitespace();
                    let Some(byte) = self.peek() else {
                        return Err(self.err(ErrorKind::UnexpectedEof));
                    };
                    if byte == b'}' {
                        self.close_object()?;
                        return Ok(Some(Token::ObjectEnd));
                    }
                    if byte != b'"' {
                        return Err(self.err(ErrorKind::Expected {
                            expected: "a string key or '}'",
                            found: describe_byte(byte),
                        }));
                    }
                    let key_position = self.position();
                    let key = self.parse_string()?;
                    let duplicate = if self.options.reject_duplicate_keys {
                        match self.stack.last() {
                            Some(Frame::Object { seen, .. }) => {
                                seen.iter().any(|existing| existing == key.as_ref())
                            }
                            _ => false,
                        }
                    } else {
                        false
                    };
                    if duplicate {
                        return Err(Error::new(ErrorKind::DuplicateKey {
                            key: key.into_owned(),
                        })
                        .at(key_position)
                        .with_path(self.current_path()));
                    }
                    if let Some(Frame::Object {
                        key: slot, seen, ..
                    }) = self.stack.last_mut()
                    {
                        *slot = Some(key.as_ref().to_owned());
                        if self.options.reject_duplicate_keys {
                            seen.push(key.as_ref().to_owned());
                        }
                    }
                    self.state = State::Colon;
                    return Ok(Some(Token::ObjectKey(key)));
                }
                State::Colon => {
                    self.skip_whitespace();
                    let Some(byte) = self.peek() else {
                        return Err(self.err(ErrorKind::UnexpectedEof));
                    };
                    if byte != b':' {
                        return Err(self.err(ErrorKind::Expected {
                            expected: "':'",
                            found: describe_byte(byte),
                        }));
                    }
                    self.bump();
                    self.state = State::Value;
                }
                State::CommaOrEnd => {
                    self.skip_whitespace();
                    let position = self.position();
                    let Some(byte) = self.peek() else {
                        return Err(self.err(ErrorKind::UnexpectedEof));
                    };
                    match byte {
                        b',' => {
                            self.bump();
                            let mut next = State::Value;
                            if let Some(frame) = self.stack.last_mut() {
                                match frame {
                                    Frame::Array {
                                        index, after_comma, ..
                                    } => {
                                        *index += 1;
                                        *after_comma = true;
                                    }
                                    Frame::Object {
                                        after_comma, key, ..
                                    } => {
                                        *after_comma = true;
                                        *key = None;
                                        next = State::Key;
                                    }
                                }
                                self.state = next;
                            } else {
                                return Err(self.err_at(position, ErrorKind::TrailingCharacters));
                            }
                        }
                        b']' => {
                            if !self.close_array()? {
                                return Err(self.err_at(
                                    position,
                                    ErrorKind::Expected {
                                        expected: "','",
                                        found: "']'",
                                    },
                                ));
                            }
                            return Ok(Some(Token::ArrayEnd));
                        }
                        b'}' => {
                            if !self.close_object()? {
                                return Err(self.err_at(
                                    position,
                                    ErrorKind::Expected {
                                        expected: "','",
                                        found: "'}'",
                                    },
                                ));
                            }
                            return Ok(Some(Token::ObjectEnd));
                        }
                        other => {
                            return Err(self.err_at(
                                position,
                                ErrorKind::Expected {
                                    expected: "',' or a closing bracket",
                                    found: describe_byte(other),
                                },
                            ))
                        }
                    }
                }
            }
        }
    }

    /// Skip exactly one value, including all of its nested containers.
    ///
    /// ```
    /// use akku::json::{Decoder, Token};
    ///
    /// let mut decoder = Decoder::new(r#"[{"a": 1}, 2]"#);
    /// assert_eq!(decoder.next().unwrap(), Some(Token::ArrayStart));
    /// decoder.skip_value().unwrap();
    /// assert!(matches!(decoder.next().unwrap(), Some(Token::Number(_))));
    /// ```
    pub fn skip_value(&mut self) -> Result<()> {
        if self.state == State::Colon {
            self.skip_whitespace();
            let Some(byte) = self.peek() else {
                return Err(self.err(ErrorKind::UnexpectedEof));
            };
            if byte != b':' {
                return Err(self.err(ErrorKind::Expected {
                    expected: "':'",
                    found: describe_byte(byte),
                }));
            }
            self.bump();
            self.state = State::Value;
        }
        if self.state != State::Value {
            return Err(self.err(ErrorKind::Expected {
                expected: "a value",
                found: "the current position",
            }));
        }
        let mut depth = 0usize;
        loop {
            match self.next()? {
                Some(Token::ArrayStart | Token::ObjectStart) => depth += 1,
                Some(token @ (Token::ArrayEnd | Token::ObjectEnd)) => {
                    if depth == 0 {
                        let found = match token {
                            Token::ArrayEnd => "']'",
                            Token::ObjectEnd => "'}'",
                            _ => unreachable!(),
                        };
                        return Err(self.err(ErrorKind::Expected {
                            expected: "a value",
                            found,
                        }));
                    }
                    depth -= 1;
                    if depth == 0 {
                        return Ok(());
                    }
                }
                Some(_) => {
                    if depth == 0 {
                        return Ok(());
                    }
                }
                None => return Err(self.err(ErrorKind::UnexpectedEof)),
            }
        }
    }

    /// Open a container, enforcing the depth limit.
    fn open_container(&mut self, frame: Frame) -> Result<()> {
        if self.stack.len() >= self.options.max_depth {
            return Err(self.err(ErrorKind::MaxDepthExceeded {
                limit: self.options.max_depth,
            }));
        }
        self.bump();
        self.stack.push(frame);
        Ok(())
    }

    /// Close an array when the current container is one.
    fn close_array(&mut self) -> Result<bool> {
        if !matches!(self.stack.last(), Some(Frame::Array { .. })) {
            return Ok(false);
        }
        if let Some(Frame::Array { after_comma, .. }) = self.stack.last() {
            if *after_comma && !self.options.allow_trailing_commas {
                return Err(self.err(ErrorKind::TrailingComma));
            }
        }
        self.bump();
        self.stack.pop();
        self.complete_value();
        Ok(true)
    }

    /// Close an object when the current container is one.
    fn close_object(&mut self) -> Result<bool> {
        if !matches!(self.stack.last(), Some(Frame::Object { .. })) {
            return Ok(false);
        }
        if let Some(Frame::Object { after_comma, .. }) = self.stack.last() {
            if *after_comma && !self.options.allow_trailing_commas {
                return Err(self.err(ErrorKind::TrailingComma));
            }
        }
        self.bump();
        self.stack.pop();
        self.complete_value();
        Ok(true)
    }

    /// Record that a value just finished at the current position.
    ///
    /// The frame keeps pointing at the value that just finished, so an error
    /// in the following separator is still attributed to it. The index is
    /// advanced and the key dropped when the next element starts.
    fn complete_value(&mut self) {
        let state = match self.stack.last_mut() {
            Some(Frame::Array { after_comma, .. }) => {
                *after_comma = false;
                State::CommaOrEnd
            }
            Some(Frame::Object { after_comma, .. }) => {
                *after_comma = false;
                State::CommaOrEnd
            }
            None => State::End,
        };
        self.state = state;
    }

    /// The dotted path to the value currently being parsed.
    fn current_path(&self) -> Option<String> {
        let mut path = String::new();
        for frame in &self.stack {
            match frame {
                Frame::Array { index, .. } => {
                    path.push('[');
                    path.push_str(&index.to_string());
                    path.push(']');
                }
                Frame::Object { key, .. } => {
                    if let Some(key) = key {
                        if !path.is_empty() {
                            path.push('.');
                        }
                        path.push_str(key);
                    }
                }
            }
        }
        if path.is_empty() {
            None
        } else {
            Some(path)
        }
    }

    /// Build an error at the current position, with the current path.
    fn err(&self, kind: ErrorKind) -> Error {
        self.err_at(self.position(), kind)
    }

    /// Build an error at an explicit position, with the current path.
    fn err_at(&self, position: Position, kind: ErrorKind) -> Error {
        Error::new(kind).at(position).with_path(self.current_path())
    }

    /// The next byte without consuming it.
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.index).copied()
    }

    /// The byte `offset` bytes ahead without consuming it.
    fn peek_at(&self, offset: usize) -> Option<u8> {
        self.bytes.get(self.index + offset).copied()
    }

    /// Consume one byte and keep line and column up to date.
    fn bump(&mut self) -> Option<u8> {
        let byte = self.peek()?;
        self.index += 1;
        match byte {
            b'\n' => {
                self.line += 1;
                self.column = 1;
            }
            // Continuation bytes of a multi-byte character do not advance the
            // column, so the column counts characters rather than bytes.
            byte if byte & 0xC0 == 0x80 => {}
            _ => self.column += 1,
        }
        Some(byte)
    }

    /// Consume one byte or fail.
    fn bump_or_eof(&mut self) -> Result<u8> {
        match self.bump() {
            Some(byte) => Ok(byte),
            None => Err(self.err(ErrorKind::UnexpectedEof)),
        }
    }

    /// Skip JSON whitespace: space, tab, carriage return and newline.
    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.bump();
        }
    }

    /// Reset the cursor, used to undo a speculative read.
    fn rewind(&mut self, index: usize, line: u32, column: u32) {
        self.index = index;
        self.line = line;
        self.column = column;
    }

    /// Match a literal such as `null` byte by byte.
    fn expect_literal(&mut self, literal: &'static str) -> Result<()> {
        for wanted in literal.bytes() {
            match self.peek() {
                None => return Err(self.err(ErrorKind::UnexpectedEof)),
                Some(found) if found == wanted => {
                    self.bump();
                }
                Some(found) => {
                    return Err(self.err(ErrorKind::Expected {
                        expected: literal,
                        found: describe_byte(found),
                    }))
                }
            }
        }
        Ok(())
    }

    /// Enforce the optional string length limit.
    fn check_string_len(&self, len: usize) -> Result<()> {
        match self.options.max_string_len {
            Some(limit) if len > limit => Err(self.err(ErrorKind::StringTooLong { limit })),
            _ => Ok(()),
        }
    }

    /// Parse a string, starting at the opening quote.
    fn parse_string(&mut self) -> Result<Cow<'a, str>> {
        let source: &'a str = self.source;
        self.bump();
        let start = self.index;
        let mut owned: Option<String> = None;
        let mut run_start = start;
        loop {
            let Some(byte) = self.peek() else {
                return Err(self.err(ErrorKind::UnexpectedEof));
            };
            match byte {
                b'"' => {
                    let tail = &source[run_start..self.index];
                    self.bump();
                    return match owned {
                        Some(mut text) => {
                            text.push_str(tail);
                            self.check_string_len(text.len())?;
                            Ok(Cow::Owned(text))
                        }
                        None => {
                            self.check_string_len(tail.len())?;
                            Ok(Cow::Borrowed(tail))
                        }
                    };
                }
                b'\\' => {
                    let text = owned.get_or_insert_with(String::new);
                    text.push_str(&source[run_start..self.index]);
                    self.bump();
                    let escape = self.bump_or_eof()?;
                    match escape {
                        b'"' => text.push('"'),
                        b'\\' => text.push('\\'),
                        b'/' => text.push('/'),
                        b'b' => text.push('\u{0008}'),
                        b'f' => text.push('\u{000c}'),
                        b'n' => text.push('\n'),
                        b'r' => text.push('\r'),
                        b't' => text.push('\t'),
                        b'u' => self.parse_unicode_escape(text)?,
                        other => {
                            return Err(self.err(ErrorKind::InvalidEscape {
                                escape: char::from(other),
                            }))
                        }
                    }
                    run_start = self.index;
                    self.check_string_len(text.len())?;
                }
                byte if byte < 0x20 => return Err(self.err(ErrorKind::ControlCharacterInString)),
                _ => {
                    self.bump();
                    if owned.is_none() {
                        self.check_string_len(self.index - start)?;
                    }
                }
            }
        }
    }

    /// Read four hexadecimal digits.
    fn read_hex4(&mut self) -> Result<u16> {
        let mut value: u16 = 0;
        for _ in 0..4 {
            let Some(byte) = self.peek() else {
                return Err(self.err(ErrorKind::UnexpectedEof));
            };
            let digit = match byte {
                b'0'..=b'9' => byte - b'0',
                b'a'..=b'f' => byte - b'a' + 10,
                b'A'..=b'F' => byte - b'A' + 10,
                _ => return Err(self.err(ErrorKind::InvalidUnicodeEscape)),
            };
            self.bump();
            value = (value << 4) | u16::from(digit);
        }
        Ok(value)
    }

    /// Decode a `\u` escape, including a UTF-16 surrogate pair.
    fn parse_unicode_escape(&mut self, out: &mut String) -> Result<()> {
        let first = self.read_hex4()?;
        let character = if (0xD800..=0xDBFF).contains(&first) {
            let checkpoint = (self.index, self.line, self.column);
            if self.peek() == Some(b'\\') && self.peek_at(1) == Some(b'u') {
                self.bump();
                self.bump();
                match self.read_hex4() {
                    Ok(second) if (0xDC00..=0xDFFF).contains(&second) => {
                        let code = 0x1_0000
                            + ((u32::from(first) - 0xD800) << 10)
                            + (u32::from(second) - 0xDC00);
                        char::from_u32(code).unwrap_or('\u{FFFD}')
                    }
                    _ => {
                        let (index, line, column) = checkpoint;
                        self.rewind(index, line, column);
                        self.lone_surrogate()?
                    }
                }
            } else {
                self.lone_surrogate()?
            }
        } else if (0xDC00..=0xDFFF).contains(&first) {
            self.lone_surrogate()?
        } else {
            char::from_u32(u32::from(first)).unwrap_or('\u{FFFD}')
        };
        out.push(character);
        Ok(())
    }

    /// Apply the configured lone surrogate policy.
    fn lone_surrogate(&self) -> Result<char> {
        match self.options.lone_surrogate {
            LoneSurrogate::Reject => Err(self.err(ErrorKind::LoneSurrogate)),
            LoneSurrogate::Replace => Ok('\u{FFFD}'),
        }
    }

    /// Parse a number literal and classify it.
    fn parse_number(&mut self) -> Result<Number> {
        let start = self.index;
        let start_position = self.position();
        let negative = if self.peek() == Some(b'-') {
            self.bump();
            true
        } else {
            false
        };
        match self.peek() {
            Some(b'0') => {
                self.bump();
                if matches!(self.peek(), Some(b'0'..=b'9')) {
                    return Err(self.err(ErrorKind::InvalidNumber {
                        reason: "leading zero",
                    }));
                }
            }
            Some(b'1'..=b'9') => {
                self.bump();
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.bump();
                }
            }
            _ => {
                return Err(self.err(ErrorKind::InvalidNumber {
                    reason: "expected a digit",
                }))
            }
        }
        let mut is_float = false;
        if self.peek() == Some(b'.') {
            is_float = true;
            self.bump();
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(self.err(ErrorKind::InvalidNumber {
                    reason: "expected a digit after the decimal point",
                }));
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.bump();
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            is_float = true;
            self.bump();
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.bump();
            }
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(self.err(ErrorKind::InvalidNumber {
                    reason: "expected a digit in the exponent",
                }));
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.bump();
            }
        }
        let text = &self.source[start..self.index];
        if is_float {
            let value: f64 = text.parse().map_err(|_| {
                self.err_at(
                    start_position,
                    ErrorKind::InvalidNumber {
                        reason: "malformed float literal",
                    },
                )
            })?;
            if !value.is_finite() {
                return Err(self.err_at(start_position, ErrorKind::NumberOutOfRange));
            }
            return Ok(Number::F64(value));
        }
        if negative {
            if let Ok(value) = text.parse::<i64>() {
                return Ok(Number::I64(value));
            }
        } else if let Ok(value) = text.parse::<u64>() {
            return Ok(Number::U64(value));
        }
        match text.parse::<f64>() {
            Ok(value) if value.is_finite() => Ok(Number::F64(value)),
            _ => Err(self.err_at(start_position, ErrorKind::NumberOutOfRange)),
        }
    }
}

/// A short description of a byte for error messages.
fn describe_byte(byte: u8) -> &'static str {
    match byte {
        b'[' => "'['",
        b']' => "']'",
        b'{' => "'{'",
        b'}' => "'}'",
        b',' => "','",
        b':' => "':'",
        b'"' => "a string",
        b'-' | b'0'..=b'9' => "a number",
        b't' | b'f' => "a boolean",
        b'n' => "null",
        _ => "an unexpected byte",
    }
}

/// A container that is still being built.
enum Builder {
    Array(Vec<Value>),
    Object { object: Object, key: Option<String> },
}

/// Attach a finished value to its parent container, or to the root.
fn attach(stack: &mut [Builder], root: &mut Option<Value>, value: Value) -> Result<()> {
    match stack.last_mut() {
        None => {
            *root = Some(value);
            Ok(())
        }
        Some(Builder::Array(items)) => {
            items.push(value);
            Ok(())
        }
        Some(Builder::Object { object, key }) => {
            let key = key.take().ok_or_else(|| {
                Error::new(ErrorKind::Expected {
                    expected: "an object key",
                    found: "a value",
                })
            })?;
            object.insert(key, value);
            Ok(())
        }
    }
}

/// Parse a JSON document into a [`Value`] with [`ParseOptions::new`].
///
/// ```
/// use akku::json::parse;
///
/// let value = parse(r#"{"a": [1, 2],"#).unwrap_err();
/// assert_eq!(value.position().map(|p| p.line), Some(1));
/// ```
pub fn parse(source: &str) -> Result<Value> {
    parse_with(source, ParseOptions::new())
}

/// Parse a JSON document into a [`Value`] with explicit options.
///
/// ```
/// use akku::json::{parse_with, ParseOptions};
///
/// let value = parse_with("[1, 2,]", ParseOptions::lenient()).unwrap();
/// assert_eq!(value.as_array().map(Vec::len), Some(2));
/// ```
pub fn parse_with(source: &str, options: ParseOptions) -> Result<Value> {
    let mut decoder = Decoder::with_options(source, options);
    let mut stack: Vec<Builder> = Vec::new();
    let mut root: Option<Value> = None;
    while let Some(token) = decoder.next()? {
        match token {
            Token::Null => attach(&mut stack, &mut root, Value::Null)?,
            Token::Bool(value) => attach(&mut stack, &mut root, Value::Bool(value))?,
            Token::Number(number) => attach(&mut stack, &mut root, Value::Number(number))?,
            Token::String(text) => attach(&mut stack, &mut root, Value::String(text.into_owned()))?,
            Token::ArrayStart => stack.push(Builder::Array(Vec::new())),
            Token::ObjectStart => stack.push(Builder::Object {
                object: Object::new(),
                key: None,
            }),
            Token::ObjectKey(key) => match stack.last_mut() {
                Some(Builder::Object { key: slot, .. }) => *slot = Some(key.into_owned()),
                _ => {
                    return Err(Error::new(ErrorKind::Expected {
                        expected: "an object key position",
                        found: "a key",
                    })
                    .at(decoder.position()))
                }
            },
            Token::ArrayEnd => match stack.pop() {
                Some(Builder::Array(items)) => attach(&mut stack, &mut root, Value::Array(items))?,
                _ => {
                    return Err(Error::new(ErrorKind::Expected {
                        expected: "an array",
                        found: "']'",
                    })
                    .at(decoder.position()))
                }
            },
            Token::ObjectEnd => match stack.pop() {
                Some(Builder::Object { object, .. }) => {
                    attach(&mut stack, &mut root, Value::Object(object))?
                }
                _ => {
                    return Err(Error::new(ErrorKind::Expected {
                        expected: "an object",
                        found: "'}'",
                    })
                    .at(decoder.position()))
                }
            },
        }
    }
    root.ok_or_else(|| Error::new(ErrorKind::UnexpectedEof))
}

/// Parse a JSON document from a string.
///
/// This is the canonical name from the namespace document; it is identical to
/// [`parse`].
pub fn from_str(source: &str) -> Result<Value> {
    parse(source)
}

/// Parse a JSON document from UTF-8 bytes.
///
/// Bytes that are not valid UTF-8 fail with [`ErrorKind::InvalidUtf8`].
pub fn from_bytes(bytes: &[u8]) -> Result<Value> {
    from_bytes_with(bytes, ParseOptions::new())
}

/// Parse a JSON document from UTF-8 bytes with explicit options.
pub fn from_bytes_with(bytes: &[u8], options: ParseOptions) -> Result<Value> {
    let source = std::str::from_utf8(bytes).map_err(|_| Error::new(ErrorKind::InvalidUtf8))?;
    parse_with(source, options)
}

/// Read and parse a JSON document from anything implementing [`Read`].
///
/// Reading errors become [`ErrorKind::Io`]; content that is not valid UTF-8 is
/// reported as an I/O error by [`Read::read_to_string`].
pub fn from_reader<R: Read>(reader: R) -> Result<Value> {
    from_reader_with(reader, ParseOptions::new())
}

/// Read and parse a JSON document from anything implementing [`Read`], with
/// explicit options.
pub fn from_reader_with<R: Read>(mut reader: R, options: ParseOptions) -> Result<Value> {
    let mut buffer = String::new();
    reader.read_to_string(&mut buffer)?;
    parse_with(&buffer, options)
}

impl FromStr for Value {
    type Err = Error;

    fn from_str(source: &str) -> Result<Self> {
        parse(source)
    }
}
