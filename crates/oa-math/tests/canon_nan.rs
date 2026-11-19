//! `canon_nan`: every NaN, however the machine made it, becomes the one fixed NaN.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use core::hint::black_box;
use oa_math::canon_nan;

const CANON: u64 = 0x7ff8_0000_0000_0000;

// REQ: OA-MATH-034
#[test]
fn every_nan_becomes_the_one_fixed_nan_and_nothing_else_changes() {
    // invalid operations made at run time, where the compiler cannot fold them: the hardware's
    // own NaN (x86-64 makes the negative default NaN, aarch64 the positive one)
    let zero = black_box(0.0f64);
    let inf = black_box(f64::INFINITY);
    let neg = black_box(-1.0f64);
    let made = [
        zero / black_box(0.0),
        inf - black_box(f64::INFINITY),
        inf * zero,
        oa_ieee::sqrt(neg),
        -(zero / black_box(0.0)),
        f64::NAN,
        -f64::NAN,
        f64::from_bits(0x7ff8_0000_0000_1234),
        f64::from_bits(0xfff8_0000_0000_1234),
        f64::from_bits(0x7ff0_0000_0000_0001),
        f64::from_bits(0xffff_ffff_ffff_ffff),
    ];
    for x in made {
        assert!(x.is_nan());
        assert_eq!(canon_nan(x).to_bits(), CANON, "from {:#018x}", x.to_bits());
        assert_eq!(canon_nan(black_box(x)).to_bits(), CANON);
    }
    // every other bit pattern, signed zeros, subnormals and infinities included, is returned as is
    for bits in [
        0u64,
        0x8000_0000_0000_0000,
        1,
        0x000f_ffff_ffff_ffff,
        0x7fef_ffff_ffff_ffff,
        0x7ff0_0000_0000_0000,
        0xfff0_0000_0000_0000,
        0x3ff0_0000_0000_0000,
        0xbff0_0000_0000_0000,
    ] {
        assert_eq!(canon_nan(f64::from_bits(bits)).to_bits(), bits);
    }
    // the constant is the one the functions of the crate return
    assert_eq!(oa_math::sin(f64::INFINITY).to_bits(), CANON);
    assert_eq!(canon_nan(oa_math::ln(black_box(-1.0))).to_bits(), CANON);
}

// REQ: OA-MATH-034
#[test]
fn the_single_precision_form_has_one_fixed_nan_too() {
    let zero = black_box(0.0f32);
    for x in [
        zero / black_box(0.0),
        -(zero / black_box(0.0)),
        f32::from_bits(0x7fc0_1234),
        f32::from_bits(0xffff_ffff),
        f32::from_bits(0x7f80_0001),
    ] {
        assert!(x.is_nan());
        assert_eq!(oa_math::canon_nan_f32(x).to_bits(), 0x7fc0_0000);
    }
    for bits in [0u32, 0x8000_0000, 1, 0x7f80_0000, 0xff80_0000, 0x3f80_0000] {
        assert_eq!(oa_math::canon_nan_f32(f32::from_bits(bits)).to_bits(), bits);
    }
}
