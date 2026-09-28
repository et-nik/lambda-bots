//! What the map knowledge costs a bot on a real map: a lost enemy's spread worked out again, a cover query, a look
//! at the places in sight. Usage: `cargo run --release -p lb-runtime --example map_costs -- <map.bsp>`.

use std::time::Instant;

use lb_core::Vec3;
use lb_core::time::SimTime;
use lb_knowledge::{Spread, Watch};
use lb_mapknow::{MapKnowledge, MapTactics};
use lb_nav_api::MapView;

fn main() {
    let path = std::env::args().nth(1).expect("usage: map_costs <map.bsp>");
    let bytes = std::fs::read(&path).expect("the map reads");
    let mut world = lb_bsp::BspWorld::load(&bytes).expect("the map loads");
    let mech = lb_bsp::mech::Mechanisms::from_world(&world);
    let generated = lb_navgen::generate(&mut world, &mech, &lb_navgen::GenOptions::default(), "costs");
    lb_navgen::site::rest_poses(&mut world, &mech);
    let vis = lb_bsp::MapVis::build(&world.bsp);
    let graph = generated.graph;
    let tactics = MapTactics::build(&graph, &world, &vis, &[], &[]);
    let map = MapKnowledge {
        graph: &graph,
        tactics: &tactics,
        experience: None,
    };
    let n = map.node_count() as u32;
    let places: Vec<Vec3> = (0..n).step_by(37).map(|i| map.node_origin(i)).collect();

    let mut watch = Watch::default();
    let started = Instant::now();
    let mut looks = 0;
    for (k, p) in places.iter().enumerate() {
        watch.update(
            SimTime(k as f64),
            *p,
            *p + Vec3::Z * 28.0,
            Vec3::new(0.0, (k * 40 % 360) as f32, 0.0),
            50.0,
            &map,
        );
        looks += 1;
    }
    let per_look = started.elapsed().as_secs_f64() * 1e6 / looks as f64;

    let started = Instant::now();
    let mut spreads = 0;
    for (k, p) in places.iter().enumerate() {
        let mut s = Spread::new(
            &map,
            *p,
            Some(Vec3::X * 250.0),
            SimTime(0.0),
            10.0,
            SimTime(0.0),
            Some(&watch),
        )
        .expect("a place");
        for t in 1..=16 {
            s.update(SimTime(f64::from(t) * 0.5 + k as f64 * 1e-3), &map, Some(&watch));
            spreads += 1;
        }
    }
    let per_spread = started.elapsed().as_secs_f64() * 1e6 / spreads as f64;

    let started = Instant::now();
    let mut covers = 0;
    for w in places.windows(2) {
        let _ = lb_mapknow::cover::cover(&graph, &tactics, w[0], w[1], 4.0, &|_| 0.0);
        covers += 1;
    }
    let per_cover = started.elapsed().as_secs_f64() * 1e6 / covers as f64;
    println!(
        "{n} places: a look at the places in sight {per_look:.1} us, a spread worked out again {per_spread:.1} us, \
         a cover query {per_cover:.1} us"
    );
}
