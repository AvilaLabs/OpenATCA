//! Replay: recorded `.oaf` files read back through the same [`FrameReader`] and [`FeedHealth`]
//! as a live feed. Shell (reads files); the figures depend only on frame fields, never on the
//! clock (B6).

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use crate::frame::{FrameError, MAX_FRAME_LEN};
use crate::health::{observe_error, observe_frame, observe_skipped, FeedHealth};
use crate::reader::FrameReader;

/// A refused frame found while replaying, with the file it was in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileError {
    /// Index into the list of files given.
    pub file: usize,
    /// The frame error.
    pub error: FrameError,
}

/// Result of replaying one or more files.
#[derive(Clone, Debug)]
pub struct Report {
    /// Health after every frame.
    pub health: FeedHealth,
    /// Files read.
    pub files: usize,
    /// Bytes read.
    pub bytes: u64,
    /// Every refused frame.
    pub errors: Vec<FileError>,
}

/// Replays `readers` in order through one health record. Each source gets a fresh
/// [`FrameReader`] (a file never continues another's partial frame); the sequence tracking
/// carries across, since recording rotates on frame boundaries.
pub fn replay_sources<R: Read>(sources: Vec<R>) -> io::Result<Report> {
    let mut health = FeedHealth::new();
    let mut errors = Vec::new();
    let mut bytes = 0u64;
    let mut files = 0usize;
    let mut storage = vec![0u8; 2 * MAX_FRAME_LEN];
    let mut chunk = vec![0u8; 64 * 1024];
    for (i, mut src) in sources.into_iter().enumerate() {
        files += 1;
        let Ok(mut reader) = FrameReader::new(&mut storage) else {
            return Err(io::Error::other("reader buffer too small"));
        };
        loop {
            let n = src.read(&mut chunk)?;
            if n == 0 {
                break;
            }
            bytes += n as u64;
            let mut off = 0usize;
            while off < n {
                let part = chunk.get(off..n).unwrap_or(&[]);
                let took = reader.push(part);
                off += took;
                drain(&mut reader, &mut health, &mut errors, i);
                if took == 0 {
                    break;
                }
            }
        }
        drain(&mut reader, &mut health, &mut errors, i);
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
    })
}

fn drain(r: &mut FrameReader<'_>, h: &mut FeedHealth, errors: &mut Vec<FileError>, file: usize) {
    while let Some(ev) = r.next() {
        match ev {
            Ok(f) => observe_frame(h, &f, f.recv_unix_ns),
            Err(e) => {
                observe_error(h, &e);
                errors.push(FileError { file, error: e });
            }
        }
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
