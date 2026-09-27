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

/// How much a style likes each goal; 1 is the balanced style.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GoalAffinity {
    pub engage: f32,
    pub hunt: f32,
    pub retreat: f32,
    pub collect: f32,
    pub roam: f32,
}

/// Trait ranges and goal weights of every style: the built-in values with `config/styles/*.yaml` applied.
#[derive(Clone, Debug, PartialEq)]
pub struct StyleTable {
    traits: [TraitRanges; 5],
    goals: [GoalAffinity; 5],
}

impl Default for StyleTable {
    fn default() -> Self {
        StyleTable {
            traits: StyleId::ALL.map(StyleId::trait_ranges),
            goals: StyleId::ALL.map(StyleId::goal_affinity),
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
    }
}

impl StyleId {
    /// Goal weights of the style (design §8): rushers hunt and fight, snipers hold back, controllers collect.
    pub fn goal_affinity(self) -> GoalAffinity {
        let a = |engage, hunt, retreat, collect| GoalAffinity {
            engage,
            hunt,
            retreat,
            collect,
            roam: 1.0,
        };
        match self {
            StyleId::Balanced => a(1.0, 1.0, 1.0, 1.0),
            StyleId::Rusher => a(1.2, 1.4, 0.6, 1.0),
            StyleId::Sniper => a(1.0, 0.3, 1.4, 1.0),
            StyleId::Controller => a(1.0, 1.0, 1.0, 1.3),
            StyleId::Trapper => a(1.0, 0.8, 1.2, 1.0),
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
