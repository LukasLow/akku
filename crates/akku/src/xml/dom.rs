//! Lightweight owned XML DOM (`Document`, `Element`, `Node`, `Attribute`) with iterative `Drop`.

use std::collections::HashMap;
use std::str::FromStr;

use crate::xml::error::{Error, ErrorKind, Result};
use crate::xml::reader::{is_xml_whitespace, Event, ParseOptions, Reader, StartTag, XmlDecl};

const XML_NAMESPACE: &str = "http://www.w3.org/XML/1998/namespace";
const XMLNS_NAMESPACE: &str = "http://www.w3.org/2000/xmlns/";

/// An attribute on an [`Element`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attribute {
    /// Raw attribute name (possibly prefixed, e.g. `"xlink:href"`).
    pub name: String,
    /// Decoded attribute value.
    pub value: String,
    /// Resolved namespace URI, if this attribute has a namespace prefix.
    pub namespace: Option<String>,
}

impl Attribute {
    /// Create an attribute without a resolved namespace.
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
            namespace: None,
        }
    }

    /// Create an attribute with an explicit namespace URI.
    pub fn with_namespace(
        name: impl Into<String>,
        value: impl Into<String>,
        namespace: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
            namespace: Some(namespace.into()),
        }
    }

    /// Namespace prefix before `:`, if any.
    pub fn prefix(&self) -> Option<&str> {
        self.name.split_once(':').map(|(p, _)| p)
    }

    /// Local name after `:`, or the full name if unprefixed.
    pub fn local_name(&self) -> &str {
        self.name
            .split_once(':')
            .map_or(self.name.as_str(), |(_, l)| l)
    }

    /// Resolved namespace URI, if any.
    pub fn namespace_uri(&self) -> Option<&str> {
        self.namespace.as_deref()
    }
}

/// A child node inside an XML [`Document`] or [`Element`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    /// A child element.
    Element(Element),
    /// Character data.
    Text(String),
    /// A `<![CDATA[...]]>` section.
    CData(String),
    /// A `<!-- ... -->` comment.
    Comment(String),
    /// A `<?target ...?>` processing instruction.
    ProcessingInstruction {
        /// Target name.
        target: String,
        /// Optional data payload.
        data: Option<String>,
    },
}

impl From<Element> for Node {
    fn from(el: Element) -> Self {
        Self::Element(el)
    }
}

impl From<String> for Node {
    fn from(text: String) -> Self {
        Self::Text(text)
    }
}

impl From<&str> for Node {
    fn from(text: &str) -> Self {
        Self::Text(text.to_string())
    }
}

/// An XML element node with attributes, children, and optional resolved namespace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Element {
    name: String,
    namespace: Option<String>,
    attributes: Vec<Attribute>,
    children: Vec<Node>,
}

impl Element {
    /// Create a new empty element with the given tag name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            namespace: None,
            attributes: Vec::new(),
            children: Vec::new(),
        }
    }

    /// Builder: attach a namespace URI to this element.
    pub fn with_namespace(mut self, namespace: impl Into<String>) -> Self {
        self.namespace = Some(namespace.into());
        self
    }

    /// Builder: set or append an attribute.
    pub fn with_attr(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.set_attr(name, value);
        self
    }

    /// Builder: append a child [`Node`].
    pub fn with_child(mut self, node: impl Into<Node>) -> Self {
        self.children.push(node.into());
        self
    }

    /// Builder: append a [`Node::Text`] child.
    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        self.children.push(Node::Text(text.into()));
        self
    }

    /// Raw element name (possibly prefixed, e.g. `"svg:rect"`).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Namespace prefix before `:`, if present.
    pub fn prefix(&self) -> Option<&str> {
        self.name.split_once(':').map(|(p, _)| p)
    }

    /// Local element name after `:`, or the full name if unprefixed.
    pub fn local_name(&self) -> &str {
        self.name
            .split_once(':')
            .map_or(self.name.as_str(), |(_, l)| l)
    }

    /// Resolved namespace URI, if the element belongs to one.
    pub fn namespace_uri(&self) -> Option<&str> {
        self.namespace.as_deref()
    }

    /// Set the namespace URI explicitly.
    pub fn set_namespace(&mut self, namespace: Option<String>) {
        self.namespace = namespace;
    }

    /// Look up an attribute value by exact raw name.
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|a| a.name == name)
            .map(|a| a.value.as_str())
    }

    /// Look up an attribute value by `(namespace_uri, local_name)`.
    pub fn attr_ns(&self, namespace: &str, local_name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|a| a.namespace.as_deref() == Some(namespace) && a.local_name() == local_name)
            .map(|a| a.value.as_str())
    }

    /// Slice of all attributes in source/insertion order.
    pub fn attributes(&self) -> &[Attribute] {
        &self.attributes
    }

    /// Mutable slice of all attributes.
    pub fn attributes_mut(&mut self) -> &mut Vec<Attribute> {
        &mut self.attributes
    }

    /// Insert or update an attribute by raw name.
    pub fn set_attr(&mut self, name: impl Into<String>, value: impl Into<String>) {
        let name = name.into();
        let value = value.into();
        if let Some(existing) = self.attributes.iter_mut().find(|a| a.name == name) {
            existing.value = value;
        } else {
            self.attributes.push(Attribute::new(name, value));
        }
    }

    /// Remove an attribute by raw name, returning its value if present.
    pub fn remove_attr(&mut self, name: &str) -> Option<String> {
        let idx = self.attributes.iter().position(|a| a.name == name)?;
        Some(self.attributes.remove(idx).value)
    }

    /// Append a child [`Node`].
    pub fn push_child(&mut self, node: impl Into<Node>) {
        self.children.push(node.into());
    }

    /// All child nodes in document order.
    pub fn children(&self) -> &[Node] {
        &self.children
    }

    /// Mutable vector of child nodes.
    pub fn children_mut(&mut self) -> &mut Vec<Node> {
        &mut self.children
    }

    /// Iterate over direct child [`Element`]s.
    pub fn child_elements(&self) -> impl Iterator<Item = &Element> {
        self.children.iter().filter_map(|n| match n {
            Node::Element(el) => Some(el),
            _ => None,
        })
    }

    /// Find the first direct child element with exact raw name `name`.
    pub fn child(&self, name: &str) -> Option<&Element> {
        self.child_elements().find(|el| el.name == name)
    }

    /// Find the first direct child element matching `(namespace_uri, local_name)`.
    pub fn child_ns(&self, namespace: &str, local_name: &str) -> Option<&Element> {
        self.child_elements()
            .find(|el| el.namespace.as_deref() == Some(namespace) && el.local_name() == local_name)
    }

    /// Iterate over all direct child elements with exact raw name `name`.
    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Element> + 'a {
        self.child_elements().filter(move |el| el.name == name)
    }

    /// Concatenate all descendant [`Node::Text`] and [`Node::CData`] content iteratively.
    ///
    /// Uses an explicit heap stack so deeply nested documents cannot overflow the call stack.
    pub fn text(&self) -> String {
        let mut out = String::new();
        let mut stack: Vec<&Node> = self.children.iter().rev().collect();
        while let Some(node) = stack.pop() {
            match node {
                Node::Text(s) | Node::CData(s) => out.push_str(s),
                Node::Element(el) => {
                    for child in el.children.iter().rev() {
                        stack.push(child);
                    }
                }
                Node::Comment(_) | Node::ProcessingInstruction { .. } => {}
            }
        }
        out
    }
}

impl Drop for Element {
    fn drop(&mut self) {
        if self.children.is_empty() {
            return;
        }
        let mut stack = std::mem::take(&mut self.children);
        while let Some(node) = stack.pop() {
            if let Node::Element(mut el) = node {
                stack.append(&mut el.children);
            }
        }
    }
}

/// An owned XML 1.0 document tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    /// Optional `<?xml ...?>` declaration.
    pub declaration: Option<XmlDecl>,
    /// Comments or processing instructions appearing before the root element.
    pub prolog: Vec<Node>,
    /// Root element of the document.
    pub root: Element,
    /// Comments or processing instructions appearing after the root element.
    pub epilog: Vec<Node>,
}

impl Document {
    /// Wrap a root [`Element`] into a [`Document`] with no declaration, prolog, or epilog.
    pub fn new(root: Element) -> Self {
        Self {
            declaration: None,
            prolog: Vec::new(),
            root,
            epilog: Vec::new(),
        }
    }

    /// Parse a UTF-8 XML string with strict default [`ParseOptions`].
    pub fn parse(input: &str) -> Result<Self> {
        Self::parse_with(input, ParseOptions::new())
    }

    /// Parse a UTF-8 XML string with custom [`ParseOptions`].
    pub fn parse_with(input: &str, options: ParseOptions) -> Result<Self> {
        let reader = Reader::with_options(input, options);
        Self::from_reader(reader)
    }

    /// Parse UTF-8 XML bytes (stripping an optional UTF-8 BOM) with strict default options.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        Self::from_bytes_with(bytes, ParseOptions::new())
    }

    /// Parse UTF-8 XML bytes with custom [`ParseOptions`].
    pub fn from_bytes_with(bytes: &[u8], options: ParseOptions) -> Result<Self> {
        let reader = Reader::from_bytes_with(bytes, options)?;
        Self::from_reader(reader)
    }

    fn from_reader(mut reader: Reader<'_>) -> Result<Self> {
        let options = *reader.options();
        let mut declaration: Option<XmlDecl> = None;
        let mut prolog: Vec<Node> = Vec::new();
        let mut epilog: Vec<Node> = Vec::new();
        let mut root: Option<Element> = None;
        let mut stack: Vec<Element> = Vec::new();
        let mut ns_scopes = NsScopes::default();

        loop {
            let ev = reader.next_event()?;
            match ev {
                Event::Decl(decl) => {
                    declaration = Some(decl);
                }
                Event::Start(tag) => {
                    let depth = stack.len() + 1;
                    let el = build_element(&tag, depth, &mut ns_scopes);
                    stack.push(el);
                }
                Event::Empty(tag) => {
                    let depth = stack.len() + 1;
                    let el = build_element(&tag, depth, &mut ns_scopes);
                    ns_scopes.pop(depth);
                    if let Some(parent) = stack.last_mut() {
                        parent.push_child(Node::Element(el));
                    } else {
                        root = Some(el);
                    }
                }
                Event::End(_) => {
                    let closing_depth = stack.len();
                    ns_scopes.pop(closing_depth);
                    let completed = stack
                        .pop()
                        .ok_or_else(|| Error::new(ErrorKind::MissingRootElement))?;
                    if let Some(parent) = stack.last_mut() {
                        parent.push_child(Node::Element(completed));
                    } else {
                        root = Some(completed);
                    }
                }
                Event::Text(text) => {
                    if !options.preserve_whitespace && text.chars().all(is_xml_whitespace) {
                        continue;
                    }
                    if let Some(parent) = stack.last_mut() {
                        parent.push_child(Node::Text(text.into_owned()));
                    }
                }
                Event::CData(data) => {
                    if let Some(parent) = stack.last_mut() {
                        parent.push_child(Node::CData(data.into_owned()));
                    }
                }
                Event::Comment(comment) => {
                    if !options.preserve_comments {
                        continue;
                    }
                    let node = Node::Comment(comment.into_owned());
                    if let Some(parent) = stack.last_mut() {
                        parent.push_child(node);
                    } else if root.is_none() {
                        prolog.push(node);
                    } else {
                        epilog.push(node);
                    }
                }
                Event::PI { target, data } => {
                    if !options.preserve_pis {
                        continue;
                    }
                    let node = Node::ProcessingInstruction {
                        target: target.to_string(),
                        data: data.map(|d| d.into_owned()),
                    };
                    if let Some(parent) = stack.last_mut() {
                        parent.push_child(node);
                    } else if root.is_none() {
                        prolog.push(node);
                    } else {
                        epilog.push(node);
                    }
                }
                Event::Eof => break,
            }
        }

        let root = root.ok_or_else(|| Error::new(ErrorKind::MissingRootElement))?;
        Ok(Self {
            declaration,
            prolog,
            root,
            epilog,
        })
    }

    /// Borrow the root element.
    pub fn root(&self) -> &Element {
        &self.root
    }

    /// Mutably borrow the root element.
    pub fn root_mut(&mut self) -> &mut Element {
        &mut self.root
    }
}

impl FromStr for Document {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        Self::parse(s)
    }
}

/// In-scope namespace bindings: prefix (`""` = default) -> stack of URIs, plus an
/// undo log so lookups are O(1) and pops are amortised O(1) per declaration.
#[derive(Default)]
struct NsScopes {
    map: HashMap<String, Vec<Option<String>>>,
    undo: Vec<(usize, String)>,
}

impl NsScopes {
    fn push(&mut self, depth: usize, prefix: &str, uri: Option<String>) {
        self.map.entry(prefix.to_string()).or_default().push(uri);
        self.undo.push((depth, prefix.to_string()));
    }

    fn lookup(&self, prefix: &str) -> Option<&str> {
        if prefix == "xml" {
            return Some(XML_NAMESPACE);
        }
        self.map
            .get(prefix)
            .and_then(|stack| stack.last())
            .and_then(|uri| uri.as_deref())
    }

    fn pop(&mut self, depth: usize) {
        while let Some((d, _)) = self.undo.last() {
            if *d < depth {
                break;
            }
            let Some((_, prefix)) = self.undo.pop() else {
                break;
            };
            if let Some(stack) = self.map.get_mut(&prefix) {
                stack.pop();
                if stack.is_empty() {
                    self.map.remove(&prefix);
                }
            }
        }
    }
}

fn build_element(tag: &StartTag<'_>, depth: usize, ns_scopes: &mut NsScopes) -> Element {
    for attr in tag.attributes() {
        if attr.name == "xmlns" {
            let uri = if attr.value.is_empty() {
                None
            } else {
                Some(attr.value.as_ref().to_string())
            };
            ns_scopes.push(depth, "", uri);
        } else if let Some(prefix) = attr.name.strip_prefix("xmlns:") {
            ns_scopes.push(depth, prefix, Some(attr.value.as_ref().to_string()));
        }
    }

    let elem_ns = ns_scopes
        .lookup(tag.prefix().unwrap_or(""))
        .map(str::to_string);

    let mut attributes = Vec::with_capacity(tag.attributes().len());
    for raw in tag.attributes() {
        let attr_ns = if raw.name == "xmlns" || raw.name.starts_with("xmlns:") {
            Some(XMLNS_NAMESPACE.to_string())
        } else if let Some(prefix) = raw.prefix() {
            ns_scopes.lookup(prefix).map(str::to_string)
        } else {
            None
        };
        attributes.push(Attribute {
            name: raw.name.to_string(),
            value: raw.value.as_ref().to_string(),
            namespace: attr_ns,
        });
    }

    Element {
        name: tag.name().to_string(),
        namespace: elem_ns,
        attributes,
        children: Vec::new(),
    }
}
