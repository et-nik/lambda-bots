//! Offline movement checks with a `Tracer`: where a player rests at a spot, and whether one can walk between two
//! spots. The walk check is a greedy imitation of `PM_WalkMove`: short moves toward the target that keep partial
//! progress, step up to 18 u when that goes further, slide along walls they hit, and follow the floor down.

use lb_core::{Vec2, Vec3};
use lb_worldq::{HullKind, Trace, TraceQuery, Tracer};

pub const STEP_SIZE: f32 = 18.0;
/// Descents deeper than this in one move are falls, not walking.
pub const MAX_WALK_DROP: f32 = 64.0;
const MOVE: f32 = 16.0;
const MIN_FLOOR_NZ: f32 = 0.7;
/// Final height difference still counted as arriving at the target.
const ARRIVE_DZ: f32 = STEP_SIZE + 6.0;

/// Where a player standing (or crouching) at `origin` comes to rest; `None` if there is no floor within 96 u.
pub fn settle(tracer: &mut dyn Tracer, origin: Vec3, hull: HullKind) -> Option<Vec3> {
    let start = origin + Vec3::Z * 2.0;
    let tr = tracer.trace(&TraceQuery::hull(start, origin - Vec3::Z * 96.0, hull));
    (!tr.start_solid && !tr.all_solid && tr.fraction < 1.0).then_some(tr.end)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WalkCheck {
    Ok,
    /// Walkable, but falls `f32` units somewhere on the way.
    Drop(f32),
    Blocked,
    /// No floor to stand on part of the way.
    Gap,
}

fn stuck(tr: &Trace) -> bool {
    tr.start_solid || tr.all_solid
}

/// One horizontal move of at most `MOVE` toward `dir`, the better of straight and stepped up.
fn advance(tracer: &mut dyn Tracer, cur: Vec3, delta: Vec2, hull: HullKind) -> (Vec3, Option<Vec3>) {
    let direct = tracer.trace(&TraceQuery::hull(cur, cur + delta.extend(0.0), hull));
    let direct_end = if stuck(&direct) { cur } else { direct.end };
    let wall = (direct.fraction < 1.0).then_some(direct.normal);
    let up = tracer.trace(&TraceQuery::hull(cur, cur + Vec3::Z * STEP_SIZE, hull));
    if stuck(&up) {
        return (direct_end, wall);
    }
    let fwd = tracer.trace(&TraceQuery::hull(up.end, up.end + delta.extend(0.0), hull));
    if stuck(&fwd) {
        return (direct_end, wall);
    }
    let down = tracer.trace(&TraceQuery::hull(fwd.end, fwd.end - Vec3::Z * STEP_SIZE, hull));
    let stepped = if !stuck(&down) && down.fraction < 1.0 && down.normal.z >= MIN_FLOOR_NZ {
        down.end
    } else {
        fwd.end
    };
    let gain = |p: Vec3| (p - cur).truncate().length();
    if gain(stepped) > gain(direct_end) + 0.01 {
        (stepped, (fwd.fraction < 1.0).then_some(fwd.normal))
    } else {
        (direct_end, wall)
    }
}

/// Walks from `from` to `to` (both resting hull centres) with `hull`.
pub fn walk_check(tracer: &mut dyn Tracer, from: Vec3, to: Vec3, hull: HullKind) -> WalkCheck {
    let target = to.truncate();
    let total = (target - from.truncate()).length();
    if total < 1.0 {
        return if (to.z - from.z).abs() <= ARRIVE_DZ {
            WalkCheck::Ok
        } else {
            WalkCheck::Blocked
        };
    }
    let mut cur = from;
    let mut fall = 0.0f32;
    let mut best = total;
    let mut stalls = 0;
    let max_moves = (total / MOVE).ceil() as usize * 3 + 16;
    for _ in 0..max_moves {
        let remaining = target - cur.truncate();
        let left = remaining.length();
        if left < 1.0 {
            break;
        }
        let delta = remaining.clamp_length_max(MOVE);
        let (mut next, wall) = advance(tracer, cur, delta, hull);
        if (next - cur).truncate().length() < 1.0
            && let Some(n) = wall
        {
            // Slide along the wall, as the engine clips velocity against the planes it touches.
            let n2 = n.truncate();
            let slide = delta - n2 * delta.dot(n2);
            if slide.length() > 1.0 {
                next = advance(tracer, cur, slide, hull).0;
            }
        }
        let down = tracer.trace(&TraceQuery::hull(
            next,
            next - Vec3::Z * (STEP_SIZE + MAX_WALK_DROP),
            hull,
        ));
        if stuck(&down) {
            return WalkCheck::Blocked;
        }
        if down.fraction >= 1.0 {
            let deep = tracer.trace(&TraceQuery::hull(next, next - Vec3::Z * 1024.0, hull));
            if deep.fraction >= 1.0 || deep.normal.z < MIN_FLOOR_NZ {
                return WalkCheck::Gap;
            }
            fall = fall.max(next.z - deep.end.z);
            cur = deep.end;
        } else if down.normal.z < MIN_FLOOR_NZ {
            return WalkCheck::Blocked;
        } else {
            cur = down.end;
        }
        let d = (target - cur.truncate()).length();
        if d < best - 0.5 {
            best = d;
            stalls = 0;
        } else {
            stalls += 1;
            if stalls >= 4 {
                return WalkCheck::Blocked;
            }
        }
    }
    if (target - cur.truncate()).length() > 8.0 {
        return WalkCheck::Blocked;
    }
    if fall > 0.0 {
        return WalkCheck::Drop(fall);
    }
    if (cur.z - to.z).abs() > ARRIVE_DZ {
        WalkCheck::Blocked
    } else {
        WalkCheck::Ok
    }
}
