//! sin, cos, sin_cos, tan.
//!
//! The argument is reduced to y = x - n * pi/2 with |y| <= pi/4 as a double-double. Below 2^19
//! the reduction is Cody-Waite (pi/2 in four pieces, n times a piece exact), measured at a
//! relative error of the reduced argument below 2^-87 on the 21,553 arguments below 2^19 closest
//! to a multiple of pi/2 (the closest has |y| near 2^-60.5; mpmath, `tests/reference.rs` keeps the
//! 600 hardest); above it is Payne-Hanek with 1408 stored bits of 2/pi. sin y and cos y then
//! come from a table at j/16 and a short series in the remainder.

use crate::consts::{
    COS_C, COS_TAB, INV6_DD, PIO2_1, PIO2_2, PIO2_3, PIO2_4, PIO2_DD, PIO4_HI, SIN_C, SIN_TAB,
    TWO_OVER_PI, TWO_OVER_PI_BITS,
};
use crate::dd::{self, dd, two_prod, two_sum, Dd};
use crate::fast::ratio;
use crate::util::{abs, is_neg, negate_if, pow2, round_test, NAN};

/// Below this |x|, sin x rounds to x, cos x to 1 and tan x to x.
const TINY: f64 = 7.450_580_596_923_828e-9; // 2^-27

/// Cody-Waite is used below this magnitude; larger arguments take Payne-Hanek.
const CW_LIMIT: f64 = 524_288.0; // 2^19

/// 64 bits of the 2/pi table starting at bit `pos` (bit 0 is the most significant of word 0).
fn pi_bits_at(pos: u32) -> u64 {
    let w = (pos / 64) as usize;
    let s = pos % 64;
    let a = TWO_OVER_PI_BITS.get(w).copied().unwrap_or(0);
    if s == 0 {
        return a;
    }
    let b = TWO_OVER_PI_BITS.get(w + 1).copied().unwrap_or(0);
    (a << s) | (b >> (64 - s))
}

/// `n` bits (n <= 64) of the little-endian limb number `p` starting at bit `lo`.
fn bits_of(p: &[u64; 5], lo: u32, n: u32) -> u64 {
    let w = (lo / 64) as usize;
    let s = lo % 64;
    let a = p.get(w).copied().unwrap_or(0);
    let v = if s == 0 {
        a
    } else {
        let b = p.get(w + 1).copied().unwrap_or(0);
        (a >> s) | (b << (64 - s))
    };
    if n >= 64 {
        v
    } else {
        v & ((1u64 << n) - 1)
    }
}

/// Shifts the 192-bit number `(f0, f1, f2)` (f0 most significant) left by `lz` < 192 bits.
fn shl192(f: [u64; 3], lz: u32) -> [u64; 3] {
    let src = [f[0], f[1], f[2], 0, 0];
    let w = (lz / 64) as usize;
    let s = lz % 64;
    let mut out = [0u64; 3];
    for (i, o) in out.iter_mut().enumerate() {
        let a = src.get(i + w).copied().unwrap_or(0);
        *o = if s == 0 {
            a
        } else {
            let b = src.get(i + w + 1).copied().unwrap_or(0);
            (a << s) | (b >> (64 - s))
        };
    }
    out
}

/// Payne-Hanek reduction of a finite |x| >= 2^19: returns `(n mod 4, y)` with x = (n + f) pi/2,
/// y = f pi/2 and |f| <= 1/2, as a double-double.
fn reduce_large(ax: f64) -> (u32, Dd) {
    let bits = ax.to_bits();
    let be = ((bits >> 52) & 0x7ff) as i32;
    let mant = (bits & 0x000f_ffff_ffff_ffff) | (1u64 << 52);
    let e = be - 1075; // x = mant * 2^e
    let i0 = if e - 1 < 1 { 1 } else { e - 1 }; // first bit of 2/pi that matters (weight 2^-i0)
    let p0 = (i0 - 1) as u32;
    // 256-bit window of 2/pi, little-endian limbs, multiplied by the 53-bit mantissa.
    let mut prod = [0u64; 5];
    let mut carry: u128 = 0;
    for (k, pk) in prod.iter_mut().take(4).enumerate() {
        let q = pi_bits_at(p0 + 64 * (3 - k as u32));
        let t = u128::from(mant) * u128::from(q) + carry;
        *pk = t as u64;
        carry = t >> 64;
    }
    if let Some(top) = prod.get_mut(4) {
        *top = carry as u64;
    }
    // x * 2/pi = prod / 2^sh
    let sh = (i0 + 255 - e) as u32;
    let mut n = bits_of(&prod, sh, 2);
    let mut f = [
        bits_of(&prod, sh - 64, 64),
        bits_of(&prod, sh - 128, 64),
        bits_of(&prod, sh - 192, 64),
    ];
    let mut negative = false;
    if f[0] >> 63 != 0 {
        // f >= 1/2: use the negative remainder f - 1 and the next multiple.
        let (a2, c2) = (!f[2]).overflowing_add(1);
        let (a1, c1) = (!f[1]).overflowing_add(u64::from(c2));
        let a0 = (!f[0]).wrapping_add(u64::from(c1));
        f = [a0, a1, a2];
        n += 1;
        negative = true;
    }
    let quad = (n & 3) as u32;
    let lz = if f[0] != 0 {
        f[0].leading_zeros()
    } else if f[1] != 0 {
        64 + f[1].leading_zeros()
    } else if f[2] != 0 {
        128 + f[2].leading_zeros()
    } else {
        return (quad, dd(0.0, 0.0));
    };
    let g = shl192(f, lz);
    let m22 = (1u64 << 22) - 1;
    let m42 = (1u64 << 42) - 1;
    let c0 = g[0] >> 22;
    let c1 = ((g[0] & m22) << 20) | (g[1] >> 44);
    let c2 = (g[1] >> 2) & m42;
    let a = c0 as f64 * pow2(-42);
    let b = c1 as f64 * pow2(-84);
    let c = c2 as f64 * pow2(-126);
    let s = dd::two_sum(a, b);
    let s = dd::add_d(s, c);
    let sc = pow2(-(lz as i32));
    let frac = dd(s.hi * sc, s.lo * sc);
    let y = dd::mul(frac, PIO2_DD);
    (quad, if negative { dd::neg(y) } else { y })
}

/// Reduces a finite, non-negative `ax` to `(n mod 4, y)` with ax = n pi/2 + y, |y| <= pi/4 + tiny.
fn reduce(ax: f64) -> (u32, Dd) {
    if ax < PIO4_HI {
        return (0, dd(ax, 0.0));
    }
    if ax >= CW_LIMIT {
        return reduce_large(ax);
    }
    let n = (ax * TWO_OVER_PI + 0.5) as i64;
    let nf = n as f64;
    let r = dd::two_sum(ax, -(nf * PIO2_1));
    let r = dd::add_d(r, -(nf * PIO2_2));
    let r = dd::add_d(r, -(nf * PIO2_3));
    let r = dd::sub(r, dd::two_prod(nf, PIO2_4));
    ((n & 3) as u32, r)
}

/// sin y and cos y as double-doubles for |y| <= about 0.8.
fn sincos_dd(y: Dd) -> (Dd, Dd) {
    let neg = is_neg(y.hi);
    let ay = if neg { dd::neg(y) } else { y };
    let j = (ay.hi * 16.0 + 0.5) as usize;
    let a = j as f64 * 0.0625;
    let sa = SIN_TAB.get(j).copied().unwrap_or(dd(0.0, 0.0));
    let ca = COS_TAB.get(j).copied().unwrap_or(dd(1.0, 0.0));
    let b = dd::add_d(ay, -a);
    let bh = b.hi;
    let b2 = dd::mul(b, b);
    let b3 = dd::mul(b2, b);
    let bh2 = bh * bh;
    // sin b = b - b^3/6 + b^5 * poly(b^2)
    let mut ps = 0.0;
    for c in SIN_C.iter().rev() {
        ps = ps * bh2 + c;
    }
    let sb = dd::add(b, dd::neg(dd::mul(b3, INV6_DD)));
    let sb = dd::add_d(sb, bh2 * bh2 * bh * ps);
    // cos b - 1 = -b^2/2 + b^4 * poly(b^2)
    let mut pc = 0.0;
    for c in COS_C.iter().rev() {
        pc = pc * bh2 + c;
    }
    let cb1 = dd::add_d(dd(-0.5 * b2.hi, -0.5 * b2.lo), bh2 * bh2 * pc);
    // sin(a + b) = sin a cos b + cos a sin b ; cos(a + b) = cos a cos b - sin a sin b
    let s = dd::add(dd::add(sa, dd::mul(sa, cb1)), dd::mul(ca, sb));
    let c = dd::sub(dd::add(ca, dd::mul(ca, cb1)), dd::mul(sa, sb));
    (if neg { dd::neg(s) } else { s }, c)
}

/// sin and cos of a finite `x` as double-doubles, with the quadrant folded in, or `None` for
/// |x| below the tiny threshold.
fn sin_cos_dd(x: f64) -> Option<(Dd, Dd)> {
    let ax = abs(x);
    if ax < TINY {
        return None;
    }
    let (n, y) = reduce(ax);
    let (s, c) = sincos_dd(y);
    let (s, c) = match n & 3 {
        0 => (s, c),
        1 => (c, dd::neg(s)),
        2 => (dd::neg(s), dd::neg(c)),
        _ => (dd::neg(c), s),
    };
    Some((if is_neg(x) { dd::neg(s) } else { s }, c))
}

/// The double-double path of [`sin_cos`] (also the test hook for the fast path).
pub(crate) fn sin_cos_slow(x: f64) -> (f64, f64) {
    if !x.is_finite() {
        return (NAN, NAN);
    }
    match sin_cos_dd(x) {
        Some((s, c)) => (s.hi, c.hi),
        None => (x, 1.0),
    }
}

/// The double-double path of [`sin`] (also the test hook for the fast path).
pub(crate) fn sin_slow(x: f64) -> f64 {
    sin_cos_slow(x).0
}

/// The double-double path of [`cos`] (also the test hook for the fast path).
pub(crate) fn cos_slow(x: f64) -> f64 {
    sin_cos_slow(x).1
}

/// The double-double path of [`tan`] (also the test hook for the fast path).
pub(crate) fn tan_slow(x: f64) -> f64 {
    if !x.is_finite() {
        return NAN;
    }
    let ax = abs(x);
    if ax < TINY {
        return x;
    }
    let (n, y) = reduce(ax);
    let (s, c) = sincos_dd(y);
    let t = if n & 1 == 0 {
        dd::div(s, c)
    } else {
        dd::neg(dd::div(c, s))
    };
    negate_if(t.hi, is_neg(x))
}

// ---- fast paths -------------------------------------------------------------------------
//
// Each fast path computes in plain doubles (error-free transformations only where the error
// matters), proves an absolute error bound, and returns the rounded result only when
// `round_test` says the rounding is certain; everything else (|x| < 2^-27, |x| >= 2^19, an
// argument within 2^-16 of a multiple of pi/2, a doubtful rounding) is left to the
// double-double path.

/// Below this reduced magnitude (after a nonzero quadrant) the fast path gives up: the
/// reduction error of about 2^-84 absolute would no longer be below 2^-68 relative.
const MIN_Y: f64 = 1.525_878_906_25e-5; // 2^-16

/// Relative error bound of the fast sin and cos (see `sin_of` and `cos_of`) and of the fast tan.
const SIN_COS_FAST_REL: f64 = 1.734_723_475_976_807e-18; // 2^-59
const TAN_FAST_REL: f64 = 3.469_446_951_953_614e-18; // 2^-58

/// Cody-Waite reduction in doubles: `Some((n, yh, yl))` with ax = n pi/2 + yh + yl, |yl| <= 2^-54
/// |yh|, for 2^-27 <= ax < 2^19 and, when n is not 0, |yh| >= 2^-16. Error: n pi/2 is
/// subtracted in pieces of which the first two (33 bits, n < 2^19) give exact products and
/// exact `two_sum`s; the third product is rounded (error below 2^-103); what is dropped is
/// n times the remainder of pi/2 after three pieces, 8.5e-32 n < 2^-84.9 for n < 2^18.4.
/// So `yh + yl` is within 2^-84 of the true remainder, below 2^-68 relative for |y| >= 2^-16.
fn reduce_fast(ax: f64) -> Option<(u32, f64, f64)> {
    if ax < PIO4_HI {
        return Some((0, ax, 0.0));
    }
    if ax >= CW_LIMIT {
        return None;
    }
    let n = (ax * TWO_OVER_PI + 0.5) as i64;
    let nf = n as f64;
    let s1 = two_sum(ax, -(nf * PIO2_1));
    let s2 = two_sum(s1.hi, -(nf * PIO2_2));
    let y = two_sum(s2.hi, (s2.lo + s1.lo) - nf * PIO2_3);
    if abs(y.hi) < MIN_Y {
        return None;
    }
    Some(((n & 3) as u32, y.hi, y.lo))
}

/// Table index and the short series for sin and cos of `|y| = a + b`, a = j/16, |b| <= 1/32.
struct Part {
    sa: Dd,
    ca: Dd,
    b: f64,
    /// sin b - b (with the low part of the argument): bl + b^3 S(b^2)
    sinb_l: f64,
    /// cos(b + bl) - 1
    c2: f64,
}

/// The series for the reduced argument `ay = ayh + ayl >= 0`, `ayh <= pi/4 + 2^-50`.
fn part(ayh: f64, ayl: f64) -> Part {
    let j = (ayh * 16.0 + 0.5) as usize;
    let a = j as f64 * 0.0625;
    let sa = SIN_TAB.get(j).copied().unwrap_or(dd(0.0, 0.0));
    let ca = COS_TAB.get(j).copied().unwrap_or(dd(1.0, 0.0));
    // exact: for j = 0 it is ayh; otherwise a is within a factor 2 of ayh (Sterbenz)
    let b = ayh - a;
    let b2 = b * b;
    let [s5, s7, s9, s11] = SIN_C;
    let [c4, c6, c8, c10, _] = COS_C;
    let sp = -INV6_DD.hi + b2 * (s5 + b2 * (s7 + b2 * (s9 + b2 * s11)));
    let sinb_l = ayl + (b * b2) * sp;
    let cp = c4 + b2 * (c6 + b2 * (c8 + b2 * c10));
    // cos(b + bl) - 1 = c2 - bl sin b, so the low part of the argument enters here as well
    let c2 = (-0.5 * b2 + (b2 * b2) * cp) - ayl * b;
    Part {
        sa,
        ca,
        b,
        sinb_l,
        c2,
    }
}

/// sin |y| as `(h, l)`. sin(a + b) = sa + sa c2 + ca (b + sinb_l). Error (relative to the
/// result, which is at least 2^-5 for a nonzero table index and is b itself otherwise, u = 2^-53):
/// the series cut-offs (b^13 / 13! and b^12 / 12!, below 2^-80); the roundings in `sinb_l`
/// and `c2` (about 3 u of terms of size b^3 / 6 and b^2 / 2), whose products with the table
/// values are below 2^-64 sa; the roundings of the sum of the small terms, whose size is at
/// most 2^-11.5 sa + 2^-17.6, giving below 2^-61 of the result; the neglected products of the
/// low table parts with the small terms (2^-65 relative) and of the low part of the argument
/// with c2 (below 2^-63.4 relative). Total below 2^-60.3, the bound used is 2^-59.
fn sin_of(p: &Part) -> (f64, f64) {
    let q = two_prod(p.ca.hi, p.b);
    let s = two_sum(p.sa.hi, q.hi);
    let lw = p.sa.lo + (q.lo + (p.ca.lo * p.b + (p.ca.hi * p.sinb_l + p.sa.hi * p.c2)));
    let f = two_sum(s.hi, s.lo + lw);
    (f.hi, f.lo)
}

/// cos |y| as `(h, l)`; cos(a + b) = ca + ca c2 - sa (b + sinb_l). Same analysis as `sin_of`,
/// with the result at least cos(pi/4) = 0.7 so the relative error is below 2^-61.
fn cos_of(p: &Part) -> (f64, f64) {
    let q = two_prod(p.sa.hi, p.b);
    let s = two_sum(p.ca.hi, -q.hi);
    let lw = p.ca.lo + (p.ca.hi * p.c2 - (q.lo + (p.sa.lo * p.b + p.sa.hi * p.sinb_l)));
    let f = two_sum(s.hi, s.lo + lw);
    (f.hi, f.lo)
}

/// The reduced argument as a `Part` of |y| and whether y is negative.
fn reduced(x: f64) -> Option<(u32, bool, Part)> {
    let ax = abs(x);
    if !(TINY..CW_LIMIT).contains(&ax) {
        return None;
    }
    let (n, yh, yl) = reduce_fast(ax)?;
    let neg_y = is_neg(yh);
    let (ayh, ayl) = if neg_y { (-yh, -yl) } else { (yh, yl) };
    Some((n, neg_y, part(ayh, ayl)))
}

/// Rounds a fast `(h, l)` with the relative bound `rel`, negated when `negative`.
fn finish(hl: (f64, f64), rel: f64, negative: bool) -> Option<f64> {
    let r = round_test(hl.0, hl.1, abs(hl.0) * rel)?;
    Some(negate_if(r, negative))
}

/// Whether sin(x) is negative, given the quadrant, the sign of y and of x.
fn sin_sign(n: u32, neg_y: bool, neg_x: bool) -> bool {
    let neg = match n & 3 {
        0 => neg_y,
        1 => false,
        2 => !neg_y,
        _ => true,
    };
    neg != neg_x
}

/// Whether cos(x) is negative, given the quadrant and the sign of y.
fn cos_sign(n: u32, neg_y: bool) -> bool {
    match n & 3 {
        0 => false,
        1 => !neg_y,
        2 => true,
        _ => neg_y,
    }
}

pub(crate) fn sin_fast(x: f64) -> Option<f64> {
    let (n, neg_y, p) = reduced(x)?;
    let hl = if n & 1 == 0 { sin_of(&p) } else { cos_of(&p) };
    finish(hl, SIN_COS_FAST_REL, sin_sign(n, neg_y, is_neg(x)))
}

pub(crate) fn cos_fast(x: f64) -> Option<f64> {
    let (n, neg_y, p) = reduced(x)?;
    let hl = if n & 1 == 0 { cos_of(&p) } else { sin_of(&p) };
    finish(hl, SIN_COS_FAST_REL, cos_sign(n, neg_y))
}

pub(crate) fn sin_cos_fast(x: f64) -> Option<(f64, f64)> {
    let (n, neg_y, p) = reduced(x)?;
    let (s, c) = (sin_of(&p), cos_of(&p));
    let (sx, cx) = if n & 1 == 0 { (s, c) } else { (c, s) };
    Some((
        finish(sx, SIN_COS_FAST_REL, sin_sign(n, neg_y, is_neg(x)))?,
        finish(cx, SIN_COS_FAST_REL, cos_sign(n, neg_y))?,
    ))
}

/// tan |y| = s / c (or c / s for an odd quadrant) as a double-double quotient: the relative
/// errors of the two series add (2 * 2^-60.3), the quotient's own error is below 2^-100; the
/// bound used is 2^-58.
pub(crate) fn tan_fast(x: f64) -> Option<f64> {
    let (n, neg_y, p) = reduced(x)?;
    let (s, c) = (sin_of(&p), cos_of(&p));
    let negative;
    let q = if n & 1 == 0 {
        negative = neg_y;
        ratio(s.0, s.1, c.0, c.1)
    } else {
        negative = !neg_y;
        ratio(c.0, c.1, s.0, s.1)
    };
    finish((q.hi, q.lo), TAN_FAST_REL, negative != is_neg(x))
}

/// Sine and cosine of `x` together, computed from one argument reduction. Domain: all finite
/// doubles (any magnitude). Each result is faithfully rounded; see [`sin`] and [`cos`] for the
/// special values. The pair equals `(sin(x), cos(x))` bit for bit.
pub fn sin_cos(x: f64) -> (f64, f64) {
    match sin_cos_fast(x) {
        Some(r) => r,
        None => sin_cos_slow(x),
    }
}

/// Sine of `x` (radians). Domain: all finite doubles, however large (Payne-Hanek reduction).
/// Faithfully rounded (measured maximum error in `crates/oa-math/tests/reference.rs`).
/// sin(+-0) = +-0, sin(+-inf) = NaN, NaN gives NaN; subnormal arguments return `x`.
pub fn sin(x: f64) -> f64 {
    match sin_fast(x) {
        Some(r) => r,
        None => sin_slow(x),
    }
}

/// Cosine of `x` (radians). Domain: all finite doubles. Faithfully rounded.
/// cos(+-0) = 1, cos(+-inf) = NaN, NaN gives NaN.
pub fn cos(x: f64) -> f64 {
    match cos_fast(x) {
        Some(r) => r,
        None => cos_slow(x),
    }
}

/// Tangent of `x` (radians). Domain: all finite doubles. Faithfully rounded.
/// tan(+-0) = +-0, tan(+-inf) = NaN, NaN gives NaN. Near odd multiples of pi/2 the result is
/// large but finite (at most about 1.6e16).
pub fn tan(x: f64) -> f64 {
    match tan_fast(x) {
        Some(r) => r,
        None => tan_slow(x),
    }
}
