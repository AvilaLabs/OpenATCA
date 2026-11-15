//! hypot and cbrt.

use crate::dd::{self, dd, fast_two_sum, two_prod, two_sum, Dd};
use crate::util::{
    abs, exponent, frexp1, mul_pow2, pow2, round_test, scale_round, with_sign_of, NAN,
};

/// The double-double path of [`hypot`] (also the test hook for the fast path).
pub(crate) fn hypot_slow(x: f64, y: f64) -> f64 {
    if x.is_infinite() || y.is_infinite() {
        return f64::INFINITY;
    }
    if x.is_nan() || y.is_nan() {
        return NAN;
    }
    let (ax, ay) = (abs(x), abs(y));
    let (big, small) = if ay > ax { (ay, ax) } else { (ax, ay) };
    if small == 0.0 {
        return big;
    }
    // small/big < 2^-60: sqrt(1 + q^2) rounds to 1
    if small < big * 8.673_617_379_884_035e-19 {
        return big;
    }
    let e = exponent(big);
    let a = mul_pow2(big, -e);
    let b = mul_pow2(small, -e);
    let sum = dd::add(dd::two_prod(a, a), dd::two_prod(b, b));
    scale_round(dd::sqrt(sum), e)
}

/// The double-double path of [`cbrt`] (also the test hook for the fast path).
pub(crate) fn cbrt_slow(x: f64) -> f64 {
    if x.is_nan() {
        return NAN;
    }
    if x == 0.0 || x.is_infinite() {
        return x;
    }
    let ax = abs(x);
    let (e, m) = frexp1(ax);
    // x = m * 2^e = (m * 2^r) * 2^(3q) with r in 0..=2
    let q = if e >= 0 { e / 3 } else { -((2 - e) / 3) };
    let r = e - 3 * q;
    let mm = m * pow2(r);
    // Newton on y^3 = mm from a chord start, then one double-double correction step.
    let mut y = 1.0 + (mm - 1.0) / 7.0;
    for _ in 0..6 {
        y -= (y * y * y - mm) / (3.0 * y * y);
    }
    let y2 = dd::two_prod(y, y);
    let y3 = dd::mul_d(y2, y);
    let rem = dd::sub(dd(mm, 0.0), y3);
    let delta = rem.hi / (3.0 * y2.hi);
    let yd: Dd = dd::fast_two_sum(y, delta);
    with_sign_of(scale_round(yd, q), x)
}

// ---- fast paths -------------------------------------------------------------------------

/// Relative error bound of the fast hypot and cbrt: the computed double-double results carry
/// about 2^-100 (see the derivations below); the constant is 2^-80.
const ROOT_FAST_REL: f64 = 8.271_806_125_530_277e-25;

/// The fast path of [`hypot`]: for finite operands with 2^-60 |big| <= |small|, a nonzero
/// `big` and a normal result, `a^2 + b^2` (a, b scaled by 2^-e into [1, 2) and [2^-60, 2)) as
/// a double-double from two exact products and a sum (rounding of the low parts below
/// 2^-104), a double square root and one correction with the exact square: `r = sqrt(sh)`,
/// `rem = sh + sl - r^2`, `cl = rem / 2r`. The error is the rounding of `cl` and of the low
/// sum (below 2^-103 relative) plus the second-order term `cl^2 / 2r`: below 2^-102.
pub(crate) fn hypot_fast(x: f64, y: f64) -> Option<f64> {
    if !(x.is_finite() && y.is_finite()) {
        return None;
    }
    let (ax, ay) = (abs(x), abs(y));
    let (big, small) = if ay > ax { (ay, ax) } else { (ax, ay) };
    if !(1e-290..1e290).contains(&big) || small <= 0.0 || small < big * 8.673_617_379_884_035e-19 {
        return None;
    }
    let e = exponent(big);
    let a = mul_pow2(big, -e);
    let b = mul_pow2(small, -e);
    let pa = two_prod(a, a);
    let pb = two_prod(b, b);
    let s = two_sum(pa.hi, pb.hi);
    let f = fast_two_sum(s.hi, s.lo + (pa.lo + pb.lo));
    let r = f.hi.sqrt();
    let sq = two_prod(r, r);
    let cl = (((f.hi - sq.hi) - sq.lo) + f.lo) / (2.0 * r);
    round_test(r, cl, r * ROOT_FAST_REL).map(|v| v * pow2(e))
}

/// The fast path of [`cbrt`]: for a normal nonzero finite argument. With the mantissa scaled
/// into mm in [1, 8) by a power of 2^3, three Halley steps from the chord start (error below
/// 11% at the start, cubic convergence: below 2^-50 after two, rounding-limited after three)
/// give y; one correction `delta = (mm - y^3) / (3 y^2)` with y^2 and y^3 from exact products
/// (error below 2^-100 relative to y) brings Newton's quadratic remainder to `delta^2 / y`. The
/// path is taken only when `|delta| <= 2^-44 y` (checked at run time, so the start and the
/// number of steps do not matter for correctness): the remainder is then below 2^-88 y and the
/// error below 2^-87; the bound used is 2^-80.
pub(crate) fn cbrt_fast(x: f64) -> Option<f64> {
    if !(x.is_finite() && abs(x) >= f64::MIN_POSITIVE) {
        return None;
    }
    let ax = abs(x);
    let (e, m) = frexp1(ax);
    let q = if e >= 0 { e / 3 } else { -((2 - e) / 3) };
    let mm = m * pow2(e - 3 * q);
    let mut y = 1.0 + (mm - 1.0) / 7.0;
    for _ in 0..3 {
        let y3 = y * y * y;
        y = y * (y3 + 2.0 * mm) / (2.0 * y3 + mm);
    }
    let y2 = two_prod(y, y);
    let y3 = dd::mul_d(y2, y);
    let rem = (mm - y3.hi) - y3.lo;
    let delta = rem / (3.0 * y2.hi);
    if delta.is_nan() || abs(delta) > y * 5.684_341_886_080_802e-14 {
        return None;
    }
    let f = fast_two_sum(y, delta);
    let r = round_test(f.hi, f.lo, f.hi * ROOT_FAST_REL)? * pow2(q);
    Some(with_sign_of(r, x))
}

/// Euclidean length sqrt(x^2 + y^2). Domain: all doubles. Faithfully rounded (measured maximum
/// error in `crates/oa-math/tests/reference.rs`); no intermediate overflow or underflow, the
/// result overflows only when it does not fit. hypot(+-inf, y) = +inf for every y including NaN
/// (C99 Annex F); otherwise a NaN operand gives NaN; hypot(+-0, +-0) = +0.
pub fn hypot(x: f64, y: f64) -> f64 {
    match hypot_fast(x, y) {
        Some(r) => r,
        None => hypot_slow(x, y),
    }
}

/// Cube root of `x`. Domain: all doubles. Faithfully rounded (measured maximum error in
/// `crates/oa-math/tests/reference.rs`). cbrt(+-0) = +-0, cbrt(+-inf) = +-inf, NaN gives NaN;
/// negative arguments give the negative real root.
pub fn cbrt(x: f64) -> f64 {
    match cbrt_fast(x) {
        Some(r) => r,
        None => cbrt_slow(x),
    }
}
