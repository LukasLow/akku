//! A safe, auto-escaping template engine for HTML and plain text, with zero
//! dependencies.
//!
//! ```
//! use akku::template::{render_str, Context};
//!
//! let context = Context::new().with("user", "<Ada>").with("n", 3);
//! let html = render_str("<p>{{ user }} has {{ n }} new messages</p>", &context).unwrap();
//! assert_eq!(html, "<p>&lt;Ada&gt; has 3 new messages</p>");
//! ```
//!
//! # Syntax
//!
//! | Syntax | Meaning |
//! |---|---|
//! | `{{ expr }}` | print a value, escaped under [`AutoEscape::Html`] |
//! | `{{ expr \| filter(arg) }}` | apply filters (`upper lower trim len default escape safe join replace json`) |
//! | `{% if a %}…{% elif b %}…{% else %}…{% endif %}` | conditions |
//! | `{% for x in xs %}…{% else %}…{% endfor %}` | loops with `loop.index`, `loop.index0`, `loop.first`, `loop.last`, `loop.length`, `loop.revindex` |
//! | `{% for key, value in map %}` | maps iterate in key order, lists give `(index, item)` |
//! | `{% include "name" %}` | include a registered template (shares the scope) |
//! | `{% extends "base" %}` `{% block name %}…{% endblock %}` | inheritance, `{{ super() }}` |
//! | `{% raw %}…{% endraw %}` | print `{{` and `{%` literally |
//! | `{# comment #}` | nothing |
//! | `{{-`, `-}}`, `{%-`, `-%}` | trim the whitespace on that side |
//!
//! Expressions have literals (`1`, `1.5`, `'text'`, `true`, `none`, `[1, 2]`),
//! `a.b`, `a.0` and `a["k"]` lookups, `+ - * / %`, `~` (concatenation),
//! `== != < <= > >=`, `in`, `not in`, `and`, `or` and `not`.
//!
//! # Safe by default
//!
//! * Every printed [`Value::Str`] is HTML-escaped; only [`Value::Safe`] (made
//!   by the `safe` filter, [`Value::safe`] or [`Html`](crate::html::Html)) is
//!   printed as is. The result of a filter is plain text again unless the
//!   filter keeps markup safe (`safe`, `escape`, `json`, `trim`).
//! * Escaping is right for HTML text and for **quoted** attribute values. Always
//!   quote attributes (`href=\"{{ url }}\"`), use `| json` for data inside
//!   `<script>` and validate URLs before putting them in `href`; auto-escaping
//!   cannot make an unquoted attribute or a `javascript:` URL safe.
//! * A missing variable is an error, not an empty string, so typos surface.
//!   (`default` accepts one; `if` treats it as false.)
//! * Templates only reach what you register: there is no filesystem access,
//!   no code execution and no way to call into Rust from a template except
//!   through the filters you add.
//!
//! # Bounded by construction
//!
//! Templates are compiled to a tree and rendered by a fixed evaluator, with
//! limits on everything that could run away: nesting of tags and expressions
//! (32), chains of includes and `extends` (16), the number of includes (10,000
//! per render), loop iterations (1,000,000 per render) and output size
//! (8 MiB, also applied to every filter result). Exceeding one is an
//! [`ErrorKind`], never a hang or a stack overflow. Change them on the
//! [`Engine`].
//!
//! # Errors
//!
//! Every [`Error`] has a [`Position`] with the line and column in the
//! template source (columns count characters).
//!
//! ```
//! use akku::template::{Context, ErrorKind, Template};
//!
//! let error = Template::new("a\n{% if x %}\nbody").unwrap_err();
//! assert!(matches!(error.kind(), ErrorKind::UnclosedBlock { tag: "if" }));
//! assert_eq!(error.position().map(|p| (p.line, p.column)), Some((2, 1)));
//! ```
//!
//! # Data from JSON
//!
//! ```
//! use akku::json::json;
//! use akku::template::{render_str, Context};
//!
//! let context = Context::from(json!({ "user": { "name": "Ada" }, "tags": ["a", "b"] }));
//! let out = render_str("{{ user.name }}: {{ tags | join(', ') }}", &context).unwrap();
//! assert_eq!(out, "Ada: a, b");
//! ```

mod ast;
mod context;
mod engine;
mod error;
mod eval;
mod filters;
mod lexer;
mod parser;
mod value;

pub use self::context::Context;
pub use self::engine::{render, render_str, AutoEscape, Engine, FilterFn, Template};
pub use self::error::{Error, ErrorKind, Position, Result};
pub use self::value::Value;
