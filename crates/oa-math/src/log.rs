//! ln: split off the binary exponent, reduce the mantissa by a 48-entry table of reciprocals
//! (so the remainder u is below 2^-6 and exact to form), then a log1p series in double-double.

use crate::consts::{INV3_DD, LN2_DD, LN_C, LN_NEG_LN_R, LN_R};
use crate::dd::{self, dd, Dd};
use crate::util::{frexp1, NAN};

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

/// Natural logarithm of `x`. Domain: x >= 0. Faithfully rounded (measured maximum error in
/// `crates/oa-math/tests/reference.rs`), subnormals included. ln(+0) = ln(-0) = -inf, negative
/// arguments and NaN give NaN, ln(+inf) = +inf, ln(1) = +0.
pub fn ln(x: f64) -> f64 {
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
