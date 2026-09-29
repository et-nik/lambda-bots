//! WAD3 archives: a directory of named lumps, the mip textures maps take by name among them.

use std::collections::BTreeMap;

use crate::lumps::{i32_at, u32_at};
use crate::miptex::{self, MipTex};

/// Lump type of a mip texture.
const TYPE_MIPTEX: u8 = 0x43;

pub struct Wad {
    bytes: Vec<u8>,
    /// Lowercase name → where the texture's lump lies.
    textures: BTreeMap<String, (usize, usize)>,
}

impl Wad {
    /// `None` when `bytes` is not a WAD3 file. Entries pointing outside the file are left out.
    pub fn parse(bytes: Vec<u8>) -> Option<Wad> {
        if bytes.len() < 12 || &bytes[..4] != b"WAD3" {
            return None;
        }
        let count = usize::try_from(i32_at(&bytes, 4)).ok()?;
        let table = usize::try_from(i32_at(&bytes, 8)).ok()?;
        let mut textures = BTreeMap::new();
        for i in 0..count {
            let Some(e) = table.checked_add(i * 32).and_then(|at| bytes.get(at..at + 32)) else {
                break;
            };
            let (pos, size) = (u32_at(e, 0) as usize, u32_at(e, 4) as usize);
            let (kind, compressed) = (e[12], e[13]);
            if kind != TYPE_MIPTEX || compressed != 0 || pos.checked_add(size).is_none_or(|end| end > bytes.len()) {
                continue;
            }
            let name_end = e[16..32].iter().position(|&c| c == 0).unwrap_or(16);
            let name = String::from_utf8_lossy(&e[16..16 + name_end]).to_ascii_lowercase();
            textures.entry(name).or_insert((pos, size));
        }
        Some(Wad { bytes, textures })
    }

    pub fn len(&self) -> usize {
        self.textures.len()
    }

    pub fn is_empty(&self) -> bool {
        self.textures.is_empty()
    }

    /// The texture `name` (any case), with its image.
    pub fn texture(&self, name: &str) -> Option<MipTex> {
        let &(pos, size) = self.textures.get(&name.to_ascii_lowercase())?;
        miptex::parse(&self.bytes[pos..pos + size]).filter(|t| t.image.is_some())
    }
}

/// File names of the WADs a map's worldspawn lists (`"wad" "\half-life\valve\halflife.wad;..."`), lowercase.
pub fn listed(wad_key: &str) -> Vec<String> {
    wad_key
        .split(';')
        .filter_map(|p| p.rsplit(['/', '\\']).next())
        .map(|n| n.trim().to_ascii_lowercase())
        .filter(|n| !n.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::miptex::{Image, encode};

    fn wad(entries: &[(&str, Vec<u8>, u8)]) -> Vec<u8> {
        let mut out = b"WAD3".to_vec();
        out.extend_from_slice(&(entries.len() as i32).to_le_bytes());
        out.extend_from_slice(&0i32.to_le_bytes());
        let mut dir = Vec::new();
        for (name, data, kind) in entries {
            let pos = out.len() as u32;
            out.extend_from_slice(data);
            let mut e = vec![0u8; 32];
            e[0..4].copy_from_slice(&pos.to_le_bytes());
            e[4..8].copy_from_slice(&(data.len() as u32).to_le_bytes());
            e[8..12].copy_from_slice(&(data.len() as u32).to_le_bytes());
            e[12] = *kind;
            e[16..16 + name.len()].copy_from_slice(name.as_bytes());
            dir.extend_from_slice(&e);
        }
        let table = out.len() as i32;
        out[8..12].copy_from_slice(&table.to_le_bytes());
        out.extend_from_slice(&dir);
        out
    }

    #[test]
    fn textures_are_found_by_name_in_any_case() {
        let img = Image {
            indices: vec![7; 16 * 16],
            palette: vec![[1, 2, 3]; 256],
        };
        let bytes = wad(&[
            ("CRATE01", encode("CRATE01", 16, 16, Some(&img)), TYPE_MIPTEX),
            ("CONCHARS", vec![0; 64], 0x42),
        ]);
        let w = Wad::parse(bytes).expect("a WAD");
        assert_eq!(w.len(), 1, "only mip textures");
        let t = w.texture("crate01").expect("found");
        assert_eq!(t.image, Some(img));
        assert!(w.texture("conchars").is_none());
    }

    #[test]
    fn a_broken_directory_is_not_a_panic() {
        let mut bytes = wad(&[("A", vec![0; 8], TYPE_MIPTEX)]);
        let n = bytes.len();
        bytes[n - 32..n - 28].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(Wad::parse(bytes).expect("a WAD").is_empty());
        assert!(Wad::parse(b"WAD2 no".to_vec()).is_none());
        for cut in 0..20 {
            let _ = Wad::parse(wad(&[("A", vec![0; 8], TYPE_MIPTEX)])[..cut].to_vec());
        }
    }

    #[test]
    fn the_wad_key_gives_file_names() {
        assert_eq!(
            listed(r"\half-life\valve\halflife.wad;c:/maps/My.wad;; liquids.wad"),
            ["halflife.wad", "my.wad", "liquids.wad"]
        );
    }
}
