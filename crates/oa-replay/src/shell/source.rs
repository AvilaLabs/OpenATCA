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

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use oa_feed::replay::REPLAY_LIMITS;
use oa_feed::{ErrorKind, FrameError, FrameReader, FrameWorkBudget, Kind, MAX_FRAME_LEN};
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
                File::open(&p)
                    .map(|f| Box::new(f) as Box<dyn Read>)
                    .map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", p.display()))),
            )
        }),
    }
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
    for e in std::fs::read_dir(dir)
        .map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", dir.display())))?
    {
        let p = e?.path();
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
    kind: Kind,
    payload: Vec<u8>,
}

struct Stream<'b> {
    feed: FeedKind,
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

impl Stream<'_> {
    /// Moves to the stream's next frame, which lands in `head`. `Ok(false)` at the end.
    ///
    /// This is an offline file iterator: it renews the finite per-step reader allowance until it
    /// finds a frame or reaches EOF. A file may be arbitrarily long, so this search has no
    /// whole-file deadline bound.
    fn advance(&mut self) -> io::Result<bool> {
        loop {
            let mut work = FrameWorkBudget::for_capacity(MAX_FRAME_LEN);
            match self.advance_step(&mut work)? {
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
            Some(Ok(f)) => {
                self.head.recv_ns = f.recv_unix_ns;
                self.head.mono_ns = f.mono_ns();
                self.head.seq = f.seq;
                self.head.offset = f.offset;
                self.head.kind = f.kind;
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
                self.bytes += have as u64;
                self.pending.extend_from_slice(data);
            }
            Mode::Gzip(g) => {
                for piece in data.chunks(GZ_IN) {
                    let bytes = &mut self.bytes;
                    let pending = &mut self.pending;
                    let res = g.push(
                        piece,
                        &mut |b: &[u8]| -> Result<(), std::convert::Infallible> {
                            *bytes += b.len() as u64;
                            pending.extend_from_slice(b);
                            Ok(())
                        },
                    );
                    if res.is_err() {
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

/// The merge itself: what was counted up to the point it stopped, and how it ended.
fn merge_counted(
    specs: Vec<StreamSpec>,
    chunk: usize,
    sink: &mut dyn FnMut(&Merged<'_>) -> io::Result<()>,
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
        streams.push(Stream {
            feed: spec.feed,
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
                kind: Kind::Message,
                payload: Vec::new(),
            },
            bytes: 0,
            files: 0,
            errors: [0; ERR_COUNT],
            frame_errors: Vec::new(),
        });
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
                recv_ns: s.head.recv_ns,
                mono_ns: s.head.mono_ns,
                seq: s.head.seq,
                stream: i as u32,
                offset: s.head.offset,
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
    let (report, end) = merge_counted(specs, chunk, &mut |m| {
        process_frame_mono(p, m.feed, m.kind, m.recv_ns, m.stream, m.mono_ns, m.payload);
        after_frame(p)
    });
    // The counts go in first, so they are there whether the run ended or a hook failed.
    for (a, b) in p.errors.iter_mut().zip(report.errors.iter()) {
        *a = a.saturating_add(*b);
    }
    end.map(|()| report)
}
