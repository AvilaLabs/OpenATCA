//! The check on the tracker-input split.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_adsb::{reports_push, Reports, ROW_EMPTY};
use oa_bench::split_check::{check_split, SplitViolation};

fn row(addr: u32, t_s: i64, lat: i32) -> oa_adsb::Row {
    let mut r = ROW_EMPTY;
    r.icao = addr;
    r.time_ns = 1_700_000_000_000_000_000 + t_s * 1_000_000_000;
    r.lat_e7 = lat;
    r.lon_e7 = -880_000_000;
    r
}

fn set(rows: &[oa_adsb::Row]) -> Reports {
    let mut r = Reports::default();
    for x in rows {
        reports_push(&mut r, x);
    }
    r
}

// REQ: OA-BENCH-119
#[test]
fn a_split_with_no_shared_time_and_no_copies_passes() {
    let a = set(&[row(1, 0, 10), row(1, 2, 12), row(2, 0, 10)]);
    // A conflict (two contents at one time) stays whole on one side and is allowed.
    let b = set(&[row(1, 1, 11), row(1, 3, 13), row(1, 3, 14), row(2, 1, 11)]);
    assert_eq!(check_split(&a, &b), Ok(()));
}

// REQ: OA-BENCH-119
#[test]
fn a_report_time_in_both_sets_is_a_violation() {
    let a = set(&[row(1, 0, 10), row(1, 2, 12)]);
    // Same address and time as a report of set A, different content.
    let b = set(&[row(1, 1, 11), row(1, 2, 99)]);
    assert_eq!(
        check_split(&a, &b),
        Err(SplitViolation::SharedTime {
            addr: 1,
            time_ns: 1_700_000_002_000_000_000
        })
    );
    // The same time at another address is not shared.
    let b = set(&[row(2, 2, 99)]);
    assert_eq!(check_split(&a, &b), Ok(()));
}

// REQ: OA-BENCH-119
#[test]
fn a_copy_inside_a_set_is_a_violation() {
    let a = set(&[row(1, 0, 10), row(1, 0, 10)]);
    let b = set(&[row(1, 1, 11)]);
    assert_eq!(
        check_split(&a, &b),
        Err(SplitViolation::Duplicate {
            set: b'A',
            addr: 1,
            time_ns: 1_700_000_000_000_000_000
        })
    );
    let a = set(&[row(1, 0, 10)]);
    let b = set(&[row(1, 1, 11), row(1, 1, 11)]);
    assert!(matches!(
        check_split(&a, &b),
        Err(SplitViolation::Duplicate { set: b'B', .. })
    ));
}

// REQ: OA-BENCH-121
#[test]
fn a_stale_report_and_a_current_one_at_one_time_are_not_copies() {
    let current = row(1, 0, 10);
    let mut stale = current;
    stale.flags = oa_adsb::F_STALE;
    let mut mlat = current;
    mlat.source = 3;
    assert_eq!(
        check_split(&set(&[current, stale, mlat]), &set(&[row(1, 1, 11)])),
        Ok(())
    );
    assert_ne!(
        oa_bench::split_check::observation_key(&current),
        oa_bench::split_check::observation_key(&stale)
    );
    // A copy that differs only in an unread field is still a copy.
    let mut named = current;
    named.callsign = *b"ABC123  ";
    assert_eq!(
        check_split(&set(&[current, named]), &set(&[])),
        Err(SplitViolation::Duplicate {
            set: b'A',
            addr: 1,
            time_ns: current.time_ns
        })
    );
}
