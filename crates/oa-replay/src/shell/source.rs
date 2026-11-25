//! Reading recordings as streams and merging them in constant memory. Shell (B1, B7): reads files;
//! its output depends only on the bytes it reads, never on the clock or on how they were chunked.
//!
//! * A **stream** is one feed's recording: a chain of files read one after another (rotated
//!   recordings `...-0001`, `-0002`, in name order, which is creation order). Each stream has its
//!   own [`FrameReader`] over its own fixed buffer, reads its files `chunk` bytes at a time and
//!   tells `.oaf` from `.oaf.gz` by each file's first two bytes, as `oa-feed replay` does. A gzip
//!   file is inflated as it is read, so only a small piece of decoded output is ever pending.
//! * [`merge_streams`] merges the streams with a fixed-capacity min-heap holding each stream's
//!   next frame, keyed by `(recv_unix_ns, feed kind, seq, stream index, byte offset)`. The first
//!   three are the order of OA-RPL-021; the last two make it total for any input. The feed kind
//!   order is SFDPS, TBFM, TFMS, FNS, ADS-B, so at an equal receive time ADS-B comes after every
//!   SWIM feed.
//! * **Frames that go backwards in time inside one stream are not reordered.** A recording is in
//!   receive order except for rare regressions (a bridge clock step, a redelivery). Putting such
//!   a frame in its place would need unbounded lookahead: the frame could belong arbitrarily far
//!   back, so the merge would have to hold everything read so far. Instead the frame is emitted
//!   in file order, and the pipeline counts it as a time regression (OA-RPL-018) and keeps its
//!   own clock from going back. On a recording with no regression the streamed order is exactly
//!   the order of a global sort.
//! * Frames that fail validation, a damaged or truncated gzip layer and a file ending inside a
//!   frame are counted under their [`ErrKind`] and never passed on (IF-R5).
//!
//! **Memory bound** (independent of input length): per stream, one reader buffer of
//! `MAX_FRAME_LEN` (4 MiB), one head frame (payload at most 4 MiB, in a reused buffer), one read
//! buffer of `chunk` bytes (at most [`MAX_CHUNK`]) and the pending inflate output of at most
//! [`GZ_IN`] compressed bytes (the inflater's ratio limit, 200, bounds the output). So
//! about `streams x (8.1 MiB + chunk + 200 x 16 KiB worst case)`; typical gzip pending output is
//! under 300 KiB. Add the pipeline's own fixed tables (flight table about 6.5 MB at the default
//! capacity, parser and decoder tables, the event and row tables) and the checkpoint list, which
//! grows by one 80-byte entry per checkpoint interval (about 700 KB for 24 hours at 10 s).

use std::cell::Cell;
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use oa_feed::replay::REPLAY_LIMITS;
use oa_feed::{
    ErrorKind, FrameError, FrameReader, FrameWorkBudget, FrameWorkStats, Kind, MAX_FRAME_LEN,
};
use oa_zip::GzipReader;

use crate::errors::{count_error, ErrKind, Errors, ERR_COUNT};
use crate::pipeline::{process_frame_mono, FeedKind, Pipeline};

/// Frame errors kept for reporting; all are counted.
pub const MAX_KEPT_FRAME_ERRORS: usize = 1000;
/// Largest read buffer per stream.
pub const MAX_CHUNK: usize = 1 << 20;
/// Compressed bytes handed to the inflater at once, which bounds the pending decoded output.
pub const GZ_IN: usize = 16 * 1024;

/// Opens the next file of a stream, or `None` when the chain is finished.
pub type OpenFn = Box<dyn FnMut() -> Option<io::Result<Box<dyn Read>>>>;

/// One input stream: a feed and its chain of files.
// A7.2: cold struct, size guard waived
pub struct StreamSpec {
    /// How the stream's frames are decoded.
    pub feed: FeedKind,
    /// The chain of files.
    pub open: OpenFn,
}

/// A stream over `paths`, opened one after another when the previous one is exhausted.
pub fn spec_from_paths(feed: FeedKind, paths: Vec<PathBuf>) -> StreamSpec {
    let mut it = paths.into_iter();
    StreamSpec {
        feed,
        open: Box::new(move || {
            let p = it.next()?;
            Some(
                oa_sealed::open(&p)
                    .map(|f| Box::new(f) as Box<dyn Read>)
                    .map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", p.display()))),
            )
        }),
    }
}

/// Bytes actually read from one guarded descriptor. A prefix digest includes read-ahead;
/// `eof` is true only after a verified EOF, never merely because the replay window ended.
// A7.2: cold provenance record, size guard waived.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InputReceipt {
    pub opened: bool,
    pub eof: bool,
    pub bytes: u64,
    pub sha256: [u8; 32],
}

/// Ordered pathname-to-receipt mapping. Digests identify content, not authenticated origin.
// A7.2: cold provenance record, size guard waived.
#[derive(Clone)]
pub struct ObservedInput {
    pub path: PathBuf,
    /// Full length of the same guarded regular-file descriptor supplied to the merge. For gzip,
    /// this is the compressed on-disk size. It is reserved before any input bytes are read.
    pub reserved_bytes: u64,
    pub receipt: Rc<Cell<InputReceipt>>,
}

struct HashingInput {
    file: std::fs::File,
    hash: oa_sha256::Sha256,
    receipt: Rc<Cell<InputReceipt>>,
    expected_bytes: u64,
    bytes: u64,
    eof: bool,
}

impl Read for HashingInput {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let remaining = self
            .expected_bytes
            .checked_sub(self.bytes)
            .ok_or_else(|| io::Error::other("input receipt already exceeds reserved size"))?;
        let with_growth_probe = remaining
            .checked_add(1)
            .ok_or_else(|| io::Error::other("input growth-probe size overflow"))?;
        let read_len = match usize::try_from(with_growth_probe) {
            Ok(read_cap) => buf.len().min(read_cap),
            // On a narrower platform the cap is larger than any possible caller slice.
            Err(_) => buf.len(),
        };
        let bounded_buf = buf
            .get_mut(..read_len)
            .ok_or_else(|| io::Error::other("input bounded read slice invalid"))?;
        let n = self.file.read(bounded_buf)?;
        let next = self
            .bytes
            .checked_add(
                u64::try_from(n).map_err(|_| io::Error::other("input read width overflow"))?,
            )
            .ok_or_else(|| io::Error::other("input receipt byte overflow"))?;
        self.hash
            .update(
                bounded_buf
                    .get(..n)
                    .ok_or_else(|| io::Error::other("input read length invalid"))?,
            )
            .map_err(|_| io::Error::other("input digest length refused"))?;
        self.bytes = next;
        if next > self.expected_bytes {
            return Err(io::Error::other("input grew after guarded open"));
        }
        if n == 0 {
            if next != self.expected_bytes || self.file.metadata()?.len() != self.expected_bytes {
                return Err(io::Error::other("input size changed while replaying"));
            }
            self.eof = true;
        }
        Ok(n)
    }
}

impl Drop for HashingInput {
    fn drop(&mut self) {
        self.receipt.set(InputReceipt {
            opened: true,
            eof: self.eof,
            bytes: self.bytes,
            sha256: self.hash.clone().finalize(),
        });
    }
}

/// H2 provenance from the same descriptors consumed by the merge, including partial reads.
/// Every descriptor is opened and sized before the merge can read any of them; the caller can
/// reject the aggregate reservation without processing a prefix. The path inventory is capped at
/// 128 files. For gzip, the reservation is the compressed on-disk length.
pub fn spec_from_paths_observed(
    feed: FeedKind,
    paths: Vec<PathBuf>,
) -> io::Result<(StreamSpec, Vec<ObservedInput>, u64)> {
    if paths.len() > 128 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "observed input file cap",
        ));
    }
    let mut observed: Vec<_> = paths
        .into_iter()
        .map(|path| ObservedInput {
            path,
            reserved_bytes: 0,
            receipt: Rc::new(Cell::new(InputReceipt::default())),
        })
        .collect();
    let mut readers: Vec<io::Result<Box<dyn Read>>> =
        Vec::with_capacity(observed.len().saturating_add(1));
    let mut total_reserved = 0u64;
    let mut open_error = None;
    for input in &mut observed {
        let (file, expected_bytes) = match oa_sealed::open_regular(&input.path) {
            Ok(opened) => opened,
            Err(error) => {
                open_error = Some(error);
                break;
            }
        };
        total_reserved = total_reserved.saturating_add(expected_bytes);
        input.reserved_bytes = expected_bytes;
        input.receipt.set(InputReceipt {
            opened: true,
            sha256: oa_sha256::sha256(&[]),
            ..InputReceipt::default()
        });
        readers.push(Ok(Box::new(HashingInput {
            file,
            hash: oa_sha256::Sha256::new(),
            receipt: input.receipt.clone(),
            expected_bytes,
            bytes: 0,
            eof: false,
        }) as Box<dyn Read>));
    }
    // An open failure is surfaced before any successfully opened descriptor is read. Their
    // receipts and reservations remain available for the caller's INCOMPLETE footer.
    if let Some(error) = open_error {
        readers.insert(0, Err(error));
    }
    let mut readers = readers.into_iter();
    let spec = StreamSpec {
        feed,
        open: Box::new(move || readers.next()),
    };
    Ok((spec, observed, total_reserved))
}

/// Stable ASCII provenance record; pathname bytes are hex, so controls cannot forge lines.
pub fn observed_input_line(index: usize, input: &ObservedInput) -> io::Result<String> {
    let path = input.path.as_os_str().as_encoded_bytes();
    if path.len() > 4096 {
        return Err(io::Error::other(
            "observed input pathname exceeds 4096 bytes",
        ));
    }
    use std::fmt::Write as _;
    let mut hex = String::with_capacity(path.len().saturating_mul(2));
    for byte in path {
        let _ = write!(hex, "{byte:02x}");
    }
    let receipt = input.receipt.get();
    let digest = if receipt.opened {
        oa_sha256::to_hex(&receipt.sha256)
    } else {
        "none".into()
    };
    Ok(format!("INPUT\tindex={index}\tpath_hex={hex}\topened={}\teof={}\tbytes={}\treserved_bytes={}\tsha256={digest}\n",u8::from(receipt.opened),u8::from(receipt.eof),receipt.bytes,input.reserved_bytes))
}

/// A stream over in-memory files (tests, emulators).
pub fn spec_from_bytes(feed: FeedKind, files: Vec<Vec<u8>>) -> StreamSpec {
    let mut it = files.into_iter();
    StreamSpec {
        feed,
        open: Box::new(move || {
            let b = it.next()?;
            Some(Ok(Box::new(io::Cursor::new(b)) as Box<dyn Read>))
        }),
    }
}

/// The recordings in a directory (`*.oaf`, `*.oaf.gz`), in name order, which for rotated
/// recordings is creation order.
pub fn list_recordings(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    // The sealed hold-out guard sees the directory, and every entry before it is stat-ed.
    oa_sealed::refuse_io(dir)
        .map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", dir.display())))?;
    // seal-allow: the directory is refused by the guard above and each entry is checked before it is opened
    for e in std::fs::read_dir(dir)
        .map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", dir.display())))?
    {
        let p = e?.path();
        oa_sealed::refuse_io(&p)?;
        let ok = p.is_file()
            && p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(".oaf") || n.ends_with(".oaf.gz"));
        if ok {
            out.push(p);
        }
    }
    out.sort();
    if out.is_empty() {
        return Err(io::Error::other(format!(
            "{}: no .oaf or .oaf.gz files",
            dir.display()
        )));
    }
    Ok(out)
}

enum Mode {
    Raw,
    Gzip(Box<GzipReader>),
    Dead,
}

struct Head {
    recv_ns: i64,
    mono_ns: Option<u64>,
    seq: u64,
    offset: u64,
    /// The frame's bytes, header and trailer included.
    frame_len: u64,
    kind: Kind,
    flags: u8,
    payload: Vec<u8>,
}

/// Offline whole-merge work allowance. A step renews only the existing finite per-step
/// frame-reader allowance; decoded bytes include invalid and sealed input before gates.
// A7.2: cold shell budget, size guard waived.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MergeBudget {
    pub steps_remaining: u64,
    pub decoded_bytes_remaining: u64,
    pub refused: bool,
}

type SharedBudget = Rc<Cell<MergeBudget>>;

fn charge_decoded(budget: &Option<SharedBudget>, bytes: usize) -> io::Result<()> {
    if let Some(budget) = budget {
        let mut value = budget.get();
        let bytes =
            u64::try_from(bytes).map_err(|_| io::Error::other("decoded byte width overflow"))?;
        let Some(left) = value.decoded_bytes_remaining.checked_sub(bytes) else {
            value.refused = true;
            budget.set(value);
            return Err(io::Error::other(
                "bounded merge decoded-byte budget exhausted",
            ));
        };
        value.decoded_bytes_remaining = left;
        budget.set(value);
    }
    Ok(())
}

struct Stream<'b> {
    feed: FeedKind,
    budget: Option<SharedBudget>,
    reader: FrameReader<'b>,
    open: OpenFn,
    cur: Option<Box<dyn Read>>,
    mode: Mode,
    undetermined: bool,
    eof: bool,
    inbuf: Vec<u8>,
    pending: Vec<u8>,
    pend_at: usize,
    head: Head,
    bytes: u64,
    files: usize,
    errors: Errors,
    frame_errors: Vec<FrameError>,
    /// Frames dropped because their time is in a sealed window (OA-SEAL-014).
    sealed_window_dropped: u64,
}

enum AdvanceStep {
    Continue,
    Frame,
    End,
}

fn note_error(errors: &mut Errors, kept: &mut Vec<FrameError>, e: FrameError) {
    let k = match e.kind {
        ErrorKind::BadMagic { .. } => ErrKind::FrameBadMagic,
        ErrorKind::UnknownKind { .. } => ErrKind::FrameUnknownKind,
        ErrorKind::Oversize { .. } => ErrKind::FrameOversize,
        ErrorKind::Crc { .. } => ErrKind::FrameCrc,
        ErrorKind::Reserved { .. } => ErrKind::FrameReserved,
        ErrorKind::Truncated { .. } => ErrKind::FrameTruncated,
    };
    count_error(errors, k, 1);
    if kept.len() < MAX_KEPT_FRAME_ERRORS {
        kept.push(e);
    }
}

impl<'b> Stream<'b> {
    fn new(spec: StreamSpec, reader: FrameReader<'b>, chunk: usize) -> Self {
        Stream {
            feed: spec.feed,
            budget: None,
            reader,
            open: spec.open,
            cur: None,
            mode: Mode::Raw,
            undetermined: false,
            eof: false,
            inbuf: vec![0u8; chunk.clamp(2, MAX_CHUNK)],
            pending: Vec::new(),
            pend_at: 0,
            head: Head {
                recv_ns: 0,
                mono_ns: None,
                seq: 0,
                offset: 0,
                frame_len: 0,
                kind: Kind::Message,
                flags: 0,
                payload: Vec::new(),
            },
            bytes: 0,
            files: 0,
            errors: [0; ERR_COUNT],
            frame_errors: Vec::new(),
            sealed_window_dropped: 0,
        }
    }
}

impl Stream<'_> {
    /// Moves to the stream's next frame, which lands in `head`. `Ok(false)` at the end.
    ///
    /// This is an offline file iterator: it renews the finite per-step reader allowance until it
    /// finds a frame or reaches EOF. A file may be arbitrarily long, so this search has no
    /// whole-file deadline bound.
    fn advance(&mut self) -> io::Result<bool> {
        self.advance_observed(&mut |_| {})
    }

    /// [`advance`](Self::advance), reporting the reader work each bounded step measured.
    fn advance_observed(&mut self, on_step: &mut dyn FnMut(FrameWorkStats)) -> io::Result<bool> {
        loop {
            if let Some(budget) = &self.budget {
                let mut remaining = budget.get();
                let Some(left) = remaining.steps_remaining.checked_sub(1) else {
                    remaining.refused = true;
                    budget.set(remaining);
                    return Err(io::Error::other(
                        "bounded merge reader-step budget exhausted",
                    ));
                };
                remaining.steps_remaining = left;
                budget.set(remaining);
            }
            let mut work = FrameWorkBudget::for_capacity(MAX_FRAME_LEN);
            let before = self.reader.work_stats();
            let step = self.advance_step(&mut work)?;
            let after = self.reader.work_stats();
            on_step(FrameWorkStats {
                scan_positions: after.scan_positions - before.scan_positions,
                crc_bytes: after.crc_bytes - before.crc_bytes,
                copied_bytes: after.copied_bytes - before.copied_bytes,
                compacted_bytes: after.compacted_bytes - before.compacted_bytes,
            });
            match step {
                AdvanceStep::Continue => {}
                AdvanceStep::Frame => return Ok(true),
                AdvanceStep::End => return Ok(false),
            }
        }
    }

    /// One bounded reader step. The offline wrapper renews its allowance only between these
    /// steps; bytes and decompressor state remain owned by this stream across renewals.
    fn advance_step(&mut self, work: &mut FrameWorkBudget) -> io::Result<AdvanceStep> {
        match self
            .reader
            .next_with_remaining_budget(work)
            .map_err(|e| io::Error::other(format!("frame reader work refused: {e:?}")))?
        {
            Some(Ok(f)) if oa_sealed::in_sealed_window(f.recv_unix_ns) => {
                // A frame whose time is in a sealed window is dropped and counted.
                self.sealed_window_dropped += 1;
                // Its bytes are not counted: `bytes` is the bytes after the window gate.
                self.bytes = self.bytes.saturating_sub(f.raw.len() as u64);
                oa_sealed::note_window_drops(1);
                return Ok(AdvanceStep::Continue);
            }
            Some(Ok(f)) => {
                self.head.recv_ns = f.recv_unix_ns;
                self.head.mono_ns = f.mono_ns();
                self.head.seq = f.seq;
                self.head.offset = f.offset;
                self.head.frame_len = f.raw.len() as u64;
                self.head.kind = f.kind;
                self.head.flags = f.flags;
                self.head.payload.clear();
                self.head.payload.extend_from_slice(f.payload);
                return Ok(AdvanceStep::Frame);
            }
            Some(Err(e)) => {
                note_error(&mut self.errors, &mut self.frame_errors, e);
                return Ok(AdvanceStep::Continue);
            }
            None => {}
        }
        if let Some(rest) = self.pending.get(self.pend_at..).filter(|r| !r.is_empty()) {
            let took = self
                .reader
                .push_with_remaining_budget(rest, work)
                .map_err(|e| io::Error::other(format!("frame reader work refused: {e:?}")))?;
            if took == 0 {
                return Err(io::Error::other("frame reader made no progress"));
            }
            self.pend_at += took;
            return Ok(AdvanceStep::Continue);
        }
        self.pending.clear();
        self.pend_at = 0;
        if self.eof {
            if let Some(e) = self.reader.finish() {
                note_error(&mut self.errors, &mut self.frame_errors, e);
            }
            self.eof = false;
            self.cur = None;
            return Ok(AdvanceStep::Continue);
        }
        if self.cur.is_none() {
            match (self.open)() {
                None => return Ok(AdvanceStep::End),
                Some(Err(e)) => return Err(e),
                Some(Ok(r)) => {
                    self.cur = Some(r);
                    self.files += 1;
                    self.undetermined = true;
                    self.mode = Mode::Raw;
                }
            }
        }
        self.read_more()?;
        Ok(AdvanceStep::Continue)
    }

    /// Reads the next piece of the current file into `pending`, or marks its end.
    fn read_more(&mut self) -> io::Result<()> {
        let Some(r) = self.cur.as_mut() else {
            return Ok(());
        };
        let mut have = 0usize;
        if self.undetermined {
            // The first two bytes decide the mode; a short read must not decide it early.
            while have < 2 {
                let n = r.read(self.inbuf.get_mut(have..).unwrap_or(&mut []))?;
                if n == 0 {
                    break;
                }
                have += n;
            }
            let gz = have >= 2 && self.inbuf.get(..2) == Some(&[0x1f, 0x8b]);
            self.mode = if gz {
                Mode::Gzip(Box::new(GzipReader::new(REPLAY_LIMITS)))
            } else {
                Mode::Raw
            };
            self.undetermined = false;
        } else {
            have = r.read(&mut self.inbuf)?;
        }
        if have == 0 {
            if let Mode::Gzip(g) = &self.mode {
                if g.finish().truncated.is_some() {
                    count_error(&mut self.errors, ErrKind::GzipTruncated, 1);
                }
            }
            self.eof = true;
            return Ok(());
        }
        let data = self.inbuf.get(..have).unwrap_or(&[]);
        match &mut self.mode {
            Mode::Raw => {
                charge_decoded(&self.budget, have)?;
                self.bytes += have as u64;
                self.pending.extend_from_slice(data);
            }
            Mode::Gzip(g) => {
                for piece in data.chunks(GZ_IN) {
                    let budget = self.budget.clone();
                    let bytes = &mut self.bytes;
                    let pending = &mut self.pending;
                    let res = g.push(piece, &mut |b: &[u8]| -> io::Result<()> {
                        charge_decoded(&budget, b.len())?;
                        *bytes += b.len() as u64;
                        pending.extend_from_slice(b);
                        Ok(())
                    });
                    if let Err(oa_zip::GzipError::Sink(error)) = res {
                        return Err(error);
                    } else if res.is_err() {
                        count_error(&mut self.errors, ErrKind::GzipDamaged, 1);
                        self.mode = Mode::Dead;
                        break;
                    }
                }
            }
            Mode::Dead => {}
        }
        Ok(())
    }
}

/// One frame handed to the sink, in merge order.
#[derive(Clone, Copy, Debug)]
pub struct Merged<'a> {
    /// The feed the stream was given as.
    pub feed: FeedKind,
    /// Frame kind.
    pub kind: Kind,
    /// Validated frame flags, including unknown bits preserved by the frame format.
    pub flags: u8,
    /// Bridge wall clock at receipt, ns.
    pub recv_ns: i64,
    /// The recorder's monotonic time at receipt, ns since it started; `None` for a frame of a
    /// recording made without one.
    pub mono_ns: Option<u64>,
    /// The bridge's frame counter.
    pub seq: u64,
    /// Index of the stream in the list given.
    pub stream: u32,
    /// Byte offset in the stream's decoded bytes (continues across chained files).
    pub offset: u64,
    /// The frame's bytes, header and trailer included.
    pub frame_len: u64,
    /// The payload.
    pub payload: &'a [u8],
}

/// What reading found, other than frames.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug)]
pub struct MergeReport {
    /// Files opened.
    pub files: usize,
    /// Decoded bytes read (frame bytes, not file sizes for gzip).
    pub bytes: u64,
    /// Frame and gzip failures by kind.
    pub errors: Errors,
    /// The first [`MAX_KEPT_FRAME_ERRORS`] frame errors with their stream.
    pub frame_errors: Vec<(u32, FrameError)>,
    /// Frames dropped because their time is in a sealed window (OA-SEAL-014).
    pub sealed_window_dropped: u64,
}

/// The sort key of a frame: a total order over any set of frames.
pub fn merge_key(recv_ns: i64, feed: FeedKind, seq: u64, stream: u32, offset: u64) -> MergeKey {
    (recv_ns, feed as u8, seq, stream, offset)
}

/// `(recv_unix_ns, feed kind, seq, stream index, byte offset)`.
pub type MergeKey = (i64, u8, u64, u32, u64);

/// Merges the streams and hands every frame, in order, to `sink`. Memory does not depend on the
/// input length (see the module comment). `chunk` is the read size per stream (2 to
/// [`MAX_CHUNK`]); the result does not depend on it.
pub fn merge_streams(
    specs: Vec<StreamSpec>,
    chunk: usize,
    sink: &mut dyn FnMut(&Merged<'_>),
) -> io::Result<MergeReport> {
    try_merge_streams(specs, chunk, &mut |m| {
        sink(m);
        Ok(())
    })
}

/// [`merge_streams`] with a sink that can fail: the first error it returns stops the merge (no
/// further frame is read or handed on) and is returned.
pub fn try_merge_streams(
    specs: Vec<StreamSpec>,
    chunk: usize,
    sink: &mut dyn FnMut(&Merged<'_>) -> io::Result<()>,
) -> io::Result<MergeReport> {
    let (report, end) = merge_counted(specs, chunk, sink);
    end.map(|()| report)
}

/// The merge itself: what was counted up to the point it stopped (also when the sink stopped it
/// with an error), and how it ended.
pub fn merge_counted(
    specs: Vec<StreamSpec>,
    chunk: usize,
    sink: &mut dyn FnMut(&Merged<'_>) -> io::Result<()>,
) -> (MergeReport, io::Result<()>) {
    merge_counted_inner(specs, chunk, sink, None)
}

/// A whole-merge work/decoded-byte bound, including malformed input that yields no frame.
/// Remaining allowances and reader diagnostics are retained on every exit.
pub fn merge_counted_bounded(
    specs: Vec<StreamSpec>,
    chunk: usize,
    sink: &mut dyn FnMut(&Merged<'_>) -> io::Result<()>,
    budget: &mut MergeBudget,
) -> (MergeReport, io::Result<()>) {
    if specs.len() > 128 || budget.refused {
        return (
            empty_report(),
            Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "bounded merge stream cap or exhausted budget",
            )),
        );
    }
    let shared = Rc::new(Cell::new(*budget));
    let result = merge_counted_inner(specs, chunk, sink, Some(shared.clone()));
    *budget = shared.get();
    result
}

fn merge_counted_inner(
    specs: Vec<StreamSpec>,
    chunk: usize,
    sink: &mut dyn FnMut(&Merged<'_>) -> io::Result<()>,
    budget: Option<SharedBudget>,
) -> (MergeReport, io::Result<()>) {
    let n = specs.len();
    let mut bufs: Vec<Vec<u8>> = (0..n).map(|_| vec![0u8; MAX_FRAME_LEN]).collect();
    let mut streams: Vec<Stream<'_>> = Vec::with_capacity(n);
    for (spec, buf) in specs.into_iter().zip(bufs.iter_mut()) {
        let Ok(reader) = FrameReader::new(buf) else {
            return (
                empty_report(),
                Err(io::Error::other("reader buffer too small")),
            );
        };
        let mut stream = Stream::new(spec, reader, chunk);
        stream.budget = budget.clone();
        streams.push(stream);
    }
    // Capacity n, never more: one entry per stream that still has a frame.
    let mut heap: BinaryHeap<Reverse<(MergeKey, usize)>> = BinaryHeap::with_capacity(n);
    let end = (|| -> io::Result<()> {
        for (i, s) in streams.iter_mut().enumerate() {
            if s.advance()? {
                heap.push(Reverse((
                    merge_key(s.head.recv_ns, s.feed, s.head.seq, i as u32, s.head.offset),
                    i,
                )));
            }
        }
        while let Some(Reverse((_, i))) = heap.pop() {
            let Some(s) = streams.get_mut(i) else { break };
            sink(&Merged {
                feed: s.feed,
                kind: s.head.kind,
                flags: s.head.flags,
                recv_ns: s.head.recv_ns,
                mono_ns: s.head.mono_ns,
                seq: s.head.seq,
                stream: i as u32,
                offset: s.head.offset,
                frame_len: s.head.frame_len,
                payload: &s.head.payload,
            })?;
            if s.advance()? {
                heap.push(Reverse((
                    merge_key(s.head.recv_ns, s.feed, s.head.seq, i as u32, s.head.offset),
                    i,
                )));
            }
        }
        Ok(())
    })();
    let mut report = empty_report();
    for (i, s) in streams.iter().enumerate() {
        report.files += s.files;
        report.bytes += s.bytes;
        report.sealed_window_dropped += s.sealed_window_dropped;
        for (a, b) in report.errors.iter_mut().zip(s.errors.iter()) {
            *a += b;
        }
        for e in &s.frame_errors {
            if report.frame_errors.len() < MAX_KEPT_FRAME_ERRORS {
                report.frame_errors.push((i as u32, *e));
            }
        }
    }
    (report, end)
}

fn empty_report() -> MergeReport {
    MergeReport {
        files: 0,
        bytes: 0,
        errors: [0; ERR_COUNT],
        frame_errors: Vec::new(),
        sealed_window_dropped: 0,
    }
}

/// Merges the streams into the pipeline. The frame and gzip error counts go straight into the
/// pipeline's own counts. `after_frame` is called after every frame (progress lines).
pub fn run_streams(
    p: &mut Pipeline,
    specs: Vec<StreamSpec>,
    chunk: usize,
    after_frame: &mut dyn FnMut(&Pipeline),
) -> io::Result<MergeReport> {
    run_streams_with(p, specs, chunk, &mut |p| {
        after_frame(p);
        Ok(())
    })
}

/// [`run_streams`] with a hook that may take checkpoints out of the pipeline
/// ([`crate::pipeline_drain_checkpoints`]) and may fail: its first error stops the run and is
/// returned, with the frames before it processed.
pub fn run_streams_with(
    p: &mut Pipeline,
    specs: Vec<StreamSpec>,
    chunk: usize,
    after_frame: &mut dyn FnMut(&mut Pipeline) -> io::Result<()>,
) -> io::Result<MergeReport> {
    run_streams_counted(p, specs, chunk, after_frame, &mut 0)
}

/// [`run_streams_with`], with the frames the merge dropped by receive time in a sealed window put
/// in `sealed_frames` on every exit, a failing hook included.
pub fn run_streams_counted(
    p: &mut Pipeline,
    specs: Vec<StreamSpec>,
    chunk: usize,
    after_frame: &mut dyn FnMut(&mut Pipeline) -> io::Result<()>,
    sealed_frames: &mut u64,
) -> io::Result<MergeReport> {
    run_streams_inner(p, specs, chunk, after_frame, sealed_frames, None)
}

/// [`run_streams_counted`] with an observer for frames passed through the pipeline. A message whose
/// document decoder drops it as sealed is omitted from `on_pipeline_frame`; receive-time sealed frames
/// never reach the observer and remain in the returned source refusal count.
pub fn run_streams_observed(
    p: &mut Pipeline,
    specs: Vec<StreamSpec>,
    chunk: usize,
    after_frame: &mut dyn FnMut(&mut Pipeline) -> io::Result<()>,
    sealed_frames: &mut u64,
    on_pipeline_frame: &mut dyn FnMut(&Merged<'_>),
) -> io::Result<MergeReport> {
    run_streams_inner(
        p,
        specs,
        chunk,
        after_frame,
        sealed_frames,
        Some(on_pipeline_frame),
    )
}

fn run_streams_inner(
    p: &mut Pipeline,
    specs: Vec<StreamSpec>,
    chunk: usize,
    after_frame: &mut dyn FnMut(&mut Pipeline) -> io::Result<()>,
    sealed_frames: &mut u64,
    mut on_pipeline_frame: Option<&mut dyn FnMut(&Merged<'_>)>,
) -> io::Result<MergeReport> {
    // The bytes of the frames whose document the decoder dropped whole (OA-SEAL-014): not in
    // the byte count, like the frames dropped by receive time.
    let mut dropped_bytes = 0u64;
    let (mut report, end) = merge_counted(specs, chunk, &mut |m| {
        process_frame_mono(p, m.feed, m.kind, m.recv_ns, m.stream, m.mono_ns, m.payload);
        if p.last_doc_dropped {
            dropped_bytes += m.frame_len;
        } else if let Some(observe) = on_pipeline_frame.as_mut() {
            observe(m);
        }
        after_frame(p)
    });
    report.bytes = report.bytes.saturating_sub(dropped_bytes);
    // The counts go in first, so they are there whether the run ended or a hook failed.
    *sealed_frames = report.sealed_window_dropped;
    for (a, b) in p.errors.iter_mut().zip(report.errors.iter()) {
        *a = a.saturating_add(*b);
    }
    end.map(|()| report)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    // REQ: OA-RPL-276
    #[test]
    fn no_offline_step_exceeds_one_reader_quota() {
        let quota = FrameWorkBudget::for_capacity(MAX_FRAME_LEN);
        // At least five scan quotas of garbage, then one valid frame.
        let garbage = 6 * quota.scan_positions as usize;
        let payload = b"state=up\n";
        let mut tail = vec![0u8; oa_feed::HEADER_LEN + payload.len() + oa_feed::TRAILER_LEN];
        let n = oa_feed::encode_frame(&mut tail, 3, 0, 1, 1, 0, payload).unwrap();
        tail.truncate(n);
        let mut bytes = vec![0u8; garbage];
        bytes.extend_from_slice(&tail);
        let total = bytes.len();

        let mut storage = vec![0u8; MAX_FRAME_LEN];
        let reader = FrameReader::new(&mut storage).unwrap();
        let mut stream = Stream::new(
            spec_from_bytes(FeedKind::Sfdps, vec![bytes]),
            reader,
            MAX_CHUNK,
        );
        let (mut steps, mut worst_scan, mut worst_copy, mut worst_compact, mut worst_crc) =
            (0u64, 0u64, 0u64, 0u64, 0u64);
        assert!(stream
            .advance_observed(&mut |w| {
                steps += 1;
                worst_scan = worst_scan.max(w.scan_positions);
                worst_copy = worst_copy.max(w.copied_bytes);
                worst_compact = worst_compact.max(w.compacted_bytes);
                worst_crc = worst_crc.max(w.crc_bytes);
            })
            .unwrap());
        assert_eq!(stream.head.seq, 1);
        assert!(worst_scan <= quota.scan_positions);
        assert!(worst_copy <= quota.copied_bytes);
        assert!(worst_compact <= quota.compacted_bytes);
        assert!(worst_crc <= quota.crc_bytes);
        // The search really spans many steps, and the reader scanned about the whole garbage.
        assert!(steps >= 5, "{steps}");
        assert!(stream.reader.work_stats().scan_positions >= 5 * quota.scan_positions);
        assert_eq!(stream.reader.work_stats().copied_bytes, total as u64);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod bounded_merge_tests {
    use super::*;
    use std::io::Write;
    // REQ: OA-FLT-044
    #[test]
    fn invalid_bytes_cannot_bypass_whole_reader_work_and_byte_budgets() {
        let bytes = vec![0x55; 128 * 1024];
        let mut budget = MergeBudget {
            steps_remaining: 2,
            decoded_bytes_remaining: 1024 * 1024,
            refused: false,
        };
        let (_, result) = merge_counted_bounded(
            vec![spec_from_bytes(FeedKind::Sfdps, vec![bytes.clone()])],
            4096,
            &mut |_| Ok(()),
            &mut budget,
        );
        assert!(result.is_err());
        assert!(budget.refused);
        assert_eq!(budget.steps_remaining, 0);
        let mut budget = MergeBudget {
            steps_remaining: 1000,
            decoded_bytes_remaining: 1024,
            refused: false,
        };
        let (_, result) = merge_counted_bounded(
            vec![spec_from_bytes(FeedKind::Sfdps, vec![bytes])],
            4096,
            &mut |_| Ok(()),
            &mut budget,
        );
        assert!(result.is_err());
        assert!(budget.refused);
    }

    // REQ: OA-FLT-043, OA-FLT-044
    #[test]
    fn observed_receipt_hashes_the_same_descriptor_through_verified_eof() {
        let dir =
            std::env::temp_dir().join(format!("oa-replay-observed-receipt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("input.oaf");
        let payload = b"receipt frame";
        let mut bytes = vec![0u8; oa_feed::HEADER_LEN + payload.len() + oa_feed::TRAILER_LEN];
        let used = oa_feed::encode_frame(&mut bytes, 3, 0, 1, 7, 9, payload).unwrap();
        bytes.truncate(used);
        std::fs::write(&path, &bytes).unwrap();

        let (spec, inputs, reserved) =
            spec_from_paths_observed(FeedKind::Sfdps, vec![path.clone()]).unwrap();
        assert_eq!(reserved, bytes.len() as u64);
        assert_eq!(inputs[0].reserved_bytes, bytes.len() as u64);
        let mut budget = MergeBudget {
            steps_remaining: 100,
            decoded_bytes_remaining: 1024 * 1024,
            refused: false,
        };
        let mut delivered = 0usize;
        let (report, result) = merge_counted_bounded(
            vec![spec],
            MAX_CHUNK,
            &mut |_| {
                delivered += 1;
                Ok(())
            },
            &mut budget,
        );
        assert!(result.is_ok());
        assert_eq!(report.files, 1);
        assert_eq!(delivered, 1);
        let receipt = inputs[0].receipt.get();
        assert!(receipt.opened);
        assert!(receipt.eof);
        assert_eq!(receipt.bytes, bytes.len() as u64);
        assert_eq!(receipt.sha256, oa_sha256::sha256(&bytes));
        let line = observed_input_line(0, &inputs[0]).unwrap();
        assert!(line.contains("\topened=1\teof=1\t"));
        assert!(line.contains(
            format!("\tbytes={}\treserved_bytes={}\t", bytes.len(), bytes.len()).as_str()
        ));
        let digest = oa_sha256::to_hex(&receipt.sha256);
        assert!(line.contains(digest.as_str()));
        let _ = std::fs::remove_dir_all(dir);
    }

    // REQ: OA-FLT-044
    #[test]
    fn observed_reservation_sums_full_descriptor_lengths_before_any_read() {
        let dir = std::env::temp_dir().join(format!(
            "oa-replay-observed-reservation-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let first = dir.join("a.oaf");
        let second = dir.join("b.oaf");
        let first_bytes = vec![0x11; 17];
        let second_bytes = vec![0x22; 31];
        std::fs::write(&first, &first_bytes).unwrap();
        std::fs::write(&second, &second_bytes).unwrap();

        let (spec, inputs, reserved) =
            spec_from_paths_observed(FeedKind::Sfdps, vec![first.clone(), second.clone()]).unwrap();
        assert_eq!(reserved, 48);
        assert_eq!(
            inputs.iter().map(|input| input.reserved_bytes).sum::<u64>(),
            48
        );
        assert!(inputs.iter().all(|input| input.receipt.get().opened));
        assert!(inputs.iter().all(|input| input.receipt.get().bytes == 0));
        drop(spec);
        assert!(inputs.iter().all(|input| !input.receipt.get().eof));
        let _ = std::fs::remove_dir_all(dir);
    }

    // REQ: OA-FLT-044
    #[test]
    fn a_growing_observed_file_consumes_at_most_one_probe_byte_past_its_reservation() {
        let dir =
            std::env::temp_dir().join(format!("oa-replay-observed-growth-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("growing.oaf");
        let reserved = b"01234567";
        std::fs::write(&path, reserved).unwrap();
        let file = std::fs::File::open(&path).unwrap();
        let expected_bytes = file.metadata().unwrap().len();
        let mut grow = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        grow.write_all(b"abcdefgh").unwrap();
        drop(grow);

        let receipt = Rc::new(Cell::new(InputReceipt {
            opened: true,
            sha256: oa_sha256::sha256(&[]),
            ..InputReceipt::default()
        }));
        let mut input = HashingInput {
            file,
            hash: oa_sha256::Sha256::new(),
            receipt: receipt.clone(),
            expected_bytes,
            bytes: 0,
            eof: false,
        };
        let mut caller_buffer = vec![0u8; 64 * 1024];
        let error = input
            .read(&mut caller_buffer)
            .expect_err("growth probe must refuse the changed file");
        assert!(error.to_string().contains("input grew after guarded open"));
        drop(input);

        let observed = receipt.get();
        assert_eq!(observed.bytes, expected_bytes + 1);
        assert!(!observed.eof);
        assert_eq!(observed.sha256, oa_sha256::sha256(b"01234567a"));
        let _ = std::fs::remove_dir_all(dir);
    }

    // REQ: OA-FLT-044
    #[test]
    fn bounded_merge_records_a_hashed_prefix_but_never_claims_eof_on_cap() {
        let dir =
            std::env::temp_dir().join(format!("oa-replay-observed-prefix-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("invalid.oaf");
        let bytes = vec![0x55; 2 * 1024 * 1024];
        std::fs::write(&path, &bytes).unwrap();

        let (spec, inputs, reserved) =
            spec_from_paths_observed(FeedKind::Sfdps, vec![path.clone()]).unwrap();
        assert_eq!(reserved, bytes.len() as u64);
        assert_eq!(inputs[0].reserved_bytes, bytes.len() as u64);
        let mut budget = MergeBudget {
            steps_remaining: 1,
            decoded_bytes_remaining: 4 * 1024 * 1024,
            refused: false,
        };
        let (_, result) =
            merge_counted_bounded(vec![spec], MAX_CHUNK, &mut |_| Ok(()), &mut budget);
        assert!(result.is_err());
        assert!(budget.refused);
        let receipt = inputs[0].receipt.get();
        assert!(receipt.opened);
        assert!(!receipt.eof);
        assert!(receipt.bytes > 0);
        assert!(receipt.bytes < bytes.len() as u64);
        assert_eq!(
            receipt.sha256,
            oa_sha256::sha256(&bytes[..receipt.bytes as usize])
        );
        let line = observed_input_line(0, &inputs[0]).unwrap();
        assert!(line.contains("\teof=0\t"));
        assert!(line.contains(
            format!(
                "\tbytes={}\treserved_bytes={}\t",
                receipt.bytes,
                bytes.len()
            )
            .as_str()
        ));
        let _ = std::fs::remove_dir_all(dir);
    }
}
