//! The bot's mind: every frame it turns beliefs into motor intents.
//!
//! - **Combat tick (10 Hz):** picks the enemy to fight and the weapon for its distance.
//! - **Decision (5 Hz, or at once on a new enemy or damage):** picks the goal.
//! - **Goal:** moves the bot. Engage fights in place (strafing, standing, backing off); the others walk a path.
//! - **Aim and fire (priority 70):** owns the look and the weapon whenever an enemy is in sight, whatever the
//!   goal, so a retreating or hunting bot still shoots back.
//! - **Vigilance (priority 20):** looks along the path and glances at sounds.

use lb_combat::aim::{Aim, AimSkill, Shot};
use lb_combat::fight::{Fight, FightInput, FightMove, FightSkill};
use lb_combat::policy::{self, Armed, Choice, Target, XBOW_ZOOM_FROM};
use lb_combat::{fire, target};
use lb_config::skill::SkillParams;
use lb_core::Vec3;
use lb_core::rng::BotRng;
use lb_core::time::SimTime;
use lb_decision::{Decider, Goal, GoalKind, Situation};
use lb_game::dll::DllProfile;
use lb_game::gungame::{GunGame, Kit};
use lb_game::items::Ammo;
use lb_game::mechanics::{
    AltFire, Attack, BOLT_SPEED, DART_SPEED, Damages, ROCKET_GUIDE, ROCKET_SPEED, WeaponClass, spec,
};
use lb_game::self_state::Prediction;
use lb_game::weapons::WeaponId;
use lb_knowledge::{EnemyTrack, PlayerKey, TrackState};
use lb_motor::{Fire, Intents, LookIntent, LookParams, MoveIntent, Prio, StanceIntent, WeaponIntent};
use lb_nav_api::{MapView, NavService, NavStatus, NavStep};
use lb_styles::{Emotions, GoalAffinity, TrickLikes};
use smallvec::SmallVec;

use crate::BotBrain;
use crate::arms::Arms;
use crate::attention::LookReason;
use crate::goals::Task;

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
/// A rocket is guided (and its blast kept away from) this much longer than it should take to get there: the target
/// moves on meanwhile.
const GUIDE_SLACK: f32 = 0.4;
/// Its rocket on the way to a target closer than this, the bot backs off from the blast.
const ROCKET_BACK_OFF: f32 = 450.0;
/// Taking the launcher up wants this much over the least distance for a rocket.
const ROCKET_PICK: f32 = 100.0;
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
/// A melee fighter walks a path to enemies further than this, and charges straight at closer ones.
const MELEE_CHARGE: f32 = 200.0;
const REACTION_SAMPLES: usize = 128;
/// An item spot just reached is not gone to again this soon.
const COLLECTED_REST: f64 = 3.0;
/// A target in sight is kept at least this long.
const TARGET_HOLD: f64 = 1.0;
/// Slower than this the bot stands still (for the statistics).
const STILL_SPEED: f32 = 60.0;
/// GunGame targets: the leader counts as if this much as far (yapb's `gungame_leader_priority` 0.25 on the squared
/// distance), a player one kill from winning and the one who killed the bot last this much more.
const LEADER_SCALE: f32 = 0.5;
const LAST_LEVEL_WEIGHT: f32 = 1.5;
const GRUDGE_WEIGHT: f32 = 1.2;
/// GunGame's say in how a bot feels: two levels or more behind the leader it pushes on, leading it takes care, and in
/// the warmup, where kills do not count, it has nothing to lose.
const BEHIND_AGGRESSION: f32 = 0.2;
const LEADER_FEAR: f32 = 0.15;
const WARMUP_AGGRESSION: f32 = 0.3;
const WARMUP_FEAR: f32 = -0.3;
/// A GunGame player one kill from winning is kept this far off: its crowbar and long jump.
const LAST_LEVEL_KEEP: f32 = 300.0;
/// Out of the bot's own blast by this much more.
const KEEP_MARGIN: f32 = 40.0;
/// A grenade thrown at an enemy closer than this comes down on the thrower too; a snark or a satchel turns on it.
const GRENADE_KEEP: f32 = 280.0;
const SNARK_KEEP: f32 = 200.0;
/// Distances a GunGame duel is weighed at, and how much more damage a second a distance must give the bot (its own
/// less the enemy's) than where it is, to be gone for.
const DUEL_DISTANCES: [f32; 8] = [100.0, 200.0, 300.0, 450.0, 600.0, 800.0, 1000.0, 1300.0];
const DUEL_EDGE: f32 = 10.0;

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
    /// Weapons the bot may use, as a mask of weapon bits: in GunGame what its level gave, on the stand those of
    /// `lb weapons`; all by default.
    pub allowed: u32,
    /// A GunGame match as the bot sees it.
    pub gungame: Option<GunGame>,
    /// BugfixedHL's `mp_selfgauss` (1 elsewhere): whether a charged gauss beam may come back at its shooter.
    pub selfgauss: u8,
    /// Tricks the server lets the bots use.
    pub tricks: lb_config::main_config::TricksConfig,
}

impl Body {
    pub fn allows(&self, w: WeaponId) -> bool {
        self.allowed & w.bit() != 0
    }

    /// `w` hurts other players: in GunGame only the level's weapons do.
    pub fn hurts(&self, w: WeaponId) -> bool {
        self.gungame.is_none_or(|g| g.kit.hurts_with(w))
    }

    /// What the bot holds with nothing to fire: the crowbar, in GunGame the weapon its level gave.
    pub fn fallback(&self) -> WeaponId {
        self.gungame.and_then(|g| g.kit.main()).unwrap_or(WeaponId::Crowbar)
    }

    /// The scope is on (a crossbow or a 357 zoomed in).
    pub fn zoomed(&self) -> bool {
        self.fov > 0.0 && self.fov < 89.0
    }

    pub fn armed(&self, w: WeaponId) -> Option<&Armed> {
        self.arsenal.iter().find(|a| a.id == w)
    }
}

/// How much a bot likes each weapon: a multiplier of how good it finds each gun (by `WeaponId`, 1 = as good as its
/// damage says) and how readily it throws.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WeaponLike {
    pub guns: [f32; 16],
    pub throwables: f32,
}

impl Default for WeaponLike {
    fn default() -> Self {
        WeaponLike {
            guns: [1.0; 16],
            throwables: 1.0,
        }
    }
}

impl WeaponLike {
    pub fn gun(&self, w: WeaponId) -> f32 {
        self.guns.get(w as usize).copied().unwrap_or(1.0)
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
    pub weapons: WeaponLike,
    pub tricks: TrickLikes,
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

/// What the goals came to, for `lb brain` and the stand statistics.
#[derive(Clone, Debug, Default)]
pub struct MindStats {
    /// The enemy aimed at changed to another while the first was still in sight.
    pub target_switches: u32,
    pub investigated: u32,
    /// Cover found from a threat (rather than just away from it).
    pub covers: u32,
    pub camps: u32,
    /// Items waited for and taken as they came back.
    pub controlled: u32,
    pub traps: u32,
    /// Seconds alive out of a fight and in one (a target in sight), and of them standing still.
    pub alive: [f64; 2],
    pub still: [f64; 2],
}

impl MindStats {
    /// Shares of the time alive the bot stood still, out of a fight and in one.
    pub fn still_shares(&self) -> [f64; 2] {
        [0, 1].map(|i| {
            if self.alive[i] > 0.0 {
                self.still[i] / self.alive[i]
            } else {
                0.0
            }
        })
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
    pub(crate) urgent: bool,
    seen_damage: Option<SimTime>,
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
    /// Why it was not, with a target in sight on the last frame.
    pub hold_fire: Option<&'static str>,
    /// Throws, mines, detonations, the gauss charge and dodging.
    pub arms: Arms,
    /// Where the aim was on the last frame an enemy was in sight.
    pub last_aim: Option<Vec3>,
    /// When the bot last had a target to shoot at, and when it took the current one.
    target_at: SimTime,
    target_since: SimTime,
    /// The last look along the line an explosive would take (a wall or someone close in front): when, for which
    /// weapon, and whether it was clear.
    blast_check: Option<(SimTime, WeaponId, bool)>,
    /// Using a charger: which, since when, when it last gave something and what the bot had then.
    charging: Option<(usize, SimTime, SimTime, f32)>,
    /// What the goal is doing: where it chose to go and what it does there.
    pub task: Option<Task>,
    /// Moods that come and go around the personality's aggression and fear.
    pub mood: Emotions,
    /// Spots are not held, and traps not laid, again before these.
    pub camp_rest_until: SimTime,
    pub trap_rest_until: SimTime,
    /// The distance weapons are chosen for when no enemy is about: the range a spot held watches.
    pub calm_distance: Option<f32>,
    pub stats: MindStats,
    /// Long jumps and gauss jumps: timers and statistics.
    pub tricks: crate::tricks::TrickState,
    /// Navigation stands at a gauss boost's takeoff and asks for it this frame.
    pub(crate) nav_boost: Option<lb_nav_api::BoostCall>,
    /// Where the GunGame duel with the target is won, from what each side has in hand: the target, the distance to
    /// keep it off at and the one to close in to.
    pub(crate) duel: Option<(PlayerKey, f32, f32)>,
    /// What the bot's GunGame level gave it, on the last frame.
    kit: Option<Kit>,
}

impl Mind {
    pub fn reset(&mut self) {
        let decider = std::mem::take(&mut self.decider);
        let reactions = std::mem::take(&mut self.reactions);
        let stats = std::mem::take(&mut self.stats);
        let mut mood = self.mood;
        mood.settle();
        let mut arms = std::mem::take(&mut self.arms);
        arms.reset();
        let mut tricks = std::mem::take(&mut self.tricks);
        tricks.reset();
        *self = Mind::default();
        self.tricks = tricks;
        self.decider = decider;
        self.decider.reset();
        self.reactions = reactions;
        self.stats = stats;
        self.mood = mood;
        self.arms = arms;
    }

    pub fn reloading(&self, now: SimTime) -> bool {
        now < self.reload_until
    }

    pub fn last_enemy_seen(&self) -> SimTime {
        self.last_enemy_seen
    }
}

pub(crate) fn apply_step(intents: &mut Intents, step: &NavStep, eye: Vec3, m: &mut Mind) {
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
        longjump: step.longjump,
    };
    if step.mandatory {
        intents.stance(Prio::Traversal, stance);
        let prio = if step.free_look { Prio::Goal } else { Prio::Traversal };
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
        intents.look(prio, look);
    } else {
        intents.stance(Prio::Goal, stance);
    }
    if step.use_key {
        intents.use_key(Prio::Traversal);
    }
    m.nav_fire = step.fire_at.map(|at| (at, step.melee));
    m.nav_boost = step.boost;
    m.path_look = Some(step.look_at);
}

impl BotBrain {
    /// One frame of behavior: the motor's command for this frame.
    pub fn act(
        &mut self,
        body: &Body,
        ch: &Character,
        nav: &mut dyn NavService,
        map: Option<&dyn MapView>,
        rng: &mut BotRng,
    ) -> lb_motor::MotorOut {
        let now = body.now;
        self.intents.clear();
        self.explosives.update(now);
        if self.mind.mood.base() != (ch.aggression, ch.fear) {
            self.mind.mood = Emotions::new(ch.aggression, ch.fear);
        }
        let seen = (self.mind.last_enemy_seen > SimTime::ZERO).then_some(self.mind.last_enemy_seen);
        self.mind.mood.update(now, seen);
        if std::mem::take(&mut self.hurt) {
            self.mind.mood.on_hurt(body.health);
        }
        // A GunGame level changed in the bot's hands: what its old weapons were doing is over.
        let kit = body.gungame.map(|g| g.kit);
        if kit != self.mind.kit {
            if self.mind.kit.is_some() {
                self.mind.arms.reset();
                self.mind.next_combat = now;
                self.mind.urgent = true;
            }
            self.mind.kit = kit;
        }
        // Everything below sees the bot as it feels now.
        let (aggression, fear) = gungame_spirit(body.gungame.as_ref(), self.mind.mood.aggression, self.mind.mood.fear);
        let ch = &Character {
            aggression,
            fear,
            ..ch.clone()
        };
        self.combat_tick(body, ch, nav, rng);
        let fighting = usize::from(
            self.mind
                .target
                .and_then(|k| self.beliefs.track(k))
                .is_some_and(|t| t.state == TrackState::Visible),
        );
        self.mind.stats.alive[fighting] += f64::from(body.dt);
        if body.velocity.truncate().length() < STILL_SPEED {
            self.mind.stats.still[fighting] += f64::from(body.dt);
        }
        self.decide(body, ch, map, rng);
        let tricks = self.nav_tricks(body, ch, rng);
        nav.set_tricks(tricks);
        self.pursue(body, ch, nav, map, rng);
        self.gauss_leap(body, ch, nav, rng);
        self.dodge(body, ch, nav, rng);
        self.dodge_leap_tick(body);
        self.run_protocols(body, ch, nav, rng);
        self.aim_and_fire(body, ch, nav, rng);
        self.snark_defense(body, nav);
        self.vigilance(body);
        self.beam_guard(body);
        self.blast_guard(body);
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
        // Through the scope the bot keeps to its target until the kill, or until it is out of sight for a moment.
        let scoped = match &m.arms.active {
            Some(crate::arms::Active::Scope(sc)) => Some(sc.target),
            _ => None,
        }
        .and_then(|k| self.beliefs.track(k))
        .filter(|t| now.since(t.last_seen) <= lb_combat::arms::scope::LOST_HOLD);
        // A target in sight is kept for a second at least: turning to another and back loses both.
        let held = previous
            .filter(|_| now.since(m.target_since) < TARGET_HOLD)
            .and_then(|k| self.beliefs.track(k))
            .filter(|t| t.state == TrackState::Visible);
        let grudge = self.last_killer;
        let favor = |t: &EnemyTrack| gungame_favor(body.gungame.as_ref(), t, grudge);
        let seen = match (scoped, held) {
            (Some(t), _) | (None, Some(t)) => (t.state == TrackState::Visible).then_some(t.who),
            (None, None) => target::select(self.beliefs.enemies(), body.origin, now, previous, &favor),
        };
        if seen.is_some() && seen != previous {
            m.target_since = now;
        }
        if seen.is_some() {
            m.last_enemy_seen = now;
        }
        m.target = match (scoped, seen) {
            (Some(t), _) => Some(t.who),
            (None, Some(t)) => Some(t),
            // Keep the one just lost in mind for a moment.
            (None, None) => previous.filter(|k| {
                self.beliefs
                    .track(*k)
                    .is_some_and(|t| now.since(t.last_seen) <= LOST_STARE)
            }),
        };
        if seen.is_some() && seen != previous {
            m.urgent = true;
            // Turning from an enemy still in sight to another one.
            if previous
                .and_then(|k| self.beliefs.track(k))
                .is_some_and(|t| t.state == TrackState::Visible)
            {
                m.stats.target_switches += 1;
            }
        }
        let track = m.target.and_then(|k| self.beliefs.track(k));
        let calm = m.calm_distance.unwrap_or(CALM_DISTANCE);
        let distance = track.map_or(calm, |t| t.pos.distance(body.eye));
        let speed = track
            .filter(|t| t.velocity_known(now))
            .map_or(250.0, |t| t.vel.truncate().length());
        // Near its least distance a rocket comes in and out of reach as the enemy's pace changes: the launcher is taken
        // up again only with a margin over it, not to switch back and forth.
        let rockets = body.weapon == Some(WeaponId::Rpg) || m.choice.map(Choice::weapon) == Some(WeaponId::Rpg);
        let t = Target {
            distance,
            speed,
            aim_sigma: ch.aim_sigma(distance),
            rocket_min: track.map_or(rocket_min(body), |t| rocket_from(body, t, distance))
                + if rockets { 0.0 } else { ROCKET_PICK },
        };
        // A weapon the game just would not draw (it has no ammo for it, whatever the bot believed) is left alone.
        let refused = self.motor.weapon.refused(now);
        let like = |w: WeaponId| {
            if body.allows(w) && body.hurts(w) && Some(w) != refused {
                ch.weapons.gun(w)
            } else {
                0.0
            }
        };
        let choice = policy::choose(
            &body.arsenal,
            body.weapon,
            &t,
            body.underwater,
            &body.damages,
            &like,
            body.fallback(),
        );
        if m.choice != Some(choice) {
            if let Choice::Use(w) = choice {
                m.click_interval = fire::click_interval(w, ch.skill.semi_auto_delay, &mut rng.combat);
            }
            m.choice = Some(choice);
        }
        m.duel = match (body.gungame, track) {
            (Some(g), Some(t)) if !g.warmup => {
                duel(body, ch, t, choice.weapon()).map(|(keep, close)| (t.who, keep, close))
            }
            _ => None,
        };
        self.weapon_options(body, ch, nav, rng);
    }

    fn decide(&mut self, body: &Body, ch: &Character, map: Option<&dyn MapView>, rng: &mut BotRng) {
        let now = body.now;
        let calm_for = self.calm_for(now);
        let m = &mut self.mind;
        if !m.urgent && now < m.next_decision && m.decider.current.is_some() {
            return;
        }
        m.urgent = false;
        m.next_decision = now + DECISION_PERIOD;
        let need = |a: Ammo| body.ammo_need[a.index()];
        let mines: SmallVec<[Vec3; 8]> = self.explosives.mines.iter().map(|x| x.pos).collect();
        let old = m.goal.map(|g| g.kind);
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
            allowed: body.allowed,
            map,
            calm_for,
            camp_ready: now >= m.camp_rest_until,
            trap_ready: now >= m.trap_rest_until,
            trap_under_way: match &m.task {
                Some(Task::Trap {
                    started: Some(_),
                    until,
                    ..
                }) => until.is_none_or(|u| now < u),
                Some(Task::Lure { thrown, until, .. }) => thrown.is_some() && until.is_none_or(|u| now < u),
                _ => false,
            },
            mines: &mines,
            charges_out: !self.explosives.charges.is_empty(),
            lure: self.expect.map(|(_, p)| p).or(self.approach),
            gungame: body.gungame,
        };
        let goal = m.decider.decide(&s, &mut rng.decision);
        m.goal = Some(goal);
        if let Some(old) = old
            && old != goal.kind
        {
            self.left_goal(old, ch, now, rng);
        }
    }

    fn pursue(
        &mut self,
        body: &Body,
        ch: &Character,
        nav: &mut dyn NavService,
        map: Option<&dyn MapView>,
        rng: &mut BotRng,
    ) {
        let now = body.now;
        self.mind.nav_fire = None;
        self.mind.nav_boost = None;
        // A long jump or a boost of the way in the air: steered onto its landing whatever the goal is now.
        if let Some(step) = nav.flight() {
            self.intents.movement(
                Prio::Traversal,
                MoveIntent {
                    dir: step.move_dir,
                    speed: step.speed,
                },
            );
            self.intents.stance(
                Prio::Traversal,
                StanceIntent {
                    jump: false,
                    duck: step.duck,
                    longjump: false,
                },
            );
        }
        let Some(goal) = self.mind.goal else { return };
        if !matches!(goal.kind, GoalKind::ControlItem(_)) {
            self.item_focus = None;
        }
        if !matches!(goal.kind, GoalKind::UseCharger(_)) {
            self.mind.charging = None;
        }
        match (goal.kind, map) {
            (GoalKind::Hunt(k), _) => return self.hunt(k, body, map, nav, rng),
            (GoalKind::Investigate(id), _) => return self.investigate(id, body, map, nav, rng),
            (GoalKind::Retreat, _) => return self.retreat(body, ch, nav, rng),
            (GoalKind::ControlItem(i), _) => return self.control(i, body, map, nav, rng),
            (GoalKind::Camp(i), Some(map)) => return self.camp(i, body, ch, map, nav, rng),
            (GoalKind::PlantTrap(t), Some(map)) => return self.trap(t, body, ch, map, nav, rng),
            (GoalKind::Camp(_) | GoalKind::PlantTrap(_), None) => {
                self.mind.decider.complete();
                return;
            }
            _ => {}
        }
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
                // Not at an enemy its own snarks or blasts are on the way to.
                if class == WeaponClass::Melee && distance > MELEE_CHARGE && now >= m.arms.hold_until {
                    let (status, step) = nav.go_to(t.pos);
                    if let Some(step) = step {
                        apply_step(&mut self.intents, &step, body.eye, m);
                    }
                    arrive(status, m);
                    // In sight across open ground a long jump gets there before the way does.
                    let (enemy, visible) = (t.pos, t.state == TrackState::Visible);
                    if step.is_none_or(|s| !s.mandatory) && self.attack_leap(body, ch, enemy, visible, true, nav, rng) {
                        self.leap_stance();
                    }
                    return;
                }
                let mut input = fight_input(m, t, body, ch);
                // Closing in: the way there by the graph, round walls and drops; a jump or a ladder on it is taken
                // whole.
                if input.wants_closer(distance) {
                    let (_, step) = nav.go_to(t.pos);
                    if let Some(step) = step {
                        if step.mandatory {
                            apply_step(&mut self.intents, &step, body.eye, m);
                            return;
                        }
                        input.path = Some(step.move_dir);
                    }
                }
                let mv = m.fight.update(&input, &fight_skill(ch), nav, &mut rng.combat);
                m.path_look = None;
                let (enemy, visible) = (t.pos, t.state == TrackState::Visible);
                let closing = input.wants_closer(distance);
                self.fight_step(mv, enemy, body, ch, nav, rng);
                if self.attack_leap(body, ch, enemy, visible, closing, nav, rng) {
                    self.leap_stance();
                }
            }
            GoalKind::CollectItem(i) => {
                let Some(spot) = self.items.as_ref().and_then(|items| items.spots.get(i)).copied() else {
                    m.decider.complete();
                    return;
                };
                // Not there by the time the bot gets there (as the decision reckoned the way).
                let eta = spot.origin.distance(body.origin) * 1.4 / body.maxspeed.max(100.0);
                let gone = self
                    .items
                    .as_ref()
                    .is_some_and(|items| items.availability(i, now, eta, body.opponents) <= 0.0);
                if gone {
                    m.decider.complete();
                    m.urgent = true;
                    return;
                }
                let (status, step) = nav.go_to(spot.origin);
                if let Some(step) = step {
                    apply_step(&mut self.intents, &step, body.eye, m);
                }
                // Whatever is there now (taken, or left for being of no use) shows on the next look.
                if status == NavStatus::Arrived {
                    m.decider.rest(goal.kind, now + COLLECTED_REST);
                }
                arrive(status, m);
            }
            GoalKind::UseCharger(i) => self.use_charger(i, body, nav, rng),
            GoalKind::Roam => {
                if let Some(step) = nav.roam(&mut rng.decision) {
                    apply_step(&mut self.intents, &step, body.eye, m);
                }
            }
            GoalKind::Hunt(_)
            | GoalKind::Investigate(_)
            | GoalKind::Retreat
            | GoalKind::ControlItem(_)
            | GoalKind::Camp(_)
            | GoalKind::PlantTrap(_) => {}
        }
    }

    /// Walks to the charger's spot, then faces it holding the use key while it gives; spent when it stops giving.
    fn use_charger(&mut self, i: usize, body: &Body, nav: &mut dyn NavService, rng: &mut BotRng) {
        let now = body.now;
        let Some(c) = self.chargers.as_ref().and_then(|c| c.spots.get(i)).copied() else {
            self.mind.decider.complete();
            return;
        };
        // Spent until after the bot gets there (as the decision reckoned the way).
        let eta = c.spot.distance(body.origin) * 1.4 / body.maxspeed.max(100.0);
        if !self
            .chargers
            .as_ref()
            .is_some_and(|ch| ch.available(i, now + f64::from(eta)))
        {
            self.mind.decider.complete();
            self.mind.urgent = true;
            return;
        }
        let there =
            (body.origin - c.spot).truncate().length() < CHARGER_SPOT && (body.origin.z - c.spot.z).abs() < 40.0;
        let m = &mut self.mind;
        // The goal may go straight from another charger to this one.
        m.charging = m.charging.filter(|&(at, ..)| at == i);
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

    /// Takes the fight module's move at the goal's priority; a skilled bot with the module dodges by a long jump
    /// aside instead of a hop.
    pub(crate) fn fight_step(
        &mut self,
        mut mv: FightMove,
        enemy: Vec3,
        body: &Body,
        ch: &Character,
        nav: &mut dyn NavService,
        rng: &mut BotRng,
    ) {
        if mv.jump && self.dodge_aside(body, ch, enemy, mv.velocity, nav, rng) {
            mv.jump = false;
        }
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
                longjump: false,
            },
        );
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
        m.hold_fire = None;
        // With a throwable in hand the view stays on the enemy while the gun comes out; a throw turns to its own arc
        // and the satchel radio watches its charges over it (their protocols' looks come first).
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
                    m.hold_fire = Some("no aim point yet");
                    return;
                };
                m.last_aim = Some(aim);
                self.intents
                    .look(Prio::Threat, LookIntent::Point { at: aim, engaged: true });
                let intent = match choice {
                    // Throws and mines are the protocols': in hand, never fired at the target.
                    Choice::Use(w) if spec(w).class == WeaponClass::Throwable => {
                        m.hold_fire = Some("nothing to fire but throws");
                        WeaponIntent::hold(w)
                    }
                    Choice::Use(w) => {
                        let in_hand = body.weapon == Some(w) && !m.reloading(now);
                        if in_hand && w == WeaponId::Crossbow && distance >= XBOW_ZOOM_FROM {
                            start_scope(m, self.motor.view, aim, t.who, body, ch, rng);
                        }
                        match zoom_toggle(w, Some(distance), body, &mut m.arms.zoom_ready).filter(|_| in_hand) {
                            Some(toggle) => {
                                m.hold_fire = Some("the scope goes on or off");
                                toggle
                            }
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
                        m.hold_fire = Some("nothing loaded: reloading");
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
                if t.state == TrackState::Visible {
                    m.hold_fire = Some("no weapon chosen");
                }
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
                // Breaking an obstacle in the way: the crowbar when asked (and carried), else the weapon of choice;
                // never a throw.
                let Some((at, melee)) = m.nav_fire else { unreachable!() };
                let crowbar = body.allows(WeaponId::Crowbar) && body.armed(WeaponId::Crowbar).is_some();
                let weapon = match weapon_choice {
                    Some(w) if !(melee && crowbar) => w,
                    _ if crowbar => WeaponId::Crowbar,
                    _ => body.fallback(),
                };
                if spec(weapon).class == WeaponClass::Throwable {
                    return;
                }
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
                let calm_distance = m.calm_distance.unwrap_or(CALM_DISTANCE);
                let t = Target {
                    distance: calm_distance,
                    speed: 250.0,
                    aim_sigma: ch.aim_sigma(calm_distance),
                    rocket_min: rocket_min(body),
                };
                let like = |w: WeaponId| {
                    if body.allows(w) && body.hurts(w) {
                        ch.weapons.gun(w)
                    } else {
                        0.0
                    }
                };
                let low = policy::preferred(&body.arsenal, &t, &body.damages, &like)
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

/// Rockets are not fired closer than this now: out of their blast's reach, or into its edge with health to spare.
pub(crate) fn rocket_min(body: &Body) -> f32 {
    policy::rocket_min(body.health, body.damages.primary(WeaponId::Rpg))
}

/// Rockets are not fired at `t`, `distance` away, closer than this: [`rocket_min`] where the rocket will meet it,
/// the target and the bot closing in on each other meanwhile.
fn rocket_from(body: &Body, t: &EnemyTrack, distance: f32) -> f32 {
    let to = (t.pos - body.origin).truncate().normalize_or_zero();
    let theirs = if t.velocity_known(body.now) {
        (-t.vel.truncate()).dot(to).max(0.0)
    } else {
        0.0
    };
    let mine = body.velocity.truncate().dot(to).max(0.0);
    rocket_min(body) + (theirs + mine) * rocket_flight(distance)
}

/// Aggression and fear with GunGame's say in them.
fn gungame_spirit(gungame: Option<&GunGame>, aggression: f32, fear: f32) -> (f32, f32) {
    let (more, less) = match gungame {
        Some(g) if g.warmup => (WARMUP_AGGRESSION, WARMUP_FEAR),
        Some(g) if g.leads() => (0.0, LEADER_FEAR),
        Some(g) if g.behind() >= 2 => (BEHIND_AGGRESSION, 0.0),
        _ => (0.0, 0.0),
    };
    ((aggression + more).clamp(0.0, 1.0), (fear + less).clamp(0.0, 1.0))
}

/// How much GunGame makes `t` count as a target: the leader as if nearer, a player one kill from winning and the one
/// who killed the bot last (`grudge`) more.
fn gungame_favor(gungame: Option<&GunGame>, t: &EnemyTrack, grudge: Option<u8>) -> target::Favor {
    let mut favor = target::Favor::default();
    let Some(g) = gungame.filter(|g| !g.warmup) else {
        return favor;
    };
    if g.board.leader == Some(t.who.slot) {
        favor.scale = LEADER_SCALE;
    }
    if g.on_last_level(t.who.slot, t.traits.weapon) {
        favor.weight *= LAST_LEVEL_WEIGHT;
    }
    if grudge == Some(t.who.slot) {
        favor.weight *= GRUDGE_WEIGHT;
    }
    favor
}

/// Where the duel of the bot's gun `mine` against the one `t` shows is won: the distances to keep `t` off at and to
/// close in to, when some distance gives the bot (its damage a second less the enemy's) a clear edge over where it is.
fn duel(body: &Body, ch: &Character, t: &EnemyTrack, mine: WeaponId) -> Option<(f32, f32)> {
    if matches!(spec(mine).class, WeaponClass::Melee | WeaponClass::Throwable) {
        return None;
    }
    let theirs = t.traits.weapon.filter(|w| *w != mine)?;
    let full = |w: WeaponId| Armed::new(w, Some(spec(w).clip.max(1)), Some(spec(w).clip.max(1)));
    let edge = |d: f32| {
        let at = Target {
            distance: d,
            speed: 250.0,
            aim_sigma: ch.aim_sigma(d),
            rocket_min: rocket_min(body),
        };
        policy::score(&full(mine), &at, &body.damages) - policy::score(&full(theirs), &at, &body.damages)
    };
    let now_at = t.pos.distance(body.origin);
    let here = edge(now_at);
    let (best_at, best) = DUEL_DISTANCES
        .iter()
        .map(|&d| (d, edge(d)))
        .max_by(|a, b| a.1.total_cmp(&b.1))?;
    if best < here + DUEL_EDGE {
        None
    } else if best_at > now_at {
        Some((best_at * 0.8, f32::INFINITY))
    } else {
        Some((0.0, best_at * 1.2))
    }
}

/// How close the bot lets `t` come: not into its own weapon's blast, not within the reach of a GunGame player one
/// kill from winning, not where the duel is lost.
fn keep_away(m: &Mind, t: &EnemyTrack, body: &Body) -> f32 {
    let weapon = m.choice.map(Choice::weapon).or(body.weapon);
    if weapon.is_none_or(|w| spec(w).class == WeaponClass::Melee) {
        return 0.0;
    }
    let own = match weapon {
        Some(WeaponId::Rpg) => rocket_min(body) + KEEP_MARGIN,
        Some(WeaponId::Crossbow) if !body.zoomed() => BOLT_CLEAR + KEEP_MARGIN,
        Some(WeaponId::Egon) => EGON_CLEAR + KEEP_MARGIN,
        Some(WeaponId::HandGrenade) => GRENADE_KEEP,
        Some(WeaponId::Snark | WeaponId::Satchel) => SNARK_KEEP,
        _ => 0.0,
    };
    let last = body
        .gungame
        .as_ref()
        .is_some_and(|g| g.on_last_level(t.who.slot, t.traits.weapon));
    let duel = m.duel.filter(|(k, ..)| *k == t.who).map_or(0.0, |(_, keep, _)| keep);
    own.max(if last { LAST_LEVEL_KEEP } else { 0.0 }).max(duel)
}

/// What the fight module goes by against `t`.
pub(crate) fn fight_input(m: &Mind, t: &EnemyTrack, body: &Body, ch: &Character) -> FightInput {
    let now = body.now;
    let keep = keep_away(m, t, body);
    let close = m
        .duel
        .filter(|(k, ..)| *k == t.who)
        .map_or(f32::INFINITY, |(.., close)| close);
    FightInput {
        now,
        origin: body.origin,
        enemy: t.pos,
        enemy_facing: t.traits.facing,
        enemy_faces_me: target::faces(t, body.origin),
        approach: body.health.clamp(0.0, 100.0) * ch.aggression,
        weapon: body.weapon.map_or(WeaponClass::Melee, |w| spec(w).class),
        reloading: m.reloading(now),
        hold_ground: now < m.arms.hold_until,
        back_off: m.arms.guide.is_some_and(|(until, _, _)| now < until)
            && t.pos.distance(body.origin) < ROCKET_BACK_OFF,
        on_ground: body.on_ground,
        velocity: body.velocity.truncate(),
        maxspeed: body.maxspeed,
        close_in: lb_combat::fight::close_in(body.weapon).min(close).max(keep * 1.25),
        keep_away: keep,
        path: None,
    }
}

pub(crate) fn fight_skill(ch: &Character) -> FightSkill {
    FightSkill {
        skill: ch.level,
        stay_mid: ch.skill.stay_mid,
        stay_far: ch.skill.stay_far,
        crouch_tap: ch.skill.crouch_tap,
        dodge_hop_cooldown: ch.skill.dodge_hop_cooldown,
    }
}

/// Seconds a rocket takes to fly `distance`: 250 units/s for the 0.4 s before it ignites, some 1200 on average
/// after.
fn rocket_flight(distance: f32) -> f32 {
    if distance <= 100.0 {
        distance / 250.0
    } else {
        0.4 + (distance - 100.0) / 1200.0
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
fn launch(w: WeaponId, eye: Vec3, view: Vec3, zoomed: bool, rockets_from: f32) -> Option<(Vec3, f32)> {
    let (forward, right, up) = lb_core::math::view_angle_vectors(view);
    match w {
        // The rocket leaves below and to the right of the eye: past a ledge the eye clears it may not.
        WeaponId::Rpg => Some((eye + forward * 16.0 + right * 8.0 - up * 8.0, rockets_from)),
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
    let Some((from, need)) = launch(
        a.weapon,
        body.eye,
        a.view,
        body.zoomed(),
        rocket_from(body, a.target, a.distance),
    ) else {
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
        rocket_min: rocket_from(body, t, distance),
    };
    // An explosive bursting on a wall or on someone close in front would hit the bot.
    let hold = if !a.in_hand {
        Some(if body.weapon == Some(w) {
            "reloading"
        } else {
            "its weapon is not out yet"
        })
    } else if !scoped {
        Some("waiting for the scope")
    } else if !fire::on_target(&shot) {
        Some(fire::off_target(&shot))
    } else if !blast_clear(m, a, beliefs, tracer, body) {
        Some("the blast would reach it")
    } else {
        None
    };
    let engaged = hold.is_none();
    // The gauss protocol charges; plain shots only when it rolled for them or no charge can start.
    let shoot = engaged && (w != WeaponId::Gauss || m.arms.gauss.plain_allowed(now));
    m.hold_fire = hold.or((engaged && !shoot).then_some("the gauss charges instead"));
    if engaged {
        if m.answered != Some((t.who, t.recognized_at)) {
            m.answered = Some((t.who, t.recognized_at));
            m.reactions.record(now.since(t.noticed_at), now.since(t.recognized_at));
        }
        let loaded = body.armed(w).is_some_and(|a| a.clip.is_none_or(|c| c > 0));
        if w == WeaponId::Rpg && loaded && m.arms.guide.is_none_or(|(until, _, _)| now >= until) {
            let flight = (rocket_flight(distance) + GUIDE_SLACK).min(ROCKET_GUIDE);
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
                LookReason::Expect(_) | LookReason::Danger | LookReason::Glimpse | LookReason::Sound(_) => {
                    Some(Prio::Optional)
                }
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
fn start_scope(m: &mut Mind, view: Vec3, aim: Vec3, target: PlayerKey, body: &Body, ch: &Character, rng: &mut BotRng) {
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
            body.now, settle, target,
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
