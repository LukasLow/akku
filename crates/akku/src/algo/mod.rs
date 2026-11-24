//! Zero-dependency graph, shortest-path, union-find, range-query, and string-search algorithms.
//!
//! Every algorithm in this module is iterative (no recursion, so deep inputs
//! such as 50 000-node chains cannot overflow the call stack), bounds-checked
//! (invalid indices return [`Error`] instead of panicking), and
//! overflow-checked (weights and sums use [`Number::checked_add`], and
//! floating-point `NaN`/`±∞` values are rejected rather than silently
//! propagated).
//!
//! | Module | Contents |
//! | --- | --- |
//! | [`graph`] | Arena-backed [`Graph`](graph::Graph): BFS, DFS, topological sort, Tarjan SCC, Dijkstra, A*, Bellman-Ford, Kruskal MST |
//! | [`dsu`] | [`UnionFind`](dsu::UnionFind) with path halving and union by size |
//! | [`tree`] | [`FenwickTree`](tree::FenwickTree) and monoid [`SegmentTree`](tree::SegmentTree) |
//! | [`search`] | KMP, multi-pattern [`AhoCorasick`](search::AhoCorasick), `lower_bound`/`upper_bound` |
//!
//! # Examples
//!
//! ```
//! use akku::algo::graph::Graph;
//!
//! let mut g = Graph::<&str, u32>::directed();
//! let a = g.add_node("a");
//! let b = g.add_node("b");
//! let c = g.add_node("c");
//! g.add_edge(a, b, 4)?;
//! g.add_edge(b, c, 1)?;
//! g.add_edge(a, c, 9)?;
//!
//! let path = g.shortest_path(a, c)?.expect("c is reachable");
//! assert_eq!(path.cost, 5);
//! assert_eq!(path.nodes, [a, b, c]);
//! # Ok::<(), akku::algo::Error>(())
//! ```

use std::fmt;
use std::ops::{Bound, RangeBounds};

mod num;

pub mod dsu;
pub mod graph;
pub mod search;
pub mod tree;

pub use num::Number;

use graph::{EdgeId, NodeId};

/// Errors reported by the algorithms in [`akku::algo`](self).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// A [`NodeId`] does not refer to a node of the graph.
    InvalidNode {
        /// The offending node.
        node: NodeId,
        /// Number of nodes in the graph.
        node_count: usize,
    },
    /// A plain element index is outside `0..len`.
    IndexOutOfBounds {
        /// The offending index.
        index: usize,
        /// Number of elements in the structure.
        len: usize,
    },
    /// A range is reversed or extends past the end of the structure.
    InvalidRange {
        /// Resolved inclusive start of the range.
        start: usize,
        /// Resolved exclusive end of the range.
        end: usize,
        /// Number of elements in the structure.
        len: usize,
    },
    /// An algorithm that requires non-negative weights found a negative edge.
    NegativeWeight {
        /// The negative edge.
        edge: EdgeId,
    },
    /// An edge weight is not a valid number (`NaN` or infinite).
    InvalidWeight {
        /// The invalid edge.
        edge: EdgeId,
    },
    /// A value is not a valid number (`NaN` or infinite).
    InvalidValue,
    /// An A* heuristic returned a negative, `NaN`, or infinite estimate.
    InvalidHeuristic {
        /// The node whose estimate was rejected.
        node: NodeId,
    },
    /// Bellman-Ford found a negative cycle reachable from the source.
    NegativeCycle,
    /// A topological sort found a cycle.
    Cycle,
    /// An arithmetic operation overflowed (or produced a non-finite float).
    Overflow,
    /// The algorithm is only defined for directed graphs.
    RequiresDirected,
    /// The algorithm is only defined for undirected graphs.
    RequiresUndirected,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidNode { node, node_count } => write!(
                f,
                "invalid node index {} (graph has {node_count} nodes)",
                node.index()
            ),
            Self::IndexOutOfBounds { index, len } => {
                write!(f, "index {index} out of bounds for length {len}")
            }
            Self::InvalidRange { start, end, len } => {
                write!(f, "invalid range {start}..{end} for length {len}")
            }
            Self::NegativeWeight { edge } => {
                write!(f, "edge {} has a negative weight", edge.index())
            }
            Self::InvalidWeight { edge } => {
                write!(f, "edge {} has a NaN or infinite weight", edge.index())
            }
            Self::InvalidValue => f.write_str("value is NaN or infinite"),
            Self::InvalidHeuristic { node } => write!(
                f,
                "heuristic for node {} is negative, NaN, or infinite",
                node.index()
            ),
            Self::NegativeCycle => f.write_str("graph contains a negative cycle"),
            Self::Cycle => f.write_str("graph contains a cycle"),
            Self::Overflow => f.write_str("arithmetic overflow"),
            Self::RequiresDirected => f.write_str("algorithm requires a directed graph"),
            Self::RequiresUndirected => f.write_str("algorithm requires an undirected graph"),
        }
    }
}

impl std::error::Error for Error {}

/// Resolves `range` against a structure of `len` elements into `start..end`.
pub(crate) fn resolve_range<R: RangeBounds<usize>>(
    range: &R,
    len: usize,
) -> Result<(usize, usize), Error> {
    let start = match range.start_bound() {
        Bound::Included(&s) => Some(s),
        Bound::Excluded(&s) => s.checked_add(1),
        Bound::Unbounded => Some(0),
    };
    let end = match range.end_bound() {
        Bound::Included(&e) => e.checked_add(1),
        Bound::Excluded(&e) => Some(e),
        Bound::Unbounded => Some(len),
    };
    match (start, end) {
        (Some(start), Some(end)) if start <= end && end <= len => Ok((start, end)),
        (start, end) => Err(Error::InvalidRange {
            start: start.unwrap_or(usize::MAX),
            end: end.unwrap_or(usize::MAX),
            len,
        }),
    }
}
