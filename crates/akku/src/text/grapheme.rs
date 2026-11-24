//! Unicode UAX #29 Extended Grapheme Cluster iteration and truncation.

use std::borrow::Cow;
use std::iter::FusedIterator;

use crate::text::tables::{
    grapheme_break_property, indic_conjunct_break, is_extended_pictographic, Gbp, Incb,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IncbState {
    None,
    Consonant,
    AfterLinker,
}

/// Iterator over Extended Grapheme Clusters (`&str`) in a UTF-8 string (UAX #29).
#[derive(Debug, Clone)]
pub struct Graphemes<'a> {
    rest: &'a str,
}

impl<'a> Graphemes<'a> {
    /// Create a grapheme cluster iterator over `input`.
    pub fn new(input: &'a str) -> Self {
        Self { rest: input }
    }

    /// Remaining unconsumed tail of the input string.
    pub fn as_str(&self) -> &'a str {
        self.rest
    }
}

impl<'a> Iterator for Graphemes<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        let mut chars = self.rest.char_indices();
        let (_, first) = chars.next()?;

        let mut prev_gbp = grapheme_break_property(first);
        let mut ri_odd = prev_gbp == Gbp::RegionalIndicator;
        let mut saw_extpic = is_extended_pictographic(first);
        let mut zwj_after_extpic = false;
        let mut incb = match indic_conjunct_break(first) {
            Incb::Consonant => IncbState::Consonant,
            _ => IncbState::None,
        };

        let mut end = first.len_utf8();

        for (idx, ch) in chars {
            let next_gbp = grapheme_break_property(ch);
            let next_extpic = is_extended_pictographic(ch);
            let next_incb = indic_conjunct_break(ch);

            let should_join = should_join(
                prev_gbp,
                next_gbp,
                ri_odd,
                zwj_after_extpic,
                next_extpic,
                incb,
                next_incb,
            );

            if !should_join {
                let cluster = &self.rest[..idx];
                self.rest = &self.rest[idx..];
                return Some(cluster);
            }

            // Update state inside the continuing cluster.
            if next_gbp == Gbp::RegionalIndicator {
                ri_odd = !ri_odd;
            } else {
                ri_odd = false;
            }

            if next_gbp == Gbp::Zwj && saw_extpic {
                zwj_after_extpic = true;
            } else if next_gbp == Gbp::Extend {
                // saw_extpic preserved; zwj_after_extpic resets unless already consumed.
                zwj_after_extpic = false;
            } else if next_extpic && zwj_after_extpic {
                saw_extpic = true;
                zwj_after_extpic = false;
            } else {
                saw_extpic = next_extpic;
                zwj_after_extpic = false;
            }

            incb = match (incb, next_incb) {
                (_, Incb::Consonant) => IncbState::Consonant,
                (IncbState::Consonant | IncbState::AfterLinker, Incb::Linker) => {
                    IncbState::AfterLinker
                }
                (state @ (IncbState::Consonant | IncbState::AfterLinker), Incb::Extend) => state,
                _ => IncbState::None,
            };

            prev_gbp = next_gbp;
            end = idx + ch.len_utf8();
        }

        let cluster = &self.rest[..end];
        self.rest = &self.rest[end..];
        Some(cluster)
    }
}

impl FusedIterator for Graphemes<'_> {}

#[inline]
fn should_join(
    prev: Gbp,
    next: Gbp,
    ri_odd: bool,
    zwj_after_extpic: bool,
    next_extpic: bool,
    incb: IncbState,
    next_incb: Incb,
) -> bool {
    // GB3: CR × LF
    if prev == Gbp::CR && next == Gbp::LF {
        return true;
    }
    // GB4: (Control | CR | LF) ÷
    if matches!(prev, Gbp::Control | Gbp::CR | Gbp::LF) {
        return false;
    }
    // GB5: ÷ (Control | CR | LF)
    if matches!(next, Gbp::Control | Gbp::CR | Gbp::LF) {
        return false;
    }
    // GB6: L × (L | V | LV | LVT)
    if prev == Gbp::L && matches!(next, Gbp::L | Gbp::V | Gbp::LV | Gbp::Lvt) {
        return true;
    }
    // GB7: (LV | V) × (V | T)
    if matches!(prev, Gbp::LV | Gbp::V) && matches!(next, Gbp::V | Gbp::T) {
        return true;
    }
    // GB8: (LVT | T) × T
    if matches!(prev, Gbp::Lvt | Gbp::T) && next == Gbp::T {
        return true;
    }
    // GB9: × (Extend | ZWJ)
    if matches!(next, Gbp::Extend | Gbp::Zwj) {
        return true;
    }
    // GB9a: × SpacingMark
    if next == Gbp::SpacingMark {
        return true;
    }
    // GB9b: Prepend ×
    if prev == Gbp::Prepend {
        return true;
    }
    // GB9c: Indic Conjunct Break
    if incb == IncbState::AfterLinker && next_incb == Incb::Consonant {
        return true;
    }
    // GB11: ExtPic Extend* ZWJ × ExtPic
    if zwj_after_extpic && next_extpic {
        return true;
    }
    // GB12/13: sot (RI RI)* RI × RI
    if prev == Gbp::RegionalIndicator && next == Gbp::RegionalIndicator && ri_odd {
        return true;
    }
    // GB999: Any ÷ Any
    false
}

/// Iterator yielding `(byte_offset, grapheme_slice)` pairs.
#[derive(Debug, Clone)]
pub struct GraphemeIndices<'a> {
    inner: Graphemes<'a>,
    offset: usize,
}

impl<'a> Iterator for GraphemeIndices<'a> {
    type Item = (usize, &'a str);

    fn next(&mut self) -> Option<Self::Item> {
        let cluster = self.inner.next()?;
        let start = self.offset;
        self.offset += cluster.len();
        Some((start, cluster))
    }
}

impl FusedIterator for GraphemeIndices<'_> {}

/// Iterate over UAX #29 Extended Grapheme Clusters in `input`.
pub fn graphemes(input: &str) -> Graphemes<'_> {
    Graphemes::new(input)
}

/// Iterate over `(byte_offset, grapheme_cluster)` pairs in `input`.
pub fn grapheme_indices(input: &str) -> GraphemeIndices<'_> {
    GraphemeIndices {
        inner: Graphemes::new(input),
        offset: 0,
    }
}

/// Count the number of UAX #29 Extended Grapheme Clusters in `input`.
pub fn grapheme_count(input: &str) -> usize {
    if input.is_ascii() && !input.contains('\r') {
        return input.len();
    }
    graphemes(input).count()
}

/// Truncate `input` to at most `max_graphemes` grapheme clusters.
///
/// If `input` has more than `max_graphemes` clusters, the result is truncated and
/// `ellipsis` is appended so that the final string has at most `max_graphemes`
/// clusters. If `max_graphemes` is smaller than `grapheme_count(ellipsis)`,
/// `ellipsis` itself is truncated to `max_graphemes` clusters.
///
/// Returns [`Cow::Borrowed`] when no truncation occurs.
pub fn truncate_graphemes<'a>(
    input: &'a str,
    max_graphemes: usize,
    ellipsis: &str,
) -> Cow<'a, str> {
    if max_graphemes == 0 {
        return Cow::Borrowed("");
    }

    let ellipsis_count = grapheme_count(ellipsis);
    if ellipsis_count >= max_graphemes {
        // Only check if input fits as-is without truncating.
        if grapheme_count(input) <= max_graphemes {
            return Cow::Borrowed(input);
        }
        let mut out = String::new();
        for cluster in graphemes(ellipsis).take(max_graphemes) {
            out.push_str(cluster);
        }
        return Cow::Owned(out);
    }

    let keep_count = max_graphemes - ellipsis_count;
    let mut keep_byte_end = 0usize;
    let mut total = 0usize;

    for (idx, cluster) in grapheme_indices(input) {
        if total < keep_count {
            keep_byte_end = idx + cluster.len();
        }
        total += 1;
        if total > max_graphemes {
            let mut out = String::with_capacity(keep_byte_end + ellipsis.len());
            out.push_str(&input[..keep_byte_end]);
            out.push_str(ellipsis);
            return Cow::Owned(out);
        }
    }

    Cow::Borrowed(input)
}
