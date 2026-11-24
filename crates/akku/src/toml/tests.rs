//! Integration tests for `akku::toml`.

use akku::toml::{
    to_string, to_string_with, Datetime, ErrorKind, Offset, SerializeOptions, Table, Value,
};

#[track_caller]
fn parse_ok(input: &str) -> Table {
    match akku::toml::parse(input) {
        Ok(table) => table,
        Err(error) => panic!("expected {input:?} to parse, got: {error}"),
    }
}

#[track_caller]
fn parse_err(input: &str) -> akku::toml::Error {
    match akku::toml::parse(input) {
        Ok(_) => panic!("expected {input:?} to fail"),
        Err(error) => error,
    }
}

fn is_kind(kind: &ErrorKind, name: &str) -> bool {
    format!("{kind:?}").starts_with(name)
}

#[track_caller]
fn assert_err_kind(input: &str, name: &str) {
    let error = parse_err(input);
    assert!(
        is_kind(error.kind(), name),
        "expected {name} for {input:?}, got {:?}",
        error.kind()
    );
    assert!(error.to_string().contains("line"));
    let _: &dyn std::error::Error = &error;
}

#[test]
fn parses_a_full_document() {
    let source = r#"
title = "TOML Example"

[owner]
name = "Tom"
dob = 1979-05-27T07:32:00-08:00

[database]
enabled = true
ports = [ 8000, 8001, 8002 ]
data = [ ["delta", "phi"], [3.14] ]
temp_targets = { cpu = 79.5, case = 72.0 }
"#;
    let table = parse_ok(source);
    assert_eq!(table.get_str("title"), Some("TOML Example"));
    assert_eq!(table.get_str_path("owner.name"), Some("Tom"));
    assert_eq!(table.get_bool_path("database.enabled"), Some(true));
    assert_eq!(
        table
            .get_path("database.ports")
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(
        table
            .get_path("database.temp_targets.cpu")
            .unwrap()
            .as_float(),
        Some(79.5)
    );
    assert_eq!(
        table
            .get_path("owner.dob")
            .unwrap()
            .as_datetime()
            .unwrap()
            .offset,
        Some(Offset::Custom {
            hours: -8,
            minutes: 0
        })
    );
}

#[test]
fn duplicate_table_and_key_conflicts() {
    assert_err_kind("[a]\n[a]\n", "DuplicateTable");
    assert_err_kind("[a]\nb = 1\n[a]\n", "DuplicateTable");
    assert_err_kind("x = 1\nx = 2\n", "DuplicateKey");
    assert_err_kind("[a]\nb = 1\nb = 2\n", "DuplicateKey");
    assert_err_kind("a.b = 1\na.b = 2\n", "DuplicateKey");
    // A table created by dotted keys cannot be reopened by a header.
    assert_err_kind("a.b = 1\n[a]\n", "TableRedefined");
    // A dotted key inside a header cannot be reopened either.
    assert_err_kind("[a]\nb.c = 1\n[a.b]\n", "TableRedefined");
    // A scalar cannot become a table.
    assert_err_kind("[a]\nb = 1\n[a.b]\n", "DuplicateTable");
    assert_err_kind("a = 1\n[a]\n", "DuplicateTable");
    // Implicit tables may be defined later, exactly once.
    let table = parse_ok("[a.b]\nx = 1\n[a]\ny = 2\n");
    assert_eq!(table.get_int_path("a.y"), Some(2));
    assert_eq!(table.get_int_path("a.b.x"), Some(1));
    // Extending a dotted-key table with a deeper header is allowed.
    let table =
        parse_ok("[fruit]\napple.color = \"red\"\n\n[fruit.apple.texture]\nsmooth = true\n");
    assert_eq!(table.get_str_path("fruit.apple.color"), Some("red"));
    assert_eq!(
        table.get_bool_path("fruit.apple.texture.smooth"),
        Some(true)
    );
}

#[test]
fn inline_tables_are_immutable() {
    let table = parse_ok("t = { a = 1, b = { c = 2 } }\n");
    assert_eq!(table.get_int_path("t.a"), Some(1));
    assert_eq!(table.get_int_path("t.b.c"), Some(2));

    assert_err_kind("t = { a = 1 }\n[t]\nx = 1\n", "TableRedefined");
    assert_err_kind("t = { a = 1 }\nt.b = 2\n", "DottedKeyConflict");
    assert_err_kind("t = { a = 1 }\n[t.b]\nx = 1\n", "TableRedefined");
    assert_err_kind("t = { a = 1, }\n", "InlineTableNotClosed");
    assert_err_kind("t = {\na = 1\n}\n", "NestedInlineTable");
    assert_err_kind("t = { a = 1\n}\n", "NestedInlineTable");
    assert_err_kind("t = { a = 1", "InlineTableNotClosed");
    // Dotted keys inside an inline table are allowed.
    let table = parse_ok("t = { a.b = 1, a.c = 2 }\n");
    assert_eq!(table.get_int_path("t.a.b"), Some(1));
    assert_eq!(table.get_int_path("t.a.c"), Some(2));
}

#[test]
fn arrays_of_tables_and_sub_tables() {
    let source = r#"
[[p]]
name = "one"

[p.q]
value = 1

[[p]]
name = "two"

[p.q]
value = 2
"#;
    let table = parse_ok(source);
    let products = table.get_array("p").unwrap();
    assert_eq!(products.len(), 2);
    let tables: Vec<&Table> = table.get("p").unwrap().array_of_tables().unwrap();
    assert_eq!(tables[0].get_str("name"), Some("one"));
    assert_eq!(tables[0].get_int_path("q.value"), Some(1));
    assert_eq!(tables[1].get_str("name"), Some("two"));
    assert_eq!(tables[1].get_int_path("q.value"), Some(2));

    assert_err_kind("[[p]]\n[p]\n", "DuplicateTable");
    assert_err_kind("[p]\n[[p]]\n", "DuplicateTable");
    assert_err_kind("p = [1, 2]\n[[p]]\n", "DuplicateTable");
}

#[test]
fn strings_all_four_kinds() {
    let table = parse_ok(
        r#"
basic = "I'm a string. \"You can quote me\"."
literal = 'C:\Users\nodejs\templates'
multiline = """
Roses are red
Violets are blue"""
multiline_literal = '''
The first newline is
trimmed in raw strings.
'''
"#,
    );
    assert_eq!(
        table.get_str("basic"),
        Some("I'm a string. \"You can quote me\".")
    );
    assert_eq!(
        table.get_str("literal"),
        Some("C:\\Users\\nodejs\\templates")
    );
    assert_eq!(
        table.get_str("multiline"),
        Some("Roses are red\nViolets are blue")
    );
    assert_eq!(
        table.get_str("multiline_literal"),
        Some("The first newline is\ntrimmed in raw strings.\n")
    );
}

#[test]
fn multiline_escapes_and_line_endings() {
    let table = parse_ok(
        "str = \"\"\"\\\n    The quick brown \\\n    fox jumps over \\\n    the lazy dog.\\\n    \"\"\"\n",
    );
    assert_eq!(
        table.get_str("str"),
        Some("The quick brown fox jumps over the lazy dog.")
    );

    let table = parse_ok("escapes = \"\"\"tab\\there\\u00e9\\U0001F680\"\"\"\n");
    assert_eq!(table.get_str("escapes"), Some("tab\there\u{e9}\u{1F680}"));

    let table = parse_ok("windows = \"\"\"a\r\nb\"\"\"\n");
    assert_eq!(table.get_str("windows"), Some("a\nb"));

    let table = parse_ok("raw = '''no \\t escapes'''\n");
    assert_eq!(table.get_str("raw"), Some("no \\t escapes"));
}

#[test]
fn quote_boundaries() {
    let table = parse_ok("a = \"\"\"\"\"\"\nb = \"\"\"\"\"\"\"\nc = ''''''\nd = '''''''\n");
    assert_eq!(table.get_str("a"), Some(""));
    assert_eq!(table.get_str("b"), Some("\""));
    assert_eq!(table.get_str("c"), Some(""));
    assert_eq!(table.get_str("d"), Some("'"));

    let table = parse_ok("a = \"\"\"\"a\"\"\"\"\nb = ''''a''''\n");
    assert_eq!(table.get_str("a"), Some("\"a\""));
    assert_eq!(table.get_str("b"), Some("'a'"));
}

#[test]
fn string_errors() {
    assert_err_kind("a = \"unterminated\n", "UnterminatedString");
    assert_err_kind("a = \"bad \\q escape\"\n", "InvalidEscape");
    assert_err_kind("a = \"\\uD800\"\n", "InvalidUnicodeEscape");
    assert_err_kind("a = \"raw \u{1} control\"\n", "ControlCharacterInString");
    assert_err_kind("a = 'unterminated\n", "UnterminatedString");
    assert_err_kind("a = \"\"\"open\n", "UnterminatedString");
}

#[test]
fn integers() {
    let table = parse_ok(
        "a = 42\nb = +42\nc = -17\nd = 1_000_000\ne = 0xDEADBEEF\nf = 0o755\ng = 0b1101\nh = 0\ni = -0\n",
    );
    assert_eq!(table.get_int("a"), Some(42));
    assert_eq!(table.get_int("b"), Some(42));
    assert_eq!(table.get_int("c"), Some(-17));
    assert_eq!(table.get_int("d"), Some(1_000_000));
    assert_eq!(table.get_int("e"), Some(0xDEAD_BEEF));
    assert_eq!(table.get_int("f"), Some(0o755));
    assert_eq!(table.get_int("g"), Some(0b1101));
    assert_eq!(table.get_int("h"), Some(0));

    assert_err_kind("a = 9223372036854775808\n", "IntegerOutOfRange");
    assert_err_kind("a = -9223372036854775809\n", "IntegerOutOfRange");
    assert_err_kind("a = 0xFFFFFFFFFFFFFFFFF\n", "IntegerOutOfRange");
    assert_err_kind("a = 0123\n", "InvalidNumber");
    assert_err_kind("a = 1__0\n", "InvalidNumber");
    assert_err_kind("a = 0x\n", "InvalidNumber");
    assert_err_kind("a = 0xZZ\n", "InvalidNumber");
}

#[test]
fn floats() {
    let table = parse_ok(
        "a = 3.25\nb = -0.01\nc = 5e+22\nd = 6.626e-34\ne = 224_617.445_991_228\nf = inf\ng = -inf\nh = nan\ni = +inf\nj = 1.0\n",
    );
    assert_eq!(table.get_float("a"), Some(3.25));
    assert_eq!(table.get_float("b"), Some(-0.01));
    assert_eq!(table.get_float("c"), Some(5e22));
    assert_eq!(table.get_float("d"), Some(6.626e-34));
    assert_eq!(table.get_float("e"), Some(224_617.445_991_228));
    assert_eq!(table.get_float("f"), Some(f64::INFINITY));
    assert_eq!(table.get_float("g"), Some(f64::NEG_INFINITY));
    assert!(table.get_float("h").unwrap().is_nan());
    assert_eq!(table.get_float("i"), Some(f64::INFINITY));
    assert_eq!(table.get_float("j"), Some(1.0));

    // Floating point values are not integers.
    assert_eq!(table.get_int("a"), None);
    assert_err_kind("a = 1.\n", "InvalidNumber");
    assert_err_kind("a = .5\n", "InvalidNumber");
    assert_err_kind("a = 01.5\n", "InvalidNumber");
}

#[test]
fn datetimes_all_variants() {
    let table = parse_ok(
        "odt = 1979-05-27T07:32:00-08:00\nz = 1979-05-27T07:32:00Z\nldt = 1979-05-27T00:32:00\nld = 1979-05-27\nlt = 07:32:00\nfrac = 07:32:00.999\nspace = 1979-05-27 07:32:00\n",
    );

    let odt = table.get("odt").unwrap().as_datetime().unwrap();
    assert_eq!(odt.date.unwrap().day, 27);
    assert_eq!(odt.time.unwrap().hour, 7);
    assert_eq!(
        odt.offset,
        Some(Offset::Custom {
            hours: -8,
            minutes: 0
        })
    );
    assert_eq!(odt.to_string(), "1979-05-27T07:32:00-08:00");

    let z = table.get("z").unwrap().as_datetime().unwrap();
    assert_eq!(z.offset, Some(Offset::Zulu));
    assert_eq!(z.to_string(), "1979-05-27T07:32:00Z");

    let ldt = table.get("ldt").unwrap().as_datetime().unwrap();
    assert_eq!(ldt.offset, None);
    assert_eq!(ldt.to_string(), "1979-05-27T00:32:00");

    let ld = table.get("ld").unwrap().as_datetime().unwrap();
    assert_eq!(ld.time, None);
    assert_eq!(ld.to_string(), "1979-05-27");

    let lt = table.get("lt").unwrap().as_datetime().unwrap();
    assert_eq!(lt.date, None);
    assert_eq!(lt.to_string(), "07:32:00");

    let frac = table.get("frac").unwrap().as_datetime().unwrap();
    assert_eq!(frac.time.unwrap().nanosecond, 999_000_000);
    assert_eq!(frac.to_string(), "07:32:00.999");

    let space = table.get("space").unwrap().as_datetime().unwrap();
    assert_eq!(space.to_string(), "1979-05-27T07:32:00");
}

#[test]
fn invalid_calendar_values_are_rejected() {
    assert_err_kind("d = 1979-02-30\n", "InvalidDatetime");
    assert_err_kind("d = 1979-13-01\n", "InvalidDatetime");
    assert_err_kind("d = 1979-04-31\n", "InvalidDatetime");
    assert_err_kind("t = 25:00:00\n", "InvalidDatetime");
    assert_err_kind("t = 07:61:00\n", "InvalidDatetime");
    assert_err_kind("d = 1979-05-27T07:32\n", "InvalidDatetime");

    // Leap years are honoured.
    assert!(akku::toml::parse("d = 1980-02-29\n").is_ok());
    assert!(akku::toml::parse("d = 2000-02-29\n").is_ok());
    assert!(akku::toml::parse("d = 1900-02-29\n").is_err());
    assert!(akku::toml::parse("d = 1979-02-29\n").is_err());
}

#[test]
fn datetime_type_api() {
    let datetime = Datetime::parse("1979-05-27T07:32:00Z").unwrap();
    assert_eq!(datetime.to_string(), "1979-05-27T07:32:00Z");
    assert!(Datetime::parse("").is_err());
    assert!(Datetime::parse("1979-05-27T07:32:00Q").is_err());
    let custom = Datetime::parse("1979-05-27T07:32:00+05:30").unwrap();
    assert_eq!(custom.to_string(), "1979-05-27T07:32:00+05:30");
    let from_str: Datetime = "1979-05-27".parse().unwrap();
    assert_eq!(from_str.to_string(), "1979-05-27");
}

#[test]
fn depth_limit_is_enforced_without_recursion_overflow() {
    fn nested(depth: usize) -> String {
        let mut text = String::from("a = ");
        text.push_str(&"[".repeat(depth));
        text.push_str(&"]".repeat(depth));
        text.push('\n');
        text
    }

    assert_eq!(
        akku::toml::parse(&nested(64))
            .unwrap()
            .get_array("a")
            .unwrap()
            .len(),
        1
    );
    assert_err_kind(&nested(65), "MaxDepthExceeded");

    let input = format!("a = {}\n", "[".repeat(100_000));
    let handle = std::thread::Builder::new()
        .stack_size(128 * 1024)
        .spawn(move || akku::toml::parse(&input).is_err())
        .unwrap();
    assert!(handle.join().unwrap());
}

#[test]
fn entry_limit_is_enforced() {
    let mut input = String::new();
    for index in 0..101 {
        input.push_str(&format!("k{index} = {index}\n"));
    }
    let options = akku::toml::ParseOptions {
        max_depth: 64,
        max_table_entries: 100,
    };
    let error = akku::toml::parse_with(&input, options).unwrap_err();
    assert!(matches!(
        error.kind(),
        ErrorKind::MaxEntriesExceeded { limit: 100 }
    ));
    // The default limit accepts the same document.
    assert!(akku::toml::parse(&input).is_ok());
}

#[test]
fn arrays() {
    let table = parse_ok(
        "mixed = [1, \"two\", 3.0, true, [4], { five = 5 }]\ntrailing = [1, 2, ]\nmultiline = [\n  1,\n  2, # comment\n]\nempty = []\n",
    );
    assert_eq!(table.get_array("mixed").unwrap().len(), 6);
    assert_eq!(table.get_array("trailing").unwrap().len(), 2);
    assert_eq!(table.get_array("multiline").unwrap().len(), 2);
    assert!(table.get_array("empty").unwrap().is_empty());

    assert_err_kind("a = [1 2]\n", "Expected");
    assert_err_kind("a = [1,\n", "UnexpectedEof");
}

#[test]
fn table_api() {
    let mut table = Table::new();
    assert!(table.is_empty());
    table.insert("a", Value::from(1));
    table.insert("b", Value::from(2));
    assert_eq!(table.insert("a", Value::from(3)), Some(Value::from(1)));
    assert_eq!(table.keys().collect::<Vec<_>>(), ["a", "b"]);
    assert_eq!(table.len(), 2);
    assert!(table.contains_key("b"));
    assert_eq!(table.get_int("a"), Some(3));
    assert_eq!(table.keys().next(), Some("a"));
    assert_eq!(table.remove("a"), Some(Value::from(3)));
    assert_eq!(table.remove("a"), None);
    assert_eq!(table.iter().count(), 1);
    assert_eq!(table["b"], Value::from(2));

    let collected: Table = [("x".to_string(), Value::from(1))].into_iter().collect();
    assert_eq!(collected.get_int("x"), Some(1));
    assert_eq!(collected.into_iter().count(), 1);

    table.insert("t", Value::Table(Table::new()));
    let nested = table.get_mut("t").unwrap().as_table_mut().unwrap();
    nested.insert("inner", Value::from(true));
    assert_eq!(table.get_bool_path("t.inner"), Some(true));
    assert!(table.get_path_mut("t.inner").is_some());
    assert!(table.get_path("t.missing").is_none());
    assert!(table.get_path("b.c").is_none());
}

/// Dotted paths resolve quoted segments and array indices.
#[test]
fn dotted_path_lookup() {
    let source = r#"
[server.tls]
"cert.file" = "/etc/cert.pem"

[[products]]
name = "alpha"
price = 1.5

[[products]]
name = "beta"
price = 2.5
"#;
    let table = parse_ok(source);
    assert_eq!(
        table.get_str_path("server.tls.\"cert.file\""),
        Some("/etc/cert.pem")
    );
    assert_eq!(table.get_str_path("products.0.name"), Some("alpha"));
    assert_eq!(table.get_str_path("products.1.name"), Some("beta"));
    assert_eq!(
        table.get_path("products.1.price").unwrap().as_float(),
        Some(2.5)
    );
    assert!(table.get_path("products.2.name").is_none());
    assert!(table.get_path("products.name").is_none());
}

#[test]
fn writer_layout_and_round_trip() {
    let source = r#"title = "akku"

[server]
host = "localhost"
port = 8080
tls = true

[server.limits]
max = 10

[[products]]
name = "one"

[[products]]
name = "two"

[products.details]
weight = 1.5
"#;
    let table = parse_ok(source);
    let written = to_string(&table);
    assert_eq!(written, source);

    let round_tripped = parse_ok(&written);
    assert_eq!(round_tripped, table);
    assert!(Value::Table(round_tripped).deep_eq(&Value::Table(table.clone())));

    let mut buffer = Vec::new();
    akku::toml::write_to(&table, &mut buffer).unwrap();
    assert_eq!(String::from_utf8(buffer).unwrap(), source);
}

#[test]
fn writer_values_round_trip() {
    let source = r#"int = 42
float = 1.5
whole = 2.0
nan = nan
inf = inf
neg_inf = -inf
bool = false
text = "quote \" and \\ backslash and \n newline"
empty_table = {}
empty_array = []
mixed = [1, "two", 3.0, true]
long = [1, 2, 3, 4, 5]
date = 1979-05-27
time = 07:32:00
local = 1979-05-27T07:32:00
offset = 1979-05-27T07:32:00+05:30
inline = { a = 1, b = "two" }
"#;
    let table = parse_ok(source);
    let written = to_string(&table);
    let round_tripped = parse_ok(&written);
    assert!(
        Value::Table(round_tripped.clone()).deep_eq(&Value::Table(table)),
        "round trip changed the document:\n{written}"
    );
    assert_eq!(round_tripped.get_int("whole"), None);
    assert_eq!(round_tripped.get_float("whole"), Some(2.0));
    assert!(round_tripped
        .get("nan")
        .unwrap()
        .deep_eq(&Value::Float(f64::NAN)));
    assert_eq!(round_tripped.get_int_path("inline.a"), Some(1));
}

#[test]
fn writer_options() {
    let table = parse_ok("[b]\ny = 2\n[a]\nx = 1\n");

    let sorted = to_string_with(
        &table,
        SerializeOptions {
            sort_keys: true,
            ..SerializeOptions::default()
        },
    );
    assert!(sorted.starts_with("[a]\nx = 1\n"), "{sorted}");
    assert!(sorted.contains("[b]\ny = 2"));

    let compact = to_string_with(
        &table,
        SerializeOptions {
            pretty: false,
            ..SerializeOptions::default()
        },
    );
    assert!(!compact.contains("\n\n"), "{compact}");

    let inline_arrays = to_string_with(
        &table,
        SerializeOptions {
            array_of_tables: false,
            ..SerializeOptions::default()
        },
    );
    assert!(!inline_arrays.contains("[["), "{inline_arrays}");

    let arrays = parse_ok("[[p]]\nname = \"a\"\n[[p]]\nname = \"b\"\n");
    let inline = to_string_with(
        &arrays,
        SerializeOptions {
            array_of_tables: false,
            ..SerializeOptions::default()
        },
    );
    assert!(
        inline.starts_with("p = [{ name = \"a\" }, { name = \"b\" }]"),
        "{inline}"
    );
    assert_eq!(parse_ok(&inline), arrays);
}

#[test]
fn deep_eq_treats_nan_as_equal() {
    let left = Value::Float(f64::NAN);
    let right = Value::Float(f64::NAN);
    assert_ne!(left, right);
    assert!(left.deep_eq(&right));

    let left = Value::Array(vec![Value::Float(f64::NAN), Value::from(1)]);
    let right = Value::Array(vec![Value::Float(f64::NAN), Value::from(1)]);
    assert!(left.deep_eq(&right));

    let left = Value::from(1);
    assert!(!left.deep_eq(&Value::from(2)));
    assert_eq!(Value::from(1).type_name(), "integer");
    assert_eq!(Value::from("x").type_name(), "string");
    assert_eq!(Value::Table(Table::new()).type_name(), "table");
    assert_eq!(Value::Array(Vec::new()).type_name(), "array");
    assert_eq!(Value::from(true).type_name(), "boolean");
    assert_eq!(Value::from(1.0).type_name(), "float");
}

#[test]
fn error_positions_point_at_the_failure() {
    let error = akku::toml::parse("a = 1\nb = \n").unwrap_err();
    assert_eq!(error.position().line, 2);
    assert_eq!(error.position().column, 5);
    assert!(matches!(
        error.kind(),
        ErrorKind::UnexpectedByte { .. } | ErrorKind::Expected { .. }
    ));

    let error = akku::toml::parse("[a]\n[b\n").unwrap_err();
    assert_eq!(error.position().line, 2);
}

#[test]
fn display_and_from_str() {
    let source = "a = 1\n\n[b]\nc = \"d\"\n";
    let table: Table = source.parse().unwrap();
    assert_eq!(table.to_string(), source);
    assert_eq!(akku::toml::from_str(source).unwrap(), table);
    assert_eq!(Value::from(1).to_string(), "1");
    assert_eq!(Value::from("x").to_string(), "\"x\"");
    assert_eq!(Value::from(f64::NAN).to_string(), "nan");
    assert_eq!(Value::from(f64::INFINITY).to_string(), "inf");
}

#[test]
fn line_ending_backslash_requires_a_newline() {
    // `\` inside a multiline basic string must be followed by optional
    // spaces/tabs AND a newline; otherwise it is an invalid escape.
    assert_err_kind("a = \"\"\"no newline\\  b\"\"\"\n", "InvalidEscape");
    assert_err_kind("a = \"\"\"no newline\\\tb\"\"\"\n", "InvalidEscape");

    // A backslash followed by a real line ending still trims whitespace.
    let table = parse_ok("a = \"\"\"one\\\n    two\"\"\"\n");
    assert_eq!(table.get_str("a"), Some("onetwo"));
}

#[test]
fn raw_del_is_rejected_in_all_string_kinds() {
    assert_err_kind("a = \"x\u{7f}\"\n", "ControlCharacterInString");
    assert_err_kind("a = 'x\u{7f}'\n", "ControlCharacterInString");
    assert_err_kind("a = \"\"\"x\u{7f}\"\"\"\n", "ControlCharacterInString");
    assert_err_kind("a = '''x\u{7f}'''\n", "ControlCharacterInString");

    // An escaped DEL stays allowed.
    let table = parse_ok("a = \"x\\u007F\"\n");
    assert_eq!(table.get_str("a"), Some("x\u{7f}"));
}

#[test]
fn double_signs_are_rejected() {
    assert_err_kind("a = +-inf\n", "InvalidNumber");
    assert_err_kind("a = --inf\n", "InvalidNumber");
    assert_err_kind("a = ++nan\n", "InvalidNumber");
    assert_err_kind("a = +-1\n", "InvalidNumber");
    assert_err_kind("a = -+1\n", "InvalidNumber");

    // A single sign stays valid.
    let table = parse_ok("a = +1\nb = -1\nc = +inf\nd = -inf\n");
    assert_eq!(table.get_int("a"), Some(1));
    assert_eq!(table.get_int("b"), Some(-1));
    assert_eq!(table.get_float("c"), Some(f64::INFINITY));
    assert_eq!(table.get_float("d"), Some(f64::NEG_INFINITY));
}

#[test]
fn underscore_before_exponent_is_rejected() {
    assert_err_kind("a = 5_e3\n", "InvalidNumber");
    assert_err_kind("a = 5e_3\n", "InvalidNumber");
    assert_err_kind("a = 5_E\n", "InvalidNumber");

    // Underscores between digits stay valid.
    let table = parse_ok("a = 224_617.445_991_228\nb = 1_2_3\n");
    assert_eq!(table.get_float("a"), Some(224_617.445_991_228));
    assert_eq!(table.get_int("b"), Some(123));
}

#[test]
fn unit_separator_keys_do_not_collide_with_dotted_paths() {
    // Under the plain-separator path encoding the quoted key `["a\u001Fb"]`
    // and the header path `[a.b]` produced the same defs entry.
    let table = parse_ok("[\"a\\u001Fb\"]\ny = 2\n[a.b]\nx = 1\n");
    assert_eq!(table.get_int_path("a.b.x"), Some(1));
    assert_eq!(
        table
            .get("a\u{1f}b")
            .unwrap()
            .as_table()
            .unwrap()
            .get_int("y"),
        Some(2)
    );

    let table = parse_ok("[a.b]\nx = 1\n[\"a\\u001Fb\"]\ny = 2\n");
    assert_eq!(table.get_int_path("a.b.x"), Some(1));
    assert_eq!(
        table
            .get("a\u{1f}b")
            .unwrap()
            .as_table()
            .unwrap()
            .get_int("y"),
        Some(2)
    );

    // Redefining a table stays an error in both orderings.
    assert_err_kind(
        "[\"a\\u001Fb\"]\n[a.b.c]\n[\"a\\u001Fb\"]\n",
        "DuplicateTable",
    );
}
