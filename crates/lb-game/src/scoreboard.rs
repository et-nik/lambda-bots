//! The public scoreboard: frags, deaths and teams of every player.

use crate::messages::GameMsg;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScoreEntry {
    pub frags: i32,
    pub deaths: i32,
    pub team: String,
    pub team_number: i32,
}

#[derive(Clone, Debug, Default)]
pub struct Scoreboard {
    pub entries: Vec<ScoreEntry>,
}

impl Scoreboard {
    pub fn resize(&mut self, max_clients: usize) {
        self.entries.resize(max_clients + 1, ScoreEntry::default());
    }

    pub fn apply(&mut self, msg: &GameMsg) {
        match msg {
            GameMsg::ScoreInfo {
                slot,
                frags,
                deaths,
                team,
            } => {
                if let Some(e) = self.entries.get_mut(*slot as usize) {
                    e.frags = *frags;
                    e.deaths = *deaths;
                    e.team_number = *team;
                }
            }
            GameMsg::TeamInfo { slot, team } => {
                if let Some(e) = self.entries.get_mut(*slot as usize) {
                    e.team = team.clone();
                }
            }
            _ => {}
        }
    }

    /// Frags read from entity state cover scoreboard writes by other plugins (AMXX), which
    /// Metamod message hooks cannot see.
    pub fn set_frags(&mut self, slot: u8, frags: i32) {
        if let Some(e) = self.entries.get_mut(slot as usize) {
            e.frags = frags;
        }
    }

    pub fn clear_slot(&mut self, slot: u8) {
        if let Some(e) = self.entries.get_mut(slot as usize) {
            *e = ScoreEntry::default();
        }
    }
}
