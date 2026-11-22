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
use oa_display::raster::frame_digest;
use oa_display::safety::{FeedState, FeedStatus};
use oa_display::traffic::{targets_columns, DEFAULT_STYLE, T_COAST};
use oa_safety::inhibit::{inhibits_new, InhibitTable};
use oa_safety::net::{state_report, state_table_new, NetId, NetReport, StateTable, NETS};
use oa_scope_replay::adapt::{row_columns, ColumnBuf};
use oa_scope_replay::frame::{
    replay_build_with_safety, replay_new, replay_raster, Replay, ReplayConfig, ReplaySafety,
};
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
        let path = self.dir.join(name);
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .expect("create checkpoint without replacing an existing file");
        file.write_all(bytes).expect("write synthetic checkpoint");
        file.sync_all()
            .expect("sync synthetic checkpoint before fault injection");
        path
    }
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
enum WorkerCommand {
    Step {
        second: i32,
        feed_live: bool,
    },
    Input {
        key: InputKey,
        second: i32,
        feed_live: bool,
        fail_before_apply: bool,
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

fn parse_worker_command(line: &str) -> Option<WorkerCommand> {
    let mut fields = line.split_whitespace();
    let command = fields.next()?;
    if command == "STOP" {
        return fields.next().is_none().then_some(WorkerCommand::Stop);
    }
    let parsed = match command {
        "STEP" => {
            let second = fields.next()?.parse().ok()?;
            let feed_live = match fields.next()? {
                "LIVE" => true,
                "LOST" => false,
                _ => return None,
            };
            WorkerCommand::Step { second, feed_live }
        }
        "INPUT" | "FAIL_BEFORE_APPLY" | "QUARANTINE" => {
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
                    fail_before_apply: command == "FAIL_BEFORE_APPLY",
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
        self.frame_inner(i64::from(second), None);
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
        replay_build_with_safety(
            &mut self.scope,
            &store,
            &Callsigns::default(),
            at,
            "OUTAGE DRILL",
            &ReplaySafety {
                states: &self.states,
                inhibits: &self.inhibits,
                feeds: &self.feeds,
                newest_track_ns: (newest != i64::MIN).then_some(newest),
                frozen_limit_ns: 5 * S,
            },
        )
        .unwrap();
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
            WorkerCommand::Step { second, feed_live } => {
                if stream.get(step_no) != Some(&(second, feed_live)) {
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
                    drill.process(second, feed_live)
                };
                println!("{ack}");
                let _ = std::io::stdout().flush();
            }
            WorkerCommand::Input {
                key,
                second,
                feed_live,
                fail_before_apply,
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
                if fail_before_apply {
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

    fn start_mode(checkpoint: Option<&std::path::Path>, visible_ca: bool) -> Self {
        Self::start_mode_with_quarantine(checkpoint, visible_ca, None)
    }

    fn start_mode_with_quarantine(
        checkpoint: Option<&std::path::Path>,
        visible_ca: bool,
        quarantine: Option<&std::path::Path>,
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
            .env_remove("OA_MACHINE_LOSS_VISIBLE_CA")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        if let Some(path) = checkpoint {
            command.env("OA_MACHINE_LOSS_CHECKPOINT", path);
        }
        if visible_ca {
            command.env("OA_MACHINE_LOSS_VISIBLE_CA", "1");
        }
        let mut child = command.spawn().unwrap();
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

    fn shutdown(&mut self) -> io::Result<ExitStatus> {
        writeln!(self.input, "STOP")?;
        self.input.flush()?;
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

// REQ: IF-R2, IF-R3, OA-CA-003, OA-CA-009, OA-TRK-343
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
            feed_live: true
        })
    );
    assert_eq!(parse_worker_command("STEP 12 LIVE extra"), None);
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
            fail_before_apply: false,
        })
    );
    assert_eq!(
        parse_worker_command("FAIL_BEFORE_APPLY 1 2 3 4 12 LIVE"),
        Some(WorkerCommand::Input {
            key,
            second: 12,
            feed_live: true,
            fail_before_apply: true,
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
