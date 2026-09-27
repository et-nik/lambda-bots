//! One bot's navigation: planning paths to where behavior wants to go, following them, learning from failed links
//! and noticing when the bot is stuck for good. The same code runs on the server and in the offline test course.

use lb_core::Vec3;
use lb_core::rng::Pcg32;
use lb_nav_api::{NavStatus, NavStep};
use lb_worldq::{HullKind, TraceQuery, Tracer};

use crate::exec::{EYE_HEIGHT, MechView, NavInput};
use crate::follow::{FollowStatus, PathFollower, steer};
use crate::graph::{NavGraph, NodeFlags, NodeId};
use crate::known::{FailReason, KnownChanges, LinkHealth};
use crate::plan::{Search, SearchStep};

/// Navigation calls this far apart mean the bot did something else meanwhile (fought): the stuck clock restarts.
const NAV_GAP: f64 = 0.5;
const REPLAN_EVERY: f64 = 0.5;
const GIVE_UP_AFTER: u32 = 3;
const DIRECT_WALK: f32 = 200.0;
const DIRECT_RECHECK: f64 = 0.1;

/// What navigating needs besides the bot's own state.
pub struct NavCtx<'a> {
    pub graph: &'a NavGraph,
    pub tracer: &'a mut dyn Tracer,
    pub mech: &'a dyn MechView,
    /// Links switched off for every bot; geometry failures are reported here.
    pub health: Option<&'a mut LinkHealth>,
    /// Who reports failures to `health`.
    pub bot: u32,
    /// Node expansions path searches may still spend this frame (shared by the bots); `None` = no limit.
    pub budget: Option<&'a mut u32>,
}

/// The last link that failed, for diagnostics.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Failure {
    pub from: NodeId,
    pub to: NodeId,
    pub reason: FailReason,
    pub at: f64,
}

#[derive(Default)]
pub struct Navigator {
    pub follower: Option<PathFollower>,
    pub goal: Option<NodeId>,
    pub known: KnownChanges,
    pub last_failure: Option<Failure>,
    /// Links that failed since the bot spawned.
    pub failures_total: u32,
    next_goal_at: f64,
    next_plan_at: f64,
    failures: u32,
    /// Where the bot was when it last moved noticeably, for the irrecoverably-stuck check.
    anchor: Vec3,
    anchor_at: f64,
    last_used: f64,
    /// Waiting for a mechanism on the last call: not stuck.
    waiting: bool,
    /// A destination walked to straight, whether the way was clear, and when that was checked.
    direct: Option<(Vec3, bool, f64)>,
    /// The graph node nearest to the last destination.
    dest_node: Option<(Vec3, NodeId)>,
    /// A path search that ran out of this frame's budget.
    search: Option<Search>,
}

impl Navigator {
    /// Forgets the path and the stuck clock (death, spawn); learned failures stay.
    pub fn reset(&mut self) {
        let known = std::mem::take(&mut self.known);
        let failures_total = self.failures_total;
        *self = Navigator::default();
        self.known = known;
        self.failures_total = failures_total;
    }

    /// Forgets everything, learned failures included (map change).
    pub fn clear(&mut self) {
        *self = Navigator::default();
    }

    /// Seconds of navigating without moving 48 u away from where the bot last was; waiting for a lift or a door
    /// does not count.
    pub fn stuck_for(&mut self, origin: Vec3, now: f64) -> f64 {
        if self.anchor_at == 0.0
            || self.waiting
            || origin.distance(self.anchor) > 48.0
            || now - self.last_used > NAV_GAP
        {
            self.anchor = origin;
            self.anchor_at = now;
        }
        self.last_used = now;
        now - self.anchor_at
    }

    /// What the follower is doing, for diagnostics.
    pub fn phase(&self) -> &'static str {
        self.follower.as_ref().map_or("idle", |f| f.phase())
    }

    /// Nearest node the bot can reach in a straight line.
    fn start_node(ctx: &mut NavCtx<'_>, origin: Vec3) -> Option<NodeId> {
        for (id, _) in ctx.graph.nearest(origin, 512.0, 6) {
            let node = ctx.graph.node(id);
            let tr = ctx
                .tracer
                .trace(&TraceQuery::hull(origin, node.origin, HullKind::Crouch));
            if !tr.start_solid && tr.fraction > 0.97 {
                return Some(id);
            }
        }
        ctx.graph.nearest(origin, 256.0, 1).first().map(|(id, _)| *id)
    }

    fn pick_goal(ctx: &mut NavCtx<'_>, origin: Vec3, rng: &mut Pcg32) -> Option<NodeId> {
        let g = ctx.graph;
        let goals: Vec<NodeId> = (0..g.len() as NodeId)
            .filter(|&id| {
                g.node(id)
                    .flags
                    .intersects(NodeFlags::GOAL | NodeFlags::CAMP | NodeFlags::SNIPER)
            })
            .collect();
        for _ in 0..8 {
            let id = if !goals.is_empty() && rng.chance(60.0) {
                goals[rng.range_i32(0, goals.len() as i32 - 1) as usize]
            } else {
                rng.range_i32(0, g.len() as i32 - 1) as NodeId
            };
            let node = g.node(id);
            let unfit = NodeFlags::LADDER | NodeFlags::AIRBORNE | NodeFlags::WATER | NodeFlags::ON_MOVER;
            if node.origin.distance(origin) > 400.0 && !node.flags.intersects(unfit) {
                return Some(id);
            }
        }
        None
    }

    /// Searches a path to `goal` within this frame's budget: `Some(true)` once the bot follows one, `Some(false)`
    /// when there is none, `None` while the search goes on (it resumes on the next call).
    fn plan(&mut self, ctx: &mut NavCtx<'_>, input: &NavInput, goal: NodeId) -> Option<bool> {
        let (origin, now) = (input.origin, input.now);
        if self.search.as_ref().is_none_or(|s| s.goal() != goal) {
            let Some(start) = Self::start_node(ctx, origin) else {
                return Some(false);
            };
            self.search = Some(Search::new(ctx.graph, ctx.graph.alt.as_deref(), start, goal));
        }
        self.known.expire(now);
        let known = &self.known;
        let health = ctx.health.as_deref();
        let penalty = |a: NodeId, b: NodeId| {
            if health.is_some_and(|h| h.disabled(a, b)) {
                return f32::INFINITY;
            }
            known.penalty(a, b, now)
        };
        let mut unlimited = u32::MAX;
        let budget = match ctx.budget.as_deref_mut() {
            Some(b) => b,
            None => &mut unlimited,
        };
        let search = self.search.as_mut()?;
        match search.run(ctx.graph, ctx.graph.alt.as_deref(), &penalty, budget) {
            SearchStep::Pending => None,
            SearchStep::Found(path) => {
                self.search = None;
                let mut follower = PathFollower::new(path, now);
                follower.check_start(ctx.graph, input, &mut *ctx.tracer);
                self.follower = Some(follower);
                self.goal = Some(goal);
                Some(true)
            }
            SearchStep::NoPath => {
                self.search = None;
                Some(false)
            }
        }
    }

    /// A link failed: remember it and drop the path.
    fn failed(&mut self, ctx: &mut NavCtx<'_>, from: NodeId, to: NodeId, reason: FailReason, now: f64) {
        self.known.fail(from, to, reason, now);
        if reason == FailReason::GeometryInvalid
            && let Some(h) = ctx.health.as_deref_mut()
            && h.report(from, to, ctx.bot, now)
        {
            tracing::warn!("link {from} -> {to} fails for several bots; switched off until checked again");
        }
        tracing::debug!("bot {} failed link {from} -> {to}: {}", ctx.bot, reason.as_str());
        self.last_failure = Some(Failure {
            from,
            to,
            reason,
            at: now,
        });
        self.failures_total += 1;
        self.follower = None;
        self.failures += 1;
    }

    /// Walks toward `dest`: a path to the graph node nearest to it, the last stretch straight when it is clear.
    pub fn go_to(&mut self, ctx: &mut NavCtx<'_>, input: &NavInput, dest: Vec3) -> (NavStatus, Option<NavStep>) {
        let now = input.now;
        self.waiting = false;
        let to = dest - input.origin;
        let flat = to.truncate().length();
        let standing = input.on_ground || input.on_ladder || input.waterlevel >= 2;
        if flat < 32.0 && to.z.abs() < 48.0 && standing {
            self.follower = None;
            return (NavStatus::Arrived, None);
        }
        let on_special = self.follower.as_ref().is_some_and(|f| f.on_special_link(ctx.graph));
        if flat < DIRECT_WALK && to.z.abs() < 32.0 && !on_special {
            let fresh = self
                .direct
                .filter(|(d, _, at)| d.distance(dest) < 16.0 && now - at < DIRECT_RECHECK);
            let clear = match fresh {
                Some((_, clear, _)) => clear,
                None => {
                    let clear = straight_clear(ctx.tracer, input, dest);
                    self.direct = Some((dest, clear, now));
                    clear
                }
            };
            if clear {
                self.follower = None;
                return (NavStatus::Moving, Some(straight(input, dest)));
            }
        }
        let cached = self.dest_node.filter(|(d, _)| d.distance(dest) < 32.0).map(|(_, n)| n);
        let Some(goal) = cached.or_else(|| ctx.graph.nearest(dest, 400.0, 1).first().map(|(n, _)| *n)) else {
            return (NavStatus::NoPath, None);
        };
        if cached.is_none() {
            self.dest_node = Some((dest, goal));
        }
        // A search that ran out of budget goes on every frame; a new one at most every `REPLAN_EVERY`.
        let want = self.follower.is_none() || self.goal != Some(goal);
        if want && (self.search.is_some() || now >= self.next_plan_at) {
            if self.search.is_none() {
                self.next_plan_at = now + REPLAN_EVERY;
            }
            match self.plan(ctx, input, goal) {
                None if self.follower.is_none() => return (NavStatus::Moving, None),
                None | Some(true) => {}
                Some(false) => {
                    self.failures += 1;
                    let status = if self.failures >= GIVE_UP_AFTER {
                        self.failures = 0;
                        NavStatus::NoPath
                    } else {
                        NavStatus::Moving
                    };
                    return (status, None);
                }
            }
        }
        let Some(follower) = self.follower.as_mut() else {
            return (NavStatus::Moving, None);
        };
        let out = follower.tick(ctx.graph, input, ctx.mech, &mut *ctx.tracer);
        match out.status {
            FollowStatus::Moving => (NavStatus::Moving, Some(out.step)),
            FollowStatus::Waiting => {
                self.waiting = true;
                (NavStatus::Moving, Some(out.step))
            }
            FollowStatus::Arrived => {
                // At the node closest to the destination: as close as the graph gets.
                self.follower = None;
                self.failures = 0;
                (NavStatus::Arrived, Some(out.step))
            }
            FollowStatus::Replan => {
                self.follower = None;
                self.next_plan_at = now;
                (NavStatus::Moving, Some(out.step))
            }
            FollowStatus::Failed { from, to, reason } => {
                self.failed(ctx, from, to, reason, now);
                if self.failures >= GIVE_UP_AFTER {
                    self.failures = 0;
                    return (NavStatus::NoPath, Some(out.step));
                }
                (NavStatus::Moving, Some(out.step))
            }
        }
    }

    /// A node to fall back to: well away from `threat`, not too far from the bot.
    pub fn away_from(ctx: &NavCtx<'_>, origin: Vec3, threat: Vec3) -> Option<Vec3> {
        let here = origin.distance(threat);
        ctx.graph
            .nearest(origin, 1200.0, 64)
            .into_iter()
            .map(|(id, _)| ctx.graph.node(id))
            .filter(|n| {
                !n.flags
                    .intersects(NodeFlags::LADDER | NodeFlags::AIRBORNE | NodeFlags::WATER)
            })
            .map(|n| n.origin)
            .filter(|p| p.distance(threat) > here + 200.0)
            .max_by(|a, b| {
                let score = |p: &Vec3| p.distance(threat) - 0.5 * p.distance(origin);
                score(a).total_cmp(&score(b))
            })
    }

    /// Roaming: walk to a goal, pause, pick the next one. Returns what to do this frame.
    pub fn roam(&mut self, ctx: &mut NavCtx<'_>, input: &NavInput, rng: &mut Pcg32) -> Option<NavStep> {
        let now = input.now;
        self.waiting = false;
        if self.follower.is_none() {
            let goal = match self.search.as_ref() {
                Some(s) => s.goal(),
                None => {
                    if now < self.next_goal_at {
                        return None;
                    }
                    self.next_goal_at = now + 1.0;
                    Self::pick_goal(ctx, input.origin, rng)?
                }
            };
            if self.plan(ctx, input, goal) != Some(true) {
                return None;
            }
        }
        let follower = self.follower.as_mut()?;
        let out = follower.tick(ctx.graph, input, ctx.mech, &mut *ctx.tracer);
        match out.status {
            FollowStatus::Moving => {}
            FollowStatus::Waiting => self.waiting = true,
            FollowStatus::Arrived => {
                self.follower = None;
                self.failures = 0;
                self.next_goal_at = now + f64::from(rng.range_f32(0.2, 1.5));
            }
            FollowStatus::Replan => {
                self.follower = None;
                if let Some(goal) = self.goal {
                    self.plan(ctx, input, goal);
                }
            }
            FollowStatus::Failed { from, to, reason } => {
                self.failed(ctx, from, to, reason, now);
                let goal = self.goal.filter(|_| self.failures < GIVE_UP_AFTER);
                if let Some(goal) = goal {
                    self.plan(ctx, input, goal);
                } else {
                    self.failures = 0;
                    self.next_goal_at = now + 0.5;
                }
            }
        }
        Some(out.step)
    }
}

/// Nothing in the way of walking straight to `dest` and floor under the way (no gap to fall into).
fn straight_clear(tracer: &mut dyn Tracer, input: &NavInput, dest: Vec3) -> bool {
    let level = Vec3::new(dest.x, dest.y, input.origin.z);
    let tr = tracer.trace(&TraceQuery::hull(input.origin, level, HullKind::Stand));
    if tr.start_solid || tr.fraction < 1.0 {
        return false;
    }
    let feet = input.feet();
    [0.33f32, 0.66, 1.0].iter().all(|t| {
        let p = input.origin.lerp(level, *t);
        let down = tracer.trace(&TraceQuery::line(p, Vec3::new(p.x, p.y, feet - 40.0)));
        down.fraction < 1.0 && down.normal.z >= 0.7
    })
}

/// A step straight at `dest`.
fn straight(input: &NavInput, dest: Vec3) -> NavStep {
    let mut step = NavStep::hold(Vec3::new(dest.x, dest.y, input.origin.z + EYE_HEIGHT));
    step.speed = input.max_speed;
    step.move_dir = steer(
        (dest - input.origin).truncate().normalize_or_zero(),
        step.speed,
        input.velocity,
    );
    step
}
