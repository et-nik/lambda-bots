//! What the bots' chat keeps from map to map: their talks with players, their own recent lines, what each human
//! wrote lately and when they were last around, and how many of the hour's lines nobody asked for are left. The
//! runtime owns it and lends it to the director, which is made anew every map. When a map ends, every time in it is
//! moved back by the map's length, so the next map, whose clock starts at 0, reads it the same way, live and in a
//! replay. Only `Vec`, `VecDeque` and `BTreeMap`: a replay walks them in the same order.

use std::collections::{BTreeMap, VecDeque};

use lb_core::time::SimTime;
use serde::{Deserialize, Serialize};

use crate::addressing;
use crate::journal::Who;
use crate::request::Said;

/// Seconds without a line after which a talk is over.
pub const THREAD_IDLE: f64 = 150.0;
/// Seconds a talk a greeting opened stays on until the player answers.
pub const GREETING_IDLE: f64 = 45.0;
/// Seconds away after which a player is greeted again.
pub const AWAY: f64 = 2700.0;
/// Seconds a player who called another human by name is taken to be talking with them.
pub const ASIDE: f64 = 60.0;
/// Seconds a bot's own line is kept.
pub const OWN_KEEP: f64 = 1800.0;
/// Seconds what a player wrote is kept.
pub const GIST_KEEP: f64 = 3.0 * 3600.0;
const THREADS: usize = 32;
const THREAD_LINES: usize = 12;
const OWN_LINES: usize = 6;
const PLAYERS: usize = 64;
const GISTS: usize = 16;
/// Lines nobody asked for that may come one right after the other.
const REMARKS: f64 = 2.0;

/// `t` as the next map sees it, the map that ended at `end` being over.
fn back(t: SimTime, end: SimTime) -> SimTime {
    SimTime(t.0 - end.0)
}

/// Whether `t` was less than `secs` before `now`.
fn within(t: Option<SimTime>, now: SimTime, secs: f64) -> bool {
    t.is_some_and(|t| now.since(t) < secs)
}

/// `t`, unless it is ahead of the map's start: left by a map whose end never came.
fn started(t: Option<SimTime>) -> Option<SimTime> {
    t.filter(|t| t.0 <= 0.0)
}

/// What the chat keeps across maps.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Social {
    pub talks: Talks,
    pub players: Players,
    pub remarks: Remarks,
}

impl Social {
    /// A bot's line went out. `to`: the player it answers (`userid`, name); `lb chat test`'s console (`userid`
    /// below 0) is nobody. `greeting`: the line greets them, so a talk it opens stays short until they answer.
    pub fn said(&mut self, now: SimTime, persona: &str, to: Option<(i32, &str)>, greeting: bool, text: &str) {
        let to = to.filter(|&(userid, _)| userid >= 0);
        self.talks.said(now, persona, to, greeting, text);
    }

    /// A human was put in the server, back from a map change too. Whether they are new or were away for [`AWAY`]
    /// or longer: a bot may greet them.
    pub fn arrive(&mut self, name: &str, now: SimTime) -> bool {
        self.players.arrive(name, now)
    }

    /// A human left the server.
    pub fn left(&mut self, name: &str, now: SimTime) {
        self.players.left(name, now);
    }

    /// The map ends with these humans on the server: they were there just now.
    pub fn present(&mut self, names: &[String], now: SimTime) {
        for name in names {
            self.players.seen(name, now);
        }
    }

    /// Lines of the player's talks the memory has not had yet, oldest first: seconds before `end`, the bot's
    /// persona, the bot's own line, the text ([`crate::request::PlayerMap::talk`]).
    pub fn unsaved(&self, player: i32, end: SimTime) -> Vec<(f64, String, bool, String)> {
        self.talks.unsaved(player, end)
    }

    /// The memory has every line of the talks.
    pub fn mark_saved(&mut self) {
        self.talks.mark_saved();
    }

    /// The map ended at `end` with the humans `present` (`userid`s) on the server; after [`Social::present`], and
    /// after the memory took the [`Social::unsaved`] lines. Talks with those who left and talks gone quiet are over;
    /// the bots' lines older than [`OWN_KEEP`], players away longer than [`AWAY`] and lines older than
    /// [`GIST_KEEP`] are forgotten. Every time left is moved back by `end`, to 0 or less.
    pub fn map_end(&mut self, end: SimTime, present: &[i32]) {
        self.talks.map_end(end, present);
        self.players.map_end(end);
        self.remarks.at = back(self.remarks.at, end);
    }

    /// A map starts: nobody is on the server until put in it again. Anything ahead of the start, left by a map whose
    /// end never came, is dropped.
    pub fn map_start(&mut self) {
        self.talks.map_start();
        self.players.map_start();
        self.remarks.at = self.remarks.at.min(SimTime::ZERO);
    }
}

/// A line of a talk.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Line {
    pub at: SimTime,
    /// The bot's own.
    pub mine: bool,
    pub text: String,
    /// The memory of players has it.
    pub saved: bool,
}

/// A bot's talk with a player.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Thread {
    /// The bot's persona.
    pub bot: String,
    /// The player's `userid`.
    pub player: i32,
    /// The player's name at their last line.
    pub name: String,
    /// The player spoke to the bot: by name, to the bots, with a question to it.
    pub direct: bool,
    /// A greeting opened it, and the player has not answered yet.
    pub greeting: bool,
    pub last: SimTime,
    /// Oldest first.
    pub lines: VecDeque<Line>,
}

impl Thread {
    /// Whether the talk goes on: a line within [`THREAD_IDLE`] ([`GREETING_IDLE`] after a greeting nobody answered
    /// yet), and the player spoke to the bot or the bot said something.
    pub fn active(&self, now: SimTime) -> bool {
        let idle = if self.greeting { GREETING_IDLE } else { THREAD_IDLE };
        now.since(self.last) <= idle && (self.direct || self.lines.iter().any(|l| l.mine))
    }

    /// The bot's last line in the talk.
    pub fn last_mine(&self) -> Option<&str> {
        self.lines.iter().rev().find(|l| l.mine).map(|l| l.text.as_str())
    }

    fn push(&mut self, at: SimTime, mine: bool, text: &str) {
        self.last = at;
        self.lines.push_back(Line {
            at,
            mine,
            text: text.to_string(),
            saved: false,
        });
        while self.lines.len() > THREAD_LINES {
            self.lines.pop_front();
        }
    }

    fn said(&self, now: SimTime) -> Vec<Said> {
        self.lines
            .iter()
            .map(|l| Said {
                age: now.since(l.at),
                mine: l.mine,
                text: l.text.clone(),
            })
            .collect()
    }
}

/// The bots' talks with players, and each bot's own recent lines.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Talks {
    threads: Vec<Thread>,
    /// Each bot's lines by persona, oldest first.
    own: BTreeMap<String, VecDeque<(SimTime, String)>>,
}

impl Talks {
    pub fn threads(&self) -> &[Thread] {
        &self.threads
    }

    fn find(&self, bot: &str, player: i32) -> Option<&Thread> {
        self.threads.iter().find(|t| t.bot == bot && t.player == player)
    }

    /// The talk going on between the player and one of the bots `hears` lets in; the latest if there are more.
    pub fn partner(&self, now: SimTime, player: i32, hears: impl Fn(&str) -> bool) -> Option<&Thread> {
        self.threads
            .iter()
            .filter(|t| t.player == player && t.active(now) && hears(&t.bot))
            .max_by(|a, b| a.last.0.total_cmp(&b.last.0))
    }

    /// Whether the bot said something to the player less than `secs` ago.
    pub fn with(&self, now: SimTime, bot: &str, player: i32, secs: f64) -> bool {
        self.find(bot, player)
            .is_some_and(|t| t.lines.iter().any(|l| l.mine && within(Some(l.at), now, secs)))
    }

    /// The lines of the bot's talk with the player, oldest first.
    pub fn talk(&self, now: SimTime, bot: &str, player: i32) -> Vec<Said> {
        self.find(bot, player).map_or_else(Vec::new, |t| t.said(now))
    }

    /// The bot's lines of the last [`OWN_KEEP`] seconds, oldest first.
    pub fn own(&self, now: SimTime, bot: &str) -> Vec<Said> {
        self.own.get(bot).map_or_else(Vec::new, |lines| {
            lines
                .iter()
                .filter(|(at, _)| now.since(*at) <= OWN_KEEP)
                .map(|(at, text)| Said {
                    age: now.since(*at),
                    mine: true,
                    text: text.clone(),
                })
                .collect()
        })
    }

    /// The bot's talk with the player, opened if there is none; the talk quiet longest makes room.
    fn open(&mut self, now: SimTime, bot: &str, player: i32, name: &str) -> &mut Thread {
        let at = match self.threads.iter().position(|t| t.bot == bot && t.player == player) {
            Some(at) => at,
            None => {
                if self.threads.len() >= THREADS
                    && let Some(quiet) = (0..self.threads.len())
                        .min_by(|&a, &b| self.threads[a].last.0.total_cmp(&self.threads[b].last.0))
                {
                    self.threads.remove(quiet);
                }
                self.threads.push(Thread {
                    bot: bot.to_string(),
                    player,
                    name: name.to_string(),
                    direct: false,
                    greeting: false,
                    last: now,
                    lines: VecDeque::new(),
                });
                self.threads.len() - 1
            }
        };
        &mut self.threads[at]
    }

    /// A player's line in a talk with the bot, opened if there is none; `direct`: the line speaks to the bot.
    pub(crate) fn heard(&mut self, now: SimTime, bot: &str, who: &Who, text: &str, direct: bool) {
        if who.userid < 0 {
            return;
        }
        let t = self.open(now, bot, who.userid, &who.name);
        t.name.clone_from(&who.name);
        t.direct |= direct;
        t.greeting = false;
        t.push(now, false, text);
    }

    /// A player's line that only shows the talk is still on.
    pub(crate) fn touch(&mut self, now: SimTime, bot: &str, player: i32) {
        if let Some(t) = self.threads.iter_mut().find(|t| t.bot == bot && t.player == player) {
            t.last = now;
        }
    }

    fn said(&mut self, now: SimTime, bot: &str, to: Option<(i32, &str)>, greeting: bool, text: &str) {
        let own = self.own.entry(bot.to_string()).or_default();
        own.push_back((now, text.to_string()));
        own.retain(|(at, _)| now.since(*at) <= OWN_KEEP);
        while own.len() > OWN_LINES {
            own.pop_front();
        }
        if let Some((player, name)) = to {
            let t = self.open(now, bot, player, name);
            if !t.active(now) {
                t.greeting = greeting;
            }
            t.push(now, true, text);
        }
    }

    fn unsaved(&self, player: i32, end: SimTime) -> Vec<(f64, String, bool, String)> {
        let mut lines: Vec<(&str, &Line)> = self
            .threads
            .iter()
            .filter(|t| t.player == player)
            .flat_map(|t| t.lines.iter().filter(|l| !l.saved).map(|l| (t.bot.as_str(), l)))
            .collect();
        lines.sort_by(|a, b| a.1.at.0.total_cmp(&b.1.at.0));
        lines
            .into_iter()
            .map(|(bot, l)| (end.since(l.at), bot.to_string(), l.mine, l.text.clone()))
            .collect()
    }

    fn mark_saved(&mut self) {
        for line in self.threads.iter_mut().flat_map(|t| t.lines.iter_mut()) {
            line.saved = true;
        }
    }

    fn map_end(&mut self, end: SimTime, present: &[i32]) {
        self.threads
            .retain(|t| present.contains(&t.player) && end.since(t.last) <= THREAD_IDLE);
        for t in &mut self.threads {
            t.last = back(t.last, end);
            for line in &mut t.lines {
                line.at = back(line.at, end);
            }
        }
        for lines in self.own.values_mut() {
            lines.retain(|(at, _)| end.since(*at) <= OWN_KEEP);
            for (at, _) in lines.iter_mut() {
                *at = back(*at, end);
            }
        }
        self.own.retain(|_, lines| !lines.is_empty());
    }

    fn map_start(&mut self) {
        self.threads.retain(|t| t.last.0 <= 0.0);
        for t in &mut self.threads {
            t.lines.retain(|l| l.at.0 <= 0.0);
        }
        for lines in self.own.values_mut() {
            lines.retain(|(at, _)| at.0 <= 0.0);
        }
        self.own.retain(|_, lines| !lines.is_empty());
    }
}

/// What a player's line said, to tell a repeat.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Gist {
    pub at: SimTime,
    /// See [`addressing::gist`].
    pub words: Vec<String>,
    /// A bot was asked to answer it.
    pub answered: bool,
}

/// A human the chat knows lately.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Player {
    /// On the server now.
    pub here: bool,
    /// When last seen on the server.
    pub seen: Option<SimTime>,
    /// When a bot last greeted them.
    pub greeted: Option<SimTime>,
    /// What their lines said lately, oldest first.
    pub gists: VecDeque<Gist>,
    /// When a bot last answered a line of theirs to everybody unasked.
    pub unasked: Option<SimTime>,
    /// When a bot last answered their question to everybody.
    pub asked: Option<SimTime>,
    /// The language their lines were last plainly written in.
    pub language: Option<String>,
    /// When they last called another human by name: they talk with them for [`ASIDE`] seconds.
    pub aside: Option<SimTime>,
}

/// The humans the chat knows lately, by nickname in lower case: a reconnect brings a new `userid`, never a new
/// nickname.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Players {
    by_name: BTreeMap<String, Player>,
}

/// A nickname as the players are kept by; none for an empty one.
fn key(name: &str) -> Option<String> {
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_lowercase())
}

impl Players {
    pub fn get(&self, name: &str) -> Option<&Player> {
        key(name).and_then(|k| self.by_name.get(&k))
    }

    /// The humans on the server, by nickname in lower case.
    pub fn here(&self) -> impl Iterator<Item = &str> {
        self.by_name.iter().filter(|(_, p)| p.here).map(|(k, _)| k.as_str())
    }

    /// The player's record, made if there is none: the one away longest makes room.
    fn entry(&mut self, name: &str) -> Option<&mut Player> {
        let key = key(name)?;
        if !self.by_name.contains_key(&key) && self.by_name.len() >= PLAYERS {
            let order = |p: &Player| (p.here, p.seen.map_or(f64::NEG_INFINITY, |t| t.0));
            let gone = self
                .by_name
                .iter()
                .min_by(|a, b| {
                    let (a, b) = (order(a.1), order(b.1));
                    a.0.cmp(&b.0).then(a.1.total_cmp(&b.1))
                })
                .map(|(k, _)| k.clone());
            if let Some(gone) = gone {
                self.by_name.remove(&gone);
            }
        }
        Some(self.by_name.entry(key).or_default())
    }

    fn arrive(&mut self, name: &str, now: SimTime) -> bool {
        let Some(p) = self.entry(name) else {
            return false;
        };
        let new = !p.here && !within(p.seen, now, AWAY);
        p.here = true;
        p.seen = Some(now);
        new
    }

    fn left(&mut self, name: &str, now: SimTime) {
        if let Some(p) = self.entry(name) {
            p.here = false;
            p.seen = Some(now);
        }
    }

    fn seen(&mut self, name: &str, now: SimTime) {
        if let Some(p) = self.entry(name) {
            p.seen = Some(now);
        }
    }

    /// The language the player writes in: `detected` in their line when it shows, else the last one that did.
    pub(crate) fn language(&mut self, name: &str, detected: Option<&str>) -> Option<String> {
        let Some(p) = self.entry(name) else {
            return detected.map(String::from);
        };
        if let Some(code) = detected {
            p.language = Some(code.to_string());
        }
        p.language.clone()
    }

    /// A line the player wrote, by its [`addressing::gist`].
    pub(crate) fn wrote(&mut self, name: &str, now: SimTime, words: Vec<String>) {
        let Some(p) = self.entry(name) else {
            return;
        };
        if words.is_empty() {
            return;
        }
        p.gists.retain(|g| now.since(g.at) <= GIST_KEEP);
        p.gists.push_back(Gist {
            at: now,
            words,
            answered: false,
        });
        while p.gists.len() > GISTS {
            p.gists.pop_front();
        }
    }

    /// A bot was asked to answer the player's line written at `at`.
    pub(crate) fn answered(&mut self, name: &str, at: SimTime) {
        if let Some(p) = self.entry(name)
            && let Some(g) = p.gists.iter_mut().rev().find(|g| g.at == at)
        {
            g.answered = true;
        }
    }

    /// Whether the player wrote what `words` say since `since`, and a bot was asked to answer it.
    pub fn answered_since(&self, name: &str, words: &[String], since: SimTime) -> bool {
        self.get(name).is_some_and(|p| {
            p.gists
                .iter()
                .any(|g| g.answered && g.at >= since && addressing::same_gist(&g.words, words))
        })
    }

    /// Whether the player wrote what `words` say between `from` and `to`, answered or not.
    pub fn wrote_between(&self, name: &str, words: &[String], from: SimTime, to: SimTime) -> bool {
        self.get(name).is_some_and(|p| {
            p.gists
                .iter()
                .any(|g| g.at >= from && g.at <= to && addressing::same_gist(&g.words, words))
        })
    }

    pub(crate) fn greet(&mut self, name: &str, now: SimTime) {
        if let Some(p) = self.entry(name) {
            p.greeted = Some(now);
        }
    }

    /// Whether a bot greeted the player less than `secs` ago.
    pub fn greeted_within(&self, name: &str, now: SimTime, secs: f64) -> bool {
        self.get(name).is_some_and(|p| within(p.greeted, now, secs))
    }

    /// A bot answers a line of the player's nobody asked it to.
    pub(crate) fn answer_unasked(&mut self, name: &str, now: SimTime) {
        if let Some(p) = self.entry(name) {
            p.unasked = Some(now);
        }
    }

    /// Whether a bot answered the player unasked less than `secs` ago.
    pub fn unasked_within(&self, name: &str, now: SimTime, secs: f64) -> bool {
        self.get(name).is_some_and(|p| within(p.unasked, now, secs))
    }

    /// A bot answers the player's question to everybody.
    pub(crate) fn answer_question(&mut self, name: &str, now: SimTime) {
        if let Some(p) = self.entry(name) {
            p.asked = Some(now);
        }
    }

    /// Whether a bot answered the player's question to everybody less than `secs` ago.
    pub fn asked_within(&self, name: &str, now: SimTime, secs: f64) -> bool {
        self.get(name).is_some_and(|p| within(p.asked, now, secs))
    }

    /// The player called another human by name, or was called by one.
    pub(crate) fn set_aside(&mut self, name: &str, now: SimTime) {
        if let Some(p) = self.entry(name) {
            p.aside = Some(now);
        }
    }

    /// Whether the player talks with another human: one of them called the other less than [`ASIDE`] ago.
    pub fn aside(&self, name: &str, now: SimTime) -> bool {
        self.get(name).is_some_and(|p| within(p.aside, now, ASIDE))
    }

    fn map_end(&mut self, end: SimTime) {
        self.by_name
            .retain(|_, p| p.here || p.seen.is_some_and(|t| end.since(t) <= AWAY));
        for p in self.by_name.values_mut() {
            p.gists.retain(|g| end.since(g.at) <= GIST_KEEP);
            for g in &mut p.gists {
                g.at = back(g.at, end);
            }
            for t in [&mut p.seen, &mut p.greeted, &mut p.unasked, &mut p.asked, &mut p.aside] {
                *t = t.map(|t| back(t, end));
            }
        }
    }

    fn map_start(&mut self) {
        for p in self.by_name.values_mut() {
            p.here = false;
            p.gists.retain(|g| g.at.0 <= 0.0);
            for t in [&mut p.seen, &mut p.greeted, &mut p.unasked, &mut p.asked, &mut p.aside] {
                *t = started(*t);
            }
        }
    }
}

/// The hour's lines nobody asked for, all bots together: two at once at most, refilled at
/// `chat.limits.remarks_per_hour`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Remarks {
    tokens: f64,
    at: SimTime,
}

impl Default for Remarks {
    fn default() -> Self {
        Remarks {
            tokens: REMARKS,
            at: SimTime::ZERO,
        }
    }
}

impl Remarks {
    /// Takes a line nobody asked for, if one is left.
    pub(crate) fn take(&mut self, now: SimTime, per_hour: f32) -> bool {
        self.tokens = (self.tokens + now.since(self.at).max(0.0) * f64::from(per_hour) / 3600.0).min(REMARKS);
        self.at = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// A line that took one was not said.
    pub fn give(&mut self) {
        self.tokens = (self.tokens + 1.0).min(REMARKS);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn who(userid: i32, name: &str) -> Who {
        Who {
            slot: 1,
            userid,
            name: name.into(),
            bot: false,
        }
    }

    fn t(s: f64) -> SimTime {
        SimTime(s)
    }

    #[test]
    fn talks_open_go_on_and_end() {
        let mut s = Social::default();
        let gordon = who(11, "Gordon");
        s.talks.heard(t(1.0), "Plutonium", &gordon, "Привет Плутон!", true);
        let any = |_: &str| true;
        assert!(
            s.talks.partner(t(100.0), 11, any).is_some(),
            "a word to the bot opens a talk"
        );
        assert!(
            s.talks.partner(t(152.0), 11, any).is_none(),
            "a talk is over after 150 s"
        );
        assert!(s.talks.partner(t(100.0), 11, |b| b != "Plutonium").is_none());
        s.said(t(5.0), "Plutonium", Some((11, "Gordon")), false, "привет, Гордон");
        assert!(s.talks.with(t(6.0), "Plutonium", 11, 1800.0));
        assert!(!s.talks.with(t(6.0), "Kleiner", 11, 1800.0));
        assert_eq!(
            s.talks.partner(t(150.0), 11, any).unwrap().last_mine(),
            Some("привет, Гордон")
        );
        let said = s.talks.talk(t(10.0), "Plutonium", 11);
        assert_eq!(said.len(), 2);
        assert_eq!((said[0].age, said[0].mine, said[1].mine), (9.0, false, true));

        s.said(t(20.0), "Kleiner", Some((12, "Barney")), true, "привет, Барни");
        assert!(s.talks.partner(t(64.0), 12, any).is_some());
        assert!(
            s.talks.partner(t(66.0), 12, any).is_none(),
            "a greeting nobody answered is over sooner"
        );
        s.said(t(70.0), "Kleiner", Some((12, "Barney")), true, "ну привет");
        s.talks.heard(t(80.0), "Kleiner", &who(12, "Barney"), "спасибо", false);
        assert!(
            s.talks.partner(t(200.0), 12, any).is_some(),
            "answered, it lasts as long as any"
        );

        s.said(t(30.0), "Plutonium", Some((-1, "admin")), false, "тест");
        assert!(s.talks.threads().iter().all(|t| t.player >= 0), "the console is nobody");
        assert_eq!(s.talks.own(t(30.0), "Plutonium").len(), 2);

        let ivan = who(13, "Ivan");
        s.talks.heard(t(40.0), "Plutonium", &ivan, "просто так", false);
        assert!(
            s.talks.partner(t(41.0), 13, any).is_none(),
            "a line to everybody opens no talk until the bot answers"
        );
        for i in 0..20 {
            s.talks.heard(t(50.0 + f64::from(i)), "Plutonium", &ivan, "ещё", true);
        }
        assert_eq!(s.talks.talk(t(80.0), "Plutonium", 13).len(), THREAD_LINES);
        for i in 0..40 {
            s.talks
                .heard(t(100.0 + f64::from(i)), "Kleiner", &who(100 + i, "x"), "x", true);
        }
        assert_eq!(s.talks.threads().len(), THREADS);
        assert!(
            s.talks.partner(t(140.0), 139, any).is_some(),
            "the quietest talks made room"
        );
    }

    #[test]
    fn own_lines_are_few_and_recent() {
        let mut s = Social::default();
        for i in 0..10 {
            s.said(t(f64::from(i)), "Plutonium", None, false, &format!("строка {i}"));
        }
        let own = s.talks.own(t(10.0), "Plutonium");
        assert_eq!(own.len(), 6);
        assert_eq!((own[0].text.as_str(), own[0].age, own[0].mine), ("строка 4", 6.0, true));
        assert!(s.talks.own(t(1810.0), "Plutonium").is_empty());
        assert!(s.talks.own(t(10.0), "Kleiner").is_empty());
        assert!(s.talks.threads().is_empty(), "a line to nobody opens no talk");
    }

    #[test]
    fn players_come_and_go() {
        let mut s = Social::default();
        assert!(s.arrive("Gordon", t(1.0)), "never seen");
        assert!(!s.arrive("GORDON", t(2.0)), "the same nickname in any case");
        s.left("Gordon", t(100.0));
        assert!(!s.arrive("Gordon", t(100.0 + AWAY - 1.0)));
        s.left("Gordon", t(3000.0));
        assert!(s.arrive("Gordon", t(3000.0 + AWAY)), "away long enough");
        assert!(!s.arrive("  ", t(1.0)), "empty names are never kept");
        assert!(s.players.get(" ").is_none());
        s.arrive("Barney", t(1.0));
        assert_eq!(s.players.here().collect::<Vec<_>>(), ["barney", "gordon"]);

        let p = &mut s.players;
        assert_eq!(p.language("Gordon", Some("en")).as_deref(), Some("en"));
        assert_eq!(p.language("Gordon", None).as_deref(), Some("en"), "the last plain one");
        assert_eq!(p.language("Nobody", None), None);

        let gist = addressing::gist("где рельсы?", &[]);
        p.wrote("Gordon", t(10.0), gist.clone());
        assert!(!p.answered_since("Gordon", &gist, t(0.0)));
        p.answered("Gordon", t(10.0));
        assert!(p.answered_since("Gordon", &gist, t(0.0)));
        assert!(!p.answered_since("Gordon", &gist, t(11.0)));
        assert!(p.wrote_between("Gordon", &gist, t(0.0), t(10.0)));
        assert!(!p.wrote_between("Barney", &gist, t(0.0), t(10.0)));
        for i in 0..30 {
            p.wrote("Gordon", t(20.0 + f64::from(i)), vec![format!("w{i}")]);
        }
        assert_eq!(p.get("gordon").unwrap().gists.len(), GISTS);

        p.greet("Gordon", t(10.0));
        assert!(p.greeted_within("Gordon", t(10.0 + AWAY - 1.0), AWAY));
        assert!(!p.greeted_within("Gordon", t(10.0 + AWAY), AWAY));
        p.set_aside("Barney", t(10.0));
        assert!(p.aside("barney", t(69.0)) && !p.aside("barney", t(70.0)));

        for i in 0..100 {
            s.arrive(&format!("player{i}"), t(10.0 + f64::from(i)));
            s.left(&format!("player{i}"), t(11.0 + f64::from(i)));
        }
        assert_eq!(s.players.by_name.len(), PLAYERS);
        assert!(
            s.players.get("Gordon").is_some() && s.players.get("Barney").is_some(),
            "players on the server stay"
        );
        assert!(s.players.get("player0").is_none() && s.players.get("player99").is_some());
    }

    #[test]
    fn a_map_change_moves_every_time_back() {
        let mut s = Social::default();
        let (gordon, barney) = (who(11, "Gordon"), who(12, "Barney"));
        s.arrive("Gordon", t(1.0));
        s.arrive("Barney", t(2.0));
        s.arrive("Eli", t(3.0));
        s.left("Eli", t(4.0));
        s.said(t(10.0), "Plutonium", None, false, "старое");
        s.talks.heard(t(2700.0), "Kleiner", &gordon, "давно было", true);
        s.talks.heard(t(2980.0), "Plutonium", &gordon, "Привет Плутон!", true);
        s.said(t(2985.0), "Plutonium", Some((11, "Gordon")), false, "привет, Гордон");
        s.talks.heard(t(2986.0), "Kleiner", &barney, "кляйнер, где ты?", true);
        s.players
            .wrote("Gordon", t(2980.0), addressing::gist("Привет Плутон!", &[]));
        s.players.greet("Gordon", t(2990.0));
        assert!(s.remarks.take(t(500.0), 6.0));
        assert_eq!(s.unsaved(11, t(3000.0)).len(), 3);
        s.mark_saved();
        assert!(s.unsaved(11, t(3000.0)).is_empty());
        s.said(t(2995.0), "Plutonium", Some((11, "Gordon")), false, "ну что?");
        assert_eq!(
            s.unsaved(11, t(3000.0)),
            vec![(5.0, "Plutonium".to_string(), true, "ну что?".to_string())]
        );

        s.present(&["Gordon".into(), "Barney".into()], t(3000.0));
        s.map_end(t(3000.0), &[11]);
        let threads = s.talks.threads();
        assert_eq!(threads.len(), 1, "Barney left the server; Kleiner's talk went quiet");
        assert_eq!(threads[0].last, t(-5.0));
        assert_eq!(s.talks.own(t(0.0), "Plutonium").len(), 2, "the oldest line is gone");
        assert!(s.players.get("Eli").is_none(), "away too long");
        let g = s.players.get("Gordon").unwrap();
        assert_eq!(
            (g.seen, g.greeted, g.gists[0].at),
            (Some(t(0.0)), Some(t(-10.0)), t(-20.0))
        );
        assert!(s.talks.threads().iter().all(|t| t.lines.iter().all(|l| l.at.0 <= 0.0)));
        assert!(s.remarks.at.0 <= 0.0);

        s.map_start();
        assert_eq!(s.players.here().count(), 0, "nobody until put in the server");
        assert!(!s.arrive("Gordon", t(3.0)), "back from the map change");
        assert!(s.talks.partner(t(10.0), 11, |_| true).is_some(), "the talk goes on");
        assert!(s.talks.partner(t(146.0), 11, |_| true).is_none());

        let mut stale = Social::default();
        stale.said(t(50.0), "Plutonium", Some((11, "Gordon")), false, "без конца карты");
        stale.players.greet("Gordon", t(50.0));
        stale.map_start();
        assert!(stale.talks.threads().is_empty() && stale.talks.own(t(60.0), "Plutonium").is_empty());
        assert!(!stale.players.greeted_within("Gordon", t(60.0), AWAY));
    }

    #[test]
    fn the_hour_holds_two_remarks_at_once() {
        let mut r = Remarks::default();
        assert!(r.take(t(0.0), 6.0) && r.take(t(1.0), 6.0));
        assert!(!r.take(t(2.0), 6.0));
        assert!(!r.take(t(500.0), 6.0), "one in ten minutes");
        assert!(r.take(t(601.0), 6.0));
        r.give();
        r.give();
        r.give();
        assert!(r.take(t(602.0), 6.0) && r.take(t(603.0), 6.0) && !r.take(t(604.0), 6.0));
        assert!(!r.take(t(100_000.0), 0.0), "none an hour is none");
    }
}
