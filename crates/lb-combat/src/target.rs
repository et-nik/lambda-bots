//! Target selection: among enemies in sight, the nearest counts most, one aiming at the bot or firing more, and
//! the current target keeps a bonus so the bot does not flick between two equal ones.

use lb_core::Vec3;
use lb_core::math::angle_diff;
use lb_core::time::SimTime;
use lb_knowledge::{EnemyTrack, PlayerKey, TrackState};

/// A target counts as aiming at the bot when its observed facing is this close to the bot's bearing.
pub const FACING_ME_DEGREES: f32 = 15.0;
const FIRING_RECENT: f64 = 1.0;
const CURRENT_BONUS: f32 = 1.3;

/// The track's observed facing points at `me`.
pub fn faces(track: &EnemyTrack, me: Vec3) -> bool {
    let d = me - track.pos;
    let toward_me = d.y.atan2(d.x).to_degrees();
    angle_diff(track.traits.facing, toward_me).abs() <= FACING_ME_DEGREES
}

pub fn priority(track: &EnemyTrack, me: Vec3, now: SimTime, current: Option<PlayerKey>) -> f32 {
    let d = track.pos.distance(me);
    let mut p = 1.0 / (1.0 + (d / 600.0).powi(2));
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
) -> Option<PlayerKey> {
    tracks
        .filter(|t| t.state == TrackState::Visible)
        .map(|t| (priority(t, me, now, current), t.who))
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, who)| who)
}
