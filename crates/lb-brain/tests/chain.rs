//! The whole bot on the synthetic scene of `support`: it sees, fights and keeps its commands consistent.

mod support;

use lb_brain::Body;
use lb_combat::Armed;
use lb_core::Vec3;
use lb_core::time::SimTime;
use lb_game::input::{IN_ATTACK, IN_ATTACK2};
use lb_game::weapons::WeaponId;
use lb_knowledge::PlayerKey;
use lb_motor::{LookIntent, Prio};
use support::*;

#[test]
fn an_enemy_in_sight_is_recognized_engaged_and_shot_at() {
    let world = World {
        players: vec![player(2, Vec3::new(500.0, -150.0, 0.0), Vec3::new(0.0, 100.0, 0.0))],
        ..World::default()
    };
    let r = run(world, 400, 3);
    let shot = r.first_shot.expect("the bot fired");
    // Normal skill: recognition in 0.22–0.35 s, while the glock in hand at the start deploys for 0.5 s.
    assert!((0.5..0.8).contains(&shot), "first shot at {shot}");
    let reactions = &r.brain.mind.reactions.fresh;
    assert_eq!(reactions.count, 1);
    assert!(reactions.recognition_to_shot < 0.4, "{reactions:?}");
    // The view ends up on the enemy.
    let last = r.outs.last().unwrap().angles;
    let enemy_yaw = lb_core::dmath::atan2(-150.0f32 + 100.0 * 4.0, 500.0).to_degrees();
    assert!(
        lb_core::math::angle_diff(last.y, enemy_yaw).abs() < 10.0,
        "{last} vs {enemy_yaw}"
    );
}

#[test]
fn a_silent_hidden_enemy_changes_no_command() {
    let visible = player(2, Vec3::new(400.0, -200.0, 0.0), Vec3::new(0.0, 120.0, 0.0));
    let base = World {
        walls: vec![wall(700.0, 500.0)],
        players: vec![visible],
        ..World::default()
    };
    let mut with_hidden = base.clone();
    with_hidden
        .players
        .push(player(3, Vec3::new(1000.0, 0.0, 0.0), Vec3::ZERO));
    let (a, b) = (run(base, 300, 11), run(with_hidden, 300, 11));
    assert!(a.first_shot.is_some(), "the visible enemy is fought");
    for (i, (x, y)) in a.outs.iter().zip(&b.outs).enumerate() {
        assert_eq!(
            (x.angles, x.forward, x.side, x.buttons, &x.commands),
            (y.angles, y.forward, y.side, y.buttons, &y.commands),
            "frame {i}"
        );
    }
}

#[test]
fn commands_never_conflict() {
    let world = World {
        players: vec![
            player(2, Vec3::new(300.0, -100.0, 0.0), Vec3::new(0.0, 150.0, 0.0)),
            player(3, Vec3::new(-200.0, 300.0, 0.0), Vec3::new(80.0, 0.0, 0.0)),
        ],
        ..World::default()
    };
    let r = run(world, 600, 5);
    let mut previous = 0u16;
    for out in &r.outs {
        assert!(out.buttons & IN_ATTACK == 0 || out.buttons & IN_ATTACK2 == 0);
        let jump = lb_game::input::IN_JUMP;
        assert!(
            out.buttons & jump == 0 || previous & jump == 0,
            "jumps are fresh presses"
        );
        assert!(out.angles.x.abs() <= 89.0);
        previous = out.buttons;
    }
}

/// A GunGame bot on the level `kit` gave, the enemy (slot 2) leading.
fn gungame_level(kit: &[Armed]) -> impl Fn(&mut Body) + '_ {
    move |b: &mut Body| {
        let weapons = kit.iter().fold(0, |m, a| m | a.id.bit());
        let board = lb_game::gungame::Board::new([(ME, 300, 0), (2, 700, 0)], 100, true);
        let g = lb_game::gungame::GunGame::new(&board, ME, weapons);
        b.arsenal = kit.iter().copied().collect();
        b.allowed = g.kit.weapons();
        b.gungame = Some(g);
    }
}

#[test]
fn a_gungame_bot_throws_its_grenades_and_reaches_for_nothing_else() {
    let world = World {
        players: vec![player(2, Vec3::new(550.0, -100.0, 0.0), Vec3::new(0.0, 60.0, 0.0))],
        ..World::default()
    };
    let kit = [Armed::new(WeaponId::HandGrenade, None, Some(10))];
    let r = run_dressed(world, 600, 4, WeaponId::HandGrenade, &gungame_level(&kit));
    for out in &r.outs {
        assert!(
            out.commands.iter().all(|c| c == "weapon_handgrenade"),
            "only the level's weapon is asked for: {:?}",
            out.commands
        );
    }
    // The scene has no game to take the grenade from the hand: the pin pulled is as far as a throw gets here.
    assert!(
        r.outs.iter().any(|o| o.buttons & IN_ATTACK != 0),
        "the grenades are its weapon"
    );
}

#[test]
fn a_gungame_launcher_too_close_backs_off_then_fires_pressed() {
    let world = World {
        players: vec![player(2, Vec3::new(160.0, 0.0, 0.0), Vec3::ZERO)],
        ..World::default()
    };
    let kit = [Armed::new(WeaponId::Rpg, Some(1), Some(4))];
    let r = run_dressed(world, 300, 6, WeaponId::Rpg, &gungame_level(&kit));
    assert!(
        r.outs.iter().all(|o| o.commands.is_empty()),
        "no crowbar where the level gave none"
    );
    // Recognized in some 0.3 s, the enemy stays within the rocket's least distance: the bot backs off for its patience
    // (under a second, by its aggression), then fires as close as the blast leaves it some health.
    let shot = r.first_shot.expect("pressed, it fires at last");
    assert!((0.5..1.5).contains(&shot), "first rocket at {shot}");
    let before = &r.outs[30..(shot / 0.01) as usize];
    let backing = before.iter().filter(|o| o.forward < -100.0).count();
    assert!(
        backing * 2 > before.len(),
        "out of its blast first: {backing} of {} frames",
        before.len()
    );
    assert!(
        r.outs[30..].iter().all(|o| o.forward < 100.0),
        "never toward the enemy once it is recognized"
    );
}

#[test]
fn a_recognized_enemy_is_aimed_at_on_the_same_frame() {
    let world = World {
        players: vec![player(2, Vec3::new(600.0, -100.0, 0.0), Vec3::new(0.0, 150.0, 0.0))],
        ..World::default()
    };
    let setup = Setup {
        frame: 0.002,
        ..Setup::default()
    };
    // The scene sees the intents of the frame before.
    let (mut aimed, mut last) = (None, SimTime::ZERO);
    let r = run_scene(world, 1000, &setup, &|_| {}, &mut |sc| {
        let aiming = matches!(sc.brain.intents.look, Some((Prio::Threat, LookIntent::Track { .. })));
        if aiming && aimed.is_none() {
            aimed = Some(last);
        }
        last = sc.now;
    });
    let track = r.brain.beliefs.track_by_slot(2).expect("recognized");
    assert_eq!(aimed, Some(track.recognized_at));
}

#[test]
fn the_enemy_shooting_the_bot_is_turned_to_at_once() {
    // One in front faces the bot; another off to the side, in view, starts shooting it at 2 s.
    let mut shooter = player(3, Vec3::new(383.0, 321.0, 0.0), Vec3::ZERO);
    shooter.angles.y = -140.0;
    let world = World {
        players: vec![player(2, Vec3::new(400.0, 0.0, 0.0), Vec3::ZERO), shooter],
        ..World::default()
    };
    let setup = Setup {
        frame: 0.002,
        ..Setup::default()
    };
    let mut rng = lb_core::rng::Pcg32::new(5, 5);
    let mut next_shot = 2.0;
    let mut targets = Vec::new();
    let r = run_scene(world, 2500, &setup, &|_| {}, &mut |sc| {
        targets.push((sc.now.secs(), sc.brain.mind.target.map(|k| k.slot)));
        if sc.now.secs() >= next_shot {
            next_shot += 0.3;
            sc.shots[3] = Some(sc.now);
            let from = sc.world.players[2].origin + Vec3::Z * 28.0;
            let view = sc.brain.motor.view;
            if let Some(d) = lb_perception::damage::stimulus(sc.now, 8, 0, 2, from, Vec3::ZERO, view, &mut rng) {
                sc.brain.on_damage(&d);
            }
        }
    });
    let front = targets.iter().find(|(t, _)| *t >= 1.9).and_then(|(_, k)| *k);
    assert_eq!(front, Some(2), "the one in front first");
    let seen = r
        .brain
        .beliefs
        .track_by_slot(3)
        .expect("the shooter is recognized")
        .recognized_at;
    let turned = targets
        .iter()
        .find(|(t, k)| *t >= 2.0 && *k == Some(3))
        .map(|(t, _)| *t)
        .expect("turned to the shooter");
    assert!(
        turned <= seen.secs().max(2.0) + 0.2,
        "shot at from 2 s, recognized at {}, turned to at {turned}",
        seen.secs()
    );
    assert_eq!(r.brain.mind.target, Some(PlayerKey { slot: 3, userid: 103 }));
}

#[test]
fn something_moving_off_to_the_side_is_looked_at_before_it_is_recognized() {
    let world = World {
        players: vec![player(2, Vec3::new(566.0, 566.0, 0.0), Vec3::ZERO)],
        ..World::default()
    };
    let setup = Setup {
        frame: 0.002,
        ..Setup::default()
    };
    let r = run_scene(world, 3000, &setup, &|_| {}, &mut |_| {});
    let recognized = r
        .brain
        .beliefs
        .track_by_slot(2)
        .expect("recognized")
        .recognized_at
        .secs();
    let turned = r
        .outs
        .iter()
        .position(|o| o.angles.y > 30.0)
        .map(|i| i as f64 * f64::from(setup.frame))
        .expect("looked that way");
    assert!(turned < recognized, "turned at {turned}, recognized at {recognized}");
}

/// The aim at each rocket fired at slot 2 strafing (or running) across the bot's view 800 units off, and where the
/// target was then.
fn rockets_at_a_crossing(turn_every: Option<f64>) -> Vec<(f64, Vec3, Vec3)> {
    let world = World {
        players: vec![player(2, Vec3::new(800.0, -200.0, 0.0), Vec3::new(0.0, 250.0, 0.0))],
        ..World::default()
    };
    let kit = [Armed::new(WeaponId::Rpg, Some(1), Some(4))];
    let dress = move |b: &mut Body| b.arsenal = kit.iter().copied().collect();
    let setup = Setup {
        skill: 75,
        weapon: WeaponId::Rpg,
        ..Setup::default()
    };
    // The scene sees each frame's aim before the next.
    let mut frames = Vec::new();
    let r = run_scene(world, 3000, &setup, &dress, &mut |sc| {
        if let Some(every) = turn_every {
            let leg = (sc.now.secs() / every) as i64;
            sc.world.players[1].velocity.y = if leg % 2 == 0 { 250.0 } else { -250.0 };
        }
        frames.push((sc.now.secs(), sc.world.players[1].origin, sc.brain.mind.last_aim));
    });
    r.outs
        .iter()
        .zip(frames.iter().skip(1))
        .filter(|(o, _)| o.buttons & IN_ATTACK != 0)
        .filter_map(|(_, (t, at, aim))| Some((*t, *at, (*aim)?)))
        .collect()
}

#[test]
fn a_rocket_leads_a_run_but_not_a_strafe() {
    let run = rockets_at_a_crossing(None);
    let (_, target, aim) = run.first().copied().expect("fired at the runner");
    assert!(aim.y - target.y > 120.0, "led well ahead of a runner: {target} {aim}");
    // Strafing 0.4 s each way about y = -150, once a strafe there and back is seen: the rockets go nearer the middle
    // than 200 units, where a lead by what the target does at the shot would put them at the least. The hand trails
    // the aim point as it swings, and the trigger comes as the view catches up with it, out to either side more often
    // than in the middle.
    let strafe = rockets_at_a_crossing(Some(0.4));
    let off: Vec<f32> = strafe
        .iter()
        .filter(|(t, ..)| *t > 1.5)
        .map(|(_, _, aim)| (aim.y + 150.0).abs())
        .collect();
    assert!(off.len() >= 10, "fired at the strafer: {strafe:?}");
    let mean = off.iter().sum::<f32>() / off.len() as f32;
    assert!(mean < 120.0 && off.iter().all(|d| *d < 200.0), "{off:?}");
}
