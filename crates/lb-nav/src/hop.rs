//! Bunny hopping along the way. On a stretch of plain walking links a bot that may hop jumps again on the first
//! command back on the ground and strafes its flight along the path, a little faster every hop up to its skill's
//! limit; on a server that crops fast jumps, just under the crop. Each hop is followed through the server's traces
//! before the jump: it must come down on the path's floor without bumping into anything, where the way on is walked
//! from, short enough of where the hopping has to end (a link walking does not do, a sharp turn, stairs or a ramp,
//! the end of the way) for friction to bring the speed down by then. Corners, stairs and ramps are run, and the hops
//! start again past them.

use lb_core::{Vec2, Vec3};
use lb_kin::hop::{Air, air_strafe, simulate_hop};
use lb_kin::tricks::{Traced, hazard};
use lb_worldq::{HullKind, TraceQuery, Tracer};
use smallvec::SmallVec;

use crate::exec::NavInput;
use crate::graph::{LinkFlags, LinkKind, NavGraph, NodeFlags, NodeId};

/// Hops are kept this far under the speed the server crops a jump at.
pub const CROP_MARGIN: f32 = 0.97;
/// A run of hops starts from a run at least this share of maxspeed.
const START_SPEED: f32 = 0.9;
/// Hops are taken only when they may get up to this many times maxspeed at least.
const GAIN_MIN: f32 = 1.05;
/// Where the path turns by more than this (the cosine of 60°), hops stop: the corner is run round.
const TURN: f32 = 0.5;
/// Where the path climbs or goes down steeper than this (rise per unit across, about 8°), hops stop: stairs and
/// ramps are run up and down (a hop down beside narrow stairs would not get onto them again).
const SLOPE: f32 = 0.14;
/// A hop comes down this far short of where the hops stop at least, besides the run friction takes to brake.
const STOP_ROOM: f32 = 48.0;
/// The speed the way's last node is come to at, hopping stopped before it.
const LAST_SPEED: f32 = 90.0;
/// A hop comes down this close to the path's line, across and up or down.
const LANDING_ACROSS: f32 = 40.0;
const LANDING_UP: f32 = 24.0;
/// Down off the path by more than this, across or up or down, a hop missed.
const MISSED_ACROSS: f32 = 64.0;
const MISSED_UP: f32 = 40.0;
/// A hop is followed this long at most, seconds.
const FLIGHT_LIMIT: f32 = 1.2;
/// The path is looked along this far for the stretch to hop, over this many nodes at most.
const SCAN: f32 = 1600.0;
const SCAN_NODES: usize = 16;
/// The point the flight is steered at: this many seconds of the flight's speed along the path ahead, this many
/// units at least.
const AHEAD_SECS: f32 = 0.25;
const AHEAD_MIN: f32 = 64.0;
/// Room over the head a hop needs: the height of its arc.
const HEADROOM: f32 = 46.0;
/// Commands the followed flight is cut into: no shorter than this many milliseconds (a thousand commands a second
/// would mean hundreds of moves per check), no longer than the game runs one in.
const SIM_MSEC: [f32; 2] = [4.0, 50.0];
/// Nodes a hop is not taken past or onto.
const UNFIT: NodeFlags = NodeFlags::CROUCH
    .union(NodeFlags::LADDER)
    .union(NodeFlags::WATER)
    .union(NodeFlags::AIRBORNE)
    .union(NodeFlags::ON_MOVER);

/// The line a run of hops flies along: from where the bot took off through the path's nodes to where hopping stops
/// (hull centres).
#[derive(Clone, Debug, PartialEq)]
pub struct HopPilot {
    points: SmallVec<[Vec3; 16]>,
}

impl HopPilot {
    /// How long the line is, across.
    pub fn length(&self) -> f32 {
        self.points.windows(2).map(|w| (w[1] - w[0]).truncate().length()).sum()
    }

    /// The point of the line nearest `p` across: how far along the line it is, the point, and how far `p` is from
    /// it across.
    pub fn locate(&self, p: Vec3) -> (f32, Vec3, f32) {
        let mut best = (0.0, self.points[0], (p - self.points[0]).truncate().length());
        let mut along = 0.0;
        for w in self.points.windows(2) {
            let seg = (w[1] - w[0]).truncate();
            let len = seg.length();
            if len < 1e-3 {
                continue;
            }
            let t = ((p - w[0]).truncate().dot(seg) / (len * len)).clamp(0.0, 1.0);
            let q = w[0].lerp(w[1], t);
            let off = (p - q).truncate().length();
            if off < best.2 {
                best = (along + t * len, q, off);
            }
            along += len;
        }
        best
    }

    /// The point `s` units along the line; past its end, on along the last piece.
    fn at(&self, s: f32) -> Vec3 {
        let mut left = s.max(0.0);
        for w in self.points.windows(2) {
            let len = (w[1] - w[0]).truncate().length();
            if len >= left && len > 1e-3 {
                return w[0].lerp(w[1], left / len);
            }
            left -= len;
        }
        let n = self.points.len();
        let last = self.points[n - 1];
        let dir = if n > 1 {
            (last - self.points[n - 2]).truncate().normalize_or_zero()
        } else {
            Vec2::ZERO
        };
        last + dir.extend(0.0) * left
    }

    /// Which way to fly from `origin` at `speed`: toward the point a quarter of a second's flight ahead along the
    /// line from the nearest one to it.
    pub fn want(&self, origin: Vec3, speed: f32) -> Vec2 {
        let (along, _, _) = self.locate(origin);
        let aim = self.at(along + (AHEAD_SECS * speed).max(AHEAD_MIN));
        (aim - origin).truncate().normalize_or_zero()
    }

    /// The first of the path's nodes on the line further along than `along`; the last one past its end.
    fn node_after(&self, along: f32) -> Vec3 {
        let mut at = 0.0;
        for w in self.points.windows(2) {
            at += (w[1] - w[0]).truncate().length();
            if at > along {
                return w[1];
            }
        }
        self.points[self.points.len() - 1]
    }

    /// From where a hop came down (`origin`, `feet`: standing there), the way on to the next of the path's nodes is
    /// walked in a straight line: a standing hull a step's height up (stairs do not count) gets there. A hop down
    /// beside a raised stretch of the way, off a ledge it climbs, is not on the way.
    fn walks_on(&self, tracer: &mut dyn Tracer, origin: Vec3, feet: f32) -> bool {
        let (along, _, _) = self.locate(origin);
        let to = self.node_after(along);
        let lift = 36.0 + crate::validate::STEP_SIZE;
        let tr = tracer.trace(&TraceQuery::hull(
            origin.truncate().extend(feet + lift),
            to.truncate().extend(to.z - 36.0 + lift),
            HullKind::Stand,
        ));
        !tr.start_solid && tr.fraction >= 1.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HopPhase {
    /// The jump is pressed (since then) and the bot is still on the ground.
    Takeoff {
        at: f64,
    },
    Air,
}

/// A hop under way.
#[derive(Clone, Debug, PartialEq)]
pub struct HopRun {
    pub pilot: HopPilot,
    /// Index in the path of the node hopping stops at: in the air the follower is not moved on past it.
    pub stop: usize,
    /// Speed kept to.
    pub target: f32,
    pub phase: HopPhase,
    /// When the run was last followed: one not followed for a while (a fight took over on the ground) is dropped.
    pub last_tick: f64,
}

impl HopRun {
    /// Where to press now (direction and speed), the bot as `s` says: strafing the flight along the line.
    pub fn press(&self, s: &NavInput) -> Option<(Vec2, f32)> {
        let v = s.velocity.truncate();
        let want = self.pilot.want(s.origin, v.length());
        air_strafe(v, want, self.target, &air(s, s.cmd_secs()))
    }

    /// Came down near the line (across and up or down), where the way on is walked from.
    pub fn landed_on_way(&self, s: &NavInput, tracer: &mut dyn Tracer) -> bool {
        let (_, on, across) = self.pilot.locate(s.origin);
        across <= MISSED_ACROSS
            && (s.feet() - (on.z - 36.0)).abs() <= MISSED_UP
            && self.pilot.walks_on(tracer, s.origin, s.feet())
    }
}

/// Why a hop is not taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Refusal {
    pub why: &'static str,
    /// It took a look at the way (traces, a followed flight) that is not worth taking again from the same spot soon.
    pub costly: bool,
}

fn refuse(why: &'static str) -> Refusal {
    Refusal { why, costly: false }
}

fn costly(why: &'static str) -> Refusal {
    Refusal { why, costly: true }
}

fn air(s: &NavInput, dt: f32) -> Air {
    Air {
        airaccelerate: s.airaccelerate(),
        maxspeed: s.max_speed,
        dt,
    }
}

/// The speed a bot keeps to hopping, as its limits and the server's crop allow, or nothing worth hopping for.
pub fn target_speed(s: &NavInput) -> Option<f32> {
    let limits = s.tricks.bhop?;
    let cap = s.hop_cap();
    let mult = if cap.is_finite() { limits[0] } else { limits[1] };
    let target = (mult * s.max_speed).min(CROP_MARGIN * cap);
    (target >= GAIN_MIN * s.max_speed).then_some(target)
}

/// The stretch of the path from node index `next` on that hops may fly along: its line from `origin`, the index of
/// the node they stop at and the speed the bot may come to it at.
fn stretch(g: &NavGraph, path: &[NodeId], next: usize, s: &NavInput) -> Option<(HopPilot, usize, f32)> {
    let mut points: SmallVec<[Vec3; 16]> = SmallVec::new();
    points.push(s.origin);
    let mut along = 0.0;
    let mut stop = None;
    // A node right by the bot is not a climb for being a few units off its floor.
    let steep = |d: Vec3| d.z.abs() > SLOPE * d.truncate().length().max(AHEAD_MIN);
    for k in next..path.len() {
        let n = g.node(path[k]);
        if n.flags.intersects(UNFIT) || (k == next && steep(n.origin - s.origin)) {
            if k == next {
                return None;
            }
            stop = Some((k - 1, s.max_speed));
            break;
        }
        along += (n.origin - *points.last().unwrap_or(&s.origin)).truncate().length();
        points.push(n.origin);
        let Some(&after) = path.get(k + 1) else {
            stop = Some((k, LAST_SPEED));
            break;
        };
        let link = g.find_link(path[k], after);
        if link.is_none_or(|l| l.kind != LinkKind::Walk || l.flags.contains(LinkFlags::DYNAMIC)) {
            let entry = link.and_then(|l| g.spec(l)).and_then(|spec| spec.entry_speed());
            stop = Some((k, entry.unwrap_or(s.max_speed).min(s.max_speed)));
            break;
        }
        let from = if k == next && k > 0 {
            g.node(path[k - 1]).origin
        } else {
            points[points.len() - 2]
        };
        let into = (n.origin - from).truncate().normalize_or_zero();
        let leg = g.node(after).origin - n.origin;
        let out = leg.truncate().normalize_or_zero();
        if into.dot(out) < TURN || steep(leg) {
            stop = Some((k, s.max_speed));
            break;
        }
        if along > SCAN || points.len() > SCAN_NODES {
            stop = Some((k, f32::INFINITY));
            break;
        }
    }
    let (stop, allowed) = stop?;
    // Points past the stop node are not part of the line (an unfit node ends it at the node before).
    points.truncate(stop + 2 - next);
    Some((HopPilot { points }, stop, allowed))
}

/// A hop from where the bot stands, on along `path` from node index `next` (the node walked to): the run to fly,
/// once its flight is followed through the server's traces (from `flights`, the budget shared by the bots).
pub fn check(
    g: &NavGraph,
    path: &[NodeId],
    next: usize,
    s: &NavInput,
    tracer: &mut dyn Tracer,
    flights: &mut u32,
) -> Result<HopRun, Refusal> {
    let Some(target) = target_speed(s) else {
        return Err(refuse("not allowed"));
    };
    if !s.on_ground || s.on_ladder || s.ducked || s.waterlevel > 0 || s.push != Vec3::ZERO {
        return Err(refuse("off the floor"));
    }
    let speed = s.velocity.truncate().length();
    if speed < START_SPEED * s.max_speed {
        return Err(refuse("too slow"));
    }
    if s.velocity.length() > 0.99 * s.hop_cap() {
        return Err(refuse("past the crop"));
    }
    let Some((pilot, stop, allowed)) = stretch(g, path, next, s) else {
        return Err(costly("no stretch to hop"));
    };
    let phys = crate::tricks::physics(s);
    let brake = |v: f32| (v - allowed).max(0.0) / phys.friction.max(0.1);
    let length = pilot.length();
    if length < speed * 0.6 + brake(speed) + STOP_ROOM {
        return Err(costly("too short a stretch"));
    }
    let head = tracer.trace(&TraceQuery::hull(
        s.origin,
        s.origin + Vec3::Z * HEADROOM,
        HullKind::Stand,
    ));
    if head.start_solid || head.fraction < 1.0 {
        return Err(costly("a low ceiling"));
    }
    if *flights == 0 {
        return Err(refuse("no flight left to follow this frame"));
    }
    *flights -= 1;
    let msec = (s.cmd_secs() * 1000.0).round().clamp(SIM_MSEC[0], SIM_MSEC[1]) as u8;
    let a = air(s, f32::from(msec) / 1000.0);
    let mut world = Traced(tracer);
    let flight = simulate_hop(
        &mut world,
        &phys,
        crate::tricks::player(s),
        msec,
        FLIGHT_LIMIT,
        &mut |p| {
            let v = p.velocity.truncate();
            air_strafe(v, pilot.want(p.origin, v.length()), target, &a)
        },
    );
    if !flight.jumped || !flight.landed {
        return Err(costly("does not come down on a floor"));
    }
    if flight.bumped {
        return Err(costly("runs into something"));
    }
    let p = flight.player;
    let (along, on, across) = pilot.locate(p.origin);
    if across > LANDING_ACROSS || (p.feet() - (on.z - 36.0)).abs() > LANDING_UP {
        return Err(costly("comes down off the way"));
    }
    if hazard(&mut world, &p) || phys.fall_damage(flight.impact) > 0.0 {
        return Err(costly("comes down hurt"));
    }
    if !pilot.walks_on(&mut world, p.origin, p.feet()) {
        return Err(costly("comes down where the way on is not walked"));
    }
    if length - along < brake(p.velocity.truncate().length()) + STOP_ROOM {
        return Err(costly("comes down too near where the hops stop"));
    }
    Ok(HopRun {
        pilot,
        stop,
        target,
        phase: HopPhase::Takeoff { at: s.now },
        last_tick: s.now,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pilot(points: &[(f32, f32)]) -> HopPilot {
        HopPilot {
            points: points.iter().map(|&(x, y)| Vec3::new(x, y, 36.0)).collect(),
        }
    }

    #[test]
    fn the_pilot_aims_ahead_along_the_line() {
        let p = pilot(&[(0.0, 0.0), (400.0, 0.0), (800.0, 0.0)]);
        assert_eq!(p.length(), 800.0);
        let (along, on, across) = p.locate(Vec3::new(300.0, 20.0, 36.0));
        assert_eq!((along, on.x, across), (300.0, 300.0, 20.0));
        let want = p.want(Vec3::new(300.0, 20.0, 36.0), 400.0);
        assert!(want.x > 0.98 && want.y < 0.0, "back onto the line ahead: {want}");
        let past = p.want(Vec3::new(790.0, 0.0, 36.0), 400.0);
        assert!(past.x > 0.99, "on along the last piece past the end: {past}");
    }
}
