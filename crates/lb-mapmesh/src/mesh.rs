//! The map as the editor draws it: every model's faces in triangles, grouped by texture and lightmap page.

use std::collections::BTreeMap;

use lb_bsp::{BspError, Entity};
use serde::Serialize;

use crate::light::{self, Check, Extents, Lit, PRECISIONS, Pages, Precision, Spot};
use crate::lumps::{RenderLumps, TEX_SPECIAL};
use crate::miptex::{self, Image};
use crate::wad::Wad;

/// Version of the manifest and the binary buffers.
pub const FORMAT: u32 = 1;
/// Start of the binary buffers: `LBMM`, the format, the vertex and the index count, each a little-endian u32.
const MAGIC: &[u8; 4] = b"LBMM";
const HEADER: usize = 16;

/// A WAD the map lists, and the file when there is one.
pub struct WadRef<'a> {
    pub name: String,
    pub wad: Option<&'a Wad>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Manifest {
    pub format: u32,
    pub map: String,
    /// blake3 of the BSP file, hex: the key of everything built from it.
    pub fingerprint: String,
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    pub wads: Vec<WadInfo>,
    pub textures: Vec<TextureInfo>,
    pub lightmaps: LightmapInfo,
    pub models: Vec<ModelInfo>,
    pub entities: Vec<EntityInfo>,
    pub buffers: Buffers,
    pub stats: Stats,
}

#[derive(Clone, Debug, Serialize)]
pub struct WadInfo {
    pub name: String,
    pub found: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct TextureInfo {
    pub name: String,
    pub width: u32,
    pub height: u32,
    /// `map`, a WAD's file name, or `missing`.
    pub source: String,
    /// `{` textures: palette index 255 is see-through.
    pub alpha: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct LightmapInfo {
    pub size: u32,
    pub pages: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct ModelInfo {
    pub index: usize,
    pub entity: Option<usize>,
    pub classname: String,
    pub layer: &'static str,
    /// Where the entity puts the model: its `origin`, zero for most brush entities.
    pub origin: [f32; 3],
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    pub rendermode: i32,
    pub renderamt: i32,
    pub groups: Vec<Group>,
}

/// Triangles of one model with one texture and lightmap page: `count` indices from `first`.
#[derive(Clone, Debug, Serialize)]
pub struct Group {
    pub texture: Option<usize>,
    /// The lightmap page, `None` for faces drawn without light (sky, liquids, unlit).
    pub page: Option<usize>,
    pub kind: Kind,
    pub layer: &'static str,
    pub first: u32,
    pub count: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Solid,
    /// `{` textures, cut out where the palette's last color is.
    Alpha,
    /// `!` textures.
    Water,
    Sky,
    /// Compile tool textures left on brush entities: triggers, clips, origins.
    Tool,
}

#[derive(Clone, Debug, Serialize)]
pub struct EntityInfo {
    pub index: usize,
    pub classname: String,
    pub targetname: Option<String>,
    pub origin: [f32; 3],
    pub model: Option<usize>,
    /// Every key and value in file order.
    pub kv: Vec<(String, String)>,
}

/// Byte offsets of the arrays in the binary buffers.
#[derive(Clone, Debug, Serialize)]
pub struct Buffers {
    pub vertices: u32,
    pub indices: u32,
    /// f32 x, y, z a vertex.
    pub positions: usize,
    /// f32 u, v a vertex: texture coordinates over the texture's size.
    pub uvs: usize,
    /// f32 u, v a vertex: lightmap page coordinates.
    pub light_uvs: usize,
    /// u32, three a triangle.
    pub triangles: usize,
    pub bytes: usize,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Stats {
    pub faces: usize,
    /// Faces left out: edges or texture axes pointing outside the lumps.
    pub skipped: usize,
    pub triangles: usize,
    pub missing_textures: usize,
    pub lightmaps: Check,
}

pub struct MapMesh {
    pub manifest: Manifest,
    /// The binary buffers the manifest's `buffers` describe.
    pub mesh: Vec<u8>,
    images: Vec<Option<Image>>,
    pages: Pages,
}

impl MapMesh {
    /// Texture `i` as a PNG, `None` for a missing one.
    pub fn texture_png(&self, i: usize) -> Option<Vec<u8>> {
        let image = self.images.get(i)?.as_ref()?;
        let info = &self.manifest.textures[i];
        Some(crate::png::indexed(info.width, info.height, image, info.alpha))
    }

    /// Lightmap page `i` as a PNG.
    pub fn lightmap_png(&self, i: usize) -> Option<Vec<u8>> {
        let rgb = self.pages.rgb.get(i)?;
        Some(crate::png::rgb(self.pages.size, self.pages.size, rgb))
    }
}

/// The WADs map `bsp` lists in its worldspawn, lowercase file names.
pub fn listed_wads(bsp: &[u8]) -> Result<Vec<String>, BspError> {
    let entities = entities(bsp)?;
    Ok(entities
        .first()
        .and_then(|w| w.get("wad"))
        .map(crate::wad::listed)
        .unwrap_or_default())
}

fn entities(bsp: &[u8]) -> Result<Vec<Entity>, BspError> {
    let (ofs, len) = lb_bsp::file::lump_range(bsp, lb_bsp::file::LUMP_ENTITIES, 0)?;
    let text = &bsp[ofs..ofs + len];
    let text = text.split(|b| *b == 0).next().unwrap_or(&[]);
    Ok(lb_bsp::parse_entities(&String::from_utf8_lossy(text)))
}

fn kind_of(name: &str, flags: i32) -> Kind {
    let n = name.to_ascii_lowercase();
    if n == "sky" {
        Kind::Sky
    } else if n.starts_with('!') {
        Kind::Water
    } else if matches!(
        n.as_str(),
        "aaatrigger" | "clip" | "null" | "origin" | "hint" | "skip" | "bevel" | "solidhint"
    ) || n.starts_with("cliphull")
    {
        Kind::Tool
    } else if n.starts_with('{') {
        Kind::Alpha
    } else if flags & TEX_SPECIAL != 0 {
        Kind::Water
    } else {
        Kind::Solid
    }
}

/// The editor's layer for a brush entity's class.
pub fn layer_of(classname: &str) -> &'static str {
    match classname {
        "worldspawn" => "world",
        "func_door"
        | "func_door_rotating"
        | "func_plat"
        | "func_platrot"
        | "func_train"
        | "func_tracktrain"
        | "func_trackchange"
        | "func_trackautochange"
        | "func_rotating"
        | "func_pendulum"
        | "func_button"
        | "func_rot_button"
        | "momentary_door"
        | "momentary_rot_button"
        | "func_guntarget" => "movers",
        "func_breakable" | "func_pushable" => "breakables",
        "func_water" => "water",
        "func_ladder" => "ladders",
        "func_monsterclip" | "func_friction" | "func_mortar_field" | "func_clip" => "triggers",
        c if c.starts_with("trigger_") => "triggers",
        _ => "walls",
    }
}

fn group_layer(kind: Kind, model_layer: &'static str) -> &'static str {
    match kind {
        Kind::Sky => "sky",
        Kind::Tool => "triggers",
        Kind::Water => "water",
        Kind::Solid | Kind::Alpha => model_layer,
    }
}

/// A face on its way into the buffers.
struct Drawn {
    face: usize,
    corners: Vec<[f32; 3]>,
    normal: [f32; 3],
}

pub fn build(map: &str, bsp: &[u8], wads: &[WadRef<'_>]) -> Result<MapMesh, BspError> {
    let lumps = RenderLumps::parse(bsp)?;
    let entities = entities(bsp)?;
    let mut stats = Stats::default();

    // Textures: the map's own image, else the first listed WAD that has it.
    let headers = miptex::lump(lumps.textures);
    let mut textures = Vec::with_capacity(headers.len());
    let mut images = Vec::with_capacity(headers.len());
    for h in &headers {
        let Some(h) = h else {
            textures.push(TextureInfo {
                name: String::new(),
                width: 64,
                height: 64,
                source: "missing".into(),
                alpha: false,
            });
            images.push(None);
            stats.missing_textures += 1;
            continue;
        };
        let (found, source) = match &h.image {
            Some(img) => (Some((h.width, h.height, img.clone())), "map".to_string()),
            None => wads
                .iter()
                .find_map(|w| {
                    let t = w.wad?.texture(&h.name)?;
                    Some(((t.width, t.height, t.image?), w.name.clone()))
                })
                .map_or((None, "missing".to_string()), |(f, s)| (Some(f), s)),
        };
        stats.missing_textures += usize::from(found.is_none());
        let (width, height) = found.as_ref().map_or((h.width, h.height), |f| (f.0, f.1));
        textures.push(TextureInfo {
            name: h.name.clone(),
            width,
            height,
            source,
            alpha: h.name.starts_with('{'),
        });
        images.push(found.map(|f| f.2));
    }

    // Faces worth drawing, and the lightmaps of the lit ones.
    let mut drawn: Vec<Option<Drawn>> = Vec::with_capacity(lumps.faces.len());
    let mut lit = Vec::new();
    for (i, f) in lumps.faces.iter().enumerate() {
        let (Some(corners), Some(ti), Some(&plane)) = (
            lumps.corners(f),
            lumps.texinfo.get(f.texinfo),
            lumps.planes.get(f.plane),
        ) else {
            drawn.push(None);
            stats.skipped += 1;
            continue;
        };
        let normal = if f.back {
            [-plane[0], -plane[1], -plane[2]]
        } else {
            plane
        };
        let special = ti.flags & TEX_SPECIAL != 0;
        if !special && f.light >= 0 && f.light_styles() > 0 {
            lit.push(Lit {
                face: i,
                offset: f.light as usize,
                styles: f.light_styles(),
                candidates: PRECISIONS.map(|p| light::extents(&corners, ti, p)),
            });
        }
        drawn.push(Some(Drawn {
            face: i,
            corners,
            normal,
        }));
    }
    let (settled, check) = light::settle(&mut lit, lumps.lighting.len());
    stats.lightmaps = check;
    let sizes: Vec<[u32; 2]> = settled.iter().map(|(_, e)| e.size).collect();
    let (side, spots) = light::pack(&sizes);
    let page_count = spots.iter().map(|s| s.page + 1).max().unwrap_or(0);
    let mut pages = Pages::new(side, page_count);
    let mut lightmap: Vec<Option<(Extents, Spot)>> = vec![None; lumps.faces.len()];
    for ((face, e), spot) in settled.iter().zip(&spots) {
        let from = lumps.faces[*face].light as usize;
        let bytes = e.luxels() * 3;
        let src = lumps.lighting.get(from..).map_or(&[][..], |s| &s[..bytes.min(s.len())]);
        pages.put(*spot, e.size[0], e.size[1], src);
        lightmap[*face] = Some((*e, *spot));
    }

    // Models and the entities that place them.
    let mut model_entity: BTreeMap<usize, usize> = BTreeMap::new();
    model_entity.insert(0, 0);
    for (i, e) in entities.iter().enumerate() {
        if let Some(m) = e.brush_model() {
            model_entity.entry(m).or_insert(i);
        }
    }

    let mut positions: Vec<f32> = Vec::new();
    let mut uvs: Vec<f32> = Vec::new();
    let mut light_uvs: Vec<f32> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let mut models = Vec::new();
    for (m, faces) in (0..lumps.models.len()).map(|m| (m, lumps.model_faces(m))) {
        let Some(&e) = model_entity.get(&m) else {
            continue;
        };
        let ent = entities.get(e);
        let classname = ent.map_or("worldspawn", |e| e.classname()).to_string();
        let layer = if m == 0 { "world" } else { layer_of(&classname) };
        let mut groups: BTreeMap<(Kind, Option<usize>, Option<usize>), Vec<u32>> = BTreeMap::new();
        for d in faces.filter_map(|f| drawn[f].as_ref()) {
            let f = &lumps.faces[d.face];
            let ti = &lumps.texinfo[f.texinfo];
            let texture = usize::try_from(ti.miptex).ok().filter(|&t| t < textures.len());
            let name = texture.map_or("", |t| textures[t].name.as_str());
            let kind = kind_of(name, ti.flags);
            let (tw, th) = texture.map_or((64.0, 64.0), |t| (textures[t].width as f32, textures[t].height as f32));
            let light = lightmap[d.face];
            let base = (positions.len() / 3) as u32;
            for &p in &d.corners {
                positions.extend_from_slice(&p);
                let s = light::coord(p, ti.s, Precision::Double);
                let t = light::coord(p, ti.t, Precision::Double);
                uvs.extend_from_slice(&[s as f32 / tw, t as f32 / th]);
                let luv = light.map_or([0.0, 0.0], |(e, spot)| {
                    let side = f64::from(side);
                    [
                        (((s - f64::from(e.mins[0])) / 16.0 + f64::from(spot.x) + 0.5) / side) as f32,
                        (((t - f64::from(e.mins[1])) / 16.0 + f64::from(spot.y) + 0.5) / side) as f32,
                    ]
                });
                light_uvs.extend_from_slice(&luv);
            }
            let ccw = dot(newell(&d.corners), d.normal) >= 0.0;
            let tris = groups.entry((kind, texture, light.map(|(_, s)| s.page))).or_default();
            for k in 1..d.corners.len() as u32 - 1 {
                if ccw {
                    tris.extend_from_slice(&[base, base + k, base + k + 1]);
                } else {
                    tris.extend_from_slice(&[base, base + k + 1, base + k]);
                }
            }
            stats.faces += 1;
        }
        let mut out = Vec::with_capacity(groups.len());
        for ((kind, texture, page), tris) in groups {
            out.push(Group {
                texture,
                page,
                kind,
                layer: group_layer(kind, layer),
                first: indices.len() as u32,
                count: tris.len() as u32,
            });
            indices.extend_from_slice(&tris);
        }
        let model = &lumps.models[m];
        models.push(ModelInfo {
            index: m,
            entity: ent.map(|_| e),
            classname,
            layer,
            origin: if m == 0 { [0.0; 3] } else { ent.map_or([0.0; 3], origin) },
            mins: model.mins,
            maxs: model.maxs,
            rendermode: ent.and_then(|e| e.int("rendermode")).unwrap_or(0),
            renderamt: ent.and_then(|e| e.int("renderamt")).unwrap_or(255),
            groups: out,
        });
    }
    stats.triangles = indices.len() / 3;

    let vertices = positions.len() / 3;
    let at_positions = HEADER;
    let at_uvs = at_positions + vertices * 12;
    let at_light = at_uvs + vertices * 8;
    let at_triangles = at_light + vertices * 8;
    let bytes = at_triangles + indices.len() * 4;
    let mut mesh = Vec::with_capacity(bytes);
    mesh.extend_from_slice(MAGIC);
    for v in [FORMAT, vertices as u32, indices.len() as u32] {
        mesh.extend_from_slice(&v.to_le_bytes());
    }
    for v in positions.iter().chain(&uvs).chain(&light_uvs) {
        mesh.extend_from_slice(&v.to_le_bytes());
    }
    for i in &indices {
        mesh.extend_from_slice(&i.to_le_bytes());
    }

    let world = lumps.models.first();
    let manifest = Manifest {
        format: FORMAT,
        map: map.to_string(),
        fingerprint: blake3::hash(bsp).to_hex().to_string(),
        mins: world.map_or([0.0; 3], |w| w.mins),
        maxs: world.map_or([0.0; 3], |w| w.maxs),
        wads: wads
            .iter()
            .map(|w| WadInfo {
                name: w.name.clone(),
                found: w.wad.is_some(),
            })
            .collect(),
        textures,
        lightmaps: LightmapInfo {
            size: side,
            pages: page_count,
        },
        models,
        entities: entities
            .iter()
            .enumerate()
            .map(|(i, e)| EntityInfo {
                index: i,
                classname: e.classname().to_string(),
                targetname: e.get("targetname").map(str::to_string),
                origin: origin(e),
                model: e.brush_model(),
                kv: e.kv.clone(),
            })
            .collect(),
        buffers: Buffers {
            vertices: vertices as u32,
            indices: indices.len() as u32,
            positions: at_positions,
            uvs: at_uvs,
            light_uvs: at_light,
            triangles: at_triangles,
            bytes,
        },
        stats,
    };
    Ok(MapMesh {
        manifest,
        mesh,
        images,
        pages,
    })
}

fn origin(e: &Entity) -> [f32; 3] {
    let o = e.origin();
    [o.x, o.y, o.z]
}

/// Normal of a polygon by Newell's method, the right-hand rule over its corners.
fn newell(c: &[[f32; 3]]) -> [f32; 3] {
    let mut n = [0.0f32; 3];
    for (i, a) in c.iter().enumerate() {
        let b = c[(i + 1) % c.len()];
        n[0] += (a[1] - b[1]) * (a[2] + b[2]);
        n[1] += (a[2] - b[2]) * (a[0] + b[0]);
        n[2] += (a[0] - b[0]) * (a[1] + b[1]);
    }
    n
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
