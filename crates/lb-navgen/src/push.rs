//! Push fields (`trigger_push`): where running into one throws a player. Every field is run into from eight sides,
//! from the floor around it, walking in or jumping in, drifting with the push or keeping to its middle while it lifts
//! (a lift shaft carries to its top only a player that stays in it). Each flight is flown as it goes and braked to
//! land short of that; the landings seed the floor field and end push links. The generator then steers flights at
//! nodes near the field that its entries cannot walk to.

use lb_bsp::BspWorld;
use lb_bsp::world::WorldView;
use lb_core::{Vec2, Vec3};
use lb_kin::Physics;
use lb_kin::validate::{PushRun, probe_push};
use lb_worldq::HullKind;
use rayon::prelude::*;

use crate::field::settle;

/// Where a run into a field starts.
#[derive(Clone, Copy, Debug)]
pub struct PushEntry {
    pub model: usize,
    /// Standing hull centre on the floor.
    pub origin: Vec3,
    pub dir: Vec2,
    /// Middle of the field.
    pub center: Vec2,
    /// Farthest across a flight from here lands, 0 if none comes to rest.
    pub reach: f32,
}

impl PushEntry {
    /// The runs tried from the entry: walking or jumping in, drifting or keeping to the middle while lifted.
    pub fn runs(&self) -> [PushRun; 4] {
        let run = |jump_at, hold| PushRun {
            entry: self.origin,
            dir: self.dir,
            jump_at,
            hold,
            target: None,
        };
        let jump = Some(RUN_UP - JUMP_BEFORE);
        let hold = Some(self.center);
        [run(None, None), run(jump, None), run(None, hold), run(jump, hold)]
    }
}

/// A run into a field and where it comes to rest.
#[derive(Clone, Copy, Debug)]
pub struct PushFlight {
    pub model: usize,
    pub run: PushRun,
    pub landing: Vec3,
}

pub struct PushSites {
    pub entries: Vec<PushEntry>,
    pub flights: Vec<PushFlight>,
}

/// The run into a field starts this far from where the player first touches it.
const RUN_UP: f32 = 64.0;
/// A jump into a field is taken this far before it.
const JUMP_BEFORE: f32 = 32.0;
/// Shares of a free flight a braked one is steered to cover.
const BRAKED: [f32; 3] = [0.25, 0.5, 0.75];
/// Landings closer than this (and on about the same level) are one.
const SAME_LANDING: f32 = 48.0;

pub fn in_field(world: &BspWorld, p: Vec3) -> bool {
    world.push_at(p, HullKind::Stand) != Vec3::ZERO
}

/// Centre of a field's box.
pub fn field_center(world: &BspWorld, model: usize) -> Option<Vec3> {
    world.bsp.models.get(model).map(|m| (m.mins + m.maxs) * 0.5)
}

/// Where the fields in `world.pushes` are run into from, and where they throw a player.
pub fn push_sites(world: &BspWorld, phys: &Physics) -> PushSites {
    let mut entries: Vec<PushEntry> = Vec::new();
    let mut v = WorldView::new(world);
    for &(model, _) in &world.pushes {
        let Some(m) = world.bsp.models.get(model) else { continue };
        let (mins, maxs) = (m.mins, m.maxs);
        let center = (mins + maxs) * 0.5;
        let half = (maxs - mins) * 0.5;
        for k in 0..8 {
            let angle = k as f32 * std::f32::consts::FRAC_PI_4;
            let dir = Vec2::new(lb_core::dmath::cos(angle), lb_core::dmath::sin(angle));
            // Along `dir` from the centre to the side of the field, then out to where the run starts.
            let edge = [0, 1]
                .into_iter()
                .filter(|&a| dir[a].abs() > 1e-3)
                .map(|a| half[a] / dir[a].abs())
                .fold(f32::INFINITY, f32::min);
            let start = center.truncate() - dir * (edge + 16.0 + RUN_UP);
            for z in [mins.z + 38.0, maxs.z + 38.0] {
                let Some((feet, crouch, _)) = settle(&mut v, start.extend(z)) else {
                    continue;
                };
                let origin = start.extend(feet + 36.0);
                if crouch
                    || in_field(world, origin)
                    || entries
                        .iter()
                        .any(|e| e.model == model && e.origin.distance(origin) < 16.0)
                {
                    continue;
                }
                entries.push(PushEntry {
                    model,
                    origin,
                    dir,
                    center: center.truncate(),
                    reach: 0.0,
                });
            }
        }
    }
    // Every run as it flies, then braked to shares of that flight.
    let runs: Vec<(usize, PushRun)> = entries
        .iter()
        .enumerate()
        .flat_map(|(k, e)| e.runs().map(|r| (k, r)))
        .collect();
    let flown: Vec<Vec<(usize, PushRun, Vec3)>> = runs
        .par_iter()
        .map(|&(k, run)| {
            let mut v = WorldView::new(world);
            let free = probe_push(&mut v, phys, &run);
            if !free.ok {
                return Vec::new();
            }
            let mut out = vec![(k, run, free.landing)];
            if run.hold.is_some() {
                return out;
            }
            let from = run.entry.truncate();
            let to = free.landing.truncate();
            for share in BRAKED {
                let aim = (from + (to - from) * share).extend(free.landing.z);
                let braked = probe_push(
                    &mut v,
                    phys,
                    &PushRun {
                        target: Some(aim),
                        ..run
                    },
                );
                if braked.ok {
                    out.push((k, run, braked.landing));
                }
            }
            out
        })
        .collect();
    let mut flights: Vec<PushFlight> = Vec::new();
    for (k, run, landing) in flown.into_iter().flatten() {
        let e = &mut entries[k];
        // Thrown back where it started, into another field, or the same landing as another run of this field.
        if (landing - run.entry).truncate().length() < SAME_LANDING || in_field(world, landing) {
            continue;
        }
        e.reach = e.reach.max((landing - run.entry).truncate().length());
        if flights.iter().any(|f| {
            f.model == e.model
                && (f.landing - landing).truncate().length() < SAME_LANDING
                && (f.landing.z - landing.z).abs() < 24.0
        }) {
            continue;
        }
        flights.push(PushFlight {
            model: e.model,
            run,
            landing,
        });
    }
    PushSites { entries, flights }
}
