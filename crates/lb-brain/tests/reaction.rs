//! How fast the bot answers an enemy, per skill preset: from the moment one comes into view, or starts shooting at
//! the bot from behind, to the bot's first shot at it. The whole grid runs many seeds and prints a table:
//! `cargo test --release -p lb-brain --test reaction -- --ignored --nocapture`.

mod support;

use lb_core::Vec3;
use lb_core::dmath;
use lb_core::rng::Pcg32;
use lb_game::sounds::{SoundClass, SoundKind};
use lb_game::weapons::WeaponId;
use lb_perception::SoundEvent;
use support::*;

const ENEMY: u8 = 2;
const RUN_SPEED: f32 = 250.0;
const SHOT_EVERY: f64 = 0.3;
const DMG_BULLET: i32 = 1 << 1;
/// How long the bot has to answer.
const WINDOW: f64 = 2.5;
const STEP: f32 = 0.002;
const PRESETS: [(&str, u8); 5] = [("noob", 0), ("easy", 25), ("normal", 50), ("hard", 75), ("expert", 100)];

#[derive(Clone, Copy, Debug)]
enum Scenario {
    /// Comes into view silently `angle` degrees off the bot's view and `distance` away, running across the line of
    /// sight (toward the middle of the view or away from it, by the seed).
    Runner { angle: f32, distance: f32 },
    /// Stands `distance` away, `angle` degrees off the view, looking away from the bot.
    Still { angle: f32, distance: f32 },
    /// Stands `angle` degrees off the view (behind the bot), `distance` away, and shoots it every 0.3 s.
    Shooter { angle: f32, distance: f32 },
}

impl Scenario {
    fn name(&self) -> String {
        match *self {
            Scenario::Runner { angle, distance } => format!("runner {angle}° {distance}u"),
            Scenario::Still { angle, distance } => format!("still {angle}° {distance}u"),
            Scenario::Shooter { angle, distance } => format!("shooter {angle}° {distance}u"),
        }
    }

    fn place(&self) -> (f32, f32) {
        match *self {
            Scenario::Runner { angle, distance }
            | Scenario::Still { angle, distance }
            | Scenario::Shooter { angle, distance } => (angle, distance),
        }
    }
}

fn grid() -> Vec<Scenario> {
    let mut cells = Vec::new();
    for angle in [0.0, 20.0, 45.0] {
        for distance in [300.0, 800.0, 1500.0] {
            cells.push(Scenario::Runner { angle, distance });
        }
    }
    for angle in [0.0, 45.0] {
        cells.push(Scenario::Still { angle, distance: 800.0 });
    }
    for angle in [90.0, 150.0] {
        cells.push(Scenario::Shooter { angle, distance: 500.0 });
    }
    cells
}

/// Seconds from the enemy's appearance to the bot's first shot; `None` when it did not shoot within the window.
fn trial(s: Scenario, skill: u8, seed: u64) -> Option<f64> {
    let mut scene_rng = Pcg32::new(seed, 99);
    // Past the deploy of the weapon in hand at the start; the vision phase varies with it.
    let appear = 1.0 + f64::from(scene_rng.range_f32(0.0, 0.05));
    let (angle, distance) = s.place();
    let (sin, cos) = dmath::sin_cos(angle.to_radians());
    let at = Vec3::new(cos, sin, 0.0) * distance;
    let across = Vec3::new(-sin, cos, 0.0);
    let (velocity, facing) = match s {
        Scenario::Runner { .. } => {
            let dir = if seed.is_multiple_of(2) { across } else { -across };
            (dir * RUN_SPEED, dmath::atan2(dir.y, dir.x).to_degrees())
        }
        Scenario::Still { .. } => (Vec3::ZERO, angle),
        Scenario::Shooter { .. } => (Vec3::ZERO, angle + 180.0),
    };
    let mut enemy = player(ENEMY, at - velocity * appear as f32, velocity);
    enemy.angles = Vec3::new(0.0, facing, 0.0);
    enemy.effects = EF_NODRAW;
    let world = World {
        players: vec![enemy],
        ..World::default()
    };
    let setup = Setup {
        frame: STEP,
        skill,
        seed,
        weapon: WeaponId::Glock,
    };
    let frames = ((appear + WINDOW) / f64::from(STEP)) as usize;
    let mut next_shot = appear;
    let shooter = matches!(s, Scenario::Shooter { .. });
    let r = run_scene(world, frames, &setup, &|_| {}, &mut |sc| {
        if sc.now.secs() < appear {
            return;
        }
        let e = &mut sc.world.players[1];
        e.effects &= !EF_NODRAW;
        let origin = e.origin;
        if shooter && sc.now.secs() >= next_shot {
            next_shot += SHOT_EVERY;
            sc.shots[usize::from(ENEMY)] = Some(sc.now);
            sc.sounds.push(SoundEvent {
                t: sc.now,
                source: Some(ENEMY),
                origin,
                class: SoundClass {
                    kind: SoundKind::Shot,
                    weapon: Some(WeaponId::Glock),
                },
                volume: 1.0,
                attenuation: 0.8,
                global: false,
            });
            let from = origin + Vec3::Z * 28.0;
            let view = sc.brain.motor.view;
            if let Some(d) =
                lb_perception::damage::stimulus(sc.now, 8, 0, DMG_BULLET, from, Vec3::ZERO, view, &mut scene_rng)
            {
                sc.brain.on_damage(&d);
            }
        }
    });
    r.first_fire.filter(|t| *t >= appear).map(|t| t - appear)
}

/// Reaction times of one cell over `seeds` runs.
#[derive(Clone, Debug)]
struct Cell {
    times: Vec<Option<f64>>,
}

impl Cell {
    fn run(s: Scenario, skill: u8, seeds: u64) -> Cell {
        Cell {
            times: (1..=seeds).map(|seed| trial(s, skill, seed)).collect(),
        }
    }

    /// Share of runs the bot shot in.
    fn hits(&self) -> f64 {
        self.times.iter().filter(|t| t.is_some()).count() as f64 / self.times.len() as f64
    }

    /// Percentile over all runs, a run without a shot counting as endless.
    fn percentile(&self, p: f64) -> f64 {
        let mut v: Vec<f64> = self.times.iter().map(|t| t.unwrap_or(f64::INFINITY)).collect();
        v.sort_by(f64::total_cmp);
        v[((v.len() - 1) as f64 * p).round() as usize]
    }

    fn median(&self) -> f64 {
        self.percentile(0.5)
    }

    fn show(&self) -> String {
        let secs = |t: f64| {
            if t.is_finite() {
                format!("{t:.2}")
            } else {
                "—".to_string()
            }
        };
        format!(
            "{}/{} {:>3.0}%",
            secs(self.median()),
            secs(self.percentile(0.9)),
            self.hits() * 100.0
        )
    }
}

/// Every cell of the grid at every preset, a column of presets per thread.
fn measure(cells: &[Scenario], presets: &[(&str, u8)], seeds: u64) -> Vec<Vec<Cell>> {
    std::thread::scope(|scope| {
        let columns: Vec<_> = presets
            .iter()
            .map(|&(_, skill)| scope.spawn(move || cells.iter().map(|&s| Cell::run(s, skill, seeds)).collect()))
            .collect();
        columns.into_iter().map(|c| c.join().unwrap()).collect()
    })
}

/// What a preset must answer within, seconds: the median for an enemy near the middle of the view at most 800 units
/// off, and its 90th percentile; the median for one at 45° or 1500 units; for one standing still at 0° and at 45°;
/// and for one shooting the bot from 90° and from 150°.
struct Bands {
    near: [f64; 3],
    wide: f64,
    still: [f64; 2],
    shooter: [f64; 2],
}

/// Strong players answer an enemy near the crosshair in some 0.12–0.2 s; the scale steps down from there.
fn bands(skill: u8) -> Option<Bands> {
    let b = |near, wide, still, shooter| {
        Some(Bands {
            near,
            wide,
            still,
            shooter,
        })
    };
    match skill {
        100 => b([0.10, 0.16, 0.20], 0.21, [0.30, 0.36], [0.24, 0.27]),
        75 => b([0.15, 0.24, 0.30], 0.32, [0.45, 0.53], [0.30, 0.36]),
        50 => b([0.18, 0.36, 0.45], 0.50, [0.70, 0.85], [0.37, 0.45]),
        25 => b([0.25, 0.65, 0.80], 0.95, [1.2, 1.5], [0.50, 0.60]),
        _ => None,
    }
}

/// Checks one cell against the preset's bands: `None` when it is within them.
fn off_band(s: Scenario, skill: u8, cell: &Cell) -> Option<String> {
    let b = bands(skill)?;
    let (median, p90) = (cell.median(), cell.percentile(0.9));
    let fail = |what: String| Some(format!("{}: {what} ({})", s.name(), cell.show()));
    if cell.hits() < 0.9 {
        return fail("shot in fewer than 90% of runs".into());
    }
    match s {
        Scenario::Runner { angle, distance } if angle <= 20.0 && distance <= 800.0 => {
            if !(b.near[0]..=b.near[1]).contains(&median) || p90 > b.near[2] {
                return fail(format!("median not in {:?} or p90 over {}", &b.near[..2], b.near[2]));
            }
        }
        Scenario::Runner { .. } if median > b.wide => return fail(format!("median over {}", b.wide)),
        Scenario::Still { angle, .. } => {
            let limit = if angle < 1.0 { b.still[0] } else { b.still[1] };
            if median > limit {
                return fail(format!("median over {limit}"));
            }
        }
        Scenario::Shooter { angle, .. } => {
            let limit = if angle < 120.0 { b.shooter[0] } else { b.shooter[1] };
            if median > limit {
                return fail(format!("median over {limit}"));
            }
        }
        Scenario::Runner { .. } => {}
    }
    None
}

#[test]
fn bots_answer_an_enemy() {
    let cells = [
        Scenario::Runner {
            angle: 0.0,
            distance: 300.0,
        },
        Scenario::Still {
            angle: 0.0,
            distance: 800.0,
        },
        Scenario::Shooter {
            angle: 90.0,
            distance: 500.0,
        },
    ];
    let presets = [("normal", 50), ("expert", 100)];
    let columns = measure(&cells, &presets, 12);
    for (column, (name, skill)) in columns.iter().zip(presets) {
        for (cell, s) in column.iter().zip(&cells) {
            assert_eq!(off_band(*s, skill, cell), None, "{name}");
        }
    }
}

/// Median/p90 seconds from the enemy's appearance to the first shot, and the share of runs with a shot.
#[test]
#[ignore = "the whole grid: run in release with --ignored --nocapture"]
fn reaction_grid() {
    let cells = grid();
    let columns = measure(&cells, &PRESETS, 40);
    print!("{:<22}", "median/p90 hit");
    for (name, _) in PRESETS {
        print!(" {name:<16}");
    }
    println!();
    for (i, s) in cells.iter().enumerate() {
        print!("{:<22}", s.name());
        for column in &columns {
            print!(" {:<16}", column[i].show());
        }
        println!();
    }
    let mut problems = Vec::new();
    for (i, s) in cells.iter().enumerate() {
        for ((name, skill), column) in PRESETS.iter().zip(&columns) {
            if let Some(why) = off_band(*s, *skill, &column[i]) {
                problems.push(format!("{name}, {why}"));
            }
        }
        // The better the bot, the sooner it answers.
        for pair in columns.windows(2) {
            let (worse, better) = (pair[0][i].median(), pair[1][i].median());
            if better > worse + 0.01 {
                problems.push(format!("{}: {better:.2} after {worse:.2} a level lower", s.name()));
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}
