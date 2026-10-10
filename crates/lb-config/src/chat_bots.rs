//! `config/chat/bots.yaml`: what one bot should know of itself, in the admin's words, on top of its profile.

use serde::{Deserialize, Serialize};

use crate::ConfigError;
use crate::names::sanitize_name;
use crate::yaml;

pub const KIND: &str = "chat-bots";
pub const MAJOR: u32 = 1;
/// Longest context in characters.
pub const CONTEXT_MAX: usize = 1000;

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ChatBotsFile {
    pub schema: String,
    #[serde(default)]
    pub bots: Vec<BotContext>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BotContext {
    /// The personality's name as `lb list` shows it, or the bot's nickname in the game.
    pub name: String,
    /// Who the bot is here, whom it knows, what it likes to talk about.
    pub context: String,
}

impl ChatBotsFile {
    pub fn parse(text: &str, path: &str) -> Result<ChatBotsFile, ConfigError> {
        let f: ChatBotsFile = yaml::from_str(text, path)?;
        yaml::check_schema(&f.schema, KIND, MAJOR, path)?;
        check_names(f.bots.iter().map(|b| b.name.as_str()), path)?;
        for (i, b) in f.bots.iter().enumerate() {
            let n = b.context.trim().chars().count();
            if !(1..=CONTEXT_MAX).contains(&n) {
                return Err(ConfigError::Invalid {
                    path: path.to_string(),
                    field: format!("bots[{i}].context"),
                    message: format!("must be 1..={CONTEXT_MAX} characters"),
                });
            }
        }
        Ok(f)
    }

    /// The context of the bot with this personality name or nickname, in any case.
    pub fn context(&self, name: &str) -> Option<&str> {
        let name = name.to_lowercase();
        self.bots
            .iter()
            .find(|b| b.name.to_lowercase() == name)
            .map(|b| b.context.trim())
    }
}

/// Checks the `bots[i].name` of a list (here and in `phrases.yaml`): a name as profiles take it, none twice in any
/// case.
pub(crate) fn check_names<'a>(names: impl Iterator<Item = &'a str>, path: &str) -> Result<(), ConfigError> {
    let mut seen: Vec<String> = Vec::new();
    for (i, name) in names.enumerate() {
        let bad = |message: String| ConfigError::Invalid {
            path: path.to_string(),
            field: format!("bots[{i}].name"),
            message,
        };
        if name.is_empty() || sanitize_name(name) != name {
            return Err(bad(
                "must be 1..=31 bytes without quotes, `;`, `%`, `\\` or control characters".into(),
            ));
        }
        let lower = name.to_lowercase();
        if let Some(j) = seen.iter().position(|s| *s == lower) {
            return Err(bad(format!("`{name}` is already defined in bots[{j}]")));
        }
        seen.push(lower);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEAD: &str = "schema: lambdabots/chat-bots@1\n";

    #[test]
    fn parses_and_finds_by_name() {
        let text = format!(
            "{HEAD}bots:\n  - name: \"Plutonium\"\n    context: |\n      Играет тут с открытия сервера.\n  - name: \"[B] Ёжик\"\n    context: любит гаусс\n"
        );
        let f = ChatBotsFile::parse(&text, "bots.yaml").unwrap();
        assert_eq!(f.context("plutonium"), Some("Играет тут с открытия сервера."));
        assert_eq!(f.context("[b] ёжик"), Some("любит гаусс"));
        assert_eq!(f.context("Kleiner"), None);
        assert!(ChatBotsFile::parse(HEAD, "b").unwrap().bots.is_empty());
        assert!(
            ChatBotsFile::parse(&format!("{HEAD}bots: []\n"), "b")
                .unwrap()
                .bots
                .is_empty()
        );
    }

    #[test]
    fn rejects_bad_entries() {
        let err = |body: &str| {
            ChatBotsFile::parse(&format!("{HEAD}bots:\n{body}"), "b")
                .unwrap_err()
                .to_string()
        };
        assert!(err("  - name: \"\"\n    context: x\n").contains("bots[0].name"));
        assert!(err("  - name: \" Plutonium\"\n    context: x\n").contains("bots[0].name"));
        assert!(err("  - name: \"a;b\"\n    context: x\n").contains("bots[0].name"));
        assert!(err(&format!("  - name: {}\n    context: x\n", "x".repeat(32))).contains("bots[0].name"));
        let dup = err("  - name: Ёжик\n    context: x\n  - name: ЁЖИК\n    context: y\n");
        assert!(dup.contains("bots[1].name") && dup.contains("bots[0]"), "{dup}");
        assert!(err("  - name: a\n    context: \"  \"\n").contains("bots[0].context"));
        let long = format!("  - name: a\n    context: {}\n", "я".repeat(CONTEXT_MAX + 1));
        assert!(err(&long).contains("bots[0].context"));
        assert!(err("  - name: a\n").contains("context"));
        assert!(err("  - name: a\n    context: x\n    about: y\n").contains("about"));
        assert!(ChatBotsFile::parse("schema: lambdabots/chat-bots@2\n", "b").is_err());
    }
}
