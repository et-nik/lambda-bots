//! Shortest ways over the graph by travel time, for the tactics worked out once per map and for cover.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use lb_nav::NavGraph;
use lb_nav_api::NodeId;

pub const NONE: NodeId = NodeId::MAX;

#[derive(Clone, Copy, PartialEq)]
struct Open {
    cost: f32,
    node: NodeId,
}

impl Eq for Open {}

impl Ord for Open {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .cost
            .total_cmp(&self.cost)
            .then_with(|| other.node.cmp(&self.node))
    }
}

impl PartialOrd for Open {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Least travel time from a start to every node, and the node before each on the way (`NONE` for the start and
/// for nodes out of reach).
pub struct Tree {
    pub cost: Vec<f32>,
    pub pred: Vec<NodeId>,
}

impl Tree {
    /// Nodes on the way from the start to `to`, `to` last; empty when out of reach.
    pub fn path(&self, to: NodeId) -> Vec<NodeId> {
        if !self.cost.get(to as usize).is_some_and(|c| c.is_finite()) {
            return Vec::new();
        }
        let mut path = vec![to];
        let mut n = to;
        while self.pred[n as usize] != NONE {
            n = self.pred[n as usize];
            path.push(n);
        }
        path.reverse();
        path
    }
}

/// Travel times over usable links from `start`, up to `max` seconds.
pub fn tree(graph: &NavGraph, start: NodeId, max: f32) -> Tree {
    let n = graph.len();
    let mut t = Tree {
        cost: vec![f32::INFINITY; n],
        pred: vec![NONE; n],
    };
    if start as usize >= n {
        return t;
    }
    let mut open = BinaryHeap::new();
    t.cost[start as usize] = 0.0;
    open.push(Open { cost: 0.0, node: start });
    while let Some(Open { cost, node }) = open.pop() {
        if cost > t.cost[node as usize] {
            continue;
        }
        for l in graph.links(node).iter().filter(|l| l.plain()) {
            let next = cost + l.cost;
            if next <= max && next < t.cost[l.to as usize] {
                t.cost[l.to as usize] = next;
                t.pred[l.to as usize] = node;
                open.push(Open { cost: next, node: l.to });
            }
        }
    }
    t
}
