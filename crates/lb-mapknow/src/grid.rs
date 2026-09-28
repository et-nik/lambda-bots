//! Nodes filed by where they are on the floor plan, to find the one nearest to a point without looking at all of
//! them.

use lb_core::{Vec2, Vec3};
use lb_nav_api::NodeId;

const CELL: f32 = 128.0;

#[derive(Clone, Debug, Default)]
pub struct NodeGrid {
    min: Vec2,
    cols: i32,
    rows: i32,
    /// Where each cell's nodes start in `items` (one past the last cell at the end).
    start: Vec<u32>,
    items: Vec<NodeId>,
}

/// Distance as the graph measures it: vertical distance counts double, so a node on the floor above is not taken
/// for the one underfoot.
pub fn metric(a: Vec3, b: Vec3) -> f32 {
    let d = a - b;
    (d.x * d.x + d.y * d.y + 4.0 * d.z * d.z).sqrt()
}

impl NodeGrid {
    pub fn build(origins: &[Vec3]) -> NodeGrid {
        if origins.is_empty() {
            return NodeGrid::default();
        }
        let (mut lo, mut hi) = (Vec2::splat(f32::INFINITY), Vec2::splat(f32::NEG_INFINITY));
        for o in origins {
            lo = lo.min(o.truncate());
            hi = hi.max(o.truncate());
        }
        let cols = (((hi.x - lo.x) / CELL) as i32 + 1).max(1);
        let rows = (((hi.y - lo.y) / CELL) as i32 + 1).max(1);
        let mut grid = NodeGrid {
            min: lo,
            cols,
            rows,
            start: vec![0; (cols * rows + 1) as usize],
            items: vec![0; origins.len()],
        };
        let cells: Vec<usize> = origins.iter().map(|o| grid.cell(o.truncate())).collect();
        for &c in &cells {
            grid.start[c + 1] += 1;
        }
        for i in 1..grid.start.len() {
            grid.start[i] += grid.start[i - 1];
        }
        let mut fill = grid.start.clone();
        for (n, &c) in cells.iter().enumerate() {
            grid.items[fill[c] as usize] = n as NodeId;
            fill[c] += 1;
        }
        grid
    }

    fn coords(&self, p: Vec2) -> (i32, i32) {
        (
            (((p.x - self.min.x) / CELL).floor() as i32).clamp(0, self.cols - 1),
            (((p.y - self.min.y) / CELL).floor() as i32).clamp(0, self.rows - 1),
        )
    }

    fn cell(&self, p: Vec2) -> usize {
        let (x, y) = self.coords(p);
        (y * self.cols + x) as usize
    }

    /// Nodes filed in the cells a circle of `radius` around `p` touches (a superset of those within it).
    pub fn around(&self, p: Vec3, radius: f32, mut f: impl FnMut(NodeId)) {
        if self.items.is_empty() {
            return;
        }
        let (x0, y0) = self.coords(p.truncate() - Vec2::splat(radius));
        let (x1, y1) = self.coords(p.truncate() + Vec2::splat(radius));
        for y in y0..=y1 {
            for x in x0..=x1 {
                let c = (y * self.cols + x) as usize;
                for &n in &self.items[self.start[c] as usize..self.start[c + 1] as usize] {
                    f(n);
                }
            }
        }
    }

    /// The node nearest to `p` by [`metric`] within `max`, among those `usable`.
    pub fn nearest(&self, origins: &[Vec3], p: Vec3, max: f32, usable: impl Fn(NodeId) -> bool) -> Option<NodeId> {
        let mut best: Option<(NodeId, f32)> = None;
        self.around(p, max, |n| {
            let d = metric(origins[n as usize], p);
            if d <= max && usable(n) && best.is_none_or(|(b, bd)| d < bd || (d == bd && n < b)) {
                best = Some((n, d));
            }
        });
        best.map(|(n, _)| n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_nearest_node_and_prefers_the_floor_underfoot() {
        let origins = vec![
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(300.0, 0.0, 0.0),
            Vec3::new(60.0, 0.0, 50.0),
            Vec3::new(-900.0, 500.0, 0.0),
        ];
        let g = NodeGrid::build(&origins);
        assert_eq!(g.nearest(&origins, Vec3::new(20.0, 0.0, 0.0), 512.0, |_| true), Some(0));
        assert_eq!(
            g.nearest(&origins, Vec3::new(260.0, 10.0, 0.0), 512.0, |_| true),
            Some(1)
        );
        assert_eq!(
            g.nearest(&origins, Vec3::new(60.0, 0.0, 0.0), 512.0, |_| true),
            Some(0),
            "height counts double: 50 up is farther than 60 across"
        );
        assert_eq!(
            g.nearest(&origins, Vec3::new(-400.0, 250.0, 0.0), 300.0, |_| true),
            None
        );
        assert_eq!(
            g.nearest(&origins, Vec3::new(20.0, 0.0, 0.0), 512.0, |n| n != 0),
            Some(2)
        );
    }
}
