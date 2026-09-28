//! What the bot does for a goal beyond walking somewhere:
//! - **hunt:** a lost enemy is looked for from the place that sees most of where it may be now, chosen again every
//!   1.5 s, and half a second after getting there;
//! - **investigate:** a sound is gone to see about, to a place in sight of where it came from, and looked at;
//! - **retreat:** out of the threat's sight to a place the bot gets to first, where it holds and watches the way the
//!   threat would come;
//! - **camp:** a spot held for a while, watching one way and the other (crouched at an ambush);
//! - **control:** an item about to come back is waited for close by, out of the way, and taken when it is back;
//! - **trap:** a tripmine put on a wall across a busy way, or satchels thrown at a chokepoint from close by and
//!   watched from an ambush spot out of their blast until someone comes by them.

use lb_core::Vec3;
use lb_core::math::view_angle_vectors;
use lb_core::rng::BotRng;
use lb_core::time::SimTime;
use lb_decision::{GoalKind, Trap};
use lb_knowledge::{ItemState, PlayerKey, Spread, TrackState};
use lb_motor::{LookIntent, MoveIntent, Prio, StanceIntent};
use lb_nav_api::{CampKind, MapView, NavService, NavStatus, NodeId};

use crate::BotBrain;
use crate::mind::{Body, Character, apply_step};

/// A search place is chosen again this often, and this soon once the bot is there.
const SEARCH_EVERY: f64 = 1.5;
const SEARCH_LOOK: f64 = 0.5;
/// Places a lost enemy is most likely at, looked for.
const SEARCH_PLACES: usize = 12;
/// On arriving where a sound came from, the bot looks this long.
const LOOK_AROUND: f64 = 0.8;
/// A place to see a sound's place from is at most this far from it.
const VANTAGE_REACH: f32 = 1200.0;
/// A sound seen about is not gone to again (it is old news by then).
const INVESTIGATED_REST: f64 = 10.0;
const COVER_REPLAN: f64 = 2.0;
/// A threat that moved this far since cover was chosen makes the bot look again.
const COVER_MOVED: f32 = 400.0;
/// At a spot within this, across.
const AT_SPOT: f32 = 32.0;
const CAMP_FOR: [f32; 2] = [6.0, 10.0];
const AMBUSH_FOR: [f32; 2] = [8.0, 14.0];
/// A style this fond of a kind of spot holds it half as long again, and rests less between spots.
const FOND: f32 = 1.5;
const CAMP_LOOK: [f32; 2] = [1.5, 4.0];
const CAMP_REST: [f32; 2] = [50.0, 70.0];
const FOND_CAMP_REST: [f32; 2] = [15.0, 25.0];
const TRAP_REST: [f32; 2] = [20.0, 30.0];
const FOND_TRAP_REST: [f32; 2] = [10.0, 18.0];
/// After a trap given up.
const TRAP_GIVE_UP_REST: f64 = 8.0;
/// Satchels thrown as a trap are watched this long, with the radio up (renewed while at the spot).
const SATCHEL_GUARD: [f32; 2] = [12.0, 20.0];
const RADIO_WATCH: f64 = 0.3;
/// The throw of a trap's satchels must show within this.
const TRAP_THROW_FOR: f64 = 3.0;
/// Satchels are thrown at a chokepoint from this close (a satchel flies some 200 units).
const SATCHEL_THROW: f32 = 160.0;
/// An item is waited for this close to it, out of the way.
const CONTROL_NEAR: f32 = 250.0;
/// An item not back this long after its window is given up.
const CONTROL_LATE: f64 = 3.0;
/// An item just taken is not waited for again this soon (it is gone for its respawn time anyway).
const CONTROL_REST: f64 = 5.0;

/// What the bot is doing for its goal: where it chose to go and what it does there.
#[derive(Clone, Debug)]
pub enum Task {
    /// Looking for a lost enemy from `dest`, chosen at `at`.
    Search { who: PlayerKey, dest: Vec3, at: SimTime },
    /// Going to see about a sound: from where, what to look at, since when it has been there.
    Investigate {
        id: u32,
        dest: Vec3,
        look: Vec3,
        arrived: Option<SimTime>,
    },
    /// Out of sight of a threat, or on the way there.
    Hide {
        dest: Vec3,
        threat: Vec3,
        at: SimTime,
        arrived: bool,
    },
    /// Holding a spot until `until` (set on arrival), looking one way or the other.
    Camp {
        spot: u16,
        until: Option<SimTime>,
        look: usize,
        next_look: SimTime,
    },
    /// Waiting for an item close by; whether the bot got to where it waits.
    Control { item: usize, wait_at: Vec3, waited: bool },
    /// Laying a trap: the mines laid before it started, whether the mine or the throw is under way, when the
    /// satchels thrown are watched until.
    Trap {
        trap: Trap,
        mines_before: u32,
        started: Option<SimTime>,
        until: Option<SimTime>,
    },
}

fn at_spot(body: &Body, p: Vec3) -> bool {
    (body.origin - p).truncate().length() <= AT_SPOT && (body.origin.z - p.z).abs() <= 48.0
}

/// Seconds of running the places are looked for within, around the bot.
const REACH: f32 = 12.0;

/// Seconds of running to `n` from where the bot is (by the graph: walls are walked round), a straight line's worth
/// when the bot is off the graph.
fn time_to(from: &[f32], map: &dyn MapView, n: NodeId, me: Vec3) -> f32 {
    from.get(n as usize)
        .copied()
        .filter(|t| t.is_finite())
        .unwrap_or_else(|| map.node_origin(n).distance(me) / RUN)
}

/// Running speed the graph's travel times assume.
const RUN: f32 = 300.0;

/// The place that sees most of where a lost enemy may be, less a little for how far it is to run there.
fn search_spot(map: &dyn MapView, spread: &Spread, me: Vec3) -> Option<Vec3> {
    let top = spread.likely(SEARCH_PLACES);
    let from = map
        .nearest_node(me, 256.0)
        .map(|n| lb_knowledge::travel(map, n, REACH))
        .unwrap_or_default();
    let mut best: Option<(NodeId, f32)> = None;
    let mut consider = |c: NodeId| {
        if map.transit(c) {
            return;
        }
        let cover: f32 = top
            .iter()
            .filter(|&&(m, _)| m == c || map.visible(c, m))
            .map(|&(_, p)| p)
            .sum();
        let score = cover - time_to(&from, map, c, me) * RUN / 3000.0;
        if best.is_none_or(|b| score > b.1 || (score == b.1 && c < b.0)) {
            best = Some((c, score));
        }
    };
    for &(n, _) in &top {
        consider(n);
        map.for_each_link(n, &mut |m, _| consider(m));
    }
    best.map(|(n, _)| map.node_origin(n))
}

/// A place in sight of `pos` (a sound's place) the bot gets to soon: a short run away and not too far from the sound.
fn vantage(map: &dyn MapView, pos: Vec3, me: Vec3) -> Option<Vec3> {
    let hn = map.nearest_node(pos, 400.0)?;
    let from = map
        .nearest_node(me, 256.0)
        .map(|n| lb_knowledge::travel(map, n, REACH))
        .unwrap_or_default();
    let mut best = (hn, time_to(&from, map, hn, me) * RUN);
    map.for_each_visible(hn, &mut |c| {
        let o = map.node_origin(c);
        if map.transit(c) || o.distance(pos) > VANTAGE_REACH {
            return;
        }
        let cost = time_to(&from, map, c, me) * RUN + 0.5 * o.distance(pos);
        if cost < best.1 {
            best = (c, cost);
        }
    });
    Some(map.node_origin(best.0))
}

impl BotBrain {
    fn stand_still(&mut self) {
        self.intents.movement(
            Prio::Goal,
            MoveIntent {
                dir: lb_core::Vec2::ZERO,
                speed: 0.0,
            },
        );
    }

    fn look_at(&mut self, at: Vec3) {
        self.intents.look(Prio::Goal, LookIntent::Point { at, engaged: false });
    }

    fn walk(&mut self, dest: Vec3, body: &Body, nav: &mut dyn NavService) -> NavStatus {
        let (status, step) = nav.go_to(dest);
        if let Some(step) = step {
            apply_step(&mut self.intents, &step, body.eye, &mut self.mind);
        }
        status
    }

    fn give_up(&mut self, now: SimTime, rng: &mut BotRng) {
        self.mind.decider.fail(now, &mut rng.decision);
        self.mind.urgent = true;
        self.mind.task = None;
    }

    fn done(&mut self) {
        self.mind.decider.complete();
        self.mind.urgent = true;
        self.mind.task = None;
    }

    pub(crate) fn hunt(
        &mut self,
        k: PlayerKey,
        body: &Body,
        map: Option<&dyn MapView>,
        nav: &mut dyn NavService,
        rng: &mut BotRng,
    ) {
        let now = body.now;
        let Some(t) = self.beliefs.track(k) else {
            self.done();
            return;
        };
        let searching = t.state != TrackState::RecentlyLost;
        let dest = match (map, t.spread.as_deref()) {
            (Some(map), Some(spread)) if searching => match self.mind.task {
                Some(Task::Search { who, dest, at })
                    if who == k && now.since(at) < if at_spot(body, dest) { SEARCH_LOOK } else { SEARCH_EVERY } =>
                {
                    dest
                }
                _ => {
                    let dest = search_spot(map, spread, body.origin).unwrap_or(t.pos);
                    self.mind.task = Some(Task::Search { who: k, dest, at: now });
                    dest
                }
            },
            _ => t.pos,
        };
        // At the place to search from: look about there until the next choice.
        if searching && at_spot(body, dest) {
            self.stand_still();
            return;
        }
        match self.walk(dest, body, nav) {
            NavStatus::Arrived if searching => self.stand_still(),
            NavStatus::Arrived => self.done(),
            NavStatus::NoPath => self.give_up(now, rng),
            NavStatus::Moving => {}
        }
    }

    pub(crate) fn investigate(
        &mut self,
        id: u32,
        body: &Body,
        map: Option<&dyn MapView>,
        nav: &mut dyn NavService,
        rng: &mut BotRng,
    ) {
        let now = body.now;
        let Some(pos) = self.beliefs.hypothesis(id).and_then(|h| h.pos) else {
            self.done();
            return;
        };
        if !matches!(self.mind.task, Some(Task::Investigate { id: i, .. }) if i == id) {
            let dest = map.and_then(|m| vantage(m, pos, body.origin)).unwrap_or(pos);
            self.mind.task = Some(Task::Investigate {
                id,
                dest,
                look: pos,
                arrived: None,
            });
        }
        let Some(Task::Investigate {
            dest, look, arrived, ..
        }) = self.mind.task.clone()
        else {
            return;
        };
        if let Some(at) = arrived {
            if now.since(at) >= LOOK_AROUND {
                self.mind.stats.investigated += 1;
                self.mind
                    .decider
                    .rest(GoalKind::Investigate(id), now + INVESTIGATED_REST);
                self.done();
                return;
            }
            self.stand_still();
            self.look_at(look + Vec3::Z * 8.0);
            return;
        }
        let status = if at_spot(body, dest) {
            NavStatus::Arrived
        } else {
            self.walk(dest, body, nav)
        };
        match status {
            NavStatus::Arrived => {
                if let Some(Task::Investigate { arrived, .. }) = self.mind.task.as_mut() {
                    *arrived = Some(now);
                }
            }
            NavStatus::NoPath => self.give_up(now, rng),
            NavStatus::Moving => {}
        }
    }

    pub(crate) fn retreat(&mut self, body: &Body, nav: &mut dyn NavService, rng: &mut BotRng) {
        let now = body.now;
        let threat = self
            .beliefs
            .enemies()
            .filter(|t| t.state != TrackState::Stale)
            .min_by(|a, b| a.pos.distance(body.origin).total_cmp(&b.pos.distance(body.origin)))
            .map(|t| t.pos)
            .or_else(|| {
                let bearing = self.beliefs.last_damage?.bearing?;
                let (s, c) = lb_core::dmath::sin_cos(bearing.to_radians());
                Some(body.origin + Vec3::new(c, s, 0.0) * 500.0)
            });
        let Some(threat) = threat else {
            self.give_up(now, rng);
            return;
        };
        let replan = match &self.mind.task {
            Some(Task::Hide {
                threat: then,
                at,
                arrived,
                ..
            }) => {
                if *arrived {
                    then.distance(threat) > COVER_MOVED
                } else {
                    now.since(*at) > COVER_REPLAN
                }
            }
            _ => true,
        };
        if replan {
            let dest = nav.cover_from(threat);
            if dest.is_some() {
                self.mind.stats.covers += 1;
            }
            match dest.or_else(|| nav.away_from(threat)) {
                Some(dest) => {
                    self.mind.task = Some(Task::Hide {
                        dest,
                        threat,
                        at: now,
                        arrived: false,
                    })
                }
                None => {
                    self.give_up(now, rng);
                    return;
                }
            }
        }
        let Some(Task::Hide { dest, arrived, .. }) = self.mind.task.clone() else {
            return;
        };
        if arrived || at_spot(body, dest) {
            if let Some(Task::Hide { arrived, .. }) = self.mind.task.as_mut() {
                *arrived = true;
            }
            self.stand_still();
            // Watch the way the threat would come.
            let watch = self.expect.map_or(threat, |(_, p)| p);
            self.look_at(watch + Vec3::Z * 8.0);
            return;
        }
        match self.walk(dest, body, nav) {
            NavStatus::Arrived => {
                if let Some(Task::Hide { arrived, .. }) = self.mind.task.as_mut() {
                    *arrived = true;
                }
            }
            NavStatus::NoPath => self.give_up(now, rng),
            NavStatus::Moving => {}
        }
    }

    pub(crate) fn camp(
        &mut self,
        i: u16,
        body: &Body,
        ch: &Character,
        map: &dyn MapView,
        nav: &mut dyn NavService,
        rng: &mut BotRng,
    ) {
        let now = body.now;
        let Some(spot) = map.camp_spots().get(i as usize).copied() else {
            self.done();
            return;
        };
        if !matches!(self.mind.task, Some(Task::Camp { spot: s, .. }) if s == i) {
            self.mind.task = Some(Task::Camp {
                spot: i,
                until: None,
                look: 0,
                next_look: now,
            });
        }
        let Some(Task::Camp { until, .. }) = self.mind.task.clone() else {
            return;
        };
        self.mind.calm_distance = Some(spot.range);
        let until = match until {
            Some(u) => u,
            None => {
                let status = if at_spot(body, spot.pos) {
                    NavStatus::Arrived
                } else {
                    self.walk(spot.pos, body, nav)
                };
                match status {
                    NavStatus::Arrived => {
                        let (range, fond) = match spot.kind {
                            CampKind::Overwatch => (CAMP_FOR, ch.affinity.camp >= FOND),
                            CampKind::Ambush => (AMBUSH_FOR, ch.affinity.ambush >= FOND),
                        };
                        let secs = rng.decision.range_f32(range[0], range[1]) * if fond { 1.5 } else { 1.0 };
                        let u = now + f64::from(secs);
                        if let Some(Task::Camp { until, .. }) = self.mind.task.as_mut() {
                            *until = Some(u);
                        }
                        self.mind.stats.camps += 1;
                        u
                    }
                    NavStatus::NoPath => {
                        self.give_up(now, rng);
                        return;
                    }
                    NavStatus::Moving => return,
                }
            }
        };
        if now >= until {
            self.left_goal(GoalKind::Camp(i), ch, now, rng);
            self.done();
            return;
        }
        self.stand_still();
        if spot.kind == CampKind::Ambush {
            self.intents.stance(
                Prio::Goal,
                StanceIntent {
                    jump: false,
                    duck: true,
                },
            );
        }
        let Some(Task::Camp { look, next_look, .. }) = self.mind.task.as_mut() else {
            return;
        };
        if now >= *next_look {
            *look = (*look + 1) % 2;
            *next_look = now + f64::from(rng.decision.range_f32(CAMP_LOOK[0], CAMP_LOOK[1]));
        }
        let yaw = spot.watch[*look];
        // Where a lost enemy would come into view beats the spot's own directions.
        let at = match self.expect {
            Some((_, p)) => p,
            None => {
                let (forward, _, _) = view_angle_vectors(Vec3::new(spot.pitch, yaw, 0.0));
                body.eye + forward * spot.range.clamp(200.0, 2000.0)
            }
        };
        self.look_at(at);
    }

    pub(crate) fn control(
        &mut self,
        i: usize,
        body: &Body,
        map: Option<&dyn MapView>,
        nav: &mut dyn NavService,
        rng: &mut BotRng,
    ) {
        let now = body.now;
        let Some((spot, state)) = self
            .items
            .as_ref()
            .and_then(|items| Some((*items.spots.get(i)?, items.beliefs.get(i)?.state)))
        else {
            self.done();
            return;
        };
        match state {
            ItemState::Present => {
                self.item_focus = None;
                // Came back while the bot waited by it.
                if let Some(Task::Control { waited, .. }) = self.mind.task.as_mut()
                    && std::mem::take(waited)
                {
                    self.mind.stats.controlled += 1;
                }
                match self.walk(spot.origin, body, nav) {
                    NavStatus::Arrived => {
                        self.mind.decider.rest(GoalKind::ControlItem(i), now + CONTROL_REST);
                        self.done();
                    }
                    NavStatus::NoPath => self.give_up(now, rng),
                    NavStatus::Moving => {}
                }
            }
            ItemState::Absent { back: (_, to) } => {
                if now > to + CONTROL_LATE {
                    self.item_focus = None;
                    self.done();
                    return;
                }
                self.item_focus = Some(i);
                if !matches!(self.mind.task, Some(Task::Control { item, .. }) if item == i) {
                    let wait_at = map.and_then(|m| wait_spot(m, spot.origin)).unwrap_or(spot.origin);
                    self.mind.task = Some(Task::Control {
                        item: i,
                        wait_at,
                        waited: false,
                    });
                }
                let Some(Task::Control { wait_at, .. }) = self.mind.task.clone() else {
                    return;
                };
                if at_spot(body, wait_at) {
                    if let Some(Task::Control { waited, .. }) = self.mind.task.as_mut() {
                        *waited = true;
                    }
                    self.stand_still();
                    self.look_at(spot.origin + Vec3::Z * 8.0);
                    return;
                }
                if self.walk(wait_at, body, nav) == NavStatus::NoPath {
                    self.give_up(now, rng);
                }
            }
        }
    }

    pub(crate) fn trap(
        &mut self,
        trap: Trap,
        body: &Body,
        ch: &Character,
        map: &dyn MapView,
        nav: &mut dyn NavService,
        rng: &mut BotRng,
    ) {
        let now = body.now;
        if !matches!(&self.mind.task, Some(Task::Trap { trap: t, .. }) if *t == trap) {
            self.mind.task = Some(Task::Trap {
                trap,
                mines_before: self.mind.arms.stats.mines,
                started: None,
                until: None,
            });
        }
        let Some(Task::Trap {
            mines_before,
            started,
            until,
            ..
        }) = self.mind.task.clone()
        else {
            return;
        };
        let fond = ch.affinity.trap >= FOND;
        let rest = if fond { FOND_TRAP_REST } else { TRAP_REST };
        match trap {
            Trap::Mine(i) => {
                let Some(spot) = map.mine_spots().get(i as usize).copied() else {
                    self.done();
                    return;
                };
                if self.mind.arms.stats.mines > mines_before {
                    self.mind.stats.traps += 1;
                    self.mind.trap_rest_until = now + f64::from(rng.decision.range_f32(rest[0], rest[1]));
                    self.done();
                    return;
                }
                if started.is_some() {
                    if self.mind.arms.active.is_none() {
                        self.mind.trap_rest_until = now + TRAP_GIVE_UP_REST;
                        self.give_up(now, rng);
                    }
                    return;
                }
                if !at_spot(body, spot.stand) {
                    if self.walk(spot.stand, body, nav) == NavStatus::NoPath {
                        self.give_up(now, rng);
                    }
                    return;
                }
                self.stand_still();
                if self.plant_mine(spot.wall, spot.normal, now)
                    && let Some(Task::Trap { started, .. }) = self.mind.task.as_mut()
                {
                    *started = Some(now);
                }
            }
            Trap::Satchels(i) => {
                let Some((spot, choke)) = map
                    .camp_spots()
                    .get(i as usize)
                    .and_then(|c| Some((*c, map.node_origin(c.guards?))))
                else {
                    self.done();
                    return;
                };
                // Watching them from the spot, out of their blast.
                if let Some(u) = until {
                    if now >= u || self.explosives.charges.is_empty() {
                        self.done();
                        return;
                    }
                    if !at_spot(body, spot.pos) {
                        if self.walk(spot.pos, body, nav) == NavStatus::NoPath {
                            self.done();
                        }
                        return;
                    }
                    self.stand_still();
                    self.intents.stance(
                        Prio::Goal,
                        StanceIntent {
                            jump: false,
                            duck: true,
                        },
                    );
                    self.look_at(choke);
                    // The radio up while watching: an enemy by them is set off at once.
                    self.mind.arms.radio_until = self.mind.arms.radio_until.max(now + RADIO_WATCH);
                    return;
                }
                // Thrown: once the charges lie there, off to the spot.
                if let Some(at) = started {
                    if !self.explosives.charges.is_empty() && self.mind.arms.active.is_none() {
                        self.mind.stats.traps += 1;
                        let guard = now + f64::from(rng.decision.range_f32(SATCHEL_GUARD[0], SATCHEL_GUARD[1]));
                        self.mind.trap_rest_until = guard + f64::from(rng.decision.range_f32(rest[0], rest[1]));
                        if let Some(Task::Trap { until, .. }) = self.mind.task.as_mut() {
                            *until = Some(guard);
                        }
                    } else if now.since(at) > TRAP_THROW_FOR && self.mind.arms.active.is_none() {
                        self.mind.trap_rest_until = now + TRAP_GIVE_UP_REST;
                        self.give_up(now, rng);
                    }
                    return;
                }
                // A satchel flies some 200 units: up to the chokepoint within a throw of it, then throw.
                let near = (body.origin - choke).truncate().length() <= SATCHEL_THROW
                    && nav.trace(&lb_worldq::TraceQuery::line(body.eye, choke)).fraction >= 0.95;
                if !near {
                    if self.walk(choke, body, nav) == NavStatus::NoPath {
                        self.give_up(now, rng);
                    }
                    return;
                }
                self.stand_still();
                let floor = choke - Vec3::Z * 32.0;
                if self.trap_satchels(body, nav, floor, rng)
                    && let Some(Task::Trap { started, .. }) = self.mind.task.as_mut()
                {
                    *started = Some(now);
                }
            }
        }
    }

    /// When a goal is left: spots are not held again for a while, and a trap given up waits a little.
    pub(crate) fn left_goal(&mut self, old: GoalKind, ch: &Character, now: SimTime, rng: &mut BotRng) {
        match old {
            GoalKind::Camp(_) => {
                let fond = ch.affinity.camp.max(ch.affinity.ambush) >= FOND;
                let rest = if fond { FOND_CAMP_REST } else { CAMP_REST };
                self.mind.camp_rest_until = now + f64::from(rng.decision.range_f32(rest[0], rest[1]));
            }
            GoalKind::PlantTrap(_) => {
                self.mind.trap_rest_until = self.mind.trap_rest_until.max(now + TRAP_GIVE_UP_REST);
            }
            GoalKind::ControlItem(_) => self.item_focus = None,
            _ => {}
        }
        self.mind.task = None;
        self.mind.calm_distance = None;
    }
}

/// A place close to an item to wait at: in sight of it and seen by as little traffic as can be.
fn wait_spot(map: &dyn MapView, item: Vec3) -> Option<Vec3> {
    let n = map.nearest_node(item, 200.0)?;
    let mut best = (n, map.exposure(n) + 0.5);
    map.for_each_visible(n, &mut |c| {
        let o = map.node_origin(c);
        let d = o.distance(item);
        if map.transit(c) || !(64.0..=CONTROL_NEAR).contains(&d) {
            return;
        }
        let score = map.exposure(c) + 0.3 * d / CONTROL_NEAR;
        if score < best.1 {
            best = (c, score);
        }
    });
    Some(map.node_origin(best.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lb_nav_api::{CampSpot, MineSpot};

    /// Two corridors meeting at a corner: nodes 0..5 along x (x = 0..500), then 6..10 along y from x = 500
    /// (y = 100..500). Each corridor sees along itself; the corner node 5 sees both. Node 7 is busy.
    struct Corner;

    impl Corner {
        fn arm(n: NodeId) -> u8 {
            match n {
                0..=4 => 0,
                5 => 2,
                _ => 1,
            }
        }
    }

    impl MapView for Corner {
        fn node_count(&self) -> usize {
            11
        }
        fn node_origin(&self, n: NodeId) -> Vec3 {
            if n <= 5 {
                Vec3::new(n as f32 * 100.0, 0.0, 0.0)
            } else {
                Vec3::new(500.0, (n - 5) as f32 * 100.0, 0.0)
            }
        }
        fn nearest_node(&self, p: Vec3, _max: f32) -> Option<NodeId> {
            (0..11).min_by(|&a, &b| {
                self.node_origin(a)
                    .distance(p)
                    .total_cmp(&self.node_origin(b).distance(p))
            })
        }
        fn for_each_link(&self, n: NodeId, f: &mut dyn FnMut(NodeId, f32)) {
            let links: &[NodeId] = match n {
                0 => &[1],
                5 => &[4, 6],
                6 => &[5, 7],
                10 => &[9],
                _ => &[],
            };
            if links.is_empty() {
                f(n - 1, 0.33);
                f(n + 1, 0.33);
            }
            for &m in links {
                f(m, 0.33);
            }
        }
        fn visible(&self, a: NodeId, b: NodeId) -> bool {
            let (x, y) = (Corner::arm(a), Corner::arm(b));
            a != b && (x == y || x == 2 || y == 2)
        }
        fn for_each_visible(&self, n: NodeId, f: &mut dyn FnMut(NodeId)) {
            for m in 0..11 {
                if self.visible(n, m) {
                    f(m);
                }
            }
        }
        fn flow(&self, n: NodeId) -> f32 {
            if n == 7 { 1.0 } else { 0.2 }
        }
        fn exposure(&self, n: NodeId) -> f32 {
            if n == 7 { 1.0 } else { 0.1 }
        }
        fn transit(&self, _n: NodeId) -> bool {
            false
        }
        fn danger(&self, _n: NodeId) -> f32 {
            0.0
        }
        fn danger_from(&self, _n: NodeId) -> Option<NodeId> {
            None
        }
        fn camp_spots(&self) -> &[CampSpot] {
            &[]
        }
        fn mine_spots(&self) -> &[MineSpot] {
            &[]
        }
        fn chokepoints(&self) -> &[NodeId] {
            &[]
        }
    }

    #[test]
    fn a_lost_enemy_round_the_corner_is_looked_for_from_the_corner() {
        let map = Corner;
        // Lost at node 7 (up the second corridor) a second ago, running up it.
        let spread = Spread::new(
            &map,
            map.node_origin(7),
            Some(Vec3::new(0.0, 300.0, 0.0)),
            SimTime(0.0),
            8.0,
            SimTime(1.0),
            None,
        )
        .unwrap();
        // The bot is in the first corridor at node 1: the corner sees the whole second corridor.
        let spot = search_spot(&map, &spread, map.node_origin(1)).unwrap();
        assert_eq!(spot, map.node_origin(5));
    }

    #[test]
    fn a_sound_is_seen_about_from_the_nearest_place_in_sight_of_it() {
        let map = Corner;
        // A sound at node 9, the bot at node 0: node 5 (the corner) sees it and is on the way.
        let at = vantage(&map, map.node_origin(9), map.node_origin(0)).unwrap();
        assert_eq!(at, map.node_origin(5));
        // Already in sight of it: stays where it is.
        let at = vantage(&map, map.node_origin(3), map.node_origin(1)).unwrap();
        assert_eq!(at, map.node_origin(1));
    }

    #[test]
    fn an_item_is_waited_for_out_of_the_way() {
        let map = Corner;
        // An item at node 6: nodes in sight within 250 units, the busy node 7 left out.
        let at = wait_spot(&map, map.node_origin(6)).unwrap();
        assert_ne!(at, map.node_origin(7));
        assert!(at.distance(map.node_origin(6)) <= CONTROL_NEAR);
    }
}
