//! atan, atan2, asin, acos.
//!
//! All four reduce to atan of a ratio of two double-doubles: values above 1 use
//! atan t = pi/2 - atan(1/t), then atan t = atan c + atan((t - c)/(1 + t c)) with c = j/16 from a
//! table, so the remaining argument is below 1/32 and a short series finishes it.

use crate::consts::{
    ATAN_C, ATAN_TAB, INV3_DD, PIO2_DD, PIO2_HI, PIO4_HI, PI_DD, PI_HI, THREE_PIO4_HI,
};
use crate::dd::{self, dd, two_prod, two_sum, Dd, SPLITTER};
use crate::fast::ratio;
use crate::util::{abs, exponent, is_neg, mul_pow2, negate_if, round_test, with_sign_of, NAN};

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

/// The double-double path of [`atan`] (also the test hook for the fast path).
pub(crate) fn atan_slow(x: f64) -> f64 {
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

/// The double-double path of [`atan2`] (also the test hook for the fast path).
pub(crate) fn atan2_slow(y: f64, x: f64) -> f64 {
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

/// The double-double path of [`asin`] (also the test hook for the fast path).
pub(crate) fn asin_slow(x: f64) -> f64 {
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

/// The double-double path of [`acos`] (also the test hook for the fast path).
pub(crate) fn acos_slow(x: f64) -> f64 {
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

// ---- fast paths -------------------------------------------------------------------------

/// Relative error bound used for the fast atan family (derivation at `atan_unit_fast`).
const ATAN_FAST_REL: f64 = 1.734_723_475_976_807e-18; // 2^-59

/// atan t for a double-double 0 < t <= 1 (slightly above 1 is fine) as `(h, l)`. With
/// c = j/16 the nearest table point, atan t = atan c + atan u, u = (t - c) / (1 + t c) as a
/// double-double quotient (|u| < 2^-5; `t - c` exact, `t c` split into two exact products,
/// the quotient error below 2^-100), and atan u = u + u^3 (-1/3 + u^2 Q(u^2)) with Q the
/// series 1/5 - u^2/7 + ... down to u^13 / 13. Errors (u = 2^-53): the series cut-off
/// u^15 / 15 < 2^-70 (relative to the result at least 2^-5 for j > 0, to u for j = 0); the
/// roundings of `u^3 (...)`, a quantity of size at most 2^-16.6, four of them and two more in
/// the sum of the low parts: below 2^-67; the neglected `u_lo u^2` below 2^-68 and the cube
/// taken of the high part of u alone. With the result at least atan(1/32) = 2^-5 these are
/// below 2^-61.5 relative, and for j = 0 (result about u) the relative error is
/// 5 u * u^2 / 3 < 2^-61 as well; the bound used is 2^-59.
fn atan_unit_fast(th: f64, tl: f64) -> (f64, f64) {
    let j = (th * 16.0 + 0.5) as usize;
    let c = j as f64 * 0.0625;
    let base = ATAN_TAB.get(j).copied().unwrap_or(dd(0.0, 0.0));
    let (uh, ul) = if j == 0 {
        (th, tl)
    } else {
        let nh = th - c;
        let sp = SPLITTER * th;
        let thh = sp - (sp - th);
        let thl = th - thh;
        // 1 + t c = d.hi + dl up to 2^-105: thh c and thl c are exact (26 + 5 bits), the sums
        // of 1, thh c and thl c are two exact `two_sum`s, only `tl c` and the low parts round
        let d1 = two_sum(1.0, thh * c);
        let d = two_sum(d1.hi, thl * c);
        let dl = d.lo + (d1.lo + tl * c);
        let q1 = nh / d.hi;
        let p = two_prod(q1, d.hi);
        let rem = ((nh - p.hi) - p.lo) + (tl - q1 * dl);
        (q1, rem / d.hi)
    };
    let u2 = uh * uh;
    let [k5, k7, k9, k11, k13] = ATAN_C;
    let q = k5 + u2 * (k7 + u2 * (k9 + u2 * (k11 + u2 * k13)));
    let t = (u2 * uh) * (u2 * q - INV3_DD.hi) - (u2 * uh) * INV3_DD.lo;
    let s = two_sum(base.hi, uh);
    let f = two_sum(s.hi, (s.lo + base.lo) + (ul + t));
    (f.hi, f.lo)
}

/// atan((nh + nl) / (dh + dl)) for positive doubles-doubles as `(h, l)`, same error as
/// [`atan_unit_fast`] (the ratio is a double-double with error below 2^-100; above 1 the
/// identity atan t = pi/2 - atan(1/t) subtracts a result below pi/4 from pi/2, so the absolute
/// error stays and the result is at least pi/4).
fn atan_ratio_fast(nh: f64, nl: f64, dh: f64, dl: f64) -> (f64, f64) {
    if nh > dh {
        let t = ratio(dh, dl, nh, nl);
        let (h, l) = atan_unit_fast(t.hi, t.lo);
        let s = two_sum(PIO2_DD.hi, -h);
        let f = two_sum(s.hi, (s.lo + PIO2_DD.lo) - l);
        (f.hi, f.lo)
    } else {
        let t = ratio(nh, nl, dh, dl);
        atan_unit_fast(t.hi, t.lo)
    }
}

/// `pi - (h + l)` for a result below pi/2 (so the difference is at least pi/2: no cancellation).
fn pi_minus(h: f64, l: f64) -> (f64, f64) {
    let s = two_sum(PI_DD.hi, -h);
    let f = two_sum(s.hi, (s.lo + PI_DD.lo) - l);
    (f.hi, f.lo)
}

fn finish(hl: (f64, f64)) -> Option<f64> {
    round_test(hl.0, hl.1, abs(hl.0) * ATAN_FAST_REL)
}

pub(crate) fn atan_fast(x: f64) -> Option<f64> {
    let ax = abs(x);
    if !(TINY..=1.152_921_504_606_846_9e18).contains(&ax) {
        return None;
    }
    let r = finish(atan_ratio_fast(ax, 0.0, 1.0, 0.0))?;
    Some(with_sign_of(r, x))
}

pub(crate) fn atan2_fast(y: f64, x: f64) -> Option<f64> {
    if !(x.is_finite() && y.is_finite()) || x == 0.0 || y == 0.0 {
        return None;
    }
    let ay = abs(y);
    let ax = abs(x);
    let q = ay / ax;
    if !(8.673_617_379_884_035e-19..=1.152_921_504_606_846_9e18).contains(&q) {
        return None;
    }
    let big = if ay > ax { ay } else { ax };
    let s = -exponent(big);
    let (h, l) = atan_ratio_fast(mul_pow2(ay, s), 0.0, mul_pow2(ax, s), 0.0);
    let (h, l) = if is_neg(x) { pi_minus(h, l) } else { (h, l) };
    Some(with_sign_of(finish((h, l))?, y))
}

/// sqrt((1 - a)(1 + a)) as a double-double for 0 <= a < 1: the product of the two exact sums
/// (`p.lo q.lo` below 2^-108 dropped), then a square root corrected once with the exact square
/// of its first approximation; relative error below 2^-100.
fn cos_of_asin_fast(a: f64) -> Dd {
    let p = two_sum(1.0, -a);
    let q = two_sum(1.0, a);
    let w = two_prod(p.hi, q.hi);
    let wl = w.lo + (p.hi * q.lo + p.lo * q.hi);
    let wh = w.hi + wl;
    let wl = wl - (wh - w.hi);
    let ch = wh.sqrt();
    let e = two_prod(ch, ch);
    let rem = ((wh - e.hi) - e.lo) + wl;
    dd(ch, rem / (2.0 * ch))
}

pub(crate) fn asin_fast(x: f64) -> Option<f64> {
    let ax = abs(x);
    if !(TINY..1.0).contains(&ax) {
        return None;
    }
    let c = cos_of_asin_fast(ax);
    let r = finish(atan_ratio_fast(ax, 0.0, c.hi, c.lo))?;
    Some(negate_if(r, is_neg(x)))
}

pub(crate) fn acos_fast(x: f64) -> Option<f64> {
    let ax = abs(x);
    if !(8.673_617_379_884_035e-19..1.0).contains(&ax) {
        return None;
    }
    let c = cos_of_asin_fast(ax);
    let (h, l) = atan_ratio_fast(c.hi, c.lo, ax, 0.0);
    let (h, l) = if is_neg(x) { pi_minus(h, l) } else { (h, l) };
    finish((h, l))
}

/// Arc tangent of `x`, in radians, range [-pi/2, pi/2]. Domain: all doubles. Faithfully rounded
/// (measured maximum error in `crates/oa-math/tests/reference.rs`). atan(+-0) = +-0,
/// atan(+-inf) = +-pi/2, NaN gives NaN.
pub fn atan(x: f64) -> f64 {
    match atan_fast(x) {
        Some(r) => r,
        None => atan_slow(x),
    }
}

/// Four-quadrant arc tangent of `y / x`, in radians, range [-pi, pi]. Domain: all doubles.
/// Faithfully rounded. Special values follow C99 Annex F: atan2(+-0, +0) = +-0,
/// atan2(+-0, -0) = +-pi, atan2(+-0, x<0) = +-pi, atan2(+-0, x>0) = +-0,
/// atan2(y, +-0) = +-pi/2 for y of either sign, atan2(+-y, -inf) = +-pi for finite y > 0,
/// atan2(+-y, +inf) = +-0 for finite y > 0, atan2(+-inf, x) = +-pi/2 for finite x,
/// atan2(+-inf, -inf) = +-3pi/4, atan2(+-inf, +inf) = +-pi/4; NaN in either argument gives NaN.
/// Ratios beyond 2^+-60 use the limiting forms (pi/2, y/x, pi) that round to the same double.
pub fn atan2(y: f64, x: f64) -> f64 {
    match atan2_fast(y, x) {
        Some(r) => r,
        None => atan2_slow(y, x),
    }
}

/// Arc sine of `x`, in radians, range [-pi/2, pi/2]. Domain: [-1, 1]; outside it, and for NaN,
/// the result is NaN. Faithfully rounded. asin(+-0) = +-0, asin(+-1) = +-pi/2.
pub fn asin(x: f64) -> f64 {
    match asin_fast(x) {
        Some(r) => r,
        None => asin_slow(x),
    }
}

/// Arc cosine of `x`, in radians, range [0, pi]. Domain: [-1, 1]; outside it, and for NaN, the
/// result is NaN. Faithfully rounded. acos(1) = +0, acos(-1) = pi, acos(+-0) = pi/2.
pub fn acos(x: f64) -> f64 {
    match acos_fast(x) {
        Some(r) => r,
        None => acos_slow(x),
    }
}
