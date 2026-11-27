//! Processor-local adaptation identity observations for the Tech Ops view.
//!
//! This shell bridges the runtime's constructor-captured fingerprint to the existing
//! `oa-adapt` package verifier. It does not authenticate the processor, issuer, roster or clock.

use crate::runtime::{
    regional_adaptation_sha256, regional_issued_manifest_sha256, RegionalRuntime,
};
use oa_adapt::local_inventory::{
    verify_local_inventory_with_components, LoadedComponents, LocalActivation, LocalInventoryReport,
};
use oa_sha256::Digest;
use std::time::{SystemTime, UNIX_EPOCH};

/// Read local wall time for evidence display. `None` is preserved when it cannot fit the schema.
pub fn observed_unix_time_ns() -> Option<i64> {
    let elapsed = SystemTime::now().duration_since(UNIX_EPOCH).ok()?;
    i64::try_from(elapsed.as_nanos()).ok()
}

/// Recheck disk while reporting the fingerprint captured from the actual runtime.
/// A refusal or mismatch does not replace the loaded model.
pub fn check_active_runtime_adaptation(
    runtime: &RegionalRuntime,
    activation: &LocalActivation,
    observed_at_unix_ns: Option<i64>,
) -> Result<LocalInventoryReport, &'static str> {
    if regional_issued_manifest_sha256(runtime) != Some(activation.issued_manifest_sha256()) {
        return Err("runtime and activation issued-manifest identities differ");
    }
    Ok(verify_local_inventory_with_components(
        activation,
        LoadedComponents {
            model_snapshot: Some(regional_adaptation_sha256(runtime)),
            runway_aids: crate::runtime::regional_runway_aids_sha256(runtime),
            ..LoadedComponents::default()
        },
        observed_at_unix_ns,
    ))
}

/// Explicit status for legacy/synthetic runs, which have no issued package pin.
pub fn unpinned_source_report() -> String {
    let mut report = String::new();
    report.push_str("OA_TECHOPS_PROCESSOR_REPORT\t1\n");
    report.push_str("phase\tUNAVAILABLE\n");
    report.push_str(
        "processor_id\tUNAVAILABLE\tunissued model source has no enrolled processor identity\n",
    );
    report.push_str("observed_at_unix_ns\tUNAVAILABLE\tno issued-runtime processor observation\n");
    report
        .push_str("issued_manifest_sha256\tUNAVAILABLE\tno independently pinned issue manifest\n");
    report.push_str("verified_component_coverage\tUNAVAILABLE\tno issued package verified\n");
    report.push_str("loaded_runtime_model_snapshot_sha256\tUNAVAILABLE\truntime model is not bound to an issued package\n");
    report.push_str("other_loaded_components\tUNAVAILABLE\tno component inventory captured\n");
    report.push_str("ca_configuration_state\tUNAVAILABLE\truntime CA configuration not observed; empty is not inferred\n");
    report.push_str(
        "composite_adaptation_status\tUNAVAILABLE\tno composite runtime fingerprint available\n",
    );
    report.push_str("disk_adaptation_sha256\tUNAVAILABLE\tno package recheck is available\n");
    report.push_str("status\tUNAVAILABLE\tunverified replay or synthetic source is not a processor inventory match\n");
    report.push_str("provenance\tUNVERIFIED\tno authenticated processor or issuer claim\n");
    report
}

/// Bounded report emitted when startup cannot verify the independent issue pin.
pub fn startup_refusal_report(
    processor_id: u32,
    expected_manifest_sha256: Digest,
    observed_at_unix_ns: Option<i64>,
) -> String {
    let mut report = String::new();
    report.push_str("OA_TECHOPS_PROCESSOR_REPORT\t1\n");
    report.push_str("phase\tSTARTUP_REFUSED\n");
    report.push_str(&format!("processor_id\tOBSERVED\t{processor_id}\n"));
    match observed_at_unix_ns {
        Some(value) if value >= 0 => {
            report.push_str(&format!("observed_at_unix_ns\tOBSERVED\t{value}\n"));
        }
        _ => {
            report.push_str("observed_at_unix_ns\tUNAVAILABLE\tlocal clock timestamp unavailable\n")
        }
    }
    report.push_str("clock_quality\tUNVERIFIED\tlocal system wall clock\n");
    report.push_str(
        "processor_identity\tUNAVAILABLE\tcaller-supplied label; no authenticated channel\n",
    );
    report.push_str("issued_manifest_sha256\tOBSERVED\t");
    report.push_str(&oa_sha256::to_hex(&expected_manifest_sha256));
    report.push('\n');
    report.push_str("verified_component_coverage\tUNAVAILABLE\tstartup verification refused\n");
    report
        .push_str("loaded_runtime_model_snapshot_sha256\tUNAVAILABLE\truntime did not activate\n");
    report.push_str("other_loaded_components\tUNAVAILABLE\truntime did not activate\n");
    report.push_str(
        "ca_configuration_state\tUNAVAILABLE\truntime did not activate; empty is not inferred\n",
    );
    report.push_str("composite_adaptation_status\tUNAVAILABLE\truntime did not activate\n");
    report.push_str("disk_manifest_sha256\tUNAVAILABLE\tissued-package verification refused\n");
    report.push_str("disk_adaptation_sha256\tUNAVAILABLE\tissued-package verification refused\n");
    report.push_str("status\tFAULT\tSTARTUP_VERIFICATION_REFUSED\n");
    report.push_str("provenance\tUNVERIFIED\tno authenticated processor or issuer claim\n");
    report
}
