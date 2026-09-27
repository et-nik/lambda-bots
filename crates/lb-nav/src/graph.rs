//! The navigation graph: nodes where a player can stand and directed links between them.

use lb_core::{Vec2, Vec3};

pub type NodeId = u32;

bitflags::bitflags! {
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct NodeFlags: u32 {
        /// Only reachable crouched.
        const CROUCH = 1 << 0;
        const LADDER = 1 << 1;
        /// An item, objective or other destination worth walking to.
        const GOAL = 1 << 2;
        const CAMP = 1 << 3;
        const SNIPER = 1 << 4;
        /// Needs a button or a lift (not executed before M2).
        const MECHANISM = 1 << 5;
        /// No floor under the node (mid-air or on a ladder).
        const AIRBORNE = 1 << 6;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LinkKind {
    Walk,
    Crouch,
    Jump,
    Drop,
    Ladder,
}

impl LinkKind {
    /// Movement speed along the link relative to running.
    pub fn speed_factor(self) -> f32 {
        match self {
            LinkKind::Walk | LinkKind::Drop => 1.0,
            LinkKind::Jump => 0.8,
            LinkKind::Crouch => 1.0 / 3.0,
            LinkKind::Ladder => 0.5,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            LinkKind::Walk => "walk",
            LinkKind::Crouch => "crouch",
            LinkKind::Jump => "jump",
            LinkKind::Drop => "drop",
            LinkKind::Ladder => "ladder",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NavNode {
    /// Player origin (hull centre) standing, or crouching for `CROUCH` nodes, at this node.
    pub origin: Vec3,
    pub flags: NodeFlags,
    /// Reach tolerance, units.
    pub radius: f32,
    pub first_link: u32,
    pub link_count: u16,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NavLink {
    pub to: NodeId,
    pub kind: LinkKind,
    pub length: f32,
    /// Passed the offline check (or was trusted); invalid links are not planned through.
    pub valid: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GraphStats {
    pub nodes: usize,
    pub links: usize,
    pub invalid: usize,
    pub by_kind: [usize; 5],
    pub unsettled: usize,
}

#[derive(Clone, Debug, Default)]
pub struct NavGraph {
    pub nodes: Vec<NavNode>,
    pub links: Vec<NavLink>,
    pub source: String,
    pub stats: GraphStats,
}

impl NavGraph {
    pub fn links(&self, node: NodeId) -> &[NavLink] {
        let n = &self.nodes[node as usize];
        &self.links[n.first_link as usize..n.first_link as usize + n.link_count as usize]
    }

    pub fn node(&self, id: NodeId) -> &NavNode {
        &self.nodes[id as usize]
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Up to `k` nodes closest to `p` within `max_dist`, nearest first. Vertical distance counts double so a node on
    /// the floor above is not taken for the one we stand on.
    pub fn nearest(&self, p: Vec3, max_dist: f32, k: usize) -> Vec<(NodeId, f32)> {
        let mut found: Vec<(NodeId, f32)> = self
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(i, n)| {
                let d = n.origin - p;
                let dist = (Vec2::new(d.x, d.y).length_squared() + (2.0 * d.z).powi(2)).sqrt();
                (dist <= max_dist).then_some((i as NodeId, dist))
            })
            .collect();
        found.sort_by(|a, b| a.1.total_cmp(&b.1));
        found.truncate(k);
        found
    }

    pub fn find_link(&self, from: NodeId, to: NodeId) -> Option<&NavLink> {
        self.links(from).iter().find(|l| l.to == to)
    }
}
