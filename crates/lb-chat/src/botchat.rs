//! One bot's line on its way into the chat: asked for, ready, typed, said. Typing takes as long as a player's
//! would; an alive bot types only when it can afford to stand still, a dead one right away, holding its respawn.

use lb_core::rng::Pcg32;
use lb_core::time::SimTime;

use crate::journal::Who;

/// What a line answers, in the order one line gives way to another.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    /// Something the bot says about the game unasked.
    Remark,
    /// A greeting, or a word to a player who spoke to everybody.
    Greeting,
    MatchEnd,
    /// An answer to a player who spoke to the bot.
    Answer,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    Idle,
    /// Waiting for the model's line.
    Asked {
        id: u64,
    },
    /// The line is known; typing starts when the bot can.
    Ready {
        text: String,
    },
    /// Typing since `since`; the line goes out at `done`.
    Typing {
        text: String,
        since: SimTime,
        done: SimTime,
    },
}

/// Whether the bot can type now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Can {
    /// Alive and calm: it stands still while typing.
    Alive,
    /// Dead, frozen or in the intermission: typing costs it nothing.
    Free,
    /// Not now (fighting, falling, on a ladder).
    No,
}

/// Seconds to notice a cause and read the line it answers (`read` characters).
pub fn notice_secs(rng: &mut Pcg32, read: usize) -> f64 {
    f64::from(rng.range_f32(0.3, 1.0)) + (read as f64 * 0.035).min(3.0)
}

/// Seconds to think of `reply`: short ones come at once.
pub fn think_secs(rng: &mut Pcg32, reply: &str) -> f64 {
    let quick = reply.chars().count() <= 4;
    f64::from(if quick {
        rng.range_f32(0.2, 0.6)
    } else {
        rng.range_f32(0.5, 2.0)
    })
}

/// Seconds to type `text` at `cpm` characters a minute, opening the chat and pressing enter included.
pub fn typing_secs(rng: &mut Pcg32, text: &str, cpm: f32) -> f64 {
    let chars = text.chars().count().max(1) as f64;
    chars / f64::from(cpm.max(30.0) / 60.0) * f64::from(rng.range_f32(0.9, 1.2)) + 0.3
}

#[derive(Clone, Debug)]
pub struct BotChat {
    phase: Phase,
    priority: Priority,
    team: bool,
    /// Typing does not start before this: noticing and thinking take time.
    earliest: SimTime,
    /// After this the line is not worth saying.
    expires: SimTime,
    retries: u8,
    /// The respawn is held since this time.
    hold_since: Option<SimTime>,
    /// Typing started alive: the bot stands still.
    in_the_open: bool,
    pub last_line: Option<SimTime>,
    /// The last line nobody asked for.
    pub last_remark: Option<SimTime>,
    /// Killed while typing in the open, when and by whom.
    pub killed_typing: Option<(SimTime, Option<Who>)>,
}

impl Default for BotChat {
    fn default() -> Self {
        BotChat {
            phase: Phase::Idle,
            priority: Priority::Remark,
            team: false,
            earliest: SimTime::ZERO,
            expires: SimTime::ZERO,
            retries: 0,
            hold_since: None,
            in_the_open: false,
            last_line: None,
            last_remark: None,
            killed_typing: None,
        }
    }
}

impl BotChat {
    pub fn phase(&self) -> &Phase {
        &self.phase
    }

    /// The priority of the line on its way, if any.
    pub fn busy(&self) -> Option<Priority> {
        (self.phase != Phase::Idle).then_some(self.priority)
    }

    pub fn team(&self) -> bool {
        self.team
    }

    /// Typing while alive: the bot stands still.
    pub fn typing_in_the_open(&self) -> bool {
        matches!(self.phase, Phase::Typing { .. }) && self.in_the_open
    }

    pub fn typing(&self) -> bool {
        matches!(self.phase, Phase::Typing { .. })
    }

    /// When the typing of the line on its way began.
    pub fn typing_since(&self) -> Option<SimTime> {
        match self.phase {
            Phase::Typing { since, .. } => Some(since),
            _ => None,
        }
    }

    /// A line was asked for as request `id`; noticing takes `notice` seconds, and after `keep` seconds it is stale.
    pub fn ask(&mut self, id: u64, priority: Priority, team: bool, now: SimTime, notice: f64, keep: f64) {
        self.phase = Phase::Asked { id };
        self.priority = priority;
        self.team = team;
        self.earliest = now + notice;
        self.expires = now + keep;
        self.retries = 0;
        self.in_the_open = false;
    }

    /// A line typed by hand (`lb chat say`): ready at once.
    pub fn set_line(&mut self, text: String, now: SimTime, keep: f64) {
        self.phase = Phase::Ready { text };
        self.priority = Priority::Answer;
        self.team = false;
        self.earliest = now;
        self.expires = now + keep;
        self.retries = 0;
        self.in_the_open = false;
    }

    /// The model's answer to request `id`: a line, or nothing. `false` if the bot no longer waits for it.
    pub fn answer(&mut self, id: u64, line: Option<String>, now: SimTime, think: f64) -> bool {
        if self.phase != (Phase::Asked { id }) {
            return false;
        }
        match line {
            Some(text) => {
                self.phase = Phase::Ready { text };
                self.earliest = (self.earliest + think).max(now);
            }
            None => self.phase = Phase::Idle,
        }
        true
    }

    /// Advances the line; returns it when it is typed. `typing` gives the seconds a text takes.
    pub fn tick(&mut self, now: SimTime, can: Can, typing: impl FnOnce(&str) -> f64) -> Option<String> {
        match &self.phase {
            Phase::Asked { .. } | Phase::Ready { .. } if now > self.expires => {
                self.phase = Phase::Idle;
                None
            }
            Phase::Ready { text } if now >= self.earliest && can != Can::No => {
                let done = now + typing(text);
                self.in_the_open = can == Can::Alive;
                self.phase = Phase::Typing {
                    text: text.clone(),
                    since: now,
                    done,
                };
                None
            }
            Phase::Typing { done, .. } if now >= *done => {
                let Phase::Typing { text, .. } = std::mem::replace(&mut self.phase, Phase::Idle) else {
                    return None;
                };
                self.last_line = Some(now);
                if self.priority == Priority::Remark {
                    self.last_remark = Some(now);
                }
                self.hold_since = None;
                Some(text)
            }
            _ => None,
        }
    }

    /// Danger while typing in the open: the bot drops the chat to fight, and types the line again later (once).
    pub fn interrupt(&mut self) {
        if let Phase::Typing { text, .. } = &self.phase
            && self.in_the_open
        {
            self.phase = if self.retries == 0 {
                Phase::Ready { text: text.clone() }
            } else {
                Phase::Idle
            };
            self.retries += 1;
            self.in_the_open = false;
        }
    }

    /// Whether a dead bot keeps from respawning to finish its line, at most `max_hold` seconds.
    pub fn holds_respawn(&mut self, now: SimTime, max_hold: f64) -> bool {
        if self.phase == Phase::Idle {
            self.hold_since = None;
            return false;
        }
        let since = *self.hold_since.get_or_insert(now);
        now.since(since) < max_hold
    }

    /// The bot died: a line typed in the open goes on being typed, as the chat stays open for a dead player.
    pub fn on_death(&mut self, now: SimTime, killer: Option<Who>) {
        if self.typing_in_the_open() {
            self.killed_typing = Some((now, killer));
            self.in_the_open = false;
        }
    }

    pub fn on_spawn(&mut self) {
        self.hold_since = None;
        if self.typing() {
            self.in_the_open = true;
        }
    }

    /// A new map: nothing on its way, the times of the old map forgotten.
    pub fn reset(&mut self) {
        *self = BotChat::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: f64) -> SimTime {
        SimTime(s)
    }

    #[test]
    fn asked_ready_typed_said() {
        let mut c = BotChat::default();
        c.ask(7, Priority::Answer, false, t(10.0), 1.0, 25.0);
        assert_eq!(c.busy(), Some(Priority::Answer));
        assert!(!c.answer(8, Some("x".into()), t(10.5), 0.5), "another request");
        assert!(c.answer(7, Some("привет".into()), t(10.5), 0.5));
        assert_eq!(
            c.tick(t(11.0), Can::Alive, |_| 2.0),
            None,
            "still noticing and thinking"
        );
        assert_eq!(c.tick(t(11.5), Can::No, |_| 2.0), None, "busy fighting");
        assert_eq!(c.tick(t(12.0), Can::Alive, |_| 2.0), None);
        assert!(c.typing_in_the_open());
        assert_eq!(c.tick(t(13.0), Can::Alive, |_| 2.0), None);
        assert_eq!(c.tick(t(14.0), Can::Alive, |_| 2.0).as_deref(), Some("привет"));
        assert_eq!(c.busy(), None);
        assert_eq!(c.last_line, Some(t(14.0)));
    }

    #[test]
    fn late_lines_and_silence_are_dropped() {
        let mut c = BotChat::default();
        c.ask(1, Priority::Remark, false, t(0.0), 0.5, 15.0);
        assert!(c.answer(1, None, t(1.0), 0.0));
        assert_eq!(c.busy(), None);
        c.ask(2, Priority::Remark, false, t(0.0), 0.5, 15.0);
        assert!(c.answer(2, Some("lol".into()), t(1.0), 0.0));
        assert_eq!(c.tick(t(16.0), Can::No, |_| 1.0), None);
        assert_eq!(c.busy(), None, "nobody types a line that old");
    }

    #[test]
    fn interrupted_once_retyped_then_dropped() {
        let mut c = BotChat::default();
        c.set_line("gg".into(), t(0.0), 25.0);
        c.tick(t(0.0), Can::Alive, |_| 3.0);
        c.interrupt();
        assert_eq!(c.phase(), &Phase::Ready { text: "gg".into() });
        c.tick(t(5.0), Can::Alive, |_| 3.0);
        assert!(c.typing());
        c.interrupt();
        assert_eq!(c.busy(), None);
    }

    #[test]
    fn dead_bots_hold_the_respawn_for_a_while() {
        let mut c = BotChat::default();
        c.set_line("ну и ладно".into(), t(0.0), 25.0);
        c.tick(t(0.0), Can::Alive, |_| 6.0);
        c.on_death(t(1.0), None);
        assert!(c.killed_typing.is_some());
        assert!(c.holds_respawn(t(1.5), 3.5));
        assert!(c.holds_respawn(t(4.9), 3.5));
        assert!(!c.holds_respawn(t(5.1), 3.5), "the hold has a limit");
        c.interrupt();
        assert!(c.typing(), "a dead bot's typing is never interrupted");
        c.on_spawn();
        assert!(c.typing_in_the_open(), "respawned mid-line, it finishes standing");
        assert_eq!(c.tick(t(6.0), Can::Alive, |_| 6.0).as_deref(), Some("ну и ладно"));
        assert!(!c.holds_respawn(t(7.0), 3.5));
    }

    #[test]
    fn typing_times() {
        let mut rng = Pcg32::new(1, 2);
        let short = typing_secs(&mut rng, "gg", 300.0);
        let long = typing_secs(&mut rng, "ну ты и кемпер, вылезай уже из своего угла", 300.0);
        assert!(short < 1.2 && long > 7.0, "{short} {long}");
        assert!(think_secs(&mut rng, "lol") <= 0.6);
        assert!(notice_secs(&mut rng, 1000) <= 4.0);
    }
}
