//! What the bots remember of players between maps and restarts, kept by the chat worker in `data/chat/memory.json`:
//! the names a player used, the score against each bot, favourite weapons, wins, a few lines they wrote, moments
//! worth remembering, the model's notes, and the last maps.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::request::MapSummary;

pub const SCHEMA: &str = "lambdabots/chat-memory@1";
const NAMES: usize = 5;
const LINES: usize = 10;
const MOMENTS: usize = 6;
const WEAPONS: usize = 5;
const MAPS: usize = 3;
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
            for (age, line) in &p.lines {
                push_capped(&mut m.lines, (now.saturating_sub(*age as u64), line.clone()), LINES);
            }
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
            }],
            chat: Vec::new(),
        }
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
}
