//! Tricks: what the bot tells navigation it may do on the way, and the tricks it decides on itself.
//!
//! How readily a bot long jumps is its skill's `longjump` times how much its style likes it against a balanced bot:
//! a skilled one gets about by long jumps whenever it has the module.
//!
//! - **On the way** ([`BotBrain::nav_tricks`], every frame): the long jump links open with the module, when the
//!   server allows long jumps. Long jumps along the way as readily as the bot takes them (a roll every 8–12 s);
//!   skills with bold long jumps take them round corners, down drops, over short stretches and one after another,
//!   and with more than 60 health a landing that hurts as long as 40 is left. The gauss boost links for skills with
//!   tricks and styles that gauss-jump, with a gauss, 40 uranium and 60 health, no enemy seen for two seconds.
//! - **A long jump at an enemy** ([`BotBrain::attack_leap`]): in a fight, with the module, closing in (the weapon in
//!   hand does poorly this far off, or it is the crowbar), at an enemy in sight 300–900 units away (bold: 250–1000)
//!   no more than 64 below or 40 above, the will to close in (health × aggression) of 20 at least, the view on the
//!   enemy (within 18° across, no more than 15° up or down) and moving: every half second (bold: quarter), as
//!   readily as the bot takes long jumps in a fight, when the flight followed through the server's traces comes down
//!   safely and nearer the enemy, and no snark is about the bot, the enemy or the landing. Then 0.9–1.4 s (bold:
//!   0.4–0.7 s) before the next. The aim and the shots go on in the air.
//! - **A long jump to dodge** ([`BotBrain::dodge_leap`], skills with `longjump_dodge`): for a dodge jump, a long
//!   jump aside (and in when closing in, back when backing off), and away from a blast about to go off: the view
//!   turns along it, the keys go, and in the air the view comes back to the enemy while the flight is left alone.
//! - **A gauss jump on the way** ([`BotBrain::gauss_leap`]): with the gauss in hand, 30 uranium and 60 health, no
//!   enemy about, on the way somewhere more than 1400 units or 12 nodes off: every 10–18 s, as likely as the style
//!   likes, navigation looks for a boost that lands further along the way (`NavService::gauss_leap`). Not far
//!   enough, it looks again in 4–6 s.
//! - The weapons' part of a boost (charge, turn, jump, let go) is the `GaussBoost` protocol, started when
//!   navigation stops at a boost's takeoff and asks for it.
//! - **GunGame:** in the warmup leaps at enemies come twice as readily; where a suicide costs a kill, gauss jumps and
//!   boosts need 80 health.

use lb_core::math::{angle_diff, dir_to_view_angles, view_angle_vectors};
use lb_core::rng::BotRng;
use lb_core::time::SimTime;
use lb_core::{Vec2, Vec3};
use lb_decision::GoalKind;
use lb_game::entities::ProjectileKind;
use lb_game::weapons::WeaponId;
use lb_motor::{LookIntent, MoveIntent, Prio, StanceIntent};
use lb_nav_api::{NavService, Tricks};

use crate::BotBrain;
use crate::arms::Active;
use crate::mind::{Body, Character};

/// Long jumps along the way are rolled for this often, seconds.
const RUNWAY_ROLL: [f32; 2] = [8.0, 12.0];
/// Paths take gauss boost links with this much uranium (a full charge takes 16, the rest is for the fight after);
/// a boost starts on a full charge's, and goes on as its charge eats it.
const BOOST_URANIUM: i32 = 40;
const BOOST_CHARGE_CELLS: i32 = 16;
const BOOST_HEALTH: f32 = 60.0;
/// ... and where a suicide costs a GunGame kill.
const DESCORE_BOOST_HEALTH: f32 = 80.0;
/// In the GunGame warmup (kills do not count, everyone has the crowbar and a long jump) leaps at enemies come this
/// much more readily.
const WARMUP_LEAPS: f32 = 2.0;
/// No enemy seen this long: calm enough to stop and charge for a boost.
const BOOST_CALM: f64 = 2.0;
/// A long jump at an enemy: how far (horizontally; not bold, bold), how far below and above, the least will, how
/// close the view must be to it (cosine across, degrees up or down), how fast the bot must be moving.
const LEAP_BAND: [[f32; 2]; 2] = [[300.0, 900.0], [250.0, 1000.0]];
const LEAP_DZ: [f32; 2] = [-64.0, 40.0];
const LEAP_WILL: f32 = 20.0;
const LEAP_FACING: f32 = 0.95;
const LEAP_PITCH: f32 = 15.0;
const LEAP_SPEED: f32 = 60.0;
/// Looked at this often; after a leap, the next no sooner than this (not bold, bold).
const LEAP_CHECK: [f64; 2] = [0.5, 0.25];
const LEAP_REST: [[f32; 2]; 2] = [[0.9, 1.4], [0.4, 0.7]];
/// A bold long jump along the way may land hard with more than this much health, as long as this much is left.
const HURT_HEALTH: f32 = 60.0;
const HURT_LEFT: f32 = 40.0;
/// A long jump to dodge: the view along it this close (degrees across, up or down) for the keys; the view comes
/// round within this long or the dodge is off.
const DODGE_AIM: f32 = 8.0;
const DODGE_PITCH: f32 = 12.0;
const DODGE_TURN: f64 = 0.5;
/// A dodge aside lands at least this far from the enemy, and, not closing in, this far at least and no more than
/// this much further off than the bot is now.
const DODGE_NEAR: f32 = 128.0;
const DODGE_KEEP: [f32; 2] = [200.0, 400.0];
/// The long jump keys are pressed this long: the motor lets go of duck for a command first when it is held.
const LEAP_PRESS: f64 = 0.15;
/// No leap with a snark seen this recently this close to the bot, the enemy or the landing.
const LEAP_SNARKS: f32 = 300.0;
const LEAP_SNARKS_SEEN: f64 = 1.0;
/// A gauss jump on the way: the uranium and the health it needs, how far off the destination must be, and how
/// often it is rolled for (or looked at again when the destination is near).
const GAUSS_URANIUM: i32 = 30;
const GAUSS_FAR: f32 = 1400.0;
const GAUSS_FAR_NODES: usize = 12;
const GAUSS_ROLL: [f32; 2] = [10.0, 18.0];
const GAUSS_NEAR_AGAIN: [f32; 2] = [4.0, 6.0];

/// What the tricks came to, for `lb brain` and the stand statistics.
#[derive(Clone, Debug, Default)]
pub struct TrickStats {
    /// Long jumps taken at an enemy, and to dodge (aside in a fight, away from a blast).
    pub leaps: u32,
    pub dodges: u32,
    /// Gauss jumps on the way navigation found a boost for, and gauss boosts started (links and those).
    pub gauss_jumps: u32,
    pub boosts: u32,
    /// Boosts whose charge went, and boosts given up (with why).
    pub boosts_fired: u32,
    pub boost_failures: Vec<(&'static str, u32)>,
}

impl TrickStats {
    pub(crate) fn boost_failed(&mut self, why: &'static str) {
        match self.boost_failures.iter_mut().find(|(w, _)| *w == why) {
            Some(f) => f.1 += 1,
            None => self.boost_failures.push((why, 1)),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct TrickState {
    /// What navigation was told it may do on the last frame, and the uranium then.
    pub told: Tricks,
    pub uranium: i32,
    /// Why the last look for a gauss jump on the way came to nothing.
    pub gauss_why: &'static str,
    /// Long jumps along the way are taken (the style's roll) until then.
    runway: bool,
    runway_until: SimTime,
    next_leap: SimTime,
    /// A long jump at an enemy is being pressed until then.
    leap_until: SimTime,
    /// A long jump to dodge under way.
    dodge: Option<DodgeLeap>,
    next_gauss: SimTime,
    pub stats: TrickStats,
}

/// A long jump to dodge: along `dir`, turning the view there since `since`, the keys pressed then, off the ground.
#[derive(Clone, Copy, Debug)]
struct DodgeLeap {
    dir: Vec2,
    since: SimTime,
    pressed: Option<SimTime>,
    off: bool,
}

impl TrickState {
    /// A new life: timers start over, statistics stay.
    pub fn reset(&mut self) {
        let stats = std::mem::take(&mut self.stats);
        *self = TrickState {
            stats,
            ..TrickState::default()
        };
    }
}

/// The health a gauss jump or boost needs.
fn boost_health(body: &Body) -> f32 {
    if body.gungame.is_some_and(|g| g.descore()) {
        DESCORE_BOOST_HEALTH
    } else {
        BOOST_HEALTH
    }
}

/// How readily the bot takes a long jump: along the way (`fight` false) or in a fight. The skill's readiness times
/// how much the style likes it against a balanced bot's liking.
pub fn leap_chance(ch: &Character, fight: bool) -> f32 {
    let balanced = lb_styles::StyleId::Balanced.trick_likes();
    let (like, base) = if fight {
        (ch.tricks.lj_attack, balanced.lj_attack)
    } else {
        (ch.tricks.longjump, balanced.longjump)
    };
    (ch.skill.longjump * like / base).clamp(0.0, 1.0)
}

impl BotBrain {
    /// What navigation may do on the way this frame.
    pub(crate) fn nav_tricks(&mut self, body: &Body, ch: &Character, rng: &mut BotRng) -> Tricks {
        let now = body.now;
        let t = &mut self.mind.tricks;
        if now >= t.runway_until {
            t.runway = rng.decision.next_f32() < leap_chance(ch, false);
            t.runway_until = now + f64::from(rng.decision.range_f32(RUNWAY_ROLL[0], RUNWAY_ROLL[1]));
        }
        let longjump = body.has_longjump && body.tricks.longjump;
        let uranium = self.hands(body).reserve(WeaponId::Gauss);
        let calm = self.beliefs.visible_enemies().next().is_none()
            && (self.mind.last_enemy_seen() == SimTime::ZERO || now.since(self.mind.last_enemy_seen()) > BOOST_CALM);
        let boosting = matches!(self.mind.arms.active, Some(Active::GaussBoost(_)));
        let boost_now = ch.skill.tricks
            && body.allows(WeaponId::Gauss)
            && (boosting || uranium >= BOOST_CHARGE_CELLS)
            && body.health >= boost_health(body)
            && body.waterlevel < 2
            && calm;
        let bold = ch.skill.longjump_bold;
        // Dropping a trail's mines takes the ground: no long jumps along the way meanwhile.
        let laying = self.mind.arms.trail.as_ref().is_some_and(|p| p.laying(now));
        // A weapon protocol (a throw, a mine, the scope) or a long jump of a fight has the keys: no hops meanwhile.
        let t = &self.mind.tricks;
        let hop_free =
            body.tricks.bhop && self.mind.arms.active.is_none() && !laying && t.dodge.is_none() && now >= t.leap_until;
        let bhop = ch
            .skill
            .bhop_speed
            .filter(|_| hop_free)
            .map(|capped| [capped, ch.skill.bhop_speed_uncapped.unwrap_or(capped)]);
        let told = Tricks {
            longjump,
            runway: longjump && self.mind.tricks.runway && !laying,
            runway_bold: bold,
            runway_hurt: if bold && body.health > HURT_HEALTH {
                body.health - HURT_LEFT
            } else {
                0.0
            },
            gauss_boost: body.tricks.gauss_boost && ch.tricks.gauss_jump > 0.0 && uranium >= BOOST_URANIUM,
            boost_now,
            gauss_damage: body.damages.gauss_charged,
            selfgauss: body.selfgauss == 1,
            bhop,
        };
        self.mind.tricks.told = told;
        self.mind.tricks.uranium = uranium;
        told
    }

    /// A long jump at the enemy fought, at `enemy` (in sight: `visible`), when the bot is `closing` in: whether to
    /// press it this frame (a leap decided on is pressed for `LEAP_PRESS`).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn attack_leap(
        &mut self,
        body: &Body,
        ch: &Character,
        enemy: Vec3,
        visible: bool,
        closing: bool,
        nav: &mut dyn NavService,
        rng: &mut BotRng,
    ) -> bool {
        let now = body.now;
        if now < self.mind.tricks.leap_until {
            return body.on_ground;
        }
        let warmup = body.gungame.is_some_and(|g| g.warmup);
        let chance = (leap_chance(ch, true) * if warmup { WARMUP_LEAPS } else { 1.0 }).min(1.0);
        let allowed = body.has_longjump && body.tricks.longjump && ch.skill.tricks && chance > 0.0;
        if !allowed || !closing || now < self.mind.tricks.next_leap || !visible {
            return false;
        }
        if !body.on_ground || body.on_ladder || body.waterlevel > 0 || body.velocity.truncate().length() < LEAP_SPEED {
            return false;
        }
        let m = &self.mind;
        if m.arms.busy() || m.reloading(now) || now < m.arms.hold_until || m.tricks.dodge.is_some() {
            return false;
        }
        let b = usize::from(ch.skill.longjump_bold);
        let to = enemy - body.origin;
        let d = to.truncate().length();
        let will = body.health.clamp(0.0, 100.0) * ch.aggression;
        let band = LEAP_BAND[b];
        if !(band[0]..=band[1]).contains(&d) || !(LEAP_DZ[0]..=LEAP_DZ[1]).contains(&to.z) || will < LEAP_WILL {
            return false;
        }
        let view = self.motor.view;
        let (forward, _, _) = view_angle_vectors(view);
        let facing = forward.truncate().normalize_or_zero().dot(to.truncate() / d.max(1.0));
        if facing < LEAP_FACING || view.x.abs() > LEAP_PITCH {
            return false;
        }
        self.mind.tricks.next_leap = now + LEAP_CHECK[b];
        if rng.combat.next_f32() >= chance {
            return false;
        }
        if self.snarks_by(now, enemy) || self.snarks_by(now, body.origin) {
            return false;
        }
        let Some(landing) = nav.leap_lands(view) else {
            return false;
        };
        if landing.distance(enemy) >= d || self.snarks_by(now, landing) {
            return false;
        }
        let tricks = &mut self.mind.tricks;
        tricks.leap_until = now + LEAP_PRESS;
        let rest = LEAP_REST[b];
        tricks.next_leap = now + LEAP_PRESS + f64::from(rng.combat.range_f32(rest[0], rest[1]));
        tricks.stats.leaps += 1;
        tracing::debug!(
            "long jump at an enemy {d:.0} units away, landing {:.0} from it",
            landing.distance(enemy)
        );
        true
    }

    /// The keys of a long jump at an enemy, over whatever the fight asks of the stance.
    pub(crate) fn leap_stance(&mut self) {
        self.intents.stance(
            Prio::Threat,
            StanceIntent {
                jump: false,
                duck: false,
                longjump: true,
            },
        );
    }

    /// Snarks, the bot's own or anyone's, seen a moment ago near `p`: they bite whoever comes down among them.
    fn snarks_by(&self, now: SimTime, p: Vec3) -> bool {
        self.explosives.flying.iter().any(|f| {
            f.kind == ProjectileKind::Snark && now.since(f.seen) <= LEAP_SNARKS_SEEN && f.pos.distance(p) < LEAP_SNARKS
        })
    }

    /// A long jump to dodge, instead of a dodge jump in a fight with the enemy at `enemy`: aside the way the bot
    /// strafes (`moving`, the fight's move), in a little when it closes in, back when it backs off, or the other
    /// side; landing out of the enemy's way. Whether one was started.
    pub(crate) fn dodge_aside(
        &mut self,
        body: &Body,
        ch: &Character,
        enemy: Vec3,
        moving: Vec2,
        nav: &mut dyn NavService,
        rng: &mut BotRng,
    ) -> bool {
        let to = (enemy - body.origin).truncate();
        let distance = to.length();
        let to = to / distance.max(1.0);
        let right = Vec2::new(to.y, -to.x);
        let side = if moving.dot(right).abs() > 1.0 {
            moving.dot(right).signum()
        } else if rng.combat.next_f32() < 0.5 {
            1.0
        } else {
            -1.0
        };
        let toward = moving.dot(to);
        let ahead = if toward > 1.0 {
            0.8
        } else if toward < -1.0 {
            -0.6
        } else {
            0.35
        };
        let dirs = [
            (right * side + to * ahead).normalize(),
            (right * -side + to * ahead).normalize(),
        ];
        let keep = |landing: Vec3| {
            let d = landing.truncate().distance(enemy.truncate());
            d >= DODGE_NEAR
                && if toward > 1.0 {
                    d < distance
                } else {
                    (DODGE_KEEP[0]..=distance + DODGE_KEEP[1]).contains(&d)
                }
        };
        self.dodge_leap(body, ch, &dirs, &keep, nav, rng)
    }

    /// Starts a long jump to dodge along the first of `dirs` whose landing (followed through the server's traces,
    /// coming down safely) `keep` takes, when the bot may dodge by long jumps and nothing else holds it.
    pub(crate) fn dodge_leap(
        &mut self,
        body: &Body,
        ch: &Character,
        dirs: &[Vec2],
        keep: &dyn Fn(Vec3) -> bool,
        nav: &mut dyn NavService,
        rng: &mut BotRng,
    ) -> bool {
        let now = body.now;
        let t = &self.mind.tricks;
        let allowed = body.has_longjump && body.tricks.longjump && ch.skill.tricks && ch.skill.longjump_dodge;
        if !allowed || t.dodge.is_some() || now < t.leap_until {
            return false;
        }
        // A traversal navigation cannot interrupt (a long jump of the way taking off, a flight) has the stance.
        let traversing = self.intents.stance.is_some_and(|(p, _)| p >= Prio::Traversal);
        if !body.on_ground || body.on_ladder || body.waterlevel > 0 || self.mind.arms.busy() || traversing {
            return false;
        }
        if rng.combat.next_f32() >= leap_chance(ch, true) || self.snarks_by(now, body.origin) {
            return false;
        }
        for &dir in dirs {
            let view = Vec3::new(0.0, dir_to_view_angles(dir.extend(0.0)).y, 0.0);
            let Some(landing) = nav.leap_lands(view) else {
                continue;
            };
            if !keep(landing) || self.snarks_by(now, landing) {
                continue;
            }
            self.mind.tricks.dodge = Some(DodgeLeap {
                dir,
                since: now,
                pressed: None,
                off: false,
            });
            self.mind.tricks.stats.dodges += 1;
            tracing::debug!("long jump to dodge, landing {:.0} away", landing.distance(body.origin));
            return true;
        }
        false
    }

    /// Every frame: the long jump to dodge under way. The view turns along it and the bot runs that way; lined up
    /// on the ground, the keys go; in the air the legs stay tucked and no key brakes the flight, while the view is
    /// free for the aim.
    pub(crate) fn dodge_leap_tick(&mut self, body: &Body) {
        let Some(mut d) = self.mind.tricks.dodge else {
            return;
        };
        let now = body.now;
        let airborne = !body.on_ground && !body.on_ladder && body.waterlevel < 2;
        d.off |= d.pressed.is_some() && airborne;
        if d.off {
            if !airborne {
                self.mind.tricks.dodge = None;
                return;
            }
            self.intents.movement(
                Prio::Protocol,
                MoveIntent {
                    dir: Vec2::ZERO,
                    speed: 0.0,
                },
            );
            self.intents.stance(
                Prio::Protocol,
                StanceIntent {
                    jump: false,
                    duck: true,
                    longjump: false,
                },
            );
            self.mind.tricks.dodge = Some(d);
            return;
        }
        let late = d
            .pressed
            .map_or(now.since(d.since) > DODGE_TURN, |p| now.since(p) > LEAP_PRESS + 0.2);
        if late || body.on_ladder || body.waterlevel > 0 {
            self.mind.tricks.dodge = None;
            return;
        }
        let yaw = dir_to_view_angles(d.dir.extend(0.0)).y;
        self.intents
            .look(Prio::Protocol, LookIntent::Angles(Vec3::new(0.0, yaw, 0.0)));
        self.intents.movement(
            Prio::Protocol,
            MoveIntent {
                dir: d.dir,
                speed: body.maxspeed,
            },
        );
        let view = self.motor.view;
        let lined_up = angle_diff(view.y, yaw).abs() <= DODGE_AIM && view.x.abs() <= DODGE_PITCH;
        if d.pressed.is_some() || (lined_up && body.on_ground && body.velocity.truncate().length() > LEAP_SPEED) {
            d.pressed.get_or_insert(now);
            self.intents.stance(
                Prio::Protocol,
                StanceIntent {
                    jump: false,
                    duck: false,
                    longjump: true,
                },
            );
        }
        self.mind.tricks.dodge = Some(d);
    }

    /// A gauss jump on the way somewhere far, when it is time for one: asks navigation for a boost that lands
    /// further along the way.
    pub(crate) fn gauss_leap(&mut self, body: &Body, ch: &Character, nav: &mut dyn NavService, rng: &mut BotRng) {
        let now = body.now;
        let t = &self.mind.tricks;
        if now < t.next_gauss || !body.tricks.gauss_jump || !ch.skill.tricks || ch.tricks.gauss_jump <= 0.0 {
            return;
        }
        let travel = matches!(
            self.mind.goal.map(|g| g.kind),
            Some(
                GoalKind::Roam
                    | GoalKind::Hunt(_)
                    | GoalKind::CollectItem(_)
                    | GoalKind::Investigate(_)
                    | GoalKind::ControlItem(_)
                    | GoalKind::UseCharger(_)
                    | GoalKind::Camp(_)
                    | GoalKind::PlantTrap(_)
            )
        ) && self.mind.arms.trail.as_ref().is_none_or(|p| !p.laying(now));
        let hands = self.hands(body);
        let uranium = hands.reserve(WeaponId::Gauss);
        let calm = self.beliefs.visible_enemies().next().is_none()
            && (self.mind.last_enemy_seen() == SimTime::ZERO || now.since(self.mind.last_enemy_seen()) > BOOST_CALM);
        let why = if !travel {
            "not on the way anywhere"
        } else if !hands.ready(WeaponId::Gauss) {
            "no gauss ready in hand"
        } else if uranium < GAUSS_URANIUM {
            "too little uranium"
        } else if body.health < boost_health(body) {
            "too little health"
        } else if !calm {
            "an enemy about"
        } else if !body.on_ground || body.waterlevel > 0 || self.mind.arms.busy() {
            "busy"
        } else {
            ""
        };
        if !why.is_empty() {
            self.mind.tricks.gauss_why = why;
            return;
        }
        let far = nav
            .way_left()
            .is_some_and(|(d, nodes)| d > GAUSS_FAR || nodes > GAUSS_FAR_NODES);
        let tricks = &mut self.mind.tricks;
        if !far {
            tricks.gauss_why = "not far";
            tricks.next_gauss = now + f64::from(rng.decision.range_f32(GAUSS_NEAR_AGAIN[0], GAUSS_NEAR_AGAIN[1]));
            return;
        }
        tricks.next_gauss = now + f64::from(rng.decision.range_f32(GAUSS_ROLL[0], GAUSS_ROLL[1]));
        if rng.decision.next_f32() >= ch.tricks.gauss_jump {
            tricks.gauss_why = "not this time";
            return;
        }
        if nav.gauss_leap() {
            tricks.stats.gauss_jumps += 1;
            tricks.gauss_why = "found";
            tracing::debug!("gauss jump on the way");
        } else {
            tricks.gauss_why = "no boost lands further along";
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mind::WeaponLike;
    use lb_combat::Armed;
    use lb_combat::arms::boost::GaussBoost;
    use lb_config::skill::Presets;
    use lb_core::input::{IN_ATTACK2, IN_DUCK, IN_JUMP};
    use lb_core::rng::Pcg32;
    use lb_game::self_state::{PredictedWeapon, Prediction};
    use lb_knowledge::{BeliefParams, PlayerKey, Relation, RenderCue, Sighting, Stance, parts};
    use lb_nav_api::{BoostCall, NavStatus, NavStep};
    use lb_worldq::{Trace, TraceQuery, Tracer, contents};

    /// Open floor at z = -36; tells what the brain asked of it.
    struct Nav {
        tricks: Tricks,
        lands: Option<Vec3>,
        step: NavStep,
        leaps: u32,
    }

    impl Nav {
        fn new() -> Nav {
            Nav {
                tricks: Tricks::default(),
                lands: None,
                step: NavStep::hold(Vec3::new(1000.0, 0.0, 28.0)),
                leaps: 0,
            }
        }
    }

    impl Tracer for Nav {
        fn trace(&mut self, q: &TraceQuery) -> Trace {
            if q.end.z < -36.0 && q.start.z >= -36.0 {
                let f = (q.start.z + 36.0) / (q.start.z - q.end.z);
                let mut t = Trace::clear(q.start + (q.end - q.start) * f);
                t.fraction = f;
                t.normal = Vec3::Z;
                return t;
            }
            Trace::clear(q.end)
        }

        fn point_contents(&mut self, _p: Vec3) -> i32 {
            contents::EMPTY
        }
    }

    impl NavService for Nav {
        fn go_to(&mut self, _dest: Vec3) -> (NavStatus, Option<NavStep>) {
            (NavStatus::Moving, Some(self.step))
        }
        fn roam(&mut self, _rng: &mut Pcg32) -> Option<NavStep> {
            Some(self.step)
        }
        fn away_from(&mut self, _threat: Vec3) -> Option<Vec3> {
            None
        }
        fn available(&self) -> bool {
            true
        }
        fn set_tricks(&mut self, tricks: Tricks) {
            self.tricks = tricks;
        }
        fn leap_lands(&mut self, _view: Vec3) -> Option<Vec3> {
            self.lands
        }
        fn gauss_leap(&mut self) -> bool {
            self.leaps += 1;
            true
        }
        fn way_left(&self) -> Option<(f32, usize)> {
            Some((2400.0, 20))
        }
    }

    fn new_brain() -> BotBrain {
        BotBrain::new(
            1,
            lb_perception::PerceptionParams::from_skill(&Presets::default().at(75)),
        )
    }

    fn character(level: u8, tricks: lb_styles::TrickLikes) -> Character {
        Character {
            skill: Presets::default().at(level),
            level,
            aggression: 0.8,
            fear: 0.2,
            affinity: lb_styles::StyleId::Balanced.goal_affinity(),
            weapons: WeaponLike::default(),
            tricks,
        }
    }

    fn body(now: f64) -> Body {
        Body {
            now: SimTime(now),
            dt: 0.01,
            origin: Vec3::ZERO,
            eye: Vec3::new(0.0, 0.0, 28.0),
            velocity: Vec3::new(200.0, 0.0, 0.0),
            maxspeed: 300.0,
            health: 100.0,
            armor: 0.0,
            has_longjump: true,
            on_ground: true,
            ducked: false,
            on_ladder: false,
            underwater: false,
            waterlevel: 0,
            fov: 0.0,
            weapon: Some(WeaponId::Mp5),
            arsenal: [Armed::new(WeaponId::Mp5, Some(50), Some(100))].into_iter().collect(),
            prediction: None,
            ammo_need: [0.0; 7],
            opponents: 2,
            damages: lb_game::mechanics::Damages::default(),
            dll: lb_game::dll::DllProfile::default(),
            gravity: 800.0,
            allowed: u32::MAX,
            gungame: None,
            selfgauss: 0,
            tricks: lb_config::main_config::TricksConfig::default(),
        }
    }

    fn seen(t: f64, pos: Vec3) -> Sighting {
        Sighting {
            who: PlayerKey { slot: 5, userid: 50 },
            relation: Relation::Enemy,
            t: SimTime(t),
            pos,
            sigma: 1.0,
            distance: pos.length(),
            visibility: 1.0,
            parts: parts::CHEST,
            stance: Stance::Standing,
            on_ground: true,
            on_ladder: false,
            in_water: false,
            facing: 180.0,
            weapon: None,
            firing: false,
            render: RenderCue::default(),
            first: true,
            noticed_at: SimTime(t),
        }
    }

    const PARAMS: BeliefParams = BeliefParams {
        track_forget: 12.0,
        maxspeed: 300.0,
    };

    /// Frames of `secs` with an enemy in sight at `enemy` (none: calm), `dress` setting the body up; the buttons of
    /// every command.
    fn frames(
        brain: &mut BotBrain,
        ch: &Character,
        nav: &mut Nav,
        enemy: Option<Vec3>,
        secs: f64,
        dress: &mut dyn FnMut(f64, &mut Body, &mut Nav),
    ) -> Vec<u16> {
        let mut rng = BotRng::new(3, 3);
        let mut out = Vec::new();
        let mut t = 1.0;
        while t < 1.0 + secs {
            if let Some(pos) = enemy {
                brain.beliefs.on_sighting(&seen(t, pos));
            }
            brain.update(SimTime(t), &PARAMS, None, None);
            let mut b = body(t);
            dress(t, &mut b, nav);
            let cmd = brain.act(&b, ch, nav, None, &mut rng);
            brain.motor.sent(cmd.buttons);
            out.push(cmd.buttons);
            t += 0.01;
        }
        out
    }

    fn leapt(buttons: &[u16]) -> bool {
        buttons.iter().any(|b| b & (IN_JUMP | IN_DUCK) == IN_JUMP | IN_DUCK)
    }

    #[test]
    fn a_bot_with_the_module_long_jumps_at_an_enemy_it_closes_in_on() {
        let rusher = lb_styles::StyleId::Rusher.trick_likes();
        // Long jumps at the enemy, long jumps of any kind.
        let run = |level: u8, enemy: Vec3, module: bool, weapon: WeaponId| {
            let mut brain = new_brain();
            let mut nav = Nav::new();
            nav.lands = Some(enemy * 0.6);
            let ch = character(level, rusher);
            let buttons = frames(&mut brain, &ch, &mut nav, Some(enemy), 3.0, &mut |_, b, _| {
                b.has_longjump = module;
                b.weapon = Some(weapon);
                b.arsenal = [Armed::new(weapon, Some(8), Some(40))].into_iter().collect();
            });
            (brain.mind.tricks.stats.leaps, leapt(&buttons))
        };
        let at = Vec3::new(600.0, 0.0, 0.0);
        let (leaps, pressed) = run(75, at, true, WeaponId::Shotgun);
        assert!(pressed && leaps >= 1, "{leaps}");
        assert!(!run(75, at, false, WeaponId::Shotgun).1, "no module");
        assert!(!run(10, at, true, WeaponId::Shotgun).1, "a beginner does no tricks");
        assert_eq!(
            run(75, Vec3::new(1300.0, 0.0, 0.0), true, WeaponId::Shotgun).0,
            0,
            "too far"
        );
        assert_eq!(
            run(75, Vec3::new(600.0, 0.0, 120.0), true, WeaponId::Shotgun).0,
            0,
            "too high above"
        );
        assert_eq!(
            run(75, at, true, WeaponId::Mp5).0,
            0,
            "the MP5 does well this far off: no need to close in"
        );
    }

    #[test]
    fn a_skilled_bot_dodges_by_a_long_jump_aside_and_looks_back_in_the_air() {
        let balanced = lb_styles::StyleId::Balanced.trick_likes();
        let enemy = Vec3::new(500.0, 0.0, 0.0);
        let run = |level: u8| {
            let mut brain = new_brain();
            let mut nav = Nav::new();
            // In and aside: the shotgun wants the bot closer.
            nav.lands = Some(Vec3::new(300.0, 250.0, 0.0));
            let ch = character(level, balanced);
            let mut rng = BotRng::new(3, 3);
            // The enemy aims at the bot; it takes off when the keys go, and is in the air for half a second.
            let mut took_off: Option<f64> = None;
            let mut pressed_view = None;
            let mut air_views = Vec::new();
            let mut t = 1.0;
            while t < 6.0 {
                brain.beliefs.on_sighting(&seen(t, enemy));
                brain.update(SimTime(t), &PARAMS, None, None);
                let mut b = body(t);
                b.weapon = Some(WeaponId::Shotgun);
                b.arsenal = [Armed::new(WeaponId::Shotgun, Some(8), Some(40))].into_iter().collect();
                let flying = took_off.is_some_and(|at| t - at < 0.5);
                b.on_ground = !flying;
                let cmd = brain.act(&b, &ch, &mut nav, None, &mut rng);
                brain.motor.sent(cmd.buttons);
                if flying {
                    air_views.push(brain.motor.view);
                } else if cmd.buttons & (IN_JUMP | IN_DUCK) == IN_JUMP | IN_DUCK && took_off.is_none() {
                    took_off = Some(t);
                    pressed_view = Some(brain.motor.view);
                }
                t += 0.01;
            }
            (brain.mind.tricks.stats.dodges, pressed_view, air_views)
        };
        let (dodges, pressed, air) = run(100);
        assert!(dodges >= 1, "an expert dodges by long jumps");
        let yaw = pressed.expect("the keys went").y;
        assert!(yaw.abs() > 30.0, "the view turned aside for the jump: {yaw}");
        let back = air.last().expect("in the air").y;
        assert!(back.abs() < 10.0, "back on the enemy in the air: {back}");
        assert_eq!(run(50).0, 0, "no long jump dodges below hard");
    }

    #[test]
    fn navigation_is_told_what_tricks_the_bot_may_do() {
        let balanced = lb_styles::StyleId::Balanced.trick_likes();
        let gauss = |_: f64, b: &mut Body, _: &mut Nav| b.arsenal.push(Armed::new(WeaponId::Gauss, None, Some(60)));
        let mut brain = new_brain();
        let mut nav = Nav::new();
        frames(&mut brain, &character(75, balanced), &mut nav, None, 0.2, &mut {
            gauss
        });
        let t = nav.tricks;
        assert!(
            t.longjump && t.gauss_boost && t.boost_now && t.gauss_damage > 0.0,
            "{t:?}"
        );
        let mut brain = new_brain();
        let mut nav = Nav::new();
        frames(
            &mut brain,
            &character(75, balanced),
            &mut nav,
            Some(Vec3::new(800.0, 0.0, 0.0)),
            0.2,
            &mut { gauss },
        );
        assert!(
            nav.tricks.gauss_boost && !nav.tricks.boost_now,
            "no boost with an enemy about"
        );
        let mut brain = new_brain();
        let mut nav = Nav::new();
        frames(&mut brain, &character(10, balanced), &mut nav, None, 0.2, &mut {
            gauss
        });
        assert!(
            nav.tricks.longjump && !nav.tricks.boost_now,
            "beginners long jump on the way, no more"
        );
    }

    #[test]
    fn bunny_hops_are_allowed_from_hard_on_while_nothing_else_has_the_keys() {
        let balanced = lb_styles::StyleId::Balanced.trick_likes();
        let told = |level: u8, dress: &mut dyn FnMut(&mut BotBrain, &mut Body)| {
            let mut brain = new_brain();
            let mut nav = Nav::new();
            let ch = character(level, balanced);
            let mut rng = BotRng::new(3, 3);
            brain.update(SimTime(1.0), &PARAMS, None, None);
            let mut b = body(1.0);
            dress(&mut brain, &mut b);
            brain.act(&b, &ch, &mut nav, None, &mut rng);
            nav.tricks.bhop
        };
        assert_eq!(told(75, &mut |_, _| {}), Some([1.5, 1.7]));
        assert_eq!(told(100, &mut |_, _| {}), Some([1.7, 2.0]));
        assert_eq!(told(74, &mut |_, _| {}), None, "not below hard");
        assert_eq!(
            told(100, &mut |_, b| b.tricks.bhop = false),
            None,
            "the config switched them off"
        );
        assert_eq!(
            told(100, &mut |brain, _| brain.mind.tricks.leap_until = SimTime(5.0)),
            None,
            "a long jump at an enemy has the keys"
        );
    }

    /// The gauss as the game has it: out when asked for, spinning while the secondary attack is held, firing when it
    /// is let go.
    fn gauss_game(spinning: &mut bool, buttons: Option<u16>, b: &mut Body) {
        if let Some(last) = buttons {
            *spinning = last & IN_ATTACK2 != 0;
        }
        b.weapon = Some(WeaponId::Gauss);
        b.arsenal = [Armed::new(WeaponId::Gauss, None, Some(60))].into_iter().collect();
        let mut p = Prediction {
            current: Some(WeaponId::Gauss),
            primary_ammo: 60,
            ..Prediction::default()
        };
        p.weapons[WeaponId::Gauss as usize] = Some(PredictedWeapon {
            in_attack: i32::from(*spinning),
            ..PredictedWeapon::default()
        });
        b.prediction = Some(p);
        b.velocity = Vec3::ZERO;
    }

    #[test]
    fn a_boost_navigation_asks_for_charges_turns_and_jumps() {
        let balanced = lb_styles::StyleId::Balanced.trick_likes();
        let view = Vec3::new(34.0, 180.0, 0.0);
        let mut brain = new_brain();
        let mut nav = Nav::new();
        nav.step.boost = Some(BoostCall { view, charge: 1.6 });
        let mut spinning = false;
        let mut last = None;
        let mut rng = BotRng::new(3, 3);
        let mut buttons = Vec::new();
        let mut jump_view = None;
        let mut t = 1.0;
        while t < 5.0 {
            brain.update(SimTime(t), &PARAMS, None, None);
            let mut b = body(t);
            gauss_game(&mut spinning, last, &mut b);
            let cmd = brain.act(&b, &character(75, balanced), &mut nav, None, &mut rng);
            brain.motor.sent(cmd.buttons);
            last = Some(cmd.buttons);
            buttons.push((t, cmd.buttons));
            // Thrown: navigation flies the bot now and asks no more.
            if cmd.buttons & IN_JUMP != 0 && nav.step.boost.take().is_some() {
                jump_view = Some(brain.motor.view);
            }
            t += 0.01;
        }
        let charged = buttons.iter().filter(|(_, b)| b & IN_ATTACK2 != 0).count();
        let jump = buttons.iter().find(|(_, b)| b & IN_JUMP != 0).map(|(t, _)| *t);
        assert!(charged as f64 * 0.01 >= 1.6, "charged {charged} frames");
        assert!(jump.is_some_and(|j| j >= 2.6), "jumped at {jump:?}");
        assert!(
            jump_view.is_some_and(|v| lb_combat::arms::settled(v, view, 3.0)),
            "{jump_view:?}"
        );
        assert_eq!(brain.mind.tricks.stats.boosts, 1);
    }

    #[test]
    fn a_boost_called_off_while_charging_leaves_the_charge_to_the_gauss() {
        let balanced = lb_styles::StyleId::Balanced.trick_likes();
        let mut brain = new_brain();
        let mut nav = Nav::new();
        nav.step.boost = Some(BoostCall {
            view: Vec3::new(34.0, 180.0, 0.0),
            charge: 1.6,
        });
        let mut spinning = false;
        let mut last = None;
        let mut rng = BotRng::new(3, 3);
        let mut t = 1.0;
        while t < 2.5 {
            if t > 2.0 {
                nav.step.boost = None;
            }
            brain.update(SimTime(t), &PARAMS, None, None);
            let mut b = body(t);
            gauss_game(&mut spinning, last, &mut b);
            let cmd = brain.act(&b, &character(75, balanced), &mut nav, None, &mut rng);
            brain.motor.sent(cmd.buttons);
            last = Some(cmd.buttons);
            t += 0.01;
        }
        assert!(
            brain.mind.arms.active.is_none() && brain.mind.arms.gauss.active(),
            "the gauss holds the charge"
        );
        assert!(spinning, "still charging");
    }

    #[test]
    fn a_boost_under_way_goes_on_as_its_charge_eats_the_uranium() {
        let ch = character(75, lb_styles::StyleId::Balanced.trick_likes());
        let mut brain = new_brain();
        let mut rng = BotRng::new(3, 3);
        let mut b = body(1.0);
        b.arsenal
            .push(Armed::new(WeaponId::Gauss, None, Some(BOOST_CHARGE_CELLS - 4)));
        assert!(
            !brain.nav_tricks(&b, &ch, &mut rng).boost_now,
            "none starts on less than a charge's"
        );
        brain.mind.arms.active = Some(Active::GaussBoost(GaussBoost::new(b.now, Vec3::ZERO, 1.6)));
        assert!(brain.nav_tricks(&b, &ch, &mut rng).boost_now, "one under way goes on");
    }

    #[test]
    fn a_gauss_jump_is_asked_for_on_the_way_somewhere_far() {
        let mut likes = lb_styles::StyleId::Balanced.trick_likes();
        likes.gauss_jump = 1.0;
        let mut brain = new_brain();
        let mut nav = Nav::new();
        let mut spinning = false;
        frames(
            &mut brain,
            &character(75, likes),
            &mut nav,
            None,
            2.0,
            &mut |_, b, _| gauss_game(&mut spinning, None, b),
        );
        assert_eq!(nav.leaps, 1, "once, then again in 10-18 s");
        assert_eq!(brain.mind.tricks.stats.gauss_jumps, 1);
        likes.gauss_jump = 0.0;
        let mut brain = new_brain();
        let mut nav = Nav::new();
        frames(
            &mut brain,
            &character(75, likes),
            &mut nav,
            None,
            2.0,
            &mut |_, b, _| gauss_game(&mut spinning, None, b),
        );
        assert_eq!(nav.leaps, 0, "a style that does not gauss-jump");
    }
}
