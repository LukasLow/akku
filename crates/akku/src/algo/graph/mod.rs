//! Compact, index-based directed and undirected graphs.
//!
//! [`Graph<N, E>`] stores nodes and edges in two contiguous arenas. Each node
//! keeps the head and tail of an intrusive, index-linked list of its outgoing
//! and incoming edges, so adjacency walks touch only flat `Vec` storage (good
//! cache locality, no per-node allocations) and iterate in insertion order.
//! [`NodeId`] and [`EdgeId`] are plain indices that stay stable because the
//! arenas are append-only.
//!
//! Algorithms are methods on [`Graph`]:
//!
//! | Method | Algorithm | Notes |
//! | --- | --- | --- |
//! | [`bfs`](Graph::bfs) / [`dfs`](Graph::dfs) | breadth-/depth-first traversal | lazy iterators; DFS is iterative (stack-safe) |
//! | [`toposort`](Graph::toposort) | Kahn's algorithm | [`Error::Cycle`] on cycles |
//! | [`strongly_connected_components`](Graph::strongly_connected_components) | Tarjan | iterative (stack-safe) |
//! | [`dijkstra`](Graph::dijkstra) / [`shortest_path`](Graph::shortest_path) | Dijkstra | [`Error::NegativeWeight`] on negative edges |
//! | [`astar`](Graph::astar) | A* | heuristic estimates are validated |
//! | [`bellman_ford`](Graph::bellman_ford) | Bellman-Ford | [`Error::NegativeCycle`] detection |
//! | [`minimum_spanning_tree`](Graph::minimum_spanning_tree) | Kruskal | minimum spanning forest |
//!
//! Weighted algorithms require `E: `[`Number`](super::Number); weights that
//! are `NaN` or infinite are rejected with [`Error::InvalidWeight`] and
//! overflowing sums with [`Error::Overflow`].
//!
//! # Examples
//!
//! ```
//! use akku::algo::graph::Graph;
//!
//! let mut g = Graph::<(), ()>::directed();
//! let shirt = g.add_node(());
//! let tie = g.add_node(());
//! let jacket = g.add_node(());
//! g.add_edge(shirt, tie, ())?;
//! g.add_edge(tie, jacket, ())?;
//! g.add_edge(shirt, jacket, ())?;
//!
//! assert_eq!(g.toposort()?, [shirt, tie, jacket]);
//! assert_eq!(g.dfs(shirt)?.collect::<Vec<_>>(), [shirt, tie, jacket]);
//! # Ok::<(), akku::algo::Error>(())
//! ```

mod mst;
mod path;
mod traverse;

pub use mst::SpanningTree;
pub use path::{Path, ShortestPaths};
pub use traverse::{Bfs, Dfs};

use super::Error;

/// Sentinel "no edge" link inside the intrusive adjacency lists.
const END: usize = usize::MAX;
/// Adjacency list of edges leaving a node (edge `ends[0]` is the node).
const OUT: usize = 0;
/// Adjacency list of edges entering a node (edge `ends[1]` is the node).
const IN: usize = 1;

/// Stable identifier of a node in a [`Graph`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(usize);

impl NodeId {
    /// Creates an identifier from a raw node index.
    ///
    /// The index is validated when the identifier is used; invalid
    /// identifiers produce [`Error::InvalidNode`] or `None`.
    pub const fn new(index: usize) -> Self {
        Self(index)
    }

    /// Returns the raw node index (nodes are numbered `0..node_count`).
    pub const fn index(self) -> usize {
        self.0
    }
}

/// Stable identifier of an edge in a [`Graph`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EdgeId(usize);

impl EdgeId {
    /// Creates an identifier from a raw edge index.
    pub const fn new(index: usize) -> Self {
        Self(index)
    }

    /// Returns the raw edge index (edges are numbered `0..edge_count`).
    pub const fn index(self) -> usize {
        self.0
    }
}

#[derive(Debug, Clone)]
struct Node<N> {
    weight: N,
    /// First edge of the `[OUT, IN]` adjacency lists.
    first: [usize; 2],
    /// Last edge of the `[OUT, IN]` adjacency lists (for O(1) appends).
    last: [usize; 2],
}

#[derive(Debug, Clone)]
struct Edge<E> {
    weight: E,
    /// `[source, target]` node indices.
    ends: [usize; 2],
    /// Next edge in the source's `OUT` list and the target's `IN` list.
    next: [usize; 2],
}

/// Position inside a node's adjacency lists.
#[derive(Debug, Clone, Copy)]
struct Cursor {
    list: usize,
    edge: usize,
}

/// An arena-backed graph with node weights `N` and edge weights `E`.
///
/// Parallel edges and self-loops are allowed. In an undirected graph every
/// edge is traversable from both endpoints; a self-loop is reported once.
///
/// # Examples
///
/// ```
/// use akku::algo::graph::{Graph, NodeId};
///
/// let mut g = Graph::<&str, f64>::undirected();
/// let a = g.add_node("a");
/// let b = g.add_node("b");
/// let ab = g.add_edge(a, b, 2.5)?;
///
/// assert_eq!(g.node(a), Some(&"a"));
/// assert_eq!(g.edge_weight(ab), Some(&2.5));
/// assert_eq!(g.neighbors(b)?.collect::<Vec<_>>(), [a]);
/// assert!(g.add_edge(a, NodeId::new(7), 1.0).is_err());
/// # Ok::<(), akku::algo::Error>(())
/// ```
#[derive(Debug, Clone)]
pub struct Graph<N, E> {
    nodes: Vec<Node<N>>,
    edges: Vec<Edge<E>>,
    directed: bool,
}

impl<N, E> Graph<N, E> {
    /// Creates an empty directed graph.
    pub fn directed() -> Self {
        Self::with_capacity(true, 0, 0)
    }

    /// Creates an empty undirected graph.
    pub fn undirected() -> Self {
        Self::with_capacity(false, 0, 0)
    }

    /// Creates an empty graph with room for `nodes` nodes and `edges` edges.
    pub fn with_capacity(directed: bool, nodes: usize, edges: usize) -> Self {
        Self {
            nodes: Vec::with_capacity(nodes),
            edges: Vec::with_capacity(edges),
            directed,
        }
    }

    /// Returns `true` if edges are directed.
    pub fn is_directed(&self) -> bool {
        self.directed
    }

    /// Returns the number of nodes.
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Returns the number of edges.
    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    /// Returns `true` if the graph has no nodes.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Removes every node and edge, keeping the allocated capacity.
    ///
    /// All previously issued identifiers become invalid.
    pub fn clear(&mut self) {
        self.nodes.clear();
        self.edges.clear();
    }

    /// Adds a node and returns its identifier.
    pub fn add_node(&mut self, weight: N) -> NodeId {
        let id = self.nodes.len();
        self.nodes.push(Node {
            weight,
            first: [END; 2],
            last: [END; 2],
        });
        NodeId(id)
    }

    /// Adds an edge from `from` to `to` and returns its identifier.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidNode`] if either endpoint does not exist.
    pub fn add_edge(&mut self, from: NodeId, to: NodeId, weight: E) -> Result<EdgeId, Error> {
        let ends = [self.check(from)?, self.check(to)?];
        let id = self.edges.len();
        self.edges.push(Edge {
            weight,
            ends,
            next: [END; 2],
        });
        for (list, &node) in ends.iter().enumerate() {
            let tail = self.nodes[node].last[list];
            match self.edges.get_mut(tail) {
                Some(prev) => prev.next[list] = id,
                None => self.nodes[node].first[list] = id,
            }
            self.nodes[node].last[list] = id;
        }
        Ok(EdgeId(id))
    }

    /// Returns `true` if `node` exists.
    pub fn contains_node(&self, node: NodeId) -> bool {
        node.0 < self.nodes.len()
    }

    /// Returns the weight of `node`, or `None` if it does not exist.
    pub fn node(&self, node: NodeId) -> Option<&N> {
        self.nodes.get(node.0).map(|n| &n.weight)
    }

    /// Returns the weight of `node` mutably, or `None` if it does not exist.
    pub fn node_mut(&mut self, node: NodeId) -> Option<&mut N> {
        self.nodes.get_mut(node.0).map(|n| &mut n.weight)
    }

    /// Returns the weight of `edge`, or `None` if it does not exist.
    pub fn edge_weight(&self, edge: EdgeId) -> Option<&E> {
        self.edges.get(edge.0).map(|e| &e.weight)
    }

    /// Returns the weight of `edge` mutably, or `None` if it does not exist.
    pub fn edge_weight_mut(&mut self, edge: EdgeId) -> Option<&mut E> {
        self.edges.get_mut(edge.0).map(|e| &mut e.weight)
    }

    /// Returns the `(source, target)` endpoints of `edge`.
    pub fn edge_endpoints(&self, edge: EdgeId) -> Option<(NodeId, NodeId)> {
        self.edges
            .get(edge.0)
            .map(|e| (NodeId(e.ends[OUT]), NodeId(e.ends[IN])))
    }

    /// Returns the first edge leading from `from` to `to`.
    ///
    /// In an undirected graph the edge may have been added in either
    /// direction. Returns `None` if there is no such edge or a node is invalid.
    pub fn find_edge(&self, from: NodeId, to: NodeId) -> Option<EdgeId> {
        self.edges_of(from)
            .ok()?
            .find(|e| e.target == to)
            .map(|e| e.id)
    }

    /// Iterates over all node identifiers in index order.
    pub fn node_ids(&self) -> impl DoubleEndedIterator<Item = NodeId> + ExactSizeIterator {
        (0..self.nodes.len()).map(NodeId)
    }

    /// Iterates over all nodes and their weights in index order.
    pub fn nodes(&self) -> impl DoubleEndedIterator<Item = (NodeId, &N)> + ExactSizeIterator {
        self.nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (NodeId(i), &n.weight))
    }

    /// Iterates over all edges in index order, as stored (`source → target`).
    pub fn edges(&self) -> impl DoubleEndedIterator<Item = EdgeRef<'_, E>> + ExactSizeIterator {
        self.edges.iter().enumerate().map(|(i, e)| EdgeRef {
            id: EdgeId(i),
            source: NodeId(e.ends[OUT]),
            target: NodeId(e.ends[IN]),
            weight: &e.weight,
        })
    }

    /// Iterates over the edges traversable from `node` in insertion order.
    ///
    /// For directed graphs these are the outgoing edges; for undirected
    /// graphs all incident edges. Each [`EdgeRef`] is oriented away from
    /// `node`: its [`source`](EdgeRef::source) is `node`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidNode`] if `node` does not exist.
    pub fn edges_of(&self, node: NodeId) -> Result<Edges<'_, N, E>, Error> {
        let index = self.check(node)?;
        Ok(Edges {
            graph: self,
            node: index,
            cursor: self.cursor(index),
        })
    }

    /// Iterates over the nodes reachable from `node` by one edge.
    ///
    /// A neighbor appears once per connecting edge.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidNode`] if `node` does not exist.
    pub fn neighbors(&self, node: NodeId) -> Result<impl Iterator<Item = NodeId> + '_, Error> {
        Ok(self.edges_of(node)?.map(|e| e.target))
    }

    /// Returns the number of edges traversable from `node`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidNode`] if `node` does not exist.
    pub fn degree(&self, node: NodeId) -> Result<usize, Error> {
        Ok(self.edges_of(node)?.count())
    }

    /// Validates `node` and returns its raw index.
    fn check(&self, node: NodeId) -> Result<usize, Error> {
        if node.0 < self.nodes.len() {
            Ok(node.0)
        } else {
            Err(Error::InvalidNode {
                node,
                node_count: self.nodes.len(),
            })
        }
    }

    /// Returns a cursor at the start of `node`'s adjacency (valid `node`).
    fn cursor(&self, node: usize) -> Cursor {
        Cursor {
            list: OUT,
            edge: self.nodes.get(node).map_or(END, |n| n.first[OUT]),
        }
    }

    /// Advances `cursor` and returns the next `(edge, neighbor)` of `node`.
    fn advance(&self, node: usize, cursor: &mut Cursor) -> Option<(usize, usize)> {
        loop {
            let Some(edge) = self.edges.get(cursor.edge) else {
                if cursor.list == OUT && !self.directed {
                    cursor.list = IN;
                    cursor.edge = self.nodes.get(node).map_or(END, |n| n.first[IN]);
                    continue;
                }
                return None;
            };
            let id = cursor.edge;
            cursor.edge = edge.next[cursor.list];
            if cursor.list == IN && edge.ends[OUT] == edge.ends[IN] {
                // Undirected self-loop: already reported from the OUT list.
                continue;
            }
            return Some((id, edge.ends[1 - cursor.list]));
        }
    }
}

/// A borrowed view of one edge, oriented `source → target`.
#[derive(Debug)]
pub struct EdgeRef<'a, E> {
    id: EdgeId,
    source: NodeId,
    target: NodeId,
    weight: &'a E,
}

impl<E> Clone for EdgeRef<'_, E> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<E> Copy for EdgeRef<'_, E> {}

impl<'a, E> EdgeRef<'a, E> {
    /// The edge identifier.
    pub fn id(&self) -> EdgeId {
        self.id
    }

    /// The node the edge is traversed from.
    pub fn source(&self) -> NodeId {
        self.source
    }

    /// The node the edge is traversed to.
    pub fn target(&self) -> NodeId {
        self.target
    }

    /// The edge weight.
    pub fn weight(&self) -> &'a E {
        self.weight
    }
}

/// Iterator over the edges traversable from one node; see [`Graph::edges_of`].
#[derive(Debug)]
pub struct Edges<'a, N, E> {
    graph: &'a Graph<N, E>,
    node: usize,
    cursor: Cursor,
}

impl<'a, N, E> Iterator for Edges<'a, N, E> {
    type Item = EdgeRef<'a, E>;

    fn next(&mut self) -> Option<Self::Item> {
        let (edge, target) = self.graph.advance(self.node, &mut self.cursor)?;
        let weight = &self.graph.edges.get(edge)?.weight;
        Some(EdgeRef {
            id: EdgeId(edge),
            source: NodeId(self.node),
            target: NodeId(target),
            weight,
        })
    }
}
