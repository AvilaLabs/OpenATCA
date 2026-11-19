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
    for (i, r) in rows.iter().enumerate() {
        let s = i64::try_from(i).unwrap() + 1;
        prev = chain_next(&prev, s, &h, *r);
        t.push_str(&format!("{s} {} {} {r}\n", hex32(&prev), hex32(&h)));
    }
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
