//! Node placement over the floor field: spots that must have a node first (spawns, items, ladder ends, lift
//! platforms, teleports, drop edges and landings), then corridor centres outward, keeping nodes apart by a gap
//! that grows with the room around them. Every span then belongs to the node nearest to it on foot; where two
//! nodes whose floor areas meet cannot walk straight to each other, the generator adds a node on their border
//! ([`borders`]).

use std::collections::VecDeque;

use lb_core::Vec3;
use lb_nav::NodeFlags;
use rustc_hash::FxHashMap;
use smallvec::SmallVec;

use crate::field::{CELL, FloorField, NONE, Span, SpanFlags};

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

/// Strips of floor at most this many cells across get the nodes found at their edges on their middle line.
pub const NARROW: usize = 6;

/// The span on the middle line of the narrow strip of floor span `s` is on, where the strip has a drop on either side
/// (a ramp, a beam, the top of a wall; `drops(i, d)`: off span `i` in direction `d` the floor falls away): the middle
/// of the strip across it through `s` along x or y, whichever is narrower (both on a small platform). Of two middle
/// cells, the one no drop starts from, else the one on the negative side, so that a strip's nodes line up whichever
/// of its edges they were found at. `s` itself where the strip is wider than `NARROW` cells, or has a wall or more
/// floor on a side. Even a low drop counts: a bot that slips off a strip between two trenches has lost its way.
pub fn strip_middle(spans: &[Span], s: u32, drops: impl Fn(u32, usize) -> bool) -> u32 {
    let drops_from = |i: u32| (0..8).any(|d| drops(i, d));
    let middle = |line: &[u32]| {
        let mid = (line.len() - 1) / 2;
        if line.len().is_multiple_of(2) && drops_from(line[mid]) && !drops_from(line[mid + 1]) {
            line[mid + 1]
        } else {
            line[mid]
        }
    };
    // Off both ends of the line across, a drop: straight out, or at either slant.
    let both_sides = |line: &[u32], axis: usize| {
        let off = |i: u32, d: usize| [d, (d + 1) % 8, (d + 7) % 8].into_iter().any(|d| drops(i, d));
        off(line[0], axis + 4) && off(line[line.len() - 1], axis)
    };
    // Two cells across, a strip has no middle line: either is at an edge.
    let across = |s: u32, axis: usize| {
        let line = across(spans, s, axis);
        if line.len() >= 3 && both_sides(&line, axis) {
            line
        } else {
            SmallVec::new()
        }
    };
    let (x, y) = (across(s, 0), across(s, 2));
    let (first, other_axis) = match (x.is_empty(), y.is_empty()) {
        (true, true) => return s,
        (false, true) => return middle(&x),
        (true, false) => return middle(&y),
        (false, false) if x.len() <= y.len() => (middle(&x), 2),
        (false, false) => (middle(&y), 0),
    };
    let line = across(first, other_axis);
    if line.is_empty() { first } else { middle(&line) }
}

/// The spans across the strip `s` is on along `axis` (0 for x, 2 for y), from its negative side; none where it is
/// wider than `NARROW` cells.
fn across(spans: &[Span], s: u32, axis: usize) -> SmallVec<[u32; 16]> {
    let run = |d: usize| {
        let mut out: SmallVec<[u32; NARROW]> = SmallVec::new();
        let mut i = s;
        while out.len() < NARROW {
            let n = spans[i as usize].next[d];
            if n == NONE {
                break;
            }
            out.push(n);
            i = n;
        }
        out
    };
    let (back, ahead) = (run(axis + 4), run(axis));
    if back.len() + 1 + ahead.len() > NARROW {
        return SmallVec::new();
    }
    back.iter().rev().copied().chain([s]).chain(ahead).collect()
}

/// How far apart the nodes that fill the floor are: twice the room around them, within these bounds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spacing {
    pub min: f32,
    pub max: f32,
}

impl Default for Spacing {
    fn default() -> Spacing {
        Spacing { min: 112.0, max: 224.0 }
    }
}

/// Places nodes: `required` first, then the rest of the field.
pub fn place(field: &FloorField, required: &[Spot], spacing: Spacing) -> Placement {
    let mut p = Placement::new();
    for &spot in required {
        let id = p.add(spot);
        p.required.push(id);
    }
    let mut order: Vec<u32> = (0..field.len() as u32)
        .filter(|&i| !field.spans[i as usize].flags.intersects(NO_NODE))
        .collect();
    order.sort_by_key(|&i| (std::cmp::Reverse(field.spans[i as usize].edge), i));
    for s in order {
        let origin = field.spans[s as usize].player_origin();
        let r = (room(field, s) * 2.0).clamp(spacing.min, spacing.max);
        if p.near(origin, r).next().is_none() {
            p.add(spot_at(field, s, NodeFlags::empty()));
        }
    }
    p.reassign(field);
    p
}

impl Placement {
    /// Gives every span to the node nearest to it on foot again (after nodes were added).
    pub fn reassign(&mut self, field: &FloorField) {
        self.owner = owners(field, &self.spots);
    }
}

/// Spans no node goes on: inside a door, a hazard, a push field or a ladder.
const NO_NODE: SpanFlags = SpanFlags::HAZARD
    .union(SpanFlags::GATE)
    .union(SpanFlags::PUSH)
    .union(SpanFlags::LADDER);

/// Every pair of nodes whose floor areas meet on foot (`a < b`), with the span on their border with the most room
/// around it: where a node between the two would go.
pub fn borders(field: &FloorField, owner: &[u32]) -> Vec<((u32, u32), u32)> {
    let mut best: FxHashMap<(u32, u32), (u16, u32)> = FxHashMap::default();
    for (s, span) in field.spans.iter().enumerate() {
        let a = owner[s];
        if a == NONE || span.flags.intersects(NO_NODE) {
            continue;
        }
        for (_, t) in span.walk_dirs() {
            let b = owner[t as usize];
            if b == NONE || b == a || field.spans[t as usize].flags.intersects(NO_NODE) {
                continue;
            }
            let key = (a.min(b), a.max(b));
            let cand = (span.edge, s as u32);
            best.entry(key)
                .and_modify(|e| {
                    if (cand.0, std::cmp::Reverse(cand.1)) > (e.0, std::cmp::Reverse(e.1)) {
                        *e = cand;
                    }
                })
                .or_insert(cand);
        }
    }
    let mut out: Vec<((u32, u32), u32)> = best.into_iter().map(|(k, (_, s))| (k, s)).collect();
    out.sort_unstable();
    out
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

#[cfg(test)]
mod tests {
    use lb_core::Vec2;

    use super::*;
    use crate::field::{Cell, DIRS};

    /// A flat floor over the cells, every one walkable to its neighbours among them.
    fn floor(w: i32, h: i32) -> Vec<Span> {
        let cells: Vec<(i32, i32)> = (0..w).flat_map(|x| (0..h).map(move |y| (x, y))).collect();
        let index: FxHashMap<(i32, i32), u32> = cells.iter().enumerate().map(|(i, &c)| (c, i as u32)).collect();
        cells
            .iter()
            .map(|&(x, y)| {
                let mut next = [NONE; 8];
                for (d, (dx, dy)) in DIRS.iter().enumerate() {
                    next[d] = index.get(&(x + dx, y + dy)).copied().unwrap_or(NONE);
                }
                Span {
                    cell: Cell { x, y },
                    at: Vec2::new(x as f32 * CELL + 8.0, y as f32 * CELL + 8.0),
                    feet: 0.0,
                    flags: SpanFlags::empty(),
                    support: 0,
                    next,
                    edge: 0,
                }
            })
            .collect()
    }

    fn at(spans: &[Span], x: i32, y: i32) -> u32 {
        spans.iter().position(|s| s.cell == Cell { x, y }).unwrap() as u32
    }

    fn middle(spans: &[Span], x: i32, y: i32, drops: impl Fn(Cell, usize) -> bool) -> (i32, i32) {
        let c = spans[strip_middle(spans, at(spans, x, y), |s, d| drops(spans[s as usize].cell, d)) as usize].cell;
        (c.x, c.y)
    }

    /// Drops off every edge of a `w` by `h` floor.
    fn off_every_edge(w: i32, h: i32) -> impl Fn(Cell, usize) -> bool {
        move |c, d| {
            let (dx, dy) = DIRS[d];
            !(0..w).contains(&(c.x + dx)) || !(0..h).contains(&(c.y + dy))
        }
    }

    #[test]
    fn edge_nodes_of_a_strip_with_drops_on_both_sides_line_up_on_its_middle() {
        let spans = floor(12, 4);
        let drops = off_every_edge(12, 4);
        assert_eq!(middle(&spans, 5, 0, &drops), (5, 1));
        assert_eq!(
            middle(&spans, 5, 3, &drops),
            (5, 1),
            "the same line from the other edge"
        );
        assert_eq!(
            middle(&spans, 11, 3, &drops),
            (11, 1),
            "a strip's end: across it, not along"
        );
        let beam = floor(12, 3);
        assert_eq!(middle(&beam, 4, 2, off_every_edge(12, 3)), (4, 1));
        let pipe = floor(12, 2);
        assert_eq!(
            middle(&pipe, 4, 1, off_every_edge(12, 2)),
            (4, 1),
            "no middle to two cells"
        );
    }

    #[test]
    fn a_ledge_along_a_wall_keeps_its_edge_node() {
        let spans = floor(12, 3);
        let off_the_top = |c: Cell, d: usize| c.y == 2 && matches!(d, 1..=3);
        assert_eq!(middle(&spans, 5, 2, off_the_top), (5, 2));
    }

    #[test]
    fn a_small_platform_has_it_in_the_middle_and_a_wide_floor_where_it_was() {
        assert_eq!(middle(&floor(5, 5), 0, 0, off_every_edge(5, 5)), (2, 2));
        assert_eq!(middle(&floor(12, 12), 5, 0, off_every_edge(12, 12)), (5, 0));
    }
}
