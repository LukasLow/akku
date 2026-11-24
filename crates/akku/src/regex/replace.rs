//! Replacement strings and the [`Replacer`] trait.
//!
//! # Expansion syntax
//!
//! | Syntax | Expands to |
//! |---|---|
//! | `$$` | a literal `$` |
//! | `$0`, `$1`, ... | the text of group *n* |
//! | `$name` | the text of the named group; the name is the **longest** run of `[A-Za-z0-9_]`, so `$1x` means the group named `1x` |
//! | `${1}`, `${name}` | the group, explicitly delimited — write `${1}x` for "group 1 followed by `x`" |
//! | `$` followed by anything else, or at the end | a literal `$` |
//!
//! A group that does not exist or did not participate expands to nothing.

use std::borrow::Cow;

use super::Captures;

/// Something that can produce the replacement for a match.
///
/// Implemented for `&str`, `String`, `&String` and `Cow<str>` (with `$`
/// expansion), for [`NoExpand`] (taken literally) and for closures
/// `FnMut(&Captures) -> impl AsRef<str>`.
///
/// # Examples
///
/// ```
/// use akku::regex::{Captures, Regex};
///
/// let re = Regex::new(r"\d+").unwrap();
/// let doubled = re.replace_all("3 apples, 21 pears", |caps: &Captures<'_>| {
///     let n: u32 = caps[0].parse().unwrap();
///     (n * 2).to_string()
/// });
/// assert_eq!(doubled, "6 apples, 42 pears");
/// ```
pub trait Replacer {
    /// Appends the replacement for `caps` to `dst`.
    fn replace_append(&mut self, caps: &Captures<'_>, dst: &mut String);

    /// Returns the replacement if it does not depend on the match.
    ///
    /// When this returns `Some`, the engine skips capture extraction.
    fn no_expansion(&mut self) -> Option<Cow<'_, str>> {
        None
    }
}

fn literal_if_plain(s: &str) -> Option<Cow<'_, str>> {
    if s.contains('$') {
        None
    } else {
        Some(Cow::Borrowed(s))
    }
}

impl Replacer for &str {
    fn replace_append(&mut self, caps: &Captures<'_>, dst: &mut String) {
        caps.expand(self, dst);
    }

    fn no_expansion(&mut self) -> Option<Cow<'_, str>> {
        literal_if_plain(self)
    }
}

impl Replacer for String {
    fn replace_append(&mut self, caps: &Captures<'_>, dst: &mut String) {
        caps.expand(self, dst);
    }

    fn no_expansion(&mut self) -> Option<Cow<'_, str>> {
        literal_if_plain(self)
    }
}

impl Replacer for &String {
    fn replace_append(&mut self, caps: &Captures<'_>, dst: &mut String) {
        caps.expand(self, dst);
    }

    fn no_expansion(&mut self) -> Option<Cow<'_, str>> {
        literal_if_plain(self)
    }
}

impl Replacer for Cow<'_, str> {
    fn replace_append(&mut self, caps: &Captures<'_>, dst: &mut String) {
        caps.expand(self, dst);
    }

    fn no_expansion(&mut self) -> Option<Cow<'_, str>> {
        literal_if_plain(self)
    }
}

impl<F, T> Replacer for F
where
    F: FnMut(&Captures<'_>) -> T,
    T: AsRef<str>,
{
    fn replace_append(&mut self, caps: &Captures<'_>, dst: &mut String) {
        dst.push_str((*self)(caps).as_ref());
    }
}

/// A replacement inserted verbatim — `$` has no special meaning.
///
/// # Examples
///
/// ```
/// use akku::regex::{NoExpand, Regex};
///
/// let re = Regex::new(r"(\w+)").unwrap();
/// assert_eq!(re.replace("price", NoExpand("$1")), "$1");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoExpand<'a>(pub &'a str);

impl Replacer for NoExpand<'_> {
    fn replace_append(&mut self, _caps: &Captures<'_>, dst: &mut String) {
        dst.push_str(self.0);
    }

    fn no_expansion(&mut self) -> Option<Cow<'_, str>> {
        Some(Cow::Borrowed(self.0))
    }
}

/// Expands `replacement` for `caps` into `dst` (see the module docs).
pub(crate) fn expand(caps: &Captures<'_>, replacement: &str, dst: &mut String) {
    let mut rest = replacement;
    while let Some(dollar) = rest.find('$') {
        dst.push_str(&rest[..dollar]);
        let after = &rest[dollar + 1..];
        if let Some(tail) = after.strip_prefix('$') {
            dst.push('$');
            rest = tail;
            continue;
        }
        if let Some(braced) = after.strip_prefix('{') {
            if let Some(close) = braced.find('}') {
                push_group(caps, &braced[..close], dst);
                rest = &braced[close + 1..];
            } else {
                dst.push('$');
                rest = after;
            }
            continue;
        }
        let n = after
            .bytes()
            .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
            .count();
        if n == 0 {
            dst.push('$');
            rest = after;
            continue;
        }
        push_group(caps, &after[..n], dst);
        rest = &after[n..];
    }
    dst.push_str(rest);
}

fn push_group(caps: &Captures<'_>, reference: &str, dst: &mut String) {
    let m = if !reference.is_empty() && reference.bytes().all(|b| b.is_ascii_digit()) {
        reference.parse::<usize>().ok().and_then(|i| caps.get(i))
    } else {
        caps.name(reference)
    };
    if let Some(m) = m {
        dst.push_str(m.as_str());
    }
}
