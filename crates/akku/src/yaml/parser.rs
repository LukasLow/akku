//! The YAML parser: block and flow structures, scalars, anchors and documents.
//!
//! Parsing is indentation driven. Every entry into a collection (and every node
//! whose properties stand alone on a line) bumps a depth counter and is
//! rejected before recursing when the configured limit is reached, so a hostile
//! document cannot overflow the stack. Alias expansion and the copies anchors
//! keep are budgeted before any node is cloned.

use super::error::{Error, ErrorKind, Position, Result};
use super::scanner::{is_break, Scanner};
use super::schema::{self, Tag};
use super::value::{Mapping, Value};

/// Limits and relaxations for parsing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseOptions {
    /// Maximum nesting depth of collections. Default: `128`.
    pub max_depth: usize,
    /// Maximum number of alias references in the stream. Default: `64`.
    pub max_aliases: usize,
    /// Maximum number of nodes that alias expansion may clone in total.
    ///
    /// This is the billion-laughs guard: the budget is checked before anything
    /// is cloned. Default: `10_000`.
    pub max_alias_expansions: usize,
    /// Maximum number of nodes materialized, including alias expansions and
    /// the copy every `&anchor` keeps of its node. Default: `1_000_000`.
    pub max_nodes: usize,
    /// Accept a repeated mapping key instead of failing on it. The last value
    /// wins and the key keeps its first position. Default: `false`.
    pub allow_duplicate_keys: bool,
    /// Accept sequences and mappings as mapping keys. Default: `false`.
    pub allow_complex_keys: bool,
    /// Honor the `<<` merge key. Default: `true`.
    pub merge_keys: bool,
}

impl ParseOptions {
    /// The defaults described on each field.
    pub const fn new() -> Self {
        Self {
            max_depth: 128,
            max_aliases: 64,
            max_alias_expansions: 10_000,
            max_nodes: 1_000_000,
            allow_duplicate_keys: false,
            allow_complex_keys: false,
            merge_keys: true,
        }
    }

    /// No limits at all. Only for trusted input and for tests.
    pub const fn unbounded() -> Self {
        Self {
            max_depth: usize::MAX,
            max_aliases: usize::MAX,
            max_alias_expansions: usize::MAX,
            max_nodes: usize::MAX,
            allow_duplicate_keys: false,
            allow_complex_keys: false,
            merge_keys: true,
        }
    }
}

impl Default for ParseOptions {
    fn default() -> Self {
        Self::new()
    }
}

/// A lazily parsed multi-document stream.
///
/// Yields one [`Result`] per document and stops after the first error.
#[derive(Debug)]
pub struct Documents<'a> {
    parser: Parser<'a>,
    failed: bool,
}

impl<'a> Documents<'a> {
    /// Read a stream with the default [`ParseOptions`].
    pub fn new(input: &'a str) -> Self {
        Self::with_options(input, ParseOptions::new())
    }

    /// Read a stream with explicit [`ParseOptions`].
    pub fn with_options(input: &'a str, options: ParseOptions) -> Self {
        Self {
            parser: Parser::new(input, options),
            failed: false,
        }
    }
}

impl Iterator for Documents<'_> {
    type Item = Result<Value>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed {
            return None;
        }
        match self.parser.next_document() {
            Ok(Some(value)) => Some(Ok(value)),
            Ok(None) => None,
            Err(error) => {
                self.failed = true;
                Some(Err(error))
            }
        }
    }
}

/// Parse exactly one document.
pub(crate) fn parse_one(input: &str, options: ParseOptions) -> Result<Value> {
    let mut parser = Parser::new(input, options);
    let Some(first) = parser.next_document()? else {
        return Ok(Value::Null);
    };
    let position = parser.scanner.position();
    if parser.next_document()?.is_some() {
        return Err(Error::new(ErrorKind::MultipleDocuments, position));
    }
    Ok(first)
}

/// Parse every document of a stream.
pub(crate) fn parse_many(input: &str, options: ParseOptions) -> Result<Vec<Value>> {
    let mut parser = Parser::new(input, options);
    let mut documents = Vec::new();
    while let Some(value) = parser.next_document()? {
        documents.push(value);
    }
    Ok(documents)
}

/// Whether a plain scalar is terminated at this point in flow context.
fn flow_terminator(next: Option<char>) -> bool {
    match next {
        None => true,
        Some(ch) => ch == ' ' || ch == '\t' || ch == ',' || ch == ']' || ch == '}' || is_break(ch),
    }
}

/// Where a plain scalar may end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlainMode {
    /// A block mapping key: ends at `:` followed by a space or a line break.
    BlockKey,
    /// A block value: ends at the end of the line or a comment.
    BlockValue,
    /// Inside a flow collection: ends at a flow indicator.
    Flow,
}

/// Where a node was requested from, which decides what is allowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Frame {
    /// A block context, where block scalars are fine.
    Block,
    /// A flow context, where block scalars are not.
    Flow,
}

/// A parsed node before tags and anchors are applied.
enum Item {
    /// A finished value.
    Value(Value),
    /// A plain scalar, still unresolved.
    Plain(String),
    /// A quoted or block scalar; always a string unless a tag says otherwise.
    Quoted(String),
}

/// The `&anchor` and `!!tag` in front of a node.
#[derive(Debug, Default)]
struct Props {
    anchor: Option<String>,
    tag: Option<Tag>,
}

/// How trailing line breaks are handled in a block scalar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Chomp {
    Clip,
    Strip,
    Keep,
}

/// The parser state.
#[derive(Debug)]
struct Parser<'a> {
    scanner: Scanner<'a>,
    options: ParseOptions,
    depth: usize,
    nodes: usize,
    aliases: usize,
    expansions: usize,
    /// Every anchor with its node and that node's size.
    anchors: Vec<(String, Value, usize)>,
    path: Vec<String>,
    documents: usize,
}

impl<'a> Parser<'a> {
    fn new(input: &'a str, options: ParseOptions) -> Self {
        Self {
            scanner: Scanner::new(input),
            options,
            depth: 0,
            nodes: 0,
            aliases: 0,
            expansions: 0,
            anchors: Vec::new(),
            path: Vec::new(),
            documents: 0,
        }
    }

    /// An error at the cursor, carrying the current path.
    fn err(&self, kind: ErrorKind) -> Error {
        Error::new(kind, self.scanner.position()).with_path(self.path_text())
    }

    /// An error at a recorded position, carrying the current path.
    fn err_at(&self, position: Position, kind: ErrorKind) -> Error {
        Error::new(kind, position).with_path(self.path_text())
    }

    /// The path to the current value.
    fn path_text(&self) -> String {
        let mut out = String::new();
        for segment in &self.path {
            if segment.starts_with('[') {
                out.push_str(segment);
            } else {
                if !out.is_empty() {
                    out.push('.');
                }
                out.push_str(segment);
            }
        }
        out
    }

    /// Whether the cursor is at the end of its line.
    fn at_line_end(&self) -> bool {
        match self.scanner.peek() {
            None => true,
            Some(ch) => is_break(ch),
        }
    }

    /// Count one node against the budget.
    fn count_node(&mut self) -> Result<()> {
        self.nodes += 1;
        if self.nodes > self.options.max_nodes {
            return Err(self.err(ErrorKind::MaxNodesExceeded {
                limit: self.options.max_nodes,
            }));
        }
        Ok(())
    }

    /// Skip blank lines, indentation and comments, stopping at content.
    fn skip_blank(&mut self) {
        loop {
            self.scanner.skip_spaces_only();
            if self.scanner.peek() == Some('#') {
                self.scanner.skip_comment();
            }
            if matches!(self.scanner.peek(), Some('\n') | Some('\r')) {
                self.scanner.consume_break();
            } else {
                return;
            }
        }
    }

    /// Whether the cursor sits on an explicit document marker.
    fn at_doc_marker(&self, marker: &str) -> bool {
        if !self.scanner.at_line_start() || !self.scanner.starts_with(marker) {
            return false;
        }
        let mut probe = self.scanner;
        for _ in 0..marker.chars().count() {
            probe.bump();
        }
        match probe.peek() {
            None => true,
            Some(ch) => ch == ' ' || ch == '\t' || is_break(ch),
        }
    }

    /// Consume a three character document marker.
    fn bump_marker(&mut self) {
        for _ in 0..3 {
            self.scanner.bump();
        }
    }

    /// The current indentation, in spaces.
    fn indent(&self) -> usize {
        (self.scanner.column() as usize).saturating_sub(1)
    }

    /// Whether a `- ` sequence entry starts here.
    fn is_seq_entry(&self) -> bool {
        if self.scanner.peek() != Some('-') {
            return false;
        }
        let mut probe = self.scanner;
        probe.bump();
        match probe.peek() {
            None => true,
            Some(ch) => ch == ' ' || ch == '\t' || is_break(ch),
        }
    }

    /// Whether a `? ` explicit key starts here.
    fn is_explicit_key(&self) -> bool {
        if self.scanner.peek() != Some('?') {
            return false;
        }
        let mut probe = self.scanner;
        probe.bump();
        match probe.peek() {
            None => true,
            Some(ch) => ch == ' ' || ch == '\t' || is_break(ch),
        }
    }

    /// Read the next document, or `None` at the end of the stream.
    fn next_document(&mut self) -> Result<Option<Value>> {
        self.skip_blank();
        while self.scanner.at_line_start() && self.scanner.peek() == Some('%') {
            self.parse_directive()?;
            self.skip_blank();
        }
        if self.scanner.at_eof() {
            return Ok(None);
        }
        if self.at_doc_marker("---") {
            self.bump_marker();
            self.scanner.skip_spaces();
            if self.scanner.peek() == Some('#') {
                self.scanner.skip_comment();
            }
            if !self.at_line_end() {
                self.documents += 1;
                let indent = self.indent();
                let value = self.parse_block_node(indent)?;
                self.after_value()?;
                return Ok(Some(value));
            }
            self.scanner.consume_break();
            self.skip_blank();
        } else if self.documents > 0 {
            let kind = if self.scanner.column() > 1 {
                ErrorKind::BadIndentation {
                    reason: "no matching block node",
                }
            } else {
                ErrorKind::TrailingContent
            };
            return Err(self.err(kind));
        }

        if self.scanner.at_eof() {
            self.documents += 1;
            return Ok(Some(Value::Null));
        }
        if self.at_doc_marker("...") {
            self.bump_marker();
            self.scanner.skip_spaces();
            if self.scanner.peek() == Some('#') {
                self.scanner.skip_comment();
            }
            if !self.at_line_end() {
                return Err(self.err(ErrorKind::TrailingContent));
            }
            self.scanner.consume_break();
            self.documents += 1;
            return Ok(Some(Value::Null));
        }
        if self.at_doc_marker("---") {
            self.documents += 1;
            return Ok(Some(Value::Null));
        }

        self.documents += 1;
        let indent = self.indent();
        let value = self.parse_block_node(indent)?;
        self.after_value()?;
        Ok(Some(value))
    }

    /// Consume a document end marker if it follows the value.
    fn after_value(&mut self) -> Result<()> {
        self.skip_blank();
        if self.at_doc_marker("...") {
            self.bump_marker();
            self.scanner.skip_spaces();
            if self.scanner.peek() == Some('#') {
                self.scanner.skip_comment();
            }
            if !self.at_line_end() {
                return Err(self.err(ErrorKind::TrailingContent));
            }
            self.scanner.consume_break();
            self.skip_blank();
        }
        Ok(())
    }

    /// Parse a `%` directive.
    fn parse_directive(&mut self) -> Result<()> {
        self.scanner.bump();
        let mut name = String::new();
        while let Some(ch) = self.scanner.peek() {
            if ch == ' ' || ch == '\t' || is_break(ch) {
                break;
            }
            name.push(ch);
            self.scanner.bump();
        }
        if name != "YAML" {
            return Err(self.err(ErrorKind::UnsupportedDirective { name }));
        }
        self.scanner.skip_spaces();
        let mut version = String::new();
        while let Some(ch) = self.scanner.peek() {
            if ch == ' ' || ch == '\t' || ch == '#' || is_break(ch) {
                break;
            }
            version.push(ch);
            self.scanner.bump();
        }
        let (major, minor) = version.split_once('.').unwrap_or((version.as_str(), "0"));
        let major: u8 = major.parse().unwrap_or(0);
        let minor: u8 = minor.parse().unwrap_or(0);
        if major != 1 || minor > 2 {
            return Err(self.err(ErrorKind::UnsupportedVersion { major, minor }));
        }
        self.scanner.skip_spaces();
        if self.scanner.peek() == Some('#') {
            self.scanner.skip_comment();
        }
        if !self.at_line_end() {
            return Err(self.err(ErrorKind::TrailingContent));
        }
        self.scanner.consume_break();
        Ok(())
    }

    /// Run `step` one nesting level deeper, failing before it recurses when
    /// the depth budget is spent.
    ///
    /// Every recursive path of the grammar passes through here: each
    /// collection, plus a node whose properties stand alone on their line.
    fn descend<T>(&mut self, step: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        if self.depth >= self.options.max_depth {
            return Err(self.err(ErrorKind::MaxDepthExceeded {
                limit: self.options.max_depth,
            }));
        }
        self.depth += 1;
        let result = step(self);
        self.depth -= 1;
        result
    }

    /// Parse a node that starts a block line.
    fn parse_block_node(&mut self, indent: usize) -> Result<Value> {
        let props = self.parse_properties(false)?;
        if self.at_line_end() || self.scanner.peek() == Some('#') {
            let nested = self.descend(|parser| parser.parse_nested_value(indent, true))?;
            return self.finalize(Item::Value(nested), props);
        }
        let base = indent.saturating_sub(1);
        let item = self.parse_node_item(indent, base, Frame::Block)?;
        self.finalize(item, props)
    }

    /// Parse the node that follows the cursor, block or flow.
    fn parse_node_item(&mut self, indent: usize, plain_base: usize, frame: Frame) -> Result<Item> {
        self.count_node()?;
        let Some(ch) = self.scanner.peek() else {
            return Err(self.err(ErrorKind::UnexpectedEof));
        };
        match ch {
            '-' if self.is_seq_entry() => Ok(Item::Value(self.parse_block_seq(indent)?)),
            '?' if self.is_explicit_key() => Ok(Item::Value(self.parse_explicit_map(indent)?)),
            '\'' | '"' | '*' if frame == Frame::Block && self.looks_like_block_key() => {
                Ok(Item::Value(self.parse_block_map(indent)?))
            }
            '[' | '{' => Ok(Item::Value(self.parse_flow_node()?)),
            '*' => Ok(Item::Value(self.parse_alias()?)),
            '|' | '>' if frame == Frame::Flow => Err(self.err(ErrorKind::BlockScalarInFlow)),
            '|' | '>' => Ok(Item::Quoted(self.parse_block_scalar(indent)?)),
            '\'' | '"' => Ok(Item::Quoted(self.parse_quoted()?)),
            _ => {
                if self.looks_like_block_key() {
                    Ok(Item::Value(self.parse_block_map(indent)?))
                } else {
                    Ok(Item::Plain(self.parse_plain_multiline(plain_base)))
                }
            }
        }
    }

    /// Look ahead on the current line for a `:` that ends a block mapping key.
    fn looks_like_block_key(&self) -> bool {
        let mut probe = self.scanner;
        let mut depth = 0i32;
        // Quotes and brackets are only structure at the start of the key or
        // between the tokens of a flow key; inside a plain key such as
        // `it's: 1` they are ordinary characters.
        let mut token_start = true;
        loop {
            let Some(ch) = probe.peek() else {
                return false;
            };
            if is_break(ch) {
                return false;
            }
            let structural = token_start;
            token_start = depth > 0 && matches!(ch, ' ' | '\t' | '[' | '{' | ',' | ':');
            if !structural && matches!(ch, '\'' | '"' | '[' | '{') {
                probe.bump();
                continue;
            }
            if depth == 0 && !structural && matches!(ch, ']' | '}') {
                probe.bump();
                continue;
            }
            if ch == '#' {
                let previous = probe.peek_prev();
                if matches!(previous, Some(' ') | Some('\t')) {
                    return false;
                }
            }
            match ch {
                '\'' => {
                    probe.bump();
                    loop {
                        let Some(inner) = probe.peek() else {
                            return false;
                        };
                        probe.bump();
                        if is_break(inner) {
                            return false;
                        }
                        if inner == '\'' {
                            if probe.peek() == Some('\'') {
                                probe.bump();
                            } else {
                                break;
                            }
                        }
                    }
                }
                '"' => {
                    probe.bump();
                    loop {
                        let Some(inner) = probe.peek() else {
                            return false;
                        };
                        probe.bump();
                        if is_break(inner) {
                            return false;
                        }
                        match inner {
                            '\\' => {
                                probe.bump();
                            }
                            '"' => break,
                            _ => {}
                        }
                    }
                }
                '[' | '{' => {
                    depth += 1;
                    token_start = true;
                    probe.bump();
                }
                ']' | '}' => {
                    depth -= 1;
                    probe.bump();
                }
                ':' if depth == 0 => {
                    probe.bump();
                    match probe.peek() {
                        None => return true,
                        Some(next) if next == ' ' || next == '\t' || is_break(next) => return true,
                        _ => {}
                    }
                }
                _ => {
                    probe.bump();
                }
            }
        }
    }

    /// Parse a block mapping at `indent`, with its first key at the cursor.
    fn parse_block_map(&mut self, indent: usize) -> Result<Value> {
        self.descend(|parser| parser.parse_block_map_inner(indent))
    }

    fn parse_block_map_inner(&mut self, indent: usize) -> Result<Value> {
        let mut mapping = Mapping::new();
        let mut merges: Vec<Mapping> = Vec::new();
        loop {
            let key_position = self.scanner.position();
            let (key, merge) = self.parse_key()?;
            self.scanner.skip_spaces();
            if self.scanner.peek() != Some(':') {
                return Err(self.err_at(key_position, ErrorKind::MissingValueSeparator));
            }
            self.scanner.bump();
            self.path.push(key_segment(&key));
            let value = self.parse_block_map_value(indent);
            self.path.pop();
            let value = value?;
            if self.options.merge_keys && merge {
                match merge_source(&value) {
                    Some(source) => merges.extend(source),
                    None => {
                        return Err(self.err(ErrorKind::MergeNotMapping {
                            found: value.type_name(),
                        }))
                    }
                }
            } else {
                if !self.options.allow_duplicate_keys && mapping.contains_key(&key) {
                    return Err(self.err_at(
                        key_position,
                        ErrorKind::DuplicateKey {
                            key: key_segment(&key),
                        },
                    ));
                }
                mapping.insert(key, value);
            }
            self.skip_blank();
            if self.scanner.at_eof() {
                break;
            }
            if self.at_doc_marker("---") || self.at_doc_marker("...") {
                break;
            }
            if self.scanner.peek() == Some('\t') {
                return Err(self.err(ErrorKind::TabIndentation));
            }
            if self.indent() != indent || self.is_seq_entry() || self.is_explicit_key() {
                break;
            }
        }
        for merge in merges {
            for (key, value) in merge {
                if !mapping.contains_key(&key) {
                    mapping.insert(key, value);
                }
            }
        }
        Ok(Value::Mapping(mapping))
    }

    /// Parse a mapping key, including its properties.
    ///
    /// The flag is set for the merge key: a plain, untagged `<<`. A quoted
    /// `'<<'` is an ordinary string key.
    fn parse_key(&mut self) -> Result<(Value, bool)> {
        let props = self.parse_properties(false)?;
        match self.scanner.peek() {
            Some('[') | Some('{') => {
                if !self.options.allow_complex_keys {
                    return Err(self.err(ErrorKind::UnsupportedKeyKind { kind: "collection" }));
                }
                let node = self.parse_flow_node()?;
                Ok((self.finalize(Item::Value(node), props)?, false))
            }
            Some('*') => {
                let node = self.parse_alias()?;
                Ok((self.finalize(Item::Value(node), props)?, false))
            }
            Some('\'') | Some('"') => {
                let text = self.parse_quoted()?;
                Ok((self.finalize(Item::Quoted(text), props)?, false))
            }
            None => Err(self.err(ErrorKind::UnexpectedEof)),
            Some(_) => {
                let mut text = String::new();
                self.read_plain(&mut text, PlainMode::BlockKey);
                let merge = text == "<<" && props.tag.is_none();
                Ok((self.finalize(Item::Plain(text), props)?, merge))
            }
        }
    }

    /// Parse the value after a block mapping's `:`.
    fn parse_block_map_value(&mut self, indent: usize) -> Result<Value> {
        self.scanner.skip_spaces();
        if self.scanner.peek() == Some('#') {
            self.scanner.skip_comment();
        }
        if self.at_line_end() {
            return self.parse_nested_value(indent, true);
        }
        let props = self.parse_properties(false)?;
        if self.at_line_end() || self.scanner.peek() == Some('#') {
            let nested = self.parse_nested_value(indent, true)?;
            return self.finalize(Item::Value(nested), props);
        }
        let item = self.parse_node_item(indent, indent, Frame::Block)?;
        self.finalize(item, props)
    }

    /// Parse the node that follows on the next, more indented line.
    ///
    /// With `same_indent_seq`, a `- ` sequence at the parent's own indentation
    /// is the value too, as in `key:\n- item`. A sequence entry passes `false`,
    /// so `-\n-` stays two sibling entries.
    fn parse_nested_value(&mut self, parent_indent: usize, same_indent_seq: bool) -> Result<Value> {
        self.scanner.consume_break();
        self.skip_blank();
        if self.scanner.at_eof() {
            return Ok(Value::Null);
        }
        if self.at_doc_marker("---") || self.at_doc_marker("...") {
            return Ok(Value::Null);
        }
        let indent = self.indent();
        if indent > parent_indent {
            return self.parse_block_node(indent);
        }
        if same_indent_seq && indent == parent_indent && self.is_seq_entry() {
            return self.parse_block_node(indent);
        }
        Ok(Value::Null)
    }

    /// Parse a block sequence at `indent`, with its first `-` at the cursor.
    fn parse_block_seq(&mut self, indent: usize) -> Result<Value> {
        self.descend(|parser| parser.parse_block_seq_inner(indent))
    }

    fn parse_block_seq_inner(&mut self, indent: usize) -> Result<Value> {
        let mut items = Vec::new();
        loop {
            self.scanner.bump();
            self.count_node()?;
            if !matches!(
                self.scanner.peek(),
                None | Some(' ') | Some('\t') | Some('\n') | Some('\r')
            ) {
                return Err(self.err(ErrorKind::Expected {
                    expected: "a space after '-'",
                    found: "a non-space character",
                }));
            }
            self.scanner.skip_spaces();
            if self.scanner.peek() == Some('#') {
                self.scanner.skip_comment();
            }
            let item = if self.at_line_end() {
                self.parse_nested_value(indent, false)?
            } else {
                let content_indent = self.indent();
                self.path.push(format!("[{}]", items.len()));
                let parsed = self.parse_seq_item(content_indent, indent);
                self.path.pop();
                parsed?
            };
            items.push(item);
            self.skip_blank();
            if self.scanner.at_eof() {
                break;
            }
            if self.at_doc_marker("---") || self.at_doc_marker("...") {
                break;
            }
            if self.scanner.peek() == Some('\t') {
                return Err(self.err(ErrorKind::TabIndentation));
            }
            if self.indent() != indent || !self.is_seq_entry() {
                break;
            }
        }
        Ok(Value::Sequence(items))
    }

    /// Parse one sequence entry that starts on the dash line.
    fn parse_seq_item(&mut self, content_indent: usize, seq_indent: usize) -> Result<Value> {
        let props = self.parse_properties(false)?;
        if self.at_line_end() || self.scanner.peek() == Some('#') {
            let nested = self.parse_nested_value(seq_indent, false)?;
            return self.finalize(Item::Value(nested), props);
        }
        let item = if self.looks_like_block_key() {
            Item::Value(self.parse_block_map(content_indent)?)
        } else {
            self.parse_node_item(content_indent, seq_indent, Frame::Block)?
        };
        self.finalize(item, props)
    }

    /// Parse a mapping built from `? key` / `: value` pairs.
    fn parse_explicit_map(&mut self, indent: usize) -> Result<Value> {
        if !self.options.allow_complex_keys {
            return Err(self.err(ErrorKind::UnsupportedKeyKind { kind: "complex" }));
        }
        self.descend(|parser| parser.parse_explicit_map_inner(indent))
    }

    fn parse_explicit_map_inner(&mut self, indent: usize) -> Result<Value> {
        let mut mapping = Mapping::new();
        loop {
            let key_position = self.scanner.position();
            self.scanner.bump();
            self.scanner.skip_spaces();
            let key = if self.at_line_end() {
                self.parse_nested_value(indent, false)?
            } else {
                let item = self.parse_node_item(indent, indent, Frame::Block)?;
                self.finalize(item, Props::default())?
            };
            self.skip_blank();
            self.scanner.skip_spaces();
            if self.scanner.peek() != Some(':') {
                return Err(self.err(ErrorKind::MissingValueSeparator));
            }
            self.scanner.bump();
            let value = self.parse_block_map_value(indent)?;
            if !self.options.allow_duplicate_keys && mapping.contains_key(&key) {
                return Err(self.err_at(
                    key_position,
                    ErrorKind::DuplicateKey {
                        key: key_segment(&key),
                    },
                ));
            }
            mapping.insert(key, value);
            self.skip_blank();
            if self.scanner.at_eof() || self.indent() != indent || !self.is_explicit_key() {
                break;
            }
        }
        Ok(Value::Mapping(mapping))
    }

    /// Parse a plain scalar, folding continuation lines.
    fn parse_plain_multiline(&mut self, base: usize) -> String {
        let mut text = String::new();
        self.read_plain(&mut text, PlainMode::BlockValue);
        loop {
            let mark = self.scanner;
            let mut empties = 0;
            if self.scanner.at_eof() {
                break;
            }
            self.scanner.consume_break();
            loop {
                self.scanner.skip_spaces_only();
                if self.at_line_end() && !self.scanner.at_eof() {
                    empties += 1;
                    self.scanner.consume_break();
                    continue;
                }
                break;
            }
            let Some(ch) = self.scanner.peek() else {
                self.scanner = mark;
                break;
            };
            let indent = self.indent();
            if indent <= base || !plain_continuation(ch) || self.looks_like_block_key() {
                self.scanner = mark;
                break;
            }
            if empties == 0 {
                text.push(' ');
            } else {
                for _ in 0..empties {
                    text.push('\n');
                }
            }
            self.read_plain(&mut text, PlainMode::BlockValue);
        }
        text
    }

    /// Read one plain scalar segment into `out`.
    fn read_plain(&mut self, out: &mut String, mode: PlainMode) {
        let mut previous_space = true;
        while let Some(ch) = self.scanner.peek() {
            if is_break(ch) {
                break;
            }
            if ch == '#' && previous_space {
                break;
            }
            match ch {
                ':' if mode == PlainMode::BlockKey => {
                    let mut probe = self.scanner;
                    probe.bump();
                    let stop = match probe.peek() {
                        None => true,
                        Some(next) => next == ' ' || next == '\t' || is_break(next),
                    };
                    if stop {
                        break;
                    }
                    out.push(':');
                    self.scanner.bump();
                    previous_space = false;
                }
                ':' if mode == PlainMode::Flow => {
                    let mut probe = self.scanner;
                    probe.bump();
                    if flow_terminator(probe.peek()) {
                        break;
                    }
                    out.push(':');
                    self.scanner.bump();
                    previous_space = false;
                }
                ',' | ']' | '}' if mode == PlainMode::Flow => break,
                _ => {
                    out.push(ch);
                    self.scanner.bump();
                    previous_space = ch == ' ' || ch == '\t';
                }
            }
        }
        while out.ends_with(' ') || out.ends_with('\t') {
            out.pop();
        }
    }

    /// Parse a flow collection or scalar.
    fn parse_flow_node(&mut self) -> Result<Value> {
        self.parse_flow_node_flagged().map(|(node, _)| node)
    }

    /// Parse a flow node; the flag is set for a plain, untagged `<<`, which
    /// is the merge key when it stands in key position.
    fn parse_flow_node_flagged(&mut self) -> Result<(Value, bool)> {
        self.skip_flow_ws();
        let props = self.parse_properties(true)?;
        self.skip_flow_ws();
        let Some(ch) = self.scanner.peek() else {
            return Err(self.err(ErrorKind::UnexpectedEof));
        };
        let item = match ch {
            '[' => Item::Value(self.descend(Self::parse_flow_seq)?),
            '{' => Item::Value(self.descend(Self::parse_flow_map)?),
            '*' => Item::Value(self.parse_alias()?),
            '|' | '>' => return Err(self.err(ErrorKind::BlockScalarInFlow)),
            '\'' | '"' => Item::Quoted(self.parse_quoted()?),
            ']' | '}' | ',' => Item::Value(Value::Null),
            _ => {
                let mut text = String::new();
                self.read_plain(&mut text, PlainMode::Flow);
                Item::Plain(text)
            }
        };
        self.count_node()?;
        let merge = props.tag.is_none() && matches!(&item, Item::Plain(text) if text == "<<");
        Ok((self.finalize(item, props)?, merge))
    }

    /// Skip whitespace, line breaks and comments inside a flow collection.
    fn skip_flow_ws(&mut self) {
        loop {
            match self.scanner.peek() {
                Some(' ') | Some('\t') | Some('\n') | Some('\r') => {
                    self.scanner.bump();
                }
                Some('#') => self.scanner.skip_comment(),
                _ => return,
            }
        }
    }

    /// Parse a flow sequence, with `[` at the cursor.
    fn parse_flow_seq(&mut self) -> Result<Value> {
        self.scanner.bump();
        let mut items = Vec::new();
        loop {
            self.skip_flow_ws();
            match self.scanner.peek() {
                Some(']') => {
                    self.scanner.bump();
                    break;
                }
                None => return Err(self.err(ErrorKind::UnexpectedEof)),
                _ => {}
            }
            if self.is_explicit_key() {
                self.scanner.bump();
                self.skip_flow_ws();
            }
            let first = self.parse_flow_node()?;
            self.skip_flow_ws();
            if self.scanner.peek() == Some(':') {
                self.scanner.bump();
                self.skip_flow_ws();
                let value = match self.scanner.peek() {
                    Some(',') | Some(']') | None => Value::Null,
                    _ => self.parse_flow_node()?,
                };
                let mut pair = Mapping::new();
                pair.insert(first, value);
                items.push(Value::Mapping(pair));
            } else {
                items.push(first);
            }
            self.skip_flow_ws();
            match self.scanner.peek() {
                Some(',') => {
                    self.scanner.bump();
                }
                Some(']') => {
                    self.scanner.bump();
                    break;
                }
                None => return Err(self.err(ErrorKind::UnexpectedEof)),
                Some(_) => {
                    return Err(self.err(ErrorKind::Expected {
                        expected: "',' or ']'",
                        found: "another token",
                    }))
                }
            }
        }
        Ok(Value::Sequence(items))
    }

    /// Parse a flow mapping, with `{` at the cursor.
    fn parse_flow_map(&mut self) -> Result<Value> {
        self.scanner.bump();
        let mut mapping = Mapping::new();
        let mut merges: Vec<Mapping> = Vec::new();
        loop {
            self.skip_flow_ws();
            match self.scanner.peek() {
                Some('}') => {
                    self.scanner.bump();
                    break;
                }
                None => return Err(self.err(ErrorKind::UnexpectedEof)),
                _ => {}
            }
            if self.is_explicit_key() {
                self.scanner.bump();
                self.skip_flow_ws();
            }
            let key_position = self.scanner.position();
            let (key, merge) = self.parse_flow_node_flagged()?;
            self.skip_flow_ws();
            let value = if self.scanner.peek() == Some(':') {
                self.scanner.bump();
                self.skip_flow_ws();
                match self.scanner.peek() {
                    Some(',') | Some('}') | None => Value::Null,
                    _ => self.parse_flow_node()?,
                }
            } else {
                Value::Null
            };
            if self.options.merge_keys && merge {
                match merge_source(&value) {
                    Some(source) => merges.extend(source),
                    None => {
                        return Err(self.err(ErrorKind::MergeNotMapping {
                            found: value.type_name(),
                        }))
                    }
                }
            } else {
                if !self.options.allow_duplicate_keys && mapping.contains_key(&key) {
                    return Err(self.err_at(
                        key_position,
                        ErrorKind::DuplicateKey {
                            key: key_segment(&key),
                        },
                    ));
                }
                mapping.insert(key, value);
            }
            self.skip_flow_ws();
            match self.scanner.peek() {
                Some(',') => {
                    self.scanner.bump();
                }
                Some('}') => {
                    self.scanner.bump();
                    break;
                }
                None => return Err(self.err(ErrorKind::UnexpectedEof)),
                Some(_) => {
                    return Err(self.err(ErrorKind::Expected {
                        expected: "',' or '}'",
                        found: "another token",
                    }))
                }
            }
        }
        for merge in merges {
            for (key, value) in merge {
                if !mapping.contains_key(&key) {
                    mapping.insert(key, value);
                }
            }
        }
        Ok(Value::Mapping(mapping))
    }

    /// Parse `&name` and `!!tag` properties in front of a node.
    fn parse_properties(&mut self, flow: bool) -> Result<Props> {
        let mut props = Props::default();
        loop {
            if flow {
                self.skip_flow_ws();
            } else {
                self.scanner.skip_spaces();
            }
            match self.scanner.peek() {
                Some('&') if props.anchor.is_none() => {
                    self.scanner.bump();
                    props.anchor = Some(self.read_name()?);
                }
                Some('!') if props.tag.is_none() => {
                    props.tag = Some(self.read_tag()?);
                }
                _ => return Ok(props),
            }
        }
    }

    /// Read an anchor or alias name.
    fn read_name(&mut self) -> Result<String> {
        let mut name = String::new();
        while let Some(ch) = self.scanner.peek() {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                name.push(ch);
                self.scanner.bump();
            } else {
                break;
            }
        }
        if name.is_empty() {
            return Err(self.err(ErrorKind::Expected {
                expected: "an anchor name",
                found: "nothing",
            }));
        }
        Ok(name)
    }

    /// Read a core schema tag.
    fn read_tag(&mut self) -> Result<Tag> {
        self.scanner.bump();
        if self.scanner.peek() == Some('!') {
            self.scanner.bump();
            let mut name = String::new();
            while let Some(ch) = self.scanner.peek() {
                if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                    name.push(ch);
                    self.scanner.bump();
                } else {
                    break;
                }
            }
            return Tag::from_name(&name).ok_or_else(|| {
                self.err(ErrorKind::InvalidTag {
                    tag: format!("!!{name}"),
                })
            });
        }
        if self.scanner.peek() == Some('<') {
            self.scanner.bump();
            let mut uri = String::new();
            loop {
                let Some(ch) = self.scanner.peek() else {
                    return Err(self.err(ErrorKind::UnexpectedEof));
                };
                self.scanner.bump();
                if ch == '>' {
                    break;
                }
                uri.push(ch);
            }
            let name = uri.strip_prefix("tag:yaml.org,2002:").ok_or_else(|| {
                self.err(ErrorKind::InvalidTag {
                    tag: format!("!<{uri}>"),
                })
            })?;
            return Tag::from_name(name).ok_or_else(|| {
                self.err(ErrorKind::InvalidTag {
                    tag: format!("!<{uri}>"),
                })
            });
        }
        let mut name = String::new();
        while let Some(ch) = self.scanner.peek() {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                name.push(ch);
                self.scanner.bump();
            } else {
                break;
            }
        }
        Err(self.err(ErrorKind::InvalidTag {
            tag: format!("!{name}"),
        }))
    }

    /// Resolve an alias, checking the expansion budget before cloning.
    fn parse_alias(&mut self) -> Result<Value> {
        self.scanner.bump();
        let name = self.read_name()?;
        self.aliases += 1;
        if self.aliases > self.options.max_aliases {
            return Err(self.err(ErrorKind::AliasLimitExceeded {
                limit: self.options.max_aliases,
            }));
        }
        let Some(index) = self
            .anchors
            .iter()
            .rposition(|(candidate, _, _)| candidate == &name)
        else {
            return Err(self.err(ErrorKind::AnchorNotFound { name }));
        };
        let count = self.anchors[index].2;
        if self.expansions.saturating_add(count) > self.options.max_alias_expansions {
            return Err(self.err(ErrorKind::AliasExpansionLimitExceeded {
                limit: self.options.max_alias_expansions,
            }));
        }
        if self.nodes.saturating_add(count) > self.options.max_nodes {
            return Err(self.err(ErrorKind::MaxNodesExceeded {
                limit: self.options.max_nodes,
            }));
        }
        self.expansions += count;
        self.nodes += count;
        Ok(self.anchors[index].1.clone())
    }

    /// Parse a quoted scalar, including its escapes and line folding.
    fn parse_quoted(&mut self) -> Result<String> {
        let quote = self.scanner.peek().unwrap_or('"');
        self.scanner.bump();
        let mut out = String::new();
        loop {
            let Some(ch) = self.scanner.peek() else {
                return Err(self.err(ErrorKind::UnexpectedEof));
            };
            if ch == quote {
                self.scanner.bump();
                if quote == '\'' && self.scanner.peek() == Some('\'') {
                    self.scanner.bump();
                    out.push('\'');
                    continue;
                }
                break;
            }
            if quote == '"' && ch == '\\' {
                self.scanner.bump();
                self.decode_escape(&mut out)?;
                continue;
            }
            if is_break(ch) {
                while out.ends_with(' ') || out.ends_with('\t') {
                    out.pop();
                }
                self.scanner.consume_break();
                let mut empties = 0;
                loop {
                    self.scanner.skip_spaces_only();
                    if self.at_line_end() && !self.scanner.at_eof() {
                        empties += 1;
                        self.scanner.consume_break();
                        continue;
                    }
                    break;
                }
                if self.scanner.at_eof() {
                    return Err(self.err(ErrorKind::UnexpectedEof));
                }
                if empties == 0 {
                    out.push(' ');
                } else {
                    for _ in 0..empties {
                        out.push('\n');
                    }
                }
                continue;
            }
            out.push(ch);
            self.scanner.bump();
        }
        Ok(out)
    }

    /// Decode one escape sequence after the backslash.
    fn decode_escape(&mut self, out: &mut String) -> Result<()> {
        let Some(ch) = self.scanner.peek() else {
            return Err(self.err(ErrorKind::UnexpectedEof));
        };
        let replacement = match ch {
            '0' => '\0',
            'a' => '\u{7}',
            'b' => '\u{8}',
            't' | '\t' => '\t',
            'n' => '\n',
            'v' => '\u{b}',
            'f' => '\u{c}',
            'r' => '\r',
            'e' => '\u{1b}',
            ' ' => ' ',
            '"' => '"',
            '/' => '/',
            '\\' => '\\',
            'N' => '\u{85}',
            '_' => '\u{a0}',
            'L' => '\u{2028}',
            'P' => '\u{2029}',
            'x' => return self.decode_hex(out, 2),
            'u' => return self.decode_hex(out, 4),
            'U' => return self.decode_hex(out, 8),
            '\n' | '\r' => {
                self.scanner.consume_break();
                return Ok(());
            }
            other => return Err(self.err(ErrorKind::InvalidEscape { escape: other })),
        };
        self.scanner.bump();
        out.push(replacement);
        Ok(())
    }

    /// Decode a `\x`, `\u` or `\U` escape with the given number of hex digits.
    fn decode_hex(&mut self, out: &mut String, digits: usize) -> Result<()> {
        self.scanner.bump();
        let mut value: u32 = 0;
        for _ in 0..digits {
            let Some(ch) = self.scanner.peek() else {
                return Err(self.err(ErrorKind::UnexpectedEof));
            };
            let Some(digit) = ch.to_digit(16) else {
                return Err(self.err(ErrorKind::InvalidUnicodeEscape));
            };
            value = value * 16 + digit;
            self.scanner.bump();
        }
        let decoded =
            char::from_u32(value).ok_or_else(|| self.err(ErrorKind::InvalidUnicodeEscape))?;
        out.push(decoded);
        Ok(())
    }

    /// Parse a block scalar and apply its chomping.
    fn parse_block_scalar(&mut self, parent_indent: usize) -> Result<String> {
        let folded = self.scanner.peek() == Some('>');
        self.scanner.bump();
        let mut chomp = Chomp::Clip;
        let mut explicit: Option<usize> = None;
        for _ in 0..2 {
            match self.scanner.peek() {
                Some('-') => {
                    chomp = Chomp::Strip;
                    self.scanner.bump();
                }
                Some('+') => {
                    chomp = Chomp::Keep;
                    self.scanner.bump();
                }
                Some(ch @ '1'..='9') => {
                    explicit = ch.to_digit(10).map(|digit| digit as usize);
                    self.scanner.bump();
                }
                _ => break,
            }
        }
        self.scanner.skip_spaces();
        if self.scanner.peek() == Some('#') {
            self.scanner.skip_comment();
        }
        if !self.at_line_end() {
            return Err(self.err(ErrorKind::Expected {
                expected: "a line break after the block scalar header",
                found: "content",
            }));
        }
        self.scanner.consume_break();

        let mut block_indent = explicit.map(|digit| parent_indent + digit);
        let mut lines: Vec<(String, bool)> = Vec::new();
        loop {
            if self.scanner.at_eof() {
                break;
            }
            let mark = self.scanner;
            let mut spaces = 0;
            while self.scanner.peek() == Some(' ') {
                self.scanner.bump();
                spaces += 1;
            }
            if self.scanner.peek() == Some('\t') {
                return Err(self.err(ErrorKind::TabIndentation));
            }
            if self.at_line_end() {
                lines.push((String::new(), false));
                self.scanner.consume_break();
                continue;
            }
            let required = block_indent.unwrap_or(parent_indent + 1);
            if spaces < required {
                self.scanner = mark;
                break;
            }
            if block_indent.is_none() {
                block_indent = Some(spaces);
            }
            let indent = block_indent.expect("set above");
            self.scanner = mark;
            for _ in 0..indent {
                self.scanner.bump();
            }
            let mut text = String::new();
            while let Some(ch) = self.scanner.peek() {
                if is_break(ch) {
                    break;
                }
                text.push(ch);
                self.scanner.bump();
            }
            let more_indented = text.starts_with(' ');
            lines.push((text, more_indented));
            self.scanner.consume_break();
        }

        let mut trailing = 0;
        while matches!(lines.last(), Some((text, _)) if text.is_empty()) {
            lines.pop();
            trailing += 1;
        }
        let body = if folded {
            fold_lines(&lines)
        } else {
            literal_lines(&lines)
        };
        Ok(match chomp {
            Chomp::Strip => body,
            Chomp::Clip => {
                if lines.is_empty() {
                    String::new()
                } else {
                    let mut out = body;
                    out.push('\n');
                    out
                }
            }
            Chomp::Keep => {
                let mut out = body;
                let breaks = if lines.is_empty() {
                    trailing
                } else {
                    trailing + 1
                };
                for _ in 0..breaks {
                    out.push('\n');
                }
                out
            }
        })
    }

    /// Apply a tag and register an anchor.
    fn finalize(&mut self, item: Item, props: Props) -> Result<Value> {
        let value = match (props.tag, item) {
            (None, Item::Value(value)) => value,
            (None, Item::Plain(text)) => schema::resolve_plain(&text),
            (None, Item::Quoted(text)) => Value::String(text),
            (Some(tag), Item::Value(value)) => {
                let fits = matches!(
                    (tag, &value),
                    (Tag::Seq, Value::Sequence(_)) | (Tag::Map, Value::Mapping(_))
                );
                if !fits {
                    return Err(self.err(ErrorKind::TagMismatch {
                        tag: tag.label(),
                        found: value.type_name(),
                    }));
                }
                value
            }
            (Some(tag), Item::Plain(text)) | (Some(tag), Item::Quoted(text)) => {
                schema::coerce(tag, &text).ok_or_else(|| {
                    self.err(ErrorKind::TagMismatch {
                        tag: tag.label(),
                        found: "string",
                    })
                })?
            }
        };
        if let Some(name) = props.anchor {
            // The anchor keeps its own copy, so nested anchors (and anchors
            // over aliases) would multiply memory without this budget. The
            // size is measured before anything is cloned.
            let count = value.node_count();
            if self.nodes.saturating_add(count) > self.options.max_nodes {
                return Err(self.err(ErrorKind::MaxNodesExceeded {
                    limit: self.options.max_nodes,
                }));
            }
            self.nodes += count;
            self.anchors.push((name, value.clone(), count));
        }
        Ok(value)
    }
}

/// Whether a character can continue a plain scalar onto the next line.
fn plain_continuation(ch: char) -> bool {
    !matches!(
        ch,
        '-' | '?'
            | ':'
            | ','
            | '['
            | ']'
            | '{'
            | '}'
            | '#'
            | '&'
            | '*'
            | '!'
            | '|'
            | '>'
            | '\''
            | '"'
            | '%'
            | '@'
            | '`'
    )
}

/// The path segment for a key.
fn key_segment(key: &Value) -> String {
    match key {
        Value::String(text) => text.clone(),
        other => super::to_string(other).trim_end().to_string(),
    }
}

/// The mappings a merge key contributes, or `None` if it holds something else.
fn merge_source(value: &Value) -> Option<Vec<Mapping>> {
    match value {
        Value::Mapping(mapping) => Some(vec![mapping.clone()]),
        Value::Sequence(items) => {
            let mut sources = Vec::with_capacity(items.len());
            for item in items {
                match item {
                    Value::Mapping(mapping) => sources.push(mapping.clone()),
                    _ => return None,
                }
            }
            Some(sources)
        }
        _ => None,
    }
}

/// Join block scalar lines literally, without a trailing break.
fn literal_lines(lines: &[(String, bool)]) -> String {
    let mut out = String::new();
    for (index, (text, _)) in lines.iter().enumerate() {
        out.push_str(text);
        if index + 1 < lines.len() {
            out.push('\n');
        }
    }
    out
}

/// Join block scalar lines with folding, without a trailing break.
fn fold_lines(lines: &[(String, bool)]) -> String {
    let mut out = String::new();
    let mut index = 0;
    while index < lines.len() && lines[index].0.is_empty() {
        out.push('\n');
        index += 1;
    }
    while index < lines.len() {
        out.push_str(&lines[index].0);
        let more_indented = lines[index].1;
        let mut next = index + 1;
        let mut empties = 0;
        while next < lines.len() && lines[next].0.is_empty() {
            empties += 1;
            next += 1;
        }
        if next >= lines.len() {
            break;
        }
        if empties > 0 {
            for _ in 0..empties {
                out.push('\n');
            }
        } else if more_indented || lines[next].1 {
            out.push('\n');
        } else {
            out.push(' ');
        }
        index = next;
    }
    out
}
