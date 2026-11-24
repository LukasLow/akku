//! CommonMark and GitHub-flavored Markdown: a parser, a syntax tree and an
//! XSS-safe HTML renderer, with zero dependencies.
//!
//! ```
//! use akku::markdown;
//!
//! assert_eq!(markdown::to_html("# Hi"), "<h1>Hi</h1>\n");
//! assert_eq!(
//!     markdown::to_html("*safe* <script>alert(1)</script>"),
//!     "<p><em>safe</em> &lt;script&gt;alert(1)&lt;/script&gt;</p>\n"
//! );
//! ```
//!
//! # Safe by default
//!
//! * Raw HTML is **escaped**, not passed through ([`Options::allow_raw_html`]).
//!   Even when enabled, the GFM tag filter neutralizes `<script>`, `<iframe>`,
//!   `<style>` and friends.
//! * Link and image destinations are checked by [`is_safe_url`]:
//!   `javascript:`, `vbscript:` and `data:` are never rendered, and only
//!   `http`, `https`, `mailto`, `tel` and relative URLs are allowed by
//!   default. A refused link renders as its plain text.
//! * Text, code, titles, alt text and info strings are always HTML-escaped.
//!
//! The output is therefore safe to hand to [`crate::html::Html`]:
//!
//! ```
//! use akku::html::Html;
//! use akku::markdown;
//!
//! let page = Html::new(markdown::to_html("[x](javascript:alert(1))"));
//! assert_eq!(page.as_str(), "<p>x</p>\n");
//! ```
//!
//! # Hostile input
//!
//! Parsing never fails and never recurses on the input. Block containers
//! (quotes, list items) and inline wrappers (emphasis, links) are limited by
//! [`Options::max_nesting_depth`]; deeper structure degrades to plain text.
//! Emphasis and link resolution run in (amortized) linear time, so inputs such
//! as `****…`, `[a](` repeated or `>>>…` are cheap.
//!
//! # Tree access
//!
//! ```
//! use akku::markdown::{parse, Block, Inline};
//!
//! let document = parse("## Title\n\nHello *world*");
//! assert!(matches!(&document.blocks[0], Block::Heading { level: 2, .. }));
//! if let Block::Paragraph(inlines) = &document.blocks[1] {
//!     assert_eq!(inlines[0], Inline::Text("Hello ".into()));
//!     assert!(matches!(inlines[1], Inline::Emphasis(_)));
//! }
//! ```
//!
//! # Conformance
//!
//! The target is CommonMark 0.31 plus the GFM tables, strikethrough,
//! task-list, autolink and tag-filter extensions. Known deviations: only
//! numeric and about fifty common named character references are decoded, and
//! Unicode punctuation for emphasis flanking is approximated.

mod ast;
mod autolink;
mod block;
mod entities;
mod inline;
mod options;
mod render;
mod scan;
mod url;

pub use self::ast::{Alignment, Block, Document, Inline, List, ListItem, ListKind, Table};
pub use self::options::Options;
pub use self::url::is_safe_url;

/// Parse Markdown with the default [`Options`].
pub fn parse(input: &str) -> Document {
    parse_with(input, &Options::default())
}

/// Parse Markdown with explicit [`Options`].
///
/// Parsing is infallible: constructs that exceed the nesting limit are kept
/// as plain text.
pub fn parse_with(input: &str, options: &Options) -> Document {
    block::parse(input, options, options.depth_limit())
}

/// Convert Markdown to safe HTML with the default [`Options`].
pub fn to_html(input: &str) -> String {
    to_html_with(input, &Options::default())
}

/// Convert Markdown to HTML with explicit [`Options`].
pub fn to_html_with(input: &str, options: &Options) -> String {
    render_html(&parse_with(input, options), options)
}

/// Render a [`Document`] to HTML.
///
/// The URL policy, raw-HTML policy and nesting limit of `options` are applied
/// at render time, so trees that were built by hand or deserialized are
/// rendered as safely as parsed ones.
pub fn render_html(document: &Document, options: &Options) -> String {
    render::render(document, options)
}
