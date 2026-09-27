//! What the generator takes from the map besides its floors: where players and items are (the seeds of the floor
//! field and spots that must have a node), ladders, lifts and teleports, the poses movers take while the floor is
//! flooded, and which spots hurt, lie in water or are closed off by a door at rest.

use lb_bsp::BspWorld;
use lb_bsp::mech::{Mechanisms, MoverKind, SF_DOOR_START_OPEN, TriggerKind};
use lb_bsp::world::{BrushKind, WorldView};
use lb_core::{Vec2, Vec3};
use lb_worldq::{HullKind, TraceQuery, Tracer, contents};

use crate::field::{Surroundings, harmful_contents, settle};
use lb_kin::Physics;
use lb_kin::validate::simulate_climb;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeedKind {
    Spawn,
    Item,
    /// An AI hint node the mapper placed.
    Hint,
    /// Inside a teleport trigger, where walking in sets it off.
    TeleportEntry,
    TeleportExit,
    /// On a lift platform at rest.
    LiftBoard,
    /// Next to where a lift stops raised.
    LiftExit,
    /// The floor at a ladder's foot or on the ledge at its top.
    LadderEnd,
    /// Where a button is in reach of the use key.
    UseSpot,
}

/// A standing hull centre the floor is flooded from; some also get a node of their own.
#[derive(Clone, Copy, Debug)]
pub struct Seed {
    pub origin: Vec3,
    pub kind: SeedKind,
}

impl SeedKind {
    /// Spots that get a node even where the placement would put none.
    pub fn mandatory(self) -> bool {
        !matches!(self, SeedKind::Hint)
    }
}

/// A ladder the generator puts nodes on: where a player climbs it, and which way its face looks.
#[derive(Clone, Copy, Debug)]
pub struct LadderSite {
    pub model: usize,
    /// Horizontal centre of the climbing spot and the face normal.
    pub spot: Vec2,
    pub normal: Vec3,
    pub bottom: f32,
    pub top: f32,
}

fn is_item(class: &str) -> bool {
    (class.starts_with("weapon_") || class.starts_with("item_") || class.starts_with("ammo_"))
        && class != "weapon_satchel_charge"
}

/// Where a mover is while the floor is flooded: doors open (so the floor beyond is found), lifts and the rest where
/// they wait.
pub fn flood_poses(world: &mut BspWorld, mech: &Mechanisms) {
    mech.place_at_rest(world);
    for m in &mech.movers {
        let Some(b) = world.brush_mut(m.model) else { continue };
        match m.kind {
            MoverKind::RotatingDoor => b.solid = false,
            MoverKind::Door if !is_platform(b.mins, b.maxs, (m.active - m.rest).z, m.vertical()) => {
                b.offset = if m.spawnflags & SF_DOOR_START_OPEN != 0 {
                    m.rest
                } else {
                    m.active
                };
            }
            _ => {}
        }
    }
    for br in &mech.breakables {
        if br.breakable()
            && let Some(b) = world.brush_mut(br.model)
        {
            b.solid = false;
        }
    }
}

/// Puts movers and breakables back where they are when the map starts.
pub fn rest_poses(world: &mut BspWorld, mech: &Mechanisms) {
    mech.place_at_rest(world);
    for m in &mech.movers {
        if m.kind == MoverKind::RotatingDoor
            && let Some(b) = world.brush_mut(m.model)
        {
            b.solid = true;
        }
    }
    for br in &mech.breakables {
        if let Some(b) = world.brush_mut(br.model) {
            b.solid = true;
        }
    }
}

/// A door that rises with a top a player can stand on is a lift, not a door to open.
pub fn is_platform(mins: Vec3, maxs: Vec3, rise: f32, vertical: bool) -> bool {
    let size = maxs - mins;
    vertical && rise > 0.0 && size.x.min(size.y) >= 32.0
}

/// What the map offers the generator: seeds, ladders, and teleports as (spot inside the trigger, arrival).
pub struct MapSites {
    pub seeds: Vec<Seed>,
    pub ladders: Vec<LadderSite>,
    pub teleports: Vec<(Vec3, Vec3)>,
}

pub fn sites(world: &BspWorld, mech: &Mechanisms) -> MapSites {
    let mut out = Vec::new();
    let mut teleports = Vec::new();
    let mut v = WorldView::new(world);
    for e in &world.entities {
        let class = e.classname();
        let kind = match class {
            "info_player_deathmatch" | "info_player_start" => SeedKind::Spawn,
            "info_node" => SeedKind::Hint,
            c if is_item(c) => SeedKind::Item,
            _ => continue,
        };
        // Spawn points sit at the hull centre (the engine drops the player from there); items and nodes on the floor.
        let origin = if kind == SeedKind::Spawn {
            e.origin() + Vec3::Z
        } else {
            e.origin() + Vec3::Z * 36.0
        };
        out.push(Seed { origin, kind });
    }
    for t in mech.triggers.iter().filter(|t| t.kind == TriggerKind::Teleport) {
        // Triggers are not brushes the world traces: their models stay where they were compiled.
        let Some(m) = world.bsp.models.get(t.model) else {
            continue;
        };
        let (mins, maxs) = (m.mins, m.maxs);
        let center = (mins + maxs) * 0.5;
        let inside = Vec3::new(center.x, center.y, (mins.z + 36.0).min(maxs.z));
        let entry = settle(&mut v, inside)
            .map(|(feet, _, _)| Vec3::new(center.x, center.y, feet + 36.0))
            .filter(|&spot| world.hull_overlaps(t.model, Vec3::ZERO, spot, HullKind::Stand));
        let exit = mech
            .teleport_destination(world, t)
            .map(|(dest, _)| dest + Vec3::Z * 37.0);
        if let Some(entry) = entry {
            out.push(Seed {
                origin: entry,
                kind: SeedKind::TeleportEntry,
            });
        }
        if let Some(exit) = exit {
            out.push(Seed {
                origin: exit,
                kind: SeedKind::TeleportExit,
            });
        }
        if let (Some(entry), Some(exit)) = (entry, exit) {
            teleports.push((entry, exit));
        }
    }
    for m in &mech.movers {
        let Some(b) = world.brush(m.model) else { continue };
        let rise = (m.active - m.rest).z;
        let lift = matches!(m.kind, MoverKind::Plat)
            || (m.kind == MoverKind::Door && is_platform(b.mins, b.maxs, rise, m.vertical()));
        if !lift || rise <= 0.0 {
            continue;
        }
        let (mins, maxs) = (b.mins + m.rest, b.maxs + m.rest);
        let center = (mins + maxs) * 0.5;
        out.push(Seed {
            origin: Vec3::new(center.x, center.y, maxs.z + 36.0),
            kind: SeedKind::LiftBoard,
        });
        let top = maxs.z + rise;
        for (dx, dy) in [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
            let half = (maxs - mins) * 0.5;
            let p = Vec3::new(
                center.x + dx * (half.x + 32.0),
                center.y + dy * (half.y + 32.0),
                top + 36.0 + 2.0,
            );
            if let Some((feet, _, _)) = settle(&mut v, p)
                && (feet - top).abs() <= 24.0
            {
                out.push(Seed {
                    origin: Vec3::new(p.x, p.y, feet + 36.0),
                    kind: SeedKind::LiftExit,
                });
            }
        }
    }
    for m in mech.movers.iter().filter(|m| m.kind == MoverKind::Button) {
        let Some(b) = world.brush(m.model) else { continue };
        if let Some(origin) = use_spot(&mut v, m.model, b.abs_mins(), b.abs_maxs()) {
            out.push(Seed {
                origin,
                kind: SeedKind::UseSpot,
            });
        }
    }
    let ladders = ladder_sites(world, &mut v);
    for l in &ladders {
        for end in ladder_ends(&mut v, l) {
            out.push(Seed {
                origin: end,
                kind: SeedKind::LadderEnd,
            });
        }
    }
    MapSites {
        seeds: out,
        ladders,
        teleports,
    }
}

/// Reach of the use key from a player's origin to a button's box, less a margin (`PLAYER_SEARCH_RADIUS` is 64).
const USE_REACH: f32 = 52.0;

/// A standing hull centre on the floor in front of the button with box `mins..maxs`, close enough to press it and
/// seeing it.
fn use_spot(v: &mut WorldView<'_>, model: usize, mins: Vec3, maxs: Vec3) -> Option<Vec3> {
    let center = (mins + maxs) * 0.5;
    let half = (maxs - mins) * 0.5;
    let mut best: Option<(f32, Vec3)> = None;
    for k in 0..8 {
        let angle = k as f32 * std::f32::consts::FRAC_PI_4;
        let dir = Vec2::new(lb_core::dmath::cos(angle), lb_core::dmath::sin(angle));
        let edge = [0, 1]
            .into_iter()
            .filter(|&a| dir[a].abs() > 1e-3)
            .map(|a| half[a] / dir[a].abs())
            .fold(f32::INFINITY, f32::min);
        let at = (center.truncate() + dir * (edge + 24.0)).extend(center.z);
        let tr = v.trace(&TraceQuery::hull(at, at - Vec3::Z * 128.0, HullKind::Stand));
        if tr.start_solid || tr.fraction >= 1.0 || tr.normal.z < 0.7 {
            continue;
        }
        let origin = tr.end;
        let reach = (origin.clamp(mins, maxs) - origin).length();
        if reach >= USE_REACH {
            continue;
        }
        let eye = origin + Vec3::Z * 28.0;
        let sight = v.trace(&TraceQuery::line(eye, center));
        if sight.fraction < 0.99 && sight.hit != Some(model as u32) {
            continue;
        }
        if best.is_none_or(|(r, _)| reach < r) {
            best = Some((reach, origin));
        }
    }
    best.map(|(_, o)| o)
}

/// The climbable face of every ladder: the side of its thin axis where a player fits, found at the lowest height
/// one does (a ladder may start below the floor in front of it, or end at a ledge behind it).
fn ladder_sites(world: &BspWorld, v: &mut WorldView<'_>) -> Vec<LadderSite> {
    let mut out = Vec::new();
    for b in world
        .brushes
        .iter()
        .filter(|b| b.kind == BrushKind::Volume(contents::LADDER))
    {
        let (mins, maxs) = (b.abs_mins(), b.abs_maxs());
        let size = maxs - mins;
        let axis = if size.x <= size.y { 0 } else { 1 };
        let other = 1 - axis;
        let mid_other = (mins[other] + maxs[other]) * 0.5;
        let mut best: Option<(f32, LadderSite)> = None;
        // Rungs are often a solid brush right behind the ladder volume: then stand as far out as still touches it.
        for (side, out) in [1.0f32, -1.0]
            .into_iter()
            .flat_map(|s| [14.0f32, 15.5].map(move |o| (s, o)))
        {
            let face = if side > 0.0 { maxs[axis] } else { mins[axis] };
            let mut spot = Vec2::ZERO;
            spot[axis] = face + side * out;
            spot[other] = mid_other;
            let mut z = mins.z + 36.0;
            while z <= maxs.z + 36.0 {
                let p = spot.extend(z);
                if !v.trace(&TraceQuery::hull(p, p, HullKind::Stand)).start_solid {
                    if best.is_none_or(|(lowest, _)| z < lowest) {
                        let mut normal = Vec3::ZERO;
                        normal[axis] = side;
                        best = Some((
                            z,
                            LadderSite {
                                model: b.model,
                                spot,
                                normal,
                                bottom: mins.z,
                                top: maxs.z,
                            },
                        ));
                    }
                    break;
                }
                z += 8.0;
            }
        }
        out.extend(best.map(|(_, site)| site));
    }
    out
}

/// The ledge at a ladder's top is at most this far below the top of the ladder (which may go on up the wall above
/// it).
pub const TOP_BELOW: f32 = 96.0;

/// Where a player gets off a ladder: the floor it climbs down to at the foot, and the ledge it steps onto at the top.
pub fn ladder_ends(v: &mut WorldView<'_>, l: &LadderSite) -> Vec<Vec3> {
    let mut out = Vec::new();
    // Climb from as low as a player fits on it.
    let mut z = l.bottom + 36.0;
    while z <= l.top + 36.0 {
        let p = l.spot.extend(z);
        if !v.trace(&TraceQuery::hull(p, p, HullKind::Stand)).start_solid {
            let phys = Physics::default();
            if let Some(foot) = simulate_climb(v, &phys, p, l.normal, false)
                && foot.z - 36.0 <= l.bottom + 48.0
            {
                out.push(foot);
            }
            if let Some(top) = simulate_climb(v, &phys, p, l.normal, true)
                && top.z - 36.0 >= l.top - TOP_BELOW
            {
                out.push(top);
            }
            break;
        }
        z += 8.0;
    }
    out
}

/// Damage per half-second tick of a `trigger_hurt` that makes it a place to keep out of; weaker ones (a trickle of
/// radiation) are walked through.
const HARMFUL: f32 = 20.0;

/// Hazards, gates and water, from the world and its mechanisms.
pub struct Site<'a> {
    world: &'a BspWorld,
    /// Damaging triggers that are on when the map starts.
    hurts: Vec<usize>,
    /// Doors and breakables (model, offset at rest) that close a passage until opened or broken.
    gates: Vec<(usize, Vec3)>,
    ladders: Vec<usize>,
}

impl<'a> Site<'a> {
    pub fn new(world: &'a BspWorld, mech: &Mechanisms) -> Site<'a> {
        let hurts = mech
            .triggers
            .iter()
            .filter(|t| {
                t.kind == TriggerKind::Hurt
                    && t.spawnflags & 2 == 0
                    && world.entities[t.entity]
                        .get("dmg")
                        .and_then(|d| d.trim().parse::<f32>().ok())
                        .is_some_and(|d| d >= HARMFUL)
            })
            .map(|t| t.model)
            .collect();
        let mut gates: Vec<(usize, Vec3)> = mech
            .movers
            .iter()
            .filter(|m| {
                matches!(m.kind, MoverKind::Door | MoverKind::RotatingDoor)
                    && world
                        .brush(m.model)
                        .is_some_and(|b| !is_platform(b.mins, b.maxs, (m.active - m.rest).z, m.vertical()))
            })
            .map(|m| (m.model, m.rest))
            .collect();
        // Breakables are gone while the floor is flooded, but stand there until broken.
        gates.extend(
            mech.breakables
                .iter()
                .filter(|b| b.breakable())
                .filter_map(|b| world.brush(b.model).map(|br| (b.model, br.offset))),
        );
        let ladders = world
            .brushes
            .iter()
            .filter(|b| b.kind == BrushKind::Volume(contents::LADDER))
            .map(|b| b.model)
            .collect();
        Site {
            world,
            hurts,
            gates,
            ladders,
        }
    }
}

impl Surroundings for Site<'_> {
    fn hazard(&self, origin: Vec3) -> bool {
        let feet = origin - Vec3::Z * 34.0;
        harmful_contents(self.world.point_contents_shared(feet))
            || harmful_contents(self.world.point_contents_shared(origin))
            || self
                .hurts
                .iter()
                .any(|&m| self.world.hull_overlaps(m, Vec3::ZERO, origin, HullKind::Stand))
    }

    fn gate(&self, origin: Vec3) -> bool {
        self.gates
            .iter()
            .any(|&(m, rest)| self.world.hull_overlaps(m, rest, origin, HullKind::Stand))
    }

    fn water(&self, origin: Vec3) -> bool {
        let c = self.world.point_contents_shared(origin);
        c <= contents::WATER && c > contents::TRANSLUCENT && !harmful_contents(c)
    }

    fn push(&self, origin: Vec3) -> bool {
        self.world.push_at(origin, HullKind::Stand) != Vec3::ZERO
    }

    fn ladder(&self, origin: Vec3) -> bool {
        self.ladders
            .iter()
            .any(|&m| self.world.hull_overlaps(m, Vec3::ZERO, origin, HullKind::Stand))
    }
}
