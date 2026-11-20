#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
//! Unit tests for oa-feed. A tiny Rust encoder ([`encode_frame`]) builds every fixture, so no
//! Java is needed; the one golden vector was produced by the bridge's own Java encoder.

use std::path::PathBuf;

use crate::cli::{self, Command, Source};
use crate::format_summary;
use crate::frame::{encode_frame, ErrorKind, Frame, FrameError, Kind};
use crate::health::{
    lat_bin, lat_bin_upper_ms, latency_quantile_ms, observe_error, observe_frame, observe_skipped,
    rates_milli, FeedHealth, LAT_BINS, STATE_CONNECTED, STATE_DISCONNECTED,
};
use crate::reader::FrameReader;
use crate::record::{format_recording, utc_stamp, Format, Opener, Outcome, Recorder};
use crate::replay::{replay_files, replay_sources, GzNoteKind};
use crate::{
    encode_frame_mono, FLAG_BYTES, FLAG_REDELIVERED, HEADER_LEN, HEADER_LEN_V2, MAGIC,
    MAX_FRAME_LEN, MAX_PAYLOAD, TRAILER_LEN,
};

// ---------------------------------------------------------------- helpers

fn frame(kind: u8, flags: u8, seq: u64, recv_ns: i64, jms_ms: i64, payload: &[u8]) -> Vec<u8> {
    let mut v = vec![0u8; HEADER_LEN + payload.len() + TRAILER_LEN];
    let n = encode_frame(&mut v, kind, flags, seq, recv_ns, jms_ms, payload).unwrap();
    assert_eq!(n, v.len());
    v
}

fn msg(seq: u64, payload: &[u8]) -> Vec<u8> {
    frame(
        1,
        0,
        seq,
        1_700_000_000_000_000_000 + seq as i64 * 1_000_000,
        0,
        payload,
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Got {
    kind: Kind,
    flags: u8,
    seq: u64,
    offset: u64,
    skipped_before: u64,
    payload: Vec<u8>,
    raw_len: usize,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Run {
    frames: Vec<Got>,
    errors: Vec<FrameError>,
    skipped: u64,
}

/// Feeds `chunks` through a reader (the way the shell does), then finishes.
fn run_chunks(buf: &mut [u8], chunks: &[&[u8]]) -> Run {
    let mut r = FrameReader::new(buf).unwrap();
    let mut run = Run::default();
    let drain = |r: &mut FrameReader<'_>, run: &mut Run| {
        while let Some(ev) = r.next() {
            match ev {
                Ok(f) => run.frames.push(Got {
                    kind: f.kind,
                    flags: f.flags,
                    seq: f.seq,
                    offset: f.offset,
                    skipped_before: f.skipped_before,
                    payload: f.payload.to_vec(),
                    raw_len: f.raw.len(),
                }),
                Err(e) => run.errors.push(e),
            }
        }
    };
    for c in chunks {
        let mut off = 0;
        while off < c.len() {
            let took = r.push(&c[off..]);
            off += took;
            drain(&mut r, &mut run);
            assert!(took > 0 || off == c.len(), "reader made no progress");
        }
    }
    drain(&mut r, &mut run);
    if let Some(e) = r.finish() {
        run.errors.push(e);
    }
    run.skipped = r.skipped_total();
    run
}

fn run_whole(buf: &mut [u8], data: &[u8]) -> Run {
    run_chunks(buf, &[data])
}

fn big_buf() -> Vec<u8> {
    vec![0u8; MAX_FRAME_LEN]
}

// REQ: OA-FEED-181
#[test]
fn checked_frame_work_refusal_preserves_input_for_retry() {
    let f = msg(42, b"retry after a zero-byte CRC allowance");
    let mut buf = big_buf();
    let mut r = FrameReader::new(&mut buf).unwrap();
    assert_eq!(
        r.push_with_budget(&f, crate::FrameWorkBudget::for_capacity(MAX_FRAME_LEN)),
        Ok(f.len())
    );
    let refusal = r
        .next_with_budget(crate::FrameWorkBudget {
            crc_bytes: 0,
            ..crate::FrameWorkBudget::for_capacity(MAX_FRAME_LEN)
        })
        .unwrap_err();
    assert_eq!(refusal.kind, crate::FrameWorkKind::CrcBytes);
    assert_eq!(r.offset(), 0);
    assert_eq!(r.work_stats().crc_bytes, 0);
    let decoded = r
        .next_with_budget(crate::FrameWorkBudget::for_capacity(MAX_FRAME_LEN))
        .unwrap();
    assert_eq!(decoded.unwrap().unwrap().seq, 42);
    assert_eq!(r.work_stats().crc_bytes, (f.len() - TRAILER_LEN) as u64);
}

// REQ: OA-FEED-182
#[test]
fn checked_push_refusal_is_atomic_and_retry_copies_all_bytes() {
    let f = msg(9, b"copied only after budget approval");
    let mut buf = big_buf();
    let mut r = FrameReader::new(&mut buf).unwrap();
    let budget = crate::FrameWorkBudget {
        copied_bytes: 0,
        ..crate::FrameWorkBudget::for_capacity(MAX_FRAME_LEN)
    };
    let refusal = r.push_with_budget(&f, budget).unwrap_err();
    assert_eq!(refusal.kind, crate::FrameWorkKind::CopiedBytes);
    assert_eq!(r.offset(), 0);
    assert_eq!(r.work_stats().copied_bytes, 0);
    assert_eq!(
        r.push_with_budget(&f, crate::FrameWorkBudget::for_capacity(MAX_FRAME_LEN)),
        Ok(f.len())
    );
    assert_eq!(r.work_stats().copied_bytes, f.len() as u64);
}

// REQ: OA-FEED-183
#[test]
fn corrupt_maximum_frame_spends_exact_crc_work_and_is_refused() {
    let mut raw = vec![0u8; MAX_FRAME_LEN];
    assert_eq!(
        encode_frame_mono(&mut raw, 1, 0, 17, 0, 0, 7, &vec![3u8; MAX_PAYLOAD]),
        Some(MAX_FRAME_LEN)
    );
    let last = raw.len() - 1;
    raw[last] ^= 1;
    let mut buf = big_buf();
    let mut r = FrameReader::new(&mut buf).unwrap();
    r.push_with_budget(&raw, crate::FrameWorkBudget::for_capacity(MAX_FRAME_LEN))
        .unwrap();
    let refusal = r
        .next_with_budget(crate::FrameWorkBudget {
            crc_bytes: (MAX_FRAME_LEN - TRAILER_LEN - 1) as u64,
            ..crate::FrameWorkBudget::for_capacity(MAX_FRAME_LEN)
        })
        .unwrap_err();
    assert_eq!(refusal.kind, crate::FrameWorkKind::CrcBytes);
    assert_eq!(r.work_stats().crc_bytes, 0);
    let damaged = r
        .next_with_budget(crate::FrameWorkBudget::for_capacity(MAX_FRAME_LEN))
        .unwrap();
    assert!(matches!(
        damaged,
        Some(Err(FrameError {
            kind: ErrorKind::Crc { .. },
            ..
        }))
    ));
    assert_eq!(
        r.work_stats().crc_bytes,
        (MAX_FRAME_LEN - TRAILER_LEN) as u64
    );
}

// REQ: OA-FEED-184
#[test]
fn checked_resync_scan_refusal_keeps_all_bytes_for_retry() {
    let f = msg(27, b"after damaged prefix");
    let input = cat(&[b"garbage", &f]);
    let mut buf = big_buf();
    let mut r = FrameReader::new(&mut buf).unwrap();
    r.push_with_budget(&input, crate::FrameWorkBudget::for_capacity(MAX_FRAME_LEN))
        .unwrap();
    let bad_magic = r
        .next_with_budget(crate::FrameWorkBudget::for_capacity(MAX_FRAME_LEN))
        .unwrap();
    assert!(matches!(
        bad_magic,
        Some(Err(FrameError {
            kind: ErrorKind::BadMagic { .. },
            offset: 0
        }))
    ));
    assert_eq!(r.offset(), 1);
    let budget = crate::FrameWorkBudget {
        scan_positions: 0,
        ..crate::FrameWorkBudget::for_capacity(MAX_FRAME_LEN)
    };
    let refusal = r.next_with_budget(budget).unwrap_err();
    assert_eq!(refusal.kind, crate::FrameWorkKind::ScanPositions);
    assert_eq!(r.offset(), 1);
    assert_eq!(r.work_stats().scan_positions, 0);
    let decoded = r
        .next_with_budget(crate::FrameWorkBudget::for_capacity(MAX_FRAME_LEN))
        .unwrap();
    assert_eq!(decoded.unwrap().unwrap().seq, 27);
    assert_eq!(r.work_stats().scan_positions, 7);
    assert_eq!(r.skipped_total(), 7);
}

// REQ: OA-FEED-185
#[test]
fn remaining_budget_caps_overlapping_bad_frame_crc_and_retries_valid_tail() {
    const OVERLAP: usize = 44;
    let tail = msg(88, b"valid tail");
    let total = MAX_FRAME_LEN + OVERLAP + tail.len();
    let mut input = vec![0u8; total];
    let payload = vec![5u8; MAX_PAYLOAD];
    assert_eq!(
        encode_frame_mono(&mut input[..MAX_FRAME_LEN], 1, 0, 1, 0, 0, 7, &payload),
        Some(MAX_FRAME_LEN)
    );
    assert_eq!(
        encode_frame_mono(
            &mut input[OVERLAP..OVERLAP + MAX_FRAME_LEN],
            1,
            0,
            2,
            0,
            0,
            8,
            &payload
        ),
        Some(MAX_FRAME_LEN)
    );
    let tail_start = MAX_FRAME_LEN + OVERLAP;
    input[tail_start..].copy_from_slice(&tail);
    let damaged_crc_byte = MAX_FRAME_LEN - 1;
    input[damaged_crc_byte] ^= 1;

    let mut storage = vec![0u8; total];
    let mut r = FrameReader::new(&mut storage).unwrap();
    let mut push_budget = crate::FrameWorkBudget::for_capacity(total);
    assert_eq!(
        r.push_with_remaining_budget(&input, &mut push_budget),
        Ok(total)
    );
    let crc_work = (MAX_FRAME_LEN - TRAILER_LEN) as u64;
    let mut batch = crate::FrameWorkBudget {
        crc_bytes: crc_work,
        ..crate::FrameWorkBudget::for_capacity(total)
    };
    assert!(matches!(
        r.next_with_remaining_budget(&mut batch).unwrap(),
        Some(Err(FrameError {
            kind: ErrorKind::Crc { .. },
            offset: 0
        }))
    ));
    assert_eq!(batch.crc_bytes, 0);
    assert_eq!(r.work_stats().crc_bytes, crc_work);

    let refusal = r.next_with_remaining_budget(&mut batch).unwrap_err();
    assert_eq!(refusal.kind, crate::FrameWorkKind::CrcBytes);
    assert_eq!(refusal.required, crc_work);
    assert_eq!(refusal.available, 0);
    assert_eq!(r.offset(), OVERLAP as u64);
    assert_eq!(r.work_stats().crc_bytes, crc_work);

    let mut retry = crate::FrameWorkBudget::for_capacity(total);
    assert!(matches!(
        r.next_with_remaining_budget(&mut retry).unwrap(),
        Some(Err(FrameError {
            kind: ErrorKind::Crc { .. },
            offset
        })) if offset == OVERLAP as u64
    ));
    let mut retry = crate::FrameWorkBudget::for_capacity(total);
    let valid = r.next_with_remaining_budget(&mut retry).unwrap();
    assert_eq!(valid.unwrap().unwrap().seq, 88);
}

// REQ: OA-FEED-186
#[test]
fn refusal_retry_classifier_requires_the_matching_fresh_allowance() {
    let fresh = crate::FrameWorkBudget::for_capacity(MAX_FRAME_LEN * 2);
    let cases = [
        (crate::FrameWorkKind::ScanPositions, fresh.scan_positions),
        (crate::FrameWorkKind::CrcBytes, fresh.crc_bytes),
        (crate::FrameWorkKind::CopiedBytes, fresh.copied_bytes),
        (crate::FrameWorkKind::CompactedBytes, fresh.compacted_bytes),
    ];
    for (kind, allowance) in cases {
        assert!(crate::frame_work_refusal_retryable(
            crate::FrameWorkRefusal {
                kind,
                required: allowance,
                available: 0,
            },
            fresh,
        ));
        assert!(!crate::frame_work_refusal_retryable(
            crate::FrameWorkRefusal {
                kind,
                required: allowance + 1,
                available: 0,
            },
            fresh,
        ));
    }
    assert!(!crate::frame_work_refusal_retryable(
        crate::FrameWorkRefusal {
            kind: crate::FrameWorkKind::CounterOverflow,
            required: 1,
            available: u64::MAX,
        },
        fresh,
    ));
}

fn cat(parts: &[&[u8]]) -> Vec<u8> {
    parts.iter().flat_map(|p| p.iter().copied()).collect()
}

/// Conservation: every input byte is either in a delivered frame or counted skipped.
fn assert_conserved(run: &Run, input_len: usize) {
    let delivered: u64 = run.frames.iter().map(|f| f.raw_len as u64).sum();
    assert_eq!(
        delivered + run.skipped,
        input_len as u64,
        "bytes unaccounted for"
    );
}

/// Deterministic mixed stream: messages (text and bytes, some redelivered, a gap), rejects,
/// statuses, with spread-out receive times and latencies.
fn fixture() -> Vec<u8> {
    let mut out = Vec::new();
    let base: i64 = 1_700_000_000_000_000_000;
    let mut seq = 0u64;
    let push = |out: &mut Vec<u8>,
                kind: u8,
                flags: u8,
                dt_ms: i64,
                lat_ms: i64,
                p: &[u8],
                seq: &mut u64| {
        let recv = base + dt_ms * 1_000_000;
        out.extend(frame(kind, flags, *seq, recv, recv / 1_000_000 - lat_ms, p));
        *seq += 1;
    };
    push(&mut out, 3, 0, 0, 0, b"state=connected\n", &mut seq);
    for i in 0..150i64 {
        let flags = if i % 17 == 0 {
            FLAG_REDELIVERED
        } else if i % 5 == 0 {
            FLAG_BYTES
        } else {
            0
        };
        let payload = vec![b'x'; (i as usize * 7) % 900];
        push(
            &mut out,
            1,
            flags,
            10 + i * 37,
            (i * 13) % 400,
            &payload,
            &mut seq,
        );
        if i == 60 {
            seq += 3; // three frames lost upstream
        }
        if i == 90 {
            push(
                &mut out,
                2,
                0,
                10 + i * 37 + 1,
                0,
                b"reason=oversize\nsize=5000000\njms_id=ID:1\n",
                &mut seq,
            );
        }
        if i == 120 {
            push(
                &mut out,
                3,
                0,
                10 + i * 37 + 1,
                0,
                b"state=disconnected\n",
                &mut seq,
            );
            push(
                &mut out,
                3,
                0,
                10 + i * 37 + 2,
                0,
                b"state=reconnect_attempt\ndelay_ms=2500\n",
                &mut seq,
            );
            push(
                &mut out,
                3,
                0,
                10 + i * 37 + 900,
                0,
                b"state=connected\n",
                &mut seq,
            );
        }
    }
    out
}

fn tmpdir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("oa-feed-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn fr<'a>(
    kind: Kind,
    flags: u8,
    seq: u64,
    recv_ns: i64,
    jms_ms: i64,
    payload: &'a [u8],
) -> Frame<'a> {
    Frame::new(seq, recv_ns, jms_ms, None, 0, 0, payload, &[], kind, flags)
}

const SEC: i64 = 1_000_000_000;

// ---------------------------------------------------------------- framing

// REQ: OA-FEED-001
#[test]
fn frame_round_trip_every_kind_and_field() {
    let a = frame(1, FLAG_REDELIVERED | FLAG_BYTES, 0, -5, 123, b"payload one");
    let b = frame(2, 0, 1, i64::MAX, i64::MIN, b"");
    let c = frame(3, 0xff, u64::MAX, 0, 0, &[0u8; 300]);
    let stream = cat(&[&a, &b, &c]);
    let mut buf = big_buf();
    let mut r = FrameReader::new(&mut buf).unwrap();
    assert_eq!(r.push(&stream), stream.len());
    let f = r.next().unwrap().unwrap();
    assert_eq!(
        (f.kind, f.flags, f.seq, f.recv_unix_ns, f.jms_timestamp_ms),
        (Kind::Message, 3, 0, -5, 123)
    );
    assert_eq!(
        (f.payload, f.raw, f.offset, f.skipped_before),
        (&b"payload one"[..], &a[..], 0, 0)
    );
    let f = r.next().unwrap().unwrap();
    assert_eq!(
        (f.kind, f.seq, f.recv_unix_ns, f.jms_timestamp_ms),
        (Kind::Reject, 1, i64::MAX, i64::MIN)
    );
    assert!(f.payload.is_empty());
    assert_eq!(f.offset, a.len() as u64);
    let f = r.next().unwrap().unwrap();
    assert_eq!(
        (f.kind, f.flags, f.seq, f.payload.len()),
        (Kind::Status, 0xff, u64::MAX, 300)
    );
    assert!(r.next().is_none());
    assert!(r.finish().is_none());
    assert_eq!(r.skipped_total(), 0);
}

// REQ: OA-FEED-002
#[test]
fn golden_vector_from_the_java_encoder() {
    // Frame.encode(KIND_MESSAGE, flags 3, seq 7, recv 1_700_000_000_123_456_789 ns,
    // jms 1_700_000_000_100 ms, "hello") in adapters/scds-bridge.
    let golden: [u8; 45] = [
        0x4f, 0x41, 0x46, 0x31, 0x01, 0x03, 0x00, 0x00, 0x07, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x15, 0xcd, 0x85, 0x3d, 0xfe, 0x9c, 0x97, 0x17, 0x64, 0x68, 0xe5, 0xcf, 0x8b, 0x01,
        0x00, 0x00, 0x05, 0x00, 0x00, 0x00, 0x68, 0x65, 0x6c, 0x6c, 0x6f, 0x53, 0x20, 0xae, 0x9c,
    ];
    let ours = frame(
        1,
        3,
        7,
        1_700_000_000_123_456_789,
        1_700_000_000_100,
        b"hello",
    );
    assert_eq!(&ours[..], &golden[..]);
    let mut buf = big_buf();
    let run = run_whole(&mut buf, &golden);
    assert_eq!(run.frames.len(), 1);
    assert!(run.errors.is_empty());
    assert_eq!(run.frames[0].payload, b"hello");
}

// REQ: OA-FEED-003
#[test]
fn chunking_at_every_byte_boundary_gives_identical_frames() {
    let stream = fixture();
    let mut buf = big_buf();
    let whole = run_whole(&mut buf, &stream);
    assert!(whole.errors.is_empty());
    assert_eq!(whole.frames.len(), 150 + 1 + 1 + 3);
    // Every two-chunk split of the head of the stream, and of the tail.
    let n = stream.len();
    for k in (0..=400).chain(n - 400..=n) {
        let run = run_chunks(&mut buf, &[&stream[..k], &stream[k..]]);
        assert_eq!(run, whole, "split at {k}");
    }
    for size in [1usize, 2, 3, 5, 36, 37, 64, 1000] {
        let chunks: Vec<&[u8]> = stream.chunks(size).collect();
        assert_eq!(run_chunks(&mut buf, &chunks), whole, "chunk size {size}");
    }
}

// REQ: OA-FEED-004
#[test]
fn crc_error_is_reported_and_the_reader_resyncs() {
    let (f1, mut f2, f3) = (msg(0, b"first"), msg(1, b"second frame"), msg(2, b"third"));
    f2[HEADER_LEN + 2] ^= 0x40; // payload bit flip
    let stream = cat(&[&f1, &f2, &f3]);
    let mut buf = big_buf();
    for k in 0..=stream.len() {
        let run = run_chunks(&mut buf, &[&stream[..k], &stream[k..]]);
        assert_eq!(
            run.frames.iter().map(|f| f.seq).collect::<Vec<_>>(),
            vec![0, 2],
            "split {k}"
        );
        assert_eq!(run.errors.len(), 1, "split {k}");
        let e = run.errors[0];
        assert_eq!(e.offset, f1.len() as u64);
        assert!(matches!(e.kind, ErrorKind::Crc { expected, actual } if expected != actual));
        assert_eq!(run.skipped, f2.len() as u64);
        assert_eq!(run.frames[1].skipped_before, f2.len() as u64);
        assert_eq!(run.frames[1].offset, (f1.len() + f2.len()) as u64);
        assert_conserved(&run, stream.len());
    }
}

// REQ: OA-FEED-005
#[test]
fn bad_magic_is_reported_and_garbage_is_skipped_exactly() {
    let (f1, f2) = (msg(0, b"one"), msg(1, b"two"));
    let garbage = b"\x00\x01garbage-bytes!"; // 16 bytes, no magic
    let stream = cat(&[&f1, garbage, &f2]);
    let mut buf = big_buf();
    for k in 0..=stream.len() {
        let run = run_chunks(&mut buf, &[&stream[..k], &stream[k..]]);
        assert_eq!(run.frames.len(), 2, "split {k}");
        assert_eq!(
            run.errors,
            vec![FrameError {
                offset: f1.len() as u64,
                kind: ErrorKind::BadMagic {
                    found: *b"\x00\x01ga"
                }
            }]
        );
        assert_eq!(run.skipped, garbage.len() as u64);
        assert_eq!(run.frames[1].skipped_before, garbage.len() as u64);
    }
}

// REQ: OA-FEED-006
#[test]
fn oversize_length_is_refused_before_any_payload_arrives() {
    let (f1, f3) = (msg(0, b"one"), msg(2, b"three"));
    let mut bad = frame(1, 0, 1, 0, 0, b"");
    bad[32..36].copy_from_slice(&((MAX_PAYLOAD as u32) + 1).to_le_bytes());
    let header_only = &bad[..HEADER_LEN]; // no payload follows
    let stream = cat(&[&f1, header_only, &f3]);
    let mut buf = big_buf();
    let run = run_whole(&mut buf, &stream);
    assert_eq!(
        run.frames.iter().map(|f| f.seq).collect::<Vec<_>>(),
        vec![0, 2]
    );
    assert_eq!(
        run.errors,
        vec![FrameError {
            offset: f1.len() as u64,
            kind: ErrorKind::Oversize {
                len: MAX_PAYLOAD as u32 + 1
            }
        }]
    );
    assert_eq!(run.skipped, HEADER_LEN as u64);
    // A length of u32::MAX is refused the same way, not waited for.
    bad[32..36].copy_from_slice(&u32::MAX.to_le_bytes());
    let run = run_whole(&mut buf, &cat(&[&bad[..HEADER_LEN], &f3]));
    assert!(matches!(
        run.errors[0].kind,
        ErrorKind::Oversize { len: u32::MAX }
    ));
    assert_eq!(run.frames.len(), 1);
}

// REQ: OA-FEED-007
#[test]
fn unknown_kind_is_refused_and_skipped() {
    let (f1, bad, f3) = (
        msg(0, b"one"),
        frame(9, 0, 1, 0, 0, b"zzz"),
        msg(2, b"three"),
    );
    let zero = frame(0, 0, 1, 0, 0, b"");
    for (b, kind) in [(&bad, 9u8), (&zero, 0u8)] {
        let stream = cat(&[&f1, b, &f3]);
        let mut buf = big_buf();
        let run = run_whole(&mut buf, &stream);
        assert_eq!(
            run.errors,
            vec![FrameError {
                offset: f1.len() as u64,
                kind: ErrorKind::UnknownKind { kind }
            }]
        );
        assert_eq!(run.frames.len(), 2);
        assert_eq!(run.skipped, b.len() as u64);
        assert_conserved(&run, stream.len());
    }
}

// REQ: OA-FEED-008
#[test]
fn truncated_tail_is_its_own_error_with_skipped_count() {
    let (f1, f2, f3) = (msg(0, b"one"), msg(1, b"two"), msg(2, b"three, cut short"));
    for cut in [1usize, 3, 4, 20, HEADER_LEN, HEADER_LEN + 4, f3.len() - 1] {
        let stream = cat(&[&f1, &f2, &f3[..cut]]);
        let mut buf = big_buf();
        let run = run_whole(&mut buf, &stream);
        assert_eq!(run.frames.len(), 2);
        assert_eq!(
            run.errors,
            vec![FrameError {
                offset: (f1.len() + f2.len()) as u64,
                kind: ErrorKind::Truncated { have: cut as u32 }
            }],
            "cut {cut}"
        );
        assert_eq!(run.skipped, cut as u64);
        assert_conserved(&run, stream.len());
    }
    // Leftover bytes while hunting for a magic are counted but not reported a second time.
    let stream = cat(&[&f1, b"junkjunk", b"OA"]);
    let mut buf = big_buf();
    let run = run_whole(&mut buf, &stream);
    assert_eq!(run.errors.len(), 1);
    assert!(matches!(run.errors[0].kind, ErrorKind::BadMagic { .. }));
    assert_eq!(run.skipped, 10);
}

// REQ: OA-FEED-009
#[test]
fn no_panic_on_byte_flips_truncations_and_noise_and_every_byte_is_accounted_for() {
    let stream = cat(&[
        &msg(0, b"alpha"),
        &frame(3, 0, 1, 5, 0, b"state=connected\n"),
        &msg(2, &[b'q'; 300]),
        &frame(2, 0, 3, 5, 0, b"reason=x\nsize=1\njms_id=ID:2\n"),
        &msg(4, b""),
    ]);
    let mut buf = big_buf();
    for i in 0..stream.len() {
        for mask in [0xffu8, 0x01, 0x80] {
            let mut s = stream.clone();
            s[i] ^= mask;
            let run = run_whole(&mut buf, &s);
            assert_conserved(&run, s.len());
            assert!(run.frames.len() <= 5);
        }
        let run = run_whole(&mut buf, &stream[..i]);
        assert_conserved(&run, i);
        // A one-byte insertion and a one-byte deletion.
        let mut s = stream.clone();
        s.insert(i, 0xa5);
        assert_conserved(&run_whole(&mut buf, &s), s.len());
        s.remove(i);
        s.remove(i);
        assert_conserved(&run_whole(&mut buf, &s), s.len());
    }
    // Noise, with magics sprinkled in so the header paths are exercised.
    let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut noise = Vec::new();
    for _ in 0..200_000 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        noise.push((x >> 24) as u8);
        if x.wrapping_rem(61) == 0 {
            noise.extend_from_slice(&MAGIC);
        }
    }
    for size in [1usize, 97, 65_536] {
        let chunks: Vec<&[u8]> = noise.chunks(size).collect();
        assert_conserved(&run_chunks(&mut buf, &chunks), noise.len());
    }
}

// REQ: OA-FEED-010
#[test]
fn reader_memory_is_fixed_and_a_maximum_frame_fits() {
    let mut small = vec![0u8; MAX_FRAME_LEN - 1];
    assert!(FrameReader::new(&mut small).is_err());
    let big = vec![b'm'; MAX_PAYLOAD];
    let v1_max = HEADER_LEN + MAX_PAYLOAD + TRAILER_LEN;
    let mut v = vec![0u8; v1_max];
    assert_eq!(encode_frame(&mut v, 1, 0, 1, 0, 0, &big), Some(v1_max));
    assert!(encode_frame(
        &mut vec![0u8; MAX_FRAME_LEN + 8],
        1,
        0,
        1,
        0,
        0,
        &vec![0u8; MAX_PAYLOAD + 1]
    )
    .is_none());
    assert!(encode_frame(&mut [0u8; 10], 1, 0, 1, 0, 0, b"x").is_none());
    let stream = cat(&[&msg(0, b"a"), &v, &msg(2, b"c")]);
    // The buffer is exactly one maximum frame; the stream is pushed in 1 MiB pieces.
    let mut buf = big_buf();
    let chunks: Vec<&[u8]> = stream.chunks(1 << 20).collect();
    let run = run_chunks(&mut buf, &chunks);
    assert!(run.errors.is_empty());
    assert_eq!(
        run.frames
            .iter()
            .map(|f| f.payload.len())
            .collect::<Vec<_>>(),
        vec![1, MAX_PAYLOAD, 1]
    );
    // push takes only what fits.
    let mut r = FrameReader::new(&mut buf).unwrap();
    let taken = r.push(&vec![b'z'; MAX_FRAME_LEN + 100]);
    assert_eq!(taken, MAX_FRAME_LEN);
}

// REQ: OA-FEED-021
#[test]
fn every_error_names_its_byte_offset() {
    let cases = [
        (ErrorKind::BadMagic { found: *b"abcd" }, "bad magic"),
        (ErrorKind::UnknownKind { kind: 9 }, "unknown frame kind 9"),
        (ErrorKind::Oversize { len: 5_000_000 }, "5000000"),
        (
            ErrorKind::Crc {
                expected: 1,
                actual: 2,
            },
            "crc32 mismatch",
        ),
        (ErrorKind::Truncated { have: 7 }, "7 bytes"),
    ];
    for (kind, text) in cases {
        let s = FrameError { offset: 4242, kind }.to_string();
        assert!(s.contains("byte offset 4242"), "{s}");
        assert!(s.contains(text), "{s}");
    }
}

// ---------------------------------------------------------------- health

// REQ: OA-FEED-011
#[test]
fn health_counts_sizes_and_kinds() {
    let mut h = FeedHealth::new();
    let t = 1000 * SEC;
    for (i, len) in [0usize, 1, 100, 4096].into_iter().enumerate() {
        let p = vec![0u8; len];
        let flags = if i == 2 { FLAG_REDELIVERED } else { 0 };
        observe_frame(&mut h, &fr(Kind::Message, flags, i as u64, t, 0, &p), t);
    }
    observe_frame(&mut h, &fr(Kind::Reject, 0, 4, t, 0, b"reason=x\n"), t);
    observe_frame(
        &mut h,
        &fr(Kind::Status, 0, 5, t, 0, b"state=connected\n"),
        t,
    );
    assert_eq!(
        (h.frames, h.messages, h.rejects, h.redelivered),
        (6, 4, 1, 1)
    );
    assert_eq!((h.payload_bytes, h.size_min, h.size_max), (4197, 0, 4096));
    assert_eq!(
        (
            h.size_hist[0],
            h.size_hist[1],
            h.size_hist[7],
            h.size_hist[13]
        ),
        (1, 1, 1, 1)
    );
    assert_eq!(h.size_hist.iter().sum::<u64>(), 4);
    // The largest legal payload lands in the last bin.
    let big = vec![0u8; MAX_PAYLOAD];
    observe_frame(&mut h, &fr(Kind::Message, 0, 6, t, 0, &big), t);
    assert_eq!(h.size_hist[23], 1);
    let s = format_summary(&FeedHealth::new(), 0);
    assert!(s.contains("msgs=0") && s.contains("size_min=0"), "{s}");
}

// REQ: OA-FEED-012
#[test]
fn rates_over_one_and_sixty_seconds_with_injected_times() {
    let mut h = FeedHealth::new();
    let p = [0u8; 10];
    let mut seq = 0u64;
    let mut put = |h: &mut FeedHealth, t: i64| {
        observe_frame(h, &fr(Kind::Message, 0, seq, t, 0, &p), t);
        seq += 1;
    };
    for i in 0..10 {
        put(&mut h, 100 * SEC + i * 90_000_000);
    }
    for i in 0..20 {
        put(&mut h, 101 * SEC + i * 40_000_000);
    }
    // Inside the first second nothing is complete yet.
    assert_eq!(rates_milli(&h, 100 * SEC + 500_000_000), (0, 0));
    // At 102.5 s: last complete second (101) had 20; the window is 2 s long: 30 / 2.
    assert_eq!(rates_milli(&h, 102 * SEC + 500_000_000), (20_000, 15_000));
    // One message per second for a long time: both rates settle at exactly 1.000.
    let mut h = FeedHealth::new();
    for s in 0..200 {
        put(&mut h, (1000 + s) * SEC + 250_000_000);
    }
    assert_eq!(rates_milli(&h, 1200 * SEC + 500_000_000), (1000, 1000));
    // The window slides: messages older than 60 s drop out, and a stale ring reads as empty.
    assert_eq!(rates_milli(&h, 1230 * SEC), (0, 500));
    assert_eq!(rates_milli(&h, 5000 * SEC), (0, 0));
    assert!(format_summary(&h, 1200 * SEC + 500_000_000).contains("rate_1s=1.000 rate_60s=1.000"));
}

// REQ: OA-FEED-013
#[test]
fn sequence_gaps_duplicates_and_regressions() {
    let mut h = FeedHealth::new();
    for s in [0u64, 1, 2, 5, 5, 6, 3, 4, 5] {
        observe_frame(&mut h, &fr(Kind::Message, 0, s, SEC, 0, b"x"), SEC);
    }
    // 3 and 4 came late inside the gap, so nothing is missing; the last 5 repeats a frame
    // already seen inside the gap's range, so it is a duplicate, not a restart (OA-FEED-174).
    assert_eq!(
        (h.gaps, h.missing, h.duplicates, h.regressions, h.reordered),
        (1, 0, 2, 0, 2)
    );
    // A feed may start anywhere; the first frame is never a gap.
    let mut h = FeedHealth::new();
    observe_frame(&mut h, &fr(Kind::Message, 0, 9000, SEC, 0, b"x"), SEC);
    observe_frame(
        &mut h,
        &fr(Kind::Status, 0, 9001, SEC, 0, b"state=connected\n"),
        SEC,
    );
    assert_eq!(
        (h.gaps, h.missing, h.duplicates, h.regressions),
        (0, 0, 0, 0)
    );
    // A bridge restart (seq back to 0) is a regression, then counting resumes.
    observe_frame(&mut h, &fr(Kind::Message, 0, 0, SEC, 0, b"x"), SEC);
    observe_frame(&mut h, &fr(Kind::Message, 0, 1, SEC, 0, b"x"), SEC);
    assert_eq!((h.gaps, h.duplicates, h.regressions), (0, 0, 1));
}

// REQ: OA-FEED-014
#[test]
fn latency_histogram_and_clock_skew() {
    let mut h = FeedHealth::new();
    let recv_ms = 1_700_000_000_000i64;
    let recv_ns = recv_ms * 1_000_000 + 400_000; // 0.4 ms into the millisecond
    for (i, lat) in [0i64, 3, 15, 20_000, -5, -1].into_iter().enumerate() {
        let f = fr(Kind::Message, 0, i as u64, recv_ns, recv_ms - lat, b"x");
        observe_frame(&mut h, &f, recv_ns);
    }
    observe_frame(&mut h, &fr(Kind::Message, 0, 6, recv_ns, 0, b"x"), recv_ns);
    assert_eq!(h.skew, 2);
    assert_eq!(h.no_timestamp, 1);
    assert_eq!(
        (h.lat_hist[0], h.lat_hist[1], h.lat_hist[lat_bin(20_000)]),
        (2, 1, 1)
    );
    assert_eq!(h.lat_hist.iter().sum::<u64>(), 4);
    assert_eq!(latency_quantile_ms(&h, 1, 2), Some(10)); // 2nd of 4 samples is in [0,10)
    assert_eq!(latency_quantile_ms(&h, 19, 20), Some(21_000)); // top sample is in [20 s,21 s)
    assert_eq!(latency_quantile_ms(&FeedHealth::new(), 1, 2), None);
    let s = format_summary(&h, recv_ns);
    assert!(
        s.contains("skew=2")
            && s.contains("lat_p50_ms=10 ")
            && s.contains("lat_p95_ms=20000 lat_max_ms=20000 "), // clamped to the maximum
        "{s}"
    );
}

// REQ: OA-FEED-015
#[test]
fn status_frames_drive_connection_state_and_reconnects() {
    let mut h = FeedHealth::new();
    assert_eq!(
        format_summary(&h, 0).split(' ').next(),
        Some("state=unknown")
    );
    let mut seq = 0;
    let mut st = |h: &mut FeedHealth, p: &[u8]| {
        observe_frame(h, &fr(Kind::Status, 0, seq, SEC, 0, p), SEC);
        seq += 1;
    };
    st(&mut h, b"state=connected\n");
    assert_eq!((h.state, h.reconnects), (STATE_CONNECTED, 0));
    st(&mut h, b"state=disconnected\n");
    assert_eq!(h.state, STATE_DISCONNECTED);
    st(&mut h, b"state=reconnect_attempt\ndelay_ms=4321\n");
    assert_eq!(
        (h.state, h.reconnects, h.last_reconnect_delay_ms),
        (STATE_DISCONNECTED, 1, 4321)
    );
    st(&mut h, b"state=connected\n");
    assert_eq!((h.state, h.reconnects, h.messages), (STATE_CONNECTED, 1, 0));
    st(&mut h, b"nonsense\n\xff\xfe\n");
    assert_eq!(h.state, STATE_CONNECTED);
    assert!(format_summary(&h, SEC).contains("reconnects=1"));
}

// REQ: OA-FEED-016
#[test]
fn time_since_last_frame_comes_from_the_injected_clock() {
    let mut h = FeedHealth::new();
    assert!(format_summary(&h, 99 * SEC).contains("since_last_ms=0"));
    observe_frame(
        &mut h,
        &fr(Kind::Message, 0, 0, 100 * SEC, 0, b"x"),
        100 * SEC,
    );
    assert!(format_summary(&h, 100 * SEC + 1_500_000_000).contains("since_last_ms=1500"));
    assert!(format_summary(&h, 160 * SEC).contains("since_last_ms=60000"));
    // A clock that reads earlier than the last frame is clamped, not negative.
    assert!(format_summary(&h, 90 * SEC).contains("since_last_ms=0"));
}

// REQ: OA-FEED-017
#[test]
fn hot_structs_keep_their_budgeted_sizes() {
    use core::mem::size_of;
    assert_eq!(size_of::<Frame<'static>>(), 88);
    assert_eq!(size_of::<crate::health::Slot>(), 16);
    assert_eq!(size_of::<FeedHealth>(), 2440);
    // The size budget has no hidden padding: fields sum to the struct size.
    assert_eq!(24 * 8 + 2 * 4 + 8 + 24 * 8 + 133 * 8 + 61 * 16, 2440);
}

// REQ: OA-FEED-024
#[test]
fn latency_bins_have_the_documented_edges_and_an_overflow_bin() {
    use crate::health::{LAT_BINS, LAT_OVERFLOW_MS};
    let cases = [
        (0i64, 0usize),
        (9, 0),
        (10, 1),
        (99, 9),
        (100, 10),
        (199, 10),
        (999, 18),
        (1000, 19),
        (59_999, 77),
        (60_000, 78),
        (599_999, 131),
        (600_000, 132),
        (i64::MAX, 132),
    ];
    for (ms, bin) in cases {
        assert_eq!(lat_bin(ms), bin, "{ms} ms");
    }
    assert_eq!(LAT_BINS, 133);
    assert_eq!(LAT_OVERFLOW_MS, 600_000);
    // Every bin's upper edge is where the next bin starts, and the last bin is open.
    for bin in 0..LAT_BINS - 1 {
        let up = lat_bin_upper_ms(bin).unwrap();
        assert_eq!(lat_bin(up - 1), bin);
        assert_eq!(lat_bin(up), bin + 1);
    }
    assert_eq!(lat_bin_upper_ms(LAT_BINS - 1), None);
    // A sample in the overflow bin prints as ">=600000" for p50, p95 and max.
    let mut h = FeedHealth::new();
    let recv_ms = 1_700_000_000_000i64;
    let ns = recv_ms * 1_000_000;
    observe_frame(
        &mut h,
        &fr(Kind::Message, 0, 0, ns, recv_ms - 700_000, b"x"),
        ns,
    );
    let s = format_summary(&h, ns);
    assert!(
        s.contains("lat_p50_ms=>=600000 lat_p95_ms=>=600000 lat_max_ms=>=600000 "),
        "{s}"
    );
}

// REQ: OA-FEED-025
#[test]
fn latencies_of_seven_to_ten_seconds_are_resolved() {
    // The shape measured on the live TBFM feed: 7.3 s minimum, 8.7 s median, 10.5 s maximum.
    let mut h = FeedHealth::new();
    let recv_ms = 1_700_000_000_000i64;
    let ns = recv_ms * 1_000_000;
    let lats = [7_276i64, 7_900, 8_100, 8_664, 8_700, 9_500, 10_000, 10_500];
    for (i, lat) in lats.into_iter().enumerate() {
        observe_frame(
            &mut h,
            &fr(Kind::Message, 0, i as u64, ns, recv_ms - lat, b"x"),
            ns,
        );
    }
    assert_eq!(h.lat_hist[lat_bin(7_276)], 2); // [7 s,8 s): 7276, 7900
    assert_eq!(h.lat_hist[lat_bin(8_664)], 3); // [8 s,9 s)
    assert_eq!(h.lat_max_ms, 10_500);
    assert_eq!(latency_quantile_ms(&h, 1, 2), Some(9_000)); // 4th of 8 is 8664 ms
    assert_eq!(latency_quantile_ms(&h, 19, 20), Some(11_000));
    let s = format_summary(&h, ns);
    assert!(
        s.contains("lat_p50_ms=9000 lat_p95_ms=10500 lat_max_ms=10500 "),
        "{s}"
    );
}

// ---------------------------------------------------------------- replay and recording

// REQ: OA-FEED-018
#[test]
fn replay_is_deterministic_and_matches_the_fixture() {
    let stream = fixture();
    let dir = tmpdir("replay");
    let (a, b) = stream.split_at(stream.len() / 2);
    // Rotation happens on frame boundaries, so split at one.
    let mut cut = 0;
    let mut off = 0;
    while off < stream.len() / 2 {
        let len = u32::from_le_bytes(stream[off + 32..off + 36].try_into().unwrap()) as usize;
        off += HEADER_LEN + len + TRAILER_LEN;
        cut = off;
    }
    let _ = (a, b);
    std::fs::write(dir.join("a.oaf"), &stream[..cut]).unwrap();
    std::fs::write(dir.join("b.oaf"), &stream[cut..]).unwrap();
    let files = [dir.join("a.oaf"), dir.join("b.oaf")];
    let one = replay_files(&files).unwrap();
    let two = replay_files(&files).unwrap();
    let s1 = format_summary(&one.health, one.health.last_recv_ns);
    let s2 = format_summary(&two.health, two.health.last_recv_ns);
    assert_eq!(s1, s2);
    assert_eq!(one.health, two.health);
    assert!(one.errors.is_empty());
    assert_eq!(one.bytes, stream.len() as u64);
    // Same as reading the stream as one source.
    let whole = replay_sources(vec![&stream[..]]).unwrap();
    assert_eq!(whole.health, one.health);
    // Known content of the fixture.
    let h = &one.health;
    assert_eq!(
        (h.messages, h.rejects, h.reconnects, h.state),
        (150, 1, 1, STATE_CONNECTED)
    );
    assert_eq!(
        (h.gaps, h.missing, h.duplicates, h.regressions),
        (1, 3, 0, 0)
    );
    assert_eq!(h.redelivered, 9);
    assert!(s1.starts_with("state=connected msgs=150 "), "{s1}");
    std::fs::remove_dir_all(&dir).unwrap();
}

// REQ: OA-FEED-019
#[test]
fn recorder_rotates_caps_and_never_deletes() {
    let dir = tmpdir("record");
    let f = msg(0, &[b'r'; 60]); // 100 bytes per frame
    assert_eq!(f.len(), 100);
    // A pre-existing file counts toward the cap and is left alone.
    std::fs::write(dir.join("old.oaf"), [0u8; 150]).unwrap();
    let mut rec = Recorder::open(&dir, "scds-test", 250, 150 + 700, Format::Raw).unwrap();
    let mut outcomes = Vec::new();
    for i in 0..10 {
        outcomes.push(
            rec.write_frame(&f, 1_700_000_000 + i / 3, 1_700_000_000 + i / 3)
                .unwrap(),
        );
    }
    assert_eq!(
        outcomes.iter().filter(|o| **o == Outcome::Written).count(),
        7
    );
    assert_eq!(outcomes[7], Outcome::StoppedNow);
    assert!(outcomes[8..].iter().all(|o| *o == Outcome::Stopped));
    assert_eq!(rec.total_bytes(), 850);
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert!(names.contains(&"old.oaf".to_string()));
    let ours: Vec<&String> = names
        .iter()
        .filter(|n| n.starts_with("scds-test-"))
        .collect();
    assert_eq!(ours.len() as u32, rec.files_opened());
    assert_eq!(ours.len(), 4); // 2 frames per file at 250 bytes, 7 frames
    assert!(ours
        .iter()
        .all(|n| n.ends_with(".oaf") && n.len() == "scds-test-20231114T221320Z-0001.oaf".len()));
    let mut sizes: Vec<u64> = ours
        .iter()
        .map(|n| std::fs::metadata(dir.join(n)).unwrap().len())
        .collect();
    sizes.sort();
    assert_eq!(sizes, vec![100, 200, 200, 200]);
    assert!(
        sizes.iter().all(|s| s % 100 == 0),
        "files hold whole frames"
    );
    assert_eq!(std::fs::metadata(dir.join("old.oaf")).unwrap().len(), 150);
    std::fs::remove_dir_all(&dir).unwrap();
}

// REQ: OA-FEED-020
#[test]
fn utc_names_are_correct_calendar_times() {
    assert_eq!(utc_stamp(0).as_deref(), Some("19700101T000000Z"));
    assert_eq!(utc_stamp(-1).as_deref(), Some("19691231T235959Z"));
    assert_eq!(
        utc_stamp(1_700_000_000).as_deref(),
        Some("20231114T221320Z")
    );
    assert_eq!(utc_stamp(951_782_400).as_deref(), Some("20000229T000000Z")); // leap day
    assert_eq!(
        utc_stamp(4_102_444_799).as_deref(),
        Some("20991231T235959Z")
    );
    assert_eq!(
        crate::record::utc_iso(1_700_000_000).as_deref(),
        Some("2023-11-14T22:13:20Z")
    );
}

// REQ: OA-FEED-180
#[test]
fn utc_names_refuse_times_outside_years_1_to_9999() {
    use crate::record::{UTC_MAX_SECS, UTC_MIN_SECS};
    assert_eq!(utc_stamp(UTC_MIN_SECS).as_deref(), Some("00010101T000000Z"));
    assert_eq!(utc_stamp(UTC_MAX_SECS).as_deref(), Some("99991231T235959Z"));
    assert_eq!(utc_stamp(UTC_MIN_SECS - 1), None);
    assert_eq!(utc_stamp(UTC_MAX_SECS + 1), None);
    assert_eq!(utc_stamp(i64::MIN), None);
    assert_eq!(utc_stamp(i64::MAX), None);
    assert_eq!(crate::record::utc_iso(i64::MAX), None);
    assert_eq!(
        utc_stamp(1_078_099_199).as_deref(),
        Some("20040229T235959Z")
    );
    assert_eq!(
        utc_stamp(-2_203_891_200).as_deref(),
        Some("19000301T000000Z")
    );
}

// ---------------------------------------------------------------- command line

// REQ: OA-FEED-022, OA-FEED-026
#[test]
fn command_line_parsing() {
    let a = |s: &str| -> Vec<String> { s.split_whitespace().map(String::from).collect() };
    match cli::parse(&a("--env /x/scds-tbfm.env --duration 60 --record /r")).unwrap() {
        Command::Live(l) => {
            assert_eq!(l.feed, "scds-tbfm");
            assert_eq!(l.duration_secs, Some(60));
            assert_eq!(l.record, Some(PathBuf::from("/r")));
            assert_eq!((l.max_bytes, l.summary_secs), (2 * 1024 * 1024 * 1024, 10));
            assert!(matches!(l.source, Source::Bridge { .. }));
        }
        c => panic!("{c:?}"),
    }
    match cli::parse(&a(
        "--stdin --feed t --max-bytes 5 --rotate-bytes 3 --summary-every 2",
    ))
    .unwrap()
    {
        Command::Live(l) => assert_eq!(
            (l.feed.as_str(), l.max_bytes, l.rotate_bytes, l.summary_secs),
            ("t", 5, 3, 2)
        ),
        c => panic!("{c:?}"),
    }
    assert!(
        matches!(cli::parse(&a("replay a.oaf b.oaf.gz")).unwrap(), Command::Replay(v) if v.len() == 2)
    );
    // Compressed recording is the default; `--record-raw` asks for plain files.
    match cli::parse(&a("--stdin --record /r")).unwrap() {
        Command::Live(l) => {
            assert_eq!((l.record, l.record_raw), (Some(PathBuf::from("/r")), false))
        }
        c => panic!("{c:?}"),
    }
    match cli::parse(&a("--stdin --record-raw /r")).unwrap() {
        Command::Live(l) => assert_eq!((l.record, l.record_raw), (Some(PathBuf::from("/r")), true)),
        c => panic!("{c:?}"),
    }
    assert!(matches!(cli::parse(&a("--help")).unwrap(), Command::Help));
    for bad in [
        "",
        "--env",
        "--env x --stdin",
        "--stdin --jar j",
        "replay",
        "--env x --duration soon",
        "--bogus",
        "--env x --summary-every 0",
        "--stdin --record /a --record-raw /b",
        "--stdin --record-raw /a --record /b",
        "--stdin --record-raw",
    ] {
        assert!(cli::parse(&a(bad)).is_err(), "{bad:?} should be refused");
    }
}

// REQ: OA-FEED-178
#[test]
fn durations_over_the_operational_limits_are_refused() {
    let a = |s: &str| -> Vec<String> { s.split_whitespace().map(String::from).collect() };
    for (flag, limit) in [
        ("--duration", cli::MAX_DURATION_SECS),
        ("--summary-every", cli::MAX_SUMMARY_SECS),
    ] {
        // At the limit it parses; one above it, and u64::MAX, are refused, with no panic.
        let at = cli::parse(&a(&format!("--stdin {flag} {limit}")));
        assert!(matches!(at, Ok(Command::Live(_))), "{flag} {limit}: {at:?}");
        for over in [limit + 1, u64::MAX - 1, u64::MAX] {
            let r = cli::parse(&a(&format!("--stdin {flag} {over}")));
            assert!(r.is_err(), "{flag} {over} should be refused, got {r:?}");
        }
    }
}

// ---------------------------------------------------------------- compressed recording

/// Reads a whole gzip file (or a part of one) with the oa-zip reader; the bytes delivered and
/// the summary.
fn gunzip_all(bytes: &[u8]) -> (Vec<u8>, oa_zip::GzipSummary) {
    let mut r = oa_zip::GzipReader::new(oa_zip::InflateLimits::output_only(1 << 32));
    let mut out = Vec::new();
    r.push(
        bytes,
        &mut |b: &[u8]| -> Result<(), std::convert::Infallible> {
            out.extend_from_slice(b);
            Ok(())
        },
    )
    .unwrap();
    (out, r.finish())
}

/// Frames with receive times `step_ms` apart starting at `t0_s`, 100-ish bytes and varied.
fn timed_frames(n: u64, t0_s: i64, step_ms: i64) -> Vec<(i64, Vec<u8>)> {
    (0..n)
        .map(|i| {
            let recv = t0_s * SEC + i as i64 * step_ms * 1_000_000;
            let body = format!("<msg n=\"{i}\" v=\"{}\"/>", i * 7919 % 10_007);
            (
                recv / SEC,
                frame(1, 0, i, recv, recv / 1_000_000 - 5, body.as_bytes()),
            )
        })
        .collect()
}

fn files_in(dir: &std::path::Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    v.sort();
    v
}

// REQ: OA-FEED-026
#[test]
fn compressed_recording_names_and_content() {
    let dir = tmpdir("gzname");
    let frames = timed_frames(300, 1_700_000_000, 20);
    let all: Vec<u8> = frames.iter().flat_map(|f| f.1.clone()).collect();
    let mut rec = Recorder::open(&dir, "scds-x", u64::MAX, u64::MAX, Format::Gzip).unwrap();
    for (secs, f) in &frames {
        assert_eq!(rec.write_frame(f, *secs, *secs).unwrap(), Outcome::Written);
    }
    rec.finish().unwrap();
    assert_eq!(
        files_in(&dir),
        vec!["scds-x-20231114T221320Z-0001.oaf.gz".to_string()]
    );
    let bytes = std::fs::read(dir.join("scds-x-20231114T221320Z-0001.oaf.gz")).unwrap();
    assert_eq!(&bytes[..3], &[0x1f, 0x8b, 8]);
    assert!(
        bytes.len() < all.len() / 2,
        "{} of {}",
        bytes.len(),
        all.len()
    );
    let (out, s) = gunzip_all(&bytes);
    assert!(out == all, "recorded frames are the input frames exactly");
    assert_eq!((s.members, s.truncated), (1, None));
    assert_eq!(rec.total_bytes(), bytes.len() as u64);
    assert_eq!(rec.files_opened(), 1);
    // A second recorder in the same second gets the next number, and name order is creation
    // order (OA-FEED-019); raw files keep `.oaf`.
    let mut again = Recorder::open(&dir, "scds-x", u64::MAX, u64::MAX, Format::Gzip).unwrap();
    again
        .write_frame(&frames[0].1, 1_700_000_000, 1_700_000_000)
        .unwrap();
    again.finish().unwrap();
    let mut raw = Recorder::open(&dir, "scds-x", u64::MAX, u64::MAX, Format::Raw).unwrap();
    raw.write_frame(&frames[0].1, 1_700_000_000, 1_700_000_000)
        .unwrap();
    raw.finish().unwrap();
    assert_eq!(
        files_in(&dir),
        vec![
            "scds-x-20231114T221320Z-0001.oaf".to_string(),
            "scds-x-20231114T221320Z-0001.oaf.gz".to_string(),
            "scds-x-20231114T221320Z-0002.oaf.gz".to_string(),
        ]
    );
    assert_eq!(
        std::fs::read(dir.join("scds-x-20231114T221320Z-0001.oaf")).unwrap(),
        frames[0].1
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

// REQ: OA-FEED-027
#[test]
fn compressed_recording_is_flushed_every_second_and_on_exit() {
    let dir = tmpdir("gzflush");
    // 10 frames a second for 5 seconds.
    let frames = timed_frames(50, 1_700_000_000, 100);
    let mut rec = Recorder::open(&dir, "f", u64::MAX, u64::MAX, Format::Gzip).unwrap();
    let path = |dir: &std::path::Path| dir.join(files_in(dir).remove(0));
    let mut written: Vec<u8> = Vec::new();
    let mut last_flush_idx = 0usize;
    for (i, (secs, f)) in frames.iter().enumerate() {
        rec.write_frame(f, *secs, *secs).unwrap();
        written.extend(f);
        // A crash now leaves this file: read it as it lies on disk, without finishing.
        let on_disk = std::fs::read(path(&dir)).unwrap();
        let (out, s) = gunzip_all(&on_disk);
        assert!(written.starts_with(&out), "frame {i}: not a prefix");
        // Everything up to the last frame at least one second after the previous flush is there.
        let flush_due = i > 0 && frames[i].0 > frames[last_flush_idx].0;
        if flush_due {
            assert_eq!(out.len(), written.len(), "frame {i}: a flush was due");
            last_flush_idx = i;
        }
        // At no point is more than about a second (ten frames) missing.
        let missing = written.len() - out.len();
        let per_frame = frames[i].1.len();
        assert!(
            missing <= 11 * per_frame,
            "frame {i}: {missing} bytes missing"
        );
        assert!(
            s.truncated.is_some(),
            "a file still being written is open-ended"
        );
    }
    // The feed goes quiet: an idle flush a second later saves the tail.
    let before = std::fs::read(path(&dir)).unwrap();
    let (out, _) = gunzip_all(&before);
    rec.idle_flush(1_700_000_004).unwrap(); // not yet a second past the last flush
    rec.idle_flush(1_700_000_005 + 1).unwrap();
    let (out2, _) = gunzip_all(&std::fs::read(path(&dir)).unwrap());
    assert!(out2.len() >= out.len());
    assert_eq!(
        out2.len(),
        written.len(),
        "idle flush leaves nothing behind"
    );
    // Exit finishes the member: complete, trailer verified.
    rec.finish().unwrap();
    let (out3, s) = gunzip_all(&std::fs::read(path(&dir)).unwrap());
    assert!(out3 == written);
    assert_eq!((s.members, s.truncated), (1, None));
    // Finishing twice, or flushing with nothing open, is harmless.
    rec.finish().unwrap();
    rec.idle_flush(1_800_000_000).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
}

// REQ: OA-FEED-028
#[test]
fn compressed_rotation_and_cap_count_bytes_on_disk() {
    let dir = tmpdir("gzrot");
    // Frames the compressor can barely shrink, so compressed size tracks frame size.
    let mut r = 12345u64;
    let mut noise = |n: usize| -> Vec<u8> {
        (0..n)
            .map(|_| {
                r ^= r << 13;
                r ^= r >> 7;
                r ^= r << 17;
                (r >> 24) as u8
            })
            .collect()
    };
    let frames: Vec<(i64, Vec<u8>)> = (0..60u64)
        .map(|i| {
            let recv = 1_700_000_000 * SEC + i as i64 * SEC;
            (recv / SEC, frame(1, 0, i, recv, 0, &noise(400)))
        })
        .collect();
    // Pre-existing files of both kinds count toward the cap and are left alone.
    std::fs::write(dir.join("old.oaf"), [0u8; 1000]).unwrap();
    std::fs::write(dir.join("old2.oaf.gz"), [0u8; 500]).unwrap();
    std::fs::write(dir.join("notes.txt"), [0u8; 9999]).unwrap();
    let rotate = 2000u64;
    let cap = 1500 + 12_000;
    let mut rec = Recorder::open(&dir, "r", rotate, cap, Format::Gzip).unwrap();
    assert_eq!(rec.total_bytes(), 1500);
    let mut stopped_at = None;
    for (i, (secs, f)) in frames.iter().enumerate() {
        match rec.write_frame(f, *secs, *secs).unwrap() {
            Outcome::Written => {}
            Outcome::StoppedNow => {
                assert!(stopped_at.is_none());
                stopped_at = Some(i);
            }
            Outcome::Stopped => assert!(stopped_at.is_some()),
            Outcome::Failed => panic!("no write fails here"),
        }
    }
    let stopped_at = stopped_at.expect("the cap was reached");
    rec.finish().unwrap();
    let names = files_in(&dir);
    assert!(names.contains(&"old.oaf".to_string()) && names.contains(&"old2.oaf.gz".to_string()));
    assert!(names.contains(&"notes.txt".to_string()));
    let ours: Vec<&String> = names.iter().filter(|n| n.starts_with("r-")).collect();
    assert_eq!(ours.len() as u32, rec.files_opened());
    assert!(ours.len() >= 5, "{ours:?}");
    let mut sum = 1500u64;
    let mut got = Vec::new();
    for (k, n) in ours.iter().enumerate() {
        let b = std::fs::read(dir.join(n)).unwrap();
        sum += b.len() as u64;
        // Every file is finished and holds whole frames; all but the last passed the rotation size
        // by less than a flush interval's worth.
        let (out, s) = gunzip_all(&b);
        assert_eq!((s.members, s.truncated), (1, None), "{n}");
        let mut off = 0;
        while off < out.len() {
            let len = u32::from_le_bytes(out[off + 32..off + 36].try_into().unwrap()) as usize;
            off += HEADER_LEN + len + TRAILER_LEN;
        }
        assert_eq!(off, out.len(), "{n} holds whole frames");
        if k + 1 < ours.len() {
            assert!(
                b.len() as u64 >= rotate && (b.len() as u64) < rotate + 600,
                "{n}: {}",
                b.len()
            );
        }
        got.extend(out);
    }
    assert_eq!(rec.total_bytes(), sum, "the total is bytes on disk");
    assert!(
        sum >= cap && sum < cap + 1000,
        "stopped at about the cap: {sum} vs {cap}"
    );
    let want: Vec<u8> = frames[..stopped_at]
        .iter()
        .flat_map(|f| f.1.clone())
        .collect();
    assert!(got == want, "the frames before the stop are all there");
    assert_eq!(std::fs::metadata(dir.join("old.oaf")).unwrap().len(), 1000);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A reader that hands out one byte per call.
struct OneByte<'a>(&'a [u8]);
impl std::io::Read for OneByte<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self.0.split_first() {
            Some((b, rest)) if !buf.is_empty() => {
                buf[0] = *b;
                self.0 = rest;
                Ok(1)
            }
            _ => Ok(0),
        }
    }
}

/// Gzip of `data` as one finished member via the recorder's own path (a `Recorder` file).
fn record_gz(
    dir: &std::path::Path,
    feed: &str,
    rotate: u64,
    frames: &[u8],
    secs: &[i64],
) -> Vec<PathBuf> {
    let mut rec = Recorder::open(dir, feed, rotate, u64::MAX, Format::Gzip).unwrap();
    let mut off = 0;
    for &t in secs {
        let len = u32::from_le_bytes(frames[off + 32..off + 36].try_into().unwrap()) as usize;
        let n = HEADER_LEN + len + TRAILER_LEN;
        rec.write_frame(&frames[off..off + n], t, t).unwrap();
        off += n;
    }
    assert_eq!(off, frames.len());
    rec.finish().unwrap();
    // Creation order: a name taken in the same second gets `-2`, which sorts before the `.`
    // of the plain name, so compare the names without their extension.
    let mut names = files_in(dir);
    names.sort_by_key(|n| {
        n.trim_end_matches(".gz")
            .trim_end_matches(".oaf")
            .to_string()
    });
    names.into_iter().map(|n| dir.join(n)).collect()
}

fn frame_secs(stream: &[u8]) -> Vec<i64> {
    let mut v = Vec::new();
    let mut off = 0;
    while off < stream.len() {
        let len = u32::from_le_bytes(stream[off + 32..off + 36].try_into().unwrap()) as usize;
        let recv = i64::from_le_bytes(stream[off + 16..off + 24].try_into().unwrap());
        v.push(recv.div_euclid(SEC));
        off += HEADER_LEN + len + TRAILER_LEN;
    }
    v
}

// REQ: OA-FEED-029
#[test]
fn replay_chooses_by_magic_not_by_name() {
    let dir = tmpdir("magic");
    let stream = fixture();
    let secs = frame_secs(&stream);
    let gz_dir = dir.join("gz");
    let gz = record_gz(&gz_dir, "m", u64::MAX, &stream, &secs);
    assert_eq!(gz.len(), 1);
    let gz_bytes = std::fs::read(&gz[0]).unwrap();
    // Names swapped: raw frames called `.oaf.gz`, gzip called `.oaf` or with no extension.
    std::fs::write(dir.join("raw-named-gz.oaf.gz"), &stream).unwrap();
    std::fs::write(dir.join("gz-named-oaf.oaf"), &gz_bytes).unwrap();
    std::fs::write(dir.join("gz-no-extension"), &gz_bytes).unwrap();
    let base = replay_sources(vec![&stream[..]]).unwrap();
    for name in ["raw-named-gz.oaf.gz", "gz-named-oaf.oaf", "gz-no-extension"] {
        let r = replay_files(&[dir.join(name)]).unwrap();
        assert_eq!(r.health, base.health, "{name}");
        assert_eq!(r.bytes, stream.len() as u64, "{name}");
        assert!(r.errors.is_empty() && r.gzip.is_empty(), "{name}");
    }
    // The decision does not depend on how the source chunks its reads.
    let r = replay_sources(vec![OneByte(&gz_bytes)]).unwrap();
    assert_eq!((&r.health, r.gzip.len()), (&base.health, 0));
    let r = replay_sources(vec![OneByte(&stream)]).unwrap();
    assert_eq!(r.health, base.health);
    // Empty and one-byte sources are raw, not gzip.
    let r = replay_sources(vec![&b""[..], &b"\x1f"[..]]).unwrap();
    assert!(r.gzip.is_empty() && r.health.messages == 0);
    std::fs::remove_dir_all(&dir).unwrap();
}

// REQ: OA-FEED-030
#[test]
fn compressed_replay_summary_is_identical_to_raw() {
    let dir = tmpdir("gzrep");
    let stream = fixture();
    let secs = frame_secs(&stream);
    // Rotating small makes several members/files, replayed in order like the raw ones.
    let files = record_gz(&dir, "rep", 120, &stream, &secs);
    assert!(files.len() >= 3, "{files:?}");
    let gz = replay_files(&files).unwrap();
    let raw = replay_sources(vec![&stream[..]]).unwrap();
    assert_eq!(gz.health, raw.health);
    assert_eq!(gz.bytes, raw.bytes);
    assert_eq!(
        format_summary(&gz.health, gz.health.last_recv_ns),
        format_summary(&raw.health, raw.health.last_recv_ns)
    );
    assert!(gz.errors.is_empty() && gz.gzip.is_empty());
    assert_eq!(gz.files, files.len());
    std::fs::remove_dir_all(&dir).unwrap();
}

// REQ: OA-FEED-030
#[test]
fn crashed_and_damaged_compressed_files_replay_what_is_good() {
    let dir = tmpdir("gzcrash");
    let stream = fixture();
    let secs = frame_secs(&stream);
    // A crash mid-recording: copy the file while the recorder is open and not finished.
    let mut rec = Recorder::open(&dir, "c", u64::MAX, u64::MAX, Format::Gzip).unwrap();
    let mut off = 0;
    let mut lens = Vec::new();
    for &t in &secs {
        let len = u32::from_le_bytes(stream[off + 32..off + 36].try_into().unwrap()) as usize;
        let n = HEADER_LEN + len + TRAILER_LEN;
        rec.write_frame(&stream[off..off + n], t, t).unwrap();
        off += n;
        lens.push(off);
    }
    let live = std::fs::read(dir.join(files_in(&dir).remove(0))).unwrap();
    std::fs::write(dir.join("crashed.oaf.gz"), &live).unwrap();
    rec.finish().unwrap();
    let r = replay_files(&[dir.join("crashed.oaf.gz")]).unwrap();
    assert_eq!(r.gzip.len(), 1, "never a silent success");
    let GzNoteKind::Truncated(t) = &r.gzip[0].kind else {
        panic!("{:?}", r.gzip)
    };
    assert_eq!(t.stage, oa_zip::TruncStage::Data);
    assert_eq!(t.tail_bytes, r.bytes);
    assert!(t.flushed_bytes <= t.tail_bytes);
    // Every frame replayed is a whole frame of the original (the rest is a partial tail).
    let whole = lens
        .iter()
        .rev()
        .find(|&&l| l as u64 <= r.bytes)
        .copied()
        .unwrap();
    assert!(
        whole >= lens[lens.len() / 2],
        "most of the stream survives the crash"
    );
    let upto = replay_sources(vec![&stream[..whole]]).unwrap();
    assert_eq!(r.health.messages, upto.health.messages);
    // A file cut exactly at a flush replays whole frames with no frame error.
    let cut = replay_sources(vec![&stream[..t.flushed_bytes as usize]]).unwrap();
    assert!(cut.errors.is_empty() || t.flushed_bytes as usize != whole);

    // A flipped bit in the middle: the frames before it replay and the damage is reported.
    let full =
        std::fs::read(dir.join(files_in(&dir).iter().find(|n| n.starts_with("c-")).unwrap()))
            .unwrap();
    let mut bad = full.clone();
    let at = bad.len() / 2;
    bad[at] ^= 0x10;
    std::fs::write(dir.join("bad.oaf.gz"), &bad).unwrap();
    let r = replay_files(&[dir.join("bad.oaf.gz")]).unwrap();
    assert_eq!(r.gzip.len(), 1);
    assert!(
        matches!(r.gzip[0].kind, GzNoteKind::Damaged(_))
            || matches!(r.gzip[0].kind, GzNoteKind::Truncated(_)),
        "{:?}",
        r.gzip
    );
    assert!(r.health.messages > 0 && r.health.messages < 150);
    // A wrong CRC-32 is its own report, after all the data was replayed.
    let mut bad = full.clone();
    let n = bad.len();
    bad[n - 8] ^= 1;
    std::fs::write(dir.join("crc.oaf.gz"), &bad).unwrap();
    let r = replay_files(&[dir.join("crc.oaf.gz")]).unwrap();
    assert!(matches!(
        &r.gzip[0].kind,
        GzNoteKind::Damaged(oa_zip::GzipError::CrcMismatch { .. })
    ));
    assert_eq!(r.health.messages, 150);
    std::fs::remove_dir_all(&dir).unwrap();
}

// REQ: OA-FEED-033
#[test]
fn flush_and_file_name_follow_local_clock_not_frame_stamp() {
    let dir = tmpdir("localclock");
    let mut rec = Recorder::open(&dir, "f", u64::MAX, u64::MAX, Format::Gzip).unwrap();
    let local = 1_700_000_000i64;
    let far = local + 10_000_000;
    let frames = timed_frames(4, local, 1000);
    // The first frame carries a stamp far in the future; local time is sane.
    rec.write_frame(&frames[0].1, far, local).unwrap();
    for (i, (_, f)) in frames.iter().enumerate().skip(1) {
        rec.write_frame(f, local + i as i64, local + i as i64)
            .unwrap();
    }
    let names = files_in(&dir);
    assert!(names[0].contains(&utc_stamp(local).unwrap()), "{names:?}");
    let on_disk = std::fs::read(dir.join(&names[0])).unwrap();
    let (out, _) = gunzip_all(&on_disk);
    let all: Vec<u8> = frames.iter().flat_map(|(_, f)| f.clone()).collect();
    assert_eq!(out.len(), all.len(), "the last second's frame was flushed");
    assert_eq!(rec.implausible_stamps(), 1);
    rec.finish().unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
}

// REQ: OA-FEED-034
#[test]
fn sequence_gap_total_saturates() {
    let mut h = FeedHealth::new();
    let t = 1000 * SEC;
    // The second 0 is a restart (a regression); the jump after it adds to a nearly full count.
    for seq in [0, u64::MAX - 1, 0, u64::MAX] {
        observe_frame(&mut h, &fr(Kind::Message, 0, seq, t, 0, b"<a/>"), t);
    }
    assert_eq!(h.missing, u64::MAX);
}

// REQ: OA-FEED-050
#[test]
fn nonzero_reserved_bytes_are_refused_and_counted() {
    let good = msg(0, b"ok");
    let mut bad = msg(1, b"reserved set");
    bad[6] = 1;
    bad[7] = 0x80;
    // A matching CRC: only the reserved-field rule can refuse this frame.
    let n = bad.len();
    let crc = oa_zip::crc32(&bad[..n - TRAILER_LEN]);
    bad[n - TRAILER_LEN..].copy_from_slice(&crc.to_le_bytes());
    let tail = msg(2, b"after");
    let stream = cat(&[&good, &bad, &tail]);
    let mut buf = big_buf();
    let run = run_chunks(&mut buf, &[&stream]);
    assert_eq!(
        run.frames.iter().map(|f| f.seq).collect::<Vec<_>>(),
        vec![0, 2]
    );
    assert_eq!(run.errors.len(), 1);
    assert_eq!(run.errors[0].offset, good.len() as u64);
    assert_eq!(run.errors[0].kind, ErrorKind::Reserved { value: 0x8001 });
    assert!(run.errors[0].to_string().contains("reserved"));
    assert_eq!(run.skipped, bad.len() as u64);
    assert_conserved(&run, stream.len());
    // The encoder writes zero, so every frame it makes passes.
    assert!(msg(3, b"x")[6..8] == [0, 0]);
}

/// A sink that accepts bytes until its switch is thrown, then fails every write like a full disk.
struct Switched(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl std::io::Write for Switched {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        if self.0.load(std::sync::atomic::Ordering::SeqCst) {
            Err(std::io::Error::other("no space left on device"))
        } else {
            Ok(b.len())
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn switched_recorder(
    name: &str,
    format: Format,
) -> (Recorder, std::sync::Arc<std::sync::atomic::AtomicBool>) {
    let sw = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let sw2 = sw.clone();
    let opener: Opener = Box::new(move |_| Ok(Box::new(Switched(sw2.clone()))));
    let dir = tmpdir(name);
    let rec = Recorder::open_with(&dir, "f", u64::MAX, u64::MAX, format, opener).unwrap();
    (rec, sw)
}

fn fail_now(sw: &std::sync::atomic::AtomicBool) {
    sw.store(true, std::sync::atomic::Ordering::SeqCst);
}

// REQ: OA-FEED-051
#[test]
fn a_failed_write_is_a_persistent_failed_state_with_a_count() {
    let (mut rec, sw) = switched_recorder("failwrite", Format::Raw);
    let f = msg(0, b"<a/>");
    assert_eq!(rec.write_frame(&f, 1000, 1000).unwrap(), Outcome::Written);
    assert!(rec.failure().is_none());
    assert!(!format_recording("f", &rec).contains("FAILED"));
    fail_now(&sw);
    assert!(rec.write_frame(&f, 1001, 1001).is_err());
    assert_eq!(rec.failure(), Some("no space left on device"));
    // Later frames are counted as lost, not silently dropped; nothing more is tried.
    for _ in 0..3 {
        assert_eq!(rec.write_frame(&f, 1002, 1002).unwrap(), Outcome::Failed);
    }
    assert_eq!(rec.lost_frames(), 4);
    rec.idle_flush(2000).unwrap();
    rec.finish().unwrap();
    // The failure is still there after finish, and the summary says so.
    assert!(rec.failure().is_some());
    let line = format_recording("f", &rec);
    assert!(
        line.contains("FAILED lost_frames=4 first_error=\"no space left on device\""),
        "{line}"
    );
}

// REQ: OA-FEED-051
#[test]
fn a_failed_idle_flush_or_finish_also_fails_the_recording() {
    // The compressor holds the frame until a flush: the first write succeeds.
    let (mut rec, sw) = switched_recorder("failflush", Format::Gzip);
    let f = msg(0, b"<a/>");
    assert_eq!(rec.write_frame(&f, 1000, 1000).unwrap(), Outcome::Written);
    fail_now(&sw);
    assert!(rec.idle_flush(1005).is_err());
    assert!(rec.failure().is_some());
    assert_eq!(rec.write_frame(&f, 1006, 1006).unwrap(), Outcome::Failed);
    // The frame the compressor still held when the flush failed is lost too.
    assert_eq!(rec.lost_frames(), 2);
    // A failure only at the final finish counts as well.
    let (mut rec, sw) = switched_recorder("failfinish", Format::Gzip);
    assert_eq!(rec.write_frame(&f, 1000, 1000).unwrap(), Outcome::Written);
    fail_now(&sw);
    assert!(rec.finish().is_err());
    assert_eq!(rec.lost_frames(), 1);
    assert_eq!(rec.failure(), Some("no space left on device"));
    assert!(format_recording("f", &rec).contains("FAILED"));
    // The first error is the one kept.
    assert!(rec.write_frame(&f, 1001, 1001).is_ok());
    assert_eq!(rec.failure(), Some("no space left on device"));
}

// ---------------------------------------------------------------- pinned behaviour

// REQ: OA-FEED-150
#[test]
fn refused_frames_and_skipped_bytes_are_counted_and_summarised() {
    let kinds = [
        ErrorKind::BadMagic { found: *b"XXXX" },
        ErrorKind::UnknownKind { kind: 9 },
        ErrorKind::Oversize { len: u32::MAX },
        ErrorKind::Reserved { value: 1 },
        ErrorKind::Crc {
            expected: 1,
            actual: 2,
        },
        ErrorKind::Truncated { have: 5 },
    ];
    let mut h = FeedHealth::new();
    for (i, kind) in kinds.iter().enumerate() {
        observe_error(
            &mut h,
            &FrameError {
                offset: 0,
                kind: *kind,
            },
        );
        assert_eq!(h.frame_errors, i as u64 + 1);
        // An error is not a frame: nothing else moves.
        assert_eq!((h.frames, h.messages, h.rejects), (0, 0, 0));
    }
    assert_eq!(h.skipped_bytes, 0);
    // The skipped figure is the reader's running total: set, not added.
    observe_skipped(&mut h, 7);
    assert_eq!(h.skipped_bytes, 7);
    observe_skipped(&mut h, 12);
    assert_eq!(h.skipped_bytes, 12);
    assert_eq!(h.frame_errors, 6);
    let s = format_summary(&h, 0);
    assert!(s.contains(" errors=6 skipped=12 "), "{s}");
    let s = format_summary(&FeedHealth::new(), 0);
    assert!(s.contains(" errors=0 skipped=0 "), "{s}");
}

// REQ: OA-FEED-151
#[test]
fn every_latency_bin_edge_is_exact() {
    // (first value of the bin, bin); the value one below belongs to the bin before.
    let mut edges: Vec<(i64, usize)> = Vec::new();
    for b in 0..10 {
        edges.push((b * 10, b as usize));
    }
    for b in 0..9 {
        edges.push((100 + b * 100, 10 + b as usize));
    }
    for b in 0..59 {
        edges.push((1000 + b * 1000, 19 + b as usize));
    }
    for b in 0..54 {
        edges.push((60_000 + b * 10_000, 78 + b as usize));
    }
    edges.push((600_000, LAT_BINS - 1));
    assert_eq!(edges.len(), LAT_BINS);
    for (i, &(at, bin)) in edges.iter().enumerate() {
        assert_eq!(lat_bin(at), bin, "at {at}");
        assert_eq!(lat_bin(at + 1), bin, "at {at} + 1");
        if i > 0 {
            assert_eq!(lat_bin(at - 1), bin - 1, "at {at} - 1");
            assert_eq!(lat_bin_upper_ms(bin - 1), Some(at));
        }
    }
    assert_eq!(lat_bin(-5), 0);
    assert_eq!(lat_bin(i64::MAX), LAT_BINS - 1);
    assert_eq!(lat_bin_upper_ms(LAT_BINS - 1), None);
}

// REQ: OA-FEED-152
#[test]
fn a_repeat_of_the_last_seq_is_a_duplicate_and_the_expected_one_is_not_a_gap() {
    let mut h = FeedHealth::new();
    let t = 100 * SEC;
    for seq in [4u64, 5, 5, 6] {
        observe_frame(&mut h, &fr(Kind::Message, 0, seq, t, 0, b"x"), t);
    }
    assert_eq!(
        (h.gaps, h.missing, h.duplicates, h.regressions),
        (0, 0, 1, 0)
    );
    // The duplicate leaves the expectation alone.
    assert_eq!((h.seq_last, h.seq_next), (6, 7));
    // The first frame repeated is a duplicate too, not a regression.
    let mut h = FeedHealth::new();
    for seq in [0u64, 0, 1] {
        observe_frame(&mut h, &fr(Kind::Message, 0, seq, t, 0, b"x"), t);
    }
    assert_eq!((h.gaps, h.duplicates, h.regressions), (0, 1, 0));
    // One ahead of the expected is a gap of exactly one missing frame.
    observe_frame(&mut h, &fr(Kind::Message, 0, 3, t, 0, b"x"), t);
    assert_eq!((h.gaps, h.missing, h.seq_next), (1, 1, 4));
    // One below the last that fills the open gap is reordered, not a regression (OA-FEED-172).
    observe_frame(&mut h, &fr(Kind::Message, 0, 2, t, 0, b"x"), t);
    assert_eq!(
        (h.missing, h.reordered, h.regressions, h.seq_next),
        (0, 1, 0, 4)
    );
    // One below the last outside any open gap is a regression.
    observe_frame(&mut h, &fr(Kind::Message, 0, 1, t, 0, b"x"), t);
    assert_eq!((h.duplicates, h.regressions), (1, 1));
}

// REQ: OA-FEED-153
#[test]
fn latency_is_the_receive_time_minus_the_jms_time_in_whole_milliseconds() {
    let mut h = FeedHealth::new();
    // Receive time 1_000_000_250.9 ms, JMS stamp 1_000_000_000 ms: 250 ms.
    let recv = 1_000_000_250_900_000;
    observe_frame(
        &mut h,
        &fr(Kind::Message, 0, 0, recv, 1_000_000_000, b"x"),
        5 * SEC,
    );
    assert_eq!(h.lat_max_ms, 250);
    assert_eq!(h.lat_hist[lat_bin(250)], 1);
    assert_eq!(h.lat_hist.iter().sum::<u64>(), 1);
    assert_eq!((h.skew, h.no_timestamp), (0, 0));
    // The same stamp as the receive time: zero, not skew.
    observe_frame(
        &mut h,
        &fr(Kind::Message, 0, 1, 7_000_000, 7, b"x"),
        5 * SEC,
    );
    assert_eq!((h.skew, h.lat_hist[0]), (0, 1));
    assert_eq!(h.lat_max_ms, 250);
    // A stamp ahead of the receive time by one ms is skew.
    observe_frame(
        &mut h,
        &fr(Kind::Message, 0, 2, 7_000_000, 8, b"x"),
        5 * SEC,
    );
    assert_eq!(h.skew, 1);
}

// REQ: OA-FEED-154
#[test]
fn latency_quantile_needs_samples_and_a_denominator() {
    let mut h = FeedHealth::new();
    // No samples: nothing, whatever the denominator.
    assert_eq!(latency_quantile_ms(&h, 1, 2), None);
    assert_eq!(latency_quantile_ms(&h, 1, 0), None);
    h.lat_hist[lat_bin(250)] = 4;
    // Samples but a zero denominator: nothing, and no division by zero.
    assert_eq!(latency_quantile_ms(&h, 1, 0), None);
    assert_eq!(latency_quantile_ms(&h, 0, 0), None);
    // Samples and a denominator: the bin's upper edge.
    assert_eq!(latency_quantile_ms(&h, 1, 2), Some(300));
    assert_eq!(latency_quantile_ms(&h, 19, 20), Some(300));
}

// REQ: OA-FEED-155
#[test]
fn summary_text_for_a_disconnected_feed_and_for_no_latency_samples() {
    let mut h = FeedHealth::new();
    let s = format_summary(&h, 0);
    assert!(s.starts_with("state=unknown "), "{s}");
    observe_frame(
        &mut h,
        &fr(Kind::Status, 0, 0, SEC, 0, b"state=disconnected\n"),
        SEC,
    );
    let s = format_summary(&h, SEC);
    assert!(s.starts_with("state=disconnected "), "{s}");
    // No latency samples: all three quantities print a dash, never a number or the overflow.
    assert!(
        s.contains(" lat_p50_ms=- lat_p95_ms=- lat_max_ms=- "),
        "{s}"
    );
    // A message without a JMS stamp is still no sample.
    observe_frame(&mut h, &fr(Kind::Message, 0, 1, SEC, 0, b"x"), SEC);
    let s = format_summary(&h, SEC);
    assert!(
        s.contains(" lat_p50_ms=- lat_p95_ms=- lat_max_ms=- "),
        "{s}"
    );
    // A skewed sample is kept out of the histogram, so it is no sample either.
    observe_frame(&mut h, &fr(Kind::Message, 0, 2, SEC, 5_000, b"x"), SEC);
    let s = format_summary(&h, SEC);
    assert!(s.contains(" skew=1 "), "{s}");
    assert!(
        s.contains(" lat_p50_ms=- lat_p95_ms=- lat_max_ms=- "),
        "{s}"
    );
    // One real sample replaces the dashes.
    observe_frame(&mut h, &fr(Kind::Message, 0, 3, SEC, 1_000 - 25, b"x"), SEC);
    let s = format_summary(&h, SEC);
    assert!(
        s.contains(" lat_p50_ms=25 lat_p95_ms=25 lat_max_ms=25 "),
        "{s}"
    );
}

// REQ: OA-FEED-156
#[test]
fn push_takes_exactly_what_fits_and_compacts_when_room_is_short() {
    let mut buf = big_buf();
    let cap = buf.len();
    let mut r = FrameReader::new(&mut buf).unwrap();
    // Exactly full, then one byte over.
    let junk = vec![0u8; cap + 1];
    assert_eq!(r.push(&junk), cap);
    assert_eq!(r.push(b"x"), 0);

    // Consumed bytes are reclaimed: an emptied buffer takes a full buffer again.
    let a = msg(0, &[1u8; 100]);
    let mut r = FrameReader::new(&mut buf).unwrap();
    assert_eq!(r.push(&a), a.len());
    assert!(r.next().unwrap().is_ok());
    assert!(r.next().is_none());
    assert_eq!(r.push(&junk), cap);

    // Frame A is consumed, the first four bytes of a maximum frame C remain.
    let a = msg(0, &[1u8; 1000]);
    // The largest frame there is, an OAF2 frame with a maximum payload.
    let mut c = vec![0u8; cap];
    assert_eq!(
        encode_frame_mono(&mut c, 1, 0, 1, 0, 0, 7, &vec![2u8; MAX_PAYLOAD]),
        Some(cap)
    );
    let mut r = FrameReader::new(&mut buf).unwrap();
    assert_eq!(r.push(&cat(&[&a, &c[..4]])), a.len() + 4);
    assert!(r.next().unwrap().is_ok());
    assert!(r.next().is_none());
    assert_eq!(r.offset(), a.len() as u64);
    // The rest of C does not fit behind the consumed A, but fits once compacted: all of it is
    // taken, one byte more is not.
    let mut rest = c[4..].to_vec();
    rest.push(0);
    assert_eq!(r.push(&rest), cap - 4);
    let f = r.next().unwrap().unwrap();
    assert_eq!(
        (f.seq, f.offset, f.payload.len()),
        (1, a.len() as u64, MAX_PAYLOAD)
    );
    assert!(f.payload.iter().all(|&b| b == 2));

    // Room that is exactly enough needs no compaction and loses nothing.
    let mut r = FrameReader::new(&mut buf).unwrap();
    let b = msg(2, &[3u8; 500]);
    assert_eq!(r.push(&cat(&[&a, &b[..10]])), a.len() + 10);
    assert!(r.next().unwrap().is_ok());
    assert!(r.next().is_none());
    let room = cap - (a.len() + 10);
    let fill = vec![0u8; room];
    assert_eq!(r.push(&b[10..]), b.len() - 10);
    assert_eq!(
        r.push(&fill[..room - (b.len() - 10)]),
        room - (b.len() - 10)
    );
    let f = r.next().unwrap().unwrap();
    assert_eq!((f.seq, f.payload.len()), (2, 500));
    assert!(f.payload.iter().all(|&b| b == 3));
}

// REQ: OA-FEED-157
#[test]
fn a_frame_is_available_exactly_when_all_of_its_bytes_are() {
    let f = msg(7, b"hello");
    let n = f.len();
    let next_start = *b"OAF1";
    let mut buf = big_buf();
    // One byte short: not yet.
    let mut r = FrameReader::new(&mut buf).unwrap();
    r.push(&f[..n - 1]);
    assert!(r.next().is_none());
    assert_eq!(r.offset(), 0);
    // Exactly one frame.
    r.push(&f[n - 1..]);
    let g = r.next().unwrap().unwrap();
    assert_eq!((g.seq, g.payload, g.raw.len()), (7, &b"hello"[..], n));
    assert!(r.next().is_none());
    // One byte over, and the whole magic of the next frame over.
    for extra in [&next_start[..1], &next_start[..]] {
        let mut r = FrameReader::new(&mut buf).unwrap();
        r.push(&cat(&[&f, extra]));
        let g = r.next().unwrap().unwrap();
        assert_eq!((g.seq, g.raw.len()), (7, n));
        assert!(r.next().is_none());
        assert_eq!(r.offset(), n as u64);
        assert_eq!(r.skipped_total(), 0);
    }
    // An empty payload is the smallest frame, and is delivered at HEADER_LEN + TRAILER_LEN.
    let e = msg(1, b"");
    assert_eq!(e.len(), HEADER_LEN + TRAILER_LEN);
    let mut r = FrameReader::new(&mut buf).unwrap();
    r.push(&e[..HEADER_LEN]);
    assert!(r.next().is_none());
    r.push(&e[HEADER_LEN..]);
    assert_eq!(r.next().unwrap().unwrap().payload.len(), 0);
    // Magic: three bytes wait, four are judged.
    let mut r = FrameReader::new(&mut buf).unwrap();
    r.push(b"XXX");
    assert!(r.next().is_none());
    r.push(b"X");
    let err = r.next().unwrap().unwrap_err();
    assert_eq!(err.kind, ErrorKind::BadMagic { found: *b"XXXX" });
    // Header: a bad kind is judged at the full header, not before.
    let mut bad = e.clone();
    bad[4] = 9;
    let mut r = FrameReader::new(&mut buf).unwrap();
    r.push(&bad[..HEADER_LEN - 1]);
    assert!(r.next().is_none());
    r.push(&bad[HEADER_LEN - 1..HEADER_LEN]);
    let err = r.next().unwrap().unwrap_err();
    assert_eq!(err.kind, ErrorKind::UnknownKind { kind: 9 });
}

// REQ: OA-FEED-158
#[test]
fn offset_is_the_absolute_stream_position_through_resync_and_compaction() {
    let mut buf = big_buf();
    let a = msg(0, b"aaaa");
    let b = msg(1, b"bbbbbbbb");
    let mut r = FrameReader::new(&mut buf).unwrap();
    assert_eq!(r.offset(), 0);
    r.push(&cat(&[&a, &b]));
    assert!(r.next().unwrap().is_ok());
    assert_eq!(r.offset(), a.len() as u64);
    assert!(r.next().unwrap().is_ok());
    assert_eq!(r.offset(), (a.len() + b.len()) as u64);
    assert!(r.next().is_none());
    assert_eq!(r.offset(), (a.len() + b.len()) as u64);

    // Seven bytes of garbage with no magic, then a frame.
    let mut r = FrameReader::new(&mut buf).unwrap();
    r.push(&cat(&[b"garbage", &a]));
    let err = r.next().unwrap().unwrap_err();
    assert_eq!(err.offset, 0);
    assert_eq!(r.offset(), 1);
    let g = r.next().unwrap().unwrap();
    assert_eq!((g.offset, g.skipped_before), (7, 7));
    assert_eq!(r.offset(), 7 + a.len() as u64);
    assert_eq!(r.skipped_total(), 7);

    // Garbage alone: all but a possible partial magic at the tail is dropped.
    let mut r = FrameReader::new(&mut buf).unwrap();
    r.push(b"xxxxxxxxxx");
    assert!(r.next().unwrap().is_err());
    assert_eq!(r.offset(), 1);
    assert!(r.next().is_none());
    assert_eq!(r.offset(), 7);
    assert_eq!(r.skipped_total(), 7);

    // Compaction moves bytes inside the buffer but not the stream position.
    let c = msg(2, &vec![5u8; MAX_PAYLOAD]);
    let mut r = FrameReader::new(&mut buf).unwrap();
    r.push(&cat(&[&a, &c[..4]]));
    assert!(r.next().unwrap().is_ok());
    assert!(r.next().is_none());
    assert_eq!(r.offset(), a.len() as u64);
    assert_eq!(r.push(&c[4..]), c.len() - 4);
    assert_eq!(r.offset(), a.len() as u64);
    let f = r.next().unwrap().unwrap();
    assert_eq!(f.offset, a.len() as u64);
    assert_eq!(r.offset(), (a.len() + c.len()) as u64);
    // A truncated end is skipped bytes, and the offset moves past them.
    let mut r = FrameReader::new(&mut buf).unwrap();
    r.push(&b[..10]);
    assert!(r.next().is_none());
    let e = r.finish().unwrap();
    assert_eq!(e.offset, 0);
    assert_eq!(r.offset(), 10);
}

// REQ: OA-FEED-159
#[test]
fn recording_size_defaults_are_two_gib_and_256_mib() {
    assert_eq!(cli::DEFAULT_MAX_BYTES, 2_147_483_648);
    assert_eq!(cli::DEFAULT_ROTATE_BYTES, 268_435_456);
    let args = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let live = |v: &[&str]| match cli::parse(&args(v)).unwrap() {
        Command::Live(l) => l,
        other => panic!("{other:?}"),
    };
    let l = live(&["--stdin"]);
    assert_eq!(l.max_bytes, 2 * 1024 * 1024 * 1024);
    assert_eq!(l.rotate_bytes, 256 * 1024 * 1024);
    // Each default holds when only the other is given.
    let l = live(&["--stdin", "--max-bytes", "1000"]);
    assert_eq!(
        (l.max_bytes, l.rotate_bytes),
        (1000, cli::DEFAULT_ROTATE_BYTES)
    );
    let l = live(&["--stdin", "--rotate-bytes", "500"]);
    assert_eq!((l.max_bytes, l.rotate_bytes), (cli::DEFAULT_MAX_BYTES, 500));
    assert_eq!(cli::DEFAULT_SUMMARY_SECS, 10);
}

// REQ: OA-FEED-170
#[test]
fn frames_the_compressor_held_are_lost_when_a_flush_or_finish_fails() {
    let f = msg(0, b"<a/>");
    // Three frames inside one second sit unflushed; the fourth, a second later, flushes and fails.
    let (mut rec, sw) = switched_recorder("lostflush", Format::Gzip);
    for t in [1000, 1000, 1000] {
        assert_eq!(rec.write_frame(&f, t, t).unwrap(), Outcome::Written);
    }
    fail_now(&sw);
    assert!(rec.write_frame(&f, 1001, 1001).is_err());
    assert_eq!(
        rec.lost_frames(),
        4,
        "three held and the one that hit the error"
    );
    // A flush that succeeds empties the count: only the frames after it are at risk.
    let (mut rec, sw) = switched_recorder("lostafterflush", Format::Gzip);
    for t in [1000, 1000] {
        rec.write_frame(&f, t, t).unwrap();
    }
    rec.idle_flush(1005).unwrap();
    for t in [1005, 1005] {
        rec.write_frame(&f, t, t).unwrap();
    }
    fail_now(&sw);
    assert!(rec.finish().is_err());
    assert_eq!(rec.lost_frames(), 2);
    // A failed idle flush loses what it held; a later frame adds one.
    let (mut rec, sw) = switched_recorder("lostidle", Format::Gzip);
    for t in [1000, 1000, 1000] {
        rec.write_frame(&f, t, t).unwrap();
    }
    fail_now(&sw);
    assert!(rec.idle_flush(1005).is_err());
    assert_eq!(rec.lost_frames(), 3);
    assert_eq!(rec.write_frame(&f, 1006, 1006).unwrap(), Outcome::Failed);
    assert_eq!(rec.lost_frames(), 4);
    // Raw frames go straight to the file: nothing is held back.
    let (mut rec, sw) = switched_recorder("lostraw", Format::Raw);
    for t in [1000, 1000] {
        rec.write_frame(&f, t, t).unwrap();
    }
    fail_now(&sw);
    assert!(rec.finish().is_ok());
    assert_eq!(rec.lost_frames(), 0);
    assert!(rec.failure().is_none());
}

// REQ: OA-FEED-171
#[test]
fn a_stop_at_the_size_cap_gives_an_exit_reason_and_a_healthy_recording_gives_none() {
    let dir = tmpdir("capexit");
    let f = msg(0, b"<a/>");
    let mut rec = Recorder::open(&dir, "f", u64::MAX, 100, Format::Raw).unwrap();
    assert_eq!(rec.exit_reason(), None);
    assert_eq!(rec.write_frame(&f, 1000, 1000).unwrap(), Outcome::Written);
    assert_eq!(rec.exit_reason(), None);
    loop {
        if rec.write_frame(&f, 1000, 1000).unwrap() == Outcome::StoppedNow {
            break;
        }
    }
    let reason = rec.exit_reason().unwrap();
    assert!(
        reason.contains("size cap") && reason.contains("--max-bytes 100"),
        "{reason}"
    );
    // A failure takes precedence over the cap.
    let (mut failing, sw) = switched_recorder("capfail", Format::Raw);
    fail_now(&sw);
    assert!(failing.write_frame(&f, 1000, 1000).is_err());
    assert!(failing
        .exit_reason()
        .unwrap()
        .contains("no space left on device"));
    rec.finish().unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
}

fn seqs(list: &[u64]) -> FeedHealth {
    let mut h = FeedHealth::new();
    for &s in list {
        observe_frame(&mut h, &fr(Kind::Message, 0, s, SEC, 0, b"x"), SEC);
    }
    h
}

// REQ: OA-FEED-172
#[test]
fn a_pair_swapped_on_the_wire_is_one_reordered_frame_and_nothing_missing() {
    let h = seqs(&[3, 5, 4]);
    assert_eq!((h.missing, h.reordered, h.regressions), (0, 1, 0));
    // The expectation did not move: the next frame in order is normal.
    let h = seqs(&[3, 5, 4, 6]);
    assert_eq!(
        (h.gaps, h.missing, h.reordered, h.regressions),
        (1, 0, 1, 0)
    );
    // A frame arriving late out of a wider gap takes one off what is missing.
    let h = seqs(&[3, 6, 4]);
    assert_eq!((h.missing, h.reordered, h.regressions), (1, 1, 0));
    let h = seqs(&[3, 6, 4, 5]);
    assert_eq!((h.missing, h.reordered, h.regressions), (0, 2, 0));
    // From the far end, and from the middle of a gap.
    let h = seqs(&[3, 7, 6, 4]);
    assert_eq!((h.missing, h.reordered, h.regressions), (1, 2, 0));
    let h = seqs(&[3, 8, 5]);
    assert_eq!((h.missing, h.reordered, h.regressions), (3, 1, 0));
    // The same late frame twice is filled once and then a repeat (OA-FEED-174), not a restart.
    let h = seqs(&[3, 6, 4, 4]);
    assert_eq!(
        (h.missing, h.reordered, h.duplicates, h.regressions),
        (1, 1, 1, 0)
    );
}

// REQ: OA-FEED-172
#[test]
fn a_true_regression_still_counts_and_forgets_the_gap() {
    // A bridge restart: counting from 0 again is a regression, not a late frame.
    let h = seqs(&[10, 15, 0]);
    assert_eq!((h.missing, h.reordered, h.regressions), (4, 0, 1));
    // The old gap is gone, so the restart does not fill it later.
    let h = seqs(&[10, 15, 0, 1, 12]);
    assert_eq!((h.missing, h.reordered, h.regressions), (4 + 10, 0, 1));
    // A frame below an earlier gap than the last one is a regression.
    let h = seqs(&[0, 3, 4, 8, 2]);
    assert_eq!((h.missing, h.reordered, h.regressions), (2 + 3, 0, 1));
}

// REQ: OA-FEED-173
#[test]
fn read_frames_counts_a_frame_cut_short_at_the_end_of_the_stream() {
    use crate::stream::read_frames;
    let a = frame(1, FLAG_BYTES, 0, 10, 0, b"first");
    let b = frame(1, FLAG_BYTES, 1, 20, 0, b"second");
    let run = |tail: &[u8]| {
        let mut bytes = a.clone();
        bytes.extend_from_slice(&b);
        bytes.extend_from_slice(tail);
        let mut seen: Vec<Vec<u8>> = Vec::new();
        let bad = read_frames(&mut bytes.as_slice(), &mut |f| {
            seen.push(f.payload.to_vec())
        })
        .unwrap();
        (seen, bad)
    };
    let whole = vec![b"first".to_vec(), b"second".to_vec()];
    assert_eq!(run(&[]), (whole.clone(), 0));
    // A partial header, then a partial body: the frames before are kept, the partial one counted.
    assert_eq!(run(&a[..10]), (whole.clone(), 1));
    assert_eq!(run(&a[..HEADER_LEN + 2]), (whole, 1));
}

// REQ: OA-FEED-174
#[test]
fn a_repeat_or_unremembered_late_frame_inside_the_last_gap_range_makes_no_false_gap() {
    // A redelivered late frame: 4 fills the gap, the second 4 is a repeat, 6 is the next expected.
    let h = seqs(&[3, 5, 4, 4, 6]);
    assert_eq!(
        (h.gaps, h.missing, h.duplicates, h.regressions, h.reordered),
        (1, 0, 1, 0, 1)
    );
    assert_eq!(h.seq_next, 7);
    // A middle fill keeps only the upper part of the gap; a late frame of the forgotten lower part
    // must not restart the expectation (it would count 5..7 missing again when 8 arrives).
    let h = seqs(&[3, 7, 5, 4, 8]);
    assert_eq!((h.gaps, h.regressions, h.seq_next), (1, 0, 9));
    assert!(h.missing <= 2, "missing {}", h.missing);
    // Below the last gap's range is still a regression (a bridge restart counts from 0 again).
    let h = seqs(&[3, 5, 4, 0, 1]);
    assert_eq!((h.regressions, h.seq_next), (1, 2));
    // Before any gap, a lower frame is a regression as before.
    let h = seqs(&[10, 11, 5]);
    assert_eq!((h.regressions, h.duplicates), (1, 0));
}

// ---------------------------------------------------------------- OAF2 (monotonic time)

fn frame_mono(seq: u64, recv_ns: i64, mono_ns: u64, payload: &[u8]) -> Vec<u8> {
    let mut v = vec![0u8; HEADER_LEN_V2 + payload.len() + TRAILER_LEN];
    let n = encode_frame_mono(&mut v, 1, 2, seq, recv_ns, 77, mono_ns, payload).unwrap();
    assert_eq!(n, v.len());
    v
}

// REQ: OA-FEED-175
#[test]
fn oaf2_frame_round_trips_every_field_including_the_monotonic_time() {
    let a = frame_mono(9, -5, u64::MAX, b"payload two");
    assert_eq!(&a[..4], b"OAF2");
    let b = frame(1, 0, 10, 6, 0, b"old");
    let mut buf = big_buf();
    let mut r = FrameReader::new(&mut buf).unwrap();
    assert_eq!(r.push(&cat(&[&a, &b])), a.len() + b.len());
    let f = r.next().unwrap().unwrap();
    assert_eq!(
        (f.kind, f.flags, f.seq, f.recv_unix_ns, f.jms_timestamp_ms),
        (Kind::Message, 2, 9, -5, 77)
    );
    assert_eq!(f.mono_ns(), Some(u64::MAX));
    assert_eq!(
        (f.payload, f.raw, f.offset),
        (&b"payload two"[..], &a[..], 0)
    );
    // An OAF1 frame in the same stream has no monotonic time.
    let g = r.next().unwrap().unwrap();
    assert_eq!((g.seq, g.mono_ns(), g.offset), (10, None, a.len() as u64));
    assert!(r.next().is_none() && r.finish().is_none());
}

// REQ: OA-FEED-176
#[test]
fn oaf2_frames_are_identical_however_the_stream_is_chunked_and_resync_after_damage() {
    let a = frame_mono(0, 1, 5, b"alpha");
    let b = frame(1, 0, 1, 2, 0, b"beta");
    let c = frame_mono(2, 3, 9, b"");
    let stream = cat(&[&a, &b, &c]);
    let mut buf = big_buf();
    let whole = run_whole(&mut buf, &stream);
    assert!(whole.errors.is_empty());
    assert_eq!(whole.frames.len(), 3);
    for cut in 1..stream.len() {
        let two = run_chunks(&mut buf, &[&stream[..cut], &stream[cut..]]);
        assert_eq!(two, whole, "split at {cut}");
    }
    // A bit flip in an OAF2 payload is a CRC error at its offset; the reader finds the next
    // frame (of either version) and conserves every byte.
    let mut bad = a.clone();
    bad[HEADER_LEN_V2 + 1] ^= 0x10;
    let damaged = cat(&[&bad, &b, &c]);
    let run = run_whole(&mut buf, &damaged);
    assert_eq!(run.errors.len(), 1);
    assert_eq!(run.errors[0].offset, 0);
    assert!(matches!(run.errors[0].kind, ErrorKind::Crc { .. }));
    assert_eq!(
        run.frames.iter().map(|f| f.seq).collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert_conserved(&run, damaged.len());
    // A truncated OAF2 header is a truncated tail.
    let run = run_whole(&mut buf, &a[..HEADER_LEN_V2 - 1]);
    assert!(matches!(
        run.errors.last().map(|e| e.kind),
        Some(ErrorKind::Truncated { .. })
    ));
}

// REQ: OA-FEED-177
#[test]
fn a_recorded_oaf2_frame_reads_back_from_a_gzip_recording_with_its_monotonic_time() {
    let dir = tmpdir("oaf2-rec");
    let mut rec = Recorder::open(&dir, "scds-m", u64::MAX, u64::MAX, Format::Gzip).unwrap();
    let frames = [
        frame_mono(0, 1_000_000_000, 10, b"one"),
        frame(1, 0, 1, 2_000_000_000, 0, b"two"),
        frame_mono(2, 3_000_000_000, 3_000_000_123, b"three"),
    ];
    for f in &frames {
        assert_eq!(rec.write_frame(f, 1, 1).unwrap(), Outcome::Written);
    }
    rec.finish().unwrap();
    let file = std::fs::read_dir(&dir)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let rep = replay_files(&[file]).unwrap();
    assert!(rep.errors.is_empty() && rep.gzip.is_empty());
    assert_eq!(rep.health.frames, 3);
    // The same bytes through the reader give the stored monotonic times.
    let mut buf = big_buf();
    let mut r = FrameReader::new(&mut buf).unwrap();
    r.push(&cat(&[&frames[0], &frames[1], &frames[2]]));
    let monos: Vec<Option<u64>> = (0..3)
        .map(|_| r.next().unwrap().unwrap().mono_ns())
        .collect();
    assert_eq!(monos, vec![Some(10), None, Some(3_000_000_123)]);
}

// ---------------------------------------------------------------- feed label

/// Labels that are not a single safe filename component.
fn bad_labels() -> Vec<String> {
    vec![
        "/tmp/escape".into(),
        "../escape".into(),
        "a/b".into(),
        "a\\b".into(),
        "nul\0byte".into(),
        String::new(),
        ".hidden".into(),
        "..".into(),
        "with space".into(),
        "caf\u{e9}".into(),
        "x".repeat(65),
    ]
}

// REQ: OA-FEED-179
#[test]
fn a_feed_label_that_is_not_a_single_safe_filename_component_is_refused_at_parse_time() {
    for label in bad_labels() {
        let args = vec!["--stdin".to_string(), "--feed".to_string(), label.clone()];
        assert!(cli::parse(&args).is_err(), "accepted {label:?}");
    }
    // A default derived from the env file name is held to the same rule.
    let a = |s: &str| -> Vec<String> { s.split_whitespace().map(String::from).collect() };
    assert!(cli::parse(&a("--env /x/bad\u{e9}.env")).is_err());
    for ok in ["scds-tbfm", "a.b_c-1", "x", &"y".repeat(64)] {
        let args = vec!["--stdin".to_string(), "--feed".to_string(), ok.to_string()];
        assert!(cli::parse(&args).is_ok(), "refused {ok:?}");
    }
    assert!(cli::parse(&a("--stdin")).is_ok());
    assert!(cli::parse(&a("--env /x/scds-tbfm.env")).is_ok());
}

// REQ: OA-FEED-179
#[test]
fn the_recorder_refuses_a_feed_label_that_could_leave_its_directory() {
    let dir = tmpdir("label");
    for label in bad_labels() {
        let r = Recorder::open(&dir.join("rec"), &label, u64::MAX, u64::MAX, Format::Raw);
        assert!(r.is_err(), "accepted {label:?}");
    }
    // Refused before the directory is touched.
    assert!(!dir.join("rec").exists());
    assert!(Recorder::open(&dir.join("rec"), "scds-x", u64::MAX, u64::MAX, Format::Raw).is_ok());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_cores_crc32_agrees_with_the_zip_crates() {
    // The core holds its own copy (B8: core may not name a shell crate); this keeps them equal.
    let data: Vec<u8> = (0..4099u32)
        .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
        .collect();
    for n in [0, 1, 7, 8, 9, 63, 64, 1000, data.len()] {
        assert_eq!(
            crate::core::crc32::crc32(&data[..n]),
            oa_zip::crc32(&data[..n]),
            "{n} bytes"
        );
    }
}

// ------------------------------------------- exact-limit tests through the real entry points

fn fresh_budget(capacity: usize) -> crate::FrameWorkBudget {
    crate::FrameWorkBudget::for_capacity(capacity)
}

// REQ: OA-FEED-182
#[test]
fn push_copy_allowance_is_exact_at_the_limit_and_refused_one_below() {
    let f = msg(5, b"copy allowance edge");
    let mut buf = big_buf();
    let mut r = FrameReader::new(&mut buf).unwrap();
    let below = crate::FrameWorkBudget {
        copied_bytes: f.len() as u64 - 1,
        ..fresh_budget(MAX_FRAME_LEN)
    };
    let refusal = r.push_with_budget(&f, below).unwrap_err();
    assert_eq!(refusal.kind, crate::FrameWorkKind::CopiedBytes);
    assert_eq!(refusal.required, f.len() as u64);
    assert_eq!(refusal.available, f.len() as u64 - 1);
    assert_eq!(r.buffered_len(), 0);
    assert_eq!(r.work_stats().copied_bytes, 0);
    let exact = crate::FrameWorkBudget {
        copied_bytes: f.len() as u64,
        ..fresh_budget(MAX_FRAME_LEN)
    };
    assert_eq!(r.push_with_budget(&f, exact), Ok(f.len()));
    assert_eq!(r.work_stats().copied_bytes, f.len() as u64);
}

// REQ: OA-FEED-182
#[test]
fn push_compaction_allowance_is_exact_at_the_limit_and_refused_one_below() {
    let first = msg(1, b"first");
    let second = msg(2, &[7u8; 1000]);
    let partial = &second[..500];
    let mut buf = big_buf();
    let mut r = FrameReader::new(&mut buf).unwrap();
    let wire = cat(&[&first, partial]);
    assert_eq!(
        r.push_with_budget(&wire, fresh_budget(MAX_FRAME_LEN)),
        Ok(wire.len())
    );
    let got = r
        .next_with_budget(fresh_budget(MAX_FRAME_LEN))
        .unwrap()
        .unwrap()
        .unwrap()
        .seq;
    assert_eq!(got, 1);
    // 500 bytes remain buffered after a consumed frame, so room is short for more than the
    // tail and a push of free + 1 bytes must move those 500 bytes first.
    assert_eq!(r.buffered_len(), 500);
    let free = MAX_FRAME_LEN - wire.len();
    let data = vec![0u8; free + 1];
    let below = crate::FrameWorkBudget {
        compacted_bytes: 499,
        ..fresh_budget(MAX_FRAME_LEN)
    };
    let refusal = r.push_with_budget(&data, below).unwrap_err();
    assert_eq!(refusal.kind, crate::FrameWorkKind::CompactedBytes);
    assert_eq!(refusal.required, 500);
    assert_eq!(refusal.available, 499);
    assert_eq!(r.buffered_len(), 500);
    assert_eq!(r.work_stats().compacted_bytes, 0);
    let exact = crate::FrameWorkBudget {
        compacted_bytes: 500,
        ..fresh_budget(MAX_FRAME_LEN)
    };
    assert_eq!(r.push_with_budget(&data, exact), Ok(free + 1));
    assert_eq!(r.work_stats().compacted_bytes, 500);
    assert_eq!(r.buffered_len(), 500 + free + 1);
}

// REQ: OA-FEED-184
#[test]
fn resync_scan_allowance_is_exact_at_avail_minus_three_and_refused_below() {
    let f = msg(27, b"after damaged prefix");
    let input = cat(&[b"garbage", &f]);
    let mut buf = big_buf();
    let mut r = FrameReader::new(&mut buf).unwrap();
    r.push_with_budget(&input, fresh_budget(MAX_FRAME_LEN))
        .unwrap();
    assert!(matches!(
        r.next_with_budget(fresh_budget(MAX_FRAME_LEN)).unwrap(),
        Some(Err(FrameError {
            kind: ErrorKind::BadMagic { .. },
            ..
        }))
    ));
    let avail = r.buffered_len() as u64;
    let below = crate::FrameWorkBudget {
        scan_positions: avail - 4,
        ..fresh_budget(MAX_FRAME_LEN)
    };
    let refusal = r.next_with_budget(below).unwrap_err();
    assert_eq!(refusal.kind, crate::FrameWorkKind::ScanPositions);
    assert_eq!(refusal.required, avail - 3);
    assert_eq!(refusal.available, avail - 4);
    assert_eq!(r.work_stats().scan_positions, 0);
    let exact = crate::FrameWorkBudget {
        scan_positions: avail - 3,
        ..fresh_budget(MAX_FRAME_LEN)
    };
    assert_eq!(r.next_with_budget(exact).unwrap().unwrap().unwrap().seq, 27);
}

// REQ: OA-FEED-189
#[test]
fn recorder_open_uses_the_real_directory_limit_exactly() {
    let limit = crate::record::DEFAULT_MAX_DIRECTORY_ENTRIES;
    assert_eq!(limit, 16_384);
    let dir = tmpdir("dir-limit-real");
    for i in 0..limit {
        std::fs::File::create(dir.join(format!("n{i}"))).unwrap();
    }
    let open = |dir: &std::path::Path| Recorder::open(dir, "feed", 1 << 20, 1 << 30, Format::Raw);
    assert!(open(&dir).is_ok(), "exactly the limit must pass");
    std::fs::File::create(dir.join("one-more")).unwrap();
    let refused = open(&dir).err().expect("limit + 1 must be refused");
    assert_eq!(refused.kind(), std::io::ErrorKind::InvalidData);
    assert!(refused.to_string().contains("entry limit exceeded"));
    let _ = std::fs::remove_dir_all(dir);
}

// REQ: OA-FEED-190
#[test]
fn compatibility_push_and_next_scale_with_a_large_buffer() {
    const CAP: usize = 8 << 20;
    let one = msg(1, &[9u8; 100]);
    let count = (6usize << 20).div_ceil(one.len());
    let wire = one.repeat(count);
    assert!(wire.len() >= 6 << 20);
    let mut storage = vec![0u8; CAP];
    let mut r = FrameReader::new(&mut storage).unwrap();
    assert_eq!(r.push(&wire), wire.len(), "silent stall");
    assert_eq!(r.last_work_refusal(), None);
    let mut frames = 0usize;
    while let Some(ev) = r.next() {
        assert!(ev.is_ok());
        frames += 1;
    }
    assert_eq!(frames, count);
    assert_eq!(r.last_work_refusal(), None);
    // Garbage of the same size resynchronises in one legacy call too.
    let garbage = vec![0u8; 6 << 20];
    let mut storage2 = vec![0u8; CAP];
    let mut g = FrameReader::new(&mut storage2).unwrap();
    assert_eq!(g.push(&garbage), garbage.len());
    while g.next().is_some() {}
    assert_eq!(g.last_work_refusal(), None);
    assert!(g.skipped_total() >= (6 << 20) - 3);
}

// REQ: OA-FEED-189
#[test]
fn a_refused_recorder_start_leaves_every_existing_file_byte_identical() {
    let limit = crate::record::DEFAULT_MAX_DIRECTORY_ENTRIES;
    let dir = tmpdir("dir-limit-untouched");
    let mut expected = Vec::new();
    for i in 0..=limit {
        let name = format!("f{i}.oaf");
        let body = format!("recording {i}").into_bytes();
        std::fs::write(dir.join(&name), &body).unwrap();
        expected.push((name, body));
    }
    let refused = Recorder::open(&dir, "feed", 1 << 20, 1 << 30, Format::Raw)
        .err()
        .expect("more than the limit must be refused");
    let text = refused.to_string();
    assert!(
        text.contains(&format!("{} entries found", limit + 1)),
        "{text}"
    );
    assert!(text.contains(&format!("limit of {limit}")), "{text}");
    assert!(text.contains("no existing file was touched"), "{text}");
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), limit + 1);
    for (name, body) in &expected {
        assert_eq!(&std::fs::read(dir.join(name)).unwrap(), body, "{name}");
    }
    let _ = std::fs::remove_dir_all(dir);
}

// REQ: OA-FEED-184
#[test]
fn replay_resyncs_past_a_corrupt_maximum_length_frame_in_the_larger_storage() {
    let mut bad = vec![0u8; MAX_FRAME_LEN];
    assert_eq!(
        encode_frame_mono(&mut bad, 1, 0, 2, 0, 0, 7, &vec![3u8; MAX_PAYLOAD]),
        Some(MAX_FRAME_LEN)
    );
    let last = bad.len() - 1;
    bad[last] ^= 1;
    // A short valid frame first, so the bad frame completes mid-chunk and more than a quota's
    // worth of bytes (the tail) is buffered behind it when its CRC fails.
    let mut stream = msg(1, b"before");
    stream.extend_from_slice(&bad);
    let tail = 70usize;
    for seq in 0..tail as u64 {
        stream.extend_from_slice(&msg(10 + seq, &[5u8; 1000]));
    }
    assert!(stream.len() - 6 - MAX_FRAME_LEN >= 64 * 1024);
    let r = replay_sources(vec![&stream[..]]).expect("no work refusal");
    assert_eq!(r.errors.len(), 1, "{:?}", r.errors);
    assert!(matches!(r.errors[0].error.kind, ErrorKind::Crc { .. }));
    assert_eq!(r.health.messages, 1 + tail as u64);
    assert_eq!(r.bytes, stream.len() as u64);
}

// REQ: OA-SEAL-014
#[test]
fn replay_drops_and_counts_frames_received_in_a_sealed_window() {
    // The window of the list: 2026-11-20T00:00:00Z to 2026-11-23T00:17:00Z, Unix nanoseconds.
    const FROM: i64 = 1_795_132_800_000_000_000;
    const TO: i64 = 1_795_393_020_000_000_000;
    let mut bytes = Vec::new();
    for (i, t) in [FROM - 1, FROM, TO - 1, TO].iter().enumerate() {
        let mut buf = vec![0u8; MAX_FRAME_LEN];
        let n = encode_frame(&mut buf, 1, FLAG_BYTES, i as u64, *t, t / 1_000_000, b"x").unwrap();
        bytes.extend_from_slice(&buf[..n]);
    }
    let rep = replay_sources(vec![std::io::Cursor::new(bytes)]).unwrap();
    assert_eq!(rep.sealed_window_dropped, 2);
    assert_eq!(rep.health.frames, 2);
}
