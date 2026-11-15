//! Two systems that publish at different rates, scored at the same truth times. Every number is
//! made up.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_bench::common_time::{common_time_compare, CommonTimeConfig};
use oa_bench::pair::PairTable;
use oa_bench::stats::{stats_of, MetricResult, Stats};
use oa_bench::synth::T0_MS;

const ADDR: u32 = 0xC0_0001;

/// The same error over time for both systems: 100 m for the first 10 s, nothing for the next
/// 90 s (all of it cross-track).
fn err_at(s: i64) -> f32 {
    if s < 10 {
        100.0
    } else {
        0.0
    }
}

/// A system's pairs at the given seconds of a 100 s flight.
fn system(secs: &[i64]) -> PairTable {
    let mut p = PairTable::default();
    for &s in secs {
        p.th.push(0);
        p.flight.push(0);
        p.addr.push(ADDR);
        p.t_ms.push(T0_MS + s * 1000);
        p.cross_m.push(err_at(s));
        p.along_m.push(0.0);
    }
    p
}

fn row<'a>(m: &'a MetricResult, label: &str) -> &'a Stats {
    &m.rows
        .iter()
        .find(|r| r.label == label)
        .unwrap_or_else(|| panic!("no row {label}"))
        .stats
}

// REQ: OA-BENCH-054
#[test]
fn different_publication_rates_do_not_change_the_common_time_error() {
    // One reports every second; the other every second while the error lasts, then every 10 s.
    let dense: Vec<i64> = (0..100).collect();
    let mut sparse: Vec<i64> = (0..10).collect();
    sparse.extend((10..100).step_by(10));
    let (a, b) = (system(&dense), system(&sparse));
    // Native per-report RMS depends on the rate: the same error history, two answers.
    let native =
        |p: &PairTable| stats_of(&p.cross_m.iter().map(|&x| f64::from(x)).collect::<Vec<_>>()).rms;
    assert!((native(&a) - 31.62).abs() < 0.01);
    assert!((native(&b) - 72.5).abs() < 0.1);
    // At the same truth times the two agree.
    let cfg = CommonTimeConfig {
        grid_ms: 10_000,
        tol_ms: 500,
    };
    let m = common_time_compare(&a, &b, &cfg);
    let (ra, rb) = (
        row(&m, "horizontal, system A"),
        row(&m, "horizontal, system B"),
    );
    assert_eq!(ra.n, 10);
    assert_eq!(rb.n, 10);
    assert!((ra.rms - 31.62).abs() < 0.01, "{}", ra.rms);
    assert!((rb.rms - ra.rms).abs() < 1e-9);
    // One aircraft: its own RMS is the per-flight distribution.
    assert_eq!(row(&m, "per-flight RMS, system A").n, 1);
    assert!((row(&m, "per-flight RMS, system B").rms - 31.62).abs() < 0.01);
    assert!(m.notes.iter().any(|n| n.contains("same truth times")));
}

// REQ: OA-BENCH-054
#[test]
fn a_grid_time_counts_only_when_both_systems_have_a_report_near_it() {
    let a = system(&(0..100).collect::<Vec<_>>());
    // The second system is silent for 30 s to 60 s.
    let b = system(&(0..30).chain(60..100).collect::<Vec<_>>());
    let cfg = CommonTimeConfig {
        grid_ms: 10_000,
        tol_ms: 500,
    };
    let m = common_time_compare(&a, &b, &cfg);
    // Grid times 0..=90 s: 30, 40, 50 have no report of B (a report at 29 s is 1 s away).
    assert_eq!(row(&m, "horizontal, system A").n, 7);
    assert_eq!(row(&m, "horizontal, system B").n, 7);
    let dropped = m
        .scalars
        .iter()
        .find(|(k, _)| k.contains("grid times without a report"))
        .unwrap()
        .1;
    assert_eq!(dropped, 3.0);
}

/// Rows `(flight, second, error)` of one aircraft, every time shifted by `shift_ms`.
fn rows(spec: &[(u32, i64, f32)], shift_ms: i64) -> PairTable {
    let mut p = PairTable::default();
    for &(f, s, e) in spec {
        p.th.push(0);
        p.flight.push(f);
        p.addr.push(ADDR);
        p.t_ms.push(T0_MS + s * 1000 + shift_ms);
        p.cross_m.push(e);
        p.along_m.push(0.0);
    }
    p
}

fn scalar(m: &MetricResult, k: &str) -> f64 {
    m.scalars.iter().find(|(n, _)| n == k).unwrap().1
}

// REQ: OA-BENCH-070
#[test]
fn the_duplicate_row_chosen_never_depends_on_its_error() {
    // System A: two rows for one aircraft and time from two flight records, 1,000 m (flight 0)
    // and 0 m (flight 1), one paired sample each, so the lowest flight index is the primary.
    let a = rows(&[(0, 0, 1000.0), (1, 0, 0.0)], 0);
    let b = rows(&[(0, 0, 10.0)], 0);
    let cfg = CommonTimeConfig {
        grid_ms: 10_000,
        tol_ms: 500,
    };
    let m = common_time_compare(&a, &b, &cfg);
    assert_eq!(row(&m, "horizontal, system A").rms, 1000.0);
    assert_eq!(row(&m, "horizontal, system B").rms, 10.0);
    assert_eq!(scalar(&m, "duplicate-track samples, system A"), 1.0);
    assert_eq!(scalar(&m, "duplicate-track samples, system B"), 0.0);
}

// REQ: OA-BENCH-070
#[test]
fn the_primary_track_is_the_flight_with_most_samples_whatever_the_grid_phase() {
    // Flight 3 has 60 samples at 100 m; flight 1 has 20 at 0 m at the same times as the start of
    // flight 3 (flight 1 has the lower index, flight 3 the more samples).
    let mut spec: Vec<(u32, i64, f32)> = (0..60).map(|s| (3, s, 100.0)).collect();
    spec.extend((0..20).map(|s| (1, s, 0.0)));
    let b: Vec<(u32, i64, f32)> = (0..60).map(|s| (0, s, 10.0)).collect();
    let cfg = CommonTimeConfig {
        grid_ms: 10_000,
        tol_ms: 2_000,
    };
    for shift in [0, 250, 700, 1_300, 4_100] {
        let m = common_time_compare(&rows(&spec, shift), &rows(&b, shift), &cfg);
        let ra = row(&m, "horizontal, system A");
        assert_eq!(ra.rms, 100.0, "shift {shift} ms");
        assert_eq!(ra.n, row(&m, "horizontal, system B").n, "shift {shift} ms");
        assert_eq!(
            scalar(&m, "duplicate-track samples, system A"),
            20.0,
            "shift {shift} ms"
        );
    }
}
