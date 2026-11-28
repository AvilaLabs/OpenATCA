//! Guarded bounded OAF segment merger for the full-day H2 profile.
//!
//! Each file is scanned incrementally. A single decoded message frame is copied out at a time;
//! gzip and OAF buffers are fixed-size. At most one current frame plus bounded chunk output is
//! retained per input file while a deterministic k-way merge feeds the caller.

use crate::recordings_reader::{RecordedFrame, RecordingSource};
use oa_feed::{FrameError, FrameReader, FrameWorkBudget, FrameWorkStats, Kind, MAX_FRAME_LEN};
use oa_zip::{GzipReader, InflateLimits};
use std::cmp::{Ordering, Reverse};
use std::collections::{BinaryHeap, VecDeque};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

pub const MAX_H2_INPUT_STREAMS: usize = 32;
pub const MAX_H2_PHYSICAL_INPUT_BYTES: u64 = 16 * 1024 * 1024 * 1024;
pub const MAX_H2_EXPANDED_INPUT_BYTES: u64 = 64 * 1024 * 1024 * 1024;
pub const MAX_H2_INPUT_FRAMES: u64 = 20_000_000;
pub const MAX_H2_FRAME_WORK_UNITS: u64 = 128 * 1024 * 1024 * 1024;
pub const MAX_PENDING_FRAMES_PER_STREAM: usize = 4_096;
pub const MAX_PENDING_FRAME_BYTES_PER_STREAM: usize = 4 * 1024 * 1024;
/// Recorder-activity reporting threshold, separate from the G3 candidate's 1 s state-age rule.
pub const H2_RECORDER_ACTIVITY_MAX_GAP_NS: i64 = 60_000_000_000;
const INPUT_CHUNK_BYTES: usize = 4 * 1024;
const NS_PER_HOUR: i64 = 3_600_000_000_000;
const NS_PER_UTC_DAY: i64 = 24 * NS_PER_HOUR;
type StreamSortKey = (i64, u8, u64, u8, u64);
type SelectedStream = Option<(usize, StreamSortKey)>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamInput {
    pub source: RecordingSource,
    pub path: PathBuf,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StreamCounts {
    pub frames: u64,
    pub messages: u64,
    pub status_frames: u64,
    pub reject_frames: u64,
    pub outside_window_frames: u64,
    pub outside_window_messages: u64,
    pub outside_window_status_frames: u64,
    pub outside_window_reject_frames: u64,
    /// OAF1 archive ADS-B messages decoded before the requested start for quality warmup.
    pub adsb_warmup_frames: u64,
    pub physical_bytes: u64,
    pub expanded_bytes: u64,
    pub frame_work_units: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamDigest {
    pub source: RecordingSource,
    pub stream_index: u8,
    pub bytes: u64,
    pub expanded_bytes: u64,
    pub gzip: bool,
    pub sha256: [u8; 32],
    pub messages: u64,
    pub first_frame_receive_ns: Option<i64>,
    pub last_frame_receive_ns: Option<i64>,
    pub max_frame_gap_ns: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamSummary {
    pub counts: StreamCounts,
    pub streams: Vec<StreamDigest>,
}

/// Source-only SFDPS message-frame counts for one explicit UTC day.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UtcHourMessageCounts {
    pub utc_day_start_ns: i64,
    pub sfdps_messages_by_hour: [u64; 24],
    pub stream: StreamSummary,
}

/// Aggregate-only result of adapting OAF input through the existing normalized-event adapter.
/// A caller can repeat the guarded stream for an independent runtime; no day-sized event vector
/// or payload spool is created here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NormalizedStreamSummary {
    pub recording: StreamSummary,
    pub normalized_events: u64,
    pub quarantine_rows: u64,
    pub manual_handling_required: u64,
    pub unprocessed_frames: u64,
    /// Per-input-source diagnostics for adapter refusals, including oversized receipt groups.
    pub source_counts: [NormalizedInputCounts; 2],
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NormalizedInputCounts {
    pub message_frames: u64,
    pub candidate_events_before_receipt_filter: u64,
    pub max_candidate_events_per_frame: u64,
    /// OAF1 ADS-B rows excluded by their own source time, rather than the frame's first-report time.
    pub outside_window_events: u64,
    /// Excluded pre-start ADS-B rows processed only to warm carried quality state.
    pub adsb_warmup_events: u64,
    /// Current ADS-B rows processed without retaining new per-address quality state at the cap.
    pub adsb_quality_cache_capacity_refused: u64,
    pub receipt_groups_over_capacity: u64,
    pub receipt_group_events_refused: u64,
    pub quarantine_rows: u64,
    pub quarantine_reason_counts: [u64; oa_quarantine::REASON_COUNTER_COUNT],
    pub adsb_rows_in: u64,
    pub adsb_rows_dropped: u64,
    pub adsb_quality_removed: [u64; 13],
    pub adsb_ground_excluded: u64,
    pub adsb_unknown_ground_refused: u64,
    pub adsb_source_time_regressions: u64,
    pub adsb_rows_order_refused: u64,
    pub adsb_velocity_unavailable: u64,
    pub adsb_unknown_gufi_positions: u64,
    pub invalid_position_rows: u64,
    pub source_after_receipt: u64,
    pub sfdps_rows_decoded: u64,
    pub sfdps_rows_refused: u64,
    pub sfdps_rows_no_operations: u64,
    pub sfdps_operations_unusable: u64,
    pub sfdps_owner_unknown: u64,
    pub sfdps_rows_positive_address: u64,
    pub sfdps_rows_nonempty_callsign: u64,
    pub sfdps_emitted_operations: [u64; oa_flight::OP_COUNT],
    pub sfdps_unsupported_operations: [u64; oa_flight::OP_COUNT],
    pub invalid_operation_time: u64,
    pub invalid_operation_identity: u64,
    pub invalid_operation_centre: u64,
    pub invalid_operation_address: u64,
    pub sfdps_field_errors: u64,
    pub sfdps_rejected_messages: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NormalizedStreamProfile {
    Legacy,
    RecordedHour,
}

const MAX_PENDING_NORMALIZED_EVENTS: usize = crate::recordings::MAX_ADSB_ROWS_PER_ADAPTATION;

struct PendingNormalizedEvent {
    event: crate::input::NormalizedEvent,
    provenance: crate::recordings::EventProvenance,
    arrival_order: u64,
}

impl PendingNormalizedEvent {
    fn key(&self) -> (i64, crate::input::SourceKind, u64, u64) {
        (
            self.event.receipt_ns,
            self.event.source,
            self.event.sequence,
            self.arrival_order,
        )
    }
}

impl PartialEq for PendingNormalizedEvent {
    fn eq(&self, other: &Self) -> bool {
        self.key() == other.key()
    }
}

impl Eq for PendingNormalizedEvent {}

impl PartialOrd for PendingNormalizedEvent {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PendingNormalizedEvent {
    fn cmp(&self, other: &Self) -> Ordering {
        self.key().cmp(&other.key())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceCoverage {
    pub source: RecordingSource,
    pub first_frame_receive_ns: Option<i64>,
    pub last_frame_receive_ns: Option<i64>,
    pub largest_gap_ns: i64,
    pub full_window_covered: bool,
}

/// Reconcile OAF recorder-frame activity intervals (Message, Status, and Reject) against a
/// predeclared half-open window. This is not message-data coverage; status heartbeats can bridge
/// a frame-activity gap without proving source messages were present. Gaps never erase delivered
/// comparisons.
pub fn source_coverage(
    streams: &[StreamDigest],
    source: RecordingSource,
    window: (i64, i64),
    allowed_gap_ns: i64,
) -> Result<SourceCoverage, String> {
    let (start, end) = window;
    if start < 0 || start >= end || allowed_gap_ns < 0 {
        return Err("invalid source coverage window or gap limit".into());
    }
    let mut intervals = Vec::new();
    intervals
        .try_reserve_exact(streams.len())
        .map_err(|_| "coverage interval allocation refused")?;
    let mut largest_gap = 0i64;
    for stream in streams.iter().filter(|stream| stream.source == source) {
        match (stream.first_frame_receive_ns, stream.last_frame_receive_ns) {
            (Some(first), Some(last)) if first <= last => {
                intervals.push((first, last));
                largest_gap = largest_gap.max(stream.max_frame_gap_ns);
            }
            (None, None) => {}
            _ => return Err("input stream has inconsistent frame coverage clocks".into()),
        }
    }
    intervals.sort_unstable_by_key(|(first, _)| *first);
    let Some((first_frame_receive_ns, first_end)) = intervals.first().copied() else {
        return Ok(SourceCoverage {
            source,
            first_frame_receive_ns: None,
            last_frame_receive_ns: None,
            largest_gap_ns: end.checked_sub(start).ok_or("coverage span overflow")?,
            full_window_covered: false,
        });
    };
    let mut covered_end = first_end;
    largest_gap = largest_gap.max(first_frame_receive_ns.saturating_sub(start));
    for (interval_start, interval_end) in intervals.iter().copied().skip(1) {
        if interval_start > covered_end {
            largest_gap = largest_gap.max(
                interval_start
                    .checked_sub(covered_end)
                    .ok_or("coverage gap overflow")?,
            );
        }
        covered_end = covered_end.max(interval_end);
    }
    let last_frame_receive_ns = covered_end;
    largest_gap = largest_gap.max(
        end.saturating_sub(1)
            .saturating_sub(last_frame_receive_ns)
            .max(0),
    );
    Ok(SourceCoverage {
        source,
        first_frame_receive_ns: Some(first_frame_receive_ns),
        last_frame_receive_ns: Some(last_frame_receive_ns),
        largest_gap_ns: largest_gap,
        full_window_covered: largest_gap <= allowed_gap_ns,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Limits {
    physical_bytes: u64,
    expanded_bytes: u64,
    frames: u64,
    work_units: u64,
}

const H2_LIMITS: Limits = Limits {
    physical_bytes: MAX_H2_PHYSICAL_INPUT_BYTES,
    expanded_bytes: MAX_H2_EXPANDED_INPUT_BYTES,
    frames: MAX_H2_INPUT_FRAMES,
    work_units: MAX_H2_FRAME_WORK_UNITS,
};

struct GlobalBudget {
    counts: StreamCounts,
    limits: Limits,
}

impl GlobalBudget {
    fn add_physical(&mut self, amount: usize) -> Result<(), String> {
        let amount = u64::try_from(amount).map_err(|_| "physical byte count overflow")?;
        let next = self
            .counts
            .physical_bytes
            .checked_add(amount)
            .ok_or("physical byte count overflow")?;
        if next > self.limits.physical_bytes {
            return Err("H2 aggregate physical input byte cap exceeded".into());
        }
        self.counts.physical_bytes = next;
        Ok(())
    }

    fn add_expanded(&mut self, amount: usize) -> Result<(), String> {
        let amount = u64::try_from(amount).map_err(|_| "expanded byte count overflow")?;
        let next = self
            .counts
            .expanded_bytes
            .checked_add(amount)
            .ok_or("expanded byte count overflow")?;
        if next > self.limits.expanded_bytes {
            return Err("H2 aggregate expanded input byte cap exceeded".into());
        }
        self.counts.expanded_bytes = next;
        Ok(())
    }

    fn add_frame(&mut self) -> Result<(), String> {
        let next = self
            .counts
            .frames
            .checked_add(1)
            .ok_or("input frame count overflow")?;
        if next > self.limits.frames {
            return Err("H2 input frame cap exceeded".into());
        }
        self.counts.frames = next;
        Ok(())
    }

    fn add_work(&mut self, amount: u64) -> Result<(), String> {
        let next = self
            .counts
            .frame_work_units
            .checked_add(amount)
            .ok_or("frame work count overflow")?;
        if next > self.limits.work_units {
            return Err("H2 OAF work cap exceeded".into());
        }
        self.counts.frame_work_units = next;
        Ok(())
    }
}

struct OafStream<'a> {
    source: RecordingSource,
    stream_index: u8,
    path: PathBuf,
    file: File,
    expected_len: u64,
    receive_window_ns: Option<(i64, i64)>,
    physical_read: u64,
    expanded_read: u64,
    input_eof: bool,
    decoder_finished: bool,
    gzip: Option<GzipReader>,
    gzip_probe_done: bool,
    chunk: [u8; INPUT_CHUNK_BYTES],
    reader: FrameReader<'a>,
    pending: VecDeque<RecordedFrame>,
    pending_bytes: usize,
    hash: oa_sha256::Sha256,
    hash_final: Option<[u8; 32]>,
    prior_receipt_ns: Option<i64>,
    coverage_prior_receipt_ns: Option<i64>,
    first_frame_receive_ns: Option<i64>,
    last_frame_receive_ns: Option<i64>,
    max_frame_gap_ns: i64,
    prior_work: FrameWorkStats,
    counts: StreamCounts,
}

impl<'a> OafStream<'a> {
    fn new(
        source: RecordingSource,
        stream_index: u8,
        path: &Path,
        file: File,
        expected_len: u64,
        receive_window_ns: Option<(i64, i64)>,
        scratch: &'a mut [u8],
    ) -> Result<Self, String> {
        let reader = FrameReader::new(scratch).map_err(|_| "OAF frame buffer too small")?;
        let mut pending = VecDeque::new();
        pending
            .try_reserve_exact(MAX_PENDING_FRAMES_PER_STREAM.min(64))
            .map_err(|_| "OAF pending queue allocation refused")?;
        Ok(Self {
            source,
            stream_index,
            path: path.to_path_buf(),
            file,
            expected_len,
            receive_window_ns,
            physical_read: 0,
            expanded_read: 0,
            input_eof: false,
            decoder_finished: false,
            gzip: None,
            gzip_probe_done: false,
            chunk: [0; INPUT_CHUNK_BYTES],
            reader,
            pending,
            pending_bytes: 0,
            hash: oa_sha256::Sha256::new(),
            hash_final: None,
            prior_receipt_ns: None,
            coverage_prior_receipt_ns: None,
            first_frame_receive_ns: None,
            last_frame_receive_ns: None,
            max_frame_gap_ns: 0,
            prior_work: FrameWorkStats::default(),
            counts: StreamCounts::default(),
        })
    }

    fn note_work(&mut self, budget: &mut GlobalBudget) -> Result<(), String> {
        let now = self.reader.work_stats();
        let delta = now
            .scan_positions
            .checked_sub(self.prior_work.scan_positions)
            .and_then(|value| {
                value.checked_add(now.crc_bytes.checked_sub(self.prior_work.crc_bytes)?)
            })
            .and_then(|value| {
                value.checked_add(now.copied_bytes.checked_sub(self.prior_work.copied_bytes)?)
            })
            .and_then(|value| {
                value.checked_add(
                    now.compacted_bytes
                        .checked_sub(self.prior_work.compacted_bytes)?,
                )
            })
            .ok_or("OAF work counter regressed")?;
        budget.add_work(delta)?;
        self.counts.frame_work_units = self
            .counts
            .frame_work_units
            .checked_add(delta)
            .ok_or("per-stream OAF work count overflow")?;
        self.prior_work = now;
        Ok(())
    }

    fn drain_frames(&mut self, budget: &mut GlobalBudget) -> Result<(), String> {
        loop {
            let next = self
                .reader
                .next_with_budget(FrameWorkBudget::for_capacity(MAX_FRAME_LEN))
                .map_err(|error| format!("OAF work refusal: {error:?}"))?;
            let Some(next) = next else { break };
            let frame =
                next.map_err(|error: FrameError| format!("invalid OAF frame: {error:?}"))?;
            if frame.skipped_before != 0 {
                return Err(
                    "OAF resynchronization skipped bytes; full-day input is incomplete".into(),
                );
            }
            if frame.recv_unix_ns <= 0 || oa_sealed::in_sealed_window(frame.recv_unix_ns) {
                return Err("invalid or sealed OAF receive timestamp".into());
            }
            if self
                .prior_receipt_ns
                .is_some_and(|previous| frame.recv_unix_ns < previous)
            {
                return Err("OAF segment is not monotone by recorder receive time".into());
            }
            self.prior_receipt_ns = Some(frame.recv_unix_ns);
            budget.add_frame()?;
            self.counts.frames = self
                .counts
                .frames
                .checked_add(1)
                .ok_or("per-stream frame count overflow")?;
            let outside_window = self.receive_window_ns.is_some_and(|(start, end)| {
                frame.recv_unix_ns < start || frame.recv_unix_ns >= end
            });
            let archive_adsb_warmup = outside_window
                && self.source == RecordingSource::Adsb
                && frame.mono_ns().is_none()
                && frame.kind == Kind::Message
                && self.receive_window_ns.is_some_and(|(start, end)| {
                    frame.recv_unix_ns < start && frame.recv_unix_ns < end
                });
            if outside_window {
                self.counts.outside_window_frames = self
                    .counts
                    .outside_window_frames
                    .checked_add(1)
                    .ok_or("outside-window frame count overflow")?;
                budget.counts.outside_window_frames = budget
                    .counts
                    .outside_window_frames
                    .checked_add(1)
                    .ok_or("outside-window frame count overflow")?;
                if archive_adsb_warmup {
                    self.counts.adsb_warmup_frames = self
                        .counts
                        .adsb_warmup_frames
                        .checked_add(1)
                        .ok_or("ADS-B warmup frame count overflow")?;
                    budget.counts.adsb_warmup_frames = budget
                        .counts
                        .adsb_warmup_frames
                        .checked_add(1)
                        .ok_or("aggregate ADS-B warmup frame count overflow")?;
                }
                match frame.kind {
                    Kind::Message => {
                        self.counts.outside_window_messages = self
                            .counts
                            .outside_window_messages
                            .checked_add(1)
                            .ok_or("outside-window message count overflow")?;
                        budget.counts.outside_window_messages = budget
                            .counts
                            .outside_window_messages
                            .checked_add(1)
                            .ok_or("outside-window message count overflow")?;
                    }
                    Kind::Status => {
                        self.counts.status_frames = self
                            .counts
                            .status_frames
                            .checked_add(1)
                            .ok_or("status frame count overflow")?;
                        budget.counts.status_frames = budget
                            .counts
                            .status_frames
                            .checked_add(1)
                            .ok_or("status frame count overflow")?;
                        self.counts.outside_window_status_frames = self
                            .counts
                            .outside_window_status_frames
                            .checked_add(1)
                            .ok_or("outside-window status count overflow")?;
                        budget.counts.outside_window_status_frames = budget
                            .counts
                            .outside_window_status_frames
                            .checked_add(1)
                            .ok_or("outside-window status count overflow")?;
                    }
                    Kind::Reject => {
                        self.counts.reject_frames = self
                            .counts
                            .reject_frames
                            .checked_add(1)
                            .ok_or("reject frame count overflow")?;
                        budget.counts.reject_frames = budget
                            .counts
                            .reject_frames
                            .checked_add(1)
                            .ok_or("reject frame count overflow")?;
                        self.counts.outside_window_reject_frames = self
                            .counts
                            .outside_window_reject_frames
                            .checked_add(1)
                            .ok_or("outside-window reject count overflow")?;
                        budget.counts.outside_window_reject_frames = budget
                            .counts
                            .outside_window_reject_frames
                            .checked_add(1)
                            .ok_or("outside-window reject count overflow")?;
                    }
                }
                if !archive_adsb_warmup {
                    continue;
                }
            }
            if !outside_window {
                if let Some(previous) = self.coverage_prior_receipt_ns {
                    let gap = frame
                        .recv_unix_ns
                        .checked_sub(previous)
                        .ok_or("coverage gap overflow")?;
                    self.max_frame_gap_ns = self.max_frame_gap_ns.max(gap);
                } else {
                    self.first_frame_receive_ns = Some(frame.recv_unix_ns);
                }
                self.coverage_prior_receipt_ns = Some(frame.recv_unix_ns);
                self.last_frame_receive_ns = Some(frame.recv_unix_ns);
            }
            match frame.kind {
                Kind::Message => {
                    let payload_len = frame.payload.len();
                    let new_pending_bytes = self
                        .pending_bytes
                        .checked_add(payload_len)
                        .ok_or("pending frame byte count overflow")?;
                    if self.pending.len() >= MAX_PENDING_FRAMES_PER_STREAM
                        || new_pending_bytes > MAX_PENDING_FRAME_BYTES_PER_STREAM
                    {
                        return Err("per-stream OAF chunk output cap exceeded".into());
                    }
                    let mut payload = Vec::new();
                    payload
                        .try_reserve_exact(payload_len)
                        .map_err(|_| "OAF message allocation refused")?;
                    payload.extend_from_slice(frame.payload);
                    self.pending
                        .try_reserve(1)
                        .map_err(|_| "OAF pending queue allocation refused")?;
                    self.pending.push_back(RecordedFrame {
                        source: self.source,
                        stream_index: self.stream_index,
                        receipt_ns: frame.recv_unix_ns,
                        sequence: frame.seq,
                        jms_timestamp_ms: frame.jms_timestamp_ms,
                        mono_ns: frame.mono_ns(),
                        oaf_flags: frame.flags,
                        byte_offset: frame.offset,
                        skipped_before: frame.skipped_before,
                        kind: frame.kind,
                        payload,
                    });
                    self.pending_bytes = new_pending_bytes;
                    self.counts.messages = self
                        .counts
                        .messages
                        .checked_add(1)
                        .ok_or("message frame count overflow")?;
                }
                Kind::Status => {
                    self.counts.status_frames = self
                        .counts
                        .status_frames
                        .checked_add(1)
                        .ok_or("status frame count overflow")?;
                    budget.counts.status_frames = budget
                        .counts
                        .status_frames
                        .checked_add(1)
                        .ok_or("status frame count overflow")?;
                }
                Kind::Reject => {
                    self.counts.reject_frames = self
                        .counts
                        .reject_frames
                        .checked_add(1)
                        .ok_or("reject frame count overflow")?;
                    budget.counts.reject_frames = budget
                        .counts
                        .reject_frames
                        .checked_add(1)
                        .ok_or("reject frame count overflow")?;
                }
            }
        }
        self.note_work(budget)
    }

    fn feed_decoded(&mut self, mut bytes: &[u8], budget: &mut GlobalBudget) -> Result<(), String> {
        budget.add_expanded(bytes.len())?;
        self.expanded_read = self
            .expanded_read
            .checked_add(u64::try_from(bytes.len()).map_err(|_| "expanded byte count overflow")?)
            .ok_or("expanded byte count overflow")?;
        self.counts.expanded_bytes = self
            .counts
            .expanded_bytes
            .checked_add(u64::try_from(bytes.len()).map_err(|_| "expanded byte count overflow")?)
            .ok_or("per-stream expanded byte count overflow")?;
        while !bytes.is_empty() {
            let taken = self
                .reader
                .push_with_budget(bytes, FrameWorkBudget::for_capacity(MAX_FRAME_LEN))
                .map_err(|error| format!("OAF input work refusal: {error:?}"))?;
            if taken == 0 {
                self.drain_frames(budget)?;
                return Err("OAF reader could not make progress on bounded input".into());
            }
            bytes = bytes.get(taken..).ok_or("OAF input slicing failed")?;
            self.drain_frames(budget)?;
        }
        Ok(())
    }

    fn read_chunk(&mut self, budget: &mut GlobalBudget) -> Result<(), String> {
        if self.input_eof {
            return Ok(());
        }
        let mut chunk = std::mem::replace(&mut self.chunk, [0u8; INPUT_CHUNK_BYTES]);
        let read = self
            .file
            .read(&mut chunk)
            .map_err(|error| error.to_string())?;
        if read == 0 {
            self.chunk = chunk;
            self.input_eof = true;
            if self.physical_read != self.expected_len {
                return Err("input file changed while streaming".into());
            }
            if let Some(gzip) = self.gzip.as_ref() {
                let summary = gzip.finish();
                if summary.truncated.is_some() || summary.members == 0 {
                    return Err("gzip segment is truncated or contains no verified members".into());
                }
            }
            if self.gzip_probe_done && self.gzip.is_none() && self.expected_len == 0 {
                return Err("empty OAF segment refused".into());
            }
            if let Some(error) = self.reader.finish() {
                return Err(format!("truncated OAF segment: {error:?}"));
            }
            self.drain_frames(budget)?;
            self.decoder_finished = true;
            self.hash_final = Some(self.hash.clone().finalize());
            return Ok(());
        }
        budget.add_physical(read)?;
        self.physical_read = self
            .physical_read
            .checked_add(u64::try_from(read).map_err(|_| "physical byte count overflow")?)
            .ok_or("physical byte count overflow")?;
        if self.physical_read > self.expected_len {
            return Err("input file grew beyond guarded length".into());
        }
        let bytes = chunk.get(..read).ok_or("input chunk bounds")?;
        self.hash
            .update(bytes)
            .map_err(|error| format!("input hash update: {error:?}"))?;
        if !self.gzip_probe_done {
            self.gzip_probe_done = true;
            let has_magic = bytes.starts_with(&[0x1f, 0x8b]);
            let requires_gzip = self
                .path
                .extension()
                .is_some_and(|extension| extension == "gz");
            if requires_gzip && !has_magic {
                return Err(".gz input has no gzip header".into());
            }
            if has_magic {
                self.gzip = Some(GzipReader::new(InflateLimits {
                    max_output: H2_LIMITS.expanded_bytes,
                    max_ratio: 200,
                    ratio_exempt_bytes: 1024 * 1024,
                }));
            }
        }
        if let Some(mut gzip) = self.gzip.take() {
            let mut sink_error = None;
            let result = gzip.push(bytes, &mut |expanded: &[u8]| {
                if let Err(error) = self.feed_decoded(expanded, budget) {
                    sink_error = Some(error.clone());
                    return Err(error);
                }
                Ok::<(), String>(())
            });
            self.gzip = Some(gzip);
            if let Some(error) = sink_error {
                return Err(error);
            }
            result.map_err(|error| format!("gzip segment invalid: {error}"))?;
        } else {
            self.feed_decoded(bytes, budget)?;
        }
        self.counts.physical_bytes = self
            .counts
            .physical_bytes
            .checked_add(u64::try_from(read).map_err(|_| "physical byte count overflow")?)
            .ok_or("per-stream physical byte count overflow")?;
        self.chunk = chunk;
        Ok(())
    }

    fn next_message(&mut self, budget: &mut GlobalBudget) -> Result<Option<RecordedFrame>, String> {
        loop {
            if let Some(frame) = self.pending.pop_front() {
                self.pending_bytes = self
                    .pending_bytes
                    .checked_sub(frame.payload.len())
                    .ok_or("pending frame byte count underflow")?;
                return Ok(Some(frame));
            }
            if self.decoder_finished {
                return Ok(None);
            }
            self.read_chunk(budget)?;
        }
    }

    fn digest(&self) -> Result<StreamDigest, String> {
        let sha256 = self
            .hash_final
            .ok_or("input stream was not fully consumed")?;
        Ok(StreamDigest {
            source: self.source,
            stream_index: self.stream_index,
            bytes: self.physical_read,
            expanded_bytes: self.expanded_read,
            gzip: self.gzip.is_some(),
            sha256,
            messages: self.counts.messages,
            first_frame_receive_ns: self.first_frame_receive_ns,
            last_frame_receive_ns: self.last_frame_receive_ns,
            max_frame_gap_ns: self.max_frame_gap_ns,
        })
    }
}

fn sort_key(frame: &RecordedFrame) -> (i64, u8, u64, u8, u64) {
    (
        frame.receipt_ns,
        match frame.source {
            RecordingSource::Adsb => 0,
            RecordingSource::Sfdps => 1,
        },
        frame.sequence,
        frame.stream_index,
        frame.byte_offset,
    )
}

/// Stream and merge bounded OAF segments. Input streams must each be monotone; no source payload
/// or frame history is retained after the visitor returns. Every file is guarded before it is
/// opened and its SHA-256 is computed over the exact bytes read from that descriptor.
pub fn stream_oaf_segments(
    inputs: &[StreamInput],
    visit: impl FnMut(RecordedFrame) -> Result<(), String>,
) -> Result<StreamSummary, String> {
    stream_oaf_segments_in_window(inputs, None, visit)
}

/// Stream one predeclared half-open UTC receive-time interval. Frames outside the interval are
/// still included in byte/frame/work accounting and timestamp-order checks, then counted and
/// skipped before message payload normalization. `None` preserves the general-purpose behavior.
pub fn stream_oaf_segments_in_window(
    inputs: &[StreamInput],
    receive_window_ns: Option<(i64, i64)>,
    mut visit: impl FnMut(RecordedFrame) -> Result<(), String>,
) -> Result<StreamSummary, String> {
    if inputs.is_empty() || inputs.len() > MAX_H2_INPUT_STREAMS {
        return Err("H2 input stream count outside fixed bounds".into());
    }
    if receive_window_ns.is_some_and(|(start, end)| start < 0 || start >= end) {
        return Err("invalid H2 receive-time window".into());
    }
    if receive_window_ns.is_some()
        && (!inputs
            .iter()
            .any(|input| input.source == RecordingSource::Adsb)
            || !inputs
                .iter()
                .any(|input| input.source == RecordingSource::Sfdps))
    {
        return Err("full-day H2 partition requires both ADS-B and SFDPS streams".into());
    }
    let mut opened = Vec::new();
    opened
        .try_reserve_exact(inputs.len())
        .map_err(|_| "input descriptor allocation refused")?;
    let mut physical_preflight = 0u64;
    for input in inputs {
        let (file, len) = oa_sealed::open_regular(&input.path)
            .map_err(|error| format!("guarded input refused: {error}"))?;
        physical_preflight = physical_preflight
            .checked_add(len)
            .ok_or("physical input preflight overflow")?;
        if physical_preflight > MAX_H2_PHYSICAL_INPUT_BYTES {
            return Err("H2 aggregate physical input cap exceeded before reading".into());
        }
        opened.push((file, len));
    }
    let mut scratch = Vec::new();
    scratch
        .try_reserve_exact(inputs.len())
        .map_err(|_| "OAF scratch inventory allocation refused")?;
    for _ in inputs {
        let mut buffer = Vec::new();
        buffer
            .try_reserve_exact(MAX_FRAME_LEN)
            .map_err(|_| "OAF scratch allocation refused")?;
        buffer.resize(MAX_FRAME_LEN, 0);
        scratch.push(buffer);
    }
    let mut streams = Vec::new();
    streams
        .try_reserve_exact(inputs.len())
        .map_err(|_| "stream reader allocation refused")?;
    for (index, ((input, (file, len)), buffer)) in inputs
        .iter()
        .zip(opened)
        .zip(scratch.iter_mut())
        .enumerate()
    {
        let stream_index = u8::try_from(index).map_err(|_| "stream index overflow")?;
        streams.push(OafStream::new(
            input.source,
            stream_index,
            input.path.as_path(),
            file,
            len,
            receive_window_ns,
            buffer.as_mut_slice(),
        )?);
    }
    let mut budget = GlobalBudget {
        counts: StreamCounts::default(),
        limits: H2_LIMITS,
    };
    let mut heads = Vec::new();
    heads
        .try_reserve_exact(streams.len())
        .map_err(|_| "stream head allocation refused")?;
    heads.resize_with(streams.len(), || None);
    loop {
        for (index, stream) in streams.iter_mut().enumerate() {
            let head = heads.get_mut(index).ok_or("stream head invariant")?;
            if head.is_none() {
                *head = stream.next_message(&mut budget)?;
            }
        }
        let mut selected: SelectedStream = None;
        for (index, head) in heads.iter().enumerate() {
            let Some(frame) = head.as_ref() else { continue };
            let key = sort_key(frame);
            if selected.is_none_or(|(_, prior)| key < prior) {
                selected = Some((index, key));
            }
        }
        let Some((index, _)) = selected else { break };
        let frame = heads
            .get_mut(index)
            .and_then(Option::take)
            .ok_or("selected stream head disappeared")?;
        visit(frame)?;
        budget.counts.messages = budget
            .counts
            .messages
            .checked_add(1)
            .ok_or("aggregate message frame count overflow")?;
    }
    let mut digests = Vec::new();
    digests
        .try_reserve_exact(streams.len())
        .map_err(|_| "stream digest allocation refused")?;
    for stream in &streams {
        digests.push(stream.digest()?);
    }
    Ok(StreamSummary {
        counts: budget.counts,
        streams: digests,
    })
}

/// Count SFDPS Message frames in 24 UTC bins without decoding source payloads. Inputs are still
/// scanned by the guarded H2 reader, including frames outside the selected UTC day.
pub fn count_sfdps_messages_by_utc_hour(
    inputs: &[StreamInput],
    utc_day_start_ns: i64,
) -> Result<UtcHourMessageCounts, String> {
    if utc_day_start_ns < 0 || utc_day_start_ns % NS_PER_UTC_DAY != 0 {
        return Err("UTC histogram start must be a nonnegative UTC midnight".into());
    }
    let utc_day_end_ns = utc_day_start_ns
        .checked_add(NS_PER_UTC_DAY)
        .ok_or("UTC histogram day end overflow")?;
    let mut sfdps_messages_by_hour = [0u64; 24];
    let stream =
        stream_oaf_segments_in_window(inputs, Some((utc_day_start_ns, utc_day_end_ns)), |frame| {
            if frame.source != RecordingSource::Sfdps || frame.kind != Kind::Message {
                return Ok(());
            }
            let elapsed = frame
                .receipt_ns
                .checked_sub(utc_day_start_ns)
                .ok_or("UTC histogram frame offset overflow")?;
            let hour = usize::try_from(elapsed / NS_PER_HOUR)
                .map_err(|_| "UTC histogram hour index overflow")?;
            let count = sfdps_messages_by_hour
                .get_mut(hour)
                .ok_or("UTC histogram frame escaped its day window")?;
            *count = count
                .checked_add(1)
                .ok_or("UTC histogram message count overflow")?;
            Ok(())
        })?;
    Ok(UtcHourMessageCounts {
        utc_day_start_ns,
        sfdps_messages_by_hour,
        stream,
    })
}

/// Adapt a bounded ordered OAF stream to normalized events without retaining the day. The visitor
/// receives complete issue-time groups in deterministic source/sequence order. This is suitable
/// for multiple independent runtime passes: each caller repeats the guarded source read and owns
/// its own runtime state.
pub fn stream_normalized_events(
    inputs: &[StreamInput],
    receive_window_ns: Option<(i64, i64)>,
    centre: [u8; 4],
    visit: impl FnMut(
        crate::input::NormalizedEvent,
        crate::recordings::EventProvenance,
    ) -> Result<(), String>,
) -> Result<NormalizedStreamSummary, String> {
    stream_normalized_events_with_limit(
        inputs,
        receive_window_ns,
        centre,
        NormalizedStreamProfile::Legacy,
        visit,
    )
}

/// Offline recorded-hour variant; the caller must select this path explicitly for that profile.
pub fn stream_recorded_hour_normalized_events(
    inputs: &[StreamInput],
    receive_window_ns: Option<(i64, i64)>,
    centre: [u8; 4],
    visit: impl FnMut(
        crate::input::NormalizedEvent,
        crate::recordings::EventProvenance,
    ) -> Result<(), String>,
) -> Result<NormalizedStreamSummary, String> {
    stream_normalized_events_with_limit(
        inputs,
        receive_window_ns,
        centre,
        NormalizedStreamProfile::RecordedHour,
        visit,
    )
}

pub(crate) fn stream_normalized_events_with_limit(
    inputs: &[StreamInput],
    receive_window_ns: Option<(i64, i64)>,
    centre: [u8; 4],
    profile: NormalizedStreamProfile,
    mut visit: impl FnMut(
        crate::input::NormalizedEvent,
        crate::recordings::EventProvenance,
    ) -> Result<(), String>,
) -> Result<NormalizedStreamSummary, String> {
    let receipt_group_limit = match profile {
        NormalizedStreamProfile::Legacy => crate::session::MAX_EVENTS_PER_RECEIPT,
        NormalizedStreamProfile::RecordedHour => {
            crate::session::RECORDED_HOUR_MAX_EVENTS_PER_RECEIPT
        }
    };
    if !crate::input::is_valid_centre_code(&centre) {
        return Err("normalization centre must be a valid padded ASCII alphanumeric code".into());
    }
    let mut sequence_starts = [0u64; 2];
    let mut pending: Vec<(
        crate::input::NormalizedEvent,
        crate::recordings::EventProvenance,
    )> = Vec::new();
    pending
        .try_reserve_exact(receipt_group_limit)
        .map_err(|_| "normalized event group allocation refused")?;
    let mut pending_heap = BinaryHeap::new();
    pending_heap
        .try_reserve(receipt_group_limit)
        .map_err(|_| "normalized watermark queue allocation refused")?;
    let mut pending_arrival_order = 0u64;
    let mut normalized_sequences = [0u64; 5];
    let mut quality_cache = oa_track::adsb::AdsbQualityCache::default();
    let mut counts = NormalizedAdapterCounts::default();
    let recording = stream_oaf_segments_in_window(inputs, receive_window_ns, |frame| {
        let mut input_counts = crate::recordings_reader::RecordingCounts::default();
        input_counts.total_frames = 1;
        input_counts.total_decoded_bytes = u64::try_from(frame.payload.len())
            .map_err(|_| "message payload byte count overflow")?;
        input_counts.total_bytes = input_counts.total_decoded_bytes;
        let adapted =
            crate::recordings::adapt_recorded_frames_with_sequence_starts_and_quality_limit(
                std::slice::from_ref(&frame),
                input_counts,
                100_000,
                centre,
                sequence_starts,
                receipt_group_limit,
                &mut quality_cache,
            )
            .map_err(|error| format!("normalized adapter refused message: {error:?}"))?;
        let source_index = match frame.source {
            RecordingSource::Adsb => 0,
            RecordingSource::Sfdps => 1,
        };
        let source_counts = counts
            .source_counts
            .get_mut(source_index)
            .ok_or("normalized source index invariant")?;
        source_counts.message_frames = source_counts
            .message_frames
            .checked_add(1)
            .ok_or("source message-frame count overflow")?;
        source_counts.candidate_events_before_receipt_filter = source_counts
            .candidate_events_before_receipt_filter
            .checked_add(adapted.counts.candidate_events_before_receipt_filter)
            .ok_or("source candidate-event count overflow")?;
        source_counts.max_candidate_events_per_frame = source_counts
            .max_candidate_events_per_frame
            .max(adapted.counts.candidate_events_before_receipt_filter);
        source_counts.receipt_groups_over_capacity = source_counts
            .receipt_groups_over_capacity
            .checked_add(adapted.counts.receipt_groups_over_capacity)
            .ok_or("source over-capacity-group count overflow")?;
        source_counts.receipt_group_events_refused = source_counts
            .receipt_group_events_refused
            .checked_add(adapted.counts.receipt_group_events_refused)
            .ok_or("source refused-event count overflow")?;
        source_counts.adsb_quality_cache_capacity_refused = source_counts
            .adsb_quality_cache_capacity_refused
            .checked_add(adapted.counts.adsb_quality_cache_capacity_refused)
            .ok_or("ADS-B quality-cache capacity count overflow")?;
        source_counts.quarantine_rows = source_counts
            .quarantine_rows
            .checked_add(
                u64::try_from(adapted.quarantine.len())
                    .map_err(|_| "source quarantine row count overflow")?,
            )
            .ok_or("source quarantine row count overflow")?;
        for stats in &adapted.quarantine_stats {
            for (total, count) in source_counts
                .quarantine_reason_counts
                .iter_mut()
                .zip(stats.reason_counts)
            {
                *total = total
                    .checked_add(count)
                    .ok_or("source quarantine reason count overflow")?;
            }
        }
        accumulate_adapter_diagnostics(source_counts, &adapted.counts, source_index)?;
        counts.quarantine_rows = counts
            .quarantine_rows
            .checked_add(
                u64::try_from(adapted.quarantine.len())
                    .map_err(|_| "quarantine row count overflow")?,
            )
            .ok_or("quarantine row count overflow")?;
        counts.manual_handling_required = counts
            .manual_handling_required
            .checked_add(
                u64::try_from(
                    adapted
                        .quarantine
                        .iter()
                        .filter(|row| row.manual_handling_required)
                        .count(),
                )
                .map_err(|_| "manual-handling count overflow")?,
            )
            .ok_or("manual-handling count overflow")?;
        counts.unprocessed_frames = counts
            .unprocessed_frames
            .checked_add(adapted.counts.unprocessed_frames)
            .ok_or("unprocessed frame count overflow")?;
        for (event, provenance) in adapted.events.into_iter().zip(adapted.provenance) {
            let sequence_index = match event.source {
                crate::input::SourceKind::Surveillance => Some(0),
                crate::input::SourceKind::Flight | crate::input::SourceKind::Route => Some(1),
                crate::input::SourceKind::Weather | crate::input::SourceKind::Adaptation => None,
            };
            if let Some(index) = sequence_index {
                let sequence_start = sequence_starts
                    .get_mut(index)
                    .ok_or("normalized sequence source index invariant")?;
                *sequence_start = (*sequence_start).max(
                    event
                        .sequence
                        .checked_add(1)
                        .ok_or("normalized sequence overflow")?,
                );
            }
            let archive_adsb_window = (frame.source == RecordingSource::Adsb
                && frame.mono_ns.is_none())
            .then_some(receive_window_ns)
            .flatten();
            if let Some((start, end)) = archive_adsb_window {
                if event.source_ns < start || event.source_ns >= end {
                    source_counts.outside_window_events = source_counts
                        .outside_window_events
                        .checked_add(1)
                        .ok_or("outside-window row count overflow")?;
                    if event.source_ns < start {
                        source_counts.adsb_warmup_events = source_counts
                            .adsb_warmup_events
                            .checked_add(1)
                            .ok_or("ADS-B warmup row count overflow")?;
                    }
                    continue;
                }
            }
            counts.normalized_events = counts
                .normalized_events
                .checked_add(1)
                .ok_or("normalized event count overflow")?;
            if counts.normalized_events > 10_000_000 {
                return Err("normalized event full-day cap exceeded".into());
            }
            if pending_heap.len() >= MAX_PENDING_NORMALIZED_EVENTS {
                return Err("normalized watermark queue capacity exceeded".into());
            }
            pending_heap
                .try_reserve(1)
                .map_err(|_| "normalized watermark queue allocation refused")?;
            let arrival_order = pending_arrival_order;
            pending_arrival_order = pending_arrival_order
                .checked_add(1)
                .ok_or("normalized event order overflow")?;
            pending_heap.push(Reverse(PendingNormalizedEvent {
                event,
                provenance,
                arrival_order,
            }));
        }
        flush_watermarked_groups(
            &mut pending_heap,
            frame.receipt_ns,
            receipt_group_limit,
            &mut pending,
            &mut normalized_sequences,
            &mut visit,
        )?;
        Ok(())
    })?;
    flush_watermarked_groups(
        &mut pending_heap,
        i64::MAX,
        receipt_group_limit,
        &mut pending,
        &mut normalized_sequences,
        &mut visit,
    )?;
    Ok(NormalizedStreamSummary {
        recording,
        normalized_events: counts.normalized_events,
        quarantine_rows: counts.quarantine_rows,
        manual_handling_required: counts.manual_handling_required,
        unprocessed_frames: counts.unprocessed_frames,
        source_counts: counts.source_counts,
    })
}

fn accumulate_adapter_diagnostics(
    source: &mut NormalizedInputCounts,
    adapter: &crate::recordings::RecordedAdapterCounts,
    source_index: usize,
) -> Result<(), String> {
    macro_rules! add {
        ($field:ident) => {
            source.$field = source
                .$field
                .checked_add(adapter.$field)
                .ok_or(concat!(stringify!($field), " aggregate overflow"))?;
        };
    }
    match source_index {
        0 => {
            add!(adsb_rows_in);
            add!(adsb_rows_dropped);
            add!(adsb_ground_excluded);
            add!(adsb_unknown_ground_refused);
            add!(adsb_source_time_regressions);
            add!(adsb_rows_order_refused);
            add!(adsb_velocity_unavailable);
            add!(adsb_unknown_gufi_positions);
            add!(invalid_position_rows);
            add!(source_after_receipt);
            for (total, count) in source
                .adsb_quality_removed
                .iter_mut()
                .zip(adapter.adsb_quality_removed)
            {
                *total = total
                    .checked_add(count)
                    .ok_or("ADS-B quality diagnostic overflow")?;
            }
        }
        1 => {
            add!(sfdps_rows_decoded);
            add!(sfdps_rows_refused);
            add!(sfdps_rows_no_operations);
            add!(sfdps_operations_unusable);
            add!(sfdps_owner_unknown);
            add!(sfdps_rows_positive_address);
            add!(sfdps_rows_nonempty_callsign);
            add!(invalid_operation_time);
            add!(source_after_receipt);
            add!(invalid_operation_identity);
            add!(invalid_operation_centre);
            add!(invalid_operation_address);
            add!(sfdps_field_errors);
            add!(sfdps_rejected_messages);
            for (total, count) in source
                .sfdps_emitted_operations
                .iter_mut()
                .zip(adapter.sfdps_emitted_operations)
            {
                *total = total
                    .checked_add(count)
                    .ok_or("SFDPS emitted-operation diagnostic overflow")?;
            }
            for (total, count) in source
                .sfdps_unsupported_operations
                .iter_mut()
                .zip(adapter.unsupported_operations)
            {
                *total = total
                    .checked_add(count)
                    .ok_or("SFDPS unsupported-operation diagnostic overflow")?;
            }
        }
        _ => return Err("normalized source index invariant".into()),
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Default)]
struct NormalizedAdapterCounts {
    normalized_events: u64,
    quarantine_rows: u64,
    manual_handling_required: u64,
    unprocessed_frames: u64,
    source_counts: [NormalizedInputCounts; 2],
}

fn flush_normalized_group(
    pending: &mut Vec<(
        crate::input::NormalizedEvent,
        crate::recordings::EventProvenance,
    )>,
    normalized_sequences: &mut [u64; 5],
    visit: &mut impl FnMut(
        crate::input::NormalizedEvent,
        crate::recordings::EventProvenance,
    ) -> Result<(), String>,
) -> Result<(), String> {
    pending.sort_by_key(|(event, _)| (event.source, event.sequence));
    let mut current_message: Option<(u64, u64)> = None;
    for (mut event, provenance) in pending.drain(..) {
        let sequence_index = match event.source {
            crate::input::SourceKind::Surveillance => 0,
            crate::input::SourceKind::Flight => 1,
            crate::input::SourceKind::Route => 2,
            crate::input::SourceKind::Weather => 3,
            crate::input::SourceKind::Adaptation => 4,
        };
        let next_sequence = normalized_sequences
            .get_mut(sequence_index)
            .ok_or("normalized sequence source index invariant")?;
        event.sequence = *next_sequence;
        *next_sequence = next_sequence
            .checked_add(1)
            .ok_or("normalized sequence overflow")?;
        if event.source == crate::input::SourceKind::Flight {
            event.source_message_id = match event.source_message_id {
                Some(source_message_id) => {
                    let normalized_id = match current_message {
                        Some((prior_id, normalized_id)) if prior_id == source_message_id => {
                            normalized_id
                        }
                        _ => {
                            current_message = Some((source_message_id, event.sequence));
                            event.sequence
                        }
                    };
                    Some(normalized_id)
                }
                None => {
                    current_message = None;
                    None
                }
            };
        } else {
            current_message = None;
        }
        visit(event, provenance)?;
    }
    Ok(())
}

fn flush_watermarked_groups(
    heap: &mut BinaryHeap<Reverse<PendingNormalizedEvent>>,
    watermark_ns: i64,
    receipt_group_limit: usize,
    group: &mut Vec<(
        crate::input::NormalizedEvent,
        crate::recordings::EventProvenance,
    )>,
    normalized_sequences: &mut [u64; 5],
    visit: &mut impl FnMut(
        crate::input::NormalizedEvent,
        crate::recordings::EventProvenance,
    ) -> Result<(), String>,
) -> Result<(), String> {
    while heap
        .peek()
        .is_some_and(|Reverse(next)| next.event.receipt_ns < watermark_ns)
    {
        let receipt_ns = heap
            .peek()
            .map(|Reverse(next)| next.event.receipt_ns)
            .ok_or("normalized watermark queue lost its head")?;
        group.clear();
        while heap
            .peek()
            .is_some_and(|Reverse(next)| next.event.receipt_ns == receipt_ns)
        {
            if group.len() >= receipt_group_limit {
                return Err("normalized issue-time group exceeds selected profile capacity".into());
            }
            group
                .try_reserve(1)
                .map_err(|_| "normalized event group allocation refused")?;
            let Reverse(entry) = heap
                .pop()
                .ok_or("normalized watermark queue lost an event")?;
            group.push((entry.event, entry.provenance));
        }
        flush_normalized_group(group, normalized_sequences, visit)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        count_sfdps_messages_by_utc_hour, stream_oaf_segments, stream_oaf_segments_in_window,
        RecordingSource, StreamInput, NS_PER_HOUR, NS_PER_UTC_DAY,
    };
    use std::error::Error;
    use std::fs::{self, OpenOptions};
    use std::io::Write;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(1);

    fn temporary_path(label: &str) -> Result<PathBuf, Box<dyn Error>> {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        Ok(std::env::temp_dir().join(format!(
            "oa_h2_oaf_{label}_{}_{}.oaf",
            std::process::id(),
            id
        )))
    }

    fn write_frames(path: &PathBuf, times: &[i64]) -> Result<(), Box<dyn Error>> {
        let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
        for (sequence, time) in times.iter().copied().enumerate() {
            let mut bytes = vec![0; 64];
            let size = oa_feed::encode_frame(
                &mut bytes,
                1,
                0,
                u64::try_from(sequence + 1)?,
                time,
                time / 1_000_000,
                b"synthetic",
            )
            .ok_or_else(|| std::io::Error::other("synthetic OAF frame encoding failed"))?;
            bytes.truncate(size);
            file.write_all(&bytes)?;
        }
        file.sync_all()?;
        Ok(())
    }

    fn write_kind_frames(
        path: &PathBuf,
        frames: &[(oa_feed::Kind, i64)],
    ) -> Result<(), Box<dyn Error>> {
        let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
        for (sequence, (kind, time)) in frames.iter().copied().enumerate() {
            let mut bytes = vec![0; 64];
            let size = oa_feed::encode_frame(
                &mut bytes,
                kind as u8,
                0,
                u64::try_from(sequence + 1)?,
                time,
                time / 1_000_000,
                b"synthetic",
            )
            .ok_or_else(|| std::io::Error::other("synthetic OAF frame encoding failed"))?;
            bytes.truncate(size);
            file.write_all(&bytes)?;
        }
        file.sync_all()?;
        Ok(())
    }

    fn write_payload_frame(
        path: &PathBuf,
        kind: oa_feed::Kind,
        sequence: u64,
        time_ns: i64,
        payload: &[u8],
    ) -> Result<(), Box<dyn Error>> {
        let jms_ms = time_ns / 1_000_000;
        let mut bytes = vec![0; oa_feed::HEADER_LEN + payload.len() + oa_feed::TRAILER_LEN];
        let frame_len = oa_feed::encode_frame(
            &mut bytes,
            kind as u8,
            oa_feed::FLAG_BYTES,
            sequence,
            time_ns,
            jms_ms,
            payload,
        )
        .ok_or_else(|| std::io::Error::other("synthetic OAF payload encoding failed"))?;
        bytes.truncate(frame_len);
        fs::write(path, bytes)?;
        Ok(())
    }

    fn write_adsb_message(
        file: &mut std::fs::File,
        sequence: u64,
        source_ns: i64,
        receive_ns: i64,
    ) -> Result<(), Box<dyn Error>> {
        write_adsb_message_with_reports(file, sequence, source_ns, receive_ns, 1)
    }

    fn write_adsb_message_with_reports(
        file: &mut std::fs::File,
        sequence: u64,
        source_ns: i64,
        receive_ns: i64,
        report_count: usize,
    ) -> Result<(), Box<dyn Error>> {
        let row = oa_adsb::Row {
            time_ns: source_ns,
            icao: 0xABCDEF,
            lat_e7: 419_000_000,
            lon_e7: -873_000_000,
            baro_alt_ft: 35_000,
            gs_dkt: 4_500,
            track_ddeg: 900,
            vrate_fpm: 0,
            nic: 8,
            nac_p: 9,
            nac_v: 2,
            sil: 3,
            version: 2,
            source: 0,
            ..oa_adsb::ROW_EMPTY
        };
        let mut reports = oa_adsb::Reports::default();
        for _ in 0..report_count {
            oa_adsb::reports_push(&mut reports, &row);
        }
        let mut payload = Vec::new();
        oa_adsb::encode_payload(&mut payload, &reports, 0, report_count)
            .map_err(|_| std::io::Error::other("synthetic ADS-B encode failed"))?;
        let mut frame = vec![0u8; oa_feed::HEADER_LEN + payload.len() + oa_feed::TRAILER_LEN];
        let encoded = oa_feed::encode_frame(
            &mut frame,
            oa_feed::Kind::Message as u8,
            oa_feed::FLAG_BYTES,
            sequence,
            receive_ns,
            receive_ns / 1_000_000,
            &payload,
        )
        .ok_or_else(|| std::io::Error::other("synthetic OAF encode failed"))?;
        frame.truncate(encoded);
        file.write_all(&frame)?;
        Ok(())
    }

    fn write_adsb_row(
        file: &mut std::fs::File,
        sequence: u64,
        row: &oa_adsb::Row,
    ) -> Result<(), Box<dyn Error>> {
        write_adsb_rows(file, sequence, &[row])
    }

    fn write_adsb_rows(
        file: &mut std::fs::File,
        sequence: u64,
        rows: &[&oa_adsb::Row],
    ) -> Result<(), Box<dyn Error>> {
        let first = rows
            .first()
            .ok_or_else(|| std::io::Error::other("synthetic ADS-B frame needs a report"))?;
        let mut reports = oa_adsb::Reports::default();
        for row in rows {
            oa_adsb::reports_push(&mut reports, row);
        }
        let mut payload = Vec::new();
        oa_adsb::encode_payload(&mut payload, &reports, 0, rows.len())
            .map_err(|_| std::io::Error::other("synthetic ADS-B encode failed"))?;
        let mut frame = vec![0u8; oa_feed::HEADER_LEN + payload.len() + oa_feed::TRAILER_LEN];
        let encoded = oa_feed::encode_frame(
            &mut frame,
            oa_feed::Kind::Message as u8,
            oa_feed::FLAG_BYTES,
            sequence,
            first.time_ns,
            first.time_ns / 1_000_000,
            &payload,
        )
        .ok_or_else(|| std::io::Error::other("synthetic OAF encode failed"))?;
        frame.truncate(encoded);
        file.write_all(&frame)?;
        Ok(())
    }

    // REQ: OA-MIG-415
    #[test]
    fn bounded_segment_merge_streams_messages_in_receipt_order() -> Result<(), Box<dyn Error>> {
        let adsb = temporary_path("merge_adsb")?;
        let sfdps = temporary_path("merge_sfdps")?;
        write_frames(&adsb, &[3_000_000_000, 5_000_000_000])?;
        write_frames(&sfdps, &[4_000_000_000])?;
        let inputs = [
            StreamInput {
                source: RecordingSource::Adsb,
                path: adsb.clone(),
            },
            StreamInput {
                source: RecordingSource::Sfdps,
                path: sfdps.clone(),
            },
        ];
        let mut seen = Vec::new();
        let summary = stream_oaf_segments(&inputs, |frame| {
            seen.push((frame.source, frame.receipt_ns));
            Ok(())
        });
        let _ = fs::remove_file(&adsb);
        let _ = fs::remove_file(&sfdps);
        let summary = summary?;
        assert_eq!(
            seen,
            vec![
                (RecordingSource::Adsb, 3_000_000_000),
                (RecordingSource::Sfdps, 4_000_000_000),
                (RecordingSource::Adsb, 5_000_000_000),
            ]
        );
        assert_eq!(summary.counts.frames, 3);
        assert_eq!(summary.counts.messages, 3);
        assert_eq!(summary.streams.len(), 2);
        Ok(())
    }

    // REQ: OA-MIG-415
    #[test]
    fn nonmonotone_segment_refuses_before_claiming_full_stream() -> Result<(), Box<dyn Error>> {
        let path = temporary_path("nonmonotone")?;
        write_frames(&path, &[5_000_000_000, 4_000_000_000])?;
        let input = [StreamInput {
            source: RecordingSource::Adsb,
            path: path.clone(),
        }];
        let result = stream_oaf_segments(&input, |_| Ok(()));
        let _ = fs::remove_file(&path);
        assert!(result.is_err());
        Ok(())
    }

    // REQ: OA-MIG-415
    #[test]
    fn declared_window_counts_and_skips_physical_prefix_and_suffix() -> Result<(), Box<dyn Error>> {
        let start = 1_800_000_000_000_000_000i64;
        let end = start + 10_000_000_000;
        let path = temporary_path("window")?;
        let sfdps_path = temporary_path("window_sfdps")?;
        write_frames(
            &path,
            &[start - 1_000_000_000, start, end - 1_000_000_000, end],
        )?;
        write_frames(&sfdps_path, &[start])?;
        let input = [
            StreamInput {
                source: RecordingSource::Adsb,
                path: path.clone(),
            },
            StreamInput {
                source: RecordingSource::Sfdps,
                path: sfdps_path.clone(),
            },
        ];
        let mut seen = 0u64;
        let accepted = stream_oaf_segments_in_window(&input, Some((start, end)), |_| {
            seen += 1;
            Ok(())
        });
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(&sfdps_path);
        let accepted = accepted?;
        let expected_normalized_messages = [
            (RecordingSource::Adsb, start - 1_000_000_000),
            (RecordingSource::Adsb, start),
            (RecordingSource::Adsb, end - 1_000_000_000),
            (RecordingSource::Adsb, end),
            (RecordingSource::Sfdps, start),
        ]
        .into_iter()
        .filter(|(source, receipt_ns)| {
            *source == RecordingSource::Sfdps
                || (*source == RecordingSource::Adsb && *receipt_ns < end)
        })
        .count();
        assert_eq!(
            accepted.counts.messages,
            u64::try_from(expected_normalized_messages)?
        );
        assert_eq!(accepted.counts.frames, 5);
        assert_eq!(accepted.counts.outside_window_frames, 2);
        assert_eq!(accepted.counts.outside_window_messages, 2);
        assert_eq!(accepted.counts.adsb_warmup_frames, 1);
        assert_eq!(seen, accepted.counts.messages);
        let adsb_digest = accepted
            .streams
            .iter()
            .find(|stream| stream.source == RecordingSource::Adsb)
            .ok_or("ADSB digest missing")?;
        assert_eq!(adsb_digest.first_frame_receive_ns, Some(start));
        assert_eq!(adsb_digest.last_frame_receive_ns, Some(end - 1_000_000_000));
        assert!(stream_oaf_segments_in_window(&input, Some((end, start)), |_| Ok(())).is_err());
        Ok(())
    }

    // REQ: OA-MIG-415
    #[test]
    fn sfdps_hour_histogram_counts_only_message_frames_inside_explicit_utc_day(
    ) -> Result<(), Box<dyn Error>> {
        let utc_day_start_ns = NS_PER_UTC_DAY * 20_000;
        let utc_day_end_ns = utc_day_start_ns + NS_PER_UTC_DAY;
        let sfdps_messages = [
            utc_day_start_ns + NS_PER_HOUR,
            utc_day_start_ns + NS_PER_HOUR + 1,
            utc_day_start_ns + 23 * NS_PER_HOUR,
            utc_day_end_ns,
        ];
        let sfdps_path = temporary_path("hour_histogram_sfdps")?;
        let adsb_path = temporary_path("hour_histogram_adsb")?;
        write_kind_frames(
            &sfdps_path,
            &[
                (oa_feed::Kind::Message, utc_day_start_ns - 1),
                (oa_feed::Kind::Message, sfdps_messages[0]),
                (oa_feed::Kind::Message, sfdps_messages[1]),
                (oa_feed::Kind::Status, utc_day_start_ns + 2 * NS_PER_HOUR),
                (oa_feed::Kind::Message, sfdps_messages[2]),
                (oa_feed::Kind::Message, sfdps_messages[3]),
            ],
        )?;
        write_frames(&adsb_path, &[utc_day_start_ns + NS_PER_HOUR])?;
        let inputs = [
            StreamInput {
                source: RecordingSource::Sfdps,
                path: sfdps_path.clone(),
            },
            StreamInput {
                source: RecordingSource::Adsb,
                path: adsb_path.clone(),
            },
        ];

        let result = count_sfdps_messages_by_utc_hour(&inputs, utc_day_start_ns);
        let invalid_day_start = count_sfdps_messages_by_utc_hour(&inputs, utc_day_start_ns + 1);
        let _ = fs::remove_file(&sfdps_path);
        let _ = fs::remove_file(&adsb_path);
        let result = result?;

        let mut expected = [0u64; 24];
        for receipt_ns in sfdps_messages
            .into_iter()
            .filter(|time| *time >= utc_day_start_ns && *time < utc_day_end_ns)
        {
            let hour = usize::try_from((receipt_ns - utc_day_start_ns) / NS_PER_HOUR)?;
            let count = expected.get_mut(hour).ok_or("fixture hour is in range")?;
            *count += 1;
        }
        // Mutation killed: counting non-Message frames or letting either UTC-day boundary in.
        assert_eq!(result.sfdps_messages_by_hour, expected);
        assert_eq!(result.utc_day_start_ns, utc_day_start_ns);
        assert_eq!(result.stream.streams.len(), inputs.len());
        for (index, digest) in result.stream.streams.iter().enumerate() {
            let input = inputs.get(index).ok_or("stream digest order is bounded")?;
            assert_eq!(usize::from(digest.stream_index), index);
            assert_eq!(digest.source, input.source);
            assert_eq!(digest.expanded_bytes, digest.bytes);
            assert!(!digest.gzip);
        }
        // Mutation killed: accepting a non-midnight histogram origin.
        assert!(invalid_day_start.is_err());
        Ok(())
    }

    // REQ: OA-MIG-415
    #[test]
    fn sfdps_hour_histogram_keeps_the_sealed_receipt_guard() -> Result<(), Box<dyn Error>> {
        let sealed_day_start_ns = 1_795_132_800_000_000_000i64;
        let sfdps_path = temporary_path("hour_histogram_sealed_sfdps")?;
        let adsb_path = temporary_path("hour_histogram_sealed_adsb")?;
        write_kind_frames(
            &sfdps_path,
            &[(oa_feed::Kind::Message, sealed_day_start_ns + NS_PER_HOUR)],
        )?;
        write_frames(&adsb_path, &[sealed_day_start_ns - 1])?;
        let inputs = [
            StreamInput {
                source: RecordingSource::Sfdps,
                path: sfdps_path.clone(),
            },
            StreamInput {
                source: RecordingSource::Adsb,
                path: adsb_path.clone(),
            },
        ];

        let result = count_sfdps_messages_by_utc_hour(&inputs, sealed_day_start_ns);
        let _ = fs::remove_file(&sfdps_path);
        let _ = fs::remove_file(&adsb_path);
        // Mutation killed: bypassing the sealed-receipt guard on the histogram reader.
        assert!(result.is_err());
        Ok(())
    }

    // REQ: OA-MIG-415
    #[test]
    fn normalized_stream_carries_sequences_and_exact_receive_provenance(
    ) -> Result<(), Box<dyn Error>> {
        use super::stream_normalized_events;
        use std::fs::OpenOptions;

        let path = temporary_path("normalized")?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        let source_ns = 1_893_456_780_000_000_000i64;
        let first_receive_ns = source_ns;
        let second_receive_ns = source_ns + 1_000_000_000;
        write_adsb_message(&mut file, 1, source_ns, first_receive_ns)?;
        write_adsb_message(&mut file, 2, source_ns + 1_000_000_000, second_receive_ns)?;
        file.sync_all()?;
        drop(file);
        let inputs = [StreamInput {
            source: RecordingSource::Adsb,
            path: path.clone(),
        }];
        let mut events = Vec::new();
        let result = stream_normalized_events(&inputs, None, *b"ZAU1", |event, provenance| {
            events.push((
                event.sequence,
                event.receipt_ns,
                provenance.input_receipt_ns,
            ));
            Ok(())
        });
        let _ = fs::remove_file(&path);
        let summary = result?;
        assert_eq!(summary.recording.counts.messages, 2);
        assert_eq!(summary.normalized_events, events.len() as u64);
        assert_eq!(events.len(), 2);
        let (first, second) = events
            .split_first()
            .and_then(|(first, tail)| tail.first().map(|second| (first, second)))
            .ok_or("synthetic normalized stream omitted expected events")?;
        assert!(first.0 < second.0);
        assert_eq!(first.1, source_ns);
        assert_eq!(second.1, second_receive_ns);
        assert_eq!(first.2, first_receive_ns);
        assert_eq!(second.2, second_receive_ns);
        Ok(())
    }

    #[test]
    fn archive_adsb_window_uses_row_time_and_prestart_frames_warm_quality(
    ) -> Result<(), Box<dyn Error>> {
        use super::stream_normalized_events;
        use std::fs::OpenOptions;

        let path = temporary_path("adsb_warmup_window")?;
        let sfdps_path = temporary_path("adsb_warmup_window_sfdps")?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        let start_ns = 1_893_456_780_000_000_000i64;
        let end_ns = start_ns + 20_000_000_000;
        let first = oa_adsb::Row {
            time_ns: start_ns - 1_000_000_000,
            icao: 0xABCDEF,
            lat_e7: 419_000_000,
            lon_e7: -873_000_000,
            baro_alt_ft: 35_000,
            gs_dkt: 4_500,
            track_ddeg: 900,
            vrate_fpm: 0,
            nic: 8,
            nac_p: 9,
            nac_v: 2,
            sil: 3,
            version: 2,
            source: 0,
            ..oa_adsb::ROW_EMPTY
        };
        let mut in_window = oa_adsb::Row {
            time_ns: start_ns + 1_000_000_000,
            ..first
        };
        in_window.nac_p = oa_adsb::NONE_U8;
        in_window.nic = oa_adsb::NONE_U8;
        in_window.sil = oa_adsb::NONE_U8;
        in_window.version = oa_adsb::NONE_U8;
        in_window.nac_v = oa_adsb::NONE_U8;
        write_adsb_row(&mut file, 1, &first)?;
        write_adsb_row(&mut file, 2, &in_window)?;
        file.sync_all()?;
        drop(file);

        write_kind_frames(&sfdps_path, &[(oa_feed::Kind::Status, start_ns)])?;
        let inputs = [
            StreamInput {
                source: RecordingSource::Adsb,
                path: path.clone(),
            },
            StreamInput {
                source: RecordingSource::Sfdps,
                path: sfdps_path.clone(),
            },
        ];
        let mut source_times = Vec::new();
        let result =
            stream_normalized_events(&inputs, Some((start_ns, end_ns)), *b"ZAU1", |event, _| {
                source_times.push(event.source_ns);
                Ok(())
            });
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(&sfdps_path);
        let summary = result?;
        assert_eq!(source_times, vec![start_ns + 1_000_000_000]);
        assert_eq!(summary.normalized_events, 1);
        assert_eq!(summary.recording.counts.adsb_warmup_frames, 1);
        assert_eq!(summary.source_counts[0].adsb_warmup_events, 1);
        assert_eq!(summary.source_counts[0].outside_window_events, 1);
        assert_eq!(summary.quarantine_rows, 0);
        Ok(())
    }

    #[test]
    fn archive_issue_groups_wait_for_global_header_watermark() -> Result<(), Box<dyn Error>> {
        use super::stream_normalized_events;
        use crate::session::{
            flush_receipt_group, push_event, session_cursor_new_with_profile_limits,
            MAX_EVENTS_PER_RECEIPT,
        };
        use std::fs::OpenOptions;

        let path = temporary_path("adsb_issue_watermark")?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        let base = 1_893_456_780_000_000_000i64;
        let first = oa_adsb::Row {
            time_ns: base + 100_000_000,
            icao: 0xABCDEF,
            lat_e7: 419_000_000,
            lon_e7: -873_000_000,
            baro_alt_ft: 35_000,
            gs_dkt: 4_500,
            track_ddeg: 900,
            vrate_fpm: 0,
            nic: 8,
            nac_p: 9,
            nac_v: 2,
            sil: 3,
            version: 2,
            source: 0,
            ..oa_adsb::ROW_EMPTY
        };
        let later_in_first_frame = oa_adsb::Row {
            icao: 0x123456,
            time_ns: base + 2_100_000_000,
            ..first
        };
        let middle_frame_row = oa_adsb::Row {
            icao: 0x234567,
            time_ns: base + 1_100_000_000,
            ..first
        };
        write_adsb_rows(&mut file, 1, &[&first, &later_in_first_frame])?;
        write_adsb_row(&mut file, 2, &middle_frame_row)?;
        file.sync_all()?;
        drop(file);

        let inputs = [StreamInput {
            source: RecordingSource::Adsb,
            path: path.clone(),
        }];
        let mut cursor = session_cursor_new_with_profile_limits(16, MAX_EVENTS_PER_RECEIPT)
            .map_err(|error| std::io::Error::other(format!("cursor init: {error:?}")))?;
        let mut observed = Vec::new();
        let mut completed = Vec::new();
        let result = stream_normalized_events(&inputs, None, *b"ZAU1", |event, provenance| {
            observed.push((
                event,
                event.source_message_id,
                provenance.frame_sequence,
                provenance.input_receipt_ns,
                provenance.emitted_item_ordinal,
            ));
            Ok(())
        });
        let _ = fs::remove_file(&path);
        let summary = result?;
        let normalized_events: Vec<_> = observed.iter().map(|entry| entry.0).collect();
        assert_eq!(summary.normalized_events, 3);
        assert_eq!(normalized_events.len(), 3);
        let observed_rows: Vec<_> = observed
            .iter()
            .map(
                |(event, source_message_id, frame_sequence, input_receipt_ns, ordinal)| {
                    (
                        event.receipt_ns,
                        event.source_ns,
                        event.sequence,
                        *source_message_id,
                        *frame_sequence,
                        *input_receipt_ns,
                        *ordinal,
                    )
                },
            )
            .collect();
        assert_eq!(
            observed_rows,
            vec![
                (
                    base + 1_000_000_000,
                    base + 100_000_000,
                    0,
                    None,
                    1,
                    base + 100_000_000,
                    0,
                ),
                (
                    base + 2_000_000_000,
                    base + 1_100_000_000,
                    1,
                    None,
                    2,
                    base + 1_100_000_000,
                    0,
                ),
                (
                    base + 3_000_000_000,
                    base + 2_100_000_000,
                    2,
                    None,
                    1,
                    base + 100_000_000,
                    1,
                ),
            ]
        );
        let serialized = crate::recordings::serialize_recorded_events(&normalized_events)
            .map_err(|error| std::io::Error::other(format!("serialize normalized rows: {error:?}")))?;
        let reparsed: Vec<_> = serialized
            .lines()
            .skip(1)
            .map(crate::input::parse_event_line)
            .collect::<Result<_, _>>()
            .map_err(|error| std::io::Error::other(format!("parse normalized row: {error:?}")))?;
        assert_eq!(reparsed, normalized_events);
        for event in reparsed {
            let batch = push_event(&mut cursor, event).map_err(|error| {
                std::io::Error::other(format!("session rejected event: {error:?}"))
            })?;
            if let Some(batch) = batch {
                completed.push(batch);
            }
        }
        if let Some(batch) = flush_receipt_group(&mut cursor)
            .map_err(|error| std::io::Error::other(format!("session flush: {error:?}")))?
        {
            completed.push(batch);
        }
        let completed_rows: Vec<_> = completed
            .iter()
            .flat_map(|batch| {
                batch
                    .events
                    .iter()
                    .map(|event| (batch.receipt_ns, event.source_ns, event.sequence))
            })
            .collect();
        let expected_session_rows = [
            (base + 1_000_000_000, base + 100_000_000, 0),
            (base + 2_000_000_000, base + 1_100_000_000, 1),
            (base + 3_000_000_000, base + 2_100_000_000, 2),
        ];
        assert_eq!(completed_rows, expected_session_rows);
        Ok(())
    }

    #[test]
    fn same_cutoff_sfdps_messages_keep_srcmsg_groups_through_runtime() -> Result<(), Box<dyn Error>>
    {
        use super::stream_normalized_events;
        use crate::session::{flush_receipt_group, push_event, session_cursor_new};

        let path = temporary_path("sfdps_srcmsg_groups")?;
        let payload = br#"<ns5:MessageCollection xmlns:ns5="http://www.faa.aero/nas/3.0" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"><message xsi:type="ns5:FlightMessageType"><flight xsi:type="ns5:NasFlightType" centre="ZXX" source="AH" system="SLC" timestamp="2030-01-02T03:04:05.000Z"><gufi codeSpace="urn:uuid">11111111-2222-3333-4444-555555555555</gufi><flightPlan identifier="FP7"/><flightStatus fdpsFlightStatus="ACTIVE"/><departure><runwayPositionAndTime><runwayTime><actual time="2030-01-02T03:00:00Z"/></runwayTime></runwayPositionAndTime></departure><controllingUnit unitIdentifier="???"/></flight></message><message xsi:type="ns5:FlightMessageType"><flight xsi:type="ns5:NasFlightType" centre="ZXX" source="TH" system="SLC" timestamp="2030-01-02T03:04:06.000Z"><gufi codeSpace="urn:uuid">11111111-2222-3333-4444-555555555555</gufi><flightPlan identifier="FP7"/><flightStatus fdpsFlightStatus="ACTIVE"/><flightIdentification aircraftIdentification="ABC123"/><aircraftDescription aircraftAddress="ABCDEF"/><controllingUnit unitIdentifier="ZXX" sectorIdentifier="03"/></flight></message></ns5:MessageCollection>"#;
        let receipt_ms = oa_fixm::parse_iso_ms(b"2030-01-02T03:04:07.250Z")
            .ok_or_else(|| std::io::Error::other("parse fixture receipt"))?;
        let receipt_ns = receipt_ms
            .checked_mul(1_000_000)
            .ok_or_else(|| std::io::Error::other("fixture receipt overflow"))?;
        write_payload_frame(&path, oa_feed::Kind::Message, 1, receipt_ns, payload)?;
        let inputs = [StreamInput {
            source: RecordingSource::Sfdps,
            path: path.clone(),
        }];
        let mut events = Vec::new();
        let summary = stream_normalized_events(&inputs, None, *b"ZAU1", |event, _| {
            events.push(event);
            Ok(())
        });
        let _ = fs::remove_file(&path);
        let summary = summary?;
        assert_eq!(summary.normalized_events, events.len() as u64);
        assert!(!events.is_empty());
        assert!(events
            .iter()
            .all(|event| event.receipt_ns == events[0].receipt_ns));

        let mut groups: Vec<(u64, u64, usize)> = Vec::new();
        for event in &events {
            let message_id = event
                .source_message_id
                .ok_or_else(|| std::io::Error::other("SFDPS operation lacks SRCMSG"))?;
            if let Some((current_id, first_sequence, count)) = groups.last_mut() {
                if *current_id == message_id {
                    *count += 1;
                    continue;
                }
            }
            groups.push((message_id, event.sequence, 1));
        }
        assert_eq!(groups.len(), 2);
        assert!(groups.iter().all(|(message_id, first_sequence, count)| {
            message_id == first_sequence && *count > 1
        }));
        assert_ne!(groups[0].0, groups[1].0);

        let serialized = crate::recordings::serialize_recorded_events(&events)
            .map_err(|error| std::io::Error::other(format!("serialize flight events: {error:?}")))?;
        let reparsed: Vec<_> = serialized
            .lines()
            .skip(1)
            .map(crate::input::parse_event_line)
            .collect::<Result<_, _>>()
            .map_err(|error| std::io::Error::other(format!("parse flight event: {error:?}")))?;
        assert_eq!(reparsed, events);
        let mut cursor = session_cursor_new();
        let mut batches = Vec::new();
        for event in reparsed {
            if let Some(batch) = push_event(&mut cursor, event)
                .map_err(|error| std::io::Error::other(format!("session admission: {error:?}")))?
            {
                batches.push(batch);
            }
        }
        if let Some(batch) = flush_receipt_group(&mut cursor)
            .map_err(|error| std::io::Error::other(format!("session flush: {error:?}")))?
        {
            batches.push(batch);
        }
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].events.len(), events.len());

        let model = crate::cli::synthetic_model()?;
        let snapshot = oa_airspace::core::snapshot::encode(&model)?;
        let mut runtime = crate::runtime::regional_new(
            model,
            oa_sha256::sha256(&snapshot),
            *b"ZAU1",
            crate::surveillance::SurveillanceConfig {
                centre_lat_e7: 410_000_000,
                centre_lon_e7: -870_000_000,
                range_nm: 180.0,
            },
        )
        .map_err(std::io::Error::other)?;
        let frame = crate::runtime::regional_step(&mut runtime, &batches[0])
            .map_err(std::io::Error::other)?;
        assert_eq!(frame.flight_operations.len(), events.len());
        assert!(frame
            .flight_data
            .iter()
            .any(|view| view.gufi == events[0].identity.flight_gufi));
        Ok(())
    }

    #[test]
    fn normalized_stream_reports_oversized_adsb_receipt_group_aggregates(
    ) -> Result<(), Box<dyn Error>> {
        use super::stream_normalized_events;
        use std::fs::OpenOptions;

        let path = temporary_path("oversized_receipt")?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        let source_ns = 1_893_456_780_000_000_000i64;
        let receive_ns = source_ns;
        write_adsb_message_with_reports(&mut file, 1, source_ns, receive_ns, 300)?;
        file.sync_all()?;
        drop(file);
        let inputs = [StreamInput {
            source: RecordingSource::Adsb,
            path: path.clone(),
        }];
        let mut visited = 0u64;
        let result = stream_normalized_events(&inputs, None, *b"ZAU1", |_, _| {
            visited += 1;
            Ok(())
        });
        let _ = fs::remove_file(&path);
        let summary = result?;
        let adsb = summary.source_counts[0];
        assert_eq!(adsb.message_frames, 1);
        assert!(adsb.candidate_events_before_receipt_filter > 256);
        assert_eq!(
            adsb.max_candidate_events_per_frame,
            adsb.candidate_events_before_receipt_filter
        );
        assert_eq!(adsb.receipt_groups_over_capacity, 1);
        assert_eq!(
            adsb.receipt_group_events_refused,
            adsb.candidate_events_before_receipt_filter
        );
        assert_eq!(visited, 0);
        Ok(())
    }

    #[test]
    fn recorded_hour_normalizer_uses_explicit_8192_receipt_group_profile(
    ) -> Result<(), Box<dyn Error>> {
        use super::stream_recorded_hour_normalized_events;
        use std::fs::OpenOptions;

        let path = temporary_path("recorded_hour_group")?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        let source_ns = 1_893_456_780_000_000_000i64;
        write_adsb_message_with_reports(&mut file, 1, source_ns, source_ns, 300)?;
        file.sync_all()?;
        drop(file);
        let inputs = [StreamInput {
            source: RecordingSource::Adsb,
            path: path.clone(),
        }];
        let mut visited = 0u64;
        let result = stream_recorded_hour_normalized_events(&inputs, None, *b"ZAU1", |_, _| {
            visited += 1;
            Ok(())
        });
        let _ = fs::remove_file(&path);
        let summary = result?;
        assert_eq!(summary.source_counts[0].receipt_groups_over_capacity, 0);
        assert_eq!(summary.source_counts[0].receipt_group_events_refused, 0);
        assert_eq!(summary.normalized_events, 300);
        assert_eq!(visited, 300);
        Ok(())
    }

    // REQ: OA-MIG-415
    #[test]
    fn source_coverage_reports_unobserved_prefix_without_erasing_available_rows(
    ) -> Result<(), Box<dyn Error>> {
        use super::{source_coverage, StreamDigest};
        let start = 1_795_046_400_000_000_000i64;
        let end = 1_795_132_800_000_000_000i64;
        let streams = [
            StreamDigest {
                source: RecordingSource::Adsb,
                stream_index: 0,
                bytes: 10,
                expanded_bytes: 10,
                gzip: false,
                sha256: [1; 32],
                messages: 1,
                first_frame_receive_ns: Some(start + 17 * 60 * 1_000_000_000),
                last_frame_receive_ns: Some(end - 1),
                max_frame_gap_ns: 0,
            },
            StreamDigest {
                source: RecordingSource::Sfdps,
                stream_index: 1,
                bytes: 10,
                expanded_bytes: 10,
                gzip: false,
                sha256: [2; 32],
                messages: 1,
                first_frame_receive_ns: Some(start),
                last_frame_receive_ns: Some(end - 1),
                max_frame_gap_ns: 0,
            },
        ];
        let adsb = source_coverage(
            &streams,
            RecordingSource::Adsb,
            (start, end),
            60_000_000_000,
        )?;
        let sfdps = source_coverage(
            &streams,
            RecordingSource::Sfdps,
            (start, end),
            60_000_000_000,
        )?;
        assert!(!adsb.full_window_covered);
        assert_eq!(
            adsb.first_frame_receive_ns,
            Some(start + 17 * 60 * 1_000_000_000)
        );
        assert!(sfdps.full_window_covered);
        Ok(())
    }
}
