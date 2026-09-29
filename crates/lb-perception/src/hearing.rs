//! Hearing: sounds and weapon events reach a bot as the engine delivers them to clients (the PAS of the source, or
//! everyone for global ones) and are audible when the gain the client would play them at clears the bot's
//! threshold. What is heard is anonymous and localized with an error that grows as the sound gets fainter.

use lb_core::Vec3;
use lb_core::math::normalize_angle;
use lb_core::rng::Pcg32;
use lb_core::time::SimTime;
use lb_game::sounds::{SoundClass, SoundKind};
use lb_knowledge::SoundStimulus;
use lb_worldq::VisSets;

use crate::PerceptionParams;
use lb_core::dmath;

/// The client's `sound_nominal_clip_dist`.
const NOMINAL_CLIP: f32 = 1000.0;
/// Own shots mask other sounds for this long.
const OWN_SHOT_MASK_FOR: f64 = 0.3;
const OWN_SHOT_MASK: f32 = 3.0;
const RUNNING_MASK: f32 = 1.2;
const RUNNING_SPEED: f32 = 150.0;
const WIDEST_BEARING_SIGMA: f32 = 35.0;
const RANGE_LOG_SIGMA: f32 = 0.3;
/// Up or down is told apart worse than left and right.
const ELEVATION_SIGMA: f32 = 10.0;

/// A sound or weapon event of this frame, as the engine sent it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SoundEvent {
    pub t: SimTime,
    /// Player slot (edict index) of the emitter, if a player made it.
    pub source: Option<u8>,
    pub origin: Vec3,
    pub class: SoundClass,
    pub volume: f32,
    pub attenuation: f32,
    /// Sent to every client (`FEV_GLOBAL`, `ATTN_NONE`): no PAS test.
    pub global: bool,
}

/// The bot that listens.
#[derive(Clone, Copy, Debug)]
pub struct Listener {
    pub slot: u8,
    pub origin: Vec3,
    pub eye: Vec3,
    /// View yaw, for front/back confusion.
    pub yaw: f32,
    pub speed: f32,
}

#[derive(Clone, Debug, Default)]
pub struct HearingStats {
    pub heard: u64,
    /// Delivered by the engine but below the threshold.
    pub too_quiet: u64,
}

#[derive(Clone, Debug, Default)]
pub struct Hearing {
    own_shot_at: Option<SimTime>,
    pub stats: HearingStats,
}

/// Gain the client plays a sound at, `vol · (1 − d · attn / 1000)`.
pub fn gain(volume: f32, attenuation: f32, distance: f32) -> f32 {
    if attenuation <= 0.0 {
        return volume;
    }
    volume * (1.0 - distance * attenuation / NOMINAL_CLIP).max(0.0)
}

impl Hearing {
    pub fn reset(&mut self) {
        self.own_shot_at = None;
    }

    /// The bot fired: its own shot masks quieter sounds for a moment.
    pub fn on_own_shot(&mut self, t: SimTime) {
        self.own_shot_at = Some(t);
    }

    pub fn hear(
        &mut self,
        ev: &SoundEvent,
        l: &Listener,
        vis: &dyn VisSets,
        params: &PerceptionParams,
        rng: &mut Pcg32,
    ) -> Option<SoundStimulus> {
        if ev.source == Some(l.slot) {
            if ev.class.kind == SoundKind::Shot {
                self.on_own_shot(ev.t);
            }
            return None;
        }
        if ev.source.is_none()
            && !matches!(
                ev.class.kind,
                SoundKind::Explosion | SoundKind::ItemRespawn | SoundKind::Bounce
            )
        {
            return None;
        }
        if !ev.global && !vis.in_pas(l.origin, ev.origin) {
            return None;
        }
        let distance = l.eye.distance(ev.origin);
        let g = gain(ev.volume, ev.attenuation, distance);
        let mut threshold = params.hearing_threshold;
        if self.own_shot_at.is_some_and(|t| ev.t.since(t) <= OWN_SHOT_MASK_FOR) {
            threshold *= OWN_SHOT_MASK;
        }
        if l.speed > RUNNING_SPEED {
            threshold *= RUNNING_MASK;
        }
        if g <= 0.0 || g < threshold {
            self.stats.too_quiet += 1;
            return None;
        }
        self.stats.heard += 1;
        let loudness = (g / ev.volume.max(0.01)).clamp(0.0, 1.0);
        let d = ev.origin - l.eye;
        let true_bearing = dmath::atan2(d.y, d.x).to_degrees();
        let sigma = params.sound_bearing_sigma
            + (WIDEST_BEARING_SIGMA - params.sound_bearing_sigma).max(0.0) * (1.0 - loudness);
        let mut bearing = true_bearing + rng.normal() * sigma;
        if rng.next_f32() < 0.1 * (1.0 - loudness) {
            // Front and back confused: mirrored across the listener's left-right axis.
            bearing = 2.0 * l.yaw + 180.0 - bearing;
        }
        let bearing = normalize_angle(bearing);
        let range = distance.max(1.0) * dmath::exp(rng.normal() * RANGE_LOG_SIGMA);
        let elevation =
            (dmath::atan2(d.z, d.truncate().length()).to_degrees() + rng.normal() * ELEVATION_SIGMA).clamp(-80.0, 80.0);
        let (sb, cb) = dmath::sin_cos(bearing.to_radians());
        let (se, ce) = dmath::sin_cos(elevation.to_radians());
        Some(SoundStimulus {
            t: ev.t,
            kind: ev.class.kind,
            weapon: ev.class.weapon,
            pos: l.eye + Vec3::new(cb * ce, sb * ce, se) * range,
            bearing,
            bearing_sigma: sigma,
            range,
            gain: g,
        })
    }
}
