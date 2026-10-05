//! The chat worker's files: what the bots remember (`data/chat/memory.json`), the tokens spent today
//! (`data/chat/usage.json`) and the admin's notes on players (`config/chat/players.yaml`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use lb_chat::memory::Memory;
use lb_config::chat_players::ChatPlayersFile;
use serde::{Deserialize, Serialize};

pub struct Paths {
    pub install: PathBuf,
    pub memory: PathBuf,
    pub usage: PathBuf,
    pub players: PathBuf,
}

impl Paths {
    pub fn new(install: &Path) -> Paths {
        let data = install.join("data").join("chat");
        Paths {
            install: install.to_path_buf(),
            memory: data.join("memory.json"),
            usage: data.join("usage.json"),
            players: install.join("config").join("chat").join("players.yaml"),
        }
    }

    /// A path from the config: as it is when absolute, else under the install directory.
    pub fn resolve(&self, path: &str) -> PathBuf {
        let p = Path::new(path.trim());
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.install.join(p)
        }
    }
}

/// Writes through a temporary file, so a crash never leaves half a file.
pub fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

/// The memory kept, or an empty one; a broken file is set aside, not overwritten.
pub fn load_memory(path: &Path) -> Memory {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Memory::default();
    };
    match Memory::parse(&text) {
        Ok(m) => m,
        Err(e) => {
            let aside = path.with_extension("bad.json");
            tracing::warn!("chat memory {}: {e}; moved to {}", path.display(), aside.display());
            let _ = std::fs::rename(path, &aside);
            Memory::default()
        }
    }
}

/// Tokens spent on one UTC day.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    /// Days since the epoch.
    pub day: u64,
    pub tokens: u64,
    pub requests: u64,
}

impl Usage {
    /// Counts `tokens` on `day`, starting the count over on a new day.
    pub fn add(&mut self, day: u64, tokens: u64) {
        if day != self.day {
            *self = Usage {
                day,
                ..Usage::default()
            };
        }
        self.tokens += tokens;
        self.requests += 1;
    }

    /// Tokens spent on `day`.
    pub fn on(&self, day: u64) -> u64 {
        if day == self.day { self.tokens } else { 0 }
    }
}

pub fn load_usage(path: &Path) -> Usage {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// What the admin wrote of a player.
#[derive(Clone, Debug, Default)]
struct Entry {
    note: String,
    aliases: Vec<String>,
}

/// The admin's notes and aliases: by SteamID, and by nickname in lower case for players written down without one.
#[derive(Clone, Debug, Default)]
pub struct Notes {
    by_key: BTreeMap<String, Entry>,
    by_name: BTreeMap<String, Entry>,
}

impl Notes {
    pub fn load(path: &Path) -> Notes {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Notes::default();
        };
        match ChatPlayersFile::parse(&text, &path.display().to_string()) {
            Ok(f) => {
                let mut notes = Notes::default();
                for p in f.players {
                    let entry = Entry {
                        note: p.note.trim().to_string(),
                        aliases: p.alias.iter().map(|a| a.trim().to_string()).collect(),
                    };
                    let id = p.id.trim();
                    if id.starts_with("STEAM_") || id.starts_with("VALVE_") {
                        notes.by_key.insert(id.to_string(), entry);
                    } else {
                        notes.by_name.insert(id.to_lowercase(), entry);
                    }
                }
                notes
            }
            Err(e) => {
                tracing::warn!("chat: {e}");
                Notes::default()
            }
        }
    }

    pub fn len(&self) -> usize {
        self.by_key.len() + self.by_name.len()
    }

    fn entry(&self, key: &str, name: &str) -> Option<&Entry> {
        self.by_key
            .get(key)
            .or_else(|| self.by_name.get(&name.trim().to_lowercase()))
    }

    /// The note on a player by memory key, else by name.
    pub fn get(&self, key: &str, name: &str) -> Option<&str> {
        self.entry(key, name).map(|e| e.note.as_str()).filter(|n| !n.is_empty())
    }

    /// What the bots call a player, the main name first, by memory key, else by name.
    pub fn aliases(&self, key: &str, name: &str) -> &[String] {
        self.entry(key, name).map_or(&[], |e| e.aliases.as_slice())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_starts_over_each_day() {
        let mut u = Usage::default();
        u.add(10, 500);
        u.add(10, 300);
        assert_eq!((u.on(10), u.requests), (800, 2));
        u.add(11, 5);
        assert_eq!((u.on(10), u.on(11), u.requests), (0, 5, 1));
    }

    #[test]
    fn notes_by_id_and_name() {
        let dir = std::env::temp_dir().join(format!("lb-chat-notes-{}", std::process::id()));
        let path = dir.join("players.yaml");
        write_atomic(
            &path,
            "schema: lambdabots/chat-players@1\nplayers:\n  - id: STEAM_0:0:1\n    name: ATLAS Gamer\n    alias: [Атлас, Атласыч]\n    note: strong\n  - id: 112S\n    note: chatty\n  - id: ET^NiK\n    alias: Ник\n",
        )
        .unwrap();
        let notes = Notes::load(&path);
        assert_eq!(notes.get("STEAM_0:0:1", "whoever"), Some("strong"));
        assert_eq!(
            notes.get("STEAM_0:0:2", "ATLAS Gamer"),
            None,
            "a SteamID's note is not given by nickname"
        );
        assert!(notes.aliases("STEAM_0:0:2", "ATLAS Gamer").is_empty());
        assert_eq!(notes.get("name:atlas gamer", "ATLAS Gamer"), None);
        assert_eq!(notes.get("name:112s", "112s"), Some("chatty"));
        assert_eq!(notes.get("STEAM_0:0:2", "x"), None);
        assert_eq!(
            notes.aliases("STEAM_0:0:1", "ATLAS Gamer 2"),
            ["Атлас", "Атласыч"],
            "the SteamID keeps the aliases"
        );
        assert_eq!(notes.aliases("name:et^nik", "et^nik"), ["Ник"]);
        assert_eq!(notes.get("name:et^nik", "ET^NiK"), None, "an alias without a note");
        assert!(notes.aliases("name:112s", "112S").is_empty());
        let memory = dir.join("memory.json");
        std::fs::write(&memory, "{broken").unwrap();
        assert_eq!(load_memory(&memory), Memory::default());
        assert!(dir.join("memory.bad.json").exists(), "a broken file is set aside");
        let _ = std::fs::remove_dir_all(dir);
    }
}
