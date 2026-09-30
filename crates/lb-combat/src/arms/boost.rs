//! The weapons' part of a gauss boost (yapb's gauss jump): draw the gauss, turn the view back the way the bot is to
//! fly and down, charge the gauss for as long as navigation asks (fully, or the share of a full charge the flight
//! takes), jump, and let the charge go as the bot leaves the ground. The recoil of a charged shot pushes its shooter
//! back at five times its damage, up too in multiplayer, and throws it the way it looks away from. The view comes
//! round before the charge starts, so a partial charge goes on time. Navigation stops the bot at the takeoff and asks
//! for the boost; it steers the flight after.
//!
//! The protocol holds the weapon, the look, the movement and the jump until the charge is gone, so a fight that
//! starts meanwhile does not fire it the wrong way. Called off before the jump (navigation no longer asks), a charge
//! already building is handed to the gauss's own protocol, which dumps it safely or fires it at a target.

use lb_core::Vec3;
use lb_core::time::SimTime;
use lb_game::mechanics::{Attack, Trigger};
use lb_game::weapons::WeaponId;
use lb_motor::LookIntent;

use super::{Hands, Request, Status, hold, press, settled, stop};

/// The gauss comes out within this long, or the boost is off.
const DRAW_WITHIN: f64 = 2.0;
/// The charge starts within this long of the press (no ammo, under water: it will not); the trigger may still be
/// held off for the deploy.
const START_WITHIN: f64 = 1.0;
/// The view is this close to the boost's (degrees, both ways) for the jump.
const AIM: f32 = 2.0;
/// Waiting this long for the view is enough: a little off still flies.
const AIM_WAIT: f64 = 1.2;
/// The charge goes this long after the jump at the latest, off the ground or not.
const LEAVE_WITHIN: f64 = 0.25;
/// The buttons stay up this long for the game to fire.
const RELEASE_FOR: f64 = 0.2;
/// A charged gauss shocks its holder after 10 s: the boost gives up well before.
const HOLD_LIMIT: f64 = 6.0;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    Draw,
    /// The gauss is out: the view comes round to the boost's.
    Aim {
        since: SimTime,
    },
    Charge {
        pressed: SimTime,
        started: Option<SimTime>,
    },
    Jump {
        at: SimTime,
    },
    Release {
        at: SimTime,
    },
}

#[derive(Clone, Debug)]
pub struct GaussBoost {
    phase: Phase,
    since: SimTime,
    /// View angles to let the charge go along.
    pub view: Vec3,
    /// Seconds the charge builds, from the press.
    pub charge: f32,
    /// The charge started spinning then, and is still held: it is handed on when the boost is called off.
    pub held: Option<SimTime>,
    /// When the charge went: the boost is done.
    pub fired: Option<SimTime>,
}

impl GaussBoost {
    pub fn new(now: SimTime, view: Vec3, charge: f32) -> GaussBoost {
        GaussBoost {
            phase: Phase::Draw,
            since: now,
            view,
            charge,
            held: None,
            fired: None,
        }
    }

    pub fn phase(&self) -> &'static str {
        match self.phase {
            Phase::Draw => "draw",
            Phase::Aim { .. } => "aim",
            Phase::Charge { .. } => "charge",
            Phase::Jump { .. } => "jump",
            Phase::Release { .. } => "release",
        }
    }

    /// `go`: navigation still asks for the boost (the bot stands at the takeoff). The view the boost asks for may
    /// be updated meanwhile.
    pub fn update(&mut self, h: &Hands<'_>, go: bool, view: Option<Vec3>) -> Status {
        let now = h.now;
        if let Some(v) = view {
            self.view = v;
        }
        let look = Some(LookIntent::Angles(self.view));
        let charging = Request {
            weapon: Some(press(WeaponId::Gauss, Attack::Secondary, Trigger::Hold, 0.0)),
            look,
            movement: Some(stop()),
            jump: false,
        };
        let in_hand = h.weapon == Some(WeaponId::Gauss);
        if !in_hand && !matches!(self.phase, Phase::Draw) {
            self.held = None;
            return Status::Failed("the gauss left the hand");
        }
        let holding = Request {
            weapon: Some(hold(WeaponId::Gauss)),
            look,
            movement: Some(stop()),
            jump: false,
        };
        match self.phase {
            Phase::Draw => {
                if !go {
                    return Status::Failed("called off");
                }
                if h.ready(WeaponId::Gauss) {
                    self.phase = Phase::Aim { since: now };
                    return self.update(h, go, None);
                }
                if now.since(self.since) > DRAW_WITHIN {
                    return Status::Failed("the gauss did not come out");
                }
                Status::Running(holding)
            }
            Phase::Aim { since } => {
                if !go {
                    return Status::Failed("called off");
                }
                if settled(h.view, self.view, AIM) || now.since(since) > AIM_WAIT {
                    self.phase = Phase::Charge {
                        pressed: now,
                        started: None,
                    };
                    return Status::Running(charging);
                }
                Status::Running(holding)
            }
            Phase::Charge { pressed, started } => {
                let spinning = h.predicted(WeaponId::Gauss).is_some_and(|p| p.in_attack != 0);
                let started = match started {
                    Some(t) => t,
                    None if spinning => now,
                    None if now.since(pressed) > START_WITHIN => return Status::Failed("the gauss would not charge"),
                    None => return Status::Running(charging),
                };
                self.held = Some(started);
                self.phase = Phase::Charge {
                    pressed,
                    started: Some(started),
                };
                if !go {
                    return Status::Failed("called off");
                }
                // The game counts the charge from the first command with the button down, sent soon after the press.
                let age = now.since(pressed);
                if now.since(started) > HOLD_LIMIT {
                    return Status::Failed("the bot never left the ground");
                }
                if age >= f64::from(self.charge) && h.on_ground {
                    self.phase = Phase::Jump { at: now };
                    return Status::Running(Request { jump: true, ..charging });
                }
                Status::Running(charging)
            }
            Phase::Jump { at } => {
                if !h.on_ground || now.since(at) > LEAVE_WITHIN {
                    self.phase = Phase::Release { at: now };
                    self.held = None;
                    self.fired = Some(now);
                    return Status::Running(Request {
                        weapon: Some(hold(WeaponId::Gauss)),
                        look,
                        movement: None,
                        jump: false,
                    });
                }
                Status::Running(Request { jump: true, ..charging })
            }
            Phase::Release { at } => {
                let gone = h.predicted(WeaponId::Gauss).is_none_or(|p| p.in_attack == 0);
                if now.since(at) >= RELEASE_FOR && (gone || now.since(at) >= 3.0 * RELEASE_FOR) {
                    return Status::Done;
                }
                Status::Running(Request {
                    weapon: Some(hold(WeaponId::Gauss)),
                    look,
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

    struct Bot {
        weapon: Option<WeaponId>,
        spinning: bool,
        on_ground: bool,
        view: Vec3,
    }

    fn hands<'a>(t: f64, b: &Bot, arsenal: &'a [Armed], prediction: &'a Prediction) -> Hands<'a> {
        Hands {
            now: SimTime(t),
            eye: Vec3::ZERO,
            origin: Vec3::ZERO,
            velocity: Vec3::ZERO,
            view: b.view,
            on_ground: b.on_ground,
            on_ladder: false,
            waterlevel: 0,
            fov: 0.0,
            weapon: b.weapon,
            arsenal,
            prediction: Some(prediction),
            dll: DllProfile::default(),
            gravity: 800.0,
        }
    }

    fn prediction(b: &Bot) -> Prediction {
        let mut p = Prediction {
            current: b.weapon,
            primary_ammo: 60,
            ..Prediction::default()
        };
        p.weapons[WeaponId::Gauss as usize] = Some(PredictedWeapon {
            in_attack: i32::from(b.spinning),
            ..PredictedWeapon::default()
        });
        p
    }

    #[test]
    fn draws_charges_turns_jumps_and_lets_go_off_the_ground() {
        let arsenal = [Armed::new(WeaponId::Gauss, None, Some(60))];
        let view = Vec3::new(34.0, 180.0, 0.0);
        let mut boost = GaussBoost::new(SimTime(0.0), view, 1.6);
        let mut b = Bot {
            weapon: Some(WeaponId::Crowbar),
            spinning: false,
            on_ground: true,
            view: Vec3::ZERO,
        };
        let mut jumped_at = None;
        let mut released_at = None;
        let mut t = 0.0;
        while t < 5.0 {
            let p = prediction(&b);
            let status = boost.update(&hands(t, &b, &arsenal, &p), true, None);
            let r = match status {
                Status::Running(r) => r,
                Status::Done => break,
                Status::Failed(why) => panic!("{why} at {t:.2}"),
            };
            // The game: the gauss comes out, spins while the secondary is held, fires when it is let go.
            if r.weapon.is_some_and(|w| w.select == Some(WeaponId::Gauss)) && t > 0.3 {
                b.weapon = Some(WeaponId::Gauss);
            }
            let fire = r.weapon.map_or(Fire::None, |w| w.fire);
            if fire == Fire::Secondary && b.weapon == Some(WeaponId::Gauss) {
                assert!(
                    b.spinning || settled(b.view, view, AIM),
                    "turned round before the charge: {}",
                    b.view
                );
                b.spinning = true;
            } else if b.spinning && fire == Fire::None {
                b.spinning = false;
                released_at.get_or_insert(t);
                assert!(!b.on_ground, "let go in the air");
            }
            if let Some(LookIntent::Angles(a)) = r.look {
                b.view = b.view.lerp(a, 0.05);
            }
            if r.jump {
                jumped_at.get_or_insert(t);
                if t - jumped_at.unwrap_or(t) > 0.02 {
                    b.on_ground = false;
                }
            }
            t += 0.01;
        }
        let (jumped, released) = (jumped_at.expect("jumped"), released_at.expect("let go"));
        assert!(jumped >= 1.6 + 0.3, "charged fully first: jumped at {jumped:.2}");
        assert!(
            released > jumped && released - jumped < 0.3,
            "{jumped:.2} {released:.2}"
        );
        assert!(settled(b.view, view, 3.0), "{}", b.view);
    }

    #[test]
    fn a_partial_charge_goes_its_seconds_after_the_press() {
        let arsenal = [Armed::new(WeaponId::Gauss, None, Some(60))];
        let view = Vec3::new(40.0, 90.0, 0.0);
        let mut boost = GaussBoost::new(SimTime(0.0), view, 0.7);
        let mut b = Bot {
            weapon: Some(WeaponId::Gauss),
            spinning: false,
            on_ground: true,
            view,
        };
        let mut pressed_at = None;
        let mut t = 0.0;
        let jumped = loop {
            assert!(t < 2.0, "no jump");
            let p = prediction(&b);
            let Status::Running(r) = boost.update(&hands(t, &b, &arsenal, &p), true, None) else {
                panic!("stopped at {t:.2}");
            };
            if r.weapon.is_some_and(|w| w.fire == Fire::Secondary) {
                pressed_at.get_or_insert(t);
                b.spinning = true;
            }
            if r.jump {
                break t;
            }
            t += 0.01;
        };
        let pressed = pressed_at.expect("charged");
        assert!(
            (jumped - pressed - 0.7).abs() < 0.015,
            "pressed {pressed:.2}, jumped {jumped:.2}"
        );
    }

    #[test]
    fn called_off_while_charging_hands_the_charge_on() {
        let arsenal = [Armed::new(WeaponId::Gauss, None, Some(60))];
        let view = Vec3::new(34.0, 180.0, 0.0);
        let mut boost = GaussBoost::new(SimTime(0.0), view, 1.6);
        let mut b = Bot {
            weapon: Some(WeaponId::Gauss),
            spinning: false,
            on_ground: true,
            view,
        };
        let p = prediction(&b);
        assert!(matches!(
            boost.update(&hands(0.0, &b, &arsenal, &p), true, None),
            Status::Running(_)
        ));
        b.spinning = true;
        let p = prediction(&b);
        assert!(matches!(
            boost.update(&hands(0.1, &b, &arsenal, &p), true, None),
            Status::Running(_)
        ));
        let p = prediction(&b);
        assert_eq!(
            boost.update(&hands(0.5, &b, &arsenal, &p), false, None),
            Status::Failed("called off")
        );
        assert_eq!(
            boost.held,
            Some(SimTime(0.1)),
            "the charge is still held, to be handed on"
        );
    }
}
