//! What goes to the chat worker and what comes back. Requests are plain data the main thread builds from what the
//! bot could know; the worker adds the memory of players and the clock. Replies are recorded, so they carry the
//! final text.

use serde::{Deserialize, Serialize};

use crate::journal::{Notable, Who};

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
    /// A player answered the bot's last line.
    Continued {
        from: Who,
        text: String,
    },
    /// A player wrote to everybody.
    Overheard {
        from: Who,
        text: String,
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
            Trigger::Addressed { from, .. } | Trigger::Continued { from, .. } | Trigger::Overheard { from, .. } => {
                Some(from)
            }
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

    /// Answers to a player rather than remarks.
    pub fn is_answer(&self) -> bool {
        matches!(self, Trigger::Addressed { .. } | Trigger::Continued { .. })
    }
}

/// The bot as the model plays it.
#[derive(Clone, Debug, PartialEq)]
pub struct BotCard {
    /// The name players see.
    pub name: String,
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

#[derive(Clone, Debug, PartialEq)]
pub struct ChatRequest {
    pub id: u64,
    pub bot: BotCard,
    pub trigger: Trigger,
    pub scene: Scene,
    /// What happened lately, chat included, oldest first.
    pub events: Vec<Recent>,
    pub language: String,
    /// Characters the line may take.
    pub max_chars: usize,
    /// Answer in the team chat.
    pub team: bool,
}

/// Why no line came back.
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
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Outcome {
    /// The line to type, cleaned up.
    Line(String),
    /// The model chose to say nothing.
    Skip,
    Failed(Failure),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reply {
    pub id: u64,
    pub outcome: Outcome,
}

/// What chat carries across a map change: who was on the server, so they are not greeted again.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Carry {
    pub humans: Vec<i32>,
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
        ];
        let bytes = postcard::to_allocvec(&replies).unwrap();
        assert_eq!(postcard::from_bytes::<Vec<Reply>>(&bytes).unwrap(), replies);
        let carry = Carry { humans: vec![4, 9] };
        let bytes = postcard::to_allocvec(&carry).unwrap();
        assert_eq!(postcard::from_bytes::<Carry>(&bytes).unwrap(), carry);
    }
}
