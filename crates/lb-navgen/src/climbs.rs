//! Gauss boosts up onto high ground that players were seen to make (`maps/<map>/moves.yaml`, taken from recordings of
//! their games by `lb-cli demo moves`). High ground nobody walks onto is not in the floor field and gets no nodes; the
//! floor flooded, the ways up onto it that check out flood it from where the players came down, put nodes at their
//! takeoffs and landings, and are linked with the boost. Only high ground climbed often enough is made: `SEEN` times
//! by `PLAYERS` players at least, or in a trusted recording. Without such moves the generator makes no boosts.

use lb_bsp::BspWorld;
use lb_bsp::mech::Mechanisms;
use lb_bsp::world::WorldView;
use lb_config::moves::MovesFile;
use lb_core::Vec3;
use lb_kin::Physics;
use lb_kin::tricks::{FULL_PUSH, TrickPlan, boost_view, plan_boost};
use rayon::prelude::*;
use rustc_hash::FxHashMap;

use crate::GenOptions;
use crate::field::{FloorField, NONE, Surroundings, settle};

/// High ground is made when climbed this many times...
pub const SEEN: u32 = 5;
/// ...by this many players at least (or in a trusted recording).
pub const PLAYERS: u32 = 3;
/// High ground made on a map at most, the trusted and the most climbed first...
pub const MOST: usize = 16;
/// ...and boosts made onto each at most, along the ways most used first.
pub const BOOSTS: usize = 6;

/// A way up onto high ground: off the floor at `from`, a gauss boost, down at `to` (player origins over the floor).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Climb {
    pub from: Vec3,
    pub to: Vec3,
}

/// High ground players were seen to get up onto: the ways up they took, the trusted and the most used first.
#[derive(Clone, Debug, PartialEq)]
pub struct HighGround {
    pub ways: Vec<Climb>,
}

/// Whether high ground of a moves file was climbed often enough to be made.
pub fn evident(h: &lb_config::moves::HighGround) -> bool {
    h.trusted > 0 || (h.seen >= SEEN && h.players >= PLAYERS)
}

/// The high ground of `file` to make: the evident, the trusted and the most climbed first, `MOST` at most.
pub fn to_make(file: &MovesFile) -> Vec<HighGround> {
    let mut made: Vec<&lb_config::moves::HighGround> = file.high_ground.iter().filter(|h| evident(h)).collect();
    made.sort_by(|a, b| b.trusted.cmp(&a.trusted).then(b.seen.cmp(&a.seen)));
    made.into_iter()
        .take(MOST)
        .map(|h| {
            let mut ways: Vec<&lb_config::moves::Way> = h.ways.iter().collect();
            ways.sort_by(|a, b| b.trusted.cmp(&a.trusted).then(b.seen.cmp(&a.seen)));
            HighGround {
                ways: ways
                    .into_iter()
                    .map(|w| Climb {
                        from: Vec3::from(w.from),
                        to: Vec3::from(w.to),
                    })
                    .collect(),
            }
        })
        .collect()
}

/// Takeoffs are tried round the players' own, this far off in eight directions...
const TAKEOFF_RINGS: [f32; 2] = [48.0, 96.0];
/// ...and landings round theirs.
const LANDING_RINGS: [f32; 1] = [48.0];
/// A spot round another is on the same floor when this close to its height.
const SAME_FLOOR: f32 = 18.0;

/// A way up that checks out: a boost from `from` comes down at `to` (standing origins on the floor), onto high ground
/// `ground` (an index into the options' high ground).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Checked {
    pub ground: usize,
    pub from: Vec3,
    pub to: Vec3,
}

/// Standing spots on the floor round `at`: on the floor under it, then round it on rings at the same height. None
/// when there is no floor under it, no standing room, or it is on a mover.
fn spots(v: &mut WorldView<'_>, at: Vec3, rings: &[f32]) -> Vec<Vec3> {
    let Some((feet, false, 0)) = settle(v, at) else {
        return Vec::new();
    };
    let mut out = vec![at.truncate().extend(feet + 36.0)];
    for &r in rings {
        for k in 0..8 {
            let (sin, cos) = lb_core::dmath::sin_cos((k as f32 * 45.0).to_radians());
            let p = at + Vec3::new(cos, sin, 0.0) * r;
            if let Some((f, false, 0)) = settle(v, p)
                && (f - feet).abs() <= SAME_FLOOR
            {
                out.push(p.truncate().extend(f + 36.0));
            }
        }
    }
    out
}

/// The boost from `from` onto `to` when one gets there and its beam spares the bot, as an overlay's is checked.
pub fn plan(v: &mut WorldView<'_>, phys: &Physics, from: Vec3, to: Vec3) -> Option<TrickPlan> {
    let plan = plan_boost(v, phys, from, to, FULL_PUSH)?;
    let view = boost_view((to - from).truncate().normalize_or_zero(), plan.pitch);
    lb_nav::tricks::beam_safe(v, lb_nav::tricks::boost_eye(from), view, plan.push / 5.0, true).then_some(plan)
}

/// The takeoff and the landing a boost checks out between for a way up: the players' own spots first, then the spots
/// round them. Also the traces it took.
fn check(world: &BspWorld, phys: &Physics, way: &Climb, hz: &dyn Surroundings) -> (Option<(Vec3, Vec3)>, u64) {
    let mut v = WorldView::new(world);
    // Neither end where no node goes: in a hazard, a closed door, a push field or on a ladder.
    let fit = |p: &Vec3| !(hz.hazard(*p) || hz.gate(*p) || hz.push(*p) || hz.ladder(*p));
    let landings: Vec<Vec3> = spots(&mut v, way.to, &LANDING_RINGS).into_iter().filter(fit).collect();
    let takeoffs: Vec<Vec3> = spots(&mut v, way.from, &TAKEOFF_RINGS)
        .into_iter()
        .filter(fit)
        .collect();
    let found = takeoffs.iter().find_map(|&from| {
        landings
            .iter()
            .find_map(|&to| plan(&mut v, phys, from, to).map(|_| (from, to)))
    });
    (found, v.traces)
}

/// The ways up onto `high_ground` that check out, the floor of `field` flooded on from their landings. A way onto
/// floor the field holds is left out: it is not high ground nobody walks onto. Also the traces it took.
pub fn onto_high_ground(
    world: &BspWorld,
    phys: &Physics,
    high_ground: &[HighGround],
    field: &mut FloorField,
    hz: &dyn Surroundings,
) -> (Vec<Checked>, u64) {
    let ways: Vec<(usize, &Climb)> = high_ground
        .iter()
        .enumerate()
        .flat_map(|(g, h)| h.ways.iter().map(move |w| (g, w)))
        .filter(|(_, w)| field.at(w.to).is_none())
        .collect();
    let found: Vec<(Option<(Vec3, Vec3)>, u64)> = ways.par_iter().map(|(_, w)| check(world, phys, w, hz)).collect();
    let traces = found.iter().map(|(_, t)| t).sum();
    let checked: Vec<Checked> = ways
        .iter()
        .zip(found)
        .filter_map(|(&(ground, _), (f, _))| f.map(|(from, to)| Checked { ground, from, to }))
        .collect();
    let landings: Vec<Vec3> = checked.iter().map(|c| c.to).collect();
    if !landings.is_empty() {
        field.extend(world, &landings, hz);
    }
    (checked, traces)
}

/// Which high ground nobody walks onto each of `landings` is on, as `generate` would flood the floor: the same number
/// for landings on the same high ground, none for a landing on floor reached on foot or with no floor under it.
pub fn high_ground_of(
    world: &mut BspWorld,
    mech: &Mechanisms,
    opts: &GenOptions,
    landings: &[Vec3],
) -> Vec<Option<u32>> {
    let (_, _, mut field) = crate::build::floor(world, mech, opts);
    let walked = field.spans.len() as u32;
    let high: Vec<Vec3> = landings.iter().copied().filter(|&l| field.at(l).is_none()).collect();
    let world: &BspWorld = world;
    field.extend(world, &high, &crate::site::Site::new(world, mech));
    let spans: Vec<Option<u32>> = landings.iter().map(|&l| field.at(l).filter(|&s| s >= walked)).collect();
    // Spans of the same high ground are joined by walking.
    let mut label: FxHashMap<u32, u32> = FxHashMap::default();
    let mut grounds = 0;
    for &start in spans.iter().flatten() {
        if label.contains_key(&start) {
            continue;
        }
        let mut stack = vec![start];
        label.insert(start, grounds);
        while let Some(s) = stack.pop() {
            for &n in &field.spans[s as usize].next {
                if n != NONE && n >= walked && !label.contains_key(&n) {
                    label.insert(n, grounds);
                    stack.push(n);
                }
            }
        }
        grounds += 1;
    }
    spans.into_iter().map(|s| s.map(|s| label[&s])).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use lb_config::moves::{Source, Way};

    /// The map's own surroundings, and a volume that hurts round `.1` too.
    struct HurtsAt<'a>(crate::site::Site<'a>, Vec3);

    impl Surroundings for HurtsAt<'_> {
        fn hazard(&self, origin: Vec3) -> bool {
            let d = origin - self.1;
            (d.truncate().length() < 128.0 && d.z.abs() < 72.0) || self.0.hazard(origin)
        }
        fn gate(&self, origin: Vec3) -> bool {
            self.0.gate(origin)
        }
        fn water(&self, origin: Vec3) -> bool {
            self.0.water(origin)
        }
        fn push(&self, origin: Vec3) -> bool {
            self.0.push(origin)
        }
        fn ladder(&self, origin: Vec3) -> bool {
            self.0.ladder(origin)
        }
    }

    /// dm_snow's pillar top (skipped without the map): the way players take up holds, and is left out once where
    /// they take off or come down hurts.
    #[test]
    fn no_boost_takes_off_or_comes_down_where_it_hurts() {
        let Some(dir) = lb_bsp::test_maps_dir() else { return };
        let Ok(bsp) = std::fs::read(dir.join("dm_snow.bsp")) else {
            return;
        };
        let mut world = BspWorld::load(&bsp).unwrap();
        let mech = Mechanisms::from_world(&world);
        let opts = GenOptions::default();
        let way = Climb {
            from: Vec3::new(1050.0, 250.0, -252.0),
            to: Vec3::new(900.0, 500.0, -60.0),
        };
        let high = [HighGround { ways: vec![way] }];
        let mut made = |hurts: Option<Vec3>| {
            let (_, _, mut field) = crate::build::floor(&mut world, &mech, &opts);
            let world: &BspWorld = &world;
            let site = crate::site::Site::new(world, &mech);
            let checked = match hurts {
                Some(at) => onto_high_ground(world, &opts.physics, &high, &mut field, &HurtsAt(site, at)).0,
                None => onto_high_ground(world, &opts.physics, &high, &mut field, &site).0,
            };
            checked.len()
        };
        assert_eq!(made(None), 1);
        assert_eq!(made(Some(way.to)), 0, "the landing hurts");
        assert_eq!(made(Some(way.from)), 0, "the takeoff hurts");
    }

    fn ground(seen: u32, players: u32, trusted: u32, to: f32) -> lb_config::moves::HighGround {
        let way = |y: f32, seen: u32, trusted: u32| Way {
            from: [0.0, y, 36.0],
            to: [to, y, 228.0],
            seen,
            trusted,
        };
        let mut ways = vec![way(0.0, seen - trusted, 0)];
        if trusted > 0 {
            ways.push(way(64.0, trusted, trusted));
        }
        lb_config::moves::HighGround {
            seen,
            players,
            recordings: 1,
            trusted,
            ways,
        }
    }

    #[test]
    fn only_high_ground_climbed_often_enough_or_in_a_trusted_recording_is_made() {
        let mut f = MovesFile::new("dm_example");
        f.source = Source {
            recordings: 9,
            trusted: 1,
            minutes: 200.0,
        };
        f.high_ground = vec![
            ground(4, 4, 0, 1.0),
            ground(9, 2, 0, 2.0),
            ground(5, 3, 0, 3.0),
            ground(12, 8, 0, 4.0),
            ground(1, 1, 1, 5.0),
        ];
        let made: Vec<f32> = to_make(&f).iter().map(|h| h.ways[0].to.x).collect();
        assert_eq!(made, [5.0, 4.0, 3.0], "trusted first, then the most climbed");
        assert_eq!(to_make(&f)[0].ways[0].to.y, 64.0, "its trusted way first");
        f.high_ground.truncate(2);
        assert!(to_make(&f).is_empty());
    }
}
