//! Long jumps along the way on the maps: how much a check of one costs, and how routes go with them. Slow: run with
//! `--release --ignored --nocapture`. Skipped without the maps.

use std::time::Instant;

use lb_bsp::BspWorld;
use lb_bsp::mech::Mechanisms;
use lb_core::rng::Pcg32;
use lb_kin::Physics;
use lb_kin::pmove::Player;
use lb_kin::tricks::{Traced, longjump_from};
use lb_nav::follow::TrickKind;
use lb_nav::plan::{plain, plan};
use lb_nav::{LinkKind, NavGraph, NodeId};
use lb_nav_api::Tricks;
use lb_navgen::{GenOptions, generate};
use lb_testkit::course::{Course, CourseBot, Game};

fn load(map: &str) -> Option<(BspWorld, Mechanisms, NavGraph)> {
    let maps = lb_bsp::test_maps_dir()?;
    let bsp = std::fs::read(maps.join(format!("{map}.bsp"))).ok()?;
    let mut world = BspWorld::load(&bsp).unwrap();
    let mech = Mechanisms::from_world(&world);
    let generated = generate(&mut world, &mech, &GenOptions::default(), map);
    Some((world, mech, generated.graph))
}

/// Long jumps checked from nodes of random paths onto the node 300–470 units further along.
#[test]
#[ignore]
fn a_long_jump_check_costs() {
    for map in ["dm_snow", "crossfire", "stalkyard"] {
        let Some((mut world, _, g)) = load(map) else {
            eprintln!("{map}: no map");
            continue;
        };
        let phys = Physics::default();
        let mut rng = Pcg32::new(3, 3);
        let n = g.len() as i32;
        let (mut sims, mut ok, mut elapsed) = (0u32, 0u32, 0.0f64);
        let mut worst = 0.0f64;
        while sims < 300 {
            let (a, b) = (rng.range_i32(0, n - 1) as NodeId, rng.range_i32(0, n - 1) as NodeId);
            let Some(path) = plan(&g, a, b, &plain) else { continue };
            for w in path.windows(2) {
                if !g.find_link(w[0], w[1]).is_some_and(|l| l.kind == LinkKind::Walk) {
                    continue;
                }
                let from = g.node(w[0]).origin;
                let Some(&to) = path.iter().find(|&&k| {
                    let d = (g.node(k).origin - from).truncate().length();
                    (300.0..470.0).contains(&d)
                }) else {
                    continue;
                };
                let to = g.node(to).origin;
                let dir = (to - from).truncate().normalize_or_zero();
                let mut p = Player::standing(from);
                p.velocity = (dir * 270.0).extend(0.0);
                p.longjump = true;
                let yaw = lb_core::math::dir_to_view_angles(dir.extend(0.0)).y;
                let started = Instant::now();
                let v = longjump_from(&mut Traced(&mut world), &phys, p, yaw, Some(to));
                let t = started.elapsed().as_secs_f64();
                elapsed += t;
                worst = worst.max(t);
                sims += 1;
                ok += u32::from(v.ok);
                break;
            }
        }
        eprintln!(
            "{map}: {sims} checks, {ok} land, {:.0} µs each on average, worst {:.0} µs",
            elapsed / f64::from(sims) * 1e6,
            worst * 1e6
        );
    }
}

/// How a set of routes went for one kind of bot.
#[derive(Default)]
struct Runs {
    arrived: u32,
    seconds: f64,
    landed: u32,
    missed: u32,
    failures: u32,
    hurt: f32,
    /// Seconds of the frames, for the cost of the checks.
    cpu: f64,
    /// Seconds lining up and taking off on long jumps of the way, and in the air on them.
    lining_up: f64,
    air: f64,
}

/// Runs the route; its time when the bot got there.
fn run_route(c: &mut Course<BspWorld>, from: NodeId, to: NodeId, tricks: Tricks, into: &mut Runs) -> Option<f64> {
    c.settle(10.0);
    let (start, dest) = (c.graph.node(from).origin, c.graph.node(to).origin);
    let mut bot = CourseBot::new(start, 100.0);
    bot.tricks = tricks;
    c.place(&mut bot);
    bot.nav.goal = Some(to);
    let started = Instant::now();
    let o = c.run(&mut bot, dest, 60.0, 100.0, None);
    into.cpu += started.elapsed().as_secs_f64();
    let t = bot.nav.tricks;
    let i = TrickKind::Runway as usize;
    into.landed += t.landed[i];
    into.missed += t.missed[i];
    into.failures += o.failures.len() as u32;
    for (w, next) in o.phases.windows(2).map(|w| (w[0], w[1].0)) {
        match w.1 {
            "longjump:run" | "longjump:takeoff" => into.lining_up += next - w.0,
            "longjump:air" => into.air += next - w.0,
            _ => {}
        }
    }
    into.hurt += 100.0 - bot.health;
    if o.arrived {
        into.arrived += 1;
        into.seconds += o.seconds;
    } else if std::env::var("LB_LOG").is_ok() {
        eprintln!(
            "  not there: {from} -> {to}, failures {:?}, ended at {:?}",
            o.failures.iter().map(|f| (f.from, f.to, f.reason)).collect::<Vec<_>>(),
            o.end
        );
    }
    if tricks.runway_bold && bot.nav.tricks.missed[TrickKind::Runway as usize] > 0 && std::env::var("LB_LOG").is_ok() {
        eprintln!("  missed on the way {from} -> {to}");
    }
    if std::env::var("LB_LOG").is_ok() && !o.failures.is_empty() && tricks.runway_bold {
        for f in &o.failures {
            let kind = c.graph.find_link(f.from, f.to).map(|l| l.kind.as_str());
            eprintln!("  failure {} -> {} {kind:?}: {:?}", f.from, f.to, f.reason);
        }
    }
    o.arrived.then_some(o.seconds)
}

/// Routes between random places at least 1000 units apart by the way, run without the module, and with it
/// taking long jumps along the way.
#[test]
#[ignore]
fn routes_with_long_jumps_along_the_way() {
    let maps: Vec<String> = std::env::var("LB_MAPS")
        .map(|m| m.split(',').map(str::to_string).collect())
        .unwrap_or_else(|_| vec!["dm_snow".into(), "crossfire".into(), "stalkyard".into()]);
    let routes: usize = std::env::var("LB_ROUTES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(40);
    if std::env::var("LB_LOG").is_ok() {
        tracing_subscriber::fmt()
            .with_writer(std::io::stderr)
            .with_target(false)
            .init();
    }
    for map in &maps {
        let Some((mut world, mech, g)) = load(map) else {
            eprintln!("{map}: no map");
            continue;
        };
        let game = Game::from_map(&mut world, &mech);
        let mut c = Course::new(world, game, g);
        let mut rng = Pcg32::new(11, 11);
        let n = c.graph.len() as i32;
        let mut pairs = Vec::new();
        while pairs.len() < routes {
            let (a, b) = (rng.range_i32(0, n - 1) as NodeId, rng.range_i32(0, n - 1) as NodeId);
            if let Some(path) = plan(&c.graph, a, b, &plain)
                && path_length(&c.graph, &path) >= 1000.0
            {
                pairs.push((a, b));
            }
        }
        let kinds = [
            ("no module", Tricks::default()),
            (
                "long jumps",
                Tricks {
                    longjump: true,
                    runway: true,
                    ..Tricks::default()
                },
            ),
            (
                "bold",
                Tricks {
                    longjump: true,
                    runway: true,
                    runway_bold: true,
                    runway_hurt: 60.0,
                    ..Tricks::default()
                },
            ),
        ];
        let mut runs: Vec<Runs> = kinds.iter().map(|_| Runs::default()).collect();
        // Over the routes every kind of bot got to the end of.
        let mut total = vec![0.0f64; kinds.len()];
        for &(a, b) in &pairs {
            let times: Vec<Option<f64>> = kinds
                .iter()
                .enumerate()
                .map(|(k, (_, tricks))| run_route(&mut c, a, b, *tricks, &mut runs[k]))
                .collect();
            if times.iter().all(Option::is_some) {
                for (t, x) in total.iter_mut().zip(&times) {
                    *t += x.unwrap_or(0.0);
                }
            }
        }
        eprintln!("{map}: {} routes", pairs.len());
        for ((name, _), r) in kinds.iter().zip(&runs) {
            let jumps = f64::from((r.landed + r.missed).max(1));
            eprintln!(
                "  {name:<12} arrived {}/{}, {:.1} s on average, long jumps {} landed {} missed ({:.1} a minute, \
                 {:.2} s lining up and {:.2} s in the air each, {:.0}% of the time in the air), {} failures, {:.0} \
                 health lost, {:.0} µs a frame",
                r.arrived,
                pairs.len(),
                r.seconds / f64::from(r.arrived.max(1)),
                r.landed,
                r.missed,
                f64::from(r.landed + r.missed) / (r.seconds / 60.0).max(1e-3),
                r.lining_up / jumps,
                r.air / jumps,
                r.air / r.seconds.max(1e-3) * 100.0,
                r.failures,
                r.hurt,
                r.cpu / (r.seconds.max(1e-3) * 100.0) * 1e6
            );
        }
        for (k, (name, _)) in kinds.iter().enumerate().skip(1) {
            eprintln!(
                "  {name}: {:.0}% of the time without",
                total[k] / total[0].max(1e-3) * 100.0
            );
        }
    }
}

fn path_length(g: &NavGraph, path: &[NodeId]) -> f32 {
    path.windows(2)
        .map(|w| (g.node(w[1]).origin - g.node(w[0]).origin).length())
        .sum()
}

/// One route on `LB_MAP` from `LB_FROM` to `LB_TO` with bold long jumps, frame by frame while long jumping.
#[test]
#[ignore]
fn debug_route_long_jumps() {
    let map = std::env::var("LB_MAP").unwrap_or_else(|_| "crossfire".into());
    let from: NodeId = std::env::var("LB_FROM").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let to: NodeId = std::env::var("LB_TO").ok().and_then(|v| v.parse().ok()).unwrap_or(1);
    let Some((mut world, mech, g)) = load(&map) else {
        return;
    };
    let game = Game::from_map(&mut world, &mech);
    let mut c = Course::new(world, game, g);
    c.settle(10.0);
    let (start, dest) = (c.graph.node(from).origin, c.graph.node(to).origin);
    let mut bot = CourseBot::new(start, 100.0);
    bot.tricks = Tricks {
        longjump: true,
        runway: true,
        runway_bold: true,
        runway_hurt: 60.0,
        ..Tricks::default()
    };
    c.place(&mut bot);
    bot.nav.goal = Some(to);
    let started = c.now;
    while c.now - started < 60.0 {
        let status = c.frame(&mut bot, dest, 10.0);
        let phase = bot.nav.phase();
        if phase.starts_with("longjump") {
            let p = bot.player;
            eprintln!(
                "{:.2} {phase:<18} at {:.0} {:.0} {:.0} v {:.0} {:.0} {:.0} view {:.0} {:.0} ground {} ducked {}",
                c.now - started,
                p.origin.x,
                p.origin.y,
                p.origin.z,
                p.velocity.x,
                p.velocity.y,
                p.velocity.z,
                bot.motor.view.x,
                bot.motor.view.y,
                p.on_ground(),
                p.ducked
            );
        }
        if status == lb_nav_api::NavStatus::Arrived {
            break;
        }
    }
    eprintln!("{:.1} s, tricks {:?}", c.now - started, bot.nav.tricks);
}
