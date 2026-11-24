//! The HTML renderer.
//!
//! Rendering is iterative: an explicit work stack replaces recursion, and any
//! container or wrapper nested deeper than the nesting limit is skipped, so a
//! hostile hand-built tree cannot overflow the stack during rendering.
//!
//! Everything user-controlled is escaped with [`crate::html::escape`], and
//! link/image destinations go through the URL policy first.

use super::ast::{Alignment, Block, Document, Inline, List, ListItem, ListKind};
use super::options::Options;
use super::url::{is_safe_url, normalize_href};
use crate::html::escape;

const OPEN: [&str; 6] = ["<h1>", "<h2>", "<h3>", "<h4>", "<h5>", "<h6>"];
const CLOSE: [&str; 6] = [
    "</h1>\n", "</h2>\n", "</h3>\n", "</h4>\n", "</h5>\n", "</h6>\n",
];
const CHECKBOX_OFF: &str = "<input type=\"checkbox\" disabled=\"\" /> ";
const CHECKBOX_ON: &str = "<input type=\"checkbox\" disabled=\"\" checked=\"\" /> ";

/// How a block is placed inside its parent.
#[derive(Clone, Copy)]
enum Place {
    Top,
    /// A child of a list item; `checkbox` is set for the item's first child.
    Item {
        tight: bool,
        checkbox: Option<bool>,
    },
}

enum Task<'a> {
    Block {
        block: &'a Block,
        depth: usize,
        place: Place,
    },
    Item {
        item: &'a ListItem,
        depth: usize,
        tight: bool,
    },
    Inline {
        inline: &'a Inline,
        depth: usize,
    },
    Static(&'static str),
    Owned(String),
}

/// Render a [`Document`] to HTML.
pub(crate) fn render(document: &Document, options: &Options) -> String {
    let mut renderer = Renderer {
        out: String::new(),
        stack: Vec::new(),
        options,
        max_depth: options.depth_limit(),
    };
    let tasks: Vec<Task<'_>> = document
        .blocks
        .iter()
        .map(|block| Task::Block {
            block,
            depth: 0,
            place: Place::Top,
        })
        .collect();
    renderer.stack.extend(tasks.into_iter().rev());
    renderer.run();
    renderer.out
}

struct Renderer<'a, 'd> {
    out: String,
    stack: Vec<Task<'d>>,
    options: &'a Options,
    max_depth: usize,
}

fn align_attr(alignment: Alignment) -> &'static str {
    match alignment {
        Alignment::Unset => "",
        Alignment::Left => " align=\"left\"",
        Alignment::Center => " align=\"center\"",
        Alignment::Right => " align=\"right\"",
    }
}

impl<'a, 'd> Renderer<'a, 'd> {
    fn push_all(&mut self, tasks: Vec<Task<'d>>) {
        self.stack.extend(tasks.into_iter().rev());
    }

    fn inline_tasks(&self, inlines: &'d [Inline], depth: usize) -> Vec<Task<'d>> {
        inlines
            .iter()
            .map(|inline| Task::Inline { inline, depth })
            .collect()
    }

    fn run(&mut self) {
        while let Some(task) = self.stack.pop() {
            match task {
                Task::Static(text) => self.out.push_str(text),
                Task::Owned(text) => self.out.push_str(&text),
                Task::Block {
                    block,
                    depth,
                    place,
                } => self.block(block, depth, place),
                Task::Item { item, depth, tight } => self.item(item, depth, tight),
                Task::Inline { inline, depth } => self.inline(inline, depth),
            }
        }
    }

    fn block(&mut self, block: &'d Block, depth: usize, place: Place) {
        let mut checkbox = None;
        if let Place::Item {
            tight,
            checkbox: cb,
        } = place
        {
            checkbox = cb;
            let tight_paragraph = tight && matches!(block, Block::Paragraph(_));
            if !tight_paragraph && !self.out.ends_with('\n') {
                self.out.push('\n');
            }
        }
        match block {
            Block::Heading { level, content } => {
                let index = usize::from((*level).clamp(1, 6)) - 1;
                self.out.push_str(OPEN[index]);
                let mut tasks = self.inline_tasks(content, 0);
                tasks.push(Task::Static(CLOSE[index]));
                self.push_all(tasks);
            }
            Block::Paragraph(content) => {
                let tight = matches!(place, Place::Item { tight: true, .. });
                if !tight {
                    self.out.push_str("<p>");
                }
                match checkbox {
                    Some(true) => self.out.push_str(CHECKBOX_ON),
                    Some(false) => self.out.push_str(CHECKBOX_OFF),
                    None => {}
                }
                let mut tasks = self.inline_tasks(content, 0);
                if !tight {
                    tasks.push(Task::Static("</p>\n"));
                }
                self.push_all(tasks);
            }
            Block::CodeBlock { info, literal } => {
                self.out.push_str("<pre><code");
                let language = info
                    .as_deref()
                    .and_then(|info| info.split_whitespace().next());
                if let Some(language) = language {
                    self.out.push_str(" class=\"language-");
                    self.out.push_str(&escape(language));
                    self.out.push('"');
                }
                self.out.push('>');
                self.out.push_str(&escape(literal));
                self.out.push_str("</code></pre>\n");
            }
            Block::BlockQuote(children) => {
                if depth >= self.max_depth {
                    return;
                }
                self.out.push_str("<blockquote>\n");
                let mut tasks: Vec<Task<'d>> = children
                    .iter()
                    .map(|block| Task::Block {
                        block,
                        depth: depth + 1,
                        place: Place::Top,
                    })
                    .collect();
                tasks.push(Task::Static("</blockquote>\n"));
                self.push_all(tasks);
            }
            Block::List(list) => self.list(list, depth),
            Block::ThematicBreak => self.out.push_str("<hr />\n"),
            Block::Table(table) => {
                self.out.push_str("<table>\n<thead>\n<tr>\n");
                let mut tasks: Vec<Task<'d>> = Vec::new();
                for (index, cell) in table.head.iter().enumerate() {
                    let align = table
                        .alignments
                        .get(index)
                        .copied()
                        .unwrap_or(Alignment::Unset);
                    tasks.push(Task::Owned(format!("<th{}>", align_attr(align))));
                    tasks.extend(self.inline_tasks(cell, 0));
                    tasks.push(Task::Static("</th>\n"));
                }
                tasks.push(Task::Static("</tr>\n</thead>\n"));
                if !table.rows.is_empty() {
                    tasks.push(Task::Static("<tbody>\n"));
                    for row in &table.rows {
                        tasks.push(Task::Static("<tr>\n"));
                        for (index, cell) in row.iter().enumerate() {
                            let align = table
                                .alignments
                                .get(index)
                                .copied()
                                .unwrap_or(Alignment::Unset);
                            tasks.push(Task::Owned(format!("<td{}>", align_attr(align))));
                            tasks.extend(self.inline_tasks(cell, 0));
                            tasks.push(Task::Static("</td>\n"));
                        }
                        tasks.push(Task::Static("</tr>\n"));
                    }
                    tasks.push(Task::Static("</tbody>\n"));
                }
                tasks.push(Task::Static("</table>\n"));
                self.push_all(tasks);
            }
            Block::HtmlBlock(raw) => {
                if self.options.allow_raw_html {
                    self.out.push_str(&tag_filter(raw));
                    self.out.push('\n');
                } else {
                    self.out.push_str("<p>");
                    self.out.push_str(&escape(raw));
                    self.out.push_str("</p>\n");
                }
            }
        }
    }

    fn list(&mut self, list: &'d List, depth: usize) {
        if depth >= self.max_depth {
            return;
        }
        let (open, close) = match list.kind {
            ListKind::Bullet => ("<ul>\n".to_owned(), "</ul>\n"),
            ListKind::Ordered { start: 1 } => ("<ol>\n".to_owned(), "</ol>\n"),
            ListKind::Ordered { start } => (format!("<ol start=\"{start}\">\n"), "</ol>\n"),
        };
        self.out.push_str(&open);
        let mut tasks: Vec<Task<'d>> = list
            .items
            .iter()
            .map(|item| Task::Item {
                item,
                depth: depth + 1,
                tight: list.tight,
            })
            .collect();
        tasks.push(Task::Static(close));
        self.push_all(tasks);
    }

    fn item(&mut self, item: &'d ListItem, depth: usize, tight: bool) {
        self.out.push_str("<li>");
        let first_is_paragraph = matches!(item.blocks.first(), Some(Block::Paragraph(_)));
        if !first_is_paragraph {
            match item.checked {
                Some(true) => self.out.push_str(CHECKBOX_ON),
                Some(false) => self.out.push_str(CHECKBOX_OFF),
                None => {}
            }
        }
        let mut tasks: Vec<Task<'d>> = item
            .blocks
            .iter()
            .enumerate()
            .map(|(index, block)| Task::Block {
                block,
                depth,
                place: Place::Item {
                    tight,
                    checkbox: if index == 0 { item.checked } else { None },
                },
            })
            .collect();
        tasks.push(Task::Static("</li>\n"));
        self.push_all(tasks);
    }

    /// Write `<tag attr="destination"` (the tag stays open).
    fn open_link(&mut self, tag: &str, attr: &str, destination: &str) {
        self.out.push_str(tag);
        self.out.push_str(attr);
        self.out.push_str("=\"");
        self.out.push_str(&escape(&normalize_href(destination)));
        self.out.push('"');
    }

    fn title_attr(&mut self, title: Option<&String>) {
        if let Some(title) = title {
            self.out.push_str(" title=\"");
            self.out.push_str(&escape(title));
            self.out.push('"');
        }
    }

    fn inline(&mut self, inline: &'d Inline, depth: usize) {
        match inline {
            Inline::Text(text) => self.out.push_str(&escape(text)),
            Inline::Code(code) => {
                self.out.push_str("<code>");
                self.out.push_str(&escape(code));
                self.out.push_str("</code>");
            }
            Inline::SoftBreak => self.out.push('\n'),
            Inline::HardBreak => self.out.push_str("<br />\n"),
            Inline::HtmlInline(raw) => {
                if self.options.allow_raw_html {
                    self.out.push_str(&tag_filter(raw));
                } else {
                    self.out.push_str(&escape(raw));
                }
            }
            Inline::Emphasis(children) => self.wrap(children, depth, "<em>", "</em>"),
            Inline::Strong(children) => self.wrap(children, depth, "<strong>", "</strong>"),
            Inline::Strikethrough(children) => self.wrap(children, depth, "<del>", "</del>"),
            Inline::Link {
                destination,
                title,
                content,
            } => {
                if depth >= self.max_depth {
                    return;
                }
                let mut tasks = self.inline_tasks(content, depth + 1);
                if is_safe_url(destination, self.options) {
                    self.open_link("<a", " href", destination);
                    self.title_attr(title.as_ref());
                    self.out.push('>');
                    tasks.push(Task::Static("</a>"));
                }
                self.push_all(tasks);
            }
            Inline::Image {
                destination,
                title,
                alt,
            } => {
                let text = plain_text(alt, self.max_depth);
                if is_safe_url(destination, self.options) {
                    self.open_link("<img", " src", destination);
                    self.out.push_str(" alt=\"");
                    self.out.push_str(&escape(&text));
                    self.out.push('"');
                    self.title_attr(title.as_ref());
                    self.out.push_str(" />");
                } else {
                    self.out.push_str(&escape(&text));
                }
            }
        }
    }

    fn wrap(
        &mut self,
        children: &'d [Inline],
        depth: usize,
        open: &'static str,
        close: &'static str,
    ) {
        if depth >= self.max_depth {
            return;
        }
        self.out.push_str(open);
        let mut tasks = self.inline_tasks(children, depth + 1);
        tasks.push(Task::Static(close));
        self.push_all(tasks);
    }
}

/// The plain-text content of inlines, as used for `alt` attributes.
fn plain_text(inlines: &[Inline], max_depth: usize) -> String {
    let mut out = String::new();
    let mut stack: Vec<std::slice::Iter<'_, Inline>> = vec![inlines.iter()];
    while let Some(iter) = stack.last_mut() {
        match iter.next() {
            None => {
                stack.pop();
            }
            Some(inline) => match inline {
                Inline::Text(text) | Inline::Code(text) => out.push_str(text),
                Inline::SoftBreak | Inline::HardBreak => out.push(' '),
                Inline::HtmlInline(_) => {}
                Inline::Emphasis(children)
                | Inline::Strong(children)
                | Inline::Strikethrough(children)
                | Inline::Link {
                    content: children, ..
                }
                | Inline::Image { alt: children, .. } => {
                    if stack.len() <= max_depth {
                        stack.push(children.iter());
                    }
                }
            },
        }
    }
    out
}

/// Whether `byte` ends a tag name in the HTML tokenizer: tab, line feed,
/// form feed, carriage return (a line ending after preprocessing), space,
/// `/` or `>`. Anything else would continue the name.
fn ends_tag_name(byte: u8) -> bool {
    matches!(byte, b'\t' | b'\n' | b'\x0c' | b'\r' | b' ' | b'/' | b'>')
}

/// The GFM tag filter: neutralize the `<` of tags that are never safe to emit.
fn tag_filter(raw: &str) -> String {
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
    let bytes = raw.as_bytes();
    let mut out = String::with_capacity(raw.len());
    let mut last = 0;
    for (index, &byte) in bytes.iter().enumerate() {
        if byte != b'<' {
            continue;
        }
        let mut name_start = index + 1;
        if bytes.get(name_start) == Some(&b'/') {
            name_start += 1;
        }
        let blocked = BLOCKED.iter().any(|name| {
            let end = name_start + name.len();
            bytes
                .get(name_start..end)
                .is_some_and(|candidate| candidate.eq_ignore_ascii_case(name.as_bytes()))
                && bytes.get(end).is_none_or(|&next| ends_tag_name(next))
        });
        if blocked {
            out.push_str(&raw[last..index]);
            out.push_str("&lt;");
            last = index + 1;
        }
    }
    out.push_str(&raw[last..]);
    out
}
