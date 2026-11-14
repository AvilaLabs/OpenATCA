//! The tile: sub-band and code-block geometry (T.800 Annex B), tier-2 packet decoding
//! (B.9-B.10), and assembly of the decoded blocks into the coefficient array. Core (pure).
//!
//! Sub-bands are rows of parallel arrays ([`Bands`]); code blocks are rows of parallel arrays
//! ([`Blocks`]) numbered consecutively, sub-band after sub-band, each sub-band's blocks in
//! raster order. With one precinct per resolution, one packet is one (layer, resolution) pair.

use crate::bits::HeaderBits;
use crate::dwt::Rect;
use crate::error::J2kError;
use crate::header::{Parsed, Siz};
use crate::tagtree::{self, new_tree, TagTree};
use crate::{MAX_CODE_BLOCKS, MAX_PASSES};

/// No segment.
const NONE: u32 = u32::MAX;

/// Sub-bands in decoding order: LL, then HL, LH, HH of each resolution from the lowest.
// A7.2: cold struct, size guard waived (a few dozen rows per image)
#[derive(Debug, Default)]
pub struct Bands {
    /// Resolution level of the band.
    pub res: Vec<u8>,
    pub orient: Vec<u8>,
    /// Band extent in the band's own coordinates.
    pub x0: Vec<u32>,
    pub y0: Vec<u32>,
    pub x1: Vec<u32>,
    pub y1: Vec<u32>,
    /// Where the band's top-left coefficient sits in the coefficient array.
    pub off_x: Vec<u32>,
    pub off_y: Vec<u32>,
    /// Code blocks across and down, and the number of the first block.
    pub nbx: Vec<u32>,
    pub nby: Vec<u32>,
    pub first: Vec<u32>,
    /// Bit-planes of the band's magnitudes (guard bits + exponent - 1).
    pub mb: Vec<u8>,
}

/// Per-code-block packet state, struct-of-arrays.
// A7.2: cold struct, size guard waived (built per image, a few thousand rows)
#[derive(Debug, Default)]
pub struct Blocks {
    pub included: Vec<bool>,
    pub zero_planes: Vec<u8>,
    pub passes: Vec<u16>,
    pub lblock: Vec<u8>,
    /// First and last codeword segment of the block, `NONE` when it has none.
    pub seg_first: Vec<u32>,
    pub seg_last: Vec<u32>,
}

/// Codeword segments: a byte range of the tile body and the next segment of the same block.
// A7.2: cold struct, size guard waived
#[derive(Debug, Default)]
pub struct Segments {
    pub start: Vec<u32>,
    pub len: Vec<u32>,
    pub next: Vec<u32>,
}

/// Everything tier-2 builds for a tile.
// A7.2: cold struct, size guard waived
#[derive(Debug, Default)]
pub struct Tile {
    pub bands: Bands,
    pub blocks: Blocks,
    pub segs: Segments,
    /// Resolution rectangles, lowest first.
    pub res: Vec<Rect>,
    /// Inclusion and zero-bit-plane tag trees, one pair per band.
    incl: Vec<TagTree>,
    zbp: Vec<TagTree>,
    /// Block-grid index of each band's first column and row (floor(x0 / cb width)).
    gx0: Vec<u32>,
    gy0: Vec<u32>,
    xcb: u8,
    ycb: u8,
}

fn ceil_div(a: i64, b: i64) -> i64 {
    -((-a).div_euclid(b))
}

/// Sub-band coordinates (B-15) for decomposition level `nb` and band offsets `xob`, `yob`.
fn band_coord(v: u32, nb: u32, ob: u32) -> u32 {
    let half = if nb == 0 { 0 } else { 1i64 << (nb - 1) };
    ceil_div(i64::from(v) - half * i64::from(ob), 1i64 << nb).max(0) as u32
}

/// Builds the geometry: resolutions, bands and code blocks, with empty packet state.
pub fn build(p: &Parsed) -> Result<Tile, J2kError> {
    let Siz { x0, y0, x1, y1, .. } = p.siz;
    let nl = u32::from(p.style.levels);
    let mut t = Tile {
        xcb: p.style.xcb,
        ycb: p.style.ycb,
        ..Tile::default()
    };
    for r in 0..=nl {
        let sh = i64::from(nl - r);
        let d = 1i64 << sh;
        t.res.push(Rect {
            x0: ceil_div(i64::from(x0), d) as u32,
            y0: ceil_div(i64::from(y0), d) as u32,
            x1: ceil_div(i64::from(x1), d) as u32,
            y1: ceil_div(i64::from(y1), d) as u32,
        });
    }
    let mut total_blocks = 0u64;
    for r in 0..=nl {
        let orients: &[u8] = if r == 0 { &[0] } else { &[1, 2, 3] };
        let nb = if r == 0 { nl } else { nl - r + 1 };
        let rect = t
            .res
            .get(r as usize)
            .copied()
            .ok_or(J2kError::BadMarker("resolution"))?;
        let (rw, rh) = (rect.x1 - rect.x0, rect.y1 - rect.y0);
        // One precinct per resolution: the resolution must not straddle a 2^15 boundary.
        if rw > 0
            && rh > 0
            && (rect.x0 >> 15 != (rect.x1 - 1) >> 15 || rect.y0 >> 15 != (rect.y1 - 1) >> 15)
        {
            return Err(J2kError::Unsupported(
                "more than one precinct per resolution",
            ));
        }
        let low = if r == 0 {
            None
        } else {
            t.res.get(r as usize - 1).copied()
        };
        for &o in orients {
            let (xob, yob) = (u32::from(o & 1), u32::from(o >> 1));
            let (bx0, bx1) = (band_coord(x0, nb, xob), band_coord(x1, nb, xob));
            let (by0, by1) = (band_coord(y0, nb, yob), band_coord(y1, nb, yob));
            let (bw, bh) = (bx1 - bx0, by1 - by0);
            let (off_x, off_y) = match low {
                None => (0, 0),
                Some(l) => (
                    if xob == 1 { l.x1 - l.x0 } else { 0 },
                    if yob == 1 { l.y1 - l.y0 } else { 0 },
                ),
            };
            // The band must tile the resolution exactly.
            let want_w = match low {
                Some(l) if xob == 1 => rw - (l.x1 - l.x0),
                Some(l) => l.x1 - l.x0,
                None => rw,
            };
            let want_h = match low {
                Some(l) if yob == 1 => rh - (l.y1 - l.y0),
                Some(l) => l.y1 - l.y0,
                None => rh,
            };
            if bw != want_w || bh != want_h {
                return Err(J2kError::BadMarker("sub-band does not fit its resolution"));
            }
            let idx = if r == 0 {
                0
            } else {
                1 + 3 * (r as usize - 1) + (o as usize - 1)
            };
            let exp = p
                .quant
                .exps
                .get(idx)
                .copied()
                .ok_or(J2kError::BadMarker("quantization"))?;
            // Mb = G + exponent - 1 (E-2).
            let mb = (u32::from(p.quant.guard) + u32::from(exp)).saturating_sub(1);
            if mb == 0 || mb > 30 {
                return Err(J2kError::BadMarker("magnitude bit-planes out of range"));
            }
            let (cw, ch) = (1u32 << p.style.xcb, 1u32 << p.style.ycb);
            let (gx0, gy0) = (bx0 / cw, by0 / ch);
            let (nbx, nby) = if bw > 0 && bh > 0 {
                (bx1.div_ceil(cw) - gx0, by1.div_ceil(ch) - gy0)
            } else {
                (0, 0)
            };
            let nblk = u64::from(nbx) * u64::from(nby);
            total_blocks += nblk;
            if total_blocks > MAX_CODE_BLOCKS {
                return Err(J2kError::TooManyCodeBlocks(total_blocks));
            }
            let b = &mut t.bands;
            b.res.push(r as u8);
            b.orient.push(o);
            b.x0.push(bx0);
            b.y0.push(by0);
            b.x1.push(bx1);
            b.y1.push(by1);
            b.off_x.push(off_x);
            b.off_y.push(off_y);
            b.nbx.push(nbx);
            b.nby.push(nby);
            b.first.push((total_blocks - nblk) as u32);
            b.mb.push(mb as u8);
            t.gx0.push(gx0);
            t.gy0.push(gy0);
            if nblk > 0 {
                t.incl.push(new_tree(nbx, nby));
                t.zbp.push(new_tree(nbx, nby));
            } else {
                t.incl.push(new_tree(1, 1));
                t.zbp.push(new_tree(1, 1));
            }
        }
    }
    let n = total_blocks as usize;
    t.blocks = Blocks {
        included: vec![false; n],
        zero_planes: vec![0; n],
        passes: vec![0; n],
        lblock: vec![3; n],
        seg_first: vec![NONE; n],
        seg_last: vec![NONE; n],
    };
    Ok(t)
}

/// Number of coding passes (Table B.4); the codewords are 0, 10, 11xx, 1111 xxxxx and
/// 1111 11111 xxxxxxx.
fn read_passes(hb: &mut HeaderBits) -> Result<u32, J2kError> {
    if hb.bit()? == 0 {
        return Ok(1);
    }
    if hb.bit()? == 0 {
        return Ok(2);
    }
    let v = hb.bits(2)?;
    if v < 3 {
        return Ok(3 + v);
    }
    let v = hb.bits(5)?;
    if v < 31 {
        return Ok(6 + v);
    }
    Ok(37 + hb.bits(7)?)
}

/// Reads every packet of the tile body (LRCP order) into the block state.
pub fn read_packets(t: &mut Tile, body: &[u8], layers: u16) -> Result<(), J2kError> {
    let mut pos = 0usize;
    let nres = t.res.len();
    let mut pending: Vec<(u32, u32)> = Vec::new();
    for layer in 0..u32::from(layers) {
        for r in 0..nres {
            let rect = t
                .res
                .get(r)
                .copied()
                .ok_or(J2kError::BadPacket("resolution"))?;
            if rect.x1 == rect.x0 || rect.y1 == rect.y0 {
                continue; // no precinct, so no packet
            }
            read_packet(t, body, &mut pos, layer, r, &mut pending)?;
        }
    }
    if pos != body.len() {
        return Err(J2kError::BadPacket("data after the last packet"));
    }
    Ok(())
}

fn read_packet(
    t: &mut Tile,
    body: &[u8],
    pos: &mut usize,
    layer: u32,
    r: usize,
    pending: &mut Vec<(u32, u32)>,
) -> Result<(), J2kError> {
    let rest = body.get(*pos..).ok_or(J2kError::Truncated("packet"))?;
    let mut hb = HeaderBits::new(rest);
    pending.clear();
    let (first_band, nbands) = if r == 0 { (0, 1) } else { (1 + 3 * (r - 1), 3) };
    if hb.bit()? == 1 {
        for s in first_band..first_band + nbands {
            let (Some(&nbx), Some(&nby), Some(&first)) =
                (t.bands.nbx.get(s), t.bands.nby.get(s), t.bands.first.get(s))
            else {
                return Err(J2kError::BadPacket("band"));
            };
            for k in 0..nbx * nby {
                let blk = (first + k) as usize;
                let bad = J2kError::BadPacket("block");
                let was = *t.blocks.included.get(blk).ok_or_else(|| bad.clone())?;
                let incl_now = if was {
                    hb.bit()? == 1
                } else {
                    let tree = t.incl.get_mut(s).ok_or_else(|| bad.clone())?;
                    tagtree::decode(tree, k, layer + 1, &mut hb)?
                };
                if !incl_now {
                    continue;
                }
                if !was {
                    let tree = t.zbp.get_mut(s).ok_or_else(|| bad.clone())?;
                    let mut thr = 1u32;
                    while !tagtree::decode(tree, k, thr, &mut hb)? {
                        thr += 1;
                        if thr > 64 {
                            return Err(J2kError::BadPacket("zero bit-planes out of range"));
                        }
                    }
                    let z = tagtree::leaf_value(tree, k).ok_or_else(|| bad.clone())?;
                    *t.blocks
                        .zero_planes
                        .get_mut(blk)
                        .ok_or_else(|| bad.clone())? = z as u8;
                    *t.blocks.included.get_mut(blk).ok_or_else(|| bad.clone())? = true;
                }
                let np = read_passes(&mut hb)?;
                let lblock = t.blocks.lblock.get_mut(blk).ok_or_else(|| bad.clone())?;
                while hb.bit()? == 1 {
                    *lblock += 1;
                    if *lblock > 31 {
                        return Err(J2kError::BadPacket("length indicator too long"));
                    }
                }
                let bits = u32::from(*lblock) + (31 - np.leading_zeros());
                if bits > 32 {
                    return Err(J2kError::BadPacket("length field too wide"));
                }
                let len = hb.bits(bits)?;
                let tp = t.blocks.passes.get_mut(blk).ok_or_else(|| bad.clone())?;
                let total = u32::from(*tp) + np;
                if total > MAX_PASSES {
                    return Err(J2kError::BadPacket("too many coding passes"));
                }
                *tp = total as u16;
                pending.push((blk as u32, len));
            }
        }
    }
    *pos += hb.finish()?;
    for &(blk, len) in pending.iter() {
        let end = pos
            .checked_add(len as usize)
            .filter(|&e| e <= body.len())
            .ok_or(J2kError::Truncated("packet body"))?;
        let id = t.segs.start.len() as u32;
        t.segs.start.push(*pos as u32);
        t.segs.len.push(len);
        t.segs.next.push(NONE);
        let bad = J2kError::BadPacket("block");
        let last = *t
            .blocks
            .seg_last
            .get(blk as usize)
            .ok_or_else(|| bad.clone())?;
        if last == NONE {
            *t.blocks
                .seg_first
                .get_mut(blk as usize)
                .ok_or_else(|| bad.clone())? = id;
        } else if let Some(n) = t.segs.next.get_mut(last as usize) {
            *n = id;
        }
        *t.blocks.seg_last.get_mut(blk as usize).ok_or(bad)? = id;
        *pos = end;
    }
    Ok(())
}

/// Decodes every included block (tier-1) and stores its coefficients in `coef`
/// (`width` samples per row, bands placed as [`crate::dwt`] expects).
pub fn decode_blocks(
    t: &Tile,
    body: &[u8],
    coef: &mut [i32],
    width: usize,
) -> Result<(), J2kError> {
    let mut scratch = crate::t1::Scratch::default();
    let mut data: Vec<u8> = Vec::new();
    let mut out: Vec<i32> = Vec::new();
    let (cw, ch) = (1u32 << t.xcb, 1u32 << t.ycb);
    for s in 0..t.bands.orient.len() {
        let g = |v: &[u32]| v.get(s).copied().unwrap_or(0);
        let (nbx, nby, first) = (g(&t.bands.nbx), g(&t.bands.nby), g(&t.bands.first));
        let (bx0, by0, bx1, by1) = (
            g(&t.bands.x0),
            g(&t.bands.y0),
            g(&t.bands.x1),
            g(&t.bands.y1),
        );
        let (off_x, off_y) = (g(&t.bands.off_x), g(&t.bands.off_y));
        let orient = t.bands.orient.get(s).copied().unwrap_or(0);
        let mb = u32::from(t.bands.mb.get(s).copied().unwrap_or(0));
        let (gx0, gy0) = (g(&t.gx0), g(&t.gy0));
        for by in 0..nby {
            for bx in 0..nbx {
                let blk = (first + by * nbx + bx) as usize;
                let passes = u32::from(t.blocks.passes.get(blk).copied().unwrap_or(0));
                if passes == 0 {
                    continue;
                }
                let x0 = ((gx0 + bx) * cw).max(bx0);
                let x1 = ((gx0 + bx + 1) * cw).min(bx1);
                let y0 = ((gy0 + by) * ch).max(by0);
                let y1 = ((gy0 + by + 1) * ch).min(by1);
                let (w, h) = ((x1 - x0) as usize, (y1 - y0) as usize);
                let zero = u32::from(t.blocks.zero_planes.get(blk).copied().unwrap_or(0));
                if zero >= mb {
                    return Err(J2kError::BadBlock(
                        "zero bit-planes exceed the band's planes",
                    ));
                }
                data.clear();
                let mut sg = t.blocks.seg_first.get(blk).copied().unwrap_or(NONE);
                while sg != NONE {
                    let st = t.segs.start.get(sg as usize).copied().unwrap_or(0) as usize;
                    let ln = t.segs.len.get(sg as usize).copied().unwrap_or(0) as usize;
                    data.extend_from_slice(body.get(st..st + ln).unwrap_or(&[]));
                    sg = t.segs.next.get(sg as usize).copied().unwrap_or(NONE);
                }
                out.clear();
                out.resize(w * h, 0);
                crate::t1::decode_block(
                    &mut scratch,
                    crate::t1::BlockSpec {
                        w,
                        h,
                        orient,
                        passes,
                        start_bp: mb - 1 - zero,
                    },
                    &data,
                    &mut out,
                )?;
                for row in 0..h {
                    let dst = (off_y + (y0 - by0)) as usize + row;
                    let col = (off_x + (x0 - bx0)) as usize;
                    let (Some(d), Some(sr)) = (
                        coef.get_mut(dst * width + col..dst * width + col + w),
                        out.get(row * w..row * w + w),
                    ) else {
                        return Err(J2kError::BadBlock("block outside the coefficient array"));
                    };
                    d.copy_from_slice(sr);
                }
            }
        }
    }
    Ok(())
}
