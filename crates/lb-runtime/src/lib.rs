//! Runtime: frame pipeline, scheduler, workers, bot manager, commands and cvars.

#![forbid(unsafe_code)]

pub mod buttons;
pub mod capture;
pub mod clients;
pub mod commands;
pub mod cvars;
pub mod logging;
pub mod manager;
pub mod motor_test;
pub mod names;
pub mod perf;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;

use lb_config::MainConfig;
use lb_config::main_config::{QuotaMode, parse_difficulty};
use lb_core::Vec3;
use lb_core::handles::{BotId, MapEpoch};
use lb_core::math::normalize_angle;
use lb_core::rng::{Pcg32, splitmix64};
use lb_core::time::SimTime;
use lb_ffi::{LB_CMD_SET_SEED, LbBotCommand, LbMoveFeedback, LbVec3};
use lb_game::compat::CompatibilityProfile;
use lb_game::messages::{self, GameMsg};
use lb_game::mode::{GameModeKind, ModeInputs};
use lb_game::rules::{PublicRules, RULE_CVARS};
use lb_game::scoreboard::Scoreboard;
use lb_game::self_state::WeaponRegistry;
use lb_host::strings::StringTable;
use lb_host::{CreateBotOutcome, CreateBotRequest, Host, TrackRule};
use lb_raw::{ClientEventKind, RawEvent, RawFrame};
use lb_telemetry::{CommandChannel, TelemetrySink};
use serde::Serialize;

use crate::buttons::*;
use crate::clients::Clients;
use crate::cvars::{Cv, Cvars};
use crate::manager::{Bot, BotState, Creation, Identity, desired_bots, pick_bot_to_kick, random_color};
use crate::names::NamePool;

pub const CORE_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Debug)]
pub struct InitData {
    pub adapter_version: String,
    pub plugin_path: PathBuf,
    pub game_dir: PathBuf,
    pub install_dir: PathBuf,
    pub platform: u8,
    pub late_load: bool,
}

#[derive(Clone, Debug)]
pub struct MapState {
    pub name: String,
    pub max_clients: u32,
    pub late_load: bool,
}

#[derive(Default)]
pub struct GameState {
    pub rules: PublicRules,
    pub scoreboard: Scoreboard,
    pub weapons: WeaponRegistry,
    pub mode: Option<GameModeKind>,
    pub teamplay_message: bool,
    pub gg_bridge_state: Option<bool>,
    pub resolved_msgs: Vec<(String, i32)>,
}

#[derive(Default, Debug, Clone, Serialize)]
pub struct Stats {
    pub frames: u64,
    pub malformed_records: u64,
    pub dropped_events: u64,
    pub bot_faults: u64,
    pub commands_sent: u64,
    pub move_calls_failed: u64,
    pub stale_moves: u64,
    pub max_frame_ms: f64,
}

pub struct Runtime {
    pub init: InitData,
    pub config: MainConfig,
    pub config_source: String,
    pub cvars: Cvars,
    pub strings: StringTable,
    pub epoch: MapEpoch,
    pub map: Option<MapState>,
    pub clients: Clients,
    pub bots: Vec<Bot>,
    pub creation: Creation,
    pub carry_over: Vec<Identity>,
    pub names: NamePool,
    pub rng: Pcg32,
    pub master_seed: u64,
    pub game: GameState,
    pub telemetry: TelemetrySink,
    pub commands: CommandChannel,
    pub safe_mode: Option<String>,
    pub safe_mode_applied: bool,
    pub now: SimTime,
    pub frame_no: u64,
    pub frame_time: f64,
    pub last_quota_check: SimTime,
    pub last_cvar_poll: SimTime,
    pub last_telemetry: SimTime,
    pub last_perf: SimTime,
    pub stats: Stats,
    pub core_times: perf::CoreTimes,
    pub compat: CompatibilityProfile,
    pub dev: bool,
    pub freeze: bool,
    pub game_mode_forced: i32,
    feedback: Vec<LbMoveFeedback>,
    cmds: Vec<LbBotCommand>,
    last_mono_ns: u64,
    /// Other players from this frame's snapshots, for telemetry only.
    others: Vec<(u8, Vec3, f32, bool)>,
    pub capture: Option<capture::MessageCapture>,
}

impl Runtime {
    pub fn new(host: &mut dyn Host, init: InitData) -> Runtime {
        let config_path = init.install_dir.join("config").join("lambdabots.yaml");
        let (config, config_source, config_error) = match std::fs::read_to_string(&config_path) {
            Ok(text) => match MainConfig::parse(&text, &config_path.display().to_string()) {
                Ok(cfg) => (cfg, config_path.display().to_string(), None),
                Err(e) => (MainConfig::default(), "defaults".to_string(), Some(e.to_string())),
            },
            Err(_) => (MainConfig::default(), "defaults".to_string(), None),
        };
        let log_dir = config.logging.file.then(|| init.install_dir.join("logs"));
        logging::init(
            log_dir.as_deref(),
            &config.logging.level,
            &config.logging.console_level,
            config.logging.max_files as usize,
        );
        if let Some(e) = &config_error {
            tracing::error!("config error, using defaults: {e}");
        }
        let master_seed = if config.engine.master_seed != 0 {
            config.engine.master_seed
        } else {
            let t = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(1);
            splitmix64(t)
        };
        let mut rt = Runtime {
            names: NamePool::load(&init.install_dir, &config.bots.language),
            init,
            config,
            config_source,
            cvars: Cvars::default(),
            strings: StringTable::default(),
            epoch: MapEpoch(0),
            map: None,
            clients: Clients::default(),
            bots: Vec::new(),
            creation: Creation::default(),
            carry_over: Vec::new(),
            rng: Pcg32::new(master_seed, 0x6c62),
            master_seed,
            game: GameState::default(),
            telemetry: TelemetrySink::disabled(),
            commands: CommandChannel::disabled(),
            safe_mode: None,
            safe_mode_applied: false,
            now: SimTime::ZERO,
            frame_no: 0,
            frame_time: 0.0,
            last_quota_check: SimTime::ZERO,
            last_cvar_poll: SimTime::ZERO,
            last_telemetry: SimTime::ZERO,
            last_perf: SimTime::ZERO,
            stats: Stats::default(),
            core_times: perf::CoreTimes::default(),
            compat: CompatibilityProfile::default(),
            dev: false,
            freeze: false,
            game_mode_forced: -1,
            feedback: Vec::new(),
            cmds: Vec::new(),
            last_mono_ns: 0,
            others: Vec::new(),
            capture: None,
        };
        rt.register_cvars(host);
        rt.open_telemetry();
        tracing::info!(
            "lambdabots {CORE_VERSION} started (adapter {}, config: {}, seed {:#x}, {} names)",
            rt.init.adapter_version,
            rt.config_source,
            rt.master_seed,
            rt.names.len()
        );
        rt
    }

    fn register_cvars(&mut self, host: &mut dyn Host) {
        let c = &self.config;
        let mode = match c.quota.mode {
            QuotaMode::Normal => "normal",
            QuotaMode::Fill => "fill",
            QuotaMode::Match => "match",
        };
        let defaults = vec![
            (Cv::Version, CORE_VERSION.to_string()),
            (Cv::Quota, c.quota.count.to_string()),
            (Cv::QuotaMode, mode.to_string()),
            (Cv::Difficulty, c.bots.difficulty.clone()),
            (Cv::CmdRate, c.engine.cmd_rate.to_string()),
            (Cv::ForceRespawn, (c.bots.force_respawn as u8).to_string()),
            (Cv::GameMode, "-1".to_string()),
            (Cv::GunGame, c.gungame.mode.clone()),
            (Cv::Telemetry, (c.telemetry.enabled as u8).to_string()),
            (Cv::TelemetryPort, c.telemetry.port.to_string()),
            (Cv::TelemetrySecret, c.telemetry.secret.clone()),
            (Cv::Freeze, "0".to_string()),
            (Cv::Debug, "0".to_string()),
            (Cv::Dev, "0".to_string()),
            (Cv::LogLevel, c.logging.level.clone()),
        ];
        self.cvars.register(host, &defaults);
    }

    fn open_telemetry(&mut self) {
        let t = &self.config.telemetry;
        self.telemetry = if t.enabled {
            match TelemetrySink::open(&t.dest, t.port, self.master_seed, t.max_kbps) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("telemetry disabled: {e}");
                    TelemetrySink::disabled()
                }
            }
        } else {
            TelemetrySink::disabled()
        };
        self.commands = if t.enabled && (!t.secret.is_empty() || t.allow_unauthenticated_loopback) {
            match CommandChannel::open(
                &t.cmd_bind,
                t.port.wrapping_add(1),
                &t.secret,
                t.allow_unauthenticated_loopback,
            ) {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!("telemetry command channel disabled: {e}");
                    CommandChannel::disabled()
                }
            }
        } else {
            CommandChannel::disabled()
        };
    }

    pub fn map_start(&mut self, host: &mut dyn Host, name: &str, max_clients: u32, epoch: MapEpoch, late_load: bool) {
        if let Some(c) = self.capture.take() {
            let (path, n) = c.finish();
            tracing::warn!(
                "message capture stopped by the map change: {n} messages in {}",
                path.display()
            );
        }
        self.epoch = epoch;
        self.telemetry.set_epoch(epoch.0);
        self.clients.reset(max_clients as usize);
        self.game.scoreboard = Scoreboard::default();
        self.game.scoreboard.resize(max_clients as usize);
        self.game.weapons = WeaponRegistry::default();
        self.game.mode = None;
        self.game.teamplay_message = false;
        self.strings.clear_map_scoped();
        if !self.bots.is_empty() {
            let mut saved: Vec<Identity> = self.bots.drain(..).map(|b| b.identity).collect();
            if self.config.bots.save_names {
                saved.append(&mut self.carry_over);
                self.carry_over = saved;
            }
        }
        // Sim time restarts with every map; the first frame of this one sets it again.
        self.now = SimTime::ZERO;
        self.last_quota_check = SimTime::ZERO;
        self.last_cvar_poll = SimTime::ZERO;
        self.last_telemetry = SimTime::ZERO;
        self.last_perf = SimTime::ZERO;
        self.map = Some(MapState {
            name: name.to_string(),
            max_clients,
            late_load,
        });
        self.creation = Creation::default();

        let rules: Vec<TrackRule> = lb_game::entities::TRACK_RULES
            .iter()
            .map(|(p, prefix, kind)| TrackRule {
                pattern: p.to_string(),
                prefix: *prefix,
                kind: *kind,
            })
            .collect();
        if !host.set_track_rules(&rules) {
            tracing::warn!("entity registry rules were not accepted by the adapter");
        }
        self.resolve_messages(host);
        self.read_rules(host);
        self.build_compat(host);
        tracing::info!(
            "map {name} (epoch {}, {} slots){}",
            epoch.0,
            max_clients,
            if late_load { ", late load" } else { "" }
        );
        for line in self.compat.to_yaml().lines() {
            tracing::debug!("compat: {line}");
        }
        let _ = std::fs::create_dir_all(self.init.install_dir.join("logs"));
        let _ = std::fs::write(
            self.init.install_dir.join("logs").join(format!("compat-{name}.yaml")),
            self.compat.to_yaml(),
        );
        self.telemetry.send(
            "hello",
            self.now.secs(),
            &serde_json::json!({
                "core": CORE_VERSION,
                "adapter": self.init.adapter_version,
                "abi": lb_ffi::LB_ABI_VERSION,
                "map": name,
                "seed": self.master_seed,
                "engine": self.compat.engine,
            }),
        );
    }

    pub fn map_end(&mut self, _host: &mut dyn Host) {
        if self.config.bots.save_names {
            let saved: Vec<Identity> = self
                .bots
                .iter()
                .filter(|b| !matches!(b.state, BotState::Leaving | BotState::Faulted))
                .map(|b| b.identity.clone())
                .collect();
            self.carry_over = saved;
        }
        self.bots.clear();
        self.map = None;
    }

    fn resolve_messages(&mut self, host: &mut dyn Host) {
        let mut mask = [0u8; 32];
        self.game.resolved_msgs.clear();
        for name in messages::WANTED {
            if let Some((id, _size)) = host.resolve_user_msg(name)
                && (0..256).contains(&id)
            {
                mask[id as usize / 8] |= 1 << (id % 8);
                self.strings.set_msg_name(id, name.as_bytes().to_vec());
                self.game.resolved_msgs.push((name.to_string(), id));
            }
        }
        const SVC_TEMPENTITY: usize = 23;
        const SVC_INTERMISSION: usize = 30;
        for id in [SVC_TEMPENTITY, SVC_INTERMISSION] {
            mask[id / 8] |= 1 << (id % 8);
        }
        if !host.set_capture_mask(&mask) {
            tracing::warn!("message capture mask was not accepted by the adapter");
        }
    }

    fn read_rules(&mut self, host: &mut dyn Host) {
        let mut rules = PublicRules::default();
        for name in RULE_CVARS {
            if let Some(value) = self.cvars.game_value(host, name) {
                rules.apply_cvar(name, &value);
            }
        }
        self.game.rules = rules;
    }

    fn build_compat(&mut self, host: &mut dyn Host) {
        let f = host.compat_facts();
        let bhl = self.cvars.game_value(host, "mp_respawn_fix").is_some();
        let ticrate = self
            .cvars
            .game_value(host, "sys_ticrate")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.0);
        let mut plugins = Vec::new();
        for (key, cvar) in [
            ("amxmodx", "amxmodx_version"),
            ("gungame", "gungame"),
            ("metamod", "metamod_version"),
        ] {
            if let Some(v) = self.cvars.game_value(host, cvar) {
                plugins.push((key.to_string(), v));
            }
        }
        let ch = |bit: u32| f.channels & bit != 0;
        self.compat = CompatibilityProfile {
            core_version: CORE_VERSION.to_string(),
            adapter_version: self.init.adapter_version.clone(),
            abi_version: lb_ffi::LB_ABI_VERSION,
            platform: match self.init.platform {
                lb_ffi::LB_PLATFORM_LINUX => "linux".into(),
                lb_ffi::LB_PLATFORM_WINDOWS => "windows".into(),
                lb_ffi::LB_PLATFORM_MACOS => "macos".into(),
                _ => "unknown".into(),
            },
            engine: match f.engine_kind {
                lb_ffi::LB_ENGINE_REHLDS => "rehlds".into(),
                lb_ffi::LB_ENGINE_XASH => "xash3d".into(),
                lb_ffi::LB_ENGINE_HLDS => "hlds".into(),
                _ => "unknown".into(),
            },
            engine_version: f.engine_version.clone(),
            rehlds: f.rehlds_version.map(|(a, b, c)| format!("{a}.{b} build {c}")),
            metamod: f.metamod_version.clone(),
            metamod_hook_tables: f.metamod_has_hook_tables,
            gamedll: f.gamedll_desc.clone(),
            gamedll_path: f.gamedll_path.clone(),
            bugfixedhl: bhl,
            channels: vec![
                ("rehlds_sv_startsound", ch(lb_ffi::LB_CH_SV_STARTSOUND)),
                ("rehlds_message_manager", ch(lb_ffi::LB_CH_MSGMGR)),
                ("rehlds_emit_pings", ch(lb_ffi::LB_CH_EMITPINGS)),
                ("rehlds_drop_client", ch(lb_ffi::LB_CH_DROPCLIENT)),
                ("rehlds_host_time", ch(lb_ffi::LB_CH_HOSTTIME)),
                ("metamod_hook_tables", ch(lb_ffi::LB_CH_HOOK_TABLES)),
                ("ambient_sound", ch(lb_ffi::LB_CH_AMBIENT_SOUND)),
                ("playback_event", ch(lb_ffi::LB_CH_PLAYBACK_EVENT)),
            ],
            resolved_messages: self.game.resolved_msgs.clone(),
            missing_messages: messages::WANTED
                .iter()
                .filter(|n| !self.game.resolved_msgs.iter().any(|(r, _)| r == *n))
                .map(|n| n.to_string())
                .collect(),
            event_names: 0,
            sys_ticrate: ticrate,
            plugins,
            map: self.map.as_ref().map(|m| m.name.clone()).unwrap_or_default(),
            late_load: self.map.as_ref().is_some_and(|m| m.late_load),
        };
    }

    // -----------------------------------------------------------------------------------------
    // Frame pipeline
    // -----------------------------------------------------------------------------------------

    pub fn frame_pre(&mut self, host: &mut dyn Host, frame: RawFrame, malformed: u32) {
        self.now = frame.header.sim_time;
        self.frame_no = frame.header.frame_no;
        self.frame_time = frame.header.frame_time;
        self.stats.frames += 1;
        self.stats.malformed_records += malformed as u64;
        self.stats.dropped_events += frame.dropped_events as u64;
        self.stats.max_frame_ms = self.stats.max_frame_ms.max(frame.header.frame_time * 1000.0);
        self.telemetry.begin_frame(frame.header.frame_time);

        for ev in frame.events {
            self.handle_event(host, ev.event);
        }
        for s in &frame.selves {
            if let Some(bot) = self
                .bots
                .iter_mut()
                .find(|b| b.id.slot == s.slot && b.id.generation == s.bot_gen)
            {
                let body = &mut bot.self_state.body;
                body.origin = s.origin;
                body.velocity = s.velocity;
                body.v_angle = s.v_angle;
                body.punchangle = s.punchangle;
                body.view_ofs = s.view_ofs;
                body.health = s.health;
                body.armor = s.armor;
                body.weapons_mask = s.weapons_mask;
                body.maxspeed = s.maxspeed;
                body.fov = s.fov;
                body.flags = s.flags;
                body.movetype = s.movetype;
                body.waterlevel = s.waterlevel;
                body.deadflag = s.deadflag;
                body.in_duck = s.in_duck;
                body.has_longjump = s.has_longjump;
                body.frags = s.frags;
                if !bot.view_initialized {
                    bot.view = s.v_angle;
                    bot.view_initialized = true;
                }
            }
        }
        self.others.clear();
        for c in &frame.clients {
            if c.state != lb_raw::ClientState::Free {
                self.game.scoreboard.set_frags(c.slot, c.frags as i32);
                if !c.is_ours && c.state == lb_raw::ClientState::Spawned {
                    self.others
                        .push((c.slot, c.origin, c.angles.y, c.deadflag == lb_game::self_state::DEAD_NO));
                }
            }
        }
        let force_respawn = self.config.bots.force_respawn;
        let delay = self.config.bots.respawn_delay;
        let now = self.now;
        for bot in &mut self.bots {
            bot.update_lifecycle(now, force_respawn, delay);
        }
        self.check_departures(host);
        if self.capture.as_ref().is_some_and(|c| self.now >= c.until)
            && let Some(c) = self.capture.take()
        {
            let (path, n) = c.finish();
            tracing::warn!("message capture finished: {n} messages in {}", path.display());
        }

        if self.now.since(self.last_cvar_poll) >= 1.0 || self.now < self.last_cvar_poll {
            self.last_cvar_poll = self.now;
            self.poll_cvars(host);
        }
        if self.safe_mode.is_some() {
            self.apply_safe_mode(host);
            return;
        }
        if self.now.since(self.last_quota_check) >= 0.25 || self.now < self.last_quota_check {
            self.last_quota_check = self.now;
            self.reconcile_quota(host);
        }
    }

    pub fn frame_post(&mut self, host: &mut dyn Host, mono_ns: u64) {
        if self.safe_mode.is_none() {
            self.drive_bots(host);
        }
        self.poll_command_channel(host);
        self.emit_telemetry();
        let (lines, dropped) = logging::drain_console(20);
        for line in lines {
            host.server_print(&format!("{line}\n"));
        }
        if dropped > 0 {
            host.server_print(&format!("[lambdabots] {dropped} console lines dropped\n"));
        }
        self.last_mono_ns = mono_ns;
    }

    fn handle_event(&mut self, host: &mut dyn Host, event: RawEvent) {
        match event {
            RawEvent::Client(e) => {
                tracing::debug!(
                    "client event {:?}: slot {} #{} ours {} fake {} gen {} name {:?}",
                    e.kind,
                    e.slot,
                    e.userid,
                    e.is_ours,
                    e.is_fake,
                    e.bot_gen,
                    String::from_utf8_lossy(&e.name)
                );
                self.clients.apply(&e);
                match e.kind {
                    ClientEventKind::Disconnect => {
                        let ours = |b: &Bot| b.id.slot == e.slot && (!e.is_ours || b.id.generation == e.bot_gen);
                        if let Some(i) = self.bots.iter().position(ours) {
                            let bot = self.bots.remove(i);
                            tracing::info!("bot {} left ({})", bot.identity.name, bot.state.as_str());
                        }
                        self.game.scoreboard.clear_slot(e.slot);
                    }
                    ClientEventKind::ConnectRejected => {
                        tracing::warn!("client in slot {} was rejected by the game", e.slot);
                    }
                    ClientEventKind::Connect => self.drop_slot_ghosts(e.slot, e.is_ours.then_some(e.bot_gen)),
                    _ => {}
                }
                self.telemetry.send(
                    "event",
                    self.now.secs(),
                    &serde_json::json!({
                        "kind": format!("{:?}", e.kind).to_ascii_lowercase(),
                        "slot": e.slot,
                        "ours": e.is_ours,
                        "name": String::from_utf8_lossy(&e.name),
                    }),
                );
            }
            RawEvent::UserMsg(m) => {
                let Some(name) = self.strings.msg_name(m.msg_id).map(|n| n.to_vec()) else {
                    return;
                };
                if let Some(c) = self.capture.as_mut() {
                    c.write(&name, &m);
                }
                let Some(msg) = messages::decode(&name, &m) else { return };
                if m.dest == lb_core::msg::MSG_ONE || m.dest == lb_core::msg::MSG_ONE_UNRELIABLE {
                    if let Some(bot) = self
                        .bots
                        .iter_mut()
                        .find(|b| b.id.slot == m.target_slot && b.is_active())
                    {
                        match &msg {
                            GameMsg::ResetHud => {
                                bot.seen_reset_hud = true;
                                bot.self_state.on_spawn(self.now);
                            }
                            GameMsg::WeaponList(info) => self.game.weapons.insert(info.clone()),
                            _ => bot.self_state.apply(&msg, self.now),
                        }
                    }
                } else {
                    self.game.scoreboard.apply(&msg);
                    match &msg {
                        GameMsg::GameMode { teamplay } => self.game.teamplay_message = *teamplay,
                        GameMsg::DeathMsg { killer, victim, weapon } => {
                            tracing::debug!("kill: {killer} -> {victim} ({weapon})");
                            self.telemetry.send(
                                "event",
                                self.now.secs(),
                                &serde_json::json!({
                                    "kind": "kill", "killer": killer, "victim": victim, "weapon": weapon,
                                }),
                            );
                        }
                        _ => {}
                    }
                }
            }
            RawEvent::ClientCommand(c) => {
                if c.argv.first().map(|a| a.as_slice()) == Some(b"lb") {
                    self.client_command(host, c.slot, &c.argv[1..]);
                }
            }
            RawEvent::ServerCommand(c) => {
                let args: Vec<String> = c.argv.iter().map(|a| String::from_utf8_lossy(a).into_owned()).collect();
                let refs: Vec<&str> = args.iter().skip(1).map(String::as_str).collect();
                let out = commands::execute(self, host, &refs);
                for line in out {
                    host.server_print(&format!("{line}\n"));
                }
            }
            RawEvent::Fixangle {
                slot,
                bot_gen,
                mode,
                angles,
            } => {
                tracing::debug!(
                    "fixangle slot {slot} mode {mode}: {:.1} {:.1} {:.1}",
                    angles.x,
                    angles.y,
                    angles.z
                );
                if let Some(bot) = self
                    .bots
                    .iter_mut()
                    .find(|b| b.id.slot == slot && b.id.generation == bot_gen)
                {
                    if mode == 2 {
                        bot.view.y = normalize_angle(bot.view.y + angles.y);
                    } else {
                        bot.view = Vec3::new(angles.x, angles.y, 0.0);
                    }
                    bot.view_initialized = true;
                }
            }
            RawEvent::Log { level, text } => {
                let text = String::from_utf8_lossy(&text);
                match level {
                    0 => tracing::error!("adapter: {text}"),
                    1 => tracing::warn!("adapter: {text}"),
                    _ => tracing::info!("adapter: {text}"),
                }
            }
            RawEvent::Overflow { dropped_records, .. } => {
                self.stats.dropped_events += dropped_records as u64;
            }
            RawEvent::RegisterMsg { .. } | RawEvent::PrecacheEvent { .. } => {}
            RawEvent::Sound(_) | RawEvent::Playback(_) | RawEvent::Entity(_) => {}
        }
    }

    fn client_command(&mut self, host: &mut dyn Host, slot: u8, argv: &[Vec<u8>]) {
        let Some(client) = self.clients.get(slot).cloned() else {
            return;
        };
        let access = &self.config.access;
        let by_id = !client.auth_id.is_empty() && access.admins.iter().any(|a| a.eq_ignore_ascii_case(&client.auth_id));
        let by_password =
            !access.password.is_empty() && host.client_info_key(slot, &access.password_key) == access.password;
        let listen_host = access.listen_host_is_admin && client.address == "loopback";
        if !(by_id || by_password || listen_host) {
            host.client_print(slot, lb_host::PrintKind::Console, "lambdabots: access denied\n");
            tracing::warn!("client {} ({}) was denied `lb` access", client.name, client.auth_id);
            return;
        }
        let args: Vec<String> = argv.iter().map(|a| String::from_utf8_lossy(a).into_owned()).collect();
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        tracing::info!("client {} ({}) ran: lb {}", client.name, client.auth_id, args.join(" "));
        for line in commands::execute(self, host, &refs) {
            host.client_print(slot, lb_host::PrintKind::Console, &format!("{line}\n"));
        }
    }

    fn poll_cvars(&mut self, host: &mut dyn Host) {
        for (cv, value) in self.cvars.poll_changes(host) {
            match cv {
                Cv::Quota => {
                    if let Ok(n) = value.trim().parse::<f32>() {
                        self.config.quota.count = n.clamp(0.0, 32.0) as u32;
                    }
                }
                Cv::QuotaMode => match value.trim() {
                    "normal" => self.config.quota.mode = QuotaMode::Normal,
                    "fill" => self.config.quota.mode = QuotaMode::Fill,
                    "match" => self.config.quota.mode = QuotaMode::Match,
                    other => tracing::warn!("lb_quota_mode: unknown mode `{other}`"),
                },
                Cv::Difficulty => {
                    if parse_difficulty(&value).is_some() {
                        self.config.bots.difficulty = value.trim().to_string();
                    }
                }
                Cv::CmdRate => {
                    if let Ok(r) = value.trim().parse::<f32>() {
                        self.config.engine.cmd_rate = r.clamp(0.0, 1000.0);
                        for bot in &mut self.bots {
                            bot.driver.set_rate(self.config.engine.cmd_rate as f64);
                        }
                    }
                }
                Cv::ForceRespawn => self.config.bots.force_respawn = value.trim() != "0",
                Cv::GameMode => self.game_mode_forced = value.trim().parse().unwrap_or(-1),
                Cv::GunGame => self.config.gungame.mode = value.trim().to_string(),
                Cv::Freeze => self.freeze = value.trim() != "0",
                Cv::Dev => self.dev = value.trim() != "0",
                Cv::Telemetry => {
                    self.config.telemetry.enabled = value.trim() != "0";
                    self.open_telemetry();
                }
                Cv::TelemetryPort => {
                    if let Ok(port) = value.trim().parse::<u16>() {
                        self.config.telemetry.port = port;
                        self.open_telemetry();
                    }
                }
                Cv::TelemetrySecret => {
                    self.config.telemetry.secret = value.trim().to_string();
                    self.open_telemetry();
                }
                Cv::LogLevel => {
                    logging::set_file_level(&value);
                }
                Cv::Version | Cv::Debug => {}
            }
        }
        self.read_rules(host);
        let gg_cvar = self.config.gungame.detect_cvar.clone();
        let inputs = ModeInputs {
            forced_mode: self.game_mode_forced,
            teamplay_cvar: self.game.rules.teamplay,
            teamplay_message: self.game.teamplay_message,
            gungame_mode: self.config.gungame.mode.clone(),
            gungame_cvar: self
                .cvars
                .game_value(host, &gg_cvar)
                .and_then(|v| v.trim().parse().ok()),
            gungame_teamplay_cvar: self
                .cvars
                .game_value(host, "gg_teamplay")
                .and_then(|v| v.trim().parse().ok()),
            gungame_bridge_state: self.game.gg_bridge_state,
        };
        let mode = lb_game::mode::detect(&inputs);
        if self.game.mode != Some(mode) {
            tracing::info!("game mode: {mode:?}");
            self.game.mode = Some(mode);
        }
    }

    fn reconcile_quota(&mut self, host: &mut dyn Host) {
        let Some(map) = &self.map else { return };
        let max_clients = map.max_clients;
        let humans = self.clients.humans(self.config.quota.count_connecting);
        let desired = desired_bots(&self.config.quota, humans, max_clients);
        let active = self
            .bots
            .iter()
            .filter(|b| !matches!(b.state, BotState::Leaving | BotState::Faulted))
            .count() as u32;
        if active < desired {
            if !self.creation.ready(self.now, self.config.quota.join_delay as f64) {
                return;
            }
            if self.clients.occupied() >= max_clients {
                return;
            }
            self.create_bot(host);
            let [lo, hi] = self.config.quota.join_interval;
            let wait = self.rng.range_f32(lo, hi) as f64;
            self.creation.next_at = Some(self.now + wait);
        } else if active > desired
            && let Some(i) = pick_bot_to_kick(&self.bots)
        {
            self.kick_bot_index(host, i, "quota");
        }
    }

    pub fn create_bot(&mut self, host: &mut dyn Host) -> Option<String> {
        let identity = match self.carry_over.pop() {
            Some(id) if self.clients.find_name(&id.name).is_none() => id,
            _ => self.new_identity(),
        };
        let mut infokeys = vec![("model".to_string(), identity.model.clone())];
        infokeys.push(("topcolor".to_string(), identity.topcolor.to_string()));
        infokeys.push(("bottomcolor".to_string(), identity.bottomcolor.to_string()));
        if self.config.disguise.scoreboard_bot_flag {
            infokeys.push(("*bot".to_string(), "1".to_string()));
        }
        let req = CreateBotRequest {
            name: identity.name.clone(),
            infokeys,
        };
        match host.create_bot(&req) {
            CreateBotOutcome::Created { slot, userid, bot_gen } => {
                let id = BotId {
                    slot,
                    generation: bot_gen,
                };
                let bot = Bot::new(
                    id,
                    userid,
                    identity.clone(),
                    self.now,
                    self.master_seed,
                    self.config.engine.cmd_rate as f64,
                    self.config.engine.max_cmd_debt_ms as f64,
                );
                tracing::info!("bot {} joined (slot {slot}, #{userid})", identity.name);
                self.bots.push(bot);
                Some(identity.name)
            }
            CreateBotOutcome::ServerFull => {
                self.creation.hold_until = self.now + 10.0;
                tracing::warn!("cannot add a bot: server is full");
                None
            }
            CreateBotOutcome::Rejected(reason) => {
                self.creation.hold_until = self.now + 10.0;
                tracing::warn!("bot {} was rejected: {reason}", identity.name);
                None
            }
            CreateBotOutcome::Failed(code) => {
                self.creation.hold_until = self.now + 10.0;
                tracing::warn!("bot creation failed with status {code}");
                None
            }
        }
    }

    fn new_identity(&mut self) -> Identity {
        let clients = &self.clients;
        let bots = &self.bots;
        let taken =
            |n: &str| clients.find_name(n).is_some() || bots.iter().any(|b| b.identity.name.eq_ignore_ascii_case(n));
        let name = self.names.pick(&mut self.rng, &self.config.bots.name_prefix, &taken);
        let models = &self.config.bots.models;
        let model = if models.is_empty() {
            "gordon".to_string()
        } else {
            models[self.rng.range_i32(0, models.len() as i32 - 1) as usize].clone()
        };
        Identity {
            name,
            model,
            difficulty: parse_difficulty(&self.config.bots.difficulty).unwrap_or(2),
            style: self.config.bots.style.clone(),
            topcolor: random_color(&mut self.rng),
            bottomcolor: random_color(&mut self.rng),
        }
    }

    pub fn kick_bot_index(&mut self, host: &mut dyn Host, index: usize, reason: &str) {
        let bot = &mut self.bots[index];
        if host.kick_bot(bot.id.slot, bot.id.generation, reason) {
            bot.set_state(BotState::Leaving, self.now);
            bot.kick_attempts = 1;
        } else {
            tracing::warn!("kick of {} failed; dropping it from the manager", bot.identity.name);
            self.bots.remove(index);
        }
    }

    /// A new client in a slot means its previous occupant is gone even if no disconnect was reported
    /// (Metamod-FWGS does not forward ClientDisconnect).
    fn drop_slot_ghosts(&mut self, slot: u8, new_gen: Option<u32>) {
        self.bots.retain(|b| {
            let ghost = b.id.slot == slot && Some(b.id.generation) != new_gen;
            if ghost {
                tracing::warn!(
                    "slot {slot} was taken before {} reported leaving ({}); dropping it (missed ClientDisconnect)",
                    b.identity.name,
                    b.state.as_str()
                );
            }
            !ghost
        });
    }

    /// Kicked or faulted bots whose disconnect never arrives: one more kick after 3 s, forgotten after 6 s.
    fn check_departures(&mut self, host: &mut dyn Host) {
        const RETRY_AFTER: f64 = 3.0;
        const GIVE_UP_AFTER: f64 = 6.0;
        let now = self.now;
        let mut i = 0;
        while i < self.bots.len() {
            let bot = &mut self.bots[i];
            if bot.is_active() {
                i += 1;
                continue;
            }
            let waited = now.since(bot.state_since);
            if waited >= GIVE_UP_AFTER {
                tracing::warn!(
                    "{} did not leave {GIVE_UP_AFTER} s after the kick; forgetting it",
                    bot.identity.name
                );
                self.bots.remove(i);
                continue;
            }
            if waited >= RETRY_AFTER && bot.kick_attempts < 2 {
                bot.kick_attempts = 2;
                host.kick_bot(bot.id.slot, bot.id.generation, "lambdabots");
            }
            i += 1;
        }
    }

    fn apply_safe_mode(&mut self, host: &mut dyn Host) {
        if self.safe_mode_applied {
            return;
        }
        self.safe_mode_applied = true;
        self.config.quota.count = 0;
        for i in (0..self.bots.len()).rev() {
            self.kick_bot_index(host, i, "lambdabots safe mode");
        }
        tracing::error!("safe mode: {}", self.safe_mode.as_deref().unwrap_or("?"));
    }

    // -----------------------------------------------------------------------------------------
    // Motor (M0: lifecycle, respawn protocol and scripted motor tests)
    // -----------------------------------------------------------------------------------------

    fn drive_bots(&mut self, host: &mut dyn Host) {
        let frame_ms = self.frame_time * 1000.0;
        let cmd_rate = self.config.engine.cmd_rate;
        let now = self.now;
        let freeze = self.freeze;
        self.cmds.clear();
        let mut faulted = Vec::new();
        for (i, bot) in self.bots.iter_mut().enumerate() {
            if matches!(bot.state, BotState::Leaving | BotState::Faulted) {
                continue;
            }
            let result = catch_unwind(AssertUnwindSafe(|| drive_one(bot, now, frame_ms, freeze)));
            match result {
                Ok(Some(cmd)) => self.cmds.push(cmd),
                Ok(None) => {}
                Err(payload) => {
                    let msg = panic_message(&payload);
                    tracing::error!("bot {} faulted: {msg}", bot.identity.name);
                    faulted.push(i);
                }
            }
        }
        for i in faulted.into_iter().rev() {
            self.stats.bot_faults += 1;
            self.bots[i].set_state(BotState::Faulted, now);
            self.bots[i].kick_attempts = 1;
            let (slot, generation) = (self.bots[i].id.slot, self.bots[i].id.generation);
            if !host.kick_bot(slot, generation, "lambdabots: bot fault") {
                self.bots.remove(i);
            }
        }
        for bot in &mut self.bots {
            for argv in bot.pending_client_cmds.drain(..) {
                let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
                host.bot_client_command(bot.id.slot, bot.id.generation, &refs);
            }
        }
        if self.cmds.is_empty() {
            return;
        }
        if !host.run_player_moves(&self.cmds, &mut self.feedback) {
            self.stats.move_calls_failed += 1;
            return;
        }
        self.stats.commands_sent += self.cmds.len() as u64;
        for fb in &self.feedback {
            if fb.status != lb_ffi::LB_MOVE_OK {
                self.stats.stale_moves += 1;
            }
        }
        let reports: Vec<String> = self
            .bots
            .iter_mut()
            .filter_map(|bot| {
                let done = bot.test.as_ref().is_some_and(|t| t.finished);
                if done {
                    bot.test.take().map(|t| t.report(&bot.identity.name, now, cmd_rate))
                } else {
                    None
                }
            })
            .collect();
        for r in reports {
            tracing::info!("{r}");
            logging::console_line(format!("[lambdabots] {r}"));
        }
    }

    fn poll_command_channel(&mut self, host: &mut dyn Host) {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let mut results = Vec::new();
        self.commands.poll(now_ms, &mut results);
        for r in results {
            match r {
                Ok(req) => {
                    tracing::info!("telemetry command from {}: lb {}", req.from, req.args);
                    let args: Vec<&str> = req.args.split_whitespace().collect();
                    let out = commands::execute(self, host, &args);
                    self.telemetry.send(
                        "cmd_result",
                        self.now.secs(),
                        &serde_json::json!({ "args": req.args, "out": out }),
                    );
                }
                Err((from, e)) => tracing::warn!("telemetry command from {from} rejected: {e}"),
            }
        }
    }

    fn emit_telemetry(&mut self) {
        if !self.telemetry.is_enabled() {
            return;
        }
        let hz = self.config.telemetry.hz.max(0.5) as f64;
        if self.now.since(self.last_telemetry) >= 1.0 / hz || self.now < self.last_telemetry {
            self.last_telemetry = self.now;
            let bots: Vec<serde_json::Value> = self
                .bots
                .iter()
                .map(|b| {
                    let body = &b.self_state.body;
                    serde_json::json!({
                        "slot": b.id.slot,
                        "n": b.identity.name,
                        "st": b.state.as_str(),
                        "o": [body.origin.x, body.origin.y, body.origin.z],
                        "ya": b.view.y,
                        "hp": body.health,
                        "ap": body.armor,
                        "w": b.self_state.current_weapon.get().map(|w| w.classname()),
                    })
                })
                .collect();
            let players: Vec<serde_json::Value> = self
                .others
                .iter()
                .map(|(slot, o, yaw, alive)| {
                    let name = self.clients.get(*slot).map(|c| c.name.clone()).unwrap_or_default();
                    serde_json::json!({ "slot": slot, "n": name, "o": vec3(*o), "ya": yaw, "al": alive })
                })
                .collect();
            self.telemetry.send(
                "frame",
                self.now.secs(),
                &serde_json::json!({ "bots": bots, "players": players }),
            );
        }
        if self.now.since(self.last_perf) >= 1.0 || self.now < self.last_perf {
            self.last_perf = self.now;
            let drivers: Vec<serde_json::Value> = self
                .bots
                .iter()
                .map(|b| serde_json::json!({ "slot": b.id.slot, "sent_ms": b.driver.total_sent_ms, "cmds": b.driver.commands_sent, "debt_dropped_ms": b.driver.dropped_debt_ms }))
                .collect();
            self.telemetry.send(
                "perf",
                self.now.secs(),
                &serde_json::json!({
                    "safe_mode": self.safe_mode,
                    "bots": self.bots.len(),
                    "stats": self.stats,
                    "core": self.core_times.percentiles(),
                    "drivers": drivers,
                }),
            );
            self.stats.max_frame_ms = 0.0;
        }
    }
}

/// Produces the command for one bot this frame, or `None` if nothing is due.
fn drive_one(bot: &mut Bot, now: SimTime, frame_ms: f64, freeze: bool) -> Option<LbBotCommand> {
    if bot.fault_on_next_frame {
        bot.fault_on_next_frame = false;
        panic!("injected fault (lb debug panic)");
    }
    let mut forward = 0.0f32;
    let mut side = 0.0f32;
    let mut buttons = 0u16;
    match bot.state {
        BotState::Respawning => {
            let phase = (now.secs() * 10.0) as u64 % 4;
            if phase >= 2 {
                buttons |= IN_JUMP;
            }
        }
        BotState::Alive if !freeze => {
            if let Some(test) = bot.test.as_mut() {
                let body = &bot.self_state.body;
                let on_ground = body.flags & lb_game::self_state::FL_ONGROUND != 0;
                let out = test.step(
                    now,
                    (frame_ms / 1000.0) as f32,
                    body.origin,
                    body.velocity,
                    on_ground,
                    body.maxspeed,
                );
                forward = out.forward;
                side = out.side;
                buttons |= out.buttons;
                test.yaw = normalize_angle(test.yaw + out.yaw_delta);
                bot.view.y = test.yaw;
                test.frame_ms_sum += frame_ms;
                test.frames += 1;
                if out.done {
                    test.finished = true;
                }
            }
        }
        _ => {}
    }
    buttons |= direction_buttons(forward, side);
    let sent = bot.driver.tick(frame_ms, buttons)?;
    if let Some(test) = bot.test.as_mut() {
        test.msec_sent += sent.msec as u64;
        test.commands += 1;
    }
    let seed = bot.rng.motor.next_u32();
    Some(LbBotCommand {
        slot: bot.id.slot,
        flags: LB_CMD_SET_SEED,
        buttons: sent.buttons,
        bot_gen: bot.id.generation,
        view_angles: LbVec3 {
            x: bot.view.x,
            y: bot.view.y,
            z: 0.0,
        },
        forwardmove: forward,
        sidemove: side,
        upmove: 0.0,
        impulse: 0,
        msec: sent.msec,
        pad: 0,
        random_seed: seed,
    })
}

pub fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        s.to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
}

pub fn vec3(v: Vec3) -> [f32; 3] {
    [v.x, v.y, v.z]
}
