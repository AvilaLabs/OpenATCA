#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_adapt::local_inventory::{
    verify_local_inventory_at, verify_local_startup, LocalInventoryStatus,
};
use oa_adapt::manifest::{issue_new_package, verify_current, write_new_package, ChangeClass};
use oa_airspace::model::{empty_model, Meta};
use oa_airspace::snapshot::encode;
use oa_regional_session::cli::{
    run_session_with_profile_options, ModelSource, MsawSource, RegionalCliProfile,
};
use oa_regional_session::processor_inventory::check_active_runtime_adaptation;
use oa_regional_session::runtime::{
    regional_adaptation_sha256, regional_new_from_issued_package, DEFAULT_REGIONAL_RUN_PROFILE,
};
use oa_regional_session::surveillance::SurveillanceConfig;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn fresh(label: &str) -> PathBuf {
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "oa-regional-techops-{label}-{}-{id}",
        std::process::id()
    ))
}

fn snapshot(buffer_m: u32) -> Vec<u8> {
    encode(&empty_model(Meta {
        nasr_effective: 20_261_029,
        cifp_cycle: 2611,
        artcc: "ZAU".to_owned(),
        buffer_m,
        nasr_sha256: [0x11; 32],
        cifp_sha256: [0x22; 32],
    }))
    .expect("synthetic model snapshot")
}

fn normalized_input() -> Vec<u8> {
    b"OA-RS\t1\nv1\t1000000000\t1000000000\t1\tSURVEILLANCE\t00000000000000000000000000000000\tABC101\tZAU1\tPOS\t410000000\t-870000000\t10000\t0\t0\t0\n".to_vec()
}

fn config() -> SurveillanceConfig {
    SurveillanceConfig {
        centre_lat_e7: 410_000_000,
        centre_lon_e7: -870_000_000,
        range_nm: 180.0,
    }
}

fn run_issued(
    package_root: &std::path::Path,
    pin: [u8; 32],
    processor_id: u32,
    out: &std::path::Path,
) -> Result<(), String> {
    run_session_with_profile_options(
        &normalized_input(),
        out,
        &ModelSource::IssuedPackage {
            root: package_root.to_path_buf(),
            expected_manifest_sha256: pin,
            processor_id,
        },
        &MsawSource::Disabled,
        None,
        RegionalCliProfile::Default,
    )
}

// REQ: OA-ADAPT-003, OA-TECHOPS-001, OA-TECHOPS-002
#[test]
fn actual_cli_startup_and_runtime_emit_timestamped_processor_report() {
    let package_root = fresh("cli-package");
    let output = fresh("cli-output");
    let pin = write_new_package(
        &package_root,
        &snapshot(92_600),
        "issued-1",
        "ZAU",
        "I1I2-CLI-1",
    )
    .expect("create synthetic package");

    run_issued(&package_root, pin, 17, &output).expect("issued runtime completes");

    let report = fs::read_to_string(output.join("processor-report.tsv")).expect("processor report");
    assert!(report.contains("phase\tOBSERVED\tSTARTUP"));
    assert!(report.contains("phase\tOBSERVED\tON_DEMAND"));
    assert!(report.contains("observed_at_unix_ns\tOBSERVED\t"));
    assert!(report.contains("loaded_runtime_model_snapshot_sha256\tOBSERVED\t"));
    assert!(report.contains("verified_component_coverage\tOBSERVED\tmodel_snapshot_only"));
    assert!(report.contains("other_loaded_components\tUNAVAILABLE\t"));
    assert!(report.contains("ca_configuration_state\tUNAVAILABLE\t"));
    assert!(report.contains("composite_adaptation_status\tUNAVAILABLE\t"));
    assert!(report.contains("disk_adaptation_sha256\tOBSERVED\t"));
    assert!(report.contains("processor_identity\tUNAVAILABLE\t"));
    assert!(report.contains("model_snapshot_status\tOBSERVED\tMatch"));
    assert!(output.join("COMPLETE").exists());

    let _ = fs::remove_dir_all(package_root);
    let _ = fs::remove_dir_all(output);
}

// REQ: OA-ADAPT-003, OA-TECHOPS-001, OA-TECHOPS-002
#[test]
fn wrong_independent_pin_refuses_cli_startup_and_writes_fault_report() {
    let package_root = fresh("wrong-pin-package");
    let output = fresh("wrong-pin-output");
    let pin = write_new_package(
        &package_root,
        &snapshot(92_600),
        "issued-1",
        "ZAU",
        "I1I2-CLI-2",
    )
    .expect("create synthetic package");

    let result = run_issued(&package_root, [0x44; 32], 18, &output);
    assert!(result
        .expect_err("mismatched independent pin refuses activation")
        .contains("no model activated"));
    let report = fs::read_to_string(output.join("processor-report.tsv")).expect("fault report");
    assert!(report.contains("phase\tSTARTUP_REFUSED"));
    assert!(report.contains("loaded_runtime_model_snapshot_sha256\tUNAVAILABLE"));
    assert!(report.contains("status\tFAULT\tSTARTUP_VERIFICATION_REFUSED"));
    assert!(!output.join("COMPLETE").exists());

    let _ = fs::remove_dir_all(package_root);
    let _ = fs::remove_dir_all(output);
    let _ = pin;
}

// REQ: OA-ADAPT-003, OA-TECHOPS-001, OA-TECHOPS-002
#[test]
fn on_demand_disk_replacement_reports_mismatch_and_keeps_loaded_runtime_model() {
    let package_root = fresh("replacement-package");
    let legacy_root = fresh("replacement-legacy");
    let next_root = fresh("replacement-next");
    let legacy_pin = write_new_package(
        &legacy_root,
        &snapshot(92_600),
        "legacy",
        "ZAU",
        "I1I2-REPLACE-1",
    )
    .expect("create legacy synthetic package");
    let (_, pin) = issue_new_package(
        &package_root,
        &snapshot(92_600),
        "issued-1",
        "ZAU",
        "I1I2-REPLACE-1",
        &legacy_root,
        legacy_pin,
        ChangeClass::Initial,
        "issuer-1",
        "approver-2",
        "synthetic issue",
    )
    .expect("issue baseline package");
    let (verified, activation) =
        verify_local_startup(&package_root, pin, 19).expect("baseline startup verifies");
    let original_fingerprint = verified.identity().snapshot_sha256;
    let runtime = regional_new_from_issued_package(
        verified,
        *b"ZAU1",
        config(),
        DEFAULT_REGIONAL_RUN_PROFILE,
    )
    .expect("runtime consumes verified package model");
    assert_eq!(regional_adaptation_sha256(&runtime), original_fingerprint);

    let (_, next_pin) = issue_new_package(
        &next_root,
        &snapshot(92_601),
        "issued-2",
        "ZAU",
        "I1I2-REPLACE-2",
        &package_root,
        pin,
        ChangeClass::FieldChange,
        "issuer-1",
        "approver-2",
        "synthetic field change",
    )
    .expect("issue replacement package");
    fs::copy(
        next_root.join("manifest.oag2"),
        package_root.join("manifest.oag2"),
    )
    .expect("replace disk manifest");
    fs::copy(
        next_root.join("airspace.snapshot"),
        package_root.join("airspace.snapshot"),
    )
    .expect("replace disk snapshot");

    let report =
        check_active_runtime_adaptation(&runtime, &activation, Some(1_795_000_000_123_456_789))
            .expect("on-demand check runs against retained activation");
    assert_eq!(report.status, LocalInventoryStatus::IssuedManifestMismatch);
    assert_eq!(report.loaded_adaptation_sha256, original_fingerprint);
    assert_eq!(
        report.disk_adaptation_sha256,
        Some(
            verify_current(&next_root, next_pin)
                .expect("replacement verifies")
                .identity()
                .snapshot_sha256
        )
    );
    assert_eq!(regional_adaptation_sha256(&runtime), original_fingerprint);

    let _ = fs::remove_dir_all(package_root);
    let _ = fs::remove_dir_all(legacy_root);
    let _ = fs::remove_dir_all(next_root);
}

// REQ: OA-TECHOPS-001, OA-TECHOPS-002
#[test]
fn old_unpinned_replay_path_publishes_unavailable_not_a_processor_match() {
    let output = fresh("unpinned-output");
    run_session_with_profile_options(
        &normalized_input(),
        &output,
        &ModelSource::Synthetic,
        &MsawSource::Disabled,
        None,
        RegionalCliProfile::Default,
    )
    .expect("synthetic replay completes");
    let report =
        fs::read_to_string(output.join("processor-report.tsv")).expect("unavailable report");
    assert!(report.contains("phase\tUNAVAILABLE"));
    assert!(report.contains("issued_manifest_sha256\tUNAVAILABLE"));
    assert!(report.contains("status\tUNAVAILABLE"));
    assert!(output.join("COMPLETE").exists());
    let _ = fs::remove_dir_all(output);
}
