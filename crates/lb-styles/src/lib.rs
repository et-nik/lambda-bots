//! Bot personalities: play styles, skill parameters and the personas that bind them to nicknames.

#![forbid(unsafe_code)]

pub mod emotions;
pub mod persona;
pub mod style;

pub use emotions::Emotions;
pub use persona::{Persona, PersonaSource, generate, name_seed};
pub use style::{GoalAffinity, StyleId, StyleTable, TraitRanges, TrickLikes, WeaponLikes};
