//! Bunny hopping along the way: a straight corridor, an L of two, a low ceiling and a jump link at the end of the
//! hops, driven through navigation, the motor, the command driver and the movement code at 1000 frames a second;
//! and, slow (`--release --ignored --nocapture`, skipped without the maps), routes on the maps hopped and walked.

use lb_bsp::BspWorld;
use lb_bsp::mech::Mechanisms;
use lb_core::Vec3;
use lb_core::rng::Pcg32;
use lb_kin::boxworld::BoxWorld;
use lb_nav::follow::TrickKind;
use lb_nav::graph::{GraphStats, LinkFlags, NO_SPEC};
use lb_nav::plan::{plain, plan};
use lb_nav::spec::{Action, Anchor, Cost, Needs, Stance, TraversalSpec};
use lb_nav::{LinkKind, NavGraph, NavLink, NavNode, NodeFlags, NodeId};
use lb_nav_api::NavStatus;
use lb_navgen::{GenOptions, generate};
use lb_testkit::course::{Course, CourseBot, Game, SimWorld};

const HARD: [f32; 2] = [1.5, 1.7];
const EXPERT: [f32; 2] = [1.7, 2.0];

#[derive(Default)]
struct Builder {
    nodes: Vec<NavNode>,
    out: Vec<Vec<NavLink>>,
    specs: Vec<TraversalSpec>,
}

impl Builder {
    fn node(&mut self, x: f32, y: f32) -> NodeId {
        self.nodes.push(NavNode {
            origin: Vec3::new(x, y, 36.0),
            flags: NodeFlags::empty(),
            radius: 24.0,
            support: 0,
            first_link: 0,
            link_count: 0,
        });
        self.out.push(Vec::new());
        (self.nodes.len() - 1) as NodeId
    }

    fn link(&mut self, a: NodeId, b: NodeId, kind: LinkKind, action: Option<Action>) {
        let (na, nb) = (self.nodes[a as usize], self.nodes[b as usize]);
        let length = na.origin.distance(nb.origin);
        let spec = action.map(|action| {
            self.specs.push(TraversalSpec {
                entry: Anchor {
                    origin: na.origin,
                    radius: 24.0,
                    stance: Stance::Stand,
                },
                exit: Anchor {
                    origin: nb.origin,
                    radius: 32.0,
                    stance: Stance::Stand,
                },
                action,
                needs: Needs::default(),
                deadline: 20.0,
                cost: Cost {
                    time: length / 300.0,
                    wait: 0.0,
                    damage: 0.0,
                },
            });
            (self.specs.len() - 1) as u32
        });
        self.out[a as usize].push(NavLink {
            to: b,
            kind,
            length,
            flags: LinkFlags::VALID,
            cost: length / 300.0,
            spec: spec.unwrap_or(NO_SPEC),
        });
    }

    /// Walking links both ways along `points`, 128 units apart; the nodes.
    fn walkway(&mut self, points: &[(f32, f32)]) -> Vec<NodeId> {
        let mut ids: Vec<NodeId> = Vec::new();
        for w in points.windows(2) {
            let (a, b) = (Vec3::new(w[0].0, w[0].1, 0.0), Vec3::new(w[1].0, w[1].1, 0.0));
            let n = ((b - a).length() / 128.0).round().max(1.0) as usize;
            for i in usize::from(!ids.is_empty())..=n {
                let p = a.lerp(b, i as f32 / n as f32);
                let id = self.node(p.x, p.y);
                if let Some(&last) = ids.last() {
                    self.link(last, id, LinkKind::Walk, None);
                    self.link(id, last, LinkKind::Walk, None);
                }
                ids.push(id);
            }
        }
        ids
    }

    fn build(self) -> NavGraph {
        NavGraph::from_parts(self.nodes, self.out, self.specs, "bhop", GraphStats::default())
    }
}

/// A walled corridor 256 wide along the x axis, `len` long, with a ceiling at `ceiling` above the floor if given.
fn corridor(len: f32, ceiling: Option<f32>) -> BoxWorld {
    let mut w = BoxWorld::new();
    w.solid(Vec3::new(-256.0, -256.0, -16.0), Vec3::new(len + 256.0, 256.0, 0.0));
    w.solid(Vec3::new(-256.0, 128.0, 0.0), Vec3::new(len + 256.0, 160.0, 256.0));
    w.solid(Vec3::new(-256.0, -160.0, 0.0), Vec3::new(len + 256.0, -128.0, 256.0));
    if let Some(z) = ceiling {
        w.solid(Vec3::new(-256.0, -256.0, z), Vec3::new(len + 256.0, 256.0, z + 32.0));
    }
    w
}

/// How a run went, looked at every frame.
#[derive(Debug, Default)]
struct Track {
    arrived: bool,
    seconds: f64,
    peak: f32,
    /// Horizontal speed on the last frame on the ground and on the first in the air, for every takeoff.
    takeoffs: Vec<(f32, f32)>,
    landed: u32,
    missed: u32,
    failures: u32,
    /// Takeoffs that lost speed: what navigation was doing the frame before, the speeds.
    lossy: Vec<(&'static str, f32, f32)>,
    /// The links that failed, by kind and cause.
    failed: Vec<String>,
}

impl Track {
    fn lossless(&self) -> bool {
        self.takeoffs.iter().all(|(before, after)| *after >= before * 0.98)
    }
}

/// Drives a bot from `from` to `to` (`bhop`: its limits, none = walking) for 30 s at most, 1000 frames a second.
fn drive<W: SimWorld>(c: &mut Course<W>, from: Vec3, to: Vec3, bhop: Option<[f32; 2]>) -> Track {
    let mut bot = CourseBot::new(from, 100.0);
    bot.tricks.bhop = bhop;
    c.place(&mut bot);
    let start = c.now;
    let failures = bot.nav.failures_total;
    let mut t = Track::default();
    let mut seen = failures;
    while c.now - start < 30.0 {
        let before = bot.player;
        let phase = bot.nav.phase();
        let status = c.frame(&mut bot, to, 1.0);
        let after = bot.player;
        let speed = after.velocity.truncate().length();
        t.peak = t.peak.max(speed);
        // Hop takeoffs: the follower hops after the frame (other takeoffs are traversals, falls off steps).
        if before.on_ground() && !after.on_ground() && bot.nav.phase() == "walk:hop" {
            let was = before.velocity.truncate().length();
            t.takeoffs.push((was, speed));
            if speed < was * 0.98 {
                t.lossy.push((phase, was, speed));
            }
        }
        if bot.nav.failures_total > seen {
            seen = bot.nav.failures_total;
            if let Some(f) = bot.nav.last_failure {
                let kind = c.graph.find_link(f.from, f.to).map_or("?", |l| l.kind.as_str());
                t.failed.push(format!("{kind} {:?} in {phase}", f.reason));
            }
        }
        if status == NavStatus::Arrived {
            t.arrived = true;
            break;
        }
    }
    t.seconds = c.now - start;
    let i = TrickKind::Hop as usize;
    t.landed = bot.nav.tricks.landed[i];
    t.missed = bot.nav.tricks.missed[i];
    t.failures = bot.nav.failures_total - failures;
    t
}

fn straight(bhop: Option<[f32; 2]>, cap: bool) -> Track {
    let mut g = Builder::default();
    let ids = g.walkway(&[(0.0, 0.0), (3072.0, 0.0)]);
    let graph = g.build();
    let (from, to) = (graph.node(ids[0]).origin, graph.node(*ids.last().unwrap()).origin);
    let mut c = Course::new(corridor(3072.0, None), Game::default(), graph);
    c.phys.bunnyhop_cap = cap;
    drive(&mut c, from, to, bhop)
}

#[test]
fn hops_cross_a_straight_corridor_faster_than_a_run() {
    let walk = straight(None, true);
    let hard = straight(Some(HARD), true);
    let expert = straight(Some(EXPERT), true);
    let free = straight(Some(EXPERT), false);
    for (name, t) in [
        ("walk", &walk),
        ("hard", &hard),
        ("expert", &expert),
        ("uncapped", &free),
    ] {
        eprintln!(
            "{name}: {:.2} s, peak {:.0}, {} takeoffs, {} landed, {} missed, {} failures",
            t.seconds,
            t.peak,
            t.takeoffs.len(),
            t.landed,
            t.missed,
            t.failures
        );
    }
    assert!(walk.arrived && walk.takeoffs.is_empty(), "{walk:?}");
    for t in [&hard, &expert, &free] {
        assert!(t.arrived && t.failures == 0 && t.missed == 0, "{t:?}");
        assert!(t.landed >= 6, "{t:?}");
        assert!(
            t.lossless(),
            "every takeoff on the first command back, none cropped: {:?}",
            t.takeoffs
        );
    }
    let crop = 1.7 * 270.0;
    assert!(expert.peak > 0.95 * 0.97 * crop && expert.peak < crop, "{expert:?}");
    assert!(
        hard.peak > 0.95 * 1.5 * 270.0 && hard.peak < 1.5 * 270.0 + 5.0,
        "{hard:?}"
    );
    assert!(
        free.peak > 0.95 * 2.0 * 270.0,
        "uncapped the expert goes past the crop: {free:?}"
    );
    assert!(
        expert.seconds < 0.8 * walk.seconds,
        "{} vs {}",
        expert.seconds,
        walk.seconds
    );
    assert!(
        hard.seconds < 0.85 * walk.seconds,
        "{} vs {}",
        hard.seconds,
        walk.seconds
    );
    assert!(free.seconds < expert.seconds, "{} vs {}", free.seconds, expert.seconds);
}

#[test]
fn hops_stop_for_a_corner_and_start_again_past_it() {
    let mut w = BoxWorld::new();
    w.solid(Vec3::new(-256.0, -256.0, -16.0), Vec3::new(2048.0, 2048.0, 0.0));
    w.solid(Vec3::new(-256.0, -160.0, 0.0), Vec3::new(1696.0, -128.0, 256.0));
    w.solid(Vec3::new(-256.0, 128.0, 0.0), Vec3::new(1408.0, 160.0, 256.0));
    w.solid(Vec3::new(1664.0, -160.0, 0.0), Vec3::new(1696.0, 1792.0, 256.0));
    w.solid(Vec3::new(1376.0, 128.0, 0.0), Vec3::new(1408.0, 1792.0, 256.0));
    let mut g = Builder::default();
    let ids = g.walkway(&[(0.0, 0.0), (1536.0, 0.0), (1536.0, 1536.0)]);
    let graph = g.build();
    let (from, to) = (graph.node(ids[0]).origin, graph.node(*ids.last().unwrap()).origin);
    let mut c = Course::new(w.clone(), Game::default(), graph.clone());
    let walk = drive(&mut c, from, to, None);
    let mut c = Course::new(w, Game::default(), graph);
    let expert = drive(&mut c, from, to, Some(EXPERT));
    eprintln!(
        "L: walk {:.2} s, expert {:.2} s, {} landed, {} missed",
        walk.seconds, expert.seconds, expert.landed, expert.missed
    );
    assert!(
        walk.arrived && expert.arrived && expert.failures == 0 && expert.missed == 0,
        "{expert:?}"
    );
    assert!(expert.lossless(), "{:?}", expert.takeoffs);
    assert!(expert.landed >= 4, "{expert:?}");
    assert!(
        expert.seconds < 0.88 * walk.seconds,
        "{} vs {}",
        expert.seconds,
        walk.seconds
    );
}

#[test]
fn no_hops_under_a_low_ceiling() {
    let mut g = Builder::default();
    let ids = g.walkway(&[(0.0, 0.0), (2048.0, 0.0)]);
    let graph = g.build();
    let (from, to) = (graph.node(ids[0]).origin, graph.node(*ids.last().unwrap()).origin);
    let mut c = Course::new(corridor(2048.0, Some(100.0)), Game::default(), graph);
    let t = drive(&mut c, from, to, Some(EXPERT));
    assert!(t.arrived && t.failures == 0, "{t:?}");
    assert!(t.takeoffs.is_empty(), "{t:?}");
}

#[test]
fn hops_end_in_time_for_a_jump_link() {
    // The corridor floor ends at 1900, a gap of 120, and the floor goes on from 2020.
    let mut w = BoxWorld::new();
    w.solid(Vec3::new(-256.0, -256.0, -16.0), Vec3::new(1900.0, 256.0, 0.0));
    w.solid(Vec3::new(2020.0, -256.0, -16.0), Vec3::new(2800.0, 256.0, 0.0));
    w.solid(Vec3::new(-2000.0, -2000.0, -400.0), Vec3::new(4000.0, 2000.0, -384.0));
    let mut g = Builder::default();
    let near = g.walkway(&[(0.0, 0.0), (1872.0, 0.0)]);
    let far = g.walkway(&[(2048.0, 0.0), (2560.0, 0.0)]);
    g.link(
        *near.last().unwrap(),
        far[0],
        LinkKind::Jump,
        Some(Action::Jump {
            speed: 270.0,
            duck: false,
            robustness: 1.0,
        }),
    );
    let graph = g.build();
    let (from, to) = (graph.node(near[0]).origin, graph.node(*far.last().unwrap()).origin);
    let mut c = Course::new(w, Game::default(), graph);
    let t = drive(&mut c, from, to, Some(EXPERT));
    eprintln!(
        "jump link: {:.2} s, {} landed, {} missed",
        t.seconds, t.landed, t.missed
    );
    assert!(t.arrived && t.failures == 0 && t.missed == 0, "{t:?}");
    assert!(t.landed >= 2, "{t:?}");
}

fn load(map: &str) -> Option<(BspWorld, Mechanisms, NavGraph)> {
    let maps = lb_bsp::test_maps_dir()?;
    let bsp = std::fs::read(maps.join(format!("{map}.bsp"))).ok()?;
    let mut world = BspWorld::load(&bsp).unwrap();
    let mech = Mechanisms::from_world(&world);
    let generated = generate(&mut world, &mech, &GenOptions::default(), map);
    Some((world, mech, generated.graph))
}

/// Routes between random nodes of the maps, walked and hopped (an expert on a server that crops): how many get
/// there, in what time, and how the hops go.
#[test]
#[ignore]
fn routes_on_the_maps_hopped_and_walked() {
    for map in ["crossfire", "dm_snow", "stalkyard", "datacore", "bounce"] {
        let Some((mut world, mech, graph)) = load(map) else {
            eprintln!("{map}: no map");
            continue;
        };
        let game = Game::from_map(&mut world, &mech);
        let mut c = Course::new(world, game, graph.clone());
        let mut rng = Pcg32::new(11, 11);
        let n = graph.len() as i32;
        let mut routes = 0;
        let mut tries = 0;
        let (mut walked, mut hopped) = (Track::default(), Track::default());
        let (mut both, mut walk_time, mut hop_time) = (0u32, 0.0f64, 0.0f64);
        while routes < 40 && tries < 400 {
            tries += 1;
            let (a, b) = (rng.range_i32(0, n - 1) as NodeId, rng.range_i32(0, n - 1) as NodeId);
            let (from, to) = (graph.node(a).origin, graph.node(b).origin);
            if from.distance(to) < 1200.0 || plan(&graph, a, b, &plain).is_none() {
                continue;
            }
            routes += 1;
            c.settle(10.0);
            let w = drive(&mut c, from, to, None);
            c.settle(10.0);
            let h = drive(&mut c, from, to, Some(EXPERT));
            if w.arrived && h.arrived {
                both += 1;
                walk_time += w.seconds;
                hop_time += h.seconds;
            }
            if std::env::var("LB_LOG").is_ok() && (h.missed > 0 || h.failures > w.failures || !h.lossless()) {
                eprintln!(
                    "  {a} -> {b}: walked {:.1} s, hopped {:.1} s, {} landed, {} missed, failures {} / {}",
                    w.seconds, h.seconds, h.landed, h.missed, w.failures, h.failures
                );
            }
            for (sum, t) in [(&mut walked, w), (&mut hopped, h)] {
                sum.arrived |= t.arrived;
                sum.landed += t.landed;
                sum.missed += t.missed;
                sum.failures += t.failures;
                sum.peak = sum.peak.max(t.peak);
                sum.takeoffs.extend(t.takeoffs);
                sum.lossy.extend(t.lossy);
                sum.failed.extend(t.failed);
            }
        }
        let lossy = hopped.takeoffs.iter().filter(|(b, a)| *a < b * 0.98).count();
        eprintln!(
            "{map}: {routes} routes, {both} done both ways: walked {:.1} s, hopped {:.1} s on average ({:.0}%); \
             hops {} landed, {} missed, {lossy} takeoffs lost speed; link failures walked {}, hopped {}; peak {:.0}",
            walk_time / f64::from(both.max(1)),
            hop_time / f64::from(both.max(1)),
            100.0 * hop_time / walk_time.max(1e-3),
            hopped.landed,
            hopped.missed,
            walked.failures,
            hopped.failures,
            hopped.peak
        );
        if std::env::var("LB_LOG").is_ok() {
            let tally = |items: Vec<String>| {
                let mut counts: Vec<(String, usize)> = Vec::new();
                for i in items {
                    match counts.iter_mut().find(|(k, _)| *k == i) {
                        Some(c) => c.1 += 1,
                        None => counts.push((i, 1)),
                    }
                }
                counts.sort_by(|a, b| b.1.cmp(&a.1));
                counts
                    .iter()
                    .map(|(k, n)| format!("{k} ×{n}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let lossy = hopped
                .lossy
                .iter()
                .map(|(phase, b, a)| format!("{phase} {:.0}%", 100.0 * (1.0 - a / b)))
                .collect::<Vec<_>>();
            eprintln!("  lossy takeoffs: {}", lossy.join(", "));
            eprintln!("  failures walked: {}", tally(walked.failed));
            eprintln!("  failures hopped: {}", tally(hopped.failed));
        }
    }
}
