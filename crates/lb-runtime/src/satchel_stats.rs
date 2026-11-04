//! Satchel statistics of the bots for stand runs, the satchel part of `lb stats`: each satchel a bot throws followed
//! from the throw to its blast, and how readily the bots throw with satchels on them. This is measurement, not
//! behavior: it reads what the bots do not know (where everyone is, who took the blast), and nothing here feeds back
//! into them.
//!
//! - **The throw:** the nearest enemy in the thrower's sight then (none: thrown at nobody seen), how far it was.
//! - **The blast** goes off where the satchel was last seen (one gone without a blast was taken away, its owner dead):
//!   how long after the throw, how far the nearest other player's box was, the damage it did to others and to the
//!   thrower (credited as the rest of `lb stats` credits damage), how many it hurt, and kills with a satchel a moment
//!   after it.
//! - **The pace:** seconds between one bot's throws in a life, and from a spawn to its first throw.
//! - **Spells in sight:** an enemy in the bot's sight (gone for no more than half a second) while it carries
//!   satchels: their time, the share with a satchel thrown in them, how long after the enemy came into sight the first
//!   went, and the deaths in them.
//! - **With an enemy in sight:** the time split by what the bot does: a throw under way and its phase, a satchel in
//!   flight, the radio up, or why no throw went (the brain's reason).
//! - **In hand:** the time standing still with satchels in hand, and the view's yaw off the enemy in sight.

use lb_core::Vec3;
use lb_core::time::SimTime;
use rustc_hash::FxHashMap;
use smallvec::SmallVec;

use crate::rocket_stats::PlayerBox;

/// A satchel's blast (120 damage) reaches 300 units: the bands of the nearest player's box off it.
const BLAST_BANDS: [f32; 3] = [100.0, 200.0, 300.0];
const BLAST_NAMES: [&str; 4] = ["<100 u", "100-200", "200-300", "further"];
/// A blast's damage and a kill are credited to the satchel this long after it went off.
const CREDIT_FOR: f64 = 0.5;
/// A blast follows its thrower's set-off within this (the radio's press, a frame of the snapshot).
const WHY_FOR: f64 = 0.5;
/// A satchel first seen this soon after its owner's throw is that throw's.
const THROW_MATCH: f64 = 0.5;
/// A satchel out of the snapshots this long is gone: it went off, or was taken away, where it was last seen.
const GONE_AFTER: f64 = 0.15;
/// An enemy out of the bot's sight this long ends a spell in sight.
const SPELL_GAP: f64 = 0.5;
/// A death this soon after a spell ended is that spell's (the death ends it).
const SPELL_DEATH: f64 = 0.3;
/// Throws of one bot this close in time are a run of throws.
const RUN_GAP: f64 = 15.0;
/// Under this speed on the ground a bot stands.
const STANDING: f32 = 40.0;
/// So much of the satchels and the times is kept, the latest.
const KEEP_LAST: usize = 20_000;

/// What a bot does this frame, for the stand statistics.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BotFrame {
    pub alive: bool,
    /// Satchels in the pocket; the satchel (or its radio) in hand; charges out by the game's word.
    pub carried: i32,
    pub in_hand: bool,
    pub out: bool,
    /// The nearest enemy in the bot's sight: its slot, its box and how far it is.
    pub enemy: Option<(u8, PlayerBox, f32)>,
    /// The view's yaw off that enemy's middle (degrees).
    pub yaw_off: Option<f32>,
    pub speed: f32,
    pub on_ground: bool,
    /// The weapon protocol under way (a satchel throw by its phase), and why the last look for a throw found none.
    pub doing: Option<&'static str>,
    pub hold: &'static str,
}

#[derive(Clone, Copy, Debug, Default)]
struct Throw {
    at: SimTime,
    /// The nearest enemy in the thrower's sight, how far.
    enemy: Option<f32>,
    speed: f32,
    airborne: bool,
}

#[derive(Clone, Debug, Default)]
struct Satchel {
    thrower: u8,
    throw: Throw,
    last: Vec3,
    seen: SimTime,
    /// Gone at, where; whether it went off (else it was taken away).
    end: Option<(SimTime, Vec3)>,
    burst: bool,
    /// The nearest other player's box off where it went off, and the thrower's.
    nearest: Option<f32>,
    thrower_off: Option<f32>,
    hurt: SmallVec<[u8; 4]>,
    dealt: f32,
    own: f32,
    kills: u32,
    suicide: bool,
    /// Why its thrower set it off, as the brain counted it a moment before the blast; what the thrower was doing
    /// just before it.
    why: Option<&'static str>,
    doing: Option<&'static str>,
}

#[derive(Clone, Copy, Debug)]
struct Spell {
    since: SimTime,
    last_seen: SimTime,
    thrown: bool,
}

#[derive(Clone, Debug, Default)]
struct Watch {
    last_frame: Option<SimTime>,
    alive_since: Option<SimTime>,
    thrown_this_life: bool,
    last_throw: Option<SimTime>,
    carried: Option<i32>,
    /// Throws not yet matched to a satchel seen: kept over the thrower's death.
    pending: SmallVec<[Throw; 4]>,
    spell: Option<Spell>,
    ended: Option<(SimTime, Spell)>,
    doing: Option<&'static str>,
}

#[derive(Clone, Debug, Default)]
pub struct SatchelStats {
    pub since: f64,
    flying: Vec<(u16, Satchel)>,
    done: Vec<Satchel>,
    watch: FxHashMap<u8, Watch>,
    throws: u32,
    /// Alive with satchels on it (carried or out).
    armed_time: f64,
    /// Seconds between throws in a run, from a spawn to the first throw of the life.
    gaps: Vec<f32>,
    first_after_spawn: Vec<f32>,
    spells: u32,
    spell_time: f64,
    spells_thrown: u32,
    /// From an enemy coming into sight to the first satchel thrown at it.
    reactions: Vec<f32>,
    died_dry: u32,
    died_thrown: u32,
    /// With an enemy in sight: in all, and by what the bot does.
    in_sight: f64,
    doing: FxHashMap<&'static str, f64>,
    /// Satchels in hand; standing still with them; and with an enemy in sight, and standing then.
    in_hand: f64,
    standing: f64,
    in_hand_seen: f64,
    standing_seen: f64,
    /// The view's yaw off the enemy in sight with satchels in hand, by 5° (the time; the last 155° and more).
    yaw: [f64; 32],
    /// Why satchels went off and why throws failed, by the brain's reasons.
    offs: FxHashMap<&'static str, u32>,
    failures: FxHashMap<(&'static str, &'static str), u32>,
    brain_seen: FxHashMap<u8, BrainCounts>,
    /// A bot's last set-off counted by its brain: when, why.
    last_off: FxHashMap<u8, (SimTime, &'static str)>,
}

type BrainCounts = (
    SmallVec<[(&'static str, u32); 8]>,
    SmallVec<[((&'static str, &'static str), u32); 8]>,
);

impl SatchelStats {
    pub fn reset(&mut self, now: f64) {
        *self = SatchelStats {
            since: now,
            ..SatchelStats::default()
        };
    }

    /// Sim time restarts with every map: what is under way is dropped (satchels not seen to end with it, spells in
    /// sight counted as they stand), the counts go on.
    pub fn new_map(&mut self, now: f64) {
        self.since -= now;
        self.done
            .extend(self.flying.drain(..).map(|(_, s)| s).filter(|s| s.end.is_some()));
        keep_last(&mut self.done);
        for s in std::mem::take(&mut self.watch).into_values().filter_map(|w| w.spell) {
            self.count_spell(s);
        }
        self.last_off.clear();
    }

    /// A bot's own counts of why its satchels went off and why its satchel protocols failed (`lb brain`): what they
    /// grew by since the last frame is counted. The first counts seen of a bot since the reset are where it starts.
    pub fn brain(
        &mut self,
        slot: u8,
        now: SimTime,
        offs: &[(&'static str, u32)],
        pressed: Option<(SimTime, &'static str)>,
        failures: &[(&'static str, &'static str, u32)],
    ) {
        // The radio's press, as it went down: a bot its blast kills counts no set-off after it.
        if let Some((at, why)) = pressed
            && self.last_off.get(&slot).is_none_or(|(t, _)| *t < at)
        {
            self.last_off.insert(slot, (at, why));
            self.attach_why(slot, why, now);
        }
        let satchel = |p: &str| matches!(p, "satchel" | "satchel in flight" | "detonate");
        let failed: SmallVec<[((&'static str, &'static str), u32); 8]> = failures
            .iter()
            .filter(|(p, ..)| satchel(p))
            .map(|(p, why, n)| ((*p, *why), *n))
            .collect();
        let offs: SmallVec<[(&'static str, u32); 8]> = offs.iter().copied().collect();
        let Some((last_offs, last_failed)) = self.brain_seen.get(&slot) else {
            self.brain_seen.insert(slot, (offs, failed));
            return;
        };
        let counted: SmallVec<[(&'static str, u32); 4]> = offs
            .iter()
            .map(|(why, n)| {
                (
                    *why,
                    n - last_offs.iter().find(|(w, _)| w == why).map_or(0, |e| e.1).min(*n),
                )
            })
            .filter(|(_, k)| *k > 0)
            .collect();
        for (key, n) in &failed {
            let before = last_failed.iter().find(|(k, _)| k == key).map_or(0, |e| e.1);
            if *n > before {
                *self.failures.entry(*key).or_default() += n - before;
            }
        }
        self.brain_seen.insert(slot, (offs, failed));
        for (why, k) in counted {
            *self.offs.entry(why).or_default() += k;
            self.last_off.insert(slot, (now, why));
            self.attach_why(slot, why, now);
        }
    }

    /// A set-off counted after its blast was seen: that blast's reason.
    fn attach_why(&mut self, slot: u8, why: &'static str, now: SimTime) {
        for (_, s) in self
            .flying
            .iter_mut()
            .filter(|(_, s)| s.thrower == slot && s.why.is_none())
        {
            if s.burst && s.end.is_some_and(|(t, _)| now.since(t) <= WHY_FOR) {
                s.why = Some(why);
            }
        }
    }

    fn thrown(&mut self, slot: u8, now: SimTime, f: &BotFrame) {
        self.throws += 1;
        let w = self.watch.entry(slot).or_default();
        if let Some(at) = w.last_throw
            && now.since(at) < RUN_GAP
        {
            self.gaps.push(now.since(at) as f32);
            keep_last(&mut self.gaps);
        }
        if !w.thrown_this_life
            && let Some(at) = w.alive_since
        {
            self.first_after_spawn.push(now.since(at) as f32);
            keep_last(&mut self.first_after_spawn);
        }
        w.thrown_this_life = true;
        w.last_throw = Some(now);
        if let Some(s) = &mut w.spell
            && !s.thrown
        {
            s.thrown = true;
            self.reactions.push(now.since(s.since) as f32);
            keep_last(&mut self.reactions);
        }
        w.pending.retain(|t| now.since(t.at) <= THROW_MATCH);
        w.pending.push(Throw {
            at: now,
            enemy: f.enemy.map(|(.., d)| d),
            speed: f.speed,
            airborne: !f.on_ground,
        });
    }

    /// A satchel seen in this frame's snapshot: a new one is its owner's last throw, if a bot threw it.
    pub fn seen(&mut self, index: u16, owner: u8, at: Vec3, burst: bool, now: SimTime, boxes: &[(u8, PlayerBox)]) {
        if let Some((_, s)) = self.flying.iter_mut().find(|(i, s)| *i == index && s.end.is_none()) {
            s.last = at;
            s.seen = now;
            if burst {
                blow(s, at, now, true, boxes);
                s.why = self
                    .last_off
                    .get(&s.thrower)
                    .filter(|(t, _)| now.since(*t) <= WHY_FOR)
                    .map(|(_, why)| *why);
                s.doing = self.watch.get(&s.thrower).and_then(|w| w.doing);
            }
            return;
        }
        if self.flying.iter().any(|(i, _)| *i == index) || burst {
            return;
        }
        let Some(w) = self.watch.get_mut(&owner) else {
            return;
        };
        w.pending.retain(|t| now.since(t.at) <= THROW_MATCH);
        if w.pending.is_empty() {
            return;
        }
        let throw = w.pending.remove(0);
        self.flying.push((
            index,
            Satchel {
                thrower: owner,
                throw,
                last: at,
                seen: now,
                ..Satchel::default()
            },
        ));
    }

    /// After this frame's snapshot: satchels no longer in it are gone where they were last seen (taken away: a blast
    /// shows first), and those gone a while ago are done.
    pub fn snapshot_done(&mut self, now: SimTime, boxes: &[(u8, PlayerBox)]) {
        for (_, s) in &mut self.flying {
            if s.end.is_none() && now.since(s.seen) > GONE_AFTER {
                let (at, seen) = (s.last, s.seen);
                blow(s, at, seen, false, boxes);
            }
        }
        let (old, keep): (Vec<_>, Vec<_>) = self
            .flying
            .drain(..)
            .partition(|(_, s)| s.end.is_some_and(|(t, _)| now.since(t) >= CREDIT_FOR));
        self.flying = keep;
        self.done.extend(old.into_iter().map(|(_, s)| s));
        keep_last(&mut self.done);
    }

    /// Damage the blast of satchel `index` did to the player in `victim`.
    pub fn damage(&mut self, index: u16, victim: u8, damage: f32, now: SimTime) {
        let Some((_, s)) = self
            .flying
            .iter_mut()
            .rev()
            .find(|(i, s)| *i == index && s.end.is_none_or(|(t, _)| now.since(t) < CREDIT_FOR))
        else {
            return;
        };
        if victim == s.thrower {
            s.own += damage;
        } else {
            s.dealt += damage;
            if !s.hurt.contains(&victim) {
                s.hurt.push(victim);
            }
        }
    }

    /// A kill feed line with a satchel: credited to the killer's satchel nearest to where the victim is that went off a
    /// moment before, or is about to be seen to.
    pub fn kill(&mut self, killer: u8, victim: u8, at: Vec3, now: SimTime) {
        if let Some((_, s)) = self
            .flying
            .iter_mut()
            .filter(|(_, s)| s.thrower == killer && s.end.is_none_or(|(t, _)| now.since(t) < CREDIT_FOR))
            .min_by(|a, b| a.1.last.distance(at).total_cmp(&b.1.last.distance(at)))
        {
            if killer == victim {
                s.suicide = true;
            } else {
                s.kills += 1;
            }
        }
    }

    /// A bot died: its spell in sight ends.
    pub fn died(&mut self, slot: u8, now: SimTime) {
        let Some(w) = self.watch.get_mut(&slot) else {
            return;
        };
        let spell = match w.spell.take() {
            Some(s) => {
                self.count_spell(s);
                Some(s)
            }
            None => w
                .ended
                .take()
                .filter(|(t, _)| now.since(*t) <= SPELL_DEATH)
                .map(|(_, s)| s),
        };
        if let Some(s) = spell {
            if s.thrown {
                self.died_thrown += 1;
            } else {
                self.died_dry += 1;
            }
        }
    }

    fn end_spell(&mut self, slot: u8, now: SimTime) {
        let Some(w) = self.watch.get_mut(&slot) else {
            return;
        };
        if let Some(s) = w.spell.take() {
            w.ended = Some((now, s));
            self.count_spell(s);
        }
    }

    fn count_spell(&mut self, s: Spell) {
        self.spells += 1;
        self.spell_time += s.last_seen.since(s.since);
        self.spells_thrown += u32::from(s.thrown);
    }

    /// Every frame, for each bot.
    pub fn frame(&mut self, slot: u8, now: SimTime, f: &BotFrame) {
        let w = self.watch.entry(slot).or_default();
        let dt = w.last_frame.map_or(0.0, |t| now.since(t).clamp(0.0, 0.1));
        w.last_frame = Some(now);
        w.doing = f.doing;
        if !f.alive {
            w.alive_since = None;
            w.thrown_this_life = false;
            w.last_throw = None;
            w.carried = None;
            if w.spell.is_some() {
                self.end_spell(slot, now);
            }
            return;
        }
        if w.alive_since.is_none() {
            w.alive_since = Some(now);
        }
        let before = w.carried.replace(f.carried);
        let armed = f.carried > 0 || f.out;
        let seen = armed && f.enemy.is_some();
        // Before the throws: one on the frame the enemy comes into sight is thrown in its spell.
        if seen {
            w.spell
                .get_or_insert(Spell {
                    since: now,
                    last_seen: now,
                    thrown: false,
                })
                .last_seen = now;
        }
        if let Some(before) = before {
            for _ in f.carried..before {
                self.thrown(slot, now, f);
            }
        }
        if armed {
            self.armed_time += dt;
        }
        let w = self.watch.entry(slot).or_default();
        if !seen && w.spell.is_some_and(|s| now.since(s.last_seen) > SPELL_GAP || !armed) {
            self.end_spell(slot, now);
        }
        if seen {
            self.in_sight += dt;
            let doing = f.doing.unwrap_or(if f.hold.is_empty() { "none given" } else { f.hold });
            *self.doing.entry(doing).or_default() += dt;
        }
        if f.in_hand {
            let still = f.on_ground && f.speed < STANDING;
            self.in_hand += dt;
            if still {
                self.standing += dt;
            }
            if f.enemy.is_some() {
                self.in_hand_seen += dt;
                if still {
                    self.standing_seen += dt;
                }
                if let Some(yaw) = f.yaw_off {
                    self.yaw[((yaw.abs() / 5.0) as usize).min(31)] += dt;
                }
            }
        }
    }

    pub fn report(&self, now: f64) -> Vec<String> {
        let gone = self.flying.iter().map(|(_, s)| s).filter(|s| s.end.is_some());
        let satchels: Vec<&Satchel> = self.done.iter().chain(gone).collect();
        if self.throws == 0 && self.armed_time <= 0.0 {
            return Vec::new();
        }
        let minutes = ((now - self.since) / 60.0).max(1e-6);
        let n = satchels.len().max(1) as f32;
        let pct = |k: usize| format!("{:.0}%", k as f32 / n * 100.0);
        let sum = |f: fn(&Satchel) -> f32| satchels.iter().map(|s| f(s)).sum::<f32>();
        let kills: u32 = satchels.iter().map(|s| s.kills).sum();
        let suicides = satchels.iter().filter(|s| s.suicide).count();
        let mut out = vec![format!(
            "  satchels {} thrown ({:.1}/min, {:.1} a minute alive with satchels, {:.0} s of that), {} followed: per \
             satchel {:.0} damage to others, {:.1} to the thrower, {:.2} kills ({kills}), {suicides} suicides",
            self.throws,
            f64::from(self.throws) / minutes,
            f64::from(self.throws) / (self.armed_time / 60.0).max(1e-6),
            self.armed_time,
            satchels.len(),
            sum(|s| s.dealt) / n,
            sum(|s| s.own) / n,
            kills as f32 / n,
        )];
        let blind: Vec<&&Satchel> = satchels.iter().filter(|s| s.throw.enemy.is_none()).collect();
        let mut at: Vec<f32> = satchels.iter().filter_map(|s| s.throw.enemy).collect();
        out.push(format!(
            "    at the throw: the nearest enemy in sight {} off (median; quartiles {}–{}), none in sight {} ({} kills); \
             from the air {}, running {}",
            dist(median(&mut at)),
            dist(quantile(&mut at, 0.25)),
            dist(quantile(&mut at, 0.75)),
            pct(blind.len()),
            blind.iter().map(|s| s.kills).sum::<u32>(),
            pct(satchels.iter().filter(|s| s.throw.airborne).count()),
            pct(satchels.iter().filter(|s| s.throw.speed >= 200.0).count()),
        ));
        let burst: Vec<&&Satchel> = satchels.iter().filter(|s| s.burst).collect();
        let mut after: Vec<f32> = burst
            .iter()
            .filter_map(|s| s.end.map(|(t, _)| t.since(s.throw.at) as f32))
            .collect();
        let mut bands = [0usize; 4];
        let mut offs = Vec::new();
        for s in &burst {
            if let Some(d) = s.nearest {
                bands[BLAST_BANDS.iter().position(|b| d < *b).unwrap_or(3)] += 1;
                offs.push(d);
            }
        }
        let nb = burst.len().max(1) as f32;
        let of_burst = |k: usize| format!("{:.0}%", k as f32 / nb * 100.0);
        out.push(format!(
            "    set off {} (gone without a blast {}), {} after the throw (median; quartiles {}–{}); the nearest player \
             off the blast: {} (median {}); hurt someone {}, two or more {}; the thrower {} off it (median)",
            pct(burst.len()),
            pct(satchels.len() - burst.len()),
            secs(median(&mut after)),
            secs(quantile(&mut after, 0.25)),
            secs(quantile(&mut after, 0.75)),
            BLAST_NAMES
                .iter()
                .zip(bands)
                .map(|(name, k)| format!("{name} {}", of_burst(k)))
                .collect::<Vec<_>>()
                .join(", "),
            dist(median(&mut offs)),
            of_burst(burst.iter().filter(|s| !s.hurt.is_empty()).count()),
            of_burst(burst.iter().filter(|s| s.hurt.len() >= 2).count()),
            dist(median(&mut burst.iter().filter_map(|s| s.thrower_off).collect::<Vec<_>>())),
        ));
        out.push(format!("    set off by the bots: {}", counted(&self.offs)));
        let hurt_self: Vec<&&Satchel> = satchels.iter().filter(|s| s.own > 0.0).collect();
        let by = |pick: &dyn Fn(&Satchel) -> bool| {
            let mut c: FxHashMap<&'static str, u32> = FxHashMap::default();
            for s in hurt_self.iter().filter(|s| pick(s)) {
                *c.entry(s.why.unwrap_or_else(|| uncounted(s.doing))).or_default() += 1;
            }
            counted(&c)
        };
        out.push(format!(
            "    hurting the thrower: {} ({} damage, median; the thrower {} off the blast), set off by: {}; the {} \
             suicides by: {}",
            pct(hurt_self.len()),
            median(&mut hurt_self.iter().map(|s| s.own).collect::<Vec<_>>()).map_or("-".into(), |d| format!("{d:.0}")),
            dist(median(
                &mut hurt_self.iter().filter_map(|s| s.thrower_off).collect::<Vec<_>>()
            )),
            by(&|_| true),
            suicides,
            by(&|s| s.suicide),
        ));
        // Killed by its own blast close by, or finished off by it at the edge, hurt by another's a moment before.
        let mut fatal: Vec<(f32, f32)> = satchels
            .iter()
            .filter(|s| s.suicide)
            .map(|s| (s.own, s.thrower_off.unwrap_or(f32::NAN)))
            .collect();
        fatal.sort_by(|a, b| a.0.total_cmp(&b.0));
        if !fatal.is_empty() {
            out.push(format!(
                "    the suicides' own damage and the thrower off the blast: {}",
                fatal
                    .iter()
                    .map(|(own, off)| format!("{own:.0} at {off:.0} u"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        out.push(format!(
            "    given up: {}",
            if self.failures.is_empty() {
                "-".to_string()
            } else {
                let mut v: Vec<(&(&str, &str), &u32)> = self.failures.iter().collect();
                v.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
                v.iter()
                    .map(|((protocol, why), k)| format!("{protocol}: {why} {k}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        ));
        let mut gaps = self.gaps.clone();
        let mut first = self.first_after_spawn.clone();
        out.push(format!(
            "    pace: {} between throws in a life (median of {}; quartiles {}–{}); the first throw {} after the spawn \
             (median of {})",
            secs(median(&mut gaps)),
            gaps.len(),
            secs(quantile(&mut gaps, 0.25)),
            secs(quantile(&mut gaps, 0.75)),
            secs(median(&mut first)),
            first.len(),
        ));
        let mut reactions = self.reactions.clone();
        // The spells still open count as they stand, as their time in sight and their throws' reactions do.
        let (spells, spell_time, spells_thrown) = self
            .watch
            .values()
            .filter_map(|w| w.spell)
            .fold((self.spells, self.spell_time, self.spells_thrown), |(n, t, k), s| {
                (n + 1, t + s.last_seen.since(s.since), k + u32::from(s.thrown))
            });
        out.push(format!(
            "    enemies in sight with satchels on: {:.0} s in {} spells, a satchel thrown in {:.0}%, the first {} after \
             the enemy came in sight (median; quartiles {}–{}); died in {} spells without a throw, in {} with one",
            spell_time,
            spells,
            spells_thrown as f32 / spells.max(1) as f32 * 100.0,
            secs(median(&mut reactions)),
            secs(quantile(&mut reactions, 0.25)),
            secs(quantile(&mut reactions, 0.75)),
            self.died_dry,
            self.died_thrown,
        ));
        let mut doing: Vec<(&&str, &f64)> = self.doing.iter().collect();
        doing.sort_by(|a, b| b.1.total_cmp(a.1).then(a.0.cmp(b.0)));
        out.push(format!(
            "    with an enemy in sight {:.0} s: {}",
            self.in_sight,
            doing
                .iter()
                .map(|(why, t)| format!("{why} {:.0}%", **t / self.in_sight.max(1e-6) * 100.0))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        let yaw_total: f64 = self.yaw.iter().sum();
        let yaw_median = {
            let mut acc = 0.0;
            self.yaw
                .iter()
                .position(|t| {
                    acc += t;
                    yaw_total > 0.0 && acc >= yaw_total / 2.0
                })
                .map_or("-".to_string(), |i| format!("{}°", i * 5 + 2))
        };
        out.push(format!(
            "    satchels in hand {:.0} s, standing still {:.0}%; with an enemy in sight {:.0} s, standing {:.0}%, the \
             view's yaw off it {yaw_median} (median), within 15° {:.0}%",
            self.in_hand,
            self.standing / self.in_hand.max(1e-6) * 100.0,
            self.in_hand_seen,
            self.standing_seen / self.in_hand_seen.max(1e-6) * 100.0,
            self.yaw[..3].iter().sum::<f64>() / yaw_total.max(1e-6) * 100.0,
        ));
        out
    }
}

fn blow(s: &mut Satchel, at: Vec3, now: SimTime, burst: bool, boxes: &[(u8, PlayerBox)]) {
    s.end = Some((now, at));
    s.burst = burst;
    s.nearest = boxes
        .iter()
        .filter(|(slot, _)| *slot != s.thrower)
        .map(|(_, b)| b.distance(at))
        .min_by(f32::total_cmp);
    s.thrower_off = boxes
        .iter()
        .find(|(slot, _)| *slot == s.thrower)
        .map(|(_, b)| b.distance(at));
}

/// A blast its thrower's brain counted no set-off for, by what the thrower was doing.
fn uncounted(doing: Option<&'static str>) -> &'static str {
    match doing {
        None => "uncounted, nothing under way",
        Some("detonate") => "uncounted, the radio up",
        Some("satchel in flight") => "uncounted, one in flight",
        Some(d) if d.starts_with("throwing") => "uncounted, a throw",
        Some(_) => "uncounted, something else",
    }
}

fn counted(counts: &FxHashMap<&'static str, u32>) -> String {
    if counts.is_empty() {
        return "-".into();
    }
    let mut v: Vec<(&&str, &u32)> = counts.iter().collect();
    v.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    v.iter()
        .map(|(why, k)| format!("{why} {k}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn keep_last<T>(v: &mut Vec<T>) {
    if v.len() > KEEP_LAST {
        v.drain(..v.len() - KEEP_LAST / 2);
    }
}

fn secs(v: Option<f32>) -> String {
    v.map_or("-".into(), |s| format!("{s:.2} s"))
}

fn dist(v: Option<f32>) -> String {
    v.map_or("-".into(), |d| format!("{d:.0} u"))
}

fn median(v: &mut [f32]) -> Option<f32> {
    quantile(v, 0.5)
}

fn quantile(v: &mut [f32], q: f32) -> Option<f32> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f32::total_cmp);
    let i = ((v.len() - 1) as f32 * q).round() as usize;
    Some(v[i])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(carried: i32, enemy: Option<f32>) -> BotFrame {
        BotFrame {
            alive: true,
            carried,
            in_hand: true,
            enemy: enemy.map(|d| {
                (
                    2,
                    PlayerBox::new(Vec3::new(d, 0.0, 0.0), Vec3::splat(-16.0), Vec3::splat(16.0)),
                    d,
                )
            }),
            yaw_off: enemy.map(|_| 10.0),
            on_ground: true,
            ..BotFrame::default()
        }
    }

    #[test]
    fn a_throw_at_an_enemy_in_sight_is_followed_to_its_blast_and_credited() {
        let mut s = SatchelStats::default();
        let boxes = [(
            2,
            PlayerBox::new(Vec3::new(400.0, 0.0, 0.0), Vec3::splat(-16.0), Vec3::splat(16.0)),
        )];
        s.frame(1, SimTime(0.0), &frame(2, Some(500.0)));
        s.frame(1, SimTime(0.8), &frame(2, Some(500.0)));
        s.frame(1, SimTime(0.9), &frame(1, Some(450.0)));
        s.seen(7, 1, Vec3::new(50.0, 0.0, 0.0), false, SimTime(0.95), &boxes);
        s.seen(7, 1, Vec3::new(380.0, 0.0, 0.0), true, SimTime(1.9), &boxes);
        s.damage(7, 2, 90.0, SimTime(1.9));
        s.kill(1, 2, Vec3::new(400.0, 0.0, 0.0), SimTime(1.9));
        s.snapshot_done(SimTime(3.0), &boxes);
        assert_eq!(s.throws, 1);
        assert_eq!(s.done.len(), 1);
        let d = &s.done[0];
        assert!(d.burst && d.kills == 1 && d.dealt == 90.0);
        assert!(d.nearest.is_some_and(|n| n < 10.0), "{:?}", d.nearest);
        assert_eq!(s.reactions, vec![0.9]);
        let report = s.report(60.0).join("\n");
        assert!(report.contains("satchels 1 thrown"), "{report}");
        assert!(report.contains("a satchel thrown in"), "{report}");
    }

    #[test]
    fn a_spell_in_sight_without_a_throw_counts_its_death_and_its_waits() {
        let mut s = SatchelStats::default();
        let mut f = frame(2, Some(300.0));
        f.hold = "the enemy too close";
        for i in 0..20 {
            s.frame(1, SimTime(f64::from(i) * 0.05), &f);
        }
        s.died(1, SimTime(1.0));
        assert_eq!((s.spells, s.died_dry, s.died_thrown, s.spells_thrown), (1, 1, 0, 0));
        assert!(s.doing.get("the enemy too close").is_some_and(|t| *t > 0.9));
    }

    #[test]
    fn a_satchel_given_back_is_no_throw_and_one_taken_away_has_no_blast() {
        let mut s = SatchelStats::default();
        s.frame(1, SimTime(0.0), &frame(1, None));
        s.frame(1, SimTime(0.1), &frame(2, None));
        assert_eq!(s.throws, 0);
        s.frame(1, SimTime(0.2), &frame(1, None));
        s.seen(9, 1, Vec3::ZERO, false, SimTime(0.25), &[]);
        s.snapshot_done(SimTime(2.0), &[]);
        assert_eq!(s.throws, 1);
        assert!(!s.done[0].burst && s.done[0].throw.enemy.is_none());
    }

    #[test]
    fn a_set_off_on_the_last_map_gives_no_reason_to_blasts_on_this_one() {
        let mut s = SatchelStats::default();
        s.brain(
            1,
            SimTime(300.0),
            &[],
            Some((SimTime(300.0), "an enemy in the blast")),
            &[],
        );
        s.new_map(400.0);
        s.frame(1, SimTime(0.0), &frame(3, None));
        s.frame(1, SimTime(1.0), &frame(2, None));
        s.seen(5, 1, Vec3::ZERO, false, SimTime(1.05), &[]);
        s.seen(5, 1, Vec3::ZERO, true, SimTime(2.0), &[]);
        s.frame(1, SimTime(2.5), &frame(1, None));
        s.seen(6, 1, Vec3::ZERO, false, SimTime(2.55), &[]);
        s.brain(1, SimTime(3.9), &[], Some((SimTime(3.9), "on chance")), &[]);
        s.seen(6, 1, Vec3::ZERO, true, SimTime(4.0), &[]);
        let whys: Vec<_> = s.flying.iter().map(|(i, s)| (*i, s.why)).collect();
        assert_eq!(whys, vec![(5, None), (6, Some("on chance"))]);
    }

    #[test]
    fn a_throw_as_the_enemy_comes_into_sight_is_thrown_in_its_spell() {
        let mut s = SatchelStats::default();
        s.frame(1, SimTime(0.0), &frame(2, None));
        s.frame(1, SimTime(0.05), &frame(1, Some(400.0)));
        s.died(1, SimTime(0.5));
        assert_eq!((s.spells, s.spells_thrown, s.died_dry, s.died_thrown), (1, 1, 0, 1));
        assert_eq!(s.reactions, vec![0.0]);
    }

    #[test]
    fn the_report_counts_a_spell_still_open_once() {
        let mut s = SatchelStats::default();
        for i in 0..=20 {
            s.frame(
                1,
                SimTime(f64::from(i) * 0.05),
                &frame(if i < 10 { 2 } else { 1 }, Some(300.0)),
            );
        }
        let open = s.report(60.0).join("\n");
        assert!(open.contains("1 s in 1 spells, a satchel thrown in 100%"), "{open}");
        s.frame(1, SimTime(1.6), &frame(1, None));
        assert_eq!(s.spells, 1);
        let ended = s.report(60.0).join("\n");
        assert!(ended.contains("1 s in 1 spells, a satchel thrown in 100%"), "{ended}");
    }

    #[test]
    fn a_spell_open_as_the_map_changes_stays_counted() {
        let mut s = SatchelStats::default();
        for i in 0..=20 {
            s.frame(
                1,
                SimTime(f64::from(i) * 0.05),
                &frame(if i < 10 { 2 } else { 1 }, Some(300.0)),
            );
        }
        let open = s.report(60.0).join("\n");
        assert!(open.contains("1 s in 1 spells, a satchel thrown in 100%"), "{open}");
        s.new_map(1.0);
        assert_eq!(s.spells, 1);
        let after = s.report(59.0).join("\n");
        assert!(after.contains("1 s in 1 spells, a satchel thrown in 100%"), "{after}");
    }

    #[test]
    fn no_enemy_in_sight_with_satchels_in_hand_gives_no_yaw() {
        let mut s = SatchelStats::default();
        s.frame(1, SimTime(0.0), &frame(2, None));
        s.frame(1, SimTime(0.1), &frame(1, None));
        let report = s.report(60.0).join("\n");
        assert!(report.contains("the view's yaw off it - (median)"), "{report}");
    }
}
