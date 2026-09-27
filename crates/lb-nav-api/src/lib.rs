//! Navigation interface visible to the AI: NavQuery trait and request types.
//!
//! Behavior asks navigation for movement toward a point, for wandering, or for a place to fall back to; it never
//! sees the graph. The service is also the bot's tracer, so behavior code can check walls and ledges through the
//! same handle.

#![forbid(unsafe_code)]

use lb_core::rng::Pcg32;
use lb_core::{Vec2, Vec3};
use lb_worldq::Tracer;

/// Movement for one frame along a path.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NavStep {
    /// Horizontal world direction; zero = stand still.
    pub move_dir: Vec2,
    pub speed: f32,
    /// Where to look to follow the path.
    pub look_at: Vec3,
    /// View pitch to hold on ladders.
    pub pitch: Option<f32>,
    pub jump: bool,
    pub duck: bool,
    /// A traversal that must not be disturbed (a jump in flight, a ladder): its look and stance win over combat.
    pub mandatory: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavStatus {
    Moving,
    /// At the destination (or the graph node closest to it).
    Arrived,
    /// No way there, or navigation gave up after getting stuck.
    NoPath,
}

pub trait NavService: Tracer {
    /// Walks toward `dest`, planning a path when the destination changes.
    fn go_to(&mut self, dest: Vec3) -> (NavStatus, Option<NavStep>);
    /// Wanders between places worth visiting; `rng` picks them.
    fn roam(&mut self, rng: &mut Pcg32) -> Option<NavStep>;
    /// A place to fall back to, away from `threat`.
    fn away_from(&mut self, threat: Vec3) -> Option<Vec3>;
    /// A navigation graph is loaded.
    fn available(&self) -> bool;
}
