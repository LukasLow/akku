//! The block parser.
//!
//! A line-by-line implementation of the CommonMark container/leaf algorithm.
//! Open blocks form a *chain* from the document down to the current leaf, and
//! all blocks live in an arena, so nothing here recurses while reading input.
//! Opening a block quote or list item beyond the nesting limit simply does not
//! happen: the marker stays literal paragraph text.

use super::ast::{Alignment, Block, Document, List, ListItem, ListKind, Table};
use super::entities::unescape;
use super::inline::{self, RefStall, Refs};
use super::options::Options;
use super::scan;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Doc,
    Quote,
    List {
        ordered: bool,
        delim: u8,
        start: u64,
    },
    Item {
        offset: usize,
        pad: usize,
    },
    Para,
    Heading(u8),
    Indented,
    Fenced {
        ch: u8,
        len: usize,
        offset: usize,
    },
    Html(u8),
    Thematic,
    Table,
    Removed,
}

struct Node {
    kind: Kind,
    children: Vec<usize>,
    text: String,
    info: String,
    aligns: Vec<Alignment>,
    last_blank: bool,
    start_line: usize,
    /// For a paragraph whose head failed to parse as a reference definition
    /// in a table attempt: why, and the text length at that time. Later
    /// attempts skip the rescan unless the appended text could matter.
    ref_stall: Option<(RefStall, usize)>,
}

impl Node {
    fn new(kind: Kind, start_line: usize) -> Self {
        Self {
            kind,
            children: Vec::new(),
            text: String::new(),
            info: String::new(),
            aligns: Vec::new(),
            last_blank: false,
            start_line,
            ref_stall: None,
        }
    }
}

/// A cursor over one line, tracking the virtual column so that tabs expand to
/// four-column stops and can be consumed partially.
struct Cur<'a> {
    s: &'a str,
    pos: usize,
    col: usize,
    /// Columns of a partially consumed tab at `pos`.
    tab: usize,
}

impl<'a> Cur<'a> {
    fn new(s: &'a str) -> Self {
        Self {
            s,
            pos: 0,
            col: 0,
            tab: 0,
        }
    }

    fn peek(&self) -> Option<u8> {
        if self.tab > 0 {
            Some(b' ')
        } else {
            self.s.as_bytes().get(self.pos).copied()
        }
    }

    /// The byte offset of the first non-whitespace byte and the number of
    /// whitespace columns before it.
    fn scan_ws(&self) -> (usize, usize) {
        let bytes = self.s.as_bytes();
        let (mut p, mut col, mut n) = (self.pos, self.col, 0);
        if self.tab > 0 {
            n += self.tab;
            col += self.tab;
            p += 1;
        }
        while let Some(&byte) = bytes.get(p) {
            match byte {
                b' ' => {
                    p += 1;
                    col += 1;
                    n += 1;
                }
                b'\t' => {
                    let width = 4 - col % 4;
                    p += 1;
                    col += width;
                    n += width;
                }
                _ => break,
            }
        }
        (p, n)
    }

    fn indent(&self) -> usize {
        self.scan_ws().1
    }

    fn is_blank(&self) -> bool {
        self.scan_ws().0 >= self.s.len()
    }

    fn advance_cols(&mut self, mut n: usize) {
        let bytes = self.s.as_bytes();
        while n > 0 {
            if self.tab > 0 {
                let take = n.min(self.tab);
                self.tab -= take;
                self.col += take;
                n -= take;
                if self.tab == 0 {
                    self.pos += 1;
                }
                continue;
            }
            match bytes.get(self.pos) {
                Some(b' ') => {
                    self.pos += 1;
                    self.col += 1;
                    n -= 1;
                }
                Some(b'\t') => {
                    let width = 4 - self.col % 4;
                    if width <= n {
                        self.pos += 1;
                        self.col += width;
                        n -= width;
                    } else {
                        self.tab = width - n;
                        self.col += n;
                        n = 0;
                    }
                }
                _ => break,
            }
        }
    }

    /// Advance over `n` ASCII marker bytes.
    fn adv(&mut self, n: usize) {
        self.pos += n;
        self.col += n;
    }

    /// The rest of the line, expanding a partially consumed tab.
    fn rest_text(&self) -> String {
        if self.tab > 0 {
            let mut text = " ".repeat(self.tab);
            text.push_str(&self.s[self.pos + 1..]);
            text
        } else {
            self.s[self.pos..].to_owned()
        }
    }

    /// The rest of the line without leading whitespace.
    fn rest_trimmed(&self) -> &'a str {
        &self.s[self.scan_ws().0..]
    }
}

enum Cont {
    Yes,
    No,
    Done,
}

/// Parse `input` into a [`Document`].
pub(crate) fn parse(input: &str, options: &Options, max_depth: usize) -> Document {
    let mut text = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                text.push('\n');
            }
            '\0' => text.push('\u{fffd}'),
            _ => text.push(c),
        }
    }
    let mut parser = BlockParser::new(options, max_depth);
    if !text.is_empty() {
        let body = text.strip_suffix('\n').unwrap_or(&text);
        for line in body.split('\n') {
            parser.process_line(line);
        }
    }
    parser.finish()
}

struct BlockParser<'a> {
    options: &'a Options,
    max_depth: usize,
    nodes: Vec<Node>,
    chain: Vec<usize>,
    line_no: usize,
    refs: Refs,
}

impl<'a> BlockParser<'a> {
    fn new(options: &'a Options, max_depth: usize) -> Self {
        Self {
            options,
            max_depth,
            nodes: vec![Node::new(Kind::Doc, 0)],
            chain: vec![0],
            line_no: 0,
            refs: Refs::default(),
        }
    }

    fn kind_at(&self, chain_index: usize) -> Kind {
        self.nodes[self.chain[chain_index]].kind
    }

    fn tip_kind(&self) -> Kind {
        self.nodes[*self.chain.last().expect("the chain holds the document")].kind
    }

    fn depth_at(&self, chain_index: usize) -> usize {
        self.chain[..=chain_index]
            .iter()
            .filter(|&&id| matches!(self.nodes[id].kind, Kind::Quote | Kind::Item { .. }))
            .count()
    }

    fn new_node(&mut self, kind: Kind) -> usize {
        self.nodes.push(Node::new(kind, self.line_no));
        self.nodes.len() - 1
    }

    /// Close every open block above chain index `len - 1`.
    fn close_to(&mut self, len: usize) {
        while self.chain.len() > len {
            let id = self.chain.pop().expect("the chain is not empty");
            self.finalize(id);
        }
    }

    fn finalize(&mut self, id: usize) {
        if self.nodes[id].kind != Kind::Para {
            return;
        }
        let mut text = std::mem::take(&mut self.nodes[id].text);
        let consumed = inline::extract_refs(&text, &mut self.refs);
        text.drain(..consumed);
        let trimmed = text.trim_end().len();
        text.truncate(trimmed);
        if text.is_empty() {
            self.nodes[id].kind = Kind::Removed;
        } else {
            self.nodes[id].text = text;
        }
    }

    /// Climb from `container` to the nearest block that can hold the child,
    /// closing everything below it. Returns that block's chain index.
    fn parent_for(&mut self, mut container: usize, child_is_item: bool) -> usize {
        loop {
            let accepts = match self.kind_at(container) {
                Kind::Doc | Kind::Quote | Kind::Item { .. } => !child_is_item,
                Kind::List { .. } => child_is_item,
                _ => false,
            };
            if accepts {
                break;
            }
            container -= 1;
        }
        self.close_to(container + 1);
        container
    }

    /// Open a container or accepting-leaf block; returns its chain index.
    fn add_child(&mut self, container: usize, kind: Kind) -> usize {
        let parent = self.parent_for(container, matches!(kind, Kind::Item { .. }));
        let id = self.new_node(kind);
        let parent_id = self.chain[parent];
        self.nodes[parent_id].children.push(id);
        self.chain.push(id);
        self.chain.len() - 1
    }

    /// Add a finished leaf; returns the chain index of its parent.
    fn add_leaf(&mut self, container: usize, kind: Kind, text: String) -> usize {
        let parent = self.parent_for(container, false);
        let id = self.new_node(kind);
        self.nodes[id].text = text;
        let parent_id = self.chain[parent];
        self.nodes[parent_id].children.push(id);
        parent
    }

    fn continue_block(&self, id: usize, cur: &mut Cur<'_>) -> Cont {
        let node = &self.nodes[id];
        match node.kind {
            Kind::Quote => {
                let (p, indent) = cur.scan_ws();
                if indent <= 3 && cur.s.as_bytes().get(p) == Some(&b'>') {
                    cur.advance_cols(indent);
                    cur.adv(1);
                    if matches!(cur.peek(), Some(b' ' | b'\t')) {
                        cur.advance_cols(1);
                    }
                    Cont::Yes
                } else {
                    Cont::No
                }
            }
            Kind::List { .. } => Cont::Yes,
            Kind::Item { offset, pad } => {
                if cur.is_blank() {
                    if node.children.is_empty() {
                        Cont::No
                    } else {
                        let n = cur.indent();
                        cur.advance_cols(n);
                        Cont::Yes
                    }
                } else if cur.indent() >= offset + pad {
                    cur.advance_cols(offset + pad);
                    Cont::Yes
                } else {
                    Cont::No
                }
            }
            Kind::Para | Kind::Table => {
                if cur.is_blank() {
                    Cont::No
                } else {
                    Cont::Yes
                }
            }
            Kind::Indented => {
                let n = cur.indent();
                if n >= 4 {
                    cur.advance_cols(4);
                    Cont::Yes
                } else if cur.is_blank() {
                    cur.advance_cols(n);
                    Cont::Yes
                } else {
                    Cont::No
                }
            }
            Kind::Fenced { ch, len, offset } => {
                let (p, indent) = cur.scan_ws();
                if indent <= 3 && closing_fence(&cur.s[p..], ch, len) {
                    Cont::Done
                } else {
                    let n = cur.indent().min(offset);
                    cur.advance_cols(n);
                    Cont::Yes
                }
            }
            Kind::Html(kind) => {
                if cur.is_blank() && (kind == 6 || kind == 7) {
                    Cont::No
                } else {
                    Cont::Yes
                }
            }
            _ => Cont::No,
        }
    }

    fn mark_blank(&mut self, container: usize, blank: bool) {
        let id = self.chain[container];
        // A blank line ends whatever block came last inside the container.
        if blank {
            if let Some(&last) = self.nodes[id].children.last() {
                self.nodes[last].last_blank = true;
            }
        }
        let node = &self.nodes[id];
        let blank_counts = blank
            && !(matches!(node.kind, Kind::Quote | Kind::Fenced { .. })
                || (matches!(node.kind, Kind::Item { .. })
                    && node.children.is_empty()
                    && node.start_line == self.line_no));
        self.nodes[id].last_blank = blank_counts;
        for index in 0..container {
            let ancestor = self.chain[index];
            self.nodes[ancestor].last_blank = false;
        }
    }

    fn process_line(&mut self, line: &str) {
        self.line_no += 1;
        let mut cur = Cur::new(line);

        // 1. Match the open containers.
        let mut matched = 0usize;
        let mut all_matched = true;
        let mut i = 1;
        while i < self.chain.len() {
            let id = self.chain[i];
            match self.continue_block(id, &mut cur) {
                Cont::Yes => {
                    matched = i;
                    i += 1;
                }
                Cont::No => {
                    all_matched = false;
                    break;
                }
                Cont::Done => {
                    self.close_to(i);
                    for index in 0..i {
                        let ancestor = self.chain[index];
                        self.nodes[ancestor].last_blank = false;
                    }
                    return;
                }
            }
        }

        // 2. Open new blocks.
        let mut container = matched;
        let mut consumed = false;
        loop {
            let container_kind = self.kind_at(container);
            if matches!(
                container_kind,
                Kind::Indented | Kind::Fenced { .. } | Kind::Html(_)
            ) {
                break;
            }
            let (nonspace, indent) = cur.scan_ws();
            let rest = &line[nonspace..];
            let blank = rest.is_empty();
            let indented = indent >= 4;
            let first = rest.as_bytes().first().copied();
            let paragraph_like = matches!(container_kind, Kind::Para | Kind::Table);

            if !indented && first == Some(b'>') {
                if self.depth_at(container) >= self.max_depth {
                    break;
                }
                cur.advance_cols(indent);
                cur.adv(1);
                if matches!(cur.peek(), Some(b' ' | b'\t')) {
                    cur.advance_cols(1);
                }
                container = self.add_child(container, Kind::Quote);
                continue;
            }
            if !indented && first == Some(b'#') {
                if let Some((level, content)) = atx_heading(rest) {
                    container = self.add_leaf(container, Kind::Heading(level), content);
                    consumed = true;
                    break;
                }
            }
            if !indented && matches!(first, Some(b'`' | b'~')) {
                if let Some((ch, len, info)) = fence_start(rest) {
                    container = self.add_child(
                        container,
                        Kind::Fenced {
                            ch,
                            len,
                            offset: indent,
                        },
                    );
                    let id = self.chain[container];
                    self.nodes[id].info = info;
                    consumed = true;
                    break;
                }
            }
            if !indented && first == Some(b'<') {
                if let Some(kind) = html_start(rest, paragraph_like) {
                    container = self.add_child(container, Kind::Html(kind));
                    break;
                }
            }
            if !indented && container_kind == Kind::Para && matches!(first, Some(b'=' | b'-')) {
                if let Some(level) = setext_underline(rest) {
                    let id = self.chain[container];
                    let mut text = std::mem::take(&mut self.nodes[id].text);
                    let used = inline::extract_refs(&text, &mut self.refs);
                    text.drain(..used);
                    let trimmed = text.trim_end().len();
                    text.truncate(trimmed);
                    if text.is_empty() {
                        self.nodes[id].kind = Kind::Removed;
                        self.chain.pop();
                        container -= 1;
                        continue;
                    }
                    self.nodes[id].kind = Kind::Heading(level);
                    self.nodes[id].text = text;
                    self.chain.pop();
                    container -= 1;
                    consumed = true;
                    break;
                }
            }
            if self.options.tables
                && !indented
                && container_kind == Kind::Para
                && matches!(first, Some(b':' | b'-' | b'|'))
            {
                if let Some(aligns) = delimiter_row(rest) {
                    match self.start_table(container, aligns) {
                        Some(table) => {
                            container = table;
                            consumed = true;
                            break;
                        }
                        None => {
                            if self.kind_at(container) == Kind::Removed {
                                self.chain.pop();
                                container -= 1;
                                continue;
                            }
                        }
                    }
                }
            }
            if !indented && matches!(first, Some(b'-' | b'*' | b'_')) && is_thematic_break(rest) {
                container = self.add_leaf(container, Kind::Thematic, String::new());
                consumed = true;
                break;
            }
            if !indented && matches!(first, Some(b'-' | b'+' | b'*' | b'0'..=b'9')) {
                if let Some(marker) = list_marker(rest, paragraph_like) {
                    if self.depth_at(container) >= self.max_depth {
                        break;
                    }
                    cur.advance_cols(indent);
                    cur.adv(marker.len);
                    let spaces = cur.indent();
                    let padding = if cur.is_blank() || !(1..5).contains(&spaces) {
                        if spaces > 0 {
                            cur.advance_cols(1);
                        }
                        marker.len + 1
                    } else {
                        cur.advance_cols(spaces);
                        marker.len + spaces
                    };
                    let reuse = match self.kind_at(container) {
                        Kind::List { ordered, delim, .. } => {
                            ordered == marker.ordered && delim == marker.delim
                        }
                        _ => false,
                    };
                    let list = if reuse {
                        container
                    } else {
                        self.add_child(
                            container,
                            Kind::List {
                                ordered: marker.ordered,
                                delim: marker.delim,
                                start: marker.start,
                            },
                        )
                    };
                    container = self.add_child(
                        list,
                        Kind::Item {
                            offset: indent,
                            pad: padding,
                        },
                    );
                    continue;
                }
            }
            if indented && !blank && !matches!(self.tip_kind(), Kind::Para | Kind::Table) {
                cur.advance_cols(4);
                container = self.add_child(container, Kind::Indented);
                break;
            }
            break;
        }

        let blank = cur.is_blank();
        if consumed {
            self.mark_blank(container, false);
            return;
        }

        // 3. Lazy paragraph continuation.
        if !all_matched && container == matched && !blank && self.tip_kind() == Kind::Para {
            let tip = *self.chain.last().expect("the chain is not empty");
            let text = cur.rest_trimmed();
            self.nodes[tip].text.push_str(text);
            self.nodes[tip].text.push('\n');
            return;
        }

        // 4. Add the line to the container.
        self.close_to(container + 1);
        self.mark_blank(container, blank);
        let id = self.chain[container];
        match self.kind_at(container) {
            Kind::Para | Kind::Table => {
                self.nodes[id].text.push_str(cur.rest_trimmed());
                self.nodes[id].text.push('\n');
            }
            Kind::Indented | Kind::Fenced { .. } => {
                let text = cur.rest_text();
                self.nodes[id].text.push_str(&text);
                self.nodes[id].text.push('\n');
            }
            Kind::Html(kind) => {
                let text = cur.rest_text();
                let ended = html_ends(kind, &text);
                self.nodes[id].text.push_str(&text);
                self.nodes[id].text.push('\n');
                if ended {
                    self.close_to(container);
                }
            }
            _ => {
                if !blank {
                    let para = self.add_child(container, Kind::Para);
                    let pid = self.chain[para];
                    self.nodes[pid].text.push_str(cur.rest_trimmed());
                    self.nodes[pid].text.push('\n');
                }
            }
        }
    }

    /// Turn the paragraph at `container` into a table if its last line is a
    /// header row matching the delimiter row. Returns the table's chain index.
    ///
    /// When the paragraph held nothing but reference definitions it is marked
    /// [`Kind::Removed`] and `None` is returned.
    fn start_table(&mut self, container: usize, aligns: Vec<Alignment>) -> Option<usize> {
        let id = self.chain[container];
        let mut text = std::mem::take(&mut self.nodes[id].text);
        // Every delimiter-row line lands here; rescanning the whole paragraph
        // for definitions each time would be quadratic (an unclosed title
        // scans to the end), so a known failure is only retried when the
        // lines appended since could change it.
        let skip = match self.nodes[id].ref_stall {
            Some((RefStall::Never, _)) => true,
            Some((RefStall::Title(close), scanned)) => {
                !inline::title_may_close(&text, scanned, close)
            }
            Some((RefStall::Retry, _)) | None => false,
        };
        if !skip {
            let (used, stall) = inline::extract_refs_stall(&text, &mut self.refs);
            text.drain(..used);
            self.nodes[id].ref_stall = Some((stall, text.len()));
        }
        let trimmed = text.trim_end_matches('\n').len();
        text.truncate(trimmed);
        if text.is_empty() {
            self.nodes[id].kind = Kind::Removed;
            return None;
        }
        // Only the last line is looked at, and nothing is copied unless the
        // table really starts: a failed attempt must cost O(last line), or a
        // paragraph of many near-miss rows would go quadratic.
        let split = text.rfind('\n');
        let header_start = split.map_or(0, |at| at + 1);
        if split_row(&text[header_start..]).len() != aligns.len() {
            text.push('\n');
            // Everything up to here has been looked at for a closing quote.
            if let Some((_, scanned)) = &mut self.nodes[id].ref_stall {
                *scanned = text.len();
            }
            self.nodes[id].text = text;
            return None;
        }
        let header = format!("{}\n", &text[header_start..]);
        let Some(at) = split else {
            let node = &mut self.nodes[id];
            node.kind = Kind::Table;
            node.aligns = aligns;
            node.text = header;
            return Some(container);
        };
        // The lines before the header stay a paragraph of their own.
        text.truncate(at + 1);
        self.nodes[id].text = text;
        self.close_to(container);
        let table = self.add_child(container - 1, Kind::Table);
        let table_id = self.chain[table];
        self.nodes[table_id].aligns = aligns;
        self.nodes[table_id].text = header;
        Some(table)
    }

    fn finish(mut self) -> Document {
        self.close_to(1);
        let blocks = self.blocks_of(0);
        Document { blocks }
    }

    // -- tree conversion (depth is bounded by the nesting limit) ---------------

    fn inlines(&self, text: &str) -> Vec<super::ast::Inline> {
        inline::parse_inlines(text, &self.refs, self.options, self.max_depth)
    }

    fn kids(&self, id: usize) -> impl Iterator<Item = usize> + '_ {
        self.nodes[id]
            .children
            .iter()
            .copied()
            .filter(|&child| self.nodes[child].kind != Kind::Removed)
    }

    fn blocks_of(&self, parent: usize) -> Vec<Block> {
        self.kids(parent).filter_map(|id| self.block(id)).collect()
    }

    fn block(&self, id: usize) -> Option<Block> {
        let node = &self.nodes[id];
        Some(match node.kind {
            Kind::Quote => Block::BlockQuote(self.blocks_of(id)),
            Kind::List { .. } => Block::List(self.list(id)),
            Kind::Para => Block::Paragraph(self.inlines(&node.text)),
            Kind::Heading(level) => Block::Heading {
                level,
                content: self.inlines(&node.text),
            },
            Kind::Indented => Block::CodeBlock {
                info: None,
                literal: indented_literal(&node.text),
            },
            Kind::Fenced { .. } => Block::CodeBlock {
                info: if node.info.is_empty() {
                    None
                } else {
                    Some(node.info.clone())
                },
                literal: node.text.clone(),
            },
            Kind::Html(_) => Block::HtmlBlock(html_literal(&node.text)),
            Kind::Thematic => Block::ThematicBreak,
            Kind::Table => Block::Table(self.table(id)),
            Kind::Doc | Kind::Item { .. } | Kind::Removed => return None,
        })
    }

    fn list(&self, id: usize) -> List {
        let node = &self.nodes[id];
        let (ordered, start) = match node.kind {
            Kind::List { ordered, start, .. } => (ordered, start),
            _ => (false, 0),
        };
        let items: Vec<usize> = self.kids(id).collect();
        let tight = self.is_tight(&items);
        List {
            kind: if ordered {
                ListKind::Ordered { start }
            } else {
                ListKind::Bullet
            },
            tight,
            items: items.iter().map(|&item| self.item(item)).collect(),
        }
    }

    fn item(&self, id: usize) -> ListItem {
        let mut blocks = self.blocks_of(id);
        let mut checked = None;
        if self.options.task_lists {
            let first = self.kids(id).next();
            if let (Some(first), Some(Block::Paragraph(_))) = (first, blocks.first()) {
                let text = &self.nodes[first].text;
                let marker = if text.starts_with("[ ]") {
                    Some(false)
                } else if text.starts_with("[x]") || text.starts_with("[X]") {
                    Some(true)
                } else {
                    None
                };
                if let Some(state) = marker {
                    if matches!(text.as_bytes().get(3), Some(b' ' | b'\t')) {
                        checked = Some(state);
                        let rest = text[4..].trim_start().to_owned();
                        blocks[0] = Block::Paragraph(self.inlines(&rest));
                    }
                }
            }
        }
        ListItem { checked, blocks }
    }

    fn ends_with_blank(&self, mut id: usize) -> bool {
        loop {
            if self.nodes[id].last_blank {
                return true;
            }
            match self.nodes[id].kind {
                Kind::List { .. } | Kind::Item { .. } => match self.kids(id).last() {
                    Some(last) => id = last,
                    None => return false,
                },
                _ => return false,
            }
        }
    }

    fn is_tight(&self, items: &[usize]) -> bool {
        for (index, &item) in items.iter().enumerate() {
            let has_next_item = index + 1 < items.len();
            if has_next_item && self.ends_with_blank(item) {
                return false;
            }
            let children: Vec<usize> = self.kids(item).collect();
            for (child_index, &child) in children.iter().enumerate() {
                let has_next = has_next_item || child_index + 1 < children.len();
                if has_next && self.ends_with_blank(child) {
                    return false;
                }
            }
        }
        true
    }

    fn table(&self, id: usize) -> Table {
        let node = &self.nodes[id];
        let columns = node.aligns.len();
        let mut lines = node.text.lines();
        let header = lines.next().unwrap_or("");
        let head = split_row(header)
            .iter()
            .map(|cell| self.inlines(cell))
            .collect();
        let rows = lines
            .map(|line| {
                let mut cells = split_row(line);
                cells.resize(columns, String::new());
                cells.iter().map(|cell| self.inlines(cell)).collect()
            })
            .collect();
        Table {
            alignments: node.aligns.clone(),
            head,
            rows,
        }
    }
}

// ---------------------------------------------------------------------------
// Line classifiers.
// ---------------------------------------------------------------------------

fn trim_blanks(s: &str) -> &str {
    s.trim_matches(|c| c == ' ' || c == '\t')
}

fn atx_heading(rest: &str) -> Option<(u8, String)> {
    let bytes = rest.as_bytes();
    let hashes = bytes.iter().take_while(|&&b| b == b'#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    if hashes < bytes.len() && bytes[hashes] != b' ' && bytes[hashes] != b'\t' {
        return None;
    }
    let mut content = trim_blanks(&rest[hashes..]);
    let without_closing = content.trim_end_matches('#');
    if without_closing.len() != content.len() {
        if without_closing.is_empty() {
            content = "";
        } else if without_closing.ends_with([' ', '\t']) {
            content = trim_blanks(without_closing);
        }
    }
    Some((hashes as u8, content.to_owned()))
}

fn fence_start(rest: &str) -> Option<(u8, usize, String)> {
    let bytes = rest.as_bytes();
    let ch = *bytes.first()?;
    let len = bytes.iter().take_while(|&&b| b == ch).count();
    if len < 3 {
        return None;
    }
    let info = &rest[len..];
    if ch == b'`' && info.contains('`') {
        return None;
    }
    Some((ch, len, unescape(trim_blanks(info))))
}

fn closing_fence(rest: &str, ch: u8, len: usize) -> bool {
    let bytes = rest.as_bytes();
    let run = bytes.iter().take_while(|&&b| b == ch).count();
    run >= len && trim_blanks(&rest[run..]).is_empty()
}

fn is_thematic_break(rest: &str) -> bool {
    let mut marker = 0u8;
    let mut count = 0;
    for &byte in rest.as_bytes() {
        match byte {
            b' ' | b'\t' => {}
            b'*' | b'-' | b'_' if marker == 0 || marker == byte => {
                marker = byte;
                count += 1;
            }
            _ => return false,
        }
    }
    count >= 3
}

fn setext_underline(rest: &str) -> Option<u8> {
    let trimmed = rest.trim_end_matches([' ', '\t']);
    let first = *trimmed.as_bytes().first()?;
    if trimmed.bytes().all(|b| b == first) {
        match first {
            b'=' => Some(1),
            b'-' => Some(2),
            _ => None,
        }
    } else {
        None
    }
}

struct Marker {
    ordered: bool,
    start: u64,
    delim: u8,
    len: usize,
}

fn list_marker(rest: &str, interrupting: bool) -> Option<Marker> {
    let bytes = rest.as_bytes();
    let (ordered, start, delim, len) = match *bytes.first()? {
        byte @ (b'-' | b'+' | b'*') => (false, 0, byte, 1),
        b'0'..=b'9' => {
            let digits = bytes.iter().take_while(|b| b.is_ascii_digit()).count();
            if digits > 9 || !matches!(bytes.get(digits), Some(b'.' | b')')) {
                return None;
            }
            let start: u64 = rest[..digits].parse().ok()?;
            (true, start, bytes[digits], digits + 1)
        }
        _ => return None,
    };
    if !matches!(bytes.get(len), None | Some(b' ' | b'\t')) {
        return None;
    }
    if interrupting && (trim_blanks(&rest[len..]).is_empty() || (ordered && start != 1)) {
        return None;
    }
    Some(Marker {
        ordered,
        start,
        delim,
        len,
    })
}

fn is_block_tag(name: &str) -> bool {
    if name.len() > 12 {
        return false;
    }
    matches!(
        name.to_ascii_lowercase().as_str(),
        "address"
            | "article"
            | "aside"
            | "base"
            | "basefont"
            | "blockquote"
            | "body"
            | "caption"
            | "center"
            | "col"
            | "colgroup"
            | "dd"
            | "details"
            | "dialog"
            | "dir"
            | "div"
            | "dl"
            | "dt"
            | "fieldset"
            | "figcaption"
            | "figure"
            | "footer"
            | "form"
            | "frame"
            | "frameset"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "head"
            | "header"
            | "hr"
            | "html"
            | "iframe"
            | "legend"
            | "li"
            | "link"
            | "main"
            | "menu"
            | "menuitem"
            | "nav"
            | "noframes"
            | "ol"
            | "optgroup"
            | "option"
            | "p"
            | "param"
            | "search"
            | "section"
            | "summary"
            | "table"
            | "tbody"
            | "td"
            | "tfoot"
            | "th"
            | "thead"
            | "title"
            | "tr"
            | "track"
            | "ul"
    )
}

/// The HTML block start condition (1–7) matching `rest`, if any.
fn html_start(rest: &str, interrupting: bool) -> Option<u8> {
    let bytes = rest.as_bytes();
    if bytes.first() != Some(&b'<') {
        return None;
    }
    for tag in ["script", "pre", "style", "textarea"] {
        let end = 1 + tag.len();
        if bytes.len() >= end
            && bytes[1..end].eq_ignore_ascii_case(tag.as_bytes())
            && matches!(bytes.get(end), None | Some(b' ' | b'\t' | b'>'))
        {
            return Some(1);
        }
    }
    if rest.starts_with("<!--") {
        return Some(2);
    }
    if rest.starts_with("<?") {
        return Some(3);
    }
    if rest.starts_with("<![CDATA[") {
        return Some(5);
    }
    if bytes.get(1) == Some(&b'!') && bytes.get(2).is_some_and(u8::is_ascii_alphabetic) {
        return Some(4);
    }
    let mut i = 1;
    if bytes.get(i) == Some(&b'/') {
        i += 1;
    }
    let name_start = i;
    while i < bytes.len() && bytes[i].is_ascii_alphanumeric() {
        i += 1;
    }
    let name = &rest[name_start..i];
    if !name.is_empty()
        && is_block_tag(name)
        && (matches!(bytes.get(i), None | Some(b' ' | b'\t' | b'>')) || rest[i..].starts_with("/>"))
    {
        return Some(6);
    }
    if !interrupting {
        let end = scan::scan_open_tag(bytes, 0).or_else(|| scan::scan_close_tag(bytes, 0));
        if let Some(end) = end {
            if trim_blanks(&rest[end..]).is_empty() {
                return Some(7);
            }
        }
    }
    None
}

fn contains_ignore_case(haystack: &str, needle: &str) -> bool {
    haystack
        .as_bytes()
        .windows(needle.len())
        .any(|window| window.eq_ignore_ascii_case(needle.as_bytes()))
}

fn html_ends(kind: u8, line: &str) -> bool {
    match kind {
        1 => ["</script>", "</pre>", "</style>", "</textarea>"]
            .iter()
            .any(|tag| contains_ignore_case(line, tag)),
        2 => line.contains("-->"),
        3 => line.contains("?>"),
        4 => line.contains('>'),
        5 => line.contains("]]>"),
        _ => false,
    }
}

/// Strip trailing whitespace-only lines from a block's text.
fn strip_trailing_blank_lines(text: &str) -> &str {
    let mut end = text.len();
    while end > 0 {
        let body = &text[..end];
        let body = body.strip_suffix('\n').unwrap_or(body);
        let line_start = body.rfind('\n').map_or(0, |at| at + 1);
        if trim_blanks(&body[line_start..]).is_empty() {
            end = line_start;
        } else {
            break;
        }
    }
    &text[..end]
}

fn indented_literal(text: &str) -> String {
    strip_trailing_blank_lines(text).to_owned()
}

fn html_literal(text: &str) -> String {
    strip_trailing_blank_lines(text)
        .trim_end_matches('\n')
        .to_owned()
}

// ---------------------------------------------------------------------------
// Tables.
// ---------------------------------------------------------------------------

/// Split a table row into trimmed cells; `\|` is a literal pipe.
fn split_row(line: &str) -> Vec<String> {
    let trimmed = trim_blanks(line);
    let trimmed = trimmed.strip_prefix('|').unwrap_or(trimmed);
    let trimmed = if trimmed.ends_with('|') && !trimmed.ends_with("\\|") {
        &trimmed[..trimmed.len() - 1]
    } else {
        trimmed
    };
    let mut cells = Vec::new();
    let mut cell = String::new();
    let mut chars = trimmed.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' && chars.peek() == Some(&'|') {
            cell.push('|');
            chars.next();
        } else if c == '|' {
            cells.push(trim_blanks(&cell).to_owned());
            cell.clear();
        } else {
            cell.push(c);
        }
    }
    cells.push(trim_blanks(&cell).to_owned());
    cells
}

fn delimiter_row(rest: &str) -> Option<Vec<Alignment>> {
    if !rest.contains('|') {
        return None;
    }
    let mut aligns = Vec::new();
    for cell in split_row(rest) {
        let left = cell.starts_with(':');
        let right = cell.ends_with(':') && cell.len() > 1;
        let core = cell.strip_prefix(':').unwrap_or(&cell);
        let core = core.strip_suffix(':').unwrap_or(core);
        if core.is_empty() || !core.bytes().all(|b| b == b'-') {
            return None;
        }
        aligns.push(match (left, right) {
            (true, true) => Alignment::Center,
            (true, false) => Alignment::Left,
            (false, true) => Alignment::Right,
            (false, false) => Alignment::Unset,
        });
    }
    Some(aligns)
}
