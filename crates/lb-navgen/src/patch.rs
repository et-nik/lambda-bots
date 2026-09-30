//! Overlay patches applied to a generated graph: areas bots never plan through, links taken out, and links and nodes
//! put in or moved (checked by the same classifier and trick planners as generated ones, unless trusted).
//!
//! A patch does not quietly undo an earlier one: nothing after a forbidden zone links the nodes it shut off or puts a
//! node in it, and the links a node put in or moved makes by itself leave out the links a patch took out.

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
use rustc_hash::FxHashSet;

/// How far from a patch's point its node may be.
const SNAP: f32 = 64.0;
/// A node put in is linked with the nodes within this distance, as the generator links walks...
const REACH: f32 = 384.0;
/// ...trying this many of the nearest.
const TRIED: usize = 16;
/// Reach tolerance of a node put in (and the most a moved node keeps).
const NODE_RADIUS: f32 = 32.0;
/// Another node this close makes a node put in pointless.
const TOO_CLOSE: f32 = 16.0;
/// Long jump speed along, for the flight of a trusted long jump that did not check out.
const LONGJUMP_SPEED: f32 = 560.0;
/// Pitch a trusted gauss boost that did not check out looks down at, degrees.
const TRUSTED_BOOST_PITCH: f32 = 60.0;
/// Flags a moved node keeps: what it is for. Where it stands (crouched, in water, on a lift) is found again.
const KEPT_FLAGS: NodeFlags = NodeFlags::LADDER
    .union(NodeFlags::GOAL)
    .union(NodeFlags::CAMP)
    .union(NodeFlags::SNIPER)
    .union(NodeFlags::MECHANISM);

/// What one patch did.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub struct Outcome {
    pub ok: bool,
    /// What it did, or why it did nothing.
    pub message: String,
    /// Nodes it forbade, put in or moved (none when it did not); the two ends of a link it named.
    pub nodes: Vec<NodeId>,
    /// Links it put in or took out; the links a moved node has now.
    pub links: Vec<(NodeId, NodeId)>,
    /// Links it asked for that do not check out, a patch that put in the link one way only included.
    pub refused: Vec<Refusal>,
}

/// A link a patch asked for that does not check out, and why, from what its check found.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Refusal {
    pub from: NodeId,
    pub to: NodeId,
    pub why: String,
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

fn place(p: Vec3) -> String {
    format!("{:.0} {:.0} {:.0}", p.x, p.y, p.z)
}

fn crouched(flags: NodeFlags) -> &'static str {
    if flags.contains(NodeFlags::CROUCH) {
        ", crouched"
    } else {
        ""
    }
}

fn done(message: String, nodes: Vec<NodeId>, links: Vec<(NodeId, NodeId)>) -> Outcome {
    Outcome {
        ok: true,
        message,
        nodes,
        links,
        refused: Vec::new(),
    }
}

fn failed(message: String) -> Outcome {
    Outcome {
        ok: false,
        message,
        ..Outcome::default()
    }
}

/// A patch that did nothing, naming the nodes it found.
fn refused(message: String, nodes: Vec<NodeId>) -> Outcome {
    Outcome {
        nodes,
        ..failed(message)
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
            lb_nav::tricks::beam_safe(cls.world, eye, view, plan.push / 5.0, true)
                .then(|| cls.boost_from_plan(&a, &b, &plan))
        }
        _ => Some(cls.classify(x, y, kind == Some("jump"))).filter(|c| c.valid),
    }
}

/// Why the link `x → y` of the kind an added link names does not check out, from what its check found.
fn why_not(cls: &mut Classifier<'_>, x: usize, y: usize, kind: Option<&str>) -> String {
    let (a, b) = (cls.nodes[x], cls.nodes[y]);
    let (from, to) = (stand_origin(&a), stand_origin(&b));
    let phys = cls.phys;
    match kind {
        Some("longjump") => lb_kin::tricks::why_no_longjump(cls.world, &phys, from, to),
        Some("gauss_boost") => match plan_boost(cls.world, &phys, from, to, FULL_PUSH) {
            None => lb_kin::tricks::why_no_boost(cls.world, &phys, from, to, FULL_PUSH),
            Some(_) => {
                "the beam would glance off, or come back at the bot through a wall too thick to punch through".into()
            }
        },
        Some("crouch") => walk_why(walk_check(
            cls.world,
            crouch_origin(&a),
            crouch_origin(&b),
            HullKind::Crouch,
        )),
        Some("jump") => lb_kin::validate::why_no_jump(cls.world, &phys, from, to),
        _ => format!(
            "walking there {}, and {}",
            walk_why(walk_check(cls.world, a.origin, b.origin, HullKind::Stand)),
            lb_kin::validate::why_no_jump(cls.world, &phys, from, to)
        ),
    }
}

fn walk_why(r: WalkCheck) -> String {
    match r {
        WalkCheck::Ok => "gets there".into(),
        WalkCheck::Drop(h) => format!("falls {h:.0} u on the way"),
        WalkCheck::Gap => "is cut by a gap".into(),
        _ => "is blocked".into(),
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
                push: 0.0,
                robustness: 0.5,
                flight: flight(LONGJUMP_SPEED),
                impact: 0.0,
            };
            cls.longjump_from_plan(&a, &b, &plan)
        }
        Some("gauss_boost") => {
            let plan = TrickPlan {
                pitch: TRUSTED_BOOST_PITCH,
                push: FULL_PUSH,
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

/// The kind a link of a moved node is checked again as: tricks are planned again and a jump is tried first; what
/// other links are is found anew (a walk may turn into a drop).
fn recheck_kind(kind: LinkKind) -> Option<&'static str> {
    match kind {
        LinkKind::Jump => Some("jump"),
        LinkKind::Crouch => Some("crouch"),
        LinkKind::LongJump => Some("longjump"),
        LinkKind::GaussBoost => Some("gauss_boost"),
        _ => None,
    }
}

/// The graph being patched: its nodes (in the classifier that checks their links), each node's links out, and what
/// the patches so far leave for the next.
struct Patcher<'a> {
    cls: Classifier<'a>,
    out: Vec<Vec<NavLink>>,
    /// Nodes a forbidden zone shut off.
    shut: Vec<bool>,
    /// The forbidden zones: centre and radius.
    zones: Vec<(Vec3, f32)>,
    /// Links taken out.
    taken: FxHashSet<(NodeId, NodeId)>,
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
    let mut p = Patcher {
        out: (0..nodes.len() as NodeId).map(|n| graph.links(n).to_vec()).collect(),
        shut: vec![false; nodes.len()],
        zones: Vec::new(),
        taken: FxHashSet::default(),
        cls: Classifier {
            world,
            mech,
            phys,
            nodes,
            specs,
        },
    };
    for patch in patches {
        let outcome = match patch {
            Patch::Forbid { at, radius, .. } => p.forbid(v(*at), *radius),
            Patch::RemoveLink { from, to, both, .. } => p.remove_link(v(*from), v(*to), *both),
            Patch::AddLink {
                from,
                to,
                kind,
                both,
                trust,
                ..
            } => p.add_link(v(*from), v(*to), kind.as_deref(), *both, *trust),
            Patch::AddNode { at, link, .. } => p.add_node(v(*at), *link),
            Patch::MoveNode { from, to, .. } => p.move_node(v(*from), v(*to)),
        };
        if outcome.ok {
            report.applied += 1;
        } else {
            let i = report.outcomes.len();
            report.problems.push(format!("nav.patches[{i}]: {}", outcome.message));
        }
        report.outcomes.push(outcome);
    }
    let probes = p.cls.probes(&p.out);
    let Patcher {
        cls: Classifier { nodes, specs, .. },
        out,
        ..
    } = p;
    let mut patched = NavGraph::from_parts(nodes, out, specs, &source, stats);
    patched.probes = probes;
    (patched, report)
}

impl Patcher<'_> {
    fn forbid(&mut self, at: Vec3, radius: f32) -> Outcome {
        self.zones.push((at, radius));
        let nodes = &self.cls.nodes;
        let inside: Vec<NodeId> = (0..nodes.len() as NodeId)
            .filter(|&n| nodes[n as usize].origin.distance(at) <= radius)
            .collect();
        if inside.is_empty() {
            return failed("no node there".into());
        }
        for &n in &inside {
            self.shut[n as usize] = true;
        }
        let mut links = Vec::new();
        for (n, list) in self.out.iter_mut().enumerate() {
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

    fn remove_link(&mut self, from: Vec3, to: Vec3, both: bool) -> Outcome {
        let (Some(a), Some(b)) = (nearest(&self.cls.nodes, from), nearest(&self.cls.nodes, to)) else {
            return failed("no node near an end".into());
        };
        let ends = vec![a as NodeId, b as NodeId];
        let mut links = Vec::new();
        for (x, y) in [(a, b), (b, a)].into_iter().take(if both { 2 } else { 1 }) {
            self.taken.insert((x as NodeId, y as NodeId));
            let before = self.out[x].len();
            self.out[x].retain(|l| l.to != y as NodeId);
            if self.out[x].len() < before {
                links.push((x as NodeId, y as NodeId));
            }
        }
        if links.is_empty() {
            return refused(format!("there is no link {a} -> {b}"), ends);
        }
        done(
            format!(
                "link {a} -> {b}{} taken out",
                if links.len() == 2 { " and back" } else { "" }
            ),
            ends,
            links,
        )
    }

    fn add_link(&mut self, from: Vec3, to: Vec3, kind: Option<&str>, both: bool, trust: bool) -> Outcome {
        let (Some(a), Some(b)) = (nearest(&self.cls.nodes, from), nearest(&self.cls.nodes, to)) else {
            return failed("no node near an end".into());
        };
        let ends = vec![a as NodeId, b as NodeId];
        if a == b {
            return refused("both ends are the same node".into(), ends);
        }
        if let Some(n) = [a, b].into_iter().find(|&n| self.shut[n]) {
            return refused(format!("node {n} is shut off by a forbidden zone"), ends);
        }
        let mut links = Vec::new();
        let mut refusals = Vec::new();
        let mut unchecked = false;
        for (x, y) in [(a, b), (b, a)].into_iter().take(if both { 2 } else { 1 }) {
            let (c, extra) = match checked(&mut self.cls, x, y, kind) {
                Some(c) => (c, LinkFlags::empty()),
                None if trust => {
                    unchecked = true;
                    (
                        trusted(&mut self.cls, x, y, kind),
                        LinkFlags::TRUSTED | LinkFlags::VALID,
                    )
                }
                None => {
                    refusals.push(Refusal {
                        from: x as NodeId,
                        to: y as NodeId,
                        why: why_not(&mut self.cls, x, y, kind),
                    });
                    continue;
                }
            };
            let made = c.kind;
            let link = self.cls.link(x, y, c, extra);
            self.out[x].retain(|l| l.to != y as NodeId);
            self.out[x].push(link);
            links.push((x as NodeId, y as NodeId, made));
        }
        let wanted = kind.unwrap_or("a link");
        let named: Vec<String> = refusals.iter().map(|r| format!("{} -> {}", r.from, r.to)).collect();
        if links.is_empty() {
            return Outcome {
                refused: refusals,
                ..refused(
                    format!(
                        "{} does not check out as {wanted}; `trust: true` adds it anyway",
                        named.join(" and ")
                    ),
                    ends,
                )
            };
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
        if !refusals.is_empty() {
            message.push_str(&format!("; {} does not check out", named.join(", ")));
        }
        Outcome {
            refused: refusals,
            ..done(message, ends, links.into_iter().map(|(x, y, _)| (x, y)).collect())
        }
    }

    fn add_node(&mut self, at: Vec3, link: bool) -> Outcome {
        let (origin, flags, support) = self.cls.settle(at, NodeFlags::empty());
        if flags.contains(NodeFlags::AIRBORNE) {
            return failed("no floor within 96 units under the point, or the point is inside a wall".into());
        }
        if let Some(why) = self.no_room(origin, None) {
            return failed(why);
        }
        let id = self.cls.nodes.len();
        self.cls.nodes.push(NavNode {
            origin,
            flags,
            radius: NODE_RADIUS,
            support,
            first_link: 0,
            link_count: 0,
        });
        self.out.push(Vec::new());
        self.shut.push(false);
        if !link {
            let id = id as NodeId;
            let message = format!("node {id} at {}{}: not linked", place(origin), crouched(flags));
            return done(message, vec![id], Vec::new());
        }
        let links = self.link_around(id, &FxHashSet::default());
        let id = id as NodeId;
        let outs = links.iter().filter(|(x, _)| *x == id).count();
        let message = format!(
            "node {id} at {}{}: {outs} links out, {} in",
            place(origin),
            crouched(flags),
            links.len() - outs
        );
        if links.is_empty() {
            return Outcome {
                message: format!("{message}: nothing around checks out; link it with a trusted add_link"),
                nodes: vec![id],
                links,
                ..Outcome::default()
            };
        }
        done(message, vec![id], links)
    }

    fn move_node(&mut self, from: Vec3, to: Vec3) -> Outcome {
        let Some(n) = nearest(&self.cls.nodes, from) else {
            return failed("no node within 64 units of `from`".into());
        };
        let id = n as NodeId;
        if self.shut[n] {
            return failed(format!("node {n} is shut off by a forbidden zone"));
        }
        let node = self.cls.nodes[n];
        let (origin, flags, support) = self.cls.settle(to, node.flags & KEPT_FLAGS);
        if flags.contains(NodeFlags::AIRBORNE) {
            return failed(format!(
                "node {n}: no floor within 96 units under `to`, or it is inside a wall"
            ));
        }
        if let Some(why) = self.no_room(origin, Some(n)) {
            return failed(format!("node {n}: {why}"));
        }
        // Its links both ways, to be checked again from where it stands now.
        let mut old: Vec<(usize, usize, NavLink)> = std::mem::take(&mut self.out[n])
            .into_iter()
            .map(|l| (n, l.to as usize, l))
            .collect();
        for (m, list) in self.out.iter_mut().enumerate() {
            if list.iter().any(|l| l.to == id) {
                let (into, rest): (Vec<NavLink>, Vec<NavLink>) =
                    std::mem::take(list).into_iter().partition(|l| l.to == id);
                *list = rest;
                old.extend(into.into_iter().map(|l| (m, n, l)));
            }
        }
        self.cls.nodes[n] = NavNode {
            origin,
            flags,
            support,
            radius: node.radius.min(NODE_RADIUS),
            ..node
        };
        let mut links = Vec::new();
        let mut lost = 0;
        let mut tried = FxHashSet::default();
        for (x, y, l) in old {
            tried.insert((x, y));
            let kind = recheck_kind(l.kind);
            let (c, extra) = match checked(&mut self.cls, x, y, kind) {
                Some(c) => (c, LinkFlags::empty()),
                None if l.flags.contains(LinkFlags::TRUSTED) => (
                    trusted(&mut self.cls, x, y, kind),
                    LinkFlags::TRUSTED | LinkFlags::VALID,
                ),
                None => {
                    lost += 1;
                    continue;
                }
            };
            // Two links of a pair may check out as the same one.
            if self.out[x].iter().any(|e| e.to == y as NodeId && e.kind == c.kind) {
                continue;
            }
            let link = self.cls.link(x, y, c, extra);
            self.out[x].push(link);
            links.push((x as NodeId, y as NodeId));
        }
        let held = links.len();
        links.extend(self.link_around(n, &tried));
        let message = format!(
            "node {n} moved {:.0} units to {}{}: {held} links hold, {lost} taken out, {} put in",
            node.origin.distance(origin),
            place(origin),
            crouched(flags),
            links.len() - held
        );
        if links.is_empty() {
            return Outcome {
                message: format!("{message}: nothing around checks out; link it with a trusted add_link"),
                nodes: vec![id],
                links,
                ..Outcome::default()
            };
        }
        done(message, vec![id], links)
    }

    /// Why a node may not stand at `origin`: a forbidden zone, or another node than `moving` too close.
    fn no_room(&self, origin: Vec3, moving: Option<usize>) -> Option<String> {
        if let Some((at, r)) = self.zones.iter().find(|(at, r)| at.distance(origin) <= *r) {
            return Some(format!(
                "the spot is in the forbidden zone at {} ({r:.0} units)",
                place(*at)
            ));
        }
        (0..self.cls.nodes.len())
            .find(|&m| Some(m) != moving && self.cls.nodes[m].origin.distance(origin) < TOO_CLOSE)
            .map(|m| format!("node {m} is already there"))
    }

    /// Links node `id` both ways with the nearest nodes around it where the links check out, leaving out the nodes
    /// shut off, the links taken out, the links it has and the pairs in `tried`.
    fn link_around(&mut self, id: usize, tried: &FxHashSet<(usize, usize)>) -> Vec<(NodeId, NodeId)> {
        let origin = self.cls.nodes[id].origin;
        let mut near: Vec<(f32, usize)> = (0..self.cls.nodes.len())
            .filter(|&n| n != id && !self.shut[n])
            .map(|n| (self.cls.nodes[n].origin.distance(origin), n))
            .filter(|&(d, _)| d <= REACH)
            .collect();
        near.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        let mut links = Vec::new();
        for &(_, n) in near.iter().take(TRIED) {
            for (x, y) in [(id, n), (n, id)] {
                let pair = (x as NodeId, y as NodeId);
                if tried.contains(&(x, y)) || self.taken.contains(&pair) || self.out[x].iter().any(|l| l.to == pair.1) {
                    continue;
                }
                let c = self.cls.classify(x, y, false);
                if c.valid {
                    let link = self.cls.link(x, y, c, LinkFlags::empty());
                    self.out[x].push(link);
                    links.push(pair);
                }
            }
        }
        links
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GenOptions, generate};

    /// A node with a long walk link out, the link, and the point halfway along it: floor for sure, but no node on it.
    fn long_walk(g: &NavGraph) -> (NodeId, NavLink, Vec3) {
        let (a, l) = (0..g.len() as NodeId)
            .find_map(|n| {
                g.links(n)
                    .iter()
                    .find(|l| l.kind == LinkKind::Walk && l.valid() && l.length > 150.0)
                    .map(|l| (n, *l))
            })
            .expect("a long walk link");
        (a, l, (g.node(a).origin + g.node(l.to).origin) * 0.5)
    }

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
        let (a, _, mid) = long_walk(&g);
        let patches = vec![
            Patch::AddNode {
                at: mid.to_array(),
                link: true,
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
            link: true,
            note: String::new(),
        }];
        let (_, report) = apply(g.clone(), &again, &mut world, &mech, Physics::default());
        assert!(
            !report.outcomes[0].ok && report.outcomes[0].message.contains("already there"),
            "{report:?}"
        );
        let air = vec![Patch::AddNode {
            at: [99999.0, 0.0, 0.0],
            link: true,
            note: String::new(),
        }];
        let (_, report) = apply(g, &air, &mut world, &mech, Physics::default());
        assert!(!report.outcomes[0].ok);
    }

    #[test]
    fn a_node_put_in_without_links_gets_only_the_links_after_it() {
        let Some((mut world, mech, g)) = crossfire() else {
            return;
        };
        let (a, _, mid) = long_walk(&g);
        let patches = vec![
            Patch::AddNode {
                at: mid.to_array(),
                link: false,
                note: String::new(),
            },
            Patch::AddLink {
                from: mid.to_array(),
                to: g.node(a).origin.to_array(),
                kind: None,
                both: false,
                trust: false,
                note: String::new(),
            },
        ];
        let (patched, report) = apply(g.clone(), &patches, &mut world, &mech, Physics::default());
        let id = g.len() as NodeId;
        let put = &report.outcomes[0];
        assert!(
            put.ok && put.links.is_empty() && put.message.ends_with("not linked"),
            "{report:?}"
        );
        assert!(report.outcomes[1].ok, "{report:?}");
        assert_eq!(patched.links(id).iter().map(|l| l.to).collect::<Vec<_>>(), vec![a]);
        assert!(
            (0..id).all(|n| patched.links(n).iter().all(|l| l.to != id)),
            "nothing links into it"
        );
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

    fn linked(g: &NavGraph, a: NodeId, b: NodeId) -> bool {
        g.find_link(a, b).is_some() || g.find_link(b, a).is_some()
    }

    #[test]
    fn a_moved_node_keeps_its_number_and_its_links_are_checked_again() {
        let Some((mut world, mech, g)) = crossfire() else {
            return;
        };
        let (a, l, mid) = long_walk(&g);
        let from = g.node(a).origin.to_array();
        let patches = vec![
            Patch::MoveNode {
                from,
                to: mid.to_array(),
                note: String::new(),
            },
            // Patches after it find it where it stands now.
            Patch::RemoveLink {
                from: mid.to_array(),
                to: g.node(l.to).origin.to_array(),
                both: true,
                note: String::new(),
            },
        ];
        let (patched, report) = apply(g.clone(), &patches, &mut world, &mech, Physics::default());
        assert!(report.outcomes[0].ok, "{report:?}");
        assert_eq!(report.outcomes[0].nodes, vec![a]);
        assert_eq!(patched.len(), g.len());
        assert!(patched.node(a).origin.distance(mid) < 40.0, "set down under `to`");
        assert!(!patched.links(a).is_empty(), "links out of it");
        assert!(
            (0..g.len() as NodeId).any(|n| patched.links(n).iter().any(|k| k.to == a)),
            "links into it"
        );
        assert!(report.outcomes[1].ok, "{report:?}");
        assert_eq!(report.outcomes[1].nodes, vec![a, l.to]);
        assert!(!linked(&patched, a, l.to));

        let onto = vec![Patch::MoveNode {
            from,
            to: g.node(l.to).origin.to_array(),
            note: String::new(),
        }];
        let (_, report) = apply(g.clone(), &onto, &mut world, &mech, Physics::default());
        assert!(
            !report.outcomes[0].ok && report.outcomes[0].message.contains("already there"),
            "{report:?}"
        );
        let void = vec![Patch::MoveNode {
            from,
            to: [99999.0, 0.0, 0.0],
            note: String::new(),
        }];
        let (unmoved, report) = apply(g.clone(), &void, &mut world, &mech, Physics::default());
        assert!(!report.outcomes[0].ok, "{report:?}");
        assert_eq!(unmoved.node(a).origin, g.node(a).origin);
        assert_eq!(unmoved.links(a), g.links(a));
    }

    #[test]
    fn nothing_after_a_forbidden_zone_links_into_it() {
        let Some((mut world, mech, g)) = crossfire() else {
            return;
        };
        let (a, l, mid) = long_walk(&g);
        let at = g.node(a).origin;
        let patches = vec![
            Patch::Forbid {
                at: at.to_array(),
                radius: 8.0,
                note: String::new(),
            },
            Patch::AddNode {
                at: mid.to_array(),
                link: true,
                note: String::new(),
            },
            Patch::AddLink {
                from: g.node(l.to).origin.to_array(),
                to: at.to_array(),
                kind: None,
                both: true,
                trust: true,
                note: String::new(),
            },
            Patch::MoveNode {
                from: at.to_array(),
                to: mid.to_array(),
                note: String::new(),
            },
            Patch::AddNode {
                at: (at + Vec3::X * 4.0).to_array(),
                link: true,
                note: String::new(),
            },
        ];
        let (patched, report) = apply(g.clone(), &patches, &mut world, &mech, Physics::default());
        assert!(report.outcomes[0].ok && report.outcomes[1].ok, "{report:?}");
        let id = g.len() as NodeId;
        assert!(!linked(&patched, id, a), "the node put in leaves the shut one alone");
        assert!(patched.links(a).is_empty());
        assert!((0..patched.len() as NodeId).all(|n| patched.links(n).iter().all(|k| k.to != a)));
        for i in [2, 3] {
            assert!(
                !report.outcomes[i].ok && report.outcomes[i].message.contains("shut off"),
                "{report:?}"
            );
        }
        assert!(
            !report.outcomes[4].ok && report.outcomes[4].message.contains("forbidden zone"),
            "{report:?}"
        );
    }

    #[test]
    fn links_taken_out_are_not_put_back_by_a_node_moved() {
        let Some((mut world, mech, g)) = crossfire() else {
            return;
        };
        let (a, l, _) = long_walk(&g);
        let (pa, pb) = (g.node(a).origin, g.node(l.to).origin);
        let step = pa + (pb - pa).normalize() * 24.0;
        let patches = vec![
            Patch::RemoveLink {
                from: pa.to_array(),
                to: pb.to_array(),
                both: true,
                note: String::new(),
            },
            Patch::MoveNode {
                from: pa.to_array(),
                to: step.to_array(),
                note: String::new(),
            },
        ];
        let (patched, report) = apply(g.clone(), &patches, &mut world, &mech, Physics::default());
        assert!(report.outcomes.iter().all(|o| o.ok), "{report:?}");
        assert!(!linked(&patched, a, l.to));
    }
}
