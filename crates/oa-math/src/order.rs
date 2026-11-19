//! Order, sign and extremes of floats with every NaN and every signed zero defined. The standard
//! library's `total_cmp`, `is_sign_negative`, `copysign`, `min`, `max` and `clamp` either read the
//! sign and payload of a NaN, which differ between machines, or leave open which zero they
//! return; core code calls these instead (RULESET B8). Nothing here calls the platform maths
//! library.

use crate::nan::{canon_nan, canon_nan_f32};
use core::cmp::Ordering;

const SIGN64: u64 = 0x8000_0000_0000_0000;
const SIGN32: u32 = 0x8000_0000;

/// The IEEE-754 total order of `a` and `b` after both went through [`canon_nan`]: `-NaN` does not
/// exist, so the order is `-inf < negatives < -0.0 < +0.0 < positives < +inf < NaN`, and every NaN
/// equals every other.
#[inline]
pub fn cmp_canon_f64(a: f64, b: f64) -> Ordering {
    canon_nan(a).total_cmp(&canon_nan(b))
}

/// [`cmp_canon_f64`] for `f32`.
#[inline]
pub fn cmp_canon_f32(a: f32, b: f32) -> Ordering {
    canon_nan_f32(a).total_cmp(&canon_nan_f32(b))
}

/// Whether the sign bit of `x` is set, after [`canon_nan`]: `true` for `-0.0` and the negatives,
/// `false` for every NaN (the canonical NaN is positive).
#[inline]
pub fn is_sign_negative_canon_f64(x: f64) -> bool {
    canon_nan(x).is_sign_negative()
}

/// [`is_sign_negative_canon_f64`] for `f32`.
#[inline]
pub fn is_sign_negative_canon_f32(x: f32) -> bool {
    canon_nan_f32(x).is_sign_negative()
}

/// `x` with the sign of `sign` (after [`canon_nan`], so a NaN `sign` is positive). A NaN `x`
/// gives the canonical NaN.
#[inline]
pub fn copysign_canon_f64(x: f64, sign: f64) -> f64 {
    let bits = (x.to_bits() & !SIGN64) | (canon_nan(sign).to_bits() & SIGN64);
    canon_nan(f64::from_bits(bits))
}

/// [`copysign_canon_f64`] for `f32`.
#[inline]
pub fn copysign_canon_f32(x: f32, sign: f32) -> f32 {
    let bits = (x.to_bits() & !SIGN32) | (canon_nan_f32(sign).to_bits() & SIGN32);
    canon_nan_f32(f32::from_bits(bits))
}

/// The smaller of `a` and `b`. A NaN is ignored (the other is returned; two NaNs give the
/// canonical NaN), and between `-0.0` and `0.0` the result is `-0.0`, whichever order they come in.
#[inline]
pub fn min_f64(a: f64, b: f64) -> f64 {
    if a.is_nan() {
        return canon_nan(b);
    }
    if b.is_nan() {
        return a;
    }
    if a < b {
        a
    } else if b < a {
        b
    } else if a.is_sign_negative() {
        // equal: only the zeros differ
        a
    } else {
        b
    }
}

/// [`min_f64`] for `f32`.
#[inline]
pub fn min_f32(a: f32, b: f32) -> f32 {
    if a.is_nan() {
        return canon_nan_f32(b);
    }
    if b.is_nan() {
        return a;
    }
    if a < b {
        a
    } else if b < a {
        b
    } else if a.is_sign_negative() {
        a
    } else {
        b
    }
}

/// The larger of `a` and `b`. A NaN is ignored (the other is returned; two NaNs give the
/// canonical NaN), and between `-0.0` and `0.0` the result is `0.0`, whichever order they come in.
#[inline]
pub fn max_f64(a: f64, b: f64) -> f64 {
    if a.is_nan() {
        return canon_nan(b);
    }
    if b.is_nan() {
        return a;
    }
    // equal: only the zeros differ, and 0.0 is the larger
    if a > b {
        a
    } else if b > a || a.is_sign_negative() {
        b
    } else {
        a
    }
}

/// [`max_f64`] for `f32`.
#[inline]
pub fn max_f32(a: f32, b: f32) -> f32 {
    if a.is_nan() {
        return canon_nan_f32(b);
    }
    if b.is_nan() {
        return a;
    }
    // equal: only the zeros differ, and 0.0 is the larger
    if a > b {
        a
    } else if b > a || a.is_sign_negative() {
        b
    } else {
        a
    }
}

/// `x` limited to `lo..=hi`: `max_f64(x, lo)` then `min_f64(.., hi)`, so the zeros follow those
/// two functions. A NaN `x` gives the canonical NaN (as the standard library's `clamp` gives a
/// NaN); a NaN bound is ignored; `lo > hi` does not panic, the result is `hi`.
#[inline]
pub fn clamp_f64(x: f64, lo: f64, hi: f64) -> f64 {
    if x.is_nan() {
        return canon_nan(x);
    }
    min_f64(max_f64(x, lo), hi)
}

/// [`clamp_f64`] for `f32`.
#[inline]
pub fn clamp_f32(x: f32, lo: f32, hi: f32) -> f32 {
    if x.is_nan() {
        return canon_nan_f32(x);
    }
    min_f32(max_f32(x, lo), hi)
}
