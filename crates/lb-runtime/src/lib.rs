//! Runtime: frame pipeline, scheduler, workers, bot manager, commands and cvars.

#![forbid(unsafe_code)]

pub mod arms_stats;
pub mod capture;
pub mod clients;
pub mod commands;
pub mod cvars;
pub mod editor;
pub mod logging;
pub mod manager;
pub mod motor_test;
pub mod names;
pub mod nav;
pub mod nav_test;
pub mod perf;
pub mod record;
pub mod roster;
pub mod selftest;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::Arc;

use lb_combat::Armed;
use lb_config::MainConfig;
use lb_config::main_config::QuotaMode;
use lb_core::Vec3;
use lb_core::handles::{BotId, MapEpoch};
use lb_core::math::normalize_angle;
use lb_core::rng::{Pcg32, splitmix64};
use lb_core::time::SimTime;
use lb_ffi::{LB_CMD_SET_SEED, LbBotCommand, LbMoveFeedback, LbVec3};
use lb_game::compat::CompatibilityProfile;
use lb_game::dll::DllProfile;
use lb_game::entities::ProjectileKind;
use lb_game::items::{Ammo, ItemKind};
use lb_game::messages::{self, GameMsg};
use lb_game::mode::{GameModeKind, ModeInputs};
use lb_game::rules::{PublicRules, rule_cvars};
use lb_game::scoreboard::Scoreboard;
use lb_game::self_state::SelfState;
use lb_game::self_state::WeaponRegistry;
use lb_game::sounds::{classify_event, classify_sample};
use lb_game::weapons::{WeaponId, weapons_in_mask};
use lb_host::strings::StringTable;
use lb_host::{CreateBotOutcome, CreateBotRequest, Host, TrackRule};
use lb_knowledge::{BeliefParams, ItemSpot, PlayerKey, PublicEvent};
use lb_nav::known::LinkHealth;
use lb_perception::items::{ChargerEntity, ItemEntity};
use lb_perception::projectiles::ProjectileEntity;
use lb_perception::vision::DEFAULT_ASPECT;
use lb_perception::{Listener, SoundEvent, StepSynth, Subject, Viewer};
use lb_raw::{ClientEventKind, RawClient, RawEvent, RawFrame};
use lb_telemetry::{CommandChannel, TelemetrySink};
use lb_worldq::{AllVisible, VisSets};
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

use crate::clients::Clients;
use crate::cvars::{Cv, Cvars};
use crate::manager::{Bot, BotState, Creation, desired_bots, pick_bot_to_kick};
use crate::names::NamePool;
use crate::roster::{Roster, RosterFilter};
use lb_config::skill::{DifficultyFile, Presets, SkillBand};
use lb_game::input::*;
use lb_styles::{Persona, StyleId, StyleTable};

pub const CORE_VERSION: &str = env!("CARGO_PKG_VERSION");
/// Engine message of temporary entities (explosions among them).
const SVC_TEMPENTITY: i32 = 23;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InitData {
    pub adapter_version: String,
    pub plugin_path: PathBuf,
    pub game_dir: PathBuf,
    pub install_dir: PathBuf,
    pub platform: u8,
    pub late_load: bool,
    /// No sockets (telemetry, command channel): set for a replay, which must not talk to anyone.
    pub sandbox: bool,
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
    /// How the game DLL works the weapons that differ.
    pub dll: DllProfile,
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

/// A projectile as last seen, to credit its hits in `lb stats`.
#[derive(Clone, Copy, Debug)]
struct Launched {
    row: arms_stats::Row,
    /// Slot of the player who threw or fired it; the game clears some owners in flight, the first one seen is kept.
    owner: u16,
    origin: Vec3,
    velocity: Vec3,
    seen: SimTime,
    /// Where and when it was first seen: about where it was thrown or fired from.
    first: (SimTime, Vec3),
}

impl Launched {
    /// How far from `source` it could be now; `None` when it was seen too long ago to tell.
    fn miss(&self, source: Vec3, now: SimTime) -> Option<f32> {
        let dt = now.since(self.seen) as f32;
        if !(0.0..=LAUNCHED_MEMORY).contains(&dt) {
            return None;
        }
        let at = self.origin + self.velocity * dt.min(PROJECTILE_PERIOD);
        let slack = LAUNCHED_REACH + self.velocity.length() * PROJECTILE_PERIOD;
        Some(at.distance(source)).filter(|d| *d <= slack)
    }
}

/// A projectile's hit is reported this close to where it was last seen (or would be now), beyond its travel since.
const LAUNCHED_REACH: f32 = 72.0;
/// Seconds between projectile snapshots.
const PROJECTILE_PERIOD: f32 = 0.05;
/// A projectile gone from the snapshots is remembered this long: its blast and hits are reported after it went.
const LAUNCHED_MEMORY: f32 = 0.5;
/// A bot at the source of a bullet's damage is its shooter only with its trigger pulled this recently: damage from the
/// world (falls, hurt triggers) or from a human can be reported from where a bot stands too.
const TRIGGER_MEMORY: f64 = 0.3;

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
    /// Personalities that were playing before the map change; they come back first.
    pub carry_over: Vec<String>,
    /// Personalities an admin asked for with `lb add <name>`; they join next, whatever the filters say.
    pub requested: Vec<String>,
    /// The quota could not be filled; warned once until a bot joins again.
    roster_warned: bool,
    pub names: NamePool,
    pub roster: Roster,
    pub filter: RosterFilter,
    pub presets: Presets,
    pub presets_source: String,
    /// Trait ranges and goal weights of the play styles.
    pub styles: StyleTable,
    pub styles_source: String,
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
    /// `lb selftest` froze the other bots; the freeze to go back to when it ends.
    pub freeze_before_selftest: Option<bool>,
    pub game_mode_forced: i32,
    feedback: Vec<LbMoveFeedback>,
    cmds: Vec<LbBotCommand>,
    last_mono_ns: u64,
    /// Other players from this frame's snapshots, for telemetry only.
    others: Vec<(u8, Vec3, f32, bool)>,
    pub capture: Option<capture::MessageCapture>,
    /// Event names (PrecacheEvent) arrive with the first frame of a map; the profile is rebuilt then.
    compat_pending: bool,
    pub nav_loader: Option<nav::NavLoader>,
    pub graph: Option<Arc<lb_nav::NavGraph>>,
    /// Where the map's doors, lifts and breakables are now.
    pub mechs: nav::LiveMechs,
    /// Links switched off for every bot after several failed them.
    pub link_health: LinkHealth,
    /// Special links re-checked with live engine traces after the graph loads.
    pub live_check: lb_nav::probe::LiveCheck,
    /// What happened to the map's navigation graph, for `lb nav`.
    pub nav_status: String,
    /// Path search expansions saved up (`PLAN_RATE`, at most `PLAN_BURST`).
    plan_tokens: f64,
    /// The in-game editor, when a player has it on (`lb edit on`, needs `lb_editor 1`).
    pub editor: Option<editor::Editor>,
    pub editor_allowed: bool,
    /// The player whose `lb` command runs now; `None` for the server console.
    pub command_slot: Option<u8>,
    /// The map's overlays (places, graph patches), as last loaded.
    pub overlays: std::sync::Arc<Vec<lb_config::overlay::OverlayFile>>,
    /// PVS and PAS of the map; until they load, everything counts as potentially visible and audible.
    pub vis: Option<Arc<lb_bsp::MapVis>>,
    /// Player snapshots of this frame, kept from `frame_pre` for the senses in `frame_post`.
    clients_now: Vec<RawClient>,
    /// Sounds and weapon events of this frame.
    sounds_now: Vec<SoundEvent>,
    /// Kill feed of this frame.
    public_now: Vec<PublicEvent>,
    steps: StepSynth,
    /// Last weapon event of every slot, for the muzzle flash a viewer may see.
    last_shot: Vec<Option<SimTime>>,
    /// Weapon shown by an interned `weaponmodel` string.
    weapon_models: FxHashMap<u16, Option<WeaponId>>,
    /// The ReHLDS `SV_StartSound` hook reports every sound, footsteps included.
    sound_hook: bool,
    /// Items the map places; `None` until the map is loaded.
    pub item_spots: Option<Arc<Vec<ItemSpot>>>,
    /// Item entities as the server has them, refreshed a few times a second for perception.
    item_entities: Vec<ItemEntity>,
    item_kinds: FxHashMap<u16, Option<ItemKind>>,
    next_items_at: SimTime,
    /// Projectiles and mines as the server has them, refreshed at the vision rate.
    projectile_entities: Vec<ProjectileEntity>,
    projectile_kinds: FxHashMap<u16, Option<ProjectileKind>>,
    next_projectiles_at: SimTime,
    /// Projectiles by entity index as last seen, to credit their hits in `lb stats`.
    launched: FxHashMap<u16, Launched>,
    /// Explosions of this frame (`TE_EXPLOSION`).
    explosions_now: Vec<Vec3>,
    /// Where players spawn on this map; `None` until the map is loaded.
    pub spawns: Option<Arc<Vec<Vec3>>>,
    /// The map's wall chargers; `None` until the map is loaded.
    pub chargers: Option<Arc<Vec<lb_knowledge::ChargerSpot>>>,
    /// Charger faces as the server draws them, refreshed with the items.
    charger_entities: Vec<ChargerEntity>,
    /// Weapons bots may use, as a mask of weapon bits (`lb weapons`, for stand tests).
    pub weapons_allowed: u32,
    /// Weapons every bot is given when it spawns (`lb weapons ... give`; needs `sv_cheats 1`).
    pub weapons_give: Vec<WeaponId>,
    /// Rounds, damage, kills and suicides per weapon (`lb stats`).
    pub arms_stats: arms_stats::ArmsStats,
    /// Inputs from outside the engine (the navigation loader, the command channel), kept while recording and fed
    /// from the recording in a replay.
    pub outside: record::OutsideMode,
    /// `lb record start|stop`, for the recorder, which lives outside the runtime.
    pub record_request: Option<record::RecordRequest>,
    /// What the recorder is doing, for `lb record`.
    pub record_status: String,
}

impl Runtime {
    pub fn new(host: &mut dyn Host, init: InitData) -> Runtime {
        let config_path = init.install_dir.join("config").join("lambdabots.yaml");
        let (config, config_source, config_error) = match std::fs::read_to_string(&config_path) {
            Ok(text) => match MainConfig::parse(&text, &config_path.display().to_string()) {
                Ok(cfg) => (cfg, config_path.display().to_string(), None),
                Err(e) => (MainConfig::default(), "defaults".to_string(), Some(e.to_string())),
            },
            Err(e) => (
                MainConfig::default(),
                "defaults".to_string(),
                Some(format!("{} not readable ({e})", config_path.display())),
            ),
        };
        let log_dir = config.logging.file.then(|| init.install_dir.join("logs"));
        logging::init(
            log_dir.as_deref(),
            &config.logging.level,
            &config.logging.console_level,
            config.logging.max_files as usize,
        );
        if let Some(e) = &config_error {
            tracing::error!("config: {e}; using built-in defaults");
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
        let (presets, presets_source) = load_presets(&init.install_dir);
        let (styles, styles_source) = load_styles(&init.install_dir);
        let roster = Roster::load(&init.install_dir, &config.bots.models, &styles);
        let filter = RosterFilter::from_config(&config.roster);
        let mut rt = Runtime {
            names: NamePool::load(&init.install_dir, &config.bots.language),
            roster,
            filter,
            presets,
            presets_source,
            styles,
            styles_source,
            requested: Vec::new(),
            roster_warned: false,
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
            freeze_before_selftest: None,
            game_mode_forced: -1,
            feedback: Vec::new(),
            cmds: Vec::new(),
            last_mono_ns: 0,
            others: Vec::new(),
            capture: None,
            compat_pending: false,
            nav_loader: None,
            graph: None,
            mechs: nav::LiveMechs::default(),
            link_health: LinkHealth::default(),
            live_check: lb_nav::probe::LiveCheck::default(),
            nav_status: "no map".into(),
            plan_tokens: 0.0,
            editor: None,
            editor_allowed: false,
            command_slot: None,
            overlays: Default::default(),
            vis: None,
            clients_now: Vec::new(),
            sounds_now: Vec::new(),
            public_now: Vec::new(),
            steps: StepSynth::default(),
            last_shot: Vec::new(),
            weapon_models: FxHashMap::default(),
            sound_hook: false,
            item_spots: None,
            item_entities: Vec::new(),
            item_kinds: FxHashMap::default(),
            next_items_at: SimTime::ZERO,
            projectile_entities: Vec::new(),
            projectile_kinds: FxHashMap::default(),
            next_projectiles_at: SimTime::ZERO,
            launched: FxHashMap::default(),
            explosions_now: Vec::new(),
            spawns: None,
            chargers: None,
            charger_entities: Vec::new(),
            weapons_allowed: u32::MAX,
            weapons_give: Vec::new(),
            arms_stats: arms_stats::ArmsStats::default(),
            outside: record::OutsideMode::Live,
            record_request: None,
            record_status: "not recording".into(),
        };
        rt.register_cvars(host);
        rt.open_telemetry();
        let summary = rt.startup_summary();
        tracing::info!("{summary}");
        host.server_print(&format!("[lambdabots] {summary}\n"));
        rt
    }

    /// Where the core looked for its files and what it found; printed at start and by `lb status`.
    pub fn startup_summary(&self) -> String {
        format!(
            "core {CORE_VERSION} (adapter {}): install dir {}, config {}, {} names from {}, {}",
            self.init.adapter_version,
            self.init.install_dir.display(),
            self.config_source,
            self.names.len(),
            self.names
                .source
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "built-in list".into()),
            self.roster_summary(),
        )
    }

    pub fn roster_summary(&self) -> String {
        let from_profiles = self
            .roster
            .iter()
            .filter(|p| matches!(p.source, lb_styles::PersonaSource::Profile(_)))
            .count();
        format!(
            "{} personalities ({} from profiles, {} generated), {} admitted by the filter ({})",
            self.roster.len(),
            from_profiles,
            self.roster.len() - from_profiles,
            self.roster.count_admitted(&self.filter),
            self.filter.describe()
        )
    }

    /// `lb config reload`: re-reads the main config, the skill table, the names and the profiles. File values win
    /// over earlier console changes of the same cvars; bots in the game take their updated personality at once.
    pub fn reload(&mut self, host: &mut dyn Host) -> Vec<String> {
        let path = self.init.install_dir.join("config").join("lambdabots.yaml");
        let mut out = Vec::new();
        match std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|t| MainConfig::parse(&t, &path.display().to_string()).map_err(|e| e.to_string()))
        {
            Ok(cfg) => {
                self.config = cfg;
                self.config_source = path.display().to_string();
                out.push(format!("config: {}", self.config_source));
            }
            Err(e) => out.push(format!("config not reloaded, keeping the current one: {e}")),
        }
        let (presets, source) = load_presets(&self.init.install_dir);
        self.presets = presets;
        self.presets_source = source;
        let (styles, source) = load_styles(&self.init.install_dir);
        self.styles = styles;
        self.styles_source = source;
        self.names = NamePool::load(&self.init.install_dir, &self.config.bots.language);
        self.roster = Roster::load(&self.init.install_dir, &self.config.bots.models, &self.styles);
        self.filter = RosterFilter::from_config(&self.config.roster);
        let c = &self.config;
        let values = [
            (Cv::Quota, c.quota.count.to_string()),
            (Cv::QuotaMode, quota_mode_name(c.quota.mode).to_string()),
            (Cv::Difficulty, c.roster.difficulty.clone()),
            (Cv::Style, c.roster.styles.clone()),
            (Cv::CmdRate, c.engine.cmd_rate.to_string()),
            (Cv::ForceRespawn, (c.bots.force_respawn as u8).to_string()),
            (Cv::Editor, (c.access.editor_enabled as u8).to_string()),
        ];
        for (cv, value) in values {
            self.cvars.set(host, cv, &value);
        }
        self.editor_allowed = self.config.access.editor_enabled;
        if !self.editor_allowed {
            self.editor = None;
        }
        for bot in &mut self.bots {
            if let Some(p) = self.roster.get(&bot.persona.name) {
                let skill = p.skill_params(&self.presets);
                let affinity = self.styles.goals(p.style);
                bot.set_persona(p, skill, affinity);
            }
            bot.driver.set_rate(self.config.engine.cmd_rate as f64);
        }
        out.push(format!("skill table: {}", self.presets_source));
        out.push(format!("styles: {}", self.styles_source));
        out.push(format!("roster: {}", self.roster_summary()));
        out.extend(self.roster.problems.iter().map(|p| format!("problem: {p}")));
        out
    }

    fn register_cvars(&mut self, host: &mut dyn Host) {
        let c = &self.config;
        let mode = quota_mode_name(c.quota.mode);
        let defaults = vec![
            (Cv::Version, CORE_VERSION.to_string()),
            (Cv::Quota, c.quota.count.to_string()),
            (Cv::QuotaMode, mode.to_string()),
            (Cv::Difficulty, c.roster.difficulty.clone()),
            (Cv::Style, c.roster.styles.clone()),
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
            (Cv::NavSource, c.nav.source.name().to_string()),
            (Cv::Editor, (c.access.editor_enabled as u8).to_string()),
        ];
        self.editor_allowed = c.access.editor_enabled;
        self.cvars.register(host, &defaults);
    }

    fn open_telemetry(&mut self) {
        if self.init.sandbox {
            return;
        }
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
            let mut saved: Vec<String> = self.bots.drain(..).map(|b| b.persona.name.clone()).collect();
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
        self.compat_pending = true;
        self.graph = None;
        self.vis = None;
        self.clients_now.clear();
        self.sounds_now.clear();
        self.public_now.clear();
        self.steps.reset();
        self.last_shot = vec![None; max_clients as usize + 1];
        self.weapon_models.clear();
        self.item_spots = None;
        self.item_entities.clear();
        self.item_kinds.clear();
        self.next_items_at = SimTime::ZERO;
        self.projectile_entities.clear();
        self.projectile_kinds.clear();
        self.next_projectiles_at = SimTime::ZERO;
        self.launched.clear();
        self.explosions_now.clear();
        self.spawns = None;
        self.chargers = None;
        self.charger_entities.clear();
        self.nav_status = format!("loading the graph for {name}");
        self.editor = None;
        self.mechs.set_map(None, max_clients);
        self.link_health.clear();
        self.live_check = lb_nav::probe::LiveCheck::default();
        self.start_nav_load(name);
        tracing::info!(
            "map {name} (epoch {}, {} slots){}",
            epoch.0,
            max_clients,
            if late_load { ", late load" } else { "" }
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

    /// The graph around the editing player, a few times a second.
    fn draw_editor(&mut self, host: &mut dyn Host) {
        let (Some(ed), Some(graph)) = (self.editor.as_mut(), self.graph.as_deref()) else {
            return;
        };
        let Some(player) = self.clients_now.iter().find(|c| c.slot == ed.slot) else {
            return;
        };
        if let Some(prims) = ed.draw(self.now.secs(), graph, player.origin + player.view_ofs) {
            host.send_debug(ed.slot, &prims);
        }
    }

    /// Where the player in `slot` stands, from this frame's snapshot.
    pub fn client_origin(&self, slot: u8) -> Option<Vec3> {
        self.clients_now.iter().find(|c| c.slot == slot).map(|c| c.origin)
    }

    /// Loads the map's navigation on the worker: visibility, mechanisms and the graph.
    pub(crate) fn start_nav_load(&mut self, name: &str) {
        let opts = nav::LoadOptions {
            source: self.config.nav.source,
            import: import_options(&self.game.rules, self.config.nav.trust_imported),
            threads: self.config.nav.threads,
        };
        self.nav_loader = Some(nav::NavLoader::start(
            &self.init.game_dir,
            &self.init.install_dir,
            name,
            opts,
        ));
    }

    pub fn map_end(&mut self, _host: &mut dyn Host) {
        if self.config.bots.save_names {
            let saved: Vec<String> = self
                .bots
                .iter()
                .filter(|b| !matches!(b.state, BotState::Leaving | BotState::Faulted))
                .map(|b| b.persona.name.clone())
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
        const SVC_INTERMISSION: i32 = 30;
        for id in [SVC_TEMPENTITY, SVC_INTERMISSION] {
            mask[id as usize / 8] |= 1 << (id % 8);
        }
        if !host.set_capture_mask(&mask) {
            tracing::warn!("message capture mask was not accepted by the adapter");
        }
    }

    fn read_rules(&mut self, host: &mut dyn Host) {
        let mut rules = PublicRules::default();
        for name in rule_cvars() {
            if let Some(value) = self.cvars.game_value(host, name) {
                rules.apply_cvar(name, &value);
            }
        }
        self.game.rules = rules;
    }

    fn write_compat(&self) {
        for line in self.compat.to_yaml().lines() {
            tracing::debug!("compat: {line}");
        }
        let dir = self.init.install_dir.join("logs");
        let path = dir.join(format!("compat-{}.yaml", self.compat.map));
        if let Err(e) = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&path, self.compat.to_yaml())) {
            tracing::warn!("cannot write {}: {e}", path.display());
        }
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
        self.sound_hook = ch(lb_ffi::LB_CH_SV_STARTSOUND);
        let dll = DllProfile::resolve(&self.config.game.dll, bhl);
        if self.game.dll.kind != dll.kind || self.game.dll.detected != dll.detected {
            tracing::info!(
                "weapon rules: {} ({})",
                dll.kind.as_str(),
                if dll.detected { "detected" } else { "from the config" }
            );
        }
        self.game.dll = dll;
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
            weapon_rules: format!(
                "{} ({})",
                dll.kind.as_str(),
                if dll.detected { "detected" } else { "config" }
            ),
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
            event_names: self.strings.event_count(),
            sys_ticrate: ticrate,
            plugins,
            map: self.map.as_ref().map(|m| m.name.clone()).unwrap_or_default(),
            late_load: self.map.as_ref().is_some_and(|m| m.late_load),
            install_dir: self.init.install_dir.display().to_string(),
            config: self.config_source.clone(),
            names: self
                .names
                .source
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "built-in".into()),
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
                body.groundentity = s.groundentity;
                body.basevelocity = s.basevelocity;
                if !bot.view_initialized {
                    bot.view = s.v_angle;
                    bot.view_initialized = true;
                }
            }
        }
        self.refresh_predictions(host);
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
        if !self.sound_hook {
            self.steps.frame(
                &frame.clients,
                self.game.rules.footsteps,
                self.now,
                &mut self.sounds_now,
            );
        }
        self.clients_now = frame.clients;
        let force_respawn = self.config.bots.force_respawn;
        let delay = self.config.bots.respawn_delay;
        let now = self.now;
        for bot in &mut self.bots {
            let was_alive = bot.state == BotState::Alive;
            bot.update_lifecycle(now, force_respawn, delay);
            if bot.state == BotState::Alive && !was_alive {
                give_weapons(bot, &self.weapons_give);
            }
        }
        self.count_rounds();
        self.check_departures(host);
        if let Some(result) = self.poll_nav_loader() {
            let map = self.nav_loader.take().map(|l| l.map).unwrap_or_default();
            match result {
                Ok(loaded) => {
                    tracing::info!(
                        "{map}: {} visibility leaves ({} KiB of PVS and PAS)",
                        loaded.vis.visleafs(),
                        loaded.vis.memory() / 1024
                    );
                    self.vis = Some(loaded.vis);
                    tracing::info!("{map}: {} item spots", loaded.items.len());
                    for bot in &mut self.bots {
                        bot.brain.set_items(&loaded.items, self.now);
                    }
                    self.item_spots = Some(loaded.items);
                    for bot in &mut self.bots {
                        bot.brain.spawns = loaded.spawns.to_vec();
                        bot.brain.set_chargers(&loaded.chargers);
                    }
                    tracing::info!("{map}: {} wall chargers", loaded.chargers.len());
                    self.spawns = Some(loaded.spawns);
                    self.chargers = Some(loaded.chargers);
                    let max_clients = self.map.as_ref().map_or(32, |m| m.max_clients);
                    tracing::info!(
                        "{map}: {} movers, {} breakables",
                        loaded.mechs.movers,
                        loaded.mechs.breakables
                    );
                    self.mechs.set_map(Some(loaded.mechs), max_clients);
                    self.overlays = loaded.overlays;
                    match loaded.graph {
                        Ok(graph) => {
                            let s = &graph.stats;
                            self.nav_status = format!(
                                "{map}: {} nodes, {} links ({} rejected by the check, {} added from mechanisms: {}), {} ms, {} ({})",
                                s.nodes,
                                s.links,
                                s.invalid,
                                s.added,
                                s.kinds(),
                                loaded.millis,
                                loaded.origin,
                                graph.source
                            );
                            if !loaded.patches.is_empty() {
                                self.nav_status = format!("{}; {}", self.nav_status, loaded.patches);
                            }
                            tracing::info!("navigation graph {}", self.nav_status);
                            // Node numbers belong to one graph: paths and what was learned about links go with it.
                            if self.graph.is_some() {
                                for bot in &mut self.bots {
                                    bot.nav.clear();
                                }
                                self.link_health.clear();
                                self.live_check = lb_nav::probe::LiveCheck::default();
                                if let Some(ed) = self.editor.as_mut() {
                                    ed.mark = None;
                                }
                            }
                            self.graph = Some(graph);
                        }
                        Err(e) => {
                            self.nav_status = format!("{map}: {e}");
                            tracing::warn!("no navigation on {map}: bots will stand still ({e})");
                        }
                    }
                }
                Err(e) => {
                    self.nav_status = format!("{map}: {e}");
                    tracing::warn!("cannot read {map}: bots will stand still and see without the PVS pre-filter ({e})");
                }
            }
        }
        if self.compat_pending {
            self.compat_pending = false;
            self.build_compat(host);
            self.write_compat();
        }
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
        if self.now >= self.next_items_at {
            self.next_items_at = self.now + 0.2;
            self.refresh_items(host);
        }
        if self.now >= self.next_projectiles_at || self.now + 1.0 < self.next_projectiles_at {
            self.next_projectiles_at = self.now + lb_perception::vision::PERIOD;
            self.refresh_projectiles(host);
        }
    }

    /// Projectiles and mines for perception: kind, where each is, how it moves and whose it is.
    fn refresh_projectiles(&mut self, host: &mut dyn Host) {
        use lb_game::entities::{KIND_MINE, KIND_PROJECTILE, kind_mask};
        const EF_NODRAW: u32 = 128;
        let mut snapshots = Vec::new();
        host.snapshot_entities(kind_mask(&[KIND_PROJECTILE, KIND_MINE]), &mut snapshots);
        self.projectile_entities.clear();
        for e in &snapshots {
            let kind = *self
                .projectile_kinds
                .entry(e.classname_id)
                .or_insert_with(|| ProjectileKind::from_classname(&self.strings.string_lossy(e.classname_id)));
            let Some(kind) = kind else { continue };
            let v = |x: LbVec3| Vec3::new(x.x, x.y, x.z);
            self.launch_seen(e.ent.index, kind, e.model_id, e.owner.index, v(e.origin), v(e.velocity));
            if e.effects & EF_NODRAW != 0 {
                continue;
            }
            self.projectile_entities.push(ProjectileEntity {
                index: e.ent.index,
                kind,
                origin: v(e.origin),
                velocity: v(e.velocity),
                angles: v(e.angles),
                owner: e.owner.index,
            });
        }
    }

    /// A projectile in this frame's snapshot, for `lb stats`: an exploded grenade stays a moment where it burst, out
    /// of sight, and is remembered with the rest.
    fn launch_seen(
        &mut self,
        index: u16,
        kind: ProjectileKind,
        model_id: u16,
        owner: u16,
        origin: Vec3,
        velocity: Vec3,
    ) {
        use arms_stats::Row;
        let row = match kind {
            ProjectileKind::Bolt => Row::plain(WeaponId::Crossbow),
            ProjectileKind::Rocket => Row::plain(WeaponId::Rpg),
            // The hand grenade's model; the MP5's grenade has its own.
            ProjectileKind::Grenade if self.strings.string_lossy(model_id).ends_with("w_grenade.mdl") => {
                Row::plain(WeaponId::HandGrenade)
            }
            ProjectileKind::Grenade => Row::alt(WeaponId::Mp5),
            ProjectileKind::Hornet => Row::plain(WeaponId::Hornetgun),
            ProjectileKind::Snark => Row::plain(WeaponId::Snark),
            ProjectileKind::Satchel => Row::plain(WeaponId::Satchel),
            ProjectileKind::Tripmine => Row::plain(WeaponId::Tripmine),
        };
        let now = self.now;
        let known = self
            .launched
            .get(&index)
            .filter(|l| l.row == row && now.since(l.seen) <= f64::from(LAUNCHED_MEMORY))
            .map(|l| (l.owner, l.first));
        self.launched.insert(
            index,
            Launched {
                row,
                owner: known.map(|k| k.0).filter(|o| *o != 0).unwrap_or(owner),
                origin,
                velocity,
                seen: now,
                first: known.map_or((now, origin), |k| k.1),
            },
        );
    }

    /// Damage a bot took is credited to the bot that fired a bullet (standing at the reported source) or to the owner
    /// of a projectile seen there; with neither it came from an explosion of no bot's (or from a human).
    fn credit_damage(&mut self, victim: (u8, Vec3), source: Vec3, damage: f32) {
        use arms_stats::Row;
        let now = self.now;
        self.launched
            .retain(|_, l| now.since(l.seen) <= f64::from(LAUNCHED_MEMORY));
        let shooter = self
            .bots
            .iter()
            .filter(|b| b.id.slot != victim.0 && b.state == BotState::Alive)
            .filter(|b| {
                b.brain
                    .motor
                    .weapon
                    .fired_at
                    .is_some_and(|t| now.since(t) <= TRIGGER_MEMORY)
            })
            .filter_map(|b| {
                let miss = b.self_state.body.origin.distance(source);
                let w = b.self_state.current_weapon.get()?;
                // A hitscan crossbow bolt is a zoomed shot.
                let row = if w == WeaponId::Crossbow {
                    Row::alt(w)
                } else {
                    Row::plain(w)
                };
                (miss <= arms_stats::SOURCE_MATCH).then_some((miss, u16::from(b.id.slot), row))
            });
        // The game never lets a hornet sting the one who fired it.
        let own_hornet = |l: &Launched| l.row.weapon == WeaponId::Hornetgun && l.owner == u16::from(victim.0);
        let thrown = self
            .launched
            .values()
            .filter(|l| !own_hornet(l))
            .filter_map(|l| l.miss(source, now).map(|miss| (miss, l.owner, l.row)));
        let best = shooter.chain(thrown).min_by(|a, b| a.0.total_cmp(&b.0));
        if let Some((_, owner, row)) = best
            && owner == u16::from(victim.0)
        {
            let first = self
                .launched
                .values()
                .filter(|l| l.owner == owner && l.row == row)
                .min_by(|a, b| a.origin.distance(source).total_cmp(&b.origin.distance(source)))
                .map(|l| l.first);
            if let Some((at, from)) = first {
                tracing::info!(
                    "own blast: slot {owner} took {damage:.0} from its {row:?} {:.0} u away; fired {:.1} s before from \
                     {:.0} u off the blast, {:.0} u from where the bot is now",
                    victim.1.distance(source),
                    now.since(at),
                    from.distance(source),
                    from.distance(victim.1),
                );
            }
        }
        let owner = best.and_then(|(_, owner, row)| {
            let bot = self
                .bots
                .iter()
                .find(|b| u16::from(b.id.slot) == owner && b.is_active())?;
            Some((bot.id.slot, bot.self_state.body.origin, row))
        });
        match owner {
            Some((slot, _, _)) if slot == victim.0 => self.arms_stats.hurt_self(damage),
            Some((_, at, row)) => self.arms_stats.hit(row, damage, at.distance(victim.1)),
            None if best.is_none() && victim.1.distance(source) > arms_stats::SOURCE_MATCH => {
                self.arms_stats.blasted(damage)
            }
            None => {
                // A charged gauss's bursts on walls report their shooter as the source: logged to tell what hurt it.
                if let Some(b) = self.bots.iter().find(|b| b.id.slot == victim.0)
                    && b.self_state.current_weapon.get() == Some(WeaponId::Gauss)
                    && damage > 0.0
                {
                    let g = &b.brain.mind.arms.gauss;
                    tracing::info!(
                        "unattributed damage with the gauss in hand: slot {} took {damage:.0} ({}, {:.1} s of charge)",
                        victim.0,
                        g.phase(),
                        g.charge(now)
                    );
                }
            }
        }
    }

    /// Rounds each live bot fired since the last frame from the weapon in its hands, for `lb stats` (weapons share
    /// ammo, so only the one in hand counts; a switch starts its count afresh).
    fn count_rounds(&mut self) {
        use arms_stats::Row;
        // A shot seen this soon after the scope came off was fired through it (a reload takes it off at once).
        const SCOPE_LINGER: f64 = 0.3;
        // Each its own ammo: counted whatever is in hand (the bot switches back right after a throw).
        const THROWABLES: [WeaponId; 4] = [
            WeaponId::HandGrenade,
            WeaponId::Satchel,
            WeaponId::Snark,
            WeaponId::Tripmine,
        ];
        let now = self.now;
        for bot in &mut self.bots {
            let fov = bot.self_state.body.fov;
            if fov > 0.0 && fov < 89.0 {
                bot.zoomed_at = Some(now);
            }
            let alive = bot.state == BotState::Alive;
            let arsenal = arsenal(&bot.self_state, &self.game.weapons);
            let distance = bot
                .brain
                .mind
                .target
                .and_then(|k| bot.brain.beliefs.track(k))
                .map_or(600.0, |t| t.pos.distance(bot.self_state.body.origin));
            let carried = THROWABLES.map(|w| arsenal.iter().find(|a| a.id == w).and_then(|a| a.reserve).unwrap_or(0));
            if alive && let Some(last) = bot.carried {
                for ((w, before), now_carried) in THROWABLES.iter().zip(last).zip(carried) {
                    if before > now_carried {
                        self.arms_stats
                            .fired(Row::plain(*w), (before - now_carried) as u32, distance);
                    }
                }
            }
            bot.carried = alive.then_some(carried);
            let weapon = bot
                .self_state
                .current_weapon
                .get()
                .filter(|w| alive && !THROWABLES.contains(w));
            let rounds = weapon.and_then(|w| {
                let a = arsenal.iter().find(|a| a.id == w)?;
                Some((w, a.rounds()?, a.reserve2.unwrap_or(0)))
            });
            if let (Some((w, primary, secondary)), Some((last_w, last_primary, last_secondary))) = (rounds, bot.rounds)
                && last_w == w
            {
                if last_primary > primary {
                    let scoped = w == WeaponId::Crossbow && bot.zoomed_at.is_some_and(|t| now.since(t) < SCOPE_LINGER);
                    let row = if scoped { Row::alt(w) } else { Row::plain(w) };
                    self.arms_stats.fired(row, (last_primary - primary) as u32, distance);
                }
                if w == WeaponId::Mp5 && last_secondary > secondary {
                    self.arms_stats
                        .fired(Row::alt(w), (last_secondary - secondary) as u32, distance);
                }
            }
            bot.rounds = rounds;
        }
    }

    /// Weapon prediction data of live bots, 50 times a second.
    fn refresh_predictions(&mut self, host: &mut dyn Host) {
        use lb_game::self_state::{PredictedWeapon, Prediction};
        let now = self.now;
        for bot in &mut self.bots {
            if bot.state != BotState::Alive
                || bot
                    .self_state
                    .prediction
                    .as_ref()
                    .is_some_and(|p| now.since(p.at) < 0.02 && now >= p.at)
            {
                continue;
            }
            let Some(ws) = host.weapon_state(bot.id.slot) else {
                continue;
            };
            let mut weapons = [None; 32];
            for (i, w) in ws.weapons.iter().enumerate() {
                if w.id > 0 {
                    weapons[i] = Some(PredictedWeapon {
                        clip: w.clip,
                        next_primary: w.next_primary,
                        next_secondary: w.next_secondary,
                        reloading: w.in_reload != 0 || w.in_special_reload != 0,
                        charge_ready: w.iuser1,
                        in_attack: w.iuser2,
                        fire_state: w.iuser3,
                        start_throw: w.fuser2,
                    });
                }
            }
            bot.self_state.prediction = Some(Prediction {
                at: now,
                current: WeaponId::from_id(ws.current),
                next_attack: ws.next_attack,
                primary_ammo: ws.primary_ammo,
                weapons,
            });
        }
    }

    /// Item entities for perception: where each one is and whether it is there to take; charger faces.
    fn refresh_items(&mut self, host: &mut dyn Host) {
        const EF_NODRAW: u32 = 128;
        let mut snapshots = Vec::new();
        host.snapshot_entities(1 << lb_game::entities::KIND_CHARGER, &mut snapshots);
        self.charger_entities.clear();
        for e in &snapshots {
            let model = self
                .strings
                .string_lossy(e.model_id)
                .strip_prefix('*')
                .and_then(|m| m.parse::<u16>().ok());
            if let Some(model) = model {
                self.charger_entities.push(ChargerEntity { model, frame: e.frame });
            }
        }
        snapshots.clear();
        host.snapshot_entities(1 << lb_game::entities::KIND_ITEM, &mut snapshots);
        self.item_entities.clear();
        for e in &snapshots {
            let kind = *self
                .item_kinds
                .entry(e.classname_id)
                .or_insert_with(|| ItemKind::from_classname(&self.strings.string_lossy(e.classname_id)));
            let Some(kind) = kind else { continue };
            self.item_entities.push(ItemEntity {
                origin: Vec3::new(e.origin.x, e.origin.y, e.origin.z),
                kind,
                drawn: e.effects & EF_NODRAW == 0 && e.owner.index == 0,
            });
        }
    }

    pub fn frame_post(&mut self, host: &mut dyn Host, mono_ns: u64) {
        if self.safe_mode.is_none() {
            self.drive_bots(host);
        }
        self.draw_editor(host);
        self.sounds_now.clear();
        self.public_now.clear();
        self.explosions_now.clear();
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
                            tracing::info!("bot {} left ({})", bot.persona.name, bot.state.as_str());
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
            RawEvent::UserMsg(m) if m.msg_id == SVC_TEMPENTITY => self.on_temp_entity(&m),
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
                            GameMsg::Damage {
                                armor,
                                health,
                                bits,
                                source,
                            } => {
                                bot.self_state.apply(&msg, self.now);
                                let felt = lb_perception::damage::stimulus(
                                    self.now,
                                    *health,
                                    *armor,
                                    *bits,
                                    *source,
                                    bot.self_state.body.origin,
                                    bot.view,
                                    &mut bot.rng.perception,
                                );
                                if let Some(d) = felt {
                                    bot.brain.on_damage(&d);
                                }
                                let victim = (bot.id.slot, bot.self_state.body.origin);
                                self.credit_damage(victim, *source, (*health + *armor) as f32);
                            }
                            _ => bot.self_state.apply(&msg, self.now),
                        }
                    }
                } else {
                    self.game.scoreboard.apply(&msg);
                    match &msg {
                        GameMsg::GameMode { teamplay } => self.game.teamplay_message = *teamplay,
                        GameMsg::DeathMsg { killer, victim, weapon } => {
                            tracing::debug!("kill: {killer} -> {victim} ({weapon})");
                            let ours = |slot: u8| self.bots.iter().any(|b| b.id.slot == slot && b.is_active());
                            self.arms_stats.death(
                                weapon,
                                *killer != 0 && killer != victim && ours(*killer),
                                killer == victim && ours(*victim),
                                ours(*victim),
                            );
                            self.public_now.push(PublicEvent::Death {
                                t: self.now,
                                killer: (*killer != 0 && killer != victim).then_some(*killer),
                                victim: *victim,
                                weapon: weapon.clone(),
                            });
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
                    bot.brain.motor.set_view(bot.view);
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
            RawEvent::Sound(s) => self.on_sound(&s),
            RawEvent::Playback(p) => self.on_playback(&p),
            RawEvent::RegisterMsg { .. } | RawEvent::PrecacheEvent { .. } => {}
            RawEvent::Entity(_) => {}
        }
    }

    /// Temporary entities: an explosion is seen (its flash) and heard (the client plays its sound) like other sounds.
    fn on_temp_entity(&mut self, m: &lb_core::msg::UserMsg) {
        const TE_EXPLOSION: i32 = 3;
        if m.int(0) != Some(TE_EXPLOSION) {
            return;
        }
        let (Some(x), Some(y), Some(z)) = (m.float(1), m.float(2), m.float(3)) else {
            return;
        };
        let at = Vec3::new(x, y, z);
        self.explosions_now.push(at);
        self.sounds_now.push(SoundEvent {
            t: self.now,
            source: None,
            origin: at,
            class: lb_game::sounds::SoundClass {
                kind: lb_game::sounds::SoundKind::Explosion,
                weapon: None,
            },
            volume: 1.0,
            attenuation: lb_game::sounds::ATTN_NORM,
            global: false,
        });
    }

    fn player_slot(&self, index: u16) -> Option<u8> {
        let max = self.map.as_ref().map_or(0, |m| m.max_clients);
        (index >= 1 && u32::from(index) <= max).then_some(index as u8)
    }

    fn on_sound(&mut self, s: &lb_raw::RawSound) {
        const SND_STOP: i32 = 1 << 5;
        const SND_CHANGE_VOL: i32 = 1 << 6;
        const SND_CHANGE_PITCH: i32 = 1 << 7;
        const SND_SPAWNING: i32 = 1 << 8;
        if s.flags & (SND_STOP | SND_CHANGE_VOL | SND_CHANGE_PITCH | SND_SPAWNING) != 0 || s.volume <= 0.0 {
            return;
        }
        self.sounds_now.push(SoundEvent {
            t: self.now,
            source: self.player_slot(s.entity.index),
            origin: s.origin,
            class: classify_sample(&s.sample),
            volume: s.volume,
            attenuation: s.attenuation,
            global: s.attenuation <= 0.0,
        });
    }

    fn on_playback(&mut self, p: &lb_raw::RawPlayback) {
        const FEV_GLOBAL: i32 = 1 << 2;
        const FEV_HOSTONLY: i32 = 1 << 4;
        let Some(heard) = self
            .strings
            .event_name(i32::from(p.event_index))
            .and_then(classify_event)
        else {
            return;
        };
        let source = self.player_slot(p.invoker.index);
        if heard.class.kind == lb_game::sounds::SoundKind::Shot
            && let Some(slot) = source
            && let Some(t) = self.last_shot.get_mut(slot as usize)
        {
            *t = Some(self.now);
        }
        if p.flags & FEV_HOSTONLY != 0 {
            return;
        }
        let origin = if p.origin == Vec3::ZERO {
            p.invoker_origin
        } else {
            p.origin
        };
        self.sounds_now.push(SoundEvent {
            t: self.now,
            source,
            origin,
            class: heard.class,
            volume: heard.volume,
            attenuation: heard.attenuation,
            global: p.flags & FEV_GLOBAL != 0,
        });
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
        self.command_slot = Some(slot);
        let lines = commands::execute(self, host, &refs);
        self.command_slot = None;
        for line in lines {
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
                Cv::Difficulty => match SkillBand::parse(&value) {
                    Some(band) => {
                        self.config.roster.difficulty = value.trim().to_string();
                        self.filter.skill = band;
                        tracing::info!("roster filter: {}", self.filter.describe());
                    }
                    None => tracing::warn!(
                        "lb_difficulty `{value}`: expected any, noob..expert, a number or a range like normal-hard"
                    ),
                },
                Cv::Style => match roster::parse_styles(&value) {
                    Some(styles) => {
                        self.config.roster.styles = value.trim().to_string();
                        self.filter.styles = styles;
                        tracing::info!("roster filter: {}", self.filter.describe());
                    }
                    None => tracing::warn!("lb_style `{value}`: expected any or a comma list of styles"),
                },
                Cv::CmdRate => {
                    if let Ok(r) = value.trim().parse::<f32>() {
                        self.config.engine.cmd_rate = r.clamp(0.0, 1000.0);
                        for bot in &mut self.bots {
                            bot.driver.set_rate(self.config.engine.cmd_rate as f64);
                        }
                    }
                }
                Cv::ForceRespawn => self.config.bots.force_respawn = value.trim() != "0",
                Cv::Editor => {
                    self.editor_allowed = value.trim() != "0";
                    if !self.editor_allowed {
                        self.editor = None;
                    }
                }
                Cv::NavSource => match lb_config::main_config::NavSource::parse(&value) {
                    Some(source) => {
                        self.config.nav.source = source;
                        tracing::info!("navigation graphs: {} from the next map on", source.name());
                    }
                    None => tracing::warn!("lb_nav_source `{value}`: expected generated or yapb"),
                },
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

    /// Adds the next personality (see [`Runtime::next_persona`]); returns its name.
    pub fn create_bot(&mut self, host: &mut dyn Host) -> Option<String> {
        let Some(persona) = self.next_persona() else {
            if !self.roster_warned {
                self.roster_warned = true;
                tracing::warn!(
                    "no personality can join: the roster has none left for the filter ({}) and generation is {}",
                    self.filter.describe(),
                    if self.config.roster.generate.enabled {
                        "out of names"
                    } else {
                        "off"
                    }
                );
            }
            self.creation.hold_until = self.now + 10.0;
            return None;
        };
        let display = lb_config::names::sanitize_name(&format!("{}{}", self.config.bots.name_prefix, persona.name));
        let mut infokeys = vec![
            ("model".to_string(), persona.model.clone()),
            ("topcolor".to_string(), persona.colors[0].to_string()),
            ("bottomcolor".to_string(), persona.colors[1].to_string()),
        ];
        if self.config.disguise.scoreboard_bot_flag {
            infokeys.push(("*bot".to_string(), "1".to_string()));
        }
        let req = CreateBotRequest {
            name: display,
            infokeys,
        };
        match host.create_bot(&req) {
            CreateBotOutcome::Created { slot, userid, bot_gen } => {
                let id = BotId {
                    slot,
                    generation: bot_gen,
                };
                let skill = persona.skill_params(&self.presets);
                tracing::info!(
                    "bot {} joined (slot {slot}, #{userid}): {}, skill {}, {}",
                    persona.name,
                    persona.style,
                    persona.skill,
                    persona.source.short()
                );
                let mut bot = Bot::new(
                    id,
                    userid,
                    persona.clone(),
                    skill,
                    self.styles.goals(persona.style),
                    self.now,
                    self.master_seed,
                    self.config.engine.cmd_rate as f64,
                    self.config.engine.max_cmd_debt_ms as f64,
                );
                if let Some(spots) = &self.item_spots {
                    bot.brain.set_items(spots, self.now);
                }
                if let Some(spawns) = &self.spawns {
                    bot.brain.spawns = spawns.to_vec();
                }
                if let Some(chargers) = &self.chargers {
                    bot.brain.set_chargers(chargers);
                }
                self.bots.push(bot);
                self.roster_warned = false;
                Some(persona.name.clone())
            }
            CreateBotOutcome::ServerFull => {
                self.creation.hold_until = self.now + 10.0;
                tracing::warn!("cannot add a bot: server is full");
                None
            }
            CreateBotOutcome::Rejected(reason) => {
                self.creation.hold_until = self.now + 10.0;
                tracing::warn!("bot {} was rejected: {reason}", persona.name);
                None
            }
            CreateBotOutcome::Failed(code) => {
                self.creation.hold_until = self.now + 10.0;
                tracing::warn!("bot creation failed with status {code}");
                None
            }
        }
    }

    /// A personality's nickname is in use: by one of our bots or by a connected player.
    fn name_busy(clients: &Clients, bots: &[Bot], prefix: &str, name: &str) -> bool {
        bots.iter()
            .any(|b| b.is_active() && b.persona.name.eq_ignore_ascii_case(name))
            || clients.find_name(name).is_some()
            || (!prefix.is_empty() && clients.find_name(&format!("{prefix}{name}")).is_some())
    }

    /// Who joins next, in order: a personality an admin asked for (`lb add <name>`), one that played before the map
    /// change, a new personality while the filtered roster is smaller than `roster.generate.pool`, a weighted pick
    /// among the admitted ones, and a new personality when everyone admitted is already playing.
    pub fn next_persona(&mut self) -> Option<Arc<Persona>> {
        let (clients, bots, prefix) = (&self.clients, &self.bots, self.config.bots.name_prefix.as_str());
        let busy = |name: &str| Self::name_busy(clients, bots, prefix, name);
        while let Some(name) = self.requested.pop() {
            match self.roster.get(&name) {
                Some(p) if !busy(&p.name) => return Some(p),
                Some(p) => tracing::warn!("{} is already playing", p.name),
                None => tracing::warn!("no personality named {name}"),
            }
        }
        while let Some(name) = self.carry_over.pop() {
            if let Some(p) = self.roster.get(&name)
                && self.filter.admits(&p)
                && !busy(&p.name)
            {
                return Some(p);
            }
        }
        let generate = &self.config.roster.generate;
        if generate.enabled
            && (self.roster.count_admitted(&self.filter) as u32) < generate.pool
            && let Some(p) = self.generate_persona()
        {
            return Some(p);
        }
        let (clients, bots, prefix) = (&self.clients, &self.bots, self.config.bots.name_prefix.as_str());
        let busy = |name: &str| Self::name_busy(clients, bots, prefix, name);
        if let Some(p) = self.roster.pick(&self.filter, &busy, &mut self.rng) {
            return Some(p);
        }
        if self.config.roster.generate.enabled {
            return self.generate_persona();
        }
        None
    }

    /// A personality for a nickname from `names/<language>.yaml` that has none yet; saved to `data/profiles.yaml`.
    fn generate_persona(&mut self) -> Option<Arc<Persona>> {
        let (clients, bots, prefix, roster) = (
            &self.clients,
            &self.bots,
            self.config.bots.name_prefix.as_str(),
            &self.roster,
        );
        let used = |name: &str| roster.knows(name) || Self::name_busy(clients, bots, prefix, name);
        let name = self.names.pick_unused(&mut self.rng, &used)?;
        let allowed = |s: &StyleId| self.filter.styles.is_empty() || self.filter.styles.contains(s);
        let mut weights: Vec<(StyleId, f32)> = self
            .config
            .roster
            .generate
            .styles
            .iter()
            .filter_map(|(k, w)| StyleId::parse(k).map(|s| (s, *w)))
            .filter(|(s, w)| allowed(s) && *w > 0.0)
            .collect();
        if weights.is_empty() {
            weights = StyleId::ALL
                .into_iter()
                .filter(|s| allowed(s))
                .map(|s| (s, 1.0))
                .collect();
        }
        let persona = lb_styles::generate(
            &name,
            &weights,
            self.filter.skill,
            &self.config.bots.models,
            &self.styles,
            &mut self.rng,
        );
        let persona = self.roster.add_generated(persona, &roster::today());
        tracing::info!(
            "new personality {}: {}, skill {} ({})",
            persona.name,
            persona.style,
            persona.skill,
            persona.source.describe()
        );
        Some(persona)
    }

    pub fn kick_bot_index(&mut self, host: &mut dyn Host, index: usize, reason: &str) {
        let bot = &mut self.bots[index];
        if host.kick_bot(bot.id.slot, bot.id.generation, reason) {
            bot.set_state(BotState::Leaving, self.now);
            bot.kick_attempts = 1;
        } else {
            tracing::warn!("kick of {} failed; dropping it from the manager", bot.persona.name);
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
                    b.persona.name,
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
                    bot.persona.name
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

    /// Team index of every slot from the public scoreboard; all 0 unless the mode has teams.
    fn teams(&self) -> Vec<u8> {
        let slots = self.clients_now.len().max(self.game.scoreboard.entries.len());
        let mut teams = vec![0u8; slots + 1];
        let team_mode = matches!(
            self.game.mode,
            Some(lb_game::mode::GameModeKind::Teamplay | lb_game::mode::GameModeKind::GunGame { team: true })
        );
        if !team_mode {
            return teams;
        }
        let mut names: Vec<String> = self
            .game
            .rules
            .teamlist
            .iter()
            .map(|t| t.to_ascii_lowercase())
            .collect();
        for (slot, e) in self.game.scoreboard.entries.iter().enumerate() {
            let name = e.team.trim().to_ascii_lowercase();
            if name.is_empty() || slot >= teams.len() {
                continue;
            }
            let i = names.iter().position(|n| *n == name).unwrap_or_else(|| {
                names.push(name);
                names.len() - 1
            });
            teams[slot] = (i + 1).min(255) as u8;
        }
        teams
    }

    fn drive_bots(&mut self, host: &mut dyn Host) {
        self.mechs.refresh(host, &self.strings, self.now.secs());
        self.run_live_check(host);
        let frame_ms = self.frame_time * 1000.0;
        let cmd_rate = self.config.engine.cmd_rate;
        let now = self.now;
        let freeze = self.freeze;
        self.cmds.clear();
        let mut faulted = Vec::new();
        let graph = self.graph.clone();
        let stuck_kill = f64::from(self.config.bots.stuck_kill_time);
        let teams = self.teams();
        let vis_sets = self.vis.clone();
        let subjects = subjects(
            &self.clients_now,
            &self.strings,
            &mut self.weapon_models,
            &self.last_shot,
            &teams,
        );
        let world = Senses {
            now,
            subjects: &subjects,
            sounds: &self.sounds_now,
            public: &self.public_now,
            vis: vis_sets
                .as_deref()
                .map_or(&AllVisible as &dyn VisSets, |v| v as &dyn VisSets),
            maxspeed: self.game.rules.maxspeed,
            teams: &teams,
            items: &self.item_entities,
            chargers: &self.charger_entities,
            projectiles: &self.projectile_entities,
            explosions: &self.explosions_now,
        };
        let opponents = subjects.len().saturating_sub(1);
        let registry = &self.game.weapons;
        let gravity = self.game.rules.gravity;
        let damages = self.game.rules.damages;
        let dll = self.game.dll;
        let allowed = self.weapons_allowed;
        let mut recognized = Vec::new();
        let mechs = &self.mechs;
        let link_health = &mut self.link_health;
        let mut tracer = nav::LiveTracer { host, count: 0 };
        // Path search expansions this frame, shared by the bots: a steady rate with a cap per frame.
        self.plan_tokens = (self.plan_tokens + PLAN_RATE * frame_ms / 1000.0).min(PLAN_BURST);
        let mut plan_budget = self.plan_tokens as u32;
        let budget_start = plan_budget;
        for (i, bot) in self.bots.iter_mut().enumerate() {
            if matches!(bot.state, BotState::Leaving | BotState::Faulted) {
                continue;
            }
            let result = catch_unwind(AssertUnwindSafe(|| {
                sense(bot, &world, &mut tracer, &mut recognized);
                let ctx = DriveCtx {
                    now,
                    frame_ms,
                    freeze,
                    graph: graph.as_deref(),
                    stuck_kill,
                    registry,
                    opponents,
                    mechs,
                    gravity,
                    damages,
                    dll,
                    allowed,
                    projectiles: &self.projectile_entities,
                };
                drive_one(bot, &ctx, &mut tracer, link_health, &mut plan_budget)
            }));
            match result {
                Ok(Some(cmd)) => self.cmds.push(cmd),
                Ok(None) => {}
                Err(payload) => {
                    let msg = panic_message(&payload);
                    tracing::error!("bot {} faulted: {msg}", bot.persona.name);
                    faulted.push(i);
                }
            }
        }
        self.plan_tokens -= f64::from(budget_start - plan_budget);
        let host = tracer.host;
        for (bot, r) in recognized {
            let name = self.clients.get(r.who.slot).map(|c| c.name.clone()).unwrap_or_default();
            tracing::debug!(
                "{bot} {} {name} after {:.2} s at {:.0} u",
                if r.reacquired { "found again" } else { "recognized" },
                r.latency,
                r.distance
            );
            self.telemetry.send(
                "event",
                now.secs(),
                &serde_json::json!({
                    "kind": "seen", "bot": bot, "who": r.who.slot, "name": name,
                    "latency": r.latency, "dist": r.distance, "again": r.reacquired,
                }),
            );
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
        let mut verdict = None;
        let testing = self.bots.iter().any(|b| b.selftest.is_some());
        if !testing && let Some(freeze) = self.freeze_before_selftest.take() {
            self.freeze = freeze;
        }
        for bot in &mut self.bots {
            if bot.selftest.as_ref().is_some_and(|t| t.finished)
                && let Some(t) = bot.selftest.take()
            {
                verdict = t.verdict.or(verdict);
            }
        }
        if let Some(dll) = verdict
            && dll.kind != self.game.dll.kind
        {
            tracing::warn!("weapon rules switched to {} for this session", dll.kind.as_str());
            self.game.dll = dll;
        }
        let reports: Vec<String> = self
            .bots
            .iter_mut()
            .filter_map(|bot| {
                let done = bot.test.as_ref().is_some_and(|t| t.finished);
                if done {
                    bot.test.take().map(|t| t.report(&bot.persona.name, now, cmd_rate))
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

    /// A slice of the live check of special links: up to 64 traces a frame (a count, not a time, so a replay runs
    /// the same slices).
    fn run_live_check(&mut self, host: &mut dyn Host) {
        if self.live_check.done {
            return;
        }
        let Some(graph) = self.graph.clone() else { return };
        let mut tracer = nav::LiveTracer { host, count: 0 };
        let wrong = self.live_check.run(&graph.probes, &mut tracer, 64, &mut || true);
        for (a, b) in wrong {
            self.link_health.disable(a, b);
            tracing::warn!("link {a} -> {b} looks different on the server than in the map file; switched off");
        }
        if self.live_check.done {
            tracing::info!("{}", self.live_check.progress(graph.probes.len()));
        }
    }

    fn poll_command_channel(&mut self, host: &mut dyn Host) {
        for line in self.channel_commands() {
            let args: Vec<&str> = line.split_whitespace().collect();
            let out = commands::execute(self, host, &args);
            self.telemetry.send(
                "cmd_result",
                self.now.secs(),
                &serde_json::json!({ "args": line, "out": out }),
            );
        }
    }

    /// Commands the telemetry command channel accepted since the last frame.
    fn read_command_channel(&mut self) -> Vec<String> {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let mut results = Vec::new();
        self.commands.poll(now_ms, &mut results);
        results
            .into_iter()
            .filter_map(|r| match r {
                Ok(req) => {
                    tracing::info!("telemetry command from {}: lb {}", req.from, req.args);
                    Some(req.args)
                }
                Err((from, e)) => {
                    tracing::warn!("telemetry command from {from} rejected: {e}");
                    None
                }
            })
            .collect()
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
                    let tracks: Vec<serde_json::Value> = b
                        .brain
                        .beliefs
                        .tracks
                        .iter()
                        .map(|t| {
                            serde_json::json!({
                                "slot": t.who.slot, "st": t.state.as_str(), "o": vec3(t.pos), "sig": t.sigma,
                                "age": self.now.since(t.last_seen),
                            })
                        })
                        .collect();
                    let m = &b.brain.mind;
                    let candidates: Vec<serde_json::Value> = m
                        .decider
                        .last
                        .iter()
                        .take(4)
                        .map(|g| serde_json::json!([g.kind.as_str(), g.rank, g.weight]))
                        .collect();
                    let target = m.target.and_then(|k| b.brain.beliefs.track(k));
                    serde_json::json!({
                        "slot": b.id.slot,
                        "n": b.persona.name,
                        "sty": b.persona.style.as_str(),
                        "sk": b.persona.skill,
                        "st": b.state.as_str(),
                        "o": [body.origin.x, body.origin.y, body.origin.z],
                        "ya": b.view.y,
                        "hp": body.health,
                        "ap": body.armor,
                        "w": b.self_state.current_weapon.get().map(|w| w.classname()),
                        "tr": tracks,
                        "at": b.attention.map(|a| a.reason.as_str()),
                        "goal": m.goal.map(|g| g.kind.as_str()),
                        "gw": m.goal.map(|g| g.weight),
                        "cand": candidates,
                        "tg": target.map_or(-1, |t| i32::from(t.who.slot)),
                        "see": target.is_some_and(|t| t.state == lb_knowledge::TrackState::Visible),
                        "fire": m.firing,
                        "arm": m.arms.active.as_ref().map(|a| a.name()),
                        "gauss": m.arms.gauss.active().then(|| m.arms.gauss.phase()),
                        "nv": b.nav.phase(),
                        "agr": b.persona.aggression,
                        "fear": b.persona.fear,
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
fn drive_one(
    bot: &mut Bot,
    ctx: &DriveCtx<'_>,
    tracer: &mut nav::LiveTracer<'_>,
    link_health: &mut LinkHealth,
    plan_budget: &mut u32,
) -> Option<LbBotCommand> {
    if bot.fault_on_next_frame {
        bot.fault_on_next_frame = false;
        panic!("injected fault (lb debug panic)");
    }
    let (now, frame_ms) = (ctx.now, ctx.frame_ms);
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
        BotState::Alive if !ctx.freeze || bot.selftest.is_some() => {
            if let Some(test) = bot.selftest.as_mut() {
                let frame = test.step(now, &bot.self_state, ctx.projectiles, bot.id.slot);
                for line in &frame.lines {
                    tracing::info!("{line}");
                    logging::console_line(format!("[lambdabots] {line}"));
                }
                bot.view = frame.view;
                bot.brain.motor.set_view(frame.view);
                forward = frame.forward;
                buttons |= frame.buttons;
                bot.pending_client_cmds.extend(frame.commands);
            } else if let Some(test) = bot.test.as_mut() {
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
            } else if bot.nav_test.as_ref().is_some_and(|t| !t.finished) {
                let out = drive_nav_test(bot, ctx, tracer, link_health);
                forward = out.forward;
                side = out.side;
                buttons |= out.buttons;
            } else {
                let out = behave(bot, ctx, tracer, link_health, plan_budget);
                forward = out.forward;
                side = out.side;
                buttons |= out.buttons;
            }
        }
        _ => {}
    }
    if matches!(bot.state, BotState::Dead | BotState::Respawning)
        && let (Some(t), Some(g)) = (bot.nav_test.as_mut(), ctx.graph)
    {
        t.on_death(g, now.secs());
    }
    buttons |= direction_buttons(forward, side);
    let sent = bot.driver.tick(frame_ms, buttons)?;
    bot.brain.motor.sent(sent.buttons);
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

/// The bot's own state as its brain sees it.
fn body_of(bot: &Bot, ctx: &DriveCtx<'_>) -> lb_brain::Body {
    let state = &bot.self_state;
    let b = &state.body;
    let arsenal = arsenal(state, ctx.registry);
    let ammo_need = Ammo::ALL.map(|a| ammo_need(state, ctx.registry, a));
    lb_brain::Body {
        now: ctx.now,
        dt: (ctx.frame_ms / 1000.0) as f32,
        origin: b.origin,
        eye: b.origin + b.view_ofs,
        velocity: b.velocity,
        maxspeed: if b.maxspeed > 0.0 { b.maxspeed } else { 270.0 },
        health: b.health,
        armor: b.armor,
        has_longjump: b.has_longjump,
        on_ground: b.flags & lb_game::self_state::FL_ONGROUND != 0,
        on_ladder: b.movetype == lb_game::self_state::MOVETYPE_FLY,
        underwater: b.waterlevel >= 3,
        waterlevel: b.waterlevel,
        fov: b.fov,
        weapon: state.current_weapon.get(),
        arsenal,
        prediction: state.prediction,
        ammo_need,
        opponents: ctx.opponents,
        damages: ctx.damages,
        dll: ctx.dll,
        gravity: ctx.gravity,
        allowed: ctx.allowed,
    }
}

/// Weapons the bot carries, with the clip `CurWeapon` reported and the reserve `AmmoX` reported.
fn arsenal(state: &SelfState, registry: &WeaponRegistry) -> smallvec::SmallVec<[Armed; 16]> {
    weapons_in_mask(state.body.weapons_mask)
        .map(|w| {
            let info = registry.get(w);
            let ammo = |index: Option<i32>| {
                index
                    .filter(|i| *i >= 0)
                    .and_then(|i| state.ammo.get(i as usize))
                    .and_then(|a| a.get())
            };
            Armed {
                id: w,
                clip: state.clip[w as usize].get(),
                reserve: ammo(info.map(|i| i.ammo1)),
                reserve2: ammo(info.map(|i| i.ammo2)),
            }
        })
        .collect()
}

/// `give` client commands for `weapons` and some of their ammo (the game honors them with `sv_cheats 1`).
fn give_weapons(bot: &mut Bot, weapons: &[WeaponId]) {
    for &w in weapons {
        let times = if w.is_throwable() { 3 } else { 1 };
        for _ in 0..times {
            bot.pending_client_cmds.push(vec!["give".into(), w.classname().into()]);
        }
        let ammo: &[&str] = match w {
            WeaponId::Glock => &["ammo_9mmclip"],
            WeaponId::Mp5 => &["ammo_9mmAR", "ammo_ARgrenades"],
            WeaponId::Python => &["ammo_357"],
            WeaponId::Crossbow => &["ammo_crossbow"],
            WeaponId::Shotgun => &["ammo_buckshot"],
            WeaponId::Rpg => &["ammo_rpgclip"],
            WeaponId::Gauss | WeaponId::Egon => &["ammo_gaussclip"],
            _ => &[],
        };
        for a in ammo {
            for _ in 0..3 {
                bot.pending_client_cmds.push(vec!["give".into(), (*a).into()]);
            }
        }
    }
}

/// 0..1: how short the bot is of an ammo type it has a weapon for.
fn ammo_need(state: &SelfState, registry: &WeaponRegistry, ammo: Ammo) -> f32 {
    let Some(info) = ammo
        .feeds()
        .iter()
        .filter(|w| state.owns(**w))
        .find_map(|w| registry.get(*w))
    else {
        return 0.0;
    };
    let (index, max) = if ammo == Ammo::ArGrenades {
        (info.ammo2, info.max_ammo2)
    } else {
        (info.ammo1, info.max_ammo1)
    };
    if index < 0 || max <= 0 {
        return 0.0;
    }
    match state.ammo.get(index as usize).and_then(|a| a.get()) {
        Some(have) => (1.0 - have as f32 / max as f32).clamp(0.0, 1.0),
        None => 0.5,
    }
}

/// The bot's own state as navigation needs it.
fn nav_input(bot: &Bot, ctx: &DriveCtx<'_>, body: &lb_brain::Body) -> lb_nav::exec::NavInput {
    let raw = &bot.self_state.body;
    lb_nav::exec::NavInput {
        now: ctx.now.secs(),
        origin: body.origin,
        velocity: body.velocity,
        view: bot.view,
        on_ground: body.on_ground,
        on_ladder: body.on_ladder,
        ducked: raw.flags & lb_game::self_state::FL_DUCKING != 0,
        waterlevel: raw.waterlevel,
        ground_model: if body.on_ground {
            ctx.mechs.model_of(raw.groundentity)
        } else {
            0
        },
        max_speed: body.maxspeed,
        health: body.health,
        push: if raw.flags & lb_game::self_state::FL_BASEVELOCITY != 0 {
            raw.basevelocity
        } else {
            Vec3::ZERO
        },
        gravity: ctx.gravity,
    }
}

/// `lb nav test`: the course drives the bot, nothing else does.
fn drive_nav_test(
    bot: &mut Bot,
    ctx: &DriveCtx<'_>,
    tracer: &mut nav::LiveTracer<'_>,
    link_health: &mut LinkHealth,
) -> lb_motor::MotorOut {
    use lb_motor::{Intents, LookIntent, MoveIntent, Prio, StanceIntent};
    let body = body_of(bot, ctx);
    let input = nav_input(bot, ctx, &body);
    let mut intents = Intents::default();
    if let Some(graph) = ctx.graph
        && let Some(test) = bot.nav_test.as_mut()
    {
        let mut nctx = lb_nav::navigator::NavCtx {
            graph,
            tracer,
            mech: ctx.mechs,
            health: Some(link_health),
            bot: u32::from(bot.id.slot),
            budget: None,
        };
        if let Some(step) = test.step(&mut bot.nav, &mut nctx, &input) {
            intents.movement(
                Prio::Goal,
                MoveIntent {
                    dir: step.move_dir,
                    speed: step.speed,
                },
            );
            intents.stance(
                Prio::Goal,
                StanceIntent {
                    jump: step.jump,
                    duck: step.duck,
                },
            );
            if step.use_key {
                intents.use_key(Prio::Goal);
            }
            let look = match step.pitch {
                Some(pitch) => {
                    let mut a = lb_core::math::dir_to_view_angles(step.look_at - body.eye);
                    a.x = pitch;
                    LookIntent::Angles(a)
                }
                None => LookIntent::Point {
                    at: step.look_at,
                    engaged: false,
                },
            };
            intents.look(Prio::Goal, look);
        }
        if test.finished {
            for line in test.report() {
                tracing::info!("nav test: {line}");
                logging::console_line(format!("[lambdabots] nav test: {line}"));
            }
        }
    }
    bot.brain.motor.view = bot.view;
    let motor_in = lb_motor::MotorInput {
        now: ctx.now,
        dt: body.dt,
        eye: body.eye,
        velocity: body.velocity,
        maxspeed: body.maxspeed,
        on_ladder: body.on_ladder,
        weapon: body.weapon,
    };
    let look = lb_motor::LookParams {
        model: lb_config::skill::AimModel::Spring,
        turn_speed: 900.0,
        skill: 100,
    };
    let out = bot.brain.motor.run(&intents, &motor_in, &look, &mut bot.rng.motor);
    bot.view = out.angles;
    out
}

/// Seconds without an enemy seen or heard before a stuck bot may give up its life.
const CALM_BEFORE_KILL: f64 = 5.0;
/// Path search node expansions per second shared by all bots, and the most one frame may spend.
const PLAN_RATE: f64 = 200_000.0;
const PLAN_BURST: f64 = 2000.0;

/// Runs the brain for one frame: senses have run already; this decides, fights and walks.
fn behave(
    bot: &mut Bot,
    ctx: &DriveCtx<'_>,
    tracer: &mut nav::LiveTracer<'_>,
    link_health: &mut LinkHealth,
    plan_budget: &mut u32,
) -> lb_motor::MotorOut {
    let body = body_of(bot, ctx);
    let input = nav_input(bot, ctx, &body);
    bot.brain.motor.view = bot.view;
    let calm = bot.brain.calm_for(ctx.now) > CALM_BEFORE_KILL;
    let mut service = nav::BotNavService {
        nav: &mut bot.nav,
        graph: ctx.graph,
        tracer,
        mechs: ctx.mechs,
        health: link_health,
        bot: u32::from(bot.id.slot),
        input,
        stuck_kill: ctx.stuck_kill,
        calm,
        kill: false,
        plan_budget,
    };
    let out = bot.brain.act(&body, &bot.character, &mut service, &mut bot.rng);
    if service.kill {
        tracing::info!("{} is stuck for {} s, using kill", bot.persona.name, ctx.stuck_kill);
        bot.pending_client_cmds.push(vec!["kill".to_string()]);
    }
    bot.view = out.angles;
    bot.attention = bot.brain.last_attention;
    for command in &out.commands {
        bot.pending_client_cmds.push(vec![command.clone()]);
    }
    out
}

/// The world as the senses get it this frame, shared by every bot.
struct Senses<'a> {
    now: SimTime,
    subjects: &'a [Subject<'a>],
    sounds: &'a [SoundEvent],
    public: &'a [PublicEvent],
    vis: &'a dyn VisSets,
    maxspeed: f32,
    teams: &'a [u8],
    items: &'a [ItemEntity],
    chargers: &'a [ChargerEntity],
    projectiles: &'a [ProjectileEntity],
    /// Explosions of this frame.
    explosions: &'a [Vec3],
}

/// What driving a bot needs besides the bot itself.
struct DriveCtx<'a> {
    now: SimTime,
    frame_ms: f64,
    freeze: bool,
    graph: Option<&'a lb_nav::NavGraph>,
    stuck_kill: f64,
    registry: &'a WeaponRegistry,
    opponents: usize,
    mechs: &'a nav::LiveMechs,
    /// `sv_gravity`.
    gravity: f32,
    damages: lb_game::mechanics::Damages,
    dll: DllProfile,
    /// Weapons bots may use (`lb weapons`).
    allowed: u32,
    projectiles: &'a [ProjectileEntity],
}

/// Players as vision gets them: the snapshot plus the weapon they show and their last shot.
fn subjects<'a>(
    clients: &'a [RawClient],
    strings: &StringTable,
    models: &mut FxHashMap<u16, Option<WeaponId>>,
    last_shot: &[Option<SimTime>],
    teams: &[u8],
) -> Vec<Subject<'a>> {
    clients
        .iter()
        .filter(|c| c.state == lb_raw::ClientState::Spawned)
        .map(|c| {
            let weapon = *models
                .entry(c.weaponmodel)
                .or_insert_with(|| WeaponId::from_player_model(&strings.string_lossy(c.weaponmodel)));
            Subject {
                raw: c,
                key: PlayerKey {
                    slot: c.slot,
                    userid: c.userid,
                },
                team: teams.get(c.slot as usize).copied().unwrap_or(0),
                weapon,
                shot_at: last_shot.get(c.slot as usize).copied().flatten(),
            }
        })
        .collect()
}

/// One bot's senses for this frame: the kill feed always, hearing every frame and vision on the bot's tick while it
/// lives, then its beliefs age to now.
fn sense(
    bot: &mut Bot,
    w: &Senses<'_>,
    tracer: &mut nav::LiveTracer<'_>,
    recognized: &mut Vec<(String, lb_perception::Recognition)>,
) {
    for e in w.public {
        bot.brain.on_public(e);
    }
    if bot.state == BotState::Alive {
        let body = &bot.self_state.body;
        let eye = body.origin + body.view_ofs;
        let listener = Listener {
            slot: bot.id.slot,
            origin: body.origin,
            eye,
            yaw: bot.view.y,
            speed: body.velocity.truncate().length(),
        };
        bot.brain.hear(w.sounds, &listener, w.vis, &mut bot.rng.perception);
        let viewer = Viewer {
            index: u16::from(bot.id.slot),
            eye,
            angles: bot.view,
            fov: body.fov,
            aspect: DEFAULT_ASPECT,
            head_in_water: body.waterlevel >= 3,
            team: w.teams.get(bot.id.slot as usize).copied().unwrap_or(0),
        };
        if bot
            .brain
            .see(w.now, &viewer, w.subjects, w.vis, tracer, &mut bot.rng.perception)
        {
            for r in &bot.brain.last_vision.recognitions {
                recognized.push((bot.persona.name.clone(), *r));
            }
            let range = 900.0 + 11.0 * f32::from(bot.persona.skill);
            bot.brain
                .see_items(w.now, &viewer, w.items, w.chargers, range, w.vis, tracer);
            bot.brain.see_projectiles(w.now, &viewer, w.projectiles, w.vis, tracer);
        }
        for &at in w.explosions {
            if w.vis.in_pas(body.origin, at) {
                bot.brain.on_explosion(at);
            }
        }
    }
    let params = BeliefParams {
        track_forget: bot.skill.track_forget,
        maxspeed: w.maxspeed,
    };
    bot.brain.update(w.now, &params);
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

/// Built-in styles with `config/styles/*.yaml` applied; broken files are reported and skipped.
fn load_styles(install_dir: &std::path::Path) -> (StyleTable, String) {
    let dir = install_dir.join("config").join("styles");
    let mut table = StyleTable::default();
    let mut used = Vec::new();
    for path in lb_config::check::yaml_files(&dir) {
        let shown = path.display().to_string();
        let parsed = std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|t| lb_config::styles::StyleFile::parse(&t, &shown).map_err(|e| e.to_string()));
        match parsed {
            Ok(f) => {
                table.apply(&f);
                used.push(f.id);
            }
            Err(e) => tracing::error!("{e}; this style keeps its built-in values"),
        }
    }
    let source = if used.is_empty() {
        "built-in".to_string()
    } else {
        format!("{} ({})", dir.display(), used.join(", "))
    };
    (table, source)
}

/// `config/difficulty.yaml`, or the built-in table when it is missing or broken.
fn load_presets(install_dir: &std::path::Path) -> (Presets, String) {
    let path = install_dir.join("config").join("difficulty.yaml");
    match std::fs::read_to_string(&path) {
        Ok(text) => match DifficultyFile::parse(&text, &path.display().to_string()) {
            Ok(f) => (f.presets, path.display().to_string()),
            Err(e) => {
                tracing::error!("{e}; using the built-in skill table");
                (Presets::default(), "built-in".into())
            }
        },
        Err(e) => {
            tracing::warn!("{}: {e}; using the built-in skill table", path.display());
            (Presets::default(), "built-in".into())
        }
    }
}

/// How the map's graph is checked: movement as this server's rules set it.
fn import_options(rules: &PublicRules, trust_imported: bool) -> lb_nav::import::ImportOptions {
    lb_nav::import::ImportOptions {
        trust_imported,
        physics: lb_kin::Physics {
            gravity: rules.gravity,
            maxspeed: rules.maxspeed,
            bunnyhop_cap: !rules.bunnyhop_uncapped,
            progressive_fall_damage: rules.falldamage_progressive,
            ..lb_kin::Physics::default()
        },
    }
}

fn quota_mode_name(mode: QuotaMode) -> &'static str {
    match mode {
        QuotaMode::Normal => "normal",
        QuotaMode::Fill => "fill",
        QuotaMode::Match => "match",
    }
}
