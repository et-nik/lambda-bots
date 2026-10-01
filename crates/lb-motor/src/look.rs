//! The look controller: turns the view toward the point the arbiter granted, the way yapb aims (`vision.cpp`), as
//! hard as the bot's skill allows. Spring models use a damped spring per axis whose stiffness follows the bot's turn
//! acceleration: the view is flicked at full acceleration at an enemy or at whatever calls for attention, turned more
//! gently to look along the way and around, and capped by the bot's turn rate. Springs are integrated over each
//! frame's own time in steps of at most a millisecond, so the view moves on every frame and the result does not
//! depend on the server frame rate. The newbie model is yapb's mouse model for noob bots; its recurrence is written
//! per step, so it keeps fixed 90 Hz steps.

use lb_config::skill::AimModel;
use lb_core::math::{angle_diff, normalize_angle};
use lb_core::rng::Pcg32;
use lb_core::time::SimTime;
use lb_core::{Vec2, Vec3};

const STEP_MAX: f32 = 0.001;
const NEWBIE_STEP: f32 = 1.0 / 90.0;
const SNAP_DEGREES: f32 = 1.0;
const MAX_PITCH: f32 = 89.0;
/// Spring stiffness per degree/s² of turn acceleration: yapb's 200 at 3000.
const STIFFNESS_PER_ACCEL: f32 = 1.0 / 15.0;
/// Damping ratio of the spring (yapb's damping 25 at stiffness 200), and of the combat spring aiming at an enemy.
const DAMPING: f32 = 0.884;
const COMBAT_DAMPING: f32 = 0.7;
/// The combat spring is this much stiffer (yapb's 300 over 200).
const COMBAT_STIFFNESS: f32 = 1.5;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LookParams {
    pub model: AimModel,
    /// Turn rate cap, degrees per second.
    pub turn_speed: f32,
    /// Turn acceleration cap, degrees per second squared.
    pub turn_accel: f32,
    /// Share of the turn acceleration a calm look uses (along the way, around), 0..1.
    pub calm: f32,
    /// 0..100.
    pub skill: u8,
}

impl LookParams {
    /// yapb's look at 900 degrees per second, calm or not: bots sent somewhere by hand and the obstacle courses.
    pub const NAV: LookParams = LookParams {
        model: AimModel::Spring,
        turn_speed: 900.0,
        turn_accel: 3000.0,
        calm: 1.0,
        skill: 100,
    };
}

/// How urgently the view is turned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Urgency {
    /// Looking along the way and around.
    Calm,
    /// Something calls for the view: a glimpse, damage, a shot heard, a weapon or a traversal that needs it.
    Alert,
    /// Aiming at an enemy: a skilled bot's combat spring is stiffer.
    Engaged,
}

/// What the look is asked to do this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LookGoal {
    /// View angles to reach (pitch positive down).
    pub angles: Vec3,
    pub urgency: Urgency,
}

/// The spring of one update: stiffness, damping and the acceleration and rate caps.
struct Spring {
    k: f32,
    c: f32,
    accel: f32,
    cap: f32,
}

impl Spring {
    fn new(p: &LookParams, urgency: Urgency) -> Spring {
        let mut accel = p.turn_accel.max(1.0);
        if urgency == Urgency::Calm {
            accel *= p.calm.clamp(0.05, 1.0);
        }
        let mut k = accel * STIFFNESS_PER_ACCEL;
        let mut damping = DAMPING;
        if urgency == Urgency::Engaged && p.model == AimModel::SpringCombat {
            k *= COMBAT_STIFFNESS;
            damping = COMBAT_DAMPING;
        }
        Spring {
            k,
            c: 2.0 * damping * k.sqrt(),
            accel,
            cap: p.turn_speed.max(1.0),
        }
    }
}

#[derive(Clone, Debug, Default)]
struct Newbie {
    speed: Vec2,
    randomized: Vec2,
    deviation: Vec2,
    next_randomize: f64,
    last_target: f64,
}

#[derive(Clone, Debug, Default)]
pub struct LookController {
    yaw_vel: f32,
    pitch_vel: f32,
    /// Time the newbie model has not stepped through yet.
    pending: f32,
    newbie: Newbie,
}

impl LookController {
    pub fn reset(&mut self) {
        *self = LookController::default();
    }

    /// Moves `view` toward `goal` over `dt` seconds. `moving` and `fired_at` feed the newbie model.
    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        view: &mut Vec3,
        goal: &LookGoal,
        dt: f32,
        p: &LookParams,
        now: SimTime,
        moving: bool,
        fired_at: Option<SimTime>,
        rng: &mut Pcg32,
    ) {
        let dt = dt.clamp(0.0, 0.25);
        if p.model == AimModel::Newbie {
            self.pending += dt;
            while self.pending >= NEWBIE_STEP {
                self.pending -= NEWBIE_STEP;
                self.newbie_step(view, goal, NEWBIE_STEP, p, now, moving, fired_at, rng);
            }
        } else if dt > 0.0 {
            let spring = Spring::new(p, goal.urgency);
            let steps = (dt / STEP_MAX).ceil().max(1.0);
            let h = dt / steps;
            for _ in 0..steps as u32 {
                self.spring_step(view, goal, h, &spring);
            }
        }
        view.x = view.x.clamp(-MAX_PITCH, MAX_PITCH);
        view.y = normalize_angle(view.y);
        view.z = 0.0;
    }

    fn spring_step(&mut self, view: &mut Vec3, goal: &LookGoal, h: f32, s: &Spring) {
        let yaw_err = angle_diff(goal.angles.y, view.y);
        if yaw_err.abs() < SNAP_DEGREES {
            self.yaw_vel = 0.0;
            view.y = goal.angles.y;
        } else {
            let a = (s.k * yaw_err - s.c * self.yaw_vel).clamp(-s.accel, s.accel);
            self.yaw_vel = (self.yaw_vel + h * a).clamp(-s.cap, s.cap);
            view.y += h * self.yaw_vel;
        }
        let pitch_err = goal.angles.x.clamp(-MAX_PITCH, MAX_PITCH) - view.x;
        let a = (2.0 * s.k * pitch_err - s.c * self.pitch_vel).clamp(-s.accel, s.accel);
        self.pitch_vel = (self.pitch_vel + h * a).clamp(-s.cap, s.cap);
        view.x += h * self.pitch_vel;
    }

    /// yapb `updateLookAnglesNewbie`: a lagging, slightly wandering mouse.
    #[allow(clippy::too_many_arguments)]
    fn newbie_step(
        &mut self,
        view: &mut Vec3,
        goal: &LookGoal,
        h: f32,
        p: &LookParams,
        now: SimTime,
        moving: bool,
        fired_at: Option<SimTime>,
        rng: &mut Pcg32,
    ) {
        const SPRING: f32 = 13.0;
        const DAMPER: f32 = 0.22;
        const NO_TARGET_RATIO: f32 = 0.3;
        const OFFSET_DELAY: f64 = 1.2;
        let n = &mut self.newbie;
        let offset = (f32::from(p.skill) / 25.0).clamp(1.0, 4.0) * 25.0;
        let influence = Vec2::new(0.25, 0.17) * (100.0 - offset) / 100.0;
        let randomization = Vec2::new(2.0, 0.18) * (100.0 - offset) / 100.0;
        let ideal = Vec2::new(goal.angles.x, goal.angles.y);
        let now = now.secs();
        let stiffness = if goal.urgency == Urgency::Engaged {
            n.last_target = now;
            n.randomized = ideal;
            SPRING * (0.2 + offset / 125.0)
        } else {
            let deviation = n.deviation.length();
            if now >= n.next_randomize && ((moving && deviation < 5.0) || deviation < 1.0) {
                let r = if moving { randomization } else { randomization * 0.2 };
                n.randomized = ideal + Vec2::new(rng.range_f32(-r.x * 0.5, r.x * 1.5), rng.range_f32(-r.y, r.y));
                n.next_randomize = now + f64::from(rng.range_f32(0.4, OFFSET_DELAY as f32));
            }
            let mut mult = NO_TARGET_RATIO;
            if now - (n.last_target + OFFSET_DELAY) < f64::from(NO_TARGET_RATIO) * 10.0 {
                let since_fired = fired_at.map_or(f32::INFINITY, |t| (now - t.secs()) as f32);
                mult = 1.0 - since_fired * 0.1;
                if mult < 0.0 {
                    mult = 0.5;
                }
            }
            mult *= deviation * 0.1 * 0.5;
            SPRING * mult.max(0.35)
        };
        n.deviation = Vec2::new(
            (n.randomized.x - view.x).clamp(-180.0, 180.0),
            angle_diff(n.randomized.y, view.y),
        );
        n.speed.x = stiffness * n.deviation.x - DAMPER * n.speed.x;
        n.speed.y = stiffness * n.deviation.y - DAMPER * n.speed.y;
        n.speed.x += (n.speed.y * influence.y).clamp(-50.0, 50.0);
        n.speed.y += (n.speed.x * influence.x).clamp(-200.0, 200.0);
        let cap = p.turn_speed.max(1.0);
        view.x += h * n.speed.x.clamp(-cap, cap);
        view.y += h * n.speed.y.clamp(-cap, cap);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Turns from rest toward 90° yaw and 10° pitch at `fps` for `secs`: the view after every frame, with its time.
    fn turn(p: &LookParams, urgency: Urgency, yaw: f32, fps: u32, secs: f32) -> Vec<(f32, Vec3)> {
        let mut c = LookController::default();
        let mut view = Vec3::ZERO;
        let goal = LookGoal {
            angles: Vec3::new(10.0, yaw, 0.0),
            urgency,
        };
        let mut rng = Pcg32::new(1, 1);
        let dt = 1.0 / fps as f32;
        let frames = (secs * fps as f32).round() as u32;
        (1..=frames)
            .map(|i| {
                let t = i as f32 / fps as f32;
                c.update(&mut view, &goal, dt, p, SimTime(f64::from(t)), false, None, &mut rng);
                (t, view)
            })
            .collect()
    }

    /// When the view first comes within 2° of the goal.
    fn reached(views: &[(f32, Vec3)], yaw: f32) -> f32 {
        views
            .iter()
            .find(|(_, v)| angle_diff(yaw, v.y).abs() < 2.0 && (v.x - 10.0).abs() < 2.0)
            .map_or(f32::INFINITY, |(t, _)| *t)
    }

    fn preset(model: AimModel, turn_speed: f32, turn_accel: f32) -> LookParams {
        LookParams {
            model,
            turn_speed,
            turn_accel,
            calm: 1.0,
            skill: 50,
        }
    }

    #[test]
    fn springs_settle_and_respect_the_turn_cap() {
        let views = turn(&LookParams::NAV, Urgency::Alert, 90.0, 1000, 2.0);
        let (_, last) = views.last().unwrap();
        assert!((last.y - 90.0).abs() < 0.5 && (last.x - 10.0).abs() < 0.5, "{last}");
        let t = reached(&views, 90.0);
        assert!(
            (0.36..=0.42).contains(&t),
            "yapb's look turns 90 degrees in about 0.4 s: {t}"
        );
        let slow = preset(AimModel::Spring, 180.0, 3000.0);
        let t = reached(&turn(&slow, Urgency::Alert, 90.0, 1000, 2.0), 90.0);
        assert!(
            t > 90.0 / 180.0,
            "a 90 degree turn at 180 deg/s takes at least 0.5 s: {t}"
        );
        let newbie = LookParams {
            model: AimModel::Newbie,
            skill: 0,
            ..slow
        };
        assert!(reached(&turn(&newbie, Urgency::Engaged, 90.0, 1000, 3.0), 90.0) >= 0.5);
    }

    #[test]
    fn the_result_does_not_depend_on_the_frame_rate() {
        let p = preset(AimModel::SpringCombat, 2500.0, 24000.0);
        for urgency in [Urgency::Calm, Urgency::Engaged] {
            let at = |fps: u32| {
                let views = turn(&p, urgency, 150.0, fps, 0.3);
                [0.1f32, 0.2, 0.3].map(|t| {
                    views
                        .iter()
                        .find(|(s, _)| (s - t).abs() < 1e-4)
                        .map(|(_, v)| *v)
                        .unwrap()
                })
            };
            let base = at(1000);
            for fps in [30, 120, 1100] {
                for (a, b) in at(fps).iter().zip(&base) {
                    assert!(
                        angle_diff(a.y, b.y).abs() < 0.2 && (a.x - b.x).abs() < 0.2,
                        "{fps} fps: {a} vs {b}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_view_moves_on_every_frame() {
        let views = turn(&LookParams::NAV, Urgency::Alert, 90.0, 1100, 0.2);
        for pair in views.windows(2) {
            assert!(pair[1].1.y > pair[0].1.y, "{:?}", pair);
        }
    }

    #[test]
    fn flicks_stop_on_the_goal_within_the_rate_cap() {
        for p in [
            preset(AimModel::Spring, 1000.0, 9000.0),
            preset(AimModel::SpringCombat, 1600.0, 15000.0),
            preset(AimModel::SpringCombat, 2500.0, 24000.0),
        ] {
            for yaw in [30.0, 90.0, 180.0 - 1e-3] {
                let views = turn(&p, Urgency::Engaged, yaw, 1100, 1.0);
                let overshoot = views.iter().map(|(_, v)| angle_diff(v.y, yaw)).fold(f32::MIN, f32::max);
                assert!(overshoot <= 1.0, "{p:?}, {yaw}: {overshoot}");
                // The last degree is snapped onto the goal.
                let speed = views
                    .windows(2)
                    .filter(|w| angle_diff(w[1].1.y, yaw).abs() > 1e-3)
                    .map(|w| angle_diff(w[1].1.y, w[0].1.y).abs() * 1100.0)
                    .fold(0.0, f32::max);
                assert!(speed <= p.turn_speed * 1.01, "{p:?}, {yaw}: {speed}");
            }
        }
    }

    #[test]
    fn calm_looks_turn_slower_and_combat_springs_faster() {
        let p = LookParams {
            calm: 0.5,
            ..preset(AimModel::SpringCombat, 1600.0, 15000.0)
        };
        let [calm, alert, engaged] =
            [Urgency::Calm, Urgency::Alert, Urgency::Engaged].map(|u| reached(&turn(&p, u, 90.0, 1000, 1.0), 90.0));
        assert!(calm > alert && alert > engaged, "{calm} {alert} {engaged}");
    }

    #[test]
    fn presets_turn_as_fast_as_their_players() {
        let presets = lb_config::skill::Presets::default();
        // (preset, how the look turns, turn, seconds to within 2°: least and most)
        let cases = [
            (&presets.easy, Urgency::Alert, 90.0, [0.28, 0.34]),
            (&presets.easy, Urgency::Alert, 180.0, [0.42, 0.50]),
            (&presets.normal, Urgency::Alert, 90.0, [0.20, 0.26]),
            (&presets.normal, Urgency::Alert, 180.0, [0.27, 0.34]),
            (&presets.normal, Urgency::Calm, 90.0, [0.28, 0.36]),
            (&presets.hard, Urgency::Engaged, 90.0, [0.11, 0.15]),
            (&presets.hard, Urgency::Engaged, 180.0, [0.15, 0.20]),
            (&presets.expert, Urgency::Engaged, 10.0, [0.0, 0.06]),
            (&presets.expert, Urgency::Engaged, 90.0, [0.08, 0.12]),
            (&presets.expert, Urgency::Engaged, 180.0, [0.11, 0.15]),
        ];
        for (k, urgency, yaw, [lo, hi]) in cases {
            let p = LookParams {
                model: k.aim_model,
                turn_speed: k.turn_speed,
                turn_accel: k.turn_accel,
                calm: 0.5,
                skill: 50,
            };
            let yaw = if yaw == 180.0 { 180.0 - 1e-3 } else { yaw };
            let t = reached(&turn(&p, urgency, yaw, 1000, 1.0), yaw);
            assert!((lo..=hi).contains(&t), "{p:?} {urgency:?} {yaw}°: {t}");
        }
    }

    #[test]
    fn the_newbie_mouse_gets_there_too() {
        let p = LookParams {
            model: AimModel::Newbie,
            skill: 0,
            ..preset(AimModel::Newbie, 180.0, 3000.0)
        };
        let views = turn(&p, Urgency::Engaged, 90.0, 500, 4.0);
        let (_, last) = views.last().unwrap();
        assert!(angle_diff(90.0, last.y).abs() < 3.0, "{last}");
        assert!(reached(&views, 90.0).is_finite());
    }
}
