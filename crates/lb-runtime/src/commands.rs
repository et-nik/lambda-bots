//! `lb` console commands (server console, rcon, authorized clients, telemetry channel).

use lb_config::main_config::QuotaMode;
use lb_core::math::normalize_angle;
use lb_game::weapons::WeaponId;
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
        "map [spots|mines|lanes|danger]",
        "the map as bots know it: chokepoints, spots to hold, tripmine spots, lanes for trails, where bots get hurt",
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
        "gg",
        "GunGame as the bots see it: every player's level from the scoreboard, the leader, what each bot's level gave it",
    ),
    (
        "watch [name|#userid] [off] | watch off",
        "write what a player does to the log: moves, view and weapon 20 times a second, shots, mines, the room around",
    ),
    (
        "gg mines <name|#userid|all> [off] | mines off",
        "bots play GunGame's tripmine level on a server with no GunGame: mines handed back as they go (needs sv_cheats 1)",
    ),
    (
        "weapons [all|melee|<weapon>...] [give]",
        "weapons bots may use; with give every bot gets them on spawn (needs sv_cheats 1)",
    ),
    (
        "items [none|<item>...]",
        "items every bot gets on spawn, e.g. longjump (needs sv_cheats 1)",
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
        "give <name|#userid|all> <item>...",
        "items for bots now: gauss, uranium, longjump, health, armor, a weapon or a classname (needs sv_cheats 1)",
    ),
    (
        "do <name|#userid|all> go <x y z|node N|@me|@aim|place P> [radius R] [timeout T] [tricks ...] | stop",
        "send bots to a spot, finding a jump, long jump or gauss boost where the graph has no way; others stand still",
    ),
    (
        "test [list] | add <id> <spot> [from <spot>] [give ...] | remove <id> | run [<id>...] | stop | results",
        "the map's tests (maps/<map>/tests.yaml): a bot given items goes from a start to a goal; reports in logs/tests",
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
        "map" => map_knowledge(rt, rest),
        "vision" => vision(rt, rest),
        "brain" => brain(rt, rest),
        "profile" => profile(rt, rest),
        "status" => status(rt),
        "gg" => match rest {
            ["mines", args @ ..] => gungame_mines(rt, host, args),
            _ => gungame(rt),
        },
        "weapons" => weapons(rt, rest),
        "items" => items(rt, rest),
        "selftest" => selftest(rt, host, rest),
        "stats" => match rest.first().copied() {
            Some("reset") => {
                rt.arms_stats.reset(rt.now.secs());
                rt.tricks_base = rt.bots.iter().map(|b| (b.id, bot_tricks(b))).collect();
                for b in &mut rt.bots {
                    b.stall.counts = Default::default();
                }
                vec!["weapon statistics reset".into()]
            }
            _ => {
                let mut out = rt.arms_stats.report(rt.now.secs(), &rt.game.rules.damages);
                out.push(trick_totals(rt).line());
                let mut stalls = crate::stall::Stalls::default();
                for b in &rt.bots {
                    stalls.add(&b.stall.counts);
                }
                out.extend(stall_lines(&stalls, "  "));
                out
            }
        },
        "perf" => perf(rt, rest),
        "compat" => {
            let mut out: Vec<String> = rt.compat.to_yaml().lines().map(String::from).collect();
            out.push(format!("satchel_buttons: {}", rt.satchel_buttons()));
            out
        }
        "add" => add(rt, host, rest),
        "kick" => kick(rt, host, rest),
        "kill" => kill(rt, rest),
        "quota" => quota(rt, host, rest),
        "config" => config(rt, host, rest),
        "test" => test(rt, host, rest),
        "give" => crate::orders::give(rt, host, rest),
        "do" => crate::orders::command(rt, host, rest),
        "debug" => debug(rt, host, rest),
        "record" => record(rt, rest),
        "watch" => watch(rt, rest),
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

/// A bot's GunGame: its level (from 1, as the plugin shows it), what the level gave it, the warmup and the leader.
fn gungame_text(rt: &Runtime, board: &lb_game::gungame::Board, b: &crate::manager::Bot) -> String {
    use lb_game::gungame::{GunGame, Kit};
    let g = GunGame::new(board, b.id.slot, b.self_state.body.weapons_mask);
    let kit = match g.kit {
        Kit::Gun(w) | Kit::Throwable(w) => format!("{} {}", g.kit.as_str(), w.classname()),
        Kit::Other => {
            let carried: Vec<&str> = lb_game::weapons::weapons_in_mask(b.self_state.body.weapons_mask)
                .map(|w| w.classname())
                .collect();
            if carried.is_empty() {
                "no weapons".to_string()
            } else {
                format!("other: {}", carried.join(" "))
            }
        }
        k => k.as_str().to_string(),
    };
    let place = if g.warmup {
        ", warmup".to_string()
    } else if g.leads() {
        ", leads".to_string()
    } else {
        let leader = board
            .leader
            .and_then(|slot| rt.clients.get(slot))
            .map(|c| c.name.clone())
            .unwrap_or_default();
        format!(", {} behind {leader}", g.behind())
    };
    format!(
        "level {} ({kit}){place}{}",
        g.level + 1,
        if g.descore() { ", descore" } else { "" }
    )
}

fn gungame(rt: &Runtime) -> Vec<String> {
    let Some(board) = rt.gungame_board() else {
        let drilled: Vec<&str> = rt
            .bots
            .iter()
            .filter(|b| b.drill.is_some())
            .map(|b| b.persona.name.as_str())
            .collect();
        let mut out = vec![format!(
            "not a GunGame match (mode {:?}); lb_gungame auto|on|off",
            rt.game.mode
        )];
        if !drilled.is_empty() {
            out.push(format!("  on the tripmine level (emulated): {}", drilled.join(", ")));
        }
        return out;
    };
    let name = |slot: u8| rt.clients.get(slot).map(|c| c.name.clone()).unwrap_or_default();
    let mut out = vec![format!(
        "{:?}: descore {}, leader {} (level {}), top level {}",
        rt.game.mode.unwrap_or(lb_game::mode::GameModeKind::Ffa),
        if board.descore { "on" } else { "off" },
        board.leader.map(name).unwrap_or_else(|| "-".into()),
        board.leader.and_then(|s| board.level(s)).map_or(0, |l| l + 1),
        board.top() + 1
    )];
    let mut players: Vec<(u8, i32, i32)> = board
        .levels
        .iter()
        .enumerate()
        .filter(|(_, l)| l.is_some())
        .filter_map(|(slot, _)| {
            let e = rt.game.scoreboard.entries.get(slot)?;
            Some((slot as u8, e.frags, e.deaths))
        })
        .collect();
    players.sort_by_key(|&(slot, frags, deaths)| (-frags, deaths, slot));
    for (slot, frags, deaths) in players {
        let bot = rt
            .bots
            .iter()
            .find(|b| b.id.slot == slot)
            .map(|b| format!("  bot: {}", gungame_text(rt, &board, b)))
            .unwrap_or_default();
        out.push(format!(
            "  level {:>2}  frags {frags:>5}  deaths {deaths:>3}  {}{bot}",
            board.level(slot).unwrap_or(0) + 1,
            name(slot)
        ));
    }
    out
}

/// `lb gg mines`: bots play GunGame's tripmine level on a server with no GunGame (the stand): tripmines and a glock
/// only, the mines handed back as they go off, the way the plugin does.
fn gungame_mines(rt: &mut Runtime, host: &mut dyn Host, args: &[&str]) -> Vec<String> {
    let (who, on) = match args {
        [] => return vec!["usage: lb gg mines <name|#userid|all> [off] | lb gg mines off".into()],
        ["off"] => ("all".to_string(), false),
        [name @ .., "off"] => (name.join(" "), false),
        name => (name.join(" "), true),
    };
    let bots = find_bots(rt, Some(&who));
    if bots.is_empty() {
        return vec![format!("no bot `{who}`")];
    }
    if on {
        if rt.gungame_board().is_some() {
            return vec!["a GunGame match is on: its levels are the plugin's".into()];
        }
        if !crate::orders::cheats(rt, host) {
            return vec![crate::orders::CHEATS_OFF.into()];
        }
    }
    for &i in &bots {
        rt.bots[i].drill = on.then_some(lb_game::gungame::Kit::Mines);
    }
    let names: Vec<&str> = bots.iter().map(|&i| rt.bots[i].persona.name.as_str()).collect();
    vec![format!(
        "{}: {}",
        names.join(", "),
        if on {
            "the tripmine level: mines and a glock, the mines handed back as they go off"
        } else {
            "back to the game's own weapons"
        }
    )]
}

/// `lb watch`: a player's moves, view, weapon, shots and mines go to the log (to study how people play).
fn watch(rt: &mut Runtime, args: &[&str]) -> Vec<String> {
    let name = |rt: &Runtime, slot: u8| rt.clients.get(slot).map(|c| c.name.clone()).unwrap_or_default();
    let (who, on) = match args {
        [] => {
            if rt.watched.is_empty() {
                return vec!["nobody is watched".into()];
            }
            let names: Vec<String> = rt
                .watched
                .iter()
                .map(|w| format!("{} (#{})", name(rt, w.slot), w.userid))
                .collect();
            return vec![format!("watched: {}", names.join(", "))];
        }
        ["off"] => {
            rt.watched.clear();
            return vec!["nobody is watched now".into()];
        }
        [name @ .., "off"] => (name.join(" "), false),
        name => (name.join(" "), true),
    };
    let who = who.as_str();
    let players: Vec<(u8, i32)> = rt
        .clients
        .slots
        .iter()
        .enumerate()
        .filter(|(_, c)| c.connected)
        .map(|(slot, c)| (slot as u8, c.userid))
        .collect();
    let exact: Vec<(u8, i32)> = players
        .iter()
        .copied()
        .filter(|&(slot, userid)| {
            who.strip_prefix('#').and_then(|u| u.parse().ok()) == Some(userid)
                || name(rt, slot).eq_ignore_ascii_case(who)
        })
        .collect();
    let found = if exact.is_empty() {
        let low = who.to_lowercase();
        players
            .into_iter()
            .filter(|&(slot, _)| name(rt, slot).to_lowercase().contains(&low))
            .collect()
    } else {
        exact
    };
    let [(slot, userid)] = found[..] else {
        return vec![format!("`{who}`: {} players match", found.len())];
    };
    rt.watched.retain(|w| w.slot != slot);
    if on {
        rt.watched.push(crate::watch::Watched::new(slot, userid));
    }
    vec![format!(
        "{} (#{userid}): {}",
        name(rt, slot),
        if on {
            "watched, see `watch #` lines in the log"
        } else {
            "no longer watched"
        }
    )]
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
    let edited = if lb_navgen::mapload::remove_edited(&rt.init.install_dir, &map) {
        format!(" and the map editor's one ({})", lb_navgen::mapload::EDITED)
    } else {
        String::new()
    };
    rt.nav_status = format!("remaking the graph for {map}");
    rt.start_nav_load(&map);
    vec![format!(
        "{map}: {removed} kept graph(s){edited} removed, making it again ({}); bots keep the current graph until then",
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
        lb_decision::GoalKind::ControlItem(i) => format!(
            "control {}",
            b.brain
                .items
                .as_ref()
                .and_then(|items| items.spots.get(i))
                .map(|s| s.kind.as_str())
                .unwrap_or_default()
        ),
        lb_decision::GoalKind::Investigate(id) => format!(
            "investigate {}",
            b.brain
                .beliefs
                .hypothesis(id)
                .map(|h| match h.kind {
                    lb_knowledge::HypothesisKind::Sound(k) => k.as_str(),
                    lb_knowledge::HypothesisKind::Cue => "glimpse",
                    lb_knowledge::HypothesisKind::Damage => "damage",
                })
                .unwrap_or("-")
        ),
        lb_decision::GoalKind::Camp(i) => format!(
            "camp {}",
            rt.tactics
                .as_ref()
                .and_then(|t| t.camps.get(i as usize))
                .map(|c| format!("{} node {}", c.kind.as_str(), c.node))
                .unwrap_or_default()
        ),
        lb_decision::GoalKind::PlantTrap(t) => match t {
            lb_decision::Trap::Mine(i) => format!("trap: tripmine at spot {i}"),
            lb_decision::Trap::Satchels(i) => format!("trap: satchels from spot {i}"),
            lb_decision::Trap::Loose => "trap: satchels where an enemy is expected".into(),
            lb_decision::Trap::Trail => "trap: a trail of mines".into(),
        },
        k => k.as_str().to_string(),
    }
}

fn map_knowledge(rt: &Runtime, args: &[&str]) -> Vec<String> {
    let Some(t) = rt.tactics.as_deref() else {
        return vec!["no tactics: the map's graph is not loaded".into()];
    };
    let at = |p: lb_core::Vec3| format!("{:.0} {:.0} {:.0}", p.x, p.y, p.z);
    let n = t.origins.len();
    let mut out = vec![format!(
        "{n} places, {} pairs in sight of each other ({:.1}%), {} chokepoints, {} spots to hold, {} tripmine spots, \
         {} lanes for tripmine trails; worked out in {} ms",
        t.stats.pairs,
        200.0 * t.stats.pairs as f64 / (n * n.saturating_sub(1)).max(1) as f64,
        t.chokes.len(),
        t.camps.len(),
        t.mines.len(),
        t.lanes.len(),
        t.stats.millis
    )];
    let x = rt.experience.as_ref();
    match args.first().copied() {
        Some("spots") => {
            for (i, c) in t.camps.iter().enumerate() {
                out.push(format!(
                    "  {i}: {} at {}, score {:.2}, watching {:.0}° and {:.0}° {:.0} u away{}",
                    c.kind.as_str(),
                    at(c.pos),
                    c.score,
                    c.watch[0],
                    c.watch[1],
                    c.range,
                    c.guards
                        .map(|g| format!(", guards {}", at(t.origins[g as usize])))
                        .unwrap_or_default()
                ));
            }
        }
        Some("mines") => {
            for (i, m) in t.mines.iter().enumerate() {
                out.push(format!(
                    "  {i}: {} at {}, beam {:.0} u, flow {:.2}",
                    if m.corner { "corner" } else { "across" },
                    at(m.wall),
                    m.wall.distance(m.beam_end),
                    m.flow
                ));
            }
        }
        Some("lanes") => {
            for (i, l) in t.lanes.iter().enumerate() {
                out.push(format!(
                    "  {i}: from {} toward {:.0}°, {:.0} u, room {:.0} left {:.0} right, flow {:.2}",
                    at(l.start),
                    lb_core::dmath::atan2(l.dir.y, l.dir.x).to_degrees(),
                    l.length,
                    l.room[0],
                    l.room[1],
                    l.flow
                ));
            }
        }
        Some("danger") => match x {
            Some(x) => {
                for (node, d) in x.worst(15) {
                    let from = x
                        .danger_from(node)
                        .map(|f| format!(", mostly from {}", at(t.origins[f as usize])))
                        .unwrap_or_default();
                    let e = &x.nodes[node as usize];
                    out.push(format!(
                        "  {}: danger {d:.2} ({:.0} damage, {:.1} deaths){from}",
                        at(t.origins[node as usize]),
                        e.hurt,
                        e.deaths
                    ));
                }
            }
            None => out.push("nothing learned yet".into()),
        },
        _ => {
            let learned = x.map_or(0, |x| x.worst(usize::MAX).len());
            out.push(format!(
                "experience: bots got hurt at {learned} places{}; `lb map spots|mines|lanes|danger` for the lists",
                if x.is_some_and(|x| x.changed) {
                    " (not saved yet)"
                } else {
                    ""
                }
            ));
        }
    }
    out
}

/// What the bot's goal is doing, for `lb brain`.
fn task_text(t: &lb_brain::goals::Task, now: lb_core::time::SimTime) -> String {
    use lb_brain::goals::Task;
    let at = |p: lb_core::Vec3| format!("{:.0} {:.0} {:.0}", p.x, p.y, p.z);
    match t {
        Task::Search { dest, at: when, .. } => {
            format!("searching from {} (chosen {:.1} s ago)", at(*dest), now.since(*when))
        }
        Task::Investigate { dest, arrived, .. } => match arrived {
            Some(t) => format!("looking about for {:.1} s", now.since(*t)),
            None => format!("going to see from {}", at(*dest)),
        },
        Task::Hide { dest, arrived, .. } => {
            if *arrived {
                format!("hiding at {}", at(*dest))
            } else {
                format!("off to cover at {}", at(*dest))
            }
        }
        Task::Camp { until, .. } => match until {
            Some(u) => format!("holding the spot {:.1} s more", u.since(now)),
            None => "on the way to the spot".into(),
        },
        Task::Control { wait_at, waited, .. } => {
            if *waited {
                format!("waiting at {}", at(*wait_at))
            } else {
                format!("on the way to wait at {}", at(*wait_at))
            }
        }
        Task::Trap { started, until, .. } => match (started, until) {
            (_, Some(u)) => format!("watching the trap {:.1} s more", u.since(now)),
            (Some(_), None) => "laying it".into(),
            (None, None) => "on the way".into(),
        },
        Task::Lure {
            spot, thrown, until, ..
        } => match (thrown, until) {
            (_, Some(u)) => format!("watching the satchels at {} {:.1} s more", at(*spot), u.since(now)),
            (Some(_), None) => format!("satchels thrown at {}", at(*spot)),
            (None, None) => format!("on the way to throw satchels at {}", at(*spot)),
        },
        Task::Trail(t) => {
            use lb_brain::trail::TrailPhase;
            let way = lb_core::dmath::atan2(t.dir.y, t.dir.x).to_degrees();
            match (t.phase, t.until) {
                (TrailPhase::Watch, Some(u)) if now < u => {
                    format!(
                        "watching the trail from {} {:.1} s more",
                        at(t.stand.unwrap_or_default()),
                        u.since(now)
                    )
                }
                (TrailPhase::Watch, _) => "the watch is over: setting it off".into(),
                (TrailPhase::Off, _) => match t.stand {
                    Some(w) => format!("on the way to watch the trail from {}", at(w)),
                    None => "running on past the trail".into(),
                },
                (TrailPhase::Run, _) => format!("laying a trail along its lane toward {way:.0}°"),
                (TrailPhase::Approach, _) => format!("going to a lane at {} toward {way:.0}°", at(t.start)),
            }
        }
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
        if let Some(board) = rt.gungame_board() {
            out.push(format!("  gungame: {}", gungame_text(rt, &board, b)));
        }
        let gs = &m.decider.stats;
        let ms = &m.stats;
        let taken: Vec<String> = gs.taken.iter().map(|(k, n)| format!("{k} ×{n}")).collect();
        let (own_aggr, own_fear) = m.mood.base();
        out.push(format!(
            "  mind: {}; aggression {:.2} (own {:.2}), fear {:.2} (own {:.2}); goals taken: {}; {} goal changes ({} \
             before the hold ran out, {} with no fight in them), {} target changes; {} sounds seen about, {} covers, {} \
             spots held, {} items waited for, {} traps; stood still {:.0}% of the time out of fights, {:.0}% in them{}",
            m.task.as_ref().map(|t| task_text(t, now)).unwrap_or_else(|| "-".into()),
            m.mood.aggression,
            own_aggr,
            m.mood.fear,
            own_fear,
            if taken.is_empty() { "-".into() } else { taken.join(", ") },
            gs.switches,
            gs.early,
            gs.calm,
            ms.target_switches,
            ms.investigated,
            ms.covers,
            ms.camps,
            ms.controlled,
            ms.traps,
            ms.still_shares()[0] * 100.0,
            ms.still_shares()[1] * 100.0,
            b.brain
                .expect
                .map(|(k, p)| format!(
                    "; expects {} at {:.0} {:.0} {:.0}",
                    name(k.slot),
                    p.x,
                    p.y,
                    p.z
                ))
                .unwrap_or_default()
        ));
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
            "  arms: {}; thrown {} grenades ({} where an enemy was expected, {} in series), {} satchels, {} snarks ({} \
             barrages); {} m203, {} mines laid, {} detonations, {} mines shot, {} scoped in {} zooms, gauss {} fired ({} \
             through walls) {} dumped {} plain rolls {:.1} s cramped, {} dodges, {} runs from snarks, {} failed{}; \
             explosives known: {} own satchels, {} mines, {} in flight",
            arms.describe(now),
            st.grenades,
            st.blind,
            st.series,
            st.satchels,
            st.snarks,
            st.barrages,
            st.lobs,
            st.mines,
            st.detonations,
            st.mine_shots,
            st.scoped,
            st.zooms,
            arms.gauss.fired,
            st.wallbangs,
            arms.gauss.dumped,
            arms.gauss.plain_rolls,
            f64::from(arms.gauss.cramped) * 0.1,
            st.dodges,
            st.snark_runs,
            st.failed,
            arms.last_failure.map(|f| format!(" (last: {f})")).unwrap_or_default(),
            b.brain.explosives.charges.len(),
            b.brain.explosives.mines.len(),
            b.brain.explosives.flying.len(),
        ));
        let carried = crate::arsenal(&b.self_state, &rt.game.weapons);
        let count = |w: WeaponId| carried.iter().find(|a| a.id == w).and_then(|a| a.reserve).unwrap_or(0);
        out.push(format!(
            "  explosives carried: {} grenades, {} satchels, {} snarks, {} mines, {} m203",
            count(WeaponId::HandGrenade),
            count(WeaponId::Satchel),
            count(WeaponId::Snark),
            count(WeaponId::Tripmine),
            carried
                .iter()
                .find(|a| a.id == WeaponId::Mp5)
                .and_then(|a| a.reserve2)
                .unwrap_or(0),
        ));
        let ts = &m.tricks.stats;
        let counts = &b.nav.tricks;
        let went: Vec<String> = lb_nav::follow::TrickKind::ALL
            .iter()
            .enumerate()
            .filter(|(i, _)| counts.landed[*i] + counts.missed[*i] > 0)
            .map(|(i, k)| {
                format!(
                    "{} {}/{}",
                    k.as_str(),
                    counts.landed[i],
                    counts.landed[i] + counts.missed[i]
                )
            })
            .collect();
        let off: Vec<String> = ts.boost_failures.iter().map(|(w, n)| format!("{w} ×{n}")).collect();
        let told = &m.tricks.told;
        let yes = |v: bool| if v { "yes" } else { "no" };
        let ch = &b.character;
        out.push(format!(
            "  tricks: long jump module {}; long jumps taken {:.0}% on the way, {:.0}% in a fight (bold {}, to dodge \
             {}); the way may take long jumps {} (along it {}, may land for {:.0} damage), gauss boosts {} (now {}), \
             {} uranium; last look for a gauss jump: {}; landed/left the ground: {}; {} long jumps at enemies, {} to \
             dodge, {} gauss jumps on the way found, {} gauss boosts started, {} fired{}",
            yes(b.self_state.body.has_longjump),
            lb_brain::tricks::leap_chance(ch, false) * 100.0,
            lb_brain::tricks::leap_chance(ch, true) * 100.0,
            yes(ch.skill.longjump_bold),
            yes(ch.skill.longjump_dodge),
            yes(told.longjump),
            yes(told.runway),
            told.runway_hurt,
            yes(told.gauss_boost),
            yes(told.boost_now),
            m.tricks.uranium,
            if m.tricks.gauss_why.is_empty() {
                "-"
            } else {
                m.tricks.gauss_why
            },
            if went.is_empty() { "-".into() } else { went.join(", ") },
            ts.leaps,
            ts.dodges,
            ts.gauss_jumps,
            ts.boosts,
            ts.boosts_fired,
            if off.is_empty() {
                String::new()
            } else {
                format!(" (given up: {})", off.join("; "))
            }
        ));
        out.extend(stall_lines(&b.stall.counts, "  stalls, "));
        if !st.satchel_offs.is_empty() {
            let e: Vec<String> = st.satchel_offs.iter().map(|(w, n)| format!("{w} ×{n}")).collect();
            out.push(format!("  satchels set off: {}", e.join("; ")));
        }
        if st.trails > 0 || arms.trail.is_some() || !st.shot_whys.is_empty() {
            let why: Vec<String> = st.shot_whys.iter().map(|(w, n)| format!("{w} ×{n}")).collect();
            let plan = arms.trail.as_ref().map_or_else(
                || "none now".to_string(),
                |p| {
                    format!(
                        "{} dropped of {}, {} lying{}",
                        p.dropped,
                        p.wanted,
                        p.mines.len(),
                        if p.blow_at.is_some() { ", being set off" } else { "" }
                    )
                },
            );
            out.push(format!(
                "  trails: {} laid, {} mines dropped on the run, {} left lying; this one: {} (last look for the next \
                 mine: {}); mines shot: {}",
                st.trails,
                st.dropped,
                st.trails_left,
                plan,
                if arms.drop_why.is_empty() { "-" } else { arms.drop_why },
                if why.is_empty() { "-".into() } else { why.join("; ") }
            ));
        }
        if !st.scope_ends.is_empty() {
            let e: Vec<String> = st.scope_ends.iter().map(|(w, n)| format!("{w} ×{n}")).collect();
            out.push(format!("  scope off: {}", e.join("; ")));
        }
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
        for (kind, r) in [("new enemies", &r.fresh), ("enemies seen again", &r.reacquired)] {
            if r.count > 0 {
                out.push(format!(
                    "  reactions to {kind}: {} answered; first glimpse to first shot mean {:.2} s, median {:.2} s, worst {:.2} s; recognition to shot mean {:.2} s",
                    r.count,
                    r.evidence_to_shot / r.count as f64,
                    r.median().unwrap_or(0.0),
                    r.worst,
                    r.recognition_to_shot / r.count as f64
                ));
            }
        }
    }
    out
}

/// Tricks of all the bots: how the ones that left the ground went, by kind, and those the brains took.
#[derive(Clone, Copy, Debug, Default)]
pub struct TrickTotals {
    counts: lb_nav::follow::TrickCounts,
    leaps: u32,
    dodges: u32,
    gauss_jumps: u32,
    boosts: u32,
    boosts_fired: u32,
}

impl TrickTotals {
    /// What happened since `base`.
    fn since(&self, base: &TrickTotals) -> TrickTotals {
        let mut d = *self;
        for i in 0..4 {
            d.counts.landed[i] = d.counts.landed[i].saturating_sub(base.counts.landed[i]);
            d.counts.missed[i] = d.counts.missed[i].saturating_sub(base.counts.missed[i]);
        }
        d.leaps = d.leaps.saturating_sub(base.leaps);
        d.dodges = d.dodges.saturating_sub(base.dodges);
        d.gauss_jumps = d.gauss_jumps.saturating_sub(base.gauss_jumps);
        d.boosts = d.boosts.saturating_sub(base.boosts);
        d.boosts_fired = d.boosts_fired.saturating_sub(base.boosts_fired);
        d
    }

    fn line(&self) -> String {
        let went: Vec<String> = lb_nav::follow::TrickKind::ALL
            .iter()
            .enumerate()
            .map(|(i, k)| {
                let (landed, all) = (self.counts.landed[i], self.counts.landed[i] + self.counts.missed[i]);
                format!("{} {landed}/{all}", k.as_str())
            })
            .collect();
        format!(
            "  tricks landed/left the ground: {}; {} long jumps at enemies, {} to dodge, {} gauss jumps on the way \
             found, {} gauss boosts started, {} fired",
            went.join(", "),
            self.leaps,
            self.dodges,
            self.gauss_jumps,
            self.boosts,
            self.boosts_fired
        )
    }
}

/// What the stall watch counted: enemies in plain sight not seen, targets not fought, standing still.
fn stall_lines(s: &crate::stall::Stalls, indent: &str) -> Vec<String> {
    [
        ("enemies close in front not seen", &s.unseen),
        ("targets in sight not fought", &s.idle),
        ("standing still over 2 s", &s.still),
    ]
    .iter()
    .map(|(what, c)| {
        let (n, secs) = c.total();
        let why = c.line();
        format!(
            "{indent}{what}: {n} ({secs:.1} s){}",
            if why.is_empty() {
                String::new()
            } else {
                format!(": {why}")
            }
        )
    })
    .collect()
}

/// One bot's tricks so far.
fn bot_tricks(b: &crate::manager::Bot) -> TrickTotals {
    let s = &b.brain.mind.tricks.stats;
    TrickTotals {
        counts: b.nav.tricks,
        leaps: s.leaps,
        dodges: s.dodges,
        gauss_jumps: s.gauss_jumps,
        boosts: s.boosts,
        boosts_fired: s.boosts_fired,
    }
}

/// The bots' tricks since the statistics were last reset: each bot's since then, all of those of a bot that joined
/// after; bots that left take theirs with them.
pub fn trick_totals(rt: &Runtime) -> TrickTotals {
    let mut t = TrickTotals::default();
    for b in &rt.bots {
        let base = rt
            .tricks_base
            .iter()
            .find(|(id, _)| *id == b.id)
            .map_or_else(TrickTotals::default, |(_, base)| *base);
        let d = bot_tricks(b).since(&base);
        for i in 0..4 {
            t.counts.landed[i] += d.counts.landed[i];
            t.counts.missed[i] += d.counts.missed[i];
        }
        t.leaps += d.leaps;
        t.dodges += d.dodges;
        t.gauss_jumps += d.gauss_jumps;
        t.boosts += d.boosts;
        t.boosts_fired += d.boosts_fired;
    }
    t
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
    let reflex = rt.config.bots.reflex;
    let k = p.skill_params(&rt.presets).with_reflex(reflex);
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
            "  weapons: favourites {}; the style's likes {}, throws ×{:.1}; tags: {}",
            if p.weapons.is_empty() {
                "none".into()
            } else {
                p.weapons.join(", ")
            },
            {
                let likes = &rt.styles.weapons(p.style).guns;
                if likes.is_empty() {
                    "none".to_string()
                } else {
                    likes
                        .iter()
                        .map(|(n, v)| format!("{n} ×{v:.2}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                }
            },
            rt.styles.weapons(p.style).throwables,
            if p.tags.is_empty() {
                "-".into()
            } else {
                p.tags.join(", ")
            }
        ),
        format!(
            "  skill{}: recognition {:.2}-{:.2} s (not before {:.2} s), aim latency {:.2} s, {:?} aim, headshot \
             {:.0}%, turn {:.0} deg/s at {:.0} deg/s²",
            if reflex == 1.0 {
                String::new()
            } else {
                format!(" with reflexes ×{reflex}")
            },
            k.recognition_delay[0],
            k.recognition_delay[1],
            k.recognition_floor,
            k.aim_latency,
            k.aim_model,
            k.headshot * 100.0,
            k.turn_speed,
            k.turn_accel
        ),
        format!(
            "         hearing {:.3} (bearing {:.0} deg), memory {:.0} s, dodge jump {}, tricks {}, long jumps {:.0}% \
             (bold {}, to dodge {}), throws ×{:.2} (grenades in series {}), gauss through walls {}, bhop {}",
            k.hearing_threshold,
            k.sound_bearing_sigma,
            k.track_forget,
            opt(k.dodge_hop_cooldown, " s"),
            if k.tricks { "yes" } else { "no" },
            k.longjump * 100.0,
            if k.longjump_bold { "yes" } else { "no" },
            if k.longjump_dodge { "yes" } else { "no" },
            k.throw_rate,
            if k.throw_series { "yes" } else { "no" },
            if k.gauss_walls { "yes" } else { "no" },
            opt(k.bhop_speed, "x"),
        ),
        {
            let g = rt.styles.goals(p.style);
            format!(
                "  goals: engage ×{:.1}, hunt ×{:.1}, retreat ×{:.1}, collect ×{:.1}, investigate ×{:.1}, camp ×{:.1}, \
                 ambush ×{:.1}, control ×{:.1}, trap ×{:.1}",
                g.engage, g.hunt, g.retreat, g.collect, g.investigate, g.camp, g.ambush, g.control, g.trap
            )
        },
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

pub(crate) fn find_bots(rt: &Runtime, target: Option<&str>) -> Vec<usize> {
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
        return crate::testrun::command(rt, host, args);
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
            rt.weapons_give = if give { WeaponId::ALL.to_vec() } else { Vec::new() };
        }
        "melee" => {
            rt.weapons_allowed = WeaponId::Crowbar.bit();
            rt.weapons_give = if give { vec![WeaponId::Crowbar] } else { Vec::new() };
        }
        _ => {
            let mut list = Vec::new();
            for n in &names {
                match crate::orders::weapon(n) {
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

/// `lb items`: items every bot is given on spawn (`give` works with `sv_cheats 1`), by classname with or without
/// `item_`.
fn items(rt: &mut Runtime, args: &[&str]) -> Vec<String> {
    match args {
        [] => {}
        ["none"] => rt.items_give.clear(),
        names => {
            rt.items_give = names
                .iter()
                .map(|n| {
                    if n.starts_with("item_") {
                        n.to_string()
                    } else {
                        format!("item_{n}")
                    }
                })
                .collect();
        }
    }
    vec![format!(
        "given on spawn: {}",
        if rt.items_give.is_empty() {
            "nothing".into()
        } else {
            rt.items_give.join(", ")
        }
    )]
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
