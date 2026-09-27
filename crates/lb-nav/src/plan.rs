//! A* over the navigation graph. Costs are seconds: each link's traversal time (waiting for mechanisms included),
//! plus caller penalties. The heuristic is the larger of two lower bounds: straight-line distance at running speed
//! (off on maps with teleports, which make it overestimate) and ALT landmarks — exact graph distances from and to a
//! few spread-out nodes, which bound any distance from below by the triangle inequality. Searches run in slices of
//! node expansions, so the server can spread them over frames.

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

/// Landmarks for the ALT heuristic: graph distances (by link cost) from and to each landmark.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Alt {
    pub landmarks: Vec<NodeId>,
    from: Vec<Vec<f32>>,
    to: Vec<Vec<f32>>,
}

/// Distances by link cost from `start` over valid links, forward or against their direction.
fn distances(graph: &NavGraph, rev: &[Vec<(NodeId, f32)>], start: NodeId, backward: bool) -> Vec<f32> {
    let mut d = vec![f32::INFINITY; graph.len()];
    let mut open = BinaryHeap::new();
    d[start as usize] = 0.0;
    open.push(Open { f: 0.0, node: start });
    while let Some(Open { f, node }) = open.pop() {
        if f > d[node as usize] {
            continue;
        }
        let mut relax = |to: NodeId, cost: f32| {
            let cand = f + cost;
            if cand < d[to as usize] {
                d[to as usize] = cand;
                open.push(Open { f: cand, node: to });
            }
        };
        if backward {
            for &(from, cost) in &rev[node as usize] {
                relax(from, cost);
            }
        } else {
            for l in graph.links(node).iter().filter(|l| l.valid()) {
                relax(l.to, l.cost);
            }
        }
    }
    d
}

impl Alt {
    /// `count` landmarks spread over the graph: each the node farthest (by cost) from those picked before.
    pub fn build(graph: &NavGraph, count: usize) -> Alt {
        let mut alt = Alt::default();
        if graph.is_empty() {
            return alt;
        }
        let mut rev: Vec<Vec<(NodeId, f32)>> = vec![Vec::new(); graph.len()];
        for a in 0..graph.len() as NodeId {
            for l in graph.links(a).iter().filter(|l| l.valid()) {
                rev[l.to as usize].push((a, l.cost));
            }
        }
        // Distance to the nearest landmark picked so far; the next is the farthest reachable node.
        let mut nearest = distances(graph, &rev, 0, false);
        for _ in 0..count {
            let Some(next) = (0..graph.len() as NodeId)
                .filter(|&n| nearest[n as usize].is_finite() && !alt.landmarks.contains(&n))
                .max_by(|&a, &b| nearest[a as usize].total_cmp(&nearest[b as usize]).then(b.cmp(&a)))
            else {
                break;
            };
            let from = distances(graph, &rev, next, false);
            let to = distances(graph, &rev, next, true);
            for (n, d) in nearest.iter_mut().enumerate() {
                *d = if alt.landmarks.is_empty() {
                    from[n]
                } else {
                    d.min(from[n])
                };
            }
            alt.landmarks.push(next);
            alt.from.push(from);
            alt.to.push(to);
        }
        alt
    }

    /// A lower bound of the cost from `n` to `t`.
    pub fn bound(&self, n: NodeId, t: NodeId) -> f32 {
        let (n, t) = (n as usize, t as usize);
        let mut best = 0.0f32;
        for (from, to) in self.from.iter().zip(&self.to) {
            if from[n].is_finite() && from[t].is_finite() {
                best = best.max(from[t] - from[n]);
            }
            if to[n].is_finite() && to[t].is_finite() {
                best = best.max(to[n] - to[t]);
            }
        }
        best
    }
}

/// How a search slice ended.
#[derive(Clone, Debug, PartialEq)]
pub enum SearchStep {
    /// Out of budget: run it again later.
    Pending,
    Found(Vec<NodeId>),
    NoPath,
}

/// A search from `from` to `to` that runs in slices.
pub struct Search {
    from: NodeId,
    to: NodeId,
    g: Vec<f32>,
    came: Vec<u32>,
    open: BinaryHeap<Open>,
    /// Nodes expanded so far.
    pub expanded: u32,
}

impl Search {
    pub fn new(graph: &NavGraph, alt: Option<&Alt>, from: NodeId, to: NodeId) -> Search {
        let n = graph.len();
        let mut s = Search {
            from,
            to,
            g: vec![f32::INFINITY; n],
            came: vec![u32::MAX; n],
            open: BinaryHeap::new(),
            expanded: 0,
        };
        if (from as usize) < n && (to as usize) < n {
            s.g[from as usize] = 0.0;
            s.open.push(Open {
                f: heuristic(graph, alt, from, to),
                node: from,
            });
        }
        s
    }

    pub fn goal(&self) -> NodeId {
        self.to
    }

    /// Expands nodes until the path is found, there is none, or `budget` (decreased as it goes) runs out.
    /// `penalty(a, b)` adds seconds to the link a→b; `f32::INFINITY` removes it.
    pub fn run(
        &mut self,
        graph: &NavGraph,
        alt: Option<&Alt>,
        penalty: &dyn Fn(NodeId, NodeId) -> f32,
        budget: &mut u32,
    ) -> SearchStep {
        while let Some(&Open { f, node }) = self.open.peek() {
            if *budget == 0 {
                return SearchStep::Pending;
            }
            self.open.pop();
            if node == self.to {
                let mut path = vec![self.to];
                let mut cur = self.to;
                while cur != self.from {
                    cur = self.came[cur as usize];
                    path.push(cur);
                }
                path.reverse();
                return SearchStep::Found(path);
            }
            if f > self.g[node as usize] + heuristic(graph, alt, node, self.to) + 1e-4 {
                continue;
            }
            *budget -= 1;
            self.expanded += 1;
            for link in graph.links(node) {
                if !link.valid() {
                    continue;
                }
                let extra = penalty(node, link.to);
                if !extra.is_finite() {
                    continue;
                }
                let cand = self.g[node as usize] + link.cost + extra.max(0.0);
                if cand < self.g[link.to as usize] {
                    self.g[link.to as usize] = cand;
                    self.came[link.to as usize] = node;
                    self.open.push(Open {
                        f: cand + heuristic(graph, alt, link.to, self.to),
                        node: link.to,
                    });
                }
            }
        }
        SearchStep::NoPath
    }
}

fn heuristic(graph: &NavGraph, alt: Option<&Alt>, n: NodeId, goal: NodeId) -> f32 {
    let straight = if graph.has_teleports() {
        0.0
    } else {
        graph.node(n).origin.distance(graph.node(goal).origin) / RUN_SPEED
    };
    alt.map_or(straight, |a| straight.max(a.bound(n, goal)))
}

/// Cheapest path from `from` to `to` as a list of nodes (both included), in one go.
pub fn plan(
    graph: &NavGraph,
    from: NodeId,
    to: NodeId,
    penalty: &dyn Fn(NodeId, NodeId) -> f32,
) -> Option<Vec<NodeId>> {
    match Search::new(graph, None, from, to).run(graph, None, penalty, &mut u32::MAX.clone()) {
        SearchStep::Found(path) => Some(path),
        _ => None,
    }
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
        // Landmarks find the same path, and in slices.
        let alt = Alt::build(&g, 2);
        let mut s = Search::new(&g, Some(&alt), 0, 1);
        let mut steps = 0;
        let path = loop {
            let mut budget = 1;
            steps += 1;
            match s.run(&g, Some(&alt), &|_, _| 0.0, &mut budget) {
                SearchStep::Pending => continue,
                SearchStep::Found(p) => break p,
                SearchStep::NoPath => panic!("no path"),
            }
        };
        assert_eq!(path, vec![0, 3, 2, 1]);
        assert!(steps > 1);
    }

    /// A grid with random costs and one-way links: landmarks never overestimate, and A* with them finds paths as
    /// cheap as Dijkstra's.
    #[test]
    fn landmarks_are_admissible_and_paths_optimal() {
        let side = 12u32;
        let points: Vec<(f32, f32)> = (0..side * side)
            .map(|i| ((i % side) as f32 * 100.0, (i / side) as f32 * 100.0))
            .collect();
        let mut rng = lb_core::rng::Pcg32::new(3, 5);
        let mut edges = Vec::new();
        for i in 0..side * side {
            let (x, y) = (i % side, i / side);
            for (dx, dy) in [(1i32, 0i32), (0, 1), (-1, 0), (0, -1)] {
                let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                if nx < 0 || ny < 0 || nx >= side as i32 || ny >= side as i32 || rng.chance(15.0) {
                    continue;
                }
                let kind = if rng.chance(30.0) {
                    LinkKind::Crouch
                } else {
                    LinkKind::Walk
                };
                edges.push((i, ny as u32 * side + nx as u32, kind));
            }
        }
        let g = graph(&points, &edges);
        let alt = Alt::build(&g, 6);
        assert_eq!(alt.landmarks.len(), 6);
        let exact = |a: NodeId, b: NodeId| plan(&g, a, b, &|_, _| 0.0).map(|p| path_time(&g, &p));
        for (a, b) in [(0, side * side - 1), (5, 100), (130, 7), (77, 78), (143, 0)] {
            let Some(best) = exact(a, b) else { continue };
            assert!(alt.bound(a, b) <= best + 1e-3, "bound {} > {best}", alt.bound(a, b));
            let mut s = Search::new(&g, Some(&alt), a, b);
            let SearchStep::Found(p) = s.run(&g, Some(&alt), &|_, _| 0.0, &mut u32::MAX.clone()) else {
                panic!("no path {a} -> {b}");
            };
            assert!((path_time(&g, &p) - best).abs() < 1e-3);
        }
    }
}
