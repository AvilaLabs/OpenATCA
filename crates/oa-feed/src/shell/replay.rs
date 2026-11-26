//! Replay: recorded `.oaf` and `.oaf.gz` files read back through the same [`FrameReader`] and
//! [`FeedHealth`] as a live feed. Shell (reads files); the figures depend only on frame fields,
//! never on the clock (B6).
//!
//! A file is gzip if it starts with the gzip magic (1f 8b) and raw otherwise, whatever its name:
//! a frame file starts with `OAF1`, so the two cannot be confused. A compressed file is inflated
//! as it is read, in fixed memory, and its decoded bytes go through the frame reader exactly as a
//! raw file's bytes do, so the health figures of a recording and of its compressed copy are
//! identical, and so is the `bytes=` count (bytes of frames, not of the file on disk). A
//! compressed file cut short by a crash gives every frame up to the cut and a [`GzNote`], so it
//! is never mistaken for a complete one.

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::io::{self, Read};
use std::path::Path;

use oa_zip::{GzipError, GzipReader, InflateLimits, Truncated, MAX_RATIO, RATIO_EXEMPT_BYTES};

use crate::frame::{FrameError, MAX_FRAME_LEN};
use crate::health::{observe_error, observe_frame, observe_skipped, FeedHealth};
use crate::reader::{FrameReader, FrameWorkBudget, FrameWorkRefusal};

/// Largest decoded size of one compressed file, 64 GiB: far above a rotated recording (256 MiB
/// of compressed data is a few GiB of frames) and a stop for a runaway stream.
pub const MAX_GZ_OUTPUT: u64 = 64 * 1024 * 1024 * 1024;

/// The inflate limits for a compressed recording: the output cap above and the ratio limit of
/// oa-zip (feed data compresses 10 to 18 times; a bomb is stopped at 200).
pub const REPLAY_LIMITS: InflateLimits = InflateLimits {
    max_output: MAX_GZ_OUTPUT,
    max_ratio: MAX_RATIO,
    ratio_exempt_bytes: RATIO_EXEMPT_BYTES,
};

/// Maximum distinct UTC hour buckets in a rate measurement. This bounds memory even when a
/// damaged or synthetic input carries widely separated timestamps.
pub const MAX_RATE_HOUR_BUCKETS: usize = 100_000;
/// Aggregate regular-file input bound for one `measure-rate` invocation.
pub const MAX_RATE_INPUT_BYTES: u64 = 16 * 1024 * 1024 * 1024;
/// Per-file regular-file input bound for one `measure-rate` invocation.
pub const MAX_RATE_INPUT_FILE_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const NS_PER_HOUR: i64 = 3_600_000_000_000;

/// Aggregate, payload-free measurement of decoded OAF frame bytes grouped by receive-time UTC
/// hour. This is an observed-frame metric; it does not prove continuous capture between frames.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HourlyRateReport {
    pub frames: u64,
    pub input_file_bytes: u64,
    pub observed_hours: usize,
    pub first_recv_unix_ns: Option<i64>,
    pub last_recv_unix_ns: Option<i64>,
    pub max_observed_hour_frame_bytes: u64,
    pub edge_hours_may_be_partial: usize,
}

/// Bounded accumulator used by the replay parser. It retains only per-hour byte totals.
#[derive(Default)]
pub struct HourlyRateMeter {
    buckets: BTreeMap<i64, u64>,
    frames: u64,
    first: Option<i64>,
    last: Option<i64>,
    input_file_bytes: u64,
    total_frame_bytes: u64,
}

impl HourlyRateMeter {
    fn add_input_file_bytes(&mut self, bytes: u64) -> io::Result<()> {
        self.input_file_bytes = self
            .input_file_bytes
            .checked_add(bytes)
            .ok_or_else(|| io::Error::other("input file byte count overflow"))?;
        if self.input_file_bytes > MAX_RATE_INPUT_BYTES {
            return Err(io::Error::other(
                "rate measurement input byte limit exceeded",
            ));
        }
        Ok(())
    }

    /// Add one validated frame. Timestamp assignment uses Euclidean division, including pre-epoch
    /// values. A new hour is refused before growing beyond the fixed bucket cap.
    pub fn observe(&mut self, recv_unix_ns: i64, frame_bytes: usize) -> io::Result<()> {
        let hour = recv_unix_ns.div_euclid(NS_PER_HOUR);
        if !self.buckets.contains_key(&hour) && self.buckets.len() >= MAX_RATE_HOUR_BUCKETS {
            return Err(io::Error::other("hourly rate bucket limit exceeded"));
        }
        let amount = u64::try_from(frame_bytes)
            .map_err(|_| io::Error::other("frame byte count overflow"))?;
        let bucket = self.buckets.entry(hour).or_default();
        *bucket = bucket
            .checked_add(amount)
            .ok_or_else(|| io::Error::other("hourly frame byte count overflow"))?;
        self.frames = self
            .frames
            .checked_add(1)
            .ok_or_else(|| io::Error::other("frame count overflow"))?;
        self.total_frame_bytes = self
            .total_frame_bytes
            .checked_add(amount)
            .ok_or_else(|| io::Error::other("total frame byte count overflow"))?;
        self.first = Some(self.first.map_or(recv_unix_ns, |v| v.min(recv_unix_ns)));
        self.last = Some(self.last.map_or(recv_unix_ns, |v| v.max(recv_unix_ns)));
        Ok(())
    }

    /// Finish with an aggregate only; partial edge buckets are disclosed and retained in the
    /// maximum because the input window does not imply complete-hour capture.
    pub fn finish(self) -> HourlyRateReport {
        let edge_hours_may_be_partial = match (self.first, self.last) {
            (Some(first), Some(last))
                if first.div_euclid(NS_PER_HOUR) == last.div_euclid(NS_PER_HOUR) =>
            {
                1
            }
            (Some(_), Some(_)) => 2,
            _ => 0,
        };
        HourlyRateReport {
            frames: self.frames,
            input_file_bytes: self.input_file_bytes,
            observed_hours: self.buckets.len(),
            first_recv_unix_ns: self.first,
            last_recv_unix_ns: self.last,
            max_observed_hour_frame_bytes: self.buckets.values().copied().max().unwrap_or(0),
            edge_hours_may_be_partial,
        }
    }
}

/// A refused frame found while replaying, with the file it was in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileError {
    /// Index into the list of files given.
    pub file: usize,
    /// The frame error.
    pub error: FrameError,
}

/// What went wrong with the gzip layer of one file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GzNoteKind {
    /// The file ends inside a member; the frames before the cut were replayed.
    Truncated(Truncated),
    /// The gzip data is damaged or over a limit; the frames decoded before it were replayed.
    Damaged(GzipError<Infallible>),
}

/// A gzip-layer problem and the file it was in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GzNote {
    /// Index into the list of files given.
    pub file: usize,
    /// The problem.
    pub kind: GzNoteKind,
}

/// Result of replaying one or more files.
#[derive(Clone, Debug)]
pub struct Report {
    /// Health after every frame.
    pub health: FeedHealth,
    /// Files read.
    pub files: usize,
    /// Bytes of frame data read (decoded bytes, for a compressed file).
    pub bytes: u64,
    /// Every refused frame.
    pub errors: Vec<FileError>,
    /// Every truncated or damaged compressed file.
    pub gzip: Vec<GzNote>,
    /// Frames left out of `health` because their receive time is in a sealed window
    /// (OA-SEAL-014); counted, not hidden.
    pub sealed_window_dropped: u64,
}

/// How one source is being read.
enum Mode {
    Raw,
    Gzip(Box<GzipReader>),
    /// The gzip layer failed; the rest of this file is ignored.
    Dead,
}

/// A size-stable reader for one measurement input. Reads are bounded to the opened regular
/// file's observed size and EOF is returned only if the descriptor still has that size.
struct RateInput {
    file: std::fs::File,
    expected_size: u64,
    read_bytes: u64,
}

impl Read for RateInput {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        if self.read_bytes == self.expected_size {
            let metadata = self.file.metadata()?;
            if metadata.len() != self.expected_size || !metadata.is_file() {
                return Err(io::Error::other("rate input changed while being read"));
            }
            return Ok(0);
        }
        let remaining = self.expected_size - self.read_bytes;
        let allowed = usize::try_from(remaining)
            .unwrap_or(usize::MAX)
            .min(buffer.len());
        let count = self
            .file
            .read(buffer.get_mut(..allowed).unwrap_or(&mut []))?;
        if count == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "rate input shortened while being read",
            ));
        }
        self.read_bytes = self
            .read_bytes
            .checked_add(
                u64::try_from(count)
                    .map_err(|_| io::Error::other("rate input byte count conversion failed"))?,
            )
            .ok_or_else(|| io::Error::other("rate input byte count overflow"))?;
        Ok(count)
    }
}

/// Replays `readers` in order through one health record. Each source gets a fresh
/// [`FrameReader`] (a file never continues another's partial frame); the sequence tracking
/// carries across, since recording rotates on frame boundaries.
pub fn replay_sources<R: Read>(sources: Vec<R>) -> io::Result<Report> {
    replay_sources_counted(sources, &mut 0)
}

/// [`replay_sources`], with the frames dropped in a sealed window (by receive time) so far put in
/// `sealed` on every exit, an error included.
pub fn replay_sources_counted<R: Read>(sources: Vec<R>, sealed: &mut u64) -> io::Result<Report> {
    replay_sources_counted_inner(sources, sealed, None)
}

/// Replay through the same sealed-aware decoder while also aggregating validated frame sizes by
/// receive-time UTC hour. The accumulator is the only retained measurement state.
pub fn replay_sources_counted_with_rate<R: Read>(
    sources: Vec<R>,
    sealed: &mut u64,
    rate: &mut HourlyRateMeter,
) -> io::Result<Report> {
    replay_sources_counted_inner(sources, sealed, Some(rate))
}

fn replay_sources_counted_inner<R: Read>(
    sources: Vec<R>,
    sealed: &mut u64,
    mut rate: Option<&mut HourlyRateMeter>,
) -> io::Result<Report> {
    let mut health = FeedHealth::new();
    let mut errors = Vec::new();
    let mut gzip = Vec::new();
    let mut bytes = 0u64;
    // Bytes of the frames dropped for their time in a sealed window: not counted in `bytes`,
    // which is the bytes after the window gate (OA-SEAL-014).
    let mut gated = 0u64;
    let mut files = 0usize;
    let mut storage = vec![0u8; 2 * MAX_FRAME_LEN];
    let mut chunk = vec![0u8; 64 * 1024];
    for (i, mut src) in sources.into_iter().enumerate() {
        files += 1;
        let Ok(mut reader) = FrameReader::new(&mut storage) else {
            return Err(io::Error::other("reader buffer too small"));
        };
        // The first two bytes decide the mode; a short read must not decide it early.
        let mut have = 0usize;
        while have < 2 {
            let n = src.read(chunk.get_mut(have..).unwrap_or(&mut []))?;
            if n == 0 {
                break;
            }
            have += n;
        }
        let mut mode = if chunk.get(..2) == Some(&[0x1f, 0x8b]) && have >= 2 {
            Mode::Gzip(Box::new(GzipReader::new(REPLAY_LIMITS)))
        } else {
            Mode::Raw
        };
        if rate.is_some() && matches!(&mode, Mode::Gzip(_)) {
            return Err(io::Error::other(
                "rate measurement requires raw OAF files to preserve exact byte/hour accounting",
            ));
        }
        let mut n = have;
        while n > 0 {
            let data = chunk.get(..n).unwrap_or(&[]);
            match &mut mode {
                Mode::Raw => {
                    bytes += n as u64;
                    feed_frames(
                        &mut reader,
                        &mut health,
                        &mut errors,
                        (sealed, &mut gated),
                        i,
                        data,
                        &mut rate,
                    )
                    .map_err(work_refusal_error)?;
                }
                Mode::Gzip(g) => {
                    let r = g.push(data, &mut |b: &[u8]| -> Result<(), FrameWorkRefusal> {
                        bytes += b.len() as u64;
                        feed_frames(
                            &mut reader,
                            &mut health,
                            &mut errors,
                            (sealed, &mut gated),
                            i,
                            b,
                            &mut rate,
                        )?;
                        Ok(())
                    });
                    match r {
                        Ok(()) => {}
                        Err(e) => match gzip_note_error(e) {
                            Ok(damaged) => {
                                gzip.push(GzNote {
                                    file: i,
                                    kind: GzNoteKind::Damaged(damaged),
                                });
                                mode = Mode::Dead;
                            }
                            Err(refusal) => return Err(work_refusal_error(refusal)),
                        },
                    }
                }
                Mode::Dead => {}
            }
            n = src.read(&mut chunk)?;
        }
        if let Mode::Gzip(g) = &mode {
            if let Some(t) = g.finish().truncated {
                gzip.push(GzNote {
                    file: i,
                    kind: GzNoteKind::Truncated(t),
                });
            }
        }
        drain(
            &mut reader,
            &mut health,
            &mut errors,
            (sealed, &mut gated),
            i,
            &mut rate,
        )
        .map_err(work_refusal_error)?;
        if let Some(e) = reader.finish() {
            if rate.is_some() {
                return Err(io::Error::other(
                    "rate measurement found an incomplete OAF frame",
                ));
            }
            observe_error(&mut health, &e);
            errors.push(FileError { file: i, error: e });
        }
        if rate.is_some() && reader.skipped_total() != 0 {
            return Err(io::Error::other(
                "rate measurement found unframed input bytes",
            ));
        }
        let total = health.skipped_bytes.saturating_add(reader.skipped_total());
        observe_skipped(&mut health, total);
    }
    if let Some(meter) = rate.as_deref() {
        if meter.input_file_bytes != 0 && meter.input_file_bytes != meter.total_frame_bytes {
            return Err(io::Error::other(
                "rate measurement byte accounting mismatch",
            ));
        }
    }
    Ok(Report {
        health,
        files,
        bytes: bytes.saturating_sub(gated),
        errors,
        gzip,
        sealed_window_dropped: *sealed,
    })
}

/// Pushes `data` through the frame reader, taking every frame and error out as it comes. `sealed`
/// is the count and the bytes of the frames dropped in a sealed window.
fn feed_frames(
    reader: &mut FrameReader<'_>,
    health: &mut FeedHealth,
    errors: &mut Vec<FileError>,
    sealed: (&mut u64, &mut u64),
    file: usize,
    data: &[u8],
    rate: &mut Option<&mut HourlyRateMeter>,
) -> Result<(), FrameWorkRefusal> {
    let mut off = 0usize;
    while off < data.len() {
        // The allowance follows the reader's real buffer, not a frame's worth of it.
        let budget = FrameWorkBudget::for_capacity(reader.capacity());
        let took = reader.push_with_budget(data.get(off..).unwrap_or(&[]), budget)?;
        off += took;
        drain(
            reader,
            health,
            errors,
            (&mut *sealed.0, &mut *sealed.1),
            file,
            rate,
        )?;
        if took == 0 {
            break;
        }
    }
    Ok(())
}

fn drain(
    r: &mut FrameReader<'_>,
    h: &mut FeedHealth,
    errors: &mut Vec<FileError>,
    sealed: (&mut u64, &mut u64),
    file: usize,
    rate: &mut Option<&mut HourlyRateMeter>,
) -> Result<(), FrameWorkRefusal> {
    loop {
        let budget = FrameWorkBudget::for_capacity(r.capacity());
        let Some(ev) = r.next_with_budget(budget)? else {
            return Ok(());
        };
        match ev {
            // A frame received inside a sealed window is dropped and counted (OA-SEAL-014).
            Ok(f) if oa_sealed::in_sealed_window(f.recv_unix_ns) => {
                *sealed.0 += 1;
                *sealed.1 += f.raw.len() as u64;
                oa_sealed::note_window_drops(1);
            }
            Ok(f) => {
                observe_frame(h, &f, f.recv_unix_ns);
                if let Some(meter) = rate.as_deref_mut() {
                    // `FrameReader` produced this frame; only its encoded size and time enter
                    // the aggregate. Payload, source ID, and content are not retained.
                    meter
                        .observe(f.recv_unix_ns, f.raw.len())
                        .map_err(|_| FrameWorkRefusal {
                            kind: crate::reader::FrameWorkKind::CounterOverflow,
                            required: u64::MAX,
                            available: 0,
                        })?;
                }
            }
            Err(e) => {
                // The measurement command is strict and aggregate-only. Stop on the first bad
                // frame rather than retaining an attacker-controlled number of error records.
                if rate.is_some() {
                    return Err(FrameWorkRefusal {
                        kind: crate::reader::FrameWorkKind::CounterOverflow,
                        required: u64::MAX,
                        available: 0,
                    });
                }
                observe_error(h, &e);
                errors.push(FileError { file, error: e });
            }
        }
    }
}

fn work_refusal_error(refusal: FrameWorkRefusal) -> io::Error {
    io::Error::other(format!("frame reader work refused: {refusal:?}"))
}

fn gzip_note_error(
    error: GzipError<FrameWorkRefusal>,
) -> Result<GzipError<Infallible>, FrameWorkRefusal> {
    match error {
        GzipError::BadMagic => Ok(GzipError::BadMagic),
        GzipError::UnsupportedMethod(method) => Ok(GzipError::UnsupportedMethod(method)),
        GzipError::ReservedFlags(flags) => Ok(GzipError::ReservedFlags(flags)),
        GzipError::HeaderFieldTooLong => Ok(GzipError::HeaderFieldTooLong),
        GzipError::HeaderCrc { expected, actual } => Ok(GzipError::HeaderCrc { expected, actual }),
        GzipError::Data(error) => Ok(GzipError::Data(error)),
        GzipError::CrcMismatch { expected, actual } => {
            Ok(GzipError::CrcMismatch { expected, actual })
        }
        GzipError::SizeMismatch { expected, actual } => {
            Ok(GzipError::SizeMismatch { expected, actual })
        }
        GzipError::Failed => Ok(GzipError::Failed),
        GzipError::Sink(refusal) => Err(refusal),
    }
}

/// Replays files by path.
pub fn replay_files(paths: &[impl AsRef<Path>]) -> io::Result<Report> {
    replay_files_counted(paths, &mut 0)
}

/// [`replay_files`], with the count of frames dropped in a sealed window put in `sealed` on every
/// exit (the files before one that cannot be opened are not read, so the count is then 0).
pub fn replay_files_counted(paths: &[impl AsRef<Path>], sealed: &mut u64) -> io::Result<Report> {
    replay_files_counted_inner(paths, sealed, None)
}

/// [`replay_files_counted`] plus bounded per-hour frame-byte totals.
pub fn replay_files_counted_with_rate(
    paths: &[impl AsRef<Path>],
    sealed: &mut u64,
    rate: &mut HourlyRateMeter,
) -> io::Result<Report> {
    replay_files_counted_inner(paths, sealed, Some(rate))
}

fn replay_files_counted_inner(
    paths: &[impl AsRef<Path>],
    sealed: &mut u64,
    mut rate: Option<&mut HourlyRateMeter>,
) -> io::Result<Report> {
    if rate.is_some() && paths.len() > 256 {
        return Err(io::Error::other(
            "rate measurement file-count limit exceeded",
        ));
    }
    let mut srcs: Vec<Box<dyn Read>> = Vec::with_capacity(paths.len());
    for p in paths {
        if let Some(meter) = rate.as_deref_mut() {
            let (source, size) = oa_sealed::open_regular(p.as_ref())
                .map_err(|e| io::Error::new(e.kind(), "rate input open refused"))?;
            if size > MAX_RATE_INPUT_FILE_BYTES {
                return Err(io::Error::other(
                    "rate measurement per-file input limit exceeded",
                ));
            }
            meter.add_input_file_bytes(size)?;
            srcs.push(Box::new(RateInput {
                file: source,
                expected_size: size,
                read_bytes: 0,
            }));
        } else {
            srcs.push(Box::new(oa_sealed::open(p.as_ref()).map_err(|e| {
                io::Error::new(e.kind(), format!("{}: {e}", p.as_ref().display()))
            })?));
        }
    }
    replay_sources_counted_inner(srcs, sealed, rate)
}

#[cfg(test)]
mod rate_tests {
    #![allow(clippy::unwrap_used)]

    use super::HourlyRateMeter;
    use std::io::Cursor;

    const HOUR_NS: i64 = 3_600_000_000_000;

    // REQ: OA-FEED-196
    #[test]
    fn rate_buckets_use_utc_hour_boundaries() {
        let mut meter = HourlyRateMeter::default();
        meter.observe(HOUR_NS - 1, 10).unwrap();
        meter.observe(HOUR_NS, 20).unwrap();
        meter.observe(2 * HOUR_NS - 1, 30).unwrap();
        let report = meter.finish();
        assert_eq!(report.frames, 3);
        assert_eq!(report.observed_hours, 2);
        assert_eq!(report.max_observed_hour_frame_bytes, 50);
        assert_eq!(report.edge_hours_may_be_partial, 2);
    }

    // REQ: OA-FEED-196
    #[test]
    fn rate_discloses_partial_window_and_handles_pre_epoch_time() {
        let mut meter = HourlyRateMeter::default();
        meter.observe(-1, 7).unwrap();
        meter.observe(15 * 60 * 1_000_000_000, 11).unwrap();
        let report = meter.finish();
        assert_eq!(report.observed_hours, 2);
        assert_eq!(report.first_recv_unix_ns, Some(-1));
        assert_eq!(report.last_recv_unix_ns, Some(900_000_000_000));
        assert_eq!(report.edge_hours_may_be_partial, 2);
        assert_eq!(report.max_observed_hour_frame_bytes, 11);
    }

    // REQ: OA-FEED-196
    #[test]
    fn parser_measurement_counts_only_valid_raw_frames() {
        let mut input = Vec::new();
        for (seq, recv) in [(1, HOUR_NS - 1), (2, HOUR_NS)] {
            let mut frame = vec![0; crate::frame::HEADER_LEN + crate::frame::TRAILER_LEN];
            let len = crate::frame::encode_frame(&mut frame, 1, 0, seq, recv, 0, &[]).unwrap();
            frame.truncate(len);
            input.extend_from_slice(&frame);
        }
        let mut meter = super::HourlyRateMeter::default();
        let report =
            super::replay_sources_counted_with_rate(vec![Cursor::new(input)], &mut 0, &mut meter)
                .unwrap();
        assert!(report.errors.is_empty());
        let measured = meter.finish();
        assert_eq!(measured.frames, 2);
        assert_eq!(measured.observed_hours, 2);
        assert_eq!(measured.max_observed_hour_frame_bytes, 40);
        assert_eq!(measured.edge_hours_may_be_partial, 2);
    }
}
