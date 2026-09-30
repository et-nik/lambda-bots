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

/// Room behind a jump's takeoff at `from` for a run-up against `dir`, as much as `speed` needs: clear of walls, and
/// with floor under it (not back off the ledge the jump starts from).
pub fn run_up_room(tracer: &mut dyn Tracer, from: Vec3, dir: Vec2, speed: f32) -> f32 {
    let want = 40.0 + speed * 0.1;
    // A step's height up, so stairs behind the takeoff do not count as a wall.
    let start = from + Vec3::Z * 18.0;
    let back = -dir.extend(0.0);
    let tr = tracer.trace(&TraceQuery::hull(start, start + back * want, HullKind::Stand));
    if tr.start_solid {
        return 0.0;
    }
    let clear = (want * tr.fraction - 4.0).max(0.0);
    let mut room = 0.0;
    while room + 8.0 <= clear {
        let p = start + back * (room + 8.0);
        // Stairs down behind the takeoff are run up; a ledge is not.
        let down = tracer.trace(&TraceQuery::hull(p, p - Vec3::Z * 58.0, HullKind::Stand));
        if down.start_solid || down.fraction >= 1.0 {
            break;
        }
        room += 8.0;
    }
    room
}

/// Command length of the simulation.
pub(crate) const STEP_MS: u8 = 10;
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
    /// Where it first came down, before walking the rest of the way (hull centre).
    pub touchdown: Vec3,
    /// Seconds in the air.
    pub flight: f32,
    /// Downward speed at the landing.
    pub impact: f32,
    /// Landed in water (no fall damage).
    pub in_water: bool,
}

pub(crate) fn flat_dir(from: Vec3, to: Vec3) -> Vec2 {
    (to - from).truncate().normalize_or_zero()
}

/// Where a move from `from` meant to come down at `to` (standing origins) came down instead (`down`), for people:
/// how far past, short of or to the side of the landing, and how far above or below it.
pub fn came_down(from: Vec3, to: Vec3, down: Vec3) -> String {
    let dir = flat_dir(from, to);
    let off = (down - to).truncate();
    let along = off.dot(dir);
    let across = (off - dir * along).length();
    let mut ways = Vec::new();
    if along > 8.0 {
        ways.push(format!("{along:.0} u past"));
    } else if along < -8.0 {
        ways.push(format!("{:.0} u short of", -along));
    }
    if across > 8.0 {
        ways.push(format!("{across:.0} u to the side of"));
    }
    let mut s = if ways.is_empty() {
        "comes down at the landing".to_string()
    } else {
        format!("comes down {} the landing", ways.join(" and "))
    };
    let dz = down.z - to.z;
    if dz < -ARRIVE_DZ {
        s.push_str(&format!(", {:.0} u below it", -dz));
    } else if dz > ARRIVE_DZ {
        s.push_str(&format!(", {dz:.0} u above it"));
    }
    s
}

pub(crate) fn arrived(p: &Player, to: Vec3, radius: f32) -> bool {
    let feet_to = to.z - 36.0;
    p.on_ground() && (p.origin - to).truncate().length() < radius && (p.feet() - feet_to).abs() <= ARRIVE_DZ
}

pub(crate) fn cmd_toward(p: &Player, to: Vec3, buttons: u16) -> Cmd {
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
pub(crate) fn settled(world: &mut dyn MoveWorld, phys: &Physics, origin: Vec3) -> Player {
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
        touchdown: p.origin,
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
    let touchdown = p.origin;
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
        touchdown,
        flight,
        impact,
        in_water: p.waterlevel > 0,
    }
}

/// Climbs a ladder from `start` (a spot on it) facing its face — `normal` points out of the ladder — looking up
/// (or down) and pressing forward, until the player steps off at the top (or reaches the floor at its foot); returns
/// where it stands then.
pub fn simulate_climb(world: &mut dyn MoveWorld, phys: &Physics, start: Vec3, normal: Vec3, up: bool) -> Option<Vec3> {
    let mut p = Player::standing(start);
    let mut angles = dir_to_view_angles(-normal);
    angles.x = if up { -45.0 } else { 45.0 };
    let cmd = Cmd {
        angles,
        forward: 400.0,
        buttons: IN_FORWARD,
        msec: STEP_MS,
        ..Cmd::default()
    };
    let mut climbed = false;
    let mut settled = 0;
    let mut stuck = 0;
    for _ in 0..600 {
        let before = p.origin;
        player_move(world, phys, &mut p, &cmd);
        climbed |= p.on_ladder;
        // Up against the top on the ladder: the ledge is behind or beside it (a ladder through a hole in the floor).
        stuck = if up && p.on_ladder && p.origin.distance(before) < 0.05 {
            stuck + 1
        } else {
            0
        };
        if stuck >= 10 {
            return step_off(world, phys, p, normal);
        }
        // Up: off the ladder and on the ledge. Down: standing on the floor below it (on the ladder a player is not
        // on the ground, it just stops there).
        let there = if up {
            !p.on_ladder && p.on_ground()
        } else {
            p.on_ground() || p.origin.distance(before) < 0.05
        };
        if climbed && there {
            settled += 1;
            if settled >= 10 {
                if up || p.on_ground() {
                    return Some(p.origin);
                }
                // Stopped at the ladder's bottom: the floor is right below, a short drop at most.
                let tr = world.trace(&lb_worldq::TraceQuery::hull(
                    p.origin,
                    p.origin - Vec3::Z * 64.0,
                    p.hull(),
                ));
                return (tr.fraction < 1.0 && !tr.start_solid).then_some(tr.end);
            }
        } else {
            settled = 0;
        }
    }
    None
}

/// Off the top of a ladder the climb cannot get over: backing away from it or sidestepping onto a floor no lower
/// than a step below the feet.
fn step_off(world: &mut dyn MoveWorld, phys: &Physics, at: Player, normal: Vec3) -> Option<Vec3> {
    let flat = Vec3::new(normal.x, normal.y, 0.0).normalize_or_zero();
    let side = Vec3::Z.cross(flat);
    for dir in [flat, side, -side] {
        let mut p = at;
        let cmd = Cmd {
            angles: dir_to_view_angles(dir),
            forward: 400.0,
            buttons: IN_FORWARD,
            msec: STEP_MS,
            ..Cmd::default()
        };
        for _ in 0..100 {
            player_move(world, phys, &mut p, &cmd);
            if !p.on_ladder && p.on_ground() {
                break;
            }
        }
        if !p.on_ladder && p.on_ground() && p.feet() >= at.feet() - 48.0 {
            return Some(p.origin);
        }
    }
    None
}

/// Swims from `from` to `to` the way a bot does: looking at the target and pressing forward, and jump while the
/// target is higher and the player in water (it rises; at the surface the jump climbs out onto a ledge). Starts
/// and ends on land too: walking into the water, or climbing out of it.
pub fn simulate_swim(world: &mut dyn MoveWorld, phys: &Physics, from: Vec3, to: Vec3) -> MoveVerdict {
    let mut p = Player::standing(from);
    let limit = ((from.distance(to) / 100.0 + 3.0) * 1000.0 / f32::from(STEP_MS)) as usize;
    let mut best = f32::INFINITY;
    let mut stalled = 0;
    let to_land = on_land(world, to);
    for _ in 0..limit {
        let cmd = swim_cmd(&p, to, to_land);
        player_move(world, phys, &mut p, &cmd);
        if swum(&p, to) {
            return MoveVerdict {
                ok: true,
                landing: p.origin,
                touchdown: p.origin,
                flight: 0.0,
                impact: 0.0,
                in_water: p.waterlevel > 0,
            };
        }
        let left = (to - p.origin).length();
        if left < best - 0.5 {
            best = left;
            stalled = 0;
        } else {
            stalled += 1;
            if stalled > 150 {
                break;
            }
        }
    }
    MoveVerdict {
        ok: false,
        landing: p.origin,
        touchdown: p.origin,
        flight: 0.0,
        impact: 0.0,
        in_water: p.waterlevel > 0,
    }
}

/// Whether a swimmer heading for `to` presses jump: in the water, to rise toward a higher target or to the surface
/// to climb out onto land; on land, to hop over a lip into the water.
pub fn swim_jump(origin: Vec3, velocity: Vec3, waterlevel: u8, on_ground: bool, to: Vec3, to_land: bool) -> bool {
    if waterlevel >= 2 {
        to.z > origin.z + 24.0 || (to_land && to.z > origin.z - 48.0)
    } else {
        on_ground && !to_land && velocity.truncate().length() < 60.0
    }
}

/// Swimming at `to`: looking at it and pressing forward, jumping as `swim_jump` says (the player rises, and at the
/// surface the jump climbs out onto a ledge).
fn swim_cmd(p: &Player, to: Vec3, to_land: bool) -> Cmd {
    let mut buttons = IN_FORWARD;
    if swim_jump(p.origin, p.velocity, p.waterlevel, p.on_ground(), to, to_land) {
        buttons |= IN_JUMP;
    }
    Cmd {
        angles: dir_to_view_angles(to - p.origin),
        forward: 400.0,
        buttons,
        msec: STEP_MS,
        ..Cmd::default()
    }
}

/// `to` (a hull centre) is out of the water.
fn on_land(world: &mut dyn MoveWorld, to: Vec3) -> bool {
    let c = world.point_contents(to);
    !(c <= lb_worldq::contents::WATER && c > lb_worldq::contents::TRANSLUCENT)
}

/// Got to `to` swimming: standing there, floating there, or holding on to the ladder that rises out of the water.
fn swum(p: &Player, to: Vec3) -> bool {
    arrived(p, to, ARRIVE_RADIUS)
        || (p.waterlevel >= 2 && (to - p.origin).length() < 24.0)
        || (p.on_ladder && (to - p.origin).length() < 32.0)
}

/// Air control toward `target` (a hull centre): the horizontal direction to press so the player comes down on it,
/// `None` to press nothing. The air gives up to 30 units/s along the pressed direction but takes away any amount,
/// so steering mostly brakes what would carry the player past. On the way up the fall may turn out shorter than it
/// looks (a ceiling ends the rise), so it only keeps the player on the line to the target until it comes down.
pub fn air_steer(origin: Vec3, velocity: Vec3, target: Vec3, gravity: f32) -> Option<Vec2> {
    let off = (target - origin).truncate();
    let v = velocity.truncate();
    let vz = velocity.z;
    let disc = vz * vz + 2.0 * gravity * (origin.z - target.z);
    let err = if vz > 0.0 || disc < 0.0 {
        let dir = off.normalize_or_zero();
        let along = v.dot(dir);
        -(v - dir * along) + dir * (30.0 - along).max(0.0)
    } else {
        let t = (vz + disc.sqrt()) / gravity;
        off / t.max(0.05) - v
    };
    (err.length() > 10.0).then(|| err.normalize())
}

/// A run into a push field (`trigger_push`): from rest at `entry`, along `dir`, jumping on the way where the field is
/// entered over a mound or a gap.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PushRun {
    /// Standing hull centre on the floor.
    pub entry: Vec3,
    pub dir: Vec2,
    /// Jump this far along the run.
    pub jump_at: Option<f32>,
    /// Keep over this spot while a field lifts the player: ride a lift shaft up its middle instead of drifting out
    /// of its side or under a ledge.
    pub hold: Option<Vec2>,
    /// Steered for in the air (`air_steer`), swum or walked to after; `None` leaves the flight to the push.
    pub target: Option<Vec3>,
}

/// Keeping over `spot` in the air: coasting toward it and braking to stop there (the air takes away speed fast but
/// gives only 30 units/s), braking any motion off the line to it.
pub fn hover(origin: Vec3, velocity: Vec3, spot: Vec2) -> Option<Vec2> {
    let off = spot - origin.truncate();
    let d = off.length();
    let want = if d > 1.0 {
        off / d * (2.0 * HOVER_BRAKE * d).sqrt().min(320.0)
    } else {
        Vec2::ZERO
    };
    let err = want - velocity.truncate();
    (err.length() > 10.0).then(|| err.normalize())
}

/// Braking `hover` counts on, units/s² (the air brakes up to `airaccelerate` × speed = 3200).
const HOVER_BRAKE: f32 = 2000.0;

/// Longest a push run is followed, seconds.
const PUSH_TIME: f32 = 15.0;
/// A run that gets no nearer its target for this long in the air (or a second after landing) has failed.
const PUSH_STALL: f32 = 3.5;
/// A field pushing up at least this fast is lifting the player.
pub const LIFTING: f32 = 100.0;

/// Runs into a push field: whether the player gets to the target the way a bot would (steering in the air, then
/// walking or swimming the rest); without a target, where the push throws it.
pub fn simulate_push(world: &mut dyn MoveWorld, phys: &Physics, run: &PushRun) -> MoveVerdict {
    push_run(world, phys, run, true)
}

/// Where a push throws a player steering for the target in the air (or not steering without one): `ok` once it
/// comes to rest out of the field, `landing` where.
pub fn probe_push(world: &mut dyn MoveWorld, phys: &Physics, run: &PushRun) -> MoveVerdict {
    push_run(world, phys, run, false)
}

fn push_run(world: &mut dyn MoveWorld, phys: &Physics, run: &PushRun, reach: bool) -> MoveVerdict {
    let tick = f32::from(STEP_MS) / 1000.0;
    let mut p = settled(world, phys, run.entry);
    let to_land = run.target.is_some_and(|t| on_land(world, t));
    let yaw = dir_to_view_angles(run.dir.extend(0.0));
    let (mut pushed, mut jumped, mut landed) = (false, false, false);
    let mut flight = 0.0;
    let mut impact = 0.0f32;
    let mut best = f32::INFINITY;
    let mut stalled = 0;
    let verdict = |p: &Player, ok: bool, flight: f32, impact: f32| MoveVerdict {
        ok,
        landing: p.origin,
        touchdown: p.origin,
        flight,
        impact,
        in_water: p.waterlevel > 0,
    };
    for i in 0..(PUSH_TIME / tick) as usize {
        let in_field = p.field != Vec3::ZERO;
        if in_field && landed {
            // Back in the field after coming down: round and round.
            return verdict(&p, false, flight, impact);
        }
        pushed |= in_field;
        let airborne = !p.on_ground() && !p.on_ladder && p.waterlevel < 2;
        let mut cmd = Cmd {
            angles: yaw,
            msec: STEP_MS,
            ..Cmd::default()
        };
        if !pushed {
            let along = (p.origin - run.entry).truncate().dot(run.dir);
            if i as f32 * tick > 3.0 {
                return verdict(&p, false, flight, impact);
            }
            let jump = !jumped && p.on_ground() && run.jump_at.is_some_and(|j| along >= j);
            jumped |= jump;
            cmd.forward = 400.0;
            cmd.buttons = IN_FORWARD | if jump { IN_JUMP } else { 0 };
        } else if let Some(spot) = run.hold.filter(|_| p.field.z > LIFTING) {
            if let Some(d) = hover(p.origin, p.velocity, spot) {
                cmd.angles = dir_to_view_angles(d.extend(0.0));
                cmd.forward = 400.0;
                cmd.buttons = IN_FORWARD;
            }
        } else if let Some(target) = run.target.filter(|_| reach || !landed) {
            if p.waterlevel >= 2 {
                cmd = swim_cmd(&p, target, to_land);
            } else if airborne {
                if let Some(d) = air_steer(p.origin, p.velocity, target, phys.gravity) {
                    cmd.angles = dir_to_view_angles(d.extend(0.0));
                    cmd.forward = 400.0;
                    cmd.buttons = IN_FORWARD;
                }
            } else {
                cmd = cmd_toward(&p, target, 0);
            }
        } else if in_field && !airborne {
            // Carried along the floor: keep going with it.
            cmd.forward = 400.0;
            cmd.buttons = IN_FORWARD;
        }
        let ev = player_move(world, phys, &mut p, &cmd);
        if !p.on_ground() && !p.on_ladder && p.waterlevel < 2 {
            flight += tick;
        }
        if pushed && let Some(v) = ev.landed {
            impact = impact.max(v);
        }
        let out = p.field == Vec3::ZERO;
        if pushed && out && (p.on_ground() || p.waterlevel >= 2) {
            landed = true;
        }
        match run.target.filter(|_| reach) {
            Some(target) => {
                if pushed && swum(&p, target) {
                    return verdict(&p, true, flight, impact);
                }
                if pushed {
                    let left = (target - p.origin).length();
                    if left < best - 0.5 {
                        best = left;
                        stalled = 0;
                    } else {
                        stalled += 1;
                        let limit = if landed { 1.0 } else { PUSH_STALL };
                        if stalled as f32 * tick > limit {
                            return verdict(&p, false, flight, impact);
                        }
                    }
                }
            }
            None => {
                // At rest, or sinking slowly in the water.
                let slow = p.velocity.truncate().length() < 20.0 && p.velocity.z.abs() < 80.0;
                if landed && slow && (p.on_ground() || p.waterlevel >= 2) {
                    return verdict(&p, true, flight, impact);
                }
            }
        }
    }
    verdict(&p, false, flight, impact)
}

/// A bot walking off a ledge aims this far past the landing, so it does not stop on the ledge right above it.
pub const DROP_OVERRUN: f32 = 48.0;
/// A drop that touches down within this of its landing walks the rest; farther off it has failed.
pub const DROP_SLACK: f32 = 96.0;

/// Walks off a ledge from `from` (starting at rest) toward `to`, no faster than `speed`, and reports the landing:
/// on the ledge heading past the landing (`DROP_OVERRUN`), in the air at it. Touching down more than `DROP_SLACK`
/// from it is a miss, as for the bot.
pub fn simulate_drop(world: &mut dyn MoveWorld, phys: &Physics, from: Vec3, to: Vec3, speed: f32) -> MoveVerdict {
    let mut p = settled(world, phys, from);
    p.client_maxspeed = speed;
    let mut airborne = false;
    let mut touchdown: Option<Vec3> = None;
    let mut flight = 0.0;
    let mut impact = 0.0f32;
    let beyond = to + flat_dir(from, to).extend(0.0) * DROP_OVERRUN;
    let limit = 400 + ((from - to).length() / 2.0) as usize;
    for _ in 0..limit {
        let cmd = cmd_toward(&p, if airborne { to } else { beyond }, 0);
        let ev = player_move(world, phys, &mut p, &cmd);
        if !p.on_ground() && !p.on_ladder {
            airborne = true;
            flight += f32::from(STEP_MS) / 1000.0;
        }
        if let Some(v) = ev.landed {
            impact = impact.max(v);
        }
        if airborne && (p.on_ground() || p.waterlevel >= 2) {
            touchdown.get_or_insert(p.origin);
        }
        if airborne && p.on_ground() && (p.origin - to).truncate().length() < ARRIVE_RADIUS {
            break;
        }
    }
    let near = touchdown.is_some_and(|t| (t - to).truncate().length() < DROP_SLACK);
    MoveVerdict {
        ok: airborne && near && arrived(&p, to, ARRIVE_RADIUS),
        landing: p.origin,
        touchdown: touchdown.unwrap_or(p.origin),
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
        touchdown: p.origin,
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

/// Why no jump from `from` onto `to` holds: where the farthest-reaching one (the fastest run-up, ducking) comes down,
/// or that jumps land only in some of the tries a little off.
pub fn why_no_jump(world: &mut dyn MoveWorld, phys: &Physics, from: Vec3, to: Vec3) -> String {
    let q = JumpQuery {
        from,
        to,
        speed: JUMP_SPEEDS[JUMP_SPEEDS.len() - 1].min(phys.maxspeed),
        duck: true,
        longjump: false,
    };
    let room = run_up_room(world, from, flat_dir(from, to), q.speed);
    let v = simulate_run_jump(world, phys, &q, room);
    if !v.ok {
        return format!("a running jump, ducking, {}", came_down(from, to, v.touchdown));
    }
    "jumps land in fewer than half of the tries a little off (the speed 10%, the takeoff 8 u, 3° across)".into()
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
