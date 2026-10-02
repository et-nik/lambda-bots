//! The whole bot on a small synthetic scene: senses, beliefs, decisions, combat and motor, with a fake navigation
//! service. Walls and players are boxes; traces are exact segment tests.

#![allow(dead_code)]

use lb_brain::{Body, BotBrain, Character};
use lb_combat::Armed;
use lb_config::skill::Presets;
use lb_core::rng::{BotRng, Pcg32};
use lb_core::time::SimTime;
use lb_core::{Vec2, Vec3};
use lb_game::input::{IN_ATTACK, IN_ATTACK2};
use lb_game::weapons::WeaponId;
use lb_knowledge::{BeliefParams, PlayerKey};
use lb_motor::MotorOut;
use lb_nav_api::{NavService, NavStatus, NavStep};
use lb_perception::vision::DEFAULT_ASPECT;
use lb_perception::{Listener, SoundEvent, Subject, Viewer};
use lb_raw::{ClientState, RawClient};
use lb_styles::StyleId;
use lb_worldq::{AllVisible, Trace, TraceQuery, Tracer, contents};

pub const ME: u8 = 1;
pub const FRAME: f32 = 0.01;
pub const FL_ONGROUND: u32 = 1 << 9;
/// A player not drawn is out of everyone's sight.
pub const EF_NODRAW: u32 = 128;
pub const EYE: Vec3 = Vec3::new(0.0, 0.0, 28.0);

#[derive(Clone, Copy)]
pub struct Aabb {
    pub mins: Vec3,
    pub maxs: Vec3,
}

fn enter(a: Vec3, b: Vec3, bx: &Aabb) -> Option<f32> {
    let d = b - a;
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    for k in 0..3 {
        if d[k].abs() < 1e-6 {
            if a[k] < bx.mins[k] || a[k] > bx.maxs[k] {
                return None;
            }
            continue;
        }
        let (mut lo, mut hi) = ((bx.mins[k] - a[k]) / d[k], (bx.maxs[k] - a[k]) / d[k]);
        if lo > hi {
            std::mem::swap(&mut lo, &mut hi);
        }
        t0 = t0.max(lo);
        t1 = t1.min(hi);
        if t0 > t1 {
            return None;
        }
    }
    Some(t0)
}

/// The world: walls, players (a flat floor at z = -36 under everything) and a navigation service that walks
/// straight.
#[derive(Clone, Default)]
pub struct World {
    pub walls: Vec<Aabb>,
    pub players: Vec<RawClient>,
}

impl Tracer for World {
    fn trace(&mut self, q: &TraceQuery) -> Trace {
        let mut best = (1.0f32, None);
        let floor = Aabb {
            mins: Vec3::new(-10_000.0, -10_000.0, -1000.0),
            maxs: Vec3::new(10_000.0, 10_000.0, -36.0),
        };
        for w in self.walls.iter().chain(std::iter::once(&floor)) {
            if let Some(t) = enter(q.start, q.end, w)
                && t < best.0
            {
                best = (t, None);
            }
        }
        if !q.ignore_monsters {
            for c in &self.players {
                if Some(u16::from(c.slot)) == q.ignore {
                    continue;
                }
                let bx = Aabb {
                    mins: c.origin + c.mins,
                    maxs: c.origin + c.maxs,
                };
                if let Some(t) = enter(q.start, q.end, &bx)
                    && t < best.0
                {
                    best = (t, Some(u32::from(c.slot)));
                }
            }
        }
        let mut tr = Trace::clear(q.start + (q.end - q.start) * best.0);
        tr.fraction = best.0;
        tr.hit = best.1;
        tr
    }

    fn point_contents(&mut self, _p: Vec3) -> i32 {
        contents::EMPTY
    }
}

impl NavService for World {
    fn go_to(&mut self, dest: Vec3) -> (NavStatus, Option<NavStep>) {
        let dir = (dest - Vec3::ZERO).truncate().normalize_or_zero();
        (
            NavStatus::Moving,
            Some(NavStep {
                move_dir: dir,
                speed: 300.0,
                ..NavStep::hold(dest)
            }),
        )
    }

    fn roam(&mut self, _rng: &mut Pcg32) -> Option<NavStep> {
        Some(NavStep {
            move_dir: Vec2::X,
            speed: 300.0,
            ..NavStep::hold(Vec3::new(1000.0, 0.0, 28.0))
        })
    }

    fn away_from(&mut self, threat: Vec3) -> Option<Vec3> {
        Some(-threat)
    }

    fn available(&self) -> bool {
        true
    }
}

pub fn player(slot: u8, origin: Vec3, velocity: Vec3) -> RawClient {
    RawClient {
        slot,
        state: ClientState::Spawned,
        is_fake: true,
        is_ours: false,
        userid: 100 + i32::from(slot),
        origin,
        velocity,
        angles: Vec3::new(0.0, 180.0, 0.0),
        view_ofs: Vec3::new(0.0, 0.0, 28.0),
        mins: Vec3::new(-16.0, -16.0, -36.0),
        maxs: Vec3::new(16.0, 16.0, 36.0),
        flags: FL_ONGROUND,
        effects: 0,
        movetype: 3,
        solid: 3,
        deadflag: 0,
        waterlevel: 0,
        rendermode: 0,
        renderfx: 0,
        renderamt: 0.0,
        rendercolor: Vec3::ZERO,
        step_left: 0,
        frame: 0.0,
        frags: 0.0,
        sequence: 0,
        gaitsequence: 0,
        weaponmodel: 0,
        model: 0,
        ping_ms: 0,
    }
}

pub fn character(skill: u8) -> Character {
    Character {
        skill: Presets::default().at(skill),
        level: skill,
        aggression: 0.55,
        fear: 0.5,
        affinity: StyleId::Balanced.goal_affinity(),
        weapons: lb_brain::WeaponLike::default(),
        tricks: StyleId::Balanced.trick_likes(),
    }
}

pub fn body(now: SimTime, dt: f32, weapon: Option<WeaponId>) -> Body {
    Body {
        now,
        dt,
        origin: Vec3::ZERO,
        eye: EYE,
        velocity: Vec3::ZERO,
        maxspeed: 300.0,
        health: 100.0,
        armor: 0.0,
        has_longjump: false,
        on_ground: true,
        ducked: false,
        on_ladder: false,
        underwater: false,
        waterlevel: 0,
        fov: 0.0,
        weapon,
        arsenal: [
            Armed {
                id: WeaponId::Crowbar,
                clip: None,
                reserve: None,
                reserve2: None,
            },
            Armed {
                id: WeaponId::Glock,
                clip: Some(17),
                reserve: Some(68),
                reserve2: None,
            },
        ]
        .into_iter()
        .collect(),
        prediction: None,
        ammo_need: [0.0; 7],
        opponents: 2,
        damages: lb_game::mechanics::Damages::default(),
        dll: lb_game::dll::DllProfile::default(),
        gravity: 800.0,
        allowed: u32::MAX,
        gungame: None,
        selfgauss: 0,
        tricks: lb_config::main_config::TricksConfig::default(),
    }
}

pub struct Run {
    pub outs: Vec<MotorOut>,
    /// When the primary attack was first pressed, seconds.
    pub first_shot: Option<f64>,
    /// When either attack was first pressed (the glock's rapid fire is its secondary), seconds.
    pub first_fire: Option<f64>,
    pub brain: BotBrain,
}

/// How a run is set up.
#[derive(Clone, Copy, Debug)]
pub struct Setup {
    pub frame: f32,
    pub skill: u8,
    pub seed: u64,
    /// In hand at the start.
    pub weapon: WeaponId,
}

impl Default for Setup {
    fn default() -> Self {
        Setup {
            frame: FRAME,
            skill: 50,
            seed: 1,
            weapon: WeaponId::Glock,
        }
    }
}

/// What a scene may do before each frame's senses: show, hide and turn players, mark their shots, make sounds and
/// hurt the bot.
pub struct Scene<'a> {
    pub now: SimTime,
    pub world: &'a mut World,
    pub brain: &'a mut BotBrain,
    /// Last weapon event of each player, by slot (what vision sees as firing).
    pub shots: &'a mut [Option<SimTime>; 33],
    /// Sounds of this frame; the bot hears them before it looks.
    pub sounds: &'a mut Vec<SoundEvent>,
}

/// The bot stands at the origin; the scene's players move with their velocities. The game confirms any weapon
/// the bot selects on the next frame.
pub fn run(world: World, frames: usize, seed: u64) -> Run {
    run_dressed(world, frames, seed, WeaponId::Glock, &|_| {})
}

/// As [`run`], the bot's body dressed by `dress` every frame and `weapon` in hand at the start.
pub fn run_dressed(world: World, frames: usize, seed: u64, weapon: WeaponId, dress: &dyn Fn(&mut Body)) -> Run {
    let setup = Setup {
        seed,
        weapon,
        ..Setup::default()
    };
    run_scene(world, frames, &setup, dress, &mut |_| {})
}

/// As [`run_dressed`], set up by `setup`, with `scene` acting on the world before every frame.
pub fn run_scene(
    mut world: World,
    frames: usize,
    setup: &Setup,
    dress: &dyn Fn(&mut Body),
    scene: &mut dyn FnMut(&mut Scene<'_>),
) -> Run {
    world.players.insert(0, player(ME, Vec3::ZERO, Vec3::ZERO));
    let ch = character(setup.skill);
    let mut brain = BotBrain::new(ME, lb_perception::PerceptionParams::from_skill(&ch.skill));
    let mut rng = BotRng::new(setup.seed, 7);
    let mut weapon = Some(setup.weapon);
    let mut outs = Vec::new();
    let (mut first_shot, mut first_fire) = (None, None);
    let mut shots = [None; 33];
    let mut sounds = Vec::new();
    for i in 0..frames {
        let now = SimTime(i as f64 * f64::from(setup.frame));
        sounds.clear();
        scene(&mut Scene {
            now,
            world: &mut world,
            brain: &mut brain,
            shots: &mut shots,
            sounds: &mut sounds,
        });
        if !sounds.is_empty() {
            let listener = Listener {
                slot: ME,
                origin: Vec3::ZERO,
                eye: EYE,
                yaw: brain.motor.view.y,
                speed: 0.0,
            };
            brain.hear(&sounds, &listener, &AllVisible, &mut rng.perception);
        }
        let players = world.players.clone();
        let subjects: Vec<Subject<'_>> = players
            .iter()
            .map(|c| Subject {
                raw: c,
                key: PlayerKey {
                    slot: c.slot,
                    userid: c.userid,
                },
                team: 0,
                weapon: Some(WeaponId::Glock),
                shot_at: shots[usize::from(c.slot)],
            })
            .collect();
        let viewer = Viewer {
            index: u16::from(ME),
            eye: EYE,
            angles: brain.motor.view,
            fov: 0.0,
            aspect: DEFAULT_ASPECT,
            head_in_water: false,
            team: 0,
        };
        brain.see(now, &viewer, &subjects, &AllVisible, &mut world, &mut rng.perception);
        brain.update(
            now,
            &BeliefParams {
                track_forget: 8.0,
                maxspeed: 300.0,
            },
            None,
            None,
        );
        let mut b = body(now, setup.frame, weapon);
        dress(&mut b);
        let out = brain.act(&b, &ch, &mut world, None, &mut rng);
        if let Some(w) = out.commands.first().and_then(|c| WeaponId::from_classname(c)) {
            weapon = Some(w);
        }
        if out.buttons & IN_ATTACK != 0 && first_shot.is_none() {
            first_shot = Some(now.secs());
        }
        if out.buttons & (IN_ATTACK | IN_ATTACK2) != 0 && first_fire.is_none() {
            first_fire = Some(now.secs());
        }
        brain.motor.sent(out.buttons);
        outs.push(out);
        for p in world.players.iter_mut().skip(1) {
            p.origin += p.velocity * setup.frame;
        }
    }
    Run {
        outs,
        first_shot,
        first_fire,
        brain,
    }
}

pub fn wall(x: f32, half_width: f32) -> Aabb {
    Aabb {
        mins: Vec3::new(x, -half_width, -100.0),
        maxs: Vec3::new(x + 16.0, half_width, 200.0),
    }
}
