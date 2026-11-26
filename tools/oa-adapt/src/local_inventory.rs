//! Local issued-package startup verification and on-demand disk comparison.
//!
//! This is a shell-side integrity helper. A processor ID is a caller-supplied label, not an
//! authenticated identity, and the report is not a Tech Ops fleet assessment or a PASS.

use crate::manifest::{parse_pin, verify_current, VerifiedPackage, VerifyError};
use oa_sha256::Digest;
use oa_techops::{
    compare_adaptation_identity, AdaptationIdentityComparison, AdaptationIdentityState,
};
use std::path::{Path, PathBuf};

/// Retained local process identity created only after its issued package verifies at startup.
pub struct LocalActivation {
    processor_id: u32,
    package_root: PathBuf,
    issued_manifest_sha256: Digest,
    loaded_adaptation_sha256: Digest,
}

impl LocalActivation {
    pub fn processor_id(&self) -> u32 {
        self.processor_id
    }

    pub fn issued_manifest_sha256(&self) -> Digest {
        self.issued_manifest_sha256
    }

    pub fn loaded_adaptation_sha256(&self) -> Digest {
        self.loaded_adaptation_sha256
    }
}

/// Result of one bounded local disk recheck. Missing disk digests mean verification refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalInventoryReport {
    pub processor_id: u32,
    pub observed_at_unix_ns: Option<i64>,
    pub issued_manifest_sha256: Digest,
    pub expected_adaptation_sha256: Option<Digest>,
    pub loaded_adaptation_sha256: Digest,
    pub disk_manifest_sha256: Option<Digest>,
    pub disk_adaptation_sha256: Option<Digest>,
    pub comparison: AdaptationIdentityComparison,
    /// Always true: local IDs and content hashes do not authenticate a processor or reporter.
    pub provenance_unverified: bool,
    pub status: LocalInventoryStatus,
}

/// Local comparison only; matching content never authenticates a processor or report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocalInventoryStatus {
    Match,
    IssuedManifestMismatch,
    LoadedRuntimeMismatch,
    LoadedDiskMismatch,
    PackageRefused,
}

/// Verify the exact issued package before startup and retain its local identity for later checks.
pub fn verify_local_startup(
    package_root: &Path,
    expected_manifest_sha256: Digest,
    processor_id: u32,
) -> Result<(VerifiedPackage, LocalActivation), VerifyError> {
    let package = verify_current(package_root, expected_manifest_sha256)?;
    let activation = LocalActivation {
        processor_id,
        package_root: package_root.to_path_buf(),
        issued_manifest_sha256: expected_manifest_sha256,
        loaded_adaptation_sha256: package.identity().snapshot_sha256,
    };
    Ok((package, activation))
}

/// Reverify disk against the startup pin. If the pin changed, validate the observed package with
/// its observed manifest digest before reporting its snapshot digest; invalid replacements remain
/// refused and have no reported disk snapshot digest.
pub fn verify_local_inventory(activation: &LocalActivation) -> LocalInventoryReport {
    verify_local_inventory_at(activation, activation.loaded_adaptation_sha256, None)
}

/// Recheck disk using the fingerprint captured by the actual runtime constructor.
/// `observed_at_unix_ns` is caller-supplied wall-clock evidence and is never authenticated here.
pub fn verify_local_inventory_at(
    activation: &LocalActivation,
    loaded_runtime_adaptation_sha256: Digest,
    observed_at_unix_ns: Option<i64>,
) -> LocalInventoryReport {
    let mut report = LocalInventoryReport {
        processor_id: activation.processor_id,
        observed_at_unix_ns,
        issued_manifest_sha256: activation.issued_manifest_sha256,
        expected_adaptation_sha256: Some(activation.loaded_adaptation_sha256),
        loaded_adaptation_sha256: loaded_runtime_adaptation_sha256,
        disk_manifest_sha256: None,
        disk_adaptation_sha256: None,
        comparison: compare_adaptation_identity(
            activation.issued_manifest_sha256,
            Some(activation.loaded_adaptation_sha256),
            None,
            Some(loaded_runtime_adaptation_sha256),
            None,
        ),
        provenance_unverified: true,
        status: LocalInventoryStatus::PackageRefused,
    };

    match verify_current(&activation.package_root, activation.issued_manifest_sha256) {
        Ok(package) => {
            report.disk_manifest_sha256 = Some(package.identity().manifest_sha256);
            report.disk_adaptation_sha256 = Some(package.identity().snapshot_sha256);
            report.status = LocalInventoryStatus::Match;
        }
        Err(VerifyError::ManifestIdentityMismatch { actual, .. }) => {
            if let Ok(observed_pin) = parse_pin(&actual) {
                if let Ok(package) = verify_current(&activation.package_root, observed_pin) {
                    report.disk_manifest_sha256 = Some(package.identity().manifest_sha256);
                    report.disk_adaptation_sha256 = Some(package.identity().snapshot_sha256);
                    report.status = LocalInventoryStatus::IssuedManifestMismatch;
                }
            }
        }
        Err(_) => {}
    }
    report.comparison = compare_adaptation_identity(
        activation.issued_manifest_sha256,
        Some(activation.loaded_adaptation_sha256),
        report.disk_manifest_sha256,
        Some(loaded_runtime_adaptation_sha256),
        report.disk_adaptation_sha256,
    );
    if report.status != LocalInventoryStatus::IssuedManifestMismatch {
        report.status = match report.comparison.state {
            AdaptationIdentityState::Match => LocalInventoryStatus::Match,
            AdaptationIdentityState::Mismatch
                if report.comparison.loaded_matches_issued == Some(false) =>
            {
                LocalInventoryStatus::LoadedRuntimeMismatch
            }
            AdaptationIdentityState::Mismatch => LocalInventoryStatus::LoadedDiskMismatch,
            AdaptationIdentityState::Incomplete
            | AdaptationIdentityState::IncompleteAndMismatch => {
                LocalInventoryStatus::PackageRefused
            }
        };
    }
    report
}

/// Render one actual local processor observation for a consumer such as the Tech Ops view.
/// Each field is explicitly tagged `OBSERVED` or `UNAVAILABLE`; local labels and hashes do not
/// authenticate the processor, issuer, clock, or reporting channel.
pub fn render_processor_report_tsv(phase: &str, report: &LocalInventoryReport) -> String {
    let mut out = String::from("OA_TECHOPS_PROCESSOR_REPORT\t1\n");
    out.push_str(&format!("phase\tOBSERVED\t{phase}\n"));
    out.push_str(&format!(
        "processor_id\tOBSERVED\t{}\n",
        report.processor_id
    ));
    match report.observed_at_unix_ns {
        Some(value) if value >= 0 => out.push_str(&format!("observed_at_unix_ns\tOBSERVED\t{value}\n")),
        _ => out.push_str("observed_at_unix_ns\tUNAVAILABLE\tlocal wall clock did not yield a bounded Unix timestamp\n"),
    }
    out.push_str(
        "clock_quality\tUNVERIFIED\tlocal system wall clock; no independent clock evidence\n",
    );
    out.push_str("processor_identity\tUNAVAILABLE\tcaller-supplied process label; no authenticated reporter channel\n");
    out.push_str(
        "issuer_identity\tUNAVAILABLE\tmanifest principal IDs are not authenticated here\n",
    );
    out.push_str(&format!(
        "issued_manifest_sha256\tOBSERVED\t{}\n",
        oa_sha256::to_hex(&report.issued_manifest_sha256)
    ));
    push_optional_digest(
        &mut out,
        "expected_adaptation_sha256",
        report.expected_adaptation_sha256,
    );
    out.push_str(&format!(
        "loaded_runtime_adaptation_sha256\tOBSERVED\t{}\n",
        oa_sha256::to_hex(&report.loaded_adaptation_sha256)
    ));
    push_optional_digest(
        &mut out,
        "disk_manifest_sha256",
        report.disk_manifest_sha256,
    );
    push_optional_digest(
        &mut out,
        "disk_adaptation_sha256",
        report.disk_adaptation_sha256,
    );
    push_optional_bool(
        &mut out,
        "issued_manifest_matches",
        report.comparison.issued_manifest_matches,
    );
    push_optional_bool(
        &mut out,
        "loaded_matches_issued",
        report.comparison.loaded_matches_issued,
    );
    push_optional_bool(
        &mut out,
        "disk_matches_issued",
        report.comparison.disk_matches_issued,
    );
    push_optional_bool(
        &mut out,
        "loaded_matches_disk",
        report.comparison.loaded_matches_disk,
    );
    out.push_str(&format!(
        "comparison\tOBSERVED\t{:?}\n",
        report.comparison.state
    ));
    out.push_str(&format!("local_status\tOBSERVED\t{:?}\n", report.status));
    out.push_str("provenance\tUNVERIFIED\tcontent comparison only; no authenticated processor or issuer claim\n");
    out
}

fn push_optional_digest(out: &mut String, name: &str, digest: Option<Digest>) {
    match digest {
        Some(value) => out.push_str(&format!(
            "{name}\tOBSERVED\t{}\n",
            oa_sha256::to_hex(&value)
        )),
        None => out.push_str(&format!(
            "{name}\tUNAVAILABLE\tpackage verification yielded no digest\n"
        )),
    }
}

fn push_optional_bool(out: &mut String, name: &str, value: Option<bool>) {
    match value {
        Some(result) => out.push_str(&format!("{name}\tOBSERVED\t{result}\n")),
        None => out.push_str(&format!(
            "{name}\tUNAVAILABLE\trequired identity input not observed\n"
        )),
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::{
        render_processor_report_tsv, verify_local_inventory, verify_local_inventory_at,
        verify_local_startup, AdaptationIdentityState, LocalInventoryStatus,
    };
    use crate::manifest::{issue_new_package, write_new_package, ChangeClass};
    use oa_airspace::model::{empty_model, Meta};
    use oa_airspace::snapshot::encode;
    use oa_sha256::sha256;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    fn fresh_root(label: &str) -> PathBuf {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("oa-g2-local-{label}-{}-{id}", std::process::id()))
    }

    fn sample(buffer_m: u32) -> Vec<u8> {
        encode(&empty_model(Meta {
            nasr_effective: 20_261_029,
            cifp_cycle: 2611,
            artcc: "ZAU".to_owned(),
            buffer_m,
            nasr_sha256: [0x11; 32],
            cifp_sha256: [0x22; 32],
        }))
        .expect("synthetic adaptation snapshot")
    }

    // REQ: OA-ADAPT-003, OA-TECHOPS-001, OA-TECHOPS-002
    #[test]
    fn startup_verifies_pin_and_on_demand_reports_local_loaded_and_disk_digests() {
        let root = fresh_root("match");
        let snapshot = sample(92_600);
        let pin = write_new_package(&root, &snapshot, "issued-1", "ZAU", "G2-LOCAL-1")
            .expect("synthetic issued package");
        let (verified, activation) =
            verify_local_startup(&root, pin, 17).expect("startup pin accepted");
        assert_eq!(verified.identity().manifest_sha256, pin);
        assert_eq!(verified.identity().snapshot_sha256, sha256(&snapshot));
        assert_eq!(activation.processor_id(), 17);

        let report = verify_local_inventory(&activation);
        assert_eq!(report.processor_id, 17);
        assert_eq!(report.issued_manifest_sha256, pin);
        assert_eq!(report.loaded_adaptation_sha256, sha256(&snapshot));
        assert_eq!(report.disk_manifest_sha256, Some(pin));
        assert_eq!(report.disk_adaptation_sha256, Some(sha256(&snapshot)));
        assert!(report.provenance_unverified);
        assert_eq!(report.status, LocalInventoryStatus::Match);
        assert_eq!(report.comparison.state, AdaptationIdentityState::Match);
        let _ = fs::remove_dir_all(root);
    }

    // REQ: OA-TECHOPS-001, OA-TECHOPS-002
    #[test]
    fn on_demand_uses_runtime_fingerprint_and_preserves_timestamp_and_unavailable_fields() {
        let root = fresh_root("runtime-fingerprint");
        let snapshot = sample(92_600);
        let pin = write_new_package(&root, &snapshot, "issued-1", "ZAU", "G2-RUNTIME-1")
            .expect("synthetic issued package");
        let (verified, activation) =
            verify_local_startup(&root, pin, 44).expect("startup pin accepted");
        let expected = verified.identity().snapshot_sha256;
        let report =
            verify_local_inventory_at(&activation, [0x77; 32], Some(1_795_000_000_123_456_789));
        assert_eq!(report.observed_at_unix_ns, Some(1_795_000_000_123_456_789));
        assert_eq!(report.expected_adaptation_sha256, Some(expected));
        assert_eq!(report.loaded_adaptation_sha256, [0x77; 32]);
        assert_eq!(report.disk_adaptation_sha256, Some(expected));
        assert_eq!(report.status, LocalInventoryStatus::LoadedRuntimeMismatch);
        assert_eq!(report.comparison.loaded_matches_issued, Some(false));
        assert_eq!(report.comparison.disk_matches_issued, Some(true));
        let text = render_processor_report_tsv("ON_DEMAND", &report);
        assert!(text.contains("observed_at_unix_ns\tOBSERVED\t1795000000123456789"));
        assert!(text.contains("processor_identity\tUNAVAILABLE\t"));
        assert!(text.contains("disk_adaptation_sha256\tOBSERVED\t"));
        assert!(text.contains("loaded_matches_issued\tOBSERVED\tfalse"));
        assert!(text.contains("provenance\tUNVERIFIED\t"));
        let _ = fs::remove_dir_all(root);
    }

    // REQ: OA-ADAPT-003, OA-TECHOPS-001, OA-TECHOPS-002
    #[test]
    fn on_demand_reports_a_valid_replacement_without_treating_it_as_the_issued_package() {
        let root = fresh_root("replace");
        let legacy_root = fresh_root("replace-legacy");
        let replacement = fresh_root("replacement-source");
        let baseline = sample(92_600);
        let legacy_pin = write_new_package(
            &legacy_root,
            &baseline,
            "legacy-baseline",
            "ZAU",
            "G2-LOCAL-2",
        )
        .expect("synthetic legacy package");
        let (version_one, pin) = issue_new_package(
            &root,
            &baseline,
            "issued-1",
            "ZAU",
            "G2-LOCAL-2",
            &legacy_root,
            legacy_pin,
            ChangeClass::Initial,
            "issuer-1",
            "approver-2",
            "synthetic initial issue",
        )
        .expect("synthetic version one");
        assert_eq!(version_one, 1);
        let (_, activation) = verify_local_startup(&root, pin, 23).expect("startup verifies");

        let changed = sample(92_601);
        let (version, new_pin) = issue_new_package(
            &replacement,
            &changed,
            "issued-2",
            "ZAU",
            "G2-LOCAL-3",
            &root,
            pin,
            ChangeClass::FieldChange,
            "issuer-1",
            "approver-2",
            "synthetic field change",
        )
        .expect("synthetic next version");
        assert_eq!(version, 2);
        fs::copy(
            replacement.join("manifest.oag2"),
            root.join("manifest.oag2"),
        )
        .expect("replace synthetic manifest");
        fs::copy(
            replacement.join("airspace.snapshot"),
            root.join("airspace.snapshot"),
        )
        .expect("replace synthetic snapshot");

        let report = verify_local_inventory(&activation);
        assert_eq!(report.processor_id, 23);
        assert_eq!(report.issued_manifest_sha256, pin);
        assert_eq!(report.loaded_adaptation_sha256, sha256(&baseline));
        assert_eq!(report.disk_manifest_sha256, Some(new_pin));
        assert_eq!(report.disk_adaptation_sha256, Some(sha256(&changed)));
        assert_eq!(report.status, LocalInventoryStatus::IssuedManifestMismatch);
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(legacy_root);
        let _ = fs::remove_dir_all(replacement);
    }

    // REQ: OA-ADAPT-003, OA-TECHOPS-001, OA-TECHOPS-002
    #[test]
    fn startup_refuses_wrong_manifest_pin_before_returning_an_activation() {
        let root = fresh_root("wrong-pin");
        let snapshot = sample(92_600);
        let pin = write_new_package(&root, &snapshot, "issued-1", "ZAU", "G2-LOCAL-4")
            .expect("synthetic issued package");
        assert!(verify_local_startup(&root, [0x55; 32], 31).is_err());
        assert!(verify_local_startup(&root, pin, 31).is_ok());
        let _ = fs::remove_dir_all(root);
    }
}
