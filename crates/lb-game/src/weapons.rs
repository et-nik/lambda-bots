//! HLDM weapon identifiers (`WEAPON_*` in the SDK) and item classes.

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum WeaponId {
    Crowbar = 1,
    Glock = 2,
    Python = 3,
    Mp5 = 4,
    Crossbow = 6,
    Shotgun = 7,
    Rpg = 8,
    Gauss = 9,
    Egon = 10,
    Hornetgun = 11,
    HandGrenade = 12,
    Tripmine = 13,
    Satchel = 14,
    Snark = 15,
}

pub const WEAPON_SUIT_BIT: u32 = 31;

impl WeaponId {
    pub const ALL: [WeaponId; 14] = [
        WeaponId::Crowbar,
        WeaponId::Glock,
        WeaponId::Python,
        WeaponId::Mp5,
        WeaponId::Crossbow,
        WeaponId::Shotgun,
        WeaponId::Rpg,
        WeaponId::Gauss,
        WeaponId::Egon,
        WeaponId::Hornetgun,
        WeaponId::HandGrenade,
        WeaponId::Tripmine,
        WeaponId::Satchel,
        WeaponId::Snark,
    ];

    pub fn from_id(id: i32) -> Option<WeaponId> {
        WeaponId::ALL.into_iter().find(|w| *w as i32 == id)
    }

    /// Canonical classname, also the client command that selects the weapon.
    pub fn classname(self) -> &'static str {
        match self {
            WeaponId::Crowbar => "weapon_crowbar",
            WeaponId::Glock => "weapon_9mmhandgun",
            WeaponId::Python => "weapon_357",
            WeaponId::Mp5 => "weapon_9mmAR",
            WeaponId::Crossbow => "weapon_crossbow",
            WeaponId::Shotgun => "weapon_shotgun",
            WeaponId::Rpg => "weapon_rpg",
            WeaponId::Gauss => "weapon_gauss",
            WeaponId::Egon => "weapon_egon",
            WeaponId::Hornetgun => "weapon_hornetgun",
            WeaponId::HandGrenade => "weapon_handgrenade",
            WeaponId::Tripmine => "weapon_tripmine",
            WeaponId::Satchel => "weapon_satchel",
            WeaponId::Snark => "weapon_snark",
        }
    }

    pub fn from_classname(name: &str) -> Option<WeaponId> {
        let name = name.strip_prefix("weapon_").unwrap_or(name);
        match name {
            "glock" => Some(WeaponId::Glock),
            "python" => Some(WeaponId::Python),
            "mp5" => Some(WeaponId::Mp5),
            _ => WeaponId::ALL
                .into_iter()
                .find(|w| &w.classname()["weapon_".len()..] == name),
        }
    }

    pub fn is_melee(self) -> bool {
        self == WeaponId::Crowbar
    }

    pub fn is_throwable(self) -> bool {
        matches!(
            self,
            WeaponId::HandGrenade | WeaponId::Tripmine | WeaponId::Satchel | WeaponId::Snark
        )
    }

    pub fn is_primary(self) -> bool {
        matches!(
            self,
            WeaponId::Mp5 | WeaponId::Crossbow | WeaponId::Shotgun | WeaponId::Rpg | WeaponId::Gauss | WeaponId::Egon
        )
    }

    pub fn is_secondary(self) -> bool {
        matches!(self, WeaponId::Glock | WeaponId::Python | WeaponId::Hornetgun)
    }

    pub fn bit(self) -> u32 {
        1 << (self as u32)
    }
}

/// Iterates the weapons present in an `entvars.weapons` bit mask.
pub fn weapons_in_mask(mask: u32) -> impl Iterator<Item = WeaponId> {
    WeaponId::ALL.into_iter().filter(move |w| mask & w.bit() != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classnames_roundtrip_and_aliases() {
        for w in WeaponId::ALL {
            assert_eq!(WeaponId::from_classname(w.classname()), Some(w));
        }
        assert_eq!(WeaponId::from_classname("weapon_glock"), Some(WeaponId::Glock));
        assert_eq!(WeaponId::from_classname("weapon_mp5"), Some(WeaponId::Mp5));
        let mask = WeaponId::Crowbar.bit() | WeaponId::Glock.bit() | (1 << WEAPON_SUIT_BIT);
        assert_eq!(
            weapons_in_mask(mask).collect::<Vec<_>>(),
            vec![WeaponId::Crowbar, WeaponId::Glock]
        );
    }
}
