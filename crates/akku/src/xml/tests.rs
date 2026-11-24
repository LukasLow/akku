//! Integration tests for `akku::xml`.

use std::borrow::Cow;

use akku::xml::{
    from_bytes, parse, parse_with, to_string, to_string_pretty, DtdPolicy, Element, ErrorKind,
    Event, Node, ParseOptions, Reader, SerializeOptions, Writer,
};

#[test]
fn parse_minimal_element_and_empty_tag() {
    let a = parse("<root/>").unwrap();
    let b = parse("<root></root>").unwrap();
    assert_eq!(a, b);
    assert_eq!(a.root().name(), "root");
    assert_eq!(a.root().children().len(), 0);
}

#[test]
fn xml_declaration_variants() {
    let doc = parse("<?xml version=\"1.0\" encoding='UTF-8' standalone=\"yes\"?>\n<doc/>").unwrap();
    let decl = doc.declaration.as_ref().unwrap();
    assert_eq!(decl.version, "1.0");
    assert_eq!(decl.encoding.as_deref(), Some("UTF-8"));
    assert_eq!(decl.standalone, Some(true));

    assert!(matches!(
        parse("<?xml version=\"1.1\"?><doc/>").unwrap_err().kind(),
        ErrorKind::InvalidDecl
    ));
    assert!(matches!(
        parse(" <?xml version=\"1.0\"?><doc/>").unwrap_err().kind(),
        ErrorKind::InvalidPiTarget { .. }
    ));
    assert!(matches!(
        parse("<?xml standalone=\"no\" version=\"1.0\"?><doc/>")
            .unwrap_err()
            .kind(),
        ErrorKind::InvalidDecl
    ));
}

#[test]
fn pull_reader_event_sequence_and_zero_copy() {
    let input = "<?xml version=\"1.0\"?><!--note--><?app go?><root a=\"1\">hi<empty/><![CDATA[raw]]></root>";
    let mut reader = Reader::new(input);

    assert!(matches!(reader.next_event().unwrap(), Event::Decl(_)));
    assert!(matches!(
        reader.next_event().unwrap(),
        Event::Comment(Cow::Borrowed("note"))
    ));
    assert!(matches!(
        reader.next_event().unwrap(),
        Event::PI {
            target: "app",
            data: Some(Cow::Borrowed("go"))
        }
    ));
    match reader.next_event().unwrap() {
        Event::Start(tag) => {
            assert_eq!(tag.name(), "root");
            assert_eq!(tag.attr("a"), Some("1"));
            assert!(matches!(tag.attributes()[0].value, Cow::Borrowed("1")));
        }
        other => panic!("unexpected event: {other:?}"),
    }
    assert!(matches!(
        reader.next_event().unwrap(),
        Event::Text(Cow::Borrowed("hi"))
    ));
    assert!(matches!(reader.next_event().unwrap(), Event::Empty(t) if t.name() == "empty"));
    assert!(matches!(
        reader.next_event().unwrap(),
        Event::CData(Cow::Borrowed("raw"))
    ));
    assert_eq!(reader.next_event().unwrap(), Event::End("root"));
    assert_eq!(reader.next_event().unwrap(), Event::Eof);
    assert_eq!(reader.next_event().unwrap(), Event::Eof);
}

#[test]
fn predefined_entities_decoded_single_pass() {
    let doc =
        parse(r#"<root k="&amp;&lt;&gt;&quot;&apos;">&amp;lt; &lt;tag&gt; &quot;hi&apos;</root>"#)
            .unwrap();
    assert_eq!(doc.root().attr("k"), Some("&<>\"'"));
    assert_eq!(doc.root().text(), "&lt; <tag> \"hi'");
}

#[test]
fn numeric_character_references() {
    let doc = parse("<r a=\"&#65;&#x42;\">&#128512; &#x1F600;</r>").unwrap();
    assert_eq!(doc.root().attr("a"), Some("AB"));
    assert_eq!(doc.root().text(), "😀 😀");
}

#[test]
fn reject_invalid_char_refs() {
    for bad in [
        "<r>&#0;</r>",
        "<r>&#x0;</r>",
        "<r>&#xD800;</r>",
        "<r>&#xDFFF;</r>",
        "<r>&#x110000;</r>",
        "<r>&#xFFFE;</r>",
        "<r>&#x1;</r>",
        "<r>&#;</r>",
        "<r>&#x;</r>",
        "<r>&#xZZ;</r>",
    ] {
        let err = parse(bad).unwrap_err();
        assert!(
            matches!(
                err.kind(),
                ErrorKind::InvalidCharRef { .. } | ErrorKind::InvalidXmlChar { .. }
            ),
            "expected char ref error for {bad}, got {err:?}"
        );
    }
}

#[test]
fn reject_unknown_named_entity() {
    let err = parse("<r>&nbsp;</r>").unwrap_err();
    assert!(matches!(err.kind(), ErrorKind::UnknownEntity { name } if name == "nbsp"));

    let err_attr = parse("<r a='&custom;'/>").unwrap_err();
    assert!(matches!(err_attr.kind(), ErrorKind::UnknownEntity { name } if name == "custom"));
}

#[test]
fn reject_xxe_and_doctype_by_default() {
    let xxe = r#"<?xml version="1.0"?>
<!DOCTYPE foo [
  <!ENTITY xxe SYSTEM "file:///etc/passwd">
]>
<foo>&xxe;</foo>"#;
    let err = parse(xxe).unwrap_err();
    assert!(matches!(err.kind(), ErrorKind::DtdNotAllowed));
}

#[test]
fn reject_entity_bomb_billion_laughs() {
    let bomb = r#"<!DOCTYPE lolz [
  <!ENTITY lol "lol">
  <!ENTITY lol2 "&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;">
  <!ENTITY lol3 "&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;">
]>
<lolz>&lol3;</lolz>"#;
    let err = parse(bomb).unwrap_err();
    assert!(matches!(err.kind(), ErrorKind::DtdNotAllowed));
}

#[test]
fn dtd_ignore_simple_declaration_still_blocks_subset_and_external() {
    let opts = ParseOptions {
        dtd_policy: DtdPolicy::IgnoreSimpleDeclaration,
        ..ParseOptions::new()
    };
    let ok = parse_with("<!DOCTYPE root><root>ok</root>", opts).unwrap();
    assert_eq!(ok.root().text(), "ok");

    let err_subset = parse_with("<!DOCTYPE root [<!ENTITY a \"b\">]><root/>", opts).unwrap_err();
    assert!(matches!(err_subset.kind(), ErrorKind::DtdNotAllowed));

    let err_ext =
        parse_with("<!DOCTYPE root SYSTEM \"http://evil/x.dtd\"><root/>", opts).unwrap_err();
    assert!(matches!(
        err_ext.kind(),
        ErrorKind::ExternalEntityNotAllowed
    ));
}

#[test]
fn reject_mismatched_and_unclosed_tags() {
    let err = parse("<a>\n  <b></a></b>").unwrap_err();
    assert!(
        matches!(err.kind(), ErrorKind::MismatchedTag { expected, found } if expected == "b" && found == "a")
    );
    assert_eq!(err.position().map(|p| p.line), Some(2));

    let err_unclosed = parse("<a><b></a>").unwrap_err();
    assert!(matches!(
        err_unclosed.kind(),
        ErrorKind::MismatchedTag { .. }
    ));

    let err_eof = parse("<a><b>").unwrap_err();
    assert!(matches!(err_eof.kind(), ErrorKind::UnclosedTag { name } if name == "b"));

    let err_stray = parse("</a>").unwrap_err();
    assert!(matches!(
        err_stray.kind(),
        ErrorKind::UnexpectedEndTag { name } if name == "a"
    ));
}

#[test]
fn reject_duplicate_attributes() {
    let err = parse("<a x=\"1\" y=\"2\" x=\"3\"/>").unwrap_err();
    assert!(matches!(err.kind(), ErrorKind::DuplicateAttribute { name } if name == "x"));
}

#[test]
fn reject_duplicate_namespaced_attributes() {
    let err = parse(r#"<a xmlns:p="urn:x" xmlns:q="urn:x" p:k="1" q:k="2"/>"#).unwrap_err();
    assert!(matches!(
        err.kind(),
        ErrorKind::DuplicateNamespaceAttribute {
            namespace,
            local_name
        } if namespace == "urn:x" && local_name == "k"
    ));
}

#[test]
fn namespace_resolution_and_shadowing() {
    let doc = parse(
        r#"<feed xmlns="urn:default" xmlns:ex="urn:example" xml:lang="en">
             <ex:entry ex:id="7" plain="yes">
               <local xmlns="">no-ns</local>
             </ex:entry>
           </feed>"#,
    )
    .unwrap();

    let root = doc.root();
    assert_eq!(root.prefix(), None);
    assert_eq!(root.local_name(), "feed");
    assert_eq!(root.namespace_uri(), Some("urn:default"));
    assert_eq!(
        root.attr_ns("http://www.w3.org/XML/1998/namespace", "lang"),
        Some("en")
    );

    let entry = root.child_ns("urn:example", "entry").unwrap();
    assert_eq!(entry.prefix(), Some("ex"));
    assert_eq!(entry.local_name(), "entry");
    assert_eq!(entry.attr_ns("urn:example", "id"), Some("7"));
    // Unprefixed attributes never inherit default xmlns.
    assert_eq!(entry.attr("plain"), Some("yes"));
    assert_eq!(entry.attr_ns("urn:default", "plain"), None);

    let local = entry.child("local").unwrap();
    assert_eq!(local.namespace_uri(), None);
    assert_eq!(local.text(), "no-ns");
}

#[test]
fn reject_unbound_namespace_prefix() {
    let err = parse("<ex:root/>").unwrap_err();
    assert!(matches!(err.kind(), ErrorKind::InvalidNamespace { .. }));
}

#[test]
fn reject_invalid_xml_chars_in_input() {
    for bad in [
        "<r>\x00</r>",
        "<r>\x07</r>",
        "<r a=\"\x0B\"/>",
        "<r><!--\x0C--></r>",
        "<r><![CDATA[\u{FFFE}]]></r>",
    ] {
        let err = parse(bad).unwrap_err();
        assert!(matches!(err.kind(), ErrorKind::InvalidXmlChar { .. }));
    }
}

#[test]
fn crlf_and_cr_normalization() {
    let doc =
        parse("<r a=\"x&#xD;&#xA;y\">line1\r\nline2\rline3<![CDATA[c1\r\nc2\rc3]]></r>").unwrap();
    assert_eq!(doc.root().attr("a"), Some("x\r\ny"));
    assert_eq!(doc.root().text(), "line1\nline2\nline3c1\nc2\nc3");
}

#[test]
fn attribute_whitespace_normalization() {
    let doc = parse("<r a=\"a\tb\nc\r\nd&#10;e&#9;f\"/>").unwrap();
    assert_eq!(doc.root().attr("a"), Some("a b c d\ne\tf"));
}

#[test]
fn cdata_preserves_markup_and_rejects_unescaped_text_end() {
    let doc = parse("<r><![CDATA[<a & b>]]></r>").unwrap();
    assert_eq!(doc.root().text(), "<a & b>");
    assert!(matches!(doc.root().children()[0], Node::CData(_)));

    let err = parse("<r>bad]]>text</r>").unwrap_err();
    assert!(matches!(err.kind(), ErrorKind::UnescapedCdataEndInText));
}

#[test]
fn comment_and_pi_rules() {
    assert!(matches!(
        parse("<r><!-- a--b --></r>").unwrap_err().kind(),
        ErrorKind::InvalidComment
    ));
    assert!(matches!(
        parse("<r><!-- bad---></r>").unwrap_err().kind(),
        ErrorKind::InvalidComment
    ));
    assert!(matches!(
        parse("<r><?xml foo?></r>").unwrap_err().kind(),
        ErrorKind::InvalidPiTarget { .. }
    ));
    let doc = parse("<r><?target?><?app param=\"1\"?></r>").unwrap();
    assert_eq!(doc.root().children().len(), 2);
}

#[test]
fn reject_multiple_roots_and_missing_root() {
    assert!(matches!(
        parse("<!-- only comment -->").unwrap_err().kind(),
        ErrorKind::MissingRootElement
    ));
    assert!(matches!(
        parse("<a/><b/>").unwrap_err().kind(),
        ErrorKind::TrailingContentAfterRoot
    ));
    assert!(matches!(
        parse("hello<a/>").unwrap_err().kind(),
        ErrorKind::MissingRootElement
    ));
    assert!(matches!(
        parse("<a/>hello").unwrap_err().kind(),
        ErrorKind::TrailingContentAfterRoot
    ));
}

#[test]
fn utf8_bom_and_invalid_utf8() {
    let mut bom_xml = Vec::from(b"\xEF\xBB\xBF");
    bom_xml.extend_from_slice(b"<ok>bom</ok>");
    let doc = from_bytes(&bom_xml).unwrap();
    assert_eq!(doc.root().text(), "bom");

    let err = from_bytes(&[0xFF, 0xFE, b'<']).unwrap_err();
    assert!(matches!(err.kind(), ErrorKind::InvalidUtf8));
}

#[test]
fn max_depth_limit_enforced() {
    let depth = 200;
    let xml = format!("{}<leaf/>{}", "<a>".repeat(depth), "</a>".repeat(depth));
    let err = parse(&xml).unwrap_err();
    assert!(matches!(
        err.kind(),
        ErrorKind::MaxDepthExceeded { limit: 128 }
    ));
}

#[test]
fn deep_dom_drop_and_text_no_stack_overflow() {
    let depth = 20_000;
    let mut xml = String::with_capacity(depth * 7 + 4);
    for _ in 0..depth {
        xml.push_str("<a>");
    }
    xml.push_str("leaf");
    for _ in 0..depth {
        xml.push_str("</a>");
    }

    let opts = ParseOptions {
        max_depth: 30_000,
        max_nodes: 100_000,
        ..ParseOptions::new()
    };
    let doc = parse_with(&xml, opts).unwrap();
    assert_eq!(doc.root().text(), "leaf");
    let serialized = to_string(&doc).unwrap();
    assert_eq!(serialized.len(), xml.len());
    drop(doc);
}

#[test]
fn max_nodes_and_max_attributes_limits() {
    let opts_nodes = ParseOptions {
        max_nodes: 3,
        ..ParseOptions::new()
    };
    let err_nodes = parse_with("<r><a/><b/><c/></r>", opts_nodes).unwrap_err();
    assert!(matches!(
        err_nodes.kind(),
        ErrorKind::MaxNodesExceeded { limit: 3 }
    ));

    let opts_attrs = ParseOptions {
        max_attributes: 2,
        ..ParseOptions::new()
    };
    let err_attrs = parse_with("<r a='1' b='2' c='3'/>", opts_attrs).unwrap_err();
    assert!(matches!(
        err_attrs.kind(),
        ErrorKind::MaxAttributesExceeded { limit: 2 }
    ));
}

#[test]
fn round_trip_compact_and_pretty_preserves_mixed_content() {
    let src = concat!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>",
        "<!--top-->",
        "<root xmlns=\"urn:app\">",
        "<p>Hello <b>world</b> &amp; friends!</p>",
        "<items><item id=\"1\"/><item id=\"2\"><![CDATA[x<y>z]]></item></items>",
        "</root>"
    );
    let doc = parse(src).unwrap();
    let compact = to_string(&doc).unwrap();
    assert_eq!(parse(&compact).unwrap(), doc);

    let pretty = to_string_pretty(&doc).unwrap();
    assert!(pretty.contains("\n  <p>Hello <b>world</b> &amp; friends!</p>\n"));
    let reparsed_no_ws = parse_with(
        &pretty,
        ParseOptions {
            preserve_whitespace: false,
            ..ParseOptions::new()
        },
    )
    .unwrap();
    assert_eq!(
        reparsed_no_ws.root().child("p").unwrap().text(),
        "Hello world & friends!"
    );
}

#[test]
fn writer_cdata_split_and_attr_escaping() {
    let mut w = Writer::new(Vec::new());
    w.write_start("root", &[("attr", "a & b < \"c\"\n\r\t")])
        .unwrap();
    w.write_cdata("before]]>after").unwrap();
    w.write_end().unwrap();
    let out = String::from_utf8(w.finish().unwrap()).unwrap();
    assert_eq!(
        out,
        "<root attr=\"a &amp; b &lt; &quot;c&quot;&#xA;&#xD;&#x9;\"><![CDATA[before]]]]><![CDATA[>after]]></root>"
    );
    let doc = parse(&out).unwrap();
    assert_eq!(doc.root().attr("attr"), Some("a & b < \"c\"\n\r\t"));
    assert_eq!(doc.root().text(), "before]]>after");
}

#[test]
fn writer_rejects_invalid_comment_and_unclosed_finish() {
    let mut w = Writer::new(Vec::new());
    assert!(matches!(
        w.write_comment("bad--comment").unwrap_err().kind(),
        ErrorKind::InvalidComment
    ));
    w.write_start("open", &[]).unwrap();
    assert!(matches!(
        w.finish().unwrap_err().kind(),
        ErrorKind::UnclosedTag { name } if name == "open"
    ));

    let doc = akku::xml::Document::new(Element::new("empty"));
    let non_self_closing = akku::xml::to_string_with(
        &doc,
        &SerializeOptions {
            self_closing_empty: false,
            ..SerializeOptions::new()
        },
    )
    .unwrap();
    assert_eq!(non_self_closing, "<empty></empty>");
}

#[test]
fn dom_builder_and_query_ergonomics() {
    let mut root = Element::new("feed")
        .with_namespace("urn:feed")
        .with_attr("version", "2")
        .with_child(Element::new("item").with_attr("id", "1").with_text("one"))
        .with_child(Element::new("item").with_attr("id", "2").with_text("two"));

    assert_eq!(root.children_named("item").count(), 2);
    assert_eq!(root.child("item").unwrap().attr("id"), Some("1"));
    root.set_attr("version", "3");
    assert_eq!(root.attr("version"), Some("3"));
    assert_eq!(root.remove_attr("version").as_deref(), Some("3"));
    assert_eq!(root.attr("version"), None);
    assert_eq!(root.text(), "onetwo");
}

// ---------------------------------------------------------------------------
// B6b regression tests: writer injection, buffering, namespace complexity.
// ---------------------------------------------------------------------------

use akku::xml::XmlDecl;

fn decl(version: &str, encoding: Option<&str>) -> XmlDecl {
    XmlDecl {
        version: version.to_string(),
        encoding: encoding.map(str::to_string),
        standalone: None,
    }
}

#[test]
fn writer_decl_rejects_injection_and_invalid_values() {
    for bad_enc in ["UTF-8\"?><evil/>", "", "8bit", "UTF 8", "UTF-8'", "ü"] {
        let mut w = Writer::new(Vec::new());
        let err = w.write_decl(&decl("1.0", Some(bad_enc))).unwrap_err();
        assert!(matches!(err.kind(), ErrorKind::InvalidDecl), "{bad_enc}");
        assert!(w.into_inner().is_empty(), "nothing written for {bad_enc}");
    }
    for bad_ver in [
        "1.0\"?><evil/>",
        "",
        "1.",
        "2.0",
        "1.0a",
        "1",
        " 1.0",
        "1.1",
        "1.10",
    ] {
        let mut w = Writer::new(Vec::new());
        assert!(matches!(
            w.write_decl(&decl(bad_ver, None)).unwrap_err().kind(),
            ErrorKind::InvalidDecl
        ));
        assert!(w.into_inner().is_empty());
    }

    // DOM path is protected too.
    let mut doc = parse("<a/>").unwrap();
    doc.declaration = Some(decl("1.0", Some("x\"?><evil/>")));
    assert!(matches!(
        to_string(&doc).unwrap_err().kind(),
        ErrorKind::InvalidDecl
    ));

    let mut w = Writer::new(Vec::new());
    w.write_decl(&XmlDecl {
        version: "1.0".into(),
        encoding: Some("ISO-8859-1".into()),
        standalone: Some(false),
    })
    .unwrap();
    assert_eq!(
        String::from_utf8(w.into_inner()).unwrap(),
        "<?xml version=\"1.0\" encoding=\"ISO-8859-1\" standalone=\"no\"?>"
    );
    // Writer and reader agree: a written declaration always parses back.
    let mut doc = parse("<a/>").unwrap();
    doc.declaration = Some(decl("1.1", None));
    assert!(matches!(
        to_string(&doc).unwrap_err().kind(),
        ErrorKind::InvalidDecl
    ));
}

#[test]
fn writer_other_paths_cannot_break_out() {
    let mut w = Writer::new(Vec::new());
    assert!(w.write_pi("t", Some("a?><evil/>")).is_err());
    assert!(w.write_pi("xMl", None).is_err());
    assert!(w.write_pi("a b", None).is_err());
    assert!(w.write_pi("t", Some(" leading")).is_err());
    assert!(w.write_comment("a--b").is_err());
    assert!(w.write_comment("trail-").is_err());
    assert!(w.write_start("a\"b", &[]).is_err());
    assert!(w.write_start("a", &[("x y", "1")]).is_err());
    assert!(w.write_start("a", &[("x", "1"), ("x", "2")]).is_err());
    assert!(w.write_text("\u{0}").is_err());
    assert!(
        w.into_inner().is_empty(),
        "failed calls must not emit output"
    );

    // Many attributes -> hashed duplicate path.
    let names: Vec<String> = (0..40).map(|i| format!("a{i}")).collect();
    let mut attrs: Vec<(&str, &str)> = names.iter().map(|n| (n.as_str(), "v")).collect();
    attrs.push(("a7", "dup"));
    let mut w = Writer::new(Vec::new());
    assert!(matches!(
        w.write_empty("e", &attrs).unwrap_err().kind(),
        ErrorKind::DuplicateAttribute { .. }
    ));

    // Round-trip of every special in attributes, text and CDATA.
    let mut w = Writer::new(Vec::new());
    let attr_val = "q\"<&>'\t\n\r end";
    let text_val = "t<&>]]>\r\n";
    let cdata_val = "x]]>y]]>]]>";
    w.write_start("r", &[("v", attr_val)]).unwrap();
    w.write_text(text_val).unwrap();
    w.write_cdata(cdata_val).unwrap();
    w.write_pi("p", Some("d?")).unwrap();
    w.write_end().unwrap();
    let out = String::from_utf8(w.finish().unwrap()).unwrap();
    let doc = parse(&out).unwrap();
    assert_eq!(doc.root().attr("v"), Some(attr_val));
    assert_eq!(doc.root().text(), format!("{text_val}{cdata_val}"));
    assert!(doc.root().children().iter().any(|n| matches!(
        n,
        Node::ProcessingInstruction { target, data } if target == "p" && data.as_deref() == Some("d?")
    )));
}

struct CountingWriter {
    data: Vec<u8>,
    writes: usize,
}

impl std::io::Write for CountingWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.writes += 1;
        self.data.extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn writer_batches_output_into_few_writes() {
    let text = "<&>\"".repeat(5_000);
    let mut root = Element::new("root").with_attr("a", text.clone());
    for _ in 0..100 {
        root.push_child(Element::new("c").with_text(text.clone()));
    }
    let doc = akku::xml::Document::new(root);
    let mut sink = CountingWriter {
        data: Vec::new(),
        writes: 0,
    };
    akku::xml::write_to(&doc, &mut sink).unwrap();
    let expected = to_string(&doc).unwrap();
    assert_eq!(sink.data, expected.as_bytes());
    // ~2 MiB of output; one write per ~8 KiB chunk at most, not one per char.
    let max_writes = expected.len() / 8192 + 105;
    assert!(
        sink.writes <= max_writes,
        "{} writes for {} bytes",
        sink.writes,
        expected.len()
    );

    // Streaming API: one write per call.
    let mut sink = CountingWriter {
        data: Vec::new(),
        writes: 0,
    };
    let mut w = Writer::new(&mut sink);
    w.write_start("a", &[("x", "&&&&&&")]).unwrap();
    w.write_text(&"&<".repeat(1000)).unwrap();
    w.write_end().unwrap();
    w.finish().unwrap();
    assert_eq!(sink.writes, 3);
}

#[test]
fn namespace_many_siblings_and_deep_prefixes_are_fast() {
    use std::fmt::Write as _;
    use std::time::Instant;

    // 50k siblings, each declaring a namespace.
    let mut xml = String::from("<root xmlns:r=\"urn:root\">");
    for i in 0..50_000 {
        let _ = write!(
            xml,
            "<p{i}:e xmlns:p{i}=\"urn:{i}\" p{i}:a=\"1\" r:b=\"2\"/>"
        );
    }
    xml.push_str("</root>");
    let start = Instant::now();
    let doc = parse(&xml).unwrap();
    assert_eq!(doc.root().children().len(), 50_000);
    let last = doc.root().child_elements().last().unwrap();
    assert_eq!(last.namespace_uri(), Some("urn:49999"));
    assert_eq!(last.attr_ns("urn:root", "b"), Some("2"));
    assert!(
        start.elapsed().as_secs() < 20,
        "siblings: {:?}",
        start.elapsed()
    );

    // 10k nested elements, 20 prefixes declared at the top and redeclared all the way down.
    let depth = 10_000;
    let mut xml = String::from("<d");
    for p in 0..20 {
        let _ = write!(xml, " xmlns:q{p}=\"urn:top{p}\"");
    }
    xml.push('>');
    for i in 0..depth {
        let _ = write!(
            xml,
            "<q{}:n xmlns:z{}=\"urn:z{i}\" q0:a=\"x\">",
            i % 20,
            i % 7
        );
    }
    for i in (0..depth).rev() {
        let _ = write!(xml, "</q{}:n>", i % 20);
    }
    xml.push_str("</d>");
    let options = ParseOptions {
        max_depth: depth + 10,
        ..ParseOptions::new()
    };
    let start = Instant::now();
    let doc = parse_with(&xml, options).unwrap();
    let mut el = doc.root();
    let mut n = 0;
    while let Some(child) = el.child_elements().next() {
        assert_eq!(
            child.namespace_uri(),
            Some(format!("urn:top{}", n % 20).as_str())
        );
        el = child;
        n += 1;
    }
    assert_eq!(n, depth);
    assert!(
        start.elapsed().as_secs() < 20,
        "nested: {:?}",
        start.elapsed()
    );
}

#[test]
fn namespace_shadowing_undeclaring_and_reserved_prefixes() {
    let doc = parse(concat!(
        r#"<a xmlns="urn:d" xmlns:p="urn:p1">"#,
        r#"<p:b xmlns:p="urn:p2"><p:c/></p:b>"#,
        r#"<p:b2/>"#,
        r#"<u xmlns=""><v/></u>"#,
        r#"<w/>"#,
        r#"<x xml:lang="en"/>"#,
        "</a>"
    ))
    .unwrap();
    let root = doc.root();
    assert_eq!(root.namespace_uri(), Some("urn:d"));
    let kids: Vec<_> = root.child_elements().collect();
    assert_eq!(kids[0].namespace_uri(), Some("urn:p2"));
    assert_eq!(
        kids[0].child_elements().next().unwrap().namespace_uri(),
        Some("urn:p2")
    );
    assert_eq!(kids[1].namespace_uri(), Some("urn:p1"), "shadow undone");
    assert_eq!(
        kids[2].namespace_uri(),
        None,
        "xmlns=\"\" undeclares default"
    );
    assert_eq!(
        kids[2].child_elements().next().unwrap().namespace_uri(),
        None
    );
    assert_eq!(kids[3].namespace_uri(), Some("urn:d"), "default restored");
    assert_eq!(
        kids[4].attr_ns("http://www.w3.org/XML/1998/namespace", "lang"),
        Some("en")
    );
    assert_eq!(
        root.attributes()[0].namespace_uri(),
        Some("http://www.w3.org/2000/xmlns/")
    );

    // Prefix bound only inside a closed sibling is out of scope afterwards.
    assert!(matches!(
        parse(r#"<a><b xmlns:p="urn:x"/><p:c/></a>"#)
            .unwrap_err()
            .kind(),
        ErrorKind::InvalidNamespace { .. }
    ));
    // Reserved prefixes.
    for bad in [
        r#"<a xmlns:xmlns="urn:x"/>"#,
        r#"<a xmlns:xml="urn:x"/>"#,
        r#"<a xmlns:p="http://www.w3.org/XML/1998/namespace"/>"#,
        r#"<a xmlns="http://www.w3.org/2000/xmlns/"/>"#,
        r#"<xmlns:a/>"#,
        r#"<a xmlns:p=""/>"#,
    ] {
        assert!(
            matches!(
                parse(bad).unwrap_err().kind(),
                ErrorKind::InvalidNamespace { .. }
            ),
            "{bad}"
        );
    }
    assert!(parse(r#"<a xmlns:xml="http://www.w3.org/XML/1998/namespace"/>"#).is_ok());
    // Duplicate expanded names via two prefixes for the same URI.
    assert!(matches!(
        parse(r#"<a xmlns:p="urn:x" xmlns:q="urn:x" p:k="1" q:k="2"/>"#)
            .unwrap_err()
            .kind(),
        ErrorKind::DuplicateNamespaceAttribute { .. }
    ));
}

#[test]
fn many_attributes_duplicate_detection() {
    use std::fmt::Write as _;
    let mut xml = String::from("<a");
    for i in 0..200 {
        let _ = write!(xml, " k{i}=\"v\"");
    }
    xml.push_str("/>");
    assert_eq!(parse(&xml).unwrap().root().attributes().len(), 200);
    let dup = xml.replace("/>", " k150=\"x\"/>");
    assert!(matches!(
        parse(&dup).unwrap_err().kind(),
        ErrorKind::DuplicateAttribute { .. }
    ));
}

#[test]
fn non_xml_whitespace_text_is_not_dropped() {
    let options = ParseOptions {
        preserve_whitespace: false,
        ..ParseOptions::new()
    };
    let doc = parse_with("<a>\u{A0}</a>", options).unwrap();
    assert_eq!(doc.root().text(), "\u{A0}");
}

#[test]
fn writer_enforces_namespace_well_formedness() {
    let bad: &[(&str, &[(&str, &str)])] = &[
        ("a:b:c", &[]),
        ("p:x", &[]),
        (":x", &[]),
        ("x:", &[]),
        ("xmlns:a", &[("xmlns:a", "urn:a")]),
        ("a", &[("xmlns:p", "")]),
        ("a", &[("xmlns:xmlns", "urn:x")]),
        ("a", &[("xmlns:xml", "urn:x")]),
        ("a", &[("xmlns:p", "http://www.w3.org/XML/1998/namespace")]),
        ("a", &[("xmlns", "http://www.w3.org/2000/xmlns/")]),
        ("a", &[("q:k", "1")]),
        ("a", &[("k:l:m", "1")]),
        (
            "a",
            &[
                ("xmlns:p", "urn:x"),
                ("xmlns:q", "urn:x"),
                ("p:k", "1"),
                ("q:k", "2"),
            ],
        ),
    ];
    for (name, attrs) in bad {
        let mut w = Writer::new(Vec::new());
        assert!(w.write_empty(name, attrs).is_err(), "{name} {attrs:?}");
        assert!(w.write_start(name, attrs).is_err(), "{name} {attrs:?}");
        assert!(w.into_inner().is_empty(), "{name}");
        // Each of these is also rejected by the reader.
        let mut xml = format!("<{name}");
        for (k, v) in attrs.iter() {
            xml.push_str(&format!(" {k}=\"{v}\""));
        }
        xml.push_str("/>");
        assert!(parse(&xml).is_err(), "{xml}");
    }

    // Scoping: bound inside, out of scope after the element closes.
    let mut w = Writer::new(Vec::new());
    w.write_start("r", &[("xmlns:p", "urn:p")]).unwrap();
    w.write_empty("p:x", &[("p:a", "1"), ("xml:lang", "en")])
        .unwrap();
    w.write_empty("q:y", &[("xmlns:q", "urn:q")]).unwrap();
    assert!(w.write_empty("q:y", &[]).is_err(), "q out of scope");
    w.write_end().unwrap();
    assert!(w.write_empty("p:x", &[]).is_err(), "p out of scope");
    let out = String::from_utf8(w.finish().unwrap()).unwrap();
    assert!(parse(&out).is_ok(), "{out}");

    // DOM path, and failed calls leave no binding behind.
    let doc = akku::xml::Document::new(Element::new("a").with_child(Element::new("p:b")));
    assert!(matches!(
        to_string(&doc).unwrap_err().kind(),
        ErrorKind::InvalidNamespace { .. }
    ));
    let mut w = Writer::new(Vec::new());
    assert!(w
        .write_start("a", &[("xmlns:p", "urn:p"), ("x", "\u{0}")])
        .is_err());
    assert!(w.write_empty("p:b", &[]).is_err(), "binding rolled back");

    // Opt-out mirrors ParseOptions::check_namespace_well_formed.
    let lax = SerializeOptions {
        check_namespace_well_formed: false,
        ..SerializeOptions::new()
    };
    let mut w = Writer::with_options(Vec::new(), lax);
    w.write_empty("p:x", &[]).unwrap();
    assert_eq!(w.finish().unwrap(), b"<p:x/>");
}

#[test]
fn writer_cr_in_cdata_comment_and_pi() {
    let mut w = Writer::new(Vec::new());
    w.write_start("r", &[]).unwrap();
    w.write_cdata("a\rb]]>c\r\n\r").unwrap();
    w.write_end().unwrap();
    let out = String::from_utf8(w.finish().unwrap()).unwrap();
    assert!(!out.contains('\r'), "{out:?}");
    assert_eq!(parse(&out).unwrap().root().text(), "a\rb]]>c\r\n\r");

    let mut w = Writer::new(Vec::new());
    w.write_start("r", &[]).unwrap();
    w.write_cdata("").unwrap();
    w.write_end().unwrap();
    assert_eq!(w.finish().unwrap(), b"<r><![CDATA[]]></r>");

    let mut w = Writer::new(Vec::new());
    assert!(w.write_comment("a\rb").is_err());
    assert!(w.write_pi("t", Some("a\rb")).is_err());
    assert!(w.into_inner().is_empty());
}

#[derive(Debug)]
struct FailingWriter {
    data: Vec<u8>,
    fail: std::rc::Rc<std::cell::Cell<bool>>,
}

impl std::io::Write for FailingWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.fail.get() {
            return Err(std::io::Error::other("sink down"));
        }
        self.data.extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn failing_writer() -> (FailingWriter, std::rc::Rc<std::cell::Cell<bool>>) {
    let fail = std::rc::Rc::new(std::cell::Cell::new(false));
    (
        FailingWriter {
            data: Vec::new(),
            fail: fail.clone(),
        },
        fail,
    )
}

#[test]
fn writer_end_keeps_tag_open_on_io_failure() {
    let (sink, fail) = failing_writer();
    let mut w = Writer::new(sink);
    w.write_start("a", &[("xmlns:p", "urn:p")]).unwrap();
    fail.set(true);
    assert!(matches!(
        w.write_end().unwrap_err().kind(),
        ErrorKind::Io(_)
    ));
    fail.set(false);
    // Tag and its namespace scope survived the failed write.
    w.write_empty("p:b", &[]).unwrap();
    w.write_end().unwrap();
    let sink = w.finish().unwrap();
    assert_eq!(sink.data, b"<a xmlns:p=\"urn:p\"><p:b/></a>");

    // A failed write_end leaves the element open, so finish() reports it.
    let (sink, fail) = failing_writer();
    let mut w = Writer::new(sink);
    w.write_start("a", &[]).unwrap();
    fail.set(true);
    assert!(w.write_end().is_err());
    assert!(matches!(
        w.finish().unwrap_err().kind(),
        ErrorKind::UnclosedTag { name } if name == "a"
    ));

    // A failed write_start does not leave a phantom open tag or binding.
    let (sink, fail) = failing_writer();
    let mut w = Writer::new(sink);
    fail.set(true);
    assert!(w.write_start("a", &[("xmlns:p", "urn:p")]).is_err());
    fail.set(false);
    assert!(w.write_empty("p:x", &[]).is_err());
    assert!(w.finish().is_ok());
}

#[test]
fn writer_document_validates_whole_tree_before_writing() {
    let big = "x".repeat(64 * 1024);
    let mut root = Element::new("root");
    for _ in 0..8 {
        root.push_child(Element::new("c").with_text(big.clone()));
    }
    // Invalid node at the very end, after > 512 KiB of valid output.
    root.push_child(Node::Comment("bad--comment".into()));
    let doc = akku::xml::Document::new(root);
    let mut sink = CountingWriter {
        data: Vec::new(),
        writes: 0,
    };
    assert!(matches!(
        akku::xml::write_to(&doc, &mut sink).unwrap_err().kind(),
        ErrorKind::InvalidComment
    ));
    assert_eq!(sink.writes, 0, "nothing written on validation failure");
    assert!(sink.data.is_empty());

    // Namespace error deep in the tree is caught by the validation pass as well.
    let mut root = Element::new("root").with_text(big.clone());
    root.push_child(Element::new("p:x"));
    let doc = akku::xml::Document::new(root);
    let mut w = Writer::new(Vec::new());
    assert!(w.write_document(&doc).is_err());
    assert!(w.into_inner().is_empty());

    // Valid documents still stream in chunks and round-trip.
    let mut root = Element::new("root").with_attr("xmlns:p", "urn:p");
    for _ in 0..8 {
        root.push_child(Element::new("p:c").with_text(big.clone()));
    }
    let doc = akku::xml::Document::new(root);
    let mut sink = CountingWriter {
        data: Vec::new(),
        writes: 0,
    };
    akku::xml::write_to(&doc, &mut sink).unwrap();
    assert!(sink.writes > 1);
    assert_eq!(sink.data, to_string(&doc).unwrap().as_bytes());
}

#[test]
fn writer_streams_huge_text_in_bounded_chunks() {
    let text = "a<&\r\u{e9}".repeat(200_000);
    let mut sink = CountingWriter {
        data: Vec::new(),
        writes: 0,
    };
    let mut w = Writer::new(&mut sink);
    w.write_start("r", &[]).unwrap();
    w.write_text(&text).unwrap();
    w.write_end().unwrap();
    w.finish().unwrap();
    assert!(sink.writes > 10, "streamed in chunks: {}", sink.writes);
    let out = String::from_utf8(sink.data).unwrap();
    assert_eq!(parse(&out).unwrap().root().text(), text);

    // Invalid char at the very end: validated first, nothing written.
    let mut bad = "z".repeat(100_000);
    bad.push('\u{1}');
    let mut sink = CountingWriter {
        data: Vec::new(),
        writes: 0,
    };
    let mut w = Writer::new(&mut sink);
    assert!(matches!(
        w.write_text(&bad).unwrap_err().kind(),
        ErrorKind::InvalidXmlChar { codepoint: 1 }
    ));
    drop(w);
    assert_eq!(sink.writes, 0);
}

#[test]
fn reader_is_poisoned_and_consistent_after_error() {
    for xml in [
        r#"<a xmlns:p="urn:p" x="1" x="2"/>"#,
        r#"<a xmlns:p="urn:p" xmlns:q=""/>"#,
        r#"<r><a xmlns:p="urn:p" k="1" k="1"><p:b/></a></r>"#,
        "<a><b></c></a>",
    ] {
        let mut reader = Reader::new(xml);
        let err = loop {
            match reader.next_event() {
                Ok(Event::Eof) => panic!("{xml} should fail"),
                Ok(_) => {}
                Err(e) => break e,
            }
        };
        assert!(!matches!(err.kind(), ErrorKind::ReaderPoisoned), "{xml}");
        for _ in 0..3 {
            assert!(
                matches!(
                    reader.next_event().unwrap_err().kind(),
                    ErrorKind::ReaderPoisoned
                ),
                "{xml}"
            );
        }
    }

    // Too many attributes after an xmlns binding.
    let options = ParseOptions {
        max_attributes: 2,
        ..ParseOptions::new()
    };
    let mut reader = Reader::with_options(r#"<a xmlns:p="urn:p" b="1" c="2"/>"#, options);
    assert!(matches!(
        reader.next_event().unwrap_err().kind(),
        ErrorKind::MaxAttributesExceeded { .. }
    ));
    assert_eq!(reader.depth(), 0);
    assert!(reader.next_event().is_err());

    // Mismatched end tag keeps the element open (depth unchanged).
    let mut reader = Reader::new("<a><b></c></b></a>");
    reader.next_event().unwrap();
    reader.next_event().unwrap();
    assert_eq!(reader.depth(), 2);
    assert!(matches!(
        reader.next_event().unwrap_err().kind(),
        ErrorKind::MismatchedTag { .. }
    ));
    assert_eq!(reader.depth(), 2);
}
