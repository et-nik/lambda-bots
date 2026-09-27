//! Setting off explosives the bot knows of when an enemy walks up to them.
//!
//! - **Satchels** (yapb's `DetonateSatchel`): draw the satchel (its radio while charges are out) and press the DLL's
//!   detonate button once. Confirmed when the game reports the charges gone off. If the press threw another satchel
//!   instead, the buttons are the other way round on this server: the protocol says so, so the profile can be
//!   swapped, and the other button is pressed.
//! - **Tripmines** (yapb's `DetonateTripmine`): shooting a mine sets it off and credits the shooter, whoever placed
//!   it. Aim at the mine with a hitscan gun and fire while the view is on it; given up after 4 s.

use lb_core::Vec3;
use lb_core::math::dir_to_view_angles;
use lb_core::time::SimTime;
use lb_game::mechanics::{Trigger, spec};
use lb_game::weapons::WeaponId;
use lb_motor::LookIntent;

use super::{Hands, Request, Status, hold, press, settled};

const DRAW_TIMEOUT: f64 = 2.0;
const CONFIRM: f64 = 0.6;
const SHOOT_FOR: f64 = 4.0;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    Draw,
    Pressed { at: SimTime, before: i32, swapped: bool },
}

#[derive(Clone, Debug)]
pub struct SatchelTrigger {
    phase: Phase,
    started: SimTime,
    /// The satchel buttons turned out swapped on this server.
    pub learned_swap: bool,
}

impl SatchelTrigger {
    pub fn new(now: SimTime) -> SatchelTrigger {
        SatchelTrigger {
            phase: Phase::Draw,
            started: now,
            learned_swap: false,
        }
    }

    pub fn update(&mut self, h: &Hands<'_>) -> Status {
        let now = h.now;
        let w = WeaponId::Satchel;
        let state = h.predicted(w).map_or(0, |p| p.charge_ready);
        let count = h.reserve(w);
        match self.phase {
            Phase::Draw => {
                if state != 1 {
                    return if state == 2 {
                        Status::Done
                    } else {
                        Status::Failed("no satchel out")
                    };
                }
                if now.since(self.started) > DRAW_TIMEOUT {
                    return Status::Failed("the satchel radio was not drawn");
                }
                let mut weapon = hold(w);
                if h.ready(w) {
                    weapon = press(w, h.dll.satchel_detonate(), Trigger::Tap, 1.0);
                    self.phase = Phase::Pressed {
                        at: now,
                        before: count,
                        swapped: false,
                    };
                }
                Status::Running(Request {
                    weapon: Some(weapon),
                    ..Request::default()
                })
            }
            Phase::Pressed { at, before, swapped } => {
                if state == 2 {
                    return Status::Done;
                }
                if count < before && !swapped {
                    // A satchel was thrown: the detonate button is the other one here.
                    self.learned_swap = true;
                    let mut dll = h.dll;
                    dll.swap_satchel_buttons();
                    self.phase = Phase::Pressed {
                        at: now,
                        before: count,
                        swapped: true,
                    };
                    return Status::Running(Request {
                        weapon: Some(press(w, dll.satchel_detonate(), Trigger::Tap, 1.0)),
                        ..Request::default()
                    });
                }
                if now.since(at) > CONFIRM {
                    return Status::Failed("the satchels did not go off");
                }
                Status::Running(Request {
                    weapon: Some(hold(w)),
                    ..Request::default()
                })
            }
        }
    }
}

/// Shooting a tripmine with `weapon`.
#[derive(Clone, Debug)]
pub struct MineShot {
    pub mine: Vec3,
    pub weapon: WeaponId,
    started: SimTime,
}

impl MineShot {
    pub fn new(mine: Vec3, weapon: WeaponId, now: SimTime) -> MineShot {
        MineShot {
            mine,
            weapon,
            started: now,
        }
    }

    /// `interval`: the pause between clicks of a semi-automatic gun.
    pub fn update(&mut self, h: &Hands<'_>, interval: f32) -> Status {
        if h.now.since(self.started) > SHOOT_FOR {
            return Status::Failed("the mine did not go off");
        }
        let angles = dir_to_view_angles(self.mine - h.eye);
        let s = spec(self.weapon);
        let weapon = if h.ready(self.weapon) && settled(h.view, angles, 0.8) {
            press(self.weapon, lb_game::mechanics::Attack::Primary, s.trigger, interval)
        } else {
            hold(self.weapon)
        };
        Status::Running(Request {
            weapon: Some(weapon),
            look: Some(LookIntent::Angles(angles)),
            movement: Some(super::stop()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::Armed;
    use lb_game::dll::DllProfile;
    use lb_game::mechanics::Attack;
    use lb_game::self_state::{PredictedWeapon, Prediction};
    use lb_motor::Fire;

    fn hands<'a>(t: f64, arsenal: &'a [Armed], prediction: &'a Prediction, dll: DllProfile) -> Hands<'a> {
        Hands {
            now: SimTime(t),
            eye: Vec3::ZERO,
            origin: Vec3::ZERO,
            velocity: Vec3::ZERO,
            view: Vec3::ZERO,
            on_ground: true,
            on_ladder: false,
            waterlevel: 0,
            weapon: Some(WeaponId::Satchel),
            arsenal,
            prediction: Some(prediction),
            dll,
            gravity: 800.0,
        }
    }

    fn predicted(state: i32) -> Prediction {
        let mut p = Prediction {
            current: Some(WeaponId::Satchel),
            ..Prediction::default()
        };
        p.weapons[WeaponId::Satchel as usize] = Some(PredictedWeapon {
            charge_ready: state,
            ..PredictedWeapon::default()
        });
        p
    }

    #[test]
    fn a_wrong_detonate_button_is_learned() {
        // The profile says BHL (secondary sets them off) but the server is classic: the secondary throws.
        let dll = DllProfile::default();
        assert_eq!(dll.satchel_detonate(), Attack::Secondary);
        let mut trigger = SatchelTrigger::new(SimTime(0.0));
        let arsenal = [Armed::new(WeaponId::Satchel, None, Some(3))];
        let out = predicted(1);
        let Status::Running(r) = trigger.update(&hands(0.0, &arsenal, &out, dll)) else {
            panic!()
        };
        assert_eq!(r.weapon.unwrap().fire, Fire::Secondary);
        let fewer = [Armed::new(WeaponId::Satchel, None, Some(2))];
        let Status::Running(r) = trigger.update(&hands(0.1, &fewer, &out, dll)) else {
            panic!()
        };
        assert!(trigger.learned_swap);
        assert_eq!(r.weapon.unwrap().fire, Fire::Primary, "the other button");
        let gone = predicted(2);
        assert_eq!(trigger.update(&hands(0.2, &fewer, &gone, dll)), Status::Done);
    }
}
