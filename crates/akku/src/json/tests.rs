//! Integration tests for `akku::json`.
//!
//! The suite covers the RFC 8259 edge cases, the streaming decoder, the
//! iterative depth guards, serialization and conversions.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::io::Cursor;

use akku::json::{
    from_bytes, from_bytes_with, from_reader, json, parse, parse_with, to_string, to_string_pretty,
    to_string_with, write_to, write_to_pretty, write_to_with, Decoder, Error, ErrorKind, FromJson,
    LoneSurrogate, Number, Object, ParseOptions, Position, SerializeOptions, ToJson, Token, Value,
};

/// Parse and return the error, panicking when parsing succeeds.
fn parse_error(source: &str) -> Error {
    match parse(source) {
        Ok(value) => panic!("expected an error, parsed {value:?}"),
        Err(error) => error,
    }
}

// ---------------------------------------------------------------------------
// Scalars, whitespace and numbers
// ---------------------------------------------------------------------------

#[test]
fn top_level_scalars() {
    assert_eq!(parse("null").unwrap(), Value::Null);
    assert_eq!(parse("true").unwrap(), Value::Bool(true));
    assert_eq!(parse("false").unwrap(), Value::Bool(false));
    assert_eq!(parse("0").unwrap().as_i64(), Some(0));
    assert_eq!(parse("-1").unwrap().as_i64(), Some(-1));
    assert_eq!(parse(r#""hello""#).unwrap(), "hello");
    assert_eq!(parse("1.5").unwrap().as_f64(), Some(1.5));
}

#[test]
fn whitespace_soup_is_accepted() {
    let source = " \t\r\n { \n \"a\" \t : \r\n [ 1 , 2 , { } , [ ] ] \n } \r\n\t ";
    let value = parse(source).unwrap();
    assert_eq!(
        value.get("a").and_then(Value::as_array).map(Vec::len),
        Some(4)
    );
    assert_eq!(value.get_path("a.0").and_then(Value::as_i64), Some(1));
}

#[test]
fn integers_keep_their_exact_representation() {
    assert_eq!(parse("0").unwrap().as_number(), Some(Number::U64(0)));
    assert_eq!(parse("-0").unwrap().as_number(), Some(Number::I64(0)));
    assert_eq!(
        parse("9223372036854775807").unwrap().as_i64(),
        Some(i64::MAX)
    );
    assert_eq!(
        parse("18446744073709551615").unwrap().as_u64(),
        Some(u64::MAX)
    );
    assert_eq!(
        parse("-9223372036854775808").unwrap().as_i64(),
        Some(i64::MIN)
    );
}

#[test]
fn integers_beyond_u64_fall_back_to_f64() {
    let value = parse("18446744073709551616").unwrap();
    let number = value.as_number().unwrap();
    assert!(number.is_f64());
    assert_eq!(number.as_f64(), 1.8446744073709552e19);
    assert_eq!(number.as_u64(), None);
}

#[test]
fn tiny_and_large_floats_roundtrip() {
    for source in ["5e-324", "1e300", "-1.7976931348623157e308", "0.1"] {
        let value = parse(source).unwrap();
        assert!(value.as_number().unwrap().is_f64(), "{source}");
        let rendered = to_string(&value);
        assert_eq!(rendered, source, "{source}");
        assert_eq!(parse(&rendered).unwrap(), value, "{source}");
    }
}

#[test]
fn negative_zero_is_bit_identical_after_roundtrip() {
    let value = parse("-0.0").unwrap();
    let number = value.as_number().unwrap();
    assert!(number.is_f64());
    assert_eq!(number.as_f64().to_bits(), (-0.0f64).to_bits());
    assert_eq!(to_string(&value), "-0.0");
    let again = parse(&to_string(&value)).unwrap();
    assert_eq!(
        again.as_f64().unwrap().to_bits(),
        (-0.0f64).to_bits(),
        "the sign bit must survive"
    );
}

#[test]
fn invalid_numbers_are_rejected() {
    for source in [
        "01", "-01", "1.", "1.e5", "1e", "1e+", "+1", ".5", "-", "--1",
    ] {
        let error = parse_error(source);
        assert!(
            matches!(
                error.kind(),
                ErrorKind::InvalidNumber { .. } | ErrorKind::UnexpectedByte { .. }
            ),
            "{source}: {:?}",
            error.kind()
        );
    }
}

#[test]
fn number_out_of_range_is_reported() {
    let error = parse_error("1e400");
    assert!(matches!(error.kind(), ErrorKind::NumberOutOfRange));
    assert_eq!(
        parse_error("-1e400").kind().to_string(),
        "number out of range"
    );
}

#[test]
fn number_display_uses_json_spelling() {
    assert_eq!(Number::F64(1.0).to_string(), "1.0");
    assert_eq!(Number::F64(-0.0).to_string(), "-0.0");
    assert_eq!(Number::F64(1e300).to_string(), "1e300");
    assert_eq!(Number::I64(-7).to_string(), "-7");
    assert_eq!(Number::U64(7).to_string(), "7");
}

#[test]
fn number_from_f64_rejects_non_finite() {
    assert_eq!(Number::from_f64(f64::NAN), None);
    assert_eq!(Number::from_f64(f64::INFINITY), None);
    assert_eq!(Number::from_f64(f64::NEG_INFINITY), None);
    assert!(Number::from_f64(1.25).unwrap().is_f64());
    assert!(matches!(
        Number::try_from(f64::NAN).unwrap_err().kind(),
        ErrorKind::NumberOutOfRange
    ));
}

// ---------------------------------------------------------------------------
// Strings and unicode
// ---------------------------------------------------------------------------

#[test]
fn unicode_escapes_are_decoded() {
    assert_eq!(parse(r#""\u00e9""#).unwrap(), "é");
    assert_eq!(parse(r#""\u0041\u0042""#).unwrap(), "AB");
    assert_eq!(parse(r#""\ud83d\ude00""#).unwrap(), "😀");
    assert_eq!(parse(r#""a\\tb""#).unwrap(), "a\\tb");
    assert_eq!(parse(r#""\b\f\n\r\t""#).unwrap(), "\u{8}\u{c}\n\r\t");
    assert_eq!(parse(r#""\/""#).unwrap(), "/");
}

#[test]
fn raw_utf8_passes_through() {
    assert_eq!(parse(r#""héllo 😀""#).unwrap(), "héllo 😀");
}

#[test]
fn lone_surrogates_can_be_rejected_or_replaced() {
    let error = parse_error(r#""\ud800""#);
    assert!(matches!(error.kind(), ErrorKind::LoneSurrogate));

    let error = parse_error(r#""\udc00""#);
    assert!(matches!(error.kind(), ErrorKind::LoneSurrogate));

    let error = parse_error(r#""\ud800x""#);
    assert!(matches!(error.kind(), ErrorKind::LoneSurrogate));

    let options = ParseOptions::lenient();
    assert_eq!(options.lone_surrogate, LoneSurrogate::Replace);
    assert_eq!(parse_with(r#""\ud800""#, options).unwrap(), "\u{FFFD}");
    assert_eq!(
        parse_with(r#""\ud800\ud800""#, options).unwrap(),
        "\u{FFFD}\u{FFFD}"
    );

    let replace = ParseOptions {
        lone_surrogate: LoneSurrogate::Replace,
        ..ParseOptions::new()
    };
    assert_eq!(
        parse_with(r#""a\udfff b""#, replace).unwrap(),
        "a\u{FFFD} b"
    );
}

#[test]
fn raw_control_characters_are_rejected() {
    let error = parse_error("\"a\u{0001}b\"");
    assert!(matches!(error.kind(), ErrorKind::ControlCharacterInString));
    assert_eq!(error.position().map(|p| p.column), Some(3));

    let error = parse_error("\"line\nbreak\"");
    assert!(matches!(error.kind(), ErrorKind::ControlCharacterInString));
}

#[test]
fn invalid_escapes_are_rejected() {
    let error = parse_error(r#""\q""#);
    match error.kind() {
        ErrorKind::InvalidEscape { escape } => assert_eq!(*escape, 'q'),
        other => panic!("unexpected kind: {other:?}"),
    }

    let error = parse_error(r#""\u12""#);
    assert!(matches!(error.kind(), ErrorKind::InvalidUnicodeEscape));

    let error = parse_error(r#""\uZZZZ""#);
    assert!(matches!(error.kind(), ErrorKind::InvalidUnicodeEscape));
}

#[test]
fn string_length_limit_is_enforced() {
    let options = ParseOptions {
        max_string_len: Some(3),
        ..ParseOptions::new()
    };
    assert_eq!(parse_with(r#""abc""#, options).unwrap(), "abc");
    let error = parse_with(r#""abcd""#, options).unwrap_err();
    assert!(matches!(
        error.kind(),
        ErrorKind::StringTooLong { limit: 3 }
    ));
}

// ---------------------------------------------------------------------------
// Structure, paths and positions
// ---------------------------------------------------------------------------

#[test]
fn error_path_and_position_are_exact() {
    // The offending byte is the second digit of `01`, at path a.b[1].c.
    let error = parse_error(r#"{"a": {"b": [0, {"c": 01}]}}"#);
    assert!(matches!(
        error.kind(),
        ErrorKind::InvalidNumber {
            reason: "leading zero"
        }
    ));
    assert_eq!(error.path(), Some("a.b[1].c"));
    assert_eq!(
        error.position(),
        Some(Position {
            offset: 23,
            line: 1,
            column: 24
        })
    );
    assert_eq!(
        error.to_string(),
        "invalid number: leading zero at line 1 column 24 (at \"a.b[1].c\")"
    );
}

#[test]
fn line_and_column_track_newlines() {
    let error = parse_error("{\n  \"a\": 1,\n  \"b\": nope\n}");
    assert!(matches!(
        error.kind(),
        ErrorKind::Expected {
            expected: "null",
            ..
        }
    ));
    assert_eq!(error.path(), Some("b"));
    assert_eq!(error.position().map(|p| p.line), Some(3));
    assert_eq!(error.position().map(|p| p.column), Some(9));
}

#[test]
fn carriage_return_newlines_are_counted() {
    let error = parse_error("{\r\n  \"a\": x\r\n}");
    assert!(matches!(
        error.kind(),
        ErrorKind::UnexpectedByte { byte: b'x' }
    ));
    assert_eq!(error.position().map(|p| p.line), Some(2));
    assert_eq!(error.position().map(|p| p.column), Some(8));
}

#[test]
fn array_index_appears_in_the_path() {
    let error = parse_error("[[1], [2, tru]]");
    assert_eq!(error.path(), Some("[1][1]"));
    let error = parse_error(r#"{"list": [1, {"key": falsey}]}"#);
    assert_eq!(error.path(), Some("list[1].key"));
}

#[test]
fn structure_errors_are_reported() {
    assert!(matches!(
        parse_error("[}").kind(),
        ErrorKind::UnexpectedByte { byte: b'}' }
    ));
    assert!(matches!(
        parse_error(r#"{"a" 1}"#).kind(),
        ErrorKind::Expected {
            expected: "':'",
            ..
        }
    ));
    assert!(matches!(
        parse_error("[1 2]").kind(),
        ErrorKind::Expected { .. }
    ));
    assert!(matches!(parse_error("").kind(), ErrorKind::UnexpectedEof));
    assert!(matches!(
        parse_error("[1, ").kind(),
        ErrorKind::UnexpectedEof
    ));
    assert!(matches!(
        parse_error(r#""abc"#).kind(),
        ErrorKind::UnexpectedEof
    ));
    assert!(matches!(
        parse_error("{}x").kind(),
        ErrorKind::TrailingCharacters
    ));
    assert!(matches!(
        parse_error("1 2").kind(),
        ErrorKind::TrailingCharacters
    ));
    assert!(matches!(
        parse_error("[1,]").kind(),
        ErrorKind::TrailingComma
    ));
    assert!(matches!(
        parse_error(r#"{"a": 1,}"#).kind(),
        ErrorKind::TrailingComma
    ));
    assert!(matches!(
        parse_error("[1] [2]").kind(),
        ErrorKind::TrailingCharacters
    ));
}

#[test]
fn trailing_commas_are_configurable() {
    let options = ParseOptions {
        allow_trailing_commas: true,
        ..ParseOptions::new()
    };
    assert_eq!(
        parse_with("[1, 2,]", options)
            .unwrap()
            .as_array()
            .map(Vec::len),
        Some(2)
    );
    assert_eq!(
        parse_with(r#"{"a": 1,}"#, options)
            .unwrap()
            .get("a")
            .and_then(Value::as_i64),
        Some(1)
    );
    assert_eq!(parse_with("[]", options).unwrap(), Value::Array(vec![]));
    assert!(matches!(
        parse_with("[1,,2]", options).unwrap_err().kind(),
        ErrorKind::UnexpectedByte { byte: b',' }
    ));
}

#[test]
fn duplicate_keys_are_configurable() {
    let error = parse_error(r#"{"a": 1, "b": 2, "a": 3}"#);
    match error.kind() {
        ErrorKind::DuplicateKey { key } => assert_eq!(key, "a"),
        other => panic!("unexpected kind: {other:?}"),
    }
    assert_eq!(error.position().map(|p| p.column), Some(18));

    // Lenient parsing overwrites the value but keeps the first position.
    let options = ParseOptions::lenient();
    let value = parse_with(r#"{"a": 1, "b": 2, "a": 3}"#, options).unwrap();
    assert_eq!(value["a"], 3i64);
    assert_eq!(
        value.as_object().unwrap().keys().collect::<Vec<_>>(),
        vec!["a", "b"]
    );
}

#[test]
fn nested_duplicate_key_path_is_reported() {
    let error = parse_error(r#"{"outer": {"x": 1, "x": 2}}"#);
    assert!(matches!(error.kind(), ErrorKind::DuplicateKey { .. }));
    assert_eq!(error.path(), Some("outer"));
}

#[test]
fn depth_limit_is_enforced() {
    let ok = format!("{}{}", "[".repeat(128), "]".repeat(128));
    assert!(parse(&ok).is_ok());

    let too_deep = format!("{}{}", "[".repeat(129), "]".repeat(129));
    let error = parse(&too_deep).unwrap_err();
    assert!(matches!(
        error.kind(),
        ErrorKind::MaxDepthExceeded { limit: 128 }
    ));

    let objects = format!(
        r#"{}{}"#,
        r#"{"a":"#.repeat(129),
        format_args!("1{}", "}".repeat(129))
    );
    assert!(matches!(
        parse(&objects).unwrap_err().kind(),
        ErrorKind::MaxDepthExceeded { .. }
    ));

    let shallow = ParseOptions {
        max_depth: 2,
        ..ParseOptions::new()
    };
    assert!(parse_with("[[[1]]]", shallow).is_err());
    assert!(parse_with("[[1]]", shallow).is_ok());
}

#[test]
fn huge_nesting_returns_an_error_instead_of_overflowing() {
    let result = std::thread::Builder::new()
        .stack_size(128 * 1024)
        .spawn(|| {
            let source = "[".repeat(100_000);
            match parse(&source) {
                Err(error) => matches!(error.kind(), ErrorKind::MaxDepthExceeded { .. }),
                Ok(_) => false,
            }
        })
        .unwrap()
        .join()
        .unwrap();
    assert!(result, "a deep document must fail cleanly");
}

#[test]
fn deep_values_are_dropped_iteratively() {
    let result = std::thread::Builder::new()
        .stack_size(128 * 1024)
        .spawn(|| {
            let mut value = Value::Null;
            for _ in 0..100_000 {
                value = Value::Array(vec![value]);
            }
            drop(value);

            let mut value = Value::Null;
            for _ in 0..100_000 {
                let mut object = Object::new();
                object.insert("k", value);
                value = Value::Object(object);
            }
            drop(value);
            true
        })
        .unwrap()
        .join()
        .unwrap();
    assert!(result);
}

#[test]
fn deep_values_are_serialized_iteratively() {
    let result = std::thread::Builder::new()
        .stack_size(128 * 1024)
        .spawn(|| {
            let mut value = Value::Null;
            for _ in 0..50_000 {
                value = Value::Array(vec![value]);
            }
            let text = to_string(&value);
            assert_eq!(text.len(), 50_000 * 2 + 4);
            true
        })
        .unwrap()
        .join()
        .unwrap();
    assert!(result);
}

// ---------------------------------------------------------------------------
// Byte and reader input
// ---------------------------------------------------------------------------

#[test]
fn bytes_and_readers_are_supported() {
    assert_eq!(
        from_bytes(br#"{"a": true}"#).unwrap().get("a"),
        Some(&Value::Bool(true))
    );
    let error = from_bytes(&[b'"', 0xFF, b'"']).unwrap_err();
    assert!(matches!(error.kind(), ErrorKind::InvalidUtf8));

    let mut reader = Cursor::new(br#"[1, 2, 3]"#.to_vec());
    assert_eq!(
        from_reader(&mut reader).unwrap().as_array().map(Vec::len),
        Some(3)
    );

    let options = ParseOptions::lenient();
    assert!(from_bytes_with(b"[1,]", options).is_ok());
    let mut reader = Cursor::new(b"[1,]".to_vec());
    assert!(akku::json::from_reader_with(&mut reader, options).is_ok());
}

#[test]
fn value_implements_from_str() {
    let value: Value = "{\"a\": 1}".parse().unwrap();
    assert_eq!(value["a"], 1i64);
    assert!("{".parse::<Value>().is_err());
}

// ---------------------------------------------------------------------------
// Streaming decoder
// ---------------------------------------------------------------------------

#[test]
fn decoder_yields_the_whole_token_sequence() {
    let mut decoder = Decoder::new(r#"{"a": [1, true]}"#);
    let tokens: Vec<Token<'_>> = std::iter::from_fn(|| decoder.next().transpose())
        .collect::<akku::json::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(
        tokens,
        vec![
            Token::ObjectStart,
            Token::ObjectKey("a".into()),
            Token::ArrayStart,
            Token::Number(Number::U64(1)),
            Token::Bool(true),
            Token::ArrayEnd,
            Token::ObjectEnd,
        ]
    );
}

#[test]
fn decoder_borrows_escape_free_strings() {
    let mut decoder = Decoder::new(r#""plain" "#);
    match decoder.next().unwrap() {
        Some(Token::String(Cow::Borrowed(text))) => assert_eq!(text, "plain"),
        other => panic!("expected a borrowed string, got {other:?}"),
    }

    let mut decoder = Decoder::new(r#""a\nb""#);
    match decoder.next().unwrap() {
        Some(Token::String(Cow::Owned(text))) => assert_eq!(text, "a\nb"),
        other => panic!("expected an owned string, got {other:?}"),
    }
}

#[test]
fn decoder_reports_depth_and_position() {
    let mut decoder = Decoder::new("[[1]]");
    assert_eq!(decoder.depth(), 0);
    assert_eq!(decoder.position().column, 1);
    assert_eq!(decoder.next().unwrap(), Some(Token::ArrayStart));
    assert_eq!(decoder.depth(), 1);
    assert_eq!(decoder.next().unwrap(), Some(Token::ArrayStart));
    assert_eq!(decoder.depth(), 2);
    assert_eq!(
        decoder.next().unwrap().and_then(|token| match token {
            Token::Number(number) => Some(number),
            _ => None,
        }),
        Some(Number::U64(1))
    );
    assert_eq!(decoder.position().column, 4);
    assert_eq!(decoder.next().unwrap(), Some(Token::ArrayEnd));
    assert_eq!(decoder.depth(), 1);
    assert_eq!(decoder.next().unwrap(), Some(Token::ArrayEnd));
    assert_eq!(decoder.next().unwrap(), None);
}

#[test]
fn decoder_skip_value_handles_nesting() {
    let mut decoder = Decoder::new(r#"[{"a": [1, {"b": 2}]}, "after"]"#);
    assert_eq!(decoder.next().unwrap(), Some(Token::ArrayStart));
    decoder.skip_value().unwrap();
    assert_eq!(
        decoder.next().unwrap(),
        Some(Token::String(Cow::Borrowed("after")))
    );
    assert_eq!(decoder.next().unwrap(), Some(Token::ArrayEnd));
    assert_eq!(decoder.next().unwrap(), None);

    // Skipping a scalar is fine too.
    let mut decoder = Decoder::new("true false");
    decoder.skip_value().unwrap();
    assert!(decoder.next().is_err());
}

#[test]
fn decoder_skip_value_requires_a_value_position() {
    let mut decoder = Decoder::new("{}");
    assert_eq!(decoder.next().unwrap(), Some(Token::ObjectStart));
    assert!(decoder.skip_value().is_err());

    // Calling skip_value inside an empty array must fail rather than consuming `]`.
    let mut decoder = Decoder::new("[]");
    assert_eq!(decoder.next().unwrap(), Some(Token::ArrayStart));
    let err = decoder.skip_value().unwrap_err();
    assert!(matches!(
        err.kind(),
        ErrorKind::Expected {
            expected: "a value",
            found: "']'",
        }
    ));

    // Skipping an entire empty array or empty object from the outside still succeeds.
    let mut decoder = Decoder::new("[]");
    decoder.skip_value().unwrap();
    assert_eq!(decoder.next().unwrap(), None);

    let mut decoder = Decoder::new("{}");
    decoder.skip_value().unwrap();
    assert_eq!(decoder.next().unwrap(), None);
}

#[test]
fn decoder_skip_value_works_right_after_object_key() {
    let mut decoder = Decoder::new(
        r#"{"scalar": 42, "nested": {"x": [1, true]}, "empty_arr": [], "empty_obj": {}, "keep": "ok"}"#,
    );
    assert_eq!(decoder.next().unwrap(), Some(Token::ObjectStart));

    assert_eq!(
        decoder.next().unwrap(),
        Some(Token::ObjectKey(Cow::Borrowed("scalar")))
    );
    decoder.skip_value().unwrap();

    assert_eq!(
        decoder.next().unwrap(),
        Some(Token::ObjectKey(Cow::Borrowed("nested")))
    );
    decoder.skip_value().unwrap();

    assert_eq!(
        decoder.next().unwrap(),
        Some(Token::ObjectKey(Cow::Borrowed("empty_arr")))
    );
    decoder.skip_value().unwrap();

    assert_eq!(
        decoder.next().unwrap(),
        Some(Token::ObjectKey(Cow::Borrowed("empty_obj")))
    );
    decoder.skip_value().unwrap();

    assert_eq!(
        decoder.next().unwrap(),
        Some(Token::ObjectKey(Cow::Borrowed("keep")))
    );
    assert_eq!(
        decoder.next().unwrap(),
        Some(Token::String(Cow::Borrowed("ok")))
    );
    assert_eq!(decoder.next().unwrap(), Some(Token::ObjectEnd));
    assert_eq!(decoder.next().unwrap(), None);
}

#[test]
fn decoder_reports_structure_errors() {
    let mut decoder = Decoder::new("[}");
    assert_eq!(decoder.next().unwrap(), Some(Token::ArrayStart));
    assert!(matches!(
        decoder.next().unwrap_err().kind(),
        ErrorKind::UnexpectedByte { byte: b'}' }
    ));

    let mut decoder = Decoder::new(r#"{"a"}"#);
    assert_eq!(decoder.next().unwrap(), Some(Token::ObjectStart));
    assert_eq!(
        decoder.next().unwrap(),
        Some(Token::ObjectKey(Cow::Borrowed("a")))
    );
    assert!(matches!(
        decoder.next().unwrap_err().kind(),
        ErrorKind::Expected {
            expected: "':'",
            ..
        }
    ));

    let mut decoder = Decoder::new("[1 2]");
    assert!(decoder.next().is_ok());
    assert!(decoder.next().is_ok());
    assert!(decoder.next().is_err());
}

// ---------------------------------------------------------------------------
// Serializer
// ---------------------------------------------------------------------------

#[test]
fn serializer_escapes_control_characters() {
    let value = Value::String("\u{0001}\u{001f}\n\r\t\u{8}\u{c}\"\\".to_owned());
    assert_eq!(to_string(&value), r#""\u0001\u001f\n\r\t\b\f\"\\""#);
}

#[test]
fn serializer_escapes_html_on_request() {
    let value = json!({ "html": "<script>alert('x & y')</script>" });
    assert_eq!(
        to_string(&value),
        r#"{"html":"<script>alert('x & y')</script>"}"#
    );

    let options = SerializeOptions::new().with_escape_html(true);
    assert_eq!(
        to_string_with(&value, options),
        r#"{"html":"\u003cscript\u003ealert(\u0027x \u0026 y\u0027)\u003c/script\u003e"}"#
    );
}

#[test]
fn serializer_escapes_non_ascii_on_request() {
    let value = Value::String("héllo 😀".to_owned());
    assert_eq!(to_string(&value), "\"héllo 😀\"");

    let options = SerializeOptions::new().with_escape_non_ascii(true);
    assert_eq!(
        to_string_with(&value, options),
        r#""h\u00e9llo \ud83d\ude00""#
    );
}

#[test]
fn serializer_pretty_prints() {
    let value = parse(r#"{"a":[1,2],"b":{"c":null},"d":[],"e":{}}"#).unwrap();
    assert_eq!(
        to_string_pretty(&value),
        "{\n  \"a\": [\n    1,\n    2\n  ],\n  \"b\": {\n    \"c\": null\n  },\n  \"d\": [],\n  \"e\": {}\n}"
    );

    let options = SerializeOptions::pretty_printed().with_indent(4);
    assert_eq!(
        to_string_with(&json!({"a": [1]}), options),
        "{\n    \"a\": [\n        1\n    ]\n}"
    );

    let options = SerializeOptions::pretty_printed().with_indent(0);
    assert_eq!(to_string_with(&json!([1, 2]), options), "[\n1,\n2\n]");
}

#[test]
fn write_to_matches_to_string_byte_for_byte() {
    let value = parse(r#"{"a":[1,true,null,"x"],"b":{"c":1.5}}"#).unwrap();

    let mut compact = Vec::new();
    write_to(&value, &mut compact).unwrap();
    assert_eq!(String::from_utf8(compact).unwrap(), to_string(&value));

    let mut pretty = Vec::new();
    write_to_pretty(&value, &mut pretty).unwrap();
    assert_eq!(String::from_utf8(pretty).unwrap(), to_string_pretty(&value));

    let options = SerializeOptions::pretty_printed()
        .with_escape_html(true)
        .with_escape_non_ascii(true);
    let mut custom = Vec::new();
    write_to_with(&value, &mut custom, options).unwrap();
    assert_eq!(
        String::from_utf8(custom).unwrap(),
        to_string_with(&value, options)
    );
}

#[test]
fn null_character_roundtrips() {
    let value = parse(r#""\u0000""#).unwrap();
    assert_eq!(value.as_str(), Some("\u{0}"));
    assert_eq!(to_string(&value), "\"\\u0000\"");
    assert_eq!(parse(&to_string(&value)).unwrap(), value);
}

/// A writer that always fails, to exercise the `Io` error path.
struct FailingWriter;

impl std::io::Write for FailingWriter {
    fn write(&mut self, _buffer: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other("no space left"))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn write_errors_surface_as_io_errors() {
    let error = write_to(&json!([1, 2]), FailingWriter).unwrap_err();
    assert!(matches!(error.kind(), ErrorKind::Io(_)));
    let source = &error as &dyn std::error::Error;
    assert!(source.source().is_some());
    assert_eq!(
        std::io::Error::from(parse_error("[1,]")).kind(),
        std::io::ErrorKind::InvalidData
    );
}

#[test]
fn display_matches_compact_serialization() {
    let value = json!({"a": [1, "two"]});
    assert_eq!(value.to_string(), to_string(&value));
    assert_eq!(value.to_string(), r#"{"a":[1,"two"]}"#);
}

#[test]
fn serializer_keeps_object_order() {
    let value = json!({ "z": 1, "a": 2, "m": 3 });
    assert_eq!(to_string(&value), r#"{"z":1,"a":2,"m":3}"#);
}

// ---------------------------------------------------------------------------
// Value accessors, paths and pointers
// ---------------------------------------------------------------------------

#[test]
fn value_accessors_are_typed() {
    let value =
        parse(r#"{"n": null, "b": true, "i": -3, "u": 3, "f": 1.5, "s": "x", "a": [1], "o": {}}"#)
            .unwrap();
    assert!(value.get("n").unwrap().is_null());
    assert_eq!(value.get_path("b").unwrap().as_bool(), Some(true));
    assert_eq!(value.get_path("i").unwrap().as_i64(), Some(-3));
    assert_eq!(value.get_path("u").unwrap().as_u64(), Some(3));
    assert_eq!(value.get_path("f").unwrap().as_f64(), Some(1.5));
    assert_eq!(value.get_path("s").unwrap().as_str(), Some("x"));
    assert_eq!(
        value.get_path("a").unwrap().as_array().map(Vec::len),
        Some(1)
    );
    assert!(value.get_path("o").unwrap().as_object().unwrap().is_empty());
    assert_eq!(value.get_index(0), None);
    assert_eq!(value.get("missing"), None);
    assert_eq!(value.type_name(), "object");
    assert_eq!(value["s"], "x");
    assert_eq!(value["a"][0], 1i64);
}

#[test]
fn value_mutation_helpers() {
    let mut value = Value::Null;
    value.insert("a", Value::from(1));
    *value.get_mut("a").unwrap() = Value::Array(vec![Value::from(9)]);
    value
        .get_mut("a")
        .unwrap()
        .as_array_mut()
        .unwrap()
        .push(Value::from(10));
    assert_eq!(value["a"][0], 9i64);
    assert_eq!(value["a"][1], 10i64);
    assert_eq!(value.remove("a").map(|v| v.is_null()), Some(false));
    assert!(value.is_object());

    let mut arrays = Value::Array(vec![Value::Null]);
    arrays.push(Value::from(true));
    assert_eq!(arrays.as_array().map(Vec::len), Some(2));
    assert_eq!(arrays.into_array().map(|items| items.len()), Some(2));

    let object = json!({ "a": 1 });
    assert_eq!(object.into_object().map(|o| o.len()), Some(1));
    assert_eq!(json!("text").into_string(), Some("text".to_owned()));
    assert_eq!(json!(1).into_string(), None);
    assert_eq!(json!(2).into_number(), Some(Number::U64(2)));
}

#[test]
fn dotted_paths_support_escaped_dots() {
    let value = parse(r#"{"a": {"b": [10, 20]}, "a.b": 7, "x\\y": 8}"#).unwrap();
    assert_eq!(value.get_path("a.b.1").and_then(Value::as_i64), Some(20));
    assert_eq!(value.get_path(r"a\.b").and_then(Value::as_i64), Some(7));
    assert_eq!(value.get_path(r"x\\y").and_then(Value::as_i64), Some(8));
    assert_eq!(value.get_path("a.b.9"), None);
    assert_eq!(value.get_path("a.missing"), None);
    assert_eq!(value.get_path("").map(Value::type_name), Some("object"));

    let mut mutable = parse(r#"{"a": {"b": [10]}}"#).unwrap();
    *mutable.get_path_mut("a.b.0").unwrap() = Value::from(11);
    assert_eq!(mutable.get_path("a.b.0").and_then(Value::as_i64), Some(11));
    assert!(mutable.get_path_mut("a.nope").is_none());
}

#[test]
fn json_pointers_follow_rfc_6901() {
    let value = parse(
        r#"{"foo": ["bar", "baz"], "": 0, "a/b": 1, "c%d": 2, "e^f": 3, "g|h": 4, "i\\j": 5, "k\"l": 6, " ": 7, "m~n": 8}"#,
    )
    .unwrap();
    assert_eq!(value.pointer("").map(Value::type_name), Some("object"));
    assert_eq!(value.pointer("/foo").map(Value::type_name), Some("array"));
    assert_eq!(
        value.pointer("/foo/0"),
        Some(&Value::String("bar".to_owned()))
    );
    assert_eq!(value.pointer("/").and_then(Value::as_i64), Some(0));
    assert_eq!(value.pointer("/a~1b").and_then(Value::as_i64), Some(1));
    assert_eq!(value.pointer("/c%d").and_then(Value::as_i64), Some(2));
    assert_eq!(value.pointer("/i\\j").and_then(Value::as_i64), Some(5));
    assert_eq!(value.pointer("/k\"l").and_then(Value::as_i64), Some(6));
    assert_eq!(value.pointer("/ ").and_then(Value::as_i64), Some(7));
    assert_eq!(value.pointer("/m~0n").and_then(Value::as_i64), Some(8));
    assert_eq!(value.pointer("/nope"), None);
    assert_eq!(value.pointer("/foo/9"), None);
    assert_eq!(value.pointer("foo"), None, "a pointer must start with '/'");
    assert_eq!(value.pointer("/m~2n"), None, "invalid escapes are rejected");

    let mut mutable = parse(r#"{"a": {"b": [1]}}"#).unwrap();
    *mutable.pointer_mut("/a/b/0").unwrap() = Value::from(2);
    assert_eq!(mutable.pointer("/a/b/0").and_then(Value::as_i64), Some(2));
    assert!(mutable.pointer_mut("/a/x").is_none());
}

#[test]
fn value_partial_eq_shortcuts() {
    let value = json!({ "b": true, "n": 5 });
    assert!(value["b"] == true);
    assert!(value["n"] == 5i64);
    assert!(value["n"] == 5u64);
    assert!(json!("x") == "x");
    assert!(json!("x") == *"x");
    assert_eq!(value["n"], 5i64);
}

// ---------------------------------------------------------------------------
// Object behaviour
// ---------------------------------------------------------------------------

#[test]
fn object_preserves_insertion_order() {
    let mut object = Object::new();
    object.insert("z", Value::from(1));
    object.insert("a", Value::from(2));
    object.insert("m", Value::from(3));
    assert_eq!(object.keys().collect::<Vec<_>>(), vec!["z", "a", "m"]);
    assert_eq!(
        object.values().map(|v| v.as_i64()).collect::<Vec<_>>(),
        vec![Some(1), Some(2), Some(3)]
    );
    assert_eq!(
        object.iter().map(|(k, _)| k).collect::<Vec<_>>(),
        vec!["z", "a", "m"]
    );
}

#[test]
fn object_equality_ignores_order() {
    let mut left = Object::new();
    left.insert("a", Value::from(1));
    left.insert("b", Value::from(2));

    let mut right = Object::new();
    right.insert("b", Value::from(2));
    right.insert("a", Value::from(1));

    assert_eq!(left, right);
    assert_eq!(Value::Object(left.clone()), Value::Object(right.clone()));

    right.insert("c", Value::from(3));
    assert_ne!(left, right);
}

#[test]
fn object_sort_keys_and_helpers() {
    let mut object: Object = vec![("b", Value::from(2)), ("a", Value::from(1))]
        .into_iter()
        .collect();
    object.sort_keys();
    assert_eq!(object.keys().collect::<Vec<_>>(), vec!["a", "b"]);

    assert_eq!(object.len(), 2);
    assert!(!object.is_empty());
    assert!(object.contains_key("a"));
    assert_eq!(object.get("a"), Some(&Value::from(1)));
    assert_eq!(object["a"], 1i64);
    object["a"] = Value::from(10);
    assert_eq!(object["a"], 10i64);
    assert_eq!(object.iter_mut().count(), 2);
    assert_eq!(object.clone().into_iter().count(), 2);
    assert_eq!((&object).into_iter().count(), 2);
    assert_eq!((&mut object).into_iter().count(), 2);
    object.clear();
    assert!(object.is_empty());
    assert_eq!(Object::with_capacity(4).len(), 0);
}

#[test]
fn object_entry_api() {
    let mut object = Object::new();
    *object.entry("a").or_insert(Value::Null) = Value::from(1);
    object.entry("a").or_insert_with(|| Value::from(99));
    *object
        .entry("b")
        .or_insert_with_key(|key| Value::from(key.len())) = Value::from(2);
    assert_eq!(object["a"], 1i64);
    assert_eq!(object["b"], 2i64);
    assert_eq!(object.entry("a").key(), "a");

    match object.entry("a") {
        akku::json::Entry::Occupied(mut entry) => {
            assert_eq!(entry.key(), "a");
            assert_eq!(entry.get(), &Value::from(1));
            assert_eq!(entry.insert(Value::from(5)), Value::from(1));
            assert_eq!(entry.get_mut(), &Value::from(5));
            assert_eq!(entry.remove(), Value::from(5));
        }
        akku::json::Entry::Vacant(_) => panic!("expected an occupied entry"),
    }
    assert!(!object.contains_key("a"));

    match object.entry("z") {
        akku::json::Entry::Vacant(entry) => {
            assert_eq!(entry.key(), "z");
            assert_eq!(entry.into_key(), "z");
        }
        akku::json::Entry::Occupied(_) => panic!("expected a vacant entry"),
    }

    match object.entry("new") {
        akku::json::Entry::Vacant(entry) => {
            entry.insert(Value::from(7));
        }
        akku::json::Entry::Occupied(_) => panic!("expected a vacant entry"),
    }
    assert_eq!(object["new"], 7i64);
}

// ---------------------------------------------------------------------------
// Conversions
// ---------------------------------------------------------------------------

#[test]
fn to_json_covers_primitives() {
    assert_eq!(true.to_json(), Value::Bool(true));
    assert_eq!(1i8.to_json().as_i64(), Some(1));
    assert_eq!(1i64.to_json().as_i64(), Some(1));
    assert_eq!(1u64.to_json().as_u64(), Some(1));
    assert_eq!(1usize.to_json().as_u64(), Some(1));
    assert_eq!(1.5f64.to_json().as_f64(), Some(1.5));
    assert_eq!(1.5f32.to_json().as_f64(), Some(1.5));
    assert_eq!("text".to_json(), Value::String("text".to_owned()));
    assert_eq!(String::from("text").to_json(), "text");
    assert_eq!(Cow::Borrowed("text").to_json(), "text");
    assert_eq!(().to_json(), Value::Null);
    assert_eq!(None::<i32>.to_json(), Value::Null);
    assert_eq!(Some(3).to_json().as_i64(), Some(3));
    assert_eq!(vec![1, 2].to_json().as_array().map(Vec::len), Some(2));
    assert_eq!([1, 2].to_json().as_array().map(Vec::len), Some(2));
    assert_eq!((1, "two").to_json().as_array().map(Vec::len), Some(2));
    assert_eq!((1, 2, 3, 4).to_json().as_array().map(Vec::len), Some(4));
    assert_eq!(
        (1, 2, 3, 4, 5, 6).to_json().as_array().map(Vec::len),
        Some(6)
    );
    assert_eq!(json!(1).to_json(), json!(1));
}

#[test]
fn non_finite_floats_become_null() {
    assert_eq!(f64::NAN.to_json(), Value::Null);
    assert_eq!(f64::INFINITY.to_json(), Value::Null);
    assert_eq!(f32::NEG_INFINITY.to_json(), Value::Null);
}

#[test]
fn to_json_maps_and_objects() {
    let mut sorted = BTreeMap::new();
    sorted.insert("a".to_owned(), 1);
    let value = sorted.to_json();
    assert_eq!(value.get("a").and_then(Value::as_i64), Some(1));

    let mut hashed = HashMap::new();
    hashed.insert("b".to_owned(), true);
    assert_eq!(
        hashed.to_json().get("b").and_then(Value::as_bool),
        Some(true)
    );

    let mut object = Object::new();
    object.insert("c", Value::from(3));
    assert_eq!(object.to_json(), json!({"c": 3}));
}

#[test]
fn from_json_roundtrips_through_text() {
    let value = json!({
        "bool": true,
        "int": 42,
        "big": u64::MAX,
        "float": 1.5,
        "text": "héllo",
        "list": [1, 2, 3],
        "nested": { "ok": false },
        "null": null,
    });

    assert!(bool::from_json(value.get("bool").unwrap()).unwrap());
    assert_eq!(i64::from_json(value.get("int").unwrap()).unwrap(), 42);
    assert_eq!(u64::from_json(value.get("big").unwrap()).unwrap(), u64::MAX);
    assert_eq!(f64::from_json(value.get("float").unwrap()).unwrap(), 1.5);
    assert_eq!(
        String::from_json(value.get("text").unwrap()).unwrap(),
        "héllo"
    );
    assert_eq!(
        Vec::<u8>::from_json(value.get("list").unwrap()).unwrap(),
        vec![1, 2, 3]
    );
    assert_eq!(<()>::from_json(value.get("null").unwrap()).unwrap(), ());
    assert_eq!(
        Option::<i32>::from_json(value.get("null").unwrap()).unwrap(),
        None
    );
    assert_eq!(
        Option::<i32>::from_json(value.get("int").unwrap()).unwrap(),
        Some(42)
    );
    assert_eq!(Value::from_json(&value).unwrap(), value);

    let nested: BTreeMap<String, bool> = BTreeMap::from_json(value.get("nested").unwrap()).unwrap();
    assert_eq!(nested.get("ok"), Some(&false));

    let hashed: HashMap<String, bool> = HashMap::from_json(value.get("nested").unwrap()).unwrap();
    assert_eq!(hashed.get("ok"), Some(&false));

    let object = Object::from_json(value.as_object().map(|_| &value).unwrap()).unwrap();
    assert_eq!(object.len(), 8);
    assert_eq!(value.as_number(), None);
    assert_eq!(
        Number::from_json(value.get("int").unwrap()).unwrap(),
        Number::U64(42)
    );
}

#[test]
fn from_json_tuple_and_number() {
    let value = json!([1, "two", true, null]);
    assert_eq!(
        <(i64, String, bool, ())>::from_json(&value).unwrap(),
        (1, "two".to_owned(), true, ())
    );
    assert!(<(i64, i64)>::from_json(&value).is_err());
    assert!(
        <(i64, i64)>::from_json(&json!([1, 2])).is_ok(),
        "tuples of the matching length convert"
    );
    assert!(<Number>::from_json(&json!(1)).is_ok());
    assert!(<Number>::from_json(&json!("x")).is_err());
}

#[test]
fn from_json_reports_type_mismatch() {
    let error = String::from_json(&json!(1)).unwrap_err();
    assert!(matches!(
        error.kind(),
        ErrorKind::TypeMismatch {
            expected: "a string",
            found: "number"
        }
    ));

    let error = bool::from_json(&json!("true")).unwrap_err();
    assert!(matches!(
        error.kind(),
        ErrorKind::TypeMismatch {
            expected: "a boolean",
            ..
        }
    ));

    let error = i64::from_json(&json!(1.5)).unwrap_err();
    assert!(matches!(
        error.kind(),
        ErrorKind::TypeMismatch {
            expected: "an integer",
            found: "number"
        }
    ));

    assert!(matches!(
        Vec::<u8>::from_json(&json!({})).unwrap_err().kind(),
        ErrorKind::TypeMismatch {
            expected: "an array",
            ..
        }
    ));
    assert!(matches!(
        <()>::from_json(&json!(0)).unwrap_err().kind(),
        ErrorKind::TypeMismatch {
            expected: "null",
            ..
        }
    ));
    assert!(matches!(
        <(i64, i64)>::from_json(&json!(1)).unwrap_err().kind(),
        ErrorKind::TypeMismatch { .. }
    ));
}

#[test]
fn from_json_integer_ranges_are_enforced() {
    assert!(matches!(
        u8::from_json(&json!(-1)).unwrap_err().kind(),
        ErrorKind::NumberOutOfRange
    ));
    assert!(matches!(
        u8::from_json(&json!(-1)).unwrap_err().kind(),
        ErrorKind::NumberOutOfRange
    ));
    assert!(matches!(
        u8::from_json(&json!(300)).unwrap_err().kind(),
        ErrorKind::NumberOutOfRange
    ));
    assert!(matches!(
        i8::from_json(&json!(200)).unwrap_err().kind(),
        ErrorKind::NumberOutOfRange
    ));
    assert!(matches!(
        i64::from_json(&json!(u64::MAX)).unwrap_err().kind(),
        ErrorKind::NumberOutOfRange
    ));
    assert!(matches!(
        f32::from_json(&json!(1e300)).unwrap_err().kind(),
        ErrorKind::NumberOutOfRange
    ));
    assert_eq!(u8::from_json(&json!(255)).unwrap(), 255);
    assert_eq!(i8::from_json(&json!(-128)).unwrap(), -128);
    assert_eq!(f32::from_json(&json!(1.5)).unwrap(), 1.5f32);
}

// ---------------------------------------------------------------------------
// The `json!` macro
// ---------------------------------------------------------------------------

#[test]
fn json_macro_builds_nested_values() {
    let name = "akku";
    let count = 3;
    let value = json!({
        "name": name,
        "count": count,
        "ratio": 0.5,
        "tags": ["rust", "json",],
        "meta": { "ok": true, "nested": [null, false] },
        "nothing": null,
        "sum": (1 + 2),
    });

    assert_eq!(value.get("name").and_then(Value::as_str), Some("akku"));
    assert_eq!(value.get("count").and_then(Value::as_i64), Some(3));
    assert_eq!(value.get("ratio").and_then(Value::as_f64), Some(0.5));
    assert_eq!(
        value.get_path("tags.1").and_then(Value::as_str),
        Some("json")
    );
    assert_eq!(
        value.get_path("meta.nested.1").and_then(Value::as_bool),
        Some(false)
    );
    assert_eq!(value.get("nothing"), Some(&Value::Null));
    assert_eq!(value.get("sum").and_then(Value::as_i64), Some(3));
    assert_eq!(value.as_object().map(Object::len), Some(7));
}

#[test]
fn json_macro_supports_scalars_and_empty_containers() {
    assert_eq!(json!(null), Value::Null);
    assert_eq!(json!(true), Value::Bool(true));
    assert_eq!(json!(false), Value::Bool(false));
    assert_eq!(json!(7).as_i64(), Some(7));
    assert_eq!(json!(-7).as_i64(), Some(-7));
    assert_eq!(json!(1.25).as_f64(), Some(1.25));
    assert_eq!(json!("text"), "text");
    assert_eq!(json!([]), Value::Array(Vec::new()));
    assert_eq!(json!({}), Value::Object(Object::new()));
    assert_eq!(json!([1]), Value::from(vec![Value::from(1)]));
}

#[test]
fn json_macro_output_is_valid_json() {
    let value = json!({
        "a": [1, 2, [3, [4]]],
        "b": { "c": null },
        "d": "text with \"quotes\" and \n newline",
        "e": -0.0,
    });
    let text = to_string(&value);
    let reparsed = parse(&text).unwrap();
    assert_eq!(reparsed, value);
    assert_eq!(
        reparsed.get_path("a.2.1.0").and_then(Value::as_i64),
        Some(4)
    );
}

// ---------------------------------------------------------------------------
// Errors as a trait object
// ---------------------------------------------------------------------------

#[test]
fn error_implements_std_error() {
    let error = parse_error("[1,]");
    let dynamic: &dyn std::error::Error = &error;
    assert!(dynamic.source().is_none());
    assert!(dynamic.to_string().contains("trailing comma"));

    let io_error = akku::json::Error::from(std::io::Error::other("boom"));
    assert!(matches!(io_error.kind(), ErrorKind::Io(_)));
    assert!(io_error.source_is_io());
}

/// Small helper trait so the test above reads clearly.
trait SourceIsIo {
    fn source_is_io(&self) -> bool;
}

impl SourceIsIo for Error {
    fn source_is_io(&self) -> bool {
        matches!(self.kind(), ErrorKind::Io(_))
    }
}
