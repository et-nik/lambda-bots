//! The look controller: turns the view toward the point the arbiter granted, the way yapb aims (`vision.cpp`).
//! Spring models use a damped spring per axis, stiffer in a fight for skilled bots and capped by the bot's turn
//! rate. The newbie model is yapb's mouse model for noob bots. Integration runs in fixed steps (120 Hz, newbie at
//! 90 Hz), so the result does not depend on the server frame rate.

use lb_config::skill::AimModel;
use lb_core::math::{angle_diff, normalize_angle};
use lb_core::rng::Pcg32;
use lb_core::time::SimTime;
use lb_core::{Vec2, Vec3};

const STEP: f32 = 1.0 / 120.0;
const NEWBIE_STEP: f32 = 1.0 / 90.0;
const SNAP_DEGREES: f32 = 1.0;
const MAX_PITCH: f32 = 89.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LookParams {
    pub model: AimModel,
    /// Turn rate cap, degrees per second.
    pub turn_speed: f32,
    /// 0..100.
    pub skill: u8,
}

/// What the look is asked to do this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LookGoal {
    /// View angles to reach (pitch positive down).
    pub angles: Vec3,
    /// Aiming at an enemy: skilled bots stiffen the spring.
    pub engaged: bool,
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
        let step = if p.model == AimModel::Newbie { NEWBIE_STEP } else { STEP };
        self.pending += dt.clamp(0.0, 0.25);
        while self.pending >= step {
            self.pending -= step;
            if p.model == AimModel::Newbie {
                self.newbie_step(view, goal, step, p, now, moving, fired_at, rng);
            } else {
                self.spring_step(view, goal, step, p);
            }
        }
        view.x = view.x.clamp(-MAX_PITCH, MAX_PITCH);
        view.y = normalize_angle(view.y);
        view.z = 0.0;
    }

    fn spring_step(&mut self, view: &mut Vec3, goal: &LookGoal, h: f32, p: &LookParams) {
        let (mut k, mut c, mut accel) = (200.0, 25.0, 3000.0);
        if goal.engaged && p.model == AimModel::SpringCombat {
            k += 100.0;
            c -= 5.0;
            if p.skill >= 100 {
                accel += 300.0;
            }
        }
        let cap = p.turn_speed.max(1.0);
        let yaw_err = angle_diff(goal.angles.y, view.y);
        if yaw_err.abs() < SNAP_DEGREES {
            self.yaw_vel = 0.0;
            view.y = goal.angles.y;
        } else {
            let a = (k * yaw_err - c * self.yaw_vel).clamp(-accel, accel);
            self.yaw_vel = (self.yaw_vel + h * a).clamp(-cap, cap);
            view.y += h * self.yaw_vel;
        }
        let pitch_err = goal.angles.x.clamp(-MAX_PITCH, MAX_PITCH) - view.x;
        let a = (2.0 * k * pitch_err - c * self.pitch_vel).clamp(-accel, accel);
        self.pitch_vel = (self.pitch_vel + h * a).clamp(-cap, cap);
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
        let stiffness = if goal.engaged {
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

    fn settle(model: AimModel, skill: u8, turn: f32, fps: f32, secs: f32) -> (Vec3, f32) {
        let mut c = LookController::default();
        let mut view = Vec3::ZERO;
        let goal = LookGoal {
            angles: Vec3::new(10.0, 90.0, 0.0),
            engaged: true,
        };
        let p = LookParams {
            model,
            turn_speed: turn,
            skill,
        };
        let mut rng = Pcg32::new(1, 1);
        let dt = 1.0 / fps;
        let mut t = 0.0;
        let mut reached = f32::INFINITY;
        while t < secs {
            c.update(&mut view, &goal, dt, &p, SimTime(f64::from(t)), false, None, &mut rng);
            t += dt;
            if reached.is_infinite() && angle_diff(90.0, view.y).abs() < 2.0 && (view.x - 10.0).abs() < 2.0 {
                reached = t;
            }
        }
        (view, reached)
    }

    #[test]
    fn springs_settle_and_respect_the_turn_cap() {
        let (view, t) = settle(AimModel::Spring, 50, 450.0, 1000.0, 2.0);
        assert!((view.y - 90.0).abs() < 0.5 && (view.x - 10.0).abs() < 0.5, "{view}");
        assert!(
            t > 90.0 / 450.0,
            "a 90 degree turn at 450 deg/s takes at least 0.2 s: {t}"
        );
        assert!(t < 0.6, "{t}");
        let (_, slow) = settle(AimModel::Spring, 0, 180.0, 1000.0, 3.0);
        assert!(slow >= 0.5, "{slow}");
    }

    #[test]
    fn the_result_does_not_depend_on_the_frame_rate() {
        let (a, _) = settle(AimModel::SpringCombat, 75, 650.0, 1000.0, 0.15);
        let (b, _) = settle(AimModel::SpringCombat, 75, 650.0, 120.0, 0.15);
        assert!((a - b).length() < 1.5, "{a} vs {b}");
    }

    #[test]
    fn the_newbie_mouse_gets_there_too() {
        let (view, t) = settle(AimModel::Newbie, 0, 180.0, 500.0, 4.0);
        assert!(angle_diff(90.0, view.y).abs() < 3.0, "{view}");
        assert!(t.is_finite());
    }
}
