//! Overlay patches applied to a generated graph: areas bots never plan through, links taken out, and links and nodes
//! put in (checked by the same classifier and trick planners as generated ones, unless trusted).

use lb_bsp::BspWorld;
use lb_bsp::mech::Mechanisms;
use lb_config::overlay::Patch;
use lb_core::Vec3;
use lb_kin::Physics;
use lb_kin::tricks::{FULL_PUSH, TrickPlan, boost_view, plan_boost, plan_longjump};
use lb_nav::classify::{Classified, Classifier, crouch_origin, stand_origin};
use lb_nav::graph::{LinkFlags, LinkKind, NavGraph, NavLink, NavNode, NodeFlags, NodeId};
use lb_nav::validate::{WalkCheck, walk_check};
use lb_worldq::HullKind;

/// How far from a patch's point its node may be.
const SNAP: f32 = 64.0;
/// A node put in is linked with the nodes within this distance, as the generator links walks...
const REACH: f32 = 384.0;
/// ...trying this many of the nearest.
const TRIED: usize = 16;
/// Reach tolerance of a node put in.
const NODE_RADIUS: f32 = 32.0;
/// Another node this close makes a node put in pointless.
const TOO_CLOSE: f32 = 16.0;
/// Long jump speed along, for the flight of a trusted long jump that did not check out.
const LONGJUMP_SPEED: f32 = 560.0;
/// Pitch a trusted gauss boost that did not check out looks down at, degrees.
const TRUSTED_BOOST_PITCH: f32 = 60.0;

/// What one patch did.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub struct Outcome {
    pub ok: bool,
    /// What it did, or why it did nothing.
    pub message: String,
    /// Nodes it forbade or put in.
    pub nodes: Vec<NodeId>,
    /// Links it put in or took out.
    pub links: Vec<(NodeId, NodeId)>,
}

/// What applying the patches came to.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PatchReport {
    pub applied: usize,
    /// Patches that did nothing, and why (`nav.patches[i]: ...`).
    pub problems: Vec<String>,
    /// One per patch, in order.
    pub outcomes: Vec<Outcome>,
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

fn done(message: String, nodes: Vec<NodeId>, links: Vec<(NodeId, NodeId)>) -> Outcome {
    Outcome {
        ok: true,
        message,
        nodes,
        links,
    }
}

fn failed(message: String) -> Outcome {
    Outcome {
        ok: false,
        message,
        ..Outcome::default()
    }
}

/// The link `x → y` of the kind an added link names, when it checks out: `None` when it does not.
fn checked(cls: &mut Classifier<'_>, x: usize, y: usize, kind: Option<&str>) -> Option<Classified> {
    let (a, b) = (cls.nodes[x], cls.nodes[y]);
    match kind {
        Some("crouch") => {
            let r = walk_check(cls.world, crouch_origin(&a), crouch_origin(&b), HullKind::Crouch);
            matches!(r, WalkCheck::Ok | WalkCheck::Drop(_)).then(|| Classified::plain(LinkKind::Crouch, true))
        }
        Some("longjump") => {
            let plan = plan_longjump(cls.world, &cls.phys, stand_origin(&a), stand_origin(&b))?;
            Some(cls.longjump_from_plan(&a, &b, &plan))
        }
        Some("gauss_boost") => {
            let (from, to) = (stand_origin(&a), stand_origin(&b));
            let plan = plan_boost(cls.world, &cls.phys, from, to, FULL_PUSH)?;
            let view = boost_view((to - from).truncate().normalize_or_zero(), plan.pitch);
            let eye = lb_nav::tricks::boost_eye(from);
            lb_nav::tricks::beam_safe(cls.world, eye, view, FULL_PUSH / 5.0, true)
                .then(|| cls.boost_from_plan(&a, &b, &plan))
        }
        _ => Some(cls.classify(x, y, kind == Some("jump"))).filter(|c| c.valid),
    }
}

/// The link a trusted patch puts in when the check fails: the kind it names, with a contract that makes the bot try.
fn trusted(cls: &mut Classifier<'_>, x: usize, y: usize, kind: Option<&str>) -> Classified {
    let (a, b) = (cls.nodes[x], cls.nodes[y]);
    let flight = |speed: f32| (a.origin.distance(b.origin) / speed).clamp(0.3, 2.0);
    match kind {
        Some("longjump") => {
            let plan = TrickPlan {
                pitch: 0.0,
                robustness: 0.5,
                flight: flight(LONGJUMP_SPEED),
                impact: 0.0,
            };
            cls.longjump_from_plan(&a, &b, &plan)
        }
        Some("gauss_boost") => {
            let plan = TrickPlan {
                pitch: TRUSTED_BOOST_PITCH,
                robustness: 0.5,
                flight: flight(LONGJUMP_SPEED),
                impact: 0.0,
            };
            cls.boost_from_plan(&a, &b, &plan)
        }
        Some("crouch") => Classified::plain(LinkKind::Crouch, false),
        _ => cls.classify(x, y, kind == Some("jump")),
    }
}

/// Applies `patches` in order to `graph` (made for `world`, as `mapload::prepare_world` leaves it).
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
    for patch in patches {
        let outcome = match patch {
            Patch::Forbid { at, radius, .. } => forbid(&cls.nodes, &mut out, v(*at), *radius),
            Patch::RemoveLink { from, to, both, .. } => remove_link(&cls.nodes, &mut out, v(*from), v(*to), *both),
            Patch::AddLink {
                from,
                to,
                kind,
                both,
                trust,
                ..
            } => add_link(&mut cls, &mut out, v(*from), v(*to), kind.as_deref(), *both, *trust),
            Patch::AddNode { at, .. } => add_node(&mut cls, &mut out, v(*at)),
        };
        if outcome.ok {
            report.applied += 1;
        } else {
            let i = report.outcomes.len();
            report.problems.push(format!("nav.patches[{i}]: {}", outcome.message));
        }
        report.outcomes.push(outcome);
    }
    let probes = cls.probes(&out);
    let Classifier { nodes, specs, .. } = cls;
    let mut patched = NavGraph::from_parts(nodes, out, specs, &source, stats);
    patched.probes = probes;
    (patched, report)
}

fn forbid(nodes: &[NavNode], out: &mut [Vec<NavLink>], at: Vec3, radius: f32) -> Outcome {
    let inside: Vec<NodeId> = (0..nodes.len() as NodeId)
        .filter(|&n| nodes[n as usize].origin.distance(at) <= radius)
        .collect();
    if inside.is_empty() {
        return failed("no node there".into());
    }
    let mut links = Vec::new();
    for (n, list) in out.iter_mut().enumerate() {
        let n = n as NodeId;
        if inside.binary_search(&n).is_ok() {
            links.extend(list.iter().map(|l| (n, l.to)));
            list.clear();
        } else {
            links.extend(
                list.iter()
                    .filter(|l| inside.binary_search(&l.to).is_ok())
                    .map(|l| (n, l.to)),
            );
            list.retain(|l| inside.binary_search(&l.to).is_err());
        }
    }
    let message = format!("{} nodes shut off, {} links out", inside.len(), links.len());
    done(message, inside, links)
}

fn remove_link(nodes: &[NavNode], out: &mut [Vec<NavLink>], from: Vec3, to: Vec3, both: bool) -> Outcome {
    let (Some(a), Some(b)) = (nearest(nodes, from), nearest(nodes, to)) else {
        return failed("no node near an end".into());
    };
    let mut links = Vec::new();
    for (x, y) in [(a, b), (b, a)].into_iter().take(if both { 2 } else { 1 }) {
        let before = out[x].len();
        out[x].retain(|l| l.to != y as NodeId);
        if out[x].len() < before {
            links.push((x as NodeId, y as NodeId));
        }
    }
    if links.is_empty() {
        return failed(format!("there is no link {a} -> {b}"));
    }
    done(
        format!(
            "link {a} -> {b}{} taken out",
            if links.len() == 2 { " and back" } else { "" }
        ),
        vec![],
        links,
    )
}

fn add_link(
    cls: &mut Classifier<'_>,
    out: &mut [Vec<NavLink>],
    from: Vec3,
    to: Vec3,
    kind: Option<&str>,
    both: bool,
    trust: bool,
) -> Outcome {
    let (Some(a), Some(b)) = (nearest(&cls.nodes, from), nearest(&cls.nodes, to)) else {
        return failed("no node near an end".into());
    };
    if a == b {
        return failed("both ends are the same node".into());
    }
    let mut links = Vec::new();
    let mut refused = Vec::new();
    let mut unchecked = false;
    for (x, y) in [(a, b), (b, a)].into_iter().take(if both { 2 } else { 1 }) {
        let (c, extra) = match checked(cls, x, y, kind) {
            Some(c) => (c, LinkFlags::empty()),
            None if trust => {
                unchecked = true;
                (trusted(cls, x, y, kind), LinkFlags::TRUSTED | LinkFlags::VALID)
            }
            None => {
                refused.push(format!("{x} -> {y}"));
                continue;
            }
        };
        let made = c.kind;
        let link = cls.link(x, y, c, extra);
        out[x].retain(|l| l.to != y as NodeId);
        out[x].push(link);
        links.push((x as NodeId, y as NodeId, made));
    }
    let wanted = kind.unwrap_or("a link");
    if links.is_empty() {
        return failed(format!(
            "{} does not check out as {wanted}; `trust: true` adds it anyway",
            refused.join(" and ")
        ));
    }
    let made: Vec<String> = links
        .iter()
        .map(|(x, y, k)| format!("{x} -> {y} {}", k.as_str()))
        .collect();
    let mut message = format!(
        "{} put in{}",
        made.join(", "),
        if unchecked { " without the check (trusted)" } else { "" }
    );
    if !refused.is_empty() {
        message.push_str(&format!("; {} does not check out", refused.join(", ")));
    }
    done(message, vec![], links.into_iter().map(|(x, y, _)| (x, y)).collect())
}

fn add_node(cls: &mut Classifier<'_>, out: &mut Vec<Vec<NavLink>>, at: Vec3) -> Outcome {
    let (origin, flags, support) = cls.settle(at, NodeFlags::empty());
    if flags.contains(NodeFlags::AIRBORNE) {
        return failed("no floor within 96 units under the point, or the point is inside a wall".into());
    }
    if let Some(n) = (0..cls.nodes.len()).find(|&n| cls.nodes[n].origin.distance(origin) < TOO_CLOSE) {
        return failed(format!("node {n} is already there"));
    }
    let id = cls.nodes.len();
    cls.nodes.push(NavNode {
        origin,
        flags,
        radius: NODE_RADIUS,
        support,
        first_link: 0,
        link_count: 0,
    });
    out.push(Vec::new());
    let mut near: Vec<(f32, usize)> = (0..id)
        .map(|n| (cls.nodes[n].origin.distance(origin), n))
        .filter(|&(d, _)| d <= REACH)
        .collect();
    near.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    let mut links = Vec::new();
    for &(_, n) in near.iter().take(TRIED) {
        for (x, y) in [(id, n), (n, id)] {
            let c = cls.classify(x, y, false);
            if c.valid {
                let link = cls.link(x, y, c, LinkFlags::empty());
                out[x].push(link);
                links.push((x as NodeId, y as NodeId));
            }
        }
    }
    let id = id as NodeId;
    let outs = links.iter().filter(|(x, _)| *x == id).count();
    let message = format!(
        "node {id} at {:.0} {:.0} {:.0}{}: {outs} links out, {} in",
        origin.x,
        origin.y,
        origin.z,
        if flags.contains(NodeFlags::CROUCH) {
            ", crouched"
        } else {
            ""
        },
        links.len() - outs
    );
    if links.is_empty() {
        return Outcome {
            ok: false,
            message: format!("{message}: nothing around checks out; link it with a trusted add_link"),
            nodes: vec![id],
            links,
        };
    }
    done(message, vec![id], links)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GenOptions, generate};

    fn crossfire() -> Option<(BspWorld, Mechanisms, NavGraph)> {
        let maps = lb_bsp::test_maps_dir()?;
        let bsp = std::fs::read(maps.join("crossfire.bsp")).ok()?;
        let mut world = BspWorld::load(&bsp).unwrap();
        let mech = Mechanisms::from_world(&world);
        let g = generate(&mut world, &mech, &GenOptions::default(), "crossfire").graph;
        Some((world, mech, g))
    }

    #[test]
    fn patches_change_the_crossfire_graph() {
        let Some((mut world, mech, g)) = crossfire() else {
            return;
        };
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
        assert_eq!(report.outcomes.len(), 3);
        assert_eq!(report.outcomes[0].links, vec![(a, b)]);
        assert_eq!(report.outcomes[1].nodes, vec![5]);
        assert!(!report.outcomes[2].ok);
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

    #[test]
    fn a_node_put_in_gets_links_both_ways_and_later_patches_use_it() {
        let Some((mut world, mech, g)) = crossfire() else {
            return;
        };
        // Halfway along a walk link: floor for sure, but no node on it.
        let (a, l) = (0..g.len() as NodeId)
            .find_map(|n| {
                g.links(n)
                    .iter()
                    .find(|l| l.kind == LinkKind::Walk && l.valid() && l.length > 150.0)
                    .map(|l| (n, *l))
            })
            .expect("a long walk link");
        let mid = (g.node(a).origin + g.node(l.to).origin) * 0.5;
        let patches = vec![
            Patch::AddNode {
                at: mid.to_array(),
                note: String::new(),
            },
            Patch::RemoveLink {
                from: mid.to_array(),
                to: g.node(a).origin.to_array(),
                both: true,
                note: String::new(),
            },
        ];
        let (patched, report) = apply(g.clone(), &patches, &mut world, &mech, Physics::default());
        let id = g.len() as NodeId;
        assert!(report.outcomes[0].ok, "{report:?}");
        assert_eq!(report.outcomes[0].nodes, vec![id]);
        assert_eq!(patched.len(), g.len() + 1);
        assert!(!patched.links(id).is_empty(), "links out of the new node");
        assert!(
            (0..id).any(|n| patched.links(n).iter().any(|l| l.to == id)),
            "links into it"
        );
        assert!(report.outcomes[1].ok, "{report:?}");
        assert!(patched.find_link(id, a).is_none() && patched.find_link(a, id).is_none());
        let again = vec![Patch::AddNode {
            at: g.node(a).origin.to_array(),
            note: String::new(),
        }];
        let (_, report) = apply(g.clone(), &again, &mut world, &mech, Physics::default());
        assert!(
            !report.outcomes[0].ok && report.outcomes[0].message.contains("already there"),
            "{report:?}"
        );
        let air = vec![Patch::AddNode {
            at: [99999.0, 0.0, 0.0],
            note: String::new(),
        }];
        let (_, report) = apply(g, &air, &mut world, &mech, Physics::default());
        assert!(!report.outcomes[0].ok);
    }

    #[test]
    fn trick_kinds_are_planned_and_trust_puts_them_in_anyway() {
        let Some((mut world, mech, g)) = crossfire() else {
            return;
        };
        let (a, b) = (0 as NodeId, g.links(0)[0].to);
        let (pa, pb) = (g.node(a).origin.to_array(), g.node(b).origin.to_array());
        let link = |kind: &str, trust: bool| Patch::AddLink {
            from: pa,
            to: pb,
            kind: Some(kind.into()),
            both: false,
            trust,
            note: String::new(),
        };
        for kind in ["crouch", "longjump", "gauss_boost"] {
            let (patched, report) = apply(g.clone(), &[link(kind, true)], &mut world, &mech, Physics::default());
            assert!(report.outcomes[0].ok, "{kind}: {report:?}");
            let l = patched.find_link(a, b).expect("put in");
            assert_eq!(l.kind.as_str(), kind);
            assert!(l.valid());
            if l.kind.is_trick() {
                assert!(patched.spec(l).is_some(), "{kind} carries a contract");
            }
        }
        // A long jump onto a node a step away does not plan (it overshoots); without trust nothing is put in.
        let (patched, report) = apply(
            g.clone(),
            &[link("longjump", false)],
            &mut world,
            &mech,
            Physics::default(),
        );
        if !report.outcomes[0].ok {
            assert!(patched.find_link(a, b).is_none_or(|l| l.kind != LinkKind::LongJump));
        }
    }
}
