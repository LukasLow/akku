//! Linear-time string search and binary-search helpers.
//!
//! - [`kmp_find`] / [`kmp_find_all`]: single-pattern Knuth-Morris-Pratt
//!   search in `O(n + m)` over any `PartialEq` slice.
//! - [`AhoCorasick`]: multi-pattern byte search in `O(n + total pattern
//!   length + matches)`, independent of how many patterns there are or how
//!   they overlap.
//! - [`lower_bound`] / [`upper_bound`] / [`equal_range`]: sorted-slice
//!   bounds (plus `_by` variants for custom or floating-point orderings).
//!
//! All positions are element (byte, for `&str`) offsets.
//!
//! # Examples
//!
//! ```
//! use akku::algo::search::{kmp_find_all, AhoCorasick, Match};
//!
//! assert_eq!(kmp_find_all(b"abababa", b"aba"), [0, 2, 4]);
//!
//! let ac = AhoCorasick::new(["he", "she", "hers"]);
//! let found: Vec<Match> = ac.find_iter("ushers").collect();
//! assert_eq!(
//!     found,
//!     [
//!         Match { pattern: 1, start: 1, end: 4 }, // "she"
//!         Match { pattern: 0, start: 2, end: 4 }, // "he"
//!         Match { pattern: 2, start: 2, end: 6 }, // "hers"
//!     ]
//! );
//! ```

use std::cmp::Ordering;
use std::collections::VecDeque;
use std::ops::{ControlFlow, Range};

/// Computes the KMP failure function (longest proper border of each prefix).
fn failure_table<T: PartialEq>(pattern: &[T]) -> Vec<usize> {
    let mut table = vec![0usize; pattern.len()];
    let mut k = 0usize;
    for i in 1..pattern.len() {
        while k > 0 && pattern[i] != pattern[k] {
            k = table[k - 1];
        }
        if pattern[i] == pattern[k] {
            k += 1;
        }
        table[i] = k;
    }
    table
}

/// Runs KMP over `haystack`, calling `on_match(start)` for every occurrence.
fn kmp_scan<T: PartialEq>(
    haystack: &[T],
    needle: &[T],
    mut on_match: impl FnMut(usize) -> ControlFlow<()>,
) {
    if needle.is_empty() {
        for start in 0..=haystack.len() {
            if on_match(start).is_break() {
                return;
            }
        }
        return;
    }
    if needle.len() > haystack.len() {
        return;
    }
    let table = failure_table(needle);
    let mut k = 0usize;
    for (i, item) in haystack.iter().enumerate() {
        while k > 0 && *item != needle[k] {
            k = table[k - 1];
        }
        if *item == needle[k] {
            k += 1;
        }
        if k == needle.len() {
            if on_match(i + 1 - k).is_break() {
                return;
            }
            k = table[k - 1];
        }
    }
}

/// Returns the start of the first occurrence of `needle` in `haystack`.
///
/// An empty needle matches at `0`. Runs in `O(n + m)` worst case.
///
/// # Examples
///
/// ```
/// use akku::algo::search::kmp_find;
///
/// assert_eq!(kmp_find(b"hello world", b"world"), Some(6));
/// assert_eq!(kmp_find(&[1, 2, 3], &[4]), None);
/// ```
pub fn kmp_find<T: PartialEq>(haystack: &[T], needle: &[T]) -> Option<usize> {
    let mut found = None;
    kmp_scan(haystack, needle, |start| {
        found = Some(start);
        ControlFlow::Break(())
    });
    found
}

/// Returns the starts of all (possibly overlapping) occurrences of `needle`.
///
/// An empty needle matches at every position `0..=haystack.len()`, like
/// [`str::match_indices`]. Runs in `O(n + m)` worst case.
///
/// # Examples
///
/// ```
/// use akku::algo::search::kmp_find_all;
///
/// assert_eq!(kmp_find_all(b"aaaa", b"aa"), [0, 1, 2]);
/// ```
pub fn kmp_find_all<T: PartialEq>(haystack: &[T], needle: &[T]) -> Vec<usize> {
    let mut found = Vec::new();
    kmp_scan(haystack, needle, |start| {
        found.push(start);
        ControlFlow::Continue(())
    });
    found
}

/// Sentinel "no state" link.
const NONE: usize = usize::MAX;
/// The root state of the automaton.
const ROOT: usize = 0;

/// One match reported by [`AhoCorasick`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Match {
    /// Index of the matched pattern (in construction order).
    pub pattern: usize,
    /// Byte offset where the match starts.
    pub start: usize,
    /// Byte offset just past the end of the match.
    pub end: usize,
}

impl Match {
    /// The matched byte range `start..end`.
    pub fn range(&self) -> Range<usize> {
        self.start..self.end
    }
}

#[derive(Debug, Clone, Default)]
struct State {
    /// Sorted sparse trie transitions `(byte, state)`.
    next: Vec<(u8, usize)>,
    /// Failure link: longest proper suffix that is also a trie path.
    fail: usize,
    /// Dictionary link: nearest state on the failure chain with patterns.
    dict: usize,
    /// Patterns ending exactly at this state.
    patterns: Vec<usize>,
}

impl State {
    fn goto(&self, byte: u8) -> Option<usize> {
        self.next
            .binary_search_by_key(&byte, |&(b, _)| b)
            .ok()
            .and_then(|i| self.next.get(i))
            .map(|&(_, s)| s)
    }
}

/// A multi-pattern byte-string matcher (Aho-Corasick automaton).
///
/// Construction is `O(total pattern length · log σ)`; searching is
/// `O(n + matches)` regardless of the number of patterns, because each
/// haystack byte follows amortized-constant failure links and each reported
/// match follows one dictionary link. Overlapping matches are all reported.
///
/// # Examples
///
/// ```
/// use akku::algo::search::AhoCorasick;
///
/// let ac = AhoCorasick::new(["cat", "at", "t"]);
/// let ends: Vec<_> = ac.find_iter("cat").map(|m| (m.pattern, m.range())).collect();
/// assert_eq!(ends, [(0, 0..3), (1, 1..3), (2, 2..3)]);
/// assert!(ac.is_match("bat"));
/// assert!(!ac.is_match("dog"));
/// ```
#[derive(Debug, Clone)]
pub struct AhoCorasick {
    states: Vec<State>,
    /// Dense transition table for the root (every byte resolves in O(1)).
    root: Vec<usize>,
    /// Length of every pattern, by pattern index.
    lengths: Vec<usize>,
}

impl AhoCorasick {
    /// Builds an automaton for `patterns`.
    ///
    /// Duplicate patterns are kept and reported under each index. An empty
    /// pattern matches at every position.
    pub fn new<I, P>(patterns: I) -> Self
    where
        I: IntoIterator<Item = P>,
        P: AsRef<[u8]>,
    {
        let mut states = vec![State {
            fail: ROOT,
            dict: NONE,
            ..State::default()
        }];
        let mut lengths = Vec::new();
        for (index, pattern) in patterns.into_iter().enumerate() {
            let pattern = pattern.as_ref();
            let mut state = ROOT;
            for &byte in pattern {
                state = match states[state].goto(byte) {
                    Some(next) => next,
                    None => {
                        let next = states.len();
                        states.push(State {
                            fail: ROOT,
                            dict: NONE,
                            ..State::default()
                        });
                        let slot = &mut states[state].next;
                        let at = slot.partition_point(|&(b, _)| b < byte);
                        slot.insert(at, (byte, next));
                        next
                    }
                };
            }
            states[state].patterns.push(index);
            lengths.push(pattern.len());
        }

        let mut root = vec![ROOT; 256];
        for &(byte, next) in &states[ROOT].next {
            root[usize::from(byte)] = next;
        }

        // Breadth-first: failure links of shallower states are final before
        // deeper states use them.
        let mut queue = VecDeque::from([ROOT]);
        while let Some(state) = queue.pop_front() {
            for i in 0..states[state].next.len() {
                let (byte, child) = states[state].next[i];
                let fail = if state == ROOT {
                    ROOT
                } else {
                    transition(&states, &root, states[state].fail, byte)
                };
                states[child].fail = fail;
                states[child].dict = if states[fail].patterns.is_empty() {
                    states[fail].dict
                } else {
                    fail
                };
                queue.push_back(child);
            }
        }
        Self {
            states,
            root,
            lengths,
        }
    }

    /// Returns the number of patterns.
    pub fn pattern_count(&self) -> usize {
        self.lengths.len()
    }

    /// Iterates over all (overlapping) matches in `haystack`.
    ///
    /// Matches are ordered by end offset; matches ending at the same offset
    /// are ordered longest first.
    pub fn find_iter<'a, 'h, H>(&'a self, haystack: &'h H) -> FindIter<'a, 'h>
    where
        H: AsRef<[u8]> + ?Sized,
    {
        let mut iter = FindIter {
            automaton: self,
            haystack: haystack.as_ref(),
            pos: 0,
            state: ROOT,
            output: NONE,
            output_index: 0,
        };
        iter.load_outputs();
        iter
    }

    /// Collects all (overlapping) matches in `haystack`.
    pub fn find_all<H: AsRef<[u8]> + ?Sized>(&self, haystack: &H) -> Vec<Match> {
        self.find_iter(haystack).collect()
    }

    /// Returns the match that ends earliest (longest on ties).
    pub fn find<H: AsRef<[u8]> + ?Sized>(&self, haystack: &H) -> Option<Match> {
        self.find_iter(haystack).next()
    }

    /// Returns `true` if any pattern occurs in `haystack`.
    pub fn is_match<H: AsRef<[u8]> + ?Sized>(&self, haystack: &H) -> bool {
        self.find(haystack).is_some()
    }
}

/// Follows goto/failure transitions from `state` on `byte`.
fn transition(states: &[State], root: &[usize], mut state: usize, byte: u8) -> usize {
    loop {
        if state == ROOT {
            return root.get(usize::from(byte)).copied().unwrap_or(ROOT);
        }
        let Some(current) = states.get(state) else {
            return ROOT;
        };
        if let Some(next) = current.goto(byte) {
            return next;
        }
        state = current.fail;
    }
}

/// Iterator over overlapping matches; see [`AhoCorasick::find_iter`].
#[derive(Debug, Clone)]
pub struct FindIter<'a, 'h> {
    automaton: &'a AhoCorasick,
    haystack: &'h [u8],
    pos: usize,
    state: usize,
    /// State whose patterns are currently being reported.
    output: usize,
    /// Next pattern to report within `output`.
    output_index: usize,
}

impl FindIter<'_, '_> {
    fn load_outputs(&mut self) {
        let states = &self.automaton.states;
        self.output = match states.get(self.state) {
            Some(s) if !s.patterns.is_empty() => self.state,
            Some(s) => s.dict,
            None => NONE,
        };
        self.output_index = 0;
    }
}

impl Iterator for FindIter<'_, '_> {
    type Item = Match;

    fn next(&mut self) -> Option<Match> {
        let automaton = self.automaton;
        loop {
            while let Some(state) = automaton.states.get(self.output) {
                if let Some(&pattern) = state.patterns.get(self.output_index) {
                    self.output_index += 1;
                    let len = automaton.lengths.get(pattern).copied().unwrap_or(0);
                    return Some(Match {
                        pattern,
                        start: self.pos.saturating_sub(len),
                        end: self.pos,
                    });
                }
                self.output = state.dict;
                self.output_index = 0;
            }
            let &byte = self.haystack.get(self.pos)?;
            self.pos += 1;
            self.state = transition(&automaton.states, &automaton.root, self.state, byte);
            self.load_outputs();
        }
    }
}

/// Returns the first index whose element is not less than `value`.
///
/// `slice` must be sorted ascending. Returns `slice.len()` if every element
/// is less than `value`.
///
/// # Examples
///
/// ```
/// use akku::algo::search::{lower_bound, upper_bound, equal_range};
///
/// let v = [1, 2, 2, 2, 5];
/// assert_eq!(lower_bound(&v, &2), 1);
/// assert_eq!(upper_bound(&v, &2), 4);
/// assert_eq!(equal_range(&v, &2), 1..4);
/// assert_eq!(lower_bound(&v, &9), 5);
/// ```
pub fn lower_bound<T: Ord>(slice: &[T], value: &T) -> usize {
    slice.partition_point(|x| x < value)
}

/// Returns the first index whose element is greater than `value`.
///
/// `slice` must be sorted ascending.
pub fn upper_bound<T: Ord>(slice: &[T], value: &T) -> usize {
    slice.partition_point(|x| x <= value)
}

/// Returns the index range of elements equal to `value`.
///
/// `slice` must be sorted ascending.
pub fn equal_range<T: Ord>(slice: &[T], value: &T) -> Range<usize> {
    lower_bound(slice, value)..upper_bound(slice, value)
}

/// [`lower_bound`] with a comparator: `compare(element)` returns the
/// ordering of `element` relative to the target.
///
/// # Examples
///
/// ```
/// use akku::algo::search::{lower_bound_by, upper_bound_by};
///
/// let v = [0.5_f64, 1.0, 1.0, 2.5];
/// assert_eq!(lower_bound_by(&v, |x| x.total_cmp(&1.0)), 1);
/// assert_eq!(upper_bound_by(&v, |x| x.total_cmp(&1.0)), 3);
/// ```
pub fn lower_bound_by<T, F>(slice: &[T], mut compare: F) -> usize
where
    F: FnMut(&T) -> Ordering,
{
    slice.partition_point(|x| compare(x) == Ordering::Less)
}

/// [`upper_bound`] with a comparator: `compare(element)` returns the
/// ordering of `element` relative to the target.
pub fn upper_bound_by<T, F>(slice: &[T], mut compare: F) -> usize
where
    F: FnMut(&T) -> Ordering,
{
    slice.partition_point(|x| compare(x) != Ordering::Greater)
}
