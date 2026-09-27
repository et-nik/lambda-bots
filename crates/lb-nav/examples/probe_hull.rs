//! Where a standing player fits around a point: a map of '.' (fits), '#' (does not) and 'L' (in a ladder).
//! Usage: cargo run -p lb-nav --example probe_hull -- <map.bsp> <x> <y> <z> [step] [radius]

use lb_bsp::mech::Mechanisms;
use lb_core::Vec3;
use lb_kin::MoveWorld;
use lb_worldq::{HullKind, TraceQuery, Tracer};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let mut world = lb_bsp::BspWorld::load(&std::fs::read(&a[1]).unwrap()).unwrap();
    Mechanisms::from_world(&world).place_at_rest(&mut world);
    let c = Vec3::new(a[2].parse().unwrap(), a[3].parse().unwrap(), a[4].parse().unwrap());
    let step: f32 = a.get(5).and_then(|s| s.parse().ok()).unwrap_or(8.0);
    let r: i32 = a.get(6).and_then(|s| s.parse().ok()).unwrap_or(12);
    println!(
        "x from {} to {} (step {step}), y rows top = +",
        c.x - r as f32 * step,
        c.x + r as f32 * step
    );
    for dy in (-r..=r).rev() {
        let y = c.y + dy as f32 * step;
        let mut row = format!("y {y:>7.0} ");
        for dx in -r..=r {
            let p = Vec3::new(c.x + dx as f32 * step, y, c.z);
            let tr = world.trace(&TraceQuery::hull(p, p, HullKind::Stand));
            let ch = if tr.start_solid {
                '#'
            } else if world.ladder(p, HullKind::Stand).is_some() {
                'L'
            } else {
                '.'
            };
            row.push(ch);
        }
        println!("{row}");
    }
}
