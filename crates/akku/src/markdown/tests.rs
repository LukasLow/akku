//! Integration tests for `akku::markdown`.

use std::thread;
use std::time::{Duration, Instant};

use akku::markdown::{
    is_safe_url, parse, parse_with, render_html, to_html, to_html_with, Alignment, Block, Document,
    Inline, List, ListItem, ListKind, Options,
};

// -- helpers -------------------------------------------------------------------

/// Run `f` on a thread with a deliberately small stack, so that a missing
/// depth limit shows up as a stack overflow instead of passing by luck.
fn on_small_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    on_stack(256, f)
}

fn on_stack<T: Send + 'static>(kibibytes: usize, f: impl FnOnce() -> T + Send + 'static) -> T {
    thread::Builder::new()
        .stack_size(kibibytes * 1024)
        .spawn(f)
        .expect("spawn")
        .join()
        .expect("the thread must not overflow its stack or panic")
}

fn count(haystack: &str, needle: &str) -> usize {
    haystack.matches(needle).count()
}

/// The deepest nesting of `<tag>` … `</tag>` in `html`.
fn max_depth(html: &str, tag: &str) -> usize {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let (mut depth, mut deepest, mut i) = (0usize, 0usize, 0usize);
    while i < html.len() {
        if html[i..].starts_with(&open) {
            depth += 1;
            deepest = deepest.max(depth);
            i += open.len();
        } else if html[i..].starts_with(&close) {
            depth = depth.saturating_sub(1);
            i += close.len();
        } else {
            i += 1;
        }
    }
    deepest
}

/// Convert in a small-stack thread and fail when it takes unreasonably long.
fn bounded_html(input: String) -> String {
    let started = Instant::now();
    let html = on_small_stack(move || to_html(&input));
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "conversion took {:?}",
        started.elapsed()
    );
    html
}

// -- CommonMark ------------------------------------------------------------------

#[test]
fn headings_atx_and_setext() {
    assert_eq!(
        to_html("# A\n\nB\n===\n\n### C ###"),
        "<h1>A</h1>\n<h1>B</h1>\n<h3>C</h3>\n"
    );
    assert_eq!(to_html("Sub\n---"), "<h2>Sub</h2>\n");
    assert_eq!(to_html("####### x"), "<p>####### x</p>\n");
    assert_eq!(to_html("#5 bolt"), "<p>#5 bolt</p>\n");
    assert_eq!(to_html("#"), "<h1></h1>\n");
    assert_eq!(to_html("## foo ##   "), "<h2>foo</h2>\n");
}

#[test]
fn paragraphs_and_line_breaks() {
    assert_eq!(to_html("a\nb  \nc\\\nd"), "<p>a\nb<br />\nc<br />\nd</p>\n");
    assert_eq!(to_html("one\n\ntwo"), "<p>one</p>\n<p>two</p>\n");
    assert_eq!(to_html("a  "), "<p>a</p>\n");
}

#[test]
fn emphasis_and_strong() {
    assert_eq!(
        to_html("*a* **b** _c_ __d__ ***e***"),
        "<p><em>a</em> <strong>b</strong> <em>c</em> <strong>d</strong> \
         <em><strong>e</strong></em></p>\n"
    );
    assert_eq!(
        to_html("*(**foo**)*"),
        "<p><em>(<strong>foo</strong>)</em></p>\n"
    );
    assert_eq!(to_html("foo*bar*"), "<p>foo<em>bar</em></p>\n");
}

#[test]
fn emphasis_flanking_edge_cases() {
    assert_eq!(to_html("snake_case_word"), "<p>snake_case_word</p>\n");
    assert_eq!(to_html("2 * 3 * 4"), "<p>2 * 3 * 4</p>\n");
    assert_eq!(to_html("**foo bar **"), "<p>**foo bar **</p>\n");
    assert_eq!(to_html("foo_bar_"), "<p>foo_bar_</p>\n");
    assert_eq!(
        to_html("*foo**bar**baz*"),
        "<p><em>foo<strong>bar</strong>baz</em></p>\n"
    );
}

#[test]
fn code_spans() {
    assert_eq!(to_html("`` `a` ``"), "<p><code>`a`</code></p>\n");
    assert_eq!(to_html("`  a  `"), "<p><code> a </code></p>\n");
    assert_eq!(to_html("`a\nb`"), "<p><code>a b</code></p>\n");
    assert_eq!(to_html("``foo`"), "<p>``foo`</p>\n");
    assert_eq!(to_html("`<b>&`"), "<p><code>&lt;b&gt;&amp;</code></p>\n");
}

#[test]
fn fenced_code_blocks() {
    assert_eq!(
        to_html("```rust\nlet a = 1 < 2;\n```"),
        "<pre><code class=\"language-rust\">let a = 1 &lt; 2;\n</code></pre>\n"
    );
    assert_eq!(
        to_html("```\na\nb"),
        "<pre><code>a\nb\n</code></pre>\n",
        "an unclosed fence runs to the end of the document"
    );
    assert_eq!(
        to_html("~~~ruby extra\nx\n~~~"),
        "<pre><code class=\"language-ruby\">x\n</code></pre>\n"
    );
    assert_eq!(to_html("```\n```"), "<pre><code></code></pre>\n");
    assert_eq!(
        to_html("  ```\n  a\n   b\n  ```"),
        "<pre><code>a\n b\n</code></pre>\n"
    );
}

#[test]
fn indented_code_blocks() {
    assert_eq!(to_html("    a\n    b"), "<pre><code>a\nb\n</code></pre>\n");
    assert_eq!(to_html("\tcode"), "<pre><code>code\n</code></pre>\n");
    assert_eq!(
        to_html("    a\n\n    b\n\n"),
        "<pre><code>a\n\nb\n</code></pre>\n"
    );
    assert_eq!(
        to_html("a\n    b"),
        "<p>a\nb</p>\n",
        "cannot interrupt a paragraph"
    );
}

#[test]
fn thematic_breaks() {
    assert_eq!(to_html("---\n***\n___"), "<hr />\n<hr />\n<hr />\n");
    assert_eq!(to_html("- - -"), "<hr />\n");
    assert_eq!(to_html("--"), "<p>--</p>\n");
}

#[test]
fn block_quotes_and_lazy_continuation() {
    assert_eq!(
        to_html("> a\nb"),
        "<blockquote>\n<p>a\nb</p>\n</blockquote>\n"
    );
    assert_eq!(
        to_html("> a\n\n> b"),
        "<blockquote>\n<p>a</p>\n</blockquote>\n<blockquote>\n<p>b</p>\n</blockquote>\n"
    );
    assert_eq!(
        to_html("> # H\n> text"),
        "<blockquote>\n<h1>H</h1>\n<p>text</p>\n</blockquote>\n"
    );
}

#[test]
fn bullet_lists_tight_and_loose() {
    assert_eq!(to_html("- a\n- b"), "<ul>\n<li>a</li>\n<li>b</li>\n</ul>\n");
    assert_eq!(
        to_html("- a\n\n- b"),
        "<ul>\n<li>\n<p>a</p>\n</li>\n<li>\n<p>b</p>\n</li>\n</ul>\n"
    );
    assert_eq!(
        to_html("* a\n+ b"),
        "<ul>\n<li>a</li>\n</ul>\n<ul>\n<li>b</li>\n</ul>\n",
        "a different bullet starts a new list"
    );
}

#[test]
fn ordered_lists_keep_their_start() {
    assert_eq!(
        to_html("3. a\n4. b"),
        "<ol start=\"3\">\n<li>a</li>\n<li>b</li>\n</ol>\n"
    );
    assert_eq!(to_html("1. a"), "<ol>\n<li>a</li>\n</ol>\n");
    assert_eq!(
        to_html("para\n2. not a list"),
        "<p>para\n2. not a list</p>\n",
        "only 1. may interrupt a paragraph"
    );
}

#[test]
fn nested_lists_and_item_content() {
    assert_eq!(
        to_html("- a\n  - b"),
        "<ul>\n<li>a\n<ul>\n<li>b</li>\n</ul>\n</li>\n</ul>\n"
    );
    assert_eq!(
        to_html("- a\n\n      code"),
        "<ul>\n<li>\n<p>a</p>\n<pre><code>code\n</code></pre>\n</li>\n</ul>\n"
    );
    assert_eq!(
        to_html("1. # h\n   text"),
        "<ol>\n<li>\n<h1>h</h1>\ntext</li>\n</ol>\n",
        "a tight list item does not wrap its paragraph"
    );
}

#[test]
fn links_inline_reference_and_title() {
    assert_eq!(
        to_html("[a](/u \"t\") [b][r] [c]\n\n[r]: /v\n[c]: <w x> 'ct'"),
        "<p><a href=\"/u\" title=\"t\">a</a> <a href=\"/v\">b</a> \
         <a href=\"w%20x\" title=\"ct\">c</a></p>\n"
    );
    assert_eq!(to_html("[a]()"), "<p><a href=\"\">a</a></p>\n");
    assert_eq!(
        to_html("[a [b](/b)](/a)"),
        "<p>[a <a href=\"/b\">b</a>](/a)</p>\n",
        "links cannot contain links"
    );
}

#[test]
fn link_reference_definitions() {
    assert_eq!(
        to_html("[FOO]: /one\n[foo]: /two\n\n[Foo]"),
        "<p><a href=\"/one\">Foo</a></p>\n",
        "labels are case-insensitive and the first definition wins"
    );
    assert_eq!(
        to_html("[a]: /u\n\"multi\nline\"\n\n[a]"),
        "<p><a href=\"/u\" title=\"multi\nline\">a</a></p>\n"
    );
    assert_eq!(
        to_html("[a]: /u\n"),
        "",
        "a definition alone renders nothing"
    );
    assert_eq!(to_html("[x][missing]"), "<p>[x][missing]</p>\n");
}

#[test]
fn images_flatten_alt_text() {
    assert_eq!(
        to_html("![a *b* `c`](/i.png \"t\")"),
        "<p><img src=\"/i.png\" alt=\"a b c\" title=\"t\" /></p>\n"
    );
    assert_eq!(
        to_html("![a ![b](/inner.png)](/outer.png)"),
        "<p><img src=\"/outer.png\" alt=\"a b\" /></p>\n"
    );
}

#[test]
fn entities_and_backslash_escapes() {
    assert_eq!(
        to_html("&amp; &#35; &#x41; \\* \\<"),
        "<p>&amp; # A * &lt;</p>\n"
    );
    assert_eq!(to_html("&bogus; &#0;"), "<p>&amp;bogus; \u{fffd}</p>\n");
    assert_eq!(to_html("&copy; &nbsp;"), "<p>\u{a9} \u{a0}</p>\n");
}

#[test]
fn html_blocks_and_comments() {
    let raw = Options::default().with_raw_html(true);
    assert_eq!(
        to_html_with("<div>\n*hi*\n</div>\n\nafter", &raw),
        "<div>\n*hi*\n</div>\n<p>after</p>\n"
    );
    assert_eq!(
        to_html_with("<!-- c -->\nfoo", &raw),
        "<!-- c -->\n<p>foo</p>\n"
    );
    assert_eq!(
        to_html("<!-- c -->\nfoo"),
        "<p>&lt;!-- c --&gt;</p>\n<p>foo</p>\n"
    );
    assert!(matches!(
        parse("<div>x</div>").blocks[0],
        Block::HtmlBlock(_)
    ));
}

#[test]
fn line_endings_nul_and_unicode() {
    assert_eq!(to_html("a\r\nb\rc"), "<p>a\nb\nc</p>\n");
    assert_eq!(to_html("a\0b"), "<p>a\u{fffd}b</p>\n");
    assert_eq!(
        to_html("héllo *wörld* ✓"),
        "<p>héllo <em>wörld</em> ✓</p>\n"
    );
    assert_eq!(to_html(""), "");
    assert_eq!(to_html("\n\n"), "");
}

// -- GFM -------------------------------------------------------------------------

#[test]
fn gfm_tables_alignment_and_edge_cases() {
    let table = "| a | b |\n|:--|--:|\n| 1 | 2 |";
    assert_eq!(
        to_html(table),
        "<table>\n<thead>\n<tr>\n<th align=\"left\">a</th>\n<th align=\"right\">b</th>\n</tr>\n\
         </thead>\n<tbody>\n<tr>\n<td align=\"left\">1</td>\n<td align=\"right\">2</td>\n</tr>\n\
         </tbody>\n</table>\n"
    );
    // The wrong number of delimiter cells is not a table.
    assert!(!to_html("| a | b |\n|---|\n| 1 | 2 |").contains("<table>"));
    // Disabled by option.
    assert!(!to_html_with(table, &Options::commonmark()).contains("<table>"));
    // Rows are padded and truncated; pipes can be escaped; header only is fine.
    let html = to_html("a | b\n--|--\n1\n2 | 3 | 4\n\\| x | y");
    assert_eq!(count(&html, "<td>"), 6, "{html}");
    assert!(html.contains("<td>| x</td>"), "{html}");
    assert_eq!(
        to_html("| h |\n|---|"),
        "<table>\n<thead>\n<tr>\n<th>h</th>\n</tr>\n</thead>\n</table>\n"
    );
    // A paragraph before the header stays a paragraph, and a blank line ends the table.
    let html = to_html("intro\n| a |\n|---|\n| 1 |\n\nafter");
    assert!(html.starts_with("<p>intro</p>\n<table>"), "{html}");
    assert!(html.ends_with("</table>\n<p>after</p>\n"), "{html}");
}

#[test]
fn gfm_table_cells_hold_inline_markup() {
    let document = parse("| *a* | `b` |\n|---|---|\n| [c](/c) | d |");
    let Block::Table(table) = &document.blocks[0] else {
        panic!("expected a table")
    };
    assert_eq!(table.alignments, vec![Alignment::Unset, Alignment::Unset]);
    assert!(matches!(table.head[0][0], Inline::Emphasis(_)));
    assert!(matches!(table.head[1][0], Inline::Code(_)));
    assert!(matches!(table.rows[0][0][0], Inline::Link { .. }));
}

#[test]
fn gfm_strikethrough() {
    assert_eq!(to_html("~~a~~ ~b~"), "<p><del>a</del> <del>b</del></p>\n");
    let mut off = Options::default();
    off.strikethrough = false;
    assert_eq!(to_html_with("~~a~~", &off), "<p>~~a~~</p>\n");
    assert_eq!(to_html("x ~~~a~~~ y"), "<p>x ~~~a~~~ y</p>\n");
}

#[test]
fn gfm_task_lists() {
    assert_eq!(
        to_html("- [x] a\n- [ ] b\n- [x]c"),
        "<ul>\n\
         <li><input type=\"checkbox\" disabled=\"\" checked=\"\" /> a</li>\n\
         <li><input type=\"checkbox\" disabled=\"\" /> b</li>\n\
         <li>[x]c</li>\n</ul>\n"
    );
    let Block::List(list) = &parse("- [x] a\n- [ ] b").blocks[0] else {
        panic!("expected a list")
    };
    assert_eq!(list.items[0].checked, Some(true));
    assert_eq!(list.items[1].checked, Some(false));
    let mut off = Options::default();
    off.task_lists = false;
    assert!(to_html_with("- [x] a", &off).contains("[x] a"));
}

#[test]
fn gfm_autolinks() {
    assert_eq!(
        to_html("see https://a.com/x?y=1. and www.b.com, x@y.org"),
        "<p>see <a href=\"https://a.com/x?y=1\">https://a.com/x?y=1</a>. and \
         <a href=\"http://www.b.com\">www.b.com</a>, \
         <a href=\"mailto:x@y.org\">x@y.org</a></p>\n"
    );
    assert_eq!(
        to_html("(https://a.com/b_(c))"),
        "<p>(<a href=\"https://a.com/b_(c)\">https://a.com/b_(c)</a>)</p>\n"
    );
    // Not autolinks.
    assert_eq!(to_html("awww.b.com"), "<p>awww.b.com</p>\n");
    assert_eq!(to_html("http://nodot"), "<p>http://nodot</p>\n");
    // Angle autolinks work without the extension; literals do not.
    let off = Options::commonmark();
    assert_eq!(
        to_html_with("<https://c.d> https://e.f", &off),
        "<p><a href=\"https://c.d\">https://c.d</a> https://e.f</p>\n"
    );
    assert_eq!(
        to_html("<me@x.org>"),
        "<p><a href=\"mailto:me@x.org\">me@x.org</a></p>\n"
    );
}

// -- security --------------------------------------------------------------------

#[test]
fn raw_html_is_escaped_by_default() {
    assert_eq!(
        to_html("<script>alert(1)</script>"),
        "<p>&lt;script&gt;alert(1)&lt;/script&gt;</p>\n"
    );
    assert_eq!(
        to_html("a <b onclick=\"x\">b</b>"),
        "<p>a &lt;b onclick=&#34;x&#34;&gt;b&lt;/b&gt;</p>\n"
    );
    assert_eq!(
        to_html("<img src=x onerror=alert(1)>"),
        "<p>&lt;img src=x onerror=alert(1)&gt;</p>\n"
    );
    assert!(!to_html("<iframe src=//evil></iframe>").contains("<iframe"));
}

#[test]
fn raw_html_opt_in_applies_the_tag_filter() {
    let raw = Options::default().with_raw_html(true);
    assert_eq!(
        to_html_with("<b>ok</b> <iframe src=x></iframe>", &raw),
        "<p><b>ok</b> &lt;iframe src=x>&lt;/iframe></p>\n"
    );
    let html = to_html_with("<SCRIPT>alert(1)</SCRIPT>\n\n<style>p{}</style>", &raw);
    assert!(!html.to_lowercase().contains("<script"), "{html}");
    assert!(!html.to_lowercase().contains("<style"), "{html}");
    assert!(to_html_with("<div>x</div>", &raw).contains("<div>x</div>"));
}

#[test]
fn dangerous_url_schemes_are_neutralized() {
    for source in [
        "[x](javascript:alert(1))",
        "[x](JaVaScRiPt:alert(1))",
        "[x](java&#x09;script:alert(1))",
        "[x](&#106;avascript:alert(1))",
        "[x](&#x6A;&#x61;vascript:alert(1))",
        "[x](vbscript:msgbox(1))",
        "[x](data:text/html;base64,PHNjcmlwdD4=)",
        "[x](<javascript:alert(1)>)",
        "[x][r]\n\n[r]: javascript:alert(1)",
        "[x](a:b)",
    ] {
        let html = to_html(source);
        assert_eq!(html, "<p>x</p>\n", "{source}");
    }
    for source in [
        "![x](javascript:alert(1))",
        "![x](data:image/svg+xml;base64,PHN2Zz4=)",
        "![x](vbscript:x)",
    ] {
        let html = to_html(source);
        assert_eq!(html, "<p>x</p>\n", "{source}");
    }
    assert!(!to_html("<javascript:alert(1)>").contains("href=\"javascript"));
    assert!(!to_html("javascript:alert(1)").contains("<a"));
}

#[test]
fn safe_urls_are_kept_and_normalized() {
    assert_eq!(
        to_html("[a](<https://e.com/a b>)"),
        "<p><a href=\"https://e.com/a%20b\">a</a></p>\n"
    );
    assert_eq!(
        to_html("[a](/x?a=1&b=2)"),
        "<p><a href=\"/x?a=1&amp;b=2\">a</a></p>\n"
    );
    assert_eq!(
        to_html("[a](/caf\u{e9})"),
        "<p><a href=\"/caf%C3%A9\">a</a></p>\n"
    );
    assert_eq!(to_html("[a](/100%25 /50%)"), "<p>[a](/100%25 /50%)</p>\n");
    for url in [
        "#frag",
        "mailto:a@b.c",
        "//cdn.example/x",
        "./y:z",
        "tel:+1234",
    ] {
        let html = to_html(&format!("[a]({url})"));
        assert!(html.contains("<a href="), "{url}: {html}");
    }
    let ftp = Options::default().with_allowed_scheme("ftp");
    assert!(to_html_with("[a](ftp://h/f)", &ftp).contains("href=\"ftp://h/f\""));
    assert!(!to_html("[a](ftp://h/f)").contains("href"));
    // `data:` can never be allowed back.
    let data = Options::default().with_allowed_scheme("data");
    assert!(!to_html_with("[a](data:text/plain,hi)", &data).contains("href"));
}

#[test]
fn attribute_injection_is_escaped() {
    let html = to_html("[a](/u \"x\\\" onmouseover=\\\"y\")");
    assert_eq!(
        html,
        "<p><a href=\"/u\" title=\"x&#34; onmouseover=&#34;y\">a</a></p>\n"
    );
    let html = to_html("```\"><script>alert(1)</script>\ncode\n```");
    assert!(!html.contains("<script>"), "{html}");
    assert!(
        html.contains("class=\"language-&#34;&gt;&lt;script&gt;"),
        "{html}"
    );
    let html = to_html("![\"><img src=x onerror=alert(1)>](/i.png)");
    assert!(!html.contains("<img src=x"), "{html}");
    let html = to_html("[a](/u\"onclick=\"x)");
    assert!(!html.contains("onclick=\"x"), "{html}");
}

#[test]
fn hand_built_trees_are_rendered_safely() {
    let document = Document {
        blocks: vec![
            Block::Paragraph(vec![Inline::Link {
                destination: "javascript:alert(1)".into(),
                title: None,
                content: vec![Inline::Text("<b>".into())],
            }]),
            Block::HtmlBlock("<img onerror=x>".into()),
            Block::Heading {
                level: 99,
                content: vec![Inline::HtmlInline("<script>".into())],
            },
            Block::CodeBlock {
                info: Some("a\"b c".into()),
                literal: "<x>".into(),
            },
        ],
    };
    assert_eq!(
        render_html(&document, &Options::default()),
        "<p>&lt;b&gt;</p>\n<p>&lt;img onerror=x&gt;</p>\n<h6>&lt;script&gt;</h6>\n\
         <pre><code class=\"language-a&#34;b\">&lt;x&gt;</code></pre>\n"
    );
}

#[test]
fn is_safe_url_policy_table() {
    let options = Options::default();
    for ok in [
        "http://a",
        "HTTPS://a",
        "mailto:x@y",
        "tel:1",
        "/p",
        "p/q",
        "#f",
        "?q=a:b",
        "//h",
        "a/b:c",
        "",
        "./a:b",
    ] {
        assert!(is_safe_url(ok, &options), "{ok:?} should be allowed");
    }
    for bad in [
        "javascript:1",
        " JAVASCRIPT:1",
        "java\tscript:1",
        "java\nscript:1",
        "\u{1}javascript:1",
        "vbscript:1",
        "data:text/html,x",
        "DATA:image/png;base64,AA",
        "file:///etc/passwd",
        "foo:bar",
        "j\u{a0}avascript:1",
    ] {
        assert!(!is_safe_url(bad, &options), "{bad:?} should be refused");
    }
}

// -- nesting bombs and hostile input ----------------------------------------------

#[test]
fn block_quote_nesting_bomb() {
    let html = bounded_html(">".repeat(100_000));
    assert_eq!(count(&html, "<blockquote>"), 32);
    assert!(
        html.contains("&gt;&gt;&gt;"),
        "the excess markers stay as text"
    );

    let narrow = on_small_stack(|| {
        to_html_with(
            &">".repeat(1_000),
            &Options::default().with_max_nesting_depth(4),
        )
    });
    assert_eq!(count(&narrow, "<blockquote>"), 4);

    let spaced = bounded_html("> ".repeat(50_000));
    assert_eq!(count(&spaced, "<blockquote>"), 32);

    let lines = bounded_html("> a\n".repeat(50_000));
    assert_eq!(count(&lines, "<blockquote>"), 1);
}

#[test]
fn list_nesting_bomb() {
    let html = bounded_html(format!("{}x", "- ".repeat(100_000)));
    assert!(count(&html, "<ul>") <= 32, "{}", count(&html, "<ul>"));

    let mut staircase = String::new();
    for level in 0..2_000 {
        staircase.push_str(&" ".repeat(level * 2));
        staircase.push_str("- x\n");
    }
    let html = bounded_html(staircase);
    assert!(count(&html, "<ul>") <= 32);
    assert!(max_depth(&html, "ul") <= 32);

    let ordered = bounded_html("1. ".repeat(50_000));
    assert!(count(&ordered, "<ol>") <= 32);
}

#[test]
fn inline_nesting_bombs() {
    let cases = [
        "[".repeat(100_000),
        "![".repeat(100_000),
        "[a](".repeat(50_000),
        "[a]".repeat(50_000),
        format!("{}a{}", "[".repeat(50_000), "]".repeat(50_000)),
        format!("{}a{}", "*".repeat(100_000), "*".repeat(100_000)),
        format!("{}b{}", "*a ".repeat(40_000), "* ".repeat(40_000)),
        format!("{}x{}", "_a ".repeat(30_000), " a_".repeat(30_000)),
        "*a **a ".repeat(20_000) + &"a** a* ".repeat(20_000),
        format!("{}x{}", "~~a ".repeat(30_000), " a~~".repeat(30_000)),
    ];
    for (index, input) in cases.into_iter().enumerate() {
        let html = bounded_html(input);
        assert!(max_depth(&html, "em") <= 32, "case {index}");
        assert!(max_depth(&html, "strong") <= 32, "case {index}");
        assert!(max_depth(&html, "del") <= 32, "case {index}");
        assert!(max_depth(&html, "a") <= 32, "case {index}");
    }
}

#[test]
fn nested_emphasis_beyond_the_limit_keeps_the_text() {
    let depth = 200;
    let input = format!("{}core{}", "*a ".repeat(depth), "* ".repeat(0));
    assert!(bounded_html(input).contains("core"));

    let open = "*x **y ".repeat(60);
    let close = "y** x* ".repeat(60);
    let html = bounded_html(format!("{open}core {close}"));
    assert!(html.contains("core"));
    assert!(max_depth(&html, "em") <= 32);
    assert!(max_depth(&html, "strong") <= 32);
    // The text of flattened wrappers is preserved.
    assert!(count(&html, "y") >= 120);
}

#[test]
fn adversarial_inputs_stay_linear() {
    let cases: Vec<(&str, String)> = vec![
        ("openers", "*a ".repeat(60_000)),
        ("closers", "a* ".repeat(60_000)),
        ("underscores", "_a".repeat(100_000)),
        ("mixed delimiters", "*a_ ".repeat(40_000)),
        ("rule of three", "***a**".repeat(30_000)),
        ("link openers", "[a](".repeat(60_000)),
        ("link text", "[a]".repeat(60_000)),
        ("empty brackets", "[]".repeat(60_000)),
        ("angle destinations", "[a](<".repeat(40_000)),
        ("unterminated titles", "[a](b \"".repeat(40_000)),
        ("image openers", "![a".repeat(60_000)),
        ("single backticks", "`a".repeat(100_000)),
        (
            "varying backtick runs",
            (1..450).map(|n| format!("{}a", "`".repeat(n))).collect(),
        ),
        ("comment openers", "<!--".repeat(50_000)),
        ("pi openers", "<?a ".repeat(50_000)),
        ("cdata openers", "<![CDATA[".repeat(30_000)),
        ("declaration openers", "<!A ".repeat(50_000)),
        ("tag openers", "<a b='".repeat(40_000)),
        ("tags without end", "<a ".repeat(60_000)),
        ("ampersands", "&".repeat(200_000)),
        ("backslashes", "\\".repeat(200_000)),
        ("www links", "www.".repeat(50_000)),
        ("http links", "http://".repeat(30_000)),
        ("emails", "a@".repeat(60_000)),
        ("bad domains", "www.a_.b_".repeat(30_000)),
        ("quote lines", "> a\n".repeat(60_000)),
        ("list lines", "- a\n".repeat(60_000)),
        ("paragraph lines", "a\n".repeat(120_000)),
        ("blank lines", "\n".repeat(200_000)),
        ("ref definitions", "[x]: /u\n".repeat(30_000)),
        ("table rows", "| a |\n|---|\n".repeat(30_000)),
        ("fences", "```\n".repeat(60_000)),
        ("setext", "a\n---\n".repeat(40_000)),
        ("html blocks", "<div>\n\n".repeat(40_000)),
        ("tab indents", "\t\t\t\ta\n".repeat(40_000)),
        ("near-miss tables", "a|b|c\n|-|\n".repeat(50_000)),
        (
            "wide tables",
            format!("{}|\n{}|\n", "|a".repeat(50_000), "|-".repeat(50_000)),
        ),
        (
            "table rows",
            format!("|a|\n|-|\n{}", "|x|\n".repeat(60_000)),
        ),
        ("bracket emphasis", "[*a ".repeat(30_000) + "]"),
        (
            "long labels",
            format!("[{}]: /u\n\n{}", "a".repeat(900), "[a]".repeat(30_000)),
        ),
        ("nested quotes in lists", "- > ".repeat(60_000)),
        ("many item blank lines", "- a\n\n".repeat(40_000)),
        (
            "nested emphasis runs",
            "*_".repeat(80_000) + "a" + &"_*".repeat(80_000),
        ),
        ("deep links", "[a](b ".repeat(40_000)),
    ];
    for (name, input) in cases {
        let started = Instant::now();
        let html = on_small_stack(move || to_html(&input));
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_secs(20),
            "{name}: {elapsed:?} for {} bytes of output",
            html.len()
        );
    }
}

#[test]
fn reference_lookups_do_not_go_quadratic() {
    let mut input = String::new();
    for n in 0..2_000 {
        input.push_str(&format!("[r{n}]: /u{n}\n"));
    }
    input.push('\n');
    input.push_str(&"[r1999] [nope][r3] [r4][] ".repeat(10_000));
    let started = Instant::now();
    let html = on_small_stack(move || to_html(&input));
    assert!(started.elapsed() < Duration::from_secs(20));
    assert!(html.contains("href=\"/u1999\""));
}

#[test]
fn renderer_skips_hand_built_trees_beyond_the_limit() {
    let mut block = Block::Paragraph(vec![Inline::Text("deep".into())]);
    for _ in 0..1_000 {
        block = Block::BlockQuote(vec![block]);
    }
    let mut inline = Inline::Text("leaf".into());
    for _ in 0..1_000 {
        inline = Inline::Emphasis(vec![inline]);
    }
    let document = Document {
        blocks: vec![block, Block::Paragraph(vec![inline])],
    };
    let options = Options::default();
    // Render on a tiny stack: rendering must be iterative. The tree is
    // dropped on this (large) stack.
    let html = thread::scope(|scope| {
        thread::Builder::new()
            .stack_size(128 * 1024)
            .spawn_scoped(scope, || render_html(&document, &options))
            .expect("spawn")
            .join()
            .expect("rendering must not overflow the stack")
    });
    assert_eq!(count(&html, "<blockquote>"), 32);
    assert_eq!(count(&html, "<em>"), 32);
    assert!(!html.contains("deep") && !html.contains("leaf"));
}

/// Every `<img` and `<a` must be a generated tag whose URL is not scripted.
fn assert_only_safe_links(html: &str, input: &str) {
    for (tag, attribute) in [("<img", " src=\""), ("<a", " href=\"")] {
        for (at, _) in html.match_indices(tag) {
            let rest = &html[at + tag.len()..];
            assert!(
                rest.starts_with(attribute),
                "{input:?} produced a stray {tag}: {html:?}"
            );
            let url = rest[attribute.len()..].to_lowercase();
            for scheme in ["javascript", "vbscript", "data:"] {
                assert!(!url.starts_with(scheme), "{input:?} -> {html:?}");
            }
        }
    }
}

#[test]
fn hostile_fuzz_lite_never_panics_or_emits_active_content() {
    let fragments = [
        "<script>",
        "javascript:",
        "[",
        "](",
        ")",
        "*",
        "`",
        "\n",
        ">",
        "- ",
        "|",
        "<",
        "&",
        "\"",
        "onerror=",
        "!",
        "1. ",
        "# ",
        "~~",
        "_",
        "\t",
        "    ",
        "<!--",
        "-->",
        "](<",
        "[x]: ",
        "www.",
        "@",
        "http://",
        "\\",
        "---",
        "=",
        ":",
        "<img src=",
        "é",
        "日本",
        "😀",
        "\u{a0}",
        "\u{2028}",
        "\u{feff}",
        "\\é",
        "[é](é)",
        "&é;",
        "<é>",
        "www.é.com",
        "é@é.org",
    ];
    let mut state = 0x2545_f491_4f6c_dd1du64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let raw = Options::default().with_raw_html(true);
    for _ in 0..3_000 {
        let pieces = (next() % 40) as usize;
        let input: String = (0..pieces)
            .map(|_| fragments[(next() % fragments.len() as u64) as usize])
            .collect();
        let html = to_html(&input);
        let lower = html.to_lowercase();
        assert!(!lower.contains("<script"), "{input:?} -> {html:?}");
        assert_only_safe_links(&html, &input);
        // With raw HTML on, the tag filter still removes `<script`.
        let lower = to_html_with(&input, &raw).to_lowercase();
        assert!(!lower.contains("<script"), "{input:?}");
        let _ = parse_with(&input, &Options::commonmark());
    }
}

// -- API and options -----------------------------------------------------------

#[test]
fn options_defaults_and_builders() {
    let options = Options::default();
    assert!(options.tables && options.strikethrough && options.task_lists && options.autolinks);
    assert!(!options.allow_raw_html);
    assert_eq!(options.max_nesting_depth, 32);
    assert_eq!(options.allowed_schemes, ["http", "https", "mailto", "tel"]);
    assert_eq!(Options::new(), options);

    let commonmark = Options::commonmark();
    assert!(
        !commonmark.tables
            && !commonmark.strikethrough
            && !commonmark.task_lists
            && !commonmark.autolinks
    );
    assert!(!commonmark.allow_raw_html);

    let built = Options::new().with_raw_html(true).with_max_nesting_depth(3);
    assert!(built.allow_raw_html);
    assert_eq!(built.max_nesting_depth, 3);
}

#[test]
fn zero_and_huge_depth_limits_are_clamped() {
    let zero = Options::default().with_max_nesting_depth(0);
    assert_eq!(count(&to_html_with("> a", &zero), "<blockquote>"), 1);
    let huge = Options::default().with_max_nesting_depth(usize::MAX);
    // The hard cap of 128 levels needs a few hundred KiB in a debug build.
    let html = on_stack(1024, move || to_html_with(&">".repeat(5_000), &huge));
    assert_eq!(count(&html, "<blockquote>"), 128);
}

#[test]
fn ast_shape_matches_the_source() {
    let document = parse("# T\n\n- a\n- b\n\n```rs\nx\n```");
    assert_eq!(
        document,
        Document {
            blocks: vec![
                Block::Heading {
                    level: 1,
                    content: vec![Inline::Text("T".into())],
                },
                Block::List(List {
                    kind: ListKind::Bullet,
                    tight: true,
                    items: vec![
                        ListItem {
                            checked: None,
                            blocks: vec![Block::Paragraph(vec![Inline::Text("a".into())])],
                        },
                        ListItem {
                            checked: None,
                            blocks: vec![Block::Paragraph(vec![Inline::Text("b".into())])],
                        },
                    ],
                }),
                Block::CodeBlock {
                    info: Some("rs".into()),
                    literal: "x\n".into(),
                },
            ]
        }
    );
    assert_eq!(parse_with("*a*", &Options::commonmark()), parse("*a*"));
    assert_eq!(Document::default().blocks.len(), 0);
}

#[test]
fn rendering_a_parsed_tree_equals_to_html() {
    let source = "# T\n\n> q\n\n1. a\n2. b\n\n| a |\n|---|\n| b |\n\n`c` **d** [e](/f)";
    let options = Options::default();
    assert_eq!(render_html(&parse(source), &options), to_html(source));
}

// -- regression: verified audit findings ------------------------------------

#[test]
fn tag_filter_matches_every_html_tag_name_terminator() {
    const BLOCKED: [&str; 9] = [
        "title",
        "textarea",
        "style",
        "xmp",
        "iframe",
        "noembed",
        "noframes",
        "script",
        "plaintext",
    ];
    const SEPARATORS: [&str; 7] = ["\t", "\n", "\x0c", "\r", " ", "/", ">"];
    let raw = Options::default().with_raw_html(true);
    for tag in BLOCKED {
        for separator in SEPARATORS {
            for case in [tag.to_owned(), tag.to_uppercase()] {
                for source in [
                    format!("<{case}{separator}>x</{case}{separator}>"),
                    format!("a <{case}{separator}x=1> b </{case}{separator}>"),
                    format!("<div>\n<{case}{separator}>\n</div>"),
                    format!("<{case}{separator}"),
                ] {
                    let html = to_html_with(&source, &raw).to_lowercase();
                    for opener in [format!("<{tag}"), format!("</{tag}")] {
                        for next in SEPARATORS {
                            let needle = format!("{opener}{next}");
                            assert!(
                                !html.contains(&needle),
                                "{source:?} -> {html:?} contains {needle:?}"
                            );
                        }
                        assert!(
                            !html.ends_with(&opener),
                            "{source:?} -> {html:?} ends with {opener:?}"
                        );
                    }
                }
            }
        }
    }
    // A tag whose name merely starts with a blocked name is not filtered.
    assert!(to_html_with("<scripts>", &raw).contains("<scripts>"));
}

#[test]
fn www_autolinks_stop_at_whitespace_and_trim_punctuation() {
    assert_eq!(
        to_html("www.example.com foo"),
        "<p><a href=\"http://www.example.com\">www.example.com</a> foo</p>\n"
    );
    assert_eq!(
        to_html("www.example.com\tfoo"),
        "<p><a href=\"http://www.example.com\">www.example.com</a>\tfoo</p>\n"
    );
    assert_eq!(
        to_html("https://example.com foo"),
        "<p><a href=\"https://example.com\">https://example.com</a> foo</p>\n"
    );
    for (suffix, rest) in [
        (".", "."),
        ("?", "?"),
        ("!", "!"),
        (",", ","),
        (":", ":"),
        ("~", "~"),
        ("...", "..."),
        ("?!.", "?!."),
    ] {
        assert_eq!(
            to_html(&format!("www.example.com{suffix} x")),
            format!("<p><a href=\"http://www.example.com\">www.example.com</a>{rest} x</p>\n"),
            "{suffix:?}"
        );
    }
    // Unbalanced closing parentheses are trimmed; balanced ones stay.
    assert_eq!(
        to_html("(www.example.com)"),
        "<p>(<a href=\"http://www.example.com\">www.example.com</a>)</p>\n"
    );
    assert_eq!(
        to_html("www.example.com/a(b) x"),
        "<p><a href=\"http://www.example.com/a(b)\">www.example.com/a(b)</a> x</p>\n"
    );
    assert_eq!(
        to_html("www.example.com/a(b))) x"),
        "<p><a href=\"http://www.example.com/a(b)\">www.example.com/a(b)</a>)) x</p>\n"
    );
    // Trailing `*` / `_` belong to emphasis or are trimmed, never linked.
    for source in [
        "*www.example.com*",
        "_www.example.com_",
        "www.example.com_",
        "www.example.com*",
    ] {
        let html = to_html(source);
        assert!(
            html.contains("<a href=\"http://www.example.com\">www.example.com</a>"),
            "{source:?} -> {html:?}"
        );
    }
    // A trailing entity reference is not part of the link; one inside stays.
    assert_eq!(
        to_html("www.example.com/a&hl; x"),
        "<p><a href=\"http://www.example.com/a\">www.example.com/a</a>&amp;hl; x</p>\n"
    );
    let html = to_html("www.example.com/?a=1&amp;b=2 x");
    assert!(html.contains("</a> x</p>"), "{html}");
    assert!(!html.contains(" x</a>"), "{html}");
    // Whitespace before an `<` or at the end never ends up in the link.
    assert_eq!(
        to_html("www.example.com "),
        "<p><a href=\"http://www.example.com\">www.example.com</a></p>\n"
    );
}

#[test]
fn autolink_trailing_parens_are_linear() {
    let input = format!("www.a.com{}", ")".repeat(200_000));
    let started = Instant::now();
    let html = to_html(&input);
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "took {:?}",
        started.elapsed()
    );
    assert!(html.contains("<a href=\"http://www.a.com\">www.a.com</a>)))"));
    // Balanced parentheses still stay in the link.
    assert!(to_html("www.a.com/(b)))").contains(">www.a.com/(b)</a>))"));
}

#[test]
fn delimiter_rows_after_an_unclosed_ref_title_are_linear() {
    let mut input = String::from("[a]: b \"\n");
    for _ in 0..100_000 {
        input.push_str("x\n|-|-|\n");
    }
    let started = Instant::now();
    let html = to_html(&input);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "took {:?}",
        started.elapsed()
    );
    assert!(!html.contains("<table>"));
    assert!(html.starts_with("<p>[a]: b &#34;"), "{}", &html[..40]);

    // Escaped quotes do not count as closing the title, and stay linear too.
    let mut input = String::from("[a]: b \"\n");
    for _ in 0..50_000 {
        input.push_str("\\\"\n|-|-|\n");
    }
    let started = Instant::now();
    to_html(&input);
    assert!(started.elapsed() < Duration::from_secs(2));

    // A title closed by a later line still makes a definition, and the table
    // after it starts.
    let html = to_html("[a]: /u \"t\nu\"\n| h | i |\n|-|-|\n\n[x][a]");
    assert!(html.contains("<table>"), "{html}");
    assert!(
        html.contains("<a href=\"/u\" title=\"t\nu\">x</a>"),
        "{html}"
    );
    let html = to_html("[a]:\n/u\n| h | i |\n|-|-|\n\n[x][a]");
    assert!(
        html.contains("<table>") && html.contains("href=\"/u\""),
        "{html}"
    );
    let html = to_html("[a]: /u \"t\nx\n|-|-|\nu\" y\n| h | i |\n|-|-|\n\n[x][a]");
    assert!(html.contains("<table>") && !html.contains("href"), "{html}");
}
