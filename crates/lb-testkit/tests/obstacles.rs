//! The obstacle set: a small box world per traversal the bot must get through. Some worlds also have a way that
//! fails (a use door that never opens, a jump too far, a walled-up passage); there the bot must report the right
//! cause and take another way.

use lb_bsp::mech::{Mover, MoverKind, TriggerKind};
use lb_core::Vec3;
use lb_kin::boxworld::BoxWorld;
use lb_nav::graph::{GraphStats, LinkFlags, NO_SPEC};
use lb_nav::known::FailReason;
use lb_nav::spec::{Action, Anchor, Cost, Interaction, MechRef, Needs, Stance, TraversalSpec};
use lb_nav::{LinkKind, NavGraph, NavLink, NavNode, NodeFlags, NodeId};
use lb_testkit::course::{Course, CourseBot, Game, Outcome};
use lb_worldq::contents;

const DOOR: u16 = 10;
const BUTTON: u16 = 11;
const PLAT: u16 = 20;
const TELEPORT: u16 = 30;
const GLASS: u16 = 40;

#[derive(Default)]
struct Builder {
    nodes: Vec<NavNode>,
    out: Vec<Vec<NavLink>>,
    specs: Vec<TraversalSpec>,
}

impl Builder {
    fn node(&mut self, x: f32, y: f32, z: f32) -> NodeId {
        self.nodes.push(NavNode {
            origin: Vec3::new(x, y, z),
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
                    wait: 1.0,
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
            cost: length / 300.0 + if spec.is_some() { 1.0 } else { 0.0 },
            spec: spec.unwrap_or(NO_SPEC),
        });
    }

    fn walk(&mut self, a: NodeId, b: NodeId) {
        self.link(a, b, LinkKind::Walk, None);
    }

    fn build(self) -> NavGraph {
        NavGraph::from_parts(self.nodes, self.out, self.specs, "obstacles", GraphStats::default())
    }
}

fn mover(model: u16, kind: MoverKind, active: Vec3, speed: f32, wait: f32) -> Mover {
    Mover {
        entity: 0,
        model: model as usize,
        kind,
        classname: "func_door".into(),
        targetname: None,
        target: None,
        master: None,
        spawnflags: 0,
        movedir: active.normalize_or_zero(),
        rest: Vec3::ZERO,
        active,
        speed,
        wait,
        dmg: 0.0,
        health: 0.0,
        touch: false,
        usable: false,
    }
}

fn door_ref(active: Vec3) -> MechRef {
    MechRef {
        model: DOOR,
        rest: Vec3::ZERO,
        active,
        travel: active.length() / 200.0,
        wait: 2.0,
    }
}

/// A floor, a wall across x = 200..216 with a 96-wide doorway blocked by a door that slides up, and a way around
/// through a far opening (for when the door fails).
fn door_world() -> BoxWorld {
    let mut w = BoxWorld::new();
    w.floor(0.0, 2048.0);
    w.solid(Vec3::new(200.0, -600.0, 0.0), Vec3::new(216.0, -48.0, 200.0));
    w.solid(Vec3::new(200.0, 48.0, 0.0), Vec3::new(216.0, 500.0, 200.0));
    w.entity(
        Vec3::new(200.0, -48.0, 0.0),
        Vec3::new(216.0, 48.0, 112.0),
        u32::from(DOOR),
    );
    w.solid(Vec3::new(200.0, -48.0, 112.0), Vec3::new(216.0, 48.0, 200.0));
    w
}

/// Nodes: 0 before the door, 1 after it, 2..4 the long way around through y = 550.
fn door_graph(open: Option<Interaction>) -> (NavGraph, NodeId, NodeId) {
    let mut g = Builder::default();
    let a = g.node(100.0, 0.0, 36.0);
    let b = g.node(320.0, 0.0, 36.0);
    let c = g.node(100.0, 550.0, 36.0);
    let d = g.node(320.0, 550.0, 36.0);
    if let Some(open) = open {
        g.link(
            a,
            b,
            LinkKind::Door,
            Some(Action::Door {
                door: door_ref(Vec3::new(0.0, 0.0, 108.0)),
                open,
                via: None,
            }),
        );
    }
    g.walk(a, c);
    g.walk(c, d);
    g.walk(d, b);
    (g.build(), a, b)
}

fn run(course: &mut Course<BoxWorld>, from: Vec3, to: Vec3, seconds: f64) -> Outcome {
    let mut bot = CourseBot::new(from, 100.0);
    course.place(&mut bot);
    course.run(&mut bot, to, seconds, 100.0, None)
}

fn phases(o: &Outcome) -> Vec<&'static str> {
    o.phases.iter().map(|(_, p)| *p).collect()
}

fn describe(o: &Outcome) -> String {
    format!(
        "arrived {} in {:.1}s at {:?}, failures {:?}, phases {:?}, log {:?}",
        o.arrived,
        o.seconds,
        o.end,
        o.failures.iter().map(|f| (f.from, f.to, f.reason)).collect::<Vec<_>>(),
        o.phases.iter().map(|(t, p)| format!("{t:.1}:{p}")).collect::<Vec<_>>(),
        o.log
    )
}

#[test]
fn touch_door_opens_when_walked_into() {
    let (g, a, b) = door_graph(Some(Interaction::Touch {
        model: DOOR,
        spot: Vec3::new(100.0, 0.0, 36.0),
    }));
    let mut game = Game::default();
    game.add_mover(Mover {
        touch: true,
        ..mover(DOOR, MoverKind::Door, Vec3::new(0.0, 0.0, 108.0), 200.0, 2.0)
    });
    let mut c = Course::new(door_world(), game, g);
    let (from, to) = (c.graph.node(a).origin, c.graph.node(b).origin);
    let o = run(&mut c, from, to, 15.0);
    assert!(o.arrived && o.failures.is_empty(), "{}", describe(&o));
    assert!(o.seconds < 4.0, "through the door, not around: {}", describe(&o));
}

#[test]
fn use_door_is_opened_with_the_use_key() {
    let aim = Vec3::new(208.0, 0.0, 56.0);
    let (g, a, b) = door_graph(Some(Interaction::Use {
        model: DOOR,
        spot: Vec3::new(160.0, 0.0, 36.0),
        aim,
    }));
    let mut game = Game::default();
    game.add_mover(Mover {
        usable: true,
        ..mover(DOOR, MoverKind::Door, Vec3::new(0.0, 0.0, 108.0), 200.0, 2.0)
    });
    let mut c = Course::new(door_world(), game, g);
    let (from, to) = (c.graph.node(a).origin, c.graph.node(b).origin);
    let o = run(&mut c, from, to, 15.0);
    assert!(o.arrived && o.failures.is_empty(), "{}", describe(&o));
    assert!(
        o.log.iter().any(|l| l.contains("*10 func_door set off")),
        "{}",
        describe(&o)
    );
    assert!(o.seconds < 5.0, "{}", describe(&o));
}

#[test]
fn remote_button_opens_the_door() {
    // A button on the wall of the first room, 120 units from the door, fires the door by name.
    let mut w = door_world();
    w.entity(
        Vec3::new(96.0, 180.0, 40.0),
        Vec3::new(104.0, 188.0, 56.0),
        u32::from(BUTTON),
    );
    let button_spot = Vec3::new(100.0, 140.0, 36.0);
    let (g, a, b) = door_graph(Some(Interaction::Use {
        model: BUTTON,
        spot: button_spot,
        aim: Vec3::new(100.0, 184.0, 48.0),
    }));
    let mut game = Game::default();
    game.add_mover(Mover {
        targetname: Some("gate".into()),
        wait: 4.0,
        ..mover(DOOR, MoverKind::Door, Vec3::new(0.0, 0.0, 108.0), 200.0, 4.0)
    });
    game.add_mover(Mover {
        usable: true,
        target: Some("gate".into()),
        classname: "func_button".into(),
        ..mover(BUTTON, MoverKind::Button, Vec3::ZERO, 40.0, 1.0)
    });
    let mut c = Course::new(w, game, g);
    let (from, to) = (c.graph.node(a).origin, c.graph.node(b).origin);
    let o = run(&mut c, from, to, 20.0);
    assert!(o.arrived && o.failures.is_empty(), "{}", describe(&o));
    let ph = phases(&o);
    assert!(
        ph.contains(&"door:go-activate") && ph.contains(&"door:wait"),
        "{}",
        describe(&o)
    );
    assert!(o.seconds < 8.0, "{}", describe(&o));
}

#[test]
fn a_door_that_never_opens_is_reported_and_walked_around() {
    // The graph says use opens it; the game's door ignores the use key.
    let (g, a, b) = door_graph(Some(Interaction::Use {
        model: DOOR,
        spot: Vec3::new(160.0, 0.0, 36.0),
        aim: Vec3::new(208.0, 0.0, 56.0),
    }));
    let mut game = Game::default();
    game.add_mover(mover(DOOR, MoverKind::Door, Vec3::new(0.0, 0.0, 108.0), 200.0, 2.0));
    let mut c = Course::new(door_world(), game, g);
    let (from, to) = (c.graph.node(a).origin, c.graph.node(b).origin);
    let mut bot = CourseBot::new(from, 100.0);
    c.place(&mut bot);
    let o = c.run(&mut bot, to, 40.0, 100.0, None);
    assert!(o.arrived, "{}", describe(&o));
    assert!(
        bot.nav
            .known
            .active(c.now)
            .any(|((f, t), fail)| (f, t) == (a, b) && fail.reason == FailReason::WaitingForInteraction),
        "the door link is blocked for a while: {}",
        describe(&o)
    );
}

#[test]
fn a_plat_rides_up_when_stood_on() {
    let mut w = BoxWorld::new();
    // Floor around a shaft (x 0..128, y -64..64) with a platform whose top rests at floor level, and a ledge 128 up.
    w.solid(Vec3::new(-2048.0, -2048.0, -16.0), Vec3::new(0.0, 2048.0, 0.0));
    w.solid(Vec3::new(0.0, -2048.0, -16.0), Vec3::new(128.0, -64.0, 0.0));
    w.solid(Vec3::new(0.0, 64.0, -16.0), Vec3::new(128.0, 2048.0, 0.0));
    w.solid(Vec3::new(128.0, -2048.0, -16.0), Vec3::new(2048.0, 2048.0, 0.0));
    w.solid(Vec3::new(-2048.0, -2048.0, -400.0), Vec3::new(2048.0, 2048.0, -384.0));
    w.entity(
        Vec3::new(0.0, -64.0, -128.0),
        Vec3::new(128.0, 64.0, 0.0),
        u32::from(PLAT),
    );
    w.solid(Vec3::new(128.0, -256.0, 0.0), Vec3::new(600.0, 256.0, 128.0));
    let mut g = Builder::default();
    let before = g.node(-100.0, 0.0, 36.0);
    let on = g.node(64.0, 0.0, 36.0);
    let top = g.node(200.0, 0.0, 164.0);
    g.nodes[on as usize].flags |= NodeFlags::ON_MOVER;
    g.nodes[on as usize].support = PLAT;
    g.walk(before, on);
    let platform = MechRef {
        model: PLAT,
        rest: Vec3::ZERO,
        active: Vec3::new(0.0, 0.0, 128.0),
        travel: 128.0 / 150.0,
        wait: 3.0,
    };
    g.link(
        on,
        top,
        LinkKind::Lift,
        Some(Action::Lift {
            platform,
            start: Interaction::Touch {
                model: PLAT,
                spot: Vec3::new(64.0, 0.0, 36.0),
            },
        }),
    );
    let mut game = Game::default();
    game.add_mover(Mover {
        touch: true,
        classname: "func_plat".into(),
        ..mover(PLAT, MoverKind::Plat, Vec3::new(0.0, 0.0, 128.0), 150.0, 3.0)
    });
    let mut c = Course::new(w, game, g.build());
    let o = run(&mut c, Vec3::new(-100.0, 0.0, 36.0), Vec3::new(200.0, 0.0, 164.0), 15.0);
    assert!(o.arrived && o.failures.is_empty(), "{}", describe(&o));
    assert!(
        o.log.iter().any(|l| l.contains("*20 func_plat set off")),
        "{}",
        describe(&o)
    );
}

#[test]
fn teleports_are_walked_into() {
    let mut w = BoxWorld::new();
    w.floor(0.0, 4096.0);
    w.trigger(
        Vec3::new(380.0, -32.0, 0.0),
        Vec3::new(420.0, 32.0, 72.0),
        u32::from(TELEPORT),
    );
    let dest = Vec3::new(2000.0, 0.0, 0.0);
    let mut g = Builder::default();
    let a = g.node(300.0, 0.0, 36.0);
    let b = g.node(2000.0, 0.0, 37.0);
    let far = g.node(2200.0, 0.0, 36.0);
    g.link(
        a,
        b,
        LinkKind::Teleport,
        Some(Action::Teleport {
            trigger: TELEPORT,
            touch: Vec3::new(400.0, 0.0, 36.0),
            dest: dest + Vec3::Z * 37.0,
        }),
    );
    g.walk(b, far);
    let mut game = Game::default();
    game.add_trigger(TELEPORT, TriggerKind::Teleport, None, Some((dest, 0.0)));
    let mut graph = g.build();
    graph.stats.by_kind[LinkKind::Teleport.index()] = 1;
    let mut c = Course::new(w, game, graph);
    let o = run(&mut c, Vec3::new(300.0, 0.0, 36.0), Vec3::new(2200.0, 0.0, 36.0), 10.0);
    assert!(o.arrived && o.failures.is_empty(), "{}", describe(&o));
    assert!(o.seconds < 3.0, "teleported, not walked: {}", describe(&o));
}

#[test]
fn breakables_are_shot_out_of_the_way() {
    let mut w = BoxWorld::new();
    w.floor(0.0, 2048.0);
    w.solid(Vec3::new(200.0, -600.0, 0.0), Vec3::new(216.0, -48.0, 200.0));
    w.solid(Vec3::new(200.0, 48.0, 0.0), Vec3::new(216.0, 600.0, 200.0));
    w.entity(
        Vec3::new(200.0, -48.0, 0.0),
        Vec3::new(216.0, 48.0, 200.0),
        u32::from(GLASS),
    );
    let mut g = Builder::default();
    let a = g.node(100.0, 0.0, 36.0);
    let b = g.node(320.0, 0.0, 36.0);
    g.link(
        a,
        b,
        LinkKind::Breakable,
        Some(Action::Breakable {
            model: GLASS,
            aim: Vec3::new(208.0, 0.0, 64.0),
            health: 30.0,
            crowbar: false,
        }),
    );
    let mut game = Game::default();
    game.add_breakable(GLASS, 30.0);
    let mut c = Course::new(w, game, g.build());
    let o = run(&mut c, Vec3::new(100.0, 0.0, 36.0), Vec3::new(320.0, 0.0, 36.0), 15.0);
    assert!(o.arrived && o.failures.is_empty(), "{}", describe(&o));
    assert!(o.log.iter().any(|l| l.contains("broken")), "{}", describe(&o));
}

#[test]
fn an_impossible_jump_fails_and_the_bot_goes_around() {
    // A 260-unit gap no jump clears, and a bridge further along.
    let mut w = BoxWorld::new();
    w.solid(Vec3::new(-600.0, -600.0, -16.0), Vec3::new(0.0, 600.0, 0.0));
    w.solid(Vec3::new(260.0, -600.0, -16.0), Vec3::new(900.0, 600.0, 0.0));
    w.solid(Vec3::new(0.0, 400.0, -16.0), Vec3::new(260.0, 600.0, 0.0));
    // The gap is a pit 128 deep with stairs up at its far end.
    w.solid(Vec3::new(-2000.0, -2000.0, -144.0), Vec3::new(2000.0, 2000.0, -128.0));
    for i in 1..8 {
        let (y, top) = (400.0 - 24.0 * (8 - i) as f32, -128.0 + 16.0 * i as f32);
        w.solid(Vec3::new(0.0, y, -128.0), Vec3::new(260.0, 400.0, top));
    }
    let mut g = Builder::default();
    let a = g.node(-20.0, 0.0, 36.0);
    let b = g.node(300.0, 0.0, 36.0);
    let c1 = g.node(-20.0, 500.0, 36.0);
    let c2 = g.node(300.0, 500.0, 36.0);
    g.link(
        a,
        b,
        LinkKind::Jump,
        Some(Action::Jump {
            speed: 270.0,
            duck: false,
            robustness: 1.0,
        }),
    );
    g.walk(a, c1);
    g.walk(c1, c2);
    g.walk(c2, b);
    // Falling into the gap needs a way back up.
    let pit = g.node(130.0, 0.0, -92.0);
    let stairs = g.node(130.0, 440.0, 36.0);
    g.walk(pit, stairs);
    g.walk(stairs, c2);
    let mut c = Course::new(w, Game::default(), g.build());
    let from = Vec3::new(-20.0, 0.0, 36.0);
    let to = Vec3::new(300.0, 0.0, 36.0);
    let mut bot = CourseBot::new(from, 100.0);
    c.place(&mut bot);
    let o = c.run(&mut bot, to, 20.0, 100.0, None);
    assert!(o.arrived, "{}", describe(&o));
    assert!(
        o.failures
            .iter()
            .any(|f| (f.from, f.to) == (a, b) && f.reason == FailReason::ControllerFailure),
        "the missed jump is reported: {}",
        describe(&o)
    );
}

#[test]
fn a_walled_up_passage_is_reported_as_geometry() {
    // The direct walk runs into a wall that is not in the graph; the long way around is open.
    let mut w = BoxWorld::new();
    w.floor(0.0, 2048.0);
    w.solid(Vec3::new(200.0, -300.0, 0.0), Vec3::new(216.0, 300.0, 200.0));
    let mut g = Builder::default();
    let a = g.node(100.0, 0.0, 36.0);
    let b = g.node(320.0, 0.0, 36.0);
    let c1 = g.node(100.0, 400.0, 36.0);
    let c2 = g.node(320.0, 400.0, 36.0);
    g.walk(a, b);
    g.walk(a, c1);
    g.walk(c1, c2);
    g.walk(c2, b);
    let mut c = Course::new(w, Game::default(), g.build());
    let mut bot = CourseBot::new(Vec3::new(100.0, 0.0, 36.0), 100.0);
    c.place(&mut bot);
    let o = c.run(&mut bot, Vec3::new(320.0, 0.0, 36.0), 30.0, 100.0, None);
    assert!(o.arrived, "{}", describe(&o));
    assert!(
        bot.nav
            .known
            .active(c.now)
            .any(|((f, t), fail)| (f, t) == (a, b) && fail.reason == FailReason::GeometryInvalid),
        "{}",
        describe(&o)
    );
}

#[test]
fn swimming_across_a_pool_and_climbing_out() {
    let mut w = BoxWorld::new();
    // Pool floor at -160, water up to 0, deck at 0 on both sides.
    w.solid(Vec3::new(-2000.0, -2000.0, -176.0), Vec3::new(2000.0, 2000.0, -160.0));
    w.solid(Vec3::new(-600.0, -600.0, -160.0), Vec3::new(0.0, 600.0, 0.0));
    w.solid(Vec3::new(400.0, -600.0, -160.0), Vec3::new(1000.0, 600.0, 0.0));
    w.volume(
        Vec3::new(0.0, -600.0, -160.0),
        Vec3::new(400.0, 600.0, -8.0),
        contents::WATER,
    );
    let mut g = Builder::default();
    let a = g.node(-60.0, 0.0, 36.0);
    let s1 = g.node(100.0, 0.0, -80.0);
    let s2 = g.node(340.0, 0.0, -40.0);
    let b = g.node(460.0, 0.0, 36.0);
    g.nodes[s1 as usize].flags |= NodeFlags::WATER;
    g.nodes[s2 as usize].flags |= NodeFlags::WATER;
    g.link(a, s1, LinkKind::Swim, Some(Action::Swim));
    g.link(s1, s2, LinkKind::Swim, Some(Action::Swim));
    g.link(s2, b, LinkKind::Swim, Some(Action::Swim));
    let mut c = Course::new(w, Game::default(), g.build());
    let o = run(&mut c, Vec3::new(-60.0, 0.0, 36.0), Vec3::new(460.0, 0.0, 36.0), 20.0);
    assert!(o.arrived && o.failures.is_empty(), "{}", describe(&o));
}

/// A 260-unit gap, a bridge 500 units along it.
fn gap_world() -> BoxWorld {
    let mut w = BoxWorld::new();
    w.solid(Vec3::new(-600.0, -600.0, -16.0), Vec3::new(0.0, 600.0, 0.0));
    w.solid(Vec3::new(260.0, -600.0, -16.0), Vec3::new(900.0, 600.0, 0.0));
    w.solid(Vec3::new(0.0, 400.0, -16.0), Vec3::new(260.0, 600.0, 0.0));
    w.solid(Vec3::new(-2000.0, -2000.0, -144.0), Vec3::new(2000.0, 2000.0, -128.0));
    w
}

#[test]
fn a_long_jump_crosses_a_gap_with_the_module_and_the_bot_goes_around_without() {
    let mut g = Builder::default();
    let a = g.node(-20.0, 0.0, 36.0);
    let b = g.node(300.0, 0.0, 36.0);
    let c1 = g.node(-20.0, 500.0, 36.0);
    let c2 = g.node(300.0, 500.0, 36.0);
    g.link(a, b, LinkKind::LongJump, Some(Action::LongJump { robustness: 1.0 }));
    g.walk(a, c1);
    g.walk(c1, c2);
    g.walk(c2, b);
    let graph = g.build();
    let from = Vec3::new(-20.0, 0.0, 36.0);
    let to = Vec3::new(300.0, 0.0, 36.0);
    for module in [true, false] {
        let mut c = Course::new(gap_world(), Game::default(), graph.clone());
        let mut bot = CourseBot::new(from, 100.0);
        bot.tricks.longjump = module;
        c.place(&mut bot);
        let o = c.run(&mut bot, to, 20.0, 100.0, None);
        assert!(o.arrived && o.failures.is_empty(), "module {module}: {}", describe(&o));
        let leapt = phases(&o).contains(&"longjump:air");
        assert_eq!(leapt, module, "module {module}: {}", describe(&o));
        if module {
            assert!(o.seconds < 3.0, "straight across: {}", describe(&o));
        }
    }
}

#[test]
fn a_gauss_boost_gets_the_bot_onto_a_ledge() {
    let mut w = BoxWorld::new();
    w.floor(0.0, 4096.0);
    w.solid(Vec3::new(400.0, -256.0, 0.0), Vec3::new(900.0, 256.0, 200.0));
    let mut g = Builder::default();
    let a = g.node(0.0, 0.0, 36.0);
    let b = g.node(600.0, 0.0, 236.0);
    g.link(
        a,
        b,
        LinkKind::GaussBoost,
        Some(Action::GaussBoost {
            pitch: 34.0,
            robustness: 1.0,
            push: 1000.0,
        }),
    );
    let mut c = Course::new(w, Game::default(), g.build());
    let from = Vec3::new(0.0, 0.0, 36.0);
    let mut bot = CourseBot::new(from, 100.0);
    bot.tricks.gauss_boost = true;
    bot.tricks.boost_now = true;
    bot.tricks.gauss_damage = 200.0;
    c.place(&mut bot);
    let o = c.run(&mut bot, Vec3::new(600.0, 0.0, 236.0), 15.0, 100.0, None);
    assert!(o.arrived && o.failures.is_empty(), "{}", describe(&o));
    assert!(phases(&o).contains(&"boost:air"), "{}", describe(&o));
    // Without the gun there is no way up.
    let mut c = Course::new(
        {
            let mut w = BoxWorld::new();
            w.floor(0.0, 4096.0);
            w.solid(Vec3::new(400.0, -256.0, 0.0), Vec3::new(900.0, 256.0, 200.0));
            w
        },
        Game::default(),
        {
            let mut g = Builder::default();
            let a = g.node(0.0, 0.0, 36.0);
            let b = g.node(600.0, 0.0, 236.0);
            g.link(
                a,
                b,
                LinkKind::GaussBoost,
                Some(Action::GaussBoost {
                    pitch: 34.0,
                    robustness: 1.0,
                    push: 1000.0,
                }),
            );
            g.build()
        },
    );
    let mut bot = CourseBot::new(from, 100.0);
    c.place(&mut bot);
    let o = c.run(&mut bot, Vec3::new(600.0, 0.0, 236.0), 3.0, 100.0, None);
    assert!(!o.arrived && !phases(&o).contains(&"boost:charge"), "{}", describe(&o));
}

#[test]
fn a_long_jump_speeds_the_bot_along_a_straight_run() {
    let mut w = BoxWorld::new();
    w.floor(0.0, 4096.0);
    let mut g = Builder::default();
    let nodes: Vec<NodeId> = (0..=12).map(|i| g.node(i as f32 * 150.0, 0.0, 36.0)).collect();
    for p in nodes.windows(2) {
        g.walk(p[0], p[1]);
    }
    let graph = g.build();
    let from = Vec3::new(0.0, 0.0, 36.0);
    let to = Vec3::new(1800.0, 0.0, 36.0);
    let mut times = Vec::new();
    for runway in [false, true] {
        let mut c = Course::new(w.clone(), Game::default(), graph.clone());
        let mut bot = CourseBot::new(from, 100.0);
        bot.tricks.longjump = true;
        bot.tricks.runway = runway;
        c.place(&mut bot);
        let o = c.run(&mut bot, to, 15.0, 100.0, None);
        assert!(o.arrived && o.failures.is_empty(), "runway {runway}: {}", describe(&o));
        assert_eq!(phases(&o).contains(&"longjump:air"), runway, "{}", describe(&o));
        times.push(o.seconds);
    }
    assert!(times[1] < times[0] * 0.85, "long jumps save time: {times:?}");
}

#[test]
fn a_gauss_jump_on_the_way_lands_nearer_the_goal_and_the_way_goes_on() {
    // A long walk round a wall to a far goal: the boost clears the wall.
    let mut w = BoxWorld::new();
    w.floor(0.0, 4096.0);
    w.solid(Vec3::new(600.0, -200.0, 0.0), Vec3::new(640.0, 2000.0, 160.0));
    let mut g = Builder::default();
    let round = [
        g.node(0.0, 0.0, 36.0),
        g.node(300.0, -400.0, 36.0),
        g.node(900.0, -400.0, 36.0),
    ];
    let goal = g.node(1500.0, 0.0, 36.0);
    let beyond = g.node(1200.0, 0.0, 36.0);
    for p in round.windows(2) {
        g.walk(p[0], p[1]);
    }
    g.walk(round[round.len() - 1], beyond);
    g.walk(beyond, goal);
    let mut c = Course::new(w, Game::default(), g.build().with_landmarks());
    let from = Vec3::new(0.0, 0.0, 36.0);
    let to = Vec3::new(1500.0, 0.0, 36.0);
    let mut bot = CourseBot::new(from, 100.0);
    bot.tricks.boost_now = true;
    bot.tricks.gauss_damage = 200.0;
    c.place(&mut bot);
    // A frame to plan the way, then the boost is asked for.
    c.frame(&mut bot, to, 10.0);
    assert!(c.gauss_leap(&mut bot), "a boost toward the goal");
    let o = c.run(&mut bot, to, 15.0, 100.0, None);
    assert!(o.arrived && o.failures.is_empty(), "{}", describe(&o));
    assert!(phases(&o).contains(&"boost:air"), "{}", describe(&o));
    assert_eq!(bot.nav.tricks.landed[3], 1, "{:?}", bot.nav.tricks);
}

/// A bot with the module on a course: long jumps along the way, bold ones or not, and the fall damage a bold one
/// may take; how it went, and how many long jumps it made.
fn run_leaping(graph: &NavGraph, w: &BoxWorld, from: Vec3, to: Vec3, bold: bool, hurt: f32) -> (Outcome, usize, f32) {
    let mut c = Course::new(w.clone(), Game::default(), graph.clone());
    let mut bot = CourseBot::new(from, 100.0);
    bot.tricks.longjump = true;
    bot.tricks.runway = true;
    bot.tricks.runway_bold = bold;
    bot.tricks.runway_hurt = hurt;
    c.place(&mut bot);
    let o = c.run(&mut bot, to, 20.0, 100.0, None);
    let leaps = phases(&o).iter().filter(|p| **p == "longjump:air").count();
    (o, leaps, 100.0 - bot.health)
}

#[test]
fn bold_long_jumps_follow_one_another_along_a_straight_run() {
    let mut w = BoxWorld::new();
    w.floor(0.0, 4096.0);
    let mut g = Builder::default();
    let nodes: Vec<NodeId> = (0..=12).map(|i| g.node(i as f32 * 150.0, 0.0, 36.0)).collect();
    for p in nodes.windows(2) {
        g.walk(p[0], p[1]);
    }
    let graph = g.build();
    let (from, to) = (Vec3::new(0.0, 0.0, 36.0), Vec3::new(1800.0, 0.0, 36.0));
    let (plain, plain_leaps, _) = run_leaping(&graph, &w, from, to, false, 0.0);
    let (bold, bold_leaps, _) = run_leaping(&graph, &w, from, to, true, 0.0);
    assert!(
        plain.arrived && bold.arrived && bold.failures.is_empty(),
        "{}",
        describe(&bold)
    );
    assert!(
        bold_leaps >= 3 && bold_leaps > plain_leaps,
        "{plain_leaps} {bold_leaps}: {}",
        describe(&bold)
    );
    assert!(bold.seconds < plain.seconds, "{} {}", bold.seconds, plain.seconds);
}

#[test]
fn bold_long_jumps_cut_across_a_winding_way() {
    // The way zigzags 60 units across an open floor: no straight stretch, a clear line all along.
    let mut w = BoxWorld::new();
    w.floor(0.0, 4096.0);
    let mut g = Builder::default();
    let nodes: Vec<NodeId> = (0..=10)
        .map(|i| g.node(i as f32 * 110.0, if i % 2 == 0 { 0.0 } else { 60.0 }, 36.0))
        .collect();
    for p in nodes.windows(2) {
        g.walk(p[0], p[1]);
    }
    let graph = g.build();
    let (from, to) = (Vec3::new(0.0, 0.0, 36.0), Vec3::new(1100.0, 0.0, 36.0));
    let (plain, plain_leaps, _) = run_leaping(&graph, &w, from, to, false, 0.0);
    let (bold, bold_leaps, _) = run_leaping(&graph, &w, from, to, true, 0.0);
    assert!(
        plain.arrived && bold.arrived && bold.failures.is_empty(),
        "{}",
        describe(&bold)
    );
    assert_eq!(plain_leaps, 0, "not straight: {}", describe(&plain));
    assert!(bold_leaps >= 1, "{}", describe(&bold));
}

/// A ledge `high` units up with the way off its edge (a drop) and on along the floor below.
fn ledge_course(high: f32) -> (NavGraph, BoxWorld) {
    let mut w = BoxWorld::new();
    w.floor(0.0, 4096.0);
    w.solid(Vec3::new(-400.0, -256.0, 0.0), Vec3::new(220.0, 256.0, high));
    let mut g = Builder::default();
    let top: Vec<NodeId> = (0..=4)
        .map(|i| g.node(-200.0 + i as f32 * 100.0, 0.0, high + 36.0))
        .collect();
    let below: Vec<NodeId> = (0..=6).map(|i| g.node(280.0 + i as f32 * 100.0, 0.0, 36.0)).collect();
    for p in top.windows(2).chain(below.windows(2)) {
        g.walk(p[0], p[1]);
    }
    g.link(
        top[4],
        below[0],
        LinkKind::Drop,
        Some(Action::Drop {
            speed: 150.0,
            damage: 0.0,
        }),
    );
    (g.build(), w)
}

#[test]
fn bold_long_jumps_go_down_drops_and_land_hard_only_with_health_to_spare() {
    let (from, to) = (Vec3::new(-200.0, 0.0, 0.0), Vec3::new(880.0, 0.0, 36.0));
    // Off a ledge 120 units up: no harm in it.
    let (graph, w) = ledge_course(120.0);
    let start = from + Vec3::Z * 156.0;
    let (plain, _, _) = run_leaping(&graph, &w, start, to, false, 0.0);
    let (bold, _, hurt) = run_leaping(&graph, &w, start, to, true, 0.0);
    assert!(
        plain.arrived && bold.arrived && bold.failures.is_empty(),
        "{}",
        describe(&bold)
    );
    let dropped = |o: &Outcome| phases(o).iter().any(|p| p.starts_with("drop"));
    assert!(dropped(&plain), "walked off the edge: {}", describe(&plain));
    assert!(!dropped(&bold), "flew down: {}", describe(&bold));
    assert!(bold.seconds < plain.seconds, "{} {}", bold.seconds, plain.seconds);
    assert_eq!(hurt, 0.0);
    // Off one 300 units up the landing hurts: only a bot that may take it long jumps down.
    let (graph, w) = ledge_course(300.0);
    let start = from + Vec3::Z * 336.0;
    let (careful, _, _) = run_leaping(&graph, &w, start, to, true, 0.0);
    let (hardy, _, hurt) = run_leaping(&graph, &w, start, to, true, 60.0);
    assert!(careful.arrived && hardy.arrived, "{}", describe(&hardy));
    assert!(dropped(&careful), "walked off the edge: {}", describe(&careful));
    assert!(!dropped(&hardy), "flew down: {}", describe(&hardy));
    assert!(hurt > 0.0 && hurt <= 60.0, "{hurt}");
}

#[test]
fn bold_long_jumps_take_short_stretches_to_the_goal() {
    let mut w = BoxWorld::new();
    w.floor(0.0, 4096.0);
    let mut g = Builder::default();
    let nodes: Vec<NodeId> = (0..=4).map(|i| g.node(i as f32 * 80.0, 0.0, 36.0)).collect();
    for p in nodes.windows(2) {
        g.walk(p[0], p[1]);
    }
    let graph = g.build();
    let (from, to) = (Vec3::new(0.0, 0.0, 36.0), Vec3::new(320.0, 0.0, 36.0));
    let (plain, plain_leaps, _) = run_leaping(&graph, &w, from, to, false, 0.0);
    let (bold, bold_leaps, _) = run_leaping(&graph, &w, from, to, true, 0.0);
    assert!(
        plain.arrived && bold.arrived && bold.failures.is_empty(),
        "{}",
        describe(&bold)
    );
    assert_eq!(plain_leaps, 0, "a runway is 400 units long at least");
    assert_eq!(bold_leaps, 1, "{}", describe(&bold));
}
