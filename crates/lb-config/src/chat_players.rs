//! `config/chat/players.yaml`: what the bots know of regular players, in the admin's words.

use serde::{Deserialize, Serialize};

use crate::ConfigError;
use crate::yaml;

pub const KIND: &str = "chat-players";
pub const MAJOR: u32 = 1;
/// Longest note in characters.
pub const NOTE_MAX: usize = 500;

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
    /// A SteamID (`STEAM_0:1:123`), or the nickname of a player without one.
    pub id: String,
    /// The nickname the bots know the player by, when `id` is a SteamID.
    #[serde(default)]
    pub name: String,
    pub note: String,
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
            if p.note.chars().count() > NOTE_MAX {
                return bad("note", &format!("must be at most {NOTE_MAX} characters"));
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
}
