//! Node placement over the floor field: spots that must have a node first (spawns, items, ladder ends, lift
//! platforms, teleports, drop edges and landings), then corridor centres outward, keeping nodes apart by a radius
//! that grows with the room around them. Every span then belongs to the node nearest to it on foot.

use std::collections::VecDeque;

use lb_core::Vec3;
use lb_nav::NodeFlags;
use rustc_hash::FxHashMap;

use crate::field::{CELL, FloorField, NONE, SpanFlags};

/// Nodes closer than this (and on the same floor) are one.
const MERGE: f32 = 24.0;
/// Nodes on floors this far apart vertically do not crowd each other.
const SAME_LEVEL: f32 = 48.0;

/// A node before it goes into the graph.
#[derive(Clone, Copy, Debug)]
pub struct Spot {
    /// The span it stands on; `NONE` for nodes off the floor (on a ladder).
    pub span: u32,
    /// Player origin: the standing hull centre, the crouching one for crouch nodes.
    pub origin: Vec3,
    pub flags: NodeFlags,
    pub support: u16,
    /// Reach tolerance.
    pub radius: f32,
}

/// Room around a span in units: its distance to the nearest spot that cannot be walked on.
pub fn room(field: &FloorField, span: u32) -> f32 {
    f32::from(field.spans[span as usize].edge) * CELL + CELL * 0.5
}

pub struct Placement {
    pub spots: Vec<Spot>,
    /// Node each span belongs to (`NONE` for spans no node reaches on foot).
    pub owner: Vec<u32>,
    /// Node of each required spot, in the order given.
    pub required: Vec<u32>,
    grid: FxHashMap<(i32, i32), Vec<u32>>,
}

const BUCKET: f32 = 128.0;

fn bucket(p: Vec3) -> (i32, i32) {
    ((p.x / BUCKET).floor() as i32, (p.y / BUCKET).floor() as i32)
}

impl Placement {
    fn new() -> Placement {
        Placement {
            spots: Vec::new(),
            owner: Vec::new(),
            required: Vec::new(),
            grid: FxHashMap::default(),
        }
    }

    /// Nodes within `r` of `p` horizontally, at any height.
    pub fn around(&self, p: Vec3, r: f32) -> impl Iterator<Item = u32> + '_ {
        let (bx, by) = bucket(p);
        let reach = (r / BUCKET).ceil() as i32;
        (-reach..=reach)
            .flat_map(move |dx| (-reach..=reach).map(move |dy| (bx + dx, by + dy)))
            .filter_map(|b| self.grid.get(&b))
            .flatten()
            .copied()
            .filter(move |&i| (self.spots[i as usize].origin - p).truncate().length() < r)
    }

    /// Nodes within `r` of `p` horizontally, on about the same level.
    pub fn near(&self, p: Vec3, r: f32) -> impl Iterator<Item = u32> + '_ {
        let (bx, by) = bucket(p);
        let reach = (r / BUCKET).ceil() as i32;
        (-reach..=reach)
            .flat_map(move |dx| (-reach..=reach).map(move |dy| (bx + dx, by + dy)))
            .filter_map(|b| self.grid.get(&b))
            .flatten()
            .copied()
            .filter(move |&i| {
                let o = self.spots[i as usize].origin;
                (o - p).truncate().length() < r && (o.z - p.z).abs() < SAME_LEVEL
            })
    }

    /// Adds a node, or returns one already this close.
    pub fn add(&mut self, spot: Spot) -> u32 {
        let merge = if spot.flags.intersects(NodeFlags::LADDER | NodeFlags::AIRBORNE) {
            8.0
        } else {
            MERGE
        };
        let same = self.near(spot.origin, merge).find(|&i| {
            let s = &self.spots[i as usize];
            (s.origin.z - spot.origin.z).abs() < 18.0
                && s.flags.contains(NodeFlags::LADDER) == spot.flags.contains(NodeFlags::LADDER)
        });
        if let Some(i) = same {
            self.spots[i as usize].flags |= spot.flags & (NodeFlags::GOAL | NodeFlags::MECHANISM);
            return i;
        }
        let i = self.spots.len() as u32;
        self.grid.entry(bucket(spot.origin)).or_default().push(i);
        self.spots.push(spot);
        i
    }
}

/// A node at span `s` of the field.
pub fn spot_at(field: &FloorField, s: u32, extra: NodeFlags) -> Spot {
    let span = &field.spans[s as usize];
    let mut flags = extra;
    if span.flags.contains(SpanFlags::CROUCH) {
        flags |= NodeFlags::CROUCH;
    }
    if span.flags.contains(SpanFlags::WATER) {
        flags |= NodeFlags::WATER;
    }
    if span.support != 0 {
        flags |= NodeFlags::ON_MOVER;
    }
    Spot {
        span: s,
        origin: span.player_origin(),
        flags,
        support: span.support,
        radius: (room(field, s) - 16.0).clamp(8.0, 64.0),
    }
}

/// Places nodes: `required` first, then the rest of the field.
pub fn place(field: &FloorField, required: &[Spot]) -> Placement {
    let mut p = Placement::new();
    for &spot in required {
        let id = p.add(spot);
        p.required.push(id);
    }
    let mut order: Vec<u32> = (0..field.len() as u32)
        .filter(|&i| {
            !field.spans[i as usize]
                .flags
                .intersects(SpanFlags::HAZARD | SpanFlags::GATE | SpanFlags::PUSH | SpanFlags::LADDER)
        })
        .collect();
    order.sort_by_key(|&i| (std::cmp::Reverse(field.spans[i as usize].edge), i));
    for s in order {
        let origin = field.spans[s as usize].player_origin();
        let r = (room(field, s) * 1.2).clamp(48.0, 128.0);
        if p.near(origin, r).next().is_none() {
            p.add(spot_at(field, s, NodeFlags::empty()));
        }
    }
    p.owner = owners(field, &p.spots);
    p
}

/// The node each span belongs to: the nearest on foot, found by one walk outward from every node at once.
fn owners(field: &FloorField, spots: &[Spot]) -> Vec<u32> {
    let mut owner = vec![NONE; field.len()];
    let mut queue = VecDeque::new();
    for (i, s) in spots.iter().enumerate() {
        if s.span != NONE && owner[s.span as usize] == NONE {
            owner[s.span as usize] = i as u32;
            queue.push_back(s.span);
        }
    }
    while let Some(s) = queue.pop_front() {
        let o = owner[s as usize];
        for (_, t) in field.spans[s as usize].walk_dirs() {
            if owner[t as usize] == NONE {
                owner[t as usize] = o;
                queue.push_back(t);
            }
        }
    }
    owner
}
