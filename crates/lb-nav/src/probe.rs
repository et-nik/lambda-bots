//! Live checks of special links: a few traces per link (the floor at both ends, the way between) whose results
//! offline the live engine must reproduce. A mismatch means the offline world and the server disagree (an entity
//! the BSP does not show, a different build of the map), and the link is not to be trusted.

use lb_bsp::BspWorld;
use lb_bsp::mech::Mechanisms;
use lb_core::Vec3;
use lb_worldq::{HullKind, Trace, TraceQuery, Tracer};
use smallvec::SmallVec;

use crate::graph::{LinkKind, NavNode, NodeId};
use crate::spec::{Action, TraversalSpec};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Probe {
    Trace {
        query: TraceQuery,
        fraction: f32,
        start_solid: bool,
    },
    Contents {
        point: Vec3,
        contents: i32,
    },
}

impl Probe {
    /// Traces a probe costs.
    pub fn cost(&self) -> u32 {
        1
    }

    pub fn run(&self, tracer: &mut dyn Tracer) -> bool {
        match *self {
            Probe::Trace {
                query,
                fraction,
                start_solid,
            } => {
                let live = tracer.trace(&query);
                live.start_solid == start_solid && (live.fraction - fraction).abs() < 0.02
            }
            Probe::Contents { point, contents } => tracer.point_contents(point) == contents,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct LinkProbe {
    pub from: NodeId,
    pub to: NodeId,
    pub probes: SmallVec<[Probe; 4]>,
}

fn offline(world: &mut BspWorld, mech: &Mechanisms, query: TraceQuery) -> Option<Probe> {
    let tr: Trace = world.trace(&query);
    // Where a door or lift stands depends on the moment: such traces prove nothing.
    if tr.hit.is_some_and(|h| h != 0 && mech.mover(h as usize).is_some()) {
        return None;
    }
    Some(Probe::Trace {
        query,
        fraction: tr.fraction,
        start_solid: tr.start_solid,
    })
}

/// Probes of a jump, drop or ladder link; `None` for other kinds (they are checked when used).
pub fn probes_for(
    world: &mut BspWorld,
    mech: &Mechanisms,
    a: (NodeId, &NavNode),
    b: (NodeId, &NavNode),
    kind: LinkKind,
    spec: &TraversalSpec,
) -> Option<LinkProbe> {
    let (from, na) = a;
    let (to, nb) = b;
    let mut probes = SmallVec::new();
    match (kind, spec.action) {
        (LinkKind::Jump | LinkKind::Drop, _) => {
            let floor = |p: Vec3| TraceQuery::hull(p + Vec3::Z * 2.0, p - Vec3::Z * 16.0, HullKind::Stand);
            let eye = |p: Vec3| p + Vec3::Z * 28.0;
            for q in [
                floor(na.origin),
                TraceQuery::line(eye(na.origin), eye(nb.origin)),
                floor(nb.origin),
            ] {
                probes.extend(offline(world, mech, q));
            }
        }
        (LinkKind::Ladder, Action::Ladder { mount, .. }) => {
            probes.push(Probe::Contents {
                point: mount,
                contents: world.point_contents(mount),
            });
            probes.extend(offline(world, mech, TraceQuery::hull(mount, mount, HullKind::Stand)));
        }
        _ => return None,
    }
    (!probes.is_empty()).then_some(LinkProbe { from, to, probes })
}

/// Runs the probes of a graph a slice at a time on the live server, switching off links whose probes disagree.
#[derive(Clone, Debug, Default)]
pub struct LiveCheck {
    next: usize,
    pub confirmed: usize,
    pub mismatched: Vec<(NodeId, NodeId)>,
    pub done: bool,
}

impl LiveCheck {
    /// Checks links until `traces` traces are spent or `more()` says to stop; returns the links found wrong in
    /// this slice.
    pub fn run(
        &mut self,
        probes: &[LinkProbe],
        tracer: &mut dyn Tracer,
        mut traces: u32,
        more: &mut dyn FnMut() -> bool,
    ) -> Vec<(NodeId, NodeId)> {
        let mut wrong = Vec::new();
        while let Some(link) = probes.get(self.next) {
            let cost: u32 = link.probes.iter().map(Probe::cost).sum();
            if cost > traces || !more() {
                return wrong;
            }
            traces -= cost;
            self.next += 1;
            if link.probes.iter().all(|p| p.run(tracer)) {
                self.confirmed += 1;
            } else {
                self.mismatched.push((link.from, link.to));
                wrong.push((link.from, link.to));
            }
        }
        self.done = true;
        wrong
    }

    pub fn progress(&self, total: usize) -> String {
        format!(
            "{}/{total} special links checked on the server: {} confirmed, {} disagree{}",
            self.next.min(total),
            self.confirmed,
            self.mismatched.len(),
            if self.done { "" } else { " (running)" }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lb_kin::boxworld::BoxWorld;

    #[test]
    fn probes_agree_with_the_same_world_and_catch_a_changed_one() {
        let q = TraceQuery::hull(Vec3::new(0.0, 0.0, 38.0), Vec3::new(0.0, 0.0, 20.0), HullKind::Stand);
        let mut w = BoxWorld::new();
        w.floor(0.0, 512.0);
        let tr = w.trace(&q);
        let link = LinkProbe {
            from: 1,
            to: 2,
            probes: SmallVec::from_slice(&[Probe::Trace {
                query: q,
                fraction: tr.fraction,
                start_solid: tr.start_solid,
            }]),
        };
        let mut check = LiveCheck::default();
        assert!(
            check
                .run(std::slice::from_ref(&link), &mut w, 64, &mut || true)
                .is_empty()
        );
        assert!(check.done && check.confirmed == 1);
        // The server has no floor there.
        let mut empty = BoxWorld::new();
        let mut check = LiveCheck::default();
        assert_eq!(check.run(&[link], &mut empty, 64, &mut || true), vec![(1, 2)]);
    }
}
