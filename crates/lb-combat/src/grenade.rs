//! A hand grenade planned to go off where the enemy will be, with the game's own flight, bounces and fuse.
//!
//! - **Fuse** (`CHandGrenade::WeaponIdle`, `CGrenade::ShootTimed`, `TumbleThink`): the grenade goes off three
//!   seconds after the pin came out, but only on its own clock: it thinks every 0.1 s from the throw, and the think
//!   that finds the fuse run out sets the blast for the next one. A grenade thrown with `f` seconds of fuse left
//!   bursts `0.1·(⌈f/0.1⌉ + 1)` s after the throw, so how long the pin has been out when it leaves the hand picks the
//!   0.1 s tick it bursts at. The game throws no sooner than 0.5 s after the pin; a grenade still in hand at three
//!   seconds goes off there, so the bot lets go by [`LATEST`].
//! - **Flight and bounces** (`SV_Physics_Bounce`, `CGrenade::BounceTouch`): half gravity; a hit keeps the velocity
//!   along the surface and turns back a fifth of the rest (overbounce 2 − friction 0.8). On the floor the hops die
//!   out, and once it rolls each touch of the floor (about every 12 ms) keeps 0.8 of its speed: it stops some 0.05 s
//!   of its speed further on, below 30 units a second at once.
//! - **Blast** (`CGrenade::Detonate`, `Explode`, `RadiusDamage`): with the floor within 32 units below, the blast is
//!   lifted 45.6 units off it; 100 damage less 0.4 a unit from it to where its line meets the player's box, out to
//!   250 units, and only with nothing in between.
//!
//! The plan tries view pitches, each aimed so the grenade heads at where the enemy will be, flies each throw through
//! the world tick by tick, and weighs every tick it could burst at by the damage it would do there: the enemy where
//! its motion takes it, blurred by how far it may have turned since the throw (the longer the grenade is out, the
//! more), less what waiting longer with the pin out costs. Cooked so it bursts as it gets there, a grenade leaves no
//! time to run from it.

use lb_core::math::view_angle_vectors;
use lb_core::{Vec2, Vec3, dmath};
use lb_game::dll::DllProfile;
use lb_game::mechanics::{GRENADE_FUSE, GRENADE_MIN_COOK, PROJECTILE_GRAVITY};
use lb_worldq::{TraceQuery, Tracer};

use crate::ballistics::{Throw, grenade_launch};

/// The grenade's think interval: it bursts on one of these ticks after the throw.
pub const THINK: f32 = 0.1;
/// Longest the pin stays out: the grenade must be well out of the hand by the three seconds.
pub const LATEST: f32 = 2.6;
pub const DAMAGE: f32 = 100.0;
pub const RADIUS: f32 = 250.0;
/// The game throws on its idle frame after the button is let go: this long after, at most.
const LET_GO: f32 = 0.02;
/// The fuse is aimed this far inside its 0.1 s tick, clear of both edges.
const TICK_MARGIN: f32 = 0.03;
/// Bounces: the part of the velocity into the surface turned back (2 − the grenade's friction 0.8).
const OVERBOUNCE: f32 = 1.2;
const FLOOR_NORMAL: f32 = 0.7;
/// Hops off the floor slower than this are under a unit high: the grenade rolls.
const ROLL_VZ: f32 = 20.0;
/// Rolling, each touch of the floor keeps this much of the speed, a touch about this often.
const ROLL_KEEP: f32 = 0.8;
const ROLL_TOUCH: f32 = 0.012;
const REST_SPEED: f32 = 30.0;
/// The blast is lifted this far off a floor within `DETONATE_BELOW` under the grenade ((damage − 24)·0.6).
const LIFT: f32 = 45.6;
const DETONATE_BELOW: f32 = 32.0;
/// The blast's line ends on the player's box (`BodyTarget` is some 22 units above its origin, the box 16 across).
const BODY_UP: f32 = 22.0;
const BODY_HALF: f32 = 16.0;
/// An enemy in motion is followed along this share of its motion, for this long at most: players turn, and in
/// the stand's measures a full lead did no better than none.
const LEAD: f32 = 0.5;
const PREDICT_FOR: f32 = 0.6;
/// What a second more with the pin out costs, in damage: the bot holds a grenade instead of a gun.
const WAIT_COST: f32 = 3.0;
/// The bot keeps this far from its own blast.
pub const SELF_SAFE: f32 = RADIUS + 50.0;
/// View pitches tried (down positive): every few degrees, then finer around the best.
const PITCHES: [f32; 2] = [-56.0, 44.0];
const PITCH_STEP: f32 = 4.0;
const FINE_STEP: f32 = 0.75;
/// Candidates checked for the floor under the blast and a clear line to the enemy.
const VERIFY: usize = 4;

/// What the grenade is thrown at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aim {
    /// The enemy's origin and velocity now (a spot to cover: no velocity).
    pub pos: Vec3,
    pub vel: Vec3,
    /// How far off it may already be (1σ, units), and how fast that grows while the grenade is out (units a second).
    pub sigma: f32,
    pub spread: f32,
}

impl Aim {
    /// Where it will be `t` seconds from now.
    pub fn at(&self, t: f32) -> Vec3 {
        self.pos + self.vel.truncate().extend(0.0) * (LEAD * t.clamp(0.0, PREDICT_FOR))
    }
}

/// A planned grenade.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plan {
    /// The view to throw with; `flight` is the time from the throw to the blast, `start` and `velocity` the launch.
    pub throw: Throw,
    /// Let go once the pin has been out this long.
    pub release_held: f32,
    /// Where it bursts, and where the enemy should be then.
    pub burst: Vec3,
    pub target: Vec3,
    /// Damage it should do there (blurred by where the enemy may have gone), and that less the wait.
    pub damage: f32,
    pub score: f32,
}

/// The grenade's place on one think tick after the throw, and the floor it rolls or lies on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tick {
    pub at: Vec3,
    pub floor: Option<f32>,
}

/// The launch of a throw with view `pitch`, `yaw` from `eye` while moving at `velocity`.
pub fn launch(eye: Vec3, velocity: Vec3, pitch: f32, yaw: f32, dll: DllProfile) -> (Vec3, Vec3) {
    let (throw_pitch, speed) = grenade_launch(pitch, dll);
    let (forward, _, _) = view_angle_vectors(Vec3::new(throw_pitch, yaw, 0.0));
    (eye + forward * 16.0, forward * speed + velocity)
}

/// The yaw that sends a throw at view `pitch` along the ground at `at`, against the sideways part of the thrower's
/// own velocity; `None` when the throw cannot make up for it.
fn yaw_at(eye: Vec3, velocity: Vec3, pitch: f32, at: Vec3, dll: DllProfile) -> Option<f32> {
    let d = (at - eye).truncate();
    let dir = d.normalize_or_zero();
    if dir == Vec2::ZERO {
        return None;
    }
    let base = dmath::atan2(d.y, d.x).to_degrees();
    let (_, v) = launch(eye, Vec3::ZERO, pitch, base, dll);
    let own = v.truncate().length();
    let carried = velocity.truncate();
    let sideways = carried.dot(Vec2::new(-dir.y, dir.x));
    if own <= sideways.abs() {
        return None;
    }
    let along = (own * own - sideways * sideways).sqrt() + carried.dot(dir);
    if along <= 1.0 {
        return None;
    }
    Some(base - dmath::atan2(sideways, (own * own - sideways * sideways).sqrt()).to_degrees())
}

/// The grenade from `start` at `velocity` on each of `ticks` think ticks; `None` when it starts in a wall.
pub fn fly(tracer: &mut dyn Tracer, start: Vec3, velocity: Vec3, sv_gravity: f32, ticks: usize) -> Option<Vec<Tick>> {
    let g = sv_gravity * PROJECTILE_GRAVITY;
    let roll_rate = -dmath::ln(ROLL_KEEP) / ROLL_TOUCH;
    let (mut p, mut v) = (start, velocity);
    let mut floor: Option<f32> = None;
    let mut out = Vec::with_capacity(ticks);
    let mut t = 0.0f32;
    for k in 1..=ticks {
        let end = k as f32 * THINK;
        // A few hits within a tick at most; the rest of the tick is skipped past a corner it keeps meeting.
        for _ in 0..6 {
            let dt = end - t;
            if dt <= 1e-4 {
                break;
            }
            let speed = v.length();
            if floor.is_some() && speed < REST_SPEED {
                v = Vec3::ZERO;
                break;
            }
            if floor.is_some() {
                // Rolling: the speed falls off by the touches; it stops on reaching the rest speed.
                let stop = dmath::ln(speed / REST_SPEED) / roll_rate;
                let run = dt.min(stop);
                let to = p + v * ((1.0 - dmath::exp(-roll_rate * run)) / roll_rate);
                let tr = tracer.trace(&TraceQuery::line(p, to));
                if tr.start_solid {
                    return None;
                }
                if tr.fraction < 1.0 {
                    p = tr.end + tr.normal * 0.1;
                    v *= dmath::exp(-roll_rate * run * tr.fraction);
                    v -= tr.normal * (OVERBOUNCE * v.dot(tr.normal));
                    v.z = 0.0;
                    t += run * tr.fraction.max(0.05);
                    continue;
                }
                p = to;
                v = if run < dt {
                    Vec3::ZERO
                } else {
                    v * dmath::exp(-roll_rate * run)
                };
                // Rolled off an edge: it falls on.
                let down = tracer.trace(&TraceQuery::line(p + Vec3::Z, p - Vec3::Z * 4.0));
                if down.fraction >= 1.0 {
                    floor = None;
                } else {
                    floor = Some(down.end.z);
                    p.z = down.end.z;
                }
                break;
            }
            let to = p + v * dt - Vec3::Z * (0.5 * g * dt * dt);
            let tr = tracer.trace(&TraceQuery::line(p, to));
            if tr.start_solid || tr.all_solid {
                return None;
            }
            if tr.fraction >= 1.0 {
                p = to;
                v.z -= g * dt;
                break;
            }
            let hit = dt * tr.fraction;
            v.z -= g * hit;
            p = tr.end + tr.normal * 0.1;
            t += hit.max(1e-3);
            v -= tr.normal * (OVERBOUNCE * v.dot(tr.normal));
            if tr.normal.z > FLOOR_NORMAL && v.z < ROLL_VZ {
                v.z = 0.0;
                floor = Some(tr.end.z);
                p.z = tr.end.z;
            }
        }
        t = end;
        out.push(Tick { at: p, floor });
    }
    Some(out)
}

/// Where a grenade at `tick` bursts: lifted off a floor close under it.
fn burst_point(tracer: &mut dyn Tracer, tick: Tick) -> Vec3 {
    if let Some(fz) = tick.floor {
        return Vec3::new(tick.at.x, tick.at.y, fz + LIFT);
    }
    let from = tick.at + Vec3::Z * 8.0;
    let tr = tracer.trace(&TraceQuery::line(from, from - Vec3::Z * (DETONATE_BELOW + 8.0)));
    if tr.fraction < 1.0 {
        tr.end + tr.normal * LIFT
    } else {
        tick.at
    }
}

/// Damage of a blast at `burst` to a player standing at `origin`, as the game counts it (line not checked).
pub fn blast_damage(burst: Vec3, origin: Vec3) -> f32 {
    let d = ((burst + Vec3::Z) - (origin + Vec3::Z * BODY_UP)).length() - BODY_HALF;
    (DAMAGE - d.max(0.0) * (DAMAGE / RADIUS)).max(0.0)
}

/// Damage a blast at `burst` should do to a player expected at `origin`, off it by `sigma` units (1σ) either way
/// on the ground: the mean over rings of places it may be (quantiles of how far, eight ways round each).
fn expected_damage(burst: Vec3, origin: Vec3, sigma: f32) -> f32 {
    const QUANTILES: [f32; 5] = [0.1, 0.3, 0.5, 0.7, 0.9];
    const WAYS: usize = 8;
    if sigma <= 1.0 {
        return blast_damage(burst, origin);
    }
    let mut sum = 0.0;
    for (i, u) in QUANTILES.iter().enumerate() {
        let r = sigma * (-2.0 * dmath::ln(1.0 - u)).sqrt();
        for k in 0..WAYS {
            let a = (k as f32 + 0.5 * (i % 2) as f32) * std::f32::consts::TAU / WAYS as f32;
            let (sin, cos) = dmath::sin_cos(a);
            sum += blast_damage(burst, origin + Vec3::new(cos, sin, 0.0) * r);
        }
    }
    sum / (QUANTILES.len() * WAYS) as f32
}

/// How long the pin should have been out at the throw for the grenade to burst on tick `m` after it, let go with
/// the pin out `ready` at the soonest; `None` when that tick cannot be had any more.
pub fn release_for(m: u32, ready: f32) -> Option<f32> {
    // Fuse left at the throw in ((m − 2)·0.1, (m − 1)·0.1]: the blast on tick m.
    let lo = GRENADE_FUSE - (m as f32 - 1.0) * THINK + TICK_MARGIN;
    let hi = GRENADE_FUSE - (m as f32 - 2.0) * THINK - TICK_MARGIN - LET_GO;
    let soonest = ready.max(GRENADE_MIN_COOK);
    let at = lo.max(soonest);
    (at <= hi && at <= LATEST).then_some(at)
}

struct Candidate {
    pitch: f32,
    yaw: f32,
    start: Vec3,
    velocity: Vec3,
    tick: u32,
    held: f32,
    point: Tick,
    target: Vec3,
    damage: f32,
    score: f32,
}

/// The best grenade from `eye`, the thrower at `me` moving at `velocity`, at `aim`; the pin out `held` seconds now
/// (0 before it is pulled), the throw ready no sooner than `ready` seconds of it. Pitches around `near` only when
/// given (a plan made again a moment later). `None` when no throw would hurt the enemy and spare the thrower.
#[allow(clippy::too_many_arguments)]
pub fn plan(
    tracer: &mut dyn Tracer,
    eye: Vec3,
    me: Vec3,
    velocity: Vec3,
    aim: &Aim,
    sv_gravity: f32,
    dll: DllProfile,
    held: f32,
    ready: f32,
    near: Option<f32>,
) -> Option<Plan> {
    let soonest = ready.max(held);
    let last_tick = ((GRENADE_FUSE - soonest) / THINK).floor() as u32 + 2;
    // The yaw heads for where the enemy will be about when the grenade gets there; once more for the best found.
    let guess = (soonest - held) + (aim.pos - eye).truncate().length() / 800.0;
    let mut lead = aim.at(guess);
    let mut best: Option<Candidate> = None;
    let mut pitches: Vec<f32> = match near {
        Some(p) => (-3..=3).map(|i| p + i as f32 * FINE_STEP * 2.0).collect(),
        None => {
            let n = ((PITCHES[1] - PITCHES[0]) / PITCH_STEP) as i32;
            (0..=n).map(|i| PITCHES[0] + i as f32 * PITCH_STEP).collect()
        }
    };
    for pass in 0..2 {
        let mut found: Vec<Candidate> = Vec::new();
        for &pitch in &pitches {
            let Some(yaw) = yaw_at(eye, velocity, pitch, lead, dll) else {
                continue;
            };
            let (start, v) = launch(eye, velocity, pitch, yaw, dll);
            if tracer.trace(&TraceQuery::line(eye, start)).fraction < 1.0 {
                continue;
            }
            let Some(ticks) = fly(tracer, start, v, sv_gravity, last_tick as usize) else {
                continue;
            };
            let mut top: Option<Candidate> = None;
            for (i, tick) in ticks.iter().enumerate() {
                let m = i as u32 + 1;
                let Some(at) = release_for(m, soonest) else {
                    continue;
                };
                let wait = at - held;
                let flight = m as f32 * THINK;
                let point = Vec3::new(tick.at.x, tick.at.y, tick.floor.map_or(tick.at.z, |f| f + LIFT));
                if point.distance(me) < SELF_SAFE {
                    continue;
                }
                let target = aim.at(wait + flight);
                let damage = expected_damage(point, target, aim.sigma + aim.spread * flight);
                let score = damage - WAIT_COST * wait;
                if damage > 0.0 && top.as_ref().is_none_or(|c| score > c.score) {
                    top = Some(Candidate {
                        pitch,
                        yaw,
                        start,
                        velocity: v,
                        tick: m,
                        held: at,
                        point: *tick,
                        target,
                        damage,
                        score,
                    });
                }
            }
            found.extend(top);
        }
        found.sort_by(|a, b| b.score.total_cmp(&a.score));
        // The best few: the blast where the floor really is, and a clear line from it to the enemy.
        for mut c in found.into_iter().take(VERIFY) {
            let burst = burst_point(tracer, c.point);
            if burst.distance(me) < SELF_SAFE {
                continue;
            }
            let body = c.target + Vec3::Z * BODY_UP;
            let tr = tracer.trace(&TraceQuery::line(burst + Vec3::Z, body));
            if tr.fraction < 1.0 && tr.end.distance(body) > BODY_HALF {
                continue;
            }
            let flight = c.tick as f32 * THINK;
            c.damage = expected_damage(burst, c.target, aim.sigma + aim.spread * flight);
            c.score = c.damage - WAIT_COST * (c.held - held);
            c.point.at = burst;
            if c.damage > 0.0 && best.as_ref().is_none_or(|b| c.score > b.score) {
                best = Some(c);
            }
        }
        let b = best.as_ref()?;
        if pass == 0 {
            lead = aim.at(b.held - held + b.tick as f32 * THINK);
            let p = b.pitch;
            pitches = (-3..=3).map(|i| p + i as f32 * FINE_STEP).collect();
            best = None;
        }
    }
    best.map(|c| Plan {
        throw: Throw {
            pitch: c.pitch,
            yaw: c.yaw,
            flight: c.tick as f32 * THINK,
            start: c.start,
            velocity: c.velocity,
        },
        release_held: c.held,
        burst: c.point.at,
        target: c.target,
        damage: c.damage,
        score: c.score,
    })
}

/// Ways and pitches a grenade that must go now is tried at.
const DUMP_WAYS: usize = 8;
const DUMP_PITCHES: [f32; 4] = [-35.0, -15.0, 0.0, 15.0];

/// A grenade that must leave the hand now with no throw at anyone to be had (the fuse running out, the bot boxed
/// in): the way and pitch whose blast, on the tick the fuse gives it, is farthest from the bot. `None` when it
/// cannot leave the hand at all (no way out of a wall).
#[allow(clippy::too_many_arguments)]
pub fn dump(
    tracer: &mut dyn Tracer,
    eye: Vec3,
    me: Vec3,
    velocity: Vec3,
    sv_gravity: f32,
    dll: DllProfile,
    held: f32,
) -> Option<Plan> {
    let fuse = (GRENADE_FUSE - held - LET_GO).max(THINK);
    let m = (fuse / THINK).ceil() as u32 + 1;
    let mut best: Option<(f32, Plan)> = None;
    for i in 0..DUMP_WAYS {
        let yaw = i as f32 * 360.0 / DUMP_WAYS as f32;
        for &pitch in &DUMP_PITCHES {
            let (start, v) = launch(eye, velocity, pitch, yaw, dll);
            if tracer.trace(&TraceQuery::line(eye, start)).fraction < 1.0 {
                continue;
            }
            let Some(ticks) = fly(tracer, start, v, sv_gravity, m as usize) else {
                continue;
            };
            let Some(&tick) = ticks.last() else { continue };
            let burst = burst_point(tracer, tick);
            let off = burst.distance(me);
            if best.as_ref().is_none_or(|b| off > b.0) {
                let throw = Throw {
                    pitch,
                    yaw,
                    flight: m as f32 * THINK,
                    start,
                    velocity: v,
                };
                let plan = Plan {
                    throw,
                    release_held: held,
                    burst,
                    target: burst,
                    damage: 0.0,
                    score: off,
                };
                best = Some((off, plan));
            }
        }
    }
    best.map(|b| b.1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lb_worldq::{Trace, contents};

    /// A flat floor at z = −36, and a wall across x = `wall` when given.
    struct Room {
        wall: Option<f32>,
    }

    impl Tracer for Room {
        fn trace(&mut self, q: &TraceQuery) -> Trace {
            let mut t = Trace::clear(q.end);
            let d = q.end - q.start;
            let mut hit = |f: f32, n: Vec3| {
                if (0.0..t.fraction).contains(&f) {
                    t.fraction = f;
                    t.normal = n;
                    t.end = q.start + d * f;
                }
            };
            if q.end.z < -36.0 && q.start.z >= -36.0 && d.z != 0.0 {
                hit((-36.0 - q.start.z) / d.z, Vec3::Z);
            }
            if let Some(x) = self.wall
                && (q.start.x - x) * (q.end.x - x) < 0.0
            {
                hit((x - q.start.x) / d.x, Vec3::new(-d.x.signum(), 0.0, 0.0));
            }
            t
        }

        fn point_contents(&mut self, _p: Vec3) -> i32 {
            contents::EMPTY
        }
    }

    const EYE: Vec3 = Vec3::new(0.0, 0.0, 28.0);

    #[test]
    fn the_tick_of_the_blast_is_picked_by_how_long_the_pin_was_out() {
        // Thrown with 1.0 s of fuse left (held 2.0): the think at 1.0 finds it run out, the blast on 1.1.
        let at = release_for(11, 0.6).unwrap();
        let fuse = GRENADE_FUSE - at;
        assert!(fuse > 0.9 && fuse <= 1.0, "{fuse}");
        // A tick already gone is not had; nor one that needs the pin out past the latest.
        assert_eq!(release_for(11, 2.5), None);
        assert_eq!(release_for(3, 0.6), None);
        // Not sooner than the game throws: 2.5 s of fuse left at the soonest, the blast on tick 26.
        assert_eq!(release_for(27, 0.0), None);
        assert!(release_for(26, 0.0).unwrap() >= GRENADE_MIN_COOK);
    }

    #[test]
    fn a_flat_throw_hops_and_rolls_on_along_the_floor() {
        let dll = DllProfile::resolve("auto", true);
        let (start, v) = launch(EYE, Vec3::new(270.0, 0.0, 0.0), 7.0, 0.0, dll);
        let ticks = fly(&mut Room { wall: None }, start, v, 800.0, 30).unwrap();
        let rest = ticks.last().unwrap();
        assert_eq!(rest.floor, Some(-36.0));
        assert!((rest.at.z + 36.0).abs() < 0.5);
        // The hops after the first touch keep the speed along the floor: it gets far past it.
        let first = ticks.iter().position(|t| t.at.z < -30.0).unwrap();
        assert!(rest.at.x > ticks[first].at.x + 100.0, "{ticks:?}");
        assert!(ticks.windows(2).all(|w| w[1].at.x >= w[0].at.x - 0.01));
        // And it comes to rest.
        assert_eq!(ticks[28].at, ticks[29].at);
    }

    #[test]
    fn a_wall_turns_it_back_slowed() {
        let dll = DllProfile::resolve("auto", true);
        let (start, v) = launch(EYE, Vec3::ZERO, -10.0, 0.0, dll);
        let ticks = fly(&mut Room { wall: Some(300.0) }, start, v, 800.0, 25).unwrap();
        assert!(ticks.iter().all(|t| t.at.x < 300.0));
        let rest = ticks.last().unwrap().at.x;
        assert!(rest < 290.0 && rest > 0.0, "{rest}");
    }

    #[test]
    fn it_is_cooked_to_burst_on_a_standing_enemy() {
        let dll = DllProfile::resolve("auto", true);
        let aim = Aim {
            pos: Vec3::new(600.0, 0.0, 0.0),
            vel: Vec3::ZERO,
            sigma: 20.0,
            spread: 110.0,
        };
        let p = plan(
            &mut Room { wall: None },
            EYE,
            Vec3::ZERO,
            Vec3::ZERO,
            &aim,
            800.0,
            dll,
            0.0,
            0.0,
            None,
        )
        .expect("a plan");
        assert!(blast_damage(p.burst, aim.pos) > 70.0, "{p:?}");
        assert!((GRENADE_MIN_COOK..=LATEST).contains(&p.release_held), "{p:?}");
        // Cooked: it bursts within a second of the throw, as it gets there.
        assert!(p.throw.flight <= 1.0, "{p:?}");
    }

    #[test]
    fn a_running_enemy_is_led() {
        let dll = DllProfile::resolve("auto", true);
        let aim = Aim {
            pos: Vec3::new(600.0, 0.0, 0.0),
            vel: Vec3::new(0.0, 250.0, 0.0),
            sigma: 20.0,
            spread: 110.0,
        };
        let p = plan(
            &mut Room { wall: None },
            EYE,
            Vec3::ZERO,
            Vec3::ZERO,
            &aim,
            800.0,
            dll,
            0.6,
            0.6,
            None,
        )
        .expect("a plan");
        assert!(p.burst.y > 40.0, "led: {p:?}");
        assert!(p.burst.distance(p.target) < 120.0, "{p:?}");
    }

    #[test]
    fn the_run_goes_into_the_throw() {
        let dll = DllProfile::resolve("auto", true);
        let aim = Aim {
            pos: Vec3::new(900.0, 0.0, 0.0),
            vel: Vec3::ZERO,
            sigma: 20.0,
            spread: 110.0,
        };
        let standing = plan(
            &mut Room { wall: None },
            EYE,
            Vec3::ZERO,
            Vec3::ZERO,
            &aim,
            800.0,
            dll,
            0.6,
            0.6,
            None,
        )
        .expect("a plan");
        let running = plan(
            &mut Room { wall: None },
            EYE,
            Vec3::ZERO,
            Vec3::new(270.0, 0.0, 0.0),
            &aim,
            800.0,
            dll,
            0.6,
            0.6,
            None,
        )
        .expect("a plan");
        assert!(
            running.throw.velocity.x > standing.throw.velocity.x,
            "{standing:?} {running:?}"
        );
        assert!(blast_damage(running.burst, aim.pos) > 60.0, "{running:?}");
    }

    #[test]
    fn a_grenade_that_must_go_goes_away_from_the_wall() {
        let dll = DllProfile::resolve("auto", true);
        // A wall 40 units ahead along +x: the blast goes the other way, out of reach of the bot.
        let p = dump(
            &mut Room { wall: Some(40.0) },
            EYE,
            Vec3::ZERO,
            Vec3::ZERO,
            800.0,
            dll,
            2.0,
        )
        .expect("a way");
        assert!(p.burst.x < -150.0, "{p:?}");
        assert!(p.burst.distance(Vec3::ZERO) >= SELF_SAFE, "{p:?}");
        assert_eq!(p.release_held, 2.0);
    }

    #[test]
    fn never_its_own_blast() {
        let dll = DllProfile::resolve("auto", true);
        let aim = Aim {
            pos: Vec3::new(150.0, 0.0, 0.0),
            vel: Vec3::ZERO,
            sigma: 20.0,
            spread: 110.0,
        };
        if let Some(p) = plan(
            &mut Room { wall: None },
            EYE,
            Vec3::ZERO,
            Vec3::ZERO,
            &aim,
            800.0,
            dll,
            0.0,
            0.0,
            None,
        ) {
            assert!(p.burst.distance(Vec3::ZERO) >= SELF_SAFE, "{p:?}");
        }
        // Behind a wall close by: the throw would come back.
        let aim = Aim {
            pos: Vec3::new(600.0, 0.0, 0.0),
            ..aim
        };
        if let Some(p) = plan(
            &mut Room { wall: Some(120.0) },
            EYE,
            Vec3::ZERO,
            Vec3::ZERO,
            &aim,
            800.0,
            dll,
            0.0,
            0.0,
            None,
        ) {
            panic!("thrown into a wall: {p:?}");
        }
    }
}
