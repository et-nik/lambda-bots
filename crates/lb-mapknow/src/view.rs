//! The map as the bots know it: the graph, its tactics and what was learned by playing it, behind [`MapView`].

use lb_core::Vec3;
use lb_nav::NavGraph;
use lb_nav_api::{CampSpot, MapView, MineSpot, NodeId};

use crate::experience::Experience;
use crate::tactics::MapTactics;

#[derive(Clone, Copy)]
pub struct MapKnowledge<'a> {
    pub graph: &'a NavGraph,
    pub tactics: &'a MapTactics,
    pub experience: Option<&'a Experience>,
}

impl MapView for MapKnowledge<'_> {
    fn node_count(&self) -> usize {
        self.tactics.origins.len()
    }

    fn node_origin(&self, n: NodeId) -> Vec3 {
        self.tactics.origins.get(n as usize).copied().unwrap_or(Vec3::ZERO)
    }

    fn nearest_node(&self, p: Vec3, max: f32) -> Option<NodeId> {
        self.tactics.nearest(p, max)
    }

    fn for_each_link(&self, n: NodeId, f: &mut dyn FnMut(NodeId, f32)) {
        if (n as usize) < self.graph.len() {
            for l in self.graph.links(n).iter().filter(|l| l.plain()) {
                f(l.to, l.cost);
            }
        }
    }

    fn visible(&self, a: NodeId, b: NodeId) -> bool {
        self.tactics.vis.get(a, b)
    }

    fn for_each_visible(&self, n: NodeId, f: &mut dyn FnMut(NodeId)) {
        for m in self.tactics.vis.seen_from(n) {
            f(m);
        }
    }

    fn flow(&self, n: NodeId) -> f32 {
        self.tactics.flow.get(n as usize).copied().unwrap_or(0.0)
    }

    fn exposure(&self, n: NodeId) -> f32 {
        self.tactics.spots.get(n as usize).map_or(0.0, |s| s.near)
    }

    fn transit(&self, n: NodeId) -> bool {
        self.tactics.transit.get(n as usize).copied().unwrap_or(true)
    }

    fn danger(&self, n: NodeId) -> f32 {
        self.experience.map_or(0.0, |x| x.danger(n))
    }

    fn danger_from(&self, n: NodeId) -> Option<NodeId> {
        self.experience.and_then(|x| x.danger_from(n))
    }

    fn camp_spots(&self) -> &[CampSpot] {
        &self.tactics.camps
    }

    fn mine_spots(&self) -> &[MineSpot] {
        &self.tactics.mines
    }

    fn chokepoints(&self) -> &[NodeId] {
        &self.tactics.chokes
    }
}
