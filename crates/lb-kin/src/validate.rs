//! Traversal checks by simulation: a player does the move with the same inputs a bot would send, and the check
//! looks where it ends up. Used to classify links (jump, crouch jump, drop), to learn what a jump needs (run-up
//! speed, ducking in the air) and how much a fall costs.

use lb_core::input::{IN_DUCK, IN_FORWARD, IN_JUMP};
use lb_core::math::dir_to_view_angles;
use lb_core::{Vec2, Vec3};

use lb_worldq::{HullKind, TraceQuery, Tracer};

use crate::physics::Physics;
use crate::pmove::{Cmd, MoveWorld, Player, player_move};

/// How a bot takes off for a running jump. The jump executor follows these rules and the validator simulates them,
/// so a jump that validates is one a bot can make.
pub mod takeoff {
    /// Where along the jump, from the takeoff point, the bot may leave the ground.
    pub const WINDOW: (f32, f32) = (-12.0, 12.0);
    /// A run-up shorter than this cannot gather speed before the window: the speed is gathered in it, and the
    /// window reaches `SHORT_REACH` further.
    pub const SHORT_RUN: f32 = 16.0;
    pub const SHORT_REACH: f32 = 8.0;
    /// Share of the planned speed a takeoff needs, after a full and after a short run-up.
    pub const SPEED: f32 = 0.9;
    pub const SPEED_SHORT: f32 = 0.75;
    /// Faster than this share of the planned speed is too fast.
    pub const TOO_FAST: f32 = 1.15;
}

/// Room behind a jump's takeoff at `from` for a run-up against `dir`, as much as `speed` needs.
pub fn run_up_room(tracer: &mut dyn Tracer, from: Vec3, dir: Vec2, speed: f32) -> f32 {
    let want = 40.0 + speed * 0.1;
    // A step's height up, so stairs behind the takeoff do not count as a wall.
    let start = from + Vec3::Z * 18.0;
    let tr = tracer.trace(&TraceQuery::hull(
        start,
        start - dir.extend(0.0) * want,
        HullKind::Stand,
    ));
    if tr.start_solid {
        0.0
    } else {
        (want * tr.fraction - 4.0).max(0.0)
    }
}

/// Command length of the simulation.
const STEP_MS: u8 = 10;
/// Horizontal miss still counted as arriving.
pub const ARRIVE_RADIUS: f32 = 32.0;
/// Height difference of the feet still counted as arriving.
pub const ARRIVE_DZ: f32 = 20.0;

/// A jump from `from` to `to` (standing hull centres on the floor).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct JumpQuery {
    pub from: Vec3,
    pub to: Vec3,
    /// Speed toward `to` at the takeoff.
    pub speed: f32,
    /// Duck once in the air (the feet rise 18 units).
    pub duck: bool,
    pub longjump: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MoveVerdict {
    pub ok: bool,
    /// Where the player came to rest (hull centre).
    pub landing: Vec3,
    /// Seconds in the air.
    pub flight: f32,
    /// Downward speed at the landing.
    pub impact: f32,
    /// Landed in water (no fall damage).
    pub in_water: bool,
}

fn flat_dir(from: Vec3, to: Vec3) -> Vec2 {
    (to - from).truncate().normalize_or_zero()
}

fn arrived(p: &Player, to: Vec3, radius: f32) -> bool {
    let feet_to = to.z - 36.0;
    p.on_ground() && (p.origin - to).truncate().length() < radius && (p.feet() - feet_to).abs() <= ARRIVE_DZ
}

fn cmd_toward(p: &Player, to: Vec3, buttons: u16) -> Cmd {
    let d = (to - p.origin).truncate();
    let mut angles = dir_to_view_angles(d.extend(0.0));
    angles.x = 0.0;
    let near = d.length() < 8.0;
    Cmd {
        angles,
        forward: if near { 0.0 } else { 400.0 },
        side: 0.0,
        up: 0.0,
        buttons: if near {
            buttons & !IN_FORWARD
        } else {
            buttons | IN_FORWARD
        },
        msec: STEP_MS,
    }
}

/// Places a standing player at `origin` and lets it settle onto the floor.
fn settled(world: &mut dyn MoveWorld, phys: &Physics, origin: Vec3) -> Player {
    let mut p = Player::standing(origin);
    let idle = Cmd {
        msec: STEP_MS,
        ..Cmd::default()
    };
    player_move(world, phys, &mut p, &idle);
    p
}

/// Jumps from `q.from` toward `q.to`, already moving at `q.speed`, and reports where the player landed.
pub fn simulate_jump(world: &mut dyn MoveWorld, phys: &Physics, q: &JumpQuery) -> MoveVerdict {
    let mut p = settled(world, phys, q.from);
    p.longjump = q.longjump;
    p.velocity = (flat_dir(q.from, q.to) * q.speed).extend(0.0);
    jump_from(world, phys, p, q)
}

/// A running jump the way a bot makes it: from rest `room` units behind `q.from`, run toward `q.to` at `q.speed`
/// and take off in the window once fast enough (`takeoff`).
pub fn simulate_run_jump(world: &mut dyn MoveWorld, phys: &Physics, q: &JumpQuery, room: f32) -> MoveVerdict {
    let dir = flat_dir(q.from, q.to);
    let mut p = settled(world, phys, q.from - dir.extend(0.0) * room);
    p.longjump = q.longjump;
    let short = room < takeoff::SHORT_RUN;
    let front = takeoff::WINDOW.1 + if short { takeoff::SHORT_REACH } else { 0.0 };
    let need = q.speed * if short { takeoff::SPEED_SHORT } else { takeoff::SPEED };
    let aim = q.from + dir.extend(0.0) * 32.0;
    for _ in 0..200 {
        let along = (p.origin - q.from).truncate().dot(dir);
        if along > front {
            break;
        }
        if along >= takeoff::WINDOW.0 && p.velocity.truncate().dot(dir) >= need && p.on_ground() {
            return jump_from(world, phys, p, q);
        }
        let mut cmd = cmd_toward(&p, aim, 0);
        cmd.forward = q.speed;
        player_move(world, phys, &mut p, &cmd);
    }
    MoveVerdict {
        ok: false,
        landing: p.origin,
        flight: 0.0,
        impact: 0.0,
        in_water: false,
    }
}

fn jump_from(world: &mut dyn MoveWorld, phys: &Physics, mut p: Player, q: &JumpQuery) -> MoveVerdict {
    let mut airborne = false;
    let mut flight = 0.0;
    let mut impact = 0.0;
    let mut jumped = false;
    for i in 0..250 {
        let mut buttons = 0;
        if i == 0 {
            buttons |= IN_JUMP;
            if q.longjump {
                buttons |= IN_DUCK;
            }
        }
        if airborne && (q.duck || q.longjump) {
            buttons |= IN_DUCK;
        }
        let cmd = cmd_toward(&p, q.to, buttons);
        let ev = player_move(world, phys, &mut p, &cmd);
        jumped |= ev.jumped;
        if !p.on_ground() && !p.on_ladder {
            airborne = true;
            flight += f32::from(STEP_MS) / 1000.0;
        }
        if let Some(v) = ev.landed {
            impact = v;
        }
        if airborne && p.on_ground() {
            break;
        }
        if !jumped && i > 0 {
            break;
        }
    }
    let mut ok = jumped && airborne && arrived(&p, q.to, ARRIVE_RADIUS);
    if !ok && jumped && airborne && p.on_ground() && (p.feet() - (q.to.z - 36.0)).abs() <= ARRIVE_DZ {
        // Landed on the right floor short of the node: walk the rest, stopping at it (not past an edge).
        for _ in 0..60 {
            if (q.to - p.origin).truncate().length() < 12.0 || !p.on_ground() {
                break;
            }
            let cmd = cmd_toward(&p, q.to, 0);
            player_move(world, phys, &mut p, &cmd);
        }
        ok = arrived(&p, q.to, ARRIVE_RADIUS);
    }
    MoveVerdict {
        ok,
        landing: p.origin,
        flight,
        impact,
        in_water: p.waterlevel > 0,
    }
}

/// Walks off a ledge from `from` (starting at rest) toward `to`, no faster than `speed`, and reports the landing.
pub fn simulate_drop(world: &mut dyn MoveWorld, phys: &Physics, from: Vec3, to: Vec3, speed: f32) -> MoveVerdict {
    let mut p = settled(world, phys, from);
    p.client_maxspeed = speed;
    let mut airborne = false;
    let mut flight = 0.0;
    let mut impact = 0.0f32;
    let limit = 400 + ((from - to).length() / 2.0) as usize;
    for _ in 0..limit {
        let cmd = cmd_toward(&p, to, 0);
        let ev = player_move(world, phys, &mut p, &cmd);
        if !p.on_ground() && !p.on_ladder {
            airborne = true;
            flight += f32::from(STEP_MS) / 1000.0;
        }
        if let Some(v) = ev.landed {
            impact = impact.max(v);
        }
        if airborne && p.on_ground() && (p.origin - to).truncate().length() < ARRIVE_RADIUS {
            break;
        }
    }
    MoveVerdict {
        ok: airborne && arrived(&p, to, ARRIVE_RADIUS),
        landing: p.origin,
        flight,
        impact,
        in_water: p.waterlevel > 0,
    }
}

/// Runs from `from` straight at `to` (holding forward, turning toward it every command, crouched when `duck`) and
/// reports where the player stopped. This is what a bot following the link would do.
pub fn simulate_walk(world: &mut dyn MoveWorld, phys: &Physics, from: Vec3, to: Vec3, duck: bool) -> MoveVerdict {
    let mut p = settled(world, phys, from);
    let buttons = if duck { IN_DUCK } else { 0 };
    let speed = if duck { phys.maxspeed * 0.333 } else { phys.maxspeed };
    let limit = ((from.distance(to) / speed.max(1.0) * 1.5 + 1.5) * 1000.0 / f32::from(STEP_MS)) as usize;
    let mut airborne = false;
    let mut flight = 0.0;
    let mut impact = 0.0f32;
    let mut best = f32::INFINITY;
    let mut stalled = 0;
    for _ in 0..limit {
        let cmd = cmd_toward(&p, to, buttons);
        let ev = player_move(world, phys, &mut p, &cmd);
        if !p.on_ground() && !p.on_ladder {
            airborne = true;
            flight += f32::from(STEP_MS) / 1000.0;
        }
        if let Some(v) = ev.landed {
            impact = impact.max(v);
        }
        let left = (to - p.origin).truncate().length();
        if left < 12.0 && p.on_ground() {
            break;
        }
        if left < best - 0.5 {
            best = left;
            stalled = 0;
        } else {
            stalled += 1;
            if stalled > 50 {
                break;
            }
        }
    }
    let _ = airborne;
    MoveVerdict {
        ok: arrived(&p, to, 20.0),
        landing: p.origin,
        flight,
        impact,
        in_water: p.waterlevel > 0,
    }
}

/// How a jump link is done: the least run-up speed and whether to duck in the air, with the share of perturbed
/// attempts (speed −10%, direction ±3°, takeoff ±8 units) that still land.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct JumpPlan {
    pub speed: f32,
    pub duck: bool,
    pub robustness: f32,
    pub flight: f32,
    pub impact: f32,
}

/// Run-up speeds a jump is tried with, slowest first.
pub const JUMP_SPEEDS: [f32; 5] = [0.0, 100.0, 150.0, 200.0, 270.0];
/// A plan at least this robust is taken as soon as found (the simplest first); otherwise the most robust one.
const ROBUST: f32 = 0.8;

/// Finds the easiest way to jump from `from` to `to`, if any.
pub fn plan_jump(world: &mut dyn MoveWorld, phys: &Physics, from: Vec3, to: Vec3) -> Option<JumpPlan> {
    let mut best: Option<JumpPlan> = None;
    for duck in [false, true] {
        for speed in JUMP_SPEEDS {
            let q = JumpQuery {
                from,
                to,
                speed: speed.min(phys.maxspeed),
                duck,
                longjump: false,
            };
            let v = if q.speed > 0.0 {
                let room = run_up_room(world, from, flat_dir(from, to), q.speed);
                simulate_run_jump(world, phys, &q, room)
            } else {
                simulate_jump(world, phys, &q)
            };
            if !v.ok {
                continue;
            }
            let robustness = jump_robustness(world, phys, &q);
            let plan = JumpPlan {
                speed: q.speed,
                duck,
                robustness,
                flight: v.flight,
                impact: v.impact,
            };
            if robustness >= ROBUST {
                return Some(plan);
            }
            if robustness >= 0.5 && best.as_ref().is_none_or(|b| robustness > b.robustness) {
                best = Some(plan);
            }
        }
    }
    best
}

fn jump_robustness(world: &mut dyn MoveWorld, phys: &Physics, q: &JumpQuery) -> f32 {
    let dir = flat_dir(q.from, q.to).extend(0.0);
    let side = Vec3::new(-dir.y, dir.x, 0.0);
    let dist = (q.to - q.from).truncate().length();
    // A bot takes off within 10% of the planned speed (a standing jump drifts a little).
    let speeds: &[f32] = if q.speed > 0.0 { &[0.9, 1.1] } else { &[] };
    let mut variants: Vec<JumpQuery> = speeds
        .iter()
        .map(|k| JumpQuery {
            speed: q.speed * k,
            ..*q
        })
        .collect();
    if q.speed == 0.0 {
        variants.push(JumpQuery { speed: 40.0, ..*q });
    }
    for along in [-8.0, 8.0] {
        variants.push(JumpQuery {
            from: q.from + dir * along,
            ..*q
        });
    }
    for deg in [-3.0f32, 3.0] {
        let off = side * (dist * lb_core::dmath::tan(deg.to_radians()));
        variants.push(JumpQuery { to: q.to + off, ..*q });
    }
    let ok = variants
        .iter()
        .filter(|v| {
            let r = simulate_jump(world, phys, v);
            r.ok && (r.landing - q.to).truncate().length() < ARRIVE_RADIUS * 1.5
        })
        .count();
    ok as f32 / variants.len() as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::boxworld::BoxWorld;

    fn ledge(height: f32) -> BoxWorld {
        let mut w = BoxWorld::new();
        w.floor(0.0, 2048.0);
        w.solid(Vec3::new(64.0, -256.0, 0.0), Vec3::new(512.0, 256.0, height));
        w
    }

    #[test]
    fn jumps_learn_their_run_up_and_duck() {
        let phys = Physics::default();
        // A 40-unit box: a standing jump from right in front does it.
        let mut w = ledge(40.0);
        let plan = plan_jump(&mut w, &phys, Vec3::new(40.0, 0.0, 36.0), Vec3::new(120.0, 0.0, 76.0)).unwrap();
        assert!(!plan.duck, "{plan:?}");
        // 60 units needs ducking in the air.
        let mut w = ledge(60.0);
        let plan = plan_jump(&mut w, &phys, Vec3::new(40.0, 0.0, 36.0), Vec3::new(120.0, 0.0, 96.0)).unwrap();
        assert!(plan.duck, "{plan:?}");
        // 70 is out of reach.
        let mut w = ledge(70.0);
        assert!(plan_jump(&mut w, &phys, Vec3::new(40.0, 0.0, 36.0), Vec3::new(120.0, 0.0, 106.0)).is_none());
    }

    #[test]
    fn gaps_need_speed() {
        let phys = Physics::default();
        let mut w = BoxWorld::new();
        w.solid(Vec3::new(-512.0, -256.0, -16.0), Vec3::new(0.0, 256.0, 0.0));
        w.solid(Vec3::new(160.0, -256.0, -16.0), Vec3::new(700.0, 256.0, 0.0));
        w.solid(Vec3::new(-1024.0, -1024.0, -600.0), Vec3::new(1024.0, 1024.0, -584.0));
        let plan = plan_jump(&mut w, &phys, Vec3::new(-8.0, 0.0, 36.0), Vec3::new(220.0, 0.0, 36.0)).unwrap();
        assert!(plan.speed >= 150.0, "a 160-unit gap needs a run-up: {plan:?}");
    }

    #[test]
    fn drops_report_the_landing_speed() {
        let phys = Physics::default();
        let mut w = BoxWorld::new();
        w.solid(Vec3::new(-512.0, -256.0, 284.0), Vec3::new(0.0, 256.0, 300.0));
        w.floor(0.0, 2048.0);
        let v = simulate_drop(
            &mut w,
            &phys,
            Vec3::new(-20.0, 0.0, 336.0),
            Vec3::new(80.0, 0.0, 36.0),
            150.0,
        );
        assert!(v.ok, "{v:?}");
        assert!((v.impact - phys.fall_speed(300.0)).abs() < 25.0, "{v:?}");
        assert_eq!(phys.fall_damage(v.impact), 10.0);
    }
}
