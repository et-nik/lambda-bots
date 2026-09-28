//! What the bots learn by playing, kept between games: where they get hurt on each map
//! (`data/experience/<map>.json`) and how long items take to come back on this server (`data/learned/respawn.json`).

use std::path::{Path, PathBuf};

use lb_knowledge::Learned;
use lb_mapknow::{Experience, MapTactics};

/// Seconds of play between saves of what was learned.
pub const SAVE_EVERY: f64 = 300.0;

pub fn experience_file(install: &Path, map: &str) -> PathBuf {
    install.join("data").join("experience").join(format!("{map}.json"))
}

/// What was learned on `map` before, put on the nodes of `tactics`; nothing when no file was kept or it does not
/// read.
pub fn load_experience(install: &Path, map: &str, tactics: &MapTactics) -> Experience {
    let path = experience_file(install, map);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Experience::new(tactics.origins.len());
    };
    match Experience::parse(&text) {
        Ok(f) => {
            let (x, placed) = Experience::from_file(&f, tactics);
            tracing::info!(
                "{map}: experience of {placed} of {} places read from {}",
                f.entries.len(),
                path.display()
            );
            x
        }
        Err(e) => {
            tracing::warn!("{}: {e}; the bots start learning the map afresh", path.display());
            Experience::new(tactics.origins.len())
        }
    }
}

/// Writes what was learned on `map` when something was since the last save (whole or not at all).
pub fn save_experience(install: &Path, map: &str, tactics: &MapTactics, x: &mut Experience) {
    if !x.changed {
        return;
    }
    let path = experience_file(install, map);
    let written = serde_json::to_string_pretty(&x.to_file(map, tactics))
        .map_err(|e| e.to_string())
        .and_then(|text| {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            }
            let tmp = path.with_extension("json.tmp");
            std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
            std::fs::rename(&tmp, &path).map_err(|e| format!("{}: {e}", path.display()))
        });
    match written {
        Ok(()) => x.changed = false,
        Err(e) => tracing::warn!("{map}: experience not kept: {e}"),
    }
}

pub fn respawn_file(install: &Path) -> PathBuf {
    install.join("data").join("learned").join("respawn.json")
}

/// The respawn file: items (health, armor, the long jump), weapons and ammo, in seconds with the timings behind.
#[derive(serde::Serialize, serde::Deserialize)]
struct RespawnFile {
    schema: String,
    items: Learned,
    weapons: Learned,
    ammo: Learned,
}

const RESPAWN_SCHEMA: &str = "lambdabots/respawn@1";

/// Respawn times timed on this server before; nothing timed when the file is missing or does not read.
pub fn load_respawns(install: &Path) -> [Learned; 3] {
    let path = respawn_file(install);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return [Learned::default(); 3];
    };
    match serde_json::from_str::<RespawnFile>(&text) {
        Ok(f) if f.schema == RESPAWN_SCHEMA => [f.items, f.weapons, f.ammo],
        Ok(f) => {
            tracing::warn!("{}: schema `{}`, expected `{RESPAWN_SCHEMA}`", path.display(), f.schema);
            [Learned::default(); 3]
        }
        Err(e) => {
            tracing::warn!("{}: {e}", path.display());
            [Learned::default(); 3]
        }
    }
}

pub fn save_respawns(install: &Path, r: &[Learned; 3]) {
    let path = respawn_file(install);
    let f = RespawnFile {
        schema: RESPAWN_SCHEMA.into(),
        items: r[0],
        weapons: r[1],
        ammo: r[2],
    };
    let written = serde_json::to_string_pretty(&f)
        .map_err(|e| e.to_string())
        .and_then(|text| {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            }
            let tmp = path.with_extension("json.tmp");
            std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
            std::fs::rename(&tmp, &path).map_err(|e| format!("{}: {e}", path.display()))
        });
    if let Err(e) = written {
        tracing::warn!("respawn times not kept: {e}");
    }
}
