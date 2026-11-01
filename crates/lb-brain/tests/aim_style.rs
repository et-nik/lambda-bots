//! The bot's aim against an enemy that keeps showing up around it, strafing and stopping, written as an aimlog (the
//! format of `lb_aimlog`, `adapter/src/aimlog.cpp`) for the same analysis as a recording of people on a server:
//! `LB_AIMSIM=<dir> cargo test --release -p lb-brain --test aim_style -- --ignored --nocapture` writes
//! `<dir>/aim-sim-<preset>.tsv` for the hand presets (`-s<seed>` before `.tsv` with `LB_AIMSIM_SEEDS=<n>` seeds).
//!
//! With `LB_AIMSIM_REPLAY=<file>` the enemies are a recorded player's instead: where each of them was from that
//! player's eye, how it moved and whether it was in the player's sight, every 4 ms (a line a sample and enemy:
//! `t slot shown dx dy dz vx vy vz yaw flags`). The bot then faces what the player faced, and the two aims compare.

mod support;

use std::fmt::Write as _;

use lb_core::Vec3;
use lb_core::dmath;
use lb_core::rng::Pcg32;
use lb_game::input::{IN_ATTACK, IN_ATTACK2};
use lb_game::weapons::WeaponId;
use support::*;

const ENEMY: u8 = 2;
const STEP: f32 = 0.002;
const MINUTES: f64 = 10.0;
/// The states are written at 250 Hz, as the adapter does.
const SAMPLE: f64 = 0.004;

/// Where the enemy is and where it goes: it shows up at some angle off the bot's view, strafes or stands for a while,
/// hides, and shows up again elsewhere.
struct Enemy {
    rng: Pcg32,
    shown: bool,
    until: f64,
    next_turn: f64,
    speed: f32,
}

impl Enemy {
    fn appear(&mut self, now: f64, view_yaw: f32, e: &mut lb_raw::RawClient) {
        let r = &mut self.rng;
        let off = (16.0 * dmath::exp(0.8 * r.normal())).clamp(2.0, 175.0);
        let yaw = (view_yaw + if r.next_f32() < 0.5 { off } else { -off }).to_radians();
        let distance = r.range_f32(200.0, 800.0);
        let (sin, cos) = dmath::sin_cos(yaw);
        e.origin = Vec3::new(cos, sin, 0.0) * distance;
        self.speed = if r.next_f32() < 0.25 {
            0.0
        } else {
            r.range_f32(150.0, 300.0)
        };
        let across = Vec3::new(-sin, cos, 0.0) * if r.next_f32() < 0.5 { 1.0 } else { -1.0 };
        e.velocity = across * self.speed;
        self.next_turn = now + f64::from(r.range_f32(0.25, 0.8));
        self.until = now + f64::from(r.range_f32(1.0, 3.0));
        self.shown = true;
        e.effects &= !EF_NODRAW;
    }

    fn update(&mut self, now: f64, view_yaw: f32, e: &mut lb_raw::RawClient) {
        if now < self.until {
            if self.shown && now >= self.next_turn {
                self.next_turn = now + f64::from(self.rng.range_f32(0.25, 0.8));
                let to = e.origin.truncate();
                let across = Vec3::new(-to.y, to.x, 0.0).normalize_or_zero();
                let sign = if e.velocity.dot(across) > 0.0 { -1.0 } else { 1.0 };
                let speed = if self.rng.next_f32() < 0.2 { 0.0 } else { self.speed };
                e.velocity = across * sign * speed;
            }
            return;
        }
        if self.shown {
            self.shown = false;
            self.until = now + f64::from(self.rng.range_f32(0.4, 1.2));
            e.effects |= EF_NODRAW;
            e.velocity = Vec3::ZERO;
        } else {
            self.appear(now, view_yaw, e);
        }
    }
}

const FL_DUCKING: u32 = 1 << 14;

fn state(out: &mut String, t: f64, slot: u8, origin: Vec3, velocity: Vec3, view: Vec3, flags: u32) {
    let _ = writeln!(
        out,
        "S\t{t:.4}\t{slot}\t0\t{:.2}\t{:.2}\t{:.2}\t{:.1}\t{:.1}\t{:.1}\t28\t{:.3}\t{:.3}\t{flags}\t100\t0\t9mmhandgun\t0",
        origin.x, origin.y, origin.z, velocity.x, velocity.y, velocity.z, view.x, view.y
    );
}

/// One recorded enemy at one sample.
#[derive(Clone, Copy, Default)]
struct Mark {
    shown: bool,
    at: Vec3,
    vel: Vec3,
    yaw: f32,
    flags: u32,
}

/// A recorded player's enemies: their slots, and every 4 ms where each of them was.
struct Replay {
    slots: Vec<u8>,
    marks: Vec<Vec<Mark>>,
}

impl Replay {
    fn load(path: &str) -> Replay {
        let text = std::fs::read_to_string(path).expect("the replay");
        let mut slots: Vec<u8> = Vec::new();
        let mut marks: Vec<Vec<Mark>> = Vec::new();
        for line in text.lines().filter(|l| !l.starts_with('#')) {
            let f: Vec<&str> = line.split('\t').collect();
            let num = |i: usize| f[i].parse::<f32>().unwrap();
            let slot: u8 = f[1].parse().unwrap();
            let k = slots.iter().position(|s| *s == slot).unwrap_or_else(|| {
                slots.push(slot);
                slots.len() - 1
            });
            let i = (f64::from(num(0)) / SAMPLE).round() as usize;
            if marks.len() <= i {
                marks.resize(i + 1, Vec::new());
            }
            let row = &mut marks[i];
            if row.len() <= k {
                row.resize(k + 1, Mark::default());
            }
            row[k] = Mark {
                shown: f[2] == "1",
                at: Vec3::new(num(3), num(4), num(5)),
                vel: Vec3::new(num(6), num(7), num(8)),
                yaw: num(9),
                flags: f[10].parse().unwrap(),
            };
        }
        Replay { slots, marks }
    }

    fn secs(&self) -> f64 {
        self.marks.len() as f64 * SAMPLE
    }
}

fn simulate_replay(skill: u8, seed: u64, replay: &Replay) -> String {
    let mut log = String::new();
    let _ = writeln!(log, "# aim-sim replay skill {skill} seed {seed}");
    let _ = writeln!(log, "P\t0\t{ME}\t101\t1\tbot");
    let players: Vec<_> = replay
        .slots
        .iter()
        .map(|s| {
            let _ = writeln!(log, "P\t0\t{}\t{}\t0\tenemy{s}", s + 1, 101 + s);
            let mut p = player(s + 1, Vec3::new(5000.0, 0.0, 0.0), Vec3::ZERO);
            p.effects = EF_NODRAW;
            p
        })
        .collect();
    let world = World {
        players,
        no_floor: true,
        ..World::default()
    };
    let setup = Setup {
        frame: STEP,
        skill,
        seed,
        weapon: WeaponId::Glock,
    };
    let mut next_sample = 0.0;
    let mut seen = vec![None; replay.slots.len()];
    let frames = (replay.secs() / f64::from(STEP)) as usize;
    let r = run_scene(world, frames, &setup, &|_| {}, &mut |sc| {
        let now = sc.now.secs();
        let i = ((now / SAMPLE) as usize).min(replay.marks.len() - 1);
        let sample = now >= next_sample;
        if sample {
            next_sample += SAMPLE;
            state(
                &mut log,
                now,
                ME,
                Vec3::ZERO,
                Vec3::ZERO,
                sc.brain.motor.view,
                FL_ONGROUND,
            );
        }
        for (k, e) in sc.world.players.iter_mut().skip(1).enumerate() {
            let m = replay.marks[i].get(k).copied().unwrap_or_default();
            // Out of sight an enemy is put far below, out of everyone's way.
            e.origin = if m.shown {
                m.at + EYE
            } else {
                Vec3::new(0.0, 0.0, -10_000.0)
            };
            e.velocity = m.vel;
            e.angles = Vec3::new(0.0, m.yaw, 0.0);
            e.flags = m.flags;
            let half = if m.flags & FL_DUCKING != 0 { 18.0 } else { 36.0 };
            e.mins = Vec3::new(-16.0, -16.0, -half);
            e.maxs = Vec3::new(16.0, 16.0, half);
            if m.shown {
                e.effects &= !EF_NODRAW;
            } else {
                e.effects |= EF_NODRAW;
            }
            if sample {
                state(&mut log, now, e.slot, e.origin, e.velocity, e.angles, m.flags);
            }
            if seen[k] != Some(m.shown) {
                seen[k] = Some(m.shown);
                let v = u8::from(m.shown);
                let _ = writeln!(log, "V\t{now:.4}\t{}\t{ME}\t{v}\t{v}", e.slot);
            }
        }
    });
    shots(&mut log, &r.outs);
    log
}

fn shots(log: &mut String, outs: &[lb_motor::MotorOut]) {
    let mut held = 0u16;
    for (i, out) in outs.iter().enumerate() {
        let pressed = out.buttons & (IN_ATTACK | IN_ATTACK2);
        if pressed & !held != 0 {
            let _ = writeln!(
                log,
                "E\t{:.4}\t{ME}\tevents/glock1.sc",
                (i + 1) as f64 * f64::from(STEP)
            );
        }
        held = pressed;
    }
}

fn simulate(skill: u8, seed: u64) -> String {
    let mut log = String::new();
    let _ = writeln!(log, "# aim-sim skill {skill} seed {seed}");
    let _ = writeln!(log, "P\t0\t{ME}\t101\t1\tbot");
    let _ = writeln!(log, "P\t0\t{ENEMY}\t102\t0\tenemy");
    let mut enemy_player = player(ENEMY, Vec3::new(500.0, 0.0, 0.0), Vec3::ZERO);
    enemy_player.effects = EF_NODRAW;
    let world = World {
        players: vec![enemy_player],
        ..World::default()
    };
    let setup = Setup {
        frame: STEP,
        skill,
        seed,
        weapon: WeaponId::Glock,
    };
    let mut enemy = Enemy {
        rng: Pcg32::new(seed, 41),
        shown: false,
        until: 1.0,
        next_turn: 0.0,
        speed: 0.0,
    };
    let mut next_sample = 0.0;
    let mut seen = None;
    let frames = (MINUTES * 60.0 / f64::from(STEP)) as usize;
    let r = run_scene(world, frames, &setup, &|_| {}, &mut |sc| {
        let now = sc.now.secs();
        let view = sc.brain.motor.view;
        let e = &mut sc.world.players[1];
        enemy.update(now, view.y, e);
        if now >= next_sample {
            next_sample += SAMPLE;
            state(&mut log, now, ME, Vec3::ZERO, Vec3::ZERO, view, FL_ONGROUND);
            let to = e.origin + Vec3::Z * 28.0 - EYE;
            let facing = Vec3::new(0.0, dmath::atan2(-to.y, -to.x).to_degrees(), 0.0);
            state(&mut log, now, ENEMY, e.origin, e.velocity, facing, FL_ONGROUND);
        }
        if seen != Some(enemy.shown) {
            seen = Some(enemy.shown);
            let v = u8::from(enemy.shown);
            let _ = writeln!(log, "V\t{now:.4}\t{ENEMY}\t{ME}\t{v}\t{v}");
        }
    });
    shots(&mut log, &r.outs);
    log
}

#[test]
#[ignore = "writes aimlogs: run in release with --ignored and LB_AIMSIM=<dir>"]
fn aim_sim() {
    let dir = std::env::var("LB_AIMSIM").unwrap_or_else(|_| "target/aim-sim".into());
    std::fs::create_dir_all(&dir).unwrap();
    let replay = std::env::var("LB_AIMSIM_REPLAY").ok().map(|p| Replay::load(&p));
    let seeds: u64 = std::env::var("LB_AIMSIM_SEEDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);
    std::thread::scope(|scope| {
        for (name, skill) in [("easy", 25u8), ("normal", 50), ("hard", 75), ("expert", 100)] {
            for seed in 1..=seeds {
                let (dir, replay) = (&dir, &replay);
                scope.spawn(move || {
                    let suffix = if seeds > 1 { format!("-s{seed}") } else { String::new() };
                    let (path, log) = match replay {
                        Some(r) => (
                            format!("{dir}/aim-replay-{name}{suffix}.tsv"),
                            simulate_replay(skill, seed, r),
                        ),
                        None => (format!("{dir}/aim-sim-{name}{suffix}.tsv"), simulate(skill, seed)),
                    };
                    std::fs::write(&path, log).unwrap();
                    println!("{path}");
                });
            }
        }
    });
}
