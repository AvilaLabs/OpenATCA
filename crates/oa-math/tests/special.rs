//! IEEE-754 / C99 Annex F special values for every function.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_math::{acos, asin, atan, atan2, cbrt, cos, exp, hypot, ln, pow, powi, sin, sin_cos, tan};
use std::f64::consts::{E, FRAC_PI_2, FRAC_PI_4, LN_2, PI};

const INF: f64 = f64::INFINITY;
const NINF: f64 = f64::NEG_INFINITY;
const CANON_NAN: u64 = 0x7ff8_0000_0000_0000;
const MIN_SUB: f64 = 5e-324;
const MIN_NORMAL: f64 = 2.2250738585072014e-308;

/// Bitwise equality, so the sign of zero counts.
fn same(got: f64, want: f64, what: &str) {
    assert_eq!(
        got.to_bits(),
        want.to_bits(),
        "{what}: got {got:e}, want {want:e}"
    );
}

/// The result is a NaN and it is the one fixed bit pattern.
fn nan(got: f64, what: &str) {
    assert_eq!(got.to_bits(), CANON_NAN, "{what}: got {got:e}, want NaN");
}

/// All NaN bit patterns an input can take.
fn nans() -> [f64; 4] {
    [
        f64::NAN,
        -f64::NAN,
        f64::from_bits(0x7ff0_0000_0000_0001),
        f64::from_bits(0xfff8_0000_dead_beef),
    ]
}

// REQ: OA-MATH-006
#[test]
fn trig_special_values() {
    same(sin(0.0), 0.0, "sin(+0)");
    same(sin(-0.0), -0.0, "sin(-0)");
    same(cos(0.0), 1.0, "cos(+0)");
    same(cos(-0.0), 1.0, "cos(-0)");
    same(tan(0.0), 0.0, "tan(+0)");
    same(tan(-0.0), -0.0, "tan(-0)");
    for x in [INF, NINF] {
        nan(sin(x), "sin(inf)");
        nan(cos(x), "cos(inf)");
        nan(tan(x), "tan(inf)");
        let (s, c) = sin_cos(x);
        nan(s, "sin_cos(inf).0");
        nan(c, "sin_cos(inf).1");
    }
    for x in nans() {
        nan(sin(x), "sin(NaN)");
        nan(cos(x), "cos(NaN)");
        nan(tan(x), "tan(NaN)");
        let (s, c) = sin_cos(x);
        nan(s, "sin_cos(NaN).0");
        nan(c, "sin_cos(NaN).1");
    }
    // subnormals and tiny arguments: sin x = x, cos x = 1, tan x = x
    for x in [MIN_SUB, -MIN_SUB, 1e-310, -MIN_NORMAL, 1e-9, -1e-9] {
        same(sin(x), x, "sin(tiny)");
        same(cos(x), 1.0, "cos(tiny)");
        same(tan(x), x, "tan(tiny)");
    }
    // exact-ish landmarks
    same(sin(FRAC_PI_2), 1.0, "sin(pi/2)");
    same(cos(PI), -1.0, "cos(pi)");
    same(sin(PI), 1.2246467991473532e-16, "sin(pi)");
    same(cos(FRAC_PI_2), 6.123233995736766e-17, "cos(pi/2)");
    same(tan(FRAC_PI_4), 0.9999999999999999, "tan(pi/4)");
    same(tan(FRAC_PI_2), 1.633123935319537e16, "tan(pi/2)");
    // the largest finite argument is handled (Payne-Hanek), not rejected
    assert!(sin(f64::MAX).is_finite() && cos(f64::MAX).is_finite() && tan(f64::MAX).is_finite());
    same(sin(f64::MAX), 0.004961954789184062, "sin(DBL_MAX)");
    same(cos(f64::MAX), -0.9999876894265599, "cos(DBL_MAX)");
    // the classic hard case for argument reduction
    let x = 6381956970095103.0 * 2.0f64.powi(797);
    same(
        cos(x),
        -4.687165924254628e-19,
        "cos(6381956970095103 * 2^797)",
    );
}

// REQ: OA-MATH-007
#[test]
fn inverse_trig_special_values() {
    same(atan(0.0), 0.0, "atan(+0)");
    same(atan(-0.0), -0.0, "atan(-0)");
    same(atan(INF), FRAC_PI_2, "atan(inf)");
    same(atan(NINF), -FRAC_PI_2, "atan(-inf)");
    same(atan(1.0), FRAC_PI_4, "atan(1)");
    same(atan(MIN_SUB), MIN_SUB, "atan(min subnormal)");
    same(asin(0.0), 0.0, "asin(+0)");
    same(asin(-0.0), -0.0, "asin(-0)");
    same(asin(1.0), FRAC_PI_2, "asin(1)");
    same(asin(-1.0), -FRAC_PI_2, "asin(-1)");
    same(acos(1.0), 0.0, "acos(1)");
    same(acos(-1.0), PI, "acos(-1)");
    same(acos(0.0), FRAC_PI_2, "acos(+0)");
    same(acos(-0.0), FRAC_PI_2, "acos(-0)");
    for x in [
        1.0000000000000002,
        -1.0000000000000002,
        2.0,
        INF,
        NINF,
        f64::MAX,
    ] {
        nan(asin(x), "asin(|x|>1)");
        nan(acos(x), "acos(|x|>1)");
    }
    for x in nans() {
        nan(atan(x), "atan(NaN)");
        nan(asin(x), "asin(NaN)");
        nan(acos(x), "acos(NaN)");
        nan(atan2(x, 1.0), "atan2(NaN, 1)");
        nan(atan2(1.0, x), "atan2(1, NaN)");
        nan(atan2(x, x), "atan2(NaN, NaN)");
    }
}

// REQ: OA-MATH-008
#[test]
fn atan2_annex_f_table() {
    let p = 0.0;
    let m = -0.0;
    // y = +-0
    same(atan2(p, p), 0.0, "atan2(+0, +0)");
    same(atan2(m, p), -0.0, "atan2(-0, +0)");
    same(atan2(p, m), PI, "atan2(+0, -0)");
    same(atan2(m, m), -PI, "atan2(-0, -0)");
    for x in [-1.0, -MIN_SUB, NINF, -1e300] {
        same(atan2(p, x), PI, "atan2(+0, x<0)");
        same(atan2(m, x), -PI, "atan2(-0, x<0)");
    }
    for x in [1.0, MIN_SUB, INF, 1e300] {
        same(atan2(p, x), 0.0, "atan2(+0, x>0)");
        same(atan2(m, x), -0.0, "atan2(-0, x>0)");
    }
    // x = +-0, y nonzero
    for y in [1.0, MIN_SUB, 1e300, INF] {
        same(atan2(y, p), FRAC_PI_2, "atan2(y>0, +0)");
        same(atan2(y, m), FRAC_PI_2, "atan2(y>0, -0)");
        same(atan2(-y, p), -FRAC_PI_2, "atan2(y<0, +0)");
        same(atan2(-y, m), -FRAC_PI_2, "atan2(y<0, -0)");
    }
    // infinities
    for y in [1.0, MIN_SUB, 1e300] {
        same(atan2(y, NINF), PI, "atan2(y>0, -inf)");
        same(atan2(-y, NINF), -PI, "atan2(y<0, -inf)");
        same(atan2(y, INF), 0.0, "atan2(y>0, +inf)");
        same(atan2(-y, INF), -0.0, "atan2(y<0, +inf)");
    }
    for x in [-1e300, -1.0, 0.0, -0.0, 1.0, 1e300] {
        same(atan2(INF, x), FRAC_PI_2, "atan2(+inf, x)");
        same(atan2(NINF, x), -FRAC_PI_2, "atan2(-inf, x)");
    }
    same(atan2(INF, NINF), 2.356194490192345, "atan2(+inf, -inf)");
    same(atan2(NINF, NINF), -2.356194490192345, "atan2(-inf, -inf)");
    same(atan2(INF, INF), FRAC_PI_4, "atan2(+inf, +inf)");
    same(atan2(NINF, INF), -FRAC_PI_4, "atan2(-inf, +inf)");
    // extreme ratios
    same(
        atan2(MIN_SUB, f64::MAX),
        0.0,
        "atan2(min, max) underflows to +0",
    );
    same(atan2(-MIN_SUB, f64::MAX), -0.0, "atan2(-min, max)");
    same(atan2(MIN_SUB, 1.0), MIN_SUB, "atan2(min, 1)");
    same(atan2(1.0, MIN_SUB), FRAC_PI_2, "atan2(1, min)");
    same(atan2(MIN_SUB, -1.0), PI, "atan2(min, -1)");
    same(atan2(MIN_SUB, MIN_SUB), FRAC_PI_4, "atan2(min, min)");
    same(atan2(-3.0, -3.0), -2.356194490192345, "atan2(-3, -3)");
    same(atan2(3.0, -3.0), 2.356194490192345, "atan2(3, -3)");
}

// REQ: OA-MATH-009
#[test]
fn exp_ln_special_values() {
    same(exp(0.0), 1.0, "exp(+0)");
    same(exp(-0.0), 1.0, "exp(-0)");
    same(exp(INF), INF, "exp(inf)");
    same(exp(NINF), 0.0, "exp(-inf)");
    same(exp(1.0), E, "exp(1)");
    same(
        exp(f64::from_bits(709.782712893384f64.to_bits() + 1)),
        INF,
        "exp above the overflow limit",
    );
    same(
        exp(709.782712893384),
        1.7976931348622732e308,
        "exp at ln(DBL_MAX)",
    );
    same(
        exp(709.7827128933839),
        1.7976931348620688e308,
        "exp just below it",
    );
    same(exp(710.0), INF, "exp(710)");
    same(exp(1e300), INF, "exp(1e300)");
    same(
        exp(-745.1332191019411),
        5e-324,
        "exp(-745.133...) = min subnormal",
    );
    same(
        exp(-745.1332191019412),
        0.0,
        "exp(-745.1332191019412) underflows",
    );
    same(exp(-746.0), 0.0, "exp(-746)");
    same(exp(-1e300), 0.0, "exp(-1e300)");
    same(exp(MIN_SUB), 1.0, "exp(min)");
    same(exp(-MIN_SUB), 1.0, "exp(-min)");
    same(exp(2.220446049250313e-16), 1.0000000000000002, "exp(2^-52)");
    for x in nans() {
        nan(exp(x), "exp(NaN)");
        nan(ln(x), "ln(NaN)");
    }
    same(ln(0.0), NINF, "ln(+0)");
    same(ln(-0.0), NINF, "ln(-0)");
    same(ln(1.0), 0.0, "ln(1)");
    same(ln(INF), INF, "ln(inf)");
    for x in [-1.0, -MIN_SUB, NINF, -1e300, -MIN_NORMAL] {
        nan(ln(x), "ln(x<0)");
    }
    same(ln(MIN_SUB), -744.4400719213812, "ln(min subnormal)");
    same(ln(f64::MAX), 709.782712893384, "ln(DBL_MAX)");
    same(ln(MIN_NORMAL), -708.3964185322641, "ln(min normal)");
    same(ln(2.0), LN_2, "ln(2)");
    same(
        ln(1.0000000000000002),
        2.2204460492503128e-16,
        "ln(1 + eps)",
    );
    same(
        ln(0.9999999999999999),
        -1.1102230246251565e-16,
        "ln(1 - eps/2)",
    );
}

// REQ: OA-MATH-010
#[test]
fn pow_annex_f_table() {
    let ys = [
        -INF, -1e300, -3.0, -2.0, -1.5, -1.0, -0.5, -MIN_SUB, 0.5, 1.0, 1.5, 2.0, 3.0, 1e300, INF,
    ];
    // pow(x, +-0) = 1 and pow(1, y) = 1 for all, NaN included
    for x in [0.0, -0.0, 1.0, -1.0, 2.0, -2.0, INF, NINF, f64::NAN, 1e300] {
        same(pow(x, 0.0), 1.0, "pow(x, +0)");
        same(pow(x, -0.0), 1.0, "pow(x, -0)");
    }
    for y in ys.iter().copied().chain([f64::NAN, 0.0, -0.0]) {
        same(pow(1.0, y), 1.0, "pow(1, y)");
    }
    // other NaN operands
    for n in nans() {
        for y in [-1.0, 2.0, 0.5, INF, NINF] {
            nan(pow(n, y), "pow(NaN, y)");
            nan(pow(y, n), "pow(x, NaN)");
        }
        nan(pow(n, 1.0), "pow(NaN, 1)");
        nan(pow(n, n), "pow(NaN, NaN)");
    }
    // pow(-1, +-inf) = 1
    same(pow(-1.0, INF), 1.0, "pow(-1, +inf)");
    same(pow(-1.0, NINF), 1.0, "pow(-1, -inf)");
    // |x| < 1 and > 1 with infinite y
    for x in [0.5, -0.5, 0.0, -0.0, MIN_SUB] {
        same(pow(x, INF), 0.0, "pow(|x|<1, +inf)");
        same(pow(x, NINF), INF, "pow(|x|<1, -inf)");
    }
    for x in [2.0, -2.0, 1e300, INF, NINF] {
        same(pow(x, INF), INF, "pow(|x|>1, +inf)");
        same(pow(x, NINF), 0.0, "pow(|x|>1, -inf)");
    }
    // zeros
    same(pow(0.0, -3.0), INF, "pow(+0, odd<0)");
    same(pow(-0.0, -3.0), NINF, "pow(-0, odd<0)");
    for y in [-2.0, -1.5, -0.5, -1e300, -MIN_SUB] {
        same(pow(0.0, y), INF, "pow(+0, y<0 not odd)");
        same(pow(-0.0, y), INF, "pow(-0, y<0 not odd)");
    }
    same(pow(0.0, 3.0), 0.0, "pow(+0, odd>0)");
    same(pow(-0.0, 3.0), -0.0, "pow(-0, odd>0)");
    for y in [2.0, 1.5, 0.5, 1e300, MIN_SUB] {
        same(pow(0.0, y), 0.0, "pow(+0, y>0 not odd)");
        same(pow(-0.0, y), 0.0, "pow(-0, y>0 not odd)");
    }
    // infinite x
    same(pow(INF, -2.0), 0.0, "pow(+inf, y<0)");
    same(pow(INF, 0.5), INF, "pow(+inf, y>0)");
    same(pow(NINF, -3.0), -0.0, "pow(-inf, odd<0)");
    same(pow(NINF, -2.0), 0.0, "pow(-inf, even<0)");
    same(pow(NINF, -0.5), 0.0, "pow(-inf, non-integer<0)");
    same(pow(NINF, 3.0), NINF, "pow(-inf, odd>0)");
    same(pow(NINF, 2.0), INF, "pow(-inf, even>0)");
    same(pow(NINF, 0.5), INF, "pow(-inf, non-integer>0)");
    // negative finite x
    nan(pow(-2.0, 0.5), "pow(-2, 0.5)");
    nan(pow(-2.0, -1.5), "pow(-2, -1.5)");
    nan(pow(-MIN_SUB, 0.5), "pow(-min, 0.5)");
    same(pow(-2.0, 3.0), -8.0, "pow(-2, 3)");
    same(pow(-2.0, -3.0), -0.125, "pow(-2, -3)");
    same(pow(-2.0, 2.0), 4.0, "pow(-2, 2)");
    same(pow(-2.0, 1e300), INF, "pow(-2, huge even)");
    same(pow(-0.5, 9007199254740993.0), 0.0, "pow(-0.5, 2^53+1 odd)");
    same(pow(-2.0, 1025.0), NINF, "pow(-2, 1025) overflows with sign");
    same(
        pow(-2.0, -1075.0),
        -0.0,
        "pow(-2, -1075) underflows with sign",
    );
    // overflow and underflow
    same(pow(2.0, 1024.0), INF, "pow(2, 1024)");
    same(pow(2.0, 1023.0), 8.98846567431158e307, "pow(2, 1023)");
    same(pow(2.0, -1074.0), 5e-324, "pow(2, -1074)");
    same(pow(2.0, -1075.0), 0.0, "pow(2, -1075)");
    same(pow(10.0, 308.0), 1e308, "pow(10, 308)");
    same(pow(10.0, -323.0), 1e-323, "pow(10, -323)");
    same(pow(MIN_SUB, 1.0), MIN_SUB, "pow(min, 1)");
    same(pow(4.0, 0.5), 2.0, "pow(4, 0.5)");
    same(pow(2.0, 10.0), 1024.0, "pow(2, 10)");
    same(pow(2.5, 3.0), 15.625, "pow(2.5, 3)");
}

// REQ: OA-MATH-011
#[test]
fn hypot_cbrt_powi_special_values() {
    for x in [0.0, -0.0, 1.0, -1.0, 1e300, MIN_SUB, f64::NAN, -f64::NAN] {
        same(hypot(INF, x), INF, "hypot(inf, x)");
        same(hypot(x, NINF), INF, "hypot(x, -inf)");
        same(hypot(NINF, x), INF, "hypot(-inf, x)");
        same(hypot(x, INF), INF, "hypot(x, inf)");
    }
    for n in nans() {
        nan(hypot(n, 1.0), "hypot(NaN, 1)");
        nan(hypot(2.0, n), "hypot(2, NaN)");
        nan(hypot(n, 0.0), "hypot(NaN, 0)");
    }
    same(hypot(0.0, 0.0), 0.0, "hypot(+0, +0)");
    same(hypot(-0.0, -0.0), 0.0, "hypot(-0, -0)");
    same(hypot(-0.0, 0.0), 0.0, "hypot(-0, +0)");
    same(hypot(-3.0, 0.0), 3.0, "hypot(-3, 0)");
    same(hypot(0.0, -MIN_SUB), MIN_SUB, "hypot(0, -min)");
    same(hypot(3.0, 4.0), 5.0, "hypot(3, 4)");
    same(hypot(-5.0, 12.0), 13.0, "hypot(-5, 12)");
    same(hypot(f64::MAX, f64::MAX), INF, "hypot(max, max) overflows");
    same(hypot(f64::MAX, 1.0), f64::MAX, "hypot(max, 1)");
    same(
        hypot(1e300, 1e300),
        1.4142135623730952e300,
        "hypot(1e300, 1e300) no intermediate overflow",
    );
    same(
        hypot(MIN_SUB, MIN_SUB),
        MIN_SUB,
        "hypot(min, min) = 1.41 units rounds to 1",
    );
    same(
        hypot(2.0 * MIN_SUB, 2.0 * MIN_SUB),
        3.0 * MIN_SUB,
        "hypot(2 min, 2 min) = 2.83 units rounds to 3",
    );
    same(hypot(3e-320, 4e-320), 5e-320, "hypot subnormal 3-4-5");

    same(cbrt(0.0), 0.0, "cbrt(+0)");
    same(cbrt(-0.0), -0.0, "cbrt(-0)");
    same(cbrt(INF), INF, "cbrt(inf)");
    same(cbrt(NINF), NINF, "cbrt(-inf)");
    for x in nans() {
        nan(cbrt(x), "cbrt(NaN)");
    }
    same(cbrt(27.0), 3.0, "cbrt(27)");
    same(cbrt(-27.0), -3.0, "cbrt(-27)");
    same(cbrt(1.0), 1.0, "cbrt(1)");
    same(cbrt(1e300), 1e100, "cbrt(1e300)");
    same(
        cbrt(MIN_SUB),
        1.7031839360032603e-108,
        "cbrt(min subnormal)",
    );
    same(cbrt(f64::MAX), 5.643803094122362e102, "cbrt(DBL_MAX)");
    same(cbrt(-1e-320), -2.1544266950262728e-107, "cbrt(-1e-320)");

    // powi: x^0 = 1 for everything; the chain is multiplications and one division
    for x in [0.0, -0.0, 2.0, INF, NINF, f64::NAN, 1e300] {
        same(powi(x, 0), 1.0, "powi(x, 0)");
    }
    same(powi(2.0, 10), 1024.0, "powi(2, 10)");
    same(powi(2.0, -2), 0.25, "powi(2, -2)");
    same(powi(-2.0, 3), -8.0, "powi(-2, 3)");
    same(powi(0.0, -1), INF, "powi(+0, -1)");
    same(powi(-0.0, -1), NINF, "powi(-0, -1)");
    same(powi(-0.0, -2), INF, "powi(-0, -2)");
    same(powi(0.0, 5), 0.0, "powi(+0, 5)");
    same(powi(INF, 3), INF, "powi(inf, 3)");
    same(powi(INF, -3), 0.0, "powi(inf, -3)");
    same(powi(2.0, i32::MIN), 0.0, "powi(2, i32::MIN)");
    same(powi(2.0, i32::MAX), INF, "powi(2, i32::MAX)");
    same(powi(1.0, i32::MIN), 1.0, "powi(1, i32::MIN)");
    same(powi(-1.0, i32::MAX), -1.0, "powi(-1, i32::MAX)");
    assert!(powi(f64::NAN, 3).is_nan() && powi(f64::NAN, -3).is_nan());
}

// REQ: OA-MATH-024
#[test]
fn powi_negative_powers_are_not_lost_to_the_opposite_overflow() {
    let bits = |x: f64| x.to_bits();
    assert_eq!(bits(powi(2.0, -1022)), 1u64 << 52, "2^-1022");
    assert_eq!(bits(powi(2.0, -1023)), 1u64 << 51, "2^-1023");
    assert_eq!(bits(powi(2.0, -1024)), 1u64 << 50, "2^-1024");
    assert_eq!(bits(powi(2.0, -1025)), 1u64 << 49, "2^-1025");
    assert_eq!(bits(powi(2.0, -1074)), 1, "2^-1074");
    // exactly half the smallest subnormal: a tie, which goes to the even neighbour, zero
    assert_eq!(bits(powi(2.0, -1075)), 0, "2^-1075");
    assert_eq!(bits(powi(-2.0, -1075)), 1u64 << 63, "(-2)^-1075");
    assert_eq!(bits(powi(-2.0, -1074)), 1, "(-2)^-1074");
    assert_eq!(bits(powi(-2.0, -1073)), (1u64 << 63) | 2, "(-2)^-1073");
    assert_eq!(bits(powi(0.5, 1074)), 1, "0.5^1074");
    assert_eq!(bits(powi(0.5, 1075)), 0, "0.5^1075");
    assert_eq!(bits(powi(4.0, -537)), 1, "4^-537 = 2^-1074");
    // large positive powers up to the overflow limit
    assert_eq!(bits(powi(2.0, 1023)), 0x7fe0_0000_0000_0000, "2^1023");
    assert_eq!(powi(2.0, 1024), INF, "2^1024");
    assert_eq!(bits(powi(0.5, -1023)), 0x7fe0_0000_0000_0000, "0.5^-1023");
    assert_eq!(powi(0.5, -1024), INF, "0.5^-1024");
    assert_eq!(
        bits(powi(4.0, 511)),
        0x7fd0_0000_0000_0000,
        "4^511 = 2^1022"
    );
    assert_eq!(powi(-2.0, 1024), INF, "(-2)^1024");
    assert_eq!(powi(-2.0, 1025), NINF, "(-2)^1025");
    assert_eq!(bits(powi(-2.0, 1023)), 0xffe0_0000_0000_0000, "(-2)^1023");
    assert_eq!(bits(powi(MIN_SUB, -1)), INF.to_bits(), "min subnormal ^ -1");
    let p512 = f64::from_bits(1535u64 << 52);
    assert_eq!(powi(f64::from_bits(511u64 << 52), -1), p512, "2^-512 ^ -1");
    assert_eq!(powi(f64::from_bits(511u64 << 52), -2), INF, "2^-512 ^ -2");
    assert_eq!(
        bits(powi(0.125, -341)),
        0x7fe0_0000_0000_0000,
        "8^341 = 2^1023"
    );
}

// REQ: OA-MATH-025
#[test]
fn powi_of_a_nan_is_one_for_zero_and_the_canonical_nan_otherwise() {
    for x in nans() {
        same(powi(x, 0), 1.0, "powi(NaN, 0)");
        for n in [1, -1, 2, 3, -4, i32::MAX, i32::MIN] {
            nan(powi(x, n), "powi(NaN, n)");
        }
    }
    nan(
        powi(f64::from_bits(0x7ff8_0000_0000_1234), 1),
        "powi(payload NaN, 1)",
    );
}

// REQ: OA-MATH-027
#[test]
fn hypot_just_below_a_subnormal_tie_rounds_down() {
    // A^2 - 2 B^2 = -1, so sqrt(2) A B = A sqrt(A^2 + 1) = u + 1/2 - 1/(8 A^2) + ...: just below
    // the half-way point between u and u + 1 (units of the operand's ulp)
    let (a, b) = (54_608_393u64, 38_613_965u64);
    let (u, v) = (a * a, a * b);
    assert!(
        u % 2 == 1 && (1u64 << 51..1u64 << 52).contains(&u),
        "u = {u}"
    );
    assert_eq!(2 * (b as i128) * (b as i128) - (a as i128) * (a as i128), 1);
    let x = f64::from_bits(v);
    assert_eq!(
        hypot(x, x).to_bits(),
        u,
        "hypot(x, x) for x = from_bits({v})"
    );
}

// REQ: OA-MATH-020
#[test]
fn every_nan_result_has_the_one_fixed_bit_pattern() {
    let mut inputs = nans().to_vec();
    inputs.extend([
        f64::from_bits(0x7fff_ffff_ffff_ffff),
        f64::from_bits(0xffff_ffff_ffff_ffff),
    ]);
    for x in inputs.iter().copied().chain([INF, NINF, 2.0, -2.0]) {
        for y in inputs
            .iter()
            .copied()
            .chain([INF, NINF, 2.0, -2.0, 0.5, -0.5])
        {
            for r in [atan2(x, y), hypot(x, y), pow(x, y)] {
                if r.is_nan() {
                    assert_eq!(r.to_bits(), CANON_NAN, "binary function at ({x:e}, {y:e})");
                }
            }
        }
        for r in [
            sin(x),
            cos(x),
            tan(x),
            atan(x),
            asin(x),
            acos(x),
            exp(x),
            ln(x),
            cbrt(x),
        ] {
            if r.is_nan() {
                assert_eq!(r.to_bits(), CANON_NAN, "unary function at {x:e}");
            }
        }
        for n in [1, -1, 2, -3, 1000, i32::MAX, i32::MIN] {
            let r = powi(x, n);
            if r.is_nan() {
                assert_eq!(r.to_bits(), CANON_NAN, "powi at ({x:e}, {n})");
            }
        }
        let (s, c) = sin_cos(x);
        for r in [s, c] {
            if r.is_nan() {
                assert_eq!(r.to_bits(), CANON_NAN, "sin_cos at {x:e}");
            }
        }
    }
    // the NaN inputs really did produce NaNs above
    nan(sin(inputs[0]), "sin(NaN)");
    nan(pow(inputs[1], 2.0), "pow(NaN, 2)");
    nan(atan2(inputs[2], 1.0), "atan2(NaN, 1)");
    nan(hypot(inputs[3], 1.0), "hypot(NaN, 1)");
    nan(powi(inputs[0], 1), "powi(NaN, 1)");
    nan(powi(inputs[2], -3), "powi(NaN, -3)");
    same(powi(inputs[3], 0), 1.0, "powi(NaN, 0)");
}
