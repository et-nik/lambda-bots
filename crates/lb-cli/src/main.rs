//! Command line tools: config, replay, navigation, BSP, ABI.

#![forbid(unsafe_code)]

use std::path::Path;
use std::process::ExitCode;

use anyhow::{Result, bail};
use lb_config::check::{check_file, yaml_files};

const USAGE: &str = "usage:
  lb-cli config check <file-or-dir>...
  lb-cli nav tracecheck <map.bsp> <tracedump.jsonl>";

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
        ["nav", "tracecheck", bsp, dump] => trace_check(Path::new(bsp), Path::new(dump)),
        _ => bail!(USAGE),
    }
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
