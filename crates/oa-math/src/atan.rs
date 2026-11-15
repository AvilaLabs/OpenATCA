//! atan, atan2, asin, acos.
//!
//! All four reduce to atan of a ratio of two double-doubles: values above 1 use
//! atan t = pi/2 - atan(1/t), then atan t = atan c + atan((t - c)/(1 + t c)) with c = j/16 from a
//! table, so the remaining argument is below 1/32 and a short series finishes it.

use crate::consts::{
    ATAN_C, ATAN_TAB, INV3_DD, PIO2_DD, PIO2_HI, PIO4_HI, PI_DD, PI_HI, THREE_PIO4_HI,
};
use crate::dd::{self, dd, Dd};
use crate::util::{abs, exponent, is_neg, mul_pow2, negate_if, with_sign_of, NAN};

/// Below this |x|, atan x and asin x round to x.
const TINY: f64 = 7.450_580_596_923_828e-9; // 2^-27

/// atan t for a double-double 0 <= t <= 1 (slightly above 1 is fine).
fn atan_unit(t: Dd) -> Dd {
    let j = (t.hi * 16.0 + 0.5) as usize;
    let c = j as f64 * 0.0625;
    let base = ATAN_TAB.get(j).copied().unwrap_or(dd(0.0, 0.0));
    let u = if j == 0 {
        t
    } else {
        let num = dd::add_d(t, -c);
        let den = dd::add_d(dd::mul_d(t, c), 1.0);
        dd::div(num, den)
    };
    // atan u = u - u^3/3 + u^5 * poly(u^2)
    let uh = u.hi;
    let uh2 = uh * uh;
    let mut p = 0.0;
    for k in ATAN_C.iter().rev() {
        p = p * uh2 + k;
    }
    let u3 = dd::mul(dd::mul(u, u), u);
    let s = dd::add(u, dd::neg(dd::mul(u3, INV3_DD)));
    let s = dd::add_d(s, uh2 * uh2 * uh * p);
    dd::add(base, s)
}

/// atan(num / den) for double-doubles num >= 0, den >= 0, not both zero.
fn atan_ratio(num: Dd, den: Dd) -> Dd {
    if num.hi > den.hi {
        dd::sub(PIO2_DD, atan_unit(dd::div(den, num)))
    } else {
        atan_unit(dd::div(num, den))
    }
}

/// Arc tangent of `x`, in radians, range [-pi/2, pi/2]. Domain: all doubles. Faithfully rounded
/// (measured maximum error in `crates/oa-math/tests/reference.rs`). atan(+-0) = +-0,
/// atan(+-inf) = +-pi/2, NaN gives NaN.
pub fn atan(x: f64) -> f64 {
    if x.is_nan() {
        return NAN;
    }
    let ax = abs(x);
    if ax < TINY {
        return x;
    }
    if ax > 1.152_921_504_606_846_9e18 {
        // 2^60: pi/2 - 1/x rounds to the double nearest pi/2
        return with_sign_of(PIO2_HI, x);
    }
    let r = atan_ratio(dd(ax, 0.0), dd(1.0, 0.0));
    with_sign_of(r.hi, x)
}

/// Four-quadrant arc tangent of `y / x`, in radians, range [-pi, pi]. Domain: all doubles.
/// Faithfully rounded. Special values follow C99 Annex F: atan2(+-0, +0) = +-0,
/// atan2(+-0, -0) = +-pi, atan2(+-0, x<0) = +-pi, atan2(+-0, x>0) = +-0,
/// atan2(y, +-0) = +-pi/2 for y of either sign, atan2(+-y, -inf) = +-pi for finite y > 0,
/// atan2(+-y, +inf) = +-0 for finite y > 0, atan2(+-inf, x) = +-pi/2 for finite x,
/// atan2(+-inf, -inf) = +-3pi/4, atan2(+-inf, +inf) = +-pi/4; NaN in either argument gives NaN.
/// Ratios beyond 2^+-60 use the limiting forms (pi/2, y/x, pi) that round to the same double.
pub fn atan2(y: f64, x: f64) -> f64 {
    if x.is_nan() || y.is_nan() {
        return NAN;
    }
    let neg_x = is_neg(x);
    if y == 0.0 {
        let r = if neg_x { PI_HI } else { 0.0 };
        return with_sign_of(r, y);
    }
    if x == 0.0 {
        return with_sign_of(PIO2_HI, y);
    }
    if y.is_infinite() {
        let r = if x.is_infinite() {
            if neg_x {
                THREE_PIO4_HI
            } else {
                PIO4_HI
            }
        } else {
            PIO2_HI
        };
        return with_sign_of(r, y);
    }
    if x.is_infinite() {
        return with_sign_of(if neg_x { PI_HI } else { 0.0 }, y);
    }
    let ay = abs(y);
    let ax = abs(x);
    let q = ay / ax;
    if q > 1.152_921_504_606_846_9e18 {
        return with_sign_of(PIO2_HI, y);
    }
    if q < 8.673_617_379_884_035e-19 {
        // 2^-60: the angle is y/x (x > 0) or pi (x < 0) to within half an ulp.
        return if neg_x { with_sign_of(PI_HI, y) } else { y / x };
    }
    let big = if ay > ax { ay } else { ax };
    let s = -exponent(big);
    let a = dd(mul_pow2(ay, s), 0.0);
    let b = dd(mul_pow2(ax, s), 0.0);
    let r = atan_ratio(a, b);
    let r = if neg_x { dd::sub(PI_DD, r) } else { r };
    with_sign_of(r.hi, y)
}

/// sqrt((1 - a)(1 + a)) as a double-double for 0 <= a < 1.
fn cos_of_asin(a: f64) -> Dd {
    let p = dd::two_sum(1.0, -a);
    let q = dd::two_sum(1.0, a);
    dd::sqrt(dd::mul(p, q))
}

/// Arc sine of `x`, in radians, range [-pi/2, pi/2]. Domain: [-1, 1]; outside it, and for NaN,
/// the result is NaN. Faithfully rounded. asin(+-0) = +-0, asin(+-1) = +-pi/2.
pub fn asin(x: f64) -> f64 {
    if x.is_nan() {
        return NAN;
    }
    let ax = abs(x);
    if ax > 1.0 {
        return NAN;
    }
    if ax < TINY {
        return x;
    }
    if ax == 1.0 {
        return with_sign_of(PIO2_HI, x);
    }
    let c = cos_of_asin(ax);
    let r = atan_ratio(dd(ax, 0.0), c);
    negate_if(r.hi, is_neg(x))
}

/// Arc cosine of `x`, in radians, range [0, pi]. Domain: [-1, 1]; outside it, and for NaN, the
/// result is NaN. Faithfully rounded. acos(1) = +0, acos(-1) = pi, acos(+-0) = pi/2.
pub fn acos(x: f64) -> f64 {
    if x.is_nan() {
        return NAN;
    }
    let ax = abs(x);
    if ax > 1.0 {
        return NAN;
    }
    if ax < 8.673_617_379_884_035e-19 {
        // 2^-60: pi/2 -+ x rounds to the double nearest pi/2
        return PIO2_HI;
    }
    if ax == 1.0 {
        return if is_neg(x) { PI_HI } else { 0.0 };
    }
    let c = cos_of_asin(ax);
    let r = atan_ratio(c, dd(ax, 0.0));
    if is_neg(x) {
        dd::sub(PI_DD, r).hi
    } else {
        r.hi
    }
}
