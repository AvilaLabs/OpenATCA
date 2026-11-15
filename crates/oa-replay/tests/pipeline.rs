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
use oa_replay::{
    errors_total, new_pipeline, process_frame, rows_applied_total, snapshot, state_hash,
    Checkpoint, ErrKind, FeedKind, Pipeline, PipelineConfig, ERR_NAMES,
};
use oa_replay::{merge_key, merge_streams, run_streams, spec_from_bytes};

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
    frames: [u64; oa_replay::FEED_COUNT],
    rows_applied: [u64; oa_replay::FEED_COUNT],
    errors: [u64; oa_replay::ERR_COUNT],
    bytes: u64,
    flights: usize,
}

fn run_sources(srcs: Vec<(FeedKind, Vec<u8>)>, chunk: usize) -> Outcome {
    run_chains(srcs.into_iter().map(|(f, b)| (f, vec![b])).collect(), chunk)
}

/// Each stream is a feed and its chain of files (rotated recordings).
fn run_chains(streams: Vec<(FeedKind, Vec<Vec<u8>>)>, chunk: usize) -> Outcome {
    let specs = streams
        .into_iter()
        .map(|(f, files)| spec_from_bytes(f, files))
        .collect();
    let mut p = pipeline();
    let r = run_streams(&mut p, specs, chunk, &mut |_| {}).unwrap();
    Outcome {
        checkpoints: p.checkpoints.to_vec(),
        last: snapshot(&p),
        frames: p.frames,
        rows_applied: p.rows_applied,
        errors: p.errors,
        bytes: r.bytes,
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
    for c in p.checkpoints.iter() {
        assert_eq!(c.replay_ns.rem_euclid(1_000_000_000), 500_000_000);
        assert!(c.flights > 0 && c.rows_applied > 0);
    }
    let times: Vec<i64> = p.checkpoints.iter().map(|c| c.replay_ns).collect();
    assert!(times.windows(2).all(|w| w[0] < w[1]));
    // The hash is the state's hash at that point: the last checkpoint is before the last frames.
    assert_ne!(p.checkpoints.get(3).unwrap().hash, state_hash(&p.table));
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
    assert_eq!(p.other_frames, [1, 1, 0, 0, 0]);
    assert_eq!(p.frames, [6, 4, 0, 0, 0]);
    assert_eq!(p.messages, [5, 3, 0, 0, 0]);
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

// REQ: OA-RPL-021
#[test]
fn merge_order_is_total_and_stable() {
    // Four streams of frames with many equal receive times; each stream is in receive order, as
    // a recording is. The merged order must be exactly the global sort of the total key.
    let feeds = [
        FeedKind::Sfdps,
        FeedKind::Tbfm,
        FeedKind::Sfdps,
        FeedKind::Tbfm,
    ];
    for seed in 1..=20u64 {
        let mut rng = Rng(seed * 7919);
        let mut files: Vec<Vec<u8>> = Vec::new();
        for _ in 0..4 {
            let mut t = 0i64;
            let mut bytes = Vec::new();
            for seq in 0..(10 + rng.below(30)) as u64 {
                t += rng.below(3) as i64;
                bytes.extend(frame(1, seq, t, b"x"));
            }
            files.push(bytes);
        }
        let specs = files
            .iter()
            .zip(feeds)
            .map(|(b, f)| spec_from_bytes(f, vec![b.clone()]))
            .collect();
        let mut got = Vec::new();
        merge_streams(specs, 17, &mut |m| {
            got.push(merge_key(m.recv_ns, m.feed, m.seq, m.stream, m.offset));
        })
        .unwrap();
        let mut want = got.clone();
        want.sort_unstable();
        assert_eq!(got, want, "seed {seed}");
        assert!(got.windows(2).all(|w| w[0] < w[1]), "total: no ties");
        let n: usize = files
            .iter()
            .map(|f| f.len() / frame(1, 0, 0, b"x").len())
            .sum();
        assert_eq!(got.len(), n);
    }
    // Equal time across feeds: SFDPS first, whatever order the streams were named in.
    let a = frame(1, 5, T0, b"a");
    let b = frame(1, 1, T0, b"b");
    for order in [[0usize, 1], [1, 0]] {
        let mk = |i: usize| {
            if i == 0 {
                spec_from_bytes(FeedKind::Tbfm, vec![b.clone()])
            } else {
                spec_from_bytes(FeedKind::Sfdps, vec![a.clone()])
            }
        };
        let mut feeds = Vec::new();
        merge_streams(vec![mk(order[0]), mk(order[1])], 100, &mut |m| {
            feeds.push(m.feed)
        })
        .unwrap();
        assert_eq!(feeds, [FeedKind::Sfdps, FeedKind::Tbfm]);
    }
}

// REQ: OA-RPL-027
#[test]
fn a_frame_going_backwards_inside_a_file_is_not_reordered_and_is_counted() {
    // Reordering would need unbounded lookahead, so the merge keeps file order within a stream.
    let mut a = Vec::new();
    for (seq, t) in [10i64, 20, 5, 30].into_iter().enumerate() {
        a.extend(frame(
            1,
            seq as u64,
            T0 + t * 1_000_000_000,
            sfdps_payload(t).as_bytes(),
        ));
    }
    let mut b = Vec::new();
    for (seq, t) in [15i64, 25].into_iter().enumerate() {
        b.extend(frame(
            1,
            seq as u64,
            T0 + t * 1_000_000_000,
            tbfm_payload(t).as_bytes(),
        ));
    }
    let specs = vec![
        spec_from_bytes(FeedKind::Sfdps, vec![a.clone()]),
        spec_from_bytes(FeedKind::Tbfm, vec![b.clone()]),
    ];
    let mut order = Vec::new();
    merge_streams(specs, 64, &mut |m| {
        order.push((m.recv_ns - T0) / 1_000_000_000)
    })
    .unwrap();
    assert_eq!(order, [10, 15, 20, 5, 25, 30]);
    let o = run_sources(vec![(FeedKind::Sfdps, a), (FeedKind::Tbfm, b)], 64);
    assert_eq!(o.errors[ErrKind::TimeRegression as usize], 1);
    assert_eq!(o.frames[..2], [4, 2]);
}

// REQ: OA-RPL-028
#[test]
fn rotated_files_chain_to_the_same_replay_as_one_file() {
    let (s, t) = recordings(80);
    let whole = run_sources(
        vec![(FeedKind::Sfdps, s.clone()), (FeedKind::Tbfm, t.clone())],
        4096,
    );
    // Cut each recording on frame boundaries into several files, one of them compressed.
    let split = |bytes: &[u8], parts: usize| -> Vec<Vec<u8>> {
        let mut bounds = vec![0usize];
        let mut at = 0;
        while at < bytes.len() {
            let len = u32::from_le_bytes(bytes[at + 32..at + 36].try_into().unwrap()) as usize;
            at += 40 + len;
            bounds.push(at);
        }
        let per = (bounds.len() - 1).div_ceil(parts);
        let mut out = Vec::new();
        let mut i = 0;
        while i < bounds.len() - 1 {
            let j = (i + per).min(bounds.len() - 1);
            out.push(bytes[bounds[i]..bounds[j]].to_vec());
            i = j;
        }
        out
    };
    for parts in [2usize, 3, 7] {
        let mut sf_files = split(&s, parts);
        let tb_files = split(&t, parts);
        assert_eq!(sf_files.len(), tb_files.len());
        assert!(sf_files.len() > 1);
        let o = run_chains(
            vec![
                (FeedKind::Sfdps, sf_files.clone()),
                (FeedKind::Tbfm, tb_files.clone()),
            ],
            1000,
        );
        same(&whole, &o);
        // One of the files compressed.
        sf_files[1] = gzip(&sf_files[1]);
        let o = run_chains(
            vec![(FeedKind::Sfdps, sf_files), (FeedKind::Tbfm, tb_files)],
            333,
        );
        same(&whole, &o);
    }
}

// REQ: OA-RPL-029
#[test]
fn tfms_and_fns_payloads_are_decoded_and_a_foreign_document_is_a_typed_error() {
    let mut p = pipeline();
    for k in 0..5i64 {
        process_frame(
            &mut p,
            FeedKind::Tfms,
            Kind::Message,
            T0 + k,
            b"<anything/>",
        );
    }
    process_frame(&mut p, FeedKind::Fns, Kind::Message, T0 + 9, b"<notam/>");
    process_frame(
        &mut p,
        FeedKind::Fns,
        Kind::Status,
        T0 + 10,
        b"state=connected",
    );
    // Every feed has a decoder now: nothing is left uncounted as "not applied".
    assert_eq!(p.not_applied, [0; 5]);
    assert_eq!(p.frames, [0, 0, 5, 2, 0]);
    assert_eq!(p.other_frames, [0, 0, 0, 1, 0]);
    assert_eq!(p.errors[ErrKind::TfmsNotTfms as usize], 5);
    assert_eq!(p.errors[ErrKind::FnsRejected as usize], 1);
    assert_eq!(errors_total(&p.errors), 6);
    assert_eq!(rows_applied_total(&p), 0);
    assert_eq!(p.table.live, 0);
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
