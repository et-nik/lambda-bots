//! How much of a map the graph covers: which nodes a player can get to from the spawn points and back, how much of
//! the floor belongs to them, which items can be fetched, and where the floor is cut off.

use std::collections::VecDeque;

use lb_bsp::BspWorld;
use lb_core::Vec3;
use lb_nav::NavGraph;
use serde::Serialize;

use crate::build::Generated;
use crate::field::{NONE, SpanFlags};

#[derive(Clone, Debug, Default, Serialize)]
pub struct Coverage {
    /// Spans of floor a player can stand on.
    pub spans: usize,
    /// Of them, spans whose node can be reached from a spawn point and left back to one.
    pub covered: usize,
    pub nodes: usize,
    /// Nodes reachable from a spawn point, and of them those from which a spawn point can be reached again.
    pub reachable: usize,
    pub roundtrip: usize,
    pub spawns: usize,
    pub items: usize,
    /// Items that can be fetched and left again.
    pub items_ok: usize,
    /// Floor not covered, by the node it belongs to: (node origin, spans), largest first.
    pub cut_off: Vec<(Vec3, usize)>,
    /// Per node: reachable from a spawn point, and a spawn point reachable from it.
    #[serde(skip)]
    pub from_spawns: Vec<bool>,
    #[serde(skip)]
    pub to_spawns: Vec<bool>,
    /// Per node: both.
    #[serde(skip)]
    pub roundtrip_nodes: Vec<bool>,
}

impl Coverage {
    pub fn ratio(&self) -> f64 {
        self.covered as f64 / self.spans.max(1) as f64
    }
}

fn walk(g: &NavGraph, starts: &[u32], backward: bool) -> Vec<bool> {
    let mut rev: Vec<Vec<u32>> = Vec::new();
    if backward {
        rev = vec![Vec::new(); g.len()];
        for a in 0..g.len() as u32 {
            for l in g.links(a).iter().filter(|l| l.valid()) {
                rev[l.to as usize].push(a);
            }
        }
    }
    let mut seen = vec![false; g.len()];
    let mut queue: VecDeque<u32> = starts.iter().copied().collect();
    for &s in starts {
        seen[s as usize] = true;
    }
    while let Some(n) = queue.pop_front() {
        let next: Vec<u32> = if backward {
            rev[n as usize].clone()
        } else {
            g.links(n).iter().filter(|l| l.valid()).map(|l| l.to).collect()
        };
        for m in next {
            if !seen[m as usize] {
                seen[m as usize] = true;
                queue.push_back(m);
            }
        }
    }
    seen
}

pub fn coverage(generated: &Generated) -> Coverage {
    let g = &generated.graph;
    let fwd = walk(g, &generated.spawns, false);
    let back = walk(g, &generated.spawns, true);
    let ok = |n: u32| fwd[n as usize] && back[n as usize];
    let mut c = Coverage {
        nodes: g.len(),
        reachable: fwd.iter().filter(|x| **x).count(),
        roundtrip: (0..g.len() as u32).filter(|&n| ok(n)).count(),
        spawns: generated.spawns.len(),
        items: generated.items.len(),
        items_ok: generated.items.iter().filter(|&&n| ok(n)).count(),
        ..Coverage::default()
    };
    let mut lost: rustc_hash::FxHashMap<u32, usize> = rustc_hash::FxHashMap::default();
    for (s, span) in generated.field.spans.iter().enumerate() {
        if span.flags.contains(SpanFlags::HAZARD) {
            continue;
        }
        c.spans += 1;
        let o = generated.owner[s];
        if o != NONE && ok(o) {
            c.covered += 1;
        } else if o != NONE {
            *lost.entry(o).or_default() += 1;
        }
    }
    let mut cut: Vec<(Vec3, usize)> = lost.into_iter().map(|(n, k)| (g.node(n).origin, k)).collect();
    cut.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.x.total_cmp(&b.0.x)));
    c.cut_off = cut;
    c.roundtrip_nodes = (0..g.len() as u32).map(ok).collect();
    c.from_spawns = fwd;
    c.to_spawns = back;
    c
}

/// An item the graph does not get to and back from.
#[derive(Clone, Debug, Serialize)]
pub struct LostItem {
    pub class: String,
    pub origin: Vec3,
    /// `no floor` (nothing to stand on under it), `no node` (its floor is not walked to), or the way that is
    /// missing: `in`, `out`, `in and out`.
    pub why: &'static str,
    /// The node nearest to it the graph does get to and back from: origin, distance, and how much higher the item
    /// stands.
    pub nearest: Option<(Vec3, f32, f32)>,
}

/// The items of the map the graph does not get to and back from, with what is missing.
pub fn lost_items(generated: &Generated, world: &BspWorld, c: &Coverage) -> Vec<LostItem> {
    let g = &generated.graph;
    let mut out = Vec::new();
    for e in &world.entities {
        let class = e.classname();
        let item = (class.starts_with("weapon_") || class.starts_with("item_") || class.starts_with("ammo_"))
            && class != "weapon_satchel_charge";
        if !item {
            continue;
        }
        let origin = e.origin();
        let why = match generated.field.at(origin + Vec3::Z * 36.0) {
            None => "no floor",
            Some(s) => match generated.owner[s as usize] {
                NONE => "no node",
                n => match (c.from_spawns[n as usize], c.to_spawns[n as usize]) {
                    (true, true) => continue,
                    (false, true) => "in",
                    (true, false) => "out",
                    (false, false) => "in and out",
                },
            },
        };
        let at = origin + Vec3::Z * 36.0;
        let nearest = (0..g.len())
            .filter(|&n| c.roundtrip_nodes[n])
            .map(|n| (g.nodes[n].origin, g.nodes[n].origin.distance(at)))
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(o, d)| (o, d, at.z - o.z));
        out.push(LostItem {
            class: class.to_string(),
            origin,
            why,
            nearest,
        });
    }
    out
}
