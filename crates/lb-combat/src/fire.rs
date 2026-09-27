//! Fire control: pull the trigger when the view is on the target closely enough (yapb `focusEnemy`), the target is
//! within the weapon's reach, and no blast would reach the shooter; semi-automatic weapons are clicked at a human
//! cadence. The secondary attack is used where it pays: the glock's rapid fire and the hornet gun's darts up close,
//! both shotgun barrels at a few steps (half the time, as yapb), the crossbow's scope far away and the 357's very far.

use lb_core::Vec3;
use lb_core::math::view_angle_vectors;
use lb_core::rng::Pcg32;
use lb_game::mechanics::{AltFire, Attack, Trigger, WeaponClass, spec};
use lb_game::weapons::WeaponId;

use crate::policy::{Armed, ROCKET_MIN, XBOW_ZOOM_FROM};

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
    if s.distance > w.reach || (s.weapon == WeaponId::Rpg && s.distance < ROCKET_MIN) {
        return false;
    }
    match s.weapon {
        // One rocket per reload, aimed at the feet: worth a careful aim.
        WeaponId::Rpg => return dot > 0.995,
        WeaponId::Crossbow if s.distance >= XBOW_ZOOM_FROM => return dot > 0.998,
        // The beam sweeps onto the target.
        WeaponId::Egon => return dot > 0.97,
        _ => {}
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

/// Which button fires `a` at a target this far, how it is worked, and the least time between presses.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mode {
    pub attack: Attack,
    pub trigger: Trigger,
    pub cycle: f32,
}

/// Glock rapid fire only this close: its cone is ten times wider.
const RAPID_UNDER: f32 = 150.0;
/// Hornet darts this close, with at least `DARTS_HORNETS` hornets.
const DARTS_UNDER: f32 = 250.0;
const DARTS_HORNETS: i32 = 4;
/// Both barrels between these distances.
const DOUBLE_BAND: [f32; 2] = [32.0, 300.0];

/// How to fire `a` at `distance`; `double` allows both shotgun barrels for this shot.
pub fn mode(a: &Armed, distance: f32, double: bool) -> Mode {
    let s = spec(a.id);
    let primary = Mode {
        attack: Attack::Primary,
        trigger: s.trigger,
        cycle: s.cycle,
    };
    match s.alt {
        AltFire::Rapid { cycle, .. } if distance < RAPID_UNDER => Mode {
            attack: Attack::Secondary,
            trigger: Trigger::Hold,
            cycle,
        },
        AltFire::Darts { cycle } if distance <= DARTS_UNDER && a.reserve.is_some_and(|r| r >= DARTS_HORNETS) => Mode {
            attack: Attack::Secondary,
            trigger: Trigger::Hold,
            cycle,
        },
        AltFire::Double { cycle, .. }
            if double && (DOUBLE_BAND[0]..=DOUBLE_BAND[1]).contains(&distance) && a.clip.is_some_and(|c| c >= 2) =>
        {
            Mode {
                attack: Attack::Secondary,
                trigger: Trigger::Tap,
                cycle,
            }
        }
        _ => primary,
    }
}

/// Whether the shotgun's next shot uses both barrels: half the time, as yapb.
pub fn roll_double(rng: &mut Pcg32) -> bool {
    rng.next_f32() < 0.5
}

/// The 357's scope only very far away: it narrows the view and does not steady the shot.
const PYTHON_ZOOM: [f32; 2] = [700.0, 1000.0];
/// The crossbow unzooms under this, zooms from `XBOW_ZOOM_FROM`.
const XBOW_UNZOOM: f32 = 400.0;

/// Whether the view should be zoomed with `w` against a target `distance` away (`None`: no target); `zoomed` is the
/// state now, kept between the two thresholds.
pub fn zoom_wanted(w: WeaponId, distance: Option<f32>, zoomed: bool) -> bool {
    let Some(d) = distance else { return false };
    let [off, on] = match w {
        WeaponId::Crossbow => [XBOW_UNZOOM, XBOW_ZOOM_FROM],
        WeaponId::Python => PYTHON_ZOOM,
        _ => return false,
    };
    if d >= on {
        true
    } else if d < off {
        false
    } else {
        zoomed
    }
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
        let off = Vec3::new(500.0, 500.0 * lb_core::dmath::tan(10f32.to_radians()), 0.0);
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
            "no rockets at point blank"
        );
        assert!(on_target(&shot(Vec3::new(800.0, 0.0, 0.0), false, WeaponId::Rpg)));
        assert!(
            !on_target(&shot(off, true, WeaponId::Rpg)),
            "rockets are aimed with care"
        );
        assert!(!on_target(&shot(Vec3::new(200.0, 0.0, 0.0), false, WeaponId::Crowbar)));
        assert!(on_target(&shot(Vec3::new(40.0, 0.0, 0.0), false, WeaponId::Crowbar)));
    }

    #[test]
    fn secondary_modes_where_they_pay() {
        let glock = Armed::new(WeaponId::Glock, Some(17), Some(50));
        assert_eq!(mode(&glock, 100.0, false).attack, Attack::Secondary);
        assert_eq!(mode(&glock, 400.0, false).attack, Attack::Primary);
        let hornets = |n| Armed::new(WeaponId::Hornetgun, None, Some(n));
        assert_eq!(mode(&hornets(8), 200.0, false).attack, Attack::Secondary);
        assert_eq!(mode(&hornets(2), 200.0, false).attack, Attack::Primary);
        let shotgun = |clip| Armed::new(WeaponId::Shotgun, Some(clip), Some(20));
        assert_eq!(mode(&shotgun(8), 150.0, true).attack, Attack::Secondary);
        assert_eq!(mode(&shotgun(1), 150.0, true).attack, Attack::Primary, "one shell left");
        assert_eq!(mode(&shotgun(8), 150.0, false).attack, Attack::Primary);
        assert_eq!(mode(&shotgun(8), 500.0, true).attack, Attack::Primary);
        assert!(zoom_wanted(WeaponId::Crossbow, Some(900.0), false));
        assert!(
            zoom_wanted(WeaponId::Crossbow, Some(500.0), true),
            "kept between the thresholds"
        );
        assert!(!zoom_wanted(WeaponId::Crossbow, Some(500.0), false));
        assert!(!zoom_wanted(WeaponId::Crossbow, None, true));
        assert!(!zoom_wanted(WeaponId::Mp5, Some(2000.0), false));
    }
}
