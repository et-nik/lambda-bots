//! Mip textures (`miptex_t`) as the map's texture lump and WAD3 files keep them: a name, the size, four mip levels of
//! palette indices, and after the smallest level the palette.

use crate::lumps::{i32_at, u16_at, u32_at};

#[derive(Clone, Debug)]
pub struct MipTex {
    pub name: String,
    pub width: u32,
    pub height: u32,
    /// The full-size image, or `None` when it lives in a WAD.
    pub image: Option<Image>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    /// Palette indices, row by row from the top.
    pub indices: Vec<u8>,
    pub palette: Vec<[u8; 3]>,
}

/// The largest side the editor takes a texture with.
const MAX_SIDE: u32 = 4096;

fn cstr(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).into_owned()
}

/// The mip texture at the start of `b`. An image with no palette after it is left to the WADs.
pub fn parse(b: &[u8]) -> Option<MipTex> {
    if b.len() < 40 {
        return None;
    }
    let name = cstr(&b[..16]);
    let (width, height) = (u32_at(b, 16), u32_at(b, 20));
    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
        return None;
    }
    let first = u32_at(b, 24) as usize;
    let last = u32_at(b, 36) as usize;
    let image = (first != 0)
        .then(|| image(b, first, last, width as usize, height as usize))
        .flatten();
    Some(MipTex {
        name,
        width,
        height,
        image,
    })
}

fn image(b: &[u8], first: usize, last: usize, w: usize, h: usize) -> Option<Image> {
    let indices = b.get(first..first.checked_add(w * h)?)?.to_vec();
    let at = last.checked_add((w / 8) * (h / 8))?;
    let count = usize::from(u16_at(b.get(at..at + 2)?, 0));
    if count == 0 || count > 256 {
        return None;
    }
    let mut palette = vec![[0u8; 3]; 256];
    for (slot, c) in palette
        .iter_mut()
        .zip(b.get(at + 2..at + 2 + count * 3)?.chunks_exact(3))
    {
        *slot = [c[0], c[1], c[2]];
    }
    Some(Image { indices, palette })
}

/// The map's texture lump: one entry a texture index, `None` where it is unreadable.
pub fn lump(b: &[u8]) -> Vec<Option<MipTex>> {
    if b.len() < 4 {
        return Vec::new();
    }
    let count = usize::try_from(i32_at(b, 0)).unwrap_or(0).min((b.len() - 4) / 4);
    (0..count)
        .map(|i| {
            let at = usize::try_from(i32_at(b, 4 + i * 4)).ok()?;
            parse(b.get(at..)?)
        })
        .collect()
}

#[cfg(test)]
pub(crate) fn encode(name: &str, w: u32, h: u32, image: Option<&Image>) -> Vec<u8> {
    let mut out = vec![0u8; 40];
    out[..name.len()].copy_from_slice(name.as_bytes());
    out[16..20].copy_from_slice(&w.to_le_bytes());
    out[20..24].copy_from_slice(&h.to_le_bytes());
    let Some(image) = image else {
        return out;
    };
    let (w, h) = (w as usize, h as usize);
    let mut at = 40;
    for (level, size) in [w * h, w * h / 4, w * h / 16, w * h / 64].into_iter().enumerate() {
        out[24 + level * 4..28 + level * 4].copy_from_slice(&(at as u32).to_le_bytes());
        if level == 0 {
            out.extend_from_slice(&image.indices);
        } else {
            out.extend(std::iter::repeat_n(0u8, size));
        }
        at += size;
    }
    out.extend_from_slice(&256u16.to_le_bytes());
    for c in &image.palette {
        out.extend_from_slice(c);
    }
    out.extend_from_slice(&[0, 0]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture() -> Image {
        let mut palette = vec![[0u8; 3]; 256];
        palette[1] = [255, 0, 0];
        palette[255] = [0, 0, 255];
        Image {
            indices: (0..16 * 16).map(|i| (i % 3) as u8).collect(),
            palette,
        }
    }

    #[test]
    fn an_image_and_its_palette_come_back() {
        let img = picture();
        let t = parse(&encode("{grate", 16, 16, Some(&img))).expect("parses");
        assert_eq!((t.name.as_str(), t.width, t.height), ("{grate", 16, 16));
        assert_eq!(t.image, Some(img));
    }

    #[test]
    fn a_texture_from_a_wad_has_only_its_size() {
        let t = parse(&encode("crate01", 64, 32, None)).expect("parses");
        assert_eq!((t.width, t.height), (64, 32));
        assert!(t.image.is_none());
    }

    #[test]
    fn broken_textures_are_none_not_panics() {
        let good = encode("wall", 16, 16, Some(&picture()));
        for cut in [0, 10, 39, 40, 200, good.len() - 700] {
            let _ = parse(&good[..cut]);
        }
        let mut huge = good.clone();
        huge[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(parse(&huge).is_none());
        let mut lump = 3i32.to_le_bytes().to_vec();
        for off in [-1i32, 1_000_000, 16] {
            lump.extend_from_slice(&off.to_le_bytes());
        }
        lump.extend_from_slice(&good);
        let parsed = super::lump(&lump);
        assert!(parsed[0].is_none() && parsed[1].is_none());
        assert_eq!(parsed[2].as_ref().map(|t| t.name.as_str()), Some("wall"));
    }
}
