//! Long jumps and gauss boosts the way a bot makes them: for the links that use them, and for the checks a bot makes
//! before it takes one on the way.
//!
//! - **Long jump.** With the module, duck and jump pressed together on the ground while moving faster than 50
//!   units/s set the horizontal speed to 560 along the view (less looking up or down) and throw the player 56 units
//!   up: about 420 units over flat ground in three quarters of a second. The bot holds duck in the air and steers
//!   onto its landing; the air takes speed away fast, so a long jump comes down anywhere short of its full reach.
//! - **Gauss boost.** A charged gauss shot pushes its shooter back at five times its damage, up and down too in
//!   multiplayer. Looking back and 30–38° down, a full charge let go as the bot jumps adds some 850 units/s forward
//!   and 550 up: a flight of over a thousand units, or onto a ledge a few hundred units up. The damage grows evenly
//!   over the 1.5 s of a full charge and the game lets a charge go after half a second at the soonest, so a boost is
//!   charged for the push it needs, a third of a full one or more: a lower arc under ceilings, a shorter fall, an
//!   unsteered flight that ends near the landing.

use lb_core::input::{IN_DUCK, IN_FORWARD, IN_JUMP};
use lb_core::math::{dir_to_view_angles, view_angle_vectors};
use lb_core::{Vec2, Vec3};
use lb_worldq::{HullKind, Trace, TraceQuery, Tracer, contents};
use smallvec::SmallVec;

use crate::physics::Physics;
use crate::pmove::{Cmd, JUMP_SPEED, LONGJUMP_SPEED, LONGJUMP_UP, Ladder, MoveWorld, Player, player_move};
use crate::validate::{
    ARRIVE_DZ, ARRIVE_RADIUS, MoveVerdict, STEP_MS, air_steer, arrived, came_down, cmd_toward, flat_dir, settled,
};

/// A long jump is taken moving at least this fast (the game wants more than 50 units/s).
pub const LONGJUMP_TAKEOFF: f32 = 100.0;
/// Ducked in the air, the player's feet come up this far.
const TUCK: f32 = 18.0;

/// A flight is followed this long at most, seconds.
const FLIGHT_LIMIT: f32 = 4.0;
/// Commands a player that came down short on the landing's floor walks the rest in, at most.
const WALK_REST: usize = 60;

/// A world known only through traces (the live server's): no ladders, no push fields. Enough to follow a flight.
pub struct Traced<'a>(pub &'a mut dyn Tracer);

impl Tracer for Traced<'_> {
    fn trace(&mut self, q: &TraceQuery) -> Trace {
        self.0.trace(q)
    }

    fn point_contents(&mut self, p: Vec3) -> i32 {
        self.0.point_contents(p)
    }
}

impl MoveWorld for Traced<'_> {
    fn ladder(&mut self, _origin: Vec3, _hull: HullKind) -> Option<Ladder> {
        None
    }
}

/// Lava or slime at the player's feet: a landing there hurts on and on.
pub fn hazard(world: &mut dyn MoveWorld, p: &Player) -> bool {
    let c = world.point_contents(Vec3::new(p.origin.x, p.origin.y, p.feet() + 2.0));
    c == contents::LAVA || c == contents::SLIME
}

/// Lava or slime where a long jump followed by [`longjump_from`] came down (`landing`, the hull centre): the player
/// lands ducked, holding duck in the air.
pub fn lands_in_hazard(world: &mut dyn MoveWorld, landing: Vec3) -> bool {
    let mut p = Player::standing(landing);
    p.ducked = true;
    hazard(world, &p)
}

fn verdict(p: &Player, ok: bool, flight: f32, impact: f32) -> MoveVerdict {
    MoveVerdict {
        ok,
        landing: p.origin,
        touchdown: p.origin,
        flight,
        impact,
        in_water: p.waterlevel > 0,
    }
}

/// Follows a flight from `p` until it comes down (on the ground, in the water or on a ladder), holding `hold` and
/// steering onto `to` when given; a player that came down short on the floor of `to` walks the rest, as the bot
/// does. `ok` says it got to `to`, or without one that it came down at all.
fn fly(world: &mut dyn MoveWorld, phys: &Physics, mut p: Player, yaw: f32, to: Option<Vec3>, hold: u16) -> MoveVerdict {
    let tick = f32::from(STEP_MS) / 1000.0;
    let mut flight = 0.0;
    let mut impact = 0.0f32;
    let mut down = false;
    for _ in 0..(FLIGHT_LIMIT / tick) as usize {
        let mut cmd = Cmd {
            angles: Vec3::new(0.0, yaw, 0.0),
            buttons: hold,
            msec: STEP_MS,
            ..Cmd::default()
        };
        if let Some(d) = to.and_then(|t| air_steer(p.origin, p.velocity, t, phys.gravity)) {
            cmd.angles = dir_to_view_angles(d.extend(0.0));
            cmd.forward = 400.0;
            cmd.buttons |= IN_FORWARD;
        }
        let ev = player_move(world, phys, &mut p, &cmd);
        if let Some(v) = ev.landed {
            impact = impact.max(v);
        }
        if p.on_ground() || p.waterlevel >= 2 || p.on_ladder {
            down = true;
            break;
        }
        flight += tick;
    }
    let Some(to) = to else {
        return verdict(&p, down, flight, impact);
    };
    let touchdown = p.origin;
    if down && !arrived(&p, to, ARRIVE_RADIUS) && p.on_ground() && (p.feet() - (to.z - 36.0)).abs() <= ARRIVE_DZ {
        for _ in 0..WALK_REST {
            if (to - p.origin).truncate().length() < 12.0 || !p.on_ground() {
                break;
            }
            let cmd = cmd_toward(&p, to, 0);
            player_move(world, phys, &mut p, &cmd);
        }
    }
    let ok = down && arrived(&p, to, ARRIVE_RADIUS);
    MoveVerdict {
        touchdown,
        ..verdict(&p, ok, flight, impact)
    }
}

/// A long jump taken now by `p` (on the ground, moving faster than 50 units/s), looking level along `yaw`, with the
/// flight steered onto `to` when given. Not a long jump at all (too slow, not on the ground) is a failure.
pub fn longjump_from(
    world: &mut dyn MoveWorld,
    phys: &Physics,
    mut p: Player,
    yaw: f32,
    to: Option<Vec3>,
) -> MoveVerdict {
    p.longjump = true;
    // Both keys pressed afresh: the command before let them go.
    p.oldbuttons &= !(IN_DUCK | IN_JUMP);
    let takeoff = Cmd {
        angles: Vec3::new(0.0, yaw, 0.0),
        buttons: IN_DUCK | IN_JUMP,
        msec: STEP_MS,
        ..Cmd::default()
    };
    if !player_move(world, phys, &mut p, &takeoff).longjumped {
        return verdict(&p, false, 0.0, 0.0);
    }
    fly(world, phys, p, yaw, to, IN_DUCK)
}

/// How far a long jump taken looking level carries at most onto a floor `dz` units above the takeoff's (below when
/// negative), ducked in the air; `None` when it cannot get up there.
pub fn longjump_reach(dz: f32, gravity: f32) -> Option<f32> {
    let disc = LONGJUMP_UP * LONGJUMP_UP - 2.0 * gravity * (dz - TUCK);
    (disc >= 0.0).then(|| LONGJUMP_SPEED * (LONGJUMP_UP + disc.sqrt()) / gravity)
}

/// The top of a long jump: how far along it comes at full speed, and how high over the takeoff its origin gets.
pub fn longjump_top(gravity: f32) -> (f32, f32) {
    (
        LONGJUMP_SPEED * LONGJUMP_UP / gravity,
        LONGJUMP_UP * LONGJUMP_UP / (2.0 * gravity),
    )
}

/// Yaw of looking from `from` at `to`.
fn yaw_to(from: Vec3, to: Vec3) -> f32 {
    dir_to_view_angles((to - from).truncate().extend(0.0)).y
}

/// A long jump from `from` onto `to` (standing hull centres on the floor) the way the link's executor makes it:
/// from rest a run-up behind the takeoff, running at `to`, then at `from` the jump looking at `to` and the flight
/// steered onto it. `offset` shifts the takeoff across the jump (checks of how precise it must be).
pub fn simulate_longjump(world: &mut dyn MoveWorld, phys: &Physics, from: Vec3, to: Vec3, offset: f32) -> MoveVerdict {
    let dir = flat_dir(from, to);
    let side = Vec2::new(-dir.y, dir.x) * offset;
    let start = from - dir.extend(0.0) * 16.0 + side.extend(0.0);
    let mut p = settled(world, phys, start);
    for _ in 0..100 {
        let along = (p.origin - from).truncate().dot(dir);
        if p.on_ground() && p.velocity.truncate().length() >= LONGJUMP_TAKEOFF && along >= -2.0 {
            return longjump_from(world, phys, p, yaw_to(p.origin, to), Some(to));
        }
        if along > 16.0 {
            break;
        }
        let mut cmd = cmd_toward(&p, to, 0);
        cmd.forward = phys.maxspeed;
        player_move(world, phys, &mut p, &cmd);
    }
    verdict(&p, false, 0.0, 0.0)
}

/// How a trick link is done: the view's pitch it needs (the gauss boost's look down) and the push of the boost's
/// charge, the share of perturbed attempts that still land, how long it flies and how hard it comes down.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrickPlan {
    pub pitch: f32,
    /// A boost's recoil, units/s (0 for a long jump).
    pub push: f32,
    pub robustness: f32,
    pub flight: f32,
    pub impact: f32,
}

/// A plan at least this robust is taken.
pub const TRICK_ROBUST: f32 = 0.75;

/// The long jump from `from` onto `to`, if one gets there surely: the takeoff anywhere within 12 units along the
/// jump and 8 across lands too (a long jump comes down short of its reach only by braking: near the reach a takeoff a
/// little early falls short).
pub fn plan_longjump(world: &mut dyn MoveWorld, phys: &Physics, from: Vec3, to: Vec3) -> Option<TrickPlan> {
    let v = simulate_longjump(world, phys, from, to, 0.0);
    if !v.ok {
        return None;
    }
    let dir = flat_dir(from, to).extend(0.0);
    let variants = [
        (dir * -12.0, 0.0),
        (dir * 12.0, 0.0),
        (Vec3::ZERO, -8.0),
        (Vec3::ZERO, 8.0),
    ];
    let landed = variants
        .iter()
        .filter(|(along, side)| simulate_longjump(world, phys, from + *along, to + *along, *side).ok)
        .count();
    let robustness = landed as f32 / variants.len() as f32;
    (landed == variants.len()).then_some(TrickPlan {
        pitch: 0.0,
        push: 0.0,
        robustness,
        flight: v.flight,
        impact: v.impact,
    })
}

/// A gauss boost: from standing at `from`, looking back against `dir` and `pitch` degrees down, the bot jumps and
/// lets a charged shot go on the next command; the recoil (`push`: five times the shot's damage) throws it along
/// `dir` and up. The flight is steered onto `to` when given.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoostQuery {
    pub from: Vec3,
    pub dir: Vec2,
    pub pitch: f32,
    pub push: f32,
    pub to: Option<Vec3>,
}

/// View angles of a boost along `dir`: looking back against it, `pitch` degrees down.
pub fn boost_view(dir: Vec2, pitch: f32) -> Vec3 {
    let mut angles = dir_to_view_angles(-dir.extend(0.0));
    angles.x = pitch;
    angles
}

/// Speed a full charge pushes its shooter back with: five times the shot's 200 damage.
pub const FULL_PUSH: f32 = 1000.0;

pub fn simulate_boost(world: &mut dyn MoveWorld, phys: &Physics, q: &BoostQuery) -> MoveVerdict {
    let mut p = settled(world, phys, q.from);
    if !p.on_ground() {
        return verdict(&p, false, 0.0, 0.0);
    }
    let view = boost_view(q.dir, q.pitch);
    let jump = Cmd {
        angles: view,
        buttons: IN_JUMP,
        msec: STEP_MS,
        ..Cmd::default()
    };
    player_move(world, phys, &mut p, &jump);
    // The buttons come up on the next command: the charge goes after its move.
    let release = Cmd {
        angles: view,
        msec: STEP_MS,
        ..Cmd::default()
    };
    player_move(world, phys, &mut p, &release);
    let (forward, _, _) = view_angle_vectors(view);
    p.velocity -= forward * q.push;
    fly(world, phys, p, dir_to_view_angles(q.dir.extend(0.0)).y, q.to, 0)
}

/// Seconds a charge takes to build fully in multiplayer (`CGauss::GetFullChargeTime`); its damage, and so its push,
/// grows evenly until then.
pub const FULL_CHARGE: f32 = 1.5;
/// The game lets a charge go no sooner than this after it starts: a boost pushes a third of a full one at least.
pub const MIN_CHARGE: f32 = 0.5;

/// Seconds to charge the gauss for a boost of `push` when a full charge pushes `full` (five times its damage).
pub fn charge_for(push: f32, full: f32) -> f32 {
    (FULL_CHARGE * push / full.max(1.0)).clamp(MIN_CHARGE, FULL_CHARGE)
}

/// Pitches a boost is tried with: the shallow ones throw far, the steep ones high.
pub const BOOST_PITCHES: [f32; 11] = [30.0, 34.0, 38.0, 42.0, 46.0, 50.0, 54.0, 58.0, 62.0, 66.0, 70.0];
/// A boost is pushed to come down unsteered this far past its landing, and this share of the way further: the air
/// steering brakes the rest, and a charge let go a little early still gets there.
const OVERSHOOT: f32 = 32.0;
const OVERSHOOT_SHARE: f32 = 0.08;
/// The top of a boost's arc clears its landing by this much at least.
const APEX_CLEAR: f32 = 24.0;
/// A boost's steered flight comes down this near its landing at most (flat).
const BOOST_TOUCHDOWN: f32 = 64.0;

/// The least push that throws a boost looking `pitch` degrees down onto a landing `along` units away and `up` units
/// above the takeoff (standing origins), with the overshoot the steering brakes, in the open. `None` when a full
/// charge's push (`full`) does not.
pub fn boost_push(gravity: f32, pitch: f32, along: f32, up: f32, full: f32) -> Option<f32> {
    let (s, c) = lb_core::dmath::sin_cos(pitch.to_radians());
    let want = along * (1.0 + OVERSHOOT_SHARE) + OVERSHOOT;
    let reach = |push: f32| {
        let vz = JUMP_SPEED + push * s;
        if vz * vz < 2.0 * gravity * (up + APEX_CLEAR) {
            return 0.0;
        }
        push * c * (vz + (vz * vz - 2.0 * gravity * up).sqrt()) / gravity
    };
    if reach(full) < want {
        return None;
    }
    let (mut lo, mut hi) = (0.0f32, full);
    for _ in 0..24 {
        let mid = 0.5 * (lo + hi);
        if reach(mid) >= want {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    Some(hi.max(full * MIN_CHARGE / FULL_CHARGE))
}

/// The pitches and pushes a boost from `from` onto `to` is tried with when a full charge pushes `full`: every pitch
/// that can throw that far and high, with the least push it takes (`boost_push`), the gentlest first.
pub fn boost_tries(gravity: f32, from: Vec3, to: Vec3, full: f32) -> SmallVec<[(f32, f32); 11]> {
    let along = (to - from).truncate().length();
    let mut tries: SmallVec<[(f32, f32); 11]> = BOOST_PITCHES
        .iter()
        .filter_map(|&pitch| boost_push(gravity, pitch, along, to.z - from.z, full).map(|push| (pitch, push)))
        .collect();
    tries.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.total_cmp(&b.0)));
    tries
}

/// The boost from `from` onto `to` looking `pitch` degrees down with `push`, if it gets there reliably: its steered
/// flight still lands with the view off by 3° across or 2° down, the takeoff 8 units along the way or the push 4% off
/// (no more than `full`), and it comes down somewhere safe unsteered too (the steering may stop when a fight takes
/// the bot's mind off it).
pub fn check_boost(
    world: &mut dyn MoveWorld,
    phys: &Physics,
    from: Vec3,
    to: Vec3,
    (pitch, push): (f32, f32),
    full: f32,
) -> Option<TrickPlan> {
    try_boost(world, phys, from, to, (pitch, push), full).ok()
}

/// `check_boost`, saying what fails when the boost does not hold.
pub fn try_boost(
    world: &mut dyn MoveWorld,
    phys: &Physics,
    from: Vec3,
    to: Vec3,
    (pitch, push): (f32, f32),
    full: f32,
) -> Result<TrickPlan, String> {
    let dir = flat_dir(from, to);
    let q = BoostQuery {
        from,
        dir,
        pitch,
        push,
        to: Some(to),
    };
    // Down near the landing: a flight that hits an edge on the way and walks the rest is not one to count on.
    let lands = |v: &MoveVerdict| v.ok && (v.touchdown - to).truncate().length() <= BOOST_TOUCHDOWN;
    let v = simulate_boost(world, phys, &q);
    if !lands(&v) {
        return Err(format!("the flight {}", came_down(from, to, v.touchdown)));
    }
    let turn = |deg: f32| {
        let (s, c) = lb_core::dmath::sin_cos(deg.to_radians());
        Vec2::new(dir.x * c - dir.y * s, dir.x * s + dir.y * c)
    };
    let variants = [
        BoostQuery { dir: turn(-3.0), ..q },
        BoostQuery { dir: turn(3.0), ..q },
        BoostQuery {
            pitch: pitch - 2.0,
            ..q
        },
        BoostQuery {
            pitch: pitch + 2.0,
            ..q
        },
        BoostQuery {
            from: from - dir.extend(0.0) * 8.0,
            ..q
        },
        BoostQuery {
            from: from + dir.extend(0.0) * 8.0,
            ..q
        },
        BoostQuery { push: push * 0.96, ..q },
        BoostQuery {
            push: (push * 1.04).min(full),
            ..q
        },
    ];
    let landed = variants
        .iter()
        .filter(|v| lands(&simulate_boost(world, phys, v)))
        .count();
    let robustness = landed as f32 / variants.len() as f32;
    if robustness < TRICK_ROBUST {
        return Err(format!(
            "only {landed} of {} tries a little off (the view 2–3°, the takeoff 8 u, the push 4%) land",
            variants.len()
        ));
    }
    let free = simulate_boost(world, phys, &BoostQuery { to: None, ..q });
    if !free.ok {
        return Err("unsteered, the flight does not come down".into());
    }
    let hurt = phys.fall_damage(free.impact);
    if hurt > phys.fall_damage(v.impact).max(10.0) {
        return Err(format!("unsteered, it comes down hard ({hurt:.0} damage)"));
    }
    let mut p = settled(world, phys, free.landing);
    p.origin = free.landing;
    if hazard(world, &p) {
        return Err("unsteered, it comes down in lava or slime".into());
    }
    Ok(TrickPlan {
        pitch,
        push,
        robustness,
        flight: v.flight,
        impact: v.impact,
    })
}

/// Why no boost from `from` onto `to` holds when a full charge pushes `full`: what fails for the gentlest try.
pub fn why_no_boost(world: &mut dyn MoveWorld, phys: &Physics, from: Vec3, to: Vec3, full: f32) -> String {
    match boost_tries(phys.gravity, from, to, full).first() {
        None => "no pitch from 30° to 70° down throws the bot that far and that high, even with a full charge".into(),
        Some(&(pitch, push)) => match try_boost(world, phys, from, to, (pitch, push), full) {
            Err(why) => format!("looking {pitch:.0}° down with a push of {push:.0}: {why}"),
            Ok(_) => "a boost holds".into(),
        },
    }
}

/// Why no long jump from `from` onto `to` holds: where the jump comes down, or which of the takeoffs a little off it
/// must allow fails.
pub fn why_no_longjump(world: &mut dyn MoveWorld, phys: &Physics, from: Vec3, to: Vec3) -> String {
    let v = simulate_longjump(world, phys, from, to, 0.0);
    if !v.ok {
        if v.flight == 0.0 {
            return "the run-up there does not end in a long jump".into();
        }
        return format!("the long jump {}", came_down(from, to, v.touchdown));
    }
    let dir = flat_dir(from, to).extend(0.0);
    let tries = [
        (dir * -12.0, 0.0, "12 u earlier"),
        (dir * 12.0, 0.0, "12 u further on"),
        (Vec3::ZERO, -8.0, "8 u to one side"),
        (Vec3::ZERO, 8.0, "8 u to the other side"),
    ];
    for (along, side, name) in tries {
        let r = simulate_longjump(world, phys, from + along, to + along, side);
        if r.ok {
            continue;
        }
        if r.flight == 0.0 {
            return format!("taking off {name} is off the floor: the takeoff is at an edge");
        }
        return format!(
            "taking off {name}, the long jump {}",
            came_down(from + along, to + along, r.touchdown)
        );
    }
    "a long jump holds".into()
}

/// The gauss boost from `from` onto `to` when a full charge pushes `full`, if one gets there reliably: the gentlest
/// of `boost_tries` that `check_boost` passes.
pub fn plan_boost(world: &mut dyn MoveWorld, phys: &Physics, from: Vec3, to: Vec3, full: f32) -> Option<TrickPlan> {
    boost_tries(phys.gravity, from, to, full)
        .into_iter()
        .find_map(|t| check_boost(world, phys, from, to, t, full))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::boxworld::BoxWorld;
    use crate::validate::JumpQuery;

    fn flat() -> BoxWorld {
        let mut w = BoxWorld::new();
        w.floor(0.0, 4096.0);
        w
    }

    fn running(world: &mut BoxWorld, phys: &Physics, at: Vec3, speed: f32) -> Player {
        let mut p = settled(world, phys, at);
        p.velocity = Vec3::new(speed, 0.0, 0.0);
        p
    }

    #[test]
    fn a_long_jump_flies_about_420_units_and_steers_short() {
        let phys = Physics::default();
        let mut w = flat();
        let start = Vec3::new(0.0, 0.0, 36.0);
        let p = running(&mut w, &phys, start, 270.0);
        let free = longjump_from(&mut w, &phys, p, 0.0, None);
        assert!(free.ok);
        let far = free.landing.x - start.x;
        assert!((400.0..460.0).contains(&far), "{far}");
        assert!(free.impact < 400.0, "{}", free.impact);
        // Steered for a spot 300 units ahead, it brakes in the air and comes down there.
        let to = Vec3::new(300.0, 0.0, 36.0);
        let steered = longjump_from(&mut w, &phys, p, 0.0, Some(to));
        assert!(steered.ok, "{:?}", steered.landing);
        // Standing still there is no long jump.
        let still = settled(&mut w, &phys, start);
        assert!(!longjump_from(&mut w, &phys, still, 0.0, None).ok);
        let reach = longjump_reach(0.0, phys.gravity).unwrap();
        assert!((reach - far).abs() < 16.0, "{reach} {far}");
    }

    #[test]
    fn a_long_jump_into_a_shallow_lava_pool_is_seen_to_hurt() {
        let phys = Physics::default();
        let mut w = flat();
        w.volume(
            Vec3::new(300.0, -256.0, 0.0),
            Vec3::new(700.0, 256.0, 12.0),
            contents::LAVA,
        );
        let p = running(&mut w, &phys, Vec3::new(0.0, 0.0, 36.0), 270.0);
        let v = longjump_from(&mut w, &phys, p, 0.0, None);
        assert!(v.ok && v.landing.x > 300.0, "{:?}", v.landing);
        assert!(lands_in_hazard(&mut w, v.landing), "{:?}", v.landing);
        let short = longjump_from(&mut w, &phys, p, 0.0, Some(Vec3::new(260.0, 0.0, 36.0)));
        assert!(
            short.ok && !lands_in_hazard(&mut w, short.landing),
            "{:?}",
            short.landing
        );
    }

    #[test]
    fn a_long_jump_carries_further_down_and_not_high_up() {
        let phys = Physics::default();
        let level = longjump_reach(0.0, phys.gravity).unwrap();
        // Off a ledge 200 units high onto the floor below.
        let mut w = BoxWorld::new();
        w.solid(Vec3::new(-1024.0, -512.0, -512.0), Vec3::new(64.0, 512.0, 200.0));
        w.floor(0.0, 4096.0);
        let p = running(&mut w, &phys, Vec3::new(0.0, 0.0, 236.0), 270.0);
        let free = longjump_from(&mut w, &phys, p, 0.0, None);
        let down = longjump_reach(-200.0, phys.gravity).unwrap();
        assert!(
            free.ok && (free.landing.x - down).abs() < 24.0,
            "{:?} {down}",
            free.landing
        );
        assert!(down > level + 150.0);
        assert!(longjump_reach(80.0, phys.gravity).is_none());
    }

    #[test]
    fn a_long_jump_crosses_a_gap_a_jump_cannot() {
        let phys = Physics::default();
        // Floors at x < 0 and x > 320, a pit between.
        let mut w = BoxWorld::new();
        w.solid(Vec3::new(-1024.0, -512.0, -512.0), Vec3::new(0.0, 512.0, 0.0));
        w.solid(Vec3::new(320.0, -512.0, -512.0), Vec3::new(1024.0, 512.0, 0.0));
        let from = Vec3::new(-24.0, 0.0, 36.0);
        let to = Vec3::new(400.0, 0.0, 36.0);
        let jump = crate::validate::simulate_run_jump(
            &mut w,
            &phys,
            &JumpQuery {
                from,
                to,
                speed: 270.0,
                duck: true,
                longjump: false,
            },
            64.0,
        );
        assert!(!jump.ok, "a running jump falls short: {:?}", jump.landing);
        let plan = plan_longjump(&mut w, &phys, from, to).expect("a long jump makes it");
        assert!(plan.robustness >= TRICK_ROBUST && plan.flight < 1.0, "{plan:?}");
    }

    #[test]
    fn a_gauss_boost_throws_far_and_up_onto_a_ledge() {
        let phys = Physics::default();
        let mut w = flat();
        let from = Vec3::new(0.0, 0.0, 36.0);
        let q = BoostQuery {
            from,
            dir: Vec2::X,
            pitch: 34.0,
            push: FULL_PUSH,
            to: None,
        };
        let free = simulate_boost(&mut w, &phys, &q);
        assert!(free.ok);
        assert!(free.landing.x > 1000.0, "{:?}", free.landing);
        assert!(
            phys.fall_damage(free.impact) > 0.0,
            "a boost comes down hard: {}",
            free.impact
        );
        // A ledge 200 units up and 500 along: the boost gets onto it.
        let mut w = flat();
        w.solid(Vec3::new(400.0, -256.0, 0.0), Vec3::new(900.0, 256.0, 200.0));
        let top = Vec3::new(600.0, 0.0, 236.0);
        let plan = plan_boost(&mut w, &phys, from, top, FULL_PUSH).expect("the boost gets onto the ledge");
        assert!(plan.robustness >= TRICK_ROBUST, "{plan:?}");
        // A jump does not.
        assert!(crate::validate::plan_jump(&mut w, &phys, from, top).is_none());
    }

    #[test]
    fn a_boost_is_charged_for_the_push_it_needs() {
        let g = 800.0;
        let near = boost_push(g, 34.0, 400.0, 0.0, FULL_PUSH).unwrap();
        let far = boost_push(g, 34.0, 900.0, 0.0, FULL_PUSH).unwrap();
        assert!(near < far && far < FULL_PUSH, "{near} {far}");
        assert!(boost_push(g, 34.0, 3000.0, 0.0, FULL_PUSH).is_none(), "too far");
        assert!(boost_push(g, 34.0, 100.0, 900.0, FULL_PUSH).is_none(), "too high");
        let least = boost_push(g, 34.0, 20.0, 0.0, FULL_PUSH).unwrap();
        assert!(
            (least - FULL_PUSH / 3.0).abs() < 0.1,
            "no less than half a second's charge: {least}"
        );
        assert!((charge_for(FULL_PUSH / 2.0, FULL_PUSH) - 0.75).abs() < 1e-5);
        assert_eq!(charge_for(FULL_PUSH / 10.0, FULL_PUSH), MIN_CHARGE);
        assert_eq!(charge_for(2.0 * FULL_PUSH, FULL_PUSH), FULL_CHARGE);
    }

    #[test]
    fn a_boost_under_a_low_ceiling_is_charged_partly_and_lands() {
        let phys = Physics::default();
        // A hall 360 units high, a ledge 120 up from 300 units along.
        let mut w = flat();
        w.solid(Vec3::new(-512.0, -512.0, 360.0), Vec3::new(1400.0, 512.0, 420.0));
        w.solid(Vec3::new(300.0, -256.0, 0.0), Vec3::new(1400.0, 256.0, 120.0));
        let from = Vec3::new(0.0, 0.0, 36.0);
        let top = Vec3::new(420.0, 0.0, 156.0);
        let plan = plan_boost(&mut w, &phys, from, top, FULL_PUSH).expect("the boost gets onto the ledge");
        assert!(
            plan.push < 0.8 * FULL_PUSH && plan.robustness >= TRICK_ROBUST,
            "{plan:?}"
        );
        let free = simulate_boost(
            &mut w,
            &phys,
            &BoostQuery {
                from,
                dir: Vec2::X,
                pitch: plan.pitch,
                push: plan.push,
                to: None,
            },
        );
        assert!(
            free.ok && (free.landing - top).truncate().length() < 200.0,
            "unsteered it comes down past the landing, not across the hall: {:?}",
            free.landing
        );
    }

    #[test]
    fn the_view_of_a_boost_looks_back_and_down() {
        let view = boost_view(Vec2::X, 34.0);
        let (forward, _, _) = view_angle_vectors(view);
        assert!(forward.x < -0.8 && forward.z < -0.5, "{forward}");
    }
}
