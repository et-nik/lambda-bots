//! Projectiles and placed explosives: seen like item spots, within a range per kind (a rocket's glow carries far, a
//! satchel on the floor does not), in the PVS and the view frustum, with a clear line to it. A few traces per look,
//! nearest first. The bot knows its own satchels and mines as its own. A tripmine in view shows its beam: the line
//! along the way it faces up to the first wall, traced once per mine. A mine the bot knows of, where it would see it
//! and none is there, is gone (one such look per look).

use lb_core::Vec3;
use lb_core::dmath;
use lb_core::time::SimTime;
use lb_game::entities::ProjectileKind;
use lb_knowledge::{Explosives, ProjectileSighting};
use lb_worldq::{TraceQuery, Tracer, VisSets};

use crate::vision::{Frustum, PERIOD, Viewer};

/// Traces per look spent on projectiles.
pub const PROJECTILE_TRACES: u32 = 4;
const BEAM_LENGTH: f32 = 2048.0;
/// A mine already known this close does not need its beam traced again.
const KNOWN_MINE: f32 = 24.0;
/// A mine known for less than this may not be among the server's entities yet: not looked for.
const MISSING_GRACE: f64 = 0.5;

/// A projectile entity as the server has it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProjectileEntity {
    pub index: u16,
    pub kind: ProjectileKind,
    pub origin: Vec3,
    pub velocity: Vec3,
    /// Entity angles (pitch up positive): a tripmine faces along its beam.
    pub angles: Vec3,
    /// Edict index of the player who threw or placed it.
    pub owner: u16,
    /// A tripmine's beam is on: it armed (2.5 s after it was placed), and stopping a bullet now sets it off.
    pub armed: bool,
}

/// Direction entity `angles` face (pitch up positive, as `UTIL_VecToAngles` makes them).
pub fn facing(angles: Vec3) -> Vec3 {
    let (sp, cp) = dmath::sin_cos(angles.x.to_radians());
    let (sy, cy) = dmath::sin_cos(angles.y.to_radians());
    Vec3::new(cp * cy, cp * sy, sp)
}

/// Looks at the projectiles in view, nearest first, into `out`, and for a known mine that is not where it should be,
/// into `missing`; returns the traces spent.
#[allow(clippy::too_many_arguments)]
pub fn look(
    now: SimTime,
    viewer: &Viewer,
    entities: &[ProjectileEntity],
    known: &Explosives,
    vis: &dyn VisSets,
    tracer: &mut dyn Tracer,
    out: &mut Vec<ProjectileSighting>,
    missing: &mut Vec<Vec3>,
) -> u32 {
    let frustum = Frustum::new(viewer.eye, viewer.angles, viewer.fov, viewer.aspect);
    let in_view =
        |p: Vec3| frustum.contains(p) && vis.box_in_pvs(viewer.eye, p - Vec3::splat(4.0), p + Vec3::splat(4.0));
    let mut traces = 0;
    // Looked for one at a time, in turn from look to look.
    let gone: smallvec::SmallVec<[Vec3; 4]> = known
        .mines
        .iter()
        .filter(|m| {
            now.since(m.seen) >= MISSING_GRACE
                && m.pos.distance(viewer.eye) <= ProjectileKind::Tripmine.view_range()
                && !entities
                    .iter()
                    .any(|e| e.kind == ProjectileKind::Tripmine && e.origin.distance(m.pos) <= KNOWN_MINE)
                && in_view(m.pos)
        })
        .map(|m| m.pos)
        .collect();
    if !gone.is_empty() {
        let p = gone[(now.0 / PERIOD) as usize % gone.len()];
        traces += 1;
        if sight_reaches(tracer, viewer, p) {
            missing.push(p);
        }
    }
    let mut order: Vec<(f32, &ProjectileEntity)> = entities
        .iter()
        .map(|e| (e.origin.distance(viewer.eye), e))
        .filter(|(d, e)| *d <= e.kind.view_range())
        .collect();
    order.sort_by(|a, b| a.0.total_cmp(&b.0));
    for (_, e) in order {
        if traces >= PROJECTILE_TRACES {
            break;
        }
        let own = e.owner == viewer.index;
        if !in_view(e.origin) {
            continue;
        }
        traces += 1;
        if !sight_reaches(tracer, viewer, e.origin) {
            continue;
        }
        let beam = if e.kind == ProjectileKind::Tripmine
            && !known
                .mines
                .iter()
                .any(|m| m.pos.distance(e.origin) <= KNOWN_MINE && m.beam_end.is_some())
        {
            let dir = facing(e.angles);
            let tr = tracer.trace(&TraceQuery::line(e.origin, e.origin + dir * BEAM_LENGTH));
            traces += 1;
            Some((dir, tr.end))
        } else {
            None
        };
        out.push(ProjectileSighting {
            t: now,
            kind: e.kind,
            index: e.index,
            pos: e.origin,
            vel: e.velocity,
            own,
            beam,
            armed: e.armed,
        });
    }
    traces
}

/// Nothing stands between the eye and `p`.
fn sight_reaches(tracer: &mut dyn Tracer, viewer: &Viewer, p: Vec3) -> bool {
    let tr = tracer.trace(&TraceQuery::sight(viewer.eye, p, viewer.index));
    tr.fraction >= 1.0 || tr.end.distance(p) <= 12.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use lb_worldq::{AllVisible, Trace, contents};

    struct Open;
    impl Tracer for Open {
        fn trace(&mut self, q: &TraceQuery) -> Trace {
            // A wall at y = -300 stops beams.
            if q.end.y < -300.0 && q.start.y > -300.0 {
                let f = (q.start.y + 300.0) / (q.start.y - q.end.y);
                let mut t = Trace::clear(q.start + (q.end - q.start) * f);
                t.fraction = f;
                return t;
            }
            Trace::clear(q.end)
        }
        fn point_contents(&mut self, _p: Vec3) -> i32 {
            contents::EMPTY
        }
    }

    fn viewer() -> Viewer {
        Viewer {
            index: 1,
            eye: Vec3::ZERO,
            angles: Vec3::ZERO,
            fov: 0.0,
            aspect: 16.0 / 9.0,
            head_in_water: false,
            team: 0,
        }
    }

    #[test]
    fn sees_what_is_in_view_and_in_range_and_traces_mine_beams() {
        let entities = [
            ProjectileEntity {
                index: 70,
                kind: ProjectileKind::Rocket,
                origin: Vec3::new(2500.0, 0.0, 0.0),
                velocity: Vec3::new(-2000.0, 0.0, 0.0),
                angles: Vec3::ZERO,
                owner: 3,
                armed: false,
            },
            ProjectileEntity {
                index: 71,
                kind: ProjectileKind::Satchel,
                origin: Vec3::new(1500.0, 0.0, 0.0),
                velocity: Vec3::ZERO,
                angles: Vec3::ZERO,
                owner: 1,
                armed: false,
            },
            ProjectileEntity {
                index: 72,
                kind: ProjectileKind::Tripmine,
                origin: Vec3::new(400.0, 100.0, 0.0),
                velocity: Vec3::ZERO,
                angles: Vec3::new(0.0, -90.0, 0.0),
                owner: 4,
                armed: false,
            },
            ProjectileEntity {
                index: 73,
                kind: ProjectileKind::Grenade,
                origin: Vec3::new(-300.0, 0.0, 0.0),
                velocity: Vec3::ZERO,
                angles: Vec3::ZERO,
                owner: 4,
                armed: false,
            },
        ];
        let mut out = Vec::new();
        let mut missing = Vec::new();
        look(
            SimTime(1.0),
            &viewer(),
            &entities,
            &Explosives::default(),
            &AllVisible,
            &mut Open,
            &mut out,
            &mut missing,
        );
        assert!(missing.is_empty());
        let kinds: Vec<ProjectileKind> = out.iter().map(|s| s.kind).collect();
        assert_eq!(
            kinds,
            [ProjectileKind::Tripmine, ProjectileKind::Rocket],
            "the satchel is too far, the grenade behind"
        );
        let (dir, end) = out[0].beam.unwrap();
        assert!((dir - Vec3::NEG_Y).length() < 1e-4);
        assert!((end.y + 300.0).abs() < 1e-3, "the beam ends at the wall: {end}");
    }

    #[test]
    fn a_known_mine_not_where_it_should_be_is_missing() {
        let mut known = Explosives::default();
        known.placed_mine(Vec3::new(300.0, 0.0, -28.0), Vec3::Z, SimTime(1.0));
        known.placed_mine(Vec3::new(500.0, 50.0, -28.0), Vec3::Z, SimTime(1.0));
        known.placed_mine(Vec3::new(-300.0, 0.0, -28.0), Vec3::Z, SimTime(1.0));
        let there = [ProjectileEntity {
            index: 80,
            kind: ProjectileKind::Tripmine,
            origin: Vec3::new(503.0, 50.0, -28.0),
            velocity: Vec3::ZERO,
            angles: Vec3::new(90.0, 0.0, 0.0),
            owner: 0,
            armed: true,
        }];
        let look_at = |t: f64| {
            let (mut out, mut missing) = (Vec::new(), Vec::new());
            look(
                SimTime(t),
                &viewer(),
                &there,
                &known,
                &AllVisible,
                &mut Open,
                &mut out,
                &mut missing,
            );
            missing
        };
        assert!(look_at(1.2).is_empty(), "just placed: maybe not listed yet");
        assert_eq!(
            look_at(3.0),
            [Vec3::new(300.0, 0.0, -28.0)],
            "the one in view with none there; the one behind is not looked at"
        );
    }
}
