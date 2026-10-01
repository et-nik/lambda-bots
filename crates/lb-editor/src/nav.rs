//! A map's navigation for the page: the graph the server plays on, the overlay files, what the edited overlay comes
//! to, and routes over it.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use lb_bsp::BspWorld;
use lb_bsp::mech::Mechanisms;
use lb_config::overlay::{OverlayFile, Patch};
use lb_core::Vec3;
use lb_kin::Physics;
use lb_nav::graph::{LinkKind, NavGraph, NavLink, NodeId};
use lb_nav::store::GraphKey;
use lb_nav::validate::{WalkCheck, walk_check};
use lb_navgen::mapload::{self, OVERLAYS};
use lb_navgen::patch::{self, Outcome};
use lb_worldq::HullKind;
use rustc_hash::FxHashMap;
use serde::Serialize;

use crate::problems::{self, Problem};

/// Graphs of maps kept at once (each keeps its world for checking changes): a page open on an older graph of a map
/// than the one opened last keeps working on its own.
const KEEP: usize = 3;
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
    /// The key of the server's graph (`Origin::Server`).
    pub base_key: Option<GraphKey>,
    /// Which load of the map this is: a page names it with its changes, since node numbers belong to one graph.
    pub revision: u64,
    world: Mutex<BspWorld>,
    mech: Mechanisms,
    last: Mutex<Option<(String, Arc<Patched>)>>,
    /// How far walking a link falls off a ledge on the way, by where its ends stand (and crouched or not).
    falls: Mutex<FxHashMap<FallKey, Option<f32>>>,
}

type FallKey = ([u32; 3], [u32; 3], bool);

/// Walking a link falls further than this on the way: more than a step down.
const FALL: f32 = 20.0;

/// The graph with the overlays applied, what each of their patches did, and what in it the page flags (but the
/// test runs, read as the files change).
pub struct Patched {
    pub graph: NavGraph,
    pub editor: Vec<Outcome>,
    pub overlay: Vec<Outcome>,
    pub problems: Vec<Problem>,
    /// The graph with the planner's landmarks, made the first time a route asks: previews do without.
    routable: std::sync::OnceLock<NavGraph>,
}

impl Patched {
    pub fn routable(&self) -> &NavGraph {
        self.routable.get_or_init(|| self.graph.clone().with_landmarks())
    }
}

impl NavMap {
    pub fn load(map: &str, bsp: &Path, install: &Path) -> Result<NavMap, String> {
        let bytes = std::fs::read(bsp).map_err(|e| format!("{}: {e}", bsp.display()))?;
        let mut world = BspWorld::load(&bytes).map_err(|e| format!("{}: {e}", bsp.display()))?;
        let mech = Mechanisms::from_world(&world);
        let (base, origin, base_key) = match mapload::kept_graph(install, map, &world) {
            Some((key, g)) => (g, Origin::Server, Some(key)),
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
                (g, Origin::Made, None)
            }
        };
        mapload::prepare_world(&mut world, &mech);
        Ok(NavMap {
            map: map.to_string(),
            bsp_size: bytes.len() as u64,
            base,
            origin,
            base_key,
            revision: 0,
            world: Mutex::new(world),
            mech,
            last: Mutex::new(None),
            falls: Mutex::new(FxHashMap::default()),
        })
    }

    /// The walk and crouch links of `g` that fall further than a step off a ledge on the way, and how far: checked
    /// once a link, as its ends stand.
    fn falls(&self, g: &NavGraph) -> Vec<(NodeId, NodeId, f32)> {
        let bits = |v: lb_core::Vec3| [v.x.to_bits(), v.y.to_bits(), v.z.to_bits()];
        let mut cache = self.falls.lock().expect("no panics while locked");
        let mut world = self.world.lock().expect("no panics while locked");
        let mut out = Vec::new();
        for n in 0..g.len() as NodeId {
            for l in g.links(n) {
                let crouch = l.kind == LinkKind::Crouch;
                if !l.valid() || !(crouch || l.kind == LinkKind::Walk) {
                    continue;
                }
                let (a, b) = (g.node(n), g.node(l.to));
                let fall = *cache
                    .entry((bits(a.origin), bits(b.origin), crouch))
                    .or_insert_with(|| {
                        let (from, to, hull) = if crouch {
                            (
                                lb_nav::classify::crouch_origin(a),
                                lb_nav::classify::crouch_origin(b),
                                HullKind::Crouch,
                            )
                        } else {
                            (a.origin, b.origin, HullKind::Stand)
                        };
                        match walk_check(&mut *world, from, to, hull) {
                            WalkCheck::Drop(h) if h > FALL => Some(h),
                            _ => None,
                        }
                    });
                if let Some(h) = fall {
                    out.push((n, l.to, h));
                }
            }
        }
        out
    }

    /// The player physics changes are checked with: the server's, the graph was made with; else the defaults.
    pub fn physics(&self) -> Physics {
        self.base_key.as_ref().map_or_else(Physics::default, |k| k.movement)
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
        let patches = all_patches(editor, overlay);
        let split = editor.nav.patches.len();
        let (graph, report) = {
            let mut world = self.world.lock().expect("no panics while locked");
            patch::apply(self.base.clone(), &patches, &mut world, &self.mech, self.physics())
        };
        let mut editor_out = report.outcomes;
        let overlay_out = editor_out.split_off(split);
        let mut found = problems::for_changes(&graph, &editor.nav.patches, &editor_out, "editor");
        if let Some(o) = overlay {
            found.extend(problems::for_changes(&graph, &o.nav.patches, &overlay_out, "overlay"));
        }
        found.extend(problems::for_links(&graph, &self.falls(&graph)));
        let p = Arc::new(Patched {
            graph,
            routable: std::sync::OnceLock::new(),
            editor: editor_out,
            overlay: overlay_out,
            problems: found,
        });
        *self.last.lock().expect("no panics while locked") = Some((key, p.clone()));
        p
    }
}

/// The patches of the editor file, then of the hand-written overlay: in the order the server applies them.
fn all_patches(editor: &OverlayFile, overlay: Option<&OverlayFile>) -> Vec<Patch> {
    editor
        .nav
        .patches
        .iter()
        .chain(overlay.iter().flat_map(|o| o.nav.patches.iter()))
        .cloned()
        .collect()
}

/// A map's navigation kept, with the BSP file and its modification time it was loaded from, and the server's graph
/// files of the map then.
type Kept = (PathBuf, Option<std::time::SystemTime>, Vec<String>, Arc<NavMap>);

/// The server's kept graph files of `map`, by name: another set of them may hold another graph of it.
fn graph_files(install: &Path, map: &str) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(install.join("nav").join(map))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.ends_with(".lbnav"))
        .collect();
    names.sort();
    names
}

/// Navigation of the maps opened last.
#[derive(Default)]
pub struct NavMaps {
    kept: Mutex<Vec<Kept>>,
    /// Held while a map loads: making a graph takes a while, and two pages asking at once make one.
    loading: Mutex<()>,
    revisions: std::sync::atomic::AtomicU64,
}

impl NavMaps {
    /// The map's navigation, loaded again when its BSP changed, or (`fresh`: a page opening the map) when the server's
    /// graphs of it did. A page keeps the graph it opened (its nodes' numbers) until it opens the map again.
    pub fn get(&self, map: &str, bsp: &Path, install: &Path, fresh: bool) -> Result<Arc<NavMap>, String> {
        let stamp = std::fs::metadata(bsp).and_then(|m| m.modified()).ok();
        let graphs = fresh.then(|| graph_files(install, map));
        let find = || {
            let mut kept = self.kept.lock().expect("no panics while locked");
            let i = kept
                .iter()
                .position(|(p, s, g, _)| p == bsp && *s == stamp && graphs.as_ref().is_none_or(|now| now == g))?;
            let k = kept.remove(i);
            let nav = k.3.clone();
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
        let files = graphs.clone().unwrap_or_else(|| graph_files(install, map));
        let mut loaded = NavMap::load(map, bsp, install)?;
        loaded.revision = self.revisions.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        let nav = Arc::new(loaded);
        let mut kept = self.kept.lock().expect("no panics while locked");
        kept.insert(0, (bsp.to_path_buf(), stamp, files, nav.clone()));
        kept.truncate(KEEP);
        Ok(nav)
    }

    /// The graph of the map from `bsp` a page opened (`revision`), while it is kept.
    pub fn revision(&self, bsp: &Path, revision: u64) -> Option<Arc<NavMap>> {
        let mut kept = self.kept.lock().expect("no panics while locked");
        let i = kept
            .iter()
            .position(|(p, _, _, n)| p == bsp && n.revision == revision)?;
        let k = kept.remove(i);
        let nav = k.3.clone();
        kept.insert(0, k);
        Some(nav)
    }
}

/// Graph data for the page, flat: `[x, y, z, flags]` a node and `[from, to, kind, flags, centiseconds]` a link,
/// kinds as in `kinds()`.
#[derive(Clone, Debug, Default, Serialize)]
pub struct GraphJson {
    pub nodes: Vec<f32>,
    pub links: Vec<f32>,
}

/// Link kind names by `LinkKind::index`, the kind a link is written with.
pub fn kinds() -> Vec<&'static str> {
    LinkKind::ALL.iter().map(|k| k.as_str()).collect()
}

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

/// What the overlays change in the base graph: the nodes put in (numbered on from the base's), the nodes of the base
/// moved (`[node, x, y, z, flags]` each), links put in or changed, links taken out or changed (`[from, to]` each),
/// and each patch's outcome.
#[derive(Clone, Debug, Serialize)]
pub struct Preview {
    pub nodes: Vec<f32>,
    pub moved: Vec<f32>,
    pub added: Vec<f32>,
    pub removed: Vec<u32>,
    pub editor: Vec<Outcome>,
    pub overlay: Vec<Outcome>,
    /// What the page flags, errors first.
    pub problems: Vec<Problem>,
}

fn same(a: &NavLink, b: &NavLink) -> bool {
    a.to == b.to && a.kind == b.kind && a.flags == b.flags
}

/// What the overlays change in `base`, with what the page flags: `p`'s problems and `tested`, those the test runs
/// found.
pub fn preview(base: &NavGraph, p: &Patched, tested: Vec<Problem>) -> Preview {
    let g = &p.graph;
    let mut found = p.problems.clone();
    found.extend(tested);
    problems::sort(&mut found);
    let mut out = Preview {
        nodes: Vec::new(),
        moved: Vec::new(),
        added: Vec::new(),
        removed: Vec::new(),
        editor: p.editor.clone(),
        overlay: p.overlay.clone(),
        problems: found,
    };
    for n in base.len()..g.len() {
        push_node(&mut out.nodes, g, n);
    }
    for n in 0..base.len() {
        let (was, now) = (&base.nodes[n], &g.nodes[n]);
        if was.origin != now.origin || was.flags != now.flags {
            out.moved.push(n as f32);
            push_node(&mut out.moved, g, n);
        }
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

/// What became of the editor's graph (`mapload::EDITED`) on a save.
#[derive(Clone, Debug, Serialize)]
pub struct EditedGraph {
    /// Written with the overlays now on disk applied; when not, one written before is removed.
    pub written: bool,
    /// Where it is, or why it is not.
    pub detail: String,
}

/// Writes the server's graph with the map's overlays applied as the server reads them (checked with its physics), for
/// the server to play on as it is (`mapload::EDITED`). Only on the server's own graph: otherwise, and without changes,
/// the server applies the overlays itself and one written before is removed.
pub fn save_edited(nav: &NavMap, install: &Path) -> EditedGraph {
    let map = nav.map.as_str();
    let not = |why: &str| EditedGraph {
        written: false,
        detail: if mapload::remove_edited(install, map) {
            format!("{why}; the one saved before is removed")
        } else {
            why.to_string()
        },
    };
    let Some(base) = &nav.base_key else {
        return not(
            "the server has not made a graph of this build of the map with this version of the bots yet (it makes one \
             when it loads the map; open the map here again then): it applies the changes itself",
        );
    };
    let files = mapload::read_overlay_files(install, map, nav.bsp_size);
    let file = |name: &str| files.iter().find(|(n, _)| *n == name).map(|(_, f)| f);
    let empty = OverlayFile::new(map);
    let (editor, overlay) = (file(OVERLAYS[0]).unwrap_or(&empty), file(OVERLAYS[1]));
    let patches = all_patches(editor, overlay);
    if patches.is_empty() {
        return not("no changes: the server plays on its own graph");
    }
    let patched = nav.patched(editor, overlay);
    match mapload::write_edited(install, map, &mapload::edited_key(base, &patches), &patched.graph) {
        Ok(path) => EditedGraph {
            written: true,
            detail: path.display().to_string(),
        },
        Err(e) => not(&e),
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
    fn kind_names_go_by_the_index_links_are_written_with() {
        let names = kinds();
        for k in LinkKind::ALL {
            assert_eq!(names[k.index()], k.as_str());
        }
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
    fn a_save_writes_the_graph_the_server_plays_on_with_the_overlays_saved() {
        let Some(bsp) = stand() else { return };
        let install = std::env::temp_dir().join(format!("lb-editor-edited-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&install);
        let made = NavMap::load("crossfire", &bsp, &install).expect("crossfire loads");
        let g = &made.base;
        let (a, b) = (0 as NodeId, g.links(0)[0].to);
        let unlink = |both: bool| Patch::RemoveLink {
            from: g.node(a).origin.to_array(),
            to: g.node(b).origin.to_array(),
            both,
            note: String::new(),
        };
        let mut f = OverlayFile::new("crossfire");
        f.nav.patches.push(unlink(true));
        let v1 = save_editor(&install, "crossfire", &f, "none").ok().expect("saved");
        let e = save_edited(&made, &install);
        assert!(!e.written && e.detail.contains("has not made"), "{e:?}");

        // The server's graph, kept as the server keeps it.
        let world = BspWorld::load(&std::fs::read(&bsp).unwrap()).unwrap();
        let key = lb_navgen::cache::key(&world, &lb_navgen::GenOptions::default(), 0, 0);
        lb_navgen::cache::GraphCache::new(&install.join("nav"), "crossfire")
            .store(&key, &made.base)
            .unwrap();
        let nav = NavMap::load("crossfire", &bsp, &install).expect("crossfire loads");
        assert_eq!(nav.origin, Origin::Server);
        let e = save_edited(&nav, &install);
        assert!(e.written, "{e:?}");
        // What the server reads: the overlays on disk, and the graph saved with exactly them.
        let on_disk = || -> Vec<Patch> {
            mapload::read_overlays(&install, "crossfire", nav.bsp_size)
                .into_iter()
                .flat_map(|o| o.nav.patches)
                .collect()
        };
        let played = mapload::edited_graph(&install, "crossfire", &key, &on_disk()).expect("goes with the overlays");
        let applied = nav.patched(&f, None);
        assert_eq!(
            (&played.nodes, &played.links),
            (&applied.graph.nodes, &applied.graph.links)
        );
        assert!(played.find_link(a, b).is_none() && played.find_link(b, a).is_none());

        // Changed since (by hand, or from the game): the server applies the overlays itself.
        f.nav.patches[0] = unlink(false);
        let v2 = save_editor(&install, "crossfire", &f, &v1).ok().expect("saved");
        assert!(mapload::edited_graph(&install, "crossfire", &key, &on_disk()).is_none());
        save_editor(&install, "crossfire", &OverlayFile::new("crossfire"), &v2)
            .ok()
            .expect("saved");
        let e = save_edited(&nav, &install);
        assert!(!e.written && e.detail.contains("removed"), "{e:?}");
        assert!(!mapload::overlay_path(&install, "crossfire", mapload::EDITED).exists());
        let _ = std::fs::remove_dir_all(&install);
    }

    #[test]
    fn changes_are_checked_and_saved_with_the_physics_of_the_servers_graph() {
        let Some(bsp) = stand() else { return };
        let install = std::env::temp_dir().join(format!("lb-editor-physics-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&install);
        let mut world = BspWorld::load(&std::fs::read(&bsp).unwrap()).unwrap();
        let mech = Mechanisms::from_world(&world);
        let opts = lb_navgen::GenOptions {
            physics: Physics {
                maxspeed: 300.0,
                ..Physics::default()
            },
            ..lb_navgen::GenOptions::default()
        };
        let made = lb_navgen::generate(&mut world, &mech, &opts, "server").graph;
        let key = lb_navgen::cache::key(&world, &opts, 0, 0);
        lb_navgen::cache::GraphCache::new(&install.join("nav"), "crossfire")
            .store(&key, &made)
            .unwrap();
        let nav = NavMap::load("crossfire", &bsp, &install).expect("crossfire loads");
        assert_eq!((nav.origin, nav.physics().maxspeed), (Origin::Server, 300.0));
        let (a, b) = (0 as NodeId, made.links(0)[0].to);
        let mut f = OverlayFile::new("crossfire");
        f.nav.patches.push(Patch::RemoveLink {
            from: made.node(a).origin.to_array(),
            to: made.node(b).origin.to_array(),
            both: true,
            note: String::new(),
        });
        save_editor(&install, "crossfire", &f, "none").ok().expect("saved");
        let e = save_edited(&nav, &install);
        assert!(e.written, "{e:?}");
        // The server with that physics plays on it.
        assert!(mapload::edited_graph(&install, "crossfire", &key, &f.nav.patches).is_some());
        let _ = std::fs::remove_dir_all(&install);
    }

    #[test]
    fn opening_the_map_again_takes_the_graph_the_server_made_since() {
        let Some(bsp) = stand() else { return };
        let install = std::env::temp_dir().join(format!("lb-editor-fresh-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&install);
        let maps = NavMaps::default();
        let made = maps.get("crossfire", &bsp, &install, true).expect("crossfire loads");
        assert_eq!(made.origin, Origin::Made);
        let world = BspWorld::load(&std::fs::read(&bsp).unwrap()).unwrap();
        let key = lb_navgen::cache::key(&world, &lb_navgen::GenOptions::default(), 0, 0);
        lb_navgen::cache::GraphCache::new(&install.join("nav"), "crossfire")
            .store(&key, &made.base)
            .unwrap();
        let same = maps.get("crossfire", &bsp, &install, false).unwrap();
        assert!(
            Arc::ptr_eq(&made, &same),
            "a page's graph stays until the map is opened again"
        );
        let server = maps.get("crossfire", &bsp, &install, true).unwrap();
        assert_eq!(server.origin, Origin::Server);
        assert_ne!(server.revision, made.revision);
        // A page still on the graph it opened keeps it.
        let old = maps.revision(&bsp, made.revision).expect("kept");
        assert!(Arc::ptr_eq(&old, &made));
        assert!(maps.revision(&bsp, 999).is_none());
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
        let diff = preview(&nav.base, &p, Vec::new());
        assert_eq!(diff.removed, vec![a, b]);
        assert!(diff.added.is_empty() && diff.nodes.is_empty() && diff.moved.is_empty());
        assert!(
            Arc::ptr_eq(&p, &nav.patched(&editor, None)),
            "the same changes are applied once"
        );
        let (c, l) = (0..g.len() as NodeId)
            .find_map(|n| {
                g.links(n)
                    .iter()
                    .find(|l| l.kind == LinkKind::Walk && l.valid() && l.length > 150.0)
                    .map(|l| (n, *l))
            })
            .expect("a long walk link");
        let step = g.node(c).origin + (g.node(l.to).origin - g.node(c).origin).normalize() * 32.0;
        let mut moving = editor.clone();
        moving.nav.patches.push(Patch::MoveNode {
            from: g.node(c).origin.to_array(),
            to: step.to_array(),
            note: String::new(),
        });
        let moved = preview(&nav.base, &nav.patched(&moving, None), Vec::new());
        assert!(moved.editor[1].ok, "{:?}", moved.editor[1]);
        assert_eq!(
            (moved.moved.len(), moved.moved[0]),
            (5, c as f32),
            "the node moved, where it is now"
        );
        assert!(moved.nodes.is_empty(), "no node put in");
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
        let r = route(p.routable(), g.node(a).origin, g.node(far).origin, false, false).expect("a way across the map");
        assert_eq!((r.start, *r.nodes.last().unwrap()), (a, far));
        assert!(r.time > 0.0 && r.legs.len() + 1 == r.nodes.len());
        assert!(r.legs.iter().all(|l| l.kind != "longjump" && l.kind != "gauss_boost"));
        let _ = std::fs::remove_dir_all(&install);
    }
}
