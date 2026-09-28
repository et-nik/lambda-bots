//! Cover from a threat (the sound core of yapb's `findCoverNode`): a place out of sight of where the threat is and
//! of every place next to it, that the bot gets to before the threat could, not too far, not where bots get hurt,
//! with little traffic in sight and more than one way out.

use lb_core::Vec3;
use lb_nav::NavGraph;
use lb_nav_api::NodeId;
use smallvec::SmallVec;

use crate::paths;
use crate::tactics::MapTactics;

/// The bot must be there this many seconds before the threat could be.
const LEAD: f32 = 0.3;

/// Up to four places out of sight of `threat`, reached from `from` within `reach` seconds, best first.
pub fn cover(
    graph: &NavGraph,
    t: &MapTactics,
    from: Vec3,
    threat: Vec3,
    reach: f32,
    danger: &dyn Fn(NodeId) -> f32,
) -> SmallVec<[NodeId; 4]> {
    let mut out = SmallVec::new();
    let (Some(me), Some(th)) = (t.nearest(from, 400.0), t.nearest(threat, 600.0)) else {
        return out;
    };
    let mut watchers: SmallVec<[NodeId; 16]> = SmallVec::new();
    watchers.push(th);
    for l in graph.links(th).iter().filter(|l| l.plain()) {
        watchers.push(l.to);
    }
    let mine = paths::tree(graph, me, reach);
    let theirs = paths::tree(graph, th, reach + 3.0);
    let mut cands: Vec<(NodeId, f32)> = (0..graph.len() as NodeId)
        .filter(|&n| {
            let i = n as usize;
            mine.cost[i].is_finite()
                && !t.transit[i]
                && mine.cost[i] + LEAD < theirs.cost[i]
                && !watchers.iter().any(|&w| w == n || t.vis.get(w, n))
        })
        .map(|n| {
            let i = n as usize;
            let s = &t.spots[i];
            let score =
                mine.cost[i] + 1.5 * danger(n) + 0.8 * s.near + 0.5 * t.flow[i] - 0.2 * f32::from(s.exits.min(3));
            (n, score)
        })
        .collect();
    cands.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
    out.extend(cands.into_iter().take(4).map(|c| c.0));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testmap::corridor;

    #[test]
    fn cover_is_out_of_sight_and_reached_first() {
        let (g, t) = corridor();
        // The bot at x = 200, the threat at x = 0: cover is past the corner, the nearest node there first.
        let got = cover(&g, &t, Vec3::new(200.0, 0.0, 0.0), Vec3::ZERO, 5.0, &|_| 0.0);
        assert_eq!(got.first(), Some(&4));
        assert!(got.iter().all(|&n| n >= 4));
        // Danger makes a place worse.
        let got = cover(&g, &t, Vec3::new(200.0, 0.0, 0.0), Vec3::ZERO, 5.0, &|n| {
            if n == 4 { 1.0 } else { 0.0 }
        });
        assert_eq!(got.first(), Some(&5));
        // The threat past the corner, the bot before it: the threat would get there first.
        let got = cover(
            &g,
            &t,
            Vec3::new(300.0, 0.0, 0.0),
            Vec3::new(700.0, 0.0, 0.0),
            5.0,
            &|_| 0.0,
        );
        assert!(got.iter().all(|&n| n < 4), "{got:?}");
    }
}
