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
use oa_geo::aeqd::{frame_velocity, unproject};
use oa_track::adsb::{adsb_measurements, AdsbConfig};
use oa_track::score::{
    esassp_of, initiation_delays, interval_stats, run_tracker, run_tracker_spilled, score,
    score_ranges, score_source, scoring_config, split_reports, transversal_accel, RowSet,
    RowSource, CLASS_STRAIGHT, CLASS_TURN, SCORE_CHUNK_ROWS, SCORE_DROP_AFTER_NS, TURN_ACCEL_MPS2,
};
use oa_track::{DEFAULT_CONFIG, END_FLUSHED};
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
    // Aircraft 2 has eight reports: 0,1,2,3,3',4,5,6 -> A gets positions 0,2,4,6 = 0,2,3',5.
    assert_eq!(times_of(&a, 0x100002), vec![0, 2, 3, 5]);
    assert_eq!(times_of(&b, 0x100002), vec![1, 3, 4, 6]);
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
    // Only the two same-time reports can swap sides (input order breaks that tie).
    let diff = ka.iter().filter(|x| !ka2.contains(x)).count();
    assert!(diff <= 1, "{diff}");
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
            );
            let (te, tn) = frame_velocity(&f, tr.e, tr.n, tr.ve, tr.vn);
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
    assert!(sc.pairs > 10_000);
    assert!(sc.esassp.position_m[0].rms < 60.0);
    assert!(run.stats.confirmed as f64 > 0.8 * run.stats.started as f64);
}

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
            assert_eq!(sc.bench.metrics, bench.metrics, "{rows:?} {chunk_rows}");
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
        assert_eq!(a.bench.metrics, b.bench.metrics);
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
