//! The built-in filters.
//!
//! A filter receives its input and arguments and returns a value or a message
//! describing what was wrong. Only `safe`, `escape`, `json` and `trim` keep
//! markup safe; every other filter returns plain (escaped-on-output) text, so
//! a filter can never turn untrusted text into trusted markup by accident.
//! (`default` is handled by the evaluator because it must accept undefined
//! values.)

use std::borrow::Cow;
use std::io;

use crate::json;

use super::value::Value;

/// Why a built-in filter failed.
#[derive(Debug)]
pub(crate) enum FilterError {
    /// The input or arguments were unsuitable; the message says why.
    Message(String),
    /// The result would exceed the output limit; nothing was allocated.
    TooLarge,
}

impl From<String> for FilterError {
    fn from(message: String) -> Self {
        FilterError::Message(message)
    }
}

/// The signature of a built-in filter. The last argument is the largest
/// result (in bytes) the filter may produce; amplifying filters check it
/// *before* allocating.
pub(crate) type BuiltinFn = fn(&Value, &[Value], usize) -> Result<Value, FilterError>;

/// The built-in filter called `name`.
pub(crate) fn builtin(name: &str) -> Option<BuiltinFn> {
    Some(match name {
        "upper" => upper,
        "lower" => lower,
        "trim" => trim,
        "len" => len,
        "escape" | "e" => escape_filter,
        "safe" => safe,
        "join" => join,
        "replace" => replace,
        "json" => json_filter,
        _ => return None,
    })
}

fn arity(args: &[Value], min: usize, max: usize) -> Result<(), String> {
    if args.len() < min || args.len() > max {
        let expected = if min == max {
            format!("{min}")
        } else {
            format!("{min} to {max}")
        };
        return Err(format!(
            "expected {expected} argument(s), found {}",
            args.len()
        ));
    }
    Ok(())
}

/// Fail with [`FilterError::TooLarge`] when `len` exceeds `limit`.
fn cap(len: usize, limit: usize) -> Result<(), FilterError> {
    if len > limit {
        Err(FilterError::TooLarge)
    } else {
        Ok(())
    }
}

fn text(value: &Value) -> Result<Cow<'_, str>, String> {
    value
        .scalar_text()
        .ok_or_else(|| format!("expected a string or number, found a {}", value.type_name()))
}

/// Map every character of `input` through `push`, failing as soon as the
/// output grows past `limit` (case mapping can expand text up to 3x).
fn map_chars(
    input: &str,
    limit: usize,
    mut push: impl FnMut(char, &mut String),
) -> Result<String, FilterError> {
    cap(input.len(), limit)?;
    let mut output = String::with_capacity(input.len());
    for character in input.chars() {
        push(character, &mut output);
        cap(output.len(), limit)?;
    }
    Ok(output)
}

fn upper(input: &Value, args: &[Value], limit: usize) -> Result<Value, FilterError> {
    arity(args, 0, 0)?;
    let input = text(input)?;
    let output = map_chars(&input, limit, |c, out| out.extend(c.to_uppercase()))?;
    Ok(Value::Str(output))
}

fn lower(input: &Value, args: &[Value], limit: usize) -> Result<Value, FilterError> {
    arity(args, 0, 0)?;
    let input = text(input)?;
    let output = map_chars(&input, limit, |c, out| out.extend(c.to_lowercase()))?;
    Ok(Value::Str(output))
}

/// Trimming cannot introduce markup, so safe text stays safe.
fn trim(input: &Value, args: &[Value], _limit: usize) -> Result<Value, FilterError> {
    arity(args, 0, 0)?;
    let trimmed = text(input)?.trim().to_owned();
    Ok(match input {
        Value::Safe(_) => Value::Safe(trimmed),
        _ => Value::Str(trimmed),
    })
}

fn len(input: &Value, args: &[Value], _limit: usize) -> Result<Value, FilterError> {
    arity(args, 0, 0)?;
    let length = match input {
        Value::Str(text) | Value::Safe(text) => text.chars().count(),
        Value::List(items) => items.len(),
        Value::Map(entries) => entries.len(),
        other => return Err(format!("cannot take the length of a {}", other.type_name()).into()),
    };
    Ok(Value::from(length))
}

/// HTML-escape; already safe markup is left alone (no double escaping).
fn escape_filter(input: &Value, args: &[Value], limit: usize) -> Result<Value, FilterError> {
    arity(args, 0, 0)?;
    Ok(match input {
        Value::Safe(_) => input.clone(),
        other => {
            let input = text(other)?;
            // The exact escaped length is known before anything is written.
            let extra: usize = input
                .bytes()
                .map(|byte| match byte {
                    b'&' | b'"' | b'\'' => 4,
                    b'<' | b'>' => 3,
                    _ => 0,
                })
                .sum();
            cap(input.len().saturating_add(extra), limit)?;
            Value::Safe(crate::html::escape(&input))
        }
    })
}

fn safe(input: &Value, args: &[Value], _limit: usize) -> Result<Value, FilterError> {
    arity(args, 0, 0)?;
    Ok(Value::Safe(text(input)?.into_owned()))
}

/// Items are joined as plain text; the (unsafe) result is escaped as a whole
/// on output.
fn join(input: &Value, args: &[Value], limit: usize) -> Result<Value, FilterError> {
    arity(args, 0, 1)?;
    let Value::List(items) = input else {
        return Err(format!("expected a list, found a {}", input.type_name()).into());
    };
    let separator = match args.first() {
        Some(separator) => text(separator)?,
        None => Cow::Borrowed(""),
    };
    let mut joined = String::new();
    for (index, item) in items.iter().enumerate() {
        let item = text(item)?;
        let extra = if index > 0 { separator.len() } else { 0 };
        let needed = joined
            .len()
            .checked_add(extra)
            .and_then(|len| len.checked_add(item.len()))
            .ok_or(FilterError::TooLarge)?;
        cap(needed, limit)?;
        if index > 0 {
            joined.push_str(&separator);
        }
        joined.push_str(&item);
    }
    Ok(Value::Str(joined))
}

fn replace(input: &Value, args: &[Value], limit: usize) -> Result<Value, FilterError> {
    arity(args, 2, 2)?;
    let from = text(&args[0])?;
    let to = text(&args[1])?;
    if from.is_empty() {
        return Err("the text to replace must not be empty".to_owned().into());
    }
    let input = text(input)?;
    // Compute the exact result length without allocating; a huge input
    // times a long replacement must fail before any memory is reserved.
    let matches = input.matches(from.as_ref()).count();
    let removed = matches
        .checked_mul(from.len())
        .ok_or(FilterError::TooLarge)?;
    let added = matches.checked_mul(to.len()).ok_or(FilterError::TooLarge)?;
    let result_len = (input.len() - removed)
        .checked_add(added)
        .ok_or(FilterError::TooLarge)?;
    cap(result_len, limit)?;
    Ok(Value::Str(input.replace(from.as_ref(), &to)))
}

/// Compact JSON that is safe inside `<script>` and single-quoted attributes:
/// `<`, `>`, `&`, `'` and the line separators are written as `\uXXXX`.
fn json_filter(input: &Value, args: &[Value], limit: usize) -> Result<Value, FilterError> {
    arity(args, 0, 0)?;
    if let Value::Str(text) | Value::Safe(text) = input {
        // The encoding is never shorter than the text itself.
        cap(text.len(), limit)?;
    }
    // The writer refuses to grow past the limit, so escapes cannot amplify
    // the output beyond it before the check.
    let mut writer = BoundedWriter {
        buffer: Vec::new(),
        limit,
    };
    let options = json::SerializeOptions::new().with_escape_html(true);
    json::write_to_with(&json::Value::from(input.clone()), &mut writer, options)
        .map_err(|_| FilterError::TooLarge)?;
    let encoded = String::from_utf8(writer.buffer).expect("JSON output is valid UTF-8");
    // The line separators are three bytes raw and six escaped.
    let separators = encoded.matches(['\u{2028}', '\u{2029}']).count();
    cap(
        encoded.len().saturating_add(separators.saturating_mul(3)),
        limit,
    )?;
    if separators == 0 {
        return Ok(Value::Safe(encoded));
    }
    let safe = encoded
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029");
    Ok(Value::Safe(safe))
}

/// An in-memory writer that fails instead of growing past `limit` bytes.
struct BoundedWriter {
    buffer: Vec<u8>,
    limit: usize,
}

impl io::Write for BoundedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.buffer.len().saturating_add(bytes.len()) > self.limit {
            return Err(io::Error::other("output limit exceeded"));
        }
        self.buffer.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
