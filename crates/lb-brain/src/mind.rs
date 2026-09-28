//! The bot's mind: every frame it turns beliefs into motor intents.
//!
//! - **Combat tick (10 Hz):** picks the enemy to fight and the weapon for its distance.
//! - **Decision (5 Hz, or at once on a new enemy or damage):** picks the goal.
//! - **Goal:** moves the bot. Engage fights in place (strafing, standing, backing off); the others walk a path.
//! - **Aim and fire (priority 70):** owns the look and the weapon whenever an enemy is in sight, whatever the
//!   goal, so a retreating or hunting bot still shoots back.
//! - **Vigilance (priority 20):** looks along the path and glances at sounds.

use lb_combat::aim::{Aim, AimSkill, Shot};
use lb_combat::fight::{Fight, FightInput, FightSkill};
use lb_combat::policy::{self, Armed, Choice, Target, XBOW_ZOOM_FROM};
use lb_combat::{fire, target};
use lb_config::skill::SkillParams;
use lb_core::Vec3;
use lb_core::rng::BotRng;
use lb_core::time::SimTime;
use lb_decision::{Decider, Goal, GoalKind, Situation};
use lb_game::dll::DllProfile;
use lb_game::items::Ammo;
use lb_game::mechanics::{
    AltFire, Attack, BOLT_SPEED, DART_SPEED, Damages, ROCKET_GUIDE, ROCKET_SPEED, WeaponClass, spec,
};
use lb_game::self_state::Prediction;
use lb_game::weapons::WeaponId;
use lb_knowledge::{EnemyTrack, PlayerKey, TrackState};
use lb_motor::{Fire, Intents, LookIntent, LookParams, MoveIntent, Prio, StanceIntent, WeaponIntent};
use lb_nav_api::{NavService, NavStatus, NavStep};
use lb_styles::GoalAffinity;
use smallvec::SmallVec;

use crate::BotBrain;
use crate::arms::Arms;
use crate::attention::LookReason;

const COMBAT_PERIOD: f64 = 0.1;
const DECISION_PERIOD: f64 = 0.2;
/// Keep looking where a lost target went for this long.
const LOST_STARE: f64 = 1.0;
/// Reload only after this long without an enemy in sight.
const CALM_BEFORE_RELOAD: f64 = 2.0;
/// A scope comes off after this long without a target.
const UNZOOM_AFTER: f64 = 1.5;
/// Distance weapons are chosen for when no enemy is about.
const CALM_DISTANCE: f32 = 600.0;
/// A rocket flies about this fast on average over its way.
const ROCKET_AVERAGE: f32 = 1500.0;
/// The crossbow's scope goes on only with the target this close to the view's center (the zoomed view is 20° wide).
const SCOPE_START_DOT: f32 = 0.9945;
/// A bolt's and the egon beam's end blast reach this far: the line of fire must be clear beyond it.
const BOLT_CLEAR: f32 = 160.0;
const EGON_CLEAR: f32 = 128.0;
/// Seconds a look along an explosive's line holds.
const BLAST_CHECK_PERIOD: f64 = 0.03;
/// At a charger's spot within this, across.
const CHARGER_SPOT: f32 = 24.0;
/// A charger that gave nothing for this long is spent; nobody stays at one longer than the timeout.
const CHARGER_DRY: f64 = 1.5;
const CHARGER_TIMEOUT: f64 = 15.0;
const RETREAT_REPLAN: f64 = 2.0;
/// A melee fighter walks a path to enemies further than this, and charges straight at closer ones.
const MELEE_CHARGE: f32 = 200.0;
const REACTION_SAMPLES: usize = 128;

/// How fast the bot answers an enemy: from the first glimpse and from recognition to the first shot at it.
#[derive(Clone, Debug, Default)]
pub struct Reactions {
    pub count: u64,
    pub evidence_to_shot: f64,
    pub recognition_to_shot: f64,
    pub worst: f64,
    /// Latest evidence-to-shot times, for percentiles.
    pub recent: std::collections::VecDeque<f64>,
}

impl Reactions {
    fn record(&mut self, from_evidence: f64, from_recognition: f64) {
        self.count += 1;
        self.evidence_to_shot += from_evidence;
        self.recognition_to_shot += from_recognition;
        self.worst = self.worst.max(from_evidence);
        if self.recent.len() == REACTION_SAMPLES {
            self.recent.pop_front();
        }
        self.recent.push_back(from_evidence);
    }

    /// Median of the latest evidence-to-shot times.
    pub fn median(&self) -> Option<f64> {
        let mut v: Vec<f64> = self.recent.iter().copied().collect();
        v.sort_by(f64::total_cmp);
        v.get(v.len() / 2).copied()
    }
}

/// What the bot knows about itself this frame.
#[derive(Clone, Debug)]
pub struct Body {
    pub now: SimTime,
    pub dt: f32,
    pub origin: Vec3,
    pub eye: Vec3,
    pub velocity: Vec3,
    pub maxspeed: f32,
    pub health: f32,
    pub armor: f32,
    pub has_longjump: bool,
    pub on_ground: bool,
    pub on_ladder: bool,
    pub underwater: bool,
    /// 0 dry, 1 feet, 2 waist, 3 head under water.
    pub waterlevel: u8,
    /// Field of view the game set (`pev->fov`): 0 is the default, less is a zoomed scope.
    pub fov: f32,
    /// Weapon confirmed by `CurWeapon`.
    pub weapon: Option<WeaponId>,
    pub arsenal: SmallVec<[Armed; 16]>,
    /// What the bot's own client would be told for weapon prediction.
    pub prediction: Option<Prediction>,
    /// How much each ammo type is needed, 0..1, by [`Ammo::index`].
    pub ammo_need: [f32; 7],
    /// Other players on the server.
    pub opponents: usize,
    /// Weapon damage the server deals.
    pub damages: Damages,
    /// How the server's game DLL works the weapons that differ.
    pub dll: DllProfile,
    /// `sv_gravity`.
    pub gravity: f32,
    /// Weapons the bot may use, as a mask of weapon bits (`lb weapons` on the stand); all by default.
    pub allowed: u32,
}

impl Body {
    pub fn allows(&self, w: WeaponId) -> bool {
        self.allowed & w.bit() != 0
    }

    /// The scope is on (a crossbow or a 357 zoomed in).
    pub fn zoomed(&self) -> bool {
        self.fov > 0.0 && self.fov < 89.0
    }

    pub(crate) fn armed(&self, w: WeaponId) -> Option<&Armed> {
        self.arsenal.iter().find(|a| a.id == w)
    }
}

/// Who the bot is.
#[derive(Clone, Debug)]
pub struct Character {
    pub skill: SkillParams,
    /// 0..100.
    pub level: u8,
    pub aggression: f32,
    pub fear: f32,
    pub affinity: GoalAffinity,
}

impl Character {
    fn aim(&self) -> AimSkill {
        AimSkill {
            headshot: self.skill.headshot,
            latency: self.skill.aim_latency,
            error: self.skill.aim_error,
            skill: self.level,
        }
    }

    /// Typical aim error at `distance`, units.
    fn aim_sigma(&self, distance: f32) -> f32 {
        let e = self.skill.aim_error;
        let level = (f32::from(self.level) / 25.0).clamp(1.0, 4.0);
        (e[0] + e[1]) * 0.5 * (1.0 + distance / (1280.0 * level))
    }
}

#[derive(Clone, Debug, Default)]
pub struct Mind {
    pub decider: Decider,
    pub goal: Option<Goal>,
    pub target: Option<PlayerKey>,
    pub aim: Aim,
    pub fight: Fight,
    pub choice: Option<Choice>,
    pub(crate) click_interval: f32,
    next_combat: SimTime,
    next_decision: SimTime,
    urgent: bool,
    seen_damage: Option<SimTime>,
    retreat_to: Option<(Vec3, SimTime)>,
    reload_until: SimTime,
    last_enemy_seen: SimTime,
    /// Where the path wants the bot to look, for vigilance.
    path_look: Option<Vec3>,
    /// Navigation wants this point shot at (an obstacle to break), with the crowbar when set.
    nav_fire: Option<(Vec3, bool)>,
    /// The contact already shot at: player and when it was recognized.
    answered: Option<(PlayerKey, SimTime)>,
    pub reactions: Reactions,
    /// The trigger was pulled on the last frame.
    pub firing: bool,
    /// Throws, mines, detonations, the gauss charge and dodging.
    pub arms: Arms,
    /// Where the aim was on the last frame an enemy was in sight.
    pub last_aim: Option<Vec3>,
    /// When the bot last had a target to shoot at.
    target_at: SimTime,
    /// The last look along the line an explosive would take (a wall or someone close in front): when, for which
    /// weapon, and whether it was clear.
    blast_check: Option<(SimTime, WeaponId, bool)>,
    /// Using a charger: which, since when, when it last gave something and what the bot had then.
    charging: Option<(usize, SimTime, SimTime, f32)>,
}

impl Mind {
    pub fn reset(&mut self) {
        let decider = std::mem::take(&mut self.decider);
        let reactions = std::mem::take(&mut self.reactions);
        let mut arms = std::mem::take(&mut self.arms);
        arms.reset();
        *self = Mind::default();
        self.decider = decider;
        self.decider.reset();
        self.reactions = reactions;
        self.arms = arms;
    }

    pub fn reloading(&self, now: SimTime) -> bool {
        now < self.reload_until
    }

    pub fn last_enemy_seen(&self) -> SimTime {
        self.last_enemy_seen
    }
}

fn apply_step(intents: &mut Intents, step: &NavStep, eye: Vec3, m: &mut Mind) {
    intents.movement(
        Prio::Goal,
        MoveIntent {
            dir: step.move_dir,
            speed: step.speed,
        },
    );
    let stance = StanceIntent {
        jump: step.jump,
        duck: step.duck,
    };
    if step.mandatory {
        intents.stance(Prio::Traversal, stance);
        let look = match step.pitch {
            Some(pitch) => {
                let mut angles = lb_core::math::dir_to_view_angles(step.look_at - eye);
                angles.x = pitch;
                LookIntent::Angles(angles)
            }
            None => LookIntent::Point {
                at: step.look_at,
                engaged: false,
            },
        };
        intents.look(Prio::Traversal, look);
    } else {
        intents.stance(Prio::Goal, stance);
    }
    if step.use_key {
        intents.use_key(Prio::Traversal);
    }
    m.nav_fire = step.fire_at.map(|at| (at, step.melee));
    m.path_look = Some(step.look_at);
}

impl BotBrain {
    /// One frame of behavior: the motor's command for this frame.
    pub fn act(
        &mut self,
        body: &Body,
        ch: &Character,
        nav: &mut dyn NavService,
        rng: &mut BotRng,
    ) -> lb_motor::MotorOut {
        let now = body.now;
        self.intents.clear();
        self.explosives.update(now);
        self.combat_tick(body, ch, nav, rng);
        self.decide(body, ch, rng);
        self.pursue(body, ch, nav, rng);
        self.dodge(body, nav);
        self.run_protocols(body, ch, nav, rng);
        self.aim_and_fire(body, ch, nav, rng);
        self.snark_defense(body);
        self.vigilance(body);
        self.beam_guard(body);
        let input = lb_motor::MotorInput {
            now,
            dt: body.dt,
            eye: body.eye,
            velocity: body.velocity,
            maxspeed: body.maxspeed,
            on_ladder: body.on_ladder,
            weapon: body.weapon,
        };
        let look = LookParams {
            model: ch.skill.aim_model,
            turn_speed: ch.skill.turn_speed,
            skill: ch.level,
        };
        self.motor.run(&self.intents, &input, &look, &mut rng.motor)
    }

    fn combat_tick(&mut self, body: &Body, ch: &Character, nav: &mut dyn NavService, rng: &mut BotRng) {
        let now = body.now;
        let m = &mut self.mind;
        if let Some(d) = self.beliefs.last_damage
            && m.seen_damage != Some(d.t)
        {
            m.seen_damage = Some(d.t);
            m.urgent = true;
        }
        if now < m.next_combat {
            return;
        }
        m.next_combat = now + COMBAT_PERIOD;
        let previous = m.target;
        let seen = target::select(self.beliefs.enemies(), body.origin, now, previous);
        if seen.is_some() {
            m.last_enemy_seen = now;
        }
        m.target = match seen {
            Some(t) => Some(t),
            // Keep the one just lost in mind for a moment.
            None => previous.filter(|k| {
                self.beliefs
                    .track(*k)
                    .is_some_and(|t| now.since(t.last_seen) <= LOST_STARE)
            }),
        };
        if seen.is_some() && seen != previous {
            m.urgent = true;
        }
        let track = m.target.and_then(|k| self.beliefs.track(k));
        let distance = track.map_or(CALM_DISTANCE, |t| t.pos.distance(body.eye));
        let speed = track
            .filter(|t| t.velocity_known(now))
            .map_or(250.0, |t| t.vel.truncate().length());
        let t = Target {
            distance,
            speed,
            aim_sigma: ch.aim_sigma(distance),
        };
        let allowed = |w: WeaponId| body.allows(w);
        let choice = policy::choose(&body.arsenal, body.weapon, &t, body.underwater, &body.damages, &allowed);
        if m.choice != Some(choice) {
            if let Choice::Use(w) = choice {
                m.click_interval = fire::click_interval(w, ch.skill.semi_auto_delay, &mut rng.combat);
            }
            m.choice = Some(choice);
        }
        self.weapon_options(body, ch, nav, rng);
    }

    fn decide(&mut self, body: &Body, ch: &Character, rng: &mut BotRng) {
        let now = body.now;
        let m = &mut self.mind;
        if !m.urgent && now < m.next_decision && m.decider.current.is_some() {
            return;
        }
        m.urgent = false;
        m.next_decision = now + DECISION_PERIOD;
        let need = |a: Ammo| body.ammo_need[a.index()];
        let s = Situation {
            now,
            origin: body.origin,
            health: body.health,
            armor: body.armor,
            has_longjump: body.has_longjump,
            aggression: ch.aggression,
            fear: ch.fear,
            affinity: ch.affinity,
            beliefs: &self.beliefs,
            items: self.items.as_ref(),
            chargers: self.chargers.as_ref(),
            weapons: &body.arsenal,
            ammo_need: &need,
            reloading: now < m.reload_until,
            target: m.target,
            opponents: body.opponents,
            maxspeed: body.maxspeed,
        };
        m.goal = Some(m.decider.decide(&s, &mut rng.decision));
    }

    fn pursue(&mut self, body: &Body, ch: &Character, nav: &mut dyn NavService, rng: &mut BotRng) {
        let now = body.now;
        self.mind.nav_fire = None;
        let Some(goal) = self.mind.goal else { return };
        let m = &mut self.mind;
        let mut arrive = |status: NavStatus, m: &mut Mind| match status {
            NavStatus::Arrived => {
                m.decider.complete();
                m.urgent = true;
            }
            NavStatus::NoPath => {
                m.decider.fail(now, &mut rng.decision);
                m.urgent = true;
            }
            NavStatus::Moving => {}
        };
        match goal.kind {
            GoalKind::Engage(k) => {
                let Some(t) = self.beliefs.track(k) else {
                    m.decider.complete();
                    return;
                };
                let class = body.weapon.map_or(WeaponClass::Melee, |w| spec(w).class);
                let distance = t.pos.distance(body.origin);
                if class == WeaponClass::Melee && distance > MELEE_CHARGE {
                    let (status, step) = nav.go_to(t.pos);
                    if let Some(step) = step {
                        apply_step(&mut self.intents, &step, body.eye, m);
                    }
                    arrive(status, m);
                    return;
                }
                let input = FightInput {
                    now,
                    origin: body.origin,
                    enemy: t.pos,
                    enemy_facing: t.traits.facing,
                    enemy_faces_me: target::faces(t, body.origin),
                    approach: body.health.clamp(0.0, 100.0) * ch.aggression,
                    weapon: class,
                    reloading: m.reloading(now),
                    hold_ground: now < m.arms.hold_until,
                    on_ground: body.on_ground,
                    maxspeed: body.maxspeed,
                };
                let skill = FightSkill {
                    skill: ch.level,
                    stay_mid: ch.skill.stay_mid,
                    stay_far: ch.skill.stay_far,
                    crouch_tap: ch.skill.crouch_tap,
                    dodge_hop_cooldown: ch.skill.dodge_hop_cooldown,
                };
                let mv = m.fight.update(&input, &skill, nav, &mut rng.combat);
                self.intents.movement(
                    Prio::Goal,
                    MoveIntent {
                        dir: mv.velocity,
                        speed: mv.velocity.length(),
                    },
                );
                self.intents.stance(
                    Prio::Goal,
                    StanceIntent {
                        jump: mv.jump,
                        duck: mv.duck,
                    },
                );
                m.path_look = None;
            }
            GoalKind::Hunt(k) => {
                let Some(t) = self.beliefs.track(k) else {
                    m.decider.complete();
                    return;
                };
                let (status, step) = nav.go_to(t.pos);
                if let Some(step) = step {
                    apply_step(&mut self.intents, &step, body.eye, m);
                }
                arrive(status, m);
            }
            GoalKind::Retreat => {
                let threat = self
                    .beliefs
                    .enemies()
                    .filter(|t| t.state != TrackState::Stale)
                    .min_by(|a, b| a.pos.distance(body.origin).total_cmp(&b.pos.distance(body.origin)))
                    .map(|t| t.pos);
                let stale = m.retreat_to.is_none_or(|(_, at)| now.since(at) > RETREAT_REPLAN);
                if stale && let Some(threat) = threat {
                    m.retreat_to = nav.away_from(threat).map(|p| (p, now));
                }
                match m.retreat_to {
                    Some((dest, _)) => {
                        let (status, step) = nav.go_to(dest);
                        if let Some(step) = step {
                            apply_step(&mut self.intents, &step, body.eye, m);
                        }
                        if status != NavStatus::Moving {
                            m.retreat_to = None;
                        }
                        arrive(status, m);
                    }
                    None => m.decider.fail(now, &mut rng.decision),
                }
            }
            GoalKind::CollectItem(i) => {
                let Some(spot) = self.items.as_ref().and_then(|items| items.spots.get(i)).copied() else {
                    m.decider.complete();
                    return;
                };
                let gone = self
                    .items
                    .as_ref()
                    .is_some_and(|items| items.availability(i, now, 0.0, body.opponents) <= 0.0);
                if gone {
                    m.decider.complete();
                    m.urgent = true;
                    return;
                }
                let (status, step) = nav.go_to(spot.origin);
                if let Some(step) = step {
                    apply_step(&mut self.intents, &step, body.eye, m);
                }
                arrive(status, m);
            }
            GoalKind::UseCharger(i) => self.use_charger(i, body, nav, rng),
            GoalKind::Roam => {
                if let Some(step) = nav.roam(&mut rng.decision) {
                    apply_step(&mut self.intents, &step, body.eye, m);
                }
            }
        }
        if !matches!(goal.kind, GoalKind::UseCharger(_)) {
            self.mind.charging = None;
        }
    }

    /// Walks to the charger's spot, then faces it holding the use key while it gives; spent when it stops giving.
    fn use_charger(&mut self, i: usize, body: &Body, nav: &mut dyn NavService, rng: &mut BotRng) {
        let now = body.now;
        let Some(c) = self.chargers.as_ref().and_then(|c| c.spots.get(i)).copied() else {
            self.mind.decider.complete();
            return;
        };
        if !self.chargers.as_ref().is_some_and(|ch| ch.available(i, now)) {
            self.mind.decider.complete();
            self.mind.urgent = true;
            return;
        }
        let there =
            (body.origin - c.spot).truncate().length() < CHARGER_SPOT && (body.origin.z - c.spot.z).abs() < 40.0;
        let m = &mut self.mind;
        if !there && m.charging.is_none() {
            let (status, step) = nav.go_to(c.spot);
            if let Some(step) = step {
                apply_step(&mut self.intents, &step, body.eye, m);
            }
            if status == NavStatus::NoPath {
                m.decider.fail(now, &mut rng.decision);
                m.urgent = true;
            }
            return;
        }
        let value = if c.suit { body.armor } else { body.health };
        let (_, since, last_gain, last_value) = *m.charging.get_or_insert((i, now, now, value));
        let last_gain = if value > last_value { now } else { last_gain };
        m.charging = Some((i, since, last_gain, value));
        let full = value >= 99.0;
        if full || now.since(since) > CHARGER_TIMEOUT {
            m.decider.complete();
            m.urgent = true;
            m.charging = None;
            return;
        }
        if now.since(last_gain) > CHARGER_DRY {
            if let Some(ch) = self.chargers.as_mut() {
                ch.drained(i, now);
            }
            m.decider.complete();
            m.urgent = true;
            m.charging = None;
            return;
        }
        self.intents.movement(
            Prio::Goal,
            MoveIntent {
                dir: lb_core::Vec2::ZERO,
                speed: 0.0,
            },
        );
        self.intents.look(
            Prio::Goal,
            LookIntent::Point {
                at: c.center,
                engaged: false,
            },
        );
        self.intents.use_hold(Prio::Goal);
        m.path_look = None;
    }

    fn aim_and_fire(&mut self, body: &Body, ch: &Character, nav: &mut dyn NavService, rng: &mut BotRng) {
        let now = body.now;
        let m = &mut self.mind;
        let track = m.target.and_then(|k| self.beliefs.track(k));
        let aim_skill = ch.aim();
        if let Some(t) = track {
            m.aim.follow(t, &aim_skill, &mut rng.combat);
        } else {
            m.aim.clear();
        }
        let weapon_choice = m.choice.map(Choice::weapon);
        m.firing = false;
        match (track, m.choice) {
            (Some(t), Some(choice)) if t.state == TrackState::Visible => {
                m.target_at = now;
                let distance = t.pos.distance(body.eye);
                let w = choice.weapon();
                let armed = body.armed(w).copied().unwrap_or_else(|| Armed::new(w, None, None));
                let mode = fire::mode(
                    &armed,
                    distance,
                    m.arms.double,
                    ch.aim_sigma(distance),
                    m.click_interval,
                );
                let shot = shot_of(w, mode.attack, body.zoomed());
                let Some(aim) = m.aim.point(now, body.eye, &shot, &aim_skill, &mut rng.combat) else {
                    return;
                };
                m.last_aim = Some(aim);
                self.intents
                    .look(Prio::Threat, LookIntent::Point { at: aim, engaged: true });
                let intent = match choice {
                    Choice::Use(w) => {
                        let in_hand = body.weapon == Some(w) && !m.reloading(now);
                        if in_hand && w == WeaponId::Crossbow && distance >= XBOW_ZOOM_FROM {
                            start_scope(m, self.motor.view, aim, body, ch, rng);
                        }
                        match zoom_toggle(w, Some(distance), body, &mut m.arms.zoom_ready).filter(|_| in_hand) {
                            Some(toggle) => toggle,
                            None => {
                                let shot = Aimed {
                                    view: self.motor.view,
                                    target: t,
                                    weapon: w,
                                    mode,
                                    aim,
                                    distance,
                                    in_hand,
                                };
                                shoot(m, &shot, &self.beliefs, nav, body, rng)
                            }
                        }
                    }
                    Choice::Reload(w) => {
                        if body.weapon == Some(w) && !m.reloading(now) {
                            m.reload_until = now + f64::from(spec(w).reload);
                        }
                        WeaponIntent {
                            reload: true,
                            ..WeaponIntent::hold(w)
                        }
                    }
                };
                self.intents.weapon(Prio::Threat, intent);
            }
            (Some(t), _) if now.since(t.last_seen) <= LOST_STARE => {
                self.intents.look(
                    Prio::Threat,
                    LookIntent::Point {
                        at: t.pos + Vec3::Z * 8.0,
                        engaged: false,
                    },
                );
                if let Some(w) = weapon_choice {
                    self.intents.weapon(Prio::Threat, WeaponIntent::hold(w));
                }
            }
            _ if m.nav_fire.is_some() => {
                // Breaking an obstacle in the way: the crowbar when asked, else the weapon of choice.
                let Some((at, melee)) = m.nav_fire else { unreachable!() };
                let weapon = if melee || weapon_choice.is_none() {
                    WeaponId::Crowbar
                } else {
                    weapon_choice.unwrap_or(WeaponId::Crowbar)
                };
                let (forward, _, _) = lb_core::math::view_angle_vectors(self.motor.view);
                let on_it = forward.dot((at - body.eye).normalize_or_zero()) > 0.995;
                let ready = body.weapon == Some(weapon);
                self.intents.weapon(
                    Prio::Goal,
                    WeaponIntent {
                        select: Some(weapon),
                        fire: if on_it && ready { Fire::Primary } else { Fire::None },
                        trigger: spec(weapon).trigger,
                        interval: m.click_interval.max(0.15),
                        reload: false,
                    },
                );
            }
            _ => {
                // Calm: take the scope off, reload the gun worth having loaded, otherwise hold the best weapon.
                let calm = now.since(m.last_enemy_seen) >= CALM_BEFORE_RELOAD;
                if now.since(m.target_at) >= UNZOOM_AFTER
                    && let Some(w) = body.weapon
                    && let Some(toggle) = zoom_toggle(w, None, body, &mut m.arms.zoom_ready)
                {
                    self.intents.weapon(Prio::Goal, toggle);
                    return;
                }
                let t = Target {
                    distance: CALM_DISTANCE,
                    speed: 250.0,
                    aim_sigma: ch.aim_sigma(CALM_DISTANCE),
                };
                let allowed = |w: WeaponId| body.allows(w);
                let low = policy::preferred(&body.arsenal, &t, &body.damages, &allowed)
                    .and_then(|w| body.armed(w))
                    .filter(|a| {
                        let clip = spec(a.id).clip;
                        a.can_reload() && a.clip.is_some_and(|c| c * 4 < clip || c < 5)
                    });
                if calm && let Some(a) = low {
                    if body.weapon == Some(a.id) && !m.reloading(now) {
                        m.reload_until = now + f64::from(spec(a.id).reload);
                    }
                    self.intents.weapon(
                        Prio::Goal,
                        WeaponIntent {
                            reload: true,
                            ..WeaponIntent::hold(a.id)
                        },
                    );
                } else if let Some(w) = weapon_choice {
                    self.intents.weapon(Prio::Optional, WeaponIntent::hold(w));
                }
            }
        }
    }
}

/// A shot the aim is on: the view, the target and the weapon worked in `mode` at `aim`, `distance` away.
struct Aimed<'a> {
    view: Vec3,
    target: &'a EnemyTrack,
    weapon: WeaponId,
    mode: fire::Mode,
    aim: Vec3,
    distance: f32,
    in_hand: bool,
}

/// Where the game launches `w`'s projectile from when looking along `view`, and how far along the view it must fly
/// clear for its blast to spare the shooter; `None` for weapons without a blast.
fn launch(w: WeaponId, eye: Vec3, view: Vec3, zoomed: bool) -> Option<(Vec3, f32)> {
    let (forward, right, up) = lb_core::math::view_angle_vectors(view);
    match w {
        // The rocket leaves below and to the right of the eye: past a ledge the eye clears it may not.
        WeaponId::Rpg => Some((eye + forward * 16.0 + right * 8.0 - up * 8.0, policy::ROCKET_MIN)),
        WeaponId::Crossbow if !zoomed => Some((eye - up * 2.0, BOLT_CLEAR)),
        WeaponId::Egon => Some((eye, EGON_CLEAR)),
        _ => None,
    }
}

/// The line an explosive would take from where it is launched is clear of walls and of other players near the
/// shooter's end: looked at while the shot is about to go, again every 30 ms at most.
fn blast_clear(
    m: &mut Mind,
    a: &Aimed<'_>,
    beliefs: &lb_knowledge::Beliefs,
    tracer: &mut dyn lb_worldq::Tracer,
    body: &Body,
) -> bool {
    let Some((from, need)) = launch(a.weapon, body.eye, a.view, body.zoomed()) else {
        return true;
    };
    if let Some((at, w, clear)) = m.blast_check
        && w == a.weapon
        && body.now.since(at) < BLAST_CHECK_PERIOD
    {
        return clear;
    }
    let (forward, _, _) = lb_core::math::view_angle_vectors(a.view);
    let reach = a.aim.distance(body.eye);
    let to = from + forward * reach;
    let walls = tracer.trace(&lb_worldq::TraceQuery::line(from, to)).fraction * reach;
    let people = a.weapon != WeaponId::Egon && crate::arms::crowded(beliefs, from, to, Some(a.target.who));
    let clear = walls >= need && !people;
    m.blast_check = Some((body.now, a.weapon, clear));
    clear
}

/// Fires at a target in sight when the view is on it; a fired rocket is guided from then on.
fn shoot(
    m: &mut Mind,
    a: &Aimed<'_>,
    beliefs: &lb_knowledge::Beliefs,
    tracer: &mut dyn lb_worldq::Tracer,
    body: &Body,
    rng: &mut BotRng,
) -> WeaponIntent {
    let now = body.now;
    let (t, w, mode, aim, distance) = (a.target, a.weapon, a.mode, a.aim, a.distance);
    // A crossbow is only worth this far zoomed in: wait for the scope rather than send a bolt.
    let scoped = w != WeaponId::Crossbow || distance < XBOW_ZOOM_FROM || body.zoomed();
    let shot = fire::Shot {
        eye: body.eye,
        view: a.view,
        aim,
        distance,
        enemy_faces_me: target::faces(t, body.origin),
        weapon: w,
    };
    // An explosive bursting on a wall or on someone close in front would hit the bot.
    let engaged = a.in_hand && scoped && fire::on_target(&shot) && blast_clear(m, a, beliefs, tracer, body);
    // The gauss protocol charges; plain shots only when it rolled for them or no charge can start.
    let shoot = engaged && (w != WeaponId::Gauss || m.arms.gauss.plain_allowed(now));
    if engaged {
        if m.answered != Some((t.who, t.recognized_at)) {
            m.answered = Some((t.who, t.recognized_at));
            m.reactions.record(now.since(t.noticed_at), now.since(t.recognized_at));
        }
        let loaded = body.armed(w).is_some_and(|a| a.clip.is_none_or(|c| c > 0));
        if w == WeaponId::Rpg && loaded && m.arms.guide.is_none_or(|(until, _, _)| now >= until) {
            let flight = (distance / ROCKET_AVERAGE + 0.3).min(ROCKET_GUIDE);
            m.arms.guide = Some((now + f64::from(flight), aim, t.who));
            m.arms.hold_until = m.arms.hold_until.max(now + f64::from(flight));
        }
        if w == WeaponId::Shotgun {
            m.arms.double = fire::roll_double(&mut rng.combat);
        }
    }
    m.firing = engaged;
    WeaponIntent {
        select: Some(w),
        fire: if shoot {
            match mode.attack {
                Attack::Primary => Fire::Primary,
                Attack::Secondary => Fire::Secondary,
            }
        } else {
            Fire::None
        },
        trigger: mode.trigger,
        interval: m.click_interval.max(mode.cycle),
        reload: false,
    }
}

impl BotBrain {
    fn vigilance(&mut self, body: &Body) {
        if let Some(a) = self.attention(body.now, body.eye) {
            let prio = match a.reason {
                LookReason::Enemy(_) => None,
                LookReason::Damage => Some(Prio::Threat),
                LookReason::Lost(_) => Some(Prio::Goal),
                LookReason::Glimpse | LookReason::Sound(_) => Some(Prio::Optional),
            };
            if let Some(prio) = prio {
                self.intents.look(
                    prio,
                    LookIntent::Point {
                        at: a.point,
                        engaged: false,
                    },
                );
            }
            self.last_attention = Some(a);
        } else {
            self.last_attention = None;
        }
        if let Some(p) = self.mind.path_look {
            self.intents
                .look(Prio::Optional, LookIntent::Point { at: p, engaged: false });
        }
    }
}

/// The crossbow's scope is snapped on for a shot when the target is far and the view close enough for it to be in the
/// zoomed view; the protocol takes it off again.
fn start_scope(m: &mut Mind, view: Vec3, aim: Vec3, body: &Body, ch: &Character, rng: &mut BotRng) {
    let (forward, _, _) = lb_core::math::view_angle_vectors(view);
    let near = forward.dot((aim - body.eye).normalize_or_zero()) >= SCOPE_START_DOT;
    // Loaded, not reloading, and the scope's toggle ready again: otherwise the game would not put it on. The weapon
    // data may be a frame behind a toggle, the view's zoom is not.
    let toggle = match spec(WeaponId::Crossbow).alt {
        AltFire::Zoom { toggle, .. } => f64::from(toggle),
        _ => 0.0,
    };
    let loaded = body.now.since(m.arms.zoom_flip) >= toggle
        && body.prediction.is_some_and(|p| {
            p.current == Some(WeaponId::Crossbow)
                && p.next_attack <= 0.0
                && p.weapons[WeaponId::Crossbow as usize]
                    .is_some_and(|w| w.clip > 0 && !w.reloading && w.next_secondary <= 0.0)
        });
    if near && loaded && !m.arms.busy() && !body.zoomed() {
        let [lo, hi] = ch.skill.scope_settle;
        let settle = rng.combat.range_f32(lo, hi.max(lo));
        m.arms.active = Some(crate::arms::Active::Scope(lb_combat::arms::scope::Scope::new(
            body.now, settle,
        )));
    }
}

/// How the shot flies, for the aim: projectiles lead the target, rockets go for the feet. A zoomed crossbow shoots a
/// hitscan bolt in multiplayer, and a scope steadies the aim.
fn shot_of(w: WeaponId, attack: Attack, zoomed: bool) -> Shot {
    let (speed, feet) = match (w, attack) {
        (WeaponId::Rpg, _) => (Some(ROCKET_SPEED), true),
        (WeaponId::Crossbow, _) if !zoomed => (Some(BOLT_SPEED), false),
        (WeaponId::Hornetgun, Attack::Secondary) => (Some(DART_SPEED), false),
        _ => (None, false),
    };
    Shot {
        weapon: Some(w),
        speed,
        feet,
        steady: zoomed,
    }
}

/// A press of the scope toggle when the zoom of `w` in hand should change for a target `distance` away (`None`: no
/// target), and the toggle is ready again.
fn zoom_toggle(w: WeaponId, distance: Option<f32>, body: &Body, ready: &mut SimTime) -> Option<WeaponIntent> {
    let AltFire::Zoom { toggle, .. } = spec(w).alt else {
        return None;
    };
    let zoomed = body.zoomed();
    if body.weapon != Some(w) || fire::zoom_wanted(w, distance, zoomed) == zoomed || body.now < *ready {
        return None;
    }
    *ready = body.now + f64::from(toggle) + 0.3;
    Some(WeaponIntent {
        select: Some(w),
        fire: Fire::Secondary,
        trigger: lb_game::mechanics::Trigger::Tap,
        interval: 0.0,
        reload: false,
    })
}
