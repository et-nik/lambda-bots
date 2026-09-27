//! The engine's visibility sets. PVS rows are decompressed once per map (`Mod_DecompressVis`) and the PAS is
//! derived from them as `SV_CalcPHS` does: a leaf hears whatever is visible from any leaf it sees. Entities sit in
//! the leaves their box touches (`SV_FindTouchedLeafs`); a view origin gets the fat PVS of every leaf within 8 units
//! (`SV_FatPVS`); a sound reaches a client whose origin leaf is in the PAS of the sound's leaf
//! (`SV_ValidClientMulticast`).

use lb_core::Vec3;
use lb_worldq::{VisSets, contents};
use smallvec::SmallVec;

use crate::file::{Bsp, ClipNode, Plane};

const FAT_PVS_RADIUS: f32 = 8.0;

type Leafs = SmallVec<[u32; 16]>;

pub struct MapVis {
    planes: Vec<Plane>,
    nodes: Vec<ClipNode>,
    solid: Vec<bool>,
    head: i32,
    /// Leaves 1..=visleafs have rows; bit `leaf - 1` of a row stands for `leaf`.
    visleafs: usize,
    words: usize,
    pvs: Vec<u64>,
    pas: Vec<u64>,
    /// No visibility data: the engine sends and plays everything.
    open: bool,
}

impl MapVis {
    pub fn build(bsp: &Bsp) -> MapVis {
        let visleafs = bsp.models[0].visleafs.max(0) as usize;
        let words = visleafs.div_ceil(64).max(1);
        let open = bsp.visdata.is_empty() || visleafs == 0;
        let mut pvs = vec![0u64; visleafs * words];
        if !open {
            for leaf in 1..=visleafs {
                let row = &mut pvs[(leaf - 1) * words..leaf * words];
                match bsp.pvs_row(leaf) {
                    Some(bytes) => {
                        for (i, b) in bytes.iter().enumerate() {
                            row[i / 8] |= u64::from(*b) << ((i % 8) * 8);
                        }
                    }
                    // No row (or a truncated one) means the leaf sees everything (`mod_novis`).
                    None => row.fill(u64::MAX),
                }
            }
        }
        let mut pas = pvs.clone();
        if !open {
            for leaf in 0..visleafs {
                let row = leaf * words;
                for w in 0..words {
                    let mut bits = pvs[row + w];
                    while bits != 0 {
                        let other = w * 64 + bits.trailing_zeros() as usize;
                        bits &= bits - 1;
                        if other >= visleafs {
                            break;
                        }
                        let src = other * words;
                        for k in 0..words {
                            pas[row + k] |= pvs[src + k];
                        }
                    }
                }
            }
        }
        MapVis {
            planes: bsp.planes.clone(),
            nodes: bsp.nodes.clone(),
            solid: bsp.leafs.iter().map(|l| l.contents == contents::SOLID).collect(),
            head: bsp.models[0].headnode[0],
            visleafs,
            words,
            pvs,
            pas,
            open,
        }
    }

    pub fn visleafs(&self) -> usize {
        self.visleafs
    }

    /// Bytes held by the two matrices.
    pub fn memory(&self) -> usize {
        (self.pvs.len() + self.pas.len()) * 8
    }

    fn dist(&self, node: &ClipNode, p: Vec3) -> f32 {
        let plane = &self.planes[node.plane as usize];
        if plane.kind < 3 {
            p[plane.kind as usize] - plane.dist
        } else {
            plane.normal.dot(p) - plane.dist
        }
    }

    /// Leaf containing `p` (0 is the shared solid leaf).
    pub fn leaf_at(&self, p: Vec3) -> u32 {
        let mut num = self.head;
        while num >= 0 {
            let node = &self.nodes[num as usize];
            num = node.children[usize::from(self.dist(node, p) < 0.0)];
        }
        (-1 - num) as u32
    }

    fn is_solid(&self, leaf: u32) -> bool {
        self.solid.get(leaf as usize).copied().unwrap_or(true)
    }

    /// Non-solid leaves touched by a box (`SV_FindTouchedLeafs`).
    fn box_leafs(&self, num: i32, mins: Vec3, maxs: Vec3, out: &mut Leafs) {
        if num < 0 {
            let leaf = (-1 - num) as u32;
            if !self.is_solid(leaf) {
                out.push(leaf);
            }
            return;
        }
        let node = &self.nodes[num as usize];
        let sides = box_on_plane_side(mins, maxs, &self.planes[node.plane as usize]);
        if sides & 1 != 0 {
            self.box_leafs(node.children[0], mins, maxs, out);
        }
        if sides & 2 != 0 {
            self.box_leafs(node.children[1], mins, maxs, out);
        }
    }

    /// Non-solid leaves within 8 units of a view origin (`SV_AddToFatPVS`).
    fn fat_leafs(&self, mut num: i32, p: Vec3, out: &mut Leafs) {
        loop {
            if num < 0 {
                let leaf = (-1 - num) as u32;
                if !self.is_solid(leaf) {
                    out.push(leaf);
                }
                return;
            }
            let node = &self.nodes[num as usize];
            let plane = &self.planes[node.plane as usize];
            let d = plane.normal.dot(p) - plane.dist;
            if d > FAT_PVS_RADIUS {
                num = node.children[0];
            } else if d < -FAT_PVS_RADIUS {
                num = node.children[1];
            } else {
                self.fat_leafs(node.children[0], p, out);
                num = node.children[1];
            }
        }
    }

    fn bit(&self, rows: &[u64], from: u32, to: u32) -> bool {
        let (from, to) = (from as usize, to as usize);
        if self.open || from == 0 || to == 0 || from > self.visleafs || to > self.visleafs {
            return true;
        }
        let word = rows[(from - 1) * self.words + (to - 1) / 64];
        word & (1 << ((to - 1) % 64)) != 0
    }

    /// `to` is in the PVS of `from` (leaf numbers).
    pub fn pvs(&self, from: u32, to: u32) -> bool {
        self.bit(&self.pvs, from, to)
    }

    /// `to` is in the PAS of `from` (leaf numbers).
    pub fn pas(&self, from: u32, to: u32) -> bool {
        self.bit(&self.pas, from, to)
    }
}

impl VisSets for MapVis {
    fn box_in_pvs(&self, eye: Vec3, mins: Vec3, maxs: Vec3) -> bool {
        if self.open {
            return true;
        }
        let mut eyes = Leafs::new();
        self.fat_leafs(self.head, eye, &mut eyes);
        let mut targets = Leafs::new();
        self.box_leafs(self.head, mins, maxs, &mut targets);
        eyes.iter().any(|&e| targets.iter().any(|&t| self.pvs(e, t)))
    }

    fn in_pas(&self, origin: Vec3, source: Vec3) -> bool {
        self.pas(self.leaf_at(source), self.leaf_at(origin))
    }
}

/// `BoxOnPlaneSide`: 1 = the box is in front, 2 = behind, 3 = crosses the plane.
fn box_on_plane_side(mins: Vec3, maxs: Vec3, p: &Plane) -> u8 {
    if p.kind < 3 {
        let k = p.kind as usize;
        if p.dist <= mins[k] {
            return 1;
        }
        if p.dist >= maxs[k] {
            return 2;
        }
        return 3;
    }
    let mut near = Vec3::ZERO;
    let mut far = Vec3::ZERO;
    for k in 0..3 {
        if p.normal[k] >= 0.0 {
            far[k] = maxs[k];
            near[k] = mins[k];
        } else {
            far[k] = mins[k];
            near[k] = maxs[k];
        }
    }
    let mut sides = 0;
    if p.normal.dot(far) >= p.dist {
        sides = 1;
    }
    if p.normal.dot(near) < p.dist {
        sides |= 2;
    }
    sides
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn box_sides() {
        let x = Plane {
            normal: Vec3::X,
            dist: 10.0,
            kind: 0,
        };
        assert_eq!(box_on_plane_side(Vec3::splat(11.0), Vec3::splat(20.0), &x), 1);
        assert_eq!(box_on_plane_side(Vec3::splat(0.0), Vec3::splat(10.0), &x), 2);
        assert_eq!(box_on_plane_side(Vec3::splat(0.0), Vec3::splat(20.0), &x), 3);
        let diag = Plane {
            normal: Vec3::new(1.0, 1.0, 0.0).normalize(),
            dist: 0.0,
            kind: 3,
        };
        assert_eq!(box_on_plane_side(Vec3::splat(1.0), Vec3::splat(2.0), &diag), 1);
        assert_eq!(box_on_plane_side(Vec3::splat(-2.0), Vec3::splat(-1.0), &diag), 2);
        assert_eq!(box_on_plane_side(Vec3::splat(-1.0), Vec3::splat(1.0), &diag), 3);
    }
}
