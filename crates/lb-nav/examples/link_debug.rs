//! Explains why links fail: walk checks with movers solid and passable, what blocks a straight move, jump plans.
//! Usage: cargo run -p lb-nav --example link_debug -- <map.bsp> <map.graph> <from> <to> [<from> <to> ...]

use lb_bsp::mech::Mechanisms;
use lb_nav::import::{ImportOptions, import_yapb};
use lb_nav::validate::walk_check;
use lb_nav::yapb;
use lb_worldq::{HullKind, TraceQuery, Tracer};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut world = lb_bsp::BspWorld::load(&std::fs::read(&args[1]).unwrap()).unwrap();
    let mech = Mechanisms::from_world(&world);
    let y = yapb::parse(&std::fs::read(&args[2]).unwrap()).unwrap();
    let g = import_yapb(&y, &mut world, &mech, &ImportOptions::default(), "x");
    for pair in args[3..].chunks(2) {
        let (a, b): (u32, u32) = (pair[0].parse().unwrap(), pair[1].parse().unwrap());
        let (na, nb) = (g.node(a), g.node(b));
        println!(
            "{a} -> {b}: {:?} {:?} -> {:?} {:?}",
            na.origin, na.flags, nb.origin, nb.flags
        );
        println!(
            "  yapb origins {:?} -> {:?}",
            y.nodes[a as usize].origin, y.nodes[b as usize].origin
        );
        world.set_movers_solid(true);
        println!(
            "  walk, movers solid: {:?}",
            walk_check(&mut world, na.origin, nb.origin, HullKind::Stand)
        );
        let tr = world.trace(&TraceQuery::hull(
            na.origin + lb_core::Vec3::Z * 2.0,
            nb.origin + lb_core::Vec3::Z * 2.0,
            HullKind::Stand,
        ));
        println!(
            "  straight: frac {:.3} hit {:?} end {:?} normal {:?}",
            tr.fraction, tr.hit, tr.end, tr.normal
        );
        world.set_movers_solid(false);
        println!(
            "  walk, movers passable: {:?}",
            walk_check(&mut world, na.origin, nb.origin, HullKind::Stand)
        );
        world.set_movers_solid(true);
        for duck in [false, true] {
            let v =
                lb_kin::validate::simulate_walk(&mut world, &lb_kin::Physics::default(), na.origin, nb.origin, duck);
            println!(
                "  simulated walk duck {duck}: ok {} stopped {:?} flight {:.2} impact {:.0}",
                v.ok, v.landing, v.flight, v.impact
            );
        }
        let plan = lb_kin::validate::plan_jump(&mut world, &lb_kin::Physics::default(), na.origin, nb.origin);
        println!("  jump plan: {plan:?}");
        for speed in [0.0f32, 150.0, 270.0] {
            for duck in [false, true] {
                let v = lb_kin::validate::simulate_jump(
                    &mut world,
                    &lb_kin::Physics::default(),
                    &lb_kin::validate::JumpQuery {
                        from: na.origin,
                        to: nb.origin,
                        speed,
                        duck,
                        longjump: false,
                    },
                );
                println!(
                    "    jump speed {speed} duck {duck}: ok {} landing {:?} flight {:.2}",
                    v.ok, v.landing, v.flight
                );
            }
        }
    }
    for b in world
        .brushes
        .iter()
        .filter(|b| [46usize, 47, 44, 66].contains(&b.model))
    {
        println!(
            "*{} {} offset {:?} mins {:?} maxs {:?} solid {}",
            b.model,
            b.classname,
            b.offset,
            b.abs_mins(),
            b.abs_maxs(),
            b.solid
        );
    }
}
