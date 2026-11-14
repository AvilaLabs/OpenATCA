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
    errors_total, load_files, merge_order, new_pipeline, run_loaded, snapshot, ErrKind, FeedKind,
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
    let mut l = load_files(files, chunk).unwrap();
    merge_order(&mut l);
    let mut p = new_pipeline(PipelineConfig::default()).unwrap();
    for k in 0..oa_replay::ERR_COUNT {
        p.errors[k] += l.errors[k];
    }
    let t = Instant::now();
    run_loaded(&mut p, &l);
    let elapsed = t.elapsed().as_secs_f64();
    let final_hash = to_hex(&snapshot(&p).hash);
    Run {
        p,
        final_hash,
        elapsed,
        bytes: l.bytes,
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

    assert_eq!(a.final_hash, b.final_hash);
    assert_eq!(a.final_hash, c.final_hash);
    assert_eq!(a.p.checkpoints, b.p.checkpoints);
    assert_eq!(a.p.checkpoints, c.p.checkpoints);
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
