use oa_math::{cbrt, hypot, pow, powi};

// REQ: OA-MATH-010
#[test]
fn pow_negative_unit_tracks_integer_parity_at_binary64_integer_cutoff() {
    let cutoff: f64 = 9_007_199_254_740_992.0; // 2^53; all representable integers here are even
    let below = f64::from_bits(cutoff.to_bits() - 1); // 2^53 - 1, the last odd integer
    let above = f64::from_bits(cutoff.to_bits() + 1); // 2^53 + 2
    assert_eq!(below, cutoff - 1.0);
    assert_eq!(pow(-1.0, below).to_bits(), (-1.0f64).to_bits());
    assert_eq!(pow(-1.0, cutoff).to_bits(), 1.0f64.to_bits());
    assert_eq!(pow(-1.0, above).to_bits(), 1.0f64.to_bits());
    assert_eq!(pow(-1.0, -below).to_bits(), (-1.0f64).to_bits());
}

// REQ: OA-MATH-016
#[test]
fn powi_i32_minimum_exponent_keeps_the_negative_power_scale() {
    assert_eq!(powi(2.0, i32::MIN), 0.0);
    assert_eq!(powi(0.5, i32::MIN), f64::INFINITY);
    assert_eq!(powi(-1.0, i32::MIN), 1.0);
    assert_eq!(powi(-1.0, i32::MAX), -1.0);
    assert_eq!(powi(f64::INFINITY, -1).to_bits(), 0.0f64.to_bits());
    assert_eq!(powi(f64::NEG_INFINITY, -1).to_bits(), (-0.0f64).to_bits());
}

// REQ: OA-MATH-005
#[test]
fn hypot_diagonal_is_stable_across_plain_fast_and_scaled_boundaries() {
    // Each expected value was rounded from sqrt(2) times the exact binary64 input using
    // 150-decimal-digit mpmath; the expectations do not use this crate's square-root implementation.
    let cases = [
        (1.0e-114, 0x284b_dc86_6646_826b),
        (
            f64::from_bits(1.0e-114f64.to_bits() - 1),
            0x284b_dc86_6646_826a,
        ),
        (1.0e150, 0x5f1b_a66e_c32a_cb5f),
        (
            f64::from_bits(1.0e150f64.to_bits() - 1),
            0x5f1b_a66e_c32a_cb5e,
        ),
    ];
    for (x, expected) in cases {
        assert_eq!(hypot(x, x).to_bits(), expected, "hypot({x:e}, {x:e})");
    }
    // An asymmetric scaled case pins magnitude ordering and has an independent mpmath result.
    assert_eq!(hypot(3.0e200, 4.0e200).to_bits(), 0x699a_20df_0dcd_3af0);
}

// REQ: OA-MATH-005
#[test]
fn cbrt_exact_cubes_straddle_normal_and_subnormal_inputs() {
    let cases = [
        (8.0, 2.0),
        (-8.0, -2.0),
        (f64::MIN_POSITIVE, f64::from_bits(0x2aa4_28a2_f98d_728b)),
        (f64::from_bits(1), f64::from_bits(0x2990_0000_0000_0000)),
    ];
    for (x, expected) in cases {
        let got = cbrt(x);
        assert_eq!(got.to_bits(), expected.to_bits(), "cbrt({x:e})");
    }
}
