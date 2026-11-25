//! Synthetic package integrity and route activation checks.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_airspace::core::model::{empty_model, Meta, Model};
use oa_airspace::shell::cli::run;
use oa_airspace::shell::manifest::{verify_current, write_new_package};
use oa_airspace::shell::route_cli::run_route;
use oa_airspace::snapshot::encode;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn fresh_root() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("test clock after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("oa-adapt-g2-{}-{nonce}", std::process::id()))
}

fn synthetic_model() -> Model {
    empty_model(Meta {
        nasr_effective: 20_261_029,
        cifp_cycle: 2611,
        artcc: "ZAU".to_owned(),
        buffer_m: 92_600,
        nasr_sha256: [0x11; 32],
        cifp_sha256: [0x22; 32],
    })
}

// REQ: OA-ADAPT-001, OA-ADAPT-002
#[test]
fn pinned_package_is_checked_before_route_conversion_and_uses_exact_loaded_bytes() {
    let model = synthetic_model();
    let snapshot = encode(&model).expect("fixture snapshot");
    let root = fresh_root();
    let pin = write_new_package(&root, &snapshot, "synthetic-1", "ZAU", "TEST-REQ")
        .expect("package write");
    let verified = verify_current(&root, pin).expect("package verification");
    assert_eq!(
        verified.identity().snapshot_sha256,
        oa_sha256::sha256(&snapshot)
    );

    let args = vec![
        "--package-root".to_owned(),
        root.to_string_lossy().into_owned(),
        "--expected-manifest-sha256".to_owned(),
        oa_sha256::to_hex(&pin),
        "--steps".to_owned(),
        "FIX:ALPHA:K3:1:-".to_owned(),
    ];
    let mut output = Vec::new();
    let route_refusal =
        run_route(&args, &mut output).expect_err("synthetic empty model has no route point");
    assert!(route_refusal
        .to_string()
        .contains("route conversion refused"));
    let report = String::from_utf8(output).expect("UTF-8 report");
    assert!(report.contains("adaptation_status\tverified_for_this_process"));
    assert!(report.contains(&format!("manifest_sha256\t{}", oa_sha256::to_hex(&pin))));

    let mut altered = fs::read(root.join("airspace.snapshot")).expect("read synthetic component");
    let last = altered.len().checked_sub(1).expect("nonempty component");
    altered[last] ^= 1;
    fs::write(root.join("airspace.snapshot"), altered).expect("mutate synthetic component");
    let mut refused_output = Vec::new();
    let refusal = run_route(&args, &mut refused_output).expect_err("changed component refuses");
    assert!(refusal.to_string().contains("snapshot digest mismatch"));
    assert!(refused_output.is_empty());
    let _ = fs::remove_dir_all(root);
}

// REQ: OA-ADAPT-001, OA-ADAPT-002
#[test]
fn incompatible_or_extra_package_paths_refuse_before_route_output() {
    let model = synthetic_model();
    let snapshot = encode(&model).expect("fixture snapshot");
    let root = fresh_root();
    let pin = write_new_package(&root, &snapshot, "synthetic-1", "ZAU", "TEST-REQ")
        .expect("package write");
    fs::write(root.join("unreferenced.bin"), b"extra").expect("write synthetic extra");
    let args = vec![
        "--package-root".to_owned(),
        root.to_string_lossy().into_owned(),
        "--expected-manifest-sha256".to_owned(),
        oa_sha256::to_hex(&pin),
        "--steps".to_owned(),
        "FIX:ALPHA:K3:1:-".to_owned(),
    ];
    let mut output = Vec::new();
    let refusal = run_route(&args, &mut output).expect_err("unreferenced path refuses");
    assert!(refusal.to_string().contains("package"));
    assert!(output.is_empty());
    let _ = fs::remove_dir_all(root);
}

// REQ: OA-ADAPT-001, OA-ADAPT-002
#[test]
fn package_command_reads_back_before_reporting_the_activation_pin() {
    let snapshot = oa_airspace::snapshot::encode(&synthetic_model()).expect("synthetic snapshot");
    let scratch = fresh_root();
    fs::create_dir(&scratch).expect("synthetic scratch directory");
    let snapshot_path = scratch.join("snapshot.synthetic");
    let package_root = scratch.join(format!(
        "package\tmanifest_sha256\t{}\nforged-field",
        "a".repeat(64)
    ));
    fs::write(&snapshot_path, snapshot).expect("write synthetic snapshot");
    let args = vec![
        "package".to_owned(),
        "--snapshot".to_owned(),
        snapshot_path.to_string_lossy().into_owned(),
        "--out-root".to_owned(),
        package_root.to_string_lossy().into_owned(),
        "--release".to_owned(),
        "synthetic-1".to_owned(),
        "--facility".to_owned(),
        "ZAU".to_owned(),
        "--change-ref".to_owned(),
        "TEST-REQ".to_owned(),
    ];
    let mut output = Vec::new();
    run(&args, &mut output).expect("package command");
    let output = String::from_utf8(output).expect("UTF-8 output");
    assert!(output.contains("adaptation_package_status\tcreated_unverified_for_activation"));
    assert!(!output.contains("package_root\t"));
    assert_eq!(output.matches("manifest_sha256\t").count(), 1);
    let pin_text = output
        .lines()
        .find_map(|line| line.strip_prefix("manifest_sha256\t"))
        .expect("reported manifest pin");
    let pin = oa_airspace::shell::manifest::parse_pin(pin_text).expect("canonical reported pin");
    let verified = verify_current(&package_root, pin).expect("read-back verified package");
    assert_eq!(verified.identity().release, "synthetic-1");
    let _ = fs::remove_dir_all(scratch);
}

// REQ: OA-ADAPT-001
#[test]
fn package_command_refuses_an_oversized_argument_before_cloning_or_io() {
    let args = vec![
        "package".to_owned(),
        "--snapshot".to_owned(),
        "x".repeat(4097),
        "--out-root".to_owned(),
        "unused".to_owned(),
        "--release".to_owned(),
        "synthetic".to_owned(),
        "--facility".to_owned(),
        "ZAU".to_owned(),
        "--change-ref".to_owned(),
        "TEST-REQ".to_owned(),
    ];
    let refusal = run(&args, &mut Vec::new()).expect_err("oversized argument refuses");
    assert!(refusal
        .to_string()
        .contains("package argument exceeds byte limit"));
}
