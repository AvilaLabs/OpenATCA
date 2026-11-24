//! End-to-end evidence for explicit synthetic performance input in the regional CLI.
#![allow(clippy::expect_used)]

use std::fs;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

// REQ: OA-REG-014
#[test]
fn opt_in_keeps_horizontal_frame_and_publishes_performance_evidence() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock follows the Unix epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "oa-regional-performance-cli-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir(&root).expect("create isolated test directory");
    let input = root.join("synthetic-session.tsv");
    let performance = root.join("synthetic-performance.tsv");
    fs::write(&input, include_str!("../examples/synthetic-session.tsv"))
        .expect("write public synthetic session");
    let performance_bytes = include_bytes!("../examples/synthetic-performance.tsv");
    fs::write(&performance, performance_bytes).expect("write public synthetic performance");

    // A standalone copy keeps executable provenance outside Cargo's shared deps directory.
    let executable = root.join("oa-regional-session");
    fs::copy(env!("CARGO_BIN_EXE_oa-regional-session"), &executable)
        .expect("install standalone test binary");

    let baseline_output = root.join("baseline");
    let baseline = Command::new(&executable)
        .args(["run", "--synthetic-model", "--input"])
        .arg(&input)
        .arg("--out")
        .arg(&baseline_output)
        .output()
        .expect("run baseline session");
    assert!(
        baseline.status.success(),
        "baseline CLI failed: {}",
        String::from_utf8_lossy(&baseline.stderr)
    );

    let performance_output = root.join("with-performance");
    let result = Command::new(&executable)
        .args(["run", "--synthetic-model", "--input"])
        .arg(&input)
        .arg("--performance")
        .arg(&performance)
        .arg("--out")
        .arg(&performance_output)
        .output()
        .expect("run session with explicit performance input");
    assert!(
        result.status.success(),
        "performance CLI failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );

    let frame = fs::read_to_string(performance_output.join("frame-0002.txt"))
        .expect("read second frame evidence");
    assert!(frame.contains("vertical_profile: Some("), "{frame}");
    assert!(frame.contains("availability: Available"), "{frame}");
    assert!(frame.contains("refusal: None"), "{frame}");

    let retained = fs::read(performance_output.join("performance-inputs.tsv"))
        .expect("read retained source evidence");
    assert_eq!(retained, performance_bytes);
    let manifest = fs::read_to_string(performance_output.join("manifest.tsv"))
        .expect("read output manifest");
    let digest = oa_sha256::to_hex(&oa_sha256::sha256(performance_bytes));
    assert!(manifest.contains(&format!("performance_sha256\t{digest}\n")));

    assert_eq!(
        fs::read(baseline_output.join("frame-0002.ppm")).expect("baseline pixels"),
        fs::read(performance_output.join("frame-0002.ppm")).expect("opt-in pixels")
    );
    fs::remove_dir_all(root).expect("remove isolated test directory");
}
