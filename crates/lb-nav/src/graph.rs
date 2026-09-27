//! The navigation graph: nodes where a player can stand and directed links between them. Special links carry a
//! traversal contract (`spec`).

use lb_core::{Vec2, Vec3};

use crate::spec::TraversalSpec;

pub type NodeId = u32;

/// `NavLink::spec` of links without a contract.
pub const NO_SPEC: u32 = u32::MAX;

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
        /// yapb marked a button or a lift here.
        const MECHANISM = 1 << 5;
        /// No floor under the node (mid-air or on a ladder).
        const AIRBORNE = 1 << 6;
        /// Under water: reached by swimming.
        const WATER = 1 << 7;
        /// Stands on a moving brush (a lift platform at rest): `NavNode::support`.
        const ON_MOVER = 1 << 8;
    }
}

bitflags::bitflags! {
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct LinkFlags: u16 {
        /// Planning may use it.
        const VALID = 1 << 0;
        /// Came from an imported graph.
        const IMPORTED = 1 << 1;
        /// Failed the offline check but is used because imported links are trusted.
        const TRUSTED = 1 << 2;
        /// The live server confirmed the check.
        const LIVE_CONFIRMED = 1 << 3;
        /// The live server disagreed with the offline check; not planned through.
        const LIVE_MISMATCH = 1 << 4;
        /// Depends on where a moving brush is (a drop into a lift shaft, a walk across a platform).
        const DYNAMIC = 1 << 5;
        /// Added from the map's mechanisms (lifts), not from the imported graph.
        const MECHANISM = 1 << 6;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LinkKind {
    Walk,
    Crouch,
    Jump,
    Drop,
    Ladder,
    Swim,
    Door,
    Lift,
    Teleport,
    Breakable,
}

impl LinkKind {
    pub const ALL: [LinkKind; 10] = [
        LinkKind::Walk,
        LinkKind::Crouch,
        LinkKind::Jump,
        LinkKind::Drop,
        LinkKind::Ladder,
        LinkKind::Swim,
        LinkKind::Door,
        LinkKind::Lift,
        LinkKind::Teleport,
        LinkKind::Breakable,
    ];

    /// Movement speed along the link relative to running.
    pub fn speed_factor(self) -> f32 {
        match self {
            LinkKind::Walk | LinkKind::Drop | LinkKind::Door | LinkKind::Lift | LinkKind::Teleport => 1.0,
            LinkKind::Breakable => 1.0,
            LinkKind::Jump => 0.8,
            LinkKind::Crouch => 1.0 / 3.0,
            LinkKind::Ladder => 0.5,
            LinkKind::Swim => 0.6,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            LinkKind::Walk => "walk",
            LinkKind::Crouch => "crouch",
            LinkKind::Jump => "jump",
            LinkKind::Drop => "drop",
            LinkKind::Ladder => "ladder",
            LinkKind::Swim => "swim",
            LinkKind::Door => "door",
            LinkKind::Lift => "lift",
            LinkKind::Teleport => "teleport",
            LinkKind::Breakable => "breakable",
        }
    }

    pub fn index(self) -> usize {
        self as usize
    }

    /// Links that just walk (crouched or not) between nodes.
    pub fn is_walk(self) -> bool {
        matches!(self, LinkKind::Walk | LinkKind::Crouch)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NavNode {
    /// Player origin (hull centre) standing, or crouching for `CROUCH` nodes, at this node.
    pub origin: Vec3,
    pub flags: NodeFlags,
    /// Reach tolerance, units.
    pub radius: f32,
    /// Brush model the node stands on when it is a mover (`ON_MOVER`); 0 otherwise.
    pub support: u16,
    pub first_link: u32,
    pub link_count: u16,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NavLink {
    pub to: NodeId,
    pub kind: LinkKind,
    pub length: f32,
    pub flags: LinkFlags,
    /// Least time to traverse, seconds (movement, plus waiting for mechanisms).
    pub cost: f32,
    /// Index into `NavGraph::specs`, `NO_SPEC` for plain walking.
    pub spec: u32,
}

impl NavLink {
    pub fn valid(&self) -> bool {
        self.flags.contains(LinkFlags::VALID) && !self.flags.contains(LinkFlags::LIVE_MISMATCH)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GraphStats {
    pub nodes: usize,
    pub links: usize,
    pub invalid: usize,
    pub by_kind: [usize; 10],
    pub unsettled: usize,
    /// Links added from the map's mechanisms.
    pub added: usize,
    pub traces: u64,
    pub millis: u128,
}

impl GraphStats {
    /// `walk 1200, jump 30, ...` for the kinds present.
    pub fn kinds(&self) -> String {
        LinkKind::ALL
            .iter()
            .filter(|k| self.by_kind[k.index()] > 0)
            .map(|k| format!("{} {}", k.as_str(), self.by_kind[k.index()]))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

#[derive(Clone, Debug, Default)]
pub struct NavGraph {
    pub nodes: Vec<NavNode>,
    pub links: Vec<NavLink>,
    pub specs: Vec<TraversalSpec>,
    /// Checks of special links against the live server.
    pub probes: Vec<crate::probe::LinkProbe>,
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

    pub fn spec(&self, link: &NavLink) -> Option<&TraversalSpec> {
        self.specs.get(link.spec as usize)
    }

    /// Teleports make straight-line distance a bad lower bound of travel time.
    pub fn has_teleports(&self) -> bool {
        self.stats.by_kind[LinkKind::Teleport.index()] > 0
    }

    /// Builds a graph from nodes and per-node outgoing links (CSR).
    pub fn from_parts(
        mut nodes: Vec<NavNode>,
        out: Vec<Vec<NavLink>>,
        specs: Vec<TraversalSpec>,
        source: &str,
        mut stats: GraphStats,
    ) -> NavGraph {
        let mut links = Vec::with_capacity(out.iter().map(Vec::len).sum());
        stats.links = 0;
        stats.invalid = 0;
        stats.by_kind = [0; 10];
        for (i, list) in out.into_iter().enumerate() {
            nodes[i].first_link = links.len() as u32;
            nodes[i].link_count = list.len() as u16;
            for l in list {
                stats.links += 1;
                stats.by_kind[l.kind.index()] += 1;
                if !l.valid() {
                    stats.invalid += 1;
                }
                links.push(l);
            }
        }
        stats.nodes = nodes.len();
        NavGraph {
            nodes,
            links,
            specs,
            probes: Vec::new(),
            source: source.to_string(),
            stats,
        }
    }
}
