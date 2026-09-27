//! Compatibility profile printed at map start and via `lb compat`.

use std::fmt::Write;

use lb_config::yaml::quote;

#[derive(Clone, Debug, Default)]
pub struct CompatibilityProfile {
    pub core_version: String,
    pub adapter_version: String,
    pub abi_version: u32,
    pub platform: String,
    pub engine: String,
    pub engine_version: String,
    pub rehlds: Option<String>,
    pub metamod: String,
    pub metamod_hook_tables: bool,
    pub gamedll: String,
    pub gamedll_path: String,
    pub bugfixedhl: bool,
    pub channels: Vec<(&'static str, bool)>,
    pub resolved_messages: Vec<(String, i32)>,
    pub missing_messages: Vec<String>,
    pub event_names: usize,
    pub sys_ticrate: f32,
    pub plugins: Vec<(String, String)>,
    pub map: String,
    pub late_load: bool,
    pub install_dir: String,
    pub config: String,
    pub names: String,
}

impl CompatibilityProfile {
    pub fn to_yaml(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(s, "schema: lambdabots/compat@1");
        let _ = writeln!(s, "core_version: \"{}\"", self.core_version);
        let _ = writeln!(s, "adapter_version: \"{}\"", self.adapter_version);
        let _ = writeln!(s, "abi_version: {}", self.abi_version);
        let _ = writeln!(s, "platform: {}", self.platform);
        let _ = writeln!(s, "engine: {}", self.engine);
        let _ = writeln!(s, "engine_version: \"{}\"", self.engine_version);
        let _ = writeln!(s, "rehlds_api: {}", self.rehlds.as_deref().unwrap_or("none"));
        let _ = writeln!(s, "metamod: \"{}\"", self.metamod);
        let _ = writeln!(s, "metamod_hook_tables: {}", self.metamod_hook_tables);
        let _ = writeln!(s, "gamedll: \"{}\"", self.gamedll);
        let _ = writeln!(s, "gamedll_path: \"{}\"", self.gamedll_path);
        let _ = writeln!(s, "bugfixedhl: {}", self.bugfixedhl);
        let _ = writeln!(s, "map: {}", self.map);
        let _ = writeln!(s, "late_load: {}", self.late_load);
        let _ = writeln!(s, "install_dir: {}", quote(&self.install_dir));
        let _ = writeln!(s, "config: {}", quote(&self.config));
        let _ = writeln!(s, "names: {}", quote(&self.names));
        let _ = writeln!(s, "sys_ticrate: {}", self.sys_ticrate);
        let _ = writeln!(s, "channels:");
        for (name, on) in &self.channels {
            let _ = writeln!(s, "  {name}: {on}");
        }
        let _ = writeln!(s, "messages_resolved: {}", self.resolved_messages.len());
        if !self.missing_messages.is_empty() {
            let _ = writeln!(s, "messages_missing: [{}]", self.missing_messages.join(", "));
        }
        let _ = writeln!(s, "event_names: {}", self.event_names);
        if !self.plugins.is_empty() {
            let _ = writeln!(s, "plugins:");
            for (k, v) in &self.plugins {
                let _ = writeln!(s, "  {k}: \"{v}\"");
            }
        }
        s
    }
}
