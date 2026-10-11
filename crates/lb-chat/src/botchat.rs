//! One bot's line on its way into the chat: asked for, ready, typed, said. Typing takes as long as a player's
//! would; an alive bot types when it can afford to stand still, on the run when no such moment comes soon, a dead one
//! right away, holding its respawn. A line once written is paid for: it is never dropped for being late.

use lb_core::rng::Pcg32;
use lb_core::time::SimTime;
use serde::{Deserialize, Serialize};

use crate::journal::Who;

/// Seconds a ready line waits for a calm moment; then an alive bot types it on the run.
pub const CALM_WAIT: f64 = 8.0;
/// Seconds a bot waits for the worker's reply. The worker answers every request, at worst a provider timeout after
/// its turn came, so only a worker that is gone keeps a bot waiting this long.
pub const ASKED_KEEP: f64 = 180.0;

/// What a line answers, from the least pressing to the most. Recorded in a [`crate::Carry`]: new variants go last.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
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
    /// Alive but not calm (fighting, falling, on a ladder, under a test): a line that waited [`CALM_WAIT`] for a calm
    /// moment is typed on the run.
    Busy,
    /// Not at all: its last line is too recent for the game, or it is not in the game.
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

/// A line asked of a bot, on its way into the chat once its text came: what the runtime keeps of it besides the bot's
/// [`BotChat`]. The line is the personality's: when its bot is gone (the map changed, it left), the line waits for the
/// personality's bot to come back and be said by it. Recorded in a [`crate::Carry`]: a change to its fields is a new
/// recording format.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pending {
    pub id: u64,
    /// The bot it was asked of, as the runtime keeps it: a bot back on the next map takes the line over.
    pub slot: u8,
    pub generation: u32,
    /// The bot's personality: the director marked the player's line answered by it.
    pub persona: String,
    pub priority: Priority,
    pub team: bool,
    /// When it was asked for: a reply not come [`ASKED_KEEP`] later is not coming.
    pub asked: SimTime,
    /// The line to type, once the reply brought one.
    pub text: Option<String>,
    /// The line is a ready phrase, not the model's: it cost nothing, and is not kept for a bot that is gone.
    pub phrase: bool,
    /// The player the line answers or greets.
    pub to: Option<Who>,
    /// When the player's line it answers came ([`crate::director::Speak::line_at`]).
    pub line_at: Option<SimTime>,
    /// That line, when the director left it out of the bot's talk with the player (a line or a question to
    /// everybody, a hello): it goes into the talk with the answer.
    pub unheard: Option<String>,
    /// It greets them: a talk it opens stays short until they answer.
    pub greeting: bool,
    /// Asking for it marked them greeted: when they were greeted before ([`crate::director::Speak::greeted`]).
    pub greeted: Option<Option<SimTime>>,
    /// It took one of the hour's lines nobody asked for ([`crate::director::Speak::unasked`]).
    pub unasked: bool,
    /// Said, it starts the gap between remarks: a game moment, or a word on a line to everybody.
    pub remark: bool,
}

impl Pending {
    /// Its times as the next map sees them, the map that ended at `end` being over.
    pub fn map_end(&mut self, end: SimTime) {
        let back = |t: SimTime| SimTime(t.0 - end.0);
        self.asked = SimTime(self.asked.0 - end.0);
        self.line_at = self.line_at.map(back);
        self.greeted = self.greeted.map(|before| before.map(back));
    }
}

#[derive(Clone, Debug)]
pub struct BotChat {
    phase: Phase,
    priority: Priority,
    team: bool,
    /// Typing does not start before this: noticing and thinking take time.
    earliest: SimTime,
    /// A reply asked for and not come by then is not coming ([`ASKED_KEEP`]).
    expires: SimTime,
    /// The respawn is held since this time.
    hold_since: Option<SimTime>,
    /// Typing started alive: the bot stands still.
    in_the_open: bool,
    /// Typed on the run (no calm moment came, or a danger broke the standing): no standing still for this line, after
    /// a respawn neither.
    rushed: bool,
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
            hold_since: None,
            in_the_open: false,
            rushed: false,
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

    /// A line was asked for as request `id` at `asked` (now, or on the map before for a line given back to the bot);
    /// noticing takes `notice` seconds from `now`.
    pub fn ask(&mut self, id: u64, priority: Priority, team: bool, asked: SimTime, now: SimTime, notice: f64) {
        self.phase = Phase::Asked { id };
        self.priority = priority;
        self.team = team;
        self.earliest = now + notice;
        self.expires = asked + ASKED_KEEP;
        self.in_the_open = false;
        self.rushed = false;
    }

    /// A line whose text is known, typed from `at` on: one typed by hand (`lb chat say`), or one given back to the
    /// bot on the map after the one it was asked on.
    pub fn set_line(&mut self, text: String, priority: Priority, team: bool, at: SimTime) {
        self.phase = Phase::Ready { text };
        self.priority = priority;
        self.team = team;
        self.earliest = at;
        self.in_the_open = false;
        self.rushed = false;
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

    /// Advances the line; returns it when it is typed. `typing` gives the seconds a text takes. A line whose text is
    /// known never goes stale: one that found no calm moment in `calm_wait` seconds ([`CALM_WAIT`], none while a
    /// player waits for the bot's answer) is typed on the run.
    pub fn tick(&mut self, now: SimTime, can: Can, calm_wait: f64, typing: impl FnOnce(&str) -> f64) -> Option<String> {
        let may_type = match can {
            Can::Alive | Can::Free => true,
            Can::Busy => now.since(self.earliest) >= calm_wait,
            Can::No => false,
        };
        match &self.phase {
            Phase::Asked { .. } if now > self.expires => {
                self.phase = Phase::Idle;
                None
            }
            Phase::Ready { text } if now >= self.earliest && may_type => {
                let done = now + typing(text);
                self.in_the_open = can == Can::Alive;
                self.rushed = can == Can::Busy;
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

    /// Danger while typing in the open: the bot stops standing still to fight and finishes the line on the run.
    pub fn interrupt(&mut self) {
        if self.typing_in_the_open() {
            self.in_the_open = false;
            self.rushed = true;
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

    /// Respawned: a line it was typing while dead it finishes standing; one it typed on the run, on the run.
    pub fn on_spawn(&mut self) {
        self.hold_since = None;
        if self.typing() && !self.rushed {
            self.in_the_open = true;
        }
    }

    /// Chat switched off: nothing on its way, the times forgotten.
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
        c.ask(7, Priority::Answer, false, t(10.0), t(10.0), 1.0);
        assert_eq!(c.busy(), Some(Priority::Answer));
        assert!(!c.answer(8, Some("x".into()), t(10.5), 0.5), "another request");
        assert!(c.answer(7, Some("привет".into()), t(10.5), 0.5));
        assert_eq!(
            c.tick(t(11.0), Can::Alive, CALM_WAIT, |_| 2.0),
            None,
            "still noticing and thinking"
        );
        assert_eq!(c.tick(t(11.5), Can::Busy, CALM_WAIT, |_| 2.0), None, "busy fighting");
        assert_eq!(c.tick(t(12.0), Can::Alive, CALM_WAIT, |_| 2.0), None);
        assert!(c.typing_in_the_open());
        assert_eq!(c.tick(t(13.0), Can::Alive, CALM_WAIT, |_| 2.0), None);
        assert_eq!(
            c.tick(t(14.0), Can::Alive, CALM_WAIT, |_| 2.0).as_deref(),
            Some("привет")
        );
        assert_eq!(c.busy(), None);
        assert_eq!(c.last_line, Some(t(14.0)));
    }

    #[test]
    fn a_line_never_goes_stale_and_silence_frees_the_bot() {
        let mut c = BotChat::default();
        c.ask(1, Priority::Remark, false, t(0.0), t(0.0), 0.5);
        assert!(c.answer(1, None, t(1.0), 0.0));
        assert_eq!(c.busy(), None);
        c.ask(2, Priority::Remark, false, t(0.0), t(0.0), 0.5);
        assert_eq!(c.tick(t(60.0), Can::No, CALM_WAIT, |_| 1.0), None);
        assert!(
            c.answer(2, Some("lol".into()), t(60.0), 0.0),
            "a slow reply is still waited for"
        );
        assert_eq!(c.tick(t(600.0), Can::No, CALM_WAIT, |_| 1.0), None);
        assert_eq!(
            c.phase(),
            &Phase::Ready { text: "lol".into() },
            "a line once written is kept"
        );
        assert_eq!(c.tick(t(600.5), Can::Free, CALM_WAIT, |_| 1.0), None);
        assert_eq!(c.tick(t(601.5), Can::Free, CALM_WAIT, |_| 1.0).as_deref(), Some("lol"));
        c.ask(3, Priority::Remark, false, t(700.0), t(700.0), 0.5);
        c.tick(t(700.0 + ASKED_KEEP + 1.0), Can::Alive, CALM_WAIT, |_| 1.0);
        assert_eq!(c.busy(), None, "a reply that never comes frees the bot");
        assert!(!c.answer(3, Some("поздно".into()), t(900.0), 0.0));
        c.ask(4, Priority::Remark, false, t(-150.0), t(1.0), 0.5);
        c.tick(t(31.0), Can::Alive, CALM_WAIT, |_| 1.0);
        assert_eq!(
            c.busy(),
            None,
            "given back on the next map, it waits no longer than it would have"
        );
    }

    #[test]
    fn with_no_calm_moment_the_line_is_typed_on_the_run() {
        let mut c = BotChat::default();
        c.set_line("gg".into(), Priority::MatchEnd, false, t(0.0));
        assert_eq!(c.tick(t(CALM_WAIT - 0.1), Can::Busy, CALM_WAIT, |_| 2.0), None);
        assert!(!c.typing(), "it waits for a calm moment a while");
        assert_eq!(c.tick(t(CALM_WAIT), Can::No, CALM_WAIT, |_| 2.0), None);
        assert!(!c.typing(), "never right after its last line");
        c.tick(t(CALM_WAIT), Can::Busy, CALM_WAIT, |_| 2.0);
        assert!(c.typing() && !c.typing_in_the_open(), "then types it on the run");
        c.on_death(t(CALM_WAIT + 1.0), None);
        c.on_spawn();
        assert!(!c.typing_in_the_open(), "respawned, it goes on typing on the run");
        assert!(c.killed_typing.is_none());
        assert_eq!(
            c.tick(t(CALM_WAIT + 2.0), Can::Busy, CALM_WAIT, |_| 2.0).as_deref(),
            Some("gg")
        );
        c.set_line("ну что там".into(), Priority::Remark, false, t(20.0));
        c.tick(t(20.0), Can::Busy, 0.0, |_| 2.0);
        assert!(c.typing(), "with a player waiting for its answer, at once");
    }

    #[test]
    fn an_interrupted_line_is_finished_on_the_run() {
        let mut c = BotChat::default();
        c.set_line("gg".into(), Priority::Answer, false, t(0.0));
        c.tick(t(0.0), Can::Alive, CALM_WAIT, |_| 3.0);
        assert!(c.typing_in_the_open());
        c.interrupt();
        assert!(c.typing() && !c.typing_in_the_open());
        assert_eq!(c.tick(t(2.0), Can::Busy, CALM_WAIT, |_| 3.0), None);
        assert_eq!(c.tick(t(3.0), Can::Busy, CALM_WAIT, |_| 3.0).as_deref(), Some("gg"));
    }

    #[test]
    fn dead_bots_hold_the_respawn_for_a_while() {
        let mut c = BotChat::default();
        c.set_line("ну и ладно".into(), Priority::Answer, false, t(0.0));
        c.tick(t(0.0), Can::Alive, CALM_WAIT, |_| 6.0);
        c.on_death(t(1.0), None);
        assert!(c.killed_typing.is_some());
        assert!(c.holds_respawn(t(1.5), 3.5));
        assert!(c.holds_respawn(t(4.9), 3.5));
        assert!(!c.holds_respawn(t(5.1), 3.5), "the hold has a limit");
        c.interrupt();
        assert!(c.typing(), "a dead bot's typing is never interrupted");
        c.on_spawn();
        assert!(c.typing_in_the_open(), "respawned mid-line, it finishes standing");
        assert_eq!(
            c.tick(t(6.0), Can::Alive, CALM_WAIT, |_| 6.0).as_deref(),
            Some("ну и ладно")
        );
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
