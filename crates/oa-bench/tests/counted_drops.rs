//! Rows, samples and values the benchmark leaves out are counted by reason, and a swap is named
//! the same way whatever the order of the legs. Every number is made up.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_bench::adsb_truth::map_query;
use oa_bench::common_time::{common_time_compare, CommonTimeConfig};
use oa_bench::legacy::{flight_ix, new_legacy, push_th, Limits, NO_ALT, NO_TIME};
use oa_bench::metrics_flow::{t9_association_errors, SwapConfig};
use oa_bench::metrics_th::{fit_offset, t2_along_corrected, OffsetConfig};
use oa_bench::pair::PairTable;
use oa_bench::stats::MetricResult;
use oa_bench::tracker_input::{legacy_from_track_samples_into, TrackSamples, DROP_FLIGHT_FULL};
use oa_bench::truth::{LegStart, NoTruth, Truth, TruthLeg, TruthSample};
use oa_truth::{LegIx, TruthQuery, TruthSample as TruthState};

fn scalar(m: &MetricResult, name: &str) -> f64 {
    m.scalars
        .iter()
        .find(|s| s.0 == name)
        .unwrap_or_else(|| panic!("no scalar {name}"))
        .1
}

// REQ: OA-BENCH-129
#[test]
fn tracker_rows_of_a_flight_the_table_refuses_are_counted() {
    let c = (
        vec![1u32, 2, 2, 1],
        vec![
            1_000_000_000i64,
            2_000_000_000,
            3_000_000_000,
            4_000_000_000,
        ],
        vec![0i32; 4],
        vec![0i32; 4],
        vec![NO_ALT; 4],
        vec![f32::NAN; 4],
        vec![false; 4],
    );
    let s = TrackSamples {
        addr: &c.0,
        t_ns: &c.1,
        lat_e7: &c.2,
        lon_e7: &c.3,
        alt_ft: &c.4,
        vel_east_mps: &c.5,
        vel_north_mps: &c.5,
        coast: &c.6,
    };
    let mut leg = new_legacy();
    leg.limits = Limits {
        flights: 1,
        ..Limits::default()
    };
    let leg = legacy_from_track_samples_into(&s, leg).unwrap();
    assert_eq!(leg.th.flight.len(), 2);
    assert_eq!(
        leg.counts.reasons.get(DROP_FLIGHT_FULL).copied(),
        Some(2),
        "{:?}",
        leg.counts.reasons
    );
}

// REQ: OA-BENCH-129
#[test]
fn offset_fit_counts_samples_it_leaves_out() {
    let mut p = PairTable::default();
    let rows = [
        (10.0f32, 100.0f32),
        (f32::NAN, 100.0),
        (10.0, f32::NAN),
        (10.0, 1.0),
        (20.0, 200.0),
    ];
    for (i, (e, v)) in rows.into_iter().enumerate() {
        p.th.push(0);
        p.flight.push(0);
        p.addr.push(1);
        p.t_ms.push(i as i64 * 1000);
        p.cross_m.push(0.0);
        p.along_m.push(e);
        p.truth_speed_mps.push(v);
    }
    let fit = fit_offset(&p, &OffsetConfig::default());
    assert_eq!((fit.n, fit.not_finite, fit.below_min_speed), (2, 2, 1));
    let r = t2_along_corrected(&p, &OffsetConfig::default());
    assert_eq!(scalar(&r, "fit_excluded_not_finite"), 2.0);
    assert_eq!(scalar(&r, "fit_excluded_below_min_speed"), 1.0);
}

// REQ: OA-BENCH-129
#[test]
fn common_time_counts_samples_without_a_finite_error_and_one_sided_aircraft() {
    let mk = |addr: u32, cross: &[f32]| {
        let mut p = PairTable::default();
        for (i, &c) in cross.iter().enumerate() {
            p.th.push(0);
            p.flight.push(0);
            p.addr.push(addr);
            p.t_ms.push(i as i64 * 1000);
            p.cross_m.push(c);
            p.along_m.push(0.0);
        }
        p
    };
    let mut a = mk(1, &[1.0, f32::NAN, 1.0]);
    let b = mk(1, &[1.0, 1.0, 1.0]);
    let extra = mk(2, &[1.0]);
    for i in 0..extra.t_ms.len() {
        a.th.push(0);
        a.flight.push(0);
        a.addr.push(2);
        a.t_ms.push(extra.t_ms[i]);
        a.cross_m.push(1.0);
        a.along_m.push(0.0);
    }
    let cfg = CommonTimeConfig {
        grid_ms: 1000,
        tol_ms: 100,
    };
    let m = common_time_compare(&a, &b, &cfg);
    assert_eq!(scalar(&m, "samples without a finite error, system A"), 1.0);
    assert_eq!(scalar(&m, "aircraft of system A only"), 1.0);
}

// REQ: OA-BENCH-129
#[test]
fn a_truth_state_with_a_non_finite_altitude_is_refused() {
    let s = |alt: Option<f64>, vr: Option<f64>| {
        map_query(TruthQuery::Ok(TruthState {
            lat_deg: 41.0,
            lon_deg: -88.0,
            baro_alt_ft: alt,
            vel_east_mps: 1.0,
            vel_north_mps: 1.0,
            vert_rate_fpm: vr,
            sigma95_h_m: 5.0,
            leg: LegIx(0),
        }))
    };
    assert_eq!(s(Some(f64::NAN), None), Err(NoTruth::NotFinite));
    assert_eq!(s(Some(f64::INFINITY), None), Err(NoTruth::NotFinite));
    assert_eq!(s(Some(1.0e12), None), Err(NoTruth::NotFinite));
    assert_eq!(s(None, Some(f64::NAN)), Err(NoTruth::NotFinite));
    assert_eq!(
        s(Some(35_000.4), Some(-640.6)).unwrap().baro_alt_ft,
        Some(35_000)
    );
}

/// A swap world: aircraft `own` (far from its ERAM position), and `others` all at the same
/// point, which is where the ERAM track is.
fn swap_world(others: &[u32], cfg: &SwapConfig) -> MetricResult {
    let mut leg = new_legacy();
    let f = flight_ix(&mut leg, 1).unwrap();
    let mut p = PairTable::default();
    for k in 0..6i64 {
        push_th(
            &mut leg,
            f,
            k * 1000,
            NO_TIME,
            410_000_000,
            -880_000_000,
            NO_ALT,
            None,
            false,
        );
        p.th.push(k as u32);
        p.flight.push(f);
        p.addr.push(0xC0);
        p.t_ms.push(k * 1000);
        p.cross_m.push(5000.0);
        p.along_m.push(0.0);
    }
    let legs: Vec<TruthLeg> = others
        .iter()
        .map(|&a| TruthLeg {
            addr: a,
            start_ms: 0,
            end_ms: 10_000,
            callsign: None,
            start: LegStart::Unknown,
        })
        .collect();
    let q = |a: u32, _t: i64| {
        let (lat, lon) = if a == 0xC0 {
            (42.0, -88.0)
        } else {
            (41.0, -88.0)
        };
        Ok(TruthSample {
            lat,
            lon,
            baro_alt_ft: None,
            vel_east_mps: 0.0,
            vel_north_mps: 0.0,
            vert_rate_fpm: None,
            sigma95_h_m: 5.0,
            leg: 0,
        })
    };
    let truth = Truth {
        query: &q,
        legs: &legs,
    };
    t9_association_errors(&leg, &p, &truth, cfg)
}

// REQ: OA-BENCH-130
#[test]
fn a_swap_between_two_equally_near_aircraft_names_the_lower_address_in_any_order() {
    let cfg = SwapConfig {
        stride: 1,
        ..SwapConfig::default()
    };
    let a = swap_world(&[0xB0, 0xA0], &cfg);
    let b = swap_world(&[0xA0, 0xB0], &cfg);
    assert_eq!(a.cases, b.cases);
    assert!(a.cases[0].contains("follows 0000A0"), "{:?}", a.cases);
    assert_eq!(scalar(&a, "nearest_tied_samples"), 6.0);
}

// REQ: OA-BENCH-130
#[test]
fn t9_refuses_and_counts_samples_past_its_work_budget() {
    let cfg = SwapConfig {
        stride: 1,
        max_leg_visits: 4,
        ..SwapConfig::default()
    };
    let m = swap_world(&[0xB0, 0xA0], &cfg);
    // Two legs per tested sample: two samples fit, four are refused.
    assert_eq!(scalar(&m, "samples_refused_work_budget"), 4.0);
    assert_eq!(scalar(&m, "samples_tested"), 2.0);
}
