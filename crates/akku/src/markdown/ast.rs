//! The Markdown syntax tree.
//!
//! The tree is deliberately plain data: every type has public fields or
//! variants so callers can inspect, build and transform documents freely.
//! Link and image destinations are stored exactly as written (after backslash
//! and entity decoding) and are only checked against the URL policy when the
//! tree is rendered, so hand-built trees are rendered safely too.
//!
//! Trees produced by [`parse`](crate::markdown::parse) are never nested
//! deeper than [`Options::max_nesting_depth`](crate::markdown::Options). The
//! renderer is iterative and skips anything deeper, but dropping an extremely
//! deep *hand-built* tree recurses; keep hand-built trees shallow.

/// A parsed Markdown document: a list of top-level blocks.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Document {
    /// The top-level blocks, in source order.
    pub blocks: Vec<Block>,
}

/// A block-level element.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    /// An ATX (`# Title`) or setext (`Title\n===`) heading.
    Heading {
        /// The heading level, `1..=6`. The renderer clamps other values.
        level: u8,
        /// The inline content of the heading.
        content: Vec<Inline>,
    },
    /// A paragraph of inline content.
    Paragraph(Vec<Inline>),
    /// An indented or fenced code block.
    CodeBlock {
        /// The info string of a fenced block (for example `rust`), if any.
        info: Option<String>,
        /// The code, without fences. It ends with a newline unless empty.
        literal: String,
    },
    /// A block quote.
    BlockQuote(Vec<Block>),
    /// A bullet or ordered list.
    List(List),
    /// A thematic break (`---`).
    ThematicBreak,
    /// A GitHub-flavored table.
    Table(Table),
    /// Raw HTML. The renderer escapes it unless raw HTML is explicitly enabled.
    HtmlBlock(String),
}

/// A list of items.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct List {
    /// Whether the list is a bullet list or an ordered list.
    pub kind: ListKind,
    /// Whether the list is tight (no blank lines between items or blocks).
    pub tight: bool,
    /// The list items.
    pub items: Vec<ListItem>,
}

/// The flavor of a [`List`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListKind {
    /// A bullet list (`-`, `+` or `*`).
    Bullet,
    /// An ordered list.
    Ordered {
        /// The number of the first item.
        start: u64,
    },
}

/// One item of a [`List`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListItem {
    /// The GFM task-list state: `Some(true)` for `[x]`, `Some(false)` for
    /// `[ ]`, `None` for a regular item.
    pub checked: Option<bool>,
    /// The blocks inside the item.
    pub blocks: Vec<Block>,
}

/// A GFM table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    /// The column alignments, one per header cell.
    pub alignments: Vec<Alignment>,
    /// The header cells.
    pub head: Vec<Vec<Inline>>,
    /// The body rows. Every parsed row has exactly `head.len()` cells.
    pub rows: Vec<Vec<Vec<Inline>>>,
}

/// The alignment of a table column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Alignment {
    /// No alignment was requested.
    Unset,
    /// `:--`
    Left,
    /// `:-:`
    Center,
    /// `--:`
    Right,
}

/// An inline-level element.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inline {
    /// Plain text. Adjacent text is merged into one node.
    Text(String),
    /// A code span.
    Code(String),
    /// A soft line break.
    SoftBreak,
    /// A hard line break.
    HardBreak,
    /// Emphasis (`*text*`).
    Emphasis(Vec<Inline>),
    /// Strong emphasis (`**text**`).
    Strong(Vec<Inline>),
    /// GFM strikethrough (`~~text~~`).
    Strikethrough(Vec<Inline>),
    /// A link. Autolinks are links whose content is the literal address.
    Link {
        /// The destination, unchecked. The renderer applies the URL policy.
        destination: String,
        /// The optional title.
        title: Option<String>,
        /// The link text.
        content: Vec<Inline>,
    },
    /// An image.
    Image {
        /// The destination, unchecked. The renderer applies the URL policy.
        destination: String,
        /// The optional title.
        title: Option<String>,
        /// The alternative text, as inline content.
        alt: Vec<Inline>,
    },
    /// Raw inline HTML. The renderer escapes it unless raw HTML is enabled.
    HtmlInline(String),
}
