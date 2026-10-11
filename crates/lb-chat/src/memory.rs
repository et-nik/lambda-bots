//! What the bots remember of players between maps and restarts, kept by the chat worker in `data/chat/memory.json`:
//! the names a player used, the score against each bot, favourite weapons, wins, a few lines they wrote, their talks
//! with the bots, moments worth remembering, the model's notes, and the last maps.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::addressing::{self, Noise};
use crate::aliases::Aliases;
use crate::profanity;
use crate::request::MapSummary;

pub const SCHEMA: &str = "lambdabots/chat-memory@1";
const NAMES: usize = 5;
const LINES: usize = 10;
const MOMENTS: usize = 6;
const WEAPONS: usize = 5;
const MAPS: usize = 3;
/// Lines of a player's talk kept for each bot.
const TALK_LINES: usize = 12;
/// Bots a player's talks are kept with; the talk that ended longest ago goes first.
const TALK_BOTS: usize = 3;
pub const NOTES_MAX: usize = 300;
/// Players kept; the ones seen longest ago go first.
pub const MAX_PLAYERS: usize = 2000;
/// The engine's nickname for a player who never set one.
const DEFAULT_NICKNAME: &str = "Player";

/// The key a player is remembered by: a real SteamID, else the nickname in lower case.
pub fn player_key(auth_id: &str, name: &str) -> String {
    let id = auth_id.trim();
    let mut parts = id.split(':');
    let real = matches!(parts.next(), Some(p) if p.starts_with("STEAM_") || p.starts_with("VALVE_"))
        && parts.next().is_some_and(|p| p.parse::<u32>().is_ok())
        && parts.next().is_some_and(|p| p.parse::<u64>().is_ok_and(|n| n > 0));
    if real {
        id.to_string()
    } else {
        format!("name:{}", name.trim().to_lowercase())
    }
}

/// `name` without the `(1)` the engine puts before a nickname already taken on the server: `(1)Gordon` → `Gordon`.
fn unnumbered(name: &str) -> &str {
    let name = name.trim();
    let Some((n, rest)) = name.strip_prefix('(').and_then(|rest| rest.split_once(')')) else {
        return name;
    };
    if (1..=2).contains(&n.len()) && n.bytes().all(|b| b.is_ascii_digit()) && !rest.is_empty() {
        rest
    } else {
        name
    }
}

/// Whether two nicknames are the same but for case and the `(1)` the engine puts before a nickname already taken, as
/// those of a player who reconnected and of the ghost of their earlier connection, which still holds the nickname.
pub fn same_nickname(a: &str, b: &str) -> bool {
    nickname(a) == nickname(b)
}

/// A nickname as [`same_nickname`] compares it: trimmed, without the engine's `(1)`, in lower case.
fn nickname(name: &str) -> String {
    unnumbered(name).to_lowercase()
}

/// Whether `name` is the engine's default nickname, `Player`, in any case, with the engine's `(1)` too.
pub fn default_nickname(name: &str) -> bool {
    unnumbered(name).eq_ignore_ascii_case(DEFAULT_NICKNAME)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlayerMemory {
    /// Names the player used, the latest first.
    pub names: Vec<String>,
    /// Unix seconds.
    pub first_seen: u64,
    pub last_seen: u64,
    pub maps: u32,
    pub kills: u32,
    pub deaths: u32,
    /// Bot name → [the player killed the bot, the bot killed the player].
    pub vs_bots: BTreeMap<String, [u32; 2]>,
    /// Kills by weapon, the favourite ones only.
    pub weapons: BTreeMap<String, u32>,
    pub wins: u32,
    /// Lines the player wrote: (unix seconds, text), the latest last.
    pub lines: Vec<(u64, String)>,
    /// Talks with the bots, by the bot's persona: (unix seconds, the bot's own line, text), the latest last.
    pub talks: BTreeMap<String, Vec<(u64, bool, String)>>,
    /// Moments worth remembering: (unix seconds, text), the latest last.
    pub moments: Vec<(u64, String)>,
    /// The model's notes on the player.
    pub notes: String,
}

impl PlayerMemory {
    /// The weapons the player kills with most, best first.
    pub fn favourite_weapons(&self) -> Vec<&str> {
        let mut w: Vec<(&String, &u32)> = self.weapons.iter().collect();
        w.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        w.into_iter().map(|(name, _)| name.as_str()).collect()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MapRecap {
    pub map: String,
    /// Unix seconds.
    pub ended: u64,
    pub minutes: u32,
    pub winner: Option<String>,
    pub top: Vec<(String, i32)>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Memory {
    pub schema: String,
    pub players: BTreeMap<String, PlayerMemory>,
    /// The last maps, the latest last.
    pub maps: Vec<MapRecap>,
}

impl Default for Memory {
    fn default() -> Self {
        Memory {
            schema: SCHEMA.into(),
            players: BTreeMap::new(),
            maps: Vec::new(),
        }
    }
}

fn push_capped<T>(list: &mut Vec<T>, item: T, cap: usize) {
    list.push(item);
    if list.len() > cap {
        list.drain(..list.len() - cap);
    }
}

/// Whether a player's line is worth remembering: no noise ([`Noise::hard`]), no swearing, no slurs. `names`:
/// nicknames, left out first.
pub fn memorable(line: &str, names: &[String]) -> bool {
    !addressing::noise(line).is_some_and(Noise::hard) && !profanity::has(line, names) && !profanity::slur(line, names)
}

/// The names a map's summary may hold, which are no swearing: its players, the bots they fought, everyone who wrote,
/// and every alias. The memory and the request for notes leave them out of their checks.
pub fn names(s: &MapSummary, aliases: &Aliases) -> Vec<String> {
    let players = s
        .players
        .iter()
        .flat_map(|p| std::iter::once(&p.name).chain(p.vs_bots.iter().map(|(bot, ..)| bot)));
    let writers = s.chat.iter().map(|(_, name, ..)| name);
    let mut names: Vec<String> = players.chain(writers).cloned().chain(aliases.words()).collect();
    names.sort();
    names.dedup();
    names
}

/// Drops the talks beyond [`TALK_BOTS`], the one whose last line is oldest first.
fn cap_talks(talks: &mut BTreeMap<String, Vec<(u64, bool, String)>>) {
    let mut last: Vec<(u64, String)> = talks
        .iter()
        .map(|(bot, lines)| (lines.last().map_or(0, |l| l.0), bot.clone()))
        .collect();
    last.sort();
    for (_, bot) in last.into_iter().take(talks.len().saturating_sub(TALK_BOTS)) {
        talks.remove(&bot);
    }
}

/// Whether a player used the nickname `name`, given trimmed and in lower case.
fn used(p: &PlayerMemory, name: &str) -> bool {
    p.names.iter().any(|n| n.trim().to_lowercase() == name)
}

/// Clears a map's winner and drops its best scores when they go under one of `names` (in lower case).
fn unname(names: &BTreeSet<String>, winner: &mut Option<String>, top: &mut Vec<(String, i32)>) {
    let named = |n: &str| names.contains(&n.trim().to_lowercase());
    if winner.as_deref().is_some_and(named) {
        *winner = None;
    }
    top.retain(|(n, _)| !named(n));
}

/// Players [`Memory::forget`] forgot, for the maps that end after: their keys, the nicknames forgotten by name
/// (without the engine's `(1)`), and every name they used, the last two in lower case.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Forgotten {
    keys: BTreeSet<String>,
    nicknames: BTreeSet<String>,
    names: BTreeSet<String>,
}

impl Forgotten {
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// The keys, in order.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.keys.iter().map(String::as_str)
    }

    /// Takes in players forgotten since.
    pub fn add(&mut self, more: Forgotten) {
        self.keys.extend(more.keys);
        self.nicknames.extend(more.nicknames);
        self.names.extend(more.names);
    }

    /// Leaves them out of a map's summary: the players under their keys or under a nickname forgotten by name, with
    /// the engine's `(1)` before it too, and the winner and the best scores under a name they used.
    pub fn leave_out(&self, s: &mut MapSummary) {
        if self.is_empty() {
            return;
        }
        let mut names = self.names.clone();
        s.players.retain(|p| {
            let gone = self.keys.contains(&p.key) || self.nicknames.contains(&nickname(&p.name));
            if gone {
                names.insert(p.name.trim().to_lowercase());
            }
            !gone
        });
        unname(&names, &mut s.winner, &mut s.top);
    }
}

impl Memory {
    pub fn parse(text: &str) -> Result<Memory, String> {
        let m: Memory = serde_json::from_str(text).map_err(|e| e.to_string())?;
        if m.schema != SCHEMA {
            return Err(format!("schema `{}` is not `{SCHEMA}`", m.schema));
        }
        Ok(m)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// Adds a map that ended at `now` (unix seconds); `aliases`: those of its players.
    pub fn merge(&mut self, s: &MapSummary, aliases: &Aliases, now: u64) {
        let names = names(s, aliases);
        for p in &s.players {
            let m = self.players.entry(p.key.clone()).or_default();
            if m.first_seen == 0 {
                m.first_seen = now;
            }
            m.last_seen = now;
            m.maps += 1;
            m.names.retain(|n| n != &p.name);
            m.names.insert(0, p.name.clone());
            m.names.truncate(NAMES);
            m.kills += p.kills;
            m.deaths += p.deaths;
            m.wins += u32::from(p.won);
            for (bot, killed, died) in &p.vs_bots {
                let duel = m.vs_bots.entry(bot.clone()).or_default();
                duel[0] += killed;
                duel[1] += died;
            }
            for (weapon, n) in &p.weapons {
                *m.weapons.entry(weapon.clone()).or_default() += n;
            }
            let keep: Vec<String> = m
                .favourite_weapons()
                .into_iter()
                .take(WEAPONS)
                .map(String::from)
                .collect();
            m.weapons.retain(|w, _| keep.contains(w));
            for (age, line) in p.lines.iter().filter(|(_, line)| memorable(line, &names)) {
                push_capped(&mut m.lines, (now.saturating_sub(*age as u64), line.clone()), LINES);
            }
            for (age, bot, mine, text) in &p.talk {
                let talk = m.talks.entry(bot.clone()).or_default();
                push_capped(talk, (now.saturating_sub(*age as u64), *mine, text.clone()), TALK_LINES);
            }
            cap_talks(&mut m.talks);
            for moment in &p.moments {
                push_capped(&mut m.moments, (now, moment.clone()), MOMENTS);
            }
        }
        let recap = MapRecap {
            map: s.map.clone(),
            ended: now,
            minutes: s.minutes,
            winner: s.winner.clone(),
            top: s.top.clone(),
        };
        push_capped(&mut self.maps, recap, MAPS);
    }

    /// Forgets players not seen for `days`, and the oldest beyond [`MAX_PLAYERS`].
    pub fn prune(&mut self, now: u64, days: u32) {
        let horizon = now.saturating_sub(u64::from(days) * 86_400);
        self.players.retain(|_, p| p.last_seen >= horizon);
        if self.players.len() > MAX_PLAYERS {
            let mut seen: Vec<(u64, String)> = self.players.iter().map(|(k, p)| (p.last_seen, k.clone())).collect();
            seen.sort();
            for (_, key) in seen.into_iter().take(self.players.len() - MAX_PLAYERS) {
                self.players.remove(&key);
            }
        }
    }

    /// The model's notes, for players it already knows.
    pub fn apply_notes(&mut self, notes: &BTreeMap<String, String>) {
        for (key, note) in notes {
            if let Some(p) = self.players.get_mut(key) {
                p.notes = note.trim().chars().take(NOTES_MAX).collect();
            }
        }
    }

    /// A player by key, else by a name they used ([`Memory::named`]).
    pub fn find(&self, query: &str) -> Option<(&String, &PlayerMemory)> {
        self.players.get_key_value(query.trim()).or_else(|| self.named(query))
    }

    /// The player seen last of those who used `name`, case-insensitive: many keys may share a nickname.
    pub fn named(&self, name: &str) -> Option<(&String, &PlayerMemory)> {
        self.named_except(name, &[])
    }

    /// [`Memory::named`] but for the players under the keys `skip`.
    pub fn named_except(&self, name: &str, skip: &[&str]) -> Option<(&String, &PlayerMemory)> {
        let name = name.trim().to_lowercase();
        self.players
            .iter()
            .filter(|(key, p)| !skip.contains(&key.as_str()) && used(p, &name))
            .max_by_key(|(_, p)| p.last_seen)
    }

    /// Forgets the player under the key `query`, else every player who used the nickname `query` (case-insensitive,
    /// with the engine's `(1)` before it or without, [`same_nickname`]), and their names in the last maps' winners
    /// and best scores: who was forgotten, nobody when the memory has no such player.
    pub fn forget(&mut self, query: &str) -> Forgotten {
        let query = query.trim();
        let mut gone = Forgotten::default();
        let keys: Vec<String> = if self.players.contains_key(query) {
            vec![query.to_string()]
        } else {
            let name = nickname(query);
            let keys: Vec<String> = self
                .players
                .iter()
                .filter(|(_, p)| p.names.iter().any(|n| nickname(n) == name))
                .map(|(key, _)| key.clone())
                .collect();
            if !keys.is_empty() {
                gone.nicknames.insert(name);
            }
            keys
        };
        for key in keys {
            if let Some(p) = self.players.remove(&key) {
                gone.names.extend(p.names.iter().map(|n| n.trim().to_lowercase()));
                gone.keys.insert(key);
            }
        }
        for m in &mut self.maps {
            unname(&gone.names, &mut m.winner, &mut m.top);
        }
        gone
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request::PlayerMap;

    fn summary(winner: bool) -> MapSummary {
        MapSummary {
            map: "crossfire".into(),
            language: "ru".into(),
            minutes: 15,
            winner: winner.then(|| "ATLAS Gamer".into()),
            top: vec![("ATLAS Gamer".into(), 40)],
            players: vec![PlayerMap {
                key: "STEAM_0:0:219579426".into(),
                name: "ATLAS Gamer".into(),
                vs_bots: vec![("DUT9 ATLASA".into(), 4, 1)],
                weapons: vec![("shotgun".into(), 30), ("crowbar".into(), 2)],
                kills: 40,
                deaths: 7,
                won: winner,
                lines: vec![(120.0, "изи".into())],
                moments: vec!["убил DUT9 ATLASA ломом".into()],
                talk: Vec::new(),
            }],
            chat: Vec::new(),
        }
    }

    /// A map where Gordon talked with the bots.
    fn talker(talk: Vec<(f64, String, bool, String)>) -> MapSummary {
        MapSummary {
            map: "gg_cold_rock".into(),
            players: vec![PlayerMap {
                key: "STEAM_0:1:42".into(),
                name: "Gordon".into(),
                talk,
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    fn line(age: f64, bot: &str, mine: bool, text: &str) -> (f64, String, bool, String) {
        (age, bot.into(), mine, text.into())
    }

    #[test]
    fn keys() {
        assert_eq!(player_key("STEAM_0:0:219579426", "x"), "STEAM_0:0:219579426");
        assert_eq!(player_key("VALVE_0:1:5", "x"), "VALVE_0:1:5");
        assert_eq!(player_key("STEAM_ID_LAN", "ATLAS Gamer"), "name:atlas gamer");
        assert_eq!(player_key("STEAM_ID_PENDING", "Bob"), "name:bob");
        assert_eq!(player_key("BOT", " Bob "), "name:bob");
        assert_eq!(player_key("STEAM_0:0:0", "Bob"), "name:bob");
    }

    #[test]
    fn merges_round_trips_and_prunes() {
        let mut m = Memory::default();
        m.merge(&summary(false), &Aliases::default(), 1_000_000);
        m.merge(&summary(true), &Aliases::default(), 1_000_900);
        let p = &m.players["STEAM_0:0:219579426"];
        assert_eq!((p.maps, p.kills, p.wins), (2, 80, 1));
        assert_eq!(p.vs_bots["DUT9 ATLASA"], [8, 2]);
        assert_eq!(p.favourite_weapons(), vec!["shotgun", "crowbar"]);
        assert_eq!(p.lines.last().unwrap(), &(1_000_780, "изи".to_string()));
        assert_eq!((p.first_seen, p.last_seen), (1_000_000, 1_000_900));
        assert_eq!(m.maps.len(), 2);
        let back = Memory::parse(&m.to_json()).unwrap();
        assert_eq!(back, m);
        assert!(Memory::parse("{\"schema\":\"other@1\"}").is_err());
        assert_eq!(m.find("atlas gamer").unwrap().0, "STEAM_0:0:219579426");
        m.apply_notes(&BTreeMap::from([
            ("STEAM_0:0:219579426".to_string(), "  играет с дробовиком  ".to_string()),
            ("STEAM_0:1:1".to_string(), "чужой".to_string()),
        ]));
        assert_eq!(m.players["STEAM_0:0:219579426"].notes, "играет с дробовиком");
        assert_eq!(m.players.len(), 1, "notes never make players up");
        m.prune(1_000_900 + 121 * 86_400, 120);
        assert!(m.players.is_empty());
    }

    #[test]
    fn caps_hold() {
        let mut m = Memory::default();
        for i in 0..20 {
            m.merge(&summary(false), &Aliases::default(), 1000 + i);
        }
        let p = &m.players["STEAM_0:0:219579426"];
        assert_eq!((p.lines.len(), p.moments.len(), p.names.len()), (LINES, MOMENTS, 1));
        assert_eq!(m.maps.len(), MAPS);
        for i in 0..(MAX_PLAYERS + 5) {
            m.players.insert(
                format!("name:p{i}"),
                PlayerMemory {
                    last_seen: 5000 + i as u64,
                    ..Default::default()
                },
            );
        }
        m.prune(6000, 120);
        assert_eq!(m.players.len(), MAX_PLAYERS);
        assert!(!m.players.contains_key("STEAM_0:0:219579426"), "the oldest go first");
    }

    #[test]
    fn talks_are_kept_with_the_last_bots() {
        let mut m = Memory::default();
        m.merge(
            &talker(vec![
                line(300.0, "Plutonium", false, "Привет Плутон!"),
                line(290.0, "Plutonium", true, "здарова"),
                line(250.0, "Kleiner", false, "kleiner, hi"),
            ]),
            &Aliases::default(),
            10_000,
        );
        let talks = &m.players["STEAM_0:1:42"].talks;
        assert_eq!(
            talks["Plutonium"],
            [(9_700, false, "Привет Плутон!".into()), (9_710, true, "здарова".into())]
        );
        assert_eq!(talks["Kleiner"], [(9_750, false, "kleiner, hi".into())]);
        let long = (0..20)
            .map(|i| line(f64::from(100 - i), "Plutonium", i % 2 == 1, &format!("строка {i}")))
            .collect();
        m.merge(&talker(long), &Aliases::default(), 20_000);
        let lines = &m.players["STEAM_0:1:42"].talks["Plutonium"];
        assert_eq!(lines.len(), TALK_LINES);
        assert_eq!(lines.last().unwrap(), &(19_919, true, "строка 19".into()));
        m.merge(
            &talker(vec![
                line(50.0, "Barney", false, "barney?"),
                line(40.0, "Alyx", false, "alyx, where to?"),
            ]),
            &Aliases::default(),
            30_000,
        );
        let bots: Vec<&str> = m.players["STEAM_0:1:42"].talks.keys().map(String::as_str).collect();
        assert_eq!(
            bots,
            ["Alyx", "Barney", "Plutonium"],
            "the talk with Kleiner ended longest ago"
        );
        assert_eq!(Memory::parse(&m.to_json()).unwrap(), m);
        assert!(!m.forget("gordon").is_empty());
        assert!(m.players.is_empty(), "talks go with the player");
    }

    #[test]
    fn a_memory_from_before_talks_loads() {
        let old = r#"{"schema": "lambdabots/chat-memory@1", "players": {"STEAM_0:1:42": {"names": ["Gordon"],
            "first_seen": 1000, "last_seen": 2000, "maps": 3, "kills": 40, "deaths": 7, "vs_bots": {"Kleiner": [4, 1]},
            "weapons": {"crowbar": 30}, "wins": 1, "lines": [[1900, "изи"]], "moments": [], "notes": "любит лом"}},
            "maps": []}"#;
        let m = Memory::parse(old).unwrap();
        let p = &m.players["STEAM_0:1:42"];
        assert!(p.talks.is_empty());
        assert_eq!(
            (p.maps, p.vs_bots["Kleiner"], p.notes.as_str()),
            (3, [4, 1], "любит лом")
        );
    }

    #[test]
    fn noise_and_swearing_are_not_remembered() {
        let mut s = talker(Vec::new());
        s.players[0].lines = [
            "ахахахах",
            "RUUUUN!!!1",
            "gg_cold_rock",
            "бляяяяяя",
            "хохлы",
            "ок",
            "где рельсы?",
            "_FUCK_ опять тут",
        ]
        .into_iter()
        .map(|l| (10.0, l.to_string()))
        .collect();
        s.chat.push((5.0, "_FUCK_".into(), "всем привет".into(), false));
        let mut m = Memory::default();
        m.merge(&s, &Aliases::default(), 1000);
        let kept: Vec<&str> = m.players["STEAM_0:1:42"]
            .lines
            .iter()
            .map(|(_, l)| l.as_str())
            .collect();
        assert_eq!(
            kept,
            ["ок", "где рельсы?", "_FUCK_ опять тут"],
            "a nickname is no swearing"
        );
    }

    /// A player who used `names`, seen last at `last_seen`.
    fn seen(names: &[&str], last_seen: u64, notes: &str) -> PlayerMemory {
        PlayerMemory {
            names: names.iter().map(|n| n.to_string()).collect(),
            last_seen,
            notes: notes.into(),
            ..Default::default()
        }
    }

    #[test]
    fn a_name_finds_the_player_seen_last() {
        let mut m = Memory::default();
        m.players
            .insert("STEAM_0:0:3".into(), seen(&["Nordwind"], 100, "oldest"));
        m.players
            .insert("STEAM_0:1:7".into(), seen(&["Nordwind"], 300, "newest"));
        m.players
            .insert("name:nordwind".into(), seen(&["Nordwind"], 200, "older"));
        assert_eq!(
            m.named(" NORDWIND ").map(|(key, _)| key.as_str()),
            Some("STEAM_0:1:7"),
            "neither the first key nor the last"
        );
        assert_eq!(m.find("nordwind").map(|(_, p)| p.notes.as_str()), Some("newest"));
        assert_eq!(
            m.find("STEAM_0:0:3").map(|(_, p)| p.notes.as_str()),
            Some("oldest"),
            "a key first"
        );
        assert!(m.named("Barney").is_none());
        assert_eq!(
            m.named_except("Nordwind", &["STEAM_0:1:7"])
                .map(|(key, _)| key.as_str()),
            Some("name:nordwind"),
            "the one seen last but for the keys skipped"
        );
        assert!(
            m.named_except("Nordwind", &["STEAM_0:0:3", "STEAM_0:1:7", "name:nordwind"])
                .is_none()
        );
    }

    #[test]
    fn nicknames_the_engine_gives() {
        for (a, b) in [
            ("(1)Gordon", "gordon"),
            ("Gordon", "(12)GORDON"),
            ("(1)Gordon", "(2)Gordon"),
        ] {
            assert!(same_nickname(a, b), "{a} {b}");
        }
        for (a, b) in [
            ("Gordon", "Gordon2"),
            ("(x)Gordon", "Gordon"),
            ("(123)Gordon", "Gordon"),
            ("(1)", "(2)"),
        ] {
            assert!(!same_nickname(a, b), "{a} {b}");
        }
        for name in ["Player", "player", " PLAYER ", "(1)Player", "(2)player"] {
            assert!(default_nickname(name), "{name}");
        }
        for name in ["Player2", "[TAG]Player", "(1)", "Players"] {
            assert!(!default_nickname(name), "{name}");
        }
    }

    #[test]
    fn a_nickname_forgets_every_player_who_used_it() {
        let mut m = Memory::default();
        m.players
            .insert("STEAM_0:0:7".into(), seen(&["Gordon"], 100, "first visit"));
        m.players
            .insert("STEAM_0:1:42".into(), seen(&["Freeman", "gordon"], 200, "second visit"));
        m.players
            .insert("STEAM_0:0:9".into(), seen(&["Barney"], 300, "the guard"));
        m.maps = ["crossfire", "stalkyard"]
            .into_iter()
            .map(|map| MapRecap {
                map: map.into(),
                winner: Some(if map == "crossfire" { "Freeman" } else { "Barney" }.into()),
                top: vec![("Freeman".into(), 30), ("Barney".into(), 20), ("Gordon".into(), 10)],
                ..Default::default()
            })
            .collect();
        let gone = m.forget(" GORDON ");
        assert_eq!(gone.keys().collect::<Vec<_>>(), ["STEAM_0:0:7", "STEAM_0:1:42"]);
        assert_eq!(m.players.keys().collect::<Vec<_>>(), ["STEAM_0:0:9"]);
        assert_eq!(
            m.maps.iter().map(|r| r.winner.as_deref()).collect::<Vec<_>>(),
            [None, Some("Barney")],
            "every name they used goes from the last maps"
        );
        assert!(m.maps.iter().all(|r| r.top == [("Barney".to_string(), 20)]));
        assert!(m.forget("gordon").is_empty(), "nobody left");
        let gone = m.forget("STEAM_0:0:9");
        assert_eq!(gone.keys().collect::<Vec<_>>(), ["STEAM_0:0:9"]);
        assert!(m.players.is_empty());
    }

    /// A map Gordon won against Barney.
    fn gordon_won() -> MapSummary {
        let player = |key: &str, name: &str| PlayerMap {
            key: key.into(),
            name: name.into(),
            ..Default::default()
        };
        MapSummary {
            map: "stalkyard".into(),
            winner: Some("Gordon".into()),
            top: vec![("Gordon".into(), 30), ("Barney".into(), 20)],
            players: vec![player("STEAM_0:1:42", "Gordon"), player("STEAM_0:0:9", "Barney")],
            ..Default::default()
        }
    }

    fn keys_of(s: &MapSummary) -> Vec<&str> {
        s.players.iter().map(|p| p.key.as_str()).collect()
    }

    #[test]
    fn a_player_forgotten_leaves_the_maps() {
        let mut m = Memory::default();
        m.merge(&gordon_won(), &Aliases::default(), 1000);
        let gone = m.forget("STEAM_0:1:42");
        assert_eq!(m.maps[0].winner, None);
        assert_eq!(m.maps[0].top, [("Barney".to_string(), 20)]);
        let mut s = gordon_won();
        gone.leave_out(&mut s);
        assert_eq!(keys_of(&s), ["STEAM_0:0:9"]);
        assert_eq!((s.winner, s.top), (None, vec![("Barney".to_string(), 20)]));
        let mut kept = gordon_won();
        Forgotten::default().leave_out(&mut kept);
        assert_eq!(kept, gordon_won(), "nobody forgotten");
    }

    #[test]
    fn a_steamid_forgets_that_player_alone() {
        let mut m = Memory::default();
        m.players
            .insert("STEAM_0:0:7".into(), seen(&["Gordon"], 100, "first visit"));
        m.players
            .insert("STEAM_0:1:42".into(), seen(&["Gordon"], 200, "second visit"));
        let mut gone = m.forget("STEAM_0:1:42");
        assert_eq!(gone.keys().collect::<Vec<_>>(), ["STEAM_0:1:42"]);
        assert_eq!(m.players.keys().collect::<Vec<_>>(), ["STEAM_0:0:7"]);
        let mut s = gordon_won();
        s.players[0].key = "STEAM_0:1:5".into();
        let mut by_key = s.clone();
        gone.leave_out(&mut by_key);
        assert_eq!(keys_of(&by_key), ["STEAM_0:1:5", "STEAM_0:0:9"], "the SteamID alone");
        assert_eq!(
            (by_key.winner, by_key.top),
            (None, vec![("Barney".to_string(), 20)]),
            "but no win or score under a name they used"
        );
        gone.add(m.forget("gordon"));
        gone.leave_out(&mut s);
        assert_eq!(keys_of(&s), ["STEAM_0:0:9"], "whoever plays under a nickname forgotten");
    }

    #[test]
    fn a_nickname_forgotten_holds_for_one_who_reconnects() {
        let mut m = Memory::default();
        m.players
            .insert("STEAM_0:1:42".into(), seen(&["Gordon"], 100, "first visit"));
        m.players
            .insert("STEAM_0:0:5".into(), seen(&["(1)Gordon"], 200, "after a crash"));
        m.players
            .insert("STEAM_0:0:9".into(), seen(&["Barney"], 300, "the guard"));
        let gone = m.forget("gordon");
        assert_eq!(
            gone.keys().collect::<Vec<_>>(),
            ["STEAM_0:0:5", "STEAM_0:1:42"],
            "one who used it only after a reconnect too"
        );
        assert_eq!(m.players.keys().collect::<Vec<_>>(), ["STEAM_0:0:9"]);
        let player = |key: &str, name: &str| PlayerMap {
            key: key.into(),
            name: name.into(),
            ..Default::default()
        };
        let mut s = MapSummary {
            map: "stalkyard".into(),
            winner: Some("(1)Gordon".into()),
            top: vec![("(1)Gordon".into(), 30), ("Barney".into(), 20)],
            players: vec![
                player("STEAM_0:1:777", "(1)Gordon"),
                player("STEAM_0:0:9", "Barney"),
                player("name:(1)eli", "(1)Eli"),
            ],
            ..Default::default()
        };
        gone.leave_out(&mut s);
        assert_eq!(
            keys_of(&s),
            ["STEAM_0:0:9", "name:(1)eli"],
            "back as (1)Gordon under a new SteamID"
        );
        assert_eq!((s.winner, s.top), (None, vec![("Barney".to_string(), 20)]));
    }
}
