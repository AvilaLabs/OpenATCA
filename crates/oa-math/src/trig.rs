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
use crate::dd::{self, dd, fast_two_sum, two_sum, Dd};
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

/// 1.5 * 2^52: adding it to a double of size below 2^51 rounds that double to an integer (ties
/// to even) and leaves the integer in the low bits of the sum.
const ROUND_MAGIC: f64 = 6_755_399_441_055_744.0;

/// The integer held in the low bits of `t = v + ROUND_MAGIC` (as a wrapped difference of bit
/// patterns, so a negative integer is read as a two's-complement `u64`).
fn magic_int(t: f64) -> u64 {
    t.to_bits().wrapping_sub(ROUND_MAGIC.to_bits())
}

const SIGN_BIT: u64 = 0x8000_0000_0000_0000;

/// Cody-Waite reduction in doubles: `Some((n, yh, yl))` with ax = n pi/2 + yh + yl, |yl| <= 2^-54
/// |yh|, for 2^-27 <= ax < 2^19 and, when n is not 0, |yh| >= 2^-16. n is ax 2/pi rounded to an
/// integer by the magic-number sum (ties to even); n is 0 below about pi/4, where `yh` is ax and
/// `yl` is 0, and |yh| is at most pi/4 + 2^-50 in any case. Error: n pi/2 is subtracted in
/// pieces of which the first two (33 bits, n < 2^19) give exact products. The first difference
/// `ax - n PIO2_1` is exact: both terms are multiples of ulp(ax) (at most 2^-34 below 2^19, the
/// product a multiple of 2^-32), and the result is below 1 in size while ulp(ax) >= 2^-53 for
/// ax >= 1/2 (n = 0 gives ax itself), so it has fewer than 2^53 units. The second is a `two_sum`
/// (the result may be small enough to cancel); the third product is rounded (error below
/// 2^-103), and the final sum is a `fast_two_sum`: when it matters, |yh| >= 2^-16 exceeds the
/// remaining terms (below 2^-62), and otherwise the result is refused below. What is dropped is
/// n times the remainder of pi/2 after three pieces, 8.5e-32 n < 2^-84.9 for n < 2^18.4. So
/// `yh + yl` is within 2^-84 of the true remainder, below 2^-68 relative for |y| >= 2^-16.
#[inline]
fn reduce_fast(ax: f64) -> Option<(u64, f64, f64)> {
    let t = ax * TWO_OVER_PI + ROUND_MAGIC;
    let n = magic_int(t);
    let nf = t - ROUND_MAGIC;
    let s1 = ax - nf * PIO2_1;
    let s2 = two_sum(s1, -(nf * PIO2_2));
    let y = fast_two_sum(s2.hi, s2.lo - nf * PIO2_3);
    if n != 0 && abs(y.hi) < MIN_Y {
        return None;
    }
    Some((n & 3, y.hi, y.lo))
}

/// sin and cos of j/16 as `[sin, cos]` for each j, and the same with the roles swapped: the
/// value an offset starts from and the value that multiplies the offset, for sin |y| (index 0)
/// and cos |y| (index 1).
const fn pair_tab(first: &[Dd; 15], second: &[Dd; 15]) -> [Dd; 30] {
    let mut t = [dd(0.0, 0.0); 30];
    let mut j = 0;
    while j < 15 {
        #[allow(clippy::indexing_slicing)]
        {
            t[2 * j] = first[j];
            t[2 * j + 1] = second[j];
        }
        j += 1;
    }
    t
}
pub(crate) const START_TAB: [Dd; 30] = pair_tab(&SIN_TAB, &COS_TAB);
pub(crate) const SLOPE_TAB: [Dd; 30] = pair_tab(&COS_TAB, &SIN_TAB);

/// The table index and the short series for sin and cos of `|y| = a + b`, a = j/16, |b| <= 1/32.
struct Part {
    j: usize,
    b: f64,
    /// sin b - b (with the low part of the argument): bl + b^3 S(b^2)
    sinb_l: f64,
    /// cos(b + bl) - 1
    c2: f64,
}

/// The series for the reduced argument `ay = ayh + ayl >= 0`, `ayh <= pi/4 + 2^-50`. j is
/// ayh 16 rounded to an integer (ties to even); the series are evaluated in the order of
/// Estrin, which changes their roundings by far less than the bound allows (the terms past the
/// leading ones are below 2^-24).
#[inline]
fn part(ayh: f64, ayl: f64) -> Part {
    let t = ayh * 16.0 + ROUND_MAGIC;
    let j = magic_int(t) as usize;
    let a = (t - ROUND_MAGIC) * 0.0625;
    // exact: for j = 0 it is ayh; otherwise a is within a factor 2 of ayh (Sterbenz)
    let b = ayh - a;
    let b2 = b * b;
    let b4 = b2 * b2;
    let [s5, s7, s9, s11] = SIN_C;
    let [c4, c6, c8, c10, _] = COS_C;
    let sp = -INV6_DD.hi + b2 * ((s5 + b2 * s7) + b4 * (s9 + b2 * s11));
    let sinb_l = ayl + (b * b2) * sp;
    let cp = (c4 + b2 * c6) + b4 * (c8 + b2 * c10);
    // cos(b + bl) - 1 = c2 - bl sin b, so the low part of the argument enters here as well
    let c2 = (-0.5 * b2 + b4 * cp) - ayl * b;
    Part { j, b, sinb_l, c2 }
}

/// `x` with the low 27 bits of its mantissa cleared.
#[inline]
fn cut27(x: f64) -> f64 {
    f64::from_bits(x.to_bits() & !0x7ff_ffff)
}

/// Exact product up to 2^-78 relative: `hi + lo == a * b`, `hi = fl(a * b)`, the operands split
/// by clearing the low 27 mantissa bits (pieces of 26 and at most 27 bits, so all but the last
/// partial product are exact; the last is rounded, below 2^-107 relative, and the sums of
/// partial products round at most by 2^-79 relative).
#[inline]
pub(crate) fn two_prod_cut(a: f64, b: f64) -> Dd {
    let p = a * b;
    let ah = cut27(a);
    let bh = cut27(b);
    let (al, bl) = (a - ah, b - bh);
    let err = (((ah * bh - p) + ah * bl) + al * bh) + al * bl;
    dd(p, err)
}

/// sin |y| (`k` = 0) or cos |y| (`k` = 1) as `(h, l)`. With T the value the offset starts from
/// (sin a or cos a) and V the one that multiplies it (cos a or sin a), and the sign `s` of the
/// slope (+1 for sin, -1 for cos): sin(a + b) = sa + sa c2 + ca (b + sinb_l) and
/// cos(a + b) = ca + ca c2 - sa (b + sinb_l) are both `T + T c2 + V s (b + sinb_l)`. Error
/// (relative to the result, which is at least 2^-5 for a nonzero table index and is b itself
/// otherwise, u = 2^-53): the series cut-offs (b^13 / 13! and b^12 / 12!, below 2^-80); the
/// roundings in `sinb_l` and `c2` (about 3 u of terms of size b^3 / 6 and b^2 / 2), whose
/// products with the table values are below 2^-64 T; the roundings of the sum of the small
/// terms, whose size is at most 2^-11.5 T + 2^-17.6, giving below 2^-61 of the result; the
/// neglected products of the low table parts with the small terms (2^-65 relative) and of the
/// low part of the argument with c2 (below 2^-63.4 relative). Total below 2^-60.3, the bound
/// used is 2^-59 (the same for sin and cos: the terms differ only in sign). The sums are
/// `fast_two_sum`s: |T.hi| >= |V.hi b| (T >= 0.0624 or 0 where b is the whole result, V.hi |b|
/// <= 0.78 / 32 < 0.0624). The pair returned is not renormalised: `l = s.lo + lw` is below
/// 2^-10 of `h`, so `round_test` rounds `l + err` with an error below 2^-63 of `h`, which the
/// factor 2.5 between the total and the bound used (2^-59) covers.
#[inline]
fn eval(p: &Part, k: usize) -> (f64, f64) {
    let t = START_TAB.get(2 * p.j + k).copied().unwrap_or(dd(0.0, 0.0));
    let v = SLOPE_TAB.get(2 * p.j + k).copied().unwrap_or(dd(0.0, 0.0));
    let flip = (k as u64) << 63;
    let bs = f64::from_bits(p.b.to_bits() ^ flip);
    let ls = f64::from_bits(p.sinb_l.to_bits() ^ flip);
    let q = two_prod_cut(v.hi, bs);
    let s = fast_two_sum(t.hi, q.hi);
    let lw = t.lo + (q.lo + (v.lo * bs + (v.hi * ls + t.hi * p.c2)));
    (s.hi, s.lo + lw)
}

/// The quadrant, the sign bit of y and the part of the reduced |y|.
#[inline]
fn reduced(x: f64) -> Option<(u64, u64, Part)> {
    let ax = abs(x);
    if !(TINY..CW_LIMIT).contains(&ax) {
        return None;
    }
    let (n, yh, yl) = reduce_fast(ax)?;
    let neg_y = yh.to_bits() & SIGN_BIT;
    let (ayh, ayl) = (abs(yh), f64::from_bits(yl.to_bits() ^ neg_y));
    Some((n, neg_y >> 63, part(ayh, ayl)))
}

/// Rounds a fast `(h, l)` (positive) with the relative bound `rel`, negated when `negative` is 1.
#[inline]
fn finish(hl: (f64, f64), rel: f64, negative: u64) -> Option<f64> {
    let r = round_test(hl.0, hl.1, abs(hl.0) * rel)?;
    Some(f64::from_bits(r.to_bits() ^ (negative << 63)))
}

/// Whether sin(x) is negative (0 or 1), given the quadrant n, the sign bit of y and of x:
/// the quadrants 0 to 3 give y, never, not y, always.
#[inline]
pub(crate) fn sin_sign(n: u64, neg_y: u64, neg_x: u64) -> u64 {
    (((n >> 1) & 1) ^ (neg_y & !n & 1)) ^ neg_x
}

/// Whether cos(x) is negative (0 or 1): cos is the sine of the next quadrant.
#[inline]
pub(crate) fn cos_sign(n: u64, neg_y: u64) -> u64 {
    ((n >> 1) ^ n ^ (neg_y & n)) & 1
}

pub(crate) fn sin_fast(x: f64) -> Option<f64> {
    let (n, neg_y, p) = reduced(x)?;
    finish(
        eval(&p, (n & 1) as usize),
        SIN_COS_FAST_REL,
        sin_sign(n, neg_y, x.to_bits() >> 63),
    )
}

pub(crate) fn cos_fast(x: f64) -> Option<f64> {
    let (n, neg_y, p) = reduced(x)?;
    finish(
        eval(&p, ((n & 1) ^ 1) as usize),
        SIN_COS_FAST_REL,
        cos_sign(n, neg_y),
    )
}

pub(crate) fn sin_cos_fast(x: f64) -> Option<(f64, f64)> {
    let (n, neg_y, p) = reduced(x)?;
    let k = (n & 1) as usize;
    let (sx, cx) = (eval(&p, k), eval(&p, k ^ 1));
    Some((
        finish(sx, SIN_COS_FAST_REL, sin_sign(n, neg_y, x.to_bits() >> 63))?,
        finish(cx, SIN_COS_FAST_REL, cos_sign(n, neg_y))?,
    ))
}

/// tan |y| = s / c (or c / s for an odd quadrant) as a double-double quotient: the relative
/// errors of the two series add (2 * 2^-60.3), the quotient's own error is below 2^-100; the
/// bound used is 2^-58.
pub(crate) fn tan_fast(x: f64) -> Option<f64> {
    let (n, neg_y, p) = reduced(x)?;
    let norm = |hl: (f64, f64)| {
        let f = fast_two_sum(hl.0, hl.1);
        (f.hi, f.lo)
    };
    let (s, c) = (norm(eval(&p, 0)), norm(eval(&p, 1)));
    let negative;
    let q = if n & 1 == 0 {
        negative = neg_y;
        ratio(s.0, s.1, c.0, c.1)
    } else {
        negative = neg_y ^ 1;
        ratio(c.0, c.1, s.0, s.1)
    };
    finish((q.hi, q.lo), TAN_FAST_REL, negative ^ (x.to_bits() >> 63))
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

#[cfg(kani)]
mod proofs {
    //! Kani proof harnesses (docs/VERIFICATION.md). Compiled only under `cargo kani`.
    use super::*;

    // REQ: OA-MATH-030
    #[kani::proof]
    fn quadrant_signs_follow_the_quadrant_table() {
        let n: u64 = kani::any();
        let neg_y: u64 = kani::any();
        let neg_x: u64 = kani::any();
        kani::assume(n < 4 && neg_y < 2 && neg_x < 2);
        // The quadrants 0 to 3: sin is negative for y, never, not y, always; cos is the sine of
        // the next quadrant.
        let sin_neg = match n {
            0 => neg_y == 1,
            1 => false,
            2 => neg_y == 0,
            _ => true,
        };
        let cos_neg = match n {
            0 => false,
            1 => neg_y == 0,
            2 => true,
            _ => neg_y == 1,
        };
        assert_eq!(sin_sign(n, neg_y, neg_x), u64::from(sin_neg != (neg_x == 1)));
        assert_eq!(cos_sign(n, neg_y), u64::from(cos_neg));
    }

    /// `v` rounded to an integer, ties to even, by integer arithmetic on the truncation.
    fn round_half_even(v: f64) -> i64 {
        let t = v as i64;
        let frac = v - t as f64;
        if frac > 0.5 {
            t + 1
        } else if frac < -0.5 {
            t - 1
        } else if frac == 0.5 {
            t + (t & 1)
        } else if frac == -0.5 {
            t - (t & 1)
        } else {
            t
        }
    }

    // REQ: OA-MATH-031
    #[kani::proof]
    fn magic_number_rounds_half_even() {
        let v: f64 = kani::any();
        kani::assume(!v.is_nan() && v > -2_251_799_813_685_248.0 && v < 2_251_799_813_685_248.0);
        let t = v + ROUND_MAGIC;
        let want = round_half_even(v);
        // The integer in the low bits, as `reduce_fast` and the other fast paths read it.
        assert_eq!(magic_int(t) as i64, want);
        // And the float they subtract back out.
        assert_eq!(t - ROUND_MAGIC, want as f64);
    }

    // REQ: OA-MATH-032
    #[kani::proof]
    fn cut_split_halves_are_exact_and_short() {
        let a: f64 = kani::any();
        let bits = a.to_bits();
        // Normal numbers whose low part (units of 2^(e-52), at least 2^-1022) is normal too.
        let e = (bits >> 52) & 0x7ff;
        kani::assume(e >= 53 && e < 0x7ff);
        let m = (bits & 0x000f_ffff_ffff_ffff) | (1 << 52);
        let ah = cut27(a);
        let al = a - ah;
        // The unit of the mantissa's last bit, an exact power of two.
        let unit = f64::from_bits((e - 52) << 52);
        let neg = if bits >> 63 == 1 { -1.0 } else { 1.0 };
        let hi_m = m >> 27;
        let lo_m = m & 0x7ff_ffff;
        // The halves are the high and the low bits of the mantissa, each exact in a double.
        assert_eq!(ah, neg * (hi_m << 27) as f64 * unit);
        assert_eq!(al, neg * lo_m as f64 * unit);
        // Pieces of 26 bits (the high one) and 27 bits (the low one) at most.
        assert!(hi_m < 1 << 26);
        assert!(lo_m < 1 << 27);
        // They sum to the input.
        assert_eq!((hi_m << 27) + lo_m, m);
    }
}
