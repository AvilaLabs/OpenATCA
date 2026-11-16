//! Exact limits of the truth builder and its queries: every boundary the design states, tested at
//! the value itself and one step either side, with answers worked out from the rules or the
//! Kalman/RTS equations rather than read back from the code.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_adsb::{
    reports_push, Reports, Row, F_STALE, NONE_I32, NONE_U16, NONE_U8, ROW_EMPTY, SPEED_ENVELOPE_MPS,
};
use oa_truth::*;

const LAT0: f64 = 41.0;
const LON0: f64 = -88.0;
const T0: i64 = 1_700_000_000_000_000_000;
const ADDR: u32 = 0xa12345;

fn ns(t_s: f64) -> i64 {
    T0 + (t_s * 1e9).round() as i64
}

fn good(addr: u32, t_s: f64, lat: f64, lon: f64) -> Row {
    let mut r = ROW_EMPTY;
    r.icao = addr;
    r.time_ns = ns(t_s);
    r.lat_e7 = (lat * 1e7).round() as i32;
    r.lon_e7 = (lon * 1e7).round() as i32;
    r.nic = 8;
    r.nac_p = 10;
    r.sil = 3;
    r.version = 2;
    r.source = 0;
    r
}

/// A report of a stationary aircraft.
fn still(t_s: f64) -> Row {
    good(ADDR, t_s, LAT0, LON0)
}

fn reports(rows: &[Row]) -> Reports {
    let mut r = Reports::default();
    for row in rows {
        reports_push(&mut r, row);
    }
    r
}

fn build_with(rows: &[Row], cfg: &TruthConfig) -> (Truth, TruthStats) {
    build_truth(&reports(rows), cfg).unwrap()
}

fn build(rows: &[Row]) -> (Truth, TruthStats) {
    build_with(rows, &DEFAULT_CONFIG)
}

fn removed(s: &TruthStats, kind: usize) -> u64 {
    s.removed[kind]
}

fn balanced(s: &TruthStats) -> bool {
    s.rows_in == s.rows_kept + s.removed.iter().sum::<u64>()
}

// REQ: OA-TRUTH-050
#[test]
fn every_configuration_limit_is_exact() {
    let ok = |f: &dyn Fn(&mut TruthConfig)| {
        let mut c = DEFAULT_CONFIG;
        f(&mut c);
        build_truth(&Reports::default(), &c).is_ok()
    };
    // NACp gate: 1 to 11.
    assert!(!ok(&|c| c.min_nacp = 0));
    assert!(ok(&|c| c.min_nacp = 1));
    assert!(ok(&|c| c.min_nacp = 11));
    assert!(!ok(&|c| c.min_nacp = 12));
    // NIC at most 11, SIL at most 3, version at most 7; zero is allowed.
    assert!(ok(&|c| c.min_nic = 0));
    assert!(ok(&|c| c.min_nic = 11));
    assert!(!ok(&|c| c.min_nic = 12));
    assert!(ok(&|c| c.required_sil = 0));
    assert!(ok(&|c| c.required_sil = 3));
    assert!(!ok(&|c| c.required_sil = 4));
    assert!(ok(&|c| c.min_version = 0));
    assert!(ok(&|c| c.min_version = 7));
    assert!(!ok(&|c| c.min_version = 8));
    // Time gates: the carry may be zero, the others must be positive; none over one day.
    assert!(!ok(&|c| c.quality_carry_ms = -1));
    assert!(ok(&|c| c.quality_carry_ms = 0));
    assert!(!ok(&|c| c.leg_gap_ms = 0));
    assert!(ok(&|c| c.leg_gap_ms = 1));
    assert!(!ok(&|c| c.bracket_ms = 0));
    assert!(ok(&|c| c.bracket_ms = 1));
    for set in [
        (|c: &mut TruthConfig, v: i64| c.quality_carry_ms = v) as fn(&mut TruthConfig, i64),
        |c, v| c.leg_gap_ms = v,
        |c, v| c.bracket_ms = v,
    ] {
        assert!(ok(&|c| set(c, 86_400_000)));
        assert!(!ok(&|c| set(c, 86_400_001)));
    }
    // Floating-point settings: finite and positive, the covariance ones with a cap.
    assert!(ok(&|c| c.max_sigma95_h_m = f64::MAX));
    assert!(!ok(&|c| c.max_sigma95_h_m = 0.0));
    assert!(!ok(&|c| c.max_sigma95_h_m = f64::NAN));
    assert!(!ok(&|c| c.max_jump_kt = 0.0));
    assert!(!ok(&|c| c.max_jump_kt = f64::INFINITY));
    assert!(ok(&|c| c.accel_q = MAX_ACCEL_Q));
    assert!(!ok(&|c| c.accel_q = MAX_ACCEL_Q + 1.0));
    assert!(!ok(&|c| c.accel_q = 0.0));
    assert!(ok(&|c| c.sigma_scale = MAX_SIGMA_SCALE));
    assert!(!ok(&|c| c.sigma_scale = MAX_SIGMA_SCALE + 1.0));
    assert!(!ok(&|c| c.sigma_scale = -1.0));
    // The jump allowance may be zero but not negative, infinite or NaN.
    assert!(ok(&|c| c.jump_slack_m = 0.0));
    assert!(ok(&|c| c.jump_slack_m = 1e9));
    assert!(!ok(&|c| c.jump_slack_m = -0.5));
    assert!(!ok(&|c| c.jump_slack_m = f64::NAN));
    assert!(!ok(&|c| c.jump_slack_m = f64::INFINITY));
    // Counts: at least 1 jump, 3 points, a stride of 2.
    assert!(!ok(&|c| c.jump_resync = 0));
    assert!(ok(&|c| c.jump_resync = 1));
    assert!(!ok(&|c| c.min_leg_points = 2));
    assert!(ok(&|c| c.min_leg_points = 3));
    assert!(!ok(&|c| c.loo_stride = 1));
    assert!(ok(&|c| c.loo_stride = 2));
    // The refusal names the value.
    let mut c = DEFAULT_CONFIG;
    c.min_nacp = 12;
    assert_eq!(
        build_truth(&Reports::default(), &c).unwrap_err(),
        TruthError::BadConfig("min_nacp")
    );
}

// REQ: OA-TRUTH-050
#[test]
fn the_report_limit_and_the_error_texts_are_the_documented_ones() {
    assert_eq!(MAX_REPORTS, 209_715_200);
    assert!((MAX_FRAME_ARC_RAD - 3_000_000.0 / 6_371_000.0).abs() < 1e-12);
    assert_eq!(
        TruthError::ColumnLength.to_string(),
        "report columns have different lengths"
    );
    assert_eq!(
        TruthError::TooManyReports(209_715_201).to_string(),
        "209715201 reports exceeds the limit of 209715200"
    );
    assert_eq!(
        TruthError::BadConfig("min_nacp").to_string(),
        "bad configuration: min_nacp"
    );
    assert_eq!(TruthError::Capacity.to_string(), "index capacity exceeded");
}

// REQ: OA-TRUTH-053
#[test]
fn a_leg_that_reaches_3000_km_from_its_origin_is_split_there_and_counted() {
    // North at 0.2 degrees (22.2 km) every 100 s, about 432 kt: no jump. The angle from the first
    // report passes 3,000 km / 6,371 km = 0.4709 rad (26.98 degrees) between the reports at 26.8
    // and 27.0 degrees, i.e. at the 136th report (index 135).
    let rows: Vec<Row> = (0..150)
        .map(|k| good(ADDR, f64::from(k) * 100.0, 0.2 * f64::from(k), 10.0))
        .collect();
    let (truth, s) = build(&rows);
    assert!(balanced(&s));
    assert_eq!(s.splits[SPLIT_FRAME], 1);
    assert_eq!(s.splits.iter().sum::<u64>(), 1);
    assert_eq!(s.legs, 2);
    assert_eq!(leg_reports(&truth, LegIx(0)), Some(135));
    assert_eq!(leg_reports(&truth, LegIx(1)), Some(15));
    assert_eq!(leg_end_ns(&truth, LegIx(0)), Some(ns(134.0 * 100.0)));
    assert_eq!(leg_start_ns(&truth, LegIx(1)), Some(ns(135.0 * 100.0)));
    // A leg that stays inside the range is not split: 26.8 degrees from the origin.
    let (_, s) = build(&rows[..135]);
    assert_eq!((s.splits[SPLIT_FRAME], s.legs), (0, 1));
}

/// Twenty stationary reports before and after an instant (20 s) carrying `at_instant`.
fn instant_track(at_instant: &[Row]) -> Vec<Row> {
    let mut rows: Vec<Row> = (0..20).map(|k| still(f64::from(k))).collect();
    rows.extend_from_slice(at_instant);
    rows.extend((21..41).map(|k| still(f64::from(k))));
    rows
}

fn kept_at_instant(t: &Truth) -> Option<usize> {
    t.pt_t_ns.iter().position(|&x| x == ns(20.0))
}

/// Which of two same-instant rows is kept follows the preference order baro altitude, geometric
/// altitude, vertical rate, ground speed, track; checked in both input orders.
// REQ: OA-TRUTH-051
#[test]
fn the_kept_row_of_an_instant_follows_the_preference_order_field_by_field() {
    let at = || still(20.0);
    // (winner, loser, what the kept point shows): the loser has the field next in the order.
    let mut baro = at();
    baro.baro_alt_ft = 10_000;
    let mut vr_only = at();
    vr_only.vrate_fpm = 500;
    let mut geom = at();
    geom.geom_alt_ft = 10_000;
    let mut gs = at();
    gs.gs_dkt = 1000;
    for (a, b) in [(&baro, &vr_only), (&vr_only, &baro)] {
        let (t, s) = build(&instant_track(&[*a, *b]));
        assert!(balanced(&s));
        let i = kept_at_instant(&t).unwrap();
        // Barometric altitude beats a vertical rate: altitude kept, no rate.
        assert_eq!((t.pt_alt_ft[i], t.pt_vrate_fpm[i]), (10_000, NONE_I32));
    }
    for (a, b) in [(&geom, &vr_only), (&vr_only, &geom)] {
        let (t, _) = build(&instant_track(&[*a, *b]));
        let i = kept_at_instant(&t).unwrap();
        // Geometric altitude beats a vertical rate: the kept row has no rate.
        assert_eq!(t.pt_vrate_fpm[i], NONE_I32);
    }
    for (a, b) in [(&vr_only, &gs), (&gs, &vr_only)] {
        let (t, _) = build(&instant_track(&[*a, *b]));
        let i = kept_at_instant(&t).unwrap();
        // A vertical rate beats a ground speed: the kept row has the rate.
        assert_eq!(t.pt_vrate_fpm[i], 500);
    }
}

/// Ground speed ahead of track: the ground speed of the kept row is what the next jump check
/// compares with, so the preference shows in whether a 500 m step one second later is a jump.
// REQ: OA-TRUTH-051
#[test]
fn a_row_with_a_ground_speed_is_kept_over_one_with_only_a_track() {
    let with_gs = || {
        let mut r = still(20.0);
        r.gs_dkt = 1000; // 100 kt
        r
    };
    let with_track = || {
        let mut r = still(20.0);
        r.track_ddeg = 900;
        r
    };
    let step_after = |instant: &[Row]| {
        // The report at 21 s is 500 m north of the instant: 500 m in 1 s is plausible with no
        // ground speed known (ceiling 2000 kt) but not for 100 kt (1.2 * 100 + 40 + 2 = 162 kt =
        // 83 m/s, plus the 200 m allowance = 283 m).
        let mut rows = instant_track(instant);
        let next = rows.iter().position(|r| r.time_ns == ns(21.0)).unwrap();
        rows[next] = good(ADDR, 21.0, LAT0 + 500.0 / 111_195.0, LON0);
        build(&rows)
    };
    for pair in [[with_gs(), with_track()], [with_track(), with_gs()]] {
        let (_, s) = step_after(&pair);
        assert_eq!(removed(&s, REM_JUMP), 1, "{pair:?}");
    }
    // Control: the same step after a row that has only a track is not a jump.
    let (_, s) = step_after(&[with_track()]);
    assert_eq!(removed(&s, REM_JUMP), 0);
}

// REQ: OA-TRUTH-051
#[test]
fn altitudes_of_one_instant_more_than_25_ft_apart_drop_the_instant_per_altitude_kind() {
    let row = |baro: i32, geom: i32| {
        let mut r = still(20.0);
        r.baro_alt_ft = baro;
        r.geom_alt_ft = geom;
        r
    };
    let run = |a: Row, b: Row| {
        let (t, s) = build(&instant_track(&[a, b]));
        assert!(balanced(&s));
        (kept_at_instant(&t).is_some(), s)
    };
    // Geometric altitudes 100 ft apart: both rows dropped, counted as ambiguous.
    let (kept, s) = run(row(NONE_I32, 10_000), row(NONE_I32, 10_100));
    assert!(!kept);
    assert_eq!(removed(&s, REM_AMBIGUOUS), 2);
    // 26 ft apart is over the quantum, 25 ft is not (the second row is then a duplicate time).
    let (kept, s) = run(row(NONE_I32, 10_000), row(NONE_I32, 10_026));
    assert!(!kept && removed(&s, REM_AMBIGUOUS) == 2);
    let (kept, s) = run(row(NONE_I32, 10_000), row(NONE_I32, 10_025));
    assert!(kept);
    assert_eq!(
        (removed(&s, REM_AMBIGUOUS), removed(&s, REM_DUPLICATE)),
        (0, 1)
    );
    // One geometric altitude absent is not a contradiction.
    let (kept, s) = run(row(NONE_I32, 10_000), row(NONE_I32, NONE_I32));
    assert!(kept);
    assert_eq!(removed(&s, REM_AMBIGUOUS), 0);
    // The same for barometric altitudes.
    let (kept, s) = run(row(10_000, NONE_I32), row(10_026, NONE_I32));
    assert!(!kept && removed(&s, REM_AMBIGUOUS) == 2);
    let (kept, s) = run(row(10_000, NONE_I32), row(10_025, NONE_I32));
    assert!(kept && removed(&s, REM_AMBIGUOUS) == 0);
}

// REQ: OA-TRUTH-051
#[test]
fn the_conflict_check_measures_every_pair_of_distinct_same_instant_positions_once() {
    // n distinct eligible positions 1 mm apart (one position measured n times, no conflict).
    for (n, pairs) in [(2u64, 1u64), (3, 3), (4, 6), (5, 10)] {
        let at: Vec<Row> = (0..n)
            .map(|k| {
                let mut r = still(20.0);
                r.lon_e7 += k as i32;
                r
            })
            .collect();
        let before = pair_evals_this_thread();
        let (t, s) = build(&instant_track(&at));
        assert_eq!(pair_evals_this_thread() - before, pairs, "{n} rows");
        assert!(kept_at_instant(&t).is_some());
        assert_eq!(removed(&s, REM_DUPLICATE), n - 1);
    }
    // A single report is not checked against anything.
    let before = pair_evals_this_thread();
    build(&instant_track(&[still(20.0)]));
    assert_eq!(pair_evals_this_thread(), before);
}

// REQ: OA-TRUTH-052
#[test]
fn a_report_that_fails_on_quality_still_sets_the_value_the_reports_after_it_carry() {
    // Ten good reports, one with NACp 7 (fails), five that print no NACp (they carry the 7 and
    // fail with it), then good ones with their own NACp again.
    let mut rows: Vec<Row> = (0..10).map(|k| still(f64::from(k))).collect();
    let mut bad = still(10.0);
    bad.nac_p = 7;
    rows.push(bad);
    for k in 11..16 {
        let mut r = still(f64::from(k));
        r.nac_p = NONE_U8;
        rows.push(r);
    }
    rows.extend((16..26).map(|k| still(f64::from(k))));
    let (_, s) = build(&rows);
    assert_eq!(removed(&s, REM_NACP), 6);
    assert_eq!(removed(&s, REM_NO_QUALITY), 0);
    assert_eq!(s.rows_kept, 20);
    assert!(balanced(&s));
    // One of several rows of an instant that fail does not set it: the later reports carry the 10.
    let mut rows: Vec<Row> = (0..10).map(|k| still(f64::from(k))).collect();
    for _ in 0..2 {
        let mut bad = still(10.0);
        bad.nac_p = 7;
        rows.push(bad);
    }
    for k in 11..16 {
        let mut r = still(f64::from(k));
        r.nac_p = NONE_U8;
        rows.push(r);
    }
    let (_, s) = build(&rows);
    assert_eq!(removed(&s, REM_NACP), 2);
    assert_eq!(s.rows_kept, 15);
}

// REQ: OA-TRUTH-054
#[test]
fn the_measurement_sigma_is_the_nacp_radius_over_sigma95_times_the_scale() {
    // Three stationary reports, NACp 10 (10 m 95 % radius). At the first report the prior
    // variance P = 1e6 m^2 meets the report variance r = (10 / SIGMA95 * scale)^2: the filtered
    // position variance is P r / (P + r); velocity and acceleration are the prior (62500, 900).
    for scale in [1.0, 3.0] {
        let cfg = TruthConfig {
            sigma_scale: scale,
            loo: false,
            ..DEFAULT_CONFIG
        };
        let (t, _) = build_with(&[still(0.0), still(1.0), still(2.0)], &cfg);
        let r = (10.0 / SIGMA95 * scale).powi(2);
        let p = 1.0e6 * r / (1.0e6 + r);
        let got = t.pt_pf[0];
        assert!(
            (got[0] - p).abs() < 1e-9 * p,
            "scale {scale}: {got:?} vs {p}"
        );
        assert!(got[1].abs() < 1e-9 && got[2].abs() < 1e-9 && got[4].abs() < 1e-9);
        assert!((got[3] - 62_500.0).abs() < 1e-6 && (got[5] - 900.0).abs() < 1e-9);
    }
}

// REQ: OA-TRUTH-054
#[test]
fn the_uncertainty_shares_count_intervals_up_to_twice_the_bracket_against_the_gate() {
    // Reports 0, 20, 40, 60 and 81 s, NACp 11 (3 m): at the reports the smoothed 95 % radius is
    // 3.00 m. At the midpoints of the three intervals of 20 s it is 63.5, 42.3 and 42.7 m (the
    // Kalman/RTS equations at q = 0.1, the covariance does not depend on the positions); the
    // 21 s interval is longer than twice the bracket and has no midpoint.
    let rows: Vec<Row> = [0.0, 20.0, 40.0, 60.0, 81.0]
        .iter()
        .map(|&t| {
            let mut r = still(t);
            r.nac_p = 11;
            r
        })
        .collect();
    let (_, s) = build(&rows);
    assert_eq!((s.unc_at_reports.n, s.unc_at_reports.hit), (5, 5));
    assert_eq!((s.unc_at_midpoints.n, s.unc_at_midpoints.hit), (3, 2));
    let with_gate = |g: f64| {
        let cfg = TruthConfig {
            max_sigma95_h_m: g,
            ..DEFAULT_CONFIG
        };
        let (_, s) = build_with(&rows, &cfg);
        (s.unc_at_reports, s.unc_at_midpoints)
    };
    let (rep, mid) = with_gate(1.0e9);
    assert_eq!((rep.hit, mid.hit, mid.n), (5, 3, 3));
    let (rep, mid) = with_gate(1.0);
    assert_eq!((rep.n, rep.hit, mid.n, mid.hit), (5, 0, 3, 0));
}

/// A truth with one leg of the given report times and altitudes about the frame origin, the same
/// state at every report.
fn crafted(t_ns: &[i64], alt: &[i32], vr: &[i32], x: [f64; 6], p: [f64; 6]) -> Truth {
    let n = t_ns.len();
    let mut t = Truth {
        cfg: TruthConfig {
            loo: false,
            ..DEFAULT_CONFIG
        },
        ..Truth::default()
    };
    t.ac_addr.push(ADDR);
    t.ac_first_leg = vec![0, 1];
    t.leg_addr.push(ADDR);
    t.leg_first_pt = vec![0, n as u32];
    t.leg_lat0.push(0.0);
    t.leg_lon0.push(0.0);
    t.pt_t_ns = t_ns.to_vec();
    t.pt_alt_ft = alt.to_vec();
    t.pt_vrate_fpm = vr.to_vec();
    for _ in 0..n {
        t.pt_xf.push(x);
        t.pt_xs.push(x);
        t.pt_pf.push(p);
        t.pt_ps.push(p);
    }
    t
}

const X: [f64; 6] = [1.0e4, 100.0, 0.0, 2.0e4, 50.0, 0.0];
const P: [f64; 6] = [1.0e-4, 0.0, 0.0, 1.0e-6, 0.0, 1.0e-6];

fn at_s(t: &Truth, s: f64) -> TruthQuery {
    truth_at(t, ADDR, ns(s) / 1_000_000)
}

// REQ: OA-TRUTH-055
#[test]
fn the_uncertainty_gate_is_inclusive() {
    let limit = sigma95_h_m(P[0]).unwrap();
    let mut t = crafted(&[ns(0.0), ns(1.0)], &[0, 0], &[0, 0], X, P);
    t.cfg.max_sigma95_h_m = limit;
    // A knot time returns the smoothed covariance itself: exactly the limit is truth.
    assert!(matches!(at_s(&t, 0.0), TruthQuery::Ok(_)));
    t.cfg.max_sigma95_h_m = limit * (1.0 - 1e-9);
    assert_eq!(at_s(&t, 0.0), TruthQuery::TooUncertain);
}

// REQ: OA-TRUTH-055
#[test]
fn a_velocity_past_the_speed_envelope_is_not_truth() {
    // The frame metre is the sphere's and the true metre the ellipsoid's (up to 0.3 % apart), so
    // the envelope is tested well inside and well outside it.
    let q = |ve: f64| {
        let x = [0.0, ve, 0.0, 0.0, 0.0, 0.0];
        at_s(&crafted(&[ns(0.0), ns(1.0)], &[0, 0], &[0, 0], x, P), 0.0)
    };
    assert!(matches!(q(0.99 * SPEED_ENVELOPE_MPS), TruthQuery::Ok(_)));
    assert_eq!(q(1.01 * SPEED_ENVELOPE_MPS), TruthQuery::TooUncertain);
}

// REQ: OA-TRUTH-055
#[test]
fn vertical_rate_is_interpolated_from_the_rates_else_from_the_altitudes_over_at_least_2_s() {
    let sample = |q: TruthQuery| match q {
        TruthQuery::Ok(s) => s,
        other => panic!("{other:?}"),
    };
    let t4 = [ns(0.0), ns(4.0)];
    // Both rates present: 500 and 700 fpm, a quarter of the way: 550; the altitudes are level.
    let t = crafted(&t4, &[10_000, 10_000], &[500, 700], X, P);
    let s = sample(at_s(&t, 1.0));
    assert!((s.vert_rate_fpm.unwrap() - 550.0).abs() < 1e-9);
    assert!((s.baro_alt_ft.unwrap() - 10_000.0).abs() < 1e-9);
    // One rate missing: the altitude difference over the bracket, 100 ft in 4 s = 1500 fpm.
    for vr in [[NONE_I32, 700], [500, NONE_I32], [NONE_I32, NONE_I32]] {
        let t = crafted(&t4, &[10_000, 10_100], &vr, X, P);
        let s = sample(at_s(&t, 1.0));
        assert!((s.vert_rate_fpm.unwrap() - 1500.0).abs() < 1e-9, "{vr:?}");
        assert!((s.baro_alt_ft.unwrap() - 10_025.0).abs() < 1e-9);
    }
    // A bracket of exactly 2 s is wide enough (100 ft in 2 s = 3000 fpm), 1 s is not.
    let t2 = crafted(&[ns(0.0), ns(2.0)], &[10_000, 10_100], &[NONE_I32; 2], X, P);
    assert!((sample(at_s(&t2, 1.0)).vert_rate_fpm.unwrap() - 3000.0).abs() < 1e-9);
    let t1 = crafted(&[ns(0.0), ns(1.0)], &[10_000, 10_100], &[NONE_I32; 2], X, P);
    assert_eq!(sample(at_s(&t1, 0.5)).vert_rate_fpm, None);
    // No altitude, no rate: nothing.
    let t = crafted(&t4, &[NONE_I32; 2], &[NONE_I32; 2], X, P);
    let s = sample(at_s(&t, 1.0));
    assert_eq!((s.vert_rate_fpm, s.baro_alt_ft), (None, None));
}

// REQ: OA-TRUTH-055
#[test]
fn leg_times_round_inward_and_an_empty_leg_has_no_time() {
    // A leg from 1.5 ms to 1001.5 ms: first whole millisecond inside is 2, last is 1001.
    let t = crafted(&[1_500_000, 1_001_500_000], &[0, 0], &[0, 0], X, P);
    assert_eq!(leg_start_ns(&t, LegIx(0)), Some(1_500_000));
    assert_eq!(leg_start_ms(&t, LegIx(0)), Some(2));
    assert_eq!(leg_end_ms(&t, LegIx(0)), Some(1001));
    // On a whole millisecond, and before 1970 (-1.5 ms rounds up to -1).
    let t = crafted(&[2_000_000, 3_000_000], &[0, 0], &[0, 0], X, P);
    assert_eq!(leg_start_ms(&t, LegIx(0)), Some(2));
    assert_eq!(leg_end_ms(&t, LegIx(0)), Some(3));
    let t = crafted(&[-1_500_000, 500_000], &[0, 0], &[0, 0], X, P);
    assert_eq!(leg_start_ms(&t, LegIx(0)), Some(-1));
    assert_eq!(leg_end_ms(&t, LegIx(0)), Some(0));
    // A leg with no reports has no start, end or count.
    let mut t = crafted(&[ns(0.0), ns(1.0)], &[0, 0], &[0, 0], X, P);
    t.ac_first_leg = vec![0, 2];
    t.leg_addr.push(ADDR);
    t.leg_lat0.push(0.0);
    t.leg_lon0.push(0.0);
    t.leg_first_pt = vec![0, 0, 2];
    assert_eq!(leg_start_ns(&t, LegIx(0)), None);
    assert_eq!(leg_end_ns(&t, LegIx(0)), None);
    assert_eq!(leg_reports(&t, LegIx(0)), None);
    assert_eq!(leg_reports(&t, LegIx(1)), Some(2));
}

fn verdict(f: impl Fn(&mut Row)) -> QualityVerdict {
    let mut row = still(0.0);
    f(&mut row);
    quality_check(&mut QualityState::default(), &row, ns(0.0), &DEFAULT_CONFIG)
}

// REQ: OA-TRUTH-056
#[test]
fn each_quality_gate_passes_at_its_limit_and_fails_one_step_below() {
    assert_eq!(verdict(|_| {}).removal, None);
    assert_eq!(verdict(|r| r.nac_p = 8).removal, None);
    assert_eq!(verdict(|r| r.nac_p = 7).removal, Some(REM_NACP));
    assert_eq!(verdict(|r| r.nac_p = 11).removal, None);
    assert_eq!(verdict(|r| r.nac_p = 12).removal, Some(REM_NACP));
    assert_eq!(verdict(|r| r.nic = 7).removal, None);
    assert_eq!(verdict(|r| r.nic = 6).removal, Some(REM_NIC));
    assert_eq!(verdict(|r| r.sil = 3).removal, None);
    assert_eq!(verdict(|r| r.sil = 2).removal, Some(REM_SIL));
    assert_eq!(verdict(|r| r.version = 2).removal, None);
    assert_eq!(verdict(|r| r.version = 1).removal, Some(REM_VERSION));
    assert_eq!(verdict(|r| r.flags |= F_STALE).removal, Some(REM_STALE));
    assert_eq!(verdict(|r| r.source = 5).removal, Some(REM_SOURCE));
}

// REQ: OA-TRUTH-056
#[test]
fn a_row_with_any_one_of_nacp_nic_or_sil_missing_has_no_quality() {
    assert_eq!(verdict(|r| r.nac_p = NONE_U8).removal, Some(REM_NO_QUALITY));
    assert_eq!(verdict(|r| r.nic = NONE_U8).removal, Some(REM_NO_QUALITY));
    assert_eq!(verdict(|r| r.sil = NONE_U8).removal, Some(REM_NO_QUALITY));
    // The version is only checked where present.
    assert_eq!(verdict(|r| r.version = NONE_U8).removal, None);
}

// REQ: OA-TRUTH-056
#[test]
fn the_navigation_tables_are_the_standards_values() {
    let inf = f64::INFINITY;
    for (v, want) in [
        (0u8, inf),
        (1, 10.0),
        (2, 3.0),
        (3, 1.0),
        (4, 0.3),
        (5, inf),
        (255, inf),
    ] {
        assert_eq!(nacv_epu_mps(v), want, "NACv {v}");
    }
    for (v, want) in [
        (0u8, inf),
        (1, 18_520.0),
        (8, 92.6),
        (9, 30.0),
        (10, 10.0),
        (11, 3.0),
        (12, inf),
    ] {
        assert_eq!(nacp_epu_m(v), want, "NACp {v}");
    }
    let _ = NONE_U16;
}
