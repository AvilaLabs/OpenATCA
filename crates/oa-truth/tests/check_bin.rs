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

// REQ: OA-TRUTH-042
#[test]
fn a_non_finite_or_non_positive_tuning_value_is_a_usage_error() {
    let exe = env!("CARGO_BIN_EXE_oa-truth-check");
    for opt in ["--accel-q", "--sigma-scale"] {
        for v in ["nan", "inf", "-inf", "1e999", "0", "-1", "x"] {
            // The clip does not exist: a usage error comes before it is read.
            let o = Command::new(exe)
                .args(["/nonexistent/clip.oaf", opt, v])
                .output()
                .unwrap();
            let err = String::from_utf8_lossy(&o.stderr);
            assert_eq!(o.status.code(), Some(2), "{opt} {v}");
            assert!(err.contains("usage:"), "{opt} {v}: {err}");
        }
    }
}

/// Writes `r` as an OAF1 clip of 100-report frames.
fn write_clip(r: &Reports, name: &str) -> PathBuf {
    let n = reports_len(r);
    let mut bytes = Vec::new();
    let (mut seq, mut at) = (0u64, 0);
    while at < n {
        let end = (at + 100).min(n);
        let mut payload = Vec::new();
        encode_payload(&mut payload, r, at, end).unwrap();
        let mut buf = vec![0u8; MAX_FRAME_LEN];
        let len = encode_frame(&mut buf, 1, FLAG_BYTES, seq, 0, 0, &payload).unwrap();
        bytes.extend_from_slice(&buf[..len]);
        seq += 1;
        at = end;
    }
    let path = tmp(name);
    std::fs::write(&path, &bytes).unwrap();
    path
}

fn run(path: &PathBuf, extra: &[&str]) -> (String, String, Option<i32>) {
    let o = Command::new(env!("CARGO_BIN_EXE_oa-truth-check"))
        .arg(path)
        .args(extra)
        .output()
        .unwrap();
    (
        String::from_utf8_lossy(&o.stdout).into_owned(),
        String::from_utf8_lossy(&o.stderr).into_owned(),
        o.status.code(),
    )
}

// REQ: OA-TRUTH-059
#[test]
fn check_binary_prints_the_removal_total_and_the_shares_it_computed() {
    // 300 good reports plus three that fail NACp (7 < 8), on the first aircraft after its track.
    let mut r = track();
    for k in 0..3 {
        let mut row = ROW_EMPTY;
        row.icao = 0xa00001;
        row.time_ns = 1_700_000_000_000_000_000 + (400 + 2 * k) * 1_000_000_000;
        row.lat_e7 = 410_000_000;
        row.lon_e7 = -880_000_000;
        row.nic = 8;
        row.nac_p = 7;
        row.sil = 3;
        row.version = 2;
        row.source = 0;
        reports_push(&mut r, &row);
    }
    let (out, err, code) = run(&write_clip(&r, "truth_check_removed.oaf"), &[]);
    assert_eq!(code, Some(0), "{out} {err}");
    // `{:<20}` pads the name to 20 columns, then one space and the count.
    let line = |name: &str, tail: &str| format!("  {name:<20} {tail}");
    assert!(out.contains(&line("nacp", "3\n")), "{out}");
    assert!(
        out.contains(&line(
            "total",
            "3   (kept + removed = 303, rows in = 303)\n"
        )),
        "{out}"
    );
    assert!(out.contains("rows kept          300\n"), "{out}");
    // Every report and every interval of the 2 s track is far inside the 50 m gate.
    assert!(
        out.contains("at report times:      100.00 % (300 of 300)\n"),
        "{out}"
    );
    assert!(
        out.contains("at interval midpoints: 100.00 % (298 of 298)\n"),
        "{out}"
    );
    assert!(!out.contains("records dropped"), "{out}");
    // Without leave-one-out there is nothing to take a share of.
    let (out, _, _) = run(&write_clip(&r, "truth_check_removed.oaf"), &["--no-loo"]);
    assert!(out.contains("at hidden times: n/a\n"), "{out}");
    assert!(
        out.contains("(calibration, ~95 % if honest): n/a\n"),
        "{out}"
    );
}

// REQ: OA-TRUTH-059
#[test]
fn check_binary_reports_records_dropped_from_frames_that_decoded() {
    let mut r = track();
    for k in 0..2 {
        let mut row = ROW_EMPTY;
        row.icao = 0xa00001;
        row.time_ns = 1_700_000_000_000_000_000 + (400 + 2 * k) * 1_000_000_000;
        row.lat_e7 = 910_000_000;
        row.lon_e7 = -880_000_000;
        reports_push(&mut r, &row);
    }
    let (out, err, code) = run(&write_clip(&r, "truth_check_dropped.oaf"), &["--no-loo"]);
    assert_eq!(code, Some(0), "{out} {err}");
    assert!(
        out.contains("300 reports decoded, 0 frames refused\n"),
        "{out}"
    );
    assert!(
        out.contains(
            "2 records dropped from frames that decoded (position or flags out of range)\n"
        ),
        "{out}"
    );
}

// REQ: OA-TRUTH-059
#[test]
fn check_binary_refuses_an_unknown_option_and_a_second_path_with_the_usage_text() {
    let path = write_clip(&track(), "truth_check_usage.oaf");
    for extra in [&["--bogus"][..], &["--accel-q"], &["other.oaf"]] {
        let (_, err, code) = run(&path, extra);
        assert_eq!(code, Some(2), "{extra:?}");
        assert!(
            err.contains("oa-truth-check: usage: oa-truth-check <clip.oaf> [--accel-q <v>] [--sigma-scale <v>] [--no-loo]"),
            "{extra:?}: {err}"
        );
    }
    // An unknown option on its own is not taken for the clip path.
    let o = Command::new(env!("CARGO_BIN_EXE_oa-truth-check"))
        .arg("--bogus")
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&o.stderr).starts_with("oa-truth-check: usage: oa-truth-check "),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
}
