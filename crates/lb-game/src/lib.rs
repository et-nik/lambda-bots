//! HLDM adapter: message decoders, self state, public rules, weapon tables, game modes data.
//!
//! Everything here is information a human player's client also receives (their own HUD
//! messages, the scoreboard, the kill feed, server cvars). It never exposes other players'
//! hidden state.

#![forbid(unsafe_code)]

pub mod compat;
pub mod entities;
pub mod input;
pub mod items;
pub mod mechanics;
pub mod messages;
pub mod mode;
pub mod rules;
pub mod scoreboard;
pub mod self_state;
pub mod sounds;
pub mod weapons;

/// A value that may not have been observed yet; never silently assumed to be zero.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum Known<T> {
    #[default]
    Unknown,
    Value(T),
}

impl<T: Copy> Known<T> {
    pub fn get(self) -> Option<T> {
        match self {
            Known::Value(v) => Some(v),
            Known::Unknown => None,
        }
    }

    pub fn or(self, default: T) -> T {
        self.get().unwrap_or(default)
    }
}
