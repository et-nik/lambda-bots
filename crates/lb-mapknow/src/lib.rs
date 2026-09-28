//! Map knowledge: what an experienced player knows of a map, worked out once from its graph and geometry (who sees
//! whom from where, where players pass, chokepoints, spots to hold, walls for tripmines, cover), and what the bots
//! learn by playing it (where they get hurt).

#![forbid(unsafe_code)]

pub mod cover;
pub mod experience;
pub mod grid;
pub mod paths;
pub mod tactics;
#[cfg(test)]
mod testmap;
pub mod view;
pub mod vis;

pub use experience::{Experience, ExperienceFile};
pub use tactics::{MapTactics, SpotStats, TacticsStats};
pub use view::MapKnowledge;
pub use vis::VisTable;
