//! exp: reduction by multiples of ln 2 / 64, a table of 2^(j/64) and a short Taylor tail.

use crate::consts::{EXP_C, EXP_TAB, INV_LN2_64, LN2_64_1, LN2_64_2, LN2_64_3};
use crate::dd::{self, dd, Dd};
use crate::util::{scale_round, NAN};

/// exp(z) for a double-double `z` with |z| below about 800, as `(d, k)` with exp(z) = d * 2^k
/// and `d` normalised, relative error about 2^-75.
pub(crate) fn exp_dd(z: Dd) -> (Dd, i32) {
    let v = z.hi * INV_LN2_64;
    let m = if v >= 0.0 {
        (v + 0.5) as i64
    } else {
        (v - 0.5) as i64
    };
    let mf = m as f64;
    // r = z - m * (ln 2 / 64); the first two products are exact (m < 2^17, pieces of 35 bits).
    let r = dd::add_d(z, -(mf * LN2_64_1));
    let r = dd::add_d(r, -(mf * LN2_64_2));
    let r = dd::sub(r, dd::two_prod(mf, LN2_64_3));
    let j = (m & 63) as usize;
    let k = (m >> 6) as i32;
    let t = EXP_TAB.get(j).copied().unwrap_or(dd(1.0, 0.0));
    // exp(r) - 1 = r + r^2/2 + r^3 * poly(r), |r| <= ln 2 / 128.
    let r2 = dd::mul(r, r);
    let half = dd(r2.hi * 0.5, r2.lo * 0.5);
    let rh = r.hi;
    let mut p = 0.0;
    for c in EXP_C.iter().rev() {
        p = p * rh + c;
    }
    let tail = rh * rh * rh * p;
    let q = dd::add_d(dd::add(r, half), tail);
    (dd::add(t, dd::mul(t, q)), k)
}

/// e raised to `x`. Domain: all doubles. Result is faithfully rounded (measured maximum error
/// in `crates/oa-math/tests/reference.rs`); results below 2^-1022 are correctly rounded
/// subnormals. NaN gives NaN, +inf gives +inf, -inf gives +0, +-0 gives 1; arguments above
/// 709.782712893384 overflow to +inf and below -745.1332191019411 underflow to +0.
pub fn exp(x: f64) -> f64 {
    if x.is_nan() {
        return NAN;
    }
    if x >= 800.0 {
        return f64::INFINITY;
    }
    if x <= -800.0 {
        return 0.0;
    }
    let (d, k) = exp_dd(dd(x, 0.0));
    scale_round(d, k)
}
