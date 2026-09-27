//! Resolved personalities. Whatever a profile leaves out is derived from the personality's seed (by default a hash
//! of the nickname), so a nickname resolves to the same bot in every session and on every server.

use std::path::PathBuf;

use lb_config::profiles::{PersonaSpec, TraitsSpec};
use lb_config::skill::{Presets, SkillBand, SkillOverrides, SkillParams, SkillValue};
use lb_core::rng::{Pcg32, fnv1a64, splitmix64};

use crate::style::{StyleId, StyleTable};

/// Stream of the draws that fill a profile's gaps. Fixed: changing it would change every derived personality.
const FILL_STREAM: u64 = 0x7065_7273;
pub const DEFAULT_SKILL: u8 = 50;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PersonaSource {
    /// Written by the admin in `profiles/*.yaml`.
    Profile(PathBuf),
    /// Created by the server in `data/profiles.yaml`.
    Generated(PathBuf),
    /// Only in memory (the server could not save it).
    Unsaved,
}

impl PersonaSource {
    pub fn describe(&self) -> String {
        match self {
            PersonaSource::Profile(p) => format!("profile {}", p.display()),
            PersonaSource::Generated(p) => format!("generated, {}", p.display()),
            PersonaSource::Unsaved => "generated, not saved".into(),
        }
    }

    pub fn short(&self) -> &'static str {
        match self {
            PersonaSource::Profile(_) => "profile",
            PersonaSource::Generated(_) => "generated",
            PersonaSource::Unsaved => "unsaved",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Persona {
    pub name: String,
    pub style: StyleId,
    pub skill: u8,
    pub model: String,
    pub colors: [u8; 2],
    pub aggression: f32,
    pub fear: f32,
    pub weapons: Vec<String>,
    pub weight: f32,
    pub tags: Vec<String>,
    pub seed: u64,
    pub overrides: SkillOverrides,
    pub source: PersonaSource,
}

/// Default seed of a nickname (case-insensitive).
pub fn name_seed(name: &str) -> u64 {
    splitmix64(fnv1a64(name.to_lowercase().as_bytes()))
}

impl Persona {
    /// `models` are the server's model choices for profiles without one (`bots.models`); `styles` gives the trait
    /// ranges traits are drawn from when a profile leaves them out.
    pub fn resolve(spec: &PersonaSpec, source: PersonaSource, models: &[String], styles: &StyleTable) -> Persona {
        let seed = spec.seed.unwrap_or_else(|| name_seed(&spec.name));
        let style = spec
            .style
            .as_deref()
            .and_then(StyleId::parse)
            .unwrap_or(StyleId::Balanced);
        let ranges = styles.traits(style);
        // Every draw happens in a fixed order whether or not the profile sets the field, so filling in one field
        // later never changes the others.
        let mut rng = Pcg32::new(seed, FILL_STREAM);
        let aggression = rng.range_f32(ranges.aggression[0], ranges.aggression[1]);
        let fear = rng.range_f32(ranges.fear[0], ranges.fear[1]);
        let model_index = rng.range_i32(0, models.len().max(1) as i32 - 1) as usize;
        let top = rng.range_i32(0, 255) as u8;
        let bottom = rng.range_i32(0, 255) as u8;
        let traits = spec.traits.unwrap_or_default();
        Persona {
            name: spec.name.clone(),
            style,
            skill: spec
                .skill
                .as_ref()
                .and_then(SkillValue::resolve)
                .unwrap_or(DEFAULT_SKILL),
            model: spec
                .model
                .clone()
                .or_else(|| models.get(model_index).cloned())
                .unwrap_or_else(|| "gordon".into()),
            colors: spec.colors.unwrap_or([top, bottom]),
            aggression: traits.aggression.unwrap_or(aggression),
            fear: traits.fear.unwrap_or(fear),
            weapons: spec.weapons.clone(),
            weight: spec.weight.unwrap_or(1.0),
            tags: spec.tags.clone(),
            seed,
            overrides: spec.overrides.clone(),
            source,
        }
    }

    pub fn skill_params(&self, presets: &Presets) -> SkillParams {
        let mut params = presets.at(self.skill);
        params.apply(&self.overrides);
        params
    }

    /// The spec that reproduces this personality with every field written out (for `data/profiles.yaml`).
    pub fn to_spec(&self, created: Option<String>) -> PersonaSpec {
        PersonaSpec {
            name: self.name.clone(),
            style: Some(self.style.as_str().into()),
            skill: Some(SkillValue::Number(f32::from(self.skill))),
            model: Some(self.model.clone()),
            colors: Some(self.colors),
            traits: Some(TraitsSpec {
                aggression: Some(round2(self.aggression)),
                fear: Some(round2(self.fear)),
            }),
            weapons: self.weapons.clone(),
            weight: (self.weight != 1.0).then_some(self.weight),
            tags: self.tags.clone(),
            seed: Some(self.seed),
            created,
            overrides: self.overrides.clone(),
        }
    }
}

fn round2(v: f32) -> f32 {
    (v * 100.0).round() / 100.0
}

/// A new personality for `name`: style by `style_weights`, skill inside `band` (more often near its middle), the
/// rest from the nickname's seed. `rng` is the server's own stream, so the style and skill of a new nickname are
/// not predictable from the name.
pub fn generate(
    name: &str,
    style_weights: &[(StyleId, f32)],
    band: SkillBand,
    models: &[String],
    styles: &StyleTable,
    rng: &mut Pcg32,
) -> Persona {
    let total: f32 = style_weights.iter().map(|(_, w)| w.max(0.0)).sum();
    let mut pick = rng.next_f32() * total;
    let mut style = StyleId::Balanced;
    for (id, w) in style_weights {
        if pick < w.max(0.0) {
            style = *id;
            break;
        }
        pick -= w.max(0.0);
    }
    let span = f32::from(band.max - band.min);
    let skill = band.min + ((rng.next_f32() + rng.next_f32()) * 0.5 * span).round() as u8;
    let spec = PersonaSpec {
        name: name.to_string(),
        style: Some(style.as_str().into()),
        skill: Some(SkillValue::Number(f32::from(skill.min(band.max)))),
        ..Default::default()
    };
    Persona::resolve(&spec, PersonaSource::Unsaved, models, styles)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn models() -> Vec<String> {
        ["gordon", "barney", "gina"].into_iter().map(String::from).collect()
    }

    #[test]
    fn missing_fields_come_from_the_nickname() {
        let spec = PersonaSpec {
            name: "Kleiner".into(),
            style: Some("sniper".into()),
            ..Default::default()
        };
        let a = Persona::resolve(&spec, PersonaSource::Unsaved, &models(), &StyleTable::default());
        let b = Persona::resolve(&spec, PersonaSource::Unsaved, &models(), &StyleTable::default());
        assert_eq!(a, b, "the same nickname resolves identically every time");
        assert_eq!(a.skill, DEFAULT_SKILL);
        let r = StyleId::Sniper.trait_ranges();
        assert!((r.aggression[0]..=r.aggression[1]).contains(&a.aggression));
        assert!((r.fear[0]..=r.fear[1]).contains(&a.fear));
        let other = Persona::resolve(
            &PersonaSpec {
                name: "Barney".into(),
                ..spec.clone()
            },
            PersonaSource::Unsaved,
            &models(),
            &StyleTable::default(),
        );
        assert_ne!(a.seed, other.seed);
        let upper = Persona::resolve(
            &PersonaSpec {
                name: "KLEINER".into(),
                ..spec
            },
            PersonaSource::Unsaved,
            &models(),
            &StyleTable::default(),
        );
        assert_eq!(upper.seed, a.seed, "seeds ignore case like name matching does");
    }

    #[test]
    fn setting_one_field_does_not_move_the_others() {
        let base = PersonaSpec {
            name: "Gina".into(),
            ..Default::default()
        };
        let a = Persona::resolve(&base, PersonaSource::Unsaved, &models(), &StyleTable::default());
        let with_colors = PersonaSpec {
            colors: Some([1, 2]),
            ..base
        };
        let b = Persona::resolve(&with_colors, PersonaSource::Unsaved, &models(), &StyleTable::default());
        assert_eq!((a.aggression, a.fear, &a.model), (b.aggression, b.fear, &b.model));
        assert_eq!(b.colors, [1, 2]);
    }

    #[test]
    fn written_out_spec_reproduces_the_personality() {
        let mut rng = Pcg32::new(7, 1);
        let styles = [(StyleId::Rusher, 1.0), (StyleId::Sniper, 1.0)];
        let p = generate(
            "-|NoS|-",
            &styles,
            SkillBand { min: 38, max: 62 },
            &models(),
            &StyleTable::default(),
            &mut rng,
        );
        assert!((38..=62).contains(&p.skill));
        assert!(matches!(p.style, StyleId::Rusher | StyleId::Sniper));
        let again = Persona::resolve(
            &p.to_spec(None),
            PersonaSource::Unsaved,
            &models(),
            &StyleTable::default(),
        );
        assert_eq!(
            (again.style, again.skill, again.colors, &again.model),
            (p.style, p.skill, p.colors, &p.model)
        );
        assert!((again.aggression - p.aggression).abs() < 0.006);
    }

    #[test]
    fn generated_skill_prefers_the_middle_of_the_band() {
        let mut rng = Pcg32::new(11, 3);
        let band = SkillBand { min: 0, max: 100 };
        let skills: Vec<u8> = (0..2000)
            .map(|i| {
                let one = [(StyleId::Balanced, 1.0)];
                generate(
                    &format!("n{i}"),
                    &one,
                    band,
                    &models(),
                    &StyleTable::default(),
                    &mut rng,
                )
                .skill
            })
            .collect();
        let middle = skills.iter().filter(|s| (25..=75).contains(*s)).count();
        assert!(
            middle > 1300,
            "triangular draw keeps most skills near the middle: {middle}"
        );
        assert!(skills.iter().all(|s| *s <= 100));
    }

    #[test]
    fn skill_params_apply_overrides() {
        let spec = PersonaSpec {
            name: "x".into(),
            skill: Some(SkillValue::Name("hard".into())),
            overrides: SkillOverrides {
                turn_speed: Some(123.0),
                ..Default::default()
            },
            ..Default::default()
        };
        let p = Persona::resolve(&spec, PersonaSource::Unsaved, &models(), &StyleTable::default());
        let params = p.skill_params(&Presets::default());
        assert_eq!(params.turn_speed, 123.0);
        assert_eq!(params.headshot, Presets::default().hard.headshot);
    }
}
