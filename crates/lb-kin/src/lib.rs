//! GoldSrc kinematics: player movement as the server runs it, and the traversal checks built on it.

#![forbid(unsafe_code)]

pub mod boxworld;
pub mod physics;
pub mod pmove;
pub mod validate;

pub use physics::Physics;
pub use pmove::{Cmd, Ladder, MoveEvents, MoveWorld, Player, player_move};

use lb_bsp::BspWorld;
use lb_bsp::world::BrushKind;
use lb_core::Vec3;
use lb_worldq::{HullKind, contents};

impl MoveWorld for BspWorld {
    fn ladder(&mut self, origin: Vec3, hull: HullKind) -> Option<Ladder> {
        let (hmin, hmax) = hull.extents();
        let found = self.brushes.iter().find(|b| {
            b.kind == BrushKind::Volume(contents::LADDER)
                && (origin + hmax).cmpgt(b.abs_mins()).all()
                && (origin + hmin).cmplt(b.abs_maxs()).all()
                && self.hull_overlaps(b.model, b.position(), origin, hull)
        })?;
        let center = (found.abs_mins() + found.abs_maxs()) * 0.5;
        let normal = self
            .bsp
            .hull(found.model, HullKind::Point)
            .map(|h| h.trace(origin, center, found.position()))
            .filter(|t| t.fraction < 1.0)
            .map(|t| t.normal);
        Some(Ladder { normal })
    }
}
