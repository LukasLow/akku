//! HTML as its own domain.
//!
//! `Html` is trusted markup. Build it with [`Html::new`] or [`Html::from`] and
//! let the HTTP layer turn it into a response. Text that must be escaped goes
//! through [`escape`]. Text that carries character references is decoded with
//! [`unescape`].

mod entities;

use std::borrow::Cow;
use std::fmt;

/// Trusted HTML markup.
///
/// Constructing `Html` is an explicit statement that the string is already
/// valid, trusted markup. It is not escaped again by the HTTP layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Html(String);

impl Html {
    /// Create trusted HTML from a string.
    pub fn new(body: impl Into<String>) -> Self {
        Self(body.into())
    }

    /// Borrow the markup.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consume the wrapper and return the markup.
    pub fn into_string(self) -> String {
        self.0
    }
}

impl From<&str> for Html {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for Html {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl fmt::Display for Html {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Escape text for use in HTML text and quoted attribute values.
///
/// The replacements are `&`, `<`, `>`, `"` and `'`.
pub fn escape(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    for character in input.chars() {
        match character {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '"' => output.push_str("&#34;"),
            '\'' => output.push_str("&#39;"),
            _ => output.push(character),
        }
    }
    output
}

/// Windows-1252 replacements for the numeric references `&#128;` to `&#159;`.
///
/// `'\0'` marks the five unassigned slots, which keep their C1 code point.
const C1: [char; 32] = [
    '\u{20AC}', '\0', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}', '\u{2021}',
    '\u{02C6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\0', '\u{017D}', '\0', '\0',
    '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}', '\u{2022}', '\u{2013}', '\u{2014}', '\u{02DC}',
    '\u{2122}', '\u{0161}', '\u{203A}', '\u{0153}', '\0', '\u{017E}', '\u{0178}',
];

/// Longest named reference key in the table, including the `;`.
const MAX_NAME: usize = 32;

/// Decode HTML character references.
///
/// Follows the WHATWG HTML tokenizer (the same behaviour as Go's
/// `html.UnescapeString` and Python's `html.unescape`):
///
/// - Decimal and hexadecimal references (`&#39;`, `&#x27;`, `&#X27;`) are
///   decoded. A missing `;` is accepted. Zero, surrogates and values above
///   U+10FFFF (including values too large for any integer) become U+FFFD, and
///   `&#128;` to `&#159;` map through the Windows-1252 table.
/// - All 2,231 WHATWG named references are decoded. Legacy names (`&amp`,
///   `&lt`, `&copy`, ...) also work without `;`, and the longest matching
///   prefix wins, so `&notit;` becomes `¬it;`.
/// - Anything else, such as `&`, `&;`, `&#;`, `&#x;` or `&unknown;`, is kept
///   literally.
///
/// Decoding is a single pass in linear time; decoded text is never decoded
/// again. The result borrows the input when there is nothing to decode, which
/// is the common case. Unlike [`escape`], which always builds a new string,
/// `unescape` therefore allocates only when it has to.
///
/// ```
/// use akku::html::unescape;
///
/// assert_eq!(unescape("&lt;b&gt; &amp; &#39;x&#x27;"), "<b> & 'x'");
/// assert_eq!(unescape("&notit;"), "\u{AC}it;");
/// assert_eq!(unescape("&#99999999999999999999;"), "\u{FFFD}");
/// assert!(matches!(unescape("no references"), std::borrow::Cow::Borrowed(_)));
/// ```
pub fn unescape(s: &str) -> Cow<'_, str> {
    let bytes = s.as_bytes();
    let Some(mut at) = bytes.iter().position(|&b| b == b'&') else {
        return Cow::Borrowed(s);
    };
    let mut out: Option<String> = None;
    let mut copied = 0; // bytes of `s` already handled
    loop {
        // `at` is the index of a `&`.
        if let Some((used, repl)) = decode(&bytes[at + 1..]) {
            let buf = out.get_or_insert_with(|| String::with_capacity(s.len()));
            buf.push_str(&s[copied..at]);
            match repl {
                Repl::Char(c) => buf.push(c),
                Repl::Str(text) => buf.push_str(text),
            }
            copied = at + 1 + used;
            at = copied;
        } else {
            at += 1;
        }
        match bytes[at..].iter().position(|&b| b == b'&') {
            Some(offset) => at += offset,
            None => break,
        }
    }
    match out {
        None => Cow::Borrowed(s),
        Some(mut buf) => {
            buf.push_str(&s[copied..]);
            Cow::Owned(buf)
        }
    }
}

enum Repl {
    Char(char),
    Str(&'static str),
}

/// Decode the reference that follows a `&`; returns the bytes consumed after it.
fn decode(rest: &[u8]) -> Option<(usize, Repl)> {
    match rest.first()? {
        b'#' => decode_numeric(rest),
        b if b.is_ascii_alphanumeric() => decode_named(rest),
        _ => None,
    }
}

fn decode_numeric(rest: &[u8]) -> Option<(usize, Repl)> {
    let (radix, mut i) = match rest.get(1) {
        Some(b'x' | b'X') => (16, 2),
        _ => (10, 1),
    };
    let digits_start = i;
    let mut value: u32 = 0;
    while let Some(digit) = rest.get(i).and_then(|&b| (b as char).to_digit(radix)) {
        // Saturate above the Unicode range so huge references cannot overflow.
        value = value
            .saturating_mul(radix)
            .saturating_add(digit)
            .min(0x11_0000);
        i += 1;
    }
    if i == digits_start {
        return None;
    }
    if rest.get(i) == Some(&b';') {
        i += 1;
    }
    let c = match value {
        0x80..=0x9F => match C1[(value - 0x80) as usize] {
            '\0' => char::from_u32(value).unwrap_or('\u{FFFD}'),
            mapped => mapped,
        },
        0 => '\u{FFFD}',
        v => char::from_u32(v).unwrap_or('\u{FFFD}'),
    };
    Some((i, Repl::Char(c)))
}

fn decode_named(rest: &[u8]) -> Option<(usize, Repl)> {
    let run = rest
        .iter()
        .take(MAX_NAME)
        .take_while(|b| b.is_ascii_alphanumeric())
        .count();
    // The run is ASCII, so every slice below is valid UTF-8.
    let key = |len: usize| std::str::from_utf8(&rest[..len]).ok();
    if rest.get(run) == Some(&b';') {
        if let Some(text) = lookup(key(run + 1)?) {
            return Some((run + 1, Repl::Str(text)));
        }
    }
    // Legacy references may omit the `;`; take the longest one.
    (2..=run)
        .rev()
        .find_map(|len| lookup(key(len)?).map(|text| (len, Repl::Str(text))))
}

fn lookup(name: &str) -> Option<&'static str> {
    entities::ENTITIES
        .binary_search_by(|(key, _)| (*key).cmp(name))
        .ok()
        .map(|index| entities::ENTITIES[index].1)
}

#[cfg(feature = "http")]
impl axum::response::IntoResponse for Html {
    fn into_response(self) -> axum::response::Response {
        (
            [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
            self.0,
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::{escape, Html, MAX_NAME};

    #[test]
    fn entity_table_is_sorted_and_bounded() {
        let table = super::entities::ENTITIES;
        assert_eq!(table.len(), 2231);
        assert!(table.windows(2).all(|pair| pair[0].0 < pair[1].0));
        assert!(table.iter().all(|(key, _)| key.len() <= MAX_NAME));
    }

    #[test]
    fn escapes_the_five_html_characters() {
        assert_eq!(
            escape(r#"a & b < c > d "e" 'f'"#),
            "a &amp; b &lt; c &gt; d &#34;e&#34; &#39;f&#39;"
        );
    }

    #[test]
    fn html_is_trusted_and_keeps_its_markup() {
        let html = Html::new("<h1>Hello</h1>");
        assert_eq!(html.as_str(), "<h1>Hello</h1>");
        assert_eq!(html.to_string(), "<h1>Hello</h1>");
    }
}
