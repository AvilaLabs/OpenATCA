//! Shell (B1): the two inputs, read through the guarded readers of `oa-bench` and `oa-replay`.

use std::io;
use std::path::Path;

use oa_bench::input::{file_opener, list_inputs, load_adsb_counted, DirKind};
use oa_replay::{merge_counted, spec_from_paths, FeedKind, ERR_NAMES};

use crate::core::gaps::{scan_unsorted, Stats, Window};
use crate::core::report::Feed;

/// The SFDPS recording: the receive time of every message frame in the window, sorted, and the
/// reader's counts. Frames are in file order, so files that overlap or are named out of order
/// still give the true intervals.
pub fn read_sfdps(dir: &Path, window: Window) -> io::Result<Feed> {
    // The guard sees the directory and every entry before anything is opened.
    oa_sealed::refuse_io(dir)?;
    let (kind, files) = list_inputs(dir)?;
    if kind != DirKind::Oaf || files.is_empty() {
        return Err(io::Error::other(format!(
            "{}: no .oaf or .oaf.gz recordings (raw files carry no receive time)",
            dir.display()
        )));
    }
    let n_files = files.len();
    let mut times: Vec<i64> = Vec::new();
    let (mut prev, mut regress) = (i64::MIN, 0u64);
    let spec = spec_from_paths(FeedKind::Sfdps, files);
    let (rep, end) = merge_counted(vec![spec], 1 << 16, &mut |m| {
        if m.kind == oa_feed::Kind::Message {
            if m.recv_ns < prev {
                regress += 1;
            }
            prev = m.recv_ns;
            if m.recv_ns >= window.from_ns && m.recv_ns <= window.to_ns {
                times.push(m.recv_ns);
            }
        }
        Ok(())
    });
    end?;
    let mut damage: Vec<(String, u64)> = Vec::new();
    for (name, n) in ERR_NAMES.iter().zip(rep.errors.iter()) {
        if *n > 0 {
            damage.push((format!("reader {name}"), *n));
        }
    }
    let total: u64 = rep.errors.iter().sum();
    Ok(Feed {
        name: "SFDPS",
        stats: scan_unsorted(window, &mut times),
        damage,
        info: vec![
            ("files".to_string(), n_files as u64),
            ("reader gzip and frame errors".to_string(), total),
            (
                "frames earlier than the one before in file order (sorted, not damage)".to_string(),
                regress,
            ),
            (
                "frames left out in a sealed window (not damage)".to_string(),
                rep.sealed_window_dropped,
            ),
        ],
    })
}

/// The ADS-B clip: every report time in the window, sorted, and the clip's damage counts.
pub fn read_adsb(path: &Path, window: Window) -> io::Result<Feed> {
    oa_sealed::refuse_io(path)?;
    let mut sealed = 0u64;
    let clip = load_adsb_counted(path, &file_opener(), &mut sealed)?;
    let mut times: Vec<i64> = clip
        .reports
        .time_ns
        .iter()
        .copied()
        .filter(|t| *t >= window.from_ns && *t <= window.to_ns)
        .collect();
    let stats: Stats = scan_unsorted(window, &mut times);
    Ok(Feed {
        name: "ADS-B",
        stats,
        damage: vec![("damaged frames".to_string(), clip.bad_frames)],
        info: vec![
            (
                "records dropped from decoded frames (not damage)".to_string(),
                clip.dropped_records,
            ),
            (
                "rows left out in a sealed window (not damage)".to_string(),
                clip.sealed_window_dropped,
            ),
        ],
    })
}
