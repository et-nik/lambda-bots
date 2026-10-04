//! `config/lambdabots.yaml`: server-level settings (quota, engine, telemetry, access, logging, chat).

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::ConfigError;
use crate::profiles::STYLE_IDS;
use crate::skill::{REFLEX_RANGE, SkillBand};
use crate::yaml;

pub const KIND: &str = "main";
pub const MAJOR: u32 = 1;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct MainConfig {
    pub schema: String,
    pub quota: QuotaConfig,
    pub bots: BotsConfig,
    pub roster: RosterConfig,
    pub engine: EngineConfig,
    pub nav: NavConfig,
    pub telemetry: TelemetryConfig,
    pub access: AccessConfig,
    pub logging: LoggingConfig,
    pub gungame: GunGameDetect,
    pub game: GameConfig,
    pub tricks: TricksConfig,
    pub disguise: DisguiseConfig,
    pub chat: ChatConfig,
}

impl Default for MainConfig {
    fn default() -> Self {
        MainConfig {
            schema: format!("lambdabots/{KIND}@{MAJOR}"),
            quota: QuotaConfig::default(),
            bots: BotsConfig::default(),
            roster: RosterConfig::default(),
            engine: EngineConfig::default(),
            nav: NavConfig::default(),
            telemetry: TelemetryConfig::default(),
            access: AccessConfig::default(),
            logging: LoggingConfig::default(),
            gungame: GunGameDetect::default(),
            game: GameConfig::default(),
            tricks: TricksConfig::default(),
            disguise: DisguiseConfig::default(),
            chat: ChatConfig::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum QuotaMode {
    Normal,
    Fill,
    Match,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct QuotaConfig {
    /// Bots to keep (normal), total players to reach (fill), or bots per human (match).
    pub count: u32,
    pub mode: QuotaMode,
    pub match_ratio: f32,
    pub autovacate: bool,
    pub keep_slots: u32,
    pub count_connecting: bool,
    pub join_after_player: bool,
    /// Seconds after map start before the first bot joins.
    pub join_delay: f32,
    pub join_interval: [f32; 2],
}

impl Default for QuotaConfig {
    fn default() -> Self {
        QuotaConfig {
            count: 8,
            mode: QuotaMode::Fill,
            match_ratio: 1.0,
            autovacate: true,
            keep_slots: 1,
            count_connecting: true,
            join_after_player: false,
            join_delay: 5.0,
            join_interval: [0.5, 3.0],
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct BotsConfig {
    /// Shown before every bot name, e.g. `[BOT] `; not part of the personality.
    pub name_prefix: String,
    /// The same bots come back after a map change.
    pub save_names: bool,
    /// `names/<language>.yaml`: nicknames for new personalities.
    pub language: String,
    /// Models new personalities choose from.
    pub models: Vec<String>,
    pub rotate: RotateConfig,
    pub force_respawn: bool,
    pub respawn_delay: [f32; 2],
    /// Seconds without progress before an irrecoverably stuck bot uses `kill`.
    pub stuck_kill_time: f32,
    /// How quick every bot is, on top of its skill: 2 recognizes and aims in half the time and turns twice as fast,
    /// 0.5 the other way round (`SkillParams::with_reflex`).
    pub reflex: f32,
}

impl Default for BotsConfig {
    fn default() -> Self {
        BotsConfig {
            name_prefix: String::new(),
            save_names: true,
            language: "en".into(),
            models: [
                "barney",
                "gina",
                "gman",
                "gordon",
                "helmet",
                "hgrunt",
                "recon",
                "robo",
                "scientist",
                "zombie",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
            rotate: RotateConfig::default(),
            force_respawn: true,
            respawn_delay: [0.3, 1.2],
            stuck_kill_time: 20.0,
            reflex: 1.0,
        }
    }
}

/// Which personalities may join and how new ones are created.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct RosterConfig {
    /// Skill filter: `any`, a preset (`hard` = 63..87), a number (`60` = 48..72) or a range (`normal-hard`,
    /// `40-70`). A personality's own skill never changes; this only chooses who joins.
    pub difficulty: String,
    /// Style filter: `any` or a comma list (`rusher,sniper`).
    pub styles: String,
    pub generate: GenerateConfig,
}

impl Default for RosterConfig {
    fn default() -> Self {
        RosterConfig {
            difficulty: "normal".into(),
            styles: "any".into(),
            generate: GenerateConfig::default(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct GenerateConfig {
    /// Create personalities for new nicknames from `names/<language>.yaml` and keep them in `data/profiles.yaml`.
    pub enabled: bool,
    /// Keep at least this many personalities that pass the filters; fewer, and the next bot is a new one.
    pub pool: u32,
    /// Style mix of new personalities (relative weights).
    pub styles: BTreeMap<String, f32>,
}

impl Default for GenerateConfig {
    fn default() -> Self {
        GenerateConfig {
            enabled: true,
            pool: 16,
            styles: [
                ("balanced", 4.0),
                ("rusher", 2.0),
                ("sniper", 1.0),
                ("controller", 2.0),
                ("trapper", 1.0),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect(),
        }
    }
}

/// `any` or a comma list of style ids; `None` for an invalid list.
pub fn parse_style_filter(s: &str) -> Option<Vec<String>> {
    let s = s.trim().to_ascii_lowercase();
    if s.is_empty() || s == "any" {
        return Some(Vec::new());
    }
    let styles: Vec<String> = s
        .split(',')
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect();
    styles.iter().all(|p| STYLE_IDS.contains(&p.as_str())).then_some(styles)
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct RotateConfig {
    pub enabled: bool,
    pub stay: [f32; 2],
    pub rejoin_delay: [f32; 2],
}

impl Default for RotateConfig {
    fn default() -> Self {
        RotateConfig {
            enabled: false,
            stay: [360.0, 3600.0],
            rejoin_delay: [20.0, 90.0],
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct EngineConfig {
    /// Commands per second sent to the engine per bot; 0 = every frame.
    pub cmd_rate: f32,
    /// Largest accumulated command debt in milliseconds before it is dropped.
    pub max_cmd_debt_ms: f32,
    pub workers: i32,
    /// 0 = random seed per start.
    pub master_seed: u64,
}

impl Default for EngineConfig {
    fn default() -> Self {
        EngineConfig {
            cmd_rate: 100.0,
            max_cmd_debt_ms: 200.0,
            workers: -1,
            master_seed: 0,
        }
    }
}

/// Where a map's navigation graph comes from.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum NavSource {
    /// Made from the map on a worker (and kept in `nav/<map>/`); the map's yapb graph if that fails.
    Generated,
    /// The map's yapb graph, checked against the map.
    Yapb,
}

impl NavSource {
    pub fn parse(s: &str) -> Option<NavSource> {
        match s.trim() {
            "generated" => Some(NavSource::Generated),
            "yapb" => Some(NavSource::Yapb),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            NavSource::Generated => "generated",
            NavSource::Yapb => "yapb",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct NavConfig {
    pub source: NavSource,
    /// Threads making a graph; negative = all cores but that many.
    pub threads: i32,
    /// Plan through imported yapb links the map check failed.
    pub trust_imported: bool,
}

impl Default for NavConfig {
    fn default() -> Self {
        NavConfig {
            source: NavSource::Generated,
            threads: -2,
            trust_imported: false,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct TelemetryConfig {
    pub enabled: bool,
    pub dest: String,
    pub port: u16,
    pub hz: f32,
    pub cmd_bind: String,
    /// HMAC secret for the command channel; empty disables commands.
    pub secret: String,
    pub allow_unauthenticated_loopback: bool,
    pub max_kbps: u32,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        TelemetryConfig {
            enabled: false,
            dest: "127.0.0.1".into(),
            port: 27070,
            hz: 10.0,
            cmd_bind: "127.0.0.1".into(),
            secret: String::new(),
            allow_unauthenticated_loopback: false,
            max_kbps: 1024,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct AccessConfig {
    /// SteamIDs allowed to use client-side `lb` commands and the editor.
    pub admins: Vec<String>,
    pub password: String,
    pub password_key: String,
    pub editor_enabled: bool,
    pub listen_host_is_admin: bool,
}

impl Default for AccessConfig {
    fn default() -> Self {
        AccessConfig {
            admins: Vec::new(),
            password: String::new(),
            password_key: "_lbpw".into(),
            editor_enabled: false,
            listen_host_is_admin: true,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct LoggingConfig {
    pub level: String,
    pub console_level: String,
    pub file: bool,
    pub max_files: u32,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        LoggingConfig {
            level: "info".into(),
            console_level: "warn".into(),
            file: true,
            max_files: 7,
        }
    }
}

/// Game DLLs whose weapon rules differ (see `lb_game::dll`); `auto` tells BugfixedHL-Rebased by its cvars and takes
/// anything else for the 2023 update's grenade and the classic satchel buttons.
pub const DLL_NAMES: [&str; 4] = ["auto", "bugfixed", "hl25", "classic"];

/// The server's game DLL, where its weapon rules differ.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct GameConfig {
    /// One of [`DLL_NAMES`].
    pub dll: String,
}

impl Default for GameConfig {
    fn default() -> Self {
        GameConfig { dll: "auto".into() }
    }
}

/// Tricks the bots may use; how often is up to each bot's style and skill.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, default)]
pub struct TricksConfig {
    /// Long jumps (with the module): across gaps, along straight stretches of the way, at enemies.
    pub longjump: bool,
    /// Gauss jumps on the way to somewhere far, the gauss in hand.
    pub gauss_jump: bool,
    /// The gauss boost links of the navigation graph, onto ledges and across.
    pub gauss_boost: bool,
    /// Satchels thrown from a jump and set off in flight.
    pub satchel_jump: bool,
    /// Hand grenades thrown from a jump (a long jump with the module) at an enemy far off or above.
    pub grenade_jump: bool,
    /// Bunny hopping along straight stretches of the way and closing in on an enemy, on any server: one that crops
    /// faster jumps is hopped just under the crop.
    pub bhop: bool,
}

impl Default for TricksConfig {
    fn default() -> Self {
        TricksConfig {
            longjump: true,
            gauss_jump: true,
            gauss_boost: true,
            satchel_jump: true,
            grenade_jump: true,
            bhop: true,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct GunGameDetect {
    /// `auto`, `on` or `off`.
    pub mode: String,
    pub detect_cvar: String,
    pub frags_per_level: f32,
}

impl Default for GunGameDetect {
    fn default() -> Self {
        GunGameDetect {
            mode: "auto".into(),
            detect_cvar: "gg_enabled".into(),
            frags_per_level: 100.0,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct DisguiseConfig {
    pub fake_ping: bool,
    pub scoreboard_bot_flag: bool,
    pub avatars: bool,
    pub fake_steamid: bool,
    pub hide_bots_in_queries: bool,
}

/// A string kept out of logs and printouts: `Debug` only tells whether it is set.
#[derive(Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(transparent)]
pub struct Secret(pub String);

impl Secret {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if self.0.is_empty() { "\"\"" } else { REDACTED })
    }
}

/// What secrets read as in recordings and printouts.
pub const REDACTED: &str = "<redacted>";

/// Bots talking in the game chat through a chat model.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct ChatConfig {
    pub enabled: bool,
    /// The language bots write in unless a player speaks to them in another (`ru`, `en`, …).
    pub language: String,
    /// A line about the server for the bots ("GunGame server hldm.org").
    pub server: String,
    /// No requests while no human is on the server.
    pub require_humans: bool,
    pub provider: ChatProvider,
    pub limits: ChatLimits,
    pub typing: ChatTyping,
    pub memory: ChatMemory,
    /// Chat commands of server plugins: a bot never says a line starting with one, and a player's line starting
    /// with one is not chat.
    pub blocked: Vec<String>,
}

impl Default for ChatConfig {
    fn default() -> Self {
        ChatConfig {
            enabled: false,
            language: "ru".into(),
            server: String::new(),
            require_humans: true,
            provider: ChatProvider::default(),
            limits: ChatLimits::default(),
            typing: ChatTyping::default(),
            memory: ChatMemory::default(),
            blocked: [
                "rtv",
                "rockthevote",
                "nominate",
                "timeleft",
                "nextmap",
                "thetime",
                "currentmap",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    /// The Anthropic Messages API.
    Anthropic,
    /// OpenAI-compatible chat completions.
    Openai,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct ChatProvider {
    pub kind: ProviderKind,
    /// Empty: `https://api.anthropic.com` for anthropic; openai needs one (`http://127.0.0.1:11434/v1`). A gateway
    /// in front of the API goes here.
    pub base_url: String,
    pub model: String,
    /// The key itself, else a file holding it, else an environment variable. Only the chat worker reads them.
    pub api_key: Secret,
    pub api_key_file: String,
    pub api_key_env: String,
    /// PEM certificates trusted on top of the system's (a gateway's own CA).
    pub ca_file: String,
    /// Extra HTTP headers for every request.
    pub headers: BTreeMap<String, String>,
    /// Seconds a request may take.
    pub timeout: f32,
    pub max_tokens: u32,
    /// Sent only when set: some models refuse sampling parameters.
    pub temperature: Option<f32>,
    /// A JSON object merged into every request body (`{"output_config": {"effort": "low"}}`); empty for none.
    pub extra_body: String,
}

impl Default for ChatProvider {
    fn default() -> Self {
        ChatProvider {
            kind: ProviderKind::Anthropic,
            base_url: String::new(),
            model: "claude-haiku-4-5".into(),
            api_key: Secret::default(),
            api_key_file: String::new(),
            api_key_env: "ANTHROPIC_API_KEY".into(),
            ca_file: String::new(),
            headers: BTreeMap::new(),
            timeout: 12.0,
            max_tokens: 100,
            temperature: None,
            extra_body: String::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct ChatLimits {
    /// Lines all bots together may say a minute.
    pub lines_per_minute: f32,
    /// Seconds between two lines nobody asked for, whoever says them.
    pub remark_gap: f32,
    /// Seconds between two lines nobody asked for from one bot.
    pub bot_remark_gap: f32,
    pub requests_per_minute: f32,
    /// Tokens (in and out) a day, UTC; 0 = no limit.
    pub tokens_per_day: u64,
}

impl Default for ChatLimits {
    fn default() -> Self {
        ChatLimits {
            lines_per_minute: 2.0,
            remark_gap: 60.0,
            bot_remark_gap: 240.0,
            requests_per_minute: 6.0,
            tokens_per_day: 2_000_000,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct ChatTyping {
    /// Characters a minute; a personality without its own speed gets one from this range.
    pub cpm: [f32; 2],
    /// Seconds without an enemy seen or heard before an alive bot starts typing.
    pub calm: f32,
}

impl Default for ChatTyping {
    fn default() -> Self {
        ChatTyping {
            cpm: [150.0, 330.0],
            calm: 3.0,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct ChatMemory {
    /// Remember players between maps and restarts (`data/chat/memory.json`).
    pub enabled: bool,
    /// After each map, one request updates short notes about the players the bots met.
    pub ai_notes: bool,
    /// Players not seen for this many days are forgotten.
    pub forget_after_days: u32,
}

impl Default for ChatMemory {
    fn default() -> Self {
        ChatMemory {
            enabled: true,
            ai_notes: true,
            forget_after_days: 120,
        }
    }
}

impl MainConfig {
    pub fn parse(text: &str, path: &str) -> Result<MainConfig, ConfigError> {
        let cfg: MainConfig = yaml::from_str(text, path)?;
        yaml::check_schema(&cfg.schema, KIND, MAJOR, path)?;
        cfg.validate(path)?;
        Ok(cfg)
    }

    pub fn validate(&self, path: &str) -> Result<(), ConfigError> {
        let err = |field: &str, message: &str| {
            Err(ConfigError::Invalid {
                path: path.to_string(),
                field: field.to_string(),
                message: message.to_string(),
            })
        };
        if self.quota.count > 32 {
            return err("quota.count", "must be in 0..=32");
        }
        if !(0.0..=1000.0).contains(&self.engine.cmd_rate) {
            return err("engine.cmd_rate", "must be in 0..=1000 (0 = every frame)");
        }
        if self.engine.max_cmd_debt_ms < 50.0 {
            return err("engine.max_cmd_debt_ms", "must be at least 50");
        }
        if self.bots.respawn_delay[0] > self.bots.respawn_delay[1] || self.bots.respawn_delay[0] < 0.0 {
            return err("bots.respawn_delay", "must be [min, max] with 0 <= min <= max");
        }
        if !(REFLEX_RANGE[0]..=REFLEX_RANGE[1]).contains(&self.bots.reflex) {
            return err("bots.reflex", "must be in 0.5..=2");
        }
        if self.quota.join_interval[0] > self.quota.join_interval[1] {
            return err("quota.join_interval", "must be [min, max] with min <= max");
        }
        if self.bots.rotate.stay[0] > self.bots.rotate.stay[1] {
            return err("bots.rotate.stay", "must be [min, max] with min <= max");
        }
        if SkillBand::parse(&self.roster.difficulty).is_none() {
            return err(
                "roster.difficulty",
                "expected any, noob..expert, a number 0..100 or a range like normal-hard",
            );
        }
        if parse_style_filter(&self.roster.styles).is_none() {
            return err(
                "roster.styles",
                "expected any or a comma list of balanced, rusher, sniper, controller, trapper",
            );
        }
        let g = &self.roster.generate;
        if let Some(bad) = g.styles.keys().find(|k| !STYLE_IDS.contains(&k.as_str())) {
            return err("roster.generate.styles", &format!("unknown style `{bad}`"));
        }
        if g.styles.values().any(|w| !w.is_finite() || *w < 0.0) || g.styles.values().sum::<f32>() <= 0.0 {
            return err(
                "roster.generate.styles",
                "weights must be 0 or more with a positive sum",
            );
        }
        if self.bots.models.is_empty() {
            return err("bots.models", "must list at least one model");
        }
        if !(1.0..=1000.0).contains(&self.gungame.frags_per_level) {
            return err("gungame.frags_per_level", "must be in 1..=1000");
        }
        if !DLL_NAMES.iter().any(|n| n.eq_ignore_ascii_case(self.game.dll.trim())) {
            return err("game.dll", "expected auto, bugfixed, hl25 or classic");
        }
        self.chat.validate(path)
    }

    /// The config with every secret replaced by [`REDACTED`], for printing.
    pub fn redacted(&self) -> MainConfig {
        let mut c = self.clone();
        for secret in [&mut c.telemetry.secret, &mut c.access.password] {
            if !secret.is_empty() {
                *secret = REDACTED.into();
            }
        }
        c.chat.redact();
        c
    }
}

impl ChatConfig {
    fn validate(&self, path: &str) -> Result<(), ConfigError> {
        let err = |field: &str, message: &str| {
            Err(ConfigError::Invalid {
                path: path.to_string(),
                field: format!("chat.{field}"),
                message: message.to_string(),
            })
        };
        let lang = self.language.trim();
        if !(2..=8).contains(&lang.len()) || !lang.chars().all(|c| c.is_ascii_alphabetic() || c == '-') {
            return err("language", "expected a language code such as ru or en");
        }
        if self.server.chars().count() > 200 {
            return err("server", "must be at most 200 characters");
        }
        let p = &self.provider;
        let base = p.base_url.trim();
        if !base.is_empty() && !base.starts_with("http://") && !base.starts_with("https://") {
            return err("provider.base_url", "must start with http:// or https://");
        }
        if p.kind == ProviderKind::Openai && base.is_empty() {
            return err(
                "provider.base_url",
                "an openai provider needs one, e.g. http://127.0.0.1:11434/v1",
            );
        }
        if p.model.trim().is_empty() {
            return err("provider.model", "must not be empty");
        }
        if !(1.0..=120.0).contains(&p.timeout) {
            return err("provider.timeout", "must be in 1..=120 seconds");
        }
        if !(16..=4096).contains(&p.max_tokens) {
            return err("provider.max_tokens", "must be in 16..=4096");
        }
        if p.temperature.is_some_and(|t| !(0.0..=2.0).contains(&t)) {
            return err("provider.temperature", "must be in 0..=2");
        }
        if !p.extra_body.trim().is_empty()
            && !serde_json::from_str::<serde_json::Value>(&p.extra_body).is_ok_and(|v| v.is_object())
        {
            return err("provider.extra_body", "must be a JSON object");
        }
        let token = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b));
        if let Some(name) = p.headers.keys().find(|k| !token(k)) {
            return err("provider.headers", &format!("`{name}` is not a header name"));
        }
        if p.headers.values().any(|v| v.contains(['\r', '\n'])) {
            return err("provider.headers", "values must be one line");
        }
        let l = &self.limits;
        if !(0.1..=60.0).contains(&l.lines_per_minute) {
            return err("limits.lines_per_minute", "must be in 0.1..=60");
        }
        if !(0.1..=120.0).contains(&l.requests_per_minute) {
            return err("limits.requests_per_minute", "must be in 0.1..=120");
        }
        if !(0.0..=3600.0).contains(&l.remark_gap) || !(0.0..=3600.0).contains(&l.bot_remark_gap) {
            return err("limits", "remark gaps must be in 0..=3600 seconds");
        }
        let t = &self.typing;
        if !(30.0 <= t.cpm[0] && t.cpm[0] <= t.cpm[1] && t.cpm[1] <= 1500.0) {
            return err("typing.cpm", "must be [min, max] with 30 <= min <= max <= 1500");
        }
        if !(0.0..=30.0).contains(&t.calm) {
            return err("typing.calm", "must be in 0..=30 seconds");
        }
        if self.memory.forget_after_days == 0 {
            return err("memory.forget_after_days", "must be at least 1");
        }
        if self.blocked.iter().any(|b| b.trim().is_empty()) {
            return err("blocked", "entries must not be empty");
        }
        Ok(())
    }

    /// Replaces the key and the header values (a gateway's own key may be one) by [`REDACTED`].
    pub fn redact(&mut self) {
        if !self.provider.api_key.is_empty() {
            self.provider.api_key = Secret(REDACTED.into());
        }
        for value in self.provider.headers.values_mut() {
            *value = REDACTED.into();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_roundtrip() {
        let text = yaml::to_string(&MainConfig::default()).unwrap();
        let parsed = MainConfig::parse(&text, "defaults").unwrap();
        assert_eq!(parsed, MainConfig::default());
    }

    #[test]
    fn partial_file_uses_defaults() {
        let cfg = MainConfig::parse("schema: lambdabots/main@1\nquota:\n  count: 12\n", "t.yaml").unwrap();
        assert_eq!(cfg.quota.count, 12);
        assert_eq!(cfg.quota.mode, QuotaMode::Fill);
        assert_eq!(cfg.engine.cmd_rate, 100.0);
    }

    #[test]
    fn unknown_keys_and_bad_values_are_rejected() {
        assert!(MainConfig::parse("schema: lambdabots/main@1\nquota:\n  cuont: 3\n", "t.yaml").is_err());
        assert!(MainConfig::parse("schema: lambdabots/main@1\nquota:\n  count: 99\n", "t.yaml").is_err());
        assert!(MainConfig::parse("schema: lambdabots/main@2\n", "t.yaml").is_err());
        let e = MainConfig::parse("schema: lambdabots/main@1\nroster:\n  difficulty: insane\n", "t.yaml").unwrap_err();
        assert!(e.to_string().contains("roster.difficulty"));
        let e = MainConfig::parse("schema: lambdabots/main@1\nroster:\n  styles: camper\n", "t.yaml").unwrap_err();
        assert!(e.to_string().contains("roster.styles"));
        assert!(
            MainConfig::parse(
                "schema: lambdabots/main@1\nroster:\n  difficulty: normal-hard\n",
                "t.yaml"
            )
            .is_ok()
        );
    }

    #[test]
    fn chat_section() {
        let text = "schema: lambdabots/main@1\nchat:\n  enabled: true\n  provider:\n    kind: openai\n    base_url: http://127.0.0.1:8080/v1\n    api_key: sk-123\n    headers: { x-gateway-key: g-456 }\n    temperature: 0.8\n    extra_body: '{\"top_k\": 40}'\n";
        let cfg = MainConfig::parse(text, "t.yaml").unwrap();
        assert!(cfg.chat.enabled);
        assert_eq!(cfg.chat.provider.kind, ProviderKind::Openai);
        assert_eq!(cfg.chat.provider.temperature, Some(0.8));
        assert_eq!(cfg.chat.limits, ChatLimits::default());
        let shown = format!("{:?}{}", cfg.redacted(), yaml::to_string(&cfg.redacted()).unwrap());
        assert!(!shown.contains("sk-123") && !shown.contains("g-456"), "{shown}");
        assert!(!format!("{cfg:?}").contains("sk-123"));
        let bad = |body: &str| MainConfig::parse(&format!("schema: lambdabots/main@1\nchat:\n{body}"), "t").is_err();
        assert!(bad("  provider: { kind: openai }\n"), "openai without base_url");
        assert!(bad("  provider: { base_url: api.anthropic.com }\n"));
        assert!(bad("  provider: { extra_body: '[1]' }\n"));
        assert!(bad("  provider: { headers: { \"bad header\": x } }\n"));
        assert!(bad("  typing: { cpm: [300, 100] }\n"));
        assert!(bad("  language: русский\n"));
        assert!(bad("  limits: { lines_per_minute: 0 }\n"));
        assert!(bad("  mood: calm\n"));
    }

    #[test]
    fn redaction_keeps_empty_secrets_empty() {
        let mut cfg = MainConfig::default();
        cfg.telemetry.secret = "t".into();
        let r = cfg.redacted();
        assert_eq!(r.telemetry.secret, REDACTED);
        assert_eq!(r.access.password, "");
        assert!(r.chat.provider.api_key.is_empty());
    }
}

#[cfg(test)]
mod shipped {
    use super::*;

    #[test]
    fn shipped_config_parses_and_matches_defaults_where_expected() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/config/lambdabots.yaml");
        let text = std::fs::read_to_string(path).expect("data/config/lambdabots.yaml");
        let cfg = MainConfig::parse(&text, path).unwrap();
        assert_eq!(cfg.quota, QuotaConfig::default());
        assert_eq!(cfg.engine, EngineConfig::default());
        assert_eq!(cfg.bots, BotsConfig::default());
        assert_eq!(cfg.roster, RosterConfig::default());
        assert_eq!(cfg.chat, ChatConfig::default());
    }

    #[test]
    fn shipped_names_parse() {
        for lang in ["en", "ru"] {
            let path = format!("{}/../../data/names/{lang}.yaml", env!("CARGO_MANIFEST_DIR"));
            let text = std::fs::read_to_string(&path).unwrap();
            let names = crate::names::NamesFile::parse(&text, &path).unwrap();
            assert!(names.names.len() > 100, "{lang}");
        }
    }
}
