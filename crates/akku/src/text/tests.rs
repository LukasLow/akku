//! Integration tests for `akku::text` and `akku::diff`.

use std::borrow::Cow;

use akku::diff as flat_diff;
use akku::text::diff::{
    apply_patch, diff_chars, diff_graphemes, diff_lines, diff_lines_with, diff_slices, hunks,
    unified_diff, unified_diff_with, DiffOp, DiffOptions,
};
use akku::text::{
    fill, fill_with, grapheme_count, grapheme_indices, graphemes, is_nfc, is_nfd, levenshtein,
    levenshtein_graphemes, nfc, nfd, normalize, normalized_similarity, slugify, split_words,
    to_camel_case, to_kebab_case, to_pascal_case, to_snake_case, truncate_graphemes, wrap,
    wrap_with, NormalizationForm, WidthMode, WrapOptions,
};

#[test]
fn graphemes_ascii_and_empty() {
    assert_eq!(graphemes("").collect::<Vec<_>>(), Vec::<&str>::new());
    assert_eq!(grapheme_count(""), 0);
    assert_eq!(graphemes("abc").collect::<Vec<_>>(), vec!["a", "b", "c"]);
    assert_eq!(
        grapheme_indices("héllo").collect::<Vec<_>>(),
        vec![(0, "h"), (1, "é"), (3, "l"), (4, "l"), (5, "o")]
    );
}

#[test]
fn graphemes_crlf_never_splits() {
    assert_eq!(
        graphemes("a\r\nb\r\n\n\r").collect::<Vec<_>>(),
        vec!["a", "\r\n", "b", "\r\n", "\n", "\r"]
    );
    assert_eq!(grapheme_count("a\r\nb"), 3);
}

#[test]
fn graphemes_combining_marks_and_zalgo() {
    assert_eq!(grapheme_count("e\u{0301}"), 1);
    assert_eq!(
        graphemes("e\u{0301}a\u{0300}\u{0323}\u{0302}x").collect::<Vec<_>>(),
        vec!["e\u{0301}", "a\u{0300}\u{0323}\u{0302}", "x"]
    );
}

#[test]
fn graphemes_hangul_jamo_composition() {
    // L + V + T stays one cluster; LV + T stays one cluster; two syllables split.
    let jamo = "\u{1100}\u{1161}\u{11A8}\u{AC00}\u{11A8}";
    assert_eq!(
        graphemes(jamo).collect::<Vec<_>>(),
        vec!["\u{1100}\u{1161}\u{11A8}", "\u{AC00}\u{11A8}"]
    );
}

#[test]
fn graphemes_zwj_emoji_sequences() {
    assert_eq!(grapheme_count("👨‍👩‍👧‍👦"), 1);
    assert_eq!(grapheme_count("👩‍💻"), 1);
    assert_eq!(grapheme_count("🏳️‍🌈"), 1);
    assert_eq!(grapheme_count("👍🏽"), 1);
    assert_eq!(
        graphemes("hi 👨‍👩‍👧‍👦👍🏽!").collect::<Vec<_>>(),
        vec!["h", "i", " ", "👨‍👩‍👧‍👦", "👍🏽", "!"]
    );
}

#[test]
fn graphemes_regional_indicator_flags() {
    assert_eq!(
        graphemes("🇩🇪🇺🇸🇯🇵").collect::<Vec<_>>(),
        vec!["🇩🇪", "🇺🇸", "🇯🇵"]
    );
    assert_eq!(graphemes("🇩🇪🇩").collect::<Vec<_>>(), vec!["🇩🇪", "🇩"]);
}

#[test]
fn graphemes_prepend_and_spacing_marks() {
    // Devanagari ka + vowel sign aa (SpacingMark) stays 1 cluster; Arabic prepend \u{0600} + '1' stays 1 cluster.
    assert_eq!(graphemes("का").collect::<Vec<_>>(), vec!["का"]);
    assert_eq!(
        graphemes("\u{0600}1").collect::<Vec<_>>(),
        vec!["\u{0600}1"]
    );
}

#[test]
fn truncate_graphemes_edge_cases() {
    assert!(matches!(
        truncate_graphemes("abc", 3, "…"),
        Cow::Borrowed("abc")
    ));
    assert_eq!(truncate_graphemes("👨‍👩‍👧‍👦🇩🇪!", 2, "…"), "👨‍👩‍👧‍👦…");
    assert_eq!(truncate_graphemes("hello world", 2, "..."), "..");
    assert_eq!(truncate_graphemes("hello", 0, "..."), "");
}

#[test]
fn wrap_and_fill_basic_and_paragraphs() {
    let wrapped = wrap("hello world from\n\nakku text", 10);
    assert_eq!(wrapped, vec!["hello", "world from", "", "akku text"]);
    assert_eq!(fill("one two\r\nthree four", 7), "one two\nthree\nfour");
}

#[test]
fn wrap_overlong_words_break_vs_keep() {
    let broken = wrap("👨‍👩‍👧‍👦👨‍👩‍👧‍👦👨‍👩‍👧‍👦", 2);
    assert_eq!(broken, vec!["👨‍👩‍👧‍👦👨‍👩‍👧‍👦", "👨‍👩‍👧‍👦"]);

    let kept = wrap_with(
        "superlongword ok",
        &WrapOptions {
            break_words: false,
            ..WrapOptions::new(5)
        },
    );
    assert_eq!(kept, vec!["superlongword", "ok"]);
}

#[test]
fn wrap_with_indents_and_display_columns() {
    let out = fill_with(
        "alpha beta gamma",
        &WrapOptions {
            width: 12,
            initial_indent: "* ",
            subsequent_indent: "  ",
            ..WrapOptions::new(12)
        },
    );
    assert_eq!(out, "* alpha beta\n  gamma");

    // "日本" = 4 display columns, "語" = 2 display columns.
    let cjk = wrap_with(
        "日本語 ok",
        &WrapOptions {
            width_mode: WidthMode::DisplayColumns,
            ..WrapOptions::new(4)
        },
    );
    assert_eq!(cjk, vec!["日本", "語", "ok"]);
}

#[test]
fn case_converters_all_styles() {
    assert_eq!(split_words("HTTPServer"), vec!["HTTP", "Server"]);
    assert_eq!(split_words("getV2Response"), vec!["get", "V2", "Response"]);
    assert_eq!(to_snake_case("HTTPServerV2"), "http_server_v2");
    assert_eq!(to_kebab_case("user_id_42"), "user-id-42");
    assert_eq!(to_camel_case("__foo--bar.baz  qux"), "fooBarBazQux");
    assert_eq!(to_pascal_case("hello_world"), "HelloWorld");
    assert_eq!(to_snake_case(""), "");
}

#[test]
fn slugify_unicode_diacritics_and_punctuation() {
    assert_eq!(
        slugify("  Crème Brûlée & Straße — № 42!  "),
        "creme-brulee-strasse-42"
    );
    assert_eq!(slugify("Æsir Øresund Łukasz"), "aesir-oresund-lukasz");
    assert_eq!(slugify("!!!"), "");
}

#[test]
fn levenshtein_and_grapheme_distance() {
    assert_eq!(levenshtein("kitten", "sitting"), 3);
    assert_eq!(levenshtein("", "abc"), 3);
    assert_eq!(levenshtein("same", "same"), 0);
    assert_eq!(levenshtein_graphemes("👨‍👩‍👧‍👦!", "👨!"), 1);
    assert_eq!(levenshtein_graphemes("🇩🇪🇺🇸", "🇩🇪🇯🇵"), 1);
}

#[test]
fn normalized_similarity_bounds() {
    assert!((normalized_similarity("", "") - 1.0).abs() < 1e-12);
    assert!((normalized_similarity("abc", "") - 0.0).abs() < 1e-12);
    let s1 = normalized_similarity("akku", "akku!");
    let s2 = normalized_similarity("akku!", "akku");
    assert!((s1 - s2).abs() < 1e-12);
    assert!((0.0..=1.0).contains(&s1));
}

#[test]
fn nfd_and_nfc_round_trip_latin_and_hangul() {
    assert_eq!(nfd("é"), "e\u{0301}");
    assert_eq!(nfc("e\u{0301}"), "é");
    assert_eq!(
        nfd("한글"),
        "\u{1112}\u{1161}\u{11AB}\u{1100}\u{1173}\u{11AF}"
    );
    assert_eq!(
        nfc("\u{1112}\u{1161}\u{11AB}\u{1100}\u{1173}\u{11AF}"),
        "한글"
    );

    // Canonical ordering of combining marks with distinct ccc (220 vs 230).
    let a = "a\u{0302}\u{0323}";
    let b = "a\u{0323}\u{0302}";
    assert_eq!(nfd(a), nfd(b));
    assert_eq!(nfc(a), nfc(b));
}

#[test]
fn nfc_composition_exclusions_and_is_checks() {
    // U+212B ANGSTROM SIGN -> U+00C5 LATIN CAPITAL LETTER A WITH RING ABOVE
    assert_eq!(nfc("\u{212B}"), "\u{00C5}");
    assert!(!is_nfc("\u{212B}"));
    assert!(is_nfc("ascii only"));
    assert!(is_nfd("ascii only"));
    assert!(matches!(
        normalize("plain ascii", NormalizationForm::Nfc),
        Cow::Borrowed("plain ascii")
    ));
}

#[test]
fn diff_lines_empty_and_identical() {
    assert_eq!(diff_lines("", ""), Vec::<DiffOp<&str>>::new());
    assert_eq!(
        diff_lines("a\nb\n", "a\nb\n"),
        vec![DiffOp::Equal("a"), DiffOp::Equal("b")]
    );
    assert_eq!(
        diff_lines("", "x\ny\n"),
        vec![DiffOp::Insert("x"), DiffOp::Insert("y")]
    );
}

#[test]
fn diff_lines_insert_delete_modify() {
    let ops = diff_lines("a\nb\nc\nd\n", "a\nx\nc\nd\ne\n");
    assert_eq!(
        ops,
        vec![
            DiffOp::Equal("a"),
            DiffOp::Delete("b"),
            DiffOp::Insert("x"),
            DiffOp::Equal("c"),
            DiffOp::Equal("d"),
            DiffOp::Insert("e"),
        ]
    );
}

#[test]
fn diff_chars_unicode_and_emoji() {
    let ops = diff_chars("café 🚀", "cafe 🎉");
    assert_eq!(
        ops,
        vec![
            DiffOp::Equal('c'),
            DiffOp::Equal('a'),
            DiffOp::Equal('f'),
            DiffOp::Delete('é'),
            DiffOp::Insert('e'),
            DiffOp::Equal(' '),
            DiffOp::Delete('🚀'),
            DiffOp::Insert('🎉'),
        ]
    );
}

#[test]
fn diff_graphemes_zwj_integrity() {
    let ops = diff_graphemes("a👨‍👩‍👧‍👦b", "a👩‍💻b");
    assert_eq!(
        ops,
        vec![
            DiffOp::Equal("a"),
            DiffOp::Delete("👨‍👩‍👧‍👦"),
            DiffOp::Insert("👩‍💻"),
            DiffOp::Equal("b"),
        ]
    );
}

#[test]
fn diff_bounded_huge_input_no_oom_or_hang() {
    let mut old = String::new();
    let mut new = String::new();
    for i in 0..25_000 {
        old.push_str(&format!("old-{i}\n"));
        new.push_str(&format!("new-{i}\n"));
    }
    let ops = diff_lines_with(
        &old,
        &new,
        DiffOptions {
            max_edit_distance: Some(256),
            max_steps: Some(50_000),
        },
    );
    let reconstructed_new: Vec<&str> = ops
        .iter()
        .filter_map(|op| match *op {
            DiffOp::Equal(l) | DiffOp::Insert(l) => Some(l),
            DiffOp::Delete(_) => None,
        })
        .collect();
    assert_eq!(reconstructed_new.len(), 25_000);
    assert_eq!(reconstructed_new[0], "new-0");
    assert_eq!(reconstructed_new[24_999], "new-24999");
}

#[test]
fn diff_common_prefix_suffix_stripped_before_budget() {
    let mut old = String::new();
    let mut new = String::new();
    for i in 0..20_000 {
        let line = format!("prefix-{i}\n");
        old.push_str(&line);
        new.push_str(&line);
    }
    old.push_str("alpha\nbeta\n");
    new.push_str("ALPHA\nBETA\n");
    for i in 0..20_000 {
        let line = format!("suffix-{i}\n");
        old.push_str(&line);
        new.push_str(&line);
    }

    let ops = diff_lines_with(
        &old,
        &new,
        DiffOptions {
            max_edit_distance: Some(8),
            max_steps: Some(1_000),
        },
    );
    let deletes: Vec<&str> = ops
        .iter()
        .filter_map(|op| match *op {
            DiffOp::Delete(l) => Some(l),
            _ => None,
        })
        .collect();
    let inserts: Vec<&str> = ops
        .iter()
        .filter_map(|op| match *op {
            DiffOp::Insert(l) => Some(l),
            _ => None,
        })
        .collect();
    assert_eq!(deletes, vec!["alpha", "beta"]);
    assert_eq!(inserts, vec!["ALPHA", "BETA"]);
}

#[test]
fn unified_diff_formatting_and_context_merging() {
    let old = "1\n2\n3\n4\n5\n6\n7\n";
    let new = "1\nTWO\n3\n4\nFIVE\n6\n7\n";
    let ops = diff_lines(old, new);
    // Changes at lines 2 and 5 are 2 lines apart -> merged with context=1 (2*1 >= 2).
    assert_eq!(hunks(&ops, 1).len(), 1);
    assert_eq!(hunks(&ops, 0).len(), 2);

    let single = unified_diff("a\n", "b\n", "a/f", "b/f", 3);
    assert_eq!(single, "--- a/f\n+++ b/f\n@@ -1 +1 @@\n-a\n+b\n");

    let from_empty = unified_diff("", "x\ny\n", "a/f", "b/f", 3);
    assert_eq!(from_empty, "--- a/f\n+++ b/f\n@@ -0,0 +1,2 @@\n+x\n+y\n");
}

#[test]
fn unified_diff_identical_returns_empty() {
    assert_eq!(unified_diff("same\n", "same\n", "a", "b", 3), "");
}

#[test]
fn unified_diff_no_trailing_newline_marker() {
    let patch = unified_diff("a\nb", "a\nb\n", "a/f", "b/f", 1);
    assert!(patch.contains("\\ No newline at end of file"));
    assert_eq!(apply_patch("a\nb", &patch).unwrap(), "a\nb\n");

    let patch2 = unified_diff("a\nb\n", "a\nb", "a/f", "b/f", 1);
    assert!(patch2.contains("\\ No newline at end of file"));
    assert_eq!(apply_patch("a\nb\n", &patch2).unwrap(), "a\nb");
}

#[test]
fn apply_patch_round_trip_suite() {
    let cases = [
        ("", "hello\nworld\n"),
        ("hello\nworld\n", ""),
        ("a\nb\nc\nd\ne\nf\ng\nh\n", "a\nB\nc\nd\ne\nf\nG\nh\n"),
        ("α\nβ\nγ", "α\nΒ\nγ\nδ"),
        ("only line", "changed line"),
    ];
    for (old, new) in cases {
        let patch = unified_diff(old, new, "old", "new", 2);
        let applied = apply_patch(old, &patch).unwrap();
        assert_eq!(applied, new, "round-trip failed for patch:\n{patch}");
    }
}

#[test]
fn apply_patch_rejects_mismatch_and_malformed() {
    let err_hdr = apply_patch("a\n", "--- a\n+++ b\nbad header\n").unwrap_err();
    assert!(err_hdr.message.contains("hunk header"));

    let patch = unified_diff("a\nb\nc\n", "a\nB\nc\n", "a", "b", 1);
    let err_ctx = apply_patch("a\nX\nc\n", &patch).unwrap_err();
    assert!(err_ctx.message.contains("mismatch"));
}

#[test]
fn akku_diff_reexport_equivalence() {
    let old = "one\ntwo\n";
    let new = "one\nTWO\n";
    let p1 = unified_diff(old, new, "a", "b", 1);
    let p2 = flat_diff::unified_diff(old, new, "a", "b", 1);
    assert_eq!(p1, p2);
    assert_eq!(flat_diff::apply_patch(old, &p2).unwrap(), new);
    assert_eq!(flat_diff::diff_lines(old, new), diff_lines(old, new));
}

#[test]
fn utf8_and_crlf_diff_preserves_line_endings() {
    let ops = diff_lines("a\r\nb\r\n", "a\nb\r\n");
    assert_eq!(
        ops,
        vec![
            DiffOp::Delete("a\r"),
            DiffOp::Insert("a"),
            DiffOp::Equal("b\r"),
        ]
    );
}

// ---------------------------------------------------------------------------
// B6c regressions
// ---------------------------------------------------------------------------

/// Naive reference for canonical ordering on starter `q` + marks with known classes
/// (the previous O(n^2) insertion-sort behavior).
fn ccc_ref(c: char) -> u8 {
    match c {
        '\u{0334}' => 1,
        '\u{0327}' => 202,
        '\u{0316}' => 220,
        '\u{0300}' | '\u{0301}' => 230,
        '\u{0345}' => 240,
        _ => 0,
    }
}

fn naive_canonical_order(input: &str) -> String {
    let mut out: Vec<char> = Vec::new();
    for ch in input.chars() {
        out.push(ch);
        let cc = ccc_ref(ch);
        if cc == 0 {
            continue;
        }
        let mut i = out.len() - 1;
        while i > 0 && ccc_ref(out[i - 1]) > cc {
            out.swap(i - 1, i);
            i -= 1;
        }
    }
    out.into_iter().collect()
}

#[test]
fn nfd_canonical_ordering_matches_naive_reference() {
    const ALPHABET: [char; 7] = [
        'q', '\u{0334}', '\u{0327}', '\u{0316}', '\u{0300}', '\u{0301}', '\u{0345}',
    ];
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    for _ in 0..2_000 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let len = (state % 24) as usize;
        let mut s = String::new();
        let mut r = state;
        for _ in 0..len {
            r = r
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            s.push(ALPHABET[((r >> 33) % ALPHABET.len() as u64) as usize]);
        }
        let expected = naive_canonical_order(&s);
        assert_eq!(
            nfd(&s),
            expected,
            "input {:?}",
            s.escape_unicode().to_string()
        );
        assert!(is_nfd(&expected));
    }
}

#[test]
fn nfd_long_reverse_ordered_mark_run_is_fast() {
    // 100k combining marks in strictly descending ccc order (worst case for insertion sort).
    let classes = ['\u{0345}', '\u{0301}', '\u{0316}', '\u{0327}', '\u{0334}'];
    let per = 20_000;
    let mut input = String::from("q");
    for &c in &classes {
        for _ in 0..per {
            input.push(c);
        }
    }
    let start = std::time::Instant::now();
    let out = nfd(&input);
    let nfc_out = nfc(&input);
    assert!(
        start.elapsed() < std::time::Duration::from_secs(5),
        "took {:?}",
        start.elapsed()
    );
    let mut expected = String::from("q");
    for &c in classes.iter().rev() {
        for _ in 0..per {
            expected.push(c);
        }
    }
    assert_eq!(out, expected);
    assert_eq!(nfc_out, expected);
    assert!(is_nfd(&out));
}

#[test]
fn diff_tiny_budget_on_large_disjoint_input_is_fast() {
    let old: Vec<u32> = (0..2_000_000).collect();
    let new: Vec<u32> = (2_000_000..4_000_000).collect();
    let start = std::time::Instant::now();
    let ops = diff_slices(
        &old,
        &new,
        DiffOptions {
            max_edit_distance: Some(4),
            max_steps: Some(16),
        },
    );
    assert!(
        start.elapsed() < std::time::Duration::from_secs(5),
        "took {:?}",
        start.elapsed()
    );
    assert_eq!(ops.len(), 4_000_000);
    assert!(ops[..2_000_000]
        .iter()
        .all(|op| matches!(op, DiffOp::Delete(_))));
    assert!(ops[2_000_000..]
        .iter()
        .all(|op| matches!(op, DiffOp::Insert(_))));

    // Steps-only budget also bounds the work and allocation.
    let ops = diff_slices(
        &old,
        &new,
        DiffOptions {
            max_edit_distance: None,
            max_steps: Some(8),
        },
    );
    assert_eq!(ops.len(), 4_000_000);
}

#[test]
fn diff_budgeted_results_still_reconstruct_inputs() {
    let old = "a\nb\nc\nd\ne\nf\n";
    let new = "a\nx\nc\ny\ne\nz\n";
    for opts in [
        DiffOptions::new(),
        DiffOptions::unbounded(),
        DiffOptions {
            max_edit_distance: Some(1),
            max_steps: Some(3),
        },
    ] {
        let ops = diff_lines_with(old, new, opts);
        let rebuilt_old: Vec<&str> = ops
            .iter()
            .filter_map(|op| match *op {
                DiffOp::Equal(l) | DiffOp::Delete(l) => Some(l),
                DiffOp::Insert(_) => None,
            })
            .collect();
        let rebuilt_new: Vec<&str> = ops
            .iter()
            .filter_map(|op| match *op {
                DiffOp::Equal(l) | DiffOp::Insert(l) => Some(l),
                DiffOp::Delete(_) => None,
            })
            .collect();
        assert_eq!(rebuilt_old, vec!["a", "b", "c", "d", "e", "f"]);
        assert_eq!(rebuilt_new, vec!["a", "x", "c", "y", "e", "z"]);
    }
}

#[test]
fn patch_round_trip_line_ending_matrix() {
    let samples = [
        "",
        "a\nb\nc\n",
        "a\nb\nc",
        "a\r\nb\r\nc\r\n",
        "a\r\nb\r\nc",
        "a\r\nb\nc\r\n",
        "a\nb\r\nc",
        "a\rb\rc\r",
        "a\rb\nc\r",
        "x\r",
        "\r\n",
        "\n",
        "a\n\nb\r\n\r\nc",
        "a\r\nB\r\nc\r\n",
        "a\nB\nc",
        "only",
    ];
    for old in samples {
        for new in samples {
            for ctx in [0, 1, 3] {
                let patch = unified_diff(old, new, "a/f", "b/f", ctx);
                let applied = apply_patch(old, &patch).unwrap_or_else(|e| {
                    panic!("apply failed {old:?} -> {new:?} (ctx {ctx}): {e}\n{patch}")
                });
                assert_eq!(applied, new, "{old:?} -> {new:?} (ctx {ctx}):\n{patch}");
            }
        }
    }
}

#[test]
fn crlf_patch_preserves_carriage_returns() {
    let old = "one\r\ntwo\r\nthree\r\n";
    let new = "one\r\nTWO\r\nthree\r\n";
    let patch = unified_diff(old, new, "a", "b", 1);
    assert_eq!(
        patch,
        "--- a\n+++ b\n@@ -1,3 +1,3 @@\n one\r\n-two\r\n+TWO\r\n three\r\n"
    );
    assert_eq!(apply_patch(old, &patch).unwrap(), new);

    // Line-ending-only change is a real change and round-trips.
    let lf = "one\ntwo\n";
    let crlf = "one\r\ntwo\r\n";
    let p = unified_diff(lf, crlf, "a", "b", 0);
    assert!(!p.is_empty());
    assert_eq!(apply_patch(lf, &p).unwrap(), crlf);
    // A CRLF patch must not silently apply to the LF variant of its context.
    let p2 = unified_diff(old, new, "a", "b", 1);
    assert!(apply_patch("one\ntwo\nthree\n", &p2).is_err());
}

#[test]
fn missing_final_newline_marker_placement() {
    // Last line keeps content but gains a newline: emitted as -/+ with marker after '-'.
    let p = unified_diff("a\nb", "a\nb\n", "a", "b", 0);
    assert_eq!(
        p,
        "--- a\n+++ b\n@@ -2 +2 @@\n-b\n\\ No newline at end of file\n+b\n"
    );
    // Appending after a final line without newline.
    let p = unified_diff("a", "a\nb\n", "a", "b", 3);
    assert_eq!(
        p,
        "--- a\n+++ b\n@@ -1 +1,2 @@\n-a\n\\ No newline at end of file\n+a\n+b\n"
    );
    assert_eq!(apply_patch("a", &p).unwrap(), "a\nb\n");
    // CRLF line before the marker keeps its '\r'.
    let p = unified_diff("x\r\ny\r", "x\r\nY\r", "a", "b", 1);
    assert!(p.contains("-y\r\n\\ No newline at end of file\n+Y\r\n\\ No newline"));
    assert_eq!(apply_patch("x\r\ny\r", &p).unwrap(), "x\r\nY\r");
}

#[test]
fn apply_patch_tolerates_crlf_structural_lines_and_unterminated_last_line() {
    // Headers / markers with CRLF are still recognized.
    let patch = "--- a\r\n+++ b\r\n@@ -1 +1 @@\r\n-a\n+b\n";
    assert_eq!(apply_patch("a\n", patch).unwrap(), "b\n");
    // A patch whose last body line lacks '\n' is treated as a full line.
    let patch = "--- a\n+++ b\n@@ -1 +1 @@\n-a\n+b";
    assert_eq!(apply_patch("a\n", patch).unwrap(), "b\n");
    // Bounded options keep working through unified_diff_with.
    let p = unified_diff_with(
        "a\r\nb\r\n",
        "A\r\nB\r\n",
        "a",
        "b",
        1,
        DiffOptions {
            max_edit_distance: Some(0),
            max_steps: Some(1),
        },
    );
    assert_eq!(apply_patch("a\r\nb\r\n", &p).unwrap(), "A\r\nB\r\n");
}

// ---------------------------------------------------------------------------
// UCD 16.0.0 canonical combining class / decomposition / composition coverage.
// ---------------------------------------------------------------------------

#[test]
fn nfd_reorders_hebrew_points_by_real_ccc() {
    // BET + DAGESH (ccc 21) + QAMATS (ccc 18): canonical order puts QAMATS first.
    let input = "\u{5D1}\u{5BC}\u{5B8}";
    assert!(!is_nfd(input));
    assert_eq!(nfd(input), "\u{5D1}\u{5B8}\u{5BC}");
    assert!(is_nfd("\u{5D1}\u{5B8}\u{5BC}"));
    // U+FB31 (BET WITH DAGESH) is a composition exclusion: NFC must not recompose it.
    assert_eq!(nfc(input), "\u{5D1}\u{5B8}\u{5BC}");
    assert_eq!(nfd("\u{FB31}"), "\u{5D1}\u{5BC}");
    assert_eq!(nfc("\u{FB31}"), "\u{5D1}\u{5BC}");
    // SHEVA (10) after HIRIQ (14) and SHIN DOT (24) after DAGESH (21).
    assert_eq!(nfd("\u{5E9}\u{5C1}\u{5BC}"), "\u{5E9}\u{5BC}\u{5C1}");
    assert_eq!(nfd("\u{5D0}\u{5B4}\u{5B0}"), "\u{5D0}\u{5B0}\u{5B4}");
}

#[test]
fn nfd_reorders_arabic_marks() {
    // BEH + SHADDA (33) + FATHA (30) -> FATHA, SHADDA.
    let input = "\u{628}\u{651}\u{64E}";
    assert!(!is_nfd(input));
    assert_eq!(nfd(input), "\u{628}\u{64E}\u{651}");
    // U+0610 (230) vs U+0618 (30): previously missing from the table.
    assert_eq!(nfd("\u{628}\u{610}\u{618}"), "\u{628}\u{618}\u{610}");
    // ALEF + HAMZA ABOVE composes to U+0623 in NFC.
    assert_eq!(nfc("\u{627}\u{654}"), "\u{623}");
    assert_eq!(nfd("\u{623}"), "\u{627}\u{654}");
    // Quranic marks U+06D6..U+06ED are non-starters too.
    assert_eq!(nfd("\u{628}\u{6E3}\u{6D6}"), "\u{628}\u{6E3}\u{6D6}");
    assert_eq!(nfd("\u{628}\u{6D6}\u{6E3}"), "\u{628}\u{6E3}\u{6D6}");
}

#[test]
fn nfd_reorders_devanagari_and_excludes_qa() {
    // KA + VIRAMA (9) + NUKTA (7) -> NUKTA, VIRAMA.
    assert_eq!(nfd("\u{915}\u{94D}\u{93C}"), "\u{915}\u{93C}\u{94D}");
    assert!(!is_nfd("\u{915}\u{94D}\u{93C}"));
    // U+0958 QA decomposes but is excluded from recomposition.
    assert_eq!(nfd("\u{958}"), "\u{915}\u{93C}");
    assert_eq!(nfc("\u{958}"), "\u{915}\u{93C}");
    assert!(!is_nfc("\u{958}"));
    // Stress sign U+0951 (230) after NUKTA (7).
    assert_eq!(nfd("\u{915}\u{951}\u{93C}"), "\u{915}\u{93C}\u{951}");
}

/// Assert that `mark` behaves exactly like a mark of class `ccc` under canonical ordering,
/// using reference marks of known classes (equal class = stable, otherwise sorted).
fn assert_ccc(mark: char, ccc: u8) {
    const REFS: &[(char, u8)] = &[
        ('\u{0334}', 1),
        ('\u{093C}', 7),
        ('\u{094D}', 9),
        ('\u{05B0}', 10),
        ('\u{05BC}', 21),
        ('\u{064E}', 30),
        ('\u{0E38}', 103),
        ('\u{0F71}', 129),
        ('\u{0321}', 202),
        ('\u{031B}', 216),
        ('\u{0316}', 220),
        ('\u{0300}', 230),
        ('\u{0315}', 232),
        ('\u{0345}', 240),
    ];
    assert!(
        REFS.iter().any(|&(_, c)| c == ccc),
        "no reference mark with ccc {ccc}"
    );
    for &(r, rc) in REFS {
        if r == mark {
            continue;
        }
        let mr: String = ['x', mark, r].iter().collect();
        let rm: String = ['x', r, mark].iter().collect();
        let (exp_mr, exp_rm) = if ccc < rc {
            (mr.clone(), mr.clone())
        } else if ccc > rc {
            (rm.clone(), rm.clone())
        } else {
            (mr.clone(), rm.clone())
        };
        assert_eq!(
            nfd(&mr),
            exp_mr,
            "U+{:04X} (ccc {ccc}) vs ref ccc {rc}",
            mark as u32
        );
        assert_eq!(
            nfd(&rm),
            exp_rm,
            "U+{:04X} (ccc {ccc}) vs ref ccc {rc}",
            mark as u32
        );
    }
    // A starter never reorders with a mark.
    let s: String = [mark, 'x'].iter().collect();
    assert_eq!(nfd(&s), s);
}

#[test]
fn ccc_values_across_blocks_match_ucd() {
    for &(cp, ccc) in &[
        (0x05B0u32, 10u8),
        (0x05B8, 18),
        (0x05BC, 21),
        (0x0610, 230),
        (0x0618, 30),
        (0x0619, 31),
        (0x061A, 32),
        (0x0656, 220),
        (0x0670, 35),
        (0x06ED, 220),
        (0x0951, 230),
        (0x0952, 220),
        (0x0C55, 84),
        (0x0CBC, 7),
        (0x0DCA, 9),
        (0x0EBA, 9),
        (0x0F71, 129),
        (0x0F72, 130),
        (0x1037, 7),
        (0x1039, 9),
        (0x17D2, 9),
        (0x1AB0, 230),
        (0x1DC0, 230),
        (0x1DC2, 220),
        (0x20D0, 230),
        (0x20D2, 1),
        (0xFE20, 230),
        (0xFE27, 220),
        (0x1D165, 216),
        (0x1D167, 1),
        (0x1E94A, 7),
        (0x10A0D, 220),
        (0x11046, 9),
    ] {
        let ch = char::from_u32(cp).unwrap();
        if [84u8, 31, 32, 35, 18, 130].contains(&ccc) {
            // Classes without a dedicated reference mark: check strict ordering bounds.
            let below = '\u{05B0}'; // ccc 10
            let above = '\u{0300}'; // 230
            let s: String = ['x', above, ch, below].iter().collect();
            let expect: String = ['x', below, ch, above].iter().collect();
            assert_eq!(nfd(&s), expect, "U+{cp:04X}");
        } else {
            assert_ccc(ch, ccc);
        }
    }
}

#[test]
fn ccc_starters_stay_zero() {
    for ch in [
        'a',
        '\u{05D0}',
        '\u{0628}',
        '\u{0915}',
        '\u{4E00}',
        '\u{1F600}',
        '\u{10FFFF}',
    ] {
        let s: String = ['x', '\u{0300}', ch, '\u{0316}'].iter().collect();
        assert_eq!(nfd(&s), s);
    }
}

#[test]
fn astral_decompositions_and_compositions() {
    // MUSICAL SYMBOL HALF NOTE decomposes (and is excluded from composition).
    assert_eq!(nfd("\u{1D15E}"), "\u{1D157}\u{1D165}");
    assert_eq!(nfc("\u{1D15E}"), "\u{1D157}\u{1D165}");
    // KAITHI LETTER RHA is a primary composite.
    assert_eq!(nfc("\u{11099}\u{110BA}"), "\u{1109A}");
    assert_eq!(nfd("\u{1109A}"), "\u{11099}\u{110BA}");
    // CJK compatibility ideograph (singleton) never round-trips.
    assert_eq!(nfc("\u{F900}"), "\u{8C48}");
    assert_eq!(nfc("\u{2F800}"), "\u{4E3D}");
}

#[test]
fn nfc_composes_beyond_latin1() {
    // Vietnamese: e + dot below + circumflex -> U+1EC7 (via U+1EB9).
    assert_eq!(nfc("e\u{323}\u{302}"), "\u{1EC7}");
    assert_eq!(nfc("e\u{302}\u{323}"), "\u{1EC7}");
    assert_eq!(nfd("\u{1EC7}"), "e\u{323}\u{302}");
    // Greek with ypogegrammeni: U+1F80.
    assert_eq!(nfd("\u{1F80}"), "\u{3B1}\u{313}\u{345}");
    assert_eq!(nfc("\u{3B1}\u{313}\u{345}"), "\u{1F80}");
    // Malayalam two-part vowel (starter + starter composition).
    assert_eq!(nfc("\u{D46}\u{D3E}"), "\u{D4A}");
    // Non-starter decomposition U+0344 is excluded.
    assert_eq!(nfc("\u{344}"), "\u{308}\u{301}");
}

/// Curated subset of `NormalizationTest.txt` (Unicode 16.0.0): `(source, NFC, NFD)`.
/// The full file (19 965 cases) passed against this implementation when the tables were
/// regenerated; only a representative selection is embedded here.
const UCD_NORMALIZATION_VECTORS: &[(&str, &str, &str)] = &[
    (
        "\u{1E0A}\u{0323}",
        "\u{1E0C}\u{0307}",
        "\u{0044}\u{0323}\u{0307}",
    ),
    (
        "\u{1100}\u{AC00}\u{11A8}",
        "\u{1100}\u{AC01}",
        "\u{1100}\u{1100}\u{1161}\u{11A8}",
    ),
    ("\u{01D5}", "\u{01D5}", "\u{0055}\u{0308}\u{0304}"),
    ("\u{0344}", "\u{0308}\u{0301}", "\u{0308}\u{0301}"),
    ("\u{0958}", "\u{0915}\u{093C}", "\u{0915}\u{093C}"),
    ("\u{0B48}", "\u{0B48}", "\u{0B47}\u{0B56}"),
    ("\u{0D4A}", "\u{0D4A}", "\u{0D46}\u{0D3E}"),
    ("\u{0F73}", "\u{0F71}\u{0F72}", "\u{0F71}\u{0F72}"),
    ("\u{1F80}", "\u{1F80}", "\u{03B1}\u{0313}\u{0345}"),
    ("\u{2ADC}", "\u{2ADD}\u{0338}", "\u{2ADD}\u{0338}"),
    (
        "\u{FB2C}",
        "\u{05E9}\u{05BC}\u{05C1}",
        "\u{05E9}\u{05BC}\u{05C1}",
    ),
    ("\u{1109A}", "\u{1109A}", "\u{11099}\u{110BA}"),
    ("\u{1D15E}", "\u{1D157}\u{1D165}", "\u{1D157}\u{1D165}"),
    (
        "\u{1D160}",
        "\u{1D158}\u{1D165}\u{1D16E}",
        "\u{1D158}\u{1D165}\u{1D16E}",
    ),
    (
        "\u{0061}\u{0315}\u{0300}\u{05AE}\u{0300}\u{0062}",
        "\u{00E0}\u{05AE}\u{0300}\u{0315}\u{0062}",
        "\u{0061}\u{05AE}\u{0300}\u{0300}\u{0315}\u{0062}",
    ),
    (
        "\u{0061}\u{05B8}\u{05B7}\u{05B6}\u{05B7}\u{0062}",
        "\u{0061}\u{05B6}\u{05B7}\u{05B7}\u{05B8}\u{0062}",
        "\u{0061}\u{05B6}\u{05B7}\u{05B7}\u{05B8}\u{0062}",
    ),
    (
        "\u{0061}\u{0610}\u{0315}\u{0300}\u{05AE}\u{0062}",
        "\u{0061}\u{05AE}\u{0610}\u{0300}\u{0315}\u{0062}",
        "\u{0061}\u{05AE}\u{0610}\u{0300}\u{0315}\u{0062}",
    ),
    (
        "\u{0061}\u{0618}\u{0619}\u{0618}\u{064D}\u{0062}",
        "\u{0061}\u{064D}\u{0618}\u{0618}\u{0619}\u{0062}",
        "\u{0061}\u{064D}\u{0618}\u{0618}\u{0619}\u{0062}",
    ),
    (
        "\u{0061}\u{1AB0}\u{0315}\u{0300}\u{05AE}\u{0062}",
        "\u{0061}\u{05AE}\u{1AB0}\u{0300}\u{0315}\u{0062}",
        "\u{0061}\u{05AE}\u{1AB0}\u{0300}\u{0315}\u{0062}",
    ),
    (
        "\u{0061}\u{1DC0}\u{0315}\u{0300}\u{05AE}\u{0062}",
        "\u{0061}\u{05AE}\u{1DC0}\u{0300}\u{0315}\u{0062}",
        "\u{0061}\u{05AE}\u{1DC0}\u{0300}\u{0315}\u{0062}",
    ),
    (
        "\u{0061}\u{20D0}\u{0315}\u{0300}\u{05AE}\u{0062}",
        "\u{0061}\u{05AE}\u{20D0}\u{0300}\u{0315}\u{0062}",
        "\u{0061}\u{05AE}\u{20D0}\u{0300}\u{0315}\u{0062}",
    ),
    (
        "\u{0061}\u{302A}\u{0316}\u{1DFA}\u{031B}\u{0062}",
        "\u{0061}\u{031B}\u{302A}\u{1DFA}\u{0316}\u{0062}",
        "\u{0061}\u{031B}\u{302A}\u{1DFA}\u{0316}\u{0062}",
    ),
    (
        "\u{0061}\u{FE20}\u{0315}\u{0300}\u{05AE}\u{0062}",
        "\u{0061}\u{05AE}\u{FE20}\u{0300}\u{0315}\u{0062}",
        "\u{0061}\u{05AE}\u{FE20}\u{0300}\u{0315}\u{0062}",
    ),
];

#[test]
fn ucd_normalization_test_subset() {
    for &(src, c2, c3) in UCD_NORMALIZATION_VECTORS {
        assert_eq!(nfc(src), c2, "NFC({src:?})");
        assert_eq!(nfd(src), c3, "NFD({src:?})");
        assert_eq!(nfc(c3), c2, "NFC(NFD({src:?}))");
        assert_eq!(nfd(c2), c3, "NFD(NFC({src:?}))");
        assert!(is_nfc(c2) && is_nfd(c3), "{src:?}");
    }
}
