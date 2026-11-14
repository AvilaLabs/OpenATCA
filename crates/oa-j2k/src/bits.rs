//! Packet-header bit reader with the bit-stuffing of T.800 B.10.1. Core (pure).
//!
//! Bits are read most significant first. After a byte of 0xFF the next byte carries only seven
//! bits (its top bit is a stuffed 0). A header ends on a byte boundary, and when its last byte is
//! 0xFF one more byte (a stuffed 0) follows it.

use crate::error::J2kError;

// A7.2: cold struct, size guard waived (one per packet header)
pub struct HeaderBits<'a> {
    data: &'a [u8],
    /// Index of the next byte to load.
    pos: usize,
    cur: u8,
    /// Bits of `cur` not yet handed out.
    left: u8,
    /// The byte last loaded was 0xFF.
    prev_ff: bool,
}

impl<'a> HeaderBits<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        HeaderBits {
            data,
            pos: 0,
            cur: 0,
            left: 0,
            prev_ff: false,
        }
    }

    pub fn bit(&mut self) -> Result<u32, J2kError> {
        if self.left == 0 {
            let b = *self
                .data
                .get(self.pos)
                .ok_or(J2kError::Truncated("packet header"))?;
            self.pos += 1;
            self.left = if self.prev_ff { 7 } else { 8 };
            if self.prev_ff && b & 0x80 != 0 {
                return Err(J2kError::BadPacket("stuffed bit after 0xFF is not 0"));
            }
            self.prev_ff = b == 0xFF;
            self.cur = b;
        }
        self.left -= 1;
        Ok(u32::from((self.cur >> self.left) & 1))
    }

    /// `n` bits (n <= 32), most significant first.
    pub fn bits(&mut self, n: u32) -> Result<u32, J2kError> {
        let mut v = 0u32;
        for _ in 0..n {
            v = (v << 1) | self.bit()?;
        }
        Ok(v)
    }

    /// Ends the header: drops the unread bits of the current byte, steps over the stuffed byte
    /// that follows a final 0xFF, and returns the number of bytes the header occupied.
    pub fn finish(mut self) -> Result<usize, J2kError> {
        if self.prev_ff {
            let b = *self
                .data
                .get(self.pos)
                .ok_or(J2kError::Truncated("packet header"))?;
            if b & 0x80 != 0 {
                return Err(J2kError::BadPacket("stuffed byte after 0xFF is not 0"));
            }
            self.pos += 1;
        }
        Ok(self.pos)
    }
}
