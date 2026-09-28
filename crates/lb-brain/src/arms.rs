//! Weapons beyond the trigger: which protocol runs, when a throw, a mine, a launched grenade or a detonation is worth
//! it (ten times a second, from what the bot believes), and dodging what it sees about to blow up.
//!
//! - **Throws:** three times a second at the nearest enemy in sight, or lost up to 3 s ago with its position still
//!   tight: a grenade 300–800 units away when a throw lands there and the blast spares the bot, a satchel at 150–400
//!   with a clear line to the spot, a snark at 150–800 likewise. Out of sight a grenade comes first (a satchel when
//!   close), in sight a snark; the chance per look is the kind's base times the skill's `throw_rate`, and a throw rests
//!   the arm 3–6 s. With no gun but the crowbar, throws are the weapon (yapb's grenade war).
//! - **The MP5's grenade** at a target in sight 300–700 units away, every 2.5–4 s, when the lob is clear; the bot does
//!   not close in on the target while its grenade or rocket is on the way.
//! - **Satchels** go off when an enemy is within 160 units of one and closer to it than the bot; a bot too close to
//!   be spared backs off first.
//! - **Tripmines** are shot when an enemy is within 140 units of one 400–1200 units away; a known enemy mine ahead is
//!   shot to clear the way when nothing else goes on. When quiet the bot now and then lays a mine across a corridor it
//!   walks along, never near a spawn point and not next to another mine.
//! - **Dodge:** a grenade coming down (its own too), an MP5 grenade landing or a rocket passing near the bot makes it
//!   run away from the blast (yapb ran toward it), checking for ledges.
//! - **Gauss:** its charge runs whenever the gauss is in hand; nothing else starts while it charges.

use lb_combat::arms::detonate::{MineShot, SatchelTrigger};
use lb_combat::arms::gauss::{Gauss, GaussInput};
use lb_combat::arms::launcher::Lob;
use lb_combat::arms::mine::Planter;
use lb_combat::arms::scope::Scope;
use lb_combat::arms::throw::{Kind, Thrower};
use lb_combat::arms::{Hands, Request, Status};
use lb_combat::ballistics;
use lb_combat::fight::drops;
use lb_combat::policy::ROCKET_MIN;
use lb_core::math::view_angle_vectors;
use lb_core::rng::BotRng;
use lb_core::time::SimTime;
use lb_core::{Vec2, Vec3};
use lb_decision::GoalKind;
use lb_game::mechanics::{WeaponClass, blast_radius, spec};
use lb_game::weapons::WeaponId;
use lb_knowledge::{EnemyTrack, PlayerKey, TrackState};
use lb_motor::{LookIntent, MoveIntent, Prio};
use lb_nav_api::NavService;
use lb_worldq::{TraceQuery, Tracer};

use crate::BotBrain;
use crate::mind::{Body, Character};

/// Throw windows (horizontal distance).
const GRENADE_BAND: [f32; 2] = [300.0, 800.0];
const SATCHEL_BAND: [f32; 2] = [150.0, 400.0];
const SNARK_BAND: [f32; 2] = [150.0, 800.0];
const LOB_BAND: [f32; 2] = [300.0, 700.0];
/// A throw's blast must land at least this far from the thrower.
const SELF_CLEAR: f32 = 300.0;
/// With nothing but throws, closer ones are worth it (the blast spares 250 units; a near grenade is the risk taken).
const WAR_GRENADE_MIN: f32 = 220.0;
const WAR_SELF_CLEAR: f32 = 280.0;
/// Seconds before a quick throw leaves the hand (the game's least).
const WAR_LEAD: f32 = 0.5;
/// An MP5 grenade bursts on the first thing it touches and the bot moves on while it flies: it must land further off,
/// and a hurt bot keeps it for far targets.
const LOB_CLEAR: f32 = 400.0;
const LOB_HURT: f32 = 40.0;
const LOB_HURT_CLEAR: f32 = 550.0;
/// Enemies out of sight are thrown at while lost this recently, placed this tightly.
const THROW_TRACK_AGE: f64 = 3.0;
const THROW_TRACK_SIGMA: f32 = 400.0;
/// Throws are weighed this often.
const THROW_CHECK: f64 = 0.3;
/// Chance per check to take a throw (times the skill's `throw_rate`): at an enemy out of sight and in sight.
const UNSEEN_GRENADE: f32 = 0.5;
const UNSEEN_SATCHEL: f32 = 0.35;
const UNSEEN_SNARK: f32 = 0.35;
const SEEN_GRENADE: f32 = 0.12;
const SEEN_SATCHEL: f32 = 0.15;
const SEEN_SNARK: f32 = 0.25;
/// Seconds before the next throw is weighed after one.
const THROW_REST: [f32; 2] = [3.0, 6.0];
/// An enemy this far above is out of throwing reach.
const TOO_HIGH: f32 = 500.0;
const SNARK_TOO_HIGH: f32 = 200.0;
/// An enemy within this of a satchel is worth setting it off.
const SATCHEL_VICTIM: f32 = 160.0;
/// The bot keeps this far beyond a satchel's blast before setting it off.
const SATCHEL_SPARED: f32 = 24.0;
const MINE_VICTIM: f32 = 140.0;
/// Far enough to be spared a mine's blast, near enough to hit its small box.
const MINE_SHOT_BAND: [f32; 2] = [400.0, 800.0];
/// A corridor this wide at most gets a mine; its nearer wall must be this close.
const CORRIDOR: f32 = 300.0;
const WALL_NEAR: f32 = 90.0;
const MINE_SPACING: f32 = 96.0;
const SPAWN_CLEAR: f32 = 256.0;
/// Quiet this long before laying a mine or clearing one.
const QUIET: f64 = 5.0;
/// A dodge lasts this long once started.
const DODGE_FOR: f64 = 0.5;
/// Someone else's snark this close is shot at; the bot's own when it comes back this close.
const SNARK_NEAR: f32 = 300.0;
const OWN_SNARK_NEAR: f32 = 150.0;
/// A snark this close is shot even with a player in sight, unless that player is closer than `SNARK_OVER_PLAYER`.
const SNARK_BITING: f32 = 150.0;
const SNARK_OVER_PLAYER: f32 = 300.0;
/// Snarks are shot with these, best first: none has a blast.
const SNARK_GUNS: [WeaponId; 5] = [
    WeaponId::Shotgun,
    WeaponId::Mp5,
    WeaponId::Glock,
    WeaponId::Python,
    WeaponId::Hornetgun,
];
/// Navigation keeps off a known beam this long, told again after `BEAM_REPORT`.
const BEAM_AVOID: f32 = 60.0;
const BEAM_REPORT: f64 = 30.0;
const BEAM_LENGTH: f32 = 2048.0;
const DODGE_MARGIN: f32 = 40.0;

#[derive(Clone, Debug)]
pub enum Active {
    Throw(Thrower),
    Mine(Planter),
    Lob(Lob),
    Detonate(SatchelTrigger),
    Shoot(MineShot),
    Scope(Scope),
}

impl Active {
    pub fn name(&self) -> &'static str {
        match self {
            Active::Throw(t) => t.kind.as_str(),
            Active::Mine(_) => "tripmine",
            Active::Lob(_) => "m203",
            Active::Detonate(_) => "detonate",
            Active::Shoot(_) => "shoot a mine",
            Active::Scope(_) => "scope",
        }
    }
}

/// What the weapon protocols did, for `lb brain` and the stand statistics.
#[derive(Clone, Debug, Default)]
pub struct ArmsStats {
    pub grenades: u32,
    pub satchels: u32,
    pub snarks: u32,
    pub mines: u32,
    pub lobs: u32,
    pub detonations: u32,
    pub mine_shots: u32,
    pub dodges: u32,
    /// Zoomed crossbow shots.
    pub scoped: u32,
    pub failed: u32,
    /// Failures by protocol and reason.
    pub failures: Vec<(&'static str, &'static str, u32)>,
}

impl ArmsStats {
    fn failure(&mut self, protocol: &'static str, why: &'static str) {
        self.failed += 1;
        match self.failures.iter_mut().find(|(p, w, _)| *p == protocol && *w == why) {
            Some(f) => f.2 += 1,
            None => self.failures.push((protocol, why, 1)),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Arms {
    pub gauss: Gauss,
    pub active: Option<Active>,
    pub stats: ArmsStats,
    pub last_failure: Option<&'static str>,
    /// This server's satchel buttons turned out the other way round than its DLL profile says.
    pub satchel_swapped: bool,
    /// A fired rocket is guided until then; the point it was fired at and the target.
    pub guide: Option<(SimTime, Vec3, PlayerKey)>,
    /// The bot's own rocket or launched grenade is on its way to the target until then.
    pub hold_until: SimTime,
    /// Both shotgun barrels on the next shot.
    pub double: bool,
    /// When the next zoom toggle may be pressed.
    pub zoom_ready: SimTime,
    /// When the view was last seen to zoom in or out: the game lets the scope toggle again a second after.
    pub zoom_flip: SimTime,
    zoomed: bool,
    next_throw: SimTime,
    next_lob: SimTime,
    next_mine: SimTime,
    /// How far back the bot may be thrown before it would drop further than is safe.
    recoil_room: f32,
    /// When the gauss's line of fire was last looked along, and how far its first wall was.
    gauss_wall: Option<(SimTime, f32)>,
    /// Running from a blast until then, along this direction.
    dodge: Option<(SimTime, Vec2)>,
}

impl Arms {
    /// A new life: what was going on stops; statistics and what was learned about the server stay.
    pub fn reset(&mut self) {
        let stats = std::mem::take(&mut self.stats);
        let swapped = self.satchel_swapped;
        let mut gauss = std::mem::take(&mut self.gauss);
        gauss.reset();
        *self = Arms {
            gauss,
            stats,
            satchel_swapped: swapped,
            ..Arms::default()
        };
    }

    /// Every frame: notes when the view zooms in or out.
    pub fn watch_zoom(&mut self, zoomed: bool, now: SimTime) {
        if zoomed != self.zoomed {
            self.zoomed = zoomed;
            self.zoom_flip = now;
        }
    }

    /// A protocol runs or the gauss charges: nothing else starts.
    pub fn busy(&self) -> bool {
        self.active.is_some() || self.gauss.active()
    }

    pub fn describe(&self, now: SimTime) -> String {
        let mut parts = Vec::new();
        if let Some(a) = &self.active {
            parts.push(a.name().to_string());
        }
        if self.gauss.active() {
            parts.push(format!("gauss {} {:.1} s", self.gauss.phase(), self.gauss.charge(now)));
        }
        if self.guide.is_some_and(|(until, _, _)| now < until) {
            parts.push("guiding a rocket".into());
        }
        if self.dodge.is_some_and(|(until, _)| now < until) {
            parts.push("dodging".into());
        }
        if parts.is_empty() { "-".into() } else { parts.join(", ") }
    }
}

/// The view is on the aim point closely enough for a shot that must count: within the angle a body's width makes at
/// that distance.
pub(crate) fn on_target(view: Vec3, eye: Vec3, aim: Vec3) -> bool {
    let (forward, _, _) = view_angle_vectors(view);
    let to = aim - eye;
    let d = to.length().max(1.0);
    let off = lb_core::dmath::acos(forward.dot(to / d).clamp(-1.0, 1.0));
    off <= lb_core::dmath::atan(ON_TARGET_HALF_WIDTH / d).max(ON_TARGET_MIN.to_radians())
}

/// Half a body's width less a margin, and the least angle counted on target, degrees.
const ON_TARGET_HALF_WIDTH: f32 = 12.0;
const ON_TARGET_MIN: f32 = 0.4;

/// A move is checked this far ahead against known beams.
const BEAM_LOOKAHEAD: f32 = 0.3;

/// A player standing at `p` touches the beam from `a` to `b`.
fn near_beam(p: Vec3, a: Vec3, b: Vec3) -> bool {
    let s = (b - a).truncate();
    let len = s.length_squared().max(1e-6);
    let u = ((p - a).truncate().dot(s) / len).clamp(0.0, 1.0);
    let on = a + (b - a) * u;
    (on.truncate() - p.truncate()).length() <= 20.0 && (on.z - p.z).abs() <= 36.0
}

/// Near the shooter's end of the line of fire: an explosive would burst on whoever stands there.
const CROWD_REACH: f32 = 350.0;
const CROWD_RADIUS: f32 = 64.0;

/// Someone the bot sees (other than `target`) stands near the first stretch of the line from `eye` to `at`, where an
/// explosive would burst on them close to the bot.
pub(crate) fn crowded(
    beliefs: &lb_knowledge::Beliefs,
    eye: Vec3,
    at: Vec3,
    target: Option<lb_knowledge::PlayerKey>,
) -> bool {
    let dir = (at - eye).normalize_or_zero();
    beliefs
        .tracks
        .iter()
        .filter(|t| t.state == TrackState::Visible && Some(t.who) != target)
        .any(|t| {
            let along = (t.pos - eye).dot(dir).clamp(0.0, CROWD_REACH);
            (eye + dir * along).distance(t.pos) <= CROWD_RADIUS
        })
}

/// An enemy in sight, or lost half a second ago with a tight position: sure enough to set off a trap on.
fn fresh(t: &EnemyTrack, now: SimTime) -> bool {
    t.state == TrackState::Visible || (t.age(now) <= 0.5 && t.sigma < 60.0)
}

/// Room behind for a charged gauss shot's throw: how far along `back` the bot can slide before the floor drops away
/// further than is safe; unbounded when a wall stops the throw first or no drop is near.
fn recoil_room(tracer: &mut dyn Tracer, origin: Vec3, back: Vec2) -> f32 {
    const STEP: f32 = 80.0;
    const REACH: f32 = 720.0;
    let dir = back.extend(0.0);
    let wall = tracer.trace(&TraceQuery::line(origin, origin + dir * REACH)).fraction * REACH;
    let mut d = STEP;
    while d < wall {
        // `drops` looks a fifth of a second along a velocity.
        if drops(tracer, origin, back * d * 5.0) {
            return d - STEP;
        }
        d += STEP;
    }
    f32::INFINITY
}

fn clear_line(tracer: &mut dyn Tracer, from: Vec3, to: Vec3) -> bool {
    tracer.trace(&TraceQuery::line(from, to)).fraction >= 0.95
}

impl BotBrain {
    pub(crate) fn hands<'a>(&self, body: &'a Body) -> Hands<'a> {
        let mut dll = body.dll;
        if self.mind.arms.satchel_swapped {
            dll.swap_satchel_buttons();
        }
        Hands {
            now: body.now,
            eye: body.eye,
            origin: body.origin,
            velocity: body.velocity,
            view: self.motor.view,
            on_ground: body.on_ground,
            on_ladder: body.on_ladder,
            waterlevel: body.waterlevel,
            fov: body.fov,
            weapon: body.weapon,
            arsenal: &body.arsenal,
            prediction: body.prediction.as_ref(),
            dll,
            gravity: body.gravity,
        }
    }

    /// Ten times a second: start a detonation, a mine shot, a launched grenade, a throw or a mine when one is worth
    /// it.
    pub(crate) fn weapon_options(&mut self, body: &Body, ch: &Character, nav: &mut dyn NavService, rng: &mut BotRng) {
        self.keep_off_beams(body, nav);
        let (forward, _, _) = view_angle_vectors(self.motor.view);
        let back = -forward.truncate().normalize_or(Vec2::X);
        self.mind.arms.recoil_room = if body.weapon == Some(WeaponId::Gauss) {
            recoil_room(nav, body.origin, back)
        } else {
            0.0
        };
        let satchel_out = body
            .prediction
            .and_then(|p| p.weapons[WeaponId::Satchel as usize])
            .map(|p| p.charge_ready);
        if satchel_out == Some(0) && !self.mind.arms.busy() {
            // The game reports none out: the charges went off or were taken away with the satchel.
            self.explosives.detonated();
        }
        if self.mind.arms.busy() || body.on_ladder {
            return;
        }
        if satchel_out == Some(1) && self.detonation(body) {
            return;
        }
        if self.mine_shot(body, nav) {
            return;
        }
        if self.lob(body, nav, rng) {
            return;
        }
        if self.throw(body, ch, nav, rng) {
            return;
        }
        self.lay_mine(body, nav, rng);
    }

    /// Tells navigation of every armed tripmine beam the bot knows of (its own mines' beams are traced once), again
    /// before the last word runs out.
    fn keep_off_beams(&mut self, body: &Body, nav: &mut dyn NavService) {
        let now = body.now;
        for m in &mut self.explosives.mines {
            if now < m.armed_at || m.avoided_at.is_some_and(|t| now.since(t) < BEAM_REPORT) {
                continue;
            }
            let end = match m.beam_end {
                Some(e) => e,
                None if m.dir != Vec3::ZERO => {
                    let e = nav.trace(&TraceQuery::line(m.pos, m.pos + m.dir * BEAM_LENGTH)).end;
                    m.beam_end = Some(e);
                    e
                }
                None => continue,
            };
            nav.avoid_line(m.pos, end, BEAM_AVOID);
            m.avoided_at = Some(now);
        }
    }

    fn detonation(&mut self, body: &Body) -> bool {
        let now = body.now;
        let charges = &self.explosives.charges;
        if charges.is_empty() {
            return false;
        }
        let victim = self.beliefs.enemies().filter(|t| fresh(t, now)).any(|t| {
            charges
                .iter()
                .any(|c| c.pos.distance(t.pos) <= SATCHEL_VICTIM && c.pos.distance(t.pos) < c.pos.distance(body.origin))
        });
        if !victim {
            return false;
        }
        let nearest = charges
            .iter()
            .min_by(|a, b| a.pos.distance(body.origin).total_cmp(&b.pos.distance(body.origin)))
            .map(|c| c.pos)
            .unwrap_or(body.origin);
        if nearest.distance(body.origin) < blast_radius(body.damages.satchel) + SATCHEL_SPARED {
            let away = (body.origin - nearest).truncate().normalize_or(Vec2::X);
            self.mind.arms.dodge = Some((now + DODGE_FOR, away));
            return true;
        }
        self.mind.arms.active = Some(Active::Detonate(SatchelTrigger::new(now)));
        true
    }

    fn mine_shot(&mut self, body: &Body, nav: &mut dyn NavService) -> bool {
        let now = body.now;
        let gun = [WeaponId::Python, WeaponId::Glock, WeaponId::Mp5, WeaponId::Gauss]
            .into_iter()
            .find(|w| body.arsenal.iter().any(|a| a.id == *w && a.loaded()) && body.allows(*w));
        let Some(gun) = gun else { return false };
        let calm = self.calm_for(now) >= QUIET;
        let heading = body.velocity.truncate().normalize_or_zero();
        let target = self.explosives.mines.iter().find_map(|m| {
            if now < m.armed_at {
                return None;
            }
            let d = m.pos.distance(body.eye);
            if !(MINE_SHOT_BAND[0]..=MINE_SHOT_BAND[1]).contains(&d) || d < blast_radius(body.damages.tripmine) + 25.0 {
                return None;
            }
            let victim = self
                .beliefs
                .enemies()
                .any(|t| fresh(t, now) && t.pos.distance(m.pos) <= MINE_VICTIM);
            let in_the_way = calm && !m.own && heading.dot((m.pos - body.origin).truncate().normalize_or_zero()) > 0.7;
            (victim || in_the_way).then_some(m.pos)
        });
        let Some(mine) = target else { return false };
        if nav.trace(&TraceQuery::line(body.eye, mine)).fraction < 0.9 {
            return false;
        }
        self.mind.arms.active = Some(Active::Shoot(MineShot::new(mine, gun, now)));
        true
    }

    fn lob(&mut self, body: &Body, nav: &mut dyn NavService, rng: &mut BotRng) -> bool {
        let now = body.now;
        let m = &self.mind;
        if body.weapon != Some(WeaponId::Mp5)
            || now < m.arms.next_lob
            || body.waterlevel >= 3
            || !body.allows(WeaponId::Mp5)
            || body
                .arsenal
                .iter()
                .find(|a| a.id == WeaponId::Mp5)
                .and_then(|a| a.reserve2)
                .unwrap_or(0)
                <= 0
        {
            return false;
        }
        let Some(t) = m
            .target
            .and_then(|k| self.beliefs.track(k))
            .filter(|t| t.state == TrackState::Visible)
        else {
            return false;
        };
        let d = t.pos.distance(body.eye);
        if !(LOB_BAND[0]..=LOB_BAND[1]).contains(&d) {
            return false;
        }
        let lead = if t.velocity_known(now) {
            t.vel * (d / 700.0)
        } else {
            Vec3::ZERO
        };
        let feet = t.pos + lead - Vec3::Z * 32.0;
        let clear = if body.health < LOB_HURT {
            LOB_HURT_CLEAR
        } else {
            LOB_CLEAR
        };
        if feet.distance(body.origin) < clear || crowded(&self.beliefs, body.eye, t.pos, Some(t.who)) {
            return false;
        }
        self.mind.arms.next_lob = now + f64::from(rng.combat.range_f32(2.5, 4.0));
        let Some(throw) = ballistics::m203(nav, body.eye, feet, body.gravity) else {
            return false;
        };
        self.mind.arms.active = Some(Active::Lob(Lob::new(throw, now)));
        true
    }

    fn throw(&mut self, body: &Body, ch: &Character, nav: &mut dyn NavService, rng: &mut BotRng) -> bool {
        let now = body.now;
        if now < self.mind.arms.next_throw || self.mind.reloading(now) {
            return false;
        }
        self.mind.arms.next_throw = now + THROW_CHECK;
        // With no gun but the crowbar, throws are the weapon (yapb's grenade war).
        let war = !body.arsenal.iter().any(|a| {
            let class = spec(a.id).class;
            body.allows(a.id) && a.loaded() && !matches!(class, WeaponClass::Melee | WeaponClass::Throwable)
        });
        let Some(t) = self
            .beliefs
            .enemies()
            .filter(|t| {
                t.state == TrackState::Visible
                    || (matches!(t.state, TrackState::RecentlyLost | TrackState::Predicted)
                        && t.age(now) <= THROW_TRACK_AGE
                        && t.sigma <= THROW_TRACK_SIGMA)
            })
            .min_by(|a, b| a.pos.distance(body.origin).total_cmp(&b.pos.distance(body.origin)))
        else {
            return false;
        };
        let seen = t.state == TrackState::Visible;
        let above = t.pos.z - body.origin.z;
        if above > TOO_HIGH || (!t.traits.on_ground && above > 72.0) {
            return false;
        }
        let d = (t.pos - body.origin).truncate().length();
        let count = |w: WeaponId| {
            if body.allows(w) {
                body.arsenal
                    .iter()
                    .find(|a| a.id == w)
                    .and_then(|a| a.reserve)
                    .unwrap_or(0)
            } else {
                0
            }
        };
        let coming = t.velocity_known(now) && t.vel.truncate().dot((body.origin - t.pos).truncate()) > 0.0;
        let grenade_band = if war {
            [WAR_GRENADE_MIN, GRENADE_BAND[1]]
        } else {
            GRENADE_BAND
        };
        let in_band = |band: [f32; 2]| (band[0]..=band[1]).contains(&d);
        // The kinds worth a throw now, best first: at an enemy out of sight a grenade, a satchel close by, a snark;
        // at one in sight a snark, a grenade, a satchel when it is coming this way.
        let mut kinds: smallvec::SmallVec<[Kind; 3]> = smallvec::SmallVec::new();
        let grenade = count(WeaponId::HandGrenade) > 0 && in_band(grenade_band);
        let satchel = count(WeaponId::Satchel) > 0
            && self.explosives.charges.is_empty()
            && in_band(SATCHEL_BAND)
            && (!seen || coming || war);
        let snark = count(WeaponId::Snark) > 0 && body.waterlevel < 2 && above <= SNARK_TOO_HIGH && in_band(SNARK_BAND);
        if seen {
            kinds.extend(
                [(snark, Kind::Snark), (grenade, Kind::Grenade), (satchel, Kind::Satchel)]
                    .into_iter()
                    .filter(|k| k.0)
                    .map(|k| k.1),
            );
        } else {
            if satchel && d < 300.0 {
                kinds.push(Kind::Satchel);
            }
            kinds.extend(
                [(grenade, Kind::Grenade), (satchel, Kind::Satchel), (snark, Kind::Snark)]
                    .into_iter()
                    .filter(|k| k.0 && !(k.1 == Kind::Satchel && d < 300.0))
                    .map(|k| k.1),
            );
        }
        let Some(&kind) = kinds.first() else { return false };
        let base = match (kind, seen) {
            (Kind::Grenade, false) => UNSEEN_GRENADE,
            (Kind::Satchel, false) => UNSEEN_SATCHEL,
            (Kind::Snark, false) => UNSEEN_SNARK,
            (Kind::Grenade, true) => SEEN_GRENADE,
            (Kind::Satchel, true) => SEEN_SATCHEL,
            (Kind::Snark, true) => SEEN_SNARK,
        };
        let bold = if ch.aggression > ch.fear { 1.1 } else { 0.9 };
        let chance = (base * ch.skill.throw_rate * bold * if war { 2.0 } else { 1.0 }).min(0.9);
        if rng.combat.next_f32() >= chance {
            return false;
        }
        // A target in sight is led by where it will be when the throw comes down.
        let lead = if seen && t.velocity_known(now) {
            t.vel.truncate().extend(0.0) * (WAR_LEAD + d / 650.0)
        } else {
            Vec3::ZERO
        };
        let floor = t.pos + lead - Vec3::Z * 32.0;
        let throw = match kind {
            Kind::Grenade => {
                let solved = ballistics::grenade(nav, body.eye, body.velocity, floor, body.gravity, body.dll, 2.4);
                let clear = if war { WAR_SELF_CLEAR } else { SELF_CLEAR };
                match solved {
                    Some(s) if floor.distance(body.origin) >= clear => s,
                    _ => return false,
                }
            }
            Kind::Satchel | Kind::Snark => {
                if !clear_line(nav, body.eye, floor + Vec3::Z * 8.0) {
                    return false;
                }
                ballistics::satchel(nav, body.origin, body.velocity, floor, body.gravity)
            }
        };
        let target = if kind == Kind::Snark { t.pos } else { floor };
        let thrower = Thrower::new(kind, target, throw, now);
        // At an enemy in sight a grenade goes at once: it will not wait for a cooked one.
        let thrower = if seen { thrower.quick() } else { thrower };
        self.mind.arms.active = Some(Active::Throw(thrower));
        self.mind.arms.next_throw = now + f64::from(rng.combat.range_f32(THROW_REST[0], THROW_REST[1]));
        true
    }

    fn lay_mine(&mut self, body: &Body, nav: &mut dyn NavService, rng: &mut BotRng) {
        let now = body.now;
        if now < self.mind.arms.next_mine {
            return;
        }
        self.mind.arms.next_mine = now + f64::from(rng.combat.range_f32(1.5, 3.0));
        let walking = matches!(
            self.mind.goal.map(|g| g.kind),
            Some(GoalKind::Roam | GoalKind::CollectItem(_))
        );
        let speed = body.velocity.truncate().length();
        if !walking
            || speed < 100.0
            || !body.on_ground
            || body.waterlevel >= 2
            || self.calm_for(now) < QUIET
            || !body.allows(WeaponId::Tripmine)
            || body
                .arsenal
                .iter()
                .find(|a| a.id == WeaponId::Tripmine)
                .and_then(|a| a.reserve)
                .unwrap_or(0)
                <= 0
        {
            return;
        }
        let heading = body.velocity.truncate() / speed;
        let across = Vec2::new(-heading.y, heading.x).extend(0.0);
        let waist = body.origin + Vec3::Z * 8.0;
        let side = |tracer: &mut dyn Tracer, dir: Vec3| {
            let tr = tracer.trace(&TraceQuery::line(waist, waist + dir * CORRIDOR));
            (tr.fraction < 1.0 && tr.normal.z.abs() <= 0.3).then_some((tr.end, tr.normal, tr.fraction * CORRIDOR))
        };
        let (Some(left), Some(right)) = (side(nav, across), side(nav, -across)) else {
            return;
        };
        if left.2 + right.2 > CORRIDOR {
            return;
        }
        let (spot, normal, near) = if left.2 <= right.2 { left } else { right };
        if near > WALL_NEAR {
            return;
        }
        let too_close = self.spawns.iter().any(|s| s.distance(spot) < SPAWN_CLEAR)
            || self
                .explosives
                .mines
                .iter()
                .any(|m| m.pos.distance(spot) < MINE_SPACING);
        if too_close {
            return;
        }
        self.mind.arms.active = Some(Active::Mine(Planter::new(spot, normal, now)));
        self.mind.arms.next_mine = now + f64::from(rng.combat.range_f32(20.0, 30.0));
    }

    /// Every frame: the gauss charge, the running protocol and a rocket in flight get their say at `Prio::Protocol`.
    pub(crate) fn run_protocols(&mut self, body: &Body, ch: &Character, nav: &mut dyn NavService, rng: &mut BotRng) {
        let now = body.now;
        self.mind.arms.watch_zoom(body.zoomed(), now);
        let hands = self.hands(body);
        let mut requests: smallvec::SmallVec<[Request; 2]> = smallvec::SmallVec::new();
        if body.weapon == Some(WeaponId::Gauss) || self.mind.arms.gauss.active() {
            let target = self
                .mind
                .target
                .and_then(|k| self.beliefs.track(k))
                .filter(|t| t.state == TrackState::Visible)
                .map(|t| {
                    let aim = self.mind.last_aim.unwrap_or(t.pos);
                    (t.pos.distance(body.eye), on_target(self.motor.view, body.eye, aim))
                });
            let expected = matches!(self.mind.goal.map(|g| g.kind), Some(GoalKind::Hunt(_)));
            let input = GaussInput {
                target,
                expected,
                precharge: ch.skill.gauss_precharge,
                recoil_room: self.mind.arms.recoil_room,
                wall_ahead: self.gauss_wall(body, nav),
                full_damage: body.damages.gauss_charged,
                heading: body.velocity.truncate().normalize_or_zero(),
                allowed: body.allows(WeaponId::Gauss),
                charge_share: ch.skill.gauss_charge,
            };
            if let Some(r) = self.mind.arms.gauss.update(&hands, &input, &mut rng.combat) {
                requests.push(r);
            }
        }
        if let Some(mut active) = self.mind.arms.active.take() {
            let in_sight = self.beliefs.visible_enemies().next().is_some();
            let status = match &mut active {
                Active::Throw(t) => t.update(&hands, nav),
                // Laying a mine with an enemy in sight is left for later.
                Active::Mine(_) if in_sight => Status::Failed("an enemy came into sight"),
                Active::Mine(p) => p.update(&hands, nav),
                Active::Lob(l) => {
                    let gravity = body.gravity * lb_game::mechanics::PROJECTILE_GRAVITY;
                    let landing = ballistics::position_at(l.throw.start, l.throw.velocity, gravity, l.throw.flight);
                    let target = self.mind.target.and_then(|k| self.beliefs.track(k));
                    let clear = target.is_none_or(|t| t.pos.distance(body.eye) >= LOB_BAND[0])
                        && !crowded(&self.beliefs, body.eye, landing, target.map(|t| t.who));
                    l.update(&hands, nav, clear)
                }
                Active::Detonate(d) => d.update(&hands),
                Active::Scope(sc) => {
                    let on = self
                        .mind
                        .last_aim
                        .is_some_and(|aim| on_target(self.motor.view, body.eye, aim));
                    sc.update(&hands, on)
                }
                Active::Shoot(s) => {
                    if self.explosives.mines.iter().any(|m| m.pos.distance(s.mine) < 24.0) {
                        s.update(&hands, self.mind.click_interval.max(spec(s.weapon).cycle))
                    } else {
                        Status::Done
                    }
                }
            };
            match status {
                Status::Running(r) => {
                    requests.push(r);
                    self.mind.arms.active = Some(active);
                }
                Status::Done => self.finished(&active, body),
                Status::Failed(why) => {
                    tracing::debug!("{} failed: {why}", active.name());
                    self.mind.arms.stats.failure(active.name(), why);
                    self.mind.arms.last_failure = Some(why);
                }
            }
        }
        if let Some((until, at, target)) = self.mind.arms.guide {
            if now < until {
                // The rocket follows the view: kept on the target it was fired at while that stays far enough (a
                // new target close by, or this one come close, would bring the rocket back to the bot).
                let point = self
                    .mind
                    .last_aim
                    .filter(|p| self.mind.target == Some(target) && p.distance(body.eye) >= ROCKET_MIN)
                    .unwrap_or(at);
                self.intents.look(
                    Prio::Protocol,
                    LookIntent::Point {
                        at: point,
                        engaged: true,
                    },
                );
            } else {
                self.mind.arms.guide = None;
            }
        }
        for r in requests {
            if let Some(w) = r.weapon {
                self.intents.weapon(Prio::Protocol, w);
            }
            if let Some(l) = r.look {
                self.intents.look(Prio::Protocol, l);
            }
            if let Some(m) = r.movement {
                self.intents.movement(Prio::Protocol, m);
            }
        }
    }

    /// How far along the view the first wall is with the gauss in hand, through players (the beam goes through them):
    /// looked at again every 30 ms at most, as far as a full charge's burst on a wall reaches.
    fn gauss_wall(&mut self, body: &Body, tracer: &mut dyn Tracer) -> f32 {
        const PERIOD: f64 = 0.03;
        if let Some((at, wall)) = self.mind.arms.gauss_wall
            && body.now.since(at) < PERIOD
        {
            return wall;
        }
        let reach = lb_combat::arms::gauss::wall_blast(body.damages.gauss_charged);
        let (forward, _, _) = view_angle_vectors(self.motor.view);
        let tr = tracer.trace(&TraceQuery::line(body.eye, body.eye + forward * reach));
        let wall = if tr.fraction >= 1.0 {
            f32::INFINITY
        } else {
            tr.fraction * reach
        };
        self.mind.arms.gauss_wall = Some((body.now, wall));
        wall
    }

    fn finished(&mut self, active: &Active, body: &Body) {
        let now = body.now;
        let stats = &mut self.mind.arms.stats;
        match active {
            Active::Throw(t) => match t.kind {
                Kind::Grenade => stats.grenades += 1,
                Kind::Satchel => {
                    stats.satchels += 1;
                    self.explosives.thrown_satchel(t.landing(body.gravity), now);
                }
                Kind::Snark => stats.snarks += 1,
            },
            Active::Mine(p) => {
                stats.mines += 1;
                self.explosives.placed_mine(p.mine(), p.normal, now);
            }
            Active::Lob(l) => {
                stats.lobs += 1;
                let landing = now + f64::from(l.throw.flight);
                self.mind.arms.hold_until = self.mind.arms.hold_until.max(landing);
            }
            Active::Detonate(d) => {
                stats.detonations += 1;
                self.explosives.detonated();
                if d.learned_swap {
                    self.mind.arms.satchel_swapped = !self.mind.arms.satchel_swapped;
                    tracing::info!("the satchel buttons are the other way round on this server");
                }
            }
            Active::Shoot(_) => stats.mine_shots += 1,
            Active::Scope(sc) => stats.scoped += u32::from(sc.fired),
        }
    }

    /// Every frame: shoot someone else's snark coming within 300 units, or the bot's own coming back at it within 150
    /// (a snark bites its owner too), when no player is in sight; with a player in sight 300 units away or more, a
    /// snark about to bite (within 150) is shot first. yapb shot at its own snarks and hornets whatever they did.
    pub(crate) fn snark_defense(&mut self, body: &Body) {
        let now = body.now;
        let player = self
            .mind
            .target
            .and_then(|k| self.beliefs.track(k))
            .filter(|t| t.state == TrackState::Visible)
            .map(|t| t.pos.distance(body.origin));
        if player.is_some_and(|d| d < SNARK_OVER_PLAYER) || self.mind.arms.busy() {
            return;
        }
        // With a player in sight only a snark about to bite is worth the turn.
        let reach = |r: f32| if player.is_some() { r.min(SNARK_BITING) } else { r };
        let snark = self
            .explosives
            .flying
            .iter()
            .filter(|f| f.kind == lb_game::entities::ProjectileKind::Snark && now.since(f.seen) <= 0.3)
            .filter(|f| {
                let d = f.pos.distance(body.origin);
                let coming = f.vel.dot(body.origin - f.pos) > 0.0;
                if f.own {
                    d <= reach(OWN_SNARK_NEAR) && coming
                } else {
                    d <= reach(SNARK_NEAR)
                }
            })
            .min_by(|a, b| a.pos.distance(body.origin).total_cmp(&b.pos.distance(body.origin)));
        let Some(s) = snark else { return };
        // Small, fast and close: a gun without a blast, the one in hand if it is one.
        let usable = |w: WeaponId| body.allows(w) && body.armed(w).is_some_and(|a| a.loaded());
        let w = body
            .weapon
            .filter(|w| SNARK_GUNS.contains(w) && usable(*w))
            .or_else(|| SNARK_GUNS.into_iter().find(|w| usable(*w)))
            .unwrap_or(WeaponId::Crowbar);
        // Over a player in sight, the snark must win the view and the trigger.
        let prio = if player.is_some() { Prio::Protocol } else { Prio::Threat };
        let (forward, _, _) = view_angle_vectors(self.motor.view);
        let on_it = forward.dot((s.pos - body.eye).normalize_or_zero()) > 0.98;
        self.intents.look(
            prio,
            LookIntent::Point {
                at: s.pos,
                engaged: true,
            },
        );
        let spec = spec(w);
        self.intents.weapon(
            prio,
            lb_motor::WeaponIntent {
                select: Some(w),
                fire: if on_it && body.weapon == Some(w) {
                    lb_motor::Fire::Primary
                } else {
                    lb_motor::Fire::None
                },
                trigger: spec.trigger,
                interval: self.mind.click_interval.max(spec.cycle),
                reload: false,
            },
        );
    }

    /// Every frame, after everything else asked to move: a move that would take the bot into the beam of an armed
    /// tripmine it knows of (its own included) is stopped. Paths keep off beams; this also covers strafing and
    /// dodging, which do not follow paths.
    pub(crate) fn beam_guard(&mut self, body: &Body) {
        let now = body.now;
        let Some((prio, mv)) = self.intents.movement else {
            return;
        };
        if prio >= Prio::Traversal || mv.speed <= 0.0 || mv.dir == Vec2::ZERO {
            return;
        }
        let ahead = body.origin + (mv.dir.normalize_or_zero() * mv.speed * BEAM_LOOKAHEAD).extend(0.0);
        let into = self.explosives.mines.iter().any(|m| {
            now >= m.armed_at
                && m.beam_end.is_some_and(|end| {
                    (1..=3).any(|k| near_beam(body.origin.lerp(ahead, k as f32 / 3.0), m.pos, end))
                        && !near_beam(body.origin, m.pos, end)
                })
        });
        if into {
            self.intents.movement(Prio::Protocol, lb_combat::arms::stop());
        }
    }

    /// Every frame: run from a blast about to go off near the bot.
    pub(crate) fn dodge(&mut self, body: &Body, nav: &mut dyn NavService) {
        let now = body.now;
        let floor = body.origin.z - 36.0;
        let threat = self
            .explosives
            .blasts(now, body.gravity, floor)
            .chain(self.explosives.rocket_at(body.eye))
            .filter(|b| b.at.distance(body.origin) < b.radius + DODGE_MARGIN)
            .min_by(|a, b| a.at.distance(body.origin).total_cmp(&b.at.distance(body.origin)));
        if let Some(b) = threat {
            let away = (body.origin - b.at).truncate();
            let dir = if away.length() > 16.0 {
                away.normalize()
            } else {
                let (_, right, _) = view_angle_vectors(self.motor.view);
                right.truncate().normalize_or(Vec2::Y)
            };
            let options = [dir, Vec2::new(-dir.y, dir.x), Vec2::new(dir.y, -dir.x)];
            if let Some(safe) = options
                .into_iter()
                .find(|d| !drops(nav, body.origin, *d * body.maxspeed))
            {
                if self.mind.arms.dodge.is_none_or(|(until, _)| now >= until) {
                    self.mind.arms.stats.dodges += 1;
                }
                self.mind.arms.dodge = Some((now + DODGE_FOR, safe));
            }
        }
        match self.mind.arms.dodge {
            Some((until, dir)) if now < until => self.intents.movement(
                Prio::Threat,
                MoveIntent {
                    dir,
                    speed: body.maxspeed,
                },
            ),
            Some(_) => self.mind.arms.dodge = None,
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lb_combat::Armed;
    use lb_config::skill::Presets;
    use lb_core::rng::Pcg32;
    use lb_knowledge::{BeliefParams, PlayerKey, Relation, RenderCue, Sighting, Stance, parts};
    use lb_nav_api::{NavStatus, NavStep};
    use lb_worldq::{Trace, contents};

    /// Open floor at z = -36 everywhere.
    struct Open;

    impl Tracer for Open {
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

    impl NavService for Open {
        fn go_to(&mut self, dest: Vec3) -> (NavStatus, Option<NavStep>) {
            (NavStatus::Moving, Some(NavStep::hold(dest)))
        }
        fn roam(&mut self, _rng: &mut Pcg32) -> Option<NavStep> {
            None
        }
        fn away_from(&mut self, _threat: Vec3) -> Option<Vec3> {
            None
        }
        fn available(&self) -> bool {
            true
        }
    }

    fn character() -> Character {
        Character {
            skill: Presets::default().at(100),
            level: 100,
            aggression: 0.8,
            fear: 0.2,
            affinity: lb_styles::StyleId::Balanced.goal_affinity(),
        }
    }

    fn body(now: f64) -> Body {
        Body {
            now: SimTime(now),
            dt: 0.01,
            origin: Vec3::ZERO,
            eye: Vec3::new(0.0, 0.0, 28.0),
            velocity: Vec3::ZERO,
            maxspeed: 300.0,
            health: 100.0,
            armor: 0.0,
            has_longjump: false,
            on_ground: true,
            on_ladder: false,
            underwater: false,
            waterlevel: 0,
            fov: 0.0,
            weapon: Some(WeaponId::Glock),
            arsenal: [
                Armed::new(WeaponId::Glock, Some(17), Some(50)),
                Armed::new(WeaponId::HandGrenade, None, Some(3)),
            ]
            .into_iter()
            .collect(),
            prediction: None,
            ammo_need: [0.0; 7],
            opponents: 2,
            damages: lb_game::mechanics::Damages::default(),
            dll: lb_game::dll::DllProfile::default(),
            gravity: 800.0,
            allowed: u32::MAX,
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

    fn considered(brain: &mut BotBrain, now: f64, extra: &[Armed]) -> Option<&'static str> {
        let mut rng = BotRng::new(7, 7);
        for i in 0..20 {
            let t = now + f64::from(i) * 0.1;
            brain.update(
                SimTime(t),
                &BeliefParams {
                    track_forget: 12.0,
                    maxspeed: 300.0,
                },
            );
            let mut b = body(t);
            b.arsenal.extend(extra.iter().copied());
            brain.weapon_options(&b, &character(), &mut Open, &mut rng);
            if let Some(a) = &brain.mind.arms.active {
                return Some(a.name());
            }
        }
        None
    }

    #[test]
    fn grenades_go_after_a_lost_enemy_and_snarks_at_one_in_sight() {
        let enemy = Vec3::new(500.0, 150.0, 0.0);
        let mut brain = BotBrain::new(1, lb_perception::PerceptionParams::from_skill(&character().skill));
        brain.beliefs.on_sighting(&seen(0.0, enemy));
        assert_eq!(
            considered(&mut brain, 1.0, &[]),
            Some("grenade"),
            "lost a second ago, 500 units away"
        );
        let mut brain = BotBrain::new(1, lb_perception::PerceptionParams::from_skill(&character().skill));
        let mut rng = BotRng::new(7, 7);
        let snarks = [Armed::new(WeaponId::Snark, None, Some(5))];
        let mut thrown = None;
        for i in 0..40 {
            let t = f64::from(i) * 0.1;
            brain.beliefs.on_sighting(&seen(t, enemy));
            brain.update(
                SimTime(t),
                &BeliefParams {
                    track_forget: 12.0,
                    maxspeed: 300.0,
                },
            );
            let mut b = body(t);
            b.arsenal.extend(snarks);
            brain.weapon_options(&b, &character(), &mut Open, &mut rng);
            if let Some(a) = &brain.mind.arms.active {
                thrown = Some(a.name());
                break;
            }
        }
        assert_eq!(thrown, Some("snark"), "an enemy in sight is sent snarks first");
    }
}
