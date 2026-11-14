//! DEFLATE decompression per RFC 1951: stored, fixed Huffman and dynamic Huffman blocks.
//! Core (pure): bytes in, bytes out. Back-references reach at most [`WINDOW_LEN`] bytes back
//! and never before the start of the output.

use std::fmt;

/// RFC 1951 history window: the farthest back-reference distance.
pub const WINDOW_LEN: usize = 32 * 1024;
/// Longest Huffman code length RFC 1951 allows.
const MAX_CODE_LEN: usize = 15;
/// Bits resolved by the single-lookup fast table; longer codes take the canonical slow path.
const FAST_BITS: usize = 10;
const FAST_SIZE: usize = 1 << FAST_BITS;
/// RFC 1951 limits: 286 literal/length symbols may be used (288 exist), 30 distance symbols.
const MAX_LIT_SYMBOLS: usize = 288;
const MAX_USED_LIT: usize = 286;
const MAX_USED_DIST: usize = 30;
const END_OF_BLOCK: u16 = 256;

/// Inflate errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InflateError {
    /// The stream ended before the final block was complete.
    Truncated,
    /// Block type 3 (reserved).
    ReservedBlockType,
    /// Stored block LEN and NLEN are not complements.
    StoredLengthMismatch,
    /// A Huffman table was over-subscribed, incomplete or otherwise invalid.
    BadHuffmanTable(&'static str),
    /// A bit pattern matches no code in the table.
    InvalidCode,
    /// A length/distance symbol outside the used range (286, 287, 30, 31).
    InvalidSymbol,
    /// A back-reference reaches before the start of output or beyond [`WINDOW_LEN`].
    DistanceTooFar,
    /// The output would exceed the caller's limit (`max_output`).
    OutputLimit { max_output: usize },
}

impl fmt::Display for InflateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InflateError::Truncated => write!(f, "inflate: stream truncated"),
            InflateError::ReservedBlockType => write!(f, "inflate: reserved block type"),
            InflateError::StoredLengthMismatch => {
                write!(f, "inflate: stored block LEN/NLEN mismatch")
            }
            InflateError::BadHuffmanTable(w) => write!(f, "inflate: bad huffman table: {w}"),
            InflateError::InvalidCode => write!(f, "inflate: invalid huffman code"),
            InflateError::InvalidSymbol => write!(f, "inflate: invalid length/distance symbol"),
            InflateError::DistanceTooFar => write!(f, "inflate: back-reference too far"),
            InflateError::OutputLimit { max_output } => {
                write!(
                    f,
                    "inflate: output exceeds limit max_output ({max_output} bytes)"
                )
            }
        }
    }
}

impl std::error::Error for InflateError {}

/// LSB-first bit reader. Past the end of input it supplies virtual zero bits and reports
/// `Truncated` as soon as any of them is consumed.
// A7.2: cold struct, size guard waived
struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
    buf: u64,
    cnt: u32,
    /// Virtual (zero) bits sitting at the top of `buf`; consuming into them is an overrun.
    pad: u32,
}

impl<'a> Bits<'a> {
    fn new(data: &'a [u8]) -> Self {
        Bits {
            data,
            pos: 0,
            buf: 0,
            cnt: 0,
            pad: 0,
        }
    }

    fn refill(&mut self) {
        while self.cnt <= 56 {
            match self.data.get(self.pos) {
                Some(&b) => {
                    self.buf |= u64::from(b) << self.cnt;
                    self.pos += 1;
                }
                None => self.pad += 8,
            }
            self.cnt += 8;
        }
    }

    fn consume(&mut self, n: u32) -> Result<(), InflateError> {
        self.buf >>= n;
        self.cnt -= n; // n <= cnt: callers consume at most what refill provided
        if self.cnt < self.pad {
            return Err(InflateError::Truncated);
        }
        Ok(())
    }

    /// Read `n` (<= 16) bits.
    fn take(&mut self, n: u32) -> Result<u32, InflateError> {
        self.refill();
        let v = (self.buf & ((1u64 << n) - 1)) as u32;
        self.consume(n)?;
        Ok(v)
    }

    /// Drop buffered bits and rewind to the next byte boundary of the real input.
    fn align_and_rewind(&mut self) -> Result<(), InflateError> {
        let real = self
            .cnt
            .checked_sub(self.pad)
            .ok_or(InflateError::Truncated)?;
        let whole = (real / 8) as usize;
        self.pos = self.pos.checked_sub(whole).ok_or(InflateError::Truncated)?;
        self.buf = 0;
        self.cnt = 0;
        self.pad = 0;
        Ok(())
    }
}

/// Canonical Huffman decoder: a fast single-lookup table plus the counts/symbols form.
// A7.2: cold struct, size guard waived
struct Huffman {
    count: [u16; MAX_CODE_LEN + 1],
    symbol: [u16; MAX_LIT_SYMBOLS],
    /// `symbol << 4 | length`, indexed by the next `FAST_BITS` stream bits; 0 = not resolvable.
    fast: [u16; FAST_SIZE],
}

impl Huffman {
    // E3 / H2: `count`, `offs` and `next_code` have MAX_CODE_LEN + 2 entries and are indexed by a
    // code length that is 1..=MAX_CODE_LEN (lengths above it were rejected in the first loop) or
    // by 0; `fast` is indexed below FAST_SIZE by the loop condition.
    #[allow(clippy::indexing_slicing)]
    fn build(lengths: &[u8]) -> Result<Huffman, InflateError> {
        let mut h = Huffman {
            count: [0; MAX_CODE_LEN + 1],
            symbol: [0; MAX_LIT_SYMBOLS],
            fast: [0; FAST_SIZE],
        };
        if lengths.len() > MAX_LIT_SYMBOLS {
            return Err(InflateError::BadHuffmanTable("too many symbols"));
        }
        for &l in lengths {
            let slot = h
                .count
                .get_mut(usize::from(l))
                .ok_or(InflateError::BadHuffmanTable("code length above 15"))?;
            *slot += 1;
        }
        let used = lengths.len() - usize::from(h.count[0]);
        let mut left: i32 = 1;
        for len in 1..=MAX_CODE_LEN {
            left <<= 1;
            left -= i32::from(h.count[len]);
            if left < 0 {
                return Err(InflateError::BadHuffmanTable("over-subscribed"));
            }
        }
        // Incomplete codes are legal only for a lone code of length 1 (or no codes at all).
        if left > 0 && used != usize::from(h.count[1]) {
            return Err(InflateError::BadHuffmanTable("incomplete"));
        }
        let mut offs = [0u16; MAX_CODE_LEN + 2];
        for len in 1..=MAX_CODE_LEN {
            offs[len + 1] = offs[len] + h.count[len];
        }
        let mut next_code = [0u32; MAX_CODE_LEN + 2];
        let mut code = 0u32;
        let mut prev_count = 0u32; // codes of length 0 take no code space
        for (len, slot) in next_code
            .iter_mut()
            .enumerate()
            .take(MAX_CODE_LEN + 1)
            .skip(1)
        {
            code = (code + prev_count) << 1;
            *slot = code;
            prev_count = u32::from(h.count[len]);
        }
        for (sym, &l) in lengths.iter().enumerate() {
            let len = usize::from(l);
            if len == 0 {
                continue;
            }
            let o = offs[len];
            offs[len] += 1;
            if let Some(s) = h.symbol.get_mut(usize::from(o)) {
                *s = sym as u16; // sym < 288
            }
            let c = next_code[len];
            next_code[len] += 1;
            if len <= FAST_BITS {
                let rev = (c.reverse_bits() >> (32 - len)) as usize;
                let entry = ((sym as u16) << 4) | len as u16;
                let mut i = rev;
                while i < FAST_SIZE {
                    h.fast[i] = entry;
                    i += 1 << len;
                }
            }
        }
        Ok(h)
    }

    // E3 / H2: the `fast` index is masked to FAST_SIZE - 1 and `count` is indexed by a length in
    // 1..=MAX_CODE_LEN, both inside their arrays.
    #[allow(clippy::indexing_slicing)]
    fn decode(&self, bits: &mut Bits) -> Result<u16, InflateError> {
        bits.refill();
        let e = self.fast[(bits.buf as usize) & (FAST_SIZE - 1)];
        if e != 0 {
            bits.consume(u32::from(e & 15))?;
            return Ok(e >> 4);
        }
        // Slow path: canonical decode one bit at a time (puff-style).
        let mut code: i32 = 0;
        let mut first: i32 = 0;
        let mut index: i32 = 0;
        for len in 1..=MAX_CODE_LEN {
            code |= ((bits.buf >> (len - 1)) & 1) as i32;
            let count = i32::from(self.count[len]);
            if code - count < first {
                bits.consume(len as u32)?;
                let at = usize::try_from(index + (code - first))
                    .map_err(|_| InflateError::InvalidCode)?;
                return self
                    .symbol
                    .get(at)
                    .copied()
                    .ok_or(InflateError::InvalidCode);
            }
            index += count;
            first += count;
            first <<= 1;
            code <<= 1;
        }
        Err(InflateError::InvalidCode)
    }
}

const LEN_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LEN_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];
/// Order in which code-length code lengths are stored in a dynamic block header.
const CL_ORDER: [usize; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

fn fixed_tables() -> Result<(Huffman, Huffman), InflateError> {
    let mut l = [0u8; MAX_LIT_SYMBOLS];
    for (i, v) in l.iter_mut().enumerate() {
        *v = match i {
            0..=143 => 8,
            144..=255 => 9,
            256..=279 => 7,
            _ => 8,
        };
    }
    Ok((Huffman::build(&l)?, Huffman::build(&[5u8; 32])?))
}

fn dynamic_tables(bits: &mut Bits) -> Result<(Huffman, Huffman), InflateError> {
    let hlit = bits.take(5)? as usize + 257;
    let hdist = bits.take(5)? as usize + 1;
    let hclen = bits.take(4)? as usize + 4;
    if hlit > MAX_USED_LIT || hdist > MAX_USED_DIST {
        return Err(InflateError::BadHuffmanTable(
            "too many length/distance codes",
        ));
    }
    let mut cl = [0u8; 19];
    for &slot in CL_ORDER.iter().take(hclen) {
        let v = bits.take(3)? as u8;
        if let Some(c) = cl.get_mut(slot) {
            *c = v;
        }
    }
    let clh = Huffman::build(&cl)?;
    let mut lengths = [0u8; MAX_USED_LIT + MAX_USED_DIST];
    let total = hlit + hdist;
    let mut i = 0usize;
    while i < total {
        let sym = clh.decode(bits)?;
        let (val, rep) = match sym {
            0..=15 => (sym as u8, 1usize),
            16 => {
                let prev = i
                    .checked_sub(1)
                    .and_then(|p| lengths.get(p))
                    .copied()
                    .ok_or(InflateError::BadHuffmanTable(
                        "repeat with no previous length",
                    ))?;
                (prev, 3 + bits.take(2)? as usize)
            }
            17 => (0, 3 + bits.take(3)? as usize),
            _ => (0, 11 + bits.take(7)? as usize),
        };
        if i + rep > total {
            return Err(InflateError::BadHuffmanTable(
                "length repeat overruns table",
            ));
        }
        for slot in lengths.iter_mut().skip(i).take(rep) {
            *slot = val;
        }
        i += rep;
    }
    let (lit, dist) = lengths.split_at(hlit);
    if lit.get(usize::from(END_OF_BLOCK)).copied().unwrap_or(0) == 0 {
        return Err(InflateError::BadHuffmanTable("no end-of-block code"));
    }
    let dist = dist
        .get(..hdist)
        .ok_or(InflateError::BadHuffmanTable("distance table"))?;
    Ok((Huffman::build(lit)?, Huffman::build(dist)?))
}

fn inflate_block(
    bits: &mut Bits,
    lit: &Huffman,
    dist: &Huffman,
    out: &mut Vec<u8>,
    max_output: usize,
) -> Result<(), InflateError> {
    loop {
        let sym = lit.decode(bits)?;
        if sym < END_OF_BLOCK {
            if out.len() >= max_output {
                return Err(InflateError::OutputLimit { max_output });
            }
            out.push(sym as u8); // sym < 256
            continue;
        }
        if sym == END_OF_BLOCK {
            return Ok(());
        }
        let li = usize::from(sym - 257);
        let base = *LEN_BASE.get(li).ok_or(InflateError::InvalidSymbol)?;
        let extra = *LEN_EXTRA.get(li).ok_or(InflateError::InvalidSymbol)?;
        let len = usize::from(base) + bits.take(u32::from(extra))? as usize;
        let ds = usize::from(dist.decode(bits)?);
        let dbase = *DIST_BASE.get(ds).ok_or(InflateError::InvalidSymbol)?;
        let dextra = *DIST_EXTRA.get(ds).ok_or(InflateError::InvalidSymbol)?;
        let d = usize::from(dbase) + bits.take(u32::from(dextra))? as usize;
        if d > WINDOW_LEN || d > out.len() {
            return Err(InflateError::DistanceTooFar);
        }
        if len > max_output.saturating_sub(out.len()) {
            return Err(InflateError::OutputLimit { max_output });
        }
        let start = out.len() - d; // d <= out.len()
        if d >= len {
            out.extend_from_within(start..start + len);
        } else {
            // Overlapping copy: the pattern repeats, so copy byte by byte.
            for k in 0..len {
                let b = out
                    .get(start + k)
                    .copied()
                    .ok_or(InflateError::DistanceTooFar)?;
                out.push(b);
            }
        }
    }
}

/// Inflate a raw DEFLATE stream. `size_hint` pre-sizes the output (capped at `max_output`);
/// output beyond `max_output` bytes is an error, so a hostile stream cannot grow memory without
/// bound. Bytes after the final block are ignored.
pub fn inflate(input: &[u8], size_hint: usize, max_output: usize) -> Result<Vec<u8>, InflateError> {
    let mut out: Vec<u8> = Vec::with_capacity(size_hint.min(max_output));
    let mut bits = Bits::new(input);
    let mut fixed: Option<(Huffman, Huffman)> = None;
    loop {
        let hdr = bits.take(3)?;
        let last = hdr & 1 == 1;
        match hdr >> 1 {
            0 => {
                bits.align_and_rewind()?;
                let h: [u8; 4] = input
                    .get(bits.pos..bits.pos.saturating_add(4))
                    .and_then(|s| s.try_into().ok())
                    .ok_or(InflateError::Truncated)?;
                let len = u16::from_le_bytes([h[0], h[1]]);
                let nlen = u16::from_le_bytes([h[2], h[3]]);
                if len != !nlen {
                    return Err(InflateError::StoredLengthMismatch);
                }
                let start = bits.pos + 4;
                let len = usize::from(len);
                let body = input
                    .get(start..start.saturating_add(len))
                    .ok_or(InflateError::Truncated)?;
                if len > max_output.saturating_sub(out.len()) {
                    return Err(InflateError::OutputLimit { max_output });
                }
                out.extend_from_slice(body);
                bits.pos = start + len;
            }
            1 => {
                if fixed.is_none() {
                    fixed = Some(fixed_tables()?);
                }
                if let Some((l, d)) = fixed.as_ref() {
                    inflate_block(&mut bits, l, d, &mut out, max_output)?;
                }
            }
            2 => {
                let (l, d) = dynamic_tables(&mut bits)?;
                inflate_block(&mut bits, &l, &d, &mut out, max_output)?;
            }
            _ => return Err(InflateError::ReservedBlockType),
        }
        if last {
            return Ok(out);
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    // REQ: OA-ZIP-002
    #[test]
    fn stored_block() {
        let s = [0x01, 0x03, 0x00, 0xfc, 0xff, b'a', b'b', b'c'];
        assert_eq!(inflate(&s, 3, 100).unwrap(), b"abc");
    }

    // REQ: OA-ZIP-002
    #[test]
    fn fixed_block_empty_and_hello() {
        assert_eq!(inflate(&[0x03, 0x00], 0, 10).unwrap(), b"");
        // "hello" compressed with fixed Huffman (zlib level 9, raw).
        let h = [0xcb, 0x48, 0xcd, 0xc9, 0xc9, 0x07, 0x00];
        assert_eq!(inflate(&h, 5, 10).unwrap(), b"hello");
    }

    // REQ: OA-ZIP-002
    #[test]
    fn errors_not_panics() {
        assert_eq!(inflate(&[], 0, 10), Err(InflateError::Truncated));
        assert_eq!(
            inflate(&[0x07], 0, 10),
            Err(InflateError::ReservedBlockType)
        );
        assert_eq!(
            inflate(&[0x01, 0x03, 0x00, 0x00, 0x00], 0, 10),
            Err(InflateError::StoredLengthMismatch)
        );
    }

    // REQ: OA-ZIP-002, OA-ZIP-003
    #[test]
    fn output_limit_enforced() {
        let s = [0x01, 0x03, 0x00, 0xfc, 0xff, b'a', b'b', b'c'];
        assert_eq!(
            inflate(&s, 0, 2),
            Err(InflateError::OutputLimit { max_output: 2 })
        );
    }
}
