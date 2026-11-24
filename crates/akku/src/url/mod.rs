//! RFC 3986 and WHATWG-pragmatic URL parsing, resolution, building, and encoding.
//!
//! This module provides:
//! - [`Url`] and [`Host`] for parsing, inspecting, mutating, and resolving URLs
//! - [`UrlBuilder`] for ergonomic URL construction
//! - [`percent_encode`], [`percent_encode_bytes`], [`percent_decode`], [`percent_decode_bytes`],
//!   and [`AsciiSet`] for percent-encoding
//! - [`FormUrlEncoded`], [`QueryBuilder`], [`form_encode`], and [`form_decode`] for
//!   `application/x-www-form-urlencoded` data
//!
//! # Examples
//!
//! ```
//! use akku::url::{Host, Url};
//!
//! let base = Url::parse("https://Example.COM:443/docs/v1/intro.html?lang=en#top").unwrap();
//! assert_eq!(base.scheme(), "https");
//! assert_eq!(base.host(), Some(&Host::Domain("example.com".into())));
//! assert_eq!(base.port(), None);
//! assert_eq!(base.port_or_default(), Some(443));
//!
//! let next = base.join("../guide/start.html").unwrap();
//! assert_eq!(next.as_str(), "https://example.com/docs/guide/start.html");
//! ```

mod form;
mod join;
mod parse;
mod percent;

use std::fmt;
use std::iter::FusedIterator;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::str::FromStr;

pub use form::{form_decode, form_encode, FormUrlEncoded, QueryBuilder};
pub use parse::{ParseError, SetError};
pub use percent::{
    percent_decode, percent_decode_bytes, percent_encode, percent_encode_bytes, AsciiSet,
    PercentDecodeError, COMPONENT, FORM, FRAGMENT, NON_ALPHANUMERIC, PATH, PATH_SEGMENT, QUERY,
    USERINFO,
};

use join::join_url;
use parse::{
    check_no_control_chars, default_port, is_special_scheme, parse_host_input, parse_url,
    remove_dot_segments, validate_scheme,
};

/// The host component of a [`Url`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Host {
    /// A lowercased domain name or opaque host name.
    Domain(String),
    /// An IPv4 address.
    Ipv4(Ipv4Addr),
    /// An IPv6 address (formatted with surrounding `[...]` in [`fmt::Display`]).
    Ipv6(Ipv6Addr),
}

impl fmt::Display for Host {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Domain(domain) => f.write_str(domain),
            Self::Ipv4(addr) => write!(f, "{addr}"),
            Self::Ipv6(addr) => write!(f, "[{addr}]"),
        }
    }
}

/// A parsed and normalized URL.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Url {
    pub(crate) scheme: String,
    pub(crate) has_authority: bool,
    pub(crate) username: String,
    pub(crate) password: Option<String>,
    pub(crate) host: Option<Host>,
    pub(crate) port: Option<u16>,
    pub(crate) path: String,
    pub(crate) query: Option<String>,
    pub(crate) fragment: Option<String>,
    pub(crate) cannot_be_a_base: bool,
}

impl Url {
    /// Parses an absolute URL string.
    ///
    /// # Errors
    ///
    /// Returns [`ParseError`] if `input` is a relative reference without a base,
    /// has an invalid scheme, host, IPv6 literal, port, or contains control characters.
    pub fn parse(input: &str) -> Result<Self, ParseError> {
        parse_url(input)
    }

    /// Resolves a URL reference against `self` as the base URL per RFC 3986 §5.2.
    ///
    /// Opaque (`cannot_be_a_base`) URLs only permit `#fragment` references or full
    /// absolute URLs with a scheme; all other relative references return
    /// [`ParseError::CannotBeABase`].
    pub fn join(&self, reference: &str) -> Result<Self, ParseError> {
        join_url(self, reference)
    }

    /// Creates a [`UrlBuilder`] starting with the given URL `scheme`.
    pub fn builder(scheme: &str) -> UrlBuilder {
        UrlBuilder::new(scheme)
    }

    /// Returns the URL serialized as a `String`.
    ///
    /// Note that `Url` also implements [`fmt::Display`].
    pub fn as_str(&self) -> String {
        self.to_string()
    }

    /// Returns the lowercased scheme (without trailing `:`).
    pub fn scheme(&self) -> &str {
        &self.scheme
    }

    /// Returns the percent-encoded username, or `""` if none is present.
    pub fn username(&self) -> &str {
        &self.username
    }

    /// Returns the percent-encoded password, if present.
    pub fn password(&self) -> Option<&str> {
        self.password.as_deref()
    }

    /// Returns the parsed [`Host`], if the URL has one.
    pub fn host(&self) -> Option<&Host> {
        self.host.as_ref()
    }

    /// Returns the host serialized as a `String` without brackets for IPv6 addresses.
    pub fn hostname(&self) -> Option<String> {
        self.host.as_ref().map(|h| match h {
            Host::Domain(d) => d.clone(),
            Host::Ipv4(ip) => ip.to_string(),
            Host::Ipv6(ip) => ip.to_string(),
        })
    }

    /// Returns the explicit port if one was specified and differs from the scheme's default port.
    pub fn port(&self) -> Option<u16> {
        self.port
    }

    /// Returns the explicit port or the scheme's default port (`80` for `http`/`ws`,
    /// `443` for `https`/`wss`, `21` for `ftp`).
    pub fn port_or_default(&self) -> Option<u16> {
        self.port.or_else(|| default_port(&self.scheme))
    }

    /// Returns the formatted authority (`[userinfo@]host[:port]`), or `""` if the URL has no host.
    pub fn authority(&self) -> String {
        let Some(host) = &self.host else {
            return String::new();
        };
        let mut out = String::new();
        if !self.username.is_empty() || self.password.is_some() {
            out.push_str(&self.username);
            if let Some(pw) = &self.password {
                out.push(':');
                out.push_str(pw);
            }
            out.push('@');
        }
        out.push_str(&host.to_string());
        if let Some(port) = self.port {
            out.push(':');
            out.push_str(&port.to_string());
        }
        out
    }

    /// Returns the normalized path (or opaque body for `cannot_be_a_base` URLs).
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Returns an iterator over the `/` separated path segments, or `None` for
    /// `cannot_be_a_base` URLs.
    pub fn segments(&self) -> Option<Segments<'_>> {
        if self.cannot_be_a_base {
            return None;
        }
        if self.path.is_empty() {
            return Some(Segments { inner: None });
        }
        let rest = self.path.strip_prefix('/').unwrap_or(&self.path);
        Some(Segments {
            inner: Some(rest.split('/')),
        })
    }

    /// Returns the query string without the leading `?`, if present.
    pub fn query(&self) -> Option<&str> {
        self.query.as_deref()
    }

    /// Returns a [`FormUrlEncoded`] iterator over the query string's key-value pairs.
    pub fn query_pairs(&self) -> FormUrlEncoded<'_> {
        FormUrlEncoded::parse(self.query.as_deref().unwrap_or(""))
    }

    /// Returns the fragment identifier without the leading `#`, if present.
    pub fn fragment(&self) -> Option<&str> {
        self.fragment.as_deref()
    }

    /// Returns `true` if this URL has an opaque path (such as `mailto:`, `data:`, or `javascript:`).
    pub fn cannot_be_a_base(&self) -> bool {
        self.cannot_be_a_base
    }

    /// Returns `true` if the scheme is a special scheme (`http`, `https`, `ws`, `wss`, `ftp`, `file`).
    pub fn is_special(&self) -> bool {
        is_special_scheme(&self.scheme)
    }

    /// Sets the username, percent-encoding reserved userinfo characters.
    ///
    /// # Errors
    ///
    /// Returns [`SetError::CannotHaveHost`] if the URL is opaque, has no host, or is a `file:` URL.
    pub fn set_username(&mut self, username: &str) -> Result<(), SetError> {
        if self.cannot_be_a_base || self.host.is_none() || self.scheme == "file" {
            return Err(SetError::CannotHaveHost);
        }
        self.username = percent_encode(username, &USERINFO).into_owned();
        Ok(())
    }

    /// Sets or clears the password, percent-encoding reserved userinfo characters.
    ///
    /// # Errors
    ///
    /// Returns [`SetError::CannotHaveHost`] if the URL is opaque, has no host, or is a `file:` URL.
    pub fn set_password(&mut self, password: Option<&str>) -> Result<(), SetError> {
        if self.cannot_be_a_base || self.host.is_none() || self.scheme == "file" {
            return Err(SetError::CannotHaveHost);
        }
        self.password = password.and_then(|p| {
            if p.is_empty() {
                None
            } else {
                Some(percent_encode(p, &USERINFO).into_owned())
            }
        });
        Ok(())
    }

    /// Sets or clears the host.
    ///
    /// Accepts domain names, IPv4 addresses, and IPv6 addresses (with or without `[...]`).
    ///
    /// # Errors
    ///
    /// Returns [`ParseError::CannotBeABase`] for opaque URLs, [`ParseError::EmptyHost`]
    /// when clearing or passing an empty host on a scheme that requires one, or
    /// [`ParseError::InvalidHost`] / [`ParseError::InvalidIpv6`] for malformed hosts.
    pub fn set_host(&mut self, host: Option<&str>) -> Result<(), ParseError> {
        if self.cannot_be_a_base {
            return Err(ParseError::CannotBeABase);
        }
        match host {
            None => {
                if self.is_special() && self.scheme != "file" {
                    return Err(ParseError::EmptyHost);
                }
                self.host = None;
                self.port = None;
                self.username.clear();
                self.password = None;
                if self.scheme != "file" {
                    self.has_authority = false;
                }
                Ok(())
            }
            Some("") => {
                if self.scheme == "file" {
                    self.host = None;
                    self.port = None;
                    self.username.clear();
                    self.password = None;
                    Ok(())
                } else {
                    Err(ParseError::EmptyHost)
                }
            }
            Some(raw) => {
                let parsed = parse_host_input(raw)?;
                self.host = Some(parsed);
                self.has_authority = true;
                Ok(())
            }
        }
    }

    /// Sets or clears the port.
    ///
    /// If `port` equals the scheme's default port, [`Url::port`] will return `None`.
    ///
    /// # Errors
    ///
    /// Returns [`SetError::CannotHaveHost`] if the URL is opaque, has no host, or is a `file:` URL.
    pub fn set_port(&mut self, port: Option<u16>) -> Result<(), SetError> {
        if self.cannot_be_a_base || self.host.is_none() || self.scheme == "file" {
            return Err(SetError::CannotHaveHost);
        }
        self.port = match port {
            Some(p) if default_port(&self.scheme) == Some(p) => None,
            other => other,
        };
        Ok(())
    }

    /// Replaces the URL path, percent-encoding invalid path characters and resolving dot segments.
    ///
    /// No-op on `cannot_be_a_base` URLs.
    pub fn set_path(&mut self, path: &str) {
        if self.cannot_be_a_base {
            return;
        }
        let encoded = percent_encode(path, &PATH);
        let mut normalized = remove_dot_segments(&encoded);
        if self.is_special() && normalized.is_empty() {
            normalized.push('/');
        } else if self.has_authority && !normalized.is_empty() && !normalized.starts_with('/') {
            normalized.insert(0, '/');
        }
        self.path = normalized;
    }

    /// Sets or clears the raw query string (without leading `?`), encoding invalid query characters.
    pub fn set_query(&mut self, query: Option<&str>) {
        self.query = query.map(|q| percent_encode(q, &QUERY).into_owned());
    }

    /// Replaces the query string with `application/x-www-form-urlencoded` encoded pairs.
    ///
    /// Clears the query if `pairs` yields no items.
    pub fn set_query_pairs<I, K, V>(&mut self, pairs: I)
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<str>,
        V: AsRef<str>,
    {
        QueryBuilder::new().extend(pairs).apply(self);
    }

    /// Sets or clears the fragment identifier (without leading `#`), encoding invalid fragment characters.
    pub fn set_fragment(&mut self, fragment: Option<&str>) {
        self.fragment = fragment.map(|f| percent_encode(f, &FRAGMENT).into_owned());
    }
}

impl fmt::Display for Url {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.scheme)?;
        f.write_str(":")?;
        if !self.cannot_be_a_base && self.has_authority {
            f.write_str("//")?;
            if !self.username.is_empty() || self.password.is_some() {
                f.write_str(&self.username)?;
                if let Some(pw) = &self.password {
                    f.write_str(":")?;
                    f.write_str(pw)?;
                }
                f.write_str("@")?;
            }
            if let Some(host) = &self.host {
                write!(f, "{host}")?;
            }
            if let Some(port) = self.port {
                write!(f, ":{port}")?;
            }
        }
        f.write_str(&self.path)?;
        if let Some(query) = &self.query {
            f.write_str("?")?;
            f.write_str(query)?;
        }
        if let Some(fragment) = &self.fragment {
            f.write_str("#")?;
            f.write_str(fragment)?;
        }
        Ok(())
    }
}

impl FromStr for Url {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

/// Iterator over a [`Url`]'s path segments, returned by [`Url::segments`].
#[derive(Debug, Clone)]
pub struct Segments<'a> {
    inner: Option<std::str::Split<'a, char>>,
}

impl<'a> Iterator for Segments<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.as_mut().and_then(Iterator::next)
    }
}

impl DoubleEndedIterator for Segments<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.inner.as_mut().and_then(DoubleEndedIterator::next_back)
    }
}

impl FusedIterator for Segments<'_> {}

/// Builder for constructing a [`Url`] programmatically.
///
/// Created via [`Url::builder`].
///
/// # Examples
///
/// ```
/// use akku::url::Url;
///
/// let url = Url::builder("https")
///     .host("example.com")
///     .push_segment("api")
///     .push_segment("v1/items")
///     .query_pairs([("q", "hello world")])
///     .fragment("results")
///     .build()
///     .unwrap();
///
/// assert_eq!(url.as_str(), "https://example.com/api/v1%2Fitems?q=hello+world#results");
/// ```
#[derive(Debug, Clone)]
pub struct UrlBuilder {
    scheme: String,
    username: Option<String>,
    password: Option<String>,
    host: Option<String>,
    port: Option<u16>,
    path: Option<String>,
    query: Option<String>,
    fragment: Option<String>,
}

impl UrlBuilder {
    fn new(scheme: &str) -> Self {
        Self {
            scheme: scheme.to_owned(),
            username: None,
            password: None,
            host: None,
            port: None,
            path: None,
            query: None,
            fragment: None,
        }
    }

    /// Sets the username.
    pub fn username(mut self, username: &str) -> Self {
        self.username = Some(username.to_owned());
        self
    }

    /// Sets the password.
    pub fn password(mut self, password: &str) -> Self {
        self.password = Some(password.to_owned());
        self
    }

    /// Sets the host (domain name, IPv4, `[::1]`, or bare `::1`).
    pub fn host(mut self, host: &str) -> Self {
        self.host = Some(host.to_owned());
        self
    }

    /// Sets the port.
    pub fn port(mut self, port: u16) -> Self {
        self.port = Some(port);
        self
    }

    /// Sets the path (replacing any previously set path or pushed segments).
    pub fn path(mut self, path: &str) -> Self {
        self.path = Some(percent_encode(path, &PATH).into_owned());
        self
    }

    /// Appends a single path segment, percent-encoding `/` and `%` (as well as other
    /// unsafe path segment characters).
    pub fn push_segment(mut self, segment: &str) -> Self {
        let encoded = percent_encode(segment, &PATH_SEGMENT);
        let path = self.path.get_or_insert_with(String::new);
        if !path.ends_with('/') {
            path.push('/');
        }
        path.push_str(&encoded);
        self
    }

    /// Sets the raw query string (without leading `?`).
    pub fn query(mut self, query: &str) -> Self {
        self.query = Some(query.to_owned());
        self
    }

    /// Sets the query string from key-value pairs using `application/x-www-form-urlencoded`.
    pub fn query_pairs<I, K, V>(mut self, pairs: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<str>,
        V: AsRef<str>,
    {
        let qb = QueryBuilder::new().extend(pairs);
        self.query = if qb.is_empty() {
            None
        } else {
            Some(qb.finish())
        };
        self
    }

    /// Sets the fragment identifier (without leading `#`).
    pub fn fragment(mut self, fragment: &str) -> Self {
        self.fragment = Some(fragment.to_owned());
        self
    }

    /// Validates all fields and constructs the [`Url`].
    pub fn build(self) -> Result<Url, ParseError> {
        check_no_control_chars(&self.scheme)?;
        let scheme = validate_scheme(&self.scheme)?;
        let special = is_special_scheme(&scheme);

        let host = match self.host.as_deref() {
            None => {
                if special && scheme != "file" {
                    return Err(ParseError::EmptyHost);
                }
                if self.username.is_some() || self.password.is_some() || self.port.is_some() {
                    return Err(ParseError::EmptyHost);
                }
                None
            }
            Some("") => {
                if scheme == "file" {
                    None
                } else {
                    return Err(ParseError::EmptyHost);
                }
            }
            Some(raw) => Some(parse_host_input(raw)?),
        };

        let port = match self.port {
            None => None,
            Some(p) => {
                if host.is_none() || scheme == "file" {
                    return Err(ParseError::InvalidPort);
                }
                if default_port(&scheme) == Some(p) {
                    None
                } else {
                    Some(p)
                }
            }
        };

        let username = match self.username {
            Some(u) => {
                check_no_control_chars(&u)?;
                if scheme == "file" {
                    return Err(ParseError::InvalidCharacter);
                }
                percent_encode(&u, &USERINFO).into_owned()
            }
            None => String::new(),
        };

        let password = match self.password {
            Some(p) => {
                check_no_control_chars(&p)?;
                if scheme == "file" {
                    return Err(ParseError::InvalidCharacter);
                }
                if p.is_empty() {
                    None
                } else {
                    Some(percent_encode(&p, &USERINFO).into_owned())
                }
            }
            None => None,
        };

        let has_authority = host.is_some() || scheme == "file";
        let raw_path = self.path.unwrap_or_default();
        check_no_control_chars(&raw_path)?;

        let (path, cannot_be_a_base) =
            if !has_authority && !raw_path.is_empty() && !raw_path.starts_with('/') {
                (percent_encode(&raw_path, &FRAGMENT).into_owned(), true)
            } else {
                let mut normalized = remove_dot_segments(&raw_path);
                if special && normalized.is_empty() {
                    normalized.push('/');
                } else if has_authority && !normalized.is_empty() && !normalized.starts_with('/') {
                    normalized.insert(0, '/');
                }
                (normalized, false)
            };

        let query = match self.query {
            Some(q) => {
                check_no_control_chars(&q)?;
                Some(percent_encode(&q, &QUERY).into_owned())
            }
            None => None,
        };

        let fragment = match self.fragment {
            Some(f) => {
                check_no_control_chars(&f)?;
                Some(percent_encode(&f, &FRAGMENT).into_owned())
            }
            None => None,
        };

        Ok(Url {
            scheme,
            has_authority,
            username,
            password,
            host,
            port,
            path,
            query,
            fragment,
            cannot_be_a_base,
        })
    }
}
