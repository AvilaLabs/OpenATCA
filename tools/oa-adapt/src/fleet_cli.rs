//! Offline fleet comparison for issued adaptation identities. Shell only: bounded input reads,
//! guarded paths, and report formatting. Processor values remain caller-supplied claims.
//!
//! The independent expected file is header-free TSV with seven fields per row:
//! `processor_id source_id firmware_sha256 running_image_sha256 adaptation_sha256 sequence_first sequence_next`.
//! The reports directory has one `<processor_id>.tsv` per processor, with fifteen fields:
//! `processor_id issued_manifest_sha256 loaded_adaptation_sha256 disk_manifest_sha256|- disk_adaptation_sha256|- provenance_unverified local_status source_id epoch clock_domain_id sequence sample_time_ns firmware_sha256 running_image_sha256 telemetry_loss_events`.
//! Hash and identity values in a per-processor report are unverified claims, not an attestation.

use crate::local_inventory::{LocalInventoryReport, LocalInventoryStatus};
use crate::manifest::{parse_pin, verify_current};
use oa_sha256::Digest;
use oa_techops::{
    assess_inventory, ClockDomainId, ExpectedProcessor, InventoryQuery, ProcessorId,
    ProcessorReport, ServiceEpoch, SourceId, FLAG_DISK_ADAPTATION_MISMATCH,
    FLAG_LOADED_ADAPTATION_MISMATCH, FLAG_MISSING_PROCESSOR, FLAG_MISSING_SEQUENCE,
    FLAG_RELEASE_MANIFEST_MISMATCH,
};
use std::fmt;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const MAX_ARGUMENT_BYTES: usize = 4_096;
const MAX_EXPECTED_BYTES: usize = 32_768;
const MAX_REPORT_FILES: usize = 512;
const MAX_REPORT_BYTES: usize = 2_048;
// Across all 512 reports, at most 1 MiB of report bytes is admitted; reads are one file at a time.
const REPORT_FIELDS: usize = 15;

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

fn read_expected(path: &Path) -> Result<Vec<ExpectedProcessor>, FleetError> {
    let bytes = read_bounded(path, MAX_EXPECTED_BYTES)?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| FleetError::Failed("expected processor list is not UTF-8".into()))?;
    let mut expected = Vec::new();
    expected
        .try_reserve_exact(oa_techops::MAX_EXPECTED_PROCESSORS)
        .map_err(|_| FleetError::Failed("expected list allocation refused".into()))?;
    for line in text.lines() {
        if expected.len() == oa_techops::MAX_EXPECTED_PROCESSORS {
            return Err(FleetError::Failed(
                "expected processor list exceeds 128 rows".into(),
            ));
        }
        let row = fields(line, 7, "expected processor row")?;
        let processor_id = parse_u32(field_value(&row, 0)?, "expected processor ID")?;
        let source_id = parse_u32(field_value(&row, 1)?, "expected source ID")?;
        if processor_id == 0 || source_id == 0 {
            return Err(FleetError::Failed(
                "expected processor and source IDs must be nonzero".into(),
            ));
        }
        expected.push(ExpectedProcessor {
            processor_id: ProcessorId(processor_id),
            source_id: SourceId(source_id),
            firmware_sha256: parse_digest(field_value(&row, 2)?, "expected firmware")?,
            running_image_sha256: parse_digest(field_value(&row, 3)?, "expected running image")?,
            adaptation_sha256: parse_digest(field_value(&row, 4)?, "expected adaptation")?,
            sequence_first: parse_u64(field_value(&row, 5)?, "expected first sequence")?,
            sequence_next: parse_u64(field_value(&row, 6)?, "expected next sequence")?,
        });
    }
    Ok(expected)
}

struct ReportRow {
    local: LocalInventoryReport,
    context: ProcessorContext,
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
    }
}

fn local_status(
    issued: Digest,
    expected_pin: Digest,
    expected_adaptation: Digest,
    loaded: Digest,
    disk_manifest: Option<Digest>,
    disk_adaptation: Option<Digest>,
) -> Result<LocalInventoryStatus, FleetError> {
    match (disk_manifest, disk_adaptation) {
        (None, None) => Ok(LocalInventoryStatus::PackageRefused),
        (Some(disk_pin), Some(disk_digest)) => {
            if issued != expected_pin || disk_pin != expected_pin {
                Ok(LocalInventoryStatus::IssuedManifestMismatch)
            } else if disk_digest != expected_adaptation {
                Err(FleetError::Failed(
                    "disk adaptation digest conflicts with the issued manifest".into(),
                ))
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
    expected_pin: Digest,
    expected_adaptation: Digest,
) -> Result<ReportRow, FleetError> {
    let bytes = read_bounded(path, MAX_REPORT_BYTES)?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| FleetError::Failed("processor report is not UTF-8".into()))?;
    let mut lines = text.lines();
    let line = lines
        .next()
        .ok_or_else(|| FleetError::Failed("processor report is empty".into()))?;
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
    let status = local_status(
        issued_pin,
        expected_pin,
        expected_adaptation,
        loaded,
        disk_manifest,
        disk_adaptation,
    )?;
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
    let local = LocalInventoryReport {
        processor_id,
        issued_manifest_sha256: issued_pin,
        loaded_adaptation_sha256: loaded,
        disk_manifest_sha256: disk_manifest,
        disk_adaptation_sha256: disk_adaptation,
        provenance_unverified: true,
        status,
    };
    Ok(ReportRow {
        local,
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

fn local_status_name(status: LocalInventoryStatus) -> &'static str {
    match status {
        LocalInventoryStatus::Match => "match",
        LocalInventoryStatus::IssuedManifestMismatch => "issued_manifest_mismatch",
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
    if args.len() % 2 != 0 {
        return Err(FleetError::Usage(
            "fleet-check needs flag/value pairs".into(),
        ));
    }
    if args.iter().any(|arg| arg.len() > MAX_ARGUMENT_BYTES) {
        return Err(FleetError::Usage(
            "fleet-check argument exceeds 4096 bytes".into(),
        ));
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
        .any(|processor| processor.adaptation_sha256 != expected_adaptation)
    {
        return Err(FleetError::Failed(
            "expected adaptation digests do not match the verified issued package".into(),
        ));
    }
    let reports_dir = PathBuf::from(option(&options, "--reports-dir")?);
    let report_paths = discover_reports(&reports_dir)?;
    let mut processor_reports = Vec::new();
    let mut local_by_id = Vec::new();
    processor_reports
        .try_reserve_exact(report_paths.len())
        .map_err(|_| FleetError::Failed("report allocation refused".into()))?;
    local_by_id
        .try_reserve_exact(report_paths.len())
        .map_err(|_| FleetError::Failed("report allocation refused".into()))?;
    for (id, path) in report_paths {
        let row = read_report(&path, id, expected_pin, expected_adaptation)?;
        local_by_id.push((id, row.local.status));
        let is_expected = expected.iter().any(|item| item.processor_id.0 == id);
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
    let assessment = assess_inventory(
        &expected,
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
            .find(|(id, _)| *id == row.processor_id.0)
            .map_or("missing_report", |(_, status)| local_status_name(*status));
        writeln!(
            out,
            "processor\t{}\tlocal={}\tflags=0x{:08x}\trows={}\tunique={}\tmissing_sequences={}\tloaded_adaptation_mismatches={}\tdisk_adaptation_mismatches={}",
            row.processor_id.0,
            status,
            row.flags,
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

    let adaptation_or_missing = assessment.flags
        & (FLAG_LOADED_ADAPTATION_MISMATCH
            | FLAG_DISK_ADAPTATION_MISMATCH
            | FLAG_RELEASE_MANIFEST_MISMATCH
            | FLAG_MISSING_PROCESSOR
            | FLAG_MISSING_SEQUENCE)
        != 0
        || local_by_id
            .iter()
            .any(|(_, status)| *status != LocalInventoryStatus::Match)
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
        let mut expected_text = String::new();
        for id in ids {
            expected_text.push_str(&format!(
                "{id}\t{}\t{}\t{}\t{}\t1\t2\n",
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
            "{}\t{}\t{}\t{}\t{}\ttrue\t{}\t{}\t7\t9\t1\t1000\t{}\t{}\t0\n",
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
        assert!(output.contains("aggregate\tstate="));
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
        assert!(output.contains("processor\t2\tlocal=missing_report"));
        assert!(output.contains("missing_processors=1"));
        let _ = fs::remove_dir_all(case.root);
    }

    // REQ: OA-ADAPT-003, OA-TECHOPS-001, OA-TECHOPS-003
    #[test]
    fn g2_field_change_end_to_end() {
        let mut case = setup("field-change", &[1, 2, 3]);
        let base_root = case.root.join("base-package");
        let base_pin = write_new_package(&base_root, &case.snapshot, "base-0", "ZAU", "G2-BASE")
            .expect("write base package");
        let old_root = case.root.join("old-issued");
        let (old_version, old_pin) = issue_new_package(
            &old_root,
            &case.snapshot,
            "issued-1",
            "ZAU",
            "G2-INITIAL",
            &base_root,
            base_pin,
            ChangeClass::Initial,
            "issuer-1",
            "approver-2",
            "synthetic initial issue",
        )
        .expect("issue initial v1 package");
        assert_eq!(old_version, 1);
        let mut activations = Vec::new();
        for id in [1, 2] {
            let (_, activation) =
                crate::local_inventory::verify_local_startup(&old_root, old_pin, id)
                    .expect("processor starts on v1");
            activations.push((id, activation));
        }
        let changed = snapshot(92_601);
        let changed_root = case.root.join("changed-issued");
        let (version, changed_pin) = issue_new_package(
            &changed_root,
            &changed,
            "issued-2",
            "ZAU",
            "G2-CHANGE",
            &old_root,
            old_pin,
            ChangeClass::FieldChange,
            "issuer-1",
            "approver-2",
            "synthetic field change",
        )
        .expect("issue v2 field change");
        assert_eq!(version, 2);
        let changed_digest = sha256(&changed);
        let mut expected_text = String::new();
        for id in [1, 2, 3] {
            expected_text.push_str(&format!(
                "{id}\t{}\t{}\t{}\t{}\t1\t2\n",
                id + 100,
                "11".repeat(32),
                "22".repeat(32),
                oa_sha256::to_hex(&changed_digest),
            ));
        }
        fs::write(&case.expected_path, expected_text).expect("replace expected package identity");
        fs::copy(
            changed_root.join("manifest.oag2"),
            case.package_root.join("manifest.oag2"),
        )
        .expect("replace central issued manifest");
        fs::copy(
            changed_root.join("airspace.snapshot"),
            case.package_root.join("airspace.snapshot"),
        )
        .expect("replace central issued snapshot");
        case.pin = changed_pin;
        fs::copy(
            changed_root.join("manifest.oag2"),
            old_root.join("manifest.oag2"),
        )
        .expect("replace processors 1 and 2 disk manifest");
        fs::copy(
            changed_root.join("airspace.snapshot"),
            old_root.join("airspace.snapshot"),
        )
        .expect("replace processors 1 and 2 disk snapshot");
        let (_, processor3) =
            crate::local_inventory::verify_local_startup(&changed_root, changed_pin, 3)
                .expect("processor 3 activates v2");
        for (id, activation) in activations {
            let local = crate::local_inventory::verify_local_inventory(&activation);
            assert_eq!(local.status, LocalInventoryStatus::IssuedManifestMismatch);
            write_report(
                &case,
                id,
                &report_line(
                    id,
                    old_pin,
                    local.loaded_adaptation_sha256,
                    Some(changed_pin),
                    Some(changed_digest),
                    LocalInventoryStatus::IssuedManifestMismatch,
                ),
            );
        }
        let local3 = crate::local_inventory::verify_local_inventory(&processor3);
        assert_eq!(local3.status, LocalInventoryStatus::Match);
        write_report(
            &case,
            3,
            &report_line(
                3,
                changed_pin,
                local3.loaded_adaptation_sha256,
                Some(changed_pin),
                Some(changed_digest),
                LocalInventoryStatus::Match,
            ),
        );
        let (result, output) = run_case(&case);
        assert!(result.is_err());
        assert!(
            output.contains("processor\t1\tlocal=issued_manifest_mismatch"),
            "{result:?}; {output}"
        );
        assert!(
            output.contains("processor\t2\tlocal=issued_manifest_mismatch"),
            "{output}"
        );
        assert!(output.contains("processor\t3\tlocal=match"), "{output}");
        let _ = fs::remove_dir_all(case.root);
    }
}
