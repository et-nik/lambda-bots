//! World query contracts: the `Tracer` trait, traces, hulls and contents. Implemented by the offline BSP world
//! (`lb-bsp`) and by the live engine (`lb-runtime`), so navigation and perception code runs on either.

#![forbid(unsafe_code)]

use lb_core::Vec3;
use serde::{Deserialize, Serialize};

/// Collision hulls, numbered as in the engine (`TraceHull` hull numbers and BSP model hulls).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum HullKind {
    Point = 0,
    /// Standing player, 32×32×72.
    Stand = 1,
    Large = 2,
    /// Crouching player, 32×32×36.
    Crouch = 3,
}

impl HullKind {
    pub const ALL: [HullKind; 4] = [HullKind::Point, HullKind::Stand, HullKind::Large, HullKind::Crouch];

    pub fn index(self) -> usize {
        self as usize
    }

    /// Box of the hull around its origin (`model.cpp` clip sizes).
    pub fn extents(self) -> (Vec3, Vec3) {
        match self {
            HullKind::Point => (Vec3::ZERO, Vec3::ZERO),
            HullKind::Stand => (Vec3::new(-16.0, -16.0, -36.0), Vec3::new(16.0, 16.0, 36.0)),
            HullKind::Large => (Vec3::splat(-32.0), Vec3::splat(32.0)),
            HullKind::Crouch => (Vec3::new(-16.0, -16.0, -18.0), Vec3::new(16.0, 16.0, 18.0)),
        }
    }

    /// The hull the engine uses for a moving box (`SV_HullForBsp`).
    pub fn for_size(mins: Vec3, maxs: Vec3) -> HullKind {
        let size = maxs - mins;
        if size.x <= 8.0 {
            HullKind::Point
        } else if size.x <= 36.0 {
            if size.z <= 36.0 {
                HullKind::Crouch
            } else {
                HullKind::Stand
            }
        } else {
            HullKind::Large
        }
    }
}

/// Engine `CONTENTS_*` values.
pub mod contents {
    pub const EMPTY: i32 = -1;
    pub const SOLID: i32 = -2;
    pub const WATER: i32 = -3;
    pub const SLIME: i32 = -4;
    pub const LAVA: i32 = -5;
    pub const SKY: i32 = -6;
    pub const ORIGIN: i32 = -7;
    pub const CLIP: i32 = -8;
    pub const CURRENT_0: i32 = -9;
    pub const CURRENT_DOWN: i32 = -14;
    pub const TRANSLUCENT: i32 = -15;
    pub const LADDER: i32 = -16;

    pub fn is_liquid(c: i32) -> bool {
        matches!(c, WATER | SLIME | LAVA) || (CURRENT_DOWN..=CURRENT_0).contains(&c)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct TraceQuery {
    pub start: Vec3,
    pub end: Vec3,
    pub hull: HullKind,
    /// Players and monsters do not block (the offline world has none anyway).
    pub ignore_monsters: bool,
    /// Brush entities drawn with a render mode (glass, grates) do not block: the engine's `ignore_glass` flag.
    pub ignore_glass: bool,
    /// Entity the trace passes through, as an edict index; usually the player who looks.
    pub ignore: Option<u16>,
}

impl TraceQuery {
    pub fn hull(start: Vec3, end: Vec3, hull: HullKind) -> TraceQuery {
        TraceQuery {
            start,
            end,
            hull,
            ignore_monsters: true,
            ignore_glass: false,
            ignore: None,
        }
    }

    pub fn line(start: Vec3, end: Vec3) -> TraceQuery {
        TraceQuery {
            start,
            end,
            hull: HullKind::Point,
            ignore_monsters: true,
            ignore_glass: false,
            ignore: None,
        }
    }

    /// Line of sight of the player with edict index `viewer`: other players block, glass does not.
    pub fn sight(start: Vec3, end: Vec3, viewer: u16) -> TraceQuery {
        TraceQuery {
            start,
            end,
            hull: HullKind::Point,
            ignore_monsters: false,
            ignore_glass: true,
            ignore: Some(viewer),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Trace {
    pub all_solid: bool,
    pub start_solid: bool,
    pub in_open: bool,
    pub in_water: bool,
    /// Share of the move completed before the hit; 1 = no hit.
    pub fraction: f32,
    pub end: Vec3,
    pub normal: Vec3,
    pub dist: f32,
    /// What was hit: `Some(0)` = world model, `Some(n)` = brush model `*n`, entity index for live traces.
    pub hit: Option<u32>,
}

impl Trace {
    pub fn clear(end: Vec3) -> Trace {
        Trace {
            all_solid: false,
            start_solid: false,
            in_open: true,
            in_water: false,
            fraction: 1.0,
            end,
            normal: Vec3::ZERO,
            dist: 0.0,
            hit: None,
        }
    }

    pub fn blocked(&self) -> bool {
        self.fraction < 1.0 || self.start_solid || self.all_solid
    }
}

pub trait Tracer {
    fn trace(&mut self, q: &TraceQuery) -> Trace;
    fn point_contents(&mut self, p: Vec3) -> i32;
}

/// The engine's visibility sets over BSP leaves: which entities a client may be sent (PVS) and which sounds reach
/// it (PAS). Both are coarse pre-filters; line of sight still needs traces.
pub trait VisSets {
    /// Some leaf touched by the box `mins..maxs` is in the PVS the engine builds around the view origin `eye`.
    fn box_in_pvs(&self, eye: Vec3, mins: Vec3, maxs: Vec3) -> bool;
    /// A sound or event at `source` is delivered to a client standing at `origin`.
    fn in_pas(&self, origin: Vec3, source: Vec3) -> bool;
}

/// A map without visibility data: the engine sends and plays everything.
pub struct AllVisible;

impl VisSets for AllVisible {
    fn box_in_pvs(&self, _eye: Vec3, _mins: Vec3, _maxs: Vec3) -> bool {
        true
    }

    fn in_pas(&self, _origin: Vec3, _source: Vec3) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hull_selection_matches_the_engine() {
        let (mins, maxs) = HullKind::Stand.extents();
        assert_eq!(HullKind::for_size(mins, maxs), HullKind::Stand);
        let (mins, maxs) = HullKind::Crouch.extents();
        assert_eq!(HullKind::for_size(mins, maxs), HullKind::Crouch);
        assert_eq!(HullKind::for_size(Vec3::ZERO, Vec3::ZERO), HullKind::Point);
        let (mins, maxs) = HullKind::Large.extents();
        assert_eq!(HullKind::for_size(mins, maxs), HullKind::Large);
        assert!(
            contents::is_liquid(contents::WATER) && contents::is_liquid(-12) && !contents::is_liquid(contents::SOLID)
        );
    }
}
