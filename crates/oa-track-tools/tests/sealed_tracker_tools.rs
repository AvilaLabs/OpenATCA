//! Recording CLI shells refuse sealed inputs/outputs (OA-SEAL-005).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::{Path, PathBuf};
use std::process::Command;

const CLIP: &str = "/nonexistent-fake/scds/rec-20261119/day2/clip.oaf";

fn out_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("sealed_{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn refused(mut command: Command, out: &Path) {
    let result = command.output().unwrap();
    assert_eq!(result.status.code(), Some(2), "{command:?}");
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("OA-SEAL-001"),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(!out.exists(), "{}", out.display());
}

// REQ: OA-SEAL-005
#[test]
fn oa_track_run_refuses_sealed_input_output_and_rows_paths() {
    let bin = env!("CARGO_BIN_EXE_oa-track-run");
    let out = out_dir("run");
    let mut command = Command::new(bin);
    command.args(["--adsb", CLIP, "--out"]).arg(&out);
    refused(command, &out);

    let mut command = Command::new(bin);
    command.args([
        "--adsb",
        "/nonexistent-fake/x.oaf",
        "--out",
        "/nonexistent-fake/scds/stdds-20261119-holdout/o",
    ]);
    refused(command, &out);

    let mut command = Command::new(bin);
    command
        .args(["--adsb", "/nonexistent-fake/x.oaf", "--no-score", "--out"])
        .arg(&out)
        .args(["--rows", CLIP]);
    refused(command, &out);
}

// REQ: OA-SEAL-005
#[test]
fn oa_track_fusion_refuses_a_sealed_clip() {
    let out = out_dir("fusion");
    let mut command = Command::new(env!("CARGO_BIN_EXE_oa-track-fusion"));
    command.args(["--quick", "--clip", CLIP, "--out"]).arg(&out);
    refused(command, &out);
}

// REQ: OA-SEAL-005
#[test]
fn oa_track_run_refuses_a_symlink_into_sealed_data() {
    use std::os::unix::fs::symlink;

    let root = out_dir("run_link");
    let sealed = root.join("scds/rec-20261119/day2");
    std::fs::create_dir_all(&sealed).unwrap();
    std::fs::write(sealed.join("clip.oaf"), b"synthetic fixture only").unwrap();
    let linked_input = root.join("clip.oaf");
    symlink(sealed.join("clip.oaf"), &linked_input).unwrap();
    let out = root.join("out");

    let mut command = Command::new(env!("CARGO_BIN_EXE_oa-track-run"));
    command
        .args(["--adsb"])
        .arg(linked_input)
        .arg("--out")
        .arg(&out);
    refused(command, &out);
}
