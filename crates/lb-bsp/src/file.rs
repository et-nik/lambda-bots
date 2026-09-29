//! BSP v30 lumps (`bspfile.h`): only what the tracer, PVS and entity model need.

use lb_core::Vec3;

pub const VERSION: i32 = 30;
const LUMPS: usize = 15;
pub const LUMP_ENTITIES: usize = 0;
pub const LUMP_PLANES: usize = 1;
pub const LUMP_TEXTURES: usize = 2;
pub const LUMP_VERTICES: usize = 3;
pub const LUMP_VISIBILITY: usize = 4;
pub const LUMP_NODES: usize = 5;
pub const LUMP_TEXINFO: usize = 6;
pub const LUMP_FACES: usize = 7;
pub const LUMP_LIGHTING: usize = 8;
pub const LUMP_CLIPNODES: usize = 9;
pub const LUMP_LEAFS: usize = 10;
pub const LUMP_EDGES: usize = 12;
pub const LUMP_SURFEDGES: usize = 13;
pub const LUMP_MODELS: usize = 14;
pub const MAX_HULLS: usize = 4;

#[derive(Debug, thiserror::Error)]
pub enum BspError {
    #[error("file is too short for a BSP header")]
    Truncated,
    #[error("BSP version {0} is not supported (need 30)")]
    Version(i32),
    #[error("lump {lump} is out of the file bounds or has a bad size")]
    BadLump { lump: usize },
    #[error("{0}")]
    Invalid(String),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plane {
    pub normal: Vec3,
    pub dist: f32,
    /// 0..2 = axial (x, y, z); 3..5 = not axial.
    pub kind: i32,
}

/// A hull tree node: plane and two children (≥ 0 = node, < 0 = contents).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClipNode {
    pub plane: u32,
    pub children: [i32; 2],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Leaf {
    pub contents: i32,
    /// Offset of the leaf's compressed PVS row; -1 = sees everything.
    pub visofs: i32,
    pub mins: [i16; 3],
    pub maxs: [i16; 3],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Model {
    pub mins: Vec3,
    pub maxs: Vec3,
    pub origin: Vec3,
    pub headnode: [i32; MAX_HULLS],
    pub visleafs: i32,
}

#[derive(Clone, Debug)]
pub struct Bsp {
    pub planes: Vec<Plane>,
    /// Hull 0 tree made from the render nodes (`Mod_MakeHull0`): leaf children carry the leaf contents.
    pub hull0: Vec<ClipNode>,
    /// Render-node children with leaf indices (`-1 - leaf`) kept, for leaf lookup.
    pub nodes: Vec<ClipNode>,
    /// Hulls 1..3 of every model.
    pub clipnodes: Vec<ClipNode>,
    pub leafs: Vec<Leaf>,
    pub models: Vec<Model>,
    pub visdata: Vec<u8>,
    pub entities: String,
    pub textures: Vec<String>,
    /// blake3 of the whole file and its size: the key of anything derived from this map.
    pub fingerprint: ([u8; 32], u64),
}

struct Reader<'a> {
    bytes: &'a [u8],
}

impl Reader<'_> {
    fn i32(&self, at: usize) -> i32 {
        i32::from_le_bytes(self.bytes[at..at + 4].try_into().expect("4 bytes"))
    }

    fn i16(&self, at: usize) -> i16 {
        i16::from_le_bytes(self.bytes[at..at + 2].try_into().expect("2 bytes"))
    }

    fn f32(&self, at: usize) -> f32 {
        f32::from_le_bytes(self.bytes[at..at + 4].try_into().expect("4 bytes"))
    }

    fn vec3(&self, at: usize) -> Vec3 {
        Vec3::new(self.f32(at), self.f32(at + 4), self.f32(at + 8))
    }
}

/// Offset and length of lump `i` of a BSP v30 file, checked against the file and, when `record` is not 0, against
/// the size of its records.
pub fn lump_range(bytes: &[u8], i: usize, record: usize) -> Result<(usize, usize), BspError> {
    if bytes.len() < 4 + LUMPS * 8 {
        return Err(BspError::Truncated);
    }
    let r = Reader { bytes };
    let version = r.i32(0);
    if version != VERSION {
        return Err(BspError::Version(version));
    }
    if i >= LUMPS {
        return Err(BspError::BadLump { lump: i });
    }
    let ofs = r.i32(4 + i * 8);
    let len = r.i32(8 + i * 8);
    if ofs < 0 || len < 0 || (ofs as usize).saturating_add(len as usize) > bytes.len() {
        return Err(BspError::BadLump { lump: i });
    }
    if record > 0 && !(len as usize).is_multiple_of(record) {
        return Err(BspError::BadLump { lump: i });
    }
    Ok((ofs as usize, len as usize))
}

impl Bsp {
    pub fn parse(bytes: &[u8]) -> Result<Bsp, BspError> {
        let r = Reader { bytes };
        let lump = |i: usize, record: usize| lump_range(bytes, i, record);
        lump(LUMP_ENTITIES, 0)?;

        let (ofs, len) = lump(LUMP_PLANES, 20)?;
        let planes: Vec<Plane> = (0..len / 20)
            .map(|i| {
                let at = ofs + i * 20;
                Plane {
                    normal: r.vec3(at),
                    dist: r.f32(at + 12),
                    kind: r.i32(at + 16),
                }
            })
            .collect();
        if planes.iter().any(|p| !(0..=5).contains(&p.kind)) {
            return Err(BspError::Invalid("plane type out of range".into()));
        }

        let (ofs, len) = lump(LUMP_LEAFS, 28)?;
        let leafs: Vec<Leaf> = (0..len / 28)
            .map(|i| {
                let at = ofs + i * 28;
                Leaf {
                    contents: r.i32(at),
                    visofs: r.i32(at + 4),
                    mins: [r.i16(at + 8), r.i16(at + 10), r.i16(at + 12)],
                    maxs: [r.i16(at + 14), r.i16(at + 16), r.i16(at + 18)],
                }
            })
            .collect();

        let (ofs, len) = lump(LUMP_NODES, 24)?;
        let mut nodes = Vec::with_capacity(len / 24);
        let mut hull0 = Vec::with_capacity(len / 24);
        for i in 0..len / 24 {
            let at = ofs + i * 24;
            let plane = r.i32(at);
            let children = [i32::from(r.i16(at + 4)), i32::from(r.i16(at + 6))];
            if plane < 0 || plane as usize >= planes.len() {
                return Err(BspError::Invalid(format!("node {i} has plane {plane}")));
            }
            let mut contents = [0i32; 2];
            for (j, &c) in children.iter().enumerate() {
                contents[j] = if c >= 0 {
                    c
                } else {
                    let leaf = (-1 - c) as usize;
                    leafs
                        .get(leaf)
                        .map(|l| l.contents)
                        .ok_or_else(|| BspError::Invalid(format!("node {i} leaf {leaf}")))?
                };
            }
            nodes.push(ClipNode {
                plane: plane as u32,
                children,
            });
            hull0.push(ClipNode {
                plane: plane as u32,
                children: contents,
            });
        }
        if nodes
            .iter()
            .any(|n| n.children.iter().any(|&c| c >= 0 && c as usize >= nodes.len()))
        {
            return Err(BspError::Invalid("node child out of range".into()));
        }

        let (ofs, len) = lump(LUMP_CLIPNODES, 8)?;
        let clipnodes: Vec<ClipNode> = (0..len / 8)
            .map(|i| {
                let at = ofs + i * 8;
                ClipNode {
                    plane: r.i32(at) as u32,
                    children: [i32::from(r.i16(at + 4)), i32::from(r.i16(at + 6))],
                }
            })
            .collect();
        if clipnodes.iter().any(|c| {
            c.plane as usize >= planes.len() || c.children.iter().any(|&ch| ch >= 0 && ch as usize >= clipnodes.len())
        }) {
            return Err(BspError::Invalid("clipnode plane or child out of range".into()));
        }

        let (ofs, len) = lump(LUMP_MODELS, 64)?;
        let models: Vec<Model> = (0..len / 64)
            .map(|i| {
                let at = ofs + i * 64;
                Model {
                    mins: r.vec3(at),
                    maxs: r.vec3(at + 12),
                    origin: r.vec3(at + 24),
                    headnode: [r.i32(at + 36), r.i32(at + 40), r.i32(at + 44), r.i32(at + 48)],
                    visleafs: r.i32(at + 52),
                }
            })
            .collect();
        if models.is_empty() {
            return Err(BspError::Invalid("no models".into()));
        }
        let head = models[0].headnode[0];
        if head < 0 || head as usize >= nodes.len() {
            return Err(BspError::Invalid(format!("world head node {head} out of range")));
        }
        // Leaves 1..=visleafs carry PVS rows; leaf 0 is the shared solid leaf.
        let visleafs = models[0].visleafs;
        if visleafs < 0 || visleafs as usize >= leafs.len() {
            return Err(BspError::Invalid(format!("world visleafs {visleafs} out of range")));
        }

        let (ofs, len) = lump(LUMP_VISIBILITY, 0)?;
        let visdata = bytes[ofs..ofs + len].to_vec();

        let (ofs, len) = lump(LUMP_ENTITIES, 0)?;
        let text = &bytes[ofs..ofs + len];
        let text = text.split(|b| *b == 0).next().unwrap_or(&[]);
        let entities = String::from_utf8_lossy(text).into_owned();

        let (ofs, len) = lump(LUMP_TEXTURES, 0)?;
        let mut textures = Vec::new();
        if len >= 4 {
            let count = r.i32(ofs).max(0) as usize;
            for i in 0..count.min((len - 4) / 4) {
                let off = r.i32(ofs + 4 + i * 4);
                if off < 0 {
                    textures.push(String::new());
                    continue;
                }
                let at = ofs + off as usize;
                let name = bytes.get(at..at + 16).unwrap_or(&[]);
                let name = name.split(|b| *b == 0).next().unwrap_or(&[]);
                textures.push(String::from_utf8_lossy(name).into_owned());
            }
        }

        Ok(Bsp {
            planes,
            hull0,
            nodes,
            clipnodes,
            leafs,
            models,
            visdata,
            entities,
            textures,
            fingerprint: (*blake3::hash(bytes).as_bytes(), bytes.len() as u64),
        })
    }

    /// Hex of the first 8 bytes of the fingerprint, for file names and logs.
    pub fn short_id(&self) -> String {
        self.fingerprint.0[..8].iter().map(|b| format!("{b:02x}")).collect()
    }
}
