//! Fixed D2 synthetic stability mode. It keeps one production `RegionalRuntime` alive for the
//! whole run, feeding a bounded synthetic surveillance workload at real five-second cadence.
//! It is not an availability, field-shadow, recorded-traffic, or operational qualification.
#![forbid(unsafe_code)]

use oa_regional_session::input::{EventKind, Identity, NormalizedEvent, SourceKind};
use oa_regional_session::runtime::{
    regional_ack_presentation, regional_new_with_profile, regional_pixels,
    regional_presentation_counts, regional_step,
};
use oa_regional_session::surveillance::SurveillanceConfig;
use oa_regional_session::{cli::synthetic_model, ReceiptBatch};
use oa_sha256::{to_hex, Sha256};
use std::env::ArgsOs;
use std::ffi::OsString;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const SCHEMA: &str = "openatca-d2-paced-soak/1";
const TICK_NS: i64 = 5_000_000_000;
const TICK_DURATION: Duration = Duration::from_secs(5);
const FRAME_COUNT: u64 = 34_560;
const RUN_DURATION: Duration = Duration::from_secs(48 * 60 * 60);
const FIRST_ISSUE_NS: i64 = 1_000_000_000;
const AIRCRAFT_COUNT: usize = 16;
const SEQUENCE_STRIDE: u64 = 32;
const FEED_FAULT_START_TICK: u64 = 12_000;
const FEED_FAULT_TICKS: u64 = 2;
const SCHEDULE_LATE_THRESHOLD_NS: u64 = 250_000_000;
const MAX_PROGRESS_BYTES: u64 = 16 * 1024 * 1024;
const MAX_EXECUTABLE_BYTES: u64 = 128 * 1024 * 1024;
const PROGRESS_SYNC_FRAMES: u64 = 12;
const PROFILE_BYTES: &[u8] = b"D2-PACED-SOAK-V1;duration_ns=172800000000000;tick_ns=5000000000;frames=34560;tracks=16;grid_e7=250000;renderer=regional-base-raster-and-control-overlay-every-frame;profile=bounded-recorded-peak-derived;synthetic-model=public-test-fixture;feed-outage-start-tick=12000;feed-outage-ticks=2;schedule-late-threshold-ns=250000000;presentation-ack=ordered-after-progress-write;max-pending-presentations=8192";

fn clean_reason(reason: &str) -> String {
    reason
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect()
}

struct Arguments {
    candidate_commit: String,
    output: PathBuf,
}

#[derive(Default)]
struct Counts {
    frames: u64,
    progress_bytes: u64,
    late_frames: u64,
    max_lateness_ns: u64,
    max_actual_gap_ns: u64,
    max_processing_ns: u64,
    min_confirmed_tracks: usize,
    max_tracks: usize,
    max_open_alerts: usize,
    closed_alerts: u64,
    measurement_rejections: u64,
    quarantine_records: u64,
    incomplete_ca_frames: u64,
    presentation_updates_total: u64,
    presentation_pending_max: usize,
    presentation_pending_current: usize,
    feed_fault_start_utc_ns: Option<u128>,
    feed_fault_start_elapsed_ns: Option<u64>,
    feed_recovered_elapsed_ns: Option<u64>,
}

fn parse_args(args: ArgsOs) -> Result<Arguments, String> {
    let values: Vec<OsString> = args.collect();
    if values.len() != 4 {
        return Err(
            "usage: oa-regional-session soak --candidate-commit HEX40 --out NEW_DIR".into(),
        );
    }
    let flag_commit = values
        .first()
        .and_then(|value| value.to_str())
        .ok_or("candidate commit flag is not UTF-8")?;
    let candidate_commit = values
        .get(1)
        .and_then(|value| value.to_str())
        .ok_or("candidate commit is not UTF-8")?;
    let flag_out = values
        .get(2)
        .and_then(|value| value.to_str())
        .ok_or("output flag is not UTF-8")?;
    if flag_commit != "--candidate-commit"
        || flag_out != "--out"
        || candidate_commit.len() != 40
        || !candidate_commit
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("expected --candidate-commit lowercase-hex-40 --out NEW_DIR".into());
    }
    let output = PathBuf::from(values.get(3).ok_or("output path is missing")?);
    if !output.is_absolute() {
        return Err("output directory must be absolute".into());
    }
    Ok(Arguments {
        candidate_commit: candidate_commit.to_owned(),
        output,
    })
}

fn unix_ns() -> Result<u128, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .map_err(|_| "UTC clock is before Unix epoch".to_string())
}

fn elapsed_ns(start: Instant, now: Instant) -> Result<u64, String> {
    u64::try_from(
        now.checked_duration_since(start)
            .ok_or("monotonic clock regressed")?
            .as_nanos(),
    )
    .map_err(|_| "monotonic elapsed time exceeds u64".into())
}

fn hash_file(path: &Path, cap: u64) -> Result<[u8; 32], String> {
    let (mut file, length) = oa_sealed::open_regular(path).map_err(|error| error.to_string())?;
    if length > cap {
        return Err("executable exceeds its byte cap".into());
    }
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut bytes = 0_u64;
    loop {
        let read = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        bytes = bytes
            .checked_add(u64::try_from(read).map_err(|_| "executable length overflow")?)
            .ok_or("executable length overflow")?;
        if bytes > cap {
            return Err("executable grew beyond its byte cap".into());
        }
        hash.update(&buffer[..read])
            .map_err(|error| error.to_string())?;
    }
    if bytes != length {
        return Err("executable length changed while hashing".into());
    }
    Ok(hash.finalize())
}

fn create_output(path: &Path) -> Result<(), String> {
    oa_sealed::refuse(path).map_err(|error| format!("output path refused: {error}"))?;
    let mut builder = DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path).map_err(|error| error.to_string())
}

fn create_new_file(path: &Path) -> Result<File, String> {
    oa_sealed::refuse(path).map_err(|error| format!("output file refused: {error}"))?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path).map_err(|error| error.to_string())?;
    oa_sealed::Sealed::production()
        .verify_open(path, &file)
        .map_err(|error| error.to_string())?;
    Ok(file)
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = create_new_file(path)?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| error.to_string())
}

fn stop_requested(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.to_string()),
        Ok(metadata) if !metadata.file_type().is_file() => {
            Err("stop request must be a regular non-symlink file".into())
        }
        Ok(_) => {
            let (mut file, length) =
                oa_sealed::open_regular(path).map_err(|error| error.to_string())?;
            if length > 64 {
                return Err("stop request exceeds 64 bytes".into());
            }
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)
                .map_err(|error| error.to_string())?;
            Ok(bytes == b"OA-D2-SOAK-STOP-1\n")
        }
    }
}

fn make_batch(tick: u64) -> Result<ReceiptBatch, String> {
    let tick_i64 = i64::try_from(tick).map_err(|_| "tick index width")?;
    let receipt_ns = FIRST_ISSUE_NS
        .checked_add(tick_i64.checked_mul(TICK_NS).ok_or("issue time overflow")?)
        .ok_or("issue time overflow")?;
    let feed_down = (FEED_FAULT_START_TICK
        ..FEED_FAULT_START_TICK
            .checked_add(FEED_FAULT_TICKS)
            .ok_or("feed fault interval overflow")?)
        .contains(&tick);
    let feed_up = tick
        == FEED_FAULT_START_TICK
            .checked_add(FEED_FAULT_TICKS)
            .ok_or("feed recovery tick overflow")?;
    let mut events = Vec::with_capacity(AIRCRAFT_COUNT + if feed_down || feed_up { 1 } else { 0 });
    let sequence_base = tick
        .checked_mul(SEQUENCE_STRIDE)
        .ok_or("source sequence overflow")?;
    if feed_down || feed_up {
        events.push(NormalizedEvent {
            receipt_ns,
            source_ns: receipt_ns,
            sequence: sequence_base
                .checked_add(1)
                .ok_or("feed-state sequence overflow")?,
            source: SourceKind::Surveillance,
            identity: Identity {
                flight_gufi: 0,
                aircraft_address: 0,
            },
            centre: *b"ZAU1",
            kind: EventKind::FeedState {
                available: !feed_down,
                reason: u8::from(feed_down),
            },
        });
    }
    if feed_down {
        return Ok(ReceiptBatch {
            receipt_ns,
            source_clock_regressions: 0,
            events,
        });
    }
    for index in 0..AIRCRAFT_COUNT {
        let row = i32::try_from(index / 4).map_err(|_| "synthetic row width")?;
        let col = i32::try_from(index % 4).map_err(|_| "synthetic column width")?;
        let lat_offset = row.checked_mul(250_000).ok_or("latitude offset overflow")?;
        let lon_offset = col
            .checked_mul(250_000)
            .ok_or("longitude offset overflow")?;
        let address = 0x00ab_c101_u32
            .checked_add(u32::try_from(index).map_err(|_| "address index width")?)
            .ok_or("synthetic address overflow")?;
        let sequence = sequence_base
            .checked_add(u64::try_from(index).map_err(|_| "event index width")?)
            .and_then(|value| value.checked_add(if feed_up { 3 } else { 2 }))
            .ok_or("source sequence overflow")?;
        events.push(NormalizedEvent {
            receipt_ns,
            source_ns: receipt_ns,
            sequence,
            source: SourceKind::Surveillance,
            identity: Identity {
                flight_gufi: 0,
                aircraft_address: address,
            },
            centre: *b"ZAU1",
            kind: EventKind::Position {
                latitude_e7: 410_000_000_i32
                    .checked_add(lat_offset)
                    .ok_or("synthetic latitude overflow")?,
                longitude_e7: -870_000_000_i32
                    .checked_add(lon_offset)
                    .ok_or("synthetic longitude overflow")?,
                altitude_ft: Some(20_000),
                east_mmps: 0,
                north_mmps: 0,
                vertical_rate_fpm: Some(0),
            },
        });
    }
    Ok(ReceiptBatch {
        receipt_ns,
        source_clock_regressions: 0,
        events,
    })
}

fn hash_config(candidate_commit: &str, snapshot_sha: &[u8; 32]) -> Result<[u8; 32], String> {
    let mut hash = Sha256::new();
    hash.update(PROFILE_BYTES)
        .map_err(|error| error.to_string())?;
    hash.update(candidate_commit.as_bytes())
        .map_err(|error| error.to_string())?;
    hash.update(snapshot_sha)
        .map_err(|error| error.to_string())?;
    Ok(hash.finalize())
}

fn started_json(
    candidate: &str,
    executable_sha: &[u8; 32],
    snapshot_sha: &[u8; 32],
    config_sha: &[u8; 32],
    input_sha: &[u8; 32],
    start_utc_ns: u128,
) -> String {
    format!(
        "{{\"schema\":\"{SCHEMA}\",\"status\":\"RUNNING\",\"candidate_commit\":\"{candidate}\",\"executable_sha256\":\"{}\",\"synthetic_adaptation_sha256\":\"{}\",\"configuration_sha256\":\"{}\",\"synthetic_input_profile_sha256\":\"{}\",\"started_utc_unix_ns\":{start_utc_ns},\"planned_duration_ns\":{},\"cadence_ns\":{TICK_NS},\"frame_count\":{FRAME_COUNT},\"track_count\":{AIRCRAFT_COUNT},\"renderer\":\"regional base raster each frame; no live display\",\"recorded_or_live_inputs\":false}}\n",
        to_hex(executable_sha),
        to_hex(snapshot_sha),
        to_hex(config_sha),
        to_hex(input_sha),
        RUN_DURATION.as_nanos(),
    )
}

fn summary_json(
    candidate: &str,
    executable_sha: &[u8; 32],
    snapshot_sha: &[u8; 32],
    config_sha: &[u8; 32],
    input_sha: &[u8; 32],
    started_utc_ns: u128,
    ended_utc_ns: u128,
    elapsed_ns: u64,
    counts: &Counts,
    output_sha: &[u8; 32],
) -> String {
    let fault_start_utc = counts
        .feed_fault_start_utc_ns
        .map_or_else(|| "null".to_string(), |value| value.to_string());
    let fault_start_elapsed = counts
        .feed_fault_start_elapsed_ns
        .map_or_else(|| "null".to_string(), |value| value.to_string());
    let feed_recovery_elapsed = counts
        .feed_recovered_elapsed_ns
        .map_or_else(|| "null".to_string(), |value| value.to_string());
    format!(
        "{{\"schema\":\"{SCHEMA}\",\"status\":\"COMPLETED_SYNTHETIC_STABILITY_RUN\",\"qualification\":\"NONE\",\"candidate_commit\":\"{candidate}\",\"executable_sha256\":\"{}\",\"synthetic_adaptation_sha256\":\"{}\",\"configuration_sha256\":\"{}\",\"synthetic_input_profile_sha256\":\"{}\",\"started_utc_unix_ns\":{started_utc_ns},\"ended_utc_unix_ns\":{ended_utc_ns},\"monotonic_elapsed_ns\":{elapsed_ns},\"planned_duration_ns\":{},\"frames_expected\":{FRAME_COUNT},\"frames_completed\":{},\"tracks_min_after_warmup\":{},\"tracks_max\":{},\"ca_open_max\":{},\"ca_closed_total\":{},\"measurement_rejections\":{},\"quarantine_records\":{},\"incomplete_ca_frames\":{},\"presentation_updates_total\":{},\"presentation_pending_current\":{},\"presentation_pending_max\":{},\"schedule_late_threshold_ns\":{SCHEDULE_LATE_THRESHOLD_NS},\"late_frames_over_threshold\":{},\"max_schedule_lateness_ns\":{},\"max_actual_frame_gap_ns\":{},\"max_frame_processing_ns\":{},\"feed_fault_start_utc_ns\":{fault_start_utc},\"feed_fault_start_elapsed_ns\":{fault_start_elapsed},\"feed_recovered_elapsed_ns\":{feed_recovery_elapsed},\"progress_log_bytes\":{},\"progress_log_sha256\":\"{}\",\"recorded_or_live_inputs\":false,\"limitations\":[\"synthetic public-model workload\",\"feed-loss/reacquisition only; no process supervisor or restart recovery claim\",\"no independent thread or availability/MTBF claim\",\"no recorded-traffic or field-shadow claim\",\"no operator or live display\"]}}\n",
        to_hex(executable_sha),
        to_hex(snapshot_sha),
        to_hex(config_sha),
        to_hex(input_sha),
        RUN_DURATION.as_nanos(),
        counts.frames,
        counts.min_confirmed_tracks,
        counts.max_tracks,
        counts.max_open_alerts,
        counts.closed_alerts,
        counts.measurement_rejections,
        counts.quarantine_records,
        counts.incomplete_ca_frames,
        counts.presentation_updates_total,
        counts.presentation_pending_current,
        counts.presentation_pending_max,
        counts.late_frames,
        counts.max_lateness_ns,
        counts.max_actual_gap_ns,
        counts.max_processing_ns,
        counts.progress_bytes,
        to_hex(output_sha),
    )
}

fn write_interruption(
    output: &Path,
    candidate: &str,
    reason: &str,
    start_utc_ns: u128,
    started: Instant,
    counts: &Counts,
) -> Result<(), String> {
    let now = Instant::now();
    let body = format!(
        "{{\"schema\":\"{SCHEMA}\",\"status\":\"INTERRUPTED_INCOMPLETE\",\"candidate_commit\":\"{candidate}\",\"reason\":\"{}\",\"started_utc_unix_ns\":{start_utc_ns},\"interrupted_utc_unix_ns\":{},\"monotonic_elapsed_ns\":{},\"frames_completed\":{},\"late_frames_over_threshold\":{},\"presentation_pending_current\":{},\"presentation_pending_max\":{},\"presentation_updates_total\":{},\"complete_result_published\":false}}\n",
        clean_reason(reason).replace(['\\', '"'], " "),
        unix_ns()?,
        elapsed_ns(started, now)?,
        counts.frames,
        counts.late_frames,
        counts.presentation_pending_current,
        counts.presentation_pending_max,
        counts.presentation_updates_total,
    );
    write_new(&output.join("INTERRUPTED.json"), body.as_bytes())
}

pub fn run(args: ArgsOs) -> Result<(), String> {
    let args = parse_args(args)?;
    let executable = env::current_exe().map_err(|error| error.to_string())?;
    let executable_sha = hash_file(&executable, MAX_EXECUTABLE_BYTES)?;
    let model = synthetic_model()?;
    let snapshot = oa_airspace::core::snapshot::encode(&model)
        .map_err(|error| format!("synthetic adaptation encode: {error:?}"))?;
    let snapshot_sha = oa_sha256::sha256(&snapshot);
    let input_sha = oa_sha256::sha256(b"D2-SYNTHETIC-POSITIONS-V1;16 fixed addresses;4x4 grid;0.025deg spacing;20000ft;zero horizontal velocity;5s issues");
    let config_sha = hash_config(&args.candidate_commit, &snapshot_sha)?;
    let profile = oa_regional_session::runtime::RegionalRunProfile::paced_d2_soak();
    let mut runtime = regional_new_with_profile(
        model,
        snapshot_sha,
        *b"ZAU1",
        SurveillanceConfig {
            centre_lat_e7: 410_000_000,
            centre_lon_e7: -870_000_000,
            range_nm: 180.0,
        },
        profile,
    )?;

    create_output(&args.output)?;
    let start_utc_ns = unix_ns()?;
    let started = Instant::now();
    write_new(
        &args.output.join("STARTED.json"),
        started_json(
            &args.candidate_commit,
            &executable_sha,
            &snapshot_sha,
            &config_sha,
            &input_sha,
            start_utc_ns,
        )
        .as_bytes(),
    )?;
    let progress_path = args.output.join("progress.tsv");
    let mut progress = create_new_file(&progress_path)?;
    let header = b"tick\tutc_unix_ns\tmonotonic_elapsed_ns\tissue_time_ns\tactual_gap_ns\tschedule_lateness_ns\tprocessing_ns\tfeed_state\ttracks\tca_open\tca_closed\tmeasurement_rejections\tquarantine_records\tpresentation_pending\tpresentation_total\tpixels_sha256\tframe_sha256\n";
    progress
        .write_all(header)
        .and_then(|()| progress.sync_all())
        .map_err(|error| error.to_string())?;

    let mut counts = Counts::default();
    counts.min_confirmed_tracks = usize::MAX;
    let mut progress_hash = Sha256::new();
    progress_hash
        .update(header)
        .map_err(|error| error.to_string())?;
    counts.progress_bytes = u64::try_from(header.len()).map_err(|_| "header length width")?;
    let mut previous_tick: Option<Instant> = None;

    for tick in 0..FRAME_COUNT {
        let due = started
            .checked_add(
                TICK_DURATION
                    .checked_mul(u32::try_from(tick).map_err(|_| "tick schedule width")?)
                    .ok_or("tick schedule overflow")?,
            )
            .ok_or("tick schedule overflow")?;
        let now = Instant::now();
        if due > now {
            std::thread::sleep(due.duration_since(now));
        }
        let actual = Instant::now();
        let lateness = actual
            .checked_duration_since(due)
            .map(|duration| u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX))
            .unwrap_or(0);
        counts.max_lateness_ns = counts.max_lateness_ns.max(lateness);
        if lateness > SCHEDULE_LATE_THRESHOLD_NS {
            counts.late_frames = counts
                .late_frames
                .checked_add(1)
                .ok_or("late-frame count overflow")?;
        }
        let actual_gap = match previous_tick {
            Some(previous) => elapsed_ns(previous, actual)?,
            None => 0,
        };
        counts.max_actual_gap_ns = counts.max_actual_gap_ns.max(actual_gap);
        previous_tick = Some(actual);

        if tick == FEED_FAULT_START_TICK {
            counts.feed_fault_start_utc_ns = Some(unix_ns()?);
            counts.feed_fault_start_elapsed_ns = Some(elapsed_ns(started, actual)?);
        }
        let batch = make_batch(tick)?;
        let frame = match regional_step(&mut runtime, &batch) {
            Ok(frame) => frame,
            Err(error) => {
                write_interruption(
                    &args.output,
                    &args.candidate_commit,
                    &format!("runtime_frame_refused:{error}"),
                    start_utc_ns,
                    started,
                    &counts,
                )?;
                return Err(format!("runtime frame {tick} refused: {error}"));
            }
        };
        let expected_sequence = tick.checked_add(1).ok_or("frame sequence overflow")?;
        if frame.sequence != expected_sequence {
            write_interruption(
                &args.output,
                &args.candidate_commit,
                "runtime_frame_sequence_mismatch",
                start_utc_ns,
                started,
                &counts,
            )?;
            return Err("regional runtime returned an unexpected frame sequence".into());
        }
        if !frame.surveillance.raster_rendered {
            write_interruption(
                &args.output,
                &args.candidate_commit,
                "base_raster_missing",
                start_utc_ns,
                started,
                &counts,
            )?;
            return Err("paced D2 profile did not render its base frame".into());
        }
        let pixels = match regional_pixels(&runtime) {
            Ok(bytes) => bytes,
            Err(error) => {
                write_interruption(
                    &args.output,
                    &args.candidate_commit,
                    &format!("regional_pixels_unavailable:{error}"),
                    start_utc_ns,
                    started,
                    &counts,
                )?;
                return Err(format!(
                    "rendered output unavailable at frame {tick}: {error}"
                ));
            }
        };
        let pixels_sha = oa_sha256::sha256(pixels);
        let processing_ns = elapsed_ns(actual, Instant::now())?;
        counts.max_processing_ns = counts.max_processing_ns.max(processing_ns);
        let frame_sha = frame.surveillance.frame_sha256;
        let tracks = frame.surveillance.tracks.len();
        if tick == FEED_FAULT_START_TICK + FEED_FAULT_TICKS && tracks == AIRCRAFT_COUNT {
            counts.feed_recovered_elapsed_ns = Some(elapsed_ns(started, actual)?);
        }
        let (presentation_pending, presentation_total) = regional_presentation_counts(&runtime);
        if presentation_pending != 1 || presentation_total != frame.sequence {
            write_interruption(
                &args.output,
                &args.candidate_commit,
                "presentation_stream_sequence_mismatch",
                start_utc_ns,
                started,
                &counts,
            )?;
            return Err("presentation stream did not produce exactly one current frame".into());
        }
        counts.presentation_pending_max = counts.presentation_pending_max.max(presentation_pending);
        counts.presentation_pending_current = presentation_pending;
        if presentation_total < counts.presentation_updates_total {
            write_interruption(
                &args.output,
                &args.candidate_commit,
                "presentation_total_regressed",
                start_utc_ns,
                started,
                &counts,
            )?;
            return Err("cumulative presentation count regressed".into());
        }
        counts.presentation_updates_total = presentation_total;
        if tick >= 2 {
            counts.min_confirmed_tracks = counts.min_confirmed_tracks.min(tracks);
        }
        counts.max_tracks = counts.max_tracks.max(tracks);
        counts.max_open_alerts = counts.max_open_alerts.max(frame.surveillance.ca_open.len());
        counts.closed_alerts = counts
            .closed_alerts
            .checked_add(
                u64::try_from(frame.surveillance.ca_closed.len())
                    .map_err(|_| "alert count width")?,
            )
            .ok_or("closed-alert count overflow")?;
        counts.measurement_rejections = counts
            .measurement_rejections
            .checked_add(frame.surveillance.measurement_rejections)
            .ok_or("measurement rejection count overflow")?;
        counts.quarantine_records = counts
            .quarantine_records
            .checked_add(
                u64::try_from(frame.quarantine.records.len())
                    .map_err(|_| "quarantine count width")?,
            )
            .ok_or("quarantine count overflow")?;
        if frame.surveillance.ca_work_short != 0 || frame.surveillance.ca_input_short != 0 {
            counts.incomplete_ca_frames = counts
                .incomplete_ca_frames
                .checked_add(1)
                .ok_or("incomplete CA count overflow")?;
        }

        let row = format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            tick,
            unix_ns()?,
            elapsed_ns(started, actual)?,
            batch.receipt_ns,
            actual_gap,
            lateness,
            processing_ns,
            if tick >= FEED_FAULT_START_TICK && tick < FEED_FAULT_START_TICK + FEED_FAULT_TICKS {
                "DOWN"
            } else if tick == FEED_FAULT_START_TICK + FEED_FAULT_TICKS {
                "REACQUIRED"
            } else {
                "UP"
            },
            tracks,
            frame.surveillance.ca_open.len(),
            frame.surveillance.ca_closed.len(),
            frame.surveillance.measurement_rejections,
            frame.quarantine.records.len(),
            presentation_pending,
            presentation_total,
            to_hex(&pixels_sha),
            to_hex(&frame_sha),
        );
        let row_bytes = row.as_bytes();
        counts.progress_bytes = counts
            .progress_bytes
            .checked_add(u64::try_from(row_bytes.len()).map_err(|_| "progress row width")?)
            .ok_or("progress byte count overflow")?;
        if counts.progress_bytes > MAX_PROGRESS_BYTES {
            write_interruption(
                &args.output,
                &args.candidate_commit,
                "progress_log_cap_exceeded",
                start_utc_ns,
                started,
                &counts,
            )?;
            return Err("bounded progress output exceeded 16 MiB".into());
        }
        if let Err(error) = progress.write_all(row_bytes) {
            let reason = format!("progress_sink_refused:{error}");
            write_interruption(
                &args.output,
                &args.candidate_commit,
                &reason,
                start_utc_ns,
                started,
                &counts,
            )?;
            return Err(reason);
        }
        if let Err(error) = progress_hash.update(row_bytes) {
            let reason = format!("progress_digest_refused:{error}");
            write_interruption(
                &args.output,
                &args.candidate_commit,
                &reason,
                start_utc_ns,
                started,
                &counts,
            )?;
            return Err(reason);
        }
        if let Err(error) = regional_ack_presentation(&mut runtime, frame.sequence) {
            write_interruption(
                &args.output,
                &args.candidate_commit,
                &format!("presentation_ack_refused:{error}"),
                start_utc_ns,
                started,
                &counts,
            )?;
            return Err(format!("presentation ACK failed at frame {tick}: {error}"));
        }
        counts.presentation_pending_current = regional_presentation_counts(&runtime).0;
        counts.frames = counts
            .frames
            .checked_add(1)
            .ok_or("completed frame count overflow")?;
        if counts.frames % PROGRESS_SYNC_FRAMES == 0 {
            if let Err(error) = progress.sync_data() {
                let reason = format!("progress_sync_refused:{error}");
                write_interruption(
                    &args.output,
                    &args.candidate_commit,
                    &reason,
                    start_utc_ns,
                    started,
                    &counts,
                )?;
                return Err(reason);
            }
        }
        let stop = match stop_requested(&args.output.join("STOP")) {
            Ok(stop) => stop,
            Err(error) => {
                let reason = format!("stop_request_refused:{error}");
                write_interruption(
                    &args.output,
                    &args.candidate_commit,
                    &reason,
                    start_utc_ns,
                    started,
                    &counts,
                )?;
                return Err(reason);
            }
        };
        if stop {
            if let Err(error) = progress.sync_all() {
                let reason = format!("progress_sync_refused_on_stop:{error}");
                write_interruption(
                    &args.output,
                    &args.candidate_commit,
                    &reason,
                    start_utc_ns,
                    started,
                    &counts,
                )?;
                return Err(reason);
            }
            write_interruption(
                &args.output,
                &args.candidate_commit,
                "operator_stop_request",
                start_utc_ns,
                started,
                &counts,
            )?;
            return Ok(());
        }
    }
    if counts.frames != FRAME_COUNT {
        write_interruption(
            &args.output,
            &args.candidate_commit,
            "completed_frame_count_mismatch",
            start_utc_ns,
            started,
            &counts,
        )?;
        return Err("internal frame count differs from declared profile".into());
    }

    let finish_due = started
        .checked_add(RUN_DURATION)
        .ok_or("run deadline overflow")?;
    let now = Instant::now();
    if finish_due > now {
        std::thread::sleep(finish_due.duration_since(now));
    }
    progress.sync_all().map_err(|error| error.to_string())?;
    let ended_utc_ns = unix_ns()?;
    let elapsed = elapsed_ns(started, Instant::now())?;
    let input_digest = progress_hash.finalize();
    let result = summary_json(
        &args.candidate_commit,
        &executable_sha,
        &snapshot_sha,
        &config_sha,
        &input_sha,
        start_utc_ns,
        ended_utc_ns,
        elapsed,
        &counts,
        &input_digest,
    );
    write_new(&args.output.join("RESULT.json"), result.as_bytes())?;
    let result_sha = to_hex(&oa_sha256::sha256(result.as_bytes()));
    write_new(
        &args.output.join("COMPLETE"),
        format!("{result_sha}\n").as_bytes(),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::{make_batch, AIRCRAFT_COUNT, FEED_FAULT_TICKS, FRAME_COUNT, RUN_DURATION, TICK_NS};
    use oa_regional_session::runtime::RegionalRunProfile;

    // REQ: OA-REG-021
    #[test]
    fn d2_paced_soak_profile_fits_duration_under_existing_bounded_caps() {
        let profile = RegionalRunProfile::paced_d2_soak();
        let recorded = RegionalRunProfile::recorded_peak_rendered();
        assert_eq!(FRAME_COUNT, 48 * 60 * 60 / 5);
        assert_eq!(RUN_DURATION, Duration::from_secs(48 * 60 * 60));
        assert_eq!(profile.surveillance.max_tracks, AIRCRAFT_COUNT);
        let frames = usize::try_from(FRAME_COUNT).unwrap();
        assert_eq!(profile.surveillance.max_frames, frames);
        assert_eq!(profile.surveillance.max_presentation_updates, 8_192);
        assert_eq!(profile.max_total_events, frames * AIRCRAFT_COUNT);
        assert!(profile.surveillance.max_tracks <= recorded.surveillance.max_tracks);
        assert!(profile.surveillance.max_frames <= recorded.surveillance.max_frames);
        assert!(profile.surveillance.max_drain_rows <= recorded.surveillance.max_drain_rows);
        assert!(profile.surveillance.max_open_alerts <= recorded.surveillance.max_open_alerts);
        assert!(profile.surveillance.max_baro_evidence <= recorded.surveillance.max_baro_evidence);
        assert!(
            profile.surveillance.max_candidate_pairs <= recorded.surveillance.max_candidate_pairs
        );
        assert!(
            profile.surveillance.max_presentation_updates
                <= recorded.surveillance.max_presentation_updates
        );
        assert!(profile.max_total_events <= recorded.max_total_events);
        assert!(profile.max_flights <= recorded.max_flights);
        assert!(profile.max_flight_log_entries <= recorded.max_flight_log_entries);
        assert!(
            profile.surveillance.max_ca_work_per_frame
                <= recorded.surveillance.max_ca_work_per_frame
        );
        assert!(profile.surveillance.render_scope);
        let generated_events = usize::try_from(FRAME_COUNT).unwrap() * AIRCRAFT_COUNT
            - usize::try_from(FEED_FAULT_TICKS).unwrap() * AIRCRAFT_COUNT
            + usize::try_from(FEED_FAULT_TICKS).unwrap()
            + 1;
        assert_eq!(generated_events, 552_931);
        assert!(generated_events <= profile.max_total_events);
    }

    // REQ: OA-REG-021
    #[test]
    fn paced_synthetic_batches_preserve_unique_addresses_and_five_second_source_clock() {
        let first = make_batch(0).unwrap();
        let second = make_batch(1).unwrap();
        assert_eq!(first.events.len(), AIRCRAFT_COUNT);
        assert_eq!(second.receipt_ns - first.receipt_ns, TICK_NS);
        let addresses = first
            .events
            .iter()
            .map(|event| event.identity.aircraft_address)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(addresses.len(), AIRCRAFT_COUNT);
        assert!(first.events.iter().all(
            |event| event.source_ns == first.receipt_ns && event.receipt_ns == first.receipt_ns
        ));
        assert!(second
            .events
            .iter()
            .zip(&first.events)
            .all(|(next, prior)| next.sequence > prior.sequence));
    }

    // REQ: OA-REG-021
    #[test]
    fn feed_fault_window_is_explicit_and_reacquisition_restores_all_synthetic_tracks() {
        let down = make_batch(FEED_FAULT_START_TICK).unwrap();
        let down_next = make_batch(FEED_FAULT_START_TICK + 1).unwrap();
        let recovered = make_batch(FEED_FAULT_START_TICK + FEED_FAULT_TICKS).unwrap();
        assert_eq!(down.events.len(), 1);
        assert_eq!(down_next.events.len(), 1);
        assert!(down.events.first().is_some_and(|event| matches!(
            event.kind,
            EventKind::FeedState {
                available: false,
                ..
            }
        )));
        assert!(recovered.events.first().is_some_and(|event| matches!(
            event.kind,
            EventKind::FeedState {
                available: true,
                ..
            }
        )));
        assert_eq!(recovered.events.len(), AIRCRAFT_COUNT + 1);
        assert!(recovered
            .events
            .windows(2)
            .all(|pair| pair[0].sequence < pair[1].sequence));
    }
}
