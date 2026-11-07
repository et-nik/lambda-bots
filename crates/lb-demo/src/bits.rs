//! The bit stream of a GoldSrc network message: least significant bit first, bytes being 8-bit reads.

use crate::Error;

pub struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Bits<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn bit_pos(&self) -> usize {
        self.pos
    }

    pub fn byte_pos(&self) -> usize {
        self.pos.div_ceil(8)
    }

    pub fn remaining_bits(&self) -> usize {
        (self.data.len() * 8).saturating_sub(self.pos)
    }

    pub fn at_end(&self) -> bool {
        self.remaining_bits() < 8
    }

    /// The engine's `EndBitMode`: the next read starts at a whole byte.
    pub fn align(&mut self) {
        self.pos = self.pos.div_ceil(8) * 8;
    }

    pub fn read_bits(&mut self, n: u32) -> Result<u32, Error> {
        debug_assert!(n <= 32);
        if n == 0 {
            return Ok(0);
        }
        let n = n as usize;
        if n > self.remaining_bits() {
            return Err(Error::Overflow);
        }
        let mut out: u64 = 0;
        let mut got = 0;
        let mut pos = self.pos;
        while got < n {
            let bit = pos % 8;
            let take = (8 - bit).min(n - got);
            let chunk = (u64::from(self.data[pos / 8]) >> bit) & ((1 << take) - 1);
            out |= chunk << got;
            got += take;
            pos += take;
        }
        self.pos = pos;
        Ok(out as u32)
    }

    pub fn peek_bits(&self, n: u32) -> Result<u32, Error> {
        let mut copy = Bits {
            data: self.data,
            pos: self.pos,
        };
        copy.read_bits(n)
    }

    pub fn read_bit(&mut self) -> Result<bool, Error> {
        Ok(self.read_bits(1)? != 0)
    }

    /// A sign bit, then the magnitude in `n - 1` bits.
    pub fn read_sbits(&mut self, n: u32) -> Result<i32, Error> {
        let negative = self.read_bit()?;
        let magnitude = self.read_bits(n.saturating_sub(1))? as i32;
        Ok(if negative { -magnitude } else { magnitude })
    }

    pub fn skip_bits(&mut self, n: usize) -> Result<(), Error> {
        if n > self.remaining_bits() {
            return Err(Error::Overflow);
        }
        self.pos += n;
        Ok(())
    }

    pub fn skip_bytes(&mut self, n: usize) -> Result<(), Error> {
        self.skip_bits(n * 8)
    }

    pub fn read_u8(&mut self) -> Result<u8, Error> {
        Ok(self.read_bits(8)? as u8)
    }

    pub fn read_i8(&mut self) -> Result<i8, Error> {
        Ok(self.read_bits(8)? as u8 as i8)
    }

    pub fn read_u16(&mut self) -> Result<u16, Error> {
        Ok(self.read_bits(16)? as u16)
    }

    pub fn read_i16(&mut self) -> Result<i16, Error> {
        Ok(self.read_bits(16)? as u16 as i16)
    }

    pub fn read_u32(&mut self) -> Result<u32, Error> {
        self.read_bits(32)
    }

    pub fn read_i32(&mut self) -> Result<i32, Error> {
        Ok(self.read_bits(32)? as i32)
    }

    pub fn read_f32(&mut self) -> Result<f32, Error> {
        Ok(f32::from_bits(self.read_bits(32)?))
    }

    /// A coordinate of a byte-aligned message: a short in eighths of a unit.
    pub fn read_coord(&mut self) -> Result<f32, Error> {
        Ok(f32::from(self.read_i16()?) / 8.0)
    }

    /// Bytes up to the terminating zero, which is read and dropped.
    pub fn read_cstring(&mut self) -> Result<Vec<u8>, Error> {
        let mut out = Vec::new();
        loop {
            let c = self.read_u8()?;
            if c == 0 {
                return Ok(out);
            }
            out.push(c);
        }
    }

    pub fn read_string(&mut self) -> Result<String, Error> {
        Ok(latin1(&self.read_cstring()?))
    }

    pub fn read_bytes(&mut self, n: usize) -> Result<Vec<u8>, Error> {
        if n * 8 > self.remaining_bits() {
            return Err(Error::Overflow);
        }
        (0..n).map(|_| self.read_u8()).collect()
    }

    pub fn read_bit_coord(&mut self) -> Result<f32, Error> {
        let int_part = self.read_bit()?;
        let fract_part = self.read_bit()?;
        if !int_part && !fract_part {
            return Ok(0.0);
        }
        let negative = self.read_bit()?;
        let int_value = if int_part { self.read_bits(12)? } else { 0 };
        let fract_value = if fract_part { self.read_bits(3)? } else { 0 };
        let value = int_value as f32 + fract_value as f32 / 8.0;
        Ok(if negative { -value } else { value })
    }

    pub fn read_bit_vec3_coord(&mut self) -> Result<[f32; 3], Error> {
        let flags = [self.read_bit()?, self.read_bit()?, self.read_bit()?];
        let mut out = [0.0; 3];
        for (axis, sent) in flags.into_iter().enumerate() {
            if sent {
                out[axis] = self.read_bit_coord()?;
            }
        }
        Ok(out)
    }
}

/// Bytes as Latin-1 text: every byte keeps its value, so nothing is lost before the caller decides on an encoding.
pub fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| char::from(b)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_least_significant_bit_first() {
        let data = [0b1010_1101u8, 0b0000_0011];
        let mut b = Bits::new(&data);
        assert_eq!(b.read_bits(3).unwrap(), 0b101);
        assert_eq!(b.read_bits(7).unwrap(), 0b11_10101);
        assert_eq!(b.remaining_bits(), 6);
        b.align();
        assert_eq!(b.remaining_bits(), 0);
        assert!(b.read_bit().is_err());
    }

    #[test]
    fn signed_bits_are_sign_then_magnitude() {
        let data = [0b0000_1011u8];
        let mut b = Bits::new(&data);
        assert_eq!(b.read_sbits(4).unwrap(), -5);
    }

    #[test]
    fn reads_32_bits_across_bytes() {
        let data = [0xffu8, 0x78, 0x56, 0x34, 0x12, 0x00];
        let mut b = Bits::new(&data);
        b.read_bits(8).unwrap();
        assert_eq!(b.read_u32().unwrap(), 0x1234_5678);
    }
}
