//! Fire control: pull the trigger when the view is on the target closely enough (yapb `focusEnemy`), the target is
//! within the weapon's reach, and no blast would reach the shooter; semi-automatic weapons are clicked at a human
//! cadence. The secondary attack is used where it pays: the glock's rapid fire when it lands more bullets a second
//! than aimed single shots (up close), the hornet gun's darts up close, both shotgun barrels up close (and half the
//! time a little farther); the crossbow's scope is snapped on for single far shots (`arms::scope`).

use lb_core::Vec3;
use lb_core::math::view_angle_vectors;
use lb_core::rng::Pcg32;
use lb_game::mechanics::{AltFire, Attack, BODY, Trigger, WeaponClass, spec};
use lb_game::weapons::WeaponId;

use crate::policy::{Armed, XBOW_UNZOOM, XBOW_ZOOM_FROM};

#[derive(Clone, Copy, Debug)]
pub struct Shot {
    pub eye: Vec3,
    pub view: Vec3,
    pub aim: Vec3,
    pub distance: f32,
    pub enemy_faces_me: bool,
    pub weapon: WeaponId,
    /// Rockets are not fired closer than this (`policy::rocket_min`).
    pub rocket_min: f32,
}

/// A rocket's blast makes up for this much of a miss, units: its aim may be off by that much, within these angles.
const ROCKET_SLACK: f32 = 80.0;
const ROCKET_AIM: [f32; 2] = [5.7, 10.0];

/// The view is on the target closely enough to fire.
pub fn on_target(s: &Shot) -> bool {
    let w = spec(s.weapon);
    let (forward, _, _) = view_angle_vectors(s.view);
    let dot = forward.dot((s.aim - s.eye).normalize_or_zero());
    if w.class == WeaponClass::Melee {
        return s.distance < 64.0 && dot > 0.8;
    }
    if s.distance > w.reach || (s.weapon == WeaponId::Rpg && s.distance < s.rocket_min) {
        return false;
    }
    match s.weapon {
        // One rocket per reload, aimed at the feet: worth a careful aim far off, and close by the blast makes up for
        // a miss.
        WeaponId::Rpg => {
            let slack = lb_core::dmath::atan2(ROCKET_SLACK, s.distance.max(1.0)).to_degrees();
            return dot > lb_core::dmath::cos(slack.clamp(ROCKET_AIM[0], ROCKET_AIM[1]).to_radians());
        }
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

/// Why `s` is not on target, for the diagnostics.
pub fn off_target(s: &Shot) -> &'static str {
    let w = spec(s.weapon);
    if w.class != WeaponClass::Melee && s.distance > w.reach {
        "out of the weapon's reach"
    } else if s.weapon == WeaponId::Rpg && s.distance < s.rocket_min {
        "too close for a rocket"
    } else {
        "the aim is not on it yet"
    }
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

/// Hornet darts this close, with at least `DARTS_HORNETS` hornets.
const DARTS_UNDER: f32 = 250.0;
const DARTS_HORNETS: i32 = 4;
/// Both barrels always this close; farther, up to `DOUBLE_UNDER`, when rolled (`roll_double`).
const DOUBLE_ALWAYS: f32 = 300.0;
const DOUBLE_UNDER: f32 = 500.0;

/// How to fire `a` at `distance`; `double` allows both shotgun barrels a little farther than they are always fired
/// (`DOUBLE_ALWAYS`). `aim_sigma` is the bot's aim error there (units) and `click` its pause between single shots:
/// the glock's rapid fire, held down at five shots a second in a cone ten times wider, is taken where it lands more
/// bullets a second than the clicked aimed shots.
pub fn mode(a: &Armed, distance: f32, double: bool, aim_sigma: f32, click: f32) -> Mode {
    let s = spec(a.id);
    let primary = Mode {
        attack: Attack::Primary,
        trigger: s.trigger,
        cycle: s.cycle,
    };
    match s.alt {
        AltFire::Rapid { cycle, spread } => {
            let hits = |cone: f32, every: f32| s.hit_chance_with([cone, cone], distance, BODY, aim_sigma) / every;
            if hits(spread, cycle) > hits(s.spread[0], click.max(s.cycle)) {
                Mode {
                    attack: Attack::Secondary,
                    trigger: Trigger::Hold,
                    cycle,
                }
            } else {
                primary
            }
        }
        AltFire::Darts { cycle } if distance <= DARTS_UNDER && a.reserve.is_some_and(|r| r >= DARTS_HORNETS) => Mode {
            attack: Attack::Secondary,
            trigger: Trigger::Hold,
            cycle,
        },
        AltFire::Double { cycle, .. }
            if (distance <= DOUBLE_ALWAYS || double && distance <= DOUBLE_UNDER) && a.clip.is_some_and(|c| c >= 2) =>
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

/// Whether the shotgun's next shot beyond `DOUBLE_ALWAYS` uses both barrels: half the time.
pub fn roll_double(rng: &mut Pcg32) -> bool {
    rng.next_f32() < 0.5
}

/// Whether the view should be zoomed with `w` against a target `distance` away (`None`: no target), given the state
/// now. The crossbow's scope is put on and taken off by its protocol (`arms::scope`) and only taken off here, when it
/// is left on with no target or a close one; the 357's never helps (it narrows the view and does not steady the shot).
pub fn zoom_wanted(w: WeaponId, distance: Option<f32>, zoomed: bool) -> bool {
    match (w, distance) {
        (WeaponId::Crossbow, Some(d)) => zoomed && d >= XBOW_UNZOOM,
        _ => false,
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
            rocket_min: 300.0,
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
            "no rockets into their own blast's reach"
        );
        let healthy = Shot {
            rocket_min: 200.0,
            ..shot(Vec3::new(220.0, 30.0, 0.0), false, WeaponId::Rpg)
        };
        assert!(on_target(&healthy), "close by the blast makes up for a miss");
        assert!(on_target(&shot(Vec3::new(800.0, 0.0, 0.0), false, WeaponId::Rpg)));
        assert!(
            !on_target(&shot(off, true, WeaponId::Rpg)),
            "rockets are aimed with care far off"
        );
        assert!(!on_target(&shot(Vec3::new(200.0, 0.0, 0.0), false, WeaponId::Crowbar)));
        assert!(on_target(&shot(Vec3::new(40.0, 0.0, 0.0), false, WeaponId::Crowbar)));
    }

    #[test]
    fn secondary_modes_where_they_pay() {
        let glock = Armed::new(WeaponId::Glock, Some(17), Some(50));
        // A normal bot: aim error about 10 units up close, clicks every 0.55 s.
        assert_eq!(mode(&glock, 100.0, false, 11.0, 0.55).attack, Attack::Secondary);
        assert_eq!(mode(&glock, 200.0, false, 12.0, 0.55).attack, Attack::Secondary);
        assert_eq!(mode(&glock, 450.0, false, 14.0, 0.55).attack, Attack::Primary);
        // An expert clicks at the weapon's own rate: the rapid fire pays only closer.
        assert_eq!(mode(&glock, 120.0, false, 1.6, 0.3).attack, Attack::Secondary);
        assert_eq!(mode(&glock, 300.0, false, 1.8, 0.3).attack, Attack::Primary);
        let hornets = |n| Armed::new(WeaponId::Hornetgun, None, Some(n));
        assert_eq!(mode(&hornets(8), 200.0, false, 10.0, 0.3).attack, Attack::Secondary);
        assert_eq!(mode(&hornets(2), 200.0, false, 10.0, 0.3).attack, Attack::Primary);
        let shotgun = |clip| Armed::new(WeaponId::Shotgun, Some(clip), Some(20));
        assert_eq!(mode(&shotgun(8), 150.0, true, 10.0, 0.75).attack, Attack::Secondary);
        assert_eq!(
            mode(&shotgun(1), 150.0, true, 10.0, 0.75).attack,
            Attack::Primary,
            "one shell left"
        );
        assert_eq!(
            mode(&shotgun(8), 150.0, false, 10.0, 0.75).attack,
            Attack::Secondary,
            "always up close"
        );
        assert_eq!(mode(&shotgun(8), 20.0, false, 10.0, 0.75).attack, Attack::Secondary);
        assert_eq!(mode(&shotgun(8), 400.0, true, 10.0, 0.75).attack, Attack::Secondary);
        assert_eq!(mode(&shotgun(8), 400.0, false, 10.0, 0.75).attack, Attack::Primary);
        assert_eq!(mode(&shotgun(8), 700.0, true, 10.0, 0.75).attack, Attack::Primary);
        assert!(
            !zoom_wanted(WeaponId::Crossbow, Some(900.0), false),
            "the scope protocol puts it on"
        );
        assert!(zoom_wanted(WeaponId::Crossbow, Some(900.0), true));
        assert!(
            !zoom_wanted(WeaponId::Crossbow, Some(150.0), true),
            "off for a close target"
        );
        assert!(!zoom_wanted(WeaponId::Crossbow, None, true));
        assert!(!zoom_wanted(WeaponId::Python, Some(2000.0), true));
    }
}
