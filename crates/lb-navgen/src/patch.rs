//! Overlay patches applied to a generated graph: areas bots never plan through, links taken out, and links put in
//! (checked by the same classifier as generated ones, unless trusted).

use lb_bsp::BspWorld;
use lb_bsp::mech::Mechanisms;
use lb_config::overlay::Patch;
use lb_core::Vec3;
use lb_kin::Physics;
use lb_nav::classify::Classifier;
use lb_nav::graph::{LinkFlags, NavGraph, NavLink, NavNode, NodeId};

/// How far from a patch's point its node may be.
const SNAP: f32 = 64.0;

/// What applying the patches came to.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PatchReport {
    pub applied: usize,
    /// Patches that did nothing, and why (`nav.patches[i]: ...`).
    pub problems: Vec<String>,
}

fn nearest(nodes: &[NavNode], p: Vec3) -> Option<usize> {
    nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (i, n.origin.distance(p)))
        .filter(|&(_, d)| d <= SNAP)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

fn v(p: [f32; 3]) -> Vec3 {
    Vec3::from_array(p)
}

/// Applies `patches` in order to `graph` (made for `world`).
pub fn apply(
    graph: NavGraph,
    patches: &[Patch],
    world: &mut BspWorld,
    mech: &Mechanisms,
    phys: Physics,
) -> (NavGraph, PatchReport) {
    let mut report = PatchReport::default();
    if patches.is_empty() {
        return (graph, report);
    }
    let NavGraph {
        nodes,
        specs,
        source,
        stats,
        ..
    } = graph.clone();
    let mut out: Vec<Vec<NavLink>> = (0..nodes.len() as NodeId).map(|n| graph.links(n).to_vec()).collect();
    let mut cls = Classifier {
        world,
        mech,
        phys,
        nodes,
        specs,
    };
    for (i, patch) in patches.iter().enumerate() {
        let problem = |what: String| format!("nav.patches[{i}]: {what}");
        match patch {
            Patch::Forbid { at, radius, .. } => {
                let inside: Vec<NodeId> = (0..cls.nodes.len() as NodeId)
                    .filter(|&n| cls.nodes[n as usize].origin.distance(v(*at)) <= *radius)
                    .collect();
                if inside.is_empty() {
                    report.problems.push(problem("no node there".into()));
                    continue;
                }
                for (n, list) in out.iter_mut().enumerate() {
                    if inside.binary_search(&(n as NodeId)).is_ok() {
                        list.clear();
                    } else {
                        list.retain(|l| inside.binary_search(&l.to).is_err());
                    }
                }
                report.applied += 1;
            }
            Patch::RemoveLink { from, to, both, .. } => {
                let (Some(a), Some(b)) = (nearest(&cls.nodes, v(*from)), nearest(&cls.nodes, v(*to))) else {
                    report.problems.push(problem("no node near an end".into()));
                    continue;
                };
                let mut removed = 0;
                for (x, y) in [(a, b), (b, a)].into_iter().take(if *both { 2 } else { 1 }) {
                    let before = out[x].len();
                    out[x].retain(|l| l.to != y as NodeId);
                    removed += before - out[x].len();
                }
                if removed == 0 {
                    report.problems.push(problem(format!("there is no link {a} -> {b}")));
                } else {
                    report.applied += 1;
                }
            }
            Patch::AddLink {
                from,
                to,
                kind,
                both,
                trust,
                ..
            } => {
                let (Some(a), Some(b)) = (nearest(&cls.nodes, v(*from)), nearest(&cls.nodes, v(*to))) else {
                    report.problems.push(problem("no node near an end".into()));
                    continue;
                };
                if a == b {
                    report.problems.push(problem("both ends are the same node".into()));
                    continue;
                }
                let jump = kind.as_deref() == Some("jump");
                let mut added = false;
                for (x, y) in [(a, b), (b, a)].into_iter().take(if *both { 2 } else { 1 }) {
                    let c = cls.classify(x, y, jump);
                    if !c.valid && !trust {
                        report.problems.push(problem(format!(
                            "{x} -> {y} does not check out ({} tried); `trust: true` adds it anyway",
                            c.kind.as_str()
                        )));
                        continue;
                    }
                    let extra = if c.valid {
                        LinkFlags::empty()
                    } else {
                        LinkFlags::TRUSTED | LinkFlags::VALID
                    };
                    let link = cls.link(x, y, c, extra);
                    out[x].retain(|l| l.to != y as NodeId);
                    out[x].push(link);
                    added = true;
                }
                if added {
                    report.applied += 1;
                }
            }
        }
    }
    let probes = cls.probes(&out);
    let Classifier { nodes, specs, .. } = cls;
    let mut patched = NavGraph::from_parts(nodes, out, specs, &source, stats);
    patched.probes = probes;
    (patched, report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GenOptions, generate};

    #[test]
    fn patches_change_the_crossfire_graph() {
        let Some(maps) = lb_bsp::test_maps_dir() else { return };
        let Ok(bsp) = std::fs::read(maps.join("crossfire.bsp")) else {
            return;
        };
        let mut world = BspWorld::load(&bsp).unwrap();
        let mech = Mechanisms::from_world(&world);
        let g = generate(&mut world, &mech, &GenOptions::default(), "crossfire").graph;
        let (a, b) = (0 as NodeId, g.links(0)[0].to);
        let (pa, pb) = (g.node(a).origin.to_array(), g.node(b).origin.to_array());
        let patches = vec![
            Patch::RemoveLink {
                from: pa,
                to: pb,
                both: false,
                note: String::new(),
            },
            Patch::Forbid {
                at: g.node(5).origin.to_array(),
                radius: 1.0,
                note: String::new(),
            },
            Patch::RemoveLink {
                from: [99999.0, 0.0, 0.0],
                to: pb,
                both: false,
                note: String::new(),
            },
        ];
        let (patched, report) = apply(g.clone(), &patches, &mut world, &mech, Physics::default());
        assert_eq!(report.applied, 2, "{report:?}");
        assert_eq!(report.problems.len(), 1);
        assert!(patched.find_link(a, b).is_none());
        assert!(patched.links(5).is_empty());
        assert!((0..patched.len() as NodeId).all(|n| patched.links(n).iter().all(|l| l.to != 5)));
        // Putting the link back checks it again.
        let back = vec![Patch::AddLink {
            from: pa,
            to: pb,
            kind: None,
            both: false,
            trust: false,
            note: String::new(),
        }];
        let (restored, report) = apply(patched, &back, &mut world, &mech, Physics::default());
        assert_eq!(report.applied, 1, "{report:?}");
        assert!(restored.find_link(a, b).is_some_and(|l| l.valid()));
    }
}
