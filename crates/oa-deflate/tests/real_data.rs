//! Real SWIM data: per feed directory under `OPENATCA_SCDS`, concatenate every `*.msg`
//! (recursively, in name order), compress at levels 1, 6 and 9, verify each round trip, and
//! report ratio and speed against `gzip -6` and `xz -6` when those tools exist. Skips when the
//! variable is unset. The data is never copied into the repository.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod common;
use common::*;
use oa_zip::inflate;
use std::path::{Path, PathBuf};
use std::time::Instant;

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            collect(&p, out);
        } else if p.extension().is_some_and(|e| e == "msg") {
            out.push(p);
        }
    }
}

/// Seconds to compress `data` as one write at `level`, and the stream.
fn timed_deflate(level: u8, data: &[u8]) -> (f64, Vec<u8>) {
    let t = Instant::now();
    let z = deflate_chunked(level, data, 64 * 1024);
    (t.elapsed().as_secs_f64(), z)
}

// REQ: OA-DEF-009
#[test]
fn real_swim_feeds_compress_and_round_trip() {
    let Some(root) = std::env::var_os("OPENATCA_SCDS") else {
        eprintln!("OPENATCA_SCDS is not set: skipping the real SWIM data test");
        return;
    };
    let mut feeds: Vec<PathBuf> = std::fs::read_dir(&root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_dir())
        .collect();
    feeds.sort();
    assert!(
        !feeds.is_empty(),
        "no feed directories under {}",
        Path::new(&root).display()
    );
    // Debug builds are ~20x slower: cap the data there and do not judge speed.
    let release = !cfg!(debug_assertions);
    let cap = if release { usize::MAX } else { 2 << 20 };

    eprintln!(
        "{:<28} {:>9} | {:>7} {:>7} {:>7} | {:>7} {:>7} {:>7} | {:>7} {:>7}",
        "feed", "MB", "L1 x", "L6 x", "L9 x", "L1 MB/s", "L6 MB/s", "L9 MB/s", "gz6 x", "xz6 x"
    );
    let mut checked = 0;
    for feed in &feeds {
        let mut files = Vec::new();
        collect(feed, &mut files);
        if files.is_empty() {
            continue;
        }
        let mut data = Vec::new();
        for f in &files {
            data.extend(std::fs::read(f).unwrap());
            if data.len() >= cap {
                data.truncate(cap);
                break;
            }
        }
        let mb = data.len() as f64 / 1e6;
        let mut ratio = [0.0f64; 3];
        let mut speed = [0.0f64; 3];
        let mut size6 = 0usize;
        for (i, level) in [1u8, 6, 9].into_iter().enumerate() {
            let (secs, z) = timed_deflate(level, &data);
            let back = inflate(&z, data.len(), data.len() + 16).unwrap();
            assert!(
                back == data,
                "{}: level {level} round trip differs",
                feed.display()
            );
            ratio[i] = data.len() as f64 / z.len() as f64;
            speed[i] = mb / secs;
            if level == 6 {
                size6 = z.len();
            }
        }
        let gz = run_tool("gzip", &["-6", "-c"], &data).map(|o| o.stdout.len());
        let xz = run_tool("xz", &["-6", "-c"], &data).map(|o| o.stdout.len());
        let rat = |n: Option<usize>| {
            n.map_or("n/a".to_string(), |n| {
                format!("{:.2}", data.len() as f64 / n as f64)
            })
        };
        eprintln!(
            "{:<28} {:>9.1} | {:>7.2} {:>7.2} {:>7.2} | {:>7.1} {:>7.1} {:>7.1} | {:>7} {:>7}",
            feed.file_name().unwrap().to_string_lossy(),
            mb,
            ratio[0],
            ratio[1],
            ratio[2],
            speed[0],
            speed[1],
            speed[2],
            rat(gz),
            rat(xz)
        );
        if release {
            if let Some(gz) = gz {
                assert!(
                    (size6 as f64) <= gz as f64 * 1.10,
                    "{}: level 6 is {size6} bytes, more than 10% over gzip -6 ({gz})",
                    feed.display()
                );
            }
            // Speed is reported, not asserted: wall-clock timing depends on machine load and a
            // test must not fail because other jobs share the CPU. The 30 MB/s target is
            // checked by reading the printed table.
            if speed[1] < 30.0 {
                eprintln!(
                    "WARNING {}: level 6 ran at {:.1} MB/s, below the 30 MB/s target",
                    feed.display(),
                    speed[1]
                );
            }
        }
        checked += 1;
    }
    assert!(
        checked > 0,
        "no *.msg files under {}",
        Path::new(&root).display()
    );
}
