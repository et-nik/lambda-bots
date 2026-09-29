//! Dual utility goal selection, commitment, team board, BehaviorModule interface.
//!
//! Every goal candidate has a rank and a weight (design §4). Candidates under their threshold drop out, the
//! highest rank present wins, and among candidates within 90% of the best weight one is drawn at random. A goal
//! is held for a while once chosen: a higher rank takes over at once, the same rank only with a clear margin.
//!
//! Goals:
//! - rank 2: engage an enemy in sight, retreat when hurt and frightened, collect health when very low;
//! - rank 1: hunt a lost enemy, investigate a sound, collect an item, use a charger, wait by an item about to come
//!   back, hold a spot (camp), lay a trap;
//! - rank 0: roam.
//!
//! GunGame (`Situation::gungame`) changes a few: weapons and ammo are not picked up (the plugin blocks it), a bot
//! with the crowbar alone (the last level, the warmup) hunts the enemies it lost, and on the tripmine level, where
//! nothing hurts a player but the mines, it lays them rather than engaging and gets away from enemies close by.

#![forbid(unsafe_code)]

use lb_combat::Armed;
use lb_core::Vec3;
use lb_core::dmath;
use lb_core::rng::Pcg32;
use lb_core::time::SimTime;
use lb_game::gungame::{GunGame, Kit};
use lb_game::items::{Ammo, ItemKind};
use lb_game::mechanics::{WeaponClass, carry_max, spec};
use lb_game::sounds::SoundKind;
use lb_game::weapons::WeaponId;
use lb_knowledge::{Beliefs, Chargers, HypothesisKind, ItemState, Items, PlayerKey, Relation, TrackState};
use lb_nav_api::{CampKind, MapView};
use lb_styles::GoalAffinity;
use smallvec::SmallVec;

/// A trap to lay: a tripmine on one of the map's mine spots, a pile of satchels at the chokepoint an ambush spot
/// watches, or a satchel or two where an enemy is expected (`Situation::lure`), watched from out of their blast.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trap {
    Mine(u16),
    Satchels(u16),
    Loose,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GoalKind {
    Engage(PlayerKey),
    Hunt(PlayerKey),
    /// Go and see what made a sound (a hypothesis by id).
    Investigate(u32),
    Retreat,
    CollectItem(usize),
    /// Charge health or armor at a wall charger.
    UseCharger(usize),
    /// Wait by an item about to come back.
    ControlItem(usize),
    /// Hold one of the map's camp spots and watch.
    Camp(u16),
    PlantTrap(Trap),
    Roam,
}

impl GoalKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            GoalKind::Engage(_) => "engage",
            GoalKind::Hunt(_) => "hunt",
            GoalKind::Investigate(_) => "investigate",
            GoalKind::Retreat => "retreat",
            GoalKind::CollectItem(_) => "collect",
            GoalKind::UseCharger(_) => "charger",
            GoalKind::ControlItem(_) => "control",
            GoalKind::Camp(_) => "camp",
            GoalKind::PlantTrap(_) => "trap",
            GoalKind::Roam => "roam",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Goal {
    pub kind: GoalKind,
    pub rank: u8,
    pub weight: f32,
    /// Seconds the goal is held once chosen.
    pub hold: f32,
}

/// What the bot knows when it decides.
#[derive(Clone, Copy)]
pub struct Situation<'a> {
    pub now: SimTime,
    pub origin: Vec3,
    pub health: f32,
    pub armor: f32,
    pub has_longjump: bool,
    pub aggression: f32,
    pub fear: f32,
    pub affinity: GoalAffinity,
    pub beliefs: &'a Beliefs,
    pub items: Option<&'a Items>,
    pub chargers: Option<&'a Chargers>,
    pub weapons: &'a [Armed],
    /// Weapons the bot may use, as a mask of weapon bits.
    pub allowed: u32,
    /// How much each ammo type is needed, 0..1 (0 when no owned weapon uses it).
    pub ammo_need: &'a dyn Fn(Ammo) -> f32,
    pub reloading: bool,
    /// The enemy combat is aiming at.
    pub target: Option<PlayerKey>,
    /// Other players on the server (public scoreboard).
    pub opponents: usize,
    pub maxspeed: f32,
    /// The map as the bot knows it; `None` until it is loaded.
    pub map: Option<&'a dyn MapView>,
    /// Seconds since the bot last saw or heard an enemy.
    pub calm_for: f64,
    /// Holding a spot and laying a trap are allowed again (their rests are over).
    pub camp_ready: bool,
    pub trap_ready: bool,
    /// The trap followed now is under way: its mine being laid, or its satchels thrown and watched.
    pub trap_under_way: bool,
    /// Mines the bot knows of, its own and seen ones: no trap next to one.
    pub mines: &'a [Vec3],
    /// Its own satchels lie somewhere.
    pub charges_out: bool,
    /// Where an enemy is expected, for satchels to wait for it by.
    pub lure: Option<Vec3>,
    /// A GunGame match as the bot sees it.
    pub gungame: Option<GunGame>,
}

impl Situation<'_> {
    /// Health for the goal formulas, 0..100: health plus armor as it absorbs bullets, a third of the sum.
    pub fn effective_health(&self) -> f32 {
        ((self.health + 2.0 * self.armor) / 3.0).clamp(0.0, 100.0)
    }

    fn eta(&self, to: Vec3) -> f32 {
        self.origin.distance(to) * 1.4 / self.maxspeed.max(100.0)
    }

    fn allows(&self, w: WeaponId) -> bool {
        self.allowed & w.bit() != 0
    }

    /// Carries one of `ids` with something to fire.
    fn armed_with(&self, ids: &[WeaponId]) -> bool {
        self.weapons
            .iter()
            .any(|a| ids.contains(&a.id) && self.allows(a.id) && (a.loaded() || a.reserve.is_some_and(|r| r > 0)))
    }

    fn carried(&self, w: WeaponId) -> i32 {
        if !self.allows(w) {
            return 0;
        }
        self.weapons
            .iter()
            .find(|a| a.id == w)
            .and_then(|a| a.reserve)
            .unwrap_or(0)
    }

    /// No enemy seen or heard for `secs` and none in sight.
    fn calm(&self, secs: f64) -> bool {
        self.calm_for >= secs && self.beliefs.visible_enemies().next().is_none()
    }

    fn kit(&self) -> Option<Kit> {
        self.gungame.map(|g| g.kit)
    }
}

const ENGAGE_HOLD: f32 = 1.0;
const HUNT_HOLD: f32 = 3.0;
const RETREAT_HOLD: f32 = 2.0;
const ROAM_HOLD: f32 = 5.0;
const RETREAT_THRESHOLD: f32 = 0.4;
const HUNT_THRESHOLD: f32 = 0.5;
/// A lost enemy's place is known well enough to hunt it until it is this uncertain (σ, units): some 8 s of running.
const HUNT_SIGMA: f32 = 3000.0;
const COLLECT_THRESHOLD: f32 = 0.1;
/// A charger is worth it only this low on health (armor).
const CHARGER_HEALTH: f32 = 60.0;
const CHARGER_ARMOR: f32 = 40.0;
/// Seconds of standing at a charger.
const CHARGE_TIME: f32 = 4.0;
const INVESTIGATE_THRESHOLD: f32 = 0.25;
/// Sounds this far are not gone to.
const INVESTIGATE_RANGE: [f32; 2] = [200.0, 2500.0];
const CAMP_THRESHOLD: f32 = 0.15;
const CAMP_RANGE: f32 = 2500.0;
/// Calm this long before holding a spot or laying a trap, and this much health.
const CAMP_CALM: f64 = 3.0;
const TRAP_CALM: f64 = 5.0;
const CAMP_HEALTH: f32 = 50.0;
const CONTROL_THRESHOLD: f32 = 0.2;
/// An item waited for that is back is taken from this close.
const CONTROL_TAKE: f32 = 500.0;
/// Waiting longer than this for an item is not worth it; up to the first number it is as good as any.
const CONTROL_WAIT: [f32; 2] = [8.0, 25.0];
const TRAP_THRESHOLD: f32 = 0.15;
const TRAP_RANGE: f32 = 2000.0;
/// No trap closer than this to a known mine.
const TRAP_SPACING: f32 = 128.0;
/// Satchels at a chokepoint are watched from an ambush spot at least this far, out of their blast.
const SATCHEL_WATCH: f32 = 350.0;
/// Satchels where an enemy is expected: the weight for a balanced bot (a trapper's is higher), and how far the spot
/// may be.
const LURE_WEIGHT: f32 = 0.45;
const LURE_RANGE: f32 = 1200.0;
/// A GunGame bot with only the crowbar hunts a lost enemy with at least this weight (yapb's knife level desire 70).
const KNIFE_HUNT: f32 = 0.7;
/// On the tripmine level: enemies in sight this close are got away from, with at least this weight; mine spots are
/// liked this much (a balanced trapper's is 1), and one is laid again this soon after the last.
const MINES_EVADE: f32 = 700.0;
const MINES_RETREAT: f32 = 0.6;
const MINES_TRAP: f32 = 1.5;
/// Weapons to hold a spot with: long sightlines, close by a chokepoint.
const LONG_GUNS: [WeaponId; 4] = [WeaponId::Crossbow, WeaponId::Python, WeaponId::Gauss, WeaponId::Rpg];
const CLOSE_GUNS: [WeaponId; 4] = [WeaponId::Shotgun, WeaponId::Mp5, WeaponId::Egon, WeaponId::Gauss];

/// Weight of the retreat goal, also subtracted from hunting.
fn retreat_weight(s: &Situation<'_>) -> f32 {
    let now = s.now;
    let mut age = f64::INFINITY;
    for t in s.beliefs.enemies() {
        age = age.min(now.since(t.last_seen));
    }
    if let Some(d) = s.beliefs.last_damage {
        age = age.min(now.since(d.t));
    }
    let recency = ((10.0 - age) / 10.0).clamp(0.0, 1.0) as f32;
    if recency <= 0.0 {
        return 0.0;
    }
    let guns = s
        .weapons
        .iter()
        .filter(|a| !matches!(spec(a.id).class, WeaponClass::Melee | WeaponClass::Throwable));
    let loaded = guns.clone().filter(|a| a.loaded()).count();
    let mut mult = 0.5;
    if loaded == 0 {
        mult *= 2.0;
    } else if s.reloading && loaded <= 1 {
        mult *= 3.0;
    }
    (100.0 - s.effective_health()) * s.fear / 100.0 * recency * mult * s.affinity.retreat
}

fn item_benefit(s: &Situation<'_>, kind: ItemKind) -> f32 {
    let owns = |w| s.weapons.iter().any(|a| a.id == w);
    if s.gungame.is_some() && matches!(kind, ItemKind::Weapon(_) | ItemKind::Ammo(_)) {
        return 0.0;
    }
    match kind {
        ItemKind::Health if s.health < 85.0 => 0.8 * (1.0 - s.health / 100.0),
        ItemKind::Battery if s.armor < 90.0 => 0.6 * (1.0 - s.armor / 100.0),
        ItemKind::LongJump if !s.has_longjump => 0.8,
        // Grenades, satchels, snarks and mines are their own ammo: worth topping up.
        ItemKind::Weapon(w) if spec(w).class == WeaponClass::Throwable => {
            let carried = s
                .weapons
                .iter()
                .find(|a| a.id == w)
                .and_then(|a| a.reserve)
                .unwrap_or(0);
            0.3 * (1.0 - carried as f32 / carry_max(w).max(1) as f32).max(0.0)
        }
        ItemKind::Weapon(w) if !owns(w) => 0.4 + 0.03 * f32::from(spec(w).rank),
        ItemKind::Weapon(w) => {
            // Owned: only its ammo is worth something.
            let ammo = match w {
                WeaponId::Glock | WeaponId::Mp5 => Some(Ammo::Nine),
                WeaponId::Shotgun => Some(Ammo::Buckshot),
                WeaponId::Python => Some(Ammo::Magnum),
                WeaponId::Crossbow => Some(Ammo::Bolts),
                WeaponId::Rpg => Some(Ammo::Rockets),
                WeaponId::Gauss | WeaponId::Egon => Some(Ammo::Uranium),
                _ => None,
            };
            ammo.map_or(0.0, |a| 0.2 * (s.ammo_need)(a))
        }
        ItemKind::Ammo(a) => {
            let need = (s.ammo_need)(a);
            if need > 0.3 { 0.4 * need } else { 0.0 }
        }
        _ => 0.0,
    }
}

/// What an item about to come back is worth waiting for: what it gives the bot, and a little for keeping it from
/// others.
fn control_value(s: &Situation<'_>, kind: ItemKind) -> f32 {
    let owns = |w| s.weapons.iter().any(|a| a.id == w);
    if s.gungame.is_some() && matches!(kind, ItemKind::Weapon(_) | ItemKind::Ammo(_)) {
        return 0.0;
    }
    match kind {
        ItemKind::Battery => 0.15 + 0.6 * (1.0 - s.armor / 100.0),
        ItemKind::Health => 0.1 + 0.5 * (1.0 - s.health / 100.0),
        ItemKind::LongJump if !s.has_longjump => 0.7,
        ItemKind::Weapon(w) if spec(w).class != WeaponClass::Throwable && !owns(w) => {
            0.2 + 0.04 * f32::from(spec(w).rank)
        }
        _ => 0.0,
    }
}

/// Sounds worth going to see about: someone fighting, walking, hurt or picking something up.
fn worth_investigating(kind: HypothesisKind) -> bool {
    match kind {
        HypothesisKind::Sound(k) => matches!(
            k,
            SoundKind::Shot
                | SoundKind::Step
                | SoundKind::Jump
                | SoundKind::Pain
                | SoundKind::WeaponNoise
                | SoundKind::Pickup
                | SoundKind::Explosion
        ),
        HypothesisKind::Cue => true,
        HypothesisKind::Damage => false,
    }
}

/// Sounds heard this close to the one being seen about are the same business.
const SAME_PLACE: f32 = 400.0;

fn investigations(s: &Situation<'_>, current: Option<GoalKind>, out: &mut Vec<Goal>) {
    let aggr = s.aggression.clamp(0.0, 1.0);
    let going = match current {
        Some(GoalKind::Investigate(id)) => s.beliefs.hypothesis(id).and_then(|h| Some((id, h.pos?))),
        _ => None,
    };
    let mut best: SmallVec<[Goal; 8]> = SmallVec::new();
    for h in &s.beliefs.hypotheses {
        let Some(pos) = h.pos else { continue };
        // A sound tied to an enemy the bot tracks is the hunt's.
        let tracked = h
            .track
            .and_then(|k| s.beliefs.track(k))
            .is_some_and(|t| t.state != TrackState::Stale);
        let life = match h.kind {
            HypothesisKind::Cue => 1.0,
            _ => 5.0,
        };
        let age = s.now.since(h.t) as f32;
        let d = pos.distance(s.origin);
        if tracked
            || !worth_investigating(h.kind)
            || age > life
            || !(INVESTIGATE_RANGE[0]..=INVESTIGATE_RANGE[1]).contains(&d)
        {
            continue;
        }
        let w = 0.6 * (0.3 + h.strength) * (1.0 - age / life) * (0.5 + aggr) * s.affinity.investigate;
        if w < INVESTIGATE_THRESHOLD {
            continue;
        }
        let eta = s.eta(pos);
        let kind = match going {
            Some((id, at)) if at.distance(pos) <= SAME_PLACE => GoalKind::Investigate(id),
            _ => GoalKind::Investigate(h.id),
        };
        match best.iter_mut().find(|g| g.kind == kind) {
            Some(g) => g.weight = g.weight.max(w.min(1.0)),
            None => best.push(Goal {
                kind,
                rank: 1,
                weight: w.min(1.0),
                hold: (1.5 * eta + 3.0).min(12.0),
            }),
        }
    }
    best.sort_by(|a, b| b.weight.total_cmp(&a.weight));
    out.extend(best.into_iter().take(3));
}

fn camps(s: &Situation<'_>, out: &mut Vec<Goal>) {
    let Some(map) = s.map else { return };
    if !s.camp_ready || !s.calm(CAMP_CALM) || s.health < CAMP_HEALTH {
        return;
    }
    let suit = |kind: CampKind| match kind {
        CampKind::Overwatch if s.armed_with(&LONG_GUNS) => 1.0,
        CampKind::Overwatch if s.armed_with(&[WeaponId::Mp5]) => 0.4,
        CampKind::Ambush if s.armed_with(&CLOSE_GUNS) => 1.0,
        CampKind::Ambush if s.armed_with(&[WeaponId::Glock, WeaponId::Hornetgun]) => 0.4,
        _ => 0.0,
    };
    let aggr = s.aggression.clamp(0.0, 1.0);
    let best = map
        .camp_spots()
        .iter()
        .enumerate()
        .filter(|(_, c)| c.pos.distance(s.origin) <= CAMP_RANGE)
        .map(|(i, c)| {
            let like = match c.kind {
                CampKind::Overwatch => s.affinity.camp,
                CampKind::Ambush => s.affinity.ambush,
            };
            let eta = s.eta(c.pos);
            let w = like * c.score * (1.1 - aggr) * suit(c.kind) * dmath::exp(-eta / 12.0);
            (i, w, eta)
        })
        .max_by(|a, b| a.1.total_cmp(&b.1).then(b.0.cmp(&a.0)));
    if let Some((i, w, eta)) = best
        && w >= CAMP_THRESHOLD
    {
        out.push(Goal {
            kind: GoalKind::Camp(i as u16),
            rank: 1,
            weight: w.min(1.0),
            hold: 1.5 * eta + 8.0,
        });
    }
}

fn controls(s: &Situation<'_>, current: Option<GoalKind>, out: &mut Vec<Goal>) {
    let Some(items) = s.items else { return };
    // Back while waited for: the wait goes on until it is taken.
    if let Some(GoalKind::ControlItem(i)) = current
        && items.beliefs.get(i).is_some_and(|b| b.state == ItemState::Present)
        && items.spots[i].origin.distance(s.origin) <= CONTROL_TAKE
    {
        out.push(Goal {
            kind: GoalKind::ControlItem(i),
            rank: 1,
            weight: (s.affinity.control * control_value(s, items.spots[i].kind)).clamp(CONTROL_THRESHOLD, 1.0),
            hold: 3.0,
        });
        return;
    }
    if !s.calm(CAMP_CALM) {
        return;
    }
    let mut best: Option<Goal> = None;
    for (i, spot) in items.spots.iter().enumerate() {
        let ItemState::Absent { back: (from, to) } = items.beliefs[i].state else {
            continue;
        };
        let value = control_value(s, spot.kind);
        if value <= 0.0 {
            continue;
        }
        let eta = s.eta(spot.origin);
        let arrive = s.now + f64::from(eta);
        if arrive > to + 5.0 {
            continue;
        }
        let wait = from.since(arrive) as f32;
        let fit = if wait < 0.0 {
            0.6
        } else {
            (1.0 - (wait - CONTROL_WAIT[0]).max(0.0) / (CONTROL_WAIT[1] - CONTROL_WAIT[0])).max(0.0)
        };
        let w = s.affinity.control * value * fit;
        if w >= CONTROL_THRESHOLD && best.is_none_or(|b| w > b.weight) {
            best = Some(Goal {
                kind: GoalKind::ControlItem(i),
                rank: 1,
                weight: w.min(1.0),
                hold: eta + wait.max(0.0) + 3.0,
            });
        }
    }
    out.extend(best);
}

fn traps(s: &Situation<'_>, current: Option<GoalKind>, out: &mut Vec<Goal>) {
    let Some(map) = s.map else { return };
    // Under way, the trap goes on until it is over, its satchels out and its rest begun: only a higher rank takes
    // over.
    if let Some(GoalKind::PlantTrap(trap)) = current
        && s.trap_under_way
    {
        out.push(Goal {
            kind: GoalKind::PlantTrap(trap),
            rank: 1,
            weight: 1.0,
            hold: 0.0,
        });
        return;
    }
    // On the tripmine level mines are the only weapon: laid whenever no enemy is in sight.
    let mines_level = s.kit() == Some(Kit::Mines);
    let ready = if mines_level {
        s.beliefs.visible_enemies().next().is_none()
    } else {
        s.calm(TRAP_CALM) && s.affinity.trap > 0.0
    };
    if !s.trap_ready || !ready {
        return;
    }
    let like = if mines_level { MINES_TRAP } else { s.affinity.trap };
    if s.carried(WeaponId::Satchel) >= 1
        && !s.charges_out
        && let Some(at) = s.lure.filter(|p| p.distance(s.origin) <= LURE_RANGE)
    {
        let eta = s.eta(at);
        let w = LURE_WEIGHT * (0.6 + 0.2 * s.affinity.trap) * dmath::exp(-eta / 10.0);
        if w >= TRAP_THRESHOLD {
            out.push(Goal {
                kind: GoalKind::PlantTrap(Trap::Loose),
                rank: 1,
                weight: w.min(1.0),
                hold: 1.5 * eta + 30.0,
            });
        }
    }
    let free = |p: Vec3| s.mines.iter().all(|m| m.distance(p) >= TRAP_SPACING);
    if s.carried(WeaponId::Tripmine) > 0 {
        let best = map
            .mine_spots()
            .iter()
            .enumerate()
            .filter(|(_, m)| m.stand.distance(s.origin) <= TRAP_RANGE && free(m.wall))
            .map(|(i, m)| {
                let eta = s.eta(m.stand);
                let corner = if m.corner { 1.1 } else { 1.0 };
                (
                    i,
                    like * (0.25 + 0.75 * m.flow) * 0.6 * corner * dmath::exp(-eta / 10.0),
                    eta,
                )
            })
            .max_by(|a, b| a.1.total_cmp(&b.1).then(b.0.cmp(&a.0)));
        if let Some((i, w, eta)) = best
            && w >= TRAP_THRESHOLD
        {
            out.push(Goal {
                kind: GoalKind::PlantTrap(Trap::Mine(i as u16)),
                rank: 1,
                weight: w.min(1.0),
                hold: 1.5 * eta + 6.0,
            });
        }
    }
    if s.carried(WeaponId::Satchel) >= 2 && !s.charges_out {
        let best = map
            .camp_spots()
            .iter()
            .enumerate()
            .filter(|(_, c)| {
                c.kind == CampKind::Ambush
                    && c.guards.is_some()
                    && c.range >= SATCHEL_WATCH
                    && c.pos.distance(s.origin) <= TRAP_RANGE
            })
            .map(|(i, c)| {
                let eta = s.eta(c.pos);
                (i, s.affinity.trap * c.score * 0.5 * dmath::exp(-eta / 10.0), eta)
            })
            .max_by(|a, b| a.1.total_cmp(&b.1).then(b.0.cmp(&a.0)));
        if let Some((i, w, eta)) = best
            && w >= TRAP_THRESHOLD
        {
            out.push(Goal {
                kind: GoalKind::PlantTrap(Trap::Satchels(i as u16)),
                rank: 1,
                weight: w.min(1.0),
                hold: 1.5 * eta + 20.0,
            });
        }
    }
}

/// All goal candidates in this situation, `current` the goal followed now.
pub fn candidates(s: &Situation<'_>, current: Option<GoalKind>, out: &mut Vec<Goal>) {
    out.clear();
    let aggr = s.aggression.clamp(0.0, 1.0);
    let mines_level = s.kit() == Some(Kit::Mines);
    let mut retreat = retreat_weight(s);
    if mines_level
        && s.beliefs
            .visible_enemies()
            .any(|t| t.pos.distance(s.origin) < MINES_EVADE)
    {
        retreat = retreat.max(MINES_RETREAT);
    }
    if retreat >= RETREAT_THRESHOLD {
        out.push(Goal {
            kind: GoalKind::Retreat,
            rank: 2,
            weight: retreat.min(1.0),
            hold: RETREAT_HOLD,
        });
    }
    let engage = s
        .target
        .and_then(|k| s.beliefs.track(k))
        .filter(|t| t.state == TrackState::Visible || s.now.since(t.last_seen) <= 0.5);
    if let Some(t) = engage.filter(|_| !mines_level) {
        out.push(Goal {
            kind: GoalKind::Engage(t.who),
            rank: 2,
            weight: 0.9 * (0.6 + 0.4 * aggr) * s.affinity.engage,
            hold: ENGAGE_HOLD,
        });
    }
    for t in s.beliefs.tracks.iter().filter(|t| {
        t.relation == Relation::Enemy && matches!(t.state, TrackState::RecentlyLost | TrackState::Predicted)
    }) {
        let d = t.pos.distance(s.origin);
        let confidence = (1.0 - t.sigma / HUNT_SIGMA).clamp(0.0, 1.0);
        let mut w = (((4096.0 - (1.0 - aggr) * d) / 4096.0 - retreat).min(0.89)) * confidence * s.affinity.hunt;
        if s.kit() == Some(Kit::Crowbar) && confidence > 0.0 {
            w = w.max(KNIFE_HUNT);
        }
        if w >= HUNT_THRESHOLD && !mines_level {
            out.push(Goal {
                kind: GoalKind::Hunt(t.who),
                rank: 1,
                weight: w,
                hold: HUNT_HOLD,
            });
        }
    }
    investigations(s, current, out);
    if let Some(items) = s.items {
        let threat_near = s
            .beliefs
            .enemies()
            .any(|t| t.state != TrackState::Stale && t.pos.distance(s.origin) < 800.0);
        for (i, spot) in items.spots.iter().enumerate() {
            let benefit = item_benefit(s, spot.kind);
            if benefit <= 0.0 {
                continue;
            }
            let d = spot.origin.distance(s.origin);
            let eta = s.eta(spot.origin);
            let available = items.availability(i, s.now, eta, s.opponents);
            let mut w = (benefit * s.affinity.collect).min(1.0) * available * dmath::exp(-eta / 10.0);
            let b = &items.beliefs[i];
            let seen_there =
                b.checked_at.is_some_and(|t| s.now.since(t) <= 1.0) && b.present_at == b.checked_at.unwrap();
            if seen_there && d < 450.0 {
                w = w.max(0.5 + 0.45 * (1.0 - d / 450.0));
            }
            if w < COLLECT_THRESHOLD {
                continue;
            }
            let urgent = s.effective_health() < 30.0
                && matches!(spot.kind, ItemKind::Health | ItemKind::Battery)
                && !threat_near;
            out.push(Goal {
                kind: GoalKind::CollectItem(i),
                rank: if urgent { 2 } else { 1 },
                weight: w.min(1.0),
                hold: 1.5 * eta + 3.0,
            });
        }
    }
    if let Some(chargers) = s.chargers {
        let threat_near = s
            .beliefs
            .enemies()
            .any(|t| t.state != TrackState::Stale && t.pos.distance(s.origin) < 800.0);
        for (i, c) in chargers.spots.iter().enumerate() {
            let benefit = if c.suit {
                if s.armor < CHARGER_ARMOR {
                    0.6 * (1.0 - s.armor / 100.0)
                } else {
                    0.0
                }
            } else if s.health < CHARGER_HEALTH {
                0.8 * (1.0 - s.health / 100.0)
            } else {
                0.0
            };
            if benefit <= 0.0 {
                continue;
            }
            let eta = s.eta(c.spot);
            if !chargers.available(i, s.now + f64::from(eta)) {
                continue;
            }
            let w = (benefit * s.affinity.collect).min(1.0) * dmath::exp(-(eta + CHARGE_TIME) / 10.0);
            if w < COLLECT_THRESHOLD {
                continue;
            }
            let urgent = !c.suit && s.effective_health() < 30.0 && !threat_near;
            out.push(Goal {
                kind: GoalKind::UseCharger(i),
                rank: if urgent { 2 } else { 1 },
                weight: w,
                hold: 1.5 * eta + 3.0 + 2.0 * CHARGE_TIME,
            });
        }
    }
    controls(s, current, out);
    camps(s, out);
    traps(s, current, out);
    out.push(Goal {
        kind: GoalKind::Roam,
        rank: 0,
        weight: 0.2 * s.affinity.roam,
        hold: ROAM_HOLD,
    });
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Active {
    pub goal: Goal,
    pub since: SimTime,
    pub hold_until: SimTime,
}

/// How often the goal changed, and how long each kind was followed.
#[derive(Clone, Debug, Default)]
pub struct GoalStats {
    /// Changes from one goal to another, among them those before the old one's hold ran out, and those with no
    /// fight on either side (neither goal engages or retreats).
    pub switches: u32,
    pub early: u32,
    pub calm: u32,
    /// Goals taken up, by kind name.
    pub taken: SmallVec<[(&'static str, u32); 12]>,
}

impl GoalStats {
    fn took(&mut self, kind: GoalKind) {
        let name = kind.as_str();
        match self.taken.iter_mut().find(|(n, _)| *n == name) {
            Some(e) => e.1 += 1,
            None => self.taken.push((name, 1)),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Decider {
    pub current: Option<Active>,
    cooldowns: SmallVec<[(GoalKind, SimTime); 8]>,
    /// Candidates of the last decision, best first (for traces and `lb` commands).
    pub last: Vec<Goal>,
    pub stats: GoalStats,
    scratch: Vec<Goal>,
}

impl Decider {
    pub fn reset(&mut self) {
        self.current = None;
        self.cooldowns.clear();
        self.last.clear();
    }

    /// The goal was reached: pick anew next time.
    pub fn complete(&mut self) {
        self.current = None;
    }

    /// The goal could not be reached: do not pick it again for 8–15 s.
    pub fn fail(&mut self, now: SimTime, rng: &mut Pcg32) {
        if let Some(a) = self.current.take()
            && a.goal.kind != GoalKind::Roam
        {
            self.cooldowns
                .push((a.goal.kind, now + f64::from(rng.range_f32(8.0, 15.0))));
        }
    }

    /// Keeps a goal from being picked until `until` (a spot just held, say).
    pub fn rest(&mut self, kind: GoalKind, until: SimTime) {
        self.cooldowns.push((kind, until));
    }

    pub fn decide(&mut self, s: &Situation<'_>, rng: &mut Pcg32) -> Goal {
        let now = s.now;
        self.cooldowns.retain(|(_, until)| *until > now);
        let mut all = std::mem::take(&mut self.scratch);
        candidates(s, self.current.map(|c| c.goal.kind), &mut all);
        all.retain(|g| !self.cooldowns.iter().any(|(k, _)| *k == g.kind));
        all.sort_by(|a, b| {
            (b.rank, b.weight)
                .partial_cmp(&(a.rank, a.weight))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let best = all[0];
        let pool: SmallVec<[Goal; 8]> = all
            .iter()
            .filter(|g| g.rank == best.rank && g.weight >= best.weight * 0.9)
            .copied()
            .collect();
        let pick = pool[rng.range_i32(0, pool.len() as i32 - 1) as usize];
        let chosen = match self.current {
            Some(cur) => match all.iter().find(|g| g.kind == cur.goal.kind) {
                // The current goal is still possible: keep it unless something clearly better came up.
                Some(still) => {
                    let margin = if cur.goal.kind == GoalKind::Retreat && matches!(pick.kind, GoalKind::Engage(_)) {
                        still.weight * 1.25
                    } else {
                        still.weight * 1.15 + 0.05
                    };
                    let switch = pick.rank > still.rank || (now >= cur.hold_until && pick.weight > margin);
                    if switch { pick } else { *still }
                }
                None => pick,
            },
            None => pick,
        };
        let keep = self.current.is_some_and(|c| c.goal.kind == chosen.kind);
        if let Some(cur) = self.current
            && !keep
        {
            self.stats.switches += 1;
            if now < cur.hold_until {
                self.stats.early += 1;
            }
            let fight = |k: GoalKind| matches!(k, GoalKind::Engage(_) | GoalKind::Retreat);
            if !fight(cur.goal.kind) && !fight(chosen.kind) {
                self.stats.calm += 1;
            }
        }
        if !keep {
            self.stats.took(chosen.kind);
        }
        self.current = Some(match self.current {
            Some(c) if keep => Active { goal: chosen, ..c },
            _ => Active {
                goal: chosen,
                since: now,
                hold_until: now + f64::from(chosen.hold),
            },
        });
        self.last.clear();
        self.last.extend(all.iter().take(6).copied());
        self.scratch = all;
        chosen
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lb_knowledge::*;

    const BALANCED: GoalAffinity = GoalAffinity {
        engage: 1.0,
        hunt: 1.0,
        retreat: 1.0,
        collect: 1.0,
        roam: 1.0,
        investigate: 1.0,
        camp: 0.4,
        ambush: 0.5,
        control: 0.6,
        trap: 0.5,
    };

    fn sighting(t: f64, pos: Vec3) -> Sighting {
        Sighting {
            who: PlayerKey { slot: 3, userid: 30 },
            relation: Relation::Enemy,
            t: SimTime(t),
            pos,
            sigma: 1.0,
            distance: pos.length(),
            visibility: 1.0,
            parts: parts::CHEST,
            stance: Stance::Standing,
            on_ground: true,
            on_ladder: false,
            in_water: false,
            facing: 0.0,
            weapon: None,
            firing: false,
            render: RenderCue::default(),
            first: true,
            noticed_at: SimTime::ZERO,
        }
    }

    fn situation<'a>(
        now: f64,
        beliefs: &'a Beliefs,
        items: Option<&'a Items>,
        weapons: &'a [Armed],
        health: f32,
        target: Option<PlayerKey>,
        need: &'a dyn Fn(Ammo) -> f32,
    ) -> Situation<'a> {
        Situation {
            now: SimTime(now),
            origin: Vec3::ZERO,
            health,
            armor: 0.0,
            has_longjump: false,
            aggression: 0.55,
            fear: 0.55,
            affinity: BALANCED,
            beliefs,
            items,
            chargers: None,
            weapons,
            allowed: u32::MAX,
            ammo_need: need,
            reloading: false,
            target,
            opponents: 4,
            maxspeed: 300.0,
            map: None,
            calm_for: f64::INFINITY,
            camp_ready: true,
            trap_ready: true,
            trap_under_way: false,
            mines: &[],
            charges_out: false,
            lure: None,
            gungame: None,
        }
    }

    const KIT: [Armed; 2] = [
        Armed {
            id: WeaponId::Crowbar,
            clip: None,
            reserve: None,
            reserve2: None,
        },
        Armed {
            id: WeaponId::Glock,
            clip: Some(17),
            reserve: Some(68),
            reserve2: None,
        },
    ];

    #[test]
    fn a_visible_enemy_is_engaged_and_a_lost_one_hunted() {
        let mut b = Beliefs::default();
        let p = BeliefParams {
            track_forget: 8.0,
            maxspeed: 300.0,
        };
        b.on_sighting(&sighting(0.0, Vec3::new(600.0, 0.0, 0.0)));
        b.update(SimTime(0.0), &p);
        let none = |_| 0.0;
        let key = PlayerKey { slot: 3, userid: 30 };
        let mut d = Decider::default();
        let mut rng = Pcg32::new(1, 1);
        let g = d.decide(&situation(0.0, &b, None, &KIT, 100.0, Some(key), &none), &mut rng);
        assert_eq!(g.kind, GoalKind::Engage(key));
        b.update(SimTime(1.5), &p);
        let g = d.decide(&situation(1.5, &b, None, &KIT, 100.0, None, &none), &mut rng);
        assert_eq!(g.kind, GoalKind::Hunt(key), "lost 1.5 s ago, close: worth chasing");
        b.update(SimTime(6.0), &p);
        let g = d.decide(&situation(6.0, &b, None, &KIT, 100.0, None, &none), &mut rng);
        assert_eq!(g.kind, GoalKind::Hunt(key), "6 s on, still worth chasing");
        b.update(SimTime(9.0), &p);
        let g = d.decide(&situation(9.0, &b, None, &KIT, 100.0, None, &none), &mut rng);
        assert_eq!(g.kind, GoalKind::Roam, "forgotten by now");
        assert_eq!(d.stats.switches, 2);
    }

    #[test]
    fn hurt_and_frightened_bots_retreat_and_hurt_ones_want_health() {
        let mut b = Beliefs::default();
        b.on_sighting(&sighting(0.0, Vec3::new(600.0, 0.0, 0.0)));
        let dry = [
            KIT[0],
            Armed {
                clip: Some(0),
                reserve: Some(0),
                ..KIT[1]
            },
        ];
        let none = |_| 0.0;
        let mut s = situation(0.5, &b, None, &dry, 15.0, None, &none);
        s.fear = 0.9;
        let mut d = Decider::default();
        let mut rng = Pcg32::new(2, 2);
        assert_eq!(d.decide(&s, &mut rng).kind, GoalKind::Retreat);
        let spots = [ItemSpot {
            kind: ItemKind::Health,
            origin: Vec3::new(300.0, 0.0, 0.0),
        }];
        let items = Items::new(&spots, SimTime(0.0));
        let calm = Beliefs::default();
        let s = situation(20.0, &calm, Some(&items), &KIT, 40.0, None, &none);
        let mut d = Decider::default();
        assert_eq!(d.decide(&s, &mut rng).kind, GoalKind::CollectItem(0));
        let s = situation(20.0, &calm, Some(&items), &KIT, 100.0, None, &none);
        let mut d = Decider::default();
        assert_eq!(
            d.decide(&s, &mut rng).kind,
            GoalKind::Roam,
            "a healthy bot leaves the medkit"
        );
    }

    fn gungame(level_frags: i32, kit: &[WeaponId]) -> Option<GunGame> {
        let board = lb_game::gungame::Board::new([(1, level_frags, 0), (3, 500, 0)], 100, true);
        let weapons = kit.iter().fold(0, |m, w| m | w.bit());
        Some(GunGame::new(&board, 1, weapons))
    }

    #[test]
    fn gungame_bots_leave_weapons_and_ammo_but_take_health() {
        let spots = [
            ItemSpot {
                kind: ItemKind::Weapon(WeaponId::Shotgun),
                origin: Vec3::new(200.0, 0.0, 0.0),
            },
            ItemSpot {
                kind: ItemKind::Ammo(Ammo::Buckshot),
                origin: Vec3::new(150.0, 0.0, 0.0),
            },
            ItemSpot {
                kind: ItemKind::Health,
                origin: Vec3::new(600.0, 0.0, 0.0),
            },
        ];
        let items = Items::new(&spots, SimTime(0.0));
        let calm = Beliefs::default();
        let need = |_| 1.0;
        let mut rng = Pcg32::new(8, 8);
        let mut s = situation(1.0, &calm, Some(&items), &KIT, 100.0, None, &need);
        s.gungame = gungame(300, &[WeaponId::Glock]);
        assert_eq!(Decider::default().decide(&s, &mut rng).kind, GoalKind::Roam);
        s.health = 30.0;
        assert_eq!(Decider::default().decide(&s, &mut rng).kind, GoalKind::CollectItem(2));
    }

    #[test]
    fn on_the_tripmine_level_enemies_are_got_away_from_not_fought() {
        let mut b = Beliefs::default();
        let p = BeliefParams {
            track_forget: 8.0,
            maxspeed: 300.0,
        };
        b.on_sighting(&sighting(0.0, Vec3::new(400.0, 0.0, 0.0)));
        b.update(SimTime(0.0), &p);
        let none = |_| 0.0;
        let key = PlayerKey { slot: 3, userid: 30 };
        let mut rng = Pcg32::new(9, 9);
        let mut s = situation(0.0, &b, None, &KIT, 100.0, Some(key), &none);
        s.gungame = gungame(900, &[WeaponId::Tripmine, WeaponId::Glock]);
        assert_eq!(Decider::default().decide(&s, &mut rng).kind, GoalKind::Retreat);
        s.gungame = gungame(900, &[WeaponId::Rpg]);
        assert_eq!(Decider::default().decide(&s, &mut rng).kind, GoalKind::Engage(key));
    }

    #[test]
    fn with_the_crowbar_alone_a_lost_enemy_is_hunted_further() {
        let mut b = Beliefs::default();
        let p = BeliefParams {
            track_forget: 8.0,
            maxspeed: 300.0,
        };
        b.on_sighting(&sighting(0.0, Vec3::new(2500.0, 0.0, 0.0)));
        b.update(SimTime(3.0), &p);
        let none = |_| 0.0;
        let key = PlayerKey { slot: 3, userid: 30 };
        let mut rng = Pcg32::new(10, 10);
        let mut s = situation(3.0, &b, None, &KIT, 100.0, None, &none);
        s.aggression = 0.1;
        let plain = Decider::default().decide(&s, &mut rng).kind;
        assert_ne!(plain, GoalKind::Hunt(key), "a timid bot lets a far enemy go");
        s.gungame = gungame(1100, &[WeaponId::Crowbar]);
        assert_eq!(Decider::default().decide(&s, &mut rng).kind, GoalKind::Hunt(key));
    }

    #[test]
    fn goals_are_held_and_failures_cool_down() {
        let spots = [
            ItemSpot {
                kind: ItemKind::Weapon(WeaponId::Shotgun),
                origin: Vec3::new(400.0, 0.0, 0.0),
            },
            ItemSpot {
                kind: ItemKind::Weapon(WeaponId::Mp5),
                origin: Vec3::new(-420.0, 0.0, 0.0),
            },
        ];
        let items = Items::new(&spots, SimTime(0.0));
        let calm = Beliefs::default();
        let none = |_| 0.0;
        let s = situation(1.0, &calm, Some(&items), &KIT, 100.0, None, &none);
        let mut d = Decider::default();
        let mut rng = Pcg32::new(5, 5);
        let first = d.decide(&s, &mut rng).kind;
        for i in 0..20 {
            let s = situation(1.0 + f64::from(i) * 0.2, &calm, Some(&items), &KIT, 100.0, None, &none);
            assert_eq!(d.decide(&s, &mut rng).kind, first, "similar goals do not flip");
        }
        d.fail(SimTime(5.0), &mut rng);
        let s = situation(5.2, &calm, Some(&items), &KIT, 100.0, None, &none);
        let next = d.decide(&s, &mut rng).kind;
        assert_ne!(next, first, "the failed goal cools down");
    }

    #[test]
    fn a_shot_heard_is_gone_to_but_not_one_from_a_tracked_enemy() {
        let mut b = Beliefs::default();
        let shot = SoundStimulus {
            t: SimTime(10.0),
            kind: SoundKind::Shot,
            weapon: Some(WeaponId::Mp5),
            pos: Vec3::new(900.0, 0.0, 0.0),
            bearing: 0.0,
            bearing_sigma: 20.0,
            range: 900.0,
            gain: 0.3,
        };
        b.on_sound(&shot);
        let none = |_| 0.0;
        let mut d = Decider::default();
        let mut rng = Pcg32::new(7, 7);
        let g = d.decide(&situation(10.5, &b, None, &KIT, 100.0, None, &none), &mut rng);
        assert!(matches!(g.kind, GoalKind::Investigate(_)), "{g:?}");
        let g = d.decide(&situation(15.5, &b, None, &KIT, 100.0, None, &none), &mut rng);
        assert_eq!(g.kind, GoalKind::Roam, "old news after five seconds");
    }

    /// A map with one overwatch spot 600 units away and nothing else.
    struct OneSpot([lb_nav_api::CampSpot; 1]);

    impl MapView for OneSpot {
        fn node_count(&self) -> usize {
            1
        }
        fn node_origin(&self, _n: lb_nav_api::NodeId) -> Vec3 {
            self.0[0].pos
        }
        fn nearest_node(&self, _p: Vec3, _max: f32) -> Option<lb_nav_api::NodeId> {
            Some(0)
        }
        fn for_each_link(&self, _n: lb_nav_api::NodeId, _f: &mut dyn FnMut(lb_nav_api::NodeId, f32)) {}
        fn visible(&self, _a: lb_nav_api::NodeId, _b: lb_nav_api::NodeId) -> bool {
            false
        }
        fn for_each_visible(&self, _n: lb_nav_api::NodeId, _f: &mut dyn FnMut(lb_nav_api::NodeId)) {}
        fn flow(&self, _n: lb_nav_api::NodeId) -> f32 {
            0.0
        }
        fn exposure(&self, _n: lb_nav_api::NodeId) -> f32 {
            0.0
        }
        fn transit(&self, _n: lb_nav_api::NodeId) -> bool {
            false
        }
        fn danger(&self, _n: lb_nav_api::NodeId) -> f32 {
            0.0
        }
        fn danger_from(&self, _n: lb_nav_api::NodeId) -> Option<lb_nav_api::NodeId> {
            None
        }
        fn camp_spots(&self) -> &[lb_nav_api::CampSpot] {
            &self.0
        }
        fn mine_spots(&self) -> &[lb_nav_api::MineSpot] {
            &[]
        }
        fn chokepoints(&self) -> &[lb_nav_api::NodeId] {
            &[]
        }
    }

    #[test]
    fn a_calm_sniper_with_a_crossbow_holds_a_spot() {
        let map = OneSpot([lb_nav_api::CampSpot {
            node: 0,
            pos: Vec3::new(600.0, 0.0, 0.0),
            kind: CampKind::Overwatch,
            watch: [0.0, 90.0],
            pitch: 0.0,
            range: 1200.0,
            score: 1.0,
            guards: None,
        }]);
        let kit = [
            KIT[0],
            KIT[1],
            Armed {
                id: WeaponId::Crossbow,
                clip: Some(5),
                reserve: Some(10),
                reserve2: None,
            },
        ];
        let calm = Beliefs::default();
        let none = |_| 0.0;
        let mut rng = Pcg32::new(11, 11);
        let mut s = situation(30.0, &calm, None, &kit, 100.0, None, &none);
        s.map = Some(&map);
        s.aggression = 0.3;
        s.affinity.camp = 2.0;
        let mut d = Decider::default();
        assert_eq!(
            d.decide(&s, &mut rng).kind,
            GoalKind::Camp(0),
            "full health, no armor: fit to hold it"
        );
        s.calm_for = 1.0;
        let mut d = Decider::default();
        assert_eq!(d.decide(&s, &mut rng).kind, GoalKind::Roam, "not after a fight");
        s.calm_for = f64::INFINITY;
        let glock_only = [KIT[0], KIT[1]];
        s.weapons = &glock_only;
        let mut d = Decider::default();
        assert_eq!(d.decide(&s, &mut rng).kind, GoalKind::Roam, "no gun for the spot");
    }

    #[test]
    fn an_item_about_to_come_back_is_waited_for() {
        let spots = [ItemSpot {
            kind: ItemKind::Battery,
            origin: Vec3::new(600.0, 0.0, 0.0),
        }];
        let mut items = Items::new(&spots, SimTime(0.0));
        items.observe(0, true, SimTime(10.0));
        items.observe(0, false, SimTime(10.5));
        let calm = Beliefs::default();
        let none = |_| 0.0;
        let mut rng = Pcg32::new(9, 9);
        let mut s = situation(30.0, &calm, Some(&items), &KIT, 100.0, None, &none);
        s.affinity.control = 2.0;
        let mut d = Decider::default();
        assert_eq!(
            d.decide(&s, &mut rng).kind,
            GoalKind::ControlItem(0),
            "back at 40: worth the wait"
        );
        s.now = SimTime(12.0);
        let mut d = Decider::default();
        assert_eq!(d.decide(&s, &mut rng).kind, GoalKind::Roam, "28 s is too long to wait");
    }

    #[test]
    fn satchels_go_where_an_enemy_is_expected() {
        let map = OneSpot([lb_nav_api::CampSpot {
            node: 0,
            pos: Vec3::new(600.0, 0.0, 0.0),
            kind: CampKind::Overwatch,
            watch: [0.0, 90.0],
            pitch: 0.0,
            range: 400.0,
            score: 0.0,
            guards: None,
        }]);
        let kit = [
            KIT[0],
            KIT[1],
            Armed {
                id: WeaponId::Satchel,
                clip: None,
                reserve: Some(1),
                reserve2: None,
            },
        ];
        let calm = Beliefs::default();
        let none = |_| 0.0;
        let mut rng = Pcg32::new(13, 13);
        let mut s = situation(30.0, &calm, None, &kit, 100.0, None, &none);
        s.map = Some(&map);
        s.lure = Some(Vec3::new(500.0, 0.0, 0.0));
        let lure = GoalKind::PlantTrap(Trap::Loose);
        assert_eq!(Decider::default().decide(&s, &mut rng).kind, lure);
        s.charges_out = true;
        assert_ne!(
            Decider::default().decide(&s, &mut rng).kind,
            lure,
            "its satchels out already"
        );
        s.charges_out = false;
        s.lure = None;
        assert_ne!(
            Decider::default().decide(&s, &mut rng).kind,
            lure,
            "nowhere an enemy is expected"
        );
    }

    #[test]
    fn a_trap_under_way_goes_on_until_a_higher_rank_takes_over() {
        let map = OneSpot([lb_nav_api::CampSpot {
            node: 0,
            pos: Vec3::new(600.0, 0.0, 0.0),
            kind: CampKind::Ambush,
            watch: [0.0, 90.0],
            pitch: 0.0,
            range: 400.0,
            score: 1.0,
            guards: Some(0),
        }]);
        let kit = [
            KIT[0],
            KIT[1],
            Armed {
                id: WeaponId::Satchel,
                clip: None,
                reserve: Some(3),
                reserve2: None,
            },
        ];
        let calm = Beliefs::default();
        let none = |_| 0.0;
        let mut rng = Pcg32::new(13, 13);
        let mut s = situation(30.0, &calm, None, &kit, 100.0, None, &none);
        s.map = Some(&map);
        s.affinity.trap = 2.0;
        let mut d = Decider::default();
        let trap = GoalKind::PlantTrap(Trap::Satchels(0));
        assert_eq!(d.decide(&s, &mut rng).kind, trap);
        // Thrown and watched past its hold: the satchels out, the rest begun, a shot heard.
        let mut heard = Beliefs::default();
        heard.on_sound(&SoundStimulus {
            t: SimTime(59.5),
            kind: SoundKind::Shot,
            weapon: Some(WeaponId::Mp5),
            pos: Vec3::new(900.0, 0.0, 0.0),
            bearing: 0.0,
            bearing_sigma: 20.0,
            range: 900.0,
            gain: 0.3,
        });
        let mut s = situation(60.0, &heard, None, &KIT, 100.0, None, &none);
        s.map = Some(&map);
        s.calm_for = 0.5;
        s.trap_ready = false;
        s.charges_out = true;
        s.trap_under_way = true;
        assert_eq!(d.decide(&s, &mut rng).kind, trap, "{:?}", d.last);
        assert!(d.current.is_some_and(|c| c.hold_until < s.now));
        assert!(d.last.iter().any(|g| matches!(g.kind, GoalKind::Investigate(_))));
        let key = PlayerKey { slot: 3, userid: 30 };
        let mut seen = Beliefs::default();
        seen.on_sighting(&sighting(60.2, Vec3::new(800.0, 0.0, 0.0)));
        let p = BeliefParams {
            track_forget: 8.0,
            maxspeed: 300.0,
        };
        seen.update(SimTime(60.2), &p);
        let mut s = situation(60.2, &seen, None, &KIT, 100.0, Some(key), &none);
        s.map = Some(&map);
        s.trap_ready = false;
        s.charges_out = true;
        s.trap_under_way = true;
        assert_eq!(d.decide(&s, &mut rng).kind, GoalKind::Engage(key));
    }
}
