//! The offline world: the map's world model plus brush entities that never move, traced like `SV_Move` does
//! (world first, then every touched entity, `SV_ClipToLinks` merge rules). Doors, platforms and trains move, so
//! only live engine traces know where they are.

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

#[derive(Clone, Debug)]
pub struct StaticBrush {
    pub model: usize,
    pub origin: Vec3,
    pub mins: Vec3,
    pub maxs: Vec3,
    pub classname: String,
    /// Drawn with a render mode (glass, grates): sight traces pass through.
    pub glass: bool,
}

pub struct BspWorld {
    pub bsp: Bsp,
    pub entities: Vec<Entity>,
    pub brushes: Vec<StaticBrush>,
    pub traces: u64,
}

impl BspWorld {
    pub fn load(bytes: &[u8]) -> Result<BspWorld, BspError> {
        let bsp = Bsp::parse(bytes)?;
        let entities = parse_entities(&bsp.entities);
        let brushes = entities
            .iter()
            .filter(|e| STATIC_SOLIDS.contains(&e.classname()))
            .filter(|e| !(e.classname() == "func_wall_toggle" && e.spawnflags() & 1 != 0))
            .filter_map(|e| {
                let model = e.brush_model()?;
                let m = bsp.models.get(model)?;
                let origin = e.origin();
                Some(StaticBrush {
                    model,
                    origin,
                    mins: m.mins + origin,
                    maxs: m.maxs + origin,
                    classname: e.classname().into(),
                    glass: e.int("rendermode").unwrap_or(0) != 0,
                })
            })
            .collect();
        Ok(BspWorld {
            bsp,
            entities,
            brushes,
            traces: 0,
        })
    }

    fn trace_model(&self, model: usize, offset: Vec3, q: &TraceQuery) -> Option<Trace> {
        let hull = self.bsp.hull(model, q.hull)?;
        let mut tr = hull.trace(q.start, q.end, offset);
        if tr.fraction < 1.0 || tr.start_solid {
            tr.hit = Some(model as u32);
        }
        Some(tr)
    }
}

impl Tracer for BspWorld {
    fn trace(&mut self, q: &TraceQuery) -> Trace {
        self.traces += 1;
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
            if b.mins.cmpgt(move_max).any() || b.maxs.cmplt(move_min).any() || (q.ignore_glass && b.glass) {
                continue;
            }
            let Some(tr) = self.trace_model(b.model, b.origin, q) else {
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

    fn point_contents(&mut self, p: Vec3) -> i32 {
        let Some(hull) = self.bsp.hull(0, HullKind::Point) else {
            return contents::EMPTY;
        };
        let c = hull.point_contents(hull.first, p);
        if (contents::CURRENT_DOWN..=contents::CURRENT_0).contains(&c) {
            contents::WATER
        } else {
            c
        }
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
