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
use crate::frame::{encode_frame, ErrorKind, Frame, FrameError, Kind};
use crate::health::{
    format_summary, lat_bin, lat_bin_upper_ms, latency_quantile_ms, observe_frame, rates_milli,
    FeedHealth, STATE_CONNECTED, STATE_DISCONNECTED,
};
use crate::reader::FrameReader;
use crate::record::{utc_stamp, Format, Outcome, Recorder};
use crate::replay::{replay_files, replay_sources, GzNoteKind};
use crate::{
    FLAG_BYTES, FLAG_REDELIVERED, HEADER_LEN, MAGIC, MAX_FRAME_LEN, MAX_PAYLOAD, TRAILER_LEN,
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
    Frame {
        seq,
        recv_unix_ns: recv_ns,
        jms_timestamp_ms: jms_ms,
        offset: 0,
        skipped_before: 0,
        payload,
        raw: &[],
        kind,
        flags,
    }
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
    let mut v = vec![0u8; MAX_FRAME_LEN];
    assert_eq!(
        encode_frame(&mut v, 1, 0, 1, 0, 0, &big),
        Some(MAX_FRAME_LEN)
    );
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
    assert_eq!(
        (h.gaps, h.missing, h.duplicates, h.regressions),
        (1, 2, 1, 1)
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
    assert_eq!(size_of::<Frame<'static>>(), 80);
    assert_eq!(size_of::<crate::health::Slot>(), 16);
    assert_eq!(size_of::<FeedHealth>(), 2408);
    // The size budget has no hidden padding: fields sum to the struct size.
    assert_eq!(20 * 8 + 2 * 4 + 8 + 24 * 8 + 133 * 8 + 61 * 16, 2408);
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
        outcomes.push(rec.write_frame(&f, 1_700_000_000 + i / 3).unwrap());
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
    assert_eq!(utc_stamp(0), "19700101T000000Z");
    assert_eq!(utc_stamp(-1), "19691231T235959Z");
    assert_eq!(utc_stamp(1_700_000_000), "20231114T221320Z");
    assert_eq!(utc_stamp(951_782_400), "20000229T000000Z"); // leap day
    assert_eq!(utc_stamp(4_102_444_799), "20991231T235959Z");
    assert_eq!(
        crate::record::utc_iso(1_700_000_000),
        "2023-11-14T22:13:20Z"
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
        assert_eq!(rec.write_frame(f, *secs).unwrap(), Outcome::Written);
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
    again.write_frame(&frames[0].1, 1_700_000_000).unwrap();
    again.finish().unwrap();
    let mut raw = Recorder::open(&dir, "scds-x", u64::MAX, u64::MAX, Format::Raw).unwrap();
    raw.write_frame(&frames[0].1, 1_700_000_000).unwrap();
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
        rec.write_frame(f, *secs).unwrap();
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
        match rec.write_frame(f, *secs).unwrap() {
            Outcome::Written => {}
            Outcome::StoppedNow => {
                assert!(stopped_at.is_none());
                stopped_at = Some(i);
            }
            Outcome::Stopped => assert!(stopped_at.is_some()),
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
        rec.write_frame(&frames[off..off + n], t).unwrap();
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
        rec.write_frame(&stream[off..off + n], t).unwrap();
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
