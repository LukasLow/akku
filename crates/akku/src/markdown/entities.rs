//! HTML entity decoding.
//!
//! Numeric references are fully supported. Of the named references only a
//! curated set of the most common ones is known (the full HTML5 table has
//! over two thousand entries); an unknown `&name;` stays literal text.

/// Look up a named entity (without `&` and `;`).
fn named(name: &str) -> Option<&'static str> {
    Some(match name {
        "amp" | "AMP" => "&",
        "lt" | "LT" => "<",
        "gt" | "GT" => ">",
        "quot" | "QUOT" => "\"",
        "apos" => "'",
        "nbsp" => "\u{a0}",
        "copy" | "COPY" => "\u{a9}",
        "reg" | "REG" => "\u{ae}",
        "trade" | "TRADE" => "\u{2122}",
        "hellip" => "\u{2026}",
        "mdash" => "\u{2014}",
        "ndash" => "\u{2013}",
        "lsquo" => "\u{2018}",
        "rsquo" => "\u{2019}",
        "ldquo" => "\u{201c}",
        "rdquo" => "\u{201d}",
        "laquo" => "\u{ab}",
        "raquo" => "\u{bb}",
        "bull" => "\u{2022}",
        "middot" => "\u{b7}",
        "deg" => "\u{b0}",
        "plusmn" => "\u{b1}",
        "times" => "\u{d7}",
        "divide" => "\u{f7}",
        "euro" => "\u{20ac}",
        "pound" => "\u{a3}",
        "yen" => "\u{a5}",
        "cent" => "\u{a2}",
        "sect" => "\u{a7}",
        "para" => "\u{b6}",
        "iexcl" => "\u{a1}",
        "iquest" => "\u{bf}",
        "frac12" => "\u{bd}",
        "frac14" => "\u{bc}",
        "frac34" => "\u{be}",
        "micro" => "\u{b5}",
        "larr" => "\u{2190}",
        "rarr" => "\u{2192}",
        "uarr" => "\u{2191}",
        "darr" => "\u{2193}",
        "hearts" => "\u{2665}",
        "AElig" => "\u{c6}",
        "aelig" => "\u{e6}",
        "auml" => "\u{e4}",
        "ouml" => "\u{f6}",
        "uuml" => "\u{fc}",
        "Auml" => "\u{c4}",
        "Ouml" => "\u{d6}",
        "Uuml" => "\u{dc}",
        "szlig" => "\u{df}",
        "eacute" => "\u{e9}",
        "egrave" => "\u{e8}",
        _ => return None,
    })
}

/// Decode a character reference at the start of `s` (which begins with `&`).
///
/// Returns the decoded text and the number of bytes consumed, or `None` when
/// `s` does not start with a known, `;`-terminated reference.
pub(crate) fn parse(s: &str) -> Option<(String, usize)> {
    let bytes = s.as_bytes();
    if bytes.first() != Some(&b'&') {
        return None;
    }
    if bytes.get(1) == Some(&b'#') {
        let (hex, digits_start) = match bytes.get(2) {
            Some(b'x') | Some(b'X') => (true, 3),
            _ => (false, 2),
        };
        let mut end = digits_start;
        while end < bytes.len()
            && (if hex {
                bytes[end].is_ascii_hexdigit()
            } else {
                bytes[end].is_ascii_digit()
            })
        {
            end += 1;
        }
        let count = end - digits_start;
        let max = if hex { 6 } else { 7 };
        if count == 0 || count > max || bytes.get(end) != Some(&b';') {
            return None;
        }
        let radix = if hex { 16 } else { 10 };
        let value = u32::from_str_radix(&s[digits_start..end], radix).ok()?;
        let ch = if value == 0 {
            '\u{fffd}'
        } else {
            char::from_u32(value).unwrap_or('\u{fffd}')
        };
        return Some((ch.to_string(), end + 1));
    }
    let mut end = 1;
    while end < bytes.len() && bytes[end].is_ascii_alphanumeric() && end <= 32 {
        end += 1;
    }
    if end == 1 || bytes.get(end) != Some(&b';') {
        return None;
    }
    named(&s[1..end]).map(|text| (text.to_owned(), end + 1))
}

/// Process backslash escapes and entity references in `s`.
pub(crate) fn unescape(s: &str) -> String {
    if !s.contains(['\\', '&']) {
        return s.to_owned();
    }
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if bytes.get(i + 1).is_some_and(u8::is_ascii_punctuation) => {
                out.push(bytes[i + 1] as char);
                i += 2;
            }
            b'&' => match parse(&s[i..]) {
                Some((text, used)) => {
                    out.push_str(&text);
                    i += used;
                }
                None => {
                    out.push('&');
                    i += 1;
                }
            },
            _ => {
                let ch = s[i..].chars().next().unwrap_or('\u{fffd}');
                out.push(ch);
                i += ch.len_utf8();
            }
        }
    }
    out
}
