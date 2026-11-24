//! Edit distance and similarity metrics (`levenshtein`, `normalized_similarity`).

use crate::text::grapheme::graphemes;

/// Compute the Levenshtein edit distance between `a` and `b` in Unicode scalar values.
///
/// Uses `O(min(m, n))` heap space.
pub fn levenshtein(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    levenshtein_slices(&a_chars, &b_chars)
}

/// Compute the Levenshtein edit distance between `a` and `b` in UAX #29 Extended Grapheme Clusters.
///
/// Uses `O(min(m, n))` heap space.
pub fn levenshtein_graphemes(a: &str, b: &str) -> usize {
    let a_clusters: Vec<&str> = graphemes(a).collect();
    let b_clusters: Vec<&str> = graphemes(b).collect();
    levenshtein_slices(&a_clusters, &b_clusters)
}

/// Normalized Levenshtein similarity in `[0.0, 1.0]` over Unicode scalar values.
///
/// Returns `1.0` when both strings are empty and `0.0` when they share no edit alignment
/// of shorter length than the longer string.
pub fn normalized_similarity(a: &str, b: &str) -> f64 {
    let len_a = a.chars().count();
    let len_b = b.chars().count();
    let max_len = len_a.max(len_b);
    if max_len == 0 {
        return 1.0;
    }
    let dist = levenshtein(a, b);
    1.0 - (dist as f64 / max_len as f64)
}

fn levenshtein_slices<T: PartialEq>(a: &[T], b: &[T]) -> usize {
    // Strip common prefix and suffix for O(1) fast path on near-identical strings.
    let mut start = 0;
    let min_len = a.len().min(b.len());
    while start < min_len && a[start] == b[start] {
        start += 1;
    }
    let mut a_end = a.len();
    let mut b_end = b.len();
    while a_end > start && b_end > start && a[a_end - 1] == b[b_end - 1] {
        a_end -= 1;
        b_end -= 1;
    }

    let a_mid = &a[start..a_end];
    let b_mid = &b[start..b_end];

    let (short, long) = if a_mid.len() <= b_mid.len() {
        (a_mid, b_mid)
    } else {
        (b_mid, a_mid)
    };

    if short.is_empty() {
        return long.len();
    }

    let mut prev: Vec<usize> = (0..=short.len()).collect();
    let mut curr = vec![0usize; short.len() + 1];

    for l_item in long {
        curr[0] = prev[0] + 1;
        for (j, s_item) in short.iter().enumerate() {
            let cost = usize::from(l_item != s_item);
            let del = prev[j + 1] + 1;
            let ins = curr[j] + 1;
            let sub = prev[j] + cost;
            curr[j + 1] = del.min(ins).min(sub);
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    prev[short.len()]
}
