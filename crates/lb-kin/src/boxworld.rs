//! A world of axis-aligned boxes for tests: solids, liquids and ladders, traced like the engine traces a hull
//! through a brush (the box grows by the hull, and a hit stops 1/32 unit in front of the plane).

use lb_core::Vec3;
use lb_worldq::{HullKind, Trace, TraceQuery, Tracer, contents};

use crate::pmove::{Ladder, MoveWorld};

const DIST_EPSILON: f32 = 0.031_25;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    pub fn new(min: Vec3, max: Vec3) -> Aabb {
        Aabb { min, max }
    }

    fn grown(&self, hull: HullKind) -> Aabb {
        let (hmin, hmax) = hull.extents();
        Aabb {
            min: self.min - hmax,
            max: self.max - hmin,
        }
    }

    fn strictly_contains(&self, p: Vec3) -> bool {
        p.cmpgt(self.min).all() && p.cmplt(self.max).all()
    }

    pub fn center(&self) -> Vec3 {
        (self.min + self.max) * 0.5
    }
}

/// A solid box; `id` is reported as `Trace::hit` (0 = world).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Solid {
    pub bounds: Aabb,
    pub id: u32,
    pub offset: Vec3,
    pub enabled: bool,
}

#[derive(Clone, Debug, Default)]
pub struct BoxWorld {
    pub solids: Vec<Solid>,
    /// Non-solid volumes with contents: water, slime, lava, ladders.
    pub volumes: Vec<(Aabb, i32)>,
    /// Trigger volumes by id: they neither block nor have contents, a player touches them.
    pub triggers: Vec<(Aabb, u32)>,
    pub traces: u64,
}

/// Where a moving segment enters a box: fraction with the engine's stand-off, and the face normal.
fn clip(b: &Aabb, start: Vec3, end: Vec3) -> Option<(f32, Vec3)> {
    let d = end - start;
    let mut t_enter = f32::NEG_INFINITY;
    let mut t_exit = f32::INFINITY;
    let mut normal = Vec3::ZERO;
    let mut dists = (0.0f32, 0.0f32);
    for axis in 0..3 {
        let (s, e, lo, hi) = (start[axis], end[axis], b.min[axis], b.max[axis]);
        if d[axis] == 0.0 {
            if s <= lo || s >= hi {
                return None;
            }
            continue;
        }
        let (t0, t1, n, d1, d2) = if d[axis] > 0.0 {
            ((lo - s) / d[axis], (hi - s) / d[axis], -1.0, lo - s, lo - e)
        } else {
            ((hi - s) / d[axis], (lo - s) / d[axis], 1.0, s - hi, e - hi)
        };
        if t0 > t_enter {
            t_enter = t0;
            normal = Vec3::ZERO;
            normal[axis] = n;
            dists = (d1, d2);
        }
        t_exit = t_exit.min(t1);
    }
    if t_enter > t_exit || t_enter > 1.0 || t_exit <= 0.0 || t_enter == f32::NEG_INFINITY {
        return None;
    }
    let (d1, d2) = dists;
    // d1: how far the start is in front of the entry plane; d2 (negative): how far the end is behind it.
    let frac = ((d1 - DIST_EPSILON) / (d1 - d2)).clamp(0.0, 1.0);
    Some((frac, normal))
}

impl BoxWorld {
    pub fn new() -> BoxWorld {
        BoxWorld::default()
    }

    /// A solid box that belongs to the world.
    pub fn solid(&mut self, min: Vec3, max: Vec3) -> &mut BoxWorld {
        self.entity(min, max, 0)
    }

    /// A solid box reported as entity `id`.
    pub fn entity(&mut self, min: Vec3, max: Vec3, id: u32) -> &mut BoxWorld {
        self.solids.push(Solid {
            bounds: Aabb::new(min, max),
            id,
            offset: Vec3::ZERO,
            enabled: true,
        });
        self
    }

    pub fn volume(&mut self, min: Vec3, max: Vec3, contents: i32) -> &mut BoxWorld {
        self.volumes.push((Aabb::new(min, max), contents));
        self
    }

    pub fn trigger(&mut self, min: Vec3, max: Vec3, id: u32) -> &mut BoxWorld {
        self.triggers.push((Aabb::new(min, max), id));
        self
    }

    /// Whether a player box (`hull` at `origin`) overlaps solid or trigger `id`.
    pub fn overlaps(&self, id: u32, origin: Vec3, hull: HullKind) -> bool {
        let solid = self
            .solids
            .iter()
            .filter(|s| s.id == id)
            .any(|s| Self::placed(s).grown(hull).strictly_contains(origin));
        solid
            || self
                .triggers
                .iter()
                .any(|(b, t)| *t == id && b.grown(hull).strictly_contains(origin))
    }

    /// Bounds of solid `id` where it is now.
    pub fn bounds(&self, id: u32) -> Option<Aabb> {
        let s = self.solids.iter().find(|s| s.id == id)?;
        Some(Self::placed(s))
    }

    pub fn solid_mut(&mut self, id: u32) -> Option<&mut Solid> {
        self.solids.iter_mut().find(|s| s.id == id)
    }

    /// A floor slab whose top is at `z`, spanning `half` units around the origin.
    pub fn floor(&mut self, z: f32, half: f32) -> &mut BoxWorld {
        self.solid(Vec3::new(-half, -half, z - 16.0), Vec3::new(half, half, z))
    }

    fn placed(s: &Solid) -> Aabb {
        Aabb::new(s.bounds.min + s.offset, s.bounds.max + s.offset)
    }
}

impl Tracer for BoxWorld {
    fn trace(&mut self, q: &TraceQuery) -> Trace {
        self.traces += 1;
        let mut best = Trace::clear(q.end);
        for s in self.solids.iter().filter(|s| s.enabled) {
            let grown = Self::placed(s).grown(q.hull);
            if grown.strictly_contains(q.start) {
                best.start_solid = true;
                best.in_open = false;
                if grown.strictly_contains(q.end) {
                    best.all_solid = true;
                }
                best.hit = Some(s.id);
                continue;
            }
            if let Some((frac, normal)) = clip(&grown, q.start, q.end)
                && frac < best.fraction
            {
                best.fraction = frac;
                best.normal = normal;
                best.dist = normal.dot(q.start + (q.end - q.start) * frac);
                best.hit = Some(s.id);
            }
        }
        if best.fraction < 1.0 {
            best.end = q.start + (q.end - q.start) * best.fraction;
        }
        best
    }

    fn point_contents(&mut self, p: Vec3) -> i32 {
        if self
            .solids
            .iter()
            .any(|s| s.enabled && Self::placed(s).strictly_contains(p))
        {
            return contents::SOLID;
        }
        self.volumes
            .iter()
            .find(|(b, _)| b.strictly_contains(p))
            .map_or(contents::EMPTY, |(_, c)| *c)
    }
}

impl MoveWorld for BoxWorld {
    fn ladder(&mut self, origin: Vec3, hull: HullKind) -> Option<Ladder> {
        let (b, _) = self
            .volumes
            .iter()
            .find(|(b, c)| *c == contents::LADDER && b.grown(hull).strictly_contains(origin))?;
        let normal = clip(b, origin, b.center()).map(|(_, n)| n);
        Some(Ladder { normal })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hull_traces_stop_short_of_the_plane() {
        let mut w = BoxWorld::new();
        w.floor(0.0, 512.0);
        let start = Vec3::new(0.0, 0.0, 100.0);
        let tr = w.trace(&TraceQuery::hull(start, start - Vec3::Z * 200.0, HullKind::Stand));
        assert!((tr.end.z - 36.031_25).abs() < 1e-3, "{tr:?}");
        assert_eq!(tr.normal, Vec3::Z);
        let rest = tr.end;
        let down = w.trace(&TraceQuery::hull(rest, rest - Vec3::Z * 2.0, HullKind::Stand));
        assert!(
            down.fraction < 1e-3,
            "resting on the floor, a probe down hits at once: {down:?}"
        );
        let sideways = w.trace(&TraceQuery::hull(rest, rest + Vec3::X * 100.0, HullKind::Stand));
        assert_eq!(sideways.fraction, 1.0, "sliding along the floor is free");
        assert_eq!(w.point_contents(Vec3::new(0.0, 0.0, -1.0)), contents::SOLID);
    }
}
