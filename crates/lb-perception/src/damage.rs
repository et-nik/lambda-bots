//! The damage compass: the four HUD indicators the client lights for a `Damage` message
//! (`CHudHealth::CalcDamageDirection`). The message carries the exact source; a player only sees the compass, so
//! that is all the bot gets.

use lb_core::Vec3;
use lb_core::math::{normalize_angle, view_angle_vectors};
use lb_core::rng::Pcg32;
use lb_core::time::SimTime;
use lb_knowledge::DamageStimulus;

const LIGHT_THRESHOLD: f32 = 0.3;
const CLOSE: f32 = 50.0;
const BEARING_SIGMA: f32 = 15.0;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Compass {
    pub front: f32,
    pub rear: f32,
    pub right: f32,
    pub left: f32,
}

/// Indicators for damage from `from` to a player at `origin` looking along `view`.
pub fn compass(from: Vec3, origin: Vec3, view: Vec3) -> Compass {
    if from == Vec3::ZERO {
        return Compass::default();
    }
    let d = from - origin;
    if d.length() <= CLOSE {
        return Compass {
            front: 1.0,
            rear: 1.0,
            right: 1.0,
            left: 1.0,
        };
    }
    let d = d.normalize();
    let (forward, right, _) = view_angle_vectors(view);
    let ahead = d.dot(forward);
    let side = d.dot(right);
    let lit = |v: f32| if v > LIGHT_THRESHOLD { v } else { 0.0 };
    Compass {
        front: lit(ahead),
        rear: lit(-ahead),
        right: lit(side),
        left: lit(-side),
    }
}

/// What a `Damage` message tells the bot; `None` for messages that carry no damage.
#[allow(clippy::too_many_arguments)]
pub fn stimulus(
    t: SimTime,
    health: i32,
    armor: i32,
    bits: i32,
    from: Vec3,
    origin: Vec3,
    view: Vec3,
    rng: &mut Pcg32,
) -> Option<DamageStimulus> {
    if health <= 0 && armor <= 0 {
        return None;
    }
    let c = compass(from, origin, view);
    let x = c.front - c.rear;
    let y = c.right - c.left;
    let all = c.front > 0.0 && c.rear > 0.0 && c.right > 0.0 && c.left > 0.0;
    let bearing = if all || (x == 0.0 && y == 0.0) {
        None
    } else {
        let local = y.atan2(x).to_degrees();
        Some(normalize_angle(view.y - local + rng.normal() * BEARING_SIGMA))
    };
    Some(DamageStimulus {
        t,
        bearing,
        amount: health.max(0) + armor.max(0),
        bits,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compass_matches_the_hud() {
        let view = Vec3::new(0.0, 0.0, 0.0);
        let c = compass(Vec3::new(0.0, 500.0, 0.0), Vec3::ZERO, view);
        assert_eq!(
            (c.left, c.right, c.front, c.rear),
            (1.0, 0.0, 0.0, 0.0),
            "+Y is on the left at yaw 0"
        );
        let c = compass(Vec3::new(-500.0, -500.0, 0.0), Vec3::ZERO, view);
        assert!(c.rear > 0.7 && c.right > 0.7 && c.front == 0.0 && c.left == 0.0);
        let c = compass(Vec3::new(10.0, 0.0, 0.0), Vec3::ZERO, view);
        assert_eq!(
            c.front + c.rear + c.left + c.right,
            4.0,
            "close damage lights every side"
        );
        let c = compass(Vec3::new(0.0, 0.0, 500.0), Vec3::ZERO, view);
        assert_eq!(c, Compass::default(), "straight above lights nothing");
    }

    #[test]
    fn bearing_points_at_the_source_within_the_noise() {
        let mut rng = Pcg32::new(1, 1);
        let view = Vec3::new(0.0, 90.0, 0.0);
        let mut worst: f32 = 0.0;
        for _ in 0..200 {
            let d = stimulus(
                SimTime(1.0),
                10,
                0,
                0,
                Vec3::new(-800.0, 0.0, 0.0),
                Vec3::ZERO,
                view,
                &mut rng,
            )
            .unwrap();
            worst = worst.max(lb_core::math::angle_diff(d.bearing.unwrap(), 180.0).abs());
        }
        assert!(worst < 70.0, "{worst}");
        assert!(stimulus(SimTime(1.0), 0, 0, 0, Vec3::X * 800.0, Vec3::ZERO, view, &mut rng).is_none());
        let close = stimulus(SimTime(1.0), 5, 0, 0, Vec3::X * 10.0, Vec3::ZERO, view, &mut rng).unwrap();
        assert_eq!(close.bearing, None);
    }
}
