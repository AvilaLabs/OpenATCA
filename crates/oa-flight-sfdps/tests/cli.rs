//! The binary refuses sealed hold-out data before it opens or lists anything.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::process::Command;

fn run(dir: &str) -> (Option<i32>, String) {
    // seal-allow: runs the built binary of this package on the test's own temp files
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
    let (code, err) = run("/nonexistent/openatca-data/scds/day-20261114/rec/sfdps");
    assert_eq!(code, Some(1));
    // The line is printed on this exit too.
    assert!(err.contains("sealed-window rows dropped: 0"), "{err}");
}

// REQ: OA-SEAL-014
#[test]
fn a_refusal_prints_the_drop_line_and_a_stop_at_to_counts_the_frames_the_merge_dropped() {
    let (_, err) = run("/nonexistent/openatca-data/scds/stdds-20261119-holdout");
    assert!(err.contains("sealed-window rows dropped: 0"), "{err}");
    // The list's window is 2026-11-19T23:45:00Z to 2026-11-23T00:32:00Z (the listed window widened by 15 minutes). Frames received 2 s before
    // it, in it, and 1 s after it; `--to` ends the run just before the window, so the run
    // stops at the third frame, and the frame in the window is still counted.
    const FROM: i64 = 1_795_131_900_000_000_000;
    const TO: i64 = 1_795_393_920_000_000_000;
    let xml =
        r#"<?xml version="1.0"?><ns5:MessageCollection xmlns:ns5="http://www.faa.aero/nas/3.0"/>"#;
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("flight-to-cut");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut bytes = Vec::new();
    for (i, t) in [FROM - 2_000_000_000, FROM, TO + 1_000_000_000]
        .into_iter()
        .enumerate()
    {
        let mut buf = vec![0u8; xml.len() + 64];
        let n = oa_feed::encode_frame(&mut buf, 1, 0, i as u64, t, 0, xml.as_bytes()).unwrap();
        bytes.extend_from_slice(&buf[..n]);
    }
    std::fs::write(dir.join("a.oaf"), bytes).unwrap();
    // seal-allow: runs the built binary of this package on the test's own temp files
    let o = Command::new(env!("CARGO_BIN_EXE_oa-flight"))
        .arg(&dir)
        .args(["--to", &(FROM - 1).to_string()])
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(o.status.success(), "{err}");
    assert!(err.contains("sealed-window rows dropped: 1"), "{err}");
    let report = String::from_utf8_lossy(&o.stdout);
    assert!(report.contains("frame_and_gzip_errors=0 sealed_frames=1 sealed_message_rows=0"));
}

// REQ: OA-SEAL-014
#[test]
fn a_file_that_cannot_be_opened_partway_through_the_list_keeps_the_count_of_the_files_read() {
    const FROM: i64 = 1_795_131_900_000_000_000;
    let xml =
        r#"<?xml version="1.0"?><ns5:MessageCollection xmlns:ns5="http://www.faa.aero/nas/3.0"/>"#;
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("flight-open-fails");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // The first file holds one frame in the window; the second is listed and cannot be opened
    // (no permission) once the first has been read.
    let mut buf = vec![0u8; xml.len() + 64];
    let n = oa_feed::encode_frame(&mut buf, 1, 0, 0, FROM, 0, xml.as_bytes()).unwrap();
    std::fs::write(dir.join("a.oaf"), &buf[..n]).unwrap();
    std::fs::write(dir.join("b.oaf"), b"x").unwrap();
    std::fs::set_permissions(
        dir.join("b.oaf"),
        std::os::unix::fs::PermissionsExt::from_mode(0o000),
    )
    .unwrap();
    // seal-allow: runs the built binary of this package on the test's own temp files
    let o = Command::new(env!("CARGO_BIN_EXE_oa-flight"))
        .arg(&dir)
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&o.stderr);
    assert_eq!(o.status.code(), Some(1), "{err}");
    assert!(err.contains("sealed-window rows dropped: 1"), "{err}");
}
