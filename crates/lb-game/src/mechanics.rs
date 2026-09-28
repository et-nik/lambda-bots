//! Weapon mechanics as BugfixedHL-Rebased plays them in multiplayer: what the weapon policy, fire control and the
//! weapon protocols need to judge and work a weapon. Damage comes from the server's `mp_dmg_*` cvars when it has them
//! ([`Damages`]); the rest is fixed by the game code.

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

/// The two attack buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Attack {
    Primary,
    Secondary,
}

/// What the secondary attack does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AltFire {
    None,
    /// Faster shots in a wider cone (glock).
    Rapid {
        cycle: f32,
        spread: f32,
    },
    /// Toggles a zoomed view of `fov` degrees; the next toggle waits `toggle` seconds.
    Zoom {
        fov: f32,
        toggle: f32,
    },
    /// Both barrels: `pellets` in a wider cone for two shells (shotgun).
    Double {
        pellets: u8,
        spread: [f32; 2],
        cycle: f32,
    },
    /// A contact grenade from the launcher under the barrel (MP5).
    Launcher {
        cycle: f32,
    },
    /// Charges while held, fires on release (gauss).
    Charge,
    /// Straight darts (hornet gun).
    Darts {
        cycle: f32,
    },
    /// Sets off the thrown charges (satchel; which button does it depends on the game DLL).
    Detonate,
    /// Switches the laser guide (RPG).
    Laser,
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
    /// Damage of one bullet, pellet, hit or explosion with the default cvars; see [`Damages`] for the server's.
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
    pub alt: AltFire,
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
    alt: AltFire,
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
        alt,
    }
}

use AltFire::{Charge, Darts, Detonate, Double, Laser, Launcher, Rapid, Zoom};
use Trigger::{Hold, Tap};
use WeaponClass::*;

/// `VECTOR_CONE_DM_DOUBLESHOTGUN`: 20° by 5°.
const DOUBLE_SPREAD: [f32; 2] = [0.173_65, 0.043_62];

/// Every weapon, in yapb's order of preference.
#[rustfmt::skip]
pub const SPECS: [WeaponSpec; 14] = [
    //       weapon                  class           clip trigger cycle damage pel spread (h, v)          water  band               reach   reload rank alt fire
    spec_row(WeaponId::Crowbar,     Melee,          -1,  Hold,   0.4,  25.0,  1, [0.0, 0.0],             true,  [0.0, 64.0],       64.0,   0.0,   0,  AltFire::None),
    spec_row(WeaponId::Glock,       Pistol,         17,  Tap,    0.3,  12.0,  1, [0.01, 0.01],           true,  [0.0, 1500.0],     8192.0, 1.5,   1,  Rapid { cycle: 0.2, spread: 0.1 }),
    spec_row(WeaponId::Hornetgun,   Pistol,         -1,  Hold,   0.25, 10.0,  1, [0.0, 0.0],             true,  [150.0, 900.0],    2048.0, 0.0,   2,  Darts { cycle: 0.1 }),
    spec_row(WeaponId::Python,      Pistol,         6,   Tap,    0.75, 50.0,  1, [0.008_73, 0.008_73],   false, [0.0, 4000.0],     8192.0, 2.0,   3,  Zoom { fov: 40.0, toggle: 0.5 }),
    spec_row(WeaponId::Snark,       Throwable,      -1,  Tap,    0.3,  10.0,  1, [0.0, 0.0],             false, [150.0, 800.0],    800.0,  0.0,   4,  AltFire::None),
    spec_row(WeaponId::HandGrenade, Throwable,      -1,  Hold,   0.5,  100.0, 1, [0.0, 0.0],             true,  [300.0, 800.0],    800.0,  0.0,   5,  AltFire::None),
    spec_row(WeaponId::Satchel,     Throwable,      -1,  Tap,    1.0,  120.0, 1, [0.0, 0.0],             true,  [150.0, 400.0],    600.0,  0.0,   6,  Detonate),
    spec_row(WeaponId::Tripmine,    Throwable,      -1,  Tap,    0.3,  150.0, 1, [0.0, 0.0],             true,  [0.0, 128.0],      128.0,  0.0,   7,  AltFire::None),
    spec_row(WeaponId::Crossbow,    Sniper,         5,   Tap,    0.75, 120.0, 1, [0.0, 0.0],             true,  [400.0, 4000.0],   8192.0, 4.5,   8,  Zoom { fov: 20.0, toggle: 1.0 }),
    spec_row(WeaponId::Shotgun,     WeaponClass::Shotgun, 8, Tap, 0.75, 20.0, 4, [0.087_16, 0.043_62],   false, [0.0, 750.0],      2048.0, 4.0,   9,  Double { pellets: 8, spread: DOUBLE_SPREAD, cycle: 1.5 }),
    spec_row(WeaponId::Mp5,         Smg,            50,  Hold,   0.1,  12.0,  1, [0.052_34, 0.052_34],   false, [0.0, 2000.0],     8192.0, 1.5,   10, Launcher { cycle: 1.0 }),
    spec_row(WeaponId::Rpg,         Launcher,       1,   Tap,    1.5,  120.0, 1, [0.0, 0.0],             true,  [300.0, 5000.0],   8192.0, 2.0,   11, Laser),
    spec_row(WeaponId::Gauss,       Heavy,          -1,  Hold,   0.2,  20.0,  1, [0.0, 0.0],             false, [0.0, 3000.0],     8192.0, 0.0,   12, Charge),
    spec_row(WeaponId::Egon,        Heavy,          -1,  Hold,   0.1,  20.0,  1, [0.0, 0.0],             false, [128.0, 2000.0],   2048.0, 0.0,   13, AltFire::None),
];

/// Half width and half height of a standing player's box.
pub const BODY: [f32; 2] = [16.0, 36.0];

pub fn spec(id: WeaponId) -> &'static WeaponSpec {
    SPECS.iter().find(|s| s.id == id).expect("every weapon has a spec")
}

/// Most of a throwable a player carries (the game's `*_MAX_CARRY`); zero for other weapons.
pub fn carry_max(w: WeaponId) -> i32 {
    match w {
        WeaponId::HandGrenade => 10,
        WeaponId::Satchel | WeaponId::Tripmine => 5,
        WeaponId::Snark => 15,
        _ => 0,
    }
}

/// Radius of an explosion of `damage`: `RadiusDamage` reaches 2.5 times the damage, falling off linearly.
pub fn blast_radius(damage: f32) -> f32 {
    damage * 2.5
}

/// Damage an explosion of `damage` does `distance` away from its center (given a clear line to it).
pub fn blast_damage(damage: f32, distance: f32) -> f32 {
    let radius = blast_radius(damage);
    if distance >= radius {
        0.0
    } else {
        damage * (1.0 - distance / radius)
    }
}

/// Radius of an unzoomed crossbow bolt's blast (fixed, not from the damage).
pub const BOLT_BLAST_RADIUS: f32 = 128.0;
/// Direct hit of an unzoomed bolt on a player, before its blast (`sk_plr_xbow_bolt_client`).
pub const BOLT_HIT: f32 = 10.0;
/// Seconds of charge for the full gauss shot in multiplayer.
pub const GAUSS_FULL_CHARGE: f32 = 1.5;
/// The charge can be released this long after it started.
pub const GAUSS_MIN_CHARGE: f32 = 0.5;
/// A gauss held charging this long shocks its holder.
pub const GAUSS_OVERCHARGE: f32 = 10.0;
pub const GAUSS_OVERCHARGE_DAMAGE: f32 = 50.0;
/// A thrown hand grenade explodes this long after the pin was pulled.
pub const GRENADE_FUSE: f32 = 3.0;
/// A grenade leaves the hand no sooner than this after the pin was pulled.
pub const GRENADE_MIN_COOK: f32 = 0.5;
/// Grenades, M203 grenades and satchels fall at this share of `sv_gravity`.
pub const PROJECTILE_GRAVITY: f32 = 0.5;
pub const M203_SPEED: f32 = 800.0;
pub const SATCHEL_SPEED: f32 = 274.0;
pub const SNARK_SPEED: f32 = 200.0;
pub const BOLT_SPEED: f32 = 2000.0;
pub const DART_SPEED: f32 = 1200.0;
/// A rocket's top speed; it gets there within half a second.
pub const ROCKET_SPEED: f32 = 2000.0;
/// A rocket follows its laser spot for this long at most.
pub const ROCKET_GUIDE: f32 = 6.0;
/// Satchels further than this from their owner do not go off.
pub const SATCHEL_REACH: f32 = 4096.0;
/// A tripmine is placed where the view meets a wall at most this far from the gun.
pub const TRIPMINE_REACH: f32 = 128.0;
/// A placed tripmine arms after this long.
pub const TRIPMINE_ARM: f32 = 2.5;
/// Hornets the hornet gun holds; one grows back every `HORNET_REGROW` seconds.
pub const HORNET_MAX: i32 = 8;
pub const HORNET_REGROW: f32 = 0.5;

/// Damage the server deals: BugfixedHL's `mp_dmg_*` cvars, their defaults when a server has none.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Damages {
    pub crowbar: f32,
    pub glock: f32,
    pub python: f32,
    pub mp5: f32,
    pub buckshot: f32,
    /// A zoomed crossbow shot (a hitscan bolt in multiplayer).
    pub xbow_scope: f32,
    /// The blast of an unzoomed bolt.
    pub xbow_bolt: f32,
    pub rpg: f32,
    pub gauss: f32,
    /// A fully charged gauss shot.
    pub gauss_charged: f32,
    pub egon: f32,
    pub hornet: f32,
    pub hand_grenade: f32,
    pub satchel: f32,
    pub tripmine: f32,
    pub m203: f32,
}

impl Default for Damages {
    fn default() -> Self {
        Damages {
            crowbar: 25.0,
            glock: 12.0,
            python: 50.0,
            mp5: 12.0,
            buckshot: 20.0,
            xbow_scope: 120.0,
            xbow_bolt: 40.0,
            rpg: 120.0,
            gauss: 20.0,
            gauss_charged: 200.0,
            egon: 20.0,
            hornet: 10.0,
            hand_grenade: 100.0,
            satchel: 120.0,
            tripmine: 150.0,
            m203: 100.0,
        }
    }
}

/// The damage cvars, in the order of [`Damages::field`].
pub const DAMAGE_CVARS: [&str; 16] = [
    "mp_dmg_crowbar",
    "mp_dmg_glock",
    "mp_dmg_357",
    "mp_dmg_mp5",
    "mp_dmg_shotgun",
    "mp_dmg_xbow_scope",
    "mp_dmg_xbow_noscope",
    "mp_dmg_rpg",
    "mp_dmg_gauss_primary",
    "mp_dmg_gauss_secondary",
    "mp_dmg_egon",
    "mp_dmg_hornet",
    "mp_dmg_hgrenade",
    "mp_dmg_satchel",
    "mp_dmg_tripmine",
    "mp_dmg_m203",
];

impl Damages {
    fn field(&mut self, i: usize) -> Option<&mut f32> {
        Some(match i {
            0 => &mut self.crowbar,
            1 => &mut self.glock,
            2 => &mut self.python,
            3 => &mut self.mp5,
            4 => &mut self.buckshot,
            5 => &mut self.xbow_scope,
            6 => &mut self.xbow_bolt,
            7 => &mut self.rpg,
            8 => &mut self.gauss,
            9 => &mut self.gauss_charged,
            10 => &mut self.egon,
            11 => &mut self.hornet,
            12 => &mut self.hand_grenade,
            13 => &mut self.satchel,
            14 => &mut self.tripmine,
            15 => &mut self.m203,
            _ => return None,
        })
    }

    /// Takes the value of a damage cvar; false for other cvars.
    pub fn apply_cvar(&mut self, name: &str, value: f32) -> bool {
        let Some(i) = DAMAGE_CVARS.iter().position(|c| *c == name) else {
            return false;
        };
        if let Some(f) = self.field(i)
            && value >= 0.0
        {
            *f = value;
        }
        true
    }

    /// Damage of one primary shot, pellet, hit or explosion; a crossbow's is the zoomed shot's.
    pub fn primary(&self, w: WeaponId) -> f32 {
        match w {
            WeaponId::Crowbar => self.crowbar,
            WeaponId::Glock => self.glock,
            WeaponId::Python => self.python,
            WeaponId::Mp5 => self.mp5,
            WeaponId::Shotgun => self.buckshot,
            WeaponId::Crossbow => self.xbow_scope,
            WeaponId::Rpg => self.rpg,
            WeaponId::Gauss => self.gauss,
            WeaponId::Egon => self.egon,
            WeaponId::Hornetgun => self.hornet,
            WeaponId::HandGrenade => self.hand_grenade,
            WeaponId::Satchel => self.satchel,
            WeaponId::Tripmine => self.tripmine,
            WeaponId::Snark => spec(WeaponId::Snark).damage,
        }
    }
}

impl WeaponSpec {
    /// Share of the shots that land on a target of half width and half height `half` at `distance`, from a cone of
    /// `spread` (sines of the half angles) and the shooter's aim spread `aim_sigma` (units at that distance).
    pub fn hit_chance_with(&self, spread: [f32; 2], distance: f32, half: [f32; 2], aim_sigma: f32) -> f32 {
        let d = distance.max(1.0);
        if self.class == Melee {
            return if d <= self.reach { 1.0 } else { 0.0 };
        }
        if d > self.reach {
            return 0.0;
        }
        let axis = |half: f32, spread: f32| (half / (d * spread + aim_sigma + 1.0)).clamp(0.0, 1.0);
        axis(half[0], spread[0]) * axis(half[1], spread[1])
    }

    /// Share of primary shots that land on a target of half width and half height `half`.
    pub fn hit_chance(&self, distance: f32, half: [f32; 2], aim_sigma: f32) -> f32 {
        self.hit_chance_with(self.spread, distance, half, aim_sigma)
    }

    /// Expected damage per second of the primary fire at `distance` against a standing player, with `damage` per
    /// bullet or pellet.
    pub fn dps(&self, damage: f32, distance: f32, aim_sigma: f32) -> f32 {
        damage * f32::from(self.pellets) * self.hit_chance(distance, BODY, aim_sigma) / self.cycle
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
        let d = Damages::default();
        let (shotgun, mp5, glock) = (spec(WeaponId::Shotgun), spec(WeaponId::Mp5), spec(WeaponId::Glock));
        let dps = |s: &WeaponSpec, x: f32, sigma: f32| s.dps(d.primary(s.id), x, sigma);
        // BHL's MP5 (12 per bullet, 10 per second) leads up close; the shotgun beats the glock there, the
        // accurate glock wins far away.
        assert!(dps(mp5, 200.0, 10.0) > dps(shotgun, 200.0, 10.0));
        assert!(dps(shotgun, 200.0, 10.0) > dps(glock, 200.0, 10.0));
        assert!(dps(glock, 900.0, 20.0) > dps(shotgun, 900.0, 20.0));
        assert!(dps(glock, 1400.0, 20.0) > dps(mp5, 1400.0, 20.0));
        assert!(dps(shotgun, 900.0, 20.0) < dps(shotgun, 200.0, 20.0) / 4.0);
        assert_eq!(dps(spec(WeaponId::Crowbar), 200.0, 0.0), 0.0, "out of reach");
    }

    #[test]
    fn damage_cvars_and_blasts() {
        let mut d = Damages::default();
        assert!(d.apply_cvar("mp_dmg_357", 40.0));
        assert!(!d.apply_cvar("mp_friendlyfire", 1.0));
        assert_eq!(d.primary(WeaponId::Python), 40.0);
        assert_eq!(DAMAGE_CVARS.len(), 16);
        for (i, name) in DAMAGE_CVARS.iter().enumerate() {
            let mut d = Damages::default();
            assert!(d.apply_cvar(name, 1000.0 + i as f32));
            assert!(d.field(i).is_some_and(|v| *v == 1000.0 + i as f32), "{name}");
        }
        assert_eq!(blast_radius(120.0), 300.0);
        assert_eq!(blast_damage(100.0, 125.0), 50.0);
        assert_eq!(blast_damage(100.0, 260.0), 0.0);
    }
}
