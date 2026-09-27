//! Dual utility goal selection, commitment, team board, BehaviorModule interface.
//!
//! Every goal candidate has a rank and a weight (design §4). Candidates under their threshold drop out, the
//! highest rank present wins, and among candidates within 90% of the best weight one is drawn at random. A goal
//! is held for a while once chosen: a higher rank takes over at once, the same rank only with a clear margin.

#![forbid(unsafe_code)]

use lb_combat::Armed;
use lb_core::Vec3;
use lb_core::dmath;
use lb_core::rng::Pcg32;
use lb_core::time::SimTime;
use lb_game::items::{Ammo, ItemKind};
use lb_game::mechanics::{WeaponClass, spec};
use lb_game::weapons::WeaponId;
use lb_knowledge::{Beliefs, Items, PlayerKey, Relation, TrackState};
use lb_styles::GoalAffinity;
use smallvec::SmallVec;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GoalKind {
    Engage(PlayerKey),
    Hunt(PlayerKey),
    Retreat,
    CollectItem(usize),
    Roam,
}

impl GoalKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            GoalKind::Engage(_) => "engage",
            GoalKind::Hunt(_) => "hunt",
            GoalKind::Retreat => "retreat",
            GoalKind::CollectItem(_) => "collect",
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
    pub weapons: &'a [Armed],
    /// How much each ammo type is needed, 0..1 (0 when no owned weapon uses it).
    pub ammo_need: &'a dyn Fn(Ammo) -> f32,
    pub reloading: bool,
    /// The enemy combat is aiming at.
    pub target: Option<PlayerKey>,
    /// Other players on the server (public scoreboard).
    pub opponents: usize,
    pub maxspeed: f32,
}

impl Situation<'_> {
    /// Health for the goal formulas, 0..100: health plus armor as it absorbs bullets, a third of the sum.
    pub fn effective_health(&self) -> f32 {
        ((self.health + 2.0 * self.armor) / 3.0).clamp(0.0, 100.0)
    }

    fn eta(&self, to: Vec3) -> f32 {
        self.origin.distance(to) * 1.4 / self.maxspeed.max(100.0)
    }
}

const ENGAGE_HOLD: f32 = 1.0;
const HUNT_HOLD: f32 = 3.0;
const RETREAT_HOLD: f32 = 2.0;
const ROAM_HOLD: f32 = 5.0;
const RETREAT_THRESHOLD: f32 = 0.4;
const HUNT_THRESHOLD: f32 = 0.6;
const COLLECT_THRESHOLD: f32 = 0.1;

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
    match kind {
        ItemKind::Health if s.health < 85.0 => 0.8 * (1.0 - s.health / 100.0),
        ItemKind::Battery if s.armor < 90.0 => 0.6 * (1.0 - s.armor / 100.0),
        ItemKind::LongJump if !s.has_longjump => 0.8,
        ItemKind::Weapon(w) if !owns(w) => {
            let rank = f32::from(spec(w).rank);
            if spec(w).class == WeaponClass::Throwable {
                0.3
            } else {
                0.4 + 0.03 * rank
            }
        }
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

/// All goal candidates in this situation.
pub fn candidates(s: &Situation<'_>, out: &mut Vec<Goal>) {
    out.clear();
    let aggr = s.aggression.clamp(0.0, 1.0);
    let retreat = retreat_weight(s);
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
    if let Some(t) = engage {
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
        let confidence = (1.0 - t.sigma / 1500.0).clamp(0.0, 1.0);
        let w = (((4096.0 - (1.0 - aggr) * d) / 4096.0 - retreat).min(0.89)) * confidence * s.affinity.hunt;
        if w >= HUNT_THRESHOLD {
            out.push(Goal {
                kind: GoalKind::Hunt(t.who),
                rank: 1,
                weight: w,
                hold: HUNT_HOLD,
            });
        }
    }
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

#[derive(Clone, Debug, Default)]
pub struct Decider {
    pub current: Option<Active>,
    cooldowns: SmallVec<[(GoalKind, SimTime); 8]>,
    /// Candidates of the last decision, best first (for traces and `lb` commands).
    pub last: Vec<Goal>,
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

    pub fn decide(&mut self, s: &Situation<'_>, rng: &mut Pcg32) -> Goal {
        let now = s.now;
        self.cooldowns.retain(|(_, until)| *until > now);
        let mut all = std::mem::take(&mut self.scratch);
        candidates(s, &mut all);
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
            weapons,
            ammo_need: need,
            reloading: false,
            target,
            opponents: 4,
            maxspeed: 300.0,
        }
    }

    const KIT: [Armed; 2] = [
        Armed {
            id: WeaponId::Crowbar,
            clip: None,
            reserve: None,
        },
        Armed {
            id: WeaponId::Glock,
            clip: Some(17),
            reserve: Some(68),
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
        assert_eq!(g.kind, GoalKind::Roam, "too uncertain by now");
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
}
