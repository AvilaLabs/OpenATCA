//! The `survey` example drops a file with a message in the sealed window whole and prints the
//! count (OA-SEAL-014). The example is run as built by cargo for this package.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

fn message(ts: &str) -> String {
    format!(
        r#"<ns5:MessageCollection xmlns:ns5="http://www.faa.aero/nas/3.0" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"><message xsi:type="ns5:FlightMessageType"><flight centre="ZXX" source="TH" system="SLC" timestamp="{ts}"><gufi codeSpace="urn:uuid">aaaaaaaa-bbbb-4ccc-8ddd-000000000001</gufi></flight></message></ns5:MessageCollection>"#
    )
}

fn example() -> std::path::PathBuf {
    // target/<profile>/deps/<test> -> target/<profile>/examples/survey
    let exe = std::env::current_exe().unwrap();
    exe.parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("examples")
        .join("survey")
}

// REQ: OA-SEAL-014
#[test]
fn the_survey_drops_a_file_with_a_message_in_the_window_whole_and_prints_the_count() {
    let dir = std::env::temp_dir().join(format!("oa-fixm-survey-seal-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.msg"), message("2030-01-02T03:04:05.000Z")).unwrap();
    std::fs::write(dir.join("b.msg"), message("2026-11-21T00:00:00.000Z")).unwrap();
    let exe = example();
    assert!(exe.exists(), "{}", exe.display());
    let out = std::process::Command::new(exe).arg(&dir).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{text} {}",
        String::from_utf8_lossy(&out.stderr)
    );
    // Only the file outside the window is in the counts, files included; the dropped file is
    // in the drop line.
    assert!(text.contains("files 1\n"), "{text}");
    assert!(text.contains("messages per file {1: 1}"), "{text}");
    // The line is printed on stderr, when the run ends.
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.trim_end().ends_with("sealed-window rows dropped: 1"),
        "{err}"
    );
    std::fs::remove_dir_all(&dir).ok();
}
