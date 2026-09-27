//! Placing a tripmine: stop, draw the mine, look at the spot on the wall and press once when the view's line meets
//! the wall there within reach (the game places the mine where a 128-unit trace from the gun hits); confirmed by one
//! mine fewer. The mine arms 2.5 s later and its beam runs along the wall's normal.

use lb_core::Vec3;
use lb_core::math::{dir_to_view_angles, view_angle_vectors};
use lb_core::time::SimTime;
use lb_game::mechanics::{Attack, TRIPMINE_REACH, Trigger};
use lb_game::weapons::WeaponId;
use lb_motor::LookIntent;
use lb_worldq::{TraceQuery, Tracer};

use super::{Hands, Request, Status, hold, press, settled, stop};

const TIMEOUT: f64 = 6.0;
/// The press is held until the game shows the mine placed, for this long at most.
const CONFIRM: f64 = 0.75;
/// The view's trace must meet the wall this close to the spot.
const ON_SPOT: f32 = 12.0;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    Draw,
    Pressed { at: SimTime, before: i32 },
}

#[derive(Clone, Debug)]
pub struct Planter {
    /// The point on the wall and the wall's normal.
    pub spot: Vec3,
    pub normal: Vec3,
    phase: Phase,
    started: SimTime,
}

impl Planter {
    pub fn new(spot: Vec3, normal: Vec3, now: SimTime) -> Planter {
        Planter {
            spot,
            normal,
            phase: Phase::Draw,
            started: now,
        }
    }

    /// Where the placed mine sits (the game puts it 8 units off the wall).
    pub fn mine(&self) -> Vec3 {
        self.spot + self.normal * 8.0
    }

    pub fn update(&mut self, h: &Hands<'_>, tracer: &mut dyn Tracer) -> Status {
        let now = h.now;
        let w = WeaponId::Tripmine;
        let count = h.reserve(w);
        let angles = dir_to_view_angles(self.spot - h.eye);
        match self.phase {
            Phase::Draw => {
                if now.since(self.started) > TIMEOUT || count <= 0 {
                    return Status::Failed("no tripmine placed");
                }
                let mut weapon = hold(w);
                if h.ready(w) && settled(h.view, angles, 3.0) && meets_spot(h, self.spot, tracer) {
                    weapon = press(w, Attack::Primary, Trigger::Hold, 0.0);
                    self.phase = Phase::Pressed { at: now, before: count };
                }
                Status::Running(Request {
                    weapon: Some(weapon),
                    look: Some(LookIntent::Angles(angles)),
                    movement: Some(stop()),
                })
            }
            Phase::Pressed { at, before } => {
                if count < before {
                    return Status::Done;
                }
                if now.since(at) > CONFIRM {
                    return Status::Failed("the game did not place the mine");
                }
                Status::Running(Request {
                    weapon: Some(press(w, Attack::Primary, Trigger::Hold, 0.0)),
                    look: Some(LookIntent::Angles(angles)),
                    movement: Some(stop()),
                })
            }
        }
    }
}

/// The game's own placement trace (128 units along the view from the gun) meets the wall at the spot.
fn meets_spot(h: &Hands<'_>, spot: Vec3, tracer: &mut dyn Tracer) -> bool {
    let (forward, _, _) = view_angle_vectors(h.view);
    let tr = tracer.trace(&TraceQuery::line(h.eye, h.eye + forward * TRIPMINE_REACH));
    tr.fraction < 1.0 && tr.end.distance(spot) <= ON_SPOT
}
