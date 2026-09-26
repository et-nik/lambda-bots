//! `config/lambdabots.yaml`: server-level settings (quota, engine, telemetry, access, logging).

use serde::{Deserialize, Serialize};

use crate::ConfigError;
use crate::yaml;

pub const KIND: &str = "main";
pub const MAJOR: u32 = 1;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct MainConfig {
    pub schema: String,
    pub quota: QuotaConfig,
    pub bots: BotsConfig,
    pub engine: EngineConfig,
    pub telemetry: TelemetryConfig,
    pub access: AccessConfig,
    pub logging: LoggingConfig,
    pub gungame: GunGameDetect,
    pub disguise: DisguiseConfig,
}

impl Default for MainConfig {
    fn default() -> Self {
        MainConfig {
            schema: format!("lambdabots/{KIND}@{MAJOR}"),
            quota: QuotaConfig::default(),
            bots: BotsConfig::default(),
            engine: EngineConfig::default(),
            telemetry: TelemetryConfig::default(),
            access: AccessConfig::default(),
            logging: LoggingConfig::default(),
            gungame: GunGameDetect::default(),
            disguise: DisguiseConfig::default(),
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
    /// `noob | easy | normal | hard | expert` or `0..4`.
    pub difficulty: String,
    /// Style id or `random`.
    pub style: String,
    pub name_prefix: String,
    pub save_names: bool,
    pub language: String,
    pub use_profiles: bool,
    pub models: Vec<String>,
    pub rotate: RotateConfig,
    pub force_respawn: bool,
    pub respawn_delay: [f32; 2],
    /// Seconds without progress before an irrecoverably stuck bot uses `kill`.
    pub stuck_kill_time: f32,
}

impl Default for BotsConfig {
    fn default() -> Self {
        BotsConfig {
            difficulty: "normal".into(),
            style: "random".into(),
            name_prefix: String::new(),
            save_names: true,
            language: "en".into(),
            use_profiles: true,
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
        }
    }
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
        if self.quota.join_interval[0] > self.quota.join_interval[1] {
            return err("quota.join_interval", "must be [min, max] with min <= max");
        }
        if self.bots.rotate.stay[0] > self.bots.rotate.stay[1] {
            return err("bots.rotate.stay", "must be [min, max] with min <= max");
        }
        if crate::main_config::parse_difficulty(&self.bots.difficulty).is_none() {
            return err("bots.difficulty", "expected noob|easy|normal|hard|expert or 0..4");
        }
        if !(1.0..=1000.0).contains(&self.gungame.frags_per_level) {
            return err("gungame.frags_per_level", "must be in 1..=1000");
        }
        Ok(())
    }
}

pub fn parse_difficulty(s: &str) -> Option<u8> {
    match s.trim().to_ascii_lowercase().as_str() {
        "0" | "noob" => Some(0),
        "1" | "easy" => Some(1),
        "2" | "normal" => Some(2),
        "3" | "hard" => Some(3),
        "4" | "expert" => Some(4),
        _ => None,
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
        let e = MainConfig::parse("schema: lambdabots/main@1\nbots:\n  difficulty: insane\n", "t.yaml").unwrap_err();
        assert!(e.to_string().contains("bots.difficulty"));
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
