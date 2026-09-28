//! Tactics of a map, worked out once from its graph and geometry:
//! - who sees whom from where ([`VisTable`]);
//! - where players pass: the share of shortest ways between spawns, items and places spread over the map that go
//!   through each node (flow);
//! - how wide the way is at each node, how much traffic it sees far and near;
//! - chokepoints: narrow places with much traffic;
//! - spots to hold: overwatch (long sightlines over traffic, little in close) and ambush (out of the way, close to a
//!   chokepoint), each with the directions worth watching;
//! - walls to set tripmines on, the beam across a busy corridor or just behind a turn (yapb's
//!   `checkCornerTripminePlant`), never near where players spawn.

use std::time::Instant;

use lb_bsp::{BspWorld, MapVis};
use lb_core::{Vec2, Vec3, dmath};
use lb_nav::{NavGraph, NodeFlags};
use lb_nav_api::{CampKind, CampSpot, MineSpot, NodeId};
use lb_worldq::TraceQuery;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::grid::NodeGrid;
use crate::paths;
use crate::vis::VisTable;

/// Bump whenever the same graph and map would give other tactics.
pub const VERSION: u32 = 1;

/// Eye above the player origin, standing and crouched.
const EYE_STAND: f32 = 28.0;
const EYE_CROUCH: f32 = 12.0;
/// Floor below the player origin, standing and crouched.
const FLOOR_STAND: f32 = 36.0;
const FLOOR_CROUCH: f32 = 18.0;
/// Traffic this far counts as sighted from afar (sniping), closer as exposure.
const FAR: [f32; 2] = [700.0, 3000.0];
const WIDTH_REACH: f32 = 512.0;
/// Ends of the ways players are taken to walk: spawns, items and every n-th node for about this many.
const FLOW_SAMPLES: usize = 48;
const CHOKE_FLOW: f32 = 0.3;
const CHOKE_WIDTH: f32 = 192.0;
const CHOKE_SPACING: f32 = 256.0;
/// An overwatch spot needs this much far traffic in sight.
const OVERWATCH_SIGHT: f32 = 0.15;
const OVERWATCH_SPACING: f32 = 384.0;
/// Watch directions come from nodes at least this far.
const WATCH_FROM: f32 = 400.0;
/// An ambush spot is this far from the chokepoint it watches.
const AMBUSH_RANGE: [f32; 2] = [150.0, 650.0];
const AMBUSH_SPACING: f32 = 256.0;
const CAMPS: usize = 12;
/// No spots to hold or mine this close to where players spawn.
const SPAWN_CLEAR: f32 = 256.0;
/// A tripmine's beam this high above the floor trips standing and crouching players alike.
const BEAM_HEIGHT: f32 = 20.0;
/// The game puts a tripmine where the aim meets a wall within 128 units of the eye.
const MINE_REACH: f32 = 120.0;
const MINE_FLOW: f32 = 0.25;
/// A corridor this wide at most gets a beam across it.
const MINE_WIDTH: f32 = 320.0;
const MINE_SPACING: f32 = 128.0;
const MINES: usize = 48;
/// A corner mine goes on the wall this far past the turn, the beam at most this long.
const CORNER_PAST: f32 = 45.0;
const CORNER_PROBE: f32 = 80.0;
const CORNER_BEAM: f32 = 250.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SpotStats {
    /// Narrowest width of the way, units, and the yaw it is measured along.
    pub width: f32,
    pub across: f32,
    /// Traffic in sight far away, traffic in sight close by, share of the map in sight; 0..1.
    pub sight: f32,
    pub near: f32,
    pub seen: f32,
    /// How many 45° sectors the ways out of the node go to.
    pub exits: u8,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TacticsStats {
    pub millis: u64,
    pub traces: u64,
    /// Node pairs that see each other.
    pub pairs: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MapTactics {
    pub origins: Vec<Vec3>,
    pub transit: Vec<bool>,
    pub vis: VisTable,
    pub flow: Vec<f32>,
    pub spots: Vec<SpotStats>,
    pub chokes: Vec<NodeId>,
    pub camps: Vec<CampSpot>,
    pub mines: Vec<MineSpot>,
    pub stats: TacticsStats,
    #[serde(skip)]
    pub grid: NodeGrid,
}

fn transit(flags: NodeFlags) -> bool {
    flags.intersects(NodeFlags::LADDER | NodeFlags::AIRBORNE | NodeFlags::WATER | NodeFlags::ON_MOVER)
}

fn yaw_of(d: Vec3) -> f32 {
    dmath::atan2(d.y, d.x).to_degrees()
}

fn dir_of(yaw: f32) -> Vec3 {
    let (s, c) = dmath::sin_cos(yaw.to_radians());
    Vec3::new(c, s, 0.0)
}

impl MapTactics {
    /// Works the tactics out; the traces run on the rayon pool the caller installs.
    pub fn build(graph: &NavGraph, world: &BspWorld, pvs: &MapVis, spawns: &[Vec3], items: &[Vec3]) -> MapTactics {
        let started = Instant::now();
        let n = graph.len();
        let origins: Vec<Vec3> = graph.nodes.iter().map(|nd| nd.origin).collect();
        let transit: Vec<bool> = graph.nodes.iter().map(|nd| transit(nd.flags)).collect();
        let crouch: Vec<bool> = graph
            .nodes
            .iter()
            .map(|nd| nd.flags.contains(NodeFlags::CROUCH))
            .collect();
        let eyes: Vec<Vec3> = (0..n)
            .map(|i| origins[i] + Vec3::Z * if crouch[i] { EYE_CROUCH } else { EYE_STAND })
            .collect();
        let leaves: Vec<u32> = eyes.iter().map(|e| pvs.leaf_at(*e)).collect();
        let sight = |a: Vec3, b: Vec3| {
            let mut q = TraceQuery::line(a, b);
            q.ignore_glass = true;
            let tr = world.trace_shared(&q);
            !tr.start_solid && tr.fraction >= 0.999
        };
        let (vis, mut traces) = VisTable::build(&eyes, &|a, b| pvs.pvs(leaves[a], leaves[b]), &sight);
        let grid = NodeGrid::build(&origins);
        let mut ends: Vec<NodeId> = spawns
            .iter()
            .chain(items)
            .filter_map(|p| grid.nearest(&origins, *p, 256.0, |i| !transit[i as usize]))
            .collect();
        ends.extend(
            (0..n)
                .step_by((n / FLOW_SAMPLES).max(1))
                .filter(|&i| !transit[i])
                .map(|i| i as NodeId),
        );
        ends.sort_unstable();
        ends.dedup();
        let flow = flow(graph, &ends);
        let widths: Vec<(f32, f32)> = (0..n)
            .into_par_iter()
            .map(|i| {
                if transit[i] {
                    return (f32::INFINITY, 0.0);
                }
                let o = origins[i];
                let d: [f32; 8] = std::array::from_fn(|k| {
                    let to = o + dir_of(k as f32 * 45.0) * WIDTH_REACH;
                    world.trace_shared(&TraceQuery::line(o, to)).fraction * WIDTH_REACH
                });
                (0..4)
                    .map(|k| (d[k] + d[k + 4], k as f32 * 45.0))
                    .min_by(|a, b| a.0.total_cmp(&b.0))
                    .unwrap_or((f32::INFINITY, 0.0))
            })
            .collect();
        traces += 8 * transit.iter().filter(|t| !**t).count() as u64;
        let mut spots: Vec<SpotStats> = (0..n)
            .map(|i| {
                let (mut far, mut near, mut seen) = (0.0f32, 0.0f32, 0u32);
                for m in vis.seen_from(i as NodeId) {
                    let d = origins[i].distance(origins[m as usize]);
                    seen += 1;
                    if d < FAR[0] {
                        near += flow[m as usize];
                    } else if d <= FAR[1] {
                        far += flow[m as usize];
                    }
                }
                let mut sectors = 0u8;
                for l in graph.links(i as NodeId).iter().filter(|l| l.valid()) {
                    let d = origins[l.to as usize] - origins[i];
                    let sector = (((yaw_of(d) + 382.5) / 45.0) as usize) % 8;
                    sectors |= 1 << sector;
                }
                SpotStats {
                    width: widths[i].0,
                    across: widths[i].1,
                    sight: far,
                    near,
                    seen: seen as f32 / n.max(1) as f32,
                    exits: sectors.count_ones() as u8,
                }
            })
            .collect();
        let max_sight = spots.iter().map(|s| s.sight).fold(1e-6, f32::max);
        let max_near = spots.iter().map(|s| s.near).fold(1e-6, f32::max);
        for s in &mut spots {
            s.sight /= max_sight;
            s.near /= max_near;
        }
        let near_spawn = |p: Vec3| spawns.iter().any(|s| s.distance(p) < SPAWN_CLEAR);
        let mut t = MapTactics {
            origins,
            transit,
            vis,
            flow,
            spots,
            chokes: Vec::new(),
            camps: Vec::new(),
            mines: Vec::new(),
            stats: TacticsStats::default(),
            grid,
        };
        t.chokes = t.find_chokes();
        t.camps = t.overwatch(&near_spawn);
        t.camps.extend(t.ambushes(&near_spawn));
        let floors: Vec<f32> = (0..n)
            .map(|i| t.origins[i].z - if crouch[i] { FLOOR_CROUCH } else { FLOOR_STAND })
            .collect();
        let (mines, mine_traces) = t.find_mines(graph, world, &floors, &eyes, &near_spawn);
        t.mines = mines;
        t.stats = TacticsStats {
            millis: started.elapsed().as_millis() as u64,
            traces: traces + mine_traces,
            pairs: t.vis.pairs(),
        };
        t
    }

    /// After reading the tactics back from a file.
    pub fn index(&mut self) {
        self.grid = NodeGrid::build(&self.origins);
    }

    pub fn nearest(&self, p: Vec3, max: f32) -> Option<NodeId> {
        self.grid.nearest(&self.origins, p, max, |_| true)
    }

    /// The nearest node a player can stand at (not a ladder, water, mid-air or a lift).
    pub fn nearest_standing(&self, p: Vec3, max: f32) -> Option<NodeId> {
        self.grid.nearest(&self.origins, p, max, |n| !self.transit[n as usize])
    }

    fn find_chokes(&self) -> Vec<NodeId> {
        let mut cands: Vec<NodeId> = (0..self.origins.len() as NodeId)
            .filter(|&i| {
                let i = i as usize;
                !self.transit[i] && self.flow[i] >= CHOKE_FLOW && self.spots[i].width <= CHOKE_WIDTH
            })
            .collect();
        cands.sort_by(|&a, &b| self.flow[b as usize].total_cmp(&self.flow[a as usize]).then(a.cmp(&b)));
        let mut out: Vec<NodeId> = Vec::new();
        for c in cands {
            let p = self.origins[c as usize];
            if out
                .iter()
                .all(|&o| self.origins[o as usize].distance(p) >= CHOKE_SPACING)
            {
                out.push(c);
            }
        }
        out
    }

    /// Directions worth watching from `n`: the peaks of traffic in sight at least `WATCH_FROM` away, by yaw; the
    /// pitch and distance of what is watched.
    fn watch_from(&self, n: NodeId) -> ([f32; 2], f32, f32) {
        let o = self.origins[n as usize];
        let mut bins = [0.0f32; 36];
        let (mut sum, mut dist, mut dz) = (0.0f32, 0.0f32, 0.0f32);
        for m in self.vis.seen_from(n) {
            let d = self.origins[m as usize] - o;
            let len = d.length();
            if len < WATCH_FROM {
                continue;
            }
            let w = 0.05 + self.flow[m as usize];
            let bin = (((yaw_of(d) + 360.0) / 10.0) as usize) % 36;
            bins[bin] += w;
            sum += w;
            dist += w * len;
            dz += w * d.z;
        }
        if sum <= 0.0 {
            return ([0.0; 2], 0.0, 0.0);
        }
        let smooth: [f32; 36] = std::array::from_fn(|i| bins[(i + 35) % 36] * 0.5 + bins[i] + bins[(i + 1) % 36] * 0.5);
        let best = (0..36).max_by(|&a, &b| smooth[a].total_cmp(&smooth[b])).unwrap_or(0);
        let apart = |i: usize| {
            let d = (i as i32 - best as i32).rem_euclid(36);
            d.min(36 - d) >= 4
        };
        let second = (0..36)
            .filter(|&i| apart(i) && smooth[i] >= 0.5 * smooth[best])
            .max_by(|&a, &b| smooth[a].total_cmp(&smooth[b]))
            .unwrap_or(best);
        let yaw = |bin: usize| lb_core::math::normalize_angle(bin as f32 * 10.0 + 5.0);
        let range = dist / sum;
        let pitch = -dmath::atan2(dz / sum, range).to_degrees();
        ([yaw(best), yaw(second)], pitch, range)
    }

    fn overwatch(&self, near_spawn: &dyn Fn(Vec3) -> bool) -> Vec<CampSpot> {
        let score = |i: usize| {
            let s = &self.spots[i];
            s.sight - 0.6 * s.near - 0.4 * self.flow[i]
        };
        let mut cands: Vec<usize> = (0..self.origins.len())
            .filter(|&i| {
                !self.transit[i]
                    && self.spots[i].sight >= OVERWATCH_SIGHT
                    && self.spots[i].exits >= 1
                    && !near_spawn(self.origins[i])
            })
            .collect();
        cands.sort_by(|&a, &b| score(b).total_cmp(&score(a)).then(a.cmp(&b)));
        let mut out: Vec<CampSpot> = Vec::new();
        for i in cands {
            if out.len() == CAMPS {
                break;
            }
            let p = self.origins[i];
            if out.iter().any(|c| c.pos.distance(p) < OVERWATCH_SPACING) || score(i) <= 0.0 {
                continue;
            }
            let (watch, pitch, range) = self.watch_from(i as NodeId);
            out.push(CampSpot {
                node: i as NodeId,
                pos: p,
                kind: CampKind::Overwatch,
                watch,
                pitch,
                range,
                score: score(i),
                guards: None,
            });
        }
        let top = out.first().map_or(1.0, |c| c.score.max(1e-6));
        for c in &mut out {
            c.score /= top;
        }
        out
    }

    fn ambushes(&self, near_spawn: &dyn Fn(Vec3) -> bool) -> Vec<CampSpot> {
        let mut found: Vec<CampSpot> = Vec::new();
        for &c in &self.chokes {
            let at = self.origins[c as usize];
            let fc = self.flow[c as usize];
            let best = self
                .vis
                .seen_from(c)
                .filter(|&m| {
                    let m = m as usize;
                    let d = self.origins[m].distance(at);
                    !self.transit[m]
                        && (AMBUSH_RANGE[0]..=AMBUSH_RANGE[1]).contains(&d)
                        && self.flow[m] <= 0.6 * fc
                        && !near_spawn(self.origins[m])
                })
                .map(|m| {
                    let s = &self.spots[m as usize];
                    (m, fc * (1.0 - s.seen) * (1.0 - 0.5 * s.near))
                })
                .max_by(|a, b| a.1.total_cmp(&b.1).then(b.0.cmp(&a.0)));
            if let Some((m, score)) = best {
                let p = self.origins[m as usize];
                let d = at - p;
                let yaw = yaw_of(d);
                found.push(CampSpot {
                    node: m,
                    pos: p,
                    kind: CampKind::Ambush,
                    watch: [yaw, yaw],
                    pitch: -dmath::atan2(d.z, d.truncate().length().max(1.0)).to_degrees(),
                    range: d.length(),
                    score,
                    guards: Some(c),
                });
            }
        }
        found.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.node.cmp(&b.node)));
        let mut out: Vec<CampSpot> = Vec::new();
        for c in found {
            if out.len() < CAMPS && out.iter().all(|o| o.pos.distance(c.pos) >= AMBUSH_SPACING) {
                out.push(c);
            }
        }
        let top = out.first().map_or(1.0, |c| c.score.max(1e-6));
        for c in &mut out {
            c.score /= top;
        }
        out
    }

    fn find_mines(
        &self,
        graph: &NavGraph,
        world: &BspWorld,
        floors: &[f32],
        eyes: &[Vec3],
        near_spawn: &(dyn Fn(Vec3) -> bool + Sync),
    ) -> (Vec<MineSpot>, u64) {
        let n = self.origins.len();
        // Ways into each node, for the turns taken at it.
        let mut into: Vec<Vec<NodeId>> = vec![Vec::new(); n];
        for a in 0..n as NodeId {
            for l in graph.links(a).iter().filter(|l| l.valid() && l.kind.is_walk()) {
                into[l.to as usize].push(a);
            }
        }
        let found: Vec<(Vec<MineSpot>, u64)> = (0..n)
            .into_par_iter()
            .map(|i| {
                if self.transit[i] || self.flow[i] < MINE_FLOW {
                    return (Vec::new(), 0);
                }
                let mut spots: Vec<((Vec3, Vec3, Vec3), bool)> = Vec::new();
                let mut traces = 0u64;
                let o = self.origins[i];
                let beam_z = floors[i] + BEAM_HEIGHT;
                let mut tr = |q: &TraceQuery| {
                    traces += 1;
                    world.trace_shared(q)
                };
                let width = self.spots[i].width;
                if width <= MINE_WIDTH {
                    let across = dir_of(self.spots[i].across);
                    for side in [across, -across] {
                        let from = Vec3::new(o.x, o.y, beam_z);
                        if let Some(s) = wall_mine(&mut tr, from, side, MINE_REACH, (width + 48.0).min(400.0)) {
                            spots.push((s, false));
                        }
                    }
                }
                let outs: Vec<NodeId> = graph
                    .links(i as NodeId)
                    .iter()
                    .filter(|l| l.valid() && l.kind.is_walk())
                    .map(|l| l.to)
                    .collect();
                for &a in &into[i] {
                    for &b in &outs {
                        if a == b || self.flow[a as usize].min(self.flow[b as usize]) < 0.5 * self.flow[i] {
                            continue;
                        }
                        let incoming = (o - self.origins[a as usize]).truncate().normalize_or_zero();
                        let outgoing = (self.origins[b as usize] - o).truncate().normalize_or_zero();
                        if incoming.dot(outgoing) > 0.5 || self.vis.get(a, b) {
                            continue;
                        }
                        let turn = incoming.perp_dot(outgoing);
                        if turn.abs() < 1e-3 {
                            continue;
                        }
                        let inner = if turn > 0.0 {
                            Vec2::new(-outgoing.y, outgoing.x)
                        } else {
                            Vec2::new(outgoing.y, -outgoing.x)
                        };
                        let past = o + outgoing.extend(0.0) * CORNER_PAST;
                        let from = Vec3::new(past.x, past.y, beam_z);
                        if let Some(s) = wall_mine(&mut tr, from, inner.extend(0.0), CORNER_PROBE, CORNER_BEAM) {
                            spots.push((s, true));
                        }
                    }
                }
                let eye = eyes[i];
                let out = spots
                    .into_iter()
                    .filter(|(s, _)| eye.distance(s.0) <= MINE_REACH && !near_spawn(s.0) && !near_spawn(o))
                    .map(|((wall, normal, beam_end), corner)| MineSpot {
                        node: i as NodeId,
                        stand: o,
                        wall,
                        normal,
                        beam_end,
                        flow: self.flow[i],
                        corner,
                    })
                    .collect();
                (out, traces)
            })
            .collect();
        let traces = found.iter().map(|f| f.1).sum();
        let mut all: Vec<MineSpot> = found.into_iter().flat_map(|f| f.0).collect();
        all.sort_by(|a, b| {
            b.flow
                .total_cmp(&a.flow)
                .then(b.corner.cmp(&a.corner))
                .then(a.node.cmp(&b.node))
        });
        let mut out: Vec<MineSpot> = Vec::new();
        for m in all {
            if out.len() < MINES && out.iter().all(|o| o.wall.distance(m.wall) >= MINE_SPACING) {
                out.push(m);
            }
        }
        (out, traces)
    }
}

/// A wall within `reach` of `from` along `dir` that holds a tripmine whose beam crosses the way: upright, and the
/// beam along its normal meets the other side within `beam_max`. Returns the wall point, its normal and where the
/// beam ends.
fn wall_mine(
    tr: &mut dyn FnMut(&TraceQuery) -> lb_worldq::Trace,
    from: Vec3,
    dir: Vec3,
    reach: f32,
    beam_max: f32,
) -> Option<(Vec3, Vec3, Vec3)> {
    let hit = tr(&TraceQuery::line(from, from + dir * reach));
    if hit.start_solid || hit.fraction >= 1.0 || hit.normal.z.abs() > 0.3 || hit.normal.dot(-dir) < 0.7 {
        return None;
    }
    let start = hit.end + hit.normal * 8.0;
    let beam = tr(&TraceQuery::line(start, start + hit.normal * 2048.0));
    let len = beam.fraction * 2048.0;
    if beam.start_solid || beam.fraction >= 1.0 || !(48.0..=beam_max).contains(&len) {
        return None;
    }
    Some((hit.end, hit.normal, beam.end))
}

/// Flow: the share of the shortest ways between `ends` passing through each node, square-rooted so side ways do not
/// all sit at zero, 1 at the busiest node.
fn flow(graph: &NavGraph, ends: &[NodeId]) -> Vec<f32> {
    let n = graph.len();
    let counts = ends
        .par_iter()
        .map(|&s| {
            let t = paths::tree(graph, s, f32::INFINITY);
            let mut c = vec![0u32; n];
            for &e in ends {
                if e == s || !t.cost[e as usize].is_finite() {
                    continue;
                }
                let mut m = t.pred[e as usize];
                while m != paths::NONE && m != s {
                    c[m as usize] += 1;
                    m = t.pred[m as usize];
                }
            }
            c
        })
        .reduce(
            || vec![0u32; n],
            |mut a, b| {
                for (x, y) in a.iter_mut().zip(b) {
                    *x += y;
                }
                a
            },
        );
    let max = counts.iter().copied().max().unwrap_or(0).max(1) as f32;
    counts.iter().map(|&c| (c as f32 / max).sqrt()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use lb_bsp::mech::Mechanisms;

    /// A server that read its graph back from the cache must work out the same tactics as one that just made it (a
    /// replay makes the graph again): making it moves doors and lifts about, which must not show.
    #[test]
    fn tactics_do_not_depend_on_where_the_graph_came_from() {
        let Some(maps) = lb_bsp::test_maps_dir() else { return };
        let Ok(bsp) = std::fs::read(maps.join("crossfire.bsp")) else {
            return;
        };
        let mut made = BspWorld::load(&bsp).unwrap();
        let mech = Mechanisms::from_world(&made);
        let generated = lb_navgen::generate(&mut made, &mech, &lb_navgen::GenOptions::default(), "test");
        let mut read = BspWorld::load(&bsp).unwrap();
        let vis = MapVis::build(&read.bsp);
        let t = |world: &mut BspWorld| {
            lb_navgen::site::rest_poses(world, &mech);
            MapTactics::build(&generated.graph, world, &vis, &[], &[])
        };
        let (a, b) = (t(&mut made), t(&mut read));
        assert_eq!(a.stats.pairs, b.stats.pairs);
        assert_eq!(a.vis, b.vis);
    }
}
