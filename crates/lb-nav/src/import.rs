//! Builds a navigation graph from a yapb graph. yapb node heights are inconsistent, so every node is settled onto
//! the floor, and every link is re-checked and classified against the map: links that fail are kept but not
//! planned through (unless trusted).

use lb_core::Vec3;
use lb_worldq::{HullKind, Tracer};

use crate::graph::{GraphStats, LinkKind, NavGraph, NavLink, NavNode, NodeFlags, NodeId};
use crate::validate::{WalkCheck, jump_check, settle, walk_check};
use crate::yapb::{LINK_JUMP, YapbGraph, node_flags};

/// Centre of the crouching hull at the node's floor.
fn crouch_centre(n: &NavNode) -> Vec3 {
    if n.flags.contains(NodeFlags::CROUCH) {
        n.origin
    } else {
        n.origin - Vec3::Z * 18.0
    }
}

pub fn import_yapb(g: &YapbGraph, tracer: &mut dyn Tracer, trust_imported: bool, source: &str) -> NavGraph {
    let mut stats = GraphStats {
        nodes: g.nodes.len(),
        ..Default::default()
    };
    let mut nodes: Vec<NavNode> = Vec::with_capacity(g.nodes.len());
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
        let origin = if flags.contains(NodeFlags::LADDER) {
            n.origin
        } else {
            let hull = if flags.contains(NodeFlags::CROUCH) {
                HullKind::Crouch
            } else {
                HullKind::Stand
            };
            match settle(tracer, n.origin, hull) {
                Some(p) => p,
                None if hull == HullKind::Stand => match settle(tracer, n.origin - Vec3::Z * 18.0, HullKind::Crouch) {
                    Some(p) => {
                        flags |= NodeFlags::CROUCH;
                        p
                    }
                    None => {
                        stats.unsettled += 1;
                        flags |= NodeFlags::AIRBORNE;
                        n.origin
                    }
                },
                None => {
                    stats.unsettled += 1;
                    flags |= NodeFlags::AIRBORNE;
                    n.origin
                }
            }
        };
        nodes.push(NavNode {
            origin,
            flags,
            radius: n.radius.clamp(0.0, 128.0),
            first_link: 0,
            link_count: 0,
        });
    }

    let mut links: Vec<NavLink> = Vec::new();
    for (i, n) in g.nodes.iter().enumerate() {
        let first = links.len();
        for l in &n.links {
            let to = l.index as usize;
            if to >= nodes.len() || to == i {
                continue;
            }
            let (a, b) = (nodes[i], nodes[to]);
            let (kind, valid) = classify(tracer, &a, &b, l.flags);
            stats.links += 1;
            stats.by_kind[kind as usize] += 1;
            if !valid {
                stats.invalid += 1;
            }
            links.push(NavLink {
                to: to as NodeId,
                kind,
                length: a.origin.distance(b.origin),
                valid: valid || trust_imported,
            });
        }
        nodes[i].first_link = first as u32;
        nodes[i].link_count = (links.len() - first) as u16;
    }
    NavGraph {
        nodes,
        links,
        source: source.to_string(),
        stats,
    }
}

fn classify(tracer: &mut dyn Tracer, a: &NavNode, b: &NavNode, link_flags: u16) -> (LinkKind, bool) {
    if a.flags.contains(NodeFlags::LADDER) || b.flags.contains(NodeFlags::LADDER) {
        return (LinkKind::Ladder, true);
    }
    if link_flags & LINK_JUMP != 0 {
        return (LinkKind::Jump, true);
    }
    if a.flags.contains(NodeFlags::AIRBORNE) || b.flags.contains(NodeFlags::AIRBORNE) {
        return (LinkKind::Walk, true);
    }
    let crouch = a.flags.contains(NodeFlags::CROUCH) || b.flags.contains(NodeFlags::CROUCH);
    let as_walk = |r: WalkCheck, crouched: bool| match r {
        WalkCheck::Ok => Some(if crouched { LinkKind::Crouch } else { LinkKind::Walk }),
        WalkCheck::Drop(_) => Some(LinkKind::Drop),
        WalkCheck::Blocked | WalkCheck::Gap => None,
    };
    if !crouch && let Some(kind) = as_walk(walk_check(tracer, a.origin, b.origin, HullKind::Stand), false) {
        return (kind, true);
    }
    if let Some(kind) = as_walk(
        walk_check(tracer, crouch_centre(a), crouch_centre(b), HullKind::Crouch),
        true,
    ) {
        return (kind, true);
    }
    let stand_origin = |n: &NavNode| {
        if n.flags.contains(NodeFlags::CROUCH) {
            n.origin + Vec3::Z * 18.0
        } else {
            n.origin
        }
    };
    if jump_check(tracer, stand_origin(a), stand_origin(b)) {
        return (LinkKind::Jump, true);
    }
    (if crouch { LinkKind::Crouch } else { LinkKind::Walk }, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crossfire_imports_with_few_rejected_links() {
        let Some(maps) = lb_bsp::test_maps_dir() else { return };
        let (Ok(bsp), Ok(graph)) = (
            std::fs::read(maps.join("crossfire.bsp")),
            std::fs::read(maps.join("../addons/yapb/data/graph/crossfire.graph")),
        ) else {
            return;
        };
        let mut world = lb_bsp::BspWorld::load(&bsp).unwrap();
        let yapb = crate::yapb::parse(&graph).unwrap();
        let started = std::time::Instant::now();
        let g = import_yapb(&yapb, &mut world, false, "crossfire.graph");
        let s = &g.stats;
        eprintln!(
            "crossfire: {} nodes, {} links, {} invalid, by kind {:?}, {} unsettled, {} traces, {:?}",
            s.nodes,
            s.links,
            s.invalid,
            s.by_kind,
            s.unsettled,
            world.traces,
            started.elapsed()
        );
        assert_eq!(s.nodes, 1598);
        assert!(s.invalid * 20 < s.links, "more than 5% of links rejected: {s:?}");
        let reachable = crate::plan::plan(&g, 0, (g.len() - 1) as NodeId, &|_, _| 0.0);
        assert!(reachable.is_some(), "first and last node are connected");
    }
}
