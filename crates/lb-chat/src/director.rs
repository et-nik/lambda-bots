//! Who says something, and whether anyone does: players spoken to answer, a remark now and then, never a flood.
//! Bots never answer bots; nobody speaks while no human is there to read it.

use lb_core::rng::Pcg32;
use lb_core::time::SimTime;

use crate::addressing;
use crate::botchat::Priority;
use crate::journal::{Journal, Notable, Who};
use crate::request::Trigger;

/// A player's line within this many seconds of a bot's line to them continues the talk.
const TALK_WINDOW: f64 = 20.0;
/// Seconds back a fight makes a bot the likelier one to answer a player.
const FOUGHT_WINDOW: f64 = 60.0;
/// Bots besides the winner who say something at the end of a match.
const MATCH_END_BOTS: usize = 2;

/// Volume limits, from `chat.limits`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Limits {
    pub lines_per_minute: f32,
    pub requests_per_minute: f32,
    pub remark_gap: f32,
    pub bot_remark_gap: f32,
}

/// A bot as the director weighs it.
#[derive(Clone, Debug, PartialEq)]
pub struct Speaker {
    pub who: Who,
    pub team: u8,
    pub chattiness: f32,
    pub alive: bool,
    /// The line on its way, if any.
    pub busy: Option<Priority>,
    pub last_remark: Option<SimTime>,
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
    /// A human joined (not one back from a map change).
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

    fn take(&mut self, now: SimTime, per_minute: f32, cap: f64) -> bool {
        self.tokens = (self.tokens + now.since(self.at).max(0.0) * f64::from(per_minute) / 60.0).min(cap);
        self.at = now;
        if self.tokens >= 1.0 {
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

#[derive(Clone, Debug)]
pub struct Director {
    rng: Pcg32,
    lines: Bucket,
    requests: Bucket,
    last_remark: Option<SimTime>,
    /// Bots' recent lines to players: (bot slot, player userid, when).
    talks: Vec<(u8, i32, SimTime)>,
}

fn caps(l: &Limits) -> (f64, f64) {
    (
        f64::from(l.lines_per_minute).max(2.0),
        f64::from(l.requests_per_minute / 2.0).max(3.0),
    )
}

impl Director {
    pub fn new(seed: u64, now: SimTime, limits: &Limits) -> Director {
        let (lines, requests) = caps(limits);
        Director {
            rng: Pcg32::new(seed, 0x6469_7265),
            lines: Bucket::full(lines, now),
            requests: Bucket::full(requests, now),
            last_remark: None,
            talks: Vec::new(),
        }
    }

    /// A bot said a line `to` a player (whose answer then continues the talk).
    pub fn said(&mut self, now: SimTime, bot: u8, to: Option<i32>) {
        self.talks
            .retain(|&(b, _, at)| b != bot && now.since(at) <= TALK_WINDOW);
        if let Some(to) = to {
            self.talks.push((bot, to, now));
        }
    }

    /// A request came back without a line: its line is not used up.
    pub fn refund(&mut self, limits: &Limits) {
        self.lines.give(caps(limits).0);
    }

    /// Who should say something about `cause`. `humans`: someone is there to read it.
    pub fn react(
        &mut self,
        now: SimTime,
        cause: &Cause,
        bots: &[Speaker],
        journal: &Journal,
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
            }];
        }
        if !humans {
            return Vec::new();
        }
        let wanted = self.candidates(now, cause, bots, journal, limits);
        let (line_cap, request_cap) = caps(limits);
        let mut out = Vec::new();
        for speak in wanted {
            let bot = bots.iter().find(|b| b.who.slot == speak.slot);
            if bot.is_some_and(|b| b.busy.is_some_and(|p| p >= speak.priority)) {
                continue;
            }
            if !self.lines.take(now, limits.lines_per_minute, line_cap) {
                break;
            }
            if !self.requests.take(now, limits.requests_per_minute, request_cap) {
                self.lines.give(line_cap);
                break;
            }
            if speak.priority == Priority::Remark {
                self.last_remark = Some(now);
            }
            out.push(speak);
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
        let gap = |last: Option<SimTime>, secs: f32| last.is_none_or(|t| now.since(t) >= f64::from(secs));
        gap(self.last_remark, limits.remark_gap) && gap(bot.last_remark, limits.bot_remark_gap)
    }

    /// One bot of `bots`, weighted.
    fn pick<'a>(&mut self, bots: impl Iterator<Item = (&'a Speaker, f32)>) -> Option<&'a Speaker> {
        let weighted: Vec<(&Speaker, f32)> = bots.filter(|(_, w)| *w > 0.0).collect();
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

    fn candidates(
        &mut self,
        now: SimTime,
        cause: &Cause,
        bots: &[Speaker],
        journal: &Journal,
        limits: &Limits,
    ) -> Vec<Speak> {
        let speak = |bot: &Speaker, trigger: Trigger, priority: Priority, team: bool| Speak {
            slot: bot.who.slot,
            trigger,
            priority,
            team,
        };
        match cause {
            Cause::Chat {
                from,
                text,
                team,
                from_team,
            } => {
                let hears = |b: &&Speaker| !*team || b.team == *from_team;
                let named: Vec<&Speaker> = bots
                    .iter()
                    .filter(hears)
                    .filter(|b| addressing::mentions(text, &b.who.name))
                    .collect();
                if !named.is_empty() {
                    return named
                        .into_iter()
                        .filter(|b| {
                            let p = 0.8 + 0.15 * b.chattiness;
                            self.chance(p)
                        })
                        .take(2)
                        .map(|b| {
                            let trigger = Trigger::Addressed {
                                from: from.clone(),
                                text: text.clone(),
                            };
                            speak(b, trigger, Priority::Answer, *team)
                        })
                        .collect();
                }
                let talking = self
                    .talks
                    .iter()
                    .rev()
                    .find(|&&(_, to, at)| to == from.userid && now.since(at) <= TALK_WINDOW)
                    .and_then(|&(slot, ..)| bots.iter().filter(hears).find(|b| b.who.slot == slot));
                if let Some(bot) = talking {
                    let p = 0.55 + 0.3 * bot.chattiness;
                    if self.chance(p) {
                        let trigger = Trigger::Continued {
                            from: from.clone(),
                            text: text.clone(),
                        };
                        return vec![speak(bot, trigger, Priority::Answer, *team)];
                    }
                    return Vec::new();
                }
                let to_all = addressing::to_bots(text);
                let since = SimTime(now.secs() - FOUGHT_WINDOW);
                let bot = self.pick(bots.iter().filter(hears).map(|b| {
                    let fought = if journal.fought(b.who.userid, from.userid, since) {
                        3.0
                    } else {
                        1.0
                    };
                    let free = if b.alive { 1.0 } else { 1.5 };
                    (b, b.chattiness * fought * free)
                }));
                let Some(bot) = bot else {
                    return Vec::new();
                };
                let (p, trigger, priority) = if to_all {
                    let trigger = Trigger::Addressed {
                        from: from.clone(),
                        text: text.clone(),
                    };
                    (0.8, trigger, Priority::Answer)
                } else {
                    let trigger = Trigger::Overheard {
                        from: from.clone(),
                        text: text.clone(),
                    };
                    (Self::chatty(0.2, bot), trigger, Priority::Greeting)
                };
                if self.chance(p) {
                    vec![speak(bot, trigger, priority, *team)]
                } else {
                    Vec::new()
                }
            }
            Cause::Join { who } => {
                let bot = self.pick(bots.iter().map(|b| (b, b.chattiness)));
                match bot {
                    Some(bot) if self.chance(Self::chatty(0.3, bot)) => {
                        vec![speak(
                            bot,
                            Trigger::Joined { who: who.clone() },
                            Priority::Greeting,
                            false,
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
                    && self.chance(0.8)
                {
                    let trigger = Trigger::MatchEnd {
                        winner: winner.clone(),
                        won: true,
                    };
                    out.push(speak(bot, trigger, Priority::MatchEnd, false));
                }
                let mut others = 0;
                for bot in bots {
                    if others >= MATCH_END_BOTS || won_bot.is_some_and(|w| w.who.slot == bot.who.slot) {
                        continue;
                    }
                    if self.chance(Self::chatty(0.4, bot)) {
                        others += 1;
                        let trigger = Trigger::MatchEnd {
                            winner: winner.clone(),
                            won: false,
                        };
                        out.push(speak(bot, trigger, Priority::MatchEnd, false));
                    }
                }
                out
            }
            Cause::KilledWhileTyping { bot, killer } => {
                let Some(b) = bots.iter().find(|b| b.who.slot == *bot) else {
                    return Vec::new();
                };
                if self.remark_allowed(now, b, limits) && self.chance(Self::chatty(0.5, b)) {
                    let trigger = Trigger::KilledWhileTyping { killer: killer.clone() };
                    return vec![speak(b, trigger, Priority::Remark, false)];
                }
                Vec::new()
            }
            Cause::LastLevel { bot } => {
                let Some(b) = bots.iter().find(|b| b.who.slot == *bot) else {
                    return Vec::new();
                };
                if self.remark_allowed(now, b, limits) && self.chance(Self::chatty(0.2, b)) {
                    return vec![speak(b, Trigger::LastLevel, Priority::Remark, false)];
                }
                Vec::new()
            }
            Cause::Notable(n) => self.notable(now, n, bots, limits),
            Cause::Test { .. } => Vec::new(),
        }
    }

    fn notable(&mut self, now: SimTime, n: &Notable, bots: &[Speaker], limits: &Limits) -> Vec<Speak> {
        let bot = |who: &Who| bots.iter().find(|b| b.who.userid == who.userid);
        // Who would say something and how likely: the bot it happened to, mostly.
        let (speaker, p) = match n {
            Notable::Nemesis { killer, victim, .. } => match (bot(victim), bot(killer)) {
                (Some(v), _) if !killer.bot => (Some(v), 0.4),
                (None, Some(k)) => (Some(k), 0.12),
                _ => (None, 0.0),
            },
            Notable::Humiliation { killer, victim } => match (bot(victim), bot(killer)) {
                (Some(v), _) if !killer.bot => (Some(v), 0.35),
                (None, Some(k)) => (Some(k), 0.15),
                _ => (None, 0.0),
            },
            Notable::OwnBlast { victim, .. } => match bot(victim) {
                Some(v) => (Some(v), 0.3),
                None => (self.pick(bots.iter().map(|b| (b, b.chattiness))), 0.08),
            },
            Notable::Revenge { killer, victim } if !victim.bot => (bot(killer), 0.2),
            Notable::Multikill { killer, count } => match bot(killer) {
                Some(k) => (Some(k), if *count >= 4 { 0.35 } else { 0.2 }),
                None => (self.pick(bots.iter().map(|b| (b, b.chattiness))), 0.08),
            },
            Notable::Streak { killer, count } => match bot(killer) {
                Some(k) => (Some(k), 0.12),
                None if *count >= 10 => (self.pick(bots.iter().map(|b| (b, b.chattiness))), 0.15),
                None => (None, 0.0),
            },
            Notable::RageQuit { .. } => (self.pick(bots.iter().map(|b| (b, b.chattiness))), 0.2),
            Notable::Revenge { .. } => (None, 0.0),
        };
        let Some(speaker) = speaker else {
            return Vec::new();
        };
        if !self.remark_allowed(now, speaker, limits) || !self.chance(Self::chatty(p, speaker)) {
            return Vec::new();
        }
        vec![Speak {
            slot: speaker.who.slot,
            trigger: Trigger::Notable(n.clone()),
            priority: Priority::Remark,
            team: false,
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::Event;

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
            team: 0,
            chattiness: 0.5,
            alive: true,
            busy: None,
            last_remark: None,
        }
    }

    fn limits() -> Limits {
        Limits {
            lines_per_minute: 2.0,
            requests_per_minute: 6.0,
            remark_gap: 60.0,
            bot_remark_gap: 240.0,
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

    #[test]
    fn named_bots_answer_and_nobody_talks_to_an_empty_server() {
        let bots = [speaker(2, "DUT9 ATLASA"), speaker(3, "Kleiner")];
        let human = who(1, "112S", false);
        let j = Journal::new("crossfire", SimTime(0.0));
        let mut d = Director::new(1, SimTime(0.0), &limits());
        let out = d.react(
            SimTime(1.0),
            &chat(&human, "Атлас, ты читер"),
            &bots,
            &j,
            false,
            &limits(),
        );
        assert!(out.is_empty());
        let mut answered = 0;
        for seed in 0..50 {
            let mut d = Director::new(seed, SimTime(0.0), &limits());
            let out = d.react(
                SimTime(1.0),
                &chat(&human, "Атлас, ты читер"),
                &bots,
                &j,
                true,
                &limits(),
            );
            if let Some(s) = out.first() {
                assert_eq!(s.slot, 2);
                assert_eq!(s.priority, Priority::Answer);
                assert!(matches!(s.trigger, Trigger::Addressed { .. }));
                answered += 1;
            }
        }
        assert!(answered >= 35, "{answered}");
    }

    #[test]
    fn conversations_continue_and_lines_are_rationed() {
        let bots = [speaker(2, "DUT9 ATLASA"), speaker(3, "Kleiner")];
        let human = who(1, "112S", false);
        let j = Journal::new("crossfire", SimTime(0.0));
        let mut d = Director::new(3, SimTime(0.0), &limits());
        d.said(SimTime(5.0), 3, Some(human.userid));
        let mut continued = 0;
        for i in 0..20 {
            let t = SimTime(6.0 + f64::from(i) * 0.1);
            let out = d.react(t, &chat(&human, "а ты кто такой"), &bots, &j, true, &limits());
            for s in &out {
                assert_eq!(s.slot, 3);
                assert!(matches!(s.trigger, Trigger::Continued { .. }));
            }
            continued += out.len();
        }
        assert!(continued <= 2, "the line bucket holds two: {continued}");
        let late = d.react(SimTime(200.0), &chat(&human, "эй"), &bots, &j, true, &limits());
        assert!(late.iter().all(|s| !matches!(s.trigger, Trigger::Continued { .. })));
    }

    #[test]
    fn busy_bots_keep_their_line_unless_answering() {
        let mut bots = [speaker(2, "Kleiner")];
        bots[0].busy = Some(Priority::Answer);
        let human = who(1, "x", false);
        let j = Journal::new("crossfire", SimTime(0.0));
        let mut d = Director::new(9, SimTime(0.0), &limits());
        for _ in 0..10 {
            assert!(
                d.react(SimTime(1.0), &chat(&human, "kleiner?"), &bots, &j, true, &limits())
                    .is_empty()
            );
        }
    }

    #[test]
    fn remarks_keep_their_distance() {
        let bots = [speaker(2, "Kleiner")];
        let human = who(1, "x", false);
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
                let mut d = Director::new(seed, SimTime(0.0), &limits());
                !d.react(SimTime(2.0), &cause, &bots, &j, true, &limits()).is_empty()
            })
            .count();
        assert!((30..=90).contains(&said), "about 0.3 of the time: {said}");
        let mut d = Director::new(0, SimTime(0.0), &limits());
        let mut bots_now = bots.clone();
        let mut times = Vec::new();
        for i in 0..400 {
            let t = SimTime(f64::from(i));
            if !d.react(t, &cause, &bots_now, &j, true, &limits()).is_empty() {
                bots_now[0].last_remark = Some(t);
                times.push(i);
            }
        }
        assert!(times.windows(2).all(|w| w[1] - w[0] >= 240), "{times:?}");
        let _ = human;
    }

    #[test]
    fn match_end_and_tests() {
        let bots = [
            speaker(2, "a"),
            speaker(3, "b"),
            speaker(4, "c"),
            speaker(5, "d"),
            speaker(6, "e"),
        ];
        let j = Journal::new("crossfire", SimTime(0.0));
        let mut most = 0;
        for seed in 0..100 {
            let mut d = Director::new(seed, SimTime(0.0), &limits());
            let winner = Some(bots[0].who.clone());
            let out = d.react(SimTime(1.0), &Cause::MatchEnd { winner }, &bots, &j, true, &limits());
            most = most.max(out.len());
        }
        assert!((2..=MATCH_END_BOTS + 1).contains(&most), "{most}");
        let mut d = Director::new(1, SimTime(0.0), &limits());
        let test = Cause::Test {
            bot: 3,
            from: who(1, "admin", false),
            text: "привет".into(),
        };
        let out = d.react(SimTime(1.0), &test, &bots, &j, false, &limits());
        assert_eq!(out.len(), 1, "tests need no humans");
    }
}
