//! Getting a bot to a spot a test names (`lb do … go`, `lb test`, `lb-cli nav try`). The spot is where the bot is to
//! stand, and the bot has got there when it stands there, not at the graph node nearest the spot. The way is along
//! the graph to a node with a clear straight walk to the spot, then straight on. When the graph has no such node the
//! bot can get to, a trick is looked for on the spot from the nodes it can get to, the cheapest first: a jump, a long
//! jump (with the module) or a gauss boost charged for the push it takes, landing off the graph. The search follows
//! flights through the map's BSP a few checks a frame. What happens goes into a report: the plan, the trick and where
//! it came down, the links that failed on the way, and how the attempt ended.

use lb_core::Vec3;
use lb_kin::tricks::{boost_tries, boost_view, check_boost, plan_longjump};
use lb_kin::validate::{plan_jump, simulate_walk};
use lb_kin::{MoveWorld, Physics};
use lb_nav_api::{NavStatus, NavStep};
use lb_worldq::{HullKind, TraceQuery, Tracer};
use serde::Serialize;
use smallvec::SmallVec;

use crate::classify::{BOOST_HEALTH, BOOST_HEALTH_AFTER, BOOST_PRICE, BOOST_SETUP, DROP_RESERVE, LONGJUMP_SETUP};
use crate::exec::{NavInput, boost_charge};
use crate::graph::{NavGraph, NodeFlags, NodeId};
use crate::known::FailReason;
use crate::navigator::{NavCtx, Navigator, link_penalty, straight};
use crate::plan::{RUN_SPEED, costs_from, path_back};
use crate::spec::{Action, Anchor, Cost, Needs, Stance, TraversalSpec};
use crate::tricks::{beam_safe, boost_eye, physics};

/// How near the spot the bot must stand by default (flat), and how far its feet may be above or below the spot's.
pub const RADIUS: f32 = 32.0;
const ARRIVE_DZ: f32 = 24.0;
/// Nodes this near the spot, and this far above or below it at most, with a clear walk to it lead there; the
/// nearest few are checked.
const ANCHOR_NEAR: f32 = 320.0;
const ANCHOR_DZ: f32 = 48.0;
const ANCHORS: usize = 8;
/// A trick's takeoff is this far from the spot at most (flat) and this much lower at most: a jump, a long jump, a
/// gauss boost.
const TRICK_REACH: [f32; 3] = [320.0, 700.0, 1600.0];
const TRICK_RISE: [f32; 3] = [64.0, 48.0, 640.0];
/// Seconds to line up a jump at its takeoff, and to get the gauss out and turn round for a boost (the report's plan).
const JUMP_SETUP: f32 = 0.3;
const BOOST_READY: f32 = 1.0;
/// A boost's takeoff is priced a second for this many units to the spot: nearer ones are tried first.
const BOOST_PRICE_SPEED: f32 = 250.0;
/// A long jump's takeoff is this far from the spot at least (nearer, a jump does).
const LONGJUMP_NEAR: f32 = 200.0;
/// Takeoffs tried at most, the cheapest first, and pitches of a boost tried from each.
const TAKEOFFS: usize = 48;
const BOOST_TRIES: usize = 4;
/// A trick that came down this near the spot, on its floor, walks the rest.
const LANDING_SLACK: f32 = 96.0;
/// The last straight stretch gives up after this long without getting nearer.
const STALL: f64 = 2.0;
/// Checks of the trick search per frame on the server: each follows up to a few dozen flights through the BSP.
pub const CHECKS_PER_FRAME: u32 = 1;
/// Nodes a bot cannot stand still on to set off, or to walk off to the spot from.
const UNFIT: NodeFlags = NodeFlags::LADDER
    .union(NodeFlags::WATER)
    .union(NodeFlags::AIRBORNE)
    .union(NodeFlags::ON_MOVER);

/// The tricks an attempt may use, when the bot can too (the module, the gauss and a charge's uranium).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Allowed {
    pub jump: bool,
    pub longjump: bool,
    pub gauss: bool,
}

impl Default for Allowed {
    fn default() -> Allowed {
        Allowed {
            jump: true,
            longjump: true,
            gauss: true,
        }
    }
}

impl Allowed {
    pub const NONE: Allowed = Allowed {
        jump: false,
        longjump: false,
        gauss: false,
    };

    /// `any`, `none`, or the tricks by name: `jump`, `longjump`, `gauss`.
    pub fn parse<S: AsRef<str>>(words: &[S]) -> Result<Allowed, String> {
        let mut a = Allowed::NONE;
        for w in words {
            match w.as_ref() {
                "any" | "all" => a = Allowed::default(),
                "none" => a = Allowed::NONE,
                "jump" => a.jump = true,
                "longjump" | "lj" => a.longjump = true,
                "gauss" | "gauss_boost" | "boost" => a.gauss = true,
                other => return Err(format!("unknown trick `{other}` (jump, longjump, gauss, any, none)")),
            }
        }
        Ok(a)
    }

    pub fn names(&self) -> String {
        let names: Vec<&str> = [(self.jump, "jump"), (self.longjump, "longjump"), (self.gauss, "gauss")]
            .into_iter()
            .filter_map(|(on, n)| on.then_some(n))
            .collect();
        if names.is_empty() {
            "none".into()
        } else {
            names.join(" ")
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Trick {
    Jump,
    LongJump,
    GaussBoost,
}

impl Trick {
    pub fn as_str(self) -> &'static str {
        match self {
            Trick::Jump => "jump",
            Trick::LongJump => "long jump",
            Trick::GaussBoost => "gauss boost",
        }
    }
}

/// The trick chosen to get onto the spot, as planned.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct TrickChoice {
    pub trick: Trick,
    /// The node it sets off from, and where.
    pub from: NodeId,
    pub takeoff: Vec3,
    /// A boost's look down, recoil (units/s) and charge (seconds).
    pub pitch: f32,
    pub push: f32,
    pub charge: f32,
    /// A jump's run-up speed, and whether it ducks in the air.
    pub speed: f32,
    pub duck: bool,
    pub flight: f32,
    pub robustness: f32,
    /// Health the landing costs.
    pub damage: f32,
    /// Seconds it takes from the takeoff: lining up, a boost's gauss out and charged, the flight.
    pub seconds: f32,
}

impl TrickChoice {
    pub fn describe(&self) -> String {
        let how = match self.trick {
            Trick::Jump => format!("run-up {:.0}{}", self.speed, if self.duck { ", ducking" } else { "" }),
            Trick::LongJump => String::new(),
            Trick::GaussBoost => format!(
                "pitch {:.0}, push {:.0} (charge {:.2} s)",
                self.pitch, self.push, self.charge
            ),
        };
        format!(
            "{} from node {} ({:.0} {:.0} {:.0}){}{how}, flight {:.2} s, robustness {:.2}{}",
            self.trick.as_str(),
            self.from,
            self.takeoff.x,
            self.takeoff.y,
            self.takeoff.z,
            if how.is_empty() { "" } else { ": " },
            self.flight,
            self.robustness,
            if self.damage > 0.0 {
                format!(", lands for {:.0} damage", self.damage)
            } else {
                String::new()
            }
        )
    }
}

/// How an attempt ended.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum Outcome {
    Arrived,
    /// No way along the graph and no trick onto the spot.
    NoWay {
        why: String,
    },
    /// The way along the graph broke.
    Stuck {
        why: String,
    },
    /// The trick came down elsewhere.
    Missed {
        landing: Vec3,
        off: f32,
        why: String,
    },
    /// The last straight stretch got no nearer.
    Blocked {
        off: f32,
    },
    Timeout {
        phase: String,
        off: f32,
    },
    Died,
}

impl Outcome {
    pub fn arrived(&self) -> bool {
        matches!(self, Outcome::Arrived)
    }

    pub fn name(&self) -> &'static str {
        match self {
            Outcome::Arrived => "arrived",
            Outcome::NoWay { .. } => "no way",
            Outcome::Stuck { .. } => "stuck",
            Outcome::Missed { .. } => "missed",
            Outcome::Blocked { .. } => "blocked",
            Outcome::Timeout { .. } => "timeout",
            Outcome::Died => "died",
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Outcome::Arrived => "arrived".into(),
            Outcome::NoWay { why } => format!("no way: {why}"),
            Outcome::Stuck { why } => format!("stuck on the way: {why}"),
            Outcome::Missed { landing, off, why } => format!(
                "the trick missed: came down at {:.0} {:.0} {:.0}, {off:.0} u off{}",
                landing.x,
                landing.y,
                landing.z,
                if why.is_empty() {
                    String::new()
                } else {
                    format!(" ({why})")
                }
            ),
            Outcome::Blocked { off } => format!("the last {off:.0} u straight got no nearer"),
            Outcome::Timeout { phase, off } => format!("out of time {off:.0} u off ({phase})"),
            Outcome::Died => "died".into(),
        }
    }
}

/// A link that failed on the way, with where its ends stand (node numbers belong to one graph).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LinkFailure {
    pub t: f64,
    pub from: NodeId,
    pub to: NodeId,
    pub from_at: Vec3,
    pub to_at: Vec3,
    pub kind: String,
    pub reason: String,
}

/// What the trick search went through.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct SearchStats {
    pub takeoffs: u32,
    pub jumps: u32,
    pub longjumps: u32,
    pub boosts: u32,
}

/// Something that happened, seconds from the start.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Event {
    pub t: f64,
    pub what: String,
}

/// What an attempt planned and what came of it.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Report {
    /// Where the bot was to stand.
    pub spot: Vec3,
    /// Seconds the plan reckoned: along the graph, the trick and the last stretch.
    pub planned: Option<f32>,
    /// The planned way along the graph, and its links by kind.
    pub path: Vec<NodeId>,
    pub links: Vec<(String, u32)>,
    pub trick: Option<TrickChoice>,
    /// Where the trick came down.
    pub landing: Option<Vec3>,
    pub failures: Vec<LinkFailure>,
    pub search: SearchStats,
    pub events: Vec<Event>,
    pub seconds: f64,
    /// Where the bot was at the end, and how far from the spot (flat).
    pub end: Vec3,
    pub off: f32,
}

/// What follows the way along the graph.
#[derive(Clone, Copy, Debug)]
enum Then {
    Walk,
    Trick(TrickChoice, TraversalSpec),
}

enum State {
    Plan,
    Search(Box<Search>),
    Travel { node: NodeId, then: Then },
    Trick,
    Final { best: f32, since: f64 },
    Done(Outcome),
}

/// One attempt to get a bot to a spot.
pub struct Reach {
    /// Where the bot is to stand (the origin of a standing player).
    pub spot: Vec3,
    pub radius: f32,
    pub allowed: Allowed,
    /// Seconds the attempt may take.
    pub timeout: f64,
    /// Checks of the trick search a frame: as many as it takes offline, a few on the server (`CHECKS_PER_FRAME`).
    pub checks: u32,
    started: f64,
    state: State,
    report: Report,
    failures_seen: u32,
}

impl Reach {
    pub fn new(spot: Vec3, radius: f32, allowed: Allowed, timeout: f64, now: f64) -> Reach {
        Reach {
            spot,
            radius,
            allowed,
            timeout,
            checks: u32::MAX,
            started: now,
            state: State::Plan,
            report: Report {
                spot,
                ..Report::default()
            },
            failures_seen: u32::MAX,
        }
    }

    pub fn outcome(&self) -> Option<&Outcome> {
        match &self.state {
            State::Done(o) => Some(o),
            _ => None,
        }
    }

    pub fn report(&self) -> &Report {
        &self.report
    }

    /// What the attempt is doing, for status lines.
    pub fn phase(&self, nav: &Navigator) -> String {
        match &self.state {
            State::Plan => "planning".into(),
            State::Search(s) => format!("looking for a trick ({} takeoffs)", s.stats.takeoffs),
            State::Travel { node, then } => format!(
                "to node {node}{}",
                match then {
                    Then::Walk => "",
                    Then::Trick(..) => " for the trick",
                }
            ),
            State::Trick => format!("trick: {}", nav.phase()),
            State::Final { .. } => "the last stretch".into(),
            State::Done(o) => o.name().into(),
        }
    }

    /// The bot died: the attempt is over.
    pub fn died(&mut self, input: &NavInput) {
        self.end(Outcome::Died, input);
    }

    /// Ends the attempt with `outcome`, unless it is over already: it cannot go on, or it was called off.
    pub fn end(&mut self, outcome: Outcome, input: &NavInput) {
        if self.outcome().is_none() {
            self.finish(outcome, input);
        }
    }

    /// Standing on the spot.
    pub fn arrived(&self, input: &NavInput) -> bool {
        let standing = input.on_ground || input.on_ladder || input.waterlevel >= 2;
        standing
            && (self.spot - input.origin).truncate().length() <= self.radius
            && (input.feet() - (self.spot.z - 36.0)).abs() <= ARRIVE_DZ
    }

    fn event(&mut self, now: f64, what: String) {
        tracing::debug!("reach: {what}");
        self.report.events.push(Event {
            t: now - self.started,
            what,
        });
    }

    fn finish(&mut self, outcome: Outcome, input: &NavInput) {
        let now = input.now;
        self.report.seconds = now - self.started;
        self.report.end = input.origin;
        self.report.off = (self.spot - input.origin).truncate().length();
        self.event(now, outcome.describe());
        self.state = State::Done(outcome);
    }

    /// Links that failed on the way since the last frame.
    fn note_failures(&mut self, nav: &Navigator, graph: &NavGraph, now: f64) {
        if self.failures_seen == u32::MAX {
            self.failures_seen = nav.failures_total;
        }
        if nav.failures_total <= self.failures_seen {
            return;
        }
        self.failures_seen = nav.failures_total;
        let Some(f) = nav.last_failure else {
            return;
        };
        let kind = graph.find_link(f.from, f.to).map_or("?", |l| l.kind.as_str());
        self.report.failures.push(LinkFailure {
            t: now - self.started,
            from: f.from,
            to: f.to,
            from_at: graph.node(f.from).origin,
            to_at: graph.node(f.to).origin,
            kind: kind.into(),
            reason: f.reason.as_str().into(),
        });
        self.event(
            now,
            format!("link {} -> {} ({kind}) failed: {}", f.from, f.to, f.reason.as_str()),
        );
    }

    /// One frame of the attempt: what the bot does, `None` once it is over (see `outcome`). `world` is the map as
    /// the offline checks see it (the BSP with its mechanisms at rest): the trick search follows flights through it.
    pub fn step(
        &mut self,
        nav: &mut Navigator,
        ctx: &mut NavCtx<'_>,
        world: &mut dyn MoveWorld,
        input: &NavInput,
    ) -> Option<NavStep> {
        let now = input.now;
        if self.outcome().is_some() {
            return None;
        }
        self.note_failures(nav, ctx.graph, now);
        let flying = matches!(self.state, State::Trick) && nav.follower.as_ref().is_some_and(|f| f.flying());
        if !flying && self.arrived(input) {
            self.finish(Outcome::Arrived, input);
            return None;
        }
        if now - self.started > self.timeout {
            let phase = self.phase(nav);
            let off = (self.spot - input.origin).truncate().length();
            self.finish(Outcome::Timeout { phase, off }, input);
            return None;
        }
        let hold = NavStep::hold(input.eye() + (self.spot - input.origin).truncate().normalize_or_zero().extend(0.0));
        match std::mem::replace(&mut self.state, State::Plan) {
            State::Plan => match self.plan(nav, ctx, world, input) {
                State::Done(o) => {
                    self.finish(o, input);
                    None
                }
                next => {
                    self.state = next;
                    Some(hold)
                }
            },
            State::Search(mut s) => {
                let mut budget = self.checks;
                let found = s.step(world, ctx.graph, &physics(input), input, self.allowed, &mut budget);
                self.report.search = s.stats;
                match found {
                    None => self.state = State::Search(s),
                    Some(Some((choice, spec))) => {
                        let path = path_back(&s.came, &s.costs, choice.from).unwrap_or_default();
                        let along = s.costs[choice.from as usize];
                        self.report.planned = Some(along + choice.seconds);
                        self.planned_path(ctx.graph, path);
                        self.report.trick = Some(choice);
                        let what = format!(
                            "plan: {} links to node {} ({:.1} s), then the {}",
                            self.report.path.len().saturating_sub(1),
                            choice.from,
                            along,
                            choice.describe()
                        );
                        self.event(now, what);
                        self.state = State::Travel {
                            node: choice.from,
                            then: Then::Trick(choice, spec),
                        };
                    }
                    Some(None) => {
                        let why = self.no_way(ctx.graph, &s, input);
                        self.finish(Outcome::NoWay { why }, input);
                        return None;
                    }
                }
                Some(hold)
            }
            State::Travel { node, then } => {
                let dest = ctx.graph.node(node).origin;
                let (status, step) = nav.go_to(ctx, input, dest);
                self.state = State::Travel { node, then };
                match status {
                    NavStatus::Arrived => match then {
                        Then::Walk => {
                            self.event(now, format!("at node {node}: the last stretch straight"));
                            self.state = State::Final {
                                best: f32::INFINITY,
                                since: now,
                            };
                        }
                        Then::Trick(choice, spec) => {
                            self.event(now, format!("at node {node}: the {}", choice.trick.as_str()));
                            nav.take_trick(spec, node, now);
                            self.state = State::Trick;
                        }
                    },
                    NavStatus::NoPath => {
                        let why = match self.report.failures.last() {
                            Some(f) => format!("link {} -> {} ({}) failed: {}", f.from, f.to, f.kind, f.reason),
                            None => format!("no path to node {node}"),
                        };
                        self.finish(Outcome::Stuck { why }, input);
                        return None;
                    }
                    _ => {}
                }
                step.or(Some(hold))
            }
            State::Trick => {
                let (step, ended) = nav.run_trick(ctx, input);
                self.state = State::Trick;
                if let Some(ended) = ended {
                    self.report.landing = Some(input.origin);
                    let off = (self.spot - input.origin).truncate().length();
                    let dz = input.feet() - (self.spot.z - 36.0);
                    let how = match ended {
                        Ok(()) => "done".to_string(),
                        Err(reason) => format!("failed: {}", reason.as_str()),
                    };
                    self.event(
                        now,
                        format!(
                            "the trick {how}: at {:.0} {:.0} {:.0}, {off:.0} u off, feet {dz:+.0} u",
                            input.origin.x, input.origin.y, input.origin.z
                        ),
                    );
                    // Down on the spot's floor: the rest is walked when the way is clear.
                    let walkable = off <= LANDING_SLACK
                        || (dz.abs() <= ARRIVE_DZ
                            && input.on_ground
                            && simulate_walk(world, &physics(input), input.origin, self.spot, false).ok);
                    if dz.abs() <= ARRIVE_DZ && walkable {
                        if off > LANDING_SLACK {
                            self.event(now, format!("walking the last {off:.0} u"));
                        }
                        self.state = State::Final {
                            best: f32::INFINITY,
                            since: now,
                        };
                    } else {
                        let why = match ended {
                            Ok(()) => String::new(),
                            Err(FailReason::MissingCapability) => "not able to: health, the gauss or uranium".into(),
                            Err(reason) => reason.as_str().into(),
                        };
                        self.finish(
                            Outcome::Missed {
                                landing: input.origin,
                                off,
                                why,
                            },
                            input,
                        );
                        return None;
                    }
                }
                step.or(Some(hold))
            }
            State::Final { mut best, mut since } => {
                let off = (self.spot - input.origin).truncate().length();
                if off < best - 4.0 {
                    best = off;
                    since = now;
                }
                if now - since > STALL {
                    self.finish(Outcome::Blocked { off }, input);
                    return None;
                }
                self.state = State::Final { best, since };
                Some(straight(input, self.spot))
            }
            State::Done(o) => {
                self.state = State::Done(o);
                None
            }
        }
    }

    fn planned_path(&mut self, graph: &NavGraph, path: Vec<NodeId>) {
        let mut kinds: Vec<(String, u32)> = Vec::new();
        for w in path.windows(2) {
            let kind = graph.find_link(w[0], w[1]).map_or("?", |l| l.kind.as_str());
            match kinds.iter_mut().find(|(k, _)| k == kind) {
                Some((_, n)) => *n += 1,
                None => kinds.push((kind.into(), 1)),
            }
        }
        self.report.links = kinds;
        self.report.path = path;
    }

    /// The way along the graph to a node with a clear walk to the spot, or the trick search when there is none.
    fn plan(
        &mut self,
        nav: &mut Navigator,
        ctx: &mut NavCtx<'_>,
        world: &mut dyn MoveWorld,
        input: &NavInput,
    ) -> State {
        let now = input.now;
        let spot = self.spot;
        let phys = physics(input);
        // In the open next to the spot: straight on.
        let off = (spot - input.origin).truncate().length();
        if off <= ANCHOR_NEAR
            && (spot.z - input.origin.z).abs() <= ANCHOR_DZ
            && input.on_ground
            && simulate_walk(world, &phys, input.origin, spot, false).ok
        {
            self.report.planned = Some(off / RUN_SPEED);
            self.event(now, format!("plan: {off:.0} u straight"));
            return State::Final {
                best: f32::INFINITY,
                since: now,
            };
        }
        let graph = ctx.graph;
        let Some(start) = Navigator::start_node(ctx, input.origin) else {
            return State::Done(Outcome::NoWay {
                why: "no graph node near the bot".into(),
            });
        };
        nav.known.expire(now);
        let (costs, came) = {
            let penalty = link_penalty(graph, &nav.known, ctx.health.as_deref(), input);
            costs_from(graph, start, &penalty)
        };
        let mut best: Option<(NodeId, f32, f32)> = None;
        let mut checked = 0;
        for (n, _) in graph.nearest(spot, ANCHOR_NEAR + ANCHOR_DZ, 4 * ANCHORS) {
            let node = graph.node(n);
            let flat = (spot - node.origin).truncate().length();
            if flat > ANCHOR_NEAR
                || (spot.z - node.origin.z).abs() > ANCHOR_DZ
                || node.flags.intersects(UNFIT)
                || !costs[n as usize].is_finite()
            {
                continue;
            }
            if checked == ANCHORS {
                break;
            }
            checked += 1;
            let total = costs[n as usize] + flat / RUN_SPEED;
            if best.is_some_and(|(_, t, _)| t <= total) {
                continue;
            }
            if flat <= self.radius || simulate_walk(world, &phys, node.origin, spot, false).ok {
                best = Some((n, total, flat));
            }
        }
        if let Some((node, total, flat)) = best {
            self.report.planned = Some(total);
            self.planned_path(graph, path_back(&came, &costs, node).unwrap_or_default());
            let kinds: Vec<String> = self.report.links.iter().map(|(k, n)| format!("{k} {n}")).collect();
            let what = format!(
                "plan: {} links to node {node} ({}), {:.1} s, then {flat:.0} u straight",
                self.report.path.len().saturating_sub(1),
                kinds.join(", "),
                costs[node as usize]
            );
            self.event(now, what);
            return State::Travel { node, then: Then::Walk };
        }
        let search = Search::new(graph, spot, costs, came, input, self.allowed);
        self.event(
            now,
            format!(
                "no node with a clear walk to the spot can be got to: looking for a trick from {} takeoffs",
                search.takeoffs.len()
            ),
        );
        State::Search(Box::new(search))
    }

    /// Why there is no way: the node nearest the spot, and what the search tried.
    fn no_way(&self, graph: &NavGraph, s: &Search, input: &NavInput) -> String {
        let mut why = match graph.nearest(self.spot, 4096.0, 1).first() {
            Some(&(n, _)) => {
                let o = graph.node(n).origin;
                let dz = o.z - self.spot.z;
                let height = match dz {
                    d if d.abs() < 18.0 => "at its height".to_string(),
                    d if d > 0.0 => format!("{d:.0} u above"),
                    d => format!("{:.0} u below", -d),
                };
                format!(
                    "the nearest node {n} is {:.0} u off, {height}{}",
                    (o - self.spot).truncate().length(),
                    if s.costs[n as usize].is_finite() {
                        ""
                    } else {
                        ", and the bot cannot get to it"
                    }
                )
            }
            None => "the graph is empty".into(),
        };
        let t = input.tricks;
        let mut cannot = Vec::new();
        if !self.allowed.jump {
            cannot.push("jumps not allowed".to_string());
        }
        match (self.allowed.longjump, t.longjump) {
            (false, _) => cannot.push("long jumps not allowed".into()),
            (true, false) => cannot.push("no long jump module".into()),
            _ => {}
        }
        match (self.allowed.gauss, t.boost_now && t.gauss_damage > 0.0) {
            (false, _) => cannot.push("gauss boosts not allowed".into()),
            (true, false) => cannot.push(format!(
                "no gauss boost (the gauss, a charge's uranium and {BOOST_HEALTH:.0} health are needed)"
            )),
            _ => {}
        }
        let st = s.stats;
        if st.takeoffs == 0 {
            why.push_str("; no node the bot can get to is within a trick's reach of the spot");
        } else {
            why.push_str(&format!(
                "; tricks from {} takeoffs: {} jumps, {} long jumps, {} gauss boosts checked, none lands",
                st.takeoffs, st.jumps, st.longjumps, st.boosts
            ));
        }
        if !cannot.is_empty() {
            why.push_str(&format!(" ({})", cannot.join(", ")));
        }
        why
    }
}

/// One way to try from a takeoff.
#[derive(Clone, Copy, Debug)]
enum Try {
    Jump,
    LongJump,
    Boost(f32, f32),
}

/// The search for a trick onto the spot: takeoffs the bot can get to, the cheapest (way there and trick) first, and
/// on each the tricks that could reach the spot, one check at a time.
struct Search {
    spot: Vec3,
    takeoffs: Vec<NodeId>,
    next: usize,
    /// The takeoff under check and what is left to try on it (the next try last).
    from: NodeId,
    tries: SmallVec<[Try; 8]>,
    costs: Vec<f32>,
    came: Vec<u32>,
    stats: SearchStats,
}

impl Search {
    fn new(
        graph: &NavGraph,
        spot: Vec3,
        costs: Vec<f32>,
        came: Vec<u32>,
        input: &NavInput,
        allowed: Allowed,
    ) -> Search {
        let t = input.tricks;
        let reach = [
            if allowed.jump { TRICK_REACH[0] } else { 0.0 },
            if allowed.longjump && t.longjump {
                TRICK_REACH[1]
            } else {
                0.0
            },
            if allowed.gauss && t.boost_now && t.gauss_damage > 0.0 {
                TRICK_REACH[2]
            } else {
                0.0
            },
        ];
        let mut takeoffs: Vec<(NodeId, f32)> = (0..graph.len() as NodeId)
            .filter_map(|n| {
                let node = graph.node(n);
                let cost = costs[n as usize];
                if !cost.is_finite() || node.flags.intersects(UNFIT | NodeFlags::CROUCH) {
                    return None;
                }
                let flat = (spot - node.origin).truncate().length();
                let rise = spot.z - node.origin.z;
                let fits = (0..3).any(|k| flat <= reach[k] && rise <= TRICK_RISE[k]);
                // The way there and a rough price of the trick: a boost stands still to charge, and a long one
                // flies less surely.
                let price = if flat <= reach[0] && rise <= TRICK_RISE[0] {
                    flat / RUN_SPEED
                } else if flat <= reach[1] && rise <= TRICK_RISE[1] {
                    LONGJUMP_SETUP + 1.0
                } else {
                    BOOST_SETUP + flat / BOOST_PRICE_SPEED
                };
                fits.then_some((n, cost + price))
            })
            .collect();
        takeoffs.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
        takeoffs.truncate(TAKEOFFS);
        Search {
            spot,
            takeoffs: takeoffs.into_iter().map(|(n, _)| n).collect(),
            next: 0,
            from: 0,
            tries: SmallVec::new(),
            costs,
            came,
            stats: SearchStats::default(),
        }
    }

    /// The tricks to try from node `n`, the next last.
    fn tries_from(&self, graph: &NavGraph, n: NodeId, input: &NavInput, allowed: Allowed) -> SmallVec<[Try; 8]> {
        let t = input.tricks;
        let o = graph.node(n).origin;
        let flat = (self.spot - o).truncate().length();
        let rise = self.spot.z - o.z;
        let mut tries: SmallVec<[Try; 8]> = SmallVec::new();
        if allowed.gauss && t.boost_now && t.gauss_damage > 0.0 && flat <= TRICK_REACH[2] && rise <= TRICK_RISE[2] {
            let full = 5.0 * t.gauss_damage;
            let boosts = boost_tries(input.gravity(), o, self.spot, full);
            for &(pitch, push) in boosts.iter().take(BOOST_TRIES).rev() {
                tries.push(Try::Boost(pitch, push));
            }
        }
        if allowed.longjump && t.longjump && (LONGJUMP_NEAR..=TRICK_REACH[1]).contains(&flat) && rise <= TRICK_RISE[1] {
            tries.push(Try::LongJump);
        }
        if allowed.jump && flat <= TRICK_REACH[0] && rise <= TRICK_RISE[0] {
            tries.push(Try::Jump);
        }
        tries
    }

    /// Runs checks while `budget` lasts: `None` to go on next frame, `Some(None)` when no trick lands, the trick and
    /// its contract when one does.
    fn step(
        &mut self,
        world: &mut dyn MoveWorld,
        graph: &NavGraph,
        phys: &Physics,
        input: &NavInput,
        allowed: Allowed,
        budget: &mut u32,
    ) -> Option<Option<(TrickChoice, TraversalSpec)>> {
        loop {
            let Some(t) = self.tries.pop() else {
                let Some(&n) = self.takeoffs.get(self.next) else {
                    return Some(None);
                };
                self.next += 1;
                self.from = n;
                self.stats.takeoffs += 1;
                self.tries = self.tries_from(graph, n, input, allowed);
                continue;
            };
            if *budget == 0 {
                self.tries.push(t);
                return None;
            }
            *budget -= 1;
            if let Some(found) = self.check(world, graph, phys, input, t) {
                return Some(Some(found));
            }
        }
    }

    fn check(
        &mut self,
        world: &mut dyn MoveWorld,
        graph: &NavGraph,
        phys: &Physics,
        input: &NavInput,
        t: Try,
    ) -> Option<(TrickChoice, TraversalSpec)> {
        let from = graph.node(self.from).origin;
        let to = self.spot;
        let health = input.health;
        let mut choice = TrickChoice {
            trick: Trick::Jump,
            from: self.from,
            takeoff: from,
            pitch: 0.0,
            push: 0.0,
            charge: 0.0,
            speed: 0.0,
            duck: false,
            flight: 0.0,
            robustness: 0.0,
            damage: 0.0,
            seconds: 0.0,
        };
        let spec = match t {
            Try::Jump => {
                self.stats.jumps += 1;
                let plan = plan_jump(world, phys, from, to)?;
                let damage = phys.fall_damage(plan.impact);
                if damage > 0.0 && health - damage <= DROP_RESERVE {
                    return None;
                }
                choice.speed = plan.speed;
                choice.duck = plan.duck;
                choice.flight = plan.flight;
                choice.robustness = plan.robustness;
                choice.damage = damage;
                choice.seconds = JUMP_SETUP + plan.flight;
                trick_spec(
                    from,
                    to,
                    24.0,
                    Action::Jump {
                        speed: plan.speed,
                        duck: plan.duck,
                        robustness: plan.robustness,
                    },
                    Needs {
                        health: damage + if damage > 0.0 { DROP_RESERVE } else { 0.0 },
                        longjump: false,
                        gauss: false,
                    },
                    5.0 + plan.flight,
                    from.distance(to) / (RUN_SPEED * 0.8),
                    damage,
                )
            }
            Try::LongJump => {
                self.stats.longjumps += 1;
                let plan = plan_longjump(world, phys, from, to)?;
                let damage = phys.fall_damage(plan.impact);
                if damage > 0.0 && health - damage <= DROP_RESERVE {
                    return None;
                }
                choice.trick = Trick::LongJump;
                choice.flight = plan.flight;
                choice.robustness = plan.robustness;
                choice.damage = damage;
                choice.seconds = LONGJUMP_SETUP + plan.flight;
                trick_spec(
                    from,
                    to,
                    24.0,
                    Action::LongJump {
                        robustness: plan.robustness,
                    },
                    Needs {
                        health: damage + if damage > 0.0 { DROP_RESERVE } else { 0.0 },
                        longjump: true,
                        gauss: false,
                    },
                    5.0 + plan.flight,
                    LONGJUMP_SETUP + plan.flight,
                    damage,
                )
            }
            Try::Boost(pitch, push) => {
                self.stats.boosts += 1;
                let tricks = input.tricks;
                let full = 5.0 * tricks.gauss_damage;
                let plan = check_boost(world, phys, from, to, (pitch, push), full)?;
                let damage = phys.fall_damage(plan.impact);
                // The executor sets off with this much health only.
                let needs = BOOST_HEALTH.max(damage + BOOST_HEALTH_AFTER);
                if health < needs {
                    return None;
                }
                let view = boost_view((to - from).truncate().normalize_or_zero(), plan.pitch);
                if !beam_safe(world, boost_eye(from), view, plan.push / 5.0, tricks.selfgauss) {
                    return None;
                }
                choice.trick = Trick::GaussBoost;
                choice.pitch = plan.pitch;
                choice.push = plan.push;
                choice.charge = boost_charge(plan.push, tricks.gauss_damage);
                choice.flight = plan.flight;
                choice.robustness = plan.robustness;
                choice.damage = damage;
                choice.seconds = BOOST_READY + choice.charge + plan.flight;
                trick_spec(
                    from,
                    to,
                    12.0,
                    Action::GaussBoost {
                        pitch: plan.pitch,
                        robustness: plan.robustness,
                        push: plan.push,
                    },
                    Needs {
                        health: needs,
                        longjump: false,
                        gauss: true,
                    },
                    BOOST_SETUP + 6.0 + choice.charge + plan.flight,
                    BOOST_SETUP + choice.charge + plan.flight + BOOST_PRICE,
                    damage,
                )
            }
        };
        Some((choice, spec))
    }
}

/// The contract of a trick from `from` (a node's standing origin) onto `to` off the graph.
#[allow(clippy::too_many_arguments)]
fn trick_spec(
    from: Vec3,
    to: Vec3,
    entry_radius: f32,
    action: Action,
    needs: Needs,
    deadline: f32,
    time: f32,
    damage: f32,
) -> TraversalSpec {
    TraversalSpec {
        entry: Anchor {
            origin: from,
            radius: entry_radius,
            stance: Stance::Stand,
        },
        exit: Anchor {
            origin: to,
            radius: 32.0,
            stance: Stance::Stand,
        },
        action,
        needs,
        deadline,
        cost: Cost {
            time,
            wait: 0.0,
            damage,
        },
    }
}

/// Where a player stands at `p` (a point on or just over a floor, or in the air over one): the origin of a standing
/// player on the floor under it; `None` over no floor within 600 units, or where no player fits.
pub fn standing_spot(tracer: &mut dyn Tracer, p: Vec3) -> Option<Vec3> {
    for lift in [40.0, 56.0, 72.0] {
        let top = p + Vec3::Z * lift;
        let tr = tracer.trace(&TraceQuery::hull(top, top - Vec3::Z * 600.0, HullKind::Stand));
        if tr.start_solid || tr.all_solid {
            continue;
        }
        return (tr.fraction < 1.0).then_some(tr.end);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use lb_kin::boxworld::BoxWorld;

    #[test]
    fn tricks_are_named_or_all_or_none() {
        assert_eq!(Allowed::parse(&["any"]).unwrap(), Allowed::default());
        assert_eq!(Allowed::parse::<&str>(&[]).unwrap(), Allowed::NONE);
        let a = Allowed::parse(&["jump", "gauss"]).unwrap();
        assert!(a.jump && a.gauss && !a.longjump);
        assert_eq!(a.names(), "jump gauss");
        assert_eq!(Allowed::NONE.names(), "none");
        assert!(Allowed::parse(&["fly"]).is_err());
    }

    #[test]
    fn a_spot_is_where_a_player_stands_on_the_floor_under_it() {
        let mut w = BoxWorld::new();
        w.floor(0.0, 1024.0);
        w.solid(Vec3::new(100.0, -64.0, 0.0), Vec3::new(300.0, 64.0, 80.0));
        let on_floor = standing_spot(&mut w, Vec3::ZERO).unwrap();
        assert!(on_floor.distance(Vec3::new(0.0, 0.0, 36.0)) < 0.5, "{on_floor}");
        let on_box = standing_spot(&mut w, Vec3::new(200.0, 0.0, 80.0)).unwrap();
        assert!((on_box.z - 116.0).abs() < 0.5, "{on_box}");
        let over_floor = standing_spot(&mut w, Vec3::new(-200.0, 0.0, 300.0)).unwrap();
        assert!((over_floor.z - 36.0).abs() < 0.5, "{over_floor}");
        assert_eq!(standing_spot(&mut w, Vec3::new(5000.0, 0.0, 0.0)), None, "no floor");
    }
}
