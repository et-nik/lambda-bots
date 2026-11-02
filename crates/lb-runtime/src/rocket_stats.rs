//! RPG statistics of the bots for stand runs, the rocket part of `lb stats`: each rocket a bot fires followed from
//! the shot to its blast, and how the bots fare with the launcher in hand. This is measurement, not behavior: it reads
//! what the bots do not know (where everyone is, who took the blast), and nothing here feeds back into them.
//!
//! - **The shot:** whether the laser guide was on, how far the shooter's target was, and whether the shooter's view
//!   ran into the target's box (a guided rocket's spot on the body) or past it.
//! - **The flight:** with the laser on, the time the shooter's view stays on the target's box and off it.
//! - **The blast** goes off where the rocket was last seen: how far that is from the target's box (the blast's falloff
//!   goes by it), the damage it did to others and to the shooter (credited as the rest of `lb stats` credits damage),
//!   and a kill of the shooter's with a rocket a moment after it.
//! - **The pace:** seconds between one bot's rockets in a run of shots (the same life, under 10 s apart), and how soon
//!   after a shot its reload started.
//! - **Close by:** spells with rockets left and the target in sight within the blast's reach, whatever is in hand: their
//!   time, the time of it with the launcher out and with the crowbar, with a rocket in the clip and none fired, the
//!   spells a rocket was fired in, and the deaths in them.
//! - **Holds:** with the launcher in hand and a target in sight, the time split by why the trigger was not pulled.

use lb_core::time::SimTime;
use lb_core::{Vec2, Vec3};
use rustc_hash::FxHashMap;

/// A rocket's blast reaches this far (120 damage, radius 2.5 times that): closer is close by.
pub const CLOSE: f32 = 300.0;
/// One bot's rockets this close in time are one run of shots.
const RUN_GAP: f64 = 10.0;
/// A blast's damage and a kill are credited to the rocket this long after it went off.
const CREDIT_FOR: f64 = 0.5;
/// A rocket first seen this soon after its shooter's shot is that shot's.
const SHOT_MATCH: f64 = 0.3;
/// A rocket first seen gone off is matched to a shot fired no further than this from it.
const BURST_REACH: f32 = 400.0;
/// So much of the rockets, the pace and the reload times is kept, the latest.
const KEEP_LAST: usize = 20_000;
/// A rocket out of the snapshots this long went off where it was last seen.
const GONE_AFTER: f64 = 0.15;
/// A death this soon after a spell close by ended is that spell's (the death ends it).
const SPELL_DEATH: f64 = 0.3;
/// Upper bounds of the bands of the blast's distance off the target's box. The game pulls a blast 58 units out of
/// the surface the rocket hit, so one on the floor at the feet goes off inside the box.
const BLAST_BANDS: [f32; 4] = [10.0, 60.0, 150.0, CLOSE];
const BLAST_NAMES: [&str; 5] = ["<10 u", "10-60", "60-150", "150-300", "further"];
/// A blast this far off the target's box missed it: its splash does at most half the damage.
const MISS: f32 = 60.0;
const DISTANCE_BANDS: [f32; 4] = [CLOSE, 800.0, 1500.0, f32::INFINITY];
const DISTANCE_NAMES: [&str; 4] = ["<300", "300-800", "800-1500", ">1500"];

/// A player's box in the world, and how fast it goes along the ground.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PlayerBox {
    pub min: Vec3,
    pub max: Vec3,
    pub vel: Vec2,
}

impl PlayerBox {
    pub fn new(origin: Vec3, mins: Vec3, maxs: Vec3) -> PlayerBox {
        PlayerBox {
            min: origin + mins,
            max: origin + maxs,
            vel: Vec2::ZERO,
        }
    }

    pub fn moving(self, velocity: Vec3) -> PlayerBox {
        PlayerBox {
            vel: velocity.truncate(),
            ..self
        }
    }

    /// How far `p` is from the box; zero inside it.
    pub fn distance(&self, p: Vec3) -> f32 {
        let clamped = Vec3::new(
            p.x.clamp(self.min.x, self.max.x),
            p.y.clamp(self.min.y, self.max.y),
            p.z.clamp(self.min.z, self.max.z),
        );
        p.distance(clamped)
    }

    /// The ray from `from` along the unit `dir` runs into the box.
    pub fn hit_by(&self, from: Vec3, dir: Vec3) -> bool {
        let (mut near, mut far) = (0.0_f32, f32::INFINITY);
        for axis in 0..3 {
            let (o, d, lo, hi) = (from[axis], dir[axis], self.min[axis], self.max[axis]);
            if d.abs() < 1e-6 {
                if o < lo || o > hi {
                    return false;
                }
                continue;
            }
            let (a, b) = ((lo - o) / d, (hi - o) / d);
            near = near.max(a.min(b));
            far = far.min(a.max(b));
            if near > far {
                return false;
            }
        }
        true
    }
}

/// The shooter's side of a shot: its laser, its target (slot and box) and how far that was, whether its view ran
/// into the target's box, and where its eye was.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Shot {
    pub laser: bool,
    pub target: Option<(u8, PlayerBox)>,
    pub distance: f32,
    pub on_body: Option<bool>,
    pub eye: Vec3,
    pub forward: Vec3,
    pub airborne: bool,
}

impl Shot {
    pub fn of(f: &BotFrame) -> Shot {
        Shot {
            laser: f.laser,
            target: f.target.map(|(slot, b, _)| (slot, b)),
            distance: f.target.map_or(0.0, |(.., d)| d),
            on_body: f.target.map(|(_, b, _)| b.hit_by(f.eye, f.forward)),
            eye: f.eye,
            forward: f.forward,
            airborne: f.airborne,
        }
    }
}

/// What a bot does this frame, for the stand statistics.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BotFrame {
    pub alive: bool,
    /// The bot carries the launcher; it is in hand (or the crowbar), with this many rockets in its clip; how many the
    /// bot has in all.
    pub carries: bool,
    pub in_hand: bool,
    pub melee: bool,
    pub clip: i32,
    pub rockets: i32,
    pub reloading: bool,
    pub laser: bool,
    /// The target in sight: its slot, its box and how far it is.
    pub target: Option<(u8, PlayerBox, f32)>,
    pub eye: Vec3,
    pub forward: Vec3,
    pub airborne: bool,
    pub firing: bool,
    pub hold: Option<&'static str>,
}

#[derive(Clone, Copy, Debug, Default)]
struct Rocket {
    shooter: u8,
    shot: Shot,
    last: Vec3,
    seen: SimTime,
    blast: Option<(SimTime, Vec3)>,
    /// The blast's distance off the target's box, and off the nearest box of anyone else.
    off_target: Option<f32>,
    off_anyone: Option<f32>,
    /// A blast [`MISS`] or more off the target's box: how far past its center along the line from the shooter (less
    /// than zero: short of it), how far aside, and how far over its box's top.
    miss: Option<[f32; 3]>,
    dealt: f32,
    own: f32,
    kills: u32,
    suicide: bool,
    /// Seconds of laser guiding with the shooter's view on the target's box, and off it.
    guided: [f64; 2],
    /// Across the line from the shooter to the target at the shot, along the ground: how far the target went by the
    /// blast, how far the view at the shot was ahead of it, and how fast it went at the shot.
    lead: Option<[f32; 3]>,
    /// At the shooter's last frame before the blast: its target was out of sight; it was dead.
    lost: bool,
    dead: bool,
}

#[derive(Clone, Copy, Debug, Default)]
struct Spell {
    since: SimTime,
    fired: bool,
}

#[derive(Clone, Copy, Debug, Default)]
struct Watch {
    /// The last shot of this life, and what it was.
    last_shot: Option<(SimTime, Shot)>,
    /// The last shot not yet matched to a rocket seen: kept over the shooter's death.
    pending: Option<(SimTime, Shot)>,
    /// A shot whose reload has not been seen to start yet.
    reload_wait: Option<SimTime>,
    close: Option<Spell>,
    ended: Option<(SimTime, Spell)>,
    last_frame: Option<SimTime>,
}

#[derive(Clone, Debug, Default)]
pub struct RocketStats {
    pub since: f64,
    flying: Vec<(u16, Rocket)>,
    done: Vec<Rocket>,
    watch: FxHashMap<u8, Watch>,
    /// Rockets that left a launcher, followed or not.
    shots: u32,
    /// Seconds between rockets in a run of shots, and from a shot to its reload.
    cycles: Vec<f32>,
    reload_after: Vec<f32>,
    spells: u32,
    spell_time: f64,
    /// Close by with the launcher in hand, and with the crowbar.
    close_in_hand: f64,
    close_melee: f64,
    /// Close by with a rocket in the clip and the trigger not pulled.
    loaded_close: f64,
    spells_fired: u32,
    /// Deaths in a spell close by without a rocket fired in it, and with one.
    died_dry: u32,
    died_fired: u32,
    /// With the launcher in hand and a target in sight: in all, and by why the trigger was not pulled.
    in_sight: f64,
    holds: FxHashMap<&'static str, f64>,
    /// Alive without a rocket.
    without: f64,
}

impl RocketStats {
    pub fn reset(&mut self, now: f64) {
        *self = RocketStats {
            since: now,
            ..RocketStats::default()
        };
    }

    /// Sim time restarts with every map: what is under way is dropped (rockets not seen to go off with it), the counts
    /// go on.
    pub fn new_map(&mut self, now: f64) {
        self.since -= now;
        self.done
            .extend(self.flying.drain(..).map(|(_, r)| r).filter(|r| r.blast.is_some()));
        keep_last(&mut self.done);
        self.watch.clear();
    }

    /// A bot's rocket left its launcher (its rockets went down by one).
    pub fn shot(&mut self, shooter: u8, now: SimTime, shot: Shot) {
        self.shots += 1;
        let w = self.watch.entry(shooter).or_default();
        if let Some((at, _)) = w.last_shot
            && now.since(at) < RUN_GAP
        {
            self.cycles.push(now.since(at) as f32);
            keep_last(&mut self.cycles);
        }
        w.last_shot = Some((now, shot));
        w.pending = Some((now, shot));
        w.reload_wait = Some(now);
        if let Some(s) = &mut w.close {
            s.fired = true;
        }
    }

    /// A rocket seen in this frame's snapshot: a new one is its owner's last shot, if a bot fired it. One seen first
    /// gone off has no owner any more (the game clears it in the blast): it is the shot fired nearest to it a moment
    /// ago.
    pub fn seen(&mut self, index: u16, owner: u8, at: Vec3, burst: bool, now: SimTime, boxes: &[(u8, PlayerBox)]) {
        if let Some((_, r)) = self.flying.iter_mut().find(|(i, r)| *i == index && r.blast.is_none()) {
            r.last = at;
            r.seen = now;
            if burst {
                blow(r, at, now, boxes);
            }
            return;
        }
        if self
            .flying
            .iter()
            .any(|(i, r)| *i == index && r.blast.is_some_and(|(t, _)| now.since(t) < CREDIT_FOR))
        {
            return;
        }
        let fresh = |w: &Watch| w.pending.filter(|(t, _)| now.since(*t) <= SHOT_MATCH);
        let shooter = if self.watch.get(&owner).and_then(fresh).is_some() {
            owner
        } else if burst {
            let Some((slot, _)) = self
                .watch
                .iter()
                .filter_map(|(slot, w)| fresh(w).map(|(_, s)| (*slot, s.eye.distance(at))))
                .filter(|(_, d)| *d <= BURST_REACH)
                .min_by(|a, b| a.1.total_cmp(&b.1))
            else {
                return;
            };
            slot
        } else {
            return;
        };
        let Some((_, shot)) = self.watch.get_mut(&shooter).and_then(|w| w.pending.take()) else {
            return;
        };
        let mut r = Rocket {
            shooter,
            shot,
            last: at,
            seen: now,
            ..Rocket::default()
        };
        if burst {
            blow(&mut r, at, now, boxes);
        }
        self.flying.push((index, r));
    }

    /// After this frame's snapshot: rockets no longer in it went off where they were last seen, and those that went off
    /// a while ago are done.
    pub fn snapshot_done(&mut self, now: SimTime, boxes: &[(u8, PlayerBox)]) {
        for (_, r) in &mut self.flying {
            if r.blast.is_none() && now.since(r.seen) > GONE_AFTER {
                let at = r.last;
                let seen = r.seen;
                blow(r, at, seen, boxes);
            }
        }
        let (old, keep): (Vec<_>, Vec<_>) = self
            .flying
            .drain(..)
            .partition(|(_, r)| r.blast.is_some_and(|(t, _)| now.since(t) >= CREDIT_FOR));
        self.flying = keep;
        self.done.extend(old.into_iter().map(|(_, r)| r));
        keep_last(&mut self.done);
    }

    /// Damage the blast of rocket `index` did to the player in `victim`.
    pub fn damage(&mut self, index: u16, victim: u8, damage: f32, now: SimTime) {
        let Some((_, r)) = self
            .flying
            .iter_mut()
            .rev()
            .find(|(i, r)| *i == index && r.blast.is_none_or(|(t, _)| now.since(t) < CREDIT_FOR))
        else {
            return;
        };
        if victim == r.shooter {
            r.own += damage;
        } else {
            r.dealt += damage;
        }
    }

    /// A kill feed line with a rocket: credited to the killer's rocket nearest to where the victim is that went off a
    /// moment before, or is about to be seen to (the kill comes before the snapshot that shows the blast).
    pub fn kill(&mut self, killer: u8, victim: u8, at: Vec3, now: SimTime) {
        if let Some((_, r)) = self
            .flying
            .iter_mut()
            .filter(|(_, r)| r.shooter == killer && r.blast.is_none_or(|(t, _)| now.since(t) < CREDIT_FOR))
            .min_by(|a, b| a.1.last.distance(at).total_cmp(&b.1.last.distance(at)))
        {
            if killer == victim {
                r.suicide = true;
            } else {
                r.kills += 1;
            }
        }
    }

    /// A bot died: its spell close by ends, and so does its run of shots.
    pub fn died(&mut self, slot: u8, now: SimTime) {
        let Some(w) = self.watch.get_mut(&slot) else {
            return;
        };
        let spell = match w.close.take() {
            Some(s) => {
                self.spells += 1;
                self.spell_time += now.since(s.since);
                self.spells_fired += u32::from(s.fired);
                Some(s)
            }
            None => w
                .ended
                .take()
                .filter(|(t, _)| now.since(*t) <= SPELL_DEATH)
                .map(|(_, s)| s),
        };
        if let Some(s) = spell {
            if s.fired {
                self.died_fired += 1;
            } else {
                self.died_dry += 1;
            }
        }
        w.last_shot = None;
        w.reload_wait = None;
    }

    /// Every frame, for each bot.
    pub fn frame(&mut self, slot: u8, now: SimTime, f: &BotFrame) {
        let w = self.watch.entry(slot).or_default();
        let dt = w.last_frame.map_or(0.0, |t| now.since(t).clamp(0.0, 0.1));
        w.last_frame = Some(now);
        for (_, r) in &mut self.flying {
            if r.shooter == slot && r.blast.is_none() {
                r.dead = !f.alive;
                r.lost = f.alive && f.target.map(|(s, _, _)| s) != r.shot.target.map(|(s, _)| s);
            }
        }
        if !f.alive {
            w.last_shot = None;
            w.reload_wait = None;
            if let Some(s) = w.close.take() {
                w.ended = Some((now, s));
                self.spells += 1;
                self.spell_time += now.since(s.since);
                self.spells_fired += u32::from(s.fired);
            }
            return;
        }
        if f.carries && f.rockets <= 0 {
            self.without += dt;
        }
        if let Some(at) = w.reload_wait
            && f.reloading
        {
            self.reload_after.push(now.since(at) as f32);
            keep_last(&mut self.reload_after);
            w.reload_wait = None;
        }
        if let Some((target, b, _)) = f.target.filter(|_| f.in_hand) {
            self.in_sight += dt;
            let why = if f.firing {
                "fired"
            } else {
                f.hold.unwrap_or("none given")
            };
            *self.holds.entry(why).or_default() += dt;
            if f.laser {
                let on = usize::from(!b.hit_by(f.eye, f.forward));
                for (_, r) in &mut self.flying {
                    if r.shooter == slot && r.blast.is_none() && r.shot.target.is_some_and(|(s, _)| s == target) {
                        r.guided[on] += dt;
                    }
                }
            }
        }
        let close = f.rockets > 0 && f.target.is_some_and(|(_, _, d)| d < CLOSE);
        match (close, w.close) {
            (true, None) => {
                w.close = Some(Spell {
                    since: now,
                    fired: false,
                })
            }
            (false, Some(s)) => {
                w.close = None;
                w.ended = Some((now, s));
                self.spells += 1;
                self.spell_time += now.since(s.since);
                self.spells_fired += u32::from(s.fired);
            }
            _ => {}
        }
        if close {
            if f.clip > 0 && !(f.in_hand && f.firing) {
                self.loaded_close += dt;
            }
            if f.in_hand {
                self.close_in_hand += dt;
            } else if f.melee {
                self.close_melee += dt;
            }
        }
    }

    pub fn report(&self, now: f64) -> Vec<String> {
        let gone_off = self.flying.iter().map(|(_, r)| r).filter(|r| r.blast.is_some());
        let rockets: Vec<&Rocket> = self.done.iter().chain(gone_off).collect();
        if rockets.is_empty() && self.in_sight <= 0.0 {
            return Vec::new();
        }
        let minutes = ((now - self.since) / 60.0).max(1e-6);
        let n = rockets.len().max(1) as f32;
        let share = |k: usize| format!("{:.0}%", k as f32 / n * 100.0);
        let lasered = rockets.iter().filter(|r| r.shot.laser).count();
        let suicides = rockets.iter().filter(|r| r.suicide).count();
        let mut out = vec![format!(
            "  rockets {} ({:.1}/min, {} followed), {} with the laser on; {}",
            self.shots,
            f64::from(self.shots) / minutes,
            rockets.len(),
            share(lasered),
            per_rocket(&rockets),
        )];
        out.push(format!(
            "    rockets that hurt someone else {}, their own shooter {}, killed it {suicides}",
            share(rockets.iter().filter(|r| r.dealt > 0.0).count()),
            share(rockets.iter().filter(|r| r.own > 0.0).count()),
        ));
        let killed_it: Vec<&&Rocket> = rockets.iter().filter(|r| r.suicide && r.blast.is_some()).collect();
        if !killed_it.is_empty() {
            let from = |r: &Rocket| r.blast.map_or(0.0, |(_, at)| at.distance(r.shot.eye));
            let mut near: Vec<f32> = killed_it.iter().map(|r| from(r)).collect();
            let of = |k: usize| k as f32 / killed_it.len() as f32 * 100.0;
            out.push(format!(
                "    rockets that killed their shooter: gone off {} from where fired (median), short of the target by 60 u \
                 or more {:.0}%, within 60 u of another player {:.0}%, fired in the air {:.0}%",
                median(&mut near).map_or("-".into(), |m| format!("{m:.0} u")),
                of(killed_it.iter().filter(|r| from(r) < r.shot.distance - MISS).count()),
                of(killed_it
                    .iter()
                    .filter(|r| r.off_anyone.is_some_and(|d| d < MISS) && r.off_target.is_none_or(|d| d >= MISS))
                    .count()),
                of(killed_it.iter().filter(|r| r.shot.airborne).count()),
            ));
        }
        let mut blast = [0usize; 5];
        let mut offs = Vec::new();
        for r in &rockets {
            if let Some(d) = r.off_target {
                blast[BLAST_BANDS.iter().position(|b| d < *b).unwrap_or(4)] += 1;
                offs.push(d);
            }
        }
        let with_target = blast.iter().sum::<usize>().max(1) as f32;
        out.push(format!(
            "    blast off the target's box: {} (median {}, {} rockets without a target)",
            BLAST_NAMES
                .iter()
                .zip(blast)
                .map(|(name, k)| format!("{name} {:.0}%", k as f32 / with_target * 100.0))
                .collect::<Vec<_>>()
                .join(", "),
            median(&mut offs).map_or("-".into(), |m| format!("{m:.0} u")),
            rockets.len() - blast.iter().sum::<usize>(),
        ));
        let anyone: Vec<f32> = rockets.iter().filter_map(|r| r.off_anyone).collect();
        out.push(format!(
            "    blast within 60 u of anyone's box {}, within 150 u {}",
            share(anyone.iter().filter(|d| **d < 60.0).count()),
            share(anyone.iter().filter(|d| **d < 150.0).count()),
        ));
        let bands: Vec<String> = DISTANCE_NAMES
            .iter()
            .enumerate()
            .filter_map(|(i, name)| {
                let lo = if i == 0 { 0.0 } else { DISTANCE_BANDS[i - 1] };
                let these: Vec<&Rocket> = rockets
                    .iter()
                    .copied()
                    .filter(|r| r.shot.target.is_some() && r.shot.distance >= lo && r.shot.distance < DISTANCE_BANDS[i])
                    .collect();
                (!these.is_empty()).then(|| format!("{name} {} ({})", these.len(), short(&these)))
            })
            .collect();
        out.push(format!("    by the target's distance: {}", bands.join(", ")));
        for (laser, name) in [(true, "laser on"), (false, "laser off")] {
            let these: Vec<&Rocket> = rockets.iter().copied().filter(|r| r.shot.laser == laser).collect();
            if !these.is_empty() {
                out.push(format!("    {name}: {} ({})", these.len(), short(&these)));
                out.push(format!("      {}", misses(&these)));
                if !laser {
                    out.push(format!("      {}", lead(&these)));
                }
            }
        }
        let aimed: Vec<bool> = rockets.iter().filter_map(|r| r.shot.on_body).collect();
        let guided = rockets
            .iter()
            .fold([0.0, 0.0], |g, r| [g[0] + r.guided[0], g[1] + r.guided[1]]);
        out.push(format!(
            "    the view on the target's box at the shot {:.0}% of {}; on it while guiding {:.0}% of {:.0} s",
            aimed.iter().filter(|b| **b).count() as f32 / aimed.len().max(1) as f32 * 100.0,
            aimed.len(),
            guided[0] / (guided[0] + guided[1]).max(1e-6) * 100.0,
            guided[0] + guided[1],
        ));
        let mut cycles = self.cycles.clone();
        let mut reloads = self.reload_after.clone();
        let quick = self.reload_after.iter().filter(|t| **t <= 0.3).count();
        out.push(format!(
            "    pace: {} between rockets in a run (median of {}, quartile {}); reload {} after the shot (median of {}), \
             within 0.3 s {:.0}%",
            secs(median(&mut cycles)),
            cycles.len(),
            secs(quantile(&mut cycles, 0.25)),
            secs(median(&mut reloads)),
            reloads.len(),
            quick as f32 / reloads.len().max(1) as f32 * 100.0,
        ));
        out.push(format!(
            "    close by (rockets left, the target in sight within {CLOSE:.0} u): {:.0} s in {} spells, the launcher out \
             {:.0} s, the crowbar {:.0} s, a rocket in the clip and none fired {:.0} s; a rocket fired in {}; died in {} \
             without one, in {} with one",
            self.spell_time,
            self.spells,
            self.close_in_hand,
            self.close_melee,
            self.loaded_close,
            self.spells_fired,
            self.died_dry,
            self.died_fired,
        ));
        let mut holds: Vec<(&&str, &f64)> = self.holds.iter().collect();
        holds.sort_by(|a, b| b.1.total_cmp(a.1).then(a.0.cmp(b.0)));
        out.push(format!(
            "    in hand with a target in sight {:.0} s: {}",
            self.in_sight,
            holds
                .iter()
                .map(|(why, t)| format!("{why} {:.0}%", **t / self.in_sight.max(1e-6) * 100.0))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        out.push(format!("    alive without a rocket {:.0} s", self.without));
        out
    }
}

fn keep_last<T>(v: &mut Vec<T>) {
    if v.len() > KEEP_LAST {
        v.drain(..v.len() - KEEP_LAST / 2);
    }
}

fn blow(r: &mut Rocket, at: Vec3, now: SimTime, boxes: &[(u8, PlayerBox)]) {
    r.blast = Some((now, at));
    let target = r
        .shot
        .target
        .map(|(slot, before)| boxes.iter().find(|(s, _)| *s == slot).map_or(before, |(_, b)| *b));
    r.off_target = target.map(|b| b.distance(at));
    r.lead = r.shot.target.zip(target).map(|((_, before), after)| {
        let from = (before.min + before.max) * 0.5;
        let line = (from - r.shot.eye).truncate();
        let side = Vec2::new(-line.y, line.x).normalize_or_zero();
        let view = r.shot.forward.truncate().normalize_or_zero();
        let moved = ((after.min + after.max) * 0.5 - from).truncate().dot(side);
        let ahead = view.dot(side) / view.dot(line.normalize_or_zero()).max(0.1) * line.length();
        [moved, ahead, before.vel.dot(side)]
    });
    r.miss = target.filter(|b| b.distance(at) >= MISS).map(|b| {
        let center = (b.min + b.max) * 0.5;
        let line = (center - r.shot.eye).truncate().normalize_or_zero();
        let off = (at - center).truncate();
        let along = off.dot(line);
        [along, (off - line * along).length(), at.z - b.max.z]
    });
    r.off_anyone = boxes
        .iter()
        .filter(|(s, _)| *s != r.shooter)
        .map(|(_, b)| b.distance(at))
        .min_by(f32::total_cmp);
}

/// Where the rockets that missed went off: short of the target, past it, or aside, and over its head.
fn misses(rockets: &[&Rocket]) -> String {
    let all: Vec<[f32; 3]> = rockets.iter().filter_map(|r| r.miss).collect();
    let n = rockets.len().max(1) as f32;
    let mut parts = Vec::new();
    for (name, test) in [
        (
            "short",
            (|m: &[f32; 3]| m[0] < 0.0 && -m[0] >= m[1]) as fn(&[f32; 3]) -> bool,
        ),
        ("past", |m| m[0] > 0.0 && m[0] >= m[1]),
        ("aside", |m| m[1] > m[0].abs()),
    ] {
        let mut by: Vec<f32> = all.iter().filter(|m| test(m)).map(|m| m[0].abs().max(m[1])).collect();
        parts.push(format!(
            "{name} {:.0}% ({})",
            by.len() as f32 / n * 100.0,
            median(&mut by).map_or("-".into(), |m| format!("{m:.0} u"))
        ));
    }
    let short: Vec<&&Rocket> = rockets
        .iter()
        .filter(|r| r.miss.is_some_and(|m| m[0] < 0.0 && -m[0] >= m[1]))
        .collect();
    let of_short = |k: usize| k as f32 / short.len().max(1) as f32 * 100.0;
    format!(
        "missed by {MISS:.0} u or more: {}; over its head {:.0}%; short with the target out of sight {:.0}%, the \
         shooter dead {:.0}%",
        parts.join(", "),
        all.iter().filter(|m| m[2] > 0.0).count() as f32 / n * 100.0,
        of_short(short.iter().filter(|r| r.lost).count()),
        of_short(short.iter().filter(|r| r.dead).count()),
    )
}

/// How well the view at the shot led the target across the line to it, by where the target was at the blast.
fn lead(rockets: &[&Rocket]) -> String {
    let all: Vec<[f32; 3]> = rockets.iter().filter_map(|r| r.lead).collect();
    let n = all.len().max(1) as f32;
    let mut speed: Vec<f32> = all.iter().map(|l| l[2].abs()).collect();
    let mut moved: Vec<f32> = all.iter().map(|l| l[0].abs()).collect();
    let mut led: Vec<f32> = all.iter().map(|l| l[1].abs()).collect();
    let mut off: Vec<f32> = all.iter().map(|l| (l[1] - l[0]).abs()).collect();
    let helped = all.iter().filter(|l| (l[1] - l[0]).abs() < l[0].abs()).count();
    let wrong_way = all
        .iter()
        .filter(|l| l[0].abs() >= MISS && l[1].abs() >= MISS && l[0].signum() != l[1].signum())
        .count();
    let too_far = all
        .iter()
        .filter(|l| l[0].signum() == l[1].signum() && l[1].abs() > l[0].abs() + MISS)
        .count();
    format!(
        "across the line: the target went {} at the shot and {} by the blast (medians), the view led it {}, and was {} \
         off where it got; the lead helped {:.0}%, led too far {:.0}%, the wrong way {:.0}%",
        median(&mut speed).map_or("-".into(), |m| format!("{m:.0} u/s")),
        median(&mut moved).map_or("-".into(), |m| format!("{m:.0} u")),
        median(&mut led).map_or("-".into(), |m| format!("{m:.0} u")),
        median(&mut off).map_or("-".into(), |m| format!("{m:.0} u")),
        helped as f32 / n * 100.0,
        too_far as f32 / n * 100.0,
        wrong_way as f32 / n * 100.0,
    )
}

/// Damage to others and to the shooter, and kills, per rocket.
fn per_rocket(rockets: &[&Rocket]) -> String {
    let n = rockets.len().max(1) as f32;
    format!(
        "per rocket {:.0} damage to others, {:.1} to the shooter, {:.2} kills",
        rockets.iter().map(|r| r.dealt).sum::<f32>() / n,
        rockets.iter().map(|r| r.own).sum::<f32>() / n,
        rockets.iter().map(|r| r.kills).sum::<u32>() as f32 / n,
    )
}

fn short(rockets: &[&Rocket]) -> String {
    let n = rockets.len().max(1) as f32;
    format!(
        "hurt {:.0}%, {:.0} dmg, {:.2} kills; own {:.1} dmg, {} suicides",
        rockets.iter().filter(|r| r.dealt > 0.0).count() as f32 / n * 100.0,
        rockets.iter().map(|r| r.dealt).sum::<f32>() / n,
        rockets.iter().map(|r| r.kills).sum::<u32>() as f32 / n,
        rockets.iter().map(|r| r.own).sum::<f32>() / n,
        rockets.iter().filter(|r| r.suicide).count(),
    )
}

fn secs(t: Option<f32>) -> String {
    t.map_or("-".into(), |t| format!("{t:.2} s"))
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

    fn standing(x: f32) -> PlayerBox {
        PlayerBox::new(
            Vec3::new(x, 0.0, 36.0),
            Vec3::new(-16.0, -16.0, -36.0),
            Vec3::new(16.0, 16.0, 36.0),
        )
    }

    #[test]
    fn boxes_are_measured_and_hit() {
        let b = standing(500.0);
        assert_eq!(b.distance(Vec3::new(500.0, 0.0, 30.0)), 0.0);
        assert!((b.distance(Vec3::new(500.0, 0.0, -10.0)) - 10.0).abs() < 1e-3);
        let eye = Vec3::new(0.0, 0.0, 64.0);
        let at_feet = (Vec3::new(500.0, 0.0, 8.0) - eye).normalize();
        assert!(b.hit_by(eye, at_feet), "a view at the feet runs into the box");
        let floor_before = (Vec3::new(440.0, 0.0, 0.0) - eye).normalize();
        assert!(
            !b.hit_by(eye, floor_before),
            "a view at the floor before the feet does not"
        );
        assert!(!b.hit_by(eye, -at_feet));
    }

    #[test]
    fn the_lead_is_told_by_where_the_target_got() {
        let eye = Vec3::new(0.0, 0.0, 64.0);
        let at_shot = standing(800.0);
        let mut moved = at_shot;
        moved.min.y += 200.0;
        moved.max.y += 200.0;
        let fired = |led: f32| {
            let mut r = Rocket {
                shot: Shot {
                    target: Some((2, at_shot)),
                    eye,
                    forward: (Vec3::new(800.0, led, 0.0) - eye).normalize(),
                    ..Shot::default()
                },
                ..Rocket::default()
            };
            blow(&mut r, Vec3::new(800.0, led, 0.0), SimTime(1.0), &[(2, moved)]);
            r
        };
        let rockets = [fired(150.0), fired(450.0), fired(-100.0)];
        let l = rockets[0].lead.unwrap();
        assert!((l[0] - 200.0).abs() < 1e-3 && (l[1] - 150.0).abs() < 0.5, "{l:?}");
        let refs: Vec<&Rocket> = rockets.iter().collect();
        assert_eq!(
            lead(&refs),
            "across the line: the target went 0 u/s at the shot and 200 u by the blast (medians), the view led it 150 \
             u, and was 250 u off where it got; the lead helped 33%, led too far 33%, the wrong way 33%"
        );
    }

    #[test]
    fn misses_are_told_apart() {
        let target = standing(800.0);
        let boxes = [(2, target)];
        let blast_at = |at: Vec3| {
            let mut r = Rocket {
                shot: Shot {
                    target: Some((2, target)),
                    eye: Vec3::new(0.0, 0.0, 64.0),
                    ..Shot::default()
                },
                ..Rocket::default()
            };
            blow(&mut r, at, SimTime(1.0), &boxes);
            r
        };
        let near = blast_at(Vec3::new(760.0, 0.0, 10.0));
        assert_eq!(near.miss, None, "24 u off: a hit");
        let rockets = [
            blast_at(Vec3::new(600.0, 10.0, 0.0)),
            blast_at(Vec3::new(1000.0, 0.0, 0.0)),
            blast_at(Vec3::new(790.0, 300.0, 120.0)),
        ];
        assert!((rockets[0].miss.unwrap()[0] + 200.0).abs() < 1e-3);
        let mut lost = rockets[0];
        lost.lost = true;
        let refs: Vec<&Rocket> = [&lost, &rockets[1], &rockets[2]].into();
        assert_eq!(
            misses(&refs),
            "missed by 60 u or more: short 33% (200 u), past 33% (200 u), aside 33% (300 u); over its head 33%; short \
             with the target out of sight 100%, the shooter dead 0%"
        );
    }

    #[test]
    fn a_rocket_is_followed_from_the_shot_to_its_blast() {
        let mut s = RocketStats::default();
        let target = standing(800.0);
        let boxes = [(2, target), (1, standing(0.0))];
        let shot = Shot {
            laser: true,
            target: Some((2, target)),
            distance: 800.0,
            on_body: Some(true),
            eye: Vec3::new(0.0, 0.0, 64.0),
            forward: Vec3::X,
            airborne: false,
        };
        s.shot(1, SimTime(1.0), shot);
        s.seen(40, 1, Vec3::new(20.0, 0.0, 50.0), false, SimTime(1.05), &boxes);
        let aim = BotFrame {
            alive: true,
            in_hand: true,
            rockets: 4,
            laser: true,
            target: Some((2, target, 800.0)),
            eye: Vec3::new(0.0, 0.0, 64.0),
            forward: (Vec3::new(800.0, 0.0, 30.0) - Vec3::new(0.0, 0.0, 64.0)).normalize(),
            hold: Some("reloading"),
            ..BotFrame::default()
        };
        s.frame(1, SimTime(1.05), &aim);
        s.frame(1, SimTime(1.15), &aim);
        s.seen(40, 1, Vec3::new(740.0, 0.0, 40.0), true, SimTime(1.5), &boxes);
        s.damage(40, 2, 70.0, SimTime(1.55));
        s.kill(1, 2, Vec3::new(800.0, 0.0, 36.0), SimTime(1.55));
        s.snapshot_done(SimTime(2.5), &boxes);
        assert_eq!(s.done.len(), 1);
        let r = &s.done[0];
        assert!((r.off_target.unwrap() - 44.0).abs() < 1e-3, "{r:?}");
        assert_eq!((r.dealt, r.kills), (70.0, 1));
        assert!(r.guided[0] > 0.09, "the view stayed on the box: {r:?}");
        let report = s.report(61.0).join("\n");
        assert!(
            report.contains("rockets 1 (1.0/min, 1 followed), 100% with the laser on"),
            "{report}"
        );
        assert!(report.contains("10-60 100%"), "{report}");
    }

    #[test]
    fn a_rocket_out_of_sight_went_off_where_last_seen() {
        let mut s = RocketStats::default();
        s.shot(3, SimTime(5.0), Shot::default());
        s.seen(41, 3, Vec3::new(100.0, 0.0, 0.0), false, SimTime(5.05), &[]);
        s.snapshot_done(SimTime(5.1), &[]);
        assert!(s.flying[0].1.blast.is_none());
        s.snapshot_done(SimTime(5.3), &[]);
        assert_eq!(s.flying[0].1.blast.map(|b| b.1), Some(Vec3::new(100.0, 0.0, 0.0)));
        s.snapshot_done(SimTime(6.0), &[]);
        assert_eq!(s.done.len(), 1);
        assert!(s.flying.is_empty());
    }

    #[test]
    fn a_rocket_seen_first_gone_off_is_the_shot_nearest_to_it() {
        let mut s = RocketStats::default();
        let near = Shot {
            eye: Vec3::new(0.0, 0.0, 64.0),
            ..Shot::default()
        };
        let far = Shot {
            eye: Vec3::new(2000.0, 0.0, 64.0),
            ..Shot::default()
        };
        s.shot(1, SimTime(1.0), near);
        s.shot(4, SimTime(1.0), far);
        s.died(1, SimTime(1.05));
        s.seen(42, 0, Vec3::new(90.0, 0.0, 20.0), true, SimTime(1.1), &[]);
        assert_eq!(s.flying.len(), 1);
        assert_eq!(s.flying[0].1.shooter, 1);
        assert!(s.flying[0].1.blast.is_some());
        // Too far from any shot, or too late: not followed.
        s.seen(43, 0, Vec3::new(1000.0, 0.0, 20.0), true, SimTime(1.15), &[]);
        s.seen(44, 0, Vec3::new(2000.0, 0.0, 20.0), true, SimTime(1.5), &[]);
        assert_eq!(s.flying.len(), 1);
        // Not yet gone off when the map changed: not counted.
        s.shot(2, SimTime(2.0), Shot::default());
        s.seen(45, 2, Vec3::new(10.0, 0.0, 0.0), false, SimTime(2.05), &[]);
        s.new_map(2.1);
        assert_eq!(s.done.len(), 1);
    }

    #[test]
    fn the_pace_and_the_reload_are_timed() {
        let mut s = RocketStats::default();
        let f = |reloading| BotFrame {
            alive: true,
            in_hand: true,
            rockets: 3,
            reloading,
            ..BotFrame::default()
        };
        s.shot(1, SimTime(1.0), Shot::default());
        s.frame(1, SimTime(1.1), &f(false));
        s.frame(1, SimTime(1.2), &f(true));
        s.shot(1, SimTime(3.0), Shot::default());
        s.shot(1, SimTime(20.0), Shot::default());
        assert_eq!(s.cycles, vec![2.0]);
        assert_eq!(s.reload_after.len(), 1);
        assert!((s.reload_after[0] - 0.2).abs() < 1e-4);
    }

    #[test]
    fn spells_close_by_count_their_deaths() {
        let mut s = RocketStats::default();
        let target = standing(150.0);
        let close = BotFrame {
            alive: true,
            in_hand: true,
            clip: 1,
            rockets: 2,
            target: Some((2, target, 150.0)),
            hold: Some("too close for a rocket"),
            ..BotFrame::default()
        };
        s.frame(1, SimTime(1.0), &close);
        s.frame(1, SimTime(1.1), &close);
        s.frame(1, SimTime(1.2), &close);
        s.died(1, SimTime(1.25));
        s.frame(1, SimTime(1.3), &BotFrame::default());
        assert_eq!((s.spells, s.died_dry, s.died_fired), (1, 1, 0));
        assert!((s.loaded_close - 0.2).abs() < 1e-6);
        // Fired in the next spell, which ends just before the death.
        s.frame(1, SimTime(5.0), &close);
        s.shot(1, SimTime(5.05), Shot::default());
        s.frame(1, SimTime(5.1), &close);
        s.frame(1, SimTime(5.2), &BotFrame { target: None, ..close });
        s.died(1, SimTime(5.3));
        assert_eq!((s.spells, s.spells_fired, s.died_dry, s.died_fired), (2, 1, 1, 1));
        let report = s.report(60.0).join("\n");
        assert!(report.contains("too close for a rocket"), "{report}");
    }
}
