//! Vertical reference of the ground test and the geodesic distances of the scorer, on made-up
//! worlds.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_bench::geo::{dist_m, enu, from_enu};
use oa_bench::metrics_event::{landing_time, takeoff_time, EventConfig, GroundEvent};
use oa_bench::truth::{AirportRef, NoTruth, Truth, TruthLeg, TruthSample};

const FIELD: (f64, f64) = (39.86, -104.67);

fn sample(alt: i32) -> TruthSample {
    TruthSample {
        lat: FIELD.0,
        lon: FIELD.1,
        baro_alt_ft: Some(alt),
        vel_east_mps: 0.0,
        vel_north_mps: 0.0,
        vert_rate_fpm: None,
        sigma95_h_m: 5.0,
        leg: 0,
    }
}

/// A landing leg from 0 to 100 s that stays at one pressure altitude over the threshold.
fn landing(alt: i32, f: impl FnOnce(&Truth<'_>)) {
    let q = move |_: u32, _: i64| -> Result<TruthSample, NoTruth> { Ok(sample(alt)) };
    let legs = [TruthLeg {
        addr: 1,
        start_ms: 0,
        end_ms: 100_000,
        callsign: None,
    }];
    f(&Truth {
        query: &q,
        legs: &legs,
    });
}

fn field(altimeters: Vec<(i64, f64)>) -> AirportRef {
    AirportRef {
        elev_ft: 5434,
        thresholds: vec![FIELD],
        altimeters,
    }
}

// REQ: OA-BENCH-110
#[test]
fn a_low_pressure_day_does_not_hide_a_landing_at_a_high_field() {
    // Field 5434 ft MSL, altimeter 29.42: an aircraft on the runway reads 5934 ft pressure
    // altitude, above the 5634 ft limit as pressure altitude but on the ground in feet MSL.
    let ap = field(vec![(100_000, 29.42)]);
    landing(5934, |t| {
        assert_eq!(
            landing_time(t, 0, &ap, &EventConfig::default()),
            GroundEvent::At(100_000)
        );
        assert_eq!(
            takeoff_time(t, 0, &ap, &EventConfig::default()),
            GroundEvent::At(0)
        );
    });
}

// REQ: OA-BENCH-110
#[test]
fn a_high_pressure_day_does_not_make_an_airborne_point_ground() {
    // Altimeter 30.42: 5634 ft pressure altitude is 6134 ft MSL, 700 ft above the field.
    let ap = field(vec![(100_000, 30.42)]);
    landing(5634, |t| {
        assert_eq!(
            landing_time(t, 0, &ap, &EventConfig::default()),
            GroundEvent::NotFound
        );
    });
}

// REQ: OA-BENCH-110
#[test]
fn the_altimeter_setting_nearest_in_time_within_an_hour_is_used() {
    let mut ap = field(vec![(0, 30.42), (5_400_000, 29.42)]);
    // 100 s from the first, 5300 s from the second.
    assert_eq!(ap.altimeter_near(100_000), Some(30.42));
    // 3500 s after the first and 1900 s before the second: the second.
    assert_eq!(ap.altimeter_near(3_500_000), Some(29.42));
    // Two settings 90 minutes apart, an event 45 minutes from each: the earlier.
    assert_eq!(ap.altimeter_near(2_700_000), Some(30.42));
    ap.altimeters = vec![(0, 30.42)];
    assert_eq!(ap.altimeter_near(3_600_000), Some(30.42), "60 min is in");
    assert_eq!(ap.altimeter_near(3_600_001), None, "past 60 min is out");
    ap.altimeters = vec![(0, f64::NAN), (0, 3000.0)];
    assert_eq!(ap.altimeter_near(0), None, "not altimeter settings");
}

// REQ: OA-BENCH-110
#[test]
fn no_altimeter_setting_is_never_filled_in_with_the_elevation_or_the_standard() {
    // 5434 ft pressure altitude would pass a bare comparison with the field elevation.
    for altimeters in [vec![], vec![(10_000_000, 29.92)]] {
        let ap = field(altimeters);
        landing(5434, |t| {
            assert_eq!(
                landing_time(t, 0, &ap, &EventConfig::default()),
                GroundEvent::NoAltimeter
            );
            assert_eq!(
                takeoff_time(t, 0, &ap, &EventConfig::default()),
                GroundEvent::NoAltimeter
            );
        });
    }
}

// REQ: OA-BENCH-112
#[test]
fn distances_across_the_pole_are_geodesic() {
    // 89.99 N at 0 E and at 180 E: 0.02 degrees of arc over the pole, 2.234 km.
    let d = dist_m(89.99, 0.0, 89.99, 180.0);
    assert!((d - 2233.9).abs() < 1.0, "distance {d}");
    let (e, n) = enu(89.99, 0.0, 89.99, 180.0);
    // From 0 E the pole lies due north of 89.99 N along the 0 E meridian: the other point is
    // 2.234 km north, none east.
    assert!(
        e.abs() < 1.0 && (n - 2233.9).abs() < 1.0,
        "east {e} north {n}"
    );
}

// REQ: OA-BENCH-112
#[test]
fn midlatitude_distances_are_unchanged_to_a_tenth_of_a_percent() {
    // 0.1 degree of latitude and longitude at 41 N: 11.1 km north, 8.4 km east.
    let (e, n) = enu(41.0, -88.0, 41.1, -87.9);
    assert!((n - 11_119.0).abs() < 40.0, "north {n}");
    assert!((e - 8_395.0).abs() < 40.0, "east {e}");
    let d = dist_m(41.0, -88.0, 41.1, -87.9);
    assert!((d - e.hypot(n)).abs() < 1.0);
}

// REQ: OA-BENCH-112
#[test]
fn long_baselines_and_the_inverse_frame_hold() {
    // A quarter of the equator: 10 018 754 m on WGS-84.
    let d = dist_m(0.0, 0.0, 0.0, 90.0);
    assert!((d - 10_018_754.17).abs() < 1.0, "distance {d}");
    // A point 300 km north-east of 89.9 N, 30 E and back: the frame round-trips.
    let (la, lo) = from_enu(89.9, 30.0, 200_000.0, 300_000.0);
    let (e, n) = enu(89.9, 30.0, la, lo);
    assert!(
        (e - 200_000.0).abs() < 1e-3 && (n - 300_000.0).abs() < 1e-3,
        "{e} {n}"
    );
}

// REQ: OA-BENCH-112
#[test]
fn an_invalid_position_gives_nan_not_a_distance() {
    assert!(dist_m(95.0, 0.0, 0.0, 0.0).is_nan());
    assert!(dist_m(0.0, 0.0, f64::NAN, 0.0).is_nan());
    let (e, n) = enu(0.0, 200.0, 0.0, 0.0);
    assert!(e.is_nan() && n.is_nan());
}
