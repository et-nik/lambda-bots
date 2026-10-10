//! What goes to the chat worker and what comes back. Requests are plain data the main thread builds from what the
//! bot could know; the worker adds the memory of players and the clock. Replies are recorded, so they carry the
//! final text.

use serde::{Deserialize, Serialize};

use crate::journal::{Notable, Who};
use crate::talk::Social;

/// Request ids: the map's epoch above, a counter below, so a replay makes the same ones.
pub fn request_id(epoch: u32, seq: u32) -> u64 {
    (u64::from(epoch) << 32) | u64::from(seq)
}

/// Why a bot speaks.
#[derive(Clone, Debug, PartialEq)]
pub enum Trigger {
    /// A player wrote to the bot by name, or to the bots.
    Addressed {
        from: Who,
        text: String,
    },
    /// A player went on with a talk with the bot, without naming it.
    Continued {
        from: Who,
        text: String,
    },
    /// A player asked a question: of the bot (`to_me`, in the second person), or of everybody.
    Question {
        from: Who,
        text: String,
        to_me: bool,
    },
    /// A player said hi to everybody.
    Greeted {
        from: Who,
        text: String,
    },
    /// A player wrote to everybody; `about_bots`: the line speaks of the bots in the third person.
    Overheard {
        from: Who,
        text: String,
        about_bots: bool,
    },
    Joined {
        who: Who,
    },
    /// The match is over; `won`: this bot won it.
    MatchEnd {
        winner: Option<Who>,
        won: bool,
    },
    Notable(Notable),
    /// The bot was killed while typing.
    KilledWhileTyping {
        killer: Option<Who>,
    },
    /// GunGame: the bot reached the last level.
    LastLevel,
}

impl Trigger {
    /// The player the bot answers or talks about first, if any.
    pub fn about(&self) -> Option<&Who> {
        match self {
            Trigger::Addressed { from, .. }
            | Trigger::Continued { from, .. }
            | Trigger::Question { from, .. }
            | Trigger::Greeted { from, .. }
            | Trigger::Overheard { from, .. } => Some(from),
            Trigger::Joined { who } => Some(who),
            Trigger::MatchEnd { winner, .. } => winner.as_ref(),
            Trigger::KilledWhileTyping { killer } => killer.as_ref(),
            Trigger::Notable(n) => Some(match n {
                Notable::Nemesis { killer, .. }
                | Notable::Humiliation { killer, .. }
                | Notable::Revenge { killer, .. }
                | Notable::Multikill { killer, .. }
                | Notable::Streak { killer, .. } => killer,
                Notable::OwnBlast { victim, .. } => victim,
                Notable::RageQuit { who, .. } => who,
            }),
            Trigger::LastLevel => None,
        }
    }

    /// The player's chat line the bot answers, and who wrote it.
    pub fn line(&self) -> Option<(&Who, &str)> {
        match self {
            Trigger::Addressed { from, text }
            | Trigger::Continued { from, text }
            | Trigger::Question { from, text, .. }
            | Trigger::Greeted { from, text }
            | Trigger::Overheard { from, text, .. } => Some((from, text)),
            Trigger::Joined { .. }
            | Trigger::MatchEnd { .. }
            | Trigger::Notable(_)
            | Trigger::KilledWhileTyping { .. }
            | Trigger::LastLevel => None,
        }
    }

    /// The player a talk with the bot opens or goes on with: one who wrote, or one who joined.
    pub fn to(&self) -> Option<&Who> {
        match self {
            Trigger::Joined { who } => Some(who),
            _ => self.line().map(|(from, _)| from),
        }
    }

    /// Everyone the trigger names.
    pub fn people(&self) -> Vec<&Who> {
        match self {
            Trigger::Addressed { from, .. }
            | Trigger::Continued { from, .. }
            | Trigger::Question { from, .. }
            | Trigger::Greeted { from, .. }
            | Trigger::Overheard { from, .. } => vec![from],
            Trigger::Joined { who } => vec![who],
            Trigger::MatchEnd { winner, .. } => winner.iter().collect(),
            Trigger::Notable(n) => n.people(),
            Trigger::KilledWhileTyping { killer } => killer.iter().collect(),
            Trigger::LastLevel => Vec::new(),
        }
    }

    /// What the line answers, in a few words, for logs.
    pub fn summary(&self) -> String {
        match self {
            Trigger::Addressed { from, text } => format!("{} to the bot: {text}", from.name),
            Trigger::Continued { from, text } => format!("{} answering the bot: {text}", from.name),
            Trigger::Question { from, text, to_me } if *to_me => format!("{} asks the bot: {text}", from.name),
            Trigger::Question { from, text, .. } => format!("{} asks everybody: {text}", from.name),
            Trigger::Greeted { from, text } => format!("{} says hi to everybody: {text}", from.name),
            Trigger::Overheard { from, text, about_bots } if *about_bots => {
                format!("{} to everybody, of the bots: {text}", from.name)
            }
            Trigger::Overheard { from, text, .. } => format!("{} to everybody: {text}", from.name),
            Trigger::Joined { who } => format!("{} joined", who.name),
            Trigger::MatchEnd { won: true, .. } => "the bot won the match".into(),
            Trigger::MatchEnd { winner, .. } => {
                format!(
                    "the match is over, {} won",
                    winner.as_ref().map_or("nobody", |w| w.name.as_str())
                )
            }
            Trigger::Notable(n) => match n {
                Notable::Nemesis { killer, victim, times } => {
                    format!("{} killed {} {times} times in a row", killer.name, victim.name)
                }
                Notable::Humiliation { killer, victim } => format!("{} crowbarred {}", killer.name, victim.name),
                Notable::OwnBlast { victim, weapon } => format!("{} blew themselves up ({weapon})", victim.name),
                Notable::Revenge { killer, victim, run } => format!(
                    "{} took revenge on {}, who had killed them {run} times in a row",
                    killer.name, victim.name
                ),
                Notable::Multikill { killer, count, .. } => format!("{}: {count} kills at once", killer.name),
                Notable::Streak { killer, count, .. } => format!("{}: {count} kills in a row", killer.name),
                Notable::RageQuit { who, deaths } => format!("{} left after {deaths} deaths in a row", who.name),
            },
            Trigger::KilledWhileTyping { killer } => match killer {
                Some(k) => format!("{} killed the bot while it typed", k.name),
                None => "killed while typing".into(),
            },
            Trigger::LastLevel => "the bot reached the last level".into(),
        }
    }
}

/// The bot as the model plays it.
#[derive(Clone, Debug, PartialEq)]
pub struct BotCard {
    /// The name players see.
    pub name: String,
    /// The personality's name: `bots.yaml`, `phrases.yaml` and the memory of talks go by it.
    pub persona: String,
    pub userid: i32,
    pub skill: u8,
    /// Style id (`rusher`, `sniper`, …).
    pub style: String,
    pub favourite_weapons: Vec<String>,
    pub profanity: bool,
    /// The profile's words on how it writes, else a built-in manner.
    pub manner_text: Option<String>,
    pub manner: u8,
    pub about: Option<String>,
    /// Mood against its own temper: positive bolder, negative warier.
    pub boldness: f32,
    pub alive: bool,
    pub frags: i32,
    pub deaths: i32,
    /// GunGame: level and the weapon it gave.
    pub level: Option<(i32, String)>,
}

/// Someone on the scoreboard.
#[derive(Clone, Debug, PartialEq)]
pub struct PlayerCard {
    pub name: String,
    /// Humans only: the memory key (see [`crate::memory::player_key`]).
    pub key: Option<String>,
    pub frags: i32,
    pub deaths: i32,
    pub level: Option<i32>,
    /// The bot itself.
    pub me: bool,
    /// Kills this map between the bot and this player: (bot killed them, they killed the bot).
    pub duel: (u32, u32),
}

/// The map as the bot sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct Scene {
    pub map: String,
    pub gungame: bool,
    pub teamplay: bool,
    /// Seconds since the map started.
    pub elapsed: f64,
    pub players: Vec<PlayerCard>,
    pub leader: Option<String>,
}

/// One line of what happened, `age` seconds ago.
#[derive(Clone, Debug, PartialEq)]
pub struct Recent {
    pub age: f64,
    pub event: crate::journal::Event,
}

/// A line of a talk, `age` seconds ago; `mine`: the bot's own.
#[derive(Clone, Debug, PartialEq)]
pub struct Said {
    pub age: f64,
    pub mine: bool,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChatRequest {
    pub id: u64,
    pub bot: BotCard,
    pub trigger: Trigger,
    pub scene: Scene,
    /// What happened in the game lately, without the chat, oldest first.
    pub events: Vec<Recent>,
    /// The chat lately, oldest first.
    pub chat: Vec<Recent>,
    /// The bot's own lines lately, oldest first.
    pub own: Vec<Said>,
    /// The bot's talk with the player the request is for ([`crate::prompt::partner`]), oldest first; the prompt shows
    /// only the lines `chat` does not.
    pub talk: Vec<Said>,
    pub language: String,
    /// Characters the line may take.
    pub max_chars: usize,
    /// Answer in the team chat.
    pub team: bool,
}

/// Why no line came back. Recorded in [`Outcome`]: new variants go last.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Failure {
    /// Chat is off, or the provider refused the settings (bad key, unknown model).
    Disabled,
    /// The day's tokens are spent.
    Budget,
    /// The provider is busy or out of reach; requests wait.
    Backoff,
    /// This request failed.
    Error,
    /// The provider says the account has no money left; requests wait long between tries.
    Billing,
}

/// What came of a request. A recording holds each variant's number, so new variants go last.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Outcome {
    /// The line to type, cleaned up.
    Line(String),
    /// The model chose to say nothing.
    Skip,
    Failed(Failure),
    /// A canned phrase for a moment of the game, filled and in the bot's manner.
    Phrase(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reply {
    pub id: u64,
    pub outcome: Outcome,
}

/// What chat carries across a map change: who was in the game, so they are not greeted again, and what the chat keeps
/// from map to map, its times already moved back by the old map's length ([`Social::map_end`]).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Carry {
    pub humans: Vec<i32>,
    pub social: Social,
}

/// A human's map, for the memory of players.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlayerMap {
    pub key: String,
    pub name: String,
    /// Kills between the player and each bot: bot name → (player killed bot, bot killed player).
    pub vs_bots: Vec<(String, u32, u32)>,
    /// Kills by weapon.
    pub weapons: Vec<(String, u32)>,
    pub kills: u32,
    pub deaths: u32,
    pub won: bool,
    /// The player's chat lines, `age` seconds before the map ended.
    pub lines: Vec<(f64, String)>,
    /// Moments of the map with the player, in the server's language.
    pub moments: Vec<String>,
    /// Lines of the player's talks with the bots the memory has not had yet, oldest first: `age` seconds before the
    /// map ended, the bot's persona, the bot's own line, the text.
    pub talk: Vec<(f64, String, bool, String)>,
}

/// A map's end, for the memory: who played and how.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MapSummary {
    pub map: String,
    pub language: String,
    pub minutes: u32,
    pub winner: Option<String>,
    /// Names and frags of the top three.
    pub top: Vec<(String, i32)>,
    pub players: Vec<PlayerMap>,
    /// The chat of the map, `age` seconds before its end: name, text, bot.
    pub chat: Vec<(f64, String, String, bool)>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn who(name: &str) -> Who {
        Who {
            slot: 1,
            userid: 1,
            name: name.into(),
            bot: false,
        }
    }

    #[test]
    fn summaries() {
        let t = Trigger::Addressed {
            from: who("112S"),
            text: "Кемпер".into(),
        };
        assert_eq!(t.summary(), "112S to the bot: Кемпер");
        let t = Trigger::Notable(Notable::Nemesis {
            killer: who("ATLAS Gamer"),
            victim: who("DUT9 ATLASA"),
            times: 3,
        });
        assert_eq!(t.summary(), "ATLAS Gamer killed DUT9 ATLASA 3 times in a row");
        assert_eq!(
            Trigger::MatchEnd {
                winner: None,
                won: false
            }
            .summary(),
            "the match is over, nobody won"
        );
        let asks = |to_me| Trigger::Question {
            from: who("Gordon"),
            text: "где рельсы?".into(),
            to_me,
        };
        assert_eq!(asks(true).summary(), "Gordon asks the bot: где рельсы?");
        assert_eq!(asks(false).summary(), "Gordon asks everybody: где рельсы?");
        let t = Trigger::Greeted {
            from: who("Gordon"),
            text: "прив всем".into(),
        };
        assert_eq!(t.summary(), "Gordon says hi to everybody: прив всем");
        let overheard = |about_bots| Trigger::Overheard {
            from: who("Barney"),
            text: "боты сегодня злые".into(),
            about_bots,
        };
        assert_eq!(
            overheard(true).summary(),
            "Barney to everybody, of the bots: боты сегодня злые"
        );
        assert_eq!(overheard(false).summary(), "Barney to everybody: боты сегодня злые");
    }

    #[test]
    fn lines_talks_and_people() {
        let (gordon, bot) = (
            who("Gordon"),
            Who {
                bot: true,
                ..who("Plutonium")
            },
        );
        let q = Trigger::Question {
            from: gordon.clone(),
            text: "ты свою уже приготовил?".into(),
            to_me: true,
        };
        assert_eq!(q.line(), Some((&gordon, "ты свою уже приготовил?")));
        assert_eq!((q.to(), q.people()), (Some(&gordon), vec![&gordon]));
        let joined = Trigger::Joined { who: gordon.clone() };
        assert_eq!((joined.line(), joined.to()), (None, Some(&gordon)));
        let revenge = Trigger::Notable(Notable::Revenge {
            killer: bot.clone(),
            victim: gordon.clone(),
            run: 3,
        });
        assert_eq!((revenge.line(), revenge.to()), (None, None));
        assert_eq!(revenge.people(), vec![&bot, &gordon]);
        assert_eq!(
            revenge.summary(),
            "Plutonium took revenge on Gordon, who had killed them 3 times in a row"
        );
        let end = Trigger::MatchEnd {
            winner: None,
            won: false,
        };
        assert!(end.to().is_none() && end.people().is_empty());
    }

    #[test]
    fn ids_and_recorded_types_round_trip() {
        assert_eq!(request_id(3, 7), (3 << 32) | 7);
        let replies = vec![
            Reply {
                id: 1,
                outcome: Outcome::Line("гг вп".into()),
            },
            Reply {
                id: 2,
                outcome: Outcome::Skip,
            },
            Reply {
                id: 3,
                outcome: Outcome::Failed(Failure::Backoff),
            },
            Reply {
                id: 4,
                outcome: Outcome::Failed(Failure::Billing),
            },
            Reply {
                id: 5,
                outcome: Outcome::Phrase("гг, Атлас".into()),
            },
        ];
        let bytes = postcard::to_allocvec(&replies).unwrap();
        assert_eq!(postcard::from_bytes::<Vec<Reply>>(&bytes).unwrap(), replies);
        let number = |o: Outcome| postcard::to_allocvec(&o).unwrap();
        assert_eq!(
            number(Outcome::Failed(Failure::Error)),
            [2, 3],
            "old variants keep their numbers"
        );
        assert_eq!(number(Outcome::Failed(Failure::Billing)), [2, 4]);
        assert_eq!(number(Outcome::Phrase(String::new())), [3, 0]);
    }

    #[test]
    fn a_carry_round_trips_with_the_talks() {
        use lb_core::time::SimTime;

        let mut social = Social::default();
        let gordon = Who {
            userid: 4,
            ..who("Gordon")
        };
        social.arrive("Gordon", SimTime(1.0));
        social.arrive("Barney", SimTime(2.0));
        social.left("Barney", SimTime(300.0));
        social
            .talks
            .heard(SimTime(590.0), "Plutonium", &gordon, "Привет Плутон!", true);
        social.said(
            SimTime(595.0),
            "Plutonium",
            Some((4, "Gordon")),
            false,
            "привет, Гордон",
        );
        social.said(SimTime(596.0), "Kleiner", None, false, "гг");
        social
            .players
            .wrote("Gordon", SimTime(590.0), crate::addressing::gist("Привет Плутон!", &[]));
        social.players.answered("Gordon", SimTime(590.0), "Plutonium");
        social.players.greet("Gordon", SimTime(10.0));
        assert!(social.remarks.take(SimTime(400.0), 6.0));
        social.mark_saved();
        social.present(&["Gordon".into()], SimTime(600.0));
        social.map_end(SimTime(600.0), &[4, 9]);
        let thread = &social.talks.threads()[0];
        assert_eq!((thread.last, thread.lines[0].at), (SimTime(-5.0), SimTime(-10.0)));
        assert_eq!(social.players.get("barney").and_then(|p| p.seen), Some(SimTime(-300.0)));
        let carry = Carry {
            humans: vec![4, 9],
            social,
        };
        let bytes = postcard::to_allocvec(&carry).unwrap();
        let back = postcard::from_bytes::<Carry>(&bytes).unwrap();
        assert_eq!(back, carry);
        assert_eq!(back.social.talks.own(SimTime::ZERO, "Kleiner")[0].age, 4.0);
        assert!(back.social.players.greeted_within("Gordon", SimTime(1.0), 2700.0));
    }
}
