//! `lb_*` console variables. YAML supplies the defaults; a value changed at runtime (console,
//! rcon) overrides the YAML value until `lb config reload --force`.

use lb_host::{CvarHandle, CvarSpec, Host};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Cv {
    Version,
    Quota,
    QuotaMode,
    Difficulty,
    Style,
    CmdRate,
    ForceRespawn,
    GameMode,
    GunGame,
    Telemetry,
    TelemetryPort,
    TelemetrySecret,
    Freeze,
    Debug,
    Dev,
    LogLevel,
    NavSource,
    Editor,
}

pub const ALL: &[Cv] = &[
    Cv::Version,
    Cv::Quota,
    Cv::QuotaMode,
    Cv::Difficulty,
    Cv::Style,
    Cv::CmdRate,
    Cv::ForceRespawn,
    Cv::GameMode,
    Cv::GunGame,
    Cv::Telemetry,
    Cv::TelemetryPort,
    Cv::TelemetrySecret,
    Cv::Freeze,
    Cv::Debug,
    Cv::Dev,
    Cv::LogLevel,
    Cv::NavSource,
    Cv::Editor,
];

impl Cv {
    pub fn name(self) -> &'static str {
        match self {
            Cv::Version => "lb_version",
            Cv::Quota => "lb_quota",
            Cv::QuotaMode => "lb_quota_mode",
            Cv::Difficulty => "lb_difficulty",
            Cv::Style => "lb_style",
            Cv::CmdRate => "lb_cmd_rate",
            Cv::ForceRespawn => "lb_force_respawn",
            Cv::GameMode => "lb_game_mode",
            Cv::GunGame => "lb_gungame",
            Cv::Telemetry => "lb_telemetry",
            Cv::TelemetryPort => "lb_telemetry_port",
            Cv::TelemetrySecret => "lb_telemetry_secret",
            Cv::Freeze => "lb_freeze",
            Cv::Debug => "lb_debug",
            Cv::Dev => "lb_dev",
            Cv::LogLevel => "lb_log_level",
            Cv::NavSource => "lb_nav_source",
            Cv::Editor => "lb_editor",
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Cvars {
    handles: Vec<(Cv, CvarHandle, String)>,
    game: Vec<(String, Option<CvarHandle>)>,
}

impl Cvars {
    pub fn register(&mut self, host: &mut dyn Host, defaults: &[(Cv, String)]) {
        let specs: Vec<CvarSpec> = defaults
            .iter()
            .map(|(cv, value)| CvarSpec {
                name: cv.name().to_string(),
                default_value: value.clone(),
                flags: match cv {
                    Cv::Version => lb_ffi::LB_CVAR_SERVER | lb_ffi::LB_CVAR_READONLY,
                    Cv::TelemetrySecret => lb_ffi::LB_CVAR_PROTECTED,
                    _ => 0,
                },
            })
            .collect();
        let handles = host.cvar_register(&specs);
        self.handles.clear();
        for ((cv, value), handle) in defaults.iter().zip(handles) {
            match handle {
                Some(h) => self.handles.push((*cv, h, value.clone())),
                None => tracing::warn!("cvar {} could not be registered", cv.name()),
            }
        }
    }

    fn handle(&self, cv: Cv) -> Option<(CvarHandle, &str)> {
        self.handles
            .iter()
            .find(|(c, _, _)| *c == cv)
            .map(|(_, h, last)| (*h, last.as_str()))
    }

    pub fn handle_of(&self, cv: Cv) -> Option<CvarHandle> {
        self.handle(cv).map(|(h, _)| h)
    }

    /// Replaces the last seen value of `cv`, unless it is empty.
    pub fn redact(&mut self, cv: Cv, with: &str) {
        for (c, _, last) in &mut self.handles {
            if *c == cv && !last.is_empty() {
                *last = with.to_string();
            }
        }
    }

    pub fn get(&self, host: &mut dyn Host, cv: Cv) -> Option<String> {
        self.handle(cv).map(|(h, _)| host.cvar_string(h))
    }

    pub fn get_f32(&self, host: &mut dyn Host, cv: Cv) -> Option<f32> {
        self.handle(cv).map(|(h, _)| host.cvar_float(h))
    }

    pub fn set(&mut self, host: &mut dyn Host, cv: Cv, value: &str) {
        if let Some(entry) = self.handles.iter_mut().find(|(c, _, _)| *c == cv) {
            host.cvar_set(entry.1, value);
            entry.2 = value.to_string();
        }
    }

    /// Returns cvars whose value changed since the last call (runtime overrides).
    pub fn poll_changes(&mut self, host: &mut dyn Host) -> Vec<(Cv, String)> {
        let mut changed = Vec::new();
        for (cv, h, last) in &mut self.handles {
            let now = host.cvar_string(*h);
            if now != *last {
                *last = now.clone();
                changed.push((*cv, now));
            }
        }
        changed
    }

    /// Game cvars are looked up lazily because plugins (AMXX) register some of them late.
    pub fn game_value(&mut self, host: &mut dyn Host, name: &str) -> Option<String> {
        let idx = match self.game.iter().position(|(n, _)| n == name) {
            Some(i) => i,
            None => {
                self.game.push((name.to_string(), None));
                self.game.len() - 1
            }
        };
        if self.game[idx].1.is_none() {
            self.game[idx].1 = host.cvar_find(name);
        }
        self.game[idx].1.map(|h| host.cvar_string(h))
    }
}
