use akku::ini::{self, DuplicatePolicy, ErrorKind, Ini, IniOptions};

#[test]
fn global_and_sections_insertion_order() {
    let input = "\
\u{FEFF}; leading comment\r
app = akku\r
version = 1\r
\r
[server]\r
host = 127.0.0.1\r
port = 8080\r
\r
[database]\r
url = postgres://localhost/akku\r
pool = 16\r
";

    let doc = ini::parse(input).unwrap();
    assert!(!doc.is_empty());
    assert_eq!(doc.len(), 2);
    assert_eq!(doc.get("app"), Some("akku"));
    assert_eq!(doc.global().get("version"), Some("1"));
    assert_eq!(doc.get("server.host"), Some("127.0.0.1"));
    assert_eq!(doc.get_int("server.port").unwrap(), 8080);
    assert_eq!(
        doc.section_names().collect::<Vec<_>>(),
        vec!["server", "database"]
    );

    let server = doc.section("server").unwrap();
    assert_eq!(server.name(), "server");
    assert_eq!(&server["host"], "127.0.0.1");
    assert_eq!(server.keys().collect::<Vec<_>>(), vec!["host", "port"]);
    assert_eq!(
        server.iter().collect::<Vec<_>>(),
        vec![("host", "127.0.0.1"), ("port", "8080")]
    );
}

#[test]
fn duplicate_policies() {
    let input = "[sec]\na = 1\nb = 2\na = 3\n";

    let last = ini::parse_with(
        input,
        IniOptions {
            duplicate: DuplicatePolicy::LastWins,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(last.get("sec.a"), Some("3"));
    assert_eq!(
        last.section("sec").unwrap().keys().collect::<Vec<_>>(),
        vec!["a", "b"]
    );

    let first = ini::parse_with(
        input,
        IniOptions {
            duplicate: DuplicatePolicy::FirstWins,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(first.get("sec.a"), Some("1"));

    let err = ini::parse_with(
        input,
        IniOptions {
            duplicate: DuplicatePolicy::Error,
            ..Default::default()
        },
    )
    .unwrap_err();
    assert_eq!(err.line(), Some(4));
    assert!(matches!(
        err.kind(),
        ErrorKind::DuplicateKey {
            section: Some(sec),
            key,
        } if sec == "sec" && key == "a"
    ));

    // Global duplicate error
    let g_err = ini::parse_with(
        "k = 1\nk = 2\n",
        IniOptions {
            duplicate: DuplicatePolicy::Error,
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(matches!(
        g_err.kind(),
        ErrorKind::DuplicateKey {
            section: None,
            key,
        } if key == "k"
    ));
}

#[test]
fn case_insensitive_lookup_and_merging() {
    let input = "[Server]\nHost = localhost\n\n[server]\nport = 3000\nhost = 0.0.0.0\n";
    let doc = ini::parse_with(
        input,
        IniOptions {
            case_sensitive: false,
            ..Default::default()
        },
    )
    .unwrap();

    assert_eq!(doc.len(), 1);
    assert_eq!(doc.get("SERVER.HOST"), Some("0.0.0.0"));
    assert_eq!(doc.get("server.Port"), Some("3000"));
    assert!(doc.section("sErVeR").unwrap().contains_key("HOST"));
}

#[test]
fn keys_containing_dots_split_on_first_dot_only() {
    let input = "[s]\na.b = dotted\na.b.c = deeper\n";
    let doc = ini::parse(input).unwrap();
    assert_eq!(doc.get("s.a.b"), Some("dotted"));
    assert_eq!(doc.get("s.a.b.c"), Some("deeper"));
    assert_eq!(doc.section("s").unwrap().get("a.b"), Some("dotted"));
}

#[test]
fn inline_comments_toggle() {
    let input = "[sec]\ncolor = #ff0000\nurl = http://x#frag ; note\n";

    let off = ini::parse_with(
        input,
        IniOptions {
            inline_comments: false,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(off.get("sec.color"), Some("#ff0000"));
    assert_eq!(off.get("sec.url"), Some("http://x#frag ; note"));

    let on = ini::parse_with(
        input,
        IniOptions {
            inline_comments: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(on.get("sec.color"), Some(""));
    assert_eq!(on.get("sec.url"), Some("http://x"));
}

#[test]
fn quotes_and_escapes_and_errors() {
    let input = r#"
[q]
dbl = "hello \"world\"\nline2\t\\\; \#"
sgl = 'literal \n # ; keep'
padded = "  spaced  "
"#;
    let doc = ini::parse(input).unwrap();
    assert_eq!(doc.get("q.dbl"), Some("hello \"world\"\nline2\t\\; #"));
    assert_eq!(doc.get("q.sgl"), Some(r"literal \n # ; keep"));
    assert_eq!(doc.get("q.padded"), Some("  spaced  "));

    let unterminated = ini::parse("[q]\na = \"unclosed\n").unwrap_err();
    assert_eq!(unterminated.line(), Some(2));
    assert!(matches!(unterminated.kind(), ErrorKind::UnterminatedQuote));

    let bad_esc = ini::parse("[q]\na = \"bad\\q\"\n").unwrap_err();
    assert_eq!(bad_esc.line(), Some(2));
    assert!(matches!(
        bad_esc.kind(),
        ErrorKind::InvalidEscape { escape } if escape == "\\q"
    ));
}

#[test]
fn section_header_and_parse_errors() {
    let nested_dot = ini::parse("[a.b]\nk = v\n").unwrap_err();
    assert_eq!(nested_dot.line(), Some(1));
    assert!(matches!(nested_dot.kind(), ErrorKind::NestedSection));

    let nested_bracket = ini::parse("[a[b]]\nk = v\n").unwrap_err();
    assert!(matches!(nested_bracket.kind(), ErrorKind::NestedSection));

    let empty = ini::parse("[]\n").unwrap_err();
    assert!(matches!(empty.kind(), ErrorKind::EmptySectionName));

    let unclosed = ini::parse("[abc\n").unwrap_err();
    assert!(matches!(unclosed.kind(), ErrorKind::UnclosedSection));

    let no_delim = ini::parse("just_a_word\n").unwrap_err();
    assert!(matches!(
        no_delim.kind(),
        ErrorKind::MissingDelimiter { line: 1 }
    ));

    let empty_key = ini::parse("= value\n").unwrap_err();
    assert!(matches!(empty_key.kind(), ErrorKind::InvalidKey { key } if key.is_empty()));

    let no_global = ini::parse_with(
        "k = v\n",
        IniOptions {
            allow_global: false,
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(matches!(no_global.kind(), ErrorKind::InvalidKey { key } if key == "k"));
}

#[test]
fn typed_getters_and_mutation_and_merge() {
    let mut doc: Ini = "\
[types]
num = -42
ratio = 3.5
flag_yes = YES
flag_off = off
items = alpha, beta , gamma
bad_int = nope
"
    .parse()
    .unwrap();

    assert_eq!(doc.get_int("types.num").unwrap(), -42);
    assert!((doc.get_float("types.ratio").unwrap() - 3.5).abs() < 1e-12);
    assert!(doc.get_bool("types.flag_yes").unwrap());
    assert!(!doc.get_bool("types.flag_off").unwrap());
    assert_eq!(
        doc.get_list("types.items").unwrap(),
        vec!["alpha", "beta", "gamma"]
    );

    assert!(matches!(
        doc.get_int("types.missing").unwrap_err().kind(),
        ErrorKind::MissingKey { path } if path == "types.missing"
    ));
    assert!(matches!(
        doc.get_int("types.bad_int").unwrap_err().kind(),
        ErrorKind::WrongType { path, expected, found }
        if path == "types.bad_int" && *expected == "integer" && found == "nope"
    ));

    // In-place order preservation on set()
    let sec = doc.section_mut("types").unwrap();
    sec.set("num", "99");
    assert_eq!(sec.keys().next(), Some("num"));
    assert_eq!(sec.remove("bad_int"), Some("nope".into()));

    let mut overlay = Ini::new();
    overlay.global_mut().set("env", "prod");
    overlay.section_or_insert("types").set("num", "100");
    doc.merge(&overlay);

    assert_eq!(doc.get("env"), Some("prod"));
    assert_eq!(doc.get_int("types.num").unwrap(), 100);
}

#[test]
fn round_trip_serialization() {
    let mut ini = Ini::new();
    ini.global_mut().set("global_key", "hello # world");
    ini.global_mut().set("empty", "");

    let sec = ini.section_or_insert("app");
    sec.set("plain", "value");
    sec.set("spaced", "  lead and trail  ");
    sec.set("multiline", "line1\nline2\t\"quoted\"\\");
    sec.set("semicolon", "a;b");
    sec.set("a.b", "dotted");

    let serialized = ini::to_string(&ini);
    assert_eq!(serialized, ini.to_string());
    let reparsed = ini::parse(&serialized).unwrap();
    assert_eq!(reparsed, ini);

    let mut buf = Vec::new();
    ini::write_to(&ini, &mut buf).unwrap();
    assert_eq!(String::from_utf8(buf).unwrap(), serialized);
}
