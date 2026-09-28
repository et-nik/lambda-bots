//! What the bots learned by playing a map (yapb's practice data, with honest inputs): where they got hurt and died,
//! and where the damage came from as far as they could tell. Everything halves over 15 minutes of play, so the map
//! follows how people play it now. It is kept on disk by place, not by node, and survives a new graph.

use lb_core::{Vec3, dmath};
use lb_nav_api::NodeId;
use serde::{Deserialize, Serialize};

use crate::tactics::MapTactics;

/// Seconds of play for what was learned to count half.
const HALF_LIFE: f64 = 900.0;
const DECAY_EVERY: f64 = 30.0;
/// A death weighs as much as this much damage.
const DEATH: f32 = 60.0;
/// Entries on disk are put back on the node within this of where they were.
const REMAP: f32 = 64.0;
const SCHEMA: &str = "lambdabots/experience@1";

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct NodeExperience {
    pub hurt: f32,
    pub deaths: f32,
    /// Where the damage taken here came from most, and how much from there (the two biggest sources).
    pub from: [(NodeId, f32); 2],
}

impl NodeExperience {
    fn weight(&self) -> f32 {
        self.hurt + DEATH * self.deaths
    }
}

#[derive(Clone, Debug, Default)]
pub struct Experience {
    pub nodes: Vec<NodeExperience>,
    decayed_at: f64,
    max: f32,
    /// Something was learned since the last save.
    pub changed: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ExperienceFile {
    pub schema: String,
    pub map: String,
    pub entries: Vec<Entry>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Entry {
    pub at: [f32; 3],
    #[serde(default, skip_serializing_if = "is_zero")]
    pub hurt: f32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub deaths: f32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub from: Vec<([f32; 3], f32)>,
}

fn is_zero(v: &f32) -> bool {
    *v == 0.0
}

fn round(v: Vec3) -> [f32; 3] {
    [v.x.round(), v.y.round(), v.z.round()]
}

impl Experience {
    pub fn new(nodes: usize) -> Experience {
        Experience {
            nodes: vec![NodeExperience::default(); nodes],
            ..Experience::default()
        }
    }

    fn refresh_max(&mut self) {
        self.max = self.nodes.iter().map(NodeExperience::weight).fold(0.0, f32::max);
    }

    /// A bot at `at` took `amount` damage, from `from` when it could tell.
    pub fn hurt(&mut self, at: NodeId, amount: f32, from: Option<NodeId>) {
        let Some(e) = self.nodes.get_mut(at as usize) else {
            return;
        };
        e.hurt += amount;
        if let Some(src) = from.filter(|&s| s != at) {
            match e.from.iter().position(|(n, w)| *n == src && *w > 0.0) {
                Some(i) => e.from[i].1 += amount,
                None if e.from[1].1 < amount => e.from[1] = (src, amount),
                None => {}
            }
            if e.from[1].1 > e.from[0].1 {
                e.from.swap(0, 1);
            }
        }
        self.max = self.max.max(e.weight());
        self.changed = true;
    }

    /// A bot died at `at`.
    pub fn died(&mut self, at: NodeId) {
        let Some(e) = self.nodes.get_mut(at as usize) else {
            return;
        };
        e.deaths += 1.0;
        self.max = self.max.max(e.weight());
        self.changed = true;
    }

    /// Lets what was learned fade with play time.
    pub fn decay(&mut self, now: f64) {
        if self.decayed_at == 0.0 || now < self.decayed_at {
            self.decayed_at = now;
            return;
        }
        let dt = now - self.decayed_at;
        if dt < DECAY_EVERY {
            return;
        }
        self.decayed_at = now;
        let f = dmath::exp(-std::f32::consts::LN_2 * (dt / HALF_LIFE) as f32);
        for e in &mut self.nodes {
            e.hurt *= f;
            e.deaths *= f;
            e.from[0].1 *= f;
            e.from[1].1 *= f;
        }
        self.max *= f;
    }

    /// How much bots got hurt at `n`, 0..1 of the worst place.
    pub fn danger(&self, n: NodeId) -> f32 {
        if self.max <= 0.0 {
            return 0.0;
        }
        self.nodes.get(n as usize).map_or(0.0, |e| e.weight() / self.max)
    }

    pub fn danger_from(&self, n: NodeId) -> Option<NodeId> {
        self.nodes
            .get(n as usize)
            .and_then(|e| (e.from[0].1 > 0.0).then_some(e.from[0].0))
    }

    /// The places worst for bots, worst first.
    pub fn worst(&self, count: usize) -> Vec<(NodeId, f32)> {
        let mut v: Vec<(NodeId, f32)> = (0..self.nodes.len() as NodeId)
            .map(|n| (n, self.danger(n)))
            .filter(|(_, d)| *d > 0.0)
            .collect();
        v.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        v.truncate(count);
        v
    }

    pub fn to_file(&self, map: &str, t: &MapTactics) -> ExperienceFile {
        let entries = self
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, e)| e.weight() >= 1.0)
            .map(|(i, e)| Entry {
                at: round(t.origins[i]),
                hurt: (e.hurt * 10.0).round() / 10.0,
                deaths: (e.deaths * 100.0).round() / 100.0,
                from: e
                    .from
                    .iter()
                    .filter(|(_, w)| *w >= 1.0)
                    .map(|(n, w)| (round(t.origins[*n as usize]), (w * 10.0).round() / 10.0))
                    .collect(),
            })
            .collect();
        ExperienceFile {
            schema: SCHEMA.into(),
            map: map.into(),
            entries,
        }
    }

    /// Puts what a file holds back on the nodes of `t`; entries without a node close enough are dropped. Returns
    /// how many were put back.
    pub fn from_file(f: &ExperienceFile, t: &MapTactics) -> (Experience, usize) {
        let mut x = Experience::new(t.origins.len());
        let mut placed = 0;
        let node = |p: [f32; 3]| t.nearest(Vec3::from(p), REMAP);
        for e in &f.entries {
            let Some(n) = node(e.at) else { continue };
            placed += 1;
            let slot = &mut x.nodes[n as usize];
            slot.hurt += e.hurt;
            slot.deaths += e.deaths;
            for (at, w) in &e.from {
                if let Some(src) = node(*at) {
                    let i = usize::from(slot.from[0].1 > 0.0 && slot.from[0].0 != src);
                    slot.from[i] = (src, slot.from[i].1 + w);
                }
            }
            if slot.from[1].1 > slot.from[0].1 {
                slot.from.swap(0, 1);
            }
        }
        x.refresh_max();
        (x, placed)
    }

    pub fn parse(text: &str) -> Result<ExperienceFile, String> {
        let f: ExperienceFile = serde_json::from_str(text).map_err(|e| e.to_string())?;
        if f.schema != SCHEMA {
            return Err(format!("schema `{}`, expected `{SCHEMA}`", f.schema));
        }
        Ok(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn danger_is_relative_fades_and_survives_a_new_graph() {
        let (_, t) = crate::testmap::corridor();
        let mut x = Experience::new(8);
        x.hurt(2, 40.0, Some(5));
        x.hurt(2, 30.0, Some(6));
        x.hurt(2, 50.0, Some(6));
        x.died(3);
        assert_eq!(x.danger(2), 1.0);
        assert!((x.danger(3) - 60.0 / 120.0).abs() < 1e-6);
        assert_eq!(x.danger_from(2), Some(6), "the biggest source first");
        assert_eq!(x.danger(7), 0.0);
        x.decay(10.0);
        x.decay(10.0 + HALF_LIFE);
        assert!(
            (x.nodes[2].hurt - 60.0).abs() < 0.1,
            "halves in 15 minutes: {}",
            x.nodes[2].hurt
        );
        assert_eq!(x.danger(2), 1.0, "relative to the worst place");
        let file = x.to_file("test", &t);
        let text = serde_json::to_string(&file).unwrap();
        let (back, placed) = Experience::from_file(&Experience::parse(&text).unwrap(), &t);
        assert_eq!(placed, 2);
        assert_eq!(back.danger_from(2), Some(6));
        assert!((back.danger(3) - x.danger(3)).abs() < 0.01);
    }
}
