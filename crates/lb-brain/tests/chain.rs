//! The whole bot on a small synthetic scene: senses, beliefs, decisions, combat and motor, with a fake navigation
//! service. Walls and players are boxes; traces are exact segment tests.

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
use lb_perception::{Subject, Viewer};
use lb_raw::{ClientState, RawClient};
use lb_styles::StyleId;
use lb_worldq::{AllVisible, Trace, TraceQuery, Tracer, contents};

const ME: u8 = 1;
const FRAME: f32 = 0.01;
const FL_ONGROUND: u32 = 1 << 9;

#[derive(Clone, Copy)]
struct Aabb {
    mins: Vec3,
    maxs: Vec3,
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
struct World {
    walls: Vec<Aabb>,
    players: Vec<RawClient>,
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

fn player(slot: u8, origin: Vec3, velocity: Vec3) -> RawClient {
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

fn character(skill: u8) -> Character {
    Character {
        skill: Presets::default().at(skill),
        level: skill,
        aggression: 0.55,
        fear: 0.5,
        affinity: StyleId::Balanced.goal_affinity(),
        weapons: lb_brain::WeaponLike::default(),
    }
}

fn body(now: SimTime, weapon: Option<WeaponId>) -> Body {
    Body {
        now,
        dt: FRAME,
        origin: Vec3::ZERO,
        eye: Vec3::new(0.0, 0.0, 28.0),
        velocity: Vec3::ZERO,
        maxspeed: 300.0,
        health: 100.0,
        armor: 0.0,
        has_longjump: false,
        on_ground: true,
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
        selfgauss: 0,
    }
}

struct Run {
    outs: Vec<MotorOut>,
    first_shot: Option<f64>,
    brain: BotBrain,
}

/// The bot stands at the origin; the scene's players move with their velocities. The game confirms any weapon
/// the bot selects on the next frame.
fn run(mut world: World, frames: usize, seed: u64) -> Run {
    world.players.insert(0, player(ME, Vec3::ZERO, Vec3::ZERO));
    let ch = character(50);
    let mut brain = BotBrain::new(ME, lb_perception::PerceptionParams::from_skill(&ch.skill));
    let mut rng = BotRng::new(seed, 7);
    let mut weapon = Some(WeaponId::Glock);
    let mut outs = Vec::new();
    let mut first_shot = None;
    for i in 0..frames {
        let now = SimTime(i as f64 * f64::from(FRAME));
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
                shot_at: None,
            })
            .collect();
        let viewer = Viewer {
            index: u16::from(ME),
            eye: Vec3::new(0.0, 0.0, 28.0),
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
        let out = brain.act(&body(now, weapon), &ch, &mut world, None, &mut rng);
        if let Some(w) = out.commands.first().and_then(|c| WeaponId::from_classname(c)) {
            weapon = Some(w);
        }
        if out.buttons & IN_ATTACK != 0 && first_shot.is_none() {
            first_shot = Some(now.secs());
        }
        brain.motor.sent(out.buttons);
        outs.push(out);
        for p in world.players.iter_mut().skip(1) {
            p.origin += p.velocity * FRAME;
        }
    }
    Run {
        outs,
        first_shot,
        brain,
    }
}

fn wall(x: f32, half_width: f32) -> Aabb {
    Aabb {
        mins: Vec3::new(x, -half_width, -100.0),
        maxs: Vec3::new(x + 16.0, half_width, 200.0),
    }
}

#[test]
fn an_enemy_in_sight_is_recognized_engaged_and_shot_at() {
    let world = World {
        players: vec![player(2, Vec3::new(500.0, -150.0, 0.0), Vec3::new(0.0, 100.0, 0.0))],
        ..World::default()
    };
    let r = run(world, 400, 3);
    let shot = r.first_shot.expect("the bot fired");
    // Normal skill: recognition 0.5–1.0 s at full rate (slower at this range), then turning and aiming.
    assert!(shot > 0.4 && shot < 3.0, "first shot at {shot}");
    let reactions = &r.brain.mind.reactions;
    assert_eq!(reactions.count, 1);
    assert!(reactions.recognition_to_shot / (reactions.count as f64) < 1.0);
    // The view ends up on the enemy.
    let last = r.outs.last().unwrap().angles;
    let enemy_yaw = lb_core::dmath::atan2(-150.0f32 + 100.0 * 4.0, 500.0).to_degrees();
    assert!(
        lb_core::math::angle_diff(last.y, enemy_yaw).abs() < 10.0,
        "{last} vs {enemy_yaw}"
    );
}

#[test]
fn a_silent_hidden_enemy_changes_no_command() {
    let visible = player(2, Vec3::new(400.0, -200.0, 0.0), Vec3::new(0.0, 120.0, 0.0));
    let base = World {
        walls: vec![wall(700.0, 500.0)],
        players: vec![visible],
    };
    let mut with_hidden = base.clone();
    with_hidden
        .players
        .push(player(3, Vec3::new(1000.0, 0.0, 0.0), Vec3::ZERO));
    let (a, b) = (run(base, 300, 11), run(with_hidden, 300, 11));
    assert!(a.first_shot.is_some(), "the visible enemy is fought");
    for (i, (x, y)) in a.outs.iter().zip(&b.outs).enumerate() {
        assert_eq!(
            (x.angles, x.forward, x.side, x.buttons, &x.commands),
            (y.angles, y.forward, y.side, y.buttons, &y.commands),
            "frame {i}"
        );
    }
}

#[test]
fn commands_never_conflict() {
    let world = World {
        players: vec![
            player(2, Vec3::new(300.0, -100.0, 0.0), Vec3::new(0.0, 150.0, 0.0)),
            player(3, Vec3::new(-200.0, 300.0, 0.0), Vec3::new(80.0, 0.0, 0.0)),
        ],
        ..World::default()
    };
    let r = run(world, 600, 5);
    let mut previous = 0u16;
    for out in &r.outs {
        assert!(out.buttons & IN_ATTACK == 0 || out.buttons & IN_ATTACK2 == 0);
        let jump = lb_game::input::IN_JUMP;
        assert!(
            out.buttons & jump == 0 || previous & jump == 0,
            "jumps are fresh presses"
        );
        assert!(out.angles.x.abs() <= 89.0);
        previous = out.buttons;
    }
}
