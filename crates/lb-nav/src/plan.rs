//! A* over the navigation graph. Costs are seconds: each link's traversal time (waiting for mechanisms included),
//! plus caller penalties. With teleports on the map the heuristic is off (Dijkstra): a teleport makes straight-line
//! distance a bad lower bound.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use crate::graph::{NavGraph, NodeId};

/// Running speed used to turn distances into seconds; the heuristic stays admissible because every link is at
/// least as slow as running.
pub const RUN_SPEED: f32 = 300.0;

#[derive(Clone, Copy, PartialEq)]
struct Open {
    f: f32,
    node: NodeId,
}

impl Eq for Open {}

impl Ord for Open {
    fn cmp(&self, other: &Self) -> Ordering {
        other.f.total_cmp(&self.f).then_with(|| other.node.cmp(&self.node))
    }
}

impl PartialOrd for Open {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Cheapest path from `from` to `to` as a list of nodes (both included). `penalty(a, b)` adds seconds to the link
/// a→b; `f32::INFINITY` removes it.
pub fn plan(
    graph: &NavGraph,
    from: NodeId,
    to: NodeId,
    penalty: &dyn Fn(NodeId, NodeId) -> f32,
) -> Option<Vec<NodeId>> {
    let n = graph.len();
    if from as usize >= n || to as usize >= n {
        return None;
    }
    let goal = graph.node(to).origin;
    let scale = if graph.has_teleports() { 0.0 } else { 1.0 / RUN_SPEED };
    let h = |id: NodeId| graph.node(id).origin.distance(goal) * scale;
    let mut g = vec![f32::INFINITY; n];
    let mut came = vec![u32::MAX; n];
    let mut open = BinaryHeap::new();
    g[from as usize] = 0.0;
    open.push(Open { f: h(from), node: from });
    while let Some(Open { f, node }) = open.pop() {
        if node == to {
            let mut path = vec![to];
            let mut cur = to;
            while cur != from {
                cur = came[cur as usize];
                path.push(cur);
            }
            path.reverse();
            return Some(path);
        }
        if f > g[node as usize] + h(node) + 1e-4 {
            continue;
        }
        for link in graph.links(node) {
            if !link.valid() {
                continue;
            }
            let extra = penalty(node, link.to);
            if !extra.is_finite() {
                continue;
            }
            let cand = g[node as usize] + link.cost + extra.max(0.0);
            if cand < g[link.to as usize] {
                g[link.to as usize] = cand;
                came[link.to as usize] = node;
                open.push(Open {
                    f: cand + h(link.to),
                    node: link.to,
                });
            }
        }
    }
    None
}

/// Travel time of a path in seconds (same costs as `plan`, without penalties).
pub fn path_time(graph: &NavGraph, path: &[NodeId]) -> f32 {
    path.windows(2)
        .filter_map(|w| graph.find_link(w[0], w[1]))
        .map(|l| l.cost)
        .sum()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::graph::{GraphStats, LinkFlags, LinkKind, NO_SPEC, NavLink, NavNode, NodeFlags};
    use lb_core::Vec3;

    pub(crate) fn graph(points: &[(f32, f32)], edges: &[(u32, u32, LinkKind)]) -> NavGraph {
        let nodes: Vec<NavNode> = points
            .iter()
            .map(|&(x, y)| NavNode {
                origin: Vec3::new(x, y, 0.0),
                flags: NodeFlags::empty(),
                radius: 16.0,
                support: 0,
                first_link: 0,
                link_count: 0,
            })
            .collect();
        let mut out = vec![Vec::new(); nodes.len()];
        for &(a, b, kind) in edges {
            let length = nodes[a as usize].origin.distance(nodes[b as usize].origin);
            out[a as usize].push(NavLink {
                to: b,
                kind,
                length,
                flags: LinkFlags::VALID,
                cost: length / (RUN_SPEED * kind.speed_factor()),
                spec: NO_SPEC,
            });
        }
        NavGraph::from_parts(nodes, out, Vec::new(), "test", GraphStats::default())
    }

    #[test]
    fn prefers_fast_links_and_respects_penalties() {
        // 0 → 1 → 3 by walking (200 u) or 0 → 2 → 3 crouched (same length but three times slower).
        let g = graph(
            &[(0.0, 0.0), (100.0, 0.0), (100.0, 1.0), (200.0, 0.0)],
            &[
                (0, 1, LinkKind::Walk),
                (1, 3, LinkKind::Walk),
                (0, 2, LinkKind::Crouch),
                (2, 3, LinkKind::Crouch),
            ],
        );
        assert_eq!(plan(&g, 0, 3, &|_, _| 0.0).unwrap(), vec![0, 1, 3]);
        let blocked = |a: NodeId, b: NodeId| if (a, b) == (1, 3) { f32::INFINITY } else { 0.0 };
        assert_eq!(plan(&g, 0, 3, &blocked).unwrap(), vec![0, 2, 3]);
        assert_eq!(plan(&g, 3, 0, &|_, _| 0.0), None, "links are directed");
        assert!((path_time(&g, &[0, 1, 3]) - 200.0 / RUN_SPEED).abs() < 1e-6);
    }

    #[test]
    fn teleports_are_found_without_the_distance_heuristic() {
        // 0 → 1 walks far; 0 → 2 teleports next to 1 almost for free.
        let mut g = graph(
            &[(0.0, 0.0), (3000.0, 0.0), (2990.0, 0.0), (-50.0, 0.0)],
            &[
                (0, 1, LinkKind::Walk),
                (0, 3, LinkKind::Walk),
                (3, 2, LinkKind::Teleport),
                (2, 1, LinkKind::Walk),
            ],
        );
        let teleport = g.links.iter_mut().find(|l| l.kind == LinkKind::Teleport).unwrap();
        teleport.cost = 0.2;
        assert_eq!(plan(&g, 0, 1, &|_, _| 0.0).unwrap(), vec![0, 3, 2, 1]);
    }
}
