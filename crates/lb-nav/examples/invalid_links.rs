//! Lists links of an imported yapb graph that fail the offline check, and the special links it found.
//! Usage: cargo run -p lb-nav --example invalid_links -- <map.bsp> <map.graph> [limit]

use lb_bsp::mech::Mechanisms;
use lb_nav::import::{ImportOptions, import_yapb};
use lb_nav::yapb;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut world = lb_bsp::BspWorld::load(&std::fs::read(&args[1]).unwrap()).unwrap();
    let mech = Mechanisms::from_world(&world);
    let y = yapb::parse(&std::fs::read(&args[2]).unwrap()).unwrap();
    let limit: usize = args.get(3).and_then(|a| a.parse().ok()).unwrap_or(20);
    let g = import_yapb(&y, &mut world, &mech, &ImportOptions::default(), "x");
    println!("{} links: {}", g.stats.links, g.stats.kinds());
    let (mut up, mut flat, mut down, mut shown) = (0, 0, 0, 0);
    for (i, n) in g.nodes.iter().enumerate() {
        for l in g.links(i as u32) {
            let b = g.node(l.to);
            if let Some(spec) = g.spec(l)
                && !matches!(
                    l.kind,
                    lb_nav::LinkKind::Jump | lb_nav::LinkKind::Drop | lb_nav::LinkKind::Ladder
                )
            {
                println!("{i:>4} -> {:>4} {:<9} {:?}", l.to, l.kind.as_str(), spec.action);
            }
            if l.valid() {
                continue;
            }
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
                println!(
                    "{i:>4} -> {:>4} {:<7} dist {:>6.1} dz {:>6.1}  from {:?} {:?} to {:?} {:?}",
                    l.to,
                    l.kind.as_str(),
                    n.origin.truncate().distance(b.origin.truncate()),
                    dz,
                    n.origin,
                    n.flags,
                    b.origin,
                    b.flags
                );
            }
        }
    }
    println!("invalid: {up} going up (> 18), {flat} flat, {down} going down");
}
