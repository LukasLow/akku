//! Minimum spanning forests with Kruskal's algorithm.

use super::{EdgeId, Graph, IN, OUT};
use crate::algo::dsu::UnionFind;
use crate::algo::{Error, Number};

/// A minimum spanning tree (or forest, for disconnected graphs).
#[derive(Debug, Clone, PartialEq)]
pub struct SpanningTree<W> {
    /// The selected edges, in ascending weight order.
    pub edges: Vec<EdgeId>,
    /// Sum of the selected edge weights.
    pub total_weight: W,
}

impl<N, E: Number> Graph<N, E> {
    /// Computes a minimum spanning forest of an undirected graph (Kruskal).
    ///
    /// For a connected graph the result is a spanning tree with
    /// `node_count - 1` edges; otherwise one tree per connected component.
    /// Negative weights are allowed; self-loops are never selected. Ties are
    /// broken by edge index, so the result is deterministic. Runs in
    /// `O(E log E)`.
    ///
    /// # Errors
    ///
    /// - [`Error::RequiresUndirected`] for directed graphs.
    /// - [`Error::InvalidWeight`] if any weight is `NaN` or infinite.
    /// - [`Error::Overflow`] if the total weight overflows `E`.
    ///
    /// # Examples
    ///
    /// ```
    /// use akku::algo::graph::Graph;
    ///
    /// let mut g = Graph::<(), u32>::undirected();
    /// let n: Vec<_> = (0..4).map(|_| g.add_node(())).collect();
    /// g.add_edge(n[0], n[1], 1)?;
    /// g.add_edge(n[1], n[2], 2)?;
    /// g.add_edge(n[0], n[2], 3)?;
    /// g.add_edge(n[2], n[3], 4)?;
    ///
    /// let mst = g.minimum_spanning_tree()?;
    /// assert_eq!(mst.total_weight, 7);
    /// assert_eq!(mst.edges.len(), 3);
    /// # Ok::<(), akku::algo::Error>(())
    /// ```
    pub fn minimum_spanning_tree(&self) -> Result<SpanningTree<E>, Error> {
        if self.directed {
            return Err(Error::RequiresUndirected);
        }
        self.validate_weights(false)?;
        let mut order: Vec<usize> = (0..self.edges.len()).collect();
        order.sort_by(|&a, &b| self.edges[a].weight.total_cmp(&self.edges[b].weight));

        let mut sets = UnionFind::new(self.nodes.len());
        let mut edges = Vec::with_capacity(self.nodes.len().saturating_sub(1));
        let mut total_weight = E::ZERO;
        for index in order {
            let edge = &self.edges[index];
            if sets.union(edge.ends[OUT], edge.ends[IN])? {
                total_weight = total_weight
                    .checked_add(edge.weight)
                    .ok_or(Error::Overflow)?;
                edges.push(EdgeId(index));
                if sets.set_count() == 1 {
                    break;
                }
            }
        }
        Ok(SpanningTree {
            edges,
            total_weight,
        })
    }
}
