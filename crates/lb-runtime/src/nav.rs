//! Navigation in the runtime: loading the map (visibility sets, mechanisms, navigation graph) off the main thread,
//! live engine traces, the live state of the map's mechanisms, and each bot's navigation service.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use lb_bsp::mech::{Mechanisms, MoverKind};
use lb_config::main_config::NavSource;
use lb_config::overlay::{OverlayFile, Patch};
use lb_core::Vec3;
use lb_core::rng::Pcg32;
use lb_game::items::ItemKind;
use lb_host::strings::StringTable;
use lb_host::{EntitySnapshot, Host, TraceKind, TraceRequest};
use lb_knowledge::ItemSpot;
use lb_nav::NavGraph;
use lb_nav::exec::{HitKind, MechView, MoverState, NavInput};
use lb_nav::import::ImportOptions;
use lb_nav::known::LinkHealth;
use lb_nav::navigator::{NavCtx, Navigator};
use lb_nav_api::{NavService, NavStatus, NavStep};
use lb_navgen::GenOptions;
use lb_navgen::cache::GraphCache;
use lb_worldq::{HullKind, Trace, TraceQuery, Tracer};
use rustc_hash::FxHashMap;

/// What the runtime keeps of a map's mechanisms: where each moving brush was compiled and what it is.
#[derive(Clone, Debug, Default)]
pub struct MapMechs {
    /// Brush model → (spawn origin of its entity, is a mover, is a breakable).
    pub models: FxHashMap<u16, (Vec3, bool, bool)>,
    pub movers: usize,
    pub breakables: usize,
}

impl MapMechs {
    fn from(world: &lb_bsp::BspWorld, mech: &Mechanisms) -> MapMechs {
        let mut m = MapMechs::default();
        for mv in &mech.movers {
            let origin = world.entities.get(mv.entity).map_or(Vec3::ZERO, |e| e.origin());
            let moving = mv.kind != MoverKind::Other || mv.speed > 0.0;
            m.models.insert(mv.model as u16, (origin, moving, false));
            m.movers += 1;
        }
        for b in &mech.breakables {
            let origin = world.entities.get(b.entity).map_or(Vec3::ZERO, |e| e.origin());
            m.models.insert(b.model as u16, (origin, false, true));
            m.breakables += 1;
        }
        m
    }
}

pub struct LoadedMap {
    /// PVS and PAS of the map, for perception.
    pub vis: Arc<lb_bsp::MapVis>,
    /// Items the map places (static knowledge every bot has).
    pub items: Arc<Vec<ItemSpot>>,
    /// Where players spawn.
    pub spawns: Arc<Vec<Vec3>>,
    /// Wall chargers and where to stand to use them.
    pub chargers: Arc<Vec<lb_knowledge::ChargerSpot>>,
    pub mechs: Arc<MapMechs>,
    pub graph: Result<Arc<NavGraph>, String>,
    /// Where the graph came from: "cache", "generated" or "yapb".
    pub origin: &'static str,
    /// The map's overlays (editor's, then the hand-written one) and what applying their patches came to.
    pub overlays: Arc<Vec<OverlayFile>>,
    pub patches: String,
    pub millis: u128,
}

/// How a map's graph is had.
#[derive(Clone, Debug)]
pub struct LoadOptions {
    pub source: NavSource,
    /// Checking an imported graph (and the physics a generated one is made with).
    pub import: ImportOptions,
    /// Threads making a graph; negative = all cores but that many.
    pub threads: i32,
}

/// Loads `maps/<map>.bsp` and its graph on a worker thread: a generated graph (from the cache when it was made
/// before), or the map's yapb graph.
pub struct NavLoader {
    rx: Receiver<Result<LoadedMap, String>>,
    pub map: String,
}

impl NavLoader {
    pub fn start(game_dir: &Path, install_dir: &Path, map: &str, opts: LoadOptions) -> NavLoader {
        let (tx, rx) = channel();
        let (game, install, name) = (game_dir.to_path_buf(), install_dir.to_path_buf(), map.to_string());
        let spawned = std::thread::Builder::new().name("lb-nav-load".into()).spawn(move || {
            let _ = tx.send(load(&game, &install, &name, &opts));
        });
        if let Err(e) = spawned {
            tracing::error!("cannot start the navigation loader: {e}");
        }
        NavLoader {
            rx,
            map: map.to_string(),
        }
    }

    /// `Some` once the worker is done; `Err` when even the BSP could not be read.
    pub fn poll(&self) -> Option<Result<LoadedMap, String>> {
        match self.rx.try_recv() {
            Ok(r) => Some(r),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err("navigation loader stopped".into())),
        }
    }

    /// Blocks until the worker is done (a replay applies the result at the frame it was applied when recorded).
    pub fn wait(&self) -> Result<LoadedMap, String> {
        self.rx
            .recv()
            .unwrap_or_else(|_| Err("navigation loader stopped".into()))
    }
}

/// What a load came to, without timings or paths: a replay checks that it loaded the same.
pub fn summary(result: &Result<LoadedMap, String>) -> String {
    match result {
        Ok(m) => format!(
            "{} leaves, {} items, {} movers, {} breakables, {}",
            m.vis.visleafs(),
            m.items.len(),
            m.mechs.movers,
            m.mechs.breakables,
            match &m.graph {
                Ok(g) => format!(
                    "{} nodes, {} links ({} invalid, {} added, {})",
                    g.stats.nodes,
                    g.stats.links,
                    g.stats.invalid,
                    g.stats.added,
                    g.stats.kinds()
                ),
                Err(_) => "no graph".into(),
            }
        ),
        Err(_) => "not loaded".into(),
    }
}

/// The files a map's navigation is loaded from.
pub struct MapFiles {
    pub bsp: Option<PathBuf>,
    pub graph: Option<PathBuf>,
    /// Where the files were looked for, for the message when they are missing.
    pub graph_paths: [PathBuf; 2],
}

pub fn map_files(game: &Path, install: &Path, map: &str) -> MapFiles {
    let bsp_paths = [
        game.join("maps").join(format!("{map}.bsp")),
        game.with_file_name(format!(
            "{}_downloads/maps/{map}.bsp",
            game.file_name().and_then(|n| n.to_str()).unwrap_or("valve")
        )),
    ];
    let graph_paths = [
        install.join("nav").join(format!("{map}.graph")),
        game.join("addons/yapb/data/graph").join(format!("{map}.graph")),
    ];
    MapFiles {
        bsp: first_existing(&bsp_paths).cloned(),
        graph: first_existing(&graph_paths).cloned(),
        graph_paths,
    }
}

fn first_existing(paths: &[PathBuf]) -> Option<&PathBuf> {
    paths.iter().find(|p| p.is_file())
}

fn load(game: &Path, install: &Path, map: &str, opts: &LoadOptions) -> Result<LoadedMap, String> {
    let started = std::time::Instant::now();
    let files = map_files(game, install, map);
    let bsp_path = files
        .bsp
        .as_ref()
        .ok_or_else(|| format!("{map}.bsp not found in {}", game.display()))?;
    let bytes = std::fs::read(bsp_path).map_err(|e| format!("{}: {e}", bsp_path.display()))?;
    let mut world = lb_bsp::BspWorld::load(&bytes).map_err(|e| format!("{}: {e}", bsp_path.display()))?;
    let vis = Arc::new(lb_bsp::MapVis::build(&world.bsp));
    let items = world
        .entities
        .iter()
        .filter_map(|e| {
            ItemKind::from_classname(e.classname()).map(|kind| ItemSpot {
                kind,
                origin: e.origin(),
            })
        })
        .collect();
    let spawns: Vec<Vec3> = world
        .entities
        .iter()
        .filter(|e| matches!(e.classname(), "info_player_deathmatch" | "info_player_start"))
        .map(|e| e.origin())
        .collect();
    let chargers: Vec<lb_knowledge::ChargerSpot> = lb_navgen::site::chargers(&world)
        .into_iter()
        .map(|c| lb_knowledge::ChargerSpot {
            suit: c.suit,
            model: c.model as u16,
            center: (c.mins + c.maxs) * 0.5,
            spot: c.spot,
        })
        .collect();
    let mech = Mechanisms::from_world(&world);
    let mechs = Arc::new(MapMechs::from(&world, &mech));
    let overlays = read_overlays(install, map, world.bsp.fingerprint.1);
    let (graph, origin) = match opts.source {
        NavSource::Yapb => (load_graph(&files, map, &mut world, &mech, &opts.import), "yapb"),
        NavSource::Generated => match generated_graph(install, map, &mut world, &mech, opts) {
            Ok((graph, origin)) => (Ok(graph), origin),
            Err(e) => {
                tracing::warn!("{map}: {e}; trying the yapb graph");
                (load_graph(&files, map, &mut world, &mech, &opts.import), "yapb")
            }
        },
    };
    let patches: Vec<Patch> = overlays.iter().flat_map(|o| o.nav.patches.iter().cloned()).collect();
    let (graph, patches) = match graph {
        Ok(g) if !patches.is_empty() => {
            let base = Arc::try_unwrap(g).unwrap_or_else(|g| (*g).clone());
            let (patched, report) = lb_navgen::patch::apply(base, &patches, &mut world, &mech, opts.import.physics);
            for p in &report.problems {
                tracing::warn!("{map} overlay: {p}");
            }
            let summary = format!("{} of {} overlay patches applied", report.applied, patches.len());
            (Ok(Arc::new(patched)), summary)
        }
        other => (other, String::new()),
    };
    let graph = graph.map(|g| Arc::new(Arc::try_unwrap(g).unwrap_or_else(|g| (*g).clone()).with_landmarks()));
    Ok(LoadedMap {
        vis,
        items: Arc::new(items),
        spawns: Arc::new(spawns),
        chargers: Arc::new(chargers),
        mechs,
        graph,
        origin,
        overlays: Arc::new(overlays),
        patches,
        millis: started.elapsed().as_millis(),
    })
}

/// A map's overlay files under `maps/<map>/`, in the order they apply: the in-game editor's, then the hand-written one.
pub const OVERLAYS: [&str; 2] = ["editor.yaml", "overlay.yaml"];

/// The map's overlays (`OVERLAYS`) in the order they apply. A file that does not read is left out with a warning.
pub fn read_overlays(install: &Path, map: &str, bsp_size: u64) -> Vec<OverlayFile> {
    let dir = install.join("maps").join(map);
    OVERLAYS
        .iter()
        .filter_map(|name| {
            let path = dir.join(name);
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

/// Generator settings for the server's physics.
pub fn gen_options(opts: &LoadOptions) -> GenOptions {
    GenOptions {
        physics: opts.import.physics,
        ..GenOptions::default()
    }
}

/// The map's graph from the cache in `nav/<map>/`, or made now on a pool of its own and kept there.
fn generated_graph(
    install: &Path,
    map: &str,
    world: &mut lb_bsp::BspWorld,
    mech: &Mechanisms,
    opts: &LoadOptions,
) -> Result<(Arc<NavGraph>, &'static str), String> {
    let gen_opts = gen_options(opts);
    let cache = GraphCache::new(&install.join("nav"), map);
    let key = lb_navgen::cache::key(world, &gen_opts, 0, 0);
    if let Some(graph) = cache.load(&key) {
        return Ok((Arc::new(graph), "cache"));
    }
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get()) as i32;
    let threads = if opts.threads > 0 {
        opts.threads
    } else {
        cores + opts.threads
    }
    .max(1);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads as usize)
        .thread_name(|i| format!("lb-navgen-{i}"))
        .build()
        .map_err(|e| format!("cannot start the generator's threads: {e}"))?;
    let generated = pool.install(|| lb_navgen::generate(world, mech, &gen_opts, "generated"));
    if generated.graph.is_empty() {
        return Err("the generator found no floor".into());
    }
    let report = lb_navgen::report::coverage(&generated);
    tracing::info!(
        "{map}: graph made in {} ms on {threads} threads: {} nodes, {} links ({}); {:.1}% of the floor covered, \
         {}/{} items",
        generated.graph.stats.millis,
        generated.graph.stats.nodes,
        generated.graph.stats.links,
        generated.graph.stats.kinds(),
        report.ratio() * 100.0,
        report.items_ok,
        report.items
    );
    match cache.store(&key, &generated.graph) {
        Ok(path) => tracing::debug!("{map}: graph kept in {}", path.display()),
        Err(e) => tracing::warn!("{map}: the graph is not kept: {e}"),
    }
    Ok((Arc::new(generated.graph), "generated"))
}

fn load_graph(
    files: &MapFiles,
    map: &str,
    world: &mut lb_bsp::BspWorld,
    mech: &Mechanisms,
    opts: &ImportOptions,
) -> Result<Arc<NavGraph>, String> {
    let graph_path = files.graph.as_ref().ok_or_else(|| {
        format!(
            "no graph for {map}: looked for {} and {}",
            files.graph_paths[0].display(),
            files.graph_paths[1].display()
        )
    })?;
    let yapb_bytes = std::fs::read(graph_path).map_err(|e| format!("{}: {e}", graph_path.display()))?;
    let yapb = lb_nav::yapb::parse(&yapb_bytes).map_err(|e| format!("{}: {e}", graph_path.display()))?;
    if let Some(size) = yapb.map_size
        && size as u64 != world.bsp.fingerprint.1
    {
        tracing::warn!(
            "{} was made for a {size}-byte {map}.bsp, this one has {} bytes",
            graph_path.display(),
            world.bsp.fingerprint.1
        );
    }
    let graph = lb_nav::import::import_yapb(&yapb, world, mech, opts, &graph_path.display().to_string());
    Ok(Arc::new(graph))
}

/// Engine traces through the host (main thread only).
pub struct LiveTracer<'a> {
    pub host: &'a mut dyn Host,
    pub count: u32,
}

impl Tracer for LiveTracer<'_> {
    fn trace(&mut self, q: &TraceQuery) -> Trace {
        self.count += 1;
        let kind = if q.hull == HullKind::Point {
            TraceKind::Line
        } else {
            TraceKind::Hull(q.hull as u8)
        };
        let tr = self.host.trace(&TraceRequest {
            start: q.start,
            end: q.end,
            kind,
            ignore_monsters: q.ignore_monsters,
            ignore_glass: q.ignore_glass,
            ignore: q.ignore.map(|index| lb_ffi::LbEntRef {
                index,
                pad: 0,
                serial: 0,
            }),
        });
        let touched = tr.fraction < 1.0 || tr.start_solid;
        Trace {
            all_solid: tr.all_solid,
            start_solid: tr.start_solid,
            in_open: tr.in_open,
            in_water: tr.in_water,
            fraction: tr.fraction,
            end: tr.end_pos,
            normal: tr.plane_normal,
            dist: tr.plane_dist,
            hit: touched.then_some(u32::from(tr.hit.index)),
        }
    }

    fn point_contents(&mut self, p: Vec3) -> i32 {
        self.host.point_contents(p)
    }
}

/// The live state of the map's mechanisms, refreshed every frame from entity snapshots. Executors read it to
/// operate doors and lifts; planning never does.
#[derive(Default)]
pub struct LiveMechs {
    map: Option<Arc<MapMechs>>,
    /// Brush model → (state, still exists).
    models: FxHashMap<u16, MoverState>,
    /// Entity index → brush model, for what traces hit.
    by_index: FxHashMap<u16, u16>,
    /// Interned model name → brush model number.
    model_ids: FxHashMap<u16, Option<u16>>,
    pub max_clients: u32,
    snapshots: Vec<EntitySnapshot>,
    /// Sim time of the last refresh.
    refreshed_at: Option<f64>,
}

/// Seconds between refreshes: a lift at 200 units/s moves 2 units in that time.
const MECHS_PERIOD: f64 = 0.01;

impl LiveMechs {
    pub fn set_map(&mut self, map: Option<Arc<MapMechs>>, max_clients: u32) {
        self.map = map;
        self.max_clients = max_clients;
        self.models.clear();
        self.by_index.clear();
        self.model_ids.clear();
        self.refreshed_at = None;
    }

    pub fn is_loaded(&self) -> bool {
        self.map.is_some()
    }

    /// Reads where every mover and breakable is now, a hundred times a second.
    pub fn refresh(&mut self, host: &mut dyn Host, strings: &StringTable, now: f64) {
        let Some(map) = self.map.clone() else { return };
        if self.refreshed_at.is_some_and(|t| now >= t && now - t < MECHS_PERIOD) {
            return;
        }
        self.refreshed_at = Some(now);
        use lb_game::entities::{KIND_BREAKABLE, KIND_BUTTON, KIND_MOVER, kind_mask};
        self.snapshots.clear();
        host.snapshot_entities(
            kind_mask(&[KIND_MOVER, KIND_BUTTON, KIND_BREAKABLE]),
            &mut self.snapshots,
        );
        self.models.clear();
        self.by_index.clear();
        const EF_NODRAW: u32 = 128;
        for s in &self.snapshots {
            // Model names arrive through the string table a frame after first use: only known names are cached.
            let model = match self.model_ids.get(&s.model_id) {
                Some(m) => *m,
                None => match strings.string(s.model_id) {
                    Some(name) => {
                        let m = std::str::from_utf8(name)
                            .ok()
                            .and_then(|n| n.strip_prefix('*'))
                            .and_then(|n| n.parse().ok());
                        self.model_ids.insert(s.model_id, m);
                        m
                    }
                    None => None,
                },
            };
            let Some(model) = model else { continue };
            self.by_index.insert(s.ent.index, model);
            let Some(&(origin, _, breakable)) = map.models.get(&model) else {
                continue;
            };
            if breakable && (s.solid == 0 || s.effects & EF_NODRAW != 0) {
                continue;
            }
            let at = Vec3::new(s.origin.x, s.origin.y, s.origin.z);
            let vel = Vec3::new(s.velocity.x, s.velocity.y, s.velocity.z);
            self.models.insert(
                model,
                MoverState {
                    offset: at - origin,
                    velocity: vel,
                },
            );
        }
    }

    /// Brush model of the entity at `index` (for `groundentity`), 0 for the world or anything else.
    pub fn model_of(&self, index: u16) -> u16 {
        self.by_index.get(&index).copied().unwrap_or(0)
    }
}

impl MechView for LiveMechs {
    fn mover(&self, model: u16) -> Option<MoverState> {
        self.models.get(&model).copied()
    }

    fn exists(&self, model: u16) -> bool {
        self.models.contains_key(&model)
    }

    fn hit_kind(&self, hit: u32) -> HitKind {
        if hit == 0 {
            return HitKind::World;
        }
        if hit <= self.max_clients {
            return HitKind::Player;
        }
        let model = self.model_of(hit as u16);
        match self.map.as_ref().and_then(|m| m.models.get(&model)) {
            Some((_, true, _)) => HitKind::Mover(model),
            _ => HitKind::Other,
        }
    }
}

/// Navigation as one bot's behavior sees it for one frame: its navigator, the graph, live traces and mechanisms.
pub struct BotNavService<'a, 'h> {
    pub nav: &'a mut Navigator,
    pub graph: Option<&'a NavGraph>,
    pub tracer: &'a mut LiveTracer<'h>,
    pub mechs: &'a LiveMechs,
    pub health: &'a mut LinkHealth,
    pub bot: u32,
    pub input: NavInput,
    pub stuck_kill: f64,
    /// No enemy in sight or heard for a while: a stuck bot may give up its life.
    pub calm: bool,
    /// Stuck beyond recovery: the bot should `kill` itself.
    pub kill: bool,
    /// Path search expansions left this frame for all bots.
    pub plan_budget: &'a mut u32,
}

impl BotNavService<'_, '_> {
    /// No progress for `stuck_kill` seconds of navigating (waiting for a lift or a door does not count), with
    /// no enemy around: give up on this life.
    fn stuck(&mut self) -> bool {
        let stuck = self.nav.stuck_for(self.input.origin, self.input.now);
        if stuck > self.stuck_kill && self.calm && self.nav.failures_total > 0 {
            self.kill = true;
            self.nav.reset();
            return true;
        }
        false
    }
}

impl Tracer for BotNavService<'_, '_> {
    fn trace(&mut self, q: &TraceQuery) -> Trace {
        self.tracer.trace(q)
    }

    fn point_contents(&mut self, p: Vec3) -> i32 {
        self.tracer.point_contents(p)
    }
}

impl NavService for BotNavService<'_, '_> {
    fn go_to(&mut self, dest: Vec3) -> (NavStatus, Option<NavStep>) {
        let Some(graph) = self.graph else {
            return (NavStatus::NoPath, None);
        };
        if self.stuck() {
            return (NavStatus::NoPath, None);
        }
        let mut ctx = NavCtx {
            graph,
            tracer: &mut *self.tracer,
            mech: self.mechs,
            health: Some(&mut *self.health),
            bot: self.bot,
            budget: Some(&mut *self.plan_budget),
        };
        self.nav.go_to(&mut ctx, &self.input, dest)
    }

    fn roam(&mut self, rng: &mut Pcg32) -> Option<NavStep> {
        let graph = self.graph?;
        if self.stuck() {
            return None;
        }
        let mut ctx = NavCtx {
            graph,
            tracer: &mut *self.tracer,
            mech: self.mechs,
            health: Some(&mut *self.health),
            bot: self.bot,
            budget: Some(&mut *self.plan_budget),
        };
        self.nav.roam(&mut ctx, &self.input, rng)
    }

    fn away_from(&mut self, threat: Vec3) -> Option<Vec3> {
        let graph = self.graph?;
        let ctx = NavCtx {
            graph,
            tracer: &mut *self.tracer,
            mech: self.mechs,
            health: None,
            bot: self.bot,
            budget: None,
        };
        Navigator::away_from(&ctx, self.input.origin, threat)
    }

    fn available(&self) -> bool {
        self.graph.is_some()
    }
}
