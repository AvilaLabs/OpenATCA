//! Guarded bounded OAF segment merger for the full-day H2 profile.
//!
//! Each file is scanned incrementally. A single decoded message frame is copied out at a time;
//! gzip and OAF buffers are fixed-size. At most one current frame plus bounded chunk output is
//! retained per input file while a deterministic k-way merge feeds the caller.

use crate::recordings_reader::{RecordedFrame, RecordingSource};
use oa_feed::{FrameError, FrameReader, FrameWorkBudget, FrameWorkStats, Kind, MAX_FRAME_LEN};
use oa_zip::{GzipReader, InflateLimits};
use std::collections::VecDeque;
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
const INPUT_CHUNK_BYTES: usize = 4 * 1024;
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
    pub physical_bytes: u64,
    pub expanded_bytes: u64,
    pub frame_work_units: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamDigest {
    pub source: RecordingSource,
    pub bytes: u64,
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceCoverage {
    pub source: RecordingSource,
    pub first_frame_receive_ns: Option<i64>,
    pub last_frame_receive_ns: Option<i64>,
    pub largest_gap_ns: i64,
    pub full_window_covered: bool,
}

/// Reconcile observed frame intervals for one source against a predeclared half-open window.
/// A coverage gap is reported as metadata; it does not erase comparisons over delivered rows.
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
    first_frame_receive_ns: Option<i64>,
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
            first_frame_receive_ns: None,
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
                .receive_window_ns
                .is_some_and(|(start, end)| frame.recv_unix_ns < start || frame.recv_unix_ns >= end)
            {
                return Err("OAF frame receive time falls outside the declared day window".into());
            }
            if self
                .prior_receipt_ns
                .is_some_and(|previous| frame.recv_unix_ns < previous)
            {
                return Err("OAF segment is not monotone by recorder receive time".into());
            }
            if let Some(previous) = self.prior_receipt_ns {
                let gap = frame
                    .recv_unix_ns
                    .checked_sub(previous)
                    .ok_or("receive gap overflow")?;
                self.max_frame_gap_ns = self.max_frame_gap_ns.max(gap);
            } else if self.first_frame_receive_ns.is_none() {
                self.first_frame_receive_ns = Some(frame.recv_unix_ns);
            }
            self.prior_receipt_ns = Some(frame.recv_unix_ns);
            budget.add_frame()?;
            self.counts.frames = self
                .counts
                .frames
                .checked_add(1)
                .ok_or("per-stream frame count overflow")?;
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
            bytes: self.physical_read,
            sha256,
            messages: self.counts.messages,
            first_frame_receive_ns: self.first_frame_receive_ns,
            last_frame_receive_ns: self.prior_receipt_ns,
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

/// Stream one predeclared half-open UTC receive-time interval. Unlike a windowing filter, this
/// refuses any message outside the interval so callers cannot accidentally report a partial day
/// as a complete day. `None` preserves the legacy general-purpose reader behavior above.
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

/// Adapt a bounded ordered OAF stream to normalized events without retaining the day. The visitor
/// receives complete issue-time groups in deterministic source/sequence order. This is suitable
/// for multiple independent runtime passes: each caller repeats the guarded source read and owns
/// its own runtime state.
pub fn stream_normalized_events(
    inputs: &[StreamInput],
    receive_window_ns: Option<(i64, i64)>,
    centre: [u8; 4],
    mut visit: impl FnMut(
        crate::input::NormalizedEvent,
        crate::recordings::EventProvenance,
    ) -> Result<(), String>,
) -> Result<NormalizedStreamSummary, String> {
    if !centre.iter().all(u8::is_ascii_alphanumeric) {
        return Err("normalization centre must be four ASCII alphanumeric bytes".into());
    }
    let mut sequence_starts = [0u64; 2];
    let mut pending: Vec<(
        crate::input::NormalizedEvent,
        crate::recordings::EventProvenance,
    )> = Vec::new();
    pending
        .try_reserve_exact(crate::session::MAX_EVENTS_PER_RECEIPT)
        .map_err(|_| "normalized event group allocation refused")?;
    let mut pending_receipt = None;
    let mut counts = NormalizedAdapterCounts::default();
    let recording = stream_oaf_segments_in_window(inputs, receive_window_ns, |frame| {
        let mut input_counts = crate::recordings_reader::RecordingCounts::default();
        input_counts.total_frames = 1;
        input_counts.total_decoded_bytes = u64::try_from(frame.payload.len())
            .map_err(|_| "message payload byte count overflow")?;
        input_counts.total_bytes = input_counts.total_decoded_bytes;
        let adapted = crate::recordings::adapt_recorded_frames_with_sequence_starts(
            std::slice::from_ref(&frame),
            input_counts,
            100_000,
            centre,
            sequence_starts,
        )
        .map_err(|error| format!("normalized adapter refused message: {error:?}"))?;
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
        let new_count =
            u64::try_from(adapted.events.len()).map_err(|_| "normalized event count overflow")?;
        counts.normalized_events = counts
            .normalized_events
            .checked_add(new_count)
            .ok_or("normalized event count overflow")?;
        if counts.normalized_events > 10_000_000 {
            return Err("normalized event full-day cap exceeded".into());
        }
        for (event, provenance) in adapted.events.into_iter().zip(adapted.provenance) {
            if pending_receipt.is_some_and(|prior| event.receipt_ns < prior) {
                return Err("normalized issue-time order regressed".into());
            }
            if pending_receipt.is_some_and(|prior| event.receipt_ns > prior) {
                flush_normalized_group(&mut pending, &mut pending_receipt, &mut visit)?;
            }
            if pending.len() >= crate::session::MAX_EVENTS_PER_RECEIPT {
                return Err("normalized issue-time group exceeds 256-event capacity".into());
            }
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
            pending_receipt = Some(event.receipt_ns);
            pending
                .try_reserve(1)
                .map_err(|_| "normalized event allocation refused")?;
            pending.push((event, provenance));
        }
        Ok(())
    })?;
    flush_normalized_group(&mut pending, &mut pending_receipt, &mut visit)?;
    Ok(NormalizedStreamSummary {
        recording,
        normalized_events: counts.normalized_events,
        quarantine_rows: counts.quarantine_rows,
        manual_handling_required: counts.manual_handling_required,
        unprocessed_frames: counts.unprocessed_frames,
    })
}

#[derive(Clone, Copy, Debug, Default)]
struct NormalizedAdapterCounts {
    normalized_events: u64,
    quarantine_rows: u64,
    manual_handling_required: u64,
    unprocessed_frames: u64,
}

fn flush_normalized_group(
    pending: &mut Vec<(
        crate::input::NormalizedEvent,
        crate::recordings::EventProvenance,
    )>,
    receipt: &mut Option<i64>,
    visit: &mut impl FnMut(
        crate::input::NormalizedEvent,
        crate::recordings::EventProvenance,
    ) -> Result<(), String>,
) -> Result<(), String> {
    pending.sort_by_key(|(event, _)| (event.source, event.sequence));
    for (event, provenance) in pending.drain(..) {
        visit(event, provenance)?;
    }
    *receipt = None;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{stream_oaf_segments, stream_oaf_segments_in_window, RecordingSource, StreamInput};
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

    fn write_adsb_message(
        file: &mut std::fs::File,
        sequence: u64,
        source_ns: i64,
        receive_ns: i64,
    ) -> Result<(), Box<dyn Error>> {
        let row = oa_adsb::Row {
            time_ns: source_ns,
            icao: 0xABCDEF,
            lat_e7: 419_000_000,
            lon_e7: -873_000_000,
            baro_alt_ft: 35_000,
            gs_dkt: 4_500,
            track_ddeg: 900,
            nic: 8,
            nac_p: 9,
            nac_v: 2,
            sil: 3,
            version: 2,
            ..oa_adsb::ROW_EMPTY
        };
        let mut reports = oa_adsb::Reports::default();
        oa_adsb::reports_push(&mut reports, &row);
        let mut payload = Vec::new();
        oa_adsb::encode_payload(&mut payload, &reports, 0, 1)
            .map_err(|_| std::io::Error::other("synthetic ADS-B encode failed"))?;
        let mut frame = vec![0u8; oa_feed::HEADER_LEN + payload.len() + oa_feed::TRAILER_LEN];
        let encoded = oa_feed::encode_frame(
            &mut frame,
            oa_feed::Kind::Message as u8,
            0,
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

    // REQ: OA-MIG-411
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

    // REQ: OA-MIG-411
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

    // REQ: OA-MIG-411
    #[test]
    fn declared_window_is_half_open_and_outside_messages_refuse() -> Result<(), Box<dyn Error>> {
        let start = 1_800_000_000_000_000_000i64;
        let end = start + 10_000_000_000;
        let path = temporary_path("window")?;
        let sfdps_path = temporary_path("window_sfdps")?;
        write_frames(&path, &[start, end - 1_000_000_000])?;
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
        assert_eq!(accepted?.counts.messages, 3);
        assert_eq!(seen, 3);

        let outside = temporary_path("window_end")?;
        let sfdps_outside = temporary_path("window_end_sfdps")?;
        write_frames(&outside, &[end])?;
        write_frames(&sfdps_outside, &[start])?;
        let input = [
            StreamInput {
                source: RecordingSource::Adsb,
                path: outside.clone(),
            },
            StreamInput {
                source: RecordingSource::Sfdps,
                path: sfdps_outside.clone(),
            },
        ];
        let refused = stream_oaf_segments_in_window(&input, Some((start, end)), |_| Ok(()));
        let _ = fs::remove_file(&outside);
        let _ = fs::remove_file(&sfdps_outside);
        assert!(refused.is_err());
        assert!(stream_oaf_segments_in_window(&input, Some((end, start)), |_| Ok(())).is_err());
        Ok(())
    }

    // REQ: OA-MIG-411
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
        let receive_ns = source_ns + 2_000_000_000;
        write_adsb_message(&mut file, 1, source_ns, receive_ns)?;
        write_adsb_message(&mut file, 2, source_ns + 1_000_000_000, receive_ns)?;
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
        assert!(events.iter().all(|row| row.1 == receive_ns));
        assert!(events.iter().all(|row| row.2 == receive_ns));
        Ok(())
    }

    // REQ: OA-MIG-411
    #[test]
    fn source_coverage_reports_unobserved_prefix_without_erasing_available_rows(
    ) -> Result<(), Box<dyn Error>> {
        use super::{source_coverage, StreamDigest};
        let start = 1_795_046_400_000_000_000i64;
        let end = 1_795_132_800_000_000_000i64;
        let streams = [
            StreamDigest {
                source: RecordingSource::Adsb,
                bytes: 10,
                sha256: [1; 32],
                messages: 1,
                first_frame_receive_ns: Some(start + 17 * 60 * 1_000_000_000),
                last_frame_receive_ns: Some(end - 1),
                max_frame_gap_ns: 0,
            },
            StreamDigest {
                source: RecordingSource::Sfdps,
                bytes: 10,
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
