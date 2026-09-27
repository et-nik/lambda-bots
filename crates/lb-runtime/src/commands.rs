//! `lb` console commands (server console, rcon, authorized clients, telemetry channel).

use lb_config::main_config::QuotaMode;
use lb_core::math::normalize_angle;
use lb_host::{Host, TraceKind, TraceRequest};

use crate::cvars::Cv;
use crate::manager::BotState;
use crate::motor_test::{MotorTest, Script};
use crate::{Runtime, editor};

const HELP: &[(&str, &str)] = &[
    (
        "add [count|name]",
        "add bots or one personality by name (raises the quota)",
    ),
    ("kick [#userid|name|all]", "kick bots (lowers the quota)"),
    ("kill [#userid|all]", "kill bots with the `kill` client command"),
    ("quota <n> [normal|fill|match]", "set the bot quota"),
    ("list", "list bots"),
    ("roster [all]", "personalities admitted by the filter (or all of them)"),
    ("nav", "navigation graph status and what every bot is walking to"),
    ("nav regen", "forget the map's kept graphs and make the graph again"),
    (
        "overlay [reload]",
        "the map's overlays (places, graph patches); reload reads and applies them again",
    ),
    (
        "edit on|off|save|show|mark|link|unlink|forbid|place|info|undo",
        "the in-game editor of the map's overlay (needs lb_editor 1; run from the game console)",
    ),
    (
        "nav test <kind|all> [count] [name|#userid] | link <from> <to> ... | stop",
        "the obstacle course: a bot carries out special links (lift, jump, drop, ladder, door, ...)",
    ),
    (
        "vision [name|#userid]",
        "what bots see, hear and remember: contacts, tracks, sounds, recognition times",
    ),
    (
        "brain [name|#userid]",
        "decisions: goal and candidates, target, weapon, channel owners, reaction times",
    ),
    (
        "profile <name>",
        "one personality: style, skill, look and resolved skill parameters",
    ),
    ("status", "runtime status and counters"),
    (
        "weapons [all|melee|<weapon>...] [give]",
        "weapons bots may use; with give every bot gets them on spawn (needs sv_cheats 1)",
    ),
    (
        "stats [reset]",
        "weapon statistics: rounds, hit rate by distance, kills, suicides",
    ),
    (
        "selftest [name|#userid]",
        "check the game DLL's weapon rules with one bot while the others stand still (needs sv_cheats 1)",
    ),
    (
        "perf [reset|bots]",
        "core time per frame (p50/p95/p99/max) or command timing per bot",
    ),
    ("compat", "compatibility profile of this server"),
    (
        "config show|reload",
        "show the config, or reload config, skill table and profiles",
    ),
    (
        "test motor <#userid|all> run|strafe|jump|duckjump|spin [arg]",
        "scripted motor measurement",
    ),
    (
        "record [start [seconds]|stop]",
        "record the next map for `lb-cli replay` (starts with the map, see docs/replay.md)",
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
        "roster" => roster(rt, rest),
        "edit" => edit(rt, rest),
        "overlay" => match rest.first().copied() {
            Some("reload") => overlay_reload(rt),
            _ => overlay(rt),
        },
        "nav" => match rest.first().copied() {
            Some("test") => nav_test(rt, &rest[1..]),
            Some("regen") => nav_regen(rt),
            _ => nav(rt),
        },
        "vision" => vision(rt, rest),
        "brain" => brain(rt, rest),
        "profile" => profile(rt, rest),
        "status" => status(rt),
        "weapons" => weapons(rt, rest),
        "selftest" => selftest(rt, host, rest),
        "stats" => match rest.first().copied() {
            Some("reset") => {
                rt.arms_stats.reset(rt.now.secs());
                vec!["weapon statistics reset".into()]
            }
            _ => rt.arms_stats.report(rt.now.secs(), &rt.game.rules.damages),
        },
        "perf" => perf(rt, rest),
        "compat" => rt.compat.to_yaml().lines().map(String::from).collect(),
        "add" => add(rt, host, rest),
        "kick" => kick(rt, host, rest),
        "kill" => kill(rt, rest),
        "quota" => quota(rt, host, rest),
        "config" => config(rt, host, rest),
        "test" => test(rt, host, rest),
        "debug" => debug(rt, host, rest),
        "record" => record(rt, rest),
        other => vec![format!("lb: unknown command `{other}`, see `lb help`")],
    }
}

fn record(rt: &mut Runtime, args: &[&str]) -> Vec<String> {
    use crate::record::RecordRequest;
    match args {
        [] => vec![rt.record_status.clone()],
        ["start"] | ["start", _] => {
            let seconds = match args.get(1).map(|s| s.parse::<f64>()) {
                None => None,
                Some(Ok(s)) if s > 0.0 => Some(s),
                Some(_) => return vec!["lb record start [seconds]: seconds must be a positive number".into()],
            };
            rt.record_request = Some(RecordRequest::Start { seconds });
            vec![format!(
                "recording starts with the next map{}: `changelevel <map>` or `restart` to begin now",
                seconds.map(|s| format!(" and runs {s} s")).unwrap_or_default()
            )]
        }
        ["stop"] => {
            rt.record_request = Some(RecordRequest::Stop);
            vec!["recording stops".into()]
        }
        _ => vec!["usage: lb record [start [seconds]|stop]".into()],
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
        let goal = b
            .brain
            .mind
            .goal
            .map(|g| goal_text(rt, b, g.kind))
            .unwrap_or_else(|| "-".into());
        out.push(format!(
            "  #{:<4} slot {:<2} {:<20} {:<10} {:>3} {:<10} hp {:>3} ap {:>3} frags {:>3} weapon {:<18} {}",
            b.userid,
            b.id.slot,
            b.persona.name,
            b.persona.style.as_str(),
            b.persona.skill,
            b.state.as_str(),
            body.health as i32,
            body.armor as i32,
            body.frags as i32,
            b.self_state.current_weapon.get().map(|w| w.classname()).unwrap_or("-"),
            goal,
        ));
    }
    out
}

fn status(rt: &Runtime) -> Vec<String> {
    let s = &rt.stats;
    vec![
        rt.startup_summary(),
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
                b.persona.name,
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

/// `lb edit ...`: the in-game editor of the map's overlay.
fn edit(rt: &mut Runtime, args: &[&str]) -> Vec<String> {
    if !rt.editor_allowed {
        return vec!["the editor is off on this server (lb_editor 1 turns it on)".into()];
    }
    let Some(map) = rt.map.as_ref().map(|m| m.name.clone()) else {
        return vec!["no map".into()];
    };
    match args {
        ["on"] => {
            let Some(slot) = rt.command_slot else {
                return vec!["run it from the game console: the editor draws around the player".into()];
            };
            match editor::Editor::open(slot, &rt.init.install_dir, &map) {
                Ok(ed) => {
                    let (patches, places) = (ed.file.nav.patches.len(), ed.file.places.len());
                    rt.editor = Some(ed);
                    vec![format!(
                        "editing {map} ({patches} patches, {places} places saved); `lb edit` lists the commands"
                    )]
                }
                Err(e) => vec![e],
            }
        }
        ["off"] => match rt.editor.take() {
            Some(ed) if ed.unsaved() > 0 => vec![format!("editor off; {} changes not saved", ed.unsaved())],
            Some(_) => vec!["editor off".into()],
            None => vec!["the editor is not on".into()],
        },
        ["save"] => {
            let install = rt.init.install_dir.clone();
            let Some(ed) = rt.editor.as_mut() else {
                return vec!["the editor is not on (lb edit on)".into()];
            };
            match ed.save(&install) {
                Ok(path) => {
                    let mut out = vec![format!("saved {}", path.display())];
                    out.extend(overlay_reload(rt));
                    out
                }
                Err(e) => vec![e],
            }
        }
        _ => {
            let origin = rt.editor.as_ref().and_then(|ed| rt.client_origin(ed.slot));
            let graph = rt.graph.clone();
            match (rt.editor.as_mut(), origin) {
                (Some(ed), Some(origin)) => ed.command(args, graph.as_deref(), origin),
                (Some(_), None) => vec!["the editing player is not in the game".into()],
                (None, _) => vec!["the editor is not on (lb edit on)".into()],
            }
        }
    }
}

/// The map's overlays: places and graph patches.
fn overlay(rt: &Runtime) -> Vec<String> {
    if rt.overlays.is_empty() {
        return vec!["no overlay for this map (maps/<map>/overlay.yaml, editor.yaml)".into()];
    }
    let mut out = Vec::new();
    for o in rt.overlays.iter() {
        out.push(format!(
            "{}: {} places, {} graph patches",
            o.map,
            o.places.len(),
            o.nav.patches.len()
        ));
        for p in &o.places {
            out.push(format!(
                "  place {} at {:.0} {:.0} {:.0} r {:.0} {}",
                p.name,
                p.at[0],
                p.at[1],
                p.at[2],
                p.radius,
                p.tags.join(",")
            ));
        }
    }
    out.push(format!("graph: {}", rt.nav_status));
    out
}

/// Reads the overlays again and applies them to the map's graph (from the cache when it was made before).
fn overlay_reload(rt: &mut Runtime) -> Vec<String> {
    let Some(map) = rt.map.as_ref().map(|m| m.name.clone()) else {
        return vec!["no map".into()];
    };
    if rt.nav_loader.is_some() {
        return vec![format!("{map}: the graph is being loaded already")];
    }
    rt.start_nav_load(&map);
    vec![format!(
        "{map}: reading the overlays again; `lb overlay` shows them once the graph is back"
    )]
}

/// Forgets the map's kept graphs and makes the graph again.
fn nav_regen(rt: &mut Runtime) -> Vec<String> {
    let Some(map) = rt.map.as_ref().map(|m| m.name.clone()) else {
        return vec!["no map".into()];
    };
    if rt.nav_loader.is_some() {
        return vec![format!("{map}: the graph is being loaded already")];
    }
    let dir = rt.init.install_dir.join("nav").join(&map);
    let removed = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "lbnav"))
        .filter(|e| std::fs::remove_file(e.path()).is_ok())
        .count();
    rt.nav_status = format!("remaking the graph for {map}");
    rt.start_nav_load(&map);
    vec![format!(
        "{map}: {removed} kept graph(s) removed, making it again ({}); bots keep the current graph until then",
        rt.config.nav.source.name()
    )]
}

fn nav(rt: &Runtime) -> Vec<String> {
    let mut out = vec![format!("graph: {}", rt.nav_status)];
    let Some(g) = rt.graph.as_deref() else { return out };
    out.push(format!("live check: {}", rt.live_check.progress(g.probes.len())));
    let off: Vec<String> = rt
        .link_health
        .disabled_links()
        .map(|(a, b)| format!("{a}->{b}"))
        .collect();
    if !off.is_empty() {
        out.push(format!("switched off for everyone: {}", off.join(" ")));
    }
    let now = rt.now.secs();
    for b in &rt.bots {
        let blocked = b.nav.known.active(now).count();
        let failure = b
            .nav
            .last_failure
            .map(|f| {
                format!(
                    ", last failure {}->{} {} {:.0} s ago",
                    f.from,
                    f.to,
                    f.reason.as_str(),
                    now - f.at
                )
            })
            .unwrap_or_default();
        let line = match b.nav.follower.as_ref() {
            Some(f) => {
                let target = f.target().map(|t| t.to_string()).unwrap_or_else(|| "-".into());
                let left = f.remaining().len();
                let goal = g.node(f.goal()).origin;
                format!(
                    "  {:<20} {:<16} goal {} ({:.0} u away), next {target}, {left} nodes left, {blocked} links blocked{failure}",
                    b.persona.name,
                    f.phase(),
                    f.goal(),
                    goal.distance(b.self_state.body.origin)
                )
            }
            None => format!(
                "  {:<20} {}, {blocked} links blocked{failure}",
                b.persona.name,
                b.state.as_str()
            ),
        };
        out.push(line);
    }
    out
}

fn nav_test(rt: &mut Runtime, args: &[&str]) -> Vec<String> {
    use crate::nav_test::{NavTest, pick_links};
    use lb_nav::LinkKind;
    let Some(g) = rt.graph.clone() else {
        return vec![format!("no navigation graph: {}", rt.nav_status)];
    };
    match args.first().copied() {
        None => {
            let mut out = Vec::new();
            for b in rt.bots.iter().filter(|b| b.nav_test.is_some()) {
                out.push(format!("{}:", b.persona.name));
                out.extend(b.nav_test.as_ref().map(|t| t.report()).unwrap_or_default());
            }
            if out.is_empty() {
                out.push("no course running; lb nav test <kind|all> [count] [name|#userid]".into());
            }
            out
        }
        Some("stop") => {
            for b in &mut rt.bots {
                b.nav_test = None;
            }
            vec!["course stopped".into()]
        }
        Some("link") => {
            let nums: Vec<Option<u32>> = args[1..].iter().map(|a| a.parse().ok()).collect();
            if nums.is_empty() || !nums.len().is_multiple_of(2) || nums.iter().any(Option::is_none) {
                return vec!["usage: lb nav test link <from> <to> [<from> <to> ...]".into()];
            }
            let links: Vec<(u32, u32)> = nums.chunks(2).map(|p| (p[0].unwrap_or(0), p[1].unwrap_or(0))).collect();
            if let Some((a, b)) = links
                .iter()
                .find(|(a, b)| (*a as usize) >= g.len() || g.find_link(*a, *b).is_none())
            {
                return vec![format!("no link {a} -> {b}")];
            }
            let Some(&i) = find_bots(rt, None)
                .iter()
                .find(|&&i| rt.bots[i].state == BotState::Alive)
            else {
                return vec!["no live bot to run the course".into()];
            };
            let n = links.len();
            let bot = &mut rt.bots[i];
            bot.nav_test = Some(NavTest::new(links));
            vec![format!(
                "{} runs {n} links; `lb nav test` shows the results",
                bot.persona.name
            )]
        }
        Some(kind) => {
            let kind = match kind {
                "all" => None,
                k => match LinkKind::ALL.into_iter().find(|x| x.as_str() == k) {
                    Some(k) => Some(k),
                    None => return vec![format!("unknown link kind `{k}`")],
                },
            };
            let count: usize = args.get(1).and_then(|c| c.parse().ok()).unwrap_or(20);
            let who = (args.len() > 2).then(|| args[2..].join(" "));
            let Some(&i) = find_bots(rt, who.as_deref())
                .iter()
                .find(|&&i| rt.bots[i].state == BotState::Alive)
            else {
                return vec!["no live bot to run the course".into()];
            };
            let links = pick_links(&g, kind, count);
            if links.is_empty() {
                return vec!["no such links on this map".into()];
            }
            let n = links.len();
            let bot = &mut rt.bots[i];
            bot.nav_test = Some(NavTest::new(links));
            vec![format!(
                "{} runs {n} links; `lb nav test` shows the results",
                bot.persona.name
            )]
        }
    }
}

fn vision(rt: &Runtime, args: &[&str]) -> Vec<String> {
    let target = (!args.is_empty()).then(|| args.join(" "));
    let indices = find_bots(rt, target.as_deref());
    if indices.is_empty() {
        return vec!["no such bot".into()];
    }
    let now = rt.now;
    let name = |slot: u8| {
        rt.clients
            .get(slot)
            .map(|c| c.name.clone())
            .unwrap_or_else(|| format!("slot {slot}"))
    };
    let mut out = vec![format!(
        "visibility sets: {}",
        match rt.vis.as_deref() {
            Some(v) => format!("{} leaves", v.visleafs()),
            None => "not loaded (everything passes the PVS test)".into(),
        }
    )];
    for i in indices {
        let b = &rt.bots[i];
        let v = &b.brain.perception.vision.stats;
        let h = &b.brain.perception.hearing.stats;
        let first = v.recognitions - v.reacquisitions;
        let mean = if first > 0 { v.latency_sum / first as f64 } else { 0.0 };
        out.push(format!(
            "{} ({}, skill {}, {}): looking at {}",
            b.persona.name,
            b.persona.style.as_str(),
            b.persona.skill,
            b.state.as_str(),
            b.attention.map(|a| a.reason.as_str()).unwrap_or("path"),
        ));
        out.push(format!(
            "  vision: {} ticks, {:.1} traces per tick, {} skipped; recognized {first} (mean {mean:.2} s, max {:.2} s), found again {}",
            v.ticks,
            if v.ticks > 0 { v.traces as f64 / v.ticks as f64 } else { 0.0 },
            v.skipped,
            v.latency_max,
            v.reacquisitions
        ));
        out.push(format!("  hearing: {} heard, {} too quiet", h.heard, h.too_quiet));
        for c in &b.brain.perception.vision.contacts {
            out.push(if c.recognized {
                format!(
                    "  sees {:<20} {:>5.0} u, {:.0}% visible{}",
                    name(c.who.slot),
                    c.distance,
                    c.visibility * 100.0,
                    if c.lost_at.is_some() { ", just out of sight" } else { "" }
                )
            } else {
                format!(
                    "  notices {:<17} {:>5.0} u, evidence {:.2} of 1 (delay {:.2} s)",
                    name(c.who.slot),
                    c.distance,
                    c.evidence,
                    c.delay
                )
            });
        }
        for t in &b.brain.beliefs.tracks {
            out.push(format!(
                "  track {:<19} {:<9} {:>4.1} s ago, {:>5.0} u away, sigma {:>4.0} u, weapon {}{}",
                name(t.who.slot),
                t.state.as_str(),
                now.since(t.last_seen),
                t.pos.distance(b.self_state.body.origin),
                t.sigma,
                t.traits.weapon.map(|w| w.classname()).unwrap_or("-"),
                t.last_heard
                    .map(|h| format!(", heard {:.1} s ago", now.since(h)))
                    .unwrap_or_default()
            ));
        }
        for hyp in &b.brain.beliefs.hypotheses {
            let what = match hyp.kind {
                lb_knowledge::HypothesisKind::Sound(k) => k.as_str(),
                lb_knowledge::HypothesisKind::Damage => "damage",
                lb_knowledge::HypothesisKind::Cue => "glimpse",
            };
            out.push(format!(
                "  {what:<9} {:>4.1} s ago, bearing {:>4.0} deg +-{:.0}{}{}",
                now.since(hyp.t),
                hyp.bearing,
                hyp.bearing_sigma,
                hyp.pos
                    .map(|p| format!(", {:.0} u", p.distance(b.self_state.body.origin)))
                    .unwrap_or_default(),
                hyp.weapon.map(|w| format!(", {}", w.classname())).unwrap_or_default()
            ));
        }
    }
    out
}

fn goal_text(rt: &Runtime, b: &crate::manager::Bot, kind: lb_decision::GoalKind) -> String {
    let name = |slot: u8| rt.clients.get(slot).map(|c| c.name.clone()).unwrap_or_default();
    match kind {
        lb_decision::GoalKind::Engage(k) => format!("engage {}", name(k.slot)),
        lb_decision::GoalKind::Hunt(k) => format!("hunt {}", name(k.slot)),
        lb_decision::GoalKind::CollectItem(i) => format!(
            "collect {}",
            b.brain
                .items
                .as_ref()
                .and_then(|items| items.spots.get(i))
                .map(|s| s.kind.as_str())
                .unwrap_or_default()
        ),
        lb_decision::GoalKind::UseCharger(i) => {
            let suit = b.brain.chargers.as_ref().and_then(|c| c.spots.get(i)).map(|c| c.suit);
            format!("charger ({})", if suit == Some(true) { "suit" } else { "health" })
        }
        k => k.as_str().to_string(),
    }
}

fn brain(rt: &Runtime, args: &[&str]) -> Vec<String> {
    let target = (!args.is_empty()).then(|| args.join(" "));
    let indices = find_bots(rt, target.as_deref());
    if indices.is_empty() {
        return vec!["no such bot".into()];
    }
    let now = rt.now;
    let name = |slot: u8| rt.clients.get(slot).map(|c| c.name.clone()).unwrap_or_default();
    let mut out = Vec::new();
    for i in indices {
        let b = &rt.bots[i];
        let m = &b.brain.mind;
        let goal = match m.decider.current {
            Some(a) => format!(
                "{} (rank {}, weight {:.2}, for {:.1} s{})",
                goal_text(rt, b, a.goal.kind),
                a.goal.rank,
                a.goal.weight,
                now.since(a.since),
                if now < a.hold_until { ", held" } else { "" }
            ),
            None => "-".into(),
        };
        out.push(format!(
            "{} ({} {}, {}, hp {}): {goal}",
            b.persona.name,
            b.persona.style.as_str(),
            b.persona.skill,
            b.state.as_str(),
            b.self_state.body.health as i32
        ));
        let candidates: Vec<String> = m
            .decider
            .last
            .iter()
            .map(|g| format!("{} {}/{:.2}", goal_text(rt, b, g.kind), g.rank, g.weight))
            .collect();
        out.push(format!("  candidates: {}", candidates.join(" | ")));
        let target = m.target.map(|k| {
            let track = b.brain.beliefs.track(k);
            format!(
                "{} {}{}",
                name(k.slot),
                track.map(|t| t.state.as_str()).unwrap_or("-"),
                track
                    .map(|t| format!(", {:.0} u", t.pos.distance(b.self_state.body.origin)))
                    .unwrap_or_default()
            )
        });
        out.push(format!(
            "  target: {}; weapon: {:?} (holding {}){}{}",
            target.unwrap_or_else(|| "none".into()),
            m.choice,
            b.self_state.current_weapon.get().map(|w| w.classname()).unwrap_or("-"),
            if m.aim.aims_at_head() { ", aims at the head" } else { "" },
            if m.firing { ", firing" } else { "" }
        ));
        if let Some(p) = &b.self_state.prediction {
            let w = p.current;
            let state = w.and_then(|w| p.weapons.get(w as usize).copied().flatten());
            out.push(format!(
                "  client prediction: {} {}{}",
                w.map(|w| w.classname()).unwrap_or("-"),
                match state {
                    Some(s) if s.reloading => format!("reloading, clip {}", s.clip),
                    Some(s) if s.next_primary > 0.0 || p.next_attack > 0.0 => {
                        format!("ready in {:.2} s, clip {}", s.next_primary.max(p.next_attack), s.clip)
                    }
                    Some(s) => format!("ready, clip {}", s.clip),
                    None => "no data".into(),
                },
                state
                    .filter(|s| s.in_attack != 0)
                    .map(|s| format!(", attack state {}", s.in_attack))
                    .unwrap_or_default()
            ));
        }
        let arms = &m.arms;
        let st = &arms.stats;
        out.push(format!(
            "  arms: {}; thrown {} grenades, {} satchels, {} snarks; {} m203, {} mines laid, {} detonations, {} mines \
             shot, gauss {} fired {} dumped, {} dodges, {} failed{}; explosives known: {} own satchels, {} mines, {} \
             in flight",
            arms.describe(now),
            st.grenades,
            st.satchels,
            st.snarks,
            st.lobs,
            st.mines,
            st.detonations,
            st.mine_shots,
            arms.gauss.fired,
            arms.gauss.dumped,
            st.dodges,
            st.failed,
            arms.last_failure.map(|f| format!(" (last: {f})")).unwrap_or_default(),
            b.brain.explosives.charges.len(),
            b.brain.explosives.mines.len(),
            b.brain.explosives.flying.len(),
        ));
        if !st.failures.is_empty() {
            let f: Vec<String> = st.failures.iter().map(|(p, w, n)| format!("{p}: {w} ×{n}")).collect();
            out.push(format!("  failures: {}", f.join("; ")));
        }
        let i = &b.brain.intents;
        let owner = |p: Option<lb_motor::Prio>| p.map(|p| format!("{p:?}")).unwrap_or_else(|| "-".into());
        out.push(format!(
            "  channels: look {}, move {}, stance {}, weapon {}",
            owner(i.look.map(|x| x.0)),
            owner(i.movement.map(|x| x.0)),
            owner(i.stance.map(|x| x.0)),
            owner(i.weapon.as_ref().map(|x| x.0)),
        ));
        let r = &m.reactions;
        if r.count > 0 {
            out.push(format!(
                "  reactions: {} contacts answered; first glimpse to first shot mean {:.2} s, median {:.2} s, worst {:.2} s; recognition to shot mean {:.2} s",
                r.count,
                r.evidence_to_shot / r.count as f64,
                r.median().unwrap_or(0.0),
                r.worst,
                r.recognition_to_shot / r.count as f64
            ));
        }
    }
    out
}

fn roster(rt: &Runtime, args: &[&str]) -> Vec<String> {
    let all = args.first() == Some(&"all");
    let mut out = vec![format!("roster: {}", rt.roster_summary())];
    for p in &rt.roster.problems {
        out.push(format!("problem: {p}"));
    }
    if rt.roster.shadowed > 0 {
        out.push(format!(
            "{} entries of {} are hidden by profiles with the same name",
            rt.roster.shadowed,
            rt.roster.generated_path().display()
        ));
    }
    let playing = |name: &str| {
        rt.bots
            .iter()
            .any(|b| b.is_active() && b.persona.name.eq_ignore_ascii_case(name))
    };
    let mut rows: Vec<(u8, &lb_styles::Persona)> = rt
        .roster
        .iter()
        .map(|p| {
            let rank = if playing(&p.name) {
                0
            } else if rt.filter.admits(p) {
                1
            } else {
                2
            };
            (rank, p.as_ref())
        })
        .filter(|(rank, _)| all || *rank < 2)
        .collect();
    rows.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| a.1.name.to_lowercase().cmp(&b.1.name.to_lowercase()))
    });
    out.push(format!(
        "  {:<22} {:<10} {:>5} {:>6}  {:<9}  {}",
        "name", "style", "skill", "weight", "source", "state"
    ));
    for (rank, p) in &rows {
        let state = match rank {
            0 => "playing",
            1 if p.weight <= 0.0 => "on request",
            1 => "available",
            _ => "filtered out",
        };
        out.push(format!(
            "  {:<22} {:<10} {:>5} {:>6.1}  {:<9}  {state}",
            p.name,
            p.style.as_str(),
            p.skill,
            p.weight,
            p.source.short()
        ));
    }
    if !all {
        let hidden = rt.roster.len() - rows.len();
        if hidden > 0 {
            out.push(format!("{hidden} more outside the filter: `lb roster all`"));
        }
    }
    out
}

fn profile(rt: &Runtime, args: &[&str]) -> Vec<String> {
    if args.is_empty() {
        return vec!["usage: lb profile <name>".into()];
    }
    let name = args.join(" ");
    let Some(p) = rt.roster.get(&name) else {
        return vec![format!("no personality named `{name}`; see `lb roster all`")];
    };
    let k = p.skill_params(&rt.presets);
    let opt = |v: Option<f32>, unit: &str| v.map(|v| format!("{v:.2}{unit}")).unwrap_or_else(|| "never".into());
    let overrides = lb_config::yaml::to_string(&p.overrides).unwrap_or_default();
    vec![
        format!("{}: {}, skill {} ({})", p.name, p.style, p.skill, p.source.describe()),
        format!("  look: model {}, colors {} / {}", p.model, p.colors[0], p.colors[1]),
        format!(
            "  traits: aggression {:.2}, fear {:.2}; weight {}; seed {:#x}",
            p.aggression, p.fear, p.weight, p.seed
        ),
        format!(
            "  weapons: {}; tags: {}",
            if p.weapons.is_empty() {
                "style default".into()
            } else {
                p.weapons.join(", ")
            },
            if p.tags.is_empty() {
                "-".into()
            } else {
                p.tags.join(", ")
            }
        ),
        format!(
            "  skill: recognition {:.2}-{:.2} s, aim latency {:.2} s, {:?} aim, headshot {:.0}%, turn {:.0} deg/s",
            k.recognition_delay[0],
            k.recognition_delay[1],
            k.aim_latency,
            k.aim_model,
            k.headshot * 100.0,
            k.turn_speed
        ),
        format!(
            "         hearing {:.3} (bearing {:.0} deg), memory {:.0} s, dodge jump {}, tricks {}, bhop {}",
            k.hearing_threshold,
            k.sound_bearing_sigma,
            k.track_forget,
            opt(k.dodge_hop_cooldown, " s"),
            if k.tricks { "yes" } else { "no" },
            opt(k.bhop_speed, "x"),
        ),
        format!(
            "  overrides: {}",
            if p.overrides.is_empty() {
                "none".into()
            } else {
                overrides.trim().replace('\n', ", ")
            }
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
            .position(|b| b.persona.name.eq_ignore_ascii_case(name))
            .into_iter()
            .collect(),
    }
}

fn set_quota(rt: &mut Runtime, host: &mut dyn Host, count: u32) {
    rt.config.quota.count = count.min(32);
    rt.cvars.set(host, Cv::Quota, &rt.config.quota.count.to_string());
}

fn add(rt: &mut Runtime, host: &mut dyn Host, args: &[&str]) -> Vec<String> {
    if let Some(first) = args.first()
        && first.parse::<u32>().is_err()
    {
        let name = args.join(" ");
        let Some(p) = rt.roster.get(&name) else {
            return vec![format!("no personality named `{name}`; see `lb roster all`")];
        };
        if rt
            .bots
            .iter()
            .any(|b| b.is_active() && b.persona.name.eq_ignore_ascii_case(&p.name))
        {
            return vec![format!("{} is already playing", p.name)];
        }
        rt.requested.push(p.name.clone());
        let count = rt.config.quota.count + 1;
        set_quota(rt, host, count);
        rt.creation.next_at = Some(rt.now);
        let note = if rt.filter.admits(&p) {
            ""
        } else {
            " (outside the roster filter, joins anyway)"
        };
        return vec![format!(
            "{} joins next{note}; quota is now {}",
            p.name, rt.config.quota.count
        )];
    }
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
    let names: Vec<String> = indices.iter().map(|&i| rt.bots[i].persona.name.clone()).collect();
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

fn config(rt: &mut Runtime, host: &mut dyn Host, args: &[&str]) -> Vec<String> {
    match args.first().copied() {
        Some("reload") => rt.reload(host),
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
        let (s, c) = lb_core::dmath::sin_cos(yaw.to_radians());
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
        Some("tracedump") => {
            let n: usize = args
                .get(1)
                .and_then(|a| a.parse().ok())
                .unwrap_or(2000)
                .clamp(1, 100_000);
            match trace_dump(rt, host, n) {
                Ok((path, written)) => vec![format!("{written} engine traces written to {}", path.display())],
                Err(e) => vec![format!("tracedump failed: {e}")],
            }
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
        _ => vec![
            "usage: lb debug panic <#userid|all> | stall <ms> | stalecmd | capture <s> | tracedump [n] | safemode"
                .into(),
        ],
    }
}

/// Random engine traces from the bots' positions, for comparing the offline BSP tracer with the engine
/// (`lb-cli nav tracecheck <map.bsp> <dump>`). One JSON object per line.
fn trace_dump(rt: &mut Runtime, host: &mut dyn Host, n: usize) -> std::io::Result<(std::path::PathBuf, usize)> {
    use std::io::Write;
    let origins: Vec<lb_core::Vec3> = rt
        .bots
        .iter()
        .filter(|b| b.state == BotState::Alive)
        .map(|b| b.self_state.body.origin)
        .collect();
    if origins.is_empty() {
        return Err(std::io::Error::other("no living bots to trace from"));
    }
    let map = rt.map.as_ref().map(|m| m.name.clone()).unwrap_or_else(|| "none".into());
    let dir = rt.init.install_dir.join("logs");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("tracedump-{map}.jsonl"));
    let mut out = std::io::BufWriter::new(std::fs::File::create(&path)?);
    for i in 0..n {
        let start = origins[i % origins.len()] + lb_core::Vec3::new(0.0, 0.0, rt.rng.range_f32(-20.0, 30.0));
        let yaw = rt.rng.range_f32(0.0, std::f32::consts::TAU);
        let pitch = rt.rng.range_f32(-1.2, 1.2);
        let len = rt.rng.range_f32(16.0, 1500.0);
        let (m, n) = (lb_core::dmath::sin_cos(yaw), lb_core::dmath::sin_cos(pitch));
        let dir = lb_core::Vec3::new(m.1 * n.1, m.0 * n.1, n.0);
        let end = start + dir * len;
        let hull = rt.rng.range_i32(0, 3) as u8;
        let kind = if hull == 0 {
            TraceKind::Line
        } else {
            TraceKind::Hull(hull)
        };
        let tr = host.trace(&TraceRequest {
            start,
            end,
            kind,
            ignore_monsters: true,
            ignore_glass: false,
            ignore: None,
        });
        let v = |v: lb_core::Vec3| [v.x, v.y, v.z];
        let line = serde_json::json!({
            "start": v(start), "end": v(end), "hull": hull,
            "fraction": tr.fraction, "endpos": v(tr.end_pos), "normal": v(tr.plane_normal),
            "start_solid": tr.start_solid, "all_solid": tr.all_solid, "hit": tr.hit.index,
        });
        writeln!(out, "{line}")?;
    }
    out.flush()?;
    Ok((path, n))
}

/// Weapon names for `lb weapons`: classnames without `weapon_` and the usual aliases.
fn weapon_by_name(name: &str) -> Option<lb_game::weapons::WeaponId> {
    use lb_game::weapons::WeaponId;
    match name.to_ascii_lowercase().as_str() {
        "357" | "python" => Some(WeaponId::Python),
        "mp5" | "9mmar" => Some(WeaponId::Mp5),
        "glock" | "9mmhandgun" => Some(WeaponId::Glock),
        "hornet" | "hornetgun" => Some(WeaponId::Hornetgun),
        "grenade" | "handgrenade" => Some(WeaponId::HandGrenade),
        other => WeaponId::from_classname(other),
    }
}

fn weapons(rt: &mut Runtime, args: &[&str]) -> Vec<String> {
    use lb_game::weapons::WeaponId;
    let describe = |rt: &Runtime| {
        let allowed: Vec<&str> = WeaponId::ALL
            .into_iter()
            .filter(|w| rt.weapons_allowed & w.bit() != 0)
            .map(|w| &w.classname()["weapon_".len()..])
            .collect();
        let given: Vec<&str> = rt
            .weapons_give
            .iter()
            .map(|w| &w.classname()["weapon_".len()..])
            .collect();
        let allowed = if rt.weapons_allowed == u32::MAX {
            "all".to_string()
        } else {
            allowed.join(", ")
        };
        format!(
            "bots may use: {allowed}; given on spawn: {}",
            if given.is_empty() {
                "nothing".into()
            } else {
                given.join(", ")
            }
        )
    };
    let Some(first) = args.first().copied() else {
        return vec![describe(rt)];
    };
    let give = args.last().is_some_and(|a| a.eq_ignore_ascii_case("give"));
    let names: Vec<&str> = args
        .iter()
        .copied()
        .filter(|a| !a.eq_ignore_ascii_case("give"))
        .collect();
    match first {
        "all" | "standard" => {
            rt.weapons_allowed = u32::MAX;
            rt.weapons_give.clear();
        }
        "melee" => {
            rt.weapons_allowed = WeaponId::Crowbar.bit();
            rt.weapons_give.clear();
        }
        _ => {
            let mut list = Vec::new();
            for n in &names {
                match weapon_by_name(n) {
                    Some(w) => list.push(w),
                    None => return vec![format!("unknown weapon `{n}`")],
                }
            }
            rt.weapons_allowed = list.iter().fold(WeaponId::Crowbar.bit(), |m, w| m | w.bit());
            rt.weapons_give = if give { list } else { Vec::new() };
        }
    }
    vec![describe(rt)]
}

fn selftest(rt: &mut Runtime, host: &mut dyn Host, args: &[&str]) -> Vec<String> {
    let target = (!args.is_empty()).then(|| args.join(" "));
    let Some(i) = find_bots(rt, target.as_deref())
        .into_iter()
        .find(|&i| rt.bots[i].state == BotState::Alive)
    else {
        return vec!["no live bot to run the self-test".into()];
    };
    let yaw = open_yaw(host, rt.bots[i].self_state.body.origin);
    let (now, dll) = (rt.now, rt.game.dll);
    if rt.freeze_before_selftest.is_none() {
        rt.freeze_before_selftest = Some(rt.freeze);
    }
    rt.freeze = true;
    let b = &mut rt.bots[i];
    b.selftest = Some(crate::selftest::SelfTest::new(now, yaw, dll));
    vec![format!(
        "self-test started on {} (profile {}); results go to the console and the log",
        b.persona.name,
        dll.kind.as_str()
    )]
}
