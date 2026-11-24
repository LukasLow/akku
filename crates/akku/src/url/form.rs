//! `application/x-www-form-urlencoded` parsing and building.

use std::borrow::Cow;
use std::iter::FusedIterator;

use super::percent::{hex_val, push_percent_encoded, should_encode, FORM};
use super::Url;

/// Encodes a single `application/x-www-form-urlencoded` component.
///
/// Spaces (`' '`) are encoded as `'+'`, and all bytes other than ASCII
/// alphanumerics, `'*'`, `'-'`, `'.'`, and `'_'` are percent-encoded with
/// uppercase hex.
///
/// # Examples
///
/// ```
/// use akku::url::form_encode;
///
/// assert_eq!(form_encode("hello world!"), "hello+world%21");
/// ```
pub fn form_encode(input: &str) -> String {
    encode_component(input, true)
}

fn encode_component(input: &str, plus_for_space: bool) -> String {
    let mut out = String::with_capacity(input.len());
    for &b in input.as_bytes() {
        if b == b' ' && plus_for_space {
            out.push('+');
        } else if should_encode(b, &FORM) {
            push_percent_encoded(&mut out, b);
        } else {
            out.push(b as char);
        }
    }
    out
}

/// Decodes a single `application/x-www-form-urlencoded` component.
///
/// Replaces `'+'` with `' '`, leniently decodes valid `%XX` escapes, and
/// converts invalid UTF-8 sequences with lossy replacement.
/// Returns [`Cow::Borrowed`] when no `'+'` or valid `%XX` escape is present.
///
/// # Examples
///
/// ```
/// use akku::url::form_decode;
///
/// assert_eq!(form_decode("hello+world%21"), "hello world!");
/// ```
pub fn form_decode(input: &str) -> Cow<'_, str> {
    let bytes = input.as_bytes();
    let mut needs_decode = false;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'+' {
            needs_decode = true;
            break;
        }
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && hex_val(bytes[i + 1]).is_some()
            && hex_val(bytes[i + 2]).is_some()
        {
            needs_decode = true;
            break;
        }
        i += 1;
    }

    if !needs_decode {
        return Cow::Borrowed(input);
    }

    let mut out = Vec::with_capacity(bytes.len());
    out.extend_from_slice(&bytes[..i]);

    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                if let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                    out.push((hi << 4) | lo);
                    i += 3;
                } else {
                    out.push(b'%');
                    i += 1;
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }

    Cow::Owned(match String::from_utf8(out) {
        Ok(s) => s,
        Err(err) => String::from_utf8_lossy(err.as_bytes()).into_owned(),
    })
}

/// Iterator over `key=value` pairs in an `application/x-www-form-urlencoded` string.
///
/// Empty segments (such as leading/trailing `&` or `&&`) are skipped. Segments
/// without `=` yield `(key, "")`.
///
/// # Examples
///
/// ```
/// use akku::url::FormUrlEncoded;
///
/// let pairs: Vec<_> = FormUrlEncoded::parse("a=1&b=hello+world&&flag")
///     .map(|(k, v)| (k.into_owned(), v.into_owned()))
///     .collect();
/// assert_eq!(
///     pairs,
///     vec![
///         ("a".into(), "1".into()),
///         ("b".into(), "hello world".into()),
///         ("flag".into(), "".into()),
///     ]
/// );
/// ```
#[derive(Debug, Clone)]
pub struct FormUrlEncoded<'a> {
    segments: std::str::Split<'a, char>,
    empty: bool,
}

impl<'a> FormUrlEncoded<'a> {
    /// Parses an `application/x-www-form-urlencoded` query or body string.
    pub fn parse(input: &'a str) -> Self {
        Self {
            segments: input.split('&'),
            empty: input.is_empty(),
        }
    }
}

impl<'a> Iterator for FormUrlEncoded<'a> {
    type Item = (Cow<'a, str>, Cow<'a, str>);

    fn next(&mut self) -> Option<Self::Item> {
        if self.empty {
            return None;
        }
        for segment in self.segments.by_ref() {
            if segment.is_empty() {
                continue;
            }
            return Some(match segment.split_once('=') {
                Some((k, v)) => (form_decode(k), form_decode(v)),
                None => (form_decode(segment), Cow::Borrowed("")),
            });
        }
        None
    }
}

impl FusedIterator for FormUrlEncoded<'_> {}

#[derive(Debug, Clone)]
enum QueryEntry {
    Pair(String, String),
    Flag(String),
}

/// Builder for `application/x-www-form-urlencoded` query strings.
///
/// By default, spaces are encoded as `+` (configurable via
/// [`QueryBuilder::plus_for_space`]).
///
/// # Examples
///
/// ```
/// use akku::url::QueryBuilder;
///
/// let query = QueryBuilder::new()
///     .pair("q", "rust batteries")
///     .flag("exact")
///     .finish();
/// assert_eq!(query, "q=rust+batteries&exact");
/// ```
#[derive(Debug, Clone)]
pub struct QueryBuilder {
    entries: Vec<QueryEntry>,
    plus_for_space: bool,
}

impl QueryBuilder {
    /// Creates an empty [`QueryBuilder`] with `plus_for_space` enabled.
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            plus_for_space: true,
        }
    }

    /// Configures whether spaces are encoded as `'+'` (`true`, default) or `"%20"` (`false`).
    pub fn plus_for_space(mut self, plus: bool) -> Self {
        self.plus_for_space = plus;
        self
    }

    /// Appends a `name=value` pair.
    pub fn pair(mut self, name: &str, value: &str) -> Self {
        self.entries
            .push(QueryEntry::Pair(name.to_owned(), value.to_owned()));
        self
    }

    /// Appends a key-only flag without `=`.
    pub fn flag(mut self, name: &str) -> Self {
        self.entries.push(QueryEntry::Flag(name.to_owned()));
        self
    }

    /// Appends an iterator of `(name, value)` pairs.
    pub fn extend<I, K, V>(mut self, pairs: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<str>,
        V: AsRef<str>,
    {
        for (k, v) in pairs {
            self.entries.push(QueryEntry::Pair(
                k.as_ref().to_owned(),
                v.as_ref().to_owned(),
            ));
        }
        self
    }

    /// Returns `true` if no pairs or flags have been added.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Encodes all pairs and flags into a query string (without a leading `?`).
    pub fn finish(&self) -> String {
        let mut out = String::new();
        for (idx, entry) in self.entries.iter().enumerate() {
            if idx > 0 {
                out.push('&');
            }
            match entry {
                QueryEntry::Pair(k, v) => {
                    out.push_str(&encode_component(k, self.plus_for_space));
                    out.push('=');
                    out.push_str(&encode_component(v, self.plus_for_space));
                }
                QueryEntry::Flag(k) => {
                    out.push_str(&encode_component(k, self.plus_for_space));
                }
            }
        }
        out
    }

    /// Applies this query to `url`, clearing its query if [`QueryBuilder::is_empty`] is `true`.
    pub fn apply(&self, url: &mut Url) {
        if self.is_empty() {
            url.set_query(None);
        } else {
            let encoded = self.finish();
            url.set_query(Some(&encoded));
        }
    }
}

impl Default for QueryBuilder {
    fn default() -> Self {
        Self::new()
    }
}
