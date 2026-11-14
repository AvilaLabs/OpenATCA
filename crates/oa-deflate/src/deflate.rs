//! The DEFLATE compressor (RFC 1951): hash-chain LZ77 with greedy (levels 1-3) or lazy (levels
//! 4-9) matching, then dynamic, fixed or stored blocks, whichever is smallest. Core (pure): bytes
//! in through [`Compressor::write`], compressed bytes out through the caller's closure.
//!
//! Memory is allocated once in [`Compressor::new`] and never again. Capacities (C2):
//! - `buf`, 8 x the window = 256 KiB: input is copied in and slid down by whole windows when
//!   full. Eight windows, not two, so one block can span ~224 KiB of input; at 30x compression
//!   that keeps the block header (~100 bytes) under 2 % of the output.
//! - `head`/`prev`: hash table of 2^15 chain heads and one chain link per window position.
//! - `tok_*`: [`TOK`] tokens per block. A full block is emitted and the arrays reused.
//! - `ob`: output staging; drained to the caller's closure when nearly full and at flush.
//!
//! Hitting any capacity is a defined event: a full token array or a full buffer emits a block
//! or slides the window. Nothing reallocates, panics or drops data.
//!
//! A closure error aborts the call and leaves the compressor in an unspecified state; discard
//! it or call [`Compressor::reset`].

use crate::huffman::{assign_codes, build_lengths};

/// RFC 1951 history window: the farthest back-reference distance.
pub const WINDOW: usize = 32 * 1024;
const WMASK: usize = WINDOW - 1;
/// Input buffer, 8 windows. Always slid by a multiple of [`WINDOW`] so `pos & WMASK` chain slots
/// keep their meaning across a slide.
const BUF: usize = 8 * WINDOW;
const MIN_MATCH: usize = 3;
const MAX_MATCH: usize = 258;
/// Lookahead needed before a position is processed in streaming mode: a full match plus the
/// next position's 3 hash bytes and one more for the lazy comparison.
const MIN_LA: usize = MAX_MATCH + MIN_MATCH + 1;
const HASH_BITS: u32 = 15;
const HASH_SIZE: usize = 1 << HASH_BITS;
const NIL: u32 = u32::MAX;
/// Matches of length 3 farther than this cost more than three literals on average.
const TOO_FAR: usize = 4096;
/// Tokens per block.
const TOK: usize = 1 << 15;
/// Output staging buffer size and the room kept free for one token or one block header.
const OB: usize = 32 * 1024;
const OB_SLACK: usize = 1024;
const END_OF_BLOCK: usize = 256;
const NUM_LIT: usize = 286;
const NUM_DIST: usize = 30;

const LEN_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LEN_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
/// Order in which code-length code lengths are stored (RFC 1951 3.2.7).
const CLEN_ORDER: [usize; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

/// Extra bits following each code-length symbol (16 repeats, 17 and 18 are zero runs).
const CLEN_EXTRA: [u8; 19] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 3, 7];

/// Length symbol index (0..29) for a match length minus 3, built at compile time (F2).
const LEN_SYM: [u8; 256] = build_len_sym();

// E3 / H2: compile-time only; an out-of-range index is a build error, not a runtime panic.
#[allow(clippy::indexing_slicing)]
const fn build_len_sym() -> [u8; 256] {
    let mut t = [0u8; 256];
    let mut l = 0;
    while l < 256 {
        let len = l as u16 + 3;
        let mut s = 28;
        while LEN_BASE[s] > len {
            s -= 1;
        }
        t[l] = s as u8;
        l += 1;
    }
    t
}

/// Per-level search effort (zlib's table): `good` halves the chain once a match this long is
/// in hand, `lazy` is the longest match still worth looking past (greedy levels: the longest
/// match whose interior positions are still hashed), `nice` ends the search, `chain` caps it.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy)]
struct Config {
    good: usize,
    lazy: usize,
    nice: usize,
    chain: usize,
    greedy: bool,
}

const CONFIGS: [Config; 9] = [
    Config {
        good: 4,
        lazy: 4,
        nice: 8,
        chain: 4,
        greedy: true,
    },
    Config {
        good: 4,
        lazy: 5,
        nice: 16,
        chain: 8,
        greedy: true,
    },
    Config {
        good: 4,
        lazy: 6,
        nice: 32,
        chain: 32,
        greedy: true,
    },
    Config {
        good: 4,
        lazy: 4,
        nice: 16,
        chain: 16,
        greedy: false,
    },
    Config {
        good: 8,
        lazy: 16,
        nice: 32,
        chain: 32,
        greedy: false,
    },
    Config {
        good: 8,
        lazy: 16,
        nice: 128,
        chain: 128,
        greedy: false,
    },
    Config {
        good: 8,
        lazy: 32,
        nice: 128,
        chain: 256,
        greedy: false,
    },
    Config {
        good: 32,
        lazy: 128,
        nice: 258,
        chain: 1024,
        greedy: false,
    },
    Config {
        good: 32,
        lazy: 258,
        nice: 258,
        chain: 4096,
        greedy: false,
    },
];

/// A streaming DEFLATE compressor. A subsystem under D5: it owns its state and keeps methods.
// A7.2: cold struct (one per stream), size guard waived
pub struct Compressor {
    level: u8,
    cfg: Config,
    buf: Box<[u8]>,
    filled: usize,
    strstart: usize,
    head: Box<[u32]>,
    prev: Box<[u32]>,
    // Lazy-match state carried between calls: the match found at `strstart - 1`.
    match_len: usize,
    match_dist: usize,
    match_avail: bool,
    // Current block: tokens (dist 0 = literal in `tok_val`; else `tok_val` = length - 3).
    tok_dist: Box<[u16]>,
    tok_val: Box<[u16]>,
    tok_n: usize,
    lit_freq: [u32; NUM_LIT],
    dist_freq: [u32; NUM_DIST],
    /// Buffer index of the first byte of the current block, and how many bytes its tokens cover.
    block_start: usize,
    blk_raw: usize,
    /// Input received since the last sync flush.
    dirty: bool,
    // Code tables of the block being emitted.
    lit_len: [u8; 288],
    lit_code: [u16; 288],
    dist_len: [u8; NUM_DIST],
    dist_code: [u16; NUM_DIST],
    // Bit writer.
    acc: u64,
    nbits: u32,
    ob: Box<[u8]>,
    ob_len: usize,
}

/// Greedy/lazy level range accepted by [`Compressor::new`].
pub const MIN_LEVEL: u8 = 1;
pub const MAX_LEVEL: u8 = 9;

impl Compressor {
    /// Allocate all state. `level` is clamped to 1..=9 (1 fastest, 6 default, 9 best).
    pub fn new(level: u8) -> Compressor {
        let level = level.clamp(MIN_LEVEL, MAX_LEVEL);
        let mut c = Compressor {
            level,
            cfg: CONFIGS
                .get(usize::from(level) - 1)
                .copied()
                .unwrap_or(CONFIGS[5]),
            buf: vec![0u8; BUF].into_boxed_slice(),
            filled: 0,
            strstart: 0,
            head: vec![NIL; HASH_SIZE].into_boxed_slice(),
            prev: vec![NIL; WINDOW].into_boxed_slice(),
            match_len: MIN_MATCH - 1,
            match_dist: 0,
            match_avail: false,
            tok_dist: vec![0u16; TOK].into_boxed_slice(),
            tok_val: vec![0u16; TOK].into_boxed_slice(),
            tok_n: 0,
            lit_freq: [0; NUM_LIT],
            dist_freq: [0; NUM_DIST],
            block_start: 0,
            blk_raw: 0,
            dirty: false,
            lit_len: [0; 288],
            lit_code: [0; 288],
            dist_len: [0; NUM_DIST],
            dist_code: [0; NUM_DIST],
            acc: 0,
            nbits: 0,
            ob: vec![0u8; OB].into_boxed_slice(),
            ob_len: 0,
        };
        c.reset();
        c
    }

    /// The level this compressor was created with (after clamping).
    pub fn level(&self) -> u8 {
        self.level
    }

    /// Forget all input and start a new stream. Does not allocate.
    pub fn reset(&mut self) {
        self.head.fill(NIL);
        self.prev.fill(NIL);
        self.filled = 0;
        self.strstart = 0;
        self.match_len = MIN_MATCH - 1;
        self.match_dist = 0;
        self.match_avail = false;
        self.tok_n = 0;
        self.lit_freq = [0; NUM_LIT];
        self.dist_freq = [0; NUM_DIST];
        self.block_start = 0;
        self.blk_raw = 0;
        self.dirty = false;
        self.acc = 0;
        self.nbits = 0;
        self.ob_len = 0;
    }

    /// Compress `input`. Output reaches `out` in chunks, possibly none yet: up to the last
    /// ~262 bytes and the current block stay buffered until more input, [`flush`](Self::flush)
    /// or [`finish`](Self::finish).
    pub fn write<E>(
        &mut self,
        mut input: &[u8],
        out: &mut dyn FnMut(&[u8]) -> Result<(), E>,
    ) -> Result<(), E> {
        while !input.is_empty() {
            if self.filled >= BUF {
                self.slide(out)?;
            }
            let room = BUF - self.filled;
            let n = room.min(input.len());
            let (now, rest) = input.split_at(n);
            if let Some(dst) = self.buf.get_mut(self.filled..self.filled + n) {
                dst.copy_from_slice(now);
            }
            self.filled += n;
            self.dirty = true;
            input = rest;
            self.deflate(false, out)?;
        }
        Ok(())
    }

    /// Sync flush: emit everything received so far and end on a byte boundary with an empty
    /// stored block (`00 00 FF FF`). A decoder given the output up to here recovers exactly the
    /// input so far; the stream continues afterwards and later data may refer back across the
    /// flush point. A flush with no new input emits nothing.
    pub fn flush<E>(&mut self, out: &mut dyn FnMut(&[u8]) -> Result<(), E>) -> Result<(), E> {
        if !self.dirty {
            return self.drain(out);
        }
        self.deflate(true, out)?;
        self.flush_block(false, out)?;
        self.room(out)?;
        self.put(0, 3);
        self.align();
        self.put(0x0000, 16);
        self.put(0xFFFF, 16);
        self.dirty = false;
        self.drain(out)
    }

    /// End the stream: emit the final block and drain. The compressor is then reset and ready
    /// for a new stream.
    pub fn finish<E>(&mut self, out: &mut dyn FnMut(&[u8]) -> Result<(), E>) -> Result<(), E> {
        self.deflate(true, out)?;
        self.flush_block(true, out)?;
        self.align();
        self.drain(out)?;
        self.reset();
        Ok(())
    }

    // ---- bit writer -------------------------------------------------------------------------

    /// Append `n` (at most 24) bits of `v`, least significant first. The caller has ensured
    /// room in `ob` (see [`room`](Self::room)).
    // E3 / H2: `ob_len + 4 <= OB` is guaranteed by `room` being called before every bounded
    // group of puts (at most OB_SLACK bytes are written between calls).
    #[allow(clippy::indexing_slicing)]
    #[inline]
    fn put(&mut self, v: u32, n: u32) {
        self.acc |= u64::from(v) << self.nbits;
        self.nbits += n;
        if self.nbits >= 32 {
            let w = (self.acc as u32).to_le_bytes();
            self.ob[self.ob_len..self.ob_len + 4].copy_from_slice(&w);
            self.ob_len += 4;
            self.acc >>= 32;
            self.nbits -= 32;
        }
    }

    /// Pad to a byte boundary with zero bits.
    // E3 / H2: same room guarantee as `put`.
    #[allow(clippy::indexing_slicing)]
    fn align(&mut self) {
        while self.nbits > 0 {
            self.ob[self.ob_len] = self.acc as u8;
            self.ob_len += 1;
            self.acc >>= 8;
            self.nbits = self.nbits.saturating_sub(8);
        }
    }

    /// Hand the staged bytes to `out`.
    fn drain<E>(&mut self, out: &mut dyn FnMut(&[u8]) -> Result<(), E>) -> Result<(), E> {
        if self.ob_len > 0 {
            let n = self.ob_len;
            self.ob_len = 0;
            if let Some(s) = self.ob.get(..n) {
                out(s)?;
            }
        }
        Ok(())
    }

    /// Make sure at least [`OB_SLACK`] bytes are free in the staging buffer.
    #[inline]
    fn room<E>(&mut self, out: &mut dyn FnMut(&[u8]) -> Result<(), E>) -> Result<(), E> {
        if self.ob_len > OB - OB_SLACK {
            self.drain(out)?;
        }
        Ok(())
    }

    // ---- window and match finder ------------------------------------------------------------

    /// Slide the window down by whole windows to make room, keeping at least one window of
    /// history behind `strstart`.
    // E3 / H2: `s <= strstart <= filled <= BUF`; hash and chain entries are bounded by BUF.
    #[allow(clippy::indexing_slicing)]
    fn slide<E>(&mut self, out: &mut dyn FnMut(&[u8]) -> Result<(), E>) -> Result<(), E> {
        let s = (self.strstart.saturating_sub(WINDOW) / WINDOW) * WINDOW;
        if s == 0 {
            return Ok(());
        }
        // The current block's data must stay in the buffer for the stored-block fallback.
        if self.block_start < s {
            self.flush_block(false, out)?;
        }
        self.buf.copy_within(s..self.filled, 0);
        self.filled -= s;
        self.strstart -= s;
        self.block_start = self.block_start.saturating_sub(s);
        let s32 = s as u32;
        for e in self.head.iter_mut().chain(self.prev.iter_mut()) {
            *e = if *e != NIL && *e >= s32 {
                *e - s32
            } else {
                NIL
            };
        }
        Ok(())
    }

    /// Hash of the three bytes at `p` (needs `p + 3 <= filled`), and insert `p` at the head of
    /// its chain. Returns the previous head.
    // E3 / H2: `p + 3 <= filled <= BUF` by every caller; the hash is below HASH_SIZE.
    #[allow(clippy::indexing_slicing)]
    #[inline]
    fn insert(&mut self, p: usize) -> u32 {
        let v = u32::from(self.buf[p])
            | u32::from(self.buf[p + 1]) << 8
            | u32::from(self.buf[p + 2]) << 16;
        let h = (v.wrapping_mul(0x9E37_79B1) >> (32 - HASH_BITS)) as usize;
        let old = self.head[h];
        self.prev[p & WMASK] = old;
        self.head[h] = p as u32;
        old
    }

    /// Longest match for `pos` along the chain starting at `cand`, strictly longer than
    /// `prev_len`, at most `limit` bytes. Returns (length, distance), or (0, 0) if none.
    // E3 / H2: `cand < pos`, `pos + limit <= filled`, and `best < limit`, so every buffer index
    // is below `filled`; chain slots are masked to the window.
    #[allow(clippy::indexing_slicing)]
    fn longest_match(
        &self,
        pos: usize,
        mut cand: usize,
        prev_len: usize,
        limit: usize,
    ) -> (usize, usize) {
        let mut best = prev_len;
        let mut best_dist = 0usize;
        if best >= limit {
            return (0, 0);
        }
        let mut chain = self.cfg.chain;
        if prev_len >= self.cfg.good {
            chain >>= 2;
        }
        let nice = self.cfg.nice.min(limit);
        let buf = &self.buf[..];
        loop {
            let dist = pos - cand;
            if dist > WINDOW {
                break;
            }
            // Quick reject on the byte that would make this candidate beat the best so far.
            if buf[cand + best] == buf[pos + best] && buf[cand] == buf[pos] {
                let len = match_len(buf, cand, pos, limit);
                if len > best {
                    best = len;
                    best_dist = dist;
                    if len >= nice {
                        break;
                    }
                }
            }
            chain -= 1;
            if chain == 0 {
                break;
            }
            let next = self.prev[cand & WMASK];
            // Links only ever point backwards; a slot overwritten by a newer position does not.
            if next == NIL || next as usize >= cand {
                break;
            }
            cand = next as usize;
        }
        if best_dist == 0 {
            (0, 0)
        } else {
            (best, best_dist)
        }
    }

    // ---- LZ77 drivers -----------------------------------------------------------------------

    fn deflate<E>(
        &mut self,
        flush: bool,
        out: &mut dyn FnMut(&[u8]) -> Result<(), E>,
    ) -> Result<(), E> {
        if self.cfg.greedy {
            self.deflate_greedy(flush, out)
        } else {
            self.deflate_lazy(flush, out)
        }
    }

    // E3 / H2: `strstart < filled`, `p + 3 <= filled` checked before each insert.
    #[allow(clippy::indexing_slicing)]
    fn deflate_greedy<E>(
        &mut self,
        flush: bool,
        out: &mut dyn FnMut(&[u8]) -> Result<(), E>,
    ) -> Result<(), E> {
        loop {
            let la = self.filled - self.strstart;
            if la < MIN_LA && (!flush || la == 0) {
                break;
            }
            let pos = self.strstart;
            let head = if la >= MIN_MATCH {
                self.insert(pos)
            } else {
                NIL
            };
            let (mut len, mut dist) = (0, 0);
            if head != NIL {
                (len, dist) =
                    self.longest_match(pos, head as usize, MIN_MATCH - 1, la.min(MAX_MATCH));
                if len == MIN_MATCH && dist > TOO_FAR {
                    len = 0;
                }
            }
            if len >= MIN_MATCH {
                self.add_match(len, dist, out)?;
                if len <= self.cfg.lazy {
                    for p in pos + 1..pos + len {
                        if p + MIN_MATCH <= self.filled {
                            self.insert(p);
                        }
                    }
                }
                self.strstart += len;
            } else {
                let b = self.buf[pos];
                self.add_lit(b, out)?;
                self.strstart += 1;
            }
        }
        Ok(())
    }

    // E3 / H2: `strstart >= 1` whenever `match_avail` is set; `p + 3 <= filled` checked.
    #[allow(clippy::indexing_slicing)]
    fn deflate_lazy<E>(
        &mut self,
        flush: bool,
        out: &mut dyn FnMut(&[u8]) -> Result<(), E>,
    ) -> Result<(), E> {
        loop {
            let la = self.filled - self.strstart;
            if la < MIN_LA && (!flush || la == 0) {
                break;
            }
            let pos = self.strstart;
            let head = if la >= MIN_MATCH {
                self.insert(pos)
            } else {
                NIL
            };
            let prev_len = self.match_len;
            let prev_dist = self.match_dist;
            self.match_len = MIN_MATCH - 1;
            if head != NIL && prev_len < self.cfg.lazy {
                let (len, dist) =
                    self.longest_match(pos, head as usize, prev_len, la.min(MAX_MATCH));
                if len >= MIN_MATCH && !(len == MIN_MATCH && dist > TOO_FAR) {
                    self.match_len = len;
                    self.match_dist = dist;
                }
            }
            if prev_len >= MIN_MATCH && self.match_len <= prev_len {
                // The match found one position ago is at least as good: emit it.
                self.add_match(prev_len, prev_dist, out)?;
                for p in pos + 1..pos + prev_len - 1 {
                    if p + MIN_MATCH <= self.filled {
                        self.insert(p);
                    }
                }
                self.strstart = pos + prev_len - 1;
                self.match_avail = false;
                self.match_len = MIN_MATCH - 1;
            } else if self.match_avail {
                let b = self.buf[pos - 1];
                self.add_lit(b, out)?;
                self.strstart += 1;
            } else {
                self.match_avail = true;
                self.strstart += 1;
            }
        }
        if flush && self.match_avail {
            if let Some(&b) = self.buf.get(self.strstart.wrapping_sub(1)) {
                self.add_lit(b, out)?;
            }
            self.match_avail = false;
            self.match_len = MIN_MATCH - 1;
        }
        Ok(())
    }

    // ---- tokens and blocks ------------------------------------------------------------------

    // E3 / H2: `tok_n < TOK` (a full block is emitted below); `b` indexes the 286 frequencies.
    #[allow(clippy::indexing_slicing)]
    #[inline]
    fn add_lit<E>(&mut self, b: u8, out: &mut dyn FnMut(&[u8]) -> Result<(), E>) -> Result<(), E> {
        self.tok_dist[self.tok_n] = 0;
        self.tok_val[self.tok_n] = u16::from(b);
        self.tok_n += 1;
        self.lit_freq[usize::from(b)] += 1;
        self.blk_raw += 1;
        if self.tok_n == TOK {
            self.flush_block(false, out)?;
        }
        Ok(())
    }

    // E3 / H2: `tok_n < TOK`; `len - 3 < 256`; the symbol indexes are below 286 and 30.
    #[allow(clippy::indexing_slicing)]
    #[inline]
    fn add_match<E>(
        &mut self,
        len: usize,
        dist: usize,
        out: &mut dyn FnMut(&[u8]) -> Result<(), E>,
    ) -> Result<(), E> {
        self.tok_dist[self.tok_n] = dist as u16;
        self.tok_val[self.tok_n] = (len - MIN_MATCH) as u16;
        self.tok_n += 1;
        self.lit_freq[257 + usize::from(LEN_SYM[len - MIN_MATCH])] += 1;
        self.dist_freq[dist_symbol(dist).0] += 1;
        self.blk_raw += len;
        if self.tok_n == TOK {
            self.flush_block(false, out)?;
        }
        Ok(())
    }

    /// Emit the current block in whichever form is smallest, and start a new one. With `last`
    /// the block is final and is emitted even when empty.
    // E3 / H2: tables are indexed by symbols below 288/30/19; the stored-block slice lies inside
    // the buffer because a block is always flushed before a slide could discard its data.
    #[allow(clippy::indexing_slicing)]
    fn flush_block<E>(
        &mut self,
        last: bool,
        out: &mut dyn FnMut(&[u8]) -> Result<(), E>,
    ) -> Result<(), E> {
        if self.tok_n == 0 && !last {
            return Ok(());
        }
        self.room(out)?;
        self.lit_freq[END_OF_BLOCK] += 1;
        build_lengths(&self.lit_freq, 15, &mut self.lit_len[..NUM_LIT]);
        build_lengths(&self.dist_freq, 15, &mut self.dist_len);

        // Run-length encode the code lengths (RFC 1951 3.2.7).
        let nlit = self.lit_len[..NUM_LIT]
            .iter()
            .rposition(|&l| l != 0)
            .map_or(257, |i| (i + 1).max(257));
        let ndist = self
            .dist_len
            .iter()
            .rposition(|&l| l != 0)
            .map_or(1, |i| i + 1);
        let mut seq = [0u8; NUM_LIT + NUM_DIST];
        seq[..nlit].copy_from_slice(&self.lit_len[..nlit]);
        seq[nlit..nlit + ndist].copy_from_slice(&self.dist_len[..ndist]);
        let total = nlit + ndist;
        let mut rle_sym = [0u8; NUM_LIT + NUM_DIST];
        let mut rle_extra = [0u8; NUM_LIT + NUM_DIST];
        let mut nrle = 0usize;
        let mut clen_freq = [0u32; 19];
        let mut i = 0usize;
        while i < total {
            let v = seq[i];
            let mut run = 1;
            while i + run < total && seq[i + run] == v {
                run += 1;
            }
            let (sym, extra, used) = if v == 0 && run >= 11 {
                let r = run.min(138);
                (18u8, (r - 11) as u8, r)
            } else if v == 0 && run >= 3 {
                (17u8, (run - 3) as u8, run)
            } else if v != 0 && i > 0 && seq[i - 1] == v && run >= 3 {
                let r = run.min(6);
                (16u8, (r - 3) as u8, r)
            } else {
                (v, 0u8, 1)
            };
            rle_sym[nrle] = sym;
            rle_extra[nrle] = extra;
            nrle += 1;
            clen_freq[usize::from(sym)] += 1;
            i += used;
        }
        let mut clen_len = [0u8; 19];
        let mut clen_code = [0u16; 19];
        build_lengths(&clen_freq, 7, &mut clen_len);
        assign_codes(&clen_len, &mut clen_code);
        let hclen = CLEN_ORDER
            .iter()
            .rposition(|&s| clen_len[s] != 0)
            .map_or(4, |i| (i + 1).max(4));

        // Cost of each form in bits.
        let mut dyn_bits = 3 + 5 + 5 + 4 + 3 * hclen as u64;
        for &sym in rle_sym.iter().take(nrle) {
            let s = usize::from(sym);
            dyn_bits += u64::from(clen_len[s]) + u64::from(CLEN_EXTRA[s]);
        }
        let mut fixed_bits = 3u64;
        for (s, &freq) in self.lit_freq.iter().enumerate() {
            let f = u64::from(freq);
            let extra = if s >= 257 {
                u64::from(LEN_EXTRA[s - 257])
            } else {
                0
            };
            dyn_bits += f * (u64::from(self.lit_len[s]) + extra);
            let fl = match s {
                0..=143 => 8,
                144..=255 => 9,
                256..=279 => 7,
                _ => 8,
            };
            fixed_bits += f * (fl + extra);
        }
        for (s, &freq) in self.dist_freq.iter().enumerate() {
            let f = u64::from(freq);
            let extra = (s as u64 / 2).saturating_sub(1);
            dyn_bits += f * (u64::from(self.dist_len[s]) + extra);
            fixed_bits += f * (5 + extra);
        }
        let raw = self.blk_raw;
        let chunks = raw.div_ceil(65_535).max(1) as u64;
        let pad0 = (8 - (u64::from(self.nbits) + 3) % 8) % 8;
        let stored_bits = 3 + pad0 + 32 * chunks + 8 * (chunks - 1) + 8 * raw as u64;

        if raw > 0 && stored_bits <= dyn_bits.min(fixed_bits) {
            self.emit_stored(last, out)?;
        } else if fixed_bits <= dyn_bits {
            self.load_fixed_tables();
            self.put(u32::from(last) | 1 << 1, 3);
            self.emit_tokens(out)?;
        } else {
            self.put(u32::from(last) | 2 << 1, 3);
            self.put((nlit - 257) as u32, 5);
            self.put((ndist - 1) as u32, 5);
            self.put((hclen - 4) as u32, 4);
            for &s in &CLEN_ORDER[..hclen] {
                self.put(u32::from(clen_len[s]), 3);
            }
            for (&sym, &extra) in rle_sym.iter().zip(&rle_extra).take(nrle) {
                let s = usize::from(sym);
                self.put(u32::from(clen_code[s]), u32::from(clen_len[s]));
                let eb = u32::from(CLEN_EXTRA[s]);
                if eb > 0 {
                    self.put(u32::from(extra), eb);
                }
            }
            assign_codes(&self.lit_len[..NUM_LIT], &mut self.lit_code[..NUM_LIT]);
            assign_codes(&self.dist_len, &mut self.dist_code);
            self.emit_tokens(out)?;
        }

        self.block_start += self.blk_raw;
        self.blk_raw = 0;
        self.tok_n = 0;
        self.lit_freq = [0; NUM_LIT];
        self.dist_freq = [0; NUM_DIST];
        Ok(())
    }

    // E3 / H2: constant table sizes; indexes are symbol numbers below the table lengths.
    #[allow(clippy::indexing_slicing)]
    fn load_fixed_tables(&mut self) {
        for (s, l) in self.lit_len.iter_mut().enumerate() {
            *l = match s {
                0..=143 => 8,
                144..=255 => 9,
                256..=279 => 7,
                _ => 8,
            };
        }
        self.dist_len = [5; NUM_DIST];
        assign_codes(&self.lit_len, &mut self.lit_code);
        assign_codes(&self.dist_len, &mut self.dist_code);
    }

    /// Write the block's tokens and end-of-block with the tables loaded in `lit_*`/`dist_*`.
    // E3 / H2: `i < tok_n <= TOK`; length and distance symbols index tables of 288 and 30.
    #[allow(clippy::indexing_slicing)]
    fn emit_tokens<E>(&mut self, out: &mut dyn FnMut(&[u8]) -> Result<(), E>) -> Result<(), E> {
        for i in 0..self.tok_n {
            if self.ob_len > OB - OB_SLACK {
                self.drain(out)?;
            }
            let d = usize::from(self.tok_dist[i]);
            let v = usize::from(self.tok_val[i]);
            if d == 0 {
                self.put(u32::from(self.lit_code[v]), u32::from(self.lit_len[v]));
            } else {
                let ls = usize::from(LEN_SYM[v]);
                let sym = 257 + ls;
                self.put(u32::from(self.lit_code[sym]), u32::from(self.lit_len[sym]));
                let eb = u32::from(LEN_EXTRA[ls]);
                if eb > 0 {
                    self.put((v + MIN_MATCH) as u32 - u32::from(LEN_BASE[ls]), eb);
                }
                let (ds, deb, dev) = dist_symbol(d);
                self.put(u32::from(self.dist_code[ds]), u32::from(self.dist_len[ds]));
                if deb > 0 {
                    self.put(dev, deb);
                }
            }
        }
        self.put(
            u32::from(self.lit_code[END_OF_BLOCK]),
            u32::from(self.lit_len[END_OF_BLOCK]),
        );
        Ok(())
    }

    /// Write the block's raw bytes as stored blocks of at most 65535 bytes.
    // E3 / H2: the block's bytes lie in `buf[block_start..block_start + blk_raw]` (see
    // `flush_block`).
    #[allow(clippy::indexing_slicing)]
    fn emit_stored<E>(
        &mut self,
        last: bool,
        out: &mut dyn FnMut(&[u8]) -> Result<(), E>,
    ) -> Result<(), E> {
        let mut at = self.block_start;
        let end = at + self.blk_raw;
        while at < end {
            let n = (end - at).min(65_535);
            let fin = last && at + n == end;
            self.put(u32::from(fin), 3);
            self.align();
            let len = n as u32;
            self.put(len, 16);
            self.put(!len & 0xFFFF, 16);
            self.drain(out)?;
            out(&self.buf[at..at + n])?;
            at += n;
        }
        Ok(())
    }
}

/// Distance code for a distance of 1..=32768: (symbol, extra bit count, extra bits value).
#[inline]
fn dist_symbol(dist: usize) -> (usize, u32, u32) {
    let d = (dist - 1) as u32;
    if d < 4 {
        return (d as usize, 0, 0);
    }
    let nb = 31 - d.leading_zeros();
    let eb = nb - 1;
    let sym = 2 * nb + ((d >> eb) & 1);
    (sym as usize, eb, d & ((1 << eb) - 1))
}

/// Length of the common prefix of `buf[a..]` and `buf[b..]`, at most `limit`.
// E3 / H2: callers guarantee `a < b` and `b + limit <= buf.len()`.
#[allow(clippy::indexing_slicing)]
#[inline]
fn match_len(buf: &[u8], a: usize, b: usize, limit: usize) -> usize {
    let mut n = 0usize;
    while n + 8 <= limit {
        let x = u64::from_le_bytes([
            buf[a + n],
            buf[a + n + 1],
            buf[a + n + 2],
            buf[a + n + 3],
            buf[a + n + 4],
            buf[a + n + 5],
            buf[a + n + 6],
            buf[a + n + 7],
        ]);
        let y = u64::from_le_bytes([
            buf[b + n],
            buf[b + n + 1],
            buf[b + n + 2],
            buf[b + n + 3],
            buf[b + n + 4],
            buf[b + n + 5],
            buf[b + n + 6],
            buf[b + n + 7],
        ]);
        let d = x ^ y;
        if d != 0 {
            return n + (d.trailing_zeros() / 8) as usize;
        }
        n += 8;
    }
    while n < limit && buf[a + n] == buf[b + n] {
        n += 1;
    }
    n
}
