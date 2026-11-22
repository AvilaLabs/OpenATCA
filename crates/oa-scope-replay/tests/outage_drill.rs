//! Synthetic outage drill from tracker measurements through the scope's safety display.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_airspace::model::{empty_model, Meta};
use oa_ca::{
    alert_log_new, ca_cycle, ca_new, ca_open, event_log_new, Area, CaConfig, CaTrack, Volume,
    AF_COAST, AIRBORNE, F_COAST, F_IDENTITY_UNCONFIRMED, F_UNCERTAIN, MAX_VOLUMES,
};
use oa_display::core::palette;
use oa_display::ownership::{OwnershipPhase, OwnershipStatus};
use oa_display::raster::frame_digest;
use oa_display::safety::{FeedState, FeedStatus};
use oa_display::traffic::{targets_columns, DEFAULT_STYLE, T_COAST};
use oa_geo::aeqd::{frame_new, project};
use oa_safety::inhibit::{inhibits_new, InhibitTable};
use oa_safety::monitor::{
    check_pairs, MonitorFinding, MonitorLimits, MonitorPair, MonitorStatus, COMMON_LOCAL_FRAME,
    HAS_CLAIM, HAS_REFERENCE, VIOLATION_ALTITUDE,
};
use oa_safety::net::{state_report, state_table_new, NetId, NetReport, StateTable, NETS};
use oa_scope_replay::adapt::{row_columns, ColumnBuf};
use oa_scope_replay::frame::{
    replay_build_raw, replay_build_with_overlays, replay_new, replay_raster, Replay, ReplayConfig,
    ReplaySafety,
};
use oa_scope_replay::raw::RawObservationColumns;
use oa_scope_replay::rows::{Callsigns, RowStore, DEFAULT_CONVERT};
use oa_scope_replay::store::{store_add, store_finish};
use oa_track::{
    output_new, r95_m_of, tracker_advance, tracker_new, tracker_newest_update_ns, tracker_predict,
    tracker_stats, tracker_update, Measurement, TrackOutput, Tracker, DEFAULT_CONFIG, M_HAS_VEL,
    NONE_I32, NO_IDENT, O_HAS_ALT, O_IDENTITY_UNCONFIRMED, O_REACQUIRED,
};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const S: i64 = 1_000_000_000;
const EPOCH: i64 = 1_700_000_000 * S;
const A: u32 = 0xA00001;
const B: u32 = 0xA00002;
const LAT: i32 = 419_000_000;
const LON: i32 = -879_000_000;
const MAX_WORKER_LINE_BYTES: usize = 1_024;
const WORKER_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_LAUNCHER_PATH_BYTES: usize = 4_096;
const MAX_LAUNCHER_ARGS: usize = 16;
const MAX_LAUNCHER_ARG_BYTES: usize = 1_024;
const RECOVERY_MAGIC: &[u8; 8] = b"OARCP002";
const RECOVERY_HEADER_BYTES: usize = 8 + 4 + 32 + 4 + 4 + 8 + 8 + 8;
const RECOVERY_RECORD_BYTES: usize = 4 + 1;
const QUARANTINE_MAGIC: &[u8; 8] = b"OAPQ0001";
const QUARANTINE_BODY_BYTES: usize = 8 + 4 + 4 + 8 + 8 + 32;
const QUARANTINE_RECORD_BYTES: usize = QUARANTINE_BODY_BYTES + 32;
const RECOVERY_INITIAL_GENERATION: u64 = 6;
const RECOVERY_GENERATION: u64 = 7;
const RECOVERY_PUBLISHER: u32 = 0x4F415031;
const RECOVERY_SOURCE: u32 = 0x534E5331;
// C2/C7: three five-byte records with OAF1 framing and a 76-byte envelope; 4 KiB is a fixed
// file-read ceiling with ample format headroom, never input-controlled.
const RECOVERY_MAX_BYTES: usize = 4_096;
const RECOVERY_MAX_RECORDS: usize = 3;
const SYNTHETIC_STREAM: [(i32, bool); 15] = [
    (0, true),
    (1, true),
    (2, true),
    (3, true),
    (4, true),
    (5, true),
    (6, true),
    (7, true),
    (8, true),
    (9, true),
    (10, true),
    (40, false),
    (41, true),
    (42, true),
    (43, true),
];
const VISIBLE_CA_STREAM: [(i32, bool); 5] =
    [(0, true), (1, true), (2, true), (20, false), (21, true)];

struct TempCheckpointFiles {
    dir: PathBuf,
}

// A7.2: one cold, test-only fixture value. This models publication order only; it is not a
// production node identity, signed checkpoint, control grant, or lease.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PublisherFence {
    publisher_id: u32,
    source_id: u32,
    generation: u64,
    last_sequence: u64,
    watermark_ns: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
// A7.2: one transient publication value per synthetic input step.
struct Publication {
    publisher_id: u32,
    source_id: u32,
    generation: u64,
    sequence: u64,
    source_time_ns: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FenceRefusal {
    Identity,
    Generation,
    SequenceReplay,
    WatermarkRegression,
    GenerationNotHigher,
}

const INITIAL_FENCE: PublisherFence = PublisherFence {
    publisher_id: RECOVERY_PUBLISHER,
    source_id: RECOVERY_SOURCE,
    generation: RECOVERY_INITIAL_GENERATION,
    last_sequence: 0,
    watermark_ns: EPOCH - S,
};

fn accept_publication(
    fence: PublisherFence,
    publication: Publication,
) -> Result<PublisherFence, FenceRefusal> {
    if publication.publisher_id != fence.publisher_id || publication.source_id != fence.source_id {
        return Err(FenceRefusal::Identity);
    }
    if publication.generation != fence.generation {
        return Err(FenceRefusal::Generation);
    }
    if publication.sequence <= fence.last_sequence {
        return Err(FenceRefusal::SequenceReplay);
    }
    if publication.source_time_ns < fence.watermark_ns {
        return Err(FenceRefusal::WatermarkRegression);
    }
    Ok(PublisherFence {
        last_sequence: publication.sequence,
        watermark_ns: publication.source_time_ns,
        ..fence
    })
}

fn advance_generation(
    fence: PublisherFence,
    publisher_id: u32,
    source_id: u32,
    generation: u64,
) -> Result<PublisherFence, FenceRefusal> {
    if publisher_id != fence.publisher_id || source_id != fence.source_id {
        return Err(FenceRefusal::Identity);
    }
    if generation <= fence.generation {
        return Err(FenceRefusal::GenerationNotHigher);
    }
    Ok(PublisherFence {
        generation,
        ..fence
    })
}

impl TempCheckpointFiles {
    fn new() -> Self {
        let base = std::env::temp_dir();
        let mut created = None;
        for suffix in 0..1_000u32 {
            let candidate = base.join(format!("oa-recovery-drill-{}-{suffix}", std::process::id()));
            match std::fs::create_dir(&candidate) {
                Ok(()) => {
                    created = Some(candidate);
                    break;
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => panic!("cannot create owned checkpoint directory: {error}"),
            }
        }
        Self {
            dir: created.expect("could not reserve unique checkpoint directory"),
        }
    }

    fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.uncreated_path(name);
        write_checkpoint_create_new(&path, bytes);
        path
    }

    fn uncreated_path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }
}

fn write_checkpoint_create_new(path: &Path, bytes: &[u8]) {
    assert!(bytes.len() <= RECOVERY_MAX_BYTES);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .expect("create fixture record without replacing a file");
    file.write_all(bytes).expect("write fixture record");
    file.sync_all().expect("sync fixture record");
}

impl Drop for TempCheckpointFiles {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn conflict_track(addr: u32, lat_deg: f64, lon_deg: f64) -> CaTrack {
    CaTrack {
        lat_deg,
        lon_deg,
        addr,
        vel_east_mps: 0.0,
        vel_north_mps: 0.0,
        r95_m: 20.0,
        alt_ft: 12_000.0,
        vrate_fpm: 0.0,
        flags: 0,
        ground: AIRBORNE,
    }
}

fn ca_active(drill: &Drill) -> String {
    let active = ca_open(&drill.ca)
        .iter()
        .map(|alert| format!("{}:{}:{}", alert.addr_a, alert.addr_b, alert.start_ns))
        .collect::<Vec<_>>()
        .join(",");
    if active.is_empty() {
        "-".to_owned()
    } else {
        active
    }
}

fn ca_active_detail(drill: &Drill) -> String {
    let active = ca_open(&drill.ca)
        .iter()
        .map(|alert| {
            format!(
                "{}:{}:{}:{}:{}:{}:{}:{}",
                alert.addr_a,
                alert.addr_b,
                alert.start_ns,
                alert.end_ns,
                alert.ticks_conflict,
                alert.ticks_current,
                alert.ticks_unevaluated,
                alert.flags
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    if active.is_empty() {
        "-".to_owned()
    } else {
        active
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InputFault {
    None,
    FailBeforeApply,
    CrashAfterApply,
    CrashAfterCheckpoint,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WorkerCommand {
    Step {
        second: i32,
        feed_live: bool,
        ownership: Option<OwnershipStatus>,
    },
    Input {
        key: InputKey,
        second: i32,
        feed_live: bool,
        fault: InputFault,
    },
    QuarantineSkip {
        key: InputKey,
    },
    Stop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct InputKey {
    publisher_id: u32,
    source_id: u32,
    generation: u64,
    sequence: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct QuarantineRecord {
    key: InputKey,
    payload_digest: [u8; 32],
}

fn ownership_input(owner: &str, accepted: &str) -> Option<OwnershipStatus> {
    fn unit(text: &str) -> Option<[u8; 8]> {
        let bytes = text.as_bytes();
        if bytes.is_empty()
            || !bytes
                .iter()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
        {
            return None;
        }
        let mut unit = [0; 8];
        unit.get_mut(..bytes.len())?.copy_from_slice(bytes);
        Some(unit)
    }
    let phase = if accepted == "-" {
        OwnershipPhase::Current
    } else {
        OwnershipPhase::Accepted {
            by: unit(accepted)?,
        }
    };
    Some(OwnershipStatus {
        callsign: *b"SYN901\0\0",
        current_owner: unit(owner)?,
        phase,
    })
}

fn parse_worker_command(line: &str) -> Option<WorkerCommand> {
    let mut fields = line.split_whitespace();
    let command = fields.next()?;
    if command == "STOP" {
        return fields.next().is_none().then_some(WorkerCommand::Stop);
    }
    let parsed = match command {
        "STEP" | "STEP_OWNED" => {
            let second = fields.next()?.parse().ok()?;
            let feed_live = match fields.next()? {
                "LIVE" => true,
                "LOST" => false,
                _ => return None,
            };
            let ownership = if command == "STEP_OWNED" {
                Some(ownership_input(fields.next()?, fields.next()?)?)
            } else {
                None
            };
            WorkerCommand::Step {
                second,
                feed_live,
                ownership,
            }
        }
        "INPUT"
        | "FAIL_BEFORE_APPLY"
        | "CRASH_AFTER_APPLY"
        | "CRASH_AFTER_CHECKPOINT"
        | "QUARANTINE" => {
            let key = InputKey {
                publisher_id: fields.next()?.parse().ok()?,
                source_id: fields.next()?.parse().ok()?,
                generation: fields.next()?.parse().ok()?,
                sequence: fields.next()?.parse().ok()?,
            };
            if command == "QUARANTINE" {
                WorkerCommand::QuarantineSkip { key }
            } else {
                let second = fields.next()?.parse().ok()?;
                let feed_live = match fields.next()? {
                    "LIVE" => true,
                    "LOST" => false,
                    _ => return None,
                };
                WorkerCommand::Input {
                    key,
                    second,
                    feed_live,
                    fault: match command {
                        "FAIL_BEFORE_APPLY" => InputFault::FailBeforeApply,
                        "CRASH_AFTER_APPLY" => InputFault::CrashAfterApply,
                        "CRASH_AFTER_CHECKPOINT" => InputFault::CrashAfterCheckpoint,
                        _ => InputFault::None,
                    },
                }
            }
        }
        _ => return None,
    };
    fields.next().is_none().then_some(parsed)
}

fn source_time_ns(second: i32) -> Option<i64> {
    i64::from(second)
        .checked_mul(S)
        .and_then(|offset| EPOCH.checked_add(offset))
}

fn publication_for_step(
    fence: PublisherFence,
    stream_index: usize,
    second: i32,
) -> Option<Publication> {
    Some(Publication {
        publisher_id: fence.publisher_id,
        source_id: fence.source_id,
        generation: fence.generation,
        sequence: u64::try_from(stream_index).ok()?.checked_add(1)?,
        source_time_ns: source_time_ns(second)?,
    })
}

fn refusal_name(refusal: FenceRefusal) -> &'static str {
    match refusal {
        FenceRefusal::Identity => "identity",
        FenceRefusal::Generation => "generation",
        FenceRefusal::SequenceReplay => "sequence_replay",
        FenceRefusal::WatermarkRegression => "watermark_regression",
        FenceRefusal::GenerationNotHigher => "generation_not_higher",
    }
}

fn synthetic_payload(second: i32, live: bool) -> [u8; RECOVERY_RECORD_BYTES] {
    let mut payload = [0u8; RECOVERY_RECORD_BYTES];
    payload[..4].copy_from_slice(&second.to_le_bytes());
    payload[4] = u8::from(live);
    payload
}

fn input_identity(sequence: u64, second: i32, live: bool) -> QuarantineRecord {
    QuarantineRecord {
        key: InputKey {
            publisher_id: RECOVERY_PUBLISHER,
            source_id: RECOVERY_SOURCE,
            generation: RECOVERY_GENERATION,
            sequence,
        },
        payload_digest: oa_sha256::sha256(&synthetic_payload(second, live)),
    }
}

// Test-only quarantine envelope. Its checksum detects fixture corruption; it is not a signature
// or authority proof. The source/generation/sequence is the key; the digest binds its payload.
fn encode_quarantine(record: QuarantineRecord) -> [u8; QUARANTINE_RECORD_BYTES] {
    let mut bytes = [0u8; QUARANTINE_RECORD_BYTES];
    bytes[..8].copy_from_slice(QUARANTINE_MAGIC);
    bytes[8..12].copy_from_slice(&record.key.publisher_id.to_le_bytes());
    bytes[12..16].copy_from_slice(&record.key.source_id.to_le_bytes());
    bytes[16..24].copy_from_slice(&record.key.generation.to_le_bytes());
    bytes[24..32].copy_from_slice(&record.key.sequence.to_le_bytes());
    bytes[32..64].copy_from_slice(&record.payload_digest);
    let checksum = oa_sha256::sha256(&bytes[..QUARANTINE_BODY_BYTES]);
    bytes[64..].copy_from_slice(&checksum);
    bytes
}

fn decode_quarantine(bytes: &[u8]) -> Result<QuarantineRecord, &'static str> {
    if bytes.len() != QUARANTINE_RECORD_BYTES || bytes.get(..8) != Some(QUARANTINE_MAGIC.as_slice())
    {
        return Err("quarantine_record_shape");
    }
    let expected = oa_sha256::sha256(
        bytes
            .get(..QUARANTINE_BODY_BYTES)
            .ok_or("quarantine_record_shape")?,
    );
    if bytes.get(QUARANTINE_BODY_BYTES..) != Some(expected.as_slice()) {
        return Err("quarantine_record_digest");
    }
    let u32_at = |start| -> Result<u32, &'static str> {
        Ok(u32::from_le_bytes(
            bytes
                .get(start..start + 4)
                .and_then(|slice| slice.try_into().ok())
                .ok_or("quarantine_record_shape")?,
        ))
    };
    let u64_at = |start| -> Result<u64, &'static str> {
        Ok(u64::from_le_bytes(
            bytes
                .get(start..start + 8)
                .and_then(|slice| slice.try_into().ok())
                .ok_or("quarantine_record_shape")?,
        ))
    };
    let payload_digest = bytes
        .get(32..64)
        .and_then(|slice| slice.try_into().ok())
        .ok_or("quarantine_record_shape")?;
    Ok(QuarantineRecord {
        key: InputKey {
            publisher_id: u32_at(8)?,
            source_id: u32_at(12)?,
            generation: u64_at(16)?,
            sequence: u64_at(24)?,
        },
        payload_digest,
    })
}

fn read_quarantine(path: &Path) -> Result<QuarantineRecord, &'static str> {
    let file = oa_sealed::open(path).map_err(|_| "quarantine_open")?;
    let mut bytes = Vec::with_capacity(QUARANTINE_RECORD_BYTES + 1);
    Read::take(file, QUARANTINE_RECORD_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "quarantine_read")?;
    if bytes.len() > QUARANTINE_RECORD_BYTES {
        return Err("quarantine_record_shape");
    }
    decode_quarantine(&bytes)
}

fn validate_quarantine_for_stream(
    record: QuarantineRecord,
    stream: &[(i32, bool)],
) -> Result<(), &'static str> {
    if record.key.publisher_id != RECOVERY_PUBLISHER
        || record.key.source_id != RECOVERY_SOURCE
        || record.key.generation != RECOVERY_GENERATION
    {
        return Err("quarantine_identity");
    }
    let index = record
        .key
        .sequence
        .checked_sub(1)
        .and_then(|sequence| usize::try_from(sequence).ok())
        .ok_or("quarantine_sequence")?;
    let &(second, live) = stream.get(index).ok_or("quarantine_sequence")?;
    if record.payload_digest != oa_sha256::sha256(&synthetic_payload(second, live)) {
        return Err("quarantine_payload_mismatch");
    }
    Ok(())
}

// This test-only envelope stores real OAF1 frames. Its digest covers the complete frame prefix;
// the inner feed framing checks each CRC and source timestamp. The trailing publisher fence is
// fixture metadata, not the NODE_NETWORK signed-checkpoint schema.
fn encode_recovery_checkpoint(
    steps: &[(i32, bool)],
    initial_fence: PublisherFence,
    generation: u64,
) -> Vec<u8> {
    assert!(!steps.is_empty() && steps.len() <= RECOVERY_MAX_RECORDS);
    let mut frames = Vec::new();
    let mut frame = vec![0u8; oa_feed::HEADER_LEN + RECOVERY_RECORD_BYTES + oa_feed::TRAILER_LEN];
    let mut fence = advance_generation(
        initial_fence,
        initial_fence.publisher_id,
        initial_fence.source_id,
        generation,
    )
    .expect("fixture caller explicitly supplied a higher generation");
    for (seq, &(second, live)) in steps.iter().enumerate() {
        let publication = publication_for_step(fence, seq, second).expect("fixture time fits");
        fence = accept_publication(fence, publication).expect("fixture publication advances");
        let mut payload = second.to_le_bytes().to_vec();
        payload.push(u8::from(live));
        let len = oa_feed::encode_frame(
            &mut frame,
            1,
            0,
            seq as u64,
            publication.source_time_ns,
            0,
            &payload,
        )
        .expect("bounded fixture frame fits");
        frames.extend_from_slice(&frame[..len]);
    }
    let mut checkpoint = Vec::with_capacity(RECOVERY_HEADER_BYTES + frames.len());
    checkpoint.extend_from_slice(RECOVERY_MAGIC);
    checkpoint.extend_from_slice(&(steps.len() as u32).to_le_bytes());
    checkpoint.extend_from_slice(&oa_sha256::sha256(&frames));
    checkpoint.extend_from_slice(&fence.publisher_id.to_le_bytes());
    checkpoint.extend_from_slice(&fence.source_id.to_le_bytes());
    checkpoint.extend_from_slice(&fence.generation.to_le_bytes());
    checkpoint.extend_from_slice(&fence.last_sequence.to_le_bytes());
    checkpoint.extend_from_slice(&fence.watermark_ns.to_le_bytes());
    checkpoint.extend_from_slice(&frames);
    assert!(checkpoint.len() <= RECOVERY_MAX_BYTES);
    checkpoint
}

// A7.2: one cold decoded fixture checkpoint per worker startup.
struct RecoveryCheckpoint {
    steps: Vec<(i32, bool)>,
    fence: PublisherFence,
}

fn decode_recovery_checkpoint(
    bytes: &[u8],
    initial_fence: PublisherFence,
) -> Result<RecoveryCheckpoint, &'static str> {
    if bytes.len() > RECOVERY_MAX_BYTES || bytes.len() < RECOVERY_HEADER_BYTES {
        return Err("checkpoint_size_or_header");
    }
    if bytes.get(..8) != Some(RECOVERY_MAGIC.as_slice()) {
        return Err("checkpoint_magic");
    }
    let count_bytes: [u8; 4] = bytes
        .get(8..12)
        .and_then(|slice| slice.try_into().ok())
        .ok_or("checkpoint_header")?;
    let count = u32::from_le_bytes(count_bytes) as usize;
    if count == 0 || count > RECOVERY_MAX_RECORDS {
        return Err("checkpoint_record_bound");
    }
    let expected: [u8; 32] = bytes
        .get(12..44)
        .and_then(|slice| slice.try_into().ok())
        .ok_or("checkpoint_header")?;
    let stored_fence = PublisherFence {
        publisher_id: u32::from_le_bytes(
            bytes
                .get(44..48)
                .and_then(|slice| slice.try_into().ok())
                .ok_or("checkpoint_header")?,
        ),
        source_id: u32::from_le_bytes(
            bytes
                .get(48..52)
                .and_then(|slice| slice.try_into().ok())
                .ok_or("checkpoint_header")?,
        ),
        generation: u64::from_le_bytes(
            bytes
                .get(52..60)
                .and_then(|slice| slice.try_into().ok())
                .ok_or("checkpoint_header")?,
        ),
        last_sequence: u64::from_le_bytes(
            bytes
                .get(60..68)
                .and_then(|slice| slice.try_into().ok())
                .ok_or("checkpoint_header")?,
        ),
        watermark_ns: i64::from_le_bytes(
            bytes
                .get(68..76)
                .and_then(|slice| slice.try_into().ok())
                .ok_or("checkpoint_header")?,
        ),
    };
    if stored_fence.publisher_id != initial_fence.publisher_id
        || stored_fence.source_id != initial_fence.source_id
    {
        return Err("checkpoint_identity");
    }
    let mut fence = if stored_fence.generation == initial_fence.generation {
        initial_fence
    } else {
        advance_generation(
            initial_fence,
            stored_fence.publisher_id,
            stored_fence.source_id,
            stored_fence.generation,
        )
        .map_err(refusal_name)?
    };
    let frames = bytes
        .get(RECOVERY_HEADER_BYTES..)
        .ok_or("checkpoint_header")?;
    if oa_sha256::sha256(frames) != expected {
        return Err("checkpoint_digest");
    }
    let frame_bytes = oa_feed::HEADER_LEN + RECOVERY_RECORD_BYTES + oa_feed::TRAILER_LEN;
    if frames.len() != count.saturating_mul(frame_bytes) {
        return Err("checkpoint_frame_bytes");
    }
    let mut input = vec![0u8; oa_feed::MAX_FRAME_LEN];
    let mut reader = oa_feed::FrameReader::new(&mut input).map_err(|_| "reader_capacity")?;
    if reader.push(frames) != frames.len() {
        return Err("checkpoint_frame_bound");
    }
    let mut steps = Vec::with_capacity(count);
    loop {
        let next = reader.next();
        let Some(frame) = next else { break };
        let frame = frame.map_err(|_| "checkpoint_frame_invalid")?;
        if frame.kind != oa_feed::Kind::Message
            || frame.flags != 0
            || frame.payload.len() != RECOVERY_RECORD_BYTES
        {
            return Err("checkpoint_record_shape");
        }
        let seq = steps.len() as u64;
        let expected_payload: [u8; 4] = frame.payload[..4]
            .try_into()
            .map_err(|_| "checkpoint_record_shape")?;
        let second = i32::from_le_bytes(expected_payload);
        let live = match frame.payload[4] {
            0 => false,
            1 => true,
            _ => return Err("checkpoint_record_shape"),
        };
        let expected_source_ns = source_time_ns(second).ok_or("checkpoint_source_time_overflow")?;
        if frame.seq != seq || frame.recv_unix_ns != expected_source_ns {
            return Err("checkpoint_sequence_or_time");
        }
        let publication = Publication {
            publisher_id: stored_fence.publisher_id,
            source_id: stored_fence.source_id,
            generation: stored_fence.generation,
            sequence: frame
                .seq
                .checked_add(1)
                .ok_or("checkpoint_sequence_overflow")?,
            source_time_ns: frame.recv_unix_ns,
        };
        fence = accept_publication(fence, publication).map_err(refusal_name)?;
        steps.push((second, live));
        if steps.len() > count {
            return Err("checkpoint_record_count");
        }
    }
    if reader.finish().is_some() || steps.len() != count {
        return Err("checkpoint_truncated_or_count");
    }
    if fence != stored_fence {
        return Err("checkpoint_fence_mismatch");
    }
    Ok(RecoveryCheckpoint { steps, fence })
}

fn read_recovery_checkpoint(path: &Path) -> Result<Vec<u8>, &'static str> {
    let file = oa_sealed::open(path).map_err(|_| "checkpoint_open")?;
    let mut bytes = Vec::with_capacity(RECOVERY_MAX_BYTES);
    Read::take(file, RECOVERY_MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "checkpoint_read")?;
    if bytes.len() > RECOVERY_MAX_BYTES {
        return Err("checkpoint_size_or_header");
    }
    Ok(bytes)
}

fn exercise_fixture_fence(fence: PublisherFence) {
    let later_time = fence
        .watermark_ns
        .checked_add(S)
        .expect("small synthetic watermark has room");
    let next_sequence = fence
        .last_sequence
        .checked_add(1)
        .expect("small synthetic sequence has room");
    let other_publisher = fence
        .publisher_id
        .checked_add(1)
        .expect("synthetic publisher id has room");
    let old_generation = fence
        .generation
        .checked_sub(1)
        .expect("configured generation is above the fixture floor");
    let next_generation = fence
        .generation
        .checked_add(1)
        .expect("synthetic generation has room");
    assert_eq!(
        accept_publication(
            fence,
            Publication {
                publisher_id: other_publisher,
                source_id: fence.source_id,
                generation: fence.generation,
                sequence: next_sequence,
                source_time_ns: later_time,
            }
        ),
        Err(FenceRefusal::Identity)
    );
    assert_eq!(
        accept_publication(
            fence,
            Publication {
                publisher_id: fence.publisher_id,
                source_id: fence.source_id,
                generation: old_generation,
                sequence: next_sequence,
                source_time_ns: later_time,
            }
        ),
        Err(FenceRefusal::Generation),
        "newer source time cannot revive an older generation"
    );
    assert_eq!(
        accept_publication(
            fence,
            Publication {
                publisher_id: fence.publisher_id,
                source_id: fence.source_id,
                generation: fence.generation,
                sequence: fence.last_sequence,
                source_time_ns: later_time,
            }
        ),
        Err(FenceRefusal::SequenceReplay),
        "same-generation duplicate sequence is refused"
    );
    let advanced = advance_generation(fence, fence.publisher_id, fence.source_id, next_generation)
        .expect("caller explicitly activated higher fixture generation");
    assert_eq!(advanced.generation, next_generation);
    assert!(accept_publication(
        advanced,
        Publication {
            publisher_id: advanced.publisher_id,
            source_id: advanced.source_id,
            generation: advanced.generation,
            sequence: next_sequence,
            source_time_ns: later_time,
        }
    )
    .is_ok());
}

fn validate_worker_ack(line: &str, expected_second: i32) {
    let fields: Vec<_> = line.split_whitespace().collect();
    assert_eq!(fields.len(), 8, "malformed worker ack: {line}");
    assert_eq!(
        fields[0], "MACHINE_LOSS_ACK",
        "malformed worker ack: {line}"
    );
    assert_eq!(
        fields[1].parse::<i32>().ok(),
        Some(expected_second),
        "ack step mismatch: {line}"
    );
    assert_eq!(fields[2].len(), 64, "invalid frame digest: {line}");
    assert!(
        fields[2].bytes().all(|byte| byte.is_ascii_hexdigit()),
        "invalid frame digest: {line}"
    );
    assert_eq!(
        fields[3],
        format!("{},{}", A, B),
        "identity set mismatch: {line}"
    );
    assert!(
        fields[4].parse::<usize>().is_ok(),
        "invalid warning count: {line}"
    );
    assert!(
        fields[5].parse::<usize>().is_ok(),
        "invalid conflict count: {line}"
    );
    assert!(
        !fields[6].is_empty() && !fields[7].is_empty(),
        "empty CA event state: {line}"
    );
}

fn validate_input_ack(line: &str, identity: QuarantineRecord, second: i32) -> String {
    let fields: Vec<_> = line.split_whitespace().collect();
    assert_eq!(fields.len(), 14, "malformed identity-bound ack: {line}");
    assert_eq!(fields[0], "MACHINE_LOSS_INPUT_ACK", "{line}");
    assert_eq!(
        fields[1].parse::<u32>().ok(),
        Some(identity.key.publisher_id)
    );
    assert_eq!(fields[2].parse::<u32>().ok(), Some(identity.key.source_id));
    assert_eq!(fields[3].parse::<u64>().ok(), Some(identity.key.generation));
    assert_eq!(fields[4].parse::<u64>().ok(), Some(identity.key.sequence));
    assert_eq!(fields[5], oa_sha256::to_hex(&identity.payload_digest));
    let ack = fields[6..].join(" ");
    validate_worker_ack(&ack, second);
    ack
}

fn assert_ca_ack_state(line: &str, second: i32, transition: &str, active: &str) {
    let fields: Vec<_> = line.split_whitespace().collect();
    assert_eq!(fields[1].parse::<i32>().ok(), Some(second));
    assert_eq!(fields[6], transition, "CA transition at {second}s: {line}");
    assert!(
        fields[7].starts_with(active),
        "active CA identity at {second}s: {line}"
    );
}

struct Drill {
    tracker: Tracker,
    out: TrackOutput,
    scope: Replay,
    states: StateTable,
    inhibits: Box<InhibitTable>,
    feeds: [FeedStatus; 2],
    ca: oa_ca::CaState,
    ca_alerts: oa_ca::AlertLog,
    ca_events: oa_ca::EventLog,
}

impl Drill {
    fn new() -> Self {
        let tracker = tracker_new(&oa_track::TrackerConfig {
            max_tracks: 8,
            out_capacity: 256,
            log_capacity: 16,
            confirm_pool: 8,
            ..DEFAULT_CONFIG
        })
        .unwrap();
        let model = empty_model(Meta {
            nasr_effective: 0,
            cifp_cycle: 0,
            artcc: "ZAU".into(),
            buffer_m: 0,
            nasr_sha256: [0; 32],
            cifp_sha256: [0; 32],
        });
        let scope = replay_new(
            model,
            &ReplayConfig {
                centre_lat_e7: LAT,
                centre_lon_e7: LON,
                range_nm: 30.0,
                width: 640,
                height: 480,
                max_targets: 8,
                style: DEFAULT_STYLE,
                convert: DEFAULT_CONVERT,
            },
        )
        .unwrap();
        let volume = Volume {
            area: Area::Box {
                lat_min_e7: -900_000_000,
                lat_max_e7: 900_000_000,
                lon_min_e7: -1_800_000_000,
                lon_max_e7: 1_800_000_000,
            },
            alt_lo_ft: 0,
            alt_hi_ft: 60_000,
            h_m: 1_300.0,
            v_ft: 300.0,
        };
        // C2/C7: four input tracks have at most six pairs; all six fit and are evaluated each
        // cycle. The two alert logs fit the two fixture transitions with headroom.
        let ca_cfg = CaConfig {
            max_tracks: 4,
            max_open: 6,
            max_candidates: 6,
            look_ahead_ns: 120 * S,
            hysteresis_ns: 3 * S,
            work_budget: 10_000,
            eval_budget: 6,
            max_speed_mps: 700.0,
            r95_cap_m: 20_000.0,
            grid_band_m: 10_000.0,
            default_volume: volume,
            volumes: [volume; MAX_VOLUMES],
            n_volumes: 0,
        };
        Self {
            tracker,
            out: output_new(256),
            scope,
            states: state_table_new(10 * S, 0).unwrap(),
            inhibits: Box::new(inhibits_new()),
            feeds: [
                FeedStatus {
                    name: *b"ADSB\0\0\0\0",
                    state: FeedState::Live,
                },
                FeedStatus {
                    name: *b"RADAR\0\0\0",
                    state: FeedState::Live,
                },
            ],
            ca: ca_new(&ca_cfg).unwrap(),
            ca_alerts: alert_log_new(16).unwrap(),
            ca_events: event_log_new(16).unwrap(),
        }
    }

    fn conflict_step(&mut self, second: i32) -> String {
        let timestamp = EPOCH + i64::from(second) * S;
        let early_conflict = second <= 8;
        let late_conflict = second >= 41;
        let rows = [
            conflict_track(0xD10001, 41.90, -87.90),
            conflict_track(
                0xD10002,
                41.90,
                if early_conflict { -87.895 } else { -87.87 },
            ),
            conflict_track(0xD20001, 41.75, -87.80),
            conflict_track(
                0xD20002,
                41.75,
                if late_conflict { -87.795 } else { -87.77 },
            ),
        ];
        let alert_count = self.ca_alerts.items.len();
        let out = ca_cycle(
            &mut self.ca,
            timestamp,
            &rows,
            &mut self.ca_alerts,
            &mut self.ca_events,
            &mut self.states,
        );
        assert_eq!(out.work_short, 0, "CA work budget exhausted at {second}s");
        assert_eq!(out.input_short, 0, "CA input was lost at {second}s");
        assert_eq!(
            self.ca_alerts.overflow, 0,
            "CA alert log overflowed at {second}s"
        );
        let mut transitions = Vec::new();
        if out.stats.opened > 0 {
            for alert in ca_open(&self.ca)
                .iter()
                .filter(|alert| alert.start_ns == timestamp)
            {
                transitions.push(format!(
                    "OPEN:{}:{}:{}",
                    alert.addr_a, alert.addr_b, alert.start_ns
                ));
            }
        }
        for alert in self.ca_alerts.items.iter().skip(alert_count) {
            transitions.push(format!(
                "CLOSE:{}:{}:{}",
                alert.addr_a, alert.addr_b, alert.start_ns
            ));
        }
        if transitions.is_empty() {
            "-".to_owned()
        } else {
            transitions.join(",")
        }
    }

    fn warmup(&mut self) {
        for second in [-3, -2, -1] {
            self.offer(A, 1, second);
            self.offer(B, 2, second);
        }
    }

    fn warmup_visible_ca(&mut self) {
        for second in [-3, -2, -1] {
            self.offer_with_vrate(A, 1, second, 0);
            self.offer_with_vrate(B, 2, second, 0);
        }
    }

    fn process(&mut self, second: i32, live: bool) -> String {
        self.process_with_ownership(second, live, None)
    }

    fn process_with_ownership(
        &mut self,
        second: i32,
        live: bool,
        ownership: Option<&OwnershipStatus>,
    ) -> String {
        let ca_transitions = self.conflict_step(second);
        if live {
            self.offer(A, 1, second);
            self.offer(B, 2, second);
            self.feeds[0].state = FeedState::Live;
            self.feeds[1].state = FeedState::Live;
        } else {
            let elapsed_s = second.saturating_sub(10).max(0);
            let elapsed_ms = u32::try_from(elapsed_s)
                .unwrap_or(u32::MAX)
                .saturating_mul(1_000);
            self.feeds[0].state = FeedState::Lost { age_ms: elapsed_ms };
            self.feeds[1].state = FeedState::Lost { age_ms: elapsed_ms };
        }
        self.frame_with_ownership(i64::from(second), None, ownership);
        let digest = oa_sha256::to_hex(&frame_digest(&self.scope.spec, &self.scope.buf).unwrap());
        let ids = targets_columns(&self.scope.set)
            .addr
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let warning_count = warning_count(&self.scope);
        let conflict_count = targets_columns(&self.scope.set)
            .flags
            .iter()
            .filter(|&&flags| flags & oa_display::traffic::T_CS_CONFLICT != 0)
            .count();
        let active = ca_active(self);
        format!("MACHINE_LOSS_ACK {second} {digest} {ids} {warning_count} {conflict_count} {ca_transitions} {active}")
    }

    fn visible_ca_rows(&self, at_ns: i64) -> Vec<CaTrack> {
        let targets = targets_columns(&self.scope.set);
        let mut rows = Vec::with_capacity(targets.addr.len());
        for (index, &addr) in targets.addr.iter().enumerate() {
            let (row, confirmed, newest_ns) = tracker_predict(&self.tracker, addr, at_ns)
                .expect("every displayed identity has a predicted tracker row");
            assert!(confirmed, "scope target {addr} is not a confirmed track");
            assert_eq!(row.addr, addr);
            assert!(newest_ns <= at_ns, "track {addr} comes from the future");
            assert_eq!(targets.lat_e7[index], row.lat_e7);
            assert_eq!(targets.lon_e7[index], row.lon_e7);

            let mut flags = 0;
            // Prediction provides geometry and motion; the emitted scope row carries coast
            // and uncertainty status from the display tick at the same source time.
            if targets.flags[index] & T_COAST != 0 {
                flags |= F_COAST;
                assert!(
                    newest_ns < at_ns,
                    "coasting track {addr} has a fresh update"
                );
            }
            if targets.flags[index] & oa_display::traffic::T_UNCERTAIN != 0 {
                flags |= F_UNCERTAIN;
            }
            if row.flags & O_IDENTITY_UNCONFIRMED != 0 {
                flags |= F_IDENTITY_UNCONFIRMED;
            }
            let altitude = if row.flags & O_HAS_ALT != 0 {
                assert_ne!(targets.alt_ft[index], oa_display::traffic::NO_ALT);
                assert!(row.alt_ft.is_finite() && row.vrate_fpm.is_finite());
                row.alt_ft
            } else {
                assert_eq!(targets.alt_ft[index], oa_display::traffic::NO_ALT);
                f32::NAN
            };
            let r95_m = r95_m_of(&row.pos_cov_m2);
            assert!(r95_m.is_finite() && r95_m >= 0.0);
            assert!(row.vel_east_mps.is_finite() && row.vel_north_mps.is_finite());
            rows.push(CaTrack {
                lat_deg: f64::from(row.lat_e7) / 10_000_000.0,
                lon_deg: f64::from(row.lon_e7) / 10_000_000.0,
                addr,
                vel_east_mps: row.vel_east_mps,
                vel_north_mps: row.vel_north_mps,
                r95_m: r95_m as f32,
                alt_ft: altitude,
                vrate_fpm: if row.flags & O_HAS_ALT != 0 {
                    row.vrate_fpm
                } else {
                    f32::NAN
                },
                flags,
                ground: AIRBORNE,
            });
        }
        assert_eq!(rows.len(), targets.addr.len());
        assert_eq!(
            rows.len(),
            2,
            "fixture CA pair must use both visible tracks"
        );
        rows
    }

    fn process_visible_ca(&mut self, second: i32, live: bool) -> String {
        if live {
            self.offer_with_vrate(A, 1, second, 0);
            self.offer_with_vrate(B, 2, second, 0);
            self.feeds[0].state = FeedState::Live;
            self.feeds[1].state = FeedState::Live;
        } else {
            let elapsed_s = second.saturating_sub(2).max(0);
            let elapsed_ms = u32::try_from(elapsed_s)
                .unwrap_or(u32::MAX)
                .saturating_mul(1_000);
            self.feeds[0].state = FeedState::Lost { age_ms: elapsed_ms };
            self.feeds[1].state = FeedState::Lost { age_ms: elapsed_ms };
        }
        let at_ns = EPOCH + i64::from(second) * S;
        self.frame_inner(i64::from(second), None);
        assert_eq!(
            self.out.overflow, 0,
            "tracker output overflowed at {second}s"
        );
        let rows = self.visible_ca_rows(at_ns);
        if !live {
            assert!(rows.iter().all(|row| row.flags & F_COAST != 0));
        }
        let alert_count = self.ca_alerts.items.len();
        let out = ca_cycle(
            &mut self.ca,
            at_ns,
            &rows,
            &mut self.ca_alerts,
            &mut self.ca_events,
            &mut self.states,
        );
        assert_eq!(out.work_short, 0, "CA work budget exhausted at {second}s");
        assert_eq!(out.input_short, 0, "CA input was lost at {second}s");
        assert_eq!(
            self.ca_alerts.overflow, 0,
            "CA alert log overflowed at {second}s"
        );
        if !live {
            let alerts = ca_open(&self.ca);
            assert_eq!(alerts.len(), 1, "source-silent CA alert ended at {second}s");
            assert_eq!((alerts[0].addr_a, alerts[0].addr_b), (A, B));
            assert_ne!(alerts[0].flags & AF_COAST, 0);
        }
        let mut transitions = Vec::new();
        if out.stats.opened > 0 {
            transitions.extend(
                ca_open(&self.ca)
                    .iter()
                    .filter(|alert| alert.start_ns == at_ns)
                    .map(|alert| {
                        format!("OPEN:{}:{}:{}", alert.addr_a, alert.addr_b, alert.start_ns)
                    }),
            );
        }
        for alert in self.ca_alerts.items.iter().skip(alert_count) {
            transitions.push(format!(
                "CLOSE:{}:{}:{}",
                alert.addr_a, alert.addr_b, alert.start_ns
            ));
        }
        let ca_transitions = if transitions.is_empty() {
            "-".to_owned()
        } else {
            transitions.join(",")
        };
        let digest = oa_sha256::to_hex(&frame_digest(&self.scope.spec, &self.scope.buf).unwrap());
        let ids = targets_columns(&self.scope.set)
            .addr
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let warnings = warning_count(&self.scope);
        let conflict_count = targets_columns(&self.scope.set)
            .flags
            .iter()
            .filter(|&&flags| flags & oa_display::traffic::T_CS_CONFLICT != 0)
            .count();
        let active = ca_active_detail(self);
        format!("MACHINE_LOSS_ACK {second} {digest} {ids} {warnings} {conflict_count} {ca_transitions} {active}")
    }

    fn offer(&mut self, addr: u32, sensor: u16, second: i32) {
        self.offer_with_vrate(addr, sensor, second, NONE_I32);
    }

    fn offer_with_vrate(&mut self, addr: u32, sensor: u16, second: i32, vrate_fpm: i32) {
        tracker_update(
            &mut self.tracker,
            &Measurement {
                toa_ns: EPOCH + i64::from(second) * S,
                pos_cov_m2: [100.0, 0.0, 100.0],
                vel_cov_m2s2: [4.0, 0.0, 4.0],
                vel_east_mps: 83.0,
                vel_north_mps: 0.0,
                lat_e7: LAT + i32::from(sensor) * 80_000,
                lon_e7: LON + second * 10_000,
                alt_ft: 12_000,
                vrate_fpm,
                addr,
                alt_quant_ft: 25,
                sensor,
                ident: NO_IDENT,
                flags: M_HAS_VEL,
                category: 0,
                vel_lag_s: 0.0,
            },
            &mut self.out,
        );
    }

    fn frame(&mut self, second: i64, expected_warnings: &[&str]) {
        self.frame_inner(second, Some(expected_warnings));
    }

    fn frame_inner(&mut self, second: i64, expected_warnings: Option<&[&str]>) {
        self.frame_with_ownership(second, expected_warnings, None);
    }

    fn frame_with_ownership(
        &mut self,
        second: i64,
        expected_warnings: Option<&[&str]>,
        ownership: Option<&OwnershipStatus>,
    ) {
        let at = EPOCH + second * S;
        tracker_advance(&mut self.tracker, at, &mut self.out);
        let mut store = RowStore::default();
        let stats = store_add(
            &mut store,
            &row_columns(&self.out, &mut ColumnBuf::default()),
            i64::MIN,
        )
        .unwrap();
        assert_eq!(stats.kept, stats.rows_in);
        assert_eq!(store_finish(&mut store), 0);
        let newest = tracker_newest_update_ns(&self.tracker);
        let info = replay_build_with_overlays(
            &mut self.scope,
            &store,
            &Callsigns::default(),
            at,
            "OUTAGE DRILL",
            Some(&ReplaySafety {
                states: &self.states,
                inhibits: &self.inhibits,
                feeds: &self.feeds,
                newest_track_ns: (newest != i64::MIN).then_some(newest),
                frozen_limit_ns: 5 * S,
            }),
            ownership,
        )
        .unwrap();
        assert!(!info.ownership_clipped);
        replay_raster(&mut self.scope).unwrap();
        let list = &self.scope.list;
        let warnings: Vec<_> = list
            .texts
            .start
            .iter()
            .zip(&list.texts.len)
            .map(|(&start, &len)| &list.text[start as usize..start as usize + len as usize])
            .filter(|s| {
                s.starts_with("FEED ") || s.starts_with("PICTURE ") || s.starts_with("MSAW ")
            })
            .collect();
        if let Some(expected) = expected_warnings {
            assert_eq!(warnings, expected, "unexpected warnings at {second}s");
        }
        assert_eq!(targets_columns(&self.scope.set).addr, &[A, B]);
    }
}

// Child-process protocol for the machine-loss drill. Each child runs this real tracker + scope
// composition and accepts exactly the same synthetic seconds in the same order.
#[test]
fn machine_loss_worker_process() {
    if std::env::var_os("OA_MACHINE_LOSS_WORKER").is_none() {
        return;
    }
    let stdin = std::io::stdin();
    let mut input = BufReader::new(stdin.lock());
    let mut line_bytes = Vec::with_capacity(MAX_WORKER_LINE_BYTES);
    let mut drill = Drill::new();
    let visible_ca = std::env::var_os("OA_MACHINE_LOSS_VISIBLE_CA").is_some();
    if visible_ca {
        drill.warmup_visible_ca();
    } else {
        drill.warmup();
    }
    let stream = if visible_ca {
        &VISIBLE_CA_STREAM[..]
    } else {
        &SYNTHETIC_STREAM[..]
    };
    let mut step_no = 0usize;
    let mut publisher_fence = advance_generation(
        INITIAL_FENCE,
        RECOVERY_PUBLISHER,
        RECOVERY_SOURCE,
        RECOVERY_GENERATION,
    )
    .expect("fixture explicitly activates its configured generation");
    if let Some(path) = std::env::var_os("OA_MACHINE_LOSS_CHECKPOINT") {
        let path = PathBuf::from(path);
        let checkpoint = match read_recovery_checkpoint(&path)
            .and_then(|data| decode_recovery_checkpoint(&data, INITIAL_FENCE))
        {
            Ok(checkpoint) => checkpoint,
            Err(reason) => {
                println!("MACHINE_LOSS_ERROR checkpoint_refused {reason}");
                let _ = std::io::stdout().flush();
                return;
            }
        };
        let retained_fence = checkpoint.fence;
        for (index, &(second, live)) in checkpoint.steps.iter().enumerate() {
            if stream.get(step_no) != Some(&(second, live)) {
                println!("MACHINE_LOSS_ERROR checkpoint_prefix_mismatch");
                let _ = std::io::stdout().flush();
                return;
            }
            let Some(publication) = publication_for_step(publisher_fence, index, second) else {
                println!("MACHINE_LOSS_ERROR checkpoint_source_time_overflow");
                let _ = std::io::stdout().flush();
                return;
            };
            publisher_fence = match accept_publication(publisher_fence, publication) {
                Ok(next) => next,
                Err(refusal) => {
                    println!(
                        "MACHINE_LOSS_ERROR checkpoint_fence_{}",
                        refusal_name(refusal)
                    );
                    let _ = std::io::stdout().flush();
                    return;
                }
            };
            step_no += 1;
            let ack = if visible_ca {
                drill.process_visible_ca(second, live)
            } else {
                drill.process(second, live)
            };
            println!("{ack}");
            let _ = std::io::stdout().flush();
        }
        if publisher_fence != retained_fence {
            println!("MACHINE_LOSS_ERROR checkpoint_fence_not_retained");
            let _ = std::io::stdout().flush();
            return;
        }
        exercise_fixture_fence(publisher_fence);
    }
    loop {
        let line = match read_bounded_line(&mut input, &mut line_bytes) {
            Ok(Some(line)) => line,
            Ok(None) => break,
            Err(_) => {
                println!("MACHINE_LOSS_ERROR invalid_worker_command");
                let _ = std::io::stdout().flush();
                break;
            }
        };
        let Some(command) = parse_worker_command(&line) else {
            println!("MACHINE_LOSS_ERROR invalid_worker_command");
            let _ = std::io::stdout().flush();
            break;
        };
        match command {
            WorkerCommand::Stop if step_no == stream.len() => break,
            WorkerCommand::Stop => {
                println!("MACHINE_LOSS_ERROR premature_stop");
                let _ = std::io::stdout().flush();
                break;
            }
            WorkerCommand::QuarantineSkip { key } => {
                let Some(&(second, live)) = stream.get(step_no) else {
                    println!("MACHINE_LOSS_ERROR quarantine_skip_without_record");
                    let _ = std::io::stdout().flush();
                    break;
                };
                let Some(expected) = publication_for_step(publisher_fence, step_no, second) else {
                    println!("MACHINE_LOSS_ERROR source_time_overflow");
                    let _ = std::io::stdout().flush();
                    break;
                };
                if key.publisher_id != expected.publisher_id
                    || key.source_id != expected.source_id
                    || key.generation != expected.generation
                    || key.sequence != expected.sequence
                {
                    println!("MACHINE_LOSS_ERROR quarantine_identity_or_order_mismatch");
                    let _ = std::io::stdout().flush();
                    break;
                }
                publisher_fence = match accept_publication(publisher_fence, expected) {
                    Ok(next) => next,
                    Err(refusal) => {
                        println!(
                            "MACHINE_LOSS_ERROR publication_refused_{}",
                            refusal_name(refusal)
                        );
                        let _ = std::io::stdout().flush();
                        break;
                    }
                };
                step_no = step_no.saturating_add(1);
                let digest =
                    oa_sha256::to_hex(&oa_sha256::sha256(&synthetic_payload(second, live)));
                println!(
                    "MACHINE_LOSS_QUARANTINED {} {} {} {} {}",
                    key.publisher_id, key.source_id, key.generation, key.sequence, digest
                );
                let _ = std::io::stdout().flush();
            }
            WorkerCommand::Step {
                second,
                feed_live,
                ownership,
            } => {
                if (visible_ca && ownership.is_some())
                    || stream.get(step_no) != Some(&(second, feed_live))
                {
                    println!("MACHINE_LOSS_ERROR stream_order_or_record_mismatch");
                    let _ = std::io::stdout().flush();
                    break;
                }
                let Some(publication) = publication_for_step(publisher_fence, step_no, second)
                else {
                    println!("MACHINE_LOSS_ERROR source_time_overflow");
                    let _ = std::io::stdout().flush();
                    break;
                };
                publisher_fence = match accept_publication(publisher_fence, publication) {
                    Ok(next) => next,
                    Err(refusal) => {
                        println!(
                            "MACHINE_LOSS_ERROR publication_refused_{}",
                            refusal_name(refusal)
                        );
                        let _ = std::io::stdout().flush();
                        break;
                    }
                };
                step_no = step_no.saturating_add(1);
                let ack = if visible_ca {
                    drill.process_visible_ca(second, feed_live)
                } else {
                    drill.process_with_ownership(second, feed_live, ownership.as_ref())
                };
                println!("{ack}");
                let _ = std::io::stdout().flush();
            }
            WorkerCommand::Input {
                key,
                second,
                feed_live,
                fault,
            } => {
                let identity = QuarantineRecord {
                    key,
                    payload_digest: oa_sha256::sha256(&synthetic_payload(second, feed_live)),
                };
                let Some(expected) = publication_for_step(publisher_fence, step_no, second) else {
                    println!("MACHINE_LOSS_ERROR source_time_overflow");
                    let _ = std::io::stdout().flush();
                    break;
                };
                if key.publisher_id != expected.publisher_id
                    || key.source_id != expected.source_id
                    || key.generation != expected.generation
                    || key.sequence != expected.sequence
                    || stream.get(step_no) != Some(&(second, feed_live))
                {
                    println!("MACHINE_LOSS_ERROR input_identity_or_order_mismatch");
                    let _ = std::io::stdout().flush();
                    break;
                }
                if fault == InputFault::FailBeforeApply {
                    println!(
                        "MACHINE_LOSS_FAIL_BEFORE_APPLY {} {} {} {} {}",
                        key.publisher_id,
                        key.source_id,
                        key.generation,
                        key.sequence,
                        oa_sha256::to_hex(&identity.payload_digest)
                    );
                    let _ = std::io::stdout().flush();
                    std::process::exit(86);
                }
                publisher_fence = match accept_publication(publisher_fence, expected) {
                    Ok(next) => next,
                    Err(refusal) => {
                        println!(
                            "MACHINE_LOSS_ERROR publication_refused_{}",
                            refusal_name(refusal)
                        );
                        let _ = std::io::stdout().flush();
                        break;
                    }
                };
                step_no = step_no.saturating_add(1);
                let ack = if visible_ca {
                    drill.process_visible_ca(second, feed_live)
                } else {
                    drill.process(second, feed_live)
                };
                if fault == InputFault::CrashAfterApply {
                    let fields: Vec<_> = ack.split_whitespace().collect();
                    println!(
                        "MACHINE_LOSS_APPLIED_NO_ACK {} {} {} {} {} {} {} {}",
                        key.publisher_id,
                        key.source_id,
                        key.generation,
                        key.sequence,
                        oa_sha256::to_hex(&identity.payload_digest),
                        fields[2],
                        fields[6],
                        fields[7]
                    );
                    let _ = std::io::stdout().flush();
                    std::process::exit(87);
                }
                if fault == InputFault::CrashAfterCheckpoint {
                    let output = std::env::var_os("OA_MACHINE_LOSS_CHECKPOINT_OUTPUT")
                        .map(PathBuf::from)
                        .expect("test child has a fixture checkpoint output path");
                    let checkpoint = encode_recovery_checkpoint(
                        &stream[..step_no],
                        INITIAL_FENCE,
                        RECOVERY_GENERATION,
                    );
                    write_checkpoint_create_new(&output, &checkpoint);
                    let fields: Vec<_> = ack.split_whitespace().collect();
                    println!(
                        "MACHINE_LOSS_CHECKPOINTED_NO_ACK {} {} {} {} {} {} {} {} {}",
                        key.publisher_id,
                        key.source_id,
                        key.generation,
                        key.sequence,
                        oa_sha256::to_hex(&identity.payload_digest),
                        oa_sha256::to_hex(&oa_sha256::sha256(&checkpoint)),
                        fields[2],
                        fields[6],
                        fields[7]
                    );
                    let _ = std::io::stdout().flush();
                    std::process::exit(88);
                }
                println!(
                    "MACHINE_LOSS_INPUT_ACK {} {} {} {} {} {ack}",
                    key.publisher_id,
                    key.source_id,
                    key.generation,
                    key.sequence,
                    oa_sha256::to_hex(&identity.payload_digest)
                );
                let _ = std::io::stdout().flush();
            }
        }
    }
}

struct Replica {
    child: Child,
    input: ChildStdin,
    output: Option<Receiver<io::Result<String>>>,
    reader: Option<JoinHandle<()>>,
    quarantine: Option<QuarantineRecord>,
}

impl Replica {
    fn start() -> Self {
        Self::start_with_checkpoint(None)
    }

    fn start_with_checkpoint(checkpoint: Option<&std::path::Path>) -> Self {
        Self::start_mode(checkpoint, false)
    }

    fn start_visible_ca(checkpoint: Option<&std::path::Path>) -> Self {
        Self::start_mode(checkpoint, true)
    }

    fn start_visible_ca_with_quarantine(path: &std::path::Path) -> Self {
        Self::start_mode_with_quarantine(None, true, Some(path))
    }

    fn start_visible_ca_with_checkpoint_output(
        checkpoint: &std::path::Path,
        output: &std::path::Path,
    ) -> Self {
        Self::start_mode_with_checkpoint_output(Some(checkpoint), true, None, Some(output))
    }

    fn start_mode(checkpoint: Option<&std::path::Path>, visible_ca: bool) -> Self {
        Self::start_mode_with_quarantine(checkpoint, visible_ca, None)
    }

    fn start_mode_with_quarantine(
        checkpoint: Option<&std::path::Path>,
        visible_ca: bool,
        quarantine: Option<&std::path::Path>,
    ) -> Self {
        Self::start_mode_with_checkpoint_output(checkpoint, visible_ca, quarantine, None)
    }

    fn start_mode_with_checkpoint_output(
        checkpoint: Option<&std::path::Path>,
        visible_ca: bool,
        quarantine: Option<&std::path::Path>,
        checkpoint_output: Option<&std::path::Path>,
    ) -> Self {
        let quarantine = quarantine.map(|path| {
            let record = read_quarantine(path).expect("replacement reads durable fixture marker");
            let stream = if visible_ca {
                &VISIBLE_CA_STREAM[..]
            } else {
                &SYNTHETIC_STREAM[..]
            };
            validate_quarantine_for_stream(record, stream)
                .expect("fixture marker names a record in its selected synthetic stream");
            record
        });
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "machine_loss_worker_process", "--nocapture"])
            .env("OA_MACHINE_LOSS_WORKER", "1")
            .env_remove("OA_MACHINE_LOSS_CHECKPOINT")
            .env_remove("OA_MACHINE_LOSS_CHECKPOINT_OUTPUT")
            .env_remove("OA_MACHINE_LOSS_VISIBLE_CA");
        if let Some(path) = checkpoint {
            command.env("OA_MACHINE_LOSS_CHECKPOINT", path);
        }
        if let Some(path) = checkpoint_output {
            command.env("OA_MACHINE_LOSS_CHECKPOINT_OUTPUT", path);
        }
        if visible_ca {
            command.env("OA_MACHINE_LOSS_VISIBLE_CA", "1");
        }
        Self::spawn(command, quarantine, Stdio::inherit())
    }

    fn start_controller(path: &Path) -> Self {
        // The manual drill passes the controller fixture built at the same commit.
        oa_sealed::refuse(path).unwrap();
        let mut command = Command::new(path);
        command
            .args([
                "--exact",
                "controller_takeover_worker_process",
                "--nocapture",
            ])
            .env("OA_CONTROLLER_TAKEOVER_WORKER", "1");
        Self::spawn(command, None, Stdio::inherit())
    }

    fn start_transport(side: &str) -> Self {
        let launcher_key = format!("OA_MACHINE_LOSS_TRANSPORT_{side}_LAUNCHER");
        let args_key = format!("OA_MACHINE_LOSS_TRANSPORT_{side}_ARGS");
        let executable = std::env::var_os(&launcher_key)
            .unwrap_or_else(|| panic!("set {launcher_key} to an explicit protocol launcher"));
        let path = PathBuf::from(executable);
        assert!(
            path.is_absolute(),
            "transport launcher path must be absolute"
        );
        let path_text = path.as_os_str().to_string_lossy();
        assert!(
            path_text.len() <= MAX_LAUNCHER_PATH_BYTES,
            "transport launcher path exceeds bound"
        );
        // The local launcher is a caller-supplied test executable. Refuse sealed artifacts before
        // invoking it; remote launchers remain responsible for their own authenticated setup.
        oa_sealed::refuse(&path).expect("transport launcher must not be a sealed input artifact");
        let args = std::env::var(&args_key).unwrap_or_default();
        assert!(
            args.len() <= 8_192,
            "transport launcher argument list exceeds total bound"
        );
        let mut command = Command::new(path);
        if !args.is_empty() {
            let parsed: Vec<_> = args.split('\n').collect();
            assert!(
                parsed.len() <= MAX_LAUNCHER_ARGS,
                "too many transport launcher arguments"
            );
            for arg in parsed {
                assert!(
                    !arg.is_empty(),
                    "transport launcher arguments cannot be empty lines"
                );
                assert!(
                    arg.len() <= MAX_LAUNCHER_ARG_BYTES,
                    "transport launcher argument exceeds bound"
                );
                assert!(
                    !arg.contains('\0'),
                    "transport launcher argument contains NUL"
                );
                command.arg(arg);
            }
        }
        // Launcher contract: bridge this process's stdin/stdout to the synthetic worker protocol.
        // A bridge EOF is a transport disconnect; it does not establish that a remote worker died.
        // This enables the documented local control: point at this test binary and pass the
        // machine_loss_worker_process test-harness arguments. A remote bridge may use its own
        // configured remote command and need not forward this local environment variable.
        command.env("OA_MACHINE_LOSS_WORKER", "1");
        // Do not accept unbounded launcher diagnostics on the test's output channel.
        Self::spawn(command, None, Stdio::null())
    }

    fn spawn(mut command: Command, quarantine: Option<QuarantineRecord>, stderr: Stdio) -> Self {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(stderr)
            .spawn()
            .unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let (sender, output) = mpsc::sync_channel(8);
        let reader = thread::spawn(move || read_worker_output(stdout, sender));
        Self {
            input: child.stdin.take().unwrap(),
            output: Some(output),
            reader: Some(reader),
            quarantine,
            child,
        }
    }

    fn receive_record(&self, context: &str) -> String {
        let deadline = Instant::now() + WORKER_TIMEOUT;
        let output = self.output.as_ref().expect("worker output receiver exists");
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(
                !remaining.is_zero(),
                "worker response timed out at {context}"
            );
            let line = output
                .recv_timeout(remaining)
                .unwrap_or_else(|error| panic!("worker response timeout/EOF at {context}: {error}"))
                .unwrap_or_else(|error| panic!("invalid worker output at {context}: {error}"));
            if line.starts_with("MACHINE_LOSS_") {
                return line.trim_end_matches(['\r', '\n']).to_owned();
            }
        }
    }

    fn recovered_prefix_line(&self, second: i32) -> String {
        let line = self.receive_record("checkpoint recovery");
        validate_worker_ack(&line, second);
        line
    }

    fn assert_checkpoint_refused(&mut self, reason: &str) {
        let line = self.receive_record("checkpoint refusal");
        assert_eq!(
            line,
            format!("MACHINE_LOSS_ERROR checkpoint_refused {reason}")
        );
        let status = wait_bounded(&mut self.child).expect("refusing worker exits promptly");
        assert!(status.success(), "refusal worker test failed: {status}");
    }

    fn step(&mut self, second: i32, live: bool) -> String {
        writeln!(
            self.input,
            "STEP {second} {}",
            if live { "LIVE" } else { "LOST" }
        )
        .unwrap();
        self.input.flush().unwrap();
        let line = self.receive_record(&format!("{second}s"));
        assert!(
            !line.starts_with("MACHINE_LOSS_ERROR "),
            "worker rejected command at {second}s: {line}"
        );
        validate_worker_ack(&line, second);
        line
    }

    fn controller_step(&mut self, phase: u8) -> String {
        self.input.write_all(&[phase]).unwrap();
        self.input.flush().unwrap();
        self.receive_record("controller takeover")
    }

    fn owned_step(&mut self, second: i32, live: bool, control: &str) -> String {
        let fields: Vec<_> = control.split_whitespace().collect();
        let (owner, accepted_by) = (fields[2], fields[3]);
        writeln!(
            self.input,
            "STEP_OWNED {second} {} {owner} {accepted_by}",
            if live { "LIVE" } else { "LOST" }
        )
        .unwrap();
        self.input.flush().unwrap();
        let line = self.receive_record("scope with controller takeover state");
        validate_worker_ack(&line, second);
        line
    }

    fn input_command(&mut self, verb: &str, key: InputKey, second: i32, live: bool) -> String {
        writeln!(
            self.input,
            "{verb} {} {} {} {} {second} {}",
            key.publisher_id,
            key.source_id,
            key.generation,
            key.sequence,
            if live { "LIVE" } else { "LOST" },
        )
        .unwrap();
        self.input.flush().unwrap();
        self.receive_record("identity-bound input")
    }

    fn input(&mut self, identity: QuarantineRecord, second: i32, live: bool) -> String {
        let actual_digest = oa_sha256::sha256(&synthetic_payload(second, live));
        if identity.payload_digest != actual_digest {
            return format!(
                "MACHINE_LOSS_ERROR input_payload_binding_mismatch {} {} {} {}",
                identity.key.publisher_id,
                identity.key.source_id,
                identity.key.generation,
                identity.key.sequence
            );
        }
        if let Some(record) = self.quarantine.filter(|record| record.key == identity.key) {
            if record.payload_digest != identity.payload_digest {
                return format!(
                    "MACHINE_LOSS_ERROR quarantine_identity_conflict {} {} {} {} {}",
                    identity.key.publisher_id,
                    identity.key.source_id,
                    identity.key.generation,
                    identity.key.sequence,
                    oa_sha256::to_hex(&identity.payload_digest)
                );
            }
            return self.quarantine_skip(identity.key);
        }
        self.input_command("INPUT", identity.key, second, live)
    }

    fn quarantine_skip(&mut self, key: InputKey) -> String {
        writeln!(
            self.input,
            "QUARANTINE {} {} {} {}",
            key.publisher_id, key.source_id, key.generation, key.sequence
        )
        .unwrap();
        self.input.flush().unwrap();
        self.receive_record("quarantine skip")
    }

    fn fail_before_apply(&mut self, identity: QuarantineRecord, second: i32, live: bool) -> String {
        self.input_command("FAIL_BEFORE_APPLY", identity.key, second, live)
    }

    fn crash_after_apply(&mut self, identity: QuarantineRecord, second: i32, live: bool) -> String {
        self.input_command("CRASH_AFTER_APPLY", identity.key, second, live)
    }

    fn crash_after_checkpoint(
        &mut self,
        identity: QuarantineRecord,
        second: i32,
        live: bool,
    ) -> String {
        self.input_command("CRASH_AFTER_CHECKPOINT", identity.key, second, live)
    }

    fn shutdown(&mut self) -> io::Result<ExitStatus> {
        writeln!(self.input, "STOP")?;
        self.input.flush()?;
        self.finish()
    }

    fn finish(&mut self) -> io::Result<ExitStatus> {
        let status = wait_bounded(&mut self.child)?;
        drop(self.output.take());
        if let Some(reader) = self.reader.take() {
            reader
                .join()
                .map_err(|_| io::Error::other("worker output reader panicked"))?;
        }
        Ok(status)
    }
}

#[test]
fn durable_prefix_recovers_in_fresh_process_and_refuses_damage() {
    let prefix = &SYNTHETIC_STREAM[..RECOVERY_MAX_RECORDS];
    let checkpoint = encode_recovery_checkpoint(prefix, INITIAL_FENCE, RECOVERY_GENERATION);
    let mut wrong_checkpoint = checkpoint.clone();
    wrong_checkpoint[12] ^= 1;
    let mut truncated = checkpoint.clone();
    truncated.pop();
    let truncated_digest = oa_sha256::sha256(&truncated[RECOVERY_HEADER_BYTES..]);
    truncated[12..44].copy_from_slice(&truncated_digest);

    // The prefix carries synthetic Unix source times. No wall clock value is fed to the core;
    // this drill makes no failover-latency claim (the companion drill measures that separately).
    let files = TempCheckpointFiles::new();
    let path = files.write("synthetic-prefix.oarc", &checkpoint);
    let bad_digest_path = files.write("wrong-digest.oarc", &wrong_checkpoint);
    let truncated_path = files.write("truncated-prefix.oarc", &truncated);

    let mut bad_digest = Replica::start_with_checkpoint(Some(&bad_digest_path));
    bad_digest.assert_checkpoint_refused("checkpoint_digest");
    let mut bad_prefix = Replica::start_with_checkpoint(Some(&truncated_path));
    bad_prefix.assert_checkpoint_refused("checkpoint_frame_bytes");

    let mut serving = Replica::start();
    let mut reference = Drill::new();
    reference.warmup();
    let mut expected_prefix = Vec::with_capacity(prefix.len());
    for &(second, live) in prefix {
        let expected = local_reference_step(&mut reference, second, live);
        assert_eq!(serving.step(second, live), expected);
        expected_prefix.push(expected);
    }
    let killed_status = {
        serving.child.kill().unwrap();
        wait_bounded(&mut serving.child).unwrap()
    };
    assert!(!killed_status.success());
    drop(serving);

    let mut recovered = Replica::start_with_checkpoint(Some(&path));
    for (&(second, _), expected) in prefix.iter().zip(expected_prefix) {
        let recovered_line = recovered.recovered_prefix_line(second);
        assert_eq!(
            recovered_line, expected,
            "durable replay diverged at {second}s"
        );
    }
    for &(second, live) in SYNTHETIC_STREAM.iter().skip(prefix.len()) {
        assert_eq!(
            recovered.step(second, live),
            local_reference_step(&mut reference, second, live),
            "recovered continuation diverged at {second}s"
        );
    }
    let status = recovered.shutdown().unwrap();
    assert!(status.success());
}

// REQ: OA-CA-003, OA-CA-009, OA-TRK-343
#[test]
fn visible_tracker_ca_alert_survives_silent_durable_replacement() {
    let prefix = &VISIBLE_CA_STREAM[..RECOVERY_MAX_RECORDS];
    let checkpoint = encode_recovery_checkpoint(prefix, INITIAL_FENCE, RECOVERY_GENERATION);
    let files = TempCheckpointFiles::new();
    let path = files.write("visible-ca-prefix.oarc", &checkpoint);
    let active = format!("{}:{}:{}:", A, B, EPOCH);
    let opened = format!("OPEN:{}:{}:{}", A, B, EPOCH);

    let mut serving = Replica::start_visible_ca(None);
    let mut reference = Drill::new();
    reference.warmup_visible_ca();
    let mut prefix_ack = Vec::with_capacity(prefix.len());
    for (index, &(second, live)) in prefix.iter().enumerate() {
        let ack = serving.step(second, live);
        let expected = reference.process_visible_ca(second, live);
        assert_eq!(ack, expected, "serving output diverged at {second}s");
        assert_ca_ack_state(
            &ack,
            second,
            if index == 0 { &opened } else { "-" },
            &active,
        );
        prefix_ack.push(ack);
    }

    serving.child.kill().unwrap();
    let killed_status = wait_bounded(&mut serving.child).unwrap();
    assert!(!killed_status.success());
    drop(serving);

    let mut recovered = Replica::start_visible_ca(Some(&path));
    for (index, &(second, _)) in prefix.iter().enumerate() {
        let ack = recovered.recovered_prefix_line(second);
        assert_eq!(
            ack, prefix_ack[index],
            "recovered prefix diverged at {second}s"
        );
        assert_ca_ack_state(
            &ack,
            second,
            if index == 0 { &opened } else { "-" },
            &active,
        );
    }
    for &(second, live) in VISIBLE_CA_STREAM.iter().skip(prefix.len()) {
        let ack = recovered.step(second, live);
        let expected = reference.process_visible_ca(second, live);
        assert_eq!(ack, expected, "silent recovery diverged at {second}s");
        assert_ca_ack_state(&ack, second, "-", &active);
        if !live {
            let fields: Vec<_> = ack.split_whitespace().collect();
            assert!(fields[4].parse::<usize>().is_ok_and(|count| count > 0));
        }
    }
    let status = recovered.shutdown().unwrap();
    assert!(status.success());
}

// REQ: OA-CA-003, OA-CA-009, OA-TRK-343
#[test]
fn crash_after_apply_before_ack_replays_from_verified_prefix() {
    let prefix = &VISIBLE_CA_STREAM[..RECOVERY_MAX_RECORDS];
    let checkpoint = encode_recovery_checkpoint(prefix, INITIAL_FENCE, RECOVERY_GENERATION);
    let files = TempCheckpointFiles::new();
    let path = files.write("apply-before-ack-prefix.oarc", &checkpoint);

    let mut reference = Drill::new();
    reference.warmup_visible_ca();
    let expected_prefix: Vec<_> = prefix
        .iter()
        .map(|&(second, live)| reference.process_visible_ca(second, live))
        .collect();

    // This child restores the last verified checkpoint, applies the next source record through
    // tracker, scope, raster digest and CA, then exits before its success acknowledgement.
    let mut crashed = Replica::start_visible_ca(Some(&path));
    for (&(second, _), expected) in prefix.iter().zip(&expected_prefix) {
        assert_eq!(crashed.recovered_prefix_line(second), *expected);
    }
    let (second, live) = VISIBLE_CA_STREAM[prefix.len()];
    let expected_applied = reference.process_visible_ca(second, live);
    let active = format!("{}:{}:{}:", A, B, EPOCH);
    assert_ca_ack_state(&expected_applied, second, "-", &active);
    let identity = input_identity(prefix.len() as u64 + 1, second, live);
    let fields: Vec<_> = expected_applied.split_whitespace().collect();
    let marker = crashed.crash_after_apply(identity, second, live);
    assert_eq!(
        marker,
        format!(
            "MACHINE_LOSS_APPLIED_NO_ACK {} {} {} {} {} {} {} {}",
            identity.key.publisher_id,
            identity.key.source_id,
            identity.key.generation,
            identity.key.sequence,
            oa_sha256::to_hex(&identity.payload_digest),
            fields[2],
            fields[6],
            fields[7]
        )
    );
    let status =
        wait_bounded(&mut crashed.child).expect("injected post-apply crash exits promptly");
    assert_eq!(
        status.code(),
        Some(87),
        "post-apply crash injection was not exercised"
    );
    let terminal_output = crashed
        .output
        .as_ref()
        .unwrap()
        .recv_timeout(WORKER_TIMEOUT);
    assert!(
        matches!(terminal_output, Err(mpsc::RecvTimeoutError::Disconnected)),
        "expected EOF after the fault marker, got {terminal_output:?}"
    );
    drop(crashed);

    // The replacement receives only the retained prefix, so it reconstructs the applied-but-
    // unacknowledged step once in its new process. Compare every output with uninterrupted state.
    let mut replacement = Replica::start_visible_ca(Some(&path));
    for (&(prefix_second, _), expected) in prefix.iter().zip(&expected_prefix) {
        assert_eq!(replacement.recovered_prefix_line(prefix_second), *expected);
    }
    let line = replacement.input(identity, second, live);
    let replayed = validate_input_ack(&line, identity, second);
    assert_eq!(
        replayed, expected_applied,
        "replacement diverged on replayed step"
    );
    assert_ca_ack_state(&replayed, second, "-", &active);
    for &(suffix_second, suffix_live) in VISIBLE_CA_STREAM.iter().skip(prefix.len() + 1) {
        let ack = replacement.step(suffix_second, suffix_live);
        let expected = reference.process_visible_ca(suffix_second, suffix_live);
        assert_eq!(ack, expected, "replacement diverged at {suffix_second}s");
    }
    assert!(replacement.shutdown().unwrap().success());
}

// REQ: OA-CA-003, OA-CA-009, OA-TRK-343
#[test]
fn checkpointed_apply_before_ack_recovers_and_continues() {
    let prefix_len = 2;
    let files = TempCheckpointFiles::new();
    let prefix_path = files.write(
        "verified-two-step-prefix.oarc",
        &encode_recovery_checkpoint(
            &VISIBLE_CA_STREAM[..prefix_len],
            INITIAL_FENCE,
            RECOVERY_GENERATION,
        ),
    );
    let checkpoint_path = files.uncreated_path("checkpointed-apply-before-ack.oarc");
    assert!(!checkpoint_path.exists());
    let expected_checkpoint = encode_recovery_checkpoint(
        &VISIBLE_CA_STREAM[..prefix_len + 1],
        INITIAL_FENCE,
        RECOVERY_GENERATION,
    );

    let mut reference = Drill::new();
    reference.warmup_visible_ca();
    let mut expected_prefix = Vec::with_capacity(prefix_len + 1);
    let mut serving =
        Replica::start_visible_ca_with_checkpoint_output(&prefix_path, &checkpoint_path);
    for &(second, live) in VISIBLE_CA_STREAM.iter().take(prefix_len) {
        let expected = reference.process_visible_ca(second, live);
        assert_eq!(serving.recovered_prefix_line(second), expected);
        expected_prefix.push(expected);
    }

    // Apply the third stream step, persist a checkpoint that includes it, and crash without the
    // normal success ACK. The checkpoint writer uses create_new and sync_all before the marker.
    let (second, live) = VISIBLE_CA_STREAM[prefix_len];
    let expected_applied = reference.process_visible_ca(second, live);
    let active = format!("{}:{}:{}:", A, B, EPOCH);
    assert_ca_ack_state(&expected_applied, second, "-", &active);
    let identity = input_identity(prefix_len as u64 + 1, second, live);
    let fields: Vec<_> = expected_applied.split_whitespace().collect();
    let marker = serving.crash_after_checkpoint(identity, second, live);
    assert_eq!(
        marker,
        format!(
            "MACHINE_LOSS_CHECKPOINTED_NO_ACK {} {} {} {} {} {} {} {} {}",
            identity.key.publisher_id,
            identity.key.source_id,
            identity.key.generation,
            identity.key.sequence,
            oa_sha256::to_hex(&identity.payload_digest),
            oa_sha256::to_hex(&oa_sha256::sha256(&expected_checkpoint)),
            fields[2],
            fields[6],
            fields[7]
        )
    );
    let status =
        wait_bounded(&mut serving.child).expect("checkpointed post-apply fault exits promptly");
    assert_eq!(
        status.code(),
        Some(88),
        "checkpointed fault was not exercised"
    );
    let terminal_output = serving
        .output
        .as_ref()
        .unwrap()
        .recv_timeout(WORKER_TIMEOUT);
    assert!(
        matches!(terminal_output, Err(mpsc::RecvTimeoutError::Disconnected)),
        "expected EOF after the committed marker, got {terminal_output:?}"
    );
    drop(serving);

    // Confirm the exact bounded file is visible through the sealed reader, then recover it in a
    // fresh process. The worker validates digest, frame, identity and watermark before replay.
    assert_eq!(
        read_recovery_checkpoint(&checkpoint_path).unwrap(),
        expected_checkpoint
    );
    expected_prefix.push(expected_applied);
    let mut recovered = Replica::start_visible_ca(Some(&checkpoint_path));
    for (&(prefix_second, _), expected) in VISIBLE_CA_STREAM
        .iter()
        .take(prefix_len + 1)
        .zip(&expected_prefix)
    {
        assert_eq!(recovered.recovered_prefix_line(prefix_second), *expected);
    }
    for &(suffix_second, suffix_live) in VISIBLE_CA_STREAM.iter().skip(prefix_len + 1) {
        let ack = recovered.step(suffix_second, suffix_live);
        let expected = reference.process_visible_ca(suffix_second, suffix_live);
        assert_eq!(
            ack, expected,
            "recovered continuation diverged at {suffix_second}s"
        );
    }
    assert!(recovered.shutdown().unwrap().success());

    let mut duplicate = Replica::start_visible_ca(Some(&checkpoint_path));
    for (&(prefix_second, _), expected) in VISIBLE_CA_STREAM
        .iter()
        .take(prefix_len + 1)
        .zip(&expected_prefix)
    {
        assert_eq!(duplicate.recovered_prefix_line(prefix_second), *expected);
    }
    assert_eq!(
        duplicate.input(identity, second, live),
        "MACHINE_LOSS_ERROR input_identity_or_order_mismatch",
        "the restored prefix already represents this input"
    );
    assert!(wait_bounded(&mut duplicate.child).unwrap().success());
}

// Exercises the pre-application failure described by IF-R2 and IF-R3 in docs/INTERFACES.md.
// REQ: OA-CA-003, OA-CA-009, OA-TRK-343
#[test]
fn poison_input_is_quarantined_before_replacement_application() {
    // This is a normal, well-formed synthetic tracker measurement. The explicit test command
    // fails the serving worker before application; the replacement receives only a quarantine
    // decision for that identity and then processes the following useful source records.
    let prefix = &VISIBLE_CA_STREAM[..2];
    let active = format!("{}:{}:{}:", A, B, EPOCH);
    let opened = format!("OPEN:{}:{}:{}", A, B, EPOCH);
    let mut serving = Replica::start_visible_ca(None);
    let mut reference = Drill::new();
    reference.warmup_visible_ca();
    let mut prefix_acks = Vec::with_capacity(prefix.len());
    for (index, &(second, live)) in prefix.iter().enumerate() {
        let identity = input_identity(index as u64 + 1, second, live);
        let line = serving.input(identity, second, live);
        let ack = validate_input_ack(&line, identity, second);
        let expected = reference.process_visible_ca(second, live);
        assert_eq!(ack, expected, "serving prefix diverged at {second}s");
        assert_ca_ack_state(
            &ack,
            second,
            if index == 0 { &opened } else { "-" },
            &active,
        );
        prefix_acks.push((identity, second, live, ack));
    }

    let poison = input_identity(3, 2, true);
    let poison_digest = oa_sha256::to_hex(&poison.payload_digest);
    let stalled = serving.fail_before_apply(poison, 2, true);
    assert_eq!(
        stalled,
        format!(
            "MACHINE_LOSS_FAIL_BEFORE_APPLY {} {} {} {} {}",
            poison.key.publisher_id,
            poison.key.source_id,
            poison.key.generation,
            poison.key.sequence,
            poison_digest
        )
    );
    let status = wait_bounded(&mut serving.child).expect("injected worker failure exits promptly");
    assert_eq!(
        status.code(),
        Some(86),
        "failure injection was not exercised"
    );
    drop(serving);

    let encoded_marker = encode_quarantine(poison);
    assert_eq!(decode_quarantine(&encoded_marker), Ok(poison));
    let files = TempCheckpointFiles::new();
    let marker_path = files.write("synthetic-poison-quarantine.oaq", &encoded_marker);
    let mut replacement = Replica::start_visible_ca_with_quarantine(&marker_path);

    // Rebuild the accepted prefix in a fresh child and compare it with saved primary output.
    for (identity, second, live, expected) in prefix_acks {
        let line = replacement.input(identity, second, live);
        assert_eq!(validate_input_ack(&line, identity, second), expected);
    }

    // A reused sequence with different ordinary synthetic content conflicts with the stored
    // payload binding at the input gate; it cannot evade quarantine under the same source key.
    let conflict = replacement.input(input_identity(3, 3, true), 3, true);
    assert_eq!(
        conflict,
        format!(
            "MACHINE_LOSS_ERROR quarantine_identity_conflict {} {} {} {} {}",
            poison.key.publisher_id,
            poison.key.source_id,
            poison.key.generation,
            poison.key.sequence,
            oa_sha256::to_hex(&oa_sha256::sha256(&synthetic_payload(3, true)))
        )
    );

    // The replacement's input gate sends only a skip control to its worker; the payload itself
    // never crosses the child boundary or reaches Drill processing.
    let quarantined = replacement.input(poison, 2, true);
    assert_eq!(
        quarantined,
        format!(
            "MACHINE_LOSS_QUARANTINED {} {} {} {} {}",
            poison.key.publisher_id,
            poison.key.source_id,
            poison.key.generation,
            poison.key.sequence,
            poison_digest
        )
    );
    assert_eq!(
        usize::from(quarantined.starts_with("MACHINE_LOSS_QUARANTINED ")),
        1
    );

    // The lost measurement is omitted from the reference too; later source silence still opens
    // the feed warning, and confirmed tracker-derived CA identities remain useful and comparable.
    for (index, &(second, live)) in VISIBLE_CA_STREAM.iter().enumerate().skip(prefix.len() + 1) {
        let identity = input_identity(index as u64 + 1, second, live);
        let line = replacement.input(identity, second, live);
        let ack = validate_input_ack(&line, identity, second);
        let expected = reference.process_visible_ca(second, live);
        assert_eq!(ack, expected, "replacement diverged at {second}s");
        assert_ca_ack_state(&ack, second, "-", &active);
        if !live {
            assert!(
                ack.split_whitespace()
                    .nth(4)
                    .and_then(|count| count.parse::<usize>().ok())
                    .is_some_and(|count| count > 0),
                "feed-loss warning missing after quarantined input: {ack}"
            );
        }
    }
    let status = replacement.shutdown().unwrap();
    assert!(status.success());
}

impl Drop for Replica {
    fn drop(&mut self) {
        drop(self.output.take());
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

fn read_bounded_line<R: BufRead>(
    reader: &mut R,
    bytes: &mut Vec<u8>,
) -> io::Result<Option<String>> {
    bytes.clear();
    loop {
        let (used, newline, eof) = {
            let available = reader.fill_buf()?;
            if available.is_empty() {
                if bytes.is_empty() {
                    return Ok(None);
                }
                (0, true, true)
            } else {
                let used = available
                    .iter()
                    .position(|&byte| byte == b'\n')
                    .map_or(available.len(), |position| position + 1);
                if bytes.len().saturating_add(used) > MAX_WORKER_LINE_BYTES {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "worker output line exceeds 1024-byte bound",
                    ));
                }
                bytes.extend_from_slice(&available[..used]);
                (used, bytes.last() == Some(&b'\n'), false)
            }
        };
        reader.consume(used);
        if newline || eof {
            let line = String::from_utf8(std::mem::take(bytes)).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "worker output is not UTF-8")
            })?;
            return Ok(Some(line));
        }
    }
}

#[test]
fn worker_protocol_is_typed_and_caps_output_lines() {
    assert_eq!(
        parse_worker_command("STEP 12 LIVE"),
        Some(WorkerCommand::Step {
            second: 12,
            feed_live: true,
            ownership: None,
        })
    );
    assert_eq!(parse_worker_command("STEP 12 LIVE extra"), None);
    assert_eq!(
        parse_worker_command("STEP_OWNED 12 LIVE ZAU12 ZMP34"),
        Some(WorkerCommand::Step {
            second: 12,
            feed_live: true,
            ownership: ownership_input("ZAU12", "ZMP34"),
        }),
    );
    assert_eq!(parse_worker_command("STEP_OWNED 12 LIVE ZAU12 zmp34"), None);
    assert_eq!(
        parse_worker_command("STEP_OWNED 12 LIVE ZAU12 ZMP34 extra"),
        None
    );
    let key = InputKey {
        publisher_id: 1,
        source_id: 2,
        generation: 3,
        sequence: 4,
    };
    assert_eq!(
        parse_worker_command("INPUT 1 2 3 4 12 LIVE"),
        Some(WorkerCommand::Input {
            key,
            second: 12,
            feed_live: true,
            fault: InputFault::None,
        })
    );
    assert_eq!(
        parse_worker_command("FAIL_BEFORE_APPLY 1 2 3 4 12 LIVE"),
        Some(WorkerCommand::Input {
            key,
            second: 12,
            feed_live: true,
            fault: InputFault::FailBeforeApply,
        })
    );
    assert_eq!(
        parse_worker_command("CRASH_AFTER_APPLY 1 2 3 4 12 LOST"),
        Some(WorkerCommand::Input {
            key,
            second: 12,
            feed_live: false,
            fault: InputFault::CrashAfterApply,
        })
    );
    assert_eq!(
        parse_worker_command("CRASH_AFTER_CHECKPOINT 1 2 3 4 12 LOST"),
        Some(WorkerCommand::Input {
            key,
            second: 12,
            feed_live: false,
            fault: InputFault::CrashAfterCheckpoint,
        })
    );
    assert_eq!(
        parse_worker_command("QUARANTINE 1 2 3 4"),
        Some(WorkerCommand::QuarantineSkip { key })
    );
    assert_eq!(parse_worker_command("INPUT 1 2 3 4 12 LIVE extra"), None);
    let mut input = io::Cursor::new(vec![b'x'; MAX_WORKER_LINE_BYTES + 1]);
    let mut line = Vec::with_capacity(MAX_WORKER_LINE_BYTES);
    assert_eq!(
        read_bounded_line(&mut input, &mut line)
            .err()
            .map(|error| error.kind()),
        Some(io::ErrorKind::InvalidData)
    );
}

fn read_worker_output(mut reader: BufReader<ChildStdout>, sender: SyncSender<io::Result<String>>) {
    let mut bytes = Vec::with_capacity(MAX_WORKER_LINE_BYTES);
    loop {
        match read_bounded_line(&mut reader, &mut bytes) {
            Ok(Some(line)) => {
                if sender.send(Ok(line)).is_err() {
                    break;
                }
            }
            Ok(None) => break,
            Err(error) => {
                let _ = sender.send(Err(error));
                break;
            }
        }
    }
}

fn wait_bounded(child: &mut Child) -> io::Result<ExitStatus> {
    let deadline = Instant::now() + WORKER_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "worker did not exit within 60 seconds",
            ));
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn local_reference_step(drill: &mut Drill, second: i32, live: bool) -> String {
    drill.process(second, live)
}

fn warning_count(scope: &Replay) -> usize {
    scope
        .list
        .texts
        .start
        .iter()
        .zip(&scope.list.texts.len)
        .filter_map(|(&start, &len)| {
            let start = start as usize;
            let end = start.saturating_add(len as usize);
            scope.list.text.get(start..end)
        })
        .filter(|s| s.starts_with("FEED ") || s.starts_with("PICTURE ") || s.starts_with("MSAW "))
        .count()
}

// REQ: OA-DSP-073
#[test]
fn radar_processing_loss_appears_on_a_fresh_scope() {
    use oa_scope_replay::adapt::radar_work_notice;
    use oa_track::fusion::{
        fusion_advance, fusion_config, fusion_new, fusion_plots, fusion_stats,
        fusion_tracker_config,
    };
    use oa_track::fusion_params::MAX_BUCKET_PLOTS;
    use oa_track::radar::{RadarPlot, RadarSite, NONE_CODE, PLOT_NO_ADDR};

    let site = RadarSite {
        lat_deg: 41.9,
        lon_deg: -87.9,
        height_m: 200.0,
        sigma_range_m: 70.0,
        sigma_az_deg: 0.08,
        range_quantum_m: 0.0,
        alt_quantum_ft: 100,
        period_ns: 4 * S,
        epoch_ns: EPOCH - S,
        sensor: 3,
    };
    let mut cfg = fusion_config(fusion_tracker_config(
        &oa_track::TrackerConfig {
            max_tracks: 8,
            out_capacity: 256,
            log_capacity: 16,
            confirm_pool: 8,
            ..DEFAULT_CONFIG
        },
        &[site],
    ));
    cfg.registration = false;
    let mut fusion = fusion_new(&cfg, &[site]).unwrap();
    let mut rows = output_new(256);
    let quiet = *fusion_stats(&fusion);
    let plot = RadarPlot {
        time_ns: EPOCH - S,
        range_m: 25_000.0,
        az_deg: 90.0,
        alt_ft: 12_000,
        addr: PLOT_NO_ADDR,
        label: A,
        mode_a: NONE_CODE,
    };
    fusion_plots(&mut fusion, 0, &[plot]);
    fusion_advance(&mut fusion, EPOCH, &mut rows);
    let healthy = *fusion_stats(&fusion);
    assert_eq!(healthy.plots_in, 1);
    assert_eq!(healthy.plots_refused, 0);
    assert!(radar_work_notice(&quiet, &healthy).is_none());

    let mut d = Drill::new();
    for net in NETS {
        state_report(&mut d.states, net, NetReport::Running, EPOCH, EPOCH).unwrap();
    }
    d.warmup();
    d.offer(A, 1, 0);
    d.offer(B, 2, 0);
    d.frame(0, &[]);

    // One bucket exceeds the real per-radar plot limit by one. Flight picture updates
    // continue independently, so only the newly reported radar work loss is visible.
    let burst = vec![
        RadarPlot {
            time_ns: EPOCH,
            ..plot
        };
        MAX_BUCKET_PLOTS + 1
    ];
    fusion_plots(&mut fusion, 0, &burst);
    fusion_advance(&mut fusion, EPOCH + S, &mut rows);
    let limited = *fusion_stats(&fusion);
    assert_eq!(
        limited.bucket_plot_overflow - healthy.bucket_plot_overflow,
        1
    );
    let warning = radar_work_notice(&healthy, &limited).unwrap();
    assert_eq!(warning.state, FeedState::CoverageLimited);
    assert!(radar_work_notice(&limited, &limited).is_none());
    assert_eq!(
        radar_work_notice(&limited, &healthy).unwrap().state,
        FeedState::InvalidHealth,
    );
    d.feeds[1] = warning;
    d.offer(A, 1, 1);
    d.offer(B, 2, 1);
    d.frame(1, &["FEED RADAR COVERAGE LIMITED"]);
    let colour = palette::bytes_of(palette::ALERT).unwrap();
    assert!(d.scope.buf.chunks_exact(4).any(|pixel| pixel == colour));
}

// This is a process fault injection over synthetic records; it does not claim production
// supervision or durable state transfer. Both workers replay the same stream through real code.
#[test]
fn serving_process_loss_promotes_synchronized_scope_replica() {
    let mut serving = Replica::start();
    let mut standby = Replica::start();
    let mut reference = Drill::new();
    reference.warmup();
    let mut takeover = String::new();
    let mut pre_kill_open = String::new();
    for &(second, live) in SYNTHETIC_STREAM.iter().take(11) {
        let a = serving.step(second, live);
        let b = standby.step(second, live);
        let expected = local_reference_step(&mut reference, second, live);
        assert_eq!(a, b, "replicas diverged before loss at {second}");
        assert_eq!(a, expected, "reference diverged before loss at {second}");
        if second == 10 {
            takeover = b;
        } else if second == 0 {
            pre_kill_open = b;
        }
    }
    assert!(
        pre_kill_open.split_whitespace().nth(6)
            == Some(format!("OPEN:{}:{}:{}", 0xD10001, 0xD10002, EPOCH).as_str()),
        "pre-kill alert did not open: {pre_kill_open}"
    );
    assert!(
        takeover
            .split_whitespace()
            .nth(7)
            .is_some_and(|ids| ids.starts_with(&format!("{}:{}:", 0xD10001, 0xD10002))),
        "active CA alert missing at takeover: {takeover}"
    );
    assert_eq!(
        takeover.split_whitespace().nth(6),
        Some("-"),
        "active alert was re-announced at takeover: {takeover}"
    );

    // The feed outage begins at source time 11s. Kill the active OS process at its last
    // synchronized checkpoint (10s), while no feed events are available.
    let promotion_started = Instant::now();
    serving.child.kill().unwrap();
    let killed_status = wait_bounded(&mut serving.child).unwrap();
    assert!(
        !killed_status.success(),
        "serving worker survived kill injection"
    );
    drop(serving);

    // The next source frame is at 40s: logical display gap is measured on source time, not the
    // tiny wall-clock time spent killing/promoting the process.
    let mut promoted = String::new();
    let mut promotion_observed_ms = 0;
    let mut recovery = String::new();
    let mut post_recovery_open = String::new();
    for &(second, live) in SYNTHETIC_STREAM.iter().skip(11) {
        let ack = standby.step(second, live);
        let expected = local_reference_step(&mut reference, second, live);
        assert_eq!(ack, expected);
        if second == 40 {
            promoted = ack;
            promotion_observed_ms = promotion_started.elapsed().as_millis();
        } else if second == 41 {
            post_recovery_open = ack.clone();
            recovery = ack;
        } else {
            recovery = ack;
        }
    }
    assert!(
        promoted.split_whitespace().nth(6)
            == Some(format!("CLOSE:{}:{}:{}", 0xD10001, 0xD10002, EPOCH).as_str()),
        "pre-takeover alert did not close deterministically: {promoted}"
    );
    assert!(
        post_recovery_open.split_whitespace().nth(6)
            == Some(format!("OPEN:{}:{}:{}", 0xD20001, 0xD20002, EPOCH + 41 * S).as_str()),
        "post-recovery alert was not emitted: {post_recovery_open}"
    );
    let status = standby.shutdown().unwrap();
    assert!(
        status.success(),
        "standby worker exited unsuccessfully: {status}"
    );
    let fields: Vec<_> = recovery.split_whitespace().collect();
    assert_eq!(fields[3], format!("{},{}", A, B));
    assert_eq!(fields[2].len(), 64);
    eprintln!("machine-loss: last_frame_s=10 next_frame_s=40 logical_display_gap_s=30 promotion_observed_ms={promotion_observed_ms} recovery={recovery}");
}

// Opt-in transport exercise. The caller supplies a test launcher that bridges the bounded
// MACHINE_LOSS line protocol over stdin/stdout to a worker on another host. No network client,
// credentials, remote kill, or production endpoint is selected by this test.
#[test]
#[ignore = "set OA_MACHINE_LOSS_TRANSPORT_LAUNCHER to an explicit synthetic protocol bridge"]
fn transport_separated_workers_match_synthetic_scope_reference() {
    let mut left = Replica::start_transport("LEFT");
    let mut right = Replica::start_transport("RIGHT");
    let mut reference = Drill::new();
    reference.warmup();
    for &(second, live) in &SYNTHETIC_STREAM {
        let left_ack = left.step(second, live);
        let right_ack = right.step(second, live);
        let expected = local_reference_step(&mut reference, second, live);
        assert_eq!(
            left_ack, right_ack,
            "transport workers diverged at {second}s"
        );
        assert_eq!(
            left_ack, expected,
            "transport worker differs from local reference at {second}s"
        );
    }
    let left_status = left.shutdown().expect("launcher exits after protocol STOP");
    let right_status = right
        .shutdown()
        .expect("launcher exits after protocol STOP");
    assert!(
        left_status.success(),
        "left launcher exited unsuccessfully: {left_status}"
    );
    assert!(
        right_status.success(),
        "right launcher exited unsuccessfully: {right_status}"
    );
    eprintln!(
        "transport-drill: workers=2 records={} result=synthetic-output-equivalence launcher_exit=observed remote_process_death=not_claimed",
        SYNTHETIC_STREAM.len()
    );
}

fn controller_ownership(record: &str, phase: &str) -> OwnershipStatus {
    let fields: Vec<_> = record.split_whitespace().collect();
    let ["MACHINE_LOSS_CONTROL", got_phase, owner, accepted_by, "36000", "24000", completed, digest, address, "SYN901"] =
        fields.as_slice()
    else {
        panic!("invalid controller witness {record}");
    };
    assert_eq!(*got_phase, phase);
    assert_eq!(address.parse::<u32>().unwrap(), A);
    assert_eq!(digest.len(), 16);
    assert!(u64::from_str_radix(digest, 16).is_ok());
    match phase {
        "P" => assert_eq!((*owner, *accepted_by, *completed), ("ZAU12", "ZMP34", "0")),
        "T" | "R" => assert_eq!((*owner, *accepted_by, *completed), ("ZMP34", "-", "1")),
        _ => panic!("unexpected witness phase {phase}"),
    }
    ownership_input(owner, accepted_by).unwrap()
}

#[test]
#[ignore = "set OA_CONTROLLER_TAKEOVER_EXECUTABLE to the oa-flight test binary from this build"]
fn controller_handoff_and_scope_continue_after_serving_processes_are_killed() {
    const PREFIX_LEN: usize = 11;
    let executable = PathBuf::from(std::env::var_os("OA_CONTROLLER_TAKEOVER_EXECUTABLE").unwrap());
    let mut serving = Replica::start();
    let mut standby = Replica::start();
    let mut serving_control = Replica::start_controller(&executable);
    let mut standby_control = Replica::start_controller(&executable);
    let mut reference_control = Replica::start_controller(&executable);
    let mut reference_scope = Drill::new();
    reference_scope.warmup();

    let pending = serving_control.controller_step(b'P');
    let backup_pending = standby_control.controller_step(b'P');
    assert_eq!(pending, backup_pending);
    assert_eq!(pending, reference_control.controller_step(b'P'));
    let pending_ownership = controller_ownership(&pending, "P");

    let mut fence = INITIAL_FENCE;
    for (step, &(second, live)) in SYNTHETIC_STREAM.iter().enumerate().take(PREFIX_LEN) {
        let primary = serving.owned_step(second, live, &pending);
        assert_eq!(primary, standby.owned_step(second, live, &backup_pending));
        assert_eq!(
            primary,
            reference_scope.process_with_ownership(second, live, Some(&pending_ownership)),
        );
        let publication = publication_for_step(fence, step, second).unwrap();
        fence = accept_publication(fence, publication).unwrap();
    }

    let promotion_started = Instant::now();
    serving.child.kill().unwrap();
    serving_control.child.kill().unwrap();
    assert!(!serving.finish().unwrap().success());
    assert!(!serving_control.finish().unwrap().success());

    let old_fence = fence;
    fence = advance_generation(
        fence,
        RECOVERY_PUBLISHER,
        RECOVERY_SOURCE,
        RECOVERY_GENERATION,
    )
    .unwrap();
    let delayed =
        publication_for_step(old_fence, PREFIX_LEN, SYNTHETIC_STREAM[PREFIX_LEN].0).unwrap();
    assert_eq!(
        accept_publication(fence, delayed),
        Err(FenceRefusal::Generation)
    );

    let mut witness = backup_pending;
    let mut ownership = pending_ownership;
    let mut first_frame_ms = None;
    for (step, &(second, live)) in SYNTHETIC_STREAM.iter().enumerate().skip(PREFIX_LEN) {
        if second == 41 {
            witness = standby_control.controller_step(b'T');
            assert_eq!(witness, reference_control.controller_step(b'T'));
            ownership = controller_ownership(&witness, "T");
        }
        if second == 42 {
            witness = standby_control.controller_step(b'R');
            assert_eq!(witness, reference_control.controller_step(b'R'));
            ownership = controller_ownership(&witness, "R");
        }

        let candidate = standby.owned_step(second, live, &witness);
        assert_eq!(
            candidate,
            reference_scope.process_with_ownership(second, live, Some(&ownership)),
        );
        let publication = publication_for_step(fence, step, second).unwrap();
        fence = accept_publication(fence, publication).unwrap();
        assert_eq!(
            accept_publication(fence, publication),
            Err(FenceRefusal::SequenceReplay),
        );
        if second == 40 {
            first_frame_ms = Some(promotion_started.elapsed().as_millis());
            assert!(reference_scope
                .scope
                .list
                .text
                .contains("FLIGHT SYN901 OWNER ZAU12 | ACCEPTED BY ZMP34"));
        }
        if second >= 41 {
            assert!(reference_scope
                .scope
                .list
                .text
                .contains("FLIGHT SYN901 OWNER ZMP34 | CURRENT"));
        }
    }

    assert!(standby.shutdown().unwrap().success());
    for control in [&mut standby_control, &mut reference_control] {
        control.input.write_all(b"Q").unwrap();
        control.input.flush().unwrap();
        assert!(control.finish().unwrap().success());
    }
    assert_eq!(fence.last_sequence, SYNTHETIC_STREAM.len() as u64);
    eprintln!(
        "controller-takeover: last_source_s=10 first_promoted_source_s=40 \
         source_gap_s=30 first_verified_frame_local_ms={} completed_handoffs=1",
        first_frame_ms.unwrap()
    );
}

// REQ: OA-DSP-043, OA-DSP-061, OA-DSP-063, OA-DSP-064, OA-TRK-343
#[test]
fn blackout_and_staggered_recovery_keep_tracks_and_clear_only_recovered_warnings() {
    let mut d = Drill::new();
    for second in 0..=10 {
        d.offer(A, 1, second);
        d.offer(B, 2, second);
    }
    let last_healthy = EPOCH + 10 * S;
    for net in NETS {
        state_report(
            &mut d.states,
            net,
            NetReport::Running,
            last_healthy,
            last_healthy,
        )
        .unwrap();
    }
    d.frame(10, &[]);
    let started = tracker_stats(&d.tracker).started;
    assert_eq!(started, 2);

    // Both feeds stop. Only MSAW loses its heartbeat. Tracker ticks still display coasting
    // positions, but they must not make the picture look fresh.
    let blackout = EPOCH + 40 * S;
    for net in NETS.into_iter().filter(|&net| net != NetId::Msaw) {
        state_report(&mut d.states, net, NetReport::Running, blackout, blackout).unwrap();
    }
    d.feeds[0].state = FeedState::Lost { age_ms: 30_000 };
    d.feeds[1].state = FeedState::Lost { age_ms: 30_000 };
    d.frame(
        40,
        &[
            "PICTURE FROZEN 30.0S",
            "MSAW UNAVAILABLE: STATE STALE, AGE 30S",
            "FEED ADSB LOST 30.0S",
            "FEED RADAR LOST 30.0S",
        ],
    );
    assert_eq!(tracker_newest_update_ns(&d.tracker), last_healthy);
    assert!(targets_columns(&d.scope.set)
        .flags
        .iter()
        .all(|&flag| flag & T_COAST != 0));

    // A feed becomes live before it supplies any accepted track update.
    d.feeds[0].state = FeedState::Live;
    d.feeds[1].state = FeedState::Lost { age_ms: 31_000 };
    d.frame(
        41,
        &[
            "PICTURE FROZEN 31.0S",
            "MSAW UNAVAILABLE: STATE STALE, AGE 31S",
            "FEED RADAR LOST 31.0S",
        ],
    );

    // ADS-B reacquires A, but RADAR's B is coasting and MSAW is still stale.
    d.offer(A, 1, 41);
    d.frame(
        41,
        &[
            "MSAW UNAVAILABLE: STATE STALE, AGE 31S",
            "FEED RADAR LOST 31.0S",
        ],
    );
    let flags = targets_columns(&d.scope.set).flags;
    assert_eq!(flags[0] & T_COAST, 0);
    assert_ne!(flags[1] & T_COAST, 0);

    d.feeds[1].state = FeedState::Live;
    d.offer(B, 2, 42);
    d.frame(42, &["MSAW UNAVAILABLE: STATE STALE, AGE 32S"]);
    for (addr, second) in [(A, 41), (B, 42)] {
        assert!(d.out.addr.iter().enumerate().any(|(i, &stored)| {
            stored == addr
                && d.out.t_ns[i] == EPOCH + second * S
                && d.out.flags[i] & O_REACQUIRED != 0
        }));
    }
    assert_eq!(tracker_stats(&d.tracker).started, started);
    assert_eq!(
        tracker_stats(&d.tracker).applied,
        tracker_stats(&d.tracker).offered
    );

    let restored = EPOCH + 43 * S;
    state_report(
        &mut d.states,
        NetId::Msaw,
        NetReport::Running,
        restored,
        restored,
    )
    .unwrap();
    d.frame(43, &[]);
}

// REQ: OA-SAF-020, OA-DSP-072
#[test]
fn independent_monitor_contradiction_selects_synthetic_raw_fallback() {
    let mut drill = Drill::new();
    drill.warmup_visible_ca();
    drill.offer_with_vrate(A, 1, 0, 0);
    drill.offer_with_vrate(B, 2, 0, 0);
    drill.frame_inner(0, None);

    // This label comes from the fixture's accepted synthetic measurement, not from the output.
    let truth_lat_e7 = LAT + 80_000;
    let truth_lon_e7 = LON;
    let truth_alt_ft = 12_000;
    let truth_time_ns = EPOCH;
    let frame = frame_new(
        f64::from(truth_lat_e7) / 10_000_000.0,
        f64::from(truth_lon_e7) / 10_000_000.0,
    );
    let (truth_east, truth_north) = project(
        &frame,
        f64::from(truth_lat_e7) / 10_000_000.0,
        f64::from(truth_lon_e7) / 10_000_000.0,
    );
    let (accepted, confirmed, newest_ns) = tracker_predict(&drill.tracker, A, EPOCH)
        .expect("synthetic warmed target has tracker output");
    assert!(confirmed, "the monitor pair must use a confirmed track");
    assert_eq!(accepted.addr, A);
    assert_eq!(
        newest_ns, truth_time_ns,
        "use accepted, fresh measurement state"
    );
    assert_eq!(accepted.t_ns, EPOCH);

    let (claim_east, claim_north) = project(
        &frame,
        f64::from(accepted.lat_e7) / 10_000_000.0,
        f64::from(accepted.lon_e7) / 10_000_000.0,
    );
    let make_pair = |claim_alt_ft: i32| MonitorPair {
        reference_id: A,
        claim_id: accepted.addr,
        reference_time_ns: truth_time_ns,
        claim_time_ns: accepted.t_ns,
        reference_east_mm: (truth_east * 1_000.0).round() as i64,
        reference_north_mm: (truth_north * 1_000.0).round() as i64,
        claim_east_mm: (claim_east * 1_000.0).round() as i64,
        claim_north_mm: (claim_north * 1_000.0).round() as i64,
        reference_alt_ft: truth_alt_ft,
        claim_alt_ft,
        presence: HAS_REFERENCE | HAS_CLAIM | COMMON_LOCAL_FRAME,
    };
    let limits = MonitorLimits {
        max_age_ns: S.unsigned_abs(),
        max_pair_skew_ns: 0,
        max_horizontal_rate_mm_s: 0,
        max_vertical_rate_ft_s: 0,
        horizontal_tolerance_mm: 100_000,
        vertical_tolerance_ft: 25,
    };
    let mut finding = [MonitorFinding {
        reference_id: 0,
        claim_id: 0,
        status: MonitorStatus::InvalidEvidence,
        violations: 0,
    }];
    let healthy = [make_pair(accepted.alt_ft.round() as i32)];
    let healthy_report = check_pairs(EPOCH, &healthy, limits, &mut finding).unwrap();
    assert_eq!(healthy_report.healthy, 1);
    assert_eq!(finding[0].status, MonitorStatus::Healthy);

    // Mutation is isolated to the checker input. The tracker's original output and the
    // independently declared raw fallback observation remain unchanged.
    let mut altered_output = accepted;
    altered_output.alt_ft += 1_000.0;
    let altered = [make_pair(altered_output.alt_ft.round() as i32)];
    let altered_report = check_pairs(EPOCH, &altered, limits, &mut finding).unwrap();
    assert_eq!(altered_report.contradicted, 1);
    assert_eq!(finding[0].status, MonitorStatus::Contradiction);
    assert_eq!(finding[0].violations, VIOLATION_ALTITUDE);

    // This scenario policy chooses the explicit raw-only display after the monitor result.
    // The monitor does not trigger, authorize, or wire this fallback in product operation.
    let times = [truth_time_ns];
    let addresses = [A];
    let latitudes = [truth_lat_e7];
    let longitudes = [truth_lon_e7];
    let altitudes = [truth_alt_ft];
    let speeds = [oa_display::traffic::NO_U16];
    let tracks = [oa_display::traffic::NO_U16];
    let raw = RawObservationColumns {
        t_ns: &times,
        addr: &addresses,
        lat_e7: &latitudes,
        lon_e7: &longitudes,
        alt_ft: &altitudes,
        gs_dkt: &speeds,
        track_ddeg: &tracks,
    };
    let (_, stats) = replay_build_raw(&mut drill.scope, &raw, EPOCH, "MONITOR DRILL").unwrap();
    assert_eq!(stats.targets_admitted, 1);
    let shown = targets_columns(&drill.scope.set);
    assert_eq!(shown.addr, &[A]);
    assert_eq!(
        (shown.lat_e7[0], shown.lon_e7[0]),
        (truth_lat_e7, truth_lon_e7)
    );
    assert_eq!(shown.alt_ft[0], truth_alt_ft);
    replay_raster(&mut drill.scope).unwrap();
    let text: Vec<&str> = (0..drill.scope.list.texts.x.len())
        .map(|i| {
            let start = drill.scope.list.texts.start[i] as usize;
            let len = drill.scope.list.texts.len[i] as usize;
            &drill.scope.list.text[start..start + len]
        })
        .collect();
    assert!(text.contains(&"RAW SURVEILLANCE ONLY"));
    let alert_pixel = palette::PALETTE[usize::from(palette::ALERT)].to_le_bytes();
    assert!(drill
        .scope
        .buf
        .chunks_exact(drill.scope.spec.stride as usize)
        .skip(10)
        .take(16)
        .flat_map(|line| line.chunks_exact(4))
        .any(|pixel| pixel == alert_pixel));
}
