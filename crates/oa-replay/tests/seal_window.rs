//! The recording reader drops and counts the frames whose time is in a sealed window (OA-SEAL-014).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod common;
use common::*;
use oa_feed::{encode_frame, FLAG_BYTES, MAX_FRAME_LEN};
use oa_replay::{merge_streams, spec_from_bytes, FeedKind};
use oa_replay::{new_pipeline, process_frame, FeedKind as Feed, PipelineConfig};

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

// REQ: OA-SEAL-014
#[test]
fn a_row_timed_in_a_window_is_not_applied_though_its_frame_was_received_before_it() {
    // 2026-11-20T00:00:00.000Z in Unix milliseconds.
    // The fixtures count milliseconds from `DAY`.
    let from_ms = FROM / 1_000_000 - DAY;
    let mut p = new_pipeline(PipelineConfig::default()).unwrap();
    // One frame received at from - 1 ns: the row timed at from - 1 ms is applied, the rows timed
    // at from and at to - 1 ms are dropped and counted, the row timed at to is applied.
    let to_ms = TO / 1_000_000 - DAY;
    let doc = sf_doc(&[
        sf(1, from_ms - 1),
        sf(2, from_ms),
        sf(3, to_ms - 1),
        sf(4, to_ms),
    ]);
    process_frame(
        &mut p,
        Feed::Sfdps,
        oa_feed::Kind::Message,
        FROM - 1,
        doc.as_bytes(),
    );
    assert_eq!(p.sealed_window_dropped, 2);
    assert_eq!(p.rows_seen[Feed::Sfdps as usize], 2);
}
