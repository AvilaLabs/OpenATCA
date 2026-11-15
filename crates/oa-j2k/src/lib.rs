#![forbid(unsafe_code)]
//! oa-j2k: a JPEG 2000 Part 1 codestream decoder for exactly the profile NOAA's GRIB2 files use
//! (template 5.40), written from ITU-T T.800 (08/2002) = ISO/IEC 15444-1. Core (pure): bytes in,
//! samples out.
//!
//! Supported: a raw codestream (`SOC` ... `EOC`, no JP2 box) with one tile in one tile-part,
//! one unsubsampled component of 1 to 16 bits, LRCP progression with any number of layers, the
//! reversible 5-3 wavelet with 0 to 32 levels, no quantization, code blocks of any legal size,
//! and code-block style 0 (no bypass, reset, termination on each pass, vertically causal
//! contexts, predictable termination or segmentation symbols). Everything else is refused with
//! a typed [`J2kError`]: the other progression orders, precinct partitioning, SOP and EPH
//! markers, the 9-7 wavelet, scalar quantization, several tiles, tile-parts or components, and
//! the POC, PPM, PPT and RGN markers.
//!
//! * [`header`]: marker segments.
//! * [`tile`]: sub-band and code-block geometry, tag trees ([`tagtree`]) and packets.
//! * [`t1`]: the EBCOT block decoder over the MQ decoder ([`mq`]).
//! * [`dwt`]: the inverse 5-3 transform.
//!
//! Every input is bounded by the constants below and refused when it exceeds them (RULESET E5).
//! Decoded magnitudes keep every bit-plane the codestream holds; a block whose passes were cut
//! short (lossy truncation) is reconstructed with the missing planes zero, with no mid-point
//! offset, which is exact for the lossless streams this decoder is for.

pub mod bits;
pub mod dwt;
pub mod error;
pub mod header;
pub mod mq;
pub mod t1;
pub mod tagtree;
pub mod tile;

#[cfg(test)]
mod testenc;
#[cfg(test)]
mod tests;

pub use error::J2kError;

/// Limit: longest side of the image, 2^15 (RAP is 451, HRRR 1799), so a resolution never spans
/// more than one default 2^15 precinct from the origin.
pub const MAX_DIM: u32 = 1 << 15;
/// Limit: samples in the image, 16 Mi, the same as oa-grib2's largest grid.
pub const MAX_SAMPLES: u64 = 16 * 1024 * 1024;
/// Limit: bits per sample (GRIB2 packs at most 31; NOAA's JPEG 2000 fields use up to 15).
pub const MAX_BITS: u8 = 16;
/// Limit: decomposition levels (the standard allows 32; images here have at most 15 levels).
pub const MAX_LEVELS: u8 = 15;
/// Limit: quality layers.
pub const MAX_LAYERS: u16 = 64;
/// Limit: code blocks in the tile, 256 Ki: a 16 Mi image at 8 x 8 (RAP has 150 at 64 x 64).
pub const MAX_CODE_BLOCKS: u64 = 1 << 18;
/// Limit: coding passes of one block, 3 per bit-plane over at most 30 planes, less 2.
pub const MAX_PASSES: u32 = 88;

/// Limit: tier-1 work with no data behind it, 4 Mi sample visits (the cost of one fully coded
/// 64 x 64 block is 0.4 Mi, a few of them are all a tiny stream may ask for).
pub const WORK_FLOOR: u64 = 1 << 22;
/// Limit: tier-1 sample visits per byte of tile body, on top of [`WORK_FLOOR`]. Real NOAA fields
/// (RAP, HRRR, GEFS) reach 590, so this leaves a margin of about 14.
pub const WORK_PER_BYTE: u64 = 8192;

/// The tier-1 work a codestream may ask for, in sample visits (one visit is one sample in one
/// coding pass). A block of a `bits`-deep image has at most `3 * (bits + 8) - 2` passes (the
/// largest sub-band gain is 2 bits and a stream has at most 7 guard bits), so no honest stream
/// visits a sample more than `3 * (bits + 8)` times. The work must also be backed by data: at
/// most [`WORK_PER_BYTE`] visits per byte of tile body plus [`WORK_FLOOR`].
pub fn work_budget(samples: u64, bits: u8, body_bytes: usize) -> u64 {
    let by_size = samples.saturating_mul(3 * (u64::from(bits) + 8));
    let by_data = WORK_FLOOR.saturating_add(WORK_PER_BYTE.saturating_mul(body_bytes as u64));
    by_size.min(by_data)
}

/// A decoded image: `width * height` samples, row-major, already DC-level shifted.
// A7.2: cold struct, size guard waived
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub bits: u8,
    pub signed: bool,
    pub samples: Vec<i32>,
}

/// Decodes a codestream.
pub fn decode(data: &[u8]) -> Result<Image, J2kError> {
    let p = header::parse(data)?;
    let mut t = tile::build(&p)?;
    tile::read_packets(&mut t, p.body, p.layers)?;
    let budget = work_budget(
        u64::from(p.siz.x1 - p.siz.x0) * u64::from(p.siz.y1 - p.siz.y0),
        p.siz.bits,
        p.body.len(),
    );
    let work = tile::work(&t);
    if work > budget {
        return Err(J2kError::WorkBudget { work, budget });
    }
    let (w, h) = (p.siz.x1 - p.siz.x0, p.siz.y1 - p.siz.y0);
    let mut coef = vec![0i32; w as usize * h as usize];
    tile::decode_blocks(&t, p.body, &mut coef, w as usize)?;
    dwt::inverse(&mut coef, w as usize, &t.res);
    // G.1.2: unsigned samples carry a level shift of 2^(bits-1), then are clipped to the range.
    let bits = u32::from(p.siz.bits);
    let (shift, lo, hi) = if p.siz.signed {
        (0, -(1i32 << (bits - 1)), (1i32 << (bits - 1)) - 1)
    } else {
        (1i32 << (bits - 1), 0, (1i32 << bits) - 1)
    };
    for v in &mut coef {
        *v = v.saturating_add(shift).clamp(lo, hi);
    }
    Ok(Image {
        width: w,
        height: h,
        bits: p.siz.bits,
        signed: p.siz.signed,
        samples: coef,
    })
}
