//! `lb nav test`: the obstacle course on a live server. A bot, taken off its behavior, walks to the entry of each
//! chosen special link and carries the link out; every link's outcome is reported.

use std::collections::VecDeque;

use lb_core::Vec3;
use lb_nav::exec::NavInput;
use lb_nav::follow::PathFollower;
use lb_nav::known::FailReason;
use lb_nav::navigator::{NavCtx, Navigator};
use lb_nav::{LinkKind, NavGraph, NodeFlags, NodeId};
use lb_nav_api::{NavStatus, NavStep};

/// Seconds to reach a link's entry, and to carry the link out.
const APPROACH_TIMEOUT: f64 = 30.0;
const TRAVERSE_TIMEOUT: f64 = 30.0;

#[derive(Clone, Debug)]
pub struct LinkResult {
    pub from: NodeId,
    pub to: NodeId,
    pub kind: LinkKind,
    pub outcome: Outcome,
    pub seconds: f64,
    /// Executor phases seen, in order, and links that failed on the way.
    pub phases: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Outcome {
    Done,
    Failed(FailReason),
    /// The traversal did not finish in time.
    Timeout,
    /// The entry could not be reached.
    Unreached,
    Died,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    Approach,
    Traverse,
}

#[derive(Clone, Debug)]
pub struct NavTest {
    queue: VecDeque<(NodeId, NodeId)>,
    current: Option<(NodeId, NodeId, Phase, f64)>,
    phases: Vec<String>,
    failures_seen: u32,
    pub results: Vec<LinkResult>,
    pub total: usize,
    pub finished: bool,
}

/// Valid links of `kind` (all special kinds for `None`) from nodes a bot can stand on, `count` of them spread
/// over the map.
pub fn pick_links(g: &NavGraph, kind: Option<LinkKind>, count: usize) -> Vec<(NodeId, NodeId)> {
    let all: Vec<(NodeId, NodeId)> = (0..g.len() as NodeId)
        .filter(|&a| !g.node(a).flags.intersects(NodeFlags::AIRBORNE | NodeFlags::LADDER))
        .flat_map(|a| {
            g.links(a)
                .iter()
                .filter(|l| l.valid() && kind.map_or(!l.kind.is_walk(), |k| l.kind == k))
                .map(move |l| (a, l.to))
        })
        .collect();
    let step = (all.len() / count.max(1)).max(1);
    all.into_iter().step_by(step).take(count).collect()
}

impl NavTest {
    pub fn new(links: Vec<(NodeId, NodeId)>) -> NavTest {
        NavTest {
            total: links.len(),
            queue: links.into(),
            current: None,
            phases: Vec::new(),
            failures_seen: 0,
            results: Vec::new(),
            finished: false,
        }
    }

    fn finish(&mut self, g: &NavGraph, outcome: Outcome, now: f64) {
        if let Some((a, b, _, since)) = self.current.take() {
            let kind = g.find_link(a, b).map_or(LinkKind::Walk, |l| l.kind);
            tracing::info!(
                "nav test {a} -> {b} ({}): {outcome:?} in {:.1} s",
                kind.as_str(),
                now - since
            );
            self.results.push(LinkResult {
                from: a,
                to: b,
                kind,
                outcome,
                seconds: now - since,
                phases: std::mem::take(&mut self.phases),
            });
        }
    }

    /// The bot died: the link in progress counts as failed, the course goes on after the respawn.
    pub fn on_death(&mut self, g: &NavGraph, now: f64) {
        self.finish(g, Outcome::Died, now);
    }

    /// One frame of the course.
    pub fn step(&mut self, nav: &mut Navigator, ctx: &mut NavCtx<'_>, input: &NavInput) -> Option<NavStep> {
        let now = input.now;
        let g = ctx.graph;
        if self.current.is_none() {
            match self.queue.pop_front() {
                Some((a, b)) => {
                    // Every link on its own: nothing learned from the previous ones.
                    nav.clear();
                    self.failures_seen = nav.failures_total;
                    self.current = Some((a, b, Phase::Approach, now));
                }
                None => {
                    self.finished = true;
                    return None;
                }
            }
        }
        let (a, b, phase, since) = self.current?;
        match phase {
            Phase::Approach => {
                let entry = g.node(a).origin;
                // Standing there: a bot still in the air from the way here would start the link mid-flight.
                let there = (entry - input.origin).truncate().length() < 32.0
                    && (entry.z - input.origin.z).abs() < 40.0
                    && (input.on_ground || input.on_ladder);
                if there {
                    nav.follower = Some(PathFollower::new(vec![a, b], now));
                    nav.goal = Some(b);
                    self.failures_seen = nav.failures_total;
                    self.current = Some((a, b, Phase::Traverse, now));
                    return Some(NavStep::hold(input.eye() + Vec3::X));
                }
                if now - since > APPROACH_TIMEOUT {
                    self.finish(g, Outcome::Unreached, now);
                    return None;
                }
                let (status, step) = nav.go_to(ctx, input, entry);
                if nav.failures_total > self.failures_seen {
                    self.failures_seen = nav.failures_total;
                    if let Some(f) = nav.last_failure {
                        self.phases
                            .push(format!("on the way {}->{} {}", f.from, f.to, f.reason.as_str()));
                    }
                }
                if status == NavStatus::NoPath && now - since > 5.0 {
                    self.finish(g, Outcome::Unreached, now);
                }
                step
            }
            Phase::Traverse => {
                let dest = g.node(b).origin;
                let (status, step) = nav.go_to(ctx, input, dest);
                let phase = nav.phase();
                if self.phases.last().map(String::as_str) != Some(phase) && phase != "idle" {
                    self.phases.push(phase.to_string());
                }
                if nav.failures_total > self.failures_seen {
                    let reason = nav.last_failure.map_or(FailReason::ControllerFailure, |f| f.reason);
                    self.finish(g, Outcome::Failed(reason), now);
                } else if status == NavStatus::Arrived {
                    self.finish(g, Outcome::Done, now);
                } else if now - since > TRAVERSE_TIMEOUT {
                    self.finish(g, Outcome::Timeout, now);
                }
                step
            }
        }
    }

    pub fn report(&self) -> Vec<String> {
        let mut out = Vec::new();
        for kind in LinkKind::ALL {
            let of: Vec<&LinkResult> = self.results.iter().filter(|r| r.kind == kind).collect();
            if of.is_empty() {
                continue;
            }
            let done = of.iter().filter(|r| r.outcome == Outcome::Done).count();
            let unreached = of.iter().filter(|r| r.outcome == Outcome::Unreached).count();
            let tried = of.len() - unreached;
            let mean = of
                .iter()
                .filter(|r| r.outcome == Outcome::Done)
                .map(|r| r.seconds)
                .sum::<f64>()
                / done.max(1) as f64;
            out.push(format!(
                "{:<9} {done}/{tried} done, mean {mean:.1} s{}",
                kind.as_str(),
                if unreached > 0 {
                    format!(", {unreached} not reached")
                } else {
                    String::new()
                }
            ));
        }
        for r in self.results.iter().filter(|r| r.outcome != Outcome::Done) {
            out.push(format!(
                "  {} -> {} {}: {:?} after {:.1} s, phases {}",
                r.from,
                r.to,
                r.kind.as_str(),
                r.outcome,
                r.seconds,
                r.phases.join(" ")
            ));
        }
        out.push(format!(
            "{} of {} links run{}",
            self.results.len(),
            self.total,
            if self.finished { ", finished" } else { "" }
        ));
        out
    }
}
