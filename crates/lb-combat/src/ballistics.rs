//! Ballistics of thrown and launched explosives, with the game's own launch rules:
//!
//! - **Hand grenade** (`CHandGrenade::WeaponIdle`): the view pitch `p` (down positive) becomes the throw pitch
//!   `−10 + p·80/90` looking up and `−10 + p·100/90` looking down; the speed is `(90 − pitch′)·k`, capped, with the
//!   DLL's `k` ([`DllProfile::grenade_speed`]); the thrower's velocity is added; it leaves 16 units in front of the
//!   eyes.
//! - **M203 grenade** (`CGrenade::ShootContact`): 800 units per second along the view from the same point, without
//!   the thrower's velocity; it explodes on contact.
//! - **Satchel** (`CSatchel::Throw`): 274 units per second along the view plus the thrower's velocity, from the
//!   thrower's origin; it slides after landing.
//!
//! All of them fall at half of `sv_gravity`. A throw is solved for the view pitch that lands on a point, low arc
//! first, and checked for walls along the arc with a few traces.

use lb_core::Vec3;
use lb_core::dmath;
use lb_core::math::view_angle_vectors;
use lb_game::dll::DllProfile;
use lb_game::mechanics::{M203_SPEED, PROJECTILE_GRAVITY, SATCHEL_SPEED};
use lb_worldq::{TraceQuery, Tracer};

/// A throw that lands on the target: view angles to hold when it leaves the hand.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Throw {
    /// View pitch (down positive) and yaw, degrees.
    pub pitch: f32,
    pub yaw: f32,
    /// Seconds from release to landing.
    pub flight: f32,
    pub start: Vec3,
    pub velocity: Vec3,
}

/// A tracer that finds nothing in the way: for solving a throw again when only the thrower's motion changed.
pub struct Unchecked;

impl Tracer for Unchecked {
    fn trace(&mut self, q: &TraceQuery) -> lb_worldq::Trace {
        lb_worldq::Trace::clear(q.end)
    }

    fn point_contents(&mut self, _p: Vec3) -> i32 {
        lb_worldq::contents::EMPTY
    }
}

/// Segments an arc is checked in.
const ARC_CHECKS: usize = 8;
/// A throw's arc must be clear this far off it on every side.
const ARC_MARGIN: f32 = 8.0;
/// The pitch search: every degree, then bisection to this precision.
const PITCH_PRECISION: f32 = 0.05;

/// Where a projectile launched from `start` at `velocity` is after `t` seconds, falling at `gravity`.
pub fn position_at(start: Vec3, velocity: Vec3, gravity: f32, t: f32) -> Vec3 {
    start + velocity * t - Vec3::Z * (0.5 * gravity * t * t)
}

/// Seconds until a projectile from `start` at `velocity` comes down to height `z` (on its way down); `None` when it
/// never gets there.
pub fn time_to_height(start: Vec3, velocity: Vec3, gravity: f32, z: f32) -> Option<f32> {
    let (a, b, c) = (-0.5 * gravity, velocity.z, start.z - z);
    if gravity <= 0.0 {
        return (b != 0.0).then(|| -c / b).filter(|t| *t >= 0.0);
    }
    let disc = b * b - 4.0 * a * c;
    if disc < 0.0 {
        return None;
    }
    let t = (-b - disc.sqrt()) / (2.0 * a);
    (t >= 0.0).then_some(t)
}

/// The arc from `start` at `velocity` is clear of walls for `flight` seconds (the last segment may end in the
/// target's floor).
pub fn arc_clear(tracer: &mut dyn Tracer, start: Vec3, velocity: Vec3, gravity: f32, flight: f32) -> bool {
    // A little off the arc on every side as well: a throw a degree or so off would meet an edge it grazes, and
    // bounce back.
    let across = velocity.truncate().normalize_or_zero().perp().extend(0.0) * ARC_MARGIN;
    let up = Vec3::Z * ARC_MARGIN;
    [Vec3::ZERO, across, -across, up, -up].into_iter().all(|off| {
        let mut from = start;
        for i in 1..=ARC_CHECKS {
            let to = position_at(start, velocity, gravity, flight * i as f32 / ARC_CHECKS as f32);
            // The margin fades toward the landing, which is on the floor.
            let fade = if i == ARC_CHECKS { 0.0 } else { 1.0 };
            let tr = tracer.trace(&TraceQuery::line(from + off * fade, to + off * fade));
            if tr.start_solid || (tr.fraction < 1.0 && (i < ARC_CHECKS || tr.end.distance(to) > 24.0)) {
                return false;
            }
            from = to;
        }
        true
    })
}

/// Throw pitch and speed of the hand grenade for view pitch `p`.
pub fn grenade_launch(p: f32, dll: DllProfile) -> (f32, f32) {
    let pitch = if p < 0.0 {
        -10.0 + p * (80.0 / 90.0)
    } else {
        -10.0 + p * (100.0 / 90.0)
    };
    let (k, cap) = dll.grenade_speed();
    (pitch, ((90.0 - pitch) * k).clamp(0.0, cap))
}

/// How a launcher turns the view into a start point and a velocity.
trait Launcher {
    fn launch(&self, pitch: f32, yaw: f32) -> (Vec3, Vec3);
    /// The thrower's velocity the launch carries.
    fn carried(&self) -> Vec3;
}

struct Grenade {
    eye: Vec3,
    velocity: Vec3,
    dll: DllProfile,
}

impl Launcher for Grenade {
    fn launch(&self, pitch: f32, yaw: f32) -> (Vec3, Vec3) {
        let (throw_pitch, speed) = grenade_launch(pitch, self.dll);
        let (forward, _, _) = view_angle_vectors(Vec3::new(throw_pitch, yaw, 0.0));
        (self.eye + forward * 16.0, forward * speed + self.velocity)
    }

    fn carried(&self) -> Vec3 {
        self.velocity
    }
}

struct Straight {
    from: Vec3,
    /// Added to the start along the view.
    ahead: f32,
    speed: f32,
    velocity: Vec3,
}

impl Launcher for Straight {
    fn launch(&self, pitch: f32, yaw: f32) -> (Vec3, Vec3) {
        let (forward, _, _) = view_angle_vectors(Vec3::new(pitch, yaw, 0.0));
        (self.from + forward * self.ahead, forward * self.speed + self.velocity)
    }

    fn carried(&self) -> Vec3 {
        self.velocity
    }
}

/// The yaw that sends a launch at view `pitch` straight at `target` from `from`: turned against the sideways part
/// of the velocity the thrower carries. `None` when the throw cannot make up for it.
fn aim_yaw(l: &dyn Launcher, pitch: f32, from: Vec3, target: Vec3) -> Option<f32> {
    let carried = l.carried().truncate();
    let mut start = from;
    let mut yaw = 0.0;
    // The launch point moves with the yaw; twice is enough to settle it.
    for _ in 0..2 {
        let d = (target - start).truncate();
        let base = dmath::atan2(d.y, d.x).to_degrees();
        let dir = d.normalize_or_zero();
        let (_, v) = l.launch(pitch, base);
        let own = (v.truncate() - carried).length();
        let sideways = carried.dot(lb_core::Vec2::new(-dir.y, dir.x));
        if own <= sideways.abs() {
            return None;
        }
        yaw = base - dmath::atan2(sideways, (own * own - sideways * sideways).sqrt()).to_degrees();
        start = l.launch(pitch, yaw).0;
    }
    Some(yaw)
}

/// Signed height miss at the target's distance for view `pitch` (positive: over), the flight time and the yaw;
/// `None` when the throw does not get that far.
fn miss(l: &dyn Launcher, pitch: f32, from: Vec3, target: Vec3, gravity: f32) -> Option<(f32, f32, f32)> {
    let yaw = aim_yaw(l, pitch, from, target)?;
    let (start, v) = l.launch(pitch, yaw);
    let to = (target - start).truncate();
    let dir = to.normalize_or_zero();
    let along = v.truncate().dot(dir);
    if along <= 1.0 {
        return None;
    }
    let t = to.length() / along;
    let z = position_at(start, v, gravity, t).z;
    Some((z - target.z, t, yaw))
}

/// View angles that land a launch on `target`: the lowest arc that reaches it within `max_flight` seconds with a
/// clear path, else the higher one.
fn solve(
    l: &dyn Launcher,
    target: Vec3,
    from: Vec3,
    gravity: f32,
    max_flight: f32,
    tracer: &mut dyn Tracer,
) -> Option<Throw> {
    // Down-positive view pitch from the flattest throw upward; each crossing of the target height is a solution.
    let mut roots = Vec::new();
    let mut previous: Option<(f32, f32)> = None;
    for step in 0..=178 {
        let pitch = 89.0 - step as f32;
        let current = miss(l, pitch, from, target, gravity).map(|(m, _, _)| (pitch, m));
        if let (Some((p0, m0)), Some((p1, m1))) = (previous, current)
            && m0.signum() != m1.signum()
        {
            roots.push(refine(l, [p0, p1], [m0, m1], from, target, gravity));
        }
        previous = current;
    }
    for pitch in roots {
        let Some((_, flight, yaw)) = miss(l, pitch, from, target, gravity) else {
            continue;
        };
        if flight > max_flight {
            continue;
        }
        let (start, velocity) = l.launch(pitch, yaw);
        if arc_clear(tracer, start, velocity, gravity, flight) {
            return Some(Throw {
                pitch,
                yaw,
                flight,
                start,
                velocity,
            });
        }
    }
    None
}

fn refine(l: &dyn Launcher, mut p: [f32; 2], mut m: [f32; 2], from: Vec3, target: Vec3, gravity: f32) -> f32 {
    while (p[0] - p[1]).abs() > PITCH_PRECISION {
        let mid = 0.5 * (p[0] + p[1]);
        let Some((mm, _, _)) = miss(l, mid, from, target, gravity) else {
            break;
        };
        if mm.signum() == m[0].signum() {
            p[0] = mid;
            m[0] = mm;
        } else {
            p[1] = mid;
            m[1] = mm;
        }
    }
    0.5 * (p[0] + p[1])
}

/// A hand grenade thrown from `eye` while moving at `velocity` that lands on `target` within `max_flight` seconds.
#[allow(clippy::too_many_arguments)]
pub fn grenade(
    tracer: &mut dyn Tracer,
    eye: Vec3,
    velocity: Vec3,
    target: Vec3,
    sv_gravity: f32,
    dll: DllProfile,
    max_flight: f32,
) -> Option<Throw> {
    let l = Grenade { eye, velocity, dll };
    solve(&l, target, eye, sv_gravity * PROJECTILE_GRAVITY, max_flight, tracer)
}

/// An M203 grenade from `eye` that lands on `target`.
pub fn m203(tracer: &mut dyn Tracer, eye: Vec3, target: Vec3, sv_gravity: f32) -> Option<Throw> {
    let l = Straight {
        from: eye,
        ahead: 16.0,
        speed: M203_SPEED,
        velocity: Vec3::ZERO,
    };
    solve(&l, target, eye, sv_gravity * PROJECTILE_GRAVITY, 3.0, tracer)
}

/// A satchel thrown from `origin` while moving at `velocity` toward `target`: the throw that lands on it, or the
/// longest one when it is out of reach (the charge slides on).
pub fn satchel(tracer: &mut dyn Tracer, origin: Vec3, velocity: Vec3, target: Vec3, sv_gravity: f32) -> Throw {
    let l = Straight {
        from: origin,
        ahead: 0.0,
        speed: SATCHEL_SPEED,
        velocity,
    };
    let gravity = sv_gravity * PROJECTILE_GRAVITY;
    if let Some(t) = solve(&l, target, origin, gravity, 3.0, tracer) {
        return t;
    }
    let d = target - origin;
    let yaw = dmath::atan2(d.y, d.x).to_degrees();
    let pitch = -45.0;
    let (start, velocity) = l.launch(pitch, yaw);
    let flight = time_to_height(start, velocity, gravity, target.z).unwrap_or(1.0);
    Throw {
        pitch,
        yaw,
        flight,
        start,
        velocity,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lb_game::dll::{DllKind, DllProfile};
    use lb_worldq::{Trace, contents};

    struct Open;
    impl Tracer for Open {
        fn trace(&mut self, q: &TraceQuery) -> Trace {
            Trace::clear(q.end)
        }
        fn point_contents(&mut self, _p: Vec3) -> i32 {
            contents::EMPTY
        }
    }

    /// A wall across x = 150 from the floor up to z = 150.
    struct Wall;
    impl Tracer for Wall {
        fn trace(&mut self, q: &TraceQuery) -> Trace {
            let (a, b) = (q.start, q.end);
            if (a.x - 150.0) * (b.x - 150.0) < 0.0 {
                let f = (150.0 - a.x) / (b.x - a.x);
                let p = a + (b - a) * f;
                if p.z < 150.0 {
                    let mut t = Trace::clear(p);
                    t.fraction = f;
                    return t;
                }
            }
            Trace::clear(b)
        }
        fn point_contents(&mut self, _p: Vec3) -> i32 {
            contents::EMPTY
        }
    }

    const BHL: DllProfile = DllProfile {
        kind: DllKind::Bugfixed,
        detected: true,
        satchel: lb_game::dll::SatchelButtons::PrimaryThrows,
    };

    fn lands(t: &Throw, gravity: f32, target: Vec3) -> f32 {
        position_at(t.start, t.velocity, gravity * PROJECTILE_GRAVITY, t.flight).distance(target)
    }

    #[test]
    fn the_game_turns_the_view_into_a_throw() {
        assert_eq!(grenade_launch(0.0, BHL), (-10.0, 650.0));
        assert_eq!(grenade_launch(-90.0, BHL).1, 1000.0, "capped");
        let classic = DllProfile::resolve("classic", false);
        assert_eq!(grenade_launch(0.0, classic), (-10.0, 400.0));
        assert!(grenade_launch(90.0, BHL).1.abs() < 1e-3);
    }

    #[test]
    fn grenades_land_on_the_target_and_go_over_walls() {
        let eye = Vec3::new(0.0, 0.0, 64.0);
        let target = Vec3::new(500.0, 200.0, 0.0);
        let t = grenade(&mut Open, eye, Vec3::ZERO, target, 800.0, BHL, 3.0).expect("in reach");
        assert!(lands(&t, 800.0, target) < 2.0, "{t:?}");
        assert!(t.flight < 1.5, "the low arc: {t:?}");
        let running = Vec3::new(0.0, 250.0, 0.0);
        let t = grenade(&mut Open, eye, running, target, 800.0, BHL, 3.0).unwrap();
        assert!(lands(&t, 800.0, target) < 2.0, "the run is part of the throw: {t:?}");
        let near = Vec3::new(300.0, 0.0, 0.0);
        assert!(
            grenade(&mut Wall, eye, Vec3::ZERO, near, 800.0, BHL, 3.0).is_none(),
            "BHL throws upward at 650 and more: the arc over the wall outlasts the fuse"
        );
        let classic = DllProfile::resolve("classic", false);
        let low = grenade(&mut Open, eye, Vec3::ZERO, near, 800.0, classic, 3.0).unwrap();
        let over = grenade(&mut Wall, eye, Vec3::ZERO, near, 800.0, classic, 3.0).unwrap();
        assert!(over.pitch < low.pitch - 30.0, "lobbed over the wall: {low:?} {over:?}");
        assert!(lands(&over, 800.0, near) < 2.0);
        assert!(grenade(&mut Open, eye, Vec3::ZERO, Vec3::new(3000.0, 0.0, 0.0), 800.0, BHL, 3.0).is_none());
    }

    #[test]
    fn m203_and_satchel() {
        let eye = Vec3::new(0.0, 0.0, 64.0);
        let target = Vec3::new(600.0, 0.0, 0.0);
        let t = m203(&mut Open, eye, target, 800.0).unwrap();
        assert!(lands(&t, 800.0, target) < 2.0);
        assert!(t.pitch < 0.0 && t.pitch > -20.0, "a low arc up: {t:?}");
        let s = satchel(&mut Open, Vec3::ZERO, Vec3::ZERO, Vec3::new(150.0, 0.0, -36.0), 800.0);
        assert!(lands(&s, 800.0, Vec3::new(150.0, 0.0, -36.0)) < 2.0, "{s:?}");
        let far = satchel(&mut Open, Vec3::ZERO, Vec3::ZERO, Vec3::new(600.0, 0.0, -36.0), 800.0);
        assert_eq!(far.pitch, -45.0, "the longest throw when out of reach");
    }
}
