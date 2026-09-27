//! A* over the navigation graph. Costs are seconds: link length over the speed of its kind, plus caller penalties.

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
    let h = |id: NodeId| graph.node(id).origin.distance(goal) / RUN_SPEED;
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
            if !link.valid {
                continue;
            }
            let extra = penalty(node, link.to);
            if !extra.is_finite() {
                continue;
            }
            let cost = link.length / (RUN_SPEED * link.kind.speed_factor()) + extra.max(0.0);
            let cand = g[node as usize] + cost;
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
        .map(|l| l.length / (RUN_SPEED * l.kind.speed_factor()))
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{LinkKind, NavLink, NavNode, NodeFlags};
    use lb_core::Vec3;

    fn graph(points: &[(f32, f32)], edges: &[(u32, u32, LinkKind)]) -> NavGraph {
        let mut nodes: Vec<NavNode> = points
            .iter()
            .map(|&(x, y)| NavNode {
                origin: Vec3::new(x, y, 0.0),
                flags: NodeFlags::empty(),
                radius: 16.0,
                first_link: 0,
                link_count: 0,
            })
            .collect();
        let mut links = Vec::new();
        for (i, node) in nodes.iter_mut().enumerate() {
            node.first_link = links.len() as u32;
            for &(a, b, kind) in edges.iter().filter(|e| e.0 == i as u32) {
                let length = Vec3::new(points[a as usize].0, points[a as usize].1, 0.0).distance(Vec3::new(
                    points[b as usize].0,
                    points[b as usize].1,
                    0.0,
                ));
                links.push(NavLink {
                    to: b,
                    kind,
                    length,
                    valid: true,
                });
            }
            node.link_count = (links.len() - node.first_link as usize) as u16;
        }
        NavGraph {
            nodes,
            links,
            ..Default::default()
        }
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
}
