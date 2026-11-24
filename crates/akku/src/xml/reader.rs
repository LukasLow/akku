//! Strict XML 1.0 pull parser (`Reader<'a>`) with zero-copy borrowing and XXE protection.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use crate::xml::error::{Error, ErrorKind, Position, Result};

pub(crate) const XML_NAMESPACE: &str = "http://www.w3.org/XML/1998/namespace";
pub(crate) const XMLNS_NAMESPACE: &str = "http://www.w3.org/2000/xmlns/";

/// How the parser handles `<!DOCTYPE ...>` declarations.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DtdPolicy {
    /// Reject any `<!DOCTYPE` or DTD declaration (`<!ENTITY`, etc.) immediately.
    ///
    /// This is the default and guarantees immunity against XXE and entity-expansion bombs.
    #[default]
    Reject,
    /// Allow a minimal `<!DOCTYPE root>` with no internal subset (`[...]`) and no
    /// external identifier (`SYSTEM` / `PUBLIC`). Internal subsets and external
    /// entities are still rejected.
    IgnoreSimpleDeclaration,
}

/// Configuration and safety limits for XML parsing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseOptions {
    /// Maximum element nesting depth. Default: `128`.
    pub max_depth: usize,
    /// Maximum number of parsed events/nodes before aborting. Default: `250_000`.
    pub max_nodes: usize,
    /// Maximum number of attributes allowed on a single element. Default: `256`.
    pub max_attributes: usize,
    /// Maximum length in bytes of any single text, attribute, comment, CDATA, or PI payload.
    /// Default: `Some(16 * 1024 * 1024)` (16 MiB).
    pub max_token_len: Option<usize>,
    /// Policy for `<!DOCTYPE ...>` declarations. Default: [`DtdPolicy::Reject`].
    pub dtd_policy: DtdPolicy,
    /// Whether DOM building preserves whitespace-only text nodes inside elements. Default: `true`.
    pub preserve_whitespace: bool,
    /// Whether DOM building preserves comment nodes. Default: `true`.
    pub preserve_comments: bool,
    /// Whether DOM building preserves processing instruction nodes. Default: `true`.
    pub preserve_pis: bool,
    /// Whether to enforce XML Namespaces 1.0 well-formedness (QName syntax, bound prefixes,
    /// and duplicate `(namespace, local_name)` attributes). Default: `true`.
    pub check_namespace_well_formed: bool,
}

impl Default for ParseOptions {
    fn default() -> Self {
        Self::new()
    }
}

impl ParseOptions {
    /// Strict, XXE-safe XML 1.0 defaults.
    pub const fn new() -> Self {
        Self {
            max_depth: 128,
            max_nodes: 250_000,
            max_attributes: 256,
            max_token_len: Some(16 * 1024 * 1024),
            dtd_policy: DtdPolicy::Reject,
            preserve_whitespace: true,
            preserve_comments: true,
            preserve_pis: true,
            check_namespace_well_formed: true,
        }
    }
}

/// A parsed `<?xml ...?>` declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XmlDecl {
    /// XML version string (always `"1.0"` for valid XML 1.0 declarations).
    pub version: String,
    /// Optional encoding declaration (e.g. `"UTF-8"`).
    pub encoding: Option<String>,
    /// Optional standalone declaration (`Some(true)` for `"yes"`, `Some(false)` for `"no"`).
    pub standalone: Option<bool>,
}

/// A single attribute on a start or empty element tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawAttr<'a> {
    /// Raw attribute name (possibly prefixed, e.g. `"xlink:href"`).
    pub name: &'a str,
    /// Decoded and whitespace-normalized attribute value.
    pub value: Cow<'a, str>,
}

impl<'a> RawAttr<'a> {
    /// Optional namespace prefix before `:`.
    pub fn prefix(&self) -> Option<&'a str> {
        self.name.split_once(':').map(|(p, _)| p)
    }

    /// Local attribute name after `:`, or the full name if unprefixed.
    pub fn local_name(&self) -> &'a str {
        self.name.split_once(':').map_or(self.name, |(_, l)| l)
    }
}

/// Payload for [`Event::Start`] and [`Event::Empty`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartTag<'a> {
    /// Raw element name.
    pub name: &'a str,
    attributes: Vec<RawAttr<'a>>,
}

impl<'a> StartTag<'a> {
    /// Create a start tag event payload.
    pub fn new(name: &'a str, attributes: Vec<RawAttr<'a>>) -> Self {
        Self { name, attributes }
    }

    /// Raw element name (e.g. `"svg:path"`).
    pub fn name(&self) -> &'a str {
        self.name
    }

    /// Namespace prefix before `:`, if present.
    pub fn prefix(&self) -> Option<&'a str> {
        self.name.split_once(':').map(|(p, _)| p)
    }

    /// Local name after `:`, or the full name if unprefixed.
    pub fn local_name(&self) -> &'a str {
        self.name.split_once(':').map_or(self.name, |(_, l)| l)
    }

    /// All attributes on this tag in source order.
    pub fn attributes(&self) -> &[RawAttr<'a>] {
        &self.attributes
    }

    /// Look up an attribute value by exact raw name.
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|a| a.name == name)
            .map(|a| a.value.as_ref())
    }
}

/// One event yielded by [`Reader::next_event`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event<'a> {
    /// The `<?xml ...?>` declaration at the very beginning of the document.
    Decl(XmlDecl),
    /// An opening element `<name ...>`.
    Start(StartTag<'a>),
    /// A self-closing element `<name .../>`.
    Empty(StartTag<'a>),
    /// A closing element `</name>`.
    End(&'a str),
    /// Character data inside an element (entities decoded, line endings normalized).
    Text(Cow<'a, str>),
    /// A `<![CDATA[...]]>` section (raw except `\r\n`/`\r` normalized to `\n`).
    CData(Cow<'a, str>),
    /// A `<!-- ... -->` comment.
    Comment(Cow<'a, str>),
    /// A `<?target ...?>` processing instruction.
    PI {
        /// Target name.
        target: &'a str,
        /// Optional instruction data.
        data: Option<Cow<'a, str>>,
    },
    /// End of document (root element closed and trailing input consumed).
    Eof,
}

/// Streaming XML 1.0 pull parser.
///
/// Enforces XML 1.0 well-formedness without recursion, rejecting external entities,
/// DTD entity bombs, invalid characters, mismatched tags, and duplicate attributes.
///
/// The reader is *poisoned* after the first error: every later call to
/// [`Reader::next_event`] returns [`ErrorKind::ReaderPoisoned`]. Well-formedness errors are
/// fatal (XML 1.0 §1.2), so parsing never resumes past malformed input.
pub struct Reader<'a> {
    input: &'a str,
    pos: Position,
    options: ParseOptions,
    open_tags: Vec<&'a str>,
    /// prefix -> stack of in-scope URIs (innermost last); O(1) lookup.
    ns_map: HashMap<&'a str, Vec<String>>,
    /// Undo log of `(depth, prefix)` bindings, popped when an element closes.
    ns_undo: Vec<(usize, &'a str)>,
    at_start: bool,
    seen_root: bool,
    finished: bool,
    poisoned: bool,
    nodes_seen: usize,
}

impl<'a> Reader<'a> {
    /// Create a pull parser over UTF-8 text with default strict [`ParseOptions`].
    pub fn new(input: &'a str) -> Self {
        Self::with_options(input, ParseOptions::new())
    }

    /// Create a pull parser with custom [`ParseOptions`].
    pub fn with_options(input: &'a str, options: ParseOptions) -> Self {
        let trimmed = input.strip_prefix('\u{FEFF}').unwrap_or(input);
        Self {
            input: trimmed,
            pos: Position::start(),
            options,
            open_tags: Vec::new(),
            ns_map: HashMap::new(),
            ns_undo: Vec::new(),
            at_start: true,
            seen_root: false,
            finished: false,
            poisoned: false,
            nodes_seen: 0,
        }
    }

    /// Create a pull parser from UTF-8 bytes (stripping an optional UTF-8 BOM).
    pub fn from_bytes(bytes: &'a [u8]) -> Result<Self> {
        Self::from_bytes_with(bytes, ParseOptions::new())
    }

    /// Create a pull parser from UTF-8 bytes with custom [`ParseOptions`].
    pub fn from_bytes_with(bytes: &'a [u8], options: ParseOptions) -> Result<Self> {
        let stripped = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
        let text = std::str::from_utf8(stripped)
            .map_err(|_| Error::at(ErrorKind::InvalidUtf8, Position::start()))?;
        Ok(Self::with_options(text, options))
    }

    /// Current byte/line/column position in the input.
    pub fn position(&self) -> Position {
        self.pos
    }

    /// Current element nesting depth.
    pub fn depth(&self) -> usize {
        self.open_tags.len()
    }

    /// Configured parser options.
    pub fn options(&self) -> &ParseOptions {
        &self.options
    }

    /// Pull the next [`Event`] from the XML stream.
    ///
    /// After an error, the reader is poisoned and every further call returns
    /// [`ErrorKind::ReaderPoisoned`].
    pub fn next_event(&mut self) -> Result<Event<'a>> {
        if self.poisoned {
            return Err(Error::at(ErrorKind::ReaderPoisoned, self.pos));
        }
        if self.finished {
            return Ok(Event::Eof);
        }
        let result = self.next_event_inner();
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    fn next_event_inner(&mut self) -> Result<Event<'a>> {
        loop {
            if self.pos.offset >= self.input.len() {
                if let Some(unclosed) = self.open_tags.last() {
                    return Err(Error::at(
                        ErrorKind::UnclosedTag {
                            name: (*unclosed).to_string(),
                        },
                        self.pos,
                    ));
                }
                if !self.seen_root {
                    return Err(Error::at(ErrorKind::MissingRootElement, self.pos));
                }
                self.finished = true;
                return Ok(Event::Eof);
            }

            if self.open_tags.is_empty() {
                if self.at_start && self.rest().starts_with("<?xml") && self.is_xml_decl_prefix() {
                    self.at_start = false;
                    let decl = self.parse_xml_decl()?;
                    return Ok(Event::Decl(decl));
                }

                // Prolog or epilog: skip XML whitespace, parse comments/PIs/root/doctype.
                let ws_start = self.pos;
                let ws = self.consume_xml_whitespace()?;
                if !ws.is_empty() {
                    self.at_start = false;
                    continue;
                }
                if self.pos.offset >= self.input.len() {
                    continue;
                }
                let _ = ws_start;

                if !self.rest().starts_with('<') {
                    let kind = if self.seen_root {
                        ErrorKind::TrailingContentAfterRoot
                    } else {
                        ErrorKind::MissingRootElement
                    };
                    return Err(Error::at(kind, self.pos));
                }
            } else {
                // Inside an element: text until '<' or markup.
                if !self.rest().starts_with('<') {
                    self.at_start = false;
                    let text = self.parse_text()?;
                    self.bump_node_count(self.pos)?;
                    return Ok(Event::Text(text));
                }
            }

            self.at_start = false;
            let rest = self.rest();
            if rest.starts_with("<!--") {
                let comment = self.parse_comment()?;
                self.bump_node_count(self.pos)?;
                return Ok(Event::Comment(comment));
            } else if rest.starts_with("<![CDATA[") {
                if self.open_tags.is_empty() {
                    let kind = if self.seen_root {
                        ErrorKind::TrailingContentAfterRoot
                    } else {
                        ErrorKind::MissingRootElement
                    };
                    return Err(Error::at(kind, self.pos));
                }
                let cdata = self.parse_cdata()?;
                self.bump_node_count(self.pos)?;
                return Ok(Event::CData(cdata));
            } else if rest.starts_with("<?") {
                let (target, data) = self.parse_pi()?;
                self.bump_node_count(self.pos)?;
                return Ok(Event::PI { target, data });
            } else if rest.starts_with("<!") {
                self.handle_bang_decl()?;
                continue;
            } else if rest.starts_with("</") {
                let end_name = self.parse_end_tag()?;
                return Ok(Event::End(end_name));
            } else {
                if self.seen_root && self.open_tags.is_empty() {
                    return Err(Error::at(ErrorKind::TrailingContentAfterRoot, self.pos));
                }
                let (tag, is_empty) = self.parse_start_or_empty_tag()?;
                self.bump_node_count(self.pos)?;
                return Ok(if is_empty {
                    Event::Empty(tag)
                } else {
                    Event::Start(tag)
                });
            }
        }
    }

    #[inline]
    fn rest(&self) -> &'a str {
        &self.input[self.pos.offset..]
    }

    fn is_xml_decl_prefix(&self) -> bool {
        let after = &self.rest()["<?xml".len()..];
        after
            .chars()
            .next()
            .is_some_and(|c| is_xml_whitespace(c) || c == '?')
    }

    fn bump_node_count(&mut self, pos: Position) -> Result<()> {
        self.nodes_seen += 1;
        if self.nodes_seen > self.options.max_nodes {
            return Err(Error::at(
                ErrorKind::MaxNodesExceeded {
                    limit: self.options.max_nodes,
                },
                pos,
            ));
        }
        Ok(())
    }

    fn check_token_len(&self, len: usize, pos: Position) -> Result<()> {
        if let Some(limit) = self.options.max_token_len {
            if len > limit {
                return Err(Error::at(ErrorKind::MaxTokenLenExceeded { limit }, pos));
            }
        }
        Ok(())
    }

    fn advance_char(&mut self) -> Option<char> {
        let ch = self.rest().chars().next()?;
        let len = ch.len_utf8();
        self.pos.offset += len;
        if ch == '\r' {
            if self.rest().starts_with('\n') {
                self.pos.offset += 1;
            }
            self.pos.line += 1;
            self.pos.column = 1;
        } else if ch == '\n' {
            self.pos.line += 1;
            self.pos.column = 1;
        } else {
            self.pos.column += 1;
        }
        Some(ch)
    }

    fn advance_bytes_no_newline(&mut self, byte_len: usize, char_count: u32) {
        self.pos.offset += byte_len;
        self.pos.column += char_count;
    }

    fn expect_str(&mut self, lit: &str, expected: &'static str) -> Result<()> {
        if self.rest().starts_with(lit) {
            for _ in lit.chars() {
                self.advance_char();
            }
            Ok(())
        } else {
            let found = self.rest().chars().take(8).collect::<String>();
            let found = if found.is_empty() {
                "EOF".to_string()
            } else {
                format!("'{found}'")
            };
            Err(Error::at(
                ErrorKind::UnexpectedToken { expected, found },
                self.pos,
            ))
        }
    }

    fn consume_xml_whitespace(&mut self) -> Result<&'a str> {
        let start = self.pos.offset;
        while let Some(ch) = self.rest().chars().next() {
            if is_xml_whitespace(ch) {
                self.advance_char();
            } else {
                break;
            }
        }
        Ok(&self.input[start..self.pos.offset])
    }

    fn parse_name(&mut self) -> Result<&'a str> {
        let start_pos = self.pos;
        let rest = self.rest();
        let mut iter = rest.char_indices();
        let Some((_, first)) = iter.next() else {
            return Err(Error::at(ErrorKind::UnexpectedEof, start_pos));
        };
        if !is_xml_char(first) {
            return Err(Error::at(
                ErrorKind::InvalidXmlChar {
                    codepoint: first as u32,
                },
                start_pos,
            ));
        }
        if !is_name_start_char(first) {
            return Err(Error::at(
                ErrorKind::InvalidName {
                    name: first.to_string(),
                },
                start_pos,
            ));
        }

        let mut end = first.len_utf8();
        let mut chars = 1u32;
        for (idx, ch) in iter {
            if is_name_char(ch) {
                end = idx + ch.len_utf8();
                chars += 1;
            } else {
                break;
            }
        }

        let name = &rest[..end];
        self.advance_bytes_no_newline(end, chars);
        self.check_token_len(name.len(), start_pos)?;
        Ok(name)
    }

    fn validate_qname(&self, name: &str, is_element: bool, pos: Position) -> Result<()> {
        if !self.options.check_namespace_well_formed {
            return Ok(());
        }
        match qname_error(name, is_element) {
            Some(kind) => Err(Error::at(kind, pos)),
            None => Ok(()),
        }
    }

    fn parse_xml_decl(&mut self) -> Result<XmlDecl> {
        let decl_pos = self.pos;
        self.expect_str("<?xml", "'<?xml'")?;
        let ws = self.consume_xml_whitespace()?;
        if ws.is_empty() {
            return Err(Error::at(ErrorKind::InvalidDecl, self.pos));
        }

        // version is mandatory and must come first
        let (attr_name, version) = self.parse_decl_attr()?;
        if attr_name != "version" || version != "1.0" {
            return Err(Error::at(ErrorKind::InvalidDecl, decl_pos));
        }

        let mut encoding: Option<String> = None;
        let mut standalone: Option<bool> = None;
        let mut seen_encoding = false;
        let mut seen_standalone = false;

        loop {
            let ws = self.consume_xml_whitespace()?;
            if self.rest().starts_with("?>") {
                self.expect_str("?>", "'?>'")?;
                break;
            }
            if ws.is_empty() {
                return Err(Error::at(ErrorKind::InvalidDecl, self.pos));
            }
            let (name, val) = self.parse_decl_attr()?;
            match name {
                "encoding" if !seen_encoding && !seen_standalone => {
                    if !is_valid_encoding_name(&val) {
                        return Err(Error::at(ErrorKind::InvalidDecl, self.pos));
                    }
                    encoding = Some(val.into_owned());
                    seen_encoding = true;
                }
                "standalone" if !seen_standalone => {
                    standalone = match val.as_ref() {
                        "yes" => Some(true),
                        "no" => Some(false),
                        _ => return Err(Error::at(ErrorKind::InvalidDecl, self.pos)),
                    };
                    seen_standalone = true;
                }
                _ => return Err(Error::at(ErrorKind::InvalidDecl, self.pos)),
            }
        }

        Ok(XmlDecl {
            version: version.into_owned(),
            encoding,
            standalone,
        })
    }

    fn parse_decl_attr(&mut self) -> Result<(&'a str, Cow<'a, str>)> {
        let name = self.parse_name()?;
        self.consume_xml_whitespace()?;
        self.expect_str("=", "'='")?;
        self.consume_xml_whitespace()?;
        let quote = match self.rest().chars().next() {
            Some(q @ ('\'' | '"')) => {
                self.advance_char();
                q
            }
            Some(other) => {
                return Err(Error::at(
                    ErrorKind::UnexpectedToken {
                        expected: "quote",
                        found: format!("'{other}'"),
                    },
                    self.pos,
                ));
            }
            None => return Err(Error::at(ErrorKind::UnexpectedEof, self.pos)),
        };
        let start = self.pos.offset;
        loop {
            let Some(ch) = self.rest().chars().next() else {
                return Err(Error::at(ErrorKind::UnexpectedEof, self.pos));
            };
            if ch == quote {
                let val = &self.input[start..self.pos.offset];
                self.advance_char();
                return Ok((name, Cow::Borrowed(val)));
            }
            if !is_xml_char(ch) || ch == '<' || ch == '&' {
                return Err(Error::at(ErrorKind::InvalidDecl, self.pos));
            }
            self.advance_char();
        }
    }

    fn handle_bang_decl(&mut self) -> Result<()> {
        let start_pos = self.pos;
        if self.rest().starts_with("<!DOCTYPE") {
            let after = &self.rest()["<!DOCTYPE".len()..];
            let next_ok = after.chars().next().is_some_and(is_xml_whitespace);
            if !next_ok {
                return Err(Error::at(ErrorKind::DtdNotAllowed, start_pos));
            }
            if self.options.dtd_policy == DtdPolicy::Reject {
                return Err(Error::at(ErrorKind::DtdNotAllowed, start_pos));
            }

            // DtdPolicy::IgnoreSimpleDeclaration: allow ONLY `<!DOCTYPE Name >`.
            self.expect_str("<!DOCTYPE", "'<!DOCTYPE'")?;
            self.consume_xml_whitespace()?;
            let _name = self.parse_name()?;
            self.consume_xml_whitespace()?;
            if self.rest().starts_with("SYSTEM") || self.rest().starts_with("PUBLIC") {
                return Err(Error::at(ErrorKind::ExternalEntityNotAllowed, self.pos));
            }
            if self.rest().starts_with('[') {
                return Err(Error::at(ErrorKind::DtdNotAllowed, self.pos));
            }
            if !self.rest().starts_with('>') {
                return Err(Error::at(ErrorKind::DtdNotAllowed, self.pos));
            }
            self.advance_char();
            return Ok(());
        }
        Err(Error::at(ErrorKind::DtdNotAllowed, start_pos))
    }

    fn parse_comment(&mut self) -> Result<Cow<'a, str>> {
        let start_pos = self.pos;
        self.expect_str("<!--", "'<!--'")?;
        let raw_start = self.pos.offset;
        let mut has_cr = false;
        let mut owned = String::new();
        let mut copied_until = raw_start;

        loop {
            if self.rest().starts_with("-->") {
                let raw_end = self.pos.offset;
                if self.input[raw_start..raw_end].ends_with('-') {
                    return Err(Error::at(ErrorKind::InvalidComment, self.pos));
                }
                let len = raw_end - raw_start;
                self.check_token_len(len, start_pos)?;
                self.expect_str("-->", "'-->'")?;
                if !has_cr {
                    return Ok(Cow::Borrowed(&self.input[raw_start..raw_end]));
                }
                owned.push_str(&self.input[copied_until..raw_end]);
                return Ok(Cow::Owned(owned));
            }
            if self.rest().starts_with("--") {
                return Err(Error::at(ErrorKind::InvalidComment, self.pos));
            }
            let Some(ch) = self.rest().chars().next() else {
                return Err(Error::at(ErrorKind::UnexpectedEof, self.pos));
            };
            if !is_xml_char(ch) {
                return Err(Error::at(
                    ErrorKind::InvalidXmlChar {
                        codepoint: ch as u32,
                    },
                    self.pos,
                ));
            }
            if ch == '\r' {
                if !has_cr {
                    has_cr = true;
                }
                owned.push_str(&self.input[copied_until..self.pos.offset]);
                owned.push('\n');
                self.advance_char();
                copied_until = self.pos.offset;
            } else {
                self.advance_char();
            }
            if let Some(limit) = self.options.max_token_len {
                if self.pos.offset - raw_start > limit {
                    return Err(Error::at(
                        ErrorKind::MaxTokenLenExceeded { limit },
                        start_pos,
                    ));
                }
            }
        }
    }

    fn parse_cdata(&mut self) -> Result<Cow<'a, str>> {
        let start_pos = self.pos;
        self.expect_str("<![CDATA[", "'<![CDATA['")?;
        let raw_start = self.pos.offset;
        let mut has_cr = false;
        let mut owned = String::new();
        let mut copied_until = raw_start;

        loop {
            if self.rest().starts_with("]]>") {
                let raw_end = self.pos.offset;
                self.check_token_len(raw_end - raw_start, start_pos)?;
                self.expect_str("]]>", "']]>'")?;
                if !has_cr {
                    return Ok(Cow::Borrowed(&self.input[raw_start..raw_end]));
                }
                owned.push_str(&self.input[copied_until..raw_end]);
                return Ok(Cow::Owned(owned));
            }
            let Some(ch) = self.rest().chars().next() else {
                return Err(Error::at(ErrorKind::UnexpectedEof, self.pos));
            };
            if !is_xml_char(ch) {
                return Err(Error::at(
                    ErrorKind::InvalidXmlChar {
                        codepoint: ch as u32,
                    },
                    self.pos,
                ));
            }
            if ch == '\r' {
                has_cr = true;
                owned.push_str(&self.input[copied_until..self.pos.offset]);
                owned.push('\n');
                self.advance_char();
                copied_until = self.pos.offset;
            } else {
                self.advance_char();
            }
            if let Some(limit) = self.options.max_token_len {
                if self.pos.offset - raw_start > limit {
                    return Err(Error::at(
                        ErrorKind::MaxTokenLenExceeded { limit },
                        start_pos,
                    ));
                }
            }
        }
    }

    fn parse_pi(&mut self) -> Result<(&'a str, Option<Cow<'a, str>>)> {
        let start_pos = self.pos;
        self.expect_str("<?", "'<?'")?;
        let target_pos = self.pos;
        let target = self.parse_name()?;
        if target.eq_ignore_ascii_case("xml") {
            return Err(Error::at(
                ErrorKind::InvalidPiTarget {
                    target: target.to_string(),
                },
                target_pos,
            ));
        }

        if self.rest().starts_with("?>") {
            self.expect_str("?>", "'?>'")?;
            return Ok((target, None));
        }
        let ws = self.consume_xml_whitespace()?;
        if ws.is_empty() {
            return Err(Error::at(
                ErrorKind::UnexpectedToken {
                    expected: "whitespace or '?>'",
                    found: self.rest().chars().take(4).collect::<String>(),
                },
                self.pos,
            ));
        }

        let raw_start = self.pos.offset;
        let mut has_cr = false;
        let mut owned = String::new();
        let mut copied_until = raw_start;

        loop {
            if self.rest().starts_with("?>") {
                let raw_end = self.pos.offset;
                self.check_token_len(raw_end - raw_start, start_pos)?;
                self.expect_str("?>", "'?>'")?;
                let data = if !has_cr {
                    Cow::Borrowed(&self.input[raw_start..raw_end])
                } else {
                    owned.push_str(&self.input[copied_until..raw_end]);
                    Cow::Owned(owned)
                };
                return Ok((target, if data.is_empty() { None } else { Some(data) }));
            }
            let Some(ch) = self.rest().chars().next() else {
                return Err(Error::at(ErrorKind::UnexpectedEof, self.pos));
            };
            if !is_xml_char(ch) {
                return Err(Error::at(
                    ErrorKind::InvalidXmlChar {
                        codepoint: ch as u32,
                    },
                    self.pos,
                ));
            }
            if ch == '\r' {
                has_cr = true;
                owned.push_str(&self.input[copied_until..self.pos.offset]);
                owned.push('\n');
                self.advance_char();
                copied_until = self.pos.offset;
            } else {
                self.advance_char();
            }
            if let Some(limit) = self.options.max_token_len {
                if self.pos.offset - raw_start > limit {
                    return Err(Error::at(
                        ErrorKind::MaxTokenLenExceeded { limit },
                        start_pos,
                    ));
                }
            }
        }
    }

    fn parse_text(&mut self) -> Result<Cow<'a, str>> {
        let start_pos = self.pos;
        let raw_start = self.pos.offset;
        let mut owned = String::new();
        let mut use_owned = false;
        let mut copied_until = raw_start;
        let mut bracket_run = 0usize;

        while let Some(ch) = self.rest().chars().next() {
            if ch == '<' {
                break;
            }
            if !is_xml_char(ch) {
                return Err(Error::at(
                    ErrorKind::InvalidXmlChar {
                        codepoint: ch as u32,
                    },
                    self.pos,
                ));
            }
            if ch == ']' {
                bracket_run += 1;
                self.advance_char();
                continue;
            }
            if ch == '>' && bracket_run >= 2 {
                return Err(Error::at(ErrorKind::UnescapedCdataEndInText, self.pos));
            }
            bracket_run = 0;

            if ch == '&' {
                if !use_owned {
                    use_owned = true;
                }
                owned.push_str(&self.input[copied_until..self.pos.offset]);
                let decoded = self.parse_reference()?;
                owned.push(decoded);
                copied_until = self.pos.offset;
            } else if ch == '\r' {
                if !use_owned {
                    use_owned = true;
                }
                owned.push_str(&self.input[copied_until..self.pos.offset]);
                owned.push('\n');
                self.advance_char();
                copied_until = self.pos.offset;
            } else {
                self.advance_char();
            }

            if let Some(limit) = self.options.max_token_len {
                let approx_len = if use_owned {
                    owned.len() + (self.pos.offset - copied_until)
                } else {
                    self.pos.offset - raw_start
                };
                if approx_len > limit {
                    return Err(Error::at(
                        ErrorKind::MaxTokenLenExceeded { limit },
                        start_pos,
                    ));
                }
            }
        }

        if !use_owned {
            let slice = &self.input[raw_start..self.pos.offset];
            self.check_token_len(slice.len(), start_pos)?;
            Ok(Cow::Borrowed(slice))
        } else {
            owned.push_str(&self.input[copied_until..self.pos.offset]);
            self.check_token_len(owned.len(), start_pos)?;
            Ok(Cow::Owned(owned))
        }
    }

    fn parse_reference(&mut self) -> Result<char> {
        let ref_pos = self.pos;
        self.expect_str("&", "'&'")?;
        let start = self.pos.offset;
        loop {
            let Some(ch) = self.rest().chars().next() else {
                return Err(Error::at(ErrorKind::UnexpectedEof, self.pos));
            };
            if ch == ';' {
                let body = &self.input[start..self.pos.offset];
                self.advance_char();
                return decode_entity_body(body, ref_pos);
            }
            if !is_xml_char(ch) || ch == '<' || ch == '&' || is_xml_whitespace(ch) {
                return Err(Error::at(
                    ErrorKind::InvalidCharRef {
                        raw: self.input[start..self.pos.offset].to_string(),
                    },
                    ref_pos,
                ));
            }
            self.advance_char();
            if self.pos.offset - start > 64 {
                return Err(Error::at(
                    ErrorKind::InvalidCharRef {
                        raw: self.input[start..self.pos.offset].to_string(),
                    },
                    ref_pos,
                ));
            }
        }
    }

    fn parse_start_or_empty_tag(&mut self) -> Result<(StartTag<'a>, bool)> {
        let next_depth = self.open_tags.len() + 1;
        let result = self.parse_start_or_empty_tag_inner();
        if result.is_err() {
            // Roll back any `xmlns` bindings this tag pushed before failing.
            self.pop_ns_bindings(next_depth);
        }
        result
    }

    fn parse_start_or_empty_tag_inner(&mut self) -> Result<(StartTag<'a>, bool)> {
        let tag_pos = self.pos;
        self.expect_str("<", "'<'")?;
        let name_pos = self.pos;
        let name = self.parse_name()?;
        self.validate_qname(name, true, name_pos)?;

        let mut attributes: Vec<RawAttr<'a>> = Vec::new();
        let mut seen_names: DupSet<&'a str> = DupSet::new();
        let next_depth = self.open_tags.len() + 1;
        let mut added_ns_count = 0usize;

        let is_empty = loop {
            let ws = self.consume_xml_whitespace()?;
            if self.rest().starts_with("/>") {
                self.expect_str("/>", "'/>'")?;
                break true;
            }
            if self.rest().starts_with('>') {
                self.advance_char();
                break false;
            }
            if ws.is_empty() {
                return Err(Error::at(
                    ErrorKind::UnexpectedToken {
                        expected: "whitespace, '>', or '/>'",
                        found: self.rest().chars().take(4).collect(),
                    },
                    self.pos,
                ));
            }
            if attributes.len() >= self.options.max_attributes {
                return Err(Error::at(
                    ErrorKind::MaxAttributesExceeded {
                        limit: self.options.max_attributes,
                    },
                    self.pos,
                ));
            }

            let attr_pos = self.pos;
            let attr_name = self.parse_name()?;
            self.validate_qname(attr_name, false, attr_pos)?;
            if !seen_names.insert(attr_name) {
                return Err(Error::at(
                    ErrorKind::DuplicateAttribute {
                        name: attr_name.to_string(),
                    },
                    attr_pos,
                ));
            }

            self.consume_xml_whitespace()?;
            self.expect_str("=", "'='")?;
            self.consume_xml_whitespace()?;
            let attr_value = self.parse_attr_value()?;

            if self.options.check_namespace_well_formed {
                match ns_decl_prefix(attr_name, attr_value.as_ref()) {
                    Ok(Some(prefix)) => {
                        self.push_ns_binding(next_depth, prefix, attr_value.to_string());
                        added_ns_count += 1;
                    }
                    Ok(None) => {}
                    Err(kind) => return Err(Error::at(kind, attr_pos)),
                }
            }

            attributes.push(RawAttr {
                name: attr_name,
                value: attr_value,
            });
        };

        if self.options.check_namespace_well_formed {
            let names = attributes.iter().map(|a| a.name);
            if let Err((kind, on_element)) =
                check_tag_namespaces(name, names, |p| self.resolve_prefix(p))
            {
                let pos = if on_element { name_pos } else { tag_pos };
                return Err(Error::at(kind, pos));
            }
        }

        if is_empty {
            if added_ns_count > 0 {
                self.pop_ns_bindings(next_depth);
            }
            if !self.seen_root && self.open_tags.is_empty() {
                self.seen_root = true;
            }
        } else {
            if self.open_tags.len() >= self.options.max_depth {
                if added_ns_count > 0 {
                    self.pop_ns_bindings(next_depth);
                }
                return Err(Error::at(
                    ErrorKind::MaxDepthExceeded {
                        limit: self.options.max_depth,
                    },
                    tag_pos,
                ));
            }
            if !self.seen_root && self.open_tags.is_empty() {
                self.seen_root = true;
            }
            self.open_tags.push(name);
        }

        Ok((StartTag::new(name, attributes), is_empty))
    }

    fn parse_attr_value(&mut self) -> Result<Cow<'a, str>> {
        let start_pos = self.pos;
        let quote = match self.rest().chars().next() {
            Some(q @ ('\'' | '"')) => {
                self.advance_char();
                q
            }
            Some(other) => {
                return Err(Error::at(
                    ErrorKind::UnexpectedToken {
                        expected: "attribute quote",
                        found: format!("'{other}'"),
                    },
                    self.pos,
                ));
            }
            None => return Err(Error::at(ErrorKind::UnexpectedEof, self.pos)),
        };

        let raw_start = self.pos.offset;
        let mut owned = String::new();
        let mut use_owned = false;
        let mut copied_until = raw_start;

        loop {
            let Some(ch) = self.rest().chars().next() else {
                return Err(Error::at(ErrorKind::UnexpectedEof, self.pos));
            };
            if ch == quote {
                let end = self.pos.offset;
                self.advance_char();
                if !use_owned {
                    let slice = &self.input[raw_start..end];
                    self.check_token_len(slice.len(), start_pos)?;
                    return Ok(Cow::Borrowed(slice));
                }
                owned.push_str(&self.input[copied_until..end]);
                self.check_token_len(owned.len(), start_pos)?;
                return Ok(Cow::Owned(owned));
            }
            if ch == '<' {
                return Err(Error::at(
                    ErrorKind::UnexpectedToken {
                        expected: "attribute character",
                        found: "'<'".to_string(),
                    },
                    self.pos,
                ));
            }
            if !is_xml_char(ch) {
                return Err(Error::at(
                    ErrorKind::InvalidXmlChar {
                        codepoint: ch as u32,
                    },
                    self.pos,
                ));
            }

            if ch == '&' {
                use_owned = true;
                owned.push_str(&self.input[copied_until..self.pos.offset]);
                let decoded = self.parse_reference()?;
                owned.push(decoded);
                copied_until = self.pos.offset;
            } else if ch == '\t' || ch == '\n' || ch == '\r' {
                // XML 1.0 §3.3.3 attribute value normalization: raw whitespace chars become ' '.
                use_owned = true;
                owned.push_str(&self.input[copied_until..self.pos.offset]);
                owned.push(' ');
                self.advance_char();
                copied_until = self.pos.offset;
            } else {
                self.advance_char();
            }

            if let Some(limit) = self.options.max_token_len {
                let approx_len = if use_owned {
                    owned.len() + (self.pos.offset - copied_until)
                } else {
                    self.pos.offset - raw_start
                };
                if approx_len > limit {
                    return Err(Error::at(
                        ErrorKind::MaxTokenLenExceeded { limit },
                        start_pos,
                    ));
                }
            }
        }
    }

    fn parse_end_tag(&mut self) -> Result<&'a str> {
        let tag_pos = self.pos;
        self.expect_str("</", "'</'")?;
        let name_pos = self.pos;
        let name = self.parse_name()?;
        self.validate_qname(name, true, name_pos)?;
        self.consume_xml_whitespace()?;
        self.expect_str(">", "'>'")?;

        let closing_depth = self.open_tags.len();
        let Some(expected) = self.open_tags.pop() else {
            return Err(Error::at(
                ErrorKind::UnexpectedEndTag {
                    name: name.to_string(),
                },
                tag_pos,
            ));
        };
        if expected != name {
            // Keep the element open so `depth()` and namespace scope stay consistent.
            self.open_tags.push(expected);
            return Err(Error::at(
                ErrorKind::MismatchedTag {
                    expected: expected.to_string(),
                    found: name.to_string(),
                },
                tag_pos,
            ));
        }
        self.pop_ns_bindings(closing_depth);
        Ok(name)
    }

    fn resolve_prefix(&self, prefix: &str) -> Option<&str> {
        resolve_in(&self.ns_map, prefix)
    }

    fn push_ns_binding(&mut self, depth: usize, prefix: &'a str, uri: String) {
        self.ns_map.entry(prefix).or_default().push(uri);
        self.ns_undo.push((depth, prefix));
    }

    fn pop_ns_bindings(&mut self, depth: usize) {
        while let Some(&(d, prefix)) = self.ns_undo.last() {
            if d < depth {
                break;
            }
            self.ns_undo.pop();
            if let Some(stack) = self.ns_map.get_mut(prefix) {
                stack.pop();
                if stack.is_empty() {
                    self.ns_map.remove(prefix);
                }
            }
        }
    }
}

/// Resolve `prefix` in a prefix -> URI-stack map (`xml` is always bound).
pub(crate) fn resolve_in<'m, K>(map: &'m HashMap<K, Vec<String>>, prefix: &str) -> Option<&'m str>
where
    K: std::borrow::Borrow<str> + Eq + std::hash::Hash,
{
    if prefix == "xml" {
        return Some(XML_NAMESPACE);
    }
    map.get(prefix)
        .and_then(|stack| stack.last())
        .map(String::as_str)
}

/// XML Namespaces 1.0 QName syntax check (`a:b:c`, empty prefix/local, `xmlns:` elements).
pub(crate) fn qname_error(name: &str, is_element: bool) -> Option<ErrorKind> {
    let (prefix, local) = name.split_once(':')?;
    if prefix.is_empty() || local.is_empty() || local.contains(':') {
        return Some(ErrorKind::InvalidName {
            name: name.to_string(),
        });
    }
    if is_element && prefix == "xmlns" {
        return Some(ErrorKind::InvalidNamespace {
            reason: format!("element '{name}' cannot use reserved 'xmlns' prefix"),
        });
    }
    None
}

/// Classify a namespace declaration attribute.
///
/// Returns `Ok(Some(prefix))` for `xmlns` (prefix `""`) or `xmlns:prefix`, `Ok(None)` for
/// ordinary attributes, and an error for reserved or otherwise illegal bindings.
pub(crate) fn ns_decl_prefix<'n>(
    attr_name: &'n str,
    uri: &str,
) -> std::result::Result<Option<&'n str>, ErrorKind> {
    if attr_name == "xmlns" {
        if uri == XML_NAMESPACE || uri == XMLNS_NAMESPACE {
            return Err(ErrorKind::InvalidNamespace {
                reason: format!("cannot bind default namespace to '{uri}'"),
            });
        }
        return Ok(Some(""));
    }
    let Some(prefix) = attr_name.strip_prefix("xmlns:") else {
        return Ok(None);
    };
    if uri.is_empty() {
        return Err(ErrorKind::InvalidNamespace {
            reason: format!(
                "namespace declaration 'xmlns:{prefix}' cannot have empty URI in XML 1.0"
            ),
        });
    }
    if prefix == "xmlns" {
        return Err(ErrorKind::InvalidNamespace {
            reason: "prefix 'xmlns' cannot be declared".to_string(),
        });
    }
    if prefix == "xml" && uri != XML_NAMESPACE {
        return Err(ErrorKind::InvalidNamespace {
            reason: format!("prefix 'xml' must bind to '{XML_NAMESPACE}', not '{uri}'"),
        });
    }
    if prefix != "xml" && (uri == XML_NAMESPACE || uri == XMLNS_NAMESPACE) {
        return Err(ErrorKind::InvalidNamespace {
            reason: format!("prefix '{prefix}' cannot bind to reserved URI '{uri}'"),
        });
    }
    Ok(Some(prefix))
}

/// Check that the element and attribute prefixes are bound (after this tag's own
/// declarations are in scope) and that no two attributes share `(namespace, local_name)`.
///
/// On error, the `bool` is `true` when the element name (not an attribute) is at fault.
pub(crate) fn check_tag_namespaces<'n, 'u>(
    name: &str,
    attr_names: impl Iterator<Item = &'n str>,
    resolve: impl Fn(&str) -> Option<&'u str>,
) -> std::result::Result<(), (ErrorKind, bool)> {
    if let Some((prefix, _)) = name.split_once(':') {
        if resolve(prefix).is_none() {
            return Err((
                ErrorKind::InvalidNamespace {
                    reason: format!("unbound namespace prefix '{prefix}' on <{name}>"),
                },
                true,
            ));
        }
    }
    let mut seen_ns_attrs: DupSet<(&'u str, &'n str)> = DupSet::new();
    for attr_name in attr_names {
        if attr_name == "xmlns" || attr_name.starts_with("xmlns:") {
            continue;
        }
        if let Some((prefix, local)) = attr_name.split_once(':') {
            let Some(uri) = resolve(prefix) else {
                return Err((
                    ErrorKind::InvalidNamespace {
                        reason: format!(
                            "unbound namespace prefix '{prefix}' on attribute '{attr_name}'"
                        ),
                    },
                    false,
                ));
            };
            if !seen_ns_attrs.insert((uri, local)) {
                return Err((
                    ErrorKind::DuplicateNamespaceAttribute {
                        namespace: uri.to_string(),
                        local_name: local.to_string(),
                    },
                    false,
                ));
            }
        }
    }
    Ok(())
}

/// Duplicate detector: linear scan for small sets, hashed beyond [`DupSet::LINEAR_MAX`].
pub(crate) struct DupSet<T> {
    small: Vec<T>,
    large: Option<HashSet<T>>,
}

impl<T: Eq + std::hash::Hash + Copy> DupSet<T> {
    const LINEAR_MAX: usize = 16;

    pub(crate) fn new() -> Self {
        Self {
            small: Vec::new(),
            large: None,
        }
    }

    /// Insert `item`, returning `false` if it was already present.
    pub(crate) fn insert(&mut self, item: T) -> bool {
        if let Some(set) = &mut self.large {
            return set.insert(item);
        }
        if self.small.contains(&item) {
            return false;
        }
        self.small.push(item);
        if self.small.len() > Self::LINEAR_MAX {
            self.large = Some(self.small.drain(..).collect());
        }
        true
    }
}

fn decode_entity_body(body: &str, pos: Position) -> Result<char> {
    match body {
        "amp" => Ok('&'),
        "lt" => Ok('<'),
        "gt" => Ok('>'),
        "quot" => Ok('"'),
        "apos" => Ok('\''),
        _ if body.starts_with("#x") => {
            let hex = &body[2..];
            if hex.is_empty() || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(Error::at(
                    ErrorKind::InvalidCharRef {
                        raw: format!("&{body};"),
                    },
                    pos,
                ));
            }
            let code = u32::from_str_radix(hex, 16).map_err(|_| {
                Error::at(
                    ErrorKind::InvalidCharRef {
                        raw: format!("&{body};"),
                    },
                    pos,
                )
            })?;
            char_from_xml_codepoint(code, body, pos)
        }
        _ if body.starts_with('#') => {
            let dec = &body[1..];
            if dec.is_empty() || !dec.bytes().all(|b| b.is_ascii_digit()) {
                return Err(Error::at(
                    ErrorKind::InvalidCharRef {
                        raw: format!("&{body};"),
                    },
                    pos,
                ));
            }
            let code = dec.parse::<u32>().map_err(|_| {
                Error::at(
                    ErrorKind::InvalidCharRef {
                        raw: format!("&{body};"),
                    },
                    pos,
                )
            })?;
            char_from_xml_codepoint(code, body, pos)
        }
        _ => Err(Error::at(
            ErrorKind::UnknownEntity {
                name: body.to_string(),
            },
            pos,
        )),
    }
}

fn char_from_xml_codepoint(code: u32, body: &str, pos: Position) -> Result<char> {
    let Some(ch) = char::from_u32(code) else {
        return Err(Error::at(
            ErrorKind::InvalidCharRef {
                raw: format!("&{body};"),
            },
            pos,
        ));
    };
    if !is_xml_char(ch) {
        return Err(Error::at(
            ErrorKind::InvalidXmlChar { codepoint: code },
            pos,
        ));
    }
    Ok(ch)
}

pub(crate) fn is_valid_encoding_name(enc: &str) -> bool {
    let mut bytes = enc.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    first.is_ascii_alphabetic()
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-')
}

#[inline]
pub(crate) fn is_xml_whitespace(ch: char) -> bool {
    matches!(ch, ' ' | '\t' | '\n' | '\r')
}

#[inline]
pub(crate) fn is_xml_char(ch: char) -> bool {
    matches!(
        ch as u32,
        0x9 | 0xA | 0xD | 0x20..=0xD7FF | 0xE000..=0xFFFD | 0x10000..=0x10FFFF
    )
}

#[inline]
pub(crate) fn is_name_start_char(ch: char) -> bool {
    matches!(
        ch as u32,
        0x3A // ':'
        | 0x41..=0x5A // 'A'..='Z'
        | 0x5F // '_'
        | 0x61..=0x7A // 'a'..='z'
        | 0xC0..=0xD6
        | 0xD8..=0xF6
        | 0xF8..=0x2FF
        | 0x370..=0x37D
        | 0x37F..=0x1FFF
        | 0x200C..=0x200D
        | 0x2070..=0x218F
        | 0x2C00..=0x2FEF
        | 0x3001..=0xD7FF
        | 0xF900..=0xFDCF
        | 0xFDF0..=0xFFFD
        | 0x10000..=0xEFFFF
    )
}

#[inline]
pub(crate) fn is_name_char(ch: char) -> bool {
    is_name_start_char(ch)
        || matches!(
            ch as u32,
            0x2D // '-'
            | 0x2E // '.'
            | 0x30..=0x39 // '0'..='9'
            | 0xB7
            | 0x0300..=0x036F
            | 0x203F..=0x2040
        )
}

pub(crate) fn is_valid_xml_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    is_name_start_char(first) && chars.all(is_name_char)
}
