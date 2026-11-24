//! Unweighted traversals: BFS, DFS, topological sort, and Tarjan SCC.

use std::collections::VecDeque;

use super::{Cursor, Graph, NodeId};
use crate::algo::Error;

/// Marker for nodes Tarjan's algorithm has not visited yet.
const UNVISITED: usize = usize::MAX;

/// Lazy breadth-first traversal; see [`Graph::bfs`].
#[derive(Debug)]
pub struct Bfs<'a, N, E> {
    graph: &'a Graph<N, E>,
    visited: Vec<bool>,
    queue: VecDeque<usize>,
}

impl<N, E> Iterator for Bfs<'_, N, E> {
    type Item = NodeId;

    fn next(&mut self) -> Option<NodeId> {
        let node = self.queue.pop_front()?;
        let mut cursor = self.graph.cursor(node);
        while let Some((_, next)) = self.graph.advance(node, &mut cursor) {
            if let Some(seen) = self.visited.get_mut(next) {
                if !*seen {
                    *seen = true;
                    self.queue.push_back(next);
                }
            }
        }
        Some(NodeId(node))
    }
}

/// Lazy, iterative depth-first traversal in pre-order; see [`Graph::dfs`].
#[derive(Debug)]
pub struct Dfs<'a, N, E> {
    graph: &'a Graph<N, E>,
    visited: Vec<bool>,
    stack: Vec<(usize, Cursor)>,
    start: Option<usize>,
}

impl<N, E> Iterator for Dfs<'_, N, E> {
    type Item = NodeId;

    fn next(&mut self) -> Option<NodeId> {
        if let Some(start) = self.start.take() {
            return Some(NodeId(start));
        }
        while let Some((node, cursor)) = self.stack.last_mut() {
            match self.graph.advance(*node, cursor) {
                Some((_, next)) => {
                    if let Some(seen) = self.visited.get_mut(next) {
                        if !*seen {
                            *seen = true;
                            self.stack.push((next, self.graph.cursor(next)));
                            return Some(NodeId(next));
                        }
                    }
                }
                None => {
                    self.stack.pop();
                }
            }
        }
        None
    }
}

impl<N, E> Graph<N, E> {
    /// Breadth-first traversal from `start`, yielding nodes in visit order.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidNode`] if `start` does not exist.
    ///
    /// # Examples
    ///
    /// ```
    /// use akku::algo::graph::Graph;
    ///
    /// let mut g = Graph::<(), ()>::undirected();
    /// let n: Vec<_> = (0..4).map(|_| g.add_node(())).collect();
    /// g.add_edge(n[0], n[1], ())?;
    /// g.add_edge(n[0], n[2], ())?;
    /// g.add_edge(n[1], n[3], ())?;
    /// assert_eq!(g.bfs(n[0])?.collect::<Vec<_>>(), [n[0], n[1], n[2], n[3]]);
    /// # Ok::<(), akku::algo::Error>(())
    /// ```
    pub fn bfs(&self, start: NodeId) -> Result<Bfs<'_, N, E>, Error> {
        let start = self.check(start)?;
        let mut visited = vec![false; self.nodes.len()];
        visited[start] = true;
        Ok(Bfs {
            graph: self,
            visited,
            queue: VecDeque::from([start]),
        })
    }

    /// Depth-first traversal from `start`, yielding nodes in pre-order.
    ///
    /// Neighbors are explored in insertion order, matching a recursive DFS,
    /// but the traversal uses an explicit heap-allocated stack and cannot
    /// overflow the call stack on deep graphs.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidNode`] if `start` does not exist.
    ///
    /// # Examples
    ///
    /// ```
    /// use akku::algo::graph::Graph;
    ///
    /// let mut g = Graph::<(), ()>::directed();
    /// let n: Vec<_> = (0..4).map(|_| g.add_node(())).collect();
    /// g.add_edge(n[0], n[1], ())?;
    /// g.add_edge(n[0], n[2], ())?;
    /// g.add_edge(n[1], n[3], ())?;
    /// assert_eq!(g.dfs(n[0])?.collect::<Vec<_>>(), [n[0], n[1], n[3], n[2]]);
    /// # Ok::<(), akku::algo::Error>(())
    /// ```
    pub fn dfs(&self, start: NodeId) -> Result<Dfs<'_, N, E>, Error> {
        let start = self.check(start)?;
        let mut visited = vec![false; self.nodes.len()];
        visited[start] = true;
        Ok(Dfs {
            graph: self,
            visited,
            stack: vec![(start, self.cursor(start))],
            start: Some(start),
        })
    }

    /// Topologically sorts a directed graph with Kahn's algorithm.
    ///
    /// Among nodes that are ready at the same time, lower indices come first,
    /// so the order is deterministic. Runs in `O(V + E)`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::RequiresDirected`] for undirected graphs and
    /// [`Error::Cycle`] if the graph contains a cycle (including self-loops).
    ///
    /// # Examples
    ///
    /// ```
    /// use akku::algo::{graph::Graph, Error};
    ///
    /// let mut g = Graph::<(), ()>::directed();
    /// let a = g.add_node(());
    /// let b = g.add_node(());
    /// g.add_edge(b, a, ())?;
    /// assert_eq!(g.toposort()?, [b, a]);
    ///
    /// g.add_edge(a, b, ())?;
    /// assert_eq!(g.toposort(), Err(Error::Cycle));
    /// # Ok::<(), akku::algo::Error>(())
    /// ```
    pub fn toposort(&self) -> Result<Vec<NodeId>, Error> {
        if !self.directed {
            return Err(Error::RequiresDirected);
        }
        let mut in_degree = vec![0usize; self.nodes.len()];
        for edge in &self.edges {
            if let Some(degree) = in_degree.get_mut(edge.ends[1]) {
                *degree += 1;
            }
        }
        let mut queue: VecDeque<usize> = in_degree
            .iter()
            .enumerate()
            .filter(|(_, &d)| d == 0)
            .map(|(i, _)| i)
            .collect();
        let mut order = Vec::with_capacity(self.nodes.len());
        while let Some(node) = queue.pop_front() {
            order.push(NodeId(node));
            let mut cursor = self.cursor(node);
            while let Some((_, next)) = self.advance(node, &mut cursor) {
                if let Some(degree) = in_degree.get_mut(next) {
                    *degree = degree.saturating_sub(1);
                    if *degree == 0 {
                        queue.push_back(next);
                    }
                }
            }
        }
        if order.len() == self.nodes.len() {
            Ok(order)
        } else {
            Err(Error::Cycle)
        }
    }

    /// Computes strongly connected components with Tarjan's algorithm.
    ///
    /// Components are returned in reverse topological order of the
    /// condensation (a component appears before every component that can reach
    /// it). For undirected graphs the components are the connected
    /// components. The implementation is iterative, runs in `O(V + E)`, and
    /// is safe on arbitrarily deep graphs.
    ///
    /// # Examples
    ///
    /// ```
    /// use akku::algo::graph::Graph;
    ///
    /// let mut g = Graph::<(), ()>::directed();
    /// let n: Vec<_> = (0..3).map(|_| g.add_node(())).collect();
    /// g.add_edge(n[0], n[1], ())?;
    /// g.add_edge(n[1], n[0], ())?;
    /// g.add_edge(n[1], n[2], ())?;
    ///
    /// let mut sccs = g.strongly_connected_components();
    /// sccs.iter_mut().for_each(|c| c.sort());
    /// assert_eq!(sccs, [vec![n[2]], vec![n[0], n[1]]]);
    /// # Ok::<(), akku::algo::Error>(())
    /// ```
    pub fn strongly_connected_components(&self) -> Vec<Vec<NodeId>> {
        let n = self.nodes.len();
        let mut index = vec![UNVISITED; n];
        let mut low = vec![0usize; n];
        let mut on_stack = vec![false; n];
        let mut stack: Vec<usize> = Vec::new();
        let mut frames: Vec<(usize, Cursor)> = Vec::new();
        let mut components = Vec::new();
        let mut counter = 0usize;

        for root in 0..n {
            if index[root] != UNVISITED {
                continue;
            }
            index[root] = counter;
            low[root] = counter;
            counter += 1;
            stack.push(root);
            on_stack[root] = true;
            frames.push((root, self.cursor(root)));

            while let Some((node, cursor)) = frames.last_mut() {
                let node = *node;
                if let Some((_, next)) = self.advance(node, cursor) {
                    if index[next] == UNVISITED {
                        index[next] = counter;
                        low[next] = counter;
                        counter += 1;
                        stack.push(next);
                        on_stack[next] = true;
                        frames.push((next, self.cursor(next)));
                    } else if on_stack[next] {
                        low[node] = low[node].min(index[next]);
                    }
                    continue;
                }
                frames.pop();
                if let Some(&(parent, _)) = frames.last() {
                    low[parent] = low[parent].min(low[node]);
                }
                if low[node] == index[node] {
                    let mut component = Vec::new();
                    while let Some(member) = stack.pop() {
                        on_stack[member] = false;
                        component.push(NodeId(member));
                        if member == node {
                            break;
                        }
                    }
                    components.push(component);
                }
            }
        }
        components
    }
}
