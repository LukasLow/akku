//! Integration tests for `akku::regex`.

use std::borrow::Cow;
use std::time::{Duration, Instant};

use akku::regex::{escape, Captures, ErrorKind, NoExpand, Regex, RegexBuilder, RegexSet, Span};

fn re(p: &str) -> Regex {
    Regex::new(p).unwrap_or_else(|e| panic!("{p:?} failed to compile: {e}"))
}

fn find(p: &str, h: &str) -> Option<(usize, usize)> {
    re(p).find(h).map(|m| (m.start(), m.end()))
}

fn find_str<'t>(p: &str, h: &'t str) -> Option<&'t str> {
    re(p).find(h).map(|m| m.as_str())
}

fn all(p: &str, h: &str) -> Vec<(usize, usize)> {
    re(p).find_iter(h).map(|m| (m.start(), m.end())).collect()
}

fn kind(p: &str) -> ErrorKind {
    Regex::new(p).expect_err(p).kind().clone()
}

/// Best of three runs, to keep timing assertions robust on noisy CI.
fn best_of_3(mut f: impl FnMut()) -> Duration {
    (0..3)
        .map(|_| {
            let t = Instant::now();
            f();
            t.elapsed()
        })
        .min()
        .unwrap()
}

// ---- construction & errors ----------------------------------------------------

#[test]
fn compiles_valid_corpus() {
    fn assert_traits<T: Send + Sync + Clone + std::fmt::Debug>() {}
    assert_traits::<Regex>();
    assert_traits::<RegexSet>();
    for p in [
        "",
        "a",
        "abc",
        "a|b|",
        "(a)(b)",
        "(?:x)",
        "(?P<n>x)",
        "(?<n>x)",
        "a*b+c?",
        "a{3}",
        "a{2,}",
        "a{2,5}?",
        "[a-z]",
        "[^a]",
        "[]]",
        "[-a-]",
        r"\d\D\w\W\s\S",
        r"\bx\B",
        r"\Ax\z",
        "^$",
        "(?i)x",
        "(?im-s)",
        "(?i:x)",
        r"\x41\x{1F600}\u{E9}é",
        r"\.\*\{\}",
        "}",
        "]",
    ] {
        re(p);
    }
    let r: Regex = "a+".parse().unwrap();
    assert_eq!(r.as_str(), "a+");
    assert_eq!(r.to_string(), "a+");
    assert_eq!(format!("{r:?}"), r#"Regex("a+")"#);
    let cloned = r.clone();
    assert!(cloned.is_match("aa"));
}

#[test]
fn error_kinds_and_spans() {
    let cases: Vec<(&str, ErrorKind, usize, usize)> = vec![
        ("a(b", ErrorKind::UnclosedGroup, 1, 2),
        ("a)", ErrorKind::UnopenedGroup, 1, 2),
        ("[a", ErrorKind::UnclosedClass, 0, 1),
        (
            "[z-a]",
            ErrorKind::InvalidClassRange {
                start: 'z',
                end: 'a',
            },
            1,
            4,
        ),
        ("*a", ErrorKind::RepetitionMissing, 0, 1),
        ("a**", ErrorKind::RepetitionStacked, 2, 3),
        ("a{2", ErrorKind::RepetitionCountUnclosed, 1, 3),
        (
            "a{3,2}",
            ErrorKind::RepetitionCountInvalid { min: 3, max: 2 },
            1,
            6,
        ),
        (
            "a{1001}",
            ErrorKind::RepetitionCountTooLarge { limit: 1000 },
            1,
            7,
        ),
        ("\\q", ErrorKind::InvalidEscape { c: 'q' }, 0, 2),
        ("a\\", ErrorKind::TrailingBackslash, 1, 2),
        ("\\xZZ", ErrorKind::InvalidHexEscape, 0, 3),
        (
            "\\u{D800}",
            ErrorKind::InvalidCodepoint { value: 0xD800 },
            0,
            8,
        ),
        ("(?<1a>x)", ErrorKind::InvalidGroupName, 3, 5),
        (
            "(?<n>a)(?<n>b)",
            ErrorKind::DuplicateGroupName { name: "n".into() },
            10,
            11,
        ),
        ("(?z)", ErrorKind::UnknownFlag { flag: 'z' }, 2, 3),
        ("(?)", ErrorKind::InvalidFlags, 0, 3),
        ("(?i-)", ErrorKind::InvalidFlags, 0, 5),
        ("(?ii)", ErrorKind::InvalidFlags, 3, 4),
        ("a|*", ErrorKind::RepetitionMissing, 2, 3),
        ("(+)", ErrorKind::RepetitionMissing, 1, 2),
    ];
    for (p, k, start, end) in cases {
        let e = Regex::new(p).expect_err(p);
        assert_eq!(e.kind(), &k, "pattern {p:?}");
        assert_eq!(e.span(), Some(Span { start, end }), "pattern {p:?}");
        assert_eq!(e.pattern(), p);
    }
}

#[test]
fn error_span_is_byte_offset_in_non_ascii_pattern() {
    let e = Regex::new("é(").unwrap_err();
    assert_eq!(e.kind(), &ErrorKind::UnclosedGroup);
    assert_eq!(e.span(), Some(Span { start: 2, end: 3 }));
    let shown = e.to_string();
    assert!(shown.contains("unclosed group"), "{shown}");
    assert!(shown.contains("é("), "{shown}");
    assert!(shown.ends_with("\n     ^"), "{shown:?}");
    let _: &dyn std::error::Error = &e;
}

#[test]
fn unsupported_features_rejected() {
    for p in [
        r"(a)\1",
        "(?P<n>a)(?P=n)",
        "(?=a)",
        "(?!a)",
        "(?<=a)",
        "(?<!a)",
        "(?>a)",
        "a*+",
        "a++",
        "a?+",
        "a{2}+",
        "(?#c)",
        r"\pL",
        r"\p{L}",
        r"\PL",
        "[[:alpha:]]",
        "[a&&b]",
        "[a--b]",
        "[a~~b]",
        r"\Z",
        r"\G",
        r"\K",
        r"\0",
        r"\<",
        r"\>",
        "(?x)a",
        "(?U)a",
        "(?u)a",
        "(?R)a",
    ] {
        assert!(
            matches!(kind(p), ErrorKind::Unsupported { .. }),
            "{p:?} gave {:?}",
            kind(p)
        );
    }
}

#[test]
fn nest_limit_enforced() {
    let deep = format!("{}a{}", "(".repeat(300), ")".repeat(300));
    assert_eq!(kind(&deep), ErrorKind::NestLimitExceeded { limit: 250 });
    let ok = RegexBuilder::new(&deep).nest_limit(400).build().unwrap();
    assert_eq!(ok.captures_len(), 301);
    assert!(ok.is_match("xa"));
    // Non-capturing nesting and repetitions count too.
    let noncap = format!("{}a{}", "(?:".repeat(300), ")".repeat(300));
    assert!(matches!(kind(&noncap), ErrorKind::NestLimitExceeded { .. }));
    // The hard ceiling (1000) compiles, matches and drops without exhausting
    // the 2 MiB test-thread stack.
    let max = format!("{}a{}", "(?:b|(".repeat(499), "))".repeat(499));
    let r = RegexBuilder::new(&max)
        .nest_limit(u32::MAX)
        .size_limit(usize::MAX)
        .build()
        .expect("depth 998 within hard ceiling");
    assert!(r.is_match("a") && r.find("xa").is_some());
    let caps = format!("{}a{}", "(".repeat(1000), ")".repeat(1000));
    let r = RegexBuilder::new(&caps)
        .nest_limit(u32::MAX)
        .size_limit(usize::MAX)
        .build()
        .expect("depth 1000 within hard ceiling");
    assert_eq!(r.find("ba").map(|m| m.range()), Some(1..2));
    drop(r);
    let over = format!("{}a{}", "(".repeat(1001), ")".repeat(1001));
    let e = RegexBuilder::new(&over)
        .nest_limit(u32::MAX)
        .build()
        .unwrap_err();
    assert_eq!(e.kind(), &ErrorKind::NestLimitExceeded { limit: 1000 });
}

#[test]
fn size_and_length_limits_enforced_without_blowup() {
    let t = Instant::now();
    let e = Regex::new("(?:(?:a{1000}){1000}){1000}").unwrap_err();
    assert!(matches!(e.kind(), ErrorKind::SizeLimitExceeded { .. }));
    assert_eq!(e.span(), None);
    assert!(e.to_string().contains("size limit"));
    let e = Regex::new("(?:a{1000}){1000}").unwrap_err();
    assert!(matches!(e.kind(), ErrorKind::SizeLimitExceeded { .. }));
    assert!(t.elapsed() < Duration::from_millis(50), "{:?}", t.elapsed());

    let small = RegexBuilder::new("a{500}")
        .size_limit(16 << 10)
        .build()
        .unwrap_err();
    assert_eq!(
        small.kind(),
        &ErrorKind::SizeLimitExceeded { limit: 16 << 10 }
    );
    assert!(RegexBuilder::new("a{50}")
        .size_limit(16 << 10)
        .build()
        .is_ok());

    let long = "a".repeat(200 << 10);
    assert_eq!(kind(&long), ErrorKind::PatternTooLong { limit: 128 << 10 });
    let r = RegexBuilder::new(&long)
        .pattern_len_limit(usize::MAX)
        .size_limit(usize::MAX)
        .build()
        .unwrap();
    assert!(r.is_match(&long));
    assert!(RegexBuilder::new("abcdef")
        .pattern_len_limit(3)
        .build()
        .is_err());
}

#[test]
fn case_fold_compile_is_cheap() {
    re("(?i)a"); // builds the lazy fold table once
    let p = r"(?i)[\x00-\x{10FFFF}]".repeat(50);
    let t = Instant::now();
    let r = re(&p);
    assert!(
        t.elapsed() < Duration::from_millis(250),
        "{:?}",
        t.elapsed()
    );
    assert!(r.is_match(&"é".repeat(50)));
}

// ---- syntax semantics ------------------------------------------------------------

#[test]
fn dot_and_dotall() {
    assert_eq!(find("a.c", "a\nc abc"), Some((4, 7)));
    assert!(!re("^.$").is_match("\n"));
    assert!(re("(?s)^.$").is_match("\n"));
    assert!(RegexBuilder::new("^.$")
        .dot_matches_new_line(true)
        .build()
        .unwrap()
        .is_match("\n"));
    assert_eq!(find_str(".", "€"), Some("€"));
}

#[test]
fn anchors_default_and_multiline() {
    assert!(
        !re("a$").is_match("a\n"),
        "$ must not match before a trailing newline"
    );
    assert!(re("(?m)a$").is_match("a\n"));
    let starts: Vec<usize> = all("(?m)^", "a\nb\n").into_iter().map(|m| m.0).collect();
    assert_eq!(starts, [0, 2, 4]);
    let ends: Vec<usize> = all("(?m)$", "a\nb").into_iter().map(|m| m.0).collect();
    assert_eq!(ends, [1, 3]);
    assert!(!re("^b").is_match("a\nb"));
    assert!(re("(?m)^b").is_match("a\nb"));
    assert!(!re(r"(?m)\Aa").is_match("b\na"));
    assert!(!re(r"(?m)a\z").is_match("a\nb"));
    assert!(RegexBuilder::new("^b$")
        .multi_line(true)
        .build()
        .unwrap()
        .is_match("a\nb\nc"));
}

#[test]
fn word_boundaries_ascii() {
    assert_eq!(find(r"\bfoo\b", "a foo."), Some((2, 5)));
    assert!(!re(r"\bfoo\b").is_match("afoo"));
    assert_eq!(find(r"\Bo\B", "foo"), Some((1, 2)));
    assert_eq!(all(r"\b", "ab cd"), [(0, 0), (2, 2), (3, 3), (5, 5)]);
    // \b is ASCII-only: 'é' is not a word character.
    assert!(!re(r"\bé").is_match("é"));
    assert!(re(r"\Bé\B").is_match("é"));
}

#[test]
fn greedy_vs_lazy() {
    assert_eq!(find("a*?", "aaa"), Some((0, 0)));
    assert_eq!(find_str("a+?", "aaa"), Some("a"));
    assert_eq!(find_str("a??", "aaa"), Some(""));
    assert_eq!(find_str("a{2,4}?", "aaaa"), Some("aa"));
    assert_eq!(find_str("<.+?>", "<a><b>"), Some("<a>"));
    assert_eq!(find_str("a*", "aaa"), Some("aaa"));
    assert_eq!(find_str("<.+>", "<a><b>"), Some("<a><b>"));
    assert_eq!(find_str("a*?b", "aaab"), Some("aaab"));
}

#[test]
fn counted_repetition() {
    let three = re("^a{3}$");
    assert!(three.is_match("aaa") && !three.is_match("aa") && !three.is_match("aaaa"));
    let atleast = re("^a{2,}$");
    assert!(atleast.is_match("aa") && atleast.is_match("aaaaa") && !atleast.is_match("a"));
    let range = re("^a{2,4}$");
    assert!(range.is_match("aa") && range.is_match("aaaa") && !range.is_match("aaaaa"));
    assert_eq!(find("a{0}", "aaa"), Some((0, 0)));
    assert!(re("^(?:ab){2}$").is_match("abab"));
    assert_eq!(find_str("x{1,3}", "xxxxx"), Some("xxx"));
}

#[test]
fn character_classes() {
    assert_eq!(find_str("[]a]+", "x]a]y"), Some("]a]"));
    assert_eq!(find_str("[^]a]", "]ab"), Some("b"));
    assert_eq!(find_str("[a-]+", "x-a-"), Some("-a-"));
    assert_eq!(find_str("[-a]+", "x-a-"), Some("-a-"));
    assert_eq!(find_str(r"[\]\-\\]+", r"x]-\y"), Some(r"]-\"));
    assert_eq!(find_str(r"[\d_]+", "ab1_2c"), Some("1_2"));
    assert_eq!(find_str(r"[^\s]+", "  héllo "), Some("héllo"));
    assert_eq!(find("[^a-z]", "abcÄ"), Some((3, 5)));
    assert_eq!(find_str("[α-ω]+", "xαβγx"), Some("αβγ"));
    assert_eq!(find_str(r"[\x41-\x43]+", "xABCD"), Some("ABC"));
}

#[test]
fn perl_classes_ascii() {
    assert_eq!(find_str(r"\d+", "ab123c"), Some("123"));
    assert!(!re(r"\d").is_match("٣"));
    assert_eq!(find_str(r"\D+", "12ab3"), Some("ab"));
    assert_eq!(find_str(r"\w+", "héllo"), Some("h"));
    assert_eq!(find_str(r"\W", "a_b-c"), Some("-"));
    assert_eq!(find_str(r"\s+", "a \t\nb"), Some(" \t\n"));
    assert!(!re(r"\s").is_match("\u{A0}"));
    assert_eq!(find_str(r"\S+", "  ab  "), Some("ab"));
}

#[test]
fn hex_unicode_and_control_escapes() {
    assert!(re(r"^\x41$").is_match("A"));
    assert!(re(r"^\x{41}$").is_match("A"));
    assert!(re(r"^\x{1F600}$").is_match("😀"));
    assert!(re(r"^\u{E9}$").is_match("é"));
    assert!(re(r"^é$").is_match("é"));
    assert!(re(r"^a\tb\n$").is_match("a\tb\n"));
    assert!(re(r"^\.$").is_match(".") && !re(r"^\.$").is_match("a"));
    assert!(re(r"^\$\^\(\)$").is_match("$^()"));
    assert!(re(r"^\e\a\f\v\r$").is_match("\x1B\x07\x0C\x0B\r"));
}

#[test]
fn inline_flags_scoped_and_negated() {
    assert!(re("a(?i)b").is_match("aB") && !re("a(?i)b").is_match("AB"));
    assert!(re("^(?i:a)b$").is_match("Ab") && !re("^(?i:a)b$").is_match("AB"));
    assert!(re("^(?i)a(?-i)b$").is_match("Ab") && !re("^(?i)a(?-i)b$").is_match("AB"));
    assert_eq!(
        find_str("a(?i)b|c", "xC"),
        Some("C"),
        "flags extend over later alternatives"
    );
    assert!(re("^(?:(?i)a)a$").is_match("Aa") && !re("^(?:(?i)a)a$").is_match("AA"));
    assert!(re("(?ms)^a.b$").is_match("x\na\nb\ny"));
    let ci = RegexBuilder::new("abc")
        .case_insensitive(true)
        .build()
        .unwrap();
    assert!(ci.is_match("ABC"));
    let off = RegexBuilder::new("(?-i)abc")
        .case_insensitive(true)
        .build()
        .unwrap();
    assert!(!off.is_match("ABC"));
}

#[test]
fn case_insensitive_unicode() {
    assert!(re("(?i)ä").is_match("Ä"));
    assert!(re("(?i)Σ").is_match("σ"));
    assert!(re("(?i)σ").is_match("Σ") && re("(?i)σ").is_match("ς"));
    assert!(re("(?i)k").is_match("\u{212A}"));
    assert!(re("(?i)[a-z]").is_match("\u{212A}"));
    assert!(
        !re("(?i)straße").is_match("STRASSE"),
        "multi-char folds are not applied"
    );
    assert!(!re("(?i)[^a]").is_match("A"));
    assert_eq!(find_str("(?i)hello", "say HeLLo"), Some("HeLLo"));
}

// ---- matching semantics ----------------------------------------------------------

#[test]
fn leftmost_first_alternation() {
    assert_eq!(find_str("a|ab", "ab"), Some("a"));
    assert_eq!(find_str("ab|a", "ab"), Some("ab"));
    assert_eq!(find_str("sam|samwise", "samwise"), Some("sam"));
    let caps = re("(a|ab)(c|bcd)(d*)").captures("abcd").unwrap();
    assert_eq!((&caps[1], &caps[2], &caps[3]), ("a", "bcd", ""));
    // Leftmost wins over longest-later.
    assert_eq!(find_str("b+|ab", "abbb"), Some("ab"));
}

#[test]
fn capture_numbering_and_names() {
    let r = re(r"((a)(?:b)(?P<c>c))(?<d>d)");
    assert_eq!(r.captures_len(), 5);
    let names: Vec<_> = r.capture_names().collect();
    assert_eq!(names, [None, None, None, Some("c"), Some("d")]);
    let caps = r.captures("xabcd").unwrap();
    assert_eq!(caps.len(), 5);
    assert_eq!(&caps[0], "abcd");
    assert_eq!(&caps[1], "abc");
    assert_eq!(&caps[2], "a");
    assert_eq!(&caps["c"], "c");
    assert_eq!(&caps[4], "d");
    assert_eq!(caps.name("d").unwrap().range(), 4..5);
    assert!(caps.name("zz").is_none() && caps.get(9).is_none());
    let groups: Vec<Option<&str>> = caps.iter().map(|m| m.map(|m| m.as_str())).collect();
    assert_eq!(
        groups,
        [Some("abcd"), Some("abc"), Some("a"), Some("c"), Some("d")]
    );
    assert_eq!(caps.get_match().start(), 1);
    assert!(format!("{caps:?}").contains("\"abcd\""));
}

#[test]
fn non_participating_and_repeated_groups() {
    let caps = re("(a)|(b)").captures("b").unwrap();
    assert!(caps.get(1).is_none());
    assert_eq!(caps.get(2).unwrap().range(), 0..1);
    let caps = re("(a|b)*").captures("ab").unwrap();
    assert_eq!(caps.get(1).unwrap().range(), 1..2, "last iteration wins");
    let caps = re("(a*)*").captures("b").unwrap();
    assert_eq!(caps.get(1).map(|m| m.range()), Some(0..0));
    let caps = re("(a*)+").captures("b").unwrap();
    assert_eq!(caps.get(1).map(|m| m.range()), Some(0..0));
    let caps = re("(a)?b").captures("b").unwrap();
    assert!(caps.get(1).is_none());
}

#[test]
fn empty_matches_and_zero_width_loops() {
    for p in [
        "(?:)*",
        "(|a)+",
        "(?:a?)*?",
        "(a*)*",
        r"(?:\b)*",
        "(?:a*)*b",
        "(?:a?){5}",
        "(?:^)*$",
    ] {
        let r = re(p);
        let n = r.find_iter("aab").take(100).count();
        assert!(n > 0 && n < 100, "{p:?} produced {n} matches");
    }
    assert_eq!(all("a*", "baaa"), [(0, 0), (1, 4)]);
    assert_eq!(all("", "abc"), [(0, 0), (1, 1), (2, 2), (3, 3)]);
    assert_eq!(all("a*", "aaa"), [(0, 3)]);
    assert_eq!(find("(|a)+", "aa"), Some((0, 0)));
    assert_eq!(re("x*").replace_all("abc", "-"), "-a-b-c-");
}

#[test]
fn find_at_and_captures_at_respect_lookbehind() {
    assert!(re("^b").find_at("ab", 1).is_none());
    assert!(re(r"\bb").find_at("ab", 1).is_none());
    assert_eq!(
        re("(?m)^b").find_at("a\nb", 2).map(|m| m.range()),
        Some(2..3)
    );
    assert_eq!(re("b").find_at("ab", 1).map(|m| m.range()), Some(1..2));
    assert!(!re("^b").is_match_at("ab", 1));
    assert!(re("b").is_match_at("ab", 1));
    assert!(!re("a").is_match_at("ab", 1));
    assert_eq!(
        re("(a)")
            .captures_at("xay", 1)
            .unwrap()
            .get(1)
            .unwrap()
            .range(),
        1..2
    );
    assert!(re("x").find_at("ab", 2).is_none());
}

#[test]
fn find_iter_matches_captures_iter() {
    let r = re(r"\w+");
    let hay = "foo bar  baz";
    let a: Vec<_> = r.find_iter(hay).map(|m| m.range()).collect();
    let b: Vec<_> = r
        .captures_iter(hay)
        .map(|c| c.get_match().range())
        .collect();
    assert_eq!(a, [0..3, 4..7, 9..12]);
    assert_eq!(a, b);
    let mut it = r.find_iter("x");
    assert!(it.next().is_some());
    assert!(it.next().is_none() && it.next().is_none());
    let pairs: Vec<(String, String)> = re(r"(\w)=(\d)")
        .captures_iter("a=1,b=2")
        .map(|c| (c[1].to_owned(), c[2].to_owned()))
        .collect();
    assert_eq!(pairs, [("a".into(), "1".into()), ("b".into(), "2".into())]);
}

// ---- UTF-8 safety ----------------------------------------------------------------

#[test]
fn offsets_always_on_char_boundaries() {
    let hay = "aé€😀\nßa".repeat(3);
    for p in [
        ".",
        r"\w*",
        "[^a]",
        "",
        "é|€",
        "(?i)ß",
        r"\b",
        "😀+",
        "(?s).{2}",
        r"[^\x00-\x7F]+",
        r"\B",
        "a*",
        "(?m)$",
        r"\W",
        "(.)(.)?",
    ] {
        let r = re(p);
        for m in r.find_iter(&hay) {
            assert!(
                hay.is_char_boundary(m.start()) && hay.is_char_boundary(m.end()),
                "{p:?} {m:?}"
            );
        }
        for caps in r.captures_iter(&hay) {
            for m in caps.iter().flatten() {
                assert!(
                    hay.is_char_boundary(m.start()) && hay.is_char_boundary(m.end()),
                    "{p:?}"
                );
            }
        }
    }
    assert_eq!(find(".", "€"), Some((0, 3)));
    assert_eq!(find("é", "café"), Some((3, 5)));
}

#[test]
fn empty_pattern_on_multibyte_text() {
    let starts: Vec<usize> = all("", "äö€").into_iter().map(|m| m.0).collect();
    assert_eq!(starts, [0, 2, 4, 7]);
    let parts: Vec<&str> = re("").split("äö€").collect();
    assert_eq!(parts, ["", "ä", "ö", "€", ""]);
    assert_eq!(re("").replace_all("äö€", "-"), "-ä-ö-€-");
}

#[test]
#[should_panic(expected = "char boundary")]
fn find_at_non_char_boundary_panics() {
    let _ = re("x").find_at("é", 1);
}

// ---- split, replace, escape --------------------------------------------------------

#[test]
fn split_and_splitn() {
    let comma = re(",");
    assert_eq!(
        comma.split("a,b,,c,").collect::<Vec<_>>(),
        ["a", "b", "", "c", ""]
    );
    assert_eq!(
        re(r"\s+").split("a  b\tc").collect::<Vec<_>>(),
        ["a", "b", "c"]
    );
    assert_eq!(comma.split("").collect::<Vec<_>>(), [""]);
    assert!(comma.splitn("a,b,c", 0).next().is_none());
    assert_eq!(comma.splitn("a,b,c", 1).collect::<Vec<_>>(), ["a,b,c"]);
    assert_eq!(comma.splitn("a,b,c", 2).collect::<Vec<_>>(), ["a", "b,c"]);
    assert_eq!(
        comma.splitn("a,b,c", 10).collect::<Vec<_>>(),
        ["a", "b", "c"]
    );
}

#[test]
fn replacement_expansion() {
    let r = re(r"(?P<y>\d{4})-(?P<m>\d{2})");
    let h = "2024-05";
    let cases = [
        ("$m/$y", "05/2024"),
        ("$0!", "2024-05!"),
        ("${y}x", "2024x"),
        ("${1}x", "2024x"),
        ("$1x", ""),
        ("$$", "$"),
        ("a$", "a$"),
        ("$!", "$!"),
        ("${y", "${y"),
        ("$9", ""),
        ("$nope", ""),
        ("[$2]", "[05]"),
    ];
    for (rep, want) in cases {
        assert_eq!(r.replace(h, rep), want, "replacement {rep:?}");
        let caps = r.captures(h).unwrap();
        let mut out = String::new();
        caps.expand(rep, &mut out);
        assert_eq!(out, want, "expand {rep:?}");
    }
    assert_eq!(r.replace(h, String::from("$y")), "2024");
    assert_eq!(r.replace(h, &String::from("$m")), "05");
    assert_eq!(r.replace(h, Cow::Borrowed("$m")), "05");
}

#[test]
fn replace_first_all_n_and_cow() {
    let a = re("a");
    assert_eq!(a.replace("banana", "o"), "bonana");
    assert_eq!(a.replace_all("banana", "o"), "bonono");
    assert_eq!(a.replacen("banana", 2, "o"), "bonona");
    assert!(matches!(a.replace_all("xyz", "o"), Cow::Borrowed("xyz")));
    assert!(matches!(
        re("(a)").replace_all("xyz", "$1"),
        Cow::Borrowed(_)
    ));
    assert!(matches!(a.replace_all("aa", "b"), Cow::Owned(_)));
}

#[test]
fn replace_with_closure_and_no_expand() {
    let r = re(r"\b(\w)(\w*)");
    let titled = r.replace_all("hello big world", |c: &Captures<'_>| {
        format!("{}{}", c[1].to_uppercase(), &c[2])
    });
    assert_eq!(titled, "Hello Big World");
    assert_eq!(re(r"\w+").replace_all("a b", NoExpand("$0")), "$0 $0");
}

#[test]
fn escape_roundtrip() {
    let nasty = r"\.+*?()|[]{}^$#&-~ é😀 a";
    let r = re(&escape(nasty));
    let m = r.find(nasty).unwrap();
    assert_eq!(m.range(), 0..nasty.len());
    assert_eq!(escape("a.b"), r"a\.b");
    assert_eq!(escape("plain"), "plain");
}

// ---- ReDoS & linear time ---------------------------------------------------------

#[test]
fn redos_nested_quantifier_under_5ms() {
    let r = re("(a+)+b");
    let hay = "a".repeat(1000);
    let elapsed = best_of_3(|| {
        assert!(!r.is_match(&hay));
        assert!(r.find(&hay).is_none());
        assert!(r.captures(&hay).is_none());
    });
    assert!(elapsed < Duration::from_millis(5), "took {elapsed:?}");
}

/// (pattern, haystack, expected match range)
type RedosCase = (&'static str, String, Option<(usize, usize)>);

#[test]
fn redos_classic_patterns() {
    let cases: Vec<RedosCase> = vec![
        ("(a|a)*b", "a".repeat(1000), None),
        ("(a|aa)+$", "a".repeat(1000) + "!", None),
        ("(.*a){12}", "a".repeat(11) + &"b".repeat(5000), None),
        (r"^(\w+\s?)*$", "a ".repeat(2000) + "!", None),
        ("(a?){30}a{30}", "a".repeat(30), Some((0, 30))),
        ("(x+x+)+y", "x".repeat(5000), None),
    ];
    for (p, hay, want) in cases {
        let r = re(p);
        let elapsed = best_of_3(|| {
            assert_eq!(r.find(&hay).map(|m| (m.start(), m.end())), want, "{p:?}");
        });
        assert!(
            elapsed < Duration::from_millis(20),
            "{p:?} took {elapsed:?}"
        );
        // Force the PikeVM (DFA disabled): it must be immune as well.
        let pike = RegexBuilder::new(p).dfa_size_limit(0).build().unwrap();
        let elapsed = best_of_3(|| {
            let got = pike.captures(&hay).map(|c| c.get_match().range());
            assert_eq!(got, want.map(|(s, e)| s..e), "{p:?}");
        });
        assert!(
            elapsed < Duration::from_millis(250),
            "{p:?} PikeVM took {elapsed:?}"
        );
    }
}

#[test]
fn search_time_scales_linearly() {
    for p in ["(a+)+b", "(x+x+)+y"] {
        let r = re(p);
        let unit = if p.starts_with("(a") { "a" } else { "x" };
        let tail = if p.starts_with("(a") { "b" } else { "y" };
        let small = unit.repeat(20_000) + tail;
        let large = unit.repeat(200_000) + tail;
        // Matching haystacks force the PikeVM to run over the whole input.
        let t_small = best_of_3(|| assert!(r.captures(&small).is_some()));
        let t_large = best_of_3(|| assert!(r.captures(&large).is_some()));
        let ratio = t_large.as_secs_f64() / t_small.as_secs_f64().max(1e-6);
        assert!(
            ratio < 25.0,
            "{p:?}: {t_small:?} vs {t_large:?} (ratio {ratio:.1})"
        );
    }
}

#[test]
fn tiny_dfa_cache_falls_back_correctly() {
    let hay = "the quick brown fox ".repeat(500) + "jumps";
    for limit in [0, 64, 1 << 20] {
        let r = RegexBuilder::new(r"(?:\w+\s){3}jumps$")
            .dfa_size_limit(limit)
            .build()
            .unwrap();
        assert!(r.is_match(&hay), "limit {limit}");
        assert_eq!(r.find(&hay).unwrap().end(), hay.len());
        assert!(!r.is_match("no match here"));
    }
}

#[test]
fn large_input_throughput() {
    let hay = "lorem ipsum dolor sit amet 12345 ".repeat(30_000); // ~1 MB
    let r = re(r"\d{5}");
    let t = Instant::now();
    assert_eq!(r.find_iter(&hay).count(), 30_000);
    let r = re(r"zzz\d");
    assert!(!r.is_match(&hay));
    assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
}

// ---- set ---------------------------------------------------------------------------

#[test]
fn regex_set_matches() {
    let set = RegexSet::new([r"\d+", "foo", "^bar", "x{2}"]).unwrap();
    assert_eq!(set.len(), 4);
    assert_eq!(set.patterns()[1], "foo");
    let m = set.matches("foo 42");
    assert_eq!(m.iter().collect::<Vec<_>>(), [0, 1]);
    assert!(m.matched(0) && m.matched(1) && !m.matched(2) && !m.matched(3) && !m.matched(99));
    assert!(m.matched_any() && m.len() == 4);
    for hay in ["", "bar", "xx", "nothing", "foo 42"] {
        assert_eq!(set.is_match(hay), set.matches(hay).matched_any(), "{hay:?}");
    }
    let empty = RegexSet::empty();
    assert!(empty.is_empty() && !empty.is_match("anything") && !empty.matches("x").matched_any());
    let err = RegexSet::new(["a", "("]).unwrap_err();
    assert_eq!(err.kind(), &ErrorKind::UnclosedGroup);
    assert_eq!(err.pattern(), "(");
}

// ---- differential testing against a naive backtracking reference ----------

/// A tiny backtracking reference matcher for a subset of the syntax:
/// literals, `.`, `\w \d \s \W`, `[abc]`, `^ $ \A \z \b \B`, `(?m)` as a
/// leading flag, `( )`, `(?: )`, `|` and the greedy/lazy quantifiers
/// `* + ?` / `*? +? ??`. Exploration order is leftmost-first priority, which
/// is exactly what the PikeVM must reproduce.
mod reference {
    #[derive(Debug)]
    pub enum Node {
        Char(char),
        Any,
        Class(Vec<char>),
        Perl(char),
        Look(&'static str),
        Group(Option<usize>, Box<Node>),
        Concat(Vec<Node>),
        Alt(Vec<Node>),
        Rep {
            sub: Box<Node>,
            min: usize,
            max: Option<usize>,
            greedy: bool,
        },
    }

    pub struct Pattern {
        pub root: Node,
        pub groups: usize,
    }

    pub fn parse(p: &str) -> Pattern {
        let (multi, body) = match p.strip_prefix("(?m)") {
            Some(rest) => (true, rest),
            None => (false, p),
        };
        let chars: Vec<char> = body.chars().collect();
        let mut pos = 0;
        let mut groups = 1;
        let root = alt(&chars, &mut pos, &mut groups, multi);
        assert_eq!(
            pos,
            chars.len(),
            "reference parser: trailing input in {p:?}"
        );
        Pattern { root, groups }
    }

    fn alt(c: &[char], pos: &mut usize, g: &mut usize, m: bool) -> Node {
        let mut items = vec![concat(c, pos, g, m)];
        while c.get(*pos) == Some(&'|') {
            *pos += 1;
            items.push(concat(c, pos, g, m));
        }
        if items.len() == 1 {
            items.pop().unwrap()
        } else {
            Node::Alt(items)
        }
    }

    fn concat(c: &[char], pos: &mut usize, g: &mut usize, m: bool) -> Node {
        let mut items = Vec::new();
        while let Some(&ch) = c.get(*pos) {
            if ch == '|' || ch == ')' {
                break;
            }
            let atom = atom(c, pos, g, m);
            items.push(quantified(c, pos, atom));
        }
        Node::Concat(items)
    }

    fn quantified(c: &[char], pos: &mut usize, atom: Node) -> Node {
        let (min, max) = match c.get(*pos) {
            Some('*') => (0, None),
            Some('+') => (1, None),
            Some('?') => (0, Some(1)),
            _ => return atom,
        };
        *pos += 1;
        let greedy = if c.get(*pos) == Some(&'?') {
            *pos += 1;
            false
        } else {
            true
        };
        Node::Rep {
            sub: Box::new(atom),
            min,
            max,
            greedy,
        }
    }

    fn atom(c: &[char], pos: &mut usize, g: &mut usize, m: bool) -> Node {
        let ch = c[*pos];
        *pos += 1;
        match ch {
            '.' => Node::Any,
            '^' => Node::Look(if m { "^m" } else { "A" }),
            '$' => Node::Look(if m { "$m" } else { "z" }),
            '(' => {
                let index = if c.get(*pos..*pos + 2) == Some(&['?', ':']) {
                    *pos += 2;
                    None
                } else {
                    *g += 1;
                    Some(*g - 1)
                };
                let sub = alt(c, pos, g, m);
                assert_eq!(c.get(*pos), Some(&')'));
                *pos += 1;
                Node::Group(index, Box::new(sub))
            }
            '[' => {
                let mut set = Vec::new();
                while c[*pos] != ']' {
                    set.push(c[*pos]);
                    *pos += 1;
                }
                *pos += 1;
                Node::Class(set)
            }
            '\\' => {
                let e = c[*pos];
                *pos += 1;
                match e {
                    'A' => Node::Look("A"),
                    'z' => Node::Look("z"),
                    'b' => Node::Look("b"),
                    'B' => Node::Look("B"),
                    'w' | 'W' | 'd' | 's' => Node::Perl(e),
                    other => Node::Char(other),
                }
            }
            other => Node::Char(other),
        }
    }

    fn is_word(c: Option<char>) -> bool {
        c.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
    }

    fn look(kind: &str, text: &str, at: usize) -> bool {
        let prev = text[..at].chars().next_back();
        let next = text[at..].chars().next();
        match kind {
            "A" => prev.is_none(),
            "z" => next.is_none(),
            "^m" => prev.is_none_or(|c| c == '\n'),
            "$m" => next.is_none_or(|c| c == '\n'),
            "b" => is_word(prev) != is_word(next),
            "B" => is_word(prev) == is_word(next),
            _ => unreachable!(),
        }
    }

    type Caps = Vec<Option<(usize, usize)>>;
    type Cont<'a> = &'a mut dyn FnMut(usize, &mut Caps) -> bool;

    fn one(text: &str, at: usize, ok: impl Fn(char) -> bool) -> Option<usize> {
        let c = text[at..].chars().next()?;
        ok(c).then(|| at + c.len_utf8())
    }

    fn m(n: &Node, text: &str, at: usize, caps: &mut Caps, k: Cont<'_>) -> bool {
        let step = |next: Option<usize>, caps: &mut Caps, k: Cont<'_>| match next {
            Some(to) => k(to, caps),
            None => false,
        };
        match n {
            Node::Char(x) => step(one(text, at, |c| c == *x), caps, k),
            Node::Any => step(one(text, at, |c| c != '\n'), caps, k),
            Node::Class(set) => step(one(text, at, |c| set.contains(&c)), caps, k),
            Node::Perl(e) => {
                let ok = |c: char| match e {
                    'w' => is_word(Some(c)),
                    'W' => !is_word(Some(c)),
                    'd' => c.is_ascii_digit(),
                    's' => c.is_ascii_whitespace(),
                    _ => unreachable!(),
                };
                step(one(text, at, ok), caps, k)
            }
            Node::Look(kind) => look(kind, text, at) && k(at, caps),
            Node::Group(index, sub) => match index {
                None => m(sub, text, at, caps, k),
                Some(i) => {
                    let i = *i;
                    m(sub, text, at, caps, &mut |end, caps: &mut Caps| {
                        let old = caps[i];
                        caps[i] = Some((at, end));
                        if k(end, caps) {
                            return true;
                        }
                        caps[i] = old;
                        false
                    })
                }
            },
            Node::Concat(items) => seq(items, text, at, caps, k),
            Node::Alt(items) => items.iter().any(|it| m(it, text, at, caps, k)),
            Node::Rep {
                sub,
                min,
                max,
                greedy,
            } => rep(sub, *min, *max, *greedy, 0, text, at, caps, k),
        }
    }

    fn seq(items: &[Node], text: &str, at: usize, caps: &mut Caps, k: Cont<'_>) -> bool {
        match items.split_first() {
            None => k(at, caps),
            Some((head, rest)) => m(head, text, at, caps, &mut |to, caps: &mut Caps| {
                seq(rest, text, to, caps, k)
            }),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn rep(
        sub: &Node,
        min: usize,
        max: Option<usize>,
        greedy: bool,
        count: usize,
        text: &str,
        at: usize,
        caps: &mut Caps,
        k: Cont<'_>,
    ) -> bool {
        let can_stop = count >= min;
        let can_more = max.is_none_or(|x| count < x);
        let more = |caps: &mut Caps, k: Cont<'_>| {
            can_more
                && m(sub, text, at, caps, &mut |to, caps: &mut Caps| {
                    // An iteration that consumed nothing may not loop again.
                    to != at && rep(sub, min, max, greedy, count + 1, text, to, caps, k)
                })
        };
        if !greedy && can_stop && k(at, caps) {
            return true;
        }
        if more(caps, k) {
            return true;
        }
        greedy && can_stop && k(at, caps)
    }

    /// Leftmost-first captures of the first match starting at or after `start`.
    pub fn captures_at(p: &Pattern, text: &str, start: usize) -> Option<Caps> {
        let mut at = start;
        loop {
            let mut caps: Caps = vec![None; p.groups];
            let mut end = None;
            if m(&p.root, text, at, &mut caps, &mut |e, _| {
                end = Some(e);
                true
            }) {
                caps[0] = Some((at, end.unwrap()));
                return Some(caps);
            }
            at += text[at..].chars().next()?.len_utf8();
        }
    }

    /// Mirrors `find_iter`'s empty-match rule.
    pub fn find_iter(p: &Pattern, text: &str) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        let mut at = 0;
        let mut last_end = None;
        while let Some(caps) = captures_at(p, text, at) {
            let (s, e) = caps[0].unwrap();
            if s == e && last_end == Some(e) {
                match text[at..].chars().next() {
                    Some(c) => {
                        at += c.len_utf8();
                        continue;
                    }
                    None => break,
                }
            }
            out.push((s, e));
            at = e;
            last_end = Some(e);
        }
        out
    }
}

#[test]
fn differential_against_backtracking_reference() {
    let patterns = [
        "",
        "a",
        "a*",
        "a+",
        "a*?",
        "a??b",
        "$",
        r"\z",
        "(?m)$",
        "(?m)^",
        "^",
        r"\A",
        "^a",
        "a$",
        "(?m)a$",
        "(?m)^b",
        r"\b",
        r"\B",
        r"\bfoo",
        r"foo\b",
        r"\bfoo\b",
        r"\Bo",
        r"o\B",
        r"\b\w+\b",
        r"\w+$",
        r"\d+",
        r"\s*$",
        "(a)|b",
        "(a|ab)(c|bcd)(d*)",
        "(a+)(b)?",
        "x*$",
        r"(?m)^\w*$",
        r"\b(\w)(\w)?",
        "(?:ab)+",
        "[éa]+",
        ".$",
        ".*",
        "(?m).$",
        r"é\b",
        r"\Wfoo",
        r"(\w+)\s(\w+)",
    ];
    let haystacks = [
        "",
        "a",
        "b",
        "aaa",
        "baaa",
        "foo",
        " foo",
        "afoo",
        "foo.",
        "foo bar foo",
        "a\nb\n",
        "a\nb",
        "\n",
        "\n\n",
        "ab\ncd",
        "abcd",
        "x é foo",
        "é",
        "éfoo é",
        "héllo wörld",
        "12 ab 345",
        "  ",
        "ab ",
        "_x_ y",
    ];
    for p in patterns {
        let r = re(p);
        let rp = reference::parse(p);
        for h in haystacks {
            let want = reference::captures_at(&rp, h, 0);
            let got = r.captures(h).map(|c| {
                (0..c.len())
                    .map(|i| c.get(i).map(|m| (m.start(), m.end())))
                    .collect::<Vec<_>>()
            });
            assert_eq!(got, want, "captures({p:?}, {h:?})");
            let want_find = want.as_ref().map(|c| c[0].unwrap());
            assert_eq!(find(p, h), want_find, "find({p:?}, {h:?})");
            assert_eq!(r.is_match(h), want_find.is_some(), "is_match({p:?}, {h:?})");
            assert_eq!(
                all(p, h),
                reference::find_iter(&rp, h),
                "find_iter({p:?}, {h:?})"
            );
            let starts: Vec<usize> = (0..=h.len()).filter(|&i| h.is_char_boundary(i)).collect();
            for &s in &starts {
                let want_at = reference::captures_at(&rp, h, s).map(|c| c[0].unwrap());
                let got_at = r.find_at(h, s).map(|m| (m.start(), m.end()));
                assert_eq!(got_at, want_at, "find_at({p:?}, {h:?}, {s})");
                assert_eq!(
                    r.is_match_at(h, s),
                    want_at.is_some(),
                    "is_match_at({p:?}, {h:?}, {s})"
                );
            }
        }
    }
}

#[test]
fn failed_seed_does_not_end_unanchored_search() {
    // Regression: the PikeVM stopped as soon as a seed produced no thread.
    assert_eq!(find("$", "ab"), Some((2, 2)));
    assert_eq!(find(r"\z", "ab"), Some((2, 2)));
    assert_eq!(find(r"\bfoo", " foo"), Some((1, 4)));
    assert_eq!(find(r"\B", "a b"), None);
    assert_eq!(find(r"\B", "ab"), Some((1, 1)));
    assert_eq!(find(r"(?m)^b", "a\nb"), Some((2, 3)));
    assert_eq!(all("(?m)$", "ab\ncd"), [(2, 2), (5, 5)]);
    assert_eq!(all(r"\b", " a "), [(1, 1), (2, 2)]);
    assert_eq!(re("$").replace_all("abc", "!"), "abc!");
    assert!(re(r"\bfoo").captures(" foo").is_some());
}

#[test]
fn replacement_group_number_overflow_does_not_leak_group_zero() {
    let r = re(r"(\w+)");
    let caps = r.captures("hello").unwrap();
    for rep in [
        "$9223372036854775808",
        "${9223372036854775808}",
        "$18446744073709551615",
        "${18446744073709551615}",
        "${99999999999999999999}",
        "$99999999999999999999",
        "${9223372036854775809}",
    ] {
        let mut out = String::new();
        caps.expand(rep, &mut out);
        assert_eq!(out, "", "expand {rep:?}");
        assert_eq!(r.replace("hello", rep), "", "replace {rep:?}");
    }
    assert!(caps.get(usize::MAX).is_none());
    assert!(caps.get(usize::MAX / 2 + 1).is_none());
    assert!(caps.get(usize::MAX / 2).is_none());
}

#[test]
fn malformed_pattern_limits_never_panic() {
    for p in [
        "a{4294967295}",
        "a{4294967296}",
        "a{99999999999999999999}",
        "a{0,4294967295}",
        "a{4294967295,}",
        "(?:a{1000}){1000}",
        "((((((((((a*)*)*)*)*)*)*)*)*)*)*",
        "(?:a+)+(?:b+)+(?:c+)+",
    ] {
        let _ = Regex::new(p);
    }
    let deep = "(".repeat(100_000) + &")".repeat(100_000);
    assert!(Regex::new(&deep).is_err());
    let deep = "(?:".repeat(100_000) + "a" + &")*".repeat(100_000);
    assert!(Regex::new(&deep).is_err());
    let deep = "a*".repeat(50_000);
    let _ = Regex::new(&deep);
    // At the hard nest ceiling everything (compile, analyses, drop) must
    // still work on the default test-thread stack.
    let n = 1000;
    let max = "(".repeat(n) + "a" + &")".repeat(n);
    let r = RegexBuilder::new(&max).nest_limit(1000).build();
    if let Ok(r) = r {
        assert!(r.is_match("a"));
        assert_eq!(r.captures("a").unwrap().len(), n + 1);
    }
    let max = "(?:a*".repeat(n / 2) + &")*".repeat(n / 2);
    let _ = RegexBuilder::new(&max).nest_limit(1000).build();
}
