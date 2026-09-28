//! The offline obstacle course: bots navigate a map with the server's movement and the game's mechanisms simulated,
//! so traversals can be tested without a server. A bot here is the same navigator, motor and command driver the
//! server runs; the player moves by `lb_kin::player_move`; doors, lifts, buttons, triggers, teleports and
//! breakables behave as the SDK's entities do (`doors.cpp`, `plats.cpp`, `buttons.cpp`, `triggers.cpp`).

use lb_bsp::BspWorld;
use lb_bsp::mech::{Mechanisms, Mover, MoverKind, SF_TRIGGER_NOCLIENTS, TriggerKind};
use lb_config::skill::AimModel;
use lb_core::input::IN_USE;
use lb_core::math::view_angle_vectors;
use lb_core::rng::Pcg32;
use lb_core::time::SimTime;
use lb_core::{Vec2, Vec3};
use lb_host::driver::CommandDriver;
use lb_kin::boxworld::BoxWorld;
use lb_kin::{Cmd, MoveEvents, MoveWorld, Physics, Player, player_move};
use lb_motor::{Intents, LookIntent, LookParams, Motor, MotorInput, MoveIntent, Prio, StanceIntent};
use lb_nav::NavGraph;
use lb_nav::exec::{HitKind, MechView, MoverState, NavInput};
use lb_nav::known::LinkHealth;
use lb_nav::navigator::{Failure, NavCtx, Navigator};
use lb_nav_api::{NavStatus, NavStep, Tricks};
use lb_worldq::HullKind;

/// A world the course can move brushes in.
pub trait SimWorld: MoveWorld {
    fn place(&mut self, model: u16, offset: Vec3);
    fn set_solid(&mut self, model: u16, solid: bool);
    /// Bounds of brush `model` where it is now.
    fn bounds(&self, model: u16) -> Option<(Vec3, Vec3)>;
    /// A player box (`hull` at `origin`) overlaps brush or trigger `model` (the engine's hull-point test).
    fn overlaps(&self, model: u16, origin: Vec3, hull: HullKind) -> bool;
}

impl SimWorld for BspWorld {
    fn place(&mut self, model: u16, offset: Vec3) {
        if let Some(b) = self.brush_mut(model as usize) {
            b.offset = offset;
        }
    }

    fn set_solid(&mut self, model: u16, solid: bool) {
        if let Some(b) = self.brush_mut(model as usize) {
            b.solid = solid;
        }
    }

    fn bounds(&self, model: u16) -> Option<(Vec3, Vec3)> {
        match self.brush(model as usize) {
            Some(b) => Some((b.abs_mins(), b.abs_maxs())),
            None => self.bsp.models.get(model as usize).map(|m| (m.mins, m.maxs)),
        }
    }

    fn overlaps(&self, model: u16, origin: Vec3, hull: HullKind) -> bool {
        let offset = self.brush(model as usize).map_or(Vec3::ZERO, |b| b.position());
        self.hull_overlaps(model as usize, offset, origin, hull)
    }
}

impl SimWorld for BoxWorld {
    fn place(&mut self, model: u16, offset: Vec3) {
        if let Some(s) = self.solid_mut(u32::from(model)) {
            s.offset = offset;
        }
    }

    fn set_solid(&mut self, model: u16, solid: bool) {
        if let Some(s) = self.solid_mut(u32::from(model)) {
            s.enabled = solid;
        }
    }

    fn bounds(&self, model: u16) -> Option<(Vec3, Vec3)> {
        BoxWorld::bounds(self, u32::from(model))
            .map(|b| (b.min, b.max))
            .or_else(|| {
                self.triggers
                    .iter()
                    .find(|(_, id)| *id == u32::from(model))
                    .map(|(b, _)| (b.min, b.max))
            })
    }

    fn overlaps(&self, model: u16, origin: Vec3, hull: HullKind) -> bool {
        BoxWorld::overlaps(self, u32::from(model), origin, hull)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MState {
    Rest,
    ToActive,
    Active,
    ToRest,
}

#[derive(Clone, Debug)]
struct SimMover {
    m: Mover,
    state: MState,
    offset: Vec3,
    velocity: Vec3,
    wait_until: f64,
    /// Players this mover carries or pushes this frame.
    moved: Vec3,
}

#[derive(Clone, Debug)]
struct SimTrigger {
    model: u16,
    kind: TriggerKind,
    target: Option<String>,
    wait: f32,
    delay: f32,
    ready_at: f64,
    spent: bool,
    dest: Option<(Vec3, f32)>,
}

#[derive(Clone, Debug)]
struct SimBreakable {
    model: u16,
    health: f32,
    broken: bool,
}

/// The game's mechanisms, simulated.
#[derive(Clone, Debug, Default)]
pub struct Game {
    movers: Vec<SimMover>,
    triggers: Vec<SimTrigger>,
    breakables: Vec<SimBreakable>,
    /// `(name, targets with delays)` of multi_managers and relays.
    relays: Vec<(String, Vec<(String, f32)>)>,
    pending: Vec<(f64, String)>,
    pub log: Vec<String>,
}

fn clamp_to_box(v: Vec3, half: Vec3) -> Vec3 {
    let mut out = v;
    for i in 0..3 {
        out[i] = if v[i] > half[i] {
            v[i] - half[i]
        } else if v[i] < -half[i] {
            v[i] + half[i]
        } else {
            0.0
        };
    }
    out.normalize_or_zero()
}

fn box_distance(p: Vec3, mins: Vec3, maxs: Vec3) -> f32 {
    (p.clamp(mins, maxs) - p).length()
}

impl Game {
    /// Mechanisms of a map, movers placed at rest in `world`.
    pub fn from_map(world: &mut BspWorld, mech: &Mechanisms) -> Game {
        mech.place_at_rest(world);
        let mut g = Game::default();
        for m in &mech.movers {
            g.add_mover(m.clone());
        }
        for t in &mech.triggers {
            if matches!(t.kind, TriggerKind::Multiple | TriggerKind::Once) && t.spawnflags & SF_TRIGGER_NOCLIENTS != 0 {
                continue;
            }
            let dest = if t.kind == TriggerKind::Teleport {
                mech.teleport_destination(world, t)
            } else {
                None
            };
            g.triggers.push(SimTrigger {
                model: t.model as u16,
                kind: t.kind,
                target: t.target.clone(),
                wait: t.wait,
                delay: t.delay,
                ready_at: 0.0,
                spent: false,
                dest,
            });
        }
        for b in &mech.breakables {
            if b.breakable() {
                g.breakables.push(SimBreakable {
                    model: b.model as u16,
                    health: b.health.max(1.0),
                    broken: false,
                });
            }
        }
        for e in &world.entities {
            let class = e.classname();
            let Some(name) = e.get("targetname") else { continue };
            let targets: Vec<(String, f32)> = match class {
                "multi_manager" => e
                    .kv
                    .iter()
                    .filter(|(k, _)| !["classname", "origin", "targetname", "spawnflags", "wait"].contains(&k.as_str()))
                    .map(|(k, v)| {
                        (
                            k.split('#').next().unwrap_or(k).to_string(),
                            v.trim().parse().unwrap_or(0.0),
                        )
                    })
                    .collect(),
                "trigger_relay" => e
                    .get("target")
                    .map(|t| {
                        vec![(
                            t.to_string(),
                            e.get("delay").and_then(|d| d.parse().ok()).unwrap_or(0.0),
                        )]
                    })
                    .unwrap_or_default(),
                _ => continue,
            };
            g.relays.push((name.to_string(), targets));
        }
        g
    }

    pub fn add_mover(&mut self, m: Mover) {
        self.movers.push(SimMover {
            offset: m.rest,
            m,
            state: MState::Rest,
            velocity: Vec3::ZERO,
            wait_until: 0.0,
            moved: Vec3::ZERO,
        });
    }

    pub fn add_trigger(&mut self, model: u16, kind: TriggerKind, target: Option<&str>, dest: Option<(Vec3, f32)>) {
        self.triggers.push(SimTrigger {
            model,
            kind,
            target: target.map(str::to_string),
            wait: 0.2,
            delay: 0.0,
            ready_at: 0.0,
            spent: false,
            dest,
        });
    }

    pub fn add_breakable(&mut self, model: u16, health: f32) {
        self.breakables.push(SimBreakable {
            model,
            health,
            broken: false,
        });
    }

    fn mover_mut(&mut self, model: u16) -> Option<&mut SimMover> {
        self.movers.iter_mut().find(|m| m.m.model == model as usize)
    }

    /// Sets off a mover: doors and platforms start moving, buttons press in.
    fn activate(&mut self, model: u16, now: f64) {
        let Some(m) = self.mover_mut(model) else { return };
        let toggles = m.m.toggles();
        match (m.m.kind, m.state) {
            (_, MState::Rest) => m.state = MState::ToActive,
            (MoverKind::Door | MoverKind::RotatingDoor, MState::Active) if toggles => m.state = MState::ToRest,
            (MoverKind::Plat, MState::Active) => {
                // Someone on a raised platform keeps it up a second longer.
                m.wait_until = m.wait_until.max(now + 1.0);
                return;
            }
            _ => return,
        }
        let name = m.m.classname.clone();
        self.log.push(format!("{now:.2} *{model} {name} set off"));
    }

    /// Fires everything named `name` (`SUB_UseTargets`).
    fn fire(&mut self, name: &str, now: f64) {
        let models: Vec<u16> = self
            .movers
            .iter()
            .filter(|m| m.m.targetname.as_deref() == Some(name))
            .map(|m| m.m.model as u16)
            .collect();
        for model in models {
            self.activate(model, now);
        }
        let relayed: Vec<(String, f32)> = self
            .relays
            .iter()
            .filter(|(n, _)| n == name)
            .flat_map(|(_, t)| t.clone())
            .collect();
        for (target, delay) in relayed {
            self.pending.push((now + f64::from(delay), target));
        }
    }

    /// Moves the mechanisms by `dt`; players standing on a moving platform ride along.
    pub fn tick<W: SimWorld>(&mut self, world: &mut W, now: f64, dt: f64, players: &mut [Player]) {
        let due: Vec<String> = self
            .pending
            .iter()
            .filter(|(t, _)| *t <= now)
            .map(|(_, n)| n.clone())
            .collect();
        self.pending.retain(|(t, _)| *t > now);
        for name in due {
            self.fire(&name, now);
        }
        let mut fires = Vec::new();
        for m in &mut self.movers {
            m.moved = Vec3::ZERO;
            let goal = match m.state {
                MState::ToActive => m.m.active,
                MState::ToRest => m.m.rest,
                MState::Active => {
                    if m.m.wait >= 0.0 && !m.m.toggles() && m.wait_until <= now {
                        m.state = MState::ToRest;
                    }
                    m.velocity = Vec3::ZERO;
                    continue;
                }
                MState::Rest => {
                    m.velocity = Vec3::ZERO;
                    continue;
                }
            };
            let to = goal - m.offset;
            let step = (m.m.speed * dt as f32).max(0.0);
            let delta = if to.length() <= step {
                to
            } else {
                to.normalize_or_zero() * step
            };
            let new_offset = m.offset + delta;
            // Something standing in the way of a door that returns: it goes back (`CBaseDoor::Blocked`).
            let model = m.m.model as u16;
            let mut blocked = false;
            world.place(model, new_offset);
            for p in players.iter() {
                if p.ground != Some(u32::from(model)) && world.overlaps(model, p.origin, p.hull()) {
                    blocked = true;
                }
            }
            if blocked && m.m.kind != MoverKind::Button {
                world.place(model, m.offset);
                if m.m.wait >= 0.0 {
                    m.state = if m.state == MState::ToActive {
                        MState::ToRest
                    } else {
                        MState::ToActive
                    };
                }
                m.velocity = Vec3::ZERO;
                continue;
            }
            m.offset = new_offset;
            m.moved = delta;
            m.velocity = if dt > 0.0 { delta / dt as f32 } else { Vec3::ZERO };
            for p in players.iter_mut() {
                if p.ground == Some(u32::from(model)) {
                    p.origin += delta;
                    // Rounding sinks a rider into its platform over hundreds of small moves: push it back out, as
                    // `SV_PushMove` does.
                    let mut lift = 0;
                    while lift < 8 && world.overlaps(model, p.origin, p.hull()) {
                        p.origin.z += 1.0 / 64.0;
                        lift += 1;
                    }
                }
            }
            if (goal - m.offset).length() < 1e-3 {
                m.velocity = Vec3::ZERO;
                if m.state == MState::ToActive {
                    m.state = MState::Active;
                    m.wait_until = now + f64::from(m.m.wait.max(0.0));
                    if m.m.kind == MoverKind::Button
                        && let Some(t) = &m.m.target
                    {
                        fires.push(t.clone());
                    }
                } else {
                    m.state = MState::Rest;
                }
            }
        }
        for name in fires {
            self.fire(&name, now);
        }
    }

    /// Touches, triggers and the use key after a player's command.
    pub fn after_move<W: SimWorld>(
        &mut self,
        world: &mut W,
        p: &mut Player,
        ev: &MoveEvents,
        used: bool,
        view: Vec3,
        now: f64,
    ) -> bool {
        for &hit in &ev.touched {
            let touchy = self
                .movers
                .iter()
                .any(|m| m.m.model == hit as usize && m.m.touch && m.m.kind != MoverKind::Plat);
            if touchy {
                self.activate(hit as u16, now);
            }
        }
        // Platform trigger fields: a player on a platform at rest sends it up.
        let plats: Vec<u16> = self
            .movers
            .iter()
            .filter(|m| m.m.kind == MoverKind::Plat && m.m.touch && p.ground == Some(m.m.model as u32))
            .map(|m| m.m.model as u16)
            .collect();
        for model in plats {
            self.activate(model, now);
        }
        let mut teleported = false;
        let hull = p.hull();
        for i in 0..self.triggers.len() {
            let t = &self.triggers[i];
            if t.spent || now < t.ready_at || !world.overlaps(t.model, p.origin, hull) {
                continue;
            }
            match t.kind {
                TriggerKind::Teleport => {
                    if let Some((dest, yaw)) = t.dest {
                        p.origin = dest + Vec3::Z * (1.0 - hull.extents().0.z);
                        p.velocity = Vec3::ZERO;
                        p.ground = None;
                        let _ = yaw;
                        teleported = true;
                        self.log.push(format!("{now:.2} teleported by *{}", t.model));
                    }
                }
                TriggerKind::Multiple | TriggerKind::Once => {
                    let (target, delay, wait, once) = (t.target.clone(), t.delay, t.wait, t.kind == TriggerKind::Once);
                    let t = &mut self.triggers[i];
                    t.ready_at = now + f64::from(wait.max(0.2));
                    t.spent = once;
                    if let Some(target) = target {
                        self.pending.push((now + f64::from(delay), target));
                    }
                }
                _ => {}
            }
        }
        if used {
            self.player_use(world, p, view, now);
        }
        teleported
    }

    /// `CBasePlayer::PlayerUse`: the usable object within 64 units most in front of the view.
    fn player_use<W: SimWorld>(&mut self, world: &W, p: &Player, view: Vec3, now: f64) {
        let eye = p.eye();
        let (forward, _, _) = view_angle_vectors(view);
        let mut best: Option<(f32, u16)> = None;
        for m in &self.movers {
            if !m.m.usable {
                continue;
            }
            let model = m.m.model as u16;
            let Some((mins, maxs)) = world.bounds(model) else {
                continue;
            };
            if box_distance(p.origin, mins, maxs) > 64.0 {
                continue;
            }
            let center = (mins + maxs) * 0.5;
            let dot = clamp_to_box(center - eye, (maxs - mins) * 0.5).dot(forward);
            if dot > best.map_or(0.7, |b| b.0) {
                best = Some((dot, model));
            }
        }
        match best {
            Some((_, model)) => self.activate(model, now),
            None => self.log.push(format!("{now:.2} use pressed, nothing usable in front")),
        }
    }

    /// Damage to a breakable (or a shootable button).
    pub fn damage<W: SimWorld>(&mut self, world: &mut W, model: u16, amount: f32, now: f64) {
        if let Some(b) = self.breakables.iter_mut().find(|b| b.model == model && !b.broken) {
            b.health -= amount;
            if b.health <= 0.0 {
                b.broken = true;
                world.set_solid(model, false);
                self.log.push(format!("{now:.2} *{model} broken"));
            }
            return;
        }
        if self
            .movers
            .iter()
            .any(|m| m.m.model == model as usize && m.m.health > 0.0)
        {
            self.activate(model, now);
        }
    }

    /// Every mover is back at rest.
    pub fn quiet(&self) -> bool {
        self.pending.is_empty() && self.movers.iter().all(|m| m.state == MState::Rest || m.m.toggles())
    }

    pub fn mover_offset(&self, model: u16) -> Option<Vec3> {
        self.movers
            .iter()
            .find(|m| m.m.model == model as usize)
            .map(|m| m.offset)
    }
}

impl MechView for Game {
    fn mover(&self, model: u16) -> Option<MoverState> {
        let m = self.movers.iter().find(|m| m.m.model == model as usize)?;
        Some(MoverState {
            offset: m.offset,
            velocity: m.velocity,
        })
    }

    fn exists(&self, model: u16) -> bool {
        !self.breakables.iter().any(|b| b.model == model && b.broken)
    }

    fn hit_kind(&self, hit: u32) -> HitKind {
        if hit == 0 {
            HitKind::World
        } else if self.movers.iter().any(|m| m.m.model == hit as usize) {
            HitKind::Mover(hit as u16)
        } else {
            HitKind::Other
        }
    }
}

/// A bot on the course.
pub struct CourseBot {
    pub player: Player,
    pub nav: Navigator,
    pub motor: Motor,
    pub driver: CommandDriver,
    pub health: f32,
    pub rng: Pcg32,
    /// Seconds without moving 48 units while navigating (the server's stuck clock).
    pub stuck: f64,
    pub worst_stuck: f64,
    pub phases: Vec<(f64, &'static str)>,
    pub gait: Gait,
    /// What the bot may do on the way; with `longjump` it has the module.
    pub tricks: Tricks,
    /// A gauss boost the course plays the weapons' part of.
    boost: Option<CourseBoost>,
    last_fire: f64,
}

/// The weapons' part of a gauss boost: charging while the view turns to the boost's, the jump, and the recoil on
/// the command after the bot leaves the ground (the button comes up then, and the game fires after that move).
#[derive(Clone, Copy, Debug)]
struct CourseBoost {
    since: f64,
    view: Vec3,
    jumped: bool,
    /// Off the ground after the jump: the charge goes on the next command.
    armed: bool,
}

/// How a bot walked: time on plain walks, time of it spent against a wall, time looking steeply up or down, and
/// how far its view turned, degrees.
#[derive(Clone, Copy, Debug, Default)]
pub struct Gait {
    pub walking: f64,
    pub walled: f64,
    pub steep: f64,
    pub turned: f64,
}

impl Gait {
    pub fn add(&mut self, o: &Gait) {
        self.walking += o.walking;
        self.walled += o.walled;
        self.steep += o.steep;
        self.turned += o.turned;
    }
}

impl CourseBot {
    pub fn new(origin: Vec3, cmd_rate: f64) -> CourseBot {
        CourseBot {
            player: Player::standing(origin),
            nav: Navigator::default(),
            motor: Motor::default(),
            driver: CommandDriver::new(cmd_rate, 200.0),
            health: 100.0,
            rng: Pcg32::new(7, 7),
            stuck: 0.0,
            worst_stuck: 0.0,
            phases: Vec::new(),
            gait: Gait::default(),
            tricks: Tricks::default(),
            boost: None,
            last_fire: 0.0,
        }
    }
}

/// How a run went.
#[derive(Clone, Debug, Default)]
pub struct Outcome {
    pub arrived: bool,
    pub seconds: f64,
    /// Links that failed on the way (the navigator then planned around them).
    pub failures: Vec<Failure>,
    pub worst_stuck: f64,
    /// Buttons still held at the end (none should be).
    pub held: u16,
    pub phases: Vec<(f64, &'static str)>,
    pub log: Vec<String>,
    pub end: Vec3,
    pub gait: Gait,
}

pub struct Course<W: SimWorld> {
    pub world: W,
    pub game: Game,
    pub graph: NavGraph,
    pub phys: Physics,
    pub health: LinkHealth,
    pub now: f64,
    pub look: LookParams,
}

impl<W: SimWorld> Course<W> {
    pub fn new(world: W, game: Game, graph: NavGraph) -> Course<W> {
        Course {
            world,
            game,
            graph,
            phys: Physics::default(),
            health: LinkHealth::default(),
            now: 1.0,
            look: LookParams {
                model: AimModel::Spring,
                turn_speed: 900.0,
                skill: 100,
            },
        }
    }

    /// The bot's own state as navigation sees it now.
    fn input(&self, bot: &CourseBot) -> NavInput {
        let p = bot.player;
        NavInput {
            now: self.now,
            origin: p.origin,
            velocity: p.velocity,
            view: bot.motor.view,
            on_ground: p.on_ground(),
            on_ladder: p.on_ladder,
            ducked: p.ducked,
            waterlevel: p.waterlevel,
            ground_model: p.ground.map_or(0, |g| g as u16),
            max_speed: self.phys.maxspeed,
            health: bot.health,
            push: p.field,
            gravity: self.phys.gravity,
            tricks: bot.tricks,
        }
    }

    /// Asks the bot's navigation for a gauss boost toward its goal (`Navigator::gauss_leap`).
    pub fn gauss_leap(&mut self, bot: &mut CourseBot) -> bool {
        let input = self.input(bot);
        let mut ctx = NavCtx {
            graph: &self.graph,
            tracer: &mut self.world,
            mech: &self.game,
            health: Some(&mut self.health),
            bot: 1,
            budget: None,
        };
        bot.nav.gauss_leap(&mut ctx, &input)
    }

    /// One server frame for one bot heading to `dest`; `frame_ms` of game time passes.
    pub fn frame(&mut self, bot: &mut CourseBot, dest: Vec3, frame_ms: f64) -> NavStatus {
        let now = self.now;
        let p = bot.player;
        let input = NavInput {
            now,
            origin: p.origin,
            velocity: p.velocity,
            view: bot.motor.view,
            on_ground: p.on_ground(),
            on_ladder: p.on_ladder,
            ducked: p.ducked,
            waterlevel: p.waterlevel,
            ground_model: p.ground.map_or(0, |g| g as u16),
            max_speed: self.phys.maxspeed,
            health: bot.health,
            push: p.field,
            gravity: self.phys.gravity,
            tricks: bot.tricks,
        };
        bot.player.longjump = bot.tricks.longjump;
        let mut ctx = NavCtx {
            graph: &self.graph,
            tracer: &mut self.world,
            mech: &self.game,
            health: Some(&mut self.health),
            bot: 1,
            budget: None,
        };
        let (status, step) = bot.nav.go_to(&mut ctx, &input, dest);
        bot.stuck = bot.nav.stuck_for(p.origin, now);
        bot.worst_stuck = bot.worst_stuck.max(bot.stuck);
        let phase = bot.nav.phase();
        if bot.phases.last().is_none_or(|(_, ph)| *ph != phase) {
            bot.phases.push((now, phase));
        }
        let mut intents = Intents::default();
        if let Some(step) = step {
            apply(&mut intents, &step, &input);
            if let Some(at) = step.fire_at {
                self.fire(bot, &input, at, step.melee);
            }
        }
        match step.and_then(|s| s.boost) {
            Some(call) => {
                let b = bot.boost.get_or_insert(CourseBoost {
                    since: now,
                    view: call.view,
                    jumped: false,
                    armed: false,
                });
                intents.look(Prio::Protocol, LookIntent::Angles(call.view));
                intents.movement(
                    Prio::Protocol,
                    MoveIntent {
                        dir: Vec2::ZERO,
                        speed: 0.0,
                    },
                );
                let v = bot.motor.view;
                let settled =
                    lb_core::math::angle_diff(v.y, call.view.y).abs() <= 2.0 && (v.x - call.view.x).abs() <= 2.0;
                if (now - b.since >= f64::from(call.charge) && settled && p.on_ground()) || b.jumped {
                    b.jumped = true;
                    intents.stance(
                        Prio::Protocol,
                        StanceIntent {
                            jump: true,
                            duck: false,
                            longjump: false,
                        },
                    );
                }
            }
            None if bot.boost.is_some_and(|b| !b.jumped) => bot.boost = None,
            None => {}
        }
        let eye = p.eye();
        let view_before = bot.motor.view;
        let out = bot.motor.run(
            &intents,
            &MotorInput {
                now: SimTime(now),
                dt: (frame_ms / 1000.0) as f32,
                eye,
                velocity: p.velocity,
                maxspeed: self.phys.maxspeed,
                on_ladder: p.on_ladder,
                weapon: None,
            },
            &self.look,
            &mut bot.rng,
        );
        if let Some(sent) = bot.driver.tick(frame_ms, out.buttons) {
            bot.motor.sent(sent.buttons);
            let cmd = Cmd {
                angles: out.angles,
                forward: out.forward,
                side: out.side,
                up: 0.0,
                buttons: sent.buttons,
                msec: sent.msec,
            };
            let ev = player_move(&mut self.world, &self.phys, &mut bot.player, &cmd);
            if let Some(v) = ev.landed {
                bot.health -= self.phys.fall_damage(v);
            }
            if let Some(b) = bot.boost.as_mut().filter(|b| b.jumped) {
                if b.armed {
                    let (forward, _, _) = view_angle_vectors(b.view);
                    bot.player.velocity -= forward * 5.0 * bot.tricks.gauss_damage;
                    bot.boost = None;
                } else if !bot.player.on_ground() {
                    b.armed = true;
                }
            }
            if phase == "walk" && ev.walled {
                bot.gait.walled += f64::from(sent.msec) / 1000.0;
            }
            let used = sent.pressed(IN_USE);
            if self
                .game
                .after_move(&mut self.world, &mut bot.player, &ev, used, out.angles, now)
            {
                bot.motor.set_view(bot.motor.view);
            }
        }
        if phase == "walk" {
            let dt = frame_ms / 1000.0;
            bot.gait.walking += dt;
            if bot.motor.view.x.abs() > 20.0 {
                bot.gait.steep += dt;
            }
            bot.gait.turned += f64::from(lb_core::math::angle_diff(bot.motor.view.y, view_before.y).abs());
        }
        self.game.tick(
            &mut self.world,
            now,
            frame_ms / 1000.0,
            std::slice::from_mut(&mut bot.player),
        );
        self.now += frame_ms / 1000.0;
        status
    }

    /// Fire at a breakable or a shootable button: a hit every 0.1 s once the view is on it.
    fn fire(&mut self, bot: &mut CourseBot, input: &NavInput, at: Vec3, melee: bool) {
        let (forward, _, _) = view_angle_vectors(bot.motor.view);
        let want = (at - input.eye()).normalize_or_zero();
        let interval = if melee { 0.5 } else { 0.1 };
        if forward.dot(want) < 0.98 || self.now - bot.last_fire < interval {
            return;
        }
        bot.last_fire = self.now;
        if melee && input.eye().distance(at) > 96.0 {
            return;
        }
        let end = input.eye() + forward * 4096.0;
        let tr = self.world.trace(&lb_worldq::TraceQuery::line(input.eye(), end));
        if let Some(hit) = tr.hit.filter(|h| *h != 0) {
            let now = self.now;
            self.game
                .damage(&mut self.world, hit as u16, if melee { 25.0 } else { 12.0 }, now);
        }
    }

    /// Puts a bot where it stands: one idle command settles it on the floor and sets its ground.
    pub fn place(&mut self, bot: &mut CourseBot) {
        let idle = Cmd {
            msec: 10,
            ..Cmd::default()
        };
        player_move(&mut self.world, &self.phys, &mut bot.player, &idle);
    }

    /// Lets the mechanisms run with no one around until they are back at rest (at most `seconds`).
    pub fn settle(&mut self, seconds: f64) {
        let start = self.now;
        while !self.game.quiet() && self.now - start < seconds {
            self.game.tick(&mut self.world, self.now, 0.01, &mut []);
            self.now += 0.01;
        }
    }

    /// Runs a bot toward `dest` for at most `seconds` at `fps`; `long_frame` inserts one frame of that many ms
    /// halfway.
    pub fn run(&mut self, bot: &mut CourseBot, dest: Vec3, seconds: f64, fps: f64, long_frame: Option<f64>) -> Outcome {
        let frame_ms = 1000.0 / fps;
        let start = self.now;
        let log_start = self.game.log.len();
        let mut out = Outcome::default();
        let mut long = long_frame;
        let mut failures_seen = bot.nav.failures_total;
        while self.now - start < seconds {
            let ms = match long {
                Some(ms) if self.now - start > seconds / 4.0 => {
                    long = None;
                    ms
                }
                _ => frame_ms,
            };
            let status = self.frame(bot, dest, ms);
            if bot.nav.failures_total > failures_seen {
                failures_seen = bot.nav.failures_total;
                out.failures.extend(bot.nav.last_failure);
            }
            if status == NavStatus::Arrived {
                out.arrived = true;
                break;
            }
        }
        // A few idle frames: buttons must not stay pressed.
        for _ in 0..(fps * 0.1) as usize {
            let out_buttons = {
                let intents = Intents::default();
                let p = bot.player;
                bot.motor.run(
                    &intents,
                    &MotorInput {
                        now: SimTime(self.now),
                        dt: (frame_ms / 1000.0) as f32,
                        eye: p.eye(),
                        velocity: p.velocity,
                        maxspeed: self.phys.maxspeed,
                        on_ladder: p.on_ladder,
                        weapon: None,
                    },
                    &self.look,
                    &mut bot.rng,
                )
            };
            if let Some(sent) = bot.driver.tick(frame_ms, out_buttons.buttons) {
                bot.motor.sent(sent.buttons);
                out.held = sent.buttons;
            }
            self.now += frame_ms / 1000.0;
        }
        out.seconds = self.now - start;
        out.worst_stuck = bot.worst_stuck;
        out.phases = std::mem::take(&mut bot.phases);
        out.log = self.game.log[log_start..].to_vec();
        out.end = bot.player.origin;
        out.gait = std::mem::take(&mut bot.gait);
        out
    }
}

/// What the brain does with a navigation step: movement, stance and use, and the look (a traversal's look wins
/// over anything but lifecycle).
pub fn apply(intents: &mut Intents, step: &NavStep, input: &NavInput) {
    let prio = if step.mandatory { Prio::Traversal } else { Prio::Goal };
    intents.movement(
        Prio::Goal,
        MoveIntent {
            dir: step.move_dir,
            speed: step.speed,
        },
    );
    intents.stance(
        prio,
        StanceIntent {
            jump: step.jump,
            duck: step.duck,
            longjump: step.longjump,
        },
    );
    if step.use_key {
        intents.use_key(Prio::Traversal);
    }
    let look = match step.pitch {
        Some(pitch) => {
            let mut angles = lb_core::math::dir_to_view_angles(step.look_at - input.eye());
            angles.x = pitch;
            LookIntent::Angles(angles)
        }
        None => LookIntent::Point {
            at: step.look_at,
            engaged: false,
        },
    };
    intents.look(prio, look);
    let _ = Vec2::ZERO;
}
