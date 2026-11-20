//! The recording reader drops and counts the frames whose time is in a sealed window (OA-SEAL-014).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_feed::{encode_frame, FLAG_BYTES, MAX_FRAME_LEN};
use oa_replay::{merge_streams, spec_from_bytes, FeedKind};

/// The window of the list: 2026-11-20T00:00:00Z to 2026-11-23T00:17:00Z, Unix nanoseconds.
const FROM: i64 = 1_795_132_800_000_000_000;
const TO: i64 = 1_795_393_020_000_000_000;

fn file(times: &[i64]) -> Vec<u8> {
    let mut out = Vec::new();
    for (i, t) in times.iter().enumerate() {
        let mut buf = vec![0u8; MAX_FRAME_LEN];
        let n = encode_frame(&mut buf, 1, FLAG_BYTES, i as u64, *t, t / 1_000_000, b"x").unwrap();
        out.extend_from_slice(&buf[..n]);
    }
    out
}

// REQ: OA-SEAL-014
#[test]
fn frames_at_from_minus_one_and_at_to_are_kept_and_those_inside_are_dropped_and_counted() {
    let bytes = file(&[FROM - 1, FROM, TO - 1, TO]);
    let mut seen = Vec::new();
    let report = merge_streams(
        vec![spec_from_bytes(FeedKind::Sfdps, vec![bytes])],
        4096,
        &mut |m| seen.push(m.recv_ns),
    )
    .unwrap();
    assert_eq!(seen, [FROM - 1, TO]);
    assert_eq!(report.sealed_window_dropped, 2);
    // Nothing in the window: the count is 0.
    let bytes = file(&[FROM - 1, TO]);
    let report = merge_streams(
        vec![spec_from_bytes(FeedKind::Sfdps, vec![bytes])],
        4096,
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(report.sealed_window_dropped, 0);
}
