//! `config/chat/server.yaml`: what every bot should know of the server, in the admin's words.

use serde::{Deserialize, Serialize};

use crate::ConfigError;
use crate::yaml;

pub const KIND: &str = "chat-server";
pub const MAJOR: u32 = 1;
/// Longest context in characters.
pub const CONTEXT_MAX: usize = 2000;

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ChatServerFile {
    pub schema: String,
    /// The server, its rules, its regulars, what goes on there; empty for nothing.
    #[serde(default)]
    pub context: Option<String>,
}

impl ChatServerFile {
    pub fn parse(text: &str, path: &str) -> Result<ChatServerFile, ConfigError> {
        let f: ChatServerFile = yaml::from_str(text, path)?;
        yaml::check_schema(&f.schema, KIND, MAJOR, path)?;
        if f.context().is_some_and(|c| c.chars().count() > CONTEXT_MAX) {
            return Err(ConfigError::Invalid {
                path: path.to_string(),
                field: "context".into(),
                message: format!("must be at most {CONTEXT_MAX} characters"),
            });
        }
        Ok(f)
    }

    /// The context without the blank space around it; `None` when there is none.
    pub fn context(&self) -> Option<&str> {
        self.context.as_deref().map(str::trim).filter(|c| !c.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEAD: &str = "schema: lambdabots/chat-server@1\n";

    #[test]
    fn parses_and_trims() {
        let f =
            ChatServerFile::parse(&format!("{HEAD}context: |\n  GunGame-сервер.\n  Вечером людно.\n"), "s").unwrap();
        assert_eq!(f.context(), Some("GunGame-сервер.\nВечером людно."));
        for empty in ["", "context:\n", "context: \"\"\n", "context: \"  \"\n"] {
            assert_eq!(
                ChatServerFile::parse(&format!("{HEAD}{empty}"), "s").unwrap().context(),
                None,
                "{empty:?}"
            );
        }
    }

    #[test]
    fn rejects_bad_files() {
        let long = format!("{HEAD}context: {}\n", "я".repeat(CONTEXT_MAX + 1));
        assert!(
            ChatServerFile::parse(&long, "s")
                .unwrap_err()
                .to_string()
                .contains("context")
        );
        let fits = format!("{HEAD}context: \"  {}  \"\n", "я".repeat(CONTEXT_MAX));
        assert!(ChatServerFile::parse(&fits, "s").is_ok());
        assert!(ChatServerFile::parse(&format!("{HEAD}text: x\n"), "s").is_err());
        assert!(ChatServerFile::parse("schema: lambdabots/chat-bots@1\n", "s").is_err());
    }
}
