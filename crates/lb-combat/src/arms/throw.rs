//! Throwing hand grenades, satchels and snarks.
//!
//! - **Grenade:** press the primary attack; the pin is out when the game reports the throw started (its own clock,
//!   so the fuse is known to the frame). Hold it to cook the grenade so it goes off soon after landing (as long as
//!   the plan's delivery lets it: at once on GunGame's grenade level), turn to the throw solved for the target, stop
//!   for the last moment, and let go: the game throws on its next idle frame. Once the pin is out the grenade is
//!   always thrown, at the latest a little after its moment, well before the fuse runs out. Thrown on the run (as
//!   players throw), the bot does not stop: it backs off with the pin out, for the last moment of the cooking runs at
//!   the target, and lets go running at it, the run in the throw (some 850 units a second, nearly flat). From a jump
//!   it jumps at the target and lets go near the top of the jump; from a long jump the leap carries the grenade
//!   further still.
//! - **Satchel:** draw it (a second), turn to the throw, press the DLL's throw button once the game takes it;
//!   confirmed when the game reports a charge out and one satchel fewer. A pile is thrown one after another as the
//!   game allows (a second apart), each at the same spot. Thrown on the run (as players throw at an enemy), the bot
//!   runs at the target once the satchel is in hand and throws when it moves at it: the run goes into the throw, and
//!   the satchel flies twice as fast and far. From a jump, it then jumps and presses the button a moment after its
//!   feet leave the ground: the jump's lift goes in as well. A press that did nothing is followed by the other
//!   button; what the presses showed of the server's satchel buttons (a throw with charges out, a button that did
//!   nothing, or one that set the charges off) is reported.
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
use crate::grenade::{self, Aim, Delivery, Plan};

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
/// A grenade on the run: the run at the target starts this long before the cooking is done; the grenade leaves
/// running at it this fast, or once the run has gone on this long. With the target this close the bot runs no closer
/// (players dash at an enemy closer still, and back off at once).
const GRENADE_RUN_UP: f32 = 0.35;
const GRENADE_RUN_FOR: f64 = 0.6;
const GRENADE_RUN_CLOSE: f32 = 150.0;
/// A grenade from a jump leaves this long after the feet leave the ground, near the top of the jump (players: 0.2–0.4
/// s); the jump goes this long before the cooking is done.
const GRENADE_JUMP_THROW: f64 = 0.28;
/// A planned grenade is planned again this often (in the air, on the run-up), and over all pitches this often.
const PLAN_AIR: f64 = 0.03;
/// A planned grenade leaves only on a plan this fresh, made with the velocity the throw will carry; the plans come
/// this often in the last moments before it.
const PLAN_FRESH: f64 = 0.05;
const PLAN_LAST: f32 = 0.15;
/// This long before the fuse forces it out, with no plan to be had for `DUMP_STALE`, the grenade goes where its blast
/// is farthest off: time to turn to that throw, while the fuse still carries the grenade far.
const DUMP_BEFORE: f32 = 0.5;
const DUMP_STALE: f64 = 0.15;
/// A grenade to go at once is let go this long after its moment at the latest, whatever the view: held into the last
/// half second of its fuse a grenade bursts by the thrower (every suicide by grenade on the GunGame server, 2026-10-02).
const RUN_OUT: f32 = 0.9;
/// A planned grenade that missed its moment goes as soon as it can: planned again to leave within this.
const REPLAN_SLACK: f32 = 0.2;
/// A quick grenade leaves with the view this close to the throw (degrees): many grenades, not exact ones.
const QUICK_SETTLED: f32 = 3.0;
/// Cooking a planned grenade before the run-up the bot keeps moving, as players do: back from the target this close
/// (along a slant, out of a straight line of fire), across it further off.
const COOK_BACK: f32 = 650.0;
const COOK_SLANT: f32 = 0.5;
const PLAN_RUN: f64 = 0.05;
const PLAN_FULL: f64 = 0.3;
/// The bot runs at this speed at the throw, as it is planned before the run-up.
const RUN_EXPECTED: f32 = 270.0;
/// A planned grenade leaves within this of the pin time asked: later, the blast would come a tick late.
const RELEASE_WINDOW: f32 = 0.05;
/// Thrown on the run, the satchel leaves once the bot runs at the target this fast and the throw lands within this
/// of it; not so by then, or with the target this close (unless told otherwise), it stays in hand (it would go off by
/// the thrower).
const RUN_UP_SPEED: f32 = 320.0;
const RUN_UP_MIN: f32 = 200.0;
const REACHES: f32 = 32.0;
const RUN_UP_FOR: f64 = 1.2;
const RUN_CLOSE: f32 = 250.0;
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

/// A grenade planned at `aim` as `plan` (made at `made`), thrown as `delivery`; planned again last at `at`, over all
/// pitches at `full`. `fallback`: no throw at the enemy could be had as the fuse ran low; it goes where the enemy most
/// likely is round a corner (`"behind cover"`), else where its blast is farthest off (`"dumped"`).
#[derive(Clone, Debug)]
struct Planned {
    aim: Aim,
    plan: Plan,
    delivery: Delivery,
    made: SimTime,
    at: SimTime,
    full: SimTime,
    fallback: Option<&'static str>,
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
    /// Satchels and grenades are thrown on the run at the target; since when it runs (the satchel in hand, the
    /// grenade's run-up); a satchel on the run is given up at a target closer than `run_close`.
    run: bool,
    run_from: Option<SimTime>,
    run_close: f32,
    /// Satchels and grenades are thrown from a jump; a long jump (its keys are the caller's to press): the leap
    /// carries them far.
    jump: bool,
    leap: bool,
    /// When a grenade's jump was asked for.
    jumped: Option<SimTime>,
    /// A planned grenade; where and when it should burst once thrown.
    planned: Option<Box<Planned>>,
    burst: Option<(Vec3, SimTime)>,
    /// Let go only because the fuse was running out.
    forced: bool,
    /// The view pitch and the bot's velocity as a grenade was let go.
    launch: Option<(f32, Vec3)>,
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
            run: false,
            run_from: None,
            run_close: RUN_CLOSE,
            jump: false,
            leap: false,
            jumped: None,
            planned: None,
            burst: None,
            forced: false,
            launch: None,
            landings: Vec::new(),
            button: None,
            pressed_out: false,
            learned: None,
            set_off: false,
            pin: None,
        }
    }

    /// A grenade planned at `aim` (see [`grenade::plan`]), first as `plan`, thrown as `delivery`: cooked to burst where
    /// the enemy will be, as long as the delivery lets it.
    pub fn planned(mut self, aim: Aim, plan: Plan, delivery: Delivery) -> Thrower {
        self.planned = Some(Box::new(Planned {
            aim,
            plan,
            delivery,
            made: self.started,
            at: self.started,
            full: self.started,
            fallback: None,
        }));
        self.throw = plan.throw;
        self.target = plan.target;
        self
    }

    /// The enemy a planned grenade is for, seen again: the plan follows it.
    pub fn retarget(&mut self, aim: Aim) {
        if let Some(p) = &mut self.planned {
            p.aim = aim;
        }
    }

    /// The grenade's plan as it stands.
    pub fn plan(&self) -> Option<&Plan> {
        self.planned.as_ref().map(|p| &p.plan)
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

    /// Satchels or a grenade thrown on the run at the target.
    pub fn on_the_run(mut self) -> Thrower {
        self.run = true;
        self
    }

    /// A satchel on the run thrown at a target as close as `d`.
    pub fn closer(mut self, d: f32) -> Thrower {
        self.run_close = d;
        self
    }

    /// Satchels or a grenade thrown on the run, from a jump.
    pub fn from_jump(mut self) -> Thrower {
        self.run = true;
        self.jump = true;
        self
    }

    /// Satchels or a grenade thrown on the run, from a long jump.
    pub fn from_leap(mut self) -> Thrower {
        self.leap = true;
        self.from_jump()
    }

    /// The jump asked for is a long jump.
    pub fn leaps(&self) -> bool {
        self.leap
    }

    /// Runs at the target for the throw (a satchel in hand, a grenade's run-up).
    pub fn runs_up(&self) -> bool {
        self.run_from.is_some()
    }

    /// Let go only because the fuse was running out (`Some("forced by the fuse")`), or on a fallback with no throw
    /// at the enemy to be had (`Some("behind cover")`, `Some("dumped")`).
    pub fn forced(&self) -> Option<&'static str> {
        self.planned
            .as_ref()
            .and_then(|p| p.fallback)
            .or(self.forced.then_some("forced by the fuse"))
    }

    /// While a planned grenade cooks before its run-up: back from the target along a slant when it is close, across
    /// it when it is far, the side picked by when the throw began; none toward a drop (the fight moves the bot then).
    fn cook_move(&self, h: &Hands<'_>, tracer: &mut dyn Tracer, toward: lb_core::Vec2) -> Option<MoveIntent> {
        if toward == lb_core::Vec2::ZERO || !h.on_ground {
            return None;
        }
        let side = if (self.started.secs() * 10.0) as i64 % 2 == 0 {
            1.0
        } else {
            -1.0
        };
        let across = lb_core::Vec2::new(-toward.y, toward.x) * side;
        let far = (self.target - h.origin).truncate().length() >= COOK_BACK;
        let dir = if far {
            across
        } else {
            (-toward + across * COOK_SLANT).normalize()
        };
        [dir, if far { -across } else { -toward }]
            .into_iter()
            .find(|d| !crate::fight::drops(tracer, h.origin, *d * RUN_UP_SPEED))
            .map(|dir| MoveIntent {
                dir,
                speed: RUN_UP_SPEED,
            })
    }

    /// The view pitch and the bot's velocity as the grenade was let go: the game throws it with them.
    pub fn launch(&self) -> Option<(f32, Vec3)> {
        self.launch
    }

    /// How it is thrown: from a stand, on the run, from a jump or a long jump.
    pub fn way(&self) -> &'static str {
        match (self.run, self.jumped.is_some(), self.leap) {
            (false, _, _) => "standing",
            (true, false, _) => "on the run",
            (true, true, false) => "from a jump",
            (true, true, true) => "from a long jump",
        }
    }

    /// A grenade to back off from at once, as players do: one at the enemy, not from a long jump (the leap carries the
    /// bot on) nor one dumped (its blast is anywhere but by the enemy).
    pub fn backs_off(&self) -> bool {
        !self.leap
            && match &self.planned {
                Some(p) => p.fallback != Some("dumped"),
                None => self.run,
            }
    }

    /// Where the thrown grenade or satchel should come down (a planned grenade: where it bursts).
    pub fn landing(&self, sv_gravity: f32) -> Vec3 {
        if let Some(p) = &self.planned {
            return p.plan.burst;
        }
        let t = &self.throw;
        ballistics::position_at(
            t.start,
            t.velocity,
            sv_gravity * lb_game::mechanics::PROJECTILE_GRAVITY,
            t.flight,
        )
    }

    /// When the grenade goes off, once its pin is out (a planned one: on its tick after the throw).
    pub fn goes_off(&self) -> Option<SimTime> {
        self.burst
            .map(|b| b.1)
            .or_else(|| self.pin.map(|pin| SimTime(pin + f64::from(GRENADE_FUSE))))
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
            Kind::Grenade => self.grenade(h, tracer),
            Kind::Satchel => self.satchel(h),
            Kind::Snark => self.snark(h, tracer),
        }
    }

    fn grenade(&mut self, h: &Hands<'_>, tracer: &mut dyn Tracer) -> Status {
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
                let deadline = self
                    .planned
                    .as_ref()
                    .map_or(grenade::LATEST, |p| p.delivery.latest + RUN_OUT)
                    .min(grenade::LATEST)
                    .min(GRENADE_FUSE - FUSE_MARGIN);
                let airborne = self.jumped.is_some() && !h.on_ground;
                let toward = (self.target - h.origin).truncate().normalize_or_zero();
                // Close to the target the bot runs no closer: the grenade goes as from a stand.
                let run = self.run && (self.target - h.origin).truncate().length() >= GRENADE_RUN_CLOSE;
                if let Some(pl) = &mut self.planned {
                    // Planned again as the bot and the enemy move: in the air and on the run-up often (the velocity
                    // the throw carries changes fast), and at once when the moment to let go has passed.
                    let every = if airborne || held >= pl.plan.release_held - PLAN_LAST {
                        PLAN_AIR
                    } else if self.run_from.is_some() {
                        PLAN_RUN
                    } else {
                        RESOLVE
                    };
                    let missed = held > pl.plan.release_held + RELEASE_WINDOW;
                    if missed || now.since(pl.at) >= every {
                        pl.at = now;
                        // Before the run-up the throw is planned with the run it will carry.
                        let velocity = if run && self.run_from.is_none() && self.jumped.is_none() {
                            toward.extend(0.0) * RUN_EXPECTED
                        } else {
                            h.velocity
                        };
                        // A quick grenade keeps the pitch it was planned with: the view stays on the throw.
                        let full = !pl.delivery.quick && now.since(pl.full) >= PLAN_FULL;
                        if full {
                            pl.full = now;
                        }
                        let near = (!full).then_some(pl.plan.throw.pitch);
                        let aim = pl.aim;
                        // Past its moment the grenade goes as soon as it can.
                        let delivery = Delivery {
                            latest: pl.delivery.latest.max(held + REPLAN_SLACK),
                            ..pl.delivery
                        };
                        let made = grenade::plan(
                            tracer, h.eye, h.origin, velocity, &aim, h.gravity, h.dll, held, held, near, delivery,
                        );
                        // Nothing over the few pitches near the last: over all of them.
                        let made = match made {
                            None if near.is_some() => {
                                pl.full = now;
                                grenade::plan(
                                    tracer, h.eye, h.origin, velocity, &aim, h.gravity, h.dll, held, held, None,
                                    delivery,
                                )
                            }
                            made => made,
                        };
                        if let Some(p) = made
                            && pl.fallback.is_none()
                        {
                            pl.plan = p;
                            pl.made = now;
                            self.throw = p.throw;
                            self.target = p.target;
                        }
                    }
                    // Nothing to throw at to be had while the fuse runs low (the bot boxed in, the enemy come
                    // close): away from the bot, as far as it goes.
                    if pl.fallback.is_none() && held >= deadline - DUMP_BEFORE && now.since(pl.made) > DUMP_STALE {
                        // Where the enemy most likely is round the corner, else away from the bot.
                        let aim = pl.aim;
                        let to = grenade::plan_behind_cover(
                            tracer, h.eye, h.origin, h.velocity, &aim, h.gravity, h.dll, held, deadline,
                        )
                        .map(|p| (p, "behind cover"))
                        .or_else(|| {
                            grenade::dump(tracer, h.eye, h.origin, h.velocity, h.gravity, h.dll, held)
                                .map(|p| (p, "dumped"))
                        });
                        pl.fallback = Some(to.map_or("dumped", |t| t.1));
                        if let Some((p, _)) = to {
                            pl.plan = p;
                            pl.made = now;
                            self.throw = p.throw;
                        }
                    }
                } else if now.since(self.resolved_at) >= RESOLVE || airborne {
                    // In the air the velocity the throw carries changes fast: solved every frame.
                    self.resolved_at = now;
                    if let Some(t) =
                        ballistics::grenade(&mut Unchecked, h.eye, h.velocity, self.target, h.gravity, h.dll, 3.0)
                    {
                        self.throw = t;
                    }
                }
                // A quick grenade goes as soon as the bot is ready, on whatever tick that gives it.
                let quick = self.planned.as_ref().is_some_and(|p| p.delivery.quick);
                let (cook, late) = match &self.planned {
                    Some(p) => (
                        p.plan.release_held,
                        !quick && p.fallback.is_none() && held > p.plan.release_held + RELEASE_WINDOW,
                    ),
                    _ if self.quick => (GRENADE_MIN_COOK, false),
                    _ => (
                        (GRENADE_FUSE - self.throw.flight - LAND_BEFORE_FUSE).clamp(GRENADE_MIN_COOK, MAX_COOK),
                        false,
                    ),
                };
                if !run || self.planned.as_ref().is_some_and(|p| p.fallback.is_some()) {
                    self.run_from = None;
                } else if held >= cook - GRENADE_RUN_UP || self.jumped.is_some() {
                    self.run_from.get_or_insert(now);
                }
                // Before a long jump the view is on the target: the leap goes along it.
                let angles = if self.leap && self.jumped.is_none() && self.run_from.is_some() {
                    dir_to_view_angles(self.target - h.eye)
                } else {
                    self.angles()
                };
                if settled(h.view, angles, if quick { QUICK_SETTLED } else { 1.5 }) {
                    self.steady_since.get_or_insert(now);
                } else {
                    self.steady_since = None;
                }
                let steady = self.steady_since.is_some_and(|t| now.since(t) >= STEADY);
                let running = h.on_ground && h.velocity.truncate().dot(toward) >= RUN_UP_MIN;
                let mut jump = false;
                if let Some(at) = self.jumped
                    && h.on_ground
                    && now.since(at) >= JUMP_GIVE_UP
                {
                    // The jump never came (no room, or no safe landing for a long jump): from the ground.
                    self.jump = false;
                    self.leap = false;
                    self.jumped = None;
                }
                let dumped = self.planned.as_ref().is_some_and(|p| p.fallback.is_some());
                let go = if held >= deadline {
                    true
                } else if dumped {
                    // Once the view is on it, whatever the run.
                    steady && held >= cook
                } else if late {
                    // Its tick is gone (the view was not on the throw in time): the plan takes the next one.
                    false
                } else if let Some(at) = self.jumped {
                    jump = h.on_ground;
                    !h.on_ground && now.since(at) >= GRENADE_JUMP_THROW && steady && held >= cook.min(deadline)
                } else if self.run_from.is_some() {
                    let ran_for = self.run_from.is_some_and(|t| now.since(t) >= GRENADE_RUN_FOR);
                    if self.jump && running && held >= cook - GRENADE_JUMP_THROW as f32 && settled(h.view, angles, 5.0)
                    {
                        self.jumped = Some(now);
                        jump = true;
                        false
                    } else if self.jump && ran_for {
                        // Never got to run at the target for the jump: from the ground, the view onto the throw.
                        self.jump = false;
                        self.leap = false;
                        false
                    } else {
                        held >= cook && steady && (running || ran_for)
                    }
                } else {
                    held >= cook && steady
                };
                let run_up = MoveIntent {
                    dir: toward,
                    speed: RUN_UP_SPEED,
                };
                // Cooking before the run-up the bot moves as the fight has it; a planned grenade is not stopped for
                // either: the plan carries whatever velocity the bot has.
                let planned = self.planned.is_some();
                let movement = if dumped {
                    None
                } else if self.run_from.is_some() {
                    Some(run_up)
                } else if planned {
                    self.cook_move(h, tracer, toward)
                } else if run {
                    None
                } else {
                    (held >= cook - STOP_BEFORE).then(stop)
                };
                let fresh = self
                    .planned
                    .as_ref()
                    .is_none_or(|p| p.fallback.is_some() || now.since(p.made) <= PLAN_FRESH);
                self.forced = held >= deadline && !(go && fresh);
                let go = held >= deadline || (go && fresh);
                if go {
                    self.phase = Phase::Released { at: now };
                    self.launch = Some((h.view.x, h.velocity));
                    if let Some(p) = &self.planned {
                        // Let go by the fuse on a plan gone stale, or quick off its tick: it bursts on the tick the
                        // fuse left gives it.
                        let flight = if (self.forced || quick) && p.fallback.is_none() {
                            let fuse = (GRENADE_FUSE - held).max(0.0);
                            ((fuse / grenade::THINK).ceil() + 1.0) * grenade::THINK
                        } else {
                            p.plan.throw.flight
                        };
                        self.burst = Some((p.plan.burst, now + f64::from(flight)));
                    }
                    return Status::Running(Request {
                        weapon: Some(hold(w)),
                        look: Some(LookIntent::Angles(self.angles())),
                        movement: if self.run_from.is_some() {
                            Some(run_up)
                        } else {
                            (!planned).then(stop)
                        },
                        jump: false,
                    });
                }
                Status::Running(Request {
                    weapon: Some(press(w, Attack::Primary, Trigger::Hold, 0.0)),
                    look: Some(LookIntent::Angles(angles)),
                    movement,
                    jump,
                })
            }
            Phase::Released { at } => {
                if pin.is_none() || now.since(at) >= CONFIRM {
                    return Status::Done;
                }
                // The game throws on its next idle frame, with the velocity the bot has then: the run goes on.
                let toward = (self.target - h.origin).truncate().normalize_or_zero();
                Status::Running(Request {
                    weapon: Some(hold(w)),
                    look: Some(LookIntent::Angles(self.angles())),
                    movement: if self.run_from.is_some() {
                        Some(MoveIntent {
                            dir: toward,
                            speed: RUN_UP_SPEED,
                        })
                    } else {
                        self.planned.is_none().then(stop)
                    },
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
        // On the run the bot runs at the target once the satchel is in hand: the run carries the satchel on.
        let toward = (self.target - h.origin).truncate().normalize_or_zero();
        if self.run && h.ready(w) {
            self.run_from.get_or_insert(now);
        }
        let run_up = self.run_from.is_some().then_some(MoveIntent {
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
        let reaches = self.landing(h.gravity).truncate().distance(self.target.truncate()) <= REACHES;
        // Before a long jump the view is on the target: the leap goes along it, and makes another throw.
        let angles = if self.leap && self.phase == Phase::Draw {
            dir_to_view_angles(self.target - h.eye)
        } else {
            self.angles()
        };
        match self.phase {
            Phase::Draw => {
                if now.since(self.started) > DRAW_TIMEOUT + 1.0 || count <= 0 {
                    return finished(&self.landings, "no satchel to throw");
                }
                if let Some(from) = self.run_from {
                    // Running at the target fast enough for the satchel to get there (from a long jump it will),
                    // from the ground for a jump, from a hop as well; never one that would come down short, by the bot.
                    if (self.target - h.origin).truncate().length() < self.run_close {
                        return finished(&self.landings, "the target came close");
                    }
                    let run = h.velocity.truncate().dot(toward);
                    if !((h.on_ground || !self.jump) && run >= RUN_UP_MIN && (reaches || self.leap)) {
                        if now.since(from) > RUN_UP_FOR {
                            return finished(&self.landings, "the throw would not reach on the run");
                        }
                        return running(hold(w), angles, false);
                    }
                }
                if !(h.ready(w) && button_ready && settled(h.view, angles, 3.0)) {
                    return running(hold(w), angles, false);
                }
                if self.jump && h.on_ground {
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
                // Rising a moment after the feet left the ground, the view on the throw the lift makes: the lift goes
                // into the throw. One that would not get there (a long jump not taken) stays in hand.
                if (h.on_ground || now.since(at) < JUMP_THROW || !settled(h.view, angles, 3.0) || !reaches)
                    && now.since(at) < JUMP_GIVE_UP
                {
                    return running(hold(w), angles, h.on_ground);
                }
                if !reaches {
                    return finished(&self.landings, "the throw would not reach from the jump");
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
    use lb_worldq::TraceQuery;

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

    /// A flat floor at z = -36.
    struct Floor;

    impl Tracer for Floor {
        fn trace(&mut self, q: &TraceQuery) -> lb_worldq::Trace {
            let mut t = lb_worldq::Trace::clear(q.end);
            if q.end.z < -36.0 && q.start.z >= -36.0 {
                t.fraction = (q.start.z + 36.0) / (q.start.z - q.end.z);
                t.end = q.start + (q.end - q.start) * t.fraction;
                t.normal = Vec3::Z;
            }
            t
        }

        fn point_contents(&mut self, _p: Vec3) -> i32 {
            lb_worldq::contents::EMPTY
        }
    }

    /// What a planned grenade did, thrown by a bot that moves as asked (full speed at once on the ground).
    struct Thrown {
        pin: f64,
        released: f64,
        velocity: Vec3,
        origin: Vec3,
        on_ground: bool,
        backed_off: bool,
        jumped: bool,
        after: Option<MoveIntent>,
    }

    fn throw_planned(th: &mut Thrower, aim: &Aim) -> Thrown {
        let mut scene = Scene {
            view: Vec3::ZERO,
            arsenal: vec![Armed::new(WeaponId::HandGrenade, None, Some(3))],
            prediction: Prediction {
                current: Some(WeaponId::HandGrenade),
                primary_ammo: 3,
                ..Prediction::default()
            },
            dll: DllProfile::resolve("auto", true),
        };
        let (mut origin, mut velocity) = (Vec3::ZERO, Vec3::ZERO);
        let mut air: Option<f64> = None;
        let mut out = Thrown {
            pin: -1.0,
            released: -1.0,
            velocity: Vec3::ZERO,
            origin: Vec3::ZERO,
            on_ground: true,
            backed_off: false,
            jumped: false,
            after: None,
        };
        // The game's clock: a pin pulled at 0 would read as none.
        let mut t = 1.0;
        while t < 5.0 {
            let mut h = scene.hands(t, WeaponId::HandGrenade);
            h.origin = origin;
            h.eye = origin + Vec3::Z * 28.0;
            h.velocity = velocity;
            h.on_ground = air.is_none();
            let Status::Running(r) = th.update(&h, &mut Floor) else {
                break;
            };
            let fire = r.weapon.map_or(Fire::None, |w| w.fire);
            if fire == Fire::Primary && out.pin < 0.0 {
                out.pin = t;
            }
            if out.pin >= 0.0 && out.released < 0.0 {
                if fire == Fire::None {
                    out.released = t;
                    out.velocity = velocity;
                    out.origin = origin;
                    out.on_ground = air.is_none();
                } else if r.movement.is_some_and(|m| m.dir.x < 0.5) {
                    out.backed_off = true;
                }
            } else if out.released >= 0.0 && out.after.is_none() {
                out.after = r.movement;
            }
            if let Some(LookIntent::Angles(a)) = r.look {
                let d = a - scene.view;
                scene.view += d.clamp_length_max(5.0);
            }
            if r.jump && air.is_none() {
                out.jumped = true;
                air = Some(t);
                velocity.z = 268.0;
            }
            if let Some(m) = r.movement
                && air.is_none()
            {
                velocity = (m.dir * m.speed.min(300.0)).extend(0.0);
            }
            if let Some(at) = air {
                velocity.z -= 800.0 * 0.01;
                if t - at > 0.6 {
                    air = None;
                    velocity.z = 0.0;
                }
            }
            origin += velocity * 0.01;
            // The game throws on its idle frame a moment after the button is let go.
            let thrown = out.released >= 0.0 && t > out.released + 0.015;
            let pin = if thrown { 0.0 } else { out.pin.max(0.0) as f32 };
            scene.prediction.weapons[WeaponId::HandGrenade as usize] = Some(PredictedWeapon {
                start_throw: pin,
                ..PredictedWeapon::default()
            });
            th.retarget(*aim);
            t += 0.01;
        }
        out
    }

    #[test]
    fn a_planned_grenade_goes_on_the_run_cooked_to_its_tick() {
        let dll = DllProfile::resolve("auto", true);
        let aim = Aim {
            pos: Vec3::new(700.0, 0.0, 0.0),
            vel: Vec3::ZERO,
            sigma: 20.0,
            spread: 110.0,
        };
        let eye = Vec3::new(0.0, 0.0, 28.0);
        let run = Vec3::new(270.0, 0.0, 0.0);
        let plan = grenade::plan(
            &mut Floor,
            eye,
            Vec3::ZERO,
            run,
            &aim,
            800.0,
            dll,
            0.0,
            0.0,
            None,
            Delivery::COOKED,
        )
        .unwrap();
        let mut th = Thrower::new(Kind::Grenade, plan.target, plan.throw, SimTime(1.0))
            .planned(aim, plan, Delivery::COOKED)
            .on_the_run();
        let out = throw_planned(&mut th, &aim);
        assert!(out.released > 0.0, "thrown");
        let held = (out.released - out.pin) as f32;
        let p = th.plan().unwrap();
        assert!(
            held >= p.release_held - 0.011 && held <= p.release_held + RELEASE_WINDOW,
            "let go with the pin out {held} s, planned {}",
            p.release_held
        );
        assert!(held <= grenade::LATEST);
        assert!(
            out.backed_off,
            "it keeps moving off the line to the target while it cooks"
        );
        let toward = (aim.pos - out.origin).truncate().normalize();
        assert!(
            out.velocity.truncate().dot(toward) >= RUN_UP_MIN && out.on_ground,
            "running at it: {:?}",
            out.velocity
        );
        assert!(grenade::blast_damage(p.burst, aim.pos) > 60.0, "{p:?}");
        assert!(
            out.after.is_some_and(|m| m.dir.dot(toward) > 0.9),
            "the run goes on until the game throws"
        );
        assert!(th.goes_off().is_some());
    }

    #[test]
    fn a_planned_grenade_from_a_jump_leaves_in_the_air() {
        let dll = DllProfile::resolve("auto", true);
        let aim = Aim {
            pos: Vec3::new(800.0, 0.0, 0.0),
            vel: Vec3::ZERO,
            sigma: 20.0,
            spread: 110.0,
        };
        let eye = Vec3::new(0.0, 0.0, 28.0);
        let run = Vec3::new(270.0, 0.0, 0.0);
        let plan = grenade::plan(
            &mut Floor,
            eye,
            Vec3::ZERO,
            run,
            &aim,
            800.0,
            dll,
            0.0,
            0.0,
            None,
            Delivery::COOKED,
        )
        .unwrap();
        let mut th = Thrower::new(Kind::Grenade, plan.target, plan.throw, SimTime(1.0))
            .planned(aim, plan, Delivery::COOKED)
            .from_jump();
        let out = throw_planned(&mut th, &aim);
        assert!(out.jumped && out.released > 0.0);
        assert!(!out.on_ground, "let go in the air");
        assert!((out.released - out.pin) as f32 <= grenade::LATEST);
    }

    const QUICK: Delivery = Delivery {
        latest: 0.75,
        quick: true,
        flat: true,
        retreat: true,
    };

    #[test]
    fn a_quick_grenade_goes_at_once_with_a_dash_at_the_enemy() {
        let dll = DllProfile::resolve("auto", true);
        let aim = Aim {
            pos: Vec3::new(400.0, 0.0, 0.0),
            vel: Vec3::ZERO,
            sigma: 20.0,
            spread: 110.0,
        };
        let eye = Vec3::new(0.0, 0.0, 28.0);
        let run = Vec3::new(270.0, 0.0, 0.0);
        let plan = grenade::plan(
            &mut Floor,
            eye,
            Vec3::ZERO,
            run,
            &aim,
            800.0,
            dll,
            0.0,
            0.0,
            None,
            QUICK,
        )
        .unwrap();
        let mut th = Thrower::new(Kind::Grenade, plan.target, plan.throw, SimTime(1.0))
            .planned(aim, plan, QUICK)
            .on_the_run();
        let out = throw_planned(&mut th, &aim);
        let held = (out.released - out.pin) as f32;
        assert!(
            held <= QUICK.latest + RELEASE_WINDOW + 0.02,
            "let go with the pin out {held} s"
        );
        let toward = (aim.pos - out.origin).truncate().normalize();
        assert!(
            out.velocity.truncate().dot(toward) >= RUN_UP_MIN,
            "dashing at it: {:?}",
            out.velocity
        );
        assert!(th.backs_off(), "and backs off once it is out");
    }

    #[test]
    fn a_quick_grenade_with_no_throw_to_be_had_is_not_held_into_the_fuse() {
        let dll = DllProfile::resolve("auto", true);
        let near = Aim {
            pos: Vec3::new(400.0, 0.0, 0.0),
            vel: Vec3::ZERO,
            sigma: 20.0,
            spread: 110.0,
        };
        let eye = Vec3::new(0.0, 0.0, 28.0);
        let plan = grenade::plan(
            &mut Floor,
            eye,
            Vec3::ZERO,
            Vec3::ZERO,
            &near,
            800.0,
            dll,
            0.0,
            0.0,
            None,
            QUICK,
        )
        .unwrap();
        let mut th = Thrower::new(Kind::Grenade, plan.target, plan.throw, SimTime(1.0)).planned(near, plan, QUICK);
        // The enemy is gone far out of reach at once: no throw at it to be had.
        let gone = Aim {
            pos: Vec3::new(9000.0, 0.0, 0.0),
            ..near
        };
        let out = throw_planned(&mut th, &gone);
        let held = (out.released - out.pin) as f32;
        assert!(out.released > 0.0, "thrown");
        assert!(
            held <= QUICK.latest + RUN_OUT + 0.02,
            "let go with the pin out {held} s"
        );
        assert!(th.forced().is_some(), "{:?}", th.forced());
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
        // The lift makes another throw: it waits for the view to come onto it.
        airborne.now = SimTime(0.5);
        airborne.velocity = Vec3::new(250.0, 0.0, 150.0);
        let Status::Running(r) = th.update(&airborne, &mut Unchecked) else {
            panic!()
        };
        assert_eq!(
            r.weapon.unwrap().fire,
            Fire::None,
            "the view is not on the lifted throw yet"
        );
        airborne.now = SimTime(0.52);
        airborne.view = th.angles();
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
    fn a_satchel_on_the_run_leaves_running_at_the_target_and_never_short() {
        let target = Vec3::new(420.0, 0.0, -36.0);
        let throw = ballistics::satchel(&mut Unchecked, Vec3::ZERO, Vec3::ZERO, target, 800.0);
        let mut scene = Scene {
            view: Vec3::ZERO,
            arsenal: vec![Armed::new(WeaponId::Satchel, None, Some(1))],
            prediction: Prediction {
                current: Some(WeaponId::Satchel),
                primary_ammo: 1,
                ..Prediction::default()
            },
            dll: DllProfile::resolve("auto", true),
        };
        scene.prediction.weapons[WeaponId::Satchel as usize] = Some(PredictedWeapon::default());
        let fire = |th: &mut Thrower, h: &Hands<'_>| match th.update(h, &mut Unchecked) {
            Status::Running(r) => r.weapon.unwrap().fire,
            other => panic!("{other:?}"),
        };
        let mut th = Thrower::new(Kind::Satchel, target, throw, SimTime(0.0)).on_the_run();
        // Standing, a satchel would come down some 200 units off, by the bot: it runs at the target instead.
        let mut h = scene.hands(0.0, WeaponId::Satchel);
        h.view = th.angles();
        assert_eq!(fire(&mut th, &h), Fire::None);
        // Running at it in the air, falling from a hop: it would come down short.
        h.now = SimTime(0.3);
        h.velocity = Vec3::new(260.0, 0.0, -150.0);
        h.on_ground = false;
        let _ = fire(&mut th, &h);
        h.view = th.angles();
        assert_eq!(fire(&mut th, &h), Fire::None, "never short");
        // On the ground at a run: thrown, the run in the throw.
        let mut ground = th.clone();
        h.now = SimTime(0.45);
        h.velocity = Vec3::new(260.0, 0.0, 0.0);
        h.on_ground = true;
        let _ = fire(&mut ground, &h);
        h.view = ground.angles();
        assert_eq!(fire(&mut ground, &h), Fire::Primary);
        assert!(ground.throw.velocity.x > 450.0, "{:?}", ground.throw);
        // So it is at the top of the hop.
        h.on_ground = false;
        assert_eq!(fire(&mut th, &h), Fire::Primary, "from a hop");
        // Never getting to run at it: given up, the satchel kept.
        let mut stuck = Thrower::new(Kind::Satchel, target, throw, SimTime(0.0)).on_the_run();
        let mut h = scene.hands(0.0, WeaponId::Satchel);
        h.view = stuck.angles();
        for t in [0.0, 0.5, 1.0] {
            h.now = SimTime(t);
            assert_eq!(fire(&mut stuck, &h), Fire::None);
        }
        h.now = SimTime(1.3);
        assert_eq!(
            stuck.update(&h, &mut Unchecked),
            Status::Failed("the throw would not reach on the run")
        );
        // The target come close: kept, unless told it may be that close.
        let mut close = Thrower::new(Kind::Satchel, target, throw, SimTime(0.0)).on_the_run();
        close.target = Vec3::new(200.0, 0.0, -36.0);
        assert_eq!(
            close.update(&scene.hands(0.0, WeaponId::Satchel), &mut Unchecked),
            Status::Failed("the target came close")
        );
        let mut closer = Thrower::new(Kind::Satchel, target, throw, SimTime(0.0))
            .on_the_run()
            .closer(150.0);
        closer.target = Vec3::new(200.0, 0.0, -36.0);
        assert!(matches!(
            closer.update(&scene.hands(0.0, WeaponId::Satchel), &mut Unchecked),
            Status::Running(_)
        ));
    }

    #[test]
    fn a_satchel_from_a_long_jump_goes_far_and_never_from_a_plain_jump_that_would_not_get_there() {
        // Further than a satchel from a plain jump at a run gets (some 870 units).
        let target = Vec3::new(900.0, 0.0, -36.0);
        let throw = ballistics::satchel(&mut Unchecked, Vec3::ZERO, Vec3::ZERO, target, 800.0);
        let mut scene = Scene {
            view: Vec3::ZERO,
            arsenal: vec![Armed::new(WeaponId::Satchel, None, Some(1))],
            prediction: Prediction {
                current: Some(WeaponId::Satchel),
                primary_ammo: 1,
                ..Prediction::default()
            },
            dll: DllProfile::resolve("auto", true),
        };
        scene.prediction.weapons[WeaponId::Satchel as usize] = Some(PredictedWeapon::default());
        let request = |th: &mut Thrower, h: &Hands<'_>| match th.update(h, &mut Unchecked) {
            Status::Running(r) => r,
            other => panic!("{other:?}"),
        };
        for leap in [true, false] {
            let mut th = Thrower::new(Kind::Satchel, target, throw, SimTime(0.0));
            th = if leap { th.from_leap() } else { th.from_jump() };
            assert_eq!(th.leaps(), leap);
            // Running at it: far out of a throw's reach from the ground, but a long jump will carry the satchel.
            let mut h = scene.hands(0.0, WeaponId::Satchel);
            h.velocity = Vec3::new(260.0, 0.0, 0.0);
            h.view = dir_to_view_angles(target - h.eye);
            let r = request(&mut th, &h);
            assert_eq!(r.jump, leap, "the leap goes at once, along the view on the target");
            if !leap {
                continue;
            }
            // In the leap (560 along, 299 up at the takeoff): thrown once the view is on the throw it makes.
            h.now = SimTime(0.15);
            h.on_ground = false;
            h.velocity = Vec3::new(560.0, 0.0, 180.0);
            let _ = request(&mut th, &h);
            h.view = th.angles();
            assert_eq!(request(&mut th, &h).weapon.unwrap().fire, Fire::Primary);
            // A plain jump instead: the throw would not get there, the satchel stays in hand.
            let mut plain = Thrower::new(Kind::Satchel, target, throw, SimTime(0.0)).from_leap();
            let mut h = scene.hands(0.0, WeaponId::Satchel);
            h.velocity = Vec3::new(260.0, 0.0, 0.0);
            h.view = dir_to_view_angles(target - h.eye);
            assert!(request(&mut plain, &h).jump);
            h.on_ground = false;
            h.velocity = Vec3::new(260.0, 0.0, 170.0);
            for t in [0.15, 0.3] {
                h.now = SimTime(t);
                let _ = request(&mut plain, &h);
                h.view = plain.angles();
                assert_eq!(request(&mut plain, &h).weapon.unwrap().fire, Fire::None);
            }
            h.now = SimTime(0.45);
            assert_eq!(
                plain.update(&h, &mut Unchecked),
                Status::Failed("the throw would not reach from the jump")
            );
        }
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
