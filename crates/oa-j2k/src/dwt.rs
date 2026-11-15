//! Inverse reversible 5-3 wavelet transform (T.800 Annex F.3, equations F-5 and F-6). Core
//! (pure). Integer exact: the shifts are arithmetic, so they floor negative sums as the
//! standard's floor does.
//!
//! The coefficients of all resolutions sit in one array in the usual layout: the lowest
//! resolution top-left, and each higher resolution's HL, LH and HH bands to the right of, below
//! and diagonal to the reconstruction so far. One level interleaves the four bands by the parity
//! of the sample's coordinate in the resolution grid (even: low-pass), then runs the horizontal
//! lifting over every row and the vertical lifting over every column (F.3.2).

use crate::error::J2kError;

/// A resolution's rectangle in its own sample grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// A7.2: cold struct, size guard waived (one per resolution level)
pub struct Rect {
    pub x0: u32,
    pub y0: u32,
    pub x1: u32,
    pub y1: u32,
}

/// The refusal for a line whose lifting leaves the 32-bit coefficient range.
const OVERFLOW: J2kError = J2kError::BadBlock("wavelet coefficients overflow 32 bits");

/// 1D_SR on one interleaved line `s`. `p` is the parity (0 or 1) of the global coordinate of
/// `s[0]`; samples whose global coordinate is even are the low-pass ones. The arithmetic is done
/// in 64 bits and each result is checked to fit `i32`, so a stream whose coefficients are out of
/// range for its precision is refused instead of overflowing; `s` is unspecified after an error.
pub fn lift_line(s: &mut [i32], p: usize) -> Result<(), J2kError> {
    let n = s.len();
    if n == 0 {
        return Ok(());
    }
    if n == 1 {
        if p == 1 {
            if let Some(v) = s.first_mut() {
                *v /= 2;
            }
        }
        return Ok(());
    }
    // Whole-sample symmetric extension needs only one reflected neighbour at each end.
    let at = |s: &[i32], i: usize, up: bool| -> i64 {
        // The neighbour below (up = false: i - 1) or above (up = true: i + 1), reflected at ends.
        let j = if up {
            if i + 1 < n {
                i + 1
            } else {
                i.saturating_sub(1)
            }
        } else if i > 0 {
            i - 1
        } else {
            1
        };
        s.get(j).copied().map_or(0, i64::from)
    };
    // F-5: even-coordinate samples.
    let mut i = p;
    while i < n {
        let sum = at(s, i, false) + at(s, i, true) + 2;
        if let Some(v) = s.get_mut(i) {
            *v = i32::try_from(i64::from(*v) - (sum >> 2)).map_err(|_| OVERFLOW)?;
        }
        i += 2;
    }
    // F-6: odd-coordinate samples, from the updated even ones.
    let mut i = 1 - p;
    while i < n {
        let sum = at(s, i, false) + at(s, i, true);
        if let Some(v) = s.get_mut(i) {
            *v = i32::try_from(i64::from(*v) + (sum >> 1)).map_err(|_| OVERFLOW)?;
        }
        i += 2;
    }
    Ok(())
}

/// Runs the inverse transform over `coef` (row stride `stride`), given the rectangles of
/// resolutions 0 to NL. Resolution NL ends up as the tile-component. Refuses a coefficient set
/// whose lifting overflows 32 bits.
pub fn inverse(coef: &mut [i32], stride: usize, res: &[Rect]) -> Result<(), J2kError> {
    let mut line: Vec<i32> = Vec::new();
    let mut tmp: Vec<i32> = Vec::new();
    for r in 1..res.len() {
        let (Some(cur), Some(low)) = (res.get(r), res.get(r - 1)) else {
            return Ok(());
        };
        let w = (cur.x1 - cur.x0) as usize;
        let h = (cur.y1 - cur.y0) as usize;
        let nlx = (low.x1 - low.x0) as usize;
        let nly = (low.y1 - low.y0) as usize;
        let px = (cur.x0 & 1) as usize;
        let py = (cur.y0 & 1) as usize;
        line.clear();
        line.resize(w.max(h), 0);
        tmp.clear();
        tmp.resize(w.max(h), 0);
        // HOR_SR over every row.
        for y in 0..h {
            let base = y * stride;
            let Some(row) = coef.get_mut(base..base + w) else {
                return Ok(());
            };
            // Interleave: position x is low-pass when its global coordinate is even, and the
            // band index is x / 2 either way; the high band is stored after the nlx low ones.
            for x in 0..w {
                let k = if (x + px) & 1 == 0 {
                    x / 2
                } else {
                    nlx + x / 2
                };
                if let (Some(l), Some(&c)) = (line.get_mut(x), row.get(k)) {
                    *l = c;
                }
            }
            if let Some(l) = line.get_mut(..w) {
                lift_line(l, px)?;
                row.copy_from_slice(l);
            }
        }
        // VER_SR over every column.
        for x in 0..w {
            for y in 0..h {
                let low_y = (y + py) & 1 == 0;
                let k = if low_y { y / 2 } else { nly + y / 2 };
                if let (Some(t), Some(&c)) = (tmp.get_mut(y), coef.get(k * stride + x)) {
                    *t = c;
                }
            }
            if let Some(t) = tmp.get_mut(..h) {
                lift_line(t, py)?;
                for (y, &v) in t.iter().enumerate() {
                    if let Some(c) = coef.get_mut(y * stride + x) {
                        *c = v;
                    }
                }
            }
        }
    }
    Ok(())
}
