//! Offline fleet comparison for issued adaptation identities. Shell only: bounded input reads,
//! guarded paths, and report formatting. Processor values remain caller-supplied claims.
//!
//! The independent expected file is `OA_EXPECTED_PROCESSORS\t2` followed by eight fields:
//! `processor_id process_role source_id firmware_sha256 running_image_sha256 adaptation_sha256 sequence_first sequence_next`.
//! Each report has an `OA_FLEET_PROCESSOR_REPORT\t2` header, then one row with eighteen fields:
//! `processor_id issued_manifest_sha256 loaded_adaptation_sha256 disk_manifest_sha256|- disk_adaptation_sha256|- provenance_unverified local_status source_id epoch clock_domain_id sequence sample_time_ns firmware_sha256 running_image_sha256 telemetry_loss_events loaded_ca_sha256|- loaded_pver_sha256|- loaded_runway_sha256|-`.
//! Hash and identity values in a per-processor report are unverified claims, not an attestation.

use crate::local_inventory::{LocalInventoryReport, LocalInventoryStatus};
use crate::manifest::{parse_pin, verify_current};
use oa_sha256::Digest;
use oa_techops::{
    assess_inventory, compare_adaptation_identity, ClockDomainId, ExpectedProcessor,
    InventoryQuery, ProcessorId, ProcessorReport, ServiceEpoch, SourceId,
    FLAG_ADAPTATION_COMPONENT_INCOMPLETE, FLAG_DISK_ADAPTATION_MISMATCH,
    FLAG_LOADED_ADAPTATION_MISMATCH, FLAG_MISSING_PROCESSOR, FLAG_MISSING_SEQUENCE,
    FLAG_RELEASE_MANIFEST_MISMATCH,
};
use std::fmt;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const MAX_ARGUMENT_BYTES: usize = 4_096;
const FLEET_OPTION_COUNT: usize = 9;
const MAX_FLEET_ARGUMENTS: usize = FLEET_OPTION_COUNT * 2;
// Three path values may each use the existing 4 KiB limit; the manifest digest is 64 ASCII
// hex bytes; five numeric values need at most 20 decimal bytes each; each of the nine fixed
// option names is shorter than 32 bytes. This admits every supported path/value shape.
const MAX_FLEET_OPTION_BYTES: usize = 3 * MAX_ARGUMENT_BYTES + 64 + 5 * 20 + 9 * 32;
const FATAL_INVENTORY_FLAGS: u32 = FLAG_LOADED_ADAPTATION_MISMATCH
    | FLAG_DISK_ADAPTATION_MISMATCH
    | FLAG_RELEASE_MANIFEST_MISMATCH
    | FLAG_MISSING_PROCESSOR
    | FLAG_MISSING_SEQUENCE
    | FLAG_ADAPTATION_COMPONENT_INCOMPLETE;

fn has_fatal_inventory_flag(flags: u32) -> bool {
    flags & FATAL_INVENTORY_FLAGS != 0
}
// 128 rows at worst-case decimal widths, 64-byte hashes, 16-byte role, separators, plus header.
const MAX_EXPECTED_BYTES: usize = 36_864;
const MAX_REPORT_FILES: usize = 512;
const MAX_REPORT_BYTES: usize = 2_048;
// Across all 512 reports, at most 1 MiB of report bytes is admitted; reads are one file at a time.
const REPORT_FIELDS: usize = 18;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProcessRole {
    Generic,
    RegionalSession,
    ConflictAlert,
    Certification,
}

impl ProcessRole {
    fn parse(value: &str) -> Result<Self, FleetError> {
        match value {
            "generic" => Ok(Self::Generic),
            "regional-session" => Ok(Self::RegionalSession),
            "conflict-alert" => Ok(Self::ConflictAlert),
            "certification" => Ok(Self::Certification),
            _ => Err(FleetError::Failed("unknown expected processor role".into())),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Generic => "generic",
            Self::RegionalSession => "regional-session",
            Self::ConflictAlert => "conflict-alert",
            Self::Certification => "certification",
        }
    }

    fn requires(self, component: Component) -> bool {
        match self {
            Self::Generic => true,
            Self::RegionalSession => matches!(component, Component::Runway),
            Self::ConflictAlert => matches!(component, Component::Ca),
            Self::Certification => matches!(component, Component::Pver),
        }
    }
}

#[derive(Clone, Copy)]
enum Component {
    Ca,
    Pver,
    Runway,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ComponentState {
    Match,
    Mismatch,
    Unavailable,
    NotIssued,
    NotApplicable,
}

impl ComponentState {
    fn name(self) -> &'static str {
        match self {
            Self::Match => "MATCH",
            Self::Mismatch => "MISMATCH",
            Self::Unavailable => "UNAVAILABLE",
            Self::NotIssued => "NOT_ISSUED",
            Self::NotApplicable => "NOT_APPLICABLE",
        }
    }
}

#[derive(Clone, Copy)]
struct ExpectedRow {
    processor: ExpectedProcessor,
    role: ProcessRole,
}

/// `oa-adapt fleet-check` input and status failure.
#[derive(Debug)]
pub enum FleetError {
    Usage(String),
    Failed(String),
}

impl fmt::Display for FleetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(message) | Self::Failed(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for FleetError {}

fn failed(step: &str, error: impl fmt::Display) -> FleetError {
    FleetError::Failed(format!("{step}: {error}"))
}

fn read_bounded(path: &Path, limit: usize) -> Result<Vec<u8>, FleetError> {
    let (file, declared_len) = oa_sealed::open_regular(path)
        .map_err(|error| failed(&format!("open {}", path.display()), error))?;
    let max = u64::try_from(limit).map_err(|_| FleetError::Failed("input limit width".into()))?;
    if declared_len > max {
        return Err(FleetError::Failed(format!(
            "{} exceeds the {limit}-byte input limit",
            path.display()
        )));
    }
    let capacity = usize::try_from(declared_len)
        .map_err(|_| FleetError::Failed("input length width".into()))?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(capacity)
        .map_err(|_| FleetError::Failed("input allocation refused".into()))?;
    file.take(max + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| failed(&format!("read {}", path.display()), error))?;
    if bytes.len() > limit || u64::try_from(bytes.len()).ok() != Some(declared_len) {
        return Err(FleetError::Failed(format!(
            "{} changed length while being read",
            path.display()
        )));
    }
    Ok(bytes)
}

fn field_value<'a>(values: &[&'a str], index: usize) -> Result<&'a str, FleetError> {
    values
        .get(index)
        .copied()
        .ok_or_else(|| FleetError::Failed("validated row field is absent".into()))
}

fn parse_u32(text: &str, field: &str) -> Result<u32, FleetError> {
    text.parse::<u32>()
        .map_err(|_| FleetError::Failed(format!("invalid {field}")))
}

fn parse_u64(text: &str, field: &str) -> Result<u64, FleetError> {
    text.parse::<u64>()
        .map_err(|_| FleetError::Failed(format!("invalid {field}")))
}

fn parse_i64(text: &str, field: &str) -> Result<i64, FleetError> {
    text.parse::<i64>()
        .map_err(|_| FleetError::Failed(format!("invalid {field}")))
}

fn parse_digest(text: &str, field: &str) -> Result<Digest, FleetError> {
    parse_pin(text).map_err(|_| FleetError::Failed(format!("invalid {field} SHA-256")))
}

fn fields<'a>(line: &'a str, expected: usize, what: &str) -> Result<Vec<&'a str>, FleetError> {
    let values = line.split('\t').collect::<Vec<_>>();
    if values.len() != expected || values.iter().any(|value| value.is_empty()) {
        return Err(FleetError::Failed(format!(
            "{what} must have exactly {expected} nonempty tab-separated fields"
        )));
    }
    Ok(values)
}

fn read_expected(path: &Path) -> Result<Vec<ExpectedRow>, FleetError> {
    let bytes = read_bounded(path, MAX_EXPECTED_BYTES)?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| FleetError::Failed("expected processor list is not UTF-8".into()))?;
    let mut expected = Vec::new();
    expected
        .try_reserve_exact(oa_techops::MAX_EXPECTED_PROCESSORS)
        .map_err(|_| FleetError::Failed("expected list allocation refused".into()))?;
    let mut lines = text.lines();
    if lines.next() != Some("OA_EXPECTED_PROCESSORS\t2") {
        return Err(FleetError::Failed(
            "expected processor roster needs version 2 header".into(),
        ));
    }
    for line in lines {
        if expected.len() == oa_techops::MAX_EXPECTED_PROCESSORS {
            return Err(FleetError::Failed(
                "expected processor list exceeds 128 rows".into(),
            ));
        }
        let row = fields(line, 8, "expected processor row")?;
        let processor_id = parse_u32(field_value(&row, 0)?, "expected processor ID")?;
        let role = ProcessRole::parse(field_value(&row, 1)?)?;
        let source_id = parse_u32(field_value(&row, 2)?, "expected source ID")?;
        if processor_id == 0 || source_id == 0 {
            return Err(FleetError::Failed(
                "expected processor and source IDs must be nonzero".into(),
            ));
        }
        expected.push(ExpectedRow {
            role,
            processor: ExpectedProcessor {
                processor_id: ProcessorId(processor_id),
                source_id: SourceId(source_id),
                firmware_sha256: parse_digest(field_value(&row, 3)?, "expected firmware")?,
                running_image_sha256: parse_digest(
                    field_value(&row, 4)?,
                    "expected running image",
                )?,
                adaptation_sha256: parse_digest(field_value(&row, 5)?, "expected adaptation")?,
                sequence_first: parse_u64(field_value(&row, 6)?, "expected first sequence")?,
                sequence_next: parse_u64(field_value(&row, 7)?, "expected next sequence")?,
            },
        });
    }
    Ok(expected)
}

struct ReportRow {
    local: LocalInventoryReport,
    context: ProcessorContext,
    loaded_components: crate::local_inventory::LoadedComponents,
}

struct ProcessorContext {
    source_id: SourceId,
    epoch: ServiceEpoch,
    clock_domain_id: ClockDomainId,
    sequence: u64,
    sample_time_ns: i64,
    firmware_sha256: Digest,
    running_image_sha256: Digest,
    telemetry_loss_events: u32,
}

fn processor_report(local: LocalInventoryReport, context: ProcessorContext) -> ProcessorReport {
    ProcessorReport {
        processor_id: ProcessorId(local.processor_id),
        source_id: context.source_id,
        epoch: context.epoch,
        clock_domain_id: context.clock_domain_id,
        sequence: context.sequence,
        sample_time_ns: context.sample_time_ns,
        release_manifest_sha256: local.issued_manifest_sha256,
        firmware_sha256: context.firmware_sha256,
        running_image_sha256: context.running_image_sha256,
        loaded_adaptation_sha256: local.loaded_adaptation_sha256,
        // An unavailable digest is represented by no row for an expected processor, which
        // Tech Ops counts as missing. For an unexpected processor the digest is ignored by the
        // comparator and is not treated as a successful expected check.
        disk_adaptation_sha256: local.disk_adaptation_sha256.unwrap_or([0; 32]),
        telemetry_loss_events: context.telemetry_loss_events,
        adaptation_components_complete: local.adaptation_components_complete.unwrap_or(false),
    }
}

fn local_status(
    issued: Digest,
    loaded: Digest,
    disk_manifest: Option<Digest>,
    disk_adaptation: Option<Digest>,
) -> Result<LocalInventoryStatus, FleetError> {
    match (disk_manifest, disk_adaptation) {
        (None, None) => Ok(LocalInventoryStatus::PackageRefused),
        (Some(disk_pin), Some(disk_digest)) => {
            if disk_pin != issued {
                Ok(LocalInventoryStatus::IssuedManifestMismatch)
            } else if loaded != disk_digest {
                Ok(LocalInventoryStatus::LoadedDiskMismatch)
            } else {
                Ok(LocalInventoryStatus::Match)
            }
        }
        _ => Err(FleetError::Failed(
            "local report must provide both disk digests or neither".into(),
        )),
    }
}

fn read_report(
    path: &Path,
    expected_id: u32,
    identity: &crate::manifest::PackageIdentity,
) -> Result<ReportRow, FleetError> {
    let bytes = read_bounded(path, MAX_REPORT_BYTES)?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| FleetError::Failed("processor report is not UTF-8".into()))?;
    let mut lines = text.lines();
    if lines.next() != Some("OA_FLEET_PROCESSOR_REPORT\t2") {
        return Err(FleetError::Failed(
            "processor report needs version 2 header".into(),
        ));
    }
    let line = lines
        .next()
        .ok_or_else(|| FleetError::Failed("processor report row is missing".into()))?;
    if lines.next().is_some() {
        return Err(FleetError::Failed(
            "each processor report must contain exactly one row".into(),
        ));
    }
    let row = fields(line, REPORT_FIELDS, "processor report row")?;
    let processor_id = parse_u32(field_value(&row, 0)?, "report processor ID")?;
    if processor_id != expected_id {
        return Err(FleetError::Failed(
            "processor report ID does not match its filename".into(),
        ));
    }
    let issued_pin = parse_digest(field_value(&row, 1)?, "reported issued manifest")?;
    let loaded = parse_digest(field_value(&row, 2)?, "loaded adaptation")?;
    let disk_manifest = if field_value(&row, 3)? == "-" {
        None
    } else {
        Some(parse_digest(field_value(&row, 3)?, "disk manifest")?)
    };
    let disk_adaptation = if field_value(&row, 4)? == "-" {
        None
    } else {
        Some(parse_digest(field_value(&row, 4)?, "disk adaptation")?)
    };
    if field_value(&row, 5)? != "true" {
        return Err(FleetError::Failed(
            "processor report must retain provenance_unverified=true".into(),
        ));
    }
    let status = local_status(issued_pin, loaded, disk_manifest, disk_adaptation)?;
    if field_value(&row, 6)? != local_status_name(status) {
        return Err(FleetError::Failed(
            "local inventory status does not match its digest fields".into(),
        ));
    }
    let source_id = parse_u32(field_value(&row, 7)?, "reported source ID")?;
    let epoch = parse_u64(field_value(&row, 8)?, "reported service epoch")?;
    let clock_domain_id = parse_u32(field_value(&row, 9)?, "reported clock domain")?;
    let sequence = parse_u64(field_value(&row, 10)?, "reported sequence")?;
    let sample_time_ns = parse_i64(field_value(&row, 11)?, "reported sample time")?;
    let firmware_sha256 = parse_digest(field_value(&row, 12)?, "reported firmware")?;
    let running_image_sha256 = parse_digest(field_value(&row, 13)?, "reported running image")?;
    let telemetry_loss_events = parse_u32(field_value(&row, 14)?, "reported telemetry loss")?;
    let loaded_components = crate::local_inventory::LoadedComponents {
        model_snapshot: Some(loaded),
        ca_preset: parse_optional_digest(field_value(&row, 15)?, "loaded CA preset")?,
        pver_catalog: parse_optional_digest(field_value(&row, 16)?, "loaded PVER catalog")?,
        runway_aids: parse_optional_digest(field_value(&row, 17)?, "loaded runway aids")?,
    };
    let local = LocalInventoryReport {
        processor_id,
        observed_at_unix_ns: Some(sample_time_ns),
        issued_manifest_sha256: issued_pin,
        expected_adaptation_sha256: Some(identity.snapshot_sha256),
        loaded_adaptation_sha256: loaded,
        disk_manifest_sha256: disk_manifest,
        disk_adaptation_sha256: disk_adaptation,
        comparison: compare_adaptation_identity(
            issued_pin,
            None,
            disk_manifest,
            Some(loaded),
            disk_adaptation,
        ),
        provenance_unverified: true,
        status,
        issued_components: crate::local_inventory::IssuedComponents {
            ca_preset: identity.ca_preset_sha256,
            pver_catalog: identity.pver_catalog_sha256,
            runway_aids: identity.runway_aids_sha256,
        },
        loaded_components,
        disk_components: crate::local_inventory::IssuedComponents {
            ca_preset: identity.ca_preset_sha256,
            pver_catalog: identity.pver_catalog_sha256,
            runway_aids: identity.runway_aids_sha256,
        },
        adaptation_components_complete: None,
    };
    Ok(ReportRow {
        local,
        loaded_components,
        context: ProcessorContext {
            source_id: SourceId(source_id),
            epoch: ServiceEpoch(epoch),
            clock_domain_id: ClockDomainId(clock_domain_id),
            sequence,
            sample_time_ns,
            firmware_sha256,
            running_image_sha256,
            telemetry_loss_events,
        },
    })
}

fn components_match_role(
    role: ProcessRole,
    issued_model: Digest,
    issued: crate::local_inventory::IssuedComponents,
    loaded: crate::local_inventory::LoadedComponents,
) -> bool {
    let entries = [
        (Component::Ca, issued.ca_preset, loaded.ca_preset),
        (Component::Pver, issued.pver_catalog, loaded.pver_catalog),
        (Component::Runway, issued.runway_aids, loaded.runway_aids),
    ];
    entries.into_iter().all(
        |(component, expected, observed)| match (expected, observed) {
            (None, None) => true,
            (Some(expected), Some(observed)) => expected == observed,
            (Some(_), None) => !role.requires(component),
            (None, Some(_)) => false,
        },
    ) && loaded.model_snapshot == Some(issued_model)
}

fn component_state(
    role: Option<ProcessRole>,
    component: Component,
    issued: Option<Digest>,
    loaded: Option<Digest>,
) -> ComponentState {
    match (issued, loaded) {
        (None, None) => ComponentState::NotIssued,
        (None, Some(_)) => ComponentState::Mismatch,
        (Some(expected), Some(observed)) if expected == observed => ComponentState::Match,
        (Some(_), Some(_)) => ComponentState::Mismatch,
        (Some(_), None) if role.is_some_and(|role| role.requires(component)) => {
            ComponentState::Unavailable
        }
        (Some(_), None) if role.is_some() => ComponentState::NotApplicable,
        (Some(_), None) => ComponentState::Unavailable,
    }
}

fn parse_optional_digest(value: &str, field: &str) -> Result<Option<Digest>, FleetError> {
    if value == "-" {
        Ok(None)
    } else {
        parse_digest(value, field).map(Some)
    }
}

fn local_status_name(status: LocalInventoryStatus) -> &'static str {
    match status {
        LocalInventoryStatus::Match => "match",
        LocalInventoryStatus::IssuedManifestMismatch => "issued_manifest_mismatch",
        LocalInventoryStatus::LoadedRuntimeMismatch => "loaded_runtime_mismatch",
        LocalInventoryStatus::LoadedDiskMismatch => "loaded_disk_mismatch",
        LocalInventoryStatus::PackageRefused => "package_refused",
    }
}

fn discover_reports(root: &Path) -> Result<Vec<(u32, PathBuf)>, FleetError> {
    oa_sealed::refuse_io(root).map_err(|error| failed("reports directory", error))?;
    // seal-allow: refuse_io checked the reports root; each discovered child is guarded below.
    let entries = fs::read_dir(root).map_err(|error| failed("read reports directory", error))?;
    let mut reports = Vec::new();
    reports
        .try_reserve_exact(MAX_REPORT_FILES)
        .map_err(|_| FleetError::Failed("report inventory allocation refused".into()))?;
    for entry in entries {
        if reports.len() == MAX_REPORT_FILES {
            return Err(FleetError::Failed(
                "reports directory exceeds 512 processor files".into(),
            ));
        }
        let entry = entry.map_err(|error| failed("read report entry", error))?;
        let path = entry.path();
        oa_sealed::refuse_io(&path).map_err(|error| failed("processor report path", error))?;
        if !entry
            .file_type()
            .map_err(|error| failed("inspect report entry", error))?
            .is_file()
        {
            return Err(FleetError::Failed(
                "reports directory may contain only regular per-processor files".into(),
            ));
        }
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| FleetError::Failed("report filename is not UTF-8".into()))?;
        let id_text = name.strip_suffix(".tsv").ok_or_else(|| {
            FleetError::Failed("report filenames must be <processor-id>.tsv".into())
        })?;
        let processor_id = parse_u32(id_text, "report filename processor ID")?;
        if processor_id == 0 || reports.iter().any(|(id, _)| *id == processor_id) {
            return Err(FleetError::Failed(
                "report filenames must have distinct nonzero processor IDs".into(),
            ));
        }
        reports.push((processor_id, path));
    }
    reports.sort_by_key(|(id, _)| *id);
    Ok(reports)
}

fn parse_options(args: &[String]) -> Result<Vec<(String, String)>, FleetError> {
    if args.len() > MAX_FLEET_ARGUMENTS {
        return Err(FleetError::Usage(
            "fleet-check accepts at most nine flag/value pairs".into(),
        ));
    }
    if args.len() % 2 != 0 {
        return Err(FleetError::Usage(
            "fleet-check needs flag/value pairs".into(),
        ));
    }
    let mut total_bytes = 0usize;
    for arg in args {
        if arg.len() > MAX_ARGUMENT_BYTES {
            return Err(FleetError::Usage(
                "fleet-check argument exceeds 4096 bytes".into(),
            ));
        }
        total_bytes = total_bytes
            .checked_add(arg.len())
            .filter(|bytes| *bytes <= MAX_FLEET_OPTION_BYTES)
            .ok_or_else(|| FleetError::Usage("fleet-check arguments exceed byte limit".into()))?;
    }
    let mut pairs = Vec::new();
    pairs
        .try_reserve_exact(args.len() / 2)
        .map_err(|_| FleetError::Failed("argument allocation refused".into()))?;
    let mut index = 0usize;
    while index < args.len() {
        let flag = args
            .get(index)
            .ok_or_else(|| FleetError::Usage("missing argument flag".into()))?;
        let value = args
            .get(index + 1)
            .ok_or_else(|| FleetError::Usage(format!("{flag} needs a value")))?;
        if !flag.starts_with("--") || value.is_empty() || value.starts_with("--") {
            return Err(FleetError::Usage(format!("invalid value for {flag}")));
        }
        if pairs
            .iter()
            .any(|(prior, _): &(String, String)| prior == flag)
        {
            return Err(FleetError::Usage(format!("{flag} given more than once")));
        }
        pairs.push((flag.clone(), value.clone()));
        index += 2;
    }
    Ok(pairs)
}

fn option<'a>(pairs: &'a [(String, String)], name: &str) -> Result<&'a str, FleetError> {
    pairs
        .iter()
        .find(|(flag, _)| flag == name)
        .map(|(_, value)| value.as_str())
        .ok_or_else(|| FleetError::Usage(format!("fleet-check needs {name}")))
}

/// Run the bounded self-reported processor/adaptation comparison.
pub fn run(args: &[String], out: &mut dyn Write) -> Result<(), FleetError> {
    let options = parse_options(args)?;
    const ALLOWED: [&str; 9] = [
        "--issued-package-root",
        "--expected-manifest-sha256",
        "--expected",
        "--reports-dir",
        "--previous-epoch",
        "--epoch",
        "--clock-domain-id",
        "--query-time-ns",
        "--freshness-limit-ns",
    ];
    if options
        .iter()
        .any(|(flag, _)| !ALLOWED.contains(&flag.as_str()))
    {
        return Err(FleetError::Usage("unknown fleet-check option".into()));
    }
    if options.len() != ALLOWED.len() {
        return Err(FleetError::Usage(
            "fleet-check requires every listed option exactly once".into(),
        ));
    }
    let package_root = PathBuf::from(option(&options, "--issued-package-root")?);
    let expected_pin = parse_pin(option(&options, "--expected-manifest-sha256")?)
        .map_err(|error| failed("expected manifest pin", error))?;
    let package = verify_current(&package_root, expected_pin)
        .map_err(|error| failed("issued adaptation package", error))?;
    let expected_adaptation = package.identity().snapshot_sha256;
    let expected_path = PathBuf::from(option(&options, "--expected")?);
    let expected = read_expected(&expected_path)?;
    if expected.is_empty() {
        return Err(FleetError::Failed(
            "expected processor inventory is empty".into(),
        ));
    }
    if expected
        .iter()
        .any(|row| row.processor.adaptation_sha256 != expected_adaptation)
    {
        return Err(FleetError::Failed(
            "expected adaptation digests do not match the verified issued package".into(),
        ));
    }
    let reports_dir = PathBuf::from(option(&options, "--reports-dir")?);
    let report_paths = discover_reports(&reports_dir)?;
    let mut processor_reports = Vec::new();
    // A7.2: one bounded fixed-size local outcome tuple for each discovered report.
    let mut local_by_id = Vec::new();
    processor_reports
        .try_reserve_exact(report_paths.len())
        .map_err(|_| FleetError::Failed("report allocation refused".into()))?;
    local_by_id
        .try_reserve_exact(report_paths.len())
        .map_err(|_| FleetError::Failed("report allocation refused".into()))?;
    for (id, path) in report_paths {
        let mut row = read_report(&path, id, package.identity())?;
        let expected_row = expected
            .iter()
            .find(|entry| entry.processor.processor_id.0 == id);
        if let Some(expected_row) = expected_row {
            row.local.adaptation_components_complete = Some(components_match_role(
                expected_row.role,
                expected_adaptation,
                row.local.issued_components,
                row.loaded_components,
            ));
        }
        let role = expected_row.map(|entry| entry.role);
        let ca_state = component_state(
            role,
            Component::Ca,
            row.local.issued_components.ca_preset,
            row.loaded_components.ca_preset,
        );
        let pver_state = component_state(
            role,
            Component::Pver,
            row.local.issued_components.pver_catalog,
            row.loaded_components.pver_catalog,
        );
        let runway_state = component_state(
            role,
            Component::Runway,
            row.local.issued_components.runway_aids,
            row.loaded_components.runway_aids,
        );
        local_by_id.push((
            id,
            row.local.status,
            role,
            ca_state,
            pver_state,
            runway_state,
        ));
        let is_expected = expected_row.is_some();
        if row.local.status != LocalInventoryStatus::PackageRefused || !is_expected {
            processor_reports.push(processor_report(row.local, row.context));
        }
    }
    let previous_text = option(&options, "--previous-epoch")?;
    let previous_epoch = if previous_text == "none" {
        None
    } else {
        Some(ServiceEpoch(parse_u64(previous_text, "previous epoch")?))
    };
    let epoch = ServiceEpoch(parse_u64(option(&options, "--epoch")?, "epoch")?);
    let clock_domain_id = ClockDomainId(parse_u32(
        option(&options, "--clock-domain-id")?,
        "clock domain",
    )?);
    let query_time_ns = parse_i64(option(&options, "--query-time-ns")?, "query time")?;
    let freshness_limit_ns =
        parse_i64(option(&options, "--freshness-limit-ns")?, "freshness limit")?;
    if freshness_limit_ns <= 0 {
        return Err(FleetError::Usage("freshness limit must be positive".into()));
    }
    let mut expected_processors = Vec::new();
    expected_processors
        .try_reserve_exact(expected.len())
        .map_err(|_| FleetError::Failed("expected assessment allocation refused".into()))?;
    expected_processors.extend(expected.iter().map(|row| row.processor));
    let assessment = assess_inventory(
        &expected_processors,
        &processor_reports,
        InventoryQuery {
            previous_epoch,
            epoch,
            clock_domain_id,
            release_manifest_sha256: expected_pin,
            query_time_ns,
            freshness_limit_ns,
        },
    )
    .map_err(|error| failed("Tech Ops inventory assessment", format!("{error:?}")))?;
    let row_count = usize::try_from(assessment.processor_result_count)
        .map_err(|_| FleetError::Failed("processor result count width".into()))?;
    for row in assessment.processors.iter().take(row_count) {
        let status = local_by_id
            .iter()
            .find(|(id, _, _, _, _, _)| *id == row.processor_id.0)
            .map_or("missing_report", |(_, status, _, _, _, _)| {
                local_status_name(*status)
            });
        let role = local_by_id
            .iter()
            .find(|(id, _, _, _, _, _)| *id == row.processor_id.0)
            .and_then(|(_, _, role, _, _, _)| *role)
            .map_or("unknown", ProcessRole::name);
        let (ca_state, pver_state, runway_state) = local_by_id
            .iter()
            .find(|(id, _, _, _, _, _)| *id == row.processor_id.0)
            .map_or(
                (
                    ComponentState::Unavailable,
                    ComponentState::Unavailable,
                    ComponentState::Unavailable,
                ),
                |(_, _, _, ca, pver, runway)| (*ca, *pver, *runway),
            );
        writeln!(
            out,
            "processor\t{}\trole={}\tlocal={}\tflags=0x{:08x}\tcomponents_complete={}\tca_preset={}\tpver_catalog={}\trunway_aids={}\trows={}\tunique={}\tmissing_sequences={}\tloaded_adaptation_mismatches={}\tdisk_adaptation_mismatches={}",
            row.processor_id.0,
            role,
            status,
            row.flags,
            row.flags & FLAG_ADAPTATION_COMPONENT_INCOMPLETE == 0,
            ca_state.name(),
            pver_state.name(),
            runway_state.name(),
            row.report_rows,
            row.unique_sequences,
            row.missing_sequences,
            row.loaded_adaptation_mismatches,
            row.disk_adaptation_mismatches
        )
        .map_err(|error| failed("write processor output", error))?;
    }
    writeln!(
        out,
        "aggregate\tstate={:?}\tprovenance_unverified={}\texpected={}\treports={}\tmissing_processors={}\tmissing_sequences={}\tunexpected_reports={}\tloaded_adaptation_mismatches={}\tdisk_adaptation_mismatches={}\trelease_manifest_mismatches={}\tflags=0x{:08x}",
        assessment.state,
        assessment.provenance_unverified,
        assessment.expected_processor_count,
        assessment.report_rows,
        assessment.missing_processors,
        assessment.missing_sequences,
        assessment.unexpected_reports,
        assessment.loaded_adaptation_mismatches,
        assessment.disk_adaptation_mismatches,
        assessment.release_manifest_mismatches,
        assessment.flags
    )
    .map_err(|error| failed("write aggregate output", error))?;

    let adaptation_or_missing = has_fatal_inventory_flag(assessment.flags)
        || local_by_id
            .iter()
            .any(|(_, status, _, _, _, _)| *status != LocalInventoryStatus::Match)
        || assessment.unexpected_reports != 0;
    if adaptation_or_missing {
        return Err(FleetError::Failed(
            "fleet adaptation or missing-report finding; see emitted unverified status rows".into(),
        ));
    }
    // Other Tech Ops flags remain visible but are outside this adaptation-only exit policy.
    Ok(())
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
    use crate::manifest::{issue_new_package, write_new_package, ChangeClass};
    use oa_airspace::model::{empty_model, Meta};
    use oa_airspace::snapshot::encode;
    use oa_sha256::sha256;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Case {
        root: PathBuf,
        package_root: PathBuf,
        reports_root: PathBuf,
        expected_path: PathBuf,
        snapshot: Vec<u8>,
        digest: Digest,
        pin: Digest,
    }

    fn fresh_root(label: &str) -> PathBuf {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "oa-adapt-fleet-{label}-{}-{id}",
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
        .expect("synthetic adaptation snapshot")
    }

    fn setup(label: &str, ids: &[u32]) -> Case {
        let root = fresh_root(label);
        let package_root = root.join("issued");
        let reports_root = root.join("reports");
        fs::create_dir_all(&reports_root).expect("create reports directory");
        let snapshot = snapshot(92_600);
        let pin = write_new_package(&package_root, &snapshot, "issued-1", "ZAU", "G2-1")
            .expect("write synthetic issued package");
        let digest = sha256(&snapshot);
        let expected_path = root.join("expected.tsv");
        let mut expected_text = String::from("OA_EXPECTED_PROCESSORS\t2\n");
        for id in ids {
            expected_text.push_str(&format!(
                "{id}\tgeneric\t{}\t{}\t{}\t{}\t1\t2\n",
                id + 100,
                "11".repeat(32),
                "22".repeat(32),
                oa_sha256::to_hex(&digest),
            ));
        }
        fs::write(&expected_path, expected_text).expect("write expected processor inventory");
        Case {
            root,
            package_root,
            reports_root,
            expected_path,
            snapshot,
            digest,
            pin,
        }
    }

    fn report_line(
        id: u32,
        pin: Digest,
        loaded: Digest,
        disk_pin: Option<Digest>,
        disk: Option<Digest>,
        status: LocalInventoryStatus,
    ) -> String {
        format!(
            "OA_FLEET_PROCESSOR_REPORT\t2\n{}\t{}\t{}\t{}\t{}\ttrue\t{}\t9\t7\t1\t1000\t{}\t{}\t0\t-\t-\t-\n",
            id,
            oa_sha256::to_hex(&pin),
            oa_sha256::to_hex(&loaded),
            disk_pin.map_or_else(|| "-".to_owned(), |v| oa_sha256::to_hex(&v)),
            disk.map_or_else(|| "-".to_owned(), |v| oa_sha256::to_hex(&v)),
            local_status_name(status),
            id + 100,
            "11".repeat(32),
            "22".repeat(32),
        )
    }

    // REQ: OA-ADAPT-003, OA-TECHOPS-003
    #[test]
    fn expected_process_role_controls_component_not_applicable_and_gaps_are_blocking() {
        let issued = crate::local_inventory::IssuedComponents {
            ca_preset: Some([1; 32]),
            pver_catalog: Some([2; 32]),
            runway_aids: Some([3; 32]),
        };
        let all_loaded = crate::local_inventory::LoadedComponents {
            model_snapshot: Some([9; 32]),
            ca_preset: Some([1; 32]),
            pver_catalog: Some([2; 32]),
            runway_aids: Some([3; 32]),
        };
        assert!(components_match_role(
            ProcessRole::Generic,
            [9; 32],
            issued,
            all_loaded
        ));
        for loaded in [
            crate::local_inventory::LoadedComponents {
                ca_preset: None,
                ..all_loaded
            },
            crate::local_inventory::LoadedComponents {
                pver_catalog: None,
                ..all_loaded
            },
            crate::local_inventory::LoadedComponents {
                runway_aids: None,
                ..all_loaded
            },
            crate::local_inventory::LoadedComponents {
                ca_preset: Some([8; 32]),
                ..all_loaded
            },
            crate::local_inventory::LoadedComponents {
                pver_catalog: Some([8; 32]),
                ..all_loaded
            },
            crate::local_inventory::LoadedComponents {
                runway_aids: Some([8; 32]),
                ..all_loaded
            },
        ] {
            let complete = components_match_role(ProcessRole::Generic, [9; 32], issued, loaded);
            assert!(!complete, "each issued companion mutation blocks coverage");
            let expected = [ExpectedProcessor {
                processor_id: ProcessorId(1),
                source_id: SourceId(2),
                firmware_sha256: [4; 32],
                running_image_sha256: [5; 32],
                adaptation_sha256: [9; 32],
                sequence_first: 1,
                sequence_next: 2,
            }];
            let report = [ProcessorReport {
                processor_id: ProcessorId(1),
                source_id: SourceId(2),
                epoch: ServiceEpoch(3),
                clock_domain_id: ClockDomainId(4),
                sequence: 1,
                sample_time_ns: 1_000,
                release_manifest_sha256: [6; 32],
                firmware_sha256: [4; 32],
                running_image_sha256: [5; 32],
                loaded_adaptation_sha256: [9; 32],
                disk_adaptation_sha256: [9; 32],
                telemetry_loss_events: 0,
                adaptation_components_complete: complete,
            }];
            let assessment = assess_inventory(
                &expected,
                &report,
                InventoryQuery {
                    previous_epoch: Some(ServiceEpoch(2)),
                    epoch: ServiceEpoch(3),
                    clock_domain_id: ClockDomainId(4),
                    release_manifest_sha256: [6; 32],
                    query_time_ns: 1_000,
                    freshness_limit_ns: 100,
                },
            )
            .expect("bounded synthetic aggregate");
            assert_ne!(assessment.flags & FLAG_ADAPTATION_COMPONENT_INCOMPLETE, 0);
            assert!(has_fatal_inventory_flag(assessment.flags));
        }
        let regional_n_a = crate::local_inventory::LoadedComponents {
            ca_preset: None,
            pver_catalog: None,
            ..all_loaded
        };
        assert!(components_match_role(
            ProcessRole::RegionalSession,
            [9; 32],
            issued,
            regional_n_a
        ));
        assert_eq!(
            component_state(
                Some(ProcessRole::RegionalSession),
                Component::Ca,
                issued.ca_preset,
                None
            ),
            ComponentState::NotApplicable
        );
        assert_eq!(
            component_state(
                Some(ProcessRole::Generic),
                Component::Ca,
                issued.ca_preset,
                None
            ),
            ComponentState::Unavailable
        );
        assert_eq!(
            component_state(
                Some(ProcessRole::Generic),
                Component::Ca,
                issued.ca_preset,
                Some([8; 32])
            ),
            ComponentState::Mismatch
        );
        assert_eq!(
            component_state(Some(ProcessRole::Generic), Component::Ca, None, None),
            ComponentState::NotIssued
        );
        assert!(!components_match_role(
            ProcessRole::RegionalSession,
            [9; 32],
            issued,
            crate::local_inventory::LoadedComponents {
                runway_aids: None,
                ..regional_n_a
            }
        ));
        assert!(!has_fatal_inventory_flag(0));
        assert!(has_fatal_inventory_flag(
            FLAG_ADAPTATION_COMPONENT_INCOMPLETE
        ));
    }

    // REQ: OA-ADAPT-003, OA-TECHOPS-003
    #[test]
    fn expected_roster_requires_versioned_independent_role() {
        let root = fresh_root("role-roster");
        fs::create_dir_all(&root).expect("create synthetic roster directory");
        let path = root.join("expected.tsv");
        fs::write(&path, "1\tregional-session\t11\t11\t22\t33\t1\t2\n")
            .expect("write missing-header roster");
        assert!(read_expected(&path).is_err());
        fs::write(
            &path,
            "OA_EXPECTED_PROCESSORS\t2\n1\tcaller-says-not-applicable\t11\t11\t22\t33\t1\t2\n",
        )
        .expect("write invalid-role roster");
        assert!(read_expected(&path).is_err());
        let _ = fs::remove_dir_all(root);
    }

    fn write_report(case: &Case, id: u32, line: &str) {
        fs::write(case.reports_root.join(format!("{id}.tsv")), line)
            .expect("write synthetic processor report");
    }

    fn command(case: &Case) -> Vec<String> {
        vec![
            "--issued-package-root".to_owned(),
            case.package_root.to_string_lossy().into_owned(),
            "--expected-manifest-sha256".to_owned(),
            oa_sha256::to_hex(&case.pin),
            "--expected".to_owned(),
            case.expected_path.to_string_lossy().into_owned(),
            "--reports-dir".to_owned(),
            case.reports_root.to_string_lossy().into_owned(),
            "--previous-epoch".to_owned(),
            "8".to_owned(),
            "--epoch".to_owned(),
            "9".to_owned(),
            "--clock-domain-id".to_owned(),
            "7".to_owned(),
            "--query-time-ns".to_owned(),
            "2_000".replace('_', ""),
            "--freshness-limit-ns".to_owned(),
            "1_000".replace('_', ""),
        ]
    }

    // Every supported value shape still fits: three maximum-size paths, a 64-byte pin, and
    // bounded decimal values. This protects the cap from rejecting a valid path configuration.
    #[test]
    fn option_parser_accepts_supported_maximum_path_lengths() {
        let mut args = Vec::new();
        for (flag, value) in [
            ("--issued-package-root", "path"),
            ("--expected-manifest-sha256", "pin"),
            ("--expected", "path"),
            ("--reports-dir", "path"),
            ("--previous-epoch", "18446744073709551615"),
            ("--epoch", "18446744073709551615"),
            ("--clock-domain-id", "4294967295"),
            ("--query-time-ns", "-9223372036854775808"),
            ("--freshness-limit-ns", "18446744073709551615"),
        ] {
            args.push(flag.to_owned());
            let value = match flag {
                "--issued-package-root" | "--expected" | "--reports-dir" => {
                    "x".repeat(MAX_ARGUMENT_BYTES)
                }
                "--expected-manifest-sha256" => "a".repeat(64),
                _ => value.to_owned(),
            };
            args.push(value);
        }
        assert!(parse_options(&args).is_ok());
    }

    #[test]
    fn option_parser_refuses_many_arguments_before_pair_scanning() {
        let args = vec!["--x".to_owned(); MAX_FLEET_ARGUMENTS + 1];
        assert!(
            matches!(parse_options(&args), Err(FleetError::Usage(message)) if message.contains("at most nine"))
        );
    }

    #[test]
    fn option_parser_refuses_total_bytes_before_duplicate_scan() {
        let mut args = Vec::new();
        for _ in 0..FLEET_OPTION_COUNT {
            args.push("--issued-package-root".to_owned());
            args.push("x".repeat(MAX_ARGUMENT_BYTES));
        }
        assert!(
            matches!(parse_options(&args), Err(FleetError::Usage(message)) if message.contains("byte limit"))
        );
    }

    fn run_case(case: &Case) -> (Result<(), FleetError>, String) {
        let mut output = Vec::new();
        let mut args = vec!["fleet-check".to_owned()];
        args.extend(command(case));
        let result = crate::cli::run(&args, &mut output)
            .map_err(|error| FleetError::Failed(error.to_string()));
        (
            result,
            String::from_utf8(output).expect("status output is UTF-8"),
        )
    }

    // REQ: OA-ADAPT-003, OA-TECHOPS-001, OA-TECHOPS-003
    #[test]
    fn g2_every_processor_matches() {
        let case = setup("match", &[1, 2]);
        for id in [1, 2] {
            write_report(
                &case,
                id,
                &report_line(
                    id,
                    case.pin,
                    case.digest,
                    Some(case.pin),
                    Some(case.digest),
                    LocalInventoryStatus::Match,
                ),
            );
        }
        let (result, output) = run_case(&case);
        assert!(result.is_ok());
        assert!(output.contains("aggregate\tstate=ClaimsMatchUnverified"));
        assert!(output.contains("provenance_unverified=true"));
        assert!(!output.contains("PASS"));
        assert_eq!(output.lines().count(), 3);
        let _ = fs::remove_dir_all(case.root);
    }

    // REQ: OA-ADAPT-003, OA-TECHOPS-003
    #[test]
    fn g2_one_processor_stale_loaded() {
        let case = setup("stale-loaded", &[1, 2]);
        let old = sha256(&snapshot(92_599));
        write_report(
            &case,
            1,
            &report_line(
                1,
                case.pin,
                old,
                Some(case.pin),
                Some(case.digest),
                LocalInventoryStatus::LoadedDiskMismatch,
            ),
        );
        write_report(
            &case,
            2,
            &report_line(
                2,
                case.pin,
                case.digest,
                Some(case.pin),
                Some(case.digest),
                LocalInventoryStatus::Match,
            ),
        );
        let (result, output) = run_case(&case);
        assert!(result.is_err());
        assert!(output.contains("loaded_adaptation_mismatches=1"));
        let _ = fs::remove_dir_all(case.root);
    }

    // REQ: OA-ADAPT-003, OA-TECHOPS-003
    #[test]
    fn g2_disk_differs_from_loaded() {
        let case = setup("disk-differs", &[1]);
        let old = sha256(&snapshot(92_599));
        write_report(
            &case,
            1,
            &report_line(
                1,
                case.pin,
                case.digest,
                Some(sha256(&snapshot(92_599))),
                Some(old),
                LocalInventoryStatus::IssuedManifestMismatch,
            ),
        );
        let (result, output) = run_case(&case);
        assert!(result.is_err());
        assert!(output.contains("disk_adaptation_mismatches=1"));
        let _ = fs::remove_dir_all(case.root);
    }

    // REQ: OA-ADAPT-003, OA-TECHOPS-001
    #[test]
    fn g2_missing_processor() {
        let case = setup("missing", &[1, 2]);
        write_report(
            &case,
            1,
            &report_line(
                1,
                case.pin,
                case.digest,
                Some(case.pin),
                Some(case.digest),
                LocalInventoryStatus::Match,
            ),
        );
        let (result, output) = run_case(&case);
        assert!(result.is_err());
        assert!(output.contains("processor\t2\trole=unknown\tlocal=missing_report"));
        assert!(output.contains("missing_processors=1"));
        for fatal_flag in [
            FLAG_LOADED_ADAPTATION_MISMATCH,
            FLAG_DISK_ADAPTATION_MISMATCH,
            FLAG_RELEASE_MANIFEST_MISMATCH,
            FLAG_MISSING_PROCESSOR,
            FLAG_MISSING_SEQUENCE,
        ] {
            assert!(
                has_fatal_inventory_flag(fatal_flag),
                "CLI fatal-flag classifier omitted 0x{fatal_flag:08x}"
            );
        }
        let _ = fs::remove_dir_all(case.root);
    }

    // REQ: OA-ADAPT-003, OA-TECHOPS-001, OA-TECHOPS-003
    #[test]
    fn g2_field_change_end_to_end() {
        let mut case = setup("field-change", &[1, 2, 3]);
        let base_pin = case.pin;
        let v1_root = case.root.join("issued-v1");
        let (v1, v1_pin) = issue_new_package(
            &v1_root,
            &case.snapshot,
            "issued-1",
            "ZAU",
            "G2-INITIAL",
            &case.package_root,
            base_pin,
            ChangeClass::Initial,
            "issuer-1",
            "approver-2",
            "synthetic initial issue",
        )
        .expect("issue initial v1 package");
        assert_eq!(v1, 1);
        let mut local_roots = Vec::new();
        let mut v1_activations = Vec::new();
        for id in [1, 2, 3] {
            let local_root = case.root.join(format!("processor-{id}"));
            fs::create_dir_all(&local_root).expect("create synthetic processor package root");
            fs::copy(
                v1_root.join("manifest.oag2"),
                local_root.join("manifest.oag2"),
            )
            .expect("copy v1 manifest to processor");
            fs::copy(
                v1_root.join("airspace.snapshot"),
                local_root.join("airspace.snapshot"),
            )
            .expect("copy v1 snapshot to processor");
            let (_, activation) =
                crate::local_inventory::verify_local_startup(&local_root, v1_pin, id)
                    .expect("processor starts on v1");
            local_roots.push((id, local_root));
            v1_activations.push((id, activation));
        }

        let changed = snapshot(92_601);
        let changed_root = case.root.join("issued-v2");
        let (v2, v2_pin) = issue_new_package(
            &changed_root,
            &changed,
            "issued-2",
            "ZAU",
            "G2-CHANGE",
            &v1_root,
            v1_pin,
            ChangeClass::FieldChange,
            "issuer-1",
            "approver-2",
            "synthetic field change",
        )
        .expect("issue v2 field change");
        assert_eq!(v2, 2);
        let v2_digest = sha256(&changed);
        let mut expected_text = String::new();
        for id in [1, 2, 3] {
            expected_text.push_str(&format!(
                "{id}\t{}\t{}\t{}\t{}\t1\t2\n",
                id + 100,
                "11".repeat(32),
                "22".repeat(32),
                oa_sha256::to_hex(&v2_digest),
            ));
        }
        fs::write(&case.expected_path, expected_text).expect("replace expected with v2 inventory");
        for name in ["manifest.oag2", "airspace.snapshot"] {
            fs::copy(changed_root.join(name), case.package_root.join(name))
                .expect("install issued v2 central package");
        }
        case.pin = v2_pin;

        // Phase one: processors 1 and 2 activate v2; processor 3 remains active on v1.
        // The local package roots are independent, so changing 1/2 cannot silently update 3.
        let mut v2_activations = Vec::new();
        for id in [1, 2] {
            let local_root = local_roots
                .iter()
                .find(|(processor_id, _)| *processor_id == id)
                .map(|(_, root)| root)
                .expect("processor root exists");
            for name in ["manifest.oag2", "airspace.snapshot"] {
                fs::copy(changed_root.join(name), local_root.join(name))
                    .expect("install v2 on processor");
            }
            let (_, activation) =
                crate::local_inventory::verify_local_startup(local_root, v2_pin, id)
                    .expect("processor activates v2");
            v2_activations.push((id, activation));
        }
        for (id, activation) in &v2_activations {
            let local = crate::local_inventory::verify_local_inventory(activation);
            assert_eq!(local.status, LocalInventoryStatus::Match);
            write_report(
                &case,
                *id,
                &report_line(
                    *id,
                    v2_pin,
                    local.loaded_adaptation_sha256,
                    Some(v2_pin),
                    Some(v2_digest),
                    LocalInventoryStatus::Match,
                ),
            );
        }
        let v1_activation3 = v1_activations
            .iter()
            .find(|(id, _)| *id == 3)
            .map(|(_, activation)| activation)
            .expect("processor 3 v1 activation exists");
        let local3_v1 = crate::local_inventory::verify_local_inventory(v1_activation3);
        assert_eq!(local3_v1.status, LocalInventoryStatus::Match);
        write_report(
            &case,
            3,
            &report_line(
                3,
                v1_pin,
                local3_v1.loaded_adaptation_sha256,
                Some(v1_pin),
                Some(local3_v1.disk_adaptation_sha256.expect("v1 disk digest")),
                LocalInventoryStatus::Match,
            ),
        );
        let (phase_one, output_one) = run_case(&case);
        assert!(phase_one.is_err());
        assert!(
            output_one.contains("processor\t1\trole=generic\tlocal=match"),
            "{output_one}"
        );
        assert!(
            output_one.contains("processor\t2\trole=generic\tlocal=match"),
            "{output_one}"
        );
        assert!(
            output_one.contains("processor\t3\trole=generic\tlocal=match"),
            "{output_one}"
        );
        assert!(
            output_one.contains("processor\t1\trole=generic\tlocal=match\tflags=0x00000000"),
            "{output_one}"
        );
        assert!(
            output_one.contains("processor\t2\trole=generic\tlocal=match\tflags=0x00000000"),
            "{output_one}"
        );
        let processor3_row = output_one
            .lines()
            .find(|line| line.starts_with("processor\t3\t"))
            .expect("processor 3 assessment row");
        assert!(!processor3_row.contains("flags=0x00000000"), "{output_one}");
        assert!(
            output_one.contains("release_manifest_mismatches=1"),
            "{output_one}"
        );
        assert!(
            output_one.contains("loaded_adaptation_mismatches=1"),
            "{output_one}"
        );
        assert!(
            output_one.contains("disk_adaptation_mismatches=1"),
            "{output_one}"
        );

        // Phase two: processor 3 activates v2; the complete claimed fleet now matches.
        let local_root3 = local_roots
            .iter()
            .find(|(id, _)| *id == 3)
            .map(|(_, root)| root)
            .expect("processor 3 root exists");
        for name in ["manifest.oag2", "airspace.snapshot"] {
            fs::copy(changed_root.join(name), local_root3.join(name))
                .expect("install v2 on processor 3");
        }
        let (_, activation3_v2) =
            crate::local_inventory::verify_local_startup(local_root3, v2_pin, 3)
                .expect("processor 3 activates v2");
        let local3_v2 = crate::local_inventory::verify_local_inventory(&activation3_v2);
        assert_eq!(local3_v2.status, LocalInventoryStatus::Match);
        write_report(
            &case,
            3,
            &report_line(
                3,
                v2_pin,
                local3_v2.loaded_adaptation_sha256,
                Some(v2_pin),
                Some(v2_digest),
                LocalInventoryStatus::Match,
            ),
        );
        let (phase_two, output_two) = run_case(&case);
        assert!(phase_two.is_ok(), "{phase_two:?}; {output_two}");
        for id in [1, 2, 3] {
            assert!(
                output_two.contains(&format!("processor\t{id}\trole=generic\tlocal=match")),
                "{output_two}"
            );
        }
        assert!(
            output_two.contains("state=ClaimsMatchUnverified"),
            "{output_two}"
        );
        assert!(
            output_two.contains("provenance_unverified=true"),
            "{output_two}"
        );
        assert!(!output_two.contains("PASS"));
        let _ = fs::remove_dir_all(case.root);
    }
}
