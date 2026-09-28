//! Weapon statistics of the bots for stand runs (`lb stats`): rounds fired and damage dealt per weapon and distance,
//! kills and suicides from the kill feed. This is measurement, not behavior: it may read what the bots do not know
//! (who hit whom), and nothing here feeds back into them.
//!
//! - **Rounds** are what a weapon's clip and reserve lost (a reload moves ammo, it loses none) while in hand (weapons
//!   share ammo), and what a bot carries less of grenades, satchels, snarks and mines whatever is in hand; the
//!   distance band is that of the bot's target at the time. The crossbow's zoomed shots and the MP5's grenades have rows
//!   of their own.
//! - **Damage** a bot takes is credited to the bot firing where the `Damage` message says it came from (bullets
//!   and the zoomed crossbow report the shooter), or else to whoever threw or fired the projectile seen there moments
//!   before (a bolt, a rocket, a grenade, a satchel, a mine, a snark, a hornet). What neither explains is counted
//!   apart, and so is what a bot's own explosives did to it.
//! - **Hit rate** is damage over rounds times the damage a round does when it hits.

use lb_game::mechanics::{BOLT_HIT, Damages};
use lb_game::weapons::WeaponId;
use rustc_hash::FxHashMap;

/// Distance bands, upper bounds.
pub const BANDS: [f32; 4] = [300.0, 800.0, 1500.0, f32::INFINITY];
const BAND_NAMES: [&str; 4] = ["<300", "300-800", "800-1500", ">1500"];
/// A shooter stands this close to the reported source of a bullet's damage.
pub const SOURCE_MATCH: f32 = 40.0;

pub fn band(distance: f32) -> usize {
    BANDS.iter().position(|b| distance < *b).unwrap_or(BANDS.len() - 1)
}

/// A weapon, or its secondary fire where that is another weapon in all but name: the crossbow's zoomed hitscan shot,
/// the MP5's grenade launcher.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Row {
    pub weapon: WeaponId,
    pub alt: bool,
}

impl Row {
    pub fn plain(weapon: WeaponId) -> Row {
        Row { weapon, alt: false }
    }

    pub fn alt(weapon: WeaponId) -> Row {
        Row { weapon, alt: true }
    }

    fn name(self) -> String {
        match (self.weapon, self.alt) {
            (WeaponId::Crossbow, true) => "weapon_crossbow+scope".to_string(),
            (WeaponId::Mp5, true) => "weapon_9mmAR+m203".to_string(),
            (w, _) => w.classname().to_string(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct WeaponRow {
    pub rounds: [u32; 4],
    pub damage: [f32; 4],
}

#[derive(Clone, Debug, Default)]
pub struct ArmsStats {
    pub since: f64,
    pub weapons: FxHashMap<Row, WeaponRow>,
    /// Kill feed weapon name → (kills by bots, bot suicides).
    pub kills: FxHashMap<String, (u32, u32)>,
    /// Deaths of bots to the world (falls) and to other players.
    pub deaths: u32,
    /// Damage bots took from explosions no bot's projectile explains.
    pub blast: f32,
    /// Damage bots took from their own explosives.
    pub own_blast: f32,
}

/// Damage one round does when it hits: a shotgun shell's pellets, a gauss cell's share of a shot, a bolt's hit and
/// blast.
fn round_damage(row: Row, d: &Damages) -> f32 {
    match (row.weapon, row.alt) {
        (WeaponId::Shotgun, _) => 4.0 * d.buckshot,
        (WeaponId::Gauss, _) => d.gauss / 2.0,
        (WeaponId::Crossbow, false) => BOLT_HIT + d.xbow_bolt,
        (WeaponId::Mp5, true) => d.m203,
        (w, _) => d.primary(w),
    }
}

impl ArmsStats {
    pub fn reset(&mut self, now: f64) {
        *self = ArmsStats {
            since: now,
            ..ArmsStats::default()
        };
    }

    pub fn fired(&mut self, row: Row, rounds: u32, distance: f32) {
        self.weapons.entry(row).or_default().rounds[band(distance)] += rounds;
    }

    pub fn blasted(&mut self, damage: f32) {
        self.blast += damage;
    }

    pub fn hurt_self(&mut self, damage: f32) {
        self.own_blast += damage;
    }

    pub fn hit(&mut self, row: Row, damage: f32, distance: f32) {
        self.weapons.entry(row).or_default().damage[band(distance)] += damage;
    }

    /// A kill feed line: `bot_killer` a bot killed someone, `suicide` a bot killed itself.
    pub fn death(&mut self, weapon: &str, bot_killer: bool, suicide: bool, bot_victim: bool) {
        if suicide || bot_killer {
            let e = self.kills.entry(weapon.to_string()).or_default();
            if suicide {
                e.1 += 1;
            } else {
                e.0 += 1;
            }
        }
        if bot_victim {
            self.deaths += 1;
        }
    }

    pub fn report(&self, now: f64, damages: &Damages) -> Vec<String> {
        let minutes = ((now - self.since) / 60.0).max(1e-6);
        let mut out = vec![format!("weapon statistics over {minutes:.1} min")];
        let mut ids: Vec<&Row> = self.weapons.keys().collect();
        ids.sort();
        if !ids.is_empty() {
            out.push(format!(
                "  {:<22} {:>8} {:>9}  hit rate by distance {:?}",
                "weapon", "rounds", "damage", BAND_NAMES
            ));
        }
        for w in ids {
            let r = &self.weapons[w];
            let rounds: u32 = r.rounds.iter().sum();
            let damage: f32 = r.damage.iter().sum();
            let per = round_damage(*w, damages).max(1.0);
            let rates: Vec<String> = (0..4)
                .map(|i| {
                    if r.rounds[i] == 0 {
                        "-".to_string()
                    } else {
                        format!(
                            "{:.0}% of {}",
                            (r.damage[i] / (r.rounds[i] as f32 * per) * 100.0).min(100.0),
                            r.rounds[i]
                        )
                    }
                })
                .collect();
            out.push(format!(
                "  {:<22} {rounds:>8} {damage:>9.0}  {}",
                w.name(),
                rates.join(", ")
            ));
        }
        let mut names: Vec<(&String, &(u32, u32))> = self.kills.iter().collect();
        names.sort_by(|a, b| (b.1.0 + b.1.1).cmp(&(a.1.0 + a.1.1)).then(a.0.cmp(b.0)));
        let (kills, suicides) = self.kills.values().fold((0, 0), |(k, s), (a, b)| (k + a, s + b));
        out.push(format!(
            "  damage from explosions of no bot's {:.0}, from a bot's own {:.0}",
            self.blast, self.own_blast
        ));
        out.push(format!(
            "  kills by bots {kills} ({:.1}/min), bot suicides {suicides} ({:.1}/h), bot deaths {}",
            f64::from(kills) / minutes,
            f64::from(suicides) / minutes * 60.0,
            self.deaths
        ));
        for (name, (k, s)) in names {
            out.push(format!("    {name:<14} kills {k:>4}, suicides {s:>3}"));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates_per_band() {
        let mut s = ArmsStats::default();
        s.fired(Row::plain(WeaponId::Glock), 10, 500.0);
        s.hit(Row::plain(WeaponId::Glock), 60.0, 500.0);
        s.fired(Row::alt(WeaponId::Crossbow), 4, 900.0);
        s.hit(Row::alt(WeaponId::Crossbow), 120.0, 900.0);
        s.death("9mmhandgun", true, false, true);
        s.death("rpg_rocket", false, true, true);
        s.death("crowbar", false, false, false);
        s.death("357", false, false, true);
        let r = s.report(60.0, &Damages::default());
        assert!(!r.iter().any(|l| l.contains("crowbar") || l.contains("357")), "{r:?}");
        assert!(r.iter().any(|l| l.contains("bot deaths 3")), "{r:?}");
        assert!(
            r.iter()
                .any(|l| l.contains("weapon_9mmhandgun") && l.contains("50% of 10")),
            "{r:?}"
        );
        assert!(
            r.iter()
                .any(|l| l.contains("weapon_crossbow+scope") && l.contains("25% of 4")),
            "{r:?}"
        );
        assert!(r.iter().any(|l| l.contains("bot suicides 1")), "{r:?}");
        assert_eq!(band(299.0), 0);
        assert_eq!(band(5000.0), 3);
    }
}
