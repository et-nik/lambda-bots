//! The obstacle course on crossfire: every special link of the imported graph (lifts, jumps, drops, ladders) and
//! a sample of walks, each carried out from its entry to its exit by a simulated bot. Skipped without the map.

use lb_bsp::BspWorld;
use lb_bsp::mech::Mechanisms;
use lb_nav::follow::PathFollower;
use lb_nav::import::{ImportOptions, import_yapb};
use lb_nav::{LinkKind, NavGraph, NodeId};
use lb_testkit::course::{Course, CourseBot, Game, Outcome};

fn crossfire() -> Option<Course<BspWorld>> {
    let maps = lb_bsp::test_maps_dir()?;
    let bsp = std::fs::read(maps.join("crossfire.bsp")).ok()?;
    let graph = std::fs::read(maps.join("../addons/yapb/data/graph/crossfire.graph")).ok()?;
    let mut world = BspWorld::load(&bsp).unwrap();
    let mech = Mechanisms::from_world(&world);
    let yapb = lb_nav::yapb::parse(&graph).unwrap();
    let g = import_yapb(&yapb, &mut world, &mech, &ImportOptions::default(), "crossfire.graph");
    let game = Game::from_map(&mut world, &mech);
    Some(Course::new(world, game, g))
}

/// Valid links of a kind that start where a bot can stand (not at nodes yapb put inside walls).
fn links_of(g: &NavGraph, kind: LinkKind) -> Vec<(NodeId, NodeId)> {
    (0..g.len() as NodeId)
        .filter(|&a| !g.node(a).flags.contains(lb_nav::NodeFlags::AIRBORNE))
        .flat_map(|a| {
            g.links(a)
                .iter()
                .filter(|l| l.kind == kind && l.valid())
                .map(move |l| (a, l.to))
        })
        .collect()
}

/// The link was carried out: the bot arrived and no link failed on the way (the navigator plans around a failed
/// link, so arriving alone does not prove the link works).
fn clean(o: &Outcome) -> bool {
    o.arrived && o.failures.is_empty()
}

/// Puts a bot at `a` with the path `a → b` and runs it.
fn run_link(c: &mut Course<BspWorld>, a: NodeId, b: NodeId, fps: f64, long_frame: Option<f64>) -> Outcome {
    c.settle(10.0);
    let start = c.graph.node(a).origin;
    let dest = c.graph.node(b).origin;
    let mut bot = CourseBot::new(start, 100.0);
    c.place(&mut bot);
    bot.nav.follower = Some(PathFollower::new(vec![a, b], c.now));
    bot.nav.goal = Some(b);
    c.run(&mut bot, dest, 25.0, fps, long_frame)
}

/// A node a bot walks to `a` from (not `b`, not a ladder or mid-air node), farthest first, so it comes in running.
fn walk_in(g: &NavGraph, a: NodeId, b: NodeId) -> Option<NodeId> {
    use lb_nav::NodeFlags;
    let unfit = NodeFlags::AIRBORNE | NodeFlags::LADDER | NodeFlags::WATER | NodeFlags::ON_MOVER;
    (0..g.len() as NodeId)
        .filter(|&p| p != b && p != a && !g.node(p).flags.intersects(unfit))
        .filter(|&p| g.find_link(p, a).is_some_and(|l| l.valid() && l.kind == LinkKind::Walk))
        .max_by(|&x, &y| {
            let (dx, dy) = (
                g.node(x).origin.distance(g.node(a).origin),
                g.node(y).origin.distance(g.node(a).origin),
            );
            dx.total_cmp(&dy)
        })
}

/// Puts a bot at a node that walks into `a` and runs the path `p → a → b`: the link is entered at speed, from
/// whatever direction the graph brings a bot in.
fn run_link_moving(c: &mut Course<BspWorld>, a: NodeId, b: NodeId, fps: f64) -> Option<Outcome> {
    let p = walk_in(&c.graph, a, b)?;
    c.settle(10.0);
    let start = c.graph.node(p).origin;
    let dest = c.graph.node(b).origin;
    let mut bot = CourseBot::new(start, 100.0);
    c.place(&mut bot);
    bot.nav.follower = Some(PathFollower::new(vec![p, a, b], c.now));
    bot.nav.goal = Some(b);
    Some(c.run(&mut bot, dest, 25.0, fps, None))
}

fn report(kind: LinkKind, results: &[((NodeId, NodeId), Outcome)]) -> f64 {
    let ok = results.iter().filter(|(_, o)| clean(o)).count();
    eprintln!("{}: {ok}/{} carried out", kind.as_str(), results.len());
    for ((a, b), o) in results.iter().filter(|(_, o)| !clean(o)).take(12) {
        eprintln!(
            "  {a} -> {b}: {:?}, ended at {:?}, phases {:?}, log {:?}",
            o.failures.first().map(|f| f.reason),
            o.end,
            o.phases.iter().map(|(t, p)| format!("{t:.1}:{p}")).collect::<Vec<_>>(),
            o.log
        );
    }
    ok as f64 / results.len().max(1) as f64
}

#[test]
fn every_lift_carries_a_bot_up() {
    let Some(mut c) = crossfire() else { return };
    let lifts = links_of(&c.graph, LinkKind::Lift);
    assert!(!lifts.is_empty());
    let results: Vec<_> = lifts
        .iter()
        .map(|&(a, b)| ((a, b), run_link(&mut c, a, b, 100.0, None)))
        .collect();
    let rate = report(LinkKind::Lift, &results);
    assert!(rate >= 0.95, "lifts: {rate}");
    let phases: Vec<&str> = results[0].1.phases.iter().map(|(_, p)| *p).collect();
    for p in ["lift:start", "lift:ride", "lift:exit"] {
        assert!(phases.contains(&p), "{phases:?}");
    }
}

#[test]
fn jumps_drops_and_ladders_are_carried_out() {
    let Some(mut c) = crossfire() else { return };
    // The live server runs at 1000 fps: executors must not count ticks where they mean tries.
    for fps in [100.0, 1000.0] {
        for (kind, need) in [(LinkKind::Jump, 0.9), (LinkKind::Drop, 0.9)] {
            let links = links_of(&c.graph, kind);
            let results: Vec<_> = links
                .iter()
                .map(|&(a, b)| ((a, b), run_link(&mut c, a, b, fps, None)))
                .collect();
            eprint!("{fps} fps: ");
            let rate = report(kind, &results);
            assert!(rate >= need, "{fps} fps, {}: {rate}", kind.as_str());
        }
    }
}

/// The same links entered running, as a bot following a path does.
#[test]
fn jumps_and_drops_entered_at_speed() {
    let Some(mut c) = crossfire() else { return };
    for (kind, need) in [(LinkKind::Jump, 0.9), (LinkKind::Drop, 0.9)] {
        let links = links_of(&c.graph, kind);
        let results: Vec<_> = links
            .iter()
            .filter_map(|&(a, b)| run_link_moving(&mut c, a, b, 1000.0).map(|o| ((a, b), o)))
            .collect();
        eprint!("entered running, 1000 fps: ");
        let rate = report(kind, &results);
        assert!(rate >= need, "{}: {rate}", kind.as_str());
    }
}

#[test]
fn walks_arrive() {
    let Some(mut c) = crossfire() else { return };
    let walks = links_of(&c.graph, LinkKind::Walk);
    let sample: Vec<_> = walks.iter().step_by(walks.len() / 150 + 1).copied().collect();
    let results: Vec<_> = sample
        .iter()
        .map(|&(a, b)| ((a, b), run_link(&mut c, a, b, 100.0, None)))
        .collect();
    let rate = report(LinkKind::Walk, &results);
    assert!(rate >= 0.97, "walks: {rate}");
}

#[test]
#[ignore]
fn debug_one_lift() {
    use lb_nav::exec::MechView;
    let Some(mut c) = crossfire() else { return };
    let lifts = links_of(&c.graph, LinkKind::Lift);
    let (a, b) = lifts[0];
    let spec = *c.graph.spec(c.graph.find_link(a, b).unwrap()).unwrap();
    eprintln!("link {a} -> {b}: {spec:?}");
    let start = c.graph.node(a).origin;
    let dest = c.graph.node(b).origin;
    let mut bot = CourseBot::new(start, 100.0);
    bot.nav.follower = Some(PathFollower::new(vec![a, b], c.now));
    bot.nav.goal = Some(b);
    for i in 0..300 {
        let st = c.frame(&mut bot, dest, 10.0);
        if i % 10 == 0 {
            let model = match spec.action {
                lb_nav::spec::Action::Lift { platform, .. } => platform.model,
                _ => 0,
            };
            eprintln!(
                "{i:>3} {st:?} phase {} origin {:?} ground {:?} mover {:?} view {:?}",
                bot.nav.phase(),
                bot.player.origin,
                bot.player.ground,
                c.game.mover(model),
                bot.motor.view
            );
        }
    }
}

/// Floor → ladder nodes → floor routes up and down every ladder: a bot boards from the floor, climbs and steps off.
fn ladder_routes(g: &NavGraph) -> Vec<Vec<NodeId>> {
    use lb_nav::NodeFlags;
    let ladder = |n: NodeId| g.node(n).flags.contains(NodeFlags::LADDER);
    // Onto and off a ladder: a ladder link, or a walk to or from a ladder node standing at its foot or top.
    let usable = |l: &lb_nav::NavLink| l.valid() && matches!(l.kind, LinkKind::Ladder | LinkKind::Walk);
    let mut routes = Vec::new();
    for f in (0..g.len() as NodeId).filter(|&n| !ladder(n) && !g.node(n).flags.contains(NodeFlags::AIRBORNE)) {
        for l in g.links(f).iter().filter(|l| usable(l) && ladder(l.to)) {
            for up in [true, false] {
                let mut path = vec![f, l.to];
                let mut cur = l.to;
                loop {
                    let z = g.node(cur).origin.z;
                    let next = g
                        .links(cur)
                        .iter()
                        .filter(|k| k.valid() && k.kind == LinkKind::Ladder && ladder(k.to) && !path.contains(&k.to))
                        .filter(|k| {
                            if up {
                                g.node(k.to).origin.z > z + 8.0
                            } else {
                                g.node(k.to).origin.z < z - 8.0
                            }
                        })
                        .max_by(|a, b| {
                            let (za, zb) = (g.node(a.to).origin.z, g.node(b.to).origin.z);
                            if up { za.total_cmp(&zb) } else { zb.total_cmp(&za) }
                        });
                    let Some(next) = next else { break };
                    path.push(next.to);
                    cur = next.to;
                }
                // Off the ladder onto a floor well above (or below) where the route started.
                let z0 = g.node(f).origin.z;
                let from = g.node(cur).origin;
                let off = g
                    .links(cur)
                    .iter()
                    .filter(|k| usable(k) && !ladder(k.to) && !path.contains(&k.to))
                    .filter(|k| {
                        if up {
                            g.node(k.to).origin.z > z0 + 40.0
                        } else {
                            g.node(k.to).origin.z < z0 - 40.0
                        }
                    })
                    .min_by(|a, b| {
                        g.node(a.to)
                            .origin
                            .distance(from)
                            .total_cmp(&g.node(b.to).origin.distance(from))
                    });
                if let Some(off) = off {
                    path.push(off.to);
                    routes.push(path);
                }
            }
        }
    }
    routes.sort();
    routes.dedup();
    routes
}

#[test]
fn ladders_are_climbed_up_and_down() {
    let Some(mut c) = crossfire() else { return };
    let routes = ladder_routes(&c.graph);
    assert!(!routes.is_empty());
    let mut failed = Vec::new();
    for path in &routes {
        c.settle(5.0);
        let start = c.graph.node(path[0]).origin;
        let dest = c.graph.node(*path.last().unwrap()).origin;
        let mut bot = CourseBot::new(start, 100.0);
        c.place(&mut bot);
        bot.nav.follower = Some(PathFollower::new(path.clone(), c.now));
        bot.nav.goal = path.last().copied();
        let o = c.run(&mut bot, dest, 30.0, 100.0, None);
        if !clean(&o) {
            failed.push((path.clone(), o));
        }
    }
    eprintln!(
        "ladder routes: {}/{} climbed",
        routes.len() - failed.len(),
        routes.len()
    );
    for (path, o) in failed.iter().take(8) {
        eprintln!(
            "  {path:?}: {:?} ended at {:?}, phases {:?}",
            o.failures.first().map(|f| (f.from, f.to, f.reason)),
            o.end,
            o.phases.iter().map(|(t, p)| format!("{t:.1}:{p}")).collect::<Vec<_>>()
        );
    }
    assert!(
        failed.len() * 10 <= routes.len(),
        "{} of {} ladder routes failed",
        failed.len(),
        routes.len()
    );
}

#[test]
#[ignore]
fn debug_one_ladder() {
    let Some(mut c) = crossfire() else { return };
    let path: Vec<NodeId> = std::env::var("LB_PATH")
        .unwrap_or("96,0,1564".into())
        .split(',')
        .map(|x| x.parse().unwrap())
        .collect();
    for w in path.windows(2) {
        let l = c.graph.find_link(w[0], w[1]).unwrap();
        eprintln!(
            "{} -> {}: {:?} {:?}",
            w[0],
            w[1],
            l.kind,
            c.graph.spec(l).map(|s| s.action)
        );
    }
    let start = c.graph.node(path[0]).origin;
    let dest = c.graph.node(*path.last().unwrap()).origin;
    let mut bot = CourseBot::new(start, 100.0);
    c.place(&mut bot);
    bot.nav.follower = Some(PathFollower::new(path.clone(), c.now));
    bot.nav.goal = path.last().copied();
    for i in 0..400 {
        let st = c.frame(&mut bot, dest, 10.0);
        if i % 10 == 0 {
            let p = bot.player;
            eprintln!(
                "{i:>3} {st:?} {} o {:?} v {:?} ladder {} ground {:?} view {:?} target {:?}",
                bot.nav.phase(),
                p.origin,
                p.velocity,
                p.on_ladder,
                p.ground,
                bot.motor.view,
                bot.nav.follower.as_ref().and_then(|f| f.target())
            );
        }
    }
}

#[test]
#[ignore]
fn debug_node_solid() {
    use lb_worldq::{HullKind, TraceQuery, Tracer};
    let Some(mut c) = crossfire() else { return };
    for n in [93u32, 1257] {
        let o = c.graph.node(n).origin;
        let tr = c.world.trace(&TraceQuery::hull(o, o, HullKind::Stand));
        eprintln!(
            "node {n} {o:?} {:?}: start_solid {} hit {:?}",
            c.graph.node(n).flags,
            tr.start_solid,
            tr.hit
        );
        if let Some(h) = tr.hit.filter(|h| *h != 0) {
            let b = c
                .world
                .brush(h as usize)
                .map(|b| (b.classname.clone(), b.abs_mins(), b.abs_maxs(), b.offset));
            eprintln!("  brush {b:?} mover {:?}", c.game.mover_offset(h as u16));
        }
    }
}

/// The same traversals at several server frame rates and with a 300 ms frame halfway: success holds and no button
/// stays pressed afterwards.
#[test]
fn frame_rates_and_long_frames_do_not_break_traversals() {
    let Some(mut c) = crossfire() else { return };
    let mut sample: Vec<(NodeId, NodeId)> = Vec::new();
    for kind in [LinkKind::Lift, LinkKind::Jump, LinkKind::Drop] {
        let links = links_of(&c.graph, kind);
        sample.extend(links.iter().step_by(links.len() / 8 + 1).copied());
    }
    for (fps, long) in [
        (100.0, None),
        (500.0, None),
        (1000.0, None),
        (100.0, Some(300.0)),
        (1000.0, Some(300.0)),
    ] {
        let mut ok = 0;
        for &(a, b) in &sample {
            let o = run_link(&mut c, a, b, fps, long);
            assert_eq!(
                o.held, 0,
                "fps {fps}, long frame {long:?}: buttons {:#x} left pressed after {a} -> {b}",
                o.held
            );
            if clean(&o) {
                ok += 1;
            } else {
                eprintln!(
                    "fps {fps} long {long:?}: {a} -> {b} failed {:?} at {:?}, phases {:?}",
                    o.failures.first().map(|f| f.reason),
                    o.end,
                    o.phases.iter().map(|(t, p)| format!("{t:.1}:{p}")).collect::<Vec<_>>()
                );
            }
        }
        eprintln!("fps {fps}, long frame {long:?}: {ok}/{} carried out", sample.len());
        assert!(
            ok * 10 >= sample.len() * 9,
            "fps {fps} long {long:?}: {ok}/{}",
            sample.len()
        );
    }
}

#[test]
#[ignore]
fn debug_lift_1000fps() {
    use lb_nav::exec::MechView;
    let Some(mut c) = crossfire() else { return };
    let (a, b) = (247u32, 925u32);
    let start = c.graph.node(a).origin;
    let dest = c.graph.node(b).origin;
    let mut bot = CourseBot::new(start, 100.0);
    c.place(&mut bot);
    bot.nav.follower = Some(PathFollower::new(vec![a, b], c.now));
    bot.nav.goal = Some(b);
    for i in 0..2500 {
        let st = c.frame(&mut bot, dest, 1.0);
        if i % 25 == 0 || (700..1000).contains(&i) && i % 5 == 0 {
            let p = bot.player;
            eprintln!(
                "{i:>4} {st:?} {} o {:?} v {:?} ground {:?} mover {:?}",
                bot.nav.phase(),
                p.origin,
                p.velocity,
                p.ground,
                c.game.mover(34)
            );
        }
        if st == lb_nav_api::NavStatus::Arrived {
            break;
        }
    }
}

#[test]
#[ignore]
fn debug_inbound() {
    let Some(c) = crossfire() else { return };
    let target: NodeId = std::env::var("LB_NODE")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(247);
    let g = &c.graph;
    eprintln!(
        "node {target}: {:?} {:?} support {}",
        g.node(target).origin,
        g.node(target).flags,
        g.node(target).support
    );
    for a in 0..g.len() as NodeId {
        for l in g.links(a).iter().filter(|l| l.to == target) {
            eprintln!(
                "  {a} -> {target}: {} valid {} flags {:?} from {:?} {:?}",
                l.kind.as_str(),
                l.valid(),
                l.flags,
                g.node(a).origin,
                g.node(a).flags
            );
        }
    }
    for l in g.links(target) {
        eprintln!("  {target} -> {}: {} valid {}", l.to, l.kind.as_str(), l.valid());
    }
}

/// Whole routes the navigator plans itself between random places: nearly all arrive, in about the planned time.
#[test]
fn random_routes_arrive_in_about_the_planned_time() {
    use lb_core::rng::Pcg32;
    use lb_nav::NodeFlags;
    let Some(mut c) = crossfire() else { return };
    let unfit = NodeFlags::AIRBORNE | NodeFlags::LADDER | NodeFlags::WATER | NodeFlags::ON_MOVER;
    let nodes: Vec<NodeId> = (0..c.graph.len() as NodeId)
        .filter(|&n| !c.graph.node(n).flags.intersects(unfit))
        .collect();
    let mut rng = Pcg32::new(42, 1);
    let (mut tried, mut arrived, mut detours) = (0, 0, 0);
    let mut ratios = Vec::new();
    let mut failed = Vec::new();
    while tried < 60 {
        let a = nodes[rng.range_i32(0, nodes.len() as i32 - 1) as usize];
        let b = nodes[rng.range_i32(0, nodes.len() as i32 - 1) as usize];
        let (pa, pb) = (c.graph.node(a).origin, c.graph.node(b).origin);
        if pa.distance(pb) < 600.0 {
            continue;
        }
        let Some(path) = lb_nav::plan::plan(&c.graph, a, b, &|_, _| 0.0) else {
            continue;
        };
        let planned = lb_nav::plan::path_time(&c.graph, &path) as f64;
        tried += 1;
        c.settle(5.0);
        let mut bot = CourseBot::new(pa, 100.0);
        c.place(&mut bot);
        let o = c.run(&mut bot, pb, (planned * 4.0).max(30.0), 100.0, None);
        if o.arrived {
            arrived += 1;
            detours += usize::from(!o.failures.is_empty());
            ratios.push(o.seconds / planned.max(0.5));
        } else {
            failed.push((a, b, planned, o));
        }
    }
    ratios.sort_by(f64::total_cmp);
    let median = ratios.get(ratios.len() / 2).copied().unwrap_or(0.0);
    eprintln!(
        "routes: {arrived}/{tried} arrived ({} of them around a failed link), time / planned: median {median:.2}, \
         worst {:.2}",
        detours,
        ratios.last().unwrap_or(&0.0)
    );
    for (a, b, planned, o) in failed.iter().take(10) {
        eprintln!(
            "  {a} -> {b} (planned {planned:.1} s): stopped at {:?} after {:.1} s, failures {:?}, phases {:?}",
            o.end,
            o.seconds,
            o.failures.iter().map(|f| (f.from, f.to, f.reason)).collect::<Vec<_>>(),
            o.phases
                .iter()
                .rev()
                .take(6)
                .map(|(t, p)| format!("{t:.1}:{p}"))
                .collect::<Vec<_>>()
        );
    }
    assert!(arrived * 100 >= tried * 90, "{arrived}/{tried}");
    assert!(median <= 1.3, "median time / planned {median}");
}

#[test]
#[ignore]
fn debug_reach() {
    use std::collections::VecDeque;
    let Some(c) = crossfire() else { return };
    let g = &c.graph;
    let start: NodeId = std::env::var("LB_NODE")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(1188);
    let mut seen = vec![false; g.len()];
    let mut q = VecDeque::from([start]);
    seen[start as usize] = true;
    while let Some(n) = q.pop_front() {
        for l in g.links(n).iter().filter(|l| l.valid()) {
            if !seen[l.to as usize] {
                seen[l.to as usize] = true;
                q.push_back(l.to);
            }
        }
    }
    let unreached: Vec<NodeId> = (0..g.len() as NodeId).filter(|&n| !seen[n as usize]).collect();
    eprintln!(
        "from {start}: {} of {} nodes reachable",
        g.len() - unreached.len(),
        g.len()
    );
    // Unreached nodes with a link from a reached one: where the way in is cut.
    for &n in &unreached {
        for a in (0..g.len() as NodeId).filter(|&a| seen[a as usize]) {
            if let Some(l) = g.find_link(a, n) {
                eprintln!(
                    "  cut {a} -> {n}: {} valid {} {:?} -> {:?}",
                    l.kind.as_str(),
                    l.valid(),
                    g.node(a).origin,
                    g.node(n).origin
                );
            }
        }
    }
}
