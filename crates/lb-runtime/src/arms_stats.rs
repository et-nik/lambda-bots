//! Weapon statistics of the bots for stand runs (`lb stats`): rounds fired and damage dealt per weapon and distance,
//! kills and suicides from the kill feed. This is measurement, not behavior: it may read what the bots do not know
//! (who hit whom), and nothing here feeds back into them.
//!
//! - **Rounds** are what a weapon's clip and reserve lost (a reload moves ammo, it loses none); the distance band is
//!   that of the bot's target at the time.
//! - **Damage** a bot takes is credited to the bot standing where the `Damage` message says it came from (bullets
//!   report the shooter); explosions report the blast, so their damage is not credited.
//! - **Hit rate** is damage over rounds times the damage a round does when it hits.

use lb_game::mechanics::Damages;
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

#[derive(Clone, Debug, Default)]
pub struct WeaponRow {
    pub rounds: [u32; 4],
    pub damage: [f32; 4],
}

#[derive(Clone, Debug, Default)]
pub struct ArmsStats {
    pub since: f64,
    pub weapons: FxHashMap<WeaponId, WeaponRow>,
    /// Kill feed weapon name → (kills by bots, bot suicides).
    pub kills: FxHashMap<String, (u32, u32)>,
    /// Deaths of bots to the world (falls) and to other players.
    pub deaths: u32,
}

/// Damage one round does when it hits: a shotgun shell's pellets, a gauss cell's share of a shot.
fn round_damage(w: WeaponId, d: &Damages) -> f32 {
    match w {
        WeaponId::Shotgun => 4.0 * d.buckshot,
        WeaponId::Gauss => d.gauss / 2.0,
        _ => d.primary(w),
    }
}

impl ArmsStats {
    pub fn reset(&mut self, now: f64) {
        *self = ArmsStats {
            since: now,
            ..ArmsStats::default()
        };
    }

    pub fn fired(&mut self, w: WeaponId, rounds: u32, distance: f32) {
        self.weapons.entry(w).or_default().rounds[band(distance)] += rounds;
    }

    pub fn hit(&mut self, w: WeaponId, damage: f32, distance: f32) {
        self.weapons.entry(w).or_default().damage[band(distance)] += damage;
    }

    /// A kill feed line: `bot_killer` a bot killed someone, `suicide` a bot killed itself.
    pub fn death(&mut self, weapon: &str, bot_killer: bool, suicide: bool, bot_victim: bool) {
        let e = self.kills.entry(weapon.to_string()).or_default();
        if suicide {
            e.1 += 1;
        } else if bot_killer {
            e.0 += 1;
        }
        if bot_victim {
            self.deaths += 1;
        }
    }

    pub fn report(&self, now: f64, damages: &Damages) -> Vec<String> {
        let minutes = ((now - self.since) / 60.0).max(1e-6);
        let mut out = vec![format!("weapon statistics over {minutes:.1} min")];
        let mut ids: Vec<&WeaponId> = self.weapons.keys().collect();
        ids.sort();
        if !ids.is_empty() {
            out.push(format!("  {:<18} {:>8} {:>9}  hit rate by distance {:?}", "weapon", "rounds", "damage", BAND_NAMES));
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
                        format!("{:.0}% of {}", (r.damage[i] / (r.rounds[i] as f32 * per) * 100.0).min(100.0), r.rounds[i])
                    }
                })
                .collect();
            out.push(format!("  {:<18} {rounds:>8} {damage:>9.0}  {}", w.classname(), rates.join(", ")));
        }
        let mut names: Vec<(&String, &(u32, u32))> = self.kills.iter().collect();
        names.sort_by(|a, b| (b.1.0 + b.1.1).cmp(&(a.1.0 + a.1.1)).then(a.0.cmp(b.0)));
        let (kills, suicides) = self.kills.values().fold((0, 0), |(k, s), (a, b)| (k + a, s + b));
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
        s.fired(WeaponId::Glock, 10, 500.0);
        s.hit(WeaponId::Glock, 60.0, 500.0);
        s.death("9mmhandgun", true, false, true);
        s.death("rpg_rocket", false, true, true);
        let r = s.report(60.0, &Damages::default());
        assert!(r.iter().any(|l| l.contains("weapon_9mmhandgun") && l.contains("50% of 10")), "{r:?}");
        assert!(r.iter().any(|l| l.contains("bot suicides 1")), "{r:?}");
        assert_eq!(band(299.0), 0);
        assert_eq!(band(5000.0), 3);
    }
}
