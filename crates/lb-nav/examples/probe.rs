//! Probes the map around a point: floor and ceiling heights and hits on a grid.
//! Usage: cargo run -p lb-nav --example probe -- <map.bsp> <x> <y> <z> [step]

use lb_bsp::mech::Mechanisms;
use lb_core::Vec3;
use lb_worldq::{HullKind, TraceQuery, Tracer};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let mut world = lb_bsp::BspWorld::load(&std::fs::read(&a[1]).unwrap()).unwrap();
    Mechanisms::from_world(&world).place_at_rest(&mut world);
    let c = Vec3::new(a[2].parse().unwrap(), a[3].parse().unwrap(), a[4].parse().unwrap());
    let step: f32 = a.get(5).and_then(|s| s.parse().ok()).unwrap_or(16.0);
    for dy in -6..=6 {
        let y = c.y + dy as f32 * step;
        let mut row = format!("y {y:>7.0}:");
        for dx in -3..=3 {
            let p = Vec3::new(c.x + dx as f32 * step, y, c.z);
            let down = world.trace(&TraceQuery::line(p, p - Vec3::Z * 256.0));
            let up = world.trace(&TraceQuery::line(p, p + Vec3::Z * 256.0));
            let floor = if down.start_solid { f32::NAN } else { down.end.z };
            let ceil = if up.start_solid { f32::NAN } else { up.end.z };
            row += &format!(
                " {:>6.0}/{:<6.0}{}",
                floor,
                ceil,
                down.hit.map(|h| h.to_string()).unwrap_or_default()
            );
        }
        println!("{row}");
    }
    let tr = world.trace(&TraceQuery::hull(c, c - Vec3::Z * 128.0, HullKind::Stand));
    println!("stand hull down from centre: {tr:?}");
}
