//! Prints a simulated jump step by step.
//! Usage: cargo run -p lb-nav --example jump_trace -- <map.bsp> <x y z> <x y z> <speed> <duck 0|1>

use lb_bsp::mech::Mechanisms;
use lb_core::Vec3;
use lb_core::input::{IN_DUCK, IN_FORWARD, IN_JUMP};
use lb_core::math::dir_to_view_angles;
use lb_kin::{Cmd, Physics, Player, player_move};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let mut world = lb_bsp::BspWorld::load(&std::fs::read(&a[1]).unwrap()).unwrap();
    Mechanisms::from_world(&world).place_at_rest(&mut world);
    let v = |i: usize| {
        Vec3::new(
            a[i].parse().unwrap(),
            a[i + 1].parse().unwrap(),
            a[i + 2].parse().unwrap(),
        )
    };
    let (from, to) = (v(2), v(5));
    let speed: f32 = a[8].parse().unwrap();
    let duck = a[9] == "1";
    let phys = Physics::default();
    let mut p = Player::standing(from);
    player_move(
        &mut world,
        &phys,
        &mut p,
        &Cmd {
            msec: 10,
            ..Cmd::default()
        },
    );
    p.velocity = ((to - from).truncate().normalize() * speed).extend(0.0);
    for i in 0..120 {
        let d = (to - p.origin).truncate();
        let mut angles = dir_to_view_angles(d.extend(0.0));
        angles.x = 0.0;
        let mut b = if d.length() > 8.0 { IN_FORWARD } else { 0 };
        if i == 0 {
            b |= IN_JUMP;
        }
        if duck && !p.on_ground() {
            b |= IN_DUCK;
        }
        let cmd = Cmd {
            angles,
            forward: if b & IN_FORWARD != 0 { 400.0 } else { 0.0 },
            buttons: b,
            msec: 10,
            ..Cmd::default()
        };
        let ev = player_move(&mut world, &phys, &mut p, &cmd);
        println!(
            "{i:>3} o {:>8.2} {:>8.2} {:>8.2} v {:>7.1} {:>7.1} {:>7.1} ground {:?} ducked {} ev {:?}",
            p.origin.x, p.origin.y, p.origin.z, p.velocity.x, p.velocity.y, p.velocity.z, p.ground, p.ducked, ev
        );
    }
}
