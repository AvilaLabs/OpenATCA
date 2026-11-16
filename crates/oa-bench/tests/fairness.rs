//! Fairness of the benchmark to the system under test: association that does not depend on the
//! error being measured, empty sets that never read as zero, and one treatment of the along-track
//! time offset for every scored source.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_bench::assoc::{associate, astatus, index_truth, AssocConfig};
use oa_bench::legacy::{new_legacy, push_th, set_addr, NONE, NO_ALT, NO_TIME};
use oa_bench::pair::{build_pairs, PairConfig};
use oa_bench::report::{code_version, render_csv, render_text, Header};
use oa_bench::run::{find, run_benchmark, score_track_samples, RunConfig};
use oa_bench::stats::{nearest_rank, stats_of, MetricResult};
use oa_bench::synth::{build_world, env, run_world, self_test_config, truth_at, Injected, T0_MS};
use oa_bench::thindex::build_th_index;
use oa_bench::tracker_input::{legacy_from_track_samples, TrackSamples};
use oa_bench::truth::{NoTruth, Truth, TruthLeg, TruthSample};

fn level_truth(t_ms: i64) -> TruthSample {
    let s = (t_ms - T0_MS) as f64 / 1000.0;
    TruthSample {
        lat: 41.0,
        lon: -88.0 + 200.0 * s / 82_000.0,
        baro_alt_ft: Some(30_000),
        vel_east_mps: 200.0,
        vel_north_mps: 0.0,
        vert_rate_fpm: Some(0),
        sigma95_h_m: 5.0,
        leg: 0,
    }
}

fn legs() -> [TruthLeg; 1] {
    [TruthLeg {
        addr: 0xABC001,
        start_ms: T0_MS,
        end_ms: T0_MS + 1_000_000,
        callsign: Some(*b"FAIR01\0\0"),
        start: Default::default(),
    }]
}

/// One flight whose samples are `dlat` degrees north of the truth.
fn one_flight(
    callsign: &[u8],
    addr: Option<u32>,
    dlat: f64,
) -> (oa_bench::assoc::Assoc, oa_bench::pair::PairTable) {
    let legs = legs();
    let q = |a: u32, t: i64| {
        if a == 0xABC001 {
            Ok(level_truth(t))
        } else {
            Err(NoTruth::UnknownAircraft)
        }
    };
    let mut leg = new_legacy();
    let f = oa_bench::legacy::flight_ix(&mut leg, 1).unwrap();
    leg.flights.callsign[f as usize][..callsign.len()].copy_from_slice(callsign);
    if let Some(a) = addr {
        set_addr(&mut leg, f, a);
    }
    for i in 0..10 {
        let t = T0_MS + 12_000 * i as i64;
        let ts = level_truth(t);
        push_th(
            &mut leg,
            f,
            t,
            NO_TIME,
            ((ts.lat + dlat) * 1e7) as i32,
            (ts.lon * 1e7) as i32,
            30_000,
            None,
            false,
        );
    }
    let truth = Truth {
        query: &q,
        legs: &legs,
    };
    let ix = build_th_index(&leg.th, 1);
    let tix = index_truth(&legs);
    let a = associate(&leg, &ix, &truth, &tix, &AssocConfig::default());
    let (p, _) = build_pairs(&leg, &ix, &a, &truth, &PairConfig::default());
    (a, p)
}

// REQ: OA-BENCH-035
#[test]
fn a_callsign_flight_is_kept_however_large_the_eram_error_is() {
    // 0.1 deg north is 6 NM: far outside the 3 NM check, inside any plausible region.
    let (a, p) = one_flight(b"FAIR01", None, 0.1);
    assert_eq!(a.addr[0], 0xABC001, "status {}", a.status[0]);
    assert_eq!(a.status[0], astatus::GATE_FAILED);
    assert_eq!(p.t_ms.len(), 10, "its errors are scored, not dropped");
    // 0.02 deg north is 1.2 NM: passes the check, associated the ordinary way.
    let (a, _) = one_flight(b"FAIR01", None, 0.02);
    assert_eq!(a.status[0], astatus::CALLSIGN);
}

// REQ: OA-BENCH-036
#[test]
fn an_address_whose_truth_is_nowhere_near_is_flagged_and_scored_separately() {
    // 5 degrees north is 300 NM: the address cannot name this track.
    let (a, p) = one_flight(b"FAIR01", Some(0xABC001), 5.0);
    assert_eq!(a.addr[0], NONE, "status {}", a.status[0]);
    assert!(p.t_ms.is_empty());
    // The same address with a believable error is trusted as before.
    let (a, p) = one_flight(b"FAIR01", Some(0xABC001), 0.1);
    assert_eq!(a.status[0], astatus::ADDRESS);
    assert_eq!(p.t_ms.len(), 10);
    // A callsign that disagrees with the truth leg's callsign flags the pair too.
    let (a, _) = one_flight(b"OTHER9", Some(0xABC001), 0.0);
    assert_eq!(a.addr[0], NONE, "status {}", a.status[0]);
}

// REQ: OA-BENCH-037
#[test]
fn statistics_of_nothing_are_not_a_number_and_print_as_no_data() {
    let s = stats_of(&[]);
    assert_eq!(s.n, 0);
    assert!(s.mean.is_nan() && s.rms.is_nan() && s.median.is_nan());
    assert!(s.p95.is_nan() && s.p99.is_nan() && s.max.is_nan());
    assert!(nearest_rank(&[], 50).is_nan());
    // A world with no truth at all: every table is empty and says so.
    let w = build_world(&Injected::default());
    let q = |_: u32, _: i64| Err(NoTruth::UnknownAircraft);
    let truth = Truth {
        query: &q,
        legs: &[],
    };
    let out = run_benchmark(&w.legacy, &truth, &env(), &RunConfig::default());
    let h = Header {
        mode: "synthetic".into(),
        code_version: code_version(),
        inputs: vec![],
        replay_hash: "none".into(),
        counts: vec![],
        notes: vec![],
        truth: vec![],
    };
    let text = render_text(&h, &out);
    let csv = render_csv(&h, &out);
    let t1 = text.lines().skip_while(|l| !l.starts_with("[T1]")).nth(1);
    assert!(t1.unwrap().contains("no data"), "{t1:?}");
    assert!(text.contains("no data"));
    let row = csv.lines().find(|l| l.starts_with("stat,T1,all,")).unwrap();
    assert_eq!(row, "stat,T1,all,0,NA,NA,NA,NA,NA,NA");
    assert!(!text.contains("NaN") && !csv.contains("NaN"));
    // No scalar of an empty set reads as zero.
    let t7 = find(&out, "T7").unwrap();
    assert!(scalar(t7, "presence").is_nan());
    assert!(scalar(find(&out, "R1").unwrap(), "max_flight_share").is_nan());
}

fn scalar(m: &MetricResult, k: &str) -> f64 {
    m.scalars
        .iter()
        .find(|(n, _)| n == k)
        .unwrap_or_else(|| panic!("{} has no scalar {k}", m.id))
        .1
}

// REQ: OA-BENCH-038
#[test]
fn raw_along_track_is_primary_and_the_offset_is_published_with_its_uncertainty() {
    let (_, out) = run_world(&Injected::default(), &self_test_config());
    let ids: Vec<&str> = out.metrics.iter().map(|m| m.id).collect();
    let (raw, off) = (
        ids.iter().position(|&i| i == "T2-raw").unwrap(),
        ids.iter().position(|&i| i == "T2-offset").unwrap(),
    );
    assert!(raw < off);
    assert!(out.metrics[raw].name.contains("primary"));
    assert!(out.metrics[off].name.contains("secondary"));
    let t2 = find(&out, "T2-offset").unwrap();
    let tau = scalar(t2, "fitted_offset_s");
    let (se, se_g, u95) = (
        scalar(t2, "fitted_offset_se_s"),
        scalar(t2, "fitted_offset_between_aircraft_se_s"),
        scalar(t2, "fitted_offset_u95_s"),
    );
    assert!(se > 0.0 && se_g > 0.0 && u95 >= 1.96 * se.max(se_g) - 1e-12);
    // The synthetic offset is within its own published uncertainty (plus the test tolerance).
    assert!((tau - 1.5).abs() <= u95 + 0.05, "{tau} {u95}");
    // The trimmed drift test is joined by the untrimmed worst group.
    assert!(scalar(t2, "max_group_dev_untrimmed_s") >= scalar(t2, "max_group_dev_s"));
    assert!(t2.notes.iter().any(|n| n.contains("worst aircraft groups")));
    assert!(t2.notes.iter().any(|n| n.contains("ERAM lag")));
}

// REQ: OA-BENCH-039
#[test]
fn a_tracker_is_scored_by_the_same_code_path_and_the_same_offset_treatment() {
    // A tracker that lags the truth by 2 s, as columns.
    let w = build_world(&Injected::default());
    let (mut addr, mut t_ns, mut lat, mut lon, mut alt) = (vec![], vec![], vec![], vec![], vec![]);
    for l in &w.legs {
        let mut t = l.start_ms + 5000;
        while t <= l.end_ms - 5000 {
            if let Ok(s) = truth_at(&w.aircraft, l.addr, t - 2000) {
                addr.push(l.addr);
                t_ns.push(t * 1_000_000);
                lat.push((s.lat * 1e7).round() as i32);
                lon.push((s.lon * 1e7).round() as i32);
                alt.push(s.baro_alt_ft.unwrap_or(NO_ALT));
            }
            t += 1000;
        }
    }
    let n = addr.len();
    let (ve, vn, coast) = (vec![f32::NAN; n], vec![f32::NAN; n], vec![false; n]);
    let cols = TrackSamples {
        addr: &addr,
        t_ns: &t_ns,
        lat_e7: &lat,
        lon_e7: &lon,
        alt_ft: &alt,
        vel_east_mps: &ve,
        vel_north_mps: &vn,
        coast: &coast,
    };
    let q = |a: u32, t: i64| truth_at(&w.aircraft, a, t);
    let truth = Truth {
        query: &q,
        legs: &w.legs,
    };
    let cfg = self_test_config();
    let out = score_track_samples(&cols, &truth, &env(), &cfg).unwrap();
    // The tracker entry point is the legacy adapter followed by the one benchmark function.
    let leg = legacy_from_track_samples(&cols).unwrap();
    let direct = run_benchmark(&leg, &truth, &env(), &cfg);
    // Compared as reports: scalars of empty sets are NaN and NaN is not equal to itself.
    let h = Header {
        mode: "synthetic".into(),
        code_version: code_version(),
        inputs: vec![],
        replay_hash: "none".into(),
        counts: vec![],
        notes: vec![],
        truth: vec![],
    };
    assert_eq!(render_csv(&h, &out), render_csv(&h, &direct));
    assert_eq!(out.exclusions, direct.exclusions);
    let (_, eram) = run_world(&Injected::default(), &cfg);
    // The same metric ids, in the same order, with the same offset publications.
    let ids = |o: &oa_bench::run::RunOutput| o.metrics.iter().map(|m| m.id).collect::<Vec<_>>();
    assert_eq!(ids(&out), ids(&eram));
    let keys = |o: &oa_bench::run::RunOutput| {
        find(o, "T2-offset")
            .unwrap()
            .scalars
            .iter()
            .map(|s| s.0.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(keys(&out), keys(&eram));
    // Raw along-track shows the lag (about 2 s at the aircraft speeds); the fitted offset is the
    // secondary number and is published with its uncertainty.
    let raw = find(&out, "T2-raw").unwrap().rows[0].stats;
    assert!(raw.rms > 100.0, "{}", raw.rms);
    let off = find(&out, "T2-offset").unwrap();
    assert!((scalar(off, "fitted_offset_s") + 2.0).abs() < 0.3);
    assert!(scalar(off, "fitted_offset_u95_s") > 0.0);
    // Sources of different kinds share the scalars and the notes that mark the raw number primary.
    assert!(off.notes.iter().any(|n| n.contains("ERAM lag")));
}
