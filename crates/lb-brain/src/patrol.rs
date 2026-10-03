//! Patrol: a throwable is not fought with in place. A bot with one in hand that does not close in on its enemy (one
//! off its level, too little will, no way there), out of the way of its own throw and with the enemy no closer than
//! it keeps it off at, keeps moving round its own level rather than stand: on, far, to places it has not been
//! lately, throwing as it goes. On GunGame's octagon, where each level's players have a ring of their own, that is
//! round and round its ring, as players go there.
//!
//! - **Where it has been:** four times a second the places within 250 units of the bot on its floor are marked; a
//!   place not been to for 40 s is new again.
//! - **Its level:** the places within 48 units of the height it first went on patrol at in this life, or since it was
//!   last moved far at once (a teleport; on the octagon, to its next level's ring): its ring of the octagon, its floor,
//!   got to without leaving that height. A level with nothing 2 s of running off lets it off.
//! - **The next stop:** of the places within 10 s of running, the best for being far (6 s and more as good as any,
//!   under 2 s none), for the way there going through places not been to lately, and onward: leaving along the way
//!   the bot runs, not back.
//! - **On the way:** the next stop is chosen 300 units short of this one, so the bot never stops; a stop with no way
//!   to it counts as been to.

use lb_core::rng::BotRng;
use lb_core::time::SimTime;
use lb_core::{Vec2, Vec3};
use lb_nav_api::{MapView, NavService, NavStatus, NodeId};

use crate::BotBrain;
use crate::mind::{Body, apply_step};

const MARK_PERIOD: f64 = 0.25;
const MARK_NEAR: f32 = 250.0;
const FORGET: f64 = 40.0;
/// An enemy this much above or below is off the bot's level.
pub(crate) const LEVEL: f32 = 128.0;
/// Places this much above or below the height of the level patrolled are off it.
const FLOOR: f32 = 48.0;
const REACH: f32 = 10.0;
const FAR: f32 = 6.0;
const NEAR: f32 = 2.0;
/// The way's first second of running shows which way it leaves.
const LEAVES: f32 = 1.0;
/// Running slower than this the bot has no way it is going.
const HEADING_SPEED: f32 = 100.0;
const NEXT_AT: f32 = 300.0;
/// Out of use this long, the stop chosen is old news.
const STALE: f64 = 1.0;
/// With no way to an enemy, closing in on it is not tried for this long.
pub(crate) const NO_WAY_FOR: f64 = 3.0;
/// The patrol moved the bot this recently: it is going round.
const GOING: f64 = 0.1;
/// Moved this far between two marks, the bot was teleported.
const TELEPORTED: f32 = 400.0;

/// `b` is on the level of `a`.
pub(crate) fn same_level(a: Vec3, b: Vec3) -> bool {
    (a.z - b.z).abs() <= LEVEL
}

#[derive(Clone, Debug, Default)]
pub struct Patrol {
    /// When the bot was last at each place.
    been: Vec<f64>,
    next_mark: SimTime,
    /// Where it is going, and when it last went on there.
    stop: Option<(NodeId, Vec3)>,
    went: SimTime,
    /// The height of the level it keeps to, and where it was at the last mark.
    level: Option<f32>,
    at: Option<Vec3>,
    /// No way to the enemy it would close in on was found: not looked for again before this.
    pub(crate) no_way_until: SimTime,
}

impl Patrol {
    /// The graph was replaced: its places are other ones.
    pub fn reset(&mut self) {
        *self = Patrol::default();
    }

    /// A new life: the way it was going and its level are over, where it has been is not.
    pub fn on_spawn(&mut self) {
        self.stop = None;
        self.level = None;
    }

    /// Marks the places about the bot as been to, a few times a second.
    pub fn mark(&mut self, now: SimTime, origin: Vec3, map: &dyn MapView) {
        if self.been.len() != map.node_count() {
            *self = Patrol {
                been: vec![f64::NEG_INFINITY; map.node_count()],
                ..Patrol::default()
            };
        }
        if now < self.next_mark {
            return;
        }
        self.next_mark = now + MARK_PERIOD;
        if self.at.is_some_and(|at| at.distance(origin) > TELEPORTED) {
            self.on_spawn();
        }
        self.at = Some(origin);
        for (n, been) in self.been.iter_mut().enumerate() {
            let p = map.node_origin(n as NodeId);
            if (p - origin).truncate().length() <= MARK_NEAR && (p.z - origin.z).abs() <= FLOOR {
                *been = now.0;
            }
        }
    }

    /// The best place to go on to from `origin`, running along `heading`, keeping to the level at height `level` (see
    /// the module's description).
    pub fn next_stop(
        &self,
        now: SimTime,
        origin: Vec3,
        level: f32,
        heading: Option<Vec2>,
        map: &dyn MapView,
        rng: &mut BotRng,
    ) -> Option<(NodeId, Vec3)> {
        let start = map.nearest_node(origin, 256.0)?;
        let new = |n: NodeId| {
            self.been
                .get(n as usize)
                .map_or(1.0, |&at| ((now.0 - at) / FORGET).clamp(0.0, 1.0) as f32)
        };
        let best = |within: &dyn Fn(NodeId) -> bool, rng: &mut BotRng| {
            let (time, prev) = lb_knowledge::travel_tree_within(map, start, REACH, within);
            let mut best: Option<(f32, NodeId)> = None;
            for (n, &t) in time.iter().enumerate() {
                let n = n as NodeId;
                if !(NEAR..=REACH).contains(&t) || map.transit(n) {
                    continue;
                }
                let far = (t / FAR).min(1.0);
                // Back along the way from the stop to where it is a second on: how new its places are, and which way
                // it leaves.
                let (mut leaves, mut fresh, mut places) = (n, new(n), 1.0);
                while let Some(&p) = prev.get(leaves as usize)
                    && p != NodeId::MAX
                    && time[p as usize] >= LEAVES
                {
                    leaves = p;
                    fresh += new(p);
                    places += 1.0;
                }
                let fresh = fresh / places;
                let way = (map.node_origin(leaves) - origin).truncate().normalize_or_zero();
                let onward = heading.map_or(1.0, |h| (1.0 + way.dot(h)) / 2.0);
                let score = far * (0.2 + 0.8 * fresh) * (0.1 + 0.9 * onward) * rng.decision.range_f32(0.9, 1.1);
                if best.is_none_or(|b| score > b.0) {
                    best = Some((score, n));
                }
            }
            best.map(|(_, n)| n)
        };
        let on_level = |n: NodeId| (map.node_origin(n).z - level).abs() <= FLOOR;
        best(&on_level, rng)
            .or_else(|| best(&|_| true, rng))
            .map(|n| (n, map.node_origin(n)))
    }

    /// The patrol moved the bot on its last frame (a frame or so before `now`).
    pub fn going(&self, now: SimTime) -> bool {
        now.since(self.went) <= GOING
    }

    /// Counts the stop gone to as been to: there is no way there.
    fn give_up(&mut self, now: SimTime) {
        if let Some((n, _)) = self.stop.take()
            && let Some(been) = self.been.get_mut(n as usize)
        {
            *been = now.0;
        }
    }
}

impl BotBrain {
    /// Moves the bot on round its level (see [`crate::patrol`]); false when it has nowhere to go.
    pub(crate) fn patrol(
        &mut self,
        body: &Body,
        map: Option<&dyn MapView>,
        nav: &mut dyn NavService,
        rng: &mut BotRng,
    ) -> bool {
        let Some(map) = map else { return false };
        let now = body.now;
        let p = &mut self.patrol;
        if now.since(p.went) > STALE {
            p.stop = None;
        }
        let close = p
            .stop
            .is_some_and(|(_, at)| (at - body.origin).truncate().length() < NEXT_AT && same_level(at, body.origin));
        if p.stop.is_none() || close {
            let speed = body.velocity.truncate().length();
            let heading = (speed >= HEADING_SPEED).then(|| body.velocity.truncate() / speed);
            let level = *p.level.get_or_insert_with(|| {
                map.nearest_node(body.origin, 256.0)
                    .map_or(body.origin.z, |n| map.node_origin(n).z)
            });
            p.stop = p.next_stop(now, body.origin, level, heading, map, rng).or(p.stop);
        }
        let Some((_, stop)) = p.stop else { return false };
        let (status, step) = nav.go_to(stop);
        match status {
            NavStatus::Arrived => self.patrol.stop = None,
            NavStatus::NoPath => {
                self.patrol.give_up(now);
                return false;
            }
            NavStatus::Moving => {}
        }
        if let Some(step) = step {
            apply_step(&mut self.intents, &step, body.eye, &mut self.mind);
        }
        self.patrol.went = now;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lb_core::dmath;
    use lb_nav_api::{CampSpot, MineSpot};

    const UPPER: u32 = 48;
    const LOWER: u32 = 32;

    /// Two rings round the middle, as the octagon's: `UPPER` places 1200 units out at height 500, then `LOWER` places
    /// 700 units out at height 300; from every place of the upper one a drop to the lower one, and a lift back up
    /// from place 0 of the lower one. Then a platform of two places at height 900 over the upper ring's place 0, with
    /// a drop down to it.
    struct Rings;

    impl Rings {
        fn ring(n: NodeId) -> Option<(u32, u32, f32, f32)> {
            if n < UPPER {
                Some((n, UPPER, 1200.0, 500.0))
            } else if n < UPPER + LOWER {
                Some((n - UPPER, LOWER, 700.0, 300.0))
            } else {
                None
            }
        }

        fn below(n: NodeId) -> NodeId {
            UPPER + n * LOWER / UPPER
        }
    }

    impl MapView for Rings {
        fn node_count(&self) -> usize {
            (UPPER + LOWER + 2) as usize
        }
        fn node_origin(&self, n: NodeId) -> Vec3 {
            match Rings::ring(n) {
                Some((i, count, r, z)) => {
                    let a = std::f32::consts::TAU * i as f32 / count as f32;
                    let (sin, cos) = dmath::sin_cos(a);
                    Vec3::new(r * cos, r * sin, z)
                }
                None => Vec3::new(1300.0 + 100.0 * (n - UPPER - LOWER) as f32, 0.0, 900.0),
            }
        }
        fn nearest_node(&self, p: Vec3, _max: f32) -> Option<NodeId> {
            (0..self.node_count() as NodeId).min_by(|&a, &b| {
                self.node_origin(a)
                    .distance(p)
                    .total_cmp(&self.node_origin(b).distance(p))
            })
        }
        fn for_each_link(&self, n: NodeId, f: &mut dyn FnMut(NodeId, f32)) {
            let run = |m: NodeId| self.node_origin(n).distance(self.node_origin(m)) / 300.0;
            match Rings::ring(n) {
                Some((i, count, ..)) => {
                    let base = n - i;
                    for m in [base + (i + 1) % count, base + (i + count - 1) % count] {
                        f(m, run(m));
                    }
                    if count == UPPER {
                        f(Rings::below(n), 1.0);
                    } else if i == 0 {
                        f(0, 10.0);
                    }
                }
                None => {
                    let other = if n == UPPER + LOWER { n + 1 } else { n - 1 };
                    f(other, run(other));
                    f(0, 1.0);
                }
            }
        }
        fn visible(&self, _a: NodeId, _b: NodeId) -> bool {
            true
        }
        fn for_each_visible(&self, _n: NodeId, _f: &mut dyn FnMut(NodeId)) {}
        fn flow(&self, _n: NodeId) -> f32 {
            0.5
        }
        fn exposure(&self, _n: NodeId) -> f32 {
            0.5
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

    fn angle(p: Vec3) -> f32 {
        dmath::atan2(p.y, p.x)
    }

    /// Runs a bot on patrol from `from`, along `heading`, for `stops` stops: each time it gets 300 units short of
    /// its stop it chooses the next one. It runs at 300 units a second round the ring it is on, marking as it goes.
    /// The stops and the angle it went round the middle, counterclockwise.
    fn run(from: NodeId, heading: Option<Vec2>, stops: usize) -> (Vec<Vec3>, f32) {
        let map = Rings;
        let mut p = Patrol::default();
        let mut rng = BotRng::new(1, 1);
        let (mut now, mut at, mut heading) = (SimTime(0.0), map.node_origin(from), heading);
        let level = at.z;
        p.mark(now, at, &map);
        let (mut seen, mut turned) = (Vec::new(), 0.0);
        for _ in 0..stops {
            let (_, stop) = p.next_stop(now, at, level, heading, &map, &mut rng).expect("a stop");
            seen.push(stop);
            // Round the middle the short way, to 300 units short of the stop.
            let r = at.truncate().length();
            let mut sweep = angle(stop) - angle(at);
            sweep = (sweep + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
            let sweep = sweep - sweep.signum() * 300.0 / r;
            let steps = (sweep.abs() * r / 75.0).ceil().max(1.0) as usize;
            for _ in 0..steps {
                let a = angle(at) + sweep / steps as f32;
                let (sin, cos) = dmath::sin_cos(a);
                let next = Vec3::new(r * cos, r * sin, at.z);
                heading = Some((next - at).truncate().normalize());
                at = next;
                now += 0.25;
                p.mark(now, at, &map);
            }
            turned += sweep;
        }
        (seen, turned)
    }

    #[test]
    fn round_its_ring_the_bot_goes_on_and_on() {
        // On the upper ring, running counterclockwise: never down to the lower ring, never back.
        let tangent = Vec2::new(0.0, 1.0);
        let (stops, turned) = run(0, Some(tangent), 12);
        assert!(stops.iter().all(|s| s.z == 500.0), "{stops:?}");
        assert!(turned > 2.0 * std::f32::consts::TAU, "{turned}");
        // Standing at first: one way round, then on that way.
        let (stops, turned) = run(10, None, 12);
        assert!(stops.iter().all(|s| s.z == 500.0), "{stops:?}");
        assert!(turned.abs() > 2.0 * std::f32::consts::TAU, "{turned}");
    }

    #[test]
    fn a_stop_is_far_and_where_the_bot_has_not_been() {
        let map = Rings;
        let mut p = Patrol::default();
        let mut rng = BotRng::new(2, 2);
        // Been round the upper ring counterclockwise from place 0 to place 12 lately: it goes on clockwise round to
        // places not been to, standing at place 0.
        for (k, n) in (0..=12).enumerate() {
            p.mark(SimTime(k as f64 * 0.5), map.node_origin(n), &map);
        }
        let at = map.node_origin(0);
        let (n, stop) = p.next_stop(SimTime(7.0), at, at.z, None, &map, &mut rng).unwrap();
        assert!(n < UPPER && n > 24, "{n}");
        assert!(stop.distance(at) > 1000.0);
    }

    #[test]
    fn a_teleport_to_another_ring_makes_that_ring_its_level() {
        let map = Rings;
        let mut p = Patrol::default();
        p.mark(SimTime(0.0), map.node_origin(UPPER), &map);
        p.level = Some(300.0);
        // Walked on along the lower ring: still its level.
        p.mark(SimTime(0.3), map.node_origin(UPPER + 1), &map);
        assert_eq!(p.level, Some(300.0));
        // Teleported up to the upper ring: its level is to be taken anew there.
        p.mark(SimTime(0.6), map.node_origin(5), &map);
        assert_eq!(p.level, None);
    }

    #[test]
    fn a_level_with_nowhere_to_go_lets_the_bot_off_it() {
        let map = Rings;
        let mut p = Patrol::default();
        let mut rng = BotRng::new(3, 3);
        let at = map.node_origin(UPPER + LOWER);
        p.mark(SimTime(0.0), at, &map);
        let (n, _) = p.next_stop(SimTime(0.0), at, at.z, None, &map, &mut rng).unwrap();
        assert!(n < UPPER + LOWER, "{n}");
    }
}
