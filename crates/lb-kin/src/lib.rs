//! GoldSrc kinematics: player movement as the server runs it, and the traversal checks built on it.

#![forbid(unsafe_code)]

pub mod boxworld;
pub mod hop;
pub mod physics;
pub mod pmove;
pub mod tricks;
pub mod validate;

pub use physics::Physics;
pub use pmove::{Cmd, Ladder, MoveEvents, MoveWorld, Player, player_move};

use lb_bsp::BspWorld;
use lb_bsp::world::{BrushKind, WorldView};
use lb_core::Vec3;
use lb_worldq::{HullKind, contents};

/// The ladder a player box at `origin` touches (`PM_Ladder`), with the face it touches.
fn ladder_at(world: &BspWorld, origin: Vec3, hull: HullKind) -> Option<Ladder> {
    let (hmin, hmax) = hull.extents();
    let found = world.brushes.iter().find(|b| {
        b.kind == BrushKind::Volume(contents::LADDER)
            && (origin + hmax).cmpgt(b.abs_mins()).all()
            && (origin + hmin).cmplt(b.abs_maxs()).all()
            && world.hull_overlaps(b.model, b.position(), origin, hull)
    })?;
    let center = (found.abs_mins() + found.abs_maxs()) * 0.5;
    let normal = world
        .bsp
        .hull(found.model, HullKind::Point)
        .map(|h| h.trace(origin, center, found.position()))
        .filter(|t| t.fraction < 1.0)
        .map(|t| t.normal);
    Some(Ladder { normal })
}

impl MoveWorld for BspWorld {
    fn ladder(&mut self, origin: Vec3, hull: HullKind) -> Option<Ladder> {
        ladder_at(self, origin, hull)
    }

    fn push(&mut self, origin: Vec3, hull: HullKind) -> Vec3 {
        self.push_at(origin, hull)
    }
}

impl MoveWorld for WorldView<'_> {
    fn ladder(&mut self, origin: Vec3, hull: HullKind) -> Option<Ladder> {
        ladder_at(self.world, origin, hull)
    }

    fn push(&mut self, origin: Vec3, hull: HullKind) -> Vec3 {
        self.world.push_at(origin, hull)
    }
}
