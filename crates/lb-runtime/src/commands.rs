//! `lb` console commands (server console, rcon, authorized clients, telemetry channel).

use lb_config::MainConfig;
use lb_config::main_config::QuotaMode;
use lb_core::math::normalize_angle;
use lb_host::{Host, TraceKind, TraceRequest};

use crate::Runtime;
use crate::cvars::Cv;
use crate::manager::BotState;
use crate::motor_test::{MotorTest, Script};

const HELP: &[(&str, &str)] = &[
    ("add [count]", "add bots (raises the quota)"),
    ("kick [#userid|name|all]", "kick bots (lowers the quota)"),
    ("kill [#userid|all]", "kill bots with the `kill` client command"),
    ("quota <n> [normal|fill|match]", "set the bot quota"),
    ("list", "list bots"),
    ("status", "runtime status and counters"),
    (
        "perf [reset|bots]",
        "core time per frame (p50/p95/p99/max) or command timing per bot",
    ),
    ("compat", "compatibility profile of this server"),
    ("config show|reload", "show or reload config/lambdabots.yaml"),
    (
        "test motor <#userid|all> run|strafe|jump|duckjump|spin [arg]",
        "scripted motor measurement",
    ),
    ("debug panic|stall|stalecmd ...", "fault injection (requires lb_dev 1)"),
    ("version", "versions"),
];

pub fn execute(rt: &mut Runtime, host: &mut dyn Host, args: &[&str]) -> Vec<String> {
    let sub = args.first().copied().unwrap_or("help");
    let rest = if args.is_empty() { &[][..] } else { &args[1..] };
    match sub {
        "help" => HELP.iter().map(|(c, d)| format!("lb {c:<58} {d}")).collect(),
        "version" => vec![format!(
            "lambdabots core {} adapter {} abi {}",
            crate::CORE_VERSION,
            rt.init.adapter_version,
            lb_ffi::LB_ABI_VERSION
        )],
        "list" => list(rt),
        "status" => status(rt),
        "perf" => perf(rt, rest),
        "compat" => rt.compat.to_yaml().lines().map(String::from).collect(),
        "add" => add(rt, host, rest),
        "kick" => kick(rt, host, rest),
        "kill" => kill(rt, rest),
        "quota" => quota(rt, host, rest),
        "config" => config(rt, rest),
        "test" => test(rt, host, rest),
        "debug" => debug(rt, host, rest),
        other => vec![format!("lb: unknown command `{other}`, see `lb help`")],
    }
}

fn list(rt: &Runtime) -> Vec<String> {
    let mut out = vec![format!(
        "{} bots (quota {} {:?})",
        rt.bots.len(),
        rt.config.quota.count,
        rt.config.quota.mode
    )];
    for b in &rt.bots {
        let body = &b.self_state.body;
        out.push(format!(
            "  #{:<4} slot {:<2} {:<20} {:<10} hp {:>3} ap {:>3} yaw {:>4.0}/{:<4.0} weapon {}",
            b.userid,
            b.id.slot,
            b.identity.name,
            b.state.as_str(),
            body.health as i32,
            body.armor as i32,
            b.view.y,
            body.v_angle.y,
            b.self_state.current_weapon.get().map(|w| w.classname()).unwrap_or("-"),
        ));
    }
    out
}

fn status(rt: &Runtime) -> Vec<String> {
    let s = &rt.stats;
    vec![
        format!("map: {}", rt.map.as_ref().map(|m| m.name.as_str()).unwrap_or("-")),
        format!("mode: {:?}", rt.game.mode),
        format!("safe mode: {}", rt.safe_mode.as_deref().unwrap_or("no")),
        format!(
            "frames {} | commands {} | stale moves {} | move failures {}",
            s.frames, s.commands_sent, s.stale_moves, s.move_calls_failed
        ),
        format!(
            "dropped events {} | malformed records {} | bot faults {}",
            s.dropped_events, s.malformed_records, s.bot_faults
        ),
        format!(
            "cmd rate {} | max frame time {:.1} ms",
            rt.config.engine.cmd_rate, s.max_frame_ms
        ),
        format!(
            "telemetry: {} (sent {}, dropped {}), command channel rejected {}",
            rt.telemetry.is_enabled(),
            rt.telemetry.sent,
            rt.telemetry.dropped,
            rt.commands.rejected
        ),
    ]
}

fn perf(rt: &mut Runtime, args: &[&str]) -> Vec<String> {
    if args.first() == Some(&"reset") {
        rt.core_times.reset();
        return vec!["perf counters reset".into()];
    }
    if args.first() == Some(&"bots") {
        let mut out = vec![format!(
            "{:<20} {:>8} {:>12} {:>12} {:>10} {:>9}",
            "bot", "cmds", "frame ms", "sent ms", "dropped", "unsent"
        )];
        for b in &rt.bots {
            let d = &b.driver;
            out.push(format!(
                "{:<20} {:>8} {:>12.1} {:>12} {:>10.1} {:>9.2}",
                b.identity.name,
                d.commands_sent,
                d.total_frame_ms,
                d.total_sent_ms,
                d.dropped_debt_ms,
                d.unsent_ms()
            ));
        }
        return out;
    }
    let t = &rt.core_times;
    let p = t.percentiles();
    let avg = if t.frames > 0 {
        t.total_ns as f64 / t.frames as f64 / 1000.0
    } else {
        0.0
    };
    vec![
        format!(
            "core time per frame, last {} frames: p50 {:.1} us, p95 {:.1} us, p99 {:.1} us, max {:.1} us",
            p.samples, p.p50_us, p.p95_us, p.p99_us, p.max_us
        ),
        format!(
            "since reset: {} frames, avg {:.1} us, max {:.1} us",
            t.frames,
            avg,
            t.max_ns as f64 / 1000.0
        ),
    ]
}

fn find_bots(rt: &Runtime, target: Option<&str>) -> Vec<usize> {
    match target {
        None | Some("all") => (0..rt.bots.len()).collect(),
        Some(t) if t.starts_with('#') => {
            let id: i32 = t[1..].parse().unwrap_or(-1);
            rt.bots.iter().position(|b| b.userid == id).into_iter().collect()
        }
        Some(name) => rt
            .bots
            .iter()
            .position(|b| b.identity.name.eq_ignore_ascii_case(name))
            .into_iter()
            .collect(),
    }
}

fn set_quota(rt: &mut Runtime, host: &mut dyn Host, count: u32) {
    rt.config.quota.count = count.min(32);
    rt.cvars.set(host, Cv::Quota, &rt.config.quota.count.to_string());
}

fn add(rt: &mut Runtime, host: &mut dyn Host, args: &[&str]) -> Vec<String> {
    let n: u32 = args.first().and_then(|a| a.parse().ok()).unwrap_or(1).clamp(1, 32);
    let count = rt.config.quota.count + n;
    set_quota(rt, host, count);
    rt.creation.next_at = Some(rt.now);
    vec![format!(
        "quota is now {} ({:?})",
        rt.config.quota.count, rt.config.quota.mode
    )]
}

fn kick(rt: &mut Runtime, host: &mut dyn Host, args: &[&str]) -> Vec<String> {
    let target = args.first().copied();
    let mut indices = find_bots(rt, target.or(Some("__one__")).filter(|t| *t != "__one__"));
    if target.is_none() {
        indices = crate::manager::pick_bot_to_kick(&rt.bots).into_iter().collect();
    }
    if indices.is_empty() {
        return vec!["no such bot".into()];
    }
    let n = indices.len() as u32;
    let names: Vec<String> = indices.iter().map(|&i| rt.bots[i].identity.name.clone()).collect();
    for &i in indices.iter().rev() {
        rt.kick_bot_index(host, i, "kicked by admin");
    }
    let count = if target == Some("all") {
        0
    } else {
        rt.config.quota.count.saturating_sub(n)
    };
    set_quota(rt, host, count);
    vec![format!("kicked {} (quota {})", names.join(", "), rt.config.quota.count)]
}

fn kill(rt: &mut Runtime, args: &[&str]) -> Vec<String> {
    let indices = find_bots(rt, args.first().copied());
    for &i in &indices {
        rt.bots[i].pending_client_cmds.push(vec!["kill".to_string()]);
    }
    vec![format!("kill sent to {} bot(s)", indices.len())]
}

fn quota(rt: &mut Runtime, host: &mut dyn Host, args: &[&str]) -> Vec<String> {
    let Some(n) = args.first().and_then(|a| a.parse::<u32>().ok()) else {
        return vec![format!("quota {} {:?}", rt.config.quota.count, rt.config.quota.mode)];
    };
    if let Some(mode) = args.get(1) {
        rt.config.quota.mode = match *mode {
            "normal" => QuotaMode::Normal,
            "fill" => QuotaMode::Fill,
            "match" => QuotaMode::Match,
            other => return vec![format!("unknown quota mode `{other}`")],
        };
        rt.cvars.set(host, Cv::QuotaMode, mode);
    }
    set_quota(rt, host, n);
    vec![format!("quota {} {:?}", rt.config.quota.count, rt.config.quota.mode)]
}

fn config(rt: &mut Runtime, args: &[&str]) -> Vec<String> {
    match args.first().copied() {
        Some("reload") => {
            let path = rt.init.install_dir.join("config").join("lambdabots.yaml");
            match std::fs::read_to_string(&path)
                .map_err(|e| e.to_string())
                .and_then(|t| MainConfig::parse(&t, &path.display().to_string()).map_err(|e| e.to_string()))
            {
                Ok(cfg) => {
                    rt.config = cfg;
                    rt.config_source = path.display().to_string();
                    vec!["config reloaded".into()]
                }
                Err(e) => vec![format!("config not reloaded: {e}")],
            }
        }
        _ => {
            let mut out = vec![format!("# source: {}", rt.config_source)];
            match lb_config::yaml::to_string(&rt.config) {
                Ok(text) => out.extend(text.lines().map(String::from)),
                Err(e) => out.push(e.to_string()),
            }
            out
        }
    }
}

/// Yaw of the longest clear run for a standing player (other players are ignored).
fn open_yaw(host: &mut dyn Host, origin: lb_core::Vec3) -> f32 {
    let mut best = (0.0f32, -1.0f32);
    for step in 0..16 {
        let yaw = step as f32 * 22.5;
        let (s, c) = yaw.to_radians().sin_cos();
        let end = origin + lb_core::Vec3::new(c, s, 0.0) * 1200.0;
        let tr = host.trace(&TraceRequest {
            start: origin,
            end,
            kind: TraceKind::Hull(1),
            ignore_monsters: true,
            ignore_glass: false,
            ignore: None,
        });
        if tr.fraction > best.1 {
            best = (yaw, tr.fraction);
        }
    }
    best.0
}

fn test(rt: &mut Runtime, host: &mut dyn Host, args: &[&str]) -> Vec<String> {
    if args.first() != Some(&"motor") {
        return vec![
            "usage: lb test motor <#userid|all> run|strafe|jump|duckjump|spin [arg] | lb test motor stop".into(),
        ];
    }
    if args.get(1) == Some(&"stop") {
        for b in &mut rt.bots {
            b.test = None;
        }
        return vec!["motor tests stopped".into()];
    }
    let Some(script) = Script::parse(args.get(2..).unwrap_or(&[])) else {
        return vec!["unknown motor script".into()];
    };
    let indices = find_bots(rt, args.get(1).copied());
    let now = rt.now;
    let mut started = 0;
    for i in indices {
        let b = &mut rt.bots[i];
        if b.state == BotState::Alive {
            let origin = b.self_state.body.origin;
            let yaw = match script {
                Script::Run { .. } => open_yaw(host, origin),
                Script::Strafe { .. } => normalize_angle(open_yaw(host, origin) + 90.0),
                _ => b.view.y,
            };
            b.test = Some(MotorTest::new(script, now, origin, yaw));
            started += 1;
        }
    }
    vec![format!(
        "motor test {script:?} started on {started} bot(s); results go to the log and console"
    )]
}

fn debug(rt: &mut Runtime, host: &mut dyn Host, args: &[&str]) -> Vec<String> {
    if !rt.dev {
        return vec!["debug commands require lb_dev 1".into()];
    }
    match args.first().copied() {
        Some("panic") => {
            let indices = find_bots(rt, args.get(1).copied());
            for &i in &indices {
                rt.bots[i].fault_on_next_frame = true;
            }
            vec![format!("fault injected into {} bot(s)", indices.len())]
        }
        Some("stall") => {
            let ms: u64 = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(300).min(5000);
            std::thread::sleep(std::time::Duration::from_millis(ms));
            vec![format!("stalled the main thread for {ms} ms")]
        }
        Some("stalecmd") => {
            let Some(b) = rt.bots.first() else {
                return vec!["no bots".into()];
            };
            let cmd = lb_ffi::LbBotCommand {
                slot: b.id.slot,
                flags: 0,
                buttons: 0,
                bot_gen: b.id.generation.wrapping_add(1000),
                view_angles: lb_ffi::LbVec3::default(),
                forwardmove: 0.0,
                sidemove: 0.0,
                upmove: 0.0,
                impulse: 0,
                msec: 10,
                pad: 0,
                random_seed: 0,
            };
            let mut fb = Vec::new();
            host.run_player_moves(&[cmd], &mut fb);
            let status = fb.first().map(|f| f.status).unwrap_or(255);
            vec![format!(
                "stale command status {status} (expected {} = stale)",
                lb_ffi::LB_MOVE_STALE
            )]
        }
        Some("capture") => {
            let secs: f64 = args
                .get(1)
                .and_then(|a| a.parse::<f64>().ok())
                .unwrap_or(30.0)
                .clamp(1.0, 600.0);
            let map = rt.map.as_ref().map(|m| m.name.clone()).unwrap_or_else(|| "none".into());
            let dir = rt.init.install_dir.join("logs");
            match crate::capture::MessageCapture::start(&dir, &map, rt.now + secs) {
                Ok(c) => {
                    let path = c.path.display().to_string();
                    rt.capture = Some(c);
                    vec![format!("capturing user messages for {secs} s into {path}")]
                }
                Err(e) => vec![format!("capture failed: {e}")],
            }
        }
        Some("safemode") => {
            rt.safe_mode = Some("entered by lb debug safemode".into());
            vec!["safe mode entered".into()]
        }
        _ => vec!["usage: lb debug panic <#userid|all> | stall <ms> | stalecmd | capture <s> | safemode".into()],
    }
}
