//! Tricks a bot takes on the way where no link of the graph has them, checked by following the flight through the
//! live server's traces: a long jump at an enemy, a long jump along a straight stretch of the path (the follower's
//! runway), a gauss boost further along the path.

use lb_core::Vec3;
use lb_core::math::view_angle_vectors;
use lb_kin::tricks::{BoostQuery, Traced, boost_view, hazard, longjump_from, simulate_boost};
use lb_kin::{Physics, Player};
use lb_worldq::{TraceQuery, Tracer};

use crate::classify::{BOOST_HEALTH, BOOST_HEALTH_AFTER, anchor};
use crate::exec::{BOOST_CHARGE, EYE_HEIGHT, NavInput};
use crate::graph::{NavGraph, NodeFlags, NodeId};
use crate::spec::{Action, Anchor, Cost, Needs, Stance, TraversalSpec};

/// The pitch a boost taken on the way looks down at.
pub const LEAP_PITCH: f32 = 34.0;
/// A boost on the way must bring the bot this many seconds (of the planner's reckoning) nearer its goal.
const LEAP_GAIN: f32 = 2.0;
/// Landings of a boost on the way: nodes within this of the flight's line, from this share of its unsteered reach
/// on, no higher than this above where it comes down unsteered; the best few are tried steered.
const LEAP_NEAR: f32 = 160.0;
const LEAP_SHORT: f32 = 0.4;
const LEAP_ABOVE: f32 = 160.0;
const LEAP_TRIES: usize = 3;
/// How far a gauss beam goes.
const BEAM_REACH: f32 = 8192.0;

/// The server's movement settings as the bot's input tells them.
pub fn physics(input: &NavInput) -> Physics {
    Physics {
        gravity: input.gravity(),
        maxspeed: input.max_speed,
        ..Physics::default()
    }
}

/// The bot as the movement code sees it.
pub fn player(input: &NavInput) -> Player {
    let mut p = Player::standing(input.origin);
    p.velocity = input.velocity;
    p.longjump = input.tricks.longjump;
    p
}

/// Where a long jump taken now looking along `view` comes down, if it comes down safely: on a floor or in water,
/// without fall damage, out of lava and slime.
pub fn leap_lands(tracer: &mut dyn Tracer, input: &NavInput, view: Vec3) -> Option<Vec3> {
    if !input.on_ground || input.on_ladder || input.waterlevel > 0 {
        return None;
    }
    let phys = physics(input);
    let mut world = Traced(tracer);
    let mut p = player(input);
    p.longjump = true;
    let v = longjump_from(&mut world, &phys, p, view.y, None);
    if !v.ok || phys.fall_damage(v.impact) > 0.0 {
        return None;
    }
    let mut landed = Player::standing(v.landing);
    landed.origin = v.landing;
    (!hazard(&mut world, &landed)).then_some(v.landing)
}

/// A charged gauss beam fired from `eye` along `view` spares its shooter: it meets the wall or floor square enough
/// not to glance off (a glancing beam bursts where it meets the wall, around a shooter next to it), and punches
/// through it or dies in it. With `selfgauss` a wall too thick to punch through sends the beam back at the shooter.
pub fn beam_safe(tracer: &mut dyn Tracer, eye: Vec3, view: Vec3, damage: f32, selfgauss: bool) -> bool {
    let (dir, _, _) = view_angle_vectors(view);
    let hit = tracer.trace(&TraceQuery::line(eye, eye + dir * BEAM_REACH));
    if hit.fraction >= 1.0 {
        return true;
    }
    if hit.start_solid || -hit.normal.dot(dir) < 0.5 {
        return false;
    }
    if !selfgauss {
        return true;
    }
    let on = tracer.trace(&TraceQuery::line(hit.end + dir * 8.0, eye + dir * BEAM_REACH));
    if on.all_solid {
        return true;
    }
    let out = tracer.trace(&TraceQuery::line(on.end, hit.end)).end;
    out.distance(hit.end) < damage
}

/// The eyes of a player standing at `origin` when a boost's charge goes (a jump's first moment up).
pub fn boost_eye(origin: Vec3) -> Vec3 {
    origin + Vec3::Z * (EYE_HEIGHT + 3.0)
}

/// The contract of a boost on the way from `from` onto node `to` of the graph.
fn leap_spec(graph: &NavGraph, from: Vec3, to: NodeId, flight: f32, damage: f32) -> TraversalSpec {
    TraversalSpec {
        entry: Anchor {
            origin: from,
            radius: 12.0,
            stance: Stance::Stand,
        },
        exit: anchor(graph.node(to), 32.0),
        action: Action::GaussBoost {
            pitch: LEAP_PITCH,
            robustness: 1.0,
        },
        needs: Needs {
            health: BOOST_HEALTH.max(damage + BOOST_HEALTH_AFTER),
            longjump: false,
            gauss: true,
        },
        deadline: BOOST_CHARGE + 10.0 + flight,
        cost: Cost {
            time: BOOST_CHARGE + flight,
            wait: 0.0,
            damage,
        },
    }
}

/// A gauss boost from where the bot stands toward its goal (`path[next..]`, the goal last). The ways toward the next
/// few nodes of the path and toward the goal are tried: where each flight comes down unsteered is followed, and the
/// nodes along its line short of that (the air brakes a flight onto them) are the landings to steer for. The one the
/// planner reckons most seconds nearer the goal, `LEAP_GAIN` at least, is taken if the steered flight gets there,
/// comes down leaving the bot `BOOST_HEALTH_AFTER` health, and the beam spares the bot. Off the path, the way on is
/// planned again after the landing. Returns the contract, the landing node and its index in the path when on it.
pub fn leap_along(
    graph: &NavGraph,
    tracer: &mut dyn Tracer,
    input: &NavInput,
    path: &[NodeId],
    next: usize,
) -> Option<(TraversalSpec, NodeId, Option<usize>)> {
    let t = input.tricks;
    if !t.boost_now || !input.on_ground || input.waterlevel > 0 || input.health < BOOST_HEALTH || path.is_empty() {
        return None;
    }
    let phys = physics(input);
    let push = 5.0 * t.gauss_damage;
    let goal = *path.last()?;
    let here = path.get(next.saturating_sub(1)).copied().unwrap_or(path[0]);
    let from_here = crate::plan::estimate(graph, here, goal);
    let unfit = NodeFlags::LADDER | NodeFlags::WATER | NodeFlags::AIRBORNE | NodeFlags::ON_MOVER | NodeFlags::CROUCH;
    let mut ways: smallvec::SmallVec<[lb_core::Vec2; 5]> = smallvec::SmallVec::new();
    let targets = (next + 1..(next + 4).min(path.len())).chain(std::iter::once(path.len() - 1));
    for k in targets {
        let d = (graph.node(path[k]).origin - input.origin).truncate();
        if d.length() < 128.0 {
            continue;
        }
        let dir = d.normalize();
        if !ways.iter().any(|w| w.dot(dir) > 0.985) {
            ways.push(dir);
        }
    }
    // Landings along each free flight's line, by how much nearer the goal they are.
    let mut landings: Vec<(NodeId, f32)> = Vec::new();
    for dir in ways {
        let q = BoostQuery {
            from: input.origin,
            dir,
            pitch: LEAP_PITCH,
            push,
            to: None,
        };
        let free = simulate_boost(&mut Traced(tracer), &phys, &q);
        if !free.ok || free.in_water {
            continue;
        }
        let reach = (free.landing - input.origin).truncate().dot(dir);
        for (i, node) in graph.nodes.iter().enumerate() {
            let rel = (node.origin - input.origin).truncate();
            let along = rel.dot(dir);
            let across = (rel - dir * along).length();
            let fits = !node.flags.intersects(unfit)
                && (LEAP_SHORT * reach..=reach + 64.0).contains(&along)
                && across < LEAP_NEAR
                && node.origin.z < free.landing.z + LEAP_ABOVE;
            if !fits {
                continue;
            }
            let gain = from_here - crate::plan::estimate(graph, i as NodeId, goal);
            if gain >= LEAP_GAIN && !landings.iter().any(|(n, _)| *n == i as NodeId) {
                landings.push((i as NodeId, gain));
            }
        }
    }
    landings.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    for &(landing, _) in landings.iter().take(LEAP_TRIES) {
        let to = graph.node(landing).origin;
        let dir = (to - input.origin).truncate().normalize_or_zero();
        let q = BoostQuery {
            from: input.origin,
            dir,
            pitch: LEAP_PITCH,
            push,
            to: Some(to),
        };
        let v = simulate_boost(&mut Traced(tracer), &phys, &q);
        let damage = phys.fall_damage(v.impact);
        if !v.ok || input.health - damage < BOOST_HEALTH_AFTER {
            continue;
        }
        let view = boost_view(dir, LEAP_PITCH);
        if !beam_safe(tracer, boost_eye(input.origin), view, t.gauss_damage, t.selfgauss) {
            continue;
        }
        let at = path.iter().skip(next).position(|&n| n == landing).map(|i| i + next);
        return Some((leap_spec(graph, input.origin, landing, v.flight, damage), landing, at));
    }
    None
}
