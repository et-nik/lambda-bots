//! What the server and the map editor both load for a map's navigation: its overlay files, the graph the server made
//! of it, and the world as the generator leaves it (movers at rest, push fields), which overlay patches are checked
//! against.

use std::path::{Path, PathBuf};

use lb_bsp::BspWorld;
use lb_bsp::mech::Mechanisms;
use lb_config::overlay::OverlayFile;
use lb_nav::NavGraph;

use crate::cache::{GENERATOR, GraphCache};

/// A map's overlay files under `maps/<map>/`, in the order they apply: the editors', then the hand-written one.
pub const OVERLAYS: [&str; 2] = ["editor.yaml", "overlay.yaml"];

/// `maps/<map>/<name>` under the install directory.
pub fn overlay_path(install: &Path, map: &str, name: &str) -> PathBuf {
    install.join("maps").join(map).join(name)
}

/// The map's overlays (`OVERLAYS`) in the order they apply. A file that does not read is left out with a warning.
pub fn read_overlays(install: &Path, map: &str, bsp_size: u64) -> Vec<OverlayFile> {
    OVERLAYS
        .iter()
        .filter_map(|name| {
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
                Ok(o) => Some(o),
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
/// `<install>/nav`: the one it plays on. Read without marking it used.
pub fn kept_graph(install: &Path, map: &str, world: &BspWorld) -> Option<NavGraph> {
    let (bsp, bsp_size) = world.bsp.fingerprint;
    GraphCache::new(&install.join("nav"), map)
        .kept()
        .into_iter()
        .filter(|(_, key)| key.bsp == bsp && key.bsp_size == bsp_size && key.generator == GENERATOR)
        .find_map(|(path, _)| {
            let bytes = std::fs::read(&path).ok()?;
            lb_nav::store::read(&bytes).ok().map(|(_, graph)| graph)
        })
}
