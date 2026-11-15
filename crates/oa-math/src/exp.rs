//! exp: reduction by multiples of ln 2 / 64, a table of 2^(j/64) and a short Taylor tail.

use crate::consts::{EXP_C, EXP_TAB, INV_LN2_64, LN2_64_1, LN2_64_2, LN2_64_3};
use crate::dd::{self, dd, Dd};
use crate::dd::{fast_two_sum, two_sum};
use crate::util::{abs, round_test, scale_round, NAN};

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

/// Relative-to-the-mantissa error bound of [`exp_fast_core`] (the result `d` of a call is in
/// [0.99, 2.02] and the error is absolute in that scale). Derivation, with u = 2^-53 and the
/// reduced argument |r| <= ln 2 / 128 < 2^-7.5 (the argument |z| is below 700, so |m| < 2^16):
/// * the reduction `r = z - m L1 - m L2 - m L3` is exact except for the dropped rest of
///   ln 2 / 64 after L3 (below 2^-114 per unit of m) and the rounding of `m L3` and of the low
///   part (below 2^-100): negligible;
/// * the neglected cross term `rl * (exp(rh) - 1)` is below 2^-61.5 * 0.0055 < 2^-69;
/// * the Taylor series is cut after r^7: the next term is below r^8 / 40320 < 2^-75;
/// * evaluating r^2 (0.5 + r p) in doubles: four roundings of quantities below 2^-15.1, each at
///   most 2^-53 of its size, together below 2^-66, times the table value below 2: 2^-65;
/// * the product `th * qh` (|qh| < 0.0056, th < 2) is rounded once, by at most half an ulp of a
///   number below 2^-6: 2^-60, the dominant term;
/// * the table entry is exact to 2^-106, the remaining additions are below 2^-100.
///
/// The sum is below 2^-59.8; the constant is 2^-58, a factor of 3.4 above it.
pub(crate) const EXP_FAST_ERR: f64 = 3.469_446_951_953_614e-18; // 2^-58

/// exp(zh + zl) as `(h, l, k)` with exp(zh + zl) = (h + l) * 2^k to within [`EXP_FAST_ERR`]
/// absolute, for |zh| < 700 and |zl| < 2^-40. `l` is below a few ulps of `h`.
pub(crate) fn exp_fast_core(zh: f64, zl: f64) -> (f64, f64, i32) {
    let v = zh * INV_LN2_64;
    let m = if v >= 0.0 {
        (v + 0.5) as i64
    } else {
        (v - 0.5) as i64
    };
    let mf = m as f64;
    // m L1 and m L2 are exact products (m < 2^17, pieces of 35 bits), the two sums exact.
    let s1 = two_sum(zh, -(mf * LN2_64_1));
    let s2 = two_sum(s1.hi, -(mf * LN2_64_2));
    let rl = (s2.lo + s1.lo) + (zl - mf * LN2_64_3);
    let r = two_sum(s2.hi, rl);
    let (rh, rl) = (r.hi, r.lo);
    let [k3, k4, k5, k6, k7, _] = EXP_C;
    let p = k3 + rh * (k4 + rh * (k5 + rh * (k6 + rh * k7)));
    let q = two_sum(rh, rl + (rh * rh) * (0.5 + rh * p));
    let j = (m & 63) as usize;
    let k = (m >> 6) as i32;
    let t = EXP_TAB.get(j).copied().unwrap_or(dd(1.0, 0.0));
    // t (1 + q) = t.hi + t.hi q.hi + (t.lo (1 + q.hi) + t.hi q.lo)
    let w = t.hi * q.hi;
    let s = fast_two_sum(t.hi, w);
    (s.hi, s.lo + (t.lo * (1.0 + q.hi) + t.hi * q.lo), k)
}

/// 1.5 * 2^52: adding it to a double of size below 2^51 rounds that double to an integer (ties
/// to even) and leaves the integer in the low bits of the sum.
const ROUND_MAGIC: f64 = 6_755_399_441_055_744.0;

/// Absolute error bound of [`exp_fast_plain`] (the result `d` is in [0.99, 2.02]): 2^-59, see there.
const EXP_PLAIN_ERR: f64 = 1.734_723_475_976_807e-18; // 2^-59

/// exp(x) for a plain double `x`, |x| < 700, as `(h, l, k)` like [`exp_fast_core`] and with the
/// error of the same derivation, computed with a shorter dependency chain (the argument has no
/// low part, which lets the reduction skip its normalising sum). Differences from the general
/// core:
/// * m is x / (ln 2 / 64) rounded to an integer by the magic-number sum (ties to even), so
///   |r| <= ln 2 / 128 (1 + 2^-50) as before;
/// * `x - m L1` is exact: m L1 is an exact product of at most 52 bits, a multiple of 2^-41, x
///   is a multiple of ulp(x) <= 2^-43 (|x| < 700), and the difference is below 2^-7 in size, so
///   it has at most 2^-7 / ulp(x) <= 2^53 units, with ulp(x) >= 2^-60 when m is not 0 (then
///   |x| >= ln 2 / 128 > 2^-8) and the difference equals x when m is 0;
/// * the sum with `-m L2` stays a `two_sum` (the exact value of the difference may cancel);
/// * the remainder `rl = s2.lo - m L3` is not renormalised against `rh`: it is below
///   2^-60 + 2^-61 (the low part of a sum, at most half an ulp of `rh` < 2^-7.5, and
///   m L3 < 2^-60), so the neglected cross term `rl (exp(rh) - 1)` is below 2^-60 * 2^-7.5, which
///   with the table value below 2 is 2^-66.5; the same size comes from the difference between
///   (rh + rl)^2 and rh^2 (halved, doubled by the table value);
/// * the tail `p` is evaluated in the order of Estrin (same terms; the roundings of p are
///   below 2^-72 of the result).
///
/// The error is then that of [`exp_fast_core`] plus 2^-66 (the cross terms) and 2^-72: the
/// rounding of the product `t.hi q.hi` (below 2^-60, the dominant term), the evaluation of
/// r^2 (0.5 + r p) (2^-65) and the Taylor cut-off (2^-74) sum to below 2^-59.8. The bound used
/// is 2^-59, a factor 1.7 above that (the largest error seen against the double-double path
/// over 20 million arguments is 1.01 2^-60).
pub(crate) fn exp_fast_plain(x: f64) -> (f64, f64, i32) {
    let t = x * INV_LN2_64 + ROUND_MAGIC;
    let m = t.to_bits().wrapping_sub(ROUND_MAGIC.to_bits()) as i64;
    let mf = t - ROUND_MAGIC;
    let d = x - mf * LN2_64_1;
    let s2 = two_sum(d, -(mf * LN2_64_2));
    let rh = s2.hi;
    let rl = s2.lo - mf * LN2_64_3;
    let [k3, k4, k5, k6, k7, _] = EXP_C;
    let r2 = rh * rh;
    let p = ((k3 + rh * k4) + r2 * (k5 + rh * k6)) + (r2 * r2) * k7;
    let q = two_sum(rh, rl + r2 * (0.5 + rh * p));
    let j = (m & 63) as usize;
    let k = (m >> 6) as i32;
    let t = EXP_TAB.get(j).copied().unwrap_or(dd(1.0, 0.0));
    let w = t.hi * q.hi;
    let s = fast_two_sum(t.hi, w);
    (s.hi, s.lo + (t.lo * (1.0 + q.hi) + t.hi * q.lo), k)
}

/// The fast path of [`exp`]: `None` unless the argument is in (-700, 700) and the rounding of
/// the result is certain (then the result is what [`exp_slow`] returns).
pub(crate) fn exp_fast(x: f64) -> Option<f64> {
    if !(0.0..700.0).contains(&abs(x)) {
        return None;
    }
    let (h, l, k) = exp_fast_plain(x);
    // -1011 <= k <= 1010, so 2^k is a normal number and the scaling is exact
    round_test(h, l, EXP_PLAIN_ERR).map(|a| a * f64::from_bits(((k + 1023) as u64) << 52))
}

/// e raised to `x`. Domain: all doubles. Result is faithfully rounded (measured maximum error
/// in `crates/oa-math/tests/reference.rs`); results below 2^-1022 are correctly rounded
/// subnormals. NaN gives NaN, +inf gives +inf, -inf gives +0, +-0 gives 1; arguments above
/// 709.782712893384 overflow to +inf and below -745.1332191019411 underflow to +0.
pub fn exp(x: f64) -> f64 {
    match exp_fast(x) {
        Some(r) => r,
        None => exp_slow(x),
    }
}

/// The double-double path of [`exp`] (also the test hook for the fast path).
pub(crate) fn exp_slow(x: f64) -> f64 {
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
