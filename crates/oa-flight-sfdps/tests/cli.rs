//! The binary refuses sealed hold-out data before it opens or lists anything.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::process::Command;

fn run(dir: &str) -> (Option<i32>, String) {
    let o = Command::new(env!("CARGO_BIN_EXE_oa-flight"))
        .arg(dir)
        .output()
        .unwrap();
    (
        o.status.code(),
        String::from_utf8_lossy(&o.stderr).into_owned(),
    )
}

// REQ: OA-FLT-023
#[test]
fn a_sealed_recording_directory_is_refused_with_exit_2() {
    for d in [
        "/nonexistent/openatca-data/scds/rec-20261119/day2-a/rec/sfdps",
        "/nonexistent/openatca-data/scds/day-20261114/../rec-20261119/day3-b",
        "/nonexistent/openatca-data/scds/stdds-20261119-holdout",
        "/nonexistent/openatca-data/scds/stdds-smoke-20261119T192936Z",
    ] {
        let (code, err) = run(d);
        assert_eq!(code, Some(2), "{d}: {err}");
        assert!(err.contains("sealed"), "{err}");
    }
}

// REQ: OA-FLT-023
#[test]
fn an_unsealed_missing_directory_is_a_failed_run_not_a_refusal() {
    let (code, _) = run("/nonexistent/openatca-data/scds/day-20261114/rec/sfdps");
    assert_eq!(code, Some(1));
}
