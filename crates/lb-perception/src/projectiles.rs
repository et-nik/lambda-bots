//! Projectiles and placed explosives: seen like item spots, within a range per kind (a rocket's glow carries far, a
//! satchel on the floor does not), in the PVS and the view frustum, with a clear line to it. A few traces per look,
//! nearest first. The bot knows its own satchels and mines as its own. A tripmine in view shows its beam: the line
//! along the way it faces up to the first wall, traced once per mine.

use lb_core::Vec3;
use lb_core::dmath;
use lb_core::time::SimTime;
use lb_game::entities::ProjectileKind;
use lb_knowledge::{Explosives, ProjectileSighting};
use lb_worldq::{TraceQuery, Tracer, VisSets};

use crate::vision::{Frustum, Viewer};

/// Traces per look spent on projectiles.
pub const PROJECTILE_TRACES: u32 = 4;
const BEAM_LENGTH: f32 = 2048.0;
/// A mine already known this close does not need its beam traced again.
const KNOWN_MINE: f32 = 24.0;

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
}

/// Direction entity `angles` face (pitch up positive, as `UTIL_VecToAngles` makes them).
pub fn facing(angles: Vec3) -> Vec3 {
    let (sp, cp) = dmath::sin_cos(angles.x.to_radians());
    let (sy, cy) = dmath::sin_cos(angles.y.to_radians());
    Vec3::new(cp * cy, cp * sy, sp)
}

/// Looks at the projectiles in view, nearest first; returns the traces spent.
pub fn look(
    now: SimTime,
    viewer: &Viewer,
    entities: &[ProjectileEntity],
    known: &Explosives,
    vis: &dyn VisSets,
    tracer: &mut dyn Tracer,
    out: &mut Vec<ProjectileSighting>,
) -> u32 {
    let frustum = Frustum::new(viewer.eye, viewer.angles, viewer.fov, viewer.aspect);
    let mut order: Vec<(f32, &ProjectileEntity)> = entities
        .iter()
        .map(|e| (e.origin.distance(viewer.eye), e))
        .filter(|(d, e)| *d <= e.kind.view_range())
        .collect();
    order.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut traces = 0;
    for (_, e) in order {
        if traces >= PROJECTILE_TRACES {
            break;
        }
        let own = e.owner == viewer.index;
        if !frustum.contains(e.origin)
            || !vis.box_in_pvs(viewer.eye, e.origin - Vec3::splat(4.0), e.origin + Vec3::splat(4.0))
        {
            continue;
        }
        traces += 1;
        let tr = tracer.trace(&TraceQuery::sight(viewer.eye, e.origin, viewer.index));
        if tr.fraction < 1.0 && tr.end.distance(e.origin) > 12.0 {
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
        });
    }
    traces
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
            },
            ProjectileEntity {
                index: 71,
                kind: ProjectileKind::Satchel,
                origin: Vec3::new(1500.0, 0.0, 0.0),
                velocity: Vec3::ZERO,
                angles: Vec3::ZERO,
                owner: 1,
            },
            ProjectileEntity {
                index: 72,
                kind: ProjectileKind::Tripmine,
                origin: Vec3::new(400.0, 100.0, 0.0),
                velocity: Vec3::ZERO,
                angles: Vec3::new(0.0, -90.0, 0.0),
                owner: 4,
            },
            ProjectileEntity {
                index: 73,
                kind: ProjectileKind::Grenade,
                origin: Vec3::new(-300.0, 0.0, 0.0),
                velocity: Vec3::ZERO,
                angles: Vec3::ZERO,
                owner: 4,
            },
        ];
        let mut out = Vec::new();
        look(
            SimTime(1.0),
            &viewer(),
            &entities,
            &Explosives::default(),
            &AllVisible,
            &mut Open,
            &mut out,
        );
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
}
