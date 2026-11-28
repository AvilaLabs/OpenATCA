#![forbid(unsafe_code)]
//! Bounded recorded OAF to OA-RS importer and create-new evidence publisher (shell, B1/B3/C6).

use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use oa_regional_session::recordings_cli::{
    convert_recorded_hour_window_scoped, convert_recordings, validate_recorded_hour_window,
    WindowRecordingCliOutput, MAX_WINDOW_OUTPUT_BYTES,
};
use oa_regional_session::recordings_reader::{
    RecordingInput, RecordingSource, MAX_RECORDING_FRAMES, MAX_RECORDING_INPUT_BYTES,
    MAX_RECORDING_STREAMS,
};
use oa_regional_session::recordings_stream::{
    NormalizedStreamSummary, MAX_H2_EXPANDED_INPUT_BYTES, MAX_H2_FRAME_WORK_UNITS,
    MAX_H2_INPUT_FRAMES, MAX_H2_INPUT_STREAMS, MAX_H2_PHYSICAL_INPUT_BYTES,
};
use oa_regional_session::session::{MAX_EVENTS_PER_RECEIPT, MAX_SESSION_EVENTS};
use oa_regional_session::surveillance::{MAX_FRAMES, MAX_FRAME_GAP_NS};

const MAX_NORMALIZED_BYTES: usize = 16 * 1024 * 1024;
const MAX_PROVENANCE_BYTES: usize = 32 * 1024 * 1024;
const MAX_QUARANTINE_BYTES: usize = 64 * 1024 * 1024;
const MAX_REPORT_BYTES: usize = 64 * 1024;
const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
const MAX_EXECUTABLE_BYTES: u64 = 128 * 1024 * 1024;
const HASH_CHUNK_BYTES: usize = 64 * 1024;

// A7.2: cold shell command-line values, with at most thirty-two source paths.
struct Arguments {
    inputs: Vec<RecordingInput>,
    centre: Option<[u8; 4]>,
    output: Option<PathBuf>,
    window: Option<(i64, i64)>,
    flight_centre_scope: Option<[u8; 4]>,
}

fn parse_epoch_ns(value: &std::ffi::OsStr, label: &str) -> Result<i64, String> {
    let text = value
        .to_str()
        .ok_or_else(|| format!("{label} must be ASCII decimal nanoseconds"))?;
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(format!("{label} must be ASCII decimal nanoseconds"));
    }
    text.parse::<i64>()
        .map_err(|_| format!("{label} exceeds signed nanosecond range"))
}

fn parse_centre(value: &std::ffi::OsStr, label: &str) -> Result<[u8; 4], String> {
    let text = value
        .to_str()
        .ok_or_else(|| format!("{label} must be UTF-8"))?;
    let raw = text.as_bytes();
    if raw.is_empty() || raw.len() > 4 {
        return Err(format!(
            "{label} must be one to four ASCII alphanumeric bytes"
        ));
    }
    let mut code = [0u8; 4];
    code.iter_mut()
        .zip(raw)
        .for_each(|(target, source)| *target = *source);
    if !oa_regional_session::input::is_valid_centre_code(&code) {
        return Err(format!("{label} code has invalid padding"));
    }
    Ok(code)
}

fn centre_text(code: &[u8; 4]) -> Result<String, String> {
    if !oa_regional_session::input::is_valid_centre_code(code) {
        return Err("centre code has invalid padding".into());
    }
    let length = code
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(code.len());
    core::str::from_utf8(code.get(..length).ok_or("centre code bounds")?)
        .map(str::to_owned)
        .map_err(|_| "centre code encoding".into())
}

fn validate_window(start: i64, end: i64) -> Result<(i64, i64), String> {
    validate_recorded_hour_window(start, end)
}

fn parse_arguments() -> Result<Arguments, String> {
    let mut args = env::args_os().skip(1);
    let mut inputs = Vec::new();
    inputs
        .try_reserve_exact(MAX_H2_INPUT_STREAMS)
        .map_err(|_| "input list allocation refused")?;
    let mut centre = None;
    let mut output = None;
    let mut start = None;
    let mut end = None;
    let mut flight_centre_scope = None;
    while let Some(flag) = args.next() {
        match flag.to_str() {
            Some("--adsb") | Some("--sfdps") => {
                let source = match flag.to_str() { Some("--adsb") => RecordingSource::Adsb, _ => RecordingSource::Sfdps };
                let path = args.next().map(PathBuf::from).ok_or("missing path after source option")?;
                if inputs.len() >= MAX_H2_INPUT_STREAMS { return Err("source stream limit exceeded".into()); }
                inputs.push(RecordingInput { source, path });
            }
            Some("--centre") if centre.is_none() => {
                centre = Some(parse_centre(
                    &args.next().ok_or("missing value after --centre")?,
                    "centre",
                )?);
            }
            Some("--flight-centre") if flight_centre_scope.is_none() => {
                flight_centre_scope = Some(parse_centre(
                    &args.next().ok_or("missing value after --flight-centre")?,
                    "flight centre",
                )?);
            }
            Some("--start") if start.is_none() => {
                start = Some(parse_epoch_ns(
                    &args.next().ok_or("missing value after --start")?,
                    "--start",
                )?);
            }
            Some("--end") if end.is_none() => {
                end = Some(parse_epoch_ns(
                    &args.next().ok_or("missing value after --end")?,
                    "--end",
                )?);
            }
            Some("--out") if output.is_none() => output = Some(PathBuf::from(args.next().ok_or("missing path after --out")?)),
            _ => return Err("usage: regional-recording-import (--adsb FILE | --sfdps FILE)+ [--start EPOCH_NS --end EPOCH_NS] --centre ARTCC [--flight-centre ARTCC] --out NEW_DIR".into()),
        }
    }
    if inputs.is_empty() {
        return Err("at least one --adsb or --sfdps input is required".into());
    }
    let window = match (start, end) {
        (Some(start), Some(end)) => Some(validate_window(start, end)?),
        (None, None) => None,
        _ => return Err("--start and --end must be provided together".into()),
    };
    let stream_limit = if window.is_some() {
        MAX_H2_INPUT_STREAMS
    } else {
        MAX_RECORDING_STREAMS
    };
    if inputs.len() > stream_limit {
        return Err("source stream limit exceeded".into());
    }
    if window.is_none() && flight_centre_scope.is_some() {
        return Err("--flight-centre requires an explicit --start/--end window".into());
    }
    Ok(Arguments {
        inputs,
        centre: Some(centre.ok_or("--centre is required")?),
        output: Some(output.ok_or("--out is required")?),
        window,
        flight_centre_scope,
    })
}

fn digest_file(mut file: File, max_bytes: u64) -> Result<([u8; 32], u64), String> {
    let mut hash = oa_sha256::Sha256::new();
    let mut buffer = [0u8; HASH_CHUNK_BYTES];
    let mut total = 0u64;
    loop {
        let read = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(u64::try_from(read).map_err(|_| "read length overflow")?)
            .ok_or("read length overflow")?;
        if total > max_bytes {
            return Err("file grew beyond its byte limit".into());
        }
        hash.update(buffer.get(..read).ok_or("hash chunk bounds")?)
            .map_err(|error| error.to_string())?;
    }
    Ok((hash.finalize(), total))
}

fn executable_digest() -> Result<[u8; 32], String> {
    let path = env::current_exe().map_err(|error| error.to_string())?;
    let (file, len) =
        oa_sealed::open_regular(&path).map_err(|error| format!("guarded executable: {error}"))?;
    if len > MAX_EXECUTABLE_BYTES {
        return Err("executable exceeds byte limit".into());
    }
    let (digest, read) = digest_file(file, MAX_EXECUTABLE_BYTES)?;
    if read != len {
        return Err("executable changed while hashing".into());
    }
    Ok(digest)
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    oa_sealed::refuse(path).map_err(|error| format!("guarded output: {error}"))?;
    // seal-allow: create-new evidence output after guard refusal; it is never used as input.
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|error| error.to_string())?;
    oa_sealed::Sealed::production()
        .verify_open(path, &file)
        .map_err(|error| error.to_string())?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|error| error.to_string())
}

fn verify_file(path: &Path, expected_len: usize, expected_hash: &[u8; 32]) -> Result<(), String> {
    let (file, opened_len) = oa_sealed::open_regular(path)
        .map_err(|error| format!("verify-open {}: {error}", path.display()))?;
    let expected_len = u64::try_from(expected_len).map_err(|_| "expected length overflow")?;
    if opened_len != expected_len {
        return Err(format!(
            "verification length mismatch for {}",
            path.display()
        ));
    }
    let (actual_hash, read_len) = digest_file(file, expected_len)?;
    if read_len != expected_len || &actual_hash != expected_hash {
        return Err(format!("verification hash mismatch for {}", path.display()));
    }
    Ok(())
}

fn append_stream_metadata(
    output: &mut String,
    summary: &NormalizedStreamSummary,
    row_tag: &str,
) -> Result<(), String> {
    use std::fmt::Write as FmtWrite;
    for stream in &summary.recording.streams {
        let source = match stream.source {
            RecordingSource::Adsb => "ADSB",
            RecordingSource::Sfdps => "SFDPS",
        };
        writeln!(
            output,
            "{row_tag}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            stream.stream_index,
            source,
            oa_sha256::to_hex(&stream.sha256),
            stream.bytes,
            stream.expanded_bytes,
            u8::from(stream.gzip),
            stream.messages,
            stream
                .first_frame_receive_ns
                .map_or_else(|| "-".to_owned(), |v| v.to_string()),
            stream
                .last_frame_receive_ns
                .map_or_else(|| "-".to_owned(), |v| v.to_string()),
            stream.max_frame_gap_ns
        )
        .map_err(|_| "stream metadata formatting failed")?;
    }
    Ok(())
}

fn append_window_source_report(
    report: &mut String,
    label: &str,
    source: &oa_regional_session::recordings_stream::NormalizedInputCounts,
) -> Result<(), String> {
    use std::fmt::Write as FmtWrite;
    writeln!(report, "{label}_message_frames\t{}", source.message_frames)
        .map_err(|_| "hour report formatting failed")?;
    writeln!(
        report,
        "{label}_candidate_events_before_receipt_filter\t{}",
        source.candidate_events_before_receipt_filter
    )
    .map_err(|_| "hour report formatting failed")?;
    writeln!(
        report,
        "{label}_max_candidate_events_per_frame\t{}",
        source.max_candidate_events_per_frame
    )
    .map_err(|_| "hour report formatting failed")?;
    writeln!(
        report,
        "{label}_receipt_groups_over_capacity\t{}",
        source.receipt_groups_over_capacity
    )
    .map_err(|_| "hour report formatting failed")?;
    writeln!(
        report,
        "{label}_receipt_group_events_refused\t{}",
        source.receipt_group_events_refused
    )
    .map_err(|_| "hour report formatting failed")?;
    writeln!(
        report,
        "{label}_quarantine_rows\t{}",
        source.quarantine_rows
    )
    .map_err(|_| "hour report formatting failed")?;
    for (index, count) in source.adsb_quality_removed.iter().enumerate() {
        writeln!(report, "{label}_adsb_quality_removed_{index}\t{count}")
            .map_err(|_| "hour report formatting failed")?;
    }
    for (name, count) in oa_flight::OP_NAMES
        .iter()
        .zip(source.sfdps_emitted_operations)
    {
        writeln!(report, "{label}_sfdps_emitted_operation_{name}\t{count}")
            .map_err(|_| "hour report formatting failed")?;
    }
    for (name, count) in oa_flight::OP_NAMES
        .iter()
        .zip(source.sfdps_unsupported_operations)
    {
        writeln!(
            report,
            "{label}_sfdps_unsupported_operation_{name}\t{count}"
        )
        .map_err(|_| "hour report formatting failed")?;
    }
    for (field, value) in [
        ("adsb_rows_in", source.adsb_rows_in),
        ("adsb_rows_dropped", source.adsb_rows_dropped),
        ("adsb_ground_excluded", source.adsb_ground_excluded),
        (
            "adsb_unknown_ground_refused",
            source.adsb_unknown_ground_refused,
        ),
        (
            "adsb_source_time_regressions",
            source.adsb_source_time_regressions,
        ),
        ("adsb_rows_order_refused", source.adsb_rows_order_refused),
        (
            "adsb_velocity_unavailable",
            source.adsb_velocity_unavailable,
        ),
        (
            "adsb_unknown_gufi_positions",
            source.adsb_unknown_gufi_positions,
        ),
        ("invalid_position_rows", source.invalid_position_rows),
        ("source_after_receipt", source.source_after_receipt),
        ("sfdps_rows_decoded", source.sfdps_rows_decoded),
        ("sfdps_rows_refused", source.sfdps_rows_refused),
        ("sfdps_rows_no_operations", source.sfdps_rows_no_operations),
        ("sfdps_owner_unknown", source.sfdps_owner_unknown),
        (
            "sfdps_rows_positive_address",
            source.sfdps_rows_positive_address,
        ),
        (
            "sfdps_rows_nonempty_callsign",
            source.sfdps_rows_nonempty_callsign,
        ),
        (
            "sfdps_operations_unusable",
            source.sfdps_operations_unusable,
        ),
        ("invalid_operation_time", source.invalid_operation_time),
        (
            "invalid_operation_identity",
            source.invalid_operation_identity,
        ),
        ("invalid_operation_centre", source.invalid_operation_centre),
        (
            "invalid_operation_address",
            source.invalid_operation_address,
        ),
    ] {
        writeln!(report, "{label}_{field}\t{value}")
            .map_err(|_| "hour report formatting failed")?;
    }
    for (reason_index, count) in source.quarantine_reason_counts.iter().enumerate() {
        writeln!(report, "{label}_quarantine_reason_{reason_index}\t{count}")
            .map_err(|_| "hour report formatting failed")?;
    }
    Ok(())
}

fn serialize_window_report(output: &WindowRecordingCliOutput) -> Result<String, String> {
    use std::fmt::Write as FmtWrite;
    let summary = &output.summary;
    let counts = summary.recording.counts;
    let mut report = String::from("OA_RECORDING_HOUR_COUNTS\t1\n");
    writeln!(report, "window_start_ns\t{}", output.window_ns.0)
        .map_err(|_| "hour report formatting failed")?;
    writeln!(report, "window_end_ns\t{}", output.window_ns.1)
        .map_err(|_| "hour report formatting failed")?;
    writeln!(
        report,
        "flight_scope_mode\t{}",
        if output.flight_centre_scope.is_some() {
            "CENTRE_LOCAL_SFDPS"
        } else {
            "ALL_CENTRES"
        }
    )
    .map_err(|_| "hour report formatting failed")?;
    writeln!(
        report,
        "flight_scope_label\t{}",
        if output.flight_centre_scope.is_some() {
            "CENTRE_LOCAL_SFDPS_REPLAY_OTHER_CENTRE_COPIES_EXCLUDED"
        } else {
            "ALL_CENTRES_SFDPS_REPLAY"
        }
    )
    .map_err(|_| "hour report formatting failed")?;
    let flight_scope_centre = output
        .flight_centre_scope
        .map(|code| centre_text(&code))
        .transpose()?;
    writeln!(
        report,
        "flight_centre_scope\t{}",
        flight_scope_centre.as_deref().unwrap_or("-")
    )
    .map_err(|_| "hour report formatting failed")?;
    writeln!(
        report,
        "stream_normalized_events\t{}",
        summary.normalized_events
    )
    .map_err(|_| "hour report formatting failed")?;
    writeln!(
        report,
        "excluded_other_centre_events\t{}",
        output.excluded_other_centre_events
    )
    .map_err(|_| "hour report formatting failed")?;
    writeln!(
        report,
        "normalized_events\t{}",
        output.selected_normalized_events
    )
    .map_err(|_| "hour report formatting failed")?;
    writeln!(report, "quarantine_rows\t{}", summary.quarantine_rows)
        .map_err(|_| "hour report formatting failed")?;
    writeln!(
        report,
        "manual_handling_required\t{}",
        summary.manual_handling_required
    )
    .map_err(|_| "hour report formatting failed")?;
    writeln!(report, "unprocessed_frames\t{}", summary.unprocessed_frames)
        .map_err(|_| "hour report formatting failed")?;
    for (label, source) in [
        ("adsb", &summary.source_counts[0]),
        ("sfdps", &summary.source_counts[1]),
    ] {
        append_window_source_report(&mut report, label, source)?;
    }
    writeln!(report, "frames\t{}", counts.frames).map_err(|_| "hour report formatting failed")?;
    writeln!(report, "messages\t{}", counts.messages)
        .map_err(|_| "hour report formatting failed")?;
    writeln!(report, "status_frames\t{}", counts.status_frames)
        .map_err(|_| "hour report formatting failed")?;
    writeln!(report, "reject_frames\t{}", counts.reject_frames)
        .map_err(|_| "hour report formatting failed")?;
    writeln!(
        report,
        "outside_window_frames\t{}",
        counts.outside_window_frames
    )
    .map_err(|_| "hour report formatting failed")?;
    writeln!(
        report,
        "outside_window_messages\t{}",
        counts.outside_window_messages
    )
    .map_err(|_| "hour report formatting failed")?;
    writeln!(
        report,
        "outside_window_status_frames\t{}",
        counts.outside_window_status_frames
    )
    .map_err(|_| "hour report formatting failed")?;
    writeln!(
        report,
        "outside_window_reject_frames\t{}",
        counts.outside_window_reject_frames
    )
    .map_err(|_| "hour report formatting failed")?;
    writeln!(report, "physical_bytes\t{}", counts.physical_bytes)
        .map_err(|_| "hour report formatting failed")?;
    writeln!(report, "expanded_bytes\t{}", counts.expanded_bytes)
        .map_err(|_| "hour report formatting failed")?;
    writeln!(report, "frame_work_units\t{}", counts.frame_work_units)
        .map_err(|_| "hour report formatting failed")?;
    append_stream_metadata(&mut report, summary, "stream")?;
    if report.len() > MAX_REPORT_BYTES {
        return Err("hour report exceeds the 64 KiB limit".into());
    }
    Ok(report)
}

fn serialize_window_quarantine(output: &WindowRecordingCliOutput) -> String {
    format!(
        "OA-RS-QUARANTINE-AGGREGATE\t1\nwindow_start_ns\t{}\nwindow_end_ns\t{}\nquarantine_rows\t{}\nmanual_handling_required\t{}\nunprocessed_frames\t{}\n",
        output.window_ns.0,
        output.window_ns.1,
        output.summary.quarantine_rows,
        output.summary.manual_handling_required,
        output.summary.unprocessed_frames
    )
}

struct WindowManifestEvidence {
    executable_sha: [u8; 32],
    normalized_hash: [u8; 32],
    provenance_hash: [u8; 32],
    quarantine_hash: [u8; 32],
    report_hash: [u8; 32],
    report_len: usize,
    quarantine_len: usize,
}
// A7.2: cold manifest input that binds the already-serialized output bytes.

fn serialize_window_manifest(
    centre: [u8; 4],
    output: &WindowRecordingCliOutput,
    evidence: WindowManifestEvidence,
) -> Result<String, String> {
    let mut manifest = format!(
        "OA_RECORDING_HOUR_MANIFEST\t1\nmode\tRECEIVE_WINDOW_STREAMED\ncentre\t{}\nflight_scope_mode\t{}\nflight_scope_label\t{}\nflight_centre_scope\t{}\nstream_normalized_events\t{}\nselected_normalized_events\t{}\nexcluded_other_centre_events\t{}\nwindow_start_ns\t{}\nwindow_end_ns\t{}\nnormalized_bytes\t{}\nnormalized_sha256\t{}\nprovenance_bytes\t{}\nprovenance_sha256\t{}\nquarantine_bytes\t{}\nquarantine_sha256\t{}\nreport_bytes\t{}\nreport_sha256\t{}\nexecutable_sha256\t{}\nphysical_input_cap_bytes\t{}\nexpanded_input_cap_bytes\t{}\nframe_cap\t{}\nstream_cap\t{}\nframe_work_cap\t{}\nnormalized_output_cap_bytes\t{}\nprovenance_output_cap_bytes\t{}\n",
        core::str::from_utf8(&centre).map_err(|_| "centre encoding")?,
        if output.flight_centre_scope.is_some() {
            "CENTRE_LOCAL_SFDPS"
        } else {
            "ALL_CENTRES"
        },
        if output.flight_centre_scope.is_some() {
            "CENTRE_LOCAL_SFDPS_REPLAY_OTHER_CENTRE_COPIES_EXCLUDED"
        } else {
            "ALL_CENTRES_SFDPS_REPLAY"
        },
        output
            .flight_centre_scope
            .map(|code| centre_text(&code))
            .transpose()?
            .unwrap_or_else(|| "-".to_owned()),
        output.summary.normalized_events,
        output.selected_normalized_events,
        output.excluded_other_centre_events,
        output.window_ns.0,
        output.window_ns.1,
        output.normalized_tsv.len(),
        oa_sha256::to_hex(&evidence.normalized_hash),
        output.provenance_tsv.len(),
        oa_sha256::to_hex(&evidence.provenance_hash),
        evidence.quarantine_len,
        oa_sha256::to_hex(&evidence.quarantine_hash),
        evidence.report_len,
        oa_sha256::to_hex(&evidence.report_hash),
        oa_sha256::to_hex(&evidence.executable_sha),
        MAX_H2_PHYSICAL_INPUT_BYTES,
        MAX_H2_EXPANDED_INPUT_BYTES,
        MAX_H2_INPUT_FRAMES,
        MAX_H2_INPUT_STREAMS,
        MAX_H2_FRAME_WORK_UNITS,
        MAX_WINDOW_OUTPUT_BYTES,
        MAX_WINDOW_OUTPUT_BYTES
    );
    append_stream_metadata(&mut manifest, &output.summary, "input_stream")?;
    if manifest.len() > MAX_MANIFEST_BYTES {
        return Err("hour manifest exceeds the 1 MiB limit".into());
    }
    Ok(manifest)
}

fn publish_window_bundle(
    output_dir: &Path,
    centre: [u8; 4],
    output: &WindowRecordingCliOutput,
    executable_sha: [u8; 32],
) -> Result<(), String> {
    let quarantine_tsv = serialize_window_quarantine(output);
    if quarantine_tsv.len() > MAX_QUARANTINE_BYTES {
        return Err("aggregate quarantine output exceeds byte limit".into());
    }
    let report = serialize_window_report(output)?;
    let normalized_hash = oa_sha256::sha256(output.normalized_tsv.as_bytes());
    let provenance_hash = oa_sha256::sha256(output.provenance_tsv.as_bytes());
    let quarantine_hash = oa_sha256::sha256(quarantine_tsv.as_bytes());
    let report_hash = oa_sha256::sha256(report.as_bytes());
    let manifest = serialize_window_manifest(
        centre,
        output,
        WindowManifestEvidence {
            executable_sha,
            normalized_hash,
            provenance_hash,
            quarantine_hash,
            report_hash,
            report_len: report.len(),
            quarantine_len: quarantine_tsv.len(),
        },
    )?;
    let manifest_hash = oa_sha256::sha256(manifest.as_bytes());

    create_output_dir(output_dir)?;
    let normalized_path = output_dir.join("normalized.tsv");
    let provenance_path = output_dir.join("provenance.tsv");
    let quarantine_path = output_dir.join("quarantine.tsv");
    let report_path = output_dir.join("counts.tsv");
    let manifest_path = output_dir.join("hour-manifest.tsv");
    write_new(&normalized_path, output.normalized_tsv.as_bytes())?;
    write_new(&provenance_path, output.provenance_tsv.as_bytes())?;
    write_new(&quarantine_path, quarantine_tsv.as_bytes())?;
    write_new(&report_path, report.as_bytes())?;
    write_new(&manifest_path, manifest.as_bytes())?;
    verify_file(
        &normalized_path,
        output.normalized_tsv.len(),
        &normalized_hash,
    )?;
    verify_file(
        &provenance_path,
        output.provenance_tsv.len(),
        &provenance_hash,
    )?;
    verify_file(&quarantine_path, quarantine_tsv.len(), &quarantine_hash)?;
    verify_file(&report_path, report.len(), &report_hash)?;
    verify_file(&manifest_path, manifest.len(), &manifest_hash)?;

    let directory = oa_sealed::open(output_dir)
        .map_err(|error| format!("open hour output for sync: {error}"))?;
    directory
        .sync_all()
        .map_err(|error| format!("sync hour output directory: {error}"))?;
    let complete_path = output_dir.join("HOUR_COMPLETE");
    let complete = format!("{}\n", oa_sha256::to_hex(&manifest_hash));
    write_new(&complete_path, complete.as_bytes())?;
    verify_file(
        &complete_path,
        complete.len(),
        &oa_sha256::sha256(complete.as_bytes()),
    )?;
    directory
        .sync_all()
        .map_err(|error| format!("sync hour completion directory: {error}"))?;
    Ok(())
}

fn run_window_import(
    inputs: &[RecordingInput],
    centre: [u8; 4],
    window_ns: (i64, i64),
    flight_centre_scope: Option<[u8; 4]>,
    output_dir: &Path,
) -> Result<(), String> {
    oa_sealed::refuse(output_dir)
        .map_err(|error| format!("guarded hour output directory: {error}"))?;
    let executable_sha = executable_digest()?;
    let output =
        convert_recorded_hour_window_scoped(inputs, centre, window_ns, flight_centre_scope)
            .map_err(|error| format!("window recording conversion: {error:?}"))?;
    publish_window_bundle(output_dir, centre, &output, executable_sha)?;
    eprintln!(
        "window recording import: normalized_events={} selected_events={} excluded_other_centre={} quarantine_rows={} manual_handling={} streams={} frames={} output={}",
        output.summary.normalized_events,
        output.selected_normalized_events,
        output.excluded_other_centre_events,
        output.summary.quarantine_rows,
        output.summary.manual_handling_required,
        output.summary.recording.streams.len(),
        output.summary.recording.counts.frames,
        output_dir.display()
    );
    Ok(())
}

fn create_output_dir(path: &Path) -> Result<(), String> {
    oa_sealed::refuse(path).map_err(|error| format!("guarded output directory: {error}"))?;
    fs::create_dir(path).map_err(|error| format!("create new output directory: {error}"))
}

fn run() -> Result<(), String> {
    let args = parse_arguments()?;
    let centre = args.centre.ok_or("--centre is required")?;
    let output_path = args.output.as_ref().ok_or("--out is required")?;
    if let Some(window_ns) = args.window {
        return run_window_import(
            &args.inputs,
            centre,
            window_ns,
            args.flight_centre_scope,
            output_path,
        );
    }
    oa_sealed::refuse(output_path).map_err(|error| format!("guarded output directory: {error}"))?;
    let executable_sha = executable_digest()?;
    let output = convert_recordings(&args.inputs, centre)
        .map_err(|error| format!("recording conversion: {error:?}"))?;
    if output.normalized_tsv.len() > MAX_NORMALIZED_BYTES {
        return Err("normalized TSV exceeds the 16 MiB session input limit".into());
    }
    if output.provenance_tsv.len() > MAX_PROVENANCE_BYTES {
        return Err("provenance TSV exceeds the 32 MiB publication limit".into());
    }
    if output.quarantine_tsv.len() > MAX_QUARANTINE_BYTES {
        return Err("quarantine TSV exceeds the 64 MiB publication limit".into());
    }

    use std::fmt::Write as FmtWrite;
    let mut report = String::from("OA_RECORDING_IMPORT_REPORT\t1\n");
    writeln!(report, "caps\tcompressed_bytes={}\texpanded_bytes={}\tinput_frames={}\tinput_streams={}\toaf_payload_bytes={}\tnormalized_input_bytes={}\tnormalized_events={}\tevents_per_receipt={}\truntime_groups={}\truntime_gap_ns={}",
        MAX_RECORDING_INPUT_BYTES, MAX_RECORDING_INPUT_BYTES, MAX_RECORDING_FRAMES, MAX_RECORDING_STREAMS,
        oa_feed::MAX_PAYLOAD, MAX_NORMALIZED_BYTES, MAX_SESSION_EVENTS, MAX_EVENTS_PER_RECEIPT, MAX_FRAMES, MAX_FRAME_GAP_NS)
        .map_err(|_| "report formatting failed")?;
    writeln!(
        report,
        "recording_counts\t{:?}",
        output.adaptation.recording
    )
    .map_err(|_| "report formatting failed")?;
    writeln!(report, "adapter_counts\t{:?}", output.adaptation.counts)
        .map_err(|_| "report formatting failed")?;
    writeln!(
        report,
        "quarantine_stats\t{:?}",
        output.adaptation.quarantine_stats
    )
    .map_err(|_| "report formatting failed")?;
    writeln!(
        report,
        "quarantine_records\t{}",
        output.adaptation.quarantine.len()
    )
    .map_err(|_| "report formatting failed")?;
    let direct_manual_required = output
        .adaptation
        .quarantine
        .iter()
        .filter(|record| record.manual_handling_required)
        .count();
    writeln!(
        report,
        "manual_handling_required\t{}\tstatus={}",
        output.adaptation.quarantine.len(),
        if output.adaptation.quarantine.is_empty() {
            "NONE"
        } else {
            "REVIEW_REQUIRED"
        }
    )
    .map_err(|_| "report formatting failed")?;
    writeln!(report, "queue_full_direct_manual\t{direct_manual_required}")
        .map_err(|_| "report formatting failed")?;
    let [flight_stats, surveillance_stats, adaptation_stats] = output.adaptation.quarantine_stats;
    for (name, stats) in [
        ("FLIGHT", flight_stats),
        ("SURVEILLANCE", surveillance_stats),
        ("ADAPTATION", adaptation_stats),
    ] {
        writeln!(
            report,
            "quarantine_class\t{name}\taccepted={}\tqueued={}\tqueue_full_manual={}\tacknowledged={}\treasons={:?}",
            stats.accepted,
            stats.queued,
            stats.queue_full_manual_events,
            stats.acknowledged,
            stats.reason_counts
        )
        .map_err(|_| "report formatting failed")?;
    }
    for stream in &output.streams {
        let source = match stream.source {
            RecordingSource::Adsb => "ADSB",
            RecordingSource::Sfdps => "SFDPS",
        };
        writeln!(
            report,
            "input_stream\t{}\t{}\t{}\t{}\t{}\t{}",
            stream.stream_index,
            source,
            oa_sha256::to_hex(&stream.sha256),
            stream.compressed_bytes,
            stream.decoded_bytes,
            u8::from(stream.gzip)
        )
        .map_err(|_| "report formatting failed")?;
    }
    if report.len() > MAX_REPORT_BYTES {
        return Err("import report exceeds byte limit".into());
    }

    let normalized_hash = oa_sha256::sha256(output.normalized_tsv.as_bytes());
    let provenance_hash = oa_sha256::sha256(output.provenance_tsv.as_bytes());
    let quarantine_hash = oa_sha256::sha256(output.quarantine_tsv.as_bytes());
    let report_hash = oa_sha256::sha256(report.as_bytes());
    let mut manifest = format!(
        "OA_RECORDING_IMPORT_MANIFEST\t1\ncentre\t{}\nnormalized_bytes\t{}\nnormalized_sha256\t{}\nprovenance_bytes\t{}\nprovenance_sha256\t{}\nquarantine_bytes\t{}\nquarantine_sha256\t{}\nreport_bytes\t{}\nreport_sha256\t{}\nexecutable_sha256\t{}\nphysical_input_cap_bytes\t{}\nexpanded_input_cap_bytes\t{}\nframe_cap\t{}\nstream_cap\t{}\nnormalized_input_cap_bytes\t{}\nnormalized_event_cap\t{}\nevents_per_receipt_cap\t{}\nruntime_group_cap\t{}\nruntime_gap_cap_ns\t{}\nquarantine_record_cap\t{}\nquarantine_bytes_cap\t{}\n",
        core::str::from_utf8(&centre).map_err(|_| "centre encoding")?,
        output.normalized_tsv.len(),
        oa_sha256::to_hex(&normalized_hash),
        output.provenance_tsv.len(),
        oa_sha256::to_hex(&provenance_hash),
        output.quarantine_tsv.len(),
        oa_sha256::to_hex(&quarantine_hash),
        report.len(),
        oa_sha256::to_hex(&report_hash),
        oa_sha256::to_hex(&executable_sha),
        MAX_RECORDING_INPUT_BYTES,
        MAX_RECORDING_INPUT_BYTES,
        MAX_RECORDING_FRAMES,
        MAX_RECORDING_STREAMS,
        MAX_NORMALIZED_BYTES,
        MAX_SESSION_EVENTS,
        MAX_EVENTS_PER_RECEIPT,
        MAX_FRAMES,
        MAX_FRAME_GAP_NS,
        oa_regional_session::recordings::MAX_QUARANTINE_RECORDS,
        MAX_QUARANTINE_BYTES
    );
    for stream in &output.streams {
        let source = match stream.source {
            RecordingSource::Adsb => "ADSB",
            RecordingSource::Sfdps => "SFDPS",
        };
        writeln!(
            manifest,
            "input_stream\t{}\t{}\t{}\t{}\t{}\t{}",
            stream.stream_index,
            source,
            oa_sha256::to_hex(&stream.sha256),
            stream.compressed_bytes,
            stream.decoded_bytes,
            u8::from(stream.gzip)
        )
        .map_err(|_| "manifest formatting failed")?;
    }
    if manifest.len() > MAX_MANIFEST_BYTES {
        return Err("manifest exceeds byte limit".into());
    }
    let manifest_hash = oa_sha256::sha256(manifest.as_bytes());

    create_output_dir(output_path)?;
    let normalized_path = output_path.join("normalized.tsv");
    let provenance_path = output_path.join("provenance.tsv");
    let quarantine_path = output_path.join("quarantine.tsv");
    let report_path = output_path.join("counts.tsv");
    let manifest_path = output_path.join("manifest.tsv");
    write_new(&normalized_path, output.normalized_tsv.as_bytes())?;
    write_new(&provenance_path, output.provenance_tsv.as_bytes())?;
    write_new(&quarantine_path, output.quarantine_tsv.as_bytes())?;
    write_new(&report_path, report.as_bytes())?;
    write_new(&manifest_path, manifest.as_bytes())?;
    verify_file(
        &normalized_path,
        output.normalized_tsv.len(),
        &normalized_hash,
    )?;
    verify_file(
        &provenance_path,
        output.provenance_tsv.len(),
        &provenance_hash,
    )?;
    verify_file(
        &quarantine_path,
        output.quarantine_tsv.len(),
        &quarantine_hash,
    )?;
    verify_file(&report_path, report.len(), &report_hash)?;
    verify_file(&manifest_path, manifest.len(), &manifest_hash)?;

    let directory = oa_sealed::open(output_path)
        .map_err(|error| format!("open output directory for sync: {error}"))?;
    directory
        .sync_all()
        .map_err(|error| format!("sync output directory: {error}"))?;
    let complete_path = output_path.join("COMPLETE");
    let complete = format!("{}\n", oa_sha256::to_hex(&manifest_hash));
    let complete_result = write_new(&complete_path, complete.as_bytes())
        .and_then(|_| {
            verify_file(
                &complete_path,
                complete.len(),
                &oa_sha256::sha256(complete.as_bytes()),
            )
        })
        .and_then(|_| {
            directory
                .sync_all()
                .map_err(|error| format!("sync completion directory: {error}"))
        });
    if let Err(error) = complete_result {
        let cleanup = fs::remove_file(&complete_path);
        let cleanup_sync = directory.sync_all();
        return Err(format!(
            "completion marker failed: {error}; cleanup={cleanup:?}; cleanup_sync={cleanup_sync:?}"
        ));
    }
    eprintln!(
        "recording import complete: events={} quarantine={} streams={} frames={} output={}",
        output.adaptation.events.len(),
        output.adaptation.quarantine.len(),
        output.streams.len(),
        output.frame_count,
        output_path.display()
    );
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("regional-recording-import: {error}");
        std::process::exit(2);
    }
}
