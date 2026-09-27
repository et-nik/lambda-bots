//! Weapon policy: which weapon to fight with at a distance. Expected damage per second from the mechanics (spread
//! and the shooter's own aim error), outside a weapon's band only a third counts, the current weapon gets a
//! margin against flip-flopping, and yapb's order breaks ties.

use lb_game::mechanics::{WeaponClass, spec};
use lb_game::weapons::WeaponId;

/// A weapon the bot owns, with what it knows about its ammo.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Armed {
    pub id: WeaponId,
    /// Rounds in the clip; `None` when not known yet.
    pub clip: Option<i32>,
    /// Rounds in reserve; `None` when not known yet.
    pub reserve: Option<i32>,
}

impl Armed {
    /// Can fire right now: a loaded clip, or reserve ammo for clip-less weapons (the crowbar always).
    pub fn loaded(&self) -> bool {
        let s = spec(self.id);
        if s.class == WeaponClass::Melee {
            return true;
        }
        if s.clip < 0 {
            return self.reserve.is_none_or(|r| r > 0);
        }
        self.clip.is_none_or(|c| c > 0)
    }

    pub fn can_reload(&self) -> bool {
        let s = spec(self.id);
        s.clip > 0 && self.clip.is_some_and(|c| c < s.clip) && self.reserve.is_some_and(|r| r > 0)
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

const OUT_OF_BAND: f32 = 0.35;
const KEEP_MARGIN: f32 = 1.2;

/// Best weapon against a target at `distance`; `aim_sigma` is the bot's aim error there, in units.
pub fn choose(weapons: &[Armed], current: Option<WeaponId>, distance: f32, underwater: bool, aim_sigma: f32) -> Choice {
    let usable = |a: &&Armed| {
        let s = spec(a.id);
        s.class != WeaponClass::Throwable && (s.underwater || !underwater)
    };
    let score = |a: &Armed| {
        let s = spec(a.id);
        let mut v = s.dps(distance, aim_sigma);
        if !s.in_band(distance) {
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
        .filter(|a| spec(a.id).class != WeaponClass::Melee || distance <= spec(a.id).reach)
        .max_by(|a, b| score(a).total_cmp(&score(b)));
    if let Some(a) = best {
        return Choice::Use(a.id);
    }
    if let Some(a) = weapons
        .iter()
        .filter(usable)
        .filter(|a| a.can_reload())
        .max_by(|a, b| score(a).total_cmp(&score(b)))
    {
        return Choice::Reload(a.id);
    }
    Choice::Use(WeaponId::Crowbar)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn armed(id: WeaponId, clip: i32, reserve: i32) -> Armed {
        Armed {
            id,
            clip: Some(clip),
            reserve: Some(reserve),
        }
    }

    #[test]
    fn picks_by_distance_and_keeps_loaded_weapons_first() {
        let kit = [
            armed(WeaponId::Crowbar, -1, 0),
            armed(WeaponId::Glock, 17, 68),
            armed(WeaponId::Shotgun, 8, 12),
        ];
        assert_eq!(choose(&kit, None, 200.0, false, 10.0), Choice::Use(WeaponId::Shotgun));
        assert_eq!(choose(&kit, None, 1400.0, false, 20.0), Choice::Use(WeaponId::Glock));
        let dry = [armed(WeaponId::Crowbar, -1, 0), armed(WeaponId::Glock, 0, 34)];
        assert_eq!(
            choose(&dry, Some(WeaponId::Glock), 500.0, false, 10.0),
            Choice::Reload(WeaponId::Glock)
        );
        assert_eq!(choose(&dry, None, 40.0, false, 10.0), Choice::Use(WeaponId::Crowbar));
        let empty = [armed(WeaponId::Crowbar, -1, 0), armed(WeaponId::Glock, 0, 0)];
        assert_eq!(choose(&empty, None, 500.0, false, 10.0), Choice::Use(WeaponId::Crowbar));
    }

    #[test]
    fn water_rules_out_the_mp5() {
        let kit = [armed(WeaponId::Glock, 17, 68), armed(WeaponId::Mp5, 50, 100)];
        assert_eq!(choose(&kit, None, 300.0, false, 10.0), Choice::Use(WeaponId::Mp5));
        assert_eq!(choose(&kit, None, 300.0, true, 10.0), Choice::Use(WeaponId::Glock));
    }
}
