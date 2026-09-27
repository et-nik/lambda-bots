//! Draws a generated graph from above into a PPM image: floor covered by the graph green, cut-off floor red,
//! hazards dark red, water blue, nodes white, items yellow, spawns cyan. Lower floors are drawn first.
//! Usage: cargo run --release -p lb-navgen --example render -- <map.bsp> <out.ppm> [min_z max_z]
//! (`sips -s format png out.ppm --out out.png` converts it on macOS.)

use std::io::Write;

use lb_bsp::BspWorld;
use lb_bsp::mech::Mechanisms;
use lb_navgen::field::{CELL, NONE, SpanFlags};
use lb_navgen::{GenOptions, generate, report};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: render <map.bsp> <out.ppm> [min_z max_z]");
        std::process::exit(2);
    }
    let mut world = BspWorld::load(&std::fs::read(&args[1]).expect("reads the BSP")).expect("loads the BSP");
    let mech = Mechanisms::from_world(&world);
    let g = generate(&mut world, &mech, &GenOptions::default(), "render");
    let (zmin, zmax) = match (args.get(3), args.get(4)) {
        (Some(a), Some(b)) => (a.parse().expect("min_z"), b.parse().expect("max_z")),
        _ => (f32::MIN, f32::MAX),
    };
    let c = report::coverage(&g);
    eprintln!("coverage {:.1}%", c.ratio() * 100.0);
    let spans: Vec<_> = g
        .field
        .spans
        .iter()
        .enumerate()
        .filter(|(_, s)| s.feet >= zmin && s.feet <= zmax)
        .collect();
    let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
    for (_, s) in &spans {
        x0 = x0.min(s.cell.x);
        y0 = y0.min(s.cell.y);
        x1 = x1.max(s.cell.x);
        y1 = y1.max(s.cell.y);
    }
    let scale = 2;
    let (w, h) = (((x1 - x0 + 1) * scale) as usize, ((y1 - y0 + 1) * scale) as usize);
    let mut img = vec![[20u8, 20, 20]; w * h];
    let put = |x: i32, y: i32, c: [u8; 3], img: &mut Vec<[u8; 3]>| {
        for dx in 0..scale {
            for dy in 0..scale {
                let px = ((x - x0) * scale + dx) as usize;
                let py = ((y1 - y) * scale + dy) as usize;
                if px < w && py < h {
                    img[py * w + px] = c;
                }
            }
        }
    };
    let mut order = spans.clone();
    order.sort_by(|a, b| a.1.feet.total_cmp(&b.1.feet));
    for (i, s) in &order {
        let o = g.owner[*i];
        let shade = (((s.feet - zmin.max(-4096.0)) / 16.0) as i32 % 40) as u8;
        let color = if s.flags.contains(SpanFlags::HAZARD) {
            [120, 0, 0]
        } else if o == NONE || !c.roundtrip_nodes[o as usize] {
            [255, 40, 40]
        } else if s.flags.contains(SpanFlags::WATER) {
            [40, 80, 200 + shade]
        } else {
            [30, 110 + shade * 3, 40]
        };
        put(s.cell.x, s.cell.y, color, &mut img);
    }
    for n in 0..g.graph.len() as u32 {
        let o = g.graph.node(n).origin;
        if o.z - 36.0 < zmin || o.z - 36.0 > zmax {
            continue;
        }
        let color = if g.spawns.contains(&n) {
            [0, 255, 255]
        } else if g.items.contains(&n) {
            [255, 230, 0]
        } else {
            [230, 230, 230]
        };
        put(
            (o.x / CELL).floor() as i32,
            (o.y / CELL).floor() as i32,
            color,
            &mut img,
        );
    }
    let mut f = std::fs::File::create(&args[2]).expect("creates the image");
    write!(f, "P6\n{w} {h}\n255\n").expect("writes the image");
    for p in img {
        f.write_all(&p).expect("writes the image");
    }
    eprintln!("{w}x{h}, cells x {x0}..{x1} y {y0}..{y1} (x = cell * 16)");
}
