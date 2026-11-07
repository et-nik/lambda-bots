//! GoldSrc demo reader (`.dem`, demo protocol 5, network protocol 48): HLTV and player (POV) recordings.
//!
//! [`file`] walks the frames, [`net`] reads the server messages inside them, including the delta-encoded entities,
//! and [`timeline`] turns both into what a game analysis wants: players' positions and views over time, shots,
//! sounds, explosions, kills and projectiles.

#![forbid(unsafe_code)]

pub mod bits;
pub mod delta;
pub mod file;
pub mod net;
pub mod timeline;

pub use file::DemoFile;
pub use timeline::{Recording, read};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not a GoldSrc demo")]
    NotDemo,
    #[error("demo frame cut short")]
    Truncated,
    #[error("read past the end of a message")]
    Overflow,
    #[error("delta: {0}")]
    Delta(String),
    #[error("message: {0}")]
    Message(String),
}
