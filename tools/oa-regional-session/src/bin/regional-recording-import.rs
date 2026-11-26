#![forbid(unsafe_code)]
//! Bounded recorded OAF to OA-RS importer and create-new evidence publisher (shell, B1/B3/C6).

use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use oa_regional_session::recordings_cli::convert_recordings;
use oa_regional_session::recordings_reader::{
    RecordingInput, RecordingSource, MAX_RECORDING_FRAMES, MAX_RECORDING_INPUT_BYTES,
    MAX_RECORDING_STREAMS,
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

// A7.2: cold shell command-line values, with at most eight source paths.
struct Arguments {
    inputs: Vec<RecordingInput>,
    centre: [u8; 4],
    output: PathBuf,
}

fn parse_arguments() -> Result<Arguments, String> {
    let mut args = env::args_os().skip(1);
    let mut inputs = Vec::new();
    inputs
        .try_reserve_exact(MAX_RECORDING_STREAMS)
        .map_err(|_| "input list allocation refused")?;
    let mut centre = None;
    let mut output = None;
    while let Some(flag) = args.next() {
        match flag.to_str() {
            Some("--adsb") | Some("--sfdps") => {
                let source = match flag.to_str() { Some("--adsb") => RecordingSource::Adsb, _ => RecordingSource::Sfdps };
                let path = args.next().map(PathBuf::from).ok_or("missing path after source option")?;
                if inputs.len() >= MAX_RECORDING_STREAMS { return Err("source stream limit exceeded".into()); }
                inputs.push(RecordingInput { source, path });
            }
            Some("--centre") if centre.is_none() => {
                let value = args.next().ok_or("missing value after --centre")?;
                let text = value.to_str().ok_or("centre must be UTF-8")?;
                let raw = text.as_bytes();
                if raw.len() != 4 || !raw.iter().all(u8::is_ascii_alphanumeric) { return Err("centre must be four ASCII alphanumeric bytes".into()); }
                let mut code = [0u8; 4];
                code.copy_from_slice(raw);
                centre = Some(code);
            }
            Some("--out") if output.is_none() => output = Some(PathBuf::from(args.next().ok_or("missing path after --out")?)),
            _ => return Err("usage: regional-recording-import (--adsb FILE | --sfdps FILE)+ (at most 8 inputs) --centre ZAU1 --out NEW_DIR".into()),
        }
    }
    if inputs.is_empty() {
        return Err("at least one --adsb or --sfdps input is required".into());
    }
    Ok(Arguments {
        inputs,
        centre: centre.ok_or("--centre is required")?,
        output: output.ok_or("--out is required")?,
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

fn create_output_dir(path: &Path) -> Result<(), String> {
    oa_sealed::refuse(path).map_err(|error| format!("guarded output directory: {error}"))?;
    fs::create_dir(path).map_err(|error| format!("create new output directory: {error}"))
}

fn run() -> Result<(), String> {
    let args = parse_arguments()?;
    oa_sealed::refuse(&args.output)
        .map_err(|error| format!("guarded output directory: {error}"))?;
    let executable_sha = executable_digest()?;
    let output = convert_recordings(&args.inputs, args.centre)
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
    let mut manifest = format!("OA_RECORDING_IMPORT_MANIFEST\t1\ncentre\t{}\nnormalized_bytes\t{}\nnormalized_sha256\t{}\nprovenance_bytes\t{}\nprovenance_sha256\t{}\nquarantine_bytes\t{}\nquarantine_sha256\t{}\nreport_bytes\t{}\nreport_sha256\t{}\nexecutable_sha256\t{}\nphysical_input_cap_bytes\t{}\nexpanded_input_cap_bytes\t{}\nframe_cap\t{}\nstream_cap\t{}\nnormalized_input_cap_bytes\t{}\nnormalized_event_cap\t{}\nevents_per_receipt_cap\t{}\nruntime_group_cap\t{}\nruntime_gap_cap_ns\t{}\nquarantine_record_cap\t{}\nquarantine_bytes_cap\t{}\n",
        core::str::from_utf8(&args.centre).map_err(|_| "centre encoding")?, output.normalized_tsv.len(), oa_sha256::to_hex(&normalized_hash),
        output.provenance_tsv.len(), oa_sha256::to_hex(&provenance_hash), output.quarantine_tsv.len(), oa_sha256::to_hex(&quarantine_hash), report.len(), oa_sha256::to_hex(&report_hash),
        oa_sha256::to_hex(&executable_sha), MAX_RECORDING_INPUT_BYTES, MAX_RECORDING_INPUT_BYTES, MAX_RECORDING_FRAMES, MAX_RECORDING_STREAMS,
        MAX_NORMALIZED_BYTES, MAX_SESSION_EVENTS, MAX_EVENTS_PER_RECEIPT, MAX_FRAMES, MAX_FRAME_GAP_NS,
        oa_regional_session::recordings::MAX_QUARANTINE_RECORDS, MAX_QUARANTINE_BYTES);
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

    create_output_dir(&args.output)?;
    let normalized_path = args.output.join("normalized.tsv");
    let provenance_path = args.output.join("provenance.tsv");
    let quarantine_path = args.output.join("quarantine.tsv");
    let report_path = args.output.join("counts.tsv");
    let manifest_path = args.output.join("manifest.tsv");
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

    let directory = oa_sealed::open(&args.output)
        .map_err(|error| format!("open output directory for sync: {error}"))?;
    directory
        .sync_all()
        .map_err(|error| format!("sync output directory: {error}"))?;
    let complete_path = args.output.join("COMPLETE");
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
        args.output.display()
    );
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("regional-recording-import: {error}");
        std::process::exit(2);
    }
}
