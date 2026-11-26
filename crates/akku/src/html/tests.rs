use akku::html::{escape, unescape, Html};
use std::borrow::Cow;
use std::time::{Duration, Instant};

#[test]
fn html_public_api_and_escaping() {
    let raw = Html::from("<p>Hello</p>");
    assert_eq!(raw.as_str(), "<p>Hello</p>");
    assert_eq!(raw.clone().into_string(), "<p>Hello</p>");
    assert_eq!(
        escape("<script>alert(\"x\" & 'y')</script>"),
        "&lt;script&gt;alert(&#34;x&#34; &amp; &#39;y&#39;)&lt;/script&gt;"
    );
}

#[test]
fn unescape_borrows_when_nothing_to_decode() {
    assert!(matches!(
        unescape("plain text"),
        Cow::Borrowed("plain text")
    ));
    assert!(matches!(unescape(""), Cow::Borrowed("")));
    // `&` present but no valid reference: content is unchanged.
    assert_eq!(unescape("a & b"), "a & b");
    assert_eq!(unescape("&unknown;"), "&unknown;");
}

#[test]
fn unescape_numeric_references() {
    assert_eq!(unescape("&#39;&#x27;&#X27;"), "'''");
    assert_eq!(unescape("&#x1F600;"), "\u{1F600}");
    assert_eq!(unescape("&#65&#x41"), "AA"); // missing `;` is accepted
    assert_eq!(unescape("&#0065;"), "A");
    assert_eq!(unescape("&#X4a;"), "J");
}

#[test]
fn unescape_invalid_code_points_become_replacement_character() {
    assert_eq!(unescape("&#0;"), "\u{FFFD}");
    assert_eq!(unescape("&#xD800;&#xDFFF;"), "\u{FFFD}\u{FFFD}");
    assert_eq!(unescape("&#x110000;"), "\u{FFFD}");
    assert_eq!(unescape("&#99999999999999999999;"), "\u{FFFD}");
    assert_eq!(unescape(&format!("&#x{};", "F".repeat(5000))), "\u{FFFD}");
    assert_eq!(unescape(&format!("&#{};", "9".repeat(5000))), "\u{FFFD}");
}

#[test]
fn unescape_c1_controls_use_windows_1252() {
    assert_eq!(unescape("&#128;"), "\u{20AC}");
    assert_eq!(unescape("&#x93;x&#x94;"), "\u{201C}x\u{201D}");
    assert_eq!(unescape("&#159;"), "\u{178}");
    // Unassigned slots keep the C1 code point.
    assert_eq!(unescape("&#x81;"), "\u{81}");
    assert_eq!(unescape("&#x9F;&#xA0;"), "\u{178}\u{A0}");
}

#[test]
fn unescape_degenerate_references_stay_literal() {
    for s in [
        "&", "a&", "&;", "&#;", "&#x;", "&#X;", "&#", "&#x", "&&", "&#xg;", "&#-1;",
    ] {
        assert_eq!(unescape(s), s, "{s:?}");
    }
    assert_eq!(unescape("&#;&#65;"), "&#;A");
    assert_eq!(unescape("&&amp;"), "&&");
}

#[test]
fn unescape_named_references() {
    assert_eq!(unescape("&lt;&gt;&amp;&quot;&apos;"), "<>&\"'");
    assert_eq!(
        unescape("&copy;&eacute;&hearts;&nbsp;"),
        "\u{A9}\u{E9}\u{2665}\u{A0}"
    );
    assert_eq!(unescape("&NotEqualTilde;"), "\u{2242}\u{338}"); // two code points
    assert_eq!(unescape("&AMP;&LT;&GT;"), "&<>");
    assert_eq!(unescape("&AMP&amp&lt&gt&quot"), "&&<>\"");
}

#[test]
fn unescape_is_case_sensitive_and_needs_semicolon_for_modern_names() {
    assert_eq!(unescape("&Amp;"), "&Amp;");
    assert_eq!(unescape("&hearts"), "&hearts"); // not a legacy entity
    assert_eq!(unescape("&hearts;"), "\u{2665}");
}

#[test]
fn unescape_longest_prefix_wins_for_legacy_entities() {
    assert_eq!(unescape("&notit;"), "\u{AC}it;");
    assert_eq!(unescape("&notin;"), "\u{2209}");
    assert_eq!(unescape("&ampfoo"), "&foo");
    assert_eq!(unescape("&amp;amp;"), "&amp;");
    assert_eq!(unescape("&copyright"), "\u{A9}right");
    assert_eq!(unescape("&ltx;"), "<x;");
}

#[test]
fn unescape_nested_and_consecutive_references() {
    assert_eq!(unescape("&amp;lt;"), "&lt;");
    assert_eq!(unescape("&#38;#38;"), "&#38;");
    assert_eq!(unescape("&lt;&lt;&lt;"), "<<<");
    assert_eq!(unescape("&#60;&#x3c;&lt;"), "<<<");
    assert_eq!(unescape("&amp;&amp;&amp;"), "&&&");
}

#[test]
fn unescape_handles_non_ascii_input() {
    assert_eq!(
        unescape("h\u{E9}llo &amp; w\u{F6}rld \u{1F600}"),
        "h\u{E9}llo & w\u{F6}rld \u{1F600}"
    );
    assert_eq!(unescape("\u{1F600}&"), "\u{1F600}&");
    assert_eq!(unescape("&\u{E9};"), "&\u{E9};");
    assert_eq!(unescape("&#\u{E9};"), "&#\u{E9};");
    assert_eq!(unescape("&lt\u{E9}"), "<\u{E9}");
}

#[test]
fn unescape_round_trips_escape() {
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let alphabet: Vec<char> = "&<>\"'#;xX0123456789abcdefAMPlt \u{E9}\u{4E2D}\u{1F600}\n"
        .chars()
        .collect();
    for _ in 0..2000 {
        let len = (next() % 40) as usize;
        let s: String = (0..len)
            .map(|_| alphabet[(next() % alphabet.len() as u64) as usize])
            .collect();
        assert_eq!(unescape(&escape(&s)), s, "{s:?}");
    }
}

#[test]
fn unescape_runs_in_linear_time_on_hostile_input() {
    let start = Instant::now();
    for input in [
        "&".repeat(1_000_000),
        "&#".repeat(500_000),
        "&#x".repeat(300_000),
        "&a".repeat(500_000),
        format!("&{}", "a".repeat(1_000_000)),
        format!("&#{}", "1".repeat(1_000_000)),
        "&notit".repeat(200_000),
    ] {
        let out = unescape(&input);
        assert!(out.len() <= input.len() * 3);
    }
    assert!(start.elapsed() < Duration::from_secs(10), "too slow");
}
