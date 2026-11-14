//! MQ arithmetic decoder (T.800 Annex C, software conventions of C.3). Core (pure).
//!
//! Reads one code block's codeword segment. Past the end of the segment the decoder is fed
//! 1-bits, as the standard says for a terminating marker (C.3.4), so a short segment decodes
//! to something deterministic instead of failing.

/// Probability states of Table C.2: Qe, next state after an MPS, next state after an LPS, and
/// whether an LPS switches the sense of the MPS.
const QE: [u16; 47] = [
    0x5601, 0x3401, 0x1801, 0x0AC1, 0x0521, 0x0221, 0x5601, 0x5401, 0x4801, 0x3801, 0x3001, 0x2401,
    0x1C01, 0x1601, 0x5601, 0x5401, 0x5101, 0x4801, 0x3801, 0x3401, 0x3001, 0x2801, 0x2401, 0x2201,
    0x1C01, 0x1801, 0x1601, 0x1401, 0x1201, 0x1101, 0x0AC1, 0x09C1, 0x08A1, 0x0521, 0x0441, 0x02A1,
    0x0221, 0x0141, 0x0111, 0x0085, 0x0049, 0x0025, 0x0015, 0x0009, 0x0005, 0x0001, 0x5601,
];
const NMPS: [u8; 47] = [
    1, 2, 3, 4, 5, 38, 7, 8, 9, 10, 11, 12, 13, 29, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26,
    27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 45, 46,
];
const NLPS: [u8; 47] = [
    1, 6, 9, 12, 29, 33, 6, 14, 14, 14, 17, 18, 20, 21, 14, 14, 15, 16, 17, 18, 19, 19, 20, 21, 22,
    23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 46,
];
const SWITCH: [bool; 47] = {
    let mut s = [false; 47];
    s[0] = true;
    s[6] = true;
    s[14] = true;
    s
};

/// Context state: probability index and current MPS, one byte pair per context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct Cx {
    pub index: u8,
    pub mps: u8,
}
// Budget: held per context, 19 per code-block decoder.
const _: () = assert!(core::mem::size_of::<Cx>() == 2);

/// The decoder registers over one codeword segment.
// A7.2: cold struct, size guard waived (one per code block being decoded)
pub struct Mq<'a> {
    data: &'a [u8],
    /// Index of the byte BP points at.
    bp: usize,
    c: u32,
    a: u32,
    ct: u32,
}

fn byte(data: &[u8], i: usize) -> u32 {
    // Past the end: 0xFF, which with the next 0xFF reads as a marker and feeds 1-bits.
    data.get(i).map_or(0xFF, |&b| u32::from(b))
}

/// INITDEC (Figure C.20).
pub fn init(data: &[u8]) -> Mq<'_> {
    let mut mq = Mq {
        data,
        bp: 0,
        c: byte(data, 0) << 16,
        a: 0,
        ct: 0,
    };
    bytein(&mut mq);
    mq.c <<= 7;
    mq.ct -= 7;
    mq.a = 0x8000;
    mq
}

/// BYTEIN (Figure C.19).
fn bytein(mq: &mut Mq) {
    if byte(mq.data, mq.bp) == 0xFF {
        if byte(mq.data, mq.bp + 1) > 0x8F {
            mq.c = mq.c.wrapping_add(0xFF00);
            mq.ct = 8;
        } else {
            mq.bp += 1;
            mq.c = mq.c.wrapping_add(byte(mq.data, mq.bp) << 9);
            mq.ct = 7;
        }
    } else {
        mq.bp += 1;
        mq.c = mq.c.wrapping_add(byte(mq.data, mq.bp) << 8);
        mq.ct = 8;
    }
}

/// DECODE (Figure C.15) with RENORMD, for context `cx`.
pub fn decode(mq: &mut Mq, cx: &mut Cx) -> u8 {
    let i = usize::from(cx.index);
    let qe = u32::from(QE.get(i).copied().unwrap_or(0x5601));
    mq.a -= qe;
    let d;
    if (mq.c >> 16) < qe {
        // LPS_EXCHANGE (Figure C.17)
        if mq.a < qe {
            d = cx.mps;
            cx.index = NMPS.get(i).copied().unwrap_or(46);
        } else {
            d = 1 - cx.mps;
            if SWITCH.get(i).copied().unwrap_or(false) {
                cx.mps = 1 - cx.mps;
            }
            cx.index = NLPS.get(i).copied().unwrap_or(46);
        }
        mq.a = qe;
        renorm(mq);
    } else {
        mq.c = mq.c.wrapping_sub(qe << 16);
        if mq.a & 0x8000 == 0 {
            // MPS_EXCHANGE (Figure C.16)
            if mq.a < qe {
                d = 1 - cx.mps;
                if SWITCH.get(i).copied().unwrap_or(false) {
                    cx.mps = 1 - cx.mps;
                }
                cx.index = NLPS.get(i).copied().unwrap_or(46);
            } else {
                d = cx.mps;
                cx.index = NMPS.get(i).copied().unwrap_or(46);
            }
            renorm(mq);
        } else {
            d = cx.mps;
        }
    }
    d
}

/// RENORMD (Figure C.18).
fn renorm(mq: &mut Mq) {
    loop {
        if mq.ct == 0 {
            bytein(mq);
        }
        mq.a <<= 1;
        mq.c <<= 1;
        mq.ct -= 1;
        if mq.a & 0x8000 != 0 {
            break;
        }
    }
}
