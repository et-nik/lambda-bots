//! A map's navigation for the page: the graph the server plays on, the overlay files, what the edited overlay comes
//! to, and routes over it.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use lb_bsp::BspWorld;
use lb_bsp::mech::Mechanisms;
use lb_config::overlay::OverlayFile;
use lb_core::Vec3;
use lb_kin::Physics;
use lb_nav::graph::{LinkKind, NavGraph, NavLink, NodeId};
use lb_navgen::mapload;
use lb_navgen::patch::{self, Outcome};
use serde::Serialize;

/// Maps whose navigation is kept at once (each keeps its world for checking changes).
const KEEP: usize = 2;
/// A route's ends snap to the nearest node within this distance.
const ROUTE_SNAP: f32 = 256.0;

/// Where the graph the changes are made on came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Origin {
    /// The server's cache: the graph it plays on.
    Server,
    /// Made by the editor (the server has not played this build of the map), with the default physics.
    Made,
}

/// A map's graph without overlays, with the world its changes are checked in.
pub struct NavMap {
    pub map: String,
    pub bsp_size: u64,
    pub base: NavGraph,
    pub origin: Origin,
    world: Mutex<BspWorld>,
    mech: Mechanisms,
    last: Mutex<Option<(String, Arc<Patched>)>>,
}

/// The graph with the overlays applied, and what each of their patches did.
pub struct Patched {
    pub graph: NavGraph,
    pub editor: Vec<Outcome>,
    pub overlay: Vec<Outcome>,
}

impl NavMap {
    pub fn load(map: &str, bsp: &Path, install: &Path) -> Result<NavMap, String> {
        let bytes = std::fs::read(bsp).map_err(|e| format!("{}: {e}", bsp.display()))?;
        let mut world = BspWorld::load(&bytes).map_err(|e| format!("{}: {e}", bsp.display()))?;
        let mech = Mechanisms::from_world(&world);
        let (base, origin) = match mapload::kept_graph(install, map, &world) {
            Some(g) => (g, Origin::Server),
            None => {
                let started = std::time::Instant::now();
                let g = lb_navgen::generate(&mut world, &mech, &lb_navgen::GenOptions::default(), "editor").graph;
                if g.is_empty() {
                    return Err("the generator found no floor".into());
                }
                tracing::info!(
                    "{map}: no graph of the server's, made one in {} ms",
                    started.elapsed().as_millis()
                );
                (g, Origin::Made)
            }
        };
        mapload::prepare_world(&mut world, &mech);
        Ok(NavMap {
            map: map.to_string(),
            bsp_size: bytes.len() as u64,
            base,
            origin,
            world: Mutex::new(world),
            mech,
            last: Mutex::new(None),
        })
    }

    /// The graph with `editor` (the editor file as the page has it) and the hand-written overlay applied, in the
    /// server's order. The last result is kept: the page asks again for routes on the same changes.
    pub fn patched(&self, editor: &OverlayFile, overlay: Option<&OverlayFile>) -> Arc<Patched> {
        let key = serde_json::to_string(&(&editor.nav.patches, overlay.map(|o| &o.nav.patches))).unwrap_or_default();
        if let Some((k, p)) = self.last.lock().expect("no panics while locked").as_ref()
            && *k == key
        {
            return p.clone();
        }
        let mut patches = editor.nav.patches.clone();
        let split = patches.len();
        patches.extend(overlay.iter().flat_map(|o| o.nav.patches.iter().cloned()));
        let (graph, report) = {
            let mut world = self.world.lock().expect("no panics while locked");
            patch::apply(self.base.clone(), &patches, &mut world, &self.mech, Physics::default())
        };
        let mut editor_out = report.outcomes;
        let overlay_out = editor_out.split_off(split);
        let p = Arc::new(Patched {
            graph: graph.with_landmarks(),
            editor: editor_out,
            overlay: overlay_out,
        });
        *self.last.lock().expect("no panics while locked") = Some((key, p.clone()));
        p
    }
}

/// A map's navigation kept, with the BSP file and its modification time it was loaded from.
type Kept = (PathBuf, Option<std::time::SystemTime>, Arc<NavMap>);

/// Navigation of the maps opened last.
#[derive(Default)]
pub struct NavMaps {
    kept: Mutex<Vec<Kept>>,
    /// Held while a map loads: making a graph takes a while, and two pages asking at once make one.
    loading: Mutex<()>,
}

impl NavMaps {
    pub fn get(&self, map: &str, bsp: &Path, install: &Path) -> Result<Arc<NavMap>, String> {
        let stamp = std::fs::metadata(bsp).and_then(|m| m.modified()).ok();
        let find = || {
            let mut kept = self.kept.lock().expect("no panics while locked");
            let i = kept.iter().position(|(p, s, _)| p == bsp && *s == stamp)?;
            let k = kept.remove(i);
            let nav = k.2.clone();
            kept.insert(0, k);
            Some(nav)
        };
        if let Some(nav) = find() {
            return Ok(nav);
        }
        let _one = self.loading.lock().expect("no panics while locked");
        if let Some(nav) = find() {
            return Ok(nav);
        }
        let nav = Arc::new(NavMap::load(map, bsp, install)?);
        let mut kept = self.kept.lock().expect("no panics while locked");
        kept.retain(|(p, _, _)| p != bsp);
        kept.insert(0, (bsp.to_path_buf(), stamp, nav.clone()));
        kept.truncate(KEEP);
        Ok(nav)
    }
}

/// Graph data for the page, flat: `[x, y, z, flags]` a node and `[from, to, kind, flags, centiseconds]` a link,
/// kinds as in `KINDS`.
#[derive(Clone, Debug, Default, Serialize)]
pub struct GraphJson {
    pub nodes: Vec<f32>,
    pub links: Vec<f32>,
}

pub const KINDS: [&str; 13] = [
    "walk",
    "crouch",
    "jump",
    "drop",
    "ladder",
    "swim",
    "door",
    "lift",
    "teleport",
    "breakable",
    "push",
    "longjump",
    "gauss_boost",
];

fn tenth(v: f32) -> f32 {
    (v * 10.0).round() / 10.0
}

fn push_node(out: &mut Vec<f32>, g: &NavGraph, n: usize) {
    let node = &g.nodes[n];
    out.extend_from_slice(&[
        tenth(node.origin.x),
        tenth(node.origin.y),
        tenth(node.origin.z),
        node.flags.bits() as f32,
    ]);
}

fn push_link(out: &mut Vec<f32>, from: usize, l: &NavLink) {
    out.extend_from_slice(&[
        from as f32,
        l.to as f32,
        l.kind.index() as f32,
        l.flags.bits() as f32,
        (l.cost * 100.0).round(),
    ]);
}

pub fn graph_json(g: &NavGraph) -> GraphJson {
    let mut out = GraphJson::default();
    for n in 0..g.len() {
        push_node(&mut out.nodes, g, n);
        for l in g.links(n as NodeId) {
            push_link(&mut out.links, n, l);
        }
    }
    out
}

/// What the overlays change in the base graph: the nodes put in (numbered on from the base's), links put in or
/// changed, links taken out or changed (`[from, to]` each), and each patch's outcome.
#[derive(Clone, Debug, Serialize)]
pub struct Preview {
    pub nodes: Vec<f32>,
    pub added: Vec<f32>,
    pub removed: Vec<u32>,
    pub editor: Vec<Outcome>,
    pub overlay: Vec<Outcome>,
}

fn same(a: &NavLink, b: &NavLink) -> bool {
    a.to == b.to && a.kind == b.kind && a.flags == b.flags
}

pub fn preview(base: &NavGraph, p: &Patched) -> Preview {
    let g = &p.graph;
    let mut out = Preview {
        nodes: Vec::new(),
        added: Vec::new(),
        removed: Vec::new(),
        editor: p.editor.clone(),
        overlay: p.overlay.clone(),
    };
    for n in base.len()..g.len() {
        push_node(&mut out.nodes, g, n);
    }
    for n in 0..g.len() {
        let now = g.links(n as NodeId);
        let before = if n < base.len() { base.links(n as NodeId) } else { &[] };
        for l in now.iter().filter(|l| !before.iter().any(|b| same(b, l))) {
            push_link(&mut out.added, n, l);
        }
        for b in before.iter().filter(|b| !now.iter().any(|l| same(b, l))) {
            out.removed.extend_from_slice(&[n as u32, b.to]);
        }
    }
    out
}

#[derive(Clone, Debug, Serialize)]
pub struct Leg {
    pub from: NodeId,
    pub to: NodeId,
    pub kind: &'static str,
    pub cost: f32,
}

#[derive(Clone, Debug, Serialize)]
pub struct Route {
    pub start: NodeId,
    pub goal: NodeId,
    pub nodes: Vec<NodeId>,
    pub legs: Vec<Leg>,
    /// Seconds by the graph's costs.
    pub time: f32,
    /// The way without tricks when the route takes one: `None` when there is none.
    pub plain: Option<f32>,
}

/// The way a bot plans from the node nearest `from` to the one nearest `to`, with long jumps and gauss boosts when
/// the bot can do them.
pub fn route(g: &NavGraph, from: Vec3, to: Vec3, longjump: bool, gauss: bool) -> Result<Route, String> {
    let near = |p: Vec3| g.nearest(p, ROUTE_SNAP, 1).first().map(|(n, _)| *n);
    let (Some(start), Some(goal)) = (near(from), near(to)) else {
        return Err(format!("no node within {ROUTE_SNAP} units of an end"));
    };
    let can = move |_: NodeId, l: &NavLink| match l.kind {
        LinkKind::LongJump if !longjump => f32::INFINITY,
        LinkKind::GaussBoost if !gauss => f32::INFINITY,
        _ => 0.0,
    };
    let nodes =
        lb_nav::plan::plan(g, start, goal, &can).ok_or_else(|| format!("no way from node {start} to {goal}"))?;
    let legs: Vec<Leg> = nodes
        .windows(2)
        .filter_map(|w| {
            let l = g.find_link(w[0], w[1])?;
            Some(Leg {
                from: w[0],
                to: w[1],
                kind: l.kind.as_str(),
                cost: l.cost,
            })
        })
        .collect();
    let tricks = nodes
        .windows(2)
        .any(|w| g.find_link(w[0], w[1]).is_some_and(|l| l.kind.is_trick()));
    let plain = if tricks {
        lb_nav::plan::plan(g, start, goal, &lb_nav::plan::plain).map(|p| lb_nav::plan::path_time(g, &p))
    } else {
        None
    };
    Ok(Route {
        start,
        goal,
        time: lb_nav::plan::path_time(g, &nodes),
        nodes,
        legs,
        plain,
    })
}

/// An overlay file as it is on disk: parsed, with the version a save must name to replace it.
#[derive(Clone, Debug, Serialize)]
pub struct OnDisk {
    pub file: Option<OverlayFile>,
    /// `none` for no file, else blake3 of its text.
    pub version: String,
    /// Why the file does not read.
    pub error: Option<String>,
}

pub fn read_overlay(install: &Path, map: &str, name: &str) -> OnDisk {
    let path = mapload::overlay_path(install, map, name);
    match std::fs::read_to_string(&path) {
        Err(_) => OnDisk {
            file: None,
            version: "none".into(),
            error: None,
        },
        Ok(text) => {
            let version = blake3::hash(text.as_bytes()).to_hex().to_string();
            match OverlayFile::parse(&text, &path.display().to_string()) {
                Ok(f) => OnDisk {
                    file: Some(f),
                    version,
                    error: None,
                },
                Err(e) => OnDisk {
                    file: None,
                    version,
                    error: Some(e.to_string()),
                },
            }
        }
    }
}

pub enum SaveError {
    /// The file changed since the page read it.
    Conflict(Box<OnDisk>),
    Invalid(String),
    Io(String),
}

/// Writes the editor file when the one on disk is still `base`; returns the new version.
pub fn save_editor(install: &Path, map: &str, file: &OverlayFile, base: &str) -> Result<String, SaveError> {
    let now = read_overlay(install, map, mapload::OVERLAYS[0]);
    if now.version != base {
        return Err(SaveError::Conflict(Box::new(now)));
    }
    if file.map != map {
        return Err(SaveError::Invalid(format!("the file is for {}, not {map}", file.map)));
    }
    let path = mapload::overlay_path(install, map, mapload::OVERLAYS[0]);
    let text = file.to_yaml().map_err(|e| SaveError::Invalid(e.to_string()))?;
    OverlayFile::parse(&text, &path.display().to_string()).map_err(|e| SaveError::Invalid(e.to_string()))?;
    let dir = path.parent().expect("maps/<map>/editor.yaml has a parent");
    std::fs::create_dir_all(dir).map_err(|e| SaveError::Io(format!("{}: {e}", dir.display())))?;
    let tmp = path.with_extension("yaml.tmp");
    std::fs::write(&tmp, &text).map_err(|e| SaveError::Io(format!("{}: {e}", tmp.display())))?;
    std::fs::rename(&tmp, &path).map_err(|e| SaveError::Io(format!("{}: {e}", path.display())))?;
    Ok(blake3::hash(text.as_bytes()).to_hex().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lb_config::overlay::{Patch, Place};

    fn stand() -> Option<PathBuf> {
        let home = std::env::var("HOME").ok()?;
        let maps = std::env::var("LB_MAPS_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| Path::new(&home).join("Git/half-life/xash3d-fwgs-apple-arm64/valve/maps"));
        maps.join("crossfire.bsp")
            .is_file()
            .then_some(maps.join("crossfire.bsp"))
    }

    #[test]
    fn saves_refuse_a_file_changed_since_it_was_read() {
        let install = std::env::temp_dir().join(format!("lb-editor-save-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&install);
        let mut f = OverlayFile::new("crossfire");
        f.places.push(Place {
            name: "roof".into(),
            at: [0.0, 0.0, 0.0],
            radius: 64.0,
            tags: vec![],
        });
        let v1 = save_editor(&install, "crossfire", &f, "none").ok().expect("a new file");
        assert!(matches!(
            save_editor(&install, "crossfire", &f, "none"),
            Err(SaveError::Conflict(_))
        ));
        f.places[0].radius = 96.0;
        let v2 = save_editor(&install, "crossfire", &f, &v1)
            .ok()
            .expect("on the version read");
        assert_ne!(v1, v2);
        let read = read_overlay(&install, "crossfire", "editor.yaml");
        assert_eq!(
            (read.version.as_str(), read.file.unwrap().places[0].radius),
            (v2.as_str(), 96.0)
        );
        f.places[0].radius = -1.0;
        assert!(matches!(
            save_editor(&install, "crossfire", &f, &v2),
            Err(SaveError::Invalid(_))
        ));
        assert!(matches!(
            save_editor(&install, "stalkyard", &f, "none"),
            Err(SaveError::Invalid(_))
        ));
        let _ = std::fs::remove_dir_all(&install);
    }

    #[test]
    fn a_preview_shows_what_the_changes_do_and_routes_take_them() {
        let Some(bsp) = stand() else { return };
        let install = std::env::temp_dir().join(format!("lb-editor-nav-{}", std::process::id()));
        let nav = NavMap::load("crossfire", &bsp, &install).expect("crossfire loads");
        assert_eq!(nav.origin, Origin::Made, "no server cache in an empty install");
        let g = &nav.base;
        let (a, b) = (0 as NodeId, g.links(0)[0].to);
        let mut editor = OverlayFile::new("crossfire");
        editor.nav.patches.push(Patch::RemoveLink {
            from: g.node(a).origin.to_array(),
            to: g.node(b).origin.to_array(),
            both: false,
            note: String::new(),
        });
        let p = nav.patched(&editor, None);
        assert!(p.editor[0].ok);
        let diff = preview(&nav.base, &p);
        assert_eq!(diff.removed, vec![a, b]);
        assert!(diff.added.is_empty() && diff.nodes.is_empty());
        assert!(
            Arc::ptr_eq(&p, &nav.patched(&editor, None)),
            "the same changes are applied once"
        );
        let json = graph_json(&nav.base);
        assert_eq!(json.nodes.len(), g.len() * 4);
        let far = (0..g.len() as NodeId)
            .max_by(|&x, &y| {
                g.node(x)
                    .origin
                    .distance(g.node(a).origin)
                    .total_cmp(&g.node(y).origin.distance(g.node(a).origin))
            })
            .unwrap();
        let r = route(&p.graph, g.node(a).origin, g.node(far).origin, false, false).expect("a way across the map");
        assert_eq!((r.start, *r.nodes.last().unwrap()), (a, far));
        assert!(r.time > 0.0 && r.legs.len() + 1 == r.nodes.len());
        assert!(r.legs.iter().all(|l| l.kind != "longjump" && l.kind != "gauss_boost"));
        let _ = std::fs::remove_dir_all(&install);
    }
}
