//! SHA-256 per FIPS 180-4, the hasher itself. Core (pure, B1, B7): a function of the bytes it is
//! given, no I/O, no allocation, no clock. It is a subsystem under D5 (it holds precomputed
//! chaining state and a partial block), so it keeps interface methods. Used for content
//! identity of snapshots and raw downloads (A8).

use std::fmt;

/// Block size in bytes (FIPS 180-4 section 1).
pub const BLOCK_LEN: usize = 64;
/// Digest size in bytes.
pub const DIGEST_LEN: usize = 32;
/// Limit: FIPS 180-4 caps a message at 2^64 - 1 bits, so at most this many whole bytes.
pub const MAX_MESSAGE_BYTES: u64 = u64::MAX / 8;

/// A 32-byte SHA-256 digest.
pub type Digest = [u8; DIGEST_LEN];

/// The message exceeded [`MAX_MESSAGE_BYTES`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// A7.2: cold struct, size guard waived: error value on the refusal path
pub struct MessageTooLong {
    pub limit: u64,
}

impl fmt::Display for MessageTooLong {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "sha256: message exceeds limit MAX_MESSAGE_BYTES ({} bytes)",
            self.limit
        )
    }
}

impl std::error::Error for MessageTooLong {}

const INIT: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// Streaming SHA-256 state.
#[derive(Clone, Debug)]
// A7.2: cold struct, size guard waived
pub struct Sha256 {
    h: [u32; 8],
    buf: [u8; BLOCK_LEN],
    buf_len: usize,
    pub(crate) total: u64,
}

impl Default for Sha256 {
    fn default() -> Self {
        Self::new()
    }
}

/// One round of the compression function. The eight working variables are passed in rotated
/// order, so no register shuffling happens between rounds: `d` and `h` are the only two updated.
macro_rules! round {
    ($a:ident, $b:ident, $c:ident, $d:ident, $e:ident, $f:ident, $g:ident, $h:ident, $k:expr, $w:expr) => {
        let t1 = $h
            .wrapping_add($e.rotate_right(6) ^ $e.rotate_right(11) ^ $e.rotate_right(25))
            .wrapping_add($g ^ ($e & ($f ^ $g)))
            .wrapping_add($k)
            .wrapping_add($w);
        $d = $d.wrapping_add(t1);
        $h = t1
            .wrapping_add($a.rotate_right(2) ^ $a.rotate_right(13) ^ $a.rotate_right(22))
            .wrapping_add(($a & $b) | ($c & ($a | $b)));
    };
}

/// Advance word `$i` of the 16-word rolling message schedule in place.
macro_rules! expand {
    ($w:ident, $i:literal, $i1:literal, $i9:literal, $i14:literal) => {
        let x = $w[$i1];
        let y = $w[$i14];
        $w[$i] = $w[$i]
            .wrapping_add(x.rotate_right(7) ^ x.rotate_right(18) ^ (x >> 3))
            .wrapping_add($w[$i9])
            .wrapping_add(y.rotate_right(17) ^ y.rotate_right(19) ^ (y >> 10));
    };
}

/// Process one 64-byte block into the chaining state.
///
/// The schedule lives in a 16-word array rolled in place, each word computed just before the
/// round that consumes it, and all 64 rounds are unrolled with the round constants as
/// immediates. Every index is a literal, so no bounds checks remain.
// E3 / H2: every index below is a literal into a `[u32; 16]` (below 16) or the 64-word `K`
// (below 64); the compiler checks literal indexes into arrays at build time, so none can go
// out of range.
#[allow(clippy::indexing_slicing)]
fn compress(h: &mut [u32; 8], block: &[u8; BLOCK_LEN]) {
    let mut w = [0u32; 16];
    let (words, _) = block.as_chunks::<4>();
    for (wi, c) in w.iter_mut().zip(words) {
        *wi = u32::from_be_bytes(*c);
    }
    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = *h;
    round!(a, b, c, d, e, f, g, hh, K[0], w[0]);
    round!(hh, a, b, c, d, e, f, g, K[1], w[1]);
    round!(g, hh, a, b, c, d, e, f, K[2], w[2]);
    round!(f, g, hh, a, b, c, d, e, K[3], w[3]);
    round!(e, f, g, hh, a, b, c, d, K[4], w[4]);
    round!(d, e, f, g, hh, a, b, c, K[5], w[5]);
    round!(c, d, e, f, g, hh, a, b, K[6], w[6]);
    round!(b, c, d, e, f, g, hh, a, K[7], w[7]);
    round!(a, b, c, d, e, f, g, hh, K[8], w[8]);
    round!(hh, a, b, c, d, e, f, g, K[9], w[9]);
    round!(g, hh, a, b, c, d, e, f, K[10], w[10]);
    round!(f, g, hh, a, b, c, d, e, K[11], w[11]);
    round!(e, f, g, hh, a, b, c, d, K[12], w[12]);
    round!(d, e, f, g, hh, a, b, c, K[13], w[13]);
    round!(c, d, e, f, g, hh, a, b, K[14], w[14]);
    round!(b, c, d, e, f, g, hh, a, K[15], w[15]);
    expand!(w, 0, 1, 9, 14);
    round!(a, b, c, d, e, f, g, hh, K[16], w[0]);
    expand!(w, 1, 2, 10, 15);
    round!(hh, a, b, c, d, e, f, g, K[17], w[1]);
    expand!(w, 2, 3, 11, 0);
    round!(g, hh, a, b, c, d, e, f, K[18], w[2]);
    expand!(w, 3, 4, 12, 1);
    round!(f, g, hh, a, b, c, d, e, K[19], w[3]);
    expand!(w, 4, 5, 13, 2);
    round!(e, f, g, hh, a, b, c, d, K[20], w[4]);
    expand!(w, 5, 6, 14, 3);
    round!(d, e, f, g, hh, a, b, c, K[21], w[5]);
    expand!(w, 6, 7, 15, 4);
    round!(c, d, e, f, g, hh, a, b, K[22], w[6]);
    expand!(w, 7, 8, 0, 5);
    round!(b, c, d, e, f, g, hh, a, K[23], w[7]);
    expand!(w, 8, 9, 1, 6);
    round!(a, b, c, d, e, f, g, hh, K[24], w[8]);
    expand!(w, 9, 10, 2, 7);
    round!(hh, a, b, c, d, e, f, g, K[25], w[9]);
    expand!(w, 10, 11, 3, 8);
    round!(g, hh, a, b, c, d, e, f, K[26], w[10]);
    expand!(w, 11, 12, 4, 9);
    round!(f, g, hh, a, b, c, d, e, K[27], w[11]);
    expand!(w, 12, 13, 5, 10);
    round!(e, f, g, hh, a, b, c, d, K[28], w[12]);
    expand!(w, 13, 14, 6, 11);
    round!(d, e, f, g, hh, a, b, c, K[29], w[13]);
    expand!(w, 14, 15, 7, 12);
    round!(c, d, e, f, g, hh, a, b, K[30], w[14]);
    expand!(w, 15, 0, 8, 13);
    round!(b, c, d, e, f, g, hh, a, K[31], w[15]);
    expand!(w, 0, 1, 9, 14);
    round!(a, b, c, d, e, f, g, hh, K[32], w[0]);
    expand!(w, 1, 2, 10, 15);
    round!(hh, a, b, c, d, e, f, g, K[33], w[1]);
    expand!(w, 2, 3, 11, 0);
    round!(g, hh, a, b, c, d, e, f, K[34], w[2]);
    expand!(w, 3, 4, 12, 1);
    round!(f, g, hh, a, b, c, d, e, K[35], w[3]);
    expand!(w, 4, 5, 13, 2);
    round!(e, f, g, hh, a, b, c, d, K[36], w[4]);
    expand!(w, 5, 6, 14, 3);
    round!(d, e, f, g, hh, a, b, c, K[37], w[5]);
    expand!(w, 6, 7, 15, 4);
    round!(c, d, e, f, g, hh, a, b, K[38], w[6]);
    expand!(w, 7, 8, 0, 5);
    round!(b, c, d, e, f, g, hh, a, K[39], w[7]);
    expand!(w, 8, 9, 1, 6);
    round!(a, b, c, d, e, f, g, hh, K[40], w[8]);
    expand!(w, 9, 10, 2, 7);
    round!(hh, a, b, c, d, e, f, g, K[41], w[9]);
    expand!(w, 10, 11, 3, 8);
    round!(g, hh, a, b, c, d, e, f, K[42], w[10]);
    expand!(w, 11, 12, 4, 9);
    round!(f, g, hh, a, b, c, d, e, K[43], w[11]);
    expand!(w, 12, 13, 5, 10);
    round!(e, f, g, hh, a, b, c, d, K[44], w[12]);
    expand!(w, 13, 14, 6, 11);
    round!(d, e, f, g, hh, a, b, c, K[45], w[13]);
    expand!(w, 14, 15, 7, 12);
    round!(c, d, e, f, g, hh, a, b, K[46], w[14]);
    expand!(w, 15, 0, 8, 13);
    round!(b, c, d, e, f, g, hh, a, K[47], w[15]);
    expand!(w, 0, 1, 9, 14);
    round!(a, b, c, d, e, f, g, hh, K[48], w[0]);
    expand!(w, 1, 2, 10, 15);
    round!(hh, a, b, c, d, e, f, g, K[49], w[1]);
    expand!(w, 2, 3, 11, 0);
    round!(g, hh, a, b, c, d, e, f, K[50], w[2]);
    expand!(w, 3, 4, 12, 1);
    round!(f, g, hh, a, b, c, d, e, K[51], w[3]);
    expand!(w, 4, 5, 13, 2);
    round!(e, f, g, hh, a, b, c, d, K[52], w[4]);
    expand!(w, 5, 6, 14, 3);
    round!(d, e, f, g, hh, a, b, c, K[53], w[5]);
    expand!(w, 6, 7, 15, 4);
    round!(c, d, e, f, g, hh, a, b, K[54], w[6]);
    expand!(w, 7, 8, 0, 5);
    round!(b, c, d, e, f, g, hh, a, K[55], w[7]);
    expand!(w, 8, 9, 1, 6);
    round!(a, b, c, d, e, f, g, hh, K[56], w[8]);
    expand!(w, 9, 10, 2, 7);
    round!(hh, a, b, c, d, e, f, g, K[57], w[9]);
    expand!(w, 10, 11, 3, 8);
    round!(g, hh, a, b, c, d, e, f, K[58], w[10]);
    expand!(w, 11, 12, 4, 9);
    round!(f, g, hh, a, b, c, d, e, K[59], w[11]);
    expand!(w, 12, 13, 5, 10);
    round!(e, f, g, hh, a, b, c, d, K[60], w[12]);
    expand!(w, 13, 14, 6, 11);
    round!(d, e, f, g, hh, a, b, c, K[61], w[13]);
    expand!(w, 14, 15, 7, 12);
    round!(c, d, e, f, g, hh, a, b, K[62], w[14]);
    expand!(w, 15, 0, 8, 13);
    round!(b, c, d, e, f, g, hh, a, K[63], w[15]);
    for (x, y) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
        *x = x.wrapping_add(y);
    }
}

impl Sha256 {
    /// A fresh hasher.
    pub const fn new() -> Self {
        Sha256 {
            h: INIT,
            buf: [0; BLOCK_LEN],
            buf_len: 0,
            total: 0,
        }
    }

    /// Absorb more message bytes.
    pub fn update(&mut self, mut data: &[u8]) -> Result<(), MessageTooLong> {
        let len = data.len() as u64;
        self.total = match self.total.checked_add(len) {
            Some(t) if t <= MAX_MESSAGE_BYTES => t,
            _ => {
                return Err(MessageTooLong {
                    limit: MAX_MESSAGE_BYTES,
                })
            }
        };
        if self.buf_len > 0 {
            let room = BLOCK_LEN - self.buf_len;
            let take = room.min(data.len());
            let (head, tail) = data.split_at_checked(take).unwrap_or((data, &[]));
            for (dst, src) in self.buf.iter_mut().skip(self.buf_len).zip(head) {
                *dst = *src;
            }
            self.buf_len += head.len();
            data = tail;
            if self.buf_len < BLOCK_LEN {
                return Ok(());
            }
            let block = self.buf;
            compress(&mut self.h, &block);
            self.buf_len = 0;
        }
        let (blocks, rest) = data.as_chunks::<BLOCK_LEN>();
        for block in blocks {
            compress(&mut self.h, block);
        }
        for (dst, src) in self.buf.iter_mut().zip(rest) {
            *dst = *src;
        }
        self.buf_len = rest.len();
        Ok(())
    }

    /// Pad and return the digest.
    pub fn finalize(mut self) -> Digest {
        let bits = self.total.wrapping_mul(8); // total <= MAX_MESSAGE_BYTES, cannot wrap
        let mut block = [0u8; BLOCK_LEN];
        for (dst, src) in block.iter_mut().zip(self.buf.iter().take(self.buf_len)) {
            *dst = *src;
        }
        if let Some(b) = block.get_mut(self.buf_len) {
            *b = 0x80;
        }
        if self.buf_len + 1 > BLOCK_LEN - 8 {
            compress(&mut self.h, &block);
            block = [0u8; BLOCK_LEN];
        }
        if let Some(len) = block.last_chunk_mut::<8>() {
            *len = bits.to_be_bytes();
        }
        compress(&mut self.h, &block);
        let mut out = [0u8; DIGEST_LEN];
        let (words, _) = out.as_chunks_mut::<4>();
        for (o, w) in words.iter_mut().zip(self.h) {
            *o = w.to_be_bytes();
        }
        out
    }
}

/// One-shot digest of a byte slice. Infallible: a slice cannot exceed [`MAX_MESSAGE_BYTES`].
pub fn sha256(data: &[u8]) -> Digest {
    let mut h = Sha256::new();
    // A slice length is at most isize::MAX, far below MAX_MESSAGE_BYTES, so this cannot fail.
    let _ = h.update(data);
    h.finalize()
}
