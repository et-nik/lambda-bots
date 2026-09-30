//! Getting a bot to a spot the way a test asks (`lb_nav::reach`): along the graph and straight on, or by a trick
//! found on the spot onto a place the graph has no node on.

use lb_core::Vec3;
use lb_kin::boxworld::BoxWorld;
use lb_nav::graph::{GraphStats, LinkFlags, NO_SPEC};
use lb_nav::reach::{Allowed, Outcome as End, Reach, Trick};
use lb_nav::{LinkKind, NavGraph, NavLink, NavNode, NodeFlags, NodeId};
use lb_testkit::course::{Course, CourseBot, Game};

/// Nodes walked between both ways, in a row.
fn row(points: &[(f32, f32, f32)]) -> NavGraph {
    let nodes: Vec<NavNode> = points
        .iter()
        .map(|&(x, y, z)| NavNode {
            origin: Vec3::new(x, y, z),
            flags: NodeFlags::empty(),
            radius: 24.0,
            support: 0,
            first_link: 0,
            link_count: 0,
        })
        .collect();
    let mut out = vec![Vec::new(); nodes.len()];
    for i in 1..nodes.len() {
        for (a, b) in [(i - 1, i), (i, i - 1)] {
            let length = nodes[a].origin.distance(nodes[b].origin);
            out[a].push(NavLink {
                to: b as NodeId,
                kind: LinkKind::Walk,
                length,
                flags: LinkFlags::VALID,
                cost: length / 300.0,
                spec: NO_SPEC,
            });
        }
    }
    NavGraph::from_parts(nodes, out, Vec::new(), "test", GraphStats::default())
}

struct Run {
    end: End,
    trick: Option<Trick>,
    push: f32,
    report: String,
}

fn run(world: BoxWorld, graph: NavGraph, from: Vec3, spot: Vec3, setup: impl FnOnce(&mut CourseBot)) -> Run {
    let mut search = world.clone();
    let mut c = Course::new(world, Game::default(), graph);
    let mut bot = CourseBot::new(from, 100.0);
    setup(&mut bot);
    c.place(&mut bot);
    let mut reach = Reach::new(spot, 32.0, Allowed::default(), 30.0, c.now);
    let o = c.attempt(&mut bot, &mut reach, &mut search, 31.0, 100.0);
    let r = reach.report();
    let events: Vec<String> = r.events.iter().map(|e| format!("{:.2} {}", e.t, e.what)).collect();
    let Some(end) = reach.outcome().cloned() else {
        panic!(
            "not over: {}\n  {}\n  {:?}",
            reach.phase(&bot.nav),
            events.join("\n  "),
            o.phases
        );
    };
    Run {
        end,
        trick: r.trick.map(|t| t.trick),
        push: r.trick.map_or(0.0, |t| t.push),
        report: format!("{}\n  end {:?}, {:.0} u off", events.join("\n  "), r.end, r.off),
    }
}

fn gauss(bot: &mut CourseBot) {
    bot.tricks.gauss_boost = true;
    bot.tricks.boost_now = true;
    bot.tricks.gauss_damage = 200.0;
}

/// A floor with a ledge 200 units up from 400 to 900 along: nodes on the floor only.
fn ledge() -> (BoxWorld, NavGraph) {
    let mut w = BoxWorld::new();
    w.floor(0.0, 4096.0);
    w.solid(Vec3::new(400.0, -256.0, 0.0), Vec3::new(900.0, 256.0, 200.0));
    let g = row(&[
        (-400.0, 0.0, 36.0),
        (-200.0, 0.0, 36.0),
        (0.0, 0.0, 36.0),
        (200.0, 0.0, 36.0),
    ]);
    (w, g)
}

#[test]
fn a_ledge_without_nodes_is_got_onto_by_a_gauss_boost() {
    let (w, g) = ledge();
    let spot = Vec3::new(620.0, 0.0, 236.0);
    let r = run(w, g, Vec3::new(-400.0, 0.0, 36.0), spot, gauss);
    assert_eq!(r.end, End::Arrived, "{}", r.report);
    assert_eq!(r.trick, Some(Trick::GaussBoost), "{}", r.report);
    assert!(r.push < 1000.0, "charged for the push it takes: {}", r.report);
}

#[test]
fn without_the_gauss_there_is_no_way_up_and_the_report_says_why() {
    let (w, g) = ledge();
    let spot = Vec3::new(620.0, 0.0, 236.0);
    let r = run(w, g, Vec3::new(-400.0, 0.0, 36.0), spot, |_| {});
    let End::NoWay { why } = &r.end else {
        panic!("{:?}\n{}", r.end, r.report);
    };
    assert!(
        why.contains("no gauss boost") && why.contains("nearest node 3"),
        "{why}"
    );
}

#[test]
fn a_spot_across_a_gap_is_long_jumped_to_with_the_module() {
    // Floors at x < 0 and x > 320, a pit between; nodes on the near side only.
    let mut w = BoxWorld::new();
    w.solid(Vec3::new(-1024.0, -512.0, -512.0), Vec3::new(0.0, 512.0, 0.0));
    w.solid(Vec3::new(320.0, -512.0, -512.0), Vec3::new(1024.0, 512.0, 0.0));
    let g = row(&[(-600.0, 0.0, 36.0), (-300.0, 0.0, 36.0), (-24.0, 0.0, 36.0)]);
    let spot = Vec3::new(420.0, 0.0, 36.0);
    let r = run(w.clone(), g.clone(), Vec3::new(-600.0, 0.0, 36.0), spot, |b| {
        b.tricks.longjump = true;
    });
    assert_eq!(r.end, End::Arrived, "{}", r.report);
    assert_eq!(r.trick, Some(Trick::LongJump), "{}", r.report);
    let r = run(w, g, Vec3::new(-600.0, 0.0, 36.0), spot, |_| {});
    assert!(
        matches!(&r.end, End::NoWay { why } if why.contains("no long jump module")),
        "{:?}\n{}",
        r.end,
        r.report
    );
}

#[test]
fn a_spot_off_the_graph_on_its_floor_is_walked_to_from_the_nearest_node() {
    let mut w = BoxWorld::new();
    w.floor(0.0, 4096.0);
    // A wall between the bot and the spot: the way goes round it along the nodes.
    w.solid(Vec3::new(300.0, -600.0, 0.0), Vec3::new(340.0, 200.0, 300.0));
    let g = row(&[
        (0.0, 0.0, 36.0),
        (0.0, 400.0, 36.0),
        (600.0, 400.0, 36.0),
        (600.0, 100.0, 36.0),
    ]);
    let spot = Vec3::new(700.0, -200.0, 36.0);
    let r = run(w, g, Vec3::new(0.0, 0.0, 36.0), spot, |_| {});
    assert_eq!(r.end, End::Arrived, "{}", r.report);
    assert_eq!(r.trick, None, "{}", r.report);
    assert!(r.report.contains("then 316 u straight"), "{}", r.report);
}

#[test]
fn a_ledge_under_a_low_ceiling_takes_a_partial_charge() {
    let mut w = BoxWorld::new();
    w.floor(0.0, 4096.0);
    w.solid(Vec3::new(-512.0, -512.0, 360.0), Vec3::new(1400.0, 512.0, 420.0));
    w.solid(Vec3::new(300.0, -256.0, 0.0), Vec3::new(1400.0, 256.0, 120.0));
    let g = row(&[(-300.0, 0.0, 36.0), (0.0, 0.0, 36.0)]);
    let spot = Vec3::new(420.0, 0.0, 156.0);
    let r = run(w, g, Vec3::new(-300.0, 0.0, 36.0), spot, gauss);
    assert_eq!(r.end, End::Arrived, "{}", r.report);
    assert_eq!(r.trick, Some(Trick::GaussBoost), "{}", r.report);
    assert!(r.push < 800.0, "{}", r.report);
}
