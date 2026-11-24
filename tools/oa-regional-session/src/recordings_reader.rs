//! Bounded `.oaf` recording reader and deterministic multi-source frame merge. Shell I/O.
//! The source kind must be supplied by the caller; the OAF wire header itself does not identify
//! which SWIM feed a stream belongs to (RULESET B1, B3, C6, C7, E6).

use std::io::{self, Read};
use std::path::PathBuf;

use oa_feed::{Frame, FrameError, FrameReader, FrameWorkBudget, Kind, MAX_FRAME_LEN};

/// Hard total on-disk bytes read from all source files in one adaptation.
pub const MAX_RECORDING_INPUT_BYTES: usize = 16 * 1024 * 1024;
/// Hard number of complete source frames scanned across all files.
pub const MAX_RECORDING_FRAMES: usize = 100_000;
/// Maximum simultaneously opened source streams.
pub const MAX_RECORDING_STREAMS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RecordingSource {
    Sfdps,
    Adsb,
}

impl RecordingSource {
    pub const fn rank(self) -> u8 {
        // The normalized protocol requires surveillance before flight events at one receipt.
        match self { Self::Adsb => 0, Self::Sfdps => 1 }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordingInput {
    pub source: RecordingSource,
    pub path: PathBuf,
}

/// A bounded copied frame, independent of the reader's reused byte buffer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedFrame {
    pub source: RecordingSource,
    pub stream_index: u8,
    pub receipt_ns: i64,
    pub sequence: u64,
    pub jms_timestamp_ms: i64,
    pub mono_ns: Option<u64>,
    pub oaf_flags: u8,
    pub byte_offset: u64,
    pub skipped_before: u64,
    pub kind: Kind,
    pub payload: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StreamCounts {
    pub files_seen: u64,
    pub files_refused: u64,
    pub input_bytes: u64,
    pub frames_seen: u64,
    pub message_frames: u64,
    pub status_frames: u64,
    pub reject_frames: u64,
    pub invalid_frames: u64,
    pub truncated_frames: u64,
    pub skipped_bytes: u64,
    pub invalid_receipt: u64,
    pub receipt_regressions: u64,
    pub sequence_regressions: u64,
    pub sealed_receipt: u64,
    pub over_capacity_frames: u64,
    pub compressed_inputs_refused: u64,
    pub gzip_bytes_out: u64,
    pub gzip_truncated: u64,
    pub gzip_damaged: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RecordingCounts {
    pub sfdps: StreamCounts,
    pub adsb: StreamCounts,
    pub total_bytes: u64,
    pub total_decoded_bytes: u64,
    pub total_frames: u64,
    pub work_scan_positions: u64,
    pub work_crc_bytes: u64,
    pub work_copied_bytes: u64,
    pub work_compacted_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordingFrames {
    /// Sorted by receipt, source priority, source sequence, caller stream order, and byte offset.
    pub frames: Vec<RecordedFrame>,
    pub counts: RecordingCounts,
    /// Content identity of every opened source file; the digest is of exact on-disk bytes.
    pub streams: Vec<RecordingStreamProvenance>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordingStreamProvenance {
    pub source: RecordingSource,
    pub stream_index: u8,
    pub sha256: [u8; 32],
    pub compressed_bytes: u64,
    pub decoded_bytes: u64,
    pub gzip: bool,
}

#[derive(Debug)]
pub enum RecordingReadError {
    TooManyStreams,
    InputTooLarge,
    Read(io::Error),
    FrameWork(oa_feed::FrameWorkRefusal),
    FrameCountOverflow,
    InflateLimit,
    InflateAllocation,
    FrameBufferAllocation,
}

impl From<io::Error> for RecordingReadError {
    fn from(value: io::Error) -> Self { Self::Read(value) }
}

fn counts_mut(counts: &mut RecordingCounts, source: RecordingSource) -> &mut StreamCounts {
    match source { RecordingSource::Sfdps => &mut counts.sfdps, RecordingSource::Adsb => &mut counts.adsb }
}

fn note_frame_error(counts: &mut StreamCounts, error: FrameError) {
    counts.invalid_frames = counts.invalid_frames.saturating_add(1);
    if matches!(error.kind, oa_feed::ErrorKind::Truncated { .. }) {
        counts.truncated_frames = counts.truncated_frames.saturating_add(1);
    }
}

fn inflate_gzip(bytes: &[u8], max_output: usize) -> Result<(Vec<u8>, oa_zip::GzipSummary, bool), RecordingReadError> {
    let limits = oa_zip::InflateLimits { max_output: max_output as u64, max_ratio: 200, ratio_exempt_bytes: 1024 * 1024 };
    let mut gzip = oa_zip::GzipReader::new(limits);
    let mut output = Vec::new();
    output.try_reserve_exact(bytes.len().min(max_output)).map_err(|_| RecordingReadError::InflateAllocation)?;
    let mut output_limit = false;
    let mut allocation_failed = false;
    let result = gzip.push(bytes, &mut |chunk: &[u8]| -> Result<(), ()> {
        let Some(next) = output.len().checked_add(chunk.len()) else { output_limit = true; return Err(()); };
        if next > max_output { output_limit = true; return Err(()); }
        if output.try_reserve(chunk.len()).is_err() { allocation_failed = true; return Err(()); }
        output.extend_from_slice(chunk);
        Ok(())
    });
    if output_limit { return Err(RecordingReadError::InflateLimit); }
    if allocation_failed { return Err(RecordingReadError::InflateAllocation); }
    if matches!(&result, Err(oa_zip::GzipError::Data(oa_zip::InflateError::OutputLimit { .. } | oa_zip::InflateError::RatioLimit { .. }))) {
        return Err(RecordingReadError::InflateLimit);
    }
    let summary = gzip.finish();
    Ok((output, summary, result.is_err()))
}

fn capture_frame(
    frame: Frame<'_>,
    source: RecordingSource,
    stream_index: u8,
    last_receipt: &mut Option<i64>,
    last_sequence: &mut Option<u64>,
    out: &mut Vec<RecordedFrame>,
    counts: &mut StreamCounts,
) -> Result<(), RecordingReadError> {
    counts.skipped_bytes = counts.skipped_bytes.saturating_add(frame.skipped_before);
    if !validate_frame_order(frame, last_receipt, last_sequence, counts) { return Ok(()); }
    match frame.kind {
        Kind::Reject => { counts.reject_frames = counts.reject_frames.saturating_add(1); return Ok(()); }
        Kind::Status => { counts.status_frames = counts.status_frames.saturating_add(1); return Ok(()); }
        Kind::Message => counts.message_frames = counts.message_frames.saturating_add(1),
    }
    let mut payload = Vec::new();
    payload.try_reserve_exact(frame.payload.len()).map_err(|_| RecordingReadError::FrameBufferAllocation)?;
    payload.extend_from_slice(frame.payload);
    out.try_reserve(1).map_err(|_| RecordingReadError::FrameBufferAllocation)?;
    out.push(RecordedFrame {
        source,
        stream_index,
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
    Ok(())
}

fn validate_frame_order(
    frame: Frame<'_>,
    last_receipt: &mut Option<i64>,
    last_sequence: &mut Option<u64>,
    counts: &mut StreamCounts,
) -> bool {
    if frame.recv_unix_ns < 0 {
        counts.invalid_receipt = counts.invalid_receipt.saturating_add(1);
        return false;
    }
    if oa_sealed::in_sealed_window(frame.recv_unix_ns) {
        counts.sealed_receipt = counts.sealed_receipt.saturating_add(1);
        return false;
    }
    if last_receipt.is_some_and(|previous| frame.recv_unix_ns < previous) {
        counts.receipt_regressions = counts.receipt_regressions.saturating_add(1);
        return false;
    }
    if last_sequence.is_some_and(|previous| frame.seq < previous) {
        counts.sequence_regressions = counts.sequence_regressions.saturating_add(1);
        return false;
    }
    *last_receipt = Some(frame.recv_unix_ns);
    *last_sequence = Some(frame.seq);
    true
}

fn count_over_capacity_frame(
    frame: Frame<'_>,
    last_receipt: &mut Option<i64>,
    last_sequence: &mut Option<u64>,
    counts: &mut StreamCounts,
) {
    counts.frames_seen = counts.frames_seen.saturating_add(1);
    counts.over_capacity_frames = counts.over_capacity_frames.saturating_add(1);
    counts.skipped_bytes = counts.skipped_bytes.saturating_add(frame.skipped_before);
    let _order_ok = validate_frame_order(frame, last_receipt, last_sequence, counts);
    match frame.kind {
        Kind::Message => counts.message_frames = counts.message_frames.saturating_add(1),
        Kind::Status => counts.status_frames = counts.status_frames.saturating_add(1),
        Kind::Reject => counts.reject_frames = counts.reject_frames.saturating_add(1),
    }
}

/// Scans one raw OAF byte stream with `oa-feed`'s resynchronizing bounded frame decoder. The
/// byte slice must itself already have been admitted under the caller's aggregate input budget.
pub fn scan_oaf_bytes(
    source: RecordingSource,
    stream_index: u8,
    bytes: &[u8],
    remaining_frame_capacity: usize,
) -> Result<(Vec<RecordedFrame>, StreamCounts, oa_feed::FrameWorkStats), RecordingReadError> {
    let mut counts = StreamCounts { input_bytes: u64::try_from(bytes.len()).map_err(|_| RecordingReadError::InputTooLarge)?, ..StreamCounts::default() };
    let mut out = Vec::new();
    out.try_reserve_exact(remaining_frame_capacity.min(1024)).map_err(|_| RecordingReadError::FrameBufferAllocation)?;
    let mut scratch = Vec::new();
    scratch.try_reserve_exact(MAX_FRAME_LEN).map_err(|_| RecordingReadError::FrameBufferAllocation)?;
    scratch.resize(MAX_FRAME_LEN, 0);
    let mut reader = FrameReader::new(&mut scratch).map_err(|_| RecordingReadError::InputTooLarge)?;
    let mut input_pos = 0usize;
    let mut last_receipt = None;
    let mut last_sequence = None;
    while input_pos < bytes.len() {
        let budget = FrameWorkBudget::for_capacity(MAX_FRAME_LEN);
        let pushed = reader.push_with_budget(&bytes[input_pos..], budget).map_err(RecordingReadError::FrameWork)?;
        input_pos = input_pos.checked_add(pushed).ok_or(RecordingReadError::InputTooLarge)?;
        loop {
            match reader.next_with_budget(FrameWorkBudget::for_capacity(MAX_FRAME_LEN)).map_err(RecordingReadError::FrameWork)? {
                Some(Ok(frame)) => {
                    if counts.frames_seen as usize >= remaining_frame_capacity {
                        count_over_capacity_frame(frame, &mut last_receipt, &mut last_sequence, &mut counts);
                    } else {
                        counts.frames_seen = counts.frames_seen.saturating_add(1);
                        capture_frame(frame, source, stream_index, &mut last_receipt, &mut last_sequence, &mut out, &mut counts)?;
                    }
                }
                Some(Err(error)) => note_frame_error(&mut counts, error),
                None => break,
            }
        }
        if pushed == 0 && input_pos < bytes.len() {
            return Err(RecordingReadError::InputTooLarge);
        }
    }
    while let Some(next) = reader.next_with_budget(FrameWorkBudget::for_capacity(MAX_FRAME_LEN)).map_err(RecordingReadError::FrameWork)? {
        match next {
            Ok(frame) if (counts.frames_seen as usize) < remaining_frame_capacity => {
                counts.frames_seen = counts.frames_seen.saturating_add(1);
                capture_frame(frame, source, stream_index, &mut last_receipt, &mut last_sequence, &mut out, &mut counts)?;
            }
            Ok(frame) => count_over_capacity_frame(frame, &mut last_receipt, &mut last_sequence, &mut counts),
            Err(error) => note_frame_error(&mut counts, error),
        }
    }
    if let Some(error) = reader.finish() { note_frame_error(&mut counts, error); }
    counts.skipped_bytes = reader.skipped_total();
    let work = reader.work_stats();
    Ok((out, counts, work))
}

/// Opens and scans multiple source files under aggregate compressed and expanded byte/frame
/// bounds. Work is capped by 16 MiB physical input, 16 MiB expanded input, 100k complete frames,
/// each OAF payload's 4 MiB ceiling, and FIXM/XML element caps. The receipt merge sorts at most
/// 100k frames (O(n log n)); no input-dependent loop exceeds those caps.
pub fn read_oaf_inputs(inputs: &[RecordingInput]) -> Result<RecordingFrames, RecordingReadError> {
    if inputs.len() > MAX_RECORDING_STREAMS { return Err(RecordingReadError::TooManyStreams); }
    let mut result = RecordingFrames { frames: Vec::new(), counts: RecordingCounts::default(), streams: Vec::new() };
    for (i, input) in inputs.iter().enumerate() {
        let counts = counts_mut(&mut result.counts, input.source);
        counts.files_seen = counts.files_seen.saturating_add(1);
        let path = &input.path;
        let (file, size) = oa_sealed::open_regular(path.as_path()).map_err(RecordingReadError::Read)?;
        let already_read = usize::try_from(result.counts.total_bytes).map_err(|_| RecordingReadError::InputTooLarge)?;
        let remaining_bytes = MAX_RECORDING_INPUT_BYTES.checked_sub(already_read).ok_or(RecordingReadError::InputTooLarge)?;
        let file_bytes = usize::try_from(size).map_err(|_| RecordingReadError::InputTooLarge)?;
        if file_bytes > remaining_bytes {
            counts.files_refused = counts.files_refused.saturating_add(1);
            return Err(RecordingReadError::InputTooLarge);
        }
        let mut physical = Vec::new();
        physical.try_reserve_exact(file_bytes).map_err(|_| RecordingReadError::FrameBufferAllocation)?;
        file.take(u64::try_from(remaining_bytes).map_err(|_| RecordingReadError::InputTooLarge)?.saturating_add(1)).read_to_end(&mut physical)?;
        if physical.len() > remaining_bytes { return Err(RecordingReadError::InputTooLarge); }
        if physical.len() != file_bytes { return Err(RecordingReadError::Read(io::Error::new(io::ErrorKind::UnexpectedEof, "recording changed while reading"))); }
        let compressed = physical.starts_with(&[0x1f, 0x8b]) || path.extension().is_some_and(|ext| ext == "gz");
        let remaining_decoded = MAX_RECORDING_INPUT_BYTES.checked_sub(result.counts.total_decoded_bytes).ok_or(RecordingReadError::InputTooLarge)?;
        let expanded;
        let bytes = if compressed {
            let (decoded, gzip_summary, damaged) = inflate_gzip(&physical, remaining_decoded)?;
            if damaged {
                counts_mut(&mut result.counts, input.source).gzip_damaged += 1;
            }
            let stream_counts = counts_mut(&mut result.counts, input.source);
            stream_counts.gzip_bytes_out = stream_counts.gzip_bytes_out.saturating_add(gzip_summary.bytes_out);
            if gzip_summary.truncated.is_some() { stream_counts.gzip_truncated = stream_counts.gzip_truncated.saturating_add(1); }
            if damaged || gzip_summary.truncated.is_some() {
                // Gzip delivers bytes before it verifies the trailer. Do not admit even a
                // seemingly complete OAF prefix from an unverified member as source evidence.
                stream_counts.compressed_inputs_refused = stream_counts.compressed_inputs_refused.saturating_add(1);
                expanded = Vec::new();
            } else {
                expanded = decoded;
            }
            if expanded.len() > remaining_decoded { return Err(RecordingReadError::InflateLimit); }
            expanded.as_slice()
        } else { physical.as_slice() };
        if bytes.len() > remaining_decoded { return Err(RecordingReadError::InputTooLarge); }
        let seen_frames = usize::try_from(result.counts.total_frames).map_err(|_| RecordingReadError::FrameCountOverflow)?;
        let remaining_frames = MAX_RECORDING_FRAMES.saturating_sub(seen_frames);
        let (mut frames, mut stream_counts, work) = scan_oaf_bytes(input.source, u8::try_from(i).map_err(|_| RecordingReadError::TooManyStreams)?, bytes, remaining_frames)?;
        stream_counts.input_bytes = size;
        result.streams.try_reserve(1).map_err(|_| RecordingReadError::FrameBufferAllocation)?;
        result.streams.push(RecordingStreamProvenance {
            source: input.source,
            stream_index: u8::try_from(i).map_err(|_| RecordingReadError::TooManyStreams)?,
            sha256: oa_sha256::sha256(&physical),
            compressed_bytes: size,
            decoded_bytes: u64::try_from(bytes.len()).map_err(|_| RecordingReadError::InputTooLarge)?,
            gzip: compressed,
        });
        result.counts.total_bytes = result.counts.total_bytes.checked_add(size).ok_or(RecordingReadError::InputTooLarge)?;
        result.counts.total_decoded_bytes = result.counts.total_decoded_bytes.checked_add(bytes.len()).ok_or(RecordingReadError::InputTooLarge)?;
        result.counts.total_frames = result.counts.total_frames.checked_add(stream_counts.frames_seen).ok_or(RecordingReadError::FrameCountOverflow)?;
        let target = counts_mut(&mut result.counts, input.source);
        *target = target.saturating_add(stream_counts);
        result.counts.work_scan_positions = result.counts.work_scan_positions.saturating_add(work.scan_positions);
        result.counts.work_crc_bytes = result.counts.work_crc_bytes.saturating_add(work.crc_bytes);
        result.counts.work_copied_bytes = result.counts.work_copied_bytes.saturating_add(work.copied_bytes);
        result.counts.work_compacted_bytes = result.counts.work_compacted_bytes.saturating_add(work.compacted_bytes);
        result.frames.try_reserve(frames.len()).map_err(|_| RecordingReadError::FrameBufferAllocation)?;
        result.frames.append(&mut frames);
    }
    sort_recording_frames(&mut result.frames);
    Ok(result)
}

/// Canonical receipt merge. Source rank matches the normalized same-receipt category order.
pub fn sort_recording_frames(frames: &mut [RecordedFrame]) {
    frames.sort_unstable_by_key(|f| (f.receipt_ns, f.source.rank(), f.sequence, f.stream_index, f.byte_offset));
}

impl StreamCounts {
    fn saturating_add(self, rhs: Self) -> Self {
        Self {
            files_seen: self.files_seen.saturating_add(rhs.files_seen), files_refused: self.files_refused.saturating_add(rhs.files_refused),
            input_bytes: self.input_bytes.saturating_add(rhs.input_bytes), frames_seen: self.frames_seen.saturating_add(rhs.frames_seen),
            message_frames: self.message_frames.saturating_add(rhs.message_frames), status_frames: self.status_frames.saturating_add(rhs.status_frames),
            reject_frames: self.reject_frames.saturating_add(rhs.reject_frames), invalid_frames: self.invalid_frames.saturating_add(rhs.invalid_frames),
            truncated_frames: self.truncated_frames.saturating_add(rhs.truncated_frames), skipped_bytes: self.skipped_bytes.saturating_add(rhs.skipped_bytes),
            invalid_receipt: self.invalid_receipt.saturating_add(rhs.invalid_receipt), receipt_regressions: self.receipt_regressions.saturating_add(rhs.receipt_regressions),
            sequence_regressions: self.sequence_regressions.saturating_add(rhs.sequence_regressions),
            sealed_receipt: self.sealed_receipt.saturating_add(rhs.sealed_receipt), over_capacity_frames: self.over_capacity_frames.saturating_add(rhs.over_capacity_frames),
            compressed_inputs_refused: self.compressed_inputs_refused.saturating_add(rhs.compressed_inputs_refused),
            gzip_bytes_out: self.gzip_bytes_out.saturating_add(rhs.gzip_bytes_out),
            gzip_truncated: self.gzip_truncated.saturating_add(rhs.gzip_truncated),
            gzip_damaged: self.gzip_damaged.saturating_add(rhs.gzip_damaged),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(seq: u64, receipt: i64, kind: Kind) -> Vec<u8> {
        let mut out = vec![0u8; oa_feed::HEADER_LEN + 6 + oa_feed::TRAILER_LEN];
        let len = oa_feed::encode_frame(&mut out, kind as u8, 0, seq, receipt, 0, b"sample").unwrap();
        out.truncate(len);
        out
    }

    #[test]
    fn scan_counts_headers_and_keeps_only_valid_messages_in_source_order() {
        let mut bytes = frame(1, 100, Kind::Message);
        bytes.extend(frame(2, 101, Kind::Status));
        bytes.extend(frame(3, 99, Kind::Message));
        let (frames, counts, _) = scan_oaf_bytes(RecordingSource::Adsb, 0, &bytes, 10).unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!((frames[0].sequence, frames[0].receipt_ns, frames[0].payload.as_slice()), (1, 100, b"sample".as_slice()));
        assert_eq!(counts.frames_seen, 3);
        assert_eq!(counts.status_frames, 1);
        assert_eq!(counts.receipt_regressions, 1);
    }

    #[test]
    fn scan_counts_bad_frame_and_refuses_excess_complete_frames_whole() {
        let mut bytes = frame(1, 100, Kind::Message);
        bytes.extend(frame(2, 101, Kind::Message));
        bytes.push(b'X');
        let (frames, counts, _) = scan_oaf_bytes(RecordingSource::Sfdps, 0, &bytes, 1).unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(counts.over_capacity_frames, 1);
        assert!(counts.invalid_frames >= 1);
    }

    #[test]
    fn merge_order_is_receipt_then_protocol_source_then_complete_source_tie_break() {
        let make = |source, receipt_ns, sequence, stream_index, byte_offset| RecordedFrame {
            source, stream_index, receipt_ns, sequence, jms_timestamp_ms: 0, mono_ns: None,
            oaf_flags: 0, byte_offset, skipped_before: 0, kind: Kind::Message, payload: vec![],
        };
        let mut frames = vec![
            make(RecordingSource::Sfdps, 20, 1, 0, 36),
            make(RecordingSource::Adsb, 20, 2, 1, 70),
            make(RecordingSource::Adsb, 20, 1, 0, 36),
            make(RecordingSource::Sfdps, 10, 99, 0, 100),
        ];
        sort_recording_frames(&mut frames);
        assert_eq!(frames.iter().map(|f| (f.receipt_ns, f.source, f.sequence, f.stream_index, f.byte_offset)).collect::<Vec<_>>(), vec![
            (10, RecordingSource::Sfdps, 99, 0, 100),
            (20, RecordingSource::Adsb, 1, 0, 36),
            (20, RecordingSource::Adsb, 2, 1, 70),
            (20, RecordingSource::Sfdps, 1, 0, 36),
        ]);
    }

    #[test]
    fn bounded_gzip_expands_complete_oaf_bytes_and_reports_tail() {
        let payload = frame(1, 100, Kind::Message);
        let mut gzip = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 3, 1];
        let len = u16::try_from(payload.len()).unwrap();
        gzip.extend(len.to_le_bytes());
        gzip.extend((!len).to_le_bytes());
        gzip.extend(&payload);
        gzip.extend(oa_zip::crc32(&payload).to_le_bytes());
        gzip.extend(u32::try_from(payload.len()).unwrap().to_le_bytes());
        let (decoded, summary, damaged) = inflate_gzip(&gzip, 4096).unwrap();
        assert_eq!(decoded, payload);
        assert_eq!(summary.members, 1);
        assert_eq!(summary.truncated, None);
        assert!(!damaged);
        assert!(matches!(inflate_gzip(&gzip, payload.len() - 1), Err(RecordingReadError::InflateLimit)));
    }
}
