//! URL policy and normalization.

use super::options::Options;

/// Whether `url` may be used as a link or image destination.
///
/// The check is made on the *decoded* destination. ASCII and Unicode control
/// characters and whitespace are ignored before the scheme is read, because
/// browsers ignore tabs and newlines inside a scheme (`java\tscript:`).
///
/// A URL without a scheme (relative, `#fragment`, `?query`, `//host`) is
/// allowed. Otherwise the scheme must be in
/// [`Options::allowed_schemes`], and `javascript`, `vbscript` and `data` are
/// refused no matter what.
///
/// ```
/// use akku::markdown::{is_safe_url, Options};
///
/// let options = Options::default();
/// assert!(is_safe_url("https://example.com/a", &options));
/// assert!(is_safe_url("/relative/path", &options));
/// assert!(!is_safe_url("JaVaScRiPt:alert(1)", &options));
/// assert!(!is_safe_url("java\tscript:alert(1)", &options));
/// assert!(!is_safe_url("data:text/html;base64,AAAA", &options));
/// ```
pub fn is_safe_url(url: &str, options: &Options) -> bool {
    const MAX_SCHEME: usize = 64;
    let mut scheme = String::new();
    let mut overflow = false;
    let mut has_colon = false;
    for c in url.chars() {
        if c.is_control() || c.is_whitespace() {
            continue;
        }
        match c {
            ':' => {
                has_colon = true;
                break;
            }
            '/' | '?' | '#' => break,
            _ => {
                if scheme.len() < MAX_SCHEME {
                    scheme.push(c.to_ascii_lowercase());
                } else {
                    overflow = true;
                }
            }
        }
    }
    if !has_colon {
        return true;
    }
    if overflow || matches!(scheme.as_str(), "javascript" | "vbscript" | "data") {
        return false;
    }
    options
        .allowed_schemes
        .iter()
        .any(|allowed| allowed.eq_ignore_ascii_case(&scheme))
}

/// Percent-encode a destination for use in an `href` or `src` attribute.
///
/// Existing `%XX` escapes are kept, a stray `%` becomes `%25`, and every byte
/// outside the URL-safe set is encoded. The caller still HTML-escapes the
/// result.
pub(crate) fn normalize_href(url: &str) -> String {
    const SAFE: &[u8] = b"-_.~!*'();:@&=+$,/?#[]";
    let bytes = url.as_bytes();
    let mut out = String::with_capacity(url.len());
    for (i, &byte) in bytes.iter().enumerate() {
        let hex_pair = byte == b'%'
            && bytes.get(i + 1).is_some_and(u8::is_ascii_hexdigit)
            && bytes.get(i + 2).is_some_and(u8::is_ascii_hexdigit);
        if byte.is_ascii_alphanumeric() || SAFE.contains(&byte) || hex_pair {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}
