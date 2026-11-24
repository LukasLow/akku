//! Serializing JSON values to text.
//!
//! Serialization uses an explicit work stack, so a deeply nested value is
//! written without recursing.

use std::fmt;
use std::io::{self, Write};

use crate::json::error::{Error, Result};
use crate::json::value::{Number, Value};

/// How to format JSON output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SerializeOptions {
    /// Insert newlines and indentation.
    pub pretty: bool,
    /// Number of spaces per nesting level when `pretty` is set. Default: `2`.
    pub indent: u8,
    /// Escape `<`, `>`, `&` and `'` as `\uXXXX` so the output is safe to embed
    /// in HTML and inline scripts.
    pub escape_html: bool,
    /// Escape every non-ASCII character as `\uXXXX`, using a surrogate pair
    /// above `U+FFFF`.
    pub escape_non_ascii: bool,
}

impl Default for SerializeOptions {
    fn default() -> Self {
        Self::new()
    }
}

impl SerializeOptions {
    /// Compact output: no newlines, no HTML escaping, raw UTF-8. This is the
    /// default.
    pub const fn new() -> Self {
        Self {
            pretty: false,
            indent: 2,
            escape_html: false,
            escape_non_ascii: false,
        }
    }

    /// Indented output with two spaces per level.
    pub const fn pretty_printed() -> Self {
        Self {
            pretty: true,
            indent: 2,
            escape_html: false,
            escape_non_ascii: false,
        }
    }

    /// Set the number of spaces per nesting level.
    pub const fn with_indent(mut self, indent: u8) -> Self {
        self.indent = indent;
        self
    }

    /// Enable or disable HTML-safe escaping.
    pub const fn with_escape_html(mut self, escape_html: bool) -> Self {
        self.escape_html = escape_html;
        self
    }

    /// Enable or disable ASCII-only escaping.
    pub const fn with_escape_non_ascii(mut self, escape_non_ascii: bool) -> Self {
        self.escape_non_ascii = escape_non_ascii;
        self
    }
}

/// One unit of unfinished output.
enum Task<'a> {
    Value(&'a Value, usize),
    Key(&'a str),
    Comma,
    Indent(usize),
    CloseArray(usize),
    CloseObject(usize),
}

/// An escape that must be written instead of a character.
enum Escape {
    Simple(&'static str),
    Unicode(u32),
}

/// Serialize a value as compact JSON.
///
/// ```
/// use akku::json::{parse, to_string};
///
/// let value = parse(r#"{"a": [1, true, null]}"#).unwrap();
/// assert_eq!(to_string(&value), r#"{"a":[1,true,null]}"#);
/// ```
pub fn to_string(value: &Value) -> String {
    to_string_with(value, SerializeOptions::new())
}

/// Serialize a value as indented JSON with two spaces per level.
pub fn to_string_pretty(value: &Value) -> String {
    to_string_with(value, SerializeOptions::pretty_printed())
}

/// Serialize a value with explicit options.
pub fn to_string_with(value: &Value, options: SerializeOptions) -> String {
    let mut buffer = Vec::new();
    // Writing into a `Vec` cannot fail.
    let _ = write_to_with(value, &mut buffer, options);
    String::from_utf8(buffer).expect("JSON output is always valid UTF-8")
}

/// Write a value as compact JSON.
pub fn write_to<W: Write>(value: &Value, writer: W) -> Result<()> {
    write_to_with(value, writer, SerializeOptions::new())
}

/// Write a value as indented JSON with two spaces per level.
pub fn write_to_pretty<W: Write>(value: &Value, writer: W) -> Result<()> {
    write_to_with(value, writer, SerializeOptions::pretty_printed())
}

/// Write a value with explicit options.
pub fn write_to_with<W: Write>(
    value: &Value,
    mut writer: W,
    options: SerializeOptions,
) -> Result<()> {
    write_value(&mut writer, value, &options).map_err(Error::from)
}

impl fmt::Display for Value {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&to_string(self))
    }
}

/// The output loop: pop a task, write it, push follow-up tasks.
fn write_value<W: Write>(
    writer: &mut W,
    value: &Value,
    options: &SerializeOptions,
) -> io::Result<()> {
    let mut stack: Vec<Task<'_>> = vec![Task::Value(value, 0)];
    while let Some(task) = stack.pop() {
        match task {
            Task::Value(value, depth) => match value {
                Value::Null => writer.write_all(b"null")?,
                Value::Bool(true) => writer.write_all(b"true")?,
                Value::Bool(false) => writer.write_all(b"false")?,
                Value::Number(number) => write_number(writer, number)?,
                Value::String(text) => write_escaped(writer, text, options)?,
                Value::Array(items) => {
                    writer.write_all(b"[")?;
                    if items.is_empty() {
                        writer.write_all(b"]")?;
                        continue;
                    }
                    stack.push(Task::CloseArray(depth));
                    for (index, item) in items.iter().enumerate().rev() {
                        stack.push(Task::Value(item, depth + 1));
                        if options.pretty {
                            stack.push(Task::Indent(depth + 1));
                        }
                        if index > 0 {
                            stack.push(Task::Comma);
                        }
                    }
                }
                Value::Object(object) => {
                    writer.write_all(b"{")?;
                    if object.is_empty() {
                        writer.write_all(b"}")?;
                        continue;
                    }
                    stack.push(Task::CloseObject(depth));
                    for (index, (key, item)) in object.as_slice().iter().enumerate().rev() {
                        stack.push(Task::Value(item, depth + 1));
                        stack.push(Task::Key(key));
                        if options.pretty {
                            stack.push(Task::Indent(depth + 1));
                        }
                        if index > 0 {
                            stack.push(Task::Comma);
                        }
                    }
                }
            },
            Task::Key(key) => {
                write_escaped(writer, key, options)?;
                writer.write_all(if options.pretty { b": " } else { b":" })?;
            }
            Task::Comma => writer.write_all(b",")?,
            Task::Indent(depth) => {
                if options.pretty {
                    writer.write_all(b"\n")?;
                    write_spaces(writer, usize::from(options.indent) * depth)?;
                }
            }
            Task::CloseArray(depth) => {
                if options.pretty {
                    writer.write_all(b"\n")?;
                    write_spaces(writer, usize::from(options.indent) * depth)?;
                }
                writer.write_all(b"]")?;
            }
            Task::CloseObject(depth) => {
                if options.pretty {
                    writer.write_all(b"\n")?;
                    write_spaces(writer, usize::from(options.indent) * depth)?;
                }
                writer.write_all(b"}")?;
            }
        }
    }
    Ok(())
}

/// Write a number using the shortest round-tripping representation.
fn write_number<W: Write>(writer: &mut W, number: &Number) -> io::Result<()> {
    match number {
        Number::I64(value) => write!(writer, "{value}"),
        Number::U64(value) => write!(writer, "{value}"),
        Number::F64(value) => write!(writer, "{value:?}"),
    }
}

/// Indentation is written in chunks of up to 32 spaces.
const SPACES: &[u8] = b"                                ";

/// Write `count` spaces.
fn write_spaces<W: Write>(writer: &mut W, mut count: usize) -> io::Result<()> {
    while count > 0 {
        let chunk = count.min(SPACES.len());
        writer.write_all(&SPACES[..chunk])?;
        count -= chunk;
    }
    Ok(())
}

/// Decide how a single character is escaped, if at all.
fn escape_for(character: char, options: &SerializeOptions) -> Option<Escape> {
    match character {
        '"' => Some(Escape::Simple("\\\"")),
        '\\' => Some(Escape::Simple("\\\\")),
        '\u{0008}' => Some(Escape::Simple("\\b")),
        '\u{000c}' => Some(Escape::Simple("\\f")),
        '\n' => Some(Escape::Simple("\\n")),
        '\r' => Some(Escape::Simple("\\r")),
        '\t' => Some(Escape::Simple("\\t")),
        character if (character as u32) < 0x20 => Some(Escape::Unicode(character as u32)),
        '<' if options.escape_html => Some(Escape::Simple("\\u003c")),
        '>' if options.escape_html => Some(Escape::Simple("\\u003e")),
        '&' if options.escape_html => Some(Escape::Simple("\\u0026")),
        '\'' if options.escape_html => Some(Escape::Simple("\\u0027")),
        character if options.escape_non_ascii && !character.is_ascii() => {
            Some(Escape::Unicode(character as u32))
        }
        _ => None,
    }
}

/// Write a string with quotes and the required escapes.
fn write_escaped<W: Write>(
    writer: &mut W,
    text: &str,
    options: &SerializeOptions,
) -> io::Result<()> {
    writer.write_all(b"\"")?;
    let mut run_start = 0;
    for (offset, character) in text.char_indices() {
        let Some(escape) = escape_for(character, options) else {
            continue;
        };
        if run_start < offset {
            writer.write_all(&text.as_bytes()[run_start..offset])?;
        }
        match escape {
            Escape::Simple(text) => writer.write_all(text.as_bytes())?,
            Escape::Unicode(code) => write_unicode_escape(writer, code)?,
        }
        run_start = offset + character.len_utf8();
    }
    if run_start < text.len() {
        writer.write_all(&text.as_bytes()[run_start..])?;
    }
    writer.write_all(b"\"")
}

/// Write `\uXXXX`, using a surrogate pair above `U+FFFF`.
fn write_unicode_escape<W: Write>(writer: &mut W, code: u32) -> io::Result<()> {
    if code <= 0xFFFF {
        write!(writer, "\\u{code:04x}")
    } else {
        let code = code - 0x1_0000;
        let high = 0xD800 + (code >> 10);
        let low = 0xDC00 + (code & 0x3FF);
        write!(writer, "\\u{high:04x}\\u{low:04x}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::parse;

    #[test]
    fn pretty_output_is_indented() {
        let value = parse(r#"{"a":[1,2],"b":{}}"#).unwrap();
        assert_eq!(
            to_string_pretty(&value),
            "{\n  \"a\": [\n    1,\n    2\n  ],\n  \"b\": {}\n}"
        );
    }

    #[test]
    fn escaping_control_characters() {
        let value = Value::String("\u{0001}\n".to_owned());
        assert_eq!(to_string(&value), "\"\\u0001\\n\"");
    }
}
