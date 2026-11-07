//! `lb-cli demo moves`: how players get about a map, from recordings of their games: the high ground nobody walks
//! onto that they get up onto with gauss boosts, and the ways up they take, counted and written as the map's
//! `moves.yaml` for the generator (`lb_navgen::climbs`). Players are told apart to be counted; nobody's name or id is
//! written or printed.

use std::cmp::Ordering;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use lb_bsp::BspWorld;
use lb_bsp::mech::Mechanisms;
use lb_bsp::world::WorldView;
use lb_config::moves::{HighGround, MovesFile, Way};
use lb_core::Vec3;
use lb_demo::timeline::{EF_NODRAW, Recording};
use lb_worldq::{HullKind, TraceQuery, Tracer};
use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};

const USAGE: &str =
    "usage: lb-cli demo moves <map.bsp> <file.dem|dir>... [--trusted <file.dem|dir>...] [--out <moves.yaml>]";

/// `MOVETYPE_WALK`: a player on foot (the game flies one on a ladder; an observer does not walk).
const WALKING: u8 = 3;
/// Recorded positions further apart in time than this break a player's track (a death, a ladder, a gap)...
const GAP: f32 = 0.3;
/// ...and so do positions further apart than a player moves at this speed (a teleport).
const TOO_FAST: f32 = 2500.0;
/// A bunny hop touches the floor and is off it again between two recorded positions (a demo has some 18 a second):
/// the lowest of them, with the floor at most this far under it, is where it touched.
const BOUNCE: f32 = 32.0;
/// A climb leaves the floor at most this long before the charged shot...
const LEFT: f32 = 1.0;
/// ...comes down at most this long after it...
const DOWN: f32 = 2.5;
/// ...and this much higher at least: more than a crouch jump lifts the feet (63).
const RISE: f32 = 72.0;
/// The push may not show yet this soon after the shot: the floor it was shot from does not end the climb.
const PUSH_SHOWS: f32 = 0.15;
/// Climbs off the floor this close to each other onto spots this close are one way up...
const SAME_SPOT: f32 = 128.0;
/// ...when on the same floors.
const SAME_LEVEL: f32 = 24.0;

/// Where a player was at a snapshot.
#[derive(Clone, Copy, Debug)]
struct Sample {
    t: f32,
    origin: Vec3,
    ducked: bool,
    /// The player, an index into the recording's players.
    who: u16,
}

impl Sample {
    fn feet(&self) -> f32 {
        self.origin.z - if self.ducked { 18.0 } else { 36.0 }
    }
}

/// A climb: off the floor at `from`, down at `to` (standing origins over the floor).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Found {
    from: Vec3,
    to: Vec3,
}

/// Whether two samples one after the other on a slot's track are not of one stretch of a player's moves.
fn apart(a: &Sample, b: &Sample) -> bool {
    let dt = b.t - a.t;
    a.who != b.who || dt > GAP || a.origin.distance(b.origin) > TOO_FAST * dt.max(0.01)
}

/// The climbs along one slot's track, with the player who made each: for every charged gauss shot (times, in order)
/// the last touch of the floor before it and the first after it, when that is `RISE` higher or more. `touch` is the
/// height of the floor sample `k` of the track touches, if it does.
fn climbs_along(
    track: &[Sample],
    shots: &[f32],
    touch: &mut dyn FnMut(&[Sample], usize) -> Option<f32>,
) -> Vec<(u16, Found)> {
    let mut out = Vec::new();
    let mut last_takeoff = None;
    for &t in shots {
        let i = track.partition_point(|s| s.t <= t);
        if i == 0 || t - track[i - 1].t > GAP {
            continue;
        }
        let mut k = i - 1;
        let takeoff = loop {
            if t - track[k].t > LEFT {
                break None;
            }
            if let Some(feet) = touch(track, k) {
                break Some((k, feet));
            }
            if k == 0 || apart(&track[k - 1], &track[k]) {
                break None;
            }
            k -= 1;
        };
        // A second shot in the same flight is the same climb.
        let Some((k, from_feet)) = takeoff.filter(|&(k, _)| last_takeoff != Some(k)) else {
            continue;
        };
        let mut landing = None;
        for j in i..track.len() {
            if track[j].t - t > DOWN || apart(&track[j - 1], &track[j]) {
                break;
            }
            match touch(track, j) {
                Some(feet) if track[j].t - t < PUSH_SHOWS && (feet - from_feet).abs() < 1.0 => {}
                Some(feet) => {
                    landing = Some((j, feet));
                    break;
                }
                None => {}
            }
        }
        let Some((j, to_feet)) = landing.filter(|&(_, feet)| feet - from_feet >= RISE) else {
            continue;
        };
        last_takeoff = Some(k);
        out.push((
            track[k].who,
            Found {
                from: track[k].origin.truncate().extend(from_feet + 36.0),
                to: track[j].origin.truncate().extend(to_feet + 36.0),
            },
        ));
    }
    out
}

/// The height of the floor at most `reach` under the feet of a player at `origin`, the hull let down onto it.
/// Recorded origins are rounded to 1/8 unit: the hull is lifted a little first.
fn floor_below(v: &mut WorldView<'_>, origin: Vec3, ducked: bool, reach: f32) -> Option<f32> {
    let (hull, half) = if ducked {
        (HullKind::Crouch, 18.0)
    } else {
        (HullKind::Stand, 36.0)
    };
    let end = origin - Vec3::Z * reach;
    for lift in [0.25, 1.0] {
        let tr = v.trace(&TraceQuery::hull(origin + Vec3::Z * lift, end, hull));
        if tr.start_solid || tr.all_solid {
            continue;
        }
        return (tr.fraction < 1.0 && tr.normal.z >= 0.7).then_some(tr.end.z - half);
    }
    None
}

/// The floor sample `k` touches: the floor under the player standing on it, or, at the lowest point of a stretch of
/// the track, the floor just under it (a bunny hop's touch between two samples).
fn touch(v: &mut WorldView<'_>, track: &[Sample], k: usize) -> Option<f32> {
    let s = &track[k];
    if let Some(feet) = floor_below(v, s.origin, s.ducked, 2.0) {
        return Some(feet);
    }
    let prev = k.checked_sub(1).map(|j| &track[j]).filter(|p| !apart(p, s))?;
    let next = track.get(k + 1).filter(|n| !apart(s, n))?;
    if s.feet() > prev.feet() || s.feet() >= next.feet() {
        return None;
    }
    floor_below(v, s.origin, s.ducked, BOUNCE)
}

/// What one recording of the map shows: the climbs players made (each with who made it, told apart by their id, else
/// their name) and the minutes players were seen alive.
struct Shown {
    climbs: Vec<(String, Found)>,
    minutes: f32,
}

fn shown(rec: &Recording, world: &BspWorld) -> Shown {
    let mut tracks: FxHashMap<u8, Vec<Sample>> = FxHashMap::default();
    for snap in &rec.snapshots {
        for s in rec.states_of(snap) {
            if s.who == u16::MAX
                || s.spectator != 0
                || s.iuser1 != 0
                || s.effects & EF_NODRAW != 0
                || s.movetype != WALKING
            {
                continue;
            }
            tracks.entry(s.slot).or_default().push(Sample {
                t: snap.time,
                origin: Vec3::from(s.origin),
                ducked: s.usehull == 1,
                who: s.who,
            });
        }
    }
    let mut shots: FxHashMap<u8, Vec<f32>> = FxHashMap::default();
    for s in &rec.shots {
        // `bparam1` is the primary fire: the charged shot that pushes has it 0.
        if s.event == "events/gauss.sc" && s.bparam1 == 0 && (1..=i32::from(rec.max_clients)).contains(&s.entity) {
            shots.entry((s.entity - 1) as u8).or_default().push(s.time);
        }
    }
    let player = |who: u16| rec.players.get(usize::from(who)).filter(|p| !p.bot && !p.hltv);
    let mut v = WorldView::new(world);
    let mut touches = |track: &[Sample], k: usize| touch(&mut v, track, k);
    let mut slots: Vec<u8> = tracks.keys().copied().collect();
    slots.sort_unstable();
    let mut out = Shown {
        climbs: Vec::new(),
        minutes: 0.0,
    };
    for slot in slots {
        let track = &tracks[&slot];
        let alive: f32 = track
            .windows(2)
            .filter(|w| !apart(&w[0], &w[1]) && player(w[0].who).is_some())
            .map(|w| w[1].t - w[0].t)
            .sum();
        out.minutes += alive / 60.0;
        let Some(times) = shots.get_mut(&slot) else { continue };
        times.sort_by(f32::total_cmp);
        for (who, found) in climbs_along(track, times, &mut touches) {
            if let Some(p) = player(who) {
                let id = if p.steam_id.is_empty() {
                    format!("name {}", p.name)
                } else {
                    p.steam_id.clone()
                };
                out.climbs.push((id, found));
            }
        }
    }
    out
}

fn near(a: Vec3, b: Vec3) -> bool {
    (a - b).truncate().length() <= SAME_SPOT && (a.z - b.z).abs() <= SAME_LEVEL
}

fn by_place(a: &Found, b: &Found) -> Ordering {
    let key = |c: &Found| [c.to.x, c.to.y, c.to.z, c.from.x, c.from.y, c.from.z];
    key(a)
        .iter()
        .zip(key(b).iter())
        .map(|(x, y)| x.total_cmp(y))
        .find(|o| o.is_ne())
        .unwrap_or(Ordering::Equal)
}

/// The climbs `among` made into ways up: each joins the first way whose first climb took off and came down near its
/// own, else starts one. Taken in the order of where they go, so the ways do not depend on the order the recordings
/// were read in.
fn ways(climbs: &[Found], among: &[usize]) -> Vec<Vec<usize>> {
    let mut order = among.to_vec();
    order.sort_by(|&a, &b| by_place(&climbs[a], &climbs[b]));
    let mut ways: Vec<Vec<usize>> = Vec::new();
    for i in order {
        let c = climbs[i];
        match ways.iter_mut().find(|w| {
            let first = climbs[w[0]];
            near(first.from, c.from) && near(first.to, c.to)
        }) {
            Some(w) => w.push(i),
            None => ways.push(vec![i]),
        }
    }
    ways
}

/// Where a way goes: its climb nearest to the others, takeoff and landing together.
fn middle(climbs: &[Found], way: &[usize]) -> Found {
    let spread = |c: &Found| -> f32 {
        way.iter()
            .map(|&j| c.from.distance(climbs[j].from) + c.to.distance(climbs[j].to))
            .sum()
    };
    way.iter()
        .map(|&i| climbs[i])
        .min_by(|a, b| spread(a).total_cmp(&spread(b)))
        .expect("a way has a climb")
}

/// The `.dem` files of `path`: itself, or those under it.
fn demo_files(path: &Path) -> Result<Vec<PathBuf>> {
    if !path.is_dir() {
        return Ok(vec![path.to_path_buf()]);
    }
    let mut out = Vec::new();
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).with_context(|| dir.display().to_string())? {
            let p = entry?.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("dem")) {
                out.push(p);
            }
        }
    }
    out.sort();
    Ok(out)
}

/// A map's name as a demo or a file gives it: no folder, no `.bsp`, lower case.
fn map_name(name: &str) -> String {
    let name = name.rsplit(['/', '\\']).next().unwrap_or(name);
    name.strip_suffix(".bsp").unwrap_or(name).to_lowercase()
}

fn point(v: Vec3) -> [f32; 3] {
    [v.x.round(), v.y.round(), v.z.round()]
}

/// For each climb: the player who made it, the recording it is in, whether that one is trusted.
#[derive(Clone, Copy)]
struct Whose {
    player: usize,
    recording: usize,
    trusted: bool,
}

/// The high ground of the file: the climbs onto each (`grounds`: which high ground each climb came down on) made into
/// ways up and counted, the most climbed first.
fn high_ground(found: &[Found], whose: &[Whose], grounds: &[Option<u32>]) -> Vec<HighGround> {
    let mut onto: FxHashMap<u32, Vec<usize>> = FxHashMap::default();
    for (i, g) in grounds.iter().enumerate() {
        if let Some(g) = g {
            onto.entry(*g).or_default().push(i);
        }
    }
    let mut out: Vec<(Found, HighGround)> = onto
        .values()
        .map(|climbs| {
            let distinct =
                |f: fn(&Whose) -> usize| climbs.iter().map(|&i| f(&whose[i])).collect::<FxHashSet<_>>().len() as u32;
            let trusted = |of: &[usize]| of.iter().filter(|&&i| whose[i].trusted).count() as u32;
            let mut ways: Vec<(Found, Way)> = ways(found, climbs)
                .iter()
                .map(|w| {
                    let m = middle(found, w);
                    let way = Way {
                        from: point(m.from),
                        to: point(m.to),
                        seen: w.len() as u32,
                        trusted: trusted(w),
                    };
                    (m, way)
                })
                .collect();
            ways.sort_by(|a, b| b.1.seen.cmp(&a.1.seen).then(by_place(&a.0, &b.0)));
            let first = ways[0].0;
            let h = HighGround {
                seen: climbs.len() as u32,
                players: distinct(|w| w.player),
                recordings: distinct(|w| w.recording),
                trusted: trusted(climbs),
                ways: ways.into_iter().map(|(_, w)| w).collect(),
            };
            (first, h)
        })
        .collect();
    out.sort_by(|a, b| b.1.seen.cmp(&a.1.seen).then(by_place(&a.0, &b.0)));
    out.into_iter().map(|(_, h)| h).collect()
}

pub fn run(args: &[&str]) -> Result<bool> {
    let [bsp, rest @ ..] = args else { bail!(USAGE) };
    let mut inputs: Vec<(PathBuf, bool)> = Vec::new();
    let mut out = None;
    let mut trusted = false;
    let mut it = rest.iter();
    while let Some(&a) = it.next() {
        match a {
            "--trusted" => trusted = true,
            "--out" => out = Some(PathBuf::from(it.next().ok_or_else(|| anyhow!(USAGE))?)),
            path => inputs.extend(demo_files(Path::new(path))?.into_iter().map(|p| (p, trusted))),
        }
    }
    if inputs.is_empty() {
        bail!(USAGE);
    }
    let bsp = Path::new(bsp);
    let map = map_name(&bsp.display().to_string());
    let bytes = std::fs::read(bsp).with_context(|| bsp.display().to_string())?;
    let mut world = BspWorld::load(&bytes).map_err(|e| anyhow!("{}: {e}", bsp.display()))?;
    let mech = Mechanisms::from_world(&world);
    lb_navgen::mapload::prepare_world(&mut world, &mech);

    let read: Vec<(Vec<Shown>, bool)> = {
        let world = &world;
        inputs
            .par_iter()
            .filter_map(|(path, trusted)| {
                let recs = std::fs::read(path)
                    .map_err(|e| e.to_string())
                    .and_then(|b| lb_demo::read(&b).map_err(|e| e.to_string()));
                match recs {
                    Ok(recs) => {
                        let shown = recs
                            .iter()
                            .filter(|r| map_name(&r.map) == map)
                            .map(|r| shown(r, world))
                            .collect();
                        Some((shown, *trusted))
                    }
                    Err(e) => {
                        eprintln!("{}: {e}; left out", path.display());
                        None
                    }
                }
            })
            .collect()
    };

    let mut file = MovesFile::new(&map);
    file.bsp_size = Some(world.bsp.fingerprint.1);
    let mut found: Vec<Found> = Vec::new();
    let mut whose: Vec<Whose> = Vec::new();
    let mut players: FxHashMap<String, usize> = FxHashMap::default();
    let mut minutes = 0.0;
    for (shown, trusted) in read {
        for s in shown {
            let recording = file.source.recordings as usize;
            file.source.recordings += 1;
            file.source.trusted += u32::from(trusted);
            minutes += s.minutes;
            for (id, f) in s.climbs {
                let n = players.len();
                let player = *players.entry(id).or_insert(n);
                found.push(f);
                whose.push(Whose {
                    player,
                    recording,
                    trusted,
                });
            }
        }
    }
    file.source.minutes = (minutes * 10.0).round() / 10.0;
    let landings: Vec<Vec3> = found.iter().map(|f| f.to).collect();
    let grounds = lb_navgen::climbs::high_ground_of(&mut world, &mech, &lb_navgen::GenOptions::default(), &landings);
    file.high_ground = high_ground(&found, &whose, &grounds);

    let walked = grounds.iter().filter(|g| g.is_none()).count();
    let made = file
        .high_ground
        .iter()
        .filter(|h| lb_navgen::climbs::evident(h))
        .count();
    println!(
        "{map}: {} recordings ({} trusted), {:.0} minutes of players alive; {} climbs by gauss boost, {walked} of \
         them onto floor reached on foot; {} high ground nobody walks onto, {made} of it climbed often enough to make \
         (in a trusted recording, or {} times by {} players)",
        file.source.recordings,
        file.source.trusted,
        file.source.minutes,
        found.len(),
        file.high_ground.len(),
        lb_navgen::climbs::SEEN,
        lb_navgen::climbs::PLAYERS
    );
    let mut table = String::new();
    for (i, h) in file.high_ground.iter().enumerate() {
        let _ = writeln!(
            table,
            "high ground {}: climbed {} times by {} players in {} recordings{}, {} ways up; {}",
            i + 1,
            h.seen,
            h.players,
            h.recordings,
            if h.trusted > 0 {
                format!(" ({} trusted)", h.trusted)
            } else {
                String::new()
            },
            h.ways.len(),
            if lb_navgen::climbs::evident(h) {
                "made"
            } else {
                "not made"
            }
        );
        for w in &h.ways {
            let at = |p: [f32; 3]| format!("[{:.0}, {:.0}, {:.0}]", p[0], p[1], p[2]);
            let _ = writeln!(
                table,
                "  {:>3} {:<9} from {:<22} to {:<22} up {:.0}",
                w.seen,
                if w.trusted > 0 { "(trusted)" } else { "" },
                at(w.from),
                at(w.to),
                w.to[2] - w.from[2]
            );
        }
    }
    print!("{table}");
    if let Some(out) = out {
        let text = format!(
            "# High ground nobody walks onto that players got up onto with gauss boosts on {map}, from recordings of\n\
             # their games (`lb-cli demo moves`): counts only. The generator makes boosts onto the high ground climbed in\n\
             # a trusted recording, or {} times by {} players at least.\n{}",
            lb_navgen::climbs::SEEN,
            lb_navgen::climbs::PLAYERS,
            file.to_yaml()?
        );
        MovesFile::parse(&text, &out.display().to_string())?;
        if let Some(dir) = out.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir).with_context(|| dir.display().to_string())?;
        }
        std::fs::write(&out, text).with_context(|| out.display().to_string())?;
        println!("written {}", out.display());
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A track over a floor at feet height 0 with a block whose top is at 192 between x 300 and 600: twenty samples
    /// a second, `x` and `z` of the feet given for each.
    fn track(feet: &[(f32, f32)]) -> Vec<Sample> {
        feet.iter()
            .enumerate()
            .map(|(i, &(x, z))| Sample {
                t: i as f32 * 0.05,
                origin: Vec3::new(x, 0.0, z + 36.0),
                ducked: false,
                who: 0,
            })
            .collect()
    }

    /// The floor under the feet: standing on it, or at the lowest point of a stretch within 32 units over it.
    fn touch(track: &[Sample], k: usize) -> Option<f32> {
        let s = &track[k];
        let ground = if (300.0..=600.0).contains(&s.origin.x) {
            192.0
        } else {
            0.0
        };
        let over = s.feet() - ground;
        if over.abs() < 0.5 {
            return Some(ground);
        }
        let (prev, next) = (track.get(k.checked_sub(1)?)?, track.get(k + 1)?);
        (over < BOUNCE && s.feet() <= prev.feet() && s.feet() < next.feet()).then_some(ground)
    }

    #[test]
    fn a_charged_shot_that_lifts_a_player_onto_the_block_is_a_climb_from_where_they_left_the_floor() {
        // Standing, a jump at 0.1 s, the shot at 0.3 s, up and onto the block at 0.65 s.
        let feet = [
            (200.0, 0.0),
            (200.0, 0.0),
            (205.0, 12.0),
            (210.0, 22.0),
            (215.0, 30.0),
            (220.0, 34.0),
            (240.0, 80.0),
            (260.0, 130.0),
            (280.0, 170.0),
            (300.0, 200.0),
            (320.0, 215.0),
            (340.0, 210.0),
            (350.0, 200.0),
            (360.0, 192.0),
            (362.0, 192.0),
        ];
        let tr = track(&feet);
        let found = climbs_along(&tr, &[0.31, 0.33], &mut touch);
        assert_eq!(
            found.len(),
            1,
            "a second shot in the same flight is the same climb: {found:?}"
        );
        let (_, c) = found[0];
        assert_eq!(c.from, Vec3::new(200.0, 0.0, 36.0));
        assert_eq!(c.to, Vec3::new(360.0, 0.0, 228.0));
        // A shot that leaves the player on the floor, or brings them down where they were, is none.
        let standing = track(&[(200.0, 0.0); 8]);
        assert!(climbs_along(&standing, &[0.01], &mut touch).is_empty());
        let jump: Vec<(f32, f32)> = feet[..6].iter().copied().chain([(225.0, 20.0), (230.0, 0.0)]).collect();
        assert!(climbs_along(&track(&jump), &[0.3], &mut touch).is_empty());
        // Nor is a shot long after leaving the floor (a fall from somewhere), nor one before a teleport up there.
        assert!(climbs_along(&tr, &[1.2], &mut touch).is_empty());
        let mut teleported = feet;
        teleported[8] = (900.0, 170.0);
        assert!(climbs_along(&track(&teleported), &[0.31], &mut touch).is_empty());
    }

    #[test]
    fn a_bunny_hopper_never_seen_on_the_floor_climbs_from_the_lowest_point_of_the_hop() {
        // Hops whose touches fall between the samples, the shot at the third, bounding onto the block.
        let feet = [
            (100.0, 30.0),
            (120.0, 14.0),
            (140.0, 9.0),
            (160.0, 20.0),
            (180.0, 26.0),
            (200.0, 8.0),
            (220.0, 60.0),
            (250.0, 130.0),
            (280.0, 180.0),
            (310.0, 215.0),
            (340.0, 212.0),
            (370.0, 199.0),
            (400.0, 207.0),
        ];
        let found = climbs_along(&track(&feet), &[0.26], &mut touch);
        assert_eq!(found.len(), 1, "{found:?}");
        let (_, c) = found[0];
        assert_eq!(c.from, Vec3::new(200.0, 0.0, 36.0));
        assert_eq!(c.to, Vec3::new(370.0, 0.0, 228.0));
    }

    #[test]
    fn climbs_near_each_other_are_one_way_up_whatever_order_they_come_in() {
        let c = |fx: f32, tx: f32| Found {
            from: Vec3::new(fx, 0.0, 36.0),
            to: Vec3::new(tx, 0.0, 228.0),
        };
        let climbs = [c(200.0, 360.0), c(900.0, 560.0), c(230.0, 380.0), c(180.0, 340.0)];
        let all: Vec<usize> = (0..climbs.len()).collect();
        let w = ways(&climbs, &all);
        assert_eq!(w.len(), 2, "{w:?}");
        let up: Vec<usize> = w.iter().map(Vec::len).collect();
        assert_eq!(up, [3, 1]);
        assert_eq!(middle(&climbs, &w[0]), climbs[0]);
        let mut reversed = climbs;
        reversed.reverse();
        let again: Vec<Found> = ways(&reversed, &all).iter().map(|w| middle(&reversed, w)).collect();
        assert_eq!(again, [climbs[0], climbs[1]]);
    }

    #[test]
    fn high_ground_is_counted_by_players_and_recordings_and_climbs_onto_reached_floor_are_left_out() {
        let c = |fx: f32, tx: f32| Found {
            from: Vec3::new(fx, 0.0, 36.0),
            to: Vec3::new(tx, 0.0, 228.0),
        };
        let found = [
            c(200.0, 360.0),
            c(900.0, 560.0),
            c(230.0, 380.0),
            c(180.0, 340.0),
            c(0.0, -400.0),
        ];
        let w = |player, recording, trusted| Whose {
            player,
            recording,
            trusted,
        };
        let whose = [
            w(0, 0, false),
            w(1, 0, false),
            w(0, 1, false),
            w(2, 2, true),
            w(3, 3, false),
        ];
        let grounds = [Some(0), Some(0), Some(0), Some(0), None];
        let h = high_ground(&found, &whose, &grounds);
        assert_eq!(h.len(), 1);
        let h = &h[0];
        assert_eq!((h.seen, h.players, h.recordings, h.trusted), (4, 3, 3, 1));
        let ways: Vec<(u32, u32)> = h.ways.iter().map(|w| (w.seen, w.trusted)).collect();
        assert_eq!(ways, [(3, 1), (1, 0)], "the most used first");
        assert_eq!(h.ways[0].from, [200.0, 0.0, 36.0]);
    }

    #[test]
    fn map_names_are_compared_without_folder_and_extension() {
        assert_eq!(map_name("maps/DM_Snow.bsp"), "dm_snow");
        assert_eq!(map_name("/srv/hl/valve/maps/dm_snow.bsp"), "dm_snow");
        assert_eq!(map_name("dm_snow"), "dm_snow");
    }
}
