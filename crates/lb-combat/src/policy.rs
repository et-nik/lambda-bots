//! Weapon policy: which gun to fight with at a distance.
//!
//! Every gun is scored by the damage per second it is expected to deal to a standing player there: the server's damage,
//! the weapon's spread and the shooter's own aim error; for projectiles also the flight time a moving target has to
//! step aside, and the blast that makes up for part of it. Outside a weapon's band only a third counts. A weapon whose
//! blast would reach the shooter scores nothing (a bolt under 160 units, the egon's beam end under 128, a rocket
//! within its blast's reach unless the shooter has health to spare: [`rocket_min`]). The crossbow is fired zoomed
//! from 250 units on; the scope's toggling is counted in its rate. The current weapon gets a margin against
//! flip-flopping, and yapb's order breaks ties.
//! Throwables are not guns: the weapon protocols throw them.

use lb_game::mechanics::{
    AltFire, BODY, BOLT_BLAST_RADIUS, BOLT_HIT, BOLT_SPEED, Damages, ROCKET_SPEED, WeaponClass, WeaponSpec,
    blast_radius, spec,
};
use lb_game::weapons::WeaponId;

use crate::aim::SCOPE_STEADY;

/// A weapon the bot owns, with what it knows about its ammo.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Armed {
    pub id: WeaponId,
    /// Rounds in the clip; `None` when not known yet.
    pub clip: Option<i32>,
    /// Rounds in reserve; `None` when not known yet.
    pub reserve: Option<i32>,
    /// Reserve of the second ammo type (the MP5's grenades); `None` when not known or none.
    pub reserve2: Option<i32>,
}

impl Armed {
    pub fn new(id: WeaponId, clip: Option<i32>, reserve: Option<i32>) -> Armed {
        Armed {
            id,
            clip,
            reserve,
            reserve2: None,
        }
    }

    /// Can fire right now: a loaded clip, or reserve ammo for clip-less weapons (the crowbar always).
    pub fn loaded(&self) -> bool {
        let s = spec(self.id);
        if s.class == WeaponClass::Melee {
            return true;
        }
        if s.clip < 0 {
            let need = if self.id == WeaponId::Gauss { 2 } else { 1 };
            return self.reserve.is_none_or(|r| r >= need);
        }
        self.clip.is_none_or(|c| c > 0)
    }

    pub fn can_reload(&self) -> bool {
        let s = spec(self.id);
        s.clip > 0 && self.clip.is_some_and(|c| c < s.clip) && self.reserve.is_some_and(|r| r > 0)
    }

    /// Rounds left to fire: clip and reserve; `None` when not known.
    pub fn rounds(&self) -> Option<i32> {
        match (spec(self.id).clip, self.clip, self.reserve) {
            (c, _, r) if c < 0 => r,
            (_, Some(c), Some(r)) => Some(c + r),
            (_, Some(c), None) => Some(c),
            (_, None, r) => r,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    Use(WeaponId),
    /// Nothing is loaded; reload this one.
    Reload(WeaponId),
}

impl Choice {
    pub fn weapon(self) -> WeaponId {
        match self {
            Choice::Use(w) | Choice::Reload(w) => w,
        }
    }
}

/// What a weapon is weighed against.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Target {
    pub distance: f32,
    /// How fast the target moves, units per second (projectiles must lead it).
    pub speed: f32,
    pub aim_sigma: f32,
    /// Rockets are not fired at it closer than this ([`rocket_min`]).
    pub rocket_min: f32,
}

const OUT_OF_BAND: f32 = 0.35;
const KEEP_MARGIN: f32 = 1.2;
/// The crossbow is fired zoomed from here on (a hitscan bolt: an unzoomed one is slow enough to step away from);
/// closer, where a target crosses the zoomed view too fast, it fires bolts.
pub const XBOW_ZOOM_FROM: f32 = 250.0;
/// Seconds per zoomed shot, the scope's toggling counted in: it goes on for a target and comes off after the kill,
/// and the game toggles it once a second at most.
const XBOW_SCOPE_CYCLE: f32 = 2.0;
/// The crossbow's scope comes off with the target closer than this.
pub const XBOW_UNZOOM: f32 = 200.0;
/// With this much health a rocket may be fired into its own blast's edge, taking this much of it: 40 of the 120 at
/// 200 units with the damage at its default.
const ROCKET_HURT: [(f32, f32); 2] = [(80.0, 40.0), (60.0, 20.0)];

/// Rockets of `damage` are fired no closer than this by a shooter with `health`: out of the blast's reach, or into
/// its edge with health to spare (a rocket goes off at the target's feet).
pub fn rocket_min(health: f32, damage: f32) -> f32 {
    let allowed = ROCKET_HURT
        .iter()
        .find(|(h, _)| health >= *h)
        .map_or(0.0, |(_, hurt)| *hurt);
    blast_radius(damage) * (1.0 - allowed / damage.max(1.0)).max(0.0)
}
/// Unzoomed bolts and the egon's beam end blow up this close to the shooter.
const BOLT_MIN: f32 = 160.0;
const EGON_MIN: f32 = 128.0;
/// Seconds between rockets: the shot and the reload of the one-rocket clip; a rocket in the clip goes at once.
const ROCKET_CYCLE: f32 = 3.5;
const ROCKET_READY: f32 = 1.5;
/// A homing hornet finds its target this often within `HORNET_SEEK`.
const HORNET_HIT: f32 = 0.8;
const HORNET_SEEK: f32 = 1024.0;

/// Chance a projectile of `speed` lands on a target at `distance` moving at `target_speed`, from `spread` and aim
/// error `aim_sigma`; a blast of `blast` radius makes up for part of the miss.
fn projectile_hit(s: &WeaponSpec, t: &Target, speed: f32, blast: f32) -> f32 {
    let flight = t.distance / speed.max(1.0);
    let sigma = t.aim_sigma + 0.5 * t.speed * flight;
    let reach = 0.4 * blast;
    s.hit_chance_with(s.spread, t.distance, [BODY[0] + reach, BODY[1] + reach], sigma)
}

/// Expected damage per second of `a` against `t`; zero when it cannot or must not fire.
pub fn score(a: &Armed, t: &Target, damages: &Damages) -> f32 {
    let s = spec(a.id);
    let d = t.distance;
    let dmg = damages.primary(a.id);
    match a.id {
        _ if s.class == WeaponClass::Throwable => 0.0,
        WeaponId::Crossbow if d >= XBOW_ZOOM_FROM => {
            dmg * s.hit_chance(d, BODY, t.aim_sigma * SCOPE_STEADY) / XBOW_SCOPE_CYCLE
        }
        WeaponId::Crossbow if d < BOLT_MIN => 0.0,
        WeaponId::Crossbow => {
            let bolt = BOLT_HIT + damages.xbow_bolt;
            bolt * projectile_hit(s, t, BOLT_SPEED, BOLT_BLAST_RADIUS) / s.cycle
        }
        WeaponId::Rpg if d < t.rocket_min => 0.0,
        WeaponId::Rpg => {
            let cycle = if a.clip.is_some_and(|c| c > 0) {
                ROCKET_READY
            } else {
                ROCKET_CYCLE
            };
            dmg * projectile_hit(s, t, ROCKET_SPEED, blast_radius(dmg)) / cycle
        }
        WeaponId::Egon if d < EGON_MIN => 0.0,
        // Up close the rapid fire lands more.
        WeaponId::Glock => {
            let rapid = match s.alt {
                AltFire::Rapid { cycle, spread } => {
                    dmg * s.hit_chance_with([spread, spread], d, BODY, t.aim_sigma) / cycle
                }
                _ => 0.0,
            };
            s.dps(dmg, d, t.aim_sigma).max(rapid)
        }
        WeaponId::Hornetgun if d > s.reach => 0.0,
        WeaponId::Hornetgun => {
            let seek = if d <= HORNET_SEEK {
                HORNET_HIT
            } else {
                HORNET_HIT * HORNET_SEEK / d
            };
            dmg * seek / s.cycle
        }
        _ => s.dps(dmg, d, t.aim_sigma),
    }
}

/// Best gun against `t`, weighted by how much the bot likes each (`like`: 1 as good as its damage says, more for a
/// favourite, 0 for one it must not use); `underwater` rules out those that do not fire there.
pub fn choose(
    weapons: &[Armed],
    current: Option<WeaponId>,
    t: &Target,
    underwater: bool,
    damages: &Damages,
    like: &dyn Fn(WeaponId) -> f32,
) -> Choice {
    let usable = |a: &&Armed| {
        let s = spec(a.id);
        s.class != WeaponClass::Throwable && (s.underwater || !underwater) && like(a.id) > 0.0
    };
    let rank = |a: &Armed| {
        let s = spec(a.id);
        let mut v = score(a, t, damages) * like(a.id);
        if !s.in_band(t.distance) {
            v *= OUT_OF_BAND;
        }
        if Some(a.id) == current {
            v *= KEEP_MARGIN;
        }
        v + f32::from(s.rank) * 1e-3
    };
    let best = weapons
        .iter()
        .filter(usable)
        .filter(|a| a.loaded())
        .filter(|a| spec(a.id).class != WeaponClass::Melee || t.distance <= spec(a.id).reach)
        .filter(|a| spec(a.id).class == WeaponClass::Melee || score(a, t, damages) > 0.0)
        .max_by(|a, b| rank(a).total_cmp(&rank(b)));
    if let Some(a) = best {
        return Choice::Use(a.id);
    }
    if let Some(a) = weapons
        .iter()
        .filter(usable)
        .filter(|a| a.can_reload())
        .max_by(|a, b| rank(a).total_cmp(&rank(b)))
    {
        return Choice::Reload(a.id);
    }
    Choice::Use(WeaponId::Crowbar)
}

/// The gun the bot would like in hand at `distance` if everything were loaded: the one worth reloading when calm.
pub fn preferred(weapons: &[Armed], t: &Target, damages: &Damages, like: &dyn Fn(WeaponId) -> f32) -> Option<WeaponId> {
    weapons
        .iter()
        .filter(|a| spec(a.id).class != WeaponClass::Throwable && spec(a.id).class != WeaponClass::Melee)
        .filter(|a| like(a.id) > 0.0 && (a.loaded() || a.can_reload()))
        .map(|a| {
            let full = Armed {
                clip: Some(spec(a.id).clip.max(1)),
                ..*a
            };
            (
                a.id,
                score(&full, t, damages) * like(a.id) + f32::from(spec(a.id).rank) * 1e-3,
            )
        })
        .filter(|(_, v)| *v > 0.0)
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(w, _)| w)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn armed(id: WeaponId, clip: i32, reserve: i32) -> Armed {
        Armed::new(id, Some(clip), Some(reserve))
    }

    fn at(distance: f32, aim_sigma: f32) -> Target {
        Target {
            distance,
            speed: 250.0,
            aim_sigma,
            rocket_min: rocket_min(50.0, Damages::default().rpg),
        }
    }

    const ANY: &dyn Fn(WeaponId) -> f32 = &|_| 1.0;

    fn pick(kit: &[Armed], current: Option<WeaponId>, t: Target, underwater: bool) -> Choice {
        choose(kit, current, &t, underwater, &Damages::default(), ANY)
    }

    #[test]
    fn picks_by_distance_and_keeps_loaded_weapons_first() {
        let kit = [
            armed(WeaponId::Crowbar, -1, 0),
            armed(WeaponId::Glock, 17, 68),
            armed(WeaponId::Shotgun, 8, 12),
        ];
        assert_eq!(pick(&kit, None, at(200.0, 10.0), false), Choice::Use(WeaponId::Shotgun));
        assert_eq!(pick(&kit, None, at(1400.0, 20.0), false), Choice::Use(WeaponId::Glock));
        let dry = [armed(WeaponId::Crowbar, -1, 0), armed(WeaponId::Glock, 0, 34)];
        assert_eq!(
            pick(&dry, Some(WeaponId::Glock), at(500.0, 10.0), false),
            Choice::Reload(WeaponId::Glock)
        );
        assert_eq!(pick(&dry, None, at(40.0, 10.0), false), Choice::Use(WeaponId::Crowbar));
        let empty = [armed(WeaponId::Crowbar, -1, 0), armed(WeaponId::Glock, 0, 0)];
        assert_eq!(
            pick(&empty, None, at(500.0, 10.0), false),
            Choice::Use(WeaponId::Crowbar)
        );
    }

    #[test]
    fn water_rules_out_the_mp5() {
        let kit = [armed(WeaponId::Glock, 17, 68), armed(WeaponId::Mp5, 50, 100)];
        assert_eq!(pick(&kit, None, at(300.0, 10.0), false), Choice::Use(WeaponId::Mp5));
        assert_eq!(pick(&kit, None, at(300.0, 10.0), true), Choice::Use(WeaponId::Glock));
    }

    #[test]
    fn heavy_weapons_lead_and_blasts_are_kept_off_the_shooter() {
        let kit = [
            armed(WeaponId::Glock, 17, 68),
            armed(WeaponId::Mp5, 50, 100),
            armed(WeaponId::Rpg, 1, 4),
            armed(WeaponId::Gauss, -1, 60),
            armed(WeaponId::Egon, -1, 60),
            armed(WeaponId::Crossbow, 5, 10),
        ];
        assert_eq!(pick(&kit, None, at(400.0, 12.0), false), Choice::Use(WeaponId::Egon));
        let no_egon: Vec<Armed> = kit.iter().filter(|a| a.id != WeaponId::Egon).copied().collect();
        assert_eq!(
            pick(&no_egon, None, at(400.0, 12.0), false),
            Choice::Use(WeaponId::Gauss)
        );
        let rockets = [armed(WeaponId::Mp5, 50, 100), armed(WeaponId::Rpg, 1, 4)];
        assert_eq!(
            pick(&rockets, None, at(1200.0, 20.0), false),
            Choice::Use(WeaponId::Rpg)
        );
        assert_eq!(
            pick(&rockets, Some(WeaponId::Rpg), at(250.0, 10.0), false),
            Choice::Use(WeaponId::Mp5),
            "no rockets into the blast's reach when hurt"
        );
        let healthy = Target {
            rocket_min: rocket_min(100.0, Damages::default().rpg),
            ..at(250.0, 10.0)
        };
        assert_eq!(
            pick(&rockets, Some(WeaponId::Rpg), healthy, false),
            Choice::Use(WeaponId::Rpg),
            "healthy, into its edge"
        );
        let xbow = [armed(WeaponId::Glock, 17, 68), armed(WeaponId::Crossbow, 5, 10)];
        assert_eq!(
            pick(&xbow, None, at(1500.0, 20.0), false),
            Choice::Use(WeaponId::Crossbow)
        );
        assert_eq!(
            pick(&xbow, None, at(100.0, 10.0), false),
            Choice::Use(WeaponId::Glock),
            "a bolt blast would reach the shooter"
        );
        let dry_gauss = [armed(WeaponId::Glock, 17, 68), armed(WeaponId::Gauss, -1, 1)];
        assert_eq!(
            pick(&dry_gauss, None, at(400.0, 10.0), false),
            Choice::Use(WeaponId::Glock)
        );
    }

    #[test]
    fn rockets_come_closer_with_health_to_spare() {
        let d = Damages::default().rpg;
        for (health, min) in [(100.0, 200.0), (70.0, 250.0), (40.0, 300.0)] {
            assert!((rocket_min(health, d) - min).abs() < 0.01, "{health}");
        }
    }

    #[test]
    fn hornets_score_nothing_beyond_their_reach() {
        let hornets = armed(WeaponId::Hornetgun, -1, 8);
        let d = Damages::default();
        assert!(score(&hornets, &at(2000.0, 20.0), &d) > 0.0);
        assert_eq!(score(&hornets, &at(3000.0, 20.0), &d), 0.0);
        let kit = [armed(WeaponId::Glock, 17, 68), hornets];
        assert_eq!(pick(&kit, None, at(3000.0, 20.0), false), Choice::Use(WeaponId::Glock));
    }

    #[test]
    fn restrictions_and_the_gun_worth_reloading() {
        let kit = [
            armed(WeaponId::Crowbar, -1, 0),
            armed(WeaponId::Glock, 17, 68),
            armed(WeaponId::Mp5, 0, 100),
        ];
        let d = Damages::default();
        let t = at(300.0, 10.0);
        assert_eq!(
            choose(&kit, None, &t, false, &d, &|w| if w == WeaponId::Glock {
                0.0
            } else {
                1.0
            }),
            Choice::Reload(WeaponId::Mp5)
        );
        assert_eq!(choose(&kit, None, &t, false, &d, ANY), Choice::Use(WeaponId::Glock));
        assert_eq!(preferred(&kit, &t, &d, ANY), Some(WeaponId::Mp5));
    }

    #[test]
    fn a_favourite_wins_a_close_call() {
        let kit = [armed(WeaponId::Python, 6, 12), armed(WeaponId::Mp5, 50, 100)];
        let d = Damages::default();
        let t = at(900.0, 10.0);
        let plain = choose(&kit, None, &t, false, &d, ANY);
        let other = if plain == Choice::Use(WeaponId::Mp5) {
            WeaponId::Python
        } else {
            WeaponId::Mp5
        };
        let fond = move |w: WeaponId| if w == other { 3.0 } else { 1.0 };
        assert_eq!(choose(&kit, None, &t, false, &d, &fond), Choice::Use(other));
    }
}
