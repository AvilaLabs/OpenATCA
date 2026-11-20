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
    refuse_unrecognised(dir)?;
    let names = file_names(&files);
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
        files: names,
        decoded: None,
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

fn file_names(files: &[std::path::PathBuf]) -> Vec<String> {
    files
        .iter()
        .map(|p| {
            p.file_name().map_or_else(
                || p.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            )
        })
        .collect()
}

/// A regular file directly in `dir` that is not `.oaf` or `.oaf.gz` is refused by name: a
/// recording renamed or left out would otherwise be skipped silently.
fn refuse_unrecognised(dir: &Path) -> io::Result<()> {
    let walk = oa_sealed::walk_unsealed(dir)?;
    let mut bad: Vec<String> = Vec::new();
    for p in &walk.files {
        if p.parent() != Some(dir) {
            continue;
        }
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned());
        if let Some(n) = name {
            if !(n.ends_with(".oaf") || n.ends_with(".oaf.gz")) {
                bad.push(n);
            }
        }
    }
    if bad.is_empty() {
        return Ok(());
    }
    let more = bad.len().saturating_sub(20);
    bad.truncate(20);
    Err(io::Error::other(format!(
        "{}: unrecognised files (not .oaf or .oaf.gz): {}{}",
        dir.display(),
        bad.join(", "),
        if more > 0 {
            format!(" and {more} more")
        } else {
            String::new()
        }
    )))
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
    let decoded = clip.reports.time_ns.len() as u64;
    let stats: Stats = scan_unsorted(window, &mut times);
    Ok(Feed {
        name: "ADS-B",
        stats,
        files: file_names(&[path.to_path_buf()]),
        decoded: Some(decoded),
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
