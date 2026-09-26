//! Public server rules read from cvars (what any player can query).

#[derive(Clone, Debug, PartialEq)]
pub struct PublicRules {
    pub teamplay: bool,
    pub teamlist: Vec<String>,
    pub friendly_fire: bool,
    pub weaponstay: bool,
    pub footsteps: bool,
    pub falldamage_progressive: bool,
    /// BugfixedHL `mp_bunnyhop 1` (default) disables the speed cap.
    pub bunnyhop_uncapped: bool,
    pub maxspeed: f32,
    pub gravity: f32,
    pub timelimit_min: f32,
    pub fraglimit: f32,
}

impl Default for PublicRules {
    fn default() -> Self {
        PublicRules {
            teamplay: false,
            teamlist: Vec::new(),
            friendly_fire: false,
            weaponstay: false,
            footsteps: true,
            falldamage_progressive: false,
            bunnyhop_uncapped: false,
            maxspeed: 320.0,
            gravity: 800.0,
            timelimit_min: 0.0,
            fraglimit: 0.0,
        }
    }
}

/// Cvars polled by the runtime (missing ones keep defaults).
pub const RULE_CVARS: &[&str] = &[
    "mp_teamplay",
    "mp_teamlist",
    "mp_friendlyfire",
    "mp_weaponstay",
    "mp_footsteps",
    "mp_falldamage",
    "mp_bunnyhop",
    "sv_maxspeed",
    "sv_gravity",
    "mp_timelimit",
    "mp_fraglimit",
];

impl PublicRules {
    /// Applies a cvar value; `bhl` tells whether the BugfixedHL rules apply to `mp_bunnyhop`.
    pub fn apply_cvar(&mut self, name: &str, value: &str) {
        let f = value.trim().parse::<f32>().unwrap_or(0.0);
        match name {
            "mp_teamplay" => self.teamplay = f > 0.0,
            "mp_teamlist" => {
                self.teamlist = value
                    .split(';')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                    .collect()
            }
            "mp_friendlyfire" => self.friendly_fire = f > 0.0,
            "mp_weaponstay" => self.weaponstay = f > 0.0,
            "mp_footsteps" => self.footsteps = f > 0.0,
            "mp_falldamage" => self.falldamage_progressive = f > 0.0,
            "mp_bunnyhop" => self.bunnyhop_uncapped = f > 0.0,
            "sv_maxspeed" if f > 0.0 => self.maxspeed = f,
            "sv_gravity" if f > 0.0 => self.gravity = f,
            "mp_timelimit" => self.timelimit_min = f,
            "mp_fraglimit" => self.fraglimit = f,
            _ => {}
        }
    }
}
