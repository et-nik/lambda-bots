//! `lb-cli nav try`: a map's tests (`maps/<map>/tests.yaml`), or one attempt from a spot to a spot, on the offline
//! course. The graph is the server's (from its cache under `<install>/nav`, made now when it has none of this build
//! of the map) with the map's overlays; the navigation, the trick search and the executors are the server's; the
//! movement is `pm_shared`'s. Nothing is written.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use lb_bsp::BspWorld;
use lb_bsp::mech::Mechanisms;
use lb_config::map_tests::{Expect, MapTest, MapTests};
use lb_core::Vec3;
use lb_kin::Physics;
use lb_nav::reach::{Allowed, Outcome, Reach};
use lb_testkit::course::{Course, CourseBot, Game};

pub const USAGE: &str = "lb-cli nav try <map.bsp> [<test-id>...|all] [--install <dir>] [--repeat N] [--fps F]
  lb-cli nav try <map.bsp> --to <x,y,z> [--from <x,y,z>] [--give gauss,longjump] [--tricks jump,longjump,gauss]
                 [--radius R] [--timeout T] [--repeat N] [--install <dir>]";

fn point(s: &str) -> Result<Vec3> {
    let v: Vec<f32> = s
        .split(',')
        .map(|p| p.trim().parse::<f32>())
        .collect::<Result<_, _>>()
        .map_err(|_| anyhow!("`{s}`: expected x,y,z"))?;
    match v[..] {
        [x, y, z] => Ok(Vec3::new(x, y, z)),
        _ => bail!("`{s}`: expected x,y,z"),
    }
}

fn list(s: &str) -> Vec<String> {
    s.split(',').filter(|w| !w.is_empty()).map(str::to_string).collect()
}

/// The server's directory next to the map: `<game>/addons/lambdabots` for `<game>/maps/<map>.bsp`.
fn default_install(bsp: &Path) -> Option<PathBuf> {
    let dir = bsp.parent()?.parent()?.join("addons").join("lambdabots");
    dir.is_dir().then_some(dir)
}

pub fn run(bsp: &Path, args: &[&str]) -> Result<bool> {
    let (mut ids, mut install, mut repeat, mut fps) = (Vec::new(), None, None, 100.0);
    let (mut from, mut to, mut give, mut tricks) = (None, None, Vec::new(), None);
    let (mut radius, mut timeout) = (lb_config::map_tests::RADIUS, lb_config::map_tests::TIMEOUT);
    let mut it = args.iter();
    while let Some(&arg) = it.next() {
        let mut value = || {
            it.next()
                .copied()
                .ok_or_else(|| anyhow!("{arg} needs a value\n{USAGE}"))
        };
        match arg {
            "--install" => install = Some(PathBuf::from(value()?)),
            "--repeat" => repeat = Some(value()?.parse::<u32>().context("--repeat")?.max(1)),
            "--fps" => fps = value()?.parse::<f64>().context("--fps")?,
            "--from" => from = Some(point(value()?)?),
            "--to" => to = Some(point(value()?)?),
            "--give" => give = list(value()?),
            "--tricks" => tricks = Some(list(value()?)),
            "--radius" => radius = value()?.parse().context("--radius")?,
            "--timeout" => timeout = value()?.parse().context("--timeout")?,
            "all" => {}
            id if !id.starts_with("--") => ids.push(id.to_string()),
            other => bail!("unknown option {other}\n{USAGE}"),
        }
    }
    let map = bsp
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| anyhow!("{}: not a map", bsp.display()))?
        .to_string();
    let install = install.or_else(|| default_install(bsp));
    let bytes = std::fs::read(bsp).map_err(|e| anyhow!("{}: {e}", bsp.display()))?;
    let mut world = BspWorld::load(&bytes).map_err(|e| anyhow!("{}: {e}", bsp.display()))?;
    let mech = Mechanisms::from_world(&world);

    let kept = install
        .as_deref()
        .and_then(|i| lb_navgen::mapload::kept_graph(i, &map, &world));
    let (key, mut graph, mut source) = match kept {
        Some((key, g)) => (Some(key), g, "the server's graph".to_string()),
        None => {
            let g = lb_navgen::generate(&mut world, &mech, &lb_navgen::GenOptions::default(), &map).graph;
            (
                None,
                g,
                "a graph made now (the server has none of this build of the map)".to_string(),
            )
        }
    };
    let phys = key.as_ref().map_or_else(Physics::default, |k| k.movement);
    lb_navgen::mapload::prepare_world(&mut world, &mech);
    let patches: Vec<_> = install
        .as_deref()
        .map(|i| lb_navgen::mapload::read_overlays(i, &map, world.bsp.fingerprint.1))
        .unwrap_or_default()
        .into_iter()
        .flat_map(|o| o.nav.patches)
        .collect();
    if !patches.is_empty() {
        let edited = key
            .as_ref()
            .zip(install.as_deref())
            .and_then(|(k, i)| lb_navgen::mapload::edited_graph(i, &map, k, &patches));
        match edited {
            Some(g) => {
                graph = g;
                source = format!("the map editor's graph ({} overlay patches)", patches.len());
            }
            None => {
                let (g, report) = lb_navgen::patch::apply(graph, &patches, &mut world, &mech, phys);
                for p in &report.problems {
                    println!("overlay: {p}");
                }
                graph = g;
                source = format!(
                    "{source}, {} of {} overlay patches applied",
                    report.applied,
                    patches.len()
                );
            }
        }
    }
    let graph = graph.with_landmarks();
    lb_navgen::site::rest_poses(&mut world, &mech);
    let mut search = world;
    println!(
        "{map}: {source}; {} nodes; maxspeed {}, gravity {}",
        graph.len(),
        phys.maxspeed,
        phys.gravity
    );

    let cases: Vec<MapTest> = match to {
        Some(goal) => {
            let mut spot = |p: Vec3, what: &str| {
                lb_nav::reach::standing_spot(&mut search, p).ok_or_else(|| anyhow!("no floor under the {what} {p}"))
            };
            let mut t = MapTest::new("try", spot(goal, "goal")?.to_array());
            t.start = from.map(|p| spot(p, "start")).transpose()?.map(|p| p.to_array());
            t.give = give;
            t.tricks = tricks.unwrap_or_else(|| vec!["any".into()]);
            t.radius = radius;
            t.timeout = timeout;
            vec![t]
        }
        None => {
            let dir = install
                .as_deref()
                .ok_or_else(|| anyhow!("no server directory for the tests: --install <addons/lambdabots>"))?;
            let path = dir.join("maps").join(&map).join(lb_config::map_tests::FILE);
            let text = std::fs::read_to_string(&path).map_err(|e| anyhow!("{}: {e}", path.display()))?;
            let file = MapTests::parse(&text, &path.display().to_string())?;
            if ids.is_empty() {
                file.tests
            } else {
                ids.iter()
                    .map(|id| {
                        file.get(id)
                            .cloned()
                            .ok_or_else(|| anyhow!("no test `{id}` in {}", path.display()))
                    })
                    .collect::<Result<_>>()?
            }
        }
    };
    if cases.is_empty() {
        bail!("{map} has no tests");
    }
    let spawn = search
        .entities
        .iter()
        .find(|e| matches!(e.classname(), "info_player_deathmatch" | "info_player_start"))
        .map(|e| e.origin());

    let mut course_world = BspWorld::load(&bytes).map_err(|e| anyhow!("{}: {e}", bsp.display()))?;
    course_world.pushes = mech.push_fields();
    let game = Game::from_map(&mut course_world, &mech);
    let mut course = Course::new(course_world, game, graph);
    course.phys = phys;
    course.settle(10.0);
    let mut all = true;
    for t in &cases {
        let start = match t.start {
            Some(s) => Vec3::from(s),
            None => spawn
                .and_then(|s| lb_nav::reach::standing_spot(&mut search, s))
                .ok_or_else(|| anyhow!("{}: no start and no spawn point", t.id))?,
        };
        let allowed = Allowed::parse(&t.tricks).map_err(|e| anyhow!("{}: {e}", t.id))?;
        let gauss = t.give.iter().any(|g| matches!(g.as_str(), "gauss" | "weapon_gauss"));
        let longjump = t
            .give
            .iter()
            .any(|g| matches!(g.as_str(), "longjump" | "lj" | "item_longjump"));
        let attempts = repeat.unwrap_or(t.repeat);
        let mut passed = 0;
        for n in 0..attempts {
            let mut bot = CourseBot::new(start, 100.0);
            bot.tricks.longjump = longjump;
            if gauss {
                bot.tricks.gauss_boost = true;
                bot.tricks.boost_now = true;
                bot.tricks.gauss_damage = 200.0;
                bot.tricks.selfgauss = true;
            }
            course.place(&mut bot);
            let mut reach = Reach::new(Vec3::from(t.goal), t.radius, allowed, f64::from(t.timeout), course.now);
            let frames_for = f64::from(t.timeout) + 1.0;
            course.attempt(&mut bot, &mut reach, &mut search, frames_for, fps);
            let ok = match (t.expect, reach.outcome()) {
                (Expect::Arrive, Some(o)) => o.arrived(),
                (Expect::NoWay, Some(Outcome::NoWay { .. })) => true,
                _ => false,
            };
            passed += usize::from(ok);
            let mut lines = lb_runtime::orders::outcome_lines(&reach);
            if lines.is_empty() {
                lines.push("did not end".into());
            }
            println!(
                "{} [{}/{attempts}]: {}{}",
                t.id,
                n + 1,
                if ok { "" } else { "FAILED: " },
                lines[0]
            );
            for l in &lines[1..] {
                println!("{l}");
            }
        }
        println!("{}: {passed}/{attempts} passed", t.id);
        all &= passed == attempts as usize;
    }
    Ok(all)
}
