//! What the server and the map editor both load for a map's navigation: its overlay files, the graph the server made
//! of it, the graph the editor saved with the overlays applied, and the world as the generator leaves it (movers at
//! rest, push fields), which overlay patches are checked against.

use std::path::{Path, PathBuf};

use lb_bsp::BspWorld;
use lb_bsp::mech::Mechanisms;
use lb_config::overlay::{OverlayFile, Patch};
use lb_nav::NavGraph;
use lb_nav::store::GraphKey;

use crate::cache::{GENERATOR, GraphCache};

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
