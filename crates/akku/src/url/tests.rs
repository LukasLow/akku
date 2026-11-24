use std::borrow::Cow;
use std::collections::HashSet;
use std::net::{Ipv4Addr, Ipv6Addr};

use akku::url::{
    form_decode, form_encode, percent_decode, percent_decode_bytes, percent_encode,
    percent_encode_bytes, AsciiSet, FormUrlEncoded, Host, ParseError, PercentDecodeError,
    QueryBuilder, SetError, Url, COMPONENT, FORM, FRAGMENT, NON_ALPHANUMERIC, PATH, PATH_SEGMENT,
    QUERY, USERINFO,
};

#[test]
fn parse_full_http_and_userinfo_with_at() {
    let url = Url::parse("HTTP://user:p@ss@Example.COM:8080/a/b/../c?k=1&v=2#sec").unwrap();
    assert_eq!(url.scheme(), "http");
    assert_eq!(url.username(), "user");
    assert_eq!(url.password(), Some("p%40ss"));
    assert_eq!(url.host(), Some(&Host::Domain("example.com".into())));
    assert_eq!(url.hostname().as_deref(), Some("example.com"));
    assert_eq!(url.port(), Some(8080));
    assert_eq!(url.port_or_default(), Some(8080));
    assert_eq!(url.authority(), "user:p%40ss@example.com:8080");
    assert_eq!(url.path(), "/a/c");
    assert_eq!(url.query(), Some("k=1&v=2"));
    assert_eq!(url.fragment(), Some("sec"));
    assert!(!url.cannot_be_a_base());
    assert!(url.is_special());
    assert_eq!(
        url.to_string(),
        "http://user:p%40ss@example.com:8080/a/c?k=1&v=2#sec"
    );
    assert_eq!(Url::parse(&url.to_string()), Ok(url));
}

#[test]
fn parse_default_ports_and_empty_paths() {
    let cases = [
        ("http://example.com:80", 80, "http://example.com/"),
        ("https://example.com:443", 443, "https://example.com/"),
        ("ws://example.com:80/chat", 80, "ws://example.com/chat"),
        ("wss://example.com:443/chat", 443, "wss://example.com/chat"),
        ("ftp://example.com:21/files", 21, "ftp://example.com/files"),
        ("http://example.com:/", 80, "http://example.com/"),
    ];
    for (raw, default_port, expected) in cases {
        let u: Url = raw.parse().unwrap();
        assert_eq!(u.port(), None, "for {raw}");
        assert_eq!(u.port_or_default(), Some(default_port), "for {raw}");
        assert_eq!(u.to_string(), expected, "for {raw}");
        assert_eq!(Url::parse(&u.to_string()), Ok(u));
    }

    let custom = Url::parse("custom://Example.COM:9000").unwrap();
    assert!(!custom.is_special());
    assert_eq!(custom.port(), Some(9000));
    assert_eq!(custom.port_or_default(), Some(9000));
    assert_eq!(custom.path(), "");
    assert_eq!(custom.to_string(), "custom://example.com:9000");
    assert_eq!(Url::parse(&custom.to_string()), Ok(custom));
}

#[test]
fn parse_ipv4_and_ipv6_hosts() {
    let v4 = Url::parse("http://127.0.0.1:3000/api").unwrap();
    assert_eq!(v4.host(), Some(&Host::Ipv4(Ipv4Addr::new(127, 0, 0, 1))));
    assert_eq!(v4.hostname().as_deref(), Some("127.0.0.1"));
    assert_eq!(v4.authority(), "127.0.0.1:3000");
    assert_eq!(Url::parse(&v4.to_string()), Ok(v4));

    let v6 = Url::parse("https://[2001:db8::1]:8443/v6").unwrap();
    let expected_v6: Ipv6Addr = "2001:db8::1".parse().unwrap();
    assert_eq!(v6.host(), Some(&Host::Ipv6(expected_v6)));
    assert_eq!(v6.hostname().as_deref(), Some("2001:db8::1"));
    assert_eq!(v6.authority(), "[2001:db8::1]:8443");
    assert_eq!(v6.to_string(), "https://[2001:db8::1]:8443/v6");
    assert_eq!(Url::parse(&v6.to_string()), Ok(v6));
}

#[test]
fn parse_file_urls() {
    let f = Url::parse("file:///etc/passwd").unwrap();
    assert_eq!(f.scheme(), "file");
    assert_eq!(f.host(), None);
    assert_eq!(f.hostname(), None);
    assert_eq!(f.authority(), "");
    assert_eq!(f.path(), "/etc/passwd");
    assert!(f.is_special());
    assert!(!f.cannot_be_a_base());
    assert_eq!(f.to_string(), "file:///etc/passwd");
    assert_eq!(Url::parse(&f.to_string()), Ok(f.clone()));

    let joined = f.join("../hosts").unwrap();
    assert_eq!(joined.to_string(), "file:///hosts");

    let f_local = Url::parse("file://localhost/tmp/test.txt").unwrap();
    assert_eq!(f_local.host(), Some(&Host::Domain("localhost".into())));
    assert_eq!(f_local.to_string(), "file://localhost/tmp/test.txt");
}

#[test]
fn parse_opaque_schemes_and_join_rules() {
    let mailto = Url::parse("MAILTO:user@example.com?subject=hi").unwrap();
    assert_eq!(mailto.scheme(), "mailto");
    assert!(mailto.cannot_be_a_base());
    assert!(!mailto.is_special());
    assert_eq!(mailto.host(), None);
    assert_eq!(mailto.path(), "user@example.com");
    assert_eq!(mailto.query(), Some("subject=hi"));
    assert!(mailto.segments().is_none());
    assert_eq!(mailto.to_string(), "mailto:user@example.com?subject=hi");
    assert_eq!(Url::parse(&mailto.to_string()), Ok(mailto.clone()));

    assert_eq!(
        mailto.join("#frag").unwrap().to_string(),
        "mailto:user@example.com?subject=hi#frag"
    );
    assert_eq!(
        mailto.join("https://example.com").unwrap().to_string(),
        "https://example.com/"
    );
    assert_eq!(mailto.join("other"), Err(ParseError::CannotBeABase));
    assert_eq!(mailto.join("/other"), Err(ParseError::CannotBeABase));
    assert_eq!(mailto.join("?q=1"), Err(ParseError::CannotBeABase));
    assert_eq!(mailto.join(""), Err(ParseError::CannotBeABase));

    let data = Url::parse("data:text/plain;base64,SGVsbG8=").unwrap();
    assert!(data.cannot_be_a_base());
    assert_eq!(data.path(), "text/plain;base64,SGVsbG8=");

    let js = Url::parse("javascript:void(0)").unwrap();
    assert!(js.cannot_be_a_base());
    assert_eq!(js.path(), "void(0)");
}

#[test]
fn segments_and_double_ended_iterator() {
    let u = Url::parse("https://example.com/one/two/three/").unwrap();
    let fwd: Vec<&str> = u.segments().unwrap().collect();
    let rev: Vec<&str> = u.segments().unwrap().rev().collect();
    assert_eq!(fwd, vec!["one", "two", "three", ""]);
    assert_eq!(rev, vec!["", "three", "two", "one"]);

    let mut segs = u.segments().unwrap();
    assert_eq!(segs.next(), Some("one"));
    assert_eq!(segs.next_back(), Some(""));
    assert_eq!(segs.next(), Some("two"));
    assert_eq!(segs.next_back(), Some("three"));
    assert_eq!(segs.next(), None);
    assert_eq!(segs.next_back(), None);

    let root = Url::parse("https://example.com").unwrap();
    assert_eq!(root.segments().unwrap().collect::<Vec<_>>(), vec![""]);

    let empty_custom = Url::parse("custom://host").unwrap();
    assert!(empty_custom.segments().unwrap().next().is_none());
}

#[test]
fn query_pairs_and_hash_eq() {
    let u1 = Url::parse("https://Example.com:443/search?q=rust+lang&page=2&empty=&flag").unwrap();
    let u2 = Url::parse("https://example.com/search?q=rust+lang&page=2&empty=&flag").unwrap();
    assert_eq!(u1, u2);
    let mut set = HashSet::new();
    set.insert(u1);
    assert!(set.contains(&u2));

    let pairs: Vec<(String, String)> = u2
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    assert_eq!(
        pairs,
        vec![
            ("q".into(), "rust lang".into()),
            ("page".into(), "2".into()),
            ("empty".into(), "".into()),
            ("flag".into(), "".into()),
        ]
    );
}

#[test]
fn rfc3986_section_5_4_join_matrix() {
    let base = Url::parse("http://a/b/c/d;p?q").unwrap();

    let normal_cases = [
        ("g:h", "g:h"),
        ("g", "http://a/b/c/g"),
        ("./g", "http://a/b/c/g"),
        ("g/", "http://a/b/c/g/"),
        ("/g", "http://a/g"),
        ("//g", "http://g/"),
        ("?y", "http://a/b/c/d;p?y"),
        ("g?y", "http://a/b/c/g?y"),
        ("#s", "http://a/b/c/d;p?q#s"),
        ("g#s", "http://a/b/c/g#s"),
        ("g?y#s", "http://a/b/c/g?y#s"),
        (";x", "http://a/b/c/;x"),
        ("g;x", "http://a/b/c/g;x"),
        ("g;x?y#s", "http://a/b/c/g;x?y#s"),
        ("", "http://a/b/c/d;p?q"),
        (".", "http://a/b/c/"),
        ("./", "http://a/b/c/"),
        ("..", "http://a/b/"),
        ("../", "http://a/b/"),
        ("../g", "http://a/b/g"),
        ("../..", "http://a/"),
        ("../../", "http://a/"),
        ("../../g", "http://a/g"),
    ];

    for (reference, expected) in normal_cases {
        let resolved = base.join(reference).unwrap();
        assert_eq!(resolved.to_string(), expected, "normal ref {reference:?}");
    }

    let abnormal_cases = [
        ("../../../g", "http://a/g"),
        ("../../../../g", "http://a/g"),
        ("/./g", "http://a/g"),
        ("/../g", "http://a/g"),
        ("g.", "http://a/b/c/g."),
        (".g", "http://a/b/c/.g"),
        ("g..", "http://a/b/c/g.."),
        ("..g", "http://a/b/c/..g"),
        ("./../g", "http://a/b/g"),
        ("./g/.", "http://a/b/c/g/"),
        ("g/./h", "http://a/b/c/g/h"),
        ("g/../h", "http://a/b/c/h"),
        ("g;x=1/./y", "http://a/b/c/g;x=1/y"),
        ("g;x=1/../y", "http://a/b/c/y"),
        ("g?y/./x", "http://a/b/c/g?y/./x"),
        ("g?y/../x", "http://a/b/c/g?y/../x"),
        ("g#s/./x", "http://a/b/c/g#s/./x"),
        ("g#s/../x", "http://a/b/c/g#s/../x"),
    ];

    for (reference, expected) in abnormal_cases {
        let resolved = base.join(reference).unwrap();
        assert_eq!(resolved.to_string(), expected, "abnormal ref {reference:?}");
    }
}

#[test]
fn percent_encoding_and_decoding_all_256_bytes() {
    let all_bytes: Vec<u8> = (0u8..=255u8).collect();
    let encoded = percent_encode_bytes(&all_bytes, &NON_ALPHANUMERIC);
    let decoded = percent_decode_bytes(&encoded);
    assert_eq!(decoded.as_ref(), all_bytes.as_slice());

    // Ensure uppercase hex is used for encoded bytes.
    assert!(encoded.contains("%00"));
    assert!(encoded.contains("%2F"));
    assert!(encoded.contains("%FF"));
    assert!(!encoded.contains("%2f"));
    assert!(!encoded.contains("%ff"));

    // Cow::Borrowed when unchanged
    let plain = "abcXYZ019";
    assert!(matches!(
        percent_encode(plain, &NON_ALPHANUMERIC),
        Cow::Borrowed("abcXYZ019")
    ));
    assert!(matches!(
        percent_decode("hello+world"),
        Ok(Cow::Borrowed("hello+world"))
    ));
    assert!(matches!(
        percent_decode_bytes("hello+world%ZZ"),
        Cow::Borrowed(b"hello+world%ZZ")
    ));

    // Strict vs lenient decoding
    assert_eq!(percent_decode("a%20b+c").unwrap(), "a b+c");
    assert_eq!(
        percent_decode("abc%"),
        Err(PercentDecodeError::InvalidEscape { index: 3 })
    );
    assert_eq!(
        percent_decode("abc%1"),
        Err(PercentDecodeError::InvalidEscape { index: 3 })
    );
    assert_eq!(
        percent_decode("abc%1G"),
        Err(PercentDecodeError::InvalidEscape { index: 3 })
    );
    assert_eq!(
        percent_decode("ok%FFbad"),
        Err(PercentDecodeError::InvalidUtf8 { valid_up_to: 2 })
    );

    assert_eq!(
        percent_decode_bytes("100%off%20ok%2").as_ref(),
        b"100%off ok%2"
    );

    // AsciiSet operations and constants
    let custom = AsciiSet::new().add(b'a').add(b'b').remove(b'a').add(200);
    assert!(!custom.contains(b'a'));
    assert!(custom.contains(b'b'));
    assert!(!custom.contains(200));

    assert_eq!(
        percent_encode("a/b?c#d=1&2", &COMPONENT),
        "a%2Fb%3Fc%23d%3D1%262"
    );
    assert_eq!(percent_encode("a/b?c#d", &PATH), "a/b%3Fc%23d");
    assert_eq!(percent_encode("a/b%c", &PATH_SEGMENT), "a%2Fb%25c");
    assert_eq!(percent_encode("a b#c", &QUERY), "a%20b%23c");
    assert_eq!(percent_encode("a b<c>", &FRAGMENT), "a%20b%3Cc%3E");
    assert_eq!(percent_encode("u:p@h", &USERINFO), "u%3Ap%40h");
    assert_eq!(percent_encode("a b*~", &FORM), "a%20b*%7E");
}

#[test]
fn form_urlencoded_and_query_builder_round_trip() {
    let encoded = form_encode("hello world! 100% a+b=c&d~");
    assert_eq!(encoded, "hello+world%21+100%25+a%2Bb%3Dc%26d%7E");
    assert_eq!(form_decode(&encoded), "hello world! 100% a+b=c&d~");
    assert!(matches!(
        form_decode("plain_text"),
        Cow::Borrowed("plain_text")
    ));

    let qb = QueryBuilder::new()
        .pair("q", "rust batteries")
        .pair("symbols", "+&=%")
        .flag("debug")
        .extend([("k1", "v1"), ("k2", "v 2")]);
    assert!(!qb.is_empty());
    let qs = qb.finish();
    assert_eq!(
        qs,
        "q=rust+batteries&symbols=%2B%26%3D%25&debug&k1=v1&k2=v+2"
    );

    let parsed: Vec<(String, String)> = FormUrlEncoded::parse(&format!("&&{qs}&"))
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    assert_eq!(
        parsed,
        vec![
            ("q".into(), "rust batteries".into()),
            ("symbols".into(), "+&=%".into()),
            ("debug".into(), "".into()),
            ("k1".into(), "v1".into()),
            ("k2".into(), "v 2".into()),
        ]
    );

    let qb_percent = QueryBuilder::new()
        .pair("q", "hello world")
        .plus_for_space(false)
        .finish();
    assert_eq!(qb_percent, "q=hello%20world");

    let mut url = Url::parse("https://example.com/search?old=1").unwrap();
    QueryBuilder::new().pair("new", "a b").apply(&mut url);
    assert_eq!(url.query(), Some("new=a+b"));
    QueryBuilder::new().apply(&mut url);
    assert_eq!(url.query(), None);
}

#[test]
fn url_builder_and_setters() {
    let built = Url::builder("https")
        .username("u:1")
        .password("p@2")
        .host("::1")
        .port(8443)
        .path("/api")
        .push_segment("v1/items")
        .push_segment("100%pure")
        .query_pairs([("q", "hello world")])
        .fragment("top sec")
        .build()
        .unwrap();

    assert_eq!(
        built.to_string(),
        "https://u%3A1:p%402@[::1]:8443/api/v1%2Fitems/100%25pure?q=hello+world#top%20sec"
    );

    let built_bracket = Url::builder("http").host("[::1]").port(80).build().unwrap();
    assert_eq!(built_bracket.port(), None);
    assert_eq!(built_bracket.to_string(), "http://[::1]/");

    let mut u = Url::parse("https://example.com/a/b").unwrap();
    u.set_username("alice").unwrap();
    u.set_password(Some("s:e")).unwrap();
    u.set_host(Some("2001:db8::1")).unwrap();
    u.set_port(Some(443)).unwrap();
    assert_eq!(u.port(), None);
    u.set_port(Some(9443)).unwrap();
    u.set_path("/x/../y z");
    u.set_query(Some("a=1 b"));
    u.set_fragment(Some("sec 1"));
    assert_eq!(
        u.to_string(),
        "https://alice:s%3Ae@[2001:db8::1]:9443/y%20z?a=1%20b#sec%201"
    );

    u.set_query_pairs([("x", "1 2")]);
    assert_eq!(u.query(), Some("x=1+2"));
    u.set_query_pairs(std::iter::empty::<(&str, &str)>());
    assert_eq!(u.query(), None);
    u.set_password(None).unwrap();
    u.set_fragment(None);
    assert_eq!(u.to_string(), "https://alice@[2001:db8::1]:9443/y%20z");

    // Non-special host removal
    let mut custom = Url::parse("custom://host/a").unwrap();
    custom.set_host(None).unwrap();
    assert_eq!(custom.to_string(), "custom:/a");
    assert_eq!(custom.set_username("a"), Err(SetError::CannotHaveHost));
    assert_eq!(
        custom.set_password(Some("b")),
        Err(SetError::CannotHaveHost)
    );
    assert_eq!(custom.set_port(Some(80)), Err(SetError::CannotHaveHost));

    // Opaque URL setter constraints
    let mut mail = Url::parse("mailto:a@b.com").unwrap();
    assert_eq!(
        mail.set_host(Some("example.com")),
        Err(ParseError::CannotBeABase)
    );
    assert_eq!(mail.set_username("u"), Err(SetError::CannotHaveHost));
    mail.set_path("/ignored");
    assert_eq!(mail.path(), "a@b.com");
}

#[test]
fn parse_errors_all_variants() {
    assert_eq!(Url::parse(""), Err(ParseError::RelativeUrlWithoutBase));
    assert_eq!(
        Url::parse("/relative/path"),
        Err(ParseError::RelativeUrlWithoutBase)
    );
    assert_eq!(
        Url::parse("foo/bar:baz"),
        Err(ParseError::RelativeUrlWithoutBase)
    );
    assert_eq!(Url::parse("://example.com"), Err(ParseError::InvalidScheme));
    assert_eq!(
        Url::parse("1http://example.com"),
        Err(ParseError::InvalidScheme)
    );
    assert_eq!(
        Url::parse("ht tp://example.com"),
        Err(ParseError::InvalidScheme)
    );
    assert_eq!(Url::parse("http:///path"), Err(ParseError::EmptyHost));
    assert_eq!(Url::parse("http://user@:80/"), Err(ParseError::EmptyHost));
    assert_eq!(
        Url::parse("http://exa mple.com/"),
        Err(ParseError::InvalidHost)
    );
    assert_eq!(Url::parse("http://[::1/"), Err(ParseError::InvalidIpv6));
    assert_eq!(
        Url::parse("http://[not_ipv6]/"),
        Err(ParseError::InvalidIpv6)
    );
    assert_eq!(
        Url::parse("http://[::1]extra/"),
        Err(ParseError::InvalidIpv6)
    );
    assert_eq!(
        Url::parse("http://example.com:abc/"),
        Err(ParseError::InvalidPort)
    );
    assert_eq!(
        Url::parse("http://example.com:70000/"),
        Err(ParseError::InvalidPort)
    );
    assert_eq!(
        Url::parse("http://example.com:+80/"),
        Err(ParseError::InvalidPort)
    );
    assert_eq!(
        Url::parse("http://example.com/a\nb"),
        Err(ParseError::InvalidCharacter)
    );
    assert_eq!(
        Url::parse("http://example.com/\0"),
        Err(ParseError::InvalidCharacter)
    );

    let mailto = Url::parse("mailto:a@b.com").unwrap();
    assert_eq!(mailto.join("sub"), Err(ParseError::CannotBeABase));
}
