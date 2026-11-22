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

/// The window of the list: 2026-11-19T23:45:00Z to 2026-11-23T00:32:00Z (the listed window widened by 15 minutes), Unix nanoseconds.
const FROM: i64 = 1_795_131_900_000_000_000;
const TO: i64 = 1_795_393_920_000_000_000;

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
    // Frames received at from - 1 ns, one row each: the rows timed at from - 1 ms and at to are
    // applied; the rows timed at from and at to - 1 ms drop their whole document, and are counted.
    let to_ms = TO / 1_000_000 - DAY;
    for t in [from_ms - 1, from_ms, to_ms - 1, to_ms] {
        let doc = sf_doc(&[sf(1, t)]);
        process_frame(
            &mut p,
            Feed::Sfdps,
            oa_feed::Kind::Message,
            FROM - 1,
            doc.as_bytes(),
        );
    }
    assert_eq!(p.sealed_window_dropped, 2);
    assert_eq!(p.rows_seen[Feed::Sfdps as usize], 2);
    // A dropped document is in no count but the drop: not a frame, not a message.
    assert_eq!(
        (
            p.frames[Feed::Sfdps as usize],
            p.messages[Feed::Sfdps as usize]
        ),
        (2, 2)
    );
    // One document with a row in the window and others outside it is dropped whole.
    let mut q = new_pipeline(PipelineConfig::default()).unwrap();
    let doc = sf_doc(&[sf(1, from_ms - 1), sf(2, from_ms), sf(3, to_ms)]);
    process_frame(
        &mut q,
        Feed::Sfdps,
        oa_feed::Kind::Message,
        FROM - 1,
        doc.as_bytes(),
    );
    assert_eq!(q.sealed_window_dropped, 3);
    assert_eq!(q.rows_seen[Feed::Sfdps as usize], 0);
    assert_eq!(
        (
            q.frames[Feed::Sfdps as usize],
            q.messages[Feed::Sfdps as usize]
        ),
        (0, 0)
    );
    assert!(q.last_doc_dropped);
}

// REQ: OA-SEAL-014
#[test]
fn an_adsb_frame_with_a_row_in_a_window_is_dropped_whole_and_is_in_no_other_count() {
    let payload = |times: &[i64]| {
        let mut r = oa_adsb::Reports::default();
        for (k, t) in times.iter().enumerate() {
            let mut row = oa_adsb::ROW_EMPTY;
            row.time_ns = *t;
            row.icao = 0xA00001 + k as u32;
            row.lat_e7 = 410_000_000;
            row.lon_e7 = -880_000_000;
            row.nic = 8;
            row.nac_p = 10;
            row.sil = 3;
            row.version = 2;
            oa_adsb::reports_push(&mut r, &row);
        }
        let mut payload = Vec::new();
        oa_adsb::encode_payload(&mut payload, &r, 0, times.len()).unwrap();
        payload
    };
    // One frame, received before the window, with rows at from - 1 ns, from, to - 1 ns and to:
    // two rows are inside, so the whole frame is dropped and its four rows are counted.
    let mut p = new_pipeline(PipelineConfig::default()).unwrap();
    process_frame(
        &mut p,
        Feed::Adsb,
        oa_feed::Kind::Message,
        FROM - 1,
        &payload(&[FROM - 1, FROM, TO - 1, TO]),
    );
    assert_eq!(p.sealed_window_dropped, 4);
    assert!(p.last_doc_dropped);
    assert_eq!(p.rows_seen[Feed::Adsb as usize], 0);
    assert_eq!(
        (
            p.frames[Feed::Adsb as usize],
            p.messages[Feed::Adsb as usize]
        ),
        (0, 0)
    );
    assert_eq!(p.adsb.live, 0);
    // A frame with every row outside it is applied and counted as usual.
    process_frame(
        &mut p,
        Feed::Adsb,
        oa_feed::Kind::Message,
        FROM - 1,
        &payload(&[FROM - 1, TO]),
    );
    assert_eq!(p.sealed_window_dropped, 4);
    assert_eq!(p.rows_seen[Feed::Adsb as usize], 2);
    assert_eq!(
        (
            p.frames[Feed::Adsb as usize],
            p.messages[Feed::Adsb as usize]
        ),
        (1, 1)
    );
    // The bytes of the dropped frame are not in the byte count either.
    let frame = |seq: u64, times: &[i64]| {
        let body = payload(times);
        let mut buf = vec![0u8; MAX_FRAME_LEN];
        let n = encode_frame(&mut buf, 1, FLAG_BYTES, seq, FROM - 1, 0, &body).unwrap();
        buf[..n].to_vec()
    };
    let kept = frame(0, &[FROM - 1, TO]);
    let mut all = kept.clone();
    all.extend(frame(1, &[FROM, TO]));
    let mut q = new_pipeline(PipelineConfig::default()).unwrap();
    let rep = oa_replay::run_streams(
        &mut q,
        vec![spec_from_bytes(FeedKind::Adsb, vec![all])],
        4096,
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(q.sealed_window_dropped, 2);
    assert_eq!(q.frames[Feed::Adsb as usize], 1);
    assert_eq!(rep.bytes, kept.len() as u64);
}

// REQ: OA-SEAL-014
#[test]
fn every_decoder_of_the_pipeline_is_built_with_the_sealed_window_as_its_drop_window() {
    let sealed = oa_sealed::DropWindow::sealed();
    assert_eq!(sealed.span(), oa_sealed::sealed_window_ms());
    assert!(sealed.span().is_some());
    assert_eq!(oa_replay::doc_fixm_limits().drop_window, sealed);
    assert_eq!(oa_replay::doc_notam_limits().drop_window, sealed);
    assert_eq!(oa_replay::new_doc_tbfm_rows().window, sealed);
    assert_eq!(oa_replay::new_doc_tfms_rows().window, sealed);
}

// REQ: OA-SEAL-014
#[test]
fn the_byte_count_is_the_bytes_after_the_window_gate() {
    let kept = file(&[FROM - 1, TO]);
    let all = file(&[FROM - 1, FROM, TO - 1, TO]);
    let run = |b: Vec<u8>| {
        merge_streams(
            vec![spec_from_bytes(FeedKind::Sfdps, vec![b])],
            4096,
            &mut |_| {},
        )
        .unwrap()
    };
    let (a, b) = (run(all.clone()), run(kept.clone()));
    // Two frames of the same size were dropped: their bytes are not in the count.
    assert_eq!(a.sealed_window_dropped, 2);
    assert_eq!(a.bytes, kept.len() as u64);
    assert_eq!(b.bytes, kept.len() as u64);
}

// REQ: OA-SEAL-014
#[test]
fn the_bytes_of_a_document_dropped_whole_are_not_in_the_byte_count() {
    let from_ms = FROM / 1_000_000 - DAY;
    let frame = |seq: u64, t_ms: i64| {
        let doc = sf_doc(&[sf(1, t_ms)]);
        let mut buf = vec![0u8; MAX_FRAME_LEN];
        let n = encode_frame(&mut buf, 1, FLAG_BYTES, seq, FROM - 1, 0, doc.as_bytes()).unwrap();
        buf[..n].to_vec()
    };
    // Two frames received before the window: one with a row timed outside it, one inside it.
    let kept = frame(0, from_ms - 1);
    let mut all = kept.clone();
    all.extend(frame(1, from_ms));
    let run = |b: Vec<u8>| {
        let mut p = new_pipeline(PipelineConfig::default()).unwrap();
        let rep = oa_replay::run_streams(
            &mut p,
            vec![spec_from_bytes(FeedKind::Sfdps, vec![b])],
            4096,
            &mut |_| {},
        )
        .unwrap();
        (p, rep)
    };
    let (p, rep) = run(all);
    assert_eq!(p.sealed_window_dropped, 1);
    assert_eq!(p.frames[Feed::Sfdps as usize], 1);
    assert_eq!(rep.bytes, kept.len() as u64);
}

// REQ: OA-SEAL-014
#[test]
fn input_fingerprint_excludes_both_sealed_drop_gates() {
    let from_ms = FROM / 1_000_000 - DAY;
    let frame = |seq: u64, recv_ns: i64, row_ms: i64| {
        let doc = sf_doc(&[sf(1, row_ms)]);
        let mut buf = vec![0u8; MAX_FRAME_LEN];
        let n = encode_frame(&mut buf, 1, FLAG_BYTES, seq, recv_ns, 0, doc.as_bytes()).unwrap();
        buf[..n].to_vec()
    };
    let kept = frame(0, FROM - 1, from_ms - 1);
    let document_dropped = frame(1, FROM - 1, from_ms);
    let source_dropped = file(&[FROM]);
    let mut all = kept.clone();
    all.extend(document_dropped);
    all.extend(source_dropped);

    let run = |bytes: Vec<u8>| {
        let mut p = new_pipeline(PipelineConfig::default()).unwrap();
        let mut fingerprint = oa_replay::input_fingerprint::InputFingerprint::default();
        let mut sealed_frames = 0;
        let report = oa_replay::run_streams_observed(
            &mut p,
            vec![spec_from_bytes(FeedKind::Sfdps, vec![bytes])],
            4096,
            &mut |_| Ok(()),
            &mut sealed_frames,
            &mut |m| fingerprint.observe(m),
        )
        .unwrap();
        (p, fingerprint, sealed_frames, report)
    };
    let (p, fingerprint, source_drops, _) = run(all);
    let (_, baseline, baseline_source_drops, _) = run(kept);
    assert_eq!(source_drops, 1);
    assert_eq!(baseline_source_drops, 0);
    assert_eq!(p.sealed_window_dropped, 1);
    assert_eq!(fingerprint.frames(), 1);
    assert_eq!(fingerprint.finalize(), baseline.finalize());
}

// REQ: OA-SEAL-014
#[test]
fn a_dropped_adsb_frame_adds_to_no_error_count_even_with_a_bad_record() {
    let mut r = oa_adsb::Reports::default();
    for (k, t) in [FROM, FROM - 1, TO].into_iter().enumerate() {
        let mut row = oa_adsb::ROW_EMPTY;
        row.time_ns = t;
        row.icao = 0xA00001 + k as u32;
        row.lat_e7 = 410_000_000;
        row.lon_e7 = -880_000_000;
        row.nic = 8;
        row.nac_p = 10;
        row.sil = 3;
        row.version = 2;
        oa_adsb::reports_push(&mut r, &row);
    }
    let mut payload = Vec::new();
    oa_adsb::encode_payload(&mut payload, &r, 0, 3).unwrap();
    // Record 1 has a latitude out of range: read on its own it is a bad record, counted.
    let o = 16 + oa_adsb::RECORD_LEN + 4;
    payload[o] = 0x7f;
    payload[o + 3] = 0x7f;
    let errors = |p: &oa_replay::Pipeline| p.errors.iter().sum::<u64>();
    let mut p = new_pipeline(PipelineConfig::default()).unwrap();
    process_frame(
        &mut p,
        Feed::Adsb,
        oa_feed::Kind::Message,
        FROM - 1,
        &payload,
    );
    // Row 0 is in the window: the frame is dropped whole, and nothing is counted but the drop
    // (the rows that could be read: the bad record is not one of them).
    assert!(p.last_doc_dropped);
    assert_eq!(p.sealed_window_dropped, 2);
    assert_eq!((errors(&p), p.rows_seen[Feed::Adsb as usize]), (0, 0));
    // The same frame with row 0 outside the window: the bad record is counted.
    let mut r2 = oa_adsb::Reports::default();
    for (k, t) in [FROM - 2, FROM - 1, TO].into_iter().enumerate() {
        let mut row = oa_adsb::ROW_EMPTY;
        row.time_ns = t;
        row.icao = 0xA00001 + k as u32;
        row.lat_e7 = 410_000_000;
        row.lon_e7 = -880_000_000;
        row.nic = 8;
        row.nac_p = 10;
        row.sil = 3;
        row.version = 2;
        oa_adsb::reports_push(&mut r2, &row);
    }
    let mut payload = Vec::new();
    oa_adsb::encode_payload(&mut payload, &r2, 0, 3).unwrap();
    payload[o] = 0x7f;
    payload[o + 3] = 0x7f;
    let mut q = new_pipeline(PipelineConfig::default()).unwrap();
    process_frame(
        &mut q,
        Feed::Adsb,
        oa_feed::Kind::Message,
        FROM - 1,
        &payload,
    );
    assert!(!q.last_doc_dropped);
    assert_eq!(errors(&q), 1);
    assert_eq!(q.rows_seen[Feed::Adsb as usize], 2);
}
