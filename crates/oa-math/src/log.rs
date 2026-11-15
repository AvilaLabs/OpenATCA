//! ln: split off the binary exponent, reduce the mantissa by a 48-entry table of reciprocals
//! (so the remainder u is below 2^-6 and exact to form), then a log1p series in double-double.

use crate::consts::{INV3_DD, LN2_DD, LN_C, LN_NEG_LN_R, LN_R};
use crate::dd::{self, dd, fast_two_sum, Dd};
use crate::util::{abs, frexp1, round_test, NAN};

/// ln of a positive finite double as a double-double, relative error about 2^-70.
pub(crate) fn ln_dd(x: f64) -> Dd {
    let (mut e, mut m) = frexp1(x);
    if m >= 1.5 {
        m *= 0.5;
        e += 1;
    }
    // m in [0.75, 1.5); bin j = floor(64 m) in 48..=95.
    let j = ((m * 64.0) as usize).wrapping_sub(48);
    let r = LN_R.get(j).copied().unwrap_or(1.0);
    let nl = LN_NEG_LN_R.get(j).copied().unwrap_or(dd(0.0, 0.0));
    let u = if r == 1.0 {
        dd(m - 1.0, 0.0)
    } else {
        let p = dd::two_prod(m, r);
        dd::two_sum(p.hi - 1.0, p.lo)
    };
    // log1p(u) = u - u^2/2 + u^3/3 + u^4 * poly(u)
    let u2 = dd::mul(u, u);
    let u3 = dd::mul(u2, u);
    let uh = u.hi;
    let mut p = 0.0;
    for c in LN_C.iter().rev() {
        p = p * uh + c;
    }
    let u4 = (uh * uh) * (uh * uh);
    let tail = u4 * p;
    let s = dd::add(u, dd(-0.5 * u2.hi, -0.5 * u2.lo));
    let s = dd::add(s, dd::mul(u3, INV3_DD));
    let s = dd::add_d(s, tail);
    let s = dd::add(s, nl);
    if e == 0 {
        s
    } else {
        dd::add(s, dd::mul_d(LN2_DD, e as f64))
    }
}

/// ln 2 cut to 42 significant bits, so that `e * LN2_HI` is exact for |e| < 2^11; `LN2_MID` is
/// the rest of the double nearest ln 2 (11 bits, so `e * LN2_MID` is exact too).
const LN2_HI: f64 = f64::from_bits(LN2_DD.hi.to_bits() & !0x7ff);
const LN2_MID: f64 = LN2_DD.hi - LN2_HI;

/// ln of a positive normal `x` as `(h, l, err)`: `h + l` is ln x to within the absolute error
/// `err` (`l` is below a few ulps of `h`), or `None` for anything else. Error derivation, u = 2^-53,
/// with m = x / 2^e in [0.75, 1.5), the table remainder |u| < 2^-6 (|u| < 2^-7 away from the two
/// bins around 1) and ln x = e ln 2 - ln r + log1p(u):
/// * `e ln 2` and `-ln r` enter as exact products and a double-double table (error below 2^-97);
///   u is formed exactly (`two_prod`, `two_sum`; in the two bins around 1 it is `m - 1`, exact);
/// * the log1p series is cut after u^11: the next term is below u^12 / 12 < 2^-69;
/// * `B = t G` with t = u (u + 2 u_lo) and G = -1/2 + u (1/3 + u P(u)) carries three roundings
///   (t, G, the product), each at most u times a quantity of size u^2 / 2, so at most
///   1.5 u * u^2 in total (1/3 is carried as a double-double);
/// * the final low-order sum `lw` adds up to six terms of size about u^2 / 2 (plus terms of
///   size up to 2^-32 times e, whose roundings are below 2^-85): at most 1.5 u * u^2 more;
/// * `u_lo` times 2 u^2 and the like are below 2^-100.
///
/// The absolute total is below 3 u * u^2 + 2^-85; the returned `err` is `u^2 2^-49 + 2^-80 |h|`
/// with u^2 2^-49 = 16 * 2^-53 u^2, a factor of 5 above the bound.
#[inline]
pub(crate) fn ln_fast_core(x: f64) -> Option<(f64, f64, f64)> {
    if !(f64::MIN_POSITIVE..f64::INFINITY).contains(&x) {
        return None;
    }
    // m in [0.75, 1.5) and the bin j = floor(64 m) - 48 from the bits, without a branch: when the
    // top mantissa bit is set (m >= 1.5) the exponent field is lowered by one, which halves m.
    let bits = x.to_bits();
    let mant = bits & 0x000f_ffff_ffff_ffff;
    let c = (bits >> 51) & 1;
    let e = (bits >> 52) as i32 - 1023 + c as i32;
    let m = f64::from_bits(mant | ((0x3ff - c) << 52));
    let j = if c == 1 {
        (mant >> 47).wrapping_sub(16)
    } else {
        16 + (mant >> 46)
    } as usize;
    let r = LN_R.get(j).copied().unwrap_or(1.0);
    let nl = LN_NEG_LN_R.get(j).copied().unwrap_or(dd(0.0, 0.0));
    // u = m r - 1, exactly. r has at most 12 significant bits, so with the top 41 bits `mh` of m
    // the products `mh r` and `(m - mh) r` are exact, and `p = fl(m r)` differs from their sum by
    // `lo = (mh r - p) + (m - mh) r`, itself a double. `p - 1` is exact (p within 2^-5 of 1), and
    // |p - 1| >= |lo| unless it is 0 (|lo| <= 2^-54, p - 1 is a multiple of 2^-53), so the
    // `fast_two_sum` is exact. In the two bins around 1, r = 1 and u = m - 1 with `lo` = 0.
    let mh = f64::from_bits(m.to_bits() & !0xfff);
    let p = m * r;
    let lo = (mh * r - p) + (m - mh) * r;
    let us = fast_two_sum(p - 1.0, lo);
    let (uh, ul) = (us.hi, us.lo);
    // log1p(u) = u + u^2 (-1/2 + u (1/3 + u P(u))), P = -1/4 + u/5 - ... down to u^7/11, the
    // polynomial P in the order of Estrin (its rounding is far below the bound: P enters
    // multiplied by u^4).
    let [c0, c1, c2, c3, c4, c5, c6, c7, _, _, _] = LN_C;
    let u2 = uh * uh;
    let p =
        ((c0 + uh * c1) + u2 * (c2 + uh * c3)) + (u2 * u2) * ((c4 + uh * c5) + u2 * (c6 + uh * c7));
    let t = uh * (uh + (ul + ul));
    let b = t * (-0.5 + uh * (INV3_DD.hi + uh * p)) + (t * uh) * INV3_DD.lo;
    let ef = e as f64;
    // |ef LN2_HI| is 0 or at least 0.69 > |nl.hi|; |s.hi| >= |uh| (see the table: |nl.hi| >= 0.0157
    // when it is not 0, and |u| < 2^-7 there); |s2.hi| >= |lw|: the two `fast_two_sum` are exact.
    let s = fast_two_sum(ef * LN2_HI, nl.hi);
    let s2 = fast_two_sum(s.hi, uh);
    let early = (((ef * LN2_MID + ef * LN2_DD.lo) + nl.lo) + ul) + (s.lo + s2.lo);
    let lw = early + b;
    let f = fast_two_sum(s2.hi, lw);
    let err = (uh * uh) * 1.776_356_839_400_250_5e-15 + abs(f.hi) * 8.271_806_125_530_277e-25;
    Some((f.hi, f.lo, err))
}

/// The fast path of [`ln`]: `None` unless the argument is a positive normal number and the
/// rounding of the result is certain (then the result is what [`ln_slow`] returns).
pub(crate) fn ln_fast(x: f64) -> Option<f64> {
    let (h, l, err) = ln_fast_core(x)?;
    round_test(h, l, err)
}

/// Natural logarithm of `x`. Domain: x >= 0. Faithfully rounded (measured maximum error in
/// `crates/oa-math/tests/reference.rs`), subnormals included. ln(+0) = ln(-0) = -inf, negative
/// arguments and NaN give NaN, ln(+inf) = +inf, ln(1) = +0.
pub fn ln(x: f64) -> f64 {
    match ln_fast(x) {
        Some(r) => r,
        None => ln_slow(x),
    }
}

/// The double-double path of [`ln`] (also the test hook for the fast path).
pub(crate) fn ln_slow(x: f64) -> f64 {
    if x.is_nan() {
        return NAN;
    }
    if x == 0.0 {
        return f64::NEG_INFINITY;
    }
    if x < 0.0 {
        return NAN;
    }
    if x == f64::INFINITY {
        return x;
    }
    ln_dd(x).hi
}
