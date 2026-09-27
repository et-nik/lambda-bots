//! Path following: walks the plain links of a planned path itself and hands special links (jumps, drops, ladders,
//! water, doors, lifts, teleports, breakables) to their executors. Cheap per frame: walking needs no traces, only
//! getting stuck does.
//!
//! When a walk stops making progress the follower works through a fixed ladder of responses: a sidestep, backing
//! off, a jump (only onto a real step ahead, never on a ladder or at an edge), ducking under a low ceiling, and
//! finally a failure with its cause, so the link is avoided for a while and the path is planned again.

use lb_core::{Vec2, Vec3};
use lb_nav_api::NavStep;
use lb_worldq::{HullKind, TraceQuery, Tracer};

use crate::exec::{EYE_HEIGHT, Exec, ExecCtx, ExecStatus, HitKind, MechView, NavInput};
use crate::graph::{LinkFlags, LinkKind, NavGraph, NodeFlags, NodeId};
use crate::known::FailReason;

/// No progress for this long means the bot may be stuck.
const PROGRESS_WINDOW: f64 = 0.8;
/// Getting closer by less than this is no progress.
const PROGRESS_MIN: f32 = 8.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FollowStatus {
    Moving,
    /// Waiting for a mechanism (a lift on its way, a door opening): not stuck.
    Waiting,
    Arrived,
    /// The link `from → to` could not be done.
    Failed {
        from: NodeId,
        to: NodeId,
        reason: FailReason,
    },
    /// Off the path (fell, got pushed): plan again from here, the link is not to blame.
    Replan,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FollowOutput {
    pub step: NavStep,
    pub status: FollowStatus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Recovery {
    None,
    Sidestep,
    BackOff,
    Jump,
    Duck,
}

#[derive(Clone, Debug)]
pub struct PathFollower {
    path: Vec<NodeId>,
    next: usize,
    exec: Option<(NodeId, NodeId, Exec)>,
    exec_started: f64,
    best: f32,
    last_progress: f64,
    recovery: Recovery,
    tried: u8,
    recovery_until: f64,
    side: f32,
    airborne_from: Option<f32>,
}

impl PathFollower {
    /// `path[0]` is the node the bot starts at.
    pub fn new(path: Vec<NodeId>, now: f64) -> PathFollower {
        PathFollower {
            next: usize::from(path.len() > 1),
            path,
            exec: None,
            exec_started: now,
            best: f32::INFINITY,
            last_progress: now,
            recovery: Recovery::None,
            tried: 0,
            recovery_until: 0.0,
            side: 1.0,
            airborne_from: None,
        }
    }

    pub fn goal(&self) -> NodeId {
        *self.path.last().expect("paths are not empty")
    }

    pub fn target(&self) -> Option<NodeId> {
        self.path.get(self.next).copied()
    }

    pub fn remaining(&self) -> &[NodeId] {
        &self.path[self.next.min(self.path.len())..]
    }

    /// The link being followed: `(from, to)`.
    pub fn current_link(&self) -> Option<(NodeId, NodeId)> {
        let to = *self.path.get(self.next)?;
        let from = if self.next > 0 { self.path[self.next - 1] } else { to };
        Some((from, to))
    }

    /// The link being followed has a traversal contract (not plain walking).
    pub fn on_special_link(&self, g: &NavGraph) -> bool {
        let Some((from, to)) = self.current_link() else {
            return false;
        };
        self.next > 0
            && g.find_link(from, to)
                .is_some_and(|l| !l.kind.is_walk() && g.spec(l).is_some())
    }

    /// What the follower is doing: `walk`, or the executor phase of a special link.
    pub fn phase(&self) -> &'static str {
        match &self.exec {
            Some((_, _, e)) => e.phase(),
            None => match self.recovery {
                Recovery::None => "walk",
                Recovery::Sidestep => "walk:sidestep",
                Recovery::BackOff => "walk:back-off",
                Recovery::Jump => "walk:jump",
                Recovery::Duck => "walk:duck",
            },
        }
    }

    fn advance(&mut self, now: f64) {
        self.next += 1;
        self.exec = None;
        self.best = f32::INFINITY;
        self.last_progress = now;
        self.recovery = Recovery::None;
        self.tried = 0;
    }

    fn reached(&self, g: &NavGraph, s: &NavInput, kind: LinkKind) -> bool {
        let node = g.node(self.path[self.next]);
        let to = node.origin - s.origin;
        let flat = to.truncate().length();
        let last = self.next + 1 == self.path.len();
        let dz = (s.feet()
            - (node.origin.z
                - if node.flags.contains(NodeFlags::CROUCH) {
                    18.0
                } else {
                    36.0
                }))
        .abs();
        let radius = if last || kind == LinkKind::Crouch {
            24.0
        } else {
            node.radius.clamp(24.0, 48.0)
        };
        if flat < radius && dz < 40.0 {
            return true;
        }
        // Passed the node: the plane through it, across the segment we came along.
        if !last && self.next > 0 && dz < 40.0 {
            let from = g.node(self.path[self.next - 1]).origin;
            let seg = (node.origin - from).truncate();
            if seg.length() > 1.0 && seg.dot(-to.truncate()) > 0.0 && flat < 64.0 {
                return true;
            }
        }
        false
    }

    pub fn tick(&mut self, g: &NavGraph, s: &NavInput, mech: &dyn MechView, tracer: &mut dyn Tracer) -> FollowOutput {
        for _ in 0..4 {
            let Some((from, to)) = self.current_link() else {
                return FollowOutput {
                    step: NavStep::hold(s.origin + Vec3::Z * EYE_HEIGHT),
                    status: FollowStatus::Arrived,
                };
            };
            let link = (self.next > 0).then(|| g.find_link(from, to)).flatten().copied();
            let spec = link
                .and_then(|l| g.spec(&l))
                .filter(|_| !link.is_some_and(|l| l.kind.is_walk()));
            if let (Some(spec), Some(l)) = (spec, link) {
                if l.kind == LinkKind::Drop && s.health <= spec.needs.health {
                    return self.fail(s, from, to, FailReason::MissingCapability);
                }
                if !matches!(&self.exec, Some((f, t, _)) if *f == from && *t == to) {
                    self.exec = Some((from, to, Exec::new(spec, s.now)));
                    self.exec_started = s.now;
                    self.airborne_from = None;
                }
                if s.now - self.exec_started > f64::from(spec.deadline) {
                    return self.fail(s, from, to, FailReason::ControllerFailure);
                }
                let (fnode, tnode) = (*g.node(from), *g.node(to));
                let Some((_, _, exec)) = self.exec.as_mut() else {
                    unreachable!()
                };
                let mut ctx = ExecCtx {
                    input: s,
                    mech,
                    tracer: &mut *tracer,
                    spec,
                    from: &fnode,
                    to: &tnode,
                };
                let (step, status) = exec.tick(&mut ctx);
                return match status {
                    ExecStatus::Done => {
                        self.advance(s.now);
                        continue;
                    }
                    ExecStatus::Failed(reason) => self.fail(s, from, to, reason),
                    ExecStatus::Running => FollowOutput {
                        step,
                        status: FollowStatus::Moving,
                    },
                    ExecStatus::Waiting => {
                        self.last_progress = s.now;
                        FollowOutput {
                            step,
                            status: FollowStatus::Waiting,
                        }
                    }
                };
            }
            let kind = link.map_or(LinkKind::Walk, |l| l.kind);
            if self.reached(g, s, kind) {
                self.advance(s.now);
                continue;
            }
            return self.walk(g, s, mech, tracer, from, to, kind, link.map(|l| l.flags));
        }
        FollowOutput {
            step: NavStep::hold(s.origin + Vec3::Z * EYE_HEIGHT),
            status: FollowStatus::Moving,
        }
    }

    fn fail(&mut self, s: &NavInput, from: NodeId, to: NodeId, reason: FailReason) -> FollowOutput {
        self.exec = None;
        FollowOutput {
            step: NavStep::hold(s.origin + Vec3::Z * EYE_HEIGHT),
            status: FollowStatus::Failed { from, to, reason },
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn walk(
        &mut self,
        g: &NavGraph,
        s: &NavInput,
        mech: &dyn MechView,
        tracer: &mut dyn Tracer,
        from: NodeId,
        to: NodeId,
        kind: LinkKind,
        flags: Option<LinkFlags>,
    ) -> FollowOutput {
        let node = g.node(to);
        let d = (node.origin - s.origin).truncate();
        let flat = d.length();
        let mut step = NavStep::hold(node.origin + Vec3::Z * EYE_HEIGHT);
        step.move_dir = d.normalize_or_zero();
        step.speed = s.max_speed;
        if self.next + 1 == self.path.len() {
            // The last node: slow down so the bot stops there instead of running past it (and off a ledge).
            step.speed = s.max_speed.min((flat * 3.0).max(90.0));
        } else if let Some(entry) = self
            .path
            .get(self.next + 1)
            .and_then(|&after| g.find_link(to, after))
            .filter(|l| !l.kind.is_walk())
            .and_then(|l| g.spec(l))
            .and_then(|spec| spec.entry_speed())
        {
            // A traversal starts at the node: arrive no faster than it can take (friction sheds about 4 units per
            // unit of speed and second, so start braking a few dozen units out).
            step.speed = s.max_speed.min(entry.max(flat * 4.0));
        }
        // Look further along a straight walk so turns start early.
        if let Some(&after) = self.path.get(self.next + 1)
            && flat < 96.0
        {
            step.look_at = g.node(after).origin + Vec3::Z * EYE_HEIGHT;
        }
        step.duck = kind == LinkKind::Crouch || node.flags.contains(NodeFlags::CROUCH);

        // Fell off the path: plan again from where it landed, unless the target is still within a walk.
        if !s.on_ground && !s.on_ladder && s.waterlevel < 2 {
            self.airborne_from.get_or_insert(s.feet());
        } else if let Some(top) = self.airborne_from.take()
            && top - s.feet() > 40.0
            && (node.origin.z - 36.0 - s.feet()).abs() > 40.0
        {
            return FollowOutput {
                step,
                status: FollowStatus::Replan,
            };
        }

        if flat < self.best - PROGRESS_MIN {
            self.best = flat;
            self.last_progress = s.now;
            self.recovery = Recovery::None;
            self.tried = 0;
        }
        if s.now < self.recovery_until {
            self.apply_recovery(&mut step);
            return FollowOutput {
                step,
                status: FollowStatus::Moving,
            };
        }
        if s.now - self.last_progress < PROGRESS_WINDOW || s.on_ladder {
            return FollowOutput {
                step,
                status: FollowStatus::Moving,
            };
        }
        // No progress: work through the responses, then give up with a cause.
        self.last_progress = s.now;
        let dir = step.move_dir;
        let probe = probe_ahead(s, dir, tracer);
        let blocked_by = blocker(s, dir, tracer, mech);
        if matches!(blocked_by, Some(HitKind::Mover(_))) || flags.is_some_and(|f| f.contains(LinkFlags::DYNAMIC)) {
            // A door or platform in the way: give it a moment, then report the mechanism.
            if self.tried >= 3 {
                return self.fail(s, from, to, FailReason::WaitingForInteraction);
            }
            self.tried += 1;
            self.recovery_until = s.now + 0.8;
            self.recovery = Recovery::None;
            step.move_dir = Vec2::ZERO;
            return FollowOutput {
                step,
                status: FollowStatus::Waiting,
            };
        }
        self.tried += 1;
        self.recovery = match self.tried {
            1 => Recovery::Sidestep,
            2 => Recovery::BackOff,
            3 if probe.step_up && s.on_ground => Recovery::Jump,
            3 | 4 if probe.low_ceiling => Recovery::Duck,
            3 => Recovery::Sidestep,
            _ => {
                let reason = match blocked_by {
                    Some(HitKind::Player) => FailReason::TemporarilyOccupied,
                    Some(HitKind::World | HitKind::Other) => FailReason::GeometryInvalid,
                    _ => FailReason::ControllerFailure,
                };
                return self.fail(s, from, to, reason);
            }
        };
        self.side = -self.side;
        self.recovery_until = s.now
            + match self.recovery {
                Recovery::Sidestep => 0.35,
                Recovery::BackOff => 0.4,
                Recovery::Jump => 0.5,
                Recovery::Duck => 0.8,
                Recovery::None => 0.0,
            };
        self.apply_recovery(&mut step);
        FollowOutput {
            step,
            status: FollowStatus::Moving,
        }
    }

    fn apply_recovery(&self, step: &mut NavStep) {
        let dir = step.move_dir;
        match self.recovery {
            Recovery::Sidestep => step.move_dir = Vec2::new(-dir.y, dir.x) * self.side,
            Recovery::BackOff => step.move_dir = -dir,
            Recovery::Jump => step.jump = true,
            Recovery::Duck => step.duck = true,
            Recovery::None => {}
        }
    }
}

struct Probe {
    /// A floor 18–45 units higher right ahead: a jump gets onto it.
    step_up: bool,
    /// Standing does not fit ahead but crouching does.
    low_ceiling: bool,
}

fn probe_ahead(s: &NavInput, dir: Vec2, tracer: &mut dyn Tracer) -> Probe {
    let feet = s.feet();
    let ahead = s.origin + dir.extend(0.0) * 24.0;
    let high = Vec3::new(ahead.x, ahead.y, feet + 36.0 + 46.0);
    let clear = tracer.trace(&TraceQuery::hull(
        Vec3::new(s.origin.x, s.origin.y, high.z),
        high,
        HullKind::Stand,
    ));
    let mut step_up = false;
    if !clear.start_solid && clear.fraction >= 1.0 {
        let down = tracer.trace(&TraceQuery::hull(high, high - Vec3::Z * 64.0, HullKind::Stand));
        let floor = down.end.z - 36.0;
        step_up = down.fraction < 1.0 && down.normal.z >= 0.7 && (feet + 18.0..=feet + 45.0).contains(&floor);
    }
    let stand = tracer.trace(&TraceQuery::hull(
        s.origin,
        s.origin + dir.extend(0.0) * 24.0,
        HullKind::Stand,
    ));
    let crouched = Vec3::new(s.origin.x, s.origin.y, feet + 18.0);
    let crouch = tracer.trace(&TraceQuery::hull(
        crouched,
        crouched + dir.extend(0.0) * 24.0,
        HullKind::Crouch,
    ));
    Probe {
        step_up,
        low_ceiling: stand.fraction < 0.5 && crouch.fraction >= 1.0 && !crouch.start_solid,
    }
}

/// What stands right in the way, players included.
fn blocker(s: &NavInput, dir: Vec2, tracer: &mut dyn Tracer, mech: &dyn MechView) -> Option<HitKind> {
    let hull = if s.ducked { HullKind::Crouch } else { HullKind::Stand };
    let mut q = TraceQuery::hull(s.origin, s.origin + dir.extend(0.0) * 32.0, hull);
    q.ignore_monsters = false;
    let tr = tracer.trace(&q);
    (tr.fraction < 1.0).then(|| mech.hit_kind(tr.hit.unwrap_or(0)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::NoMechs;
    use lb_kin::boxworld::BoxWorld;

    fn line_graph(kinds: &[LinkKind]) -> NavGraph {
        let points: Vec<(f32, f32)> = (0..=kinds.len()).map(|i| (i as f32 * 100.0, 0.0)).collect();
        let edges: Vec<(u32, u32, LinkKind)> = kinds
            .iter()
            .enumerate()
            .map(|(i, k)| (i as u32, i as u32 + 1, *k))
            .collect();
        let mut g = crate::plan::tests::graph(&points, &edges);
        for n in &mut g.nodes {
            n.origin.z = 36.0;
        }
        g
    }

    fn input(x: f32, now: f64) -> NavInput {
        NavInput {
            origin: Vec3::new(x, 0.0, 36.0),
            on_ground: true,
            now,
            max_speed: 270.0,
            health: 100.0,
            ..Default::default()
        }
    }

    #[test]
    fn walks_node_to_node_and_arrives() {
        let g = line_graph(&[LinkKind::Walk, LinkKind::Walk]);
        let mut w = BoxWorld::new();
        let mut f = PathFollower::new(vec![0, 1, 2], 0.0);
        let out = f.tick(&g, &input(0.0, 0.0), &NoMechs, &mut w);
        assert_eq!(out.step.move_dir, Vec2::X);
        assert_eq!(f.target(), Some(1));
        f.tick(&g, &input(90.0, 0.3), &NoMechs, &mut w);
        assert_eq!(f.target(), Some(2), "within the radius of node 1");
        assert_eq!(
            f.tick(&g, &input(195.0, 0.6), &NoMechs, &mut w).status,
            FollowStatus::Arrived
        );
    }

    #[test]
    fn no_progress_escalates_to_a_failure() {
        let g = line_graph(&[LinkKind::Walk]);
        let mut w = BoxWorld::new();
        w.solid(Vec3::new(40.0, -256.0, 0.0), Vec3::new(60.0, 256.0, 200.0));
        let mut f = PathFollower::new(vec![0, 1], 0.0);
        let mut statuses = Vec::new();
        let mut sidestepped = false;
        for i in 0..100 {
            let out = f.tick(&g, &input(0.0, i as f64 * 0.1), &NoMechs, &mut w);
            sidestepped |= out.step.move_dir.y.abs() > 0.5;
            assert!(!out.step.jump, "no jump without a step to jump onto");
            statuses.push(out.status);
        }
        assert!(sidestepped, "first response is a sidestep");
        assert!(statuses.contains(&FollowStatus::Failed {
            from: 0,
            to: 1,
            reason: FailReason::GeometryInvalid
        }));
    }
}
