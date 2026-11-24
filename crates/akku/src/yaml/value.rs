//! YAML values: [`Value`], [`Mapping`] and [`Key`].
//!
//! A mapping keeps its insertion order in a `Vec` of pairs and indexes every
//! key for constant-time lookup: scalar keys by their [`Key`], sequence and
//! mapping keys by a keyed structural fingerprint. Cloning, comparing,
//! formatting and dropping a value are all iterative, so hostile nesting
//! cannot overflow the stack.

use std::collections::hash_map::RandomState;
use std::collections::HashMap;
use std::fmt;
use std::hash::{BuildHasher, Hash, Hasher};
use std::ops::Index;
use std::sync::OnceLock;

/// A comparable, hashable form of a scalar mapping key.
///
/// Sequence and mapping keys have no [`Key`]; they are indexed by a keyed
/// structural fingerprint and confirmed by an iterative structural comparison.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Key {
    /// The `null` key.
    Null,
    /// A boolean key.
    Bool(bool),
    /// An integer key.
    Int(i64),
    /// A floating-point key, compared by bit pattern.
    FloatBits(u64),
    /// A string key.
    Str(String),
}

impl Key {
    /// The key form of a scalar value.
    pub(crate) fn from_value(value: &Value) -> Option<Self> {
        Some(match value {
            Value::Null => Self::Null,
            Value::Bool(inner) => Self::Bool(*inner),
            Value::Int(inner) => Self::Int(*inner),
            Value::Float(inner) => Self::FloatBits(inner.to_bits()),
            Value::String(inner) => Self::Str(inner.clone()),
            _ => return None,
        })
    }
}

/// A YAML mapping that preserves insertion order.
///
/// Every key is indexed, so `get` is expected constant time (plus the size of
/// a sequence or mapping key being looked up) and a duplicate is detected
/// without scanning. Scalar keys are indexed by their [`Key`]; sequence and
/// mapping keys (only reachable with
/// [`super::ParseOptions::allow_complex_keys`] or through [`Mapping::insert`])
/// by a keyed structural fingerprint that is cached per entry, so nested
/// complex keys are never re-hashed.
///
/// Equality ignores the order of the entries: two mappings are equal when they
/// hold the same keys with equal values.
#[derive(Default)]
pub struct Mapping {
    pub(crate) entries: Vec<(Value, Value)>,
    /// Scalar key to position.
    index: HashMap<Key, usize>,
    /// The index of sequence and mapping keys, boxed and created on first use
    /// so a mapping (and therefore every [`Value`]) stays small.
    complex: Option<Box<ComplexIndex>>,
}

/// The fingerprint index of a mapping's sequence and mapping keys.
#[derive(Default)]
struct ComplexIndex {
    /// Fingerprint to the positions holding a key with it.
    by_hash: HashMap<u64, Vec<usize>>,
    /// Position to the cached fingerprint of the key there.
    at: HashMap<usize, u64>,
}

impl Mapping {
    /// An empty mapping.
    pub fn new() -> Self {
        Self::default()
    }

    /// An empty mapping with room for `capacity` entries.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            entries: Vec::with_capacity(capacity),
            index: HashMap::with_capacity(capacity),
            complex: None,
        }
    }

    /// The number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the mapping has no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Remove all entries.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.index.clear();
        self.complex = None;
    }

    /// The position of `key`, if present.
    fn position(&self, key: &Value) -> Option<usize> {
        match Key::from_value(key) {
            Some(index) => self.index.get(&index).copied(),
            None => self.find_complex(fingerprint(key), key),
        }
    }

    /// The positions of sequence and mapping keys with this fingerprint.
    fn complex_candidates(&self, hash: u64) -> &[usize] {
        self.complex
            .as_ref()
            .and_then(|complex| complex.by_hash.get(&hash))
            .map_or(&[], Vec::as_slice)
    }

    /// The fingerprint of the key at `position`, cached for complex keys.
    fn key_hash(&self, position: usize) -> u64 {
        let key = &self.entries[position].0;
        if Key::from_value(key).is_some() {
            return fingerprint(key);
        }
        self.complex
            .as_ref()
            .and_then(|complex| complex.at.get(&position))
            .copied()
            .expect("every complex key has a cached fingerprint")
    }

    /// The position of a sequence or mapping key with a known fingerprint.
    fn find_complex(&self, hash: u64, key: &Value) -> Option<usize> {
        self.complex_candidates(hash)
            .iter()
            .copied()
            .find(|&position| equal(Node::Val(&self.entries[position].0), Node::Val(key)))
    }

    /// Record the key at the end of `entries` in the indexes. `hash` is its
    /// fingerprint when it is a sequence or mapping key.
    fn index_last(&mut self, hash: Option<u64>) {
        let position = self.entries.len() - 1;
        match (Key::from_value(&self.entries[position].0), hash) {
            (Some(scalar), _) => {
                self.index.insert(scalar, position);
            }
            (None, hash) => {
                let hash = hash.unwrap_or_else(|| fingerprint(&self.entries[position].0));
                let complex = self.complex.get_or_insert_with(Default::default);
                complex.by_hash.entry(hash).or_default().push(position);
                complex.at.insert(position, hash);
            }
        }
    }

    /// The value stored under `key`.
    pub fn get(&self, key: &Value) -> Option<&Value> {
        let position = self.position(key)?;
        Some(&self.entries[position].1)
    }

    /// The value stored under a string key.
    pub fn get_str(&self, key: &str) -> Option<&Value> {
        let position = *self.index.get(&Key::Str(key.to_string()))?;
        Some(&self.entries[position].1)
    }

    /// The value stored under `key`, mutably.
    pub fn get_mut(&mut self, key: &Value) -> Option<&mut Value> {
        let position = self.position(key)?;
        Some(&mut self.entries[position].1)
    }

    /// Whether an entry with this key exists.
    pub fn contains_key(&self, key: &Value) -> bool {
        self.get(key).is_some()
    }

    /// Insert an entry, returning the value it replaced.
    ///
    /// A replaced key keeps its original position.
    pub fn insert(&mut self, key: Value, value: Value) -> Option<Value> {
        let (existing, hash) = match Key::from_value(&key) {
            Some(scalar) => (self.index.get(&scalar).copied(), None),
            None => {
                let hash = fingerprint(&key);
                (self.find_complex(hash, &key), Some(hash))
            }
        };
        if let Some(position) = existing {
            return Some(std::mem::replace(&mut self.entries[position].1, value));
        }
        self.entries.push((key, value));
        self.index_last(hash);
        None
    }

    /// Insert a string key.
    pub fn insert_str(&mut self, key: &str, value: impl Into<Value>) -> Option<Value> {
        self.insert(Value::String(key.to_string()), value.into())
    }

    /// Insert without duplicate detection.
    pub(crate) fn insert_raw(&mut self, key: Value, value: Value) {
        self.entries.push((key, value));
        self.index_last(None);
    }

    /// Remove an entry, returning its value.
    pub fn remove(&mut self, key: &Value) -> Option<Value> {
        let position = self.position(key)?;
        let (_, value) = self.entries.remove(position);
        self.reindex(position);
        Some(value)
    }

    /// Iterate over the entries in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = (&Value, &Value)> {
        self.entries.iter().map(|(key, value)| (key, value))
    }

    /// Iterate over the keys in insertion order.
    pub fn keys(&self) -> impl Iterator<Item = &Value> {
        self.entries.iter().map(|(key, _)| key)
    }

    /// Iterate over the values in insertion order.
    pub fn values(&self) -> impl Iterator<Item = &Value> {
        self.entries.iter().map(|(_, value)| value)
    }

    /// Take the entries out, leaving the mapping empty.
    pub(crate) fn take_entries(&mut self) -> Vec<(Value, Value)> {
        self.index.clear();
        self.complex = None;
        std::mem::take(&mut self.entries)
    }

    /// Rebuild the key indexes after the entry at `removed` was taken out,
    /// reusing the cached fingerprints of complex keys.
    fn reindex(&mut self, removed: usize) {
        self.index.clear();
        let old = self.complex.take();
        for (position, (key, _)) in self.entries.iter().enumerate() {
            if let Some(scalar) = Key::from_value(key) {
                self.index.insert(scalar, position);
            }
        }
        let Some(old) = old else {
            return;
        };
        let mut complex = ComplexIndex::default();
        for (old_position, hash) in old.at {
            let position = match old_position.cmp(&removed) {
                std::cmp::Ordering::Less => old_position,
                std::cmp::Ordering::Equal => continue,
                std::cmp::Ordering::Greater => old_position - 1,
            };
            complex.by_hash.entry(hash).or_default().push(position);
            complex.at.insert(position, hash);
        }
        if !complex.at.is_empty() {
            self.complex = Some(Box::new(complex));
        }
    }
}

impl PartialEq for Mapping {
    /// Compare the entry sets, ignoring insertion order.
    fn eq(&self, other: &Self) -> bool {
        equal(Node::Map(self), Node::Map(other))
    }
}

impl fmt::Debug for Mapping {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        debug_tree(Node::Map(self), f)
    }
}

/// The process-wide keyed hasher behind key fingerprints.
///
/// It is randomly keyed per process, so an attacker cannot precompute
/// colliding keys.
fn hash_state() -> &'static RandomState {
    static STATE: OnceLock<RandomState> = OnceLock::new();
    STATE.get_or_init(RandomState::new)
}

/// A structural fingerprint consistent with [`Value`] equality.
///
/// Computed iteratively. Mapping keys use the fingerprints cached in their
/// mapping, so a key is hashed once when it is inserted and never again.
/// A mapping's fingerprint ignores entry order, like its equality.
fn fingerprint(value: &Value) -> u64 {
    enum Step<'a> {
        Visit(&'a Value),
        Seq(usize),
        Map(&'a Mapping),
    }
    let state = hash_state();
    let mut work = vec![Step::Visit(value)];
    let mut done: Vec<u64> = Vec::new();
    while let Some(step) = work.pop() {
        match step {
            Step::Visit(Value::Sequence(items)) => {
                work.push(Step::Seq(items.len()));
                work.extend(items.iter().rev().map(Step::Visit));
            }
            Step::Visit(Value::Mapping(mapping)) => {
                work.push(Step::Map(mapping));
                work.extend(
                    mapping
                        .entries
                        .iter()
                        .rev()
                        .map(|(_, item)| Step::Visit(item)),
                );
            }
            Step::Visit(scalar) => {
                let mut hasher = state.build_hasher();
                match scalar {
                    Value::Null => 0u8.hash(&mut hasher),
                    Value::Bool(inner) => (1u8, *inner).hash(&mut hasher),
                    Value::Int(inner) => (2u8, *inner).hash(&mut hasher),
                    Value::Float(inner) => (3u8, inner.to_bits()).hash(&mut hasher),
                    Value::String(inner) => (4u8, inner.as_str()).hash(&mut hasher),
                    Value::Sequence(_) | Value::Mapping(_) => {}
                }
                done.push(hasher.finish());
            }
            Step::Seq(len) => {
                let mut hasher = state.build_hasher();
                (5u8, len).hash(&mut hasher);
                for item in done.drain(done.len() - len..) {
                    hasher.write_u64(item);
                }
                done.push(hasher.finish());
            }
            Step::Map(mapping) => {
                let len = mapping.entries.len();
                let mut sum = 0u64;
                for (position, item) in done.drain(done.len() - len..).enumerate() {
                    let pair = state.hash_one((mapping.key_hash(position), item));
                    sum = sum.wrapping_add(pair);
                }
                done.push(state.hash_one((6u8, len, sum)));
            }
        }
    }
    done.pop().expect("the root has a fingerprint")
}

/// The mapping behind a node, if it is one.
fn node_mapping<'a>(node: Node<'a>) -> Option<&'a Mapping> {
    match node {
        Node::Map(mapping) | Node::Val(Value::Mapping(mapping)) => Some(mapping),
        Node::Val(_) => None,
    }
}

/// Structural equality with an explicit work stack.
///
/// Mapping keys and values are compared on the same stack, so nesting through
/// keys is as safe as nesting through values. A nested call happens only when
/// two distinct complex keys share a 64-bit keyed fingerprint.
fn equal(left: Node<'_>, right: Node<'_>) -> bool {
    let mut stack: Vec<(Node<'_>, Node<'_>)> = vec![(left, right)];
    while let Some((left, right)) = stack.pop() {
        if let (Some(a), Some(b)) = (node_mapping(left), node_mapping(right)) {
            if a.entries.len() != b.entries.len() {
                return false;
            }
            for (position, (key, value)) in a.entries.iter().enumerate() {
                let found = match Key::from_value(key) {
                    Some(scalar) => b.index.get(&scalar).copied(),
                    None => match b.complex_candidates(a.key_hash(position)) {
                        &[only] => {
                            stack.push((Node::Val(key), Node::Val(&b.entries[only].0)));
                            Some(only)
                        }
                        candidates => candidates.iter().copied().find(|&candidate| {
                            equal(Node::Val(key), Node::Val(&b.entries[candidate].0))
                        }),
                    },
                };
                match found {
                    Some(other) => stack.push((Node::Val(value), Node::Val(&b.entries[other].1))),
                    None => return false,
                }
            }
            continue;
        }
        let (Node::Val(left), Node::Val(right)) = (left, right) else {
            return false;
        };
        match (left, right) {
            (Value::Null, Value::Null) => {}
            (Value::Bool(a), Value::Bool(b)) if a == b => {}
            (Value::Int(a), Value::Int(b)) if a == b => {}
            (Value::Float(a), Value::Float(b)) if a.to_bits() == b.to_bits() => {}
            (Value::String(a), Value::String(b)) if a == b => {}
            (Value::Sequence(a), Value::Sequence(b)) if a.len() == b.len() => {
                for (left_item, right_item) in a.iter().zip(b.iter()) {
                    stack.push((Node::Val(left_item), Node::Val(right_item)));
                }
            }
            _ => return false,
        }
    }
    true
}

/// One unit of `Debug` output.
enum DebugPiece<'a> {
    Text(&'static str),
    Node(Node<'a>),
}

/// Format a tree for `Debug` with an explicit work stack.
///
/// Scalars print like a derived `Debug` (`Int(1)`, `String("a")`); sequences
/// print as `Sequence([..])` and mappings as `Mapping({key: value, ..})`.
/// The alternate (`{:#?}`) flag is accepted but output stays on one line.
fn debug_tree(root: Node<'_>, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    let mut stack = vec![DebugPiece::Node(root)];
    while let Some(piece) = stack.pop() {
        match piece {
            DebugPiece::Text(text) => f.write_str(text)?,
            DebugPiece::Node(Node::Map(mapping)) => {
                stack.push(DebugPiece::Text("}"));
                for (position, (key, value)) in mapping.entries.iter().enumerate().rev() {
                    stack.push(DebugPiece::Node(Node::Val(value)));
                    stack.push(DebugPiece::Text(": "));
                    stack.push(DebugPiece::Node(Node::Val(key)));
                    if position > 0 {
                        stack.push(DebugPiece::Text(", "));
                    }
                }
                stack.push(DebugPiece::Text("{"));
            }
            DebugPiece::Node(Node::Val(value)) => match value {
                Value::Null => f.write_str("Null")?,
                Value::Bool(inner) => write!(f, "Bool({inner:?})")?,
                Value::Int(inner) => write!(f, "Int({inner:?})")?,
                Value::Float(inner) => write!(f, "Float({inner:?})")?,
                Value::String(inner) => write!(f, "String({inner:?})")?,
                Value::Sequence(items) => {
                    stack.push(DebugPiece::Text("])"));
                    for (position, item) in items.iter().enumerate().rev() {
                        stack.push(DebugPiece::Node(Node::Val(item)));
                        if position > 0 {
                            stack.push(DebugPiece::Text(", "));
                        }
                    }
                    f.write_str("Sequence([")?;
                }
                Value::Mapping(mapping) => {
                    stack.push(DebugPiece::Text(")"));
                    stack.push(DebugPiece::Node(Node::Map(mapping)));
                    f.write_str("Mapping(")?;
                }
            },
        }
    }
    Ok(())
}

impl Clone for Mapping {
    fn clone(&self) -> Self {
        unflatten(flatten(Node::Map(self)))
            .into_mapping()
            .expect("flattening a mapping yields a mapping")
    }
}

impl IntoIterator for Mapping {
    type Item = (Value, Value);
    type IntoIter = std::vec::IntoIter<(Value, Value)>;

    fn into_iter(mut self) -> Self::IntoIter {
        self.index.clear();
        self.complex = None;
        std::mem::take(&mut self.entries).into_iter()
    }
}

impl<'a> IntoIterator for &'a Mapping {
    type Item = (&'a Value, &'a Value);
    type IntoIter = Box<dyn Iterator<Item = (&'a Value, &'a Value)> + 'a>;

    fn into_iter(self) -> Self::IntoIter {
        Box::new(self.iter())
    }
}

/// A YAML node.
///
/// Values compare structurally. Floats compare by bit pattern, so `NaN` equals
/// `NaN` and `0.0` does not equal `-0.0`; that keeps a parsed document and its
/// re-parsed serialization exactly equal.
///
/// Dropping, cloning, comparing, `Debug`-formatting and serializing a value
/// never recurse, whether the nesting runs through sequence items, mapping
/// values or mapping keys, so a deeply nested document cannot overflow the
/// stack.
#[derive(Default)]
pub enum Value {
    /// The `null` value.
    #[default]
    Null,
    /// `true` or `false`.
    Bool(bool),
    /// A 64-bit signed integer.
    Int(i64),
    /// A 64-bit float.
    Float(f64),
    /// A string.
    String(String),
    /// A sequence.
    Sequence(Vec<Value>),
    /// A mapping that preserves insertion order.
    Mapping(Mapping),
}

/// The flattening of one value into an arena slot.
enum Flat {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Seq(Vec<usize>),
    Map(Vec<(usize, usize)>),
}

/// A borrowed node, so a mapping can be cloned without wrapping it in a value.
#[derive(Clone, Copy)]
enum Node<'a> {
    Val(&'a Value),
    Map(&'a Mapping),
}

/// Flatten a borrowed tree into an arena of payloads plus child indices.
fn flatten(root: Node<'_>) -> Vec<Flat> {
    let mut flat: Vec<Flat> = vec![Flat::Null];
    let mut stack: Vec<(Node<'_>, usize)> = vec![(root, 0)];
    while let Some((node, slot)) = stack.pop() {
        match node {
            Node::Val(Value::Null) => flat[slot] = Flat::Null,
            Node::Val(Value::Bool(inner)) => flat[slot] = Flat::Bool(*inner),
            Node::Val(Value::Int(inner)) => flat[slot] = Flat::Int(*inner),
            Node::Val(Value::Float(inner)) => flat[slot] = Flat::Float(*inner),
            Node::Val(Value::String(inner)) => flat[slot] = Flat::Str(inner.clone()),
            Node::Val(Value::Sequence(items)) => {
                let mut children = Vec::with_capacity(items.len());
                for item in items {
                    let child = flat.len();
                    flat.push(Flat::Null);
                    children.push(child);
                    stack.push((Node::Val(item), child));
                }
                flat[slot] = Flat::Seq(children);
            }
            Node::Val(Value::Mapping(mapping)) => push_map(&mut flat, &mut stack, mapping, slot),
            Node::Map(mapping) => push_map(&mut flat, &mut stack, mapping, slot),
        }
    }
    flat
}

/// Push the children of a mapping onto the arena and the work stack.
fn push_map<'a>(
    flat: &mut Vec<Flat>,
    stack: &mut Vec<(Node<'a>, usize)>,
    mapping: &'a Mapping,
    slot: usize,
) {
    let mut children = Vec::with_capacity(mapping.entries.len());
    for (key, value) in &mapping.entries {
        let key_slot = flat.len();
        flat.push(Flat::Null);
        let value_slot = flat.len();
        flat.push(Flat::Null);
        children.push((key_slot, value_slot));
        stack.push((Node::Val(key), key_slot));
        stack.push((Node::Val(value), value_slot));
    }
    flat[slot] = Flat::Map(children);
}

/// Rebuild a tree from the arena in reverse order, children before parents.
fn unflatten(flat: Vec<Flat>) -> Value {
    let mut built: Vec<Option<Value>> = std::iter::repeat_with(|| None).take(flat.len()).collect();
    for slot in (0..flat.len()).rev() {
        let value = match &flat[slot] {
            Flat::Null => Value::Null,
            Flat::Bool(inner) => Value::Bool(*inner),
            Flat::Int(inner) => Value::Int(*inner),
            Flat::Float(inner) => Value::Float(*inner),
            Flat::Str(inner) => Value::String(inner.clone()),
            Flat::Seq(children) => Value::Sequence(
                children
                    .iter()
                    .map(|&child| built[child].take().expect("child built"))
                    .collect(),
            ),
            Flat::Map(children) => {
                let mut mapping = Mapping::with_capacity(children.len());
                for &(key, value) in children {
                    mapping.insert_raw(
                        built[key].take().expect("key built"),
                        built[value].take().expect("value built"),
                    );
                }
                Value::Mapping(mapping)
            }
        };
        built[slot] = Some(value);
    }
    built[0].take().expect("root built")
}

impl Clone for Value {
    fn clone(&self) -> Self {
        unflatten(flatten(Node::Val(self)))
    }
}

impl Drop for Value {
    fn drop(&mut self) {
        // A recursive drop would overflow the stack on deeply nested input,
        // whether the nesting runs through sequence items, mapping values or
        // mapping keys. Move every child (keys included) onto a heap worklist
        // first, so each value is dropped only once it has no children left.
        let mut worklist: Vec<Value> = Vec::new();
        drain_children(self, &mut worklist);
        while let Some(mut value) = worklist.pop() {
            drain_children(&mut value, &mut worklist);
        }
    }
}

/// Move the direct children of `value` (sequence items, mapping keys and
/// values) onto `worklist`, leaving `value` childless.
fn drain_children(value: &mut Value, worklist: &mut Vec<Value>) {
    match value {
        Value::Sequence(items) => worklist.append(items),
        Value::Mapping(mapping) => {
            for (key, child) in mapping.take_entries() {
                worklist.push(key);
                worklist.push(child);
            }
        }
        _ => {}
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        equal(Node::Val(self), Node::Val(other))
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        debug_tree(Node::Val(self), f)
    }
}

impl Value {
    /// Take the mapping out of a value, leaving `null`-like emptiness behind.
    ///
    /// Needed because a type with a [`Drop`] implementation cannot be
    /// destructured by move.
    fn into_mapping(mut self) -> Option<Mapping> {
        match &mut self {
            Self::Mapping(mapping) => Some(std::mem::take(mapping)),
            _ => None,
        }
    }

    /// The YAML type name, as used in error messages.
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Null => "null",
            Self::Bool(_) => "bool",
            Self::Int(_) => "int",
            Self::Float(_) => "float",
            Self::String(_) => "string",
            Self::Sequence(_) => "sequence",
            Self::Mapping(_) => "mapping",
        }
    }

    /// Whether this is `null`.
    pub fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }

    /// Whether this is a boolean.
    pub fn is_bool(&self) -> bool {
        matches!(self, Self::Bool(_))
    }

    /// Whether this is an integer.
    pub fn is_int(&self) -> bool {
        matches!(self, Self::Int(_))
    }

    /// Whether this is a float.
    pub fn is_float(&self) -> bool {
        matches!(self, Self::Float(_))
    }

    /// Whether this is an integer or a float.
    pub fn is_number(&self) -> bool {
        matches!(self, Self::Int(_) | Self::Float(_))
    }

    /// Whether this is a string.
    pub fn is_string(&self) -> bool {
        matches!(self, Self::String(_))
    }

    /// Whether this is a sequence.
    pub fn is_sequence(&self) -> bool {
        matches!(self, Self::Sequence(_))
    }

    /// Whether this is a mapping.
    pub fn is_mapping(&self) -> bool {
        matches!(self, Self::Mapping(_))
    }

    /// The boolean, if this is one.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(inner) => Some(*inner),
            _ => None,
        }
    }

    /// The integer, if this is one.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Int(inner) => Some(*inner),
            _ => None,
        }
    }

    /// The number as `f64`; integers are converted and may lose precision.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Int(inner) => Some(*inner as f64),
            Self::Float(inner) => Some(*inner),
            _ => None,
        }
    }

    /// The string, if this is one.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(inner) => Some(inner),
            _ => None,
        }
    }

    /// The sequence, if this is one.
    pub fn as_sequence(&self) -> Option<&Vec<Value>> {
        match self {
            Self::Sequence(inner) => Some(inner),
            _ => None,
        }
    }

    /// The mapping, if this is one.
    pub fn as_mapping(&self) -> Option<&Mapping> {
        match self {
            Self::Mapping(inner) => Some(inner),
            _ => None,
        }
    }

    /// The entry under a string key, if this is a mapping.
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Self::Mapping(mapping) => mapping.get_str(key),
            _ => None,
        }
    }

    /// Look a value up by a dotted path such as `server.tls.0.cert`.
    ///
    /// Numeric segments index into sequences, so the syntax matches
    /// [`akku::json::Value::get_path`](crate::json::Value::get_path).
    pub fn get_path(&self, path: &str) -> Option<&Value> {
        let mut current = self;
        for segment in path.split('.').filter(|segment| !segment.is_empty()) {
            current = match current {
                Self::Mapping(mapping) => mapping.get_str(segment)?,
                Self::Sequence(items) => items.get(segment.parse::<usize>().ok()?)?,
                _ => return None,
            };
        }
        Some(current)
    }

    /// The number of nodes in this tree, counted iteratively.
    pub fn node_count(&self) -> usize {
        let mut count = 0;
        let mut stack = vec![self];
        while let Some(value) = stack.pop() {
            count += 1;
            match value {
                Self::Sequence(items) => stack.extend(items.iter()),
                Self::Mapping(mapping) => {
                    for (key, value) in &mapping.entries {
                        stack.push(key);
                        stack.push(value);
                    }
                }
                _ => {}
            }
        }
        count
    }

    /// The nesting depth of this tree, computed iteratively.
    pub fn depth(&self) -> usize {
        let mut deepest = 0;
        let mut stack = vec![(self, 1usize)];
        while let Some((value, depth)) = stack.pop() {
            deepest = deepest.max(depth);
            match value {
                Self::Sequence(items) => {
                    for item in items {
                        stack.push((item, depth + 1));
                    }
                }
                Self::Mapping(mapping) => {
                    for (key, value) in &mapping.entries {
                        stack.push((key, depth + 1));
                        stack.push((value, depth + 1));
                    }
                }
                _ => {}
            }
        }
        deepest
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&super::to_string(self))
    }
}

impl std::str::FromStr for Value {
    type Err = super::error::Error;

    fn from_str(text: &str) -> super::error::Result<Self> {
        super::parse(text)
    }
}

impl Index<&str> for Value {
    type Output = Value;

    fn index(&self, key: &str) -> &Self::Output {
        self.get(key)
            .unwrap_or_else(|| panic!("no entry {key:?} in this mapping"))
    }
}

impl Index<usize> for Value {
    type Output = Value;

    fn index(&self, index: usize) -> &Self::Output {
        match self {
            Self::Sequence(items) => &items[index],
            _ => panic!("cannot index a {} by position", self.type_name()),
        }
    }
}

impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl From<i64> for Value {
    fn from(value: i64) -> Self {
        Self::Int(value)
    }
}

impl From<i32> for Value {
    fn from(value: i32) -> Self {
        Self::Int(i64::from(value))
    }
}

impl From<f64> for Value {
    fn from(value: f64) -> Self {
        Self::Float(value)
    }
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Self::String(value.to_string())
    }
}

impl From<String> for Value {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl From<Vec<Value>> for Value {
    fn from(value: Vec<Value>) -> Self {
        Self::Sequence(value)
    }
}

impl From<Mapping> for Value {
    fn from(value: Mapping) -> Self {
        Self::Mapping(value)
    }
}

impl From<()> for Value {
    fn from((): ()) -> Self {
        Self::Null
    }
}

impl FromIterator<(Value, Value)> for Mapping {
    fn from_iter<I: IntoIterator<Item = (Value, Value)>>(iter: I) -> Self {
        let mut mapping = Self::new();
        for (key, value) in iter {
            mapping.insert(key, value);
        }
        mapping
    }
}

impl FromIterator<(Value, Value)> for Value {
    fn from_iter<I: IntoIterator<Item = (Value, Value)>>(iter: I) -> Self {
        Self::Mapping(Mapping::from_iter(iter))
    }
}
