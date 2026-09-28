//! The MP5's grenade launcher: turn to the lobbed arc that lands on the target's feet and press the secondary attack
//! once; confirmed by one grenade fewer. The grenade explodes on contact, so the arc is looked along again from where
//! the bot is when it fires (it moves while it turns), and the shot is called off when the way closed meanwhile.

use lb_core::Vec3;
use lb_core::math::view_angle_vectors;
use lb_core::time::SimTime;
use lb_game::mechanics::{Attack, M203_SPEED, PROJECTILE_GRAVITY, Trigger};
use lb_game::weapons::WeaponId;
use lb_motor::LookIntent;
use lb_worldq::Tracer;

use super::{Hands, Request, Status, hold, press, settled};
use crate::ballistics::{Throw, arc_clear};

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

    /// `clear`: nobody stands near the way and the target has not come too close.
    pub fn update(&mut self, h: &Hands<'_>, tracer: &mut dyn Tracer, clear: bool) -> Status {
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
                    let (forward, _, _) = view_angle_vectors(angles);
                    let start = h.eye + forward * 16.0;
                    let gravity = h.gravity * PROJECTILE_GRAVITY;
                    if !clear || !arc_clear(tracer, start, forward * M203_SPEED, gravity, self.throw.flight) {
                        return Status::Failed("the way closed");
                    }
                    weapon = press(w, Attack::Secondary, Trigger::Hold, 0.0);
                    self.phase = Phase::Pressed { at: now, before: count };
                }
                Status::Running(Request {
                    weapon: Some(weapon),
                    look: Some(LookIntent::Angles(angles)),
                    movement: None,
                    jump: false,
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
                    jump: false,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::Armed;
    use lb_game::dll::DllProfile;
    use lb_game::self_state::{PredictedWeapon, Prediction};
    use lb_motor::Fire;
    use lb_worldq::{Trace, TraceQuery, contents};

    /// Open space, or a wall across x = `wall`.
    struct Room {
        wall: Option<f32>,
    }

    impl Tracer for Room {
        fn trace(&mut self, q: &TraceQuery) -> Trace {
            let mut t = Trace::clear(q.end);
            if let Some(x) = self.wall
                && (q.start.x - x) * (q.end.x - x) < 0.0
            {
                t.fraction = (x - q.start.x) / (q.end.x - q.start.x);
            }
            t
        }

        fn point_contents(&mut self, _p: Vec3) -> i32 {
            contents::EMPTY
        }
    }

    fn fire_with(wall: Option<f32>, clear: bool) -> Status {
        let arsenal = [Armed {
            reserve2: Some(2),
            ..Armed::new(WeaponId::Mp5, Some(30), Some(50))
        }];
        let mut prediction = Prediction {
            current: Some(WeaponId::Mp5),
            ..Prediction::default()
        };
        prediction.weapons[WeaponId::Mp5 as usize] = Some(PredictedWeapon::default());
        let h = Hands {
            now: SimTime(0.0),
            eye: Vec3::ZERO,
            origin: Vec3::ZERO,
            velocity: Vec3::ZERO,
            view: Vec3::new(-10.0, 0.0, 0.0),
            on_ground: true,
            on_ladder: false,
            waterlevel: 0,
            fov: 0.0,
            weapon: Some(WeaponId::Mp5),
            arsenal: &arsenal,
            prediction: Some(&prediction),
            dll: DllProfile::default(),
            gravity: 800.0,
        };
        let throw = Throw {
            pitch: -10.0,
            yaw: 0.0,
            flight: 0.7,
            start: Vec3::X * 16.0,
            velocity: Vec3::ZERO,
        };
        Lob::new(throw, SimTime(0.0)).update(&h, &mut Room { wall }, clear)
    }

    #[test]
    fn the_arc_is_looked_along_again_when_the_grenade_goes() {
        match fire_with(None, true) {
            Status::Running(r) => assert_eq!(r.weapon.map(|w| w.fire), Some(Fire::Secondary)),
            other => panic!("{other:?}"),
        }
        assert_eq!(
            fire_with(Some(60.0), true),
            Status::Failed("the way closed"),
            "a wall came in front"
        );
        assert_eq!(
            fire_with(None, false),
            Status::Failed("the way closed"),
            "someone stepped in"
        );
    }
}
