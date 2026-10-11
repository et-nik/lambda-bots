//! Bots in the game chat. The map's journal is kept from the kill feed, the players' chat and the scoreboard; the
//! director picks who says something; requests go to the chat worker, and its replies come back as a recorded
//! outside input, so a replay types the same lines at the same frames. An alive bot types standing still, in a calm
//! moment, or on the run when none comes; a dead one types at once and holds its respawn. The line ends as `say`. A
//! line once asked for is paid for and is said: no other takes its place, and one whose bot is gone (a map change, a
//! bot that left) waits for the personality's bot to come back. Talks, the bots' recent lines and the humans seen
//! lately go on over map changes ([`Social`]); the chat log takes every line and every player coming and going, and is
//! never read.

mod backend;
mod chatlog;
mod command;
mod levels;
mod store;
mod transcript;
mod worker;

use std::collections::{BTreeMap, VecDeque};
use std::time::Duration;

use lb_chat::addressing::{self, Noise};
use lb_chat::botchat::{ASKED_KEEP, CALM_WAIT, notice_secs, think_secs, typing_secs};
use lb_chat::journal::Entry;
use lb_chat::lang::Lang;
use lb_chat::memory::player_key;
use lb_chat::prompt::{self, CHAT_WINDOW, GAME_WINDOW};
use lb_chat::request::{PlayerMap, Recent, request_epoch, request_id};
use lb_chat::{
    BotCard, Can, Carry, Cause, ChatRequest, Director, Event, Journal, Limits, MapSummary, Notable, Outcome, Pending,
    PlayerCard, Priority, Reply, Scene, Social, Speak, Speaker, Trigger, Who, sanitize,
};
use lb_core::time::SimTime;
use lb_game::dll::DllKind;
use lb_game::gungame::{self, GunGame, Kit, SLOTS};
use lb_game::mode::GameModeKind;
use lb_game::self_state::{FL_FROZEN, FL_ONGROUND, MOVETYPE_FLY};
use lb_game::weapons::WeaponId;
use lb_raw::{ClientEvent, ClientEventKind, ClientState, CommandEvent};

#[cfg(test)]
pub use backend::FakeBackend;
pub use backend::{ChatBackend, Job, NullBackend};
pub use chatlog::ChatLog;
pub(crate) use command::command;

use crate::Runtime;
use crate::clients::ClientInfo;
use crate::manager::{Bot, BotState};

/// Seconds a dead bot may keep from respawning to finish its line: about until the game respawns it anyway
/// (`mp_forcerespawn`, 5 s after it could respawn).
pub const RESPAWN_HOLD: f64 = 5.0;
/// Seconds a line whose bot is gone waits for the personality's bot: one is back a few seconds into the next map.
const CARRY_WAIT: f64 = 60.0;
/// Seconds into a map a line carried over waits for the humans it is for to be in the game again.
const READERS_WAIT: f64 = 30.0;
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
    /// The model's lines that came to be typed.
    pub lines: u64,
    /// Ready phrases that came to be typed.
    pub phrases: u64,
    pub said: u64,
    pub silent: u64,
    pub failed: u64,
    /// The model's lines that came and were never said: chat switched off, a bot that never came back.
    pub lost: u64,
}

pub struct ChatRuntime {
    pub journal: Journal,
    director: Director,
    /// What the chat keeps from map to map: talks, the bots' recent lines, the humans lately, the hour's remarks.
    pub social: Social,
    pub backend: Box<dyn ChatBackend>,
    /// `logs/chatlog.<date>.log`; none in a replay or a test.
    pub chatlog: Option<ChatLog>,
    epoch: u32,
    seq: u32,
    /// Lines asked for whose reply has not come.
    pending: Vec<Pending>,
    /// Lines on their way, their text known.
    answering: Vec<Pending>,
    /// Lines whose bot is gone (the map changed, it left), with since when: each waits [`CARRY_WAIT`] at most for the
    /// personality's bot to be back and say it.
    carried: Vec<(SimTime, Pending)>,
    causes: Vec<Cause>,
    /// Humans of this map by `userid`, with the keys the memory knows them by.
    keys: BTreeMap<i32, String>,
    /// Humans who were in the game when the last map ended: they are not new on this one.
    returning: Vec<i32>,
    /// Moments of this map with humans in them, for the memory.
    moments: Vec<(i32, Notable)>,
    levels: [Option<i16>; SLOTS],
    leader: Option<u8>,
    board_at: SimTime,
    /// GunGame: the last level's kill that may have won the match.
    gg_kill: Option<(SimTime, Who)>,
    /// GunGame: the plugin's warmup, as the players alive last showed it ([`gungame::warmup_seen`]).
    warmup: bool,
    /// GunGame: the last level (from 0), known once the match is won: the plugin puts the winner one past it.
    gg_last: Option<i32>,
    /// Bots seen with a level's kit on this map ([`Kit::armed`]): the crowbar alone of one never armed is no last level
    /// (the warmup's, which `gg_warmup 0` leaves until the bot's next spawn; a team's newcomer).
    armed: Vec<u8>,
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
            remarks_per_hour: 0.0,
            language: String::new(),
        };
        ChatRuntime {
            journal: Journal::new("", SimTime::ZERO),
            director: Director::new(0, SimTime::ZERO, &limits),
            social: Social::default(),
            backend: Box::new(NullBackend),
            chatlog: None,
            epoch: 0,
            seq: 0,
            pending: Vec::new(),
            answering: Vec::new(),
            carried: Vec::new(),
            causes: Vec::new(),
            keys: BTreeMap::new(),
            returning: Vec::new(),
            moments: Vec::new(),
            levels: [None; SLOTS],
            leader: None,
            board_at: SimTime::ZERO,
            gg_kill: None,
            warmup: false,
            gg_last: None,
            armed: Vec::new(),
            last_level: Vec::new(),
            match_over: false,
            late_load: false,
            log: VecDeque::new(),
            counts: Counts::default(),
        }
    }

    /// What carries over a map change: taken after the map's end, whose times it is already moved back by.
    pub fn carry(&self) -> Carry {
        Carry {
            humans: self.returning.clone(),
            social: self.social.clone(),
            lines: self.carried.clone(),
        }
    }

    pub fn restore(&mut self, carry: Carry) {
        self.returning = carry.humans;
        self.social = carry.social;
        self.carried = carry.lines;
    }

    fn log(&mut self, now: SimTime, line: String) {
        tracing::debug!("chat: {line}");
        self.log.push_back(format!("[{:7.1}] {line}", now.secs()));
        while self.log.len() > LOG_LINES {
            self.log.pop_front();
        }
    }

    /// A line that will not be said: one nobody asked for gives its remark back, and what asking for it marked is
    /// taken back, the player's line answered by the bot and the player greeted. The model's line was paid for: the
    /// log tells `why` it is not said.
    fn unsaid(&mut self, p: &Pending, why: &str) {
        match &p.text {
            Some(text) if p.phrase => tracing::debug!("chat {}: phrase not said ({why}): {text}", p.persona),
            Some(text) => {
                self.counts.lost += 1;
                tracing::info!("chat {}: not said ({why}): {text}", p.persona);
            }
            None => {}
        }
        if p.unasked {
            self.social.remarks.give();
        }
        let Some(to) = &p.to else {
            return;
        };
        if let Some(at) = p.line_at {
            self.social.unanswered(&to.name, at, &p.persona);
        }
        if let Some(before) = p.greeted {
            self.social.ungreet(&to.name, before);
        }
    }

    /// The lines asked for or on their way that `which` picks will not be said.
    fn unsay(&mut self, why: &str, which: impl Fn(&Pending) -> bool) {
        let mut gone = Vec::new();
        for lines in [&mut self.pending, &mut self.answering] {
            let (out, kept): (Vec<Pending>, Vec<Pending>) = std::mem::take(lines).into_iter().partition(|p| which(p));
            *lines = kept;
            gone.extend(out);
        }
        for p in &gone {
            self.unsaid(p, why);
        }
    }

    /// The lines asked for or on their way that `which` picks have lost their bot: they wait for the personality's
    /// bot to come back. A ready phrase does not: it cost nothing, and its moment is gone.
    fn carry_away(&mut self, now: SimTime, which: impl Fn(&Pending) -> bool) {
        let mut phrases = Vec::new();
        for lines in [&mut self.pending, &mut self.answering] {
            let (gone, kept): (Vec<Pending>, Vec<Pending>) = std::mem::take(lines).into_iter().partition(|p| which(p));
            *lines = kept;
            for p in gone {
                if p.phrase {
                    phrases.push(p);
                } else {
                    self.carried.push((now, p));
                }
            }
        }
        for p in &phrases {
            self.unsaid(p, "its bot is gone");
        }
    }

    /// A reply no line waits for any more (`why`): the model's line in it was paid for.
    pub(crate) fn lost_reply(&mut self, reply: &Reply, why: &str) {
        match &reply.outcome {
            Outcome::Line(text) => {
                self.counts.lost += 1;
                tracing::info!("chat: not said ({why}): {text}");
            }
            Outcome::Phrase(text) => tracing::debug!("chat: phrase not said ({why}): {text}"),
            Outcome::Skip | Outcome::Failed(_) => {}
        }
    }

    /// The personalities whose bots are gone with a line waiting for them, the longest waiting first.
    pub(crate) fn carried_personas(&self) -> impl Iterator<Item = &str> {
        self.carried.iter().map(|(_, p)| p.persona.as_str())
    }

    /// The line on its way the bot in `slot` has just said.
    fn line_said(&mut self, slot: u8) -> Option<Pending> {
        let i = self.answering.iter().position(|p| p.slot == slot)?;
        Some(self.answering.swap_remove(i))
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
        remarks_per_hour: c.limits.remarks_per_hour,
        language: c.language.clone(),
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

/// Danger for a bot typing in the open: it stops standing to fight.
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

/// While a bot types in the open it stands still: no movement and no buttons. Danger first ends the standing: it
/// finishes the line on the run.
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
        match worker::WorkerBackend::start(c.clone(), &self.init.install_dir, &self.init.game_dir) {
            Ok(w) => self.chat.backend = Box::new(w),
            Err(e) => tracing::error!("chat worker could not start: {e}"),
        }
    }

    /// `lb config reload`: chat on or off as the config says now; a worker already running takes the settings, reads
    /// `config/chat/` again and tries the provider at once. The chat log lets its file go, to open it with the next
    /// line.
    pub(crate) fn chat_reload(&mut self) {
        if !self.config.chat.enabled {
            self.chat_switched_off();
        }
        let live = self.chat.backend.live();
        self.chat_sync_backend();
        if live {
            self.chat.backend.send(Job::Reload);
        }
        if let Some(log) = self.chat.chatlog.as_mut() {
            log.close();
        }
    }

    /// Chat was switched off: nothing on its way any more, nothing waiting for its bot.
    pub(crate) fn chat_switched_off(&mut self) {
        for bot in &mut self.bots {
            bot.chat.reset();
        }
        let why = "chat switched off";
        self.chat.unsay(why, |_| true);
        for (_, p) in std::mem::take(&mut self.chat.carried) {
            self.chat.unsaid(&p, why);
        }
        self.chat.causes.clear();
        self.chat.director.clear_waiting();
    }

    pub(crate) fn chat_map_start(&mut self, map: &str, epoch: u32, late_load: bool) {
        let c = &mut self.chat;
        let seed = lb_core::rng::splitmix64(self.master_seed ^ u64::from(epoch).wrapping_mul(0x9e37_79b9));
        c.journal = Journal::new(map, SimTime::ZERO);
        c.director = Director::new(seed, SimTime::ZERO, &limits(&self.config.chat));
        c.social.map_start();
        if let Some(log) = c.chatlog.as_mut() {
            log.map(map);
            log.prune_daily(self.config.chat.chatlog_days);
        }
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
        c.warmup = false;
        c.gg_last = None;
        c.armed.clear();
        c.last_level.clear();
        c.match_over = false;
        c.late_load = late_load;
    }

    /// The map ends: its summary goes to the memory, the new lines of the talks with it; the humans in the game now
    /// are not new on the next map (one still connecting is), the lines asked for or on their way wait for their
    /// bots to be back on it, and what the chat keeps goes on with those on the server, its times moved back by the
    /// map's. All of it before a recording of the next map takes what carries over.
    pub(crate) fn chat_map_end(&mut self) {
        let humans: Vec<&ClientInfo> = self
            .clients
            .slots
            .iter()
            .filter(|c| c.connected && !c.is_fake)
            .collect();
        let present: Vec<i32> = humans.iter().map(|c| c.userid).collect();
        let in_game = || humans.iter().filter(|c| c.in_game);
        let names: Vec<String> = in_game().map(|c| c.name.clone()).collect();
        let returning: Vec<i32> = in_game().map(|c| c.userid).collect();
        if self.config.chat.enabled
            && self.config.chat.memory.enabled
            && let Some(summary) = self.chat_summary()
        {
            self.chat.backend.send(Job::MapEnd(Box::new(summary)));
            self.chat.social.mark_saved();
        }
        let end = self.now;
        self.chat.carry_away(end, |_| true);
        for (since, p) in &mut self.chat.carried {
            // The personality comes back only on the next map: its wait starts there.
            *since = SimTime::ZERO;
            p.map_end(end);
        }
        self.chat.keys.retain(|id, _| present.contains(id));
        self.chat.social.present(&names, end);
        self.chat.social.map_end(end, &present);
        self.chat.returning = returning;
    }

    fn chat_push(&mut self, event: Event) {
        self.chat.journal.gungame = matches!(self.game.mode, Some(GameModeKind::GunGame { .. }));
        let notable = self.chat.journal.push(self.now, event);
        for n in notable {
            let humans: Vec<i32> = n.people().into_iter().filter(|w| !w.bot).map(|w| w.userid).collect();
            for id in humans {
                if self.chat.moments.len() < MOMENTS {
                    self.chat.moments.push((id, n.clone()));
                }
            }
            self.chat.causes.push(Cause::Notable(n));
        }
    }

    /// Whether a human put in the server at `at` is back from the map change, not new: in the game when the last map
    /// ended, or already there when the plugin was loaded into a game under way.
    fn chat_back(&self, userid: i32, at: SimTime) -> bool {
        self.chat.returning.contains(&userid) || (self.chat.late_load && at.secs() < 2.0)
    }

    /// Writes to the chat log, when there is one and `chat.chatlog` is on.
    fn chatlog(&mut self, write: impl FnOnce(&mut ChatLog, u32)) {
        let c = &self.config.chat;
        if c.chatlog
            && let Some(log) = self.chat.chatlog.as_mut()
        {
            write(log, c.chatlog_days);
        }
    }

    /// A client event; `before` is the slot as it was. Who is on the server is kept with chat off too.
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
                let back = self.chat_back(c.userid, self.now);
                let arrived = self.chat.social.arrive(&c.name, self.now);
                if !back {
                    self.chatlog(|log, days| log.join(days, &c.name));
                }
                let who = who_of(e.slot, &c);
                self.chat_push(Event::Join { who: who.clone() });
                if !back && arrived {
                    self.chat.causes.push(Cause::Join { who });
                }
            }
            ClientEventKind::Disconnect => {
                if let Some(c) = before.filter(|c| c.in_game && !c.is_fake) {
                    self.chat.social.left(&c.name, self.now);
                    self.chatlog(|log, days| log.leave(days, &c.name));
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
                    self.chat.social.left(&old.name, self.now);
                    self.chat.social.arrive(&c.name, self.now);
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

    /// A human's `say` or `say_team`: to the chat log as typed, plugin commands too, a password masked; the rest is
    /// chat. A login or registration line ([`sanitize::secret`]) is in the log alone, and so is a team line outside the
    /// modes with teams, which only its writer sees (the SDK's `Host_Say`).
    pub(crate) fn chat_on_say(&mut self, c: &CommandEvent) {
        let team = c.argv.first().map(|a| a.as_slice()) == Some(b"say_team");
        let text = sanitize::player_line(&String::from_utf8_lossy(&c.line));
        let Some(from) = self.clients.get(c.slot).map(|info| who_of(c.slot, info)) else {
            return;
        };
        if text.is_empty() {
            return;
        }
        self.chatlog(|log, days| log.say(days, &from.name, false, team, &text));
        if sanitize::is_command(&text, &self.config.chat.blocked)
            || sanitize::secret(&text)
            || (team && !self.team_mode())
        {
            return;
        }
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

    /// GunGame levels and leader once a second (none in the warmup, whose frags are kills); a bot on the last level; a
    /// match won (the players frozen after the leader's last kill).
    fn chat_watch_gungame(&mut self) {
        if self.now.since(self.chat.board_at) < 1.0 && self.now >= self.chat.board_at {
            return;
        }
        self.chat.board_at = self.now;
        let Some(board) = self.gungame_board() else {
            self.chat.levels = [None; SLOTS];
            return;
        };
        let kits: Vec<(u8, i32, Kit)> = self
            .bots
            .iter()
            .filter(|b| b.state == BotState::Alive)
            .map(|b| {
                let gg = GunGame::new(&board, b.id.slot, b.self_state.body.weapons_mask);
                (b.id.slot, gg.level, gg.kit)
            })
            .collect();
        // The plugin's end of the match freezes every player, the dead too: the winning kill may leave no bot alive.
        let frozen = board.top() >= 2
            && self.bots.iter().any(|b| {
                matches!(b.state, BotState::Alive | BotState::Dead | BotState::Respawning)
                    && b.self_state.body.flags & FL_FROZEN != 0
            });
        let held = self
            .clients_now
            .iter()
            .filter(|c| c.state == ClientState::Spawned && c.deadflag == 0)
            .map(|c| WeaponId::from_player_model(&self.strings.string_lossy(c.weaponmodel)));
        // The warmup starts with everyone's frags at nought: a bot with the crowbar alone on the first level next to
        // players levels up is a team's newcomer, its frags unset until the team's next level.
        if let Some(seen) = gungame::warmup_seen(kits.iter().map(|&(_, level, kit)| (level, kit)), held)
            && !(seen && board.top() >= 2)
        {
            self.chat.warmup = seen;
        }
        let warmup = self.chat.warmup;
        // The warmup ends with everyone frozen for a moment too.
        let won = self
            .chat
            .gg_kill
            .clone()
            .filter(|(at, _)| !warmup && self.now.since(*at) <= GG_WIN_WINDOW && frozen && !self.chat.match_over)
            .map(|(_, winner)| winner);
        if let Some(winner) = &won {
            self.chat.gg_last = board.level(winner.slot).map(|l| l - 1);
        }
        for slot in 1..SLOTS {
            let (now, before) = (board.levels[slot], self.chat.levels[slot]);
            if let (Some(level), Some(was)) = (now, before)
                && level > was
                && !warmup
                && won.as_ref().is_none_or(|w| usize::from(w.slot) != slot)
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
                && !warmup
                && let Some(who) = self.clients.get(slot).map(|c| who_of(slot, c))
            {
                self.chat_push(Event::Leader { who });
            }
        }
        for (slot, level, kit) in kits {
            if kit.armed() && !self.chat.armed.contains(&slot) {
                self.chat.armed.push(slot);
            }
            if kit == Kit::Crowbar
                && !warmup
                && level >= 2
                && self.chat.armed.contains(&slot)
                && !self.chat.last_level.contains(&slot)
            {
                self.chat.last_level.push(slot);
                self.chat.causes.push(Cause::LastLevel { bot: slot });
            }
        }
        if let Some(winner) = won {
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
                    persona: b.persona.name.clone(),
                    calls: b.persona.chat.call.clone(),
                    team: teams.get(b.id.slot as usize).copied().unwrap_or(0),
                    chattiness: b.persona.chat.chattiness,
                    alive: b.state == BotState::Alive,
                    busy: b.chat.busy().or_else(|| {
                        self.chat
                            .carried
                            .iter()
                            .find(|(_, p)| p.persona == b.persona.name)
                            .map(|(_, p)| p.priority)
                    }),
                    last_remark: b.chat.last_remark,
                })
            })
            .collect()
    }

    /// Whether the bot can type now: a dead or frozen bot always, an alive one in a calm moment (else on the run, once
    /// its line waited for one), none right after its last line.
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
                if calm && !busy { Can::Alive } else { Can::Busy }
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
        let level_of = |s: u8| {
            let level = board.as_ref().and_then(|b| b.level(s))?;
            Some(self.chat.gg_last.map_or(level, |last| level.min(last)))
        };
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
            persona: bot.persona.name.clone(),
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
        let recent = |e: &Entry| Recent {
            age: now.since(e.t),
            event: e.event.clone(),
        };
        let journal = &self.chat.journal;
        let events = journal
            .entries()
            .filter(|e| now.since(e.t) <= GAME_WINDOW)
            .filter(|e| match &e.event {
                Event::Chat { .. } => false,
                Event::Join { who } => !self.chat_back(who.userid, e.t),
                _ => true,
            })
            .map(recent)
            .collect();
        let team_mode = self.team_mode();
        let chat = journal
            .entries()
            .filter(|e| now.since(e.t) <= CHAT_WINDOW)
            .filter(|e| match &e.event {
                Event::Chat { from, team: true, .. } => {
                    team_mode && teams.get(from.slot as usize).copied().unwrap_or(0) == my_team
                }
                Event::Chat { .. } => true,
                _ => false,
            })
            .map(recent)
            .collect();
        let language = self.config.chat.language.clone();
        // Cyrillic letters take two bytes.
        let per_char = if Lang::of(&language) == Lang::Ru { 2 } else { 1 };
        let mut req = ChatRequest {
            id,
            bot: card,
            trigger,
            scene: Scene {
                map: journal.map.clone(),
                gungame: board.is_some(),
                warmup: board.is_some() && self.chat.warmup,
                teamplay: matches!(self.game.mode, Some(GameModeKind::Teamplay)),
                elapsed: now.since(journal.started),
                players,
                leader,
            },
            events,
            chat,
            own: self.chat.social.talks.own(now, &bot.persona.name),
            talk: Vec::new(),
            language,
            max_chars: (self.chat_budget(slot, team) / per_char).clamp(16, 120),
            team,
        };
        if let Some(player) = prompt::partner(&req).map(|w| w.userid) {
            req.talk = self.chat.social.talks.talk(now, &bot.persona.name, player);
        }
        req
    }

    /// Asks the worker for a bot's line. A bot with a line on its way is asked nothing: that line is paid for, and
    /// what asking marked is taken back.
    fn chat_ask(&mut self, s: Speak) {
        let Some(i) = self.bots.iter().position(|b| b.id.slot == s.slot && b.is_active()) else {
            if s.unasked {
                self.chat.social.remarks.give();
            }
            return;
        };
        let id = request_id(self.chat.epoch, self.chat.seq);
        let bot = &self.bots[i];
        let unheard = match &s.trigger {
            Trigger::Overheard { text, .. }
            | Trigger::Greeted { text, .. }
            | Trigger::Question { text, to_me: false, .. } => Some(text.clone()),
            _ => None,
        };
        let pending = Pending {
            id,
            slot: bot.id.slot,
            generation: bot.id.generation,
            persona: bot.persona.name.clone(),
            priority: s.priority,
            team: s.team,
            asked: self.now,
            text: None,
            phrase: false,
            to: s.trigger.to().cloned(),
            line_at: s.line_at,
            unheard,
            greeting: matches!(s.trigger, Trigger::Joined { .. } | Trigger::Greeted { .. }),
            greeted: s.greeted,
            unasked: s.unasked,
            remark: s.priority == Priority::Remark || matches!(s.trigger, Trigger::Overheard { .. }),
        };
        if bot.chat.busy().is_some() {
            self.chat.unsaid(&pending, "a line on its way");
            return;
        }
        self.chat.seq += 1;
        let request = self.chat_request(&self.bots[i], id, s.trigger.clone(), s.team);
        let read = s.trigger.line().map_or(0, |(_, text)| text.chars().count());
        let now = self.now;
        let bot = &mut self.bots[i];
        let notice = notice_secs(&mut bot.rng.cosmetic, read);
        bot.chat.ask(id, s.priority, s.team, now, now, notice);
        let name = pending.persona.clone();
        self.chat.pending.push(pending);
        self.chat.counts.asked += 1;
        self.chat
            .log(now, format!("{name} asked ({:?}): {}", s.priority, s.trigger.summary()));
        self.chat.backend.send(Job::Ask(Box::new(request)));
    }

    /// The worker's reply: the model's line or a ready phrase to type, or nothing. A request that brought no line to
    /// type gives its line back to the director, a phrase its request too (it asked nothing of the model); one asked
    /// on the map before gives nothing back to this map's director, and its phrase is not said. The reply to a line
    /// whose bot is gone stays with the line ([`Runtime::chat_reply_gone`]); one no line waits for any more is told in
    /// the log.
    fn chat_apply(&mut self, reply: Reply) {
        if let Some(at) = self.chat.carried.iter().position(|(_, p)| p.id == reply.id) {
            self.chat_reply_gone(at, reply.outcome);
            return;
        }
        let Some(at) = self.chat.pending.iter().position(|p| p.id == reply.id) else {
            self.chat.lost_reply(&reply, "no line waits for it");
            return;
        };
        let mut p = self.chat.pending.swap_remove(at);
        let Some(i) = self
            .bots
            .iter()
            .position(|b| b.id.slot == p.slot && b.id.generation == p.generation && b.is_active())
        else {
            self.chat.carried.push((self.now, p));
            self.chat_reply_gone(self.chat.carried.len() - 1, reply.outcome);
            return;
        };
        let now = self.now;
        let team = self.bots[i].chat.team();
        let budget = self.chat_budget(p.slot, team);
        let ascii = self.chat_ascii_needed();
        let text = match &reply.outcome {
            Outcome::Line(text) | Outcome::Phrase(text) => Some(text.clone()),
            Outcome::Skip | Outcome::Failed(_) => None,
        };
        let phrase = matches!(reply.outcome, Outcome::Phrase(_));
        let ours = request_epoch(reply.id) == self.chat.epoch;
        let line = text
            .as_deref()
            .filter(|_| ours || !phrase)
            .and_then(|text| sanitize::fit_say(text, budget, &self.config.chat.blocked, ascii));
        p.phrase = phrase;
        let bot = &mut self.bots[i];
        let think = line.as_deref().map_or(0.0, |l| think_secs(&mut bot.rng.cosmetic, l));
        let name = bot.persona.name.clone();
        if !bot.chat.answer(reply.id, line.clone(), now, think) {
            p.text = text;
            self.chat.unsaid(&p, "the bot no longer waited for it");
            return;
        }
        match (&reply.outcome, &line) {
            (_, Some(line)) if phrase => {
                self.chat.counts.phrases += 1;
                self.chat.log(now, format!("{name} will type a phrase: {line}"));
            }
            (_, Some(line)) => {
                self.chat.counts.lines += 1;
                self.chat.log(now, format!("{name} will type: {line}"));
            }
            (Outcome::Failed(why), None) => {
                self.chat.counts.failed += 1;
                self.chat.log(now, format!("{name}: no line ({why:?})"));
            }
            (Outcome::Skip, None) => {
                self.chat.counts.silent += 1;
                self.chat.log(now, format!("{name} keeps quiet"));
            }
            (_, None) if !ours => self.chat.log(now, format!("{name}: a phrase of the map before")),
            (_, None) => self.chat.log(now, format!("{name}: the line cannot be sent")),
        }
        let limits = limits(&self.config.chat);
        if phrase && ours {
            self.chat.director.refund_request(&limits);
        }
        match line {
            Some(line) => {
                p.text = Some(line);
                self.chat.answering.push(p);
            }
            None => {
                if ours {
                    self.chat.director.refund(&limits);
                }
                let why = if ours {
                    "it cannot be sent as a chat line"
                } else {
                    "its moment is gone"
                };
                p.text = text;
                self.chat.unsaid(&p, why);
            }
        }
    }

    /// The reply to the line `carried[at]`, whose bot is gone: the model's line waits there with it; a phrase (it cost
    /// nothing, and its moment is gone) or nothing to say ends it, giving nothing back to the director's buckets.
    fn chat_reply_gone(&mut self, at: usize, outcome: Outcome) {
        let now = self.now;
        let name = self.chat.carried[at].1.persona.clone();
        let why = match outcome {
            Outcome::Line(text) => {
                self.chat.counts.lines += 1;
                self.chat
                    .log(now, format!("{name} will type once back in the game: {text}"));
                self.chat.carried[at].1.text = Some(text);
                return;
            }
            Outcome::Phrase(text) => {
                let p = &mut self.chat.carried[at].1;
                (p.text, p.phrase) = (Some(text), true);
                "its moment is gone"
            }
            Outcome::Failed(why) => {
                self.chat.counts.failed += 1;
                self.chat.log(now, format!("{name}: no line ({why:?})"));
                "nothing to say"
            }
            Outcome::Skip => {
                self.chat.counts.silent += 1;
                self.chat.log(now, format!("{name} keeps quiet"));
                "nothing to say"
            }
        };
        let (_, p) = self.chat.carried.remove(at);
        self.chat.unsaid(&p, why);
    }

    /// Whether the humans a line carried over is for are in the game to read it: the player it answers, else everyone
    /// in the game when the last map ended. Past [`READERS_WAIT`] into the map, whoever is there reads it.
    fn chat_readers_in(&self, p: &Pending) -> bool {
        let in_game = |userid: i32| {
            self.clients
                .find_userid(userid)
                .and_then(|slot| self.clients.get(slot))
                .is_some_and(|c| c.in_game)
        };
        self.now.secs() >= READERS_WAIT
            || match &p.to {
                Some(w) if !w.bot => in_game(w.userid),
                _ => self.chat.returning.iter().all(|&id| in_game(id)),
            }
    }

    /// Lines whose bot is gone go back to the personality's bot once it is in the game with nothing on its way and
    /// the humans the line is for are there too, a line to type after a moment to notice it. One whose bot is not back
    /// [`CARRY_WAIT`] after it went (after the map start, for a line carried over), or a reply waited for longer than
    /// [`ASKED_KEEP`], is not said.
    fn chat_bring_back(&mut self) {
        let now = self.now;
        let ascii = self.chat_ascii_needed();
        let team_mode = self.team_mode();
        for (since, mut p) in std::mem::take(&mut self.chat.carried) {
            if p.text.is_none() && now.since(p.asked) > ASKED_KEEP {
                self.chat.unsaid(&p, "no reply came");
                continue;
            }
            let back = self
                .bots
                .iter()
                .position(|b| b.persona.name == p.persona && b.is_active() && b.state != BotState::Connecting);
            let Some(i) = back else {
                if now.since(since) > CARRY_WAIT {
                    self.chat.unsaid(&p, "its bot did not come back");
                } else {
                    self.chat.carried.push((since, p));
                }
                continue;
            };
            if self.bots[i].chat.busy().is_some() || !self.chat_readers_in(&p) {
                self.chat.carried.push((since, p));
                continue;
            }
            let team = p.team && team_mode;
            let line = p.text.as_deref().map(|text| {
                let budget = self.chat_budget(self.bots[i].id.slot, team);
                sanitize::fit_say(text, budget, &self.config.chat.blocked, ascii)
            });
            let bot = &mut self.bots[i];
            (p.slot, p.generation, p.team) = (bot.id.slot, bot.id.generation, team);
            let notice = notice_secs(&mut bot.rng.cosmetic, 0);
            match line {
                None => {
                    bot.chat.ask(p.id, p.priority, team, p.asked, now, notice);
                    self.chat
                        .log(now, format!("{} is back, waiting for its line", p.persona));
                    self.chat.pending.push(p);
                }
                Some(Some(line)) => {
                    bot.chat.set_line(line.clone(), p.priority, team, now + notice);
                    self.chat
                        .log(now, format!("{} is back and will type: {line}", p.persona));
                    p.text = Some(line);
                    self.chat.answering.push(p);
                }
                Some(None) => self.chat.unsaid(&p, "it cannot be sent as a chat line"),
            }
        }
    }

    /// Replies, the director's picks and typing, before the bots act this frame.
    pub(crate) fn chat_tick(&mut self, replies: Vec<Reply>) {
        let enabled = self.config.chat.enabled;
        for reply in replies {
            if enabled {
                self.chat_apply(reply);
            } else {
                self.chat.lost_reply(&reply, "chat is off");
            }
        }
        if !enabled || self.map.is_none() {
            self.chat.causes.clear();
            return;
        }
        let now = self.now;
        self.chat_bring_back();
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
        if self.chat.director.has_waiting() || !causes.is_empty() {
            let humans = !self.config.chat.require_humans || self.clients.humans(false) > 0;
            let limits = limits(&self.config.chat);
            if self.chat.director.has_waiting() {
                let speakers = self.chat_speakers();
                let speaks = self
                    .chat
                    .director
                    .ready(now, &speakers, &mut self.chat.social, humans, &limits);
                for s in speaks {
                    self.chat_ask(s);
                }
            }
            for cause in causes {
                let speakers = self.chat_speakers();
                let speaks = self.chat.director.react(
                    now,
                    &cause,
                    &speakers,
                    &self.chat.journal,
                    &mut self.chat.social,
                    humans,
                    &limits,
                );
                for s in speaks {
                    self.chat_ask(s);
                }
            }
        }
        let range = self.config.chat.typing.cpm;
        let team_mode = self.team_mode();
        for i in 0..self.bots.len() {
            if !self.bots[i].is_active() || self.bots[i].chat.busy().is_none() {
                continue;
            }
            let can = self.chat_can(&self.bots[i]);
            // A player waits for this bot's answer: its line goes out now, typed on the run if need be.
            let calm_wait = if self.chat.director.waits_for(self.bots[i].id.slot) {
                0.0
            } else {
                CALM_WAIT
            };
            let bot = &mut self.bots[i];
            let cpm = bot.persona.chat.typing_cpm(range);
            let open = bot.chat.typing_in_the_open();
            let rng = &mut bot.rng.cosmetic;
            let Some(line) = bot.chat.tick(now, can, calm_wait, |text| typing_secs(rng, text, cpm)) else {
                continue;
            };
            // A team line on a map without teams would reach nobody.
            let team = bot.chat.team() && team_mode;
            let command = if team { "say_team" } else { "say" };
            bot.pending_client_cmds.push(vec![command.to_string(), line.clone()]);
            if open {
                bot.nav.reset();
            }
            let (slot, userid, name) = (bot.id.slot, bot.userid, bot.persona.name.clone());
            let netname = self.clients.get(slot).map_or_else(|| name.clone(), |c| c.name.clone());
            let asked = self.chat.line_said(slot);
            if let Some(p) = asked.as_ref().filter(|p| request_epoch(p.id) != self.chat.epoch) {
                self.chat.director.charge(now, p.remark, &limits(&self.config.chat));
            }
            let to = asked
                .as_ref()
                .and_then(|p| p.to.as_ref())
                .filter(|w| self.clients.find_userid(w.userid).is_some());
            let greeting = asked.as_ref().is_some_and(|p| p.greeting);
            if let (Some(w), Some(p)) = (to, &asked)
                && let (Some(at), Some(text)) = (p.line_at, &p.unheard)
            {
                self.chat.social.answering(at, &name, w, text);
            }
            self.chat
                .social
                .said(now, &name, to.map(|w| (w.userid, w.name.as_str())), greeting, &line);
            self.chat.counts.said += 1;
            self.chat.log(now, format!("{name}: {line}"));
            tracing::info!("chat {netname}: {line}");
            self.chatlog(|log, days| log.say(days, &netname, true, team, &line));
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
        let bots = &self.bots;
        let seat = |p: &Pending| {
            bots.iter()
                .find(|b| b.id.slot == p.slot && b.id.generation == p.generation && b.is_active())
        };
        self.chat.carry_away(now, |p| seat(p).is_none());
        self.chat
            .unsay("no reply came", |p| seat(p).is_some_and(|b| b.chat.busy().is_none()));
    }

    /// `lb chat test`: as if `from` wrote `text` to the bot in `slot`.
    pub(crate) fn chat_test(&mut self, slot: u8, from: Who, text: String) {
        let speakers = self.chat_speakers();
        let limits = limits(&self.config.chat);
        let cause = Cause::Test { bot: slot, from, text };
        let speaks = self.chat.director.react(
            self.now,
            &cause,
            &speakers,
            &self.chat.journal,
            &mut self.chat.social,
            true,
            &limits,
        );
        for s in speaks {
            self.chat_ask(s);
        }
    }

    /// `lb chat event`: the bot in `slot` speaks of a moment of the game as if it came, past the director.
    pub(crate) fn chat_event(&mut self, slot: u8, trigger: Trigger) {
        self.chat_ask(Speak {
            slot,
            trigger,
            priority: Priority::Remark,
            team: false,
            unasked: false,
            line_at: None,
            greeted: None,
        });
    }

    /// `lb chat prompt`: the prompt a line of the bot in `slot` would get, printed by the worker.
    pub(crate) fn chat_preview(&mut self, slot: u8, from: Who, text: String) -> bool {
        let Some(bot) = self.bots.iter().find(|b| b.id.slot == slot && b.is_active()) else {
            return false;
        };
        let id = request_id(self.chat.epoch, 0);
        let request = self.chat_request(bot, id, Trigger::Addressed { from, text }, false);
        self.chat.backend.send(Job::Preview(Box::new(request)));
        true
    }

    /// The map for the memory: every human who played it, against which bot, what they wrote (noise left out), the
    /// lines of their talks with the bots the memory has not had.
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
                        Event::Chat { from, text, .. }
                            if from.userid == *id && !addressing::noise(text).is_some_and(Noise::hard) =>
                        {
                            Some((end.since(e.t), text.clone()))
                        }
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
                    talk: self.chat.social.unsaved(*id, end),
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
    /// under way goes to the memory first; the lines still on their way are not said.
    pub fn shutdown(&mut self, reason: u32) {
        if self.map.is_some() {
            self.chat_map_end();
        }
        for (_, p) in std::mem::take(&mut self.chat.carried) {
            self.chat.unsaid(&p, "the plugin stops");
        }
        let wait = match reason {
            lb_ffi::LB_SHUTDOWN_DETACH => Duration::from_secs_f32(self.config.chat.provider.timeout + 2.0),
            lb_ffi::LB_SHUTDOWN_PROCESS_EXIT => Duration::from_secs(2),
            _ => Duration::from_millis(500),
        };
        self.chat.backend.shutdown(wait);
        for reply in self.chat.backend.poll() {
            self.chat.lost_reply(&reply, "the plugin stops");
        }
    }
}
