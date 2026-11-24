//! [`RegexSet`]: match many patterns in a single linear pass.

use std::fmt;
use std::sync::Arc;

use super::ast;
use super::builder::Config;
use super::dfa;
use super::error::{Error, Result};
use super::nfa::{self, Program};
use super::pikevm;
use super::Pool;

/// A set of patterns matched simultaneously in one pass over the haystack.
///
/// The whole set is compiled into a single program, so the cost of a search
/// is `O(n · m)` for the combined program size `m` — not one pass per
/// pattern.
///
/// # Examples
///
/// ```
/// use akku::regex::RegexSet;
///
/// let set = RegexSet::new([r"\d+", "foo", "^bar"]).unwrap();
/// let hits: Vec<usize> = set.matches("foo 42").iter().collect();
/// assert_eq!(hits, vec![0, 1]);
/// assert!(set.is_match("bar?"));
/// assert!(!set.is_match("nothing here"));
/// ```
#[derive(Clone)]
pub struct RegexSet {
    inner: Arc<SetInner>,
}

struct SetInner {
    patterns: Vec<String>,
    prog: Option<Program>,
    pool: Pool,
}

impl RegexSet {
    /// Compiles every pattern with default options.
    ///
    /// # Errors
    ///
    /// Returns the [`Error`] of the first pattern that fails to compile, or a
    /// size-limit error for the combined program.
    pub fn new<I, S>(patterns: I) -> Result<RegexSet>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let config = Config::default();
        let patterns: Vec<String> = patterns
            .into_iter()
            .map(|p| p.as_ref().to_owned())
            .collect();
        if patterns.is_empty() {
            return Ok(RegexSet::empty());
        }
        let mut hirs = Vec::with_capacity(patterns.len());
        for p in &patterns {
            hirs.push(ast::parse(p, &config)?.hir);
        }
        let prog = nfa::compile_set(&hirs, config.size_limit)
            .map_err(|kind| Error::new(kind, None, &patterns.join("|")))?;
        Ok(RegexSet {
            inner: Arc::new(SetInner {
                patterns,
                prog: Some(prog),
                pool: Pool::new(config.dfa_size_limit),
            }),
        })
    }

    /// A set without patterns; it never matches.
    pub fn empty() -> RegexSet {
        RegexSet {
            inner: Arc::new(SetInner {
                patterns: Vec::new(),
                prog: None,
                pool: Pool::new(0),
            }),
        }
    }

    /// Whether any pattern matches anywhere in `text`.
    pub fn is_match(&self, text: &str) -> bool {
        let Some(prog) = &self.inner.prog else {
            return false;
        };
        let mut cache = self.inner.pool.get(prog);
        if let Some(d) = cache.dfa.as_mut() {
            if let Some(found) = dfa::is_match(prog, d, text, 0) {
                return found;
            }
        }
        pikevm::search(prog, &mut cache.pike, text, 0, 0, None, true)
    }

    /// Which patterns match anywhere in `text`.
    pub fn matches(&self, text: &str) -> SetMatches {
        let mut matched = vec![false; self.inner.patterns.len()];
        if let Some(prog) = &self.inner.prog {
            let mut cache = self.inner.pool.get(prog);
            pikevm::search(prog, &mut cache.pike, text, 0, 0, Some(&mut matched), false);
        }
        SetMatches { matched }
    }

    /// Number of patterns.
    pub fn len(&self) -> usize {
        self.inner.patterns.len()
    }

    /// Whether the set has no patterns.
    pub fn is_empty(&self) -> bool {
        self.inner.patterns.is_empty()
    }

    /// The patterns, in the order given to [`RegexSet::new`].
    pub fn patterns(&self) -> &[String] {
        &self.inner.patterns
    }
}

impl fmt::Debug for RegexSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("RegexSet")
            .field(&self.inner.patterns)
            .finish()
    }
}

/// The result of [`RegexSet::matches`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetMatches {
    matched: Vec<bool>,
}

impl SetMatches {
    /// Whether pattern `i` matched (`false` for out-of-range indices).
    pub fn matched(&self, i: usize) -> bool {
        self.matched.get(i).copied().unwrap_or(false)
    }

    /// Whether any pattern matched.
    pub fn matched_any(&self) -> bool {
        self.matched.iter().any(|&m| m)
    }

    /// Number of patterns in the set (matched or not).
    pub fn len(&self) -> usize {
        self.matched.len()
    }

    /// Whether the set had no patterns.
    pub fn is_empty(&self) -> bool {
        self.matched.is_empty()
    }

    /// Indices of the matching patterns, ascending.
    pub fn iter(&self) -> SetMatchesIter<'_> {
        SetMatchesIter {
            inner: self.matched.iter().enumerate(),
        }
    }
}

impl<'a> IntoIterator for &'a SetMatches {
    type Item = usize;
    type IntoIter = SetMatchesIter<'a>;

    fn into_iter(self) -> SetMatchesIter<'a> {
        self.iter()
    }
}

/// Iterator over the indices of matching patterns.
#[derive(Debug, Clone)]
pub struct SetMatchesIter<'a> {
    inner: std::iter::Enumerate<std::slice::Iter<'a, bool>>,
}

impl Iterator for SetMatchesIter<'_> {
    type Item = usize;

    fn next(&mut self) -> Option<usize> {
        self.inner.by_ref().find(|(_, &m)| m).map(|(i, _)| i)
    }
}

impl std::iter::FusedIterator for SetMatchesIter<'_> {}
