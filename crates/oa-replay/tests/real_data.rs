//! Replay of the real SCDS recordings. Needs `OPENATCA_REC`: a directory holding
//! `scds-sfdps-20261114T144712Z.oaf` and `scds-tbfm-20261114T143917Z.oaf`; skips when it is
//! unset. The data is never copied into the repository. The two recordings cover different
//! minutes (TBFM from 14:39, SFDPS from 14:47), which is fine: the merge puts them in one
//! timeline and the flights they share are those alive in both.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_replay::{
    errors_total, new_pipeline, run_streams, snapshot, spec_from_paths, ErrKind, FeedKind,
    Pipeline, PipelineConfig, ERR_NAMES,
};
use oa_sha256::to_hex;
use std::path::PathBuf;
use std::time::Instant;

struct Run {
    p: Pipeline,
    final_hash: String,
    elapsed: f64,
    bytes: u64,
}

fn replay(files: &[(FeedKind, PathBuf)], chunk: usize) -> Run {
    let specs = files
        .iter()
        .map(|(f, p)| spec_from_paths(*f, vec![p.clone()]))
        .collect();
    let mut p = new_pipeline(PipelineConfig::default()).unwrap();
    let t = Instant::now();
    let report = run_streams(&mut p, specs, chunk, &mut |_| {}).unwrap();
    let elapsed = t.elapsed().as_secs_f64();
    let final_hash = to_hex(&snapshot(&p).hash);
    Run {
        p,
        final_hash,
        elapsed,
        bytes: report.bytes,
    }
}

// REQ: OA-RPL-025
#[test]
fn the_real_recordings_replay_to_the_same_state_every_time() {
    let Some(dir) = std::env::var_os("OPENATCA_REC") else {
        eprintln!("OPENATCA_REC is not set: skipping the real recording replay");
        return;
    };
    let dir = PathBuf::from(dir);
    let files = vec![
        (FeedKind::Sfdps, dir.join("scds-sfdps-20261114T144712Z.oaf")),
        (FeedKind::Tbfm, dir.join("scds-tbfm-20261114T143917Z.oaf")),
    ];
    let a = replay(&files, 64 * 1024);
    let b = replay(&files, 64 * 1024);
    let c = replay(&files, 4097);
    let p = &a.p;
    let st = &p.table.stats;
    eprintln!("bytes={} frames={}", a.bytes, p.frames.iter().sum::<u64>());
    for (i, name) in oa_replay::FEED_NAMES.iter().enumerate() {
        eprintln!(
            "{name}: frames={} messages={} rows={} applied={} out_of_order={} refused_full={}",
            p.frames[i],
            p.messages[i],
            p.rows_seen[i],
            p.rows_applied[i],
            p.rows_out_of_order[i],
            p.rows_refused_full[i]
        );
    }
    eprintln!(
        "combined: {:.0} frames/s {:.0} rows/s in {:.3} s",
        p.frames.iter().sum::<u64>() as f64 / a.elapsed,
        p.rows_applied.iter().sum::<u64>() as f64 / a.elapsed,
        a.elapsed
    );
    for (k, n) in p.errors.iter().enumerate() {
        if *n > 0 {
            eprintln!("error {}={n}", ERR_NAMES[k]);
        }
    }
    eprintln!(
        "flights_live={} tbfm linked_gufi={} linked_tma={} created={} unlinked={} promoted={} \
         table out_of_order={} conflicts={} key_reassigned={} evicted={}",
        p.table.live,
        st.tbfm_linked_gufi,
        st.tbfm_linked_tma,
        st.tbfm_created,
        st.tbfm_unlinked,
        st.promoted,
        st.out_of_order,
        st.conflicts,
        st.key_reassigned,
        st.evicted
    );
    eprintln!("checkpoints={}", p.checkpoints.len());
    eprintln!(
        "final state hash run1={} run2={} run3(chunk 4097)={}",
        a.final_hash, b.final_hash, c.final_hash
    );

    // The hash of the earlier all-in-memory replay of these recordings.
    assert_eq!(
        a.final_hash,
        "c0d81e68b7a4ec71bcf8f1c2aa17262ee93c551c1392f019ed94508132e3e4c6"
    );
    assert_eq!(a.final_hash, b.final_hash);
    assert_eq!(a.final_hash, c.final_hash);
    assert_eq!(a.p.checkpoints.to_vec(), b.p.checkpoints.to_vec());
    assert_eq!(a.p.checkpoints.to_vec(), c.p.checkpoints.to_vec());
    assert!(p.frames[0] > 0 && p.frames[1] > 0);
    assert!(p.rows_applied[0] > 0 && p.rows_applied[1] > 0);
    assert!(p.table.live > 0);
    assert!(st.tbfm_linked_gufi > 0, "TBFM links to SFDPS flight plans");
    // The recordings are clean: no frame, gzip or clock error. (Payload-level rejects are
    // counted and reported above, not asserted: the feeds carry message types oa-fixm refuses.)
    for k in [
        ErrKind::FrameBadMagic,
        ErrKind::FrameUnknownKind,
        ErrKind::FrameOversize,
        ErrKind::FrameCrc,
        ErrKind::FrameTruncated,
        ErrKind::FrameReserved,
        ErrKind::GzipDamaged,
        ErrKind::GzipTruncated,
        ErrKind::TimeRegression,
        ErrKind::BadRow,
    ] {
        assert_eq!(p.errors[k as usize], 0, "{}", ERR_NAMES[k as usize]);
    }
    assert!(errors_total(&p.errors) < p.messages.iter().sum::<u64>());
    assert_eq!(st.refused_full, 0);
}

// REQ: OA-RPL-039
#[test]
fn a_real_adsb_clip_replays_to_the_same_state_every_time() {
    // OPENATCA_ADSB_CLIP: an OAF clip written by `oa-adsb-clip` from an adsb.lol archive.
    let Some(path) = std::env::var_os("OPENATCA_ADSB_CLIP") else {
        eprintln!("OPENATCA_ADSB_CLIP is not set: skipping the real ADS-B clip replay");
        return;
    };
    let files = vec![(FeedKind::Adsb, PathBuf::from(path))];
    let a = replay(&files, 64 * 1024);
    let b = replay(&files, 4097);
    let f = FeedKind::Adsb as usize;
    let p = &a.p;
    eprintln!(
        "adsb: frames={} rows={} applied={} out_of_order={} aircraft={} errors={} in {:.3} s",
        p.frames[f],
        p.rows_seen[f],
        p.rows_applied[f],
        p.rows_out_of_order[f],
        p.adsb.live,
        errors_total(&p.errors),
        a.elapsed
    );
    assert!(p.frames[f] > 0 && p.rows_applied[f] > 0 && p.adsb.live > 0);
    assert_eq!(errors_total(&p.errors), 0);
    assert_eq!(p.adsb.stats.refused_full, 0);
    assert_eq!(a.final_hash, b.final_hash);
    assert_eq!(a.p.checkpoints.to_vec(), b.p.checkpoints.to_vec());
}

/// The `.msg` files of `OPENATCA_DATA/scds/<dir>`, in name order; `None` (with a note) when the
/// variable is unset or the directory is absent.
fn scds_msgs(dir: &str) -> Option<Vec<Vec<u8>>> {
    let Some(root) = std::env::var_os("OPENATCA_DATA") else {
        eprintln!("OPENATCA_DATA is not set: skipping the real {dir} replay");
        return None;
    };
    let d = PathBuf::from(root).join("scds").join(dir);
    let Ok(rd) = std::fs::read_dir(&d) else {
        eprintln!("{} is absent: skipping the real {dir} replay", d.display());
        return None;
    };
    let mut files: Vec<PathBuf> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "msg"))
        .collect();
    files.sort();
    Some(files.iter().map(|f| std::fs::read(f).unwrap()).collect())
}

/// Raw `.msg` files carry no receive time (oa-bench reads them the same way), so the test gives
/// message `i` of a feed the replay time `start_ns + i` seconds. An adapter for the test only.
fn feed_msgs(p: &mut Pipeline, f: FeedKind, msgs: &[Vec<u8>], start_ns: i64) -> i64 {
    let mut t = start_ns;
    for m in msgs {
        oa_replay::process_frame(p, f, oa_feed::Kind::Message, t, m);
        t += 1_000_000_000;
    }
    t
}

// REQ: OA-RPL-059
#[test]
fn real_tfms_and_fns_samples_replay_to_the_same_state_every_time() {
    // The early SCDS samples of 2026-11-14: TFMS from 14:09:43Z, FNS from 14:10:45Z and, to give
    // the TFMS flights something to link to, the SFDPS sample that ends before them.
    let (Some(tfms), Some(fns)) = (
        scds_msgs("tfms-20261114T140943Z"),
        scds_msgs("fns-20261114T141045Z"),
    ) else {
        return;
    };
    let sfdps = scds_msgs("sfdps-20261114T135147Z").unwrap_or_default();
    const SFDPS_START: i64 = 1_794_664_307 * 1_000_000_000; // 2026-11-14T13:51:47Z
    const TFMS_START: i64 = 1_794_665_383 * 1_000_000_000; // 2026-11-14T14:09:43Z
    const FNS_START: i64 = 1_794_665_445 * 1_000_000_000; // 2026-11-14T14:10:45Z
    let run = || {
        // Ten-minute checkpoints: each one hashes every table, which is slow in a debug build.
        let cfg = PipelineConfig {
            checkpoint_ns: 600 * 1_000_000_000,
            ..PipelineConfig::default()
        };
        let mut p = new_pipeline(cfg).unwrap();
        let end = feed_msgs(&mut p, FeedKind::Sfdps, &sfdps, SFDPS_START);
        let end = feed_msgs(&mut p, FeedKind::Tfms, &tfms, TFMS_START.max(end));
        // The FNS sample starts after the TFMS one in wall time; replay time does not go back.
        feed_msgs(&mut p, FeedKind::Fns, &fns, FNS_START.max(end));
        p
    };
    let t = Instant::now();
    let p = run();
    let elapsed = t.elapsed().as_secs_f64();
    let q = run();
    let (tf, fnx) = (FeedKind::Tfms as usize, FeedKind::Fns as usize);
    let st = &p.tfms.stats;
    eprintln!(
        "sfdps files={} flights={} | tfms files={} rows={} applied={} out_of_order={} \
         refused_full={} flights={} created={} linked_plan={} linked_acid={} ambiguous={} \
         linked_now={} conflicts={} | programmes live={} applied={} by_program={:?}",
        sfdps.len(),
        p.table.live,
        tfms.len(),
        p.rows_seen[tf],
        p.rows_applied[tf],
        p.rows_out_of_order[tf],
        p.rows_refused_full[tf],
        oa_replay::tfms_live(&p.tfms),
        st.created,
        st.linked_plan,
        st.linked_acid,
        st.ambiguous,
        oa_replay::tfms_linked(&p.tfms),
        st.conflicts,
        oa_replay::flow_live(&p.flows),
        p.flows.stats.applied,
        p.flows.stats.by_program,
    );
    let fs = &p.fns.stats;
    eprintln!(
        "fns files={} rows={} applied={} active={} in_force={} created={} replaced={} \
         cancelled={} cancel_unmatched={} replace_unmatched={} expired={} out_of_order={} \
         no_key={} in {:.3} s",
        fns.len(),
        p.rows_seen[fnx],
        p.rows_applied[fnx],
        oa_replay::fns_live(&p.fns),
        oa_replay::fns_in_force(&p.fns, p.replay_ns / 1_000_000),
        fs.created,
        fs.replaced,
        fs.cancelled,
        fs.cancel_unmatched,
        fs.replace_unmatched,
        fs.expired,
        fs.out_of_order,
        fs.no_key,
        elapsed
    );
    for (k, n) in p.errors.iter().enumerate() {
        if *n > 0 {
            eprintln!("error {}={n}", ERR_NAMES[k]);
        }
    }
    assert!(p.rows_applied[tf] > 0 && p.rows_applied[fnx] > 0);
    assert!(oa_replay::tfms_live(&p.tfms) > 0 && oa_replay::fns_live(&p.fns) > 0);
    assert_eq!(snapshot(&p).hash, snapshot(&q).hash);
    assert_eq!(p.checkpoints.to_vec(), q.checkpoints.to_vec());
    assert_eq!(p.errors, q.errors);
    for k in [ErrKind::BadRow, ErrKind::TimeRegression, ErrKind::FnsNoKey] {
        assert_eq!(p.errors[k as usize], 0, "{}", ERR_NAMES[k as usize]);
    }
}
