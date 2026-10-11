//! `config/chat/maps.yaml`: what the bots should know of maps, in the admin's words.

use serde::{Deserialize, Serialize};

use crate::ConfigError;
use crate::yaml;

pub const KIND: &str = "chat-maps";
pub const MAJOR: u32 = 1;
/// Longest note in characters.
pub const NOTE_MAX: usize = 1000;
/// Longest map name or pattern in characters.
pub const PATTERN_MAX: usize = 64;
/// Most characters of notes shown for one map.
pub const MAP_NOTES_MAX: usize = 1500;

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ChatMapsFile {
    pub schema: String,
    #[serde(default)]
    pub maps: Vec<MapNote>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MapNote {
    /// A map's name (`crossfire`), a pattern where `*` stands for any text (`gg_*`), or a list of them.
    #[serde(deserialize_with = "yaml::one_or_many")]
    pub map: Vec<String>,
    pub note: String,
}

impl ChatMapsFile {
    pub fn parse(text: &str, path: &str) -> Result<ChatMapsFile, ConfigError> {
        let f: ChatMapsFile = yaml::from_str(text, path)?;
        yaml::check_schema(&f.schema, KIND, MAJOR, path)?;
        for (i, m) in f.maps.iter().enumerate() {
            let bad = |field: &str, message: String| {
                Err(ConfigError::Invalid {
                    path: path.to_string(),
                    field: format!("maps[{i}].{field}"),
                    message,
                })
            };
            if m.map.is_empty() {
                return bad("map", "must name a map".into());
            }
            let plain = |p: &String| {
                (1..=PATTERN_MAX).contains(&p.chars().count())
                    && !p
                        .chars()
                        .any(|c| c.is_whitespace() || c.is_control() || c == '/' || c == '\\')
            };
            if !m.map.iter().all(plain) {
                return bad(
                    "map",
                    format!("names and patterns of 1..={PATTERN_MAX} characters without spaces, `/` or `\\`"),
                );
            }
            if !(1..=NOTE_MAX).contains(&m.note.trim().chars().count()) {
                return bad("note", format!("must be 1..={NOTE_MAX} characters"));
            }
        }
        Ok(f)
    }

    /// The notes on `map`: those naming it first, then those with a pattern that fits it, each in file order, joined
    /// by newlines. A note that would take the text past [`MAP_NOTES_MAX`] characters is left out, never the first.
    pub fn notes(&self, map: &str) -> Option<String> {
        let names = |m: &MapNote, pattern: bool| m.map.iter().any(|p| p.contains('*') == pattern && fits(p, map));
        let named = self.maps.iter().filter(|m| names(m, false));
        let matched = self.maps.iter().filter(|m| !names(m, false) && names(m, true));
        let mut notes = named.chain(matched).map(|m| m.note.trim());
        let mut out = notes.next()?.to_string();
        let mut used = out.chars().count();
        for note in notes {
            let n = note.chars().count();
            if used + 1 + n <= MAP_NOTES_MAX {
                used += 1 + n;
                out.push('\n');
                out.push_str(note);
            }
        }
        Some(out)
    }
}

/// `map` fits the name or pattern, in any case: `*` stands for any text, empty too.
pub fn fits(pattern: &str, map: &str) -> bool {
    let pattern = pattern.to_lowercase();
    let map = map.to_lowercase();
    let mut pieces = pattern.split('*');
    let first = pieces.next().unwrap_or_default();
    let Some(mut rest) = map.strip_prefix(first) else {
        return false;
    };
    let pieces: Vec<&str> = pieces.collect();
    let Some((last, middle)) = pieces.split_last() else {
        return rest.is_empty();
    };
    for piece in middle {
        let Some(at) = rest.find(piece) else {
            return false;
        };
        rest = &rest[at + piece.len()..];
    }
    rest.ends_with(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEAD: &str = "schema: lambdabots/chat-maps@1\n";

    #[test]
    fn patterns_fit() {
        for (pattern, map) in [
            ("crossfire", "crossfire"),
            ("Crossfire", "CROSSFIRE"),
            ("gg_*", "gg_cold_rock"),
            ("gg_*", "gg_"),
            ("*", "anything"),
            ("*_rock", "gg_cold_rock"),
            ("gg_*_rock", "gg_cold_rock"),
            ("*cold*", "gg_cold_rock"),
            ("a*b*c", "abc"),
            ("a*a", "aa"),
            ("a**", "a"),
        ] {
            assert!(fits(pattern, map), "{pattern} {map}");
        }
        for (pattern, map) in [
            ("crossfire", "crossfire2"),
            ("crossfire", "cross"),
            ("gg_*", "ag_x"),
            ("gg_*", "gg"),
            ("*_rock", "gg_rocks"),
            ("a*b*c", "ac"),
            ("a*a", "a"),
            ("a*b*a", "aba2"),
        ] {
            assert!(!fits(pattern, map), "{pattern} {map}");
        }
    }

    #[test]
    fn notes_come_named_first_then_patterns() {
        let text = format!(
            "{HEAD}maps:\n  - map: [gg_*, ag_*]\n    note: small arenas\n  - map: \"*\"\n    note: everywhere\n  - map: [stalkyard, GG_Cold_Rock]\n    note: the cold one\n  - map: gg_cold_rock\n    note: |\n      ice\n"
        );
        let f = ChatMapsFile::parse(&text, "maps.yaml").unwrap();
        assert_eq!(
            f.notes("gg_cold_rock").as_deref(),
            Some("the cold one\nice\nsmall arenas\neverywhere")
        );
        assert_eq!(f.notes("ag_bleh").as_deref(), Some("small arenas\neverywhere"));
        assert_eq!(f.notes("crossfire").as_deref(), Some("everywhere"));
        let only = ChatMapsFile::parse(&format!("{HEAD}maps:\n  - map: crossfire\n    note: x\n"), "m").unwrap();
        assert_eq!(only.notes("stalkyard"), None);
        assert_eq!(
            ChatMapsFile::parse(&format!("{HEAD}maps: []\n"), "m")
                .unwrap()
                .notes("x"),
            None
        );
    }

    #[test]
    fn notes_keep_within_the_cap() {
        let note = |c: char, n: usize| c.to_string().repeat(n);
        let text = format!(
            "{HEAD}maps:\n  - map: \"*\"\n    note: {}\n  - map: \"*\"\n    note: {}\n  - map: \"*\"\n    note: {}\n  - map: \"*\"\n    note: {}\n",
            note('a', NOTE_MAX),
            note('b', 600),
            note('c', 300),
            note('d', MAP_NOTES_MAX - NOTE_MAX - 300 - 2),
        );
        let f = ChatMapsFile::parse(&text, "m").unwrap();
        let notes = f.notes("crossfire").unwrap();
        assert_eq!(notes.chars().count(), MAP_NOTES_MAX);
        let parts: Vec<char> = notes.split('\n').map(|p| p.chars().next().unwrap()).collect();
        assert_eq!(parts, ['a', 'c', 'd'], "the one that does not fit is left out");
    }

    #[test]
    fn rejects_bad_entries() {
        let err = |body: &str| {
            ChatMapsFile::parse(&format!("{HEAD}maps:\n{body}"), "m")
                .unwrap_err()
                .to_string()
        };
        assert!(err("  - map: []\n    note: x\n").contains("maps[0].map"));
        assert!(err("  - map: \"gg *\"\n    note: x\n").contains("maps[0].map"));
        assert!(err("  - map: [crossfire, \"maps/x\"]\n    note: x\n").contains("maps[0].map"));
        assert!(err("  - map: \"a\\\\b\"\n    note: x\n").contains("maps[0].map"));
        assert!(err("  - map: \"\"\n    note: x\n").contains("maps[0].map"));
        assert!(err(&format!("  - map: {}\n    note: x\n", "x".repeat(PATTERN_MAX + 1))).contains("maps[0].map"));
        assert!(err("  - map: x\n    note: \" \"\n").contains("maps[0].note"));
        assert!(err(&format!("  - map: x\n    note: {}\n", "я".repeat(NOTE_MAX + 1))).contains("maps[0].note"));
        assert!(err("  - note: x\n").contains("map"));
        assert!(err("  - map: x\n").contains("note"));
        assert!(err("  - map: x\n    note: y\n    notes: z\n").contains("notes"));
        let fits = format!("{HEAD}maps:\n  - map: {}\n    note: x\n", "x".repeat(PATTERN_MAX));
        assert!(ChatMapsFile::parse(&fits, "m").is_ok());
    }
}
