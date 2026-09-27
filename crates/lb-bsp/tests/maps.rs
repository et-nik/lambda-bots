//! Checks on the real maps of the local stand (`LB_MAPS_DIR`); skipped when no maps are available.

use lb_bsp::{BspWorld, test_maps_dir};
use lb_core::Vec3;
use lb_worldq::{HullKind, TraceQuery, Tracer, contents};

fn dm_maps() -> Vec<std::path::PathBuf> {
    let Some(dir) = test_maps_dir() else { return Vec::new() };
    let mut maps: Vec<_> = std::fs::read_dir(dir)
        .map(|rd| rd.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    maps.retain(|p: &std::path::PathBuf| {
        let name = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        p.extension().is_some_and(|e| e == "bsp") && !name.starts_with('c') && !name.starts_with('t')
            || name == "crossfire"
    });
    maps.sort();
    maps
}

fn load(path: &std::path::Path) -> BspWorld {
    BspWorld::load(&std::fs::read(path).unwrap()).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[test]
fn every_deathmatch_map_loads_and_spawns_stand_on_a_floor() {
    let maps = dm_maps();
    if maps.is_empty() {
        eprintln!("no maps: set LB_MAPS_DIR to run this test");
        return;
    }
    for path in maps {
        let mut world = load(&path);
        let spawns: Vec<Vec3> = world
            .entities
            .iter()
            .filter(|e| e.classname() == "info_player_deathmatch")
            .map(|e| e.origin())
            .collect();
        if spawns.is_empty() {
            continue;
        }
        let mut on_floor = 0;
        for &s in &spawns {
            let start = s + Vec3::Z;
            let tr = world.trace(&TraceQuery::hull(start, start - Vec3::Z * 128.0, HullKind::Stand));
            if !tr.start_solid && tr.fraction < 1.0 && tr.normal.z > 0.7 {
                on_floor += 1;
            }
            assert!(
                world.bsp.pvs_visible(s, s),
                "{}: spawn does not see itself",
                path.display()
            );
        }
        assert!(
            on_floor * 10 >= spawns.len() * 9,
            "{}: only {on_floor} of {} spawns have a standing floor under them",
            path.display(),
            spawns.len()
        );
    }
}

#[test]
fn crossfire_traces() {
    let Some(dir) = test_maps_dir() else { return };
    let path = dir.join("crossfire.bsp");
    if !path.exists() {
        return;
    }
    let mut world = load(&path);
    assert_eq!(
        world.bsp.fingerprint.1, 1_241_704,
        "crossfire.bsp size from the yapb graph trailer"
    );
    let spawns: Vec<Vec3> = world
        .entities
        .iter()
        .filter(|e| e.classname() == "info_player_deathmatch")
        .map(|e| e.origin())
        .collect();
    assert!(spawns.len() >= 16, "{}", spawns.len());
    let s = spawns[0] + Vec3::Z;
    // Straight up hits the ceiling, a point trace through the floor stops at it.
    let up = world.trace(&TraceQuery::hull(s, s + Vec3::Z * 4096.0, HullKind::Stand));
    assert!(up.fraction < 1.0 && up.normal.z < -0.7, "{up:?}");
    let down = world.trace(&TraceQuery::line(s, s - Vec3::Z * 4096.0));
    assert!(down.fraction < 1.0 && down.normal.z > 0.7);
    assert_eq!(world.point_contents(s), contents::EMPTY);
    assert_eq!(world.point_contents(down.end - Vec3::Z * 8.0), contents::SOLID);
    // A crouching hull fits wherever a standing one does.
    let stand = world.trace(&TraceQuery::hull(s, s - Vec3::Z * 4096.0, HullKind::Stand));
    let crouch = world.trace(&TraceQuery::hull(s, s - Vec3::Z * 4096.0, HullKind::Crouch));
    assert!(
        !crouch.start_solid && crouch.fraction >= stand.fraction,
        "{crouch:?} {stand:?}"
    );
}

/// The PVS gate must never hide a player a clear line reaches, and the PAS always covers the PVS.
#[test]
fn visibility_sets_never_hide_what_a_line_reaches() {
    use lb_bsp::MapVis;
    use lb_worldq::VisSets;
    let maps = dm_maps();
    if maps.is_empty() {
        eprintln!("no maps: set LB_MAPS_DIR to run this test");
        return;
    }
    for path in maps {
        let mut world = load(&path);
        let started = std::time::Instant::now();
        let vis = MapVis::build(&world.bsp);
        let millis = started.elapsed().as_millis();
        let spawns: Vec<Vec3> = world
            .entities
            .iter()
            .filter(|e| e.classname() == "info_player_deathmatch")
            .map(|e| e.origin())
            .collect();
        let (mut clear, mut pairs) = (0, 0);
        for (i, &a) in spawns.iter().enumerate() {
            let eye = a + Vec3::Z * 28.0;
            for &b in &spawns[i + 1..] {
                pairs += 1;
                let (mins, maxs) = (b - Vec3::new(17.0, 17.0, 37.0), b + Vec3::new(17.0, 17.0, 37.0));
                let (la, lb) = (vis.leaf_at(a), vis.leaf_at(b));
                assert!(
                    !vis.pvs(la, lb) || vis.pas(la, lb),
                    "{}: PAS misses a PVS leaf",
                    path.display()
                );
                if world.trace(&TraceQuery::line(eye, b + Vec3::Z * 28.0)).fraction >= 1.0 {
                    clear += 1;
                    assert!(
                        vis.box_in_pvs(eye, mins, maxs),
                        "{}: {a} sees {b} but the PVS hides it",
                        path.display()
                    );
                    assert!(
                        vis.in_pas(b, a),
                        "{}: {b} is visible from {a} but not audible",
                        path.display()
                    );
                }
            }
            assert!(vis.box_in_pvs(eye, a - Vec3::splat(16.0), a + Vec3::splat(16.0)));
        }
        eprintln!(
            "{}: {} vis leaves, {} KiB, {millis} ms, {clear} of {pairs} spawn pairs in plain sight",
            path.file_stem().unwrap().to_string_lossy(),
            vis.visleafs(),
            vis.memory() / 1024
        );
    }
}
