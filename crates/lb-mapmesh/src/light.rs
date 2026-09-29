//! Lightmaps: a face's lightmap size as the engine works it out (`CalcSurfaceExtents`), checked against where the next
//! face's lightmap starts, and the pages the editor packs them into.

use crate::lumps::TexInfo;

/// A face's lightmap: where it starts in texture space and its size in luxels (one luxel every 16 texels).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Extents {
    pub mins: [i32; 2],
    pub size: [u32; 2],
}

impl Extents {
    pub fn luxels(&self) -> usize {
        self.size[0] as usize * self.size[1] as usize
    }
}

/// How the texture coordinates of a face's corners are summed up. Compilers work in doubles, engines in floats;
/// a face whose corners sit on a luxel line lands on different sides by it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Precision {
    Double,
    /// Summed in doubles and stored in a float, as x87 code does.
    DoubleToFloat,
    Float,
}

pub const PRECISIONS: [Precision; 3] = [Precision::Double, Precision::DoubleToFloat, Precision::Float];

/// Texture coordinate of `p` along axis `a`.
pub fn coord(p: [f32; 3], a: [f32; 4], precision: Precision) -> f64 {
    let wide = || {
        f64::from(p[0]) * f64::from(a[0])
            + f64::from(p[1]) * f64::from(a[1])
            + f64::from(p[2]) * f64::from(a[2])
            + f64::from(a[3])
    };
    match precision {
        Precision::Double => wide(),
        Precision::DoubleToFloat => f64::from(wide() as f32),
        Precision::Float => f64::from(p[0] * a[0] + p[1] * a[1] + p[2] * a[2] + a[3]),
    }
}

pub fn extents(corners: &[[f32; 3]], ti: &TexInfo, precision: Precision) -> Extents {
    let mut out = Extents {
        mins: [0; 2],
        size: [1; 2],
    };
    for (j, axis) in [ti.s, ti.t].into_iter().enumerate() {
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        for &p in corners {
            let v = coord(p, axis, precision);
            lo = lo.min(v);
            hi = hi.max(v);
        }
        if !(lo.is_finite() && hi.is_finite()) {
            continue;
        }
        let (bmin, bmax) = ((lo / 16.0).floor(), (hi / 16.0).ceil());
        out.mins[j] = (bmin * 16.0) as i32;
        out.size[j] = ((bmax - bmin).clamp(0.0, 1024.0) as u32) + 1;
    }
    out
}

/// A face with a lightmap in the lighting lump.
#[derive(Clone, Copy, Debug)]
pub struct Lit {
    pub face: usize,
    pub offset: usize,
    pub styles: usize,
    /// Its extents in each precision, in [`PRECISIONS`] order.
    pub candidates: [Extents; 3],
}

/// How the extents of the lit faces were settled.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct Check {
    pub faces: usize,
    /// Faces whose lightmap fills the gap to the next one only in float precision.
    pub float: usize,
    /// Faces no precision fits: drawn with double extents, their light may be off.
    pub mismatched: usize,
}

/// For every lit face, the extents whose lightmaps tile the lighting lump (`total` bytes) without gaps or overlaps:
/// the first precision that fits the space up to the next face's lightmap.
pub fn settle(lit: &mut [Lit], total: usize) -> (Vec<(usize, Extents)>, Check) {
    lit.sort_by_key(|l| (l.offset, l.face));
    let mut check = Check {
        faces: lit.len(),
        ..Check::default()
    };
    let mut out = Vec::with_capacity(lit.len());
    for i in 0..lit.len() {
        let l = &lit[i];
        let gap = lit.get(i + 1).map(|next| next.offset - l.offset);
        let left = total.saturating_sub(l.offset);
        let fits = |e: &Extents| {
            let bytes = e.luxels() * 3 * l.styles;
            gap.map_or(bytes <= left, |gap| bytes == gap)
        };
        let chosen = l.candidates.iter().position(fits);
        match chosen {
            Some(k) if PRECISIONS[k] != Precision::Double => check.float += 1,
            Some(_) => {}
            None => check.mismatched += 1,
        }
        out.push((l.face, l.candidates[chosen.unwrap_or(0)]));
    }
    (out, check)
}

/// Lightmap pages: square RGB images the faces' lightmaps are packed into, each with a luxel of border.
pub struct Pages {
    pub size: u32,
    pub rgb: Vec<Vec<u8>>,
}

/// Where a face's lightmap went: its page and the corner of its first luxel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Spot {
    pub page: usize,
    pub x: u32,
    pub y: u32,
}

const SIDES: [u32; 4] = [256, 512, 1024, 2048];

/// Shelf-packs lightmaps of `sizes` (luxels) and returns the page size and every lightmap's spot.
pub fn pack(sizes: &[[u32; 2]]) -> (u32, Vec<Spot>) {
    let area: u64 = sizes.iter().map(|s| u64::from(s[0] + 2) * u64::from(s[1] + 2)).sum();
    let widest = sizes.iter().map(|s| s[0].max(s[1]) + 2).max().unwrap_or(0);
    let side = SIDES
        .into_iter()
        .find(|&s| s >= widest && area * 4 <= u64::from(s) * u64::from(s) * 3)
        .unwrap_or_else(|| widest.next_power_of_two().max(2048));
    let mut order: Vec<usize> = (0..sizes.len()).collect();
    order.sort_by_key(|&i| (std::cmp::Reverse(sizes[i][1]), std::cmp::Reverse(sizes[i][0]), i));
    let mut spots = vec![Spot { page: 0, x: 0, y: 0 }; sizes.len()];
    let (mut page, mut x, mut y, mut shelf) = (0usize, 0u32, 0u32, 0u32);
    for i in order {
        let (w, h) = (sizes[i][0] + 2, sizes[i][1] + 2);
        if x + w > side {
            x = 0;
            y += shelf;
            shelf = 0;
        }
        if y + h > side {
            page += 1;
            x = 0;
            y = 0;
            shelf = 0;
        }
        spots[i] = Spot {
            page,
            x: x + 1,
            y: y + 1,
        };
        x += w;
        shelf = shelf.max(h);
    }
    (side, spots)
}

impl Pages {
    pub fn new(side: u32, count: usize) -> Pages {
        Pages {
            size: side,
            rgb: vec![vec![0; side as usize * side as usize * 3]; count],
        }
    }

    /// Copies a `w`×`h` lightmap to `at`, repeating its edge luxels into the border around it. Luxels past the end of
    /// `src` are black.
    pub fn put(&mut self, at: Spot, w: u32, h: u32, src: &[u8]) {
        let side = self.size as i64;
        let page = &mut self.rgb[at.page];
        for yy in -1..=h as i64 {
            for xx in -1..=w as i64 {
                let (sx, sy) = (xx.clamp(0, w as i64 - 1) as usize, yy.clamp(0, h as i64 - 1) as usize);
                let from = (sy * w as usize + sx) * 3;
                let (px, py) = (at.x as i64 + xx, at.y as i64 + yy);
                let to = ((py * side + px) * 3) as usize;
                let rgb = src.get(from..from + 3).unwrap_or(&[0, 0, 0]);
                page[to..to + 3].copy_from_slice(rgb);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn axes(s: [f32; 4], t: [f32; 4]) -> TexInfo {
        TexInfo {
            s,
            t,
            miptex: 0,
            flags: 0,
        }
    }

    #[test]
    fn extents_round_out_to_whole_luxels() {
        let square = [[0.0, 0.0, 0.0], [64.0, 0.0, 0.0], [64.0, 32.0, 0.0], [0.0, 32.0, 0.0]];
        let e = extents(
            &square,
            &axes([1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0]),
            Precision::Double,
        );
        assert_eq!(
            e,
            Extents {
                mins: [0, 0],
                size: [5, 3]
            }
        );
        let shifted = extents(
            &square,
            &axes([1.0, 0.0, 0.0, -8.0], [0.0, -1.0, 0.0, 0.0]),
            Precision::Double,
        );
        assert_eq!(
            shifted,
            Extents {
                mins: [-16, -32],
                size: [6, 3]
            }
        );
    }

    #[test]
    fn the_precision_that_tiles_the_lump_is_chosen() {
        let e = |w: u32, h: u32| Extents {
            mins: [0, 0],
            size: [w, h],
        };
        let mut lit = vec![
            Lit {
                face: 1,
                offset: 12,
                styles: 1,
                candidates: [e(2, 2), e(2, 2), e(2, 2)],
            },
            Lit {
                face: 0,
                offset: 0,
                styles: 1,
                candidates: [e(2, 3), e(2, 2), e(2, 2)],
            },
            Lit {
                face: 2,
                offset: 24,
                styles: 2,
                candidates: [e(9, 9), e(9, 9), e(9, 9)],
            },
        ];
        let (chosen, check) = settle(&mut lit, 48);
        assert_eq!(
            chosen[0],
            (0, e(2, 2)),
            "doubles give 18 bytes where the next face starts at 12"
        );
        assert_eq!(chosen[1], (1, e(2, 2)));
        assert_eq!(chosen[2], (2, e(9, 9)), "nothing fits: doubles");
        assert_eq!(
            check,
            Check {
                faces: 3,
                float: 1,
                mismatched: 1
            }
        );
    }

    #[test]
    fn packed_lightmaps_do_not_overlap_and_keep_a_border() {
        let sizes: Vec<[u32; 2]> = (0..500).map(|i| [1 + i % 17, 1 + (i * 7) % 17]).collect();
        let (side, spots) = pack(&sizes);
        let mut used = std::collections::BTreeSet::new();
        for (s, sp) in sizes.iter().zip(&spots) {
            assert!(sp.x >= 1 && sp.y >= 1 && sp.x + s[0] < side && sp.y + s[1] < side);
            for y in sp.y - 1..=sp.y + s[1] {
                for x in sp.x - 1..=sp.x + s[0] {
                    assert!(used.insert((sp.page, x, y)), "overlap at {x},{y}");
                }
            }
        }
    }

    #[test]
    fn the_border_repeats_the_edge() {
        let mut p = Pages::new(8, 1);
        p.put(Spot { page: 0, x: 1, y: 1 }, 2, 1, &[10, 10, 10, 20, 20, 20]);
        let at = |x: usize, y: usize| p.rgb[0][(y * 8 + x) * 3];
        assert_eq!([at(0, 0), at(1, 0), at(2, 0), at(3, 0)], [10, 10, 20, 20]);
        assert_eq!([at(0, 1), at(1, 1), at(2, 1), at(3, 1)], [10, 10, 20, 20]);
        assert_eq!([at(0, 2), at(3, 2)], [10, 20]);
    }
}
