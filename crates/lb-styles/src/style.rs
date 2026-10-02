//! Play styles. A style is data: trait ranges here, utility weights and weapon order in the AI crates.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum StyleId {
    Balanced,
    Rusher,
    Sniper,
    Controller,
    Trapper,
}

impl StyleId {
    pub const ALL: [StyleId; 5] = [
        StyleId::Balanced,
        StyleId::Rusher,
        StyleId::Sniper,
        StyleId::Controller,
        StyleId::Trapper,
    ];

    pub fn parse(s: &str) -> Option<StyleId> {
        StyleId::ALL
            .into_iter()
            .find(|id| id.as_str().eq_ignore_ascii_case(s.trim()))
    }

    pub fn as_str(self) -> &'static str {
        match self {
            StyleId::Balanced => "balanced",
            StyleId::Rusher => "rusher",
            StyleId::Sniper => "sniper",
            StyleId::Controller => "controller",
            StyleId::Trapper => "trapper",
        }
    }

    /// Ranges personalities of this style draw their base aggression and fear from.
    pub fn trait_ranges(self) -> TraitRanges {
        let r = |aggression: [f32; 2], fear: [f32; 2]| TraitRanges { aggression, fear };
        match self {
            StyleId::Balanced => r([0.40, 0.70], [0.40, 0.70]),
            StyleId::Rusher => r([0.70, 1.00], [0.00, 0.40]),
            StyleId::Sniper => r([0.20, 0.50], [0.70, 1.00]),
            StyleId::Controller => r([0.45, 0.70], [0.40, 0.60]),
            StyleId::Trapper => r([0.30, 0.60], [0.50, 0.80]),
        }
    }
}

impl std::fmt::Display for StyleId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TraitRanges {
    pub aggression: [f32; 2],
    pub fear: [f32; 2],
}

/// How much a style likes each goal; 1 is the balanced style's like for fighting, chasing, backing off, collecting
/// and wandering. Holding spots, waiting for items and laying traps are rarer for every style.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GoalAffinity {
    pub engage: f32,
    pub hunt: f32,
    pub retreat: f32,
    pub collect: f32,
    pub roam: f32,
    /// Going to see what made a sound.
    pub investigate: f32,
    /// Holding a spot with long sightlines.
    pub camp: f32,
    /// Waiting out of the way by a chokepoint.
    pub ambush: f32,
    /// Waiting by an item about to come back.
    pub control: f32,
    /// Laying tripmines and satchels where players pass.
    pub trap: f32,
}

/// Which weapons a style favours: gun names as their classnames without `weapon_` (`crossbow`, `357`, `9mmAR`) with a
/// multiplier of how good the style finds each (1 = as good as its damage says), and how readily it throws
/// grenades, satchels and snarks.
#[derive(Clone, Debug, PartialEq)]
pub struct WeaponLikes {
    pub guns: Vec<(String, f32)>,
    pub throwables: f32,
}

/// How readily a style takes tricks, as chances: long jumps along straight stretches of the way (with the module),
/// long jumps at an enemy, gauss jumps on the way, satchels and grenades thrown from a jump. The difficulty's
/// `tricks` switch lets the last four happen at all.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrickLikes {
    pub longjump: f32,
    pub lj_attack: f32,
    pub gauss_jump: f32,
    pub satchel_jump: f32,
    pub grenade_jump: f32,
}

/// Trait ranges, goal weights, weapon likes and tricks of every style: the built-in values with
/// `config/styles/*.yaml` applied.
#[derive(Clone, Debug, PartialEq)]
pub struct StyleTable {
    traits: [TraitRanges; 5],
    goals: [GoalAffinity; 5],
    weapons: [WeaponLikes; 5],
    tricks: [TrickLikes; 5],
}

impl Default for StyleTable {
    fn default() -> Self {
        StyleTable {
            traits: StyleId::ALL.map(StyleId::trait_ranges),
            goals: StyleId::ALL.map(StyleId::goal_affinity),
            weapons: StyleId::ALL.map(StyleId::weapon_likes),
            tricks: StyleId::ALL.map(StyleId::trick_likes),
        }
    }
}

impl StyleTable {
    fn index(style: StyleId) -> usize {
        StyleId::ALL
            .iter()
            .position(|s| *s == style)
            .expect("every style is listed")
    }

    pub fn traits(&self, style: StyleId) -> TraitRanges {
        self.traits[Self::index(style)]
    }

    pub fn goals(&self, style: StyleId) -> GoalAffinity {
        self.goals[Self::index(style)]
    }

    pub fn weapons(&self, style: StyleId) -> &WeaponLikes {
        &self.weapons[Self::index(style)]
    }

    pub fn tricks(&self, style: StyleId) -> TrickLikes {
        self.tricks[Self::index(style)]
    }

    /// Takes the values a style file sets.
    pub fn apply(&mut self, f: &lb_config::styles::StyleFile) {
        let Some(style) = StyleId::parse(&f.id) else { return };
        let i = Self::index(style);
        let t = &mut self.traits[i];
        t.aggression = f.traits.aggression.unwrap_or(t.aggression);
        t.fear = f.traits.fear.unwrap_or(t.fear);
        let g = &mut self.goals[i];
        let goals = &f.goals;
        g.engage = goals.engage.unwrap_or(g.engage);
        g.hunt = goals.hunt.unwrap_or(g.hunt);
        g.retreat = goals.retreat.unwrap_or(g.retreat);
        g.collect = goals.collect.unwrap_or(g.collect);
        g.roam = goals.roam.unwrap_or(g.roam);
        g.investigate = goals.investigate.unwrap_or(g.investigate);
        g.camp = goals.camp.unwrap_or(g.camp);
        g.ambush = goals.ambush.unwrap_or(g.ambush);
        g.control = goals.control.unwrap_or(g.control);
        g.trap = goals.trap.unwrap_or(g.trap);
        let w = &mut self.weapons[i];
        if let Some(guns) = &f.weapons.guns {
            w.guns = guns.iter().map(|(n, v)| (n.clone(), *v)).collect();
        }
        w.throwables = f.weapons.throwables.unwrap_or(w.throwables);
        let k = &mut self.tricks[i];
        let tricks = &f.tricks;
        k.longjump = tricks.longjump.unwrap_or(k.longjump);
        k.lj_attack = tricks.lj_attack.unwrap_or(k.lj_attack);
        k.gauss_jump = tricks.gauss_jump.unwrap_or(k.gauss_jump);
        k.satchel_jump = tricks.satchel_jump.unwrap_or(k.satchel_jump);
        k.grenade_jump = tricks.grenade_jump.unwrap_or(k.grenade_jump);
    }
}

impl StyleId {
    /// Goal weights of the style (design §8): rushers hunt and fight and never camp, snipers hold spots with long
    /// sightlines, controllers wait for items to come back, trappers lay tripmines and satchels.
    #[rustfmt::skip]
    pub fn goal_affinity(self) -> GoalAffinity {
        let a = |engage, hunt, retreat, collect, investigate, camp, ambush, control, trap| GoalAffinity {
            engage, hunt, retreat, collect, roam: 1.0, investigate, camp, ambush, control, trap,
        };
        match self {
            StyleId::Balanced =>   a(1.0, 1.0, 1.0, 1.0, 1.0, 0.25, 0.3, 0.4, 0.4),
            StyleId::Rusher =>     a(1.2, 1.4, 0.6, 1.0, 1.3, 0.0, 0.2, 0.4, 0.3),
            StyleId::Sniper =>     a(1.0, 0.3, 1.4, 1.0, 0.6, 2.0, 1.2, 0.6, 0.5),
            StyleId::Controller => a(1.0, 1.0, 1.0, 1.3, 0.9, 0.3, 0.4, 2.0, 0.4),
            StyleId::Trapper =>    a(1.0, 0.8, 1.2, 1.0, 0.8, 0.4, 1.2, 0.6, 2.0),
        }
    }
}

impl StyleId {
    /// Tricks of the style (design §8): rushers leap at enemies, controllers gauss-jump and long jump about the map,
    /// trappers and rushers throw satchels and grenades from a jump, snipers hardly ever.
    #[rustfmt::skip]
    pub fn trick_likes(self) -> TrickLikes {
        let t = |longjump, lj_attack, gauss_jump, satchel_jump, grenade_jump| {
            TrickLikes { longjump, lj_attack, gauss_jump, satchel_jump, grenade_jump }
        };
        match self {
            StyleId::Balanced =>   t(0.8, 0.6, 0.33, 0.4, 0.5),
            StyleId::Rusher =>     t(0.8, 1.0, 0.33, 0.6, 0.7),
            StyleId::Sniper =>     t(0.8, 0.6, 0.33, 0.2, 0.3),
            StyleId::Controller => t(1.0, 0.6, 0.5, 0.4, 0.5),
            StyleId::Trapper =>    t(0.8, 0.6, 0.33, 0.7, 0.6),
        }
    }
}

impl StyleId {
    /// Weapons of the style (design §8): snipers favour the crossbow and the 357, rushers the shotgun and the MP5,
    /// controllers the big guns, trappers throw most.
    pub fn weapon_likes(self) -> WeaponLikes {
        // By name, as a style file lists them.
        let w = |guns: &[(&str, f32)], throwables: f32| {
            let mut guns: Vec<(String, f32)> = guns.iter().map(|(n, v)| (n.to_string(), *v)).collect();
            guns.sort_by(|a, b| a.0.cmp(&b.0));
            WeaponLikes { guns, throwables }
        };
        match self {
            StyleId::Balanced => w(&[], 1.0),
            StyleId::Rusher => w(
                &[
                    ("shotgun", 1.25),
                    ("9mmAR", 1.15),
                    ("egon", 1.1),
                    ("crossbow", 0.8),
                    ("357", 0.9),
                ],
                1.0,
            ),
            StyleId::Sniper => w(
                &[
                    ("crossbow", 1.35),
                    ("357", 1.25),
                    ("gauss", 1.1),
                    ("shotgun", 0.8),
                    ("egon", 0.9),
                ],
                0.8,
            ),
            StyleId::Controller => w(&[("gauss", 1.15), ("rpg", 1.1), ("egon", 1.1)], 1.0),
            StyleId::Trapper => w(&[("9mmAR", 1.1)], 1.6),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shipped_style_files_match_the_built_in_table() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/config/styles");
        let mut table = StyleTable::default();
        for id in StyleId::ALL {
            let path = format!("{dir}/{}.yaml", id.as_str());
            let text = std::fs::read_to_string(&path).expect("every style has a file");
            let f = lb_config::styles::StyleFile::parse(&text, &path).unwrap();
            assert_eq!(f.id, id.as_str());
            table.apply(&f);
        }
        assert_eq!(table, StyleTable::default());
    }

    #[test]
    fn ids_match_the_config_schema() {
        let names: Vec<&str> = StyleId::ALL.iter().map(|s| s.as_str()).collect();
        assert_eq!(names, lb_config::profiles::STYLE_IDS);
        assert_eq!(StyleId::parse(" Sniper"), Some(StyleId::Sniper));
        assert_eq!(StyleId::parse("camper"), None);
    }
}
