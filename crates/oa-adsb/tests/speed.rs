//! The jump rule at its edges: every category class, every term of the guard and of the
//! consistency allowance.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_adsb::*;

const MPS_PER_KT: f64 = 0.514_444_444_444;

// REQ: OA-ADSB-150
#[test]
fn hard_cap_of_every_category_code() {
    let high = [0xa0u8, 0xa6, 0xb0, 0xb6, 0];
    for c in high {
        assert_eq!(
            category_hard_cap_kt(c, CAP_ORDINARY_KT),
            CAP_HIGH_KT,
            "{c:#x}"
        );
    }
    assert_eq!(category_hard_cap_kt(0xb7, CAP_ORDINARY_KT), CAP_SPACE_KT);
    // Ordinary classes and reserved codes keep the ordinary cap.
    for c in [
        0xa1u8, 0xa2, 0xa3, 0xa4, 0xa5, 0xa7, 0xa8, 0xaf, 0xb1, 0xb2, 0xb3, 0xb4, 0xb5, 0xb8, 0xc1,
        0xc2, 0xc3, 0xc4, 0xd0, 0xff, 1, 0x10,
    ] {
        assert_eq!(
            category_hard_cap_kt(c, CAP_ORDINARY_KT),
            CAP_ORDINARY_KT,
            "{c:#x}"
        );
        assert_eq!(category_hard_cap_kt(c, 1234.0), 1234.0, "{c:#x}");
    }
    // A configured ordinary cap above the high cap is not lowered for the high classes.
    for c in high {
        assert_eq!(category_hard_cap_kt(c, 3000.0), 3000.0, "{c:#x}");
        assert_eq!(category_hard_cap_kt(c, 1500.0), CAP_HIGH_KT, "{c:#x}");
    }
    assert_eq!(category_hard_cap_kt(0xb7, 3000.0), CAP_SPACE_KT);
    assert_eq!(CAP_ORDINARY_KT, 1000.0);
    assert_eq!(CAP_HIGH_KT, 2000.0);
    assert_eq!(CAP_SPACE_KT, 4088.0);
}

// REQ: OA-ADSB-150
#[test]
fn ceiling_of_every_category_code() {
    for c in [0xa1u8, 0xa7] {
        assert_eq!(category_ceiling_kt(c), 600.0, "{c:#x}");
    }
    for c in 0xb1u8..=0xb4 {
        assert_eq!(category_ceiling_kt(c), 450.0, "{c:#x}");
    }
    for c in 0xa2u8..=0xa5 {
        assert_eq!(category_ceiling_kt(c), 800.0, "{c:#x}");
    }
    for c in 0xc1u8..=0xc3 {
        assert_eq!(category_ceiling_kt(c), 200.0, "{c:#x}");
    }
    assert_eq!(category_ceiling_kt(0xb6), 800.0);
    assert_eq!(category_ceiling_kt(0xb7), CAP_SPACE_KT);
    // High performance, no information, and codes outside every range above.
    for c in [
        0xa0u8, 0xa6, 0xa8, 0xb0, 0xb5, 0xb8, 0xc0, 0xc4, 0xd0, 0xff, 0, 1,
    ] {
        assert_eq!(category_ceiling_kt(c), CAP_HIGH_KT, "{c:#x}");
    }
}

fn pair(dist_m: f64, dt_s: f64, category: u8, gs_a_dkt: u16, gs_b_dkt: u16) -> JumpPair {
    JumpPair {
        dist_m,
        dt_s,
        slack_m: 0.0,
        category,
        gs_a_dkt,
        gs_b_dkt,
    }
}

/// Metres covered at `kt` knots in `dt` seconds.
fn metres(kt: f64, dt: f64) -> f64 {
    kt * MPS_PER_KT * dt
}

// REQ: OA-ADSB-151
#[test]
fn a_gap_that_is_not_a_positive_number_is_a_jump_beyond_the_slack() {
    let v = |dist: f64, dt: f64, slack: f64| {
        let mut p = pair(dist, dt, 0xa3, NONE_U16, NONE_U16);
        p.slack_m = slack;
        jump_verdict(&p, CAP_ORDINARY_KT)
    };
    assert_eq!(v(1000.0, f64::NAN, 10.0), JumpVerdict::OverCap);
    assert_eq!(v(1000.0, 0.0, 10.0), JumpVerdict::OverCap);
    assert_eq!(v(1000.0, -0.0, 10.0), JumpVerdict::OverCap);
    assert_eq!(v(1000.0, -5.0, 10.0), JumpVerdict::OverCap);
    assert_eq!(v(1000.0, f64::NEG_INFINITY, 10.0), JumpVerdict::OverCap);
    assert_eq!(v(f64::NAN, 10.0, 10.0), JumpVerdict::OverCap);
    assert_eq!(v(f64::INFINITY, 10.0, 10.0), JumpVerdict::OverCap);
    // Within the slack it is plausible whatever the gap.
    assert_eq!(v(10.0, f64::NAN, 10.0), JumpVerdict::Plausible);
    assert_eq!(v(10.0, 0.0, 10.0), JumpVerdict::Plausible);
    assert_eq!(v(10.0, -5.0, 10.0), JumpVerdict::Plausible);
    assert_eq!(v(0.0, 0.0, 0.0), JumpVerdict::Plausible);
    // Just past the slack with no time between is a jump; with time it is plausible.
    assert_eq!(v(10.5, 0.0, 10.0), JumpVerdict::OverCap);
    assert_eq!(v(10.5, 1.0, 10.0), JumpVerdict::Plausible);
}

// REQ: OA-ADSB-151
#[test]
fn the_slack_widens_every_limit() {
    // 800 kt ceiling of a transport for 10 s, plus 100 m of slack.
    let edge = metres(800.0, 10.0);
    let mut p = pair(edge + 100.0 - 1.0, 10.0, 0xa3, NONE_U16, NONE_U16);
    p.slack_m = 100.0;
    assert_eq!(jump_verdict(&p, CAP_ORDINARY_KT), JumpVerdict::Plausible);
    p.dist_m = edge + 100.0 + 1.0;
    assert_eq!(jump_verdict(&p, CAP_ORDINARY_KT), JumpVerdict::OverCeiling);
    // The hard cap, the same way.
    let edge = metres(1000.0, 10.0);
    p.dist_m = edge + 100.0 + 1.0;
    assert_eq!(jump_verdict(&p, CAP_ORDINARY_KT), JumpVerdict::OverCap);
}

// REQ: OA-ADSB-152
#[test]
fn the_ground_speed_allowance_is_1_2_g_plus_40_plus_2_per_second() {
    // (gs of the first report, of the second, gap): the larger speed is the one used.
    let cases: [(u16, u16, f64); 7] = [
        (3000, NONE_U16, 10.0),
        (NONE_U16, 3000, 10.0),
        (3000, 3000, 10.0),
        (1000, 3000, 10.0),
        (3000, 1000, 10.0),
        (1000, NONE_U16, 100.0),
        (NONE_U16, 5000, 1.0),
    ];
    for (a, b, dt) in cases {
        let g = f64::from(if a == NONE_U16 { 0 } else { a }.max(if b == NONE_U16 { 0 } else { b }))
            / 10.0;
        let allowed = 1.2 * g + 40.0 + 2.0 * dt;
        assert!(allowed < CAP_ORDINARY_KT);
        // 0xc1 has a 200 kt ceiling: a reported speed replaces the ceiling.
        for cat in [0xa3u8, 0xc1, 0] {
            let inside = pair(metres(allowed, dt) * 0.999, dt, cat, a, b);
            let outside = pair(metres(allowed, dt) * 1.001, dt, cat, a, b);
            assert_eq!(
                jump_verdict(&inside, CAP_ORDINARY_KT),
                JumpVerdict::Plausible,
                "{a} {b} {dt} {cat:#x}"
            );
            assert_eq!(
                jump_verdict(&outside, CAP_ORDINARY_KT),
                JumpVerdict::InconsistentWithGroundSpeed,
                "{a} {b} {dt} {cat:#x}"
            );
        }
    }
    assert_eq!(CONSISTENCY_FACTOR, 1.2);
    assert_eq!(CONSISTENCY_BASE_KT, 40.0);
    assert_eq!(CONSISTENCY_ACCEL_KT_S, 2.0);
}

// REQ: OA-ADSB-152
#[test]
fn a_zero_ground_speed_report_still_gets_the_base_allowance() {
    // 0 kt reported (not absent): 40 kt plus 2 kt per second.
    let allowed = 40.0 + 2.0 * 5.0;
    let inside = pair(metres(allowed, 5.0) * 0.999, 5.0, 0xa3, 0, NONE_U16);
    let outside = pair(metres(allowed, 5.0) * 1.001, 5.0, 0xa3, 0, NONE_U16);
    assert_eq!(
        jump_verdict(&inside, CAP_ORDINARY_KT),
        JumpVerdict::Plausible
    );
    assert_eq!(
        jump_verdict(&outside, CAP_ORDINARY_KT),
        JumpVerdict::InconsistentWithGroundSpeed
    );
}

// REQ: OA-ADSB-152
#[test]
fn the_hard_cap_and_the_ceiling_apply_before_and_without_a_ground_speed() {
    // Over the hard cap even though consistent with a huge reported speed.
    let p = pair(metres(1001.0, 10.0), 10.0, 0xa3, 9000, 9000);
    assert_eq!(jump_verdict(&p, CAP_ORDINARY_KT), JumpVerdict::OverCap);
    // No speed: at the ceiling plausible, past it over the ceiling (but under the cap).
    let p = pair(metres(799.0, 10.0), 10.0, 0xa3, NONE_U16, NONE_U16);
    assert_eq!(jump_verdict(&p, CAP_ORDINARY_KT), JumpVerdict::Plausible);
    let p = pair(metres(801.0, 10.0), 10.0, 0xa3, NONE_U16, NONE_U16);
    assert_eq!(jump_verdict(&p, CAP_ORDINARY_KT), JumpVerdict::OverCeiling);
    // The cap of the class: a space vehicle at 3000 kt is fine, an ordinary class is not.
    let p = pair(metres(3000.0, 10.0), 10.0, 0xb7, NONE_U16, NONE_U16);
    assert_eq!(jump_verdict(&p, CAP_ORDINARY_KT), JumpVerdict::Plausible);
    let p = pair(metres(4100.0, 10.0), 10.0, 0xb7, NONE_U16, NONE_U16);
    assert_eq!(jump_verdict(&p, CAP_ORDINARY_KT), JumpVerdict::OverCap);
}

// REQ: OA-ADSB-153
#[test]
fn is_jump_is_true_exactly_when_the_verdict_is_not_plausible() {
    let pairs = [
        pair(0.0, 1.0, 0xa3, NONE_U16, NONE_U16),
        pair(metres(500.0, 10.0), 10.0, 0xa3, NONE_U16, NONE_U16),
        pair(metres(5000.0, 10.0), 10.0, 0xa3, NONE_U16, NONE_U16),
        pair(metres(900.0, 10.0), 10.0, 0xa3, 9000, NONE_U16),
        pair(metres(900.0, 10.0), 10.0, 0xa3, 4500, NONE_U16),
        pair(metres(900.0, 10.0), 10.0, 0xa3, NONE_U16, NONE_U16),
        pair(metres(900.0, 10.0), 10.0, 0xa3, NONE_U16, 4500),
        pair(100.0, 0.0, 0xa3, NONE_U16, NONE_U16),
        pair(100.0, f64::NAN, 0xa3, NONE_U16, NONE_U16),
    ];
    let mut seen = [false; 4];
    for p in pairs {
        let v = jump_verdict(&p, CAP_ORDINARY_KT);
        assert_eq!(
            is_jump(&p, CAP_ORDINARY_KT),
            v != JumpVerdict::Plausible,
            "{p:?}"
        );
        seen[match v {
            JumpVerdict::Plausible => 0,
            JumpVerdict::OverCap => 1,
            JumpVerdict::InconsistentWithGroundSpeed => 2,
            JumpVerdict::OverCeiling => 3,
        }] = true;
    }
    assert_eq!(seen, [true; 4], "the sample must reach every verdict");
}

// REQ: OA-ADSB-170
#[test]
fn the_speed_envelope_is_the_highest_category_cap() {
    let highest = (0..=255u8)
        .map(|c| category_hard_cap_kt(c, CAP_ORDINARY_KT))
        .fold(0.0, f64::max);
    assert_eq!(SPEED_ENVELOPE_KT, highest);
    assert_eq!(SPEED_ENVELOPE_KT, 4088.0);
    assert!((SPEED_ENVELOPE_MPS - 4088.0 * MPS_PER_KT).abs() < 1e-9);
}
