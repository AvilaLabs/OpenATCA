//! pow and powi.
//!
//! pow forms y * ln x as a double-double product of a double-double logarithm (about 2^-70
//! relative) and y, then exponentiates that double-double, so the result is faithfully rounded
//! even when |y ln x| is near the overflow limit of about 709.78 (where a plain-double
//! product would already carry an error of several ulps).

use crate::dd::{self, two_prod, two_sum};
use crate::exp::{exp_dd, exp_fast_core, EXP_FAST_ERR};
use crate::log::{ln_dd, ln_fast_core};
use crate::util::{abs, is_neg, pow2, round_test, scale_round, with_sign_of, NAN};

/// (is an integer, is an odd integer) for a finite y.
fn int_kind(y: f64) -> (bool, bool) {
    let ay = abs(y);
    if ay >= 9_007_199_254_740_992.0 {
        return (true, false); // 2^53 and above: always an even integer
    }
    let t = ay as u64;
    if t as f64 == ay {
        (true, t & 1 == 1)
    } else {
        (false, false)
    }
}

/// The fast path of [`pow`]: `None` unless both arguments are finite and nonzero, the result is
/// a normal number well inside the range and the rounding of the result is certain (then the
/// result is what [`pow_slow`] returns). z = y ln x is formed as a double-double product of the
/// fast logarithm (absolute error `lerr` on ln x, so |y| lerr on z) and exponentiated by the
/// fast exp for |z| < 700; the result's absolute error (mantissa scale, below 2.02) is then
/// [`EXP_FAST_ERR`] + 2.02 |y| lerr + 2^-96 (the rounding of `y * ll`). To keep the bits equal
/// to those of the double-double path, which is itself not exact for large |z| (its ln has a
/// relative error of about 2^-70, measured at most 2^-72.9 over the 3,200 committed `ln`
/// cases, bounded here by 2^-68, and
/// its exp 2^-75), the slow path's own error, 2.02 (2^-68 |z| + 2^-74), is added as well, and the
/// whole is rounded up by a factor of two.
pub(crate) fn pow_fast(x: f64, y: f64) -> Option<f64> {
    if !(x.is_finite() && y.is_finite())
        || x == 0.0
        || y == 0.0
        || abs(y) > 4.611_686_018_427_388e18
    {
        return None;
    }
    let neg_result = if is_neg(x) {
        let (is_int, is_odd) = int_kind(y);
        if !is_int {
            return None;
        }
        is_odd
    } else {
        false
    };
    let (lh, ll, lerr) = ln_fast_core(abs(x))?;
    if lh == 0.0 {
        return None;
    }
    let zp = two_prod(y, lh);
    let z = two_sum(zp.hi, zp.lo + y * ll);
    if z.hi.is_nan() || abs(z.hi) >= 700.0 {
        return None;
    }
    let (h, l, k) = exp_fast_core(z.hi, z.lo);
    let err = 2.0
        * (EXP_FAST_ERR
            + 2.02
                * (abs(y) * lerr
                    + 3.388_131_789_017_201_4e-21 * abs(z.hi)
                    + 5.293_955_920_339_377e-23));
    let r = round_test(h, l, err)? * pow2(k);
    Some(if neg_result { -r } else { r })
}

/// `x` raised to the power `y`. Domain: all doubles. Faithfully rounded (measured maximum error
/// in `crates/oa-math/tests/reference.rs`), subnormal results included. Special values follow
/// C99 Annex F: pow(x, +-0) = 1 and pow(1, y) = 1 for every x and y including NaN; other NaN
/// operands give NaN; pow(-1, +-inf) = 1; |x| < 1 gives +0 for y = +inf and +inf for y = -inf,
/// |x| > 1 the reverse; pow(+-0, y) is +-inf (divide-by-zero) for y < 0 and +-0 for y > 0, with
/// the sign of x only when y is an odd integer, else +inf or +0; pow(+inf, y) is +0 for y < 0
/// and +inf for y > 0; pow(-inf, y) is the odd-integer-signed 0 or inf of the same rule;
/// a negative finite x with a finite non-integer y gives NaN.
pub fn pow(x: f64, y: f64) -> f64 {
    match pow_fast(x, y) {
        Some(r) => r,
        None => pow_slow(x, y),
    }
}

/// The double-double path of [`pow`] (also the test hook for the fast path).
pub(crate) fn pow_slow(x: f64, y: f64) -> f64 {
    if y == 0.0 || x == 1.0 {
        return 1.0;
    }
    if x.is_nan() || y.is_nan() {
        return NAN;
    }
    let ax = abs(x);
    if y.is_infinite() {
        if ax == 1.0 {
            return 1.0;
        }
        return if (ax < 1.0) == (y > 0.0) {
            0.0
        } else {
            f64::INFINITY
        };
    }
    let (is_int, is_odd) = int_kind(y);
    let neg_result = is_neg(x) && is_odd;
    if x.is_infinite() {
        let r = if y > 0.0 { f64::INFINITY } else { 0.0 };
        return if neg_result { -r } else { r };
    }
    if x == 0.0 {
        let r = if y > 0.0 { 0.0 } else { f64::INFINITY };
        return if neg_result { -r } else { r };
    }
    if is_neg(x) && !is_int {
        return NAN;
    }
    let l = ln_dd(ax);
    let zh = y * l.hi;
    let r = if zh > 800.0 {
        f64::INFINITY
    } else if zh < -800.0 {
        0.0
    } else {
        let (d, k) = exp_dd(dd::mul_d(l, y));
        scale_round(d, k)
    };
    with_sign_of(r, if neg_result { -1.0 } else { 1.0 })
}

/// `x` raised to the integer power `n` by repeated squaring, in this fixed order (the result is
/// reproducible bit for bit): `acc = 1; b = x;` then while `m = |n|` is not zero: if `m` is odd
/// `acc = acc * b`; `m = m >> 1`; if `m` is not zero `b = b * b`. A negative `n` returns
/// `1 / acc`. Every operation is one IEEE-754 multiplication or division, so the result is
/// deterministic, but the error grows with the number of multiplications (about `log2(n)`
/// roundings in `b`, magnified by the exponent: roughly `n` ulps in the worst case); it is not
/// faithfully rounded for large `|n|`. `powi(x, 0) = 1` for every x including NaN.
pub fn powi(x: f64, n: i32) -> f64 {
    let mut m = n.unsigned_abs();
    let mut acc = 1.0;
    let mut b = x;
    while m != 0 {
        if m & 1 == 1 {
            acc *= b;
        }
        m >>= 1;
        if m != 0 {
            b *= b;
        }
    }
    if n < 0 {
        1.0 / acc
    } else {
        acc
    }
}
