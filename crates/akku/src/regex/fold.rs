//! Simple case folding derived from the standard library's Unicode tables.
//!
//! On first use, a table of case "orbits" is built by scanning every scalar
//! value up to `U+1FFFF` and merging each character with its single-character
//! lowercase and uppercase mappings (union-find). This yields complete
//! equivalence classes such as `{k, K, U+212A KELVIN SIGN}` or `{σ, ς, Σ}`
//! without shipping any data tables. Multi-character mappings (`ß` ↔ `SS`)
//! are intentionally ignored.

use std::sync::OnceLock;

use super::class::ClassSet;

/// Highest scalar value scanned; all cased characters live below it.
const SCAN_END: u32 = 0x1_FFFF;

struct FoldTable {
    /// Every character that belongs to a non-trivial orbit, sorted, with its orbit index.
    entries: Vec<(char, u32)>,
    /// Orbit members.
    orbits: Vec<Vec<char>>,
}

fn single(mut it: impl Iterator<Item = char>) -> Option<char> {
    let first = it.next()?;
    if it.next().is_some() {
        None
    } else {
        Some(first)
    }
}

fn find(parent: &mut [u32], mut x: u32) -> u32 {
    while parent[x as usize] != x {
        let grand = parent[parent[x as usize] as usize];
        parent[x as usize] = grand;
        x = grand;
    }
    x
}

fn union(parent: &mut [u32], a: u32, b: u32) {
    let ra = find(parent, a);
    let rb = find(parent, b);
    if ra != rb {
        let (lo, hi) = if ra < rb { (ra, rb) } else { (rb, ra) };
        parent[hi as usize] = lo;
    }
}

fn build() -> FoldTable {
    let mut parent: Vec<u32> = (0..=SCAN_END).collect();
    for cp in 0..=SCAN_END {
        let Some(c) = char::from_u32(cp) else {
            continue;
        };
        for mapped in [single(c.to_lowercase()), single(c.to_uppercase())]
            .into_iter()
            .flatten()
        {
            let m = mapped as u32;
            if m != cp && m <= SCAN_END {
                union(&mut parent, cp, m);
            }
        }
    }
    let mut size = vec![0u32; parent.len()];
    for cp in 0..=SCAN_END {
        let r = find(&mut parent, cp);
        size[r as usize] += 1;
    }
    let mut orbit_of_root = std::collections::HashMap::new();
    let mut entries = Vec::new();
    let mut orbits: Vec<Vec<char>> = Vec::new();
    for cp in 0..=SCAN_END {
        let r = find(&mut parent, cp);
        if size[r as usize] < 2 {
            continue;
        }
        let Some(c) = char::from_u32(cp) else {
            continue;
        };
        let idx = *orbit_of_root.entry(r).or_insert_with(|| {
            orbits.push(Vec::new());
            (orbits.len() - 1) as u32
        });
        orbits[idx as usize].push(c);
        entries.push((c, idx));
    }
    FoldTable { entries, orbits }
}

fn table() -> &'static FoldTable {
    static TABLE: OnceLock<FoldTable> = OnceLock::new();
    TABLE.get_or_init(build)
}

/// Returns `set` closed under simple case folding.
pub(crate) fn fold_class(set: &ClassSet) -> ClassSet {
    let t = table();
    let mut out = set.clone();
    let mut added = false;
    for &(lo, hi) in set.ranges() {
        let start = t.entries.partition_point(|e| e.0 < lo);
        for &(c, orbit) in &t.entries[start..] {
            if c > hi {
                break;
            }
            for &m in &t.orbits[orbit as usize] {
                if !set.contains(m) {
                    out.push(m, m);
                    added = true;
                }
            }
        }
    }
    if added {
        out.canonicalize();
    }
    out
}

/// Returns the case orbit of `c` as a class (a single char if it has no case).
pub(crate) fn fold_char(c: char) -> ClassSet {
    fold_class(&ClassSet::single(c))
}

#[cfg(test)]
mod unit {
    use super::*;

    #[test]
    fn orbits_include_inverse_only_mappings() {
        let k = fold_char('k');
        assert!(k.contains('K') && k.contains('\u{212A}'));
        let sigma = fold_char('σ');
        assert!(sigma.contains('Σ') && sigma.contains('ς'));
        assert_eq!(fold_char('1').as_single(), Some('1'));
    }
}
