//! yapb `.graph` files (yapb-halflife `storage.cpp`, `graph.h`, `crlib/ulz.h`): a 24-byte header, an ULZ-compressed
//! array of 220-byte node records and an optional 68-byte trailer with the author and the BSP size.

use lb_core::Vec3;

const MAGIC_BPAY: i32 = 0x5941_5042;
const MAGIC_UBOT: i32 = 0x544F_4255;
const OPTION_GRAPH: i32 = 8;
const OPTION_EXTEN: i32 = 64;
const RECORD: usize = 220;
pub const MAX_LINKS: usize = 8;
const MAX_NODES: i32 = 4096;
const MIN_MATCH: usize = 4;

/// yapb node flags (`NodeFlag`).
pub mod node_flags {
    pub const BUTTON: i32 = 1 << 0;
    pub const LIFT: i32 = 1 << 1;
    pub const CROUCH: i32 = 1 << 2;
    pub const GOAL: i32 = 1 << 4;
    pub const LADDER: i32 = 1 << 5;
    pub const CAMP: i32 = 1 << 7;
    pub const DOUBLE_JUMP: i32 = 1 << 9;
    pub const NARROW: i32 = 1 << 10;
    pub const SNIPER: i32 = 1 << 28;
}

/// yapb link flag (`PathFlag::Jump`).
pub const LINK_JUMP: u16 = 1;

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum YapbError {
    #[error("file is too short")]
    Truncated,
    #[error("not a yapb graph (magic {0:#x})")]
    Magic(i32),
    #[error("not a graph file (options {0:#x})")]
    NotGraph(i32),
    #[error("bad node count {0}")]
    Length(i32),
    #[error("uncompressed size {0} does not match {1} nodes")]
    Size(i32, i32),
    #[error("corrupt compressed data")]
    Decompress,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct YapbLink {
    pub index: i16,
    pub flags: u16,
    pub velocity: Vec3,
    pub distance: i32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct YapbNode {
    pub number: i32,
    pub flags: i32,
    /// Player origin (hull centre) where the node was placed.
    pub origin: Vec3,
    pub radius: f32,
    pub links: Vec<YapbLink>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct YapbGraph {
    pub version: i32,
    pub nodes: Vec<YapbNode>,
    pub author: Option<String>,
    /// Size of the BSP the graph was made for (trailer), to warn about a different map build.
    pub map_size: Option<i32>,
}

fn i32_at(b: &[u8], at: usize) -> i32 {
    i32::from_le_bytes(b[at..at + 4].try_into().expect("4 bytes"))
}

fn f32_at(b: &[u8], at: usize) -> f32 {
    f32::from_le_bytes(b[at..at + 4].try_into().expect("4 bytes"))
}

fn vec_at(b: &[u8], at: usize) -> Vec3 {
    Vec3::new(f32_at(b, at), f32_at(b, at + 4), f32_at(b, at + 8))
}

fn cstr(b: &[u8]) -> String {
    String::from_utf8_lossy(b.split(|c| *c == 0).next().unwrap_or(&[])).into_owned()
}

pub fn parse(bytes: &[u8]) -> Result<YapbGraph, YapbError> {
    if bytes.len() < 24 {
        return Err(YapbError::Truncated);
    }
    let magic = i32_at(bytes, 0);
    if magic != MAGIC_BPAY && magic != MAGIC_UBOT {
        return Err(YapbError::Magic(magic));
    }
    let version = i32_at(bytes, 4);
    let options = i32_at(bytes, 8);
    let length = i32_at(bytes, 12);
    let compressed = i32_at(bytes, 16);
    let uncompressed = i32_at(bytes, 20);
    if options & OPTION_GRAPH == 0 {
        return Err(YapbError::NotGraph(options));
    }
    if !(1..=MAX_NODES).contains(&length) {
        return Err(YapbError::Length(length));
    }
    if uncompressed as i64 != i64::from(length) * RECORD as i64 {
        return Err(YapbError::Size(uncompressed, length));
    }
    let data_end = 24usize
        .checked_add(compressed.max(0) as usize)
        .ok_or(YapbError::Truncated)?;
    let data = bytes.get(24..data_end).ok_or(YapbError::Truncated)?;
    let raw = ulz_uncompress(data, uncompressed as usize).ok_or(YapbError::Decompress)?;
    let nodes = (0..length as usize)
        .map(|i| {
            let r = &raw[i * RECORD..(i + 1) * RECORD];
            let links = (0..MAX_LINKS)
                .map(|l| {
                    let at = 56 + l * 20;
                    YapbLink {
                        velocity: vec_at(r, at),
                        distance: i32_at(r, at + 12),
                        flags: u16::from_le_bytes([r[at + 16], r[at + 17]]),
                        index: i16::from_le_bytes([r[at + 18], r[at + 19]]),
                    }
                })
                .filter(|l| l.index >= 0)
                .collect();
            YapbNode {
                number: i32_at(r, 0),
                flags: i32_at(r, 4),
                origin: vec_at(r, 8),
                radius: f32_at(r, 44),
                links,
            }
        })
        .collect();
    let (author, map_size) = if options & OPTION_EXTEN != 0 {
        match bytes.get(data_end..data_end + 68) {
            Some(t) => (Some(cstr(&t[..32])), Some(i32_at(t, 32))),
            None => (None, None),
        }
    } else {
        (None, None)
    };
    Ok(YapbGraph {
        version,
        nodes,
        author,
        map_size,
    })
}

/// Bounds-checked port of `ULZ::uncompress`. The original copies in 8-byte chunks; for the distances it uses that is
/// the same as a forward byte copy.
pub fn ulz_uncompress(input: &[u8], out_len: usize) -> Option<Vec<u8>> {
    let mut out: Vec<u8> = Vec::with_capacity(out_len);
    let mut ip = 0usize;
    let varint = |ip: &mut usize| -> Option<usize> {
        let mut val: u32 = 0;
        let mut shift = 0;
        while shift <= 21 {
            let cur = u32::from(*input.get(*ip)?);
            *ip += 1;
            val = val.wrapping_add(cur << shift);
            if cur < 128 {
                break;
            }
            shift += 7;
        }
        Some(val as usize)
    };
    while ip < input.len() {
        let token = input[ip];
        ip += 1;
        if token >= 32 {
            let mut run = usize::from(token >> 5);
            if run == 7 {
                run += varint(&mut ip)?;
            }
            if out_len - out.len() < run || input.len() - ip < run {
                return None;
            }
            out.extend_from_slice(&input[ip..ip + run]);
            ip += run;
            if ip >= input.len() {
                break;
            }
        }
        let mut length = usize::from(token & 15) + MIN_MATCH;
        if length == 15 + MIN_MATCH {
            length += varint(&mut ip)?;
        }
        if out_len - out.len() < length {
            return None;
        }
        let lo = *input.get(ip)?;
        let hi = *input.get(ip + 1)?;
        ip += 2;
        let dist = (usize::from(token & 16) << 12) + usize::from(u16::from_le_bytes([lo, hi]));
        if dist == 0 || dist > out.len() {
            return None;
        }
        for _ in 0..length {
            out.push(out[out.len() - dist]);
        }
    }
    (ip == input.len() && out.len() == out_len).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ulz_literals_and_matches() {
        // Token 0x60: 3 literals "abc", then a match of (0 & 15) + 4 = 4 bytes at distance 3 -> "abcabca";
        // token 0x20: 1 literal "z", end of input.
        let stream = [0x60u8, b'a', b'b', b'c', 0x03, 0x00, 0x20, b'z'];
        assert_eq!(ulz_uncompress(&stream, 8).unwrap(), b"abcabcaz");
        assert!(
            ulz_uncompress(&stream, 7).is_none(),
            "output longer than expected is rejected"
        );
    }

    #[test]
    fn ulz_rejects_bad_distances() {
        assert!(
            ulz_uncompress(&[0x20, b'a', 0x05, 0x00], 5).is_none(),
            "distance beyond output"
        );
        assert!(ulz_uncompress(&[0x20, b'a', 0x00, 0x00], 5).is_none(), "zero distance");
    }

    #[test]
    fn crossfire_fixture() {
        let Some(dir) = lb_bsp::test_maps_dir() else { return };
        let path = dir.join("../addons/yapb/data/graph/crossfire.graph");
        let Ok(bytes) = std::fs::read(&path) else { return };
        let g = parse(&bytes).unwrap();
        assert_eq!(g.nodes.len(), 1598);
        assert_eq!(g.map_size, Some(1_241_704));
        assert!(
            g.nodes
                .iter()
                .all(|n| n.links.iter().all(|l| (l.index as usize) < g.nodes.len()))
        );
        assert!(g.nodes.iter().any(|n| n.flags & node_flags::LADDER != 0));
    }
}
