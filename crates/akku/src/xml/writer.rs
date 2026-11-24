//! Streaming and DOM XML 1.0 serializer (`Writer<W>`, `to_string`, `to_string_pretty`).

use std::collections::{HashMap, HashSet};
use std::io::Write;

use crate::xml::dom::{Document, Element, Node};
use crate::xml::error::{Error, ErrorKind, Result};
use crate::xml::reader::{
    check_tag_namespaces, is_valid_encoding_name, is_valid_xml_name, is_xml_char,
    is_xml_whitespace, ns_decl_prefix, qname_error, resolve_in, XmlDecl,
};

/// Options controlling XML serialization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SerializeOptions {
    /// Indentation unit per nesting level (`None` for compact output, `Some("  ")` for pretty).
    pub indent: Option<String>,
    /// Write `<?xml ...?>` when [`Document::declaration`] is `Some`. Default: `true`.
    pub write_declaration: bool,
    /// Render elements without children as `<tag/>` instead of `<tag></tag>`. Default: `true`.
    pub self_closing_empty: bool,
    /// Enforce XML Namespaces 1.0 well-formedness on output (QName syntax, bound prefixes,
    /// legal `xmlns` declarations, unique `(namespace, local_name)` attributes), mirroring
    /// [`ParseOptions::check_namespace_well_formed`](crate::xml::ParseOptions::check_namespace_well_formed).
    /// Default: `true`.
    pub check_namespace_well_formed: bool,
}

impl Default for SerializeOptions {
    fn default() -> Self {
        Self::new()
    }
}

impl SerializeOptions {
    /// Compact XML serialization defaults.
    pub const fn new() -> Self {
        Self {
            indent: None,
            write_declaration: true,
            self_closing_empty: true,
            check_namespace_well_formed: true,
        }
    }

    /// Pretty-printed XML defaults using two spaces per level.
    pub fn pretty() -> Self {
        Self {
            indent: Some("  ".to_string()),
            write_declaration: true,
            self_closing_empty: true,
            check_namespace_well_formed: true,
        }
    }
}

/// Flush the internal buffer to the sink once it grows beyond this many bytes.
const FLUSH_THRESHOLD: usize = 8 * 1024;

/// Streaming XML 1.0 writer over any [`std::io::Write`].
///
/// Output is assembled in an internal buffer and handed to the sink in large chunks
/// (at most one `write_all` per streaming call, or one per ~8 KiB for
/// [`Writer::write_document`]), so unbuffered sinks such as sockets or files do not
/// receive a syscall per escaped character. Text larger than ~8 KiB is validated first
/// and then streamed to the sink in bounded chunks instead of being copied whole.
///
/// Every call validates its input completely before handing anything to the sink, so a
/// validation error writes nothing; only a genuine I/O error from the sink can leave
/// partial output behind (and leaves the writer's tag/namespace state as before the call).
pub struct Writer<W: Write> {
    inner: W,
    options: SerializeOptions,
    open_tags: Vec<String>,
    buf: Vec<u8>,
    /// prefix -> stack of in-scope URIs (innermost last).
    ns_map: HashMap<String, Vec<String>>,
    /// Undo log of `(depth, prefix)` bindings, popped when an element closes.
    ns_undo: Vec<(usize, String)>,
    /// Validation pass of [`Writer::write_document`]: output is discarded, not written.
    dry_run: bool,
}

impl<W: Write> Writer<W> {
    /// Create a compact writer wrapping `inner`.
    pub fn new(inner: W) -> Self {
        Self::with_options(inner, SerializeOptions::new())
    }

    /// Create a writer with custom [`SerializeOptions`].
    pub fn with_options(inner: W, options: SerializeOptions) -> Self {
        Self {
            inner,
            options,
            open_tags: Vec::new(),
            buf: Vec::new(),
            ns_map: HashMap::new(),
            ns_undo: Vec::new(),
            dry_run: false,
        }
    }

    /// Write an `<?xml ...?>` declaration.
    ///
    /// The writer emits XML 1.0 only, so `version` must be exactly `"1.0"` (the version the
    /// [`Reader`](crate::xml::Reader) accepts); `encoding` must match
    /// `[A-Za-z][A-Za-z0-9._-]*` (§4.3.3). Anything else returns
    /// [`ErrorKind::InvalidDecl`] and writes nothing.
    pub fn write_decl(&mut self, decl: &XmlDecl) -> Result<()> {
        let r = self.decl_inner(decl);
        self.complete(r)
    }

    /// Write an opening start tag `<name attr="val">`.
    ///
    /// With [`SerializeOptions::check_namespace_well_formed`] (the default), the element and
    /// attribute names must be valid QNames with bound prefixes, and any `xmlns` /
    /// `xmlns:p` declarations among `attrs` come into scope until the matching
    /// [`Writer::write_end`].
    pub fn write_start(&mut self, name: &str, attrs: &[(&str, &str)]) -> Result<()> {
        let depth = self.open_tags.len() + 1;
        let r = self.open_tag_inner(name, attrs, false, depth);
        let r = self.complete(r);
        if r.is_ok() {
            self.open_tags.push(name.to_string());
        } else {
            self.pop_ns_bindings(depth);
        }
        r
    }

    /// Write a self-closing empty element `<name attr="val"/>` (or `<name></name>` if configured).
    pub fn write_empty(&mut self, name: &str, attrs: &[(&str, &str)]) -> Result<()> {
        let depth = self.open_tags.len() + 1;
        let r = self.empty_inner(name, attrs, depth);
        self.complete(r)
    }

    /// Write the closing tag for the most recently opened element.
    ///
    /// The element stays open if the sink fails, so [`Writer::finish`] still reports it.
    pub fn write_end(&mut self) -> Result<()> {
        let depth = self.open_tags.len();
        let Some(name) = self.open_tags.last() else {
            return Err(Error::new(ErrorKind::UnexpectedEndTag {
                name: "<none>".to_string(),
            }));
        };
        push_close_tag(&mut self.buf, name);
        self.complete(Ok(()))?;
        self.open_tags.pop();
        self.pop_ns_bindings(depth);
        Ok(())
    }

    /// Write escaped character data.
    ///
    /// Text above ~8 KiB is validated in full first and then escaped and streamed to the
    /// sink in bounded chunks, so peak memory does not grow with the text size.
    pub fn write_text(&mut self, text: &str) -> Result<()> {
        if text.len() <= FLUSH_THRESHOLD {
            let r = push_escaped_text(&mut self.buf, text);
            return self.complete(r);
        }
        validate_xml_chars(text)?;
        let mut rest = text;
        while !rest.is_empty() {
            let mut end = rest.len().min(FLUSH_THRESHOLD);
            while !rest.is_char_boundary(end) {
                end -= 1;
            }
            let (chunk, tail) = rest.split_at(end);
            let r = push_escaped_text(&mut self.buf, chunk);
            self.complete(r)?;
            rest = tail;
        }
        Ok(())
    }

    /// Write a `<![CDATA[...]]>` section, safely splitting any internal `]]>`.
    ///
    /// A CR (`\r`) cannot survive inside CDATA (parsers normalize it to LF), so each CR is
    /// emitted as a `&#xD;` character reference between CDATA sections.
    pub fn write_cdata(&mut self, data: &str) -> Result<()> {
        let r = push_cdata(&mut self.buf, data);
        self.complete(r)
    }

    /// Write a `<!-- ... -->` comment, rejecting `--`, a trailing `-`, or a CR (`\r`, which
    /// parsers would silently normalize to LF).
    pub fn write_comment(&mut self, comment: &str) -> Result<()> {
        let r = push_comment(&mut self.buf, comment);
        self.complete(r)
    }

    /// Write a `<?target ...?>` processing instruction.
    ///
    /// Rejects invalid or reserved (`xml`) targets, data containing `?>` or a CR (`\r`), and
    /// data with leading XML whitespace (which a parser would strip, breaking round-trips).
    pub fn write_pi(&mut self, target: &str, data: Option<&str>) -> Result<()> {
        let r = push_pi(&mut self.buf, target, data);
        self.complete(r)
    }

    /// Serialize an entire [`Document`] iteratively (safe for arbitrary nesting depth).
    ///
    /// The whole tree is validated in a first pass before any byte reaches the sink, so a
    /// validation error leaves the sink untouched. Output is then streamed in ~8 KiB
    /// chunks; only an I/O error from the sink can leave a truncated document behind.
    pub fn write_document(&mut self, doc: &Document) -> Result<()> {
        let base_depth = self.open_tags.len() + 1;
        self.dry_run = true;
        let r = self.document_inner(doc, base_depth);
        self.dry_run = false;
        self.buf.clear();
        if r.is_err() {
            self.pop_ns_bindings(base_depth);
            return r;
        }
        let r = self.document_inner(doc, base_depth);
        if r.is_err() {
            self.pop_ns_bindings(base_depth);
        }
        self.complete(r)
    }

    /// Verify all started tags are closed and return the underlying writer.
    pub fn finish(self) -> Result<W> {
        if let Some(unclosed) = self.open_tags.last() {
            return Err(Error::new(ErrorKind::UnclosedTag {
                name: unclosed.clone(),
            }));
        }
        Ok(self.inner)
    }

    /// Consume the writer and return the underlying sink without checking open tags.
    pub fn into_inner(self) -> W {
        self.inner
    }

    /// Finish a public call: hand buffered output to the sink on success, discard on error.
    ///
    /// Invariant: `buf` is empty between public calls.
    fn complete(&mut self, result: Result<()>) -> Result<()> {
        match result {
            Ok(()) => {
                let r = if self.buf.is_empty() {
                    Ok(())
                } else {
                    self.inner.write_all(&self.buf).map_err(Error::from)
                };
                self.buf.clear();
                r
            }
            Err(e) => {
                self.buf.clear();
                Err(e)
            }
        }
    }

    fn flush_if_large(&mut self) -> Result<()> {
        if self.buf.len() >= FLUSH_THRESHOLD {
            if !self.dry_run {
                self.inner.write_all(&self.buf)?;
            }
            self.buf.clear();
        }
        Ok(())
    }

    fn push_ns_binding(&mut self, depth: usize, prefix: &str, uri: &str) {
        self.ns_map
            .entry(prefix.to_string())
            .or_default()
            .push(uri.to_string());
        self.ns_undo.push((depth, prefix.to_string()));
    }

    /// Drop every namespace binding made at `depth` or deeper.
    fn pop_ns_bindings(&mut self, depth: usize) {
        while let Some((d, _)) = self.ns_undo.last() {
            if *d < depth {
                break;
            }
            let Some((_, prefix)) = self.ns_undo.pop() else {
                break;
            };
            if let Some(stack) = self.ns_map.get_mut(&prefix) {
                stack.pop();
                if stack.is_empty() {
                    self.ns_map.remove(&prefix);
                }
            }
        }
    }

    fn decl_inner(&mut self, decl: &XmlDecl) -> Result<()> {
        if !is_valid_version_num(&decl.version) {
            return Err(Error::new(ErrorKind::InvalidDecl));
        }
        self.buf.extend_from_slice(b"<?xml version=\"");
        self.buf.extend_from_slice(decl.version.as_bytes());
        self.buf.push(b'"');
        if let Some(enc) = &decl.encoding {
            if !is_valid_encoding_name(enc) {
                return Err(Error::new(ErrorKind::InvalidDecl));
            }
            self.buf.extend_from_slice(b" encoding=\"");
            self.buf.extend_from_slice(enc.as_bytes());
            self.buf.push(b'"');
        }
        if let Some(standalone) = decl.standalone {
            let val: &[u8] = if standalone { b"yes" } else { b"no" };
            self.buf.extend_from_slice(b" standalone=\"");
            self.buf.extend_from_slice(val);
            self.buf.push(b'"');
        }
        self.buf.extend_from_slice(b"?>");
        Ok(())
    }

    fn empty_inner(&mut self, name: &str, attrs: &[(&str, &str)], depth: usize) -> Result<()> {
        if self.options.self_closing_empty {
            self.open_tag_inner(name, attrs, true, depth)
        } else {
            self.open_tag_inner(name, attrs, false, depth)?;
            push_close_tag(&mut self.buf, name);
            self.pop_ns_bindings(depth);
            Ok(())
        }
    }

    /// Validate and buffer a start/empty tag at nesting `depth` (root = base + 0).
    ///
    /// Namespace declarations stay in scope on success for a non-self-closing tag; they
    /// are rolled back on error or once a self-closing tag is complete.
    fn open_tag_inner(
        &mut self,
        name: &str,
        attrs: &[(&str, &str)],
        self_closing: bool,
        depth: usize,
    ) -> Result<()> {
        let r = self.open_tag_body(name, attrs, self_closing, depth);
        if r.is_err() || self_closing {
            self.pop_ns_bindings(depth);
        }
        r
    }

    fn check_tag_ns(&mut self, name: &str, attrs: &[(&str, &str)], depth: usize) -> Result<()> {
        if let Some(kind) = qname_error(name, true) {
            return Err(Error::new(kind));
        }
        for &(attr_name, attr_val) in attrs {
            if let Some(kind) = qname_error(attr_name, false) {
                return Err(Error::new(kind));
            }
            match ns_decl_prefix(attr_name, attr_val) {
                Ok(Some(prefix)) => self.push_ns_binding(depth, prefix, attr_val),
                Ok(None) => {}
                Err(kind) => return Err(Error::new(kind)),
            }
        }
        let ns_map = &self.ns_map;
        check_tag_namespaces(name, attrs.iter().map(|&(n, _)| n), |p| {
            resolve_in(ns_map, p)
        })
        .map_err(|(kind, _)| Error::new(kind))
    }

    fn open_tag_body(
        &mut self,
        name: &str,
        attrs: &[(&str, &str)],
        self_closing: bool,
        depth: usize,
    ) -> Result<()> {
        validate_name(name)?;
        for &(attr_name, _) in attrs {
            validate_name(attr_name)?;
        }
        check_duplicate_attrs(attrs.iter().map(|&(n, _)| n))?;
        if self.options.check_namespace_well_formed {
            self.check_tag_ns(name, attrs, depth)?;
        }
        self.buf.push(b'<');
        self.buf.extend_from_slice(name.as_bytes());
        for &(attr_name, attr_val) in attrs {
            self.buf.push(b' ');
            self.buf.extend_from_slice(attr_name.as_bytes());
            self.buf.extend_from_slice(b"=\"");
            push_escaped_attr(&mut self.buf, attr_val)?;
            self.buf.push(b'"');
        }
        if self_closing {
            self.buf.extend_from_slice(b"/>");
        } else {
            self.buf.push(b'>');
        }
        Ok(())
    }

    fn push_leaf_node(&mut self, node: &Node) -> Result<()> {
        match node {
            Node::Text(t) => push_escaped_text(&mut self.buf, t),
            Node::CData(c) => push_cdata(&mut self.buf, c),
            Node::Comment(c) => push_comment(&mut self.buf, c),
            Node::ProcessingInstruction { target, data } => {
                push_pi(&mut self.buf, target, data.as_deref())
            }
            Node::Element(_) => Ok(()),
        }
    }

    fn push_indent(&mut self, unit: &str, depth: usize) {
        self.buf.push(b'\n');
        for _ in 0..depth {
            self.buf.extend_from_slice(unit.as_bytes());
        }
    }

    fn document_inner(&mut self, doc: &Document, base_depth: usize) -> Result<()> {
        let indent = self.options.indent.clone();
        let mut wrote_top_level = false;

        if self.options.write_declaration {
            if let Some(decl) = &doc.declaration {
                self.decl_inner(decl)?;
                wrote_top_level = true;
            }
        }

        for node in &doc.prolog {
            if wrote_top_level && indent.is_some() {
                self.buf.push(b'\n');
            }
            self.push_leaf_node(node)?;
            wrote_top_level = true;
        }

        if wrote_top_level && indent.is_some() {
            self.buf.push(b'\n');
        }

        self.write_element_iterative(&doc.root, indent.as_deref(), base_depth)?;

        for node in &doc.epilog {
            if indent.is_some() {
                self.buf.push(b'\n');
            }
            self.push_leaf_node(node)?;
        }

        Ok(())
    }

    fn write_element_iterative(
        &mut self,
        root: &Element,
        indent: Option<&str>,
        base_depth: usize,
    ) -> Result<()> {
        enum Frame<'a> {
            OpenElement {
                el: &'a Element,
                depth: usize,
                pretty_children: bool,
            },
            Node {
                node: &'a Node,
                depth: usize,
                pretty_parent: bool,
            },
            CloseElement {
                name: &'a str,
                depth: usize,
                pretty_children: bool,
            },
        }

        let mut stack: Vec<Frame<'_>> = vec![Frame::OpenElement {
            el: root,
            depth: 0,
            pretty_children: indent.is_some(),
        }];
        let mut attrs: Vec<(&str, &str)> = Vec::new();

        while let Some(frame) = stack.pop() {
            self.flush_if_large()?;
            match frame {
                Frame::OpenElement {
                    el,
                    depth,
                    pretty_children,
                } => {
                    attrs.clear();
                    attrs.extend(
                        el.attributes()
                            .iter()
                            .map(|a| (a.name.as_str(), a.value.as_str())),
                    );

                    let has_significant_children = if pretty_children {
                        el.children().iter().any(|n| match n {
                            Node::Text(t) => !t.chars().all(is_xml_whitespace),
                            _ => true,
                        })
                    } else {
                        !el.children().is_empty()
                    };

                    if !has_significant_children {
                        self.empty_inner(el.name(), &attrs, base_depth + depth)?;
                        continue;
                    }

                    self.open_tag_inner(el.name(), &attrs, false, base_depth + depth)?;

                    let element_only = pretty_children
                        && !el.children().iter().any(|n| match n {
                            Node::Text(t) => !t.chars().all(is_xml_whitespace),
                            Node::CData(_) => true,
                            _ => false,
                        });

                    stack.push(Frame::CloseElement {
                        name: el.name(),
                        depth,
                        pretty_children: element_only,
                    });

                    for child in el.children().iter().rev() {
                        if element_only {
                            if let Node::Text(t) = child {
                                if t.chars().all(is_xml_whitespace) {
                                    continue;
                                }
                            }
                        }
                        stack.push(Frame::Node {
                            node: child,
                            depth: depth + 1,
                            pretty_parent: element_only,
                        });
                    }
                }
                Frame::Node {
                    node,
                    depth,
                    pretty_parent,
                } => {
                    if pretty_parent {
                        if let Some(unit) = indent {
                            self.push_indent(unit, depth);
                        }
                    }
                    match node {
                        Node::Element(child_el) => {
                            stack.push(Frame::OpenElement {
                                el: child_el,
                                depth,
                                pretty_children: indent.is_some() && pretty_parent,
                            });
                        }
                        _ => self.push_leaf_node(node)?,
                    }
                }
                Frame::CloseElement {
                    name,
                    depth,
                    pretty_children,
                } => {
                    if pretty_children {
                        if let Some(unit) = indent {
                            self.push_indent(unit, depth);
                        }
                    }
                    push_close_tag(&mut self.buf, name);
                    self.pop_ns_bindings(base_depth + depth);
                }
            }
        }
        Ok(())
    }
}

/// Only XML 1.0 is emitted (and accepted by the reader); escaping follows XML 1.0 rules.
fn is_valid_version_num(version: &str) -> bool {
    version == "1.0"
}

fn push_close_tag(buf: &mut Vec<u8>, name: &str) {
    buf.extend_from_slice(b"</");
    buf.extend_from_slice(name.as_bytes());
    buf.push(b'>');
}

fn cr_error(expected: &'static str) -> Error {
    Error::new(ErrorKind::UnexpectedToken {
        expected,
        found: "'\\r'".to_string(),
    })
}

fn validate_name(name: &str) -> Result<()> {
    if !is_valid_xml_name(name) {
        return Err(Error::new(ErrorKind::InvalidName {
            name: name.to_string(),
        }));
    }
    Ok(())
}

/// Reject duplicate attribute names (linear for small tags, hashed for large ones).
fn check_duplicate_attrs<'n>(names: impl ExactSizeIterator<Item = &'n str> + Clone) -> Result<()> {
    let dup = |name: &str| {
        Err(Error::new(ErrorKind::DuplicateAttribute {
            name: name.to_string(),
        }))
    };
    if names.len() <= 16 {
        let all: Vec<&str> = names.collect();
        for (i, name) in all.iter().enumerate() {
            if all[..i].contains(name) {
                return dup(name);
            }
        }
    } else {
        let mut seen = HashSet::with_capacity(names.len());
        for name in names {
            if !seen.insert(name) {
                return dup(name);
            }
        }
    }
    Ok(())
}

fn validate_xml_chars(text: &str) -> Result<()> {
    if let Some(ch) = text.chars().find(|&ch| !is_xml_char(ch)) {
        return Err(Error::new(ErrorKind::InvalidXmlChar {
            codepoint: ch as u32,
        }));
    }
    Ok(())
}

fn push_cdata(buf: &mut Vec<u8>, data: &str) -> Result<()> {
    validate_xml_chars(data)?;
    if !data.contains('\r') {
        push_cdata_section(buf, data);
        return Ok(());
    }
    // A raw CR would be normalized to LF by the parser; emit it as `&#xD;` between sections.
    for (i, seg) in data.split('\r').enumerate() {
        if i > 0 {
            buf.extend_from_slice(b"&#xD;");
        }
        if !seg.is_empty() {
            push_cdata_section(buf, seg);
        }
    }
    Ok(())
}

fn push_cdata_section(buf: &mut Vec<u8>, data: &str) {
    buf.extend_from_slice(b"<![CDATA[");
    let mut rest = data;
    while let Some(idx) = rest.find("]]>") {
        // Emit `]]` in this section and start a new one with `>`.
        buf.extend_from_slice(&rest.as_bytes()[..idx + 2]);
        buf.extend_from_slice(b"]]><![CDATA[");
        rest = &rest[idx + 2..];
    }
    buf.extend_from_slice(rest.as_bytes());
    buf.extend_from_slice(b"]]>");
}

fn push_comment(buf: &mut Vec<u8>, comment: &str) -> Result<()> {
    validate_xml_chars(comment)?;
    if comment.contains("--") || comment.ends_with('-') {
        return Err(Error::new(ErrorKind::InvalidComment));
    }
    if comment.contains('\r') {
        return Err(cr_error("comment without CR"));
    }
    buf.extend_from_slice(b"<!--");
    buf.extend_from_slice(comment.as_bytes());
    buf.extend_from_slice(b"-->");
    Ok(())
}

fn push_pi(buf: &mut Vec<u8>, target: &str, data: Option<&str>) -> Result<()> {
    validate_name(target)?;
    if target.eq_ignore_ascii_case("xml") {
        return Err(Error::new(ErrorKind::InvalidPiTarget {
            target: target.to_string(),
        }));
    }
    buf.extend_from_slice(b"<?");
    buf.extend_from_slice(target.as_bytes());
    if let Some(d) = data {
        validate_xml_chars(d)?;
        if d.contains("?>") {
            return Err(Error::new(ErrorKind::UnexpectedToken {
                expected: "PI data without '?>'",
                found: "?>".to_string(),
            }));
        }
        if d.contains('\r') {
            return Err(cr_error("PI data without CR"));
        }
        if d.starts_with(is_xml_whitespace) {
            return Err(Error::new(ErrorKind::UnexpectedToken {
                expected: "PI data without leading whitespace",
                found: "whitespace".to_string(),
            }));
        }
        if !d.is_empty() {
            buf.push(b' ');
            buf.extend_from_slice(d.as_bytes());
        }
    }
    buf.extend_from_slice(b"?>");
    Ok(())
}

/// Append `text` to `buf`, replacing chars per `escape`, copying unescaped runs as slices.
fn push_escaped(
    buf: &mut Vec<u8>,
    text: &str,
    escape: impl Fn(char) -> Option<&'static [u8]>,
) -> Result<()> {
    let mut run_start = 0;
    for (idx, ch) in text.char_indices() {
        if !is_xml_char(ch) {
            return Err(Error::new(ErrorKind::InvalidXmlChar {
                codepoint: ch as u32,
            }));
        }
        if let Some(rep) = escape(ch) {
            buf.extend_from_slice(&text.as_bytes()[run_start..idx]);
            buf.extend_from_slice(rep);
            run_start = idx + ch.len_utf8();
        }
    }
    buf.extend_from_slice(&text.as_bytes()[run_start..]);
    Ok(())
}

fn push_escaped_text(buf: &mut Vec<u8>, text: &str) -> Result<()> {
    push_escaped(buf, text, |ch| match ch {
        '&' => Some(b"&amp;"),
        '<' => Some(b"&lt;"),
        '>' => Some(b"&gt;"),
        // A literal CR would be normalized to LF by any conforming parser.
        '\r' => Some(b"&#xD;"),
        _ => None,
    })
}

fn push_escaped_attr(buf: &mut Vec<u8>, val: &str) -> Result<()> {
    push_escaped(buf, val, |ch| match ch {
        '&' => Some(b"&amp;"),
        '<' => Some(b"&lt;"),
        '"' => Some(b"&quot;"),
        '\t' => Some(b"&#x9;"),
        '\n' => Some(b"&#xA;"),
        '\r' => Some(b"&#xD;"),
        _ => None,
    })
}

/// Serialize a [`Document`] into a compact XML string.
pub fn to_string(doc: &Document) -> Result<String> {
    to_string_with(doc, &SerializeOptions::new())
}

/// Serialize a [`Document`] into a pretty-printed XML string (2-space indent).
pub fn to_string_pretty(doc: &Document) -> Result<String> {
    to_string_with(doc, &SerializeOptions::pretty())
}

/// Serialize a [`Document`] into a string using custom [`SerializeOptions`].
pub fn to_string_with(doc: &Document, options: &SerializeOptions) -> Result<String> {
    let mut buf = Vec::new();
    write_to_with(doc, &mut buf, options)?;
    Ok(String::from_utf8(buf).expect("XML serializer only emits valid UTF-8"))
}

/// Write a [`Document`] in compact form to `writer`.
pub fn write_to<W: Write>(doc: &Document, writer: W) -> Result<()> {
    write_to_with(doc, writer, &SerializeOptions::new())
}

/// Write a [`Document`] in pretty-printed form to `writer`.
pub fn write_to_pretty<W: Write>(doc: &Document, writer: W) -> Result<()> {
    write_to_with(doc, writer, &SerializeOptions::pretty())
}

/// Write a [`Document`] to `writer` using custom [`SerializeOptions`].
pub fn write_to_with<W: Write>(
    doc: &Document,
    writer: W,
    options: &SerializeOptions,
) -> Result<()> {
    let mut w = Writer::with_options(writer, options.clone());
    w.write_document(doc)?;
    let _ = w.finish()?;
    Ok(())
}
