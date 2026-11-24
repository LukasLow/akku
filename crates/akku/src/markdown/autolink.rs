//! GFM extended autolinks: `https://…`, `www.…` and e-mail addresses.
//!
//! The pass runs over finished text nodes. Scanning only moves forward; after
//! a rejected candidate it skips to the end of the surrounding token, so a
//! hostile input such as `www.www.www.…_` cannot trigger quadratic rescans.

use super::ast::Inline;

/// Apply the autolink pass to a finished inline list, recursing into
/// emphasis-like wrappers but never into links or images.
pub(crate) fn apply(inlines: &mut Vec<Inline>) {
    let mut out: Vec<Inline> = Vec::with_capacity(inlines.len());
    for mut inline in std::mem::take(inlines) {
        match &mut inline {
            Inline::Text(text) => {
                if let Some(parts) = split_text(text) {
                    out.extend(parts);
                    continue;
                }
            }
            Inline::Emphasis(children)
            | Inline::Strong(children)
            | Inline::Strikethrough(children) => apply(children),
            _ => {}
        }
        out.push(inline);
    }
    *inlines = out;
}

fn is_boundary(prev: Option<char>) -> bool {
    match prev {
        None => true,
        Some(c) => c.is_whitespace() || matches!(c, '*' | '_' | '~' | '('),
    }
}

fn starts_with_ignore_case(bytes: &[u8], prefix: &str) -> bool {
    bytes.len() >= prefix.len() && bytes[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
}

/// Whether `byte` ends an extended autolink (whitespace or `<`).
fn ends_link(byte: u8) -> bool {
    byte.is_ascii_whitespace() || byte == b'<'
}

/// The first byte at or after `from` that ends a link (whitespace or `<`),
/// or the end of the text. The result may equal `from`.
fn link_end(bytes: &[u8], from: usize) -> usize {
    let mut i = from;
    while i < bytes.len() && !ends_link(bytes[i]) {
        i += 1;
    }
    i
}

/// Like [`link_end`] but always at least one byte past `from` (so a scan
/// that skips a rejected token makes progress).
fn token_end(bytes: &[u8], from: usize) -> usize {
    link_end(bytes, from).max(from + 1).min(bytes.len())
}

/// Validate a domain: at least one dot, no empty segment, and no underscore in
/// the last two segments.
fn valid_domain(domain: &str) -> bool {
    let segments: Vec<&str> = domain.split('.').collect();
    if segments.len() < 2 || segments.iter().any(|segment| segment.is_empty()) {
        return false;
    }
    segments
        .iter()
        .rev()
        .take(2)
        .all(|segment| !segment.contains('_'))
}

/// Trim trailing punctuation from a candidate link according to GFM.
fn trim_trailing(text: &str) -> &str {
    let mut end = text.len();
    // Counted once: trimming only ever removes `)` among the parentheses
    // (entities and other punctuation contain none), so the counts are
    // updated incrementally instead of rescanning per trimmed character.
    let mut parens: Option<(usize, usize)> = None;
    loop {
        let current = &text[..end];
        let Some(last) = current.chars().next_back() else {
            return current;
        };
        match last {
            '?' | '!' | '.' | ',' | ':' | '*' | '_' | '~' | '\'' | '"' => end -= 1,
            ')' => {
                let (opens, closes) = parens.get_or_insert_with(|| {
                    (current.matches('(').count(), current.matches(')').count())
                });
                if *closes > *opens {
                    *closes -= 1;
                    end -= 1;
                } else {
                    return current;
                }
            }
            ';' => {
                // `&entity;` is not part of the link.
                let before = &current[..current.len() - 1];
                let name = before
                    .rsplit(|c: char| !c.is_ascii_alphanumeric())
                    .next()
                    .unwrap_or("");
                let amp_at = before.len() - name.len();
                if !name.is_empty() && amp_at > 0 && before.as_bytes()[amp_at - 1] == b'&' {
                    end = amp_at - 1;
                } else {
                    return current;
                }
            }
            _ => return current,
        }
    }
}

/// Try a `www.` or `http(s)://` link starting at `start`.
fn scan_url(text: &str, start: usize) -> Option<(usize, String)> {
    let bytes = text.as_bytes();
    let rest = &bytes[start..];
    let (domain_start, prefix) = if starts_with_ignore_case(rest, "https://") {
        (start + 8, "")
    } else if starts_with_ignore_case(rest, "http://") {
        (start + 7, "")
    } else if starts_with_ignore_case(rest, "www.") {
        (start, "http://")
    } else {
        return None;
    };
    let mut domain_end = domain_start;
    while domain_end < bytes.len()
        && (bytes[domain_end].is_ascii_alphanumeric()
            || matches!(bytes[domain_end], b'-' | b'_' | b'.'))
    {
        domain_end += 1;
    }
    // The link ends at whitespace or `<`; a separator right after the
    // domain (`www.example.com foo`) is never part of the link.
    let end = link_end(bytes, domain_end);
    let candidate = trim_trailing(&text[start..end]);
    let domain_stop = (start + candidate.len()).min(domain_end);
    if domain_stop <= domain_start {
        return None;
    }
    let domain = text[domain_start..domain_stop].trim_end_matches('.');
    if !valid_domain(domain) {
        return None;
    }
    Some((start + candidate.len(), format!("{prefix}{candidate}")))
}

fn is_local_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b'+')
}

/// Try an e-mail address around the `@` at `at`; `floor` is the first byte
/// that may belong to the address. Returns `(start, end)`.
fn scan_email(text: &str, at: usize, floor: usize) -> Option<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut start = at;
    while start > floor && is_local_char(bytes[start - 1]) {
        start -= 1;
    }
    if start == at {
        return None;
    }
    let mut end = at + 1;
    while end < bytes.len()
        && (bytes[end].is_ascii_alphanumeric() || matches!(bytes[end], b'-' | b'_' | b'.'))
    {
        end += 1;
    }
    let domain = text[at + 1..end].trim_end_matches('.');
    let end = at + 1 + domain.len();
    let last = domain.chars().next_back()?;
    if !last.is_ascii_alphanumeric() || !valid_email_domain(domain) {
        return None;
    }
    Some((start, end))
}

fn valid_email_domain(domain: &str) -> bool {
    domain.contains('.') && domain.split('.').all(|segment| !segment.is_empty())
}

/// Split a text node around autolinks. `None` when it contains none.
fn split_text(text: &str) -> Option<Vec<Inline>> {
    let bytes = text.as_bytes();
    if !bytes
        .iter()
        .any(|&b| matches!(b, b'@' | b'w' | b'W' | b'h' | b'H'))
    {
        return None;
    }
    let mut parts: Vec<Inline> = Vec::new();
    let mut emitted = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        let byte = bytes[i];
        if matches!(byte, b'h' | b'H' | b'w' | b'W')
            && is_boundary(text[..i].chars().next_back())
            && text.is_char_boundary(i)
        {
            if let Some((end, destination)) = scan_url(text, i) {
                if emitted < i {
                    parts.push(Inline::Text(text[emitted..i].to_owned()));
                }
                parts.push(Inline::Link {
                    destination,
                    title: None,
                    content: vec![Inline::Text(text[i..end].to_owned())],
                });
                emitted = end;
                i = end;
                continue;
            }
            if starts_with_ignore_case(&bytes[i..], "http")
                || starts_with_ignore_case(&bytes[i..], "www.")
            {
                // Rejected candidate: skip the rest of this token.
                i = token_end(bytes, i);
                continue;
            }
        } else if byte == b'@' {
            if let Some((start, end)) = scan_email(text, i, emitted) {
                if emitted < start {
                    parts.push(Inline::Text(text[emitted..start].to_owned()));
                }
                let address = &text[start..end];
                parts.push(Inline::Link {
                    destination: format!("mailto:{address}"),
                    title: None,
                    content: vec![Inline::Text(address.to_owned())],
                });
                emitted = end;
                i = end;
                continue;
            }
        }
        i += 1;
    }
    if parts.is_empty() {
        return None;
    }
    if emitted < text.len() {
        parts.push(Inline::Text(text[emitted..].to_owned()));
    }
    Some(parts)
}
