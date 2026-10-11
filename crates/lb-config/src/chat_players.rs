//! `config/chat/players.yaml`: what the bots know of regular players, in the admin's words.

use serde::{Deserialize, Serialize};

use crate::ConfigError;
use crate::yaml;

pub const KIND: &str = "chat-players";
pub const MAJOR: u32 = 1;
/// Longest note in characters.
pub const NOTE_MAX: usize = 500;
/// Longest alias in characters.
pub const ALIAS_MAX: usize = 32;
/// Most aliases of one player.
pub const ALIASES_MAX: usize = 8;

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ChatPlayersFile {
    pub schema: String,
    #[serde(default)]
    pub players: Vec<KnownPlayer>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct KnownPlayer {
    /// A SteamID (`STEAM_0:1:123`), or the nickname of a player without one, or any player's with `by_name`.
    pub id: String,
    /// The player's nickname when `id` is a SteamID, for the reader: the note and the aliases go to that SteamID only.
    #[serde(default)]
    pub name: String,
    /// What the bots call the player instead of the nickname: one name (`Атлас`) or several (`[Атлас, Атласыч]`),
    /// the main one first.
    #[serde(default, deserialize_with = "yaml::one_or_many")]
    pub alias: Vec<String>,
    #[serde(default)]
    pub note: String,
    /// The entry applies to any player with this nickname whatever the SteamID, alias and note alike; `id` is then
    /// the nickname.
    #[serde(default)]
    pub by_name: bool,
}

/// `id` names a SteamID rather than a nickname.
pub fn is_steam_id(id: &str) -> bool {
    let id = id.trim();
    id.starts_with("STEAM_") || id.starts_with("VALVE_")
}

impl ChatPlayersFile {
    pub fn parse(text: &str, path: &str) -> Result<ChatPlayersFile, ConfigError> {
        let f: ChatPlayersFile = yaml::from_str(text, path)?;
        yaml::check_schema(&f.schema, KIND, MAJOR, path)?;
        for (i, p) in f.players.iter().enumerate() {
            let bad = |field: &str, message: &str| {
                Err(ConfigError::Invalid {
                    path: path.to_string(),
                    field: format!("players[{i}].{field}"),
                    message: message.to_string(),
                })
            };
            if p.id.trim().is_empty() {
                return bad("id", "must not be empty");
            }
            if p.by_name && is_steam_id(&p.id) {
                return bad("by_name", "`id` must then be a nickname, not a SteamID");
            }
            if p.note.chars().count() > NOTE_MAX {
                return bad("note", &format!("must be at most {NOTE_MAX} characters"));
            }
            if p.alias.len() > ALIASES_MAX {
                return bad("alias", &format!("at most {ALIASES_MAX} names"));
            }
            let plain = |a: &String| {
                let a = a.trim();
                !a.is_empty()
                    && a.chars().count() <= ALIAS_MAX
                    && !a.chars().any(|c| c.is_control() || "\"%;".contains(c))
            };
            if !p.alias.iter().all(plain) {
                return bad(
                    "alias",
                    &format!("names of 1..={ALIAS_MAX} characters without quotes, `%`, `;` or control characters"),
                );
            }
        }
        Ok(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_validates() {
        let text = "schema: lambdabots/chat-players@1\nplayers:\n  - id: STEAM_0:0:219579426\n    name: ATLAS Gamer\n    note: Хороший игрок, один из лучших.\n  - id: 112S\n    note: часто пишет в чат\n";
        let f = ChatPlayersFile::parse(text, "players.yaml").unwrap();
        assert_eq!(f.players.len(), 2);
        assert_eq!(f.players[1].name, "");
        let aliased = "schema: lambdabots/chat-players@1\nplayers:\n  - id: ET^NiK\n    alias: Ник\n  - id: STEAM_0:0:1\n    alias: [Атлас, Атласыч]\n    note: x\n";
        let f = ChatPlayersFile::parse(aliased, "players.yaml").unwrap();
        assert_eq!(
            (f.players[0].alias.as_slice(), f.players[0].note.as_str()),
            (&["Ник".to_string()][..], "")
        );
        assert_eq!(f.players[1].alias, ["Атлас", "Атласыч"]);
        assert!(f.players.iter().all(|p| p.name.is_empty()));
        assert!(
            ChatPlayersFile::parse(
                "schema: lambdabots/chat-players@1\nplayers:\n  - id: a\n    alias: [x, \"\"]\n",
                "p"
            )
            .is_err(),
            "an empty name"
        );
        assert!(
            ChatPlayersFile::parse(
                "schema: lambdabots/chat-players@1\nplayers:\n  - id: a\n    alias: \"x;y\"\n",
                "p"
            )
            .is_err()
        );
        assert!(
            ChatPlayersFile::parse(
                "schema: lambdabots/chat-players@1\nplayers:\n  - id: \"\"\n    note: x\n",
                "p"
            )
            .is_err()
        );
        assert!(
            ChatPlayersFile::parse(
                "schema: lambdabots/chat-players@1\nplayers:\n  - id: a\n    note: x\n    age: 3\n",
                "p"
            )
            .is_err()
        );
        assert!(
            ChatPlayersFile::parse("schema: lambdabots/chat-players@1\n", "p")
                .unwrap()
                .players
                .is_empty()
        );
    }

    #[test]
    fn by_name_takes_a_nickname() {
        let text = "schema: lambdabots/chat-players@1\nplayers:\n  - id: \"[KZ] Lynx :>\"\n    by_name: true\n    alias: Рысь\n  - id: STEAM_0:0:1\n    note: x\n";
        let f = ChatPlayersFile::parse(text, "p").unwrap();
        assert!(f.players[0].by_name && !f.players[1].by_name);
        assert_eq!(f.players[0].id, "[KZ] Lynx :>");
        for id in ["STEAM_0:0:1", " VALVE_0:1:2"] {
            let text = format!("schema: lambdabots/chat-players@1\nplayers:\n  - id: \"{id}\"\n    by_name: true\n");
            let e = ChatPlayersFile::parse(&text, "p").unwrap_err();
            assert!(e.to_string().contains("players[0].by_name"), "{e}");
        }
    }
}
