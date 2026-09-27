//! Item spots: a bot learns whether an item is there only by looking at its spot. The spot must be within view
//! range, in the PVS and in the view frustum, with a clear line to a point 8 units above it; a few spots are
//! checked per look, taking turns.

use lb_core::Vec3;
use lb_core::time::SimTime;
use lb_game::items::ItemKind;
use lb_knowledge::Items;
use lb_worldq::{TraceQuery, Tracer, VisSets};

use crate::vision::{Frustum, Viewer};

/// Traces per look spent on item spots.
pub const ITEM_TRACES: u32 = 2;
const MATCH_FLAT: f32 = 32.0;
const MATCH_HEIGHT: f32 = 64.0;

/// An item entity as the server has it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ItemEntity {
    pub origin: Vec3,
    pub kind: ItemKind,
    /// Not taken (drawn, no owner).
    pub drawn: bool,
}

/// Looks at up to [`ITEM_TRACES`] spots in view, starting after the one checked last.
#[allow(clippy::too_many_arguments)]
pub fn look(
    now: SimTime,
    viewer: &Viewer,
    items: &mut Items,
    entities: &[ItemEntity],
    range: f32,
    vis: &dyn VisSets,
    tracer: &mut dyn Tracer,
    cursor: &mut usize,
) -> u32 {
    let n = items.spots.len();
    if n == 0 {
        return 0;
    }
    let frustum = Frustum::new(viewer.eye, viewer.angles, viewer.fov, viewer.aspect);
    let mut traces = 0;
    for k in 0..n {
        if traces >= ITEM_TRACES {
            break;
        }
        let i = (*cursor + 1 + k) % n;
        let spot = items.spots[i];
        let point = spot.origin + Vec3::Z * 8.0;
        if point.distance(viewer.eye) > range
            || !frustum.contains(point)
            || !vis.box_in_pvs(
                viewer.eye,
                spot.origin - Vec3::splat(8.0),
                spot.origin + Vec3::splat(8.0),
            )
        {
            continue;
        }
        traces += 1;
        *cursor = i;
        let tr = tracer.trace(&TraceQuery::sight(viewer.eye, point, viewer.index));
        if tr.fraction < 1.0 && tr.end.distance(point) > 16.0 {
            continue;
        }
        let present = entities.iter().any(|e| {
            e.drawn
                && e.kind == spot.kind
                && (e.origin - spot.origin).truncate().length() < MATCH_FLAT
                && (e.origin.z - spot.origin.z).abs() < MATCH_HEIGHT
        });
        items.observe(i, present, now);
    }
    traces
}
