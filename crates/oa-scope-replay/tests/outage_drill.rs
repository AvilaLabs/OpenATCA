//! Synthetic outage drill from tracker measurements through the scope's safety display.
#![allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic)]

use oa_airspace::model::{empty_model, Meta};
use oa_ca::{
    alert_log_new, ca_cycle, ca_new, ca_open, event_log_new, Area, CaConfig, CaTrack, Volume,
    AIRBORNE, MAX_VOLUMES,
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
    output_new, tracker_advance, tracker_new, tracker_newest_update_ns, tracker_stats,
    tracker_update, Measurement, TrackOutput, Tracker, DEFAULT_CONFIG, M_HAS_VEL, NONE_I32,
    NO_IDENT, O_REACQUIRED,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WorkerCommand {
    Step { second: i32, feed_live: bool },
    Stop,
}

fn parse_worker_command(line: &str) -> Option<WorkerCommand> {
    let mut fields = line.split_whitespace();
    match (fields.next()?, fields.next(), fields.next(), fields.next()) {
        ("STOP", None, None, None) => Some(WorkerCommand::Stop),
        ("STEP", Some(second), Some("LIVE"), None) => Some(WorkerCommand::Step {
            second: second.parse().ok()?,
            feed_live: true,
        }),
        ("STEP", Some(second), Some("LOST"), None) => Some(WorkerCommand::Step {
            second: second.parse().ok()?,
            feed_live: false,
        }),
        _ => None,
    }
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
        .map_err(|refusal| refusal_name(refusal))?
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
        fence = accept_publication(fence, publication).map_err(|refusal| refusal_name(refusal))?;
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

    fn offer(&mut self, addr: u32, sensor: u16, second: i32) {
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
                vrate_fpm: NONE_I32,
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
    drill.warmup();
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
            if SYNTHETIC_STREAM.get(step_no) != Some(&(second, live)) {
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
            println!("{}", drill.process(second, live));
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
        let (second, live) = match command {
            WorkerCommand::Stop if step_no == SYNTHETIC_STREAM.len() => break,
            WorkerCommand::Stop => {
                println!("MACHINE_LOSS_ERROR premature_stop");
                let _ = std::io::stdout().flush();
                break;
            }
            WorkerCommand::Step { second, feed_live } => (second, feed_live),
        };
        if SYNTHETIC_STREAM.get(step_no) != Some(&(second, live)) {
            println!("MACHINE_LOSS_ERROR stream_order_or_record_mismatch");
            let _ = std::io::stdout().flush();
            break;
        }
        let Some(publication) = publication_for_step(publisher_fence, step_no, second) else {
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
        let ack = drill.process(second, live);
        println!("{ack}");
        let _ = std::io::stdout().flush();
    }
}

struct Replica {
    child: Child,
    input: ChildStdin,
    output: Option<Receiver<io::Result<String>>>,
    reader: Option<JoinHandle<()>>,
}

impl Replica {
    fn start() -> Self {
        Self::start_with_checkpoint(None)
    }

    fn start_with_checkpoint(checkpoint: Option<&std::path::Path>) -> Self {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "machine_loss_worker_process", "--nocapture"])
            .env("OA_MACHINE_LOSS_WORKER", "1")
            .env_remove("OA_MACHINE_LOSS_CHECKPOINT")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if let Some(path) = checkpoint {
            command.env("OA_MACHINE_LOSS_CHECKPOINT", path);
        }
        let mut child = command.spawn().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let (sender, output) = mpsc::sync_channel(8);
        let reader = thread::spawn(move || read_worker_output(stdout, sender));
        Self {
            input: child.stdin.take().unwrap(),
            output: Some(output),
            reader: Some(reader),
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
            if line.starts_with("MACHINE_LOSS_ACK ") || line.starts_with("MACHINE_LOSS_ERROR ") {
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
    for &(second, live) in prefix {
        assert_eq!(
            serving.step(second, live),
            local_reference_step(&mut reference, second, live)
        );
    }
    let killed_status = {
        serving.child.kill().unwrap();
        wait_bounded(&mut serving.child).unwrap()
    };
    assert!(!killed_status.success());
    drop(serving);

    let mut recovered = Replica::start_with_checkpoint(Some(&path));
    for &(second, live) in prefix {
        let recovered_line = recovered.recovered_prefix_line(second);
        let expected = local_reference_step(&mut reference, second, live);
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
