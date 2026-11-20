#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
//! Real SCDS recordings: every `*.oaf` under `OPENATCA_SCDS_REC` is converted to `.oaf.gz`
//! through the recorder (the same path a live run takes, with the frames' own receive times),
//! both are replayed, and the summaries must be identical. Prints sizes and ratios. The
//! converted files go to the build's scratch directory, never beside the data. Skips when the
//! variable is unset.

use std::path::{Path, PathBuf};
use std::time::Instant;

use oa_feed::record::{Format, Recorder};
use oa_feed::replay::replay_files;
use oa_feed::{format_summary, FrameReader, MAX_FRAME_LEN};

/// Writes every frame of `src` to `dir` through a [`Recorder`]; returns the files made.
fn convert(src: &Path, dir: &Path, feed: &str) -> Vec<PathBuf> {
    let data = oa_sealed::read(src).unwrap();
    let mut rec = Recorder::open(dir, feed, 256 << 20, u64::MAX, Format::Gzip).unwrap();
    let mut storage = vec![0u8; 2 * MAX_FRAME_LEN];
    let mut reader = FrameReader::new(&mut storage).unwrap();
    let mut off = 0;
    while off < data.len() {
        let took = reader.push(&data[off..]);
        off += took;
        while let Some(ev) = reader.next() {
            if let Ok(f) = ev {
                let secs = f.recv_unix_ns.div_euclid(1_000_000_000);
                rec.write_frame(f.raw, secs, secs).unwrap();
            }
        }
        assert!(took > 0);
    }
    rec.finish().unwrap();
    // seal-allow: reads back a file the test wrote in its own temp dir
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    v.sort();
    v
}

// REQ: OA-FEED-032
#[test]
fn real_recordings_compress_and_replay_identically() {
    let Some(root) = std::env::var_os("OPENATCA_SCDS_REC") else {
        eprintln!("OPENATCA_SCDS_REC is not set: skipping the real recordings test");
        return;
    };
    // Sealed entries are skipped and counted, never opened (OA-SEAL-012).
    let walk = oa_sealed::walk_unsealed(Path::new(&root)).unwrap();
    eprintln!("sealed entries skipped: {}", walk.skipped);
    let recs: Vec<PathBuf> = walk
        .files
        .into_iter()
        .filter(|p| {
            p.extension().is_some_and(|x| x == "oaf") && p.parent() == Some(Path::new(&root))
        })
        .collect();
    assert!(!recs.is_empty(), "no .oaf files under {root:?}");
    let scratch = Path::new(env!("CARGO_TARGET_TMPDIR")).join("real-rec-gz");
    let _ = std::fs::remove_dir_all(&scratch);
    for (k, raw) in recs.iter().enumerate() {
        let dir = scratch.join(format!("{k}"));
        let t = Instant::now();
        let gz = convert(raw, &dir, "rec");
        let make_s = t.elapsed().as_secs_f64();
        let len = |p: &Path| oa_sealed::open(p).unwrap().metadata().unwrap().len();
        let raw_len = len(raw);
        let gz_len: u64 = gz.iter().map(|p| len(p)).sum();
        let t = Instant::now();
        let a = replay_files(std::slice::from_ref(raw)).unwrap();
        let raw_s = t.elapsed().as_secs_f64();
        let t = Instant::now();
        let b = replay_files(&gz).unwrap();
        let gz_s = t.elapsed().as_secs_f64();
        let sa = format_summary(&a.health, a.health.last_recv_ns);
        let sb = format_summary(&b.health, b.health.last_recv_ns);
        eprintln!(
            "{}: {raw_len} -> {gz_len} bytes in {} file(s), ratio {:.2}x; compress {make_s:.1}s, replay raw {raw_s:.1}s / gz {gz_s:.1}s",
            raw.file_name().unwrap().to_string_lossy(),
            gz.len(),
            raw_len as f64 / gz_len as f64
        );
        eprintln!("  raw: {sa}\n  gz:  {sb}");
        assert_eq!(sa, sb, "summaries differ for {raw:?}");
        assert_eq!(a.health, b.health);
        assert_eq!(a.bytes, b.bytes);
        assert_eq!(
            a.errors.iter().map(|e| e.error).collect::<Vec<_>>(),
            b.errors.iter().map(|e| e.error).collect::<Vec<_>>()
        );
        assert!(b.gzip.is_empty(), "{:?}", b.gzip);
        assert!(gz_len < raw_len);
    }
}
