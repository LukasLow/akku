//! Compiled templates and the engine that renders them.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use super::ast::Node;
use super::context::Context;
use super::error::{Error, Result};
use super::eval;
use super::parser;
use super::value::Value;

/// The default limit for nested tags and expressions.
const DEFAULT_NESTING_DEPTH: usize = 32;

/// Whether printed text is escaped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AutoEscape {
    /// Escape `& < > " '` in every printed [`Value::Str`]. This is the default.
    #[default]
    Html,
    /// Print text as it is (plain-text templates such as e-mails).
    None,
}

/// A custom filter: it receives the filtered value and the call's arguments.
///
/// An `Err` becomes [`ErrorKind::FilterFailed`](crate::template::ErrorKind)
/// with the message and the position of the filter name.
pub type FilterFn =
    Arc<dyn Fn(&Value, &[Value]) -> std::result::Result<Value, String> + Send + Sync + 'static>;

/// The limits that keep rendering bounded.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Limits {
    pub(crate) include_depth: usize,
    pub(crate) nesting_depth: usize,
    pub(crate) output_len: usize,
    pub(crate) loop_iterations: usize,
    pub(crate) include_calls: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            include_depth: 16,
            nesting_depth: DEFAULT_NESTING_DEPTH,
            output_len: 8 * 1024 * 1024,
            loop_iterations: 1_000_000,
            include_calls: 10_000,
        }
    }
}

pub(crate) struct Inner {
    pub(crate) name: Option<String>,
    pub(crate) source: String,
    pub(crate) nodes: Vec<Node>,
    pub(crate) parent: Option<(String, usize)>,
    pub(crate) blocks: HashMap<String, Arc<Vec<Node>>>,
}

/// A compiled template. Cloning is cheap.
///
/// A standalone template has no engine, so `{% include %}` and
/// `{% extends %}` fail with `TemplateNotFound`; register templates with an
/// [`Engine`] to use them.
///
/// ```
/// use akku::template::{Context, Template};
///
/// let template = Template::new("Hello, {{ name }}!").unwrap();
/// let output = template.render(&Context::new().with("name", "<Ada>")).unwrap();
/// assert_eq!(output, "Hello, &lt;Ada&gt;!");
/// ```
#[derive(Clone)]
pub struct Template {
    pub(crate) inner: Arc<Inner>,
}

impl Template {
    /// Compile an anonymous template.
    pub fn new(source: &str) -> Result<Template> {
        Self::compile(None, source, DEFAULT_NESTING_DEPTH)
    }

    /// Compile a template with a name, which appears in its error messages.
    pub fn named(name: impl Into<String>, source: &str) -> Result<Template> {
        Self::compile(Some(name.into()), source, DEFAULT_NESTING_DEPTH)
    }

    pub(crate) fn compile(name: Option<String>, source: &str, nesting: usize) -> Result<Template> {
        let parsed = parser::compile(source, nesting)
            .map_err(|error| (*error).bind(name.as_deref(), source))?;
        Ok(Template {
            inner: Arc::new(Inner {
                name,
                source: source.to_owned(),
                nodes: parsed.nodes,
                parent: parsed.parent,
                blocks: parsed.blocks,
            }),
        })
    }

    /// The template's name, if it has one.
    pub fn name(&self) -> Option<&str> {
        self.inner.name.as_deref()
    }

    /// Render with HTML auto-escaping and no engine.
    pub fn render(&self, context: &Context) -> Result<String> {
        Engine::new().render_template(self, context)
    }
}

impl fmt::Debug for Template {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Template")
            .field("name", &self.inner.name)
            .field("source_len", &self.inner.source.len())
            .finish()
    }
}

/// A set of named templates, custom filters and limits.
///
/// ```
/// use akku::template::{Context, Engine};
///
/// let mut engine = Engine::new();
/// engine.add_template("base", "<h1>{% block title %}Base{% endblock %}</h1>").unwrap();
/// engine
///     .add_template("page", "{% extends \"base\" %}{% block title %}{{ t }}{% endblock %}")
///     .unwrap();
/// engine.add_filter("shout", |value, _args| {
///     Ok(format!("{}!", value).into())
/// });
/// let html = engine.render("page", &Context::new().with("t", "Hi")).unwrap();
/// assert_eq!(html, "<h1>Hi</h1>");
/// assert_eq!(engine.render_str("{{ 'a' | shout }}", &Context::new()).unwrap(), "a!");
/// ```
#[derive(Clone)]
pub struct Engine {
    pub(crate) templates: HashMap<String, Template>,
    pub(crate) filters: HashMap<String, FilterFn>,
    pub(crate) auto_escape: AutoEscape,
    pub(crate) limits: Limits,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for Engine {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut templates: Vec<&String> = self.templates.keys().collect();
        templates.sort();
        let mut filters: Vec<&String> = self.filters.keys().collect();
        filters.sort();
        formatter
            .debug_struct("Engine")
            .field("templates", &templates)
            .field("filters", &filters)
            .field("auto_escape", &self.auto_escape)
            .field("limits", &self.limits)
            .finish()
    }
}

impl Engine {
    /// An engine with the built-in filters, HTML auto-escaping and the default
    /// limits (include depth 16, nesting depth 32, output 8 MiB, one million
    /// loop iterations and ten thousand includes per render).
    pub fn new() -> Self {
        Self {
            templates: HashMap::new(),
            filters: HashMap::new(),
            auto_escape: AutoEscape::Html,
            limits: Limits::default(),
        }
    }

    /// Compile and register a template. Syntax errors are reported now, with
    /// the template's name. Templates may refer to each other in any order.
    pub fn add_template(&mut self, name: impl Into<String>, source: &str) -> Result<()> {
        let name = name.into();
        let template = Template::compile(Some(name.clone()), source, self.limits.nesting_depth)?;
        self.templates.insert(name, template);
        Ok(())
    }

    /// Remove a template. Returns whether it existed.
    pub fn remove_template(&mut self, name: &str) -> bool {
        self.templates.remove(name).is_some()
    }

    /// Whether a template with this name is registered.
    pub fn has_template(&self, name: &str) -> bool {
        self.templates.contains_key(name)
    }

    /// Register a custom filter. A filter with the name of a built-in one
    /// replaces it.
    pub fn add_filter<F>(&mut self, name: impl Into<String>, filter: F) -> &mut Self
    where
        F: Fn(&Value, &[Value]) -> std::result::Result<Value, String> + Send + Sync + 'static,
    {
        self.filters.insert(name.into(), Arc::new(filter));
        self
    }

    /// Choose whether printed text is escaped (default
    /// [`AutoEscape::Html`]).
    pub fn set_auto_escape(&mut self, mode: AutoEscape) -> &mut Self {
        self.auto_escape = mode;
        self
    }

    /// The deepest chain of includes and `extends` (default 16).
    pub fn set_max_include_depth(&mut self, depth: usize) -> &mut Self {
        self.limits.include_depth = depth;
        self
    }

    /// The deepest nesting of tags and expressions, applied to templates added
    /// afterwards (default 32).
    pub fn set_max_nesting_depth(&mut self, depth: usize) -> &mut Self {
        self.limits.nesting_depth = depth.max(1);
        self
    }

    /// The largest output (and intermediate string) in bytes (default 8 MiB).
    pub fn set_max_output_len(&mut self, bytes: usize) -> &mut Self {
        self.limits.output_len = bytes;
        self
    }

    /// The most loop iterations one render may run (default 1,000,000).
    pub fn set_max_loop_iterations(&mut self, iterations: usize) -> &mut Self {
        self.limits.loop_iterations = iterations;
        self
    }

    /// The most includes one render may run (default 10,000).
    pub fn set_max_include_calls(&mut self, calls: usize) -> &mut Self {
        self.limits.include_calls = calls;
        self
    }

    /// Render the registered template `name`.
    pub fn render(&self, name: &str, context: &Context) -> Result<String> {
        let template = self.templates.get(name).ok_or_else(|| {
            Error::new(super::error::ErrorKind::TemplateNotFound {
                name: name.to_owned(),
            })
        })?;
        self.render_template(template, context)
    }

    /// Compile and render `source` as an anonymous template. It can include
    /// and extend the registered templates.
    pub fn render_str(&self, source: &str, context: &Context) -> Result<String> {
        let template = Template::compile(None, source, self.limits.nesting_depth)?;
        self.render_template(&template, context)
    }

    pub(crate) fn render_template(&self, template: &Template, context: &Context) -> Result<String> {
        eval::render(self, template, context)
    }
}

/// Render a compiled template with HTML auto-escaping and no engine.
pub fn render(template: &Template, context: &Context) -> Result<String> {
    template.render(context)
}

/// Compile and render `source` with HTML auto-escaping and no engine.
///
/// ```
/// use akku::template::{render_str, Context};
///
/// let context = Context::new().with("items", vec!["a", "<b>"]);
/// let html = render_str(
///     "{% for item in items %}<li>{{ loop.index }}: {{ item }}</li>{% endfor %}",
///     &context,
/// )
/// .unwrap();
/// assert_eq!(html, "<li>1: a</li><li>2: &lt;b&gt;</li>");
/// ```
pub fn render_str(source: &str, context: &Context) -> Result<String> {
    Engine::new().render_str(source, context)
}
