//! Integration tests for `akku::template`.

use std::collections::BTreeMap;
use std::thread;
use std::time::{Duration, Instant};

use akku::html::Html;
use akku::json;
use akku::template::{
    render, render_str, AutoEscape, Context, Engine, Error, ErrorKind, Template, Value,
};

// -- helpers -------------------------------------------------------------------

fn run(source: &str, context: &Context) -> String {
    render_str(source, context).unwrap_or_else(|error| panic!("{source:?}: {error}"))
}

fn fail(source: &str, context: &Context) -> Error {
    match render_str(source, context) {
        Ok(output) => panic!("{source:?} unexpectedly rendered {output:?}"),
        Err(error) => error,
    }
}

fn at(error: &Error) -> (u32, u32) {
    let position = error.position().expect("the error has a position");
    (position.line, position.column)
}

fn ctx() -> Context {
    Context::new()
}

/// Run `f` on a thread with a small (1 MiB) stack. The deepest nesting the
/// limits allow needs a few hundred KiB in a debug build, while the 10,000
/// levels of the bombs below would need over 100 MiB: a missing limit shows
/// up as a stack overflow instead of passing by luck.
fn on_small_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    on_stack(1024, f)
}

fn on_stack<T: Send + 'static>(kibibytes: usize, f: impl FnOnce() -> T + Send + 'static) -> T {
    thread::Builder::new()
        .stack_size(kibibytes * 1024)
        .spawn(f)
        .expect("spawn")
        .join()
        .expect("the thread must not overflow its stack or panic")
}

// -- output and escaping ---------------------------------------------------------

#[test]
fn plain_text_and_variables() {
    assert_eq!(
        run("Hello, {{ name }}!", &ctx().with("name", "World")),
        "Hello, World!"
    );
    assert_eq!(run("a { b } c {x} {", &ctx()), "a { b } c {x} {");
    assert_eq!(run("", &ctx()), "");
    assert_eq!(
        run("{{ a }}{{ b }}", &ctx().with("a", 1).with("b", 2)),
        "12"
    );
}

#[test]
fn html_is_escaped_by_default() {
    let context = ctx().with("name", "<b>\"x\"&'y'</b>");
    assert_eq!(
        run("<i>{{ name }}</i>", &context),
        "<i>&lt;b&gt;&#34;x&#34;&amp;&#39;y&#39;&lt;/b&gt;</i>"
    );
}

#[test]
fn auto_escape_can_be_turned_off_per_engine() {
    let mut engine = Engine::new();
    engine.set_auto_escape(AutoEscape::None);
    let context = ctx().with("x", "<&>");
    assert_eq!(engine.render_str("{{ x }}", &context).unwrap(), "<&>");
    // An explicit `escape` still escapes.
    assert_eq!(
        engine.render_str("{{ x | escape }}", &context).unwrap(),
        "&lt;&amp;&gt;"
    );
}

#[test]
fn safe_markup_is_not_escaped() {
    let context = ctx()
        .with("x", "<b>")
        .with("html", Html::new("<i>"))
        .with("value", Value::safe("<u>"));
    assert_eq!(run("{{ x | safe }}", &context), "<b>");
    assert_eq!(run("{{ html }}{{ value }}", &context), "<i><u>");
    assert_eq!(run("{{ x }}", &context), "&lt;b&gt;");
}

#[test]
fn escape_filter_does_not_double_escape() {
    let context = ctx().with("x", "<");
    assert_eq!(run("{{ x | escape }}", &context), "&lt;");
    assert_eq!(run("{{ x | e }}", &context), "&lt;");
    // Safe text stays as it is, so `safe | escape` prints the raw text.
    assert_eq!(run("{{ x | safe | escape }}", &context), "<");
    // Filters other than the markup-keeping ones return plain text again.
    assert_eq!(run("{{ x | escape | upper }}", &context), "&amp;LT;");
}

#[test]
fn attribute_injection_is_neutralized() {
    let context = ctx().with("t", "\" onmouseover=\"alert(1)");
    assert_eq!(
        run("<a title=\"{{ t }}\">", &context),
        "<a title=\"&#34; onmouseover=&#34;alert(1)\">"
    );
    let context = ctx().with("u", "javascript:alert(1)\"><script>");
    let html = run("<a href=\"{{ u }}\">", &context);
    assert!(
        !html.contains("<script>") && !html.contains("\"><"),
        "{html}"
    );
}

#[test]
fn comments_and_whitespace_control() {
    assert_eq!(run("a {# hidden #} b", &ctx()), "a  b");
    assert_eq!(run("x\n  {{- 1 -}}  \ny", &ctx()), "x1y");
    assert_eq!(
        run("a  {%- if true -%}  b  {%- endif -%}  c", &ctx()),
        "abc"
    );
    assert_eq!(run("a  {#- c -#}  b", &ctx()), "ab");
    // Trimming only reaches the adjacent text, not through other tags.
    assert_eq!(run("a  {# c -#}{# d #}  b", &ctx()), "a    b");
    assert_eq!(run("{{ -1 }}", &ctx()), "-1");
}

#[test]
fn lookups_attributes_indexes_and_brackets() {
    let user: BTreeMap<&str, Value> = BTreeMap::from([
        ("name", Value::from("Ada")),
        ("tags", Value::from(vec!["a", "b"])),
    ]);
    let context = ctx()
        .with("user", user)
        .with("items", vec![10, 20, 30])
        .with("key", "name");
    assert_eq!(
        run(
            "{{ user.name }}|{{ user['name'] }}|{{ user[key] }}",
            &context
        ),
        "Ada|Ada|Ada"
    );
    assert_eq!(
        run("{{ items.1 }}|{{ items[0] }}|{{ items[1 + 1] }}", &context),
        "20|10|30"
    );
    assert_eq!(run("{{ user.tags.1 }}|{{ user.tags[0] }}", &context), "b|a");
}

#[test]
fn scalars_print_and_containers_do_not() {
    let context = ctx()
        .with("i", 42)
        .with("f", 1.5)
        .with("t", true)
        .with("n", Value::Null)
        .with("list", vec![1]);
    assert_eq!(
        run("{{ i }} {{ f }} {{ t }}[{{ n }}]", &context),
        "42 1.5 true[]"
    );
    let error = fail("ab {{ list }}", &context);
    assert!(matches!(
        error.kind(),
        ErrorKind::NotPrintable { type_name: "list" }
    ));
    assert_eq!(at(&error), (1, 7));
}

// -- control flow ----------------------------------------------------------------

#[test]
fn conditions() {
    let source =
        "{% if n < 0 %}neg{% elif n == 0 %}zero{% elif n < 10 %}small{% else %}big{% endif %}";
    for (n, expected) in [(-5, "neg"), (0, "zero"), (3, "small"), (99, "big")] {
        assert_eq!(run(source, &ctx().with("n", n)), expected);
    }
}

#[test]
fn truthiness() {
    let falsy: Vec<Value> = vec![
        0.into(),
        0.0.into(),
        "".into(),
        Value::Null,
        false.into(),
        Value::List(vec![]),
        Value::Map(BTreeMap::new()),
    ];
    let truthy: Vec<Value> = vec![
        1.into(),
        0.5.into(),
        "a".into(),
        true.into(),
        vec![0].into(),
        Value::from(BTreeMap::from([("k", 1)])),
    ];
    for value in falsy {
        assert_eq!(
            run(
                "{% if v %}T{% else %}F{% endif %}",
                &ctx().with("v", value.clone())
            ),
            "F",
            "{value:?}"
        );
    }
    for value in truthy {
        assert_eq!(
            run(
                "{% if v %}T{% else %}F{% endif %}",
                &ctx().with("v", value.clone())
            ),
            "T",
            "{value:?}"
        );
    }
    // A missing variable is falsy in a condition, not an error.
    assert_eq!(run("{% if nope %}T{% else %}F{% endif %}", &ctx()), "F");
    assert_eq!(
        run("{% if nope.deeper.still %}T{% else %}F{% endif %}", &ctx()),
        "F"
    );
}

#[test]
fn operators() {
    let cases = [
        ("{{ 1 + 2 * 3 }}", "7"),
        ("{{ (1 + 2) * 3 }}", "9"),
        ("{{ 7 / 2 }}", "3.5"),
        ("{{ 7 % 4 }}", "3"),
        ("{{ -5 + 2 }}", "-3"),
        ("{{ 1.5 + 1 }}", "2.5"),
        ("{{ 'a' ~ 'b' ~ 1 }}", "ab1"),
        ("{{ 1 == 1.0 }}", "true"),
        ("{{ 'a' == 'a' and 'a' != 'b' }}", "true"),
        ("{{ 'a' < 'b' }}", "true"),
        ("{{ 2 >= 3 }}", "false"),
        ("{{ 2 in [1, 2] }}", "true"),
        ("{{ 'b' in 'abc' }}", "true"),
        ("{{ 'x' not in 'abc' }}", "true"),
        ("{{ not true or true and false }}", "false"),
        ("{{ 1 < 2 and 2 < 3 }}", "true"),
        ("{{ none == none }}", "true"),
        ("{{ nope == none }}", "true"),
    ];
    for (source, expected) in cases {
        assert_eq!(run(source, &ctx()), expected, "{source}");
    }
    for source in [
        "{{ 1 / 0 }}",
        "{{ 1 % 0 }}",
        "{{ 9223372036854775807 + 1 }}",
        "{{ 'a' < 1 }}",
        "{{ 'a' + 1 }}",
        "{{ 1.5 % 2 }}",
        "{{ [1] ~ 'a' }}",
        "{{ nope + 1 }}",
    ] {
        let error = fail(source, &ctx());
        assert!(
            matches!(
                error.kind(),
                ErrorKind::InvalidOperation { .. } | ErrorKind::UndefinedVariable { .. }
            ),
            "{source}: {error}"
        );
    }
}

#[test]
fn for_loops_and_loop_variables() {
    let context = ctx().with("xs", vec!["a", "b", "c"]);
    assert_eq!(
        run(
            "{% for x in xs %}{{ loop.index }}:{{ x }}{% if not loop.last %},{% endif %}{% endfor %}",
            &context
        ),
        "1:a,2:b,3:c"
    );
    let two = ctx().with("xs", vec![1, 2]);
    assert_eq!(
        run(
            "{% for x in xs %}{{ loop.index0 }}{{ loop.first }}{{ loop.last }}{{ loop.length }}{{ loop.revindex }},{% endfor %}",
            &two
        ),
        "0truefalse22,1falsetrue21,"
    );
}

#[test]
fn for_over_maps_empty_lists_and_pairs() {
    let map = Value::from(BTreeMap::from([("b", 2), ("a", 1)]));
    let context = ctx()
        .with("m", map)
        .with("empty", Value::List(vec![]))
        .with("xs", vec!["a", "b"]);
    assert_eq!(
        run(
            "{% for k, v in m %}{{ k }}={{ v }};{% else %}none{% endfor %}",
            &context
        ),
        "a=1;b=2;"
    );
    assert_eq!(run("{% for k in m %}{{ k }}{% endfor %}", &context), "ab");
    assert_eq!(
        run(
            "{% for k, v in empty %}x{% else %}none{% endfor %}",
            &context
        ),
        "none"
    );
    assert_eq!(
        run("{% for i, x in xs %}{{ i }}={{ x }};{% endfor %}", &context),
        "0=a;1=b;"
    );
}

#[test]
fn nested_loops_and_loop_scope() {
    let context = ctx().with("as", vec![1, 2]).with("bs", vec!["x", "y"]);
    assert_eq!(
        run(
            "{% for a in as %}{% for b in bs %}{{ a }}{{ b }}{{ loop.index }} {% endfor %}[{{ loop.index }}]{% endfor %}",
            &context
        ),
        "1x1 1y2 [1]2x1 2y2 [2]"
    );
    let error = fail("{% for a in as %}{% endfor %}{{ a }}", &context);
    assert!(matches!(error.kind(), ErrorKind::UndefinedVariable { name } if name == "a"));
    let error = fail("{% for a in nope %}{% endfor %}", &context);
    assert!(matches!(error.kind(), ErrorKind::UndefinedVariable { .. }));
    let error = fail("{% for a in 'text' %}{% endfor %}", &context);
    assert!(matches!(
        error.kind(),
        ErrorKind::NotIterable {
            type_name: "string"
        }
    ));
}

// -- filters ---------------------------------------------------------------------

#[test]
fn builtin_filters() {
    let context = ctx()
        .with("s", "  Hi <b> ")
        .with("u", "héllo")
        .with("xs", vec!["a", "b"])
        .with("m", Value::from(BTreeMap::from([("k", 1)])));
    let cases = [
        ("{{ 'abc' | upper }}", "ABC"),
        ("{{ 'ABC' | lower }}", "abc"),
        ("{{ '  Hi ' | trim | upper }}", "HI"),
        ("{{ s | trim }}", "Hi &lt;b&gt;"),
        ("{{ u | len }}", "5"),
        ("{{ xs | len }} {{ m | len }}", "2 1"),
        ("{{ xs | join(', ') }}", "a, b"),
        ("{{ xs | join }}", "ab"),
        ("{{ 'a-b-c' | replace('-', '+') }}", "a+b+c"),
        ("{{ 12 | upper }}", "12"),
        ("{{ nope | default('x') }}", "x"),
        ("{{ 'a' | upper | lower | upper }}", "A"),
        ("{{ 'a' | replace('a', 'b') | upper }}", "B"),
        ("{{ ' ab ' | trim | len }}", "2"),
        ("{{ 'a' | upper == 'A' }}", "true"),
    ];
    for (source, expected) in cases {
        assert_eq!(run(source, &context), expected, "{source}");
    }
}

#[test]
fn default_filter() {
    let context = ctx()
        .with("empty", "")
        .with("zero", 0)
        .with("n", Value::Null)
        .with("v", "set");
    assert_eq!(run("{{ nope | default('x') }}", &context), "x");
    assert_eq!(run("{{ nope.deeper | default('y') }}", &context), "y");
    assert_eq!(run("{{ n | default('z') }}", &context), "z");
    assert_eq!(run("{{ v | default('z') }}", &context), "set");
    assert_eq!(run("[{{ empty | default('w') }}]", &context), "[]");
    assert_eq!(run("{{ empty | default('w', true) }}", &context), "w");
    assert_eq!(run("{{ zero | default(5, true) }}", &context), "5");
    assert!(matches!(
        fail("{{ 1 | default }}", &context).kind(),
        ErrorKind::FilterFailed { .. }
    ));
}

#[test]
fn join_escapes_the_whole_result() {
    let context = ctx().with("xs", vec!["<a>", "b"]);
    assert_eq!(run("{{ xs | join(', ') }}", &context), "&lt;a&gt;, b");
    let nested = ctx().with("xs", Value::List(vec![Value::List(vec![])]));
    assert!(matches!(
        fail("{{ xs | join }}", &nested).kind(),
        ErrorKind::FilterFailed { .. }
    ));
}

#[test]
fn json_filter_is_safe_inside_script_tags() {
    let context = Context::from(
        json::parse(r#"{"v": {"a":"</script><b>&'","n":[1,2.5,null,true]}}"#).unwrap(),
    );
    let output = run("{{ v | json }}", &context);
    assert_eq!(
        output,
        r#"{"a":"\u003c/script\u003e\u003cb\u003e\u0026\u0027","n":[1,2.5,null,true]}"#
    );
    assert!(!output.contains(['<', '>', '&', '\'']));
    let separators = ctx().with("s", "a\u{2028}b\u{2029}");
    assert_eq!(run("{{ s | json }}", &separators), "\"a\\u2028b\\u2029\"");
}

#[test]
fn custom_filters() {
    let mut engine = Engine::new();
    engine.add_filter("repeat", |value, args| {
        let times = args
            .first()
            .and_then(Value::as_i64)
            .ok_or("missing count")?;
        Ok(Value::Str(value.to_string().repeat(times as usize)))
    });
    engine.add_filter("html", |_, _| Ok(Value::Str("<b>".into())));
    let prefix = String::from(">> ");
    engine.add_filter("prefixed", move |value, _| {
        Ok(Value::Str(format!("{prefix}{value}")))
    });
    engine.add_filter("upper", |value, _| {
        Ok(Value::Str(format!("custom {value}")))
    });
    let context = ctx();
    assert_eq!(
        engine
            .render_str("{{ 'ab' | repeat(3) }}", &context)
            .unwrap(),
        "ababab"
    );
    // A custom filter's text is untrusted and escaped like any other.
    assert_eq!(
        engine.render_str("{{ 1 | html }}", &context).unwrap(),
        "&lt;b&gt;"
    );
    assert_eq!(
        engine.render_str("{{ 'x' | prefixed }}", &context).unwrap(),
        "&gt;&gt; x"
    );
    // A custom filter overrides a built-in one.
    assert_eq!(
        engine.render_str("{{ 'a' | upper }}", &context).unwrap(),
        "custom a"
    );
    let error = engine
        .render_str("{{ 'ab' | repeat }}", &context)
        .unwrap_err();
    assert!(
        matches!(error.kind(), ErrorKind::FilterFailed { name, message } if name == "repeat" && message == "missing count")
    );
    assert_eq!(at(&error), (1, 11));
}

#[test]
fn filter_errors_have_positions() {
    let context = ctx().with("x", 1);
    let error = fail("a\n{{ x | nope }}", &context);
    assert!(matches!(error.kind(), ErrorKind::UnknownFilter { name } if name == "nope"));
    assert_eq!(at(&error), (2, 8));
    let error = fail("{{ 'a' | replace('', 'x') }}", &context);
    assert!(matches!(error.kind(), ErrorKind::FilterFailed { name, .. } if name == "replace"));
    assert_eq!(at(&error), (1, 10));
    let error = fail("{{ 1 | len }}", &context);
    assert!(matches!(error.kind(), ErrorKind::FilterFailed { .. }));
    let error = fail("{{ nope | upper }}", &context);
    assert!(matches!(error.kind(), ErrorKind::UndefinedVariable { name } if name == "nope"));
    assert_eq!(at(&error), (1, 4));
    let error = fail("{{ 'a' | upper(1) }}", &context);
    assert!(matches!(error.kind(), ErrorKind::FilterFailed { .. }));
}

// -- include and inheritance -----------------------------------------------------

#[test]
fn include_shares_the_scope() {
    let mut engine = Engine::new();
    engine.add_template("row", "<li>{{ x }}</li>").unwrap();
    engine
        .add_template("page", "{% for x in xs %}{% include \"row\" %}{% endfor %}")
        .unwrap();
    let html = engine
        .render("page", &ctx().with("xs", vec![1, 2]))
        .unwrap();
    assert_eq!(html, "<li>1</li><li>2</li>");
}

#[test]
fn include_errors() {
    let mut engine = Engine::new();
    engine.add_template("a", "{% include \"a\" %}").unwrap();
    engine.add_template("broken", "x\n{{ missing }}").unwrap();
    engine
        .add_template("uses_broken", "{% include \"broken\" %}")
        .unwrap();

    let error = engine
        .render_str("{% include \"nope\" %}", &ctx())
        .unwrap_err();
    assert!(matches!(error.kind(), ErrorKind::TemplateNotFound { name } if name == "nope"));
    assert_eq!(at(&error), (1, 12));

    let error = engine.render("a", &ctx()).unwrap_err();
    assert!(matches!(
        error.kind(),
        ErrorKind::IncludeDepthExceeded { limit: 16 }
    ));
    assert_eq!((error.template(), at(&error)), (Some("a"), (1, 12)));

    engine.set_max_include_depth(3);
    let error = engine.render("a", &ctx()).unwrap_err();
    assert!(matches!(
        error.kind(),
        ErrorKind::IncludeDepthExceeded { limit: 3 }
    ));

    // Errors inside an included template point into that template.
    let error = engine.render("uses_broken", &ctx()).unwrap_err();
    assert_eq!(error.template(), Some("broken"));
    assert_eq!(at(&error), (2, 4));

    // Looking up a missing template by name has no position.
    let error = engine.render("missing", &ctx()).unwrap_err();
    assert!(matches!(error.kind(), ErrorKind::TemplateNotFound { .. }));
    assert_eq!(error.position(), None);
}

#[test]
fn extends_and_blocks() {
    let mut engine = Engine::new();
    engine
        .add_template(
            "base",
            "<h1>{% block title %}Base{% endblock %}</h1>{% block body %}{% endblock %}",
        )
        .unwrap();
    engine
        .add_template(
            "child",
            "{% extends \"base\" %}ignored {{ nope }}{% block title %}Child{% endblock %}\
             {% block body %}<p>{{ x }}</p>{% endblock %}",
        )
        .unwrap();
    engine
        .add_template(
            "partial",
            "{% extends \"base\" %}{% block body %}only body{% endblock %}",
        )
        .unwrap();
    let context = ctx().with("x", 1);
    assert_eq!(
        engine.render("child", &context).unwrap(),
        "<h1>Child</h1><p>1</p>"
    );
    // A block that is not overridden keeps the base content.
    assert_eq!(
        engine.render("partial", &context).unwrap(),
        "<h1>Base</h1>only body"
    );
    // A block in a template that does not extend renders in place.
    assert_eq!(
        engine
            .render_str("a{% block b %}B{% endblock b %}c", &context)
            .unwrap(),
        "aBc"
    );
}

#[test]
fn multilevel_inheritance_and_super() {
    let mut engine = Engine::new();
    engine
        .add_template("grand", "[{% block c %}grand{% endblock %}]")
        .unwrap();
    engine
        .add_template(
            "parent",
            "{% extends \"grand\" %}{% block c %}parent+{{ super() }}{% endblock %}",
        )
        .unwrap();
    engine
        .add_template(
            "kid",
            "{% extends \"parent\" %}{% block c %}kid+{{ super() }}{% endblock %}",
        )
        .unwrap();
    assert_eq!(engine.render("kid", &ctx()).unwrap(), "[kid+parent+grand]");
    assert_eq!(engine.render("parent", &ctx()).unwrap(), "[parent+grand]");
    // `super()` output is trusted markup and is not escaped again.
    engine
        .add_template("raw_base", "{% block c %}<b>{% endblock %}")
        .unwrap();
    engine
        .add_template(
            "raw_kid",
            "{% extends \"raw_base\" %}{% block c %}{{ super() }}!{% endblock %}",
        )
        .unwrap();
    assert_eq!(engine.render("raw_kid", &ctx()).unwrap(), "<b>!");
    let error = engine.render_str("{{ super() }}", &ctx()).unwrap_err();
    assert!(matches!(error.kind(), ErrorKind::InvalidOperation { .. }));
}

#[test]
fn inheritance_errors() {
    let mut engine = Engine::new();
    engine.add_template("a", "{% extends \"b\" %}").unwrap();
    engine.add_template("b", "{% extends \"a\" %}").unwrap();
    engine
        .add_template("orphan", "{% extends \"nowhere\" %}")
        .unwrap();
    let error = engine.render("a", &ctx()).unwrap_err();
    assert!(
        matches!(error.kind(), ErrorKind::IncludeDepthExceeded { .. }),
        "{error}"
    );
    let error = engine.render("orphan", &ctx()).unwrap_err();
    assert!(matches!(error.kind(), ErrorKind::TemplateNotFound { name } if name == "nowhere"));
    assert_eq!(at(&error), (1, 12));

    let error = engine
        .add_template("late", "text{% extends \"a\" %}")
        .unwrap_err();
    assert!(matches!(error.kind(), ErrorKind::ExtendsNotFirst));
    assert_eq!((error.template(), at(&error)), (Some("late"), (1, 5)));
    assert!(engine
        .add_template("ok", "  {# note #}\n{% extends \"a\" %}")
        .is_ok());
    let error = engine
        .add_template("twice", "{% extends \"a\" %}{% extends \"b\" %}")
        .unwrap_err();
    assert!(matches!(error.kind(), ErrorKind::ExtendsNotFirst));
    let error = engine
        .add_template(
            "dup",
            "{% block x %}{% endblock %}{% block x %}{% endblock %}",
        )
        .unwrap_err();
    assert!(matches!(error.kind(), ErrorKind::DuplicateBlock { name } if name == "x"));
    assert_eq!(at(&error), (1, 37));
}

// -- error positions ---------------------------------------------------------------

/// A source, a check of the error kind and the expected (line, column).
type Case = (&'static str, fn(&ErrorKind) -> bool, (u32, u32));

#[test]
fn syntax_error_positions() {
    let cases: Vec<Case> = vec![
        (
            "{{ name",
            |k| matches!(k, ErrorKind::UnclosedTag { open: "{{" }),
            (1, 1),
        ),
        (
            "ab {% if x",
            |k| matches!(k, ErrorKind::UnclosedTag { open: "{%" }),
            (1, 4),
        ),
        (
            "x\n{# never",
            |k| matches!(k, ErrorKind::UnclosedTag { open: "{#" }),
            (2, 1),
        ),
        (
            "ab\n{% if x %}\nbody",
            |k| matches!(k, ErrorKind::UnclosedBlock { tag: "if" }),
            (2, 1),
        ),
        (
            "{% for x in y %}{% if z %}",
            |k| matches!(k, ErrorKind::UnclosedBlock { tag: "if" }),
            (1, 17),
        ),
        (
            "{% endif %}",
            |k| matches!(k, ErrorKind::UnexpectedTag { name } if name == "endif"),
            (1, 1),
        ),
        (
            "{% else %}",
            |k| matches!(k, ErrorKind::UnexpectedTag { .. }),
            (1, 1),
        ),
        (
            "{% if a %}{% endfor %}",
            |k| matches!(k, ErrorKind::MismatchedEndTag { expected: "endif", found } if found == "endfor"),
            (1, 11),
        ),
        (
            "{% foo %}",
            |k| matches!(k, ErrorKind::UnknownTag { name } if name == "foo"),
            (1, 4),
        ),
        (
            "{{ 1 + }}",
            |k| matches!(k, ErrorKind::UnexpectedToken { .. }),
            (1, 8),
        ),
        (
            "line1\nline2 {{ a | }}",
            |k| {
                matches!(
                    k,
                    ErrorKind::UnexpectedToken {
                        expected: "a filter name",
                        ..
                    }
                )
            },
            (2, 14),
        ),
        (
            "{{ \"abc }}",
            |k| matches!(k, ErrorKind::UnterminatedString),
            (1, 4),
        ),
        (
            "{{ 99999999999999999999 }}",
            |k| matches!(k, ErrorKind::InvalidNumber),
            (1, 4),
        ),
        (
            "{{ a $ b }}",
            |k| matches!(k, ErrorKind::UnexpectedToken { .. }),
            (1, 6),
        ),
        (
            "{% include name %}",
            |k| {
                matches!(
                    k,
                    ErrorKind::UnexpectedToken {
                        expected: "a template name",
                        ..
                    }
                )
            },
            (1, 12),
        ),
        (
            "{% if %}",
            |k| matches!(k, ErrorKind::UnexpectedToken { .. }),
            (1, 7),
        ),
        (
            "{% for x of y %}",
            |k| {
                matches!(
                    k,
                    ErrorKind::UnexpectedToken {
                        expected: "`in`",
                        ..
                    }
                )
            },
            (1, 10),
        ),
        (
            "{{ a b }}",
            |k| {
                matches!(
                    k,
                    ErrorKind::UnexpectedToken {
                        expected: "`}}`",
                        ..
                    }
                )
            },
            (1, 6),
        ),
        (
            "{% raw %}never closed",
            |k| matches!(k, ErrorKind::UnclosedBlock { tag: "raw" }),
            (1, 1),
        ),
        (
            "é{% bogus %}",
            |k| matches!(k, ErrorKind::UnknownTag { .. }),
            (1, 5),
        ),
    ];
    for (source, matches_kind, expected) in cases {
        let error = Template::new(source).expect_err(source);
        assert!(matches_kind(error.kind()), "{source:?}: {error:?}");
        assert_eq!(at(&error), expected, "{source:?}: {error}");
    }
}

#[test]
fn runtime_error_positions_and_display() {
    let error = fail("Hello\n  {{ user.name }}", &ctx());
    assert!(matches!(error.kind(), ErrorKind::UndefinedVariable { name } if name == "user"));
    assert_eq!(at(&error), (2, 6));
    assert_eq!(
        error.to_string(),
        "undefined variable `user` at <string>:2:6"
    );

    // With the object present, the path names the missing part.
    let context = ctx().with("user", Value::from(BTreeMap::from([("id", 1)])));
    let error = fail("{{ user.name }}", &context);
    assert!(matches!(error.kind(), ErrorKind::UndefinedVariable { name } if name == "user.name"));
    assert_eq!(at(&error), (1, 4));
    let list = ctx().with("xs", vec![1]);
    assert!(
        matches!(fail("{{ xs.5 }}", &list).kind(), ErrorKind::UndefinedVariable { name } if name == "xs.5")
    );

    // Columns count characters; CRLF is one newline.
    assert_eq!(at(&fail("é{{ x }}", &ctx())), (1, 5));
    assert_eq!(at(&fail("a\r\n{{ x }}", &ctx())), (2, 4));
    assert_eq!(at(&fail("日本語 {{ x }}", &ctx())), (1, 8));

    let mut engine = Engine::new();
    let error = engine
        .add_template("page.html", "<p>\n{{ 1 + }}")
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "expected an expression, found `}}` at page.html:2:8"
    );
    engine
        .add_template("page.html", "<p>\n{{ user.name }}")
        .unwrap();
    let error = engine.render("page.html", &ctx()).unwrap_err();
    assert_eq!(
        error.to_string(),
        "undefined variable `user` at page.html:2:4"
    );
    assert_eq!(error.template(), Some("page.html"));
    let _: &dyn std::error::Error = &error;
}

// -- raw, API and conversions ------------------------------------------------------

#[test]
fn raw_blocks_print_delimiters_literally() {
    assert_eq!(
        run("{% raw %}{{ x }} {% if %}{% endraw %}", &ctx()),
        "{{ x }} {% if %}"
    );
    assert_eq!(
        run("a {%- raw -%}  {{ x }}  {%- endraw -%} b", &ctx()),
        "a{{ x }}b"
    );
}

#[test]
fn free_functions_and_standalone_templates() {
    let template = Template::new("{{ a }}-{{ b }}").unwrap();
    let context = ctx().with("a", 1).with("b", "<2>");
    assert_eq!(render(&template, &context).unwrap(), "1-&lt;2&gt;");
    assert_eq!(template.render(&context).unwrap(), "1-&lt;2&gt;");
    assert_eq!(template.name(), None);
    assert_eq!(Template::named("t", "x").unwrap().name(), Some("t"));
    // Without an engine nothing can be included.
    let error = render_str("{% include \"x\" %}", &ctx()).unwrap_err();
    assert!(matches!(error.kind(), ErrorKind::TemplateNotFound { .. }));
}

#[test]
fn engine_management() {
    let mut engine = Engine::new();
    assert!(!engine.has_template("a"));
    engine.add_template("a", "A").unwrap();
    assert!(engine.has_template("a"));
    engine.add_template("a", "A2").unwrap();
    assert_eq!(engine.render("a", &ctx()).unwrap(), "A2");
    assert!(engine.remove_template("a"));
    assert!(!engine.remove_template("a"));
    engine.add_filter("f", |v, _| Ok(v.clone()));
    let debug = format!("{engine:?}");
    assert!(
        debug.contains("Engine") && debug.contains("\"f\""),
        "{debug}"
    );
    // Templates can refer to each other in any registration order.
    engine
        .add_template("outer", "{% include \"inner\" %}")
        .unwrap();
    engine.add_template("inner", "in").unwrap();
    assert_eq!(engine.render("outer", &ctx()).unwrap(), "in");
}

#[test]
fn context_ergonomics() {
    let mut context = Context::new();
    context
        .insert("name", "Ada")
        .insert("age", 36)
        .insert("ok", true);
    let context = context
        .with("score", 1.5)
        .with("tags", vec!["a", "b"])
        .with("none", Value::Null);
    assert_eq!(context.len(), 6);
    assert!(!context.is_empty() && context.contains_key("age"));
    assert_eq!(context.get("age").and_then(Value::as_i64), Some(36));
    assert_eq!(
        run(
            "{{ name }} {{ age }} {{ ok }} {{ score }} {{ tags | join('+') }}",
            &context
        ),
        "Ada 36 true 1.5 a+b"
    );

    let mut other = Context::new().with("name", "Bob");
    other.extend(Context::new().with("extra", 1));
    assert_eq!(other.len(), 2);
    assert_eq!(other.remove("extra"), Some(Value::Int(1)));
    let collected: Context = [("a", 1), ("b", 2)].into_iter().collect();
    assert_eq!(
        collected.iter().map(|(k, _)| k).collect::<Vec<_>>(),
        ["a", "b"]
    );
    let nested = Context::new().with("inner", collected);
    assert_eq!(run("{{ inner.a }}{{ inner.b }}", &nested), "12");
}

#[test]
fn json_values_become_contexts() {
    let document = json::parse(
        r#"{"user":{"name":"Ada"},"items":[1,2],"big":18446744073709551615,"n":null,"f":1.5,"neg":-3}"#,
    )
    .unwrap();
    let context = Context::from(&document);
    assert_eq!(
        run(
            "{{ user.name }}|{{ items | len }}|[{{ n }}]|{{ f }}|{{ neg }}",
            &context
        ),
        "Ada|2|[]|1.5|-3"
    );
    // Integers beyond i64 become floats; nothing is lost silently as text.
    assert!(matches!(context.get("big"), Some(Value::Float(_))));
    assert_eq!(Context::from(document).len(), 6);
    assert!(Context::from(json::parse("[1,2]").unwrap()).is_empty());
    assert!(Context::from(json::parse("\"text\"").unwrap()).is_empty());

    let value = Value::from(json::parse(r#"[1,"a",null,{"k":true}]"#).unwrap());
    assert_eq!(value.as_list().map(Vec::len), Some(4));
    let back = json::Value::from(value.clone());
    assert_eq!(json::to_string(&back), r#"[1,"a",null,{"k":true}]"#);
    assert_eq!(value.to_string(), r#"[1,"a",null,{"k":true}]"#);
    assert_eq!(json::Value::from(Value::Float(f64::NAN)), json::Value::Null);
}

#[test]
fn deeply_nested_json_is_cut_off_instead_of_overflowing() {
    let mut document = json::Value::Null;
    for _ in 0..50_000 {
        document = json::Value::Array(vec![document]);
    }
    let by_ref = Value::from(&document);
    let converted = on_small_stack(move || Value::from(document));
    for value in [by_ref, converted] {
        let (mut depth, mut current) = (0, &value);
        while let Value::List(items) = current {
            depth += 1;
            current = &items[0];
        }
        assert!((100..=130).contains(&depth), "{depth}");
        assert_eq!(current, &Value::Null);
        // Going back to JSON is bounded the same way.
        let back = on_small_stack(move || json::Value::from(value));
        assert!(back.is_array());
    }
}

#[test]
fn value_conversions_and_helpers() {
    assert_eq!(Value::from(1u8), Value::Int(1));
    assert_eq!(Value::from(u64::MAX), Value::Float(u64::MAX as f64));
    assert_eq!(Value::from(Some("a")), Value::Str("a".into()));
    assert_eq!(Value::from(None::<i32>), Value::Null);
    assert_eq!(Value::from(()), Value::Null);
    assert_eq!(Value::from(Html::new("<i>")), Value::Safe("<i>".into()));
    assert_eq!(Value::default(), Value::Null);
    assert_eq!(Value::from(1.5f32).as_f64(), Some(1.5));
    assert_eq!(Value::from("a").as_str(), Some("a"));
    assert_eq!(Value::safe("a").as_str(), Some("a"));
    assert_eq!(Value::from(true).as_bool(), Some(true));
    assert_eq!(Value::from(3).type_name(), "integer");
    assert!(Value::Null.is_null() && !Value::from(0).is_null());
    assert_eq!(Value::from(vec![1, 2]).to_string(), "[1,2]");
    assert_eq!(Value::Float(2.0).to_string(), "2");
    let map = Value::from(std::collections::HashMap::from([("k", 1)]));
    assert_eq!(map.as_map().and_then(|m| m.get("k")), Some(&Value::Int(1)));
}

// -- limits, SSTI and hostile input ---------------------------------------------------

#[test]
fn nesting_limits_stop_bombs_without_overflowing_the_stack() {
    let outcome = on_small_stack(|| {
        let tags = "{% if true %}".repeat(10_000);
        let error = Template::new(&tags).unwrap_err();
        assert!(matches!(
            error.kind(),
            ErrorKind::NestingTooDeep { limit: 32 }
        ));
        assert_eq!(
            (
                error.position().unwrap().line,
                error.position().unwrap().column
            ),
            (1, 417)
        );

        for open in ["(", "[", "-", "not "] {
            let source = format!("{{{{ {}1 }}}}", open.repeat(10_000));
            let error = Template::new(&source).unwrap_err();
            assert!(
                matches!(error.kind(), ErrorKind::NestingTooDeep { .. }),
                "{open}"
            );
        }
        for chain in [
            ".a".repeat(10_000),
            "|upper".repeat(10_000),
            "+1".repeat(10_000),
            "[0]".repeat(10_000),
            " ~ x".repeat(10_000),
        ] {
            let error = Template::new(&format!("{{{{ x{chain} }}}}")).unwrap_err();
            assert!(
                matches!(error.kind(), ErrorKind::NestingTooDeep { .. }),
                "{}",
                &chain[..4]
            );
        }
        let mut engine = Engine::new();
        engine.set_max_nesting_depth(3);
        let error = engine
            .add_template("t", "{% if a %}{% if b %}{% if c %}{% if d %}")
            .unwrap_err();
        assert!(matches!(
            error.kind(),
            ErrorKind::NestingTooDeep { limit: 3 }
        ));
        true
    });
    assert!(outcome);
}

#[test]
fn worst_case_legal_nesting_fits_a_default_thread_stack() {
    // 15 includes deep, each level 31 tags deep: the most the default limits
    // allow, rendered on a thread with the standard 2 MiB stack.
    let output = on_stack(2048, || {
        let mut engine = Engine::new();
        let level = format!(
            "{}x{}",
            "{% if true %}".repeat(31),
            "{% endif %}".repeat(31)
        );
        engine.add_template("leaf", &level).unwrap();
        for depth in 0..14 {
            let inner = if depth == 0 {
                "leaf".to_owned()
            } else {
                format!("l{}", depth - 1)
            };
            let source = format!(
                "{}{{% include \"{inner}\" %}}{}",
                "{% if true %}".repeat(31),
                "{% endif %}".repeat(31)
            );
            engine.add_template(format!("l{depth}"), &source).unwrap();
        }
        engine.render("l13", &Context::new()).unwrap()
    });
    assert_eq!(output, "x");
}

#[test]
fn output_loop_and_include_limits() {
    let mut engine = Engine::new();
    engine.set_max_output_len(100);
    let error = engine
        .render_str(
            "{% for i in xs %}0123456789{% endfor %}",
            &ctx().with("xs", vec![0; 50]),
        )
        .unwrap_err();
    assert!(matches!(
        error.kind(),
        ErrorKind::OutputTooLarge { limit: 100 }
    ));
    assert!(engine.render_str(&"x".repeat(100), &ctx()).is_ok());
    assert!(engine.render_str(&"x".repeat(101), &ctx()).is_err());
    // Intermediate strings are limited too.
    let big = ctx().with("s", "a".repeat(60));
    assert!(matches!(
        engine
            .render_str("{{ (s ~ s) | len }}", &big)
            .unwrap_err()
            .kind(),
        ErrorKind::OutputTooLarge { .. }
    ));
    assert!(matches!(
        engine
            .render_str("{{ s | replace('a', 'bbb') | len }}", &big)
            .unwrap_err()
            .kind(),
        ErrorKind::OutputTooLarge { .. }
    ));

    let mut engine = Engine::new();
    engine.set_max_loop_iterations(1_000);
    let hundred = ctx().with("xs", vec![0; 100]);
    // Nested loops are counted together: 100 + 100*100 iterations.
    let error = engine
        .render_str(
            "{% for a in xs %}{% for b in xs %}{% endfor %}{% endfor %}",
            &hundred,
        )
        .unwrap_err();
    assert!(matches!(
        error.kind(),
        ErrorKind::LoopLimitExceeded { limit: 1_000 }
    ));
    assert_eq!(at(&error), (1, 30));
    assert!(engine
        .render_str("{% for a in xs %}{% endfor %}", &hundred)
        .is_ok());
}

#[test]
fn include_fan_out_cannot_explode() {
    let mut engine = Engine::new();
    // Ten includes per level and sixteen levels deep would be 10^16 renders.
    for level in 0..15 {
        let next = format!("l{}", level + 1);
        engine
            .add_template(
                format!("l{level}"),
                &"{% include \"NEXT\" %}".replace("NEXT", &next).repeat(10),
            )
            .unwrap();
    }
    engine.add_template("l15", "x").unwrap();
    let started = Instant::now();
    let error = engine.render("l0", &ctx()).unwrap_err();
    assert!(
        matches!(
            error.kind(),
            ErrorKind::IncludeLimitExceeded { limit: 10_000 }
        ),
        "{error}"
    );
    assert!(started.elapsed() < Duration::from_secs(20));
}

#[test]
fn template_syntax_in_data_is_never_evaluated() {
    let context = ctx()
        .with("evil", "{{ 7 * 7 }} {% include \"secret\" %} {# c #}")
        .with("name", "{{ nope }}");
    let mut engine = Engine::new();
    engine.add_template("secret", "TOP SECRET").unwrap();
    let output = engine
        .render_str("{{ evil }} | {{ name | upper }}", &context)
        .unwrap();
    assert!(
        !output.contains("49") && !output.contains("TOP SECRET"),
        "{output}"
    );
    assert!(output.contains("{{ 7 * 7 }}"));
    // Safe text is printed literally too: it is output, never source.
    let safe = ctx().with("m", Value::safe("{{ 7 * 7 }}"));
    assert_eq!(run("{{ m }}", &safe), "{{ 7 * 7 }}");
}

#[test]
fn templates_cannot_reach_internals_or_the_filesystem() {
    let context = ctx().with("user", "ada");
    for source in [
        "{{ self }}",
        "{{ __class__ }}",
        "{{ ''.__class__ }}",
        "{{ user.__class__.__mro__ }}",
        "{{ config }}",
        "{{ request.environ }}",
        "{{ lipsum.__globals__ }}",
        "{{ cycler.__init__ }}",
        "{{ user | attr('__class__') }}",
        "{{ user | system('id') }}",
        "{{ range(10) }}",
        "{{ open('/etc/passwd') }}",
        "{% set x = 1 %}",
        "{% import 'os' as os %}",
        "{% macro m() %}{% endmacro %}",
        "{% include name %}",
        "{% include \"../../etc/passwd\" %}",
        "{% include \"/etc/passwd\" %}",
        "{% include \"file:///etc/passwd\" %}",
        "{% extends \"../../etc/passwd\" %}",
        "{{ user.upper() }}",
        "{{ 1 if true else 2 }}",
        "{{ {'a': 1} }}",
        "{{ (1).real }}",
    ] {
        match render_str(source, &context) {
            Err(_) => {}
            Ok(output) => assert!(
                output.is_empty() || (!output.contains("class") && !output.contains("root")),
                "{source} rendered {output:?}"
            ),
        }
    }
    // Attribute access on text or numbers finds nothing.
    assert!(matches!(
        fail("{{ user.__class__ }}", &context).kind(),
        ErrorKind::UndefinedVariable { .. }
    ));
    assert!(matches!(
        fail("{{ 'x'.y }}", &context).kind(),
        ErrorKind::UndefinedVariable { .. }
    ));
}

#[test]
fn hostile_templates_stay_linear() {
    let cases: Vec<(&str, String)> = vec![
        ("long text", "x".repeat(5_000_000)),
        ("many variables", "{{ a }}".repeat(100_000)),
        ("many comments", "{# c #}".repeat(100_000)),
        ("many raw", "{% raw %}{{ x }}{% endraw %}".repeat(50_000)),
        ("trims", "  {{- a -}}  ".repeat(100_000)),
        ("unclosed openers", "{{ ".repeat(100_000)),
        ("brace noise", "{".repeat(500_000)),
        ("percent noise", "{%".repeat(100_000)),
        ("hash noise", "{#".repeat(100_000)),
        ("strings", "{{ 'a' }}".repeat(100_000)),
        ("flat ifs", "{% if a %}x{% endif %}".repeat(50_000)),
        (
            "flat fors",
            "{% for i in xs %}{{ i }}{% endfor %}".repeat(10_000),
        ),
        (
            "long expression",
            format!("{{{{ {} }}}}", ["1"; 20].join(" + ")),
        ),
    ];
    let context = ctx().with("a", 1).with("xs", vec![1, 2, 3]);
    for (name, source) in cases {
        let context = context.clone();
        let started = Instant::now();
        let _ = on_small_stack(move || render_str(&source, &context).map(|out| out.len()));
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "{name}: {:?}",
            started.elapsed()
        );
    }
}

#[test]
fn engine_is_send_sync_and_shareable() {
    fn assert_send_sync<T: Send + Sync + 'static>() {}
    assert_send_sync::<Engine>();
    assert_send_sync::<Template>();
    assert_send_sync::<Context>();
    assert_send_sync::<Value>();
    assert_send_sync::<Error>();

    let mut engine = Engine::new();
    engine
        .add_template("t", "{{ n }}:{% for i in xs %}{{ i }}{% endfor %}")
        .unwrap();
    let engine = std::sync::Arc::new(engine);
    let handles: Vec<_> = (0..4)
        .map(|n| {
            let engine = std::sync::Arc::clone(&engine);
            thread::spawn(move || {
                let context = Context::new().with("n", n).with("xs", vec![1, 2, 3]);
                engine.render("t", &context).unwrap()
            })
        })
        .collect();
    let outputs: Vec<String> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(outputs, ["0:123", "1:123", "2:123", "3:123"]);
}

#[test]
fn unicode_templates() {
    let context = ctx().with("名前", "Ada").with("é", "<é>");
    assert_eq!(
        run("こんにちは {{ 名前 }}! {{ é }} ✓", &context),
        "こんにちは Ada! &lt;é&gt; ✓"
    );
    assert_eq!(
        run("{% for ü in [1, 2] %}{{ ü }}{% endfor %}", &ctx()),
        "12"
    );
}

#[test]
fn hostile_fuzz_lite_never_panics_and_errors_have_positions() {
    let fragments = [
        "{{",
        "}}",
        "{%",
        "%}",
        "{#",
        "#}",
        "{{-",
        "-}}",
        "{%-",
        "-%}",
        " if ",
        " for ",
        " in ",
        " endif ",
        " endfor ",
        " else ",
        " elif ",
        " block ",
        " endblock ",
        " extends ",
        " include ",
        " raw ",
        " endraw ",
        "x",
        "x.y",
        "xs",
        "m",
        "1",
        "1.5",
        "'",
        "\"",
        "'a'",
        "\"b\"",
        "|",
        " | upper",
        " | join(',')",
        " | default(1)",
        "(",
        ")",
        "[",
        "]",
        ".",
        ",",
        "+",
        "-",
        "*",
        "/",
        "%",
        "~",
        "==",
        "!=",
        "<",
        ">",
        "<=",
        " not ",
        " and ",
        " or ",
        "é",
        "日本",
        "😀",
        "\n",
        "\r\n",
        " ",
        "\t",
        "super()",
        "loop.index",
        "loop",
        "\"a\"",
        "\"b\"",
        "\\",
        "none",
        "true",
        "{",
        "}",
        "%",
    ];
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let mut engine = Engine::new();
    engine.set_max_loop_iterations(2_000);
    engine.set_max_output_len(1 << 20);
    engine.set_max_include_calls(200);
    engine
        .add_template("a", "A{% block b %}-a-{% endblock %}{{ x.y }}")
        .unwrap();
    engine
        .add_template(
            "b",
            "{% extends \"a\" %}{% block b %}[{{ super() }}]{% endblock %}",
        )
        .unwrap();
    let context = ctx()
        .with("x", Value::from(BTreeMap::from([("y", "<y>")])))
        .with("xs", vec![1, 2, 3])
        .with("m", Value::from(BTreeMap::from([("k", 1)])));
    let mut rendered = 0;
    for _ in 0..6_000 {
        let pieces = (next() % 24) as usize;
        let source: String = (0..pieces)
            .map(|_| fragments[(next() % fragments.len() as u64) as usize])
            .collect();
        match engine.render_str(&source, &context) {
            Ok(output) => {
                rendered += 1;
                // Escaped data is never turned back into markup.
                assert!(
                    !output.contains("<y>") || source.contains("safe"),
                    "{source:?}"
                );
            }
            Err(error) => {
                let position = error.position().expect("anonymous sources have positions");
                assert!(
                    position.line >= 1 && position.column >= 1,
                    "{source:?}: {error}"
                );
                assert!(
                    position.line as usize <= source.matches('\n').count() + 1,
                    "{source:?}: {error}"
                );
                let _ = error.to_string();
            }
        }
    }
    assert!(
        rendered > 100,
        "the fuzzer should also produce valid templates"
    );
}

// -- regression: verified audit findings ------------------------------------

#[test]
fn raw_block_with_bare_percent_brace_does_not_panic() {
    // `{%}` used to be split as `{%` + `%}` sharing the `%`, slicing
    // `source[start + 2..close]` with `start + 2 > close`.
    for source in [
        "{% raw %}{%}",
        "{% raw %}{%}{% endraw %}",
        "{% raw %}{%-}",
        "{% raw %}{%-%}",
        "{% raw %}{%%}",
        "{% raw -%}{%}",
    ] {
        let _ = render_str(source, &ctx());
    }
    let error = fail("{% raw %}{%}", &ctx());
    assert!(matches!(
        error.kind(),
        ErrorKind::UnclosedBlock { tag: "raw" }
    ));
    // A `{%}` inside a closed raw body stays literal text.
    assert_eq!(run("{% raw %}a{%}b{% endraw %}", &ctx()), "a{%}b");
    assert_eq!(run("{% raw %}{%-%}{% endraw %}", &ctx()), "{%-%}");
}

#[test]
fn exhaustive_short_garbage_never_panics() {
    const PIECES: &[&str] = &[
        "{", "}", "%", "#", "-", "raw", "endraw", "{{", "{%", "%}", "\"", "'", " ",
    ];
    let mut checked = 0usize;
    let mut stack: Vec<Vec<usize>> = vec![Vec::new()];
    while let Some(combo) = stack.pop() {
        let source: String = combo.iter().map(|&index| PIECES[index]).collect();
        let outcome = std::panic::catch_unwind(|| {
            let _ = render_str(&source, &Context::new());
            let mut wrapped = String::from("{% raw %}");
            wrapped.push_str(&source);
            let _ = render_str(&wrapped, &Context::new());
        });
        assert!(outcome.is_ok(), "panicked on {source:?}");
        checked += 1;
        if combo.len() < 4 {
            for index in 0..PIECES.len() {
                let mut next = combo.clone();
                next.push(index);
                stack.push(next);
            }
        }
    }
    assert!(checked > 30_000);
}

#[test]
fn replace_amplification_fails_before_allocating() {
    // 1 MiB of `a` with each `a` replaced by 10 KiB would be ~10 GiB; the
    // result length is computed first, so this fails immediately.
    let context = ctx()
        .with("s", "a".repeat(1024 * 1024))
        .with("r", "b".repeat(10 * 1024));
    let started = Instant::now();
    let error = fail("{{ s | replace('a', r) | len }}", &context);
    assert!(matches!(error.kind(), ErrorKind::OutputTooLarge { .. }));
    assert!(started.elapsed() < Duration::from_secs(10));
    // Shrinking and exactly-at-the-limit replacements still work.
    let mut engine = Engine::new();
    engine.set_max_output_len(12);
    let six = ctx().with("s", "aaaaaa");
    assert_eq!(
        engine
            .render_str("{{ s | replace('a', 'bb') }}", &six)
            .unwrap(),
        "bbbbbbbbbbbb"
    );
    assert!(matches!(
        engine
            .render_str("{{ s | replace('a', 'bbb') | len }}", &six)
            .unwrap_err()
            .kind(),
        ErrorKind::OutputTooLarge { limit: 12 }
    ));
    assert_eq!(
        engine
            .render_str("{{ s | replace('aa', '') }}|", &six)
            .unwrap(),
        "|"
    );
}

#[test]
fn join_and_escape_amplification_are_capped() {
    let context = ctx()
        .with("xs", vec![0; 10_000])
        .with("sep", "x".repeat(1024 * 1024));
    let error = fail("{{ xs | join(sep) | len }}", &context);
    assert!(matches!(error.kind(), ErrorKind::OutputTooLarge { .. }));

    let mut engine = Engine::new();
    engine.set_max_output_len(10);
    let big = ctx().with("s", "<".repeat(11));
    for source in [
        "{{ s | e | len }}",
        "{{ s | upper | len }}",
        "{{ s | lower | len }}",
        "{{ s | json | len }}",
    ] {
        assert!(
            matches!(
                engine.render_str(source, &big).unwrap_err().kind(),
                ErrorKind::OutputTooLarge { limit: 10 }
            ),
            "{source}"
        );
    }
    assert_eq!(
        engine
            .render_str("{{ xs | join(',') }}", &ctx().with("xs", vec![1, 2, 3]))
            .unwrap(),
        "1,2,3"
    );
}

#[test]
fn super_block_cycles_are_bounded_instead_of_overflowing() {
    // base `a` contains `b`; the child's `b` contains `a`, whose `super()`
    // renders base `a` again -> `b` -> `a` -> ... forever without a guard.
    let error = on_small_stack(|| {
        let mut engine = Engine::new();
        engine
            .add_template(
                "base",
                "{% block a %}{% block b %}{% endblock %}{% endblock %}",
            )
            .unwrap();
        engine
            .add_template(
                "kid",
                "{% extends \"base\" %}{% block b %}{% block a %}{{ super() }}{% endblock %}{% endblock %}",
            )
            .unwrap();
        engine.render("kid", &Context::new()).unwrap_err()
    });
    assert!(matches!(error.kind(), ErrorKind::NestingTooDeep { .. }));

    // Self-including and mutually extending templates fail cleanly too.
    let (include, extends) = on_small_stack(|| {
        let mut engine = Engine::new();
        engine.add_template("me", "x{% include \"me\" %}").unwrap();
        engine.add_template("p", "{% extends \"q\" %}").unwrap();
        engine.add_template("q", "{% extends \"p\" %}").unwrap();
        (
            engine.render("me", &Context::new()).unwrap_err(),
            engine.render("p", &Context::new()).unwrap_err(),
        )
    });
    assert!(matches!(
        include.kind(),
        ErrorKind::IncludeDepthExceeded { .. }
    ));
    assert!(matches!(
        extends.kind(),
        ErrorKind::IncludeDepthExceeded { .. }
    ));
}

#[test]
fn nested_list_literal_copies_are_budgeted() {
    // Every level copies the loop variable twice: 2^30 values without a
    // budget. The copy budget (16x the output limit) stops it early.
    const LEVELS: usize = 30;
    let mut source = String::from("{% for v0 in [[\"x\", \"x\"]] %}");
    for level in 1..LEVELS {
        let prev = level - 1;
        source.push_str(&format!("{{% for v{level} in [[v{prev}, v{prev}]] %}}"));
    }
    source.push_str("ok");
    for _ in 0..LEVELS {
        source.push_str("{% endfor %}");
    }
    let mut engine = Engine::new();
    engine.set_max_nesting_depth(128);
    let started = Instant::now();
    let error = on_small_stack(move || engine.render_str(&source, &Context::new()).unwrap_err());
    assert!(
        matches!(error.kind(), ErrorKind::CopyLimitExceeded { .. }),
        "{error}"
    );
    assert!(started.elapsed() < Duration::from_secs(20));

    // A small limit makes the budget small too, and filter arguments count.
    let mut engine = Engine::new();
    engine.set_max_output_len(64);
    let big = ctx().with("s", "a".repeat(2048));
    for source in ["{{ [s] | len }}", "{{ none | default(s) | len }}"] {
        assert!(
            matches!(
                engine.render_str(source, &big).unwrap_err().kind(),
                ErrorKind::CopyLimitExceeded { limit: 1024 }
            ),
            "{source}"
        );
    }
    // Ordinary literals and loops over borrowed data are unaffected.
    let xs = ctx().with("xs", vec![1, 2, 3]).with("s", "abc");
    assert_eq!(
        run(
            "{% for x in xs %}{% for y in [x, s] %}{{ y }}{% endfor %}{% endfor %}",
            &xs
        ),
        "1abc2abc3abc"
    );
    assert_eq!(
        run("{% for x in [[1, 2]] %}{{ x | len }}{% endfor %}", &xs),
        "2"
    );
}

#[test]
fn amplifying_filters_stop_at_the_output_limit() {
    let mut engine = Engine::new();
    engine.set_max_output_len(10);
    // The inputs fit the limit, but the outputs would not.
    let cases = [
        ("{{ s | e | len }}", "\"\"\""),
        ("{{ s | upper | len }}", "\u{fb03}\u{fb03}\u{fb03}\u{fb03}"),
        ("{{ s | json | len }}", "\u{1}\u{1}\u{1}"),
        ("{{ s | json | len }}", "\u{2028}\u{2028}"),
        ("{{ s | json | len }}", "<<<"),
    ];
    for (source, input) in cases {
        let context = ctx().with("s", input);
        assert!(
            matches!(
                engine.render_str(source, &context).unwrap_err().kind(),
                ErrorKind::OutputTooLarge { limit: 10 }
            ),
            "{source} {input:?}"
        );
    }
    // Lists and maps are bounded while encoding, too.
    let list = ctx().with("xs", vec![1000; 1000]);
    assert!(matches!(
        engine
            .render_str("{{ xs | json }}", &list)
            .unwrap_err()
            .kind(),
        ErrorKind::OutputTooLarge { limit: 10 }
    ));
    // Results exactly at the limit still work.
    assert_eq!(
        engine
            .render_str("{{ s | e }}", &ctx().with("s", "<>"))
            .unwrap(),
        "&lt;&gt;"
    );
    assert_eq!(
        engine
            .render_str("{{ s | json }}", &ctx().with("s", "\u{2028}"))
            .unwrap(),
        "\"\\u2028\""
    );
    assert_eq!(
        engine
            .render_str("{{ s | upper }}", &ctx().with("s", "\u{fb03}"))
            .unwrap(),
        "FFI"
    );
}
