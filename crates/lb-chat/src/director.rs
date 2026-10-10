//! Who says something, and whether anyone does: players spoken to answer, a remark now and then, never a flood.
//! Bots never answer bots; nobody speaks while no human is there to read it. A player's line is weighed step by
//! step: its language, a repeat, a bot's name, noise, another human's name, a talk going on, a hello, a touchy
//! subject, the bots as a whole, a question, the rest. Lines nobody asked for keep to an hourly bucket shared by all
//! bots; an answer whose bot is busy waits for it a little.

use lb_core::rng::Pcg32;
use lb_core::time::SimTime;

use crate::addressing::{self, BotsTalk, Noise};
use crate::botchat::Priority;
use crate::journal::{Event, Journal, NEMESIS, Notable, STREAK_SPOKEN, Who};
use crate::lang;
use crate::request::Trigger;
use crate::talk::{AWAY, GIST_KEEP, Social};

/// Seconds back a fight makes a bot the likelier one to answer a player.
const FOUGHT_WINDOW: f64 = 60.0;
/// Bots besides the winner who say something at the end of a match.
const MATCH_END_BOTS: usize = 1;
/// Seconds an answer waits for its bot, or for the line bucket.
pub const WAIT_KEEP: f64 = 30.0;
/// Seconds the same line again, once answered (by a bot it names, when it names any), is a repeat.
const REPEAT: f64 = 300.0;
/// Seconds from which the same line again, not naming a bot, is one the player keeps sending: a bind.
const LONG_REPEAT: f64 = 20.0;
/// Seconds back the chat's last line may be what a player's line answers.
const LAST_LINE: f64 = 30.0;
/// Seconds a bot's word to a player makes their question in the second person one to that bot.
const TALKED: f64 = 1800.0;
/// Seconds between answers to one player's questions to everybody.
const QUESTION_GAP: f64 = 120.0;
/// Seconds between unasked answers to one player.
const UNASKED_GAP: f64 = 240.0;

/// Volume limits, from `chat.limits`, and the server's language.
#[derive(Clone, Debug, PartialEq)]
pub struct Limits {
    pub lines_per_minute: f32,
    pub requests_per_minute: f32,
    pub remark_gap: f32,
    pub bot_remark_gap: f32,
    /// Lines nobody asked for an hour, all bots together.
    pub remarks_per_hour: f32,
    /// `chat.language`: a line in another language gets no answer, unless it is in English.
    pub language: String,
}

/// A bot as the director weighs it.
#[derive(Clone, Debug, PartialEq)]
pub struct Speaker {
    pub who: Who,
    /// The personality's name: talks go by it.
    pub persona: String,
    /// More names players call it by: the profile's `chat.call`.
    pub calls: Vec<String>,
    pub team: u8,
    pub chattiness: f32,
    pub alive: bool,
    /// The line on its way, if any.
    pub busy: Option<Priority>,
    pub last_remark: Option<SimTime>,
}

impl Speaker {
    /// Whether `text` calls the bot: by its name, or one it is called by.
    fn called(&self, text: &str) -> bool {
        addressing::mentions(text, &self.who.name) || self.calls.iter().any(|c| addressing::mentions(text, c))
    }

    /// Whether a line of `priority` may be asked of it now: nothing as important on its way.
    fn free(&self, priority: Priority) -> bool {
        self.busy.is_none_or(|p| p < priority)
    }
}

/// What the director reacts to.
#[derive(Clone, Debug, PartialEq)]
pub enum Cause {
    /// A human's chat line; `team`: in the team chat, heard only by `from_team`.
    Chat {
        from: Who,
        text: String,
        team: bool,
        from_team: u8,
    },
    /// A human joined who is new, or was away long enough ([`Social::arrive`]); not one back from a map change.
    Join {
        who: Who,
    },
    MatchEnd {
        winner: Option<Who>,
    },
    Notable(Notable),
    /// One of the bots was killed while typing in the open.
    KilledWhileTyping {
        bot: u8,
        killer: Option<Who>,
    },
    /// GunGame: one of the bots reached the last level.
    LastLevel {
        bot: u8,
    },
    /// `lb chat test`: as if `from` wrote `text` to `bot`.
    Test {
        bot: u8,
        from: Who,
        text: String,
    },
}

/// A bot to ask for a line.
#[derive(Clone, Debug, PartialEq)]
pub struct Speak {
    pub slot: u8,
    pub trigger: Trigger,
    pub priority: Priority,
    pub team: bool,
    /// It took one of the hour's lines nobody asked for: give it back ([`crate::talk::Remarks::give`]) when no line
    /// comes of it.
    pub unasked: bool,
    /// When the player's line it answers came: asking for it marked the line answered by the bot, which
    /// [`Social::unanswered`] takes back when no line comes of it.
    pub line_at: Option<SimTime>,
    /// It marked the player greeted: when they were greeted before it (`Some(None)`: never), which
    /// [`Social::ungreet`] puts back when no line comes of it.
    pub greeted: Option<Option<SimTime>>,
}

#[derive(Clone, Debug)]
struct Bucket {
    tokens: f64,
    at: SimTime,
}

impl Bucket {
    fn full(cap: f64, now: SimTime) -> Bucket {
        Bucket { tokens: cap, at: now }
    }

    /// Takes one, if at least `least` are there.
    fn take(&mut self, now: SimTime, per_minute: f32, cap: f64, least: f64) -> bool {
        self.tokens = (self.tokens + now.since(self.at).max(0.0) * f64::from(per_minute) / 60.0).min(cap);
        self.at = now;
        if self.tokens >= least {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    fn give(&mut self, cap: f64) {
        self.tokens = (self.tokens + 1.0).min(cap);
    }
}

/// An answer kept until its bot is free.
#[derive(Clone, Debug)]
struct Waiting {
    /// When the line it answers came.
    since: SimTime,
    /// The bot's persona: another bot may take the slot.
    persona: String,
    /// What the line said ([`addressing::gist`]).
    gist: Vec<String>,
    speak: Speak,
}

#[derive(Clone, Debug)]
pub struct Director {
    rng: Pcg32,
    lines: Bucket,
    requests: Bucket,
    last_remark: Option<SimTime>,
    /// Answers waiting for their bot or for the line bucket, oldest first.
    waiting: Vec<Waiting>,
}

fn caps(l: &Limits) -> (f64, f64) {
    (
        f64::from(l.lines_per_minute).max(2.0),
        f64::from(l.requests_per_minute / 2.0).max(3.0),
    )
}

/// What the director weighs a cause against.
struct View<'a> {
    now: SimTime,
    bots: &'a [Speaker],
    journal: &'a Journal,
    limits: &'a Limits,
}

/// A human's chat line.
struct Heard<'a> {
    from: &'a Who,
    text: &'a str,
    team: bool,
    from_team: u8,
    /// When it came.
    at: SimTime,
    /// What it says ([`addressing::gist`]).
    gist: Vec<String>,
}

impl Heard<'_> {
    /// Whether the bot reads the line: a line to everybody all do, a team line its team.
    fn reaches(&self, bot: &Speaker) -> bool {
        !self.team || bot.team == self.from_team
    }

    /// The bot answering the line, in the chat it came in.
    fn answer(&self, bot: &Speaker, trigger: Trigger, priority: Priority, unasked: bool) -> Speak {
        Speak {
            slot: bot.who.slot,
            trigger,
            priority,
            team: self.team,
            unasked,
            line_at: Some(self.at),
            greeted: None,
        }
    }

    fn addressed(&self) -> Trigger {
        Trigger::Addressed {
            from: self.from.clone(),
            text: self.text.to_string(),
        }
    }

    fn continued(&self) -> Trigger {
        Trigger::Continued {
            from: self.from.clone(),
            text: self.text.to_string(),
        }
    }
}

/// A bot's line to everybody.
fn to_all(bot: &Speaker, trigger: Trigger, priority: Priority, unasked: bool) -> Speak {
    Speak {
        slot: bot.who.slot,
        trigger,
        priority,
        team: false,
        unasked,
        line_at: None,
        greeted: None,
    }
}

/// Whether an answer may wait for its bot: to a line naming it or the bots, to a question, to a line to "you".
fn waits(trigger: &Trigger) -> bool {
    match trigger {
        Trigger::Addressed { .. } | Trigger::Question { .. } => true,
        Trigger::Continued { text, .. } => {
            addressing::noise(text).is_none() && (addressing::question(text, true) || addressing::second_person(text))
        }
        _ => false,
    }
}

/// Whether a line is question marks and nothing else.
fn asks_only(text: &str) -> bool {
    let text = text.trim();
    !text.is_empty() && text.chars().all(|c| c == '?')
}

/// Whether a bot's line asks something: it ends with `?`, smileys and brackets after it aside (`?)`, `? :)`).
fn ends_asking(text: &str) -> bool {
    text.trim_end_matches(|c: char| !c.is_alphanumeric() && c != '?')
        .ends_with('?')
}

/// Whether a line is a yes or a no in signs: `+`, `++`, `-`, three at most.
fn plus_or_minus(text: &str) -> bool {
    let text = text.trim();
    (1..=3).contains(&text.chars().count()) && text.chars().all(|c| c == '+' || c == '-')
}

/// Whether `last` is at least `secs` ago, or never.
fn gap(last: Option<SimTime>, now: SimTime, secs: f32) -> bool {
    last.is_none_or(|t| now.since(t) >= f64::from(secs))
}

/// The words of the bots' names and of the names they are called by: no part of what a line says.
fn bot_words(bots: &[Speaker]) -> Vec<String> {
    bots.iter()
        .flat_map(|b| std::iter::once(&b.who.name).chain(&b.calls))
        .flat_map(|name| addressing::name_words(name))
        .collect()
}

/// The chat's latest line within [`LAST_LINE`] that is not noise and not the player's own: what their line may
/// answer.
fn last_line<'a>(view: &View<'a>, from: &Who) -> Option<&'a Who> {
    view.journal
        .entries()
        .rev()
        .take_while(|e| view.now.since(e.t) <= LAST_LINE)
        .find_map(|e| match &e.event {
            Event::Chat { from: who, text, .. } if who.userid != from.userid && addressing::noise(text).is_none() => {
                Some(who)
            }
            _ => None,
        })
}

/// Whether the player and the bot killed one another lately.
fn fought(view: &View<'_>, bot: &Speaker, player: &Who) -> bool {
    let since = SimTime(view.now.secs() - FOUGHT_WINDOW);
    view.journal.fought(bot.who.userid, player.userid, since)
}

/// How likely the bot answers a player's line in their talk: a question nearly always; a short reply seldom, unless
/// the bot asked something.
fn going_on(text: &str, noise: Option<Noise>, bot: &Speaker, bot_asked: bool) -> f32 {
    if addressing::question(text, true) {
        return 0.95;
    }
    match noise {
        Some(_) if bot_asked => 0.6,
        Some(_) => Director::chatty(0.15, bot),
        None => 0.75 + 0.2 * bot.chattiness,
    }
}

impl Director {
    pub fn new(seed: u64, now: SimTime, limits: &Limits) -> Director {
        let (lines, requests) = caps(limits);
        Director {
            rng: Pcg32::new(seed, 0x6469_7265),
            lines: Bucket::full(lines, now),
            requests: Bucket::full(requests, now),
            last_remark: None,
            waiting: Vec::new(),
        }
    }

    /// A request came back without a line: its line is not used up.
    pub fn refund(&mut self, limits: &Limits) {
        self.lines.give(caps(limits).0);
    }

    /// A canned phrase came back: it asked nothing of the model.
    pub fn refund_request(&mut self, limits: &Limits) {
        self.requests.give(caps(limits).1);
    }

    /// Whether answers wait for [`Director::ready`].
    pub fn has_waiting(&self) -> bool {
        !self.waiting.is_empty()
    }

    /// Chat was switched off: nothing waits any more.
    pub fn clear_waiting(&mut self) {
        self.waiting.clear();
    }

    /// Who should say something about `cause`. `social`: what the chat keeps across maps, where a player's line is
    /// noted; `humans`: someone is there to read it. An answer whose bot is busy, or that finds the buckets empty,
    /// waits for [`Director::ready`].
    #[allow(clippy::too_many_arguments)]
    pub fn react(
        &mut self,
        now: SimTime,
        cause: &Cause,
        bots: &[Speaker],
        journal: &Journal,
        social: &mut Social,
        humans: bool,
        limits: &Limits,
    ) -> Vec<Speak> {
        if let Cause::Test { bot, from, text } = cause {
            return vec![Speak {
                slot: *bot,
                trigger: Trigger::Addressed {
                    from: from.clone(),
                    text: text.clone(),
                },
                priority: Priority::Answer,
                team: false,
                unasked: false,
                line_at: None,
                greeted: None,
            }];
        }
        if !humans {
            return Vec::new();
        }
        let view = View {
            now,
            bots,
            journal,
            limits,
        };
        let heard = match cause {
            Cause::Chat {
                from,
                text,
                team,
                from_team,
            } => Some(Heard {
                from,
                text,
                team: *team,
                from_team: *from_team,
                at: now,
                gist: addressing::gist(text, &bot_words(bots)),
            }),
            _ => None,
        };
        let wanted = match &heard {
            Some(line) => self.chat(&view, social, line),
            None => self.event(&view, social, cause),
        };
        let mut out = Vec::new();
        for mut speak in wanted {
            let Some(bot) = bots.iter().find(|b| b.who.slot == speak.slot) else {
                continue;
            };
            if bot.free(speak.priority) && self.admit(now, speak.priority, speak.unasked, social, limits) {
                self.spoke(now, &mut speak, &bot.persona, social);
                out.push(speak);
            } else if speak.priority == Priority::Answer && waits(&speak.trigger) {
                let gist = heard.as_ref().map_or_else(Vec::new, |h| h.gist.clone());
                self.hold(now, speak, &bot.persona, gist);
            }
        }
        out
    }

    /// Answers that waited and may go now, oldest first: one a bot free of answers, while the buckets last. Those
    /// older than [`WAIT_KEEP`] and those of bots that left are dropped; with nobody to read them, all are.
    pub fn ready(
        &mut self,
        now: SimTime,
        bots: &[Speaker],
        social: &mut Social,
        humans: bool,
        limits: &Limits,
    ) -> Vec<Speak> {
        let bot = |w: &Waiting| {
            bots.iter()
                .find(|b| b.who.slot == w.speak.slot && b.persona == w.persona)
        };
        self.waiting
            .retain(|w| humans && now.since(w.since) <= WAIT_KEEP && bot(w).is_some());
        let mut out: Vec<Speak> = Vec::new();
        let mut i = 0;
        while i < self.waiting.len() {
            let w = &self.waiting[i];
            let free = bot(w).is_some_and(|b| b.free(Priority::Answer)) && out.iter().all(|s| s.slot != w.speak.slot);
            if !free {
                i += 1;
                continue;
            }
            if !self.admit(now, w.speak.priority, w.speak.unasked, social, limits) {
                break;
            }
            let mut w = self.waiting.remove(i);
            self.spoke(now, &mut w.speak, &w.persona, social);
            out.push(w.speak);
        }
        out
    }

    fn chance(&mut self, p: f32) -> bool {
        self.rng.next_f32() < p.clamp(0.0, 0.97)
    }

    /// `p` scaled by a bot's chattiness: a bot of 0.5 keeps it.
    fn chatty(p: f32, bot: &Speaker) -> f32 {
        p * bot.chattiness * 2.0
    }

    fn remark_allowed(&self, now: SimTime, bot: &Speaker, limits: &Limits) -> bool {
        gap(self.last_remark, now, limits.remark_gap) && gap(bot.last_remark, now, limits.bot_remark_gap)
    }

    /// One bot of `bots`, weighted, among those free for a line of `priority`.
    fn pick<'a>(&mut self, bots: impl Iterator<Item = (&'a Speaker, f32)>, priority: Priority) -> Option<&'a Speaker> {
        let weighted: Vec<(&Speaker, f32)> = bots.filter(|(b, w)| *w > 0.0 && b.free(priority)).collect();
        let total: f32 = weighted.iter().map(|(_, w)| w).sum();
        let mut x = self.rng.next_f32() * total;
        for (bot, w) in &weighted {
            if x < *w {
                return Some(bot);
            }
            x -= w;
        }
        weighted.last().map(|(b, _)| *b)
    }

    /// A bot's weight to answer the player: its chattiness, more after a fight with them, more while dead (it types
    /// freely).
    fn weight(view: &View<'_>, bot: &Speaker, player: &Who) -> f32 {
        let fought = if fought(view, bot, player) { 3.0 } else { 1.0 };
        let free = if bot.alive { 1.0 } else { 1.5 };
        bot.chattiness * fought * free
    }

    /// Takes what a line needs: one of the hour's lines nobody asked for if it is one, a line (a line not answering
    /// anyone only while two are left, keeping one for answers) and a request. Nothing is taken when one is missing.
    fn admit(&mut self, now: SimTime, priority: Priority, unasked: bool, social: &mut Social, limits: &Limits) -> bool {
        let (line_cap, request_cap) = caps(limits);
        let least = if priority == Priority::Answer { 1.0 } else { 2.0 };
        if unasked && !social.remarks.take(now, limits.remarks_per_hour) {
            return false;
        }
        let taken = if !self.lines.take(now, limits.lines_per_minute, line_cap, least) {
            false
        } else if !self.requests.take(now, limits.requests_per_minute, request_cap, 1.0) {
            self.lines.give(line_cap);
            false
        } else {
            true
        };
        if !taken && unasked {
            social.remarks.give();
        }
        taken
    }

    /// What a line asked of the bot `persona` uses up: the gap between remarks, the player's line answered by the bot,
    /// a greeting, a player's unasked answer or answered question.
    fn spoke(&mut self, now: SimTime, speak: &mut Speak, persona: &str, social: &mut Social) {
        if speak.priority == Priority::Remark || matches!(speak.trigger, Trigger::Overheard { .. }) {
            self.last_remark = Some(now);
        }
        if let (Some((from, _)), Some(at)) = (speak.trigger.line(), speak.line_at) {
            social.players.answered(&from.name, at, persona);
        }
        match &speak.trigger {
            Trigger::Joined { who } | Trigger::Greeted { from: who, .. } => {
                speak.greeted = Some(social.players.get(&who.name).and_then(|p| p.greeted));
                social.players.greet(&who.name, now);
            }
            Trigger::Overheard { from, .. } => social.players.answer_unasked(&from.name, now),
            Trigger::Question { from, to_me: false, .. } => social.players.answer_question(&from.name, now),
            _ => {}
        }
    }

    /// Keeps an answer for [`Director::ready`]: a newer line of the player to the bot takes the older one's place.
    fn hold(&mut self, now: SimTime, speak: Speak, persona: &str, gist: Vec<String>) {
        let Some(player) = speak.trigger.line().map(|(from, _)| from.userid) else {
            return;
        };
        self.waiting.retain(|w| {
            w.speak.slot != speak.slot || w.speak.trigger.line().is_none_or(|(from, _)| from.userid != player)
        });
        self.waiting.push(Waiting {
            since: now,
            persona: persona.to_string(),
            gist,
            speak,
        });
    }

    /// A player's line, step by step: who answers it, if anyone.
    fn chat(&mut self, view: &View<'_>, social: &mut Social, line: &Heard<'_>) -> Vec<Speak> {
        let (now, from, text) = (view.now, line.from, line.text);
        let name = from.name.as_str();
        let hearing: Vec<&Speaker> = view.bots.iter().filter(|b| line.reaches(b)).collect();
        let me = name.trim().to_lowercase();
        let others: Vec<String> = social.players.here().filter(|n| *n != me).map(String::from).collect();
        let mut names: Vec<String> = view
            .bots
            .iter()
            .flat_map(|b| std::iter::once(&b.who.name).chain(&b.calls))
            .chain(&others)
            .cloned()
            .collect();
        names.push(from.name.clone());
        let language = social.players.language(name, lang::detect_without(text, &names));
        if language.is_some_and(|code| lang::foreign(&code, &view.limits.language)) {
            return Vec::new();
        }
        let named: Vec<&Speaker> = hearing.iter().copied().filter(|b| b.called(text)).collect();
        let talk = social
            .talks
            .partner(now, from.userid, |p| hearing.iter().any(|b| b.persona == p))
            .map(|t| {
                let asked = t.last_mine().filter(|l| ends_asking(&l.text)).map(|l| l.at);
                (t.bot.clone(), asked)
            });
        let partner = talk
            .as_ref()
            .and_then(|(p, _)| hearing.iter().copied().find(|b| b.persona == *p));
        let bot_asked = talk.as_ref().and_then(|(_, asked)| *asked);
        let noise = addressing::noise(text);
        // In a talk, a short reply to the bot's question is no repeat unless the player already gave it since the
        // question; a question is no bind, and another line is one only when sent twice before.
        let reply = bot_asked.is_some_and(|asked| {
            (noise.is_some_and(|n| !n.hard()) || plus_or_minus(text))
                && social.players.last_wrote(name, &line.gist).is_none_or(|t| t < asked)
        });
        let asks = partner.is_some() && noise.is_none() && addressing::question(text, true);
        let to: Vec<&str> = named.iter().map(|b| b.persona.as_str()).collect();
        let repeat = !reply
            && (social
                .players
                .answered_since(name, &line.gist, SimTime(now.secs() - REPEAT), &to)
                || self.waiting.iter().any(|w| {
                    w.speak.trigger.line().is_some_and(|(who, _)| who.userid == from.userid)
                        && (to.is_empty() || to.contains(&w.persona.as_str()))
                        && addressing::same_gist(&w.gist, &line.gist)
                }));
        let copies = social.players.times_wrote(
            name,
            &line.gist,
            SimTime(now.secs() - GIST_KEEP),
            SimTime(now.secs() - LONG_REPEAT),
        );
        let long_repeat = !reply && !asks && copies >= if partner.is_some() { 2 } else { 1 };
        social.players.wrote(name, now, line.gist.clone());
        if repeat {
            return Vec::new();
        }
        let touchy = addressing::touchy(text, &names);
        let at_another = touchy && others.iter().any(|o| addressing::names_other(text, o));
        if !named.is_empty() {
            if at_another {
                return Vec::new();
            }
            for bot in &named {
                social.talks.heard(now, &bot.persona, from, text, true);
            }
            return named
                .into_iter()
                .filter(|b| self.chance(0.92 + 0.05 * b.chattiness))
                .take(2)
                .map(|b| line.answer(b, line.addressed(), Priority::Answer, false))
                .collect();
        }
        if (noise.is_some_and(Noise::hard) && !reply) || long_repeat {
            let Some(bot) = partner else {
                return Vec::new();
            };
            social.talks.touch(now, &bot.persona, from.userid);
            if asks_only(text) && self.chance(0.6) {
                return vec![line.answer(bot, line.continued(), Priority::Answer, false)];
            }
            return Vec::new();
        }
        let in_talk = partner.is_some();
        let other = others.iter().find(|o| {
            if in_talk {
                addressing::vocative(text, o)
            } else {
                addressing::names_other(text, o)
            }
        });
        if let Some(other) = other {
            social.players.set_aside(name, now);
            social.players.set_aside(other, now);
            return Vec::new();
        }
        if let Some(bot) = partner {
            if at_another {
                return Vec::new();
            }
            let p = going_on(text, noise, bot, bot_asked.is_some());
            social.talks.heard(now, &bot.persona, from, text, false);
            if self.chance(p) {
                return vec![line.answer(bot, line.continued(), Priority::Answer, false)];
            }
            return Vec::new();
        }
        if addressing::greeting(text) {
            if social.players.greeted_within(name, now, AWAY) {
                return Vec::new();
            }
            let bot = self.pick(hearing.iter().map(|b| (*b, b.chattiness)), Priority::Greeting);
            return match bot {
                Some(b) if self.chance(Self::chatty(0.3, b)) => {
                    let trigger = Trigger::Greeted {
                        from: from.clone(),
                        text: text.to_string(),
                    };
                    vec![line.answer(b, trigger, Priority::Greeting, true)]
                }
                _ => Vec::new(),
            };
        }
        if noise.is_some() || touchy {
            return Vec::new();
        }
        let talk = addressing::bots_talk(text);
        if talk == BotsTalk::To {
            let bot = self.pick(
                hearing.iter().map(|b| (*b, Self::weight(view, b, from))),
                Priority::Answer,
            );
            return match bot {
                Some(b) if self.chance(0.8) => {
                    social.talks.heard(now, &b.persona, from, text, true);
                    vec![line.answer(b, line.addressed(), Priority::Answer, false)]
                }
                _ => Vec::new(),
            };
        }
        if addressing::question(text, false) {
            return self.question(view, social, line, &hearing);
        }
        self.overheard(view, social, line, &hearing, talk == BotsTalk::About)
    }

    /// Whether the player is in a talk with another human: one of them called the other by name lately, or the
    /// chat's last line is another human's.
    fn exchange(view: &View<'_>, social: &Social, from: &Who) -> bool {
        social.players.aside(&from.name, view.now) || last_line(view, from).is_some_and(|w| !w.bot)
    }

    /// A question: to "you", meaning the bot whose line it follows, that the player fought or talked with; or to
    /// everybody.
    fn question(&mut self, view: &View<'_>, social: &mut Social, line: &Heard<'_>, hearing: &[&Speaker]) -> Vec<Speak> {
        let (now, from, text) = (view.now, line.from, line.text);
        let exchange = Self::exchange(view, social, from);
        if addressing::second_person(text) {
            let last = last_line(view, from);
            let after = |b: &Speaker| last.is_some_and(|w| w.userid == b.who.userid);
            let near = |b: &Speaker| {
                after(b) || fought(view, b, from) || social.talks.with(now, &b.persona, from.userid, TALKED)
            };
            let bot = match hearing.iter().copied().find(|b| after(b)) {
                Some(b) => Some(b),
                None => {
                    let weight = |b: &Speaker| {
                        let near = if near(b) { 3.0 } else { 1.0 };
                        let free = if b.alive { 1.0 } else { 1.5 };
                        b.chattiness * near * free
                    };
                    self.pick(hearing.iter().map(|b| (*b, weight(b))), Priority::Answer)
                }
            };
            let Some(bot) = bot else {
                return Vec::new();
            };
            let p = if near(bot) {
                0.85
            } else if exchange {
                0.1
            } else {
                0.4
            };
            if !self.chance(p) {
                return Vec::new();
            }
            social.talks.heard(now, &bot.persona, from, text, true);
            let trigger = Trigger::Question {
                from: from.clone(),
                text: text.to_string(),
                to_me: true,
            };
            return vec![line.answer(bot, trigger, Priority::Answer, false)];
        }
        if social.players.asked_within(&from.name, now, QUESTION_GAP) {
            return Vec::new();
        }
        let bot = self.pick(
            hearing.iter().map(|b| (*b, Self::weight(view, b, from))),
            Priority::Answer,
        );
        let Some(bot) = bot else {
            return Vec::new();
        };
        let p = Self::chatty(0.45, bot) * if exchange { 0.25 } else { 1.0 };
        if !self.chance(p) {
            return Vec::new();
        }
        let trigger = Trigger::Question {
            from: from.clone(),
            text: text.to_string(),
            to_me: false,
        };
        vec![line.answer(bot, trigger, Priority::Answer, false)]
    }

    /// A line to everybody: a word from a bot now and then, less when it speaks of the bots or to another human.
    fn overheard(
        &mut self,
        view: &View<'_>,
        social: &Social,
        line: &Heard<'_>,
        hearing: &[&Speaker],
        about_bots: bool,
    ) -> Vec<Speak> {
        let (now, from) = (view.now, line.from);
        if !gap(self.last_remark, now, view.limits.remark_gap)
            || social.players.unasked_within(&from.name, now, UNASKED_GAP)
        {
            return Vec::new();
        }
        let exchange = Self::exchange(view, social, from);
        let bot = self.pick(
            hearing.iter().map(|b| (*b, Self::weight(view, b, from))),
            Priority::Greeting,
        );
        let Some(bot) = bot else {
            return Vec::new();
        };
        let p = Self::chatty(0.2, bot) * if about_bots { 0.5 } else { 1.0 } * if exchange { 0.25 } else { 1.0 };
        if !self.chance(p) {
            return Vec::new();
        }
        let trigger = Trigger::Overheard {
            from: from.clone(),
            text: line.text.to_string(),
            about_bots,
        };
        vec![line.answer(bot, trigger, Priority::Greeting, true)]
    }

    /// Everything but a chat line.
    fn event(&mut self, view: &View<'_>, social: &Social, cause: &Cause) -> Vec<Speak> {
        let (now, bots) = (view.now, view.bots);
        match cause {
            Cause::Join { who } => {
                if social.players.greeted_within(&who.name, now, AWAY) {
                    return Vec::new();
                }
                let bot = self.pick(bots.iter().map(|b| (b, b.chattiness)), Priority::Greeting);
                match bot {
                    Some(bot) if self.chance(Self::chatty(0.15, bot)) => {
                        vec![to_all(
                            bot,
                            Trigger::Joined { who: who.clone() },
                            Priority::Greeting,
                            true,
                        )]
                    }
                    _ => Vec::new(),
                }
            }
            Cause::MatchEnd { winner } => {
                let mut out = Vec::new();
                let won_bot = winner
                    .as_ref()
                    .and_then(|w| bots.iter().find(|b| b.who.userid == w.userid));
                if let Some(bot) = won_bot
                    && self.chance(0.45)
                {
                    let trigger = Trigger::MatchEnd {
                        winner: winner.clone(),
                        won: true,
                    };
                    out.push(to_all(bot, trigger, Priority::MatchEnd, false));
                }
                let mut others = 0;
                for bot in bots {
                    if others >= MATCH_END_BOTS {
                        break;
                    }
                    if !bot.free(Priority::MatchEnd) || won_bot.is_some_and(|w| w.who.slot == bot.who.slot) {
                        continue;
                    }
                    if self.chance(Self::chatty(0.2, bot)) {
                        others += 1;
                        let trigger = Trigger::MatchEnd {
                            winner: winner.clone(),
                            won: false,
                        };
                        out.push(to_all(bot, trigger, Priority::MatchEnd, false));
                    }
                }
                out
            }
            Cause::KilledWhileTyping { bot, killer } => {
                let Some(b) = bots.iter().find(|b| b.who.slot == *bot) else {
                    return Vec::new();
                };
                if killer.as_ref().is_some_and(|k| k.bot) {
                    return Vec::new();
                }
                if self.remark_allowed(now, b, view.limits) && self.chance(Self::chatty(0.5, b)) {
                    let trigger = Trigger::KilledWhileTyping { killer: killer.clone() };
                    return vec![to_all(b, trigger, Priority::Remark, true)];
                }
                Vec::new()
            }
            Cause::LastLevel { bot } => {
                let Some(b) = bots.iter().find(|b| b.who.slot == *bot) else {
                    return Vec::new();
                };
                if self.remark_allowed(now, b, view.limits) && self.chance(Self::chatty(0.2, b)) {
                    return vec![to_all(b, Trigger::LastLevel, Priority::Remark, true)];
                }
                Vec::new()
            }
            Cause::Notable(n) => self.notable(view, n),
            Cause::Chat { .. } | Cause::Test { .. } => Vec::new(),
        }
    }

    /// A moment of the game: mostly the bot it happened to says something, another bot only of a human's.
    fn notable(&mut self, view: &View<'_>, n: &Notable) -> Vec<Speak> {
        let bots = view.bots;
        let bot = |who: &Who| bots.iter().find(|b| b.who.userid == who.userid);
        let human = |who: &Who| !who.bot;
        let any = bots.iter().map(|b| (b, b.chattiness));
        let (speaker, p) = match n {
            Notable::Nemesis { killer, victim, .. } if human(killer) => (bot(victim), 0.4),
            Notable::Humiliation { killer, victim } => match (bot(victim), bot(killer)) {
                (Some(v), _) if human(killer) => (Some(v), 0.35),
                (None, Some(k)) if human(victim) => (Some(k), 0.15),
                _ => (None, 0.0),
            },
            Notable::OwnBlast { victim, .. } => match bot(victim) {
                Some(v) => (Some(v), 0.3),
                None if human(victim) => (self.pick(any, Priority::Remark), 0.08),
                None => (None, 0.0),
            },
            Notable::Revenge { killer, victim, run } if human(victim) && *run >= NEMESIS => (bot(killer), 0.2),
            Notable::Multikill { killer, count, humans } => match bot(killer) {
                Some(k) if *humans > 0 => (Some(k), if *count >= 4 { 0.35 } else { 0.2 }),
                None if human(killer) => (self.pick(any, Priority::Remark), 0.08),
                _ => (None, 0.0),
            },
            Notable::Streak { killer, count, humans } if *count >= STREAK_SPOKEN => match bot(killer) {
                Some(k) if *humans > 0 => (Some(k), 0.12),
                None if human(killer) => (self.pick(any, Priority::Remark), 0.15),
                _ => (None, 0.0),
            },
            Notable::Nemesis { .. } | Notable::Revenge { .. } | Notable::Streak { .. } | Notable::RageQuit { .. } => {
                (None, 0.0)
            }
        };
        let Some(speaker) = speaker else {
            return Vec::new();
        };
        if !self.remark_allowed(view.now, speaker, view.limits) || !self.chance(Self::chatty(p, speaker)) {
            return Vec::new();
        }
        vec![to_all(speaker, Trigger::Notable(n.clone()), Priority::Remark, true)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn who(slot: u8, name: &str, bot: bool) -> Who {
        Who {
            slot,
            userid: i32::from(slot) + 10,
            name: name.into(),
            bot,
        }
    }

    fn speaker(slot: u8, name: &str) -> Speaker {
        Speaker {
            who: who(slot, name, true),
            persona: name.into(),
            calls: Vec::new(),
            team: 0,
            chattiness: 0.5,
            alive: true,
            busy: None,
            last_remark: None,
        }
    }

    fn chatty(slot: u8, name: &str) -> Speaker {
        Speaker {
            chattiness: 1.0,
            ..speaker(slot, name)
        }
    }

    fn limits() -> Limits {
        Limits {
            lines_per_minute: 2.0,
            requests_per_minute: 6.0,
            remark_gap: 60.0,
            bot_remark_gap: 240.0,
            remarks_per_hour: 6.0,
            language: "ru".into(),
        }
    }

    /// Limits that stay out of the way.
    fn roomy() -> Limits {
        Limits {
            lines_per_minute: 60.0,
            requests_per_minute: 120.0,
            remark_gap: 0.0,
            bot_remark_gap: 0.0,
            remarks_per_hour: 600.0,
            ..limits()
        }
    }

    fn chat(from: &Who, text: &str) -> Cause {
        Cause::Chat {
            from: from.clone(),
            text: text.into(),
            team: false,
            from_team: 0,
        }
    }

    /// A map as the runtime keeps it for the director: the journal, what the chat keeps, the bots.
    struct Room {
        d: Director,
        j: Journal,
        s: Social,
        bots: Vec<Speaker>,
        limits: Limits,
    }

    impl Room {
        fn new(seed: u64, bots: Vec<Speaker>, limits: Limits) -> Room {
            Room {
                d: Director::new(seed, SimTime::ZERO, &limits),
                j: Journal::new("crossfire", SimTime::ZERO),
                s: Social::default(),
                bots,
                limits,
            }
        }

        fn cause(&mut self, t: f64, cause: &Cause) -> Vec<Speak> {
            let (bots, j, limits) = (&self.bots, &self.j, &self.limits);
            self.d.react(SimTime(t), cause, bots, j, &mut self.s, true, limits)
        }

        /// A human's line: into the journal, then to the director.
        fn say(&mut self, t: f64, from: &Who, text: &str) -> Vec<Speak> {
            let event = Event::Chat {
                from: from.clone(),
                text: text.into(),
                team: false,
            };
            self.j.push(SimTime(t), event);
            self.cause(t, &chat(from, text))
        }

        /// A bot's line goes out, `to` a player it answers.
        fn bot_says(&mut self, t: f64, slot: u8, to: Option<&Who>, text: &str) {
            let bot = self.bots.iter().find(|b| b.who.slot == slot).unwrap().clone();
            let event = Event::Chat {
                from: bot.who.clone(),
                text: text.into(),
                team: false,
            };
            self.j.push(SimTime(t), event);
            let to = to.map(|w| (w.userid, w.name.as_str()));
            self.s.said(SimTime(t), &bot.persona, to, false, text);
        }

        fn ready(&mut self, t: f64, humans: bool) -> Vec<Speak> {
            let (bots, limits) = (&self.bots, &self.limits);
            self.d.ready(SimTime(t), bots, &mut self.s, humans, limits)
        }
    }

    fn continued_by(out: &[Speak], slot: u8) -> bool {
        out.iter()
            .any(|s| s.slot == slot && matches!(s.trigger, Trigger::Continued { .. }))
    }

    #[test]
    fn named_bots_answer_and_nobody_talks_to_an_empty_server() {
        let bots = [speaker(2, "DUT9 ATLASA"), speaker(3, "Kleiner")];
        let human = who(1, "112S", false);
        let j = Journal::new("crossfire", SimTime(0.0));
        let line = chat(&human, "Атлас, ты кемпер");
        let mut s = Social::default();
        let mut d = Director::new(1, SimTime(0.0), &limits());
        assert!(
            d.react(SimTime(1.0), &line, &bots, &j, &mut s, false, &limits())
                .is_empty()
        );
        let mut answered = 0;
        for seed in 0..50 {
            let mut s = Social::default();
            let mut d = Director::new(seed, SimTime(0.0), &limits());
            let out = d.react(SimTime(1.0), &line, &bots, &j, &mut s, true, &limits());
            if let Some(s) = out.first() {
                assert_eq!((s.slot, s.priority, s.unasked), (2, Priority::Answer, false));
                assert!(matches!(s.trigger, Trigger::Addressed { .. }));
                answered += 1;
            }
        }
        assert!(answered >= 42, "0.92 + 0.05 of chattiness: {answered}");

        let gordon = who(1, "Gordon", false);
        let called = Speaker {
            calls: vec!["Плутоша".into()],
            ..speaker(2, "Plutonium")
        };
        let addressed = (0..20)
            .filter(|&seed| {
                let mut r = Room::new(seed, vec![called.clone()], roomy());
                let out = r.say(1.0, &gordon, "плутоша, го на рельсы");
                out.iter().any(|s| matches!(s.trigger, Trigger::Addressed { .. }))
            })
            .count();
        assert!(addressed >= 15, "a name from `chat.call`: {addressed}");
        for seed in 0..20 {
            let mut r = Room::new(seed, vec![speaker(2, "Plutonium")], roomy());
            let out = r.say(1.0, &gordon, "плутоша, го на рельсы");
            assert!(
                out.iter().all(|s| !matches!(s.trigger, Trigger::Addressed { .. })),
                "not without it"
            );
        }
    }

    /// The user's example: «Привет Плутон!», the bot's answer, then «Ты свою уже приготовил?» without its name.
    #[test]
    fn a_talk_goes_on_without_the_name() {
        let gordon = who(1, "Gordon", false);
        let question = "Ты свою уже приготовил?";
        let start = |seed: u64| {
            let mut r = Room::new(seed, vec![speaker(2, "Plutonium"), speaker(3, "Kleiner")], limits());
            r.s.arrive("Gordon", SimTime(0.5));
            let first = r.say(1.0, &gordon, "Привет Плутон!");
            assert!(
                first
                    .iter()
                    .all(|s| s.slot == 2 && matches!(s.trigger, Trigger::Addressed { .. }))
            );
            r.bot_says(5.0, 2, Some(&gordon), "привет, Гордон");
            r
        };
        let on = (0..100)
            .filter(|&seed| continued_by(&start(seed).say(9.0, &gordon, question), 2))
            .count();
        assert!(on >= 90, "{on}");
        for seed in 0..100 {
            let out = start(seed).say(156.0, &gordon, question);
            assert!(!continued_by(&out, 2), "the talk is over after 150 s: {out:?}");
        }
        let carried = (0..100)
            .filter(|&seed| {
                let mut r = start(seed);
                r.s.present(&["Gordon".into()], SimTime(20.0));
                r.s.map_end(SimTime(20.0), &[gordon.userid]);
                r.d = Director::new(seed + 1000, SimTime::ZERO, &r.limits);
                r.j = Journal::new("stalkyard", SimTime::ZERO);
                r.s.map_start();
                assert!(!r.s.arrive("Gordon", SimTime(1.0)), "back, not new");
                continued_by(&r.say(10.0, &gordon, question), 2)
            })
            .count();
        assert!(carried >= 90, "the talk goes on over the map change: {carried}");
        let later = (0..100)
            .filter(|&seed| {
                let mut r = start(seed);
                r.bots[0].busy = Some(Priority::Answer);
                assert!(!continued_by(&r.say(9.0, &gordon, question), 2), "the bot is typing");
                assert!(r.ready(10.0, true).is_empty(), "still typing");
                r.bots[0].busy = None;
                let out = r.ready(14.0, true);
                assert!(!r.d.has_waiting());
                continued_by(&out, 2)
            })
            .count();
        assert!(later >= 90, "the answer waits for the bot: {later}");
    }

    #[test]
    fn the_same_line_again_is_answered_once() {
        let gordon = who(1, "Gordon", false);
        for seed in 0..50 {
            let mut r = Room::new(seed, vec![speaker(2, "Plutonium"), speaker(3, "Kleiner")], roomy());
            let named: usize = (0..8)
                .map(|i| r.say(1.0 + 3.0 * f64::from(i), &gordon, "плутон, где рельсы?").len())
                .sum();
            assert_eq!(named, 1, "seed {seed}");
            let unnamed: usize = (0..8)
                .map(|i| r.say(400.0 + 3.0 * f64::from(i), &gordon, "а где тут квад?").len())
                .sum();
            assert!(unnamed <= 1, "seed {seed}: {unnamed}");
        }
    }

    #[test]
    fn binds_and_laughs_in_a_talk_are_not_answered() {
        let gordon = who(1, "Gordon", false);
        let noise = ["RUUUUN!!!1", "ахахаха", "LOL :)", "FUCK OFF!", "gg_cold_rock", "%l"];
        let mut asked = 0;
        for seed in 0..100 {
            let mut r = Room::new(seed, vec![speaker(2, "Plutonium")], roomy());
            r.say(1.0, &gordon, "плутон, привет");
            r.bot_says(4.0, 2, Some(&gordon), "привет");
            for (i, line) in noise.iter().enumerate() {
                let out = r.say(5.0 + i as f64, &gordon, line);
                assert!(out.is_empty(), "{line}: {out:?}");
            }
            let out = r.say(20.0, &gordon, "???");
            assert!(out.iter().all(|s| matches!(s.trigger, Trigger::Continued { .. })));
            asked += out.len();
        }
        assert!(
            (40..=80).contains(&asked),
            "a lone `???` in a talk, 0.6 of the time: {asked}"
        );
    }

    #[test]
    fn lines_in_other_languages_get_nothing_but_english_ones() {
        let (gordon, ali) = (who(1, "Gordon", false), who(4, "Ali", false));
        let mut english = 0;
        for seed in 0..50 {
            let mut r = Room::new(seed, vec![speaker(2, "Plutonium")], roomy());
            assert!(r.say(1.0, &ali, "Plutonium naber kanka").is_empty());
            english += r.say(2.0, &gordon, "Plutonium, where are you from?").len();
            assert!(
                r.say(30.0, &ali, "plutonium gg").is_empty(),
                "a line of no language is the player's last one"
            );
        }
        assert!(english >= 42, "{english}");
    }

    /// How many of 100 seeds Plutonium answers Gordon's `last` line by name, after his `before` lines 10 s apart,
    /// with `here` on the server and the server's language `language`.
    fn named_answers(language: &str, here: &[&str], before: &[&str], last: &str) -> usize {
        let gordon = who(1, "Gordon", false);
        let limits = Limits {
            language: language.into(),
            ..roomy()
        };
        (0..100)
            .filter(|&seed| {
                let mut r = Room::new(seed, vec![speaker(2, "Plutonium")], limits.clone());
                for name in here {
                    r.s.arrive(name, SimTime::ZERO);
                }
                for (i, line) in before.iter().enumerate() {
                    r.say(1.0 + 10.0 * i as f64, &gordon, line);
                }
                r.say(30.0, &gordon, last)
                    .iter()
                    .any(|s| s.slot == 2 && matches!(s.trigger, Trigger::Addressed { .. }))
            })
            .count()
    }

    #[test]
    fn nicknames_tell_no_language() {
        let n = named_answers("ru", &["=Glücksritter="], &["gg Glücksritter"], "Plutonium gg");
        assert!(n >= 85, "a nickname with ü the line before: {n}");
        let n = named_answers("ru", &["Ben"], &[], "plutonium, kill ben");
        assert!(n >= 85, "a player named Ben, a Turkish word: {n}");
        let n = named_answers("ru", &[], &["naber kanka"], "Plutonium gg");
        assert_eq!(n, 0, "a Turkish line the line before");
    }

    #[test]
    fn a_line_naming_two_bots_is_answered_by_both() {
        let gordon = who(1, "Gordon", false);
        let bots = vec![speaker(2, "Plutonium"), speaker(3, "Kleiner")];
        let both = (0..50)
            .filter(|&seed| {
                let mut r = Room::new(seed, bots.clone(), roomy());
                r.say(1.0, &gordon, "kleiner, plutonium, где вы?").len() == 2
            })
            .count();
        assert!(both >= 35, "{both}");
    }

    #[test]
    fn a_cyrillic_server_answers_cyrillic_lines() {
        for line in ["Plutonium, привіт, як справи?", "Plutonium, привет, как дела?"] {
            let n = named_answers("uk", &[], &[], line);
            assert!(n >= 85, "{line}: {n}");
        }
        assert_eq!(named_answers("uk", &[], &[], "Plutonium naber kanka"), 0, "Turkish");
    }

    #[test]
    fn a_hello_is_answered_once_in_three_quarters_of_an_hour() {
        let gordon = who(1, "Gordon", false);
        let bots = vec![chatty(2, "Plutonium"), chatty(3, "Kleiner")];
        let mut greeted = 0;
        let mut again = 0;
        for seed in 0..100 {
            let mut r = Room::new(seed, bots.clone(), roomy());
            let out = r.say(1.0, &gordon, "прив всем");
            assert!(out.len() <= 1);
            let Some(hello) = out.first() else {
                continue;
            };
            assert!(matches!(hello.trigger, Trigger::Greeted { .. }));
            assert!(hello.unasked && hello.priority == Priority::Greeting);
            greeted += 1;
            for (t, line) in [(600.0, "всем привет"), (2000.0, "hi all")] {
                assert!(r.say(t, &gordon, line).is_empty(), "{line}");
            }
            again += r.say(2800.0, &gordon, "hello everyone").len();
        }
        assert!((45..=75).contains(&greeted), "chatty(0.3) of a bot of 1.0: {greeted}");
        assert!(again > 0, "after 45 min a hello is answered again");

        let joined = (0..200)
            .filter_map(|seed| {
                let mut r = Room::new(seed, bots.clone(), roomy());
                let out = r.cause(1.0, &Cause::Join { who: gordon.clone() });
                (!out.is_empty()).then(|| r.say(5.0, &gordon, "прив всем"))
            })
            .collect::<Vec<_>>();
        assert!(!joined.is_empty());
        assert!(joined.iter().all(Vec::is_empty), "greeted when joining, not again");
    }

    #[test]
    fn unasked_lines_keep_to_the_hourly_bucket() {
        let limits = Limits {
            remarks_per_hour: 6.0,
            ..roomy()
        };
        let mut r = Room::new(5, vec![chatty(2, "Plutonium")], limits);
        let gordon = who(1, "Gordon", false);
        let (mut unasked, mut answers) = (0, 0);
        for i in 0..360 {
            let t = 10.0 * f64::from(i);
            let joined = who(5, &format!("player{i}"), false);
            for s in r.cause(t, &Cause::Join { who: joined }) {
                assert!(s.unasked);
                unasked += 1;
            }
            let blast = Notable::OwnBlast {
                victim: r.bots[0].who.clone(),
                weapon: "satchel".into(),
            };
            unasked += r.cause(t + 1.0, &Cause::Notable(blast)).len();
            if i % 6 == 0 {
                let out = r.say(t + 2.0, &gordon, &format!("плутон, вопрос {i}?"));
                assert!(out.iter().all(|s| !s.unasked));
                answers += out.len();
            }
        }
        assert!((6..=8).contains(&unasked), "two at once, six an hour: {unasked}");
        assert!(answers >= 50, "answers keep to no hourly bucket: {answers}");
    }

    #[test]
    fn moments_among_bots_and_gungame_crowbars_say_nothing() {
        let bots = vec![chatty(2, "Plutonium"), chatty(3, "Kleiner")];
        let (pluto, kleiner, gordon) = (bots[0].who.clone(), bots[1].who.clone(), who(1, "Gordon", false));
        let kill = |j: &mut Journal, t: f64, k: &Who, v: &Who, weapon: &str| {
            let event = Event::Kill {
                killer: k.clone(),
                victim: v.clone(),
                weapon: weapon.into(),
            };
            j.push(SimTime(t), event)
        };
        let mut among_bots = Journal::new("crossfire", SimTime::ZERO);
        let mut notables = Vec::new();
        for i in 0..12 {
            notables.extend(kill(&mut among_bots, 1.0 + f64::from(i), &pluto, &kleiner, "9mmAR"));
        }
        for i in 0..3 {
            notables.extend(kill(&mut among_bots, 20.0 + f64::from(i), &kleiner, &pluto, "crowbar"));
        }
        notables.extend(kill(&mut among_bots, 30.0, &pluto, &kleiner, "crossbow"));
        for t in [40.0, 50.0, 60.0] {
            notables.extend(kill(&mut among_bots, t, &pluto, &gordon, "gauss"));
        }
        notables.extend(among_bots.push(SimTime(61.0), Event::Leave { who: gordon.clone() }));
        assert!(
            notables
                .iter()
                .any(|n| matches!(n, Notable::Streak { count, .. } if *count >= STREAK_SPOKEN))
        );
        assert!(notables.iter().any(|n| matches!(n, Notable::Nemesis { .. })));
        assert!(notables.iter().any(|n| matches!(n, Notable::RageQuit { .. })));
        let mut gungame = Journal::new("gg_cold_rock", SimTime::ZERO);
        gungame.gungame = true;
        let crowbars = [
            kill(&mut gungame, 1.0, &gordon, &pluto, "crowbar"),
            kill(&mut gungame, 2.0, &pluto, &gordon, "crowbar"),
        ];
        assert!(
            crowbars
                .iter()
                .flatten()
                .all(|n| !matches!(n, Notable::Humiliation { .. }))
        );
        notables.extend(crowbars.into_iter().flatten());
        for seed in 0..200 {
            let mut r = Room::new(seed, bots.clone(), roomy());
            for (i, n) in notables.iter().enumerate() {
                let out = r.cause(100.0 + i as f64, &Cause::Notable(n.clone()));
                assert!(out.is_empty(), "{n:?}: {out:?}");
            }
        }
    }

    #[test]
    fn a_human_s_moments_get_a_word_now_and_then() {
        let bots = vec![chatty(2, "Plutonium")];
        let (pluto, gordon) = (bots[0].who.clone(), who(1, "Gordon", false));
        let streak = |count, humans| Notable::Streak {
            killer: pluto.clone(),
            count,
            humans,
        };
        let revenge = |run| Notable::Revenge {
            killer: pluto.clone(),
            victim: gordon.clone(),
            run,
        };
        let said = |n: &Notable| {
            (0..200)
                .filter(|&seed| {
                    !Room::new(seed, bots.clone(), roomy())
                        .cause(1.0, &Cause::Notable(n.clone()))
                        .is_empty()
                })
                .count()
        };
        assert_eq!(
            said(&streak(STREAK_SPOKEN - 1, 5)),
            0,
            "a streak of its own from STREAK_SPOKEN"
        );
        assert!(said(&streak(STREAK_SPOKEN, 1)) > 0);
        assert_eq!(
            said(&revenge(NEMESIS - 1)),
            0,
            "a revenge after NEMESIS deaths in a row"
        );
        assert!(said(&revenge(NEMESIS)) > 0);
        let nemesis = Notable::Nemesis {
            killer: gordon.clone(),
            victim: pluto.clone(),
            times: 3,
        };
        assert!(said(&nemesis) > 0);
        let gloat = Notable::Nemesis {
            killer: pluto.clone(),
            victim: gordon.clone(),
            times: 3,
        };
        assert_eq!(said(&gloat), 0, "no gloating");
        let typing = |killer: Option<Who>| Cause::KilledWhileTyping { bot: 2, killer };
        let by = |cause: &Cause| {
            (0..200)
                .filter(|&seed| !Room::new(seed, bots.clone(), roomy()).cause(1.0, cause).is_empty())
                .count()
        };
        assert!(by(&typing(Some(gordon.clone()))) > 0 && by(&typing(None)) > 0);
        assert_eq!(by(&typing(Some(who(4, "[BOT] Other", true)))), 0, "killed by a bot");
    }

    #[test]
    fn joins_are_greeted_now_and_then() {
        let bots = vec![speaker(2, "Plutonium"), speaker(3, "Kleiner")];
        let joined = (0..1000)
            .filter(|&seed| {
                let mut r = Room::new(seed, bots.clone(), limits());
                let out = r.cause(
                    1.0,
                    &Cause::Join {
                        who: who(1, "Gordon", false),
                    },
                );
                out.iter().all(|s| s.unasked && s.priority == Priority::Greeting) && !out.is_empty()
            })
            .count();
        assert!((110..=190).contains(&joined), "chatty(0.15): {joined}");
    }

    #[test]
    fn lines_to_everybody_keep_their_distance() {
        let limits = Limits {
            remark_gap: 60.0,
            ..roomy()
        };
        let (gordon, barney) = (who(1, "Gordon", false), who(4, "Barney", false));
        let mut times: Vec<(f64, i32)> = Vec::new();
        for seed in 0..5 {
            let mut r = Room::new(seed, vec![chatty(2, "Plutonium")], limits.clone());
            let mut last: Option<f64> = None;
            let mut mine: Vec<(f64, i32)> = Vec::new();
            for i in 0..400 {
                let t = 5.0 * f64::from(i);
                let from = if i % 2 == 0 { &gordon } else { &barney };
                for s in r.say(t, from, &format!("сегодня карта номер {i} опять")) {
                    assert!(matches!(s.trigger, Trigger::Overheard { about_bots: false, .. }) && s.unasked);
                    assert!(last.is_none_or(|l| t - l >= 60.0), "remark_gap: {last:?} {t}");
                    last = Some(t);
                    mine.push((t, from.userid));
                }
            }
            for player in [gordon.userid, barney.userid] {
                let at: Vec<f64> = mine.iter().filter(|(_, p)| *p == player).map(|(t, _)| *t).collect();
                assert!(at.windows(2).all(|w| w[1] - w[0] >= 240.0), "{at:?}");
            }
            times.extend(mine);
        }
        assert!(times.len() >= 10, "{times:?}");
    }

    #[test]
    fn a_word_to_another_human_is_left_alone() {
        let gordon = who(1, "Gordon", false);
        let bots = vec![chatty(2, "Plutonium")];
        for seed in 0..100 {
            let mut r = Room::new(seed, bots.clone(), roomy());
            r.s.arrive("Gordon", SimTime::ZERO);
            r.s.arrive("Brek", SimTime::ZERO);
            assert!(r.say(1.0, &gordon, "брек, го дуэль на рельсах").is_empty());
            let p = &r.s.players;
            assert!(p.aside("Gordon", SimTime(30.0)) && p.aside("Brek", SimTime(30.0)));
            assert!(!p.aside("Gordon", SimTime(61.0)));
            assert!(
                r.say(2.0, &gordon, "плутониум, брек за путина").is_empty(),
                "touchy, and about another human"
            );
        }
        let touchy_to_the_bot = (0..50)
            .filter(|&seed| {
                let mut r = Room::new(seed, bots.clone(), roomy());
                r.s.arrive("Brek", SimTime::ZERO);
                !r.say(1.0, &gordon, "плутониум, а ты за путина?").is_empty()
            })
            .count();
        assert!(touchy_to_the_bot >= 40, "{touchy_to_the_bot}");
        let mut r = Room::new(3, bots.clone(), roomy());
        r.s.arrive("Brek", SimTime::ZERO);
        r.say(1.0, &gordon, "плутон, привет");
        r.bot_says(3.0, 2, Some(&gordon), "привет");
        assert!(
            r.say(5.0, &gordon, "брек тоже тут, кстати").is_empty(),
            "called out first"
        );
        assert!(r.s.players.aside("Brek", SimTime(6.0)));
        let goes_on = (0..100)
            .filter(|&seed| {
                let mut r = Room::new(seed, bots.clone(), roomy());
                r.s.arrive("Brek", SimTime::ZERO);
                r.say(1.0, &gordon, "плутон, привет");
                r.bot_says(3.0, 2, Some(&gordon), "привет");
                assert!(
                    r.say(4.0, &gordon, "а брек за путина").is_empty(),
                    "touchy, and about another human, in a talk too"
                );
                continued_by(&r.say(5.0, &gordon, "а ты видел, как брек играет?"), 2)
            })
            .count();
        assert!(goes_on >= 85, "a name inside the line keeps the talk: {goes_on}");
    }

    #[test]
    fn questions_go_to_the_bot_they_follow() {
        let (gordon, barney) = (who(1, "Gordon", false), who(4, "Barney", false));
        let bots = vec![speaker(2, "Plutonium"), speaker(3, "Kleiner")];
        let (mut to_bot, mut after_human, mut general) = (0, 0, 0);
        for seed in 0..200 {
            let mut r = Room::new(seed, bots.clone(), roomy());
            r.bot_says(1.0, 3, None, "кто со мной на рельсы");
            if let [s] = &r.say(5.0, &gordon, "а ты где был?")[..] {
                assert!(s.slot == 3 && matches!(s.trigger, Trigger::Question { to_me: true, .. }));
                to_bot += 1;
            }
            let mut r = Room::new(seed, bots.clone(), roomy());
            r.say(1.0, &barney, "я на рельсах сижу");
            after_human += r.say(5.0, &gordon, "а ты где был?").len();
            let mut r = Room::new(seed, bots.clone(), roomy());
            let out = r.say(1.0, &gordon, "кто лидер сейчас?");
            if let [s] = &out[..] {
                assert!(matches!(s.trigger, Trigger::Question { to_me: false, .. }) && !s.unasked);
                general += 1;
                assert!(
                    r.say(60.0, &gordon, "а сколько до конца?").is_empty(),
                    "one a player in two minutes"
                );
            }
        }
        assert!(to_bot >= 150, "0.85 after the bot's line: {to_bot}");
        assert!(after_human <= 40, "0.1 after another human's line: {after_human}");
        assert!((60..=120).contains(&general), "chatty(0.45): {general}");
    }

    #[test]
    fn answers_wait_for_a_busy_bot_a_little() {
        let gordon = who(1, "Gordon", false);
        let held = |seed: u64| {
            let mut r = Room::new(seed, vec![speaker(2, "Kleiner")], limits());
            r.bots[0].busy = Some(Priority::Answer);
            assert!(r.say(1.0, &gordon, "kleiner, где ты?").is_empty(), "the bot is typing");
            r
        };
        let text = |r: &Room| r.d.waiting[0].speak.trigger.line().map(|(_, t)| t.to_string());
        let newer = "kleiner, ау, ответь";
        let mut r = (0..)
            .map(|seed| {
                let mut r = held(seed);
                let first = r.d.has_waiting();
                r.say(3.0, &gordon, newer);
                (first, r)
            })
            .find(|(first, r)| *first && r.d.waiting.len() == 1 && text(r).as_deref() == Some(newer))
            .map(|(_, r)| r)
            .unwrap();
        assert!(r.ready(5.0, true).is_empty(), "still typing");
        r.bots[0].busy = Some(Priority::Remark);
        let out = r.ready(6.0, true);
        assert!(matches!(&out[..], [s] if s.slot == 2 && s.trigger.line().is_some_and(|(_, t)| t == newer)));
        assert!(!r.d.has_waiting());
        assert!(
            r.say(8.0, &gordon, newer).is_empty(),
            "answered, the same line is a repeat"
        );

        let mut r = (0..).map(held).find(|r| r.d.has_waiting()).unwrap();
        r.bots[0].busy = None;
        assert!(r.ready(32.0, true).is_empty() && !r.d.has_waiting(), "too late");
        let mut r = (0..).map(held).find(|r| r.d.has_waiting()).unwrap();
        r.bots[0].busy = None;
        assert!(
            r.ready(2.0, false).is_empty() && !r.d.has_waiting(),
            "nobody to read it"
        );
        let mut r = (0..).map(held).find(|r| r.d.has_waiting()).unwrap();
        r.bots[0] = speaker(2, "Gina");
        assert!(
            r.ready(2.0, true).is_empty() && !r.d.has_waiting(),
            "another bot in the slot"
        );
        let mut r = (0..).map(held).find(|r| r.d.has_waiting()).unwrap();
        r.d.clear_waiting();
        assert!(!r.d.has_waiting());

        let mut r = Room::new(1, vec![chatty(2, "Kleiner")], roomy());
        r.bots[0].busy = Some(Priority::Answer);
        for i in 0..50 {
            r.say(300.0 * f64::from(i), &gordon, &format!("сегодня карта номер {i} опять"));
        }
        assert!(!r.d.has_waiting(), "only lines spoken to the bot wait");
    }

    #[test]
    fn remarks_keep_their_distance() {
        let bots = [speaker(2, "Kleiner")];
        let limits = Limits {
            remarks_per_hour: 600.0,
            ..limits()
        };
        let mut j = Journal::new("crossfire", SimTime(0.0));
        let blast = j.push(
            SimTime(1.0),
            Event::Suicide {
                victim: bots[0].who.clone(),
                weapon: "satchel".into(),
            },
        );
        let cause = Cause::Notable(blast[0].clone());
        let said = (0..200u64)
            .filter(|&seed| {
                let mut s = Social::default();
                let mut d = Director::new(seed, SimTime(0.0), &limits);
                !d.react(SimTime(2.0), &cause, &bots, &j, &mut s, true, &limits)
                    .is_empty()
            })
            .count();
        assert!((30..=90).contains(&said), "about 0.3 of the time: {said}");
        let mut s = Social::default();
        let mut d = Director::new(0, SimTime(0.0), &limits);
        let mut bots_now = bots.clone();
        let mut times = Vec::new();
        for i in 0..1200 {
            let t = SimTime(f64::from(i));
            if !d.react(t, &cause, &bots_now, &j, &mut s, true, &limits).is_empty() {
                bots_now[0].last_remark = Some(t);
                times.push(i);
            }
        }
        assert!(times.len() >= 2, "{times:?}");
        assert!(times.windows(2).all(|w| w[1] - w[0] >= 240), "{times:?}");
    }

    #[test]
    fn match_end_and_tests() {
        let bots: Vec<Speaker> = ["a", "b", "c", "d", "e"]
            .iter()
            .zip(2..)
            .map(|(name, slot)| speaker(slot, name))
            .collect();
        let winner = Some(bots[0].who.clone());
        let end = Cause::MatchEnd { winner };
        let (mut most, mut most_tight) = (0, 0);
        for seed in 0..100 {
            let out = Room::new(seed, bots.clone(), roomy()).cause(1.0, &end);
            assert!(out.iter().all(|s| !s.unasked && s.priority == Priority::MatchEnd));
            most = most.max(out.len());
            most_tight = most_tight.max(Room::new(seed, bots.clone(), limits()).cause(1.0, &end).len());
        }
        assert_eq!(most, MATCH_END_BOTS + 1);
        assert_eq!(most_tight, 1, "one line of two is kept for answers");
        let test = Cause::Test {
            bot: 3,
            from: who(1, "admin", false),
            text: "привет".into(),
        };
        let j = Journal::new("crossfire", SimTime(0.0));
        let mut d = Director::new(1, SimTime(0.0), &limits());
        let out = d.react(SimTime(1.0), &test, &bots, &j, &mut Social::default(), false, &limits());
        assert_eq!(out.len(), 1, "tests need no humans");
    }

    /// Gordon names Plutonium at 1 s, and it says `answer` to him at 5 s: a talk.
    fn talk(seed: u64, answer: &str) -> Room {
        let gordon = who(1, "Gordon", false);
        let mut r = Room::new(seed, vec![speaker(2, "Plutonium")], roomy());
        r.say(1.0, &gordon, "плутон, привет");
        r.bot_says(5.0, 2, Some(&gordon), answer);
        r
    }

    #[test]
    fn questions_and_replies_in_a_talk_are_answered_again() {
        let gordon = who(1, "Gordon", false);
        let again: Vec<bool> = (0..)
            .filter_map(|seed| {
                let mut r = talk(seed, "привет, как сам?");
                if !continued_by(&r.say(10.0, &gordon, "а ты?"), 2) {
                    return None;
                }
                r.bot_says(14.0, 2, Some(&gordon), "да норм");
                r.say(1800.0, &gordon, "плутон, го на рельсы");
                r.bot_says(1805.0, 2, Some(&gordon), "го");
                Some(continued_by(&r.say(1810.0, &gordon, "а ты?"), 2))
            })
            .take(100)
            .collect();
        let n = again.iter().filter(|&&on| on).count();
        assert!(n >= 85, "a question answered in a talk 30 min before: {n}");

        let (mut first, mut second) = (0, 0);
        for seed in 0..300 {
            let mut r = talk(seed, "привет, го на рельсы?");
            if !continued_by(&r.say(10.0, &gordon, "да"), 2) {
                continue;
            }
            first += 1;
            assert!(
                r.say(16.0, &gordon, "да").is_empty(),
                "seed {seed}: the same reply to the same question"
            );
            r.bot_says(65.0, 2, Some(&gordon), "а потом на квад?");
            second += usize::from(continued_by(&r.say(70.0, &gordon, "да"), 2));
        }
        assert!(
            (first * 45..=first * 75).contains(&(second * 100)),
            "the same reply to the bot's next question, 0.6: {second} of {first}"
        );

        let again: Vec<bool> = (0..)
            .filter_map(|seed| {
                let mut r = talk(seed, "привет");
                if !r.say(10.0, &gordon, "а ты где сейчас?").is_empty() {
                    return None;
                }
                Some(continued_by(&r.say(35.0, &gordon, "а ты где сейчас?"), 2))
            })
            .take(100)
            .collect();
        let n = again.iter().filter(|&&on| on).count();
        assert!(n >= 85, "a question nobody answered, asked again: {n}");

        let (mut held, mut carried) = (0, 0);
        for seed in 0..100 {
            let mut r = talk(seed, "привет");
            r.bots[0].busy = Some(Priority::Answer);
            r.say(100.0, &gordon, "а ты где сейчас?");
            if !r.d.has_waiting() {
                continue;
            }
            held += 1;
            r.s.present(&["Gordon".into()], SimTime(105.0));
            r.s.map_end(SimTime(105.0), &[gordon.userid]);
            r.d = Director::new(seed + 1000, SimTime::ZERO, &r.limits);
            r.j = Journal::new("stalkyard", SimTime::ZERO);
            r.s.map_start();
            r.bots[0].busy = None;
            r.s.arrive("Gordon", SimTime(1.0));
            carried += usize::from(continued_by(&r.say(30.0, &gordon, "а ты где сейчас?"), 2));
        }
        assert!(held >= 85, "{held}");
        assert!(
            carried * 100 >= held * 85,
            "a held question lost with the map, asked again: {carried} of {held}"
        );

        let late = (0..200)
            .filter(|&seed| {
                continued_by(
                    &talk(seed, "привет").say(100.0, &gordon, "только что зашёл с работы"),
                    2,
                )
            })
            .count();
        assert!(late >= 150, "0.75 + 0.2c while the talk lasts: {late}");
    }

    #[test]
    fn binds_in_a_talk_get_an_answer_once_or_twice() {
        let gordon = who(1, "Gordon", false);
        let mut answered_once = 0;
        for seed in 0..100 {
            let mut r = talk(seed, "привет");
            let mut at = Vec::new();
            for i in 0..40 {
                let t = 10.0 + 30.0 * f64::from(i);
                if !r.say(t, &gordon, "кто на рельсы?").is_empty() {
                    at.push(t);
                    r.bot_says(t + 4.0, 2, Some(&gordon), "я пойду");
                }
            }
            assert!(at.windows(2).all(|w| w[1] - w[0] >= 300.0), "seed {seed}: {at:?}");
            assert!(
                r.s.talks.partner(SimTime(1200.0), gordon.userid, |_| true).is_none(),
                "seed {seed}: the talk is over"
            );
            answered_once += usize::from(!at.is_empty());
        }
        assert!(answered_once >= 90, "{answered_once}");
        for seed in 0..100 {
            let mut r = talk(seed, "привет");
            let mut answers = 0;
            for i in 0..40 {
                let t = 10.0 + 30.0 * f64::from(i);
                if !r.say(t, &gordon, "ЛОВИ ПОДАРОЧЕК!!!").is_empty() {
                    answers += 1;
                    r.bot_says(t + 4.0, 2, Some(&gordon), "держи ответку");
                }
            }
            assert!(answers <= 2, "seed {seed}: {answers}");
        }
    }

    #[test]
    fn a_short_reply_to_the_bot_s_question_is_answered_more_often() {
        let gordon = who(1, "Gordon", false);
        let answered = |asked: &str, reply: &str| {
            (0..300)
                .filter(|&seed| continued_by(&talk(seed, asked).say(10.0, &gordon, reply), 2))
                .count()
        };
        for asked in ["го на рельсы?", "ну что, ещё раунд?)", "го 1на1? :)"] {
            for reply in ["да", "+", "--"] {
                let n = answered(asked, reply);
                assert!((150..=210).contains(&n), "0.6 after {asked:?}: {reply:?} {n}");
            }
        }
        assert_eq!(answered("привет", "+"), 0, "a plus to no question is noise");
        let n = answered("привет", "да");
        assert!(n <= 70, "chatty(0.15) to no question: {n}");
        assert_eq!(answered("го на рельсы?", "+++++"), 0, "signs, not a yes");
    }

    #[test]
    fn the_same_words_to_another_bot_are_no_repeat() {
        let gordon = who(1, "Gordon", false);
        let bots = vec![speaker(2, "Plutonium"), speaker(3, "Kleiner")];
        let addressed = |out: &[Speak], slot: u8| {
            out.iter()
                .any(|s| s.slot == slot && matches!(s.trigger, Trigger::Addressed { .. }))
        };
        for (first, second) in [("плутон, ты где?", "kleiner, ты где?"), ("gg Plutonium", "gg Kleiner")]
        {
            let (mut asked, mut again) = (0, 0);
            for seed in 0..200 {
                let mut r = Room::new(seed, bots.clone(), roomy());
                if !addressed(&r.say(1.0, &gordon, first), 2) {
                    continue;
                }
                asked += 1;
                let out = r.say(20.0, &gordon, second);
                again += usize::from(addressed(&out, 3));
                assert!(!addressed(&out, 2));
                for t in [23.0, 26.0, 29.0] {
                    assert!(r.say(t, &gordon, first).is_empty(), "seed {seed}: {first:?} again");
                }
            }
            assert!(asked >= 150, "{first:?}: {asked}");
            assert!(
                again * 100 >= asked * 85,
                "{second:?} after {first:?} was answered: {again} of {asked}"
            );
        }
        let (mut greeted, mut named) = (0, 0);
        for seed in 0..300 {
            let mut r = Room::new(seed, vec![chatty(2, "Plutonium"), chatty(3, "Kleiner")], roomy());
            if !r.say(1.0, &gordon, "привет").iter().any(|s| s.slot == 3) {
                continue;
            }
            greeted += 1;
            named += usize::from(addressed(&r.say(60.0, &gordon, "Привет Плутон!"), 2));
        }
        assert!(greeted >= 30, "{greeted}");
        assert!(
            named * 100 >= greeted * 85,
            "a hello by name after another bot's greeting: {named} of {greeted}"
        );
        for seed in 0..50 {
            let mut r = Room::new(seed, bots.clone(), roomy());
            if !addressed(&r.say(1.0, &gordon, "плутон, ты где?"), 2) {
                continue;
            }
            assert!(
                r.say(5.0, &gordon, "плутон, kleiner, ты где?").is_empty(),
                "one of the bots named answered it"
            );
        }
        let (mut held, mut answered) = (0, 0);
        for seed in 0..100 {
            let mut r = Room::new(seed, bots.clone(), roomy());
            r.bots[1].busy = Some(Priority::Answer);
            r.say(1.0, &gordon, "kleiner, ты где?");
            if !r.d.has_waiting() {
                continue;
            }
            held += 1;
            assert!(
                r.say(4.0, &gordon, "kleiner, ты где?").is_empty(),
                "it waits for Kleiner"
            );
            answered += usize::from(addressed(&r.say(7.0, &gordon, "плутон, ты где?"), 2));
        }
        assert!(held >= 85, "{held}");
        assert!(
            answered * 100 >= held * 85,
            "only Kleiner holds it: {answered} of {held}"
        );
    }

    #[test]
    fn no_remarks_an_hour_leaves_the_answers() {
        let none = Limits {
            remarks_per_hour: 0.0,
            ..roomy()
        };
        let bots = vec![chatty(2, "Plutonium")];
        let (gordon, barney, pluto) = (who(1, "Gordon", false), who(4, "Barney", false), bots[0].who.clone());
        let run = |limits: &Limits, seed: u64| {
            let mut r = Room::new(seed, bots.clone(), limits.clone());
            let blast = Notable::OwnBlast {
                victim: pluto.clone(),
                weapon: "satchel".into(),
            };
            let unasked = [
                r.cause(1.0, &Cause::Join { who: barney.clone() }),
                r.say(2.0, &barney, "прив всем"),
                r.say(3.0, &gordon, "сегодня карта номер один опять"),
                r.cause(4.0, &Cause::Notable(blast)),
                r.cause(5.0, &Cause::LastLevel { bot: 2 }),
            ]
            .map(|out| out.len());
            let addressed = r.say(6.0, &gordon, "плутон, где рельсы?");
            r.bot_says(8.0, 2, Some(&gordon), "на втором этаже");
            let continued = r.say(10.0, &gordon, "а ты там был?");
            let question = r.say(12.0, &barney, "кто лидер сейчас?");
            let end = r.cause(
                14.0,
                &Cause::MatchEnd {
                    winner: Some(pluto.clone()),
                },
            );
            let asked = [
                usize::from(addressed.iter().any(|s| matches!(s.trigger, Trigger::Addressed { .. }))),
                usize::from(continued_by(&continued, 2)),
                usize::from(question.iter().any(|s| matches!(s.trigger, Trigger::Question { .. }))),
                end.len(),
            ];
            (unasked, asked)
        };
        let (mut unasked, mut asked) = ([0; 5], [0; 4]);
        for seed in 0..200 {
            let (u, a) = run(&none, seed);
            assert_eq!(
                u, [0; 5],
                "seed {seed}: join, hello, line to everybody, moment, last level"
            );
            asked = std::array::from_fn(|i| asked[i] + a[i]);
            let (u, _) = run(&roomy(), seed);
            unasked = std::array::from_fn(|i| unasked[i] + u[i]);
        }
        assert!(
            asked.iter().all(|&n| n > 0),
            "named, in a talk, a question, the match end: {asked:?}"
        );
        assert!(unasked.iter().all(|&n| n > 0), "with remarks an hour: {unasked:?}");
    }
}
