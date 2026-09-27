//! Building the graph over the floor field:
//! - required nodes (spawns, items, ladders, lifts, teleports, drop edges and landings), then placement;
//! - walk links between nodes whose floor areas touch, and longer ones where the graph would otherwise make a bot
//!   go more than 15% out of its way (a greedy spanner);
//! - special links the classifier checks: drops, ladders, lifts, teleports, doors and breakables in the way, and
//!   jumps where walking around is more than twice as long.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::time::Instant;

use lb_bsp::BspWorld;
use lb_bsp::mech::{Mechanisms, MoverKind};
use lb_bsp::world::WorldView;
use lb_core::{Vec2, Vec3};
use lb_kin::Physics;
use lb_kin::validate::{JumpPlan, MoveVerdict, PushRun, plan_jump, simulate_drop, simulate_push, simulate_swim};
use lb_nav::classify::{Classified, Classifier, DROP_SPEED, crouch_origin, is_water, stand_origin};
use lb_nav::graph::{GraphStats, LinkFlags, LinkKind, NavGraph, NavLink, NavNode, NodeFlags, NodeId};
use lb_nav::plan::RUN_SPEED;
use lb_nav::validate::{WalkCheck, walk_straight};
use lb_worldq::{HullKind, TraceQuery, Tracer};
use rayon::prelude::*;

use crate::field::{FloorField, NONE, STEP, SpanFlags, settle};
use crate::place::{Spacing, Spot, borders, place, room, spot_at};
use crate::push::{PushEntry, field_center, push_sites};
use crate::site::{LadderSite, SeedKind, Site, TOP_BELOW, USE_REACH, flood_poses, ladder_ends, rest_poses, sites};

#[derive(Clone, Copy, Debug)]
pub struct GenOptions {
    pub physics: Physics,
    /// Longest walk link considered.
    pub max_link: f32,
    /// A longer walk link is added only where the graph's way is this many times longer.
    pub stretch: f32,
    pub max_out: usize,
    /// Plan jumps across gaps and onto ledges.
    pub jumps: bool,
    pub spacing: Spacing,
}

impl Default for GenOptions {
    fn default() -> GenOptions {
        GenOptions {
            physics: Physics::default(),
            max_link: 320.0,
            stretch: 1.15,
            max_out: 12,
            jumps: true,
            spacing: Spacing::default(),
        }
    }
}

pub struct Generated {
    pub graph: NavGraph,
    pub field: FloorField,
    /// Node each span belongs to.
    pub owner: Vec<u32>,
    /// Milliseconds per stage.
    pub timings: Vec<(&'static str, u128)>,
    /// Jumps planned, and how many of them made it.
    pub jumps: (usize, usize),
    /// Nodes at spawn points and at items.
    pub spawns: Vec<u32>,
    pub items: Vec<u32>,
}

/// Rounds of putting a node between two whose floor areas meet but that cannot walk straight to each other.
const PORTAL_ROUNDS: usize = 4;

/// Drop edges are this far apart at least along a ledge.
const DROP_SPACING: f32 = 96.0;
/// Ladder nodes are this far apart up the ladder.
const LADDER_STEP: f32 = 64.0;
/// Jumps are looked for this far.
const JUMP_REACH: f32 = 256.0;
const JUMP_UP: f32 = 64.0;
const JUMP_DOWN: f32 = 160.0;
/// Jumps tried from one node.
const JUMPS_PER_NODE: usize = 4;
/// Ways into and out of the water are looked for this far from a node in it, a few per node.
const SWIM_REACH: f32 = 192.0;
const SWIMS_PER_NODE: usize = 8;
/// A swimmer climbs out onto floor at most this far above the water's surface.
const CLIMB_OUT: f32 = 48.0;

/// Flights are steered at nodes this far from a field at least (farther where free flights land farther).
const PUSH_RANGE: f32 = 384.0;
/// Nodes near a field tried as flight targets.
const PUSH_TARGETS: usize = 48;

/// Nodes the valid links lead to from `starts`.
fn reachable(out: &[Vec<NavLink>], starts: &[u32]) -> Vec<bool> {
    let mut seen = vec![false; out.len()];
    let mut queue: Vec<u32> = Vec::new();
    for &s in starts {
        if !seen[s as usize] {
            seen[s as usize] = true;
            queue.push(s);
        }
    }
    while let Some(n) = queue.pop() {
        for l in out[n as usize].iter().filter(|l| l.valid()) {
            if !seen[l.to as usize] {
                seen[l.to as usize] = true;
                queue.push(l.to);
            }
        }
    }
    seen
}

/// Required spots in the order they are pushed, to find their nodes after placement.
#[derive(Default)]
struct Required {
    spots: Vec<Spot>,
}

impl Required {
    fn push(&mut self, s: Spot) -> usize {
        self.spots.push(s);
        self.spots.len() - 1
    }
}

/// A ladder's nodes: floor at its foot, the ladder itself bottom to top, the ledge at its top (indices into
/// `Required`).
struct LadderChain {
    foot: Option<usize>,
    chain: Vec<usize>,
    top: Option<usize>,
}

fn ladder_chain(v: &mut WorldView<'_>, field: &FloorField, l: &LadderSite, req: &mut Required) -> LadderChain {
    let ends = ladder_ends(v, l);
    let floor_spot = |p: Vec3| field.at(p).map(|s| spot_at(field, s, NodeFlags::empty()));
    let foot_p = ends
        .iter()
        .copied()
        .filter(|p| p.z - 36.0 <= l.bottom + 48.0)
        .min_by(|a, b| a.z.total_cmp(&b.z));
    let top_p = ends
        .iter()
        .copied()
        .filter(|p| p.z - 36.0 >= l.top - TOP_BELOW && foot_p.is_none_or(|f| p.z > f.z + 32.0))
        .max_by(|a, b| a.z.total_cmp(&b.z));
    let foot = foot_p.and_then(floor_spot).map(|s| req.push(s));
    let top = top_p.and_then(floor_spot).map(|s| req.push(s));
    let start = foot_p.map_or(l.bottom + 36.0, |p| p.z);
    let end = l.top - 8.0;
    let mut chain = Vec::new();
    let mut z = start;
    loop {
        let origin = l.spot.extend(z);
        let on_floor = settle(v, origin).is_some_and(|(feet, _, _)| origin.z - 36.0 - feet < 4.0);
        let mut flags = NodeFlags::LADDER;
        if !on_floor {
            flags |= NodeFlags::AIRBORNE;
        }
        chain.push(req.push(Spot {
            span: NONE,
            origin,
            flags,
            support: 0,
            radius: 16.0,
        }));
        if z >= end {
            break;
        }
        z = (z + LADDER_STEP).min(end);
    }
    LadderChain { foot, chain, top }
}

/// Cells a door side node steps out from the door: clear of its leaf, still in reach of the use key.
const DOOR_STEP_OUT: usize = 1;

/// A node on each side of every door where it rests, a step away from it: the link between the two goes straight
/// through the doorway.
fn door_sides(field: &FloorField, req: &mut Required) {
    let spans = &field.spans;
    let gate = |s: u32| spans[s as usize].flags.contains(SpanFlags::GATE);
    let open = |s: u32| {
        !spans[s as usize]
            .flags
            .intersects(SpanFlags::HAZARD | SpanFlags::GATE | SpanFlags::PUSH | SpanFlags::LADDER)
    };
    let mut seen = vec![false; spans.len()];
    let mut in_rim = vec![false; spans.len()];
    for start in 0..spans.len() as u32 {
        if !gate(start) || seen[start as usize] {
            continue;
        }
        seen[start as usize] = true;
        let mut door = vec![start];
        let mut k = 0;
        while k < door.len() {
            let s = door[k];
            k += 1;
            for (_, t) in spans[s as usize].walk_dirs() {
                if gate(t) && !seen[t as usize] {
                    seen[t as usize] = true;
                    door.push(t);
                }
            }
        }
        let centre = door.iter().map(|&s| spans[s as usize].at).sum::<Vec2>() / door.len() as f32;
        let mut rim: Vec<u32> = door
            .iter()
            .flat_map(|&s| spans[s as usize].walk_dirs().map(|(_, t)| t))
            .filter(|&t| open(t))
            .collect();
        rim.sort_unstable();
        rim.dedup();
        for &r in &rim {
            in_rim[r as usize] = true;
        }
        // The rim falls apart into the door's sides.
        for &first in &rim {
            if !in_rim[first as usize] {
                continue;
            }
            in_rim[first as usize] = false;
            let mut side = vec![first];
            let mut k = 0;
            while k < side.len() {
                let s = side[k];
                k += 1;
                for (_, t) in spans[s as usize].walk_dirs() {
                    if in_rim[t as usize] {
                        in_rim[t as usize] = false;
                        side.push(t);
                    }
                }
            }
            let mean = side.iter().map(|&s| spans[s as usize].at).sum::<Vec2>() / side.len() as f32;
            let Some(mut at) = side.iter().copied().min_by(|&a, &b| {
                let d = |s: u32| (spans[s as usize].at - mean).length();
                d(a).total_cmp(&d(b)).then(a.cmp(&b))
            }) else {
                continue;
            };
            let out = (spans[at as usize].at - centre).normalize_or_zero();
            for _ in 0..DOOR_STEP_OUT {
                let step = spans[at as usize]
                    .walk_dirs()
                    .filter(|&(_, t)| open(t))
                    .map(|(d, t)| {
                        let (dx, dy) = crate::field::DIRS[d];
                        (Vec2::new(dx as f32, dy as f32).normalize().dot(out), t)
                    })
                    .filter(|&(dot, _)| dot > 0.7)
                    .max_by(|x, y| x.0.total_cmp(&y.0).then(y.1.cmp(&x.1)));
                match step {
                    Some((_, t)) => at = t,
                    None => break,
                }
            }
            req.push(spot_at(field, at, NodeFlags::empty()));
        }
    }
}

/// A node on the floor in reach of every button, seeing it: the floor the flood found has room for a player where
/// probing from the button may find none (a button low in a recess).
fn use_spots(world: &BspWorld, mech: &Mechanisms, field: &FloorField, req: &mut Required) {
    let mut v = WorldView::new(world);
    for m in mech
        .movers
        .iter()
        .filter(|m| m.kind == MoverKind::Button && m.health <= 0.0)
    {
        let Some(b) = world.brush(m.model) else { continue };
        let (mins, maxs) = (b.abs_mins(), b.abs_maxs());
        let center = (mins + maxs) * 0.5;
        let mut near: Vec<(f32, u32)> = field
            .spans
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                !s.flags
                    .intersects(SpanFlags::HAZARD | SpanFlags::GATE | SpanFlags::PUSH | SpanFlags::LADDER)
            })
            .map(|(i, s)| {
                let o = s.player_origin();
                ((o.clamp(mins, maxs) - o).length(), i as u32)
            })
            .filter(|&(reach, _)| reach < USE_REACH)
            .collect();
        near.sort_by(|x, y| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1)));
        let seen = near.into_iter().find(|&(_, s)| {
            let span = &field.spans[s as usize];
            let eye = span.player_origin()
                + Vec3::Z
                    * if span.flags.contains(SpanFlags::CROUCH) {
                        12.0
                    } else {
                        28.0
                    };
            let sight = v.trace(&TraceQuery::line(eye, center));
            sight.fraction >= 0.99 || sight.hit == Some(m.model as u32)
        });
        if let Some((_, s)) = seen {
            req.push(spot_at(field, s, NodeFlags::MECHANISM));
        }
    }
}

/// Edges to walk off and where the fall lands, one per stretch of ledge and floor below.
fn drop_pairs(field: &FloorField, req: &mut Required) -> Vec<(usize, usize)> {
    let mut chosen: Vec<(Vec3, f32)> = Vec::new();
    let mut out = Vec::new();
    let mut falls: Vec<_> = field
        .falls
        .iter()
        .filter(|f| {
            let (a, b) = (&field.spans[f.from as usize], &field.spans[f.to as usize]);
            // Not into or out of push fields, nor down a ladder's shaft (the ladder takes a bot down).
            !a.flags
                .intersects(SpanFlags::HAZARD | SpanFlags::GATE | SpanFlags::PUSH | SpanFlags::LADDER)
                && !b
                    .flags
                    .intersects(SpanFlags::HAZARD | SpanFlags::PUSH | SpanFlags::LADDER)
        })
        .copied()
        .collect();
    falls.sort_by(|x, y| {
        x.height
            .total_cmp(&y.height)
            .then(x.from.cmp(&y.from))
            .then(x.dir.cmp(&y.dir))
    });
    for f in falls {
        let edge = field.spans[f.from as usize].origin();
        let land = field.spans[f.to as usize].feet;
        if chosen.iter().any(|(e, l)| {
            (*e - edge).truncate().length() < DROP_SPACING && (e.z - edge.z).abs() < 24.0 && (l - land).abs() < 48.0
        }) {
            continue;
        }
        chosen.push((edge, land));
        let a = req.push(spot_at(field, f.from, NodeFlags::empty()));
        let b = req.push(spot_at(field, f.to, NodeFlags::empty()));
        out.push((a, b));
    }
    out
}

/// Walking from node `a` to node `b`: walked, or blocked (with what blocks it).
fn walkable(v: &mut WorldView<'_>, a: &NavNode, b: &NavNode, ladders: &[(usize, f32)]) -> WalkCheck {
    let off_floor = NodeFlags::LADDER | NodeFlags::AIRBORNE;
    if a.flags.intersects(off_floor)
        || b.flags.intersects(off_floor)
        || through_push(v.world, a.origin, b.origin)
        || past_ladder(v.world, ladders, a.origin, b.origin)
    {
        return WalkCheck::Blocked;
    }
    match (a.flags.contains(NodeFlags::WATER), b.flags.contains(NodeFlags::WATER)) {
        (true, true) => {
            // Ducked through a low tunnel.
            let tr = if a.flags.contains(NodeFlags::CROUCH) || b.flags.contains(NodeFlags::CROUCH) {
                v.trace(&TraceQuery::hull(crouch_origin(a), crouch_origin(b), HullKind::Crouch))
            } else {
                v.trace(&TraceQuery::hull(a.origin, b.origin, HullKind::Stand))
            };
            return if tr.fraction >= 1.0 && !tr.start_solid {
                WalkCheck::Ok
            } else {
                WalkCheck::Blocked
            };
        }
        // Into or out of the water: swum (`swim_links`).
        (true, false) | (false, true) => return WalkCheck::Blocked,
        (false, false) => {}
    }
    if a.flags.contains(NodeFlags::CROUCH) || b.flags.contains(NodeFlags::CROUCH) {
        walk_straight(v, crouch_origin(a), crouch_origin(b), HullKind::Crouch)
    } else {
        walk_straight(v, a.origin, b.origin, HullKind::Stand)
    }
}

/// Nodes whose floor areas meet but that cannot walk straight to each other, where a node on their border would
/// join them (not a ladder or the edge of the water between them).
fn apart(v: &mut WorldView<'_>, a: &NavNode, b: &NavNode, ladders: &[(usize, f32)]) -> bool {
    let off_floor = NodeFlags::LADDER | NodeFlags::AIRBORNE;
    if a.flags.intersects(off_floor)
        || b.flags.intersects(off_floor)
        || a.flags.contains(NodeFlags::WATER) != b.flags.contains(NodeFlags::WATER)
    {
        return false;
    }
    let walks = |r: WalkCheck| matches!(r, WalkCheck::Ok) || matches!(r, WalkCheck::Drop(h) if h <= 20.0);
    !walks(walkable(v, a, b, ladders)) && !walks(walkable(v, b, a, ladders))
}

fn node_of(s: &Spot) -> NavNode {
    NavNode {
        origin: s.origin,
        flags: s.flags,
        radius: s.radius,
        support: s.support,
        first_link: 0,
        link_count: 0,
    }
}

/// A straight move from `a` to `b` passes through a push field (which throws a walker off the way).
fn through_push(world: &BspWorld, a: Vec3, b: Vec3) -> bool {
    if world.pushes.is_empty() {
        return false;
    }
    let steps = (a.distance(b) / 16.0).ceil().max(1.0) as usize;
    (0..=steps).any(|k| world.push_at(a.lerp(b, k as f32 / steps as f32), HullKind::Stand) != Vec3::ZERO)
}

/// A straight move from `a` to `b` brushes past a ladder that goes on down below the floor there: at a ledge's edge
/// a ladder catches a walker and takes it down. `ladders`: (model, bottom).
fn past_ladder(world: &BspWorld, ladders: &[(usize, f32)], a: Vec3, b: Vec3) -> bool {
    if ladders.is_empty() {
        return false;
    }
    let steps = (a.distance(b) / 16.0).ceil().max(1.0) as usize;
    (1..steps).any(|k| {
        let p = a.lerp(b, k as f32 / steps as f32);
        ladders
            .iter()
            .any(|&(m, bottom)| bottom < p.z - 36.0 - STEP && world.hull_overlaps(m, Vec3::ZERO, p, HullKind::Stand))
    })
}

fn walk_kind(a: &NavNode, b: &NavNode) -> LinkKind {
    if a.flags.contains(NodeFlags::WATER) && b.flags.contains(NodeFlags::WATER) {
        LinkKind::Swim
    } else if a.flags.contains(NodeFlags::CROUCH) || b.flags.contains(NodeFlags::CROUCH) {
        LinkKind::Crouch
    } else {
        LinkKind::Walk
    }
}

/// Outgoing links being built, with costs for the spanner's searches.
struct Links {
    out: Vec<Vec<NavLink>>,
}

impl Links {
    fn has(&self, a: u32, b: u32) -> bool {
        self.out[a as usize].iter().any(|l| l.to == b)
    }

    /// Whether the graph gets from `a` to `b` within `bound` seconds.
    fn within(&self, a: u32, b: u32, bound: f32) -> bool {
        let mut best: rustc_hash::FxHashMap<u32, f32> = rustc_hash::FxHashMap::default();
        let mut heap = BinaryHeap::new();
        best.insert(a, 0.0);
        heap.push(Reverse((ordered(0.0), a)));
        while let Some(Reverse((d, n))) = heap.pop() {
            let d = f32::from_bits(d);
            if n == b {
                return true;
            }
            if d > best.get(&n).copied().unwrap_or(f32::INFINITY) {
                continue;
            }
            for l in self.out[n as usize].iter().filter(|l| l.valid()) {
                let nd = d + l.cost;
                if nd <= bound && nd < best.get(&l.to).copied().unwrap_or(f32::INFINITY) {
                    best.insert(l.to, nd);
                    heap.push(Reverse((ordered(nd), l.to)));
                }
            }
        }
        false
    }
}

/// Non-negative floats order like their bits.
fn ordered(x: f32) -> u32 {
    x.max(0.0).to_bits()
}

fn walk_link(nodes: &[NavNode], a: u32, b: u32, kind: LinkKind) -> NavLink {
    let length = nodes[a as usize].origin.distance(nodes[b as usize].origin);
    NavLink {
        to: b as NodeId,
        kind,
        length,
        flags: LinkFlags::VALID,
        cost: length / (RUN_SPEED * kind.speed_factor()),
        spec: lb_nav::graph::NO_SPEC,
    }
}

pub fn generate(world: &mut BspWorld, mech: &Mechanisms, opts: &GenOptions, source: &str) -> Generated {
    let started = Instant::now();
    let traces_before = world.traces;
    let mut timings = Vec::new();
    let mut t = Instant::now();
    let mut lap = |name: &'static str, t: &mut Instant| {
        timings.push((name, t.elapsed().as_millis()));
        *t = Instant::now();
    };

    world.pushes = mech.push_fields();
    let map = sites(world, mech);
    flood_poses(world, mech);
    let pushes = push_sites(world, &opts.physics);
    let mut origins: Vec<Vec3> = map.seeds.iter().map(|s| s.origin).collect();
    origins.extend(pushes.entries.iter().map(|e| e.origin));
    origins.extend(pushes.flights.iter().map(|f| f.landing));
    let field = {
        let site = Site::new(world, mech);
        FloorField::flood(world, &origins, &site)
    };
    rest_poses(world, mech);
    let mut extra_traces = field.traces;
    lap("flood", &mut t);

    // Required nodes.
    let mut req = Required::default();
    let (mut spawn_req, mut item_req) = (Vec::new(), Vec::new());
    for seed in map.seeds.iter().filter(|s| s.kind.mandatory()) {
        if let Some(s) = field.at(seed.origin) {
            let flags = match seed.kind {
                SeedKind::Item => NodeFlags::GOAL,
                SeedKind::LiftBoard | SeedKind::TeleportEntry | SeedKind::UseSpot | SeedKind::TouchSpot => {
                    NodeFlags::MECHANISM
                }
                _ => NodeFlags::empty(),
            };
            let k = req.push(spot_at(&field, s, flags));
            match seed.kind {
                SeedKind::Spawn => spawn_req.push(k),
                SeedKind::Item => item_req.push(k),
                _ => {}
            }
        }
    }
    let teleports: Vec<(usize, usize)> = map
        .teleports
        .iter()
        .filter_map(|&(entry, exit)| {
            let a = field.at(entry)?;
            let b = field.at(exit)?;
            Some((
                req.push(spot_at(&field, a, NodeFlags::MECHANISM)),
                req.push(spot_at(&field, b, NodeFlags::empty())),
            ))
        })
        .collect();
    let entry_req: Vec<Option<usize>> = pushes
        .entries
        .iter()
        .map(|e| {
            field
                .at(e.origin)
                .map(|a| req.push(spot_at(&field, a, NodeFlags::MECHANISM)))
        })
        .collect();
    let flight_req: Vec<(usize, usize, usize, PushRun)> = pushes
        .flights
        .iter()
        .filter_map(|f| {
            let k = pushes
                .entries
                .iter()
                .position(|e| e.model == f.model && e.origin == f.run.entry)?;
            let a = entry_req[k]?;
            let b = field.at(f.landing)?;
            Some((a, req.push(spot_at(&field, b, NodeFlags::empty())), f.model, f.run))
        })
        .collect();
    let ladders: Vec<LadderChain> = {
        let mut v = WorldView::new(world);
        let chains = map
            .ladders
            .iter()
            .map(|l| ladder_chain(&mut v, &field, l, &mut req))
            .collect();
        extra_traces += v.traces;
        chains
    };
    let drops = drop_pairs(&field, &mut req);
    door_sides(&field, &mut req);
    use_spots(world, mech, &field, &mut req);
    let ladder_models: Vec<(usize, f32)> = world
        .brushes
        .iter()
        .filter(|b| b.kind == lb_bsp::world::BrushKind::Volume(lb_worldq::contents::LADDER))
        .map(|b| (b.model, b.abs_mins().z))
        .collect();
    let mut placement = place(&field, &req.spots, opts.spacing);
    // Neighbours that cannot see each other get a node on their border (round a corner, through a doorway).
    for _ in 0..PORTAL_ROUNDS {
        let shared: &BspWorld = world;
        let spots = &placement.spots;
        let found: Vec<(u32, u64)> = borders(&field, &placement.owner)
            .par_iter()
            .filter_map(|&((a, b), span)| {
                let mut v = WorldView::new(shared);
                let apart = apart(
                    &mut v,
                    &node_of(&spots[a as usize]),
                    &node_of(&spots[b as usize]),
                    &ladder_models,
                );
                apart.then_some((span, v.traces))
            })
            .collect();
        extra_traces += found.iter().map(|(_, n)| n).sum::<u64>();
        let before = placement.spots.len();
        for &(span, _) in &found {
            placement.add(spot_at(&field, span, NodeFlags::empty()));
        }
        if placement.spots.len() == before {
            break;
        }
        placement.reassign(&field);
    }
    let id = |k: usize| placement.required[k];
    let nodes: Vec<NavNode> = placement.spots.iter().map(node_of).collect();
    lap("place", &mut t);

    // Walk candidates: nodes whose floor areas touch, then longer ones within reach.
    let owner = &placement.owner;
    let mut touching: Vec<(u32, u32)> = Vec::new();
    for (s, span) in field.spans.iter().enumerate() {
        let a = owner[s];
        if a == NONE {
            continue;
        }
        for (_, j) in span.walk_dirs() {
            let b = owner[j as usize];
            if b != NONE && b != a {
                touching.push((a, b));
            }
        }
    }
    for f in &field.falls {
        let (a, b) = (owner[f.from as usize], owner[f.to as usize]);
        if a != NONE && b != NONE && a != b {
            touching.push((a, b));
        }
    }
    touching.sort_unstable();
    touching.dedup();
    let shared: &BspWorld = world;
    let checked: Vec<(WalkCheck, u64)> = touching
        .par_iter()
        .map(|&(a, b)| {
            let mut v = WorldView::new(shared);
            let r = walkable(&mut v, &nodes[a as usize], &nodes[b as usize], &ladder_models);
            (r, v.traces)
        })
        .collect();
    extra_traces += checked.iter().map(|(_, n)| n).sum::<u64>();
    let mut far: Vec<(u32, u32)> = Vec::new();
    for a in 0..nodes.len() as u32 {
        if nodes[a as usize]
            .flags
            .intersects(NodeFlags::LADDER | NodeFlags::AIRBORNE)
        {
            continue;
        }
        for b in placement.near(nodes[a as usize].origin, opts.max_link) {
            if b != a
                && !nodes[b as usize]
                    .flags
                    .intersects(NodeFlags::LADDER | NodeFlags::AIRBORNE)
            {
                far.push((a, b));
            }
        }
    }
    far.sort_unstable();
    far.dedup();
    far.retain(|p| touching.binary_search(p).is_err());
    lap("walk checks", &mut t);

    // The spanner: shortest first, a link only where the graph so far goes too far round.
    enum Cand {
        Touching(usize),
        Far,
    }
    let mut cands: Vec<(f32, u32, u32, Cand)> = Vec::new();
    for (i, &(a, b)) in touching.iter().enumerate() {
        let d = nodes[a as usize].origin.distance(nodes[b as usize].origin);
        cands.push((d, a, b, Cand::Touching(i)));
    }
    for &(a, b) in &far {
        let d = nodes[a as usize].origin.distance(nodes[b as usize].origin);
        cands.push((d, a, b, Cand::Far));
    }
    cands.sort_by(|x, y| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1)).then(x.2.cmp(&y.2)));
    let mut links = Links {
        out: vec![Vec::new(); nodes.len()],
    };
    let mut blocked: Vec<(u32, u32)> = Vec::new();
    let mut view = WorldView::new(shared);
    for (d, a, b, cand) in cands {
        if links.has(a, b) {
            continue;
        }
        if links.out[a as usize].len() >= opts.max_out {
            // Full for walks; what blocks a touching pair (a door) is still looked at.
            if let Cand::Touching(i) = cand
                && !matches!(checked[i].0, WalkCheck::Ok)
            {
                blocked.push((a, b));
            }
            continue;
        }
        let (na, nb) = (&nodes[a as usize], &nodes[b as usize]);
        let kind = walk_kind(na, nb);
        let cost = d / (RUN_SPEED * kind.speed_factor());
        if links.within(a, b, cost * opts.stretch) {
            continue;
        }
        let result = match cand {
            Cand::Touching(i) => checked[i].0,
            Cand::Far => walkable(&mut view, na, nb, &ladder_models),
        };
        match result {
            WalkCheck::Ok => links.out[a as usize].push(walk_link(&nodes, a, b, kind)),
            WalkCheck::Drop(h) if h <= 20.0 => links.out[a as usize].push(walk_link(&nodes, a, b, kind)),
            _ if matches!(cand, Cand::Touching(_)) => blocked.push((a, b)),
            _ => {}
        }
    }
    extra_traces += view.traces;
    lap("spanner", &mut t);

    // Touching areas the straight check could not join because a door or a breakable is in the way.
    let mut cls = Classifier {
        world,
        mech,
        phys: opts.physics,
        nodes,
        specs: Vec::new(),
    };
    for &(a, b) in &blocked {
        if links.has(a, b) {
            continue;
        }
        let (na, nb) = (cls.nodes[a as usize], cls.nodes[b as usize]);
        let special = cls.blocker(&na, &nb).and_then(|m| {
            cls.door_link(&na, &nb, m)
                .filter(|c| c.valid)
                .or_else(|| cls.breakable_link(&na, &nb, m))
        });
        if let Some(c) = special {
            let link = cls.link(a as usize, b as usize, c, LinkFlags::empty());
            links.out[a as usize].push(link);
        }
    }
    lap("doors", &mut t);

    // Into and out of the water: swum there the way a bot swims, up to the surface and over the edge.
    {
        let water = |n: &NavNode| n.flags.contains(NodeFlags::WATER);
        // Land, or a ladder: some rise out of the water.
        let land = |n: &NavNode| {
            !n.flags.contains(NodeFlags::WATER)
                && (n.flags.contains(NodeFlags::LADDER) || !n.flags.contains(NodeFlags::AIRBORNE))
        };
        let mut pairs: Vec<(u32, u32)> = touching
            .iter()
            .copied()
            .filter(|&(a, b)| water(&cls.nodes[a as usize]) != water(&cls.nodes[b as usize]))
            .collect();
        let mut v = WorldView::new(&*cls.world);
        for w in 0..cls.nodes.len() as u32 {
            let nw = cls.nodes[w as usize];
            if !water(&nw) {
                continue;
            }
            // Out of the water only onto what a swimmer at the surface can climb.
            let mut surface = nw.origin.z;
            while surface < nw.origin.z + 1024.0 && is_water(v.point_contents(nw.origin.with_z(surface + 8.0))) {
                surface += 8.0;
            }
            let mut near: Vec<(f32, u32)> = placement
                .around(nw.origin, SWIM_REACH)
                .filter(|&l| land(&cls.nodes[l as usize]) && cls.nodes[l as usize].origin.z > nw.origin.z - STEP)
                .map(|l| (cls.nodes[l as usize].origin.distance(nw.origin), l))
                .collect();
            near.sort_by(|x, y| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1)));
            for &(_, l) in near.iter().take(SWIMS_PER_NODE) {
                pairs.push((l, w));
            }
            let climbable = |&&(_, l): &&(f32, u32)| cls.nodes[l as usize].origin.z - 36.0 <= surface + CLIMB_OUT;
            for &(_, l) in near.iter().filter(climbable).take(SWIMS_PER_NODE) {
                pairs.push((w, l));
            }
        }
        extra_traces += v.traces;
        pairs.sort_unstable();
        pairs.dedup();
        pairs.retain(|&(a, b)| !links.has(a, b));
        let swims: Vec<bool> = {
            let (w, nodes, phys) = (&*cls.world, &cls.nodes, opts.physics);
            pairs
                .par_iter()
                .map(|&(a, b)| {
                    let mut v = WorldView::new(w);
                    simulate_swim(&mut v, &phys, nodes[a as usize].origin, nodes[b as usize].origin).ok
                })
                .collect()
        };
        for (&(a, b), ok) in pairs.iter().zip(swims) {
            if ok {
                let c = cls.swim_link(&cls.nodes[a as usize], &cls.nodes[b as usize]);
                let link = cls.link(a as usize, b as usize, c, LinkFlags::empty());
                links.out[a as usize].push(link);
            }
        }
    }

    // Drops, ladders, teleports, lifts.
    let mut drop_stats = (0, 0, 0);
    for &(e, l) in &drops {
        let (a, b) = (id(e) as usize, id(l) as usize);
        if a == b || links.has(a as u32, b as u32) {
            drop_stats.0 += 1;
            continue;
        }
        let (na, nb) = (cls.nodes[a], cls.nodes[b]);
        match cls.drop_link(&na, &nb) {
            Some(c) => {
                drop_stats.1 += 1;
                let link = cls.link(a, b, c, LinkFlags::empty());
                links.out[a].push(link);
            }
            None => drop_stats.2 += 1,
        }
    }
    let _ = drop_stats;
    for lad in &ladders {
        let mut chain: Vec<usize> = lad.chain.iter().map(|&k| id(k) as usize).collect();
        chain.dedup();
        let mut path: Vec<usize> = Vec::new();
        path.extend(lad.foot.map(|k| id(k) as usize));
        path.extend(chain);
        path.extend(lad.top.map(|k| id(k) as usize));
        path.dedup();
        for w in path.windows(2) {
            for (a, b) in [(w[0], w[1]), (w[1], w[0])] {
                if links.has(a as u32, b as u32) {
                    continue;
                }
                let c = cls.classify(a, b, false);
                if c.valid {
                    let link = cls.link(a, b, c, LinkFlags::empty());
                    links.out[a].push(link);
                }
            }
        }
    }
    for &(e, x) in &teleports {
        let (a, b) = (id(e) as usize, id(x) as usize);
        let (na, nb) = (cls.nodes[a], cls.nodes[b]);
        if a != b
            && let Some(c) = cls.teleport_link(&na, &nb)
        {
            let link = cls.link(a, b, c, LinkFlags::empty());
            links.out[a].push(link);
        }
    }
    let lifts = cls.lift_links(&mut links.out);
    lap("specials", &mut t);

    // Jumps where walking round is more than twice as far, planned on every thread.
    let mut jumps = (0, 0);
    if opts.jumps {
        let mut cands: Vec<(u32, u32)> = Vec::new();
        for a in 0..cls.nodes.len() as u32 {
            let na = cls.nodes[a as usize];
            let span = placement.spots[a as usize].span;
            let unfit = NodeFlags::LADDER | NodeFlags::AIRBORNE | NodeFlags::WATER;
            if na.flags.intersects(unfit | NodeFlags::CROUCH) || span == NONE || room(&field, span) > 48.0 {
                continue;
            }
            let mut near: Vec<(f32, u32)> = placement
                .around(na.origin, JUMP_REACH)
                .filter(|&b| b != a)
                .filter_map(|b| {
                    let nb = &cls.nodes[b as usize];
                    let dz = nb.origin.z - na.origin.z;
                    let d = (nb.origin - na.origin).truncate().length();
                    (!nb.flags.intersects(unfit) && (-JUMP_DOWN..=JUMP_UP).contains(&dz) && d >= 32.0).then_some((d, b))
                })
                .collect();
            near.sort_by(|x, y| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1)));
            let mut taken = 0;
            for (d, b) in near {
                if taken >= JUMPS_PER_NODE {
                    break;
                }
                if links.has(a, b) || links.within(a, b, 2.0 * d / RUN_SPEED) {
                    continue;
                }
                cands.push((a, b));
                taken += 1;
            }
        }
        // Up onto the ledges the floor field found, from wherever the nodes below are.
        let mut ledges: Vec<(u32, u32)> = field
            .ledges
            .iter()
            .map(|l| (placement.owner[l.from as usize], placement.owner[l.to as usize]))
            .filter(|&(a, b)| a != NONE && b != NONE && a != b)
            .collect();
        ledges.sort_unstable();
        ledges.dedup();
        for (a, b) in ledges {
            let (na, nb) = (&cls.nodes[a as usize], &cls.nodes[b as usize]);
            let d = (nb.origin - na.origin).truncate().length();
            let unfit = NodeFlags::LADDER | NodeFlags::AIRBORNE | NodeFlags::WATER;
            if na.flags.intersects(unfit | NodeFlags::CROUCH)
                || nb.flags.intersects(unfit)
                || d > JUMP_REACH
                || cands.contains(&(a, b))
                || links.has(a, b)
                || links.within(a, b, 2.0 * d.max(32.0) / RUN_SPEED)
            {
                continue;
            }
            cands.push((a, b));
        }
        // Down first as a walk off the edge, which is surer than a jump; jumps for the rest.
        let drops: Vec<Option<MoveVerdict>> = {
            let (w, nodes, phys) = (&*cls.world, &cls.nodes, opts.physics);
            cands
                .par_iter()
                .map(|&(a, b)| {
                    let (na, nb) = (&nodes[a as usize], &nodes[b as usize]);
                    (nb.origin.z < na.origin.z - STEP).then(|| {
                        let mut v = WorldView::new(w);
                        simulate_drop(&mut v, &phys, stand_origin(na), stand_origin(nb), DROP_SPEED)
                    })
                })
                .collect()
        };
        let mut rest = Vec::new();
        for (&(a, b), verdict) in cands.iter().zip(drops) {
            let (na, nb) = (cls.nodes[a as usize], cls.nodes[b as usize]);
            match verdict.and_then(|v| cls.drop_from(&na, &nb, &v)) {
                Some(c) => {
                    let link = cls.link(a as usize, b as usize, c, LinkFlags::empty());
                    links.out[a as usize].push(link);
                }
                None => rest.push((a, b)),
            }
        }
        jumps.0 = rest.len();
        let plans: Vec<Option<JumpPlan>> = {
            let (w, nodes, phys) = (&*cls.world, &cls.nodes, opts.physics);
            rest.par_iter()
                .map(|&(a, b)| {
                    let mut v = WorldView::new(w);
                    plan_jump(
                        &mut v,
                        &phys,
                        stand_origin(&nodes[a as usize]),
                        stand_origin(&nodes[b as usize]),
                    )
                })
                .collect()
        };
        for (&(a, b), plan) in rest.iter().zip(plans) {
            let Some(plan) = plan else { continue };
            jumps.1 += 1;
            let (na, nb) = (cls.nodes[a as usize], cls.nodes[b as usize]);
            let c: Classified = cls.jump_from_plan(&na, &nb, &plan);
            let link = cls.link(a as usize, b as usize, c, LinkFlags::empty());
            links.out[a as usize].push(link);
        }
    }
    lap("jumps", &mut t);

    // Push fields: the flights found, steered onto their landing nodes; then flights steered at nodes near each
    // field that its entries cannot get to otherwise, nearest first, until they can.
    let push_run = |b: usize, run: PushRun, cls: &Classifier<'_>| {
        let mut v = WorldView::new(&*cls.world);
        let target = Some(stand_origin(&cls.nodes[b]));
        simulate_push(&mut v, &opts.physics, &PushRun { target, ..run })
    };
    {
        let mut runs: Vec<(usize, usize, usize, PushRun)> = flight_req
            .iter()
            .map(|&(e, l, model, run)| (id(e) as usize, id(l) as usize, model, run))
            .filter(|&(a, b, ..)| a != b)
            .collect();
        runs.sort_by_key(|&(a, b, model, _)| (a, b, model));
        runs.dedup_by_key(|&mut (a, b, ..)| (a, b));
        let verdicts: Vec<MoveVerdict> = runs.par_iter().map(|&(_, b, _, run)| push_run(b, run, &cls)).collect();
        for (&(a, b, model, run), v) in runs.iter().zip(verdicts) {
            if links.has(a as u32, b as u32) {
                continue;
            }
            let (na, nb) = (cls.nodes[a], cls.nodes[b]);
            if let Some(c) = cls.push_from(&na, &nb, model, &run, &v) {
                let link = cls.link(a, b, c, LinkFlags::empty());
                links.out[a].push(link);
            }
        }
    }
    let mut models: Vec<usize> = pushes.entries.iter().map(|e| e.model).collect();
    models.sort_unstable();
    models.dedup();
    for model in models {
        let Some(center) = field_center(cls.world, model) else {
            continue;
        };
        let entries: Vec<(usize, &PushEntry)> = pushes
            .entries
            .iter()
            .zip(&entry_req)
            .filter(|(e, _)| e.model == model)
            .filter_map(|(e, k)| k.map(|k| (id(k) as usize, e)))
            .collect();
        let range = entries.iter().map(|(_, e)| e.reach).fold(PUSH_RANGE, f32::max) + 128.0;
        let starts: Vec<u32> = entries.iter().map(|&(n, _)| n as u32).collect();
        let mut reached = reachable(&links.out, &starts);
        let mut targets: Vec<(f32, u32)> = (0..cls.nodes.len() as u32)
            .filter(|&t| !reached[t as usize])
            .filter_map(|t| {
                let n = &cls.nodes[t as usize];
                let d = (n.origin - center).truncate().length();
                (d <= range && !n.flags.intersects(NodeFlags::LADDER | NodeFlags::AIRBORNE)).then_some((d, t))
            })
            .collect();
        targets.sort_by(|x, y| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1)));
        targets.truncate(PUSH_TARGETS);
        for (_, t) in targets {
            if reached[t as usize] {
                continue;
            }
            let goal = stand_origin(&cls.nodes[t as usize]);
            let tries: Vec<(usize, PushRun)> = entries
                .iter()
                .filter(|(_, e)| {
                    let to = (goal - e.origin).truncate();
                    to.dot(e.dir) >= 0.0 && (e.reach == 0.0 || to.length() <= e.reach + 128.0)
                })
                .flat_map(|&(n, e)| e.runs().map(move |r| (n, r)))
                .collect();
            let verdicts: Vec<MoveVerdict> = tries
                .par_iter()
                .map(|&(_, run)| push_run(t as usize, run, &cls))
                .collect();
            let Some((&(a, run), v)) = tries.iter().zip(&verdicts).find(|(_, v)| v.ok) else {
                continue;
            };
            let (na, nb) = (cls.nodes[a], cls.nodes[t as usize]);
            if let Some(c) = cls.push_from(&na, &nb, model, &run, v) {
                let link = cls.link(a, t as usize, c, LinkFlags::empty());
                links.out[a].push(link);
                reached = reachable(&links.out, &starts);
            }
        }
    }
    lap("pushes", &mut t);

    let probes = cls.probes(&links.out);
    let traces = cls.world.traces - traces_before + extra_traces;
    let Classifier { nodes, specs, .. } = cls;
    let stats = GraphStats {
        added: lifts,
        traces,
        millis: started.elapsed().as_millis(),
        ..Default::default()
    };
    let mut graph = NavGraph::from_parts(nodes, links.out, specs, source, stats);
    graph.probes = probes;
    lap("assemble", &mut t);
    let spawns = spawn_req.iter().map(|&k| placement.required[k]).collect();
    let items = item_req.iter().map(|&k| placement.required[k]).collect();
    Generated {
        graph,
        field,
        owner: placement.owner,
        timings,
        jumps,
        spawns,
        items,
    }
}
