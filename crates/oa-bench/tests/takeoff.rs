//! T8 takeoff as an observed event and T7 truth failures counted by reason. Every identifier and
//! position is made up.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_bench::adsb_truth::leg_start;
use oa_bench::assoc::Assoc;
use oa_bench::legacy::{flight_ix, new_legacy, push_th};
use oa_bench::metrics_flow::{t7_t8_presence, PresenceConfig};
use oa_bench::stats::MetricResult;
use oa_bench::thindex::build_th_index;
use oa_bench::truth::{empty_env, LegStart, NoTruth, Truth, TruthLeg, TruthSample};

const ADDR: u32 = 0xA1_0003;

fn sample(alt: i32) -> TruthSample {
    TruthSample {
        lat: 25.0,
        lon: -88.0,
        baro_alt_ft: Some(alt),
        vel_east_mps: 100.0,
        vel_north_mps: 0.0,
        vert_rate_fpm: Some(0),
        sigma95_h_m: 5.0,
        leg: 0,
    }
}

fn scalar(r: &MetricResult, name: &str) -> f64 {
    r.scalars
        .iter()
        .find(|s| s.0 == name)
        .unwrap_or_else(|| panic!("no scalar {name}: {:?}", r.scalars))
        .1
}

/// T7 and T8 for one leg 0 to `end_ms` whose truth is `q`, in the zone wherever it exists, with
/// ERAM positions at `pos_ms`.
fn run(
    start: LegStart,
    end_ms: i64,
    pos_ms: &[i64],
    q: &dyn Fn(i64) -> Result<TruthSample, NoTruth>,
) -> (MetricResult, MetricResult) {
    let query = |a: u32, t: i64| -> Result<TruthSample, NoTruth> {
        if a != ADDR {
            return Err(NoTruth::UnknownAircraft);
        }
        q(t)
    };
    let legs = [TruthLeg {
        addr: ADDR,
        start_ms: 0,
        end_ms,
        callsign: None,
        start,
    }];
    let truth = Truth {
        query: &query,
        legs: &legs,
    };
    let in_zone = |lat: f64, _lon: f64| lat > 20.0;
    let mut env = empty_env();
    env.in_zone = &in_zone;
    let mut leg = new_legacy();
    let f = flight_ix(&mut leg, 1).unwrap();
    for &t in pos_ms {
        push_th(
            &mut leg,
            f,
            t,
            t,
            250_000_000,
            -880_000_000,
            300,
            None,
            false,
        );
    }
    let ix = build_th_index(&leg.th, leg.flights.gufi.len());
    let assoc = Assoc {
        addr: vec![ADDR],
        ..Assoc::default()
    };
    t7_t8_presence(&leg, &ix, &assoc, &truth, &env, &PresenceConfig::default())
}

fn takeoff(t_ms: i64, gap_ms: i64) -> LegStart {
    LegStart::Takeoff { t_ms, gap_ms }
}

// REQ: OA-BENCH-125
#[test]
fn a_delayed_takeoff_at_a_low_field_is_measured_from_the_takeoff() {
    // On the ground from 0 s, takeoff at 300 s, first ERAM position at 320 s.
    let (_, r8) = run(takeoff(300_000, 4_000), 2_000_000, &[320_000], &|_| {
        Ok(sample(200))
    });
    assert_eq!(r8.rows[0].stats.n, 1, "{:?}", r8.scalars);
    assert_eq!(r8.rows[0].stats.mean, 20.0);
    assert_eq!(scalar(&r8, "takeoff_time_uncertainty_s_max"), 4.0);
}

// REQ: OA-BENCH-125
#[test]
fn a_departure_from_a_high_field_is_counted_with_its_observed_takeoff() {
    let (_, r8) = run(takeoff(300_000, 1_000), 2_000_000, &[330_000], &|_| {
        Ok(sample(5_400))
    });
    assert_eq!(r8.rows[0].stats.n, 1, "{:?}", r8.scalars);
    assert_eq!(r8.rows[0].stats.mean, 30.0);
}

// REQ: OA-BENCH-125
#[test]
fn a_leg_first_seen_airborne_in_the_zone_is_excluded_with_its_reason() {
    let (_, r8) = run(LegStart::FirstSeenAirborne, 2_000_000, &[20_000], &|_| {
        Ok(sample(200))
    });
    assert_eq!(r8.rows[0].stats.n, 0);
    assert_eq!(scalar(&r8, "legs_first_seen_airborne_in_zone"), 1.0);
}

// REQ: OA-BENCH-125
#[test]
fn a_leg_with_no_ground_report_or_never_airborne_is_excluded_with_its_reason() {
    let (_, r8) = run(LegStart::Unknown, 2_000_000, &[20_000], &|_| {
        Ok(sample(200))
    });
    assert_eq!(r8.rows[0].stats.n, 0);
    assert_eq!(
        scalar(&r8, "legs_starting_inside_zone_no_ground_report"),
        1.0
    );
    let (_, r8) = run(LegStart::NeverAirborne, 2_000_000, &[20_000], &|_| {
        Ok(sample(200))
    });
    assert_eq!(scalar(&r8, "legs_starting_inside_zone_never_airborne"), 1.0);
}

// REQ: OA-BENCH-125
#[test]
fn the_takeoff_is_the_first_airborne_report_after_a_ground_report() {
    const S: i64 = 1_000_000_000;
    assert_eq!(leg_start(&[]), LegStart::Unknown);
    assert_eq!(
        leg_start(&[(0, false), (S, true)]),
        LegStart::FirstSeenAirborne
    );
    assert_eq!(leg_start(&[(0, true), (S, true)]), LegStart::NeverAirborne);
    assert_eq!(
        leg_start(&[(0, true), (10 * S, true), (14 * S, false), (15 * S, true)]),
        LegStart::Takeoff {
            t_ms: 14_000,
            gap_ms: 4_000
        }
    );
}

// REQ: OA-BENCH-126
#[test]
fn failed_truth_queries_are_counted_by_reason_over_the_usable_points() {
    // 100 grid points (0 to 495 s): 40 usable, 40 with no nearby report, 20 too uncertain.
    let q = |t: i64| match t {
        t if t < 200_000 => Ok(sample(200)),
        t if t < 400_000 => Err(NoTruth::NoNearbyReport),
        _ => Err(NoTruth::TooUncertain),
    };
    let (r7, _) = run(takeoff(1_000, 1_000), 495_000, &[], &q);
    assert_eq!(scalar(&r7, "truth_grid_points"), 100.0);
    assert_eq!(scalar(&r7, "truth_grid_points_usable"), 40.0);
    assert_eq!(
        scalar(&r7, "truth_grid_points_failed_no_nearby_report"),
        40.0
    );
    assert_eq!(
        scalar(&r7, "truth_grid_points_failed_truth_too_uncertain"),
        20.0
    );
    assert_eq!(
        scalar(&r7, "truth_grid_points_failed_unknown_aircraft"),
        0.0
    );
    assert_eq!(scalar(&r7, "truth_grid_points_failed_outside_legs"), 0.0);
    assert_eq!(scalar(&r7, "truth_grid_points_in_zone"), 40.0);
    assert!(
        r7.notes.iter().any(|n| n.contains("40 of 100")),
        "{:?}",
        r7.notes
    );
}

// REQ: OA-BENCH-126
#[test]
fn a_leg_with_no_usable_truth_is_not_a_leg_that_never_entered_the_zone() {
    let (r7, r8) = run(takeoff(1_000, 1_000), 495_000, &[], &|_| {
        Err(NoTruth::NoNearbyReport)
    });
    assert_eq!(scalar(&r7, "legs_no_usable_truth"), 1.0);
    assert_eq!(scalar(&r8, "legs_no_usable_truth"), 1.0);
    assert_eq!(scalar(&r8, "legs_never_in_zone"), 0.0);
}
