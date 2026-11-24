//! Parser and renderer options.

/// The hard upper bound for [`Options::max_nesting_depth`].
///
/// Larger values are clamped so that the (recursive) tree conversion stays
/// within a small, fixed stack budget.
pub(crate) const MAX_NESTING_CAP: usize = 128;

/// Options for [`parse_with`](crate::markdown::parse_with),
/// [`to_html_with`](crate::markdown::to_html_with) and
/// [`render_html`](crate::markdown::render_html).
///
/// The defaults enable the GFM extensions and are **safe**: raw HTML is
/// escaped and only `http`, `https`, `mailto` and `tel` URLs (plus relative
/// URLs) are linked.
///
/// ```
/// use akku::markdown::Options;
///
/// let mut options = Options::default();
/// assert!(options.tables && !options.allow_raw_html);
/// options.max_nesting_depth = 8;
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// GFM tables. Default `true`.
    pub tables: bool,
    /// GFM strikethrough (`~~text~~`). Default `true`.
    pub strikethrough: bool,
    /// GFM task lists (`- [x] done`). Default `true`.
    pub task_lists: bool,
    /// GFM extended autolinks (`https://…`, `www.…`, e-mail addresses).
    /// `<https://…>` autolinks are always on. Default `true`.
    pub autolinks: bool,
    /// Pass raw HTML through. Default `false`.
    ///
    /// When `false`, raw HTML is rendered as escaped text. When `true` it is
    /// emitted verbatim (apart from the GFM tag filter) — only do that for
    /// trusted input.
    pub allow_raw_html: bool,
    /// The maximum nesting depth of block containers (quotes and list items)
    /// and of inline wrappers (emphasis, links). Default `32`, clamped to
    /// `1..=128`. Deeper structure degrades to plain text instead of failing.
    ///
    /// Converting nested blocks to the tree is recursive, costing a few KiB of
    /// stack per level in debug builds: the default is safe on any thread, the
    /// hard cap wants a few hundred KiB.
    pub max_nesting_depth: usize,
    /// The URL schemes links and images may use, compared case-insensitively.
    /// Relative URLs are always allowed; `javascript:`, `vbscript:` and
    /// `data:` are never allowed. Default `http`, `https`, `mailto`, `tel`.
    pub allowed_schemes: Vec<String>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            tables: true,
            strikethrough: true,
            task_lists: true,
            autolinks: true,
            allow_raw_html: false,
            max_nesting_depth: 32,
            allowed_schemes: ["http", "https", "mailto", "tel"]
                .iter()
                .map(|scheme| (*scheme).to_owned())
                .collect(),
        }
    }
}

impl Options {
    /// The default options: GFM extensions on, raw HTML off.
    pub fn new() -> Self {
        Self::default()
    }

    /// Plain CommonMark: every GFM extension off, raw HTML still escaped.
    pub fn commonmark() -> Self {
        Self {
            tables: false,
            strikethrough: false,
            task_lists: false,
            autolinks: false,
            ..Self::default()
        }
    }

    /// Allow or forbid raw HTML (see [`Options::allow_raw_html`]).
    #[must_use]
    pub fn with_raw_html(mut self, allow: bool) -> Self {
        self.allow_raw_html = allow;
        self
    }

    /// Set the maximum nesting depth (see [`Options::max_nesting_depth`]).
    #[must_use]
    pub fn with_max_nesting_depth(mut self, depth: usize) -> Self {
        self.max_nesting_depth = depth;
        self
    }

    /// Allow one more URL scheme, for example `"ftp"`.
    ///
    /// `javascript`, `vbscript` and `data` are refused even when added.
    #[must_use]
    pub fn with_allowed_scheme(mut self, scheme: impl Into<String>) -> Self {
        self.allowed_schemes.push(scheme.into());
        self
    }

    /// The nesting depth actually used.
    pub(crate) fn depth_limit(&self) -> usize {
        self.max_nesting_depth.clamp(1, MAX_NESTING_CAP)
    }
}
