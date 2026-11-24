//! Percent-encoding and decoding for URLs and form data.

use std::borrow::Cow;
use std::error::Error;
use std::fmt;

/// A bitmask of ASCII bytes (`0..128`) that should be percent-encoded.
///
/// Non-ASCII bytes (`>= 128`) are always percent-encoded by [`percent_encode`]
/// and [`percent_encode_bytes`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AsciiSet(u128);

impl AsciiSet {
    /// Creates an empty [`AsciiSet`] containing no ASCII bytes.
    pub const fn new() -> Self {
        Self(0)
    }

    /// Returns a new [`AsciiSet`] with `byte` added.
    ///
    /// Non-ASCII bytes (`>= 128`) are ignored because they are always
    /// percent-encoded.
    pub const fn add(self, byte: u8) -> Self {
        if byte < 128 {
            Self(self.0 | (1u128 << byte))
        } else {
            self
        }
    }

    /// Returns a new [`AsciiSet`] with `byte` removed.
    pub const fn remove(self, byte: u8) -> Self {
        if byte < 128 {
            Self(self.0 & !(1u128 << byte))
        } else {
            self
        }
    }

    /// Returns `true` if `byte` is an ASCII byte present in this set.
    pub const fn contains(&self, byte: u8) -> bool {
        if byte < 128 {
            (self.0 & (1u128 << byte)) != 0
        } else {
            false
        }
    }

    const fn add_range(mut self, start: u8, end_inclusive: u8) -> Self {
        let mut b = start;
        while b <= end_inclusive {
            self = self.add(b);
            if b == u8::MAX {
                break;
            }
            b += 1;
        }
        self
    }

    const fn remove_range(mut self, start: u8, end_inclusive: u8) -> Self {
        let mut b = start;
        while b <= end_inclusive {
            self = self.remove(b);
            if b == u8::MAX {
                break;
            }
            b += 1;
        }
        self
    }
}

impl Default for AsciiSet {
    fn default() -> Self {
        Self::new()
    }
}

const CONTROLS: AsciiSet = AsciiSet::new().add_range(0x00, 0x1F).add(0x7F);

/// Every ASCII byte except `A-Z`, `a-z`, and `0-9`.
pub const NON_ALPHANUMERIC: AsciiSet = AsciiSet::new()
    .add_range(0x00, 0x7F)
    .remove_range(b'0', b'9')
    .remove_range(b'A', b'Z')
    .remove_range(b'a', b'z');

/// The WHATWG fragment percent-encode set (`CONTROLS`, space, `"`, `<`, `>`, `` ` ``).
pub const FRAGMENT: AsciiSet = CONTROLS.add(b' ').add(b'"').add(b'<').add(b'>').add(b'`');

/// The WHATWG query percent-encode set (`CONTROLS`, space, `"`, `#`, `<`, `>`).
pub const QUERY: AsciiSet = CONTROLS.add(b' ').add(b'"').add(b'#').add(b'<').add(b'>');

/// The WHATWG path percent-encode set (`FRAGMENT`, `#`, `?`, `{`, `}`).
pub const PATH: AsciiSet = FRAGMENT.add(b'#').add(b'?').add(b'{').add(b'}');

/// The percent-encode set for individual path segments (`PATH`, `/`, `%`).
pub const PATH_SEGMENT: AsciiSet = PATH.add(b'/').add(b'%');

/// The WHATWG userinfo percent-encode set.
pub const USERINFO: AsciiSet = PATH
    .add(b'/')
    .add(b':')
    .add(b';')
    .add(b'=')
    .add(b'@')
    .add(b'[')
    .add(b'\\')
    .add(b']')
    .add(b'^')
    .add(b'|');

/// The percent-encode set for URI components (`encodeURIComponent`-compatible).
pub const COMPONENT: AsciiSet = USERINFO.add(b'$').add(b'%').add(b'&').add(b'+').add(b',');

/// The `application/x-www-form-urlencoded` percent-encode set.
///
/// Leaves only `A-Z`, `a-z`, `0-9`, `*`, `-`, `.`, and `_` unencoded.
pub const FORM: AsciiSet = NON_ALPHANUMERIC
    .remove(b'*')
    .remove(b'-')
    .remove(b'.')
    .remove(b'_');

const HEX_UPPER: &[u8; 16] = b"0123456789ABCDEF";

/// Error returned by [`percent_decode`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PercentDecodeError {
    /// A `%` character was not followed by two hexadecimal digits.
    InvalidEscape {
        /// The byte index of the invalid `%` in the input string.
        index: usize,
    },
    /// The percent-decoded bytes are not valid UTF-8.
    InvalidUtf8 {
        /// The byte offset in the decoded buffer up to which the data is valid UTF-8.
        valid_up_to: usize,
    },
}

impl fmt::Display for PercentDecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::InvalidEscape { index } => {
                write!(f, "invalid percent-escape at byte index {index}")
            }
            Self::InvalidUtf8 { valid_up_to } => {
                write!(
                    f,
                    "decoded bytes are not valid UTF-8 (valid up to byte {valid_up_to})"
                )
            }
        }
    }
}

impl Error for PercentDecodeError {}

#[inline]
pub(crate) fn should_encode(byte: u8, ascii_set: &AsciiSet) -> bool {
    byte >= 128 || ascii_set.contains(byte)
}

#[inline]
pub(crate) fn push_percent_encoded(out: &mut String, byte: u8) {
    out.push('%');
    out.push(HEX_UPPER[(byte >> 4) as usize] as char);
    out.push(HEX_UPPER[(byte & 0x0F) as usize] as char);
}

/// Percent-encodes UTF-8 text using `ascii_set` with uppercase hexadecimal digits.
///
/// Returns [`Cow::Borrowed`] without allocating if no byte needs encoding.
///
/// # Examples
///
/// ```
/// use std::borrow::Cow;
/// use akku::url::{percent_encode, COMPONENT};
///
/// assert_eq!(percent_encode("hello", &COMPONENT), Cow::Borrowed("hello"));
/// assert_eq!(percent_encode("a/b c?", &COMPONENT), "a%2Fb%20c%3F");
/// ```
pub fn percent_encode<'a>(input: &'a str, ascii_set: &AsciiSet) -> Cow<'a, str> {
    let bytes = input.as_bytes();
    let Some(first) = bytes.iter().position(|&b| should_encode(b, ascii_set)) else {
        return Cow::Borrowed(input);
    };

    let mut out = String::with_capacity(bytes.len() + 8);
    out.push_str(&input[..first]);
    for &b in &bytes[first..] {
        if should_encode(b, ascii_set) {
            push_percent_encoded(&mut out, b);
        } else {
            out.push(b as char);
        }
    }
    Cow::Owned(out)
}

/// Percent-encodes raw bytes using `ascii_set` with uppercase hexadecimal digits.
///
/// # Examples
///
/// ```
/// use akku::url::{percent_encode_bytes, NON_ALPHANUMERIC};
///
/// assert_eq!(percent_encode_bytes(&[0xFF, b'a', b' '], &NON_ALPHANUMERIC), "%FFa%20");
/// ```
pub fn percent_encode_bytes(input: &[u8], ascii_set: &AsciiSet) -> String {
    let mut out = String::with_capacity(input.len());
    for &b in input {
        if should_encode(b, ascii_set) {
            push_percent_encoded(&mut out, b);
        } else {
            out.push(b as char);
        }
    }
    out
}

#[inline]
pub(crate) fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Strictly percent-decodes a string into UTF-8 text.
///
/// Leaves `+` literal (`+` is not converted to a space). Returns
/// [`PercentDecodeError::InvalidEscape`] on malformed `%` sequences and
/// [`PercentDecodeError::InvalidUtf8`] if the decoded bytes are not valid UTF-8.
///
/// Returns [`Cow::Borrowed`] without allocating if `input` contains no `%`.
///
/// # Examples
///
/// ```
/// use akku::url::percent_decode;
///
/// assert_eq!(percent_decode("hello%20world+1").unwrap(), "hello world+1");
/// assert!(percent_decode("100%").is_err());
/// ```
pub fn percent_decode(input: &str) -> Result<Cow<'_, str>, PercentDecodeError> {
    let bytes = input.as_bytes();
    let Some(first) = bytes.iter().position(|&b| b == b'%') else {
        return Ok(Cow::Borrowed(input));
    };

    let mut out = Vec::with_capacity(bytes.len());
    out.extend_from_slice(&bytes[..first]);

    let mut i = first;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return Err(PercentDecodeError::InvalidEscape { index: i });
            }
            let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) else {
                return Err(PercentDecodeError::InvalidEscape { index: i });
            };
            out.push((hi << 4) | lo);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }

    match String::from_utf8(out) {
        Ok(s) => Ok(Cow::Owned(s)),
        Err(err) => Err(PercentDecodeError::InvalidUtf8 {
            valid_up_to: err.utf8_error().valid_up_to(),
        }),
    }
}

/// Leniently percent-decodes a string into raw bytes.
///
/// Valid `%XX` sequences are decoded; malformed `%` sequences and `+` are kept
/// literally. Returns [`Cow::Borrowed`] if no `%XX` sequence was decoded.
///
/// # Examples
///
/// ```
/// use akku::url::percent_decode_bytes;
///
/// assert_eq!(percent_decode_bytes("a%20b%ZZ").as_ref(), b"a b%ZZ");
/// ```
pub fn percent_decode_bytes(input: &str) -> Cow<'_, [u8]> {
    let bytes = input.as_bytes();
    let mut i = 0;
    let mut first_valid = None;
    while i + 2 < bytes.len() {
        if bytes[i] == b'%' && hex_val(bytes[i + 1]).is_some() && hex_val(bytes[i + 2]).is_some() {
            first_valid = Some(i);
            break;
        }
        i += 1;
    }

    let Some(start) = first_valid else {
        return Cow::Borrowed(bytes);
    };

    let mut out = Vec::with_capacity(bytes.len());
    out.extend_from_slice(&bytes[..start]);

    let mut idx = start;
    while idx < bytes.len() {
        if bytes[idx] == b'%' && idx + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_val(bytes[idx + 1]), hex_val(bytes[idx + 2])) {
                out.push((hi << 4) | lo);
                idx += 3;
                continue;
            }
        }
        out.push(bytes[idx]);
        idx += 1;
    }

    Cow::Owned(out)
}
