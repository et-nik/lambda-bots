//! Hull traces: exact ports of the engine's `SV_HullPointContents` and `SV_RecursiveHullCheck` (ReHLDS
//! `engine/world.cpp`). Release ReHLDS builds (`REHLDS_FIXES`) do this math in `f32`, so the port does too, including
//! the point interpolation, so offline results match live engine traces bit for bit.

use lb_core::Vec3;
use lb_worldq::{HullKind, Trace, contents};

use crate::file::{Bsp, ClipNode, Plane};

const DIST_EPSILON: f32 = 0.03125;

/// One hull of one model.
#[derive(Clone, Copy)]
pub struct Hull<'a> {
    pub nodes: &'a [ClipNode],
    pub planes: &'a [Plane],
    pub first: i32,
}

impl Bsp {
    /// Hull `kind` of model `model` (0 = world).
    pub fn hull(&self, model: usize, kind: HullKind) -> Option<Hull<'_>> {
        let m = self.models.get(model)?;
        let first = m.headnode[kind.index()];
        let nodes: &[ClipNode] = if kind == HullKind::Point {
            &self.hull0
        } else {
            &self.clipnodes
        };
        if first >= 0 && first as usize >= nodes.len() {
            return None;
        }
        Some(Hull {
            nodes,
            planes: &self.planes,
            first,
        })
    }
}

impl Hull<'_> {
    fn dist(&self, node: &ClipNode, p: Vec3) -> f32 {
        let plane = &self.planes[node.plane as usize];
        if plane.kind < 3 {
            p[plane.kind as usize] - plane.dist
        } else {
            plane.normal.dot(p) - plane.dist
        }
    }

    pub fn point_contents(&self, mut num: i32, p: Vec3) -> i32 {
        while num >= 0 {
            let node = &self.nodes[num as usize];
            num = node.children[usize::from(self.dist(node, p) < 0.0)];
        }
        num
    }

    /// Returns false once the trace has been stopped (the engine's return convention).
    pub fn recursive_check(&self, num: i32, p1f: f32, p2f: f32, p1: Vec3, p2: Vec3, tr: &mut Trace) -> bool {
        if num < 0 {
            if num != contents::SOLID {
                tr.all_solid = false;
                if num == contents::EMPTY {
                    tr.in_open = true;
                } else if num != contents::TRANSLUCENT {
                    tr.in_water = true;
                }
            } else {
                tr.start_solid = true;
            }
            return true;
        }
        let node = &self.nodes[num as usize];
        let plane = &self.planes[node.plane as usize];
        let (t1, t2) = if plane.kind < 3 {
            let k = plane.kind as usize;
            (p1[k] - plane.dist, p2[k] - plane.dist)
        } else {
            (plane.normal.dot(p1) - plane.dist, plane.normal.dot(p2) - plane.dist)
        };
        if t1 >= 0.0 && t2 >= 0.0 {
            return self.recursive_check(node.children[0], p1f, p2f, p1, p2, tr);
        }
        if t1 < 0.0 && t2 < 0.0 {
            return self.recursive_check(node.children[1], p1f, p2f, p1, p2, tr);
        }
        let pdif = p2f - p1f;
        let mut frac = if t1 < 0.0 {
            (t1 + DIST_EPSILON) / (t1 - t2)
        } else {
            (t1 - DIST_EPSILON) / (t1 - t2)
        };
        // NaN stays NaN (as in the engine) and is caught right below.
        frac = frac.clamp(0.0, 1.0);
        if frac.is_nan() {
            return false;
        }
        let mut midf = p1f + pdif * frac;
        let point = p2 - p1;
        let mut mid = p1 + point * frac;
        let side = usize::from(t1 < 0.0);

        if !self.recursive_check(node.children[side], p1f, midf, p1, mid, tr) {
            return false;
        }
        if self.point_contents(node.children[side ^ 1], mid) != contents::SOLID {
            return self.recursive_check(node.children[side ^ 1], midf, p2f, mid, p2, tr);
        }
        if tr.all_solid {
            return false;
        }
        if side == 0 {
            tr.normal = plane.normal;
            tr.dist = plane.dist;
        } else {
            tr.normal = -plane.normal;
            tr.dist = -plane.dist;
        }
        while self.point_contents(self.first, mid) == contents::SOLID {
            frac -= 0.1;
            if frac < 0.0 {
                tr.fraction = midf;
                tr.end = mid;
                return false;
            }
            midf = p1f + pdif * frac;
            mid = p1 + point * frac;
        }
        tr.fraction = midf;
        tr.end = mid;
        false
    }

    /// A move through this hull, like `SV_SingleClipMoveToEntity` for an unrotated model at `offset`.
    pub fn trace(&self, start: Vec3, end: Vec3, offset: Vec3) -> Trace {
        let mut tr = Trace {
            all_solid: true,
            start_solid: false,
            in_open: false,
            in_water: false,
            fraction: 1.0,
            end,
            normal: Vec3::ZERO,
            dist: 0.0,
            hit: None,
        };
        self.recursive_check(self.first, 0.0, 1.0, start - offset, end - offset, &mut tr);
        if tr.fraction != 1.0 {
            tr.end = start + (end - start) * tr.fraction;
        }
        tr
    }
}
