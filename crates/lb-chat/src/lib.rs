//! Bots in the game chat: what happened on the map (journal), who says something and when (director), the talks and
//! what else the chat keeps from map to map (talk), a bot's line from the request to the typed `say` (botchat), what
//! the model is asked (prompt), the ready phrases for moments of the game (phrases), what a line may be (sanitize)
//! and say (profanity), and what the bots remember of players (memory). No threads, files or network: the runtime
//! feeds it.

#![forbid(unsafe_code)]

pub mod addressing;
pub mod aliases;
pub mod botchat;
pub mod director;
pub mod journal;
pub mod lang;
pub mod memory;
pub mod phrases;
pub mod profanity;
pub mod prompt;
pub mod request;
pub mod sanitize;
pub mod talk;

pub use aliases::Aliases;
pub use botchat::{BotChat, Can, Priority};
pub use director::{Cause, Director, Limits, Speak, Speaker};
pub use journal::{Event, Journal, Notable, Who};
pub use phrases::{Offer, Ring};
pub use request::{BotCard, Carry, ChatRequest, Failure, MapSummary, Outcome, PlayerCard, Reply, Said, Scene, Trigger};
pub use talk::Social;
