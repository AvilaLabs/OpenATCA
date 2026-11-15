//! atan, atan2, asin, acos.
//!
//! All four reduce to atan of a ratio of two double-doubles: values above 1 use
//! atan t = pi/2 - atan(1/t), then atan t = atan c + atan((t - c)/(1 + t c)) with c = j/16 from a
//! table, so the remaining argument is below 1/32 and a short series finishes it.

use crate::consts::{
    ATAN_C, ATAN_TAB, INV3_DD, PIO2_DD, PIO2_HI, PIO4_HI, PI_DD, PI_HI, THREE_PIO4_HI,
};
use crate::dd::{self, dd, fast_two_sum, two_prod, two_sqr, two_sum, Dd};
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

/// Relative error bound used for the fast atan family (derivation at `atan_frac_fast`).
const ATAN_FAST_REL: f64 = 1.734_723_475_976_807e-18; // 2^-59

/// 1.5 * 2^52: adding it to a double of size below 2^51 rounds that double to an integer (ties
/// to even) and leaves the integer in the low bits of the sum.
const ROUND_MAGIC: f64 = 6_755_399_441_055_744.0;

/// The starting value of the sum for the four cases of `atan_frac_fast`, 17 entries each:
/// atan c, pi/2 - atan c (ratio inverted), pi - atan c (negative x), pi/2 + atan c (both),
/// as double-doubles (error below 2^-104 relative).
const fn base_tab() -> [Dd; 68] {
    let mut t = [dd(0.0, 0.0); 68];
    let mut j = 0;
    while j < 17 {
        #[allow(clippy::indexing_slicing)]
        {
            let a = ATAN_TAB[j];
            t[j] = a;
            t[17 + j] = dd::sub(PIO2_DD, a);
            t[34 + j] = dd::sub(PI_DD, a);
            t[51 + j] = dd::add(PIO2_DD, a);
        }
        j += 1;
    }
    t
}
const BASE_TAB: [Dd; 68] = base_tab();

/// `(hi, lo)` with `hi + lo == c * x` exactly and `hi = fl(c * x)`, for `c` a multiple of 1/16 in
/// [0, 1] (at most 5 significant bits): `x` is cut into 26 and at most 27 bits, so `c xh` and
/// `c xl` are exact, `c xh - hi` is exact (the two agree to 2^-25), and their sum is the exact
/// rounding error, a double.
#[inline]
fn two_prod_c(c: f64, x: f64) -> (f64, f64) {
    let p = c * x;
    let xh = f64::from_bits(x.to_bits() & !0x7ff_ffff);
    (p, (c * xh - p) + c * (x - xh))
}

/// `sel(m, a, b)` is `b` when the mask `m` is all ones and `a` when it is 0.
#[inline]
fn sel(m: u64, a: f64, b: f64) -> f64 {
    f64::from_bits((a.to_bits() & !m) | (b.to_bits() & m))
}

/// The sum of the four-case table value and atan of the small remainder, for
/// `(nh + nl) / (dh + dl)` (positive, `nl` and `dl` at most a few ulps of `nh` and `dh`): with
/// n the smaller and d the larger of the two (`swapped` when they were exchanged),
/// t = n / d <= 1 (up to 2^-52 above), c = j/16 the table point nearest t and
/// u = (n - c d) / (d + c n) = tan(atan t - atan c), |u| < 2^-5 (t is only an approximation of n/d
/// used to pick j, so |u| stays below 2^-5 (1 + 2^-50)), and
///
/// * atan(n / d) = atan c + atan u,
/// * pi/2 - atan(n / d) = (pi/2 - atan c) - atan u (swapped),
/// * pi - atan(n / d) = (pi - atan c) - atan u (`xneg`, the arc cosine and atan2 of a negative x),
/// * pi/2 + atan(n / d) = (pi/2 + atan c) + atan u (both),
///
/// the sign of u flipped by the sign bit of the numerator. Returns `(h, l)` with
/// `l` below 2^-11 of `h`, not renormalised. The numerator is formed exactly: `c d` as an exact
/// product and sum, `n - c dh` exact (the two agree to 2^-4 or better, Sterbenz), the pieces
/// summed with a full `two_sum`; the denominator is a sum without cancellation (relative error
/// below 2^-104). The quotient `u` is `uh = num / den.hi` from a reciprocal (error 2 u), the exact
/// remainder `rem = num - uh den` and `ul = rem * recip` (relative error below 2^-103, which
/// with the rounding of `rem` and `num` is 2^-100 of the numerator's size, below 2^-95 absolute
/// as c <= 1). Series: atan u = u + u^3 (-1/3 + u^2 Q(u^2)) with Q the series 1/5 - u^2/7 + ...
/// down to u^13 / 13, in the order of Estrin (the rounding of Q is below 2^-70 of u^5: not
/// material). Errors (u = 2^-53): the series cut-off u^15 / 15 < 2^-70 (relative to the result
/// at least 2^-5 for j > 0, to u for j = 0); the roundings of `u^3 (...)`, a quantity of size
/// at most 2^-16.6, four of them and two more in the sum of the low parts: below 2^-67; the
/// neglected `u_lo u^2` below 2^-68 and the cube taken of the high part of u alone. With the
/// result at least atan(1/32) = 2^-5 these are below 2^-61.5 relative, and for j = 0 (result
/// about u) the relative error is 5 u * u^2 / 3 < 2^-61 as well; the unnormalised low part adds a
/// rounding of 2^-64 of `h` in `round_test`; the bound used is 2^-59. The first sum is a
/// `fast_two_sum`: the table value is 0 (then it is the first term) or at least 0.0624 > |u|.
#[inline]
fn atan_frac_fast(nh: f64, nl: f64, dh: f64, dl: f64, xneg: u64) -> (f64, f64) {
    let swapped = nh > dh;
    let m = 0u64.wrapping_sub(swapped as u64);
    let (an, al) = (sel(m, nh, dh), sel(m, nl, dl));
    let (bn, bl) = (sel(m, dh, nh), sel(m, dl, nl));
    // an + al is the smaller, bn + bl the larger
    let tm = (an / bn) * 16.0 + ROUND_MAGIC;
    let j = tm.to_bits().wrapping_sub(ROUND_MAGIC.to_bits()) as usize;
    let c = (tm - ROUND_MAGIC) * 0.0625;
    let (ph, pl) = two_prod_c(c, bn);
    let (qh, ql) = two_prod_c(c, an);
    let nsum = two_sum(an - ph, -pl);
    let num = two_sum(nsum.hi, nsum.lo + (al - c * bl));
    let ds = fast_two_sum(bn, qh);
    let den = fast_two_sum(ds.hi, ds.lo + (ql + (bl + c * al)));
    let flip = ((swapped as u64) ^ xneg) << 63;
    let (numh, numl) = (
        f64::from_bits(num.hi.to_bits() ^ flip),
        f64::from_bits(num.lo.to_bits() ^ flip),
    );
    let recip = 1.0 / den.hi;
    let uh = numh * recip;
    let p = two_prod(uh, den.hi);
    let rem = ((numh - p.hi) - p.lo) + (numl - uh * den.lo);
    let ul = rem * recip;
    let u2 = uh * uh;
    let u4 = u2 * u2;
    let [k5, k7, k9, k11, k13] = ATAN_C;
    let q = ((k5 + u2 * k7) + u4 * (k9 + u2 * k11)) + (u4 * u4) * k13;
    let u3 = u2 * uh;
    let t = u3 * (u2 * q - INV3_DD.hi) - u3 * INV3_DD.lo;
    let kind = (swapped as usize) | ((xneg as usize) << 1);
    let base = BASE_TAB.get(kind * 17 + j).copied().unwrap_or(dd(0.0, 0.0));
    let s = fast_two_sum(base.hi, uh);
    (s.hi, (s.lo + base.lo) + (ul + t))
}

#[inline]
fn finish(hl: (f64, f64)) -> Option<f64> {
    round_test(hl.0, hl.1, abs(hl.0) * ATAN_FAST_REL)
}

pub(crate) fn atan_fast(x: f64) -> Option<f64> {
    let ax = abs(x);
    if !(TINY..=1.152_921_504_606_846_9e18).contains(&ax) {
        return None;
    }
    let r = finish(atan_frac_fast(ax, 0.0, 1.0, 0.0, 0))?;
    Some(with_sign_of(r, x))
}

/// Operand range of the unscaled atan2 fast path (the larger magnitude).
const ATAN2_LO: f64 = 1.0e-100;
const ATAN2_HI: f64 = 1.0e100;

pub(crate) fn atan2_fast(y: f64, x: f64) -> Option<f64> {
    let (ay, ax) = (abs(y), abs(x));
    let big = if ay > ax { ay } else { ax };
    let neg_x = x.to_bits() >> 63;
    if (ATAN2_LO..ATAN2_HI).contains(&big) {
        // a ratio beyond 2^-60 is for the slow path (limiting forms); the smaller is 0 when
        // either operand is
        let small = if ay > ax { ax } else { ay };
        if small < big * 8.673_617_379_884_035e-19 {
            return None;
        }
        let r = finish(atan_frac_fast(ay, 0.0, ax, 0.0, neg_x))?;
        return Some(with_sign_of(r, y));
    }
    if !(x.is_finite() && y.is_finite()) || x == 0.0 || y == 0.0 {
        return None;
    }
    let q = ay / ax;
    if !(8.673_617_379_884_035e-19..=1.152_921_504_606_846_9e18).contains(&q) {
        return None;
    }
    let s = -exponent(big);
    let r = finish(atan_frac_fast(
        mul_pow2(ay, s),
        0.0,
        mul_pow2(ax, s),
        0.0,
        neg_x,
    ))?;
    Some(with_sign_of(r, y))
}

/// sqrt(1 - a^2) as a double-double for 0 <= a < 1: `1 - a^2` from the exact square
/// (`ph + pl`): `1 - ph` as a `fast_two_sum` (ph < 1) and the low part `pl` subtracted, so the
/// double-double `(wh, wl)` is `1 - a^2` up to one rounding of the low part (below 2^-107
/// relative); `wh >= 2^-53 > |pl|` as a < 1 makes the last `fast_two_sum` exact. Then a square
/// root corrected once with the exact square of its first approximation; relative error below
/// 2^-100. The correction divides by 2 ch as a product with the reciprocal (computed beside the
/// square, relative error below 2^-103 of a correction below 2^-52 ch).
#[inline]
fn cos_of_asin_fast(a: f64) -> Dd {
    let sq = two_sqr(a);
    let d = fast_two_sum(1.0, -sq.hi);
    let w = fast_two_sum(d.hi, d.lo - sq.lo);
    let ch = w.hi.sqrt();
    let half_inv = 0.5 / ch;
    let e = two_sqr(ch);
    let rem = ((w.hi - e.hi) - e.lo) + w.lo;
    dd(ch, rem * half_inv)
}

pub(crate) fn asin_fast(x: f64) -> Option<f64> {
    let ax = abs(x);
    if !(TINY..1.0).contains(&ax) {
        return None;
    }
    let c = cos_of_asin_fast(ax);
    let r = finish(atan_frac_fast(ax, 0.0, c.hi, c.lo, 0))?;
    Some(negate_if(r, is_neg(x)))
}

pub(crate) fn acos_fast(x: f64) -> Option<f64> {
    let ax = abs(x);
    if !(8.673_617_379_884_035e-19..1.0).contains(&ax) {
        return None;
    }
    let c = cos_of_asin_fast(ax);
    finish(atan_frac_fast(c.hi, c.lo, ax, 0.0, x.to_bits() >> 63))
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
