//! `config/styles/<style>.yaml`: a play style's trait ranges and goal weights. Anything a file leaves out keeps
//! the style's built-in value.

use serde::{Deserialize, Serialize};

use crate::ConfigError;
use crate::profiles::STYLE_IDS;
use crate::yaml;

pub const KIND: &str = "style";
pub const MAJOR: u32 = 1;

/// Ranges new personalities of the style draw their base aggression and fear from.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct StyleTraits {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aggression: Option<[f32; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fear: Option<[f32; 2]>,
}

/// Multipliers of goal weights; 1 is the balanced style.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct StyleGoals {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub engage: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hunt: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retreat: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collect: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub roam: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub investigate: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub camp: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ambush: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub control: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trap: Option<f32>,
}

/// Weapons the style favours: gun multipliers by classname without `weapon_` (replacing the built-in list when
/// given), and how readily it throws.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct StyleWeapons {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guns: Option<std::collections::BTreeMap<String, f32>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub throwables: Option<f32>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct StyleFile {
    pub schema: String,
    /// One of the style ids (`balanced`, `rusher`, ...).
    pub id: String,
    #[serde(default)]
    pub traits: StyleTraits,
    #[serde(default)]
    pub goals: StyleGoals,
    #[serde(default)]
    pub weapons: StyleWeapons,
}

impl StyleFile {
    pub fn parse(text: &str, path: &str) -> Result<StyleFile, ConfigError> {
        let f: StyleFile = yaml::from_str(text, path)?;
        yaml::check_schema(&f.schema, KIND, MAJOR, path)?;
        let bad = |field: &str, message: String| ConfigError::Invalid {
            path: path.to_string(),
            field: field.to_string(),
            message,
        };
        if !STYLE_IDS.contains(&f.id.as_str()) {
            return Err(bad(
                "id",
                format!("unknown style `{}`; expected one of {STYLE_IDS:?}", f.id),
            ));
        }
        for (name, range) in [
            ("traits.aggression", f.traits.aggression),
            ("traits.fear", f.traits.fear),
        ] {
            if let Some([lo, hi]) = range
                && !(0.0 <= lo && lo <= hi && hi <= 1.0)
            {
                return Err(bad(name, "must be [min, max] with 0 <= min <= max <= 1".into()));
            }
        }
        let g = &f.goals;
        for (name, v) in [
            ("goals.engage", g.engage),
            ("goals.hunt", g.hunt),
            ("goals.retreat", g.retreat),
            ("goals.collect", g.collect),
            ("goals.roam", g.roam),
            ("goals.investigate", g.investigate),
            ("goals.camp", g.camp),
            ("goals.ambush", g.ambush),
            ("goals.control", g.control),
            ("goals.trap", g.trap),
        ] {
            if v.is_some_and(|v| !(0.0..=10.0).contains(&v)) {
                return Err(bad(name, "must be in 0..=10".into()));
            }
        }
        let guns = f
            .weapons
            .guns
            .iter()
            .flatten()
            .map(|(n, v)| (format!("weapons.guns.{n}"), Some(*v)));
        for (name, v) in guns.chain([("weapons.throwables".to_string(), f.weapons.throwables)]) {
            if v.is_some_and(|v| !(0.0..=10.0).contains(&v)) {
                return Err(bad(&name, "must be in 0..=10".into()));
            }
        }
        Ok(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_files_and_errors() {
        let f = StyleFile::parse(
            "schema: lambdabots/style@1\nid: rusher\ngoals:\n  hunt: 1.6\n",
            "rusher.yaml",
        )
        .unwrap();
        assert_eq!(f.goals.hunt, Some(1.6));
        assert_eq!(f.traits, StyleTraits::default());
        assert!(StyleFile::parse("schema: lambdabots/style@1\nid: camper\n", "x").is_err());
        assert!(
            StyleFile::parse(
                "schema: lambdabots/style@1\nid: sniper\ntraits:\n  fear: [0.9, 0.2]\n",
                "x"
            )
            .is_err()
        );
        assert!(StyleFile::parse("schema: lambdabots/style@1\nid: sniper\ngoals:\n  sniping: 2\n", "x").is_err());
        assert!(StyleFile::parse("schema: lambdabots/style@1\nid: sniper\ngoals:\n  camp: 11\n", "x").is_err());
    }
}
