//! Zero-dependency, **guaranteed linear-time** regular expressions.
//!
//! `akku::regex` compiles patterns into a Thompson NFA and runs them with a
//! PikeVM (for positions and captures) and a lazy DFA (for fast
//! yes/no answers). There is no backtracking anywhere, so a search over `n`
//! characters with a program of `m` instructions takes `O(n · m)` time for
//! *every* pattern — classic ReDoS patterns such as `(a+)+b`, `(a|a)*b` or
//! `(a?){30}a{30}` are as fast as any other.
//!
//! Compilation is protected as well: pattern length, nesting depth and the
//! compiled program size are all bounded (see [`RegexBuilder`]), and the
//! size check runs on an estimate *before* anything is emitted.
//!
//! # Semantics
//!
//! * Haystacks are `&str`; matching works on Unicode scalar values and every
//!   reported offset is a `char` boundary.
//! * Leftmost-first (Perl) priority: `a|ab` on `"ab"` matches `"a"`.
//! * Without `(?m)`, `^`/`$` match only at the very start/end of the text
//!   (`$` does **not** match before a trailing `\n`).
//! * Iterators never report an empty match that touches the end of the
//!   previous match, so `a*` over `"baaa"` yields `0..0` and `1..4`.
//!
//! # Syntax
//!
//! | Syntax | Meaning |
//! |---|---|
//! | `x` | a literal; escape metacharacters with `\` (see [`escape`]) |
//! | `.` | any char except `\n` (any char under `(?s)`) |
//! | `^` `$` | start/end of text; of a line under `(?m)` |
//! | `\A` `\z` | start/end of text, always |
//! | `\b` `\B` | ASCII word boundary / not a word boundary |
//! | `*` `+` `?` `{m}` `{m,}` `{m,n}` | greedy repetition (`m`, `n` ≤ 1000) |
//! | `*?` `+?` `??` `{m,n}?` | lazy repetition |
//! | `a\|b` | alternation |
//! | `(...)` `(?:...)` | capturing / non-capturing group |
//! | `(?P<name>...)` `(?<name>...)` | named group |
//! | `(?ims-ims)` `(?ims-ims:...)` | flags: `i` case-insensitive, `m` multi-line, `s` dot-all |
//! | `[a-z]` `[^...]` | character classes (`]` first is literal; `-` first/last is literal) |
//! | `\d` `\w` `\s` `\D` `\W` `\S` | ASCII digit, word, whitespace (and negations) |
//! | `\xHH` `\x{H..}` `\u{H..}` `\uHHHH` | code point escapes |
//! | `\n` `\t` `\r` `\f` `\v` `\a` `\e` | control characters |
//!
//! Case-insensitive matching uses simple case folding derived from the
//! standard library (`k` matches `K` and `U+212A KELVIN SIGN`); multi-char
//! folds such as `ß` ↔ `ss` are not applied.
//!
//! Backreferences, look-around, atomic groups, possessive quantifiers,
//! `\p{..}` classes and nested/POSIX classes are rejected with
//! [`ErrorKind::Unsupported`] rather than silently misread.
//!
//! # Examples
//!
//! ```
//! use akku::regex::Regex;
//!
//! let re = Regex::new(r"(?P<year>\d{4})-(?P<month>\d{2})").unwrap();
//! let caps = re.captures("released 2024-05, patched 2024-07").unwrap();
//! assert_eq!(&caps["year"], "2024");
//! assert_eq!(caps.get(2).unwrap().as_str(), "05");
//!
//! let dates: Vec<&str> = re.find_iter("2024-05 and 2024-07").map(|m| m.as_str()).collect();
//! assert_eq!(dates, ["2024-05", "2024-07"]);
//!
//! assert_eq!(re.replace_all("2024-05", "$month/$year"), "05/2024");
//! ```
//!
//! Catastrophic backtracking cannot happen:
//!
//! ```
//! use akku::regex::Regex;
//!
//! let re = Regex::new(r"(a+)+b").unwrap();
//! assert!(!re.is_match(&"a".repeat(100_000)));
//! ```

mod ast;
mod builder;
mod class;
mod dfa;
mod error;
mod fold;
mod hir;
mod nfa;
mod pikevm;
mod replace;
mod set;

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;
use std::iter::FusedIterator;
use std::ops::{Deref, DerefMut, Index, Range};
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use builder::Config;
use dfa::DfaCache;
use nfa::Program;
use pikevm::{PikeCache, NONE};

pub use builder::RegexBuilder;
pub use error::{Error, ErrorKind, Result, Span};
pub use replace::{NoExpand, Replacer};
pub use set::{RegexSet, SetMatches, SetMatchesIter};

// ---- cache pool --------------------------------------------------------------

/// Scratch memory for one search.
struct Cache {
    pike: PikeCache,
    dfa: Option<DfaCache>,
}

/// A small pool of caches so that `Regex` is `Sync` and searches don't allocate.
struct Pool {
    stack: Mutex<Vec<Cache>>,
    dfa_limit: usize,
}

impl Pool {
    fn new(dfa_limit: usize) -> Pool {
        Pool {
            stack: Mutex::new(Vec::new()),
            dfa_limit,
        }
    }

    fn get(&self, prog: &Program) -> PoolGuard<'_> {
        let cached = self
            .stack
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop();
        let cache = cached.unwrap_or_else(|| Cache {
            pike: PikeCache::new(),
            dfa: DfaCache::new(prog, self.dfa_limit),
        });
        PoolGuard {
            pool: self,
            cache: Some(cache),
        }
    }
}

struct PoolGuard<'a> {
    pool: &'a Pool,
    cache: Option<Cache>,
}

impl Deref for PoolGuard<'_> {
    type Target = Cache;

    fn deref(&self) -> &Cache {
        self.cache.as_ref().expect("cache present until drop")
    }
}

impl DerefMut for PoolGuard<'_> {
    fn deref_mut(&mut self) -> &mut Cache {
        self.cache.as_mut().expect("cache present until drop")
    }
}

impl Drop for PoolGuard<'_> {
    fn drop(&mut self) {
        if let Some(cache) = self.cache.take() {
            let mut stack = self
                .pool
                .stack
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if stack.len() < 8 {
                stack.push(cache);
            }
        }
    }
}

// ---- Regex -------------------------------------------------------------------

/// Group names, indexed by group number.
struct GroupNames {
    names: Vec<Option<Box<str>>>,
    index: HashMap<Box<str>, usize>,
}

struct Inner {
    pattern: String,
    prog: Program,
    names: Arc<GroupNames>,
    pool: Pool,
}

impl Inner {
    /// Finds the leftmost-first match at or after `start`, tracking `nslots`
    /// capture slots into the cache. With `gate`, the DFA rejects haystacks
    /// that cannot match before the PikeVM runs.
    fn search(
        &self,
        cache: &mut Cache,
        text: &str,
        start: usize,
        nslots: usize,
        gate: bool,
    ) -> bool {
        if gate {
            if let Some(d) = cache.dfa.as_mut() {
                if dfa::is_match(&self.prog, d, text, start) == Some(false) {
                    return false;
                }
            }
        }
        pikevm::search(
            &self.prog,
            &mut cache.pike,
            text,
            start,
            nslots,
            None,
            false,
        )
    }
}

/// A compiled regular expression.
///
/// `Regex` is cheap to clone (reference counted), `Send` and `Sync`; share
/// one instance across threads rather than recompiling.
///
/// # Examples
///
/// ```
/// use akku::regex::Regex;
///
/// let re = Regex::new(r"\b\w+@\w+\.com\b").unwrap();
/// assert_eq!(re.find("mail bob@example.com now").unwrap().as_str(), "bob@example.com");
/// ```
#[derive(Clone)]
pub struct Regex {
    inner: Arc<Inner>,
}

fn check_start(text: &str, start: usize) {
    assert!(
        text.is_char_boundary(start),
        "akku::regex: start offset {start} is out of bounds or not a char boundary (haystack length {})",
        text.len()
    );
}

impl Regex {
    /// Compiles `pattern` with default options.
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] with an [`ErrorKind`] and the byte [`Span`] of
    /// the problem for invalid syntax, unsupported features or exceeded
    /// limits.
    ///
    /// ```
    /// use akku::regex::{ErrorKind, Regex};
    ///
    /// let err = Regex::new("a(b").unwrap_err();
    /// assert_eq!(err.kind(), &ErrorKind::UnclosedGroup);
    /// assert_eq!(err.span().unwrap().start, 1);
    /// ```
    pub fn new(pattern: &str) -> Result<Regex> {
        Regex::from_config(pattern, &Config::default())
    }

    fn from_config(pattern: &str, config: &Config) -> Result<Regex> {
        let parsed = ast::parse(pattern, config)?;
        let slot_count = parsed.names.len() * 2;
        let prog = nfa::compile(&parsed.hir, slot_count, config.size_limit)
            .map_err(|kind| Error::new(kind, None, pattern))?;
        let index = parsed
            .names
            .iter()
            .enumerate()
            .filter_map(|(i, n)| n.clone().map(|n| (n, i)))
            .collect();
        let names = Arc::new(GroupNames {
            names: parsed.names,
            index,
        });
        Ok(Regex {
            inner: Arc::new(Inner {
                pattern: pattern.to_owned(),
                prog,
                names,
                pool: Pool::new(config.dfa_size_limit),
            }),
        })
    }

    /// The pattern this regex was compiled from.
    pub fn as_str(&self) -> &str {
        &self.inner.pattern
    }

    /// Number of capture groups, including the implicit group 0.
    pub fn captures_len(&self) -> usize {
        self.inner.names.names.len()
    }

    /// Group names by index (`None` for unnamed groups and group 0).
    ///
    /// ```
    /// use akku::regex::Regex;
    ///
    /// let re = Regex::new(r"(a)(?<b>b)").unwrap();
    /// let names: Vec<_> = re.capture_names().collect();
    /// assert_eq!(names, [None, None, Some("b")]);
    /// ```
    pub fn capture_names(&self) -> CaptureNames<'_> {
        CaptureNames {
            it: self.inner.names.names.iter(),
        }
    }

    /// Whether the regex matches anywhere in `text`.
    ///
    /// This is the fastest query: it runs the lazy DFA and stops at the
    /// first position where a match is certain.
    pub fn is_match(&self, text: &str) -> bool {
        self.is_match_at(text, 0)
    }

    /// Like [`is_match`](Self::is_match), but only considers matches that
    /// start at or after byte offset `start`. Look-behind assertions (`^`,
    /// `\b`) still see the text before `start`.
    ///
    /// # Panics
    ///
    /// If `start` is greater than `text.len()` or not on a char boundary.
    pub fn is_match_at(&self, text: &str, start: usize) -> bool {
        check_start(text, start);
        let inner = &*self.inner;
        let mut cache = inner.pool.get(&inner.prog);
        if let Some(d) = cache.dfa.as_mut() {
            if let Some(found) = dfa::is_match(&inner.prog, d, text, start) {
                return found;
            }
        }
        pikevm::search(&inner.prog, &mut cache.pike, text, start, 0, None, true)
    }

    /// The leftmost-first match in `text`.
    ///
    /// ```
    /// use akku::regex::Regex;
    ///
    /// let re = Regex::new(r"\d+").unwrap();
    /// let m = re.find("order #4521 shipped").unwrap();
    /// assert_eq!((m.start(), m.end(), m.as_str()), (7, 11, "4521"));
    /// ```
    pub fn find<'t>(&self, text: &'t str) -> Option<Match<'t>> {
        self.find_at(text, 0)
    }

    /// The leftmost-first match starting at or after byte offset `start`.
    ///
    /// # Panics
    ///
    /// If `start` is greater than `text.len()` or not on a char boundary.
    pub fn find_at<'t>(&self, text: &'t str, start: usize) -> Option<Match<'t>> {
        check_start(text, start);
        let inner = &*self.inner;
        let mut cache = inner.pool.get(&inner.prog);
        if !inner.search(&mut cache, text, start, 2, true) {
            return None;
        }
        let r = &cache.pike.result;
        Some(Match::new(text, r[0], r[1]))
    }

    /// An iterator over all non-overlapping matches.
    ///
    /// ```
    /// use akku::regex::Regex;
    ///
    /// let re = Regex::new(r"\w+").unwrap();
    /// let words: Vec<&str> = re.find_iter("one two  three").map(|m| m.as_str()).collect();
    /// assert_eq!(words, ["one", "two", "three"]);
    /// ```
    pub fn find_iter<'r, 't>(&'r self, text: &'t str) -> Matches<'r, 't> {
        Matches {
            it: Searcher::new(&self.inner, text),
        }
    }

    /// The capture groups of the leftmost-first match.
    ///
    /// ```
    /// use akku::regex::Regex;
    ///
    /// let re = Regex::new(r"(\w+)@(\w+)").unwrap();
    /// let caps = re.captures("to: ann@host").unwrap();
    /// assert_eq!(&caps[1], "ann");
    /// assert_eq!(&caps[2], "host");
    /// ```
    pub fn captures<'t>(&self, text: &'t str) -> Option<Captures<'t>> {
        self.captures_at(text, 0)
    }

    /// The capture groups of the leftmost-first match at or after `start`.
    ///
    /// # Panics
    ///
    /// If `start` is greater than `text.len()` or not on a char boundary.
    pub fn captures_at<'t>(&self, text: &'t str, start: usize) -> Option<Captures<'t>> {
        check_start(text, start);
        let inner = &*self.inner;
        let mut cache = inner.pool.get(&inner.prog);
        if !inner.search(&mut cache, text, start, inner.prog.slot_count, true) {
            return None;
        }
        Some(Captures {
            text,
            slots: cache.pike.result.clone(),
            names: Arc::clone(&inner.names),
        })
    }

    /// An iterator over the captures of all non-overlapping matches.
    pub fn captures_iter<'r, 't>(&'r self, text: &'t str) -> CaptureMatches<'r, 't> {
        CaptureMatches {
            it: Searcher::new(&self.inner, text),
        }
    }

    /// Splits `text` at every match; the pieces between matches are yielded,
    /// including empty pieces and the trailing piece.
    ///
    /// ```
    /// use akku::regex::Regex;
    ///
    /// let re = Regex::new(r"\s*,\s*").unwrap();
    /// let parts: Vec<&str> = re.split("a , b,c,").collect();
    /// assert_eq!(parts, ["a", "b", "c", ""]);
    /// ```
    pub fn split<'r, 't>(&'r self, text: &'t str) -> Split<'r, 't> {
        Split {
            finder: self.find_iter(text),
            last: 0,
            done: false,
        }
    }

    /// Like [`split`](Self::split), but yields at most `limit` pieces; the
    /// last piece holds the unsplit remainder. `limit == 0` yields nothing.
    pub fn splitn<'r, 't>(&'r self, text: &'t str, limit: usize) -> SplitN<'r, 't> {
        SplitN {
            split: self.split(text),
            remaining: limit,
        }
    }

    /// Replaces the first match. See [`Replacer`] for the `$` syntax.
    ///
    /// Returns [`Cow::Borrowed`] when nothing matched.
    pub fn replace<'t, R: Replacer>(&self, text: &'t str, rep: R) -> Cow<'t, str> {
        self.replacen(text, 1, rep)
    }

    /// Replaces every non-overlapping match. See [`Replacer`] for the `$` syntax.
    ///
    /// ```
    /// use akku::regex::Regex;
    ///
    /// let re = Regex::new(r"(?<k>\w+)=(?<v>\w+)").unwrap();
    /// assert_eq!(re.replace_all("a=1 b=2", "$v=$k"), "1=a 2=b");
    /// assert_eq!(re.replace_all("a=1", "${k}_x $$"), "a_x $");
    /// ```
    pub fn replace_all<'t, R: Replacer>(&self, text: &'t str, rep: R) -> Cow<'t, str> {
        self.replacen(text, 0, rep)
    }

    /// Replaces the first `limit` matches (`0` = all).
    pub fn replacen<'t, R: Replacer>(
        &self,
        text: &'t str,
        limit: usize,
        mut rep: R,
    ) -> Cow<'t, str> {
        if let Some(lit) = rep.no_expansion() {
            let mut out = String::new();
            let mut last = 0;
            let mut any = false;
            for (k, m) in self.find_iter(text).enumerate() {
                if limit != 0 && k >= limit {
                    break;
                }
                if !any {
                    out.reserve(text.len());
                    any = true;
                }
                out.push_str(&text[last..m.start()]);
                out.push_str(&lit);
                last = m.end();
            }
            if !any {
                return Cow::Borrowed(text);
            }
            out.push_str(&text[last..]);
            return Cow::Owned(out);
        }
        let mut out = String::new();
        let mut last = 0;
        let mut any = false;
        for (k, caps) in self.captures_iter(text).enumerate() {
            if limit != 0 && k >= limit {
                break;
            }
            if !any {
                out.reserve(text.len());
                any = true;
            }
            let m = caps.get_match();
            out.push_str(&text[last..m.start()]);
            rep.replace_append(&caps, &mut out);
            last = m.end();
        }
        if !any {
            return Cow::Borrowed(text);
        }
        out.push_str(&text[last..]);
        Cow::Owned(out)
    }
}

impl FromStr for Regex {
    type Err = Error;

    fn from_str(s: &str) -> Result<Regex> {
        Regex::new(s)
    }
}

impl fmt::Display for Regex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Debug for Regex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Regex").field(&self.as_str()).finish()
    }
}

/// Escapes every regex metacharacter in `text`, so that the result matches
/// `text` literally.
///
/// ```
/// use akku::regex::{escape, Regex};
///
/// assert_eq!(escape("1+1=2?"), r"1\+1=2\?");
/// let re = Regex::new(&escape("a.b*c")).unwrap();
/// assert!(re.is_match("xa.b*cx"));
/// assert!(!re.is_match("aXbbc"));
/// ```
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(
            c,
            '\\' | '.'
                | '+'
                | '*'
                | '?'
                | '('
                | ')'
                | '|'
                | '['
                | ']'
                | '{'
                | '}'
                | '^'
                | '$'
                | '#'
                | '&'
                | '-'
                | '~'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

// ---- Match & Captures ----------------------------------------------------------

/// A single match: a byte range of the haystack.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Match<'t> {
    text: &'t str,
    start: usize,
    end: usize,
}

impl<'t> Match<'t> {
    fn new(text: &'t str, start: usize, end: usize) -> Match<'t> {
        Match { text, start, end }
    }

    /// Byte offset where the match starts.
    pub fn start(&self) -> usize {
        self.start
    }

    /// Byte offset one past the end of the match.
    pub fn end(&self) -> usize {
        self.end
    }

    /// `start()..end()`.
    pub fn range(&self) -> Range<usize> {
        self.start..self.end
    }

    /// The matched text.
    pub fn as_str(&self) -> &'t str {
        &self.text[self.start..self.end]
    }

    /// Length of the match in bytes.
    pub fn len(&self) -> usize {
        self.end - self.start
    }

    /// Whether the match is empty.
    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }
}

impl fmt::Debug for Match<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Match")
            .field("start", &self.start)
            .field("end", &self.end)
            .field("text", &self.as_str())
            .finish()
    }
}

impl<'t> From<Match<'t>> for &'t str {
    fn from(m: Match<'t>) -> &'t str {
        m.as_str()
    }
}

impl From<Match<'_>> for Range<usize> {
    fn from(m: Match<'_>) -> Range<usize> {
        m.range()
    }
}

/// The capture groups of one match.
///
/// Group 0 is the whole match; groups that did not participate are `None`.
#[derive(Clone)]
pub struct Captures<'t> {
    text: &'t str,
    slots: Vec<usize>,
    names: Arc<GroupNames>,
}

impl<'t> Captures<'t> {
    /// Group `i`, if it exists and participated in the match.
    pub fn get(&self, i: usize) -> Option<Match<'t>> {
        let lo = i.checked_mul(2)?;
        let start = *self.slots.get(lo)?;
        let end = *self.slots.get(lo.checked_add(1)?)?;
        if start == NONE || end == NONE {
            return None;
        }
        Some(Match::new(self.text, start, end))
    }

    /// The group called `name`, if it exists and participated.
    pub fn name(&self, name: &str) -> Option<Match<'t>> {
        self.names.index.get(name).and_then(|&i| self.get(i))
    }

    /// The whole match (group 0).
    pub fn get_match(&self) -> Match<'t> {
        self.get(0).expect("group 0 always participates")
    }

    /// Number of groups, including group 0 (always ≥ 1).
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> usize {
        self.slots.len() / 2
    }

    /// Iterates over all groups in order.
    pub fn iter<'c>(&'c self) -> SubCaptureMatches<'c, 't> {
        SubCaptureMatches { caps: self, i: 0 }
    }

    /// Expands `replacement` (see [`Replacer`] for the `$` syntax) and
    /// appends the result to `dst`.
    ///
    /// ```
    /// use akku::regex::Regex;
    ///
    /// let re = Regex::new(r"(\w+) (\w+)").unwrap();
    /// let caps = re.captures("hello world").unwrap();
    /// let mut out = String::new();
    /// caps.expand("$2, ${1}!", &mut out);
    /// assert_eq!(out, "world, hello!");
    /// ```
    pub fn expand(&self, replacement: &str, dst: &mut String) {
        replace::expand(self, replacement, dst);
    }
}

impl fmt::Debug for Captures<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut list = f.debug_list();
        for (i, m) in self.iter().enumerate() {
            let name = self.names.names.get(i).and_then(|n| n.as_deref());
            list.entry(&(i, name, m.map(|m| m.as_str())));
        }
        list.finish()
    }
}

impl Index<usize> for Captures<'_> {
    type Output = str;

    /// # Panics
    ///
    /// If group `i` does not exist or did not participate.
    fn index(&self, i: usize) -> &str {
        match self.get(i) {
            Some(m) => m.as_str(),
            None => panic!("akku::regex: no match for capture group {i}"),
        }
    }
}

impl Index<&str> for Captures<'_> {
    type Output = str;

    /// # Panics
    ///
    /// If the named group does not exist or did not participate.
    fn index(&self, name: &str) -> &str {
        match self.name(name) {
            Some(m) => m.as_str(),
            None => panic!("akku::regex: no match for capture group `{name}`"),
        }
    }
}

// ---- iterators -----------------------------------------------------------------

/// Shared iteration state: implements the empty-match rule.
struct Searcher<'r, 't> {
    inner: &'r Inner,
    cache: PoolGuard<'r>,
    text: &'t str,
    at: usize,
    last_end: Option<usize>,
    done: bool,
    first: bool,
}

impl<'r, 't> Searcher<'r, 't> {
    fn new(inner: &'r Inner, text: &'t str) -> Searcher<'r, 't> {
        Searcher {
            inner,
            cache: inner.pool.get(&inner.prog),
            text,
            at: 0,
            last_end: None,
            done: false,
            first: true,
        }
    }

    /// Advances to the next reportable match; its slots are in the cache.
    fn advance(&mut self, nslots: usize) -> bool {
        while !self.done {
            let gate = std::mem::replace(&mut self.first, false);
            if !self
                .inner
                .search(&mut self.cache, self.text, self.at, nslots, gate)
            {
                break;
            }
            let (start, end) = {
                let r = &self.cache.pike.result;
                (r[0], r[1])
            };
            if start == end && self.last_end == Some(end) {
                match self.text[self.at..].chars().next() {
                    Some(c) => {
                        self.at += c.len_utf8();
                        continue;
                    }
                    None => break,
                }
            }
            self.at = end;
            self.last_end = Some(end);
            return true;
        }
        self.done = true;
        false
    }
}

/// Iterator over non-overlapping matches; see [`Regex::find_iter`].
pub struct Matches<'r, 't> {
    it: Searcher<'r, 't>,
}

impl<'t> Iterator for Matches<'_, 't> {
    type Item = Match<'t>;

    fn next(&mut self) -> Option<Match<'t>> {
        if !self.it.advance(2) {
            return None;
        }
        let r = &self.it.cache.pike.result;
        Some(Match::new(self.it.text, r[0], r[1]))
    }
}

impl FusedIterator for Matches<'_, '_> {}

impl fmt::Debug for Matches<'_, '_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Matches")
            .field("at", &self.it.at)
            .finish_non_exhaustive()
    }
}

/// Iterator over the captures of non-overlapping matches; see [`Regex::captures_iter`].
pub struct CaptureMatches<'r, 't> {
    it: Searcher<'r, 't>,
}

impl<'t> Iterator for CaptureMatches<'_, 't> {
    type Item = Captures<'t>;

    fn next(&mut self) -> Option<Captures<'t>> {
        let nslots = self.it.inner.prog.slot_count;
        if !self.it.advance(nslots) {
            return None;
        }
        Some(Captures {
            text: self.it.text,
            slots: self.it.cache.pike.result.clone(),
            names: Arc::clone(&self.it.inner.names),
        })
    }
}

impl FusedIterator for CaptureMatches<'_, '_> {}

impl fmt::Debug for CaptureMatches<'_, '_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CaptureMatches")
            .field("at", &self.it.at)
            .finish_non_exhaustive()
    }
}

/// Iterator over the pieces between matches; see [`Regex::split`].
pub struct Split<'r, 't> {
    finder: Matches<'r, 't>,
    last: usize,
    done: bool,
}

impl<'t> Iterator for Split<'_, 't> {
    type Item = &'t str;

    fn next(&mut self) -> Option<&'t str> {
        if self.done {
            return None;
        }
        let text = self.finder.it.text;
        match self.finder.next() {
            Some(m) => {
                let piece = &text[self.last..m.start()];
                self.last = m.end();
                Some(piece)
            }
            None => {
                self.done = true;
                Some(&text[self.last..])
            }
        }
    }
}

impl FusedIterator for Split<'_, '_> {}

impl fmt::Debug for Split<'_, '_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Split")
            .field("last", &self.last)
            .finish_non_exhaustive()
    }
}

/// Iterator over at most `limit` pieces; see [`Regex::splitn`].
pub struct SplitN<'r, 't> {
    split: Split<'r, 't>,
    remaining: usize,
}

impl<'t> Iterator for SplitN<'_, 't> {
    type Item = &'t str;

    fn next(&mut self) -> Option<&'t str> {
        if self.remaining == 0 {
            return None;
        }
        self.remaining -= 1;
        if self.remaining > 0 {
            return self.split.next();
        }
        if self.split.done {
            return None;
        }
        self.split.done = true;
        Some(&self.split.finder.it.text[self.split.last..])
    }
}

impl FusedIterator for SplitN<'_, '_> {}

impl fmt::Debug for SplitN<'_, '_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SplitN")
            .field("remaining", &self.remaining)
            .finish_non_exhaustive()
    }
}

/// Iterator over group names; see [`Regex::capture_names`].
#[derive(Debug, Clone)]
pub struct CaptureNames<'r> {
    it: std::slice::Iter<'r, Option<Box<str>>>,
}

impl<'r> Iterator for CaptureNames<'r> {
    type Item = Option<&'r str>;

    fn next(&mut self) -> Option<Option<&'r str>> {
        self.it.next().map(|n| n.as_deref())
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.it.size_hint()
    }
}

impl ExactSizeIterator for CaptureNames<'_> {}
impl FusedIterator for CaptureNames<'_> {}

/// Iterator over the groups of one match; see [`Captures::iter`].
#[derive(Debug, Clone)]
pub struct SubCaptureMatches<'c, 't> {
    caps: &'c Captures<'t>,
    i: usize,
}

impl<'t> Iterator for SubCaptureMatches<'_, 't> {
    type Item = Option<Match<'t>>;

    fn next(&mut self) -> Option<Option<Match<'t>>> {
        if self.i >= self.caps.len() {
            return None;
        }
        self.i += 1;
        Some(self.caps.get(self.i - 1))
    }
}

impl FusedIterator for SubCaptureMatches<'_, '_> {}
