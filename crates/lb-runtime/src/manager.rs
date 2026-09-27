//! Bot lifecycle and quota management.

use std::sync::Arc;

use lb_config::main_config::{QuotaConfig, QuotaMode};
use lb_config::skill::SkillParams;
use lb_core::Vec3;
use lb_core::handles::BotId;
use lb_core::rng::BotRng;
use lb_core::time::SimTime;
use lb_game::self_state::{DEAD_NO, DEAD_RESPAWNABLE, SelfState};
use lb_host::driver::CommandDriver;

use crate::motor_test::MotorTest;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BotState {
    Connecting,
    Spawned,
    Alive,
    Dead,
    Respawning,
    Leaving,
    Faulted,
}

impl BotState {
    pub fn as_str(self) -> &'static str {
        match self {
            BotState::Connecting => "connecting",
            BotState::Spawned => "spawned",
            BotState::Alive => "alive",
            BotState::Dead => "dead",
            BotState::Respawning => "respawning",
            BotState::Leaving => "leaving",
            BotState::Faulted => "faulted",
        }
    }
}

pub struct Bot {
    pub id: BotId,
    pub userid: i32,
    /// Who the bot is: nickname, style, skill, look.
    pub persona: Arc<lb_styles::Persona>,
    /// The persona's skill resolved against `config/difficulty.yaml`.
    pub skill: SkillParams,
    pub state: BotState,
    pub state_since: SimTime,
    pub self_state: SelfState,
    pub driver: CommandDriver,
    pub rng: BotRng,
    pub view: Vec3,
    pub view_initialized: bool,
    pub respawn_at: Option<SimTime>,
    pub respawn_presses: u32,
    pub test: Option<MotorTest>,
    /// `lb selftest`: the weapon rules checked by this bot.
    pub selftest: Option<crate::selftest::SelfTest>,
    /// `lb nav test`: the obstacle course instead of behavior.
    pub nav_test: Option<crate::nav_test::NavTest>,
    pub fault_on_next_frame: bool,
    pub seen_reset_hud: bool,
    pub pending_client_cmds: Vec<Vec<String>>,
    pub sim_ms_since_test: f64,
    pub kick_attempts: u8,
    pub nav: lb_nav::navigator::Navigator,
    /// Senses, beliefs, decisions and motor.
    pub brain: lb_brain::BotBrain,
    /// Skill, traits and style as the brain uses them.
    pub character: lb_brain::Character,
    /// What the bot looked at on its last frame, when not along its path.
    pub attention: Option<lb_brain::Attention>,
    /// The weapon in hand on the last frame and its rounds left, for `lb stats`.
    pub rounds: Option<(lb_game::weapons::WeaponId, i32)>,
}

impl Bot {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: BotId,
        userid: i32,
        persona: Arc<lb_styles::Persona>,
        skill: SkillParams,
        affinity: lb_styles::GoalAffinity,
        now: SimTime,
        master_seed: u64,
        cmd_rate: f64,
        max_debt_ms: f64,
    ) -> Bot {
        let rng = BotRng::new(master_seed, persona.seed);
        let brain = lb_brain::BotBrain::new(id.slot, lb_perception::PerceptionParams::from_skill(&skill));
        let character = character(&persona, &skill, affinity);
        Bot {
            id,
            userid,
            persona,
            skill,
            state: BotState::Connecting,
            state_since: now,
            self_state: SelfState::default(),
            driver: CommandDriver::new(cmd_rate, max_debt_ms),
            rng,
            view: Vec3::ZERO,
            view_initialized: false,
            respawn_at: None,
            respawn_presses: 0,
            test: None,
            selftest: None,
            fault_on_next_frame: false,
            seen_reset_hud: false,
            pending_client_cmds: Vec::new(),
            sim_ms_since_test: 0.0,
            kick_attempts: 0,
            nav_test: None,
            nav: lb_nav::navigator::Navigator::default(),
            brain,
            character,
            attention: None,
            rounds: None,
        }
    }

    /// The personality, its skill or its style changed (config reload).
    pub fn set_persona(
        &mut self,
        persona: Arc<lb_styles::Persona>,
        skill: SkillParams,
        affinity: lb_styles::GoalAffinity,
    ) {
        self.brain.params = lb_perception::PerceptionParams::from_skill(&skill);
        self.character = character(&persona, &skill, affinity);
        self.skill = skill;
        self.persona = persona;
    }

    /// Still owns its slot: not kicked and not faulted.
    pub fn is_active(&self) -> bool {
        !matches!(self.state, BotState::Leaving | BotState::Faulted)
    }

    pub fn set_state(&mut self, state: BotState, now: SimTime) {
        if self.state != state {
            tracing::debug!(
                "bot {} (#{}): {} -> {}",
                self.persona.name,
                self.userid,
                self.state.as_str(),
                state.as_str()
            );
            self.state = state;
            self.state_since = now;
        }
    }

    /// Advances the lifecycle from the bot's own entity state.
    pub fn update_lifecycle(&mut self, now: SimTime, force_respawn: bool, respawn_delay: [f32; 2]) {
        if matches!(self.state, BotState::Leaving | BotState::Faulted) {
            return;
        }
        let body = &self.self_state.body;
        let alive = body.deadflag == DEAD_NO && body.health > 0.0;
        match self.state {
            BotState::Connecting | BotState::Spawned => {
                if alive && (self.seen_reset_hud || now.since(self.state_since) > 1.0) {
                    self.set_state(BotState::Alive, now);
                }
            }
            BotState::Alive => {
                if !alive {
                    self.set_state(BotState::Dead, now);
                    self.respawn_at = None;
                    self.respawn_presses = 0;
                    self.test = None;
                    if self.selftest.take().is_some() {
                        tracing::warn!("self-test stopped: {} died", self.persona.name);
                    }
                    self.nav.reset();
                    self.brain.on_death();
                }
            }
            BotState::Dead | BotState::Respawning => {
                if alive {
                    self.self_state.on_spawn(now);
                    self.nav.reset();
                    self.brain.on_spawn();
                    self.set_state(BotState::Alive, now);
                } else if force_respawn && body.deadflag == DEAD_RESPAWNABLE {
                    if self.respawn_at.is_none() {
                        let delay = self.rng.motor.range_f32(respawn_delay[0], respawn_delay[1]) as f64;
                        self.respawn_at = Some(now + delay);
                    }
                    if self.state == BotState::Dead && self.respawn_at.is_some_and(|t| now >= t) {
                        self.set_state(BotState::Respawning, now);
                    }
                }
            }
            BotState::Leaving | BotState::Faulted => {}
        }
    }
}

fn character(
    persona: &lb_styles::Persona,
    skill: &SkillParams,
    affinity: lb_styles::GoalAffinity,
) -> lb_brain::Character {
    lb_brain::Character {
        skill: skill.clone(),
        level: persona.skill,
        aggression: persona.aggression,
        fear: persona.fear,
        affinity,
    }
}

/// How many bots the quota wants given the humans on the server.
pub fn desired_bots(q: &QuotaConfig, humans: u32, max_clients: u32) -> u32 {
    let mut desired = match q.mode {
        QuotaMode::Normal => q.count,
        QuotaMode::Fill => q.count.saturating_sub(humans),
        QuotaMode::Match => (humans as f32 * q.match_ratio).round() as u32,
    };
    if q.join_after_player && humans == 0 {
        desired = 0;
    }
    let reserve = if q.autovacate { q.keep_slots } else { 0 };
    desired.min(max_clients.saturating_sub(humans + reserve))
}

/// Pacing of bot creation. Times belong to the current map: sim time restarts on every map, so the first
/// deadline is set on the map's first quota check (`next_at == None`).
#[derive(Clone, Copy, Debug, Default)]
pub struct Creation {
    pub next_at: Option<SimTime>,
    pub hold_until: SimTime,
}

impl Creation {
    /// True when a bot may be created now; arms the join delay on the first call of a map.
    pub fn ready(&mut self, now: SimTime, join_delay: f64) -> bool {
        let next = *self.next_at.get_or_insert(now + join_delay);
        now >= next && now >= self.hold_until
    }
}

/// Chooses which bot to remove: dead ones first, then the lowest score.
pub fn pick_bot_to_kick(bots: &[Bot]) -> Option<usize> {
    let active = |b: &&Bot| !matches!(b.state, BotState::Leaving | BotState::Faulted);
    bots.iter()
        .enumerate()
        .filter(|(_, b)| active(b))
        .min_by(|(_, a), (_, b)| {
            let dead = |x: &Bot| !matches!(x.state, BotState::Dead | BotState::Respawning);
            (dead(a), a.self_state.body.frags as i32).cmp(&(dead(b), b.self_state.body.frags as i32))
        })
        .map(|(i, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quota(mode: QuotaMode, count: u32) -> QuotaConfig {
        QuotaConfig {
            mode,
            count,
            ..QuotaConfig::default()
        }
    }

    #[test]
    fn fill_leaves_a_slot_for_humans() {
        let q = quota(QuotaMode::Fill, 8);
        assert_eq!(desired_bots(&q, 0, 16), 8);
        assert_eq!(desired_bots(&q, 3, 16), 5);
        assert_eq!(desired_bots(&q, 10, 16), 0);
        let q24 = quota(QuotaMode::Normal, 24);
        assert_eq!(desired_bots(&q24, 0, 24), 23, "autovacate keeps one slot free");
        assert_eq!(desired_bots(&q24, 5, 24), 18);
    }

    #[test]
    fn creation_delay_starts_on_the_map() {
        let mut c = Creation::default();
        assert!(!c.ready(SimTime(1.0), 5.0), "join delay is armed on the first check");
        assert!(!c.ready(SimTime(5.9), 5.0));
        assert!(c.ready(SimTime(6.0), 5.0));
        c = Creation::default();
        assert!(
            !c.ready(SimTime(1.0), 5.0),
            "a new map starts over even if the old map ran longer"
        );
    }

    #[test]
    fn join_after_player_and_match() {
        let mut q = quota(QuotaMode::Match, 0);
        q.match_ratio = 1.0;
        assert_eq!(desired_bots(&q, 3, 16), 3);
        q.mode = QuotaMode::Normal;
        q.count = 4;
        q.join_after_player = true;
        assert_eq!(desired_bots(&q, 0, 16), 0);
    }
}
