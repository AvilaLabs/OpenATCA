//! A frame with any report timed in a sealed window is left out whole (OA-SEAL-014).
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use oa_adsb::{reports_extend_unsealed, reports_len, reports_push, Reports, ROW_EMPTY};

/// The window of the list: 2026-11-19T23:45:00Z to 2026-11-23T00:32:00Z (the listed window widened
/// by 15 minutes), Unix nanoseconds.
const FROM: i64 = 1_795_131_900_000_000_000;
const TO: i64 = 1_795_393_920_000_000_000;

fn frame(times: &[i64]) -> Reports {
    let mut r = Reports::default();
    for (i, t) in times.iter().enumerate() {
        let mut row = ROW_EMPTY;
        row.icao = 0xA00000 + i as u32;
        row.time_ns = *t;
        reports_push(&mut r, &row);
    }
    r
}

// REQ: OA-SEAL-014
#[test]
fn extend_leaves_out_a_whole_frame_with_one_report_in_the_window() {
    let mut dst = Reports::default();
    // One report in the window: all four are counted as dropped and none is appended.
    let gone = reports_extend_unsealed(&mut dst, &frame(&[FROM - 1, FROM, TO - 1, TO]));
    assert_eq!((gone, reports_len(&dst)), (4, 0));
    // A frame wholly outside the window is appended, and nothing is dropped.
    let gone = reports_extend_unsealed(&mut dst, &frame(&[FROM - 1, TO]));
    assert_eq!((gone, dst.time_ns.clone()), (0, vec![FROM - 1, TO]));
    // The first and last instant of the window.
    for t in [FROM, TO - 1] {
        assert_eq!(reports_extend_unsealed(&mut dst, &frame(&[t])), 1);
    }
    assert_eq!(reports_len(&dst), 2);
}
