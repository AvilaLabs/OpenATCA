//! The scoring path: the split, the scoring configuration, the straight and turning classes, the
//! run on a synthetic clip with known truth, the binary, and the real clip (skipped when absent).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use std::path::PathBuf;
use std::process::Command;

use common::{frame0, Manoeuvre, Rng, SIGMA_POS};
use oa_adsb::{
    encode_payload, reports_len, reports_push, reports_row, Reports, Row, NONE_U8, ROW_EMPTY,
};
use oa_bench::adsb_truth;
use oa_feed::{encode_frame, FLAG_BYTES, MAX_FRAME_LEN};
use oa_geo::aeqd::{try_frame_velocity, unproject};
use oa_track::adsb::{adsb_measurements, AdsbConfig};
use oa_track::score::{
    esassp_of, flight_error_summaries, initiation_delays, interval_stats, run_tracker,
    run_tracker_spilled, score, score_ranges, score_source, scored_altitude_ft, scoring_config,
    split_reports, split_reports_with, transversal_accel, RowSet, RowSource, CLASS_STRAIGHT,
    CLASS_TURN, SCORE_CHUNK_ROWS, SCORE_DROP_AFTER_NS, TURN_ACCEL_MPS2,
};
use oa_track::{DEFAULT_CONFIG, END_FLUSHED, O_HAS_ALT, O_TICK};
use oa_truth::DEFAULT_CONFIG as TRUTH;

const T0: i64 = 1_700_000_000_000_000_000;

fn base_row(addr: u32, t_s: f64) -> Row {
    Row {
        icao: addr,
        time_ns: T0 + (t_s * 1.0e9).round() as i64,
        nic: 8,
        nac_p: 9,
        nac_v: 2,
        sil: 3,
        version: 2,
        source: 0,
        baro_alt_ft: 30_000,
        ..ROW_EMPTY
    }
}

// REQ: OA-TRK-023
#[test]
fn the_split_is_alternate_reports_per_aircraft_disjoint_and_complete() {
    let mut r = Reports::default();
    // Two aircraft, interleaved in the input and out of time order; aircraft 2 has two reports
    // at the same time (input order decides).
    let times = [5.0, 1.0, 3.0, 2.0, 4.0, 0.0, 6.0];
    for (k, &t) in times.iter().enumerate() {
        for addr in [0x100001u32, 0x100002] {
            let mut row = base_row(addr, t);
            row.lat_e7 = 410_000_000 + (k as i32) * 10;
            reports_push(&mut r, &row);
        }
    }
    let mut dup = base_row(0x100002, 3.0);
    dup.lat_e7 = 999;
    reports_push(&mut r, &dup);
    let (a, b, st) = split_reports(&r);
    assert_eq!((st.rows_in, st.aircraft), (15, 2));
    assert_eq!(st.set_a + st.set_b, 15);
    assert_eq!(reports_len(&a) as u64, st.set_a);
    // Aircraft 1 sorted by time: 0,1,2,3,4,5,6 -> even positions (0,2,4,6 s) in A.
    let times_of = |s: &Reports, addr: u32| -> Vec<i64> {
        (0..reports_len(s))
            .filter(|&i| s.icao[i] == addr)
            .map(|i| (s.time_ns[i] - T0) / 1_000_000_000)
            .collect()
    };
    assert_eq!(times_of(&a, 0x100001), vec![0, 2, 4, 6]);
    assert_eq!(times_of(&b, 0x100001), vec![1, 3, 5]);
    // Aircraft 2 has eight reports in seven times; the two at 3 s differ in content and stay
    // together: times 0,1,2,3(2 reports),4,5,6 -> A gets groups 0,2,4,6 = 0,2,4,6.
    assert_eq!(times_of(&a, 0x100002), vec![0, 2, 4, 6]);
    assert_eq!(times_of(&b, 0x100002), vec![1, 3, 3, 5]);
    assert_eq!(
        (st.duplicates_removed, st.conflict_groups, st.conflict_rows),
        (0, 1, 2)
    );
    // The two halves are disjoint and together every input row (by position and time).
    let key = |s: &Reports| -> Vec<(u32, i64, i32)> {
        (0..reports_len(s))
            .map(|i| (s.icao[i], s.time_ns[i], s.lat_e7[i]))
            .collect()
    };
    let mut all = key(&a);
    all.extend(key(&b));
    all.sort_unstable();
    let mut orig = key(&r);
    orig.sort_unstable();
    assert_eq!(all, orig);
    // The result does not depend on the order of the input.
    let mut rev = Reports::default();
    for i in (0..reports_len(&r)).rev() {
        reports_push(&mut rev, &reports_row(&r, i).unwrap());
    }
    let (a2, _, _) = split_reports(&rev);
    let (mut ka, mut ka2) = (key(&a), key(&a2));
    ka.sort_unstable();
    ka2.sort_unstable();
    assert_eq!(ka, ka2);
}

// REQ: OA-TRK-308
#[test]
fn siblings_differing_in_source_or_stale_are_not_copies_and_input_order_does_not_matter() {
    let mut current = base_row(0x100009, 4.0);
    current.lat_e7 = 410_000_000;
    let mut stale = current;
    stale.flags = oa_adsb::F_STALE;
    let mut mlat = current;
    mlat.source = 3;
    let rows = [stale, mlat, current];
    let split_of = |order: &[usize]| {
        let mut r = Reports::default();
        // A neighbour before and after, so the group lands on either side.
        reports_push(&mut r, &base_row(0x100009, 0.0));
        for &i in order {
            reports_push(&mut r, &rows[i]);
        }
        let (a, b, st) = split_reports(&r);
        let dump = |s: &Reports| -> Vec<Row> {
            (0..reports_len(s))
                .map(|i| reports_row(s, i).unwrap())
                .collect()
        };
        (dump(&a), dump(&b), st)
    };
    let (a1, b1, st1) = split_of(&[0, 1, 2]);
    let (a2, b2, st2) = split_of(&[2, 1, 0]);
    assert_eq!(
        (
            st1.duplicates_removed,
            st1.conflict_groups,
            st1.conflict_rows
        ),
        (0, 1, 3)
    );
    assert_eq!(st1, st2);
    assert_eq!(a1, a2);
    assert_eq!(b1, b2);
    assert_eq!(a1.len() + b1.len(), 4);
}

// REQ: OA-TRK-024
#[test]
fn quality_fields_are_carried_over_the_whole_sequence_before_the_split() {
    // NACp printed only on the second report; the first (set A) has none of its own.
    let mut r = Reports::default();
    for (k, t) in [0.0, 5.0, 10.0, 15.0].into_iter().enumerate() {
        let mut row = base_row(0x200001, t);
        if k != 1 {
            row.nac_p = NONE_U8;
            row.nic = NONE_U8;
            row.sil = NONE_U8;
            row.version = NONE_U8;
        }
        reports_push(&mut r, &row);
    }
    let (a, b, _) = split_reports(&r);
    // A holds the reports at 0 s and 10 s; B those at 5 s and 15 s. The 10 s report inherits from
    // the 5 s one (carry within 60 s); the 0 s report has nothing before it and stays unqualified.
    assert_eq!(a.nac_p, vec![NONE_U8, 9]);
    assert_eq!(b.nac_p, vec![9, 9]);
    let (_, st) = adsb_measurements(&a, &AdsbConfig::default());
    assert_eq!((st.passed, st.removed[3]), (1, 1));
}

// REQ: OA-TRK-025
#[test]
fn the_scoring_configuration_comes_from_the_measured_intervals() {
    let mut r = Reports::default();
    // 10 s apart for one aircraft, 40 s for another.
    for k in 0..20 {
        reports_push(&mut r, &base_row(0x300001, f64::from(k) * 10.0));
    }
    for k in 0..4 {
        reports_push(&mut r, &base_row(0x300002, f64::from(k) * 40.0));
    }
    let (m, _) = adsb_measurements(&r, &AdsbConfig::default());
    let iv = interval_stats(&m).unwrap();
    assert_eq!(
        (iv.n, iv.p50_ns, iv.p90_ns),
        (22, 10_000_000_000, 40_000_000_000)
    );
    let c = scoring_config(&DEFAULT_CONFIG, &iv);
    assert_eq!(c.expected_interval_ns, 10_000_000_000);
    assert_eq!(c.init_interval_ns, 40_000_000_000);
    assert_eq!(c.drop_after_ns, SCORE_DROP_AFTER_NS);
    assert_eq!(c.drop_after_ns, 120_000_000_000);
    assert!(interval_stats(&[]).is_none());
}

// REQ: OA-TRK-026
#[test]
fn turns_are_classified_by_transversal_acceleration() {
    // 230 m/s at 3 deg/s: 230 * 0.05236 = 12 m/s^2, a turn; 0.3 deg/s is 1.2 m/s^2, straight
    // (ESASSP 4.2.14: more than 1.5 m/s^2 is a turn).
    let v = |h_deg: f64| {
        (
            230.0 * h_deg.to_radians().sin(),
            230.0 * h_deg.to_radians().cos(),
        )
    };
    let a_turn = transversal_accel(v(0.0), v(18.0), 6.0);
    assert!((a_turn - 12.05).abs() < 0.1, "{a_turn}");
    let a_line = transversal_accel(v(0.0), v(1.8), 6.0);
    assert!(a_line < TURN_ACCEL_MPS2 && a_line > 1.0, "{a_line}");
    assert_eq!(transversal_accel(v(10.0), v(10.0), 6.0), 0.0);
    // Wrap-around at north: 359 to 1 degrees is 2 degrees, not 358.
    assert!(transversal_accel(v(359.0), v(1.0), 6.0) < 2.0);
}

/// A synthetic hour-less world: three aircraft, one straight, one turning at 3 deg/s, one
/// accelerating, reporting every 2 s for 600 s, with NACp 9 noise.
fn synthetic_reports() -> Reports {
    let f = frame0();
    let mans = [
        (0xA10001u32, Manoeuvre::straight(200.0, 45.0), 0.0, 0.0),
        (
            0xA10002,
            Manoeuvre::turning(220.0, 10.0, 150.0, 330.0, 3.0),
            20_000.0,
            0.0,
        ),
        (
            0xA10003,
            Manoeuvre {
                accel: 0.3,
                ..Manoeuvre::straight(120.0, 300.0)
            },
            0.0,
            30_000.0,
        ),
    ];
    let mut rng = Rng(99);
    let mut r = Reports::default();
    for (addr, man, e0, n0) in &mans {
        for k in 0..300 {
            let t = f64::from(k) * 2.0;
            let tr = man.at(t, *e0, *n0);
            let (lat, lon) = unproject(
                &f,
                tr.e + SIGMA_POS * rng.normal(),
                tr.n + SIGMA_POS * rng.normal(),
            )
            .unwrap();
            let (te, tn) = try_frame_velocity(&f, tr.e, tr.n, tr.ve, tr.vn).unwrap();
            let mut row = base_row(*addr, t);
            row.lat_e7 = (lat * 1e7).round() as i32;
            row.lon_e7 = (lon * 1e7).round() as i32;
            let speed = te.hypot(tn) / 0.514_444_444_444 * 10.0;
            row.gs_dkt = speed.round() as u16;
            row.track_ddeg = ((te.atan2(tn).to_degrees().rem_euclid(360.0)) * 10.0).round() as u16;
            reports_push(&mut r, &row);
        }
    }
    r
}

// REQ: OA-TRK-027
#[test]
fn a_synthetic_clip_is_scored_end_to_end_with_known_truth() {
    let reports = synthetic_reports();
    let (a, b, st) = split_reports(&reports);
    assert_eq!((st.set_a, st.set_b), (450, 450));
    let (meas, ad) = adsb_measurements(&a, &AdsbConfig::default());
    assert_eq!(ad.passed, 450);
    let iv = interval_stats(&meas).unwrap();
    assert_eq!(iv.p50_ns, 4_000_000_000);
    let cfg = scoring_config(&DEFAULT_CONFIG, &iv);
    let run = run_tracker(&meas, &cfg).unwrap();
    assert_eq!(run.overflow, 0);
    assert_eq!(run.stats.started, 3);
    assert_eq!(run.stats.confirmed, 3);
    // Initiation: the second report confirms, 4 s after the first.
    let (d, never) = initiation_delays(&run.log);
    assert_eq!((d.len(), never), (3, 0));
    assert!(d.iter().all(|&x| (x - 4.0).abs() < 1e-9), "{d:?}");
    assert_eq!(
        run.log.reason.iter().filter(|&&x| x == END_FLUSHED).count(),
        3
    );
    let built = adsb_truth::build(&b, &TRUTH).unwrap();
    for rows in [RowSet::Ticks, RowSet::Updates] {
        let sc = score(&run, &built, &TRUTH, rows).unwrap();
        let e = &sc.esassp;
        println!(
            "{rows:?}: pairs {}, position rms {:.2} m (straight {:.2}, turning {:.2}), speed rms straight {:.2} turning {:.2}, unclassified {}",
            sc.pairs, e.position_m[0].rms, e.position_m[1].rms, e.position_m[2].rms,
            e.speed_mps[1].rms, e.speed_mps[2].rms, e.unclassified
        );
        assert!(sc.pairs > 400, "{}", sc.pairs);
        assert!(e.position_m[0].rms < 25.0, "{}", e.position_m[0].rms);
        // Both classes are found: the turning aircraft turns for 180 s of its 600.
        assert!(e.position_m[CLASS_STRAIGHT].n > 100 && e.position_m[CLASS_TURN].n > 20);
        assert!(e.speed_mps[CLASS_STRAIGHT].rms < 3.0);
    }
    let ticks = score(&run, &built, &TRUTH, RowSet::Ticks).unwrap();
    // The metrics ran: T1 and T7 exist, with the ticks one second apart.
    let t5 = oa_bench::run::find(&ticks.bench, "T5").unwrap();
    assert_eq!(t5.rows[0].stats.median, 1.0);
    assert!(oa_bench::run::find(&ticks.bench, "T7").is_some());
    // The run is a pure function of its input.
    let again = run_tracker(&meas, &cfg).unwrap();
    assert_eq!(run.digest, again.digest);
    let _ = esassp_of;
}

fn write_clip(path: &PathBuf, r: &Reports) {
    let n = reports_len(r);
    let mut bytes = Vec::new();
    let (mut seq, mut at) = (0u64, 0);
    while at < n {
        let end = (at + 100).min(n);
        let mut payload = Vec::new();
        encode_payload(&mut payload, r, at, end).unwrap();
        let mut buf = vec![0u8; MAX_FRAME_LEN];
        let len = encode_frame(&mut buf, 1, FLAG_BYTES, seq, 0, 0, &payload).unwrap();
        bytes.extend_from_slice(&buf[..len]);
        seq += 1;
        at = end;
    }
    std::fs::write(path, &bytes).unwrap();
}

// REQ: OA-TRK-028
#[test]
fn the_binary_writes_the_scored_report_deterministically_and_refuses_bad_usage() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let clip = dir.join("track_synth.oaf");
    write_clip(&clip, &synthetic_reports());
    let exe = env!("CARGO_BIN_EXE_oa-track-run");
    let mut digests = Vec::new();
    for k in 0..2 {
        let out = dir.join(format!("track_out_{k}"));
        let o = Command::new(exe)
            .args([
                "--adsb",
                clip.to_str().unwrap(),
                "--out",
                out.to_str().unwrap(),
            ])
            .output()
            .unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        let report = std::fs::read_to_string(out.join("report.txt")).unwrap();
        for needle in [
            "Tracker (oa-track), ADS-B only, scored on split reports",
            "not an ESASSP exit result",
            "[T1] cross-track position error",
            "[T7] presence",
            "initiation delay",
            "updates/s",
            "interacting pair",
            "single straight filter",
        ] {
            assert!(report.contains(needle), "missing {needle:?}");
        }
        assert!(
            out.join("report.csv").exists()
                && out.join("tracks.csv").exists()
                && out.join("esassp.csv").exists()
        );
        let line = report
            .lines()
            .find(|l| l.contains("rows lost to table capacity"))
            .unwrap()
            .to_string();
        digests.push(
            line.split("digest ")
                .nth(1)
                .unwrap()
                .split(',')
                .next()
                .unwrap()
                .to_string(),
        );
    }
    assert_eq!(digests[0], digests[1]);
    // The tracks table lists every track and the esassp table has the basis label.
    let tracks = std::fs::read_to_string(dir.join("track_out_0/tracks.csv")).unwrap();
    assert_eq!(tracks.lines().count(), 4);
    let es = std::fs::read_to_string(dir.join("track_out_0/esassp.csv")).unwrap();
    assert!(es
        .lines()
        .skip(1)
        .all(|l| l.ends_with("not an ESASSP exit result")));
    // Usage errors exit 2, a missing clip exits 1.
    for args in [vec![], vec!["--bogus"], vec!["--adsb", "x"], vec!["--out"]] {
        let o = Command::new(exe).args(&args).output().unwrap();
        assert_eq!(o.status.code(), Some(2), "{args:?}");
    }
    let o = Command::new(exe)
        .args([
            "--adsb",
            "/nonexistent/clip.oaf",
            "--out",
            dir.join("x").to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(1));
}

// REQ: OA-TRK-029
#[test]
fn the_real_clip_is_tracked_scored_and_reproducible() {
    let Ok(path) = std::env::var("OPENATCA_ADSB_CLIP") else {
        eprintln!("skipped: OPENATCA_ADSB_CLIP not set");
        return;
    };
    let Ok(clip) = oa_bench::input::load_adsb(std::path::Path::new(&path)) else {
        eprintln!("skipped: {path} unreadable");
        return;
    };
    let (a, b, st) = split_reports(&clip.reports);
    assert_eq!(st.set_a + st.set_b, st.rows_in);
    let (meas, _) = adsb_measurements(&a, &AdsbConfig::default());
    let iv = interval_stats(&meas).unwrap();
    let cfg = scoring_config(&DEFAULT_CONFIG, &iv);
    let run = run_tracker(&meas, &cfg).unwrap();
    assert_eq!(run.overflow, 0);
    assert_eq!(run.stats.capacity_refused, 0);
    let again = run_tracker(&meas, &cfg).unwrap();
    assert_eq!(run.digest, again.digest);
    let built = adsb_truth::build(&b, &TRUTH).unwrap();
    let sc = score(&run, &built, &TRUTH, RowSet::Updates).unwrap();
    println!(
        "real clip at report times: {} pairs, position rms {:.1} m, speed rms straight {:.2} m/s; {} tracks, {} confirmed",
        sc.pairs, sc.esassp.position_m[0].rms, sc.esassp.speed_mps[CLASS_STRAIGHT].rms, run.stats.started, run.stats.confirmed
    );
    println!(
        "real clip split: {st:?}; {} flights (truth legs) of {} aircraft, {} above the R4 value",
        sc.esassp.flights,
        built.truth.ac_addr.len(),
        sc.esassp.flights_over_r4
    );
    assert!(sc.pairs > 10_000);
    assert!(sc.esassp.position_m[0].rms < 60.0);
    assert!(run.stats.confirmed as f64 > 0.8 * run.stats.started as f64);
}

// REQ: OA-TRK-171, OA-TRK-330
#[test]
fn the_real_clip_tracker_run_has_the_pinned_digest() {
    let Ok(path) = std::env::var("OPENATCA_ADSB_CLIP") else {
        eprintln!("skipped: OPENATCA_ADSB_CLIP not set");
        return;
    };
    let Ok(clip) = oa_bench::input::load_adsb(std::path::Path::new(&path)) else {
        eprintln!("skipped: {path} unreadable");
        return;
    };
    let (a, _, st) = split_reports(&clip.reports);
    let (meas, _) = adsb_measurements(&a, &AdsbConfig::default());
    let iv = interval_stats(&meas).unwrap();
    let run = run_tracker(&meas, &scoring_config(&DEFAULT_CONFIG, &iv)).unwrap();
    println!("split {st:?}");
    println!(
        "tracker clip digest {} rows {} ticks {} skipped {}",
        run.digest, run.stats.applied, run.stats.ticks, run.stats.ticks_skipped
    );
    println!("tracker clip stats {:?}", run.stats);
    assert_eq!(run.stats.numerical_refused, 0);
    assert_eq!(run.stats.rows_unrepresentable, 0);
    assert_eq!(run.digest, PINNED_TRACKER_CLIP_DIGEST);
}

// Re-pinned twice on 2026-11-15. (1) The split removes copies of one observation and keeps
// conflicting reports of one time on one side; the counts are in the test output (`split ...`).
// (2) The covariance mapping and the exact velocity differential (OA-TRK-260 to OA-TRK-263): on
// the old split, the same 108,644 reports applied and 1,125 jump-rejected, the normalised
// innovation sum 142,114.4 to 142,128.2 (1e-4 of itself); rows differ in covariance and velocity
// at the 0.3 % and 1e-3 m/s scale. The value before both was
// "a697ae2ba723b32160a5e14e45595b3fd077b7d1bfeb09cb215453c75892f962".
// (3) Platform libm replaced by oa-math in the tracker and geodesy: the same 108,643 rows, 3,599
// ticks, none skipped; 27 of 1,553,531 output rows differ, all in the last bits of a velocity
// (largest 2e-5 m/s); no position changes at the 1e-7 degree quantum. The value before was
// "c4fdb7d84f7a18180befdba24c239de7b94f4b3bfad44007fef67794b141a5cd".
// (4) Elementary-function calls cut on the hot path (the frame's latitude sine and cosine carried
// with the filter and the small-angle series in the chart, one exponential for the model and
// vertical transitions, the model probabilities from the difference of the log weights): the
// same 108,643 rows, 3,599 ticks, none skipped; of 1,553,531 output rows 141,493 differ, none in
// position at the 1e-7 degree quantum, in velocity by at most 2e-5 m/s (one single-precision
// unit), and in covariance and model probability by at most 3.3e-7 of themselves (two
// single-precision units; terms near zero, the covariance cross term, turn rate and vertical
// rate, move by more in relative terms). The value before was
// "a70fef27c29a09f93d3a28b2a2dff3a2c70b1cc707288aea69466bf42e1f37cb".
// (5) Reports of one address and time that differ in any field scoring reads are not copies
// (OA-TRK-308), and such siblings pass quality handling in key order, not input order: split
// counts unchanged (0 copies removed, 319 conflict groups, 638 rows), the same 108,643 rows, 3,599
// ticks, none skipped; only the order of the siblings within the 319 groups of set A moved. The
// value before was "2594d4f08780d75f4f38627cb49d636212de27addc34a247bf5572f26902be46".
// (6) Nonlinear turn prediction and discovery regularization (OA-TRK-329): identical split,
// 108,643 -> 108,984 applied reports; 3,599 ticks and none skipped. Scored report pairs increase
// 77,987 -> 78,318, tracks 1,299 -> 1,287 and confirmed tracks 1,226 -> 1,217. Position RMS stays
// 8.5 m at printed precision; straight speed RMS moves 1.21 -> 1.22 m/s. Numerical refusals and
// unrepresentable output rows are zero. These are different scoring cohorts. Prior digest:
// "fad4e9c12707743add69d2364c22b1573f4ef8231ccdc528526bbad4544d9842".
// (7) Emit the current report when an already-confirmed track resyncs (OA-TRK-027).
// All 1,555,337 prior output rows and 109,633 measurement decisions are unchanged; 64
// previously omitted report-time rows are added. Each matches the prior current-time query.
// Report-time pairs increase 78,318 -> 78,378; tick rows, applied reports and track counts
// are unchanged. This is output conservation, not an accuracy improvement. Prior digest:
// "48ea32ad2c4e76abcdd750527b0f1fd7a3525854c970c9316bfc6bd180bbfb42".
const PINNED_TRACKER_CLIP_DIGEST: &str =
    "8892186a0d06e90ebbbf2545c6f2af286a46949c255bedf68b1072fff2607e22";

/// The scoring of a run in one piece, as it was done before the rows were taken in ranges: one
/// legacy table of every row, `run_benchmark`, and the pairs again for the ESASSP split.
fn one_shot(
    run: &oa_track::score::TrackRun,
    built: &adsb_truth::BuiltTruth,
    rows: RowSet,
) -> (oa_bench::run::RunOutput, oa_track::score::Esassp) {
    use oa_bench::assoc::{associate, index_truth};
    use oa_bench::pair::build_pairs;
    use oa_bench::thindex::build_th_index;
    use oa_bench::tracker_input::{legacy_from_track_samples, TrackSamples};
    use oa_bench::truth::{Env, Truth};
    let r = run.rows_in(rows, 0, u32::MAX).unwrap();
    let legacy = legacy_from_track_samples(&TrackSamples {
        addr: &r.addr,
        t_ns: &r.t_ns,
        lat_e7: &r.lat_e7,
        lon_e7: &r.lon_e7,
        alt_ft: &r.alt_ft,
        vel_east_mps: &r.vel_east_mps,
        vel_north_mps: &r.vel_north_mps,
        coast: &r.coast,
    })
    .unwrap();
    let q = |a: u32, t: i64| adsb_truth::query(&built.truth, a, t);
    let truth = Truth {
        query: &q,
        legs: &built.legs,
    };
    let in_zone = |la: f64, lo: f64| {
        let b = oa_adsb::ZAU_BOX;
        la >= f64::from(b.lat_min_e7) * 1e-7
            && la <= f64::from(b.lat_max_e7) * 1e-7
            && lo >= f64::from(b.lon_min_e7) * 1e-7
            && lo <= f64::from(b.lon_max_e7) * 1e-7
    };
    let mut env: Env<'_> = oa_bench::truth::empty_env();
    env.in_zone = &in_zone;
    let mut rcfg = oa_bench::run::RunConfig::default();
    rcfg.pair.max_sigma_m = TRUTH.max_sigma95_h_m;
    let bench = oa_bench::run::run_benchmark(&legacy, &truth, &env, &rcfg);
    let ix = build_th_index(&legacy.th, legacy.flights.gufi.len());
    let tix = index_truth(truth.legs);
    let assoc = associate(&legacy, &ix, &truth, &tix, &rcfg.assoc);
    let (pairs, _) = build_pairs(&legacy, &ix, &assoc, &truth, &rcfg.pair);
    (bench, esassp_of(&pairs, &built.truth))
}

fn synthetic_run() -> (
    Vec<oa_track::Measurement>,
    oa_track::TrackerConfig,
    adsb_truth::BuiltTruth,
) {
    let (a, b, _) = split_reports(&synthetic_reports());
    let (meas, _) = adsb_measurements(&a, &AdsbConfig::default());
    let cfg = scoring_config(&DEFAULT_CONFIG, &interval_stats(&meas).unwrap());
    (meas, cfg, adsb_truth::build(&b, &TRUTH).unwrap())
}

/// Metric results compared with NaN equal to NaN (an empty statistic is NaN) and every other
/// float compared bit for bit; everything else exactly.
fn assert_metrics_same(
    a: &[oa_bench::stats::MetricResult],
    b: &[oa_bench::stats::MetricResult],
    what: &str,
) {
    let ids = |m: &[oa_bench::stats::MetricResult]| m.iter().map(|r| r.id).collect::<Vec<_>>();
    assert_eq!(ids(a), ids(b), "{what}: metric ids and order");
    for (x, y) in a.iter().zip(b) {
        let id = x.id;
        assert_eq!((x.name, x.unit), (y.name, y.unit), "{what} {id}");
        assert_eq!(x.rows, y.rows, "{what} {id} rows");
        assert_eq!(x.scalars.len(), y.scalars.len(), "{what} {id} scalar count");
        for ((n1, v1), (n2, v2)) in x.scalars.iter().zip(&y.scalars) {
            assert_eq!(n1, n2, "{what} {id}");
            assert!(
                v1.to_bits() == v2.to_bits() || (v1.is_nan() && v2.is_nan()),
                "{what} {id} {n1}: {v1} vs {v2}"
            );
        }
        assert_eq!(x.withheld, y.withheld, "{what} {id}");
        assert_eq!(x.cases, y.cases, "{what} {id} cases");
        assert_eq!(x.notes, y.notes, "{what} {id} notes");
    }
}

// REQ: OA-TRK-044
#[test]
fn scoring_in_address_ranges_gives_the_numbers_of_scoring_in_one_piece() {
    let (meas, cfg, built) = synthetic_run();
    let run = run_tracker(&meas, &cfg).unwrap();
    for rows in [RowSet::Ticks, RowSet::Updates] {
        let (bench, es) = one_shot(&run, &built, rows);
        // One row per range, a few aircraft per range, and everything in one: all the same.
        for chunk_rows in [1usize, 700, 1 << 30] {
            let counts = run.row_counts(rows).unwrap();
            let ranges = score_ranges(&counts, chunk_rows);
            // The ranges tile the address space exactly.
            assert_eq!(ranges.first().unwrap().0, 0);
            assert_eq!(ranges.last().unwrap().1, u32::MAX);
            assert!(ranges.windows(2).all(|w| w[0].1 + 1 == w[1].0));
            if chunk_rows == 1 {
                assert_eq!(ranges.len(), counts.len());
            }
            let sc = score_source(&run, &built, &TRUTH, rows, chunk_rows).unwrap();
            println!("{rows:?} chunk_rows {chunk_rows}: {} ranges", ranges.len());
            assert_metrics_same(
                &sc.bench.metrics,
                &bench.metrics,
                &format!("{rows:?} {chunk_rows}"),
            );
            assert_eq!(sc.bench.exclusions, bench.exclusions);
            assert_eq!(sc.bench.pairs, bench.pairs);
            assert_eq!(sc.bench.assoc.counts, bench.assoc.counts);
            assert_eq!(sc.esassp, es);
        }
    }
    // The size of a range of the default is what the documentation says.
    assert_eq!(SCORE_CHUNK_ROWS, 1 << 17);
}

// REQ: OA-TRK-045
#[test]
fn a_run_spilled_to_files_has_the_digest_scores_and_counters_of_the_run_in_memory() {
    let (meas, cfg, built) = synthetic_run();
    let mem = run_tracker(&meas, &cfg).unwrap();
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("track_spill_test");
    let _ = std::fs::remove_dir_all(&dir);
    let spilled = run_tracker_spilled(&meas, &cfg, &dir).unwrap();
    // The digest is the one of the same rows in memory, bit for bit.
    assert_eq!(spilled.digest, mem.digest);
    assert_eq!(spilled.rows as usize, mem.out.t_ns.len());
    assert_eq!(spilled.stats, mem.stats);
    assert_eq!(spilled.overflow, mem.overflow);
    assert_eq!(spilled.offered, mem.offered);
    for rows in [RowSet::Ticks, RowSet::Updates] {
        assert_eq!(
            spilled.row_counts(rows).unwrap(),
            mem.row_counts(rows).unwrap()
        );
        assert_eq!(
            spilled.rows_in(rows, 0, u32::MAX).unwrap(),
            mem.rows_in(rows, 0, u32::MAX).unwrap()
        );
        // A narrower range reads the same rows as a filter of the whole.
        let mid = mem
            .row_counts(rows)
            .unwrap()
            .keys()
            .nth(1)
            .copied()
            .unwrap();
        assert_eq!(
            spilled.rows_in(rows, mid, mid).unwrap(),
            mem.rows_in(rows, mid, mid).unwrap()
        );
        let (a, b) = (
            score_source(&spilled, &built, &TRUTH, rows, 700).unwrap(),
            score_source(&mem, &built, &TRUTH, rows, 700).unwrap(),
        );
        assert_metrics_same(&a.bench.metrics, &b.bench.metrics, "spilled");
        assert_eq!(a.esassp, b.esassp);
    }
    // The files are removed with the run.
    assert!(dir.join("flags").exists());
    drop(spilled);
    assert!(!dir.exists());
}

// REQ: OA-TRK-046
#[test]
fn the_latency_stages_are_stamped_from_an_injected_clock_and_never_change_the_output() {
    use oa_track::latency::{
        latency_count, latency_new, latency_stats, update_timed, FrameStamps, StepClock,
        ST_DECODED_TO_UPDATED, ST_END_TO_END, ST_RECEIVE_TO_DECODED, ST_SENSOR_TO_RECEIVE,
        ST_UPDATED_TO_EMITTED, ST_UPDATE_CALL,
    };
    use oa_track::{output_clear, output_new, tracker_new, tracker_update};
    let (meas, cfg, _) = synthetic_run();
    let mut plain = tracker_new(&cfg).unwrap();
    let mut out_plain = output_new(cfg.out_capacity);
    for m in &meas {
        tracker_update(&mut plain, m, &mut out_plain);
    }
    let mut t = tracker_new(&cfg).unwrap();
    let mut out = output_new(cfg.out_capacity);
    let mut log = latency_new(100);
    // Each reading of the clock advances it by 10 ns: update start 0, update done 10, emitted 20
    // from the call's own start; the frame was received at 1000 and decoded at 1100.
    let mut clock = StepClock {
        now: 1_200,
        step_ns: 10,
    };
    let mut all = oa_track::TrackOutput {
        capacity: usize::MAX,
        ..oa_track::TrackOutput::default()
    };
    for m in &meas {
        let frame = FrameStamps {
            recv_ns: m.toa_ns - 500_000_000,
            received: 1_000,
            decoded: 1_100,
        };
        let mut emit = |o: &mut oa_track::TrackOutput| {
            all.t_ns.extend_from_slice(&o.t_ns);
            output_clear(o);
        };
        update_timed(&mut t, m, &frame, &mut out, &mut clock, &mut emit, &mut log);
    }
    // The clock is read three times per measurement and the tracker saw none of it: the same
    // rows come out as without it.
    assert!(out.t_ns.is_empty());
    assert_eq!(all.t_ns, out_plain.t_ns);
    assert_eq!(latency_count(&log), 100);
    assert_eq!(log.overflow as usize, meas.len() - 100);
    assert_eq!(clock.now, 1_200 + 30 * meas.len() as u64);
    let st = |i| latency_stats(&log, i);
    // recv minus sensor time is -0.5 s for every measurement.
    assert_eq!(st(ST_SENSOR_TO_RECEIVE).p50, -500_000_000);
    assert_eq!(st(ST_SENSOR_TO_RECEIVE).max, -500_000_000);
    assert_eq!(st(ST_RECEIVE_TO_DECODED).p50, 100);
    assert_eq!(st(ST_UPDATE_CALL).p99, 10);
    assert_eq!(st(ST_UPDATED_TO_EMITTED).max, 10);
    // Measurement k is updated at 1210 + 30 k and emitted at 1220 + 30 k; its frame was decoded
    // at 1100 and received at 1000: the wait after decoding is 110 + 30 k, end to end 220 + 30 k.
    let w = st(ST_DECODED_TO_UPDATED);
    assert_eq!((w.min, w.max), (110, 110 + 99 * 30));
    assert_eq!(st(ST_END_TO_END).max, 220 + 99 * 30);
    assert_eq!(st(ST_END_TO_END).n, 100);
    // A clock that goes backwards is counted, not hidden.
    let mut log2 = latency_new(4);
    let mut back = StepClock { now: 5, step_ns: 0 };
    let frame = FrameStamps {
        recv_ns: 0,
        received: 100,
        decoded: 50,
    };
    let mut t2 = tracker_new(&cfg).unwrap();
    let mut o2 = output_new(cfg.out_capacity);
    update_timed(
        &mut t2,
        &meas[0],
        &frame,
        &mut o2,
        &mut back,
        &mut |_| {},
        &mut log2,
    );
    assert_eq!(
        log2.negative, 3,
        "decoded < received, updated < decoded, emitted < received"
    );
}

// REQ: OA-TRK-160
#[test]
fn a_report_received_before_its_sensor_time_is_counted_as_clock_skew() {
    use oa_track::latency::{latency_new, latency_record, latency_stats, Stamps};
    let mut log = latency_new(8);
    let at = |toa_ns: i64, recv_ns: i64| Stamps {
        toa_ns,
        recv_ns,
        received: 10,
        decoded: 20,
        update_start: 30,
        updated: 40,
        emitted: 50,
    };
    latency_record(&mut log, &at(1_000, 1_500));
    assert_eq!(log.sensor_negative, 0);
    latency_record(&mut log, &at(2_000, 1_500));
    // The signed value is kept (the statistics show the skew); the count says how many.
    assert_eq!(log.sensor_negative, 1);
    assert_eq!(log.negative, 0);
    assert_eq!(latency_stats(&log, 0).min, -500);
}

// REQ: OA-TRK-288
#[test]
fn a_sensor_time_too_far_from_the_receive_time_is_refused_and_counted_not_clamped() {
    use oa_track::latency::{latency_count, latency_new, latency_record, Stamps};
    let mut log = latency_new(8);
    let at = |toa_ns: i64, recv_ns: i64| Stamps {
        toa_ns,
        recv_ns,
        received: 10,
        decoded: 20,
        update_start: 30,
        updated: 40,
        emitted: 50,
    };
    latency_record(&mut log, &at(i64::MIN, 5));
    latency_record(&mut log, &at(i64::MAX, -5));
    assert_eq!(
        latency_count(&log),
        0,
        "a skew that does not fit was stored"
    );
    assert_eq!(log.skew_refused, 2);
    latency_record(&mut log, &at(1_000, 1_500));
    assert_eq!(latency_count(&log), 1);
    assert_eq!(log.skew_refused, 2);
}

// REQ: OA-TRK-047
#[test]
fn no_score_runs_the_core_and_the_latency_replay_and_reports_memory_and_stages() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let clip = dir.join("track_noscore.oaf");
    // A recording is in (time, address) order, which the replay relies on; the synthetic reports
    // are built one aircraft after the other.
    let src = synthetic_reports();
    let mut order: Vec<(i64, u32, usize)> = (0..reports_len(&src))
        .map(|i| (src.time_ns[i], src.icao[i], i))
        .collect();
    order.sort_unstable();
    let mut sorted = Reports::default();
    for (_, _, i) in order {
        reports_push(&mut sorted, &reports_row(&src, i).unwrap());
    }
    write_clip(&clip, &sorted);
    let out = dir.join("track_noscore_out");
    let _ = std::fs::remove_dir_all(&out);
    let o = Command::new(env!("CARGO_BIN_EXE_oa-track-run"))
        .args([
            "--adsb",
            clip.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
            "--no-score",
        ])
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let text = std::fs::read_to_string(out.join("core.txt")).unwrap();
    assert_eq!(text, String::from_utf8_lossy(&o.stdout));
    for needle in [
        "tracker core,",
        "one-second output off",
        "its output digest equals the scoring run's",
        "receive to decode done",
        "decode done to update done",
        "receive to output emitted (end to end)",
        "tracker update call",
        "a replay is not live",
        "0 unmatched",
        "Resident memory",
    ] {
        assert!(text.contains(needle), "missing {needle:?}\n{text}");
    }
    // No scoring happened: no report, and the spill is gone.
    assert!(!out.join("report.txt").exists());
    assert_eq!(
        std::fs::read_dir(&out)
            .unwrap()
            .filter(|e| e
                .as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("spill"))
            .count(),
        0
    );
}

// REQ: OA-TRK-066
#[test]
fn an_absent_altitude_is_never_scored_as_zero_feet() {
    use oa_bench::legacy::NO_ALT;
    assert_eq!(scored_altitude_ft(O_TICK | O_HAS_ALT, 35_000.4), 35_000);
    assert_eq!(scored_altitude_ft(O_HAS_ALT, -1_200.6), -1_201);
    // a real sea-level altitude is 0, an absent one is not
    assert_eq!(scored_altitude_ft(O_HAS_ALT, 0.0), 0);
    assert_eq!(scored_altitude_ft(O_TICK, f32::NAN), NO_ALT);
    assert_eq!(scored_altitude_ft(O_TICK, 0.0), NO_ALT);
    assert_eq!(scored_altitude_ft(O_HAS_ALT, f32::NAN), NO_ALT);
    assert_eq!(scored_altitude_ft(O_HAS_ALT, f32::INFINITY), NO_ALT);
    assert_eq!(scored_altitude_ft(0, 12_000.0), NO_ALT);
}

fn obs_keys(s: &Reports) -> Vec<(u32, i64, i32, i32)> {
    (0..reports_len(s))
        .map(|i| (s.icao[i], s.time_ns[i], s.lat_e7[i], s.lon_e7[i]))
        .collect()
}

fn doubled(times: &[f64]) -> Reports {
    let mut r = Reports::default();
    for &t in times {
        let mut row = base_row(0x100001, t);
        row.lat_e7 = 410_000_000 + (t as i32) * 10;
        reports_push(&mut r, &row);
        reports_push(&mut r, &row);
    }
    r
}

// REQ: OA-TRK-270
#[test]
fn copies_of_one_observation_never_land_in_both_sets() {
    let (a, b, st) = split_reports(&doubled(&[0.0, 2.0, 4.0]));
    let (ka, kb) = (obs_keys(&a), obs_keys(&b));
    assert!(ka.iter().all(|k| !kb.contains(k)), "{ka:?} {kb:?}");
    assert_eq!((ka.len(), kb.len()), (2, 1));
    assert_eq!((st.rows_in, st.duplicates_removed), (6, 3));
    assert_eq!(st.set_a + st.set_b, 3);
}

// REQ: OA-TRK-271
#[test]
fn duplicating_a_whole_recording_gives_the_same_split() {
    let times = [0.0, 1.0, 2.0, 3.0, 4.0];
    let mut single = Reports::default();
    for &t in &times {
        let mut row = base_row(0x100001, t);
        row.lat_e7 = 410_000_000 + (t as i32) * 10;
        reports_push(&mut single, &row);
    }
    let mut twice = single.clone();
    for i in 0..reports_len(&single) {
        reports_push(&mut twice, &reports_row(&single, i).unwrap());
    }
    let (a1, b1, s1) = split_reports(&single);
    let (a2, b2, s2) = split_reports(&twice);
    assert_eq!((&a1, &b1), (&a2, &b2));
    assert_eq!((s1.set_a, s1.set_b), (s2.set_a, s2.set_b));
    assert_eq!((s1.duplicates_removed, s2.duplicates_removed), (0, 5));
}

// REQ: OA-TRK-272
#[test]
fn conflicting_reports_stay_on_one_side_and_are_counted() {
    let mut r = Reports::default();
    for (t, lat) in [(0.0, 1), (1.0, 2), (1.0, 3), (1.0, 3), (2.0, 4)] {
        let mut row = base_row(0x100001, t);
        row.lat_e7 = lat;
        reports_push(&mut r, &row);
    }
    let (a, b, st) = split_reports(&r);
    assert_eq!((reports_len(&a), reports_len(&b)), (2, 2));
    assert!(b.lat_e7 == vec![2, 3] || b.lat_e7 == vec![3, 2]);
    assert_eq!(
        (st.duplicates_removed, st.conflict_groups, st.conflict_rows),
        (1, 1, 2)
    );
}

// REQ: OA-TRK-274
#[test]
fn the_split_with_halving_off_keeps_every_distinct_report_in_the_first_set() {
    let mut r = Reports::default();
    for (t, lat) in [(0.0, 1), (1.0, 2), (1.0, 3), (1.0, 3), (2.0, 4), (3.0, 5)] {
        let mut row = base_row(0x100001, t);
        row.lat_e7 = lat;
        reports_push(&mut r, &row);
    }
    let (a, b, st) = split_reports_with(&r, false);
    let (ha, hb, hs) = split_reports(&r);
    assert_eq!((reports_len(&a), reports_len(&b)), (5, 0));
    assert_eq!((st.set_a, st.set_b), (5, 0));
    // Same removal and conflict counts as the halving run, and the same rows in all.
    assert_eq!(
        (st.duplicates_removed, st.conflict_groups, st.conflict_rows),
        (hs.duplicates_removed, hs.conflict_groups, hs.conflict_rows)
    );
    assert_eq!((st.duplicates_removed, st.conflict_groups), (1, 1));
    let mut all: Vec<i32> = ha.lat_e7.iter().chain(hb.lat_e7.iter()).copied().collect();
    all.sort_unstable();
    let mut one = a.lat_e7.clone();
    one.sort_unstable();
    assert_eq!(all, one);
}

// REQ: OA-TRK-273
#[test]
fn two_trips_of_one_address_are_two_flights_and_the_bad_one_fails() {
    // One address flies two legs (a gap over the leg gap between them), 1 s reports.
    let addr = oa_adsb::NON_ICAO | 0x100001u32;
    let mut r = Reports::default();
    let mut starts = [0.0, 0.0];
    for (leg, t0) in [0.0, 400.0].into_iter().enumerate() {
        starts[leg] = t0;
        for k in 0..60 {
            let mut row = base_row(addr, t0 + f64::from(k));
            row.lat_e7 = 410_000_000 + (leg as i32) * 1_000_000 + k * 50;
            row.lon_e7 = -880_000_000;
            row.gs_dkt = 4_000;
            row.track_ddeg = 0;
            reports_push(&mut r, &row);
        }
    }
    let tcfg = TRUTH;
    let built = adsb_truth::build(&r, &tcfg).unwrap();
    assert_eq!(oa_truth::legs_of(&built.truth, addr).count, 2);
    let mut p = oa_bench::pair::PairTable::default();
    let mut push = |t_s: f64, e: f32| {
        p.th.push(0);
        p.flight.push(0);
        p.addr.push(addr);
        p.t_ms.push((T0 / 1_000_000) + (t_s * 1000.0) as i64);
        p.cross_m.push(e);
        p.along_m.push(0.0);
        p.speed_err_kt.push(0.0);
        p.hdg_err_deg.push(0.0);
        p.alt_err_ft.push(0.0);
        p.truth_speed_mps.push(100.0);
        p.method.push(0);
        p.phase.push(0);
        p.band.push(0);
        p.coast.push(0);
    };
    // Trip one: 50 samples of 0 m. Trip two: 12 samples of 1,000 m (pooled RMS is about 480 m
    // over the two trips: about 440 m, still over 230 m, but a bigger pool hides it).
    for k in 5..55 {
        push(starts[0] + f64::from(k), 0.0);
    }
    for k in 5..17 {
        push(starts[1] + f64::from(k), 1000.0);
    }
    let e = esassp_of(&p, &built.truth);
    assert_eq!(e.flights, 2);
    assert_eq!(e.flights_over_r4, 1);
    assert!((e.flight_rms_max_m - 1000.0).abs() < 1e-9);

    let summaries = flight_error_summaries(&p, &built.truth, &built.legs).unwrap();
    assert_eq!(summaries.len(), 2);
    assert!(summaries.iter().all(|f| f.address == addr));
    assert_eq!(summaries[0].position_samples, 50);
    assert_eq!(summaries[1].position_samples, 12);
    assert_eq!(summaries[0].position_rms_m, Some(0.0));
    assert_eq!(summaries[1].position_rms_m, Some(1000.0));

    let no_pairs = oa_bench::pair::PairTable::default();
    let empty = flight_error_summaries(&no_pairs, &built.truth, &built.legs).unwrap();
    assert_eq!(empty.len(), 2);
    assert!(empty
        .iter()
        .all(|f| f.paired_samples == 0 && f.position_rms_m.is_none()));

    let mut coast = p.clone();
    coast.coast[0] = 1;
    let coast_summaries = flight_error_summaries(&coast, &built.truth, &built.legs).unwrap();
    assert_eq!(coast_summaries[0].paired_samples, 50);
    assert_eq!(coast_summaries[0].noncoast_samples, 49);
    assert_eq!(coast_summaries[0].position_samples, 49);

    let mut malformed = p.clone();
    malformed.cross_m.pop();
    assert!(
        flight_error_summaries(&malformed, &built.truth, &built.legs)
            .unwrap_err()
            .0
            .contains("column cross_m")
    );

    let mut unknown = p.clone();
    unknown.addr[0] = oa_adsb::NON_ICAO | 0x100002;
    assert!(flight_error_summaries(&unknown, &built.truth, &built.legs)
        .unwrap_err()
        .0
        .contains("unknown truth leg"));
}

// REQ: OA-TRK-296
#[test]
fn the_split_check_passes_the_split_of_the_scorer_and_refuses_a_shared_report_time() {
    use oa_bench::split_check::{check_split, SplitViolation};
    let mut r = Reports::default();
    for k in 0..9 {
        let mut row = base_row(0x100001, f64::from(k));
        row.lat_e7 = 410_000_000 + k * 10;
        reports_push(&mut r, &row);
    }
    let (a, b, _) = split_reports(&r);
    assert_eq!(check_split(&a, &b), Ok(()));
    // The same report time in both sets is refused, naming the address and time.
    let mut shared = Reports::default();
    reports_push(&mut shared, &base_row(0x100001, 0.0));
    assert_eq!(
        check_split(&a, &shared),
        Err(SplitViolation::SharedTime {
            addr: 0x100001,
            time_ns: T0
        })
    );
    // On the real clip, when one is given.
    let Ok(path) = std::env::var("OPENATCA_ADSB_CLIP") else {
        eprintln!("skipped: OPENATCA_ADSB_CLIP not set");
        return;
    };
    let Ok(clip) = oa_bench::input::load_adsb(std::path::Path::new(&path)) else {
        eprintln!("skipped: {path} unreadable");
        return;
    };
    let (a, b, _) = split_reports(&clip.reports);
    assert_eq!(check_split(&a, &b), Ok(()));
}

/// Runs `oa-track-run` on the synthetic clip with `args` after `--adsb <clip> --out <out>`.
fn run_realtime_binary(name: &str, args: &[&str]) -> (std::process::Output, PathBuf) {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let clip = dir.join("track_realtime.oaf");
    write_clip(&clip, &synthetic_reports());
    let out = dir.join(name);
    let _ = std::fs::remove_dir_all(&out);
    let o = Command::new(env!("CARGO_BIN_EXE_oa-track-run"))
        .args([
            "--adsb",
            clip.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
        ])
        .args(args)
        .output()
        .unwrap();
    (o, out)
}

fn text_after<'a>(text: &'a str, key: &str) -> &'a str {
    text.lines()
        .find(|l| l.contains(key))
        .unwrap_or_else(|| panic!("no {key:?} in\n{text}"))
}

// REQ: OA-TRK-388
#[test]
fn realtime_writes_per_second_counts_and_a_digest_that_repeats() {
    let mut digests = Vec::new();
    let mut summary = Vec::new();
    for k in 0..2 {
        let (o, out) =
            run_realtime_binary(&format!("track_rt_{k}"), &["--realtime", "--with-output"]);
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        let text = std::fs::read_to_string(out.join("realtime.txt")).unwrap();
        assert_eq!(text, String::from_utf8_lossy(&o.stdout));
        let csv = std::fs::read_to_string(out.join("seconds.csv")).unwrap();
        let rows: Vec<Vec<&str>> = csv.lines().map(|l| l.split(',').collect()).collect();
        assert_eq!(
            rows[0],
            [
                "unix_second",
                "reports",
                "work_us",
                "confirmed_tracks",
                "tentative_tracks"
            ]
        );
        // Three aircraft report every 2 s for 600 s: a row for every second, 1 to 3 reports in
        // each odd or even second, and the last second holds the three confirmed tracks.
        assert!(rows.len() > 500, "{}", rows.len());
        assert!(rows[1..].iter().all(|r| r.len() == 5));
        let last = rows.last().unwrap();
        assert_eq!(last[3], "3", "{last:?}");
        let reports: u64 = rows[1..].iter().map(|r| r[1].parse::<u64>().unwrap()).sum();
        assert!(text_after(&text, "counters:").contains(&format!("offered {reports},")));
        assert!(
            text_after(&text, "peak tracks, sampled once per second").contains(": confirmed 3,")
        );
        // One row per second from the first to the last, and the traffic line says the same.
        let first: i64 = rows[1][0].parse().unwrap();
        let last_s: i64 = last[0].parse().unwrap();
        assert_eq!(rows.len() as i64 - 1, last_s - first + 1);
        assert!(text_after(&text, "traffic:").starts_with(&format!(
            "traffic: {} s from unix second {first} ",
            last_s - first + 1
        )));
        // Every second with reports has work in it, and the work adds up to the printed total.
        let mut sum_us = 0.0;
        for r in &rows[1..] {
            let (n, us): (u64, f64) = (r[1].parse().unwrap(), r[2].parse().unwrap());
            assert!(n == 0 || us > 0.0, "{r:?}");
            sum_us += us;
        }
        let printed: f64 = text_after(&text, "processing time of a second")
            .split("total ")
            .last()
            .unwrap()
            .trim_end_matches(" s")
            .parse()
            .unwrap();
        assert!((sum_us / 1e6 - printed).abs() < 0.002, "{sum_us} {printed}");
        // The CPU share is over the traffic span.
        let span = last_s - first + 1;
        assert!(text_after(&text, "update window")
            .contains(&format!("(over the {span} s span of the traffic)")));
        for needle in [
            "config (scoring_config from the full clip): max_tracks 4096",
            "final advance and flush",
            "s span of the traffic), update only",
            "share of one core, total including ingest",
            "cpu source:",
            "single-threaded today",
            "the same --max-tracks and the same build",
            "process peak",
            "An upper bound",
            "counting as zero",
            "processing time over the 300 busy seconds only (seconds with at least one measurement): p50 ",
            "so at most 1000.000 ms; a batch completes at most its own work later, so a report's latency is at most 1 s + backlog + that second's work, at most ",
            "conditions: the input was decoded and held in memory before timing, sorted by time, and processed on one thread, so the figures exclude network input, decoding and the interrupts of a live feed",
            "the tick rows for a gap are emitted with the next update and charged to that second",
            "sum of the per-second timers (wall only; no CPU is read per second): ",
            " s wall outside the per-second timers: track counts, bookkeeping",
            "final advance and flush after the last second: ",
            "(includes the output spill)",
        ] {
            assert!(text.contains(needle), "missing {needle:?}\n{text}");
        }
        assert!(text.contains("refused no_address: 0"));
        digests.push(text_after(&text, "output rows digest:").to_string());
        summary.push(text_after(&text, "run summary digest").to_string());
        assert!(!out.join("report.txt").exists());
    }
    assert_eq!(digests[0], digests[1]);
    assert_eq!(summary[0], summary[1]);
    assert!(!digests[0].contains("not computed"));
    let (o, out) = run_realtime_binary("track_rt_plain", &["--realtime"]);
    assert!(o.status.success());
    let text = std::fs::read_to_string(out.join("realtime.txt")).unwrap();
    assert!(text_after(&text, "output rows digest:").contains("not computed"));
    // Without rows the run summary digest is the same.
    assert_eq!(text_after(&text, "run summary digest"), summary[0]);
}

// REQ: OA-TRK-388
#[test]
fn realtime_fails_on_a_full_table_an_empty_clip_and_flags_that_do_not_apply() {
    // One track slot for three aircraft: refusals, a report that says so, and a failing exit.
    let (o, out) = run_realtime_binary("track_rt_full", &["--realtime", "--max-tracks", "1"]);
    assert_eq!(o.status.code(), Some(1));
    let text = std::fs::read_to_string(out.join("realtime.txt")).unwrap();
    assert!(text.contains("NOT A FULL-LOAD MEASUREMENT"), "{text}");
    assert!(!text_after(&text, "counters:").contains("capacity-refused 0"));
    // An empty clip.
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let empty = dir.join("track_rt_empty.oaf");
    std::fs::write(&empty, b"").unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_oa-track-run"))
        .args([
            "--adsb",
            empty.to_str().unwrap(),
            "--out",
            dir.join("track_rt_empty_out").to_str().unwrap(),
            "--realtime",
        ])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&o.stderr).contains("no reports"));
    // Flags that do not apply are usage errors.
    for args in [
        vec!["--with-output"],
        vec!["--max-tracks", "9"],
        vec!["--realtime", "--digest", "d.txt"],
        vec!["--realtime", "--rows", "r.bin"],
        vec!["--realtime", "--no-score"],
        vec!["--realtime", "--no-compare"],
        vec!["--realtime", "--max-tracks", "many"],
    ] {
        let (o, _) = run_realtime_binary("track_rt_usage", &args);
        assert_eq!(o.status.code(), Some(2), "{args:?}");
    }
}
