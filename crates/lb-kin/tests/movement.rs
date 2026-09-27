//! Movement against known numbers of the game: run speed, jump height, crouch-jump reach, step height, safe fall,
//! ladders, swimming, long jumps and ducking under obstacles.

use lb_core::Vec3;
use lb_core::input::{IN_DUCK, IN_FORWARD, IN_JUMP};
use lb_kin::boxworld::BoxWorld;
use lb_kin::{Cmd, MoveEvents, Physics, Player, player_move};
use lb_worldq::contents;

fn run_cmd(yaw: f32, buttons: u16, msec: u8) -> Cmd {
    Cmd {
        angles: Vec3::new(0.0, yaw, 0.0),
        forward: if buttons & IN_FORWARD != 0 { 400.0 } else { 0.0 },
        side: 0.0,
        up: 0.0,
        buttons,
        msec,
    }
}

/// Runs `seconds` of commands of `msec` each; `f(t, player)` gives the command.
fn simulate(
    w: &mut BoxWorld,
    p: &mut Player,
    seconds: f32,
    msec: u8,
    mut f: impl FnMut(f32, &Player) -> Cmd,
) -> Vec<(f32, Player, MoveEvents)> {
    let phys = Physics::default();
    let steps = (seconds * 1000.0 / f32::from(msec)).round() as usize;
    let mut out = Vec::with_capacity(steps);
    for i in 0..steps {
        let t = i as f32 * f32::from(msec) / 1000.0;
        let cmd = f(t, p);
        let ev = player_move(w, &phys, p, &cmd);
        out.push((t, *p, ev));
    }
    out
}

fn flat() -> BoxWorld {
    let mut w = BoxWorld::new();
    w.floor(0.0, 4096.0);
    w
}

fn on_floor(x: f32, y: f32, z: f32) -> Player {
    Player::standing(Vec3::new(x, y, z + 36.0))
}

#[test]
fn run_speed_is_maxspeed_at_any_command_rate() {
    for msec in [1u8, 4, 10, 20] {
        let mut w = flat();
        let mut p = on_floor(0.0, 0.0, 0.0);
        let trace = simulate(&mut w, &mut p, 2.0, msec, |_, _| run_cmd(0.0, IN_FORWARD, msec));
        let (_, last, _) = trace.last().unwrap();
        let speed = last.velocity.truncate().length();
        assert!((speed - 270.0).abs() < 0.5, "msec {msec}: {speed}");
        assert!(last.on_ground());
        assert!(
            (last.origin.z - 36.0).abs() < 0.1,
            "stays on the floor: {}",
            last.origin.z
        );
    }
}

#[test]
fn a_jump_rises_45_units_whatever_the_rate() {
    for msec in [1u8, 4, 10, 13] {
        let mut w = flat();
        let mut p = on_floor(0.0, 0.0, 0.0);
        simulate(&mut w, &mut p, 0.1, msec, |_, _| run_cmd(0.0, 0, msec));
        let base = p.feet();
        let mut jumped = false;
        let trace = simulate(&mut w, &mut p, 1.2, msec, |t, _| {
            run_cmd(0.0, if t < 0.001 { IN_JUMP } else { 0 }, msec)
        });
        jumped |= trace.iter().any(|(_, _, e)| e.jumped);
        let apex = trace.iter().map(|(_, pl, _)| pl.feet()).fold(f32::MIN, f32::max) - base;
        assert!(jumped);
        assert!((apex - 45.0).abs() < 0.6, "msec {msec}: apex {apex}");
        assert!(trace.last().unwrap().1.on_ground(), "lands again");
        assert!(trace.iter().any(|(_, _, e)| e.landed.is_some()));
    }
}

#[test]
fn a_held_jump_does_not_pogo() {
    let mut w = flat();
    let mut p = on_floor(0.0, 0.0, 0.0);
    let trace = simulate(&mut w, &mut p, 2.0, 10, |_, _| run_cmd(0.0, IN_JUMP, 10));
    let jumps = trace.iter().filter(|(_, _, e)| e.jumped).count();
    assert_eq!(jumps, 1, "jump needs a fresh press");
}

/// Runs at a ledge of `height`, jumps near it and ducks in the air; returns whether the player got on top.
fn crouch_jump_onto(height: f32) -> bool {
    let mut w = flat();
    w.solid(Vec3::new(100.0, -256.0, 0.0), Vec3::new(400.0, 256.0, height));
    let mut p = on_floor(-200.0, 0.0, 0.0);
    let mut pressed = false;
    simulate(&mut w, &mut p, 2.0, 10, |_, pl| {
        let near = pl.origin.x > 30.0;
        let mut b = IN_FORWARD;
        if near && !pressed {
            pressed = true;
            b |= IN_JUMP;
        }
        if pressed && !pl.on_ground() {
            b |= IN_DUCK;
        }
        run_cmd(0.0, b, 10)
    });
    p.on_ground() && p.feet() > height - 1.0 && p.origin.x > 110.0
}

#[test]
fn crouch_jump_climbs_63_units_but_not_more() {
    assert!(crouch_jump_onto(40.0));
    assert!(crouch_jump_onto(60.0));
    assert!(!crouch_jump_onto(66.0));
}

#[test]
fn steps_up_to_18_units_are_walked() {
    for (h, up) in [(16.0, true), (18.0, true), (20.0, false)] {
        let mut w = flat();
        w.solid(Vec3::new(64.0, -256.0, 0.0), Vec3::new(512.0, 256.0, h));
        let mut p = on_floor(0.0, 0.0, 0.0);
        simulate(&mut w, &mut p, 1.0, 10, |_, _| run_cmd(0.0, IN_FORWARD, 10));
        assert_eq!(p.origin.x > 100.0, up, "step {h}: {:?}", p.origin);
    }
}

#[test]
fn a_fall_lands_at_the_expected_speed() {
    let mut w = BoxWorld::new();
    w.solid(Vec3::new(-512.0, -512.0, 284.0), Vec3::new(0.0, 512.0, 300.0));
    w.floor(0.0, 4096.0);
    let mut p = on_floor(-40.0, 0.0, 300.0);
    let trace = simulate(&mut w, &mut p, 2.0, 10, |_, pl| {
        run_cmd(0.0, if pl.origin.x < 30.0 { IN_FORWARD } else { 0 }, 10)
    });
    let landed = trace.iter().find_map(|(_, _, e)| e.landed).unwrap();
    let expected = (2.0f32 * 800.0 * 300.0).sqrt();
    assert!((landed - expected).abs() < 25.0, "{landed} vs {expected}");
    assert_eq!(Physics::default().fall_damage(landed), 10.0);
    assert!(p.on_ground() && p.feet().abs() < 0.1);
}

#[test]
fn ladders_are_climbed_up_and_left_at_the_top() {
    let mut w = flat();
    // A wall with a ladder on its west face and a wide ledge on top.
    w.solid(Vec3::new(0.0, -128.0, 0.0), Vec3::new(1024.0, 128.0, 200.0));
    w.volume(
        Vec3::new(-4.0, -32.0, 0.0),
        Vec3::new(0.0, 32.0, 200.0),
        contents::LADDER,
    );
    let mut p = on_floor(-40.0, 0.0, 0.0);
    let mut climbed = false;
    let trace = simulate(&mut w, &mut p, 3.0, 10, |_, pl| {
        let cmd = run_cmd(0.0, IN_FORWARD, 10);
        climbed |= pl.on_ladder;
        cmd
    });
    assert!(climbed, "the ladder is boarded by walking into it");
    let top = trace.iter().map(|(_, pl, _)| pl.origin.z).fold(f32::MIN, f32::max);
    assert!(top > 200.0, "climbs to the top: {top}");
    assert!(p.on_ground() && p.feet() > 199.0 && p.origin.x > 10.0, "{:?}", p);
}

#[test]
fn swimming_rises_and_jump_swims_up() {
    let mut w = flat();
    w.volume(
        Vec3::new(-512.0, -512.0, 0.0),
        Vec3::new(512.0, 512.0, 256.0),
        contents::WATER,
    );
    let mut p = on_floor(0.0, 0.0, 0.0);
    simulate(&mut w, &mut p, 0.2, 10, |_, _| run_cmd(0.0, 0, 10));
    assert_eq!(p.waterlevel, 3);
    // Looking up and swimming forward rises.
    let start = p.origin.z;
    simulate(&mut w, &mut p, 0.5, 10, |_, _| Cmd {
        angles: Vec3::new(-60.0, 0.0, 0.0),
        ..run_cmd(0.0, IN_FORWARD, 10)
    });
    assert!(p.origin.z > start + 40.0, "{} -> {}", start, p.origin.z);
    // Idle in water sinks, once the momentum of swimming up has faded (water only slows the speed as a whole).
    let high = p.origin.z;
    simulate(&mut w, &mut p, 2.5, 10, |_, _| run_cmd(0.0, 0, 10));
    assert!(p.origin.z < high, "{high} -> {}", p.origin.z);
    // Holding jump swims up.
    let low = p.origin.z;
    simulate(&mut w, &mut p, 0.5, 10, |_, _| run_cmd(0.0, IN_JUMP, 10));
    assert!(p.origin.z > low + 20.0, "{low} -> {}", p.origin.z);
}

#[test]
fn long_jump_flies_about_450_units_crouched() {
    let mut w = flat();
    let mut p = on_floor(0.0, 0.0, 0.0);
    p.longjump = true;
    simulate(&mut w, &mut p, 1.0, 10, |_, _| run_cmd(0.0, IN_FORWARD, 10));
    let takeoff = p.origin.x;
    let mut pressed = false;
    let trace = simulate(&mut w, &mut p, 1.5, 10, |_, _| {
        let mut b = IN_FORWARD | IN_DUCK;
        if !pressed {
            pressed = true;
            b |= IN_JUMP;
        }
        run_cmd(0.0, b, 10)
    });
    assert!(trace.iter().any(|(_, _, e)| e.longjumped));
    let landing = trace
        .iter()
        .find(|(_, _, e)| e.landed.is_some())
        .map(|(_, pl, _)| pl.origin.x)
        .unwrap();
    // 560 u/s for 0.8 s: rising 56 units and landing crouched, 18 units lower than it took off.
    let dist = landing - takeoff;
    assert!((430.0..465.0).contains(&dist), "{dist}");
}

#[test]
fn ducking_passes_under_a_low_gap() {
    for (duck, through) in [(false, false), (true, true)] {
        let mut w = flat();
        w.solid(Vec3::new(64.0, -256.0, 40.0), Vec3::new(160.0, 256.0, 200.0));
        let mut p = on_floor(0.0, 0.0, 0.0);
        simulate(&mut w, &mut p, 3.0, 10, |_, _| {
            run_cmd(0.0, IN_FORWARD | if duck { IN_DUCK } else { 0 }, 10)
        });
        assert_eq!(p.origin.x > 170.0, through, "duck {duck}: {:?}", p.origin);
    }
}

#[test]
fn standing_up_under_a_low_ceiling_stays_crouched() {
    let mut w = flat();
    w.solid(Vec3::new(-256.0, -256.0, 50.0), Vec3::new(256.0, 256.0, 100.0));
    let mut p = on_floor(0.0, 0.0, 0.0);
    p.origin.z = 18.0;
    p.ducked = true;
    simulate(&mut w, &mut p, 0.5, 10, |_, _| run_cmd(0.0, 0, 10));
    assert!(p.ducked, "no room to stand");
}
