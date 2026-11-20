//! The canonical NaN of `oa_track::canon` is the one of `oa_math`. `canon.rs` keeps its own
//! constant (the replay overlay carries that file to old commits), so this test is what stops
//! the two from drifting.

use oa_track::canon::{canon_nan_bits, canon_row, CANON_NAN_F32, OFF_VE};
use oa_track::{OutRow, O_TICK};

fn base() -> OutRow {
    OutRow {
        t_ns: 0,
        addr: 1,
        lat_e7: 0,
        lon_e7: 0,
        vel_east_mps: 0.0,
        vel_north_mps: 0.0,
        pos_cov_m2: [0.0; 3],
        vel_cov_m2s2: [0.0; 3],
        turn_rate_rps: 0.0,
        alt_ft: 0.0,
        alt_var_ft2: 0.0,
        vrate_fpm: 0.0,
        mu_h: [0.0; 2],
        mu_v: [0.0; 2],
        flags: O_TICK,
    }
}

const PAYLOADS: [u32; 6] = [
    0x7fc0_0000,
    0x7fc0_0001,
    0xffc0_0000,
    0xffc0_1234,
    0x7f80_0001,
    0xffff_ffff,
];

// REQ: OA-TRK-360
#[test]
fn the_local_canonical_nan_equals_the_oa_math_one() {
    assert_eq!(
        CANON_NAN_F32,
        oa_math::canon_nan_f32(f32::NAN).to_bits(),
        "canon.rs and oa_math disagree on the canonical f32 NaN"
    );
    for bits in PAYLOADS {
        let x = f32::from_bits(bits);
        assert!(x.is_nan());
        assert_eq!(oa_math::canon_nan_f32(x).to_bits(), CANON_NAN_F32);
    }
    // 0x7ff8... is the f64 one of oa_math, the f32 one widened.
    assert_eq!(
        oa_math::canon_nan(f64::NAN).to_bits(),
        0x7ff8_0000_0000_0000
    );
}

// REQ: OA-TRK-360
#[test]
fn every_nan_in_a_row_is_written_as_the_canonical_one_and_nothing_else_changes() {
    let mut r = base();
    for (i, bits) in PAYLOADS.into_iter().enumerate() {
        r.vel_east_mps = f32::from_bits(bits);
        r.vel_north_mps = if i % 2 == 0 { -0.0 } else { f32::INFINITY };
        let b = canon_row(&r);
        assert_eq!(b[OFF_VE..OFF_VE + 4], CANON_NAN_F32.to_le_bytes());
        let other = r.vel_north_mps.to_bits().to_le_bytes();
        assert_eq!(b[OFF_VE + 4..OFF_VE + 8], other);
    }
}

// REQ: OA-TRK-360
#[test]
fn canon_nan_bits_agrees_with_is_nan_on_every_one_of_the_2_pow_32_patterns() {
    for bits in 0..=u32::MAX {
        let want = if f32::from_bits(bits).is_nan() {
            CANON_NAN_F32
        } else {
            bits
        };
        assert_eq!(canon_nan_bits(bits), want, "{bits:#010x}");
    }
}
