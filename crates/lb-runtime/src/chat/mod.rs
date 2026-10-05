//! Bots in the game chat. The map's journal is kept from the kill feed, the players' chat and the scoreboard; the
//! director picks who says something; requests go to the chat worker, and its replies come back as a recorded
//! outside input, so a replay types the same lines at the same frames. An alive bot types standing still, in a calm
//! moment; a dead one types at once and holds its respawn. The line ends as `say`.

mod backend;
mod command;
mod store;
mod transcript;
mod worker;

use std::collections::{BTreeMap, VecDeque};
use std::time::Duration;

use lb_chat::botchat::{notice_secs, think_secs, typing_secs};
use lb_chat::lang::Lang;
use lb_chat::memory::player_key;
use lb_chat::request::{PlayerMap, Recent, request_id};
use lb_chat::{
    BotCard, Can, Carry, Cause, ChatRequest, Director, Event, Journal, Limits, MapSummary, Notable, Outcome,
    PlayerCard, Priority, Reply, Scene, Speak, Speaker, Trigger, Who, sanitize,
};
use lb_core::time::SimTime;
use lb_game::dll::DllKind;
use lb_game::gungame::{GunGame, Kit, SLOTS};
use lb_game::self_state::{FL_FROZEN, FL_ONGROUND, MOVETYPE_FLY};
use lb_raw::{ClientEvent, ClientEventKind, CommandEvent};

#[cfg(test)]
pub use backend::FakeBackend;
pub use backend::{ChatBackend, Job, NullBackend};
pub(crate) use command::command;

use crate::Runtime;
use crate::clients::ClientInfo;
use crate::manager::{Bot, BotState};

/// Seconds a dead bot may keep from respawning to finish its line: about until the game respawns it anyway
/// (`mp_forcerespawn`, 5 s after it could respawn).
pub const RESPAWN_HOLD: f64 = 5.0;
/// Seconds an answer, a remark, a word at the end of a match stay worth saying.
const KEEP_ANSWER: f64 = 25.0;
const KEEP_REMARK: f64 = 15.0;
const KEEP_MATCH_END: f64 = 12.0;
/// Seconds of the journal a request shows.
const EVENTS_WINDOW: f64 = 180.0;
/// Shortest time between two lines of one bot: faster ones the game drops.
const SAY_GAP: f64 = 1.5;
/// A bot killed while typing may complain this long after.
const KILLED_TYPING_WINDOW: f64 = 10.0;
/// A crowbar kill by the leader wins a GunGame match if the players are frozen this soon after.
const GG_WIN_WINDOW: f64 = 5.0;
/// Lines kept for `lb chat log`.
const LOG_LINES: usize = 50;
/// Moments of a map kept for the memory.
const MOMENTS: usize = 200;
/// Enemies this close, or nearer, interrupt typing.
const DANGER_RANGE: f32 = 800.0;

#[derive(Clone, Copy, Debug, Default)]
pub struct Counts {
    pub asked: u64,
    pub said: u64,
    pub silent: u64,
    pub failed: u64,
}

struct Pending {
    id: u64,
    slot: u8,
    generation: u32,
    /// The player the line answers.
    to: Option<i32>,
}

pub struct ChatRuntime {
    pub journal: Journal,
    director: Director,
    pub backend: Box<dyn ChatBackend>,
    epoch: u32,
    seq: u32,
    pending: Vec<Pending>,
    /// Bots whose line on its way answers a player: (bot slot, player userid).
    answering: Vec<(u8, i32)>,
    causes: Vec<Cause>,
    /// Humans of this map by `userid`, with the keys the memory knows them by.
    keys: BTreeMap<i32, String>,
    /// Humans who were on the server when the last map ended: they are not greeted again.
    returning: Vec<i32>,
    /// Moments of this map with humans in them, for the memory.
    moments: Vec<(i32, Notable)>,
    levels: [Option<i16>; SLOTS],
    leader: Option<u8>,
    board_at: SimTime,
    /// GunGame: the last level's kill that may have won the match.
    gg_kill: Option<(SimTime, Who)>,
    /// Bots that reached the last level on this map.
    last_level: Vec<u8>,
    pub match_over: bool,
    late_load: bool,
    pub log: VecDeque<String>,
    pub counts: Counts,
}

impl ChatRuntime {
    pub fn new() -> ChatRuntime {
        let limits = Limits {
            lines_per_minute: 1.0,
            requests_per_minute: 1.0,
            remark_gap: 0.0,
            bot_remark_gap: 0.0,
        };
        ChatRuntime {
            journal: Journal::new("", SimTime::ZERO),
            director: Director::new(0, SimTime::ZERO, &limits),
            backend: Box::new(NullBackend),
            epoch: 0,
            seq: 0,
            pending: Vec::new(),
            answering: Vec::new(),
            causes: Vec::new(),
            keys: BTreeMap::new(),
            returning: Vec::new(),
            moments: Vec::new(),
            levels: [None; SLOTS],
            leader: None,
            board_at: SimTime::ZERO,
            gg_kill: None,
            last_level: Vec::new(),
            match_over: false,
            late_load: false,
            log: VecDeque::new(),
            counts: Counts::default(),
        }
    }

    /// What carries over a map change.
    pub fn carry(&self) -> Carry {
        Carry {
            humans: self.returning.clone(),
        }
    }

    pub fn restore(&mut self, carry: Carry) {
        self.returning = carry.humans;
    }

    fn log(&mut self, now: SimTime, line: String) {
        tracing::debug!("chat: {line}");
        self.log.push_back(format!("[{:7.1}] {line}", now.secs()));
        while self.log.len() > LOG_LINES {
            self.log.pop_front();
        }
    }
}

impl Default for ChatRuntime {
    fn default() -> Self {
        ChatRuntime::new()
    }
}

fn limits(c: &lb_config::main_config::ChatConfig) -> Limits {
    Limits {
        lines_per_minute: c.limits.lines_per_minute,
        requests_per_minute: c.limits.requests_per_minute,
        remark_gap: c.limits.remark_gap,
        bot_remark_gap: c.limits.bot_remark_gap,
    }
}

fn who_of(slot: u8, c: &ClientInfo) -> Who {
    Who {
        slot,
        userid: c.userid,
        name: c.name.clone(),
        bot: c.is_fake || c.is_ours,
    }
}

/// Danger for a bot typing in the open: it drops the chat to fight.
fn typing_danger(bot: &Bot, now: SimTime) -> bool {
    let brain = &bot.brain;
    let since = bot.chat.typing_since();
    brain.beliefs.visible_enemies().next().is_some()
        || brain
            .beliefs
            .enemies_near(bot.self_state.body.origin, DANGER_RANGE, now)
            > 0
        || brain.calm_for(now) < 1.0
        || bot
            .self_state
            .last_damage
            .as_ref()
            .is_some_and(|d| since.is_some_and(|s| d.at >= s))
}

/// While a bot types in the open it stands still: no movement and no buttons. Danger first ends the typing.
pub(crate) fn typing_holds_still(bot: &mut Bot, now: SimTime) -> bool {
    if !bot.chat.typing_in_the_open() {
        return false;
    }
    if typing_danger(bot, now) {
        bot.chat.interrupt();
        bot.nav.reset();
        return false;
    }
    true
}

impl Runtime {
    /// Starts the worker once chat is on, outside a replay; new settings go to it.
    pub(crate) fn chat_sync_backend(&mut self) {
        let c = &self.config.chat;
        if self.chat.backend.live() {
            self.chat.backend.send(Job::Configure(Box::new(c.clone())));
            return;
        }
        if !c.enabled || self.init.sandbox {
            return;
        }
        match worker::WorkerBackend::start(c.clone(), &self.init.install_dir) {
            Ok(w) => self.chat.backend = Box::new(w),
            Err(e) => tracing::error!("chat worker could not start: {e}"),
        }
    }

    /// Chat was switched off: nothing on its way any more.
    pub(crate) fn chat_switched_off(&mut self) {
        for bot in &mut self.bots {
            bot.chat.reset();
        }
        self.chat.pending.clear();
        self.chat.answering.clear();
        self.chat.causes.clear();
    }

    pub(crate) fn chat_map_start(&mut self, map: &str, epoch: u32, late_load: bool) {
        let c = &mut self.chat;
        let seed = lb_core::rng::splitmix64(self.master_seed ^ u64::from(epoch).wrapping_mul(0x9e37_79b9));
        c.journal = Journal::new(map, SimTime::ZERO);
        c.director = Director::new(seed, SimTime::ZERO, &limits(&self.config.chat));
        c.epoch = epoch;
        c.seq = 0;
        c.pending.clear();
        c.answering.clear();
        c.causes.clear();
        c.moments.clear();
        c.levels = [None; SLOTS];
        c.leader = None;
        c.board_at = SimTime::ZERO;
        c.gg_kill = None;
        c.last_level.clear();
        c.match_over = false;
        c.late_load = late_load;
    }

    /// The map ends: its summary goes to the memory; the humans on the server now are not new on the next map.
    pub(crate) fn chat_map_end(&mut self) {
        let present: Vec<i32> = self
            .clients
            .slots
            .iter()
            .filter(|c| c.connected && !c.is_fake)
            .map(|c| c.userid)
            .collect();
        if self.config.chat.enabled
            && self.config.chat.memory.enabled
            && let Some(summary) = self.chat_summary()
        {
            self.chat.backend.send(Job::MapEnd(Box::new(summary)));
        }
        self.chat.keys.retain(|id, _| present.contains(id));
        self.chat.returning = present;
    }

    fn chat_push(&mut self, event: Event) {
        let notable = self.chat.journal.push(self.now, event);
        for n in notable {
            let humans: Vec<i32> = match &n {
                Notable::Nemesis { killer, victim, .. }
                | Notable::Humiliation { killer, victim }
                | Notable::Revenge { killer, victim } => [killer, victim]
                    .into_iter()
                    .filter(|w| !w.bot)
                    .map(|w| w.userid)
                    .collect(),
                Notable::OwnBlast { victim: w, .. }
                | Notable::Multikill { killer: w, .. }
                | Notable::Streak { killer: w, .. }
                | Notable::RageQuit { who: w, .. } => (!w.bot).then_some(w.userid).into_iter().collect(),
            };
            for id in humans {
                if self.chat.moments.len() < MOMENTS {
                    self.chat.moments.push((id, n.clone()));
                }
            }
            self.chat.causes.push(Cause::Notable(n));
        }
    }

    /// A client event; `before` is the slot as it was.
    pub(crate) fn chat_on_client(&mut self, e: &ClientEvent, before: Option<ClientInfo>) {
        if e.is_ours {
            return;
        }
        match e.kind {
            ClientEventKind::PutInServer => {
                let Some(c) = self.clients.get(e.slot).cloned() else {
                    return;
                };
                if c.is_fake {
                    return;
                }
                self.chat.keys.insert(c.userid, player_key(&c.auth_id, &c.name));
                let back = self.chat.returning.contains(&c.userid) || (self.chat.late_load && self.now.secs() < 2.0);
                let who = who_of(e.slot, &c);
                self.chat_push(Event::Join { who: who.clone() });
                if !back {
                    self.chat.causes.push(Cause::Join { who });
                }
            }
            ClientEventKind::Disconnect => {
                if let Some(c) = before.filter(|c| c.in_game && !c.is_fake) {
                    self.chat.returning.retain(|id| *id != c.userid);
                    self.chat_push(Event::Leave {
                        who: who_of(e.slot, &c),
                    });
                }
            }
            ClientEventKind::Info => {
                if let (Some(old), Some(c)) = (before, self.clients.get(e.slot).cloned())
                    && c.in_game
                    && !c.is_fake
                    && old.name != c.name
                    && !old.name.is_empty()
                {
                    self.chat_push(Event::Rename {
                        who: who_of(e.slot, &c),
                        old: old.name,
                    });
                }
            }
            ClientEventKind::Connect | ClientEventKind::ConnectRejected => {}
        }
    }

    /// A line of the kill feed.
    pub(crate) fn chat_on_death(&mut self, killer: u8, victim: u8, weapon: &str) {
        let Some(v) = self.clients.get(victim).map(|c| who_of(victim, c)) else {
            return;
        };
        let k = (killer != 0 && killer != victim)
            .then(|| self.clients.get(killer).map(|c| who_of(killer, c)))
            .flatten();
        if let Some(bot) = self.bots.iter_mut().find(|b| b.id.slot == victim && b.is_active()) {
            bot.chat.on_death(self.now, k.clone());
        }
        let event = match (&k, killer) {
            (Some(k), _) => {
                if weapon.eq_ignore_ascii_case("crowbar")
                    && let Some(board) = self.gungame_board()
                    && board.top() >= 2
                    && board.level(k.slot) == Some(board.top())
                {
                    self.chat.gg_kill = Some((self.now, k.clone()));
                }
                Event::Kill {
                    killer: k.clone(),
                    victim: v,
                    weapon: weapon.to_string(),
                }
            }
            (None, 0) => Event::Died {
                victim: v,
                weapon: weapon.to_string(),
            },
            (None, _) => Event::Suicide {
                victim: v,
                weapon: weapon.to_string(),
            },
        };
        self.chat_push(event);
    }

    /// A human's `say` or `say_team`.
    pub(crate) fn chat_on_say(&mut self, c: &CommandEvent) {
        let team = c.argv.first().map(|a| a.as_slice()) == Some(b"say_team");
        let text = sanitize::player_line(&String::from_utf8_lossy(&c.line));
        if text.is_empty() || sanitize::is_command(&text, &self.config.chat.blocked) {
            return;
        }
        let Some(from) = self.clients.get(c.slot).map(|info| who_of(c.slot, info)) else {
            return;
        };
        let from_team = self.teams().get(c.slot as usize).copied().unwrap_or(0);
        self.chat_push(Event::Chat {
            from: from.clone(),
            text: text.clone(),
            team,
        });
        self.chat.causes.push(Cause::Chat {
            from,
            text,
            team,
            from_team,
        });
    }

    /// The game's intermission: the match is over, the top fragger won it.
    pub(crate) fn chat_on_intermission(&mut self) {
        if self.chat.match_over {
            return;
        }
        let winner = self
            .game
            .scoreboard
            .entries
            .iter()
            .enumerate()
            .filter_map(|(slot, e)| {
                self.clients
                    .get(slot as u8)
                    .filter(|c| c.in_game)
                    .map(|c| (slot as u8, c, e))
            })
            .max_by(|a, b| {
                a.2.frags
                    .cmp(&b.2.frags)
                    .then(b.2.deaths.cmp(&a.2.deaths))
                    .then(b.0.cmp(&a.0))
            })
            .map(|(slot, c, _)| who_of(slot, c));
        self.chat_match_end(winner);
    }

    fn chat_match_end(&mut self, winner: Option<Who>) {
        self.chat.match_over = true;
        self.chat_push(Event::MatchEnd { winner: winner.clone() });
        self.chat.causes.push(Cause::MatchEnd { winner });
    }

    /// GunGame levels and leader once a second; a bot on the last level; a match won (the players frozen after the
    /// leader's last kill).
    fn chat_watch_gungame(&mut self) {
        if self.now.since(self.chat.board_at) < 1.0 && self.now >= self.chat.board_at {
            return;
        }
        self.chat.board_at = self.now;
        let Some(board) = self.gungame_board() else {
            self.chat.levels = [None; SLOTS];
            return;
        };
        for slot in 1..SLOTS {
            let (now, before) = (board.levels[slot], self.chat.levels[slot]);
            if let (Some(level), Some(was)) = (now, before)
                && level > was
                && let Some(who) = self.clients.get(slot as u8).map(|c| who_of(slot as u8, c))
            {
                self.chat_push(Event::Level {
                    who,
                    level: i32::from(level),
                });
            }
        }
        self.chat.levels = board.levels;
        if board.leader != self.chat.leader {
            self.chat.leader = board.leader;
            if let Some(slot) = board.leader
                && board.top() >= 2
                && let Some(who) = self.clients.get(slot).map(|c| who_of(slot, c))
            {
                self.chat_push(Event::Leader { who });
            }
        }
        let mut reached = Vec::new();
        let mut frozen = false;
        for bot in self.bots.iter().filter(|b| b.state == BotState::Alive) {
            let gg = GunGame::new(&board, bot.id.slot, bot.self_state.body.weapons_mask);
            if gg.kit == Kit::Crowbar && !gg.warmup && gg.level >= 2 && !self.chat.last_level.contains(&bot.id.slot) {
                reached.push(bot.id.slot);
            }
            frozen |= bot.self_state.body.flags & FL_FROZEN != 0 && board.top() >= 2;
        }
        for slot in reached {
            self.chat.last_level.push(slot);
            self.chat.causes.push(Cause::LastLevel { bot: slot });
        }
        if let Some((at, winner)) = self.chat.gg_kill.clone()
            && self.now.since(at) <= GG_WIN_WINDOW
            && frozen
            && !self.chat.match_over
        {
            self.chat.gg_kill = None;
            self.chat_match_end(Some(winner));
        }
    }

    fn chat_speakers(&self) -> Vec<Speaker> {
        let teams = self.teams();
        self.bots
            .iter()
            .filter(|b| b.is_active() && b.state != BotState::Connecting)
            .filter_map(|b| {
                let c = self.clients.get(b.id.slot)?;
                Some(Speaker {
                    who: Who {
                        slot: b.id.slot,
                        userid: b.userid,
                        name: c.name.clone(),
                        bot: true,
                    },
                    team: teams.get(b.id.slot as usize).copied().unwrap_or(0),
                    chattiness: b.persona.chat.chattiness,
                    alive: b.state == BotState::Alive,
                    busy: b.chat.busy(),
                    last_remark: b.chat.last_remark,
                })
            })
            .collect()
    }

    /// Whether the bot can type now: a dead or frozen bot always, an alive one in a calm moment.
    fn chat_can(&self, bot: &Bot) -> Can {
        let now = self.now;
        if bot.chat.last_line.is_some_and(|t| now.since(t) < SAY_GAP) {
            return Can::No;
        }
        match bot.state {
            BotState::Dead | BotState::Respawning => Can::Free,
            BotState::Alive => {
                let body = &bot.self_state.body;
                if self.chat.match_over || body.flags & FL_FROZEN != 0 {
                    return Can::Free;
                }
                let busy = bot.order.is_some()
                    || bot.test.is_some()
                    || bot.selftest.is_some()
                    || bot.nav_test.is_some()
                    || self.test_run.is_some();
                let brain = &bot.brain;
                let calm = brain.calm_for(now) >= f64::from(self.config.chat.typing.calm)
                    && brain.beliefs.visible_enemies().next().is_none()
                    && brain.mind.target.is_none()
                    && !brain.mind.firing
                    && brain.mind.arms.active.is_none()
                    && bot
                        .self_state
                        .last_damage
                        .as_ref()
                        .is_none_or(|d| now.since(d.at) > 3.0)
                    && body.flags & FL_ONGROUND != 0
                    && body.movetype != MOVETYPE_FLY
                    && body.waterlevel < 2
                    && matches!(bot.nav.phase(), "idle" | "walk");
                if calm && !busy { Can::Alive } else { Can::No }
            }
            _ => Can::No,
        }
    }

    /// The SDK's `Host_Say` before 2023 drops a line without a printable ASCII character (pure Cyrillic); BugfixedHL,
    /// the 2023 update and hlsdk-portable take UTF-8.
    pub(crate) fn chat_ascii_needed(&self) -> bool {
        self.game.dll.kind == DllKind::Classic
    }

    /// Bytes a line may take for the bot in `slot`: what `Host_Say` leaves after its name.
    pub(crate) fn chat_budget(&self, slot: u8, team: bool) -> usize {
        let name = self.clients.get(slot).map_or("", |c| c.name.as_str());
        sanitize::say_budget(name, team)
    }

    fn chat_request(&self, bot: &Bot, id: u64, trigger: Trigger, team: bool) -> ChatRequest {
        let now = self.now;
        let slot = bot.id.slot;
        let name = self
            .clients
            .get(slot)
            .map_or_else(|| bot.persona.name.clone(), |c| c.name.clone());
        let board = self.gungame_board();
        let teams = self.teams();
        let my_team = teams.get(slot as usize).copied().unwrap_or(0);
        let score = |s: u8| {
            self.game
                .scoreboard
                .entries
                .get(s as usize)
                .map_or((0, 0), |e| (e.frags, e.deaths))
        };
        let level_of = |s: u8| board.as_ref().and_then(|b| b.level(s));
        let mood = &bot.brain.mind.mood;
        let (base_a, base_f) = mood.base();
        let (frags, deaths) = score(slot);
        let level = level_of(slot).map(|l| {
            let weapon = Kit::of(bot.self_state.body.weapons_mask)
                .main()
                .map(|w| w.classname().trim_start_matches("weapon_").to_string())
                .unwrap_or_default();
            (l, weapon)
        });
        let card = BotCard {
            name,
            userid: bot.userid,
            skill: bot.persona.skill,
            style: bot.persona.style.as_str().to_string(),
            favourite_weapons: bot.persona.weapons.clone(),
            profanity: bot.persona.chat.profanity,
            manner_text: bot.persona.chat.style.clone(),
            manner: bot.persona.chat.manner,
            about: bot.persona.chat.about.clone(),
            boldness: (mood.aggression - base_a) - (mood.fear - base_f),
            alive: bot.state == BotState::Alive,
            frags,
            deaths,
            level,
        };
        let players = self
            .clients
            .slots
            .iter()
            .enumerate()
            .filter(|(_, c)| c.connected && c.in_game)
            .map(|(s, c)| {
                let s = s as u8;
                let (frags, deaths) = score(s);
                PlayerCard {
                    name: c.name.clone(),
                    key: (!c.is_fake).then(|| player_key(&c.auth_id, &c.name)),
                    frags,
                    deaths,
                    level: level_of(s),
                    me: s == slot,
                    duel: self.chat.journal.duel(bot.userid, c.userid),
                }
            })
            .collect();
        let leader = match &board {
            Some(b) => b.leader,
            None => self
                .game
                .scoreboard
                .entries
                .iter()
                .enumerate()
                .filter(|(s, _)| self.clients.get(*s as u8).is_some_and(|c| c.in_game))
                .max_by_key(|(_, e)| e.frags)
                .filter(|(_, e)| e.frags > 0)
                .map(|(s, _)| s as u8),
        }
        .and_then(|s| self.clients.get(s))
        .map(|c| c.name.clone());
        let since = SimTime(now.secs() - EVENTS_WINDOW);
        let events = self
            .chat
            .journal
            .entries()
            .filter(|e| e.t >= since)
            .filter(|e| match &e.event {
                Event::Chat { from, team: true, .. } => teams.get(from.slot as usize).copied().unwrap_or(0) == my_team,
                _ => true,
            })
            .map(|e| Recent {
                age: now.since(e.t),
                event: e.event.clone(),
            })
            .collect();
        let language = self.config.chat.language.clone();
        // Cyrillic letters take two bytes.
        let per_char = if Lang::of(&language) == Lang::Ru { 2 } else { 1 };
        ChatRequest {
            id,
            bot: card,
            trigger,
            scene: Scene {
                map: self.chat.journal.map.clone(),
                gungame: board.is_some(),
                teamplay: matches!(self.game.mode, Some(lb_game::mode::GameModeKind::Teamplay)),
                elapsed: now.since(self.chat.journal.started),
                players,
                leader,
            },
            events,
            language,
            max_chars: (self.chat_budget(slot, team) / per_char).clamp(16, 120),
            team,
        }
    }

    /// Asks the worker for a bot's line.
    fn chat_ask(&mut self, s: Speak) {
        let Some(i) = self.bots.iter().position(|b| b.id.slot == s.slot && b.is_active()) else {
            return;
        };
        let id = request_id(self.chat.epoch, self.chat.seq);
        self.chat.seq += 1;
        let request = self.chat_request(&self.bots[i], id, s.trigger.clone(), s.team);
        let read = match &s.trigger {
            Trigger::Addressed { text, .. } | Trigger::Continued { text, .. } | Trigger::Overheard { text, .. } => {
                text.chars().count()
            }
            _ => 0,
        };
        let keep = match s.priority {
            Priority::Answer => KEEP_ANSWER,
            Priority::MatchEnd => KEEP_MATCH_END,
            Priority::Greeting | Priority::Remark => KEEP_REMARK,
        };
        let to = s
            .trigger
            .is_answer()
            .then(|| s.trigger.about().map(|w| w.userid))
            .flatten();
        let now = self.now;
        let bot = &mut self.bots[i];
        let notice = notice_secs(&mut bot.rng.cosmetic, read);
        bot.chat.ask(id, s.priority, s.team, now, notice, keep);
        let (slot, generation, name) = (bot.id.slot, bot.id.generation, bot.persona.name.clone());
        self.chat.pending.retain(|p| p.slot != slot);
        self.chat.answering.retain(|(s, _)| *s != slot);
        self.chat.pending.push(Pending {
            id,
            slot,
            generation,
            to,
        });
        self.chat.counts.asked += 1;
        self.chat
            .log(now, format!("{name} asked ({:?}): {}", s.priority, s.trigger.summary()));
        self.chat.backend.send(Job::Ask(Box::new(request)));
    }

    fn chat_apply(&mut self, reply: Reply) {
        let Some(at) = self.chat.pending.iter().position(|p| p.id == reply.id) else {
            return;
        };
        let p = self.chat.pending.swap_remove(at);
        let Some(i) = self
            .bots
            .iter()
            .position(|b| b.id.slot == p.slot && b.id.generation == p.generation && b.is_active())
        else {
            return;
        };
        let now = self.now;
        let team = self.bots[i].chat.team();
        let budget = self.chat_budget(p.slot, team);
        let ascii = self.chat_ascii_needed();
        let line = match &reply.outcome {
            Outcome::Line(text) => sanitize::fit_say(text, budget, &self.config.chat.blocked, ascii),
            Outcome::Skip | Outcome::Failed(_) => None,
        };
        let bot = &mut self.bots[i];
        let think = line.as_deref().map_or(0.0, |l| think_secs(&mut bot.rng.cosmetic, l));
        let name = bot.persona.name.clone();
        if !bot.chat.answer(reply.id, line.clone(), now, think) {
            return;
        }
        match (&reply.outcome, line) {
            (_, Some(line)) => self.chat.log(now, format!("{name} will type: {line}")),
            (Outcome::Failed(why), None) => {
                self.chat.counts.failed += 1;
                self.chat.log(now, format!("{name}: no line ({why:?})"));
            }
            (_, None) => {
                self.chat.counts.silent += 1;
                self.chat.log(now, format!("{name} keeps quiet"));
            }
        }
        if !matches!(reply.outcome, Outcome::Line(_)) || self.bots[i].chat.busy().is_none() {
            self.chat.director.refund(&limits(&self.config.chat));
        }
        if let Some(to) = p.to
            && self.bots[i].chat.busy().is_some()
        {
            self.chat.pending_to(p.slot, to);
        }
    }

    /// Replies, the director's picks and typing, before the bots act this frame.
    pub(crate) fn chat_tick(&mut self, replies: Vec<Reply>) {
        let enabled = self.config.chat.enabled;
        for reply in replies {
            if enabled {
                self.chat_apply(reply);
            }
        }
        if !enabled || self.map.is_none() {
            self.chat.causes.clear();
            return;
        }
        let now = self.now;
        self.chat_watch_gungame();
        for bot in &mut self.bots {
            if bot.chat.busy().is_none()
                && let Some((t, killer)) = bot.chat.killed_typing.take()
                && now.since(t) <= KILLED_TYPING_WINDOW
            {
                self.chat.causes.push(Cause::KilledWhileTyping {
                    bot: bot.id.slot,
                    killer,
                });
            }
        }
        let causes = std::mem::take(&mut self.chat.causes);
        if !causes.is_empty() {
            let humans = !self.config.chat.require_humans || self.clients.humans(false) > 0;
            let limits = limits(&self.config.chat);
            for cause in causes {
                let speakers = self.chat_speakers();
                let speaks = self
                    .chat
                    .director
                    .react(now, &cause, &speakers, &self.chat.journal, humans, &limits);
                for s in speaks {
                    self.chat_ask(s);
                }
            }
        }
        let range = self.config.chat.typing.cpm;
        for i in 0..self.bots.len() {
            if !self.bots[i].is_active() || self.bots[i].chat.busy().is_none() {
                continue;
            }
            let can = self.chat_can(&self.bots[i]);
            let bot = &mut self.bots[i];
            let cpm = bot.persona.chat.typing_cpm(range);
            let open = bot.chat.typing_in_the_open();
            let rng = &mut bot.rng.cosmetic;
            let Some(line) = bot.chat.tick(now, can, |text| typing_secs(rng, text, cpm)) else {
                continue;
            };
            let team = bot.chat.team();
            let command = if team { "say_team" } else { "say" };
            bot.pending_client_cmds.push(vec![command.to_string(), line.clone()]);
            if open {
                bot.nav.reset();
            }
            let (slot, userid, name) = (bot.id.slot, bot.userid, bot.persona.name.clone());
            let netname = self.clients.get(slot).map_or_else(|| name.clone(), |c| c.name.clone());
            let to = self.chat.talk_to(slot);
            self.chat.director.said(now, slot, to);
            self.chat.counts.said += 1;
            self.chat.log(now, format!("{name}: {line}"));
            tracing::info!("chat {netname}: {line}");
            self.chat_push(Event::Chat {
                from: Who {
                    slot,
                    userid,
                    name: netname,
                    bot: true,
                },
                text: line,
                team,
            });
        }
    }

    /// `lb chat test`: as if `from` wrote `text` to the bot in `slot`.
    pub(crate) fn chat_test(&mut self, slot: u8, from: Who, text: String) {
        let speakers = self.chat_speakers();
        let limits = limits(&self.config.chat);
        let cause = Cause::Test { bot: slot, from, text };
        let speaks = self
            .chat
            .director
            .react(self.now, &cause, &speakers, &self.chat.journal, true, &limits);
        for s in speaks {
            self.chat_ask(s);
        }
    }

    /// `lb chat prompt`: the prompt a line of the bot in `slot` would get, printed by the worker.
    pub(crate) fn chat_preview(&mut self, slot: u8, from: Who, text: String) -> bool {
        let Some(bot) = self.bots.iter().find(|b| b.id.slot == slot && b.is_active()) else {
            return false;
        };
        let request = self.chat_request(bot, 0, Trigger::Addressed { from, text }, false);
        self.chat.backend.send(Job::Preview(Box::new(request)));
        true
    }

    /// The map for the memory: every human who played it, against which bot, what they wrote.
    fn chat_summary(&self) -> Option<MapSummary> {
        let journal = &self.chat.journal;
        let lang = Lang::of(&self.config.chat.language);
        let end = self.now;
        let winner = journal.entries().rev().find_map(|e| match &e.event {
            Event::MatchEnd { winner } => winner.clone(),
            _ => None,
        });
        let scores = journal.scores();
        let bots: Vec<(i32, &lb_chat::journal::Score)> =
            scores.iter().filter(|(_, s)| s.bot).map(|(id, s)| (*id, s)).collect();
        let players: Vec<PlayerMap> = scores
            .iter()
            .filter(|(_, s)| !s.bot)
            .filter_map(|(id, s)| {
                let key = self.chat.keys.get(id)?.clone();
                let vs_bots = bots
                    .iter()
                    .map(|(bot_id, b)| {
                        (
                            b.name.clone(),
                            s.victims.get(bot_id).copied().unwrap_or(0),
                            b.victims.get(id).copied().unwrap_or(0),
                        )
                    })
                    .filter(|(_, a, b)| a + b > 0)
                    .collect();
                let mut weapons: Vec<(String, u32)> = s.weapons.iter().map(|(w, n)| (w.clone(), *n)).collect();
                weapons.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
                let lines = journal
                    .entries()
                    .filter_map(|e| match &e.event {
                        Event::Chat { from, text, .. } if from.userid == *id => Some((end.since(e.t), text.clone())),
                        _ => None,
                    })
                    .collect();
                let moments = self
                    .chat
                    .moments
                    .iter()
                    .filter(|(who, _)| who == id)
                    .map(|(_, n)| lb_chat::prompt::moment(lang, n))
                    .collect();
                Some(PlayerMap {
                    key,
                    name: s.name.clone(),
                    vs_bots,
                    weapons,
                    kills: s.kills,
                    deaths: s.deaths,
                    won: winner.as_ref().is_some_and(|w| w.userid == *id),
                    lines,
                    moments,
                })
            })
            .collect();
        if players.is_empty() {
            return None;
        }
        let mut top: Vec<(String, i32)> = self
            .clients
            .slots
            .iter()
            .enumerate()
            .filter(|(_, c)| c.connected && c.in_game)
            .map(|(s, c)| {
                (
                    c.name.clone(),
                    self.game.scoreboard.entries.get(s).map_or(0, |e| e.frags),
                )
            })
            .collect();
        top.sort_by(|a, b| b.1.cmp(&a.1));
        top.truncate(3);
        let chat = journal
            .entries()
            .filter_map(|e| match &e.event {
                Event::Chat { from, text, .. } => Some((end.since(e.t), from.name.clone(), text.clone(), from.bot)),
                _ => None,
            })
            .collect();
        Some(MapSummary {
            map: journal.map.clone(),
            language: self.config.chat.language.clone(),
            minutes: (end.since(journal.started) / 60.0).round() as u32,
            winner: winner.map(|w| w.name),
            top,
            players,
            chat,
        })
    }

    /// Stops the chat worker before the module goes away: a thread left running would crash the server. The map
    /// under way goes to the memory first.
    pub fn shutdown(&mut self, reason: u32) {
        if self.map.is_some() {
            self.chat_map_end();
        }
        let wait = match reason {
            lb_ffi::LB_SHUTDOWN_DETACH => Duration::from_secs_f32(self.config.chat.provider.timeout + 2.0),
            lb_ffi::LB_SHUTDOWN_PROCESS_EXIT => Duration::from_secs(2),
            _ => Duration::from_millis(500),
        };
        self.chat.backend.shutdown(wait);
    }
}

impl ChatRuntime {
    /// Who the bot in `slot` answers with its line on the way.
    fn pending_to(&mut self, slot: u8, to: i32) {
        self.answering.retain(|(s, _)| *s != slot);
        self.answering.push((slot, to));
    }

    fn talk_to(&mut self, slot: u8) -> Option<i32> {
        let i = self.answering.iter().position(|(s, _)| *s == slot)?;
        Some(self.answering.swap_remove(i).1)
    }
}
