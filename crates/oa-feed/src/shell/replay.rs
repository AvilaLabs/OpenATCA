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

use std::convert::Infallible;
use std::fs::File;
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
}

/// How one source is being read.
enum Mode {
    Raw,
    Gzip(Box<GzipReader>),
    /// The gzip layer failed; the rest of this file is ignored.
    Dead,
}

/// Replays `readers` in order through one health record. Each source gets a fresh
/// [`FrameReader`] (a file never continues another's partial frame); the sequence tracking
/// carries across, since recording rotates on frame boundaries.
pub fn replay_sources<R: Read>(sources: Vec<R>) -> io::Result<Report> {
    let mut health = FeedHealth::new();
    let mut errors = Vec::new();
    let mut gzip = Vec::new();
    let mut bytes = 0u64;
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
        let mut n = have;
        while n > 0 {
            let data = chunk.get(..n).unwrap_or(&[]);
            match &mut mode {
                Mode::Raw => {
                    bytes += n as u64;
                    feed_frames(&mut reader, &mut health, &mut errors, i, data)
                        .map_err(work_refusal_error)?;
                }
                Mode::Gzip(g) => {
                    let r = g.push(data, &mut |b: &[u8]| -> Result<(), FrameWorkRefusal> {
                        bytes += b.len() as u64;
                        feed_frames(&mut reader, &mut health, &mut errors, i, b)?;
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
        drain(&mut reader, &mut health, &mut errors, i).map_err(work_refusal_error)?;
        if let Some(e) = reader.finish() {
            observe_error(&mut health, &e);
            errors.push(FileError { file: i, error: e });
        }
        let total = health.skipped_bytes.saturating_add(reader.skipped_total());
        observe_skipped(&mut health, total);
    }
    Ok(Report {
        health,
        files,
        bytes,
        errors,
        gzip,
    })
}

/// Pushes `data` through the frame reader, taking every frame and error out as it comes.
fn feed_frames(
    reader: &mut FrameReader<'_>,
    health: &mut FeedHealth,
    errors: &mut Vec<FileError>,
    file: usize,
    data: &[u8],
) -> Result<(), FrameWorkRefusal> {
    let mut off = 0usize;
    while off < data.len() {
        let took = reader.push_with_budget(
            data.get(off..).unwrap_or(&[]),
            FrameWorkBudget::for_capacity(MAX_FRAME_LEN),
        )?;
        off += took;
        drain(reader, health, errors, file)?;
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
    file: usize,
) -> Result<(), FrameWorkRefusal> {
    loop {
        let Some(ev) = r.next_with_budget(FrameWorkBudget::for_capacity(MAX_FRAME_LEN))? else {
            return Ok(());
        };
        match ev {
            Ok(f) => observe_frame(h, &f, f.recv_unix_ns),
            Err(e) => {
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
    let mut srcs = Vec::with_capacity(paths.len());
    for p in paths {
        srcs.push(
            File::open(p.as_ref())
                .map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", p.as_ref().display())))?,
        );
    }
    replay_sources(srcs)
}
