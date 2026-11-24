//! The inline parser.
//!
//! This follows the CommonMark reference algorithm: one forward scan creates
//! text, delimiter and bracket nodes in an arena; closing brackets resolve
//! links, and a delimiter-stack pass resolves emphasis. Everything is
//! iterative, and the arena is converted to the public tree with an explicit
//! stack, so hostile nesting can neither overflow the stack nor go quadratic:
//!
//! * delimiter matching uses the `openers_bottom` optimization,
//! * code-span closers are found by binary search over a one-time index,
//! * failed searches for `-->`, `?>`, `]]>` and `>` are remembered,
//! * the "deactivate earlier links" rule is a single watermark, and
//! * wrappers deeper than the nesting limit are flattened, keeping their text.

use std::collections::HashMap;

use super::ast::Inline;
use super::autolink;
use super::entities::{self, unescape};
use super::options::Options;
use super::scan;

const NIL: usize = usize::MAX;
const ROOT: usize = 0;
/// The largest label (in bytes) that is looked up in the reference map.
const MAX_LABEL: usize = 999;
/// The largest paren nesting allowed inside a raw link destination.
const MAX_PAREN_DEPTH: usize = 32;

/// The link reference definitions of a document.
#[derive(Default)]
pub(crate) struct Refs(pub HashMap<String, (String, Option<String>)>);

/// Normalize a link label for lookup: collapse whitespace, case-fold.
pub(crate) fn normalize_label(label: &str) -> String {
    label
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn is_ascii_punct(byte: u8) -> bool {
    byte.is_ascii_punctuation()
}

fn is_punct(c: char) -> bool {
    if c.is_ascii() {
        c.is_ascii_punctuation()
    } else {
        !c.is_alphanumeric() && !c.is_whitespace() && !c.is_control()
    }
}

// ---------------------------------------------------------------------------
// Link destinations, titles and reference definitions (shared with `block`).
// ---------------------------------------------------------------------------

fn skip_ws_one_newline(bytes: &[u8], mut i: usize) -> usize {
    let mut newline = false;
    while i < bytes.len() {
        match bytes[i] {
            b' ' | b'\t' => i += 1,
            b'\n' if !newline => {
                newline = true;
                i += 1;
            }
            _ => break,
        }
    }
    i
}

/// Parse a link destination starting at `i`. Returns the decoded destination
/// and the index after it. An empty raw destination is allowed (the caller
/// decides whether that is valid).
fn parse_dest(src: &str, i: usize) -> Option<(String, usize)> {
    let bytes = src.as_bytes();
    if bytes.get(i) == Some(&b'<') {
        let mut j = i + 1;
        while j < bytes.len() {
            match bytes[j] {
                b'\\' if bytes.get(j + 1).is_some_and(|&b| is_ascii_punct(b)) => j += 2,
                b'>' => return Some((unescape(&src[i + 1..j]), j + 1)),
                b'<' | b'\n' => return None,
                _ => j += 1,
            }
        }
        return None;
    }
    let mut j = i;
    let mut depth = 0usize;
    while j < bytes.len() {
        match bytes[j] {
            b'\\' if bytes.get(j + 1).is_some_and(|&b| is_ascii_punct(b)) => j += 2,
            b'(' => {
                depth += 1;
                if depth > MAX_PAREN_DEPTH {
                    return None;
                }
                j += 1;
            }
            b')' => {
                if depth == 0 {
                    break;
                }
                depth -= 1;
                j += 1;
            }
            byte if byte <= b' ' || byte == 0x7f => break,
            _ => j += 1,
        }
    }
    if depth != 0 {
        return None;
    }
    Some((unescape(&src[i..j]), j))
}

/// Parse a link title starting at `i` (which must be `"`, `'` or `(`).
fn parse_title(src: &str, i: usize) -> Option<(String, usize)> {
    scan_title(src, i).ok()
}

/// [`parse_title`], but a failure says whether the scan ran off the end of
/// `src` (`Err(true)`): only then could more text change the outcome.
fn scan_title(src: &str, i: usize) -> Result<(String, usize), bool> {
    let bytes = src.as_bytes();
    let open = *bytes.get(i).ok_or(true)?;
    let close = match open {
        b'"' => b'"',
        b'\'' => b'\'',
        b'(' => b')',
        _ => return Err(false),
    };
    let mut j = i + 1;
    while j < bytes.len() {
        match bytes[j] {
            b'\\' if bytes.get(j + 1).is_some_and(|&b| is_ascii_punct(b)) => j += 2,
            byte if byte == close => return Ok((unescape(&src[i + 1..j]), j + 1)),
            b'(' if open == b'(' => return Err(false),
            _ => j += 1,
        }
    }
    Err(true)
}

/// Why the head of a paragraph is not a link reference definition, so a
/// caller that keeps appending lines can avoid rescanning the paragraph.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RefStall {
    /// No text appended later can turn the head into a definition.
    Never,
    /// A title is still open: only text holding `close` (or, for a `(`
    /// title, `(`) can change the outcome. See [`title_may_close`].
    Title(u8),
    /// A short, bounded scan (the label or the destination line) reached the
    /// end of the text; simply retry.
    Retry,
}

/// Whether `text[from..]` could end a title stalled on `close`: it holds an
/// unescaped closing byte or, for a `(` title, an opening parenthesis that
/// rejects it. `from` must start a line, so no escape spans it.
pub(crate) fn title_may_close(text: &str, from: usize, close: u8) -> bool {
    let bytes = text.as_bytes();
    let mut j = from;
    while j < bytes.len() {
        match bytes[j] {
            b'\\' if bytes.get(j + 1).is_some_and(|&b| is_ascii_punct(b)) => j += 2,
            byte if byte == close => return true,
            b'(' if close == b')' => return true,
            _ => j += 1,
        }
    }
    false
}

type RefDef = (String, String, Option<String>, usize);

/// Parse one link reference definition at the start of `s`.
///
/// Returns `(label, destination, title, bytes consumed)`, or why there is
/// none as a [`RefStall`].
fn scan_ref_def(s: &str) -> Result<RefDef, RefStall> {
    let bytes = s.as_bytes();
    if bytes.first() != Some(&b'[') {
        return Err(RefStall::Never);
    }
    let mut i = 1;
    loop {
        match *bytes.get(i).ok_or(RefStall::Retry)? {
            b'\\' => i += 2,
            b'[' => return Err(RefStall::Never),
            b']' => break,
            _ => i += 1,
        }
        if i > MAX_LABEL + 1 {
            return Err(RefStall::Never);
        }
    }
    let label = &s[1..i];
    if label.trim().is_empty() {
        return Err(RefStall::Never);
    }
    i += 1;
    match bytes.get(i) {
        Some(&b':') => {}
        Some(_) => return Err(RefStall::Never),
        None => return Err(RefStall::Retry),
    }
    i = skip_ws_one_newline(bytes, i + 1);
    // The destination ends at the first line break; with none after `i` the
    // scan ran to the end of the text and more lines may change it.
    let dest_stall = || {
        if s[i..].contains('\n') {
            RefStall::Never
        } else {
            RefStall::Retry
        }
    };
    let (dest, after_dest) = parse_dest(s, i).ok_or_else(dest_stall)?;
    if dest.is_empty() && bytes.get(i) != Some(&b'<') {
        return Err(dest_stall());
    }
    let mut after_spaces = after_dest;
    while after_spaces < bytes.len() && matches!(bytes[after_spaces], b' ' | b'\t') {
        after_spaces += 1;
    }
    let line_ends = after_spaces >= bytes.len() || bytes[after_spaces] == b'\n';
    let mut title_at = None;
    if after_spaces < bytes.len() && bytes[after_spaces] == b'\n' {
        let mut t = after_spaces + 1;
        while t < bytes.len() && matches!(bytes[t], b' ' | b'\t') {
            t += 1;
        }
        title_at = Some(t);
    } else if after_spaces > after_dest {
        title_at = Some(after_spaces);
    }
    let mut stall = RefStall::Never;
    if let Some(t) = title_at {
        match scan_title(s, t) {
            Ok((title, after_title)) => {
                let mut k = after_title;
                while k < bytes.len() && matches!(bytes[k], b' ' | b'\t') {
                    k += 1;
                }
                if k >= bytes.len() || bytes[k] == b'\n' {
                    let consumed = if k < bytes.len() { k + 1 } else { k };
                    return Ok((label.to_owned(), dest, Some(title), consumed));
                }
            }
            Err(true) => {
                stall = match bytes.get(t) {
                    Some(&b'(') => RefStall::Title(b')'),
                    Some(&close) => RefStall::Title(close),
                    None => RefStall::Retry,
                };
            }
            Err(false) => {}
        }
    }
    if line_ends {
        let consumed = if after_spaces < bytes.len() {
            after_spaces + 1
        } else {
            after_spaces
        };
        return Ok((label.to_owned(), dest, None, consumed));
    }
    Err(stall)
}

/// Consume leading reference definitions of a paragraph into `refs`.
///
/// Returns the number of bytes consumed. The first definition of a label wins.
pub(crate) fn extract_refs(text: &str, refs: &mut Refs) -> usize {
    extract_refs_stall(text, refs).0
}

/// [`extract_refs`], also saying why the remaining text does not start with
/// another definition.
pub(crate) fn extract_refs_stall(text: &str, refs: &mut Refs) -> (usize, RefStall) {
    let mut consumed = 0;
    loop {
        match scan_ref_def(&text[consumed..]) {
            Ok((label, dest, title, used)) => {
                refs.0
                    .entry(normalize_label(&label))
                    .or_insert((dest, title));
                consumed += used;
            }
            Err(stall) => return (consumed, stall),
        }
    }
}

// ---------------------------------------------------------------------------
// The arena.
// ---------------------------------------------------------------------------

enum Kind {
    Root,
    Text(String),
    Delim(u8, usize),
    Code(String),
    Soft,
    Hard,
    Html(String),
    Emph,
    Strong,
    Strike,
    Link(String, Option<String>),
    Image(String, Option<String>),
}

struct Node {
    kind: Kind,
    prev: usize,
    next: usize,
    first: usize,
    last: usize,
}

struct Delim {
    node: usize,
    ch: u8,
    count: usize,
    orig: usize,
    can_open: bool,
    can_close: bool,
    prev: usize,
    next: usize,
}

struct Bracket {
    node: usize,
    image: bool,
    prev_delim: usize,
    /// Byte offset just after the opening bracket.
    pos: usize,
}

struct Parser<'a> {
    src: &'a str,
    bytes: &'a [u8],
    pos: usize,
    refs: &'a Refs,
    strike: bool,
    max_depth: usize,
    nodes: Vec<Node>,
    buf: String,
    delims: Vec<Delim>,
    dtop: usize,
    brackets: Vec<Bracket>,
    /// Non-image brackets below this stack index are deactivated.
    floor: usize,
    ticks: Option<HashMap<usize, Vec<usize>>>,
    /// Per terminator kind: a start offset from which no terminator exists.
    miss: [usize; 4],
}

/// Parse inline content into the public tree.
pub(crate) fn parse_inlines(
    text: &str,
    refs: &Refs,
    options: &Options,
    max_depth: usize,
) -> Vec<Inline> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut parser = Parser {
        src: text,
        bytes: text.as_bytes(),
        pos: 0,
        refs,
        strike: options.strikethrough,
        max_depth,
        nodes: vec![Node {
            kind: Kind::Root,
            prev: NIL,
            next: NIL,
            first: NIL,
            last: NIL,
        }],
        buf: String::new(),
        delims: Vec::new(),
        dtop: NIL,
        brackets: Vec::new(),
        floor: 0,
        ticks: None,
        miss: [NIL; 4],
    };
    parser.run();
    parser.process_emphasis(None);
    let mut inlines = parser.to_ast();
    if options.autolinks {
        autolink::apply(&mut inlines);
    }
    inlines
}

impl<'a> Parser<'a> {
    fn new_node(&mut self, kind: Kind) -> usize {
        self.nodes.push(Node {
            kind,
            prev: NIL,
            next: NIL,
            first: NIL,
            last: NIL,
        });
        self.nodes.len() - 1
    }

    fn append(&mut self, id: usize) {
        let last = self.nodes[ROOT].last;
        if last == NIL {
            self.nodes[ROOT].first = id;
        } else {
            self.nodes[last].next = id;
            self.nodes[id].prev = last;
        }
        self.nodes[ROOT].last = id;
    }

    fn flush(&mut self) {
        if !self.buf.is_empty() {
            let text = std::mem::take(&mut self.buf);
            let id = self.new_node(Kind::Text(text));
            self.append(id);
        }
    }

    fn add(&mut self, kind: Kind) -> usize {
        self.flush();
        let id = self.new_node(kind);
        self.append(id);
        id
    }

    fn is_special(&self, byte: u8) -> bool {
        matches!(
            byte,
            b'\n' | b'\\' | b'`' | b'&' | b'<' | b'*' | b'_' | b'[' | b']' | b'!'
        ) || (byte == b'~' && self.strike)
    }

    fn run(&mut self) {
        let len = self.bytes.len();
        while self.pos < len {
            let byte = self.bytes[self.pos];
            match byte {
                b'\n' => self.newline(),
                b'\\' => self.backslash(),
                b'`' => self.backticks(),
                b'&' => self.entity(),
                b'<' => self.angle(),
                b'*' | b'_' | b'~' if byte != b'~' || self.strike => self.delim_run(byte),
                b'[' => self.open_bracket(false),
                b'!' => {
                    if self.bytes.get(self.pos + 1) == Some(&b'[') {
                        self.open_bracket(true);
                    } else {
                        self.buf.push('!');
                        self.pos += 1;
                    }
                }
                b']' => self.close_bracket(),
                _ => {
                    let start = self.pos;
                    while self.pos < len && !self.is_special(self.bytes[self.pos]) {
                        self.pos += 1;
                    }
                    self.buf.push_str(&self.src[start..self.pos]);
                }
            }
        }
        self.flush();
    }

    fn skip_leading_spaces(&mut self) {
        while self.bytes.get(self.pos) == Some(&b' ') {
            self.pos += 1;
        }
    }

    fn newline(&mut self) {
        let trailing = self.buf.bytes().rev().take_while(|&b| b == b' ').count();
        self.buf.truncate(self.buf.len() - trailing);
        self.add(if trailing >= 2 {
            Kind::Hard
        } else {
            Kind::Soft
        });
        self.pos += 1;
        self.skip_leading_spaces();
    }

    fn backslash(&mut self) {
        match self.bytes.get(self.pos + 1) {
            Some(b'\n') => {
                self.add(Kind::Hard);
                self.pos += 2;
                self.skip_leading_spaces();
            }
            Some(&next) if is_ascii_punct(next) => {
                self.buf.push(next as char);
                self.pos += 2;
            }
            _ => {
                self.buf.push('\\');
                self.pos += 1;
            }
        }
    }

    fn entity(&mut self) {
        match entities::parse(&self.src[self.pos..]) {
            Some((text, used)) => {
                self.buf.push_str(&text);
                self.pos += used;
            }
            None => {
                self.buf.push('&');
                self.pos += 1;
            }
        }
    }

    // -- code spans ---------------------------------------------------------

    fn tick_index(&mut self) -> &HashMap<usize, Vec<usize>> {
        if self.ticks.is_none() {
            let mut map: HashMap<usize, Vec<usize>> = HashMap::new();
            let mut i = 0;
            while i < self.bytes.len() {
                if self.bytes[i] == b'`' {
                    let start = i;
                    while i < self.bytes.len() && self.bytes[i] == b'`' {
                        i += 1;
                    }
                    map.entry(i - start).or_default().push(start);
                } else {
                    i += 1;
                }
            }
            self.ticks = Some(map);
        }
        self.ticks.as_ref().expect("the index was just built")
    }

    fn backticks(&mut self) {
        let start = self.pos;
        let mut end = start;
        while end < self.bytes.len() && self.bytes[end] == b'`' {
            end += 1;
        }
        let count = end - start;
        let closer = self.tick_index().get(&count).and_then(|starts| {
            let at = starts.partition_point(|&s| s < end);
            starts.get(at).copied()
        });
        match closer {
            Some(close_start) => {
                let content = self.src[end..close_start].replace('\n', " ");
                let all_space = content.bytes().all(|b| b == b' ');
                let content = if content.len() >= 2
                    && content.starts_with(' ')
                    && content.ends_with(' ')
                    && !all_space
                {
                    content[1..content.len() - 1].to_owned()
                } else {
                    content
                };
                self.add(Kind::Code(content));
                self.pos = close_start + count;
            }
            None => {
                self.buf.push_str(&self.src[start..end]);
                self.pos = end;
            }
        }
    }

    // -- raw HTML and autolinks ----------------------------------------------

    fn find_from(&mut self, slot: usize, from: usize, pattern: &[u8]) -> Option<usize> {
        if self.miss[slot] <= from {
            return None;
        }
        let found = self.bytes[from.min(self.bytes.len())..]
            .windows(pattern.len())
            .position(|window| window == pattern);
        if found.is_none() {
            self.miss[slot] = from;
        }
        found.map(|offset| from + offset + pattern.len())
    }

    fn scan_html(&mut self, i: usize) -> Option<usize> {
        let bytes = self.bytes;
        match *bytes.get(i + 1)? {
            b'/' => scan::scan_close_tag(bytes, i),
            b'?' => self.find_from(1, i + 2, b"?>"),
            b'!' => {
                if bytes[i..].starts_with(b"<!--") {
                    if bytes[i..].starts_with(b"<!-->") {
                        return Some(i + 5);
                    }
                    if bytes[i..].starts_with(b"<!--->") {
                        return Some(i + 6);
                    }
                    self.find_from(0, i + 4, b"-->")
                } else if bytes[i..].starts_with(b"<![CDATA[") {
                    self.find_from(2, i + 9, b"]]>")
                } else if bytes.get(i + 2).is_some_and(u8::is_ascii_alphabetic) {
                    self.find_from(3, i + 3, b">")
                } else {
                    None
                }
            }
            byte if byte.is_ascii_alphabetic() => scan::scan_open_tag(bytes, i),
            _ => None,
        }
    }

    fn angle(&mut self) {
        let i = self.pos;
        if let Some((destination, text, end)) = scan::scan_autolink(self.src, i) {
            let link = self.add(Kind::Link(destination, None));
            let child = self.new_node(Kind::Text(text));
            self.nodes[link].first = child;
            self.nodes[link].last = child;
            self.pos = end;
            return;
        }
        if let Some(end) = self.scan_html(i) {
            let html = self.src[i..end].to_owned();
            self.add(Kind::Html(html));
            self.pos = end;
            return;
        }
        self.buf.push('<');
        self.pos += 1;
    }

    // -- emphasis ---------------------------------------------------------

    fn delim_run(&mut self, ch: u8) {
        let start = self.pos;
        let mut end = start;
        while end < self.bytes.len() && self.bytes[end] == ch {
            end += 1;
        }
        let count = end - start;
        self.pos = end;
        if ch == b'~' && count > 2 {
            self.buf.push_str(&self.src[start..end]);
            return;
        }
        let before = self.src[..start].chars().next_back();
        let after = self.src[end..].chars().next();
        let ws_before = before.is_none_or(char::is_whitespace);
        let ws_after = after.is_none_or(char::is_whitespace);
        let punct_before = before.is_some_and(is_punct);
        let punct_after = after.is_some_and(is_punct);
        let left = !ws_after && (!punct_after || ws_before || punct_before);
        let right = !ws_before && (!punct_before || ws_after || punct_after);
        let (can_open, can_close) = if ch == b'_' {
            (
                left && (!right || punct_before),
                right && (!left || punct_after),
            )
        } else {
            (left, right)
        };
        if !can_open && !can_close {
            self.buf.push_str(&self.src[start..end]);
            return;
        }
        let node = self.add(Kind::Delim(ch, count));
        let index = self.delims.len();
        if self.dtop != NIL {
            self.delims[self.dtop].next = index;
        }
        self.delims.push(Delim {
            node,
            ch,
            count,
            orig: count,
            can_open,
            can_close,
            prev: self.dtop,
            next: NIL,
        });
        self.dtop = index;
    }

    fn remove_delim(&mut self, index: usize) {
        let (prev, next) = (self.delims[index].prev, self.delims[index].next);
        if prev != NIL {
            self.delims[prev].next = next;
        }
        if next != NIL {
            self.delims[next].prev = prev;
        }
        if self.dtop == index {
            self.dtop = prev;
        }
    }

    fn wrap(&mut self, opener: usize, closer: usize, kind: Kind) {
        let wrapper = self.new_node(kind);
        let first = self.nodes[opener].next;
        if first != closer {
            let last = self.nodes[closer].prev;
            self.nodes[wrapper].first = first;
            self.nodes[wrapper].last = last;
            self.nodes[first].prev = NIL;
            self.nodes[last].next = NIL;
        }
        self.nodes[opener].next = wrapper;
        self.nodes[wrapper].prev = opener;
        self.nodes[closer].prev = wrapper;
        self.nodes[wrapper].next = closer;
    }

    fn set_delim_node_count(&mut self, index: usize) {
        let (node, ch, count) = {
            let d = &self.delims[index];
            (d.node, d.ch, d.count)
        };
        self.nodes[node].kind = Kind::Delim(ch, count);
    }

    /// Resolve emphasis for every delimiter above `bottom`, then drop them.
    fn process_emphasis(&mut self, bottom: Option<usize>) {
        let bottom_link = bottom.unwrap_or(NIL);
        let above = |index: usize, floor: Option<usize>| floor.is_none_or(|f| index > f);
        if self.dtop != NIL && self.dtop != bottom_link {
            let mut closer = self.dtop;
            while self.delims[closer].prev != bottom_link {
                closer = self.delims[closer].prev;
            }
            let mut openers_bottom = [bottom; 18];
            while closer != NIL {
                let (ch, can_open, can_close, orig, count, prev, next) = {
                    let d = &self.delims[closer];
                    (
                        d.ch,
                        d.can_open,
                        d.can_close,
                        d.orig,
                        d.count,
                        d.prev,
                        d.next,
                    )
                };
                if !can_close {
                    closer = next;
                    continue;
                }
                let family = match ch {
                    b'*' => 0,
                    b'_' => 1,
                    _ => 2,
                };
                let key = family * 6 + orig % 3 + if can_open { 3 } else { 0 };
                let mut opener = prev;
                let mut found = NIL;
                while opener != NIL && above(opener, bottom) && above(opener, openers_bottom[key]) {
                    let o = &self.delims[opener];
                    if o.ch == ch && o.can_open {
                        let matches = if ch == b'~' {
                            o.count == count
                        } else {
                            let odd = (can_open || o.can_close)
                                && (o.orig + orig) % 3 == 0
                                && !(o.orig % 3 == 0 && orig % 3 == 0);
                            !odd
                        };
                        if matches {
                            found = opener;
                            break;
                        }
                    }
                    opener = o.prev;
                }
                if found == NIL {
                    openers_bottom[key] = if prev == NIL { None } else { Some(prev) };
                    if !can_open {
                        self.remove_delim(closer);
                    }
                    closer = next;
                    continue;
                }
                let use_count = if ch == b'~' {
                    count
                } else if count >= 2 && self.delims[found].count >= 2 {
                    2
                } else {
                    1
                };
                self.delims[found].count -= use_count;
                self.delims[closer].count -= use_count;
                self.set_delim_node_count(found);
                self.set_delim_node_count(closer);
                let kind = match (ch, use_count) {
                    (b'~', _) => Kind::Strike,
                    (_, 2) => Kind::Strong,
                    _ => Kind::Emph,
                };
                let (opener_node, closer_node) =
                    (self.delims[found].node, self.delims[closer].node);
                self.wrap(opener_node, closer_node, kind);
                let mut between = self.delims[closer].prev;
                while between != found {
                    let before = self.delims[between].prev;
                    self.remove_delim(between);
                    between = before;
                }
                if self.delims[found].count == 0 {
                    self.remove_delim(found);
                }
                if self.delims[closer].count == 0 {
                    let after = self.delims[closer].next;
                    self.remove_delim(closer);
                    closer = after;
                }
            }
        }
        match bottom {
            Some(index) => {
                self.delims.truncate(index + 1);
                self.delims[index].next = NIL;
                self.dtop = index;
            }
            None => {
                self.delims.clear();
                self.dtop = NIL;
            }
        }
    }

    // -- brackets -----------------------------------------------------------

    fn open_bracket(&mut self, image: bool) {
        let width = if image { 2 } else { 1 };
        if self.brackets.len() >= self.max_depth {
            self.buf.push_str(&self.src[self.pos..self.pos + width]);
            self.pos += width;
            return;
        }
        let text = if image { "![" } else { "[" };
        let node = self.add(Kind::Text(text.to_owned()));
        self.pos += width;
        self.brackets.push(Bracket {
            node,
            image,
            prev_delim: self.dtop,
            pos: self.pos,
        });
    }

    fn pop_bracket(&mut self) {
        self.brackets.pop();
        self.floor = self.floor.min(self.brackets.len());
    }

    /// Scan `[label]` starting at the `[` at `i`. Returns the label and the
    /// index after the closing bracket.
    fn scan_label(&self, i: usize) -> Option<(&'a str, usize)> {
        if self.bytes.get(i) != Some(&b'[') {
            return None;
        }
        let mut j = i + 1;
        while j < self.bytes.len() && j - i <= MAX_LABEL + 1 {
            match self.bytes[j] {
                b'\\' => j += 2,
                b'[' => return None,
                b']' => return Some((&self.src[i + 1..j], j + 1)),
                _ => j += 1,
            }
        }
        None
    }

    fn skip_ws(&self, mut i: usize) -> usize {
        let mut newline = false;
        while let Some(&byte) = self.bytes.get(i) {
            match byte {
                b' ' | b'\t' => i += 1,
                b'\n' if !newline => {
                    newline = true;
                    i += 1;
                }
                _ => break,
            }
        }
        i
    }

    /// Parse `dest "title")` after the opening parenthesis at `i`.
    fn parse_inline_link(&self, i: usize) -> Option<(String, Option<String>, usize)> {
        let start = self.skip_ws(i);
        let (dest, after) = parse_dest(self.src, start)?;
        let mut j = self.skip_ws(after);
        let mut title = None;
        if j > after && matches!(self.bytes.get(j), Some(b'"' | b'\'' | b'(')) {
            let (parsed, end) = parse_title(self.src, j)?;
            title = Some(parsed);
            j = self.skip_ws(end);
        }
        if self.bytes.get(j) == Some(&b')') {
            Some((dest, title, j + 1))
        } else {
            None
        }
    }

    fn close_bracket(&mut self) {
        let close_pos = self.pos;
        self.pos += 1;
        let Some(index) = self.brackets.len().checked_sub(1) else {
            self.buf.push(']');
            return;
        };
        let image = self.brackets[index].image;
        if !image && index < self.floor {
            self.pop_bracket();
            self.buf.push(']');
            return;
        }
        let text_start = self.brackets[index].pos;
        let mut result = None;
        if self.bytes.get(self.pos) == Some(&b'(') {
            result = self.parse_inline_link(self.pos + 1);
        }
        if result.is_none() && !self.refs.0.is_empty() {
            let own = &self.src[text_start..close_pos];
            // `[text][label]`, `[text][]` (collapsed) or `[text]` (shortcut).
            let (label, end) = match self.scan_label(self.pos) {
                Some((raw, end)) if !raw.trim().is_empty() => (raw, end),
                Some((_, end)) => (own, end),
                None => (own, self.pos),
            };
            if label.len() <= MAX_LABEL {
                if let Some((dest, title)) = self.refs.0.get(&normalize_label(label)) {
                    result = Some((dest.clone(), title.clone(), end));
                }
            }
        }
        match result {
            Some((dest, title, end)) => self.make_link(index, dest, title, end),
            None => {
                self.pop_bracket();
                self.buf.push(']');
            }
        }
    }

    fn make_link(&mut self, index: usize, dest: String, title: Option<String>, end: usize) {
        self.flush();
        let bracket = self.brackets.pop().expect("a bracket is open");
        debug_assert_eq!(self.brackets.len(), index);
        let image = bracket.image;
        let kind = if image {
            Kind::Image(dest, title)
        } else {
            Kind::Link(dest, title)
        };
        let link = self.new_node(kind);
        let first = self.nodes[bracket.node].next;
        self.nodes[link].prev = bracket.node;
        self.nodes[bracket.node].next = link;
        if first != NIL {
            self.nodes[link].first = first;
            self.nodes[link].last = self.nodes[ROOT].last;
            self.nodes[first].prev = NIL;
        }
        self.nodes[ROOT].last = link;
        self.nodes[bracket.node].kind = Kind::Text(String::new());
        let bottom = if bracket.prev_delim == NIL {
            None
        } else {
            Some(bracket.prev_delim)
        };
        self.process_emphasis(bottom);
        if !image {
            self.floor = self.brackets.len();
        } else {
            self.floor = self.floor.min(self.brackets.len());
        }
        self.pos = end;
    }

    // -- conversion -----------------------------------------------------------

    fn to_ast(&self) -> Vec<Inline> {
        fn push_text(out: &mut Vec<Inline>, text: &str) {
            if text.is_empty() {
                return;
            }
            if let Some(Inline::Text(existing)) = out.last_mut() {
                existing.push_str(text);
            } else {
                out.push(Inline::Text(text.to_owned()));
            }
        }

        // (arena node, next child, whether the wrapper is kept)
        let mut frames: Vec<(usize, usize, bool)> = vec![(ROOT, self.nodes[ROOT].first, true)];
        let mut outs: Vec<Vec<Inline>> = vec![Vec::new()];
        let mut kept = 0usize;
        while let Some(top) = frames.last_mut() {
            if top.1 == NIL {
                let (id, _, keep) = frames.pop().expect("the frame exists");
                if frames.is_empty() {
                    break;
                }
                if keep {
                    let children = outs.pop().expect("a kept wrapper has an output");
                    kept -= 1;
                    let inline = self.wrapper(id, children);
                    outs.last_mut()
                        .expect("the parent has an output")
                        .push(inline);
                }
                continue;
            }
            let id = top.1;
            top.1 = self.nodes[id].next;
            let out = outs.last_mut().expect("there is always an output");
            match &self.nodes[id].kind {
                Kind::Root => {}
                Kind::Text(text) => push_text(out, text),
                Kind::Delim(ch, count) => {
                    push_text(out, &(*ch as char).to_string().repeat(*count));
                }
                Kind::Code(code) => out.push(Inline::Code(code.clone())),
                Kind::Soft => out.push(Inline::SoftBreak),
                Kind::Hard => out.push(Inline::HardBreak),
                Kind::Html(html) => out.push(Inline::HtmlInline(html.clone())),
                Kind::Emph | Kind::Strong | Kind::Strike | Kind::Link(..) | Kind::Image(..) => {
                    let keep = kept < self.max_depth;
                    frames.push((id, self.nodes[id].first, keep));
                    if keep {
                        outs.push(Vec::new());
                        kept += 1;
                    }
                }
            }
        }
        outs.pop().unwrap_or_default()
    }

    fn wrapper(&self, id: usize, children: Vec<Inline>) -> Inline {
        match &self.nodes[id].kind {
            Kind::Emph => Inline::Emphasis(children),
            Kind::Strong => Inline::Strong(children),
            Kind::Strike => Inline::Strikethrough(children),
            Kind::Link(destination, title) => Inline::Link {
                destination: destination.clone(),
                title: title.clone(),
                content: children,
            },
            Kind::Image(destination, title) => Inline::Image {
                destination: destination.clone(),
                title: title.clone(),
                alt: children,
            },
            _ => Inline::Text(String::new()),
        }
    }
}
