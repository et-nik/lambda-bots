//! Long jumps and gauss boosts the way a bot makes them: for the links that use them, and for the checks a bot makes
//! before it takes one on the way.
//!
//! - **Long jump.** With the module, duck and jump pressed together on the ground while moving faster than 50
//!   units/s set the horizontal speed to 560 along the view (less looking up or down) and throw the player 56 units
//!   up: about 420 units over flat ground in three quarters of a second. The bot holds duck in the air and steers
//!   onto its landing; the air takes speed away fast, so a long jump comes down anywhere short of its full reach.
//! - **Gauss boost.** A charged gauss shot pushes its shooter back at five times its damage, up and down too in
//!   multiplayer. Looking back and 30–38° down, a full charge let go as the bot jumps adds some 850 units/s forward
//!   and 550 up: a flight of over a thousand units, or onto a ledge a few hundred units up.

use lb_core::input::{IN_DUCK, IN_FORWARD, IN_JUMP};
use lb_core::math::{dir_to_view_angles, view_angle_vectors};
use lb_core::{Vec2, Vec3};
use lb_worldq::{HullKind, Trace, TraceQuery, Tracer, contents};

use crate::physics::Physics;
use crate::pmove::{Cmd, Ladder, MoveWorld, Player, player_move};
use crate::validate::{
    ARRIVE_DZ, ARRIVE_RADIUS, MoveVerdict, STEP_MS, air_steer, arrived, cmd_toward, flat_dir, settled,
};

/// A long jump is taken moving at least this fast (the game wants more than 50 units/s).
pub const LONGJUMP_TAKEOFF: f32 = 100.0;
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

fn verdict(p: &Player, ok: bool, flight: f32, impact: f32) -> MoveVerdict {
    MoveVerdict {
        ok,
        landing: p.origin,
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
    verdict(&p, ok, flight, impact)
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

/// How a trick link is done: the view's pitch it needs (the gauss boost's look down), the share of perturbed
/// attempts that still land, how long it flies and how hard it comes down.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrickPlan {
    pub pitch: f32,
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

/// Pitches a boost is tried with, the likeliest first.
pub const BOOST_PITCHES: [f32; 3] = [34.0, 30.0, 38.0];

/// The gauss boost from `from` onto `to`, if one gets there reliably (the view off by 3° across or 2° down, the
/// takeoff 8 units along the way, still lands), and comes down somewhere safe unsteered too (the steering may stop
/// when a fight takes the bot's mind off it).
pub fn plan_boost(world: &mut dyn MoveWorld, phys: &Physics, from: Vec3, to: Vec3, push: f32) -> Option<TrickPlan> {
    let dir = flat_dir(from, to);
    for pitch in BOOST_PITCHES {
        let q = BoostQuery {
            from,
            dir,
            pitch,
            push,
            to: Some(to),
        };
        let v = simulate_boost(world, phys, &q);
        if !v.ok {
            continue;
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
        ];
        let landed = variants.iter().filter(|v| simulate_boost(world, phys, v).ok).count();
        let robustness = landed as f32 / variants.len() as f32;
        if robustness < TRICK_ROBUST {
            continue;
        }
        let free = simulate_boost(world, phys, &BoostQuery { to: None, ..q });
        if !free.ok || phys.fall_damage(free.impact) > phys.fall_damage(v.impact).max(10.0) {
            continue;
        }
        let mut p = settled(world, phys, free.landing);
        p.origin = free.landing;
        if hazard(world, &p) {
            continue;
        }
        return Some(TrickPlan {
            pitch,
            robustness,
            flight: v.flight,
            impact: v.impact,
        });
    }
    None
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
    fn the_view_of_a_boost_looks_back_and_down() {
        let view = boost_view(Vec2::X, 34.0);
        let (forward, _, _) = view_angle_vectors(view);
        assert!(forward.x < -0.8 && forward.z < -0.5, "{forward}");
    }
}
