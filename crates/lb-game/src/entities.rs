//! Classification of engine entities the bot layer tracks (kinds used in the adapter registry).

pub const KIND_ITEM: u8 = 1;
pub const KIND_WEAPONBOX: u8 = 2;
pub const KIND_PROJECTILE: u8 = 3;
pub const KIND_MINE: u8 = 4;
pub const KIND_MOVER: u8 = 5;
pub const KIND_BUTTON: u8 = 6;
pub const KIND_BREAKABLE: u8 = 7;
pub const KIND_CHARGER: u8 = 8;
pub const KIND_TRIGGER: u8 = 9;
pub const KIND_LADDER: u8 = 10;
pub const KIND_SPAWN: u8 = 11;
pub const KIND_MONSTER: u8 = 12;

/// `(pattern, is_prefix, kind)`; the first matching rule wins.
pub const TRACK_RULES: &[(&str, bool, u8)] = &[
    ("weapon_", true, KIND_ITEM),
    ("ammo_", true, KIND_ITEM),
    ("item_", true, KIND_ITEM),
    ("weaponbox", false, KIND_WEAPONBOX),
    ("grenade", false, KIND_PROJECTILE),
    ("rpg_rocket", false, KIND_PROJECTILE),
    // The SDK names a flying bolt `bolt`, hlsdk-portable by its entity class.
    ("bolt", false, KIND_PROJECTILE),
    ("crossbow_bolt", false, KIND_PROJECTILE),
    ("hornet", false, KIND_PROJECTILE),
    ("monster_snark", false, KIND_PROJECTILE),
    ("monster_satchel", false, KIND_PROJECTILE),
    ("monster_tripmine", false, KIND_MINE),
    ("func_door", true, KIND_MOVER),
    ("func_plat", true, KIND_MOVER),
    ("func_train", false, KIND_MOVER),
    ("func_tracktrain", false, KIND_MOVER),
    ("func_rotating", false, KIND_MOVER),
    ("func_button", false, KIND_BUTTON),
    ("func_rot_button", false, KIND_BUTTON),
    ("momentary_rot_button", false, KIND_BUTTON),
    ("func_breakable", false, KIND_BREAKABLE),
    ("func_pushable", false, KIND_BREAKABLE),
    ("func_healthcharger", false, KIND_CHARGER),
    ("func_recharge", false, KIND_CHARGER),
    ("trigger_teleport", false, KIND_TRIGGER),
    ("trigger_hurt", false, KIND_TRIGGER),
    ("trigger_push", false, KIND_TRIGGER),
    ("func_ladder", false, KIND_LADDER),
    ("info_player_deathmatch", false, KIND_SPAWN),
    ("info_player_start", false, KIND_SPAWN),
];

pub fn kind_mask(kinds: &[u8]) -> u32 {
    kinds.iter().fold(0, |m, k| m | (1 << k))
}

/// Projectiles and placed explosives a player can see.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProjectileKind {
    /// A hand grenade or an MP5 grenade (the same entity class).
    Grenade,
    Rocket,
    Bolt,
    Hornet,
    Snark,
    Satchel,
    Tripmine,
}

impl ProjectileKind {
    pub fn from_classname(name: &str) -> Option<ProjectileKind> {
        Some(match name {
            "grenade" => ProjectileKind::Grenade,
            "rpg_rocket" => ProjectileKind::Rocket,
            "bolt" | "crossbow_bolt" => ProjectileKind::Bolt,
            "hornet" => ProjectileKind::Hornet,
            "monster_snark" => ProjectileKind::Snark,
            "monster_satchel" => ProjectileKind::Satchel,
            "monster_tripmine" => ProjectileKind::Tripmine,
            _ => return None,
        })
    }

    /// Farthest a player notices one: a rocket's glow and trail carry far, a satchel on the floor does not.
    pub fn view_range(self) -> f32 {
        match self {
            ProjectileKind::Grenade => 1200.0,
            ProjectileKind::Rocket => 3000.0,
            ProjectileKind::Bolt => 1500.0,
            ProjectileKind::Hornet | ProjectileKind::Satchel => 800.0,
            ProjectileKind::Snark => 900.0,
            ProjectileKind::Tripmine => 1000.0,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ProjectileKind::Grenade => "grenade",
            ProjectileKind::Rocket => "rocket",
            ProjectileKind::Bolt => "bolt",
            ProjectileKind::Hornet => "hornet",
            ProjectileKind::Snark => "snark",
            ProjectileKind::Satchel => "satchel",
            ProjectileKind::Tripmine => "tripmine",
        }
    }
}
