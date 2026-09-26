//! Core primitives: math, simulation time, handles and epochs, RNG streams, budgets.

#![forbid(unsafe_code)]

pub mod budget;
pub mod handles;
pub mod math;
pub mod msg;
pub mod rng;
pub mod time;

pub use glam::{Vec2, Vec3};
