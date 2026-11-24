//! Path and tree helpers shared by the builder and the built configuration.

use std::collections::BTreeMap;

use crate::config::value::ConfigValue;

/// Split a dotted path into its non-empty segments.
///
/// `"a.b"` yields `["a", "b"]`; repeated dots and leading or trailing dots are
/// ignored, so `"a..b."` yields the same two segments. An empty path yields no
/// segments.
pub(crate) fn split(path: &str) -> impl Iterator<Item = &str> {
    path.split('.').filter(|segment| !segment.is_empty())
}

/// Drop the recorded source for `prefix` and every path below it.
pub(crate) fn purge_sources(
    sources: &mut BTreeMap<Vec<String>, crate::config::Source>,
    prefix: &[String],
) {
    if prefix.is_empty() {
        return;
    }
    sources.retain(|existing, _| !existing.starts_with(prefix));
}

/// Look a dotted path up in a configuration tree.
pub(crate) fn lookup<'a>(
    map: &'a BTreeMap<String, ConfigValue>,
    path: &str,
) -> Option<&'a ConfigValue> {
    let mut segments = split(path);
    let first = segments.next()?;
    let mut current = map.get(first)?;
    for segment in segments {
        current = current.as_table()?.get(segment)?;
    }
    Some(current)
}

/// Look up an exact slice of path segments in a configuration tree without
/// splitting on dots.
pub(crate) fn lookup_segments<'a>(
    map: &'a BTreeMap<String, ConfigValue>,
    segments: &[&str],
) -> Option<&'a ConfigValue> {
    let (&first, rest) = segments.split_first()?;
    let mut current = map.get(first)?;
    for &segment in rest {
        current = current.as_table()?.get(segment)?;
    }
    Some(current)
}
