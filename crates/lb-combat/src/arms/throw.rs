//! Throwing hand grenades, satchels and snarks.
//!
//! - **Grenade:** press the primary attack; the pin is out when the game reports the throw started (its own clock,
//!   so the fuse is known to the frame). Hold it to cook the grenade so it goes off soon after landing, turn to the
//!   throw solved for the target, stop for the last moment, and let go: the game throws on its next idle frame.
//!   Once the pin is out the grenade is always thrown, at the latest shortly before the fuse runs out.
//! - **Satchel:** draw it (a second), turn to the throw, press the DLL's throw button once; confirmed when the game
//!   reports a charge out and one satchel fewer. A charge the game did not report is tried once more.
//! - **Snark:** draw, turn to the target (14 units above its origin, as yapb), press once when there is room in front
//!   (the game's own check); confirmed by one snark fewer.

use lb_core::Vec3;
use lb_core::math::{dir_to_view_angles, view_angle_vectors};
use lb_core::time::SimTime;
use lb_game::mechanics::{Attack, GRENADE_FUSE, GRENADE_MIN_COOK, Trigger};
use lb_game::weapons::WeaponId;
use lb_motor::LookIntent;
use lb_worldq::{TraceQuery, Tracer};

use super::{Hands, Request, Status, hold, press, settled, stop};
use crate::ballistics::{self, Throw, Unchecked};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Grenade,
    Satchel,
    Snark,
}

impl Kind {
    pub fn weapon(self) -> WeaponId {
        match self {
            Kind::Grenade => WeaponId::HandGrenade,
            Kind::Satchel => WeaponId::Satchel,
            Kind::Snark => WeaponId::Snark,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Grenade => "grenade",
            Kind::Satchel => "satchel",
            Kind::Snark => "snark",
        }
    }
}

/// The grenade lands this long before it goes off.
const LAND_BEFORE_FUSE: f32 = 0.6;
/// Longest cooking: the rest of the fuse is for turning and flying.
const MAX_COOK: f32 = 2.0;
/// The grenade is let go by this long before the fuse runs out, whatever the view.
const FUSE_MARGIN: f32 = 0.4;
/// The throw is solved again this often while turning (the bot's own motion goes into it).
const RESOLVE: f64 = 0.1;
/// Stop moving this long before the grenade leaves the hand.
const STOP_BEFORE: f32 = 0.25;
/// The view is held on the throw this long before letting go.
const STEADY: f64 = 0.05;
const DRAW_TIMEOUT: f64 = 2.5;
/// A press is held until the game shows its effect, for this long at most.
const CONFIRM: f64 = 0.8;
const SATCHEL_CONFIRM: f64 = 1.5;
/// A snark leaves this far in front of the thrower; the game wants free space there.
const SNARK_ROOM: [f32; 2] = [20.0, 64.0];
const SNARK_AIM_UP: f32 = 14.0;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    /// Drawing the weapon (and, for a grenade, pulling the pin).
    Draw,
    /// The pin is out since the game's time `pin`; turning to the throw.
    Cook { pin: f64 },
    /// Let go at `at`; waiting for the game to throw.
    Released { at: SimTime },
    /// Pressing `button` since `at`, with `before` of the weapon's ammo, until the game shows the throw.
    Pressed { at: SimTime, before: i32, button: Attack },
}

#[derive(Clone, Debug)]
pub struct Thrower {
    pub kind: Kind,
    /// Where the throw should land (grenade, satchel), or what the snark is thrown at.
    pub target: Vec3,
    throw: Throw,
    phase: Phase,
    started: SimTime,
    resolved_at: SimTime,
    steady_since: Option<SimTime>,
    /// Let go as soon as the game allows: the target is in sight and will not wait for a cooked grenade.
    quick: bool,
}

impl Thrower {
    /// A throw of `kind` at `target`, solved as `throw` (for a snark only its angles are used).
    pub fn new(kind: Kind, target: Vec3, throw: Throw, now: SimTime) -> Thrower {
        Thrower {
            kind,
            target,
            throw,
            phase: Phase::Draw,
            started: now,
            resolved_at: now,
            steady_since: None,
            quick: false,
        }
    }

    /// Thrown as soon as the game allows, without cooking.
    pub fn quick(mut self) -> Thrower {
        self.quick = true;
        self
    }

    /// Where the thrown grenade or satchel should come down.
    pub fn landing(&self, sv_gravity: f32) -> Vec3 {
        let t = &self.throw;
        ballistics::position_at(
            t.start,
            t.velocity,
            sv_gravity * lb_game::mechanics::PROJECTILE_GRAVITY,
            t.flight,
        )
    }

    /// The pin is out: the grenade must be thrown whatever else comes up.
    pub fn committed(&self) -> bool {
        matches!(self.phase, Phase::Cook { .. } | Phase::Released { .. })
    }

    pub fn phase(&self) -> &'static str {
        match self.phase {
            Phase::Draw => "draw",
            Phase::Cook { .. } => "cook",
            Phase::Released { .. } => "released",
            Phase::Pressed { .. } => "pressed",
        }
    }

    fn angles(&self) -> Vec3 {
        Vec3::new(self.throw.pitch, self.throw.yaw, 0.0)
    }

    pub fn update(&mut self, h: &Hands<'_>, tracer: &mut dyn Tracer) -> Status {
        match self.kind {
            Kind::Grenade => self.grenade(h),
            Kind::Satchel => self.satchel(h),
            Kind::Snark => self.snark(h, tracer),
        }
    }

    fn grenade(&mut self, h: &Hands<'_>) -> Status {
        let now = h.now;
        let w = WeaponId::HandGrenade;
        let pin = h.predicted(w).map(|p| f64::from(p.start_throw)).filter(|t| *t > 0.0);
        match self.phase {
            Phase::Draw => {
                if let Some(pin) = pin {
                    self.phase = Phase::Cook { pin };
                } else if now.since(self.started) > DRAW_TIMEOUT || (h.weapon == Some(w) && h.reserve(w) <= 0) {
                    return Status::Failed("no grenade in hand");
                }
                Status::Running(Request {
                    weapon: Some(press(w, Attack::Primary, Trigger::Hold, 0.0)),
                    ..Request::default()
                })
            }
            Phase::Cook { pin } => {
                let held = (now.secs() - pin) as f32;
                let cook = if self.quick {
                    GRENADE_MIN_COOK
                } else {
                    (GRENADE_FUSE - self.throw.flight - LAND_BEFORE_FUSE).clamp(GRENADE_MIN_COOK, MAX_COOK)
                };
                let deadline = GRENADE_FUSE - FUSE_MARGIN;
                if now.since(self.resolved_at) >= RESOLVE {
                    self.resolved_at = now;
                    if let Some(t) =
                        ballistics::grenade(&mut Unchecked, h.eye, h.velocity, self.target, h.gravity, h.dll, 3.0)
                    {
                        self.throw = t;
                    }
                }
                let angles = self.angles();
                if settled(h.view, angles, 1.5) {
                    self.steady_since.get_or_insert(now);
                } else {
                    self.steady_since = None;
                }
                let steady = self.steady_since.is_some_and(|t| now.since(t) >= STEADY);
                let go = held >= deadline || (held >= cook && steady);
                if go {
                    self.phase = Phase::Released { at: now };
                    return Status::Running(Request {
                        weapon: Some(hold(w)),
                        look: Some(LookIntent::Angles(angles)),
                        movement: Some(stop()),
                    });
                }
                Status::Running(Request {
                    weapon: Some(press(w, Attack::Primary, Trigger::Hold, 0.0)),
                    look: Some(LookIntent::Angles(angles)),
                    movement: (held >= cook - STOP_BEFORE).then(stop),
                })
            }
            Phase::Released { at } => {
                if pin.is_none() || now.since(at) >= CONFIRM {
                    return Status::Done;
                }
                Status::Running(Request {
                    weapon: Some(hold(w)),
                    look: Some(LookIntent::Angles(self.angles())),
                    movement: Some(stop()),
                })
            }
            Phase::Pressed { .. } => Status::Done,
        }
    }

    fn satchel(&mut self, h: &Hands<'_>) -> Status {
        let now = h.now;
        let w = WeaponId::Satchel;
        let out = h.predicted(w).is_some_and(|p| p.charge_ready == 1);
        let count = h.reserve(w);
        match self.phase {
            Phase::Draw => {
                if now.since(self.started) > DRAW_TIMEOUT + 1.0 || count <= 0 {
                    return Status::Failed("no satchel to throw");
                }
                if now.since(self.resolved_at) >= RESOLVE {
                    self.resolved_at = now;
                    self.throw = ballistics::satchel(&mut Unchecked, h.origin, h.velocity, self.target, h.gravity);
                }
                let angles = self.angles();
                let mut weapon = hold(w);
                if h.ready(w) && settled(h.view, angles, 3.0) {
                    let button = if out {
                        h.dll.satchel_throw_more()
                    } else {
                        h.dll.satchel_throw()
                    };
                    weapon = press(w, button, Trigger::Hold, 0.0);
                    self.phase = Phase::Pressed {
                        at: now,
                        before: count,
                        button,
                    };
                }
                Status::Running(Request {
                    weapon: Some(weapon),
                    look: Some(LookIntent::Angles(angles)),
                    movement: None,
                })
            }
            Phase::Pressed { at, before, button } => {
                if out && count < before {
                    return Status::Done;
                }
                if now.since(at) >= SATCHEL_CONFIRM {
                    return Status::Failed("the game did not throw the satchel");
                }
                Status::Running(Request {
                    weapon: Some(press(w, button, Trigger::Hold, 0.0)),
                    look: Some(LookIntent::Angles(self.angles())),
                    movement: None,
                })
            }
            Phase::Cook { .. } | Phase::Released { .. } => Status::Done,
        }
    }

    fn snark(&mut self, h: &Hands<'_>, tracer: &mut dyn Tracer) -> Status {
        let now = h.now;
        let w = WeaponId::Snark;
        let count = h.reserve(w);
        let aim = self.target + Vec3::Z * SNARK_AIM_UP;
        let angles = dir_to_view_angles(aim - h.eye);
        match self.phase {
            Phase::Draw => {
                if now.since(self.started) > DRAW_TIMEOUT || count <= 0 {
                    return Status::Failed("no snark to throw");
                }
                let mut weapon = hold(w);
                if h.ready(w) && settled(h.view, angles, 5.0) && room_ahead(h, tracer) {
                    weapon = press(w, Attack::Primary, Trigger::Hold, 0.0);
                    self.phase = Phase::Pressed {
                        at: now,
                        before: count,
                        button: Attack::Primary,
                    };
                }
                Status::Running(Request {
                    weapon: Some(weapon),
                    look: Some(LookIntent::Angles(angles)),
                    movement: None,
                })
            }
            Phase::Pressed { at, before, button } => {
                if count < before {
                    return Status::Done;
                }
                if now.since(at) >= CONFIRM {
                    return Status::Failed("the game did not release the snark");
                }
                Status::Running(Request {
                    weapon: Some(press(w, button, Trigger::Hold, 0.0)),
                    look: Some(LookIntent::Angles(angles)),
                    movement: None,
                })
            }
            Phase::Cook { .. } | Phase::Released { .. } => Status::Done,
        }
    }
}

/// The game releases a snark only with free space 20–64 units in front of the thrower (`CSqueak::PrimaryAttack`).
fn room_ahead(h: &Hands<'_>, tracer: &mut dyn Tracer) -> bool {
    let (forward, _, _) = view_angle_vectors(h.view);
    let tr = tracer.trace(&TraceQuery::line(
        h.origin + forward * SNARK_ROOM[0],
        h.origin + forward * SNARK_ROOM[1],
    ));
    !tr.start_solid && !tr.all_solid && tr.fraction > 0.25
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::Armed;
    use lb_game::dll::DllProfile;
    use lb_game::self_state::{PredictedWeapon, Prediction};
    use lb_motor::Fire;

    struct Scene {
        view: Vec3,
        arsenal: Vec<Armed>,
        prediction: Prediction,
    }

    impl Scene {
        fn hands(&self, t: f64, weapon: WeaponId) -> Hands<'_> {
            Hands {
                now: SimTime(t),
                eye: Vec3::new(0.0, 0.0, 28.0),
                origin: Vec3::ZERO,
                velocity: Vec3::ZERO,
                view: self.view,
                on_ground: true,
                on_ladder: false,
                waterlevel: 0,
                fov: 0.0,
                weapon: Some(weapon),
                arsenal: &self.arsenal,
                prediction: Some(&self.prediction),
                dll: DllProfile::default(),
                gravity: 800.0,
            }
        }
    }

    #[test]
    fn a_pulled_grenade_is_cooked_aimed_and_let_go_in_time() {
        let target = Vec3::new(500.0, 100.0, -36.0);
        let throw = ballistics::grenade(
            &mut Unchecked,
            Vec3::new(0.0, 0.0, 28.0),
            Vec3::ZERO,
            target,
            800.0,
            DllProfile::default(),
            3.0,
        )
        .unwrap();
        let mut th = Thrower::new(Kind::Grenade, target, throw, SimTime(1.0));
        let mut scene = Scene {
            view: Vec3::ZERO,
            arsenal: vec![Armed::new(WeaponId::HandGrenade, None, Some(3))],
            prediction: Prediction {
                current: Some(WeaponId::HandGrenade),
                primary_ammo: 3,
                ..Prediction::default()
            },
        };
        let mut pin = None;
        let mut released = None;
        let mut t = 1.0;
        while t < 5.0 {
            let status = th.update(&scene.hands(t, WeaponId::HandGrenade), &mut Unchecked);
            let Status::Running(r) = status else { break };
            let fire = r.weapon.map_or(Fire::None, |w| w.fire);
            if fire == Fire::Primary && pin.is_none() {
                pin = Some(t);
            }
            if fire == Fire::None && pin.is_some() && released.is_none() {
                released = Some((t, scene.view));
            }
            if let Some(LookIntent::Angles(a)) = r.look {
                // The view closes in on what is asked at 180 degrees per second.
                let d = a - scene.view;
                scene.view += d.clamp_length_max(1.8);
            }
            let start = pin.map_or(0.0, |p| p as f32);
            scene.prediction.weapons[WeaponId::HandGrenade as usize] = Some(PredictedWeapon {
                start_throw: if released.is_some() { 0.0 } else { start },
                ..PredictedWeapon::default()
            });
            t += 0.01;
        }
        let (at, view) = released.expect("thrown");
        let held = at - pin.unwrap();
        assert!((0.5..=2.7).contains(&held), "cooked {held} s");
        assert!(
            settled(view, Vec3::new(th.throw.pitch, th.throw.yaw, 0.0), 1.5),
            "aimed: {view}"
        );
        assert_eq!(
            th.update(&scene.hands(t, WeaponId::HandGrenade), &mut Unchecked),
            Status::Done
        );
    }

    #[test]
    fn satchels_are_confirmed_by_the_game() {
        let target = Vec3::new(150.0, 0.0, -36.0);
        let throw = ballistics::satchel(&mut Unchecked, Vec3::ZERO, Vec3::ZERO, target, 800.0);
        let mut th = Thrower::new(Kind::Satchel, target, throw, SimTime(0.0));
        let mut scene = Scene {
            view: Vec3::new(throw.pitch, throw.yaw, 0.0),
            arsenal: vec![Armed::new(WeaponId::Satchel, None, Some(2))],
            prediction: Prediction {
                current: Some(WeaponId::Satchel),
                primary_ammo: 2,
                ..Prediction::default()
            },
        };
        scene.prediction.weapons[WeaponId::Satchel as usize] = Some(PredictedWeapon::default());
        let Status::Running(r) = th.update(&scene.hands(0.0, WeaponId::Satchel), &mut Unchecked) else {
            panic!()
        };
        assert_eq!(
            r.weapon.unwrap().fire,
            Fire::Primary,
            "BHL throws with the primary attack"
        );
        assert!(matches!(
            th.update(&scene.hands(0.5, WeaponId::Satchel), &mut Unchecked),
            Status::Running(_)
        ));
        scene.arsenal[0].reserve = Some(1);
        scene.prediction.primary_ammo = 1;
        scene.prediction.weapons[WeaponId::Satchel as usize] = Some(PredictedWeapon {
            charge_ready: 1,
            ..PredictedWeapon::default()
        });
        assert_eq!(
            th.update(&scene.hands(0.6, WeaponId::Satchel), &mut Unchecked),
            Status::Done
        );
    }
}
