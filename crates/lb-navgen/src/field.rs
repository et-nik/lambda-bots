//! The floor field: every spot of floor a player can reach on foot, on a 16-unit grid, found by walking from where
//! players start the way the engine moves them (`PM_WalkMove`: straight, else a step up and over, then down onto
//! the floor).
//!
//! A column of the grid holds a span for each floor in it (a room above a room). Spans know which neighbours can be
//! walked to and where a player falls when walking off an edge. Spans only a crouching player fits in are marked.

use lb_bsp::world::WorldView;
use lb_core::{Vec2, Vec3};
use lb_worldq::{HullKind, TraceQuery, Tracer, contents};
use rayon::prelude::*;
use rustc_hash::FxHashMap;
use smallvec::SmallVec;

pub const CELL: f32 = 16.0;
/// `sv_stepsize`.
pub const STEP: f32 = 18.0;
/// Floors of a column closer than this are the same floor.
const SAME_FLOOR: f32 = 8.0;
/// Falls deeper than this are not followed (a pit with no floor, the sky).
const MAX_FALL: f32 = 2048.0;
/// Walkable ground: `PM_CheckWaterJump`/`PM_CategorizePosition` count normals up to this steep as floor.
const MIN_NORMAL_Z: f32 = 0.7;

/// The eight directions to the neighbouring cells, counter-clockwise from +x.
pub const DIRS: [(i32, i32); 8] = [(1, 0), (1, 1), (0, 1), (-1, 1), (-1, 0), (-1, -1), (0, -1), (1, -1)];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Cell {
    pub x: i32,
    pub y: i32,
}

impl Cell {
    pub fn of(p: Vec3) -> Cell {
        Cell {
            x: (p.x / CELL).floor() as i32,
            y: (p.y / CELL).floor() as i32,
        }
    }

    pub fn center(self) -> Vec2 {
        Vec2::new((self.x as f32 + 0.5) * CELL, (self.y as f32 + 0.5) * CELL)
    }

    pub fn step(self, dir: usize) -> Cell {
        let (dx, dy) = DIRS[dir];
        Cell {
            x: self.x + dx,
            y: self.y + dy,
        }
    }
}

bitflags::bitflags! {
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct SpanFlags: u8 {
        /// Only a crouching player fits.
        const CROUCH = 1 << 0;
        /// Waist deep in water or deeper.
        const WATER = 1 << 1;
        /// Lava, slime or a damaging trigger: not a place to stand.
        const HAZARD = 1 << 2;
        /// Inside a door or other mover where it rests: passable only when it opens.
        const GATE = 1 << 3;
        /// In a push field: a player is thrown from here, not walking on (push links).
        const PUSH = 1 << 4;
        /// Touching a ladder: a player standing here is on it (at the top edge it slides down).
        const LADDER = 1 << 5;
    }
}

#[derive(Clone, Debug)]
pub struct Span {
    pub cell: Cell,
    /// Where in the cell the player stands: its centre, or the exact spot of a seed (a cell's centre can be inside
    /// the wall a ladder or a spawn point is against).
    pub at: Vec2,
    /// Height of the floor under the player's feet.
    pub feet: f32,
    pub flags: SpanFlags,
    /// Brush model under the feet when it is a mover (a lift platform), else 0.
    pub support: u16,
    /// Neighbour span walked to in each direction, `u32::MAX` when none.
    pub next: [u32; 8],
    /// Distance in cells to the nearest spot a player cannot walk on (filled by `edge_distance`).
    pub edge: u16,
}

pub const NONE: u32 = u32::MAX;

impl Span {
    /// Centre of the standing hull at the span (for crouch spans, where it would be).
    pub fn origin(&self) -> Vec3 {
        self.at.extend(self.feet + 36.0)
    }

    pub fn crouch_origin(&self) -> Vec3 {
        self.at.extend(self.feet + 18.0)
    }

    /// Where a player at the span is: the crouching hull centre for crouch spans.
    pub fn player_origin(&self) -> Vec3 {
        if self.flags.contains(SpanFlags::CROUCH) {
            self.crouch_origin()
        } else {
            self.origin()
        }
    }

    pub fn walk_dirs(&self) -> impl Iterator<Item = (usize, u32)> + '_ {
        self.next
            .iter()
            .enumerate()
            .filter(|(_, n)| **n != NONE)
            .map(|(d, n)| (d, *n))
    }
}

/// Walking off an edge: from span `from` in direction `dir`, the player falls onto span `to`.
#[derive(Clone, Copy, Debug)]
pub struct Fall {
    pub from: u32,
    pub dir: u8,
    pub to: u32,
    pub height: f32,
}

/// A ledge above the next cell, too high to step up but low enough to jump onto: from span `from` in direction
/// `dir`, span `to` is `height` higher.
#[derive(Clone, Copy, Debug)]
pub struct Ledge {
    pub from: u32,
    pub dir: u8,
    pub to: u32,
    pub height: f32,
}

/// Highest ledge looked for: a crouch jump lifts the feet 63 units.
pub const LEDGE: f32 = 60.0;

/// Where a step from a span leads.
#[derive(Clone, Copy, Debug)]
enum Step {
    Walk {
        feet: f32,
        crouch: bool,
        support: u16,
    },
    Fall {
        feet: f32,
        crouch: bool,
        support: u16,
        height: f32,
    },
    Ledge {
        feet: f32,
        crouch: bool,
        support: u16,
        height: f32,
    },
    Blocked,
}

/// Where a player standing (or crouching) at `from` gets by walking toward `to`: the move `PM_WalkMove` makes,
/// straight or with a step up, then down onto the floor.
fn step(v: &mut WorldView<'_>, from: Vec3, hull: HullKind, to: Vec2) -> (Option<Vec3>, u16, f32) {
    let dest = Vec3::new(to.x, to.y, from.z);
    let tr = v.trace(&TraceQuery::hull(from, dest, hull));
    if tr.start_solid {
        return (None, 0, 0.0);
    }
    let pos = if tr.fraction >= 1.0 {
        dest
    } else {
        let up = v.trace(&TraceQuery::hull(from, from + Vec3::Z * STEP, hull));
        let raised = up.end;
        let dest = Vec3::new(to.x, to.y, raised.z);
        let over = v.trace(&TraceQuery::hull(raised, dest, hull));
        if over.fraction < 1.0 || over.start_solid {
            return (None, 0, 0.0);
        }
        dest
    };
    // Down to the floor: back to the starting height and a step lower is still walking.
    let depth = pos.z - from.z + STEP + 1.0;
    let down = v.trace(&TraceQuery::hull(pos, pos - Vec3::Z * depth, hull));
    if down.start_solid {
        return (None, 0, 0.0);
    }
    let support = |hit: Option<u32>| hit.filter(|h| *h != 0).map_or(0, |h| h as u16);
    if down.fraction < 1.0 {
        if down.normal.z < MIN_NORMAL_Z {
            return (None, 0, 0.0);
        }
        return (Some(down.end), support(down.hit), 0.0);
    }
    let fall = v.trace(&TraceQuery::hull(down.end, down.end - Vec3::Z * MAX_FALL, hull));
    if fall.start_solid || fall.fraction >= 1.0 || fall.normal.z < MIN_NORMAL_Z {
        return (None, 0, 0.0);
    }
    (Some(fall.end), support(fall.hit), from.z - fall.end.z)
}

/// A ledge toward `to` that a player at `from` (standing) could jump onto: rising `LEDGE` units, moving over, and
/// coming down on floor higher than a step. Returns the standing hull centre on it.
fn ledge(v: &mut WorldView<'_>, from: Vec3, to: Vec2) -> Option<(Vec3, u16)> {
    let up = v.trace(&TraceQuery::hull(from, from + Vec3::Z * LEDGE, HullKind::Stand));
    if up.start_solid || up.end.z < from.z + STEP + 1.0 {
        return None;
    }
    let raised = up.end;
    let dest = Vec3::new(to.x, to.y, raised.z);
    let over = v.trace(&TraceQuery::hull(raised, dest, HullKind::Stand));
    if over.start_solid || over.fraction < 1.0 {
        return None;
    }
    let down = v.trace(&TraceQuery::hull(
        dest,
        dest - Vec3::Z * (raised.z - from.z - STEP),
        HullKind::Stand,
    ));
    if down.start_solid || down.fraction >= 1.0 || down.normal.z < MIN_NORMAL_Z {
        return None;
    }
    Some((down.end, down.hit.filter(|h| *h != 0).map_or(0, |h| h as u16)))
}

/// The standing hull fits at `origin`.
fn stand_fits(v: &mut WorldView<'_>, origin: Vec3) -> bool {
    !v.trace(&TraceQuery::hull(origin, origin, HullKind::Stand)).start_solid
}

/// A step from a span toward a neighbouring cell: standing if the player fits, else crouched.
fn walk_from(v: &mut WorldView<'_>, s: &Span, to: Vec2) -> Step {
    let result = |origin: Vec3, crouched_hull: bool, support: u16, fall: f32, v: &mut WorldView<'_>| {
        let feet = origin.z - if crouched_hull { 18.0 } else { 36.0 };
        let crouch = crouched_hull && !stand_fits(v, Vec3::new(origin.x, origin.y, feet + 36.0));
        if fall > STEP {
            Step::Fall {
                feet,
                crouch,
                support,
                height: fall,
            }
        } else {
            Step::Walk { feet, crouch, support }
        }
    };
    if !s.flags.contains(SpanFlags::CROUCH) {
        let (end, support, fall) = step(v, s.origin(), HullKind::Stand, to);
        if let Some(end) = end {
            return result(end, false, support, fall, v);
        }
    }
    let (end, support, fall) = step(v, s.crouch_origin(), HullKind::Crouch, to);
    if let Some(end) = end {
        return result(end, true, support, fall, v);
    }
    if !s.flags.contains(SpanFlags::CROUCH)
        && let Some((end, support)) = ledge(v, s.origin(), to)
    {
        return Step::Ledge {
            feet: end.z - 36.0,
            crouch: false,
            support,
            height: end.z - s.origin().z,
        };
    }
    Step::Blocked
}

/// What the map has at a spot besides floor.
pub trait Surroundings: Sync {
    /// Lava, slime or a volume that hurts: not a place to stand.
    fn hazard(&self, origin: Vec3) -> bool;
    /// A mover rests there (a closed door): the spot opens only with it.
    fn gate(&self, origin: Vec3) -> bool;
    /// Waist deep in water.
    fn water(&self, origin: Vec3) -> bool;
    /// In a push field.
    fn push(&self, origin: Vec3) -> bool;
    /// Touching a ladder.
    fn ladder(&self, origin: Vec3) -> bool;
}

pub struct FloorField {
    pub spans: Vec<Span>,
    columns: FxHashMap<Cell, SmallVec<[u32; 2]>>,
    pub falls: Vec<Fall>,
    pub ledges: Vec<Ledge>,
    pub traces: u64,
}

impl FloorField {
    /// The span of the floor at `feet` in `cell`, if there is one.
    pub fn find(&self, cell: Cell, feet: f32) -> Option<u32> {
        self.columns
            .get(&cell)?
            .iter()
            .copied()
            .find(|&i| (self.spans[i as usize].feet - feet).abs() <= SAME_FLOOR)
    }

    /// Spans of a column, bottom to top in the order found.
    pub fn column(&self, cell: Cell) -> &[u32] {
        self.columns.get(&cell).map_or(&[], |c| c.as_slice())
    }

    /// The span a player standing at `origin` (hull centre) is on: the floor below within a step.
    pub fn at(&self, origin: Vec3) -> Option<u32> {
        let feet = origin.z - 36.0;
        self.columns
            .get(&Cell::of(origin))?
            .iter()
            .copied()
            .filter(|&i| {
                let f = self.spans[i as usize].feet;
                f <= feet + STEP && f >= feet - 64.0
            })
            .max_by(|&a, &b| self.spans[a as usize].feet.total_cmp(&self.spans[b as usize].feet))
    }

    fn insert(
        &mut self,
        cell: Cell,
        at: Vec2,
        feet: f32,
        crouch: bool,
        support: u16,
        hz: &dyn Surroundings,
    ) -> (u32, bool) {
        if let Some(i) = self.find(cell, feet) {
            return (i, false);
        }
        let mut flags = SpanFlags::empty();
        if crouch {
            flags |= SpanFlags::CROUCH;
        }
        let i = self.spans.len() as u32;
        let mut span = Span {
            cell,
            at,
            feet,
            flags,
            support,
            next: [NONE; 8],
            edge: 0,
        };
        if hz.hazard(span.player_origin()) {
            span.flags |= SpanFlags::HAZARD;
        }
        if hz.gate(span.player_origin()) {
            span.flags |= SpanFlags::GATE;
        }
        if hz.water(span.origin()) {
            span.flags |= SpanFlags::WATER;
        }
        if hz.push(span.player_origin()) {
            span.flags |= SpanFlags::PUSH;
        }
        if hz.ladder(span.player_origin()) {
            span.flags |= SpanFlags::LADDER;
        }
        self.spans.push(span);
        self.columns.entry(cell).or_default().push(i);
        (i, true)
    }

    /// Floods the floor from `seeds` (standing hull centres), a wave of spans at a time; each wave is stepped in
    /// parallel and merged in order, so the field does not depend on the thread count.
    pub fn flood(world: &lb_bsp::BspWorld, seeds: &[Vec3], hz: &dyn Surroundings) -> FloorField {
        let mut field = FloorField {
            spans: Vec::new(),
            columns: FxHashMap::default(),
            falls: Vec::new(),
            ledges: Vec::new(),
            traces: 0,
        };
        let mut wave: Vec<u32> = Vec::new();
        let mut view = WorldView::new(world);
        for &seed in seeds {
            let cell = Cell::of(seed);
            let Some((feet, crouch, support)) = settle(&mut view, seed) else {
                continue;
            };
            let (i, new) = field.insert(cell, seed.truncate(), feet, crouch, support, hz);
            if new
                && !field.spans[i as usize]
                    .flags
                    .intersects(SpanFlags::HAZARD | SpanFlags::PUSH)
            {
                wave.push(i);
            }
        }
        field.traces += view.traces;
        while !wave.is_empty() {
            let steps: Vec<([Step; 8], u64)> = wave
                .par_iter()
                .map(|&i| {
                    let mut v = WorldView::new(world);
                    let s = &field.spans[i as usize];
                    let mut out = [Step::Blocked; 8];
                    for (d, o) in out.iter_mut().enumerate() {
                        *o = walk_from(&mut v, s, s.cell.step(d).center());
                    }
                    (out, v.traces)
                })
                .collect();
            let mut next = Vec::new();
            for (&i, (out, traces)) in wave.iter().zip(steps) {
                field.traces += traces;
                let cell = field.spans[i as usize].cell;
                for (d, st) in out.into_iter().enumerate() {
                    let to = cell.step(d);
                    match st {
                        Step::Walk { feet, crouch, support } => {
                            let (j, new) = field.insert(to, to.center(), feet, crouch, support, hz);
                            field.spans[i as usize].next[d] = j;
                            if new
                                && !field.spans[j as usize]
                                    .flags
                                    .intersects(SpanFlags::HAZARD | SpanFlags::PUSH)
                            {
                                next.push(j);
                            }
                        }
                        Step::Fall {
                            feet,
                            crouch,
                            support,
                            height,
                        } => {
                            let (j, new) = field.insert(to, to.center(), feet, crouch, support, hz);
                            field.falls.push(Fall {
                                from: i,
                                dir: d as u8,
                                to: j,
                                height,
                            });
                            if new
                                && !field.spans[j as usize]
                                    .flags
                                    .intersects(SpanFlags::HAZARD | SpanFlags::PUSH)
                            {
                                next.push(j);
                            }
                        }
                        Step::Ledge {
                            feet,
                            crouch,
                            support,
                            height,
                        } => {
                            let (j, new) = field.insert(to, to.center(), feet, crouch, support, hz);
                            field.ledges.push(Ledge {
                                from: i,
                                dir: d as u8,
                                to: j,
                                height,
                            });
                            if new
                                && !field.spans[j as usize]
                                    .flags
                                    .intersects(SpanFlags::HAZARD | SpanFlags::PUSH)
                            {
                                next.push(j);
                            }
                        }
                        Step::Blocked => {}
                    }
                }
            }
            wave = next;
        }
        field.edge_distance();
        field
    }

    /// Cells to the nearest spot that cannot be walked on, by walking.
    fn edge_distance(&mut self) {
        let mut queue = std::collections::VecDeque::new();
        for (i, s) in self.spans.iter_mut().enumerate() {
            if s.next.contains(&NONE) {
                s.edge = 0;
                queue.push_back(i as u32);
            } else {
                s.edge = u16::MAX;
            }
        }
        while let Some(i) = queue.pop_front() {
            let d = self.spans[i as usize].edge;
            for k in 0..8 {
                let j = self.spans[i as usize].next[k];
                if j != NONE && self.spans[j as usize].edge == u16::MAX {
                    self.spans[j as usize].edge = d + 1;
                    queue.push_back(j);
                }
            }
        }
        for s in &mut self.spans {
            if s.edge == u16::MAX {
                s.edge = 0;
            }
        }
    }

    pub fn len(&self) -> usize {
        self.spans.len()
    }

    pub fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }
}

/// The floor under a standing hull centre: its height, whether only a crouching player fits, and the mover it is.
pub fn settle(v: &mut WorldView<'_>, origin: Vec3) -> Option<(f32, bool, u16)> {
    for (hull, lift) in [(HullKind::Stand, 0.0), (HullKind::Crouch, -18.0)] {
        let start = origin + Vec3::Z * (2.0 + lift);
        let tr = v.trace(&TraceQuery::hull(start, start - Vec3::Z * 256.0, hull));
        if tr.start_solid || tr.all_solid || tr.fraction >= 1.0 || tr.normal.z < MIN_NORMAL_Z {
            continue;
        }
        let crouched = hull == HullKind::Crouch;
        let feet = tr.end.z - if crouched { 18.0 } else { 36.0 };
        let crouch = crouched && !stand_fits(v, Vec3::new(tr.end.x, tr.end.y, feet + 36.0));
        let support = tr.hit.filter(|h| *h != 0).map_or(0, |h| h as u16);
        return Some((feet, crouch, support));
    }
    None
}

/// Liquids a player should not stand in.
pub fn harmful_contents(c: i32) -> bool {
    c == contents::LAVA || c == contents::SLIME
}
