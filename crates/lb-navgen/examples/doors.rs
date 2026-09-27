use lb_bsp::BspWorld;
use lb_bsp::mech::Mechanisms;
use lb_nav::graph::LinkKind;
use lb_navgen::field::SpanFlags;
use lb_navgen::{GenOptions, generate};

fn main() {
    let maps = lb_bsp::test_maps_dir().expect("maps");
    let map = std::env::args().nth(1).unwrap();
    let mut world = BspWorld::load(&std::fs::read(maps.join(format!("{map}.bsp"))).unwrap()).unwrap();
    let mech = Mechanisms::from_world(&world);
    for m in &mech.movers {
        let b = world.brush(m.model).unwrap();
        eprintln!("mover {:?} name {:?} usable {} touch {} model {} rest {:?} active {:?} box {:?}..{:?}", m.kind, m.targetname, m.usable, m.touch, m.model, m.rest, m.active, b.abs_mins(), b.abs_maxs());
    }
    for t in &mech.triggers {
        let m = &world.bsp.models[t.model];
        eprintln!("trigger {:?} model {} target {:?} box {:?}..{:?}", t.kind, t.model, world.entities[t.entity].get("target"), m.mins, m.maxs);
    }
    if let Some(d) = mech.movers.iter().find(|m| m.targetname.as_deref() == Some(match map.as_str() { "undertow" => "bunker_door", "stalkyard" => std::env::var("DOOR").unwrap_or_default().leak(), _ => "secret_door" })) {
        eprintln!("secret door activators {:?}", mech.activators(&world, match map.as_str() { "undertow" => "bunker_door", "stalkyard" => std::env::var("DOOR").unwrap_or_default().leak(), _ => "secret_door" }));
        let _ = d;
    }
    for seed in lb_navgen::site::sites(&world, &mech).seeds {
        if matches!(seed.kind, lb_navgen::site::SeedKind::UseSpot | lb_navgen::site::SeedKind::TouchSpot) {
            eprintln!("seed {:?} at {:?}", seed.kind, seed.origin);
        }
    }
    if map == "undertow" {
        use lb_worldq::{HullKind, TraceQuery, Tracer};
        let b = world.brush(52).unwrap();
        let (mins, maxs) = (b.abs_mins(), b.abs_maxs());
        let center = (mins + maxs) * 0.5;
        let half = (maxs - mins) * 0.5;
        for k in 0..8 {
            let angle = k as f32 * std::f32::consts::FRAC_PI_4;
            let dir = lb_core::Vec2::new(angle.cos(), angle.sin());
            let edge = [0usize, 1].into_iter().filter(|&a| dir[a].abs() > 1e-3).map(|a| half[a] / dir[a].abs()).fold(f32::INFINITY, f32::min);
            let at = (center.truncate() + dir * (edge + 24.0)).extend(center.z + 24.0);
            let tr = world.trace(&TraceQuery::hull(at, at - lb_core::Vec3::Z * 128.0, HullKind::Stand));
            let origin = tr.end;
            let reach = (origin.clamp(mins, maxs) - origin).length();
            let eye = origin + lb_core::Vec3::Z * 28.0;
            let sight = world.trace(&TraceQuery::line(eye, center));
            eprintln!("dir {k}: at {at:?} start_solid {} frac {} nz {} origin {origin:?} reach {reach} sight {} hit {:?}", tr.start_solid, tr.fraction, tr.normal.z, sight.fraction, sight.hit);
        }
    }
    let g = generate(&mut world, &mech, &GenOptions::default(), "check");
    let gates: Vec<_> = g.field.spans.iter().filter(|s| s.flags.contains(SpanFlags::GATE)).collect();
    eprintln!("gate spans {}", gates.len());
    for s in gates.iter().take(400).step_by(7) {
        eprintln!("  gate at {:?} feet {}", s.at, s.feet);
    }
    let nodes: Vec<_> = (0..g.graph.len() as u32).map(|i| *g.graph.node(i)).collect();
    let find = |x: f32, y: f32| (0..nodes.len()).find(|&i| (nodes[i].origin.x - x).abs() < 1.0 && (nodes[i].origin.y - y).abs() < 1.0);
    let args: Vec<f32> = std::env::args().skip(2).map(|x| x.parse().unwrap()).collect();
    let model = args.get(4).map_or(63, |m| *m as usize);
    if let (Some(a), Some(b)) = (find(args.first().copied().unwrap_or(584.0), args.get(1).copied().unwrap_or(1432.0)), find(args.get(2).copied().unwrap_or(712.0), args.get(3).copied().unwrap_or(1480.0))) {
        let mut cls = lb_nav::classify::Classifier { world: &mut world, mech: &mech, phys: lb_kin::Physics::default(), nodes: nodes.clone(), specs: Vec::new() };
        let (na, nb) = (nodes[a], nodes[b]);
        eprintln!("walk closed {:?}", cls.walk(&na, &nb).map(|x| x.0));
        let door = mech.mover(model).unwrap().clone();
        eprintln!("blocker {:?}", cls.blocker(&na, &nb));
        eprintln!("door usable {} touch {} name {:?}", door.usable, door.touch, door.targetname);
        eprintln!("opener {:?}", cls.opener(&door, na.origin));
        eprintln!("door_link a->b {:?}", cls.door_link(&na, &nb, model).map(|c| (c.kind, c.valid)));
        eprintln!("door_link b->a {:?}", cls.door_link(&nb, &na, model).map(|c| (c.kind, c.valid)));
    }
    for a in 0..g.graph.len() as u32 {
        for l in g.graph.links(a) {
            if l.kind == LinkKind::Door {
                eprintln!("door link {a} {:?} -> {} {:?} valid {}", g.graph.node(a).origin, l.to, g.graph.node(l.to).origin, l.valid());
            }
        }
    }
}

#[allow(dead_code)]
fn unused() {}
