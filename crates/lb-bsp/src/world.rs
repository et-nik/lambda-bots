//! The offline world: the map's world model plus its brush entities, traced like `SV_Move` does (world first,
//! then every touched entity, `SV_ClipToLinks` merge rules).
//!
//! Brush entities come in three kinds:
//! - **static solids** (walls, chargers, breakables) stay where the map put them;
//! - **movers** (doors, platforms, trains, buttons) block at their current offset, which starts at the model's
//!   compiled place and is moved by whoever simulates them;
//! - **volumes** (water, ladders, illusionary brushes with contents) do not block; they give point contents.

use lb_core::Vec3;
use lb_worldq::{HullKind, Trace, TraceQuery, Tracer, contents};

use crate::entities::{Entity, parse_entities};
use crate::file::{Bsp, BspError};

/// Brush entities that stay where the map put them and block movement.
const STATIC_SOLIDS: &[&str] = &[
    "func_wall",
    "func_wall_toggle",
    "func_healthcharger",
    "func_recharge",
    "func_breakable",
    "func_pushable",
];

/// Brush entities that move and block movement.
const MOVERS: &[&str] = &[
    "func_door",
    "func_door_rotating",
    "func_plat",
    "func_platrot",
    "func_train",
    "func_tracktrain",
    "func_button",
    "func_rot_button",
    "momentary_door",
    "momentary_rot_button",
    "func_rotating",
    "func_pendulum",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrushKind {
    Static,
    Mover,
    /// Non-solid with contents (`CONTENTS_*`).
    Volume(i32),
}

#[derive(Clone, Debug)]
pub struct Brush {
    /// Index into `BspWorld::entities`.
    pub entity: usize,
    pub model: usize,
    pub kind: BrushKind,
    /// Entity origin (origin brush) as the map sets it.
    pub origin: Vec3,
    /// Current displacement from `origin` (movers only).
    pub offset: Vec3,
    /// Absolute bounds at `origin`.
    pub mins: Vec3,
    pub maxs: Vec3,
    pub classname: String,
    /// Drawn with a render mode (glass, grates): sight traces pass through.
    pub glass: bool,
    /// Blocks traces; movers can be switched off (a broken breakable, a door treated as open).
    pub solid: bool,
}

impl Brush {
    pub fn position(&self) -> Vec3 {
        self.origin + self.offset
    }

    pub fn abs_mins(&self) -> Vec3 {
        self.mins + self.offset
    }

    pub fn abs_maxs(&self) -> Vec3 {
        self.maxs + self.offset
    }
}

pub struct BspWorld {
    pub bsp: Bsp,
    pub entities: Vec<Entity>,
    pub brushes: Vec<Brush>,
    pub traces: u64,
    /// `trigger_push` fields a simulated player is pushed by: (model, velocity). Empty unless whoever simulates
    /// fills it (`Mechanisms::push_fields`).
    pub pushes: Vec<(usize, Vec3)>,
}

fn classify(e: &Entity) -> Option<BrushKind> {
    let class = e.classname();
    if class == "func_ladder" {
        return Some(BrushKind::Volume(contents::LADDER));
    }
    if matches!(class, "func_water" | "func_illusionary") {
        let skin = e
            .int("skin")
            .unwrap_or(if class == "func_water" { contents::WATER } else { 0 });
        return (skin != 0).then_some(BrushKind::Volume(skin));
    }
    if class == "func_wall_toggle" && e.spawnflags() & 1 != 0 {
        return None;
    }
    if STATIC_SOLIDS.contains(&class) {
        return Some(BrushKind::Static);
    }
    if MOVERS.contains(&class) {
        // A door with contents is a liquid volume (a `func_door` used as water), a passable door is not solid.
        if class.starts_with("func_door") {
            let skin = e.int("skin").unwrap_or(0);
            if skin != 0 {
                return Some(BrushKind::Volume(skin));
            }
            if e.spawnflags() & 8 != 0 {
                return None;
            }
        }
        return Some(BrushKind::Mover);
    }
    None
}

impl BspWorld {
    pub fn load(bytes: &[u8]) -> Result<BspWorld, BspError> {
        let bsp = Bsp::parse(bytes)?;
        let entities = parse_entities(&bsp.entities);
        let brushes = entities
            .iter()
            .enumerate()
            .filter_map(|(i, e)| {
                let kind = classify(e)?;
                let model = e.brush_model()?;
                let m = bsp.models.get(model)?;
                let origin = e.origin();
                Some(Brush {
                    entity: i,
                    model,
                    kind,
                    origin,
                    offset: Vec3::ZERO,
                    mins: m.mins + origin,
                    maxs: m.maxs + origin,
                    classname: e.classname().into(),
                    glass: e.int("rendermode").unwrap_or(0) != 0,
                    solid: !matches!(kind, BrushKind::Volume(_)),
                })
            })
            .collect();
        Ok(BspWorld {
            bsp,
            entities,
            brushes,
            traces: 0,
            pushes: Vec::new(),
        })
    }

    /// The push a player box (`hull` at `origin`) gets from the push fields it is in (`CTriggerPush::Touch`: the
    /// pushes of fields touched in one frame add up).
    pub fn push_at(&self, origin: Vec3, hull: HullKind) -> Vec3 {
        self.pushes
            .iter()
            .filter(|(m, _)| self.hull_overlaps(*m, Vec3::ZERO, origin, hull))
            .map(|(_, v)| *v)
            .sum()
    }

    /// The brush entity of model `*model`.
    pub fn brush(&self, model: usize) -> Option<&Brush> {
        self.brushes.iter().find(|b| b.model == model)
    }

    pub fn brush_mut(&mut self, model: usize) -> Option<&mut Brush> {
        self.brushes.iter_mut().find(|b| b.model == model)
    }

    /// Makes every mover block (`true`) or pass (`false`).
    pub fn set_movers_solid(&mut self, solid: bool) {
        for b in &mut self.brushes {
            if b.kind == BrushKind::Mover {
                b.solid = solid;
            }
        }
    }

    fn trace_model(&self, model: usize, offset: Vec3, q: &TraceQuery) -> Option<Trace> {
        let hull = self.bsp.hull(model, q.hull)?;
        let mut tr = hull.trace(q.start, q.end, offset);
        if tr.fraction < 1.0 || tr.start_solid {
            tr.hit = Some(model as u32);
        }
        Some(tr)
    }

    /// Contents of `p` inside model `model` placed at `offset` (hull 0).
    pub fn model_contents(&self, model: usize, offset: Vec3, p: Vec3) -> i32 {
        match self.bsp.hull(model, HullKind::Point) {
            Some(h) => h.point_contents(h.first, p - offset),
            None => contents::EMPTY,
        }
    }

    /// Whether a player box (`hull` at `origin`) overlaps model `model` at `offset`: the exact hull-point test the
    /// engine uses for trigger touches and ladders.
    pub fn hull_overlaps(&self, model: usize, offset: Vec3, origin: Vec3, hull: HullKind) -> bool {
        match self.bsp.hull(model, hull) {
            Some(h) => h.point_contents(h.first, origin - offset) != contents::EMPTY,
            None => false,
        }
    }
}

impl Tracer for BspWorld {
    fn trace(&mut self, q: &TraceQuery) -> Trace {
        self.traces += 1;
        self.trace_shared(q)
    }

    fn point_contents(&mut self, p: Vec3) -> i32 {
        self.point_contents_shared(p)
    }
}

/// A world shared by several threads: each traces through its own view, which counts its traces.
pub struct WorldView<'a> {
    pub world: &'a BspWorld,
    pub traces: u64,
}

impl<'a> WorldView<'a> {
    pub fn new(world: &'a BspWorld) -> WorldView<'a> {
        WorldView { world, traces: 0 }
    }
}

impl Tracer for WorldView<'_> {
    fn trace(&mut self, q: &TraceQuery) -> Trace {
        self.traces += 1;
        self.world.trace_shared(q)
    }

    fn point_contents(&mut self, p: Vec3) -> i32 {
        self.world.point_contents_shared(p)
    }
}

impl BspWorld {
    /// A trace that does not count itself, for threads sharing the world (`WorldView`).
    pub fn trace_shared(&self, q: &TraceQuery) -> Trace {
        let mut best = self
            .trace_model(0, Vec3::ZERO, q)
            .unwrap_or_else(|| Trace::clear(q.end));
        if best.all_solid {
            return best;
        }
        let (hmin, hmax) = q.hull.extents();
        let move_min = q.start.min(q.end) + hmin - Vec3::ONE;
        let move_max = q.start.max(q.end) + hmax + Vec3::ONE;
        for b in &self.brushes {
            if !b.solid
                || b.abs_mins().cmpgt(move_max).any()
                || b.abs_maxs().cmplt(move_min).any()
                || (q.ignore_glass && b.glass)
            {
                continue;
            }
            let Some(tr) = self.trace_model(b.model, b.position(), q) else {
                continue;
            };
            if tr.all_solid || tr.start_solid || tr.fraction < best.fraction {
                let start_solid = best.start_solid;
                best = tr;
                best.start_solid |= start_solid;
            }
        }
        best
    }

    /// World contents, then the contents of a non-solid brush entity around the point (`SV_PointContents`).
    pub fn point_contents_shared(&self, p: Vec3) -> i32 {
        let Some(hull) = self.bsp.hull(0, HullKind::Point) else {
            return contents::EMPTY;
        };
        let mut c = hull.point_contents(hull.first, p);
        if (contents::CURRENT_DOWN..=contents::CURRENT_0).contains(&c) {
            c = contents::WATER;
        }
        if c == contents::SOLID {
            return c;
        }
        for b in &self.brushes {
            if let BrushKind::Volume(v) = b.kind
                && p.cmpge(b.abs_mins()).all()
                && p.cmple(b.abs_maxs()).all()
                && self.model_contents(b.model, b.position(), p) != contents::EMPTY
            {
                return v;
            }
        }
        c
    }
}

impl Bsp {
    /// Leaf of the world model containing `p` (index into `leafs`; 0 is the shared solid leaf).
    pub fn leaf_at(&self, p: Vec3) -> usize {
        let mut num = self.models[0].headnode[0];
        while num >= 0 {
            let node = &self.nodes[num as usize];
            let plane = &self.planes[node.plane as usize];
            let d = if plane.kind < 3 {
                p[plane.kind as usize] - plane.dist
            } else {
                plane.normal.dot(p) - plane.dist
            };
            num = node.children[usize::from(d < 0.0)];
        }
        (-1 - num) as usize
    }

    /// Decompressed PVS row of `leaf` (`Mod_DecompressVis`); `None` when the map has no visibility data.
    pub fn pvs_row(&self, leaf: usize) -> Option<Vec<u8>> {
        let visleafs = self.models[0].visleafs.max(0) as usize;
        let row = visleafs.div_ceil(8);
        let ofs = self.leafs.get(leaf)?.visofs;
        if ofs < 0 || self.visdata.is_empty() {
            return None;
        }
        let mut out = Vec::with_capacity(row);
        let mut i = ofs as usize;
        while out.len() < row {
            let &b = self.visdata.get(i)?;
            if b != 0 {
                out.push(b);
                i += 1;
                continue;
            }
            let &count = self.visdata.get(i + 1)?;
            i += 2;
            out.extend(std::iter::repeat_n(0u8, usize::from(count)));
        }
        out.truncate(row);
        Some(out)
    }

    /// Whether `to` is in the potentially visible set of `from` (the engine's PVS test, not line of sight).
    pub fn pvs_visible(&self, from: Vec3, to: Vec3) -> bool {
        let target = self.leaf_at(to);
        if target == 0 {
            return false;
        }
        match self.pvs_row(self.leaf_at(from)) {
            Some(row) => row
                .get((target - 1) >> 3)
                .is_some_and(|b| b & (1 << ((target - 1) & 7)) != 0),
            None => true,
        }
    }
}
