//! Honest perception: vision, hearing, damage compass, public information.
//!
//! Raw server state goes in (every player's position, every sound); what comes out is only what a human player
//! would perceive: players in view once recognized, anonymous sounds with localization error, the HUD damage
//! compass.

#![forbid(unsafe_code)]

pub mod damage;
pub mod hearing;
pub mod items;
pub mod steps;
pub mod vision;

use lb_config::skill::SkillParams;

pub use hearing::{Hearing, Listener, SoundEvent};
pub use steps::StepSynth;
pub use vision::{Contact, Frustum, Recognition, Subject, Viewer, Vision, VisionOutput};

/// The skill parameters perception uses.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PerceptionParams {
    pub recognition_delay: [f32; 2],
    pub peripheral_gain: f32,
    pub reacquire_delay: f32,
    pub reacquire_grace: f32,
    pub hearing_threshold: f32,
    pub sound_bearing_sigma: f32,
}

impl PerceptionParams {
    pub fn from_skill(s: &SkillParams) -> PerceptionParams {
        PerceptionParams {
            recognition_delay: s.recognition_delay,
            peripheral_gain: s.peripheral_gain,
            reacquire_delay: s.reacquire_delay,
            reacquire_grace: s.reacquire_grace,
            hearing_threshold: s.hearing_threshold,
            sound_bearing_sigma: s.sound_bearing_sigma,
        }
    }
}

/// One bot's senses.
#[derive(Clone, Debug, Default)]
pub struct Perception {
    pub vision: Vision,
    pub hearing: Hearing,
}

impl Perception {
    /// A new life: nothing in view, nothing heard.
    pub fn reset(&mut self) {
        self.vision.reset();
        self.hearing.reset();
    }
}
