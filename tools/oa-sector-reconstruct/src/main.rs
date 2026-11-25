#![forbid(unsafe_code)]
//! Offline private empirical sector occupancy from a pinned H2 normalized SFDPS observation
//! trace. Shell only. This is not official or legal sector geometry and has no predictor.

use std::env;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const MAX_INPUT_BYTES: usize = 128 * 1024 * 1024;
const MAX_TRACE_ROWS: usize = 1_000_000;
const MAX_TRACE_RECORDS: usize = 2_000_000;
const MAX_OUTPUT_BYTES: usize = 64 * 1024 * 1024;
const MAX_EVIDENCE_KEYS: usize = 2_000_000;
const MAX_CLI_PAIRS: usize = 8;
const MAX_CLI_ARGS: usize = 1 + 2 * MAX_CLI_PAIRS;
const MAX_CLI_ARG_BYTES: usize = 4096;
const MAX_CLI_TOTAL_BYTES: usize = 32 * 1024;
// H2-OBSERVED-STATE-1 terminal READER_ERROR records enumerate oa_replay::ERR_COUNT (44).
const H2_READER_ERROR_COUNT: usize = 44;
const H2_HEADER: &str = "H2-OBSERVED-STATE-1\tscope=SFDPS-replay-compatibility\tindependent_predictions=not-measured\tindependent_agreement_denominator=not-observable";

fn usage() -> ExitCode {
    eprintln!("usage: oa-sector-reconstruct (train|check) --schema H2-OBSERVED-STATE-1 --partition 2026-11-15|2026-11-19 --private-root PRIVATE_DIR --input TRACE --out PRIVATE_FILE [--model TRAINING_FILE]");
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let Ok(cli_args) = bounded_args(env::args_os().skip(1)) else {
        return usage();
    };
    let mut args = cli_args.into_iter();
    let Some(mode) = args.next() else {
        return usage();
    };
    if mode != "train" && mode != "check" {
        return usage();
    }
    let (mut schema, mut input, mut out, mut model, mut partition, mut private_root) =
        (None, None, None, None, None, None);
    while let Some(arg) = args.next() {
        let Some(value) = args.next() else {
            return usage();
        };
        match arg.as_str() {
            "--schema" if schema.is_none() => schema = Some(value),
            "--input" if input.is_none() => input = Some(PathBuf::from(value)),
            "--out" if out.is_none() => out = Some(PathBuf::from(value)),
            "--model" if model.is_none() => model = Some(PathBuf::from(value)),
            "--partition" if partition.is_none() => partition = Some(value),
            "--private-root" if private_root.is_none() => private_root = Some(PathBuf::from(value)),
            _ => return usage(),
        }
    }
    if schema.as_deref() != Some("H2-OBSERVED-STATE-1") {
        return usage();
    }
    let Some(partition) = partition else {
        return usage();
    };
    if (mode == "train" && partition != "2026-11-15")
        || (mode == "check" && partition != "2026-11-19")
    {
        return usage();
    }
    if (mode == "train") == model.is_some() {
        return usage();
    }
    let (Some(input), Some(out), Some(private_root)) = (input, out, private_root) else {
        return usage();
    };
    match run(
        &mode,
        &partition,
        &private_root,
        &input,
        &out,
        model.as_deref(),
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("oa-sector-reconstruct: {error}");
            ExitCode::FAILURE
        }
    }
}

fn bounded_args(args: impl IntoIterator<Item = std::ffi::OsString>) -> Result<Vec<String>, ()> {
    let mut retained = Vec::with_capacity(MAX_CLI_ARGS);
    let mut total_bytes = 0usize;
    let mut args = args.into_iter();
    for _ in 0..=MAX_CLI_ARGS {
        let Some(arg) = args.next() else {
            return Ok(retained);
        };
        if retained.len() == MAX_CLI_ARGS {
            return Err(());
        }
        let arg_bytes = arg.as_encoded_bytes().len();
        if arg_bytes > MAX_CLI_ARG_BYTES {
            return Err(());
        }
        total_bytes = total_bytes.checked_add(arg_bytes).ok_or(())?;
        if total_bytes > MAX_CLI_TOTAL_BYTES {
            return Err(());
        }
        retained.push(arg.into_string().map_err(|_| ())?);
    }
    Err(())
}

fn run(
    mode: &str,
    partition: &str,
    private_root: &Path,
    input: &Path,
    out: &Path,
    model: Option<&Path>,
) -> Result<(), String> {
    let guard = oa_sealed::Sealed::production();
    if !private_root.is_absolute() {
        return Err("--private-root must be an absolute existing directory".into());
    }
    guard.check(private_root).map_err(|e| e.to_string())?;
    let root = std::fs::canonicalize(private_root)
        .map_err(|e| format!("cannot resolve private root: {e}"))?;
    if private_root != root {
        return Err("--private-root must be supplied as its canonical path".into());
    }
    guard.check(&root).map_err(|e| e.to_string())?;
    // seal-allow: metadata-only mode check of the canonical path just accepted by the sealed guard.
    let root_meta = std::fs::metadata(&root).map_err(|e| e.to_string())?;
    if !root_meta.is_dir() || !private_permissions(&root_meta) {
        return Err("private root must be a directory with no group/other permissions".into());
    }
    if !input.is_absolute() || !out.is_absolute() {
        return Err("input and output paths must be absolute".into());
    }
    guard.check(input).map_err(|e| e.to_string())?;
    guard.check(out).map_err(|e| e.to_string())?;
    let parent = out
        .parent()
        .ok_or("output needs a private parent directory")?;
    guard.check(parent).map_err(|e| e.to_string())?;
    let input_real =
        std::fs::canonicalize(input).map_err(|e| format!("cannot resolve input: {e}"))?;
    let parent_real = std::fs::canonicalize(parent)
        .map_err(|e| format!("cannot resolve output directory: {e}"))?;
    if !private_tree(guard, &root, &input_real)? || !private_tree(guard, &root, &parent_real)? {
        return Err("input and output directory must resolve under the declared private root with private permissions".into());
    }
    if let Some(model_path) = model {
        if !model_path.is_absolute() {
            return Err("model path must be absolute".into());
        }
        guard.check(model_path).map_err(|e| e.to_string())?;
        let model_real =
            std::fs::canonicalize(model_path).map_err(|e| format!("cannot resolve model: {e}"))?;
        if !private_tree(guard, &root, &model_real)? {
            return Err(
                "model must resolve under the declared private root with private permissions"
                    .into(),
            );
        }
    }
    let (file, size) = guard.open_regular(input).map_err(|e| e.to_string())?;
    if size == 0 || size > MAX_INPUT_BYTES as u64 {
        return Err("input bytes outside 1..=134217728; INCOMPLETE".into());
    }
    let mut bytes = Vec::with_capacity(size as usize);
    file.take(MAX_INPUT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 != size {
        return Err("input changed while reading or exceeded byte cap; INCOMPLETE".into());
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| "trace is not UTF-8")?;
    // The entire bounded trace is time-pre-scanned before aggregation or output creation.
    prescan_times(text, guard, partition)?;
    let digest = oa_sha256::to_hex(&oa_sha256::sha256(&bytes));
    let binary_digest = executable_digest(guard)?;
    let policy_digest = oa_sha256::to_hex(&oa_sha256::sha256(b"oa-sector-reconstruct-v2;grid=500000e7;alt=2000ft;state-bin=900000ms;min-flights=3;min-bins=2;center=ZAU-zero-pad;frame-row-op-identity-joins=required;future-cross-clock-observations=excluded;training=2026-11-15;check=2026-11-19"));
    let observed = parse_trace(text, partition)?;
    let report = if mode == "train" {
        train_report(&observed, &digest, &binary_digest, &policy_digest)?
    } else {
        let model_path = model.ok_or("check requires --model")?;
        let (f, n) = guard.open_regular(model_path).map_err(|e| e.to_string())?;
        if n == 0 || n > MAX_OUTPUT_BYTES as u64 {
            return Err("training model size outside bounded limit".into());
        }
        let mut b = Vec::with_capacity(n as usize);
        f.take(MAX_OUTPUT_BYTES as u64 + 1)
            .read_to_end(&mut b)
            .map_err(|e| e.to_string())?;
        if b.len() as u64 != n {
            return Err("training model changed while reading".into());
        }
        let model_digest = oa_sha256::to_hex(&oa_sha256::sha256(&b));
        check_report(
            &observed,
            &digest,
            &binary_digest,
            &policy_digest,
            &model_digest,
            std::str::from_utf8(&b).map_err(|_| "training model is not UTF-8")?,
        )?
    };
    if report.len() > MAX_OUTPUT_BYTES {
        return Err("report exceeds output cap; INCOMPLETE".into());
    }
    publish_private(out, report.as_bytes()).map_err(|e| e.to_string())
}

#[cfg(unix)]
fn private_permissions(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    meta.mode() & 0o077 == 0
}
#[cfg(not(unix))]
fn private_permissions(_: &std::fs::Metadata) -> bool {
    false
}

fn private_tree(guard: &oa_sealed::Sealed, root: &Path, path: &Path) -> Result<bool, String> {
    if !path.starts_with(root) {
        return Ok(false);
    }
    let mut current = path;
    loop {
        guard.check(current).map_err(|e| e.to_string())?;
        // seal-allow: metadata-only permission check after this canonical ancestor passed the sealed guard.
        let meta = std::fs::metadata(current)
            .map_err(|e| format!("cannot verify private path permissions: {e}"))?;
        if !private_permissions(&meta) {
            return Ok(false);
        }
        if current == root {
            return Ok(true);
        }
        current = current.parent().ok_or("private path escaped root")?;
        if !current.starts_with(root) {
            return Ok(false);
        }
    }
}

fn executable_digest(guard: &oa_sealed::Sealed) -> Result<String, String> {
    let path = env::current_exe().map_err(|e| format!("cannot resolve tool executable: {e}"))?;
    let (f, size) = guard
        .open_regular(&path)
        .map_err(|e| format!("cannot verify tool executable: {e}"))?;
    if size == 0 || size > MAX_OUTPUT_BYTES as u64 {
        return Err("tool executable exceeds hash cap".into());
    }
    let mut bytes = Vec::with_capacity(size as usize);
    f.take(MAX_OUTPUT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 != size {
        return Err("tool executable changed during hashing".into());
    }
    Ok(oa_sha256::to_hex(&oa_sha256::sha256(&bytes)))
}

fn prescan_times(text: &str, guard: &oa_sealed::Sealed, partition: &str) -> Result<(), String> {
    let (start_ms, end_ms) = partition_bounds(partition)?;
    let mut lines = text.lines();
    let header = lines.next().ok_or("empty trace")?;
    if header != H2_HEADER {
        return Err("trace header/schema mismatch; no payload processed".into());
    }
    for (index, line) in lines.enumerate() {
        let line_no = index + 2;
        if index >= MAX_TRACE_RECORDS {
            return Err("trace record cap exceeded; INCOMPLETE".into());
        }
        if line.len() > 8192 {
            return Err(format!("line {line_no} exceeds 8192-byte H2 record cap"));
        }
        if line.is_empty() {
            return Err(format!("empty trace line {line_no}"));
        }
        let kind = line.split('\t').next().unwrap_or("");
        if !matches!(kind, "ROW" | "OP" | "FRAME") {
            continue;
        }
        let mut found_time = false;
        for field in line.split('\t').skip(1) {
            let Some((key, value)) = field.split_once('=') else {
                return Err(format!("malformed field at line {line_no}"));
            };
            if !allowed_field(kind, key) {
                return Err(format!(
                    "unknown H2 schema field {key} at line {line_no}; no aggregation performed"
                ));
            }
            if kind == "FRAME" && key == "disposition" && value == "6" {
                return Err(format!("trace contains a sealed-document frame at line {line_no}; no aggregation performed"));
            }
            let unit = match key {
                "source_ms" | "received_ms" | "position_ms" | "ts_ms" | "seen_ms" => {
                    Some(1_000_000i64)
                }
                "received_ns" => Some(1),
                _ => None,
            };
            if let Some(multiplier) = unit {
                if value != "none" {
                    let t = value
                        .parse::<i64>()
                        .map_err(|_| format!("invalid {key} at line {line_no}"))?;
                    let ns = t.checked_mul(multiplier).ok_or("timestamp overflow")?;
                    if guard.in_window(ns) {
                        return Err(format!(
                            "sealed timestamp in line {line_no}; no payload processed"
                        ));
                    }
                    let ms = ns.div_euclid(1_000_000);
                    if ms < start_ms || ms >= end_ms {
                        return Err(format!("timestamp outside declared {partition} partition at line {line_no}; no aggregation performed"));
                    }
                    found_time = true;
                }
            }
        }
        if !found_time {
            return Err(format!("record has no validated time at line {line_no}"));
        }
    }
    Ok(())
}

fn allowed_field(kind: &str, key: &str) -> bool {
    match kind {
        "CONFIG" => matches!(
            key,
            "from_ns"
                | "to_ns_inclusive"
                | "capacity"
                | "lag_floor_ms"
                | "max_future_ms"
                | "refusal_log"
                | "conflict_log"
                | "removal_log"
                | "sweep_every_ms"
                | "retain_ms"
                | "silence_ms"
        ),
        "BUILD" => matches!(key, "commit" | "dirty" | "binary_identity"),
        "RESULT" => matches!(
            key,
            "status"
                | "qualification"
                | "frames"
                | "decoded_rows"
                | "docs_unparsed"
                | "docs_refused"
                | "reader_loss"
                | "error_hex"
        ),
        "COUNTS" => matches!(
            key,
            "frames_visited"
                | "rows_delivered"
                | "events_delivered"
                | "missing_key_or_time"
                | "zero_op_rows"
                | "cap_refused"
                | "sink_failed"
                | "sealed_drops"
                | "merge_files"
                | "merge_bytes"
                | "reader_steps"
                | "decoded_bytes_charged"
                | "raw_input_bytes_reserved"
                | "raw_input_cap_refused"
        ),
        "READER_ERROR" => matches!(key, "reason" | "count"),
        "OP_COUNTS" => matches!(key, "kind" | "applied" | "unchanged"),
        "OP_REFUSAL" => matches!(key, "kind" | "reason" | "count"),
        "INPUT" => matches!(
            key,
            "index" | "path_hex" | "opened" | "eof" | "bytes" | "reserved_bytes" | "sha256"
        ),
        "FRAME" => matches!(
            key,
            "stream" | "offset" | "seq" | "row" | "received_ns" | "disposition" | "reason"
        ),
        "ROW" => matches!(
            key,
            "stream"
                | "offset"
                | "seq"
                | "row"
                | "source_ms"
                | "received_ms"
                | "gufi"
                | "present"
                | "errors"
                | "ops"
                | "centre"
                | "reported_owner"
                | "position_ms"
                | "lat_e7"
                | "lon_e7"
                | "reported_alt_ft"
                | "no_status"
                | "owner_unknown"
                | "handoff_incomplete"
                | "handoff_unknown_unit"
                | "handoff_receiving_only"
                | "departure_not_active"
        ),
        "OP" => {
            matches!(
                key,
                "stream"
                    | "offset"
                    | "seq"
                    | "row"
                    | "index"
                    | "kind"
                    | "gufi"
                    | "centre"
                    | "source_ms"
                    | "received_ms"
                    | "message"
                    | "state"
                    | "alt"
                    | "fix"
                    | "unit_a"
                    | "unit_b"
                    | "plan_id"
                    | "address"
                    | "callsign"
                    | "departure"
                    | "arrival"
                    | "outcome"
                    | "reason"
                    | "flight"
                    | "owner"
                    | "handoff_state"
                    | "handoff_from"
                    | "handoff_to"
                    | "accepted_by"
                    | "assigned"
                    | "interim"
                    | "requested"
                    | "plan_state"
            ) || key
                .strip_prefix("stamp")
                .is_some_and(|n| n.parse::<usize>().is_ok_and(|i| i < oa_flight::F_COUNT))
        }
        _ => false,
    }
}

fn require_fields(fields: &Fields<'_>, required: &[&str], line_no: usize) -> Result<(), String> {
    if required.iter().any(|key| !fields.contains_key(key)) {
        return Err(format!(
            "H2 record schema is missing a required field at line {line_no}"
        ));
    }
    Ok(())
}

type Fields<'a> = std::collections::BTreeMap<&'a str, &'a str>;

fn required_field<'a>(fields: &Fields<'a>, key: &str, line_no: usize) -> Result<&'a str, String> {
    fields
        .get(key)
        .copied()
        .ok_or_else(|| format!("H2 record is missing required field {key} at line {line_no}"))
}

fn parse_identity(
    fields: &std::collections::BTreeMap<&str, &str>,
    line_no: usize,
) -> Result<(u32, u64, u64, u64), String> {
    Ok((
        parse_u32(
            fields
                .get("stream")
                .copied()
                .ok_or_else(|| format!("missing stream at line {line_no}"))?,
        )?,
        parse_u64(
            fields
                .get("offset")
                .copied()
                .ok_or_else(|| format!("missing offset at line {line_no}"))?,
        )?,
        parse_u64(
            fields
                .get("seq")
                .copied()
                .ok_or_else(|| format!("missing sequence at line {line_no}"))?,
        )?,
        parse_u64(
            fields
                .get("row")
                .copied()
                .ok_or_else(|| format!("missing row ordinal at line {line_no}"))?,
        )?,
    ))
}

fn validate_op_state_fields(fields: &Fields<'_>, line_no: usize) -> Result<(), String> {
    const STATE_FIELDS: &[&str] = &[
        "owner",
        "handoff_state",
        "handoff_from",
        "handoff_to",
        "accepted_by",
        "assigned",
        "interim",
        "requested",
        "plan_state",
    ];
    match fields.get("flight").copied() {
        Some("absent") => {
            if STATE_FIELDS.iter().any(|key| fields.contains_key(key))
                || fields.keys().any(|key| key.starts_with("stamp"))
            {
                return Err(format!(
                    "absent OP flight has state fields at line {line_no}"
                ));
            }
        }
        Some("present") => {
            require_fields(fields, STATE_FIELDS, line_no)?;
            if fields.keys().filter(|key| key.starts_with("stamp")).count() != oa_flight::F_COUNT {
                return Err(format!(
                    "present OP flight has wrong stamp count at line {line_no}"
                ));
            }
        }
        _ => return Err(format!("invalid OP flight state at line {line_no}")),
    }
    let mut stamp_indices = fields
        .keys()
        .filter_map(|key| {
            key.strip_prefix("stamp")
                .and_then(|suffix| suffix.parse::<usize>().ok())
        })
        .collect::<Vec<_>>();
    stamp_indices.sort_unstable();
    if stamp_indices.iter().copied().ne(0..stamp_indices.len()) {
        return Err(format!("non-contiguous OP stamp fields at line {line_no}"));
    }
    for index in stamp_indices {
        let value = fields
            .get(format!("stamp{index}").as_str())
            .copied()
            .ok_or_else(|| format!("missing OP stamp at line {line_no}"))?;
        let mut values = value.split(',');
        let timestamp = values
            .next()
            .ok_or_else(|| format!("invalid OP stamp at line {line_no}"))?;
        let availability = values
            .next()
            .ok_or_else(|| format!("invalid OP stamp at line {line_no}"))?;
        let message = values
            .next()
            .ok_or_else(|| format!("invalid OP stamp at line {line_no}"))?;
        if values.next().is_some()
            || timestamp.parse::<i64>().is_err()
            || availability.parse::<i64>().is_err()
            || message.parse::<u64>().is_err()
        {
            return Err(format!("invalid OP stamp at line {line_no}"));
        }
    }
    Ok(())
}

fn validate_op_values(fields: &Fields<'_>, line_no: usize) -> Result<(), String> {
    let bad = || format!("invalid H2 OP identity or payload field at line {line_no}");
    let gufi = required_field(fields, "gufi", line_no)?;
    if gufi.len() != 32 || u128::from_str_radix(gufi, 16).is_err() {
        return Err(bad());
    }
    for (key, bytes) in [
        ("centre", 4),
        ("fix", 12),
        ("unit_a", 8),
        ("unit_b", 8),
        ("plan_id", 12),
        ("callsign", 8),
        ("departure", 12),
        ("arrival", 12),
    ] {
        let value = required_field(fields, key, line_no)?;
        if value.len() != bytes * 2 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(bad());
        }
    }
    required_field(fields, "message", line_no)?
        .parse::<u64>()
        .map_err(|_| bad())?;
    required_field(fields, "state", line_no)?
        .parse::<u8>()
        .map_err(|_| bad())?;
    required_field(fields, "address", line_no)?
        .parse::<u32>()
        .map_err(|_| bad())?;
    parse_triplet(required_field(fields, "alt", line_no)?, line_no)?;
    if fields.get("flight") == Some(&"present") {
        for key in ["owner", "handoff_from", "handoff_to", "accepted_by"] {
            let value = required_field(fields, key, line_no)?;
            if value.len() != 16 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(bad());
            }
        }
        for key in ["assigned", "interim", "requested"] {
            parse_triplet(required_field(fields, key, line_no)?, line_no)?;
        }
        required_field(fields, "handoff_state", line_no)?
            .parse::<u8>()
            .map_err(|_| bad())?;
        if required_field(fields, "plan_state", line_no)? != "none" {
            required_field(fields, "plan_state", line_no)?
                .parse::<u8>()
                .map_err(|_| bad())?;
        }
    }
    Ok(())
}

fn parse_triplet(value: &str, line_no: usize) -> Result<(), String> {
    let mut parts = value.split(',');
    let form = parts
        .next()
        .ok_or_else(|| format!("invalid H2 triplet at line {line_no}"))?;
    let first = parts
        .next()
        .ok_or_else(|| format!("invalid H2 triplet at line {line_no}"))?;
    let second = parts
        .next()
        .ok_or_else(|| format!("invalid H2 triplet at line {line_no}"))?;
    if form.parse::<u8>().is_err()
        || first.parse::<i32>().is_err()
        || second.parse::<i32>().is_err()
        || parts.next().is_some()
    {
        return Err(format!("invalid H2 triplet at line {line_no}"));
    }
    Ok(())
}

fn partition_bounds(partition: &str) -> Result<(i64, i64), String> {
    let start = match partition {
        "2026-11-15" => 1_794_700_800_000i64,
        "2026-11-19" => 1_795_046_400_000i64,
        _ => return Err("unsupported frozen partition".into()),
    };
    let end = start
        .checked_add(86_400_000)
        .ok_or("partition time overflow")?;
    Ok((start, end))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
// A7.2: cold struct, size guard waived; grid keys are bounded by MAX_EVIDENCE_KEYS.
struct CellKey {
    lat: i32,
    lon: i32,
    band: i32,
    sector: [u8; 8],
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
// A7.2: cold struct, size guard waived; occupancy rows are bounded by MAX_TRACE_ROWS.
struct Row {
    source_id: (u32, u64, u64, u64),
    record_sha256: [u8; 32],
    gufi: u128,
    key: CellKey,
    position_ms: i64,
    received_ms: i64,
}
#[derive(Default)]
// A7.2: cold struct, size guard waived; aggregate count is stored once per bounded grid key.
struct Counts {
    rows: u64,
}
#[derive(Default)]
// A7.2: cold struct, size guard waived; rows, identities, cells, and records have named caps.
struct Trace {
    producer_commit: String,
    producer_dirty: String,
    input_receipts: u64,
    input_eof_receipts: u64,
    input_non_eof_receipts: u64,
    input_bytes_read: u64,
    input_bytes_reserved: u64,
    rows: Vec<Row>,
    row_records: u64,
    row_ops_expected: u64,
    op_records: u64,
    op_outcomes: [u64; 3],
    op_kind_outcomes: [[u64; 3]; oa_flight::OP_COUNT],
    frame_records: u64,
    frame_dispositions: [u64; 7],
    handoff_ops: [u64; 6],
    handoff_pairs: std::collections::BTreeMap<(u8, String, String), u64>,
    handoff_incomplete_rows: u64,
    handoff_unknown_unit_rows: u64,
    handoff_receiving_only_rows: u64,
    unknown_identity: u64,
    missing_fields: u64,
    missing_altitude: u64,
    out_of_range_altitude: u64,
    duplicate_rows: u64,
    source_conflict_rows: u64,
    state_conflict_rows: u64,
    state_min_ms: Option<i64>,
    state_max_ms: Option<i64>,
    receive_min_ms: Option<i64>,
    receive_max_ms: Option<i64>,
    owner_conflict_rows: u64,
    source_clock_future_rows: u64,
    state_clock_future_rows: u64,
    clock_future_excluded_rows: u64,
    op_clock_future: u64,
    zero_op_rows: u64,
    missing_key_or_time_rows: u64,
}

fn parse_trace(text: &str, partition: &str) -> Result<Trace, String> {
    let mut trace = Trace::default();
    let mut flight_keys = std::collections::BTreeSet::new();
    let mut time_keys = std::collections::BTreeSet::new();
    let mut row_ops = std::collections::BTreeMap::<
        (u32, u64, u64, u64),
        (u64, std::collections::BTreeSet<u64>),
    >::new();
    let mut frame_ids = std::collections::BTreeMap::<(u32, u64), u64>::new();
    let (mut saw_config, mut saw_build, mut saw_result, mut saw_counts) =
        (false, false, false, false);
    let mut input_indices = std::collections::BTreeSet::new();
    let mut reader_error_indices = std::collections::BTreeSet::new();
    let mut op_count_indices = std::collections::BTreeSet::new();
    let mut op_refusal_indices = std::collections::BTreeSet::new();
    let mut reader_error_counts = std::collections::BTreeMap::<u64, u64>::new();
    let mut op_terminal_counts = std::collections::BTreeMap::<u64, (u64, u64)>::new();
    let mut op_refusal_counts = std::collections::BTreeMap::<(u64, u64), u64>::new();
    let mut input_reserved_sum = 0u64;
    let mut saw_footer = false;
    let mut saw_input = false;
    let mut lines = text.lines();
    if lines.next() != Some(H2_HEADER) {
        return Err("trace header/schema mismatch".into());
    }
    for (index, line) in lines.enumerate() {
        let line_no = index + 2;
        if line.len() > 8192 {
            return Err(format!("H2 record exceeds 8192-byte cap at line {line_no}"));
        }
        let mut fields: Fields<'_> = std::collections::BTreeMap::new();
        let kind_name = line.split('\t').next().unwrap_or("");
        if !matches!(
            kind_name,
            "ROW"
                | "OP"
                | "FRAME"
                | "CONFIG"
                | "BUILD"
                | "RESULT"
                | "COUNTS"
                | "READER_ERROR"
                | "OP_COUNTS"
                | "OP_REFUSAL"
                | "INPUT"
        ) {
            return Err(format!("unknown H2 record kind at line {line_no}"));
        }
        if saw_input && kind_name != "INPUT" {
            return Err("H2 INPUT receipts are not the final record group".into());
        }
        for field in line.split('\t').skip(1) {
            let (k, v) = field
                .split_once('=')
                .ok_or_else(|| format!("malformed line {line_no}"))?;
            if !allowed_field(kind_name, k) {
                return Err(format!(
                    "unknown field {k} for {kind_name} at line {line_no}"
                ));
            }
            if fields.insert(k, v).is_some() {
                return Err(format!("duplicate field {k} at line {line_no}"));
            }
        }
        match kind_name {
            "CONFIG" => {
                if saw_config || saw_build || trace.row_records != 0 {
                    return Err("misplaced or duplicate CONFIG record".into());
                }
                require_fields(
                    &fields,
                    &[
                        "from_ns",
                        "to_ns_inclusive",
                        "capacity",
                        "lag_floor_ms",
                        "max_future_ms",
                        "refusal_log",
                        "conflict_log",
                        "removal_log",
                        "sweep_every_ms",
                        "retain_ms",
                        "silence_ms",
                    ],
                    line_no,
                )?;
                for key in [
                    "capacity",
                    "lag_floor_ms",
                    "max_future_ms",
                    "refusal_log",
                    "conflict_log",
                    "removal_log",
                    "sweep_every_ms",
                    "retain_ms",
                    "silence_ms",
                ] {
                    if required_field(&fields, key, line_no)?
                        .parse::<i64>()
                        .is_err()
                    {
                        return Err(format!("invalid H2 CONFIG field {key} at line {line_no}"));
                    }
                }
                let (start_ms, end_ms) = partition_bounds(partition)?;
                let start_ns = start_ms
                    .checked_mul(1_000_000)
                    .ok_or("partition start overflow")?;
                let end_ns = end_ms
                    .checked_mul(1_000_000)
                    .ok_or("partition end overflow")?;
                let from_ns = required_field(&fields, "from_ns", line_no)?
                    .parse::<i64>()
                    .map_err(|_| "invalid CONFIG from_ns")?;
                let to_ns = required_field(&fields, "to_ns_inclusive", line_no)?
                    .parse::<i64>()
                    .map_err(|_| "invalid CONFIG to_ns_inclusive")?;
                if from_ns < start_ns || to_ns < from_ns || to_ns >= end_ns {
                    return Err("H2 CONFIG replay interval is outside declared partition".into());
                }
                saw_config = true;
            }
            "BUILD" => {
                if !saw_config || saw_build || trace.row_records != 0 {
                    return Err("misplaced or duplicate BUILD record".into());
                }
                require_fields(&fields, &["commit", "dirty", "binary_identity"], line_no)?;
                let commit = required_field(&fields, "commit", line_no)?;
                let dirty = required_field(&fields, "dirty", line_no)?;
                let binary_identity = required_field(&fields, "binary_identity", line_no)?;
                if !(commit == "unknown"
                    || (commit.len() == 40
                        && commit
                            .bytes()
                            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))))
                    || !matches!(dirty, "unknown" | "0" | "1")
                    || binary_identity != "outer-build-manifest-required"
                {
                    return Err("invalid H2 BUILD provenance fields".into());
                }
                trace.producer_commit = commit.to_owned();
                trace.producer_dirty = dirty.to_owned();
                saw_build = true;
            }
            "RESULT" => {
                if !saw_build || saw_result {
                    return Err("misplaced or duplicate RESULT record".into());
                }
                require_fields(
                    &fields,
                    &[
                        "status",
                        "qualification",
                        "frames",
                        "decoded_rows",
                        "docs_unparsed",
                        "docs_refused",
                        "reader_loss",
                    ],
                    line_no,
                )?;
                if fields.get("status") != Some(&"measurement-complete")
                    || fields.get("reader_loss") != Some(&"0")
                {
                    return Err("H2 replay status is not complete or has reader loss".into());
                }
                if fields.get("qualification") != Some(&"not-assessed") {
                    return Err("unexpected H2 qualification field".into());
                }
                for key in [
                    "frames",
                    "decoded_rows",
                    "docs_unparsed",
                    "docs_refused",
                    "reader_loss",
                ] {
                    if required_field(&fields, key, line_no)?
                        .parse::<u64>()
                        .is_err()
                    {
                        return Err(format!("invalid H2 RESULT counter {key}"));
                    }
                }
                saw_result = true;
                saw_footer = true;
            }
            "COUNTS" => {
                if !saw_result || saw_counts {
                    return Err("misplaced or duplicate COUNTS record".into());
                }
                require_fields(
                    &fields,
                    &[
                        "frames_visited",
                        "rows_delivered",
                        "events_delivered",
                        "missing_key_or_time",
                        "zero_op_rows",
                        "cap_refused",
                        "sink_failed",
                        "sealed_drops",
                        "merge_files",
                        "merge_bytes",
                        "reader_steps",
                        "decoded_bytes_charged",
                        "raw_input_bytes_reserved",
                        "raw_input_cap_refused",
                    ],
                    line_no,
                )?;
                if fields.get("cap_refused") != Some(&"0")
                    || fields.get("sink_failed") != Some(&"0")
                    || fields.get("sealed_drops") != Some(&"0")
                    || fields.get("raw_input_cap_refused") != Some(&"0")
                {
                    return Err("H2 replay reported a cap, sealed-input, or sink failure".into());
                }
                for key in [
                    "frames_visited",
                    "rows_delivered",
                    "events_delivered",
                    "missing_key_or_time",
                    "zero_op_rows",
                    "cap_refused",
                    "sink_failed",
                    "sealed_drops",
                    "merge_files",
                    "merge_bytes",
                    "reader_steps",
                    "decoded_bytes_charged",
                    "raw_input_bytes_reserved",
                    "raw_input_cap_refused",
                ] {
                    if fields
                        .get(key)
                        .is_none_or(|value| value.parse::<u64>().is_err())
                    {
                        return Err(format!("invalid H2 counter {key}"));
                    }
                }
                input_reserved_sum = parse_u64(required_field(
                    &fields,
                    "raw_input_bytes_reserved",
                    line_no,
                )?)?;
                if input_reserved_sum > 64 * 1024 * 1024 * 1024 {
                    return Err("H2 raw input reservation exceeds 64 GiB cap".into());
                }
                saw_counts = true;
            }
            "INPUT" => {
                if !saw_counts {
                    return Err("INPUT record appears before terminal COUNTS".into());
                }
                saw_input = true;
                require_fields(
                    &fields,
                    &[
                        "index",
                        "path_hex",
                        "opened",
                        "eof",
                        "bytes",
                        "reserved_bytes",
                        "sha256",
                    ],
                    line_no,
                )?;
                let index = parse_u64(required_field(&fields, "index", line_no)?)?;
                if index != input_indices.len() as u64 || !input_indices.insert(index) {
                    return Err("out-of-order or duplicate H2 input index".into());
                }
                let path = required_field(&fields, "path_hex", line_no)?;
                if path.len() % 2 != 0
                    || path.len() > 8192
                    || !path.bytes().all(|b| b.is_ascii_hexdigit())
                {
                    return Err("invalid INPUT path encoding".into());
                }
                let opened = parse_flag(required_field(&fields, "opened", line_no)?)?;
                let eof = parse_flag(required_field(&fields, "eof", line_no)?)?;
                let bytes = parse_u64(required_field(&fields, "bytes", line_no)?)?;
                let reserved = parse_u64(required_field(&fields, "reserved_bytes", line_no)?)?;
                let sha256 = required_field(&fields, "sha256", line_no)?;
                if !opened {
                    return Err(format!(
                        "measurement-complete H2 trace contains unopened input at line {line_no}"
                    ));
                }
                if path.is_empty()
                    || reserved < bytes
                    || (eof && bytes != reserved)
                    || sha256.len() != 64
                    || !sha256.bytes().all(|b| b.is_ascii_hexdigit())
                {
                    return Err("incomplete or inconsistent H2 input receipt".into());
                }
                trace.input_receipts = trace
                    .input_receipts
                    .checked_add(1)
                    .ok_or("input receipt count overflow")?;
                trace.input_bytes_read = trace
                    .input_bytes_read
                    .checked_add(bytes)
                    .ok_or("input byte count overflow")?;
                trace.input_bytes_reserved = trace
                    .input_bytes_reserved
                    .checked_add(reserved)
                    .ok_or("input reservation count overflow")?;
                if eof {
                    trace.input_eof_receipts += 1;
                } else {
                    trace.input_non_eof_receipts += 1;
                }
                input_reserved_sum = input_reserved_sum
                    .checked_sub(reserved)
                    .ok_or("input reservation sum underflow")?;
            }
            "READER_ERROR" | "OP_COUNTS" | "OP_REFUSAL" => {
                if !saw_counts {
                    return Err("terminal counter record appears before COUNTS".into());
                }
                let required: &[&str] = match kind_name {
                    "READER_ERROR" => &["reason", "count"],
                    "OP_COUNTS" => &["kind", "applied", "unchanged"],
                    _ => &["kind", "reason", "count"],
                };
                require_fields(&fields, required, line_no)?;
                for (key, value) in &fields {
                    if !key.is_empty() && value.parse::<u64>().is_err() {
                        return Err(format!("invalid H2 terminal count {key} at line {line_no}"));
                    }
                }
                match kind_name {
                    "READER_ERROR" => {
                        let reason = parse_u64(required_field(&fields, "reason", line_no)?)?;
                        if reason != reader_error_indices.len() as u64
                            || !reader_error_indices.insert(reason)
                        {
                            return Err("out-of-order or duplicate H2 READER_ERROR reason".into());
                        }
                        reader_error_counts.insert(
                            reason,
                            parse_u64(required_field(&fields, "count", line_no)?)?,
                        );
                    }
                    "OP_COUNTS" => {
                        let kind = parse_u64(required_field(&fields, "kind", line_no)?)?;
                        if kind != op_count_indices.len() as u64 || !op_count_indices.insert(kind) {
                            return Err("out-of-order or duplicate H2 OP_COUNTS kind".into());
                        }
                        op_terminal_counts.insert(
                            kind,
                            (
                                parse_u64(required_field(&fields, "applied", line_no)?)?,
                                parse_u64(required_field(&fields, "unchanged", line_no)?)?,
                            ),
                        );
                    }
                    _ => {
                        let kind = parse_u64(required_field(&fields, "kind", line_no)?)?;
                        let reason = parse_u64(required_field(&fields, "reason", line_no)?)?;
                        let key = (kind, reason);
                        let ordinal = kind
                            .saturating_mul(oa_flight::R_COUNT as u64)
                            .saturating_add(reason);
                        if kind >= oa_flight::OP_COUNT as u64
                            || reason >= oa_flight::R_COUNT as u64
                            || ordinal != op_refusal_indices.len() as u64
                            || !op_refusal_indices.insert(key)
                        {
                            return Err("out-of-order or duplicate H2 OP_REFUSAL reason".into());
                        }
                        op_refusal_counts
                            .insert(key, parse_u64(required_field(&fields, "count", line_no)?)?);
                    }
                }
            }
            "FRAME" => {
                require_fields(
                    &fields,
                    &[
                        "stream",
                        "offset",
                        "seq",
                        "row",
                        "received_ns",
                        "disposition",
                        "reason",
                    ],
                    line_no,
                )?;
                let identity = parse_identity(&fields, line_no)?;
                if identity.3 != 0
                    || frame_ids
                        .insert((identity.0, identity.1), identity.2)
                        .is_some()
                {
                    return Err(format!(
                        "invalid or duplicate FRAME source identity at line {line_no}"
                    ));
                }
                parse_u64(required_field(&fields, "received_ns", line_no)?)?;
                let reason = required_field(&fields, "reason", line_no)?;
                let disposition = required_field(&fields, "disposition", line_no)?;
                if reason.parse::<u8>().is_err() || disposition == "6" {
                    return Err(format!("invalid or sealed FRAME at line {line_no}"));
                }
                if saw_footer {
                    return Err("observation appears after H2 terminal RESULT".into());
                }
                trace.frame_records += 1;
                let disposition = disposition
                    .parse::<usize>()
                    .ok()
                    .filter(|v| *v < trace.frame_dispositions.len())
                    .ok_or_else(|| format!("invalid frame disposition at line {line_no}"))?;
                if let Some(count) = trace.frame_dispositions.get_mut(disposition) {
                    *count += 1;
                }
            }
            "OP" => {
                require_fields(
                    &fields,
                    &[
                        "stream",
                        "offset",
                        "seq",
                        "row",
                        "index",
                        "kind",
                        "gufi",
                        "centre",
                        "source_ms",
                        "received_ms",
                        "message",
                        "state",
                        "alt",
                        "fix",
                        "unit_a",
                        "unit_b",
                        "plan_id",
                        "address",
                        "callsign",
                        "departure",
                        "arrival",
                        "outcome",
                        "reason",
                        "flight",
                    ],
                    line_no,
                )?;
                let identity = parse_identity(&fields, line_no)?;
                if saw_footer {
                    return Err("observation appears after H2 terminal RESULT".into());
                }
                if frame_ids.get(&(identity.0, identity.1)) != Some(&identity.2) {
                    return Err(format!(
                        "OP does not join to its FRAME identity at line {line_no}"
                    ));
                }
                let op_source = parse_i64(required_field(&fields, "source_ms", line_no)?)?
                    .ok_or_else(|| format!("OP source_ms is absent at line {line_no}"))?;
                let op_received = parse_i64(required_field(&fields, "received_ms", line_no)?)?
                    .ok_or_else(|| format!("OP received_ms is absent at line {line_no}"))?;
                if op_source > op_received {
                    trace.op_clock_future += 1;
                }
                validate_op_values(&fields, line_no)?;
                let op_index = parse_u64(required_field(&fields, "index", line_no)?)?;
                if let Some((expected, seen)) = row_ops.get_mut(&identity) {
                    if op_index >= *expected || !seen.insert(op_index) {
                        return Err(format!(
                            "OP index does not join to its ROW at line {line_no}"
                        ));
                    }
                } else {
                    return Err(format!("orphan OP has no preceding ROW at line {line_no}"));
                }
                validate_op_state_fields(&fields, line_no)?;
                let kind = fields
                    .get("kind")
                    .and_then(|v| v.parse::<u8>().ok())
                    .filter(|v| usize::from(*v) < oa_flight::OP_COUNT)
                    .ok_or_else(|| format!("invalid operation kind at line {line_no}"))?;
                if let Some(outcome) = fields
                    .get("outcome")
                    .and_then(|v| v.parse::<usize>().ok())
                    .filter(|v| *v < trace.op_outcomes.len())
                {
                    if let Some(count) = trace.op_outcomes.get_mut(outcome) {
                        *count += 1;
                    }
                    let kind_outcomes = trace
                        .op_kind_outcomes
                        .get_mut(usize::from(kind))
                        .ok_or_else(|| format!("invalid operation kind at line {line_no}"))?;
                    let count = kind_outcomes
                        .get_mut(outcome)
                        .ok_or_else(|| format!("invalid operation outcome at line {line_no}"))?;
                    *count += 1;
                } else {
                    return Err(format!("invalid operation outcome at line {line_no}"));
                }
                trace.op_records += 1;
                required_field(&fields, "reason", line_no)?
                    .parse::<u8>()
                    .map_err(|_| format!("invalid operation reason at line {line_no}"))?;
                let slot = match kind {
                    oa_flight::OP_HO_INITIATE => Some(0),
                    oa_flight::OP_HO_UPDATE => Some(1),
                    oa_flight::OP_HO_ACCEPT => Some(2),
                    oa_flight::OP_HO_RETRACT => Some(3),
                    oa_flight::OP_HO_FAIL => Some(4),
                    oa_flight::OP_HO_TAKE => Some(5),
                    _ => None,
                };
                if let Some(slot) = slot {
                    if let Some(count) = trace.handoff_ops.get_mut(slot) {
                        *count += 1;
                    }
                    let from_raw = parse_hex::<8>(
                        fields
                            .get("unit_a")
                            .copied()
                            .ok_or("missing operation unit_a")?,
                    )?;
                    let to_raw = parse_hex::<8>(
                        fields
                            .get("unit_b")
                            .copied()
                            .ok_or("missing operation unit_b")?,
                    )?;
                    let from = hex(&from_raw);
                    let to = hex(&to_raw);
                    *trace.handoff_pairs.entry((kind, from, to)).or_default() += 1;
                }
            }
            "ROW" => {
                require_fields(
                    &fields,
                    &[
                        "stream",
                        "offset",
                        "seq",
                        "row",
                        "source_ms",
                        "received_ms",
                        "gufi",
                        "present",
                        "errors",
                        "ops",
                        "centre",
                        "reported_owner",
                        "position_ms",
                        "lat_e7",
                        "lon_e7",
                        "reported_alt_ft",
                        "no_status",
                        "owner_unknown",
                        "handoff_incomplete",
                        "handoff_unknown_unit",
                        "handoff_receiving_only",
                        "departure_not_active",
                    ],
                    line_no,
                )?;
                let source_id = parse_identity(&fields, line_no)?;
                if saw_footer {
                    return Err("observation appears after H2 terminal RESULT".into());
                }
                if frame_ids.get(&(source_id.0, source_id.1)) != Some(&source_id.2) {
                    return Err(format!(
                        "ROW does not join to its FRAME identity at line {line_no}"
                    ));
                }
                let required = [
                    "gufi",
                    "present",
                    "errors",
                    "centre",
                    "reported_owner",
                    "position_ms",
                    "lat_e7",
                    "lon_e7",
                    "reported_alt_ft",
                    "received_ms",
                    "owner_unknown",
                    "handoff_incomplete",
                    "handoff_unknown_unit",
                    "handoff_receiving_only",
                    "ops",
                ];
                if required.iter().any(|k| !fields.contains_key(k)) {
                    return Err(format!("H2 schema mismatch at row {line_no}"));
                }
                let expected_ops = parse_u64(required_field(&fields, "ops", line_no)?)?;
                match row_ops.get(&source_id) {
                    Some((previous, _)) if *previous != expected_ops || expected_ops != 0 => {
                        return Err(format!(
                            "duplicate or inconsistent ROW operation identity at line {line_no}"
                        ))
                    }
                    Some(_) => {}
                    None => {
                        row_ops
                            .insert(source_id, (expected_ops, std::collections::BTreeSet::new()));
                    }
                }
                let source_ms = parse_i64(required_field(&fields, "source_ms", line_no)?)?;
                let received_ms = parse_i64(required_field(&fields, "received_ms", line_no)?)?
                    .ok_or_else(|| format!("ROW received_ms is absent at line {line_no}"))?;
                let position_ms = parse_i64(required_field(&fields, "position_ms", line_no)?)?;
                let future_state = position_ms.is_some_and(|time| time > received_ms);
                let future_source = source_ms.is_some_and(|time| time > received_ms);
                trace.state_clock_future_rows += u64::from(future_state);
                trace.source_clock_future_rows += u64::from(future_source);
                trace.clock_future_excluded_rows += u64::from(future_state || future_source);
                trace.row_records += 1;
                if trace.row_records > MAX_TRACE_ROWS as u64 {
                    return Err("row cap exceeded; INCOMPLETE".into());
                }
                trace.zero_op_rows += u64::from(expected_ops == 0);
                trace.missing_key_or_time_rows += u64::from(
                    required_field(&fields, "gufi", line_no)? == "none" || source_ms.is_none(),
                );
                trace.row_ops_expected = trace
                    .row_ops_expected
                    .checked_add(parse_u64(
                        fields
                            .get("ops")
                            .copied()
                            .ok_or("missing row operation count")?,
                    )?)
                    .ok_or("row operation count overflow")?;
                trace.handoff_incomplete_rows += u64::from(parse_flag(
                    fields
                        .get("handoff_incomplete")
                        .copied()
                        .ok_or("missing handoff flag")?,
                )?);
                trace.handoff_unknown_unit_rows += u64::from(parse_flag(
                    fields
                        .get("handoff_unknown_unit")
                        .copied()
                        .ok_or("missing handoff flag")?,
                )?);
                trace.handoff_receiving_only_rows += u64::from(parse_flag(
                    fields
                        .get("handoff_receiving_only")
                        .copied()
                        .ok_or("missing handoff flag")?,
                )?);
                for flag in [
                    "no_status",
                    "owner_unknown",
                    "handoff_incomplete",
                    "handoff_unknown_unit",
                    "handoff_receiving_only",
                    "departure_not_active",
                ] {
                    let _ = parse_flag(required_field(&fields, flag, line_no)?)?;
                }
                if future_state || future_source {
                    continue;
                }
                let present = parse_opt_u64(required_field(&fields, "present", line_no)?)?;
                let errors = parse_opt_u64(required_field(&fields, "errors", line_no)?)?;
                let bits = [
                    oa_fixm::Field::Centre,
                    oa_fixm::Field::ControllingUnit,
                    oa_fixm::Field::Position,
                    oa_fixm::Field::PositionTime,
                    oa_fixm::Field::ReportedAlt,
                ]
                .iter()
                .fold(0u64, |a, f| a | (1u64 << (*f as u8)));
                if present.is_none_or(|p| p & bits != bits) || errors.is_none_or(|e| e & bits != 0)
                {
                    trace.missing_fields += 1;
                    continue;
                }
                if fields.get("owner_unknown") != Some(&"0") {
                    trace.unknown_identity += 1;
                    continue;
                }
                let centre = parse_hex::<4>(fields.get("centre").copied().unwrap_or(""))?;
                if centre != [b'Z', b'A', b'U', 0] {
                    trace.unknown_identity += 1;
                    continue;
                }
                let sector = parse_hex::<8>(fields.get("reported_owner").copied().unwrap_or(""))?;
                let unit_bytes = sector.get(..4).ok_or("invalid unit identity width")?;
                if !unit_bytes
                    .iter()
                    .all(|b| b.is_ascii_alphanumeric() || *b == 0)
                    || unit_bytes.iter().all(|b| *b == 0)
                    || !sector
                        .iter()
                        .skip(4)
                        .all(|b| b.is_ascii_alphanumeric() || *b == 0)
                {
                    trace.unknown_identity += 1;
                    continue;
                }
                let gufi_text = required_field(&fields, "gufi", line_no)?;
                let gufi = if gufi_text == "none" {
                    None
                } else {
                    if gufi_text.len() != 32 {
                        return Err(format!("invalid GUFI width at line {line_no}"));
                    }
                    Some(
                        u128::from_str_radix(gufi_text, 16)
                            .map_err(|_| format!("invalid GUFI at line {line_no}"))?,
                    )
                };
                let Some(gufi) = gufi else {
                    trace.unknown_identity += 1;
                    continue;
                };
                let lat = parse_i32(fields.get("lat_e7").copied().unwrap_or("none"))?;
                let lon = parse_i32(fields.get("lon_e7").copied().unwrap_or("none"))?;
                let altitude = parse_i32(fields.get("reported_alt_ft").copied().unwrap_or("none"))?;
                let (Some(lat), Some(lon)) = (lat, lon) else {
                    trace.missing_fields += 1;
                    continue;
                };
                if !(-900_000_000..=900_000_000).contains(&lat)
                    || !(-1_800_000_000..=1_800_000_000).contains(&lon)
                {
                    trace.missing_fields += 1;
                    continue;
                }
                let Some(alt) = altitude else {
                    trace.missing_altitude += 1;
                    continue;
                };
                if !(-2_000..=100_000).contains(&alt) {
                    trace.out_of_range_altitude += 1;
                    continue;
                }
                let Some(position_ms) = position_ms else {
                    trace.missing_fields += 1;
                    continue;
                };
                let cell = CellKey {
                    lat: lat.div_euclid(500_000),
                    lon: lon.div_euclid(500_000),
                    band: (alt + 2_000).div_euclid(2_000),
                    sector,
                };
                trace.rows.push(Row {
                    source_id,
                    record_sha256: oa_sha256::sha256(line.as_bytes()),
                    gufi,
                    key: cell,
                    position_ms,
                    received_ms,
                });
                flight_keys.insert((cell, gufi));
                time_keys.insert((cell, position_ms.div_euclid(900_000)));
                if flight_keys.len() + time_keys.len() > MAX_EVIDENCE_KEYS {
                    return Err("distinct evidence-key cap exceeded; INCOMPLETE".into());
                }
            }
            _ => return Err("unknown trace record".into()),
        }
    }
    if !saw_config
        || !saw_build
        || !saw_result
        || !saw_counts
        || input_indices.is_empty()
        || input_reserved_sum != 0
        || input_indices
            .iter()
            .copied()
            .ne(0..input_indices.len() as u64)
        || reader_error_indices.len() != H2_READER_ERROR_COUNT
        || reader_error_indices
            .iter()
            .copied()
            .ne(0..H2_READER_ERROR_COUNT as u64)
        || op_count_indices.len() != oa_flight::OP_COUNT
        || op_count_indices
            .iter()
            .copied()
            .ne(0..oa_flight::OP_COUNT as u64)
        || op_refusal_indices.len() != oa_flight::OP_COUNT * oa_flight::R_COUNT
    {
        return Err(
            "incomplete H2 envelope, terminal counter matrix, or input reservation conservation"
                .into(),
        );
    }
    if trace.row_records != parse_u64(count_field(text, "rows_delivered")?)?
        || trace.frame_records + trace.op_records + trace.row_records
            != parse_u64(count_field(text, "events_delivered")?)?
    {
        return Err("H2 event counter conservation failed".into());
    }
    if trace.frame_records != parse_u64(count_field(text, "frames_visited")?)? {
        return Err("H2 frame counter conservation failed".into());
    }
    let result_counts = result_fields(text)?;
    let result_frames = parse_u64(required_field(&result_counts, "frames", 0)?)?;
    let result_rows = parse_u64(required_field(&result_counts, "decoded_rows", 0)?)?;
    let result_unparsed = parse_u64(required_field(&result_counts, "docs_unparsed", 0)?)?;
    let result_refused = parse_u64(required_field(&result_counts, "docs_refused", 0)?)?;
    let decoded = trace
        .frame_dispositions
        .first()
        .copied()
        .ok_or("missing frame dispositions")?;
    let unparsed = trace
        .frame_dispositions
        .get(4)
        .copied()
        .ok_or("missing unparsed disposition")?;
    let refused = trace
        .frame_dispositions
        .get(5)
        .copied()
        .ok_or("missing refused disposition")?;
    let h2_reported_frames = decoded
        .checked_add(unparsed)
        .and_then(|n| n.checked_add(refused))
        .ok_or("H2 result frame count overflow")?;
    if result_frames != h2_reported_frames
        || result_rows != trace.row_records
        || result_unparsed != unparsed
        || result_refused != refused
    {
        return Err("H2 RESULT counters disagree with observed FRAME/ROW dispositions".into());
    }
    if trace.zero_op_rows != parse_u64(count_field(text, "zero_op_rows")?)?
        || trace.missing_key_or_time_rows != parse_u64(count_field(text, "missing_key_or_time")?)?
    {
        return Err("H2 row counter conservation failed".into());
    }
    if reader_error_counts.values().any(|count| *count != 0) {
        return Err("H2 reader error counters indicate source loss".into());
    }
    for kind in 0..oa_flight::OP_COUNT {
        let kind_key = kind as u64;
        let (applied, unchanged) = *op_terminal_counts
            .get(&kind_key)
            .ok_or("H2 OP_COUNTS matrix incomplete")?;
        let refused = (0..oa_flight::R_COUNT)
            .map(|reason| {
                op_refusal_counts
                    .get(&(kind_key, reason as u64))
                    .copied()
                    .unwrap_or(0)
            })
            .sum::<u64>();
        let observed = trace
            .op_kind_outcomes
            .get(kind)
            .ok_or("H2 operation kind matrix incomplete")?
            .iter()
            .sum::<u64>();
        if applied
            .checked_add(unchanged)
            .and_then(|n| n.checked_add(refused))
            != Some(observed)
        {
            return Err(format!(
                "H2 operation terminal counters disagree for kind {kind}"
            ));
        }
    }
    for (identity, (expected, indexes)) in &row_ops {
        if indexes.len() as u64 != *expected || indexes.iter().copied().ne(0..*expected) {
            return Err(format!(
                "ROW/OP per-row index conservation failed for source identity {identity:?}"
            ));
        }
    }
    let mut by_source = std::collections::BTreeMap::<(u32, u64, u64, u64), Vec<Row>>::new();
    for row in trace.rows.drain(..) {
        by_source.entry(row.source_id).or_default().push(row);
    }
    let mut unique_rows = Vec::with_capacity(by_source.len());
    for group in by_source.into_values() {
        let Some(first) = group.first().copied() else {
            continue;
        };
        if group.iter().all(|row| *row == first) {
            trace.duplicate_rows += group.len().saturating_sub(1) as u64;
            unique_rows.push(first);
        } else {
            trace.source_conflict_rows += group.len() as u64;
        }
    }
    trace.rows = unique_rows;
    let mut state_owners = std::collections::BTreeMap::<
        (u128, i64),
        (
            std::collections::BTreeSet<[u8; 8]>,
            std::collections::BTreeSet<CellKey>,
        ),
    >::new();
    for row in &trace.rows {
        let entry = state_owners.entry((row.gufi, row.position_ms)).or_default();
        entry.0.insert(row.key.sector);
        entry.1.insert(row.key);
    }
    if trace.op_records != trace.row_ops_expected {
        return Err("ROW/OP conservation mismatch; INCOMPLETE".into());
    }
    let conflicted: std::collections::BTreeSet<(u128, i64)> = state_owners
        .iter()
        .filter_map(|(id, (owners, cells))| (owners.len() > 1 || cells.len() > 1).then_some(*id))
        .collect();
    if !conflicted.is_empty() {
        trace.owner_conflict_rows = trace
            .rows
            .iter()
            .filter(|r| {
                conflicted.contains(&(r.gufi, r.position_ms))
                    && state_owners
                        .get(&(r.gufi, r.position_ms))
                        .is_some_and(|v| v.0.len() > 1)
            })
            .count() as u64;
        trace.state_conflict_rows = trace
            .rows
            .iter()
            .filter(|r| {
                conflicted.contains(&(r.gufi, r.position_ms))
                    && state_owners
                        .get(&(r.gufi, r.position_ms))
                        .is_some_and(|v| v.0.len() == 1)
            })
            .count() as u64;
        trace
            .rows
            .retain(|r| !conflicted.contains(&(r.gufi, r.position_ms)));
    }
    for row in &trace.rows {
        trace.state_min_ms = Some(
            trace
                .state_min_ms
                .map_or(row.position_ms, |v| v.min(row.position_ms)),
        );
        trace.state_max_ms = Some(
            trace
                .state_max_ms
                .map_or(row.position_ms, |v| v.max(row.position_ms)),
        );
        trace.receive_min_ms = Some(
            trace
                .receive_min_ms
                .map_or(row.received_ms, |v| v.min(row.received_ms)),
        );
        trace.receive_max_ms = Some(
            trace
                .receive_max_ms
                .map_or(row.received_ms, |v| v.max(row.received_ms)),
        );
    }
    let accounted = trace.unknown_identity
        + trace.missing_fields
        + trace.missing_altitude
        + trace.out_of_range_altitude
        + trace.clock_future_excluded_rows
        + trace.duplicate_rows
        + trace.source_conflict_rows
        + trace.owner_conflict_rows
        + trace.state_conflict_rows
        + trace.rows.len() as u64;
    if accounted != trace.row_records {
        return Err("row disposition conservation failed; INCOMPLETE".into());
    }
    Ok(trace)
}

fn parse_opt_u64(s: &str) -> Result<Option<u64>, String> {
    if s == "none" {
        Ok(None)
    } else {
        s.parse()
            .map(Some)
            .map_err(|_| "invalid unsigned integer field".into())
    }
}
fn parse_i64(s: &str) -> Result<Option<i64>, String> {
    if s == "none" {
        Ok(None)
    } else {
        s.parse()
            .map(Some)
            .map_err(|_| "invalid integer field".into())
    }
}
fn parse_i32(s: &str) -> Result<Option<i32>, String> {
    if s == "none" {
        Ok(None)
    } else {
        s.parse()
            .map(Some)
            .map_err(|_| "invalid integer field".into())
    }
}
fn parse_u32(s: &str) -> Result<u32, String> {
    s.parse()
        .map_err(|_| "invalid unsigned source identity".into())
}
fn parse_u64(s: &str) -> Result<u64, String> {
    s.parse()
        .map_err(|_| "invalid unsigned source identity".into())
}
fn parse_flag(s: &str) -> Result<bool, String> {
    match s {
        "0" => Ok(false),
        "1" => Ok(true),
        _ => Err("invalid boolean field".into()),
    }
}
fn parse_hex<const N: usize>(s: &str) -> Result<[u8; N], String> {
    if s.len() != N * 2 {
        return Err("fixed-width identity has wrong length".into());
    }
    let mut out = [0; N];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(s.get(i * 2..i * 2 + 2).ok_or("identity slice")?, 16)
            .map_err(|_| "invalid identity hex")?;
    }
    Ok(out)
}

type FlightEvidence = std::collections::BTreeMap<CellKey, std::collections::BTreeSet<u128>>;
type TimeBinEvidence = std::collections::BTreeMap<CellKey, std::collections::BTreeSet<i64>>;

// A7.2: bounded cold aggregation for the offline evidence report.
struct Aggregates {
    counts: std::collections::BTreeMap<CellKey, Counts>,
    flights: FlightEvidence,
    bins: TimeBinEvidence,
}

fn aggregate(trace: &Trace) -> Aggregates {
    let mut counts = std::collections::BTreeMap::new();
    let mut flights =
        std::collections::BTreeMap::<CellKey, std::collections::BTreeSet<u128>>::new();
    let mut bins = std::collections::BTreeMap::<CellKey, std::collections::BTreeSet<i64>>::new();
    for row in &trace.rows {
        counts.entry(row.key).or_insert_with(Counts::default).rows += 1;
        flights.entry(row.key).or_default().insert(row.gufi);
        bins.entry(row.key)
            .or_default()
            .insert(row.position_ms.div_euclid(900_000));
    }
    Aggregates {
        counts,
        flights,
        bins,
    }
}

// A7.2: cold report builder, size guard waived; every append is checked against the declared cap.
struct ReportWriter {
    text: String,
    limit: usize,
}

impl ReportWriter {
    fn new(limit: usize) -> Self {
        Self {
            text: String::new(),
            limit,
        }
    }
    fn line(&mut self, args: std::fmt::Arguments<'_>) -> Result<(), String> {
        use std::fmt::Write as _;
        self.write_fmt(args)
            .map_err(|_| "report output cap exceeded; INCOMPLETE".into())
    }
    fn finish(self) -> String {
        self.text
    }
}

impl std::fmt::Write for ReportWriter {
    fn write_str(&mut self, value: &str) -> std::fmt::Result {
        let Some(next) = self.text.len().checked_add(value.len()) else {
            return Err(std::fmt::Error);
        };
        if next > self.limit {
            return Err(std::fmt::Error);
        }
        self.text.push_str(value);
        Ok(())
    }
}

fn train_report(
    trace: &Trace,
    digest: &str,
    binary_digest: &str,
    policy_digest: &str,
) -> Result<String, String> {
    train_report_with_limit(
        trace,
        digest,
        binary_digest,
        policy_digest,
        MAX_OUTPUT_BYTES,
    )
}

fn train_report_with_limit(
    trace: &Trace,
    digest: &str,
    binary_digest: &str,
    policy_digest: &str,
    max_bytes: usize,
) -> Result<String, String> {
    let Aggregates {
        counts,
        flights,
        bins,
    } = aggregate(trace);
    let mut supported_labels = std::collections::BTreeMap::<(i32, i32, i32), usize>::new();
    for key in counts.keys() {
        let nflight = flights.get(key).map_or(0, |s| s.len());
        let nbin = bins.get(key).map_or(0, |s| s.len());
        if nflight >= 3 && nbin >= 2 {
            *supported_labels
                .entry((key.lat, key.lon, key.band))
                .or_default() += 1;
        }
    }
    let mut out = ReportWriter::new(max_bytes);
    out.line(format_args!("OA-G3-OCCUPANCY-1\ncomplete=true\ncompletion_scope=selected-window\nproducer_commit={}\nproducer_dirty={}\ninput_receipts={}\ninput_eof_receipts={}\ninput_non_eof_receipts={}\ninput_bytes_read={}\ninput_bytes_reserved={}\nclaim=empirical_observed_occupancy_only_not_legal_geometry\nsource_schema=H2-OBSERVED-STATE-1\nscope=SFDPS-replay-compatibility\nindependent_predictions=not-measured\nindependent_agreement_denominator=not-observable\nsource_revision={}\nsource_reconstruction_claim=none_exact_trace_and_binary_reproducibility_only\ntraining_partition=2026-11-15\ntraining_sha256={digest}\nbinary_sha256={binary_digest}\npolicy_sha256={policy_digest}\ngrid_e7=500000\naltitude_slab_ft=2000\ntime_bin_ms=900000\ngrid_scope=global_wgs84_no_official_coverage_mask\nrows_total={}\nrows_eligible={}\nunknown_identity={}\nmissing_required_or_invalid={}\nmissing_reported_altitude={}\nout_of_range_altitude={}\nclock_future_excluded_rows={}\nsource_clock_future_rows={}\nstate_clock_future_rows={}\nop_source_future_vs_receive={}\nclock_order_is_causal_proof=false\nduplicate_exact_records={}\nsource_identity_conflict_rows={}\nowner_conflict_rows={}\nstate_conflict_rows={}\nstate_time_min_ms={}\nstate_time_max_ms={}\nreceive_time_min_ms={}\nreceive_time_max_ms={}\nframes={}\nframe_dispositions_decoded_before_after_other_xml_refused_decoder_refused_sealed={:?}\noperations={}\noperation_outcomes_applied_unchanged_refused={:?}\nrow_handoff_incomplete={}\nrow_handoff_unknown_unit={}\nrow_handoff_receiving_only={}\nhandoff_initiate_update_accept_retract_fail_take={:?}\nminimum_flights=3\nminimum_time_bins=2\nsupport_is_not_probability=true\n", trace.producer_commit, trace.producer_dirty, trace.input_receipts, trace.input_eof_receipts, trace.input_non_eof_receipts, trace.input_bytes_read, trace.input_bytes_reserved, option_env!("OPENATCA_SOURCE_REVISION").unwrap_or("not-embedded"), trace.row_records, trace.rows.len(), trace.unknown_identity, trace.missing_fields, trace.missing_altitude, trace.out_of_range_altitude, trace.clock_future_excluded_rows, trace.source_clock_future_rows, trace.state_clock_future_rows, trace.op_clock_future, trace.duplicate_rows, trace.source_conflict_rows, trace.owner_conflict_rows, trace.state_conflict_rows, optnum(trace.state_min_ms), optnum(trace.state_max_ms), optnum(trace.receive_min_ms), optnum(trace.receive_max_ms), trace.frame_records, trace.frame_dispositions, trace.op_records, trace.op_outcomes, trace.handoff_incomplete_rows, trace.handoff_unknown_unit_rows, trace.handoff_receiving_only_rows, trace.handoff_ops))?;
    for ((kind, from, to), n) in &trace.handoff_pairs {
        out.line(format_args!("handoff\t{kind}\t{from}\t{to}\t{n}\n"))?;
    }
    for (key, count) in counts {
        let nflight = flights.get(&key).map_or(0, |s| s.len());
        let nbin = bins.get(&key).map_or(0, |s| s.len());
        let supported = nflight >= 3 && nbin >= 2;
        let competing = supported_labels
            .get(&(key.lat, key.lon, key.band))
            .copied()
            .unwrap_or(0)
            > 1;
        let class = if competing && supported {
            "contested"
        } else if supported {
            "supported"
        } else {
            "unknown"
        };
        out.line(format_args!(
            "cell\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            key.lat,
            key.lon,
            key.band,
            hex(&key.sector),
            count.rows,
            nflight,
            nbin
        ))?;
        out.line(format_args!(
            "class\t{}\t{}\t{}\t{}\t{}\n",
            key.lat,
            key.lon,
            key.band,
            hex(&key.sector),
            class
        ))?;
    }
    Ok(out.finish())
}

fn check_report(
    trace: &Trace,
    digest: &str,
    binary_digest: &str,
    policy_digest: &str,
    model_digest: &str,
    model: &str,
) -> Result<String, String> {
    check_report_with_limit(
        trace,
        digest,
        binary_digest,
        policy_digest,
        model_digest,
        model,
        MAX_OUTPUT_BYTES,
    )
}

fn check_report_with_limit(
    trace: &Trace,
    digest: &str,
    binary_digest: &str,
    policy_digest: &str,
    model_digest: &str,
    model: &str,
    max_bytes: usize,
) -> Result<String, String> {
    if !model.starts_with("OA-G3-OCCUPANCY-1\n") {
        return Err("unsupported training model schema".into());
    }
    if !model.contains("complete=true\n") {
        return Err("training model is incomplete".into());
    }
    if field_value(model, "completion_scope")? != "selected-window" {
        return Err("training model completion scope is not selected-window".into());
    }
    if !model.contains("claim=empirical_observed_occupancy_only_not_legal_geometry\n") || !model.contains("scope=SFDPS-replay-compatibility\nindependent_predictions=not-measured\nindependent_agreement_denominator=not-observable\n") { return Err("training artifact claim mismatch".into()); }
    if field_value(model, "source_reconstruction_claim")?
        != "none_exact_trace_and_binary_reproducibility_only"
    {
        return Err("training artifact provenance claim mismatch".into());
    }
    if field_value(model, "training_partition")? != "2026-11-15" {
        return Err("training artifact has wrong partition".into());
    }
    if !model.contains("grid_e7=500000\naltitude_slab_ft=2000\n") {
        return Err("training policy mismatch".into());
    }
    if field_value(model, "policy_sha256")? != policy_digest {
        return Err("training policy digest mismatch".into());
    }
    let train_binary = field_value(model, "binary_sha256")?;
    if train_binary != binary_digest {
        return Err("training/check binary digest mismatch".into());
    }
    let source_revision = option_env!("OPENATCA_SOURCE_REVISION").unwrap_or("not-embedded");
    if field_value(model, "source_revision")? != source_revision {
        return Err("training/check source revision mismatch".into());
    }
    let train_commit = field_value(model, "producer_commit")?;
    let train_dirty = field_value(model, "producer_dirty")?;
    if !(train_commit == "unknown"
        || (train_commit.len() == 40
            && train_commit
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))))
        || !matches!(train_dirty, "unknown" | "0" | "1")
    {
        return Err("training producer provenance is invalid".into());
    }
    let mut train = std::collections::BTreeMap::<(i32, i32, i32, String), String>::new();
    for line in model.lines().filter(|line| line.starts_with("class\t")) {
        let mut p = line.split('\t');
        if p.next() != Some("class") {
            return Err("malformed class row in training model".into());
        }
        let lat = p
            .next()
            .ok_or("class row latitude missing")?
            .parse()
            .map_err(|_| "class row latitude invalid")?;
        let lon = p
            .next()
            .ok_or("class row longitude missing")?
            .parse()
            .map_err(|_| "class row longitude invalid")?;
        let band = p
            .next()
            .ok_or("class row altitude band missing")?
            .parse()
            .map_err(|_| "class row altitude band invalid")?;
        let sector = p.next().ok_or("class row owner missing")?.to_owned();
        let class = p.next().ok_or("class row label missing")?;
        if p.next().is_some() || !matches!(class, "supported" | "contested" | "unknown") {
            return Err("invalid class row in training model".into());
        }
        if train
            .insert((lat, lon, band, sector), class.to_owned())
            .is_some()
        {
            return Err("duplicate class row in training model".into());
        }
    }
    let train_cells: std::collections::BTreeSet<(i32, i32, i32)> = train
        .keys()
        .map(|(lat, lon, band, _)| (*lat, *lon, *band))
        .collect();
    let Aggregates {
        counts,
        flights,
        bins,
    } = aggregate(trace);
    let mut eligible = 0u64;
    let mut sparse = 0u64;
    let mut overlap = 0u64;
    let mut contested_rows = 0u64;
    let mut training_unknown_rows = 0u64;
    let mut other_owner_rows = 0u64;
    let mut no_training_cell_rows = 0u64;
    for (key, count) in &counts {
        let nflight = flights.get(key).map_or(0, |s| s.len());
        let nbin = bins.get(key).map_or(0, |s| s.len());
        if nflight < 3 || nbin < 2 {
            sparse += count.rows;
            continue;
        }
        eligible += count.rows;
        let label_key = (key.lat, key.lon, key.band, hex(&key.sector));
        match train.get(&label_key).map(String::as_str) {
            Some("supported") => overlap += count.rows,
            Some("contested") => contested_rows += count.rows,
            Some("unknown") => training_unknown_rows += count.rows,
            _ if train_cells.contains(&(key.lat, key.lon, key.band)) => {
                other_owner_rows += count.rows
            }
            _ => no_training_cell_rows += count.rows,
        }
    }
    let without_supported =
        contested_rows + training_unknown_rows + other_owner_rows + no_training_cell_rows;
    let mut out = ReportWriter::new(max_bytes);
    out.line(format_args!("OA-G3-CHECK-1\nclaim=empirical_occupancy_transfer_only_not_boundary_accuracy\ncompletion_scope=selected-window\nproducer_commit={}\nproducer_dirty={}\ninput_receipts={}\ninput_eof_receipts={}\ninput_non_eof_receipts={}\ninput_bytes_read={}\ninput_bytes_reserved={}\nsource_schema=H2-OBSERVED-STATE-1\nscope=SFDPS-replay-compatibility\nindependent_predictions=not-measured\nindependent_agreement_denominator=not-observable\nsource_revision={}\nsource_reconstruction_claim=none_exact_trace_and_binary_reproducibility_only\ncheck_partition=2026-11-19\ncheck_sha256={digest}\nbinary_sha256={binary_digest}\npolicy_sha256={policy_digest}\ntraining_artifact_sha256={model_digest}\ntraining_sha256={}\ntraining_binary_sha256={train_binary}\nrows_total={}\nrows_eligible_after_conflicts={}\ncheck_rows_with_min_support={}\ncheck_rows_sparse_unknown={}\ncheck_rows_matching_supported_training_cell={}\ncheck_rows_without_supported_training_label={}\ncheck_rows_training_contested={}\ncheck_rows_training_unknown={}\ncheck_rows_other_owner_in_observed_cell={}\ncheck_rows_without_training_cell={}\nunknown_identity={}\nmissing_required_or_invalid={}\nmissing_reported_altitude={}\nout_of_range_altitude={}\nclock_future_excluded_rows={}\nsource_clock_future_rows={}\nstate_clock_future_rows={}\nop_source_future_vs_receive={}\nclock_order_is_causal_proof=false\nduplicate_exact_records={}\nsource_identity_conflict_rows={}\nowner_conflict_rows={}\nstate_conflict_rows={}\nstate_time_min_ms={}\nstate_time_max_ms={}\nreceive_time_min_ms={}\nreceive_time_max_ms={}\nframes={}\nframe_dispositions_decoded_before_after_other_xml_refused_decoder_refused_sealed={:?}\noperations={}\noperation_outcomes_applied_unchanged_refused={:?}\nrow_handoff_incomplete={}\nrow_handoff_unknown_unit={}\nrow_handoff_receiving_only={}\nhandoff_initiate_update_accept_retract_fail_take={:?}\n", trace.producer_commit, trace.producer_dirty, trace.input_receipts, trace.input_eof_receipts, trace.input_non_eof_receipts, trace.input_bytes_read, trace.input_bytes_reserved, option_env!("OPENATCA_SOURCE_REVISION").unwrap_or("not-embedded"), field_value(model,"training_sha256")?, trace.row_records, trace.rows.len(), eligible, sparse, overlap, without_supported, contested_rows, training_unknown_rows, other_owner_rows, no_training_cell_rows, trace.unknown_identity, trace.missing_fields, trace.missing_altitude, trace.out_of_range_altitude, trace.clock_future_excluded_rows, trace.source_clock_future_rows, trace.state_clock_future_rows, trace.op_clock_future, trace.duplicate_rows, trace.source_conflict_rows, trace.owner_conflict_rows, trace.state_conflict_rows, optnum(trace.state_min_ms), optnum(trace.state_max_ms), optnum(trace.receive_min_ms), optnum(trace.receive_max_ms), trace.frame_records, trace.frame_dispositions, trace.op_records, trace.op_outcomes, trace.handoff_incomplete_rows, trace.handoff_unknown_unit_rows, trace.handoff_receiving_only_rows, trace.handoff_ops))?;
    for ((kind, from, to), n) in &trace.handoff_pairs {
        out.line(format_args!("handoff\t{kind}\t{from}\t{to}\t{n}\n"))?;
    }
    Ok(out.finish())
}

fn field_value<'a>(text: &'a str, key: &str) -> Result<&'a str, String> {
    text.lines()
        .find_map(|l| l.strip_prefix(key).and_then(|v| v.strip_prefix('=')))
        .ok_or_else(|| format!("training model missing {key}"))
}
fn count_field<'a>(text: &'a str, key: &'a str) -> Result<&'a str, String> {
    text.lines()
        .find(|line| line.starts_with("COUNTS\t"))
        .and_then(|line| {
            line.split('\t')
                .skip(1)
                .find_map(|part| part.strip_prefix(key).and_then(|v| v.strip_prefix('=')))
        })
        .ok_or_else(|| format!("H2 COUNTS missing {key}"))
}
fn result_fields(text: &str) -> Result<std::collections::BTreeMap<&str, &str>, String> {
    let line = text
        .lines()
        .find(|line| line.starts_with("RESULT\t"))
        .ok_or("H2 RESULT missing")?;
    let mut fields = std::collections::BTreeMap::new();
    for field in line.split('\t').skip(1) {
        let (key, value) = field.split_once('=').ok_or("malformed H2 RESULT field")?;
        if fields.insert(key, value).is_some() {
            return Err("duplicate H2 RESULT field".into());
        }
    }
    Ok(fields)
}
fn optnum(value: Option<i64>) -> String {
    value.map_or_else(|| "none".into(), |v| v.to_string())
}
fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

fn publish_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    oa_sealed::refuse_io(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("output has no parent"))?;
    let name = path
        .file_name()
        .ok_or_else(|| std::io::Error::other("output has no filename"))?
        .to_string_lossy();
    let partial = parent.join(format!(".{name}.partial-{}", std::process::id()));
    oa_sealed::refuse_io(&partial)?;
    // seal-allow: exclusive write-only mode-0600 staging file after both final and partial path refusal checks.
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&partial)?;
    let write_result = file.write_all(bytes).and_then(|()| file.sync_all());
    drop(file);
    if let Err(error) = write_result {
        let _ = std::fs::remove_file(&partial);
        return Err(error);
    }
    if let Err(error) = oa_sealed::refuse_io(path) {
        let _ = std::fs::remove_file(&partial);
        return Err(error);
    }
    // seal-allow: no-replace link between this invocation's guarded staging file and guarded final path.
    if let Err(error) = std::fs::hard_link(&partial, path) {
        let _ = std::fs::remove_file(&partial);
        return Err(error);
    }
    std::fs::remove_file(&partial)?;
    // seal-allow: opens the already-guarded private output directory solely to sync directory metadata.
    OpenOptions::new().read(true).open(parent)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    #[test]
    fn cli_arguments_are_bounded_by_count_and_encoded_bytes() {
        let valid = vec!["x".to_owned(); MAX_CLI_ARGS];
        assert_eq!(
            bounded_args(valid.into_iter().map(std::ffi::OsString::from))
                .unwrap()
                .len(),
            MAX_CLI_ARGS
        );
        let too_many = vec!["x".to_owned(); MAX_CLI_ARGS + 1];
        assert!(bounded_args(too_many.into_iter().map(std::ffi::OsString::from)).is_err());
        let too_long = std::ffi::OsString::from("x".repeat(MAX_CLI_ARG_BYTES + 1));
        assert!(bounded_args([too_long]).is_err());
        let too_many_bytes =
            vec!["x".repeat(MAX_CLI_ARG_BYTES); MAX_CLI_TOTAL_BYTES / MAX_CLI_ARG_BYTES + 1];
        assert!(bounded_args(too_many_bytes.into_iter().map(std::ffi::OsString::from)).is_err());
    }

    // REQ: OA-G3-006, OA-G3-007
    #[test]
    fn train_and_check_report_writers_refuse_before_exceeding_their_output_limit() {
        let trace = trace(Vec::new());
        let policy = policy_digest();
        let train_error =
            train_report_with_limit(&trace, "trainhash", "binaryhash", &policy, 128).unwrap_err();
        assert!(train_error.contains("report output cap exceeded"));

        let model = train_report(&trace, "trainhash", "binaryhash", &policy).unwrap();
        let check_error = check_report_with_limit(
            &trace,
            "checkhash",
            "binaryhash",
            &policy,
            "modelhash",
            &model,
            128,
        )
        .unwrap_err();
        assert!(check_error.contains("report output cap exceeded"));
    }

    fn row(gufi: u128, sector: [u8; 8], pos_ms: i64) -> Row {
        Row {
            source_id: (1, gufi as u64, 1, 0),
            record_sha256: [0; 32],
            gufi,
            key: CellKey {
                lat: 840,
                lon: -1767,
                band: 16,
                sector,
            },
            position_ms: pos_ms,
            received_ms: pos_ms + 1_000,
        }
    }
    fn trace(rows: Vec<Row>) -> Trace {
        let mut t = Trace {
            producer_commit: "unknown".to_owned(),
            producer_dirty: "unknown".to_owned(),
            row_records: rows.len() as u64,
            ..Trace::default()
        };
        t.rows = rows;
        t
    }
    fn row_line(gufi: u128, owner: &str, centre: &str, offset: u64) -> String {
        let present = [
            oa_fixm::Field::Centre,
            oa_fixm::Field::ControllingUnit,
            oa_fixm::Field::Position,
            oa_fixm::Field::PositionTime,
            oa_fixm::Field::ReportedAlt,
        ]
        .iter()
        .fold(0u64, |a, f| a | (1u64 << (*f as u8)));
        format!("ROW\tstream=1\toffset={offset}\tseq=1\trow=0\tsource_ms=1794700800000\treceived_ms=1794700801000\tgufi={gufi:032x}\tpresent={present}\terrors=0\tops=0\tcentre={centre}\treported_owner={owner}\tposition_ms=1794700800000\tlat_e7=420000000\tlon_e7=-883500000\treported_alt_ft=30000\tno_status=0\towner_unknown=0\thandoff_incomplete=0\thandoff_unknown_unit=0\thandoff_receiving_only=0\tdeparture_not_active=0")
    }
    fn with_header(body: &str) -> String {
        let records = body
            .lines()
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>();
        let mut frames = std::collections::BTreeMap::new();
        let mut row_count = 0u64;
        let mut op_count = 0u64;
        let mut frame_count = 0u64;
        let mut zero_op_rows = 0u64;
        let mut missing_key_or_time = 0u64;
        for line in &records {
            match line.split('\t').next().unwrap_or("") {
                "ROW" => {
                    row_count += 1;
                    let mut fields = std::collections::BTreeMap::new();
                    for field in line.split('\t').skip(1) {
                        if let Some((key, value)) = field.split_once('=') {
                            fields.insert(key, value);
                        }
                    }
                    zero_op_rows += u64::from(fields.get("ops") == Some(&"0"));
                    missing_key_or_time += u64::from(
                        fields.get("gufi") == Some(&"none")
                            || fields.get("source_ms").is_none_or(|value| *value == "none"),
                    );
                }
                "OP" => op_count += 1,
                _ => {}
            }
            if matches!(
                line.split('\t').next().unwrap_or(""),
                "ROW" | "OP" | "FRAME"
            ) {
                let mut identity = std::collections::BTreeMap::new();
                for field in line.split('\t').skip(1) {
                    if let Some((k, v)) = field.split_once('=') {
                        identity.insert(k, v);
                    }
                }
                if let (Some(stream), Some(offset), Some(seq)) = (
                    identity.get("stream"),
                    identity.get("offset"),
                    identity.get("seq"),
                ) {
                    if frames
                        .insert(
                            ((*stream).to_owned(), (*offset).to_owned()),
                            (*seq).to_owned(),
                        )
                        .is_none()
                    {
                        frame_count += 1;
                    }
                }
            }
        }
        let mut out = format!("{H2_HEADER}\nCONFIG\tfrom_ns=1794700800000000000\tto_ns_inclusive=1794787199999999999\tcapacity=1\tlag_floor_ms=0\tmax_future_ms=0\trefusal_log=0\tconflict_log=0\tremoval_log=0\tsweep_every_ms=1\tretain_ms=1\tsilence_ms=1\nBUILD\tcommit=unknown\tdirty=unknown\tbinary_identity=outer-build-manifest-required\n");
        for ((stream, offset), seq) in frames {
            out.push_str(&format!("FRAME\tstream={stream}\toffset={offset}\tseq={seq}\trow=0\treceived_ns=1794700801000000000\tdisposition=0\treason=0\n"));
        }
        for line in records {
            if line.starts_with("FRAME\t") {
                continue;
            }
            out.push_str(line);
            out.push('\n');
        }
        let events = row_count + op_count + frame_count;
        out.push_str(&format!("RESULT\tstatus=measurement-complete\tqualification=not-assessed\tframes={frame_count}\tdecoded_rows={row_count}\tdocs_unparsed=0\tdocs_refused=0\treader_loss=0\nCOUNTS\tframes_visited={frame_count}\trows_delivered={row_count}\tevents_delivered={events}\tmissing_key_or_time={missing_key_or_time}\tzero_op_rows={zero_op_rows}\tcap_refused=0\tsink_failed=0\tsealed_drops=0\tmerge_files=0\tmerge_bytes=0\treader_steps=0\tdecoded_bytes_charged=0\traw_input_bytes_reserved=0\traw_input_cap_refused=0\n"));
        for reason in 0..H2_READER_ERROR_COUNT {
            out.push_str(&format!("READER_ERROR\treason={reason}\tcount=0\n"));
        }
        for kind in 0..oa_flight::OP_COUNT {
            out.push_str(&format!("OP_COUNTS\tkind={kind}\tapplied=0\tunchanged=0\n"));
            for reason in 0..oa_flight::R_COUNT {
                out.push_str(&format!(
                    "OP_REFUSAL\tkind={kind}\treason={reason}\tcount=0\n"
                ));
            }
        }
        out.push_str(&format!("INPUT\tindex=0\tpath_hex=2f746d702f73796e746865746963\topened=1\teof=1\tbytes=0\treserved_bytes=0\tsha256={}\n", "0".repeat(64)));
        out
    }
    fn policy_digest() -> String {
        oa_sha256::to_hex(&oa_sha256::sha256(b"oa-sector-reconstruct-v2;grid=500000e7;alt=2000ft;state-bin=900000ms;min-flights=3;min-bins=2;center=ZAU-zero-pad;frame-row-op-identity-joins=required;future-cross-clock-observations=excluded;training=2026-11-15;check=2026-11-19"))
    }

    // REQ: OA-G3-004, OA-G3-005
    #[test]
    fn sparse_cells_abstain_and_reported_support_is_counted_without_probability_claim() {
        let sparse = trace(vec![row(1, *b"UNKTAAA ", 1)]);
        let report = train_report(&sparse, "testhash", "binaryhash", &policy_digest()).unwrap();
        assert!(report.contains("support_is_not_probability=true"));
        assert!(report.contains("\tunknown\n"));
        let supported = trace(vec![
            row(1, *b"SECTAAA ", 1),
            row(2, *b"SECTAAA ", 901_000),
            row(3, *b"SECTAAA ", 1_801_000),
        ]);
        let report = train_report(&supported, "testhash", "binaryhash", &policy_digest()).unwrap();
        assert!(report.contains("\tsupported\n"));
    }

    // REQ: OA-G3-005, OA-G3-007
    #[test]
    fn competing_supported_owners_are_contested_and_check_requires_exact_label() {
        let mut rows = Vec::new();
        for (i, time) in [(1, 1), (2, 901_000), (3, 1_801_000)] {
            rows.push(row(i, *b"SECTAAA ", time));
        }
        for (i, time) in [(4, 1), (5, 901_000), (6, 1_801_000)] {
            rows.push(row(i, *b"SECTBBB ", time));
        }
        let report =
            train_report(&trace(rows), "trainhash", "binaryhash", &policy_digest()).unwrap();
        assert_eq!(report.matches("\tcontested\n").count(), 2);
        let check = trace(vec![
            row(7, *b"SECTCCC ", 1),
            row(8, *b"SECTCCC ", 901_000),
            row(9, *b"SECTCCC ", 1_801_000),
        ]);
        let result = check_report(
            &check,
            "checkhash",
            "binaryhash",
            &policy_digest(),
            "modelhash",
            &report,
        )
        .unwrap();
        assert!(result.contains("check_rows_without_supported_training_label=3"));
    }

    // REQ: OA-G3-006
    #[test]
    fn same_flight_state_time_with_conflicting_owner_is_excluded_and_counted() {
        let input = with_header(&format!(
            "{}\n{}",
            row_line(1, "5345435441414141", "5a415500", 1),
            row_line(1, "5345435442424242", "5a415500", 2)
        ));
        let t = parse_trace(&input, "2026-11-15").unwrap();
        assert!(t.rows.is_empty());
        assert_eq!(t.owner_conflict_rows, 2);
        assert!(train_report(&t, "hash", "binaryhash", &policy_digest())
            .unwrap()
            .contains("owner_conflict_rows=2"));
    }

    // REQ: OA-G3-003, OA-G3-004
    #[test]
    fn source_row_uses_positive_zau_identity_and_refuses_other_centres() {
        let valid = parse_trace(
            &with_header(&row_line(1, "5345435441414141", "5a415500", 1)),
            "2026-11-15",
        )
        .unwrap();
        assert_eq!(valid.rows.len(), 1);
        let other = parse_trace(
            &with_header(&row_line(1, "5345435441414141", "5a4f4200", 1)),
            "2026-11-15",
        )
        .unwrap();
        assert!(other.rows.is_empty());
        assert_eq!(other.unknown_identity, 1);
    }

    // REQ: OA-G3-006
    #[test]
    fn cross_clock_future_rows_abstain_without_claiming_clock_causality() {
        let source_future = row_line(1, "5345435441414141", "5a415500", 1)
            .replace("source_ms=1794700800000", "source_ms=1794700802000");
        let state_future = row_line(2, "5345435441414141", "5a415500", 2)
            .replace("position_ms=1794700800000", "position_ms=1794700802000");
        let parsed = parse_trace(
            &with_header(&format!("{source_future}\n{state_future}")),
            "2026-11-15",
        )
        .unwrap();
        assert!(parsed.rows.is_empty());
        assert_eq!(parsed.clock_future_excluded_rows, 2);
        assert_eq!(parsed.source_clock_future_rows, 1);
        assert_eq!(parsed.state_clock_future_rows, 1);
    }

    // REQ: OA-G3-001, OA-G3-006
    #[test]
    fn operation_must_join_to_a_row_and_have_a_valid_per_row_index() {
        let orphan = "OP\tstream=1\toffset=1\tseq=1\trow=0\tindex=0\tkind=1\tgufi=00000000000000000000000000000001\tcentre=5a415500\tsource_ms=1794700800000\treceived_ms=1794700801000\tmessage=1\tstate=0\talt=0,0,0\tfix=000000000000000000000000\tunit_a=0000000000000000\tunit_b=0000000000000000\tplan_id=000000000000000000000000\taddress=0\tcallsign=0000000000000000\tdeparture=000000000000000000000000\tarrival=000000000000000000000000\toutcome=0\treason=0\tflight=absent";
        let trace = with_header(orphan);
        assert!(parse_trace(&trace, "2026-11-15").is_err());
    }

    #[cfg(unix)]
    // REQ: OA-G3-007
    #[test]
    fn explicit_private_temp_root_does_not_depend_on_folder_name() {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let guard = oa_sealed::Sealed::production();
        let root = std::env::temp_dir().join(format!("oa-g3-private-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let input = root.join("trace.tsv");
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&input)
            .unwrap();
        assert!(private_tree(guard, &root, &input).unwrap());
        assert!(!private_tree(guard, &root, &std::env::temp_dir()).unwrap());
        std::fs::remove_dir_all(&root).unwrap();
    }

    // REQ: OA-G3-002
    #[test]
    fn partition_boundaries_are_half_open_and_only_frozen_dates_are_supported() {
        let (start, end) = partition_bounds("2026-11-15").unwrap();
        assert_eq!(end - start, 86_400_000);
        assert_eq!(
            partition_bounds("2026-11-19").unwrap().0 - start,
            4 * 86_400_000
        );
        assert!(partition_bounds("2026-11-16").is_err());
    }

    // REQ: OA-G3-001
    #[test]
    fn h2_header_pins_unobservable_independent_agreement_and_rejects_other_versions() {
        assert!(parse_trace(&with_header(""), "2026-11-15").is_ok());
        assert!(parse_trace(
            "H2-OBSERVED-STATE-1\tindependent_agreement_denominator=not-observable\n",
            "2026-11-15"
        )
        .is_err());
    }

    // REQ: OA-G3-001, OA-G3-006
    #[test]
    fn eof_input_receipt_must_consume_its_reserved_descriptor_length() {
        let valid = with_header("");
        let inconsistent = valid
            .replace("raw_input_bytes_reserved=0", "raw_input_bytes_reserved=2")
            .replace("bytes=0\treserved_bytes=0", "bytes=1\treserved_bytes=2");
        assert!(parse_trace(&inconsistent, "2026-11-15").is_err());
    }

    // REQ: OA-G3-001, OA-G3-007
    #[test]
    fn selected_window_report_preserves_build_and_input_receipt_scope() {
        let input = with_header("")
            .replace("raw_input_bytes_reserved=0", "raw_input_bytes_reserved=5")
            .replace(
                "eof=1\tbytes=0\treserved_bytes=0",
                "eof=0\tbytes=0\treserved_bytes=5",
            );
        let parsed = parse_trace(&input, "2026-11-15").unwrap();
        let report = train_report(&parsed, "tracehash", "binaryhash", &policy_digest()).unwrap();
        for expected in [
            "complete=true\ncompletion_scope=selected-window\nproducer_commit=unknown\nproducer_dirty=unknown\n",
            "input_receipts=1\ninput_eof_receipts=0\ninput_non_eof_receipts=1\ninput_bytes_read=0\ninput_bytes_reserved=5\n",
        ] {
            assert!(report.contains(expected), "missing report fields: {expected}");
        }
    }

    // REQ: OA-G3-001
    #[test]
    fn complete_trace_refuses_unopened_input_receipt() {
        let unopened = with_header("").replace("opened=1", "opened=0");
        let error = parse_trace(&unopened, "2026-11-15")
            .err()
            .expect("measurement-complete cannot contain an unopened input");
        assert!(error.contains("unopened input"));
    }

    // REQ: OA-G3-001, OA-G3-006
    #[test]
    fn result_terminal_counts_must_match_observed_frame_and_row_records() {
        let valid = with_header(&row_line(1, "5345435441414141", "5a415500", 1));
        assert!(parse_trace(&valid, "2026-11-15").is_ok());
        for (field, value) in [
            ("frames=1", "frames=2"),
            ("decoded_rows=1", "decoded_rows=2"),
            ("docs_unparsed=0", "docs_unparsed=1"),
            ("docs_refused=0", "docs_refused=1"),
        ] {
            let inconsistent = valid.replacen(field, value, 1);
            assert!(
                parse_trace(&inconsistent, "2026-11-15").is_err(),
                "accepted inconsistent {field}"
            );
        }
    }

    #[test]
    fn counted_xml_refusal_is_a_complete_partial_result_when_terminal_counts_match() {
        let mut trace = with_header("FRAME\tstream=1\toffset=1\tseq=1\trow=0\treceived_ns=1794700801000000000\tdisposition=0\treason=0");
        trace = trace
            .replace("disposition=0", "disposition=4")
            .replace("docs_unparsed=0", "docs_unparsed=1");
        let parsed = parse_trace(&trace, "2026-11-15")
            .expect("counted XML refusal is visible partial input");
        assert_eq!(parsed.frame_dispositions.get(4).copied(), Some(1));
        assert!(parsed.rows.is_empty());
    }

    // REQ: OA-G3-001, OA-G3-006
    #[test]
    fn exact_receipt_repeats_deduplicate_and_conflicting_receipts_are_refused_with_conservation() {
        let one = row_line(1, "5345435441414141", "5a415500", 1);
        let repeated = parse_trace(&with_header(&format!("{one}\n{one}")), "2026-11-15").unwrap();
        assert_eq!(repeated.rows.len(), 1);
        assert_eq!(repeated.duplicate_rows, 1);
        let conflict = row_line(1, "5345435442424242", "5a415500", 1);
        let contradictory =
            parse_trace(&with_header(&format!("{one}\n{conflict}")), "2026-11-15").unwrap();
        assert_eq!(contradictory.rows.len(), 0);
        assert_eq!(contradictory.source_conflict_rows, 2);
    }

    // REQ: OA-G3-002
    #[test]
    fn complete_time_prescan_refuses_sealed_and_wrong_partition_timestamps() {
        let guard = oa_sealed::Sealed::production();
        let sealed = format!("{H2_HEADER}\nFRAME\tstream=1\toffset=1\tseq=1\trow=0\treceived_ns=1795219200000000000\tdisposition=0\treason=0\n");
        assert!(prescan_times(&sealed, guard, "2026-11-19").is_err());
        let next_day = format!("{H2_HEADER}\nFRAME\tstream=1\toffset=1\tseq=1\trow=0\treceived_ns=1794787200000000000\tdisposition=0\treason=0\n");
        assert!(prescan_times(&next_day, guard, "2026-11-15").is_err());
    }
}
