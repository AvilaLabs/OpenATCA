//! The `oa-replay-diff` binary on hostile inputs and a failing output (E3, E8).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::fs::{self, File};
use std::path::PathBuf;
use std::process::{Command, Stdio};

use oa_track::canon::{chain_next, hex32, DIGEST_HEADER};

fn dir(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("rdf_cli_{name}"));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn digest_with(rows: &[u64]) -> String {
    let (h, mut prev) = ([7u8; 32], [0u8; 32]);
    let mut t = format!("{DIGEST_HEADER}\n");
    let mut total = 0u64;
    for (i, r) in rows.iter().enumerate() {
        let s = i64::try_from(i).unwrap() + 1;
        prev = chain_next(&prev, s, &h, *r);
        total = total.saturating_add(*r);
        t.push_str(&format!("{s} {} {} {r}\n", hex32(&prev), hex32(&h)));
    }
    t.push_str(&format!("end {} {total} {}\n", rows.len(), hex32(&prev)));
    t
}

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_oa-replay-diff"))
}

// REQ: OA-RDF-008
#[test]
fn a_hostile_digest_is_exit_2_with_a_message_never_a_panic() {
    let d = dir("hostile");
    let (a, b) = (d.join("a.txt"), d.join("b.txt"));
    fs::write(&a, digest_with(&[u64::MAX, u64::MAX])).unwrap();
    fs::write(&b, digest_with(&[1])).unwrap();
    let o = bin().arg(&a).arg(&b).output().unwrap();
    assert_eq!(o.status.code(), Some(2), "{o:?}");
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("rows in a second"), "{err}");
    // A digest over the byte limit is refused before it is read.
    let big = d.join("big.txt");
    let f = File::create(&big).unwrap();
    f.set_len(oa_replay_diff::MAX_DIGEST_BYTES + 1).unwrap();
    let o = bin().arg(&big).arg(&b).output().unwrap();
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stderr).contains("limit"));
}

// REQ: OA-RDF-008
#[test]
fn a_failing_stdout_is_exit_2_not_a_panic() {
    let d = dir("fullpipe");
    let a = d.join("a.txt");
    fs::write(&a, digest_with(&[1, 2])).unwrap();
    // /dev/full fails every write, as a closed pipe does.
    let full = File::options().write(true).open("/dev/full").unwrap();
    let o = bin()
        .arg(&a)
        .arg(&a)
        .stdout(Stdio::from(full))
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(2), "{o:?}");
    assert!(String::from_utf8_lossy(&o.stderr).contains("cannot write"));
    // And the same inputs print normally and exit 0.
    let o = bin().arg(&a).arg(&a).output().unwrap();
    assert_eq!(o.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&o.stdout).contains("result: identical"));
}

// REQ: OA-RDF-018
#[test]
fn two_empty_digests_and_a_truncated_digest_are_exit_2() {
    let d = dir("empty");
    let (e, a) = (d.join("e.txt"), d.join("a.txt"));
    fs::write(&e, digest_with(&[])).unwrap();
    fs::write(&a, digest_with(&[1, 2])).unwrap();
    let o = bin().arg(&e).arg(&e).output().unwrap();
    assert_eq!(o.status.code(), Some(2), "{o:?}");
    assert!(String::from_utf8_lossy(&o.stderr).contains("no seconds to compare"));
    // One empty and one not is a difference, exit 1.
    assert_eq!(
        bin().arg(&e).arg(&a).output().unwrap().status.code(),
        Some(1)
    );
    // The same digest cut after its first data line has no end line.
    let text = digest_with(&[1, 2]);
    let cut: String = text.lines().take(2).map(|l| format!("{l}\n")).collect();
    let c = d.join("c.txt");
    fs::write(&c, cut).unwrap();
    let o = bin().arg(&c).arg(&a).output().unwrap();
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stderr).contains("end line"));
}

// REQ: OA-RDF-021
#[test]
fn a_repeated_option_is_a_usage_error() {
    let d = dir("repeat");
    let a = d.join("a.txt");
    fs::write(&a, digest_with(&[1])).unwrap();
    for opt in [
        "--summary",
        "--expect",
        "--rows-a",
        "--rows-b",
        "--max-detail",
    ] {
        let o = bin()
            .arg(&a)
            .arg(&a)
            .args([opt, "5", opt, "6"])
            .output()
            .unwrap();
        assert_eq!(o.status.code(), Some(2), "{opt}");
        assert!(
            String::from_utf8_lossy(&o.stderr).contains("given twice"),
            "{opt}"
        );
    }
}

// REQ: OA-SEAL-005
#[test]
fn a_sealed_path_is_refused_with_exit_2_naming_the_rule_and_nothing_is_written() {
    let d = dir("sealed");
    let digest = d.join("a.txt");
    fs::write(&digest, digest_with(&[1])).unwrap();
    // A fake path that only names a sealed folder; nothing real is touched.
    let fake = "/nonexistent-fake/scds/rec-20261119/day2/digest.txt";
    let summary = d.join("summary.txt");
    for args in [
        vec![fake.to_string(), digest.display().to_string()],
        vec![
            digest.display().to_string(),
            digest.display().to_string(),
            "--summary".into(),
            "/nonexistent-fake/scds/stdds-20261119-holdout/s.txt".into(),
        ],
        vec![
            digest.display().to_string(),
            digest.display().to_string(),
            "--rows-a".into(),
            fake.into(),
            "--rows-b".into(),
            fake.into(),
            "--summary".into(),
            summary.display().to_string(),
        ],
    ] {
        let out = bin().args(&args).output().unwrap();
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(String::from_utf8_lossy(&out.stderr).contains("OA-SEAL-001"));
    }
    assert!(!summary.exists());
}

/// A digest with one line per `(second, rows)`.
fn digest_at(lines: &[(i64, u64)]) -> String {
    let (h, mut prev) = ([7u8; 32], [0u8; 32]);
    let mut t = format!("{DIGEST_HEADER}\n");
    let mut total = 0u64;
    for (s, r) in lines {
        prev = chain_next(&prev, *s, &h, *r);
        total += r;
        t.push_str(&format!("{s} {} {} {r}\n", hex32(&prev), hex32(&h)));
    }
    t.push_str(&format!("end {} {total} {}\n", lines.len(), hex32(&prev)));
    t
}

// The first second of the sealed window (the listed window widened by 15 minutes).
const WINDOW_SECOND: i64 = 1_795_131_900;

// REQ: OA-SEAL-014
#[test]
fn an_input_with_a_second_in_the_sealed_window_is_refused_with_a_count_and_no_differences() {
    let d = dir("sealed_second");
    let (a, b, ok) = (d.join("a.txt"), d.join("b.txt"), d.join("ok.txt"));
    fs::write(&a, digest_at(&[(WINDOW_SECOND, 1), (WINDOW_SECOND + 5, 1)])).unwrap();
    fs::write(&b, digest_at(&[(WINDOW_SECOND, 2)])).unwrap();
    fs::write(&ok, digest_at(&[(100, 1)])).unwrap();
    // In either position, a differing pair and an identical one.
    for (x, y) in [(&a, &b), (&ok, &b), (&a, &a), (&b, &ok)] {
        let o = bin().arg(x).arg(y).output().unwrap();
        assert_eq!(o.status.code(), Some(2), "{o:?}");
        assert!(
            o.stdout.is_empty(),
            "{}",
            String::from_utf8_lossy(&o.stdout)
        );
        let err = String::from_utf8_lossy(&o.stderr);
        assert!(
            err.contains("digest seconds are in the sealed window"),
            "{err}"
        );
        assert!(err.contains("sealed-window rows dropped:"), "{err}");
        assert!(!err.contains("sealed-window rows dropped: 0"), "{err}");
    }
    // The second just before the window is read, and the drop line is printed with 0.
    let (c, e) = (d.join("c.txt"), d.join("e.txt"));
    fs::write(&c, digest_at(&[(WINDOW_SECOND - 1, 1)])).unwrap();
    fs::write(&e, digest_at(&[(WINDOW_SECOND - 1, 1)])).unwrap();
    let o = bin().arg(&c).arg(&e).output().unwrap();
    assert_eq!(o.status.code(), Some(0), "{o:?}");
    assert!(String::from_utf8_lossy(&o.stderr).contains("sealed-window rows dropped: 0"));
}

// REQ: OA-SEAL-014
#[test]
fn a_rows_file_with_a_row_in_the_sealed_window_is_refused_with_a_count_and_no_differences() {
    use oa_track::canon::{canon_row, rows_header};
    use oa_track::OutRow;
    let row = |t_ns: i64| {
        canon_row(&OutRow {
            t_ns,
            addr: 1,
            lat_e7: 1,
            lon_e7: 2,
            vel_east_mps: 0.0,
            vel_north_mps: 0.0,
            pos_cov_m2: [0.0; 3],
            vel_cov_m2s2: [0.0; 3],
            turn_rate_rps: 0.0,
            alt_ft: 0.0,
            alt_var_ft2: 0.0,
            vrate_fpm: 0.0,
            mu_h: [0.0; 2],
            mu_v: [0.0; 2],
            flags: 0,
        })
    };
    let d = dir("sealed_row");
    let (a, b) = (d.join("a.txt"), d.join("b.txt"));
    fs::write(&a, digest_at(&[(100, 1)])).unwrap();
    fs::write(&b, digest_at(&[(100, 1)])).unwrap();
    let rows = |name: &str, t_ns: i64| {
        let p = d.join(name);
        let mut bytes = rows_header(1).to_vec();
        bytes.extend_from_slice(&row(t_ns));
        fs::write(&p, bytes).unwrap();
        p
    };
    // The digest second is clean; the row's own time is in the window.
    let (ra, rb) = (
        rows("ra.bin", 100_000_000_000),
        rows("rb.bin", (WINDOW_SECOND + 1) * 1_000_000_000),
    );
    let o = bin()
        .arg(&a)
        .arg(&b)
        .arg("--rows-a")
        .arg(&ra)
        .arg("--rows-b")
        .arg(&rb)
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(2), "{o:?}");
    assert!(
        o.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&o.stdout)
    );
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("1 rows are in the sealed window"), "{err}");
    assert!(err.contains("sealed-window rows dropped: 1"), "{err}");
}

fn replay_witness(settings: u8, input: u8, state: u8, frames: u64) -> String {
    format!(
        "settings_fingerprint version=1 sha256={} note=settings_and_ordered_feed_stream_kinds_only\ninput_fingerprint version=1 status=complete pipeline_frames={frames} pipeline_payload_bytes=240 sha256={} source_rejected=0 source_sealed_drops=0 semantic_sealed_drops=0 semantic_errors=0 bridge_reject_frames=0 note=diagnostic_only_not_completeness_or_cross_clock_equivalence\nfinal state_digest={}\n",
        oa_track::canon::hex32(&[settings; 32]),
        oa_track::canon::hex32(&[input; 32]),
        oa_track::canon::hex32(&[state; 32]),
    )
}

fn compare_witness_texts(a_text: &str, b_text: &str, label: &str) -> std::process::Output {
    let d = dir(label);
    let (a, b) = (d.join("a.witness"), d.join("b.witness"));
    fs::write(&a, a_text).unwrap();
    fs::write(&b, b_text).unwrap();
    bin()
        .args(["--compare-witness-files"])
        .arg(a)
        .arg(b)
        .output()
        .unwrap()
}

// REQ: OA-RDF-024
#[test]
fn witness_mode_compares_settings_then_input_then_state() {
    let base = replay_witness(1, 2, 3, 5);
    let cases = [
        (base.clone(), base.clone(), 0, "MATCH"),
        (
            base.clone(),
            replay_witness(4, 2, 3, 5),
            1,
            "SETTINGS MISMATCH",
        ),
        (
            base.clone(),
            replay_witness(1, 4, 3, 5),
            1,
            "INPUT MISMATCH",
        ),
        (
            base.clone(),
            replay_witness(1, 2, 4, 5),
            1,
            "STATE MISMATCH",
        ),
        (
            base.clone(),
            replay_witness(1, 2, 3, 6),
            1,
            "INPUT MISMATCH",
        ),
    ];
    for (index, (a, b, code, expected)) in cases.into_iter().enumerate() {
        let out = compare_witness_texts(&a, &b, &format!("witness_compare_{index}"));
        assert_eq!(out.status.code(), Some(code), "{out:?}");
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), expected);
    }
}

// REQ: OA-RDF-024
#[test]
fn witness_mode_refuses_partial_duplicate_and_nonzero_refusal_evidence() {
    let valid = replay_witness(1, 2, 3, 5);
    let partial = valid.replace("status=complete", "status=incomplete");
    let duplicate = format!("{valid}{}", valid.lines().next().unwrap());
    let nonzero = valid.replace("source_rejected=0", "source_rejected=1");
    for (index, invalid) in [partial, duplicate, nonzero].into_iter().enumerate() {
        let out = compare_witness_texts(&invalid, &valid, &format!("witness_invalid_{index}"));
        assert_eq!(out.status.code(), Some(2), "{out:?}");
        assert!(out.stdout.is_empty(), "{out:?}");
    }
}

// REQ: OA-SEAL-005, OA-RDF-024
#[test]
fn witness_mode_checks_sealed_paths_and_enforces_its_read_cap() {
    let d = dir("witness_limits");
    let valid = d.join("valid.witness");
    fs::write(&valid, replay_witness(1, 2, 3, 5)).unwrap();
    let fake = "/nonexistent-fake/scds/rec-20261119/day2/witness.txt";
    let out = bin()
        .args(["--compare-witness-files", fake, valid.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("OA-SEAL-001"));
    let oversized = d.join("oversized.witness");
    let file = File::create(&oversized).unwrap();
    file.set_len(oa_replay_diff::MAX_WITNESS_BYTES + 1).unwrap();
    let out = bin()
        .args(["--compare-witness-files"])
        .arg(&oversized)
        .arg(&valid)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("limit"));
}
