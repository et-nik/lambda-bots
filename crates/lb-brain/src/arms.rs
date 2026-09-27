//! Weapons beyond the trigger: which protocol runs, when a throw, a mine, a launched grenade or a detonation is worth
//! it (ten times a second, from what the bot believes), and dodging what it sees about to blow up.
//!
//! - **Throws** (yapb's `checkGrenadesThrow`, made honest): only at an enemy out of sight whose position is still
//!   tight (lost moments ago), never at one in view. A grenade 300–800 units away when a throw lands there and the
//!   blast spares the bot, a satchel at 150–400 with a clear line to the spot, a snark at 150–800 likewise. A throw
//!   is considered half the time, more often with skill (always for experts), and some are called off anyway: 3% for
//!   bold bots and 10% for careful ones, 10% for satchels, 25% for snarks.
//! - **The MP5's grenade** at a target in sight 300–700 units away, every 2.5–4 s, when the lob is clear.
//! - **Satchels** go off when an enemy is within 160 units of one and closer to it than the bot; a bot too close to
//!   be spared backs off first.
//! - **Tripmines** are shot when an enemy is within 140 units of one 400–1200 units away; a known enemy mine ahead is
//!   shot to clear the way when nothing else goes on. When quiet the bot now and then lays a mine across a corridor it
//!   walks along, never near a spawn point and not next to another mine.
//! - **Dodge:** a grenade coming down, an MP5 grenade landing or a rocket passing near the bot makes it run away from
//!   the blast (yapb ran toward it), checking for ledges.
//! - **Gauss:** its charge runs whenever the gauss is in hand; nothing else starts while it charges.

use lb_combat::arms::detonate::{MineShot, SatchelTrigger};
use lb_combat::arms::gauss::{Gauss, GaussInput};
use lb_combat::arms::launcher::Lob;
use lb_combat::arms::mine::Planter;
use lb_combat::arms::throw::{Kind, Thrower};
use lb_combat::arms::{Hands, Request, Status};
use lb_combat::ballistics;
use lb_combat::fight::drops;
use lb_core::math::view_angle_vectors;
use lb_core::rng::BotRng;
use lb_core::time::SimTime;
use lb_core::{Vec2, Vec3};
use lb_decision::GoalKind;
use lb_game::mechanics::{blast_radius, spec};
use lb_game::weapons::WeaponId;
use lb_knowledge::{EnemyTrack, TrackState};
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
/// Only enemies lost this recently with a position this tight are thrown at.
const THROW_TRACK_AGE: f64 = 4.0;
const THROW_TRACK_SIGMA: f32 = 200.0;
/// An enemy this far above is out of throwing reach.
const TOO_HIGH: f32 = 500.0;
const SNARK_TOO_HIGH: f32 = 200.0;
/// Victim within this of a satchel, a satchel within this of the bot spares it too little.
const SATCHEL_VICTIM: f32 = 160.0;
const SATCHEL_SPARED: f32 = 250.0;
const MINE_VICTIM: f32 = 140.0;
const MINE_SHOT_BAND: [f32; 2] = [400.0, 1200.0];
/// A corridor this wide at most gets a mine; its nearer wall must be this close.
const CORRIDOR: f32 = 300.0;
const WALL_NEAR: f32 = 90.0;
const MINE_SPACING: f32 = 96.0;
const SPAWN_CLEAR: f32 = 256.0;
/// Quiet this long before laying a mine or clearing one.
const QUIET: f64 = 5.0;
/// A dodge lasts this long once started.
const DODGE_FOR: f64 = 0.5;
const DODGE_MARGIN: f32 = 40.0;

#[derive(Clone, Debug)]
pub enum Active {
    Throw(Thrower),
    Mine(Planter),
    Lob(Lob),
    Detonate(SatchelTrigger),
    Shoot(MineShot),
}

impl Active {
    pub fn name(&self) -> &'static str {
        match self {
            Active::Throw(t) => t.kind.as_str(),
            Active::Mine(_) => "tripmine",
            Active::Lob(_) => "m203",
            Active::Detonate(_) => "detonate",
            Active::Shoot(_) => "shoot a mine",
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
    pub failed: u32,
}

#[derive(Clone, Debug, Default)]
pub struct Arms {
    pub gauss: Gauss,
    pub active: Option<Active>,
    pub stats: ArmsStats,
    pub last_failure: Option<&'static str>,
    /// This server's satchel buttons turned out the other way round than its DLL profile says.
    pub satchel_swapped: bool,
    /// A fired rocket is guided until then; the point it was fired at.
    pub guide: Option<(SimTime, Vec3)>,
    /// Both shotgun barrels on the next shot.
    pub double: bool,
    /// When the next zoom toggle may be pressed.
    pub zoom_ready: SimTime,
    next_throw: SimTime,
    next_lob: SimTime,
    next_mine: SimTime,
    safe_behind: bool,
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
        if self.guide.is_some_and(|(until, _)| now < until) {
            parts.push("guiding a rocket".into());
        }
        if self.dodge.is_some_and(|(until, _)| now < until) {
            parts.push("dodging".into());
        }
        if parts.is_empty() { "-".into() } else { parts.join(", ") }
    }
}

/// An enemy in sight, or lost half a second ago with a tight position: sure enough to set off a trap on.
fn fresh(t: &EnemyTrack, now: SimTime) -> bool {
    t.state == TrackState::Visible || (t.age(now) <= 0.5 && t.sigma < 60.0)
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
        let (forward, _, _) = view_angle_vectors(self.motor.view);
        let back = -forward.truncate().normalize_or(Vec2::X);
        self.mind.arms.safe_behind = !drops(nav, body.origin, back * 800.0);
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
        if nearest.distance(body.origin) < SATCHEL_SPARED {
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
        if feet.distance(body.origin) < SELF_CLEAR {
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
        self.mind.arms.next_throw = now + 0.3;
        let Some(t) = self
            .beliefs
            .enemies()
            .filter(|t| matches!(t.state, TrackState::RecentlyLost | TrackState::Predicted))
            .filter(|t| t.age(now) <= THROW_TRACK_AGE && t.sigma <= THROW_TRACK_SIGMA)
            .min_by(|a, b| a.pos.distance(body.origin).total_cmp(&b.pos.distance(body.origin)))
        else {
            return false;
        };
        let above = t.pos.z - body.origin.z;
        if above > TOO_HIGH || (!t.traits.on_ground && above > 72.0) {
            return false;
        }
        let diff = (f32::from(ch.level) / 25.0).clamp(0.0, 4.0);
        if rng.combat.next_f32() >= (25.0 * diff).max(50.0) / 100.0 {
            return false;
        }
        let d = (t.pos - body.origin).truncate().length();
        let floor = t.pos - Vec3::Z * 32.0;
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
        let grenades = count(WeaponId::HandGrenade) > 0 && (GRENADE_BAND[0]..=GRENADE_BAND[1]).contains(&d);
        let satchels = count(WeaponId::Satchel) > 0
            && self.explosives.charges.is_empty()
            && (SATCHEL_BAND[0]..=SATCHEL_BAND[1]).contains(&d);
        let snarks = count(WeaponId::Snark) > 0
            && body.waterlevel < 2
            && above <= SNARK_TOO_HIGH
            && (SNARK_BAND[0]..=SNARK_BAND[1]).contains(&d);
        let kind = if satchels && (!grenades || d < 300.0 || (d < 400.0 && rng.combat.next_f32() < 0.5)) {
            Kind::Satchel
        } else if grenades {
            Kind::Grenade
        } else if snarks {
            Kind::Snark
        } else {
            return false;
        };
        let cancel = match kind {
            Kind::Grenade if ch.aggression > ch.fear => 0.03,
            Kind::Grenade | Kind::Satchel => 0.10,
            Kind::Snark => 0.25,
        };
        if rng.combat.next_f32() < cancel {
            self.mind.arms.next_throw = now + 1.0;
            return false;
        }
        let throw = match kind {
            Kind::Grenade => {
                let solved = ballistics::grenade(nav, body.eye, body.velocity, floor, body.gravity, body.dll, 2.4);
                match solved {
                    Some(s) if floor.distance(body.origin) >= SELF_CLEAR => s,
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
        self.mind.arms.active = Some(Active::Throw(Thrower::new(kind, target, throw, now)));
        self.mind.arms.next_throw = now + f64::from(rng.combat.range_f32(2.0, 4.0));
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
        let hands = self.hands(body);
        let mut requests: smallvec::SmallVec<[Request; 2]> = smallvec::SmallVec::new();
        if body.weapon == Some(WeaponId::Gauss) || self.mind.arms.gauss.active() {
            let target = self
                .mind
                .target
                .and_then(|k| self.beliefs.track(k))
                .filter(|t| t.state == TrackState::Visible)
                .map(|t| {
                    let (forward, _, _) = view_angle_vectors(self.motor.view);
                    let aim = self.mind.last_aim.unwrap_or(t.pos);
                    (
                        t.pos.distance(body.eye),
                        forward.dot((aim - body.eye).normalize_or_zero()) > 0.9995,
                    )
                });
            let expected = matches!(self.mind.goal.map(|g| g.kind), Some(GoalKind::Hunt(_)));
            let input = GaussInput {
                target,
                expected,
                precharge: ch.skill.gauss_precharge,
                safe_behind: self.mind.arms.safe_behind,
                heading: body.velocity.truncate().normalize_or_zero(),
            };
            if let Some(r) = self.mind.arms.gauss.update(&hands, &input, &mut rng.combat) {
                requests.push(r);
            }
        }
        if let Some(mut active) = self.mind.arms.active.take() {
            let status = match &mut active {
                Active::Throw(t) => t.update(&hands, nav),
                Active::Mine(p) => p.update(&hands, nav),
                Active::Lob(l) => l.update(&hands),
                Active::Detonate(d) => d.update(&hands),
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
                    self.mind.arms.stats.failed += 1;
                    self.mind.arms.last_failure = Some(why);
                }
            }
        }
        if let Some((until, at)) = self.mind.arms.guide {
            if now < until {
                let point = self.mind.last_aim.unwrap_or(at);
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
            Active::Lob(_) => stats.lobs += 1,
            Active::Detonate(d) => {
                stats.detonations += 1;
                self.explosives.detonated();
                if d.learned_swap {
                    self.mind.arms.satchel_swapped = !self.mind.arms.satchel_swapped;
                    tracing::info!("the satchel buttons are the other way round on this server");
                }
            }
            Active::Shoot(_) => stats.mine_shots += 1,
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
