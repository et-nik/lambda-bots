//! The bot's mind: every frame it turns beliefs into motor intents.
//!
//! - **Combat tick (10 Hz):** picks the enemy to fight and the weapon for its distance.
//! - **Decision (5 Hz, or at once on a new enemy or damage):** picks the goal.
//! - **Goal:** moves the bot. Engage fights in place (strafing, standing, backing off); the others walk a path.
//! - **Aim and fire (priority 70):** owns the look and the weapon whenever an enemy is in sight, whatever the
//!   goal, so a retreating or hunting bot still shoots back.
//! - **Vigilance (priority 20):** looks along the path and glances at sounds.

use lb_combat::aim::{Aim, AimSkill};
use lb_combat::fight::{Fight, FightInput, FightSkill};
use lb_combat::policy::{self, Armed, Choice};
use lb_combat::{fire, target};
use lb_config::skill::SkillParams;
use lb_core::Vec3;
use lb_core::rng::BotRng;
use lb_core::time::SimTime;
use lb_decision::{Decider, Goal, GoalKind, Situation};
use lb_game::items::Ammo;
use lb_game::mechanics::{WeaponClass, spec};
use lb_game::weapons::WeaponId;
use lb_knowledge::{PlayerKey, TrackState};
use lb_motor::{Fire, Intents, LookIntent, LookParams, MoveIntent, Prio, StanceIntent, WeaponIntent};
use lb_nav_api::{NavService, NavStatus, NavStep};
use lb_styles::GoalAffinity;
use smallvec::SmallVec;

use crate::BotBrain;
use crate::attention::LookReason;

const COMBAT_PERIOD: f64 = 0.1;
const DECISION_PERIOD: f64 = 0.2;
/// Keep looking where a lost target went for this long.
const LOST_STARE: f64 = 1.0;
/// Reload only after this long without an enemy in sight.
const CALM_BEFORE_RELOAD: f64 = 2.0;
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
    /// Weapon confirmed by `CurWeapon`.
    pub weapon: Option<WeaponId>,
    pub arsenal: SmallVec<[Armed; 16]>,
    /// How much each ammo type is needed, 0..1, by [`Ammo::index`].
    pub ammo_need: [f32; 7],
    /// Other players on the server.
    pub opponents: usize,
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
    click_interval: f32,
    next_combat: SimTime,
    next_decision: SimTime,
    urgent: bool,
    seen_damage: Option<SimTime>,
    retreat_to: Option<(Vec3, SimTime)>,
    reload_until: SimTime,
    last_enemy_seen: SimTime,
    /// Where the path wants the bot to look, for vigilance.
    path_look: Option<Vec3>,
    /// The contact already shot at: player and when it was recognized.
    answered: Option<(PlayerKey, SimTime)>,
    pub reactions: Reactions,
    /// The trigger was pulled on the last frame.
    pub firing: bool,
}

impl Mind {
    pub fn reset(&mut self) {
        let decider = std::mem::take(&mut self.decider);
        let reactions = std::mem::take(&mut self.reactions);
        *self = Mind::default();
        self.decider = decider;
        self.decider.reset();
        self.reactions = reactions;
    }

    pub fn reloading(&self, now: SimTime) -> bool {
        now < self.reload_until
    }
}

fn apply_step(intents: &mut Intents, step: &NavStep, path_look: &mut Option<Vec3>) {
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
                let d = step.look_at;
                let mut angles = lb_core::math::dir_to_view_angles(d);
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
    *path_look = Some(step.look_at);
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
        self.combat_tick(body, ch, rng);
        self.decide(body, ch, rng);
        self.pursue(body, ch, nav, rng);
        self.aim_and_fire(body, ch, rng);
        self.vigilance(body);
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

    fn combat_tick(&mut self, body: &Body, ch: &Character, rng: &mut BotRng) {
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
        let distance = m
            .target
            .and_then(|k| self.beliefs.track(k))
            .map_or(600.0, |t| t.pos.distance(body.eye));
        let choice = policy::choose(
            &body.arsenal,
            body.weapon,
            distance,
            body.underwater,
            ch.aim_sigma(distance),
        );
        if m.choice != Some(choice) {
            if let Choice::Use(w) = choice {
                m.click_interval = fire::click_interval(w, ch.skill.semi_auto_delay, &mut rng.combat);
            }
            m.choice = Some(choice);
        }
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
                        apply_step(&mut self.intents, &step, &mut m.path_look);
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
                    apply_step(&mut self.intents, &step, &mut m.path_look);
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
                            apply_step(&mut self.intents, &step, &mut m.path_look);
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
                    apply_step(&mut self.intents, &step, &mut m.path_look);
                }
                arrive(status, m);
            }
            GoalKind::Roam => {
                if let Some(step) = nav.roam(&mut rng.decision) {
                    apply_step(&mut self.intents, &step, &mut m.path_look);
                }
            }
        }
    }

    fn aim_and_fire(&mut self, body: &Body, ch: &Character, rng: &mut BotRng) {
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
                let Some(aim) = m.aim.point(now, body.eye, body.weapon, &aim_skill, &mut rng.combat) else {
                    return;
                };
                self.intents
                    .look(Prio::Threat, LookIntent::Point { at: aim, engaged: true });
                let intent = match choice {
                    Choice::Use(w) => {
                        let shot = fire::Shot {
                            eye: body.eye,
                            view: self.motor.view,
                            aim,
                            distance: t.pos.distance(body.eye),
                            enemy_faces_me: target::faces(t, body.origin),
                            weapon: w,
                        };
                        let ready = body.weapon == Some(w) && !m.reloading(now);
                        let shoot = ready && fire::on_target(&shot);
                        if shoot && m.answered != Some((t.who, t.recognized_at)) {
                            m.answered = Some((t.who, t.recognized_at));
                            m.reactions.record(now.since(t.noticed_at), now.since(t.recognized_at));
                        }
                        m.firing = shoot;
                        WeaponIntent {
                            select: Some(w),
                            fire: if shoot { Fire::Primary } else { Fire::None },
                            trigger: spec(w).trigger,
                            interval: m.click_interval,
                            reload: false,
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
            _ => {
                // Calm: reload what is low, otherwise hold the best weapon.
                let calm = now.since(m.last_enemy_seen) >= CALM_BEFORE_RELOAD;
                let low = body
                    .weapon
                    .and_then(|w| body.arsenal.iter().find(|a| a.id == w))
                    .filter(|a| {
                        let clip = spec(a.id).clip;
                        a.can_reload() && a.clip.is_some_and(|c| c * 4 < clip || c < 5)
                    });
                if calm && let Some(a) = low {
                    if !m.reloading(now) {
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
