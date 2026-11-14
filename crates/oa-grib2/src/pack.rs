//! Data representation (section 5), data (section 7) and bitmap (section 6). Core (pure).
//!
//! * 5.0 simple packing; 5.2 complex packing (groups); 5.3 complex packing with spatial
//!   differencing of order 1 or 2. 5.40 (JPEG 2000) and 5.41 (PNG) are refused by number.
//! * Value of a packed integer `X`: `(R + X * 2^E) / 10^D`, computed in f64 and stored as f32.
//! * Complex packing missing-value management 1 and 2: in a group of width `w > 0` the code
//!   `2^w - 1` is the primary missing value and, with management 2, `2^w - 2` is the secondary;
//!   in a group of width 0 the same codes of the group reference (`nbits` wide) mark the whole
//!   group missing. Missing points come out as NaN.
//! * Spatial differencing runs over the non-missing points only: missing points take no part in
//!   the differenced sequence, and the first `order` values are the first non-missing ones. This
//!   reading was checked on the ecCodes file `gfs.complex.mvmu.grib2` (5.3, order 2, management
//!   1, 54% missing), which decodes to a snow depth of 0 to 2.8 m; differencing across all
//!   points does not give a physical field.

use crate::bits::{f32_at, sign_mag, u16_at, u32_at, u8_at, BitReader};
use crate::error::Grib2Error;

/// Field value for a point that has no data.
pub const MISSING: f32 = f32::NAN;

/// Most groups a complex-packed field may declare (one per point is the worst case).
pub const MAX_GROUPS: u32 = crate::MAX_GRID_POINTS as u32;

/// Complex-packing parameters (octets 22 onward of templates 5.2 and 5.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct Complex {
    pub missing_mgmt: u8,
    pub ngroups: u32,
    pub width_ref: u8,
    pub width_bits: u8,
    pub length_ref: u32,
    pub length_inc: u8,
    pub last_length: u32,
    pub length_bits: u8,
    /// 0 for template 5.2; 1 or 2 for 5.3.
    pub diff_order: u8,
    /// Octets per spatial-differencing descriptor.
    pub extra_octets: u8,
}

/// What section 5 says.
#[derive(Debug, Clone, Copy, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct Repr {
    pub template: u16,
    /// Number of values in the data section (non-missing points when a bitmap is used).
    pub ndata: u32,
    pub r: f32,
    pub e: i16,
    pub d: i16,
    pub nbits: u8,
    pub complex: Option<Complex>,
}

/// Reads section 5 (whole section).
pub fn parse_repr(sect: &[u8]) -> Result<Repr, Grib2Error> {
    let t = Grib2Error::Truncated("data representation");
    let ndata = u32_at(sect, 5).ok_or_else(|| t.clone())?;
    let template = u16_at(sect, 9).ok_or_else(|| t.clone())?;
    let need = match template {
        0 => 21,
        2 => 47,
        3 => 49,
        other => return Err(Grib2Error::UnsupportedPacking(other)),
    };
    if sect.len() < need {
        return Err(Grib2Error::BadSectionLength {
            section: 5,
            length: sect.len() as u32,
        });
    }
    let r = f32_at(sect, 11).ok_or_else(|| t.clone())?;
    let e = sign_mag(sect, 15, 2).ok_or_else(|| t.clone())? as i16;
    let d = sign_mag(sect, 17, 2).ok_or_else(|| t.clone())? as i16;
    let nbits = u8_at(sect, 19).ok_or_else(|| t.clone())?;
    if !r.is_finite() {
        return Err(Grib2Error::BadPacking("reference value is not finite"));
    }
    if nbits > 32 {
        return Err(Grib2Error::BadPacking("more than 32 bits per value"));
    }
    if !(-400..=400).contains(&e) || !(-40..=40).contains(&d) {
        return Err(Grib2Error::BadPacking("scale factor out of range"));
    }
    let complex = if template == 0 {
        None
    } else {
        if u8_at(sect, 21).ok_or_else(|| t.clone())? != 1 {
            return Err(Grib2Error::BadPacking(
                "group splitting method is not general",
            ));
        }
        let missing_mgmt = u8_at(sect, 22).ok_or_else(|| t.clone())?;
        if missing_mgmt > 2 {
            return Err(Grib2Error::BadPacking("missing value management code"));
        }
        let ngroups = u32_at(sect, 31).ok_or_else(|| t.clone())?;
        let width_ref = u8_at(sect, 35).ok_or_else(|| t.clone())?;
        let width_bits = u8_at(sect, 36).ok_or_else(|| t.clone())?;
        let length_ref = u32_at(sect, 37).ok_or_else(|| t.clone())?;
        let length_inc = u8_at(sect, 41).ok_or_else(|| t.clone())?;
        let last_length = u32_at(sect, 42).ok_or_else(|| t.clone())?;
        let length_bits = u8_at(sect, 46).ok_or_else(|| t.clone())?;
        let (diff_order, extra_octets) = if template == 3 {
            let o = u8_at(sect, 47).ok_or_else(|| t.clone())?;
            let x = u8_at(sect, 48).ok_or_else(|| t.clone())?;
            if o != 1 && o != 2 {
                return Err(Grib2Error::BadPacking(
                    "spatial differencing order is not 1 or 2",
                ));
            }
            if x == 0 || x > 8 {
                return Err(Grib2Error::BadPacking(
                    "descriptor width is not 1 to 8 octets",
                ));
            }
            (o, x)
        } else {
            (0, 0)
        };
        if ngroups == 0 || ngroups > MAX_GROUPS {
            return Err(Grib2Error::BadPacking("group count out of bounds"));
        }
        if width_bits > 32 || length_bits > 32 || width_ref > 32 {
            return Err(Grib2Error::BadPacking("group parameter wider than 32 bits"));
        }
        Some(Complex {
            missing_mgmt,
            ngroups,
            width_ref,
            width_bits,
            length_ref,
            length_inc,
            last_length,
            length_bits,
            diff_order,
            extra_octets,
        })
    };
    Ok(Repr {
        template,
        ndata,
        r,
        e,
        d,
        nbits,
        complex,
    })
}

/// Integer used internally for a missing point; never a legal packed value.
const MISS: i64 = i64::MIN;

fn scale_value(rp: &Repr, x: i64) -> f32 {
    if x == MISS {
        return MISSING;
    }
    let v =
        (f64::from(rp.r) + x as f64 * 2f64.powi(i32::from(rp.e))) * 10f64.powi(-i32::from(rp.d));
    v as f32
}

/// Unpacks `payload` (section 7 without its 5-octet header) into `rp.ndata` values.
pub fn unpack(rp: &Repr, payload: &[u8]) -> Result<Vec<f32>, Grib2Error> {
    let n = rp.ndata as usize;
    match rp.complex {
        None => unpack_simple(rp, payload, n),
        Some(c) => unpack_complex(rp, &c, payload, n),
    }
}

fn unpack_simple(rp: &Repr, payload: &[u8], n: usize) -> Result<Vec<f32>, Grib2Error> {
    let bits = u64::from(rp.nbits);
    let need = (n as u64 * bits).div_ceil(8);
    if (payload.len() as u64) < need {
        return Err(Grib2Error::BadPacking(
            "data section shorter than the packed values",
        ));
    }
    let mut out = Vec::with_capacity(n);
    let mut br = BitReader::new(payload);
    for _ in 0..n {
        let x = br
            .read(u32::from(rp.nbits))
            .ok_or(Grib2Error::BadPacking("data ran out"))?;
        out.push(scale_value(rp, i64::from(x)));
    }
    Ok(out)
}

fn unpack_complex(
    rp: &Repr,
    c: &Complex,
    payload: &[u8],
    n: usize,
) -> Result<Vec<f32>, Grib2Error> {
    let short = Grib2Error::BadPacking("data section shorter than its descriptors");
    let ng = c.ngroups as usize;
    let mut br = BitReader::new(payload);
    // Spatial-differencing descriptors: `order` first values, then the minimum.
    let mut first = [0i64; 2];
    let mut dmin = 0i64;
    if c.diff_order > 0 {
        let w = usize::from(c.extra_octets);
        for k in 0..usize::from(c.diff_order) + 1 {
            let off = k * w;
            let v = sign_mag(payload, off, w).ok_or_else(|| short.clone())?;
            if k < usize::from(c.diff_order) {
                if let Some(slot) = first.get_mut(k) {
                    *slot = v;
                }
            } else {
                dmin = v;
            }
        }
        for _ in 0..(usize::from(c.diff_order) + 1) * w {
            br.read(8).ok_or_else(|| short.clone())?;
        }
    }
    // The three group tables must fit before anything is allocated for them.
    let table_bits = (ng as u64)
        .saturating_mul(u64::from(rp.nbits) + u64::from(c.width_bits) + u64::from(c.length_bits));
    if table_bits > (br.remaining_bits() as u64).saturating_add(24) {
        return Err(short);
    }
    let mut refs = Vec::with_capacity(ng);
    for _ in 0..ng {
        refs.push(br.read(u32::from(rp.nbits)).ok_or_else(|| short.clone())?);
    }
    br.align();
    let mut widths = Vec::with_capacity(ng);
    for _ in 0..ng {
        let w = u32::from(c.width_ref)
            + br.read(u32::from(c.width_bits))
                .ok_or_else(|| short.clone())?;
        if w > 32 {
            return Err(Grib2Error::BadPacking("group width over 32 bits"));
        }
        widths.push(w);
    }
    br.align();
    let mut lengths = Vec::with_capacity(ng);
    let mut total: u64 = 0;
    for g in 0..ng {
        // The table has NG entries; the last is read and then replaced by the true length.
        let k = br
            .read(u32::from(c.length_bits))
            .ok_or_else(|| short.clone())?;
        let l = if g + 1 == ng {
            u64::from(c.last_length)
        } else {
            u64::from(c.length_ref) + u64::from(k) * u64::from(c.length_inc)
        };
        total = total.saturating_add(l);
        lengths.push(l as usize);
    }
    br.align();
    if total != n as u64 {
        return Err(Grib2Error::BadPacking(
            "group lengths do not add up to the point count",
        ));
    }
    // Packed integers, one per point, with MISS for missing ones.
    let mut ints: Vec<i64> = Vec::with_capacity(n);
    let all_ref = if rp.nbits == 0 {
        u64::MAX
    } else {
        (1u64 << rp.nbits) - 1
    };
    for g in 0..ng {
        let (r, w, l) = match (refs.get(g), widths.get(g), lengths.get(g)) {
            (Some(&r), Some(&w), Some(&l)) => (i64::from(r), w, l),
            _ => return Err(short),
        };
        if w == 0 {
            let missing = c.missing_mgmt >= 1
                && rp.nbits > 0
                && (r as u64 == all_ref || (c.missing_mgmt == 2 && r as u64 + 1 == all_ref));
            let v = if missing { MISS } else { r };
            ints.extend(std::iter::repeat_n(v, l));
        } else {
            let prim = (1u64 << w) - 1;
            for _ in 0..l {
                let raw = br.read(w).ok_or_else(|| short.clone())?;
                let v = u64::from(raw);
                let missing =
                    c.missing_mgmt >= 1 && (v == prim || (c.missing_mgmt == 2 && v + 1 == prim));
                ints.push(if missing { MISS } else { r + i64::from(raw) });
            }
        }
    }
    if c.diff_order > 0 {
        undo_differencing(&mut ints, c.diff_order, &first, dmin)?;
    }
    Ok(ints.iter().map(|&x| scale_value(rp, x)).collect())
}

fn undo_differencing(
    ints: &mut [i64],
    order: u8,
    first: &[i64; 2],
    dmin: i64,
) -> Result<(), Grib2Error> {
    let over = Grib2Error::BadPacking("spatial differencing overflow");
    let (mut p1, mut p2) = (0i64, 0i64);
    let mut k = 0usize;
    for x in ints.iter_mut() {
        if *x == MISS {
            continue;
        }
        let v = if k < usize::from(order) {
            first.get(k).copied().unwrap_or(0)
        } else {
            let raw = x.checked_add(dmin).ok_or_else(|| over.clone())?;
            if order == 1 {
                raw.checked_add(p1).ok_or_else(|| over.clone())?
            } else {
                raw.checked_add(p1.checked_mul(2).ok_or_else(|| over.clone())?)
                    .and_then(|s| s.checked_sub(p2))
                    .ok_or_else(|| over.clone())?
            }
        };
        p2 = p1;
        p1 = v;
        *x = v;
        k += 1;
    }
    Ok(())
}

/// A bitmap: bit `k` set means point `k` of the grid has a value.
#[derive(Debug, Clone, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct Bitmap {
    pub bits: Vec<u8>,
    pub npoints: u32,
    pub set: u32,
}

/// What section 6 says.
#[derive(Debug, Clone, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub enum BitmapSpec {
    /// Indicator 255: every point has a value.
    None,
    /// Indicator 254: the bitmap of the previous field applies.
    Previous,
    /// Indicator 0: a bitmap follows.
    Present(Bitmap),
}

/// Reads section 6 (whole section) for a grid of `npoints`.
pub fn parse_bitmap(sect: &[u8], npoints: u32) -> Result<BitmapSpec, Grib2Error> {
    let ind = u8_at(sect, 5).ok_or(Grib2Error::Truncated("bitmap section"))?;
    match ind {
        255 => Ok(BitmapSpec::None),
        254 => Ok(BitmapSpec::Previous),
        0 => {
            let body = sect.get(6..).ok_or(Grib2Error::Truncated("bitmap"))?;
            let need = (npoints as usize).div_ceil(8);
            if body.len() < need {
                return Err(Grib2Error::BadBitmap("shorter than the grid"));
            }
            let bits: Vec<u8> = body.iter().copied().take(need).collect();
            let mut set = 0u32;
            for k in 0..npoints as usize {
                if bit_set(&bits, k) {
                    set += 1;
                }
            }
            Ok(BitmapSpec::Present(Bitmap { bits, npoints, set }))
        }
        _ => Err(Grib2Error::BadBitmap(
            "predefined bitmaps are not supported",
        )),
    }
}

/// Bit `k`, most significant bit of octet 0 first.
pub fn bit_set(bits: &[u8], k: usize) -> bool {
    bits.get(k / 8).is_some_and(|b| b & (0x80 >> (k % 8)) != 0)
}

/// Spreads `values` (one per set bit) over the grid, NaN where the bitmap is clear.
pub fn apply_bitmap(bm: &Bitmap, values: &[f32]) -> Result<Vec<f32>, Grib2Error> {
    if values.len() != bm.set as usize {
        return Err(Grib2Error::BadBitmap(
            "set bits differ from the number of data values",
        ));
    }
    let mut it = values.iter();
    let mut out = Vec::with_capacity(bm.npoints as usize);
    for k in 0..bm.npoints as usize {
        if bit_set(&bm.bits, k) {
            out.push(*it.next().ok_or(Grib2Error::BadBitmap("values exhausted"))?);
        } else {
            out.push(MISSING);
        }
    }
    Ok(out)
}
