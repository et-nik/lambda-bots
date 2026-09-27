//! Lists links of an imported yapb graph that fail the offline check, with the reason.
//! Usage: cargo run -p lb-nav --example invalid_links -- <map.bsp> <map.graph> [limit]

use lb_core::Vec3;
use lb_nav::validate::walk_check;
use lb_nav::{NodeFlags, import::import_yapb, yapb};
use lb_worldq::HullKind;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut world = lb_bsp::BspWorld::load(&std::fs::read(&args[1]).unwrap()).unwrap();
    let y = yapb::parse(&std::fs::read(&args[2]).unwrap()).unwrap();
    let limit: usize = args.get(3).and_then(|a| a.parse().ok()).unwrap_or(20);
    let g = import_yapb(&y, &mut world, false, "x");
    let mut shown = 0;
    let (mut up, mut flat, mut down) = (0, 0, 0);
    for (i, n) in g.nodes.iter().enumerate() {
        for l in g.links(i as u32) {
            if l.valid {
                continue;
            }
            let b = g.node(l.to);
            let dz = b.origin.z - n.origin.z;
            if dz > 18.0 {
                up += 1
            } else if dz < -18.0 {
                down += 1
            } else {
                flat += 1
            }
            if shown < limit {
                shown += 1;
                let stand = walk_check(&mut world, n.origin, b.origin, HullKind::Stand);
                let c = |p: &lb_nav::NavNode| {
                    if p.flags.contains(NodeFlags::CROUCH) {
                        p.origin
                    } else {
                        p.origin - Vec3::Z * 18.0
                    }
                };
                let crouch = walk_check(&mut world, c(n), c(b), HullKind::Crouch);
                println!(
                    "{i:>4} -> {:>4}  dist {:>6.1} dz {:>6.1}  stand {:?} crouch {:?}  from {:?} to {:?}",
                    l.to,
                    n.origin.truncate().distance(b.origin.truncate()),
                    dz,
                    stand,
                    crouch,
                    n.origin,
                    b.origin
                );
            }
        }
    }
    println!("invalid: {} going up (> 18), {} flat, {} going down", up, flat, down);
}
