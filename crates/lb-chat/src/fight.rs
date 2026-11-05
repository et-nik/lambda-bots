//! Firefights of a bot, as the bot itself saw them: it and an enemy shooting at each other, each in the other's
//! sight, for at least [`FIGHT_MIN`] seconds of the part where both shoot, and how that ended. The runtime looks at
//! every bot [`LOOK`] times a second ([`Fights::look`]) and passes on the kill feed ([`Fights::death`]); the fights
//! that ended come out of [`Fights::take`].
//!
//! An exchange opens with a side's first shot and is forgotten unless the other answers before the first has been
//! silent for [`PAUSE`]. The fight runs from the answering side's first shot to a death, or to the last shot of the
//! side that went silent for longer than [`PAUSE`].

use std::collections::BTreeMap;

use lb_core::time::SimTime;

use crate::journal::Who;

/// Seconds of both shooting at each other that make a fight.
pub const FIGHT_MIN: f64 = 3.0;
/// A side silent longer than this ends the fight, or has its shot forgotten when nobody answered it.
pub const PAUSE: f64 = 3.0;
/// After one of the two died by the other, the other dying by them this soon after is a trade.
pub const TRADE: f64 = 2.0;
/// Seconds of a fight from which it is a long one.
pub const LONG: f64 = 6.0;
/// The bot's health at most, after a fight it won or broke off, that makes the fight a close one.
pub const CLOSE_HP: i32 = 20;
/// Seconds between two looks at the bots.
pub const LOOK: f64 = 0.05;
/// The bot pulling back this close to when the fight went quiet is what ended it.
const RETREAT_LAG: f64 = 1.0;
/// Seconds one side may go on after the other: shooting and still have stopped with it, in sight and still have
/// gone with it. The bot's health is taken this long after the last shot at most.
const TOGETHER: f64 = 0.5;

/// How a fight ended.
#[derive(Clone, Debug, PartialEq)]
pub enum FightEnd {
    /// The bot killed the enemy.
    Won,
    /// The enemy killed the bot.
    Lost,
    /// Both died, each by the other.
    Traded,
    /// Another player killed the enemy.
    Stolen { by: Who },
    /// A teammate of the bot killed the enemy: the runtime, which knows the teams, tells it from [`FightEnd::Stolen`].
    Helped { by: Who },
    /// Another player killed the bot, one it was not in a fight with.
    Interrupted { by: Who },
    /// The enemy stopped shooting, or went out of the bot's sight.
    Fled,
    /// The bot pulled back.
    Retreated,
}

/// A fight that ended.
#[derive(Clone, Debug, PartialEq)]
pub struct Fight {
    pub bot: Who,
    pub foe: Who,
    /// Seconds both were shooting at each other.
    pub secs: f32,
    /// The bot's health at the end; 0 when it died.
    pub health: i32,
    pub end: FightEnd,
}

impl Fight {
    /// A long fight, a trade, or one the bot won or broke off with little health left.
    pub fn bright(&self) -> bool {
        f64::from(self.secs) >= LONG
            || self.end == FightEnd::Traded
            || (matches!(self.end, FightEnd::Won | FightEnd::Fled | FightEnd::Retreated) && self.health <= CLOSE_HP)
    }

    /// The third player who ended it.
    pub fn by(&self) -> Option<&Who> {
        match &self.end {
            FightEnd::Stolen { by } | FightEnd::Helped { by } | FightEnd::Interrupted { by } => Some(by),
            _ => None,
        }
    }

    /// Its seconds as a player would say them.
    pub fn whole_secs(&self) -> u32 {
        self.secs.round().max(1.0) as u32
    }
}

/// What a bot knows of one enemy at a look.
#[derive(Clone, Debug, PartialEq)]
pub struct Seen {
    pub foe: Who,
    pub in_sight: bool,
    /// The bot's last trigger pull at them since the last look.
    pub bot_fired: Option<SimTime>,
    /// Their last shot at the bot since the last look.
    pub foe_fired: Option<SimTime>,
}

/// A bot at a look: the enemies it knows.
#[derive(Clone, Debug, PartialEq)]
pub struct Look {
    pub bot: Who,
    pub health: i32,
    /// It is pulling back from a fight.
    pub retreating: bool,
    pub foes: Vec<Seen>,
}

/// One of the two died by the other: the fight ends so unless the other follows within [`TRADE`].
#[derive(Clone, Debug)]
struct Decided {
    at: SimTime,
    secs: f64,
    end: FightEnd,
}

/// A bot and an enemy shooting at each other.
#[derive(Clone, Debug)]
struct Pair {
    bot: Who,
    foe: Who,
    /// First and last shot of each side in this exchange.
    bot_shots: Option<(SimTime, SimTime)>,
    foe_shots: Option<(SimTime, SimTime)>,
    /// The bot's health when the shooting stopped, or now while it goes on.
    health: i32,
    /// When the bot last had the enemy in sight.
    seen_at: Option<SimTime>,
    /// When the bot pulled back of late, from first look to last, and whether it still does.
    retreats: Vec<(SimTime, SimTime)>,
    retreating: bool,
    decided: Option<Decided>,
}

impl Pair {
    fn new(bot: &Who, foe: &Who, health: i32) -> Pair {
        Pair {
            bot: bot.clone(),
            foe: foe.clone(),
            bot_shots: None,
            foe_shots: None,
            health,
            seen_at: None,
            retreats: Vec::new(),
            retreating: false,
            decided: None,
        }
    }

    /// When both were shooting: the answering side's first shot.
    fn start(&self) -> Option<SimTime> {
        Some(self.bot_shots?.0.max(self.foe_shots?.0))
    }

    /// The last shot of the side that has been silent longer.
    fn quiet_since(&self) -> Option<SimTime> {
        Some(self.bot_shots?.1.min(self.foe_shots?.1))
    }

    fn last_shot(&self) -> Option<SimTime> {
        match (self.bot_shots, self.foe_shots) {
            (Some(b), Some(f)) => Some(b.1.max(f.1)),
            (Some((_, t)), None) | (None, Some((_, t))) => Some(t),
            (None, None) => None,
        }
    }

    /// Both have shot and neither has been silent longer than [`PAUSE`] by `t`.
    fn live_at(&self, t: SimTime) -> bool {
        self.quiet_since().is_some_and(|q| t.since(q) <= PAUSE)
    }

    fn retreat(&mut self, now: SimTime, retreating: bool) {
        match self.retreats.last_mut() {
            Some((_, last)) if retreating && self.retreating => *last = now,
            _ if retreating => self.retreats.push((now, now)),
            _ => {}
        }
        self.retreating = retreating;
        // The quiet only comes later: pulling back that ended before it can no longer be what ended the fight.
        if let Some(q) = self.quiet_since() {
            self.retreats.retain(|(_, last)| q.since(*last) <= RETREAT_LAG);
        }
    }

    fn retreated_at(&self, quiet: SimTime) -> bool {
        self.retreats
            .iter()
            .any(|&(first, last)| first.since(quiet) <= RETREAT_LAG && quiet.since(last) <= RETREAT_LAG)
    }

    /// How the fight ends when it went quiet at `quiet`: `None` when the bot turned away from an enemy in sight that
    /// kept shooting at it, for neither of them got away.
    fn broken_off(&self, quiet: SimTime) -> Option<FightEnd> {
        let (bot_last, foe_last) = (self.bot_shots?.1, self.foe_shots?.1);
        let foe_stopped = foe_last.since(bot_last) <= TOGETHER;
        let foe_gone = self.seen_at.is_none_or(|s| s.since(bot_last) <= TOGETHER);
        if self.retreated_at(quiet) {
            Some(FightEnd::Retreated)
        } else if foe_stopped || foe_gone {
            Some(FightEnd::Fled)
        } else {
            None
        }
    }

    fn fight(&self, secs: f64, end: FightEnd) -> Fight {
        let dead = matches!(end, FightEnd::Lost | FightEnd::Traded | FightEnd::Interrupted { .. });
        Fight {
            bot: self.bot.clone(),
            foe: self.foe.clone(),
            secs: secs as f32,
            health: if dead { 0 } else { self.health.max(0) },
            end,
        }
    }
}

/// What a death does to a fight with the one who died.
enum Then {
    Wait(Decided),
    End(Fight),
    Forget,
}

fn shot(shots: &mut Option<(SimTime, SimTime)>, t: SimTime) {
    *shots = Some(match *shots {
        Some((first, last)) => (first, last.max(t)),
        None => (t, t),
    });
}

/// The fights of every bot on the map.
#[derive(Clone, Debug, Default)]
pub struct Fights {
    /// By the bot's `userid`, then the enemy's.
    pairs: BTreeMap<(i32, i32), Pair>,
    /// Every player's last death, by `userid`: shots before it were of another life.
    deaths: BTreeMap<i32, SimTime>,
    /// The bot's last shot at each enemy within a pause, as `pairs`, kept when the enemy dies: a rocket or a grenade
    /// they let go before may still kill the bot.
    fired: BTreeMap<(i32, i32), SimTime>,
    ended: Vec<Fight>,
}

impl Fights {
    pub fn clear(&mut self) {
        *self = Fights::default();
    }

    /// What the bot knew at `now`: its and the enemies' shots since its last look, whom it sees, its health.
    pub fn look(&mut self, now: SimTime, look: &Look) {
        let me = look.bot.userid;
        for seen in &look.foes {
            let after = [self.deaths.get(&me), self.deaths.get(&seen.foe.userid)]
                .into_iter()
                .flatten()
                .copied()
                .reduce(SimTime::max);
            let fresh = |t: Option<SimTime>| t.filter(|t| after.is_none_or(|a| *t > a));
            let (bot_fired, foe_fired) = (fresh(seen.bot_fired), fresh(seen.foe_fired));
            let key = (me, seen.foe.userid);
            if let Some(t) = bot_fired {
                self.fired.insert(key, t);
            }
            if !self.pairs.contains_key(&key) && bot_fired.is_none() && foe_fired.is_none() {
                continue;
            }
            let pair = self
                .pairs
                .entry(key)
                .or_insert_with(|| Pair::new(&look.bot, &seen.foe, look.health));
            if pair.decided.is_some() {
                continue;
            }
            pair.foe.clone_from(&seen.foe);
            if let Some(t) = bot_fired {
                shot(&mut pair.bot_shots, t);
            }
            if let Some(t) = foe_fired {
                shot(&mut pair.foe_shots, t);
            }
        }
        for (key, pair) in self.pairs.range_mut((me, i32::MIN)..=(me, i32::MAX)) {
            if pair.decided.is_some() {
                continue;
            }
            pair.bot.clone_from(&look.bot);
            if pair.last_shot().is_some_and(|l| now.since(l) <= TOGETHER) {
                pair.health = look.health;
            }
            if look.foes.iter().any(|s| s.foe.userid == key.1 && s.in_sight) {
                pair.seen_at = Some(now);
            }
            pair.retreat(now, look.retreating);
        }
    }

    /// A line of the kill feed: `killer` is `None` for the world, or the victim for a suicide.
    pub fn death(&mut self, t: SimTime, killer: Option<&Who>, victim: &Who) {
        self.expire(t);
        self.deaths.insert(victim.userid, t);
        let killer = killer.filter(|k| k.userid != victim.userid);
        let keys: Vec<(i32, i32)> = self
            .pairs
            .keys()
            .copied()
            .filter(|&(bot, foe)| bot == victim.userid || foe == victim.userid)
            .collect();
        // A bot that dies to one it was shooting at lately has no one to blame for its other fights: they end with
        // nothing. Shots older than a pause are gone by now, after `expire`.
        let fought_killer = killer.is_some_and(|k| {
            let key = (victim.userid, k.userid);
            self.fired.contains_key(&key) || self.pairs.get(&key).is_some_and(|p| p.bot_shots.is_some())
        });
        for key in keys {
            let Some(pair) = self.pairs.get_mut(&key) else {
                continue;
            };
            let bot_died = key.0 == victim.userid;
            let by_other = killer.is_some_and(|k| k.userid == if bot_died { key.1 } else { key.0 });
            if let Some(d) = &pair.decided {
                let traded = by_other
                    && t.since(d.at) <= TRADE
                    && matches!((&d.end, bot_died), (FightEnd::Won, true) | (FightEnd::Lost, false));
                if traded {
                    let fight = pair.fight(d.secs, FightEnd::Traded);
                    self.ended.push(fight);
                    self.pairs.remove(&key);
                }
                continue;
            }
            let then = match (pair.start(), killer) {
                (Some(start), Some(k)) if pair.live_at(t) && t.since(start) >= FIGHT_MIN => {
                    let secs = t.since(start);
                    let wait = |end| Then::Wait(Decided { at: t, secs, end });
                    match (by_other, bot_died) {
                        (true, true) => wait(FightEnd::Lost),
                        (true, false) => wait(FightEnd::Won),
                        (false, true) if fought_killer => Then::Forget,
                        (false, true) => Then::End(pair.fight(secs, FightEnd::Interrupted { by: k.clone() })),
                        (false, false) => Then::End(pair.fight(secs, FightEnd::Stolen { by: k.clone() })),
                    }
                }
                _ => Then::Forget,
            };
            match then {
                Then::Wait(decided) => pair.decided = Some(decided),
                Then::End(fight) => {
                    self.ended.push(fight);
                    self.pairs.remove(&key);
                }
                Then::Forget => {
                    self.pairs.remove(&key);
                }
            }
        }
    }

    /// The player left: their fights end with nothing.
    pub fn left(&mut self, userid: i32) {
        self.pairs.retain(|&(bot, foe), _| bot != userid && foe != userid);
        self.fired.retain(|&(bot, foe), _| bot != userid && foe != userid);
    }

    /// Only the bots `keep` says are still in the game keep their fights.
    pub fn retain_bots(&mut self, keep: impl Fn(i32) -> bool) {
        self.pairs.retain(|&(bot, _), _| keep(bot));
        self.fired.retain(|&(bot, _), _| keep(bot));
    }

    /// The fights that ended by `now`.
    pub fn take(&mut self, now: SimTime) -> Vec<Fight> {
        self.expire(now);
        std::mem::take(&mut self.ended)
    }

    /// Ends the fights whose trade has not come and those a side went silent in; forgets shots nobody answered.
    fn expire(&mut self, now: SimTime) {
        let mut gone = Vec::new();
        for (&key, pair) in &self.pairs {
            if let Some(d) = &pair.decided {
                if now.since(d.at) > TRADE {
                    self.ended.push(pair.fight(d.secs, d.end.clone()));
                    gone.push(key);
                }
                continue;
            }
            let (Some(start), Some(quiet)) = (pair.start(), pair.quiet_since()) else {
                if pair.last_shot().is_none_or(|t| now.since(t) > PAUSE) {
                    gone.push(key);
                }
                continue;
            };
            if now.since(quiet) <= PAUSE {
                continue;
            }
            gone.push(key);
            let secs = quiet.since(start);
            if let Some(end) = pair.broken_off(quiet).filter(|_| secs >= FIGHT_MIN) {
                self.ended.push(pair.fight(secs, end));
            }
        }
        for key in gone {
            self.pairs.remove(&key);
        }
        self.fired.retain(|_, t| now.since(*t) <= PAUSE);
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

    /// A bot and a human shooting at each other at given times, the bot looking every [`LOOK`].
    struct Duel {
        fights: Fights,
        bot: Who,
        foe: Who,
        health: i32,
        in_sight: bool,
        retreating: bool,
        t: f64,
        out: Vec<Fight>,
    }

    impl Duel {
        fn new() -> Duel {
            Duel {
                fights: Fights::default(),
                bot: who(1, "bot", true),
                foe: who(2, "Gordon", false),
                health: 100,
                in_sight: true,
                retreating: false,
                t: 0.0,
                out: Vec::new(),
            }
        }

        /// Looks until `until`, the bot shooting while `bot` holds and the human while `foe` holds.
        fn shoot(&mut self, until: f64, bot: impl Fn(f64) -> bool, foe: impl Fn(f64) -> bool) {
            while self.t + LOOK <= until + 1e-9 {
                self.t += LOOK;
                let t = self.t;
                let look = Look {
                    bot: self.bot.clone(),
                    health: self.health,
                    retreating: self.retreating,
                    foes: vec![Seen {
                        foe: self.foe.clone(),
                        in_sight: self.in_sight,
                        bot_fired: bot(t).then_some(SimTime(t)),
                        foe_fired: foe(t).then_some(SimTime(t)),
                    }],
                };
                self.fights.look(SimTime(t), &look);
                self.out.extend(self.fights.take(SimTime(t)));
            }
        }

        fn wait(&mut self, until: f64) {
            self.shoot(until, |_| false, |_| false);
        }

        fn death(&mut self, killer: Option<&Who>, victim: &Who) {
            self.fights.death(SimTime(self.t), killer, victim);
            self.out.extend(self.fights.take(SimTime(self.t)));
        }

        fn only(&self) -> &Fight {
            assert_eq!(self.out.len(), 1, "{:?}", self.out);
            &self.out[0]
        }
    }

    /// Shots every `step` seconds from `from` to `to`, at the looks that fall on them.
    fn every(step: f64, from: f64, to: f64) -> impl Fn(f64) -> bool {
        move |t| {
            let k = (t - from) / step;
            t >= from - 1e-6 && t <= to + 1e-6 && (k - k.round()).abs() < 1e-6
        }
    }

    #[test]
    fn only_the_part_where_both_shoot_counts() {
        // The bot opens at once, the human answers at 0.8 s, the bot kills them at 3.2 s: 2.4 s of both shooting.
        let mut d = Duel::new();
        d.shoot(3.2, every(0.1, 0.05, 3.2), every(0.5, 0.8, 3.2));
        let (bot, foe) = (d.bot.clone(), d.foe.clone());
        d.death(Some(&bot), &foe);
        d.wait(6.0);
        assert!(d.out.is_empty(), "{:?}", d.out);
    }

    #[test]
    fn a_kill_after_three_seconds_of_both_shooting_is_a_won_fight() {
        let mut d = Duel::new();
        d.health = 35;
        d.shoot(4.0, every(0.1, 0.05, 4.0), every(0.5, 0.5, 4.0));
        let (bot, foe) = (d.bot.clone(), d.foe.clone());
        d.death(Some(&bot), &foe);
        assert!(d.out.is_empty(), "a trade may still come");
        d.wait(6.5);
        let f = d.only();
        assert_eq!(f.end, FightEnd::Won);
        assert!((f.secs - 3.5).abs() < 0.06, "{}", f.secs);
        assert_eq!((f.health, f.whole_secs()), (35, 4));
        assert!(!f.bright());
    }

    #[test]
    fn nobody_answering_is_no_fight() {
        let mut d = Duel::new();
        d.shoot(8.0, every(0.1, 0.05, 8.0), |_| false);
        let (bot, foe) = (d.bot.clone(), d.foe.clone());
        d.death(Some(&bot), &foe);
        d.wait(11.0);
        assert!(d.out.is_empty());
    }

    #[test]
    fn a_shot_long_unanswered_is_forgotten() {
        // The human's answer comes 3.5 s after the bot's only shot: an exchange of its own, too short.
        let mut d = Duel::new();
        d.shoot(5.0, |t| (t - 0.1).abs() < 1e-6, every(0.5, 3.6, 5.0));
        let (bot, foe) = (d.bot.clone(), d.foe.clone());
        d.death(Some(&foe), &bot);
        d.wait(8.0);
        assert!(d.out.is_empty(), "{:?}", d.out);
    }

    #[test]
    fn losing_and_trading() {
        let mut d = Duel::new();
        d.shoot(5.0, every(0.1, 0.05, 5.0), every(0.5, 0.5, 5.0));
        let (bot, foe) = (d.bot.clone(), d.foe.clone());
        d.death(Some(&foe), &bot);
        d.wait(6.0);
        d.death(Some(&bot), &foe);
        let f = d.only();
        assert_eq!((&f.end, f.health), (&FightEnd::Traded, 0));
        assert!(f.bright(), "a trade is bright");

        let mut d = Duel::new();
        d.shoot(5.0, every(0.1, 0.05, 5.0), every(0.5, 0.5, 5.0));
        d.death(Some(&foe), &bot);
        d.wait(7.2);
        d.death(Some(&bot), &foe);
        assert_eq!(d.only().end, FightEnd::Lost, "too late for a trade");
    }

    #[test]
    fn a_trade_either_way() {
        // The bot's kill, then the human's grenade from the grave.
        let mut d = Duel::new();
        d.shoot(5.0, every(0.1, 0.05, 5.0), every(0.5, 0.5, 5.0));
        let (bot, foe) = (d.bot.clone(), d.foe.clone());
        d.death(Some(&bot), &foe);
        d.wait(6.5);
        d.death(Some(&foe), &bot);
        let f = d.only();
        assert_eq!((&f.end, f.health), (&FightEnd::Traded, 0));
        assert!(
            (f.secs - 4.5).abs() < 0.06,
            "the fight up to the first death: {}",
            f.secs
        );

        // Someone else killing the bot after it won is no trade.
        let mut d = Duel::new();
        d.shoot(5.0, every(0.1, 0.05, 5.0), every(0.5, 0.5, 5.0));
        d.death(Some(&bot), &foe);
        d.wait(6.0);
        d.death(Some(&who(3, "Alyx", false)), &bot);
        d.wait(8.0);
        assert_eq!(d.only().end, FightEnd::Won);
    }

    #[test]
    fn a_bot_gone_takes_its_fights_along() {
        let mut d = Duel::new();
        d.shoot(4.0, every(0.1, 0.05, 4.0), every(0.5, 0.5, 4.0));
        d.fights.retain_bots(|id| id != d.bot.userid);
        let (bot, foe) = (d.bot.clone(), d.foe.clone());
        d.fights.death(SimTime(4.0), Some(&bot), &foe);
        assert!(d.fights.take(SimTime(9.0)).is_empty());
    }

    #[test]
    fn a_third_player_steals_or_interrupts() {
        let third = who(3, "Alyx", false);
        let mut d = Duel::new();
        d.shoot(4.0, every(0.1, 0.05, 4.0), every(0.5, 0.5, 4.0));
        let (bot, foe) = (d.bot.clone(), d.foe.clone());
        d.death(Some(&third), &foe);
        assert_eq!(d.only().end, FightEnd::Stolen { by: third.clone() });
        assert_eq!(d.only().by(), Some(&third));

        let mut d = Duel::new();
        d.shoot(4.0, every(0.1, 0.05, 4.0), every(0.5, 0.5, 4.0));
        d.death(Some(&third), &bot);
        let f = d.only();
        assert_eq!((&f.end, f.health), (&FightEnd::Interrupted { by: third.clone() }, 0));
    }

    #[test]
    fn dying_to_one_the_bot_was_shooting_at_leaves_its_other_fight_without_an_end() {
        // The killer never showed a shot at the bot: still, the bot was at them, not taken from behind. The same when
        // the bot killed them first and their rocket, on its way, got the bot after.
        for killed_first in [false, true] {
            let third = who(3, "Alyx", false);
            let mut d = Duel::new();
            let (bot, foe) = (d.bot.clone(), d.foe.clone());
            let mut t = 0.0;
            while t < 4.0 {
                t += LOOK;
                let look = Look {
                    bot: bot.clone(),
                    health: 50,
                    retreating: false,
                    foes: vec![
                        Seen {
                            foe: foe.clone(),
                            in_sight: true,
                            bot_fired: (t < 3.6).then_some(SimTime(t)),
                            foe_fired: Some(SimTime(t)),
                        },
                        Seen {
                            foe: third.clone(),
                            in_sight: true,
                            bot_fired: (t >= 3.6).then_some(SimTime(t)),
                            foe_fired: None,
                        },
                    ],
                };
                d.fights.look(SimTime(t), &look);
            }
            if killed_first {
                d.fights.death(SimTime(t), Some(&bot), &third);
                t += 0.25;
            }
            d.fights.death(SimTime(t), Some(&third), &bot);
            let out = d.fights.take(SimTime(t + 5.0));
            assert!(out.is_empty(), "{killed_first}: {out:?}");
        }
    }

    #[test]
    fn dying_to_another_one_it_fought_ends_only_that_fight() {
        let third = who(3, "Alyx", false);
        let mut d = Duel::new();
        let (bot, foe) = (d.bot.clone(), d.foe.clone());
        let mut t = 0.0;
        while t < 4.0 {
            t += LOOK;
            let look = Look {
                bot: bot.clone(),
                health: 50,
                retreating: false,
                foes: vec![
                    Seen {
                        foe: foe.clone(),
                        in_sight: true,
                        bot_fired: Some(SimTime(t)),
                        foe_fired: Some(SimTime(t)),
                    },
                    Seen {
                        foe: third.clone(),
                        in_sight: true,
                        bot_fired: None,
                        foe_fired: (t > 2.0).then_some(SimTime(t)),
                    },
                ],
            };
            d.fights.look(SimTime(t), &look);
        }
        let look = Look {
            bot: bot.clone(),
            health: 50,
            retreating: false,
            foes: vec![Seen {
                foe: third.clone(),
                in_sight: true,
                bot_fired: Some(SimTime(t)),
                foe_fired: None,
            }],
        };
        d.fights.look(SimTime(t), &look);
        d.fights.death(SimTime(t), Some(&third), &bot);
        assert!(
            d.fights.take(SimTime(t + 5.0)).is_empty(),
            "the short fight with the killer says it all, so nothing"
        );
    }

    #[test]
    fn suicides_the_world_and_players_leaving_end_fights_with_nothing() {
        for end in 0..3 {
            let mut d = Duel::new();
            d.shoot(4.0, every(0.1, 0.05, 4.0), every(0.5, 0.5, 4.0));
            let foe = d.foe.clone();
            match end {
                0 => d.death(Some(&foe), &foe),
                1 => d.death(None, &foe),
                _ => d.fights.left(foe.userid),
            }
            d.wait(9.0);
            assert!(d.out.is_empty(), "{end}: {:?}", d.out);
        }
    }

    #[test]
    fn breaking_off() {
        // Both stop in sight of each other: the human broke off, and the health is the bot's as the shooting stopped.
        let mut d = Duel::new();
        d.health = 40;
        d.shoot(4.0, every(0.1, 0.05, 4.0), every(0.5, 0.5, 4.0));
        d.wait(5.0);
        d.health = 100;
        d.wait(9.0);
        let f = d.only();
        assert_eq!((&f.end, f.health), (&FightEnd::Fled, 40));
        assert!((f.secs - 3.5).abs() < 0.06, "{}", f.secs);

        // The human stops first while the bot goes on shooting: they broke off.
        let mut d = Duel::new();
        d.shoot(6.0, every(0.1, 0.05, 6.0), every(0.5, 0.5, 4.0));
        d.wait(9.0);
        assert_eq!(d.only().end, FightEnd::Fled);

        // The human goes out of sight and the bot stops, the human still hurting it from where it was: hid.
        let mut d = Duel::new();
        d.shoot(4.0, every(0.1, 0.05, 4.0), every(0.5, 0.5, 4.0));
        d.in_sight = false;
        d.shoot(6.0, |_| false, every(0.5, 4.5, 6.0));
        d.wait(12.0);
        assert_eq!(d.only().end, FightEnd::Fled);

        // The bot turns away from the human in its sight that goes on shooting at it: no way out of the fight.
        let mut d = Duel::new();
        d.shoot(6.0, every(0.1, 0.05, 4.0), every(0.5, 0.5, 6.0));
        d.wait(12.0);
        assert!(d.out.is_empty(), "{:?}", d.out);
    }

    #[test]
    fn pulling_back() {
        // The bot pulls back as the shooting stops, a little hurt: a close one.
        let mut d = Duel::new();
        d.health = 15;
        d.shoot(4.0, every(0.1, 0.05, 4.0), every(0.5, 0.5, 4.0));
        d.retreating = true;
        d.wait(9.0);
        let f = d.only();
        assert_eq!((&f.end, f.health), (&FightEnd::Retreated, 15));
        assert!(f.bright());

        // Shooting as it backs off, the human losing it.
        let mut d = Duel::new();
        d.shoot(2.0, every(0.1, 0.05, 2.0), every(0.5, 0.5, 2.0));
        d.retreating = true;
        d.shoot(4.0, every(0.1, 2.05, 4.0), every(0.5, 2.5, 4.0));
        d.retreating = false;
        d.wait(9.0);
        assert_eq!(d.only().end, FightEnd::Retreated);

        // A pull back long before the end, or only well after the human broke off, did not end it.
        for (from, to) in [(1.0, 1.5), (5.5, 6.5)] {
            let mut d = Duel::new();
            d.shoot(from, every(0.1, 0.05, 4.0), every(0.5, 0.5, 4.0));
            d.retreating = true;
            d.shoot(to, every(0.1, 0.05, 4.0), every(0.5, 0.5, 4.0));
            d.retreating = false;
            d.shoot(4.0, every(0.1, 0.05, 4.0), every(0.5, 0.5, 4.0));
            d.wait(9.0);
            assert_eq!(d.only().end, FightEnd::Fled, "pulling back {from}–{to} s");
        }
    }

    #[test]
    fn a_pause_shorter_than_three_seconds_keeps_the_fight() {
        let mut d = Duel::new();
        d.shoot(2.0, every(0.1, 0.05, 2.0), every(0.5, 0.5, 2.0));
        d.wait(4.5);
        d.shoot(7.0, every(0.1, 4.55, 7.0), every(0.5, 4.6, 7.0));
        let (bot, foe) = (d.bot.clone(), d.foe.clone());
        d.death(Some(&bot), &foe);
        d.wait(9.5);
        let f = d.only();
        assert_eq!(f.end, FightEnd::Won);
        assert!((f.secs - 6.5).abs() < 0.06, "{}", f.secs);
        assert!(f.bright(), "6 s and more is long");
    }

    #[test]
    fn shots_of_an_earlier_life_are_not_taken() {
        let mut d = Duel::new();
        let (bot, foe) = (d.bot.clone(), d.foe.clone());
        d.shoot(1.0, every(0.1, 0.05, 1.0), every(0.5, 0.5, 1.0));
        d.death(Some(&foe), &bot);
        // After its respawn the bot still knows the shots of before.
        for i in 1..=80 {
            let t = 1.0 + f64::from(i) * LOOK;
            let look = Look {
                bot: bot.clone(),
                health: 100,
                retreating: false,
                foes: vec![Seen {
                    foe: foe.clone(),
                    in_sight: true,
                    bot_fired: Some(SimTime(0.9)),
                    foe_fired: Some(SimTime(t)),
                }],
            };
            d.fights.look(SimTime(t), &look);
        }
        d.fights.death(SimTime(5.0), Some(&foe), &bot);
        assert!(d.fights.take(SimTime(9.0)).is_empty());
    }

    #[test]
    fn what_makes_a_fight_bright() {
        let f = |secs: f32, health: i32, end: FightEnd| Fight {
            bot: who(1, "bot", true),
            foe: who(2, "Gordon", false),
            secs,
            health,
            end,
        };
        assert!(f(6.0, 90, FightEnd::Lost).bright());
        assert!(f(3.5, 0, FightEnd::Traded).bright());
        assert!(f(3.5, 20, FightEnd::Won).bright());
        assert!(f(3.5, 20, FightEnd::Fled).bright());
        assert!(!f(3.5, 21, FightEnd::Won).bright());
        assert!(!f(5.9, 0, FightEnd::Lost).bright());
        let third = who(3, "Alyx", false);
        assert!(!f(3.5, 10, FightEnd::Stolen { by: third }).bright());
    }
}
