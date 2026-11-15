//! The `oa-truth-check` binary on a synthetic OAF1 clip.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;
use std::process::Command;

use oa_adsb::{encode_payload, reports_len, reports_push, Reports, ROW_EMPTY};
use oa_feed::{encode_frame, FLAG_BYTES, MAX_FRAME_LEN};
use oa_sha256::{to_hex, Sha256};
use oa_truth::{build_truth, truth_hash_into, DEFAULT_CONFIG};

fn tmp(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name)
}

fn track() -> Reports {
    let mut r = Reports::default();
    for addr in [0xa00001u32, 0xa00002] {
        for k in 0..150 {
            let t = f64::from(k) * 2.0;
            let mut row = ROW_EMPTY;
            row.icao = addr;
            row.time_ns = 1_700_000_000_000_000_000 + (t * 1e9) as i64;
            row.lat_e7 = 410_000_000 + (t * 1000.0) as i32 + (k % 3) * 20;
            row.lon_e7 = -880_000_000 + (t * 1200.0) as i32 - (k % 2) * 25;
            row.nic = 8;
            row.nac_p = 10;
            row.sil = 3;
            row.version = 2;
            row.source = 0;
            row.baro_alt_ft = 30_000;
            reports_push(&mut r, &row);
        }
    }
    r
}

// REQ: OA-TRUTH-016
#[test]
fn check_binary_prints_the_summary_and_the_library_digest() {
    let r = track();
    let n = reports_len(&r);
    let mut bytes = Vec::new();
    let mut seq = 0u64;
    let mut at = 0;
    while at < n {
        let end = (at + 100).min(n);
        let mut payload = Vec::new();
        encode_payload(&mut payload, &r, at, end).unwrap();
        let mut buf = vec![0u8; MAX_FRAME_LEN];
        let len = encode_frame(&mut buf, 1, FLAG_BYTES, seq, 0, 0, &payload).unwrap();
        bytes.extend_from_slice(&buf[..len]);
        seq += 1;
        at = end;
    }
    let path = tmp("truth_check.oaf");
    std::fs::write(&path, &bytes).unwrap();
    let exe = env!("CARGO_BIN_EXE_oa-truth-check");
    let o = Command::new(exe).arg(&path).output().unwrap();
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(
        o.status.success(),
        "{out} {}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(
        out.contains("300 reports decoded, 0 frames refused"),
        "{out}"
    );
    assert!(out.contains("legs               2"), "{out}");
    assert!(
        out.contains("leave-one-out: every 5-th report hidden"),
        "{out}"
    );
    assert!(out.contains("cross-track"), "{out}");
    assert!(out.contains("kept + removed = 300, rows in = 300"), "{out}");
    // The digest the binary prints is the library's for the same reports.
    let (truth, _) = build_truth(&r, &DEFAULT_CONFIG).unwrap();
    let mut h = Sha256::new();
    truth_hash_into(&mut h, &truth);
    assert!(
        out.contains(&format!("truth digest {}", to_hex(&h.finalize()))),
        "{out}"
    );
    // The tuning options are honoured (a different model gives a different digest).
    let o = Command::new(exe)
        .arg(&path)
        .args(["--accel-q", "1", "--no-loo"])
        .output()
        .unwrap();
    let out2 = String::from_utf8_lossy(&o.stdout);
    assert!(o.status.success() && out2.contains("0 hidden"), "{out2}");
    assert!(!out2.contains(&format!(
        "truth digest {}",
        to_hex(&{
            let mut h = Sha256::new();
            truth_hash_into(&mut h, &truth);
            h.finalize()
        })
    )));
    // Usage errors and unreadable input exit 2.
    for args in [vec![], vec!["--bogus"], vec!["/nonexistent/clip.oaf"]] {
        let o = Command::new(exe).args(&args).output().unwrap();
        assert_eq!(o.status.code(), Some(2), "{args:?}");
    }
}

// REQ: OA-TRUTH-023
#[test]
fn a_clip_that_ends_inside_a_frame_keeps_its_valid_prefix_and_counts_the_partial_frame() {
    let r = track();
    let n = reports_len(&r);
    let frame = |from: usize, to: usize, seq: u64| {
        let mut payload = Vec::new();
        encode_payload(&mut payload, &r, from, to).unwrap();
        let mut buf = vec![0u8; MAX_FRAME_LEN];
        let len = encode_frame(&mut buf, 1, FLAG_BYTES, seq, 0, 0, &payload).unwrap();
        buf[..len].to_vec()
    };
    let mut whole = frame(0, n / 2, 0);
    whole.extend(frame(n / 2, n, 1));
    let extra = frame(0, 5, 2);
    let exe = env!("CARGO_BIN_EXE_oa-truth-check");
    for (name, cut, bad) in [("whole", 0, 0), ("header", 10, 1), ("body", 60, 1)] {
        let mut bytes = whole.clone();
        bytes.extend_from_slice(&extra[..cut]);
        let path = tmp(&format!("truth_tail_{name}.oaf"));
        std::fs::write(&path, &bytes).unwrap();
        let o = Command::new(exe)
            .arg(&path)
            .arg("--no-loo")
            .output()
            .unwrap();
        let out = String::from_utf8_lossy(&o.stdout);
        assert!(o.status.success(), "{name}: {out}");
        assert!(
            out.contains(&format!("300 reports decoded, {bad} frames refused")),
            "{name}: {out}"
        );
    }
}
