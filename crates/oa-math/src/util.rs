//! Bit-level helpers: sign, absolute value, exact powers of two and correct rounding of a
//! double-double scaled by a power of two. Nothing here calls the platform maths library.

use crate::dd::Dd;

const SIGN: u64 = 0x8000_0000_0000_0000;
const MANT_MASK: u64 = 0x000f_ffff_ffff_ffff;

/// The one NaN this crate returns, so output bits do not depend on the machine's default NaN.
pub(crate) const NAN: f64 = f64::from_bits(0x7ff8_0000_0000_0000);

pub(crate) fn abs(x: f64) -> f64 {
    f64::from_bits(x.to_bits() & !SIGN)
}

/// True when the sign bit is set (including -0 and negative NaN).
pub(crate) fn is_neg(x: f64) -> bool {
    x.to_bits() & SIGN != 0
}

/// `mag` with the sign of `sgn`.
pub(crate) fn with_sign_of(mag: f64, sgn: f64) -> f64 {
    f64::from_bits((mag.to_bits() & !SIGN) | (sgn.to_bits() & SIGN))
}

/// `x` negated when `neg` is true.
pub(crate) fn negate_if(x: f64, neg: bool) -> f64 {
    if neg {
        -x
    } else {
        x
    }
}

/// Exactly 2^k for -1074 <= k <= 1023; 0 below, infinity above.
pub(crate) fn pow2(k: i32) -> f64 {
    if k > 1023 {
        f64::INFINITY
    } else if k < -1074 {
        0.0
    } else if k < -1022 {
        f64::from_bits(1u64 << (k + 1074))
    } else {
        f64::from_bits(((k + 1023) as u64) << 52)
    }
}

/// `x * 2^k` in two exact steps, for |k| up to about 2040 (the product is exact whenever the
/// result is a normal number).
pub(crate) fn mul_pow2(x: f64, k: i32) -> f64 {
    let k1 = k / 2;
    x * pow2(k1) * pow2(k - k1)
}

/// Unbiased binary exponent of a positive finite nonzero double (subnormals included).
pub(crate) fn exponent(x: f64) -> i32 {
    let bits = x.to_bits();
    let be = ((bits >> 52) & 0x7ff) as i32;
    if be == 0 {
        63 - (bits & MANT_MASK).leading_zeros() as i32 - 1074
    } else {
        be - 1023
    }
}

/// Splits a positive finite double into `(e, m)` with `x = m * 2^e` and `m` in [1, 2).
pub(crate) fn frexp1(x: f64) -> (i32, f64) {
    let (y, adj) = if x < f64::MIN_POSITIVE {
        (x * pow2(54), -54)
    } else {
        (x, 0)
    };
    let bits = y.to_bits();
    let e = ((bits >> 52) & 0x7ff) as i32 - 1023 + adj;
    let m = f64::from_bits((bits & MANT_MASK) | (0x3ff << 52));
    (e, m)
}

/// Rounds `d * 2^k` to the nearest double (ties to even) in a single rounding, including the
/// subnormal range, overflow to infinity and underflow to zero. `d` must be normalised
/// (`hi == fl(hi + lo)`) and nonzero.
pub(crate) fn scale_round(d: Dd, k: i32) -> f64 {
    let neg = is_neg(d.hi);
    let hi = abs(d.hi);
    let lo = if neg { -d.lo } else { d.lo };
    let e = exponent(hi);
    let r = if e + k > 1023 {
        f64::INFINITY
    } else if e + k >= -1022 {
        mul_pow2(hi, k)
    } else if e + k < -1100 {
        0.0
    } else {
        // Subnormal result: count in units of 2^-1074, round hi + lo to an integer once.
        let s = k + 1074;
        let w = mul_pow2(hi, s);
        let lw = mul_pow2(lo, s);
        let magic = 4_503_599_627_370_496.0; // 2^52
        let n = (w + magic) - magic;
        // r = w - n is exact and in [-1/2, 1/2]. The sign of one rounded sum of two doubles is
        // the sign of the exact sum, so r - 1/2 + lw > 0 is exactly "hi + lo is above the
        // half-way point" and r + 1/2 + lw < 0 "below the lower one"; at r = +-1/2 the low term
        // alone decides, and with lw = 0 the tie stays at n (even, as w was rounded to even).
        let r = w - n;
        let n = if (r - 0.5) + lw > 0.0 {
            n + 1.0
        } else if (r + 0.5) + lw < 0.0 {
            n - 1.0
        } else {
            n
        };
        f64::from_bits(n as u64)
    };
    negate_if(r, neg)
}

/// The rounding test of Ziv's method, shared by every fast path. The true value is known to lie
/// within `err` (absolute) of `h + l`, where `|l|` is at most a few ulps of `h` (the sin, cos and
/// atan fast paths pass `|l|` up to 2^-10 of `h` and allow for the larger rounding in their `err`). Returns the
/// double nearest to `h + l` when that double is also the one nearest to every point of the
/// interval, and `None` when a rounding boundary may lie inside it (the caller then runs the
/// double-double path). `fl` is monotone, so `fl(h + l - err) == fl(h + l + err)` pins
/// `fl(v)` for every `v` between them; `l + err` and `l - err` each round once, by at most
/// 2^-53 of a quantity of the size of an ulp of `h`, which every caller's `err` (at least
/// 2^-90 of `h`, in practice 2^-60) dwarfs: the bounds are written with a factor of two or more
/// to spare. A NaN anywhere makes the comparison false, so it also returns `None`.
pub(crate) fn round_test(h: f64, l: f64, err: f64) -> Option<f64> {
    let a = h + (l + err);
    let b = h + (l - err);
    if a == b {
        Some(a)
    } else {
        None
    }
}
