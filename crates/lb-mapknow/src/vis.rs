//! Who sees whom: line of sight between every two nodes of the graph, eye to eye, worked out once per map. Glass is
//! seen through, closed doors are not. The engine's PVS rules most pairs out before any trace.

use lb_core::Vec3;
use lb_nav_api::NodeId;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

/// A symmetric bit matrix: bit `b` of row `a` is set when a player at node `a` sees one at node `b`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct VisTable {
    nodes: u32,
    words: u32,
    bits: Vec<u64>,
}

impl VisTable {
    /// A table where nobody sees anybody.
    pub fn empty(nodes: usize) -> VisTable {
        let words = nodes.div_ceil(64).max(1);
        VisTable {
            nodes: nodes as u32,
            words: words as u32,
            bits: vec![0; nodes * words],
        }
    }

    pub fn nodes(&self) -> usize {
        self.nodes as usize
    }

    fn row(&self, a: NodeId) -> &[u64] {
        let w = self.words as usize;
        let start = a as usize * w;
        self.bits.get(start..start + w).unwrap_or(&[])
    }

    fn set(&mut self, a: NodeId, b: NodeId) {
        let w = self.words as usize;
        self.bits[a as usize * w + b as usize / 64] |= 1 << (b % 64);
    }

    pub fn get(&self, a: NodeId, b: NodeId) -> bool {
        self.row(a)
            .get(b as usize / 64)
            .is_some_and(|word| word & (1 << (b % 64)) != 0)
    }

    /// Nodes seen from `a`, in order.
    pub fn seen_from(&self, a: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        self.row(a).iter().enumerate().flat_map(|(i, &word)| {
            let mut w = word;
            std::iter::from_fn(move || {
                if w == 0 {
                    return None;
                }
                let bit = w.trailing_zeros();
                w &= w - 1;
                Some(i as NodeId * 64 + bit)
            })
        })
    }

    /// How many nodes `a` sees.
    pub fn count(&self, a: NodeId) -> u32 {
        self.row(a).iter().map(|w| w.count_ones()).sum()
    }

    /// Pairs that see each other, each counted once.
    pub fn pairs(&self) -> u64 {
        (0..self.nodes).map(|a| u64::from(self.count(a))).sum::<u64>() / 2
    }

    /// Works the table out from where the eyes of players at the nodes are. `pvs(a, b)`: node `b` may be visible
    /// from node `a` at all (the engine's PVS); `clear(from, to)`: nothing blocks the line of sight. Rows are
    /// worked out in parallel; the result does not depend on the threads.
    pub fn build(
        eyes: &[Vec3],
        pvs: &(dyn Fn(usize, usize) -> bool + Sync),
        clear: &(dyn Fn(Vec3, Vec3) -> bool + Sync),
    ) -> (VisTable, u64) {
        let n = eyes.len();
        let rows: Vec<(Vec<NodeId>, u64)> = (0..n)
            .into_par_iter()
            .with_min_len(1)
            .map(|a| {
                let mut seen = Vec::new();
                let mut traces = 0;
                for b in a + 1..n {
                    if !(pvs(a, b) || pvs(b, a)) {
                        continue;
                    }
                    traces += 1;
                    if clear(eyes[a], eyes[b]) {
                        seen.push(b as NodeId);
                    }
                }
                (seen, traces)
            })
            .collect();
        let mut table = VisTable::empty(n);
        let mut traces = 0;
        for (a, (seen, t)) in rows.into_iter().enumerate() {
            traces += t;
            for b in seen {
                table.set(a as NodeId, b);
                table.set(b, a as NodeId);
            }
        }
        (table, traces)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wall_splits_the_nodes() {
        // Nodes along x; a wall at x = 250 blocks sight across it.
        let eyes: Vec<Vec3> = (0..6).map(|i| Vec3::new(i as f32 * 100.0, 0.0, 64.0)).collect();
        let clear = |a: Vec3, b: Vec3| (a.x < 250.0) == (b.x < 250.0);
        let (t, traces) = VisTable::build(&eyes, &|_, _| true, &clear);
        assert_eq!(traces, 15);
        assert!(t.get(0, 2) && t.get(2, 0));
        assert!(!t.get(2, 3) && !t.get(3, 2));
        assert_eq!(t.seen_from(4).collect::<Vec<_>>(), vec![3, 5]);
        assert_eq!(t.count(0), 2);
        assert_eq!(t.pairs(), 6);
        let (none, traces) = VisTable::build(&eyes, &|_, _| false, &clear);
        assert_eq!((none.pairs(), traces), (0, 0), "the PVS rules pairs out before tracing");
    }

    #[test]
    fn rows_cross_word_boundaries() {
        let eyes: Vec<Vec3> = (0..130).map(|i| Vec3::new(i as f32, 0.0, 0.0)).collect();
        let (t, _) = VisTable::build(&eyes, &|_, _| true, &|_, _| true);
        assert!(t.get(0, 129) && t.get(129, 64) && t.get(63, 64));
        assert_eq!(t.count(100), 129);
        assert_eq!(t.seen_from(127).last(), Some(129));
    }
}
