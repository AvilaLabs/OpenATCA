//! Order, sign, copysign, min, max and clamp with every NaN and every signed zero defined.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use core::cmp::Ordering;
use core::hint::black_box;
use oa_math::{
    clamp_f32, clamp_f64, cmp_canon_f32, cmp_canon_f64, copysign_canon_f32, copysign_canon_f64,
    is_sign_negative_canon_f32, is_sign_negative_canon_f64, max_f32, max_f64, min_f32, min_f64,
    signum_canon_f32, signum_canon_f64,
};

const CANON64: u64 = 0x7ff8_0000_0000_0000;
const CANON32: u32 = 0x7fc0_0000;

/// Every kind of f64 the functions must tell apart: both infinities, ordinary values of both
/// signs, both zeros, and positive, negative and payload NaNs (made at run time and by bits).
fn all64() -> Vec<f64> {
    let zero = black_box(0.0f64);
    vec![
        f64::NEG_INFINITY,
        -2.5,
        -f64::MIN_POSITIVE,
        -0.0,
        0.0,
        f64::MIN_POSITIVE,
        // the smallest subnormals, both signs: not zero, below the smallest normal
        f64::from_bits(1),
        -f64::from_bits(1),
        3.0,
        f64::INFINITY,
        f64::NAN,
        -f64::NAN,
        f64::from_bits(0x7ff8_0000_0000_1234),
        f64::from_bits(0xfff8_0000_0000_1234),
        f64::from_bits(0x7ff0_0000_0000_0001),
        zero / black_box(0.0),
        -(zero / black_box(0.0)),
    ]
}

fn all32() -> Vec<f32> {
    let zero = black_box(0.0f32);
    vec![
        f32::NEG_INFINITY,
        -2.5,
        -f32::MIN_POSITIVE,
        -0.0,
        0.0,
        f32::MIN_POSITIVE,
        f32::from_bits(1),
        -f32::from_bits(1),
        3.0,
        f32::INFINITY,
        f32::NAN,
        -f32::NAN,
        f32::from_bits(0x7fc0_1234),
        f32::from_bits(0xffc0_1234),
        f32::from_bits(0x7f80_0001),
        zero / black_box(0.0),
        -(zero / black_box(0.0)),
    ]
}

// REQ: OA-MATH-035
#[test]
fn the_canonical_total_order_puts_one_nan_above_everything_and_the_zeros_apart() {
    let all = all64();
    for &a in &all {
        for &b in &all {
            let got = cmp_canon_f64(a, b);
            let want = match (a.is_nan(), b.is_nan()) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                // -0.0 < +0.0 in the total order, everything else as the numbers compare
                (false, false) => a.partial_cmp(&b).unwrap().then(
                    (a.is_sign_negative() as u8)
                        .cmp(&(b.is_sign_negative() as u8))
                        .reverse(),
                ),
            };
            assert_eq!(got, want, "{:#018x} vs {:#018x}", a.to_bits(), b.to_bits());
            assert_eq!(cmp_canon_f64(b, a), got.reverse());
        }
    }
    assert_eq!(cmp_canon_f64(-0.0, 0.0), Ordering::Less);
    assert_eq!(cmp_canon_f64(f64::INFINITY, f64::NAN), Ordering::Less);
    // sorting a slice with a negative and a payload NaN in it is the same on every machine
    let mut v = all64();
    v.sort_by(|a, b| cmp_canon_f64(*a, *b));
    let nans = v.iter().filter(|x| x.is_nan()).count();
    assert!(nans >= 6 && v[v.len() - nans..].iter().all(|x| x.is_nan()));
    assert!(v[..v.len() - nans].windows(2).all(|w| w[0] <= w[1]));
}

// REQ: OA-MATH-035
#[test]
fn the_single_precision_order_agrees() {
    let all = all32();
    for &a in &all {
        for &b in &all {
            let want = match (a.is_nan(), b.is_nan()) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => a.partial_cmp(&b).unwrap().then(
                    (a.is_sign_negative() as u8)
                        .cmp(&(b.is_sign_negative() as u8))
                        .reverse(),
                ),
            };
            assert_eq!(cmp_canon_f32(a, b), want);
        }
    }
}

// REQ: OA-MATH-035
#[test]
fn the_sign_after_canon_nan_is_false_for_every_nan_and_true_for_negative_zero() {
    for x in all64() {
        let want = !x.is_nan() && x.is_sign_negative();
        assert_eq!(is_sign_negative_canon_f64(x), want, "{:#018x}", x.to_bits());
    }
    for x in all32() {
        let want = !x.is_nan() && x.is_sign_negative();
        assert_eq!(is_sign_negative_canon_f32(x), want, "{:#010x}", x.to_bits());
    }
    assert!(is_sign_negative_canon_f64(-0.0));
    assert!(!is_sign_negative_canon_f64(0.0));
}

// REQ: OA-MATH-035
#[test]
fn copysign_takes_the_sign_after_canon_nan_and_a_nan_magnitude_is_the_one_nan() {
    for x in all64() {
        for s in all64() {
            let got = copysign_canon_f64(x, s);
            if x.is_nan() {
                assert_eq!(got.to_bits(), CANON64);
            } else {
                assert_eq!(got.abs().to_bits(), x.abs().to_bits());
                assert_eq!(got.is_sign_negative(), !s.is_nan() && s.is_sign_negative());
            }
        }
    }
    for x in all32() {
        for s in all32() {
            let got = copysign_canon_f32(x, s);
            if x.is_nan() {
                assert_eq!(got.to_bits(), CANON32);
            } else {
                assert_eq!(got.abs().to_bits(), x.abs().to_bits());
                assert_eq!(got.is_sign_negative(), !s.is_nan() && s.is_sign_negative());
            }
        }
    }
    assert_eq!(copysign_canon_f64(180.0, -0.0), -180.0);
    assert_eq!(copysign_canon_f64(1.0, f64::NAN), 1.0);
}

// REQ: OA-MATH-036
#[test]
fn min_and_max_ignore_a_nan_and_order_the_zeros() {
    let all = all64();
    for &a in &all {
        for &b in &all {
            let (lo, hi) = (min_f64(a, b), max_f64(a, b));
            match (a.is_nan(), b.is_nan()) {
                (true, true) => {
                    assert_eq!(lo.to_bits(), CANON64);
                    assert_eq!(hi.to_bits(), CANON64);
                }
                (true, false) => {
                    assert_eq!(lo.to_bits(), b.to_bits());
                    assert_eq!(hi.to_bits(), b.to_bits());
                }
                (false, true) => {
                    assert_eq!(lo.to_bits(), a.to_bits());
                    assert_eq!(hi.to_bits(), a.to_bits());
                }
                (false, false) => {
                    assert!(lo <= a && lo <= b && (lo == a || lo == b));
                    assert!(hi >= a && hi >= b && (hi == a || hi == b));
                }
            }
        }
    }
    // the two zeros, in both argument orders
    for (a, b) in [(-0.0f64, 0.0f64), (0.0, -0.0), (-0.0, -0.0), (0.0, 0.0)] {
        let any_neg = a.is_sign_negative() || b.is_sign_negative();
        let all_neg = a.is_sign_negative() && b.is_sign_negative();
        assert_eq!(min_f64(a, b).is_sign_negative(), any_neg, "{a} {b}");
        assert_eq!(max_f64(a, b).is_sign_negative(), all_neg, "{a} {b}");
    }
    assert_eq!(min_f64(f64::NEG_INFINITY, f64::INFINITY), f64::NEG_INFINITY);
    assert_eq!(max_f64(f64::NEG_INFINITY, f64::INFINITY), f64::INFINITY);
}

// REQ: OA-MATH-036
#[test]
fn the_single_precision_min_and_max_agree() {
    let all = all32();
    for &a in &all {
        for &b in &all {
            let (lo, hi) = (min_f32(a, b), max_f32(a, b));
            match (a.is_nan(), b.is_nan()) {
                (true, true) => {
                    assert_eq!(lo.to_bits(), CANON32);
                    assert_eq!(hi.to_bits(), CANON32);
                }
                (true, false) => {
                    assert_eq!((lo.to_bits(), hi.to_bits()), (b.to_bits(), b.to_bits()))
                }
                (false, true) => {
                    assert_eq!((lo.to_bits(), hi.to_bits()), (a.to_bits(), a.to_bits()))
                }
                (false, false) => {
                    assert!(lo <= a && lo <= b && (lo == a || lo == b));
                    assert!(hi >= a && hi >= b && (hi == a || hi == b));
                }
            }
        }
    }
    for (a, b) in [(-0.0f32, 0.0f32), (0.0, -0.0), (-0.0, -0.0), (0.0, 0.0)] {
        let any_neg = a.is_sign_negative() || b.is_sign_negative();
        let all_neg = a.is_sign_negative() && b.is_sign_negative();
        assert_eq!(min_f32(a, b).is_sign_negative(), any_neg);
        assert_eq!(max_f32(a, b).is_sign_negative(), all_neg);
    }
}

// REQ: OA-MATH-036
#[test]
fn clamp_is_total_and_follows_min_and_max() {
    let all = all64();
    for &x in &all {
        for &lo in &all {
            for &hi in &all {
                let got = clamp_f64(x, lo, hi);
                if x.is_nan() {
                    assert_eq!(got.to_bits(), CANON64);
                    continue;
                }
                assert_eq!(got.to_bits(), min_f64(max_f64(x, lo), hi).to_bits());
                if !lo.is_nan() && !hi.is_nan() && lo <= hi {
                    assert!(got >= lo && got <= hi, "{x} {lo} {hi}: {got}");
                }
            }
        }
    }
    // lo > hi does not panic; the result is hi
    assert_eq!(clamp_f64(5.0, 3.0, 1.0), 1.0);
    // the zeros: the lower bound of -0.0 and 0.0 is the larger, 0.0
    assert!(!clamp_f64(-1.0, 0.0, 1.0).is_sign_negative());
    assert!(clamp_f64(-0.0, -1.0, 1.0).is_sign_negative());
    let all32 = all32();
    for &x in &all32 {
        for &lo in &all32 {
            for &hi in &all32 {
                let got = clamp_f32(x, lo, hi);
                if x.is_nan() {
                    assert_eq!(got.to_bits(), CANON32);
                } else {
                    assert_eq!(got.to_bits(), min_f32(max_f32(x, lo), hi).to_bits());
                }
            }
        }
    }
}

// REQ: OA-MATH-036
#[test]
fn the_smallest_subnormals_are_ordered_and_extreme_like_any_other_number() {
    let (tiny, ntiny) = (f64::from_bits(1), -f64::from_bits(1));
    assert!(tiny > 0.0 && tiny < f64::MIN_POSITIVE);
    assert_eq!(min_f64(tiny, 0.0).to_bits(), 0.0f64.to_bits());
    assert_eq!(max_f64(tiny, 0.0).to_bits(), tiny.to_bits());
    assert_eq!(min_f64(ntiny, -0.0).to_bits(), ntiny.to_bits());
    assert_eq!(max_f64(ntiny, -0.0).to_bits(), (-0.0f64).to_bits());
    assert_eq!(max_f64(ntiny, 0.0).to_bits(), 0.0f64.to_bits());
    assert_eq!(cmp_canon_f64(ntiny, -0.0), Ordering::Less);
    assert_eq!(cmp_canon_f64(0.0, tiny), Ordering::Less);
    assert_eq!(clamp_f64(tiny, 0.0, 1.0).to_bits(), tiny.to_bits());
    assert_eq!(clamp_f64(ntiny, 0.0, 1.0).to_bits(), 0.0f64.to_bits());
    assert_eq!(copysign_canon_f64(tiny, -1.0).to_bits(), ntiny.to_bits());
    assert!(is_sign_negative_canon_f64(ntiny) && !is_sign_negative_canon_f64(tiny));
    let (tiny, ntiny) = (f32::from_bits(1), -f32::from_bits(1));
    assert!(tiny > 0.0 && tiny < f32::MIN_POSITIVE);
    assert_eq!(min_f32(tiny, 0.0).to_bits(), 0.0f32.to_bits());
    assert_eq!(max_f32(ntiny, -0.0).to_bits(), (-0.0f32).to_bits());
    assert_eq!(cmp_canon_f32(ntiny, -0.0), Ordering::Less);
    assert_eq!(clamp_f32(ntiny, 0.0, 1.0).to_bits(), 0.0f32.to_bits());
    assert_eq!(copysign_canon_f32(tiny, -1.0).to_bits(), ntiny.to_bits());
    assert!(is_sign_negative_canon_f32(ntiny) && !is_sign_negative_canon_f32(tiny));
}

// REQ: OA-MATH-035
#[test]
fn signum_is_one_with_the_sign_of_the_input_and_the_one_nan_for_any_nan() {
    for x in all64() {
        let got = signum_canon_f64(x);
        if x.is_nan() {
            assert_eq!(got.to_bits(), CANON64, "{:#018x}", x.to_bits());
        } else {
            let want = if x.is_sign_negative() { -1.0 } else { 1.0 };
            assert_eq!(got.to_bits(), f64::to_bits(want), "{:#018x}", x.to_bits());
        }
    }
    for x in all32() {
        let got = signum_canon_f32(x);
        if x.is_nan() {
            assert_eq!(got.to_bits(), CANON32, "{:#010x}", x.to_bits());
        } else {
            let want = if x.is_sign_negative() { -1.0 } else { 1.0 };
            assert_eq!(got.to_bits(), f32::to_bits(want), "{:#010x}", x.to_bits());
        }
    }
    assert_eq!(signum_canon_f64(-0.0), -1.0);
    assert_eq!(signum_canon_f64(0.0), 1.0);
    assert_eq!(signum_canon_f32(f32::NEG_INFINITY), -1.0);
}
