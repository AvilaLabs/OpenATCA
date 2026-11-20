#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
//! The per-second log of `oa-track-run --realtime` (OA-TRK-388).

use oa_track::realtime::{realtime_summary, seconds_csv, SecondLog};

fn log_of(secs: &[(i64, u64)]) -> SecondLog {
    let mut l = SecondLog::default();
    for &(s, ns) in secs {
        l.add(s, ns);
    }
    l
}

// REQ: OA-TRK-388
#[test]
fn seconds_are_bucketed_and_summarised() {
    let mut l = log_of(&[
        (100, 200_000_000),
        (100, 100_000_000),
        (101, 400_000_000),
        (103, 900_000_000),
    ]);
    l.set_tracks(100, 7);
    l.set_tracks(103, 9);
    let s = realtime_summary(&l);
    assert_eq!((s.seconds, s.busy_seconds, s.reports), (4, 3, 4));
    assert_eq!((s.reports_peak, s.reports_peak_s), (2, 100));
    assert_eq!(s.work_max_ns, 900_000_000);
    assert_eq!(s.work_max_s, 103);
    assert_eq!(s.work_total_ns, 1_600_000_000);
    assert_eq!(s.work_p50_ns, 300_000_000);
    assert_eq!(s.over_one_second, 0);
    assert_eq!((s.finished_late, s.backlog_max_ns), (0, 0));
    assert_eq!(s.tracks_peak, 9);
    assert!((s.reports_mean - 1.0).abs() < 1e-12);
}

// REQ: OA-TRK-388
#[test]
fn a_slow_second_queues_work_behind_it() {
    // 1.5 s of work in the first second, then 0.2 s, 0.2 s, 0.2 s: the queue is 0.5 s behind after
    // the first, 0.0 s after the third (0.5 + 0.2 - 1 < 0).
    let l = log_of(&[
        (0, 1_500_000_000),
        (1, 200_000_000),
        (2, 200_000_000),
        (3, 200_000_000),
    ]);
    let s = realtime_summary(&l);
    assert_eq!(s.over_one_second, 1);
    assert_eq!(s.finished_late, 1);
    assert_eq!(s.backlog_max_ns, 500_000_000);
}

// REQ: OA-TRK-388
#[test]
fn out_of_range_seconds_are_counted_not_stored() {
    let mut l = log_of(&[(1000, 1)]);
    l.add(999, 5);
    l.add(1000 + 10_000_000, 5);
    assert_eq!(l.dropped, 2);
    assert_eq!(l.work_ns.len(), 1);
    let csv = seconds_csv(&l);
    assert_eq!(csv.lines().count(), 2);
    assert!(csv.lines().nth(1).unwrap().starts_with("1000,1,0.0,"));
    assert_eq!(realtime_summary(&SecondLog::default()).seconds, 0);
}
