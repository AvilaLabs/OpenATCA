//! Bounds-checked big-endian readers. Core (pure). Every read returns `None` past the end, so no
//! caller can index out of range.

/// Big-endian unsigned integer of `n` bytes (n <= 8) at `pos`.
pub fn be(data: &[u8], pos: usize, n: usize) -> Option<u64> {
    let end = pos.checked_add(n)?;
    let bytes = data.get(pos..end)?;
    let mut v: u64 = 0;
    for &b in bytes {
        v = (v << 8) | u64::from(b);
    }
    Some(v)
}

pub fn u8_at(data: &[u8], pos: usize) -> Option<u8> {
    data.get(pos).copied()
}

pub fn u16_at(data: &[u8], pos: usize) -> Option<u16> {
    be(data, pos, 2).map(|v| v as u16)
}

pub fn u32_at(data: &[u8], pos: usize) -> Option<u32> {
    be(data, pos, 4).map(|v| v as u32)
}

/// GRIB signed integer of `n` bytes: the top bit is the sign, the rest the magnitude
/// (not two's complement).
pub fn sign_mag(data: &[u8], pos: usize, n: usize) -> Option<i64> {
    if n == 0 || n > 8 {
        return None;
    }
    let v = be(data, pos, n)?;
    let top = 1u64 << (n * 8 - 1);
    let mag = (v & (top - 1)) as i64;
    Some(if v & top != 0 { -mag } else { mag })
}

/// IEEE 754 binary32 at `pos`.
pub fn f32_at(data: &[u8], pos: usize) -> Option<f32> {
    u32_at(data, pos).map(f32::from_bits)
}

/// MSB-first reader of fields of 0..=32 bits.
// A7.2: cold struct, size guard waived
pub struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> BitReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        BitReader { data, pos: 0 }
    }

    /// Reads `n` bits (n <= 32). `None` when fewer remain.
    pub fn read(&mut self, n: u32) -> Option<u32> {
        if n > 32 {
            return None;
        }
        if n == 0 {
            return Some(0);
        }
        let end = self.pos.checked_add(n as usize)?;
        if end > self.data.len().checked_mul(8)? {
            return None;
        }
        let b0 = self.pos >> 3;
        let mut acc: u64 = 0;
        for k in 0..5 {
            acc = (acc << 8) | u64::from(self.data.get(b0 + k).copied().unwrap_or(0));
        }
        // 40-bit window; offset <= 7 and n <= 32 so the shift is at least 1.
        let shift = 40 - (self.pos & 7) as u32 - n;
        self.pos = end;
        Some(((acc >> shift) & ((1u64 << n) - 1)) as u32)
    }

    /// Skips to the next octet boundary.
    pub fn align(&mut self) {
        self.pos = self.pos.div_ceil(8) * 8;
    }

    /// Whole octets consumed so far (after [`align`](Self::align), exact).
    pub fn byte_pos(&self) -> usize {
        self.pos / 8
    }

    /// Bits left.
    pub fn remaining_bits(&self) -> usize {
        self.data.len() * 8 - self.pos.min(self.data.len() * 8)
    }
}
