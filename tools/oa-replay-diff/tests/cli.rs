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
