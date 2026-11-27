//! Bounded certification data for a Tech Ops service query.
//!
//! This is a plain-data model. Digests identify supplied bytes but do not authenticate them;
//! principal, authorization, and time values are claims. The shell must bind a returned view to
//! its durable audit receipt before presenting it as a committed certification record.

use crate::PverTargetMapRow;

/// Maximum target-map rows emitted by the fixed full PVER catalog.
pub const MAX_CERTIFICATION_TARGET_ROWS: usize = 6;
/// Maximum processor evidence rows accepted by the existing certification workflow.
pub const MAX_CERTIFICATION_PROCESSOR_ROWS: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum CertificationViewOutcome {
    Certified = 1,
    CertifiedWithExceptions = 2,
    NotCertified = 3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum CertificationViewMode {
    Full = 1,
    Abbreviated = 2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum CertificationEvidenceState {
    Present = 1,
    Missing = 2,
    Refused = 3,
    Invalid = 4,
}

/// Digest plus explicit availability. `None` is preserved; it is never replaced with zero bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct CertificationDigestEvidence {
    pub state: CertificationEvidenceState,
    pub sha256: Option<[u8; 32]>,
}
const _: () = assert!(core::mem::size_of::<CertificationDigestEvidence>() == 34);

/// One processor's expected and locally observed software/adaptation artifact evidence.
///
/// The row describes local file observations and expected values. It does not claim that the
/// corresponding image or adaptation is loaded on a running or remote processor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct CertificationProcessorRow {
    pub processor_id: u32,
    pub expected_software: CertificationDigestEvidence,
    pub observed_software: CertificationDigestEvidence,
    pub expected_adaptation: CertificationDigestEvidence,
    pub observed_adaptation: CertificationDigestEvidence,
}
// A7.2/C7: cold fixed report row; at most 128 rows, 140 bytes at four-byte alignment.
const _: () = assert!(core::mem::size_of::<CertificationProcessorRow>() == 140);

/// Bounded query payload joined from one durable certification event and its source evidence.
///
/// The caller-owned row slices have exact logical lengths, at most 6 PVER targets and 128
/// processors. Missing target observations and missing processor artifacts retain explicit
/// states. `audit_sequence`/`audit_digest` and the checkpoint bind the source record; the shell
/// wrapper must expose this model only with the receipt returned by the append/read verifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CertificationServiceView<'a> {
    pub outcome: CertificationViewOutcome,
    pub mode: CertificationViewMode,
    pub pver_passed: bool,
    pub targets_checked: u8,
    pub targets_passed: u8,
    pub tracker_loss_rows: u64,
    pub tracker_refusals: u64,
    pub catalog_version: &'a str,
    pub catalog_sha256: [u8; 32],
    pub adaptation_version: Option<u64>,
    pub adaptation: CertificationDigestEvidence,
    pub profile_binding_sha256: Option<[u8; 32]>,
    pub tracker_config_sha256: [u8; 32],
    pub runner_build_claim_sha256: [u8; 32],
    pub runner_artifact: CertificationDigestEvidence,
    pub expected_inventory_sha256: [u8; 32],
    pub expected_processor_count: u16,
    pub observed_processor_count: u16,
    pub missing_processor_count: u16,
    pub duplicate_processor_count: u16,
    pub unexpected_processor_count: u16,
    pub principal_id_claim: u64,
    pub authorization_registry_sha256: [u8; 32],
    pub authorization_level_claim: u8,
    pub time_ns_claim: u64,
    pub service_id: &'a str,
    pub exceptions: &'a str,
    pub audit_sequence: u64,
    pub audit_digest: [u8; 32],
    pub checkpoint_count: u64,
    pub checkpoint_tail: [u8; 32],
    pub target_rows: &'a [Option<PverTargetMapRow>],
    pub processor_rows: &'a [Option<CertificationProcessorRow>],
}
