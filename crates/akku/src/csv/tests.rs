//! Integration tests for `akku::csv`.

use std::io::BufReader;

use akku::csv::{
    CsvOptions, ErrorKind, FromField, HeaderRecord, QuoteStyle, Reader, Record, Terminator, Trim,
    Writer,
};

const SMALL_BUFFER: usize = 8;
const LARGE_BUFFER: usize = 8192;

/// Parse `data` without headers and return the records as plain vectors.
fn parse_with_capacity(data: &str, capacity: usize) -> Vec<Vec<String>> {
    let options = CsvOptions::new().has_headers(false);
    let mut reader =
        Reader::with_options(BufReader::with_capacity(capacity, data.as_bytes()), options);
    reader
        .read_all()
        .unwrap()
        .iter()
        .map(|record| record.iter().map(str::to_string).collect())
        .collect()
}

/// Parse `data` with both a tiny and a large buffer and assert they agree.
#[track_caller]
fn parse_both(data: &str) -> Vec<Vec<String>> {
    let small = parse_with_capacity(data, SMALL_BUFFER);
    let large = parse_with_capacity(data, LARGE_BUFFER);
    assert_eq!(
        small, large,
        "buffer size changed the parsed result for {data:?}"
    );
    small
}

fn row(fields: &[&str]) -> Vec<String> {
    fields.iter().map(|field| (*field).to_string()).collect()
}

#[test]
fn rfc4180_simple_records() {
    assert_eq!(parse_both("aaa,bbb,ccc"), vec![row(&["aaa", "bbb", "ccc"])]);
    assert_eq!(
        parse_both("aaa,bbb,ccc\r\nzzz,yyy,xxx\r\n"),
        vec![row(&["aaa", "bbb", "ccc"]), row(&["zzz", "yyy", "xxx"])]
    );
    assert_eq!(
        parse_both("aaa,bbb,ccc\nzzz,yyy,xxx\n"),
        vec![row(&["aaa", "bbb", "ccc"]), row(&["zzz", "yyy", "xxx"])]
    );
    assert!(parse_both("").is_empty());
    assert!(parse_both("\r\n").is_empty());
}

#[test]
fn rfc4180_quoted_fields() {
    assert_eq!(parse_both("\"a,a\",b,c"), vec![row(&["a,a", "b", "c"])]);
    assert_eq!(parse_both("\"a\"\"b\",c"), vec![row(&["a\"b", "c"])]);
    assert_eq!(
        parse_both("\"aaa\",\"b\r\nbb\",\"ccc\"\r\nzzz,yyy,xxx"),
        vec![row(&["aaa", "b\r\nbb", "ccc"]), row(&["zzz", "yyy", "xxx"]),]
    );
    assert_eq!(parse_both("\"\"\"\""), vec![row(&["\""])]);
    assert_eq!(parse_both("\"\",\"\""), vec![row(&["", ""])]);
}

#[test]
fn rfc4180_empty_and_spaced_fields() {
    assert_eq!(parse_both("a,,b,"), vec![row(&["a", "", "b", ""])]);
    assert_eq!(parse_both(" a , b "), vec![row(&[" a ", " b "])]);
    // A quoted field keeps its spaces unless trimming is enabled.
    assert_eq!(parse_both("\" a \",b"), vec![row(&[" a ", "b"])]);
}

#[test]
fn rfc4180_utf8_fields_split_across_buffers() {
    let data = "\"grüße\",\"日本語\",\"emoji 🚀\"\n";
    assert_eq!(
        parse_both(data),
        vec![row(&["grüße", "日本語", "emoji 🚀"])]
    );
}

#[test]
fn bom_is_stripped_once() {
    let data = "\u{feff}name,age\nada,36\n";
    for capacity in [SMALL_BUFFER, LARGE_BUFFER] {
        let mut reader = Reader::new(BufReader::with_capacity(capacity, data.as_bytes()));
        let headers = reader.headers().unwrap().clone();
        assert_eq!(headers.names(), ["name", "age"]);
        let record = reader.read_all().unwrap();
        assert_eq!(record.len(), 1);
        assert_eq!(record[0].get(0), Some("ada"));
    }
}

#[test]
fn line_and_record_positions() {
    let data = "a,\"x\r\ny\"\nb,c\n";
    let options = CsvOptions::new().has_headers(false);
    let mut reader = Reader::with_options(
        BufReader::with_capacity(SMALL_BUFFER, data.as_bytes()),
        options,
    );

    assert_eq!(reader.position().line, 1);
    assert_eq!(reader.position().record, 0);

    let mut record = Record::new();
    assert!(reader.read_record(&mut record).unwrap());
    assert_eq!(record.get(1), Some("x\r\ny"));
    assert_eq!(reader.position().line, 3);
    assert_eq!(reader.position().record, 1);

    assert!(reader.read_record(&mut record).unwrap());
    assert_eq!(reader.position().line, 4);
    assert_eq!(reader.position().record, 2);

    assert!(!reader.read_record(&mut record).unwrap());
}

#[test]
fn comments_are_skipped() {
    let data = "# a comment\nname,age\n# another\nada,36\n";
    let options = CsvOptions::new().comment(Some(b'#'));
    let mut reader = Reader::with_options(
        BufReader::with_capacity(SMALL_BUFFER, data.as_bytes()),
        options,
    );
    let headers = reader.headers().unwrap().names().to_vec();
    assert_eq!(headers, ["name", "age"]);
    let records = reader.read_all().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].as_slice(), ["ada", "36"]);
}

#[test]
fn trim_modes() {
    let data = " a , b \n c , d \n";
    let cases = [
        (Trim::None, row(&[" a ", " b "]), row(&[" c ", " d "])),
        (Trim::Fields, row(&[" a ", " b "]), row(&["c", "d"])),
        (Trim::Headers, row(&["a", "b"]), row(&[" c ", " d "])),
        (Trim::All, row(&["a", "b"]), row(&["c", "d"])),
    ];
    for (trim, expected_headers, expected_record) in cases {
        let options = CsvOptions::new().trim(trim);
        let mut reader = Reader::with_options(
            BufReader::with_capacity(SMALL_BUFFER, data.as_bytes()),
            options,
        );
        let headers = reader.headers().unwrap().names().to_vec();
        assert_eq!(headers, expected_headers, "headers for {trim:?}");
        let records = reader.read_all().unwrap();
        assert_eq!(
            records[0].as_slice(),
            expected_record.as_slice(),
            "for {trim:?}"
        );
    }
}

#[test]
fn trim_does_not_touch_quoted_fields() {
    let data = "a,b\n\" x \", y \n";
    let options = CsvOptions::new().trim(Trim::All);
    let mut reader = Reader::with_options(data.as_bytes(), options);
    let records = reader.read_all().unwrap();
    assert_eq!(records[0].as_slice(), [" x ", "y"]);
}

#[test]
fn unequal_lengths_strict_and_flexible() {
    let data = "a,b\n1,2,3\n";
    let mut reader = Reader::new(data.as_bytes());
    let error = reader.read_all().unwrap_err();
    match error.kind() {
        ErrorKind::UnequalLengths { expected, got } => {
            assert_eq!((*expected, *got), (2, 3));
        }
        other => panic!("unexpected error: {other}"),
    }

    let options = CsvOptions::new().flexible_fields(true);
    let mut reader = Reader::with_options(data.as_bytes(), options);
    let records = reader.read_all().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].as_slice(), ["1", "2", "3"]);
}

#[test]
fn custom_delimiter_and_quote() {
    let data = "a;b\n'x;y';z\n";
    let options = CsvOptions::new().delimiter(b';').quote(b'\'');
    let mut reader = Reader::with_options(
        BufReader::with_capacity(SMALL_BUFFER, data.as_bytes()),
        options,
    );
    let headers = reader.headers().unwrap().names().to_vec();
    assert_eq!(headers, ["a", "b"]);
    let records = reader.read_all().unwrap();
    assert_eq!(records[0].as_slice(), ["x;y", "z"]);
}

#[test]
fn escape_byte() {
    let data = "\"a\\\"b\",c\n";
    let options = CsvOptions::new()
        .escape(Some(b'\\'))
        .double_quote(false)
        .has_headers(false);
    let mut reader = Reader::with_options(data.as_bytes(), options);
    let records = reader.read_all().unwrap();
    assert_eq!(records[0].as_slice(), ["a\"b", "c"]);
}

#[test]
fn invalid_delimiter_is_rejected() {
    let options = CsvOptions::new().delimiter(b'\n');
    let mut reader = Reader::with_options("a,b\n".as_bytes(), options);
    let error = reader.read_record(&mut Record::new()).unwrap_err();
    assert!(matches!(
        error.kind(),
        ErrorKind::InvalidDelimiter { byte: b'\n' }
    ));

    let options = CsvOptions::new().delimiter(b'"');
    let mut writer = Writer::with_options(Vec::new(), options);
    let error = writer.write_record(["a"]).unwrap_err();
    assert!(matches!(
        error.kind(),
        ErrorKind::InvalidDelimiter { byte: b'"' }
    ));
}

#[test]
fn comment_and_escape_option_collisions_are_rejected() {
    // delimiter, quote, CR and LF are all invalid comment bytes.
    for byte in *b",\"\r\n" {
        let options = CsvOptions::new().comment(Some(byte));
        let mut reader = Reader::with_options("a,b\n".as_bytes(), options);
        let error = reader.read_record(&mut Record::new()).unwrap_err();
        assert!(matches!(error.kind(), ErrorKind::InvalidDelimiter { .. }));
    }

    let options = CsvOptions::new().comment(Some(b','));
    let mut writer = Writer::with_options(Vec::new(), options);
    assert!(writer.write_record(["a", "b"]).is_err());

    // The escape byte must not be the quote byte.
    let options = CsvOptions::new().escape(Some(b'"'));
    let mut reader = Reader::with_options("a,b\n".as_bytes(), options);
    let error = reader.read_record(&mut Record::new()).unwrap_err();
    assert!(matches!(
        error.kind(),
        ErrorKind::InvalidDelimiter { byte: b'"' }
    ));

    let options = CsvOptions::new().escape(Some(b'"'));
    let mut writer = Writer::with_options(Vec::new(), options);
    assert!(writer.write_record(["a", "b"]).is_err());
}

#[test]
fn junk_after_closing_quote_is_rejected() {
    let data = "a,\"b\"x,c\n";
    let mut reader = Reader::with_options(data.as_bytes(), CsvOptions::new().has_headers(false));
    let error = reader.read_all().unwrap_err();
    assert!(matches!(
        error.kind(),
        ErrorKind::UnexpectedAfterQuote { byte: b'x' }
    ));

    // Junk after optional horizontal whitespace is rejected as well.
    let data = "a,\"b\"  x\n";
    let mut reader = Reader::with_options(data.as_bytes(), CsvOptions::new().has_headers(false));
    let error = reader.read_all().unwrap_err();
    assert!(matches!(
        error.kind(),
        ErrorKind::UnexpectedAfterQuote { byte: b'x' }
    ));

    // Horizontal whitespace after a closing quote stays allowed.
    assert_eq!(parse_both("\" a \",\" b \"\n"), vec![row(&[" a ", " b "])]);
}

#[test]
fn unterminated_quote_is_reported() {
    let mut reader =
        Reader::with_options("a,\"b\n".as_bytes(), CsvOptions::new().has_headers(false));
    let error = reader.read_all().unwrap_err();
    assert!(matches!(error.kind(), ErrorKind::UnterminatedQuote));
    assert_eq!(error.line(), 2);
}

#[test]
fn quote_styles() {
    let mut writer = Writer::new(Vec::new());
    writer.write_record(["plain", "a,b", "c\"d"]).unwrap();
    assert_eq!(writer.into_string(), "plain,\"a,b\",\"c\"\"d\"\r\n");

    let mut writer = Writer::with_options(
        Vec::new(),
        CsvOptions::new().quote_style(QuoteStyle::Always),
    );
    writer.write_record(["a", "b"]).unwrap();
    assert_eq!(writer.into_string(), "\"a\",\"b\"\r\n");

    let mut writer =
        Writer::with_options(Vec::new(), CsvOptions::new().quote_style(QuoteStyle::Never));
    writer.write_record(["a", "b,c"]).unwrap();
    assert_eq!(writer.into_string(), "a,b,c\r\n");
}

#[test]
fn quote_style_never_writes_raw_bytes() {
    // `Never` writes the field bytes as-is: quote characters are not doubled
    // and escapes are not applied — the caller guarantees safe data.
    let mut writer =
        Writer::with_options(Vec::new(), CsvOptions::new().quote_style(QuoteStyle::Never));
    writer.write_record(["he\"llo", "c,d", ""]).unwrap();
    assert_eq!(writer.into_string(), "he\"llo,c,d,\r\n");

    let mut writer = Writer::with_options(
        Vec::new(),
        CsvOptions::new()
            .quote_style(QuoteStyle::Never)
            .quote(b'\'')
            .escape(Some(b'\\'))
            .double_quote(false),
    );
    writer.write_record(["h'i", "l\\o"]).unwrap();
    assert_eq!(writer.into_string(), "h'i,l\\o\r\n");
}

#[test]
fn writer_terminators_and_flush() {
    let mut writer = Writer::with_options(Vec::new(), CsvOptions::new().terminator(Terminator::Lf));
    writer.write_record(["a", "b"]).unwrap();
    writer.flush().unwrap();
    assert_eq!(writer.into_string(), "a,b\n");

    let mut writer = Writer::new(Vec::new());
    writer
        .write_headers(&HeaderRecord::new(["x", "y"]))
        .unwrap();
    let mut data_record = Record::new();
    data_record.push("1");
    data_record.push("2");
    writer.write_all(&[data_record]).unwrap();
    assert_eq!(writer.into_string(), "x,y\r\n1,2\r\n");
}

#[test]
fn round_trip_corpus() {
    let corpora = [
        "plain,field\nwith,comma\n",
        "\"a,b\",\"c\"\"d\"\n\"line\r\nbreak\",z\n",
        "\"\",\"\"\n\"  spaced  \",x\n",
        "grüße,日本語\n🚀,ok\n",
    ];
    for corpus in corpora {
        let records = parse_both(corpus);
        let mut record_list = Vec::new();
        for fields in &records {
            let mut record = Record::new();
            for field in fields {
                record.push(field.clone());
            }
            record_list.push(record);
        }
        let mut writer = Writer::new(Vec::new());
        writer.write_all(&record_list).unwrap();
        let written = writer.into_string();
        assert_eq!(parse_both(&written), records, "corpus {corpus:?}");
    }
}

#[test]
fn from_field_conversions() {
    assert_eq!(i64::from_field(" 42 ").unwrap(), 42);
    assert_eq!(u8::from_field("255").unwrap(), 255);
    assert!(u8::from_field("256").is_err());
    assert_eq!(f64::from_field("1.5").unwrap(), 1.5);
    assert_eq!(f32::from_field("-2.25").unwrap(), -2.25);
    for truthy in ["true", "TRUE", "1", "yes", "On"] {
        assert!(bool::from_field(truthy).unwrap(), "{truthy}");
    }
    for falsy in ["false", "FALSE", "0", "no", "Off"] {
        assert!(!bool::from_field(falsy).unwrap(), "{falsy}");
    }
    assert_eq!(Option::<i32>::from_field("").unwrap(), None);
    assert_eq!(Option::<i32>::from_field("7").unwrap(), Some(7));
    assert_eq!(String::from_field("keep").unwrap(), "keep");

    let error = i32::from_field("nope").unwrap_err();
    assert!(matches!(error.kind(), ErrorKind::ParseField { .. }));
    let text = error.to_string();
    assert!(text.contains("integer"), "{text}");
    assert!(text.contains("nope"), "{text}");
}

#[test]
fn named_and_typed_access() {
    let data = "name,age,active\nada,36,yes\n";
    let mut reader = Reader::new(data.as_bytes());
    let headers = reader.headers().unwrap().clone();
    let records = reader.read_all().unwrap();
    let record = &records[0];

    assert_eq!(record.get_named(&headers, "name").unwrap(), "ada");
    assert_eq!(record.typed_named::<i64>(&headers, "age").unwrap(), 36);
    assert!(record.typed_named::<bool>(&headers, "active").unwrap());
    assert_eq!(record.typed::<String>(0).unwrap(), "ada");

    let error = record.get_named(&headers, "city").unwrap_err();
    assert!(matches!(error.kind(), ErrorKind::MissingColumn { .. }));

    let error = record.typed::<i64>(9).unwrap_err();
    assert!(matches!(error.kind(), ErrorKind::MissingField { index: 9 }));

    let duplicated = HeaderRecord::new(["a", "a", "b"]);
    assert_eq!(duplicated.index("a"), Some(0));
}

#[test]
fn record_and_header_record_basics() {
    let mut record = Record::new();
    assert!(record.is_empty());
    record.push("a");
    record.push(String::from("b"));
    assert_eq!(record.len(), 2);
    assert_eq!(record.as_slice(), ["a", "b"]);
    assert_eq!(record.get_mut(1).map(|field| field.as_str()), Some("b"));
    assert_eq!(record.iter().collect::<Vec<_>>(), ["a", "b"]);
    assert_eq!((&record).into_iter().count(), 2);
    assert_eq!(record.clone(), record);

    record.clear();
    assert!(record.is_empty());
    assert_eq!(record.get(0), None);

    let headers = HeaderRecord::from_record(Record::new());
    assert!(headers.is_empty());
    assert_eq!(headers.len(), 0);
    assert_eq!(headers.index("x"), None);
    assert_eq!(headers.iter().count(), 0);
}

#[test]
fn records_iterator() {
    let options = CsvOptions::new().has_headers(false);
    let reader = Reader::with_options(
        BufReader::with_capacity(SMALL_BUFFER, "a,b\nc,d\n".as_bytes()),
        options,
    );
    let collected: Vec<Record> = reader.records().map(|record| record.unwrap()).collect();
    assert_eq!(collected.len(), 2);
    assert_eq!(collected[1].as_slice(), ["c", "d"]);
}

#[test]
fn into_inner_returns_the_source() {
    let reader = Reader::new(BufReader::with_capacity(8, "a,b\n".as_bytes()));
    let inner = reader.into_inner();
    assert_eq!(inner.capacity(), 8);
}

#[test]
fn writer_into_inner() {
    let mut writer = Writer::new(Vec::new());
    writer.write_record(["a"]).unwrap();
    let bytes = writer.into_inner();
    assert_eq!(bytes, b"a\r\n");
}
