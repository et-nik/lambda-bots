//! Items placed on HLDM maps: what an entity class gives and when a taken one comes back
//! (`multiplay_gamerules.cpp`: items 30 s, weapons and ammo 20 s).

use crate::weapons::WeaponId;

pub const ITEM_RESPAWN: f32 = 30.0;
pub const WEAPON_RESPAWN: f32 = 20.0;
pub const AMMO_RESPAWN: f32 = 20.0;

/// Ammo types by what they feed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Ammo {
    /// Glock and MP5.
    Nine,
    Buckshot,
    Magnum,
    Bolts,
    Rockets,
    /// Gauss and egon.
    Uranium,
    ArGrenades,
}

impl Ammo {
    pub const ALL: [Ammo; 7] = [
        Ammo::Nine,
        Ammo::Buckshot,
        Ammo::Magnum,
        Ammo::Bolts,
        Ammo::Rockets,
        Ammo::Uranium,
        Ammo::ArGrenades,
    ];

    pub fn index(self) -> usize {
        self as usize
    }

    /// Weapons this ammo is fired from.
    pub fn feeds(self) -> &'static [WeaponId] {
        match self {
            Ammo::Nine => &[WeaponId::Glock, WeaponId::Mp5],
            Ammo::Buckshot => &[WeaponId::Shotgun],
            Ammo::Magnum => &[WeaponId::Python],
            Ammo::Bolts => &[WeaponId::Crossbow],
            Ammo::Rockets => &[WeaponId::Rpg],
            Ammo::Uranium => &[WeaponId::Gauss, WeaponId::Egon],
            Ammo::ArGrenades => &[WeaponId::Mp5],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ItemKind {
    Health,
    Battery,
    LongJump,
    Weapon(WeaponId),
    Ammo(Ammo),
}

impl ItemKind {
    /// From the entity class a map places (`weapon_*`, `ammo_*`, `item_*`), aliases included.
    pub fn from_classname(name: &str) -> Option<ItemKind> {
        let name = name.to_ascii_lowercase();
        if let Some(ammo) = name.strip_prefix("ammo_") {
            let a = match ammo {
                "9mmclip" | "glockclip" | "9mmar" | "mp5clip" | "9mmbox" => Ammo::Nine,
                "buckshot" => Ammo::Buckshot,
                "357" => Ammo::Magnum,
                "crossbow" => Ammo::Bolts,
                "rpgclip" => Ammo::Rockets,
                "gaussclip" | "egonclip" => Ammo::Uranium,
                "argrenades" | "mp5grenades" => Ammo::ArGrenades,
                _ => return None,
            };
            return Some(ItemKind::Ammo(a));
        }
        match name.as_str() {
            "item_healthkit" => Some(ItemKind::Health),
            "item_battery" => Some(ItemKind::Battery),
            "item_longjump" => Some(ItemKind::LongJump),
            _ if name.starts_with("weapon_") => WeaponId::from_classname(&name).map(ItemKind::Weapon),
            _ => None,
        }
    }

    /// Seconds before a taken item of this kind comes back.
    pub fn respawn(self) -> f32 {
        match self {
            ItemKind::Weapon(_) => WEAPON_RESPAWN,
            ItemKind::Ammo(_) => AMMO_RESPAWN,
            ItemKind::Health | ItemKind::Battery | ItemKind::LongJump => ITEM_RESPAWN,
        }
    }

    pub fn as_str(self) -> String {
        match self {
            ItemKind::Health => "health".into(),
            ItemKind::Battery => "battery".into(),
            ItemKind::LongJump => "longjump".into(),
            ItemKind::Weapon(w) => w.classname().to_string(),
            ItemKind::Ammo(a) => format!("{a:?}").to_ascii_lowercase(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classes() {
        assert_eq!(ItemKind::from_classname("item_healthkit"), Some(ItemKind::Health));
        assert_eq!(
            ItemKind::from_classname("weapon_9mmAR"),
            Some(ItemKind::Weapon(WeaponId::Mp5))
        );
        assert_eq!(
            ItemKind::from_classname("weapon_mp5"),
            Some(ItemKind::Weapon(WeaponId::Mp5))
        );
        assert_eq!(
            ItemKind::from_classname("ammo_9mmbox"),
            Some(ItemKind::Ammo(Ammo::Nine))
        );
        assert_eq!(
            ItemKind::from_classname("ammo_ARgrenades"),
            Some(ItemKind::Ammo(Ammo::ArGrenades))
        );
        assert_eq!(ItemKind::from_classname("item_suit"), None);
        assert_eq!(ItemKind::Weapon(WeaponId::Glock).respawn(), 20.0);
    }
}
