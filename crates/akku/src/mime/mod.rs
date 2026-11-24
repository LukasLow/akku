//! RFC 2045 / 6838 MIME media types, parsing, matching, and extension mapping.
//!
//! # Examples
//!
//! ```
//! use akku::mime::{self, Mime};
//!
//! let ct = Mime::parse("Application/VND.api+json; Charset=\"UTF-8\"").unwrap();
//! assert_eq!(ct.type_(), "application");
//! assert_eq!(ct.subtype(), "vnd.api+json");
//! assert_eq!(ct.suffix(), Some("json"));
//! assert_eq!(ct.charset(), Some("utf-8"));
//! assert!(ct.is_json());
//!
//! assert_eq!(mime::from_extension("svg"), Some(mime::IMAGE_SVG_XML));
//! ```

use std::borrow::Cow;
use std::error::Error;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::str::FromStr;

/// Error returned when parsing or constructing a [`Mime`] media type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ParseError {
    /// Input was empty or contained only whitespace.
    Empty,
    /// Input did not contain a `/` separator between type and subtype.
    MissingSlash,
    /// The media type portion was empty or contained invalid token characters.
    InvalidType,
    /// The media subtype portion was empty or contained invalid token characters.
    InvalidSubtype,
    /// A parameter name was empty or contained invalid token characters.
    InvalidParamName,
    /// A parameter value contained invalid characters or trailing garbage after quotes.
    InvalidParamValue,
    /// A parameter was missing `=value` or had an empty unquoted value.
    MissingParamValue,
    /// A quoted-string parameter value was not closed with `"`.
    UnterminatedQuote,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Empty => f.write_str("empty MIME type"),
            Self::MissingSlash => f.write_str("missing '/' in MIME type"),
            Self::InvalidType => f.write_str("invalid MIME type"),
            Self::InvalidSubtype => f.write_str("invalid MIME subtype"),
            Self::InvalidParamName => f.write_str("invalid MIME parameter name"),
            Self::InvalidParamValue => f.write_str("invalid MIME parameter value"),
            Self::MissingParamValue => f.write_str("missing MIME parameter value"),
            Self::UnterminatedQuote => f.write_str("unterminated quoted string in MIME parameter"),
        }
    }
}

impl Error for ParseError {}

#[derive(Debug, Clone)]
enum Params {
    Static(&'static [(&'static str, &'static str)]),
    Owned(Vec<(String, String)>),
}

/// A parsed MIME media type (`type/subtype` with optional parameters).
///
/// Supports `const` static instances (such as [`TEXT_PLAIN_UTF8`]) as well as
/// dynamically parsed or modified media types.
#[derive(Debug, Clone)]
pub struct Mime {
    essence: Cow<'static, str>,
    slash: usize,
    plus: Option<usize>,
    params: Params,
}

const fn is_token_byte(b: u8) -> bool {
    matches!(
        b,
        b'!' | b'#'
            | b'$'
            | b'%'
            | b'&'
            | b'\''
            | b'*'
            | b'+'
            | b'-'
            | b'.'
            | b'^'
            | b'_'
            | b'`'
            | b'|'
            | b'~'
            | b'0'..=b'9'
            | b'A'..=b'Z'
            | b'a'..=b'z'
    )
}

fn is_token(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(is_token_byte)
}

#[inline]
fn is_ows(b: u8) -> bool {
    b == b' ' || b == b'\t'
}

fn trim_ows(s: &str) -> &str {
    let bytes = s.as_bytes();
    let mut start = 0;
    while start < bytes.len() && is_ows(bytes[start]) {
        start += 1;
    }
    let mut end = bytes.len();
    while end > start && is_ows(bytes[end - 1]) {
        end -= 1;
    }
    &s[start..end]
}

fn is_valid_qdtext_char(ch: char) -> bool {
    let code = ch as u32;
    code == 0x09 || (code >= 0x20 && code != 0x7F)
}

const fn find_slash_and_plus(essence: &'static str) -> (usize, Option<usize>) {
    let bytes = essence.as_bytes();
    let mut i = 0;
    let mut slash = 0;
    let mut plus = None;
    while i < bytes.len() {
        if bytes[i] == b'/' && slash == 0 {
            slash = i;
        } else if bytes[i] == b'+' && slash > 0 && i + 1 < bytes.len() {
            plus = Some(i);
        }
        i += 1;
    }
    (slash, plus)
}

impl Mime {
    const fn from_static(
        essence: &'static str,
        params: &'static [(&'static str, &'static str)],
    ) -> Self {
        let (slash, plus) = find_slash_and_plus(essence);
        Self {
            essence: Cow::Borrowed(essence),
            slash,
            plus,
            params: Params::Static(params),
        }
    }

    /// Parses a MIME media type string (e.g., `"text/html; charset=utf-8"`).
    ///
    /// Type, subtype, parameter names, and `charset` values are normalized to
    /// ASCII lowercase. Quoted-string parameter values are unescaped. Duplicate
    /// parameter names follow first-wins semantics.
    pub fn parse(input: &str) -> Result<Self, ParseError> {
        let trimmed = trim_ows(input);
        if trimmed.is_empty() {
            return Err(ParseError::Empty);
        }

        let (media_part, params_part) = match trimmed.split_once(';') {
            Some((m, p)) => (trim_ows(m), Some(p)),
            None => (trimmed, None),
        };

        let Some((raw_type, raw_subtype)) = media_part.split_once('/') else {
            return Err(ParseError::MissingSlash);
        };

        if !is_token(raw_type) {
            return Err(ParseError::InvalidType);
        }
        if !is_token(raw_subtype) {
            return Err(ParseError::InvalidSubtype);
        }

        let lower_type = raw_type.to_ascii_lowercase();
        let lower_subtype = raw_subtype.to_ascii_lowercase();
        let slash = lower_type.len();
        let mut essence = String::with_capacity(lower_type.len() + 1 + lower_subtype.len());
        essence.push_str(&lower_type);
        essence.push('/');
        essence.push_str(&lower_subtype);

        let plus = lower_subtype
            .rfind('+')
            .filter(|&idx| idx + 1 < lower_subtype.len())
            .map(|idx| slash + 1 + idx);

        let params = if let Some(raw_params) = params_part {
            let parsed = parse_params(raw_params)?;
            if parsed.is_empty() {
                Params::Static(&[])
            } else {
                Params::Owned(parsed)
            }
        } else {
            Params::Static(&[])
        };

        Ok(Self {
            essence: Cow::Owned(essence),
            slash,
            plus,
            params,
        })
    }

    /// Constructs a [`Mime`] from explicit `type_` and `subtype` tokens without parameters.
    pub fn new(type_: &str, subtype: &str) -> Result<Self, ParseError> {
        if !is_token(type_) {
            return Err(ParseError::InvalidType);
        }
        if !is_token(subtype) {
            return Err(ParseError::InvalidSubtype);
        }
        let lower_type = type_.to_ascii_lowercase();
        let lower_subtype = subtype.to_ascii_lowercase();
        let slash = lower_type.len();
        let mut essence = String::with_capacity(lower_type.len() + 1 + lower_subtype.len());
        essence.push_str(&lower_type);
        essence.push('/');
        essence.push_str(&lower_subtype);
        let plus = lower_subtype
            .rfind('+')
            .filter(|&idx| idx + 1 < lower_subtype.len())
            .map(|idx| slash + 1 + idx);

        Ok(Self {
            essence: Cow::Owned(essence),
            slash,
            plus,
            params: Params::Static(&[]),
        })
    }

    /// Returns the primary type (the part before `/`), in lowercase.
    pub fn type_(&self) -> &str {
        &self.essence[..self.slash]
    }

    /// Returns the subtype (the part after `/`), in lowercase.
    pub fn subtype(&self) -> &str {
        &self.essence[self.slash + 1..]
    }

    /// Returns the structured syntax suffix after the last `+` in the subtype, if any.
    ///
    /// For example, `application/vnd.api+json` returns `Some("json")`.
    pub fn suffix(&self) -> Option<&str> {
        self.plus.map(|idx| &self.essence[idx + 1..])
    }

    /// Returns the `type/subtype` essence without parameters.
    pub fn essence(&self) -> &str {
        &self.essence
    }

    /// Returns an iterator over `(name, value)` parameter pairs.
    pub fn params(&self) -> impl Iterator<Item = (&str, &str)> {
        ParamIter {
            params: &self.params,
            index: 0,
        }
    }

    fn params_len(&self) -> usize {
        match &self.params {
            Params::Static(s) => s.len(),
            Params::Owned(v) => v.len(),
        }
    }

    /// Looks up a parameter value by ASCII case-insensitive `name`.
    pub fn param(&self, name: &str) -> Option<&str> {
        self.params()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v)
    }

    /// Returns the `charset` parameter value, if present.
    pub fn charset(&self) -> Option<&str> {
        self.param("charset")
    }

    /// Returns a new [`Mime`] with parameter `(name, value)` added or updated.
    ///
    /// # Errors
    ///
    /// Returns [`ParseError::InvalidParamName`] if `name` is not a valid MIME token,
    /// or [`ParseError::InvalidParamValue`] if `value` contains disallowed control characters.
    pub fn with_param(mut self, name: &str, value: &str) -> Result<Self, ParseError> {
        if !is_token(name) {
            return Err(ParseError::InvalidParamName);
        }
        if !value.chars().all(is_valid_qdtext_char) {
            return Err(ParseError::InvalidParamValue);
        }
        let lower_name = name.to_ascii_lowercase();
        let stored_val = if lower_name == "charset" {
            value.to_ascii_lowercase()
        } else {
            value.to_owned()
        };

        let vec = match &mut self.params {
            Params::Static(slice) => {
                let mut v: Vec<(String, String)> = slice
                    .iter()
                    .map(|&(k, val)| (k.to_owned(), val.to_owned()))
                    .collect();
                if let Some((_, existing)) = v.iter_mut().find(|(k, _)| k == &lower_name) {
                    *existing = stored_val;
                } else {
                    v.push((lower_name, stored_val));
                }
                self.params = Params::Owned(v);
                return Ok(self);
            }
            Params::Owned(v) => v,
        };

        if let Some((_, existing)) = vec.iter_mut().find(|(k, _)| k == &lower_name) {
            *existing = stored_val;
        } else {
            vec.push((lower_name, stored_val));
        }

        Ok(self)
    }

    /// Returns a copy of this [`Mime`] with all parameters stripped.
    pub fn without_params(&self) -> Self {
        Self {
            essence: self.essence.clone(),
            slash: self.slash,
            plus: self.plus,
            params: Params::Static(&[]),
        }
    }

    /// Returns `true` if `self` and `other` have the same `type/subtype` essence,
    /// ignoring parameters.
    pub fn essence_eq(&self, other: &Self) -> bool {
        self.essence == other.essence
    }

    /// Returns `true` if this media type matches `(type_, subtype)` (case-insensitive),
    /// supporting `"*"` wildcards on either part.
    pub fn matches(&self, type_: &str, subtype: &str) -> bool {
        let type_ok = type_ == "*" || self.type_().eq_ignore_ascii_case(type_);
        let sub_ok = subtype == "*" || self.subtype().eq_ignore_ascii_case(subtype);
        type_ok && sub_ok
    }

    /// Returns `true` if the primary type is `"text"`.
    pub fn is_text(&self) -> bool {
        self.type_() == "text"
    }

    /// Returns `true` if this media type is JSON (`application/json`, `text/json`, or `*/*+json`).
    pub fn is_json(&self) -> bool {
        matches!(self.essence.as_ref(), "application/json" | "text/json")
            || self.suffix() == Some("json")
    }

    /// Returns `true` if this media type is XML (`application/xml`, `text/xml`, or `*/*+xml`).
    pub fn is_xml(&self) -> bool {
        matches!(self.essence.as_ref(), "application/xml" | "text/xml")
            || self.suffix() == Some("xml")
    }

    /// Returns the canonical file extension (without leading `.`) for this media type's essence.
    pub fn extension(&self) -> Option<&'static str> {
        match self.essence.as_ref() {
            "text/plain" => Some("txt"),
            "text/html" => Some("html"),
            "text/css" => Some("css"),
            "text/javascript" | "application/javascript" => Some("js"),
            "text/csv" => Some("csv"),
            "text/xml" | "application/xml" => Some("xml"),
            "text/markdown" => Some("md"),
            "text/calendar" => Some("ics"),
            "text/tab-separated-values" => Some("tsv"),
            "application/json" => Some("json"),
            "application/ld+json" => Some("jsonld"),
            "application/pdf" => Some("pdf"),
            "application/zip" => Some("zip"),
            "application/gzip" => Some("gz"),
            "application/x-tar" => Some("tar"),
            "application/x-7z-compressed" => Some("7z"),
            "application/vnd.rar" => Some("rar"),
            "application/x-bzip2" => Some("bz2"),
            "application/x-xz" => Some("xz"),
            "application/zstd" => Some("zst"),
            "application/wasm" => Some("wasm"),
            "application/toml" => Some("toml"),
            "application/yaml" => Some("yaml"),
            "application/rtf" => Some("rtf"),
            "application/xhtml+xml" => Some("xhtml"),
            "application/octet-stream" => Some("bin"),
            "image/png" => Some("png"),
            "image/jpeg" => Some("jpg"),
            "image/gif" => Some("gif"),
            "image/webp" => Some("webp"),
            "image/avif" => Some("avif"),
            "image/svg+xml" => Some("svg"),
            "image/x-icon" | "image/vnd.microsoft.icon" => Some("ico"),
            "image/bmp" => Some("bmp"),
            "image/tiff" => Some("tiff"),
            "audio/mpeg" => Some("mp3"),
            "audio/wav" => Some("wav"),
            "audio/ogg" => Some("ogg"),
            "audio/flac" => Some("flac"),
            "audio/aac" => Some("aac"),
            "audio/mp4" => Some("m4a"),
            "audio/opus" => Some("opus"),
            "video/mp4" => Some("mp4"),
            "video/webm" => Some("webm"),
            "video/ogg" => Some("ogv"),
            "video/x-msvideo" => Some("avi"),
            "video/quicktime" => Some("mov"),
            "video/x-matroska" => Some("mkv"),
            "font/woff" => Some("woff"),
            "font/woff2" => Some("woff2"),
            "font/ttf" => Some("ttf"),
            "font/otf" => Some("otf"),
            "application/vnd.ms-fontobject" => Some("eot"),
            _ => None,
        }
    }
}

struct ParamIter<'a> {
    params: &'a Params,
    index: usize,
}

impl<'a> Iterator for ParamIter<'a> {
    type Item = (&'a str, &'a str);

    fn next(&mut self) -> Option<Self::Item> {
        let item = match self.params {
            Params::Static(slice) => slice.get(self.index).map(|&(k, v)| (k, v)),
            Params::Owned(vec) => vec.get(self.index).map(|(k, v)| (k.as_str(), v.as_str())),
        };
        if item.is_some() {
            self.index += 1;
        }
        item
    }
}

fn parse_params(input: &str) -> Result<Vec<(String, String)>, ParseError> {
    let mut params: Vec<(String, String)> = Vec::new();
    let bytes = input.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        while i < bytes.len() && is_ows(bytes[i]) {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        if bytes[i] == b';' {
            i += 1;
            continue;
        }

        let name_start = i;
        while i < bytes.len() && bytes[i] != b'=' && bytes[i] != b';' {
            i += 1;
        }
        let raw_name = trim_ows(&input[name_start..i]);
        if i >= bytes.len() || bytes[i] == b';' {
            if !is_token(raw_name) {
                return Err(ParseError::InvalidParamName);
            }
            return Err(ParseError::MissingParamValue);
        }

        // bytes[i] == b'='
        if !is_token(raw_name) {
            return Err(ParseError::InvalidParamName);
        }
        i += 1; // skip '='

        while i < bytes.len() && is_ows(bytes[i]) {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] == b';' {
            return Err(ParseError::MissingParamValue);
        }

        let value = if bytes[i] == b'"' {
            i += 1; // skip opening '"'
            let mut decoded = String::new();
            let mut closed = false;
            while i < bytes.len() {
                let ch = input[i..].chars().next().expect("valid UTF-8 slice");
                i += ch.len_utf8();
                if ch == '"' {
                    closed = true;
                    break;
                } else if ch == '\\' {
                    let Some(escaped) = input[i..].chars().next() else {
                        return Err(ParseError::UnterminatedQuote);
                    };
                    i += escaped.len_utf8();
                    if !is_valid_qdtext_char(escaped) {
                        return Err(ParseError::InvalidParamValue);
                    }
                    decoded.push(escaped);
                } else if is_valid_qdtext_char(ch) {
                    decoded.push(ch);
                } else {
                    return Err(ParseError::InvalidParamValue);
                }
            }
            if !closed {
                return Err(ParseError::UnterminatedQuote);
            }
            while i < bytes.len() && is_ows(bytes[i]) {
                i += 1;
            }
            if i < bytes.len() {
                if bytes[i] == b';' {
                    i += 1;
                } else {
                    return Err(ParseError::InvalidParamValue);
                }
            }
            decoded
        } else {
            let val_start = i;
            while i < bytes.len() && bytes[i] != b';' {
                i += 1;
            }
            let raw_val = trim_ows(&input[val_start..i]);
            if i < bytes.len() && bytes[i] == b';' {
                i += 1;
            }
            if raw_val.is_empty() {
                return Err(ParseError::MissingParamValue);
            }
            if !is_token(raw_val) {
                return Err(ParseError::InvalidParamValue);
            }
            raw_val.to_owned()
        };

        let lower_name = raw_name.to_ascii_lowercase();
        let stored_value = if lower_name == "charset" {
            value.to_ascii_lowercase()
        } else {
            value
        };

        // Duplicate parameters follow first-wins semantics.
        if !params.iter().any(|(k, _)| k == &lower_name) {
            params.push((lower_name, stored_value));
        }
    }

    Ok(params)
}

impl fmt::Display for Mime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.essence)?;
        for (name, value) in self.params() {
            write!(f, "; {name}=")?;
            if is_token(value) {
                f.write_str(value)?;
            } else {
                f.write_str("\"")?;
                for ch in value.chars() {
                    if ch == '"' || ch == '\\' {
                        f.write_str("\\")?;
                    }
                    write!(f, "{ch}")?;
                }
                f.write_str("\"")?;
            }
        }
        Ok(())
    }
}

impl FromStr for Mime {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl PartialEq for Mime {
    fn eq(&self, other: &Self) -> bool {
        if self.essence != other.essence || self.params_len() != other.params_len() {
            return false;
        }
        self.params().all(|(k, v)| other.param(k) == Some(v))
    }
}

impl Eq for Mime {}

fn fnv1a_pair(k: &str, v: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in k.as_bytes() {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash ^= 0xFF;
    hash = hash.wrapping_mul(0x100000001b3);
    for &b in v.as_bytes() {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

impl Hash for Mime {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.essence.hash(state);
        self.params_len().hash(state);
        let mut combined: u64 = 0;
        for (k, v) in self.params() {
            combined ^= fnv1a_pair(k, v);
        }
        state.write_u64(combined);
    }
}

/// Looks up a [`Mime`] type from a file extension without heap allocation.
///
/// Accepts an optional leading `.` and matches ASCII case-insensitively.
/// Returns `None` for unknown, empty, or overlong (`> 16` bytes) extensions.
///
/// # Examples
///
/// ```
/// use akku::mime::{self, APPLICATION_JSON, IMAGE_PNG};
///
/// assert_eq!(mime::from_extension("json"), Some(APPLICATION_JSON));
/// assert_eq!(mime::from_extension(".PNG"), Some(IMAGE_PNG));
/// assert_eq!(mime::from_extension("unknown_ext"), None);
/// ```
pub fn from_extension(ext: &str) -> Option<Mime> {
    let ext = ext.strip_prefix('.').unwrap_or(ext);
    if ext.is_empty() || ext.len() > 16 {
        return None;
    }
    let mut buf = [0u8; 16];
    for (dst, &src) in buf.iter_mut().zip(ext.as_bytes()) {
        *dst = src.to_ascii_lowercase();
    }
    match &buf[..ext.len()] {
        b"txt" => Some(TEXT_PLAIN),
        b"html" | b"htm" => Some(TEXT_HTML),
        b"css" => Some(TEXT_CSS),
        b"js" | b"mjs" | b"cjs" => Some(TEXT_JAVASCRIPT),
        b"csv" => Some(TEXT_CSV),
        b"xml" => Some(TEXT_XML),
        b"md" | b"markdown" => Some(TEXT_MARKDOWN),
        b"ics" => Some(Mime::from_static("text/calendar", &[])),
        b"tsv" => Some(Mime::from_static("text/tab-separated-values", &[])),
        b"json" => Some(APPLICATION_JSON),
        b"jsonld" => Some(Mime::from_static("application/ld+json", &[])),
        b"pdf" => Some(APPLICATION_PDF),
        b"wasm" => Some(APPLICATION_WASM),
        b"toml" => Some(APPLICATION_TOML),
        b"yaml" | b"yml" => Some(APPLICATION_YAML),
        b"rtf" => Some(Mime::from_static("application/rtf", &[])),
        b"xhtml" => Some(Mime::from_static("application/xhtml+xml", &[])),
        b"bin" | b"exe" | b"dll" => Some(APPLICATION_OCTET_STREAM),
        b"zip" => Some(APPLICATION_ZIP),
        b"gz" | b"gzip" => Some(APPLICATION_GZIP),
        b"tar" => Some(Mime::from_static("application/x-tar", &[])),
        b"7z" => Some(Mime::from_static("application/x-7z-compressed", &[])),
        b"rar" => Some(Mime::from_static("application/vnd.rar", &[])),
        b"bz2" => Some(Mime::from_static("application/x-bzip2", &[])),
        b"xz" => Some(Mime::from_static("application/x-xz", &[])),
        b"zst" => Some(Mime::from_static("application/zstd", &[])),
        b"png" => Some(IMAGE_PNG),
        b"jpg" | b"jpeg" => Some(IMAGE_JPEG),
        b"gif" => Some(IMAGE_GIF),
        b"webp" => Some(IMAGE_WEBP),
        b"avif" => Some(IMAGE_AVIF),
        b"svg" => Some(IMAGE_SVG_XML),
        b"ico" => Some(IMAGE_ICO),
        b"bmp" => Some(IMAGE_BMP),
        b"tif" | b"tiff" => Some(Mime::from_static("image/tiff", &[])),
        b"mp3" => Some(AUDIO_MPEG),
        b"wav" => Some(AUDIO_WAV),
        b"ogg" | b"oga" => Some(AUDIO_OGG),
        b"flac" => Some(Mime::from_static("audio/flac", &[])),
        b"aac" => Some(Mime::from_static("audio/aac", &[])),
        b"m4a" => Some(Mime::from_static("audio/mp4", &[])),
        b"opus" => Some(Mime::from_static("audio/opus", &[])),
        b"mp4" | b"m4v" => Some(VIDEO_MP4),
        b"webm" => Some(VIDEO_WEBM),
        b"ogv" => Some(Mime::from_static("video/ogg", &[])),
        b"avi" => Some(Mime::from_static("video/x-msvideo", &[])),
        b"mov" => Some(Mime::from_static("video/quicktime", &[])),
        b"mkv" => Some(Mime::from_static("video/x-matroska", &[])),
        b"woff" => Some(FONT_WOFF),
        b"woff2" => Some(FONT_WOFF2),
        b"ttf" => Some(FONT_TTF),
        b"otf" => Some(FONT_OTF),
        b"eot" => Some(Mime::from_static("application/vnd.ms-fontobject", &[])),
        _ => None,
    }
}

/// Looks up a [`Mime`] type from the extension of `path`.
///
/// # Examples
///
/// ```
/// use akku::mime::{self, TEXT_HTML};
///
/// assert_eq!(mime::from_path("public/index.html"), Some(TEXT_HTML));
/// assert_eq!(mime::from_path("Makefile"), None);
/// ```
pub fn from_path(path: impl AsRef<Path>) -> Option<Mime> {
    path.as_ref()
        .extension()
        .and_then(|ext| ext.to_str())
        .and_then(from_extension)
}

/// `text/plain`
pub const TEXT_PLAIN: Mime = Mime::from_static("text/plain", &[]);
/// `text/plain; charset=utf-8`
pub const TEXT_PLAIN_UTF8: Mime = Mime::from_static("text/plain", &[("charset", "utf-8")]);
/// `text/html`
pub const TEXT_HTML: Mime = Mime::from_static("text/html", &[]);
/// `text/html; charset=utf-8`
pub const TEXT_HTML_UTF8: Mime = Mime::from_static("text/html", &[("charset", "utf-8")]);
/// `text/css`
pub const TEXT_CSS: Mime = Mime::from_static("text/css", &[]);
/// `text/javascript`
pub const TEXT_JAVASCRIPT: Mime = Mime::from_static("text/javascript", &[]);
/// `text/csv`
pub const TEXT_CSV: Mime = Mime::from_static("text/csv", &[]);
/// `text/xml`
pub const TEXT_XML: Mime = Mime::from_static("text/xml", &[]);
/// `text/markdown`
pub const TEXT_MARKDOWN: Mime = Mime::from_static("text/markdown", &[]);
/// `text/event-stream`
pub const TEXT_EVENT_STREAM: Mime = Mime::from_static("text/event-stream", &[]);

/// `application/json`
pub const APPLICATION_JSON: Mime = Mime::from_static("application/json", &[]);
/// `application/javascript`
pub const APPLICATION_JAVASCRIPT: Mime = Mime::from_static("application/javascript", &[]);
/// `application/xml`
pub const APPLICATION_XML: Mime = Mime::from_static("application/xml", &[]);
/// `application/octet-stream`
pub const APPLICATION_OCTET_STREAM: Mime = Mime::from_static("application/octet-stream", &[]);
/// `application/pdf`
pub const APPLICATION_PDF: Mime = Mime::from_static("application/pdf", &[]);
/// `application/zip`
pub const APPLICATION_ZIP: Mime = Mime::from_static("application/zip", &[]);
/// `application/gzip`
pub const APPLICATION_GZIP: Mime = Mime::from_static("application/gzip", &[]);
/// `application/x-www-form-urlencoded`
pub const APPLICATION_WWW_FORM_URLENCODED: Mime =
    Mime::from_static("application/x-www-form-urlencoded", &[]);
/// `application/wasm`
pub const APPLICATION_WASM: Mime = Mime::from_static("application/wasm", &[]);
/// `application/toml`
pub const APPLICATION_TOML: Mime = Mime::from_static("application/toml", &[]);
/// `application/yaml`
pub const APPLICATION_YAML: Mime = Mime::from_static("application/yaml", &[]);

/// `multipart/form-data`
pub const MULTIPART_FORM_DATA: Mime = Mime::from_static("multipart/form-data", &[]);
/// `multipart/mixed`
pub const MULTIPART_MIXED: Mime = Mime::from_static("multipart/mixed", &[]);

/// `image/png`
pub const IMAGE_PNG: Mime = Mime::from_static("image/png", &[]);
/// `image/jpeg`
pub const IMAGE_JPEG: Mime = Mime::from_static("image/jpeg", &[]);
/// `image/gif`
pub const IMAGE_GIF: Mime = Mime::from_static("image/gif", &[]);
/// `image/webp`
pub const IMAGE_WEBP: Mime = Mime::from_static("image/webp", &[]);
/// `image/avif`
pub const IMAGE_AVIF: Mime = Mime::from_static("image/avif", &[]);
/// `image/svg+xml`
pub const IMAGE_SVG_XML: Mime = Mime::from_static("image/svg+xml", &[]);
/// `image/x-icon`
pub const IMAGE_ICO: Mime = Mime::from_static("image/x-icon", &[]);
/// `image/bmp`
pub const IMAGE_BMP: Mime = Mime::from_static("image/bmp", &[]);

/// `audio/mpeg`
pub const AUDIO_MPEG: Mime = Mime::from_static("audio/mpeg", &[]);
/// `audio/wav`
pub const AUDIO_WAV: Mime = Mime::from_static("audio/wav", &[]);
/// `audio/ogg`
pub const AUDIO_OGG: Mime = Mime::from_static("audio/ogg", &[]);

/// `video/mp4`
pub const VIDEO_MP4: Mime = Mime::from_static("video/mp4", &[]);
/// `video/webm`
pub const VIDEO_WEBM: Mime = Mime::from_static("video/webm", &[]);

/// `font/woff`
pub const FONT_WOFF: Mime = Mime::from_static("font/woff", &[]);
/// `font/woff2`
pub const FONT_WOFF2: Mime = Mime::from_static("font/woff2", &[]);
/// `font/ttf`
pub const FONT_TTF: Mime = Mime::from_static("font/ttf", &[]);
/// `font/otf`
pub const FONT_OTF: Mime = Mime::from_static("font/otf", &[]);
