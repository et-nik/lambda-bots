//! Weapon mechanics as BugfixedHL-Rebased plays them in multiplayer (defaults of its `mp_dmg_*` cvars): what the
//! weapon policy and fire control need to judge a weapon at a distance.

use crate::weapons::WeaponId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WeaponClass {
    Melee,
    Pistol,
    Shotgun,
    Smg,
    Sniper,
    Launcher,
    Heavy,
    Throwable,
}

/// How the attack button is worked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    /// Held down: fires at the cycle rate.
    Hold,
    /// Clicked: one press per shot, at a human cadence.
    Tap,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WeaponSpec {
    pub id: WeaponId,
    pub class: WeaponClass,
    /// Rounds per clip; -1 for weapons that fire straight from the reserve.
    pub clip: i32,
    pub trigger: Trigger,
    /// Seconds between primary shots.
    pub cycle: f32,
    /// Damage of one bullet, pellet, hit or rocket.
    pub damage: f32,
    pub pellets: u8,
    /// Spread of the primary fire as the sine of the half cone (`VECTOR_CONE_*`), horizontal and vertical; 0 for
    /// exact or guided shots.
    pub spread: [f32; 2],
    pub underwater: bool,
    /// Distances the weapon is a good choice at.
    pub band: [f32; 2],
    /// Farthest hit: trace length, or melee reach.
    pub reach: f32,
    /// Seconds for a full reload.
    pub reload: f32,
    /// yapb's preference when weapons score alike: higher is better.
    pub rank: u8,
}

#[allow(clippy::too_many_arguments)]
const fn spec_row(
    id: WeaponId,
    class: WeaponClass,
    clip: i32,
    trigger: Trigger,
    cycle: f32,
    damage: f32,
    pellets: u8,
    spread: [f32; 2],
    underwater: bool,
    band: [f32; 2],
    reach: f32,
    reload: f32,
    rank: u8,
) -> WeaponSpec {
    WeaponSpec {
        id,
        class,
        clip,
        trigger,
        cycle,
        damage,
        pellets,
        spread,
        underwater,
        band,
        reach,
        reload,
        rank,
    }
}

use Trigger::{Hold, Tap};
use WeaponClass::*;

/// Every weapon, in yapb's order of preference.
#[rustfmt::skip]
pub const SPECS: [WeaponSpec; 14] = [
    //       weapon                  class           clip trigger cycle damage pel spread (h, v)          water  band               reach   reload rank
    spec_row(WeaponId::Crowbar,     Melee,          -1,  Hold,   0.4,  25.0,  1, [0.0, 0.0],             true,  [0.0, 64.0],       64.0,   0.0,   0),
    spec_row(WeaponId::Glock,       Pistol,         17,  Tap,    0.3,  12.0,  1, [0.01, 0.01],           true,  [0.0, 1500.0],     8192.0, 1.5,   1),
    spec_row(WeaponId::Hornetgun,   Pistol,         -1,  Hold,   0.25, 10.0,  1, [0.0, 0.0],             true,  [150.0, 900.0],    2048.0, 0.0,   2),
    spec_row(WeaponId::Python,      Pistol,         6,   Tap,    0.75, 50.0,  1, [0.008_73, 0.008_73],   false, [0.0, 4000.0],     8192.0, 2.0,   3),
    spec_row(WeaponId::Snark,       Throwable,      -1,  Tap,    0.3,  10.0,  1, [0.0, 0.0],             false, [128.0, 800.0],    800.0,  0.0,   4),
    spec_row(WeaponId::HandGrenade, Throwable,      -1,  Hold,   0.5,  100.0, 1, [0.0, 0.0],             true,  [300.0, 800.0],    800.0,  0.0,   5),
    spec_row(WeaponId::Satchel,     Throwable,      -1,  Tap,    1.0,  120.0, 1, [0.0, 0.0],             true,  [200.0, 600.0],    600.0,  0.0,   6),
    spec_row(WeaponId::Tripmine,    Throwable,      -1,  Tap,    0.3,  150.0, 1, [0.0, 0.0],             true,  [0.0, 128.0],      128.0,  0.0,   7),
    spec_row(WeaponId::Crossbow,    Sniper,         5,   Tap,    0.75, 40.0,  1, [0.0, 0.0],             true,  [400.0, 4000.0],   8192.0, 4.5,   8),
    spec_row(WeaponId::Shotgun,     WeaponClass::Shotgun, 8, Tap, 0.75, 20.0, 4, [0.087_16, 0.043_62],   false, [0.0, 750.0],      2048.0, 4.0,   9),
    spec_row(WeaponId::Mp5,         Smg,            50,  Hold,   0.1,  12.0,  1, [0.052_34, 0.052_34],   false, [0.0, 2000.0],     8192.0, 1.5,   10),
    spec_row(WeaponId::Rpg,         Launcher,       1,   Tap,    1.5,  120.0, 1, [0.0, 0.0],             true,  [300.0, 5000.0],   8192.0, 2.0,   11),
    spec_row(WeaponId::Gauss,       Heavy,          -1,  Hold,   0.2,  20.0,  1, [0.0, 0.0],             false, [0.0, 3000.0],     8192.0, 0.0,   12),
    spec_row(WeaponId::Egon,        Heavy,          -1,  Hold,   0.1,  20.0,  1, [0.0, 0.0],             false, [128.0, 2000.0],   2048.0, 0.0,   13),
];

/// Half width and half height of a standing player's box.
pub const BODY: [f32; 2] = [16.0, 36.0];

pub fn spec(id: WeaponId) -> &'static WeaponSpec {
    SPECS.iter().find(|s| s.id == id).expect("every weapon has a spec")
}

impl WeaponSpec {
    /// Share of the shots that land on a target of half width and half height `half` at `distance`, with the
    /// shooter's aim spread `aim_sigma` (units at that distance).
    pub fn hit_chance(&self, distance: f32, half: [f32; 2], aim_sigma: f32) -> f32 {
        let d = distance.max(1.0);
        if self.class == Melee {
            return if d <= self.reach { 1.0 } else { 0.0 };
        }
        if d > self.reach {
            return 0.0;
        }
        let axis = |half: f32, spread: f32| (half / (d * spread + aim_sigma + 1.0)).clamp(0.0, 1.0);
        axis(half[0], self.spread[0]) * axis(half[1], self.spread[1])
    }

    /// Expected damage per second at `distance` against a standing player.
    pub fn dps(&self, distance: f32, aim_sigma: f32) -> f32 {
        self.damage * f32::from(self.pellets) * self.hit_chance(distance, BODY, aim_sigma) / self.cycle
    }

    pub fn in_band(&self, distance: f32) -> bool {
        (self.band[0]..=self.band[1]).contains(&distance)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_weapon_has_one_spec_and_ranks_are_unique() {
        for w in WeaponId::ALL {
            assert_eq!(spec(w).id, w);
        }
        let mut ranks: Vec<u8> = SPECS.iter().map(|s| s.rank).collect();
        ranks.sort();
        ranks.dedup();
        assert_eq!(ranks.len(), SPECS.len());
    }

    #[test]
    fn damage_per_second_follows_spread_and_range() {
        let (shotgun, mp5, glock) = (spec(WeaponId::Shotgun), spec(WeaponId::Mp5), spec(WeaponId::Glock));
        // BHL's MP5 (12 per bullet, 10 per second) leads up close; the shotgun beats the glock there, the
        // accurate glock wins far away.
        assert!(mp5.dps(200.0, 10.0) > shotgun.dps(200.0, 10.0));
        assert!(shotgun.dps(200.0, 10.0) > glock.dps(200.0, 10.0));
        assert!(glock.dps(900.0, 20.0) > shotgun.dps(900.0, 20.0));
        assert!(glock.dps(1400.0, 20.0) > mp5.dps(1400.0, 20.0));
        assert!(shotgun.dps(900.0, 20.0) < shotgun.dps(200.0, 20.0) / 4.0);
        assert_eq!(spec(WeaponId::Crowbar).dps(200.0, 0.0), 0.0, "out of reach");
    }
}
