//! Builds a navigation graph from a yapb graph and the map.
//!
//! yapb node heights are inconsistent, so every node is settled onto the floor, with the map's movers where they
//! rest (a node on a lift lands on its platform). Every link is then classified against the map (`classify`); links
//! that fail are kept but not planned through (unless imported links are trusted).

use lb_bsp::BspWorld;
use lb_bsp::mech::Mechanisms;
use lb_kin::Physics;

use crate::classify::Classifier;
use crate::graph::{GraphStats, LinkFlags, NavGraph, NavLink, NavNode, NodeFlags};
use crate::yapb::{LINK_JUMP, YapbGraph, node_flags};

#[derive(Clone, Copy, Debug, Default)]
pub struct ImportOptions {
    /// Plan through links that fail the offline check.
    pub trust_imported: bool,
    pub physics: Physics,
}

pub fn import_yapb(
    g: &YapbGraph,
    world: &mut BspWorld,
    mech: &Mechanisms,
    opts: &ImportOptions,
    source: &str,
) -> NavGraph {
    let started = std::time::Instant::now();
    let traces_before = world.traces;
    mech.place_at_rest(world);
    let mut stats = GraphStats {
        nodes: g.nodes.len(),
        ..Default::default()
    };
    let mut ctx = Classifier {
        world,
        mech,
        phys: opts.physics,
        nodes: Vec::with_capacity(g.nodes.len()),
        specs: Vec::new(),
    };
    for n in &g.nodes {
        let mut flags = NodeFlags::empty();
        for (bit, flag) in [
            (node_flags::CROUCH, NodeFlags::CROUCH),
            (node_flags::LADDER, NodeFlags::LADDER),
            (node_flags::GOAL, NodeFlags::GOAL),
            (node_flags::CAMP, NodeFlags::CAMP),
            (node_flags::SNIPER, NodeFlags::SNIPER),
            (node_flags::BUTTON | node_flags::LIFT, NodeFlags::MECHANISM),
        ] {
            if n.flags & bit != 0 {
                flags |= flag;
            }
        }
        let (origin, flags, support) = ctx.settle(n.origin, flags);
        if flags.contains(NodeFlags::AIRBORNE) {
            stats.unsettled += 1;
        }
        ctx.nodes.push(NavNode {
            origin,
            flags,
            radius: n.radius.clamp(0.0, 128.0),
            support,
            first_link: 0,
            link_count: 0,
        });
    }
    let mut out: Vec<Vec<NavLink>> = vec![Vec::new(); ctx.nodes.len()];
    for (i, n) in g.nodes.iter().enumerate() {
        for l in &n.links {
            let to = l.index as usize;
            if to >= ctx.nodes.len() || to == i {
                continue;
            }
            let c = ctx.classify(i, to, l.flags & LINK_JUMP != 0);
            let trusted = !c.valid && opts.trust_imported;
            let mut flags = LinkFlags::IMPORTED;
            if trusted {
                flags |= LinkFlags::VALID | LinkFlags::TRUSTED;
            }
            let link = ctx.link(i, to, c, flags);
            out[i].push(link);
        }
    }
    stats.added = ctx.lift_links(&mut out);
    let probes = ctx.probes(&out);
    stats.traces = ctx.world.traces - traces_before;
    stats.millis = started.elapsed().as_millis();
    let Classifier { nodes, specs, .. } = ctx;
    let mut graph = NavGraph::from_parts(nodes, out, specs, source, stats);
    graph.probes = probes;
    graph
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{LinkKind, NodeId};
    use crate::spec::{Action, Interaction};

    #[test]
    fn crossfire_imports_with_mechanisms() {
        let Some(maps) = lb_bsp::test_maps_dir() else { return };
        let (Ok(bsp), Ok(graph)) = (
            std::fs::read(maps.join("crossfire.bsp")),
            std::fs::read(maps.join("../addons/yapb/data/graph/crossfire.graph")),
        ) else {
            return;
        };
        let mut world = lb_bsp::BspWorld::load(&bsp).unwrap();
        let mech = Mechanisms::from_world(&world);
        let yapb = crate::yapb::parse(&graph).unwrap();
        let g = import_yapb(&yapb, &mut world, &mech, &ImportOptions::default(), "crossfire.graph");
        let s = &g.stats;
        eprintln!(
            "crossfire: {} nodes, {} links, {} invalid, {} added; {}; {} unsettled, {} traces, {} ms",
            s.nodes,
            s.links,
            s.invalid,
            s.added,
            s.kinds(),
            s.unsettled,
            s.traces,
            s.millis
        );
        assert_eq!(s.nodes, 1598);
        assert!(s.invalid * 20 < s.links, "more than 5% of links rejected: {s:?}");
        let reachable = crate::plan::plan(&g, 0, (g.len() - 1) as NodeId, &|_, _| 0.0);
        assert!(reachable.is_some(), "first and last node are connected");
        // Every tower lift carries bots up from its platform.
        for model in [21u16, 30, 32, 34] {
            assert!(
                g.links.iter().any(|l| l.kind == LinkKind::Lift
                    && matches!(g.spec(l).map(|s| s.action), Some(Action::Lift { platform, .. }) if platform.model == model)),
                "no lift link for *{model}"
            );
        }
        let lift = g.links.iter().find(|l| l.kind == LinkKind::Lift).unwrap();
        let Some(Action::Lift { start, .. }) = g.spec(lift).map(|s| s.action) else {
            unreachable!()
        };
        assert!(matches!(start, Interaction::Use { .. }), "{start:?}");
    }

    /// A yapb link from a lift's platform straight to the top fails the check with the lift at rest: the lift link
    /// takes its place, so the follower rides the lift the planner chose.
    #[test]
    fn crossfire_lift_replaces_imported_link() {
        let Some(maps) = lb_bsp::test_maps_dir() else { return };
        let (Ok(bsp), Ok(graph)) = (
            std::fs::read(maps.join("crossfire.bsp")),
            std::fs::read(maps.join("../addons/yapb/data/graph/crossfire.graph")),
        ) else {
            return;
        };
        let import = |yapb: &YapbGraph, trust_imported: bool| {
            let mut world = lb_bsp::BspWorld::load(&bsp).unwrap();
            let mech = Mechanisms::from_world(&world);
            let opts = ImportOptions {
                trust_imported,
                ..Default::default()
            };
            import_yapb(yapb, &mut world, &mech, &opts, "crossfire.graph")
        };
        let mut yapb = crate::yapb::parse(&graph).unwrap();
        let g = import(&yapb, false);
        let lifts: Vec<(NodeId, NodeId)> = (0..g.len() as NodeId)
            .flat_map(|a| {
                g.links(a)
                    .iter()
                    .filter(|l| l.kind == LinkKind::Lift)
                    .map(move |l| (a, l.to))
            })
            .collect();
        assert!(!lifts.is_empty());
        for &(a, b) in &lifts {
            let mut l = yapb.nodes[a as usize].links[0];
            (l.index, l.flags) = (b as i16, 0);
            yapb.nodes[a as usize].links.push(l);
        }
        for trust in [false, true] {
            let g = import(&yapb, trust);
            for &(a, b) in &lifts {
                assert_eq!(g.links(a).iter().filter(|l| l.to == b).count(), 1, "{a} -> {b}");
                assert_eq!(g.find_link(a, b).map(|l| l.kind), Some(LinkKind::Lift), "{a} -> {b}");
            }
        }
    }
}
