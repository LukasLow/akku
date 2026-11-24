//! Word splitting, identifier case conversion, and ASCII URL slugification.

use crate::text::normalize::nfd;
use crate::text::tables::is_combining_mark;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CharClass {
    Lower,
    Upper,
    Digit,
    OtherAlpha,
    Delim,
}

#[inline]
fn classify(ch: char) -> CharClass {
    if !ch.is_alphanumeric() {
        CharClass::Delim
    } else if ch.is_lowercase() {
        CharClass::Lower
    } else if ch.is_uppercase() {
        CharClass::Upper
    } else if ch.is_numeric() {
        CharClass::Digit
    } else {
        CharClass::OtherAlpha
    }
}

/// Split an identifier or phrase into word slices along delimiters and case boundaries.
///
/// Handles `snake_case`, `kebab-case`, `camelCase`, `PascalCase`, acronyms (`HTTPServer`),
/// and versioned tokens (`getV2Response`).
pub fn split_words(input: &str) -> Vec<&str> {
    let mut words = Vec::new();
    let chars: Vec<(usize, char, CharClass)> = input
        .char_indices()
        .map(|(idx, ch)| (idx, ch, classify(ch)))
        .collect();

    let mut i = 0;
    while i < chars.len() {
        if chars[i].2 == CharClass::Delim {
            i += 1;
            continue;
        }
        let start = chars[i].0;
        let mut end = start + chars[i].1.len_utf8();
        let mut prev_cls = chars[i].2;
        i += 1;

        while i < chars.len() {
            let (idx, ch, cls) = chars[i];
            if cls == CharClass::Delim {
                break;
            }
            let next_cls = chars.get(i + 1).map(|t| t.2);
            let boundary = matches!(
                (prev_cls, cls, next_cls),
                (CharClass::Lower, CharClass::Upper, _)
                    | (CharClass::Digit, CharClass::Upper, _)
                    | (CharClass::Upper, CharClass::Upper, Some(CharClass::Lower))
            );
            if boundary {
                break;
            }
            end = idx + ch.len_utf8();
            prev_cls = cls;
            i += 1;
        }

        words.push(&input[start..end]);
    }

    words
}

/// Convert `input` to `snake_case`.
pub fn to_snake_case(input: &str) -> String {
    join_lower(input, '_')
}

/// Convert `input` to `kebab-case`.
pub fn to_kebab_case(input: &str) -> String {
    join_lower(input, '-')
}

/// Convert `input` to `camelCase`.
pub fn to_camel_case(input: &str) -> String {
    let words = split_words(input);
    let mut out = String::with_capacity(input.len());
    for (idx, word) in words.into_iter().enumerate() {
        if idx == 0 {
            push_lowercase(&mut out, word);
        } else {
            push_capitalized(&mut out, word);
        }
    }
    out
}

/// Convert `input` to `PascalCase`.
pub fn to_pascal_case(input: &str) -> String {
    let words = split_words(input);
    let mut out = String::with_capacity(input.len());
    for word in words {
        push_capitalized(&mut out, word);
    }
    out
}

/// Convert `input` into a lowercase ASCII URL slug (`"Crème Brûlée!"` -> `"creme-brulee"`).
pub fn slugify(input: &str) -> String {
    let decomposed = nfd(input);
    let mut out = String::with_capacity(input.len());
    let mut pending_dash = false;

    for ch in decomposed.chars() {
        if is_combining_mark(ch) {
            continue;
        }
        let mapped: Option<&str> = match ch {
            'ß' => Some("ss"),
            'æ' | 'Æ' => Some("ae"),
            'œ' | 'Œ' => Some("oe"),
            'ø' | 'Ø' => Some("o"),
            'đ' | 'Đ' | 'ð' | 'Ð' => Some("d"),
            'ł' | 'Ł' => Some("l"),
            'þ' | 'Þ' => Some("th"),
            _ => None,
        };
        if let Some(rep) = mapped {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.push_str(rep);
        } else if ch.is_ascii_alphanumeric() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.push(ch.to_ascii_lowercase());
        } else if !out.is_empty() {
            pending_dash = true;
        }
    }

    out
}

fn join_lower(input: &str, sep: char) -> String {
    let words = split_words(input);
    let mut out = String::with_capacity(input.len() + words.len());
    for (idx, word) in words.into_iter().enumerate() {
        if idx > 0 {
            out.push(sep);
        }
        push_lowercase(&mut out, word);
    }
    out
}

fn push_lowercase(out: &mut String, word: &str) {
    for ch in word.chars() {
        for lc in ch.to_lowercase() {
            out.push(lc);
        }
    }
}

fn push_capitalized(out: &mut String, word: &str) {
    let mut chars = word.chars();
    if let Some(first) = chars.next() {
        for uc in first.to_uppercase() {
            out.push(uc);
        }
    }
    for ch in chars {
        for lc in ch.to_lowercase() {
            out.push(lc);
        }
    }
}
