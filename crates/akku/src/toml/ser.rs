//! The TOML writer.

use std::fmt::Write as _;
use std::io::Write;

use super::error::{Error, ErrorKind, Position, Result};
use super::value::{Table, Value};

/// How a [`Table`] is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SerializeOptions {
    /// Insert a blank line between tables and spread long arrays over lines.
    pub pretty: bool,
    /// Write arrays of tables as `[[name]]` sections.
    pub array_of_tables: bool,
    /// Sort keys alphabetically instead of keeping insertion order.
    pub sort_keys: bool,
}

impl Default for SerializeOptions {
    fn default() -> Self {
        Self {
            pretty: true,
            array_of_tables: true,
            sort_keys: false,
        }
    }
}

/// Serialize a table with the default [`SerializeOptions`].
pub fn to_string(table: &Table) -> String {
    to_string_with(table, SerializeOptions::default())
}

/// Serialize a table with explicit [`SerializeOptions`].
pub fn to_string_with(table: &Table, options: SerializeOptions) -> String {
    let mut output = String::new();
    write_table(&mut output, table, "", &options);
    output
}

/// Write a table to any [`Write`] using the default [`SerializeOptions`].
pub fn write_to<W: Write>(table: &Table, mut writer: W) -> Result<()> {
    writer
        .write_all(to_string(table).as_bytes())
        .map_err(|error| Error::new(ErrorKind::Io(error), Position::default()))
}

fn write_table(output: &mut String, table: &Table, path: &str, options: &SerializeOptions) {
    let mut entries: Vec<&(String, Value)> = table.0.iter().collect();
    if options.sort_keys {
        entries.sort_by(|left, right| left.0.cmp(&right.0));
    }

    for (key, value) in &entries {
        if matches!(value, Value::Table(_)) {
            continue;
        }
        if options.array_of_tables && is_table_array(value) {
            continue;
        }
        output.push_str(&format_key(key));
        output.push_str(" = ");
        push_value(output, value, options);
        output.push('\n');
    }

    for (key, value) in &entries {
        if let Value::Table(child) = value {
            let child_path = join_path(path, key);
            if options.pretty && !output.is_empty() {
                output.push('\n');
            }
            output.push('[');
            output.push_str(&child_path);
            output.push_str("]\n");
            write_table(output, child, &child_path, options);
        }
    }

    if options.array_of_tables {
        for (key, value) in &entries {
            if !is_table_array(value) {
                continue;
            }
            let child_path = join_path(path, key);
            let items = match value {
                Value::Array(items) => items,
                _ => continue,
            };
            for item in items {
                if options.pretty && !output.is_empty() {
                    output.push('\n');
                }
                output.push_str("[[");
                output.push_str(&child_path);
                output.push_str("]]\n");
                if let Value::Table(child) = item {
                    write_table(output, child, &child_path, options);
                }
            }
        }
    }
}

fn is_table_array(value: &Value) -> bool {
    match value {
        Value::Array(items) => {
            !items.is_empty() && items.iter().all(|item| matches!(item, Value::Table(_)))
        }
        _ => false,
    }
}

fn push_value(output: &mut String, value: &Value, options: &SerializeOptions) {
    match value {
        Value::String(text) => push_basic_string(output, text),
        Value::Integer(integer) => {
            let _ = write!(output, "{integer}");
        }
        Value::Float(float) => output.push_str(&format_float(*float)),
        Value::Boolean(boolean) => {
            output.push_str(if *boolean { "true" } else { "false" });
        }
        Value::Datetime(datetime) => {
            let _ = write!(output, "{datetime}");
        }
        Value::Array(items) => push_array(output, items, options),
        Value::Table(table) => {
            if table.is_empty() {
                output.push_str("{}");
                return;
            }
            output.push_str("{ ");
            let mut first = true;
            for (key, item) in &table.0 {
                if !first {
                    output.push_str(", ");
                }
                first = false;
                output.push_str(&format_key(key));
                output.push_str(" = ");
                push_value(output, item, options);
            }
            output.push_str(" }");
        }
    }
}

fn push_array(output: &mut String, items: &[Value], options: &SerializeOptions) {
    if items.is_empty() {
        output.push_str("[]");
        return;
    }
    let multiline = options.pretty && items.len() > 3;
    if !multiline {
        output.push('[');
        for (index, item) in items.iter().enumerate() {
            if index > 0 {
                output.push_str(", ");
            }
            push_value(output, item, options);
        }
        output.push(']');
        return;
    }
    output.push('[');
    for item in items {
        output.push('\n');
        output.push_str("  ");
        push_value(output, item, options);
        output.push(',');
    }
    output.push('\n');
    output.push(']');
}

fn format_float(value: f64) -> String {
    if value.is_nan() {
        return "nan".to_string();
    }
    if value.is_infinite() {
        return if value.is_sign_negative() {
            "-inf".to_string()
        } else {
            "inf".to_string()
        };
    }
    let mut text = format!("{value}");
    if !text.contains(['.', 'e', 'E']) {
        text.push_str(".0");
    }
    text
}

fn format_key(key: &str) -> String {
    let bare = !key.is_empty()
        && key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-');
    if bare {
        return key.to_string();
    }
    let mut text = String::new();
    push_basic_string(&mut text, key);
    text
}

fn push_basic_string(output: &mut String, value: &str) {
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            '\u{8}' => output.push_str("\\b"),
            '\u{c}' => output.push_str("\\f"),
            character if (character as u32) < 0x20 || character == '\u{7f}' => {
                let _ = write!(output, "\\u{:04X}", character as u32);
            }
            character => output.push(character),
        }
    }
    output.push('"');
}

fn join_path(path: &str, key: &str) -> String {
    if path.is_empty() {
        format_key(key)
    } else {
        format!("{path}.{}", format_key(key))
    }
}

/// Format a single value the way it appears in a document.
pub(crate) fn value_to_string(value: &Value) -> String {
    let mut output = String::new();
    push_value(&mut output, value, &SerializeOptions::default());
    output
}
