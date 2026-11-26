//! Stream the fixed 2026-11-19 ZAU development partition through the recorded-peak runtime.
#![forbid(unsafe_code)]

use oa_airspace::core::model::{point_kind, str_at, Model};
use oa_regional_session::cli::read_guarded;
use oa_regional_session::migration_full_day::{render_report, ReportMetadata};
use oa_regional_session::migration_full_day_runner::{run_full_day, FullDayRunner};
use oa_regional_session::migration_g3::parse_training_model;
use oa_regional_session::recordings_reader::RecordingSource;
use oa_regional_session::recordings_stream::StreamInput;
use oa_regional_session::surveillance::SurveillanceConfig;
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const MAX_G3_BYTES: usize = 64 * 1024 * 1024;
const MAX_SNAPSHOT_BYTES: usize = 64 * 1024 * 1024;
const MAX_EXECUTABLE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_REPORT_BYTES: usize = 64 * 1024;
const HASH_CHUNK_BYTES: usize = 64 * 1024;
const POLICY_BYTES: &[u8] = include_bytes!("../../../../docs/migration/H2_NONINFERIORITY_V4.json");

struct Arguments {
    inputs: Vec<StreamInput>,
    snapshot: PathBuf,
    g3_artifact: PathBuf,
    g3_artifact_sha256: String,
    g3_training_sha256: String,
    g3_source_revision: String,
    g3_binary_sha256: String,
    g3_policy_sha256: String,
    output: PathBuf,
}

fn parse_arguments() -> Result<Arguments, String> {
    let mut args = env::args_os().skip(1);
    let mut inputs = Vec::new();
    inputs
        .try_reserve_exact(32)
        .map_err(|_| "input list allocation refused")?;
    let mut snapshot = None;
    let mut g3_artifact = None;
    let mut g3_artifact_sha256 = None;
    let mut g3_training_sha256 = None;
    let mut g3_source_revision = None;
    let mut g3_binary_sha256 = None;
    let mut g3_policy_sha256 = None;
    let mut output = None;
    while let Some(flag) = args.next() {
        let name = flag.to_str().ok_or("option is not UTF-8")?;
        match name {
            "--adsb" | "--sfdps" => {
                if inputs.len() >= 32 { return Err("input stream cap of 32 exceeded".into()); }
                let source = if name == "--adsb" { RecordingSource::Adsb } else { RecordingSource::Sfdps };
                let path = PathBuf::from(args.next().ok_or("missing input path")?);
                if inputs.iter().any(|input: &StreamInput| input.path == path) {
                    return Err("duplicate input path refused".into());
                }
                inputs.push(StreamInput { source, path });
            }
            "--airspace-snapshot" if snapshot.is_none() => snapshot = Some(PathBuf::from(args.next().ok_or("missing airspace snapshot")?)),
            "--g3-artifact" if g3_artifact.is_none() => g3_artifact = Some(PathBuf::from(args.next().ok_or("missing G3 artifact")?)),
            "--g3-artifact-sha256" if g3_artifact_sha256.is_none() => g3_artifact_sha256 = Some(text_arg(args.next(), "missing G3 artifact hash")?),
            "--g3-training-sha256" if g3_training_sha256.is_none() => g3_training_sha256 = Some(text_arg(args.next(), "missing G3 training hash")?),
            "--g3-source-revision" if g3_source_revision.is_none() => g3_source_revision = Some(text_arg(args.next(), "missing G3 source revision")?),
            "--g3-binary-sha256" if g3_binary_sha256.is_none() => g3_binary_sha256 = Some(text_arg(args.next(), "missing G3 binary hash")?),
            "--g3-policy-sha256" if g3_policy_sha256.is_none() => g3_policy_sha256 = Some(text_arg(args.next(), "missing G3 policy hash")?),
            "--out" if output.is_none() => output = Some(PathBuf::from(args.next().ok_or("missing output directory")?)),
            "--help" => return Err("usage: regional-m4-h2-full-day (--adsb FILE | --sfdps FILE)+ --airspace-snapshot FILE --g3-artifact FILE --g3-artifact-sha256 SHA256 --g3-training-sha256 SHA256 --g3-source-revision REV --g3-binary-sha256 SHA256 --g3-policy-sha256 SHA256 --out NEW_DIR".into()),
            _ => return Err(format!("unknown or duplicate option: {name}")),
        }
    }
    if inputs.is_empty() {
        return Err("at least one ADS-B and one SFDPS stream are required".into());
    }
    if !inputs
        .iter()
        .any(|input| input.source == RecordingSource::Adsb)
        || !inputs
            .iter()
            .any(|input| input.source == RecordingSource::Sfdps)
    {
        return Err("both --adsb and --sfdps inputs are required".into());
    }
    Ok(Arguments {
        inputs,
        snapshot: snapshot.ok_or("--airspace-snapshot is required")?,
        g3_artifact: g3_artifact.ok_or("--g3-artifact is required")?,
        g3_artifact_sha256: g3_artifact_sha256.ok_or("--g3-artifact-sha256 is required")?,
        g3_training_sha256: g3_training_sha256.ok_or("--g3-training-sha256 is required")?,
        g3_source_revision: g3_source_revision.ok_or("--g3-source-revision is required")?,
        g3_binary_sha256: g3_binary_sha256.ok_or("--g3-binary-sha256 is required")?,
        g3_policy_sha256: g3_policy_sha256.ok_or("--g3-policy-sha256 is required")?,
        output: output.ok_or("--out is required")?,
    })
}

fn text_arg(value: Option<std::ffi::OsString>, error: &'static str) -> Result<String, String> {
    value
        .ok_or(error)?
        .into_string()
        .map_err(|_| "argument value is not UTF-8".into())
}

fn model_centre(model: &Model) -> Result<(i32, i32), String> {
    if model.meta.artcc != "ZAU" {
        return Err("airspace snapshot ARTCC is not ZAU".into());
    }
    let mut found = None;
    for index in 0..model.points.kind.len() {
        if model.points.kind.get(index) != Some(&point_kind::AIRPORT) {
            continue;
        }
        let ident = model
            .points
            .ident
            .get(index)
            .and_then(|ix| str_at(&model.strings, *ix));
        if ident == Some("KORD") {
            if found.is_some() {
                return Err("airspace snapshot has multiple KORD reference points".into());
            }
            let lat = *model
                .points
                .lat
                .get(index)
                .ok_or("KORD latitude column mismatch")?;
            let lon = *model
                .points
                .lon
                .get(index)
                .ok_or("KORD longitude column mismatch")?;
            if !(-900_000_000..=900_000_000).contains(&lat)
                || !(-1_800_000_000..=1_800_000_000).contains(&lon)
            {
                return Err("KORD reference point outside coordinate bounds".into());
            }
            found = Some((lat, lon));
        }
    }
    found.ok_or_else(|| "airspace snapshot has no KORD reference point".into())
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
            .checked_add(u64::try_from(read).map_err(|_| "digest size overflow")?)
            .ok_or("digest size overflow")?;
        if total > max_bytes {
            return Err("file grew beyond digest cap".into());
        }
        hash.update(buffer.get(..read).ok_or("digest chunk bounds")?)
            .map_err(|error| error.to_string())?;
    }
    Ok((hash.finalize(), total))
}

fn executable_sha256() -> Result<String, String> {
    let path = env::current_exe().map_err(|error| error.to_string())?;
    let (file, len) =
        oa_sealed::open_regular(&path).map_err(|error| format!("guarded executable: {error}"))?;
    if len > MAX_EXECUTABLE_BYTES {
        return Err("executable exceeds byte cap".into());
    }
    let (sha, bytes) = digest_file(file, MAX_EXECUTABLE_BYTES)?;
    if bytes != len {
        return Err("executable changed while hashing".into());
    }
    Ok(oa_sha256::to_hex(&sha))
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    oa_sealed::refuse(path).map_err(|error| format!("guarded output refused: {error}"))?;
    // seal-allow: construct exclusive create options for the already-refused report destination.
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(|error| error.to_string())?;
    oa_sealed::Sealed::production()
        .verify_open(path, &file)
        .map_err(|error| error.to_string())?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|error| error.to_string())
}

fn aggregate_input_sha256(
    summary: &oa_regional_session::recordings_stream::StreamSummary,
) -> Result<String, String> {
    let mut hash = oa_sha256::Sha256::new();
    for (index, stream) in summary.streams.iter().enumerate() {
        let ordinal = u64::try_from(index)
            .map_err(|_| "stream ordinal overflow")?
            .to_le_bytes();
        hash.update(&ordinal).map_err(|error| error.to_string())?;
        hash.update(&[match stream.source {
            RecordingSource::Adsb => 1,
            RecordingSource::Sfdps => 2,
        }])
        .map_err(|error| error.to_string())?;
        hash.update(&stream.bytes.to_le_bytes())
            .map_err(|error| error.to_string())?;
        hash.update(&stream.sha256)
            .map_err(|error| error.to_string())?;
    }
    Ok(oa_sha256::to_hex(&hash.finalize()))
}

fn run() -> Result<(), String> {
    let args = parse_arguments()?;
    oa_sealed::refuse(&args.output)
        .map_err(|error| format!("guarded output path refused: {error}"))?;
    let executable_sha = executable_sha256()?;
    let snapshot_bytes = read_guarded(&args.snapshot, MAX_SNAPSHOT_BYTES)?;
    let airspace_sha = oa_sha256::to_hex(&oa_sha256::sha256(&snapshot_bytes));
    let model = oa_airspace::core::snapshot::decode(&snapshot_bytes)
        .map_err(|error| format!("airspace snapshot decode: {error:?}"))?;
    let (centre_lat_e7, centre_lon_e7) = model_centre(&model)?;
    let g3_bytes = read_guarded(&args.g3_artifact, MAX_G3_BYTES)?;
    let g3_sha = oa_sha256::to_hex(&oa_sha256::sha256(&g3_bytes));
    if g3_sha != args.g3_artifact_sha256 {
        return Err("G3 artifact hash differs from pinned input".into());
    }
    let g3_model = parse_training_model(
        &g3_bytes,
        &args.g3_artifact_sha256,
        &args.g3_training_sha256,
        &args.g3_source_revision,
        &args.g3_binary_sha256,
        &args.g3_policy_sha256,
    )
    .map_err(|error| format!("G3 artifact provenance: {error:?}"))?;
    let runner = FullDayRunner::new(
        model,
        oa_sha256::sha256(&snapshot_bytes),
        SurveillanceConfig {
            centre_lat_e7,
            centre_lon_e7,
            range_nm: 1_000.0,
        },
        g3_model,
    )?;
    let (counts, summary) = run_full_day(&args.inputs, runner)?;
    let adsb_coverage = oa_regional_session::recordings_stream::source_coverage(
        &summary.streams,
        RecordingSource::Adsb,
        (
            oa_regional_session::migration_full_day_runner::DAY_START_NS,
            oa_regional_session::migration_full_day_runner::DAY_END_NS,
        ),
        oa_regional_session::recordings_stream::H2_RECORDER_ACTIVITY_MAX_GAP_NS,
    )?;
    let sfdps_coverage = oa_regional_session::recordings_stream::source_coverage(
        &summary.streams,
        RecordingSource::Sfdps,
        (
            oa_regional_session::migration_full_day_runner::DAY_START_NS,
            oa_regional_session::migration_full_day_runner::DAY_END_NS,
        ),
        oa_regional_session::recordings_stream::H2_RECORDER_ACTIVITY_MAX_GAP_NS,
    )?;
    let full_recorder_activity_coverage =
        adsb_coverage.full_window_covered && sfdps_coverage.full_window_covered;
    let input_sha = aggregate_input_sha256(&summary)?;
    let policy_sha = oa_sha256::to_hex(&oa_sha256::sha256(POLICY_BYTES));
    let metadata = ReportMetadata {
        input_bytes: summary.counts.physical_bytes,
        input_sha256: &input_sha,
        executable_sha256: &executable_sha,
        airspace_sha256: &airspace_sha,
        g3_artifact_sha256: &g3_sha,
        policy_sha256: &policy_sha,
        streams: &summary.streams,
    };
    let mut report = render_report(&counts, &metadata).map_err(str::to_owned)?;
    use std::fmt::Write as FmtWrite;
    writeln!(
        report,
        "input_window_disposition\tall_frames={}\tin_window_messages={}\tin_window_status_frames={}\tin_window_reject_frames={}\toutside_window_frames={}\toutside_window_messages={}\toutside_window_status_frames={}\toutside_window_reject_frames={}",
        summary.counts.frames,
        summary.counts.messages,
        summary.counts.status_frames - summary.counts.outside_window_status_frames,
        summary.counts.reject_frames - summary.counts.outside_window_reject_frames,
        summary.counts.outside_window_frames,
        summary.counts.outside_window_messages,
        summary.counts.outside_window_status_frames,
        summary.counts.outside_window_reject_frames,
    )
    .map_err(|_| "input window disposition formatting failed")?;
    let runtime_caps = oa_regional_session::runtime::RegionalRunProfile::recorded_peak();
    writeln!(
        report,
        "caps\tphysical_bytes={}\texpanded_bytes={}\tinput_frames={}\tinput_streams={}\toaf_work_units={}\tpending_frames_per_stream={}\tpending_bytes_per_stream={}\tnormalized_events={}\truntime_frames={}\ttracks={}\tflights={}\tdrain_rows={}\topen_alerts={}\tcandidate_pairs={}\tca_work_per_frame={}\tevents_per_receipt={}\truntime_frame_gap_ns={}\tamendment_baselines={}\tamendment_baseline_bytes={}\tamendment_baseline_total_bytes={}\tamendment_index_slots={}\tamendment_index_bytes={}\tamendment_max_lookup_probes={}",
        oa_regional_session::recordings_stream::MAX_H2_PHYSICAL_INPUT_BYTES,
        oa_regional_session::recordings_stream::MAX_H2_EXPANDED_INPUT_BYTES,
        oa_regional_session::recordings_stream::MAX_H2_INPUT_FRAMES,
        oa_regional_session::recordings_stream::MAX_H2_INPUT_STREAMS,
        oa_regional_session::recordings_stream::MAX_H2_FRAME_WORK_UNITS,
        oa_regional_session::recordings_stream::MAX_PENDING_FRAMES_PER_STREAM,
        oa_regional_session::recordings_stream::MAX_PENDING_FRAME_BYTES_PER_STREAM,
        runtime_caps.max_total_events,
        runtime_caps.surveillance.max_frames,
        runtime_caps.surveillance.max_tracks,
        runtime_caps.max_flights,
        runtime_caps.surveillance.max_drain_rows,
        runtime_caps.surveillance.max_open_alerts,
        runtime_caps.surveillance.max_candidate_pairs,
        runtime_caps.surveillance.max_ca_work_per_frame,
        oa_regional_session::session::MAX_EVENTS_PER_RECEIPT,
        oa_regional_session::surveillance::MAX_FRAME_GAP_NS,
        oa_regional_session::migration_amendment::MAX_AMENDMENT_BASELINES,
        oa_regional_session::migration_amendment::AMENDMENT_BASELINE_BYTES,
        oa_regional_session::migration_amendment::MAX_AMENDMENT_BASELINES
            * oa_regional_session::migration_amendment::AMENDMENT_BASELINE_BYTES,
        oa_regional_session::migration_amendment::MAX_AMENDMENT_BASELINES
            .checked_mul(2)
            .and_then(usize::checked_next_power_of_two)
            .ok_or("amendment index size overflow")?,
        oa_regional_session::migration_amendment::MAX_AMENDMENT_BASELINES
            .checked_mul(2)
            .and_then(usize::checked_next_power_of_two)
            .and_then(|slots| slots.checked_mul(core::mem::size_of::<u32>()))
            .ok_or("amendment index byte size overflow")?,
        oa_regional_session::migration_amendment::MAX_LOOKUP_PROBES,
    ).map_err(|_| "profile report formatting failed")?;
    writeln!(
        report,
        "recorder_frame_activity_max_gap_ns\t{}\ncandidate_asof_max_state_age_ns\t1000000000",
        oa_regional_session::recordings_stream::H2_RECORDER_ACTIVITY_MAX_GAP_NS,
    )
    .map_err(|_| "time threshold report formatting failed")?;
    writeln!(report, "g3_training_sha256\t{}\ng3_source_revision\t{}\ng3_binary_sha256\t{}\ng3_policy_sha256\t{}", args.g3_training_sha256, args.g3_source_revision, args.g3_binary_sha256, args.g3_policy_sha256)
        .map_err(|_| "G3 provenance formatting failed")?;
    for coverage in [adsb_coverage, sfdps_coverage] {
        let source = match coverage.source {
            RecordingSource::Adsb => "ADSB",
            RecordingSource::Sfdps => "SFDPS",
        };
        writeln!(report, "source_recorder_frame_activity\t{source}\tfirst_frame_ns={}\tlast_frame_ns={}\tlargest_frame_activity_gap_ns={}\tfull_activity_window_covered={}",
            coverage.first_frame_receive_ns.map_or_else(|| "NONE".to_owned(), |value| value.to_string()),
            coverage.last_frame_receive_ns.map_or_else(|| "NONE".to_owned(), |value| value.to_string()),
            coverage.largest_gap_ns,
            u8::from(coverage.full_window_covered),
        ).map_err(|_| "coverage report formatting failed")?;
    }
    writeln!(
        report,
        "full_24h_recorder_frame_activity_coverage\t{}",
        u8::from(full_recorder_activity_coverage)
    )
    .map_err(|_| "coverage report formatting failed")?;
    if report.len() > MAX_REPORT_BYTES {
        return Err("aggregate report exceeds fixed byte cap".into());
    }
    report.push_str(
        "qualification\tNOT_ASSESSED\ninterpretation\tDEVELOPMENT_ONLY_CAUSAL_TRANSFER_PROXY\n",
    );
    if report.len() > MAX_REPORT_BYTES {
        return Err("aggregate report exceeds fixed byte cap".into());
    }

    let mut directory = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        directory.mode(0o700);
    }
    directory
        .create(&args.output)
        .map_err(|error| format!("create new output directory: {error}"))?;
    let report_path = args.output.join("h2-full-day-report.tsv");
    write_new(&report_path, report.as_bytes())?;
    let complete_path = args.output.join("COMPLETE");
    let completion: &[u8] = if full_recorder_activity_coverage {
        b"RUN_FINISHED\tFULL_24H_RECORDER_FRAME_ACTIVITY=true\tQUALIFICATION_NOT_ASSESSED\n"
    } else {
        b"RUN_FINISHED\tFULL_24H_RECORDER_FRAME_ACTIVITY=false\tQUALIFICATION_NOT_ASSESSED\n"
    };
    write_new(&complete_path, completion)?;
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("regional M4/H2 full-day: {error}");
        std::process::exit(1);
    }
}
