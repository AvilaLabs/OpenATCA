//! hypot and cbrt.

use crate::dd::{self, dd, fast_two_sum, two_prod, two_sqr, two_sum, Dd};
use crate::util::{
    abs, exponent, frexp1, mul_pow2, pow2, round_test, scale_round, with_sign_of, NAN,
};

/// The double-double path of [`hypot`] (also the test hook for the fast path).
pub(crate) fn hypot_slow(x: f64, y: f64) -> f64 {
    match hypot_slow_operands(x, y) {
        Ok(r) => r,
        Err((big, small)) => hypot_dd(big, small),
    }
}

/// The first step of [`hypot_slow`]: `Ok(result)` where no double-double work is needed (an
/// infinity, a NaN, a zero or negligible smaller operand), else `Err((big, small))`, the larger
/// and smaller magnitude, finite, `big >= small >= 2^-60 big > 0`.
fn hypot_slow_operands(x: f64, y: f64) -> Result<f64, (f64, f64)> {
    if x.is_infinite() || y.is_infinite() {
        return Ok(f64::INFINITY);
    }
    if x.is_nan() || y.is_nan() {
        return Ok(NAN);
    }
    let (ax, ay) = (abs(x), abs(y));
    let (big, small) = if ay > ax { (ay, ax) } else { (ax, ay) };
    if small == 0.0 {
        return Ok(big);
    }
    // small/big < 2^-60: sqrt(1 + q^2) rounds to 1
    if small < big * 8.673_617_379_884_035e-19 {
        return Ok(big);
    }
    Err((big, small))
}

/// The double-double core of [`hypot_slow`] for `big >= small >= 2^-60 big > 0`, both finite: the
/// operands scaled by `2^-e` ([`dd_operands`]), the root of the sum of squares
/// ([`hypot_dd_root`]), scaled back with a single rounding. Separate functions so that each is
/// proved apart (`proofs` below).
fn hypot_dd(big: f64, small: f64) -> f64 {
    let (e, a, b) = dd_operands(big, small);
    scale_round(hypot_dd_root(a, b), e)
}

/// The exponent `e` of `big` and the operands scaled by `2^-e`.
fn dd_operands(big: f64, small: f64) -> (i32, f64, f64) {
    let e = exponent(big);
    (e, mul_pow2(big, -e), mul_pow2(small, -e))
}

/// `sqrt(a^2 + b^2)` as a double-double, for `a` in [1, 2) and `b` in (0, a] (in practice at least
/// about 2^-60).
fn hypot_dd_root(a: f64, b: f64) -> Dd {
    let sum = dd::add(dd::two_prod(a, a), dd::two_prod(b, b));
    dd::sqrt(sum)
}

/// The double-double path of [`cbrt`] (also the test hook for the fast path).
pub(crate) fn cbrt_slow(x: f64) -> f64 {
    if x.is_nan() {
        return NAN;
    }
    if x == 0.0 || x.is_infinite() {
        return x;
    }
    let ax = abs(x);
    let (e, m) = frexp1(ax);
    // x = m * 2^e = (m * 2^r) * 2^(3q) with r in 0..=2
    let q = if e >= 0 { e / 3 } else { -((2 - e) / 3) };
    let r = e - 3 * q;
    let mm = m * pow2(r);
    // Newton on y^3 = mm from a chord start, then one double-double correction step.
    let mut y = 1.0 + (mm - 1.0) / 7.0;
    for _ in 0..6 {
        y -= (y * y * y - mm) / (3.0 * y * y);
    }
    let y2 = dd::two_prod(y, y);
    let y3 = dd::mul_d(y2, y);
    let rem = dd::sub(dd(mm, 0.0), y3);
    let delta = rem.hi / (3.0 * y2.hi);
    let yd: Dd = dd::fast_two_sum(y, delta);
    with_sign_of(scale_round(yd, q), x)
}

// ---- fast paths -------------------------------------------------------------------------

/// Relative error bound of the fast hypot and cbrt: the computed double-double results carry
/// about 2^-100 (see the derivations below); the constant is 2^-80.
const ROOT_FAST_REL: f64 = 8.271_806_125_530_277e-25;

/// Operand range of the fast hypot: 2^-380 <= big < 2^500. Inside it, with small >= 2^-60 big,
/// no square, split or low part under- or overflows (the low parts of `small^2` stay above
/// 2^-986), so the computation needs no scaling by a power of two.
const HYPOT_LO: f64 = 1.0e-114;
const HYPOT_HI: f64 = 1.0e150;

#[cfg(test)]
pub(crate) fn hypot_fast_plain_for_test(big: f64, small: f64) -> Option<f64> {
    hypot_fast_plain(big, small)
}

#[cfg(test)]
pub(crate) fn hypot_fast_scaled_for_test(x: f64, y: f64) -> Option<f64> {
    hypot_fast_scaled(x, y)
}

/// The fast path of [`hypot`] for the common range: for operands with 1e-114 <= |big| < 1e150 and 2^-60 |big| <=
/// |small|, `a^2 + b^2` as a double-double from two exact squares and a sum (rounding of the low
/// parts below 2^-104), a double square root and one correction with the exact square:
/// `r = sqrt(sh)`, `rem = sh + sl - r^2`, `cl = rem * (1 / 2r)`. The error is the rounding of
/// `cl` (the reciprocal and the product, below 2^-105 of `r` as `cl` is below 2^-52 `r`) and of
/// the low sum (below 2^-103 relative) plus the second-order term `cl^2 / 2r`: below 2^-102. The
/// sum of the squares is used as `s.hi` plus the low terms without renormalising: `s.hi - r^2` is
/// exact (the two agree to 2^-52), and `cl` stays below 1.5 2^-52 `r`.
/// The operands are ordered by their bit patterns (equal to the order of magnitudes) and used
/// as they are (by [`hypot_fast`], which checks the range and the ratio): the derivation does
/// not depend on the scale, only on the ratio.
#[inline]
fn hypot_fast_plain(big: f64, small: f64) -> Option<f64> {
    let pa = two_sqr(big);
    let pb = two_sqr(small);
    // pa.hi >= pb.hi, as the squares round monotonically
    let s = fast_two_sum(pa.hi, pb.hi);
    let r = oa_ieee::sqrt(s.hi);
    let half_inv = 0.5 / r;
    let sq = two_sqr(r);
    let cl = (((s.hi - sq.hi) - sq.lo) + (s.lo + (pa.lo + pb.lo))) * half_inv;
    round_test(r, cl, r * ROOT_FAST_REL)
}

/// The fast path of [`hypot`] for the rest of the range: for finite operands with 2^-60 |big| <=
/// |small|, a nonzero `big` and a normal result, `a^2 + b^2` (a, b scaled by 2^-e into [1, 2)
/// and [2^-60, 2)) as a double-double from two exact products and a sum (rounding of the low
/// parts below 2^-104), a double square root and one correction with the exact square:
/// `r = sqrt(sh)`, `rem = sh + sl - r^2`, `cl = rem / 2r`. The error is the rounding of `cl`
/// and of the low sum (below 2^-103 relative) plus the second-order term `cl^2 / 2r`: below
/// 2^-102.
fn hypot_fast_scaled(x: f64, y: f64) -> Option<f64> {
    if !(x.is_finite() && y.is_finite()) {
        return None;
    }
    let (ax, ay) = (abs(x), abs(y));
    let (big, small) = if ay > ax { (ay, ax) } else { (ax, ay) };
    if !(1e-290..1e290).contains(&big) || small <= 0.0 || small < big * 8.673_617_379_884_035e-19 {
        return None;
    }
    let e = exponent(big);
    let a = mul_pow2(big, -e);
    let b = mul_pow2(small, -e);
    let pa = two_prod(a, a);
    let pb = two_prod(b, b);
    let s = two_sum(pa.hi, pb.hi);
    let f = fast_two_sum(s.hi, s.lo + (pa.lo + pb.lo));
    let r = oa_ieee::sqrt(f.hi);
    let sq = two_prod(r, r);
    let cl = (((f.hi - sq.hi) - sq.lo) + f.lo) / (2.0 * r);
    round_test(r, cl, r * ROOT_FAST_REL).map(|v| v * pow2(e))
}

/// The magnitudes of `x` and `y`, the larger first, ordered by their bit patterns (equal to the order
/// of the magnitudes for non-NaN values).
#[inline]
fn order_by_bits(x: f64, y: f64) -> (f64, f64) {
    let (bx, by) = (abs(x).to_bits(), abs(y).to_bits());
    let (hi_bits, lo_bits) = if by > bx { (by, bx) } else { (bx, by) };
    (f64::from_bits(hi_bits), f64::from_bits(lo_bits))
}

/// The fast path of [`hypot`]: `None` unless the rounding of the result is certain (then the
/// result is what [`hypot_slow`] returns).
#[inline]
pub(crate) fn hypot_fast(x: f64, y: f64) -> Option<f64> {
    // Operands ordered by their bit patterns (the order of the magnitudes). NaN and infinity fail
    // the range test and go the scaled way, which refuses them.
    let (big, small) = order_by_bits(x, y);
    if (HYPOT_LO..HYPOT_HI).contains(&big) {
        if small < big * 8.673_617_379_884_035e-19 {
            return None;
        }
        return hypot_fast_plain(big, small);
    }
    hypot_fast_scaled(x, y)
}

/// The fast path of [`cbrt`]: for a normal nonzero finite argument. With the mantissa scaled
/// into mm in [1, 8) by a power of 2^3, three Halley steps from the chord start (error below
/// 11% at the start, cubic convergence: below 2^-50 after two, rounding-limited after three)
/// give y; one correction `delta = (mm - y^3) / (3 y^2)` with y^2 and y^3 from exact products
/// (error below 2^-100 relative to y) brings Newton's quadratic remainder to `delta^2 / y`. The
/// path is taken only when `|delta| <= 2^-44 y` (checked at run time, so the start and the
/// number of steps do not matter for correctness): the remainder is then below 2^-88 y and the
/// error below 2^-87; the bound used is 2^-80.
pub(crate) fn cbrt_fast(x: f64) -> Option<f64> {
    if !(x.is_finite() && abs(x) >= f64::MIN_POSITIVE) {
        return None;
    }
    let ax = abs(x);
    let (e, m) = frexp1(ax);
    let q = if e >= 0 { e / 3 } else { -((2 - e) / 3) };
    let mm = m * pow2(e - 3 * q);
    let mut y = 1.0 + (mm - 1.0) / 7.0;
    for _ in 0..3 {
        let y3 = y * y * y;
        y = y * (y3 + 2.0 * mm) / (2.0 * y3 + mm);
    }
    let y2 = two_prod(y, y);
    let y3 = dd::mul_d(y2, y);
    let rem = (mm - y3.hi) - y3.lo;
    let delta = rem / (3.0 * y2.hi);
    if delta.is_nan() || abs(delta) > y * 5.684_341_886_080_802e-14 {
        return None;
    }
    let f = fast_two_sum(y, delta);
    let r = round_test(f.hi, f.lo, f.hi * ROOT_FAST_REL)? * pow2(q);
    Some(with_sign_of(r, x))
}

/// Euclidean length sqrt(x^2 + y^2). Domain: all doubles. Faithfully rounded (measured maximum
/// error in `crates/oa-math/tests/reference.rs`); no intermediate overflow or underflow, the
/// result overflows only when it does not fit. hypot(+-inf, y) = +inf for every y including NaN
/// (C99 Annex F); otherwise a NaN operand gives NaN; hypot(+-0, +-0) = +0.
pub fn hypot(x: f64, y: f64) -> f64 {
    match hypot_fast(x, y) {
        Some(r) => r,
        None => hypot_slow(x, y),
    }
}

/// Cube root of `x`. Domain: all doubles. Faithfully rounded (measured maximum error in
/// `crates/oa-math/tests/reference.rs`). cbrt(+-0) = +-0, cbrt(+-inf) = +-inf, NaN gives NaN;
/// negative arguments give the negative real root.
pub fn cbrt(x: f64) -> f64 {
    match cbrt_fast(x) {
        Some(r) => r,
        None => cbrt_slow(x),
    }
}

#[cfg(kani)]
mod proofs {
    //! Kani proof harnesses for `hypot` over the domain `|x|, |y| <= HYPOT_DOMAIN` (finite): the
    //! result is finite and not negative (what the horizontal miss of `oa-ca` takes of it,
    //! OA-CA-029). `hypot` is a composition and each part is proved apart, the contract of each
    //! being established by the part before it:
    //!
    //! 1. `operands_are_ordered_and_bounded`: `order_by_bits` gives `big`, `small` finite with
    //!    `0 <= small <= big <= HYPOT_DOMAIN`.
    //! 2. `fast_plain_*`: for `big` in `[HYPOT_LO, HYPOT_DOMAIN]` and `small` in
    //!    `[2^-60 big, big]` (the only operands `hypot_fast` hands it), `hypot_fast_plain` is
    //!    `None` or `Some(finite, not negative)`.
    //! 3. `fast_scaled_*`: `hypot_fast_scaled` on the domain is `None` or `Some(finite, not
    //!    negative)`.
    //!    So `hypot_fast` (the ordering, one range test, then 2 or 3) is `None` or such a value.
    //! 4. `slow_operands_*`: `hypot_slow_operands` on the domain gives `Ok(finite, not negative)`
    //!    or `Err((big, small))` with `0 < small <= big <= HYPOT_DOMAIN` and `small >= big * 2^-60`
    //!    (as the product rounds).
    //! 5. `dd_operands_*`: for those, `e = exponent(big)` is in `[-1074, 25]`, `a` is in `[1, 2)`
    //!    and `b` in `(0, a]`.
    //! 6. `hypot_dd_root(a, b)` is `dd::sqrt(dd::add(two_prod(a, a), two_prod(b, b)))`: the squares
    //!    (`the_square_of_*`: `hi` in `[0, 4]`, `lo` within `LO_MAX`), their sum
    //!    (`the_sum_of_*`: `hi` in `[0.5, 9]`, `lo` within `LO_MAX`) and its root (`the_root_of_*`:
    //!    `hi` in `[0.5, 3.5]`, `lo` finite, not above 1 in magnitude).
    //! 7. `scale_round_*`: for `hi` in `[0.5, 3.5]`, `|lo| <= 1`, `k` in `[-1074, 25]`, `scale_round`
    //!    is finite and not negative.
    //!    So `hypot_slow` (4, then `hypot_dd` = 5, 6, 7) is finite and not negative.
    //!
    //! `hypot` is `hypot_fast` where it has a value, else `hypot_slow`: finite and not negative.
    use super::*;

    /// The domain bound, with room over the 4.1e7 of the horizontal miss's closest point.
    const HYPOT_DOMAIN: f64 = 5.0e7;
    const RATIO: f64 = 8.673_617_379_884_035e-19;

    fn within(x: f64) -> bool {
        x.is_finite() && x >= -HYPOT_DOMAIN && x <= HYPOT_DOMAIN
    }

    // REQ: OA-MATH-039
    #[kani::proof]
    fn operands_are_ordered_and_bounded() {
        let (x, y): (f64, f64) = (kani::any(), kani::any());
        kani::assume(within(x) && within(y));
        let (big, small) = order_by_bits(x, y);
        assert!(big.is_finite() && big <= HYPOT_DOMAIN);
        assert!(0.0 <= small && small <= big);
    }

    // REQ: OA-MATH-039
    #[kani::proof]
    fn fast_plain_of_bounded_operands_is_none_or_finite_not_negative() {
        let (big, small): (f64, f64) = (kani::any(), kani::any());
        kani::assume(big >= HYPOT_LO && big <= HYPOT_DOMAIN);
        kani::assume(small <= big && small >= big * RATIO);
        if let Some(r) = hypot_fast_plain(big, small) {
            assert!(r.is_finite() && r >= 0.0);
        }
    }

    // REQ: OA-MATH-039
    #[kani::proof]
    fn fast_scaled_of_bounded_operands_is_none_or_finite_not_negative() {
        let (x, y): (f64, f64) = (kani::any(), kani::any());
        kani::assume(within(x) && within(y));
        if let Some(r) = hypot_fast_scaled(x, y) {
            assert!(r.is_finite() && r >= 0.0);
        }
    }

    // REQ: OA-MATH-039
    #[kani::proof]
    fn slow_operands_of_bounded_operands_are_a_value_or_ordered_operands() {
        let (x, y): (f64, f64) = (kani::any(), kani::any());
        kani::assume(within(x) && within(y));
        match hypot_slow_operands(x, y) {
            Ok(r) => assert!(r.is_finite() && r >= 0.0),
            Err((big, small)) => {
                assert!(small > 0.0 && small >= big * RATIO && small <= big);
                assert!(big.is_finite() && big <= HYPOT_DOMAIN);
            }
        }
    }

    // REQ: OA-MATH-039
    #[kani::proof]
    fn dd_operands_are_scaled_into_range() {
        let (big, small): (f64, f64) = (kani::any(), kani::any());
        kani::assume(big > 0.0 && big <= HYPOT_DOMAIN);
        kani::assume(small > 0.0 && small <= big && small >= big * RATIO);
        let (e, a, b) = dd_operands(big, small);
        assert!(-1074 <= e && e <= 25);
        assert!(a >= 1.0 && a < 2.0);
        // (Not 2^-60: the product `big * RATIO` of the test rounds, and for a subnormal `big`
        // `small` can lie a hair under the exact 2^-60 `big`.)
        assert!(b > 0.0 && b <= a);
    }

    /// The absolute bound on the low part of a double-double of the values here (the roundings of
    /// values of at most 8 are at most 2^-50; this leaves room).
    const LO_MAX: f64 = 1.0e-10;

    // REQ: OA-MATH-039
    #[kani::proof]
    fn the_square_of_a_scaled_operand_is_a_double_double_in_range() {
        let a: f64 = kani::any();
        kani::assume(a > 0.0 && a < 2.0);
        let p = dd::two_prod(a, a);
        assert!(p.hi >= 0.0 && p.hi <= 4.0);
        assert!(p.lo.is_finite() && p.lo >= -LO_MAX && p.lo <= LO_MAX);
    }

    // REQ: OA-MATH-039
    #[kani::proof]
    fn the_sum_of_two_squares_is_a_double_double_in_range() {
        let (h1, l1, h2, l2): (f64, f64, f64, f64) =
            (kani::any(), kani::any(), kani::any(), kani::any());
        // The first square of a in [1, 2) (at least 1), the second of b in (0, a] (at most the first).
        kani::assume(h1 >= 1.0 && h1 <= 4.0 && l1 >= -LO_MAX && l1 <= LO_MAX);
        kani::assume(h2 >= 0.0 && h2 <= h1 && l2 >= -LO_MAX && l2 <= LO_MAX);
        let s = dd::add(dd(h1, l1), dd(h2, l2));
        assert!(s.hi >= 0.5 && s.hi <= 9.0);
        assert!(s.lo.is_finite() && s.lo >= -LO_MAX && s.lo <= LO_MAX);
    }

    // REQ: OA-MATH-039
    #[kani::proof]
    fn the_root_of_a_double_double_sum_is_in_range() {
        let (h, l): (f64, f64) = (kani::any(), kani::any());
        kani::assume(h >= 0.5 && h <= 9.0 && l >= -LO_MAX && l <= LO_MAX);
        let r = dd::sqrt(dd(h, l));
        assert!(r.hi >= 0.5 && r.hi <= 3.5);
        assert!(r.lo.is_finite() && r.lo >= -1.0 && r.lo <= 1.0);
    }

    // REQ: OA-MATH-039
    #[kani::proof]
    fn scale_round_of_a_root_in_range_is_finite_not_negative() {
        let (hi, lo): (f64, f64) = (kani::any(), kani::any());
        kani::assume(hi >= 0.5 && hi <= 3.5);
        kani::assume(lo >= -1.0 && lo <= 1.0);
        let k: i32 = kani::any();
        kani::assume(-1074 <= k && k <= 25);
        let r = scale_round(Dd { hi, lo }, k);
        assert!(r.is_finite() && r >= 0.0);
    }
}
