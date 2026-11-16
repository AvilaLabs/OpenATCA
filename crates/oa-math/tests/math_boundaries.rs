//! Independent high-precision cases at algorithm and range boundaries.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_math::{acos, asin, atan, atan2, exp, ln, pow};

fn same(got: f64, want: f64, what: &str) {
    assert_eq!(
        got.to_bits(),
        want.to_bits(),
        "{what}: got {got:e}, want {want:e}"
    );
}

// REQ: OA-MATH-002
#[test]
fn inverse_trig_small_and_large_transition_values() {
    // Correctly rounded references generated from the exact binary inputs with 200-digit mpmath.
    let tiny = 2.0f64.powi(-27);
    for (x, want) in [
        (f64::from_bits(tiny.to_bits() - 1), 7.450580596923827e-9),
        (tiny, 7.450580596923828e-9),
        (f64::from_bits(tiny.to_bits() + 1), 7.45058059692383e-9),
    ] {
        same(atan(x), want, "atan around tiny cutoff");
        same(asin(x), want, "asin around tiny cutoff");
        same(acos(x), 1.570796319344316, "acos around tiny cutoff");
    }
    let large = 2.0f64.powi(60);
    for x in [large, f64::from_bits(large.to_bits() + 1)] {
        same(
            atan(x),
            std::f64::consts::FRAC_PI_2,
            "atan around large cutoff",
        );
    }
}

// REQ: OA-MATH-008
#[test]
fn atan2_ratio_cutoffs_match_independent_references() {
    let tiny = 2.0f64.powi(-60);
    let huge = 2.0f64.powi(60);
    let cases = [
        (tiny, 1.0, 8.673617379884035e-19),
        (
            f64::from_bits(tiny.to_bits() - 1),
            1.0,
            8.6736173798840345e-19,
        ),
        (
            f64::from_bits(tiny.to_bits() + 1),
            1.0,
            8.673617379884037e-19,
        ),
        (huge, 1.0, std::f64::consts::FRAC_PI_2),
        (
            f64::from_bits(huge.to_bits() - 1),
            1.0,
            std::f64::consts::FRAC_PI_2,
        ),
        (
            f64::from_bits(huge.to_bits() + 1),
            1.0,
            std::f64::consts::FRAC_PI_2,
        ),
        (1.0, tiny, std::f64::consts::FRAC_PI_2),
        (1.0, huge, 8.673617379884035e-19),
        (huge, -1.0, std::f64::consts::FRAC_PI_2),
        (tiny, -1.0, std::f64::consts::PI),
    ];
    for (y, x, want) in cases {
        same(atan2(y, x), want, "atan2 around ratio cutoff");
    }
}

// REQ: OA-MATH-003
#[test]
fn exp_double_double_and_subnormal_boundaries_match_independent_references() {
    // Correctly rounded references generated from the exact binary inputs with 200-digit mpmath.
    // |x| >= 700 uses exp_dd; negative values exercise gradual underflow and its rounding ties.
    let cases = [
        (700.0, 1.0142320547350045e304),
        (700.5, 1.6721859620674984e304),
        (704.0, 5.5375193892845935e305),
        (709.0, 8.218407461554972e307),
        (709.5, 1.3549863193146328e308),
        (-700.0, 9.85967654375977e-305),
        (-704.0, 1.8058627513522668e-306),
        (-709.0, 1.216780750623423e-308),
        (-744.0, 1.0e-323),
        (-745.0, 5.0e-324),
        (-745.1, 5.0e-324),
        (-745.13, 5.0e-324),
    ];
    for (x, want) in cases {
        same(exp(x), want, "exp double-double/subnormal range");
    }
}

// REQ: OA-MATH-003
#[test]
fn exp_fast_reduction_bin_midpoints_match_independent_references() {
    // Correctly rounded references generated from exact binary inputs with 200-digit mpmath.
    // The inputs flank half-bin arguments, where the reduced Taylor argument is largest.
    let cases = [
        (f64::from_bits(0x3f90_a2b2_3f3b_ab72), 1.016378314910953),
        (f64::from_bits(0x3f90_a2b2_3f3b_ab73), 1.016378314910953),
        (f64::from_bits(0x3f90_a2b2_3f3b_ab74), 1.016378314910953),
        (f64::from_bits(0x3fd5_d589_f2fe_5106), 1.4065759938190154),
        (f64::from_bits(0x3fd5_d589_f2fe_5107), 1.4065759938190154),
        (f64::from_bits(0x3fd5_d589_f2fe_5108), 1.4065759938190154),
        (f64::from_bits(0x3fe6_5a9f_84f8_2e62), 2.0108598022256055),
        (f64::from_bits(0x3fe6_5a9f_84f8_2e63), 2.0108598022256055),
        (f64::from_bits(0x3fe6_5a9f_84f8_2e64), 2.010859802225606),
        (f64::from_bits(0x4026_3108_c75a_1935), 65891.85399932848),
        (f64::from_bits(0x4026_3108_c75a_1936), 65891.8539993286),
        (f64::from_bits(0x4026_3108_c75a_1937), 65891.85399932871),
        (f64::from_bits(0x4056_2e9b_b806_35d7), 3.4213006652374425e38),
        (f64::from_bits(0x4056_2e9b_b806_35d8), 3.421300665237491e38),
        (f64::from_bits(0x4056_2e9b_b806_35d9), 3.421300665237539e38),
        (f64::from_bits(0x4076_2e59_2d3d_38e8), 1.348061100114054e154),
        (
            f64::from_bits(0x4076_2e59_2d3d_38e9),
            1.3480611001141304e154,
        ),
        (
            f64::from_bits(0x4076_2e59_2d3d_38ea),
            1.3480611001142072e154,
        ),
        (
            f64::from_bits(0x4085_97e4_5fca_a892),
            1.2348912812516068e300,
        ),
        (
            f64::from_bits(0x4085_97e4_5fca_a893),
            1.2348912812517471e300,
        ),
        (
            f64::from_bits(0x4085_97e4_5fca_a894),
            1.2348912812518875e300,
        ),
    ];
    for (x, want) in cases {
        same(exp(x), want, "exp fast reduction midpoint");
    }
}

// REQ: OA-MATH-004
#[test]
fn power_of_two_fractional_exponents_exercise_exp_core_paths() {
    // Correctly rounded references for 2^y, generated with 200-digit mpmath. Positive and
    // negative arguments cover the fast exp core and the double-double exp fallback used by pow.
    let cases = [
        (100.5, 1.7927286711931566e30),
        (-100.5, 5.578088954947358e-31),
        (1022.5, 6.355805030768232e307),
        (-1022.5, 1.5733648139913585e-308),
        (-1023.5, 7.866824069956793e-309),
    ];
    for (y, want) in cases {
        same(pow(2.0, y), want, "pow(2, fractional exponent)");
    }
}

// REQ: OA-MATH-003
#[test]
fn logarithm_reduction_bins_and_normal_boundary_match_independent_references() {
    // Cases around table-bin edges in ln_fast_core and the normal/subnormal input boundary.
    // Correctly rounded answers were generated from exact binary inputs with 200-digit mpmath.
    let cases = [
        (0.75, -0.2876820724517809),
        (f64::from_bits(0.75f64.to_bits() - 1), -0.28768207245178107),
        (f64::from_bits(0.75f64.to_bits() + 1), -0.2876820724517808),
        (0.765625, -0.26706278524904525),
        (
            f64::from_bits(0.765625f64.to_bits() - 1),
            -0.26706278524904536,
        ),
        (
            f64::from_bits(0.765625f64.to_bits() + 1),
            -0.2670627852490451,
        ),
        (
            f64::from_bits(1.0f64.to_bits() - 1),
            -1.1102230246251565e-16,
        ),
        (1.0, 0.0),
        (f64::from_bits(1.0f64.to_bits() + 1), 2.2204460492503128e-16),
        (1.484375, 0.394993808240869),
        (
            f64::from_bits(1.484375f64.to_bits() - 1),
            0.3949938082408688,
        ),
        (
            f64::from_bits(1.484375f64.to_bits() + 1),
            0.3949938082408691,
        ),
        (1.5, 0.4054651081081644),
        (f64::from_bits(1.5f64.to_bits() - 1), 0.4054651081081642),
        (f64::from_bits(1.5f64.to_bits() + 1), 0.40546510810816455),
        (
            f64::from_bits(f64::MIN_POSITIVE.to_bits() - 1),
            -708.3964185322641,
        ),
        (f64::MIN_POSITIVE, -708.3964185322641),
        (
            f64::from_bits(f64::MIN_POSITIVE.to_bits() + 1),
            -708.3964185322641,
        ),
    ];
    for (x, want) in cases {
        same(ln(x), want, "ln reduction/range boundary");
    }
}
