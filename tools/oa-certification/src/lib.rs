#![forbid(unsafe_code)]
//! Shell (B1): bounded synthetic PVER execution, digest comparison, and append-only certification
//! audit integration. This lane never runs live stimuli and never authenticates caller identities.

use oa_track::{
    output_clear, output_len, output_new, tracker_new, tracker_stats, tracker_update, Measurement,
    OutRow, TrackerConfig, M_HAS_VEL,
};
use std::path::Path;

const CATALOG_BYTES: &[u8] = include_bytes!("../data/synthetic-pver-v1.csv");
const CATALOG_SHA256: oa_sha256::Digest = [
    0xb9, 0xcf, 0x1e, 0x98, 0xe7, 0x25, 0x65, 0xe0, 0xdd, 0x71, 0xa8, 0x4c, 0xaf, 0x2b, 0x8a, 0xde,
    0x19, 0xf1, 0x60, 0x08, 0xd4, 0x18, 0x0e, 0xa0, 0xde, 0xcf, 0x38, 0x72, 0x73, 0xa5, 0x08, 0x40,
];
const CATALOG_VERSION: &str = "synthetic-pver-v1";
const CATALOG_DOCUMENT_HEADER: &str =
    "# OpenATCA synthetic performance verification catalog v1; non-operational fixtures";
const CATALOG_COLUMNS_HEADER: &str =
    "# schema,mode,target_id,lat_e7,lon_e7,east_mps,north_mps,position_tolerance_m,velocity_tolerance_mps";
const MAX_CATALOG_TARGETS: usize = 6;
const MAX_REPORT_CERTIFICATIONS: usize = 256;
const MAX_EXCEPTION_BYTES: usize = 1024;
const DETAILS_MAGIC_V1: &[u8; 8] = b"OACERT01";
const DETAILS_MAGIC_V2: &[u8; 8] = b"OACERT02";
const DETAILS_MAGIC: &[u8; 8] = b"OACERT03";
const DETAILS_MAGIC_V4: &[u8; 8] = b"OACERT04";
const DETAILS_MAGIC_V5: &[u8; 8] = b"OACERT05";
const DETAILS_MAGIC_V6: &[u8; 8] = b"OACERT06";
const MAX_PROCESSORS: usize = 128;
const MAX_REQUEST_SOURCES: usize = 128;
const MAX_SOFTWARE_ARTIFACT_BYTES: u64 = oa_build_info::MAX_ARTIFACT_BYTES;
pub const MAX_AUTH_REGISTRY_BYTES: usize = 64 * 1024;
const FULL_REPORTS_PER_TARGET: usize = 12;
const BASE_TIME_NS: i64 = 1_800_000_000_000_000_000;
const NS_PER_SECOND: i64 = 1_000_000_000;
const METERS_PER_DEGREE: f64 = 111_319.5;

/// Fixed scope selected by the caller. Target membership and tolerances come from the verified
/// external catalog; the abbreviation is exactly its three declared rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PverMode {
    Full,
    Abbreviated,
}

/// Versioned target row from the externally stored synthetic performance catalog.
#[derive(Clone, Debug, PartialEq)]
pub struct SyntheticTarget {
    pub id: String,
    pub latitude_e7: i32,
    pub longitude_e7: i32,
    pub east_mps: f64,
    pub north_mps: f64,
    pub position_tolerance_m: f64,
    pub velocity_tolerance_mps: f64,
}

/// Verified synthetic target catalog. Its digest is included in the audit details.
#[derive(Clone, Debug, PartialEq)]
pub struct SyntheticCatalog {
    pub version: String,
    pub sha256: oa_sha256::Digest,
    adaptation_version: Option<u64>,
    full: Vec<SyntheticTarget>,
    abbreviated: Vec<SyntheticTarget>,
}

/// Output measurements and their target-specific comparison results.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PverResult {
    mode: PverMode,
    catalog_version: String,
    catalog_sha256: oa_sha256::Digest,
    adaptation_version: Option<u64>,
    adaptation_evidence: DigestEvidence,
    runner_artifact_evidence: DigestEvidence,
    tracker_config_sha256: oa_sha256::Digest,
    runner_build_claim_sha256: oa_sha256::Digest,
    profile_binding_sha256: Option<oa_sha256::Digest>,
    full_catalog_targets: Vec<(String, u64, u64)>,
    abbreviated_catalog_targets: Vec<(String, u64, u64)>,
    target_results: Vec<TargetResult>,
    targets_checked: usize,
    targets_passed: usize,
    passed: bool,
    tracker_loss_rows: u64,
    tracker_refusals: u64,
}

impl PverResult {
    pub fn mode(&self) -> PverMode {
        self.mode
    }
    pub fn catalog_sha256(&self) -> oa_sha256::Digest {
        self.catalog_sha256
    }
    pub fn catalog_version(&self) -> &str {
        &self.catalog_version
    }
    pub fn adaptation_version(&self) -> Option<u64> {
        self.adaptation_version
    }
    pub fn adaptation_sha256(&self) -> Option<oa_sha256::Digest> {
        self.adaptation_evidence.sha256
    }
    pub fn adaptation_state(&self) -> EvidenceState {
        self.adaptation_evidence.state
    }
    pub fn runner_artifact_sha256(&self) -> Option<oa_sha256::Digest> {
        self.runner_artifact_evidence.sha256
    }
    pub fn runner_artifact_state(&self) -> EvidenceState {
        self.runner_artifact_evidence.state
    }
    pub fn profile_binding_sha256(&self) -> Option<oa_sha256::Digest> {
        self.profile_binding_sha256
    }
    pub fn tracker_config_sha256(&self) -> oa_sha256::Digest {
        self.tracker_config_sha256
    }
    pub fn runner_build_claim_sha256(&self) -> oa_sha256::Digest {
        self.runner_build_claim_sha256
    }
    pub fn target_results(&self) -> &[TargetResult] {
        &self.target_results
    }
    pub fn targets_checked(&self) -> usize {
        self.targets_checked
    }
    pub fn targets_passed(&self) -> usize {
        self.targets_passed
    }
    pub fn passed(&self) -> bool {
        self.passed
    }
    pub fn tracker_loss_rows(&self) -> u64 {
        self.tracker_loss_rows
    }
    pub fn tracker_refusals(&self) -> u64 {
        self.tracker_refusals
    }
}

/// One per-target result retained in the audit event; all distances use micro-units.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TargetResult {
    pub target_id: String,
    pub state: TargetResultState,
    pub output_time_ns: Option<i64>,
    pub position_error_um: Option<u64>,
    pub position_tolerance_um: u64,
    pub velocity_error_um_s: Option<u64>,
    pub velocity_tolerance_um_s: u64,
}

/// Per-target observable outcome retained in the PVER record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum TargetResultState {
    MissingOutput = 1,
    Passed = 2,
    PositionExceeded = 3,
    VelocityExceeded = 4,
    BothExceeded = 5,
}

/// Whether one expected evidence value exists and is usable for comparison.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum EvidenceState {
    Present = 1,
    Missing = 2,
    Refused = 3,
    Invalid = 4,
}

/// A digest and its explicit availability state. `None` carries no digest and is never a zero
/// placeholder; `Present` identifies bytes but does not authenticate their origin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DigestEvidence {
    pub state: EvidenceState,
    pub sha256: Option<oa_sha256::Digest>,
}

impl DigestEvidence {
    pub const fn present(sha256: oa_sha256::Digest) -> Self {
        Self {
            state: EvidenceState::Present,
            sha256: Some(sha256),
        }
    }

    pub const fn missing() -> Self {
        Self {
            state: EvidenceState::Missing,
            sha256: None,
        }
    }

    pub const fn refused() -> Self {
        Self {
            state: EvidenceState::Refused,
            sha256: None,
        }
    }

    pub const fn invalid() -> Self {
        Self {
            state: EvidenceState::Invalid,
            sha256: None,
        }
    }
}

/// One expected processor with its pinned and observed software and G2 adaptation evidence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessorEvidence {
    pub processor_id: u32,
    pub expected_software: DigestEvidence,
    pub observed_software: DigestEvidence,
    pub expected_adaptation: DigestEvidence,
    pub observed_adaptation: DigestEvidence,
}

impl ProcessorEvidence {
    fn missing(processor_id: u32) -> Self {
        Self {
            processor_id,
            expected_software: DigestEvidence::missing(),
            observed_software: DigestEvidence::missing(),
            expected_adaptation: DigestEvidence::missing(),
            observed_adaptation: DigestEvidence::missing(),
        }
    }

    fn invalid(processor_id: u32) -> Self {
        Self {
            processor_id,
            expected_software: DigestEvidence::invalid(),
            observed_software: DigestEvidence::invalid(),
            expected_adaptation: DigestEvidence::invalid(),
            observed_adaptation: DigestEvidence::invalid(),
        }
    }
}

fn normalize_processor_evidence(processor: &mut ProcessorEvidence) {
    if !digest_evidence_consistent(processor.expected_software) {
        processor.expected_software = DigestEvidence::invalid();
    }
    if !digest_evidence_consistent(processor.observed_software) {
        processor.observed_software = DigestEvidence::invalid();
    }
    if !digest_evidence_consistent(processor.expected_adaptation) {
        processor.expected_adaptation = DigestEvidence::invalid();
    }
    if !digest_evidence_consistent(processor.observed_adaptation) {
        processor.observed_adaptation = DigestEvidence::invalid();
    }
}

fn digest_evidence_consistent(evidence: DigestEvidence) -> bool {
    match evidence.state {
        EvidenceState::Present => evidence.sha256.is_some(),
        EvidenceState::Missing | EvidenceState::Refused | EvidenceState::Invalid => {
            evidence.sha256.is_none()
        }
    }
}

/// Human certification decision. Principal and timestamp remain caller-supplied claims.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Certification {
    principal_id: u64,
    authorization_registry_sha256: oa_sha256::Digest,
    authorization_level_claim: AuthorizationLevelClaim,
    expected_inventory_sha256: oa_sha256::Digest,
    time_ns: u64,
    service_id: String,
    pver: PverResult,
    expected_processor_ids: Vec<u32>,
    observed_processor_ids: Vec<u32>,
    processors: Vec<ProcessorEvidence>,
    missing_processor_ids: Vec<u32>,
    duplicate_processor_ids: Vec<u32>,
    unexpected_processor_ids: Vec<u32>,
    outcome: CertificationOutcome,
    exceptions: String,
}

/// Result exposed only after the append has been synced by `oa-audit-log`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommittedCertification {
    pub outcome: CertificationOutcome,
    pub checkpoint: oa_audit_core::Checkpoint,
    pub receipt: oa_audit_core::AuditReceipt,
    pub record: Certification,
}

/// Inputs for one expected processor in an on-demand certification attempt. Paths are local
/// artifact/package observations; they do not attest a running or remote processor.
#[derive(Clone, Debug)]
pub struct ProcessorSource {
    pub processor_id: u32,
    pub expected_software: DigestEvidence,
    pub software_artifact_path: Option<std::path::PathBuf>,
    pub expected_adaptation: DigestEvidence,
    pub adaptation_package: Option<(std::path::PathBuf, oa_sha256::Digest)>,
}

/// Caller-supplied request for one full or abbreviated synthetic certification attempt.
#[derive(Clone, Debug)]
pub struct CertificationRequest {
    pub authorization: PrincipalAuthorizationClaim,
    pub time_ns: u64,
    pub service_id: String,
    pub mode: PverMode,
    pub expected_processor_ids: Vec<u32>,
    /// SHA-256 of the exact separately supplied expected-roster bytes.
    pub expected_inventory_sha256: oa_sha256::Digest,
    pub processors: Vec<ProcessorSource>,
    pub exceptions: String,
    pub max_software_bytes: u64,
}

/// Caller-supplied local registry claim. It is not authentication or proof of identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PrincipalAuthorizationClaim {
    principal_id: u64,
    registry_sha256: oa_sha256::Digest,
    level: AuthorizationLevelClaim,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AuthorizationLevelClaim {
    Configure,
}

/// Parse a small local principal registry and require a `configure` entry. The digest binds the
/// exact registry bytes read by the caller; it does not authenticate the file or its author.
pub fn authorize_configure_claim(
    registry: &[u8],
    principal_id: u64,
) -> Result<PrincipalAuthorizationClaim, CertificationError> {
    if registry.is_empty() || registry.len() > MAX_AUTH_REGISTRY_BYTES || principal_id == 0 {
        return Err(CertificationError::Invalid(
            "principal registry exceeds fixed bounds",
        ));
    }
    let text = std::str::from_utf8(registry)
        .map_err(|_| CertificationError::Invalid("principal registry is not UTF-8"))?;
    let mut lines = text.lines();
    if lines.next() != Some("OACERT-LOCAL-PRINCIPALS-V1") {
        return Err(CertificationError::Invalid(
            "principal registry schema is unsupported",
        ));
    }
    let mut found = false;
    let mut count = 0usize;
    let mut seen = Vec::with_capacity(MAX_PROCESSORS);
    for line in lines {
        if line.is_empty() {
            continue;
        }
        count = count.checked_add(1).ok_or(CertificationError::Invalid(
            "principal registry count overflow",
        ))?;
        if count > MAX_PROCESSORS {
            return Err(CertificationError::Invalid(
                "principal registry row cap exceeded",
            ));
        }
        let Some((id_text, level)) = line.split_once('\t') else {
            return Err(CertificationError::Invalid(
                "principal registry row is malformed",
            ));
        };
        let id = id_text
            .parse::<u64>()
            .map_err(|_| CertificationError::Invalid("principal registry ID is invalid"))?;
        if id == 0
            || id_text != id.to_string()
            || !matches!(level, "configure" | "operate" | "monitor")
        {
            return Err(CertificationError::Invalid(
                "principal registry value is invalid",
            ));
        }
        if seen.contains(&id) {
            return Err(CertificationError::Invalid(
                "principal registry contains duplicate IDs",
            ));
        }
        seen.push(id);
        if id == principal_id {
            if found || level != "configure" {
                return Err(CertificationError::Invalid(
                    "principal is duplicate or lacks configure claim",
                ));
            }
            found = true;
        }
    }
    if !found {
        return Err(CertificationError::Invalid(
            "principal has no configure claim",
        ));
    }
    Ok(PrincipalAuthorizationClaim {
        principal_id,
        registry_sha256: oa_sha256::sha256(registry),
        level: AuthorizationLevelClaim::Configure,
    })
}

/// Build the certification claim only from a role derived by the shared bounded principal
/// registry. `registry_sha256` is the independently supplied pin verified by the shell loader;
/// it does not authenticate who supplied the registry or principal ID.
pub fn authorize_configure_registry_claim(
    registry: &oa_audit_core::PrincipalRegistry,
    registry_sha256: oa_sha256::Digest,
    principal_id: u64,
) -> Result<PrincipalAuthorizationClaim, CertificationError> {
    if registry_sha256 == [0; 32] || principal_id == 0 {
        return Err(CertificationError::Invalid(
            "principal or registry pin is invalid",
        ));
    }
    let role = oa_audit_core::authorize_principal(
        registry,
        principal_id,
        oa_audit_core::Action::Certification,
    )
    .map_err(|_| CertificationError::Invalid("principal is not authorized to certify"))?;
    if role != oa_audit_core::Role::Configure {
        return Err(CertificationError::Invalid(
            "certification requires configure role",
        ));
    }
    Ok(PrincipalAuthorizationClaim {
        principal_id,
        registry_sha256,
        level: AuthorizationLevelClaim::Configure,
    })
}

/// The bounded set of outcomes supported by this synthetic workflow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CertificationOutcome {
    Certified,
    CertifiedWithExceptions,
    NotCertified,
}

/// Invalid synthetic catalog, input, or certification record.
#[derive(Debug)]
pub enum CertificationError {
    Catalog(&'static str),
    Tracker(oa_track::TrackError),
    Invalid(&'static str),
    Adaptation(oa_adapt::manifest::VerifyError),
    Audit(oa_audit_log::shell::LogError),
    SealedPath(String),
}

impl std::fmt::Display for CertificationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Catalog(why) => write!(f, "synthetic PVER catalog refused: {why}"),
            Self::Tracker(error) => write!(f, "tracker refused: {error}"),
            Self::Invalid(why) => write!(f, "certification record refused: {why}"),
            Self::Adaptation(error) => write!(f, "G2 adaptation verification failed: {error}"),
            Self::Audit(error) => write!(f, "audit append failed: {error:?}"),
            Self::SealedPath(error) => write!(f, "sealed path refused: {error}"),
        }
    }
}

impl std::error::Error for CertificationError {}

/// Read the one embedded, content-pinned synthetic data file. The fixture is separate from the
/// runner source so target values and tolerances have an independently reviewable provenance.
pub fn synthetic_catalog() -> Result<SyntheticCatalog, CertificationError> {
    let digest = oa_sha256::sha256(CATALOG_BYTES);
    if digest != CATALOG_SHA256 {
        return Err(CertificationError::Catalog("embedded data digest mismatch"));
    }
    parse_pver_catalog(CATALOG_BYTES, None)
}

fn parse_pver_catalog(
    bytes: &[u8],
    adaptation_version: Option<u64>,
) -> Result<SyntheticCatalog, CertificationError> {
    if bytes.is_empty() || bytes.len() > oa_adapt::manifest::MAX_PVER_CATALOG_BYTES {
        return Err(CertificationError::Catalog(
            "catalog byte length exceeds bound",
        ));
    }
    let digest = oa_sha256::sha256(bytes);
    let text = std::str::from_utf8(bytes)
        .map_err(|_| CertificationError::Catalog("catalog is not UTF-8"))?;
    let mut lines = text.lines();
    if lines.next() != Some(CATALOG_DOCUMENT_HEADER) || lines.next() != Some(CATALOG_COLUMNS_HEADER)
    {
        return Err(CertificationError::Catalog(
            "catalog version or column header is invalid",
        ));
    }
    let mut full = Vec::new();
    let mut abbreviated = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split(',').collect();
        if fields.len() != 9 || fields.first().copied() != Some(CATALOG_VERSION) {
            return Err(CertificationError::Catalog("row schema is invalid"));
        }
        let mode = fields
            .get(1)
            .copied()
            .ok_or(CertificationError::Catalog("missing mode"))?;
        let target = SyntheticTarget {
            id: fields
                .get(2)
                .copied()
                .ok_or(CertificationError::Catalog("missing target ID"))?
                .to_owned(),
            latitude_e7: parse_field(fields.get(3).copied(), "latitude")?,
            longitude_e7: parse_field(fields.get(4).copied(), "longitude")?,
            east_mps: parse_field(fields.get(5).copied(), "east velocity")?,
            north_mps: parse_field(fields.get(6).copied(), "north velocity")?,
            position_tolerance_m: parse_field(fields.get(7).copied(), "position tolerance")?,
            velocity_tolerance_mps: parse_field(fields.get(8).copied(), "velocity tolerance")?,
        };
        if target.id.is_empty()
            || !(-900_000_000..=900_000_000).contains(&target.latitude_e7)
            || !(-1_800_000_000..=1_800_000_000).contains(&target.longitude_e7)
            || !target.east_mps.is_finite()
            || !target.north_mps.is_finite()
            || !target.position_tolerance_m.is_finite()
            || target.position_tolerance_m <= 0.0
            || !target.velocity_tolerance_mps.is_finite()
            || target.velocity_tolerance_mps <= 0.0
        {
            return Err(CertificationError::Catalog(
                "target values are out of range",
            ));
        }
        match mode {
            "full" => full.push(target),
            "abbreviated" => abbreviated.push(target),
            _ => return Err(CertificationError::Catalog("mode is unsupported")),
        }
        if full.len() > MAX_CATALOG_TARGETS || abbreviated.len() > MAX_CATALOG_TARGETS {
            return Err(CertificationError::Catalog(
                "target count exceeds fixed bound",
            ));
        }
    }
    if full.len() != MAX_CATALOG_TARGETS || abbreviated.len() != 3 {
        return Err(CertificationError::Catalog(
            "catalog does not contain required fixed scopes",
        ));
    }
    let full_ids = full
        .iter()
        .map(|target| target.id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let abbreviated_ids = abbreviated
        .iter()
        .map(|target| target.id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    if full_ids.len() != full.len()
        || abbreviated_ids.len() != abbreviated.len()
        || abbreviated.iter().any(|target| {
            full.iter().find(|full_target| full_target.id == target.id) != Some(target)
        })
    {
        return Err(CertificationError::Catalog(
            "abbreviated scope is not a unique exact full-scope subset",
        ));
    }
    Ok(SyntheticCatalog {
        version: CATALOG_VERSION.to_owned(),
        sha256: digest,
        adaptation_version,
        full,
        abbreviated,
    })
}

fn parse_field<T: std::str::FromStr>(
    value: Option<&str>,
    label: &'static str,
) -> Result<T, CertificationError> {
    value
        .ok_or(CertificationError::Catalog(label))?
        .parse()
        .map_err(|_| CertificationError::Catalog(label))
}

fn catalog_target_identity(target: &SyntheticTarget) -> (String, u64, u64) {
    (
        target.id.clone(),
        micro_units(target.position_tolerance_m).unwrap_or(0),
        micro_units(target.velocity_tolerance_mps).unwrap_or(0),
    )
}

/// Execute fixed synthetic target trajectories through the production `oa-track` tracker.
/// Verify the G2 package before binding the versioned target catalog to its snapshot digest and
/// running the synthetic profile. The supplied manifest pin must be independent of package bytes.
pub fn run_bound_performance(
    mode: PverMode,
    package_root: &Path,
    expected_manifest_sha256: oa_sha256::Digest,
) -> Result<PverResult, CertificationError> {
    let package = oa_adapt::manifest::verify_current(package_root, expected_manifest_sha256)
        .map_err(CertificationError::Adaptation)?;
    let adaptation = DigestEvidence::present(package.identity().snapshot_sha256);
    let catalog_bytes = package
        .pver_catalog_bytes()
        .ok_or(CertificationError::Catalog(
            "verified package has no issued PVER catalog",
        ))?;
    let catalog = parse_pver_catalog(catalog_bytes, package.identity().adaptation_version)?;
    run_catalog(&catalog, mode, adaptation)
}

fn run_catalog(
    catalog: &SyntheticCatalog,
    mode: PverMode,
    adaptation_evidence: DigestEvidence,
) -> Result<PverResult, CertificationError> {
    let targets = match mode {
        PverMode::Full => &catalog.full,
        PverMode::Abbreviated => &catalog.abbreviated,
    };
    let config = TrackerConfig {
        max_tracks: MAX_CATALOG_TARGETS,
        out_capacity: 512,
        log_capacity: 512,
        tick_ns: NS_PER_SECOND,
        expected_interval_ns: NS_PER_SECOND,
        init_interval_ns: NS_PER_SECOND,
        ..TrackerConfig::default()
    };
    let mut tracker = tracker_new(&config).map_err(CertificationError::Tracker)?;
    let mut output = output_new(config.out_capacity);
    let mut latest: Vec<Option<OutRow>> = vec![None; targets.len()];
    let mut output_overflow = 0u64;
    for second in 0..FULL_REPORTS_PER_TARGET {
        for target in targets {
            let time_ns = BASE_TIME_NS
                .checked_add(
                    (second as i64)
                        .checked_mul(NS_PER_SECOND)
                        .ok_or(CertificationError::Invalid("synthetic time overflow"))?,
                )
                .ok_or(CertificationError::Invalid("synthetic time overflow"))?;
            let m = measurement(target, time_ns, second as f64);
            output_clear(&mut output);
            tracker_update(&mut tracker, &m, &mut output);
            output_overflow = output_overflow.saturating_add(output.overflow);
            for row_index in 0..output_len(&output) {
                let Some(address) = output.addr.get(row_index).copied() else {
                    continue;
                };
                let Some(target_index) = targets
                    .iter()
                    .position(|item| target_address(item) == address)
                else {
                    continue;
                };
                let Some(row) = output_row(&output, row_index) else {
                    continue;
                };
                if latest
                    .get(target_index)
                    .is_some_and(|old| old.is_none_or(|current| current.t_ns <= row.t_ns))
                {
                    if let Some(slot) = latest.get_mut(target_index) {
                        *slot = Some(row);
                    }
                }
            }
        }
    }
    let stats = *tracker_stats(&tracker);
    let refused = stats
        .refused
        .iter()
        .copied()
        .fold(0u64, u64::saturating_add);
    let losses = stats
        .rows_unrepresentable
        .saturating_add(stats.nan_covariance);
    let no_loss = refused == 0
        && stats.capacity_refused == 0
        && stats.velocity_gated == 0
        && losses == 0
        && output_overflow == 0;
    let mut passed = no_loss
        && adaptation_evidence.state == EvidenceState::Present
        && adaptation_evidence.sha256.is_some();
    let runner_artifact_evidence = current_runner_artifact_evidence();
    passed &= runner_artifact_evidence.state == EvidenceState::Present;
    let count = targets.len();
    let mut target_results = Vec::with_capacity(count);
    let mut targets_passed = 0usize;
    for (index, target) in targets.iter().enumerate() {
        let tolerance_pos = micro_units(target.position_tolerance_m)?;
        let tolerance_vel = micro_units(target.velocity_tolerance_mps)?;
        let evidence = match latest.get(index).copied().flatten() {
            None => TargetResult {
                target_id: target.id.clone(),
                state: TargetResultState::MissingOutput,
                output_time_ns: None,
                position_error_um: None,
                position_tolerance_um: tolerance_pos,
                velocity_error_um_s: None,
                velocity_tolerance_um_s: tolerance_vel,
            },
            Some(row) => {
                let elapsed = (row.t_ns.saturating_sub(BASE_TIME_NS)) as f64 / NS_PER_SECOND as f64;
                let expected_lat = target.latitude_e7 as f64
                    + target.north_mps * elapsed / METERS_PER_DEGREE * 10_000_000.0;
                let cos_lat = (target.latitude_e7 as f64 / 10_000_000.0)
                    .to_radians()
                    .cos();
                let expected_lon = target.longitude_e7 as f64
                    + target.east_mps * elapsed / (METERS_PER_DEGREE * cos_lat) * 10_000_000.0;
                let position_error_m =
                    ((row.lat_e7 as f64 - expected_lat) / 10_000_000.0 * METERS_PER_DEGREE).hypot(
                        (row.lon_e7 as f64 - expected_lon) / 10_000_000.0
                            * METERS_PER_DEGREE
                            * cos_lat,
                    );
                let velocity_error_mps = (row.vel_east_mps as f64 - target.east_mps)
                    .hypot(row.vel_north_mps as f64 - target.north_mps);
                if !position_error_m.is_finite() || !velocity_error_mps.is_finite() {
                    return Err(CertificationError::Invalid(
                        "PVER produced a non-finite target residual",
                    ));
                }
                let position_error_um = micro_units(position_error_m)?;
                let velocity_error_um_s = micro_units(velocity_error_mps)?;
                let position_ok = within_micro_tolerance(position_error_um, tolerance_pos);
                let velocity_ok = within_micro_tolerance(velocity_error_um_s, tolerance_vel);
                let state = match (position_ok, velocity_ok) {
                    (true, true) => TargetResultState::Passed,
                    (false, true) => TargetResultState::PositionExceeded,
                    (true, false) => TargetResultState::VelocityExceeded,
                    (false, false) => TargetResultState::BothExceeded,
                };
                if state == TargetResultState::Passed {
                    targets_passed = targets_passed
                        .checked_add(1)
                        .ok_or(CertificationError::Invalid("PVER target count overflow"))?;
                }
                TargetResult {
                    target_id: target.id.clone(),
                    state,
                    output_time_ns: Some(row.t_ns),
                    position_error_um: Some(position_error_um),
                    position_tolerance_um: tolerance_pos,
                    velocity_error_um_s: Some(velocity_error_um_s),
                    velocity_tolerance_um_s: tolerance_vel,
                }
            }
        };
        if evidence.state != TargetResultState::Passed {
            passed = false;
        }
        target_results.push(evidence);
    }
    let tracker_config_sha256 = tracker_config_digest(&config)?;
    let runner_build_claim_sha256 = oa_sha256::sha256(oa_build_info::BUILD.as_bytes());
    let profile_binding_sha256 = adaptation_evidence.sha256.map(|adaptation_sha256| {
        profile_binding(
            catalog.sha256,
            mode,
            adaptation_sha256,
            runner_artifact_evidence.sha256.unwrap_or([0; 32]),
            tracker_config_sha256,
            runner_build_claim_sha256,
        )
    });
    Ok(PverResult {
        mode,
        catalog_version: catalog.version.clone(),
        catalog_sha256: catalog.sha256,
        adaptation_version: catalog.adaptation_version,
        adaptation_evidence,
        runner_artifact_evidence,
        tracker_config_sha256,
        runner_build_claim_sha256,
        profile_binding_sha256,
        full_catalog_targets: catalog.full.iter().map(catalog_target_identity).collect(),
        abbreviated_catalog_targets: catalog
            .abbreviated
            .iter()
            .map(catalog_target_identity)
            .collect(),
        target_results,
        targets_checked: count,
        targets_passed,
        passed,
        tracker_loss_rows: losses.saturating_add(output_overflow),
        tracker_refusals: refused.saturating_add(stats.capacity_refused),
    })
}

fn current_runner_artifact_evidence() -> DigestEvidence {
    let path = match std::env::current_exe() {
        Ok(path) => path,
        Err(_) => return DigestEvidence::refused(),
    };
    match oa_build_info::digest_artifact(&path, MAX_SOFTWARE_ARTIFACT_BYTES) {
        Ok(artifact) => DigestEvidence::present(artifact.sha256),
        Err(_) => DigestEvidence::refused(),
    }
}

fn micro_units(value: f64) -> Result<u64, CertificationError> {
    if !value.is_finite() || value < 0.0 {
        return Err(CertificationError::Invalid(
            "PVER metric is not finite/nonnegative",
        ));
    }
    let scaled = (value * 1_000_000.0).round();
    if !scaled.is_finite() || scaled > u64::MAX as f64 {
        return Err(CertificationError::Invalid(
            "PVER metric exceeds fixed-point range",
        ));
    }
    Ok(scaled as u64)
}

fn within_micro_tolerance(error_um: u64, tolerance_um: u64) -> bool {
    error_um <= tolerance_um
}

fn tracker_config_digest(config: &TrackerConfig) -> Result<oa_sha256::Digest, CertificationError> {
    let mut bytes = Vec::with_capacity(256);
    bytes.extend_from_slice(b"OA-PVER-TRACKER-CONFIG-V1\0");
    let mut push_usize = |value: usize| -> Result<(), CertificationError> {
        let value = u64::try_from(value)
            .map_err(|_| CertificationError::Invalid("tracker profile integer overflow"))?;
        bytes.extend_from_slice(&value.to_le_bytes());
        Ok(())
    };
    push_usize(config.max_tracks)?;
    push_usize(config.out_capacity)?;
    push_usize(config.log_capacity)?;
    for value in [
        config.tick_ns,
        config.expected_interval_ns,
        config.init_interval_ns,
        config.coast_after_intervals,
        config.drop_after_ns,
        config.reacquire_gap_ns,
        config.confirm_window_ns,
        config.ooo_window_ns,
        config.clock_step_limit_ns,
    ] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    for value in [
        config.coast_max_r95_m,
        config.vel_gate_chi2,
        config.gate_manoeuvre_mps2,
        config.confirm_move_m,
        config.confirm_agree_m,
    ] {
        bytes.extend_from_slice(&value.to_bits().to_le_bytes());
    }
    bytes.push(config.resync_consistent_k);
    bytes.push(config.init_m);
    bytes.push(config.init_n);
    bytes.push(config.h_mode);
    bytes.push(u8::from(config.confirm_moves));
    let pool = u64::try_from(config.confirm_pool)
        .map_err(|_| CertificationError::Invalid("tracker profile pool overflow"))?;
    bytes.extend_from_slice(&pool.to_le_bytes());
    Ok(oa_sha256::sha256(&bytes))
}

fn profile_binding(
    catalog_sha256: oa_sha256::Digest,
    mode: PverMode,
    adaptation_sha256: oa_sha256::Digest,
    runner_artifact_sha256: oa_sha256::Digest,
    tracker_config_sha256: oa_sha256::Digest,
    runner_build_claim_sha256: oa_sha256::Digest,
) -> oa_sha256::Digest {
    let mut bytes = Vec::with_capacity(8 + 1 + 5 * 32);
    bytes.extend_from_slice(b"OACERT-PROFILE-V2\0");
    bytes.push(match mode {
        PverMode::Full => 1,
        PverMode::Abbreviated => 2,
    });
    bytes.extend_from_slice(&catalog_sha256);
    bytes.extend_from_slice(&adaptation_sha256);
    bytes.extend_from_slice(&runner_artifact_sha256);
    bytes.extend_from_slice(&tracker_config_sha256);
    bytes.extend_from_slice(&runner_build_claim_sha256);
    oa_sha256::sha256(&bytes)
}

fn target_address(target: &SyntheticTarget) -> u32 {
    target.id.bytes().fold(0x00a0_0000u32, |value, byte| {
        value.wrapping_mul(31).wrapping_add(u32::from(byte))
    }) & 0x00ff_ffff
}

fn measurement(target: &SyntheticTarget, time_ns: i64, elapsed: f64) -> Measurement {
    let cos_lat = (target.latitude_e7 as f64 / 10_000_000.0)
        .to_radians()
        .cos();
    let latitude_e7 = (target.latitude_e7 as f64
        + target.north_mps * elapsed / METERS_PER_DEGREE * 10_000_000.0)
        .round() as i32;
    let longitude_e7 = (target.longitude_e7 as f64
        + target.east_mps * elapsed / (METERS_PER_DEGREE * cos_lat) * 10_000_000.0)
        .round() as i32;
    Measurement {
        toa_ns: time_ns,
        pos_cov_m2: [25.0, 0.0, 25.0],
        vel_cov_m2s2: [4.0, 0.0, 4.0],
        vel_east_mps: target.east_mps as f32,
        vel_north_mps: target.north_mps as f32,
        lat_e7: latitude_e7,
        lon_e7: longitude_e7,
        alt_ft: oa_track::NONE_I32,
        vrate_fpm: oa_track::NONE_I32,
        addr: target_address(target),
        alt_quant_ft: 0,
        sensor: 1,
        ident: oa_track::NO_IDENT,
        flags: M_HAS_VEL,
        category: 0,
        vel_lag_s: 0.0,
    }
}

fn output_row(output: &oa_track::TrackOutput, i: usize) -> Option<OutRow> {
    Some(OutRow {
        t_ns: *output.t_ns.get(i)?,
        addr: *output.addr.get(i)?,
        lat_e7: *output.lat_e7.get(i)?,
        lon_e7: *output.lon_e7.get(i)?,
        vel_east_mps: *output.vel_east_mps.get(i)?,
        vel_north_mps: *output.vel_north_mps.get(i)?,
        pos_cov_m2: *output.pos_cov_m2.get(i)?,
        vel_cov_m2s2: *output.vel_cov_m2s2.get(i)?,
        turn_rate_rps: *output.turn_rate_rps.get(i)?,
        alt_ft: *output.alt_ft.get(i)?,
        alt_var_ft2: *output.alt_var_ft2.get(i)?,
        vrate_fpm: *output.vrate_fpm.get(i)?,
        mu_h: *output.mu_h.get(i)?,
        mu_v: *output.mu_v.get(i)?,
        flags: *output.flags.get(i)?,
        ident: *output.ident.get(i)?,
        ident_ns: *output.ident_ns.get(i)?,
        ident_conflict: *output.ident_conflict.get(i)?,
    })
}

/// Verify a G2 adaptation package once for one processor, returning the snapshot digest from the
/// exact bytes checked by `oa-adapt`.
pub fn verified_adaptation_digest(
    package_root: &Path,
    expected_manifest_sha256: oa_sha256::Digest,
) -> Result<oa_sha256::Digest, CertificationError> {
    let package = oa_adapt::manifest::verify_current(package_root, expected_manifest_sha256)
        .map_err(CertificationError::Adaptation)?;
    Ok(package.identity().snapshot_sha256)
}

/// Observe one expected processor. Missing fields remain explicit; artifact read or G2
/// verification refusals retain their state and can be recorded as `not_certified`.
pub fn observe_processor_evidence(
    processor_id: u32,
    expected_software_sha256: DigestEvidence,
    software_artifact_path: Option<&Path>,
    expected_adaptation_sha256: DigestEvidence,
    adaptation_package: Option<(&Path, oa_sha256::Digest)>,
    max_software_bytes: u64,
) -> ProcessorEvidence {
    let observed_software = match software_artifact_path {
        Some(path) => match oa_build_info::digest_artifact(path, max_software_bytes) {
            Ok(observed) => DigestEvidence::present(observed.sha256),
            Err(_) => DigestEvidence::refused(),
        },
        None => DigestEvidence::missing(),
    };
    let observed_adaptation = match adaptation_package {
        Some((root, manifest_pin)) => match verified_adaptation_digest(root, manifest_pin) {
            Ok(digest) => DigestEvidence::present(digest),
            Err(_) => DigestEvidence::refused(),
        },
        None => DigestEvidence::missing(),
    };
    ProcessorEvidence {
        processor_id,
        expected_software: expected_software_sha256,
        observed_software,
        expected_adaptation: expected_adaptation_sha256,
        observed_adaptation,
    }
}

/// Prepare a complete attempt from an explicit expected inventory and per-processor local
/// evidence sources. Missing IDs are retained as `Missing`; failed local reads or G2 verification
/// become `Refused`. This function never infers a healthy result from absent data.
fn prepare_certification(
    request: CertificationRequest,
) -> Result<Certification, CertificationError> {
    let CertificationRequest {
        authorization,
        time_ns,
        service_id,
        mode,
        expected_processor_ids,
        expected_inventory_sha256,
        processors: sources,
        exceptions,
        max_software_bytes,
    } = request;
    // Validate every caller-controlled bound before catalog work, processor path inspection, or
    // adaptation verification. A refused request must not cause a partial inventory scan.
    if expected_processor_ids.is_empty()
        || expected_processor_ids.len() > MAX_PROCESSORS
        || sources.len() > MAX_REQUEST_SOURCES
        || max_software_bytes == 0
        || max_software_bytes > MAX_SOFTWARE_ARTIFACT_BYTES
        || service_id.is_empty()
        || service_id.len() > 96
        || exceptions.len() > MAX_EXCEPTION_BYTES
        || authorization.principal_id == 0
        || authorization.level != AuthorizationLevelClaim::Configure
        || authorization.registry_sha256 == [0; 32]
        || expected_inventory_sha256 == [0; 32]
    {
        return Err(CertificationError::Invalid(
            "request fields exceed fixed certification bounds",
        ));
    }
    let mut expected_preflight = expected_processor_ids.clone();
    expected_preflight.sort_unstable();
    if expected_preflight.contains(&0)
        || expected_preflight
            .windows(2)
            .any(|pair| pair.first().zip(pair.get(1)).is_some_and(|(a, b)| a == b))
    {
        return Err(CertificationError::Invalid(
            "expected processor IDs must be nonzero and unique",
        ));
    }
    if sources.iter().any(|source| source.processor_id == 0) {
        return Err(CertificationError::Invalid(
            "source processor IDs must be nonzero",
        ));
    }
    let mut inventory_union = expected_preflight;
    inventory_union.extend(sources.iter().map(|source| source.processor_id));
    inventory_union.sort_unstable();
    inventory_union.dedup();
    if inventory_union.len() > MAX_PROCESSORS
        || sources.iter().any(|source| {
            source
                .software_artifact_path
                .as_ref()
                .is_some_and(|path| path.as_os_str().len() > 4096)
                || source
                    .adaptation_package
                    .as_ref()
                    .is_some_and(|(path, _)| path.as_os_str().len() > 4096)
        })
    {
        return Err(CertificationError::Invalid(
            "processor inventory or path exceeds its fixed bound",
        ));
    }
    let pver_adaptation = match sources
        .iter()
        .find_map(|source| source.adaptation_package.as_ref())
    {
        Some((root, manifest_pin)) => {
            let package = oa_adapt::manifest::verify_current(root, *manifest_pin)
                .map_err(CertificationError::Adaptation)?;
            let bytes = package
                .pver_catalog_bytes()
                .ok_or(CertificationError::Catalog(
                    "verified package has no issued PVER catalog",
                ))?;
            let catalog = parse_pver_catalog(bytes, package.identity().adaptation_version)?;
            (
                catalog,
                DigestEvidence::present(package.identity().snapshot_sha256),
            )
        }
        None => {
            return Err(CertificationError::Catalog(
                "issued PVER catalog package is required",
            ))
        }
    };
    let pver = run_catalog(&pver_adaptation.0, mode, pver_adaptation.1)?;
    let processor_evidence = sources
        .iter()
        .map(|source| {
            observe_processor_evidence(
                source.processor_id,
                source.expected_software,
                source.software_artifact_path.as_deref(),
                source.expected_adaptation,
                source
                    .adaptation_package
                    .as_ref()
                    .map(|(root, pin)| (root.as_path(), *pin)),
                max_software_bytes,
            )
        })
        .collect();
    let mut certification = decide_certification(
        authorization.principal_id,
        time_ns,
        service_id,
        pver,
        expected_processor_ids,
        processor_evidence,
        exceptions,
    )?;
    certification.authorization_registry_sha256 = authorization.registry_sha256;
    certification.authorization_level_claim = authorization.level;
    certification.expected_inventory_sha256 = expected_inventory_sha256;
    Ok(certification)
}

/// Run the fixed synthetic PVER, gather all stated processor evidence, decide the outcome, and
/// return it only after the append-only audit event commits successfully.
pub fn record_certification(
    log_path: &Path,
    expected_checkpoint: oa_audit_core::Checkpoint,
    request: CertificationRequest,
) -> Result<CommittedCertification, CertificationError> {
    let record = prepare_certification(request)?;
    let (checkpoint, receipt) =
        append_certification_with_receipt(log_path, expected_checkpoint, &record)?;
    Ok(CommittedCertification {
        outcome: record.outcome,
        checkpoint,
        receipt,
        record,
    })
}

/// Decide certification using the actual PVER result, all processor digest comparisons, and the
/// exception text. Every mismatch and failed check forces NotCertified.
fn decide_certification(
    principal_id: u64,
    time_ns: u64,
    service_id: String,
    pver: PverResult,
    expected_processor_ids: Vec<u32>,
    processors: Vec<ProcessorEvidence>,
    exceptions: String,
) -> Result<Certification, CertificationError> {
    if principal_id == 0
        || service_id.is_empty()
        || service_id.len() > 96
        || expected_processor_ids.is_empty()
        || expected_processor_ids.len() > 128
        || processors.len() > 128
    {
        return Err(CertificationError::Invalid(
            "principal, service, or processor inventory is invalid",
        ));
    }
    if expected_processor_ids.contains(&0) {
        return Err(CertificationError::Invalid(
            "expected processor IDs must be nonzero",
        ));
    }
    let mut expected_sorted = expected_processor_ids.clone();
    expected_sorted.sort_unstable();
    if expected_sorted
        .windows(2)
        .any(|pair| pair.first().zip(pair.get(1)).is_some_and(|(a, b)| a == b))
    {
        return Err(CertificationError::Invalid(
            "expected processor IDs must be unique",
        ));
    }
    let expected_processor_ids = expected_sorted;
    let mut observed_processor_ids = processors
        .iter()
        .map(|processor| processor.processor_id)
        .collect::<Vec<_>>();
    observed_processor_ids.sort_unstable();
    let mut normalized_processors = processors;
    for processor in &mut normalized_processors {
        normalize_processor_evidence(processor);
    }
    if exceptions.len() > MAX_EXCEPTION_BYTES {
        return Err(CertificationError::Invalid(
            "exception text exceeds 1024 bytes",
        ));
    }
    let expected_targets = match pver.mode {
        PverMode::Full => 6,
        PverMode::Abbreviated => 3,
    };
    let catalog_targets = match pver.mode {
        PverMode::Full => &pver.full_catalog_targets,
        PverMode::Abbreviated => &pver.abbreviated_catalog_targets,
    };
    let mut checked_target_results = 0usize;
    let target_results_valid = pver.target_results.len() == expected_targets
        && pver.target_results.iter().zip(catalog_targets).all(
            |(result, (target_id, expected_position, expected_velocity))| {
                result.target_id == *target_id
                    && result.position_tolerance_um == *expected_position
                    && result.velocity_tolerance_um_s == *expected_velocity
                    && match result.state {
                        TargetResultState::MissingOutput => {
                            result.output_time_ns.is_none()
                                && result.position_error_um.is_none()
                                && result.velocity_error_um_s.is_none()
                        }
                        TargetResultState::Passed
                        | TargetResultState::PositionExceeded
                        | TargetResultState::VelocityExceeded
                        | TargetResultState::BothExceeded => {
                            let Some(position) = result.position_error_um else {
                                return false;
                            };
                            let Some(velocity) = result.velocity_error_um_s else {
                                return false;
                            };
                            if result.output_time_ns.is_none() {
                                return false;
                            }
                            let position_ok = position <= result.position_tolerance_um;
                            let velocity_ok = velocity <= result.velocity_tolerance_um_s;
                            let expected_state = match (position_ok, velocity_ok) {
                                (true, true) => TargetResultState::Passed,
                                (false, true) => TargetResultState::PositionExceeded,
                                (true, false) => TargetResultState::VelocityExceeded,
                                (false, false) => TargetResultState::BothExceeded,
                            };
                            if expected_state != result.state {
                                return false;
                            }
                            if result.state == TargetResultState::Passed {
                                checked_target_results = checked_target_results.saturating_add(1);
                            }
                            true
                        }
                    }
            },
        );
    if !digest_evidence_consistent(pver.adaptation_evidence)
        || pver.full_catalog_targets.len() != MAX_CATALOG_TARGETS
        || pver.abbreviated_catalog_targets.len() != 3
        || pver.profile_binding_sha256
            != pver.adaptation_evidence.sha256.map(|digest| {
                profile_binding(
                    pver.catalog_sha256,
                    pver.mode,
                    digest,
                    pver.runner_artifact_evidence.sha256.unwrap_or([0; 32]),
                    pver.tracker_config_sha256,
                    pver.runner_build_claim_sha256,
                )
            })
        || pver.targets_checked != expected_targets
        || pver.targets_passed > pver.targets_checked
        || !target_results_valid
        || checked_target_results != pver.targets_passed
        || (pver.passed
            && (pver.targets_passed != pver.targets_checked
                || pver.tracker_loss_rows != 0
                || pver.tracker_refusals != 0
                || pver.adaptation_evidence.state != EvidenceState::Present
                || pver.adaptation_evidence.sha256.is_none()))
        || (pver.passed
            && (pver.runner_artifact_evidence.state != EvidenceState::Present
                || pver.runner_artifact_evidence.sha256.is_none()))
    {
        return Err(CertificationError::Invalid(
            "PVER result does not match the fixed catalog contract",
        ));
    }
    let mut complete_processors = Vec::with_capacity(expected_processor_ids.len());
    let missing_processor_ids = expected_processor_ids
        .iter()
        .filter(|id| observed_processor_ids.binary_search(id).is_err())
        .copied()
        .collect::<Vec<_>>();
    let mut duplicate_processor_ids = observed_processor_ids
        .windows(2)
        .filter_map(|pair| {
            pair.first()
                .zip(pair.get(1))
                .filter(|(left, right)| left == right)
                .map(|(id, _)| *id)
        })
        .collect::<Vec<_>>();
    duplicate_processor_ids.sort_unstable();
    duplicate_processor_ids.dedup();
    for processor_id in &expected_processor_ids {
        let mut matching = normalized_processors
            .iter()
            .filter(|p| p.processor_id == *processor_id);
        let evidence = match (matching.next(), matching.next()) {
            (None, _) => ProcessorEvidence::missing(*processor_id),
            (Some(item), None) => item.clone(),
            (Some(_), Some(_)) => ProcessorEvidence::invalid(*processor_id),
        };
        complete_processors.push(evidence);
    }
    let mut unexpected_processor_ids = observed_processor_ids
        .iter()
        .filter(|id| !expected_processor_ids.contains(id))
        .copied()
        .collect::<Vec<_>>();
    unexpected_processor_ids.sort_unstable();
    unexpected_processor_ids.dedup();
    for processor_id in &unexpected_processor_ids {
        let mut matching = normalized_processors
            .iter()
            .filter(|p| p.processor_id == *processor_id);
        let evidence = match (matching.next(), matching.next()) {
            (Some(item), None) => item.clone(),
            _ => ProcessorEvidence::invalid(*processor_id),
        };
        complete_processors.push(evidence);
    }
    if complete_processors.len() > 128 {
        return Err(CertificationError::Invalid(
            "expected and unexpected processor evidence exceeds 128 records",
        ));
    }
    complete_processors.sort_by_key(|p| p.processor_id);
    let mismatch = pver.adaptation_evidence.state != EvidenceState::Present
        || pver.adaptation_evidence.sha256.is_none()
        || !unexpected_processor_ids.is_empty()
        || !missing_processor_ids.is_empty()
        || !duplicate_processor_ids.is_empty()
        || complete_processors.iter().any(|p| {
            p.expected_software.state != EvidenceState::Present
                || p.observed_software.state != EvidenceState::Present
                || p.expected_adaptation.state != EvidenceState::Present
                || p.observed_adaptation.state != EvidenceState::Present
                || p.expected_software.sha256 != p.observed_software.sha256
                || p.expected_adaptation.sha256 != p.observed_adaptation.sha256
                || p.expected_adaptation.sha256 != pver.adaptation_evidence.sha256
        });
    let outcome = if !pver.passed || mismatch {
        CertificationOutcome::NotCertified
    } else if exceptions.is_empty() {
        CertificationOutcome::Certified
    } else {
        CertificationOutcome::CertifiedWithExceptions
    };
    Ok(Certification {
        principal_id,
        authorization_registry_sha256: [0; 32],
        authorization_level_claim: AuthorizationLevelClaim::Configure,
        expected_inventory_sha256: [0; 32],
        time_ns,
        service_id,
        pver,
        expected_processor_ids,
        observed_processor_ids,
        processors: complete_processors,
        missing_processor_ids,
        duplicate_processor_ids,
        unexpected_processor_ids,
        outcome,
        exceptions,
    })
}

/// Hash the processor's observed software artifact bytes through the `oa-build-info` shell API.
pub fn software_artifact_digest(
    path: &Path,
    max_bytes: u64,
) -> Result<oa_sha256::Digest, CertificationError> {
    oa_build_info::digest_artifact(path, max_bytes)
        .map(|item| item.sha256)
        .map_err(|_| CertificationError::Invalid("processor software artifact digest failed"))
}

/// Append the canonical certification event to the verified audit chain. Timestamp and principal
/// are caller claims; the shell function does not authenticate or read a host clock.
#[cfg(test)]
fn append_certification(
    path: &Path,
    expected: oa_audit_core::Checkpoint,
    certification: &Certification,
) -> Result<oa_audit_core::Checkpoint, CertificationError> {
    append_certification_with_receipt(path, expected, certification)
        .map(|(checkpoint, _)| checkpoint)
}

fn append_certification_with_receipt(
    path: &Path,
    expected: oa_audit_core::Checkpoint,
    certification: &Certification,
) -> Result<(oa_audit_core::Checkpoint, oa_audit_core::AuditReceipt), CertificationError> {
    oa_sealed::refuse(path).map_err(CertificationError::SealedPath)?;
    let reported_processors = certification
        .observed_processor_ids
        .iter()
        .filter_map(|processor_id| {
            certification
                .processors
                .iter()
                .find(|processor| processor.processor_id == *processor_id)
                .cloned()
        })
        .collect::<Vec<_>>();
    let mut verified_decision = decide_certification(
        certification.principal_id,
        certification.time_ns,
        certification.service_id.clone(),
        certification.pver.clone(),
        certification.expected_processor_ids.clone(),
        reported_processors,
        certification.exceptions.clone(),
    )?;
    verified_decision.authorization_registry_sha256 = certification.authorization_registry_sha256;
    verified_decision.authorization_level_claim = certification.authorization_level_claim;
    verified_decision.expected_inventory_sha256 = certification.expected_inventory_sha256;
    if certification.authorization_registry_sha256 == [0; 32]
        || certification.authorization_level_claim != AuthorizationLevelClaim::Configure
        || certification.expected_inventory_sha256 == [0; 32]
        || verified_decision != *certification
    {
        return Err(CertificationError::Invalid(
            "recorded certification differs from its recomputed evidence decision",
        ));
    }
    let details = encode_details(certification)?;
    let (target, target_len) = oa_audit_core::target(certification.service_id.as_bytes())
        .map_err(|_| CertificationError::Invalid("service ID exceeds audit target limit"))?;
    let (details, details_len) = oa_audit_core::details(&details).map_err(|_| {
        CertificationError::Invalid("certification details exceed audit record limit")
    })?;
    let event = oa_audit_core::Event {
        principal: certification.principal_id,
        role: oa_audit_core::Role::Configure,
        action: oa_audit_core::Action::Certification,
        target,
        target_len,
        time_ns: certification.time_ns,
        details,
        details_len,
    };
    oa_audit_log::shell::append_receipt(path, expected, event).map_err(CertificationError::Audit)
}

fn encode_details(c: &Certification) -> Result<Vec<u8>, CertificationError> {
    let exception_bytes = c.exceptions.as_bytes();
    let exception_len = u16::try_from(exception_bytes.len())
        .map_err(|_| CertificationError::Invalid("exception text is too long"))?;
    let target_count = u16::try_from(c.pver.targets_checked)
        .map_err(|_| CertificationError::Invalid("PVER target count is too large"))?;
    let passed_count = u16::try_from(c.pver.targets_passed)
        .map_err(|_| CertificationError::Invalid("PVER pass count is too large"))?;
    let processor_count = u16::try_from(c.processors.len())
        .map_err(|_| CertificationError::Invalid("processor inventory is too large"))?;
    let expected_count = u16::try_from(c.expected_processor_ids.len())
        .map_err(|_| CertificationError::Invalid("expected processor inventory is too large"))?;
    let unexpected_count = u16::try_from(c.unexpected_processor_ids.len())
        .map_err(|_| CertificationError::Invalid("unexpected processor inventory is too large"))?;
    let missing_count = u16::try_from(c.missing_processor_ids.len())
        .map_err(|_| CertificationError::Invalid("missing processor inventory is too large"))?;
    let duplicate_count = u16::try_from(c.duplicate_processor_ids.len())
        .map_err(|_| CertificationError::Invalid("duplicate processor inventory is too large"))?;
    let mut out = Vec::with_capacity(20_480);
    out.extend_from_slice(DETAILS_MAGIC_V6);
    out.push(match c.outcome {
        CertificationOutcome::Certified => 1,
        CertificationOutcome::CertifiedWithExceptions => 2,
        CertificationOutcome::NotCertified => 3,
    });
    out.push(match c.pver.mode {
        PverMode::Full => 1,
        PverMode::Abbreviated => 2,
    });
    out.push(u8::from(c.pver.passed));
    out.extend_from_slice(&target_count.to_le_bytes());
    out.extend_from_slice(&passed_count.to_le_bytes());
    out.extend_from_slice(&c.pver.tracker_loss_rows.to_le_bytes());
    out.extend_from_slice(&c.pver.tracker_refusals.to_le_bytes());
    out.extend_from_slice(&c.pver.catalog_sha256);
    let catalog_version = c.pver.catalog_version.as_bytes();
    out.push(
        u8::try_from(catalog_version.len())
            .map_err(|_| CertificationError::Invalid("catalog version is too long"))?,
    );
    out.extend_from_slice(catalog_version);
    out.extend_from_slice(&c.pver.adaptation_version.unwrap_or(0).to_le_bytes());
    out.extend_from_slice(&c.pver.tracker_config_sha256);
    out.extend_from_slice(&c.pver.runner_build_claim_sha256);
    out.extend_from_slice(&c.authorization_registry_sha256);
    out.push(match c.authorization_level_claim {
        AuthorizationLevelClaim::Configure => 1,
    });
    out.extend_from_slice(&c.expected_inventory_sha256);
    append_digest_evidence(&mut out, c.pver.runner_artifact_evidence);
    let target_count = u8::try_from(c.pver.target_results.len())
        .map_err(|_| CertificationError::Invalid("PVER target results exceed fixed bound"))?;
    out.push(target_count);
    for target in &c.pver.target_results {
        let id = target.target_id.as_bytes();
        let id_len = u8::try_from(id.len())
            .map_err(|_| CertificationError::Invalid("PVER target ID exceeds fixed bound"))?;
        out.push(id_len);
        out.extend_from_slice(id);
        out.push(target.state as u8);
        out.extend_from_slice(&target.output_time_ns.unwrap_or(0).to_le_bytes());
        out.extend_from_slice(&target.position_error_um.unwrap_or(0).to_le_bytes());
        out.extend_from_slice(&target.position_tolerance_um.to_le_bytes());
        out.extend_from_slice(&target.velocity_error_um_s.unwrap_or(0).to_le_bytes());
        out.extend_from_slice(&target.velocity_tolerance_um_s.to_le_bytes());
    }
    append_digest_evidence(&mut out, c.pver.adaptation_evidence);
    match c.pver.profile_binding_sha256 {
        Some(digest) => {
            out.push(EvidenceState::Present as u8);
            out.extend_from_slice(&digest);
        }
        None => out.push(EvidenceState::Missing as u8),
    }
    out.extend_from_slice(&exception_len.to_le_bytes());
    out.extend_from_slice(exception_bytes);
    out.extend_from_slice(&expected_count.to_le_bytes());
    for processor_id in &c.expected_processor_ids {
        out.extend_from_slice(&processor_id.to_le_bytes());
    }
    let observed_count = u16::try_from(c.observed_processor_ids.len())
        .map_err(|_| CertificationError::Invalid("observed processor inventory is too large"))?;
    out.extend_from_slice(&observed_count.to_le_bytes());
    for processor_id in &c.observed_processor_ids {
        out.extend_from_slice(&processor_id.to_le_bytes());
    }
    out.extend_from_slice(&unexpected_count.to_le_bytes());
    for processor_id in &c.unexpected_processor_ids {
        out.extend_from_slice(&processor_id.to_le_bytes());
    }
    out.extend_from_slice(&missing_count.to_le_bytes());
    for processor_id in &c.missing_processor_ids {
        out.extend_from_slice(&processor_id.to_le_bytes());
    }
    out.extend_from_slice(&duplicate_count.to_le_bytes());
    for processor_id in &c.duplicate_processor_ids {
        out.extend_from_slice(&processor_id.to_le_bytes());
    }
    out.extend_from_slice(&processor_count.to_le_bytes());
    let mut processors = c.processors.iter().collect::<Vec<_>>();
    processors.sort_by_key(|p| p.processor_id);
    for p in processors {
        out.extend_from_slice(&p.processor_id.to_le_bytes());
        append_digest_evidence(&mut out, p.expected_software);
        append_digest_evidence(&mut out, p.observed_software);
        append_digest_evidence(&mut out, p.expected_adaptation);
        append_digest_evidence(&mut out, p.observed_adaptation);
    }
    if out.len() > oa_audit_log::DETAILS_CAPACITY {
        return Err(CertificationError::Invalid(
            "certification details exceed audit record limit",
        ));
    }
    Ok(out)
}

fn append_digest_evidence(out: &mut Vec<u8>, evidence: DigestEvidence) {
    out.push(evidence.state as u8);
    if let Some(digest) = evidence.sha256 {
        out.extend_from_slice(&digest);
    }
}

fn decode_details(bytes: &[u8]) -> Result<String, CertificationError> {
    let mut cursor = 0usize;
    let magic = take(bytes, &mut cursor, DETAILS_MAGIC.len())?;
    let schema_v6 = magic == DETAILS_MAGIC_V6;
    let schema_v5 = schema_v6 || magic == DETAILS_MAGIC_V5;
    let schema_v4 = schema_v5 || magic == DETAILS_MAGIC_V4;
    let schema_v3 = schema_v4 || magic == DETAILS_MAGIC;
    let schema_v2 = if schema_v3 || magic == DETAILS_MAGIC_V2 {
        true
    } else if magic == DETAILS_MAGIC_V1 {
        false
    } else {
        return Err(CertificationError::Invalid(
            "audit detail schema is unsupported",
        ));
    };
    let outcome = match read_u8(bytes, &mut cursor)? {
        1 => "certified",
        2 => "certified_with_exceptions",
        3 => "not_certified",
        _ => return Err(CertificationError::Invalid("audit outcome is invalid")),
    };
    let mode = match read_u8(bytes, &mut cursor)? {
        1 => "full",
        2 => "abbreviated",
        _ => return Err(CertificationError::Invalid("audit PVER mode is invalid")),
    };
    let passed = match read_u8(bytes, &mut cursor)? {
        0 => "false",
        1 => "true",
        _ => return Err(CertificationError::Invalid("audit PVER result is invalid")),
    };
    let checked = read_u16(bytes, &mut cursor)?;
    let passed_targets = read_u16(bytes, &mut cursor)?;
    let losses = read_u64(bytes, &mut cursor)?;
    let refusals = read_u64(bytes, &mut cursor)?;
    let catalog_digest = read_digest(bytes, &mut cursor)?;
    let (catalog_version, adaptation_version) = if schema_v6 {
        let len = usize::from(read_u8(bytes, &mut cursor)?);
        if len == 0 || len > 64 {
            return Err(CertificationError::Invalid(
                "audit catalog version length is invalid",
            ));
        }
        let version = std::str::from_utf8(take(bytes, &mut cursor, len)?)
            .map_err(|_| CertificationError::Invalid("audit catalog version is not UTF-8"))?;
        let issued = read_u64(bytes, &mut cursor)?;
        (Some(version.to_owned()), (issued != 0).then_some(issued))
    } else {
        (None, None)
    };
    let (tracker_config_digest, runner_build_digest, target_report) = if schema_v2 {
        let config_digest = read_digest(bytes, &mut cursor)?;
        let runner_digest = read_digest(bytes, &mut cursor)?;
        let (registry_digest, authorization_level) = if schema_v3 {
            let registry = read_digest(bytes, &mut cursor)?;
            let level = match read_u8(bytes, &mut cursor)? {
                1 => "configure_claim",
                _ => {
                    return Err(CertificationError::Invalid(
                        "audit authorization claim is invalid",
                    ))
                }
            };
            (Some(registry), level)
        } else {
            (None, "unbound_legacy")
        };
        let expected_inventory_digest = if schema_v4 {
            let digest = read_digest(bytes, &mut cursor)?;
            if digest == [0; 32] {
                return Err(CertificationError::Invalid(
                    "audit expected roster digest is absent",
                ));
            }
            Some(digest)
        } else {
            None
        };
        let runner_artifact = read_digest_evidence(bytes, &mut cursor)?;
        if !digest_evidence_consistent(runner_artifact) {
            return Err(CertificationError::Invalid(
                "audit runner artifact evidence is inconsistent",
            ));
        }
        let target_count = usize::from(read_u8(bytes, &mut cursor)?);
        if target_count != usize::from(checked) || target_count > MAX_CATALOG_TARGETS {
            return Err(CertificationError::Invalid(
                "audit PVER target result count is invalid",
            ));
        }
        let mut lines = String::new();
        if let Some(registry) = registry_digest {
            use std::fmt::Write as _;
            let _ = writeln!(
                &mut lines,
                "authorization_level_claim={authorization_level}\nauthorization_registry_sha256={}",
                oa_sha256::to_hex(&registry)
            );
        }
        if let Some(inventory) = expected_inventory_digest {
            use std::fmt::Write as _;
            let _ = writeln!(
                &mut lines,
                "expected_inventory_sha256={}",
                oa_sha256::to_hex(&inventory)
            );
        }
        let mut ids = Vec::with_capacity(target_count);
        for _ in 0..target_count {
            let id_len = usize::from(read_u8(bytes, &mut cursor)?);
            if id_len == 0 || id_len > 32 {
                return Err(CertificationError::Invalid(
                    "audit PVER target ID length is invalid",
                ));
            }
            let id = std::str::from_utf8(take(bytes, &mut cursor, id_len)?)
                .map_err(|_| CertificationError::Invalid("audit PVER target ID is not UTF-8"))?;
            if id
                .bytes()
                .any(|byte| !byte.is_ascii_alphanumeric() && byte != b'-')
                || ids.iter().any(|existing| existing == id)
            {
                return Err(CertificationError::Invalid(
                    "audit PVER target ID is invalid or duplicate",
                ));
            }
            ids.push(id.to_owned());
            let state = match read_u8(bytes, &mut cursor)? {
                1 => "missing_output",
                2 => "passed",
                3 => "position_exceeded",
                4 => "velocity_exceeded",
                5 => "both_exceeded",
                _ => {
                    return Err(CertificationError::Invalid(
                        "audit PVER target state is invalid",
                    ))
                }
            };
            let output_time_ns = read_i64(bytes, &mut cursor)?;
            let position_error_um = read_u64(bytes, &mut cursor)?;
            let position_tolerance_um = read_u64(bytes, &mut cursor)?;
            let velocity_error_um_s = read_u64(bytes, &mut cursor)?;
            let velocity_tolerance_um_s = read_u64(bytes, &mut cursor)?;
            let missing = state == "missing_output";
            if position_tolerance_um == 0
                || velocity_tolerance_um_s == 0
                || (missing
                    && (output_time_ns != 0 || position_error_um != 0 || velocity_error_um_s != 0))
                || (!missing && output_time_ns <= 0)
            {
                return Err(CertificationError::Invalid(
                    "audit PVER target evidence is inconsistent",
                ));
            }
            use std::fmt::Write as _;
            let _ = writeln!(
                &mut lines,
                "pver_target={id},state={state},output_time_ns={},position_error_um={},position_tolerance_um={},velocity_error_um_s={},velocity_tolerance_um_s={}",
                if missing { "absent".to_owned() } else { output_time_ns.to_string() },
                if missing { "absent".to_owned() } else { position_error_um.to_string() },
                position_tolerance_um,
                if missing { "absent".to_owned() } else { velocity_error_um_s.to_string() },
                velocity_tolerance_um_s,
            );
        }
        (Some(config_digest), Some(runner_digest), Some(lines))
    } else {
        (None, None, None)
    };
    let adaptation_evidence = read_digest_evidence(bytes, &mut cursor)?;
    let binding_digest = read_optional_digest(bytes, &mut cursor)?;
    let exception_len = usize::from(read_u16(bytes, &mut cursor)?);
    if exception_len > MAX_EXCEPTION_BYTES {
        return Err(CertificationError::Invalid(
            "audit exception text exceeds its bound",
        ));
    }
    let exceptions = std::str::from_utf8(take(bytes, &mut cursor, exception_len)?)
        .map_err(|_| CertificationError::Invalid("audit exception text is not UTF-8"))?;
    let expected_count = usize::from(read_u16(bytes, &mut cursor)?);
    if expected_count == 0 || expected_count > 128 {
        return Err(CertificationError::Invalid(
            "audit expected inventory count is invalid",
        ));
    }
    let mut expected_ids = Vec::with_capacity(expected_count);
    for _ in 0..expected_count {
        expected_ids.push(read_u32(bytes, &mut cursor)?);
    }
    let observed_ids = if schema_v5 {
        let observed_count = usize::from(read_u16(bytes, &mut cursor)?);
        if observed_count > 128 {
            return Err(CertificationError::Invalid(
                "audit observed inventory count is invalid",
            ));
        }
        let mut observed = Vec::with_capacity(observed_count);
        for _ in 0..observed_count {
            observed.push(read_u32(bytes, &mut cursor)?);
        }
        observed
    } else {
        Vec::new()
    };
    let unexpected_count = usize::from(read_u16(bytes, &mut cursor)?);
    if unexpected_count > 128 {
        return Err(CertificationError::Invalid(
            "audit unexpected inventory count is invalid",
        ));
    }
    let mut unexpected_ids = Vec::with_capacity(unexpected_count);
    for _ in 0..unexpected_count {
        unexpected_ids.push(read_u32(bytes, &mut cursor)?);
    }
    let (missing_ids, duplicate_ids) = if schema_v4 {
        let missing_count = usize::from(read_u16(bytes, &mut cursor)?);
        if missing_count > 128 {
            return Err(CertificationError::Invalid(
                "audit missing inventory count is invalid",
            ));
        }
        let mut missing = Vec::with_capacity(missing_count);
        for _ in 0..missing_count {
            missing.push(read_u32(bytes, &mut cursor)?);
        }
        let duplicate_count = usize::from(read_u16(bytes, &mut cursor)?);
        if duplicate_count > 128 {
            return Err(CertificationError::Invalid(
                "audit duplicate inventory count is invalid",
            ));
        }
        let mut duplicate = Vec::with_capacity(duplicate_count);
        for _ in 0..duplicate_count {
            duplicate.push(read_u32(bytes, &mut cursor)?);
        }
        (missing, duplicate)
    } else {
        (Vec::new(), Vec::new())
    };
    if schema_v5 {
        let expected_sorted = expected_ids.windows(2).all(|pair| {
            pair.first()
                .zip(pair.get(1))
                .is_some_and(|(left, right)| left < right)
        });
        let observed_sorted = observed_ids.windows(2).all(|pair| {
            pair.first()
                .zip(pair.get(1))
                .is_some_and(|(left, right)| left <= right)
        });
        let missing_sorted = missing_ids.windows(2).all(|pair| {
            pair.first()
                .zip(pair.get(1))
                .is_some_and(|(left, right)| left < right)
        });
        let duplicate_sorted = duplicate_ids.windows(2).all(|pair| {
            pair.first()
                .zip(pair.get(1))
                .is_some_and(|(left, right)| left < right)
        });
        let unexpected_sorted = unexpected_ids.windows(2).all(|pair| {
            pair.first()
                .zip(pair.get(1))
                .is_some_and(|(left, right)| left < right)
        });
        let derived_missing = expected_ids
            .iter()
            .filter(|id| observed_ids.binary_search(id).is_err())
            .copied()
            .collect::<Vec<_>>();
        let mut derived_duplicate = observed_ids
            .windows(2)
            .filter_map(|pair| {
                pair.first()
                    .zip(pair.get(1))
                    .filter(|(a, b)| a == b)
                    .map(|(a, _)| *a)
            })
            .collect::<Vec<_>>();
        derived_duplicate.dedup();
        let mut derived_unexpected = observed_ids
            .iter()
            .filter(|id| expected_ids.binary_search(id).is_err())
            .copied()
            .collect::<Vec<_>>();
        derived_unexpected.dedup();
        if expected_ids.contains(&0)
            || observed_ids.contains(&0)
            || !expected_sorted
            || !observed_sorted
            || !missing_sorted
            || !duplicate_sorted
            || !unexpected_sorted
            || derived_missing != missing_ids
            || derived_duplicate != duplicate_ids
            || derived_unexpected != unexpected_ids
        {
            return Err(CertificationError::Invalid(
                "audit processor findings do not match the preserved roster and observations",
            ));
        }
    }
    let count = usize::from(read_u16(bytes, &mut cursor)?);
    if count == 0 || count > 128 {
        return Err(CertificationError::Invalid(
            "audit processor inventory count is invalid",
        ));
    }
    use std::fmt::Write as _;
    let mut report = String::new();
    let _ = writeln!(
        &mut report,
        "outcome={outcome}\npver_mode={mode}\npver_passed={passed}\ntargets={passed_targets}/{checked}\ntracker_loss_rows={losses}\ntracker_refusals={refusals}\ncatalog_version={}\ncatalog_adaptation_version={}\ncatalog_sha256={}\ntracker_config_sha256={}\nrunner_build_claim_sha256={}\nadaptation_state={:?}\nadaptation_sha256={}\nprofile_binding_sha256={}\nexpected_processors={expected_ids:?}\nobserved_processors={observed_ids:?}\nmissing_processors={missing_ids:?}\nduplicate_processors={duplicate_ids:?}\nunexpected_processors={unexpected_ids:?}\nexceptions={}",
        catalog_version.as_deref().unwrap_or("legacy_unrecorded"),
        adaptation_version.map_or_else(|| "unrecorded".to_owned(), |v| v.to_string()),
        oa_sha256::to_hex(&catalog_digest),
        tracker_config_digest.map_or_else(|| "unbound_v1".to_owned(), |digest| oa_sha256::to_hex(&digest)),
        runner_build_digest.map_or_else(|| "unbound_v1".to_owned(), |digest| oa_sha256::to_hex(&digest)),
        adaptation_evidence.state,
        digest_text(adaptation_evidence.sha256),
        digest_text(binding_digest),
        escape_report(exceptions),
    );
    if let Some(targets) = target_report {
        report.push_str(&targets);
    }
    for _ in 0..count {
        let processor = read_u32(bytes, &mut cursor)?;
        let expected_software = read_digest_evidence(bytes, &mut cursor)?;
        let software = read_digest_evidence(bytes, &mut cursor)?;
        let expected_adaptation = read_digest_evidence(bytes, &mut cursor)?;
        let adaptation = read_digest_evidence(bytes, &mut cursor)?;
        let _ = writeln!(
            &mut report,
            "processor={processor},software_expected_state={:?},software_expected_sha256={},software_observed_state={:?},software_observed_sha256={},adaptation_expected_state={:?},adaptation_expected_sha256={},adaptation_observed_state={:?},adaptation_observed_sha256={}",
            expected_software.state, digest_text(expected_software.sha256),
            software.state, digest_text(software.sha256),
            expected_adaptation.state, digest_text(expected_adaptation.sha256),
            adaptation.state, digest_text(adaptation.sha256)
        );
    }
    if cursor != bytes.len() {
        return Err(CertificationError::Invalid(
            "audit details have trailing bytes",
        ));
    }
    Ok(report)
}

fn take<'a>(
    bytes: &'a [u8],
    cursor: &mut usize,
    count: usize,
) -> Result<&'a [u8], CertificationError> {
    let end = cursor
        .checked_add(count)
        .ok_or(CertificationError::Invalid("audit details offset overflow"))?;
    let value = bytes
        .get(*cursor..end)
        .ok_or(CertificationError::Invalid("audit details are truncated"))?;
    *cursor = end;
    Ok(value)
}

fn read_u8(bytes: &[u8], cursor: &mut usize) -> Result<u8, CertificationError> {
    take(bytes, cursor, 1)?
        .first()
        .copied()
        .ok_or(CertificationError::Invalid("audit details are truncated"))
}

fn read_u16(bytes: &[u8], cursor: &mut usize) -> Result<u16, CertificationError> {
    let raw = take(bytes, cursor, 2)?;
    let a = raw
        .first()
        .copied()
        .ok_or(CertificationError::Invalid("audit details are truncated"))?;
    let b = raw
        .get(1)
        .copied()
        .ok_or(CertificationError::Invalid("audit details are truncated"))?;
    Ok(u16::from_le_bytes([a, b]))
}

fn read_u32(bytes: &[u8], cursor: &mut usize) -> Result<u32, CertificationError> {
    let raw = take(bytes, cursor, 4)?;
    let mut out = [0u8; 4];
    out.copy_from_slice(raw);
    Ok(u32::from_le_bytes(out))
}

fn read_u64(bytes: &[u8], cursor: &mut usize) -> Result<u64, CertificationError> {
    let raw = take(bytes, cursor, 8)?;
    let mut out = [0u8; 8];
    out.copy_from_slice(raw);
    Ok(u64::from_le_bytes(out))
}

fn read_i64(bytes: &[u8], cursor: &mut usize) -> Result<i64, CertificationError> {
    let raw = take(bytes, cursor, 8)?;
    let mut out = [0u8; 8];
    out.copy_from_slice(raw);
    Ok(i64::from_le_bytes(out))
}

fn read_digest(bytes: &[u8], cursor: &mut usize) -> Result<oa_sha256::Digest, CertificationError> {
    let raw = take(bytes, cursor, 32)?;
    let mut out = [0u8; 32];
    out.copy_from_slice(raw);
    Ok(out)
}

fn read_optional_digest(
    bytes: &[u8],
    cursor: &mut usize,
) -> Result<Option<oa_sha256::Digest>, CertificationError> {
    match read_u8(bytes, cursor)? {
        value if value == EvidenceState::Present as u8 => Ok(Some(read_digest(bytes, cursor)?)),
        value if value == EvidenceState::Missing as u8 => Ok(None),
        _ => Err(CertificationError::Invalid(
            "audit digest presence marker is invalid",
        )),
    }
}

fn read_digest_evidence(
    bytes: &[u8],
    cursor: &mut usize,
) -> Result<DigestEvidence, CertificationError> {
    let state = match read_u8(bytes, cursor)? {
        value if value == EvidenceState::Present as u8 => EvidenceState::Present,
        value if value == EvidenceState::Missing as u8 => EvidenceState::Missing,
        value if value == EvidenceState::Refused as u8 => EvidenceState::Refused,
        value if value == EvidenceState::Invalid as u8 => EvidenceState::Invalid,
        _ => {
            return Err(CertificationError::Invalid(
                "audit evidence state is invalid",
            ))
        }
    };
    let sha256 = if state == EvidenceState::Present {
        Some(read_digest(bytes, cursor)?)
    } else {
        None
    };
    Ok(DigestEvidence { state, sha256 })
}

fn digest_text(digest: Option<oa_sha256::Digest>) -> String {
    digest.map_or_else(|| "absent".to_owned(), |value| oa_sha256::to_hex(&value))
}

fn escape_report(text: &str) -> String {
    text.chars().flat_map(char::escape_default).collect()
}

/// Generate a plain text report from audit events after verifying the caller's trusted checkpoint.
pub fn certification_report(
    path: &Path,
    expected: oa_audit_core::Checkpoint,
) -> Result<String, CertificationError> {
    oa_sealed::refuse(path).map_err(CertificationError::SealedPath)?;
    let mut records = Vec::new();
    let mut too_many = false;
    oa_audit_log::shell::read_records(path, expected, &mut |record| {
        if record.event.action == oa_audit_core::Action::Certification {
            if records.len() < MAX_REPORT_CERTIFICATIONS {
                records.push(record);
            } else {
                too_many = true;
            }
        }
    })
    .map_err(CertificationError::Audit)?;
    if too_many {
        return Err(CertificationError::Invalid(
            "report exceeds the fixed 256-record bound",
        ));
    }
    let mut report = String::from("OpenATCA synthetic certification log report\nCaller identities, timestamps, local authorization-registry roles, and runner build claims are unverified claims; no login, loaded-image attestation, live PVER, or operational certification is asserted.\n");
    for record in records {
        let details = decode_details(
            record
                .event
                .details
                .get(..usize::from(record.event.details_len))
                .unwrap_or_default(),
        )?;
        use std::fmt::Write as _;
        let _ = writeln!(
            &mut report,
            "record={} principal_claim={} time_ns_claim={} timestamp_source=caller_supplied_utc_claim service={}\n{}",
            record.sequence,
            record.event.principal,
            record.event.time_ns,
            String::from_utf8_lossy(
                record
                    .event
                    .target
                    .get(..usize::from(record.event.target_len))
                    .unwrap_or_default()
            ),
            details,
        );
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    fn processor() -> ProcessorEvidence {
        ProcessorEvidence {
            processor_id: 7,
            expected_software: DigestEvidence::present([1; 32]),
            observed_software: DigestEvidence::present([1; 32]),
            expected_adaptation: DigestEvidence::present([2; 32]),
            observed_adaptation: DigestEvidence::present([2; 32]),
        }
    }

    fn certification(pver: PverResult, exceptions: &str) -> Certification {
        let mut record = decide_certification(
            11,
            1_800_000_000_000_000_000,
            "synthetic-service".to_owned(),
            pver,
            vec![7],
            vec![processor()],
            exceptions.to_owned(),
        )
        .expect("valid synthetic certification");
        record.authorization_registry_sha256 = [3; 32];
        record.expected_inventory_sha256 = [4; 32];
        record
    }

    fn test_pver(mode: PverMode) -> PverResult {
        run_catalog(
            &synthetic_catalog().expect("catalog validates"),
            mode,
            DigestEvidence::present([2; 32]),
        )
        .expect("synthetic tracker run")
    }

    fn log_path() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "oa-certification-{}-{}.audit",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }

    // REQ: OA-CERT-008
    #[test]
    fn local_registry_claim_requires_unique_configure_row_and_binds_exact_bytes() {
        let registry = b"OACERT-LOCAL-PRINCIPALS-V1\n11\tconfigure\n12\tmonitor\n";
        let claim = authorize_configure_claim(registry, 11).expect("configure claim parses");
        assert_eq!(claim.principal_id, 11);
        assert_eq!(claim.registry_sha256, oa_sha256::sha256(registry));
        assert!(authorize_configure_claim(registry, 12).is_err());
        assert!(authorize_configure_claim(
            b"OACERT-LOCAL-PRINCIPALS-V1\n11\tconfigure\n11\tconfigure\n",
            11
        )
        .is_err());
    }

    // REQ: OA-CERT-007, OA-CERT-008
    #[test]
    fn request_caps_are_refused_before_any_processor_path_is_opened() {
        let registry = b"OACERT-LOCAL-PRINCIPALS-V1\n11\tconfigure\n";
        let authorization = authorize_configure_claim(registry, 11).expect("claim");
        let request = CertificationRequest {
            authorization,
            time_ns: 1,
            service_id: "synthetic".to_owned(),
            mode: PverMode::Full,
            expected_processor_ids: vec![7],
            expected_inventory_sha256: [4; 32],
            processors: vec![ProcessorSource {
                processor_id: 7,
                expected_software: DigestEvidence::missing(),
                software_artifact_path: Some("/definitely/not/read".into()),
                expected_adaptation: DigestEvidence::missing(),
                adaptation_package: Some(("/definitely/not/read".into(), [0; 32])),
            }],
            exceptions: String::new(),
            max_software_bytes: MAX_SOFTWARE_ARTIFACT_BYTES + 1,
        };
        assert!(matches!(
            prepare_certification(request),
            Err(CertificationError::Invalid(_))
        ));
    }

    // REQ: OA-CERT-001
    #[test]
    fn fixed_catalog_is_content_pinned_and_abbreviated_is_exact_three_target_subset() {
        let catalog = synthetic_catalog().expect("catalog validates");
        assert_eq!(catalog.sha256, CATALOG_SHA256);
        assert_eq!(catalog.full.len(), 6);
        assert_eq!(catalog.abbreviated.len(), 3);
        assert!(catalog
            .abbreviated
            .iter()
            .all(|target| catalog.full.iter().any(|full| full.id == target.id)));
        let abbreviated_ids = catalog
            .abbreviated
            .iter()
            .map(|target| target.id.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            abbreviated_ids.len(),
            3,
            "declared abbreviated targets are unique"
        );
    }

    // REQ: OA-CERT-002
    #[test]
    fn pver_thresholds_use_rounded_micro_units_at_the_boundary() {
        let tolerance_um = micro_units(1.0).expect("one meter converts to micro-units");
        let just_below_rounding_boundary = micro_units(1.000_000_49).expect("finite residual");
        let just_above_rounding_boundary = micro_units(1.000_000_51).expect("finite residual");
        assert!(within_micro_tolerance(
            just_below_rounding_boundary,
            tolerance_um
        ));
        assert!(!within_micro_tolerance(
            just_above_rounding_boundary,
            tolerance_um
        ));
    }

    // REQ: OA-CERT-002, OA-CERT-009
    #[test]
    fn full_and_abbreviated_pver_measure_real_tracker_position_and_velocity_outputs() {
        let full = test_pver(PverMode::Full);
        assert!(full.passed(), "full PVER: {full:?}");
        assert_eq!(full.targets_checked(), 6);
        assert_eq!(full.targets_passed(), 6);
        assert_eq!(full.tracker_refusals(), 0);
        assert_eq!(full.tracker_loss_rows(), 0);
        assert_ne!(full.tracker_config_sha256(), [0; 32]);
        assert_ne!(full.runner_build_claim_sha256(), [0; 32]);
        assert_eq!(full.runner_artifact_state(), EvidenceState::Present);
        assert_eq!(full.target_results().len(), 6);
        assert!(full
            .target_results()
            .iter()
            .all(|row| row.output_time_ns.is_some()));

        let abbreviated = test_pver(PverMode::Abbreviated);
        assert!(abbreviated.passed(), "abbreviated PVER: {abbreviated:?}");
        assert_eq!(abbreviated.targets_checked(), 3);
        assert_eq!(abbreviated.targets_passed(), 3);
    }

    // REQ: OA-CERT-002
    #[test]
    fn actual_tracker_output_outside_tolerance_fails_pver() {
        let mut catalog = synthetic_catalog().expect("catalog validates");
        if let Some(target) = catalog.full.first_mut() {
            target.position_tolerance_m = 0.0001;
        }
        let result = run_catalog(&catalog, PverMode::Full, DigestEvidence::present([2; 32]))
            .expect("PVER completes with check failure");
        assert!(!result.passed());
        assert!(result.targets_passed() < result.targets_checked());
    }

    // REQ: OA-CERT-003, OA-CERT-004
    #[test]
    fn all_three_outcomes_are_defined_and_any_processor_digest_mismatch_denies_certification() {
        let passed = test_pver(PverMode::Full);
        assert_eq!(
            certification(passed.clone(), "").outcome,
            CertificationOutcome::Certified
        );
        assert_eq!(
            certification(passed.clone(), "reviewed limitation").outcome,
            CertificationOutcome::CertifiedWithExceptions
        );

        let mut failed = test_pver(PverMode::Full);
        failed.passed = false;
        failed.tracker_loss_rows = 1;
        assert_eq!(
            certification(failed, "").outcome,
            CertificationOutcome::NotCertified
        );

        let mut mismatched = processor();
        mismatched.observed_software = DigestEvidence::present([9; 32]);
        let denied = decide_certification(
            11,
            1_800_000_000_000_000_000,
            "synthetic-service".to_owned(),
            passed.clone(),
            vec![7],
            vec![mismatched],
            String::new(),
        )
        .expect("record created");
        assert_eq!(denied.outcome, CertificationOutcome::NotCertified);

        let mut malformed = processor();
        malformed.observed_software = DigestEvidence {
            state: EvidenceState::Present,
            sha256: None,
        };
        let denied = decide_certification(
            11,
            1_800_000_000_000_000_000,
            "synthetic-service".to_owned(),
            passed.clone(),
            vec![7],
            vec![malformed],
            String::new(),
        )
        .expect("malformed digest evidence is retained as invalid");
        assert_eq!(denied.outcome, CertificationOutcome::NotCertified);
        assert_eq!(
            denied.processors.first().map(|p| p.observed_software.state),
            Some(EvidenceState::Invalid)
        );

        let mut adaptation_mismatch = processor();
        adaptation_mismatch.observed_adaptation = DigestEvidence::present([9; 32]);
        let denied = decide_certification(
            11,
            1_800_000_000_000_000_000,
            "synthetic-service".to_owned(),
            passed,
            vec![7],
            vec![adaptation_mismatch],
            String::new(),
        )
        .expect("record created");
        assert_eq!(denied.outcome, CertificationOutcome::NotCertified);
    }

    // REQ: OA-CERT-005, OA-CERT-006, OA-CERT-010
    #[test]
    fn certification_records_are_audited_and_report_is_generated_from_verified_log() {
        let path = log_path();
        let certified = certification(test_pver(PverMode::Abbreviated), "");
        let mut failed_pver = test_pver(PverMode::Full);
        failed_pver.passed = false;
        failed_pver.tracker_loss_rows = 1;
        let not_certified = certification(failed_pver, "");
        let exception = certification(
            test_pver(PverMode::Abbreviated),
            "scoped synthetic exception",
        );
        let (first, receipt) =
            append_certification_with_receipt(&path, oa_audit_core::genesis(), &certified)
                .expect("audit append succeeds");
        assert_eq!(receipt.sequence(), first.count);
        assert_eq!(receipt.role(), oa_audit_core::Role::Configure);
        let (second, receipt) = append_certification_with_receipt(&path, first, &exception)
            .expect("exception outcome audit succeeds");
        assert_eq!(receipt.sequence(), second.count);
        let (checkpoint, receipt) =
            append_certification_with_receipt(&path, second, &not_certified)
                .expect("not-certified outcome audit succeeds");
        assert_eq!(receipt.sequence(), checkpoint.count);
        let report = certification_report(&path, checkpoint).expect("verified report");
        assert!(report.contains("outcome=certified"));
        assert!(report.contains(&format!(
            "expected_inventory_sha256={}",
            oa_sha256::to_hex(&[4; 32])
        )));
        assert!(report.contains("outcome=certified_with_exceptions"));
        assert!(report.contains("outcome=not_certified"));
        assert!(report.contains("processor=7"));
        assert!(report.contains("observed_processors=[7]"));
        assert!(report.contains("timestamp_source=caller_supplied_utc_claim"));
        assert!(report.contains("profile_binding_sha256="));
        assert!(report.contains("authorization_level_claim=configure_claim"));
        assert!(report.contains("pver_target="));
        let _ = std::fs::remove_file(path);
    }

    // REQ: OA-CERT-003, OA-CERT-004, OA-CERT-005, OA-CERT-006
    #[test]
    fn missing_processor_and_refused_g2_evidence_are_logged_as_not_certified() {
        let pver = test_pver(PverMode::Full);
        let mut missing = decide_certification(
            11,
            1_800_000_000_000_000_000,
            "synthetic-service".to_owned(),
            pver,
            vec![7, 8],
            vec![processor()],
            String::new(),
        )
        .expect("missing expected processor is recorded");
        missing.authorization_registry_sha256 = [3; 32];
        missing.expected_inventory_sha256 = [5; 32];
        assert_eq!(missing.outcome, CertificationOutcome::NotCertified);
        assert_eq!(missing.processors.get(1).map(|p| p.processor_id), Some(8));
        assert_eq!(
            missing.processors.get(1).map(|p| p.observed_software.state),
            Some(EvidenceState::Missing)
        );
        let mut missing_finding_removed = missing.clone();
        missing_finding_removed.missing_processor_ids.clear();
        let refused_path = log_path();
        assert!(append_certification(
            &refused_path,
            oa_audit_core::genesis(),
            &missing_finding_removed,
        )
        .is_err());
        assert!(!refused_path.exists());

        let refused_pver = run_catalog(
            &synthetic_catalog().expect("test catalog is valid"),
            PverMode::Full,
            DigestEvidence::refused(),
        )
        .expect("synthetic test runner retains refused adaptation binding");
        assert!(!refused_pver.passed());
        assert_eq!(refused_pver.adaptation_state(), EvidenceState::Refused);
        let mut refused_processor = processor();
        refused_processor.observed_adaptation = DigestEvidence::refused();
        let mut refused = decide_certification(
            11,
            1_800_000_000_000_000_000,
            "synthetic-service".to_owned(),
            refused_pver,
            vec![7],
            vec![refused_processor],
            String::new(),
        )
        .expect("refused adaptation observation is recorded");
        refused.authorization_registry_sha256 = [3; 32];
        refused.expected_inventory_sha256 = [6; 32];
        assert_eq!(refused.outcome, CertificationOutcome::NotCertified);

        let path = log_path();
        let checkpoint = append_certification(&path, oa_audit_core::genesis(), &missing)
            .expect("not-certified evidence is still auditable");
        let checkpoint = append_certification(&path, checkpoint, &refused)
            .expect("refused G2 evidence is still auditable");
        let report = certification_report(&path, checkpoint).expect("verified refusal report");
        assert!(report.contains("outcome=not_certified"));
        assert!(report.contains("software_observed_state=Missing"));
        assert!(report.contains("adaptation_observed_state=Refused"));
        assert!(report.contains("missing_processors=[8]"));
        assert!(report.contains("observed_processors=[7]"));
        let _ = std::fs::remove_file(path);
    }

    // REQ: OA-CERT-001
    #[test]
    fn bound_pver_refuses_package_without_issued_catalog() {
        let legacy_snapshot = include_bytes!("../../oa-ca/tests/fixtures/a3-empty-synthetic.oag2");
        let model = oa_airspace::snapshot::decode(legacy_snapshot)
            .expect("legacy synthetic snapshot decodes");
        let snapshot =
            oa_airspace::snapshot::encode(&model).expect("current canonical synthetic snapshot");
        let root = std::env::temp_dir().join(format!("oa-cert-no-catalog-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let pin =
            oa_adapt::manifest::write_new_package(&root, &snapshot, "release", "ZAU", "change")
                .expect("canonical legacy package fixture");
        let verified = oa_adapt::manifest::verify_current(&root, pin)
            .expect("canonical package without catalog verifies");
        assert!(verified.pver_catalog_bytes().is_none());
        assert!(matches!(
            run_bound_performance(PverMode::Full, &root, pin),
            Err(CertificationError::Catalog(
                "verified package has no issued PVER catalog"
            ))
        ));
        let _ = std::fs::remove_dir_all(root);
    }

    // REQ: OA-CERT-004, OA-CERT-005, OA-CERT-006
    #[test]
    fn unexpected_processor_is_retained_and_forces_not_certified() {
        let mut unexpected = processor();
        unexpected.processor_id = 9;
        let mut record = decide_certification(
            11,
            1_800_000_000_000_000_000,
            "synthetic-service".to_owned(),
            test_pver(PverMode::Full),
            vec![7],
            vec![unexpected],
            String::new(),
        )
        .expect("unexpected processor is recorded");
        record.authorization_registry_sha256 = [3; 32];
        record.expected_inventory_sha256 = [7; 32];
        assert_eq!(record.outcome, CertificationOutcome::NotCertified);
        assert_eq!(record.unexpected_processor_ids, vec![9]);
        let path = log_path();
        let checkpoint = append_certification(&path, oa_audit_core::genesis(), &record)
            .expect("unexpected processor audit succeeds");
        let report = certification_report(&path, checkpoint).expect("verified report");
        assert!(report.contains("unexpected_processors=[9]"));
        assert!(report.contains("processor=9"));
        let _ = std::fs::remove_file(path);
    }

    // REQ: OA-CERT-008
    #[test]
    fn omitted_expected_processor_is_retained_as_missing_and_forces_not_certified() {
        let record = decide_certification(
            11,
            1_800_000_000_000_000_000,
            "synthetic-service".to_owned(),
            test_pver(PverMode::Full),
            vec![7, 8],
            vec![processor()],
            String::new(),
        )
        .expect("missing expected processor is recorded");
        assert_eq!(record.outcome, CertificationOutcome::NotCertified);
        assert_eq!(record.processors.len(), 2);
        assert_eq!(record.processors[1].processor_id, 8);
        assert_eq!(
            record.processors[1].expected_software.state,
            EvidenceState::Missing
        );
        assert_eq!(record.missing_processor_ids, vec![8]);
    }

    // REQ: OA-CERT-008
    #[test]
    fn duplicate_observation_is_named_and_forces_not_certified() {
        let mut record = decide_certification(
            11,
            1_800_000_000_000_000_000,
            "synthetic-service".to_owned(),
            test_pver(PverMode::Full),
            vec![7],
            vec![processor(), processor()],
            String::new(),
        )
        .expect("duplicate observation remains in the failed evidence record");
        record.authorization_registry_sha256 = [3; 32];
        record.expected_inventory_sha256 = [4; 32];
        assert_eq!(record.outcome, CertificationOutcome::NotCertified);
        assert_eq!(record.duplicate_processor_ids, vec![7]);
        let refused_path = log_path();
        let mut removed_duplicate = record.clone();
        removed_duplicate.duplicate_processor_ids.clear();
        assert!(
            append_certification(&refused_path, oa_audit_core::genesis(), &removed_duplicate,)
                .is_err()
        );
        let mut invented_duplicate = record.clone();
        invented_duplicate.duplicate_processor_ids = vec![8];
        assert!(
            append_certification(&refused_path, oa_audit_core::genesis(), &invented_duplicate,)
                .is_err()
        );
        assert!(!refused_path.exists());
        let path = log_path();
        let checkpoint = append_certification(&path, oa_audit_core::genesis(), &record)
            .expect("duplicate observation refusal is audited");
        let report = certification_report(&path, checkpoint).expect("verified duplicate report");
        assert!(report.contains("duplicate_processors=[7]"));
        assert!(report.contains("observed_processors=[7, 7]"));
        let _ = std::fs::remove_file(path);
    }

    // REQ: OA-CERT-005, OA-CERT-010
    #[test]
    fn audit_append_failure_returns_no_success_checkpoint() {
        let path = std::env::temp_dir().join(format!(
            "oa-certification-absent-dir-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let file = path.join("events.audit");
        let record = certification(test_pver(PverMode::Full), "");
        assert!(append_certification(&file, oa_audit_core::genesis(), &record).is_err());
    }

    // REQ: OA-CERT-005, OA-CERT-006, OA-CERT-010, OA-SEAL-001, OA-SEAL-002, OA-SEAL-006
    #[test]
    fn sealed_audit_paths_are_refused_before_read_or_append() {
        let sealed =
            std::path::Path::new("/tmp/synthetic/scds/rec-20261119/day2-certification.audit");
        let report = certification_report(sealed, oa_audit_core::genesis());
        assert!(matches!(report, Err(CertificationError::SealedPath(_))));

        let record = certification(test_pver(PverMode::Full), "");
        let append = append_certification(sealed, oa_audit_core::genesis(), &record);
        assert!(matches!(append, Err(CertificationError::SealedPath(_))));
        assert!(!sealed.exists());
    }

    // REQ: OA-CERT-003, OA-CERT-005
    #[test]
    fn maximum_processor_evidence_fits_the_fixed_audit_record() {
        let pver = test_pver(PverMode::Full);
        let processors = (1..=128)
            .map(|processor_id| ProcessorEvidence {
                processor_id,
                expected_software: DigestEvidence::present([1; 32]),
                observed_software: DigestEvidence::present([1; 32]),
                expected_adaptation: DigestEvidence::present([2; 32]),
                observed_adaptation: DigestEvidence::present([2; 32]),
            })
            .collect();
        let record = decide_certification(
            11,
            1_800_000_000_000_000_000,
            "synthetic-service".to_owned(),
            pver,
            (1..=128).collect(),
            processors,
            String::new(),
        )
        .expect("maximal service inventory accepted");
        let details = encode_details(&record).expect("bounded encoding");
        assert!(details.len() <= oa_audit_log::DETAILS_CAPACITY);
    }
}
