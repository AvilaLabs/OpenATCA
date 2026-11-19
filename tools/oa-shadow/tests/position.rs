//! Position divergence, the whole-second time allowance, interpolation, and altitude and coast.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod common;

use common::*;
use oa_shadow::compare::{compare_all, Class};
use oa_shadow::geo::{deg, oa_at, separation, OaPoint};
use oa_shadow::model::EramSample;
use oa_shadow::thresholds::THRESHOLDS;

// REQ: OA-SHD-003
#[test]
fn position_thresholds_are_strict_at_half_and_one_nautical_mile() {
    let t = &THRESHOLDS;
    assert_eq!(t.pos_level(925.999), None);
    assert_eq!(t.pos_level(926.0), None);
    assert_eq!(t.pos_level(926.001), Some(0));
    assert_eq!(t.pos_level(1851.999), Some(0));
    assert_eq!(t.pos_level(1852.0), Some(0));
    assert_eq!(t.pos_level(1852.001), Some(1));
}

// REQ: OA-SHD-003
#[test]
fn cross_track_divergence_classes_either_side_of_each_threshold() {
    for (east, want) in [
        // The synthetic path drifts a few metres against the comparison's mid-latitude scale, so
        // these sit 8 m from the thresholds; the exact edge is the test above.
        (900.0, None),
        (918.0, None),
        (934.0, Some(Class::PosCross05)),
        (1844.0, Some(Class::PosCross05)),
        (1860.0, Some(Class::PosCross10)),
        (-1860.0, Some(Class::PosCross10)),
    ] {
        let c = one_pair(east, 0.0);
        let rows: Vec<_> = c.events.iter().filter(|e| e.class.is_position()).collect();
        match want {
            None => assert!(rows.is_empty(), "east {east}: {rows:?}"),
            Some(class) => {
                assert_eq!(rows.len(), 9, "east {east}");
                assert!(
                    rows.iter().all(|e| e.class == class),
                    "east {east}: {rows:?}"
                );
                // Cross-track only; the along-track part is within the allowance.
                assert!(rows.iter().all(|e| e.along_m.abs() < 1.0));
            }
        }
    }
}

// REQ: OA-SHD-003
#[test]
fn along_track_divergence_is_raw_and_flagged_for_the_whole_second_stamp() {
    // 100 m/s: one second of travel is 100 m. The classification uses the raw separation.
    for (north, want, flag) in [
        (924.0, None, false),
        (928.0, Some(Class::PosAlong05), true), // 828 m after the allowance: under 926
        (1100.0, Some(Class::PosAlong05), false), // 1000 m after it: still over
        (1854.0, Some(Class::PosAlong10), true), // 1754 m after it: under 1852
        (2000.0, Some(Class::PosAlong10), false),
        (-928.0, Some(Class::PosAlong05), true),
        (-1854.0, Some(Class::PosAlong10), true),
    ] {
        let c = one_pair(0.0, north);
        let rows: Vec<_> = c.events.iter().filter(|e| e.class.is_position()).collect();
        match want {
            None => assert!(rows.is_empty(), "north {north}: {rows:?}"),
            Some(class) => {
                assert_eq!(rows.len(), 9, "north {north}");
                assert!(rows.iter().all(|e| e.class == class), "north {north}");
                assert!(rows.iter().all(|e| e.cross_m.abs() < 1.0));
                assert!(
                    rows.iter().all(|e| e.within_stamp_allowance == Some(flag)),
                    "north {north}"
                );
                assert!(rows.iter().all(|e| (e.credit_m - 100.0).abs() < 0.01));
            }
        }
    }
    // Cross-track events carry no flag. Both axes over: one row for each axis per report.
    let c = one_pair(1000.0, 1100.0);
    assert_eq!(count(&c, Class::PosCross05), 9);
    assert_eq!(count(&c, Class::PosAlong05), 9);
    assert!(of(&c, Class::PosCross05)
        .iter()
        .all(|e| e.within_stamp_allowance.is_none()));
}

// REQ: OA-SHD-004
#[test]
fn separation_is_raw_with_the_stamp_credit_beside_it() {
    let th = &THRESHOLDS;
    let oa = OaPoint {
        lat_deg: deg(LAT),
        lon_deg: deg(LON),
        alt_ft: None,
        vel_east_mps: 0.0,
        vel_north_mps: 200.0,
        coast: false,
    };
    let at = |east: f64, north: f64| {
        let (lat, lon) = displaced(east, north);
        separation(
            &EramSample {
                t_ms: 0,
                lat_e7: lat,
                lon_e7: lon,
                alt_ft: None,
                coast: false,
            },
            &oa,
            th,
        )
    };
    assert!((at(0.0, 300.0).along_m - 300.0).abs() < 1.0);
    assert!((at(0.0, -300.0).along_m + 300.0).abs() < 1.0);
    assert!((at(0.0, 300.0).credit_m - 200.0).abs() < 1e-9);
    // Right of travel is positive; cross-track is untouched by time.
    assert!((at(150.0, 0.0).cross_m - 150.0).abs() < 1.0);
    assert!((at(-150.0, 0.0).cross_m + 150.0).abs() < 1.0);
    assert!((at(150.0, 0.0).horiz_m - 150.0).abs() < 1.0);
}

// REQ: OA-SHD-004
#[test]
fn a_slow_aircraft_has_no_direction_and_its_separation_is_cross_track() {
    let th = &THRESHOLDS;
    let oa = OaPoint {
        lat_deg: deg(LAT),
        lon_deg: deg(LON),
        alt_ft: None,
        vel_east_mps: 0.5,
        vel_north_mps: 0.0,
        coast: false,
    };
    let (lat, lon) = displaced(30.0, 40.0);
    let s = separation(
        &EramSample {
            t_ms: 0,
            lat_e7: lat,
            lon_e7: lon,
            alt_ft: None,
            coast: false,
        },
        &oa,
        th,
    );
    assert!((s.cross_m - 50.0).abs() < 0.1);
    assert_eq!((s.along_m, s.credit_m), (0.0, 0.0));
}

// REQ: OA-SHD-005
#[test]
fn openatca_is_interpolated_to_the_eram_time_and_has_no_sample_across_a_gap() {
    let th = &THRESHOLDS;
    let ac = oa_north(1, 0, 10, 100.0);
    let p = oa_at(&ac.samples, T0 + 2500, th).unwrap();
    let (lat, _) = displaced(0.0, 250.0);
    assert!((p.lat_deg - deg(lat)).abs() < 1e-7);
    assert!((p.vel_north_mps - 100.0).abs() < 1e-9);
    // Exact time: the row itself.
    assert_eq!(
        oa_at(&ac.samples, T0 + 3000, th).unwrap().lat_deg,
        deg(ac.samples[3].lat_e7)
    );
    // Rows 2 s apart: still covered; 2.001 s apart: not.
    let mut gap = oa_north(1, 0, 10, 100.0);
    gap.samples.remove(5);
    assert!(oa_at(&gap.samples, T0 + 5000, th).is_some());
    gap.samples[4].t_ms -= 1;
    assert!(oa_at(&gap.samples, T0 + 5000, th).is_none());
    // Before the first row and after the last: none.
    assert!(oa_at(&ac.samples, T0 - 1, th).is_none());
    assert!(oa_at(&ac.samples, T0 + 10_001, th).is_none());
    // An ERAM stamp between two rows is compared against the interpolation, not the nearest row.
    let samples = vec![eram_at(5, 5.5, 100.0, 0.0, 0.0)];
    let c = compare_all(&[flight(Some(1), samples)], &oa_map(vec![ac]), None);
    assert_eq!(c.totals.compared, 1);
    assert!(c.events.is_empty(), "{:?}", c.events);
}

// REQ: OA-SHD-006
#[test]
fn altitude_disagreement_is_over_300_feet_either_sign_and_needs_both_altitudes() {
    assert!(!THRESHOLDS.alt_differs(300.0));
    assert!(!THRESHOLDS.alt_differs(-300.0));
    assert!(THRESHOLDS.alt_differs(300.1));
    assert!(THRESHOLDS.alt_differs(-300.1));
    for (alt, hit) in [
        (10_299, false),
        (10_300, false),
        (10_301, true),
        (9_699, true),
        (9_700, false),
    ] {
        let mut s = eram_at(12, 12.0, 100.0, 0.0, 0.0);
        s.alt_ft = Some(alt);
        let c = compare_all(
            &[flight(Some(1), vec![s])],
            &oa_map(vec![oa_north(1, 0, 20, 100.0)]),
            None,
        );
        assert_eq!(count(&c, Class::Alt), usize::from(hit), "{alt}");
        if hit {
            assert_eq!(of(&c, Class::Alt)[0].alt_diff_ft, f64::from(alt - 10_000));
        }
    }
    // ERAM without an altitude: not compared, counted.
    let mut s = eram_at(12, 12.0, 100.0, 0.0, 0.0);
    s.alt_ft = None;
    let c = compare_all(
        &[flight(Some(1), vec![s])],
        &oa_map(vec![oa_north(1, 0, 20, 100.0)]),
        None,
    );
    assert_eq!(
        (
            count(&c, Class::Alt),
            c.totals.alt_compared,
            c.totals.alt_not_comparable
        ),
        (0, 0, 1)
    );
}

// REQ: OA-SHD-010
#[test]
fn coast_status_differences_are_classed_by_which_system_coasts() {
    let mut oa = oa_north(1, 0, 40, 100.0);
    for s in &mut oa.samples[20..=30] {
        s.coast = true;
    }
    let mut eram_samples: Vec<_> = (1..=3)
        .map(|k| eram_at(k * 12, (k * 12) as f64, 100.0, 0.0, 0.0))
        .collect();
    eram_samples[0].coast = true; // 12 s: ERAM coasts, OpenATCA does not
                                  // 24 s: OpenATCA coasts (rows 20..=30), ERAM does not; 36 s: neither.
    let c = compare_all(&[flight(Some(1), eram_samples)], &oa_map(vec![oa]), None);
    assert_eq!(count(&c, Class::CoastEramOnly), 1);
    assert_eq!(count(&c, Class::CoastOaOnly), 1);
    assert_eq!(c.totals.coast_compared, 3);
    // Both coasting is not a difference.
    let mut oa = oa_north(1, 0, 40, 100.0);
    oa.samples.iter_mut().for_each(|s| s.coast = true);
    let mut e = eram_at(12, 12.0, 100.0, 0.0, 0.0);
    e.coast = true;
    let c = compare_all(&[flight(Some(1), vec![e])], &oa_map(vec![oa]), None);
    assert_eq!(
        count(&c, Class::CoastEramOnly) + count(&c, Class::CoastOaOnly),
        0
    );
}

// REQ: OA-SHD-010
#[test]
fn coast_is_compared_only_at_an_openatca_row_at_the_exact_eram_time() {
    // OpenATCA rows at half seconds: position is interpolated, the coast flag is not.
    let mut oa = oa_north(1, 0, 40, 100.0);
    for s in &mut oa.samples {
        s.t_ms += 500;
        s.coast = true;
    }
    let samples = (1..=3)
        .map(|k| eram_at(k * 12, (k * 12) as f64 + 0.5, 100.0, 0.0, 0.0))
        .collect();
    let c = compare_all(&[flight(Some(1), samples)], &oa_map(vec![oa]), None);
    assert_eq!(c.totals.compared, 3);
    assert_eq!(c.totals.coast_compared, 0);
    assert_eq!(
        count(&c, Class::CoastOaOnly) + count(&c, Class::CoastEramOnly),
        0
    );
}
