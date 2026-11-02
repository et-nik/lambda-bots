//! The hand: the view moved as a player's hand moves a mouse. Mouse counts turn into view angles directly, so the hand
//! works in the plane of pitch and yaw. Its numbers come from a player's aim recorded on the stand (`lb_aimlog`,
//! 2026-11): every move, and every flick at an enemy, of twenty minutes of deathmatch.
//!
//! - **Strokes.** A move is a stroke with the symmetric bell of speed a quick hand movement has (the minimum-jerk
//!   profile of Flash and Hogan, the recorded player's mean profile), its top a little before or after the middle
//!   by the stroke, along an arc: the wrist or the elbow turns the mouse about a point behind it, so a sweep across
//!   the screen bulges up or down, the hand's own way, and one straight up or down hardly bends. A stroke of A
//!   degrees takes about `stroke_time`·(A/30)^0.22 seconds (the recorded flicks from 6° to 90° kept to that law),
//!   give or take 15%, and never tops the turn rate. A first stroke lands some 13% short on average (one that must be
//!   exact, and a correction, 3%), scattered by `scatter` of its length along it and 40% of that across.
//! - **Corrections.** When most of a stroke has gone by (70% of its time), the hand sees where it is going and
//!   answers `lag` later: it compares where the strokes in flight will land with where the goal will be, and adds a
//!   stroke for the miss when the miss is more than the look bears. A goal that jumps far is struck at once. Strokes
//!   add up, so a correction that comes before a stroke is over joins it without a stop.
//! - **Pursuit.** The hand's own speed follows the goal's (a little less of it) with a lag of `lag`; a miss within the
//!   look's tolerance is crept off. A target that turns back is followed the old way for a moment, then caught up with.
//!
//! Strokes start at their own times, not at the server's frames, so the view does not depend on the frame rate.

use lb_core::dmath;
use lb_core::math::{angle_diff, normalize_angle};
use lb_core::rng::Pcg32;
use lb_core::{Vec2, Vec3};
use smallvec::SmallVec;

use crate::look::{LookGoal, LookParams, MAX_PITCH, STEP_MAX, Urgency};

/// A stroke of A degrees takes `stroke_time`·(A/30)^TIME_POWER seconds, times a spread of TIME_SPREAD (σ of its
/// logarithm).
const TIME_POWER: f32 = 0.22;
const TIME_SPREAD: f32 = 0.15;
/// The shortest stroke, seconds.
const T_MIN: f32 = 0.05;
/// A stroke's time is warped by an exponent (σ of its logarithm, and its range): above 1 the top speed comes after
/// the middle. The recorded player's strokes peaked at 40% to 60% of their time.
const SKEW_SPREAD: f32 = 0.2;
const SKEW: [f32; 2] = [0.75, 1.35];
/// Where a stroke lands on average, a share of the way: a first one (the recorded player's flicks at enemies landed
/// some 15% short), and a correction.
const REACH: f32 = 0.87;
const REACH_CORRECTION: f32 = 0.97;
/// The spread of where a stroke lands across it, a share of the spread along it.
const ACROSS: f32 = 0.4;
/// How much a first stroke's bend strays from the hand's own, and a correction's bend, degrees.
const BEND_SPREAD: f32 = 6.0;
const CORRECTION_BEND: f32 = 8.0;
/// The share of a stroke's time gone when the hand sees where it is going.
const SEEN: f32 = 0.7;
/// A goal that jumps more than max(BIG, 4 tolerances) in a frame is a new one, struck at once when missed by as much;
/// such strokes come this far apart at least.
const BIG: f32 = 6.0;
const BIG_GAP: f64 = 0.03;
/// A stroke past this share of the way is done: the rest goes at once.
const DONE: f32 = 0.9995;
/// The goal's rate when the look does not give one: smoothed over this long, and dropped when the goal jumps this
/// many degrees off its course in a frame.
const RATE_TAU: f32 = 0.04;
const RATE_JUMP: f32 = 3.0;

#[derive(Clone, Copy, Debug)]
struct Stroke {
    onset: f64,
    /// Seconds from the onset to the end.
    time: f32,
    /// The exponent the stroke's time is warped by.
    skew: f32,
    /// Where the stroke takes the view, degrees (pitch, yaw).
    chord: Vec2,
    /// Radians the stroke's direction turns from its start to its end.
    bend: f32,
    /// The share of the way already moved, and where that put the view.
    done: f32,
    pos: Vec2,
}

impl Stroke {
    fn new(onset: f64, chord: Vec2, time: f32, skew: f32, bend: f32) -> Stroke {
        Stroke {
            onset,
            time,
            skew,
            chord,
            bend,
            done: 0.0,
            pos: Vec2::ZERO,
        }
    }

    /// The share of the way covered at `t`.
    fn progress(&self, t: f64) -> f32 {
        let tau = (t - self.onset) as f32 / self.time;
        if tau <= 0.0 {
            return 0.0;
        }
        if tau >= 1.0 {
            return 1.0;
        }
        let s = min_jerk(dmath::pow(tau, self.skew));
        if s >= DONE { 1.0 } else { s }
    }

    /// Where along its arc the stroke has taken the view at the share `s` of the way.
    fn at(&self, s: f32) -> Vec2 {
        let len = self.chord.length();
        if len < 1e-6 || self.bend.abs() < 1e-3 {
            return self.chord * s;
        }
        let u = self.chord / len;
        let w = Vec2::new(-u.y, u.x);
        let half = 0.5 * self.bend;
        let radius = len / (2.0 * dmath::sin(half));
        let turn = self.bend * s - half;
        u * (radius * (dmath::sin(turn) + dmath::sin(half))) + w * (radius * (dmath::cos(half) - dmath::cos(turn)))
    }
}

/// The share of the way a minimum-jerk stroke has covered at the share `x` of its time.
fn min_jerk(x: f32) -> f32 {
    x * x * x * (10.0 + x * (6.0 * x - 15.0))
}

/// Where in a stroke warped by `skew` its top speed comes, a share of its time: the warped time's x where
/// d/dτ mj(τ^skew) peaks.
fn peak_x(skew: f32) -> f32 {
    (3.0 * skew - 1.0) / (5.0 * skew - 1.0)
}

fn peak_share(skew: f32) -> f32 {
    dmath::pow(peak_x(skew), 1.0 / skew)
}

/// Top speed of a stroke over its mean speed (length over time).
fn peak_ratio(skew: f32) -> f32 {
    let x = peak_x(skew);
    30.0 * skew * x * x * x * (1.0 - x) * (1.0 - x) / peak_share(skew)
}

/// `to - from` with the yaw the short way round.
fn gap(to: Vec2, from: Vec2) -> Vec2 {
    Vec2::new(to.x - from.x, angle_diff(to.y, from.y))
}

/// How far off the goal the look bears before the hand corrects, degrees.
fn tolerance(goal: &LookGoal) -> f32 {
    match goal.urgency {
        Urgency::Calm => 2.0,
        Urgency::Alert => 1.0,
        Urgency::Precise => 0.2,
        Urgency::Engaged if goal.size > 0.0 => (0.45 * goal.size).clamp(0.25, 3.0),
        Urgency::Engaged => 0.3,
    }
}

/// The share of the goal's speed the pursuit keeps up with, and of the goal's motion a stroke allows for.
fn gain(p: &LookParams) -> f32 {
    0.92 + 0.06 * f32::from(p.skill.min(100)) / 100.0
}

fn foresight(p: &LookParams) -> f32 {
    0.3 + 0.3 * f32::from(p.skill.min(100)) / 100.0
}

/// What the hand looks at this frame.
#[derive(Clone, Copy, Debug)]
struct Aim {
    /// The goal at the frame's end.
    goal: Vec2,
    tolerance: f32,
}

#[derive(Clone, Debug, Default)]
pub struct Hand {
    /// The hand's clock, seconds.
    t: f64,
    strokes: SmallVec<[Stroke; 4]>,
    /// The pursuit's speed (pitch, yaw), degrees per second.
    pursuit: Vec2,
    /// The goal's speed, given by the look or estimated from the goal's last frame.
    rate: Vec2,
    last_goal: Option<Vec2>,
    /// The goal jumped and has not been struck at yet.
    jumped: bool,
    /// From when on the hand looks at its miss again.
    next_check: f64,
    last_onset: Option<f64>,
}

impl Hand {
    pub fn update(&mut self, view: &mut Vec3, goal: &LookGoal, dt: f32, p: &LookParams, rng: &mut Pcg32) {
        if dt <= 0.0 {
            return;
        }
        let target = Vec2::new(goal.angles.x.clamp(-MAX_PITCH, MAX_PITCH), goal.angles.y);
        let aim = Aim {
            goal: target,
            tolerance: tolerance(goal),
        };
        let big = BIG.max(4.0 * aim.tolerance);
        self.jumped |= self
            .last_goal
            .is_none_or(|last| (gap(target, last) - self.rate * dt).length() > big);
        self.follow_rate(target, goal.rate, dt, p);
        let v = Vec2::new(view.x, view.y);
        // A new goal is struck at the frame's start, where it was then.
        if self.jumped && self.last_onset.is_none_or(|t| self.t - t >= BIG_GAP) {
            self.jumped = false;
            let start = target - self.rate * dt;
            if self.miss(v, start, p).length() > big {
                self.strike(self.t, v, start, true, goal.urgency, p, rng);
            }
        }
        let v = self.run(v, dt, Some((aim, goal.urgency)), p, rng);
        *view = Vec3::new(v.x.clamp(-MAX_PITCH, MAX_PITCH), normalize_angle(v.y), 0.0);
    }

    /// A frame with no goal: the strokes in flight go on, the pursuit dies down.
    pub fn coast(&mut self, view: &mut Vec3, dt: f32, p: &LookParams) {
        if dt <= 0.0 {
            return;
        }
        self.rate = Vec2::ZERO;
        self.last_goal = None;
        let mut rng = Pcg32::new(0, 0);
        let v = self.run(Vec2::new(view.x, view.y), dt, None, p, &mut rng);
        *view = Vec3::new(v.x.clamp(-MAX_PITCH, MAX_PITCH), normalize_angle(v.y), 0.0);
    }

    fn follow_rate(&mut self, target: Vec2, given: Option<Vec2>, dt: f32, p: &LookParams) {
        let cap = p.turn_speed.max(1.0);
        if let Some(rate) = given {
            self.rate = rate.clamp_length_max(cap);
        } else if let Some(last) = self.last_goal {
            let moved = gap(target, last);
            if (moved - self.rate * dt).length() > RATE_JUMP {
                self.rate = Vec2::ZERO;
            } else {
                let k = 1.0 - dmath::exp(-dt / RATE_TAU);
                self.rate = (self.rate + (moved / dt - self.rate) * k).clamp_length_max(cap);
            }
        }
        self.last_goal = Some(target);
    }

    /// Steps the hand through `dt` in steps of at most a millisecond; with a goal, it corrects as it goes.
    fn run(&mut self, mut v: Vec2, dt: f32, aim: Option<(Aim, Urgency)>, p: &LookParams, rng: &mut Pcg32) -> Vec2 {
        let start = self.t;
        let steps = (dt / STEP_MAX).ceil().max(1.0) as u32;
        let h = dt / steps as f32;
        for k in 1..=steps {
            let from = self.t;
            let to = start + f64::from(dt) * f64::from(k) / f64::from(steps);
            let goal = aim.map(|(a, _)| a.goal - self.rate * (dt * (steps - k) as f32 / steps as f32));
            if let (Some((a, urgency)), Some(g)) = (aim, goal)
                && to >= self.next_check
            {
                let at = self.next_check.max(from);
                if self.miss(v, g, p).length() > a.tolerance {
                    self.strike(at, v, g, false, urgency, p, rng);
                } else {
                    self.next_check = to;
                }
            }
            v = self.step(v, to, h, goal.zip(aim.map(|(a, _)| a.tolerance)), p);
            self.t = to;
        }
        v
    }

    /// One step to `to`: the strokes move on, the pursuit follows the goal's speed, and with no stroke in flight a
    /// small miss is crept off.
    fn step(&mut self, v: Vec2, to: f64, h: f32, goal: Option<(Vec2, f32)>, p: &LookParams) -> Vec2 {
        let mut moved = Vec2::ZERO;
        for s in &mut self.strokes {
            let done = s.progress(to);
            if done > s.done {
                let pos = s.at(done);
                moved += pos - s.pos;
                s.done = done;
                s.pos = pos;
            }
        }
        self.strokes.retain(|s| s.done < 1.0);
        let lag = p.lag.max(0.01);
        let want = self.rate * gain(p);
        self.pursuit += (want - self.pursuit) * (1.0 - dmath::exp(-h / lag));
        let mut speed = self.pursuit;
        if let Some((g, tolerance)) = goal
            && self.strokes.is_empty()
        {
            let off = gap(g, v);
            if off.length() <= 2.0 * tolerance {
                speed += off / (2.0 * lag);
            }
        }
        let d = (moved + speed * h).clamp_length_max(p.turn_speed.max(1.0) * h);
        v + d
    }

    /// Where the goal will be against where the strokes in flight land, when they land.
    fn miss(&self, v: Vec2, goal: Vec2, p: &LookParams) -> Vec2 {
        let mut land = v;
        let mut left = 0.0f32;
        for s in &self.strokes {
            land += s.chord - s.pos;
            left = left.max(s.time - (self.t - s.onset) as f32);
        }
        gap(goal, land) + self.drift(left.max(0.0), p)
    }

    /// The goal's motion over `time` that the pursuit will not have followed by then, as far as the hand foresees it.
    fn drift(&self, time: f32, p: &LookParams) -> Vec2 {
        if time <= 0.0 {
            return Vec2::ZERO;
        }
        let lag = p.lag.max(0.01);
        let want = self.rate * gain(p);
        let followed = want * time + (self.pursuit - want) * (lag * (1.0 - dmath::exp(-time / lag)));
        (self.rate * time - followed) * foresight(p)
    }

    /// Seconds a stroke of `len` degrees takes at `pace` (1 for the hand's usual).
    fn time(&self, len: f32, skew: f32, pace: f32, urgency: Urgency, p: &LookParams) -> f32 {
        let mut time =
            p.stroke_time.max(0.01) * dmath::pow(len.max(1e-3) / 30.0, TIME_POWER) * pace / p.hand.speed.max(0.1);
        if urgency == Urgency::Calm {
            time /= p.calm.clamp(0.05, 1.0).sqrt();
        }
        time.max(T_MIN).max(peak_ratio(skew) * len / p.turn_speed.max(1.0))
    }

    /// Starts a stroke at `at` from the view `v` toward `goal`: a first one at a new goal, or a correction.
    #[allow(clippy::too_many_arguments)]
    fn strike(&mut self, at: f64, v: Vec2, goal: Vec2, first: bool, urgency: Urgency, p: &LookParams, rng: &mut Pcg32) {
        let mut land = v;
        for s in &self.strokes {
            land += s.chord - s.pos;
        }
        let base = gap(goal, land);
        let skew = dmath::exp(SKEW_SPREAD * rng.normal()).clamp(SKEW[0], SKEW[1]);
        let pace = dmath::exp(TIME_SPREAD * rng.normal());
        let mut chord = base;
        let mut time = self.time(chord.length(), skew, pace, urgency, p);
        for _ in 0..2 {
            chord = base + self.drift(time, p);
            time = self.time(chord.length(), skew, pace, urgency, p);
        }
        let len = chord.length();
        if len < 1e-4 {
            return;
        }
        let spread = p.scatter
            * p.hand.speed
            * match urgency {
                Urgency::Calm => 1.5,
                Urgency::Precise => 0.7,
                Urgency::Alert | Urgency::Engaged => 1.0,
            };
        // A look that must be exact is taken with care: its first stroke lands as close as a correction.
        let reach = if first && urgency != Urgency::Precise {
            REACH
        } else {
            REACH_CORRECTION
        } + p.hand.reach;
        let along = len * (reach + spread * rng.normal());
        let across = len * ACROSS * spread * rng.normal();
        let u = chord / len;
        let w = Vec2::new(-u.y, u.x);
        // A positive bend turns a stroke along +yaw toward +pitch first: its middle bulges up the screen.
        let bend = if first {
            -p.hand.bend * u.y + BEND_SPREAD * rng.normal()
        } else {
            CORRECTION_BEND * rng.normal()
        };
        let stroke = Stroke::new(at, u * along + w * across, time, skew, bend.to_radians());
        self.next_check = at + f64::from(SEEN * time + p.lag);
        self.last_onset = Some(at);
        self.strokes.push(stroke);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::look::LookController;
    use lb_config::skill::{AimModel, Presets, SkillParams};
    use lb_core::time::SimTime;
    use lb_styles::HandTraits;

    fn params(k: &SkillParams, skill: u8) -> LookParams {
        LookParams {
            model: AimModel::Hand,
            turn_speed: k.turn_speed,
            turn_accel: k.turn_accel,
            stroke_time: k.hand_time,
            calm: 0.5,
            skill,
            scatter: k.aim_scatter,
            lag: k.hand_lag,
            hand: HandTraits::NEUTRAL,
        }
    }

    /// The view after every frame at `fps` for `secs`, from rest at 0, toward the goal `goal(t)` gives.
    fn run(p: &LookParams, fps: u32, secs: f32, seed: u64, goal: impl Fn(f32) -> LookGoal) -> Vec<(f32, Vec3)> {
        run_from(Vec3::ZERO, p, fps, secs, seed, goal)
    }

    fn run_from(
        mut view: Vec3,
        p: &LookParams,
        fps: u32,
        secs: f32,
        seed: u64,
        goal: impl Fn(f32) -> LookGoal,
    ) -> Vec<(f32, Vec3)> {
        let mut c = LookController::default();
        let mut rng = Pcg32::new(seed, 7);
        let dt = 1.0 / fps as f32;
        (1..=(secs * fps as f32).round() as u32)
            .map(|i| {
                let t = i as f32 * dt;
                c.update(&mut view, &goal(t), dt, p, SimTime(f64::from(t)), false, None, &mut rng);
                (t, view)
            })
            .collect()
    }

    fn flick(p: &LookParams, pitch: f32, yaw: f32, urgency: Urgency, seed: u64) -> Vec<(f32, Vec3)> {
        run(p, 1000, 1.0, seed, |_| {
            LookGoal::new(Vec3::new(pitch, yaw, 0.0), urgency)
        })
    }

    fn within(views: &[(f32, Vec3)], goal: Vec2, deg: f32) -> f32 {
        // Stays within from then on.
        let mut at = f32::INFINITY;
        for (t, v) in views.iter().rev() {
            if gap(goal, Vec2::new(v.x, v.y)).length() < deg {
                at = *t;
            } else {
                break;
            }
        }
        at
    }

    fn speeds(views: &[(f32, Vec3)], fps: u32) -> Vec<f32> {
        views
            .windows(2)
            .map(|w| gap(Vec2::new(w[1].1.x, w[1].1.y), Vec2::new(w[0].1.x, w[0].1.y)).length() * fps as f32)
            .collect()
    }

    fn median(mut v: Vec<f32>) -> f32 {
        v.sort_by(f32::total_cmp);
        v[v.len() / 2]
    }

    #[test]
    fn strokes_are_smooth_bells_within_the_turn_rate() {
        let presets = Presets::default();
        for (k, skill) in [(&presets.easy, 25), (&presets.expert, 100)] {
            let p = params(k, skill);
            for seed in 0..20 {
                let views = flick(&p, 10.0, 90.0, Urgency::Engaged, seed);
                let v = speeds(&views, 1000);
                let top = v.iter().copied().fold(0.0, f32::max);
                assert!(top <= p.turn_speed * 1.01, "{skill}: {top}");
                // A bell tops at about twice its mean speed; a stroke a little slow and short tops lower.
                let mean = 90.55 / (p.stroke_time * dmath::pow(90.55 / 30.0, TIME_POWER));
                assert!(
                    (0.9 * mean..3.2 * mean).contains(&top),
                    "{skill}, seed {seed}: {top} deg/s at the top, {mean} on average"
                );
                // No snap onto the goal: the speed never drops or jumps at once.
                let jolt = v.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
                assert!(
                    jolt < 100.0,
                    "{skill}, seed {seed}: the speed changed by {jolt} deg/s in a millisecond"
                );
                // It starts from rest.
                assert!(v[0] < 0.1 * top, "{skill}: {} at the start", v[0]);
            }
        }
    }

    /// The views every millisecond of a flick from rest at 0 toward `goal`, and where its first stroke lands.
    fn first_stroke(p: &LookParams, goal: Vec2, seed: u64) -> (Vec2, Vec<Vec2>) {
        let mut hand = Hand::default();
        let mut view = Vec3::ZERO;
        let mut rng = Pcg32::new(seed, 7);
        let look = LookGoal::new(Vec3::new(goal.x, goal.y, 0.0), Urgency::Engaged);
        let mut landing = None;
        let views = (0..300)
            .map(|_| {
                hand.update(&mut view, &look, 0.001, p, &mut rng);
                landing = landing.or(hand.strokes.first().map(|s| s.chord));
                Vec2::new(view.x, view.y)
            })
            .collect();
        (landing.unwrap(), views)
    }

    #[test]
    fn flicks_head_for_the_goal_along_an_arc() {
        let k = &Presets::default().hard;
        for bend in [15.0, -15.0] {
            let p = LookParams {
                hand: HandTraits {
                    bend,
                    ..HandTraits::NEUTRAL
                },
                ..params(k, 75)
            };
            for goal in [Vec2::new(10.0, 90.0), Vec2::new(-5.0, -60.0)] {
                let (mut sags, mut offs) = (Vec::new(), Vec::new());
                for seed in 0..40 {
                    let (land, views) = first_stroke(&p, goal, seed);
                    let early = views.iter().find(|q| q.length() > 0.15 * goal.length()).unwrap();
                    let off = (dmath::atan2(early.y, early.x) - dmath::atan2(goal.y, goal.x))
                        .to_degrees()
                        .abs();
                    assert!(off < 25.0, "the flick starts toward the goal, not at 45°: {off}°");
                    offs.push(off);
                    // Halfway along the first stroke, off the line to where it lands: up the screen (pitch less)
                    // for a hand whose sweeps bulge up.
                    let u = land / land.length();
                    let half = *views.iter().find(|q| q.dot(u) > 0.5 * land.length()).unwrap();
                    let sag = half - u * half.dot(u);
                    sags.push(-sag.x * f32::signum(bend) / land.length());
                }
                assert!(median(offs) < 12.0);
                let own = sags.iter().filter(|s| **s > 0.0).count();
                assert!(own >= 36, "{bend}, {goal}: sweeps bulge the hand's way: {sags:?}");
                let mid = median(sags.clone());
                assert!(
                    (0.015..0.05).contains(&mid),
                    "{bend}, {goal}: bulges {:.1}% of the way",
                    mid * 100.0
                );
                assert!(sags.iter().all(|s| s.abs() < 0.08), "{sags:?}");
            }
        }
    }

    #[test]
    fn first_strokes_land_short_and_corrections_finish() {
        let k = &Presets::default().expert;
        let p = params(k, 100);
        let goal = LookGoal::new(Vec3::new(0.0, 90.0, 0.0), Urgency::Engaged);
        let mut reach = Vec::new();
        for seed in 0..200 {
            let mut hand = Hand::default();
            let mut view = Vec3::ZERO;
            let mut rng = Pcg32::new(seed, 7);
            hand.update(&mut view, &goal, 0.001, &p, &mut rng);
            reach.push(hand.strokes[0].chord.y / 90.0);
            for _ in 0..800 {
                hand.update(&mut view, &goal, 0.001, &p, &mut rng);
            }
            assert!(
                angle_diff(90.0, view.y).abs() < 0.3 && view.x.abs() < 0.3,
                "{seed}: {view}"
            );
        }
        let mean = reach.iter().sum::<f32>() / reach.len() as f32;
        let spread = (reach.iter().map(|r| (r - mean).powi(2)).sum::<f32>() / reach.len() as f32).sqrt();
        assert!((0.85..0.89).contains(&mean), "lands at {mean} of the way");
        assert!((0.05..0.09).contains(&spread), "spread {spread}");
        let over = reach.iter().filter(|r| **r > 1.0).count();
        assert!((1..25).contains(&over), "a few overshoot: {over} of 200");
    }

    /// The first stroke at a goal takes as long as the recorded player's flicks at enemies of its length, from 12° to
    /// 93°: their median for a hard bot, their quickest tenth for an expert (the minimum-jerk times that measure as
    /// their strokes did).
    #[test]
    fn first_strokes_take_as_long_as_the_recorded_flicks() {
        let presets = Presets::default();
        let recorded = [
            (12.1, 0.223, 0.145),
            (22.5, 0.242, 0.167),
            (42.5, 0.275, 0.189),
            (92.8, 0.346, 0.216),
        ];
        for (yaw, hard, expert) in recorded {
            for (k, skill, want) in [(&presets.hard, 75, hard), (&presets.expert, 100, expert)] {
                let p = params(k, skill);
                let goal = LookGoal::new(Vec3::new(0.0, yaw, 0.0), Urgency::Engaged);
                let time = median(
                    (0..40)
                        .map(|seed| {
                            let mut hand = Hand::default();
                            let mut rng = Pcg32::new(seed, 7);
                            let mut view = Vec3::ZERO;
                            hand.update(&mut view, &goal, 0.001, &p, &mut rng);
                            hand.strokes[0].time
                        })
                        .collect(),
                );
                assert!(
                    (time / want - 1.0).abs() < 0.08,
                    "skill {skill}, {yaw}°: {time} s against the recorded {want} s"
                );
            }
        }
    }

    #[test]
    fn presets_aim_like_their_players() {
        let presets = Presets::default();
        // (preset, skill, turn, seconds to within 2° to stay: the median's least and most). The recorded player got
        // onto an enemy 25°–50° off in some 0.35 s.
        let cases = [
            (&presets.easy, 25, 90.0, [0.68, 0.90]),
            (&presets.normal, 50, 90.0, [0.53, 0.70]),
            (&presets.hard, 75, 30.0, [0.30, 0.40]),
            (&presets.hard, 75, 90.0, [0.41, 0.54]),
            (&presets.hard, 75, 150.0, [0.47, 0.63]),
            (&presets.expert, 100, 10.0, [0.09, 0.13]),
            (&presets.expert, 100, 90.0, [0.29, 0.38]),
            (&presets.expert, 100, 150.0, [0.33, 0.44]),
        ];
        for (k, skill, yaw, [lo, hi]) in cases {
            let p = params(k, skill);
            let t = median(
                (0..40)
                    .map(|seed| within(&flick(&p, 0.0, yaw, Urgency::Engaged, seed), Vec2::new(0.0, yaw), 2.0))
                    .collect(),
            );
            assert!((lo..=hi).contains(&t), "skill {skill}, {yaw}°: {t}");
        }
    }

    #[test]
    fn tracking_trails_a_target_that_turns_back() {
        let presets = Presets::default();
        // Sweeps between 20° and 38° at 30°/s across the view (an enemy strafing 250 units a second some 500 units
        // off), turning back every 0.6 s; the aim sees it `latency` late and carries it forward, as `Aim` does.
        let path = |t: f32| {
            let leg = (t / 0.6).floor();
            let into = t - leg * 0.6;
            if (leg as i32) % 2 == 0 {
                (20.0 + 30.0 * into, 30.0)
            } else {
                (38.0 - 30.0 * into, -30.0)
            }
        };
        for (k, skill, back_within) in [(&presets.hard, 75, 0.55), (&presets.expert, 100, 0.4)] {
            let p = params(k, skill);
            let latency = k.aim_latency;
            let views = run(&p, 1000, 3.0, 5, |t| {
                let (seen, rate) = path((t - latency).max(0.0));
                LookGoal {
                    angles: Vec3::new(0.0, seen + rate * latency, 0.0),
                    urgency: Urgency::Engaged,
                    rate: Some(Vec2::new(0.0, rate)),
                    size: 1.4,
                }
            });
            for leg in 1..5 {
                let from = leg as f32 * 0.6;
                let errors: Vec<(f32, f32)> = views
                    .iter()
                    .filter(|(t, _)| *t > from && *t <= from + 0.6)
                    .map(|(t, v)| (*t - from, angle_diff(path(*t).0, v.y)))
                    .collect();
                let peak = errors.iter().map(|e| e.1.abs()).fold(0.0, f32::max);
                assert!(
                    (2.0..8.0).contains(&peak),
                    "skill {skill}, leg {leg}: the view trails by {peak}° at most"
                );
                let back = errors.iter().rev().find(|e| e.1.abs() > 1.0).map_or(0.0, |e| e.0);
                assert!(
                    back < back_within,
                    "skill {skill}, leg {leg}: back on it {back} s after the turn"
                );
                let glued = errors.iter().filter(|e| e.1.abs() < 0.02).count();
                assert!(glued * 10 < errors.len(), "skill {skill}: never glued to it ({glued})");
            }
        }
    }

    #[test]
    fn the_hand_does_not_depend_on_the_frame_rate() {
        let presets = Presets::default();
        for (k, skill) in [(&presets.normal, 50), (&presets.expert, 100)] {
            let p = params(k, skill);
            for urgency in [Urgency::Calm, Urgency::Engaged] {
                let at = |fps: u32| {
                    let views = run(&p, fps, 0.4, 3, |_| LookGoal::new(Vec3::new(8.0, 150.0, 0.0), urgency));
                    [0.1f32, 0.2, 0.3, 0.4].map(|t| {
                        views
                            .iter()
                            .find(|(s, _)| (s - t).abs() < 0.5 / fps as f32)
                            .map(|(_, v)| *v)
                            .unwrap()
                    })
                };
                let base = at(1000);
                for fps in [30, 120, 1100] {
                    for (a, b) in at(fps).iter().zip(&base) {
                        assert!(
                            angle_diff(a.y, b.y).abs() < 0.3 && (a.x - b.x).abs() < 0.3,
                            "{skill} {urgency:?} {fps} fps: {a} vs {b}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn precise_looks_settle_within_the_protocols_reach() {
        let presets = Presets::default();
        for (k, skill, by) in [(&presets.normal, 50, 0.95), (&presets.expert, 100, 0.42)] {
            let p = params(k, skill);
            for seed in 0..20 {
                let views = flick(&p, -20.0, 90.0, Urgency::Precise, seed);
                let t = within(&views, Vec2::new(-20.0, 90.0), 0.4);
                assert!(t <= by, "skill {skill}, seed {seed}: within 0.4° from {t} s");
            }
        }
    }

    #[test]
    fn calm_looks_are_slower_and_looser() {
        let p = params(&Presets::default().hard, 75);
        let goal = Vec2::new(0.0, 90.0);
        let time = |urgency| {
            median(
                (0..20)
                    .map(|seed| within(&flick(&p, 0.0, 90.0, urgency, seed), goal, 4.0))
                    .collect(),
            )
        };
        let (calm, alert) = (time(Urgency::Calm), time(Urgency::Alert));
        assert!(calm > alert * 1.2, "calm {calm}, alert {alert}");
        let views = flick(&p, 0.0, 90.0, Urgency::Calm, 1);
        assert!(within(&views, goal, 2.0) < 0.9);
    }

    #[test]
    fn a_frame_without_a_look_lets_the_stroke_finish() {
        let p = params(&Presets::default().expert, 100);
        let mut hand = Hand::default();
        let mut view = Vec3::ZERO;
        let mut rng = Pcg32::new(1, 1);
        hand.update(
            &mut view,
            &LookGoal::new(Vec3::new(0.0, 60.0, 0.0), Urgency::Alert),
            0.01,
            &p,
            &mut rng,
        );
        let landing = hand.strokes[0].chord.y;
        for _ in 0..30 {
            hand.coast(&mut view, 0.01, &p);
        }
        assert!((view.y - landing).abs() < 0.05, "{} vs {landing}", view.y);
    }

    /// Trajectories of the spring the bots aimed with before and of the hand, for a before-and-after page:
    /// `cargo test --release -p lb-motor hand_traces -- --ignored --nocapture` prints them as JSON after `TRACES `.
    #[test]
    #[ignore = "prints trajectories"]
    fn hand_traces() {
        let list = |v: &[f32]| v.iter().map(|x| format!("{x:.2}")).collect::<Vec<_>>().join(",");
        let trace = |preset: &str, scene: &str, model: &str, views: &[(f32, Vec3)], every: usize, target: &[f32]| {
            let mut speed = vec![0.0];
            speed.extend(speeds(views, 1000));
            let pick = |f: &dyn Fn(usize) -> f32| (0..views.len()).step_by(every).map(f).collect::<Vec<_>>();
            format!(
                r#"{{"preset":"{preset}","scene":"{scene}","model":"{model}","every":{},"pitch":[{}],"yaw":[{}],"speed":[{}],"target":[{}]}}"#,
                every as f32 / 1000.0,
                list(&pick(&|i| views[i].1.x)),
                list(&pick(&|i| views[i].1.y)),
                list(&pick(&|i| speed[i])),
                list(&target.iter().step_by(every).copied().collect::<Vec<_>>()),
            )
        };
        let presets = Presets::default();
        let mut traces = Vec::new();
        for (name, k, skill) in [("hard", &presets.hard, 75), ("expert", &presets.expert, 100)] {
            let hand = params(k, skill);
            let spring = LookParams {
                model: AimModel::SpringCombat,
                ..hand
            };
            for (scene, goal, secs) in [
                ("flick 90", Vec2::new(10.0, 90.0), 0.35),
                ("flick 20", Vec2::new(3.0, 20.0), 0.25),
                ("turn 150", Vec2::new(0.0, 150.0), 0.4),
            ] {
                let look = |_: f32| LookGoal::new(Vec3::new(goal.x, goal.y, 0.0), Urgency::Engaged);
                traces.push(trace(name, scene, "spring", &run(&spring, 1000, secs, 0, look), 2, &[]));
                for seed in 1..=4 {
                    traces.push(trace(name, scene, "hand", &run(&hand, 1000, secs, seed, look), 2, &[]));
                }
            }
            // A strafing enemy some 500 units off, as in `tracking_trails_a_target_that_turns_back`.
            let path = |t: f32| {
                let leg = (t / 0.6).floor();
                let into = t - leg * 0.6;
                if (leg as i32) % 2 == 0 {
                    (20.0 + 30.0 * into, 30.0)
                } else {
                    (38.0 - 30.0 * into, -30.0)
                }
            };
            let latency = k.aim_latency;
            let seen = move |t: f32| {
                let (at, rate) = path((t - latency).max(0.0));
                (at + rate * latency, rate)
            };
            let start = Vec3::new(0.0, 20.0, 0.0);
            let target: Vec<f32> = (1..=2400).map(|i| path(i as f32 / 1000.0).0).collect();
            let views = run_from(start, &spring, 1000, 2.4, 0, |t| {
                LookGoal::new(Vec3::new(0.0, seen(t).0, 0.0), Urgency::Engaged)
            });
            traces.push(trace(name, "strafe", "spring", &views, 5, &target));
            let views = run_from(start, &hand, 1000, 2.4, 1, |t| LookGoal {
                angles: Vec3::new(0.0, seen(t).0, 0.0),
                urgency: Urgency::Engaged,
                rate: Some(Vec2::new(0.0, seen(t).1)),
                size: 1.4,
            });
            traces.push(trace(name, "strafe", "hand", &views, 5, &target));
        }
        println!("TRACES [{}]", traces.join(","));
    }
}
