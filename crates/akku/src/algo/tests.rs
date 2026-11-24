//! Integration tests for `akku::algo`.

use akku::algo::dsu::UnionFind;
use akku::algo::graph::{EdgeId, Graph, NodeId};
use akku::algo::search::{
    equal_range, kmp_find, kmp_find_all, lower_bound, lower_bound_by, upper_bound, upper_bound_by,
    AhoCorasick, Match,
};
use akku::algo::tree::{FenwickTree, SegmentTree};
use akku::algo::{Error, Number};

type Result<T = ()> = std::result::Result<T, Error>;

const DEEP: usize = 50_000;

fn nodes<N: Default, E>(g: &mut Graph<N, E>, count: usize) -> Vec<NodeId> {
    (0..count).map(|_| g.add_node(N::default())).collect()
}

fn chain(directed: bool, len: usize) -> Result<(Graph<(), u64>, Vec<NodeId>)> {
    let mut g = Graph::with_capacity(directed, len, len);
    let n = nodes(&mut g, len);
    for w in n.windows(2) {
        g.add_edge(w[0], w[1], 1)?;
    }
    Ok((g, n))
}

// ---------------------------------------------------------------- graph core

#[test]
fn graph_basic_accessors() -> Result {
    let mut g = Graph::<&str, i32>::directed();
    assert!(g.is_empty() && g.is_directed());
    let a = g.add_node("a");
    let b = g.add_node("b");
    let e = g.add_edge(a, b, 7)?;
    assert_eq!((g.node_count(), g.edge_count()), (2, 1));
    assert_eq!(g.node(b), Some(&"b"));
    assert_eq!(g.edge_weight(e), Some(&7));
    assert_eq!(g.edge_endpoints(e), Some((a, b)));
    *g.node_mut(a).unwrap() = "A";
    *g.edge_weight_mut(e).unwrap() = 9;
    assert_eq!(g.node(a), Some(&"A"));
    assert_eq!(g.edges().map(|r| *r.weight()).collect::<Vec<_>>(), [9]);
    assert_eq!(g.find_edge(a, b), Some(e));
    assert_eq!(g.find_edge(b, a), None, "directed edges are one-way");
    assert_eq!(g.nodes().map(|(_, w)| *w).collect::<Vec<_>>(), ["A", "b"]);
    g.clear();
    assert!(g.is_empty() && g.edge_count() == 0);
    Ok(())
}

#[test]
fn invalid_ids_are_errors_not_panics() {
    let mut g = Graph::<(), u32>::directed();
    let a = g.add_node(());
    let ghost = NodeId::new(42);
    let expected = Err(Error::InvalidNode {
        node: ghost,
        node_count: 1,
    });
    assert_eq!(g.add_edge(a, ghost, 1), expected.map(|()| EdgeId::new(0)));
    assert!(!g.contains_node(ghost));
    assert!(g.node(ghost).is_none());
    assert!(g.edge_weight(EdgeId::new(3)).is_none());
    assert!(g.edge_endpoints(EdgeId::new(3)).is_none());
    assert!(g.edges_of(ghost).is_err());
    assert!(g.neighbors(ghost).is_err());
    assert!(g.degree(ghost).is_err());
    assert!(g.bfs(ghost).is_err());
    assert!(g.dfs(ghost).is_err());
    assert!(g.dijkstra(ghost).is_err());
    assert!(g.bellman_ford(ghost).is_err());
    assert!(g.astar(a, ghost, |_| 0).is_err());
    assert!(g.shortest_path(ghost, a).is_err());
    assert_eq!(g.find_edge(a, ghost), None);
}

#[test]
fn undirected_neighbors_and_self_loops() -> Result {
    let mut g = Graph::<(), ()>::undirected();
    let n = nodes(&mut g, 3);
    g.add_edge(n[0], n[1], ())?;
    g.add_edge(n[2], n[0], ())?;
    g.add_edge(n[0], n[0], ())?;
    assert_eq!(g.neighbors(n[0])?.collect::<Vec<_>>(), [n[1], n[0], n[2]]);
    assert_eq!(g.neighbors(n[1])?.collect::<Vec<_>>(), [n[0]]);
    assert_eq!(g.degree(n[0])?, 3, "self-loop is reported once");
    assert!(g.find_edge(n[0], n[2]).is_some(), "reverse direction works");
    for edge in g.edges_of(n[2])? {
        assert_eq!(edge.source(), n[2]);
        assert_eq!(edge.target(), n[0]);
    }
    Ok(())
}

#[test]
fn parallel_edges_are_kept() -> Result {
    let mut g = Graph::<(), u32>::directed();
    let n = nodes(&mut g, 2);
    g.add_edge(n[0], n[1], 5)?;
    g.add_edge(n[0], n[1], 2)?;
    assert_eq!(g.degree(n[0])?, 2);
    assert_eq!(g.dijkstra(n[0])?.distance(n[1]), Some(2));
    Ok(())
}

// ------------------------------------------------------------- traversal

#[test]
fn bfs_order_and_disconnected() -> Result {
    let mut g = Graph::<(), ()>::directed();
    let n = nodes(&mut g, 6);
    g.add_edge(n[0], n[1], ())?;
    g.add_edge(n[0], n[2], ())?;
    g.add_edge(n[1], n[3], ())?;
    g.add_edge(n[2], n[3], ())?;
    g.add_edge(n[4], n[5], ())?;
    assert_eq!(g.bfs(n[0])?.collect::<Vec<_>>(), [n[0], n[1], n[2], n[3]]);
    assert_eq!(g.bfs(n[5])?.collect::<Vec<_>>(), [n[5]]);
    Ok(())
}

#[test]
fn dfs_preorder_matches_recursive_order() -> Result {
    let mut g = Graph::<(), ()>::directed();
    let n = nodes(&mut g, 5);
    g.add_edge(n[0], n[1], ())?;
    g.add_edge(n[0], n[4], ())?;
    g.add_edge(n[1], n[2], ())?;
    g.add_edge(n[2], n[0], ())?; // back edge
    g.add_edge(n[1], n[3], ())?;
    assert_eq!(
        g.dfs(n[0])?.collect::<Vec<_>>(),
        [n[0], n[1], n[2], n[3], n[4]]
    );
    Ok(())
}

#[test]
fn traversal_on_single_node_with_self_loop() -> Result {
    let mut g = Graph::<(), ()>::directed();
    let a = g.add_node(());
    g.add_edge(a, a, ())?;
    assert_eq!(g.bfs(a)?.collect::<Vec<_>>(), [a]);
    assert_eq!(g.dfs(a)?.collect::<Vec<_>>(), [a]);
    Ok(())
}

#[test]
fn deep_chain_is_stack_safe() -> Result {
    let (g, n) = chain(true, DEEP)?;
    assert_eq!(g.dfs(n[0])?.count(), DEEP);
    assert_eq!(g.bfs(n[0])?.count(), DEEP);
    assert_eq!(g.toposort()?, n);
    let sccs = g.strongly_connected_components();
    assert_eq!(sccs.len(), DEEP);
    assert_eq!(sccs[0], [n[DEEP - 1]], "sink component comes first");
    let last = n[DEEP - 1];
    assert_eq!(g.dijkstra(n[0])?.distance(last), Some(DEEP as u64 - 1));
    let path = g.shortest_path(n[0], last)?.unwrap();
    assert_eq!(path.nodes.len(), DEEP);
    assert_eq!(g.bellman_ford(n[0])?.distance(last), Some(DEEP as u64 - 1));
    Ok(())
}

#[test]
fn deep_cycle_is_one_scc() -> Result {
    let (mut g, n) = chain(true, DEEP)?;
    g.add_edge(n[DEEP - 1], n[0], 1)?;
    let sccs = g.strongly_connected_components();
    assert_eq!(sccs.len(), 1);
    assert_eq!(sccs[0].len(), DEEP);
    assert_eq!(g.toposort(), Err(Error::Cycle));
    Ok(())
}

#[test]
fn deep_undirected_chain_mst() -> Result {
    let (g, _) = chain(false, DEEP)?;
    let mst = g.minimum_spanning_tree()?;
    assert_eq!(mst.edges.len(), DEEP - 1);
    assert_eq!(mst.total_weight, DEEP as u64 - 1);
    Ok(())
}

#[test]
fn toposort_dag_respects_edges() -> Result {
    let mut g = Graph::<(), ()>::directed();
    let n = nodes(&mut g, 6);
    let deps = [(5, 2), (5, 0), (4, 0), (4, 1), (2, 3), (3, 1)];
    for (a, b) in deps {
        g.add_edge(n[a], n[b], ())?;
    }
    let order = g.toposort()?;
    let pos = |x: NodeId| order.iter().position(|&o| o == x).unwrap();
    for (a, b) in deps {
        assert!(pos(n[a]) < pos(n[b]));
    }
    assert_eq!(order.len(), 6);
    Ok(())
}

#[test]
fn toposort_errors() -> Result {
    let mut g = Graph::<(), ()>::directed();
    let a = g.add_node(());
    g.add_edge(a, a, ())?;
    assert_eq!(g.toposort(), Err(Error::Cycle), "self-loop is a cycle");
    let u = Graph::<(), ()>::undirected();
    assert_eq!(u.toposort(), Err(Error::RequiresDirected));
    assert_eq!(Graph::<(), ()>::directed().toposort(), Ok(vec![]));
    Ok(())
}

#[test]
fn tarjan_scc_classic() -> Result {
    let mut g = Graph::<(), ()>::directed();
    let n = nodes(&mut g, 8);
    for (a, b) in [
        (0, 1),
        (1, 2),
        (2, 0),
        (2, 3),
        (3, 4),
        (4, 5),
        (5, 3),
        (6, 5),
        (6, 7),
        (7, 6),
    ] {
        g.add_edge(n[a], n[b], ())?;
    }
    let mut sccs: Vec<Vec<usize>> = g
        .strongly_connected_components()
        .into_iter()
        .map(|c| {
            let mut c: Vec<usize> = c.into_iter().map(NodeId::index).collect();
            c.sort();
            c
        })
        .collect();
    // Reverse topological order: {3,4,5} is a sink of both other SCCs.
    assert_eq!(sccs[0], [3, 4, 5]);
    sccs.sort();
    assert_eq!(sccs, [vec![0, 1, 2], vec![3, 4, 5], vec![6, 7]]);
    Ok(())
}

#[test]
fn scc_on_undirected_graph_gives_components() -> Result {
    let mut g = Graph::<(), ()>::undirected();
    let n = nodes(&mut g, 5);
    g.add_edge(n[0], n[1], ())?;
    g.add_edge(n[1], n[2], ())?;
    g.add_edge(n[3], n[4], ())?;
    let mut sizes: Vec<usize> = g
        .strongly_connected_components()
        .iter()
        .map(Vec::len)
        .collect();
    sizes.sort();
    assert_eq!(sizes, [2, 3]);
    assert!(Graph::<(), ()>::directed()
        .strongly_connected_components()
        .is_empty());
    Ok(())
}

// ---------------------------------------------------------- shortest paths

fn sample_weighted(directed: bool) -> Result<(Graph<(), u32>, Vec<NodeId>)> {
    let mut g = Graph::with_capacity(directed, 6, 9);
    let n = nodes(&mut g, 6);
    for (a, b, w) in [
        (0, 1, 7),
        (0, 2, 9),
        (0, 5, 14),
        (1, 2, 10),
        (1, 3, 15),
        (2, 3, 11),
        (2, 5, 2),
        (3, 4, 6),
        (4, 5, 9),
    ] {
        g.add_edge(n[a], n[b], w)?;
    }
    Ok((g, n))
}

#[test]
fn dijkstra_classic_undirected() -> Result {
    let (g, n) = sample_weighted(false)?;
    let sp = g.dijkstra(n[0])?;
    let dist: Vec<_> = n.iter().map(|&x| sp.distance(x)).collect();
    assert_eq!(
        dist,
        [Some(0), Some(7), Some(9), Some(20), Some(20), Some(11)]
    );
    assert_eq!(sp.source(), n[0]);
    assert_eq!(sp.predecessor(n[0]), None);
    assert_eq!(sp.path_to(n[4]).unwrap().nodes, [n[0], n[2], n[5], n[4]]);
    Ok(())
}

#[test]
fn dijkstra_unreachable_nodes() -> Result {
    let (g, n) = sample_weighted(true)?;
    let sp = g.dijkstra(n[3])?;
    assert_eq!(sp.distance(n[0]), None);
    assert!(!sp.is_reachable(n[1]));
    assert!(sp.path_to(n[0]).is_none());
    assert_eq!(sp.distance(n[5]), Some(15));
    assert_eq!(sp.distance(NodeId::new(99)), None);
    assert_eq!(g.shortest_path(n[3], n[0])?, None);
    Ok(())
}

#[test]
fn dijkstra_rejects_negative_weights() -> Result {
    let mut g = Graph::<(), i32>::directed();
    let n = nodes(&mut g, 3);
    g.add_edge(n[0], n[1], 4)?;
    let bad = g.add_edge(n[1], n[2], -1)?;
    assert_eq!(g.dijkstra(n[0]), Err(Error::NegativeWeight { edge: bad }));
    assert_eq!(
        g.shortest_path(n[0], n[2]),
        Err(Error::NegativeWeight { edge: bad })
    );
    assert_eq!(g.bellman_ford(n[0])?.distance(n[2]), Some(3));
    Ok(())
}

#[test]
fn nan_and_infinite_weights_are_rejected() -> Result {
    let mut g = Graph::<(), f64>::undirected();
    let n = nodes(&mut g, 3);
    g.add_edge(n[0], n[1], 1.0)?;
    let nan = g.add_edge(n[1], n[2], f64::NAN)?;
    let err = Error::InvalidWeight { edge: nan };
    assert_eq!(g.dijkstra(n[0]), Err(err));
    assert_eq!(g.bellman_ford(n[0]), Err(err));
    assert_eq!(g.minimum_spanning_tree(), Err(err));
    *g.edge_weight_mut(nan).unwrap() = f64::INFINITY;
    assert_eq!(g.dijkstra(n[0]), Err(Error::InvalidWeight { edge: nan }));
    *g.edge_weight_mut(nan).unwrap() = 0.5;
    assert_eq!(g.dijkstra(n[0])?.distance(n[2]), Some(1.5));
    Ok(())
}

#[test]
fn dijkstra_overflow_is_reported() -> Result {
    let mut g = Graph::<(), u8>::directed();
    let n = nodes(&mut g, 3);
    g.add_edge(n[0], n[1], 200)?;
    g.add_edge(n[1], n[2], 100)?;
    assert_eq!(g.dijkstra(n[0]), Err(Error::Overflow));
    let mut f = Graph::<(), f64>::directed();
    let m = nodes(&mut f, 3);
    f.add_edge(m[0], m[1], f64::MAX)?;
    f.add_edge(m[1], m[2], f64::MAX)?;
    assert_eq!(f.dijkstra(m[0]), Err(Error::Overflow));
    Ok(())
}

#[test]
fn float_weights_shortest_path() -> Result {
    let mut g = Graph::<(), f32>::directed();
    let n = nodes(&mut g, 3);
    g.add_edge(n[0], n[2], 3.0)?;
    g.add_edge(n[0], n[1], 0.5)?;
    g.add_edge(n[1], n[2], 0.75)?;
    let p = g.shortest_path(n[0], n[2])?.unwrap();
    assert_eq!(p.cost, 1.25);
    assert_eq!(p.nodes, [n[0], n[1], n[2]]);
    Ok(())
}

#[test]
fn astar_grid_matches_dijkstra() -> Result {
    const W: usize = 20;
    let mut g = Graph::<(i64, i64), u32>::undirected();
    for y in 0..W {
        for x in 0..W {
            g.add_node((x as i64, y as i64));
        }
    }
    let id = |x: usize, y: usize| NodeId::new(y * W + x);
    for y in 0..W {
        for x in 0..W {
            // A wall at x == 10 except the bottom row.
            let blocked = |x: usize, y: usize| x == 10 && y != W - 1;
            if blocked(x, y) {
                continue;
            }
            if x + 1 < W && !blocked(x + 1, y) {
                g.add_edge(id(x, y), id(x + 1, y), 1)?;
            }
            if y + 1 < W && !blocked(x, y + 1) {
                g.add_edge(id(x, y), id(x, y + 1), 1)?;
            }
        }
    }
    let (start, goal) = (id(0, 0), id(W - 1, 0));
    let (gx, gy) = (W as i64 - 1, 0i64);
    let manhattan = |n: NodeId| {
        let (x, y) = *g.node(n).unwrap();
        ((gx - x).abs() + (gy - y).abs()) as u32
    };
    let path = g.astar(start, goal, manhattan)?.unwrap();
    let reference = g.dijkstra(start)?.distance(goal).unwrap();
    assert_eq!(path.cost, reference);
    assert_eq!(path.cost, 19 + 2 * (W as u32 - 1));
    assert_eq!(path.nodes.first(), Some(&start));
    assert_eq!(path.nodes.last(), Some(&goal));
    assert_eq!(path.nodes.len() as u32, path.cost + 1);
    Ok(())
}

#[test]
fn astar_rejects_nan_and_negative_heuristics() -> Result {
    let mut g = Graph::<(), f64>::directed();
    let n = nodes(&mut g, 3);
    g.add_edge(n[0], n[1], 1.0)?;
    g.add_edge(n[1], n[2], 1.0)?;
    assert_eq!(
        g.astar(n[0], n[2], |x| if x == n[1] { f64::NAN } else { 0.0 }),
        Err(Error::InvalidHeuristic { node: n[1] })
    );
    assert_eq!(
        g.astar(n[0], n[2], |_| -1.0),
        Err(Error::InvalidHeuristic { node: n[0] })
    );
    assert!(g.astar(n[0], n[2], |_| f64::INFINITY).is_err());
    Ok(())
}

#[test]
fn astar_start_equals_goal_and_unreachable() -> Result {
    let mut g = Graph::<(), u32>::directed();
    let n = nodes(&mut g, 2);
    let p = g.astar(n[0], n[0], |_| 0)?.unwrap();
    assert_eq!((p.cost, p.nodes), (0, vec![n[0]]));
    assert_eq!(g.astar(n[0], n[1], |_| 0)?, None);
    Ok(())
}

#[test]
fn astar_heuristic_called_at_most_once_per_node() -> Result {
    let (g, n) = sample_weighted(false)?;
    let mut calls = vec![0usize; n.len()];
    let path = g
        .astar(n[0], n[4], |x| {
            calls[x.index()] += 1;
            0
        })?
        .unwrap();
    assert_eq!(path.cost, 20);
    assert!(calls.iter().all(|&c| c <= 1));
    Ok(())
}

#[test]
fn bellman_ford_negative_edges_and_paths() -> Result {
    let mut g = Graph::<(), i64>::directed();
    let n = nodes(&mut g, 5);
    for (a, b, w) in [
        (0, 1, 6),
        (0, 3, 7),
        (1, 2, 5),
        (1, 3, 8),
        (1, 4, -4),
        (2, 1, -2),
        (3, 2, -3),
        (3, 4, 9),
        (4, 0, 2),
        (4, 2, 7),
    ] {
        g.add_edge(n[a], n[b], w)?;
    }
    let sp = g.bellman_ford(n[0])?;
    let dist: Vec<_> = n.iter().map(|&x| sp.distance(x).unwrap()).collect();
    assert_eq!(dist, [0, 2, 4, 7, -2]);
    assert_eq!(
        sp.path_to(n[4]).unwrap().nodes,
        [n[0], n[3], n[2], n[1], n[4]]
    );
    Ok(())
}

#[test]
fn bellman_ford_detects_negative_cycles() -> Result {
    let mut g = Graph::<(), i32>::directed();
    let n = nodes(&mut g, 4);
    g.add_edge(n[0], n[1], 1)?;
    g.add_edge(n[1], n[2], -1)?;
    g.add_edge(n[2], n[1], -1)?;
    assert_eq!(g.bellman_ford(n[0]), Err(Error::NegativeCycle));
    // Unreachable from n[3]: the cycle does not matter.
    assert_eq!(g.bellman_ford(n[3])?.distance(n[0]), None);

    let mut s = Graph::<(), i32>::directed();
    let a = s.add_node(());
    s.add_edge(a, a, -1)?;
    assert_eq!(s.bellman_ford(a), Err(Error::NegativeCycle), "self-loop");

    let mut u = Graph::<(), i32>::undirected();
    let m = nodes(&mut u, 2);
    u.add_edge(m[0], m[1], -2)?;
    assert_eq!(
        u.bellman_ford(m[0]),
        Err(Error::NegativeCycle),
        "undirected negative edge"
    );
    Ok(())
}

#[test]
fn bellman_ford_matches_dijkstra_on_non_negative() -> Result {
    let (g, n) = sample_weighted(false)?;
    let d = g.dijkstra(n[0])?;
    let b = g.bellman_ford(n[0])?;
    for &x in &n {
        assert_eq!(d.distance(x), b.distance(x));
    }
    Ok(())
}

// --------------------------------------------------------------------- MST

#[test]
fn kruskal_classic_and_forest() -> Result {
    let (g, n) = sample_weighted(false)?;
    let mst = g.minimum_spanning_tree()?;
    assert_eq!(mst.edges.len(), n.len() - 1);
    assert_eq!(mst.total_weight, 7 + 9 + 2 + 6 + 9);

    let mut f = Graph::<(), i32>::undirected();
    let m = nodes(&mut f, 5);
    f.add_edge(m[0], m[1], -3)?;
    f.add_edge(m[1], m[0], 1)?;
    f.add_edge(m[2], m[2], -100)?; // self-loop never selected
    f.add_edge(m[3], m[4], 2)?;
    let forest = f.minimum_spanning_tree()?;
    assert_eq!(forest.edges, [EdgeId::new(0), EdgeId::new(3)]);
    assert_eq!(forest.total_weight, -1);
    Ok(())
}

#[test]
fn kruskal_requires_undirected_and_checks_overflow() -> Result {
    assert_eq!(
        Graph::<(), u32>::directed().minimum_spanning_tree(),
        Err(Error::RequiresUndirected)
    );
    let empty = Graph::<(), u32>::undirected().minimum_spanning_tree()?;
    assert!(empty.edges.is_empty() && empty.total_weight == 0);
    let mut g = Graph::<(), u8>::undirected();
    let n = nodes(&mut g, 3);
    g.add_edge(n[0], n[1], 200)?;
    g.add_edge(n[1], n[2], 200)?;
    assert_eq!(g.minimum_spanning_tree(), Err(Error::Overflow));
    Ok(())
}

// --------------------------------------------------------------------- DSU

#[test]
fn union_find_basics() -> Result {
    let mut uf = UnionFind::new(6);
    assert_eq!((uf.len(), uf.set_count()), (6, 6));
    assert!(uf.union(0, 1)?);
    assert!(uf.union(2, 3)?);
    assert!(uf.union(1, 3)?);
    assert!(!uf.union(0, 2)?);
    assert!(uf.connected(0, 3)?);
    assert!(!uf.connected(0, 4)?);
    assert_eq!(uf.set_size(2)?, 4);
    assert_eq!(uf.set_count(), 3);
    assert_eq!(uf.find_root(3)?, uf.find(0)?);
    let e = uf.push();
    assert_eq!((e, uf.len(), uf.set_count()), (6, 7, 4));
    assert!(UnionFind::default().is_empty());
    Ok(())
}

#[test]
fn union_find_bounds_checked() {
    let mut uf = UnionFind::new(3);
    let err = Err(Error::IndexOutOfBounds { index: 3, len: 3 });
    assert_eq!(uf.find(3), err);
    assert_eq!(uf.find_root(3), err);
    assert_eq!(uf.union(0, 3), err.map(|_| false));
    assert_eq!(uf.connected(3, 0), err.map(|_| false));
    assert!(uf.set_size(usize::MAX).is_err());
    assert_eq!(uf.set_count(), 3, "failed calls change nothing");
}

#[test]
fn union_find_deep_chain_is_iterative() -> Result {
    let mut uf = UnionFind::new(DEEP);
    for i in 1..DEEP {
        uf.union(i - 1, i)?;
    }
    assert_eq!(uf.set_count(), 1);
    assert_eq!(uf.set_size(DEEP - 1)?, DEEP);
    assert!(uf.connected(0, DEEP - 1)?);
    Ok(())
}

// ------------------------------------------------------------------- trees

#[test]
fn fenwick_prefix_and_range_sums() -> Result {
    let values = [3i64, -1, 4, 1, -5, 9, 2, 6];
    let mut ft = FenwickTree::from_slice(&values)?;
    for end in 0..=values.len() {
        assert_eq!(ft.prefix_sum(end)?, values[..end].iter().sum::<i64>());
    }
    assert_eq!(ft.range_sum(2..5)?, 0);
    assert_eq!(ft.range_sum(2..=5)?, 9);
    assert_eq!(ft.range_sum(..)?, 19);
    assert_eq!(ft.range_sum(3..3)?, 0);
    ft.add(4, 10)?;
    assert_eq!(ft.get(4)?, 5);
    ft.set(0, 100)?;
    assert_eq!(ft.prefix_sum(1)?, 100);
    assert_eq!(ft.len(), 8);

    let mut zeros = FenwickTree::<u32>::new(4);
    zeros.add(3, 7)?;
    assert_eq!(zeros.range_sum(..)?, 7);
    Ok(())
}

#[test]
fn fenwick_errors_leave_tree_unchanged() -> Result {
    let mut ft = FenwickTree::from_slice(&[250u8, 0, 0, 0])?;
    let before = ft.clone();
    assert_eq!(ft.add(0, 10), Err(Error::Overflow));
    assert_eq!(ft, before, "overflow is all-or-nothing");
    assert_eq!(
        ft.add(4, 1),
        Err(Error::IndexOutOfBounds { index: 4, len: 4 })
    );
    assert_eq!(
        // Deliberately reversed range: must be rejected, not panic.
        ft.range_sum(std::ops::Range { start: 3, end: 2 }),
        Err(Error::InvalidRange {
            start: 3,
            end: 2,
            len: 4
        })
    );
    assert!(ft.range_sum(0..5).is_err());
    assert!(ft.prefix_sum(5).is_err());
    assert_eq!(FenwickTree::from_slice(&[200u8, 100]), Err(Error::Overflow));
    assert_eq!(
        FenwickTree::from_slice(&[1.0, f64::NAN]),
        Err(Error::InvalidValue)
    );
    let mut f = FenwickTree::<f64>::new(2);
    assert_eq!(f.add(0, f64::NAN), Err(Error::InvalidValue));
    assert_eq!(f.set(1, f64::INFINITY), Err(Error::InvalidValue));
    assert!(FenwickTree::<i32>::new(0).is_empty());
    Ok(())
}

#[test]
fn segment_tree_matches_naive() -> Result {
    let values: Vec<i64> = (0..37).map(|i| (i * 7919 % 101) - 50).collect();
    let mut st = SegmentTree::from_slice(&values, i64::MAX, |a, b| *a.min(b));
    let mut naive = values.clone();
    for step in 0..50usize {
        let i = step * 13 % naive.len();
        naive[i] = (step as i64 * 31) % 97 - 40;
        st.set(i, naive[i])?;
        for l in 0..naive.len() {
            for r in l..=naive.len() {
                let expect = naive[l..r].iter().copied().min().unwrap_or(i64::MAX);
                assert_eq!(st.query(l..r)?, expect);
            }
        }
    }
    assert_eq!(st.total(), *naive.iter().min().unwrap());
    Ok(())
}

#[test]
fn segment_tree_non_commutative_and_errors() -> Result {
    let letters: Vec<String> = "abcdefg".chars().map(String::from).collect();
    let mut st = SegmentTree::from_slice(&letters, String::new(), |a, b| format!("{a}{b}"));
    assert_eq!(st.query(..)?, "abcdefg");
    assert_eq!(st.query(2..5)?, "cde");
    assert_eq!(st.query(4..4)?, "");
    st.set(6, "G".into())?;
    assert_eq!(st.total(), "abcdefG");
    assert_eq!(st.get(0).map(String::as_str), Some("a"));
    assert_eq!(st.get(7), None);
    assert_eq!(
        st.set(7, "x".into()),
        Err(Error::IndexOutOfBounds { index: 7, len: 7 })
    );
    // Deliberately reversed range: must be rejected, not panic.
    assert!(st.query(std::ops::Range { start: 5, end: 2 }).is_err());
    assert!(st.query(0..=7).is_err());
    assert!(st.query(..=usize::MAX).is_err());

    let empty = SegmentTree::new(0, 0u64, |a, b| a.saturating_add(*b));
    assert!(empty.is_empty());
    assert_eq!(empty.total(), 0);
    assert_eq!(empty.query(..)?, 0);
    assert_eq!(*empty.identity(), 0);
    Ok(())
}

#[test]
fn segment_tree_saturating_sum_is_overflow_safe() -> Result {
    let mut st = SegmentTree::new(5, 0u32, |a, b| a.saturating_add(*b));
    for i in 0..5 {
        st.set(i, u32::MAX / 2)?;
    }
    assert_eq!(st.total(), u32::MAX);
    assert_eq!(st.query(0..1)?, u32::MAX / 2);
    Ok(())
}

// ------------------------------------------------------------------ search

#[test]
fn kmp_find_cases() {
    assert_eq!(kmp_find(b"hello world", b"o w"), Some(4));
    assert_eq!(kmp_find(b"abc", b"abcd"), None);
    assert_eq!(kmp_find(b"", b"a"), None);
    assert_eq!(kmp_find::<u8>(b"", b""), Some(0));
    assert_eq!(kmp_find(b"aabaabaaab", b"aaab"), Some(6));
    assert_eq!(kmp_find(&[1, 2, 1, 2, 3], &[1, 2, 3]), Some(2));
    let text = "naïve café".as_bytes();
    assert_eq!(kmp_find(text, "café".as_bytes()), Some(7));
}

#[test]
fn kmp_find_all_overlapping_and_matches_naive() {
    assert_eq!(kmp_find_all(b"aaaaa", b"aaa"), [0, 1, 2]);
    assert_eq!(kmp_find_all(b"abc", b""), [0, 1, 2, 3]);
    assert!(kmp_find_all(b"abc", b"x").is_empty());
    // Adversarial input for naive search: a^n b vs a^m b.
    let mut hay = vec![b'a'; DEEP];
    hay.push(b'b');
    let mut needle = vec![b'a'; 1000];
    needle.push(b'b');
    assert_eq!(kmp_find_all(&hay, &needle), [DEEP - 1000]);
    // Cross-check against a naive scan.
    let hay = b"abracadabra abracadabra";
    for needle in [&b"abra"[..], b"a", b"cad", b"ra a", b"zz"] {
        let naive: Vec<usize> = (0..=hay.len() - needle.len())
            .filter(|&i| &hay[i..i + needle.len()] == needle)
            .collect();
        assert_eq!(kmp_find_all(hay, needle), naive);
    }
}

#[test]
fn aho_corasick_overlapping_matches() {
    let ac = AhoCorasick::new(["he", "she", "his", "hers"]);
    assert_eq!(ac.pattern_count(), 4);
    let found = ac.find_all("ahishers");
    assert_eq!(
        found,
        [
            Match {
                pattern: 2,
                start: 1,
                end: 4
            },
            Match {
                pattern: 1,
                start: 3,
                end: 6
            },
            Match {
                pattern: 0,
                start: 4,
                end: 6
            },
            Match {
                pattern: 3,
                start: 4,
                end: 8
            },
        ]
    );
    assert_eq!(ac.find("xxshe").map(|m| m.pattern), Some(1));
    assert!(!ac.is_match("no match at all"));
}

#[test]
fn aho_corasick_nested_patterns_all_reported() {
    let ac = AhoCorasick::new(["a", "aa", "aaa"]);
    let found: Vec<(usize, usize, usize)> = ac
        .find_iter("aaaa")
        .map(|m| (m.pattern, m.start, m.end))
        .collect();
    assert_eq!(
        found,
        [
            (0, 0, 1),
            (1, 0, 2),
            (0, 1, 2),
            (2, 0, 3),
            (1, 1, 3),
            (0, 2, 3),
            (2, 1, 4),
            (1, 2, 4),
            (0, 3, 4),
        ]
    );
}

#[test]
fn aho_corasick_matches_naive_kmp_per_pattern() {
    let patterns = ["abra", "bra", "cad", "a", "dabra", "abracadabra", "zzz"];
    let hay = "abracadabra abracadabra";
    let ac = AhoCorasick::new(patterns);
    let mut got: Vec<(usize, usize)> = ac.find_iter(hay).map(|m| (m.pattern, m.start)).collect();
    let mut expected: Vec<(usize, usize)> = patterns
        .iter()
        .enumerate()
        .flat_map(|(p, pat)| {
            kmp_find_all(hay.as_bytes(), pat.as_bytes())
                .into_iter()
                .map(move |s| (p, s))
        })
        .collect();
    got.sort();
    expected.sort();
    assert_eq!(got, expected);
    for m in ac.find_iter(hay) {
        assert_eq!(&hay[m.range()], patterns[m.pattern]);
    }
}

#[test]
fn aho_corasick_edge_cases() {
    let none = AhoCorasick::new(Vec::<&str>::new());
    assert_eq!(none.pattern_count(), 0);
    assert!(none.find_all("anything").is_empty());

    let empty = AhoCorasick::new([""]);
    assert_eq!(
        empty
            .find_all("ab")
            .iter()
            .map(|m| m.start)
            .collect::<Vec<_>>(),
        [0, 1, 2]
    );

    let dup = AhoCorasick::new(["ab", "ab"]);
    let ids: Vec<usize> = dup.find_iter("ab").map(|m| m.pattern).collect();
    assert_eq!(ids, [0, 1]);

    let binary = AhoCorasick::new([&[0u8, 255][..], &[255, 255]]);
    assert_eq!(binary.find_all(&[0u8, 255, 255][..]).len(), 2);
    assert!(AhoCorasick::new(["x"]).find("").is_none());
}

#[test]
fn aho_corasick_adversarial_is_linear() {
    // Many patterns sharing long prefixes: a naive scan would be O(n·m·k).
    let patterns: Vec<Vec<u8>> = (1..=200)
        .map(|len| {
            let mut p = vec![b'a'; len];
            p.push(b'b');
            p
        })
        .collect();
    let hay = vec![b'a'; 200_000];
    let ac = AhoCorasick::new(&patterns);
    assert_eq!(ac.find_iter(&hay).count(), 0);
    let mut hay = hay;
    hay.push(b'b');
    assert_eq!(ac.find_iter(&hay).count(), 200);
}

#[test]
fn binary_search_bounds() {
    let v = [1, 3, 3, 3, 7, 9];
    assert_eq!(lower_bound(&v, &3), 1);
    assert_eq!(upper_bound(&v, &3), 4);
    assert_eq!(equal_range(&v, &3), 1..4);
    assert_eq!(equal_range(&v, &4), 4..4);
    assert_eq!(lower_bound(&v, &0), 0);
    assert_eq!(upper_bound(&v, &10), 6);
    let empty: [i32; 0] = [];
    assert_eq!((lower_bound(&empty, &1), upper_bound(&empty, &1)), (0, 0));
    let f = [-1.5f64, 0.0, 0.0, 2.25];
    assert_eq!(lower_bound_by(&f, |x| x.total_cmp(&0.0)), 1);
    assert_eq!(upper_bound_by(&f, |x| x.total_cmp(&0.0)), 3);
}

// ------------------------------------------------------------------ Number

#[test]
fn number_checked_semantics() {
    assert_eq!(Number::checked_add(i32::MAX, 1), None);
    assert_eq!(Number::checked_sub(0u32, 1), None);
    assert_eq!(Number::checked_add(2.5f64, 0.5), Some(3.0));
    assert_eq!(Number::checked_add(f32::MAX, f32::MAX), None);
    assert_eq!(Number::checked_sub(f64::NAN, 1.0), None);
    assert!(Number::is_negative(-1i8) && !Number::is_negative(0u64));
    assert!(Number::is_negative(-0.5f32) && !Number::is_negative(-0.0f32));
    assert!(Number::is_valid(5u8) && !Number::is_valid(f64::INFINITY));
    assert_eq!(<u16 as Number>::ZERO, 0);
}

#[test]
fn error_display_messages() {
    let cases = [
        (
            Error::InvalidNode {
                node: NodeId::new(4),
                node_count: 2,
            },
            "invalid node index 4 (graph has 2 nodes)",
        ),
        (Error::NegativeCycle, "graph contains a negative cycle"),
        (
            Error::NegativeWeight {
                edge: EdgeId::new(1),
            },
            "edge 1 has a negative weight",
        ),
        (
            Error::InvalidRange {
                start: 3,
                end: 1,
                len: 5,
            },
            "invalid range 3..1 for length 5",
        ),
    ];
    for (err, msg) in cases {
        assert_eq!(err.to_string(), msg);
        let _: &dyn std::error::Error = &err;
    }
}
