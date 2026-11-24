//! Akku-owned core HTTP types (`Method`, `StatusCode`, `Headers`).
//!
//! These types are intentionally independent of any backend crate so server and
//! client code share a single, stable vocabulary across future runtime swaps.

use std::fmt;
use std::str::FromStr;

/// Error returned when parsing an invalid HTTP method token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidMethodError {
    invalid: String,
}

impl InvalidMethodError {
    /// Returns the rejected method string.
    pub fn as_str(&self) -> &str {
        &self.invalid
    }
}

impl fmt::Display for InvalidMethodError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid HTTP method: {:?}", self.invalid)
    }
}

impl std::error::Error for InvalidMethodError {}

/// An HTTP request method.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Method {
    /// HTTP `GET`.
    Get,
    /// HTTP `POST`.
    Post,
    /// HTTP `PUT`.
    Put,
    /// HTTP `PATCH`.
    Patch,
    /// HTTP `DELETE`.
    Delete,
    /// HTTP `HEAD`.
    Head,
    /// HTTP `OPTIONS`.
    Options,
    /// A custom or extension HTTP method token (e.g. `"PROPFIND"`).
    Custom(String),
}

impl Method {
    /// Returns the wire-format method token (e.g. `"GET"`).
    pub fn as_str(&self) -> &str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
            Self::Head => "HEAD",
            Self::Options => "OPTIONS",
            Self::Custom(s) => s.as_str(),
        }
    }
}

impl fmt::Display for Method {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Method {
    type Err = InvalidMethodError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() || !s.bytes().all(is_http_tchar) {
            return Err(InvalidMethodError {
                invalid: s.to_string(),
            });
        }
        if s.eq_ignore_ascii_case("GET") {
            Ok(Self::Get)
        } else if s.eq_ignore_ascii_case("POST") {
            Ok(Self::Post)
        } else if s.eq_ignore_ascii_case("PUT") {
            Ok(Self::Put)
        } else if s.eq_ignore_ascii_case("PATCH") {
            Ok(Self::Patch)
        } else if s.eq_ignore_ascii_case("DELETE") {
            Ok(Self::Delete)
        } else if s.eq_ignore_ascii_case("HEAD") {
            Ok(Self::Head)
        } else if s.eq_ignore_ascii_case("OPTIONS") {
            Ok(Self::Options)
        } else {
            Ok(Self::Custom(s.to_string()))
        }
    }
}

/// Returns `true` if `b` is a valid HTTP token character (`tchar`, RFC 9110 §5.6.2).
pub(crate) fn is_http_tchar(b: u8) -> bool {
    b.is_ascii_alphanumeric()
        || matches!(
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
        )
}

/// An HTTP response status code (e.g. `200 OK`, `404 Not Found`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StatusCode(u16);

impl StatusCode {
    /// `200 OK`
    pub const OK: Self = Self(200);
    /// `201 Created`
    pub const CREATED: Self = Self(201);
    /// `204 No Content`
    pub const NO_CONTENT: Self = Self(204);
    /// `301 Moved Permanently`
    pub const MOVED_PERMANENTLY: Self = Self(301);
    /// `302 Found`
    pub const FOUND: Self = Self(302);
    /// `303 See Other`
    pub const SEE_OTHER: Self = Self(303);
    /// `307 Temporary Redirect`
    pub const TEMPORARY_REDIRECT: Self = Self(307);
    /// `308 Permanent Redirect`
    pub const PERMANENT_REDIRECT: Self = Self(308);
    /// `400 Bad Request`
    pub const BAD_REQUEST: Self = Self(400);
    /// `401 Unauthorized`
    pub const UNAUTHORIZED: Self = Self(401);
    /// `403 Forbidden`
    pub const FORBIDDEN: Self = Self(403);
    /// `404 Not Found`
    pub const NOT_FOUND: Self = Self(404);
    /// `405 Method Not Allowed`
    pub const METHOD_NOT_ALLOWED: Self = Self(405);
    /// `409 Conflict`
    pub const CONFLICT: Self = Self(409);
    /// `422 Unprocessable Entity`
    pub const UNPROCESSABLE_ENTITY: Self = Self(422);
    /// `429 Too Many Requests`
    pub const TOO_MANY_REQUESTS: Self = Self(429);
    /// `500 Internal Server Error`
    pub const INTERNAL_SERVER_ERROR: Self = Self(500);
    /// `502 Bad Gateway`
    pub const BAD_GATEWAY: Self = Self(502);
    /// `503 Service Unavailable`
    pub const SERVICE_UNAVAILABLE: Self = Self(503);

    /// Creates a [`StatusCode`] from a numeric HTTP status code in `100..=999`.
    pub const fn from_u16(code: u16) -> Option<Self> {
        if code >= 100 && code <= 999 {
            Some(Self(code))
        } else {
            None
        }
    }

    /// Returns the numeric status code.
    pub const fn as_u16(self) -> u16 {
        self.0
    }

    /// Returns `true` if the status code is in `100..=199`.
    pub const fn is_informational(self) -> bool {
        self.0 >= 100 && self.0 < 200
    }

    /// Returns `true` if the status code is in `200..=299`.
    pub const fn is_success(self) -> bool {
        self.0 >= 200 && self.0 < 300
    }

    /// Returns `true` if the status code is in `300..=399`.
    pub const fn is_redirection(self) -> bool {
        self.0 >= 300 && self.0 < 400
    }

    /// Returns `true` if the status code is in `400..=499`.
    pub const fn is_client_error(self) -> bool {
        self.0 >= 400 && self.0 < 500
    }

    /// Returns `true` if the status code is in `500..=599`.
    pub const fn is_server_error(self) -> bool {
        self.0 >= 500 && self.0 < 600
    }

    /// Returns the standard canonical reason phrase for known status codes.
    pub const fn canonical_reason(self) -> Option<&'static str> {
        match self.0 {
            100 => Some("Continue"),
            101 => Some("Switching Protocols"),
            200 => Some("OK"),
            201 => Some("Created"),
            202 => Some("Accepted"),
            204 => Some("No Content"),
            206 => Some("Partial Content"),
            301 => Some("Moved Permanently"),
            302 => Some("Found"),
            303 => Some("See Other"),
            304 => Some("Not Modified"),
            307 => Some("Temporary Redirect"),
            308 => Some("Permanent Redirect"),
            400 => Some("Bad Request"),
            401 => Some("Unauthorized"),
            403 => Some("Forbidden"),
            404 => Some("Not Found"),
            405 => Some("Method Not Allowed"),
            408 => Some("Request Timeout"),
            409 => Some("Conflict"),
            410 => Some("Gone"),
            413 => Some("Payload Too Large"),
            415 => Some("Unsupported Media Type"),
            422 => Some("Unprocessable Entity"),
            429 => Some("Too Many Requests"),
            500 => Some("Internal Server Error"),
            501 => Some("Not Implemented"),
            502 => Some("Bad Gateway"),
            503 => Some("Service Unavailable"),
            504 => Some("Gateway Timeout"),
            _ => None,
        }
    }
}

impl From<StatusCode> for u16 {
    fn from(status: StatusCode) -> Self {
        status.0
    }
}

impl fmt::Display for StatusCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.canonical_reason() {
            Some(reason) => write!(f, "{} {reason}", self.0),
            None => write!(f, "{}", self.0),
        }
    }
}

/// Case-insensitive multi-value HTTP header collection preserving insertion order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Headers {
    entries: Vec<(String, String)>,
}

impl Headers {
    /// Creates an empty header collection.
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Sets `name` to `value`, replacing any existing headers with the same
    /// ASCII case-insensitive name.
    pub fn insert(&mut self, name: impl Into<String>, value: impl Into<String>) {
        let name = name.into();
        let value = value.into();
        let mut first_pos = None;
        let mut idx = 0;
        while idx < self.entries.len() {
            if self.entries[idx].0.eq_ignore_ascii_case(&name) {
                if first_pos.is_none() {
                    first_pos = Some(idx);
                    self.entries[idx] = (name.clone(), value.clone());
                    idx += 1;
                } else {
                    self.entries.remove(idx);
                }
            } else {
                idx += 1;
            }
        }
        if first_pos.is_none() {
            self.entries.push((name, value));
        }
    }

    /// Appends a header `(name, value)` without removing existing headers of the
    /// same name.
    pub fn append(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.entries.push((name.into(), value.into()));
    }

    /// Returns the first value associated with `name` (ASCII case-insensitive).
    pub fn get(&self, name: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// Returns all values associated with `name` (ASCII case-insensitive) in
    /// insertion order.
    pub fn get_all(&self, name: &str) -> Vec<&str> {
        self.entries
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
            .collect()
    }

    /// Returns `true` if any header with `name` (ASCII case-insensitive) is present.
    pub fn contains(&self, name: &str) -> bool {
        self.entries
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case(name))
    }

    /// Removes all headers matching `name` (ASCII case-insensitive) and returns
    /// their values in insertion order.
    pub fn remove(&mut self, name: &str) -> Vec<String> {
        let mut removed = Vec::new();
        let mut idx = 0;
        while idx < self.entries.len() {
            if self.entries[idx].0.eq_ignore_ascii_case(name) {
                let (_, v) = self.entries.remove(idx);
                removed.push(v);
            } else {
                idx += 1;
            }
        }
        removed
    }

    /// Iterates over all `(name, value)` header pairs in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    /// Returns the total number of header entries (including multi-value duplicates).
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns `true` if there are no header entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl IntoIterator for Headers {
    type Item = (String, String);
    type IntoIter = std::vec::IntoIter<(String, String)>;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.into_iter()
    }
}

impl<'a> IntoIterator for &'a Headers {
    type Item = (&'a str, &'a str);
    type IntoIter = std::iter::Map<
        std::slice::Iter<'a, (String, String)>,
        fn(&'a (String, String)) -> (&'a str, &'a str),
    >;

    fn into_iter(self) -> Self::IntoIter {
        fn as_pair(entry: &(String, String)) -> (&str, &str) {
            (entry.0.as_str(), entry.1.as_str())
        }
        self.entries.iter().map(as_pair)
    }
}
