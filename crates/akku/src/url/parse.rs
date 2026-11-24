//! URL parsing, host/port validation, and path dot-segment removal.

use std::error::Error;
use std::fmt;
use std::net::{Ipv4Addr, Ipv6Addr};

use super::percent::{hex_val, percent_encode, FRAGMENT, PATH, QUERY, USERINFO};
use super::{Host, Url};

/// Error returned when parsing or resolving a [`Url`], or setting a host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ParseError {
    /// A relative URL reference was provided without a base URL.
    RelativeUrlWithoutBase,
    /// The URL scheme is missing, empty, or contains invalid characters.
    InvalidScheme,
    /// The URL requires a host in its authority, but the host was empty.
    EmptyHost,
    /// The host name or IPv4 address is malformed.
    InvalidHost,
    /// The IPv6 address literal is malformed or missing a closing bracket.
    InvalidIpv6,
    /// The port number is not a valid 16-bit unsigned integer (`0..=65535`).
    InvalidPort,
    /// The input contains a forbidden control character.
    InvalidCharacter,
    /// The base URL is opaque (`cannot_be_a_base`) and cannot resolve this reference or accept a host.
    CannotBeABase,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::RelativeUrlWithoutBase => f.write_str("relative URL without a base"),
            Self::InvalidScheme => f.write_str("invalid URL scheme"),
            Self::EmptyHost => f.write_str("empty host"),
            Self::InvalidHost => f.write_str("invalid host"),
            Self::InvalidIpv6 => f.write_str("invalid IPv6 address"),
            Self::InvalidPort => f.write_str("invalid port number"),
            Self::InvalidCharacter => f.write_str("invalid character in URL"),
            Self::CannotBeABase => f.write_str("URL cannot be a base"),
        }
    }
}

impl Error for ParseError {}

/// Error returned when modifying userinfo or port on a [`Url`] that cannot have a host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SetError {
    /// The URL cannot have a host (for example, an opaque URL or `file:` without a host).
    CannotHaveHost,
}

impl fmt::Display for SetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::CannotHaveHost => f.write_str("URL cannot have a host"),
        }
    }
}

impl Error for SetError {}

pub(crate) fn is_special_scheme(scheme: &str) -> bool {
    matches!(scheme, "http" | "https" | "ws" | "wss" | "ftp" | "file")
}

pub(crate) fn default_port(scheme: &str) -> Option<u16> {
    match scheme {
        "http" | "ws" => Some(80),
        "https" | "wss" => Some(443),
        "ftp" => Some(21),
        _ => None,
    }
}

pub(crate) fn validate_scheme(scheme: &str) -> Result<String, ParseError> {
    let bytes = scheme.as_bytes();
    if bytes.is_empty() || !bytes[0].is_ascii_alphabetic() {
        return Err(ParseError::InvalidScheme);
    }
    for &b in &bytes[1..] {
        if !(b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.')) {
            return Err(ParseError::InvalidScheme);
        }
    }
    Ok(scheme.to_ascii_lowercase())
}

pub(crate) fn check_no_control_chars(input: &str) -> Result<(), ParseError> {
    if input.bytes().any(|b| b < 0x20 || b == 0x7F) {
        return Err(ParseError::InvalidCharacter);
    }
    Ok(())
}

/// Checks if `input` starts with `<something>:` before `/`, `?`, or `#`.
pub(crate) fn split_scheme(input: &str) -> Option<(&str, &str)> {
    for (idx, b) in input.bytes().enumerate() {
        match b {
            b':' => return Some((&input[..idx], &input[idx + 1..])),
            b'/' | b'?' | b'#' => return None,
            _ => {}
        }
    }
    None
}

fn is_valid_domain_char(b: u8) -> bool {
    !matches!(
        b,
        0x00..=0x20
            | 0x7F..=0xFF
            | b'<'
            | b'>'
            | b'"'
            | b'`'
            | b'{'
            | b'}'
            | b'|'
            | b'^'
            | b'\\'
            | b'['
            | b']'
            | b'/'
            | b'?'
            | b'#'
            | b'@'
            | b':'
    )
}

fn validate_domain(raw: &str) -> Result<String, ParseError> {
    if raw.is_empty() {
        return Err(ParseError::EmptyHost);
    }
    let bytes = raw.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if !is_valid_domain_char(b) {
            return Err(ParseError::InvalidHost);
        }
        if b == b'%' {
            if i + 2 >= bytes.len()
                || hex_val(bytes[i + 1]).is_none()
                || hex_val(bytes[i + 2]).is_none()
            {
                return Err(ParseError::InvalidHost);
            }
            i += 3;
            continue;
        }
        i += 1;
    }
    Ok(raw.to_ascii_lowercase())
}

/// Parses a host string as provided to [`Url::set_host`] or [`super::UrlBuilder::host`].
///
/// Accepts `[::1]` as well as bare `::1` for IPv6 addresses.
pub(crate) fn parse_host_input(input: &str) -> Result<Host, ParseError> {
    check_no_control_chars(input)?;
    if input.is_empty() {
        return Err(ParseError::EmptyHost);
    }
    if input.starts_with('[') {
        if !input.ends_with(']') {
            return Err(ParseError::InvalidIpv6);
        }
        let inner = &input[1..input.len() - 1];
        let addr: Ipv6Addr = inner.parse().map_err(|_| ParseError::InvalidIpv6)?;
        return Ok(Host::Ipv6(addr));
    }
    if input.contains('[') || input.contains(']') {
        return Err(ParseError::InvalidIpv6);
    }
    if input.contains(':') {
        let addr: Ipv6Addr = input.parse().map_err(|_| ParseError::InvalidHost)?;
        return Ok(Host::Ipv6(addr));
    }
    if let Ok(ipv4) = input.parse::<Ipv4Addr>() {
        return Ok(Host::Ipv4(ipv4));
    }
    Ok(Host::Domain(validate_domain(input)?))
}

pub(crate) struct ParsedAuthority {
    pub(crate) username: String,
    pub(crate) password: Option<String>,
    pub(crate) host: Option<Host>,
    pub(crate) port: Option<u16>,
}

pub(crate) fn parse_authority(raw: &str, scheme: &str) -> Result<ParsedAuthority, ParseError> {
    let special = is_special_scheme(scheme);

    let (username, password, host_port) = match raw.rsplit_once('@') {
        Some((userinfo, hp)) => {
            if let Some((u, p)) = userinfo.split_once(':') {
                let enc_u = percent_encode(u, &USERINFO).into_owned();
                let enc_p = if p.is_empty() {
                    None
                } else {
                    Some(percent_encode(p, &USERINFO).into_owned())
                };
                (enc_u, enc_p, hp)
            } else {
                (percent_encode(userinfo, &USERINFO).into_owned(), None, hp)
            }
        }
        None => (String::new(), None, raw),
    };

    let (host, raw_port) = if let Some(after_open) = host_port.strip_prefix('[') {
        let Some(close_idx) = after_open.find(']') else {
            return Err(ParseError::InvalidIpv6);
        };
        let ipv6_str = &after_open[..close_idx];
        let after_bracket = &after_open[close_idx + 1..];
        let addr: Ipv6Addr = ipv6_str.parse().map_err(|_| ParseError::InvalidIpv6)?;
        let port = if after_bracket.is_empty() {
            None
        } else if let Some(p) = after_bracket.strip_prefix(':') {
            Some(p)
        } else {
            return Err(ParseError::InvalidIpv6);
        };
        (Some(Host::Ipv6(addr)), port)
    } else {
        let (raw_host, raw_port) = match host_port.split_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (host_port, None),
        };
        if raw_host.contains('[') || raw_host.contains(']') {
            return Err(ParseError::InvalidIpv6);
        }
        let host = if raw_host.is_empty() {
            if special && scheme != "file" {
                return Err(ParseError::EmptyHost);
            }
            if !username.is_empty() || password.is_some() || raw_port.is_some() {
                return Err(ParseError::EmptyHost);
            }
            None
        } else if let Ok(ipv4) = raw_host.parse::<Ipv4Addr>() {
            Some(Host::Ipv4(ipv4))
        } else {
            Some(Host::Domain(validate_domain(raw_host)?))
        };
        (host, raw_port)
    };

    let port = match raw_port {
        None | Some("") => None,
        Some(p) => {
            if !p.bytes().all(|b| b.is_ascii_digit()) {
                return Err(ParseError::InvalidPort);
            }
            let num: u16 = p.parse().map_err(|_| ParseError::InvalidPort)?;
            if host.is_none() || scheme == "file" {
                return Err(ParseError::InvalidPort);
            }
            if default_port(scheme) == Some(num) {
                None
            } else {
                Some(num)
            }
        }
    };

    Ok(ParsedAuthority {
        username,
        password,
        host,
        port,
    })
}

/// Removes dot segments from a URI path according to RFC 3986 §5.2.4.
pub(crate) fn remove_dot_segments(path: &str) -> String {
    let mut input = path;
    let mut output = String::with_capacity(path.len());

    while !input.is_empty() {
        if let Some(rest) = input.strip_prefix("../") {
            input = rest;
        } else if let Some(rest) = input.strip_prefix("./") {
            input = rest;
        } else if let Some(rest) = input.strip_prefix("/./") {
            input = &input[2..];
            let _ = rest;
        } else if input == "/." {
            input = "/";
        } else if input.starts_with("/../") {
            input = &input[3..];
            pop_last_segment(&mut output);
        } else if input == "/.." {
            input = "/";
            pop_last_segment(&mut output);
        } else if input == "." || input == ".." {
            input = "";
        } else {
            let search_from = if input.starts_with('/') { 1 } else { 0 };
            if let Some(rel_slash) = input[search_from..].find('/') {
                let end = search_from + rel_slash;
                output.push_str(&input[..end]);
                input = &input[end..];
            } else {
                output.push_str(input);
                input = "";
            }
        }
    }

    output
}

fn pop_last_segment(output: &mut String) {
    if let Some(idx) = output.rfind('/') {
        output.truncate(idx);
    } else {
        output.clear();
    }
}

pub(crate) fn parse_url(input: &str) -> Result<Url, ParseError> {
    check_no_control_chars(input)?;

    let Some((raw_scheme, rest)) = split_scheme(input) else {
        return Err(ParseError::RelativeUrlWithoutBase);
    };
    let scheme = validate_scheme(raw_scheme)?;
    let special = is_special_scheme(&scheme);

    let (before_frag, raw_fragment) = match rest.split_once('#') {
        Some((b, f)) => (b, Some(f)),
        None => (rest, None),
    };
    let (before_query, raw_query) = match before_frag.split_once('?') {
        Some((b, q)) => (b, Some(q)),
        None => (before_frag, None),
    };

    let (has_authority, authority, path, cannot_be_a_base) =
        if let Some(after_slashes) = before_query.strip_prefix("//") {
            let (raw_auth, raw_path) = match after_slashes.find('/') {
                Some(idx) => (&after_slashes[..idx], &after_slashes[idx..]),
                None => (after_slashes, ""),
            };
            let auth = parse_authority(raw_auth, &scheme)?;
            let encoded_path = percent_encode(raw_path, &PATH);
            let mut normalized_path = remove_dot_segments(&encoded_path);
            if special && normalized_path.is_empty() {
                normalized_path.push('/');
            } else if !normalized_path.is_empty() && !normalized_path.starts_with('/') {
                normalized_path.insert(0, '/');
            }
            (true, auth, normalized_path, false)
        } else if scheme == "file" {
            let raw_path = if before_query.is_empty() {
                "/"
            } else if before_query.starts_with('/') {
                before_query
            } else {
                return Err(ParseError::EmptyHost);
            };
            let encoded_path = percent_encode(raw_path, &PATH);
            let mut normalized_path = remove_dot_segments(&encoded_path);
            if normalized_path.is_empty() {
                normalized_path.push('/');
            }
            let auth = ParsedAuthority {
                username: String::new(),
                password: None,
                host: None,
                port: None,
            };
            (true, auth, normalized_path, false)
        } else if special {
            return Err(ParseError::EmptyHost);
        } else if before_query.starts_with('/') {
            let encoded_path = percent_encode(before_query, &PATH);
            let normalized_path = remove_dot_segments(&encoded_path);
            let auth = ParsedAuthority {
                username: String::new(),
                password: None,
                host: None,
                port: None,
            };
            (false, auth, normalized_path, false)
        } else {
            let opaque_path = percent_encode(before_query, &FRAGMENT).into_owned();
            let auth = ParsedAuthority {
                username: String::new(),
                password: None,
                host: None,
                port: None,
            };
            (false, auth, opaque_path, true)
        };

    let query = raw_query.map(|q| percent_encode(q, &QUERY).into_owned());
    let fragment = raw_fragment.map(|f| percent_encode(f, &FRAGMENT).into_owned());

    Ok(Url {
        scheme,
        has_authority,
        username: authority.username,
        password: authority.password,
        host: authority.host,
        port: authority.port,
        path,
        query,
        fragment,
        cannot_be_a_base,
    })
}
