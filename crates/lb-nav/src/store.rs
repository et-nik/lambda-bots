//! The `.lbnav` file: a navigation graph and the key of what it was made from (the BSP, the generator, the physics,
//! the rules and the map's overlay). A graph whose key differs from the one wanted is stale.
//!
//! Layout: magic, format version (u32 LE), key length and postcard key, graph length and the postcard graph
//! compressed with LZ4, then CRC-32C of everything before it.

use serde::{Deserialize, Serialize};

use crate::graph::NavGraph;

pub const MAGIC: &[u8; 8] = b"LBNAV\0\r\n";
/// Bumped when the layout or the graph types change.
pub const FORMAT: u32 = 1;

/// What a graph was made from; a graph is reused only for exactly the same key.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GraphKey {
    /// BLAKE3 of the BSP file and its size.
    pub bsp: [u8; 32],
    pub bsp_size: u64,
    /// Version of the generator: one that makes other graphs from the same map bumps it.
    pub generator: u32,
    /// Hashes of the player physics, the server rules that change links (fall damage) and the map's overlay.
    pub physics: u64,
    pub rules: u64,
    pub overlay: u64,
}

impl GraphKey {
    /// File name for the key: its hash in hex.
    pub fn file_name(&self) -> String {
        let bytes = postcard::to_allocvec(self).unwrap_or_default();
        format!("{:016x}.lbnav", xxhash_rust::xxh3::xxh3_64(&bytes))
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum StoreError {
    #[error("not a .lbnav file")]
    NotLbnav,
    #[error(".lbnav format {0}, this build reads {FORMAT}")]
    Format(u32),
    #[error("the file is cut short")]
    Truncated,
    #[error("checksum mismatch: the file is damaged")]
    Checksum,
    #[error("cannot decode: {0}")]
    Decode(String),
}

/// Encodes a graph with its key.
pub fn write(graph: &NavGraph, key: &GraphKey) -> Vec<u8> {
    let key_bytes = postcard::to_allocvec(key).expect("the key encodes");
    let graph_bytes = lz4_flex::compress_prepend_size(&postcard::to_allocvec(graph).expect("the graph encodes"));
    let mut out = Vec::with_capacity(24 + key_bytes.len() + graph_bytes.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&FORMAT.to_le_bytes());
    out.extend_from_slice(&(key_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(&key_bytes);
    out.extend_from_slice(&(graph_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(&graph_bytes);
    let crc = crc32c::crc32c(&out);
    out.extend_from_slice(&crc.to_le_bytes());
    out
}

struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], StoreError> {
        let end = self
            .at
            .checked_add(n)
            .filter(|&e| e <= self.bytes.len())
            .ok_or(StoreError::Truncated)?;
        let out = &self.bytes[self.at..end];
        self.at = end;
        Ok(out)
    }

    fn u32(&mut self) -> Result<u32, StoreError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
}

/// Reads only the key, to tell whether the file is the one wanted without decoding the graph.
pub fn read_key(bytes: &[u8]) -> Result<GraphKey, StoreError> {
    let mut c = header(bytes)?;
    let len = c.u32()? as usize;
    postcard::from_bytes(c.take(len)?).map_err(|e| StoreError::Decode(e.to_string()))
}

/// A key is a few dozen bytes: a longer one is damage, not something to read.
const MAX_KEY: usize = 1024;

/// Reads only the key from the start of a `.lbnav` file, leaving the graph after it unread.
pub fn read_key_from(r: &mut impl std::io::Read) -> Result<GraphKey, StoreError> {
    let mut bytes = vec![0; MAGIC.len() + 8];
    r.read_exact(&mut bytes).map_err(|_| StoreError::Truncated)?;
    let len = header(&bytes)?.u32()? as usize;
    if len > MAX_KEY {
        return Err(StoreError::Decode(format!("a key of {len} bytes")));
    }
    let head = bytes.len();
    bytes.resize(head + len, 0);
    r.read_exact(&mut bytes[head..]).map_err(|_| StoreError::Truncated)?;
    read_key(&bytes)
}

fn header(bytes: &[u8]) -> Result<Cursor<'_>, StoreError> {
    let mut c = Cursor { bytes, at: 0 };
    if c.take(MAGIC.len()).map_err(|_| StoreError::NotLbnav)? != MAGIC {
        return Err(StoreError::NotLbnav);
    }
    let format = c.u32()?;
    if format != FORMAT {
        return Err(StoreError::Format(format));
    }
    Ok(c)
}

/// Decodes a graph and its key, checking the file is whole.
pub fn read(bytes: &[u8]) -> Result<(GraphKey, NavGraph), StoreError> {
    if bytes.len() < MAGIC.len() + 8 {
        return Err(StoreError::Truncated);
    }
    let (body, tail) = bytes.split_at(bytes.len() - 4);
    let crc = u32::from_le_bytes([tail[0], tail[1], tail[2], tail[3]]);
    let mut c = header(body)?;
    if crc32c::crc32c(body) != crc {
        return Err(StoreError::Checksum);
    }
    let len = c.u32()? as usize;
    let key: GraphKey = postcard::from_bytes(c.take(len)?).map_err(|e| StoreError::Decode(e.to_string()))?;
    let len = c.u32()? as usize;
    let packed = c.take(len)?;
    let raw = lz4_flex::decompress_size_prepended(packed).map_err(|e| StoreError::Decode(e.to_string()))?;
    let graph: NavGraph = postcard::from_bytes(&raw).map_err(|e| StoreError::Decode(e.to_string()))?;
    Ok((key, graph))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::LinkKind;
    use crate::plan::tests::graph;

    fn key() -> GraphKey {
        GraphKey {
            bsp: [7; 32],
            bsp_size: 1234,
            generator: 3,
            physics: 5,
            rules: 6,
            overlay: 0,
        }
    }

    #[test]
    fn round_trip_keeps_the_graph_and_the_key() {
        let g = graph(
            &[(0.0, 0.0), (100.0, 0.0)],
            &[(0, 1, LinkKind::Walk), (1, 0, LinkKind::Jump)],
        );
        let bytes = write(&g, &key());
        let (k, back) = read(&bytes).unwrap();
        assert_eq!(k, key());
        assert_eq!(back, g);
        assert_eq!(read_key(&bytes).unwrap(), key());
        let mut head = &bytes[..MAGIC.len() + 8 + postcard::to_allocvec(&key()).unwrap().len()];
        assert_eq!(read_key_from(&mut head).unwrap(), key());
    }

    #[test]
    fn damage_and_other_files_are_refused() {
        let g = graph(&[(0.0, 0.0), (100.0, 0.0)], &[(0, 1, LinkKind::Walk)]);
        let mut bytes = write(&g, &key());
        let mid = bytes.len() / 2;
        bytes[mid] ^= 0x40;
        assert_eq!(read(&bytes).unwrap_err(), StoreError::Checksum);
        assert_eq!(read(b"BPAY and some more bytes").unwrap_err(), StoreError::NotLbnav);
        let whole = write(&g, &key());
        assert_eq!(read(&whole[..whole.len() - 9]).unwrap_err(), StoreError::Checksum);
        let mut future = whole.clone();
        future[8] = 99;
        assert_eq!(read(&future).unwrap_err(), StoreError::Format(99));
        assert_eq!(read_key_from(&mut &future[..]).unwrap_err(), StoreError::Format(99));
        assert_eq!(read_key_from(&mut &whole[..20]).unwrap_err(), StoreError::Truncated);
        let mut huge = whole.clone();
        huge[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(read_key_from(&mut &huge[..]), Err(StoreError::Decode(_))));
    }

    #[test]
    fn every_part_of_the_key_names_another_file() {
        let base = key().file_name();
        let variants = [
            GraphKey {
                bsp_size: 1235,
                ..key()
            },
            GraphKey { generator: 4, ..key() },
            GraphKey { physics: 1, ..key() },
            GraphKey { rules: 1, ..key() },
            GraphKey { overlay: 1, ..key() },
            GraphKey { bsp: [8; 32], ..key() },
        ];
        for v in variants {
            assert_ne!(v.file_name(), base, "{v:?}");
        }
        assert_eq!(key().file_name(), base);
    }
}
