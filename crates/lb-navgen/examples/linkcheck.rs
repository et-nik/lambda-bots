use lb_bsp::BspWorld;
use lb_bsp::mech::Mechanisms;
use lb_core::Vec3;
use lb_nav::graph::{LinkKind, NodeFlags};
use lb_navgen::{GenOptions, generate};
use lb_worldq::{HullKind, TraceQuery, Tracer};

const STEP: f32 = 18.0;

/// Straight walk without sliding: max distance the walker ends up off the line, or None when it stalls.
fn straight(t: &mut dyn Tracer, from: Vec3, to: Vec3, hull: HullKind) -> bool {
    let total = (to - from).truncate().length();
    if total < 1.0 {
        return true;
    }
    let dir = ((to - from).truncate() / total).extend(0.0);
    let mut cur = from;
    let mut done = 0.0;
    while done < total - 0.5 {
        let d = (total - done).min(16.0);
        let direct = t.trace(&TraceQuery::hull(cur, cur + dir * d, hull));
        let mut next = None;
        if !direct.start_solid && direct.fraction >= 1.0 {
            next = Some(direct.end);
        } else {
            let up = t.trace(&TraceQuery::hull(cur, cur + Vec3::Z * STEP, hull));
            if !up.start_solid {
                let fwd = t.trace(&TraceQuery::hull(up.end, up.end + dir * d, hull));
                if !fwd.start_solid && fwd.fraction >= 1.0 {
                    next = Some(fwd.end);
                }
            }
        }
        let Some(n) = next else { return false };
        let down = t.trace(&TraceQuery::hull(n, n - Vec3::Z * (STEP + 64.0), hull));
        cur = if down.fraction < 1.0 { down.end } else { n };
        done += d;
    }
    true
}

fn main() {
    let maps = lb_bsp::test_maps_dir().expect("maps");
    let names: Vec<String> = std::env::args().skip(1).collect();
    for map in names {
        let mut world = BspWorld::load(&std::fs::read(maps.join(format!("{map}.bsp"))).unwrap()).unwrap();
        let mech = Mechanisms::from_world(&world);
        let g = generate(&mut world, &mech, &GenOptions::default(), "check");
        let graph = &g.graph;
        let n = graph.len();
        let mut walks = 0;
        let mut bent = 0;
        let mut lens = Vec::new();
        let mut deg = vec![0usize; 16];
        for a in 0..n as u32 {
            let links = graph.links(a);
            deg[links.len().min(15)] += 1;
            for l in links {
                if !matches!(l.kind, LinkKind::Walk | LinkKind::Crouch) {
                    continue;
                }
                walks += 1;
                let (na, nb) = (graph.node(a), graph.node(l.to));
                lens.push(l.length);
                let crouch = na.flags.contains(NodeFlags::CROUCH) || nb.flags.contains(NodeFlags::CROUCH);
                let ok = if crouch {
                    let c = |p: Vec3, f: NodeFlags| if f.contains(NodeFlags::CROUCH) { p } else { p - Vec3::Z * 18.0 };
                    straight(&mut world, c(na.origin, na.flags), c(nb.origin, nb.flags), HullKind::Crouch)
                } else {
                    straight(&mut world, na.origin, nb.origin, HullKind::Stand)
                };
                if !ok {
                    bent += 1;
                }
            }
        }
        let mut nn = Vec::new();
        for a in 0..n as u32 {
            let o = graph.node(a).origin;
            let d = (0..n as u32)
                .filter(|&b| b != a && (graph.node(b).origin.z - o.z).abs() < 32.0)
                .map(|b| (graph.node(b).origin - o).truncate().length())
                .fold(f32::INFINITY, f32::min);
            nn.push(d);
        }
        nn.sort_by(f32::total_cmp);
        lens.sort_by(f32::total_cmp);
        let q = |v: &[f32], p: f32| v[((v.len() - 1) as f32 * p) as usize];
        println!(
            "{map}: nodes {n}, links {}, walks {walks}, not straight {bent} ({:.1}%), walk len p10/50/90 {:.0}/{:.0}/{:.0}, \
             nearest node p10/50/90 {:.0}/{:.0}/{:.0}",
            graph.stats.links,
            bent as f32 * 100.0 / walks as f32,
            q(&lens, 0.1),
            q(&lens, 0.5),
            q(&lens, 0.9),
            q(&nn, 0.1),
            q(&nn, 0.5),
            q(&nn, 0.9),
        );
        println!("  out degree: {:?}", &deg);
    }
}
