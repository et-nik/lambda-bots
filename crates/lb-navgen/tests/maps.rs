//! Every standard HLDM map: the graph is generated in time, covers at least 95% of the floor both ways (from the
//! spawn points and back), and gets to at least as many items as it did when this was last checked. The items it
//! does not get to are listed in `docs/m3-acceptance.md` with the reason. Skipped without the maps.

use lb_bsp::BspWorld;
use lb_bsp::mech::Mechanisms;
use lb_navgen::{GenOptions, generate, report};

/// Map, and items the graph gets to and back from.
#[rustfmt::skip]
const MAPS: &[(&str, usize)] = &[
    ("boot_camp", 150),
    ("bounce", 58),
    ("crossfire", 109),
    ("datacore", 57),
    ("frenzy", 40),
    ("gasworks", 73),
    ("lambda_bunker", 44),
    ("rapidcore", 58),
    ("snark_pit", 45),
    ("stalkyard", 70),
    ("subtransit", 64),
    ("undertow", 41),
];

/// Target time of a full graph (release build).
const MAX_MILLIS: u128 = 15_000;

#[test]
fn standard_maps_are_covered() {
    let Some(dir) = lb_bsp::test_maps_dir() else { return };
    let mut failed = Vec::new();
    for &(map, items) in MAPS {
        let Ok(bsp) = std::fs::read(dir.join(format!("{map}.bsp"))) else {
            eprintln!("{map}: no BSP, skipped");
            continue;
        };
        let mut world = BspWorld::load(&bsp).unwrap();
        let mech = Mechanisms::from_world(&world);
        let g = generate(&mut world, &mech, &GenOptions::default(), map);
        let c = report::coverage(&g);
        let s = &g.graph.stats;
        eprintln!(
            "{map:<14} {:>6} ms {:>6} spans {:>5} nodes {:>6} links ({}) | coverage {:>5.1}%, roundtrip {}/{}, items {}/{}, spawns {}",
            s.millis,
            g.field.len(),
            s.nodes,
            s.links,
            s.kinds(),
            c.ratio() * 100.0,
            c.roundtrip,
            c.nodes,
            c.items_ok,
            c.items,
            c.spawns,
        );
        for (o, n) in c.cut_off.iter().take(4) {
            eprintln!("    cut off: {n} spans at {o:?}");
        }
        let slow = !cfg!(debug_assertions) && s.millis > MAX_MILLIS;
        if c.ratio() < 0.95 || c.items_ok < items || slow {
            failed.push(map);
        }
    }
    assert!(failed.is_empty(), "{failed:?}");
}

/// A generated graph comes back from the cache the same, and fast.
#[test]
fn crossfire_graph_is_cached() {
    let Some(dir) = lb_bsp::test_maps_dir() else { return };
    let Ok(bsp) = std::fs::read(dir.join("crossfire.bsp")) else {
        return;
    };
    let mut world = BspWorld::load(&bsp).unwrap();
    let mech = Mechanisms::from_world(&world);
    let opts = GenOptions::default();
    let g = generate(&mut world, &mech, &opts, "crossfire").graph;
    let root = std::env::temp_dir().join(format!("lb-navgen-maps-{}", std::process::id()));
    let cache = lb_navgen::cache::GraphCache::new(&root, "crossfire");
    let key = lb_navgen::cache::key(&world, &opts, 0, 0);
    let path = cache.store(&key, &g).unwrap();
    let started = std::time::Instant::now();
    let back = cache.load(&key).unwrap();
    let millis = started.elapsed().as_millis();
    eprintln!(
        "crossfire: {} bytes on disk, loaded in {millis} ms",
        std::fs::metadata(&path).unwrap().len()
    );
    assert_eq!(back, g);
    if !cfg!(debug_assertions) {
        assert!(millis <= 50, "{millis} ms");
    }
    assert!(cache.load(&lb_navgen::cache::key(&world, &opts, 1, 0)).is_none());
    let _ = std::fs::remove_dir_all(root);
}

/// Landmarks cut the nodes a path search expands (printed for the acceptance notes).
#[test]
fn landmarks_cut_search_work() {
    use lb_nav::plan::{Search, SearchStep};
    let Some(dir) = lb_bsp::test_maps_dir() else { return };
    for map in ["crossfire", "boot_camp"] {
        let Ok(bsp) = std::fs::read(dir.join(format!("{map}.bsp"))) else {
            continue;
        };
        let mut world = BspWorld::load(&bsp).unwrap();
        let mech = Mechanisms::from_world(&world);
        let g = generate(&mut world, &mech, &GenOptions::default(), map)
            .graph
            .with_landmarks();
        let alt = g.alt.clone().unwrap();
        let mut rng = lb_core::rng::Pcg32::new(11, 13);
        let (mut plain, mut with_alt, mut paths) = (0u64, 0u64, 0u32);
        for _ in 0..300 {
            let a = rng.range_i32(0, g.len() as i32 - 1) as u32;
            let b = rng.range_i32(0, g.len() as i32 - 1) as u32;
            let mut s1 = Search::new(&g, None, a, b);
            let r1 = s1.run(&g, None, &|_, _| 0.0, &mut u32::MAX.clone());
            let mut s2 = Search::new(&g, Some(&alt), a, b);
            let r2 = s2.run(&g, Some(&alt), &|_, _| 0.0, &mut u32::MAX.clone());
            if let (SearchStep::Found(p1), SearchStep::Found(p2)) = (&r1, &r2) {
                let (t1, t2) = (lb_nav::plan::path_time(&g, p1), lb_nav::plan::path_time(&g, p2));
                assert!((t1 - t2).abs() < 1e-3, "{a} -> {b}: {t1} vs {t2}");
                plain += u64::from(s1.expanded);
                with_alt += u64::from(s2.expanded);
                paths += 1;
            }
        }
        eprintln!(
            "{map}: {} landmarks; {paths} paths, nodes expanded per search: {} straight-line, {} with landmarks",
            alt.landmarks.len(),
            plain / u64::from(paths.max(1)),
            with_alt / u64::from(paths.max(1))
        );
        assert!(with_alt <= plain);
    }
}

/// Every wall charger of the standard maps has a spot to use it from.
#[test]
fn chargers_can_be_used() {
    let Some(dir) = lb_bsp::test_maps_dir() else { return };
    let mut missing = Vec::new();
    for &(map, _) in MAPS {
        let Ok(bsp) = std::fs::read(dir.join(format!("{map}.bsp"))) else {
            continue;
        };
        let world = BspWorld::load(&bsp).unwrap();
        let all = world
            .entities
            .iter()
            .filter(|e| matches!(e.classname(), "func_healthcharger" | "func_recharge"))
            .count();
        let found = lb_navgen::site::chargers(&world).len();
        eprintln!("{map:<14} chargers {found}/{all}");
        if found < all {
            missing.push(format!("{map}: {found}/{all}"));
        }
    }
    assert!(missing.is_empty(), "chargers without a spot: {missing:?}");
}
