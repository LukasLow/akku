//! Grapheme-aware and display-column-aware word wrapping (`wrap`, `fill`).

use crate::text::grapheme::graphemes;
use crate::text::tables::{
    char_display_width, grapheme_break_property, is_extended_pictographic, Gbp,
};

/// How `wrap` and `fill` measure character/cluster widths.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum WidthMode {
    /// Count each UAX #29 Extended Grapheme Cluster as width `1`.
    #[default]
    Graphemes,
    /// Approximate terminal display width (`0` for combining marks/controls, `2` for CJK wide & emoji, `1` otherwise).
    DisplayColumns,
}

/// Configuration for [`wrap_with`] and [`fill_with`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrapOptions<'a> {
    /// Target maximum line width (including indentation).
    pub width: usize,
    /// Split individual words that exceed the available line width at grapheme boundaries. Default: `true`.
    pub break_words: bool,
    /// Prefix prepended to the first output line. Default: `""`.
    pub initial_indent: &'a str,
    /// Prefix prepended to all subsequent output lines. Default: `""`.
    pub subsequent_indent: &'a str,
    /// Width measurement mode. Default: [`WidthMode::Graphemes`].
    pub width_mode: WidthMode,
}

impl<'a> WrapOptions<'a> {
    /// Create default wrapping options for `width`.
    pub const fn new(width: usize) -> Self {
        Self {
            width,
            break_words: true,
            initial_indent: "",
            subsequent_indent: "",
            width_mode: WidthMode::Graphemes,
        }
    }
}

/// Measure the width of a single grapheme cluster under `mode`.
#[inline]
pub(crate) fn cluster_width(cluster: &str, mode: WidthMode) -> usize {
    match mode {
        WidthMode::Graphemes => 1,
        WidthMode::DisplayColumns => {
            if cluster.chars().any(|c| {
                is_extended_pictographic(c) || grapheme_break_property(c) == Gbp::RegionalIndicator
            }) {
                2
            } else {
                cluster.chars().map(char_display_width).sum()
            }
        }
    }
}

/// Measure the total width of `s` under `mode`.
#[inline]
pub(crate) fn str_width(s: &str, mode: WidthMode) -> usize {
    graphemes(s).map(|g| cluster_width(g, mode)).sum()
}

/// Wrap `input` to `width` grapheme clusters per line, returning one `String` per line.
pub fn wrap(input: &str, width: usize) -> Vec<String> {
    wrap_with(input, &WrapOptions::new(width))
}

/// Wrap `input` using custom [`WrapOptions`].
pub fn wrap_with(input: &str, options: &WrapOptions<'_>) -> Vec<String> {
    if input.is_empty() {
        return Vec::new();
    }

    let normalized = if input.contains('\r') {
        std::borrow::Cow::Owned(input.replace("\r\n", "\n").replace('\r', "\n"))
    } else {
        std::borrow::Cow::Borrowed(input)
    };

    let init_w = str_width(options.initial_indent, options.width_mode);
    let sub_w = str_width(options.subsequent_indent, options.width_mode);

    let mut lines: Vec<String> = Vec::new();
    let mut is_first_line = true;

    for para in normalized.split('\n') {
        let words: Vec<&str> = para.split_whitespace().collect();
        if words.is_empty() {
            lines.push(String::new());
            is_first_line = false;
            continue;
        }

        let mut cur = String::new();
        let mut cur_w = 0usize;
        let mut has_word = false;

        let indent_for = |first: bool| -> (&str, usize) {
            if first {
                (options.initial_indent, options.width.saturating_sub(init_w))
            } else {
                (
                    options.subsequent_indent,
                    options.width.saturating_sub(sub_w),
                )
            }
        };

        let (mut cur_indent, mut avail_w) = indent_for(is_first_line);
        cur.push_str(cur_indent);

        for word in words {
            let word_w = str_width(word, options.width_mode);

            if !has_word {
                if word_w <= avail_w || !options.break_words {
                    cur.push_str(word);
                    cur_w = word_w;
                    has_word = true;
                } else {
                    // Break overlong word across grapheme clusters.
                    for g in graphemes(word) {
                        let gw = cluster_width(g, options.width_mode);
                        if has_word && cur_w + gw > avail_w {
                            lines.push(cur);
                            is_first_line = false;
                            let next_meta = indent_for(false);
                            cur_indent = next_meta.0;
                            avail_w = next_meta.1;
                            cur = String::from(cur_indent);
                            cur_w = 0;
                        }
                        cur.push_str(g);
                        cur_w += gw;
                        has_word = true;
                    }
                }
            } else if cur_w + 1 + word_w <= avail_w {
                cur.push(' ');
                cur.push_str(word);
                cur_w += 1 + word_w;
            } else {
                lines.push(cur);
                is_first_line = false;
                let next_meta = indent_for(false);
                cur_indent = next_meta.0;
                avail_w = next_meta.1;
                cur = String::from(cur_indent);
                cur_w = 0;
                has_word = false;

                if word_w <= avail_w || !options.break_words {
                    cur.push_str(word);
                    cur_w = word_w;
                    has_word = true;
                } else {
                    for g in graphemes(word) {
                        let gw = cluster_width(g, options.width_mode);
                        if has_word && cur_w + gw > avail_w {
                            lines.push(cur);
                            let next_meta = indent_for(false);
                            cur_indent = next_meta.0;
                            avail_w = next_meta.1;
                            cur = String::from(cur_indent);
                            cur_w = 0;
                        }
                        cur.push_str(g);
                        cur_w += gw;
                        has_word = true;
                    }
                }
            }
        }

        if has_word {
            lines.push(cur);
            is_first_line = false;
        }
    }

    lines
}

/// Wrap `input` to `width` and join the resulting lines with `'\n'`.
pub fn fill(input: &str, width: usize) -> String {
    wrap(input, width).join("\n")
}

/// Wrap `input` with custom [`WrapOptions`] and join lines with `'\n'`.
pub fn fill_with(input: &str, options: &WrapOptions<'_>) -> String {
    wrap_with(input, options).join("\n")
}
