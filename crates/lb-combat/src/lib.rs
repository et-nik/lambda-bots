//! Targeting, aim, fire control, ballistics, weapon policy, combat movement.

#![forbid(unsafe_code)]

pub mod aim;
pub mod arms;
pub mod ballistics;
pub mod fight;
pub mod fire;
pub mod policy;
pub mod target;

pub use aim::{Aim, AimSkill, Shot};
pub use fight::{Fight, FightInput, FightMove, FightSkill};
pub use policy::{Armed, Choice};
