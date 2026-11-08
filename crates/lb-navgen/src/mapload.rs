//! What the server and the map editor both load for a map's navigation: its overlay files, the graph the server made
//! of it, the graph the editor saved with the overlays applied, and the world as the generator leaves it (movers at
//! rest, push fields), which overlay patches are checked against.

use std::path::{Path, PathBuf};

use lb_bsp::BspWorld;
use lb_bsp::mech::Mechanisms;
use lb_config::moves::MovesFile;
use lb_config::overlay::{OverlayFile, Patch};
use lb_nav::NavGraph;
use lb_nav::store::GraphKey;

use crate::cache::{GENERATOR, GraphCache};
use crate::climbs::{HighGround, to_make};

/// A map's overlay files under `maps/<map>/`, in the order they apply: the editors', then the hand-written one.
pub const OVERLAYS: [&str; 2] = ["editor.yaml", "overlay.yaml"];

/// `maps/<map>/<name>` under the install directory.
pub fn overlay_path(install: &Path, map: &str, name: &str) -> PathBuf {
    install.join("maps").join(map).join(name)
}

/// The map's overlays (`OVERLAYS`) in the order they apply. A file that does not read is left out with a warning.
pub fn read_overlays(install: &Path, map: &str, bsp_size: u64) -> Vec<OverlayFile> {
    read_overlay_files(install, map, bsp_size)
        .into_iter()
        .map(|(_, o)| o)
        .collect()
}

/// `read_overlays` with the name of each file.
pub fn read_overlay_files(install: &Path, map: &str, bsp_size: u64) -> Vec<(&'static str, OverlayFile)> {
    OVERLAYS
        .iter()
        .filter_map(|&name| {
            let path = overlay_path(install, map, name);
            let text = std::fs::read_to_string(&path).ok()?;
            match OverlayFile::parse(&text, &path.display().to_string()) {
                Ok(o) if o.bsp_size.is_some_and(|s| s != bsp_size) => {
                    tracing::warn!(
                        "{}: made for a {}-byte {map}.bsp, this one has {bsp_size} bytes; not applied",
                        path.display(),
                        o.bsp_size.unwrap_or(0)
                    );
                    None
                }
                Ok(o) => Some((name, o)),
                Err(e) => {
                    tracing::warn!("{e}; not applied");
                    None
                }
            }
        })
        .collect()
}

/// `maps/<map>/moves.yaml`: how players were seen to get about the map (`lb-cli demo moves`), which the generator
/// makes gauss boosts onto high ground from.
pub const MOVES: &str = "moves.yaml";

/// The high ground the generator makes gauss boosts onto on `map`: that of its `moves.yaml` climbed often enough
/// (`climbs::to_make`), with what came of the file for the log. None without the file, or when it does not read; a
/// file that does not parse or is for another map or another build of it is not used.
pub fn high_ground(install: &Path, map: &str, bsp_size: u64) -> (Vec<HighGround>, Option<String>) {
    high_ground_in(&overlay_path(install, map, MOVES), map, bsp_size)
}

/// `high_ground` of the moves file at `path`.
pub fn high_ground_in(path: &Path, map: &str, bsp_size: u64) -> (Vec<HighGround>, Option<String>) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return (Vec::new(), None);
    };
    match high_ground_from(&text, path, map, bsp_size) {
        Ok((made, note)) => (made, Some(note)),
        Err(e) => (Vec::new(), Some(format!("{e}; not used"))),
    }
}

/// `high_ground_in` of a moves file named by hand: an error when it does not read or parse, or is for another map or
/// another build of it.
pub fn read_high_ground(path: &Path, map: &str, bsp_size: u64) -> Result<(Vec<HighGround>, String), String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    high_ground_from(&text, path, map, bsp_size)
}

fn high_ground_from(text: &str, path: &Path, map: &str, bsp_size: u64) -> Result<(Vec<HighGround>, String), String> {
    let f = MovesFile::parse(text, &path.display().to_string()).map_err(|e| e.to_string())?;
    if !f.map.eq_ignore_ascii_case(map) {
        return Err(format!("{}: seen on {}, not on {map}", path.display(), f.map));
    }
    if let Some(made) = f.bsp_size.filter(|&s| s != bsp_size) {
        return Err(format!(
            "{}: seen on a {made}-byte {map}.bsp, this one has {bsp_size} bytes",
            path.display()
        ));
    }
    let made = to_make(&f);
    let note = format!(
        "{}: boosts onto {} of the {} places up high players got onto, climbed often enough (from {} recordings, {} \
         trusted)",
        path.display(),
        made.len(),
        f.high_ground.len(),
        f.source.recordings,
        f.source.trusted
    );
    Ok((made, note))
}

/// Leaves the world as the generator does: push fields found, movers and breakables where the map starts them.
/// Overlay patches are checked in this world whether the graph was made now or read from the cache.
pub fn prepare_world(world: &mut BspWorld, mech: &Mechanisms) {
    world.pushes = mech.push_fields();
    crate::site::rest_poses(world, mech);
}

/// The graph the server made of this build of the map, most recently used first, from its cache under
/// `<install>/nav`: the one it plays on, with its key. Read without marking it used.
pub fn kept_graph(install: &Path, map: &str, world: &BspWorld) -> Option<(GraphKey, NavGraph)> {
    let (bsp, bsp_size) = world.bsp.fingerprint;
    GraphCache::new(&install.join("nav"), map)
        .kept()
        .into_iter()
        .filter(|(_, key)| key.bsp == bsp && key.bsp_size == bsp_size && key.generator == GENERATOR && key.overlay == 0)
        .find_map(|(path, _)| {
            let bytes = std::fs::read(&path).ok()?;
            lb_nav::store::read(&bytes).ok()
        })
}

/// The graph the map editor saves beside the overlay files (`maps/<map>/editor.lbnav`): the server's graph with the
/// map's overlays applied. The server plays on it as it is while its key is the one of the graph it would load with
/// the overlays it reads; otherwise it applies the overlays itself.
pub const EDITED: &str = "editor.lbnav";

/// A hash of `patches` in the order they apply, for the key of a graph they are applied to (never 0, the key of a
/// graph without them).
pub fn patches_hash(patches: &[Patch]) -> u64 {
    xxhash_rust::xxh3::xxh3_64(format!("{patches:?}").as_bytes()).max(1)
}

/// The key of the graph keyed `base` with `patches` applied.
pub fn edited_key(base: &GraphKey, patches: &[Patch]) -> GraphKey {
    GraphKey {
        overlay: patches_hash(patches),
        ..base.clone()
    }
}

/// The graph the editor saved, when it is the graph keyed `base` with exactly `patches` applied.
pub fn edited_graph(install: &Path, map: &str, base: &GraphKey, patches: &[Patch]) -> Option<NavGraph> {
    let path = overlay_path(install, map, EDITED);
    let bytes = std::fs::read(&path).ok()?;
    match lb_nav::store::read(&bytes) {
        Ok((key, graph)) if key == edited_key(base, patches) => Some(graph),
        Ok(_) => {
            tracing::info!(
                "{}: saved with other overlays or on another graph; the overlays are applied anew",
                path.display()
            );
            None
        }
        Err(e) => {
            tracing::warn!("{}: {e}; the overlays are applied anew", path.display());
            None
        }
    }
}

/// Writes the editor's graph, whole or not at all.
pub fn write_edited(install: &Path, map: &str, key: &GraphKey, graph: &NavGraph) -> Result<PathBuf, String> {
    let path = overlay_path(install, map, EDITED);
    let dir = path.parent().expect("maps/<map>/editor.lbnav has a parent");
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let tmp = path.with_extension("lbnav.tmp");
    std::fs::write(&tmp, lb_nav::store::write(graph, key)).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

/// Removes the editor's graph once it no longer goes with the overlays; true when there was one.
pub fn remove_edited(install: &Path, map: &str) -> bool {
    std::fs::remove_file(overlay_path(install, map, EDITED)).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEEN_TWICE: &str = "schema: lambdabots/moves@1
map: dm_example
bsp_size: 1000
source:
  recordings: 4
  minutes: 60.0
high_ground:
  - seen: 2
    players: 1
    recordings: 1
    ways:
      - from: [0, 0, 0]
        to: [100, 0, 192]
        seen: 2
";

    #[test]
    fn a_moves_file_is_used_only_on_its_own_build_of_its_own_map() {
        let dir = std::env::temp_dir().join(format!("lb-navgen-moves-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(MOVES);
        std::fs::write(&path, SEEN_TWICE).unwrap();
        // Climbed too seldom for a boost: read all the same.
        let (made, note) = read_high_ground(&path, "DM_Example", 1000).unwrap();
        assert!(made.is_empty() && note.contains("onto 0 of the 1"), "{note}");
        let other = read_high_ground(&path, "dm_other", 1000).unwrap_err();
        assert!(other.contains("not on dm_other"), "{other}");
        let build = read_high_ground(&path, "dm_example", 999).unwrap_err();
        assert!(build.contains("999 bytes"), "{build}");
        let (made, note) = high_ground_in(&path, "dm_other", 1000);
        assert!(made.is_empty() && note.is_some_and(|n| n.ends_with("not used")));
        std::fs::write(&path, "schema: lambdabots/moves@1\nmap: [").unwrap();
        assert!(read_high_ground(&path, "dm_example", 1000).is_err());
        let none = dir.join("none.yaml");
        let (made, note) = high_ground_in(&none, "dm_example", 1000);
        assert!(made.is_empty() && note.is_none());
        assert!(read_high_ground(&none, "dm_example", 1000).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}
