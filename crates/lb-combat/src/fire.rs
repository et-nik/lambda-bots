//! Fire control: pull the trigger when the view is on the target closely enough (yapb `focusEnemy`), the target is
//! within the weapon's reach, and not a rocket at point blank; semi-automatic weapons are clicked at a human
//! cadence.

use lb_core::Vec3;
use lb_core::math::view_angle_vectors;
use lb_core::rng::Pcg32;
use lb_game::mechanics::{WeaponClass, spec};
use lb_game::weapons::WeaponId;

#[derive(Clone, Copy, Debug)]
pub struct Shot {
    pub eye: Vec3,
    pub view: Vec3,
    pub aim: Vec3,
    pub distance: f32,
    pub enemy_faces_me: bool,
    pub weapon: WeaponId,
}

/// The view is on the target closely enough to fire.
pub fn on_target(s: &Shot) -> bool {
    let w = spec(s.weapon);
    let (forward, _, _) = view_angle_vectors(s.view);
    let dot = forward.dot((s.aim - s.eye).normalize_or_zero());
    if w.class == WeaponClass::Melee {
        return s.distance < 64.0 && dot > 0.8;
    }
    if s.distance > w.reach || (w.class == WeaponClass::Launcher && s.distance < 300.0) {
        return false;
    }
    if s.distance < 90.0 {
        return true;
    }
    if s.distance < 128.0 {
        return dot > 0.8;
    }
    if dot < 0.9 {
        return false;
    }
    s.enemy_faces_me || dot > 0.99
}

/// Seconds between clicks of a semi-automatic weapon: the weapon's cycle, or a human pause from `pause` when
/// longer.
pub fn click_interval(weapon: WeaponId, pause: [f32; 2], rng: &mut Pcg32) -> f32 {
    spec(weapon).cycle.max(0.1 + rng.range_f32(pause[0], pause[1]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shot(aim: Vec3, faces: bool, weapon: WeaponId) -> Shot {
        Shot {
            eye: Vec3::ZERO,
            view: Vec3::ZERO,
            aim,
            distance: aim.length(),
            enemy_faces_me: faces,
            weapon,
        }
    }

    #[test]
    fn cones_follow_yapb() {
        // 10 degrees off at 500 units: only an enemy facing the bot is worth the shot.
        let off = Vec3::new(500.0, 500.0 * 10f32.to_radians().tan(), 0.0);
        assert!(!on_target(&shot(off, false, WeaponId::Glock)));
        assert!(on_target(&shot(off, true, WeaponId::Glock)));
        let dead_on = Vec3::new(500.0, 2.0, 0.0);
        assert!(on_target(&shot(dead_on, false, WeaponId::Glock)));
        assert!(
            on_target(&shot(Vec3::new(50.0, 40.0, 0.0), false, WeaponId::Mp5)),
            "point blank"
        );
        assert!(
            !on_target(&shot(Vec3::new(200.0, 0.0, 0.0), false, WeaponId::Rpg)),
            "no rockets under 300"
        );
        assert!(!on_target(&shot(Vec3::new(200.0, 0.0, 0.0), false, WeaponId::Crowbar)));
        assert!(on_target(&shot(Vec3::new(40.0, 0.0, 0.0), false, WeaponId::Crowbar)));
    }
}
