//! What happened on this map, as everyone saw it in the kill feed, the chat and the scoreboard, and the moments
//! worth a word: a nemesis, a revenge, a crowbar kill, a player blowing themselves up, a streak, a rage quit.

use std::collections::{BTreeMap, VecDeque};

use lb_core::time::SimTime;

use crate::lang::is_explosive;

/// Entries kept; older ones are summed up in the scores only.
pub const KEEP: usize = 256;
/// Kills within `MULTIKILL_WINDOW` seconds that make a multikill.
pub const MULTIKILL: u32 = 3;
const MULTIKILL_WINDOW: f64 = 6.0;
/// Deaths in a row to one killer that make them a nemesis (and every two more after); a revenge after as many is
/// worth a word.
pub const NEMESIS: u32 = 3;
/// Kills without dying from which a streak is worth a word, the bot's own or another player's.
pub const STREAK_SPOKEN: u32 = 10;
const STREAK_STEP: u32 = 5;
/// A player leaving this soon after dying `RAGE_DEATHS` times in a row rage quits.
const RAGE_WINDOW: f64 = 30.0;
const RAGE_DEATHS: u32 = 3;

/// A player as the chat sees them: slot, connection (`userid`, unique while the server runs) and name at the time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Who {
    pub slot: u8,
    pub userid: i32,
    pub name: String,
    /// One of our bots.
    pub bot: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Kill {
        killer: Who,
        victim: Who,
        weapon: String,
    },
    /// Killed themselves: their own explosive, `kill`.
    Suicide {
        victim: Who,
        weapon: String,
    },
    /// Killed by the world: a fall, water, a door.
    Died {
        victim: Who,
        weapon: String,
    },
    Chat {
        from: Who,
        text: String,
        team: bool,
    },
    Join {
        who: Who,
    },
    Leave {
        who: Who,
    },
    Rename {
        who: Who,
        old: String,
    },
    /// GunGame: a player went up to `level`.
    Level {
        who: Who,
        level: i32,
    },
    /// GunGame: the scoreboard's first line changed.
    Leader {
        who: Who,
    },
    MatchEnd {
        winner: Option<Who>,
    },
}

impl Event {
    /// The players in the event.
    pub fn people(&self) -> Vec<&Who> {
        match self {
            Event::Kill { killer, victim, .. } => vec![killer, victim],
            Event::Suicide { victim: who, .. }
            | Event::Died { victim: who, .. }
            | Event::Chat { from: who, .. }
            | Event::Join { who }
            | Event::Leave { who }
            | Event::Rename { who, .. }
            | Event::Level { who, .. }
            | Event::Leader { who } => vec![who],
            Event::MatchEnd { winner } => winner.iter().collect(),
        }
    }

    /// Players the event is about.
    pub fn involves(&self, userid: i32) -> bool {
        match self {
            Event::Kill { killer, victim, .. } => killer.userid == userid || victim.userid == userid,
            Event::Suicide { victim, .. } | Event::Died { victim, .. } => victim.userid == userid,
            Event::Chat { from: who, .. }
            | Event::Join { who }
            | Event::Leave { who }
            | Event::Rename { who, .. }
            | Event::Level { who, .. }
            | Event::Leader { who } => who.userid == userid,
            Event::MatchEnd { winner } => winner.as_ref().is_some_and(|w| w.userid == userid),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub t: SimTime,
    pub event: Event,
}

/// Moments worth a word, found while the journal takes events in.
#[derive(Clone, Debug, PartialEq)]
pub enum Notable {
    /// `victim` died `times` times in a row to `killer`.
    Nemesis { killer: Who, victim: Who, times: u32 },
    /// A crowbar kill, outside GunGame.
    Humiliation { killer: Who, victim: Who },
    /// Blew themselves up.
    OwnBlast { victim: Who, weapon: String },
    /// Killed the player who killed them last; `run`: how many times in a row that player had killed them.
    Revenge { killer: Who, victim: Who, run: u32 },
    /// `count` kills within a few seconds; `humans`: how many of the victims were humans.
    Multikill { killer: Who, count: u32, humans: u32 },
    /// `count` kills without dying; `humans`: how many of the victims were humans.
    Streak { killer: Who, count: u32, humans: u32 },
    /// Left soon after dying `deaths` times in a row.
    RageQuit { who: Who, deaths: u32 },
}

impl Notable {
    /// The players in the moment, the one it is about first.
    pub fn people(&self) -> Vec<&Who> {
        match self {
            Notable::Nemesis { killer, victim, .. }
            | Notable::Humiliation { killer, victim }
            | Notable::Revenge { killer, victim, .. } => vec![killer, victim],
            Notable::Multikill { killer: who, .. }
            | Notable::Streak { killer: who, .. }
            | Notable::OwnBlast { victim: who, .. }
            | Notable::RageQuit { who, .. } => vec![who],
        }
    }
}

/// One player's map so far.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Score {
    pub name: String,
    pub bot: bool,
    pub kills: u32,
    pub deaths: u32,
    pub suicides: u32,
    /// Kills since the last death.
    pub streak: u32,
    /// Humans among the streak's victims.
    pub streak_humans: u32,
    /// Deaths since the last kill.
    pub deaths_in_row: u32,
    /// Who killed this player last, and how many times in a row; over once this player kills them back.
    pub run: Option<(i32, u32)>,
    pub last_death: Option<SimTime>,
    /// Kills of the last few seconds: when, and whether the victim was human.
    recent_kills: VecDeque<(SimTime, bool)>,
    /// Kills by weapon.
    pub weapons: BTreeMap<String, u32>,
    /// Kills of each other player, by `userid`.
    pub victims: BTreeMap<i32, u32>,
    pub best_level: i32,
}

#[derive(Clone, Debug)]
pub struct Journal {
    pub map: String,
    pub started: SimTime,
    /// GunGame: the crowbar is the warmup's and the last level's weapon, so a crowbar kill is no humiliation. Set
    /// before each push: the mode is found out after the map starts.
    pub gungame: bool,
    entries: VecDeque<Entry>,
    scores: BTreeMap<i32, Score>,
}

impl Journal {
    pub fn new(map: &str, started: SimTime) -> Journal {
        Journal {
            map: map.to_string(),
            started,
            gungame: false,
            entries: VecDeque::new(),
            scores: BTreeMap::new(),
        }
    }

    pub fn entries(&self) -> impl DoubleEndedIterator<Item = &Entry> {
        self.entries.iter()
    }

    /// Scores of everyone who played this map, by `userid`, including those who left.
    pub fn scores(&self) -> &BTreeMap<i32, Score> {
        &self.scores
    }

    pub fn score(&self, userid: i32) -> Option<&Score> {
        self.scores.get(&userid)
    }

    /// Kills between `a` and `b` this map: (a killed b, b killed a).
    pub fn duel(&self, a: i32, b: i32) -> (u32, u32) {
        let killed = |x: i32, y: i32| {
            self.scores
                .get(&x)
                .and_then(|s| s.victims.get(&y))
                .copied()
                .unwrap_or(0)
        };
        (killed(a, b), killed(b, a))
    }

    /// Whether `a` and `b` killed one another lately.
    pub fn fought(&self, a: i32, b: i32, since: SimTime) -> bool {
        self.entries.iter().rev().take_while(|e| e.t >= since).any(|e| {
            matches!(&e.event, Event::Kill { killer, victim, .. }
                if (killer.userid == a && victim.userid == b) || (killer.userid == b && victim.userid == a))
        })
    }

    fn score_mut(&mut self, who: &Who) -> &mut Score {
        let s = self.scores.entry(who.userid).or_default();
        s.name.clone_from(&who.name);
        s.bot = who.bot;
        s
    }

    /// Takes an event in; returns what made it notable.
    pub fn push(&mut self, t: SimTime, event: Event) -> Vec<Notable> {
        let mut notable = Vec::new();
        match &event {
            Event::Kill { killer, victim, weapon } => {
                let human = !victim.bot;
                let k = self.score_mut(killer);
                k.kills += 1;
                k.streak += 1;
                k.streak_humans += u32::from(human);
                k.deaths_in_row = 0;
                *k.weapons.entry(weapon.to_ascii_lowercase()).or_default() += 1;
                *k.victims.entry(victim.userid).or_default() += 1;
                k.recent_kills.retain(|&(at, _)| t.since(at) <= MULTIKILL_WINDOW);
                k.recent_kills.push_back((t, human));
                let quick = k.recent_kills.len() as u32;
                let quick_humans = k.recent_kills.iter().filter(|&&(_, h)| h).count() as u32;
                let (streak, streak_humans) = (k.streak, k.streak_humans);
                let revenge = k.run.take_if(|(id, _)| *id == victim.userid).map(|(_, run)| run);
                let v = self.score_mut(victim);
                v.deaths += 1;
                v.streak = 0;
                v.streak_humans = 0;
                v.deaths_in_row += 1;
                v.last_death = Some(t);
                let times = match v.run {
                    Some((id, n)) if id == killer.userid => n + 1,
                    _ => 1,
                };
                v.run = Some((killer.userid, times));
                if times >= NEMESIS && (times - NEMESIS).is_multiple_of(2) {
                    notable.push(Notable::Nemesis {
                        killer: killer.clone(),
                        victim: victim.clone(),
                        times,
                    });
                }
                if weapon.eq_ignore_ascii_case("crowbar") && !self.gungame {
                    notable.push(Notable::Humiliation {
                        killer: killer.clone(),
                        victim: victim.clone(),
                    });
                }
                if let Some(run) = revenge {
                    notable.push(Notable::Revenge {
                        killer: killer.clone(),
                        victim: victim.clone(),
                        run,
                    });
                }
                if quick >= MULTIKILL {
                    notable.push(Notable::Multikill {
                        killer: killer.clone(),
                        count: quick,
                        humans: quick_humans,
                    });
                }
                if streak.is_multiple_of(STREAK_STEP) {
                    notable.push(Notable::Streak {
                        killer: killer.clone(),
                        count: streak,
                        humans: streak_humans,
                    });
                }
            }
            Event::Suicide { victim, weapon } | Event::Died { victim, weapon } => {
                let own = matches!(event, Event::Suicide { .. });
                let v = self.score_mut(victim);
                v.deaths += 1;
                v.suicides += u32::from(own);
                v.streak = 0;
                v.streak_humans = 0;
                v.deaths_in_row += 1;
                v.last_death = Some(t);
                if own && is_explosive(weapon) {
                    notable.push(Notable::OwnBlast {
                        victim: victim.clone(),
                        weapon: weapon.clone(),
                    });
                }
            }
            Event::Leave { who } => {
                if let Some(s) = self.scores.get(&who.userid)
                    && !who.bot
                    && s.deaths_in_row >= RAGE_DEATHS
                    && s.last_death.is_some_and(|d| t.since(d) <= RAGE_WINDOW)
                {
                    notable.push(Notable::RageQuit {
                        who: who.clone(),
                        deaths: s.deaths_in_row,
                    });
                }
            }
            Event::Level { who, level } => {
                let s = self.score_mut(who);
                s.best_level = s.best_level.max(*level);
            }
            Event::Join { who } | Event::Rename { who, .. } | Event::Leader { who } | Event::Chat { from: who, .. } => {
                self.score_mut(who);
            }
            Event::MatchEnd { .. } => {}
        }
        self.entries.push_back(Entry { t, event });
        while self.entries.len() > KEEP {
            self.entries.pop_front();
        }
        notable
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn who(slot: u8, name: &str, bot: bool) -> Who {
        Who {
            slot,
            userid: i32::from(slot) + 100,
            name: name.into(),
            bot,
        }
    }

    fn kill(j: &mut Journal, t: f64, k: &Who, v: &Who, w: &str) -> Vec<Notable> {
        j.push(
            SimTime(t),
            Event::Kill {
                killer: k.clone(),
                victim: v.clone(),
                weapon: w.into(),
            },
        )
    }

    #[test]
    fn nemesis_revenge_and_humiliation() {
        let (atlas, bot) = (who(1, "ATLAS Gamer", false), who(2, "DUT9 ATLASA", true));
        let mut j = Journal::new("crossfire", SimTime(0.0));
        assert!(kill(&mut j, 1.0, &atlas, &bot, "shotgun").is_empty());
        assert!(kill(&mut j, 20.0, &atlas, &bot, "shotgun").is_empty());
        let n = kill(&mut j, 40.0, &atlas, &bot, "crowbar");
        assert!(n.contains(&Notable::Nemesis {
            killer: atlas.clone(),
            victim: bot.clone(),
            times: 3
        }));
        assert!(n.iter().any(|n| matches!(n, Notable::Humiliation { .. })));
        assert!(
            kill(&mut j, 60.0, &atlas, &bot, "shotgun").is_empty(),
            "the 4th says nothing new"
        );
        let n = kill(&mut j, 80.0, &bot, &atlas, "crossbow");
        assert_eq!(
            n,
            vec![Notable::Revenge {
                killer: bot.clone(),
                victim: atlas.clone(),
                run: 4
            }]
        );
        assert_eq!(j.duel(atlas.userid, bot.userid), (4, 1));
        assert!(j.fought(bot.userid, atlas.userid, SimTime(70.0)));
        assert!(!j.fought(bot.userid, atlas.userid, SimTime(81.0)));
        assert_eq!(j.score(atlas.userid).unwrap().weapons["shotgun"], 3);
    }

    #[test]
    fn a_revenge_tells_the_run_and_ends_it() {
        let (h, bot, x) = (who(1, "h", false), who(2, "bot", true), who(3, "x", false));
        let mut j = Journal::new("x", SimTime(0.0));
        for t in [1.0, 20.0, 40.0] {
            kill(&mut j, t, &h, &bot, "shotgun");
        }
        let revenge = Notable::Revenge {
            killer: bot.clone(),
            victim: h.clone(),
            run: 3,
        };
        assert_eq!(kill(&mut j, 60.0, &bot, &h, "shotgun"), vec![revenge]);
        assert!(kill(&mut j, 80.0, &bot, &h, "shotgun").is_empty(), "one revenge a run");
        kill(&mut j, 100.0, &h, &bot, "shotgun");
        kill(&mut j, 120.0, &h, &bot, "shotgun");
        let n = kill(&mut j, 140.0, &h, &bot, "shotgun");
        assert!(
            n.contains(&Notable::Nemesis {
                killer: h.clone(),
                victim: bot.clone(),
                times: 3
            }),
            "the bot got one back, so the count began again: {n:?}"
        );
        kill(&mut j, 160.0, &x, &bot, "shotgun");
        assert!(
            kill(&mut j, 180.0, &bot, &h, "shotgun").is_empty(),
            "x killed the bot last"
        );
    }

    #[test]
    fn no_humiliation_in_gungame() {
        let (h, bot) = (who(1, "h", false), who(2, "bot", true));
        let mut j = Journal::new("gg_cold_rock", SimTime(0.0));
        j.gungame = true;
        assert!(kill(&mut j, 1.0, &h, &bot, "crowbar").is_empty());
        j.gungame = false;
        assert_eq!(
            kill(&mut j, 30.0, &h, &bot, "crowbar"),
            vec![Notable::Humiliation {
                killer: h.clone(),
                victim: bot.clone()
            }]
        );
    }

    #[test]
    fn multikills_and_streaks_count_the_humans_killed() {
        let (bot, other, h1, h2) = (
            who(1, "bot", true),
            who(2, "other", true),
            who(3, "h1", false),
            who(4, "h2", false),
        );
        let mut j = Journal::new("x", SimTime(0.0));
        kill(&mut j, 1.0, &bot, &other, "9mmAR");
        kill(&mut j, 2.0, &bot, &h1, "9mmAR");
        let n = kill(&mut j, 3.0, &bot, &other, "9mmAR");
        assert!(
            n.contains(&Notable::Multikill {
                killer: bot.clone(),
                count: 3,
                humans: 1
            }),
            "{n:?}"
        );
        kill(&mut j, 20.0, &bot, &h2, "gauss");
        let n = kill(&mut j, 40.0, &bot, &other, "gauss");
        assert!(
            n.contains(&Notable::Streak {
                killer: bot.clone(),
                count: 5,
                humans: 2
            }),
            "{n:?}"
        );
        j.push(
            SimTime(50.0),
            Event::Died {
                victim: bot.clone(),
                weapon: "world".into(),
            },
        );
        let s = j.score(bot.userid).unwrap();
        assert_eq!((s.streak, s.streak_humans), (0, 0), "a death ends the streak");
        let n: Vec<Notable> = [60.0, 61.0, 62.0, 80.0, 100.0]
            .into_iter()
            .flat_map(|t| kill(&mut j, t, &bot, &other, "gauss"))
            .collect();
        assert!(
            n.contains(&Notable::Multikill {
                killer: bot.clone(),
                count: 3,
                humans: 0
            }),
            "{n:?}"
        );
        assert!(
            n.contains(&Notable::Streak {
                killer: bot.clone(),
                count: 5,
                humans: 0
            }),
            "{n:?}"
        );
    }

    #[test]
    fn multikills_streaks_blasts_and_rage_quits() {
        let (a, b, c, d) = (
            who(1, "a", true),
            who(2, "b", false),
            who(3, "c", false),
            who(4, "d", false),
        );
        let mut j = Journal::new("x", SimTime(0.0));
        kill(&mut j, 1.0, &a, &b, "9mmAR");
        kill(&mut j, 3.0, &a, &c, "9mmAR");
        let n = kill(&mut j, 5.0, &a, &d, "9mmAR");
        assert!(n.contains(&Notable::Multikill {
            killer: a.clone(),
            count: 3,
            humans: 3
        }));
        kill(&mut j, 30.0, &a, &b, "gauss");
        let n = kill(&mut j, 60.0, &a, &b, "gauss");
        assert!(n.contains(&Notable::Streak {
            killer: a.clone(),
            count: 5,
            humans: 5
        }));
        let n = j.push(
            SimTime(70.0),
            Event::Suicide {
                victim: c.clone(),
                weapon: "satchel".into(),
            },
        );
        assert_eq!(
            n,
            vec![Notable::OwnBlast {
                victim: c.clone(),
                weapon: "satchel".into()
            }]
        );
        let n = j.push(SimTime(75.0), Event::Leave { who: b.clone() });
        assert_eq!(
            n,
            vec![Notable::RageQuit {
                who: b.clone(),
                deaths: 3
            }]
        );
        let n = j.push(SimTime(200.0), Event::Leave { who: d.clone() });
        assert!(n.is_empty(), "one death long ago is no rage quit");
    }

    #[test]
    fn keeps_the_last_entries_and_every_score() {
        let (a, b) = (who(1, "a", false), who(2, "b", false));
        let mut j = Journal::new("x", SimTime(0.0));
        for i in 0..(KEEP + 50) {
            kill(&mut j, i as f64, &a, &b, "crowbar");
        }
        assert_eq!(j.entries().count(), KEEP);
        assert_eq!(j.score(a.userid).unwrap().kills, (KEEP + 50) as u32);
    }
}
