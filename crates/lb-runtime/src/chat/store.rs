//! The chat worker's files: what the bots remember (`data/chat/memory.json`), the tokens spent today
//! (`data/chat/usage.json`), and what the admin wrote in `config/chat/`: the players, the server, single bots, the maps
//! and the ready phrases.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use lb_chat::memory::Memory;
use lb_config::ConfigError;
use lb_config::chat_bots::ChatBotsFile;
use lb_config::chat_maps::ChatMapsFile;
use lb_config::chat_phrases::ChatPhrasesFile;
use lb_config::chat_players::{ChatPlayersFile, is_steam_id};
use lb_config::chat_server::ChatServerFile;
use serde::{Deserialize, Serialize};

/// The phrases shipped with the module: a server whose `config/` an update kept without `phrases.yaml` has them too.
const BUILT_IN_PHRASES: &str = include_str!("../../../../data/config/chat/phrases.yaml");

pub struct Paths {
    pub install: PathBuf,
    pub memory: PathBuf,
    pub usage: PathBuf,
    /// `config/chat/`: what the admin wrote.
    pub chat: PathBuf,
}

impl Paths {
    pub fn new(install: &Path) -> Paths {
        let data = install.join("data").join("chat");
        Paths {
            install: install.to_path_buf(),
            memory: data.join("memory.json"),
            usage: data.join("usage.json"),
            chat: install.join("config").join("chat"),
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

/// The admin's notes and aliases (`players.yaml`): by SteamID; by nickname whatever the SteamID (`by_name`); by
/// nickname for players written down without one. Nicknames in lower case.
#[derive(Clone, Debug, Default)]
pub struct Notes {
    by_key: BTreeMap<String, Entry>,
    /// `by_name` entries.
    any_id: BTreeMap<String, Entry>,
    by_name: BTreeMap<String, Entry>,
}

impl Notes {
    pub fn new(file: &ChatPlayersFile) -> Notes {
        let mut notes = Notes::default();
        for p in &file.players {
            let entry = Entry {
                note: p.note.trim().to_string(),
                aliases: p.alias.iter().map(|a| a.trim().to_string()).collect(),
            };
            let id = p.id.trim();
            if p.by_name {
                notes.any_id.insert(id.to_lowercase(), entry);
            } else if is_steam_id(id) {
                notes.by_key.insert(id.to_string(), entry);
            } else {
                notes.by_name.insert(id.to_lowercase(), entry);
            }
        }
        notes
    }

    /// A player's entry: by memory key; else by a `by_name` nickname; else by nickname when the player has no SteamID
    /// or it is not known (`""`).
    fn entry(&self, key: &str, name: &str) -> Option<&Entry> {
        let name = name.trim().to_lowercase();
        self.by_key.get(key).or_else(|| self.any_id.get(&name)).or_else(|| {
            (key.is_empty() || key.starts_with("name:"))
                .then(|| self.by_name.get(&name))
                .flatten()
        })
    }

    /// The note on a player, found as [`Notes::entry`] finds it.
    pub fn get(&self, key: &str, name: &str) -> Option<&str> {
        self.entry(key, name).map(|e| e.note.as_str()).filter(|n| !n.is_empty())
    }

    /// What the bots call a player, the main name first, found as [`Notes::entry`] finds it.
    pub fn aliases(&self, key: &str, name: &str) -> &[String] {
        self.entry(key, name).map_or(&[], |e| e.aliases.as_slice())
    }

    /// Whether the admin wrote of a player at all, aliases alone too.
    pub fn has(&self, key: &str, name: &str) -> bool {
        self.entry(key, name).is_some()
    }
}

/// A file of `config/chat/` as it was found.
enum Found<T> {
    Missing,
    /// It could not be read or parsed.
    Broken,
    Read(T),
}

/// Reads the files of `config/chat/`: a line on each, and the problems of those that cannot be read or parsed, which
/// are warned about.
struct Reader<'a> {
    dir: &'a Path,
    lines: Vec<String>,
    problems: Vec<String>,
}

impl Reader<'_> {
    /// The file `name`, parsed.
    fn found<T>(&mut self, name: &str, parse: fn(&str, &str) -> Result<T, ConfigError>) -> Found<T> {
        let path = self.dir.join(name);
        let shown = path.display().to_string();
        let parsed = match std::fs::read_to_string(&path) {
            Ok(text) => parse(&text, &shown),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Found::Missing,
            Err(source) => Err(ConfigError::Io { path: shown, source }),
        };
        match parsed {
            Ok(file) => Found::Read(file),
            Err(e) => {
                tracing::warn!("chat: {e}");
                self.problems.push(e.to_string());
                Found::Broken
            }
        }
    }

    /// The file `name`, parsed, with a line on `what` it holds; an empty one when it is not there or broken.
    fn read<T: Default>(
        &mut self,
        name: &str,
        parse: fn(&str, &str) -> Result<T, ConfigError>,
        what: impl FnOnce(&T) -> String,
    ) -> T {
        let (file, line) = match self.found(name, parse) {
            Found::Read(file) => {
                let what = what(&file);
                (file, what)
            }
            Found::Missing => (T::default(), "not found".into()),
            Found::Broken => (T::default(), "broken, left out".into()),
        };
        self.lines.push(format!("{name}: {line}"));
        file
    }

    /// `phrases.yaml`, and whether its phrases are the shipped ones: those stand in when it is not there, or when it is
    /// broken and none were read `before`.
    fn phrases(&mut self, before: Option<(ChatPhrasesFile, bool)>) -> (ChatPhrasesFile, bool) {
        let (line, phrases) = match (self.found("phrases.yaml", ChatPhrasesFile::parse), before) {
            (Found::Read(f), _) => (format!("phrases.yaml: {}", phrases_held(&f)), (f, false)),
            (Found::Missing, _) => ("phrases: built-in".to_string(), (built_in(), true)),
            (Found::Broken, Some((kept, true))) => {
                ("phrases.yaml: broken, the built-in phrases stay".into(), (kept, true))
            }
            (Found::Broken, Some((kept, false))) => {
                ("phrases.yaml: broken, the earlier phrases stay".into(), (kept, false))
            }
            (Found::Broken, None) => (
                "phrases.yaml: broken, the built-in phrases instead".into(),
                (built_in(), true),
            ),
        };
        self.lines.push(line);
        phrases
    }
}

/// `n` things: `1 bot`, `2 bots`.
fn count(n: usize, thing: &str) -> String {
    if n == 1 {
        format!("1 {thing}")
    } else {
        format!("{n} {thing}s")
    }
}

/// What a `phrases.yaml` holds: `349 phrases (en, ru), 2 bots`.
fn phrases_held(file: &ChatPhrasesFile) -> String {
    let n: usize = file.phrases.values().flat_map(|m| m.values()).map(Vec::len).sum();
    let languages: Vec<&str> = file.phrases.keys().map(String::as_str).collect();
    let mut what = count(n, "phrase");
    if !languages.is_empty() {
        what.push_str(&format!(" ({})", languages.join(", ")));
    }
    if !file.bots.is_empty() {
        what.push_str(&format!(", {}", count(file.bots.len(), "bot")));
    }
    what
}

/// The shipped phrases ([`BUILT_IN_PHRASES`]); none should they not parse, which a test rules out.
fn built_in() -> ChatPhrasesFile {
    ChatPhrasesFile::parse(BUILT_IN_PHRASES, "the built-in phrases.yaml").unwrap_or_else(|e| {
        tracing::warn!("chat: {e}");
        ChatPhrasesFile::default()
    })
}

/// What the admin wrote in `config/chat/`, read when the worker starts and on a reload. A file not there means
/// nothing to say; one that cannot be read or parsed is warned about and left out, but for the phrases: the shipped
/// ones stand in when `phrases.yaml` is not there or broken at the start, and a reload keeps those read before when it
/// broke since.
pub struct Library {
    pub players: Notes,
    pub server: ChatServerFile,
    pub bots: ChatBotsFile,
    pub maps: ChatMapsFile,
    pub phrases: ChatPhrasesFile,
    /// The phrases are the shipped ones.
    built_in: bool,
    /// A line for each file: what it holds, or why nothing.
    lines: Vec<String>,
    /// The files that could not be read or parsed, and why.
    pub problems: Vec<String>,
}

impl Library {
    pub fn load(dir: &Path) -> Library {
        Library::read(dir, None)
    }

    /// Reads `dir` again; a broken `phrases.yaml` keeps the phrases read before.
    pub fn reload(&mut self, dir: &Path) {
        let before = (std::mem::take(&mut self.phrases), self.built_in);
        *self = Library::read(dir, Some(before));
    }

    fn read(dir: &Path, before: Option<(ChatPhrasesFile, bool)>) -> Library {
        let mut r = Reader {
            dir,
            lines: Vec::new(),
            problems: Vec::new(),
        };
        let players = r.read("players.yaml", ChatPlayersFile::parse, |f| {
            count(f.players.len(), "player")
        });
        let server = r.read("server.yaml", ChatServerFile::parse, |f| match f.context() {
            Some(text) => count(text.chars().count(), "character"),
            None => "no context".into(),
        });
        let bots = r.read("bots.yaml", ChatBotsFile::parse, |f| count(f.bots.len(), "bot"));
        let maps = r.read("maps.yaml", ChatMapsFile::parse, |f| count(f.maps.len(), "note"));
        let (phrases, built_in) = r.phrases(before);
        Library {
            players: Notes::new(&players),
            server,
            bots,
            maps,
            phrases,
            built_in,
            lines: r.lines,
            problems: r.problems,
        }
    }

    /// A line for each file: `bots.yaml: 2 bots`, `maps.yaml: not found`, `phrases: built-in`.
    pub fn summary(&self) -> &[String] {
        &self.lines
    }

    /// `server.yaml`'s text; empty for none.
    pub fn server(&self) -> &str {
        self.server.context().unwrap_or("")
    }

    /// `bots.yaml`'s text on a bot: by its personality's name, else by its nickname; empty for none.
    pub fn bot(&self, persona: &str, nick: &str) -> &str {
        self.bots
            .context(persona)
            .or_else(|| self.bots.context(nick))
            .unwrap_or("")
    }

    /// `maps.yaml`'s notes on `map`; empty for none.
    pub fn map(&self, map: &str) -> String {
        self.maps.notes(map).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use lb_config::chat_phrases::Moment;

    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lb-chat-store-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn players(body: &str) -> Notes {
        let text = format!("schema: lambdabots/chat-players@1\nplayers:\n{body}");
        Notes::new(&ChatPlayersFile::parse(&text, "players.yaml").unwrap())
    }

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
        let notes = players(
            "  - id: STEAM_0:0:1\n    name: ATLAS Gamer\n    alias: [Атлас, Атласыч]\n    note: strong\n  - id: 112S\n    \
             note: chatty\n  - id: ET^NiK\n    alias: Ник\n",
        );
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
        assert_eq!(notes.aliases("", "ET^NiK"), ["Ник"], "a player not known");
        assert!(
            notes.aliases("STEAM_0:0:3", "ET^NiK").is_empty(),
            "a nickname's entry is not for a player with a SteamID"
        );
        assert_eq!(notes.get("name:et^nik", "ET^NiK"), None, "an alias without a note");
        assert!(notes.aliases("name:112s", "112S").is_empty());
        let dir = dir("memory");
        let memory = dir.join("memory.json");
        std::fs::write(&memory, "{broken").unwrap();
        assert_eq!(load_memory(&memory), Memory::default());
        assert!(dir.join("memory.bad.json").exists(), "a broken file is set aside");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_by_name_entry_is_for_any_steamid() {
        let notes = players(
            "  - id: \"[KZ] Lynx :>\"\n    by_name: true\n    alias: Рысь\n    note: always with the gauss\n  - id: \
             STEAM_0:1:7\n    alias: Седьмой\n  - id: Gordon\n    alias: Гордон\n",
        );
        for key in ["STEAM_0:1:5", "name:[kz] lynx :>", ""] {
            assert_eq!(notes.aliases(key, "[kz] LYNX :>"), ["Рысь"], "{key}");
            assert_eq!(notes.get(key, " [KZ] Lynx :> "), Some("always with the gauss"), "{key}");
            assert!(notes.has(key, "[KZ] Lynx :>"), "{key}");
        }
        assert_eq!(
            notes.aliases("STEAM_0:1:7", "[KZ] Lynx :>"),
            ["Седьмой"],
            "the SteamID's own entry first"
        );
        assert!(
            notes.has("STEAM_0:1:7", "x") && notes.get("STEAM_0:1:7", "x").is_none(),
            "aliases alone are an entry"
        );
        assert!(notes.has("name:gordon", "Gordon") && notes.has("", "gordon"));
        assert!(
            !notes.has("STEAM_0:1:5", "Gordon"),
            "a nickname's entry is not for a player with a SteamID"
        );
        assert!(!notes.has("", "Barney"));
    }

    #[test]
    fn the_shipped_phrases_are_built_in() {
        let f = built_in();
        for code in ["ru", "en"] {
            for moment in Moment::ALL {
                assert!(
                    f.phrases[code].get(&moment).is_some_and(|p| p.len() >= 8),
                    "{code}.{}",
                    moment.key()
                );
            }
        }
    }

    #[test]
    fn missing_files_are_quiet_and_broken_ones_left_out() {
        let dir = dir("library");
        let nothing = Library::load(&dir);
        assert_eq!(
            nothing.summary(),
            [
                "players.yaml: not found",
                "server.yaml: not found",
                "bots.yaml: not found",
                "maps.yaml: not found",
                "phrases: built-in"
            ]
        );
        assert!(nothing.problems.is_empty());
        assert_eq!(nothing.phrases, built_in());
        assert_eq!(
            (
                nothing.server(),
                nothing.bot("Kleiner", "x"),
                nothing.map("crossfire").as_str()
            ),
            ("", "", "")
        );

        let write = |name: &str, text: &str| std::fs::write(dir.join(name), text).unwrap();
        write(
            "players.yaml",
            "schema: lambdabots/chat-players@1\nplayers:\n  - id: Gordon\n    alias: Гордон\n",
        );
        write(
            "server.yaml",
            "schema: lambdabots/chat-server@1\ncontext: GunGame, вечером людно\n",
        );
        write(
            "bots.yaml",
            "schema: lambdabots/chat-bots@1\nbots:\n  - name: Kleiner\n    context: учёный\n  - name: \"[B] Barney\"\n    \
             context: охранник\n",
        );
        write(
            "maps.yaml",
            "schema: lambdabots/chat-maps@1\nmaps:\n  - map: \"gg_*\"\n    note: GunGame map\n",
        );
        write(
            "phrases.yaml",
            "schema: lambdabots/chat-phrases@1\nphrases:\n  en:\n    win: [\"gg, all\"]\n",
        );
        let all = Library::load(&dir);
        assert_eq!(
            all.summary(),
            [
                "players.yaml: 1 player",
                "server.yaml: 22 characters",
                "bots.yaml: 2 bots",
                "maps.yaml: 1 note",
                "phrases.yaml: 1 phrase (en)"
            ]
        );
        assert!(all.problems.is_empty());
        assert_eq!(all.players.aliases("", "gordon"), ["Гордон"]);
        assert_eq!(all.server(), "GunGame, вечером людно");
        assert_eq!(all.bot("kleiner", "[B] Kleiner"), "учёный", "by personality");
        assert_eq!(all.bot("Barney", "[b] barney"), "охранник", "else by nickname");
        assert_eq!(all.map("GG_cold_rock"), "GunGame map");
        assert_eq!(all.map("crossfire"), "");

        write(
            "bots.yaml",
            "schema: lambdabots/chat-bots@1\nbots:\n  - name: Kleiner\n",
        );
        std::fs::write(dir.join("server.yaml"), [0xff, 0xfe, b's', 0]).unwrap();
        write(
            "phrases.yaml",
            "schema: lambdabots/chat-phrases@1\nphrases:\n  ru:\n    greeting: [привет]\n",
        );
        let broken = Library::load(&dir);
        assert_eq!(
            broken.summary(),
            [
                "players.yaml: 1 player",
                "server.yaml: broken, left out",
                "bots.yaml: broken, left out",
                "maps.yaml: 1 note",
                "phrases.yaml: broken, the built-in phrases instead"
            ]
        );
        assert_eq!(broken.problems.len(), 3, "{:?}", broken.problems);
        assert!(broken.problems.iter().all(|p| p.contains(&*dir.display().to_string())));
        assert!(broken.problems[0].contains("server.yaml"), "a file that is no UTF-8");
        assert!(broken.problems[1].contains("bots.yaml"));
        assert_eq!((broken.server(), broken.bot("Kleiner", "x")), ("", ""));
        assert_eq!(broken.phrases, built_in());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_reload_keeps_the_phrases_when_their_file_broke() {
        let dir = dir("reload");
        let phrases = dir.join("phrases.yaml");
        let own = "schema: lambdabots/chat-phrases@1\nphrases:\n  ru:\n    win: [\"ну наконец-то\", изи]\n";
        std::fs::write(&phrases, own).unwrap();
        let mut library = Library::load(&dir);
        let read = library.phrases.clone();
        assert_eq!(library.summary()[4], "phrases.yaml: 2 phrases (ru)");
        std::fs::write(&phrases, "schema: lambdabots/chat-phrases@1\nphrases: [\n").unwrap();
        library.reload(&dir);
        assert_eq!(library.phrases, read, "the earlier phrases stay");
        assert_eq!(library.summary()[4], "phrases.yaml: broken, the earlier phrases stay");
        assert_eq!(library.problems.len(), 1);
        std::fs::remove_file(&phrases).unwrap();
        library.reload(&dir);
        assert_eq!(library.phrases, built_in(), "no file: the built-in ones");
        assert_eq!(library.summary()[4], "phrases: built-in");
        assert!(library.problems.is_empty());
        std::fs::write(&phrases, "schema: lambdabots/chat-phrases@2\n").unwrap();
        library.reload(&dir);
        assert_eq!(library.phrases, built_in());
        assert_eq!(library.summary()[4], "phrases.yaml: broken, the built-in phrases stay");
        std::fs::write(&phrases, own).unwrap();
        library.reload(&dir);
        assert_eq!(library.phrases, read);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
