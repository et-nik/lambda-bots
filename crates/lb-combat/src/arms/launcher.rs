//! The MP5's grenade launcher: turn to the lobbed arc that lands on the target's feet and press the secondary attack
//! once; confirmed by one grenade fewer. The grenade explodes on contact.

use lb_core::Vec3;
use lb_core::time::SimTime;
use lb_game::mechanics::{Attack, Trigger};
use lb_game::weapons::WeaponId;
use lb_motor::LookIntent;

use super::{Hands, Request, Status, hold, press, settled};
use crate::ballistics::Throw;

const TIMEOUT: f64 = 1.5;
/// The press is held until the game shows the grenade gone, for this long at most.
const CONFIRM: f64 = 0.6;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    Aim,
    Pressed { at: SimTime, before: i32 },
}

#[derive(Clone, Debug)]
pub struct Lob {
    pub throw: Throw,
    phase: Phase,
    started: SimTime,
}

impl Lob {
    pub fn new(throw: Throw, now: SimTime) -> Lob {
        Lob {
            throw,
            phase: Phase::Aim,
            started: now,
        }
    }

    pub fn update(&mut self, h: &Hands<'_>) -> Status {
        let now = h.now;
        let w = WeaponId::Mp5;
        let count = h.armed(w).and_then(|a| a.reserve2).unwrap_or(0);
        let angles = Vec3::new(self.throw.pitch, self.throw.yaw, 0.0);
        match self.phase {
            Phase::Aim => {
                if now.since(self.started) > TIMEOUT || count <= 0 {
                    return Status::Failed("no grenade launched");
                }
                let mut weapon = hold(w);
                if h.ready(w) && settled(h.view, angles, 1.5) {
                    weapon = press(w, Attack::Secondary, Trigger::Hold, 0.0);
                    self.phase = Phase::Pressed { at: now, before: count };
                }
                Status::Running(Request {
                    weapon: Some(weapon),
                    look: Some(LookIntent::Angles(angles)),
                    movement: None,
                })
            }
            Phase::Pressed { at, before } => {
                if count < before {
                    return Status::Done;
                }
                if now.since(at) > CONFIRM {
                    return Status::Failed("the game did not launch the grenade");
                }
                Status::Running(Request {
                    weapon: Some(press(w, Attack::Secondary, Trigger::Hold, 0.0)),
                    look: Some(LookIntent::Angles(angles)),
                    movement: None,
                })
            }
        }
    }
}
