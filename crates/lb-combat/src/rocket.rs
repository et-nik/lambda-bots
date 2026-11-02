//! Rockets: how the game's rocket flies and what its blast does, for the aim and for when one is worth firing.
//!
//! - **Flight** (`rpg.cpp`): the rocket leaves 16 units in front of the eye at 250 units/s, 30° above the view, the
//!   shooter's own speed along the view added, under half gravity. It ignites at 0.4 s, and from 0.5 s on, every
//!   0.1 s, it turns to the view (unguided) or toward a laser spot, keeping a fifth of its speed and putting on 400
//!   units/s, up to 2000. An unguided one flies on above the view's line (5.6 units looking level, 12.6 at 30° up or
//!   down) and 8 units to its right.
//! - **Blast** (`ggrenade.cpp`, `combat.cpp`): it goes off pulled 58 units out of the surface the rocket hit, so on the
//!   floor at a player's feet it bursts inside his box; damage falls off linearly to nothing at 2.5 times the damage
//!   (300 units) from the victim's box, and in multiplayer a blast takes four fifths of what it does from armor, one
//!   for one. The line from the blast to a player takes the hitgroup it meets: one into his head triples it.
//! - **Hitboxes:** the laser's trace and the rocket (a point) meet a player's hitboxes, not his box: his shins are some
//!   12 units across, his hips and chest twice that.
//! - **Where to put one:** at the floor on the near side of the target's box, so low that a rocket (or the laser's
//!   line) passing between his legs still meets the floor just past him: a near miss bursts on the floor beside where he
//!   is, pulled up into his box, rather than on a wall far behind him. A guided rocket's lead shrinks as it flies, so
//!   the laser spot comes onto that floor as the rocket does. His middle, where his hitboxes are widest, is the aim
//!   where that floor is out of sight.
//! - **A trade:** how close a rocket bursting on the target's box may be fired for the shooter to keep some health.

use lb_core::Vec3;
use lb_worldq::{TraceQuery, Tracer};

/// The rocket leaves this far in front of the eye and this far under it, at these speeds along the view and up.
const MUZZLE: f32 = 16.0;
const MUZZLE_DROP: f32 = 8.0;
const LAUNCH_FORWARD: f32 = 216.5;
const LAUNCH_UP: f32 = 125.0;
/// Half of `sv_gravity` pulls it until it ignites; it turns first then, and every step after.
const GRAVITY: f32 = 400.0;
const IGNITE: f32 = 0.4;
const FIRST_TURN: f32 = 0.5;
const STEP: f32 = 0.1;
/// Each turn keeps this share of its speed and puts on this much along its new heading, up to the top speed.
const KEEP: f32 = 0.2;
const THRUST: f32 = 400.0;
const TOP_SPEED: f32 = 2000.0;
/// An unguided rocket flies this far to the right of the view's line.
pub const PATH_RIGHT: f32 = 8.0;
/// The unguided aim at the floor: at least this far over it, at most this far, and no higher than lets a rocket
/// passing the target's legs meet the floor this far past the point; this far in front of his box's center.
const FLOOR_MIN: f32 = 1.0;
const FLOOR_MAX: f32 = 4.0;
const PAST: f32 = 30.0;
const NEAR_SIDE: f32 = 20.0;
/// A player's box: half its width; and the gap between two players' centers and the box of one, as a blast on the
/// other's near side sees it.
pub const HALF_WIDTH: f32 = 16.0;
const BOX_GAP: f32 = 2.0 * HALF_WIDTH;
/// A floor this much below where a player stands is another level; one in the air is over his floor this much at
/// most (a jump, a bunny hop).
const FLOOR_STEP: f32 = 40.0;
const JUMP_HEIGHT: f32 = 80.0;
/// Armor takes this share of a blast in multiplayer, one point for each point of damage.
const ARMOR_SHARE: f32 = 0.8;

/// Seconds a rocket takes to get `distance` from the eye along the view, its shooter moving at `along` units/s along
/// it.
pub fn flight(distance: f32, along: f32) -> f32 {
    let d = distance - MUZZLE;
    if d <= 0.0 {
        return 0.0;
    }
    let mut v = LAUNCH_FORWARD + along;
    if v > 0.0 && d <= v * FIRST_TURN {
        return d / v;
    }
    let (mut x, mut t) = (v * FIRST_TURN, FIRST_TURN);
    let mut across = LAUNCH_UP - GRAVITY * IGNITE;
    for _ in 0..64 {
        let speed = (v * v + across * across).sqrt();
        v = KEEP * v + (1.0 - KEEP) * speed + THRUST;
        across *= KEEP;
        let total = (v * v + across * across).sqrt();
        if total > TOP_SPEED {
            v *= TOP_SPEED / total;
            across *= TOP_SPEED / total;
        }
        if v > 0.0 && d <= x + v * STEP {
            return t + (d - x) / v;
        }
        x += v * STEP;
        t += STEP;
    }
    t
}

/// How far above the view's line an unguided rocket is `t` seconds after the shot, the view's direction `level` long
/// along the ground (the cosine of its pitch, either way): it rises on its launch until it ignites, and its speed
/// across the view dies away from the first turn.
pub fn path_up(level: f32, t: f32) -> f32 {
    let g = GRAVITY * level;
    let rising = t.clamp(0.0, IGNITE);
    let mut up = -MUZZLE_DROP + LAUNCH_UP * rising - 0.5 * g * rising * rising;
    if t > IGNITE {
        let across = LAUNCH_UP - g * IGNITE;
        up += across * (t.min(FIRST_TURN) - IGNITE);
        if t > FIRST_TURN {
            up += across * STEP * KEEP / (1.0 - KEEP);
        }
    }
    up
}

/// How far a blast of `damage` goes off out of the surface the rocket hit.
pub fn pull_out(damage: f32) -> f32 {
    ((damage - 24.0) * 0.6).max(0.0)
}

/// Health a blast of `damage` takes from a player `gap` units from its center to his box, who has `armor`.
pub fn blast_hurt(gap: f32, damage: f32, armor: f32) -> f32 {
    let raw = (damage * (1.0 - gap.max(0.0) / (2.5 * damage.max(1.0)))).max(0.0);
    if armor >= ARMOR_SHARE * raw {
        raw * (1.0 - ARMOR_SHARE)
    } else {
        raw - armor.max(0.0)
    }
}

/// The least distance between the shooter's center and the target's at which a rocket of `damage` bursting on the
/// near side of the target's box leaves the shooter, with `health` and `armor`, at least `spare` health (the blast's
/// line into his body, not his head).
pub fn trade_min(health: f32, armor: f32, damage: f32, spare: f32) -> f32 {
    let radius = 2.5 * damage.max(1.0);
    let left = |d: f32| health - blast_hurt(d - BOX_GAP, damage, armor);
    if left(0.0) >= spare {
        return 0.0;
    }
    let (mut lo, mut hi) = (0.0_f32, radius + BOX_GAP);
    for _ in 0..24 {
        let mid = 0.5 * (lo + hi);
        if left(mid) >= spare {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    hi
}

/// Where on the target a rocket is put.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Height {
    /// His middle: where the floor in front of him is out of the rocket's way.
    Middle,
    /// The floor on the near side of his box.
    Floor,
}

/// Where to put a rocket at a player whose box's center is expected at `at` (seen at `from` now), `half_height` half
/// the height of his box, from `eye`, walls on the way to `at` stopping him short. The floor is looked for under him,
/// in the air within a jump's height; no floor found, or a point the eye cannot see, puts the aim on his center.
pub fn aim_point(
    eye: Vec3,
    from: Vec3,
    at: Vec3,
    half_height: f32,
    on_ground: bool,
    height: Height,
    tracer: &mut dyn Tracer,
) -> Vec3 {
    let level = Vec3::new(at.x, at.y, from.z);
    let way = level - from;
    let led = if way.length() > 1.0 {
        let t = tracer.trace(&TraceQuery::line(from, level));
        if t.fraction < 1.0 {
            from + way.normalize_or_zero() * (way.length() * t.fraction - HALF_WIDTH).max(0.0)
        } else {
            level
        }
    } else {
        level
    };
    let center = Vec3::new(led.x, led.y, at.z.max(from.z));
    let depth = half_height + if on_ground { FLOOR_STEP } else { JUMP_HEIGHT } + (at.z - from.z).abs();
    let down = tracer.trace(&TraceQuery::line(center, center - Vec3::Z * depth));
    if down.start_solid || down.fraction >= 1.0 {
        return center;
    }
    let floor = center.z - depth * down.fraction;
    let toward = (eye - center).truncate().normalize_or_zero();
    let base = Vec3::new(center.x + toward.x * NEAR_SIDE, center.y + toward.y * NEAR_SIDE, floor);
    let over = match height {
        Height::Middle => half_height,
        Height::Floor => {
            let across = (base - eye).truncate().length().max(1.0);
            (PAST * (eye.z - floor) / across).clamp(FLOOR_MIN, FLOOR_MAX)
        }
    };
    let point = base + Vec3::Z * over;
    let sight = tracer.trace(&TraceQuery::line(eye, point));
    if sight.fraction < 0.98 {
        Vec3::new(led.x, led.y, floor + half_height)
    } else {
        point
    }
}

/// The view's direction to put a point on, and the vectors to its right and up.
fn view_basis(eye: Vec3, point: Vec3) -> (Vec3, Vec3, Vec3) {
    let forward = (point - eye).normalize_or_zero();
    let right = forward.cross(Vec3::Z).normalize_or_zero();
    (forward, right, right.cross(forward))
}

/// Where an unguided rocket goes for the view on `view` from `eye`, `distance` along it, its shooter moving at `along`
/// units/s along the view.
pub fn unguided_path(eye: Vec3, view: Vec3, distance: f32, along: f32) -> Vec3 {
    let (forward, right, up) = view_basis(eye, view);
    eye + forward * distance + up * path_up(forward.truncate().length(), flight(distance, along)) + right * PATH_RIGHT
}

/// The first of a rocket's way along the view on `view` from `eye`, its shooter moving at `along` units/s along it:
/// where it leaves, the top of its rise (some 11 units over the view's line, 70 units out, standing still) and where it
/// turns first (to the view, or to a laser spot), with how far along the view each is.
pub fn rise(eye: Vec3, view: Vec3, along: f32) -> [(Vec3, f32); 3] {
    let (forward, right, up) = view_basis(eye, view);
    let level = forward.truncate().length();
    let speed = (LAUNCH_FORWARD + along).max(0.0);
    let top = (LAUNCH_UP / (GRAVITY * level).max(1.0)).min(IGNITE);
    let at = |t: f32| {
        let d = MUZZLE + speed * t;
        (eye + forward * d + up * path_up(level, t) + right * PATH_RIGHT, d)
    };
    [at(0.0), at(top), at(FIRST_TURN)]
}

/// The point to put the view on for an unguided rocket to fly at `point`: its path runs above the view's line and to
/// its right.
pub fn unguided_view(eye: Vec3, point: Vec3, along: f32) -> Vec3 {
    let (forward, right, up) = view_basis(eye, point);
    let t = flight(point.distance(eye), along);
    point - up * path_up(forward.truncate().length(), t) - right * PATH_RIGHT
}

#[cfg(test)]
mod tests {
    use super::*;
    use lb_worldq::{Trace, contents};

    /// Flat floor at z = 0, a wall across x = `wall` when given.
    struct Floor {
        wall: Option<f32>,
    }

    impl Tracer for Floor {
        fn trace(&mut self, q: &TraceQuery) -> Trace {
            let mut t = Trace::clear(q.end);
            if q.end.z < 0.0 && q.start.z >= 0.0 {
                t.fraction = q.start.z / (q.start.z - q.end.z);
            }
            if let Some(x) = self.wall
                && (q.start.x - x) * (q.end.x - x) < 0.0
            {
                t.fraction = t.fraction.min((x - q.start.x) / (q.end.x - q.start.x));
            }
            t
        }

        fn point_contents(&mut self, _p: Vec3) -> i32 {
            contents::EMPTY
        }
    }

    #[test]
    fn flight_follows_the_game() {
        // The game's steps, run by hand: 100 units at 0.39 s, 300 at 0.71, 600 at 0.89, 1000 at 1.09.
        assert_eq!(flight(10.0, 0.0), 0.0);
        for (d, t) in [(100.0, 0.39), (300.0, 0.71), (600.0, 0.89), (1000.0, 1.09)] {
            assert!((flight(d, 0.0) - t).abs() < 0.01, "{d}: {}", flight(d, 0.0));
        }
        assert!(flight(250.0, 0.0) > flight(200.0, 0.0) && flight(250.0, 0.0) < flight(300.0, 0.0));
        // The shooter's run carries the rocket on, backing off holds it back.
        assert!((flight(300.0, 300.0) - 0.53).abs() < 0.03, "{}", flight(300.0, 300.0));
        assert!(flight(150.0, -250.0) > 0.7, "{}", flight(150.0, -250.0));
    }

    #[test]
    fn an_unguided_path_runs_above_the_view() {
        assert!((path_up(1.0, 1.0) - 5.6).abs() < 0.1, "{}", path_up(1.0, 1.0));
        // 30° up or down.
        assert!((path_up(0.866, 1.0) - 12.6).abs() < 0.2, "{}", path_up(0.866, 1.0));
        assert!((path_up(1.0, 0.0) + 8.0).abs() < 1e-4, "it leaves under the eye");
        let eye = Vec3::new(0.0, 0.0, 64.0);
        let point = Vec3::new(600.0, 0.0, 4.0);
        let view = unguided_view(eye, point, 0.0);
        let goes = unguided_path(eye, view, view.distance(eye), 0.0);
        assert!(goes.distance(point) < 1.0, "{goes:?}");
        let [(from, d0), (top, d1), (turn, d2)] = rise(eye, Vec3::new(600.0, 0.0, 64.0), 0.0);
        assert_eq!(d0, 16.0);
        assert!(
            (from.z - 56.0).abs() < 1e-3 && (from.y + 8.0).abs() < 1e-3,
            "under the eye, to its right: {from}"
        );
        assert!((top.z - 75.5).abs() < 0.2 && (d1 - 83.7).abs() < 0.5, "{top} {d1}");
        assert!((turn.z - 70.5).abs() < 0.2 && (d2 - 124.25).abs() < 0.1, "{turn} {d2}");
    }

    #[test]
    fn blasts_hurt_as_in_multiplayer() {
        assert!((blast_hurt(0.0, 120.0, 0.0) - 120.0).abs() < 1e-3);
        assert!((blast_hurt(150.0, 120.0, 0.0) - 60.0).abs() < 1e-3);
        assert_eq!(blast_hurt(400.0, 120.0, 0.0), 0.0);
        // 100 armor takes four fifths of 120, 50 takes its 50 and leaves the rest to health.
        assert!((blast_hurt(0.0, 120.0, 100.0) - 24.0).abs() < 1e-3);
        assert!((blast_hurt(0.0, 120.0, 50.0) - 70.0).abs() < 1e-3);
    }

    #[test]
    fn a_trade_comes_closer_with_health_and_armor() {
        let full = trade_min(100.0, 0.0, 120.0, 15.0);
        // 85 lost at most: the blast 87.5 units from the shooter's box, its center 119.5 from the target's.
        assert!((full - 119.5).abs() < 0.5, "{full}");
        let hurt = trade_min(50.0, 0.0, 120.0, 15.0);
        assert!(hurt > 240.0 && hurt < 250.0, "{hurt}");
        assert!(trade_min(100.0, 100.0, 120.0, 15.0) < 40.0);
        assert!(trade_min(10.0, 0.0, 120.0, 15.0) > 300.0, "no trade leaves 15 of 10");
    }

    #[test]
    fn the_aim_is_low_on_the_near_side_or_in_the_middle() {
        let eye = Vec3::new(0.0, 0.0, 64.0);
        let at = Vec3::new(600.0, 0.0, 36.0);
        let open = &mut Floor { wall: None };
        // 30 units past the point over 580 seen from 64 up: some 3.3 over the floor.
        let p = aim_point(eye, at, at, 36.0, true, Height::Floor, open);
        assert!((p - Vec3::new(580.0, 0.0, 3.31)).length() < 0.01, "{p:?}");
        let p = aim_point(
            eye,
            Vec3::new(100.0, 0.0, 36.0),
            Vec3::new(100.0, 0.0, 36.0),
            36.0,
            true,
            Height::Floor,
            open,
        );
        assert!((p.z - 4.0).abs() < 1e-3, "no higher than the shins up close: {p:?}");
        let p = aim_point(eye, at, at, 36.0, true, Height::Middle, open);
        assert!((p.z - 36.0).abs() < 1e-3, "{p:?}");
        // In a low jump: the floor under him; high over it, his center.
        let jumping = Vec3::new(600.0, 0.0, 80.0);
        let p = aim_point(eye, jumping, jumping, 36.0, false, Height::Floor, open);
        assert!(p.z < 4.0 && p.z >= 1.0, "{p:?}");
        let flying = Vec3::new(600.0, 0.0, 200.0);
        assert_eq!(aim_point(eye, flying, flying, 36.0, false, Height::Floor, open), flying);
        // Led into a wall: stopped short of it. Led down into the floor (a landing's velocity): along it.
        let from = Vec3::new(500.0, 0.0, 36.0);
        let p = aim_point(
            eye,
            from,
            at,
            36.0,
            true,
            Height::Floor,
            &mut Floor { wall: Some(550.0) },
        );
        assert!(p.x < 534.0 && p.x > 500.0, "{p:?}");
        let p = aim_point(eye, from, Vec3::new(600.0, 0.0, -50.0), 36.0, true, Height::Floor, open);
        assert!(p.x > 570.0 && p.z >= 1.0, "{p:?}");
    }

    #[test]
    fn an_unguided_view_makes_up_for_the_path() {
        let eye = Vec3::new(0.0, 0.0, 64.0);
        let point = Vec3::new(600.0, 0.0, 4.0);
        let v = unguided_view(eye, point, 0.0);
        assert!(v.z < point.z - 5.0, "below: {v:?}");
        assert!(v.y > 7.0, "to the left (+y looking along +x): {v:?}");
    }
}
