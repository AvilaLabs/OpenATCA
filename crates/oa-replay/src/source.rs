//! Reading recordings and merging them into one ordered stream. Shell (B1): reads files; its
//! output depends only on the bytes it reads, never on the clock or on how they were chunked.
//!
//! A recording is `.oaf` (raw frames) or `.oaf.gz`, told apart by its first two bytes as
//! `oa-feed replay` does. Each frame is kept as a small record plus its payload in one arena, so
//! the whole input is held in memory (a few hundred megabytes at most for the recordings made
//! so far; this is an offline tool, C6). Frames that fail validation, a damaged or truncated
//! gzip layer and a file ending inside a frame are counted under their [`ErrKind`] and never
//! passed on (IF-R5).
//!
//! The merge order is `(recv_unix_ns, feed kind, seq)`, then the file's position on the command
//! line and the frame's byte offset in its file. The first three alone are total across files
//! recorded by one bridge run; the last two make the order total for any input, including a
//! file given twice or two bridge runs whose counters restarted, and make the sort's result
//! independent of the sort algorithm.

use std::convert::Infallible;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use oa_feed::replay::REPLAY_LIMITS;
use oa_feed::{ErrorKind, FrameError, FrameReader, Kind, MAX_FRAME_LEN};
use oa_zip::GzipReader;

use crate::errors::{count_error, ErrKind, Errors, ERR_COUNT};
use crate::pipeline::{process_frame, FeedKind, Pipeline};

/// Frame errors kept for reporting; all are counted.
pub const MAX_KEPT_FRAME_ERRORS: usize = 1000;

/// One frame's record. The payload is in the owning [`Loaded`] arena.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct Rec {
    /// Bridge wall clock at receipt, ns since the Unix epoch.
    pub recv_ns: i64,
    /// The bridge's frame counter.
    pub seq: u64,
    /// Byte offset of the frame in its (decoded) file.
    pub offset: u64,
    /// Start of the payload in the arena.
    pub start: usize,
    /// Payload length.
    pub len: u32,
    /// Position of the file on the command line.
    pub file: u32,
    /// Which feed the file was given as.
    pub feed: FeedKind,
    /// Frame kind.
    pub kind: Kind,
}
// Budget: held in quantity (one per frame, hundreds of thousands): 8 + 8 + 8 + 8 + 4 + 4 + 1 + 1
// bytes, padded to 48.
const _: () = assert!(core::mem::size_of::<Rec>() == 48);

/// Everything read from the recordings.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug)]
pub struct Loaded {
    /// One record per valid frame, in file order until [`merge_order`] sorts them.
    pub recs: Vec<Rec>,
    /// Payload bytes of every record.
    pub arena: Vec<u8>,
    /// Files read.
    pub files: usize,
    /// Decoded bytes read (frame bytes, not file sizes for gzip).
    pub bytes: u64,
    /// Frame, gzip and file-read failures by kind.
    pub errors: Errors,
    /// The first [`MAX_KEPT_FRAME_ERRORS`] frame errors with their file.
    pub frame_errors: Vec<(u32, FrameError)>,
}

/// The payload of `r`.
pub fn payload<'a>(l: &'a Loaded, r: &Rec) -> &'a [u8] {
    l.arena
        .get(r.start..r.start.saturating_add(r.len as usize))
        .unwrap_or(&[])
}

enum Mode {
    Raw,
    Gzip(Box<GzipReader>),
    Dead,
}

/// Reads every source, `chunk` bytes at a time (at least 2). The result does not depend on
/// `chunk`.
pub fn load_sources<R: Read>(sources: Vec<(FeedKind, R)>, chunk: usize) -> io::Result<Loaded> {
    let mut out = Loaded {
        recs: Vec::new(),
        arena: Vec::new(),
        files: 0,
        bytes: 0,
        errors: [0; ERR_COUNT],
        frame_errors: Vec::new(),
    };
    let mut storage = vec![0u8; 2 * MAX_FRAME_LEN];
    let mut buf = vec![0u8; chunk.max(2)];
    for (i, (feed, mut src)) in sources.into_iter().enumerate() {
        let file = i as u32;
        out.files += 1;
        let Ok(mut reader) = FrameReader::new(&mut storage) else {
            return Err(io::Error::other("reader buffer too small"));
        };
        // The first two bytes decide the mode; a short read must not decide it early.
        let mut have = 0usize;
        while have < 2 {
            let n = src.read(buf.get_mut(have..).unwrap_or(&mut []))?;
            if n == 0 {
                break;
            }
            have += n;
        }
        let mut mode = if have >= 2 && buf.get(..2) == Some(&[0x1f, 0x8b]) {
            Mode::Gzip(Box::new(GzipReader::new(REPLAY_LIMITS)))
        } else {
            Mode::Raw
        };
        let mut n = have;
        while n > 0 {
            let data = buf.get(..n).unwrap_or(&[]);
            match &mut mode {
                Mode::Raw => {
                    out.bytes += n as u64;
                    take(&mut out, &mut reader, feed, file, data);
                }
                Mode::Gzip(g) => {
                    let r = g.push(data, &mut |b: &[u8]| -> Result<(), Infallible> {
                        out.bytes += b.len() as u64;
                        take(&mut out, &mut reader, feed, file, b);
                        Ok(())
                    });
                    if r.is_err() {
                        count_error(&mut out.errors, ErrKind::GzipDamaged, 1);
                        mode = Mode::Dead;
                    }
                }
                Mode::Dead => {}
            }
            n = src.read(&mut buf)?;
        }
        if let Mode::Gzip(g) = &mode {
            if g.finish().truncated.is_some() {
                count_error(&mut out.errors, ErrKind::GzipTruncated, 1);
            }
        }
        drain(&mut out, &mut reader, feed, file);
        if let Some(e) = reader.finish() {
            note_error(&mut out, file, e);
        }
    }
    Ok(out)
}

/// Reads recordings by path.
pub fn load_files(files: &[(FeedKind, PathBuf)], chunk: usize) -> io::Result<Loaded> {
    let mut srcs = Vec::with_capacity(files.len());
    for (feed, p) in files {
        srcs.push((*feed, open(p)?));
    }
    load_sources(srcs, chunk)
}

fn open(p: &Path) -> io::Result<File> {
    File::open(p).map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", p.display())))
}

fn take(out: &mut Loaded, reader: &mut FrameReader<'_>, feed: FeedKind, file: u32, data: &[u8]) {
    let mut off = 0usize;
    while off < data.len() {
        let took = reader.push(data.get(off..).unwrap_or(&[]));
        off += took;
        drain(out, reader, feed, file);
        if took == 0 {
            break;
        }
    }
}

fn drain(out: &mut Loaded, r: &mut FrameReader<'_>, feed: FeedKind, file: u32) {
    while let Some(ev) = r.next() {
        match ev {
            Ok(f) => {
                let start = out.arena.len();
                out.arena.extend_from_slice(f.payload);
                out.recs.push(Rec {
                    recv_ns: f.recv_unix_ns,
                    seq: f.seq,
                    offset: f.offset,
                    start,
                    len: f.payload.len() as u32,
                    file,
                    feed,
                    kind: f.kind,
                });
            }
            Err(e) => note_error(out, file, e),
        }
    }
}

fn note_error(out: &mut Loaded, file: u32, e: FrameError) {
    let k = match e.kind {
        ErrorKind::BadMagic { .. } => ErrKind::FrameBadMagic,
        ErrorKind::UnknownKind { .. } => ErrKind::FrameUnknownKind,
        ErrorKind::Oversize { .. } => ErrKind::FrameOversize,
        ErrorKind::Crc { .. } => ErrKind::FrameCrc,
        ErrorKind::Truncated { .. } => ErrKind::FrameTruncated,
    };
    count_error(&mut out.errors, k, 1);
    if out.frame_errors.len() < MAX_KEPT_FRAME_ERRORS {
        out.frame_errors.push((file, e));
    }
}

/// The sort key of a record: a total order over any set of frames.
pub fn merge_key(r: &Rec) -> (i64, u8, u64, u32, u64) {
    (r.recv_ns, r.feed as u8, r.seq, r.file, r.offset)
}

/// Sorts the records into merge order.
pub fn merge_order(l: &mut Loaded) {
    l.recs.sort_unstable_by_key(merge_key);
}

/// Feeds every record, in the order held, to the pipeline.
pub fn run_loaded(p: &mut Pipeline, l: &Loaded) {
    for r in &l.recs {
        process_frame(p, r.feed, r.kind, r.recv_ns, payload(l, r));
    }
}
