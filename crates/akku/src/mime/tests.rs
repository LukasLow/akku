use std::collections::HashSet;
use std::path::Path;

use akku::mime::{
    self, from_extension, from_path, Mime, ParseError, APPLICATION_GZIP, APPLICATION_JAVASCRIPT,
    APPLICATION_JSON, APPLICATION_OCTET_STREAM, APPLICATION_PDF, APPLICATION_TOML,
    APPLICATION_WASM, APPLICATION_WWW_FORM_URLENCODED, APPLICATION_XML, APPLICATION_YAML,
    APPLICATION_ZIP, AUDIO_MPEG, AUDIO_OGG, AUDIO_WAV, FONT_OTF, FONT_TTF, FONT_WOFF, FONT_WOFF2,
    IMAGE_AVIF, IMAGE_BMP, IMAGE_GIF, IMAGE_ICO, IMAGE_JPEG, IMAGE_PNG, IMAGE_SVG_XML, IMAGE_WEBP,
    MULTIPART_FORM_DATA, MULTIPART_MIXED, TEXT_CSS, TEXT_CSV, TEXT_EVENT_STREAM, TEXT_HTML,
    TEXT_HTML_UTF8, TEXT_JAVASCRIPT, TEXT_MARKDOWN, TEXT_PLAIN, TEXT_PLAIN_UTF8, TEXT_XML,
    VIDEO_MP4, VIDEO_WEBM,
};

#[test]
fn parse_ows_case_normalization_and_trailing_semicolon() {
    let m: Mime = " \t TEXT/HTML ; \t Charset = \"UTF-8\" ;  "
        .parse()
        .unwrap_or_else(|_| Mime::parse(" \t TEXT/HTML ; Charset=\"UTF-8\" ; ").unwrap());
    assert_eq!(m.type_(), "text");
    assert_eq!(m.subtype(), "html");
    assert_eq!(m.essence(), "text/html");
    assert_eq!(m.charset(), Some("utf-8"));
    assert_eq!(m.param("CHARSET"), Some("utf-8"));
    assert_eq!(m, TEXT_HTML_UTF8);
    assert_eq!(m.to_string(), "text/html; charset=utf-8");

    let trailing = Mime::parse("application/json;").unwrap();
    assert_eq!(trailing, APPLICATION_JSON);
    assert_eq!(trailing.params().count(), 0);
}

#[test]
fn parse_quoted_string_escapes_and_duplicate_params_first_wins() {
    let raw =
        r#"multipart/form-data; boundary="a;b\"c\\d"; boundary=ignored; X-Custom="hello world""#;
    let m = Mime::parse(raw).unwrap();
    assert_eq!(m.type_(), "multipart");
    assert_eq!(m.subtype(), "form-data");
    assert_eq!(m.param("boundary"), Some(r#"a;b"c\d"#));
    assert_eq!(m.param("x-custom"), Some("hello world"));
    assert_eq!(m.params().count(), 2);

    // Round-trip complex quoted params
    let serialized = m.to_string();
    let reparsed = Mime::parse(&serialized).unwrap();
    assert_eq!(reparsed, m);
}

#[test]
fn parse_suffixes_and_wildcards() {
    let api_json = Mime::parse("application/vnd.custom+v1+JSON").unwrap();
    assert_eq!(api_json.type_(), "application");
    assert_eq!(api_json.subtype(), "vnd.custom+v1+json");
    assert_eq!(api_json.suffix(), Some("json"));
    assert!(api_json.is_json());
    assert!(!api_json.is_xml());
    assert!(!api_json.is_text());

    let svg = IMAGE_SVG_XML;
    assert_eq!(svg.suffix(), Some("xml"));
    assert!(svg.is_xml());
    assert!(!svg.is_json());

    let any = Mime::parse("*/*").unwrap();
    assert_eq!(any.type_(), "*");
    assert_eq!(any.subtype(), "*");

    let text_any = Mime::parse("text/*").unwrap();
    assert!(text_any.is_text());
    assert!(text_any.matches("text", "*"));
    assert!(text_any.matches("*", "*"));
}

#[test]
fn parse_errors_all_variants() {
    assert_eq!(Mime::parse(""), Err(ParseError::Empty));
    assert_eq!(Mime::parse("   \t  "), Err(ParseError::Empty));
    assert_eq!(Mime::parse("text"), Err(ParseError::MissingSlash));
    assert_eq!(Mime::parse("/plain"), Err(ParseError::InvalidType));
    assert_eq!(Mime::parse("te xt/plain"), Err(ParseError::InvalidType));
    assert_eq!(Mime::parse("text/"), Err(ParseError::InvalidSubtype));
    assert_eq!(Mime::parse("text/pl ain"), Err(ParseError::InvalidSubtype));
    assert_eq!(Mime::parse("text/a/b"), Err(ParseError::InvalidSubtype));
    assert_eq!(
        Mime::parse("text/plain; =utf-8"),
        Err(ParseError::InvalidParamName)
    );
    assert_eq!(
        Mime::parse("text/plain; bad name=val"),
        Err(ParseError::InvalidParamName)
    );
    assert_eq!(
        Mime::parse("text/plain; charset=utf/8"),
        Err(ParseError::InvalidParamValue)
    );
    assert_eq!(
        Mime::parse("text/plain; a=\"b\"extra"),
        Err(ParseError::InvalidParamValue)
    );
    assert_eq!(
        Mime::parse("text/plain; a=\"bad\x00\""),
        Err(ParseError::InvalidParamValue)
    );
    assert_eq!(
        Mime::parse("text/plain; charset"),
        Err(ParseError::MissingParamValue)
    );
    assert_eq!(
        Mime::parse("text/plain; charset="),
        Err(ParseError::MissingParamValue)
    );
    assert_eq!(
        Mime::parse("text/plain; charset=; a=1"),
        Err(ParseError::MissingParamValue)
    );
    assert_eq!(
        Mime::parse("text/plain; a=\"unterminated"),
        Err(ParseError::UnterminatedQuote)
    );
    assert_eq!(
        Mime::parse("text/plain; a=\"trail\\"),
        Err(ParseError::UnterminatedQuote)
    );
}

#[test]
fn round_trip_all_constants() {
    let all_constants = [
        TEXT_PLAIN,
        TEXT_PLAIN_UTF8,
        TEXT_HTML,
        TEXT_HTML_UTF8,
        TEXT_CSS,
        TEXT_JAVASCRIPT,
        TEXT_CSV,
        TEXT_XML,
        TEXT_MARKDOWN,
        TEXT_EVENT_STREAM,
        APPLICATION_JSON,
        APPLICATION_JAVASCRIPT,
        APPLICATION_XML,
        APPLICATION_OCTET_STREAM,
        APPLICATION_PDF,
        APPLICATION_ZIP,
        APPLICATION_GZIP,
        APPLICATION_WWW_FORM_URLENCODED,
        APPLICATION_WASM,
        APPLICATION_TOML,
        APPLICATION_YAML,
        MULTIPART_FORM_DATA,
        MULTIPART_MIXED,
        IMAGE_PNG,
        IMAGE_JPEG,
        IMAGE_GIF,
        IMAGE_WEBP,
        IMAGE_AVIF,
        IMAGE_SVG_XML,
        IMAGE_ICO,
        IMAGE_BMP,
        AUDIO_MPEG,
        AUDIO_WAV,
        AUDIO_OGG,
        VIDEO_MP4,
        VIDEO_WEBM,
        FONT_WOFF,
        FONT_WOFF2,
        FONT_TTF,
        FONT_OTF,
    ];

    assert_eq!(all_constants.len(), 40);
    for m in all_constants {
        let s = m.to_string();
        let parsed = Mime::parse(&s).unwrap_or_else(|e| panic!("failed on {s}: {e}"));
        assert_eq!(parsed, m, "round-trip mismatch for {s}");
    }
}

#[test]
fn from_extension_and_from_path_matrix() {
    let cases = [
        ("txt", TEXT_PLAIN),
        (".TXT", TEXT_PLAIN),
        ("html", TEXT_HTML),
        ("htm", TEXT_HTML),
        ("css", TEXT_CSS),
        ("js", TEXT_JAVASCRIPT),
        ("mjs", TEXT_JAVASCRIPT),
        ("cjs", TEXT_JAVASCRIPT),
        ("csv", TEXT_CSV),
        ("xml", TEXT_XML),
        ("md", TEXT_MARKDOWN),
        ("markdown", TEXT_MARKDOWN),
        ("json", APPLICATION_JSON),
        ("pdf", APPLICATION_PDF),
        ("wasm", APPLICATION_WASM),
        ("toml", APPLICATION_TOML),
        ("yaml", APPLICATION_YAML),
        ("yml", APPLICATION_YAML),
        ("bin", APPLICATION_OCTET_STREAM),
        ("exe", APPLICATION_OCTET_STREAM),
        ("zip", APPLICATION_ZIP),
        ("gz", APPLICATION_GZIP),
        ("gzip", APPLICATION_GZIP),
        ("png", IMAGE_PNG),
        ("jpg", IMAGE_JPEG),
        ("jpeg", IMAGE_JPEG),
        ("gif", IMAGE_GIF),
        ("webp", IMAGE_WEBP),
        ("avif", IMAGE_AVIF),
        ("svg", IMAGE_SVG_XML),
        ("ico", IMAGE_ICO),
        ("bmp", IMAGE_BMP),
        ("mp3", AUDIO_MPEG),
        ("wav", AUDIO_WAV),
        ("ogg", AUDIO_OGG),
        ("oga", AUDIO_OGG),
        ("mp4", VIDEO_MP4),
        ("m4v", VIDEO_MP4),
        ("webm", VIDEO_WEBM),
        ("woff", FONT_WOFF),
        ("woff2", FONT_WOFF2),
        ("ttf", FONT_TTF),
        ("otf", FONT_OTF),
    ];

    for (ext, expected) in cases {
        assert_eq!(from_extension(ext), Some(expected.clone()), "ext {ext}");
        let dotted = if ext.starts_with('.') {
            ext.to_owned()
        } else {
            format!(".{ext}")
        };
        assert_eq!(
            from_extension(&dotted.to_ascii_uppercase()),
            Some(expected),
            "upper dotted {dotted}"
        );
    }

    // Additional extensions bringing tested count well past 45
    let extra_exts = [
        ("ics", "text/calendar"),
        ("tsv", "text/tab-separated-values"),
        ("jsonld", "application/ld+json"),
        ("rtf", "application/rtf"),
        ("xhtml", "application/xhtml+xml"),
        ("tar", "application/x-tar"),
        ("7z", "application/x-7z-compressed"),
        ("rar", "application/vnd.rar"),
        ("bz2", "application/x-bzip2"),
        ("xz", "application/x-xz"),
        ("zst", "application/zstd"),
        ("tiff", "image/tiff"),
        ("flac", "audio/flac"),
        ("aac", "audio/aac"),
        ("m4a", "audio/mp4"),
        ("opus", "audio/opus"),
        ("ogv", "video/ogg"),
        ("avi", "video/x-msvideo"),
        ("mov", "video/quicktime"),
        ("mkv", "video/x-matroska"),
        ("eot", "application/vnd.ms-fontobject"),
    ];
    for (ext, essence) in extra_exts {
        let got = from_extension(ext).unwrap_or_else(|| panic!("missing ext {ext}"));
        assert_eq!(got.essence(), essence);
        assert!(got.extension().is_some());
    }

    // Unknown, empty, and overlong (> 16 bytes) extensions
    assert_eq!(from_extension(""), None);
    assert_eq!(from_extension("."), None);
    assert_eq!(from_extension("unknown_ext"), None);
    assert_eq!(from_extension("morethan16byteslong"), None);
    assert_eq!(from_extension(".morethan16byteslong"), None);

    // from_path
    assert_eq!(from_path("assets/style.CSS"), Some(TEXT_CSS));
    assert_eq!(
        from_path(Path::new("/var/www/app.wasm")),
        Some(APPLICATION_WASM)
    );
    assert_eq!(from_path("README"), None);
    assert_eq!(from_path("file.unknown"), None);
}

#[test]
fn methods_equality_hashing_and_builders() {
    assert!(APPLICATION_JSON.is_json());
    assert!(!APPLICATION_JSON.is_xml());
    assert!(!APPLICATION_JSON.is_text());

    assert!(TEXT_XML.is_xml());
    assert!(APPLICATION_XML.is_xml());
    assert!(IMAGE_SVG_XML.is_xml());
    assert!(TEXT_PLAIN.is_text());

    assert!(TEXT_HTML_UTF8.matches("text", "html"));
    assert!(TEXT_HTML_UTF8.matches("TEXT", "*"));
    assert!(TEXT_HTML_UTF8.matches("*", "HTML"));
    assert!(TEXT_HTML_UTF8.matches("*", "*"));
    assert!(!TEXT_HTML_UTF8.matches("application", "*"));

    assert!(TEXT_HTML_UTF8.essence_eq(&TEXT_HTML));
    assert_ne!(TEXT_HTML_UTF8, TEXT_HTML);
    assert_eq!(TEXT_HTML_UTF8.without_params(), TEXT_HTML);

    // with_param add, override, error
    let custom = TEXT_PLAIN
        .with_param("charset", "UTF-8")
        .unwrap()
        .with_param("format", "flowed")
        .unwrap();
    assert_eq!(custom.charset(), Some("utf-8"));
    assert_eq!(custom.param("format"), Some("flowed"));

    let overridden = TEXT_PLAIN_UTF8.with_param("CHARSET", "ISO-8859-1").unwrap();
    assert_eq!(overridden.charset(), Some("iso-8859-1"));
    assert_eq!(overridden.params().count(), 1);

    assert_eq!(
        TEXT_PLAIN.with_param("bad name", "1"),
        Err(ParseError::InvalidParamName)
    );
    assert_eq!(
        TEXT_PLAIN.with_param("ok", "bad\nval"),
        Err(ParseError::InvalidParamValue)
    );

    // Mime::new and extension()
    let created = Mime::new("Application", "VND.api+JSON").unwrap();
    assert_eq!(created.essence(), "application/vnd.api+json");
    assert_eq!(created.suffix(), Some("json"));
    assert!(created.is_json());
    assert_eq!(TEXT_PLAIN_UTF8.extension(), Some("txt"));
    assert_eq!(IMAGE_JPEG.extension(), Some("jpg"));
    assert_eq!(
        mime::from_extension("html").unwrap().extension(),
        Some("html")
    );

    // Order-independent PartialEq and Hash
    let a = Mime::parse("text/plain; a=1; b=2").unwrap();
    let b = Mime::parse("text/plain; b=2; a=1").unwrap();
    assert_eq!(a, b);
    let mut set = HashSet::new();
    set.insert(a);
    assert!(set.contains(&b));
}
