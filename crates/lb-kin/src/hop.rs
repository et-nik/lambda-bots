//! Bunny hopping. The game jumps before it brakes, so a jump pressed again on the first command back on the ground
//! loses nothing to friction. In the air a press adds speed along its direction until the flight's own speed that
//! way reaches 30 units/s: pressed nearly square to the flight, every command adds some and turns the flight a
//! little toward the press — a strafe in the air. With `sv_airaccelerate 10` and 10 ms commands the air adds about
//! 900 to the square of the speed each command, a run of 270 grows to some 365 over one hop and 440 over the next.
//!
//! A server that crops jumps (the SDK always, BugfixedHL with `mp_bunnyhop 0`) cuts the speed of a jump taken faster
//! than 1.7 × maxspeed to 0.65 of that: hops there are kept just under it.

use lb_core::input::IN_JUMP;
use lb_core::math::dir_to_view_angles;
use lb_core::{Vec2, dmath};

use crate::physics::Physics;
use crate::pmove::{Cmd, MoveWorld, Player, player_move};

/// The air adds along a press only until the flight's speed that way reaches this (`PM_AirAccelerate`).
pub const AIR_WISH: f32 = 30.0;
/// A jump taken faster than this many times maxspeed is cropped where the server crops.
pub const CROP_FACTOR: f32 = 1.7;
/// Above the speed kept to by more than this, the air brakes.
const BRAKE_SLACK: f32 = 10.0;

/// The air as one command meets it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Air {
    /// `sv_airaccelerate`.
    pub airaccelerate: f32,
    /// The fastest the player presses: its maxspeed.
    pub maxspeed: f32,
    /// Seconds of one command.
    pub dt: f32,
}

impl Air {
    /// Speed one command adds at most along a direction pressed at full speed.
    pub fn gain(&self) -> f32 {
        self.airaccelerate * self.maxspeed * self.dt
    }
}

/// `v` turned by `angle` radians, counter-clockwise.
fn rotate(v: Vec2, angle: f32) -> Vec2 {
    let (s, c) = dmath::sin_cos(angle);
    Vec2::new(v.x * c - v.y * s, v.x * s + v.y * c)
}

/// Where to press in the air, and how hard (a direction and a speed to press it at), to fly along `want` at
/// `target` units/s; `None`: press nothing.
///
/// Below `target`, nearly square to the flight on the side of `want`: the most the air adds, the flight turning that
/// way at the same time. Flying along `want` already, each press swings the flight a little past it, so the side
/// changes every command and the flight keeps to `want`. At `target`, a little back of square and just hard enough to
/// turn the flight onto `want` without speeding it up; along `want` already, nothing. Well above `target`, against
/// the excess, pressed just hard enough for the air to take it off (with a strong `sv_airaccelerate` a full press
/// would stop the flight dead).
pub fn air_strafe(v: Vec2, want: Vec2, target: f32, air: &Air) -> Option<(Vec2, f32)> {
    let want = want.normalize_or_zero();
    let gain = air.gain();
    if want == Vec2::ZERO || gain <= 0.0 {
        return None;
    }
    let speed = v.length();
    if speed < 1.0 {
        return Some((want, air.maxspeed));
    }
    let heading = v / speed;
    let turn = dmath::atan2(heading.perp_dot(want), heading.dot(want));
    let side = if turn >= 0.0 { 1.0 } else { -1.0 };
    if speed > target + BRAKE_SLACK {
        let error = want * target - v;
        let excess = error.length();
        let wish = (excess / (air.airaccelerate * air.dt)).min(air.maxspeed);
        return Some((error / excess, wish));
    }
    if speed < target {
        // Pressed where the flight's own speed along the press leaves the whole gain to add.
        let along = (AIR_WISH - gain).max(0.0);
        let push = dmath::acos((along / speed).clamp(-1.0, 1.0));
        return Some((rotate(heading, side * push), air.maxspeed));
    }
    if turn.abs() < 1e-3 {
        return None;
    }
    // Back of square so that what the press adds keeps the speed (the flight's speed along it is half the addition,
    // against it), adding about the turn left times the speed: that much turns the flight by it.
    let added = (turn.abs() * speed).min(gain).min(2.0 * AIR_WISH);
    let back = dmath::acos((-added / (2.0 * speed)).clamp(-1.0, 1.0));
    let wish = (added / (air.airaccelerate * air.dt)).min(air.maxspeed);
    Some((rotate(heading, side * back), wish))
}

/// A hop followed through the movement code.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HopFlight {
    /// The jump went off.
    pub jumped: bool,
    /// Down on a floor again (not in water, not on a ladder) within the time allowed.
    pub landed: bool,
    /// The player where the hop ended.
    pub player: Player,
    /// Seconds in the air.
    pub flight: f32,
    /// Downward speed it came down at.
    pub impact: f32,
    /// It ran into a wall, a ceiling or a slope too steep to stand on.
    pub bumped: bool,
    /// Horizontal speed after the command that jumped (what a crop left of it, and the first press).
    pub takeoff: f32,
}

/// A hop taken now by `p`, on the ground: the jump pressed afresh on the first command, then each command what
/// `steer` presses for the player as it is (a direction and a speed), until it comes down or `limit` seconds pass.
pub fn simulate_hop(
    world: &mut dyn MoveWorld,
    phys: &Physics,
    mut p: Player,
    msec: u8,
    limit: f32,
    steer: &mut dyn FnMut(&Player) -> Option<(Vec2, f32)>,
) -> HopFlight {
    p.oldbuttons &= !IN_JUMP;
    let tick = f32::from(msec.max(1)) / 1000.0;
    let mut out = HopFlight {
        jumped: false,
        landed: false,
        player: p,
        flight: 0.0,
        impact: 0.0,
        bumped: false,
        takeoff: 0.0,
    };
    let commands = (limit / tick).ceil() as usize;
    for i in 0..=commands {
        let mut cmd = Cmd {
            buttons: if i == 0 { IN_JUMP } else { 0 },
            msec: msec.max(1),
            ..Cmd::default()
        };
        if let Some((dir, speed)) = steer(&p) {
            cmd.angles = dir_to_view_angles(dir.extend(0.0));
            cmd.forward = speed;
        }
        let ev = player_move(world, phys, &mut p, &cmd);
        out.bumped |= ev.bumped;
        if i == 0 {
            if !ev.jumped {
                break;
            }
            out.jumped = true;
            out.takeoff = p.velocity.truncate().length();
            continue;
        }
        out.flight += tick;
        if let Some(v) = ev.landed {
            out.impact = out.impact.max(v);
        }
        if p.on_ground() || p.waterlevel >= 2 || p.on_ladder {
            out.landed = p.on_ground() && p.waterlevel == 0 && !p.on_ladder;
            break;
        }
    }
    out.player = p;
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::boxworld::BoxWorld;
    use lb_core::Vec3;

    const MSEC: u8 = 10;

    fn air(phys: &Physics) -> Air {
        Air {
            airaccelerate: phys.airaccelerate,
            maxspeed: phys.maxspeed,
            dt: f32::from(MSEC) / 1000.0,
        }
    }

    /// A player running along +x on a wide floor at full speed.
    fn running(w: &mut BoxWorld, phys: &Physics) -> Player {
        w.floor(0.0, 8192.0);
        let mut p = Player::standing(Vec3::new(-6000.0, 0.0, 36.0));
        let cmd = Cmd {
            forward: phys.maxspeed,
            msec: MSEC,
            ..Cmd::default()
        };
        for _ in 0..100 {
            player_move(w, phys, &mut p, &cmd);
        }
        p
    }

    /// Hops along the x axis one after another, each taken on the first command back on the ground and steered onto
    /// the axis 200 units ahead: the speed before each takeoff and after it.
    fn hops(phys: &Physics, target: f32, count: usize) -> (Vec<(f32, f32)>, Player) {
        let mut w = BoxWorld::new();
        let mut p = running(&mut w, phys);
        let a = air(phys);
        let mut out = Vec::new();
        for _ in 0..count {
            let before = p.velocity.truncate().length();
            let f = simulate_hop(&mut w, phys, p, MSEC, 1.2, &mut |pl: &Player| {
                let want = Vec2::new(200.0, -pl.origin.y);
                air_strafe(pl.velocity.truncate(), want, target, &a)
            });
            assert!(f.jumped && f.landed && !f.bumped, "{f:?}");
            out.push((before, f.takeoff));
            p = f.player;
        }
        (out, p)
    }

    #[test]
    fn hops_gain_speed_up_to_just_under_the_crop() {
        let phys = Physics::default();
        let target = 0.97 * CROP_FACTOR * phys.maxspeed;
        let (takeoffs, end) = hops(&phys, target, 6);
        let speeds: Vec<f32> = takeoffs.iter().map(|(before, _)| *before).collect();
        assert!((speeds[0] - 270.0).abs() < 1.0, "{speeds:?}");
        assert!((350.0..380.0).contains(&speeds[1]), "one hop: {speeds:?}");
        assert!((425.0..target + 1.0).contains(&speeds[2]), "two hops: {speeds:?}");
        for s in &speeds[3..] {
            assert!(
                (target - 5.0..=target + 2.0).contains(s),
                "kept just under the crop: {speeds:?}"
            );
        }
        for (before, after) in &takeoffs {
            assert!(
                after >= &(before * 0.98),
                "no crop, no friction at a takeoff: {takeoffs:?}"
            );
        }
        assert!(end.origin.y.abs() < 8.0, "keeps to its line: {}", end.origin.y);
    }

    #[test]
    fn hops_go_faster_where_the_server_does_not_crop() {
        let phys = Physics {
            bunnyhop_cap: false,
            ..Physics::default()
        };
        let target = 2.0 * phys.maxspeed;
        let (takeoffs, _) = hops(&phys, target, 7);
        let last = takeoffs.last().unwrap().0;
        assert!((target - 5.0..=target + 2.0).contains(&last), "{takeoffs:?}");
    }

    #[test]
    fn a_jump_past_the_crop_loses_a_third_of_its_speed() {
        let phys = Physics::default();
        let mut w = BoxWorld::new();
        let mut p = running(&mut w, &phys);
        p.velocity.x = 500.0;
        let f = simulate_hop(&mut w, &phys, p, MSEC, 1.2, &mut |_: &Player| None);
        let crop = CROP_FACTOR * phys.maxspeed;
        assert!(f.jumped);
        assert!((f.takeoff - 0.65 * crop).abs() < 1.0, "{}", f.takeoff);
    }

    #[test]
    fn the_air_brakes_to_the_speed_kept() {
        for airaccelerate in [10.0, 100.0] {
            let phys = Physics {
                airaccelerate,
                ..Physics::default()
            };
            let a = air(&phys);
            let mut v = Vec2::new(600.0, 0.0);
            for _ in 0..40 {
                if let Some((dir, wish)) = air_strafe(v, Vec2::X, 445.0, &a) {
                    let added = (a.airaccelerate * wish * a.dt).min(AIR_WISH - v.dot(dir)).max(0.0);
                    v += dir * added;
                }
            }
            assert!((v.length() - 445.0).abs() < BRAKE_SLACK + 1.0, "{airaccelerate}: {v}");
            assert!(v.x > 0.0, "{airaccelerate}: braked, not turned back: {v}");
        }
    }

    #[test]
    fn the_air_turns_the_flight_at_the_speed_kept() {
        let phys = Physics::default();
        let a = air(&phys);
        let mut v = Vec2::new(445.0, 0.0);
        let mut commands = 0;
        while v.normalize().dot(Vec2::Y) < 0.999 && commands < 100 {
            if let Some((dir, wish)) = air_strafe(v, Vec2::Y, 445.0, &a) {
                let added = (a.airaccelerate * wish * a.dt).min(AIR_WISH - v.dot(dir)).max(0.0);
                v += dir * added;
            }
            commands += 1;
        }
        assert!(
            commands < 35,
            "a right angle in about a quarter of a second: {commands}"
        );
        assert!((v.length() - 445.0).abs() < 1.0, "turning does not speed up: {v}");
    }

    #[test]
    fn a_strong_air_still_hops_to_the_speed_kept() {
        let phys = Physics {
            airaccelerate: 100.0,
            ..Physics::default()
        };
        let target = 0.97 * CROP_FACTOR * phys.maxspeed;
        let (takeoffs, end) = hops(&phys, target, 5);
        let last = takeoffs.last().unwrap().0;
        assert!((target - 5.0..=target + 2.0).contains(&last), "{takeoffs:?}");
        assert!(end.origin.y.abs() < 8.0, "{}", end.origin.y);
    }
}
