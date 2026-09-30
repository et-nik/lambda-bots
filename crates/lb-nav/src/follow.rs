//! Path following: walks the plain links of a planned path itself and hands special links (jumps, drops, ladders,
//! water, doors, lifts, teleports, breakables) to their executors. Cheap per frame: walking traces only when it
//! cuts a corner toward the next node and when it gets stuck.
//!
//! A walking bot looks a stretch ahead along the path, nearly level, and corrects its sideways drift at every turn
//! so it keeps to the links, which are clear in a straight line.
//!
//! When a walk stops making progress the follower works through a fixed ladder of responses: a sidestep, backing
//! off, a jump (only onto a real step ahead, never on a ladder or at an edge), ducking under a low ceiling, and
//! finally a failure with its cause, so the link is avoided for a while and the path is planned again.
//!
//! A bot with the long jump module and the will to use it long jumps along straight, level stretches of the path
//! at least 400 units long (yapb's runway), onto a node 300–470 units ahead, once the flight is followed through the
//! server's traces and comes down there without fall damage. A bold one (skilled) also long jumps round corners onto
//! the path past them, down drops, onto nodes only 250 units off and one long jump after another, and takes a hard
//! landing it can afford. The same way the follower takes a gauss boost onto a node further along when the bot asks
//! for one: both are shortcuts off the graph's links, and a failed one is not the link's fault.

use lb_core::math::view_angle_vectors;
use lb_core::{Vec2, Vec3};
use lb_kin::tricks::LONGJUMP_TAKEOFF;
use lb_nav_api::NavStep;
use lb_worldq::{HullKind, TraceQuery, Tracer};

use crate::classify::anchor;
use crate::exec::{EYE_HEIGHT, Exec, ExecCtx, ExecStatus, HitKind, MechView, NavInput, travel_look};
use crate::graph::{LinkFlags, LinkKind, NavGraph, NavNode, NodeFlags, NodeId};
use crate::known::FailReason;
use crate::spec::{Action, Anchor, Cost, Needs, Stance, TraversalSpec};

/// No progress for this long means the bot may be stuck.
const PROGRESS_WINDOW: f64 = 0.8;
/// Getting closer by less than this is no progress.
const PROGRESS_MIN: f32 = 8.0;
/// How far along the path a walking bot looks.
const LOOK_AHEAD: f32 = 256.0;
/// A point to look at closer than this only spins the view round: the look keeps its heading.
const LOOK_NEAR: f32 = 48.0;
/// Sideways speed counts against the heading this much, so the bot turns onto a link instead of drifting wide.
const DRIFT: f32 = 1.0;
/// A clear way on from short of a node is checked again after this long.
const RECHECK: f64 = 0.25;
/// A straight, level stretch of the path this long is a runway for a long jump.
const RUNWAY: f32 = 400.0;
/// A long jump along the path lands this far ahead; a bold one from this near on, as far as it carries.
const RUNWAY_REACH: [f32; 2] = [300.0, 470.0];
const BOLD_REACH: f32 = 250.0;
/// A bold long jump comes down this far below the takeoff at most.
const BOLD_DROP: f32 = 400.0;
/// The nodes a long jump that is not bold passes over lie this close to its line.
const RUNWAY_CORRIDOR: f32 = 24.0;
/// A long jump goes this close to the way the bot runs (cosine): not bold, bold.
const RUNWAY_HEADING: [f32; 2] = [0.93, 0.87];
/// A runway is looked for this often at most, and not again this soon after a long jump: not bold, bold (one long
/// jump after another).
const RUNWAY_CHECK: [f64; 2] = [0.5, 0.05];
const LEAP_COOLDOWN: [f64; 2] = [1.1, 0.0];
/// How far along the path, and over how many nodes, landings are looked for.
const RUNWAY_SCAN: f32 = 1100.0;
const RUNWAY_NODES: usize = 20;
/// Landings whose flights are followed per look at most; one that did not come down right is not tried again from
/// within this of where it was tried.
const RUNWAY_TRIES: usize = 2;
const RUNWAY_RETRY: f32 = 48.0;
/// A long jump along the path takes off within this of where its flight was followed from, or not at all: a
/// takeoff further along overshoots a near landing, or meets what the flight cleared from there.
const RUNWAY_TAKEOFF: f32 = 32.0;
/// Room a long jump along the path has on each side of its flight: taking off a little off the line or the view a
/// little off, it still gets through.
const RUNWAY_MARGIN: [f32; 2] = [-8.0, 8.0];

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

/// Tricks on the way, for the counts of how they went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrickKind {
    /// A long jump link.
    LongJump,
    /// A long jump along a straight stretch of the way.
    Runway,
    /// A gauss boost link.
    Boost,
    /// A gauss boost onto a node further along the way.
    GaussLeap,
}

impl TrickKind {
    pub const ALL: [TrickKind; 4] = [
        TrickKind::LongJump,
        TrickKind::Runway,
        TrickKind::Boost,
        TrickKind::GaussLeap,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            TrickKind::LongJump => "long jump links",
            TrickKind::Runway => "long jumps on the way",
            TrickKind::Boost => "gauss boost links",
            TrickKind::GaussLeap => "gauss jumps on the way",
        }
    }
}

/// How the tricks that left the ground went: landed where they should, or not, by kind.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TrickCounts {
    pub landed: [u32; 4],
    pub missed: [u32; 4],
}

impl TrickCounts {
    pub fn record(&mut self, kind: TrickKind, landed: bool) {
        let i = kind as usize;
        if landed {
            self.landed[i] += 1;
        } else {
            self.missed[i] += 1;
        }
    }
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

/// A trick taken off the graph's links: a long jump along a straight stretch of the path, a gauss boost toward the
/// goal, or a trick onto a spot off the graph. It lands at node `to`, `path[at]` when that is on the path (off it,
/// the way on is planned again), or at its exit when it lands off the graph.
#[derive(Clone, Debug)]
struct Shortcut {
    spec: TraversalSpec,
    exec: Exec,
    to: Option<NodeId>,
    at: Option<usize>,
    /// Where it starts and where it lands, as nodes.
    from: NavNode,
    exit: NavNode,
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
    /// Heading of the last look along the path, and its tilt (rise per unit across).
    gaze: Option<(Vec2, f32)>,
    /// Whether the way from the bot to the node after the target was clear, for which target and when.
    way_on: Option<(usize, bool, f64)>,
    shortcut: Option<Shortcut>,
    /// When a runway was last looked for, and when a long jump may be taken again.
    runway_at: f64,
    leap_after: f64,
    /// Landings whose flights did not come down right, and where they were tried from.
    leap_misses: smallvec::SmallVec<[(NodeId, Vec3); 4]>,
    /// A trick that left the ground just ended: which, and whether it landed where it should.
    event: Option<(TrickKind, bool)>,
    /// The shortcut just ended: done, or why it failed.
    ended: Option<Result<(), FailReason>>,
}

impl PathFollower {
    /// The rest of the path runs through the link `from → to`.
    pub fn uses(&self, from: NodeId, to: NodeId) -> bool {
        let start = self.next.saturating_sub(1);
        self.path[start.min(self.path.len())..]
            .windows(2)
            .any(|w| w[0] == from && w[1] == to)
    }

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
            gaze: None,
            way_on: None,
            shortcut: None,
            runway_at: f64::NEG_INFINITY,
            leap_after: f64::NEG_INFINITY,
            leap_misses: smallvec::SmallVec::new(),
            event: None,
            ended: None,
        }
    }

    /// A trick that left the ground and ended since the last call: which, and whether it landed where it should.
    pub fn take_event(&mut self) -> Option<(TrickKind, bool)> {
        self.event.take()
    }

    /// How the shortcut under way ended, when it did since the last call.
    pub fn take_ended(&mut self) -> Option<Result<(), FailReason>> {
        self.ended.take()
    }

    pub fn path(&self) -> &[NodeId] {
        &self.path
    }

    /// Index in the path of the node walked to.
    pub fn next_index(&self) -> usize {
        self.next
    }

    /// On a special link or a shortcut: nothing else is to be started on the way now.
    pub fn busy(&self) -> bool {
        self.shortcut.is_some() || self.exec.is_some()
    }

    /// In the air on a long jump or a boost: steered onto its landing until it comes down, whatever else happens.
    pub fn flying(&self) -> bool {
        self.shortcut.as_ref().is_some_and(|sc| sc.exec.flying())
            || self.exec.as_ref().is_some_and(|(_, _, e)| e.flying())
    }

    /// Takes a trick off the graph's links next: from where the bot is, landing at node `to` (`path[at]` when on the
    /// path), or off the graph at the spec's exit without one. `once`: a long jump that is not lined up in time is
    /// given up rather than tried again.
    pub fn take_shortcut(&mut self, spec: TraversalSpec, to: Option<NodeId>, at: Option<usize>, now: f64, once: bool) {
        let mut exec = Exec::new(&spec, now);
        if let Exec::LongJump(e) | Exec::GaussBoost(e) = &mut exec {
            e.once = once;
        }
        let spot = |a: &Anchor| NavNode {
            origin: a.origin,
            flags: NodeFlags::empty(),
            radius: a.radius,
            support: 0,
            first_link: 0,
            link_count: 0,
        };
        self.shortcut = Some(Shortcut {
            exec,
            to,
            at,
            from: spot(&spec.entry),
            exit: spot(&spec.exit),
            spec,
        });
        self.ended = None;
    }

    /// Walks to the path's first node before the rest when the second one is not in a straight line from `origin`
    /// (the first is the node nearest the bot, the second may be round a corner from where the bot stands).
    pub fn check_start(&mut self, g: &NavGraph, s: &NavInput, tracer: &mut dyn Tracer) {
        if self.next == 1 && !clear_way(g, s, self.path[1], tracer) {
            self.next = 0;
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

    /// The link being followed has a traversal contract (not plain walking), or a shortcut is under way.
    pub fn on_special_link(&self, g: &NavGraph) -> bool {
        if self.shortcut.is_some() {
            return true;
        }
        let Some((from, to)) = self.current_link() else {
            return false;
        };
        self.next > 0
            && g.find_link(from, to)
                .is_some_and(|l| !l.kind.is_walk() && g.spec(l).is_some())
    }

    /// What the follower is doing: `walk`, or the executor phase of a special link.
    pub fn phase(&self) -> &'static str {
        if let Some(sc) = &self.shortcut {
            return sc.exec.phase();
        }
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

    fn reached(&mut self, g: &NavGraph, s: &NavInput, kind: LinkKind, tracer: &mut dyn Tracer) -> bool {
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
            return last || flat < 16.0 || self.way_on_clear(g, s, tracer);
        }
        // Passed the node: the plane through it, across the segment we came along.
        if !last && self.next > 0 && dz < 40.0 {
            let from = g.node(self.path[self.next - 1]).origin;
            let seg = (node.origin - from).truncate();
            if seg.length() > 1.0 && seg.dot(-to.truncate()) > 0.0 && flat < 64.0 {
                return self.way_on_clear(g, s, tracer);
            }
        }
        false
    }

    /// Short of the target node: heading on to the node after it from here does not cut a corner. A link that is
    /// not walked starts at its entry, which its executor goes to.
    fn way_on_clear(&mut self, g: &NavGraph, s: &NavInput, tracer: &mut dyn Tracer) -> bool {
        let Some(&after) = self.path.get(self.next + 1) else {
            return true;
        };
        if g.find_link(self.path[self.next], after)
            .is_some_and(|l| !l.kind.is_walk())
        {
            return true;
        }
        if let Some((next, clear, at)) = self.way_on
            && next == self.next
            && s.now - at < RECHECK
        {
            return clear;
        }
        let clear = clear_way(g, s, after, tracer);
        self.way_on = Some((self.next, clear, s.now));
        clear
    }

    /// Where a walking bot looks: `LOOK_AHEAD` along the path, past the small bends between nodes and round a
    /// corner before it gets there, nearly level. The look ends where the path stops being walked (a jump, a
    /// ladder: their executors look for themselves) and where the path turns back more than a right angle.
    fn gaze(&mut self, g: &NavGraph, s: &NavInput) -> Vec3 {
        let eye = s.eye();
        let mut from = s.origin;
        let mut left = LOOK_AHEAD;
        let mut heading: Option<Vec2> = None;
        let mut point = None;
        for k in self.next..self.path.len() {
            let at = g.node(self.path[k]).origin;
            let seg = (at - from).truncate();
            let len = seg.length();
            if len > 1.0 {
                let dir = seg / len;
                match heading {
                    Some(h) if h.dot(dir) < 0.0 => break,
                    Some(_) => {}
                    None => heading = Some(dir),
                }
                if len >= left {
                    point = Some(from.lerp(at, left / len));
                    break;
                }
                left -= len;
            }
            point = Some(at);
            from = at;
            let walked_on = self
                .path
                .get(k + 1)
                .is_none_or(|&after| g.find_link(self.path[k], after).is_some_and(|l| l.kind.is_walk()));
            if !walked_on {
                break;
            }
        }
        if let Some(p) = point {
            let d = travel_look(eye, p + Vec3::Z * EYE_HEIGHT) - eye;
            let across = d.truncate().length();
            if across >= LOOK_NEAR {
                self.gaze = Some((d.truncate() / across, d.z / across));
            }
        }
        self.look_on(s)
    }

    /// A point along the last heading looked in, or ahead of the view when there is none.
    fn look_on(&self, s: &NavInput) -> Vec3 {
        let (dir, tilt) = self.gaze.unwrap_or_else(|| {
            let (forward, _, _) = view_angle_vectors(s.view);
            (forward.truncate().normalize_or(Vec2::X), 0.0)
        });
        s.eye() + dir.extend(tilt) * 128.0
    }

    /// Standing where it is, looking on.
    fn hold(&self, s: &NavInput) -> NavStep {
        NavStep::hold(self.look_on(s))
    }

    /// One frame on the path. `flights`: long jump flights the follower may still follow through the traces this
    /// frame (shared by the bots), taken from as it does.
    pub fn tick(
        &mut self,
        g: &NavGraph,
        s: &NavInput,
        mech: &dyn MechView,
        tracer: &mut dyn Tracer,
        flights: &mut u32,
    ) -> FollowOutput {
        for _ in 0..4 {
            if let Some(out) = self.run_shortcut(g, s, mech, tracer) {
                match out {
                    Some(out) => return out,
                    None => continue,
                }
            }
            let Some((from, to)) = self.current_link() else {
                return FollowOutput {
                    step: self.hold(s),
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
                let (mut step, status) = exec.tick(&mut ctx);
                if step.pitch.is_none() {
                    // Only a direction to go in: look that way, nearly level.
                    step.look_at = travel_look(s.eye(), step.look_at);
                }
                let trick = match l.kind {
                    LinkKind::LongJump => Some(TrickKind::LongJump),
                    LinkKind::GaussBoost => Some(TrickKind::Boost),
                    _ => None,
                };
                if let Some(kind) = trick {
                    match status {
                        ExecStatus::Done => self.event = Some((kind, true)),
                        ExecStatus::Failed(_) if exec.flying() => {
                            self.event = Some((kind, false));
                            missed(kind, s, spec, exec.launch());
                        }
                        _ => {}
                    }
                }
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
            if self.reached(g, s, kind, tracer) {
                self.advance(s.now);
                continue;
            }
            if kind == LinkKind::Walk && self.next > 0 && self.runway(g, s, tracer, flights) {
                continue;
            }
            return self.walk(g, s, mech, tracer, from, to, kind, link.map(|l| l.flags));
        }
        FollowOutput {
            step: self.hold(s),
            status: FollowStatus::Moving,
        }
    }

    /// Runs the shortcut under way: `None` without one, `Some(None)` when it just ended and the path goes on from
    /// here.
    fn run_shortcut(
        &mut self,
        g: &NavGraph,
        s: &NavInput,
        mech: &dyn MechView,
        tracer: &mut dyn Tracer,
    ) -> Option<Option<FollowOutput>> {
        let sc = self.shortcut.as_mut()?;
        let to = sc.to.map_or(sc.exit, |n| *g.node(n));
        let mut ctx = ExecCtx {
            input: s,
            mech,
            tracer: &mut *tracer,
            spec: &sc.spec,
            from: &sc.from,
            to: &to,
        };
        let (mut step, status) = sc.exec.tick(&mut ctx);
        let flying = sc.exec.flying();
        let kind = match sc.spec.action {
            Action::GaussBoost { .. } => TrickKind::GaussLeap,
            _ => TrickKind::Runway,
        };
        // In the air the view is free: along the way on from the landing, lined up for the next long jump.
        if flying && let Some(at) = sc.at {
            step.look_at = along_path(g, &self.path, at, LOOK_AHEAD) + Vec3::Z * EYE_HEIGHT;
        }
        if step.pitch.is_none() {
            step.look_at = travel_look(s.eye(), step.look_at);
        }
        match status {
            ExecStatus::Done => {
                self.event = Some((kind, true));
                self.ended = Some(Ok(()));
                let at = sc.at;
                self.shortcut = None;
                match at {
                    Some(at) => {
                        self.next = at;
                        self.advance(s.now);
                        Some(None)
                    }
                    None => Some(Some(FollowOutput {
                        step,
                        status: FollowStatus::Replan,
                    })),
                }
            }
            ExecStatus::Failed(reason) => {
                tracing::debug!("{} off the path failed: {}", sc.spec.action.name(), reason.as_str());
                if flying {
                    self.event = Some((kind, false));
                    missed(kind, s, &sc.spec, sc.exec.launch());
                }
                self.ended = Some(Err(reason));
                self.shortcut = None;
                // Down somewhere else: the way on is planned from there. Not lined up in time: walk on.
                flying.then_some(Some(FollowOutput {
                    step,
                    status: FollowStatus::Replan,
                }))
            }
            ExecStatus::Running => Some(Some(FollowOutput {
                step,
                status: FollowStatus::Moving,
            })),
            ExecStatus::Waiting => {
                self.last_progress = s.now;
                Some(Some(FollowOutput {
                    step,
                    status: FollowStatus::Waiting,
                }))
            }
        }
    }

    /// A long jump along the path ahead, when the bot may take one: onto the node furthest along the path that the
    /// flight, followed through the server's traces, comes down on without more fall damage than the bot may take.
    /// Not bold: along a straight, level stretch 400 units long at least, onto a node 300–470 units ahead. Bold:
    /// onto any node from 250 units off to as far as the jump carries, round corners, down drops. Takes it as a
    /// shortcut and says so.
    fn runway(&mut self, g: &NavGraph, s: &NavInput, tracer: &mut dyn Tracer, flights: &mut u32) -> bool {
        let t = s.tricks;
        let bold = t.runway_bold;
        let b = usize::from(bold);
        if !(t.longjump && t.runway) || s.now < self.leap_after || s.now - self.runway_at < RUNWAY_CHECK[b] {
            return false;
        }
        if !s.on_ground || s.on_ladder || s.ducked || s.waterlevel > 0 || *flights == 0 {
            return false;
        }
        let v = s.velocity.truncate();
        if v.length() < if bold { LONGJUMP_TAKEOFF } else { 150.0 } {
            return false;
        }
        self.runway_at = s.now;
        let feet = s.feet();
        let unfit =
            NodeFlags::CROUCH | NodeFlags::LADDER | NodeFlags::WATER | NodeFlags::AIRBORNE | NodeFlags::ON_MOVER;
        let (low, high) = if bold { (-BOLD_DROP, 40.0) } else { (-64.0, 40.0) };
        let mut points: smallvec::SmallVec<[Vec3; 24]> = smallvec::SmallVec::new();
        points.push(s.origin);
        let mut along = 0.0;
        let mut landings: smallvec::SmallVec<[usize; 24]> = smallvec::SmallVec::new();
        for k in self.next..self.path.len() {
            let n = g.node(self.path[k]);
            let dz = n.origin.z - 36.0 - feet;
            let flown_over = k == self.next
                || g.find_link(self.path[k - 1], self.path[k]).is_some_and(|l| {
                    !l.flags.contains(LinkFlags::DYNAMIC)
                        && match l.kind {
                            LinkKind::Walk => true,
                            LinkKind::Drop | LinkKind::Jump => bold,
                            _ => false,
                        }
                });
            if n.flags.intersects(unfit) || !(low..=high).contains(&dz) || !flown_over {
                break;
            }
            along += (n.origin - *points.last().unwrap_or(&s.origin)).truncate().length();
            points.push(n.origin);
            let flat = (n.origin - s.origin).truncate().length();
            let reach = if bold {
                BOLD_REACH..=lb_kin::tricks::longjump_reach(dz, s.gravity()).unwrap_or(0.0) - 24.0
            } else {
                RUNWAY_REACH[0]..=RUNWAY_REACH[1]
            };
            // Not onto the start of a link walking does not do: its executor wants the bot there under control.
            let walked_on = self
                .path
                .get(k + 1)
                .is_none_or(|&after| g.find_link(self.path[k], after).is_some_and(|l| l.kind.is_walk()));
            if reach.contains(&flat) && walked_on {
                landings.push(k);
            }
            if along > RUNWAY_SCAN || points.len() > RUNWAY_NODES {
                break;
            }
        }
        if !bold && (along < RUNWAY || points.len() < 3) {
            return false;
        }
        let heading = v.normalize();
        let hurt_allowed = if bold { t.runway_hurt.max(0.0) } else { 0.0 };
        let phys = crate::tricks::physics(s);
        let mut tries = 0;
        // Furthest along the path first.
        for &k in landings.iter().rev() {
            let node = *g.node(self.path[k]);
            let to = node.origin;
            let line = (to - s.origin).truncate();
            if heading.dot(line.normalize_or_zero()) < RUNWAY_HEADING[b] {
                continue;
            }
            let passed = &points[1..=k - self.next];
            let straight = passed.iter().all(|p| {
                let off = (*p - s.origin).truncate();
                (off - line * (off.dot(line) / line.length_squared())).length() <= RUNWAY_CORRIDOR
            });
            let cut = !straight && bold && crate::tricks::leap_line_clear(tracer, s, to, 0.0);
            if !straight && !cut {
                continue;
            }
            if !RUNWAY_MARGIN
                .iter()
                .all(|&side| crate::tricks::leap_line_clear(tracer, s, to, side))
            {
                continue;
            }
            if self
                .leap_misses
                .iter()
                .any(|(n, at)| *n == self.path[k] && at.distance(s.origin) < RUNWAY_RETRY)
            {
                continue;
            }
            if tries == RUNWAY_TRIES || *flights == 0 {
                break;
            }
            tries += 1;
            *flights -= 1;
            let yaw = lb_core::math::dir_to_view_angles(line.extend(0.0)).y;
            let mut world = lb_kin::tricks::Traced(&mut *tracer);
            let v = lb_kin::tricks::longjump_from(&mut world, &phys, crate::tricks::player(s), yaw, Some(to));
            let hurt = phys.fall_damage(v.impact);
            if !v.ok || hurt > hurt_allowed || lb_kin::tricks::lands_in_hazard(&mut world, v.landing) {
                if self.leap_misses.len() == self.leap_misses.inline_size() {
                    self.leap_misses.remove(0);
                }
                self.leap_misses.push((self.path[k], s.origin));
                continue;
            }
            let spec = TraversalSpec {
                entry: Anchor {
                    origin: s.origin,
                    radius: RUNWAY_TAKEOFF,
                    stance: Stance::Stand,
                },
                exit: anchor(&node, 32.0),
                action: Action::LongJump { robustness: 1.0 },
                needs: Needs {
                    health: 0.0,
                    longjump: true,
                    gauss: false,
                },
                deadline: 3.0,
                cost: Cost {
                    time: v.flight,
                    wait: 0.0,
                    damage: hurt,
                },
            };
            self.leap_after = s.now + LEAP_COOLDOWN[b];
            self.take_shortcut(spec, Some(self.path[k]), Some(k), s.now, true);
            return true;
        }
        false
    }

    fn fail(&mut self, s: &NavInput, from: NodeId, to: NodeId, reason: FailReason) -> FollowOutput {
        self.exec = None;
        FollowOutput {
            step: self.hold(s),
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
        let mut step = NavStep::hold(self.gaze(g, s));
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
        step.move_dir = steer(d.normalize_or_zero(), step.speed, s.velocity);
        step.duck = kind == LinkKind::Crouch || node.flags.contains(NodeFlags::CROUCH);

        // Fell off the path: plan again from where it landed, unless the target is still within a walk. Falling
        // while working loose from being stuck on the link is the link's fault.
        if !s.on_ground && !s.on_ladder && s.waterlevel < 2 {
            self.airborne_from.get_or_insert(s.feet());
        } else if let Some(top) = self.airborne_from.take()
            && top - s.feet() > 40.0
            && (node.origin.z - 36.0 - s.feet()).abs() > 40.0
        {
            if self.tried > 0 {
                return self.fail(s, from, to, FailReason::GeometryInvalid);
            }
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

/// A trick that left the ground came down elsewhere: where, and how fast it left the ground, for the stand's log.
fn missed(kind: TrickKind, s: &NavInput, spec: &TraversalSpec, launch: Option<(f32, f32)>) {
    let (a, b) = (spec.entry.origin, spec.exit.origin);
    let (along, up) = launch.unwrap_or_default();
    tracing::info!(
        "{} missed: from {:.0} {:.0} {:.0} for {:.0} {:.0} {:.0} ({:.0} across, {:+.0} up), left the ground at {along:.0} \
         along and {up:.0} up, down at {:.0} {:.0} {:.0}, {:.0} short of it across, {:+.0} up",
        kind.as_str(),
        a.x,
        a.y,
        a.z,
        b.x,
        b.y,
        b.z,
        (b - a).truncate().length(),
        b.z - a.z,
        s.origin.x,
        s.origin.y,
        s.origin.z,
        (b - s.origin).truncate().length(),
        s.origin.z - b.z
    );
}

/// The point `dist` along `path` on from its node `from` (the path's last node when it ends sooner).
fn along_path(g: &NavGraph, path: &[NodeId], from: usize, dist: f32) -> Vec3 {
    let mut at = g.node(path[from]).origin;
    let mut left = dist;
    for &n in &path[from + 1..] {
        let next = g.node(n).origin;
        let len = (next - at).length();
        if len >= left {
            return at.lerp(next, left / len.max(1e-3));
        }
        left -= len;
        at = next;
    }
    at
}

/// Heading `dir` at `speed` with `velocity` now: the way to push so the sideways part of the velocity dies out
/// quicker than friction alone would kill it.
pub fn steer(dir: Vec2, speed: f32, velocity: Vec3) -> Vec2 {
    if dir == Vec2::ZERO {
        return dir;
    }
    let v = velocity.truncate();
    let side = v - dir * v.dot(dir);
    (dir * speed.max(1.0) - side * DRIFT).normalize_or(dir)
}

/// Nothing in the way of walking straight from where the bot stands to node `to` (a step's height up, so stairs do
/// not count).
fn clear_way(g: &NavGraph, s: &NavInput, to: NodeId, tracer: &mut dyn Tracer) -> bool {
    let node = g.node(to);
    let crouch = s.ducked || node.flags.contains(NodeFlags::CROUCH);
    let (hull, half) = if crouch {
        (HullKind::Crouch, 18.0)
    } else {
        (HullKind::Stand, 36.0)
    };
    let node_feet = node.origin.z
        - if node.flags.contains(NodeFlags::CROUCH) {
            18.0
        } else {
            36.0
        };
    let lift = half + crate::validate::STEP_SIZE;
    let from = s.origin.truncate().extend(s.feet() + lift);
    let at = node.origin.truncate().extend(node_feet + lift);
    let tr = tracer.trace(&TraceQuery::hull(from, at, hull));
    !tr.start_solid && tr.fraction >= 1.0
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
            gravity: 800.0,
            ..Default::default()
        }
    }

    #[test]
    fn walks_node_to_node_and_arrives() {
        let g = line_graph(&[LinkKind::Walk, LinkKind::Walk]);
        let mut w = BoxWorld::new();
        let mut f = PathFollower::new(vec![0, 1, 2], 0.0);
        let mut flights = u32::MAX;
        let out = f.tick(&g, &input(0.0, 0.0), &NoMechs, &mut w, &mut flights);
        assert_eq!(out.step.move_dir, Vec2::X);
        assert_eq!(f.target(), Some(1));
        f.tick(&g, &input(90.0, 0.3), &NoMechs, &mut w, &mut flights);
        assert_eq!(f.target(), Some(2), "within the radius of node 1");
        assert_eq!(
            f.tick(&g, &input(195.0, 0.6), &NoMechs, &mut w, &mut flights).status,
            FollowStatus::Arrived
        );
    }

    #[test]
    fn no_progress_escalates_to_a_failure() {
        let g = line_graph(&[LinkKind::Walk]);
        let mut w = BoxWorld::new();
        w.solid(Vec3::new(40.0, -256.0, 0.0), Vec3::new(60.0, 256.0, 200.0));
        let mut f = PathFollower::new(vec![0, 1], 0.0);
        let mut flights = u32::MAX;
        let mut statuses = Vec::new();
        let mut sidestepped = false;
        for i in 0..100 {
            let out = f.tick(&g, &input(0.0, i as f64 * 0.1), &NoMechs, &mut w, &mut flights);
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
