//! Integration tests for `akku::yaml`.

use akku::yaml::{
    parse, parse_all, parse_with, to_string, to_string_with, write_all_to, Documents, ErrorKind,
    Mapping, ParseOptions, QuoteStyle, Value, WriteOptions,
};

/// Parse one document and expect success.
fn doc(input: &str) -> Value {
    parse(input).expect("input parses")
}

/// Parse one document and expect a specific error kind.
fn failure(input: &str) -> akku::yaml::Error {
    parse(input).expect_err("input must fail")
}

/// Run `action` on a thread with a deliberately small stack.
fn on_small_stack<F: FnOnce() + Send + 'static>(action: F) {
    std::thread::Builder::new()
        .stack_size(128 * 1024)
        .spawn(action)
        .expect("thread spawns")
        .join()
        .expect("action does not panic");
}

#[test]
fn write_options_are_deterministic() {
    let value = doc("b: 2\na:\n  - 1\n  - x: true\n");

    let sorted = to_string_with(
        &value,
        WriteOptions {
            sort_keys: true,
            ..WriteOptions::new()
        },
    );
    let a = sorted.find("a:").expect("a is written");
    let b = sorted.find("b:").expect("b is written");
    assert!(a < b, "sorted output puts a before b: {sorted:?}");
    assert_eq!(parse(&sorted).expect("sorted re-parses"), value);

    let wide = to_string_with(
        &value,
        WriteOptions {
            indent: 4,
            ..WriteOptions::new()
        },
    );
    assert!(wide.contains("    - 1"), "indent is honored: {wide:?}");
    assert_eq!(parse(&wide).expect("wide re-parses"), value);

    let framed = to_string_with(
        &value,
        WriteOptions {
            explicit_start: true,
            document_end: true,
            ..WriteOptions::new()
        },
    );
    assert!(framed.starts_with("---\n"), "{framed:?}");
    assert!(framed.trim_end().ends_with("..."), "{framed:?}");
    assert_eq!(parse(&framed).expect("framed re-parses"), value);

    // Every quoting style keeps the string a string.
    for style in [QuoteStyle::Single, QuoteStyle::Double] {
        let rendered = to_string_with(
            &Value::String("plain text".into()),
            WriteOptions {
                quote_style: style,
                ..WriteOptions::new()
            },
        );
        assert_eq!(
            parse(&rendered).expect("quoted re-parses"),
            Value::String("plain text".into()),
            "{style:?} rendered {rendered:?}"
        );
    }

    // Special floats survive byte for byte in meaning.
    for number in [
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NAN,
        -0.0,
        1.0e300,
        f64::MIN_POSITIVE,
    ] {
        let value = Value::Float(number);
        let rendered = to_string(&value);
        assert_eq!(
            parse(&rendered).expect("float re-parses"),
            value,
            "{number} rendered as {rendered:?}"
        );
    }

    // A one space string must not become an empty one.
    assert_eq!(
        parse(&to_string(&Value::String(" ".into()))).expect("space survives"),
        Value::String(" ".into())
    );
}

/// A mapping built from string keys and simple values.
fn map_of(pairs: &[(&str, Value)]) -> Value {
    let mut mapping = Mapping::new();
    for (key, value) in pairs {
        mapping.insert_str(key, value.clone());
    }
    Value::Mapping(mapping)
}

// ---------------------------------------------------------------- value model

#[test]
fn value_type_names_and_predicates() {
    assert_eq!(Value::Null.type_name(), "null");
    assert_eq!(Value::Bool(true).type_name(), "bool");
    assert_eq!(Value::Int(1).type_name(), "int");
    assert_eq!(Value::Float(1.0).type_name(), "float");
    assert_eq!(Value::String("x".into()).type_name(), "string");
    assert_eq!(Value::Sequence(Vec::new()).type_name(), "sequence");
    assert_eq!(Value::Mapping(Mapping::new()).type_name(), "mapping");

    assert!(Value::Int(1).is_number());
    assert!(Value::Float(1.0).is_number());
    assert!(!Value::String("1".into()).is_number());
    assert!(Value::Null.is_null());
}

#[test]
fn mapping_preserves_insertion_order() {
    let value = doc("z: 1\na: 2\nm: 3\n");
    let keys: Vec<&str> = value
        .as_mapping()
        .expect("mapping")
        .keys()
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(keys, ["z", "a", "m"]);
}

#[test]
fn mapping_insert_replaces_and_keeps_position() {
    let mut mapping = Mapping::new();
    assert_eq!(mapping.insert(Value::Int(1), Value::from("x")), None);
    assert_eq!(
        mapping.insert(Value::Int(1), Value::from("y")),
        Some(Value::from("x"))
    );
    assert_eq!(mapping.len(), 1);
    let first = mapping.keys().next().expect("one key");
    assert_eq!(first, &Value::Int(1));
    assert_eq!(mapping.get(&Value::Int(1)), Some(&Value::from("y")));
}

#[test]
fn mapping_equality_ignores_order() {
    let mut left = Mapping::new();
    left.insert_str("a", 1i64);
    left.insert_str("b", 2i64);
    let mut right = Mapping::new();
    right.insert_str("b", 2i64);
    right.insert_str("a", 1i64);
    assert_eq!(Value::Mapping(left), Value::Mapping(right));
}

#[test]
fn mapping_typed_keys_are_distinct() {
    let value = doc("1: a\n\"1\": b\ntrue: c\n");
    let mapping = value.as_mapping().expect("mapping");
    assert_eq!(mapping.len(), 3);
    assert_eq!(mapping.get_str("1"), Some(&Value::from("b")));
    assert_eq!(mapping.get(&Value::Int(1)), Some(&Value::from("a")));
    assert_eq!(mapping.get(&Value::Bool(true)), Some(&Value::from("c")));
}

// -------------------------------------------------------------- core schema

#[test]
fn norway_problem_yes_no_are_strings() {
    for literal in [
        "NO", "no", "No", "YES", "yes", "Yes", "On", "on", "OFF", "off", "y", "n",
    ] {
        let value = doc(&format!("key: {literal}\n"));
        assert_eq!(
            value.get("key").and_then(Value::as_str),
            Some(literal),
            "{literal} must stay a string"
        );
    }
}

#[test]
fn strict_booleans() {
    for literal in ["true", "True", "TRUE"] {
        assert_eq!(
            doc(&format!("key: {literal}\n")).get("key"),
            Some(&Value::Bool(true))
        );
    }
    for literal in ["false", "False", "FALSE"] {
        assert_eq!(
            doc(&format!("key: {literal}\n")).get("key"),
            Some(&Value::Bool(false))
        );
    }
    assert_eq!(
        doc("key: True\n").get("key").and_then(Value::as_str),
        None,
        "True must not be a string"
    );

    // A YAML 1.1 directive does not switch the schema back to 1.1 types.
    assert_eq!(
        doc("%YAML 1.1\n---\nkey: no\n")
            .get("key")
            .and_then(Value::as_str),
        Some("no")
    );
}

#[test]
fn null_forms() {
    for literal in ["null", "Null", "NULL", "~"] {
        assert_eq!(
            doc(&format!("key: {literal}\n")).get("key"),
            Some(&Value::Null)
        );
    }
    assert_eq!(doc("key:\n").get("key"), Some(&Value::Null));
}

#[test]
fn integer_forms() {
    assert_eq!(doc("key: 0\n").get("key"), Some(&Value::Int(0)));
    assert_eq!(doc("key: -17\n").get("key"), Some(&Value::Int(-17)));
    assert_eq!(doc("key: +42\n").get("key"), Some(&Value::Int(42)));
    assert_eq!(doc("key: 0x1F\n").get("key"), Some(&Value::Int(31)));
    assert_eq!(doc("key: 0o17\n").get("key"), Some(&Value::Int(15)));
    // A leading zero is decimal in YAML 1.2, never octal.
    assert_eq!(doc("key: 010\n").get("key"), Some(&Value::Int(10)));
    // Binary is a YAML 1.1 form and stays a string in the core schema.
    assert_eq!(
        doc("key: 0b1010\n").get("key").and_then(Value::as_str),
        Some("0b1010")
    );
    // Underscore separators are YAML 1.1 as well.
    assert_eq!(
        doc("key: 1_000\n").get("key").and_then(Value::as_str),
        Some("1_000")
    );
    // An integer that does not fit i64 widens to a float.
    assert!(doc("key: 9223372036854775808\n")
        .get("key")
        .is_some_and(Value::is_float));
}

#[test]
fn float_forms() {
    assert_eq!(doc("key: 1.0\n").get("key"), Some(&Value::Float(1.0)));
    assert_eq!(
        doc("key: -2.5e3\n").get("key"),
        Some(&Value::Float(-2500.0))
    );
    assert_eq!(doc("key: 1e3\n").get("key"), Some(&Value::Float(1000.0)));
    assert_eq!(doc("key: .5\n").get("key"), Some(&Value::Float(0.5)));
    assert_eq!(doc("key: 1.\n").get("key"), Some(&Value::Float(1.0)));
    for literal in [".inf", ".Inf", ".INF"] {
        assert_eq!(
            doc(&format!("key: {literal}\n")).get("key"),
            Some(&Value::Float(f64::INFINITY))
        );
    }
    assert_eq!(
        doc("key: -.inf\n").get("key"),
        Some(&Value::Float(f64::NEG_INFINITY))
    );
    assert!(doc("key: .nan\n")
        .get("key")
        .and_then(Value::as_f64)
        .is_some_and(f64::is_nan));
}

#[test]
fn quoted_scalars_never_resolve() {
    assert_eq!(
        doc("key: \"true\"\n").get("key").and_then(Value::as_str),
        Some("true")
    );
    assert_eq!(
        doc("key: '42'\n").get("key").and_then(Value::as_str),
        Some("42")
    );
    assert_eq!(
        doc("key: 'null'\n").get("key").and_then(Value::as_str),
        Some("null")
    );
    assert_eq!(
        doc("key: 'it''s'\n").get("key").and_then(Value::as_str),
        Some("it's")
    );
    assert_eq!(
        doc("key: \"a\\tb\\u00e9\\x41\\U0001F600\"\n")
            .get("key")
            .and_then(Value::as_str),
        Some("a\tbéA😀")
    );
}

#[test]
fn explicit_tags_force_types() {
    assert_eq!(
        doc("key: !!str 42\n").get("key").and_then(Value::as_str),
        Some("42")
    );
    assert_eq!(doc("key: !!int \"42\"\n").get("key"), Some(&Value::Int(42)));
    assert_eq!(doc("key: !!float 3\n").get("key"), Some(&Value::Float(3.0)));
    assert_eq!(
        doc("key: !!bool true\n").get("key"),
        Some(&Value::Bool(true))
    );
    assert_eq!(doc("key: !!null ~\n").get("key"), Some(&Value::Null));
    assert_eq!(
        doc("key: !<tag:yaml.org,2002:str> 7\n")
            .get("key")
            .and_then(Value::as_str),
        Some("7")
    );

    let mismatch = failure("key: !!int abc\n");
    assert!(matches!(mismatch.kind(), ErrorKind::TagMismatch { .. }));

    let unknown = failure("key: !foo bar\n");
    assert!(matches!(unknown.kind(), ErrorKind::InvalidTag { .. }));
}

// ---------------------------------------------------------- block structures

#[test]
fn block_mapping_basic() {
    let value = doc("# a comment\nname: akku\nnested:\n  a: 1\n  b: {c: 2}\n\n");
    assert_eq!(value.get("name").and_then(Value::as_str), Some("akku"));
    assert_eq!(value.get_path("nested.a"), Some(&Value::Int(1)));
    assert_eq!(value.get_path("nested.b.c"), Some(&Value::Int(2)));

    // A plain scalar continues on a more indented line and folds with a space.
    assert_eq!(
        doc("text: one\n  two\n")
            .get("text")
            .and_then(Value::as_str),
        Some("one two")
    );
    // Inline comments end a value.
    assert_eq!(
        doc("count: 7  # seven\n").get("count"),
        Some(&Value::Int(7))
    );
}

#[test]
fn block_sequence_basic() {
    let value = doc("- a\n- 5\n- - x\n-\n");
    let items = value.as_sequence().expect("sequence");
    assert_eq!(items.len(), 4);
    assert_eq!(items[0], Value::from("a"));
    assert_eq!(items[1], Value::Int(5));
    assert_eq!(items[2], Value::Sequence(vec![Value::from("x")]));
    assert_eq!(items[3], Value::Null);
}

#[test]
fn sequence_at_key_indentation() {
    let value = doc("key:\n- a\n- b\n");
    let items = value
        .get("key")
        .and_then(Value::as_sequence)
        .expect("sequence");
    assert_eq!(items, &[Value::from("a"), Value::from("b")]);
}

#[test]
fn compact_mapping_in_sequence() {
    let value = doc("- id: 1\n  tags: [a]\n- id: 2\n");
    let items = value.as_sequence().expect("sequence");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].get_path("id"), Some(&Value::Int(1)));
    assert_eq!(
        items[0].get_path("tags.0"),
        Some(&Value::String("a".into()))
    );
    assert_eq!(items[1].get_path("id"), Some(&Value::Int(2)));
}

#[test]
fn deeply_structured_document() {
    let value = doc(
        "root:\n  items:\n    - name: one\n      meta:\n        tags: [x, y]\n    - name: two\n",
    );
    let expected = map_of(&[(
        "root",
        map_of(&[(
            "items",
            Value::Sequence(vec![
                map_of(&[
                    ("name", Value::from("one")),
                    (
                        "meta",
                        map_of(&[(
                            "tags",
                            Value::Sequence(vec![Value::from("x"), Value::from("y")]),
                        )]),
                    ),
                ]),
                map_of(&[("name", Value::from("two"))]),
            ]),
        )]),
    )]);
    assert_eq!(value, expected);
}

// --------------------------------------------------------------------- flow

#[test]
fn flow_collections() {
    let value = doc("key: [1, two, {a: b}]\n");
    assert_eq!(value.get_path("key.0"), Some(&Value::Int(1)));
    assert_eq!(value.get_path("key.1"), Some(&Value::String("two".into())));
    assert_eq!(value.get_path("key.2.a"), Some(&Value::String("b".into())));
}

#[test]
fn flow_json_style_and_trailing_comma() {
    let json = doc("key: {\"a\":1,\"b\":2}\n");
    assert_eq!(json.get_path("key.a"), Some(&Value::Int(1)));
    assert_eq!(json.get_path("key.b"), Some(&Value::Int(2)));

    assert_eq!(
        doc("key: [1,]\n").get("key"),
        Some(&Value::Sequence(vec![Value::Int(1)]))
    );
    assert_eq!(
        doc("key: {a: 1,}\n").get("key"),
        Some(&map_of(&[("a", Value::Int(1))]))
    );
    assert_eq!(
        doc("key: []\n").get("key"),
        Some(&Value::Sequence(Vec::new()))
    );
    assert_eq!(
        doc("key: {}\n").get("key"),
        Some(&Value::Mapping(Mapping::new()))
    );
}

#[test]
fn block_scalar_in_flow_is_rejected() {
    let error = failure("key: [a: |\n  x\n]\n");
    assert!(matches!(error.kind(), ErrorKind::BlockScalarInFlow));
}

// ------------------------------------------------------------ block scalars

#[test]
fn literal_clip() {
    assert_eq!(
        doc("a: |\n  x\n  y\n").get("a").and_then(Value::as_str),
        Some("x\ny\n")
    );
}

#[test]
fn literal_strip() {
    assert_eq!(
        doc("a: |-\n  x\n  y\n").get("a").and_then(Value::as_str),
        Some("x\ny")
    );
}

#[test]
fn literal_keep() {
    assert_eq!(
        doc("a: |+\n  x\n\n\n").get("a").and_then(Value::as_str),
        Some("x\n\n\n")
    );
}

#[test]
fn folded_clip() {
    assert_eq!(
        doc("a: >\n  one\n  two\n\n  three\n")
            .get("a")
            .and_then(Value::as_str),
        Some("one two\nthree\n")
    );
    // More indented lines keep their breaks.
    assert_eq!(
        doc("a: >\n  one\n    indented\n  two\n")
            .get("a")
            .and_then(Value::as_str),
        Some("one\n  indented\ntwo\n")
    );
}

#[test]
fn folded_strip_and_keep() {
    assert_eq!(
        doc("a: >-\n  one\n  two\n")
            .get("a")
            .and_then(Value::as_str),
        Some("one two")
    );
    assert_eq!(
        doc("a: >+\n  x\n\n").get("a").and_then(Value::as_str),
        Some("x\n\n")
    );
}

#[test]
fn explicit_indentation_indicator() {
    assert_eq!(
        doc("a: |2\n   x\n").get("a").and_then(Value::as_str),
        Some(" x\n")
    );
    assert_eq!(
        doc("a: |2\n\n   x\n").get("a").and_then(Value::as_str),
        Some("\n x\n")
    );
    assert_eq!(
        doc("a: >1-\n x\n y\n").get("a").and_then(Value::as_str),
        Some("x y")
    );
}

// ------------------------------------------------------------ multi document

#[test]
fn parse_all_two_documents() {
    let documents = parse_all("---\na: 1\n---\nb: 2\n").expect("stream parses");
    assert_eq!(documents.len(), 2);
    assert_eq!(documents[0].get("a"), Some(&Value::Int(1)));
    assert_eq!(documents[1].get("b"), Some(&Value::Int(2)));
}

#[test]
fn parse_all_document_end_marker() {
    let documents = parse_all("a: 1\n...\n---\nb: 2\n...\n").expect("stream parses");
    assert_eq!(documents.len(), 2);
    assert_eq!(documents[0].get("a"), Some(&Value::Int(1)));
    assert_eq!(documents[1].get("b"), Some(&Value::Int(2)));
}

#[test]
fn parse_rejects_second_document() {
    let error = failure("---\na: 1\n---\nb: 2\n");
    assert!(matches!(error.kind(), ErrorKind::MultipleDocuments));
    assert_eq!(error.line(), 3);
    assert_eq!(error.column(), 1);
}

#[test]
fn empty_and_null_documents() {
    assert!(parse_all("").expect("empty stream").is_empty());
    assert_eq!(
        parse_all("---\n---\n").expect("two empty documents"),
        vec![Value::Null, Value::Null]
    );
    assert_eq!(doc(""), Value::Null);
    assert_eq!(doc("# only a comment\n\n"), Value::Null);
    assert_eq!(
        Documents::new("---\na: 1\n---\nb: 2\n").count(),
        2,
        "the iterator agrees with parse_all"
    );
}

// -------------------------------------------------- anchors, aliases, merge

#[test]
fn anchors_and_aliases() {
    let value = doc("base: &base\n  x: 1\nfirst: *base\nsecond: *base\n");
    assert_eq!(value.get("first"), value.get("base"));
    assert_eq!(value.get("second"), value.get("base"));
    // The aliases are independent copies, not shared references.
    assert_eq!(
        value.get("first").map(Value::node_count),
        value.get("base").map(Value::node_count)
    );
}

#[test]
fn merge_key_single_and_list() {
    let single = doc("base: &base\n  a: 1\n  b: 2\nderived:\n  <<: *base\n  b: 3\n");
    assert_eq!(single.get_path("derived.a"), Some(&Value::Int(1)));
    assert_eq!(single.get_path("derived.b"), Some(&Value::Int(3)));
    assert!(
        single
            .get("derived")
            .and_then(Value::as_mapping)
            .is_some_and(|mapping| !mapping.contains_key(&Value::from("<<"))),
        "the merge key itself does not survive"
    );

    let list = doc("one: &one {a: 1, b: 1}\ntwo: &two {a: 2, c: 2}\nmerged:\n  <<: [*one, *two]\n");
    assert_eq!(list.get_path("merged.a"), Some(&Value::Int(1)));
    assert_eq!(list.get_path("merged.b"), Some(&Value::Int(1)));
    assert_eq!(list.get_path("merged.c"), Some(&Value::Int(2)));
}

#[test]
fn merge_key_validation() {
    let error = failure("derived:\n  <<: 42\n");
    assert!(matches!(error.kind(), ErrorKind::MergeNotMapping { .. }));

    let literal = parse_with(
        "derived:\n  <<: 42\n",
        ParseOptions {
            merge_keys: false,
            ..ParseOptions::new()
        },
    )
    .expect("merge keys disabled");
    assert_eq!(literal.get_path("derived.<<"), Some(&Value::Int(42)));
}

#[test]
fn undefined_and_self_referential_alias() {
    let undefined = failure("a: *nope\n");
    assert!(matches!(undefined.kind(), ErrorKind::AnchorNotFound { .. }));

    let recursive = failure("a: &a [*a]\n");
    assert!(matches!(recursive.kind(), ErrorKind::AnchorNotFound { .. }));

    let limited = parse_with(
        "a: &a x\nb: *a\nc: *a\nd: *a\n",
        ParseOptions {
            max_aliases: 2,
            ..ParseOptions::new()
        },
    );
    assert!(matches!(
        limited.expect_err("alias limit").kind(),
        ErrorKind::AliasLimitExceeded { limit: 2 }
    ));
}

#[test]
fn billion_laughs_rejected() {
    let mut source = String::from("a: &a [x, x, x, x, x, x, x, x, x]\n");
    for (level, name) in ["b", "c", "d", "e", "f"].iter().enumerate() {
        let previous = (b'a' + level as u8) as char;
        source.push_str(&format!(
            "{name}: &{name} [*{previous}, *{previous}, *{previous}, *{previous}, *{previous}, *{previous}, *{previous}, *{previous}, *{previous}]\n"
        ));
    }
    let started = std::time::Instant::now();
    let error = parse(&source).expect_err("alias bomb must be rejected");
    assert!(
        matches!(error.kind(), ErrorKind::AliasExpansionLimitExceeded { .. }),
        "unexpected kind: {:?}",
        error.kind()
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(2),
        "the bomb is rejected before it is expanded"
    );
}

// ------------------------------------------------------------------- errors

#[test]
fn error_line_and_column() {
    let eof = failure("a: 1\nb: [1, 2\n");
    assert!(matches!(eof.kind(), ErrorKind::UnexpectedEof));
    assert_eq!(eof.line(), 3);
    assert_eq!(eof.column(), 1);

    let indentation = failure("a:\n  b: 1\n c: 2\n");
    assert!(
        matches!(indentation.kind(), ErrorKind::BadIndentation { .. }),
        "unexpected kind: {:?}",
        indentation.kind()
    );
    assert_eq!(indentation.line(), 3);
    assert_eq!(indentation.column(), 2);
}

#[test]
fn error_path_points_at_failing_value() {
    let error = failure("server:\n  port: !!int abc\n");
    assert!(matches!(error.kind(), ErrorKind::TagMismatch { .. }));
    assert_eq!(error.path(), "server.port");
}

#[test]
fn tab_indentation_rejected() {
    let error = failure("a: 1\n\tb: 2\n");
    assert!(matches!(error.kind(), ErrorKind::TabIndentation));
    assert_eq!(error.line(), 2);
    assert_eq!(error.column(), 1);

    // A tab inside content is fine.
    assert_eq!(
        doc("a: b\tc\n").get("a").and_then(Value::as_str),
        Some("b\tc")
    );
}

#[test]
fn duplicate_keys() {
    let error = failure("a: 1\na: 2\n");
    assert!(matches!(error.kind(), ErrorKind::DuplicateKey { key } if key == "a"));
    assert_eq!(error.line(), 2);
    assert_eq!(error.column(), 1);

    let allowed = parse_with(
        "a: 1\na: 2\n",
        ParseOptions {
            allow_duplicate_keys: true,
            ..ParseOptions::new()
        },
    )
    .expect("duplicates allowed");
    assert_eq!(allowed.get("a"), Some(&Value::Int(2)));
    assert_eq!(allowed.as_mapping().expect("mapping").len(), 1);

    // Different key types are not duplicates.
    let typed = doc("1: x\n\"1\": y\n");
    assert_eq!(typed.as_mapping().expect("mapping").len(), 2);
}

// ---------------------------------------------------------------- robustness

#[test]
fn depth_limit_and_small_stack() {
    // Deep nesting is rejected by the depth budget, not by the stack.
    let deep = format!("{}{}", "[".repeat(4096), "]".repeat(4096));
    let error = parse(&deep).expect_err("depth limit");
    assert!(matches!(
        error.kind(),
        ErrorKind::MaxDepthExceeded { limit: 128 }
    ));

    // Deep but legal nesting parses and drops.
    let nested = format!("{}1{}", "[".repeat(100), "]".repeat(100));
    let value = parse(&nested).expect("100 levels fit");
    drop(value);

    // Drop, clone and compare must not recurse at all: run them on a stack far
    // too small for a recursive implementation.
    on_small_stack(|| {
        let mut deep = Value::Null;
        for _ in 0..200_000 {
            deep = Value::Sequence(vec![deep]);
        }
        drop(deep);

        let mut deeper = Value::Null;
        for _ in 0..50_000 {
            deeper = Value::Sequence(vec![deeper]);
        }
        let copy = deeper.clone();
        assert_eq!(copy, deeper);
        drop(copy);
        drop(deeper);
    });
}

#[test]
fn malformed_input_errors_instead_of_hanging() {
    let cases = [
        "a: [",
        "{",
        "[1, 2",
        "a: 'unterminated",
        "a: \"unterminated",
        "a: &",
        "a: *",
        "a: !x y",
        "a: |0\n x",
        "a:\n\t- 1",
        "<<: 1",
        "? a\n",
        "%TAG !e! tag:example.com,2000:\n",
        "%YAML 2.0\n---\na: 1\n",
    ];
    for input in cases {
        assert!(
            parse(input).is_err(),
            "{input:?} must be rejected, not accepted"
        );
    }

    // A multi-document stream stops at the first broken document.
    let mut documents = Documents::new("a: 1\n---\nb: [\n---\nc: 3\n");
    assert!(documents.next().expect("first document").is_ok());
    assert!(documents.next().expect("second document").is_err());
    assert!(
        documents.next().is_none(),
        "the iterator stops after an error"
    );
}

#[test]
fn node_budget_exceeded() {
    let error = parse_with(
        "[1, 2, 3, 4, 5, 6]",
        ParseOptions {
            max_nodes: 5,
            ..ParseOptions::new()
        },
    )
    .expect_err("node budget");
    assert!(matches!(
        error.kind(),
        ErrorKind::MaxNodesExceeded { limit: 5 }
    ));
}

#[test]
fn round_trip_and_utf8() {
    let tricky = [
        "#comment",
        ": colon",
        "- dash",
        "~",
        "yes",
        "NO",
        "1.0",
        "",
        " padded ",
        "a\tb",
        "line\nbreak",
        "ends:",
        "a: b",
        "quote'd",
        "double\"quote",
        "back\\slash",
        "emoji 😀",
        "日本語のキー",
        "עברית",
        "e\u{301}",
        "@at",
        "%percent",
        "*star",
        "&amp",
        "[bracket]",
        "{brace}",
    ];
    for text in tricky {
        let value = Value::String(text.to_string());
        let rendered = to_string(&value);
        let back = parse(&rendered).unwrap_or_else(|error| {
            panic!("{text:?} rendered as {rendered:?} must re-parse: {error}")
        });
        assert_eq!(back, value, "{text:?} rendered as {rendered:?}");
        assert_eq!(to_string(&value), rendered, "output is deterministic");
    }

    let document = doc("root:\n  name: ünïcode\n  items:\n    - 1\n    - nested: true\n");
    for flow in [false, true] {
        let options = WriteOptions {
            flow,
            ..WriteOptions::new()
        };
        let rendered = to_string_with(&document, options);
        let reparsed = parse(&rendered)
            .unwrap_or_else(|error| panic!("flow={flow} rendered {rendered:?}: {error}"));
        assert_eq!(reparsed, document, "flow={flow} rendered {rendered:?}");
    }

    let mut buffer = Vec::new();
    write_all_to(&[document.clone(), Value::Null], &mut buffer).expect("writes");
    let written = String::from_utf8(buffer).expect("utf-8");
    let documents = parse_all(&written).expect("stream parses");
    assert_eq!(documents.len(), 2);
    assert_eq!(documents[0], document);
    assert_eq!(documents[1], Value::Null);

    // Columns count characters, not bytes.
    let error = failure("b: [ü");
    assert!(matches!(error.kind(), ErrorKind::UnexpectedEof));
    assert_eq!(error.column(), 6, "multibyte characters count once");

    // Multibyte keys work end to end.
    let keys = doc("名前: 1\n");
    assert_eq!(keys.get("名前"), Some(&Value::Int(1)));
}

// ------------------------------------------------------- hardening regressions

/// Run `action` on a thread with a 2 MiB stack (the default of a spawned
/// thread), far too small for 200 000 levels of recursion.
fn on_parser_stack<F: FnOnce() + Send + 'static>(action: F) {
    std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(action)
        .expect("thread spawns")
        .join()
        .expect("action does not panic");
}

/// Assert that `input` fails with a depth error on a bounded stack.
fn assert_depth_rejected(input: String, options: ParseOptions) {
    on_parser_stack(move || {
        let error = parse_with(&input, options).expect_err("hostile nesting must be rejected");
        assert!(
            matches!(error.kind(), ErrorKind::MaxDepthExceeded { .. }),
            "unexpected kind for {:?}...: {:?}",
            &input[..input.len().min(16)],
            error.kind()
        );
    });
}

#[test]
fn depth_limit_covers_every_recursive_path() {
    let complex = ParseOptions {
        allow_complex_keys: true,
        ..ParseOptions::new()
    };
    assert_depth_rejected("- ".repeat(200_000), ParseOptions::new());
    assert_depth_rejected(format!("{}x", "- ".repeat(200_000)), ParseOptions::new());
    assert_depth_rejected("[".repeat(200_000), ParseOptions::new());
    assert_depth_rejected("{".repeat(200_000), ParseOptions::new());
    assert_depth_rejected("{a: ".repeat(200_000), ParseOptions::new());
    assert_depth_rejected(format!("{}x", "? ".repeat(200_000)), complex);
    assert_depth_rejected(format!("{}x", "a: ".repeat(200_000)), ParseOptions::new());
    assert_depth_rejected(format!("{}x", "- a: ".repeat(200_000)), ParseOptions::new());
    let mut dashes = String::new();
    for level in 0..2_000 {
        dashes.push_str(&" ".repeat(level));
        dashes.push_str("-\n");
    }
    assert_depth_rejected(dashes, ParseOptions::new());

    // Nested block mappings, each one level deeper.
    let mut nested = String::new();
    for level in 0..2_000 {
        nested.push_str(&" ".repeat(level));
        nested.push_str("k:\n");
    }
    assert_depth_rejected(nested, ParseOptions::new());

    // Anchors on their own line, each one level deeper.
    let mut anchors = String::new();
    for level in 0..2_000 {
        anchors.push_str(&" ".repeat(level));
        anchors.push_str(&format!("&a{level}\n"));
    }
    anchors.push_str(&" ".repeat(2_000));
    anchors.push('x');
    assert_depth_rejected(anchors, ParseOptions::new());

    // The limit is per level, not per helper call: 100 levels of each kind fit.
    for (input, options) in [
        (format!("{}x", "- ".repeat(100)), ParseOptions::new()),
        (format!("{}x", "a: ".repeat(100)), ParseOptions::new()),
        (
            format!("{}1{}", "[".repeat(100), "]".repeat(100)),
            ParseOptions::new(),
        ),
    ] {
        on_parser_stack(move || {
            let value = parse_with(&input, options).expect("100 levels fit the default depth");
            assert!(value.depth() > 100);
        });
    }
}

#[test]
fn sibling_null_entries_do_not_nest() {
    let value = doc("-\n-\n- x\n");
    assert_eq!(
        value,
        Value::Sequence(vec![Value::Null, Value::Null, Value::from("x")])
    );
}

#[test]
fn billion_laughs_with_raised_alias_limits_is_cheap() {
    let mut source = String::from("lol0: &lol0 lol\n");
    for level in 1..10 {
        let refs = vec![format!("*lol{}", level - 1); 10].join(", ");
        source.push_str(&format!("lol{level}: &lol{level} [{refs}]\n"));
    }
    for options in [
        ParseOptions::new(),
        ParseOptions {
            max_aliases: usize::MAX,
            ..ParseOptions::new()
        },
        ParseOptions {
            max_aliases: usize::MAX,
            max_alias_expansions: usize::MAX,
            ..ParseOptions::new()
        },
    ] {
        let started = std::time::Instant::now();
        let error = parse_with(&source, options).expect_err("alias bomb must be rejected");
        assert!(
            matches!(
                error.kind(),
                ErrorKind::AliasLimitExceeded { .. }
                    | ErrorKind::AliasExpansionLimitExceeded { .. }
                    | ErrorKind::MaxNodesExceeded { .. }
            ),
            "unexpected kind: {:?}",
            error.kind()
        );
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "the bomb is rejected before it is expanded"
        );
    }
}

#[test]
fn nested_anchor_copies_are_budgeted() {
    // Every anchor copies its whole subtree, so anchors nested `depth` deep
    // over a wide leaf would materialize `depth * width` nodes.
    let depth = 120;
    let width = 100_000;
    let mut source = String::new();
    for level in 0..depth {
        source.push_str(&format!("&n{level} ["));
    }
    source.push_str(&vec!["x"; width].join(","));
    source.push_str(&"]".repeat(depth));
    let started = std::time::Instant::now();
    let error = parse(&source).expect_err("anchor copies exceed the node budget");
    assert!(
        matches!(error.kind(), ErrorKind::MaxNodesExceeded { .. }),
        "unexpected kind: {:?}",
        error.kind()
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(10));

    // Anchors over aliases of anchors count their copies as well.
    let mut chain = String::from("a0: &a0 [x, x, x, x, x, x, x, x]\n");
    for level in 1..40 {
        chain.push_str(&format!("a{level}: &a{level} [*a{}]\n", level - 1));
    }
    let value = parse(&chain).expect("a linear chain of small anchors is fine");
    assert_eq!(
        value.get_path("a39.0.0.0").map(Value::is_sequence),
        Some(true)
    );
}

#[test]
fn drop_of_extremely_deep_values_is_iterative() {
    on_small_stack(|| {
        let mut deep = Value::Null;
        for _ in 0..1_000_000 {
            deep = Value::Sequence(vec![deep]);
        }
        drop(deep);

        // Nesting through mapping keys.
        let mut keyed = Value::Null;
        for _ in 0..1_000_000 {
            let mut mapping = Mapping::new();
            mapping.insert(keyed, Value::Null);
            keyed = Value::Mapping(mapping);
        }
        drop(keyed);

        // Nesting through mapping values.
        let mut valued = Value::Null;
        for _ in 0..1_000_000 {
            let mut mapping = Mapping::new();
            mapping.insert_str("k", valued);
            valued = Value::Mapping(mapping);
        }
        drop(valued);

        // A standalone mapping holding a deep key.
        let mut inner = Value::Null;
        for _ in 0..1_000_000 {
            inner = Value::Sequence(vec![inner]);
        }
        let mut outer = Mapping::new();
        outer.insert(inner, Value::Null);
        drop(outer);
    });
}

/// Strings that are easy to mis-serialize as plain scalars.
const TRICKY_STRINGS: &[&str] = &[
    "it's",
    "'",
    "''",
    "\"",
    "say \"hi\"",
    "a'b'c",
    "'quoted'",
    "\"dq\"",
    "---",
    "...",
    "--- x",
    "... x",
    "----",
    "--",
    "true",
    "false",
    "True",
    "TRUE",
    "null",
    "Null",
    "NULL",
    "~",
    "1",
    "-1",
    "+1",
    "1.0",
    "1e3",
    "0x1F",
    "0o17",
    ".inf",
    "-.inf",
    ".nan",
    ".NaN",
    "",
    " ",
    "  ",
    " lead",
    "trail ",
    "\tlead",
    "trail\t",
    ": ",
    "a: b",
    "a:",
    ":",
    ":a",
    "a:b",
    "a #b",
    "a#b",
    "#",
    "-",
    "- ",
    "- x",
    "-x",
    "?",
    "? x",
    "?x",
    "&a",
    "*a",
    "!a",
    "!!str",
    "|",
    "|-",
    ">",
    ">+",
    "%YAML 1.2",
    "%",
    "@",
    "`tick`",
    ",",
    "a,b",
    "[",
    "]",
    "{",
    "}",
    "[a]",
    "{a: b}",
    "<<",
    "\n",
    "a\nb",
    "a\n",
    "\na",
    "a\r\nb",
    "\r",
    "\t",
    "\0",
    "\u{7}",
    "\u{1b}[0m",
    "\u{85}",
    "\u{a0}",
    "\u{2028}",
    "\u{feff}bom",
    "back\\slash",
    "\\n",
    "emoji 😀",
    "日本語",
    "a : b",
    "a :b",
    "a\t#b",
    "x: y: z",
    "a, b",
];

#[test]
fn tricky_strings_round_trip_as_keys_and_values() {
    let styles = [QuoteStyle::Auto, QuoteStyle::Single, QuoteStyle::Double];
    for &text in TRICKY_STRINGS {
        let string = Value::from(text);
        let mut as_key = Mapping::new();
        as_key.insert(string.clone(), Value::Int(1));
        let mut as_value = Mapping::new();
        as_value.insert_str("k", string.clone());
        let mut nested = Mapping::new();
        nested.insert(string.clone(), Value::Sequence(vec![string.clone()]));
        let shapes = [
            string.clone(),
            Value::Mapping(as_key),
            Value::Mapping(as_value),
            Value::Sequence(vec![string.clone(), string.clone()]),
            Value::Sequence(vec![Value::Mapping(nested.clone())]),
            Value::Mapping(nested),
        ];
        for shape in &shapes {
            for flow in [false, true] {
                for quote_style in styles {
                    for explicit_start in [false, true] {
                        let options = WriteOptions {
                            flow,
                            quote_style,
                            explicit_start,
                            ..WriteOptions::new()
                        };
                        let rendered = to_string_with(shape, options);
                        let back = parse(&rendered).unwrap_or_else(|error| {
                            panic!("{text:?} rendered as {rendered:?} must re-parse: {error}")
                        });
                        assert_eq!(&back, shape, "{text:?} rendered as {rendered:?}");
                    }
                }
            }
        }
    }

    // Every pair of tricky strings as one mapping entry, in both positions.
    for &key in TRICKY_STRINGS {
        let mut mapping = Mapping::new();
        for &value in TRICKY_STRINGS {
            mapping.insert(Value::from(format!("{key}{value}")), Value::from(value));
        }
        let value = Value::Mapping(mapping);
        for flow in [false, true] {
            let options = WriteOptions {
                flow,
                ..WriteOptions::new()
            };
            let rendered = to_string_with(&value, options);
            let back = parse(&rendered)
                .unwrap_or_else(|error| panic!("rendered {rendered:?} must re-parse: {error}"));
            assert_eq!(back, value, "rendered {rendered:?}");
        }
    }

    // A multi-document stream of document-marker strings.
    let documents: Vec<Value> = ["---", "...", "--- a", "x"]
        .iter()
        .map(|&text| Value::from(text))
        .collect();
    let mut buffer = Vec::new();
    write_all_to(&documents, &mut buffer).expect("writes");
    let written = String::from_utf8(buffer).expect("utf-8");
    assert_eq!(parse_all(&written).expect("stream parses"), documents);
}

// ------------------------------------------------- complex-key hardening (fix 2)

/// Options that accept sequence and mapping keys.
fn complex_keys() -> ParseOptions {
    ParseOptions {
        allow_complex_keys: true,
        ..ParseOptions::new()
    }
}

#[test]
fn many_complex_flow_keys_are_not_quadratic() {
    let count = 40_000;
    let mut source = String::from("{");
    for index in 0..count {
        if index > 0 {
            source.push_str(", ");
        }
        source.push_str(&format!("[{index}]: {index}"));
    }
    source.push('}');
    let started = std::time::Instant::now();
    let value = parse_with(&source, complex_keys()).expect("distinct complex keys parse");
    let mapping = value.as_mapping().expect("a mapping");
    assert_eq!(mapping.len(), count);
    let probe = Value::Sequence(vec![Value::Int(39_999)]);
    assert_eq!(mapping.get(&probe), Some(&Value::Int(39_999)));
    let copy = value.clone();
    assert_eq!(copy, value);
    let mut changed = value.clone();
    if let Value::Mapping(inner) = &mut changed {
        *inner.get_mut(&probe).expect("present") = Value::Int(-1);
    }
    assert_ne!(changed, value);
    // Debug builds are slow; the old quadratic scan took minutes even in release.
    assert!(
        started.elapsed() < std::time::Duration::from_secs(20),
        "complex keys took {:?}",
        started.elapsed()
    );

    // Duplicates are still caught through the fingerprint index.
    let error = parse_with("{[1, {a: 2}]: x, [1, {a: 2}]: y}", complex_keys())
        .expect_err("duplicate complex key");
    assert!(matches!(error.kind(), ErrorKind::DuplicateKey { .. }));
}

#[test]
fn complex_keys_compare_structurally_and_ignore_entry_order() {
    let left = parse_with("{{a: 1, b: [2]}: x, [1, 2]: y}", complex_keys()).expect("parses");
    let right = parse_with("{[1, 2]: y, {b: [2], a: 1}: x}", complex_keys()).expect("parses");
    assert_eq!(left, right);
    let other = parse_with("{{a: 1, b: [3]}: x, [1, 2]: y}", complex_keys()).expect("parses");
    assert_ne!(left, other);

    // `insert` replaces an equal complex key instead of duplicating it.
    let mut mapping = Mapping::new();
    let key = Value::Sequence(vec![Value::Int(1), Value::from("a")]);
    assert_eq!(mapping.insert(key.clone(), Value::Int(1)), None);
    assert_eq!(
        mapping.insert(key.clone(), Value::Int(2)),
        Some(Value::Int(1))
    );
    assert_eq!(mapping.len(), 1);
    assert_eq!(mapping.remove(&key), Some(Value::Int(2)));
    assert!(mapping.is_empty());
    assert!(!mapping.contains_key(&key));
}

/// A value nested `depth` levels deep through mapping keys: `{{{0: 0}: 0}: 0}`.
fn deep_key_chain(depth: usize) -> Value {
    let mut value = Value::Int(0);
    for _ in 0..depth {
        let mut mapping = Mapping::new();
        mapping.insert(value, Value::Int(0));
        value = Value::Mapping(mapping);
    }
    value
}

#[test]
fn deep_nesting_through_keys_is_iterative() {
    let small = deep_key_chain(3);
    assert_eq!(to_string(&small), "{{{0: 0}: 0}: 0}\n");
    assert_eq!(
        format!("{small:?}"),
        "Mapping({Mapping({Mapping({Int(0): Int(0)}): Int(0)}): Int(0)})"
    );
    assert_eq!(
        parse_with(&to_string(&small), complex_keys()).expect("parses"),
        small
    );

    on_parser_stack(|| {
        let depth = 200_000;
        let left = deep_key_chain(depth);
        let right = deep_key_chain(depth);
        assert!(left == right);
        let mut different = Mapping::new();
        different.insert(deep_key_chain(depth - 1), Value::Int(1));
        let different = Value::Mapping(different);
        assert!(left != different);
        if let Value::Mapping(mapping) = &left {
            let key = deep_key_chain(depth - 1);
            assert_eq!(mapping.get(&key), Some(&Value::Int(0)));
        }

        let text = to_string(&left);
        assert_eq!(text.len(), depth * 5 + 2);
        let displayed = format!("{left}");
        assert_eq!(displayed, text);
        let sorted = to_string_with(
            &left,
            WriteOptions {
                sort_keys: true,
                ..WriteOptions::new()
            },
        );
        assert_eq!(sorted, text);

        let debug = format!("{left:?}");
        assert!(debug.starts_with("Mapping({Mapping({"));
        let mapping_debug = format!("{:?}", left.as_mapping().expect("mapping"));
        assert!(mapping_debug.starts_with("{Mapping({"));

        let copy = left.clone();
        assert!(copy == left);
        drop(copy);
        drop(left);
        drop(right);
    });
}

#[test]
fn deep_nesting_through_keys_survives_a_small_stack() {
    on_small_stack(|| {
        let value = deep_key_chain(200_000);
        let other = deep_key_chain(200_000);
        assert!(value == other);
        assert!(!to_string(&value).is_empty());
        assert!(!format!("{value:?}").is_empty());
        drop(value);
        drop(other);
    });
}

#[test]
fn signed_radix_bodies_are_strings() {
    for text in ["0x+1F", "0o+7", "0x-1", "-0x+1", "0x", "0o", "0o8"] {
        assert_eq!(
            doc(&format!("k: {text}\n")).get("k"),
            Some(&Value::from(text)),
            "{text}"
        );
    }
    assert_eq!(doc("k: -0x1F\n").get("k"), Some(&Value::Int(-31)));
    assert_eq!(doc("k: +0o17\n").get("k"), Some(&Value::Int(15)));
    // The writer quotes nothing it does not have to and still round-trips.
    let value = Value::from("0x+1F");
    assert_eq!(parse(&to_string(&value)).expect("parses"), value);
}

#[test]
fn explicit_keys_reject_duplicates() {
    let error = parse_with("? a\n: 1\n? a\n: 2\n", complex_keys()).expect_err("duplicate");
    assert!(matches!(error.kind(), ErrorKind::DuplicateKey { key } if key == "a"));
    let error = parse_with("? [1]\n: 1\n? [1]\n: 2\n", complex_keys()).expect_err("duplicate");
    assert!(matches!(error.kind(), ErrorKind::DuplicateKey { .. }));
    let relaxed = ParseOptions {
        allow_duplicate_keys: true,
        ..complex_keys()
    };
    let value = parse_with("? a\n: 1\n? a\n: 2\n", relaxed).expect("relaxed");
    assert_eq!(value.get("a"), Some(&Value::Int(2)));
}

#[test]
fn flow_mappings_honor_merge_keys() {
    let value = doc("base: &b {x: 1, y: 2}\nflow: {<<: *b, y: 3}\n");
    let flow = value.get("flow").expect("flow");
    assert_eq!(flow.get("x"), Some(&Value::Int(1)));
    assert_eq!(flow.get("y"), Some(&Value::Int(3)));
    assert_eq!(flow.get("<<"), None);
    assert_eq!(flow.as_mapping().expect("mapping").len(), 2);

    let block = doc("base: &b {x: 1, y: 2}\nblock:\n  <<: *b\n  y: 3\n");
    assert_eq!(block.get("block"), Some(flow));

    let list = doc("a: &a {x: 1}\nb: &b {x: 2, z: 3}\nm: {<<: [*a, *b]}\n");
    assert_eq!(list.get_path("m.x"), Some(&Value::Int(1)));
    assert_eq!(list.get_path("m.z"), Some(&Value::Int(3)));

    let error = parse("{<<: 1}").expect_err("merge of a scalar");
    assert!(matches!(error.kind(), ErrorKind::MergeNotMapping { .. }));
    // Quoted or tagged `<<` stays an ordinary key, and so does `<<` with
    // merge keys disabled.
    assert_eq!(doc("{'<<': 1}").get("<<"), Some(&Value::Int(1)));
    assert_eq!(doc("{!!str <<: 1}").get("<<"), Some(&Value::Int(1)));
    let plain = ParseOptions {
        merge_keys: false,
        ..ParseOptions::new()
    };
    assert_eq!(
        parse_with("{<<: 1}", plain).expect("parses").get("<<"),
        Some(&Value::Int(1))
    );
}
