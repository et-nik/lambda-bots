//! A small map for tests: a corridor of nodes along x with a corner at x = 350 nobody sees around.

use lb_core::Vec3;
use lb_nav::NavGraph;
use lb_nav::graph::{GraphStats, LinkFlags, LinkKind, NavLink, NavNode, NodeFlags};
use lb_nav_api::NodeId;

use crate::tactics::{MapTactics, SpotStats};
use crate::vis::VisTable;

/// A corridor of nodes along x; the ones past a corner (x ≥ 400) are out of sight of those before it.
pub fn corridor() -> (NavGraph, MapTactics) {
    let n = 8;
    let nodes: Vec<NavNode> = (0..n)
        .map(|i| NavNode {
            origin: Vec3::new(i as f32 * 100.0, 0.0, 0.0),
            flags: NodeFlags::empty(),
            radius: 16.0,
            support: 0,
            first_link: 0,
            link_count: 0,
        })
        .collect();
    let link = |to: usize| NavLink {
        to: to as NodeId,
        kind: LinkKind::Walk,
        length: 100.0,
        flags: LinkFlags::VALID,
        cost: 1.0 / 3.0,
        spec: lb_nav::graph::NO_SPEC,
    };
    let out: Vec<Vec<NavLink>> = (0..n)
        .map(|i| {
            let mut v = Vec::new();
            if i > 0 {
                v.push(link(i - 1));
            }
            if i + 1 < n {
                v.push(link(i + 1));
            }
            v
        })
        .collect();
    let graph = NavGraph::from_parts(nodes, out, Vec::new(), "test", GraphStats::default());
    let eyes: Vec<Vec3> = graph.nodes.iter().map(|n| n.origin).collect();
    let (vis, _) = VisTable::build(&eyes, &|_, _| true, &|a, b| (a.x < 350.0) == (b.x < 350.0));
    let mut t = MapTactics {
        origins: eyes,
        transit: vec![false; n],
        vis,
        flow: vec![0.5; n],
        spots: vec![SpotStats::default(); n],
        ..MapTactics::default()
    };
    t.index();
    (graph, t)
}
