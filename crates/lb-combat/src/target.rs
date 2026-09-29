//! Target selection: among enemies in sight, the nearest counts most, one aiming at the bot or firing more, and
//! the current target keeps a bonus so the bot does not flick between two equal ones. The game mode may favor some
//! (in GunGame the leader and a player one kill from winning).

use lb_core::Vec3;
use lb_core::dmath;
use lb_core::math::angle_diff;
use lb_core::time::SimTime;
use lb_knowledge::{EnemyTrack, PlayerKey, TrackState};

/// A target counts as aiming at the bot when its observed facing is this close to the bot's bearing.
pub const FACING_ME_DEGREES: f32 = 15.0;
const FIRING_RECENT: f64 = 1.0;
/// Another enemy takes over only when this much more pressing: facing and firing flicker as enemies strafe.
const CURRENT_BONUS: f32 = 1.6;

/// The track's observed facing points at `me`.
pub fn faces(track: &EnemyTrack, me: Vec3) -> bool {
    let d = me - track.pos;
    let toward_me = dmath::atan2(d.y, d.x).to_degrees();
    angle_diff(track.traits.facing, toward_me).abs() <= FACING_ME_DEGREES
}

/// How the game mode favors a target: its distance counts as `scale` of what it is, and its priority is `weight`
/// times more.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Favor {
    pub scale: f32,
    pub weight: f32,
}

impl Default for Favor {
    fn default() -> Self {
        Favor {
            scale: 1.0,
            weight: 1.0,
        }
    }
}

pub fn priority(track: &EnemyTrack, me: Vec3, now: SimTime, current: Option<PlayerKey>, favor: Favor) -> f32 {
    let d = track.pos.distance(me) * favor.scale;
    let mut p = favor.weight / (1.0 + (d / 600.0).powi(2));
    let mut threat = 1.0;
    if faces(track, me) {
        threat += 0.5;
    }
    if track.traits.fired_at.is_some_and(|t| now.since(t) <= FIRING_RECENT) {
        threat += 0.5;
    }
    p *= threat;
    if Some(track.who) == current {
        p *= CURRENT_BONUS;
    }
    p
}

/// The enemy to fight: the best priority among those in sight.
pub fn select<'a>(
    tracks: impl Iterator<Item = &'a EnemyTrack>,
    me: Vec3,
    now: SimTime,
    current: Option<PlayerKey>,
    favor: &dyn Fn(&EnemyTrack) -> Favor,
) -> Option<PlayerKey> {
    tracks
        .filter(|t| t.state == TrackState::Visible)
        .map(|t| (priority(t, me, now, current, favor(t)), t.who))
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, who)| who)
}
