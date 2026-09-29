//! Bot skill: the 0–100 scale, the parameters it controls, `config/difficulty.yaml` and roster skill filters.
//!
//! Five presets sit at fixed points of the scale (noob 0, easy 25, normal 50, hard 75, expert 100). A skill between
//! two presets interpolates numeric parameters linearly; switches (aim model, tricks, optional abilities) keep the
//! value of the lower preset until the next preset is reached.

use serde::{Deserialize, Serialize};

use crate::ConfigError;
use crate::yaml;

pub const KIND: &str = "difficulty";
pub const MAJOR: u32 = 1;

pub const PRESET_NAMES: [&str; 5] = ["noob", "easy", "normal", "hard", "expert"];
pub const PRESET_STEP: u8 = 25;

/// A skill value written as a number (`62`) or a preset name (`hard`).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(untagged)]
pub enum SkillValue {
    Number(f32),
    Name(String),
}

impl SkillValue {
    pub fn resolve(&self) -> Option<u8> {
        match self {
            SkillValue::Number(v) if v.is_finite() && (0.0..=100.0).contains(v) => Some(v.round() as u8),
            SkillValue::Number(_) => None,
            SkillValue::Name(n) => parse_skill(n),
        }
    }
}

/// `noob|easy|normal|hard|expert` or a number 0..=100.
pub fn parse_skill(s: &str) -> Option<u8> {
    let s = s.trim().to_ascii_lowercase();
    if let Some(i) = PRESET_NAMES.iter().position(|p| *p == s) {
        return Some(i as u8 * PRESET_STEP);
    }
    s.parse::<f32>()
        .ok()
        .filter(|v| (0.0..=100.0).contains(v))
        .map(|v| v.round() as u8)
}

/// Inclusive skill range used to filter which personalities may join (`lb_difficulty`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SkillBand {
    pub min: u8,
    pub max: u8,
}

impl SkillBand {
    pub const ANY: SkillBand = SkillBand { min: 0, max: 100 };
    /// Half-width of the band a single value stands for (`hard` = 63..87, `60` = 48..72).
    const HALF: u8 = 12;

    /// `any`, a preset or a number (a band around it), or a range `a-b` whose ends are presets or numbers
    /// (`normal-hard` = 38..87, `40-70` = 40..70).
    pub fn parse(s: &str) -> Option<SkillBand> {
        let s = s.trim().to_ascii_lowercase();
        if s == "any" || s.is_empty() {
            return Some(SkillBand::ANY);
        }
        if let Some((a, b)) = s.split_once('-') {
            let lo = Self::end(a.trim(), true)?;
            let hi = Self::end(b.trim(), false)?;
            return (lo <= hi).then_some(SkillBand { min: lo, max: hi });
        }
        let v = parse_skill(&s)?;
        Some(SkillBand {
            min: v.saturating_sub(Self::HALF),
            max: v.saturating_add(Self::HALF).min(100),
        })
    }

    fn end(s: &str, lower: bool) -> Option<u8> {
        if PRESET_NAMES.contains(&s) {
            let v = parse_skill(s)?;
            return Some(if lower {
                v.saturating_sub(Self::HALF)
            } else {
                v.saturating_add(Self::HALF).min(100)
            });
        }
        parse_skill(s)
    }

    pub fn contains(&self, skill: u8) -> bool {
        (self.min..=self.max).contains(&skill)
    }
}

impl std::fmt::Display for SkillBand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}..{}", self.min, self.max)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AimModel {
    /// Fixed-rate turning with overshoot, like a beginner with a mouse.
    Newbie,
    /// Damped spring toward the target.
    Spring,
    /// Stiffer spring in combat.
    SpringCombat,
}

/// Values mixed between two presets; switches keep the lower preset's value.
pub trait Blend: Sized + Clone {
    fn blend(lo: &Self, hi: &Self, t: f32) -> Self;
}

impl Blend for f32 {
    fn blend(lo: &Self, hi: &Self, t: f32) -> Self {
        lo + (hi - lo) * t
    }
}

impl<const N: usize> Blend for [f32; N] {
    fn blend(lo: &Self, hi: &Self, t: f32) -> Self {
        std::array::from_fn(|i| lo[i] + (hi[i] - lo[i]) * t)
    }
}

impl Blend for Option<f32> {
    fn blend(lo: &Self, hi: &Self, t: f32) -> Self {
        match (lo, hi) {
            (Some(a), Some(b)) => Some(a + (b - a) * t),
            _ => *lo,
        }
    }
}

impl Blend for bool {
    fn blend(lo: &Self, _hi: &Self, _t: f32) -> Self {
        *lo
    }
}

impl Blend for AimModel {
    fn blend(lo: &Self, _hi: &Self, _t: f32) -> Self {
        *lo
    }
}

macro_rules! skill_params {
    ($( $(#[doc = $doc:literal])* $field:ident : $ty:ty ),* $(,)?) => {
        /// Parameters a skill level resolves to.
        #[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
        #[serde(deny_unknown_fields)]
        pub struct SkillParams {
            $( $(#[doc = $doc])* pub $field: $ty, )*
        }

        /// Per-personality corrections applied on top of the interpolated parameters.
        #[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
        #[serde(deny_unknown_fields, default)]
        pub struct SkillOverrides {
            $( #[serde(skip_serializing_if = "Option::is_none")] pub $field: Option<$ty>, )*
        }

        impl SkillParams {
            pub fn blend(lo: &SkillParams, hi: &SkillParams, t: f32) -> SkillParams {
                SkillParams { $( $field: <$ty as Blend>::blend(&lo.$field, &hi.$field, t), )* }
            }

            pub fn apply(&mut self, o: &SkillOverrides) {
                $( if let Some(v) = &o.$field { self.$field = v.clone(); } )*
            }
        }

        impl SkillOverrides {
            pub fn is_empty(&self) -> bool {
                true $( && self.$field.is_none() )*
            }
        }
    };
}

skill_params! {
    /// Seconds from first sight to recognizing an enemy, drawn once per contact from [min, max].
    recognition_delay: [f32; 2],
    /// Recognition speed at the edge of the view relative to its center, 0..1.
    peripheral_gain: f32,
    /// Seconds to recognize again an enemy lost less than `reacquire_grace` seconds ago.
    reacquire_delay: f32,
    /// Seconds after losing sight during which an enemy is recognized again quickly.
    reacquire_grace: f32,
    /// Simulated view latency of the aim, seconds.
    aim_latency: f32,
    aim_model: AimModel,
    /// Chance to aim at the head for a contact, 0..1.
    headshot: f32,
    /// Aim error amplitude along x, y, z in units.
    aim_error: [f32; 3],
    /// Maximum turn rate, degrees per second.
    turn_speed: f32,
    /// Extra pause between semi-automatic shots, seconds [min, max].
    semi_auto_delay: [f32; 2],
    /// Chance to stand still while fighting at 768–1024 units, 0..1.
    stay_mid: f32,
    /// Chance to stand still while fighting beyond 1024 units, 0..1.
    stay_far: f32,
    /// Chance of a crouch tap while strafing, 0..1.
    crouch_tap: f32,
    /// Cooldown of dodge jumps in seconds; none = never dodge-jumps.
    dodge_hop_cooldown: Option<f32>,
    /// Quietest sound gain the bot notices, 0..1.
    hearing_threshold: f32,
    /// Bearing error of a heard sound, degrees.
    sound_bearing_sigma: f32,
    /// Seconds a lost enemy stays in memory.
    track_forget: f32,
    /// Share of gauss charge held before an expected fight, 0..1.
    gauss_precharge: f32,
    /// Share of gauss shots fired charged rather than with the plain primary attack, 0..1.
    gauss_charge: f32,
    /// Seconds from putting the crossbow's scope on to the shot, [min, max].
    scope_settle: [f32; 2],
    /// How readily grenades, satchels and snarks are thrown, 1 = as the normal preset.
    throw_rate: f32,
    /// Grenades are thrown one after another until none is left, at the enemy or where one is expected.
    throw_series: bool,
    /// Gauss jump, satchel jump and attacking long jumps are allowed.
    tricks: bool,
    /// How readily long jumps are taken, 0..1, times the style's liking: the share of the time the bot long jumps
    /// along its way, and the chance it takes one in a fight when one fits.
    longjump: f32,
    /// Long jumps along the way round corners, down drops, over short stretches and one after another, and onto a
    /// hard landing with health to spare.
    longjump_bold: bool,
    /// Long jumps aside or away in a fight (for a dodge jump, away from a blast), the view back on the enemy in the
    /// air.
    longjump_dodge: bool,
    /// Charged gauss shots through thin walls at an enemy lost behind one a moment ago.
    gauss_walls: bool,
    /// Bunny hop speed limit as a multiple of maxspeed; none = no bunny hopping.
    bhop_speed: Option<f32>,
}

/// `config/difficulty.yaml`: parameters at the five presets. A preset may leave parameters out; they keep their
/// built-in values, so files written for an older version still load.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DifficultyFile {
    pub schema: String,
    pub presets: Presets,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(from = "PresetsFile")]
pub struct Presets {
    pub noob: SkillParams,
    pub easy: SkillParams,
    pub normal: SkillParams,
    pub hard: SkillParams,
    pub expert: SkillParams,
}

/// Presets as written in the file: every parameter is optional.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PresetsFile {
    #[serde(default)]
    noob: SkillOverrides,
    #[serde(default)]
    easy: SkillOverrides,
    #[serde(default)]
    normal: SkillOverrides,
    #[serde(default)]
    hard: SkillOverrides,
    #[serde(default)]
    expert: SkillOverrides,
}

impl From<PresetsFile> for Presets {
    fn from(f: PresetsFile) -> Presets {
        let mut p = Presets::default();
        p.noob.apply(&f.noob);
        p.easy.apply(&f.easy);
        p.normal.apply(&f.normal);
        p.hard.apply(&f.hard);
        p.expert.apply(&f.expert);
        p
    }
}

impl Presets {
    fn ordered(&self) -> [&SkillParams; 5] {
        [&self.noob, &self.easy, &self.normal, &self.hard, &self.expert]
    }

    /// Parameters for a skill value 0..=100.
    pub fn at(&self, skill: u8) -> SkillParams {
        let skill = skill.min(100);
        let p = self.ordered();
        let i = (skill / PRESET_STEP) as usize;
        let rest = skill % PRESET_STEP;
        if rest == 0 {
            return p[i].clone();
        }
        SkillParams::blend(p[i], p[i + 1], f32::from(rest) / f32::from(PRESET_STEP))
    }
}

impl Default for Presets {
    /// The table from the project plan (yapb difficulty.cfg, combat.cpp, jk_botti latency).
    fn default() -> Self {
        let p = |recognition: [f32; 2],
                 vision: [f32; 3],
                 aim_latency: f32,
                 aim_model: AimModel,
                 headshot: f32,
                 aim_error: [f32; 3],
                 turn_speed: f32,
                 semi_auto_delay: [f32; 2],
                 stay: [f32; 2],
                 crouch_tap: f32,
                 dodge_hop_cooldown: Option<f32>,
                 hearing: [f32; 2],
                 track_forget: f32,
                 gauss_precharge: f32,
                 tricks: bool,
                 bhop_speed: Option<f32>| SkillParams {
            recognition_delay: recognition,
            peripheral_gain: vision[0],
            reacquire_delay: vision[1],
            reacquire_grace: vision[2],
            aim_latency,
            aim_model,
            headshot,
            aim_error,
            turn_speed,
            semi_auto_delay,
            stay_mid: stay[0],
            stay_far: stay[1],
            crouch_tap,
            dodge_hop_cooldown,
            hearing_threshold: hearing[0],
            sound_bearing_sigma: hearing[1],
            track_forget,
            gauss_precharge,
            gauss_charge: 0.0,
            scope_settle: [0.0, 0.0],
            throw_rate: 1.0,
            throw_series: false,
            tricks,
            longjump: 0.0,
            longjump_bold: false,
            longjump_dodge: false,
            gauss_walls: false,
            bhop_speed,
        };
        use AimModel::*;
        let mut presets = Presets {
            noob: p(
                [1.5, 2.0],
                [0.35, 0.35, 1.0],
                0.30,
                Newbie,
                0.15,
                [20.0, 20.0, 40.0],
                180.0,
                [0.7, 0.8],
                [0.60, 0.85],
                0.0,
                None,
                [0.07, 35.0],
                4.0,
                0.0,
                false,
                None,
            ),
            easy: p(
                [1.0, 1.5],
                [0.40, 0.25, 1.5],
                0.24,
                Spring,
                0.20,
                [15.0, 15.0, 30.0],
                300.0,
                [0.5, 0.6],
                [0.40, 0.70],
                0.0,
                Some(5.25),
                [0.055, 28.0],
                6.0,
                0.5,
                false,
                None,
            ),
            normal: p(
                [0.5, 1.0],
                [0.45, 0.15, 2.0],
                0.18,
                Spring,
                0.25,
                [10.0, 10.0, 20.0],
                450.0,
                [0.4, 0.5],
                [0.20, 0.45],
                0.04,
                Some(4.0),
                [0.04, 20.0],
                8.0,
                0.6,
                true,
                Some(1.25),
            ),
            hard: p(
                [0.25, 0.5],
                [0.50, 0.10, 2.5],
                0.12,
                SpringCombat,
                0.50,
                [5.0, 5.0, 10.0],
                650.0,
                [0.3, 0.4],
                [0.08, 0.20],
                0.06,
                Some(2.75),
                [0.03, 14.0],
                10.0,
                0.7,
                true,
                Some(1.5),
            ),
            expert: p(
                [0.1, 0.25],
                [0.55, 0.05, 3.0],
                0.06,
                SpringCombat,
                0.75,
                [1.5, 1.5, 3.0],
                900.0,
                [0.1, 0.2],
                [0.0, 0.10],
                0.08,
                Some(1.5),
                [0.02, 10.0],
                12.0,
                0.8,
                true,
                Some(1.7),
            ),
        };
        // (gauss_charge, scope_settle, throw_rate, longjump): skilled players fight the gauss charged, snap the
        // crossbow's scope on only for the shot, and get about by long jumps whenever they have the module.
        let extra = [
            (0.2, [1.0, 1.4], 0.5, 0.15),
            (0.45, [0.6, 0.9], 0.75, 0.35),
            (0.75, [0.35, 0.55], 1.0, 0.6),
            (0.85, [0.2, 0.3], 1.2, 0.9),
            (0.9, [0.1, 0.15], 1.4, 1.0),
        ];
        let all = [
            &mut presets.noob,
            &mut presets.easy,
            &mut presets.normal,
            &mut presets.hard,
            &mut presets.expert,
        ];
        for (params, (charge, settle, throws, longjump)) in all.into_iter().zip(extra) {
            params.gauss_charge = charge;
            params.scope_settle = settle;
            params.throw_rate = throws;
            params.longjump = longjump;
        }
        for p in [&mut presets.hard, &mut presets.expert] {
            p.gauss_walls = true;
            p.longjump_bold = true;
            p.longjump_dodge = true;
            p.throw_series = true;
        }
        presets
    }
}

impl DifficultyFile {
    pub fn parse(text: &str, path: &str) -> Result<DifficultyFile, ConfigError> {
        let f: DifficultyFile = yaml::from_str(text, path)?;
        yaml::check_schema(&f.schema, KIND, MAJOR, path)?;
        for (name, p) in PRESET_NAMES.iter().zip(f.presets.ordered()) {
            validate_params(p, &format!("presets.{name}"), path)?;
        }
        Ok(f)
    }
}

pub fn validate_params(p: &SkillParams, at: &str, path: &str) -> Result<(), ConfigError> {
    let bad = |field: &str, message: &str| ConfigError::Invalid {
        path: path.to_string(),
        field: format!("{at}.{field}"),
        message: message.to_string(),
    };
    let unit = |v: f32| (0.0..=1.0).contains(&v);
    let range = |r: [f32; 2]| r[0] >= 0.0 && r[0] <= r[1];
    if !range(p.recognition_delay) {
        return Err(bad("recognition_delay", "must be [min, max] with 0 <= min <= max"));
    }
    if !range(p.semi_auto_delay) {
        return Err(bad("semi_auto_delay", "must be [min, max] with 0 <= min <= max"));
    }
    if !range(p.scope_settle) {
        return Err(bad("scope_settle", "must be [min, max] with 0 <= min <= max"));
    }
    if !(0.0..=10.0).contains(&p.throw_rate) {
        return Err(bad("throw_rate", "must be in 0..=10"));
    }
    for (name, v) in [
        ("headshot", p.headshot),
        ("stay_mid", p.stay_mid),
        ("stay_far", p.stay_far),
        ("crouch_tap", p.crouch_tap),
        ("hearing_threshold", p.hearing_threshold),
        ("peripheral_gain", p.peripheral_gain),
        ("gauss_precharge", p.gauss_precharge),
        ("gauss_charge", p.gauss_charge),
        ("longjump", p.longjump),
    ] {
        if !unit(v) {
            return Err(bad(name, "must be in 0..=1"));
        }
    }
    if p.turn_speed <= 0.0 || p.aim_latency < 0.0 || p.track_forget <= 0.0 || p.sound_bearing_sigma < 0.0 {
        return Err(bad(
            "turn_speed/aim_latency/track_forget/sound_bearing_sigma",
            "must be positive",
        ));
    }
    if p.reacquire_delay < 0.0 || p.reacquire_grace < 0.0 {
        return Err(bad("reacquire_delay/reacquire_grace", "must not be negative"));
    }
    if p.aim_error.iter().any(|v| *v < 0.0) {
        return Err(bad("aim_error", "must not be negative"));
    }
    Ok(())
}

impl Default for DifficultyFile {
    fn default() -> Self {
        DifficultyFile {
            schema: format!("lambdabots/{KIND}@{MAJOR}"),
            presets: Presets::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_values_and_presets() {
        assert_eq!(parse_skill("hard"), Some(75));
        assert_eq!(parse_skill(" Normal "), Some(50));
        assert_eq!(parse_skill("62"), Some(62));
        assert_eq!(parse_skill("101"), None);
        assert_eq!(SkillValue::Number(61.6).resolve(), Some(62));
        assert_eq!(SkillValue::Name("expert".into()).resolve(), Some(100));
    }

    #[test]
    fn bands() {
        assert_eq!(SkillBand::parse("any"), Some(SkillBand::ANY));
        assert_eq!(SkillBand::parse("hard"), Some(SkillBand { min: 63, max: 87 }));
        assert_eq!(SkillBand::parse("normal-hard"), Some(SkillBand { min: 38, max: 87 }));
        assert_eq!(SkillBand::parse("40-70"), Some(SkillBand { min: 40, max: 70 }));
        assert_eq!(SkillBand::parse("60"), Some(SkillBand { min: 48, max: 72 }));
        assert_eq!(SkillBand::parse("noob"), Some(SkillBand { min: 0, max: 12 }));
        assert_eq!(SkillBand::parse("expert"), Some(SkillBand { min: 88, max: 100 }));
        assert_eq!(SkillBand::parse("hard-normal"), None);
        assert_eq!(SkillBand::parse("fast"), None);
    }

    #[test]
    fn presets_are_hit_exactly_and_mixed_between() {
        let p = Presets::default();
        assert_eq!(p.at(50), p.normal);
        assert_eq!(p.at(100), p.expert);
        assert_eq!(p.at(0), p.noob);
        let mid = p.at(62);
        assert!(mid.turn_speed > p.normal.turn_speed && mid.turn_speed < p.hard.turn_speed);
        assert_eq!(mid.aim_model, AimModel::Spring, "switches keep the lower preset");
        assert_eq!(p.at(10).dodge_hop_cooldown, None, "no dodge jumps until easy");
        assert!(!p.at(30).tricks);
        assert!(p.at(50).tricks);
        assert!(p.at(62).longjump > p.normal.longjump && p.at(62).longjump < p.hard.longjump);
        assert!(!p.at(74).longjump_bold && p.at(75).longjump_bold && p.at(100).longjump_dodge);
    }

    #[test]
    fn overrides_replace_single_parameters() {
        let mut params = Presets::default().at(62);
        let o = SkillOverrides {
            turn_speed: Some(520.0),
            recognition_delay: Some([0.35, 0.5]),
            ..Default::default()
        };
        params.apply(&o);
        assert_eq!(params.turn_speed, 520.0);
        assert_eq!(params.recognition_delay, [0.35, 0.5]);
        assert!(!o.is_empty());
    }

    #[test]
    fn shipped_table_matches_the_built_in_one() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/config/difficulty.yaml");
        let text = std::fs::read_to_string(path).expect("data/config/difficulty.yaml");
        assert_eq!(DifficultyFile::parse(&text, path).unwrap(), DifficultyFile::default());
    }

    #[test]
    fn presets_may_leave_parameters_out() {
        let text = "schema: lambdabots/difficulty@1\npresets:\n  hard:\n    turn_speed: 700\n";
        let f = DifficultyFile::parse(text, "partial").unwrap();
        assert_eq!(f.presets.hard.turn_speed, 700.0);
        assert_eq!(f.presets.hard.headshot, Presets::default().hard.headshot);
        assert_eq!(f.presets.noob, Presets::default().noob);
        let typo = "schema: lambdabots/difficulty@1\npresets:\n  hard:\n    turn_sped: 1\n";
        assert!(DifficultyFile::parse(typo, "typo").is_err());
    }

    #[test]
    fn default_table_roundtrips_through_yaml() {
        let text = yaml::to_string(&DifficultyFile::default()).unwrap();
        assert_eq!(
            DifficultyFile::parse(&text, "defaults").unwrap(),
            DifficultyFile::default()
        );
    }
}
