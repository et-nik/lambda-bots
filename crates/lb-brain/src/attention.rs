//! Where a bot looks when something calls for it: a recognized enemy, the damage compass, an enemy lost a moment
//! ago, an unrecognized glimpse, a sound. Stands in for the vigilance layer and the combat look until the arbiter
//! arrives; the path follower looks along the path otherwise.

use lb_core::Vec3;
use lb_core::time::SimTime;
use lb_game::sounds::SoundKind;
use lb_knowledge::{HypothesisKind, PlayerKey, TrackState};

use crate::BotBrain;
use lb_core::dmath;

const CHEST: f32 = 8.0;
const DAMAGE_LOOK_FOR: f64 = 1.0;
const GLANCE_FOR: f64 = 0.8;
/// Quiet time after a glance before a sound or a glimpse draws the eyes again.
const GLANCE_GAP: f64 = 1.0;
const FRESH: f64 = 0.5;

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

pub(crate) fn pick(brain: &mut BotBrain, now: SimTime, eye: Vec3) -> Option<Attention> {
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
                HypothesisKind::Sound(
                    k @ (SoundKind::Shot
                    | SoundKind::Step
                    | SoundKind::Jump
                    | SoundKind::Pain
                    | SoundKind::WeaponNoise
                    | SoundKind::Pickup),
                ) => Some((h, k)),
                _ => None,
            })
            .max_by(|x, y| x.0.strength.total_cmp(&y.0.strength))
            .map(|(h, k)| Attention {
                point: h.pos.unwrap_or_else(|| toward(eye, h.bearing)),
                reason: LookReason::Sound(k),
            })
    };
    let a = glimpse.or_else(sound)?;
    brain.glance.current = Some((a, now + GLANCE_FOR));
    brain.glance.quiet_until = now + GLANCE_FOR + GLANCE_GAP;
    Some(a)
}
