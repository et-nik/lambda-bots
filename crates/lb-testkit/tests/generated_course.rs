//! The generated graphs of the standard maps on the obstacle course: a sample of every kind of special link carried
//! out by a simulated bot from its entry to its exit, and routes from spawn points to every item the coverage
//! report counts as reachable. Slow (a quarter of a minute on eight cores): run with `--ignored`. Skipped without
//! the maps.

use lb_bsp::BspWorld;
use lb_bsp::mech::Mechanisms;
use lb_nav::follow::PathFollower;
use lb_nav::{LinkKind, NavGraph, NodeFlags, NodeId};
use lb_navgen::report::coverage;
use lb_navgen::{GenOptions, generate};
use lb_testkit::course::{Course, CourseBot, Gait, Game, Outcome};

const MAPS: [&str; 12] = [
    "boot_camp",
    "bounce",
    "crossfire",
    "datacore",
    "frenzy",
    "gasworks",
    "lambda_bunker",
    "rapidcore",
    "snark_pit",
    "stalkyard",
    "subtransit",
    "undertow",
];

/// Special links tried per kind and map.
const PER_KIND: usize = 40;

fn clean(o: &Outcome) -> bool {
    o.arrived && o.failures.is_empty()
}

fn run_link(c: &mut Course<BspWorld>, a: NodeId, b: NodeId) -> Outcome {
    c.settle(10.0);
    let (start, dest) = (c.graph.node(a).origin, c.graph.node(b).origin);
    let mut bot = CourseBot::new(start, 100.0);
    // Tricks the links need: the long jump module, a gauss with its uranium.
    bot.tricks = lb_nav_api::Tricks {
        longjump: true,
        runway: false,
        gauss_boost: true,
        boost_now: true,
        gauss_damage: 200.0,
        selfgauss: true,
    };
    c.place(&mut bot);
    bot.nav.follower = Some(PathFollower::new(vec![a, b], c.now));
    bot.nav.goal = Some(b);
    c.run(&mut bot, dest, 30.0, 100.0, None)
}

fn run_route(c: &mut Course<BspWorld>, from: NodeId, to: NodeId) -> Outcome {
    c.settle(10.0);
    let (start, dest) = (c.graph.node(from).origin, c.graph.node(to).origin);
    let mut bot = CourseBot::new(start, 100.0);
    c.place(&mut bot);
    bot.nav.goal = Some(to);
    c.run(&mut bot, dest, 120.0, 100.0, None)
}

fn sample(g: &NavGraph, kind: LinkKind) -> Vec<(NodeId, NodeId)> {
    let all: Vec<(NodeId, NodeId)> = (0..g.len() as NodeId)
        .filter(|&a| !g.node(a).flags.contains(NodeFlags::AIRBORNE))
        .flat_map(|a| {
            g.links(a)
                .iter()
                .filter(|l| l.kind == kind && l.valid())
                .map(move |l| (a, l.to))
        })
        .collect();
    let step = all.len() / PER_KIND + 1;
    all.into_iter().step_by(step).collect()
}

struct MapResult {
    lines: Vec<String>,
    /// (kind, clean, tried).
    kinds: Vec<(LinkKind, usize, usize)>,
    routes: (usize, usize),
    /// Over the routes.
    gait: Gait,
}

fn check_map(map: &str) -> Option<MapResult> {
    let maps = lb_bsp::test_maps_dir()?;
    let bsp = std::fs::read(maps.join(format!("{map}.bsp"))).ok()?;
    let mut world = BspWorld::load(&bsp).unwrap();
    let mech = Mechanisms::from_world(&world);
    let generated = generate(&mut world, &mech, &GenOptions::default(), map);
    let cov = coverage(&generated);
    let (spawns, items) = (generated.spawns.clone(), generated.items.clone());
    let game = Game::from_map(&mut world, &mech);
    let mut c = Course::new(world, game, generated.graph);
    let mut lines = Vec::new();
    let mut kinds = Vec::new();
    for kind in LinkKind::ALL.into_iter().filter(|k| !k.is_walk()) {
        let links = sample(&c.graph, kind);
        if links.is_empty() {
            continue;
        }
        let mut ok = 0;
        for &(a, b) in &links {
            let o = run_link(&mut c, a, b);
            if clean(&o) {
                ok += 1;
            } else if lines.len() < 40 {
                lines.push(format!(
                    "  {map} {} {a} {:?} -> {b} {:?}: {:?}, ended at {:?}, phases {:?}",
                    kind.as_str(),
                    c.graph.node(a).origin,
                    c.graph.node(b).origin,
                    o.failures.first().map(|f| f.reason),
                    o.end,
                    o.phases.iter().map(|(t, p)| format!("{t:.1}:{p}")).collect::<Vec<_>>()
                ));
            }
        }
        kinds.push((kind, ok, links.len()));
    }
    // Every item the report counts as reachable, from the nearest spawn point.
    let reach = |n: NodeId| cov.roundtrip_nodes.get(n as usize).copied().unwrap_or(false);
    let mut routes = (0, 0);
    let mut gait = Gait::default();
    for &item in items.iter().filter(|&&i| reach(i)) {
        let Some(&spawn) = spawns.iter().min_by(|&&x, &&y| {
            let at = c.graph.node(item).origin;
            c.graph
                .node(x)
                .origin
                .distance(at)
                .total_cmp(&c.graph.node(y).origin.distance(at))
        }) else {
            continue;
        };
        let o = run_route(&mut c, spawn, item);
        gait.add(&o.gait);
        routes.1 += 1;
        if o.arrived {
            routes.0 += 1;
        } else if lines.len() < 60 {
            let tail = o.phases.len().saturating_sub(8);
            lines.push(format!(
                "  {map} route {spawn} {:?} -> item {item} {:?}: ended at {:?} after {:.0} s, failures {:?}, last \
                 phases {:?}",
                c.graph.node(spawn).origin,
                c.graph.node(item).origin,
                o.end,
                o.seconds,
                o.failures.iter().map(|f| f.reason).collect::<Vec<_>>(),
                o.phases[tail..]
                    .iter()
                    .map(|(t, p)| format!("{t:.1}:{p}"))
                    .collect::<Vec<_>>()
            ));
        }
    }
    Some(MapResult {
        lines,
        kinds,
        routes,
        gait,
    })
}

#[test]
#[ignore]
fn generated_graphs_are_walked() {
    let results: Vec<(&str, Option<MapResult>)> = std::thread::scope(|s| {
        let handles: Vec<_> = MAPS.iter().map(|&m| (m, s.spawn(move || check_map(m)))).collect();
        handles.into_iter().map(|(m, h)| (m, h.join().unwrap())).collect()
    });
    let (mut routes_ok, mut routes_all) = (0, 0);
    let mut by_kind: Vec<(LinkKind, usize, usize)> = Vec::new();
    let mut gait = Gait::default();
    let walked = |g: &Gait| {
        format!(
            "walking {:.0} s: against a wall {:.1}%, looking steeply {:.1}%, view turning {:.0} deg/s",
            g.walking,
            g.walled * 100.0 / g.walking.max(1e-9),
            g.steep * 100.0 / g.walking.max(1e-9),
            g.turned / g.walking.max(1e-9)
        )
    };
    for (map, r) in &results {
        let Some(r) = r else { continue };
        let kinds: Vec<String> = r
            .kinds
            .iter()
            .map(|(k, ok, n)| format!("{} {ok}/{n}", k.as_str()))
            .collect();
        eprintln!("{map}: routes {}/{}; {}", r.routes.0, r.routes.1, kinds.join(", "));
        eprintln!("  {}", walked(&r.gait));
        gait.add(&r.gait);
        for l in &r.lines {
            eprintln!("{l}");
        }
        routes_ok += r.routes.0;
        routes_all += r.routes.1;
        for &(k, ok, n) in &r.kinds {
            match by_kind.iter_mut().find(|(kk, ..)| *kk == k) {
                Some(e) => {
                    e.1 += ok;
                    e.2 += n;
                }
                None => by_kind.push((k, ok, n)),
            }
        }
    }
    if routes_all == 0 {
        return;
    }
    let kinds: Vec<String> = by_kind
        .iter()
        .map(|(k, ok, n)| format!("{} {ok}/{n}", k.as_str()))
        .collect();
    eprintln!("all maps: routes {routes_ok}/{routes_all}; {}", kinds.join(", "));
    eprintln!("all maps: {}", walked(&gait));
    // 0.93 on 27.09.2026 (docs/m3-acceptance.md): doors opened from a remote button are not carried out yet.
    let rate = routes_ok as f64 / routes_all as f64;
    assert!(rate >= 0.9, "routes {rate:.3}");
}

/// One route with a log of where the bot is and which link it follows: `LB_MAP`, `LB_ROUTE=from,to`.
#[test]
#[ignore]
fn debug_route() {
    let Ok(map) = std::env::var("LB_MAP") else { return };
    let ab: Vec<u32> = std::env::var("LB_ROUTE")
        .unwrap()
        .split(',')
        .map(|s| s.parse().unwrap())
        .collect();
    let maps = lb_bsp::test_maps_dir().unwrap();
    let mut world = BspWorld::load(&std::fs::read(maps.join(format!("{map}.bsp"))).unwrap()).unwrap();
    let mech = Mechanisms::from_world(&world);
    let generated = generate(&mut world, &mech, &GenOptions::default(), &map);
    let game = Game::from_map(&mut world, &mech);
    let mut c = Course::new(world, game, generated.graph);
    let (from, to) = (ab[0], ab[1]);
    let dest = c.graph.node(to).origin;
    let mut bot = CourseBot::new(c.graph.node(from).origin, 100.0);
    c.place(&mut bot);
    bot.nav.goal = Some(to);
    let mut last = String::new();
    for i in 0..6000 {
        let st = c.frame(&mut bot, dest, 10.0);
        let link = bot.nav.follower.as_ref().and_then(|f| f.current_link());
        let kind = link.and_then(|(a, b)| c.graph.find_link(a, b)).map(|l| l.kind.as_str());
        let now = format!("{:?} {} {:?}", link, bot.nav.phase(), kind);
        if now != last || i % 100 == 0 {
            eprintln!(
                "{:.2} {st:?} at {:?} ground {:?} ladder {} {now}",
                c.now, bot.player.origin, bot.player.ground, bot.player.on_ladder
            );
            last = now;
        }
        if st == lb_nav_api::NavStatus::Arrived {
            break;
        }
    }
}

/// One link carried out with a log: `LB_MAP`, `LB_LINK=from,to`.
#[test]
#[ignore]
fn debug_link() {
    let Ok(map) = std::env::var("LB_MAP") else { return };
    let ab: Vec<u32> = std::env::var("LB_LINK")
        .unwrap()
        .split(',')
        .map(|s| s.parse().unwrap())
        .collect();
    let maps = lb_bsp::test_maps_dir().unwrap();
    let mut world = BspWorld::load(&std::fs::read(maps.join(format!("{map}.bsp"))).unwrap()).unwrap();
    let mech = Mechanisms::from_world(&world);
    let generated = generate(&mut world, &mech, &GenOptions::default(), &map);
    let game = Game::from_map(&mut world, &mech);
    let mut c = Course::new(world, game, generated.graph);
    let (a, b) = (ab[0], ab[1]);
    let l = c.graph.find_link(a, b).unwrap();
    eprintln!("{l:?}\n{:?}", c.graph.spec(l));
    let dest = c.graph.node(b).origin;
    let mut bot = CourseBot::new(c.graph.node(a).origin, 100.0);
    bot.tricks = lb_nav_api::Tricks {
        longjump: true,
        runway: false,
        gauss_boost: true,
        boost_now: true,
        gauss_damage: 200.0,
        selfgauss: true,
    };
    c.place(&mut bot);
    bot.nav.follower = Some(PathFollower::new(vec![a, b], c.now));
    bot.nav.goal = Some(b);
    let every: usize = std::env::var("LB_EVERY").ok().and_then(|v| v.parse().ok()).unwrap_or(10);
    for i in 0..1500 {
        let st = c.frame(&mut bot, dest, 10.0);
        if i % every == 0 {
            eprintln!(
                "{:.2} {st:?} at {:?} v {:?} ground {:?} {} view {:?} buttons {:#x} ducked {}",
                c.now,
                bot.player.origin,
                bot.player.velocity,
                bot.player.ground,
                bot.nav.phase(),
                bot.motor.view,
                bot.player.oldbuttons,
                bot.player.ducked
            );
        }
        if st == lb_nav_api::NavStatus::Arrived {
            break;
        }
    }
}

/// A long jump link's check replayed: `LB_MAP`, `LB_LINK=from,to`.
#[test]
#[ignore]
fn debug_longjump_check() {
    let Ok(map) = std::env::var("LB_MAP") else { return };
    let ab: Vec<u32> = std::env::var("LB_LINK")
        .unwrap()
        .split(',')
        .map(|s| s.parse().unwrap())
        .collect();
    let maps = lb_bsp::test_maps_dir().unwrap();
    let mut world = BspWorld::load(&std::fs::read(maps.join(format!("{map}.bsp"))).unwrap()).unwrap();
    let mech = Mechanisms::from_world(&world);
    let generated = generate(&mut world, &mech, &GenOptions::default(), &map);
    let g = generated.graph;
    let (a, b) = (g.node(ab[0]).origin, g.node(ab[1]).origin);
    let phys = lb_kin::Physics::default();
    if std::env::var("LB_GAME").is_ok() {
        let _game = Game::from_map(&mut world, &mech);
        eprintln!("mechanisms placed as on the course");
    }
    let dir = (b - a).truncate().normalize().extend(0.0);
    if let (Ok(x), Ok(y), Ok(vx), Ok(vy)) = (
        std::env::var("LB_X").map(|v| v.parse::<f32>().unwrap()),
        std::env::var("LB_Y").map(|v| v.parse::<f32>().unwrap()),
        std::env::var("LB_VX").map(|v| v.parse::<f32>().unwrap()),
        std::env::var("LB_VY").map(|v| v.parse::<f32>().unwrap()),
    ) {
        let mut p = lb_kin::Player::standing(lb_core::Vec3::new(x, y, a.z));
        p.velocity = lb_core::Vec3::new(vx, vy, 0.0);
        let yaw = lb_core::math::dir_to_view_angles((b - p.origin).truncate().extend(0.0)).y;
        let v = lb_kin::tricks::longjump_from(&mut world, &phys, p, yaw, Some(b));
        eprintln!("from the given state: ok {} landing {:?} flight {:.2}", v.ok, v.landing, v.flight);
    }
    for (along, side) in [(0.0f32, 0.0f32), (-12.0, 0.0), (12.0, 0.0), (0.0, -8.0), (0.0, 8.0)] {
        let v = lb_kin::tricks::simulate_longjump(&mut world, &phys, a + dir * along, b + dir * along, side);
        eprintln!("along {along} side {side}: ok {} landing {:?} flight {:.2} impact {:.0}", v.ok, v.landing, v.flight, v.impact);
    }
}
