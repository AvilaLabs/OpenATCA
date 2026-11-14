//! Tests of the frame pipeline, the recording reader and the merge: checkpoints, error counting,
//! determinism across runs and chunkings, gzip, merge order and corrupted input.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod common;

use common::*;
use oa_deflate::GzipWriter;
use oa_feed::Kind;
use oa_replay::source::{load_sources, merge_key, merge_order, payload, run_loaded, Rec};
use oa_replay::{
    errors_total, new_pipeline, process_frame, snapshot, state_hash, Checkpoint, ErrKind, FeedKind,
    Pipeline, PipelineConfig, ERR_NAMES,
};
use std::io::Cursor;

const T0: i64 = DAY * 1_000_000 + 3_500_000_000; // DAY in ns, plus 3.5 s: not on a boundary.

fn pipeline() -> Pipeline {
    small_pipeline(256)
}

/// Frame `k` of the SFDPS recording: three flights, positions that advance.
fn sfdps_payload(k: i64) -> String {
    let base = (k % 12) as u32;
    let msgs: Vec<Sf> = (0..3u32)
        .map(|j| {
            let n = base + j + 1;
            let mut m = sf(n, k * 1000)
                .plan(&format!("KA{n:06}AB"))
                .call(&format!("TST{n}"))
                .pos(30.0 + f64::from(n), -80.0, k * 1000);
            if k % 5 == 0 {
                m = m.arr("KTST").unit("ZXX", "08");
            }
            m
        })
        .collect();
    sf_doc(&msgs)
}

/// Frame `k` of the TBFM recording: two flights whose gufi is the SFDPS plan id.
fn tbfm_payload(k: i64) -> String {
    let base = (k % 12) as u32;
    let airs: Vec<Tb> = (0..2u32)
        .map(|j| {
            let n = base + j + 1;
            tb(k * 1000, Some(&format!("KA{n:06}AB")), &format!("T{n:05}"))
                .mrp("MFXA1", "09L", "NEW")
                .eta("MFXA1", 600_000 + k * 1000)
                .sta("MFXA1", 700_000)
                .sch("MFXA1", if k % 7 == 0 { "YES" } else { "NO" })
        })
        .collect();
    tb_doc(k * 1000, "ZTT", &airs)
}

/// Two recordings: (sfdps bytes, tbfm bytes). SFDPS frames are 1.5 s apart, TBFM 1.7 s, so the
/// merged order interleaves; each has a STATUS frame and a payload that is not XML.
fn recordings(n: i64) -> (Vec<u8>, Vec<u8>) {
    let mut sfd = Vec::new();
    let mut tbf = Vec::new();
    for k in 0..n {
        sfd.extend(frame(
            1,
            k as u64,
            T0 + k * 1_500_000_000,
            sfdps_payload(k).as_bytes(),
        ));
        tbf.extend(frame(
            1,
            k as u64,
            T0 + k * 1_700_000_000,
            tbfm_payload(k).as_bytes(),
        ));
    }
    sfd.extend(frame(3, 900, T0 + 7_000_000_000, b"state=connected"));
    sfd.extend(frame(1, 901, T0 + 8_000_000_000, b"not xml at all"));
    tbf.extend(frame(1, 902, T0 + 9_000_000_000, b"<unclosed>"));
    (sfd, tbf)
}

struct Outcome {
    checkpoints: Vec<Checkpoint>,
    last: Checkpoint,
    frames: [u64; 2],
    rows_applied: [u64; 2],
    errors: [u64; oa_replay::ERR_COUNT],
    bytes: u64,
    flights: usize,
}

fn run_sources(srcs: Vec<(FeedKind, Vec<u8>)>, chunk: usize) -> Outcome {
    let cursors = srcs.into_iter().map(|(f, b)| (f, Cursor::new(b))).collect();
    let mut l = load_sources(cursors, chunk).unwrap();
    merge_order(&mut l);
    let mut p = pipeline();
    for k in 0..oa_replay::ERR_COUNT {
        p.errors[k] += l.errors[k];
    }
    run_loaded(&mut p, &l);
    Outcome {
        checkpoints: p.checkpoints.clone(),
        last: snapshot(&p),
        frames: p.frames,
        rows_applied: p.rows_applied,
        errors: p.errors,
        bytes: l.bytes,
        flights: p.table.live,
    }
}

fn same(a: &Outcome, b: &Outcome) {
    assert_eq!(a.checkpoints, b.checkpoints);
    assert_eq!(a.last, b.last);
    assert_eq!((a.frames, a.rows_applied), (b.frames, b.rows_applied));
    assert_eq!(
        (a.errors, a.bytes, a.flights),
        (b.errors, b.bytes, b.flights)
    );
}

// REQ: OA-RPL-016
#[test]
fn checkpoints_fall_on_the_replay_time_cadence_and_record_counts_and_hash() {
    let mut p = pipeline();
    let times: Vec<i64> = (0..40).map(|k| T0 + k * 1_000_000_000).collect();
    let mut expect_frames = Vec::new();
    let mut last_bucket = times[0].div_euclid(10_000_000_000);
    for (j, &t) in times.iter().enumerate() {
        feed(&mut p, FeedKind::Sfdps, t, &sfdps_payload(j as i64));
        let b = t.div_euclid(10_000_000_000);
        if b > last_bucket {
            expect_frames.push(j as u64);
            last_bucket = b;
        }
    }
    // 40 s of frames starting at 3.5 s into the cadence cross buckets at 10, 20, 30, 40 s.
    assert_eq!(p.checkpoints.len(), 4);
    assert_eq!(
        p.checkpoints.iter().map(|c| c.frames).collect::<Vec<_>>(),
        expect_frames
    );
    for c in &p.checkpoints {
        assert_eq!(c.replay_ns.rem_euclid(1_000_000_000), 500_000_000);
        assert!(c.flights > 0 && c.rows_applied > 0);
    }
    let times: Vec<i64> = p.checkpoints.iter().map(|c| c.replay_ns).collect();
    assert!(times.windows(2).all(|w| w[0] < w[1]));
    // The hash is the state's hash at that point: the last checkpoint is before the last frames.
    assert_ne!(p.checkpoints[3].hash, state_hash(&p.table));
    // A different cadence is a setting.
    let slow = PipelineConfig {
        checkpoint_ns: 20_000_000_000,
        ..PipelineConfig::default()
    };
    let mut q = new_pipeline(slow).unwrap();
    for k in 0..40i64 {
        feed(
            &mut q,
            FeedKind::Sfdps,
            T0 + k * 1_000_000_000,
            &sfdps_payload(k),
        );
    }
    assert_eq!(q.checkpoints.len(), 2);
    let zero = PipelineConfig {
        checkpoint_ns: 0,
        ..PipelineConfig::default()
    };
    assert!(new_pipeline(zero).is_err());
}

// REQ: OA-RPL-017
#[test]
fn every_failure_is_counted_by_kind() {
    let mut p = pipeline();
    let t = T0;
    feed(&mut p, FeedKind::Sfdps, t, "not xml");
    feed(&mut p, FeedKind::Sfdps, t + 1, &tbfm_payload(1));
    feed(&mut p, FeedKind::Tbfm, t + 2, "<unclosed>");
    feed(&mut p, FeedKind::Tbfm, t + 3, &sfdps_payload(1));
    feed(
        &mut p,
        FeedKind::Sfdps,
        t + 4,
        &sf_doc(&[sf(1, 0)]).replace(&uuid(1), "not-a-uuid"),
    );
    feed(
        &mut p,
        FeedKind::Sfdps,
        t + 5,
        &sf_doc(&[sf(2, 0)]).replace("timestamp=\"2026", "timestamp=\"20x6"),
    );
    let long = tb_doc(
        1000,
        "ZTT",
        &[tb(1000, Some("KA000001AB"), "T00001").mrp("M", "09L", "NEW")],
    )
    .replace("aid=\"TST123\"", "aid=\"WAYTOOLONGNAME\"");
    feed(&mut p, FeedKind::Tbfm, t + 6, &long);
    // A message the decoder accepts but one of whose fields is malformed keeps the row.
    let bad_field = sf_doc(&[sf(3, 0).pos(30.0, -80.0, 0).alt(1)]).replace(
        "<altitude uom=\"FEET\">1.0</altitude>",
        "<altitude uom=\"FURLONGS\">1.0</altitude>",
    );
    feed(&mut p, FeedKind::Sfdps, t + 7, &bad_field);
    process_frame(
        &mut p,
        FeedKind::Sfdps,
        Kind::Status,
        t + 8,
        b"state=connected",
    );
    process_frame(
        &mut p,
        FeedKind::Tbfm,
        Kind::Reject,
        t + 9,
        b"reason=oversize",
    );
    let n = |k: ErrKind| p.errors[k as usize];
    assert_eq!(n(ErrKind::SfdpsXml), 1);
    assert_eq!(n(ErrKind::SfdpsUnsupportedRoot), 1);
    assert_eq!(n(ErrKind::TbfmXml), 1);
    assert_eq!(n(ErrKind::TbfmNotTbfm), 1);
    assert_eq!(n(ErrKind::SfdpsBadGufi), 1);
    assert_eq!(n(ErrKind::SfdpsBadTimestamp), 1);
    assert_eq!(n(ErrKind::TbfmField), 1);
    assert_eq!(n(ErrKind::SfdpsField), 1);
    assert_eq!(errors_total(&p.errors), 8, "{:?}", p.errors);
    assert_eq!(p.other_frames, [1, 1]);
    assert_eq!(p.frames, [6, 4]);
    assert_eq!(p.messages, [5, 3]);
    // The row with the bad field was kept without it.
    assert_eq!(p.rows_applied[0], 1);
    assert_eq!(ERR_NAMES.len(), oa_replay::ERR_COUNT);
}

// REQ: OA-RPL-018
#[test]
fn replay_time_never_goes_backwards_and_the_regression_is_counted() {
    let mut p = pipeline();
    feed(
        &mut p,
        FeedKind::Sfdps,
        T0 + 5_000_000_000,
        &sfdps_payload(1),
    );
    feed(
        &mut p,
        FeedKind::Sfdps,
        T0 + 1_000_000_000,
        &sfdps_payload(2),
    );
    assert_eq!(p.errors[ErrKind::TimeRegression as usize], 1);
    assert_eq!(p.replay_ns, T0 + 5_000_000_000);
    feed(
        &mut p,
        FeedKind::Sfdps,
        T0 + 6_000_000_000,
        &sfdps_payload(3),
    );
    assert_eq!(p.replay_ns, T0 + 6_000_000_000);
    assert_eq!(p.errors[ErrKind::TimeRegression as usize], 1);
}

// REQ: OA-RPL-019
#[test]
fn replay_is_deterministic_across_runs_and_across_chunkings() {
    let (s, t) = recordings(80);
    let srcs = || vec![(FeedKind::Sfdps, s.clone()), (FeedKind::Tbfm, t.clone())];
    let base = run_sources(srcs(), 64 * 1024);
    assert!(base.checkpoints.len() >= 10, "{}", base.checkpoints.len());
    assert!(base.last.flights > 0 && base.rows_applied[0] > 0 && base.rows_applied[1] > 0);
    assert_eq!(base.errors[ErrKind::SfdpsXml as usize], 1);
    assert_eq!(base.errors[ErrKind::TbfmXml as usize], 1);
    // Twice with the same chunking, then every other chunking, including one byte at a time
    // (the reader takes two at least, to tell gzip from raw).
    same(&base, &run_sources(srcs(), 64 * 1024));
    for chunk in [1usize, 2, 3, 7, 100, 4096, 1 << 20, 1 << 24] {
        same(&base, &run_sources(srcs(), chunk));
    }
    // The order the files are named in does not change the replay: the merge is a total order.
    let swapped = vec![(FeedKind::Tbfm, t.clone()), (FeedKind::Sfdps, s.clone())];
    same(&base, &run_sources(swapped, 4096));
}

fn gzip(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut w = GzipWriter::new(6);
    let mut sink = |b: &[u8]| -> Result<(), ()> {
        out.extend_from_slice(b);
        Ok(())
    };
    w.write(data, &mut sink).unwrap();
    w.finish(&mut sink).unwrap();
    out
}

// REQ: OA-RPL-020
#[test]
fn compressed_and_raw_recordings_replay_identically() {
    let (s, t) = recordings(40);
    let raw = run_sources(
        vec![(FeedKind::Sfdps, s.clone()), (FeedKind::Tbfm, t.clone())],
        4096,
    );
    let gz = |c| {
        run_sources(
            vec![(FeedKind::Sfdps, gzip(&s)), (FeedKind::Tbfm, gzip(&t))],
            c,
        )
    };
    same(&raw, &gz(4096));
    same(&raw, &gz(5));
    // One of each.
    let mixed = run_sources(
        vec![(FeedKind::Sfdps, gzip(&s)), (FeedKind::Tbfm, t.clone())],
        999,
    );
    same(&raw, &mixed);
    // A gzip file cut short is counted, not mistaken for a complete one; its frames up to the
    // cut still replay.
    let z = gzip(&s);
    let cut = run_sources(vec![(FeedKind::Sfdps, z[..z.len() / 2].to_vec())], 4096);
    assert_eq!(cut.errors[ErrKind::GzipTruncated as usize], 1);
    assert!(cut.frames[0] > 0 && cut.frames[0] < raw.frames[0]);
}

fn rec(feed: FeedKind, recv_ns: i64, seq: u64, file: u32, offset: u64) -> Rec {
    Rec {
        recv_ns,
        seq,
        offset,
        start: 0,
        len: 0,
        file,
        feed,
        kind: Kind::Message,
    }
}

// REQ: OA-RPL-021
#[test]
fn merge_order_is_total_and_stable() {
    use FeedKind::{Sfdps, Tbfm};
    // Equal times across feeds: SFDPS before TBFM; equal time and feed: by seq; equal all three
    // (two bridge runs with restarted counters, or one file given twice): by file, then offset.
    let want = vec![
        rec(Tbfm, 5, 0, 1, 0),
        rec(Sfdps, 10, 3, 0, 0),
        rec(Sfdps, 10, 7, 0, 40),
        rec(Sfdps, 10, 7, 2, 0),
        rec(Sfdps, 10, 7, 2, 40),
        rec(Tbfm, 10, 1, 1, 0),
        rec(Tbfm, 11, 0, 1, 80),
    ];
    let keys: Vec<_> = want.iter().map(merge_key).collect();
    assert!(
        keys.windows(2).all(|w| w[0] < w[1]),
        "strictly ascending: total, no ties"
    );
    for seed in 1..=50u64 {
        let mut v = want.clone();
        Rng(seed * 7919).shuffle(&mut v);
        let mut l = oa_replay::Loaded {
            recs: v,
            arena: Vec::new(),
            files: 3,
            bytes: 0,
            errors: [0; oa_replay::ERR_COUNT],
            frame_errors: Vec::new(),
        };
        merge_order(&mut l);
        assert_eq!(l.recs, want, "seed {seed}");
    }
    // Through real files: frames of two recordings at the same instant come out SFDPS first.
    let a = frame(1, 5, T0, sf_doc(&[sf(1, 0)]).as_bytes());
    let b = frame(
        1,
        1,
        T0,
        tb_doc(
            0,
            "ZTT",
            &[tb(0, Some("KA000001AB"), "T00001").flt("B38M/L")],
        )
        .as_bytes(),
    );
    let mut l = load_sources(
        vec![
            (FeedKind::Tbfm, Cursor::new(b)),
            (FeedKind::Sfdps, Cursor::new(a)),
        ],
        100,
    )
    .unwrap();
    merge_order(&mut l);
    assert_eq!(
        l.recs.iter().map(|r| r.feed).collect::<Vec<_>>(),
        [Sfdps, Tbfm]
    );
    assert!(payload(&l, &l.recs[0]).starts_with(b"<?xml"));
}

// REQ: OA-RPL-022
#[test]
fn corrupted_recordings_never_panic_and_always_leave_a_count() {
    let mut sfd = Vec::new();
    let mut tbf = Vec::new();
    for k in 0..3i64 {
        sfd.extend(frame(
            1,
            k as u64,
            T0 + k * 1_000_000_000,
            sfdps_payload(k).as_bytes(),
        ));
        tbf.extend(frame(
            1,
            k as u64,
            T0 + k * 1_000_000_000,
            tbfm_payload(k).as_bytes(),
        ));
    }
    let mut bounds = vec![0usize];
    for k in 0..3i64 {
        let n = frame(1, 0, 0, sfdps_payload(k).as_bytes()).len();
        bounds.push(bounds[bounds.len() - 1] + n);
    }
    let clean = run_sources(
        vec![
            (FeedKind::Sfdps, sfd.clone()),
            (FeedKind::Tbfm, tbf.clone()),
        ],
        4096,
    );
    assert_eq!(errors_total(&clean.errors), 0);
    // Every single-byte flip of the SFDPS file, and every truncation: never a panic, never
    // silent (a frame error is counted), never more frames than the clean run.
    // (Every byte of the first frame and its neighbour's header; every fifth byte after.)
    for i in (0..sfd.len()).filter(|&i| i < 2500 || i % 5 == 0) {
        let mut bad = sfd.clone();
        bad[i] ^= 0xff;
        let o = run_sources(
            vec![(FeedKind::Sfdps, bad), (FeedKind::Tbfm, tbf.clone())],
            4096,
        );
        assert!(errors_total(&o.errors) >= 1, "flip at {i}");
        assert!(o.frames[0] <= clean.frames[0]);
    }
    for cut in (0..sfd.len()).filter(|&c| c < 200 || c % 13 == 0) {
        let o = run_sources(vec![(FeedKind::Sfdps, sfd[..cut].to_vec())], 4096);
        assert!(o.frames[0] <= clean.frames[0]);
        // A cut inside a frame is a counted truncation; between frames it is just a shorter file.
        assert_eq!(
            errors_total(&o.errors) == 0,
            bounds.contains(&cut),
            "cut at {cut}"
        );
    }
    // Random garbage and random damage.
    let mut rng = Rng(0xdead_beef);
    for _ in 0..200 {
        let n = rng.below(3000);
        let junk: Vec<u8> = (0..n).map(|_| rng.next() as u8).collect();
        run_sources(
            vec![(FeedKind::Sfdps, junk.clone()), (FeedKind::Tbfm, junk)],
            4096,
        );
        let mut bad = tbf.clone();
        for _ in 0..1 + rng.below(8) {
            let i = rng.below(bad.len());
            bad[i] = rng.next() as u8;
        }
        run_sources(vec![(FeedKind::Tbfm, bad)], 4096);
    }
    // Payload damage under a valid CRC (the frame is re-encoded), through the whole pipeline.
    let mut p = pipeline();
    for (feed, payload) in [
        (FeedKind::Sfdps, sfdps_payload(2)),
        (FeedKind::Tbfm, tbfm_payload(2)),
    ] {
        let bytes = payload.into_bytes();
        for i in 0..bytes.len() {
            for flip in [0xffu8, b'<'] {
                let mut bad = bytes.clone();
                bad[i] = if flip == 0xff { bad[i] ^ 0xff } else { flip };
                process_frame(&mut p, feed, Kind::Message, T0 + i as i64, &bad);
            }
        }
        // Truncated payloads.
        for cut in 0..bytes.len() {
            process_frame(&mut p, feed, Kind::Message, T0, &bytes[..cut]);
        }
    }
    assert!(errors_total(&p.errors) > 0);
    assert!(p.table.live <= p.table.capacity);
}
