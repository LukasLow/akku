//! The YAML writer: deterministic, round-trip safe and iterative.
//!
//! The writer never folds lines and never emits block scalars, so the same
//! value always produces the same bytes and re-parsing them yields an equal
//! value. Nested values are written with an explicit work stack, so a deeply
//! nested document cannot overflow the stack.

use std::fmt::Write as _;
use std::io::Write;

use super::error::{Error, ErrorKind, Result};
use super::schema::resolve_plain;
use super::value::Value;

/// How strings are quoted when they cannot be written as plain scalars.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum QuoteStyle {
    /// Plain when unambiguous, single quotes otherwise, double quotes when the
    /// string holds control characters.
    #[default]
    Auto,
    /// Always quote; double quotes are still used for control characters.
    Single,
    /// Always use double quotes.
    Double,
}

/// How a [`Value`] is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteOptions {
    /// Spaces per nesting level, clamped to `1..=9`. Default: `2`.
    pub indent: usize,
    /// Write the whole document in flow style. Default: `false`.
    pub flow: bool,
    /// Put `---` in front of the document. Default: `false`.
    pub explicit_start: bool,
    /// Put `...` after the document. Default: `false`.
    pub document_end: bool,
    /// Sort mapping keys alphabetically instead of keeping insertion order.
    pub sort_keys: bool,
    /// How strings are quoted. Default: [`QuoteStyle::Auto`].
    pub quote_style: QuoteStyle,
}

impl WriteOptions {
    /// The defaults: two spaces, block style, insertion order, automatic quotes.
    pub const fn new() -> Self {
        Self {
            indent: 2,
            flow: false,
            explicit_start: false,
            document_end: false,
            sort_keys: false,
            quote_style: QuoteStyle::Auto,
        }
    }
}

impl Default for WriteOptions {
    fn default() -> Self {
        Self::new()
    }
}

/// One unit of output work.
enum Piece<'a> {
    /// Text that is already formatted.
    Text(String),
    /// A node that still has to be formatted, at the given indent, and whether
    /// it sits inside a flow collection (or is a mapping key).
    Node(&'a Value, usize, bool),
}

/// Serialize a value with the default [`WriteOptions`].
pub fn to_string(value: &Value) -> String {
    to_string_with(value, WriteOptions::default())
}

/// Serialize a value with explicit [`WriteOptions`].
///
/// The output always ends with exactly one line break.
pub fn to_string_with(value: &Value, options: WriteOptions) -> String {
    let mut out = String::new();
    if options.explicit_start {
        out.push_str("---\n");
    }
    write_value(&mut out, value, options);
    if options.document_end {
        out.push_str("\n...");
    }
    out.push('\n');
    out
}

/// Write a value to any [`Write`].
pub fn write_to<W: Write>(value: &Value, writer: W) -> Result<()> {
    write_to_with(value, WriteOptions::default(), writer)
}

/// Write a value to any [`Write`] with explicit options.
pub fn write_to_with<W: Write>(value: &Value, options: WriteOptions, mut writer: W) -> Result<()> {
    writer
        .write_all(to_string_with(value, options).as_bytes())
        .map_err(|error| Error::new(ErrorKind::Io(error), Default::default()))
}

/// Write a multi-document stream, each document behind an explicit `---`.
pub fn write_all_to<W: Write>(documents: &[Value], mut writer: W) -> Result<()> {
    let mut out = String::new();
    for document in documents {
        out.push_str("---\n");
        write_value(&mut out, document, WriteOptions::default());
        out.push('\n');
    }
    writer
        .write_all(out.as_bytes())
        .map_err(|error| Error::new(ErrorKind::Io(error), Default::default()))
}

/// Append one value in block or flow style, without a trailing line break.
pub(crate) fn write_value(out: &mut String, value: &Value, options: WriteOptions) {
    let mut stack: Vec<Piece<'_>> = Vec::new();
    push_node(&mut stack, value, 0, options.flow, options);
    while let Some(piece) = stack.pop() {
        match piece {
            Piece::Text(text) => out.push_str(&text),
            Piece::Node(node, indent, flow) => push_node(&mut stack, node, indent, flow, options),
        }
    }
}

/// Schedule the pieces that render one node.
///
/// Mapping keys are scheduled as flow nodes on the same stack instead of being
/// rendered by a nested call, so nesting through keys cannot overflow the
/// stack either.
fn push_node<'a>(
    stack: &mut Vec<Piece<'a>>,
    value: &'a Value,
    indent: usize,
    flow: bool,
    options: WriteOptions,
) {
    match value {
        Value::Sequence(items) => {
            if items.is_empty() {
                stack.push(Piece::Text("[]".to_string()));
            } else if flow {
                stack.push(Piece::Text("]".to_string()));
                for index in (0..items.len()).rev() {
                    stack.push(Piece::Node(&items[index], 0, true));
                    if index > 0 {
                        stack.push(Piece::Text(", ".to_string()));
                    }
                }
                stack.push(Piece::Text("[".to_string()));
            } else {
                let child_indent = indent + 2;
                for index in (0..items.len()).rev() {
                    stack.push(Piece::Node(&items[index], child_indent, false));
                    stack.push(Piece::Text("- ".to_string()));
                    if index > 0 {
                        stack.push(Piece::Text(break_text(indent)));
                    }
                }
            }
        }
        Value::Mapping(mapping) => {
            let flow = flow || mapping.iter().any(|(key, _)| !is_scalar(key));
            if mapping.is_empty() {
                stack.push(Piece::Text("{}".to_string()));
                return;
            }
            // `sort_keys` reorders the schedule, not the mapping itself.
            let mut order: Vec<usize> = (0..mapping.entries.len()).collect();
            if options.sort_keys && order.len() > 1 {
                order.sort_by_cached_key(|&index| sort_text(&mapping.entries[index].0, options));
            }
            if flow {
                stack.push(Piece::Text("}".to_string()));
                for (position, &index) in order.iter().enumerate().rev() {
                    let (key, item) = &mapping.entries[index];
                    // Executes as: [", "] key ": " node.
                    stack.push(Piece::Node(item, 0, true));
                    stack.push(Piece::Text(": ".to_string()));
                    stack.push(Piece::Node(key, 0, true));
                    if position > 0 {
                        stack.push(Piece::Text(", ".to_string()));
                    }
                }
                stack.push(Piece::Text("{".to_string()));
            } else {
                let child_indent = indent + options.indent.clamp(1, 9);
                for (position, &index) in order.iter().enumerate().rev() {
                    let (key, item) = &mapping.entries[index];
                    // Executes as: break key ":" break node, or key ": " node.
                    stack.push(Piece::Node(item, child_indent, false));
                    if is_block_collection(item) {
                        stack.push(Piece::Text(break_text(child_indent)));
                        stack.push(Piece::Text(format!("{}:", key_text(key, options))));
                    } else {
                        stack.push(Piece::Text(format!("{}: ", key_text(key, options))));
                    }
                    if position > 0 {
                        stack.push(Piece::Text(break_text(indent)));
                    }
                }
            }
        }
        scalar => stack.push(Piece::Text(scalar_text(scalar, options, flow))),
    }
}

/// The textual form of a scalar.
fn scalar_text(value: &Value, options: WriteOptions, flow: bool) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(true) => "true".to_string(),
        Value::Bool(false) => "false".to_string(),
        Value::Int(number) => number.to_string(),
        Value::Float(number) => float_text(*number),
        Value::String(text) => string_text(text, options.quote_style, flow),
        _ => String::new(),
    }
}

/// The textual form of a key in a block mapping, whose keys are all scalars.
fn key_text(key: &Value, options: WriteOptions) -> String {
    scalar_text(key, options, true)
}

/// The text a key sorts by under [`WriteOptions::sort_keys`].
///
/// A sequence or mapping key sorts by its flow form with its own keys in
/// insertion order; rendering it with `sort_keys` off keeps this a single
/// iterative pass instead of a recursion through nested keys.
fn sort_text(key: &Value, options: WriteOptions) -> String {
    if is_scalar(key) {
        return key_text(key, options);
    }
    let mut text = String::new();
    write_value(
        &mut text,
        key,
        WriteOptions {
            flow: true,
            sort_keys: false,
            ..options
        },
    );
    text
}

/// Whether a value is a scalar.
fn is_scalar(value: &Value) -> bool {
    !matches!(value, Value::Sequence(_) | Value::Mapping(_))
}

/// Whether a value needs its own line when used as a mapping value.
fn is_block_collection(value: &Value) -> bool {
    match value {
        Value::Sequence(items) => !items.is_empty(),
        Value::Mapping(mapping) => !mapping.is_empty(),
        _ => false,
    }
}

/// A line break followed by `indent` spaces.
fn break_text(indent: usize) -> String {
    let mut text = String::with_capacity(indent + 1);
    text.push('\n');
    for _ in 0..indent {
        text.push(' ');
    }
    text
}

/// The shortest text that re-parses to the same float.
fn float_text(value: f64) -> String {
    if value.is_nan() {
        return ".nan".to_string();
    }
    if value.is_infinite() {
        return if value.is_sign_negative() {
            "-.inf".to_string()
        } else {
            ".inf".to_string()
        };
    }
    let text = value.to_string();
    // `1.0` prints as `1`, which would re-parse as an integer.
    if text.contains('.') || text.contains('e') || text.contains('E') {
        text
    } else {
        let mut with_fraction = text;
        with_fraction.push_str(".0");
        with_fraction
    }
}

/// Format a string so that it re-parses to the same string.
fn string_text(text: &str, style: QuoteStyle, flow: bool) -> String {
    match style {
        QuoteStyle::Double => double_quote(text),
        QuoteStyle::Single => {
            if text.chars().any(char::is_control) {
                double_quote(text)
            } else {
                single_quote(text)
            }
        }
        QuoteStyle::Auto => {
            if can_be_plain(text, flow) {
                text.to_string()
            } else if text.chars().any(char::is_control) {
                double_quote(text)
            } else {
                single_quote(text)
            }
        }
    }
}

/// Whether a string may stand as a plain scalar.
fn can_be_plain(text: &str, flow: bool) -> bool {
    if text.is_empty() {
        return false;
    }
    let first = text.chars().next().expect("non-empty");
    if first == ' ' || first == '\t' || first == '\u{feff}' {
        return false;
    }
    // Document markers would end or start a document at column one, and a
    // plain `<<` would be read back as a merge key.
    if text.starts_with("---") || text.starts_with("...") || text == "<<" {
        return false;
    }
    if text.ends_with(' ') || text.ends_with('\t') {
        return false;
    }
    if text.chars().any(char::is_control) {
        return false;
    }
    // A plain scalar that already resolves to another type would change type
    // on the way back in.
    if !matches!(resolve_plain(text), Value::String(_)) {
        return false;
    }
    if "-?:,[]{}#&*!|>'\"%@`".contains(first) {
        let allowed = matches!(first, '-' | '?' | ':')
            && text
                .chars()
                .nth(1)
                .is_some_and(|second| second != ' ' && second != '\t');
        if !allowed {
            return false;
        }
    }
    if text.contains(": ") || text.contains(":\t") || text.contains(" #") || text.contains("\t#") {
        return false;
    }
    if text.ends_with(':') {
        return false;
    }
    if flow
        && text
            .chars()
            .any(|ch| matches!(ch, ',' | '[' | ']' | '{' | '}'))
    {
        return false;
    }
    true
}

/// Single quotes, with `'` doubled.
fn single_quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('\'');
    for ch in text.chars() {
        if ch == '\'' {
            out.push('\'');
        }
        out.push(ch);
    }
    out.push('\'');
    out
}

/// Double quotes, with the escapes YAML understands.
fn double_quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\0' => out.push_str("\\0"),
            control if control.is_control() => {
                let _ = write!(out, "\\x{:02x}", control as u32);
            }
            other => out.push(other),
        }
    }
    out.push('"');
    out
}
