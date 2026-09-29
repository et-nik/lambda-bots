//! The lumps the engine draws a map from (`bspfile.h`): vertices, edges, faces, texture axes, lighting, models.

use lb_bsp::BspError;
use lb_bsp::file::{
    LUMP_EDGES, LUMP_FACES, LUMP_LIGHTING, LUMP_MODELS, LUMP_PLANES, LUMP_SURFEDGES, LUMP_TEXINFO, LUMP_TEXTURES,
    LUMP_VERTICES, lump_range,
};

/// `TEX_SPECIAL`: sky and liquids, drawn without a lightmap.
pub const TEX_SPECIAL: i32 = 1;

#[derive(Clone, Copy, Debug)]
pub struct Face {
    pub plane: usize,
    /// The face looks along its plane's back.
    pub back: bool,
    pub first_edge: usize,
    pub edges: usize,
    pub texinfo: usize,
    /// Light styles of the face's lightmaps, 255 past the last.
    pub styles: [u8; 4],
    /// Where the face's lightmaps start in the lighting lump; negative for none.
    pub light: i32,
}

impl Face {
    /// Lightmaps the face keeps, one a light style.
    pub fn light_styles(&self) -> usize {
        self.styles.iter().take_while(|&&s| s != 255).count()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct TexInfo {
    /// Texture axes: s = dot(p, s.xyz) + s.w, t the same.
    pub s: [f32; 4],
    pub t: [f32; 4],
    pub miptex: i32,
    pub flags: i32,
}

#[derive(Clone, Copy, Debug)]
pub struct ModelFaces {
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    pub first: usize,
    pub count: usize,
}

pub struct RenderLumps<'a> {
    /// Plane normals.
    pub planes: Vec<[f32; 3]>,
    pub vertices: Vec<[f32; 3]>,
    pub edges: Vec<[u16; 2]>,
    pub surfedges: Vec<i32>,
    pub faces: Vec<Face>,
    pub texinfo: Vec<TexInfo>,
    pub models: Vec<ModelFaces>,
    pub lighting: &'a [u8],
    /// The mip texture lump, whole.
    pub textures: &'a [u8],
}

pub(crate) fn f32_at(b: &[u8], at: usize) -> f32 {
    f32::from_le_bytes(b[at..at + 4].try_into().expect("4 bytes"))
}

pub(crate) fn i32_at(b: &[u8], at: usize) -> i32 {
    i32::from_le_bytes(b[at..at + 4].try_into().expect("4 bytes"))
}

pub(crate) fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().expect("4 bytes"))
}

pub(crate) fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(b[at..at + 2].try_into().expect("2 bytes"))
}

/// An index or count read as signed: a negative one points nowhere.
fn index(v: i32) -> usize {
    usize::try_from(v).unwrap_or(usize::MAX)
}

fn vec3(c: &[u8], at: usize) -> [f32; 3] {
    [f32_at(c, at), f32_at(c, at + 4), f32_at(c, at + 8)]
}

impl<'a> RenderLumps<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<RenderLumps<'a>, BspError> {
        let slice = |i: usize, record: usize| -> Result<&'a [u8], BspError> {
            let (ofs, len) = lump_range(bytes, i, record)?;
            Ok(&bytes[ofs..ofs + len])
        };
        let planes = slice(LUMP_PLANES, 20)?.chunks_exact(20).map(|c| vec3(c, 0)).collect();
        let vertices = slice(LUMP_VERTICES, 12)?.chunks_exact(12).map(|c| vec3(c, 0)).collect();
        let edges = slice(LUMP_EDGES, 4)?
            .chunks_exact(4)
            .map(|c| [u16_at(c, 0), u16_at(c, 2)])
            .collect();
        let surfedges = slice(LUMP_SURFEDGES, 4)?
            .chunks_exact(4)
            .map(|c| i32_at(c, 0))
            .collect();
        let faces = slice(LUMP_FACES, 20)?
            .chunks_exact(20)
            .map(|c| Face {
                plane: usize::from(u16_at(c, 0)),
                back: u16_at(c, 2) != 0,
                first_edge: index(i32_at(c, 4)),
                edges: usize::from(u16_at(c, 8)),
                texinfo: usize::from(u16_at(c, 10)),
                styles: [c[12], c[13], c[14], c[15]],
                light: i32_at(c, 16),
            })
            .collect();
        let texinfo = slice(LUMP_TEXINFO, 40)?
            .chunks_exact(40)
            .map(|c| TexInfo {
                s: [f32_at(c, 0), f32_at(c, 4), f32_at(c, 8), f32_at(c, 12)],
                t: [f32_at(c, 16), f32_at(c, 20), f32_at(c, 24), f32_at(c, 28)],
                miptex: i32_at(c, 32),
                flags: i32_at(c, 36),
            })
            .collect();
        let models = slice(LUMP_MODELS, 64)?
            .chunks_exact(64)
            .map(|c| ModelFaces {
                mins: vec3(c, 0),
                maxs: vec3(c, 12),
                first: index(i32_at(c, 56)),
                count: index(i32_at(c, 60)),
            })
            .collect();
        Ok(RenderLumps {
            planes,
            vertices,
            edges,
            surfedges,
            faces,
            texinfo,
            models,
            lighting: slice(LUMP_LIGHTING, 0)?,
            textures: slice(LUMP_TEXTURES, 0)?,
        })
    }

    /// The face's corners in order, or `None` when its edges point outside the lumps.
    pub fn corners(&self, f: &Face) -> Option<Vec<[f32; 3]>> {
        if f.edges < 3 {
            return None;
        }
        let mut out = Vec::with_capacity(f.edges);
        for i in 0..f.edges {
            let se = *self.surfedges.get(f.first_edge.checked_add(i)?)?;
            let v = if se >= 0 {
                self.edges.get(se as usize)?[0]
            } else {
                self.edges.get(se.unsigned_abs() as usize)?[1]
            };
            out.push(*self.vertices.get(usize::from(v))?);
        }
        Some(out)
    }

    /// Model `m`'s faces, as far as they are in the face lump.
    pub fn model_faces(&self, m: usize) -> std::ops::Range<usize> {
        let Some(model) = self.models.get(m) else {
            return 0..0;
        };
        let end = model.first.saturating_add(model.count).min(self.faces.len());
        model.first.min(end)..end
    }
}
