//! Where a bot looks when something calls for it: a recognized enemy, the damage compass, an enemy lost a moment
//! ago, an unrecognized glimpse, a sound. Stands in for the vigilance layer and the combat look until the arbiter
//! arrives; the path follower looks along the path otherwise.
//!
//! A bot on the move does not turn its head at every noise: only at sounds near enough to matter, not already in
//! front of it, and with a few quiet seconds between glances. A sound's height is a guess, so the glance at it
//! stays near level.

use lb_core::Vec3;
use lb_core::math::angle_diff;
use lb_core::time::SimTime;
use lb_game::sounds::SoundKind;
use lb_knowledge::{HypothesisKind, PlayerKey, TrackState};

use crate::BotBrain;
use lb_core::dmath;

const CHEST: f32 = 8.0;
const DAMAGE_LOOK_FOR: f64 = 1.0;
const GLANCE_FOR: f64 = 0.8;
/// Quiet time after a glance before a sound or a glimpse draws the eyes again: this, up to twice as long.
const GLANCE_GAP: f64 = 2.5;
const FRESH: f64 = 0.5;
/// A sound this near the view's heading is in front of the bot already, degrees.
const IN_VIEW: f32 = 40.0;
/// Steepest glance up or down at a sound: the tangent of about 15°.
const SOUND_TILT: f32 = 0.27;

/// A sound of `kind` about `distance` away is worth a glance.
fn worth_a_look(kind: SoundKind, distance: f32) -> bool {
    let reach = match kind {
        SoundKind::Shot => 1500.0,
        SoundKind::Pain => 1000.0,
        SoundKind::Step | SoundKind::Jump | SoundKind::Pickup => 700.0,
        SoundKind::WeaponNoise => 500.0,
        _ => 0.0,
    };
    distance <= reach
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LookReason {
    Enemy(PlayerKey),
    Damage,
    Lost(PlayerKey),
    Glimpse,
    Sound(SoundKind),
}

impl LookReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            LookReason::Enemy(_) => "enemy",
            LookReason::Damage => "damage",
            LookReason::Lost(_) => "lost",
            LookReason::Glimpse => "glimpse",
            LookReason::Sound(_) => "sound",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Attention {
    pub point: Vec3,
    pub reason: LookReason,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Glance {
    current: Option<(Attention, SimTime)>,
    quiet_until: SimTime,
}

fn toward(eye: Vec3, bearing: f32) -> Vec3 {
    let (s, c) = dmath::sin_cos(bearing.to_radians());
    eye + Vec3::new(c, s, 0.0) * 300.0
}

/// A look at `p` from `eye`, tilted no more than `SOUND_TILT`.
fn level(eye: Vec3, p: Vec3) -> Vec3 {
    let d = p - eye;
    let flat = d.truncate().length();
    eye + d.truncate().extend(d.z.clamp(-flat * SOUND_TILT, flat * SOUND_TILT))
}

pub(crate) fn pick(brain: &mut BotBrain, now: SimTime, eye: Vec3) -> Option<Attention> {
    let yaw = brain.motor.view.y;
    let b = &brain.beliefs;
    let nearest = |state: TrackState| {
        b.enemies()
            .filter(move |t| t.state == state)
            .min_by(|x, y| x.pos.distance(eye).total_cmp(&y.pos.distance(eye)))
    };
    if let Some(t) = nearest(TrackState::Visible) {
        brain.glance.current = None;
        return Some(Attention {
            point: t.pos + Vec3::Z * CHEST,
            reason: LookReason::Enemy(t.who),
        });
    }
    if let Some(d) = b.last_damage.filter(|d| now.since(d.t) <= DAMAGE_LOOK_FOR)
        && let Some(bearing) = d.bearing
    {
        return Some(Attention {
            point: toward(eye, bearing),
            reason: LookReason::Damage,
        });
    }
    if let Some(t) = nearest(TrackState::RecentlyLost) {
        return Some(Attention {
            point: t.pos + Vec3::Z * CHEST,
            reason: LookReason::Lost(t.who),
        });
    }
    if let Some((a, until)) = brain.glance.current {
        if now < until {
            return Some(a);
        }
        brain.glance.current = None;
    }
    if now < brain.glance.quiet_until {
        return None;
    }
    let glimpse = b
        .hypotheses
        .iter()
        .filter(|h| h.kind == HypothesisKind::Cue && now.since(h.t) <= FRESH)
        .max_by(|x, y| x.t.0.total_cmp(&y.t.0))
        .and_then(|h| h.pos)
        .map(|point| Attention {
            point,
            reason: LookReason::Glimpse,
        });
    let sound = || {
        b.hypotheses
            .iter()
            .filter(|h| now.since(h.t) <= FRESH)
            .filter_map(|h| match h.kind {
                HypothesisKind::Sound(k) => Some((h, k, h.pos.unwrap_or_else(|| toward(eye, h.bearing)))),
                _ => None,
            })
            .filter(|&(h, k, p)| worth_a_look(k, p.distance(eye)) && angle_diff(h.bearing, yaw).abs() > IN_VIEW)
            .max_by(|x, y| x.0.strength.total_cmp(&y.0.strength))
            .map(|(_, k, p)| Attention {
                point: level(eye, p),
                reason: LookReason::Sound(k),
            })
    };
    let a = glimpse.or_else(sound)?;
    // The gap varies from glance to glance without a random stream: by the time of the glance.
    let jitter = (now.secs() * 7.31).fract();
    brain.glance.current = Some((a, now + GLANCE_FOR));
    brain.glance.quiet_until = now + GLANCE_FOR + GLANCE_GAP * (1.0 + jitter);
    Some(a)
}
