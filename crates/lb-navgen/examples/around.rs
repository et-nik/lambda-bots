use lb_bsp::BspWorld;
use lb_bsp::mech::Mechanisms;
use lb_core::Vec3;
use lb_navgen::{GenOptions, generate};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let maps = lb_bsp::test_maps_dir().expect("maps");
    let mut world = BspWorld::load(&std::fs::read(maps.join(format!("{}.bsp", a[1]))).unwrap()).unwrap();
    let mech = Mechanisms::from_world(&world);
    let g = generate(&mut world, &mech, &GenOptions::default(), "check");
    let p = Vec3::new(a[2].parse().unwrap(), a[3].parse().unwrap(), a[4].parse().unwrap());
    let r: f32 = a.get(5).map_or(200.0, |s| s.parse().unwrap());
    let gr = &g.graph;
    for n in 0..gr.len() as u32 {
        let o = gr.node(n).origin;
        if (o - p).length() > r {
            continue;
        }
        println!("node {n} {:?} {:?} r {}", o, gr.node(n).flags, gr.node(n).radius);
        for l in gr.links(n) {
            println!("   -> {} {:?} {:?} valid {}", l.to, gr.node(l.to).origin, l.kind, l.valid());
        }
        for m in 0..gr.len() as u32 {
            for l in gr.links(m) {
                if l.to == n && (gr.node(m).origin - p).length() > r {
                    println!("   <- {m} {:?} {:?} valid {}", gr.node(m).origin, l.kind, l.valid());
                }
            }
        }
    }
}
