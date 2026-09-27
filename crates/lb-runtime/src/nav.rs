//! Navigation in the runtime: loading the map (visibility sets and navigation graph) off the main thread, live
//! engine traces, and the per-bot roaming state (goal, path follower, penalties for links that got the bot stuck).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use lb_core::Vec3;
use lb_core::rng::Pcg32;
use lb_game::items::ItemKind;
use lb_host::{Host, TraceKind, TraceRequest};
use lb_knowledge::ItemSpot;
use lb_nav::follow::{EYE_HEIGHT, FollowInput, FollowOutput, FollowStatus, PathFollower};
use lb_nav::{NavGraph, NodeFlags, NodeId};
use lb_nav_api::{NavService, NavStatus, NavStep};
use lb_worldq::{HullKind, Trace, TraceQuery, Tracer};

pub struct LoadedMap {
    /// PVS and PAS of the map, for perception.
    pub vis: Arc<lb_bsp::MapVis>,
    /// Items the map places (static knowledge every bot has).
    pub items: Arc<Vec<ItemSpot>>,
    pub graph: Result<Arc<NavGraph>, String>,
    pub millis: u128,
}

/// Loads `maps/<map>.bsp` and a yapb graph for it on a worker thread.
pub struct NavLoader {
    rx: Receiver<Result<LoadedMap, String>>,
    pub map: String,
}

impl NavLoader {
    pub fn start(game_dir: &Path, install_dir: &Path, map: &str) -> NavLoader {
        let (tx, rx) = channel();
        let (game, install, name) = (game_dir.to_path_buf(), install_dir.to_path_buf(), map.to_string());
        let spawned = std::thread::Builder::new().name("lb-nav-load".into()).spawn(move || {
            let _ = tx.send(load(&game, &install, &name));
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
}

fn first_existing(paths: &[PathBuf]) -> Option<&PathBuf> {
    paths.iter().find(|p| p.is_file())
}

fn load(game: &Path, install: &Path, map: &str) -> Result<LoadedMap, String> {
    let started = std::time::Instant::now();
    let bsp_paths = [
        game.join("maps").join(format!("{map}.bsp")),
        game.with_file_name(format!(
            "{}_downloads/maps/{map}.bsp",
            game.file_name().and_then(|n| n.to_str()).unwrap_or("valve")
        )),
    ];
    let bsp_path = first_existing(&bsp_paths).ok_or_else(|| format!("{map}.bsp not found in {}", game.display()))?;
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
    let graph = load_graph(install, game, map, &mut world);
    Ok(LoadedMap {
        vis,
        items: Arc::new(items),
        graph,
        millis: started.elapsed().as_millis(),
    })
}

fn load_graph(install: &Path, game: &Path, map: &str, world: &mut lb_bsp::BspWorld) -> Result<Arc<NavGraph>, String> {
    let graph_paths = [
        install.join("nav").join(format!("{map}.graph")),
        game.join("addons/yapb/data/graph").join(format!("{map}.graph")),
    ];
    let graph_path = first_existing(&graph_paths).ok_or_else(|| {
        format!(
            "no graph for {map}: looked for {} and {}",
            graph_paths[0].display(),
            graph_paths[1].display()
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
    let graph = lb_nav::import::import_yapb(&yapb, world, false, &graph_path.display().to_string());
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
        Trace {
            all_solid: tr.all_solid,
            start_solid: tr.start_solid,
            in_open: tr.in_open,
            in_water: tr.in_water,
            fraction: tr.fraction,
            end: tr.end_pos,
            normal: tr.plane_normal,
            dist: tr.plane_dist,
            hit: (tr.hit.index != 0).then_some(u32::from(tr.hit.index)),
        }
    }

    fn point_contents(&mut self, p: Vec3) -> i32 {
        self.host.point_contents(p)
    }
}

/// A link a bot got stuck on, avoided when planning until `until`.
#[derive(Clone, Copy, Debug)]
struct Avoid {
    from: NodeId,
    to: NodeId,
    until: f64,
}

/// Navigation calls this far apart mean the bot did something else meanwhile (fought): the stuck clock restarts.
const NAV_GAP: f64 = 0.5;
const REPLAN_EVERY: f64 = 0.5;
const GIVE_UP_AFTER: u32 = 3;
const DIRECT_WALK: f32 = 200.0;
const DIRECT_RECHECK: f64 = 0.1;

#[derive(Default)]
pub struct BotNav {
    pub follower: Option<PathFollower>,
    pub goal: Option<NodeId>,
    next_goal_at: f64,
    next_plan_at: f64,
    avoid: Vec<Avoid>,
    failures: u32,
    /// Where the bot was when it last moved noticeably, for the irrecoverably-stuck check.
    anchor: Vec3,
    anchor_at: f64,
    last_used: f64,
    /// A destination walked to straight, whether the way was clear, and when that was checked.
    direct: Option<(Vec3, bool, f64)>,
    /// The graph node nearest to the last destination.
    dest_node: Option<(Vec3, NodeId)>,
}

pub struct NavContext<'a> {
    pub graph: &'a NavGraph,
    pub tracer: &'a mut dyn Tracer,
}

impl BotNav {
    pub fn reset(&mut self) {
        *self = BotNav::default();
    }

    /// Seconds of navigating without moving 48 u away from where the bot last was.
    pub fn stuck_for(&mut self, origin: Vec3, now: f64) -> f64 {
        if self.anchor_at == 0.0 || origin.distance(self.anchor) > 48.0 || now - self.last_used > NAV_GAP {
            self.anchor = origin;
            self.anchor_at = now;
        }
        self.last_used = now;
        now - self.anchor_at
    }

    /// Nearest node the bot can reach in a straight line.
    fn start_node(ctx: &mut NavContext<'_>, origin: Vec3) -> Option<NodeId> {
        for (id, _) in ctx.graph.nearest(origin, 512.0, 6) {
            let node = ctx.graph.node(id);
            let tr = ctx
                .tracer
                .trace(&TraceQuery::hull(origin, node.origin, HullKind::Crouch));
            if !tr.start_solid && tr.fraction > 0.97 {
                return Some(id);
            }
        }
        ctx.graph.nearest(origin, 256.0, 1).first().map(|(id, _)| *id)
    }

    fn pick_goal(ctx: &mut NavContext<'_>, origin: Vec3, rng: &mut Pcg32) -> Option<NodeId> {
        let g = ctx.graph;
        let goals: Vec<NodeId> = (0..g.len() as NodeId)
            .filter(|&id| {
                g.node(id)
                    .flags
                    .intersects(NodeFlags::GOAL | NodeFlags::CAMP | NodeFlags::SNIPER)
            })
            .collect();
        for _ in 0..8 {
            let id = if !goals.is_empty() && rng.chance(60.0) {
                goals[rng.range_i32(0, goals.len() as i32 - 1) as usize]
            } else {
                rng.range_i32(0, g.len() as i32 - 1) as NodeId
            };
            let node = g.node(id);
            if node.origin.distance(origin) > 400.0 && !node.flags.intersects(NodeFlags::LADDER | NodeFlags::AIRBORNE) {
                return Some(id);
            }
        }
        None
    }

    fn plan(&mut self, ctx: &mut NavContext<'_>, origin: Vec3, goal: NodeId, now: f64) -> bool {
        let Some(start) = Self::start_node(ctx, origin) else {
            return false;
        };
        self.avoid.retain(|a| a.until > now);
        let avoid = &self.avoid;
        let penalty = |a: NodeId, b: NodeId| {
            if avoid.iter().any(|x| x.from == a && x.to == b) {
                f32::INFINITY
            } else {
                0.0
            }
        };
        match lb_nav::plan::plan(ctx.graph, start, goal, &penalty) {
            Some(path) => {
                self.follower = Some(PathFollower::new(path, now));
                self.goal = Some(goal);
                true
            }
            None => false,
        }
    }

    /// Walks toward `dest`: a path to the graph node nearest to it, the last stretch straight when it is clear.
    pub fn go_to(
        &mut self,
        ctx: &mut NavContext<'_>,
        input: &FollowInput,
        dest: Vec3,
    ) -> (NavStatus, Option<FollowOutput>) {
        let now = input.now;
        let to = dest - input.origin;
        let flat = to.truncate().length();
        if flat < 32.0 && to.z.abs() < 48.0 {
            self.follower = None;
            return (NavStatus::Arrived, None);
        }
        if flat < DIRECT_WALK && to.z.abs() < 32.0 {
            let fresh = self
                .direct
                .filter(|(d, _, at)| d.distance(dest) < 16.0 && now - at < DIRECT_RECHECK);
            let clear = match fresh {
                Some((_, clear, _)) => clear,
                None => {
                    let level = Vec3::new(dest.x, dest.y, input.origin.z);
                    let tr = ctx
                        .tracer
                        .trace(&TraceQuery::hull(input.origin, level, HullKind::Stand));
                    let clear = !tr.start_solid && tr.fraction >= 1.0;
                    self.direct = Some((dest, clear, now));
                    clear
                }
            };
            if clear {
                self.follower = None;
                return (NavStatus::Moving, Some(straight(input, dest)));
            }
        }
        let cached = self.dest_node.filter(|(d, _)| d.distance(dest) < 32.0).map(|(_, n)| n);
        let Some(goal) = cached.or_else(|| ctx.graph.nearest(dest, 400.0, 1).first().map(|(n, _)| *n)) else {
            return (NavStatus::NoPath, None);
        };
        if cached.is_none() {
            self.dest_node = Some((dest, goal));
        }
        if (self.follower.is_none() || self.goal != Some(goal)) && now >= self.next_plan_at {
            self.next_plan_at = now + REPLAN_EVERY;
            if !self.plan(ctx, input.origin, goal, now) {
                self.failures += 1;
                let status = if self.failures >= GIVE_UP_AFTER {
                    self.failures = 0;
                    NavStatus::NoPath
                } else {
                    NavStatus::Moving
                };
                return (status, None);
            }
        }
        let Some(follower) = self.follower.as_mut() else {
            return (NavStatus::Moving, None);
        };
        let out = follower.tick(ctx.graph, input);
        match out.status {
            FollowStatus::Moving => (NavStatus::Moving, Some(out)),
            FollowStatus::Arrived => {
                // At the node closest to the destination: as close as the graph gets.
                self.follower = None;
                self.failures = 0;
                (NavStatus::Arrived, Some(out))
            }
            FollowStatus::Stuck { from, to } => {
                self.avoid.push(Avoid {
                    from,
                    to,
                    until: now + 10.0,
                });
                self.follower = None;
                self.failures += 1;
                if self.failures >= GIVE_UP_AFTER {
                    self.failures = 0;
                    return (NavStatus::NoPath, Some(out));
                }
                (NavStatus::Moving, Some(out))
            }
        }
    }

    /// A node to fall back to: well away from `threat`, not too far from the bot.
    pub fn away_from(ctx: &NavContext<'_>, origin: Vec3, threat: Vec3) -> Option<Vec3> {
        let here = origin.distance(threat);
        ctx.graph
            .nearest(origin, 1200.0, 64)
            .into_iter()
            .map(|(id, _)| ctx.graph.node(id))
            .filter(|n| !n.flags.intersects(NodeFlags::LADDER | NodeFlags::AIRBORNE))
            .map(|n| n.origin)
            .filter(|p| p.distance(threat) > here + 200.0)
            .max_by(|a, b| {
                let score = |p: &Vec3| p.distance(threat) - 0.5 * p.distance(origin);
                score(a).total_cmp(&score(b))
            })
    }

    /// Roaming: walk to a goal, pause, pick the next one. Returns what to do this frame.
    pub fn tick(&mut self, ctx: &mut NavContext<'_>, input: &FollowInput, rng: &mut Pcg32) -> Option<FollowOutput> {
        let now = input.now;
        if self.follower.is_none() {
            if now < self.next_goal_at {
                return None;
            }
            self.next_goal_at = now + 1.0;
            let goal = Self::pick_goal(ctx, input.origin, rng)?;
            if !self.plan(ctx, input.origin, goal, now) {
                return None;
            }
        }
        let follower = self.follower.as_mut()?;
        let out = follower.tick(ctx.graph, input);
        match out.status {
            FollowStatus::Moving => {}
            FollowStatus::Arrived => {
                self.follower = None;
                self.failures = 0;
                self.next_goal_at = now + f64::from(rng.range_f32(0.2, 1.5));
            }
            FollowStatus::Stuck { from, to } => {
                self.avoid.push(Avoid {
                    from,
                    to,
                    until: now + 10.0,
                });
                self.failures += 1;
                self.follower = None;
                let goal = self.goal.filter(|_| self.failures < 3);
                if let Some(goal) = goal {
                    self.plan(ctx, input.origin, goal, now);
                } else {
                    self.failures = 0;
                    self.next_goal_at = now + 0.5;
                }
            }
        }
        Some(out)
    }
}

/// A step straight at `dest`.
fn straight(input: &FollowInput, dest: Vec3) -> FollowOutput {
    FollowOutput {
        move_dir: (dest - input.origin).truncate().normalize_or_zero(),
        speed: input.max_speed,
        look_at: Vec3::new(dest.x, dest.y, input.origin.z + EYE_HEIGHT),
        pitch: None,
        jump: false,
        duck: false,
        mandatory: false,
        status: FollowStatus::Moving,
    }
}

fn step(o: &FollowOutput) -> NavStep {
    NavStep {
        move_dir: o.move_dir,
        speed: o.speed,
        look_at: o.look_at,
        pitch: o.pitch,
        jump: o.jump,
        duck: o.duck,
        mandatory: o.mandatory,
    }
}

/// Navigation as one bot's behavior sees it for one frame: its path state, the graph, live traces.
pub struct BotNavService<'a, 'h> {
    pub nav: &'a mut BotNav,
    pub graph: Option<&'a NavGraph>,
    pub tracer: &'a mut LiveTracer<'h>,
    pub input: FollowInput,
    pub stuck_kill: f64,
    /// Stuck beyond recovery: the bot should `kill` itself.
    pub kill: bool,
}

impl BotNavService<'_, '_> {
    /// No progress for `stuck_kill` seconds of navigating: give up on this life.
    fn stuck(&mut self) -> bool {
        if self.nav.stuck_for(self.input.origin, self.input.now) > self.stuck_kill {
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
        let mut ctx = NavContext {
            graph,
            tracer: &mut *self.tracer,
        };
        let (status, out) = self.nav.go_to(&mut ctx, &self.input, dest);
        (status, out.as_ref().map(step))
    }

    fn roam(&mut self, rng: &mut Pcg32) -> Option<NavStep> {
        let graph = self.graph?;
        if self.stuck() {
            return None;
        }
        let mut ctx = NavContext {
            graph,
            tracer: &mut *self.tracer,
        };
        self.nav.tick(&mut ctx, &self.input, rng).as_ref().map(step)
    }

    fn away_from(&mut self, threat: Vec3) -> Option<Vec3> {
        let graph = self.graph?;
        let ctx = NavContext {
            graph,
            tracer: &mut *self.tracer,
        };
        BotNav::away_from(&ctx, self.input.origin, threat)
    }

    fn available(&self) -> bool {
        self.graph.is_some()
    }
}
