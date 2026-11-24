//! Character class sets: sorted, disjoint, non-adjacent ranges of Unicode
//! scalar values.
//!
//! Ranges never contain surrogate code points (`char` cannot represent them),
//! so complement and successor arithmetic skip the `D800..=DFFF` gap.

/// A canonical set of `char` ranges (inclusive on both ends).
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub(crate) struct ClassSet {
    ranges: Vec<(char, char)>,
}

/// The ASCII Perl classes supported by the parser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Perl {
    /// `\d` — `[0-9]`.
    Digit,
    /// `\w` — `[0-9A-Za-z_]`.
    Word,
    /// `\s` — `[\t\n\v\f\r ]`.
    Space,
}

/// Returns `true` for ASCII word characters (`[0-9A-Za-z_]`).
#[inline]
pub(crate) fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// The next scalar value after `c`, skipping the surrogate gap.
pub(crate) fn succ(c: char) -> Option<char> {
    match c {
        '\u{D7FF}' => Some('\u{E000}'),
        char::MAX => None,
        _ => char::from_u32(c as u32 + 1),
    }
}

/// The scalar value before `c`, skipping the surrogate gap.
pub(crate) fn pred(c: char) -> Option<char> {
    match c {
        '\u{E000}' => Some('\u{D7FF}'),
        '\0' => None,
        _ => char::from_u32(c as u32 - 1),
    }
}

impl ClassSet {
    /// An empty set.
    pub(crate) fn new() -> ClassSet {
        ClassSet { ranges: Vec::new() }
    }

    /// Every scalar value.
    pub(crate) fn all() -> ClassSet {
        ClassSet {
            ranges: vec![('\0', char::MAX)],
        }
    }

    /// Every scalar value except `\n` (the default meaning of `.`).
    pub(crate) fn all_except_newline() -> ClassSet {
        ClassSet {
            ranges: vec![('\0', '\t'), ('\u{0B}', char::MAX)],
        }
    }

    /// A single-character set.
    pub(crate) fn single(c: char) -> ClassSet {
        ClassSet {
            ranges: vec![(c, c)],
        }
    }

    /// The ASCII Perl class `kind`, optionally negated.
    pub(crate) fn perl(kind: Perl, negated: bool) -> ClassSet {
        let ranges = match kind {
            Perl::Digit => vec![('0', '9')],
            Perl::Word => vec![('0', '9'), ('A', 'Z'), ('_', '_'), ('a', 'z')],
            Perl::Space => vec![('\t', '\r'), (' ', ' ')],
        };
        let mut set = ClassSet { ranges };
        if negated {
            set.negate();
        }
        set
    }

    /// The canonical ranges.
    pub(crate) fn ranges(&self) -> &[(char, char)] {
        &self.ranges
    }

    /// Appends a range without re-canonicalising; call [`canonicalize`](Self::canonicalize) afterwards.
    pub(crate) fn push(&mut self, lo: char, hi: char) {
        debug_assert!(lo <= hi);
        self.ranges.push((lo, hi));
    }

    /// Adds every range of `other` and re-canonicalises.
    pub(crate) fn union(&mut self, other: &ClassSet) {
        if other.ranges.is_empty() {
            return;
        }
        self.ranges.extend_from_slice(&other.ranges);
        self.canonicalize();
    }

    /// Sorts and merges overlapping or adjacent ranges.
    pub(crate) fn canonicalize(&mut self) {
        if self.ranges.len() <= 1 {
            return;
        }
        self.ranges.sort_unstable();
        let mut out: Vec<(char, char)> = Vec::with_capacity(self.ranges.len());
        for &(lo, hi) in &self.ranges {
            if let Some(last) = out.last_mut() {
                let touches = match succ(last.1) {
                    Some(next) => lo <= next,
                    None => true,
                };
                if touches {
                    if hi > last.1 {
                        last.1 = hi;
                    }
                    continue;
                }
            }
            out.push((lo, hi));
        }
        self.ranges = out;
    }

    /// Replaces the set with its complement over all scalar values.
    pub(crate) fn negate(&mut self) {
        let mut out = Vec::with_capacity(self.ranges.len() + 1);
        let mut next = Some('\0');
        for &(lo, hi) in &self.ranges {
            if let Some(n) = next {
                if n < lo {
                    if let Some(p) = pred(lo) {
                        out.push((n, p));
                    }
                }
            }
            next = succ(hi);
        }
        if let Some(n) = next {
            out.push((n, char::MAX));
        }
        self.ranges = out;
    }

    /// Membership test (binary search; linear for tiny sets).
    #[inline]
    pub(crate) fn contains(&self, c: char) -> bool {
        if self.ranges.len() <= 4 {
            return self.ranges.iter().any(|&(lo, hi)| lo <= c && c <= hi);
        }
        self.ranges
            .binary_search_by(|&(lo, hi)| {
                if hi < c {
                    std::cmp::Ordering::Less
                } else if lo > c {
                    std::cmp::Ordering::Greater
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .is_ok()
    }

    /// The only member, if the set contains exactly one character.
    pub(crate) fn as_single(&self) -> Option<char> {
        match self.ranges.as_slice() {
            [(lo, hi)] if lo == hi => Some(*lo),
            _ => None,
        }
    }

    /// Approximate heap size in bytes.
    pub(crate) fn heap_bytes(&self) -> usize {
        self.ranges.len() * std::mem::size_of::<(char, char)>()
    }
}

#[cfg(test)]
mod unit {
    use super::*;

    #[test]
    fn negate_skips_surrogates_and_roundtrips() {
        let mut s = ClassSet::single('\u{D7FF}');
        s.push('\u{E000}', '\u{E000}');
        s.canonicalize();
        assert_eq!(s.ranges(), &[('\u{D7FF}', '\u{E000}')]);
        let mut n = s.clone();
        n.negate();
        assert!(!n.contains('\u{D7FF}'));
        assert!(n.contains('\u{D7FE}'));
        n.negate();
        assert_eq!(n, s);
        let mut all = ClassSet::all();
        all.negate();
        assert!(all.ranges().is_empty());
    }
}
