//! The `oa-clip-sealcheck` binary on synthetic OAF1 clips (no real data).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;
use std::process::Command;

use oa_adsb::{encode_payload, reports_push, Reports, ROW_EMPTY};
use oa_feed::{encode_frame, FLAG_BYTES, MAX_FRAME_LEN};

// The sealed window of the list, widened by 15 minutes (crates/oa-sealed/sealed_paths.txt).
const FROM: i64 = 1_795_131_900_000_000_000;
const TO: i64 = 1_795_393_920_000_000_000;

fn clip(name: &str, recv: i64, row: i64) -> PathBuf {
    let mut r = Reports::default();
    let mut x = ROW_EMPTY;
    x.icao = 0xA00001;
    x.time_ns = row;
    x.lat_e7 = 410_000_000;
    x.lon_e7 = -880_000_000;
    reports_push(&mut r, &x);
    let mut payload = Vec::new();
    encode_payload(&mut payload, &r, 0, 1).unwrap();
    let mut buf = vec![0u8; MAX_FRAME_LEN];
    let n = encode_frame(&mut buf, 1, FLAG_BYTES, 0, recv, 0, &payload).unwrap();
    let p = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::write(&p, &buf[..n]).unwrap();
    p
}

fn check(p: &PathBuf) -> (Option<i32>, String) {
    let o = Command::new(env!("CARGO_BIN_EXE_oa-clip-sealcheck"))
        .arg(p)
        .output()
        .unwrap();
    (
        o.status.code(),
        String::from_utf8_lossy(&o.stdout).into_owned(),
    )
}

// REQ: OA-SEAL-014
#[test]
fn the_sealcheck_binary_exits_zero_only_for_a_clip_with_nothing_in_the_window() {
    let before = FROM - 3_600_000_000_000;
    let (code, out) = check(&clip("seal_clean.oaf", before, before));
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("1 frames, 1 rows"), "{out}");
    // A row in the window.
    let (code, out) = check(&clip("seal_row.oaf", before, FROM + 1));
    assert_eq!(code, Some(1), "{out}");
    assert!(out.contains("1 rows;"), "{out}");
    // A frame received in the window, its row outside it.
    let (code, _) = check(&clip("seal_recv.oaf", TO - 1, before));
    assert_eq!(code, Some(1));
    // A file that is no clip, and a path that does not exist.
    let junk = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("seal_junk.oaf");
    std::fs::write(&junk, b"not a clip").unwrap();
    assert_eq!(check(&junk).0, Some(2));
    assert_eq!(check(&junk.with_extension("missing")).0, Some(2));
}
