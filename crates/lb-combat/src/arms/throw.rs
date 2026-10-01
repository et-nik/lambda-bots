//! Throwing hand grenades, satchels and snarks.
//!
//! - **Grenade:** press the primary attack; the pin is out when the game reports the throw started (its own clock,
//!   so the fuse is known to the frame). Hold it to cook the grenade so it goes off soon after landing, turn to the
//!   throw solved for the target, stop for the last moment, and let go: the game throws on its next idle frame.
//!   Once the pin is out the grenade is always thrown, at the latest shortly before the fuse runs out.
//! - **Satchel:** draw it (a second), turn to the throw, press the DLL's throw button once the game takes it;
//!   confirmed when the game reports a charge out and one satchel fewer. A pile is thrown one after another as the
//!   game allows (a second apart), each at the same spot. Thrown from a jump, the bot runs at the target, jumps, and
//!   presses the button a moment after its feet leave the ground: the jump's lift and the run go into the throw. A
//!   press that did nothing is followed by the other button; what the presses showed of the server's satchel buttons
//!   (a throw with charges out, a button that did nothing, or one that set the charges off) is reported.
//! - **Snark:** draw, turn to the target (14 units above its origin, as yapb), press once when there is room in front
//!   (the game's own check); confirmed by one snark fewer.
//! - **Snark barrage** ([`Barrage`]): at an enemy close by, all the snarks: held down, the game lets one go every 0.3 s
//!   while there is room in front, and they swarm the enemy. At an enemy in sight further off, a few of them the same
//!   way (a stream).

use lb_core::Vec3;
use lb_core::math::{dir_to_view_angles, view_angle_vectors};
use lb_core::time::SimTime;
use lb_game::mechanics::{Attack, GRENADE_FUSE, GRENADE_MIN_COOK, Trigger};
use lb_game::weapons::WeaponId;
use lb_knowledge::PlayerKey;
use lb_motor::{LookIntent, MoveIntent, WeaponIntent};
use lb_worldq::{TraceQuery, Tracer};

use super::{Hands, Request, Status, hold, press, settled, stop, takes};
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
/// A satchel press that shows nothing by then did nothing.
const NO_EFFECT: f64 = 0.3;
/// A snark leaves this far in front of the thrower; the game wants free space there.
const SNARK_ROOM: [f32; 2] = [20.0, 64.0];
const SNARK_AIM_UP: f32 = 14.0;
/// A satchel from a jump leaves this long after the feet leave the ground; the jump waits this long for them to.
const JUMP_THROW: f64 = 0.12;
const JUMP_GIVE_UP: f64 = 0.4;
/// Before a jump throw the bot runs at the target until this fast toward it, for this long at most.
const RUN_UP_SPEED: f32 = 320.0;
const RUN_UP_MIN: f32 = 200.0;
const RUN_UP_FOR: f64 = 0.8;
/// A snark barrage lasts this long at most, and ends when the enemy is out of sight this long.
const BARRAGE_FOR: f64 = 6.0;
const BARRAGE_LOST: f64 = 0.4;

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
    /// Jumped at `at` to throw a satchel from the air.
    Jump { at: SimTime },
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
    /// Satchels still to throw at the target, this one included.
    pile: u32,
    /// Satchels are thrown from a jump.
    jump: bool,
    /// Where the satchels thrown so far should land.
    pub landings: Vec<Vec3>,
    /// The satchel button that throws here, once one did or the one the DLL profile names did nothing.
    button: Option<Attack>,
    /// Satchels were out when the button was pressed.
    pressed_out: bool,
    /// What a press showed of the server's satchel buttons: the one that sets the charges off. Taken by the caller.
    pub learned: Option<Attack>,
    /// A press set the satchels out off instead of throwing another.
    pub set_off: bool,
    /// When a grenade's pin came out, by the game's clock.
    pin: Option<f64>,
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
            pile: 1,
            jump: false,
            landings: Vec::new(),
            button: None,
            pressed_out: false,
            learned: None,
            set_off: false,
            pin: None,
        }
    }

    /// Thrown as soon as the game allows, without cooking.
    pub fn quick(mut self) -> Thrower {
        self.quick = true;
        self
    }

    /// `n` satchels thrown one after another at the target.
    pub fn pile(mut self, n: u32) -> Thrower {
        self.pile = n.max(1);
        self
    }

    /// Satchels thrown from a jump.
    pub fn from_jump(mut self) -> Thrower {
        self.jump = true;
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

    /// When the grenade goes off, once its pin is out.
    pub fn goes_off(&self) -> Option<SimTime> {
        self.pin.map(|pin| SimTime(pin + f64::from(GRENADE_FUSE)))
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
            Phase::Jump { .. } => "jump",
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
                    self.pin = Some(pin);
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
                        jump: false,
                    });
                }
                Status::Running(Request {
                    weapon: Some(press(w, Attack::Primary, Trigger::Hold, 0.0)),
                    look: Some(LookIntent::Angles(angles)),
                    movement: (held >= cook - STOP_BEFORE).then(stop),
                    jump: false,
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
                    jump: false,
                })
            }
            Phase::Pressed { .. } | Phase::Jump { .. } => Status::Done,
        }
    }

    fn satchel(&mut self, h: &Hands<'_>) -> Status {
        let now = h.now;
        let w = WeaponId::Satchel;
        let game = h.predicted(w);
        let state = game.map_or(0, |p| p.charge_ready);
        let out = state == 1;
        let count = h.reserve(w);
        let button = self.button.unwrap_or(h.dll.satchel_throw());
        // Just after satchels went off the game takes no throw until it has seen both buttons up (its idle frame).
        let button_ready = state != 2 && takes(game, button);
        // From a jump the bot runs at the target first: the run carries the satchel on.
        let toward = (self.target - h.origin).truncate().normalize_or_zero();
        let run_up = self.jump.then_some(MoveIntent {
            dir: toward,
            speed: RUN_UP_SPEED,
        });
        let running = |weapon: WeaponIntent, angles: Vec3, jump: bool| {
            Status::Running(Request {
                weapon: Some(weapon),
                look: Some(LookIntent::Angles(angles)),
                movement: run_up,
                jump,
            })
        };
        let finished = |landings: &Vec<Vec3>, why: &'static str| {
            if landings.is_empty() {
                Status::Failed(why)
            } else {
                Status::Done
            }
        };
        if now.since(self.resolved_at) >= RESOLVE || matches!(self.phase, Phase::Jump { .. }) {
            self.resolved_at = now;
            self.throw = ballistics::satchel(&mut Unchecked, h.origin, h.velocity, self.target, h.gravity);
        }
        let angles = self.angles();
        match self.phase {
            Phase::Draw => {
                if now.since(self.started) > DRAW_TIMEOUT + 1.0 || count <= 0 {
                    return finished(&self.landings, "no satchel to throw");
                }
                if !(h.ready(w) && button_ready && settled(h.view, angles, 3.0)) {
                    return running(hold(w), angles, false);
                }
                if self.jump && h.on_ground {
                    let run = h.velocity.truncate().dot(toward);
                    if run < RUN_UP_MIN && now.since(self.started) < RUN_UP_FOR {
                        return running(hold(w), angles, false);
                    }
                    self.phase = Phase::Jump { at: now };
                    return running(hold(w), angles, true);
                }
                self.phase = Phase::Pressed {
                    at: now,
                    before: count,
                    button,
                };
                self.pressed_out = out;
                running(press(w, button, Trigger::Hold, 0.0), angles, false)
            }
            Phase::Jump { at } => {
                // Rising a moment after the feet left the ground: the lift goes into the throw.
                if (h.on_ground || now.since(at) < JUMP_THROW) && now.since(at) < JUMP_GIVE_UP {
                    return running(hold(w), angles, h.on_ground);
                }
                self.phase = Phase::Pressed {
                    at: now,
                    before: count,
                    button,
                };
                self.pressed_out = out;
                running(press(w, button, Trigger::Hold, 0.0), angles, false)
            }
            Phase::Pressed { at, before, button } => {
                if self.pressed_out && state == 2 {
                    // The press set the charges out off: that is the detonate button here.
                    self.learned = Some(button);
                    self.set_off = true;
                    return Status::Failed("the throw button set the satchels off");
                }
                if out && count < before {
                    // A throw with charges out tells the buttons apart; so does one by the other button after a
                    // press that did nothing.
                    if self.pressed_out || self.button.is_some() {
                        self.learned = Some(button.other());
                    }
                    self.button = Some(button);
                    self.landings.push(self.landing(h.gravity));
                    if self.pile > 1 && count > 0 {
                        self.pile -= 1;
                        self.phase = Phase::Draw;
                        self.started = now;
                        return running(hold(w), angles, false);
                    }
                    return Status::Done;
                }
                if self.button.is_none() && count == before && now.since(at) >= NO_EFFECT {
                    // Nothing thrown: the other button, once the game takes it.
                    self.button = Some(button.other());
                    self.phase = Phase::Draw;
                    self.started = now;
                    return running(hold(w), angles, false);
                }
                if now.since(at) >= SATCHEL_CONFIRM {
                    return finished(&self.landings, "the game did not throw the satchel");
                }
                running(press(w, button, Trigger::Hold, 0.0), angles, false)
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
                    jump: false,
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
                    jump: false,
                })
            }
            Phase::Cook { .. } | Phase::Released { .. } | Phase::Jump { .. } => Status::Done,
        }
    }
}

/// Emptying the snarks at an enemy close by. Held down, the game lets one go every 0.3 s while there is room in front
/// (not with the enemy right against the thrower); they swarm the enemy and bite it where it stands.
#[derive(Clone, Debug)]
pub struct Barrage {
    pub target: PlayerKey,
    started: SimTime,
    lost: Option<SimTime>,
    last_count: Option<i32>,
    /// Snarks let go.
    pub thrown: u32,
    /// Snarks to let go at most (a stream); all of them when `None`.
    pub limit: Option<u32>,
}

impl Barrage {
    pub fn new(target: PlayerKey, now: SimTime) -> Barrage {
        Barrage {
            target,
            started: now,
            lost: None,
            last_count: None,
            thrown: 0,
            limit: None,
        }
    }

    /// `n` snarks at most.
    pub fn stream(mut self, n: u32) -> Barrage {
        self.limit = Some(n.max(1));
        self
    }

    /// `at`: where the enemy is, while in sight.
    pub fn update(&mut self, h: &Hands<'_>, at: Option<Vec3>) -> Status {
        let now = h.now;
        let w = WeaponId::Snark;
        let count = h.reserve(w);
        if let Some(last) = self.last_count
            && count < last
        {
            self.thrown += (last - count) as u32;
        }
        self.last_count = Some(count);
        if count <= 0 || now.since(self.started) > BARRAGE_FOR || self.limit.is_some_and(|n| self.thrown >= n) {
            return Status::Done;
        }
        let Some(at) = at else {
            let lost = *self.lost.get_or_insert(now);
            if now.since(lost) > BARRAGE_LOST {
                return Status::Done;
            }
            return Status::Running(Request {
                weapon: Some(hold(w)),
                ..Request::default()
            });
        };
        self.lost = None;
        let weapon = if h.ready(w) {
            press(w, Attack::Primary, Trigger::Hold, 0.0)
        } else {
            hold(w)
        };
        Status::Running(Request {
            weapon: Some(weapon),
            look: Some(LookIntent::Angles(dir_to_view_angles(at - h.eye))),
            ..Request::default()
        })
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
    use lb_game::dll::{DllKind, DllProfile};
    use lb_game::self_state::{PredictedWeapon, Prediction};
    use lb_motor::Fire;

    struct Scene {
        view: Vec3,
        arsenal: Vec<Armed>,
        prediction: Prediction,
        dll: DllProfile,
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
                dll: self.dll,
                gravity: 800.0,
                deploying: false,
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
            dll: DllProfile::default(),
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
    fn a_pile_of_satchels_is_thrown_one_after_another() {
        let target = Vec3::new(250.0, 0.0, -36.0);
        let throw = ballistics::satchel(&mut Unchecked, Vec3::ZERO, Vec3::ZERO, target, 800.0);
        let mut th = Thrower::new(Kind::Satchel, target, throw, SimTime(0.0)).pile(3);
        let mut scene = Scene {
            view: Vec3::new(throw.pitch, throw.yaw, 0.0),
            arsenal: vec![Armed::new(WeaponId::Satchel, None, Some(4))],
            prediction: Prediction {
                current: Some(WeaponId::Satchel),
                primary_ammo: 4,
                ..Prediction::default()
            },
            dll: DllProfile::resolve("auto", true),
        };
        // BugfixedHL: a press of the primary attack throws one when its cycle allows, a second apart.
        let (mut carried, mut next, mut out) = (4, 0.0f64, 0);
        let mut presses = Vec::new();
        let mut t = 0.0;
        while t < 6.0 {
            scene.arsenal[0].reserve = Some(carried);
            scene.prediction.primary_ammo = carried;
            scene.prediction.weapons[WeaponId::Satchel as usize] = Some(PredictedWeapon {
                charge_ready: out,
                next_primary: (next - t) as f32,
                ..PredictedWeapon::default()
            });
            match th.update(&scene.hands(t, WeaponId::Satchel), &mut Unchecked) {
                Status::Running(r) => {
                    if r.weapon.is_some_and(|w| w.fire == Fire::Primary) && t >= next && carried > 0 {
                        carried -= 1;
                        out = 1;
                        next = t + 1.0;
                        presses.push(t);
                    }
                }
                Status::Done => break,
                Status::Failed(why) => panic!("{why}"),
            }
            t += 0.01;
        }
        assert_eq!(presses.len(), 3, "{presses:?}");
        assert_eq!(th.landings.len(), 3);
        assert!(
            presses.windows(2).all(|p| p[1] - p[0] < 1.2),
            "as soon as the game allows: {presses:?}"
        );
        assert_eq!(carried, 1, "the pile, not every satchel");
    }

    /// What a press of a satchel button does in a game DLL, charges out (`out`, the game's `m_chargeReady`) or not.
    #[derive(Debug, PartialEq)]
    enum Press {
        Throw,
        SetOff,
        Nothing,
    }

    fn satchel_press(kind: DllKind, fire: Fire, out: i32, carried: i32) -> Press {
        let throw = if carried > 0 { Press::Throw } else { Press::Nothing };
        match (kind, fire, out) {
            (_, _, 2) | (_, Fire::None, _) => Press::Nothing,
            (DllKind::Classic, Fire::Primary, 1) => Press::SetOff,
            (DllKind::Classic, _, _) => throw,
            (_, Fire::Secondary, 1) => Press::SetOff,
            (DllKind::Valve25, Fire::Secondary, _) => Press::Nothing,
            _ => throw,
        }
    }

    #[test]
    fn a_pile_shows_the_satchel_buttons_of_the_server() {
        for kind in DllKind::ALL {
            let target = Vec3::new(250.0, 0.0, -36.0);
            let throw = ballistics::satchel(&mut Unchecked, Vec3::ZERO, Vec3::ZERO, target, 800.0);
            let mut th = Thrower::new(Kind::Satchel, target, throw, SimTime(0.0)).pile(3);
            let mut scene = Scene {
                view: Vec3::new(throw.pitch, throw.yaw, 0.0),
                arsenal: vec![Armed::new(WeaponId::Satchel, None, Some(4))],
                prediction: Prediction {
                    current: Some(WeaponId::Satchel),
                    ..Prediction::default()
                },
                dll: DllProfile::default(),
            };
            // The game's clocks for the two buttons: a throw holds the primary back a second, the secondary half.
            let (mut carried, mut out, mut next) = (4, 0, [0.0f64; 2]);
            let (mut learned, mut end, mut t) = (None, None, 0.0);
            while t < 8.0 && end.is_none() {
                scene.arsenal[0].reserve = Some(carried);
                scene.prediction.primary_ammo = carried;
                scene.prediction.weapons[WeaponId::Satchel as usize] = Some(PredictedWeapon {
                    charge_ready: out,
                    next_primary: (next[0] - t) as f32,
                    next_secondary: (next[1] - t) as f32,
                    ..PredictedWeapon::default()
                });
                let status = th.update(&scene.hands(t, WeaponId::Satchel), &mut Unchecked);
                // What a press showed holds for the next ones, as it does for every bot on the server.
                if let Some(f) = th.learned.take() {
                    learned = Some(f);
                    scene.dll.set_satchel_detonate(f);
                }
                match status {
                    Status::Running(r) => {
                        let fire = r.weapon.map_or(Fire::None, |w| w.fire);
                        let ready = match fire {
                            Fire::Primary => t >= next[0],
                            Fire::Secondary => t >= next[1],
                            _ => false,
                        };
                        match satchel_press(kind, fire, out, carried) {
                            Press::Throw if ready => {
                                carried -= 1;
                                out = 1;
                                next = [t + 1.0, t + 0.5];
                            }
                            Press::SetOff if ready => out = 2,
                            _ => {}
                        }
                    }
                    other => end = Some(other),
                }
                t += 0.01;
            }
            let thrown = 4 - carried;
            match kind {
                DllKind::Classic => {
                    assert_eq!((end, thrown), (Some(Status::Done), 3), "{kind:?}");
                    assert_eq!(
                        learned,
                        Some(Attack::Primary),
                        "the second throw of the pile shows the buttons"
                    );
                }
                DllKind::Valve25 => {
                    assert_eq!((end, thrown), (Some(Status::Done), 3), "{kind:?}");
                    assert_eq!(
                        learned,
                        Some(Attack::Secondary),
                        "the secondary did nothing, the primary threw"
                    );
                }
                DllKind::Bugfixed => {
                    // Not told by its cvars: the second press sets the first satchel off, and shows the buttons.
                    assert_eq!(
                        (end, thrown),
                        (Some(Status::Failed("the throw button set the satchels off")), 1)
                    );
                    assert!(th.set_off);
                    assert_eq!(learned, Some(Attack::Secondary));
                }
            }
        }
    }

    #[test]
    fn a_satchel_from_a_jump_leaves_as_the_bot_rises() {
        let target = Vec3::new(450.0, 0.0, -36.0);
        let throw = ballistics::satchel(&mut Unchecked, Vec3::ZERO, Vec3::ZERO, target, 800.0);
        let mut th = Thrower::new(Kind::Satchel, target, throw, SimTime(0.0)).from_jump();
        let mut scene = Scene {
            view: Vec3::new(throw.pitch, throw.yaw, 0.0),
            arsenal: vec![Armed::new(WeaponId::Satchel, None, Some(1))],
            prediction: Prediction {
                current: Some(WeaponId::Satchel),
                primary_ammo: 1,
                ..Prediction::default()
            },
            dll: DllProfile::resolve("auto", true),
        };
        scene.prediction.weapons[WeaponId::Satchel as usize] = Some(PredictedWeapon::default());
        let Status::Running(r) = th.update(&scene.hands(0.0, WeaponId::Satchel), &mut Unchecked) else {
            panic!()
        };
        let run = r.movement.expect("a run-up");
        assert!(!r.jump && run.dir.x > 0.99, "runs at the target first: {r:?}");
        // Running: the throw is solved again for the run, and the view is on it.
        let run = Vec3::new(250.0, 0.0, 0.0);
        let solved = ballistics::satchel(&mut Unchecked, Vec3::ZERO, run, target, 800.0);
        scene.view = Vec3::new(solved.pitch, solved.yaw, 0.0);
        let mut running = scene.hands(0.3, WeaponId::Satchel);
        running.velocity = run;
        let Status::Running(r) = th.update(&running, &mut Unchecked) else {
            panic!()
        };
        assert!(r.jump && r.weapon.unwrap().fire == Fire::None, "then jumps");
        // Airborne from 0.32 s: the throw waits for the lift.
        let mut airborne = scene.hands(0.35, WeaponId::Satchel);
        airborne.on_ground = false;
        airborne.velocity = Vec3::new(250.0, 0.0, 250.0);
        let Status::Running(r) = th.update(&airborne, &mut Unchecked) else {
            panic!()
        };
        assert_eq!(r.weapon.unwrap().fire, Fire::None);
        airborne.now = SimTime(0.5);
        airborne.velocity = Vec3::new(250.0, 0.0, 150.0);
        let Status::Running(r) = th.update(&airborne, &mut Unchecked) else {
            panic!()
        };
        assert_eq!(r.weapon.unwrap().fire, Fire::Primary, "thrown while rising");
        assert!(th.throw.velocity.z > 0.0 && th.throw.start == Vec3::ZERO);
        assert!(
            th.throw.velocity.x > 450.0,
            "the run carries it on: {:?}",
            th.throw.velocity
        );
    }

    #[test]
    fn a_barrage_empties_the_snarks_at_an_enemy_close_by() {
        let mut b = Barrage::new(PlayerKey { slot: 3, userid: 3 }, SimTime(0.0));
        let mut scene = Scene {
            view: Vec3::ZERO,
            arsenal: vec![Armed::new(WeaponId::Snark, None, Some(3))],
            prediction: Prediction {
                current: Some(WeaponId::Snark),
                primary_ammo: 3,
                ..Prediction::default()
            },
            dll: DllProfile::default(),
        };
        let enemy = Some(Vec3::new(150.0, 0.0, 0.0));
        let Status::Running(r) = b.update(&scene.hands(0.0, WeaponId::Snark), enemy) else {
            panic!()
        };
        assert_eq!(r.weapon.unwrap().fire, Fire::Primary);
        assert!(matches!(r.look, Some(LookIntent::Angles(_))));
        for (t, left) in [(0.3, 2), (0.6, 1)] {
            scene.arsenal[0].reserve = Some(left);
            scene.prediction.primary_ammo = left;
            assert!(matches!(
                b.update(&scene.hands(t, WeaponId::Snark), enemy),
                Status::Running(_)
            ));
        }
        assert_eq!(b.thrown, 2);
        // Out of sight a moment: held; longer: over.
        assert!(matches!(
            b.update(&scene.hands(0.7, WeaponId::Snark), None),
            Status::Running(_)
        ));
        assert_eq!(b.update(&scene.hands(1.2, WeaponId::Snark), None), Status::Done);
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
            dll: DllProfile::resolve("auto", true),
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
            th.update(&scene.hands(0.1, WeaponId::Satchel), &mut Unchecked),
            Status::Running(_)
        ));
        scene.arsenal[0].reserve = Some(1);
        scene.prediction.primary_ammo = 1;
        scene.prediction.weapons[WeaponId::Satchel as usize] = Some(PredictedWeapon {
            charge_ready: 1,
            ..PredictedWeapon::default()
        });
        assert_eq!(
            th.update(&scene.hands(0.12, WeaponId::Satchel), &mut Unchecked),
            Status::Done
        );
    }
}
