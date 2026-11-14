//! The `oa-wind-check adsb` mode as a program: argument handling, the skip when there is no RAP
//! data, and (when `OPENATCA_DATA` is present) a run on the real files.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::process::Command;

fn run(args: &[&str]) -> (Option<i32>, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_oa-wind-check"))
        .args(args)
        .output()
        .unwrap();
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

// REQ: OA-WIND-017
#[test]
fn the_adsb_mode_checks_its_arguments_skips_without_data_and_runs_on_real_data() {
    // A bad gap is an argument error.
    let (code, _, err) = run(&["adsb", "/nonexistent", "--gap-s", "soon"]);
    assert_eq!(code, Some(2), "{err}");
    let (code, _, _) = run(&["adsb", "/nonexistent", "--gap-s", "-1"]);
    assert_eq!(code, Some(2));
    // No RAP files: a note and success, not a failure.
    let (code, out, _) = run(&["adsb", "/nonexistent"]);
    assert_eq!(code, Some(0));
    assert!(out.contains("no RAP files"), "{out}");
    // Real data: either a comparison or the statement that no day overlaps.
    let Some(root) = std::env::var_os("OPENATCA_DATA") else {
        println!("OPENATCA_DATA not set: real-data part skipped");
        return;
    };
    if !std::path::Path::new(&root).join("raw/rap").is_dir() {
        println!("no raw/rap: real-data part skipped");
        return;
    }
    let root = root.to_string_lossy().into_owned();
    let (code, out, err) = run(&["adsb", &root]);
    assert_eq!(code, Some(0), "{err}");
    assert!(out.starts_with("RAP: "), "{out}");
    assert!(
        out.contains("does not overlap")
            || out.contains("no adsb.lol day overlaps")
            || out.contains("compared "),
        "{out}"
    );
}
