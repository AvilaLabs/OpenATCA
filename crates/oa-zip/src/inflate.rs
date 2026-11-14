//! DEFLATE decompression per RFC 1951: stored, fixed Huffman and dynamic Huffman blocks. Core
//! (pure): bytes in, bytes out through a caller-supplied closure. Back-references reach at most
//! [`WINDOW_LEN`] bytes back and never before the start of the output.
//!
//! [`Inflater`] is the one implementation: a resumable state machine with fixed preallocated
//! state (a 32 KiB window, three Huffman tables, a 64-bit bit accumulator) that allocates only in
//! [`Inflater::new`]. Input arrives in pieces of any size ([`Inflater::push`]); output leaves
//! through a closure in runs of at most one window. The one-shot [`inflate`] is a thin wrapper.
//!
//! Every step of the machine (a block header, one literal or one length/distance pair, one
//! code-length symbol) is atomic: it needs at most 48 bits, the accumulator always holds at least
//! 57 bits when input is available, so a step that lacks bits has seen all the input there is,
//! leaves its state untouched and waits for the next push. That is what makes the output
//! independent of how the input is chunked.

use std::convert::Infallible;
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
/// Code lengths of one dynamic block header: literal/length plus distance.
const MAX_LENGTHS: usize = MAX_USED_LIT + MAX_USED_DIST;

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
    /// The output grew past `max_ratio` times the input consumed (a decompression bomb).
    RatioLimit { max_ratio: u64 },
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
            InflateError::RatioLimit { max_ratio } => {
                write!(
                    f,
                    "inflate: output exceeds {max_ratio} times the input (decompression bomb)"
                )
            }
        }
    }
}

impl std::error::Error for InflateError {}

/// What [`Inflater::push`] can fail with: the stream is bad, or the caller's output closure
/// refused a run of bytes (its error is returned unchanged).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamError<E> {
    /// The DEFLATE stream is invalid, truncated by a limit, or over a limit.
    Inflate(InflateError),
    /// The output closure returned this error.
    Sink(E),
}

impl<E: fmt::Display> fmt::Display for StreamError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StreamError::Inflate(e) => e.fmt(f),
            StreamError::Sink(e) => write!(f, "inflate: output failed: {e}"),
        }
    }
}

impl<E: fmt::Debug + fmt::Display> std::error::Error for StreamError<E> {}

impl<E> From<InflateError> for StreamError<E> {
    fn from(e: InflateError) -> Self {
        StreamError::Inflate(e)
    }
}

/// The hard limits of one DEFLATE stream (OA-ZIP-003): the total output, and the compression
/// ratio above a small exempt size. Plain record (A1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct InflateLimits {
    /// Most bytes the stream may produce.
    pub max_output: u64,
    /// Most output bytes per input byte consumed; `u64::MAX` for no ratio limit.
    pub max_ratio: u64,
    /// Output at or below this size is exempt from `max_ratio`.
    pub ratio_exempt_bytes: u64,
}

impl InflateLimits {
    /// An output cap and nothing else.
    pub const fn output_only(max_output: u64) -> InflateLimits {
        InflateLimits {
            max_output,
            max_ratio: u64::MAX,
            ratio_exempt_bytes: 0,
        }
    }
}

/// What one [`Inflater::push`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct Consumed {
    /// Bytes of the input slice that belong to the stream (all of it unless `finished`).
    pub input: usize,
    /// The final block ended; any further input bytes belong to whatever follows the stream.
    pub finished: bool,
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
    const fn empty() -> Huffman {
        Huffman {
            count: [0; MAX_CODE_LEN + 1],
            symbol: [0; MAX_LIT_SYMBOLS],
            fast: [0; FAST_SIZE],
        }
    }

    // E3 / H2: `count`, `offs` and `next_code` have MAX_CODE_LEN + 2 entries and are indexed by a
    // code length that is 1..=MAX_CODE_LEN (lengths above it were rejected in the first loop) or
    // by 0; `fast` is indexed below FAST_SIZE by the loop condition.
    #[allow(clippy::indexing_slicing)]
    fn build(lengths: &[u8]) -> Result<Huffman, InflateError> {
        let mut h = Huffman::empty();
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

    /// Looks up the next code in `acc` (of which `cnt` low bits are real) without consuming it.
    /// `Ok(None)` means the real bits so far are a proper prefix of a code: wait for more input.
    // E3 / H2: the `fast` index is masked to FAST_SIZE - 1 and `count` is indexed by a length in
    // 1..=MAX_CODE_LEN, both inside their arrays.
    #[allow(clippy::indexing_slicing)]
    fn decode(&self, acc: u64, cnt: u32) -> Result<Option<(u16, u32)>, InflateError> {
        let e = self.fast[(acc as usize) & (FAST_SIZE - 1)];
        if e != 0 {
            let len = u32::from(e & 15);
            // The entry was found with unknown upper bits read as zero; it is real only if the
            // whole code lies within the bits that exist.
            return Ok((len <= cnt).then_some((e >> 4, len)));
        }
        // Slow path: canonical decode one bit at a time (puff-style).
        let mut code: i32 = 0;
        let mut first: i32 = 0;
        let mut index: i32 = 0;
        for len in 1..=MAX_CODE_LEN {
            if len as u32 > cnt {
                return Ok(None);
            }
            code |= ((acc >> (len - 1)) & 1) as i32;
            let count = i32::from(self.count[len]);
            if code - count < first {
                let at = usize::try_from(index + (code - first))
                    .map_err(|_| InflateError::InvalidCode)?;
                return self
                    .symbol
                    .get(at)
                    .map(|&s| Some((s, len as u32)))
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

/// The fixed literal/length code lengths of RFC 1951 section 3.2.6.
fn fixed_lit_lengths() -> [u8; MAX_LIT_SYMBOLS] {
    let mut l = [0u8; MAX_LIT_SYMBOLS];
    for (i, v) in l.iter_mut().enumerate() {
        *v = match i {
            0..=143 => 8,
            144..=255 => 9,
            256..=279 => 7,
            _ => 8,
        };
    }
    l
}

/// Where the machine is. Each variant is a point between atomic steps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Waiting for a 3-bit block header.
    BlockHeader,
    /// Aligned; waiting for LEN and NLEN (32 bits).
    StoredHeader,
    /// Copying this many more stored bytes.
    StoredCopy(u16),
    /// Waiting for HLIT, HDIST and HCLEN (14 bits).
    DynCounts,
    /// Reading the code-length code lengths: `i` of `hclen` done.
    DynClLens { hclen: u8, i: u8 },
    /// Reading the literal/length and distance code lengths: `i` of `hlit + hdist` done.
    DynLens { i: u16 },
    /// Decoding literal/length symbols with the current tables.
    Codes,
    /// The final block has ended.
    Done,
}

/// Streaming DEFLATE decoder. A subsystem under D5: it owns the window, the tables and the bit
/// accumulator. After an error, [`reset`](Self::reset) it or drop it; its state is still safe to
/// push into but the stream is lost.
// A7.2: cold struct (one per stream), size guard waived
pub struct Inflater {
    /// Circular history, `WINDOW_LEN` bytes; decoded bytes are written here and emitted from it.
    win: Box<[u8]>,
    /// Next write position in `win`.
    wpos: usize,
    /// Start of the bytes in `win` not yet handed to the output closure.
    emitted: usize,
    acc: u64,
    cnt: u32,
    state: State,
    last: bool,
    hlit: u16,
    hdist: u16,
    cl_lens: [u8; 19],
    lengths: [u8; MAX_LENGTHS],
    lit: Box<Huffman>,
    dist: Box<Huffman>,
    cl: Box<Huffman>,
    limits: InflateLimits,
    total_in: u64,
    total_out: u64,
    sync_out: u64,
}

impl Inflater {
    /// Allocates all state (about 45 KiB); nothing allocates afterwards.
    pub fn new(limits: InflateLimits) -> Inflater {
        Inflater {
            win: vec![0u8; WINDOW_LEN].into_boxed_slice(),
            wpos: 0,
            emitted: 0,
            acc: 0,
            cnt: 0,
            state: State::BlockHeader,
            last: false,
            hlit: 0,
            hdist: 0,
            cl_lens: [0; 19],
            lengths: [0; MAX_LENGTHS],
            lit: Box::new(Huffman::empty()),
            dist: Box::new(Huffman::empty()),
            cl: Box::new(Huffman::empty()),
            limits,
            total_in: 0,
            total_out: 0,
            sync_out: 0,
        }
    }

    /// Starts a new stream (a new gzip member) with new limits, keeping the allocations.
    pub fn reset(&mut self, limits: InflateLimits) {
        self.wpos = 0;
        self.emitted = 0;
        self.acc = 0;
        self.cnt = 0;
        self.state = State::BlockHeader;
        self.last = false;
        self.limits = limits;
        self.total_in = 0;
        self.total_out = 0;
        self.sync_out = 0;
    }

    /// Input bytes taken into the stream so far.
    pub fn total_in(&self) -> u64 {
        self.total_in
    }

    /// Output bytes produced so far (all of them delivered once `push` returns).
    pub fn total_out(&self) -> u64 {
        self.total_out
    }

    /// Output bytes produced up to the end of the most recent sync-flush marker (an empty stored
    /// block); 0 if there has been none. Everything up to here is whole, flushed data.
    pub fn sync_out(&self) -> u64 {
        self.sync_out
    }

    /// True once the final block has ended.
    pub fn finished(&self) -> bool {
        self.state == State::Done
    }

    /// Feeds `input`, handing decoded bytes to `out` in runs of at most [`WINDOW_LEN`]. Takes
    /// the whole slice unless the stream ends inside it; the result says how much it took and
    /// whether the stream is finished. Everything decoded before an error is delivered first.
    pub fn push<E>(
        &mut self,
        input: &[u8],
        out: &mut dyn FnMut(&[u8]) -> Result<(), E>,
    ) -> Result<Consumed, StreamError<E>> {
        let mut pos = 0usize;
        match self.run(input, &mut pos, out) {
            Ok(()) => {
                self.flush(out).map_err(StreamError::Sink)?;
                Ok(Consumed {
                    input: pos,
                    finished: self.state == State::Done,
                })
            }
            Err(StreamError::Inflate(e)) => {
                // Deliver the good prefix, then report why the stream stopped.
                let _ = self.flush(out);
                Err(StreamError::Inflate(e))
            }
            Err(e) => Err(e),
        }
    }

    fn refill(&mut self, input: &[u8], pos: &mut usize) {
        while self.cnt <= 56 {
            let Some(&b) = input.get(*pos) else { break };
            self.acc |= u64::from(b) << self.cnt;
            self.cnt += 8;
            *pos += 1;
            self.total_in += 1;
        }
    }

    /// Takes `n` (<= `cnt`) bits.
    fn take(&mut self, n: u32) -> u32 {
        let v = (self.acc & ((1u64 << n) - 1)) as u32;
        self.acc >>= n;
        self.cnt -= n;
        v
    }

    fn end_block(&mut self) {
        self.state = if self.last {
            State::Done
        } else {
            State::BlockHeader
        };
    }

    /// Fails if `n` more output bytes would break a limit (the ratio is against the input taken
    /// so far, which is at most one accumulator ahead of what the stream really used).
    fn check_room(&self, n: u64) -> Result<(), InflateError> {
        let total = self.total_out.saturating_add(n);
        if total > self.limits.max_output {
            return Err(InflateError::OutputLimit {
                max_output: usize::try_from(self.limits.max_output).unwrap_or(usize::MAX),
            });
        }
        if total > self.limits.ratio_exempt_bytes
            && total > self.total_in.saturating_mul(self.limits.max_ratio)
        {
            return Err(InflateError::RatioLimit {
                max_ratio: self.limits.max_ratio,
            });
        }
        Ok(())
    }

    /// Hands the bytes written since the last flush to `out`.
    fn flush<E>(&mut self, out: &mut dyn FnMut(&[u8]) -> Result<(), E>) -> Result<(), E> {
        let (from, to) = (self.emitted, self.wpos);
        self.emitted = to;
        match self.win.get(from..to) {
            Some(run) if !run.is_empty() => out(run),
            _ => Ok(()),
        }
    }

    fn put<E>(
        &mut self,
        b: u8,
        out: &mut dyn FnMut(&[u8]) -> Result<(), E>,
    ) -> Result<(), StreamError<E>> {
        if let Some(slot) = self.win.get_mut(self.wpos) {
            *slot = b;
        }
        self.wpos += 1;
        self.total_out += 1;
        if self.wpos == WINDOW_LEN {
            self.flush(out).map_err(StreamError::Sink)?;
            self.wpos = 0;
            self.emitted = 0;
        }
        Ok(())
    }

    /// Output bytes available as history for a back-reference.
    fn history(&self) -> u64 {
        self.total_out.min(WINDOW_LEN as u64)
    }

    fn run<E>(
        &mut self,
        input: &[u8],
        pos: &mut usize,
        out: &mut dyn FnMut(&[u8]) -> Result<(), E>,
    ) -> Result<(), StreamError<E>> {
        loop {
            match self.state {
                State::Done => {
                    // Bytes pulled into the accumulator past the end belong to the caller. They
                    // came from this slice: the step that finished needed more than the old bits.
                    let back = (self.cnt / 8) as usize;
                    *pos = pos.saturating_sub(back);
                    self.total_in = self.total_in.saturating_sub(back as u64);
                    self.acc = 0;
                    self.cnt = 0;
                    return Ok(());
                }
                State::BlockHeader => {
                    self.refill(input, pos);
                    if self.cnt < 3 {
                        return Ok(());
                    }
                    let h = self.take(3);
                    self.last = h & 1 == 1;
                    match h >> 1 {
                        0 => {
                            let drop = self.cnt % 8;
                            self.take(drop);
                            self.state = State::StoredHeader;
                        }
                        1 => {
                            let dl = [5u8; 32];
                            *self.lit = Huffman::build(&fixed_lit_lengths())?;
                            *self.dist = Huffman::build(&dl)?;
                            self.state = State::Codes;
                        }
                        2 => self.state = State::DynCounts,
                        _ => return Err(InflateError::ReservedBlockType.into()),
                    }
                }
                State::StoredHeader => {
                    self.refill(input, pos);
                    if self.cnt < 32 {
                        return Ok(());
                    }
                    let len = self.take(16) as u16;
                    let nlen = self.take(16) as u16;
                    if len != !nlen {
                        return Err(InflateError::StoredLengthMismatch.into());
                    }
                    if len == 0 {
                        // The marker a sync flush leaves: everything before it is whole.
                        self.sync_out = self.total_out;
                        self.end_block();
                    } else {
                        self.state = State::StoredCopy(len);
                    }
                }
                State::StoredCopy(mut rem) => {
                    while rem > 0 {
                        if self.cnt >= 8 {
                            self.check_room(1)?;
                            let b = self.take(8) as u8;
                            self.put(b, out)?;
                            rem -= 1;
                        } else {
                            let avail = input.len().saturating_sub(*pos);
                            let n = usize::from(rem).min(avail);
                            if n == 0 {
                                break;
                            }
                            self.total_in += n as u64;
                            self.check_room(n as u64)?;
                            let body = input.get(*pos..*pos + n).unwrap_or(&[]);
                            for &b in body {
                                self.put(b, out)?;
                            }
                            *pos += n;
                            rem -= n as u16; // n <= rem
                        }
                    }
                    if rem == 0 {
                        self.end_block();
                    } else {
                        self.state = State::StoredCopy(rem);
                        return Ok(());
                    }
                }
                State::DynCounts => {
                    self.refill(input, pos);
                    if self.cnt < 14 {
                        return Ok(());
                    }
                    let hlit = self.take(5) as u16 + 257;
                    let hdist = self.take(5) as u16 + 1;
                    let hclen = self.take(4) as u8 + 4;
                    if usize::from(hlit) > MAX_USED_LIT || usize::from(hdist) > MAX_USED_DIST {
                        return Err(InflateError::BadHuffmanTable(
                            "too many length/distance codes",
                        )
                        .into());
                    }
                    self.hlit = hlit;
                    self.hdist = hdist;
                    self.cl_lens = [0; 19];
                    self.state = State::DynClLens { hclen, i: 0 };
                }
                State::DynClLens { hclen, mut i } => {
                    while i < hclen {
                        self.refill(input, pos);
                        if self.cnt < 3 {
                            self.state = State::DynClLens { hclen, i };
                            return Ok(());
                        }
                        let v = self.take(3) as u8;
                        if let Some(slot) = CL_ORDER
                            .get(usize::from(i))
                            .and_then(|&s| self.cl_lens.get_mut(s))
                        {
                            *slot = v;
                        }
                        i += 1;
                    }
                    *self.cl = Huffman::build(&self.cl_lens)?;
                    self.lengths = [0; MAX_LENGTHS];
                    self.state = State::DynLens { i: 0 };
                }
                State::DynLens { mut i } => {
                    let total = usize::from(self.hlit) + usize::from(self.hdist);
                    while usize::from(i) < total {
                        self.refill(input, pos);
                        let Some((sym, l)) = self.cl.decode(self.acc, self.cnt)? else {
                            self.state = State::DynLens { i };
                            return Ok(());
                        };
                        let extra = match sym {
                            16 => 2,
                            17 => 3,
                            18 => 7,
                            _ => 0,
                        };
                        if self.cnt < l + extra {
                            self.state = State::DynLens { i };
                            return Ok(());
                        }
                        self.take(l);
                        let at = usize::from(i);
                        let (val, rep) = match sym {
                            0..=15 => (sym as u8, 1usize),
                            16 => {
                                let prev = at
                                    .checked_sub(1)
                                    .and_then(|p| self.lengths.get(p))
                                    .copied()
                                    .ok_or(InflateError::BadHuffmanTable(
                                        "repeat with no previous length",
                                    ))?;
                                (prev, 3 + self.take(extra) as usize)
                            }
                            17 => (0, 3 + self.take(extra) as usize),
                            _ => (0, 11 + self.take(extra) as usize),
                        };
                        if at + rep > total {
                            return Err(InflateError::BadHuffmanTable(
                                "length repeat overruns table",
                            )
                            .into());
                        }
                        for slot in self.lengths.iter_mut().skip(at).take(rep) {
                            *slot = val;
                        }
                        i += rep as u16; // at + rep <= total <= 316
                    }
                    let (lit, dist) = self.lengths.split_at(usize::from(self.hlit));
                    if lit.get(usize::from(END_OF_BLOCK)).copied().unwrap_or(0) == 0 {
                        return Err(InflateError::BadHuffmanTable("no end-of-block code").into());
                    }
                    let dist = dist
                        .get(..usize::from(self.hdist))
                        .ok_or(InflateError::BadHuffmanTable("distance table"))?;
                    *self.lit = Huffman::build(lit)?;
                    *self.dist = Huffman::build(dist)?;
                    self.state = State::Codes;
                }
                State::Codes => {
                    if !self.codes(input, pos, out)? {
                        return Ok(());
                    }
                }
            }
        }
    }

    /// Decodes symbols until the block ends (`true`) or the input runs out (`false`).
    fn codes<E>(
        &mut self,
        input: &[u8],
        pos: &mut usize,
        out: &mut dyn FnMut(&[u8]) -> Result<(), E>,
    ) -> Result<bool, StreamError<E>> {
        loop {
            self.refill(input, pos);
            // A local copy of the accumulator: a step that runs short of bits is abandoned
            // without having changed anything.
            let (mut a, mut c) = (self.acc, self.cnt);
            let Some((sym, l)) = self.lit.decode(a, c)? else {
                return Ok(false);
            };
            a >>= l;
            c -= l;
            if sym < END_OF_BLOCK {
                self.check_room(1)?;
                self.acc = a;
                self.cnt = c;
                self.put(sym as u8, out)?; // sym < 256
                continue;
            }
            if sym == END_OF_BLOCK {
                self.acc = a;
                self.cnt = c;
                self.end_block();
                return Ok(true);
            }
            let li = usize::from(sym - 257);
            let base = *LEN_BASE.get(li).ok_or(InflateError::InvalidSymbol)?;
            let extra = u32::from(*LEN_EXTRA.get(li).ok_or(InflateError::InvalidSymbol)?);
            if c < extra {
                return Ok(false);
            }
            let len = usize::from(base) + (a & ((1u64 << extra) - 1)) as usize;
            a >>= extra;
            c -= extra;
            let Some((ds, dl)) = self.dist.decode(a, c)? else {
                return Ok(false);
            };
            a >>= dl;
            c -= dl;
            let dbase = *DIST_BASE
                .get(usize::from(ds))
                .ok_or(InflateError::InvalidSymbol)?;
            let dextra = u32::from(
                *DIST_EXTRA
                    .get(usize::from(ds))
                    .ok_or(InflateError::InvalidSymbol)?,
            );
            if c < dextra {
                return Ok(false);
            }
            let d = usize::from(dbase) + (a & ((1u64 << dextra) - 1)) as usize;
            a >>= dextra;
            c -= dextra;
            if d as u64 > self.history() {
                return Err(InflateError::DistanceTooFar.into());
            }
            self.check_room(len as u64)?;
            self.acc = a;
            self.cnt = c;
            // The copy may overlap itself (distance below length): byte by byte repeats the
            // pattern, as the format requires.
            let mut from = (self.wpos + WINDOW_LEN - d) % WINDOW_LEN;
            for _ in 0..len {
                let b = self.win.get(from).copied().unwrap_or(0);
                self.put(b, out)?;
                from = (from + 1) % WINDOW_LEN;
            }
        }
    }
}

/// Inflate a raw DEFLATE stream. `size_hint` pre-sizes the output (capped at `max_output`);
/// output beyond `max_output` bytes is an error, so a hostile stream cannot grow memory without
/// bound. Bytes after the final block are ignored. A convenience over [`Inflater`]; use that to
/// stream, to read past the end of the stream or to apply a ratio limit.
pub fn inflate(input: &[u8], size_hint: usize, max_output: usize) -> Result<Vec<u8>, InflateError> {
    let mut out: Vec<u8> = Vec::with_capacity(size_hint.min(max_output));
    let mut inf = Inflater::new(InflateLimits::output_only(max_output as u64));
    let mut sink = |b: &[u8]| -> Result<(), Infallible> {
        out.extend_from_slice(b);
        Ok(())
    };
    let done = match inf.push(input, &mut sink) {
        Ok(c) => c.finished,
        Err(StreamError::Inflate(e)) => return Err(e),
        Err(StreamError::Sink(never)) => match never {},
    };
    if done {
        Ok(out)
    } else {
        Err(InflateError::Truncated)
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

    // REQ: OA-ZIP-007
    #[test]
    fn state_is_fixed_after_creation() {
        // A stored block, a fixed block and a dynamic block of real data, all in one stream.
        let stored = [0x00, 0x03, 0x00, 0xfc, 0xff, b'a', b'b', b'c'];
        let fixed = [0xcb, 0x48, 0xcd, 0xc9, 0xc9, 0x07, 0x00];
        let mut inf = Inflater::new(InflateLimits::output_only(1 << 20));
        let addr = |i: &Inflater| {
            (
                i.win.as_ptr(),
                i.win.len(),
                &*i.lit as *const Huffman,
                &*i.dist as *const Huffman,
                &*i.cl as *const Huffman,
            )
        };
        let before = addr(&inf);
        assert_eq!(before.1, WINDOW_LEN);
        let mut n = 0usize;
        let mut sink = |b: &[u8]| -> Result<(), Infallible> {
            n += b.len();
            Ok(())
        };
        for chunk in [&stored[..], &fixed[..]] {
            for piece in chunk.chunks(1) {
                inf.push(piece, &mut sink).unwrap();
            }
        }
        inf.reset(InflateLimits::output_only(1 << 20));
        inf.push(&fixed, &mut sink).unwrap();
        assert_eq!(addr(&inf), before);
        assert_eq!(n, 3 + 5 + 5);
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
