//! Bots in the game chat: what happened on the map (journal), who says something and when (director), a bot's line
//! from the request to the typed `say` (botchat), what the model is asked (prompt) and what a line may be
//! (sanitize), and what the bots remember of players (memory). No threads, files or network: the runtime feeds it.

#![forbid(unsafe_code)]

pub mod addressing;
pub mod botchat;
pub mod director;
pub mod journal;
pub mod lang;
pub mod memory;
pub mod prompt;
pub mod request;
pub mod sanitize;

pub use botchat::{BotChat, Can, Priority};
pub use director::{Cause, Director, Limits, Speak, Speaker};
pub use journal::{Event, Journal, Notable, Who};
pub use request::{BotCard, Carry, ChatRequest, Failure, MapSummary, Outcome, PlayerCard, Reply, Scene, Trigger};
