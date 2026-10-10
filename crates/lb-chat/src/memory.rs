//! What the bots remember of players between maps and restarts, kept by the chat worker in `data/chat/memory.json`:
//! the names a player used, the score against each bot, favourite weapons, wins, a few lines they wrote, their talks
//! with the bots, moments worth remembering, the model's notes, and the last maps.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::addressing::{self, Noise};
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

/// The names in a map's summary: its players, the bots they fought, everyone who wrote.
fn names(s: &MapSummary) -> Vec<String> {
    let players = s
        .players
        .iter()
        .flat_map(|p| std::iter::once(&p.name).chain(p.vs_bots.iter().map(|(bot, ..)| bot)));
    let writers = s.chat.iter().map(|(_, name, ..)| name);
    let mut names: Vec<String> = players.chain(writers).cloned().collect();
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

    /// Adds a map that ended at `now` (unix seconds).
    pub fn merge(&mut self, s: &MapSummary, now: u64) {
        let names = names(s);
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

    /// A player by key or by any name they used, case-insensitive.
    pub fn find(&self, query: &str) -> Option<(&String, &PlayerMemory)> {
        let q = query.trim().to_lowercase();
        self.players.get_key_value(query.trim()).or_else(|| {
            self.players
                .iter()
                .find(|(_, p)| p.names.iter().any(|n| n.to_lowercase() == q))
        })
    }

    pub fn forget(&mut self, query: &str) -> Option<String> {
        let key = self.find(query)?.0.clone();
        self.players.remove(&key);
        Some(key)
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
        m.merge(&summary(false), 1_000_000);
        m.merge(&summary(true), 1_000_900);
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
            m.merge(&summary(false), 1000 + i);
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
        m.merge(&talker(long), 20_000);
        let lines = &m.players["STEAM_0:1:42"].talks["Plutonium"];
        assert_eq!(lines.len(), TALK_LINES);
        assert_eq!(lines.last().unwrap(), &(19_919, true, "строка 19".into()));
        m.merge(
            &talker(vec![
                line(50.0, "Barney", false, "barney?"),
                line(40.0, "Alyx", false, "alyx, where to?"),
            ]),
            30_000,
        );
        let bots: Vec<&str> = m.players["STEAM_0:1:42"].talks.keys().map(String::as_str).collect();
        assert_eq!(
            bots,
            ["Alyx", "Barney", "Plutonium"],
            "the talk with Kleiner ended longest ago"
        );
        assert_eq!(Memory::parse(&m.to_json()).unwrap(), m);
        assert!(m.forget("gordon").is_some());
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
            "DIIIIIEEEEE!!!!1",
            "gg_cold_rock",
            "бляяя",
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
        m.merge(&s, 1000);
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
}
