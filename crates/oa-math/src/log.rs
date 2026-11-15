//! ln: split off the binary exponent, reduce the mantissa by a 48-entry table of reciprocals
//! (so the remainder u is below 2^-6 and exact to form), then a log1p series in double-double.

use crate::consts::{INV3_DD, LN2_DD, LN_C, LN_NEG_LN_R, LN_R};
use crate::dd::{self, dd, two_prod, two_sum, Dd};
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
pub(crate) fn ln_fast_core(x: f64) -> Option<(f64, f64, f64)> {
    if !(f64::MIN_POSITIVE..f64::INFINITY).contains(&x) {
        return None;
    }
    let (mut e, mut m) = frexp1(x);
    if m >= 1.5 {
        m *= 0.5;
        e += 1;
    }
    let j = ((m * 64.0) as usize).wrapping_sub(48);
    let r = LN_R.get(j).copied().unwrap_or(1.0);
    let nl = LN_NEG_LN_R.get(j).copied().unwrap_or(dd(0.0, 0.0));
    let (uh, ul) = if r == 1.0 {
        (m - 1.0, 0.0)
    } else {
        let p = two_prod(m, r);
        let s = two_sum(p.hi - 1.0, p.lo);
        (s.hi, s.lo)
    };
    // log1p(u) = u + u^2 (-1/2 + u (1/3 + u P(u))), P = -1/4 + u/5 - ... down to u^7/11
    let [c0, c1, c2, c3, c4, c5, c6, c7, _, _, _] = LN_C;
    let p = c0 + uh * (c1 + uh * (c2 + uh * (c3 + uh * (c4 + uh * (c5 + uh * (c6 + uh * c7))))));
    let t = uh * (uh + (ul + ul));
    let b = t * (-0.5 + uh * (INV3_DD.hi + uh * p)) + (t * uh) * INV3_DD.lo;
    let ef = e as f64;
    let s = two_sum(ef * LN2_HI, nl.hi);
    let s2 = two_sum(s.hi, uh);
    let lw = ((((ef * LN2_MID + ef * LN2_DD.lo) + nl.lo) + ul) + b) + (s.lo + s2.lo);
    let f = two_sum(s2.hi, lw);
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
