//! Byte-level scanners shared by the block and inline parsers.
//!
//! Every scanner is bounded by the token it recognizes and never backtracks
//! over text it has already rejected, which keeps hostile input linear.

/// CommonMark whitespace (space, tab, line ending, form feed, vertical tab).
pub(crate) fn is_ws(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

fn is_attr_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_' || byte == b':'
}

fn is_attr_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b':' | b'-')
}

/// Scan an HTML open tag starting at `start` (which must be `<`).
///
/// Returns the index just after the closing `>`.
pub(crate) fn scan_open_tag(bytes: &[u8], start: usize) -> Option<usize> {
    if bytes.get(start) != Some(&b'<') {
        return None;
    }
    let mut i = start + 1;
    if !bytes.get(i)?.is_ascii_alphabetic() {
        return None;
    }
    i += 1;
    while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'-') {
        i += 1;
    }
    loop {
        let before_ws = i;
        while i < bytes.len() && is_ws(bytes[i]) {
            i += 1;
        }
        let byte = *bytes.get(i)?;
        if byte == b'>' {
            return Some(i + 1);
        }
        if byte == b'/' {
            return if bytes.get(i + 1) == Some(&b'>') {
                Some(i + 2)
            } else {
                None
            };
        }
        if i == before_ws || !is_attr_start(byte) {
            return None;
        }
        i += 1;
        while i < bytes.len() && is_attr_char(bytes[i]) {
            i += 1;
        }
        // Optional `= value`.
        let mut j = i;
        while j < bytes.len() && is_ws(bytes[j]) {
            j += 1;
        }
        if bytes.get(j) == Some(&b'=') {
            j += 1;
            while j < bytes.len() && is_ws(bytes[j]) {
                j += 1;
            }
            match *bytes.get(j)? {
                quote @ (b'"' | b'\'') => {
                    let close = bytes[j + 1..].iter().position(|&b| b == quote)?;
                    i = j + 1 + close + 1;
                }
                _ => {
                    let value_start = j;
                    while j < bytes.len()
                        && !is_ws(bytes[j])
                        && !matches!(bytes[j], b'"' | b'\'' | b'=' | b'<' | b'>' | b'`')
                    {
                        j += 1;
                    }
                    if j == value_start {
                        return None;
                    }
                    i = j;
                }
            }
        }
    }
}

/// Scan an HTML closing tag starting at `start` (which must be `</`).
pub(crate) fn scan_close_tag(bytes: &[u8], start: usize) -> Option<usize> {
    if bytes.get(start) != Some(&b'<') || bytes.get(start + 1) != Some(&b'/') {
        return None;
    }
    let mut i = start + 2;
    if !bytes.get(i)?.is_ascii_alphabetic() {
        return None;
    }
    i += 1;
    while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'-') {
        i += 1;
    }
    while i < bytes.len() && is_ws(bytes[i]) {
        i += 1;
    }
    if bytes.get(i) == Some(&b'>') {
        Some(i + 1)
    } else {
        None
    }
}

/// The result of an angle-bracket autolink: `(destination, text, end)`.
pub(crate) type Autolink = (String, String, usize);

/// Scan `<scheme:address>` or `<user@host>` starting at `start` (a `<`).
pub(crate) fn scan_autolink(src: &str, start: usize) -> Option<Autolink> {
    let bytes = src.as_bytes();
    let mut i = start + 1;
    // URI autolink.
    if bytes.get(i)?.is_ascii_alphabetic() {
        let scheme_start = i;
        i += 1;
        while i < bytes.len()
            && (bytes[i].is_ascii_alphanumeric() || matches!(bytes[i], b'+' | b'.' | b'-'))
        {
            i += 1;
        }
        let scheme_len = i - scheme_start;
        if (2..=32).contains(&scheme_len) && bytes.get(i) == Some(&b':') {
            let mut j = i + 1;
            while j < bytes.len() {
                let byte = bytes[j];
                if byte == b'>' {
                    let text = &src[start + 1..j];
                    return Some((text.to_owned(), text.to_owned(), j + 1));
                }
                if byte <= b' ' || byte == 0x7f || byte == b'<' {
                    break;
                }
                j += 1;
            }
        }
    }
    // E-mail autolink.
    let local_start = start + 1;
    let mut i = local_start;
    while i < bytes.len()
        && (bytes[i].is_ascii_alphanumeric() || b".!#$%&'*+/=?^_`{|}~-".contains(&bytes[i]))
    {
        i += 1;
    }
    if i == local_start || bytes.get(i) != Some(&b'@') {
        return None;
    }
    let domain_start = i + 1;
    let mut j = domain_start;
    let mut label_len = 0usize;
    let mut last_alnum = false;
    let mut first = true;
    loop {
        match bytes.get(j) {
            Some(b'>') => break,
            Some(b'.') => {
                if first || !last_alnum {
                    return None;
                }
                label_len = 0;
                last_alnum = false;
                first = true;
            }
            Some(&byte) if byte.is_ascii_alphanumeric() => {
                label_len += 1;
                last_alnum = true;
                first = false;
            }
            Some(b'-') => {
                if first {
                    return None;
                }
                label_len += 1;
                last_alnum = false;
            }
            _ => return None,
        }
        if label_len > 63 {
            return None;
        }
        j += 1;
    }
    if !last_alnum || j == domain_start {
        return None;
    }
    let text = &src[local_start..j];
    Some((format!("mailto:{text}"), text.to_owned(), j + 1))
}
