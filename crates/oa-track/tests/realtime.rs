#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
//! The per-second log of `oa-track-run --realtime` (OA-TRK-388).

use oa_track::realtime::{
    check_flags, cpu_use, realtime_failure, realtime_summary, run_summary_digest, seconds_csv,
    CpuMarks, SecondLog,
};
use oa_track::types::{log_new, TrackerStats};

const MS: u64 = 1_000_000;

fn log_of(secs: &[(i64, u32, u64)]) -> SecondLog {
    let mut l = SecondLog::default();
    for &(s, r, ns) in secs {
        l.push_second(s, r, ns, 0, 0);
    }
    l
}

// REQ: OA-TRK-388
#[test]
fn confirmed_and_tentative_counts_cover_every_second_including_the_last_and_an_empty_one() {
    let mut l = SecondLog::default();
    l.push_second(100, 3, 300 * MS, 7, 3);
    l.push_second(102, 1, 100 * MS, 9, 1);
    l.push_second(103, 2, 200 * MS, 8, 4);
    assert_eq!((l.confirmed.len(), l.tentative.len()), (4, 4));
    // The second with no measurements (101) carries the counts it was left with; the last second
    // (103) has its own.
    assert_eq!(l.confirmed, vec![7, 7, 9, 8]);
    assert_eq!(l.tentative, vec![3, 3, 1, 4]);
    let csv = seconds_csv(&l);
    let rows: Vec<&str> = csv.lines().collect();
    assert_eq!(
        rows[0],
        "unix_second,reports,work_us,confirmed_tracks,tentative_tracks"
    );
    assert_eq!(rows[2], "101,0,0.0,7,3");
    assert_eq!(rows[4], "103,2,200000.0,8,4");
    let s = realtime_summary(&l);
    assert_eq!((s.confirmed_peak, s.tentative_peak), (9, 4));
    assert_eq!((s.seconds, s.busy_seconds, s.reports), (4, 3, 6));
    assert_eq!((s.reports_peak, s.reports_peak_s), (3, 100));
    assert_eq!((s.work_max_ns, s.work_max_s), (300 * MS, 100));
    assert!((s.reports_mean - 1.5).abs() < 1e-12);
}

// REQ: OA-TRK-388
#[test]
fn percentiles_of_zero_one_and_many_seconds() {
    let none = realtime_summary(&SecondLog::default());
    assert_eq!(
        (none.seconds, none.work_p50_ns, none.work_p99_ns),
        (0, 0, 0)
    );
    let one = realtime_summary(&log_of(&[(5, 1, 7 * MS)]));
    assert_eq!(
        (one.work_p50_ns, one.work_p99_ns, one.work_max_ns),
        (7 * MS, 7 * MS, 7 * MS)
    );
    // 1 ms, 2 ms, ... 100 ms: nearest rank p50 is the 50th, p99 the 99th.
    let many: Vec<(i64, u32, u64)> = (1..=100u64).map(|i| (i as i64, 1, i * MS)).collect();
    let s = realtime_summary(&log_of(&many));
    assert_eq!(
        (s.work_p50_ns, s.work_p99_ns, s.work_max_ns),
        (50 * MS, 99 * MS, 100 * MS)
    );
    // An empty second counts as zero: of 0, 0, 10, 10 ms the median (2nd of 4) is 0 and p99 is 10.
    let gap = realtime_summary(&log_of(&[(0, 1, 10 * MS), (3, 1, 10 * MS)]));
    assert_eq!(
        (gap.seconds, gap.work_p50_ns, gap.work_p99_ns),
        (4, 0, 10 * MS)
    );
}

// REQ: OA-TRK-388
#[test]
fn a_slow_second_queues_work_behind_it() {
    // 1.5 s of work in the first second, then 0.2 s each: 0.5 s behind after the first, caught up
    // by the second after it (0.5 + 0.2 - 1 < 0).
    let l = log_of(&[
        (0, 1, 1_500 * MS),
        (1, 1, 200 * MS),
        (2, 1, 200 * MS),
        (3, 1, 200 * MS),
    ]);
    let s = realtime_summary(&l);
    assert_eq!((s.over_one_second, s.finished_late), (1, 1));
    assert_eq!(s.backlog_max_ns, 500 * MS);
    // 1.5 s, 1.5 s, 0.1 s: the queue is 0.5 s, then 1.0 s, then 0.1 s; all three finish late.
    let s = realtime_summary(&log_of(&[
        (0, 1, 1_500 * MS),
        (1, 1, 1_500 * MS),
        (2, 1, 100 * MS),
    ]));
    assert_eq!((s.over_one_second, s.finished_late), (2, 3));
    assert_eq!(s.backlog_max_ns, 1_000 * MS);
    let huge = realtime_summary(&log_of(&[(0, 1, u64::MAX), (1, 1, u64::MAX)]));
    assert!(huge.backlog_max_ns > 0);
}

// REQ: OA-TRK-388
#[test]
fn a_dropped_measurement_an_empty_run_a_full_table_and_an_overflow_each_fail_the_run() {
    let ok = log_of(&[(1, 1, 1)]);
    let mut l = log_of(&[(1000, 2, 1)]);
    l.push_second(999, 4, 5, 0, 0);
    l.push_second(1000 + 10_000_000, 1, 5, 0, 0);
    assert_eq!(l.dropped, 5);
    assert_eq!(l.work_ns.len(), 1);
    let why = realtime_failure(&l, 7, 0, 0, 0).unwrap();
    assert!(
        why.starts_with("5 measurements fall outside the log"),
        "{why}"
    );
    assert!(realtime_failure(&ok, 0, 0, 0, 0)
        .unwrap()
        .contains("no measurements"));
    let full = realtime_failure(&ok, 3, 12, 0, 0).unwrap();
    assert!(
        full.starts_with("12 measurements refused for lack of a track slot"),
        "{full}"
    );
    assert!(full.contains("not a full-load measurement"));
    // Rows or track log entries lost to a full table: the digests cover truncated data.
    let rows = realtime_failure(&ok, 3, 0, 9, 0).unwrap();
    assert!(
        rows.starts_with("9 output rows and 0 track log entries were lost"),
        "{rows}"
    );
    let tracks = realtime_failure(&ok, 3, 0, 0, 4).unwrap();
    assert!(
        tracks.starts_with("0 output rows and 4 track log entries were lost"),
        "{tracks}"
    );
    assert!(tracks.contains("digests then cover truncated data"));
    assert_eq!(realtime_failure(&ok, 3, 0, 0, 0), None);
}

// REQ: OA-TRK-388
#[test]
fn cpu_use_splits_the_readings_and_divides_by_the_span_of_the_traffic() {
    const S: u64 = 1_000_000_000;
    // start 0, loaded 4 s, update window 5 s to 105 s (100 s), finished 110 s.
    let m = CpuMarks {
        start: 0,
        loaded: 4 * S,
        loop_start: 5 * S,
        looped: 105 * S,
        finished: 110 * S,
    };
    let u = cpu_use(&m, 3600.0).unwrap();
    assert_eq!((u.ingest_s, u.update_s, u.total_s), (4.0, 100.0, 110.0));
    // The denominator is the span of the traffic: 100 s over 3,600 s is 2.777...%.
    assert!((u.update_pct - 100.0 * 100.0 / 3600.0).abs() < 1e-12);
    assert!((u.total_pct - 100.0 * 110.0 / 3600.0).abs() < 1e-12);
    // A shorter span gives a larger share; under one second the span is one second.
    assert!((cpu_use(&m, 200.0).unwrap().update_pct - 50.0).abs() < 1e-12);
    assert!((cpu_use(&m, 0.0).unwrap().total_pct - 11_000.0).abs() < 1e-9);
    // A reading taken out of order (a digest's time before the final flush's) is refused.
    let late_flush = CpuMarks {
        finished: 100 * S,
        ..m
    };
    assert_eq!(cpu_use(&late_flush, 3600.0), None);
}

// REQ: OA-TRK-388
#[test]
fn the_total_cpu_is_read_before_the_digests_in_the_shell() {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/main.rs")).unwrap();
    let body = &src[src.find("fn run_realtime_inner").unwrap()
        ..src
            .find(
                "
fn run(",
            )
            .unwrap()];
    let at = |needle: &str| body.find(needle).unwrap();
    let flush = at("tracker_flush(&mut t, end, &mut out);");
    let finished = at("let cpu_finished = cpu_ns();");
    assert!(flush < finished);
    assert!(finished < at("run_summary_digest("));
    assert!(finished < at("spill_finish("));
    assert!(finished < at("cpu_use(&m, traffic_s)"));
    // No other reading of the CPU counter comes after it.
    assert_eq!(body[finished..].matches("cpu_ns()").count(), 1);
}

// REQ: OA-TRK-388
#[test]
fn flags_that_do_not_go_together_are_refused() {
    assert_eq!(check_flags(true, true, true, &[]), None);
    assert_eq!(
        check_flags(false, false, false, &["--digest", "--no-score"]),
        None
    );
    assert_eq!(
        check_flags(false, true, false, &[]).unwrap(),
        "--with-output needs --realtime"
    );
    assert_eq!(
        check_flags(false, false, true, &[]).unwrap(),
        "--max-tracks needs --realtime"
    );
    for f in ["--digest", "--rows", "--no-score", "--no-compare"] {
        let why = check_flags(true, false, false, &[f]).unwrap();
        assert_eq!(why, format!("{f} does not apply with --realtime"));
    }
}

// REQ: OA-TRK-388
#[test]
fn the_run_summary_digest_is_stable_and_sees_the_log_and_the_counters() {
    let mut log = log_new(4);
    log.addr.push(0xA1);
    log.first_ns.push(1);
    log.confirmed_ns.push(2);
    log.last_ns.push(3);
    log.updates.push(4);
    log.reason.push(0);
    let stats = TrackerStats::default();
    let d = run_summary_digest(&log, &stats);
    assert_eq!(d.len(), 64);
    assert_eq!(d, run_summary_digest(&log, &stats));
    let mut more = stats;
    more.capacity_refused = 1;
    assert_ne!(d, run_summary_digest(&log, &more));
    log.updates[0] = 5;
    assert_ne!(d, run_summary_digest(&log, &stats));
}
