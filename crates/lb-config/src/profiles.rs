//! Bot personalities: `profiles/*.yaml` written by the admin and `data/profiles.yaml` written by the server.
//! A personality binds a nickname to a play style, a skill and a look, so the same nickname always plays the
//! same way.

use serde::{Deserialize, Serialize};

use crate::ConfigError;
use crate::names::sanitize_name;
use crate::skill::{SkillOverrides, SkillValue};
use crate::yaml;

pub const KIND: &str = "profiles";
pub const MAJOR: u32 = 1;

/// Play style identifiers; their behaviour lives in the AI crates.
pub const STYLE_IDS: [&str; 5] = ["balanced", "rusher", "sniper", "controller", "trapper"];

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ProfilesFile {
    pub schema: String,
    /// `bots:` with no entries is allowed (a template or a fresh server-written file).
    #[serde(default)]
    pub bots: Option<Vec<PersonaSpec>>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PersonaSpec {
    /// Nickname; unique across all profile files (case-insensitive).
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
    /// 0..=100 or a preset name (noob 0, easy 25, normal 50, hard 75, expert 100).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill: Option<SkillValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Top and bottom colour, 0..=255.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub colors: Option<[u8; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub traits: Option<TraitsSpec>,
    /// Preferred weapons, best first (classnames without `weapon_`, e.g. `crossbow`, `357`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub weapons: Vec<String>,
    /// Relative chance to be picked when a bot joins; 0 = only on request (`lb add <name>`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight: Option<f32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Seed of the bot's own random habits; defaults to a hash of the name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    /// Date the server created the personality (informational).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
    #[serde(default, skip_serializing_if = "SkillOverrides::is_empty")]
    pub overrides: SkillOverrides,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat: Option<ChatSpec>,
}

/// How the personality talks in the chat; whatever is left out comes from its seed and style.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ChatSpec {
    /// 0..1: how readily it speaks when nobody asked (0 only answers).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chattiness: Option<f32>,
    /// It may swear.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profanity: Option<bool>,
    /// Characters a minute it types.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub typing_cpm: Option<f32>,
    /// How it writes, in a few words for the model ("short, lower case, ends with ))").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
    /// Who it is, for the model ("plays here every evening, loves the crossbow").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub about: Option<String>,
    /// More names players call it by: one (`Плутон`) or several (`[Плутон, Плутоныч]`). A line with one of them
    /// speaks to the bot as one with its nickname does.
    #[serde(
        default,
        deserialize_with = "yaml::one_or_many",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub call: Vec<String>,
}

/// Longest `chat.style` / `chat.about` in characters.
pub const CHAT_TEXT_MAX: usize = 300;
/// Most names in `chat.call`.
pub const CALLS_MAX: usize = 8;
/// Longest name in `chat.call`, in characters.
pub const CALL_MAX: usize = 32;
/// Typing speeds a personality may have, characters a minute.
pub const TYPING_CPM_RANGE: [f32; 2] = [30.0, 1500.0];

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TraitsSpec {
    /// 0..1: how eagerly the bot seeks and presses fights.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aggression: Option<f32>,
    /// 0..1: how early it retreats and how much it values cover.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fear: Option<f32>,
}

impl ProfilesFile {
    pub fn parse(text: &str, path: &str) -> Result<ProfilesFile, ConfigError> {
        let f: ProfilesFile = yaml::from_str(text, path)?;
        yaml::check_schema(&f.schema, KIND, MAJOR, path)?;
        let bots = f.bots.as_deref().unwrap_or(&[]);
        for (i, spec) in bots.iter().enumerate() {
            spec.validate(&format!("bots[{i}] ({})", spec.name), path)?;
            if let Some(j) = bots[..i].iter().position(|o| o.name.eq_ignore_ascii_case(&spec.name)) {
                return Err(ConfigError::Invalid {
                    path: path.to_string(),
                    field: format!("bots[{i}].name"),
                    message: format!("`{}` is already defined in bots[{j}]", spec.name),
                });
            }
        }
        Ok(f)
    }

    pub fn entries(&self) -> &[PersonaSpec] {
        self.bots.as_deref().unwrap_or(&[])
    }
}

impl PersonaSpec {
    pub fn validate(&self, at: &str, path: &str) -> Result<(), ConfigError> {
        let bad = |field: &str, message: String| ConfigError::Invalid {
            path: path.to_string(),
            field: format!("{at}.{field}"),
            message,
        };
        if self.name.is_empty() || sanitize_name(&self.name) != self.name {
            return Err(bad(
                "name",
                "must be 1..=31 bytes without quotes, `;`, `%`, `\\` or control characters".into(),
            ));
        }
        if let Some(style) = &self.style
            && !STYLE_IDS.contains(&style.as_str())
        {
            return Err(bad(
                "style",
                format!("unknown style `{style}`, expected one of {}", STYLE_IDS.join(", ")),
            ));
        }
        if let Some(skill) = &self.skill
            && skill.resolve().is_none()
        {
            return Err(bad("skill", "expected 0..=100 or noob|easy|normal|hard|expert".into()));
        }
        if let Some(t) = self.traits {
            for (name, v) in [("aggression", t.aggression), ("fear", t.fear)] {
                if v.is_some_and(|v| !(0.0..=1.0).contains(&v)) {
                    return Err(bad(&format!("traits.{name}"), "must be in 0..=1".into()));
                }
            }
        }
        if self.weight.is_some_and(|w| !w.is_finite() || w < 0.0) {
            return Err(bad("weight", "must be 0 or more".into()));
        }
        if self
            .model
            .as_deref()
            .is_some_and(|m| m.is_empty() || m.contains(['"', '\\', ';']))
        {
            return Err(bad("model", "must be a plain model name".into()));
        }
        if let Some(chat) = &self.chat {
            if chat.chattiness.is_some_and(|c| !(0.0..=1.0).contains(&c)) {
                return Err(bad("chat.chattiness", "must be in 0..=1".into()));
            }
            if chat
                .typing_cpm
                .is_some_and(|c| !(TYPING_CPM_RANGE[0]..=TYPING_CPM_RANGE[1]).contains(&c))
            {
                return Err(bad("chat.typing_cpm", "must be in 30..=1500".into()));
            }
            for (field, text) in [("style", &chat.style), ("about", &chat.about)] {
                if text.as_deref().is_some_and(|t| t.chars().count() > CHAT_TEXT_MAX) {
                    return Err(bad(
                        &format!("chat.{field}"),
                        format!("must be at most {CHAT_TEXT_MAX} characters"),
                    ));
                }
            }
            if chat.call.len() > CALLS_MAX {
                return Err(bad("chat.call", format!("at most {CALLS_MAX} names")));
            }
            let plain = |name: &String| {
                let name = name.trim();
                !name.is_empty()
                    && name.chars().count() <= CALL_MAX
                    && !name.chars().any(|c| c.is_control() || "\"%;".contains(c))
            };
            if !chat.call.iter().all(plain) {
                return Err(bad(
                    "chat.call",
                    format!("names of 1..={CALL_MAX} characters without quotes, `%`, `;` or control characters"),
                ));
            }
        }
        Ok(())
    }

    /// The entry as YAML lines for appending under `bots:` (two-space list indent). Strings are written as JSON
    /// strings, which are valid YAML double-quoted scalars.
    pub fn to_yaml_entry(&self) -> String {
        let q = yaml::quote;
        let mut s = format!("  - name: {}\n", q(&self.name));
        if let Some(style) = &self.style {
            s += &format!("    style: {style}\n");
        }
        if let Some(skill) = self.skill.as_ref().and_then(SkillValue::resolve) {
            s += &format!("    skill: {skill}\n");
        }
        if let Some(model) = &self.model {
            s += &format!("    model: {}\n", q(model));
        }
        if let Some([top, bottom]) = self.colors {
            s += &format!("    colors: [{top}, {bottom}]\n");
        }
        if let Some(t) = self.traits {
            let mut parts = Vec::new();
            if let Some(a) = t.aggression {
                parts.push(format!("aggression: {a:.2}"));
            }
            if let Some(f) = t.fear {
                parts.push(format!("fear: {f:.2}"));
            }
            if !parts.is_empty() {
                s += &format!("    traits: {{ {} }}\n", parts.join(", "));
            }
        }
        if let Some(w) = self.weight {
            s += &format!("    weight: {w}\n");
        }
        if let Some(seed) = self.seed {
            s += &format!("    seed: {seed:#x}\n");
        }
        if let Some(created) = &self.created {
            s += &format!("    created: {}\n", q(created));
        }
        s
    }
}

/// Header of the server-written profile file.
pub const GENERATED_HEADER: &str = "\
schema: lambdabots/profiles@1
# Personalities the server created the first time a nickname joined. From then on the nickname always
# plays with the style, skill and look stored here. Edit entries freely: the server only appends new
# ones at the end, so keep `bots:` the last key. An entry with the same name in profiles/*.yaml wins.
bots:
";

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
schema: lambdabots/profiles@1
bots:
  - name: "Kleiner"
    style: sniper
    skill: 70
    model: scientist
    colors: [160, 40]
    traits: { aggression: 0.35, fear: 0.8 }
    weapons: [crossbow, "357"]
    weight: 2
    overrides:
      turn_speed: 520
  - name: "-|NoS|-"
    skill: hard
    seed: 0x5f3a91c2
"#;

    #[test]
    fn parses_hand_written_profiles() {
        let f = ProfilesFile::parse(SAMPLE, "roster.yaml").unwrap();
        let bots = f.entries();
        assert_eq!(bots.len(), 2);
        assert_eq!(bots[0].style.as_deref(), Some("sniper"));
        assert_eq!(bots[0].overrides.turn_speed, Some(520.0));
        assert_eq!(bots[1].skill.as_ref().and_then(SkillValue::resolve), Some(75));
        assert_eq!(bots[1].seed, Some(0x5f3a91c2));
    }

    #[test]
    fn empty_list_is_valid() {
        let f = ProfilesFile::parse(GENERATED_HEADER, "profiles.yaml").unwrap();
        assert!(f.entries().is_empty());
    }

    #[test]
    fn rejects_bad_entries() {
        let bad =
            |body: &str| ProfilesFile::parse(&format!("schema: lambdabots/profiles@1\nbots:\n{body}"), "x").is_err();
        assert!(bad("  - name: \"a\"\n    style: camper\n"));
        assert!(bad("  - name: \"a\"\n    skill: 120\n"));
        assert!(bad("  - name: \"a;b\"\n"));
        assert!(bad("  - name: \"a\"\n    traits: { aggression: 1.5 }\n"));
        assert!(
            bad("  - name: \"a\"\n  - name: \"A\"\n"),
            "duplicate names, case-insensitive"
        );
        assert!(bad("  - name: \"a\"\n    speed: 3\n"), "unknown fields");
        assert!(bad("  - name: \"a\"\n    chat: { chattiness: 2 }\n"));
        assert!(bad("  - name: \"a\"\n    chat: { typing_cpm: 5 }\n"));
        assert!(
            bad("  - name: \"a\"\n    chat: { mood: angry }\n"),
            "unknown chat fields"
        );
        assert!(bad("  - name: \"a\"\n    chat: { call: [x, \"\"] }\n"), "an empty name");
        assert!(bad("  - name: \"a\"\n    chat: { call: \"x;y\" }\n"));
        assert!(bad(&format!(
            "  - name: \"a\"\n    chat: {{ call: {} }}\n",
            "я".repeat(CALL_MAX + 1)
        )));
        let nine = (0..=CALLS_MAX).map(|i| format!("n{i}")).collect::<Vec<_>>().join(", ");
        assert!(bad(&format!("  - name: \"a\"\n    chat: {{ call: [{nine}] }}\n")));
    }

    #[test]
    fn chat_block() {
        let text = "schema: lambdabots/profiles@1\nbots:\n  - name: \"DUT9 ATLASA\"\n    chat:\n      chattiness: 0.7\n      profanity: true\n      style: \"коротко, строчными, ставит ))\"\n      about: \"довольно хороший игрок, любит арбалет\"\n      call: [Атлас, Атласыч]\n  - name: Kleiner\n    chat: { call: Кляйнер }\n  - name: Gina\n    chat: { chattiness: 0.2 }\n";
        let f = ProfilesFile::parse(text, "roster.yaml").unwrap();
        let chat = f.entries()[0].chat.as_ref().unwrap();
        assert_eq!(chat.chattiness, Some(0.7));
        assert_eq!(chat.profanity, Some(true));
        assert_eq!(chat.typing_cpm, None);
        assert_eq!(chat.style.as_deref(), Some("коротко, строчными, ставит ))"));
        assert_eq!(chat.call, ["Атлас", "Атласыч"]);
        assert_eq!(f.entries()[1].chat.as_ref().unwrap().call, ["Кляйнер"], "one name");
        assert!(f.entries()[2].chat.as_ref().unwrap().call.is_empty());
    }

    #[test]
    fn appended_entries_parse_back() {
        let spec = PersonaSpec {
            name: "-|NoS|- Ковбой".into(),
            style: Some("rusher".into()),
            skill: Some(SkillValue::Number(58.0)),
            model: Some("gordon".into()),
            colors: Some([12, 200]),
            traits: Some(TraitsSpec {
                aggression: Some(0.81),
                fear: Some(0.22),
            }),
            seed: Some(0xdead_beef),
            created: Some("2026-09-27".into()),
            ..Default::default()
        };
        let text = format!("{GENERATED_HEADER}{}", spec.to_yaml_entry());
        let back = ProfilesFile::parse(&text, "profiles.yaml").unwrap();
        let got = &back.entries()[0];
        assert_eq!(got.name, spec.name);
        assert_eq!(got.skill.as_ref().and_then(SkillValue::resolve), Some(58));
        assert_eq!(got.colors, Some([12, 200]));
        assert_eq!(got.seed, Some(0xdead_beef));
        assert_eq!(got.traits.unwrap().fear, Some(0.22));
    }
}
