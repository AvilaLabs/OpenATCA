//! Tier-1 EBCOT block decoder (T.800 Annex D), default code-block style. Core (pure).
//!
//! A code block is decoded one bit-plane at a time, most significant first, in up to three
//! passes per plane (significance propagation, magnitude refinement, cleanup; the first plane
//! has only the cleanup), each over stripes of four rows, column by column. All passes read one
//! MQ codeword segment. The context of a sample comes from a flag word per sample that holds
//! the significance of its eight neighbours and the sign of its four side neighbours, updated
//! when a neighbour becomes significant, so no pass looks at the neighbours again.

use crate::error::J2kError;
use crate::mq::{self, Cx, Mq};

/// Sub-band orientation codes.
pub const LL: u8 = 0;
pub const HL: u8 = 1;
pub const LH: u8 = 2;
pub const HH: u8 = 3;

// Flag word layout (see the module comment). Bits 0-7: neighbour significant, 8-11: side
// neighbour negative, then the sample's own state.
const N_SIG: u32 = 1;
const S_SIG: u32 = 1 << 1;
const W_SIG: u32 = 1 << 2;
const E_SIG: u32 = 1 << 3;
const NW_SIG: u32 = 1 << 4;
const NE_SIG: u32 = 1 << 5;
const SW_SIG: u32 = 1 << 6;
const SE_SIG: u32 = 1 << 7;
const N_NEG: u32 = 1 << 8;
const S_NEG: u32 = 1 << 9;
const W_NEG: u32 = 1 << 10;
const E_NEG: u32 = 1 << 11;
const SIG: u32 = 1 << 12;
const VISITED: u32 = 1 << 13;
const REFINED: u32 = 1 << 14;
const NEG: u32 = 1 << 15;
const NEIGHBOURS: u32 = 0xFF;

// Context indexes: zero coding 0-8, sign 9-13, magnitude refinement 14-16, run-length, uniform.
const CX_SC: usize = 9;
const CX_MR: usize = 14;
const CX_RL: usize = 17;
const CX_UNI: usize = 18;
const NCX: usize = 19;

/// Zero-coding context label of Table D.1 for a neighbour pattern.
const fn zc_label(orient: u8, f: u32) -> u8 {
    let v = (f & 1) + ((f >> 1) & 1);
    let h = ((f >> 2) & 1) + ((f >> 3) & 1);
    let d = ((f >> 4) & 1) + ((f >> 5) & 1) + ((f >> 6) & 1) + ((f >> 7) & 1);
    if orient == HH {
        let hv = h + v;
        return if d >= 3 {
            8
        } else if d == 2 {
            if hv >= 1 {
                7
            } else {
                6
            }
        } else if d == 1 {
            if hv >= 2 {
                5
            } else if hv == 1 {
                4
            } else {
                3
            }
        } else if hv >= 2 {
            2
        } else if hv == 1 {
            1
        } else {
            0
        };
    }
    // LL and LH read H then V; HL swaps them.
    let (h, v) = if orient == HL { (v, h) } else { (h, v) };
    if h == 2 {
        8
    } else if h == 1 {
        if v >= 1 {
            7
        } else if d >= 1 {
            6
        } else {
            5
        }
    } else if v == 2 {
        4
    } else if v == 1 {
        3
    } else if d >= 2 {
        2
    } else {
        d as u8
    }
}

#[allow(clippy::indexing_slicing)] // const table: the index is bounded by the loop
const fn zc_table(orient: u8) -> [u8; 256] {
    let mut t = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        t[i] = zc_label(orient, i as u32);
        i += 1;
    }
    t
}

static ZC_LL: [u8; 256] = zc_table(LL);
static ZC_HL: [u8; 256] = zc_table(HL);
static ZC_LH: [u8; 256] = zc_table(LH);
static ZC_HH: [u8; 256] = zc_table(HH);

/// Contribution of two opposite neighbours (Table D.2), each given as (significant, negative).
const fn contribution(sa: bool, na: bool, sb: bool, nb: bool) -> i32 {
    let a = if sa {
        if na {
            -1
        } else {
            1
        }
    } else {
        0
    };
    let b = if sb {
        if nb {
            -1
        } else {
            1
        }
    } else {
        0
    };
    let s = a + b;
    if s > 1 {
        1
    } else if s < -1 {
        -1
    } else {
        s
    }
}

/// Sign context (Table D.3) for index `(f & 0xF) | ((f >> 8) & 0xF) << 4`: label in the low
/// nibble, XOR bit in bit 4.
#[allow(clippy::indexing_slicing)] // const table: the index is bounded by the loop
const fn sc_table() -> [u8; 256] {
    let mut t = [0u8; 256];
    let mut i = 0usize;
    while i < 256 {
        let sig = i & 0xF;
        let neg = i >> 4;
        let v = contribution(sig & 1 != 0, neg & 1 != 0, sig & 2 != 0, neg & 2 != 0);
        let h = contribution(sig & 4 != 0, neg & 4 != 0, sig & 8 != 0, neg & 8 != 0);
        let (label, x) = if h == 1 {
            if v == 1 {
                (13, 0)
            } else if v == 0 {
                (12, 0)
            } else {
                (11, 0)
            }
        } else if h == 0 {
            if v == 1 {
                (10, 0)
            } else if v == 0 {
                (9, 0)
            } else {
                (10, 1)
            }
        } else if v == 1 {
            (11, 1)
        } else if v == 0 {
            (12, 1)
        } else {
            (13, 1)
        };
        t[i] = label | (x << 4);
        i += 1;
    }
    t
}

static SC: [u8; 256] = sc_table();

/// Buffers reused from block to block, so decoding allocates only when a block is larger than
/// any before it.
// A7.2: cold struct, size guard waived (one per decode)
#[derive(Default)]
pub struct Scratch {
    flags: Vec<u32>,
    mag: Vec<u32>,
}

// A7.2: cold struct, size guard waived (one per block being decoded)
struct Blk<'a> {
    w: usize,
    h: usize,
    stride: usize,
    flags: &'a mut [u32],
    mag: &'a mut [u32],
    zc: &'static [u8; 256],
    mq: Mq<'a>,
    cx: [Cx; NCX],
}

fn flag(b: &Blk, i: usize) -> u32 {
    b.flags.get(i).copied().unwrap_or(0)
}

fn flag_or(b: &mut Blk, i: usize, bits: u32) {
    if let Some(f) = b.flags.get_mut(i) {
        *f |= bits;
    }
}

fn flag_clear(b: &mut Blk, i: usize, bits: u32) {
    if let Some(f) = b.flags.get_mut(i) {
        *f &= !bits;
    }
}

fn decode_bit(b: &mut Blk, cx: usize) -> u32 {
    match b.cx.get_mut(cx) {
        Some(c) => u32::from(mq::decode(&mut b.mq, c)),
        None => 0,
    }
}

/// Marks sample `fi` significant with the given sign and tells its neighbours.
fn set_significant(b: &mut Blk, fi: usize, neg: bool, bp: u32) {
    let st = b.stride;
    flag_or(b, fi, SIG | if neg { NEG } else { 0 });
    let side = |sig: u32, n: u32| sig | if neg { n } else { 0 };
    flag_or(b, fi - st, side(S_SIG, S_NEG));
    flag_or(b, fi + st, side(N_SIG, N_NEG));
    flag_or(b, fi - 1, side(E_SIG, E_NEG));
    flag_or(b, fi + 1, side(W_SIG, W_NEG));
    flag_or(b, fi - st - 1, SE_SIG);
    flag_or(b, fi - st + 1, SW_SIG);
    flag_or(b, fi + st - 1, NE_SIG);
    flag_or(b, fi + st + 1, NW_SIG);
    // Sample (x, y) of the block lives at padded index fi; its magnitude slot follows from it.
    let (x, y) = (fi % st - 1, fi / st - 1);
    if let Some(m) = b.mag.get_mut(y * b.w + x) {
        *m |= 1 << bp;
    }
}

/// Decodes the sign of sample `fi` (D.3.2) and makes it significant.
fn decode_sign_and_set(b: &mut Blk, fi: usize, bp: u32) {
    let f = flag(b, fi);
    let idx = ((f & 0xF) | (((f >> 8) & 0xF) << 4)) as usize;
    let e = SC.get(idx).copied().unwrap_or(0);
    let d = decode_bit(b, CX_SC + usize::from(e & 0xF) - 9);
    let neg = (d ^ u32::from(e >> 4)) != 0;
    set_significant(b, fi, neg, bp);
}

fn sigprop(b: &mut Blk, bp: u32) {
    let mut y0 = 0;
    while y0 < b.h {
        for x in 0..b.w {
            for y in y0..(y0 + 4).min(b.h) {
                let fi = (y + 1) * b.stride + x + 1;
                let f = flag(b, fi);
                if f & (SIG | VISITED) != 0 || f & NEIGHBOURS == 0 {
                    continue;
                }
                let ctx = usize::from(b.zc.get((f & NEIGHBOURS) as usize).copied().unwrap_or(0));
                flag_or(b, fi, VISITED);
                if decode_bit(b, ctx) == 1 {
                    decode_sign_and_set(b, fi, bp);
                }
            }
        }
        y0 += 4;
    }
}

fn magref(b: &mut Blk, bp: u32) {
    let mut y0 = 0;
    while y0 < b.h {
        for x in 0..b.w {
            for y in y0..(y0 + 4).min(b.h) {
                let fi = (y + 1) * b.stride + x + 1;
                let f = flag(b, fi);
                if f & SIG == 0 || f & VISITED != 0 {
                    continue;
                }
                let ctx = if f & REFINED != 0 {
                    CX_MR + 2
                } else if f & NEIGHBOURS != 0 {
                    CX_MR + 1
                } else {
                    CX_MR
                };
                let bit = decode_bit(b, ctx);
                if let Some(m) = b.mag.get_mut(y * b.w + x) {
                    *m |= bit << bp;
                }
                flag_or(b, fi, REFINED);
            }
        }
        y0 += 4;
    }
}

fn cleanup(b: &mut Blk, bp: u32) {
    let st = b.stride;
    let mut y0 = 0;
    while y0 < b.h {
        for x in 0..b.w {
            let mut y = y0;
            let top = (y0 + 1) * st + x + 1;
            let quiet = SIG | VISITED | NEIGHBOURS;
            if y0 + 4 <= b.h && (0..4).all(|k| flag(b, top + k * st) & quiet == 0) {
                if decode_bit(b, CX_RL) == 0 {
                    continue;
                }
                let k = ((decode_bit(b, CX_UNI) << 1) | decode_bit(b, CX_UNI)) as usize;
                y = y0 + k;
                decode_sign_and_set(b, (y + 1) * st + x + 1, bp);
                y += 1;
            }
            while y < (y0 + 4).min(b.h) {
                let fi = (y + 1) * st + x + 1;
                let f = flag(b, fi);
                if f & (SIG | VISITED) == 0 {
                    let ctx =
                        usize::from(b.zc.get((f & NEIGHBOURS) as usize).copied().unwrap_or(0));
                    if decode_bit(b, ctx) == 1 {
                        decode_sign_and_set(b, fi, bp);
                    }
                }
                flag_clear(b, fi, VISITED);
                y += 1;
            }
        }
        y0 += 4;
    }
}

/// One code block to decode.
// A7.2: cold struct, size guard waived
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockSpec {
    pub w: usize,
    pub h: usize,
    /// Orientation of the block's sub-band ([`LL`], [`HL`], [`LH`], [`HH`]).
    pub orient: u8,
    /// Coding passes the block's data holds.
    pub passes: u32,
    /// Index of the bit-plane of the first pass (`Mb - 1 - zero bit-planes`).
    pub start_bp: u32,
}

/// Decodes the code block `spec` from `data`, its codeword segment. The signed coefficients,
/// all decoded planes exact and the undecoded ones zero, go to `out` (`w * h`, row-major).
pub fn decode_block(
    s: &mut Scratch,
    spec: BlockSpec,
    data: &[u8],
    out: &mut [i32],
) -> Result<(), J2kError> {
    let BlockSpec {
        w,
        h,
        orient,
        passes,
        start_bp,
    } = spec;
    if w == 0 || h == 0 || out.len() != w * h {
        return Err(J2kError::BadBlock("block size"));
    }
    out.fill(0);
    if passes == 0 {
        return Ok(());
    }
    if start_bp > 29 || passes > 3 * (start_bp + 1) - 2 {
        return Err(J2kError::BadBlock("more coding passes than bit-planes"));
    }
    let stride = w + 2;
    s.flags.clear();
    s.flags.resize(stride * (h + 2), 0);
    s.mag.clear();
    s.mag.resize(w * h, 0);
    let zc = match orient {
        HL => &ZC_HL,
        LH => &ZC_LH,
        HH => &ZC_HH,
        _ => &ZC_LL,
    };
    let mut cx = [Cx { index: 0, mps: 0 }; NCX];
    if let Some(c) = cx.get_mut(0) {
        c.index = 4;
    }
    if let Some(c) = cx.get_mut(CX_RL) {
        c.index = 3;
    }
    if let Some(c) = cx.get_mut(CX_UNI) {
        c.index = 46;
    }
    let mut b = Blk {
        w,
        h,
        stride,
        flags: &mut s.flags,
        mag: &mut s.mag,
        zc,
        mq: mq::init(data),
        cx,
    };
    let mut bp = start_bp;
    let mut kind = 2u8; // the first pass is a cleanup
    for _ in 0..passes {
        match kind {
            0 => sigprop(&mut b, bp),
            1 => magref(&mut b, bp),
            _ => cleanup(&mut b, bp),
        }
        if kind == 2 {
            kind = 0;
            bp = bp.saturating_sub(1);
        } else {
            kind += 1;
        }
    }
    for (k, o) in out.iter_mut().enumerate() {
        let (x, y) = (k % w, k / w);
        let m = b.mag.get(k).copied().unwrap_or(0) as i32;
        let neg = flag(&b, (y + 1) * stride + x + 1) & NEG != 0;
        *o = if neg { -m } else { m };
    }
    Ok(())
}
