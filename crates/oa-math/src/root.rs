//! hypot and cbrt.

use crate::dd::{self, dd, Dd};
use crate::util::{abs, exponent, frexp1, mul_pow2, pow2, scale_round, with_sign_of, NAN};

/// Euclidean length sqrt(x^2 + y^2). Domain: all doubles. Faithfully rounded (measured maximum
/// error in `crates/oa-math/tests/reference.rs`); no intermediate overflow or underflow, the
/// result overflows only when it does not fit. hypot(+-inf, y) = +inf for every y including NaN
/// (C99 Annex F); otherwise a NaN operand gives NaN; hypot(+-0, +-0) = +0.
pub fn hypot(x: f64, y: f64) -> f64 {
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

/// Cube root of `x`. Domain: all doubles. Faithfully rounded (measured maximum error in
/// `crates/oa-math/tests/reference.rs`). cbrt(+-0) = +-0, cbrt(+-inf) = +-inf, NaN gives NaN;
/// negative arguments give the negative real root.
pub fn cbrt(x: f64) -> f64 {
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
