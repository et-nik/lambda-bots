//! Places on the map in a bot's mind: when it last had each in sight (and saw nobody there), and where a lost enemy
//! may be by now.
//!
//! A lost enemy may have gone as far as a player runs in the time since, but not through a place the bot has watched
//! since before the enemy could have got there. The places it could have reached are weighted by how likely a player
//! goes there (on along the way it was running, where players pass, farther on the longer it is gone), less the
//! places in sight since: one in sight now is all but ruled out, one looked at a while ago less and less.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use lb_core::math::view_angle_vectors;
use lb_core::time::SimTime;
use lb_core::{Vec3, dmath};
use lb_nav_api::{MapView, NodeId};
use smallvec::SmallVec;

/// Places in sight count as looked at this close, and within the view's cone.
const LOOK_RANGE: f32 = 1500.0;
/// Places this close count as looked at wherever the view is.
const AROUND: f32 = 128.0;
const WATCH_PERIOD: f64 = 0.25;
/// A place looked at on consecutive looks this far apart has been watched all along.
const STREAK_GAP: f64 = 0.6;
/// A place looked at this recently is in sight now.
const IN_SIGHT: f64 = 0.35;
/// Eye above a node's player origin.
const EYE: f32 = 28.0;
/// Players may run a little faster than the graph's travel times assume.
const FASTER: f32 = 0.85;
/// A place looked at empty comes back as likely as before in about this long (someone may walk in again).
const RECOVER: f32 = 2.5;
const SPREAD_PERIOD: f64 = 0.5;
/// How much the way a lost player ran weighs: a place straight ahead is this much likelier than one behind, over 2.
const AHEAD: f32 = 0.6;

/// When the bot last had each place in sight, and since when without a break.
#[derive(Clone, Debug, Default)]
pub struct Watch {
    seen: Vec<f64>,
    since: Vec<f64>,
    /// The place the bot is at.
    pub node: Option<NodeId>,
    next: SimTime,
}

impl Watch {
    /// A few times a second: the bot's place, and every place it sees from there within its view.
    pub fn update(&mut self, now: SimTime, origin: Vec3, eye: Vec3, view: Vec3, half_fov: f32, map: &dyn MapView) {
        if self.seen.len() != map.node_count() {
            self.seen = vec![f64::NEG_INFINITY; map.node_count()];
            self.since = self.seen.clone();
            self.next = SimTime::ZERO;
        }
        if now < self.next {
            return;
        }
        self.next = now + WATCH_PERIOD;
        self.node = map.nearest_node(origin, 256.0);
        let Some(at) = self.node else { return };
        let (forward, _, _) = view_angle_vectors(view);
        let cone = dmath::cos(half_fov.to_radians());
        let (seen, since) = (&mut self.seen, &mut self.since);
        let mut note = |m: NodeId| {
            let i = m as usize;
            if now.0 - seen[i] > STREAK_GAP {
                since[i] = now.0;
            }
            seen[i] = now.0;
        };
        note(at);
        map.for_each_visible(at, &mut |m| {
            let d = map.node_origin(m) + Vec3::Z * EYE - eye;
            let len = d.length();
            if len <= AROUND || (len <= LOOK_RANGE && forward.dot(d / len) >= cone) {
                note(m);
            }
        });
    }

    /// When the place was last in sight.
    pub fn seen_at(&self, n: NodeId) -> Option<SimTime> {
        self.seen.get(n as usize).filter(|t| t.is_finite()).map(|&t| SimTime(t))
    }

    /// Since when the place has been in sight without a break, if it is in sight at `now`.
    pub fn watched_since(&self, n: NodeId, now: SimTime) -> Option<SimTime> {
        let i = n as usize;
        let seen = *self.seen.get(i)?;
        (now.0 - seen <= IN_SIGHT).then(|| SimTime(self.since[i]))
    }

    /// How many places have been in sight.
    pub fn known(&self) -> usize {
        self.seen.iter().filter(|t| t.is_finite()).count()
    }
}

#[derive(Clone, Copy, PartialEq)]
struct Open {
    cost: f32,
    node: NodeId,
}

impl Eq for Open {}

impl Ord for Open {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .cost
            .total_cmp(&self.cost)
            .then_with(|| other.node.cmp(&self.node))
    }
}

impl PartialOrd for Open {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Seconds of running from `from` to every place within `max` seconds (infinite beyond).
pub fn travel(map: &dyn MapView, from: NodeId, max: f32) -> Vec<f32> {
    travel_tree(map, from, max).0
}

/// As [`travel`], with the place each one is reached from on the quickest way (`NodeId::MAX` for `from` and the
/// places out of reach).
pub fn travel_tree(map: &dyn MapView, from: NodeId, max: f32) -> (Vec<f32>, Vec<NodeId>) {
    travel_tree_within(map, from, max, &|_| true)
}

/// As [`travel_tree`], by way of the places `within` lets through only.
pub fn travel_tree_within(
    map: &dyn MapView,
    from: NodeId,
    max: f32,
    within: &dyn Fn(NodeId) -> bool,
) -> (Vec<f32>, Vec<NodeId>) {
    let mut cost = vec![f32::INFINITY; map.node_count()];
    let mut prev = vec![NodeId::MAX; map.node_count()];
    let Some(c0) = cost.get_mut(from as usize) else {
        return (cost, prev);
    };
    *c0 = 0.0;
    let mut open = BinaryHeap::new();
    open.push(Open { cost: 0.0, node: from });
    while let Some(Open { cost: c, node }) = open.pop() {
        if c > cost[node as usize] {
            continue;
        }
        map.for_each_link(node, &mut |to, t| {
            let next = c + t;
            if next <= max && next < cost[to as usize] && within(to) {
                cost[to as usize] = next;
                prev[to as usize] = node;
                open.push(Open { cost: next, node: to });
            }
        });
    }
    (cost, prev)
}

/// Where a lost player may be.
#[derive(Clone, Debug)]
pub struct Spread {
    /// When and where the player was last placed.
    pub lost_at: SimTime,
    pub from: NodeId,
    heading: lb_core::Vec2,
    horizon: f32,
    /// Places it may be at and the chance of each, likeliest first.
    places: Vec<(NodeId, f32)>,
    pub updated: SimTime,
}

impl Spread {
    /// A player last placed at `pos` at `lost_at`, running along `vel` when that is known; it is looked for as far
    /// as it runs in `horizon` seconds.
    pub fn new(
        map: &dyn MapView,
        pos: Vec3,
        vel: Option<Vec3>,
        lost_at: SimTime,
        horizon: f32,
        now: SimTime,
        watch: Option<&Watch>,
    ) -> Option<Spread> {
        let mut s = Spread {
            lost_at,
            from: map.nearest_node(pos, 256.0)?,
            heading: vel.map(|v| v.truncate().normalize_or_zero()).unwrap_or_default(),
            horizon,
            places: Vec::new(),
            updated: SimTime(f64::NEG_INFINITY),
        };
        s.update(now, map, watch);
        Some(s)
    }

    /// Works the chances out again, every half a second at most.
    pub fn update(&mut self, now: SimTime, map: &dyn MapView, watch: Option<&Watch>) {
        if now.since(self.updated) < SPREAD_PERIOD {
            return;
        }
        self.updated = now;
        let tau = now.since(self.lost_at).max(0.0) as f32;
        let reach = (tau / FASTER + 0.2).min(self.horizon);
        let n = map.node_count();
        let mut cost = vec![f32::INFINITY; n];
        let mut ahead = vec![0.5f32; n];
        // Watched since before the player could have come by: it did not pass there.
        let closed = |m: NodeId, arrive: f32| {
            watch
                .and_then(|w| w.watched_since(m, now))
                .is_some_and(|since| since.since(self.lost_at) as f32 <= arrive * FASTER + 0.2)
        };
        let start = map.node_origin(self.from);
        let mut open = BinaryHeap::new();
        cost[self.from as usize] = 0.0;
        open.push(Open {
            cost: 0.0,
            node: self.from,
        });
        let mut order = Vec::new();
        while let Some(Open { cost: c, node }) = open.pop() {
            if c > cost[node as usize] {
                continue;
            }
            order.push(node);
            if node != self.from && closed(node, c) {
                continue;
            }
            map.for_each_link(node, &mut |to, t| {
                let next = c + t;
                if next <= reach && next < cost[to as usize] {
                    cost[to as usize] = next;
                    ahead[to as usize] = if node == self.from {
                        let dir = (map.node_origin(to) - start).truncate().normalize_or_zero();
                        0.5 + 0.5 * dir.dot(self.heading)
                    } else {
                        ahead[node as usize]
                    };
                    open.push(Open { cost: next, node: to });
                }
            });
        }
        let mut sum = 0.0;
        self.places.clear();
        for m in order {
            let arrive = cost[m as usize];
            let soonest = arrive * FASTER;
            let on = if tau > 0.05 { (soonest / tau).min(1.0) } else { 1.0 };
            let prior = (1.0 - AHEAD + 2.0 * AHEAD * ahead[m as usize]) * (0.5 + map.flow(m));
            let mut w = prior * (0.35 + 0.65 * on);
            if let Some(t) = watch.and_then(|w| w.seen_at(m))
                && t.since(self.lost_at) as f32 >= soonest - 0.2
            {
                let ago = now.since(t).max(0.0);
                w *= if ago <= IN_SIGHT {
                    0.02
                } else {
                    1.0 - 0.95 * dmath::exp(-(ago as f32) / RECOVER)
                };
            }
            sum += w;
            self.places.push((m, w));
        }
        if sum > 0.0 {
            for p in &mut self.places {
                p.1 /= sum;
            }
        }
        self.places.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    }

    /// The `k` likeliest places and their chance, likeliest first.
    pub fn likely(&self, k: usize) -> SmallVec<[(NodeId, f32); 16]> {
        self.places.iter().take(k).copied().collect()
    }

    /// The chance the player is at `n`.
    pub fn chance(&self, n: NodeId) -> f32 {
        self.places.iter().find(|p| p.0 == n).map_or(0.0, |p| p.1)
    }

    /// Places it may be at.
    pub fn places(&self) -> usize {
        self.places.len()
    }

    /// Worked out less than half a second ago.
    pub fn fresh(&self, now: SimTime) -> bool {
        now.since(self.updated) < SPREAD_PERIOD
    }
}

#[cfg(test)]
pub(crate) mod testmap {
    use super::*;
    use lb_nav_api::{CampSpot, MineSpot};

    /// A corridor of nodes along x, 100 units apart, a third of a second's walk each; a corner at x = 350 nobody sees
    /// around.
    pub struct Corridor {
        pub n: usize,
    }

    impl MapView for Corridor {
        fn node_count(&self) -> usize {
            self.n
        }
        fn node_origin(&self, n: NodeId) -> Vec3 {
            Vec3::new(n as f32 * 100.0, 0.0, 0.0)
        }
        fn nearest_node(&self, p: Vec3, _max: f32) -> Option<NodeId> {
            Some(((p.x / 100.0).round().max(0.0) as usize).min(self.n - 1) as NodeId)
        }
        fn for_each_link(&self, n: NodeId, f: &mut dyn FnMut(NodeId, f32)) {
            if n > 0 {
                f(n - 1, 1.0 / 3.0);
            }
            if (n as usize) + 1 < self.n {
                f(n + 1, 1.0 / 3.0);
            }
        }
        fn visible(&self, a: NodeId, b: NodeId) -> bool {
            (a < 4) == (b < 4)
        }
        fn for_each_visible(&self, n: NodeId, f: &mut dyn FnMut(NodeId)) {
            for m in 0..self.n as NodeId {
                if m != n && self.visible(n, m) {
                    f(m);
                }
            }
        }
        fn flow(&self, _n: NodeId) -> f32 {
            0.5
        }
        fn exposure(&self, _n: NodeId) -> f32 {
            0.0
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
}

#[cfg(test)]
mod tests {
    use super::testmap::Corridor;
    use super::*;

    fn look(w: &mut Watch, map: &Corridor, t: f64, x: f32, yaw: f32) {
        w.update(
            SimTime(t),
            Vec3::new(x, 0.0, 0.0),
            Vec3::new(x, 0.0, 28.0),
            Vec3::new(0.0, yaw, 0.0),
            50.0,
            map,
        );
    }

    #[test]
    fn a_lost_player_spreads_on_ahead_and_not_past_where_the_bot_watches() {
        let map = Corridor { n: 12 };
        let run = Some(Vec3::new(300.0, 0.0, 0.0));
        // Lost at node 5 running toward +x.
        let mut s = Spread::new(
            &map,
            Vec3::new(500.0, 0.0, 0.0),
            run,
            SimTime(0.0),
            8.0,
            SimTime(0.0),
            None,
        )
        .unwrap();
        assert_eq!(s.likely(1)[0].0, 5, "at first where it was");
        s.update(SimTime(1.0), &map, None);
        let top: Vec<NodeId> = s.likely(3).iter().map(|x| x.0).collect();
        assert!(top.iter().all(|&n| n > 5), "on ahead after a second: {top:?}");
        assert!(
            s.likely(16).iter().all(|&(n, _)| (1..=9).contains(&n)),
            "no farther than it can run"
        );
        // From 1.1 s on the bot at node 9 looks back along the corridor: nodes 4..11 are in sight, empty.
        let mut w = Watch::default();
        for i in 0..4 {
            look(&mut w, &map, 1.1 + f64::from(i) * 0.25, 900.0, 180.0);
        }
        s.update(SimTime(1.9), &map, Some(&w));
        let top = s.likely(1)[0].0;
        assert!(
            top < 4,
            "the places in sight are ruled out, and so is the way past them: {top}"
        );
        assert_eq!(
            s.chance(10),
            0.0,
            "it could only get past node 9 through the bot's sight"
        );
    }

    #[test]
    fn the_watch_notes_places_in_view_only() {
        let map = Corridor { n: 8 };
        let mut w = Watch::default();
        // At node 1 looking along +x: nodes 2 and 3 are in sight (4.. are round the corner), node 0 behind.
        look(&mut w, &map, 3.0, 100.0, 0.0);
        assert_eq!(w.node, Some(1));
        assert_eq!(w.seen_at(3), Some(SimTime(3.0)));
        assert_eq!(w.seen_at(0), Some(SimTime(3.0)), "close enough to count behind");
        assert_eq!(w.seen_at(5), None);
        look(&mut w, &map, 3.25, 100.0, 0.0);
        assert_eq!(w.watched_since(3, SimTime(3.3)), Some(SimTime(3.0)));
        assert_eq!(w.watched_since(3, SimTime(4.0)), None, "out of sight since");
        let mut w = Watch::default();
        look(&mut w, &map, 3.0, 300.0, 0.0);
        assert_eq!(w.seen_at(0), None, "behind and far");
    }
}
