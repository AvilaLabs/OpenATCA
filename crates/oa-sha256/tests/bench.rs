//! Throughput report. Release builds only; prints MB/s and asserts nothing, since timing depends
//! on machine load.
#![cfg(not(debug_assertions))]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use oa_sha256::sha256;
use std::hint::black_box;
use std::time::Instant;

/// Best-of-N MB/s (10^6 bytes per second) for hashing `data`, repeated to ~0.3 s per trial.
fn mbps(data: &[u8]) -> f64 {
    let reps = (64 * 1024 * 1024 / data.len()).max(1);
    let mut best: f64 = 0.0;
    for _ in 0..5 {
        let t = Instant::now();
        for _ in 0..reps {
            black_box(sha256(black_box(data)));
        }
        let rate = (data.len() * reps) as f64 / t.elapsed().as_secs_f64() / 1e6;
        best = best.max(rate);
    }
    best
}

// REQ: OA-SHA-004
#[test]
fn report_throughput() {
    let mut x = 0x1234_5678_9abc_def0u64;
    let big: Vec<u8> = (0..8 * 1024 * 1024)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect();
    for (name, len) in [
        ("1 KiB", 1024),
        ("64 KiB", 64 * 1024),
        ("8 MiB", 8 * 1024 * 1024),
    ] {
        let data = big.get(..len).unwrap();
        eprintln!("oa-sha256 {name}: {:.0} MB/s", mbps(data));
    }

    let path = std::env::temp_dir().join(format!("oa-sha256-bench-{}.bin", std::process::id()));
    std::fs::write(&path, &big).unwrap();
    let mut best: f64 = 0.0;
    let mut ran = false;
    for _ in 0..5 {
        let t = Instant::now();
        let out = std::process::Command::new("sha256sum").arg(&path).output();
        let dt = t.elapsed().as_secs_f64();
        match out {
            Ok(o) if o.status.success() => {
                ran = true;
                best = best.max(big.len() as f64 / dt / 1e6);
            }
            _ => break,
        }
    }
    let _ = std::fs::remove_file(&path);
    if ran {
        eprintln!("sha256sum 8 MiB (incl. process start): {best:.0} MB/s");
    } else {
        eprintln!("sha256sum not available");
    }
}
