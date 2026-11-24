#![allow(clippy::expect_used)]

use std::fs;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn writes_input_only_report_to_a_new_output_directory() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock follows the Unix epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "oa-regional-session-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir(&root).expect("create isolated test directory");
    let input = root.join("synthetic.tsv");
    let output = root.join("new-output");
    fs::write(&input, include_str!("../examples/synthetic-normalized.tsv"))
        .expect("write synthetic protocol input");

    let result = Command::new(env!("CARGO_BIN_EXE_oa-regional-session"))
        .arg("stage")
        .arg("--input")
        .arg(&input)
        .arg("--out")
        .arg(&output)
        .output()
        .expect("run the local CLI binary");

    assert!(
        result.status.success(),
        "CLI failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(output.is_dir());
    let report =
        fs::read_to_string(output.join("session-report.tsv")).expect("read session report");
    assert!(report.contains("status\tSTAGED_INPUT_ONLY"));
    assert!(report.contains("subsystem_execution\tNOT_IMPLEMENTED"));
    fs::remove_dir_all(root).expect("remove isolated test directory");
}

#[test]
fn reports_failure_counts_and_creates_no_partial_output() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock follows the Unix epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "oa-regional-session-failure-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir(&root).expect("create isolated test directory");
    let input = root.join("malformed.tsv");
    let output = root.join("must-not-exist");
    let mut sample_rows = include_str!("../examples/synthetic-normalized.tsv").lines();
    let header = sample_rows.next().expect("sample has a header");
    let admitted = sample_rows.next().expect("sample has an event");
    let unexamined = sample_rows.next().expect("sample has a second event");
    fs::write(&input, format!("{header}\n{admitted}\nbad\n{unexamined}\n"))
        .expect("write malformed protocol input");

    let result = Command::new(env!("CARGO_BIN_EXE_oa-regional-session"))
        .arg("stage")
        .arg("--input")
        .arg(&input)
        .arg("--out")
        .arg(&output)
        .output()
        .expect("run the local CLI binary");

    assert!(!result.status.success());
    assert!(!output.exists());
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(
        stderr.contains("normalized_event_rows admitted=1 refused=1 unexamined=1 denominator=3")
    );
    fs::remove_dir_all(root).expect("remove isolated test directory");
}

// REQ: OA-REG-008, OA-SEAL-014
#[test]
fn timed_guard_precedes_payload_interpretation_and_output_creation() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let root =
        std::env::temp_dir().join(format!("oa-regional-seal-{}-{nonce}", std::process::id()));
    fs::create_dir(&root).expect("isolated directory");
    let stamp = oa_sealed::sealed_windows()
        .first()
        .expect("sealed list")
        .from_ns;
    let input = root.join("neutral.tsv");
    let output = root.join("out");
    fs::write(&input, format!("OA-RS\t1\nv1\t{stamp}\t{stamp}\tbad-sequence\tbad-source\tbad-gufi\tbad-address\tbad-centre\tbad-body\n")).expect("fake sealed-time row");
    let result = Command::new(env!("CARGO_BIN_EXE_oa-regional-session"))
        .args(["run", "--synthetic-model", "--input"])
        .arg(&input)
        .arg("--out")
        .arg(&output)
        .output()
        .expect("CLI");
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("SealedTimestamp"));
    assert!(!output.exists());
    fs::remove_dir_all(root).expect("cleanup");
}

// REQ: OA-REG-011, OA-REG-012, OA-REG-008
#[test]
fn explicit_synthetic_msaw_evaluates_warning_and_closes_evidence() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "oa-regional-msaw-cli-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir(&root).expect("isolated directory");
    let input = root.join("synthetic-msaw.tsv");
    fs::write(&input, include_str!("../examples/synthetic-msaw.tsv"))
        .expect("public synthetic input");
    let executable = root.join("oa-regional-session");
    fs::copy(env!("CARGO_BIN_EXE_oa-regional-session"), &executable).expect("standalone binary");
    let output = root.join("out");
    let result = Command::new(&executable)
        .args(["run", "--synthetic-model", "--synthetic-msaw", "--input"])
        .arg(&input)
        .arg("--out")
        .arg(&output)
        .output()
        .expect("CLI");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let second =
        fs::read_to_string(output.join("frame-0002.txt")).expect("confirmed caution evidence");
    assert!(second.contains("peak: Caution"));
    let last = fs::read_to_string(output.join("frame-0004.txt")).expect("warning evidence");
    assert!(last.contains("peak: Warning"));
    assert!(last.contains("synthetic: true"));
    let settings = fs::read_to_string(output.join("settings.tsv")).expect("settings");
    assert!(settings.contains("msaw\tPUBLIC_SYNTHETIC_FLOOR_WEATHER"));
    let manifest = fs::read(output.join("manifest.tsv")).expect("manifest");
    let complete = fs::read_to_string(output.join("COMPLETE")).expect("completion");
    assert_eq!(
        complete.trim(),
        oa_sha256::to_hex(&oa_sha256::sha256(&manifest))
    );

    // Weather admission must fail before creating output. Actual receipt cannot be
    // guessed from observation time, including when the supplied files do not exist.
    let inputs = root.join("msaw-inputs.tsv");
    fs::write(
        &inputs,
        "OA-MSAW-INPUTS\t1\nCONFIG\tmissing-config.json\nMETAR\t2000\tmissing-weather.json\n",
    )
    .expect("future weather manifest");
    let refused = root.join("refused");
    let result = Command::new(&executable)
        .args(["run", "--synthetic-model", "--msaw-inputs"])
        .arg(&inputs)
        .arg("--input")
        .arg(&input)
        .arg("--out")
        .arg(&refused)
        .output()
        .expect("refused CLI");
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr)
        .contains("METAR receipt is after first regional cutoff"));
    assert!(!refused.exists());
    fs::remove_dir_all(root).expect("cleanup");
}

// REQ: OA-REG-004, OA-REG-005, OA-REG-008
#[test]
fn full_synthetic_session_has_closed_identical_evidence_and_valid_images() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let root =
        std::env::temp_dir().join(format!("oa-regional-closed-{}-{nonce}", std::process::id()));
    fs::create_dir(&root).expect("isolated directory");
    let input = root.join("synthetic.tsv");
    fs::write(&input, include_str!("../examples/synthetic-session.tsv"))
        .expect("public synthetic input");
    // Cargo hardlinks its executable into deps/. An installed standalone copy obeys the
    // same one-name sealed-path guard as every other provenance input; no guard bypass.
    let executable = root.join("oa-regional-session");
    fs::copy(env!("CARGO_BIN_EXE_oa-regional-session"), &executable)
        .expect("install standalone test binary");
    for name in ["first", "second"] {
        let output = root.join(name);
        let result = Command::new(&executable)
            .args(["run", "--synthetic-model", "--input"])
            .arg(&input)
            .arg("--out")
            .arg(&output)
            .output()
            .expect("CLI");
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let manifest = fs::read(output.join("manifest.tsv")).expect("closed manifest");
        let complete = fs::read_to_string(output.join("COMPLETE")).expect("completion marker");
        assert_eq!(
            complete.trim(),
            oa_sha256::to_hex(&oa_sha256::sha256(&manifest))
        );
        let picture = fs::read(output.join("frame-0002.ppm")).expect("confirmed picture");
        let header = b"P6\n800 600\n255\n";
        assert!(picture.starts_with(header));
        assert_eq!(picture.len(), header.len() + 800 * 600 * 3);
        let evidence =
            fs::read_to_string(output.join("frame-0002.txt")).expect("confirmed evidence");
        assert!(evidence.contains("TrajectoryIssueOutput"));
        assert!(evidence.contains("ca_open: [\n"));
    }
    let first = root.join("first");
    let second = root.join("second");
    for entry in fs::read_dir(&first).expect("published files") {
        let entry = entry.expect("file");
        assert_eq!(
            fs::read(entry.path()).expect("first bytes"),
            fs::read(second.join(entry.file_name())).expect("second bytes")
        );
    }
    let before = fs::read(first.join("manifest.tsv")).expect("original");
    let rerun = Command::new(&executable)
        .args(["run", "--synthetic-model", "--input"])
        .arg(&input)
        .arg("--out")
        .arg(&first)
        .output()
        .expect("CLI");
    assert!(!rerun.status.success());
    assert_eq!(
        before,
        fs::read(first.join("manifest.tsv")).expect("unchanged")
    );
    fs::remove_dir_all(root).expect("cleanup");
}
