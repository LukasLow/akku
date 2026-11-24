//! Weighted shortest paths: Dijkstra, A*, and Bellman-Ford.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use super::{EdgeId, Graph, NodeId, IN, OUT};
use crate::algo::{Error, Number};

/// A path between two nodes together with its total cost.
#[derive(Debug, Clone, PartialEq)]
pub struct Path<W> {
    /// Sum of the edge weights along the path.
    pub cost: W,
    /// The visited nodes, from the start to the goal (both inclusive).
    pub nodes: Vec<NodeId>,
}

/// Single-source shortest-path distances and a predecessor tree.
///
/// Returned by [`Graph::dijkstra`] and [`Graph::bellman_ford`].
#[derive(Debug, Clone, PartialEq)]
pub struct ShortestPaths<W> {
    source: NodeId,
    dist: Vec<Option<W>>,
    pred: Vec<Option<NodeId>>,
}

impl<W: Number> ShortestPaths<W> {
    /// The node the distances are measured from.
    pub fn source(&self) -> NodeId {
        self.source
    }

    /// Shortest distance to `node`, or `None` if it is unreachable or invalid.
    pub fn distance(&self, node: NodeId) -> Option<W> {
        self.dist.get(node.index()).copied().flatten()
    }

    /// Returns `true` if `node` is reachable from the source.
    pub fn is_reachable(&self, node: NodeId) -> bool {
        self.distance(node).is_some()
    }

    /// The node preceding `node` on a shortest path (`None` for the source,
    /// unreachable, or invalid nodes).
    pub fn predecessor(&self, node: NodeId) -> Option<NodeId> {
        self.pred.get(node.index()).copied().flatten()
    }

    /// Reconstructs a shortest path from the source to `target`.
    pub fn path_to(&self, target: NodeId) -> Option<Path<W>> {
        let cost = self.distance(target)?;
        Some(Path {
            cost,
            nodes: walk_back(&self.pred, target),
        })
    }
}

/// Follows predecessor links from `target`, bounded by the node count.
fn walk_back(pred: &[Option<NodeId>], target: NodeId) -> Vec<NodeId> {
    let mut nodes = vec![target];
    let mut current = target;
    while let Some(prev) = pred.get(current.index()).copied().flatten() {
        if nodes.len() > pred.len() {
            break; // Defensive: predecessor trees are acyclic by construction.
        }
        nodes.push(prev);
        current = prev;
    }
    nodes.reverse();
    nodes
}

/// Min-heap entry ordered by `priority`, then by node index.
#[derive(Debug, Clone, Copy)]
struct Entry<W> {
    priority: W,
    cost: W,
    node: usize,
}

impl<W: Number> Ord for Entry<W> {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reversed so `BinaryHeap` pops the smallest priority first.
        other
            .priority
            .total_cmp(&self.priority)
            .then_with(|| other.node.cmp(&self.node))
    }
}

impl<W: Number> PartialOrd for Entry<W> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<W: Number> PartialEq for Entry<W> {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl<W: Number> Eq for Entry<W> {}

fn is_less<W: Number>(candidate: W, current: Option<W>) -> bool {
    current.is_none_or(|current| candidate.total_cmp(&current) == Ordering::Less)
}

impl<N, E: Number> Graph<N, E> {
    /// Checks every edge weight; with `non_negative`, also rejects negatives.
    pub(super) fn validate_weights(&self, non_negative: bool) -> Result<(), Error> {
        for (index, edge) in self.edges.iter().enumerate() {
            if !edge.weight.is_valid() {
                return Err(Error::InvalidWeight {
                    edge: EdgeId(index),
                });
            }
            if non_negative && edge.weight.is_negative() {
                return Err(Error::NegativeWeight {
                    edge: EdgeId(index),
                });
            }
        }
        Ok(())
    }

    /// Single-source shortest paths with Dijkstra's algorithm.
    ///
    /// Runs in `O((V + E) log V)` using a binary heap.
    ///
    /// # Errors
    ///
    /// - [`Error::InvalidNode`] if `start` does not exist.
    /// - [`Error::NegativeWeight`] if any edge weight is negative (use
    ///   [`bellman_ford`](Self::bellman_ford) instead).
    /// - [`Error::InvalidWeight`] if any weight is `NaN` or infinite.
    /// - [`Error::Overflow`] if a path cost overflows `E`.
    ///
    /// # Examples
    ///
    /// ```
    /// use akku::algo::graph::Graph;
    ///
    /// let mut g = Graph::<(), u64>::undirected();
    /// let n: Vec<_> = (0..4).map(|_| g.add_node(())).collect();
    /// g.add_edge(n[0], n[1], 7)?;
    /// g.add_edge(n[0], n[2], 2)?;
    /// g.add_edge(n[2], n[1], 3)?;
    ///
    /// let paths = g.dijkstra(n[0])?;
    /// assert_eq!(paths.distance(n[1]), Some(5));
    /// assert_eq!(paths.distance(n[3]), None);
    /// assert_eq!(paths.path_to(n[1]).unwrap().nodes, [n[0], n[2], n[1]]);
    /// # Ok::<(), akku::algo::Error>(())
    /// ```
    pub fn dijkstra(&self, start: NodeId) -> Result<ShortestPaths<E>, Error> {
        let source = self.check(start)?;
        self.validate_weights(true)?;
        let n = self.nodes.len();
        let mut dist: Vec<Option<E>> = vec![None; n];
        let mut pred: Vec<Option<NodeId>> = vec![None; n];
        let mut heap = BinaryHeap::new();
        dist[source] = Some(E::ZERO);
        heap.push(Entry {
            priority: E::ZERO,
            cost: E::ZERO,
            node: source,
        });
        while let Some(Entry { cost, node, .. }) = heap.pop() {
            if is_less(dist[node].unwrap_or(cost), Some(cost)) {
                continue; // Stale entry.
            }
            let mut cursor = self.cursor(node);
            while let Some((edge, next)) = self.advance(node, &mut cursor) {
                let weight = self.edges[edge].weight;
                let candidate = cost.checked_add(weight).ok_or(Error::Overflow)?;
                if is_less(candidate, dist[next]) {
                    dist[next] = Some(candidate);
                    pred[next] = Some(NodeId(node));
                    heap.push(Entry {
                        priority: candidate,
                        cost: candidate,
                        node: next,
                    });
                }
            }
        }
        Ok(ShortestPaths {
            source: start,
            dist,
            pred,
        })
    }

    /// Shortest path from `start` to `goal` (Dijkstra with early exit).
    ///
    /// Returns `Ok(None)` if `goal` is unreachable.
    ///
    /// # Errors
    ///
    /// Same as [`dijkstra`](Self::dijkstra), plus [`Error::InvalidNode`] for
    /// an invalid `goal`.
    pub fn shortest_path(&self, start: NodeId, goal: NodeId) -> Result<Option<Path<E>>, Error> {
        self.astar(start, goal, |_| E::ZERO)
    }

    /// Shortest path from `start` to `goal` with the A* algorithm.
    ///
    /// `heuristic(node)` estimates the remaining cost from `node` to `goal`.
    /// It is called at most once per node. With an admissible heuristic
    /// (never overestimating) the result is optimal; nodes are re-expanded
    /// when a cheaper route is found, so inconsistent-but-admissible
    /// heuristics are handled correctly too.
    ///
    /// Returns `Ok(None)` if `goal` is unreachable.
    ///
    /// # Errors
    ///
    /// - [`Error::InvalidNode`] if `start` or `goal` does not exist.
    /// - [`Error::NegativeWeight`] / [`Error::InvalidWeight`] for negative,
    ///   `NaN`, or infinite edge weights.
    /// - [`Error::InvalidHeuristic`] if the heuristic returns a negative,
    ///   `NaN`, or infinite estimate.
    /// - [`Error::Overflow`] if a cost or estimate sum overflows `E`.
    ///
    /// # Examples
    ///
    /// ```
    /// use akku::algo::graph::{Graph, NodeId};
    ///
    /// // A 3x3 grid; node weights are (x, y) coordinates.
    /// let mut g = Graph::<(i32, i32), u32>::undirected();
    /// for y in 0..3 {
    ///     for x in 0..3 {
    ///         g.add_node((x, y));
    ///     }
    /// }
    /// for y in 0..3 {
    ///     for x in 0..3 {
    ///         let id = NodeId::new(y * 3 + x);
    ///         if x < 2 { g.add_edge(id, NodeId::new(y * 3 + x + 1), 1)?; }
    ///         if y < 2 { g.add_edge(id, NodeId::new((y + 1) * 3 + x), 1)?; }
    ///     }
    /// }
    /// let goal = NodeId::new(8);
    /// let manhattan = |n: NodeId| {
    ///     let (x, y) = g.node(n).copied().unwrap_or_default();
    ///     ((2 - x).abs() + (2 - y).abs()) as u32
    /// };
    /// let path = g.astar(NodeId::new(0), goal, manhattan)?.unwrap();
    /// assert_eq!(path.cost, 4);
    /// assert_eq!(path.nodes.len(), 5);
    /// # Ok::<(), akku::algo::Error>(())
    /// ```
    pub fn astar<H>(
        &self,
        start: NodeId,
        goal: NodeId,
        mut heuristic: H,
    ) -> Result<Option<Path<E>>, Error>
    where
        H: FnMut(NodeId) -> E,
    {
        let source = self.check(start)?;
        let target = self.check(goal)?;
        self.validate_weights(true)?;
        let n = self.nodes.len();
        let mut best: Vec<Option<E>> = vec![None; n];
        let mut estimate: Vec<Option<E>> = vec![None; n];
        let mut pred: Vec<Option<NodeId>> = vec![None; n];
        let mut estimate_of = |node: usize, cache: &mut Vec<Option<E>>| -> Result<E, Error> {
            if let Some(h) = cache[node] {
                return Ok(h);
            }
            let h = heuristic(NodeId(node));
            if !h.is_valid() || h.is_negative() {
                return Err(Error::InvalidHeuristic { node: NodeId(node) });
            }
            cache[node] = Some(h);
            Ok(h)
        };

        let mut heap = BinaryHeap::new();
        best[source] = Some(E::ZERO);
        heap.push(Entry {
            priority: estimate_of(source, &mut estimate)?,
            cost: E::ZERO,
            node: source,
        });
        while let Some(Entry { cost, node, .. }) = heap.pop() {
            if is_less(best[node].unwrap_or(cost), Some(cost)) {
                continue; // Stale entry.
            }
            if node == target {
                return Ok(Some(Path {
                    cost,
                    nodes: walk_back(&pred, goal),
                }));
            }
            let mut cursor = self.cursor(node);
            while let Some((edge, next)) = self.advance(node, &mut cursor) {
                let weight = self.edges[edge].weight;
                let candidate = cost.checked_add(weight).ok_or(Error::Overflow)?;
                if is_less(candidate, best[next]) {
                    let priority = candidate
                        .checked_add(estimate_of(next, &mut estimate)?)
                        .ok_or(Error::Overflow)?;
                    best[next] = Some(candidate);
                    pred[next] = Some(NodeId(node));
                    heap.push(Entry {
                        priority,
                        cost: candidate,
                        node: next,
                    });
                }
            }
        }
        Ok(None)
    }

    /// Single-source shortest paths with the Bellman-Ford algorithm.
    ///
    /// Supports negative edge weights. Runs in `O(V · E)` with early exit
    /// once distances stabilize. In an undirected graph a negative edge is
    /// itself a negative cycle (it can be walked back and forth).
    ///
    /// # Errors
    ///
    /// - [`Error::InvalidNode`] if `start` does not exist.
    /// - [`Error::InvalidWeight`] if any weight is `NaN` or infinite.
    /// - [`Error::NegativeCycle`] if a negative cycle is reachable from
    ///   `start`.
    /// - [`Error::Overflow`] if a path cost overflows `E`.
    ///
    /// # Examples
    ///
    /// ```
    /// use akku::algo::{graph::Graph, Error};
    ///
    /// let mut g = Graph::<(), i32>::directed();
    /// let n: Vec<_> = (0..3).map(|_| g.add_node(())).collect();
    /// g.add_edge(n[0], n[1], 4)?;
    /// g.add_edge(n[1], n[2], -3)?;
    /// g.add_edge(n[0], n[2], 2)?;
    /// assert_eq!(g.bellman_ford(n[0])?.distance(n[2]), Some(1));
    ///
    /// g.add_edge(n[2], n[1], 1)?;
    /// assert_eq!(g.bellman_ford(n[0]), Err(Error::NegativeCycle));
    /// # Ok::<(), akku::algo::Error>(())
    /// ```
    pub fn bellman_ford(&self, start: NodeId) -> Result<ShortestPaths<E>, Error> {
        let source = self.check(start)?;
        self.validate_weights(false)?;
        let n = self.nodes.len();
        let mut dist: Vec<Option<E>> = vec![None; n];
        let mut pred: Vec<Option<NodeId>> = vec![None; n];
        dist[source] = Some(E::ZERO);

        let directions: &[(usize, usize)] = if self.directed {
            &[(OUT, IN)]
        } else {
            &[(OUT, IN), (IN, OUT)]
        };
        // Without negative cycles distances settle within V - 1 rounds, so a
        // change in round V proves a reachable negative cycle.
        for _ in 0..n {
            let mut changed = false;
            for edge in &self.edges {
                for &(from, to) in directions {
                    let (u, v) = (edge.ends[from], edge.ends[to]);
                    let Some(du) = dist[u] else { continue };
                    let candidate = du.checked_add(edge.weight).ok_or(Error::Overflow)?;
                    if is_less(candidate, dist[v]) {
                        dist[v] = Some(candidate);
                        pred[v] = Some(NodeId(u));
                        changed = true;
                    }
                }
            }
            if !changed {
                return Ok(ShortestPaths {
                    source: start,
                    dist,
                    pred,
                });
            }
        }
        Err(Error::NegativeCycle)
    }
}
