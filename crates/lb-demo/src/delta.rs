//! Delta-encoded structures. The server describes each structure field by field (`svc_deltadescription`); a delta
//! is a mask of the fields sent, then the sent fields. A decoded structure is kept as the bytes of the engine's C
//! struct at the server's offsets, so fields left out are copied from the base just as the engine copies them.

use rustc_hash::FxHashMap;

use crate::Error;
use crate::bits::Bits;

pub const DT_BYTE: u32 = 1 << 0;
pub const DT_SHORT: u32 = 1 << 1;
pub const DT_FLOAT: u32 = 1 << 2;
pub const DT_INTEGER: u32 = 1 << 3;
pub const DT_ANGLE: u32 = 1 << 4;
pub const DT_TIMEWINDOW_8: u32 = 1 << 5;
pub const DT_TIMEWINDOW_BIG: u32 = 1 << 6;
pub const DT_STRING: u32 = 1 << 7;
pub const DT_SIGNED: u32 = 1 << 31;

/// The engine sends at most 64 fields (a mask of up to eight bytes).
const MAX_FIELDS: usize = 64;

#[derive(Clone, Debug)]
pub struct Field {
    pub name: String,
    pub kind: u32,
    pub signed: bool,
    pub offset: usize,
    pub size: usize,
    pub bits: u32,
    pub premultiply: f64,
    pub postmultiply: f64,
}

impl Field {
    /// Bytes the field takes in the C struct.
    pub fn width(&self) -> usize {
        match self.kind {
            DT_BYTE => 1,
            DT_SHORT => 2,
            DT_STRING => self.size.max(1),
            _ => 4,
        }
    }

    fn scaled(&self, raw: f64) -> f64 {
        let mut d = raw;
        if self.premultiply != 0.0 && !(0.9999..=1.0001).contains(&self.premultiply) {
            d /= self.premultiply;
        }
        if !(0.9999..=1.0001).contains(&self.postmultiply) {
            d *= self.postmultiply;
        }
        d
    }

    fn read_number(&self, b: &mut Bits<'_>) -> Result<f64, Error> {
        let raw = if self.signed {
            f64::from(b.read_sbits(self.bits)?)
        } else {
            f64::from(b.read_bits(self.bits)?)
        };
        Ok(self.scaled(raw))
    }
}

#[derive(Clone, Debug, Default)]
pub struct Desc {
    pub fields: Vec<Field>,
}

impl Desc {
    /// Bytes a structure needs to hold every field.
    pub fn size(&self) -> usize {
        self.fields.iter().map(|f| f.offset + f.width()).max().unwrap_or(0)
    }

    pub fn field(&self, name: &str) -> Option<&Field> {
        self.fields.iter().find(|f| f.name == name)
    }

    /// Decodes one delta from `from` into `to` (both at least [`Desc::size`] long). `time` is the server time of
    /// the message, the base of the time-window fields.
    pub fn decode(&self, b: &mut Bits<'_>, from: &[u8], to: &mut [u8], time: f64) -> Result<(), Error> {
        let mask = read_mask(b)?;
        for (i, f) in self.fields.iter().enumerate() {
            let range = f.offset..f.offset + f.width();
            if range.end > to.len() || range.end > from.len() {
                return Err(Error::Delta(format!("field {} past the structure", f.name)));
            }
            if i >= MAX_FIELDS || mask & (1 << i) == 0 {
                to[range.clone()].copy_from_slice(&from[range]);
                continue;
            }
            match f.kind {
                DT_BYTE => {
                    let d = f.read_number(b)?;
                    to[f.offset] = if f.signed { (d as i8) as u8 } else { d as u8 };
                }
                DT_SHORT => {
                    let d = f.read_number(b)?;
                    let v = if f.signed { (d as i16) as u16 } else { d as u16 };
                    to[range].copy_from_slice(&v.to_le_bytes());
                }
                DT_FLOAT => {
                    let d = f.read_number(b)?;
                    to[range].copy_from_slice(&(d as f32).to_le_bytes());
                }
                DT_INTEGER => {
                    let d = f.read_number(b)?;
                    let v = if f.signed { (d as i32) as u32 } else { d as u32 };
                    to[range].copy_from_slice(&v.to_le_bytes());
                }
                DT_ANGLE => {
                    let raw = f64::from(b.read_bits(f.bits)?);
                    let v = (raw * (360.0 / f64::from(1u32 << f.bits.min(31)))) as f32;
                    to[range].copy_from_slice(&v.to_le_bytes());
                }
                DT_TIMEWINDOW_8 => {
                    let add = f64::from(b.read_sbits(8)?);
                    let v = ((time * 100.0 - add) / 100.0) as f32;
                    to[range].copy_from_slice(&v.to_le_bytes());
                }
                DT_TIMEWINDOW_BIG => {
                    let add = f64::from(b.read_sbits(f.bits)?);
                    let v = if (0.9999..=1.0001).contains(&f.premultiply) || f.premultiply == 0.0 {
                        (time - add) as f32
                    } else {
                        ((time * f.premultiply - add) / f.premultiply) as f32
                    };
                    to[range].copy_from_slice(&v.to_le_bytes());
                }
                DT_STRING => {
                    let s = b.read_cstring()?;
                    let dst = &mut to[range];
                    dst.fill(0);
                    let n = s.len().min(dst.len().saturating_sub(1));
                    dst[..n].copy_from_slice(&s[..n]);
                }
                other => return Err(Error::Delta(format!("field {} has unknown type {other:#x}", f.name))),
            }
        }
        Ok(())
    }

    /// Reads past one delta without keeping it (the values do not change which bits follow).
    pub fn skip(&self, b: &mut Bits<'_>) -> Result<(), Error> {
        let size = self.size();
        let zero = vec![0u8; size];
        let mut out = vec![0u8; size];
        self.decode(b, &zero, &mut out, 0.0)
    }
}

fn read_mask(b: &mut Bits<'_>) -> Result<u64, Error> {
    let nbytes = b.read_bits(3)?;
    let mut mask = 0u64;
    for i in 0..nbytes {
        mask |= u64::from(b.read_bits(8)?) << (8 * i);
    }
    Ok(mask)
}

/// One field description of `svc_deltadescription`, sent as a delta of the engine's own meta description against
/// a zeroed description: type (32 bits), name, offset (16), size (8), bits (8), then the two multipliers as 32-bit
/// integers in 1/4000ths.
pub fn read_field_description(b: &mut Bits<'_>) -> Result<Field, Error> {
    let mask = read_mask(b)?;
    let sent = |i: u32| mask & (1 << i) != 0;
    let field_type = if sent(0) { b.read_bits(32)? } else { 0 };
    let name = if sent(1) {
        crate::bits::latin1(&b.read_cstring()?)
    } else {
        String::new()
    };
    let offset = if sent(2) { b.read_bits(16)? } else { 0 };
    let size = if sent(3) { b.read_bits(8)? } else { 0 };
    let bits = if sent(4) { b.read_bits(8)? } else { 0 };
    let premultiply = if sent(5) {
        f64::from(b.read_bits(32)?) / 4000.0
    } else {
        0.0
    };
    let postmultiply = if sent(6) {
        f64::from(b.read_bits(32)?) / 4000.0
    } else {
        0.0
    };
    if bits > 32 {
        return Err(Error::Delta(format!("field {name} has {bits} bits")));
    }
    Ok(Field {
        name,
        kind: field_type & !DT_SIGNED,
        signed: field_type & DT_SIGNED != 0,
        offset: offset as usize,
        size: size as usize,
        bits,
        premultiply,
        postmultiply,
    })
}

/// The descriptions a recording carried, by name (`entity_state_t`, `clientdata_t`, `event_t`, ...).
#[derive(Default)]
pub struct Deltas {
    pub by_name: FxHashMap<String, Desc>,
}

impl Deltas {
    pub fn get(&self, name: &str) -> Result<&Desc, Error> {
        self.by_name
            .get(name)
            .ok_or_else(|| Error::Delta(format!("no description of {name}")))
    }
}

/// Where a named field lies in the bytes of a decoded structure.
#[derive(Clone, Copy, Debug)]
pub struct Slot {
    pub offset: usize,
    pub kind: u32,
    pub signed: bool,
}

/// The layout of a structure described by several descriptions (the three of `entity_state_t`).
#[derive(Clone, Debug, Default)]
pub struct Layout {
    slots: FxHashMap<String, Slot>,
    pub size: usize,
}

impl Layout {
    pub fn from_descs<'d>(descs: impl IntoIterator<Item = &'d Desc>) -> Self {
        let mut layout = Layout::default();
        for d in descs {
            layout.size = layout.size.max(d.size());
            for f in &d.fields {
                layout.slots.entry(f.name.clone()).or_insert(Slot {
                    offset: f.offset,
                    kind: f.kind,
                    signed: f.signed,
                });
            }
        }
        layout
    }

    pub fn slot(&self, name: &str) -> Option<Slot> {
        self.slots.get(name).copied()
    }
}

impl Slot {
    pub fn read_f32(self, bytes: &[u8]) -> f32 {
        match self.kind {
            DT_FLOAT | DT_ANGLE | DT_TIMEWINDOW_8 | DT_TIMEWINDOW_BIG => f32::from_le_bytes(word(bytes, self.offset)),
            _ => self.read_i64(bytes) as f32,
        }
    }

    pub fn read_i64(self, bytes: &[u8]) -> i64 {
        let o = self.offset;
        match (self.kind, self.signed) {
            (DT_BYTE, true) => i64::from(bytes.get(o).copied().unwrap_or(0) as i8),
            (DT_BYTE, false) => i64::from(bytes.get(o).copied().unwrap_or(0)),
            (DT_SHORT, true) => i64::from(i16::from_le_bytes(half(bytes, o))),
            (DT_SHORT, false) => i64::from(u16::from_le_bytes(half(bytes, o))),
            (DT_INTEGER, true) => i64::from(i32::from_le_bytes(word(bytes, o))),
            (DT_INTEGER, false) => i64::from(u32::from_le_bytes(word(bytes, o))),
            (DT_FLOAT | DT_ANGLE | DT_TIMEWINDOW_8 | DT_TIMEWINDOW_BIG, _) => f32::from_le_bytes(word(bytes, o)) as i64,
            _ => 0,
        }
    }
}

fn word(bytes: &[u8], o: usize) -> [u8; 4] {
    bytes.get(o..o + 4).and_then(|s| s.try_into().ok()).unwrap_or([0; 4])
}

fn half(bytes: &[u8], o: usize) -> [u8; 2] {
    bytes.get(o..o + 2).and_then(|s| s.try_into().ok()).unwrap_or([0; 2])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Writer {
        bytes: Vec<u8>,
        bit: usize,
    }

    impl Writer {
        fn put(&mut self, v: u32, n: u32) -> &mut Writer {
            for i in 0..n {
                if self.bit.is_multiple_of(8) {
                    self.bytes.push(0);
                }
                if (v >> i) & 1 == 1 {
                    *self.bytes.last_mut().unwrap() |= 1 << (self.bit % 8);
                }
                self.bit += 1;
            }
            self
        }
    }

    fn field(name: &str, kind: u32, offset: usize, bits: u32, premultiply: f64) -> Field {
        Field {
            name: name.into(),
            kind: kind & !DT_SIGNED,
            signed: kind & DT_SIGNED != 0,
            offset,
            size: 4,
            bits,
            premultiply,
            postmultiply: 1.0,
        }
    }

    fn desc() -> Desc {
        Desc {
            fields: vec![
                field("origin", DT_FLOAT | DT_SIGNED, 0, 16, 8.0),
                field("angle", DT_ANGLE, 4, 8, 1.0),
                field("frame", DT_INTEGER, 8, 8, 1.0),
                field("modelindex", DT_SHORT, 12, 10, 1.0),
            ],
        }
    }

    fn f32_at(bytes: &[u8], o: usize) -> f32 {
        f32::from_le_bytes(bytes[o..o + 4].try_into().unwrap())
    }

    #[test]
    fn a_delta_changes_the_fields_it_sends_and_keeps_the_rest() {
        let d = desc();
        let mut from = vec![0u8; d.size()];
        from[8..12].copy_from_slice(&7u32.to_le_bytes());
        from[12..14].copy_from_slice(&300u16.to_le_bytes());
        let mut w = Writer::default();
        // One mask byte: fields 0 and 1 sent.
        w.put(1, 3).put(0b0011, 8);
        // origin -12.5 sent as -100 (sign, then 15 bits of magnitude); angle 64/256 of a turn.
        w.put(1, 1).put(100, 15).put(64, 8);
        let mut to = vec![0u8; d.size()];
        d.decode(&mut Bits::new(&w.bytes), &from, &mut to, 10.0).unwrap();
        assert_eq!(f32_at(&to, 0), -12.5);
        assert_eq!(f32_at(&to, 4), 90.0);
        assert_eq!(u32::from_le_bytes(to[8..12].try_into().unwrap()), 7, "not sent: kept");
        assert_eq!(
            u16::from_le_bytes(to[12..14].try_into().unwrap()),
            300,
            "not sent: kept"
        );
    }

    #[test]
    fn skipping_a_delta_reads_the_same_bits() {
        let d = desc();
        let mut w = Writer::default();
        w.put(1, 3).put(0b1100, 8).put(5, 8).put(513, 10).put(0b101, 3);
        let mut b = Bits::new(&w.bytes);
        d.skip(&mut b).unwrap();
        assert_eq!(
            b.read_bits(3).unwrap(),
            0b101,
            "the next value starts where the delta ends"
        );
    }
}
