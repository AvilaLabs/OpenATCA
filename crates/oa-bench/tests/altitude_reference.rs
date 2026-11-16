//! T4 compares ERAM altitude with the truth on ERAM's own reference: a flight level against
//! pressure altitude, an altitude (below 18,000 ft) against pressure altitude brought to
//! indicated altitude with the altimeter setting. Every identifier and position is made up.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_bench::assoc::Assoc;
use oa_bench::legacy::{flight_ix, new_legacy, push_th};
use oa_bench::metrics_th::t4_altitude;
use oa_bench::pair::{build_pairs_with, PairConfig};
use oa_bench::stats::MetricResult;
use oa_bench::thindex::build_th_index;
use oa_bench::truth::{nearest_setting, NoTruth, Truth, TruthLeg, TruthSample};

const ADDR: u32 = 0xA1_0005;
/// 1029 hPa in inches of mercury.
const INHG_1029: f64 = 30.39;

type Excl = Vec<((&'static str, &'static str), u64)>;

/// One paired sample: truth at pressure altitude `truth_ft`, ERAM reporting `eram_ft`, the
/// altimeter lookup `alt`. Returns the T4 table and the exclusions.
fn t4_of(
    truth_ft: i32,
    eram_ft: i32,
    alt: &dyn Fn(f64, f64, i64) -> Option<f64>,
) -> (MetricResult, Excl) {
    let q = |_: u32, _: i64| -> Result<TruthSample, NoTruth> {
        Ok(TruthSample {
            lat: 41.0,
            lon: -88.0,
            baro_alt_ft: Some(truth_ft),
            vel_east_mps: 100.0,
            vel_north_mps: 0.0,
            vert_rate_fpm: Some(0),
            sigma95_h_m: 5.0,
            leg: 0,
        })
    };
    let legs = [TruthLeg {
        addr: ADDR,
        start_ms: 0,
        end_ms: 100_000_000,
        callsign: None,
        start: Default::default(),
    }];
    let truth = Truth {
        query: &q,
        legs: &legs,
    };
    let mut leg = new_legacy();
    let f = flight_ix(&mut leg, 1).unwrap();
    push_th(
        &mut leg,
        f,
        10_000_000,
        10_000_000,
        410_000_000,
        -880_000_000,
        eram_ft,
        Some((1944, 0)),
        false,
    );
    let ix = build_th_index(&leg.th, 1);
    let assoc = Assoc {
        addr: vec![ADDR],
        method: vec![0],
        status: vec![0],
        ..Assoc::default()
    };
    let (p, ex) = build_pairs_with(&leg, &ix, &assoc, &truth, &PairConfig::default(), alt);
    (t4_altitude(&p), ex.into_iter().collect())
}

fn scalar(r: &MetricResult, k: &str) -> f64 {
    r.scalars.iter().find(|(n, _)| n == k).unwrap().1
}

fn all_mean(r: &MetricResult) -> f64 {
    r.rows.iter().find(|x| x.label == "all").unwrap().stats.mean
}

// REQ: OA-BENCH-131
#[test]
fn below_the_transition_the_truth_is_brought_to_indicated_altitude() {
    // Truth at 5,000 ft pressure altitude on a 1029 hPa day is 5,470 ft indicated; ERAM says so.
    let (r, ex) = t4_of(5_000, 5_470, &|_, _, _| Some(INHG_1029));
    assert!(all_mean(&r).abs() < 1.0, "T4 error {}", all_mean(&r));
    assert_eq!(scalar(&r, "corrected_with_altimeter"), 1.0);
    assert_eq!(scalar(&r, "compared_as_flight_level"), 0.0);
    assert!((scalar(&r, "mean_altimeter_correction_ft") - 470.0).abs() < 1.0);
    assert!(ex.is_empty(), "{ex:?}");
}

// REQ: OA-BENCH-131
#[test]
fn a_flight_level_is_compared_uncorrected() {
    let (r, _) = t4_of(25_000, 25_030, &|_, _, _| Some(INHG_1029));
    assert!((all_mean(&r) - 30.0).abs() < 1e-6, "{}", all_mean(&r));
    assert_eq!(scalar(&r, "compared_as_flight_level"), 1.0);
    assert_eq!(scalar(&r, "corrected_with_altimeter"), 0.0);
}

// REQ: OA-BENCH-131
#[test]
fn a_pair_with_no_altimeter_setting_is_excluded_with_its_reason() {
    let (r, ex) = t4_of(5_000, 5_470, &|_, _, _| None);
    assert_eq!(scalar(&r, "excluded_no_altimeter_setting"), 1.0);
    assert_eq!(scalar(&r, "corrected_with_altimeter"), 0.0);
    assert!(r.rows.iter().all(|x| x.label != "all" || x.stats.n == 0));
    assert!(
        ex.iter()
            .any(|((s, w), n)| *s == "T4" && *w == "no altimeter setting" && *n == 1),
        "{ex:?}"
    );
}

// REQ: OA-BENCH-131
#[test]
fn the_setting_is_the_one_nearest_in_time_within_an_hour() {
    let obs = [(0, 29.92), (3_000_000, 30.39), (12_000_000, 30.00)];
    assert_eq!(nearest_setting(&obs, 2_500_000), Some(30.39));
    assert_eq!(nearest_setting(&obs, 3_000_000 + 3_600_000), Some(30.39));
    assert_eq!(nearest_setting(&obs, 3_000_000 + 3_600_001), None);
}
