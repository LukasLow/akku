//! RFC 3986 §5.2 relative URL resolution (`merge` and `remove_dot_segments`).

use super::parse::{
    check_no_control_chars, is_special_scheme, parse_authority, parse_url, remove_dot_segments,
    split_scheme, validate_scheme, ParseError,
};
use super::percent::{percent_encode, FRAGMENT, PATH, QUERY};
use super::Url;

/// Merges a relative path onto a base path per RFC 3986 §5.2.3.
fn merge_paths(base_has_authority: bool, base_path: &str, rel_path: &str) -> String {
    if base_has_authority && base_path.is_empty() {
        format!("/{rel_path}")
    } else if let Some(last_slash) = base_path.rfind('/') {
        format!("{}{}", &base_path[..=last_slash], rel_path)
    } else {
        rel_path.to_owned()
    }
}

pub(crate) fn join_url(base: &Url, reference: &str) -> Result<Url, ParseError> {
    check_no_control_chars(reference)?;

    if let Some((raw_scheme, _)) = split_scheme(reference) {
        validate_scheme(raw_scheme)?;
        return parse_url(reference);
    }

    if base.cannot_be_a_base {
        if let Some(frag) = reference.strip_prefix('#') {
            let mut out = base.clone();
            out.fragment = Some(percent_encode(frag, &FRAGMENT).into_owned());
            return Ok(out);
        }
        return Err(ParseError::CannotBeABase);
    }

    let (before_frag, raw_fragment) = match reference.split_once('#') {
        Some((b, f)) => (b, Some(f)),
        None => (reference, None),
    };
    let (before_query, raw_query) = match before_frag.split_once('?') {
        Some((b, q)) => (b, Some(q)),
        None => (before_frag, None),
    };

    let special = is_special_scheme(&base.scheme);

    if let Some(after_slashes) = before_query.strip_prefix("//") {
        let (raw_auth, raw_path) = match after_slashes.find('/') {
            Some(idx) => (&after_slashes[..idx], &after_slashes[idx..]),
            None => (after_slashes, ""),
        };
        let auth = parse_authority(raw_auth, &base.scheme)?;
        let encoded_path = percent_encode(raw_path, &PATH);
        let mut path = remove_dot_segments(&encoded_path);
        if special && path.is_empty() {
            path.push('/');
        } else if !path.is_empty() && !path.starts_with('/') {
            path.insert(0, '/');
        }
        return Ok(Url {
            scheme: base.scheme.clone(),
            has_authority: true,
            username: auth.username,
            password: auth.password,
            host: auth.host,
            port: auth.port,
            path,
            query: raw_query.map(|q| percent_encode(q, &QUERY).into_owned()),
            fragment: raw_fragment.map(|f| percent_encode(f, &FRAGMENT).into_owned()),
            cannot_be_a_base: false,
        });
    }

    let rel_path = before_query;
    let (path, query) = if rel_path.is_empty() {
        let query = match raw_query {
            Some(q) => Some(percent_encode(q, &QUERY).into_owned()),
            None => base.query.clone(),
        };
        (base.path.clone(), query)
    } else {
        let encoded_rel = percent_encode(rel_path, &PATH);
        let mut path = if encoded_rel.starts_with('/') {
            remove_dot_segments(&encoded_rel)
        } else {
            let merged = merge_paths(base.has_authority, &base.path, &encoded_rel);
            remove_dot_segments(&merged)
        };
        if special && path.is_empty() {
            path.push('/');
        }
        (
            path,
            raw_query.map(|q| percent_encode(q, &QUERY).into_owned()),
        )
    };

    Ok(Url {
        scheme: base.scheme.clone(),
        has_authority: base.has_authority,
        username: base.username.clone(),
        password: base.password.clone(),
        host: base.host.clone(),
        port: base.port,
        path,
        query,
        fragment: raw_fragment.map(|f| percent_encode(f, &FRAGMENT).into_owned()),
        cannot_be_a_base: false,
    })
}
