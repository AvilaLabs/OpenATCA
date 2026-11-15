//! pow and powi.
//!
//! pow forms y * ln x as a double-double product of a double-double logarithm (about 2^-70
//! relative) and y, then exponentiates that double-double, so the result is faithfully rounded
//! even when |y ln x| is near the overflow limit of about 709.78 (where a plain-double
//! product would already carry an error of several ulps).

use crate::dd::{self, two_prod, two_sum, Dd};
use crate::exp::{exp_dd, exp_fast_core, EXP_FAST_ERR};
use crate::log::{ln_dd, ln_fast_core};
use crate::util::{abs, frexp1, is_neg, pow2, round_test, scale_round, with_sign_of, NAN};

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
/// a negative finite x with a finite non-integer y gives NaN. A power-of-two base with an integer
/// y of at most 2^31 in magnitude is exact (it is [`powi`]).
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
    // a power of two to an integer power is exact (and its half-way cases are true ties)
    if is_int && abs(y) <= 2_147_483_647.0 && frexp1(ax).1 == 1.0 {
        return powi(x, y as i32);
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

/// Product of two normalised double-doubles in [1, 2) with binary exponents `ka` and `kb`,
/// returned in [1, 2) with the exponents added.
fn mul_scaled(a: Dd, ka: i64, b: Dd, kb: i64) -> (Dd, i64) {
    let d = dd::mul(a, b);
    if d.hi >= 2.0 {
        (dd::dd(d.hi * 0.5, d.lo * 0.5), ka + kb + 1)
    } else {
        (d, ka + kb)
    }
}

/// `x` raised to the integer power `n`. `x = m 2^e` with `m` in [1, 2); `m^|n|` is formed in
/// double-double by repeated squaring (`acc = 1; b = m;` then while `p = |n|` is not zero: if
/// `p` is odd `acc = acc * b`; `p = p >> 1`; if `p` is not zero `b = b * b`), every product
/// renormalised into [1, 2) with the power of two carried in an integer, so nothing overflows
/// or underflows for any `n`; a negative `n` takes the double-double reciprocal of that. The
/// result is `d 2^k` with `k = +-(e |n|) + carried`, rounded once by `scale_round`, so a
/// representable power (2^-1074 for instance) is never lost to the overflow of the power of the
/// opposite sign. The relative error before the final rounding is about `|n| 2^-104` (at most
/// 2^-72 for any `i32`), so the result is within one ulp, and exact whenever the exact power
/// is representable (the double-double chain is then exact). Zeros and infinities follow the
/// sign rule of an odd `n`; `powi(x, 0) = 1` for every x including NaN, any other NaN gives the
/// canonical NaN.
pub fn powi(x: f64, n: i32) -> f64 {
    if n == 0 {
        return 1.0;
    }
    if x.is_nan() {
        return NAN;
    }
    let neg_result = is_neg(x) && n & 1 == 1;
    let r = if x == 0.0 || x.is_infinite() {
        if (x == 0.0) == (n < 0) {
            f64::INFINITY
        } else {
            0.0
        }
    } else {
        let (e, m) = frexp1(abs(x));
        let mut p = n.unsigned_abs();
        let (mut acc, mut ka) = (dd::dd(1.0, 0.0), 0i64);
        let (mut b, mut kb) = (dd::dd(m, 0.0), 0i64);
        while p != 0 {
            if p & 1 == 1 {
                (acc, ka) = mul_scaled(acc, ka, b, kb);
            }
            p >>= 1;
            if p != 0 {
                (b, kb) = mul_scaled(b, kb, b, kb);
            }
        }
        let mut k = i64::from(e) * i64::from(n.unsigned_abs()) + ka;
        if n < 0 {
            acc = dd::div(dd::dd(1.0, 0.0), acc);
            k = -k;
        }
        // |d| is in (1/2, 2): beyond this the result is surely infinite or zero
        let k = if k > 4000 {
            4000
        } else if k < -4000 {
            -4000
        } else {
            k as i32
        };
        scale_round(acc, k)
    };
    if neg_result {
        -r
    } else {
        r
    }
}
