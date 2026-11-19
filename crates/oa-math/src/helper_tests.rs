//! Tests of the helpers the fast paths rely on (exact products, bit-built tables and signs) and of
//! the fast paths on inputs near the places where they could go wrong. Compiled for tests only.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use crate::atan::{acos_fast, acos_slow, asin_fast, asin_slow, atan2_fast, atan2_slow};
use crate::atan::{atan_fast, atan_slow, two_prod_c, BASE_TAB};
use crate::consts::{ATAN_TAB, COS_TAB, LN_R, PIO2_DD, PI_DD, SIN_TAB};
use crate::dd::{self, two_prod, two_sqr};
use crate::exp::{exp_fast, exp_slow};
use crate::log::{ln_fast, ln_slow};
use crate::root::{hypot_fast, hypot_slow};
use crate::trig::{
    cos_fast, cos_sign, cos_slow, sin_cos_fast, sin_cos_slow, sin_fast, two_prod_cut,
};
use crate::trig::{sin_sign, sin_slow, tan_fast, tan_slow, SLOPE_TAB, START_TAB};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    /// A number in `0..n` (n below 2^32).
    fn below(&mut self, n: u64) -> u64 {
        ((self.next() >> 32) * n) >> 32
    }
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / 9_007_199_254_740_992.0
    }
    fn uni(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.unit()
    }
    /// A magnitude 2^e (1 + unit) with e uniform in `lo..=hi`, of either sign when `signed`.
    fn logu(&mut self, lo: i32, hi: i32, signed: bool) -> f64 {
        let e = lo + (self.next() & 0xffff) as i32 * (hi - lo + 1) / 65536;
        let v = (1.0 + self.unit()) * f64::from_bits(((e + 1023) as u64) << 52);
        if signed && self.next() & 1 == 1 {
            -v
        } else {
            v
        }
    }
    /// `p` moved by up to four units in the last place either way.
    fn near(&mut self, p: f64) -> f64 {
        let k = self.below(9) as i64 - 4;
        let bits = p.to_bits();
        f64::from_bits(if k >= 0 {
            bits.wrapping_add(k as u64)
        } else {
            bits.wrapping_sub(k.unsigned_abs())
        })
    }
}

/// Number of significant bits of a positive finite double.
fn bit_length(x: f64) -> u32 {
    let m = (x.to_bits() & 0x000f_ffff_ffff_ffff) | (1 << 52);
    53 - m.trailing_zeros()
}

// REQ: OA-MATH-029
#[test]
fn ln_table_multipliers_are_short() {
    // `ln` forms m r exactly from a 41-bit piece of m, which needs at most 12 bits in r
    for r in LN_R {
        assert!(bit_length(r) <= 12, "{r} has {} bits", bit_length(r));
    }
}

// REQ: OA-MATH-029
#[test]
fn square_and_cut_products_are_exact_or_close() {
    let mut rng = Rng(0x0123_4567_89ab_cdef);
    for _ in 0..200_000 {
        let a = rng.logu(-200, 200, true);
        let b = rng.logu(-200, 200, true);
        let s = two_sqr(a);
        let e = two_prod(a, a);
        assert_eq!(s.hi.to_bits(), e.hi.to_bits());
        assert_eq!(s.lo.to_bits(), e.lo.to_bits(), "two_sqr({a:e})");
        let c = two_prod_cut(a, b);
        let e = two_prod(a, b);
        assert_eq!(c.hi.to_bits(), e.hi.to_bits());
        assert!(
            (c.lo - e.lo).abs() <= c.hi.abs() * f64::from_bits((1023 - 77) << 52),
            "two_prod_cut({a:e}, {b:e})"
        );
    }
}

// REQ: OA-MATH-029
#[test]
fn product_with_a_multiple_of_a_sixteenth_is_exact() {
    let mut rng = Rng(0xfeed_face_0bad_f00d);
    for _ in 0..200_000 {
        let x = rng.logu(-100, 100, true);
        let c = rng.below(17) as f64 / 16.0;
        let (h, l) = two_prod_c(c, x);
        let e = two_prod(c, x);
        assert_eq!(h.to_bits(), e.hi.to_bits());
        assert_eq!(l.to_bits(), e.lo.to_bits(), "{c} * {x:e}");
    }
}

// REQ: OA-MATH-029
#[test]
fn quadrant_sign_bits_follow_the_quadrants() {
    for n in 0u64..4 {
        for neg_y in 0u64..2 {
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
            assert_eq!(cos_sign(n, neg_y), cos_neg as u64, "cos n={n} y={neg_y}");
            for neg_x in 0u64..2 {
                assert_eq!(
                    sin_sign(n, neg_y, neg_x),
                    (sin_neg != (neg_x == 1)) as u64,
                    "sin n={n} y={neg_y} x={neg_x}"
                );
            }
        }
    }
}

// REQ: OA-MATH-029
#[test]
fn bit_built_tables_hold_the_table_entries() {
    for j in 0..15 {
        assert_eq!(START_TAB[2 * j].hi.to_bits(), SIN_TAB[j].hi.to_bits());
        assert_eq!(START_TAB[2 * j + 1].lo.to_bits(), COS_TAB[j].lo.to_bits());
        assert_eq!(SLOPE_TAB[2 * j].hi.to_bits(), COS_TAB[j].hi.to_bits());
        assert_eq!(SLOPE_TAB[2 * j + 1].lo.to_bits(), SIN_TAB[j].lo.to_bits());
    }
    // the compile-time sums equal the same sums at run time (only + - * / are involved)
    for (j, a) in ATAN_TAB.iter().enumerate() {
        let want = [
            *a,
            dd::sub(PIO2_DD, *a),
            dd::sub(PI_DD, *a),
            dd::add(PIO2_DD, *a),
        ];
        for (kind, w) in want.iter().enumerate() {
            let got = BASE_TAB[kind * 17 + j];
            assert_eq!(got.hi.to_bits(), w.hi.to_bits(), "kind {kind} j {j}");
            assert_eq!(got.lo.to_bits(), w.lo.to_bits(), "kind {kind} j {j}");
        }
    }
}

/// Compares a fast path (when it answers) with the double-double path on `n` inputs.
fn equal1(
    name: &str,
    n: usize,
    fast: fn(f64) -> Option<f64>,
    slow: fn(f64) -> f64,
    gen: &mut dyn FnMut(&mut Rng) -> f64,
) {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15 ^ name.len() as u64);
    let mut taken = 0;
    for _ in 0..n {
        let x = gen(&mut rng);
        if let Some(a) = fast(x) {
            taken += 1;
            assert_eq!(a.to_bits(), slow(x).to_bits(), "{name}({x:e})");
        }
    }
    assert!(taken > n / 2, "{name}: fast path took {taken} of {n}");
}

fn equal2(
    name: &str,
    n: usize,
    fast: fn(f64, f64) -> Option<f64>,
    slow: fn(f64, f64) -> f64,
    gen: &mut dyn FnMut(&mut Rng) -> (f64, f64),
) {
    let mut rng = Rng(0x1234_5678_9abc_def1 ^ name.len() as u64);
    let mut taken = 0;
    for _ in 0..n {
        let (x, y) = gen(&mut rng);
        if let Some(a) = fast(x, y) {
            taken += 1;
            assert_eq!(a.to_bits(), slow(x, y).to_bits(), "{name}({x:e}, {y:e})");
        }
    }
    assert!(taken > n / 2, "{name}: fast path took {taken} of {n}");
}

const N_EDGE: usize = 300_000;

/// A point halfway between two table arguments j/16 (where the table index flips).
fn midpoint(rng: &mut Rng) -> f64 {
    (rng.below(33) as f64 + 0.5) / 16.0
}

// REQ: OA-MATH-021
#[test]
fn trig_fast_equals_slow_near_table_and_quadrant_edges() {
    let mut gen = |r: &mut Rng| -> f64 {
        match r.below(5) {
            0 => {
                let k = r.below(400) as f64 - 200.0;
                r.near(k * core::f64::consts::PI / 32.0)
            }
            1 => {
                let k = r.below(20_000) as f64;
                r.near(k * core::f64::consts::FRAC_PI_2)
            }
            2 => r.logu(-26, 18, true),
            3 => {
                let m = midpoint(r);
                r.near(m)
            }
            _ => r.uni(-1000.0, 1000.0),
        }
    };
    equal1("sin", N_EDGE, sin_fast, sin_slow, &mut gen);
    equal1("cos", N_EDGE, cos_fast, cos_slow, &mut gen);
    equal1("tan", N_EDGE, tan_fast, tan_slow, &mut gen);
    let mut rng = Rng(0xabcd_ef01_2345_6789);
    for _ in 0..N_EDGE {
        let x = gen(&mut rng);
        if let Some((s, c)) = sin_cos_fast(x) {
            let (s2, c2) = sin_cos_slow(x);
            assert_eq!(
                (s.to_bits(), c.to_bits()),
                (s2.to_bits(), c2.to_bits()),
                "{x:e}"
            );
        }
    }
}

// REQ: OA-MATH-021
#[test]
fn exp_and_ln_fast_equal_slow_near_reduction_edges() {
    equal1(
        "exp",
        N_EDGE,
        exp_fast,
        exp_slow,
        &mut |r| match r.below(4) {
            0 => r.uni(-700.0, 700.0),
            1 => r.uni(-1.0, 1.0),
            2 => {
                let k = r.below(2000) as f64 - 1000.0;
                r.near(k * core::f64::consts::LN_2 / 64.0)
            }
            _ => r.logu(-40, 9, true),
        },
    );
    equal1("ln", N_EDGE, ln_fast, ln_slow, &mut |r| match r.below(4) {
        0 => r.logu(-1000, 1000, false),
        1 => {
            let k = r.below(64) as f64 / 64.0;
            r.near(1.0 + k)
        }
        2 => r.uni(0.5, 2.0),
        _ => r.near(1.0),
    });
}

// REQ: OA-MATH-021
#[test]
fn atan_family_fast_equals_slow_near_table_and_swap_edges() {
    let mut tan_like = |r: &mut Rng| -> f64 {
        match r.below(5) {
            0 => {
                let m = midpoint(r);
                r.near(m)
            }
            1 => {
                let m = midpoint(r);
                1.0 / r.near(m)
            }
            2 => r.logu(-26, 62, true),
            3 => r.uni(-3.0, 3.0),
            _ => {
                let j = r.below(17) as f64 / 16.0;
                r.near(j)
            }
        }
    };
    equal1("atan", N_EDGE, atan_fast, atan_slow, &mut tan_like);
    let mut unit = |r: &mut Rng| -> f64 {
        match r.below(6) {
            0 => r.uni(-1.0, 1.0),
            1 => {
                let m = midpoint(r) / 2.0;
                r.near(m)
            }
            2 => 1.0 - r.logu(-53, -1, false) * 0.5,
            3 => r.logu(-27, -1, true),
            4 => r.near(core::f64::consts::FRAC_1_SQRT_2),
            _ => -r.uni(0.0, 1.0),
        }
    };
    equal1("asin", N_EDGE, asin_fast, asin_slow, &mut unit);
    equal1("acos", N_EDGE, acos_fast, acos_slow, &mut unit);
    equal2(
        "atan2",
        N_EDGE,
        atan2_fast,
        atan2_slow,
        &mut |r| match r.below(6) {
            0 => (r.uni(-10.0, 10.0), r.uni(-10.0, 10.0)),
            1 => {
                let m = midpoint(r);
                let x = r.logu(-5, 5, true);
                (x * m, x)
            }
            2 => {
                let m = midpoint(r);
                let x = r.logu(-5, 5, true);
                (x, x * m)
            }
            3 => (r.logu(-400, 400, true), r.logu(-400, 400, true)),
            4 => (r.logu(-50, 50, true), r.logu(-50, 50, true)),
            _ => (r.logu(-330, 330, true), r.logu(-330, 330, true)),
        },
    );
}

// REQ: OA-MATH-021
#[test]
fn hypot_fast_equals_slow_around_the_unscaled_range() {
    equal2(
        "hypot",
        N_EDGE,
        hypot_fast,
        hypot_slow,
        &mut |r| match r.below(5) {
            0 => (r.uni(-10.0, 10.0), r.uni(-10.0, 10.0)),
            1 => (r.logu(-400, 400, true), r.logu(-400, 400, true)),
            2 => {
                let x = r.logu(-5, 5, true);
                (x, x * r.uni(0.0, 1.0))
            }
            3 => {
                // across the edges of the unscaled range (1e-114 and 1e150)
                let e = if r.next() & 1 == 0 { -378 } else { 498 };
                (r.logu(e - 3, e + 3, true), r.logu(e - 70, e + 3, true))
            }
            _ => (r.logu(-30, 30, true), r.logu(-30, 30, true)),
        },
    );
}

// REQ: OA-MATH-029
#[test]
fn pow2_returns_exact_ieee_encodings_at_range_edges() {
    use crate::util::pow2;

    let cases: [(i32, u64); 12] = [
        (-1076, 0),
        (-1075, 0),
        (-1074, 0x0000_0000_0000_0001),
        (-1073, 0x0000_0000_0000_0002),
        (-1023, 0x0008_0000_0000_0000),
        (-1022, 0x0010_0000_0000_0000),
        (-1021, 0x0020_0000_0000_0000),
        (1022, 0x7fd0_0000_0000_0000),
        (1023, 0x7fe0_0000_0000_0000),
        (1024, 0x7ff0_0000_0000_0000),
        (1025, 0x7ff0_0000_0000_0000),
        (2048, 0x7ff0_0000_0000_0000),
    ];
    for (exponent, expected_bits) in cases {
        assert_eq!(pow2(exponent).to_bits(), expected_bits, "2^{exponent}");
    }
}

// REQ: OA-MATH-029
#[test]
fn frexp1_reconstructs_subnormals_and_exact_powers() {
    use crate::util::frexp1;

    let cases = [
        (f64::from_bits(1), -1074, 1.0),
        (f64::from_bits(2), -1073, 1.0),
        (f64::from_bits(3), -1073, 1.5),
        (f64::MIN_POSITIVE, -1022, 1.0),
        (
            f64::from_bits(0x0010_0000_0000_0001),
            -1022,
            1.0 + f64::EPSILON,
        ),
        (f64::from_bits(0x3ff0_0000_0000_0000), 0, 1.0),
        (f64::from_bits(0x7fe0_0000_0000_0000), 1023, 1.0),
    ];
    for (x, expected_exponent, expected_mantissa) in cases {
        let (exponent, mantissa) = frexp1(x);
        assert_eq!(exponent, expected_exponent, "frexp1({x:e}) exponent");
        assert_eq!(
            mantissa.to_bits(),
            expected_mantissa.to_bits(),
            "frexp1({x:e}) mantissa"
        );
        assert_eq!(
            (mantissa * crate::util::pow2(exponent)).to_bits(),
            x.to_bits(),
            "frexp1({x:e}) reconstruction"
        );
    }
}

// REQ: OA-MATH-029
#[test]
fn exponent_reports_subnormal_and_adjacent_normal_binades() {
    use crate::util::exponent;

    let cases = [
        (f64::from_bits(0x0000_0000_0000_0001), -1074),
        (f64::from_bits(0x0000_0000_0000_0002), -1073),
        (f64::from_bits(0x0000_0000_0000_0003), -1073),
        (f64::from_bits(0x000f_ffff_ffff_ffff), -1023),
        (f64::MIN_POSITIVE, -1022),
        (f64::from_bits(0x0010_0000_0000_0001), -1022),
        (f64::from_bits(0x3ff0_0000_0000_0000), 0),
        (f64::from_bits(0x4000_0000_0000_0000), 1),
        (f64::from_bits(0x4000_0000_0000_0001), 1),
        (f64::from_bits(0x7fe0_0000_0000_0000), 1023),
    ];
    for (x, expected) in cases {
        assert_eq!(exponent(x), expected, "exponent({x:e})");
    }
}

// REQ: OA-MATH-026
#[test]
fn scale_round_returns_exact_dyadic_boundary_bits() {
    use crate::dd::dd;
    use crate::util::scale_round;

    let cases = [
        (dd(1.5, 0.0), 0, 0x3ff8_0000_0000_0000), // ordinary exact value
        (dd(1.0, 0.0), -1022, 0x0010_0000_0000_0000), // least normal
        (dd(1.0, 0.0), -1074, 0x0000_0000_0000_0001), // least subnormal
        (dd(f64::from_bits(1), 0.0), 27, 0x0000_0000_0800_0000), // subnormal input scaled to 2^-1047
        (dd(1.0, 0.0), -1075, 0x0000_0000_0000_0000),            // half least subnormal, even zero
        (dd(3.0, 0.0), -1075, 0x0000_0000_0000_0002),            // tie to even two units
        (dd(5.0, 0.0), -1075, 0x0000_0000_0000_0002),            // tie to even two units
        (dd(1.0, 0.0), 1023, 0x7fe0_0000_0000_0000),             // largest finite binade power
        (dd(1.0, 0.0), 1024, 0x7ff0_0000_0000_0000),             // overflow
        // These distinguish e+k from the mutant e-k and e*k cutoff predicates.
        (
            dd(f64::from_bits(0x0170_0000_0000_0000), 0.0),
            101,
            0x07c0_0000_0000_0000,
        ), // 2^-1000 * 2^101 = 2^-899
        (
            dd(f64::from_bits(0x0170_0000_0000_0000), 0.0),
            2,
            0x0190_0000_0000_0000,
        ), // 2^-1000 * 2^2 = 2^-998
        // Exact midpoint between max finite and infinity; nearest-even overflows.
        (
            dd(
                f64::from_bits(0x7fe0_0000_0000_0000),
                -f64::from_bits(0x7c80_0000_0000_0000),
            ),
            1,
            0x7ff0_0000_0000_0000,
        ),
        // Just below the overflow midpoint, normalized about the lower neighbour.
        (
            dd(
                f64::from_bits(0x7fdf_ffff_ffff_ffff),
                f64::from_bits(0x7c80_0000_0000_0000) - f64::from_bits(0x7940_0000_0000_0000),
            ),
            1,
            0x7fef_ffff_ffff_ffff,
        ),
    ];
    for (value, scale, expected_bits) in cases {
        assert_eq!(
            (value.hi + value.lo).to_bits(),
            value.hi.to_bits(),
            "normalized input"
        );
        assert_eq!(
            scale_round(value, scale).to_bits(),
            expected_bits,
            "scale_round({:?}, {scale})",
            value
        );
    }
}
