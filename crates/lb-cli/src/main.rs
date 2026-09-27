//! Command line tools: config, replay, navigation, BSP, ABI.

#![forbid(unsafe_code)]

use std::path::Path;
use std::process::ExitCode;

use anyhow::{Result, bail};
use lb_config::check::{check_file, yaml_files};

const USAGE: &str = "usage:
  lb-cli config check <file-or-dir>...
  lb-cli nav gen <map.bsp> [--out <file.lbnav>]
  lb-cli nav coverage <map.bsp> [--json]
  lb-cli nav path <map.bsp> <x,y,z> <x,y,z>
  lb-cli nav validate-overlay <map.bsp> <overlay.yaml>...
  lb-cli nav tracecheck <map.bsp> <tracedump.jsonl>
  lb-cli replay <recording.lbrec> [--console] [--keep] [--dir <dir>] [--diffs <n>]";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    match run(&refs) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(2)
        }
    }
}

fn run(args: &[&str]) -> Result<bool> {
    match args {
        ["config", "check", paths @ ..] if !paths.is_empty() => Ok(config_check(paths)),
        ["nav", "gen", bsp, opts @ ..] => nav_gen(Path::new(bsp), opts),
        ["nav", "coverage", bsp, opts @ ..] => nav_coverage(Path::new(bsp), opts),
        ["nav", "path", bsp, from, to] => nav_path(Path::new(bsp), from, to),
        ["nav", "validate-overlay", bsp, files @ ..] if !files.is_empty() => validate_overlay(Path::new(bsp), files),
        ["nav", "tracecheck", bsp, dump] => trace_check(Path::new(bsp), Path::new(dump)),
        ["replay", file, opts @ ..] => replay(Path::new(file), opts),
        _ => bail!(USAGE),
    }
}

/// Makes the graph of a map, as the server does with the default physics.
fn generate(bsp: &Path) -> Result<(lb_bsp::BspWorld, lb_navgen::Generated, lb_nav::store::GraphKey)> {
    let bytes = std::fs::read(bsp).map_err(|e| anyhow::anyhow!("{}: {e}", bsp.display()))?;
    let mut world = lb_bsp::BspWorld::load(&bytes).map_err(|e| anyhow::anyhow!("{}: {e}", bsp.display()))?;
    let mech = lb_bsp::mech::Mechanisms::from_world(&world);
    let opts = lb_navgen::GenOptions::default();
    let name = bsp.file_stem().and_then(|s| s.to_str()).unwrap_or("map");
    let generated = lb_navgen::generate(&mut world, &mech, &opts, name);
    let key = lb_navgen::cache::key(&world, &opts, 0, 0);
    Ok((world, generated, key))
}

fn nav_gen(bsp: &Path, args: &[&str]) -> Result<bool> {
    let out = match args {
        [] => bsp.with_extension("lbnav"),
        ["--out", path] => Path::new(path).to_path_buf(),
        _ => bail!(USAGE),
    };
    let (_, generated, key) = generate(bsp)?;
    let s = &generated.graph.stats;
    println!(
        "{} nodes, {} links ({}), {} traces, {} ms",
        s.nodes,
        s.links,
        s.kinds(),
        s.traces,
        s.millis
    );
    let stages: Vec<String> = generated.timings.iter().map(|(n, ms)| format!("{n} {ms}")).collect();
    println!("stages, ms: {}", stages.join(", "));
    let c = lb_navgen::report::coverage(&generated);
    println!(
        "coverage {:.1}%: {} of {} nodes reached from the spawn points and back; items {}/{}",
        c.ratio() * 100.0,
        c.roundtrip,
        c.nodes,
        c.items_ok,
        c.items
    );
    std::fs::write(&out, lb_nav::store::write(&generated.graph, &key))
        .map_err(|e| anyhow::anyhow!("{}: {e}", out.display()))?;
    println!("written {}", out.display());
    Ok(true)
}

fn nav_coverage(bsp: &Path, args: &[&str]) -> Result<bool> {
    let json = match args {
        [] => false,
        ["--json"] => true,
        _ => bail!(USAGE),
    };
    let (world, generated, _) = generate(bsp)?;
    let c = lb_navgen::report::coverage(&generated);
    let lost = lb_navgen::report::lost_items(&generated, &world, &c);
    if json {
        let doc = serde_json::json!({
            "map": bsp.file_stem().and_then(|s| s.to_str()),
            "coverage": c.ratio(),
            "report": c,
            "millis": generated.graph.stats.millis,
            "links": generated.graph.stats.kinds(),
            "lost_items": lost,
        });
        println!("{}", serde_json::to_string_pretty(&doc)?);
        return Ok(c.ratio() >= 0.95);
    }
    println!(
        "coverage {:.1}% ({} of {} spans), {} of {} nodes reached from the spawn points and back, items {}/{}, \
         {} spawn points, made in {} ms",
        c.ratio() * 100.0,
        c.covered,
        c.spans,
        c.roundtrip,
        c.nodes,
        c.items_ok,
        c.items,
        c.spawns,
        generated.graph.stats.millis
    );
    for (at, spans) in c.cut_off.iter().take(10) {
        println!("  cut off: {spans} spans around {:.0} {:.0} {:.0}", at.x, at.y, at.z);
    }
    for item in &lost {
        let nearest = item
            .nearest
            .map(|(o, d, dz)| {
                format!(
                    "; nearest node reached {:.0} {:.0} {:.0}, {d:.0} u away, {dz:+.0} u",
                    o.x, o.y, o.z
                )
            })
            .unwrap_or_default();
        println!(
            "  {} at {:.0} {:.0} {:.0}: {}{nearest}",
            item.class, item.origin.x, item.origin.y, item.origin.z, item.why
        );
    }
    Ok(c.ratio() >= 0.95)
}

/// Reads overlays and applies their patches to the map's graph, reporting what does not apply.
fn validate_overlay(bsp: &Path, files: &[&str]) -> Result<bool> {
    let mut overlays = Vec::new();
    let mut ok = true;
    for f in files {
        let text = std::fs::read_to_string(f).map_err(|e| anyhow::anyhow!("{f}: {e}"))?;
        match lb_config::overlay::OverlayFile::parse(&text, f) {
            Ok(o) => overlays.push(o),
            Err(e) => {
                println!("{e}");
                ok = false;
            }
        }
    }
    let (mut world, generated, _) = generate(bsp)?;
    for o in &overlays {
        if o.bsp_size.is_some_and(|s| s != world.bsp.fingerprint.1) {
            println!(
                "{}: made for a {}-byte BSP, this one has {} bytes",
                o.map,
                o.bsp_size.unwrap_or(0),
                world.bsp.fingerprint.1
            );
            ok = false;
        }
    }
    let patches: Vec<_> = overlays.iter().flat_map(|o| o.nav.patches.iter().cloned()).collect();
    let mech = lb_bsp::mech::Mechanisms::from_world(&world);
    let before = generated.graph.stats.links;
    let (patched, report) =
        lb_navgen::patch::apply(generated.graph, &patches, &mut world, &mech, lb_kin::Physics::default());
    println!(
        "{} places; {} of {} patches applied ({} links before, {} after)",
        overlays.iter().map(|o| o.places.len()).sum::<usize>(),
        report.applied,
        patches.len(),
        before,
        patched.stats.links
    );
    for p in &report.problems {
        println!("  {p}");
    }
    Ok(ok && report.problems.is_empty())
}

fn nav_path(bsp: &Path, from: &str, to: &str) -> Result<bool> {
    let point = |s: &str| -> Result<lb_core::Vec3> {
        let v: Vec<f32> = s
            .split(',')
            .map(|p| p.trim().parse::<f32>())
            .collect::<Result<_, _>>()?;
        match v[..] {
            [x, y, z] => Ok(lb_core::Vec3::new(x, y, z)),
            _ => bail!("`{s}`: expected x,y,z"),
        }
    };
    let (a, b) = (point(from)?, point(to)?);
    let (_, generated, _) = generate(bsp)?;
    let g = &generated.graph;
    let near = |p: lb_core::Vec3| g.nearest(p, 512.0, 1).first().map(|(n, _)| *n);
    let (Some(start), Some(goal)) = (near(a), near(b)) else {
        bail!("no node within 512 units of one of the points");
    };
    let Some(path) = lb_nav::plan::plan(g, start, goal, &|_, _| 0.0) else {
        println!("no path from node {start} to node {goal}");
        return Ok(false);
    };
    println!(
        "{} nodes, {:.1} s by the graph's costs",
        path.len(),
        lb_nav::plan::path_time(g, &path)
    );
    for w in path.windows(2) {
        let o = g.node(w[1]).origin;
        let kind = g.find_link(w[0], w[1]).map_or("?", |l| l.kind.as_str());
        println!("  {kind:<9} -> {:>5} at {:.0} {:.0} {:.0}", w[1], o.x, o.y, o.z);
    }
    Ok(true)
}

/// Runs a recording through a fresh core and reports whether the bots decide the same.
fn replay(file: &Path, args: &[&str]) -> Result<bool> {
    let mut opts = lb_runtime::record::ReplayOptions {
        max_diffs: 20,
        ..Default::default()
    };
    let mut console = false;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match *arg {
            "--console" => console = true,
            "--keep" => opts.keep_dir = true,
            "--dir" => {
                opts.dir = Some(
                    it.next()
                        .ok_or_else(|| anyhow::anyhow!("--dir needs a directory"))?
                        .into(),
                )
            }
            "--diffs" => opts.max_diffs = it.next().and_then(|n| n.parse().ok()).unwrap_or(opts.max_diffs),
            other => bail!("replay: unknown option `{other}`\n{USAGE}"),
        }
    }
    let report = lb_runtime::record::replay(file, &opts, &mut |line| {
        if console {
            println!("| {line}");
        }
    })
    .map_err(anyhow::Error::msg)?;
    for line in report.lines() {
        println!("{line}");
    }
    if opts.keep_dir {
        println!("files and logs of the replay: {}", report.dir.display());
    }
    Ok(report.matches())
}

fn config_check(paths: &[&str]) -> bool {
    let mut files = Vec::new();
    for p in paths {
        let path = Path::new(p);
        if path.is_dir() {
            files.extend(yaml_files(path));
        } else {
            files.push(path.to_path_buf());
        }
    }
    let mut ok = true;
    for f in &files {
        match check_file(f) {
            Ok(kind) => println!("ok     {} ({kind})", f.display()),
            Err(e) => {
                println!("error  {e}");
                ok = false;
            }
        }
    }
    println!(
        "{} file(s) checked, {}",
        files.len(),
        if ok { "all valid" } else { "errors found" }
    );
    ok
}

/// Compares engine traces dumped by `lb debug tracedump` with the offline BSP tracer. Traces that hit a brush entity
/// in the engine are skipped: doors and other movers are only known live.
fn trace_check(bsp: &Path, dump: &Path) -> Result<bool> {
    use lb_core::Vec3;
    use lb_worldq::{HullKind, TraceQuery};

    let world = lb_bsp::BspWorld::load(&std::fs::read(bsp)?)?;
    let v = |x: &serde_json::Value| -> Vec3 {
        Vec3::new(
            x[0].as_f64().unwrap_or(0.0) as f32,
            x[1].as_f64().unwrap_or(0.0) as f32,
            x[2].as_f64().unwrap_or(0.0) as f32,
        )
    };
    let (mut total, mut skipped, mut exact, mut close) = (0usize, 0usize, 0usize, 0usize);
    let mut worst: Vec<(f32, String)> = Vec::new();
    for line in std::fs::read_to_string(dump)?.lines().filter(|l| !l.trim().is_empty()) {
        let t: serde_json::Value = serde_json::from_str(line)?;
        if t["hit"].as_u64().unwrap_or(0) != 0 {
            skipped += 1;
            continue;
        }
        total += 1;
        let hull = HullKind::ALL[t["hull"].as_u64().unwrap_or(0).min(3) as usize];
        let q = TraceQuery::hull(v(&t["start"]), v(&t["end"]), hull);
        let ours = world
            .bsp
            .hull(0, hull)
            .map(|h| h.trace(q.start, q.end, Vec3::ZERO))
            .unwrap_or_else(|| lb_worldq::Trace::clear(q.end));
        let engine_fraction = t["fraction"].as_f64().unwrap_or(1.0) as f32;
        let engine_end = v(&t["endpos"]);
        let engine_start_solid = t["start_solid"].as_bool().unwrap_or(false);
        if ours.fraction == engine_fraction && ours.end == engine_end && ours.start_solid == engine_start_solid {
            exact += 1;
            continue;
        }
        let diff = (ours.fraction - engine_fraction).abs() * (q.end - q.start).length();
        if diff < 0.01 && ours.start_solid == engine_start_solid {
            close += 1;
            continue;
        }
        worst.push((
            diff,
            format!(
                "hull {:?} start {:?} end {:?}: engine fraction {engine_fraction} solid {engine_start_solid}, \
                 ours {} solid {}",
                hull, q.start, q.end, ours.fraction, ours.start_solid
            ),
        ));
    }
    worst.sort_by(|a, b| b.0.total_cmp(&a.0));
    println!(
        "{} traces compared ({} skipped: hit an entity in the engine): {} exact, {} within 0.01 u, {} different",
        total,
        skipped,
        exact,
        close,
        worst.len()
    );
    for (diff, text) in worst.iter().take(10) {
        println!("  {diff:8.3} u  {text}");
    }
    Ok(worst.is_empty())
}
