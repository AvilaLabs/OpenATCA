//! Frame recording: frames appended to size-rotated files, gzip-compressed as they are written
//! (`.oaf.gz`, the default) or verbatim (`.oaf`). Shell (file system); the clock is passed in as
//! a value so naming is testable. Frame stamps never drive flushing or naming.
//!
//! Files are named `<feed>-<UTC start>-<NNNN>.oaf.gz` or `.oaf` (UTC as `YYYYMMDDTHHMMSSZ`;
//! `NNNN` counts from 0001 within one naming second). A startup watermark from existing names
//! for this feed keeps order across a process restart and a backward local-clock step. This
//! assumes one writer for a feed and directory; `create_new` prevents overwrite but does not
//! coordinate concurrent recorders or deduplicate broker deliveries. Frames are never split
//! across files. The total size of every `.oaf` and `.oaf.gz` file in the directory is capped; at
//! the cap recording stops, once, with a warning, and nothing is deleted (a recording is
//! evidence, A8).
//!
//! Compressed recordings (oa-deflate, level 6) are a sequence of gzip members, one per file. The
//! compressor is sync-flushed when the local whole second changes after the last flush, and by
//! [`Recorder::idle_flush`] when the feed is quiet. With the default writer this helps recovery
//! after a process crash while the operating system remains running; it is not a power-loss
//! guarantee. Each file is finished (final block and CRC-32 trailer) on rotation, at the cap and
//! in [`Recorder::finish`]. The opt-in [`Recorder::open_sync_closed_segments`] requests file and
//! containing-directory syncs for closed segments only. Those synchronous operations can delay
//! capture; they are OS requests, not certification of hardware power-loss behavior. Rotation and
//! the cap count bytes accepted by the writer, not confirmed persistent bytes: the compressor
//! holds a little back, so a file can pass the rotation size, and the total the cap, by the output
//! of one flush interval before the next check sees it. A file cut by a process crash is read back
//! up to its last flush (oa-zip `GzipReader`).
//!
//! A write, flush or finish that fails (a full disk) is a persistent failed state, not a
//! warning: the recorder keeps the first error, stops writing, and counts every frame handed to
//! it afterwards as lost ([`Recorder::failure`], [`Recorder::lost_frames`], [`format_recording`]),
//! so the shell can report it and exit non-zero. A stop at the size cap exits non-zero too
//! ([`Recorder::exit_reason`]): the run did not record what it was asked to.
//! A closed-segment sync failure also stops recording, but its segment is reported as persistence
//! unconfirmed rather than as known lost frames. In sync mode, a generic write/flush/finalize
//! failure likewise reports accepted frames in the active segment not counted as lost from the
//! unflushed compressor interval; a triggering write frame remains lost.
//!
//! Startup counts existing recordings across at most [`DEFAULT_MAX_DIRECTORY_ENTRIES`] directory
//! entries. This is an operational enumeration cap, not an elapsed-time guarantee; unrelated and
//! zero-byte entries count toward it too.

use std::convert::Infallible;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use oa_deflate::{GzipWriter, DEFAULT_LEVEL};

/// Maximum directory entries inspected when a recorder counts existing recordings at startup.
pub const DEFAULT_MAX_DIRECTORY_ENTRIES: usize = 16_384;

/// How frames are stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// Gzip members at level 6, `.oaf.gz`.
    Gzip,
    /// The frames' own bytes, `.oaf`.
    Raw,
}

/// What [`write_frame`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The frame was appended.
    Written,
    /// The cap was reached by this frame: recording has just stopped (print the warning).
    StoppedNow,
    /// Recording had already stopped; the frame was not written.
    Stopped,
    /// Recording has failed (an earlier write error): the frame was not written and is counted
    /// as lost.
    Failed,
}

/// Creates the file for a new recording at the given path, failing if it exists. The default
/// creates a real file; tests pass one whose writes fail on demand.
pub type Opener = Box<dyn FnMut(&Path) -> io::Result<Box<dyn Write + Send>> + Send>;

trait SyncingWriter: Write + Send {
    fn sync_all(&mut self) -> io::Result<()>;
}

type SyncingOpener = Box<dyn FnMut(&Path) -> io::Result<Box<dyn SyncingWriter>> + Send>;
type DirectorySyncer = Box<dyn FnMut() -> io::Result<()> + Send>;

enum RecorderOpener {
    FlushOnly(Opener),
    SyncClosedSegments {
        file: SyncingOpener,
        directory: DirectorySyncer,
    },
}

enum RecordingFile {
    FlushOnly(Box<dyn Write + Send>),
    Syncing(Box<dyn SyncingWriter>),
}

impl RecordingFile {
    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        match self {
            Self::FlushOnly(file) => file.write_all(bytes),
            Self::Syncing(file) => file.write_all(bytes),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::FlushOnly(file) => file.flush(),
            Self::Syncing(file) => file.flush(),
        }
    }

    fn is_syncing(&self) -> bool {
        matches!(self, Self::Syncing(_))
    }

    fn sync_all(&mut self) -> io::Result<()> {
        match self {
            Self::FlushOnly(_) => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "file sync requested for a flush-only recorder",
            )),
            Self::Syncing(file) => file.sync_all(),
        }
    }
}

#[cfg(target_os = "linux")]
struct SyncingFile(File);

#[cfg(target_os = "linux")]
impl Write for SyncingFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

#[cfg(target_os = "linux")]
impl SyncingWriter for SyncingFile {
    fn sync_all(&mut self) -> io::Result<()> {
        self.0.sync_all()
    }
}

/// The open file: bytes accepted by the writer, the compressor, and when it was last flushed.
// A7.2: cold struct, size guard waived
struct Current {
    file: RecordingFile,
    disk: u64,
    gz: Option<GzipWriter>,
    flushed_secs: i64,
    frames: u64,
}

/// Recording state for one feed. A subsystem under D5: it owns the open file.
pub struct Recorder {
    dir: PathBuf,
    feed: String,
    format: Format,
    rotate_bytes: u64,
    max_bytes: u64,
    total: u64,
    current: Option<Current>,
    scratch: Vec<u8>,
    stopped: bool,
    files_opened: u32,
    last_file_name: Option<(String, u32)>,
    rotations_after_clock_step_back: u64,
    implausible_stamps: u64,
    /// Keeps the checked Linux directory descriptor alive for descriptor-anchored creation.
    _directory_anchor: Option<File>,
    opener: RecorderOpener,
    sync_started: u64,
    sync_completed: u64,
    sync_failures: u64,
    sync_failed_stage: Option<&'static str>,
    persistence_unconfirmed_frames: u64,
    /// The first error that failed the recording; once set, nothing more is written.
    failed: Option<String>,
    lost_frames: u64,
    /// Frames given to the compressor since its last successful flush: accepted by the writer
    /// only once the next flush or finish succeeds, and lost if that write or flush fails.
    unflushed: u64,
}

/// A frame stamp this many seconds from local time is counted as implausible.
const MAX_STAMP_SKEW_SECS: u64 = 3600;

/// Most recordings that can be opened in one UTC second (the four-digit name counter).
const MAX_SAME_SECOND: u32 = 9999;

/// Longest feed label, characters.
pub const MAX_FEED_LABEL_LEN: usize = 64;

/// A feed label becomes part of every recording's file name, so it must be one safe filename
/// component: 1 to [`MAX_FEED_LABEL_LEN`] ASCII letters, digits, `-`, `_` and `.`, not starting
/// with `.` (no separator, no `..`, no hidden file, no NUL).
pub fn check_feed_label(label: &str) -> Result<(), String> {
    if label.is_empty() || label.len() > MAX_FEED_LABEL_LEN {
        return Err(format!(
            "a feed label is 1 to {MAX_FEED_LABEL_LEN} characters, got {}",
            label.len()
        ));
    }
    if label.starts_with('.') {
        return Err("a feed label does not start with '.'".into());
    }
    if !label
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    {
        return Err("a feed label is ASCII letters, digits, '-', '_' and '.' only".into());
    }
    Ok(())
}

fn is_recording(p: &Path) -> bool {
    p.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.ends_with(".oaf") || n.ends_with(".oaf.gz"))
}

fn recording_name_watermark(path: &Path, feed: &str) -> Option<(String, u32)> {
    let name = path.file_name()?.to_str()?;
    let stem = name
        .strip_suffix(".oaf.gz")
        .or_else(|| name.strip_suffix(".oaf"))?;
    let body = stem.strip_prefix(feed)?.strip_prefix('-')?;
    let (stamp, number) = body.rsplit_once('-')?;
    if number.len() != 4 || !number.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let number = number.parse::<u32>().ok()?;
    if !(1..=MAX_SAME_SECOND).contains(&number) {
        return None;
    }
    Some((canonical_utc_stamp(stamp)?.to_owned(), number))
}

/// Validates the fixed-width UTC stamp used in recording names. Its byte order is chronological.
fn canonical_utc_stamp(stamp: &str) -> Option<&str> {
    let bytes = stamp.as_bytes();
    if bytes.len() != 16
        || bytes.get(8) != Some(&b'T')
        || bytes.get(15) != Some(&b'Z')
        || !bytes
            .iter()
            .enumerate()
            .all(|(i, b)| i == 8 || i == 15 || b.is_ascii_digit())
    {
        return None;
    }
    let field = |start, end| {
        std::str::from_utf8(bytes.get(start..end)?)
            .ok()?
            .parse::<u32>()
            .ok()
    };
    let (year, month, day) = (field(0, 4)?, field(4, 6)?, field(6, 8)?);
    let (hour, minute, second) = (field(9, 11)?, field(11, 13)?, field(13, 15)?);
    if !(1..=9999).contains(&year)
        || !(1..=12).contains(&month)
        || !(0..=23).contains(&hour)
        || !(0..=59).contains(&minute)
        || !(0..=59).contains(&second)
    {
        return None;
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days_in_month = match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    if !(1..=days_in_month).contains(&day) {
        return None;
    }

    Some(stamp)
}

#[cfg(target_os = "linux")]
fn checked_directory_anchor(dir: &Path) -> io::Result<(PathBuf, File)> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::MetadataExt;

    oa_sealed::refuse_io_embedded(dir)?;
    let canonical = fs::canonicalize(dir)?;
    oa_sealed::refuse_io_embedded(&canonical)?;
    // seal-allow: probes identity metadata for the checked directory before opening its descriptor
    let expected = fs::metadata(&canonical)?;
    // seal-allow: opens the caller-checked existing recording directory; it reads no contents
    let directory = File::open(&canonical)?;
    let opened = directory.metadata()?;
    if !opened.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "sync-closed-segments requires an existing directory",
        ));
    }
    if (expected.dev(), expected.ino()) != (opened.dev(), opened.ino()) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "recording directory changed while its sync descriptor was opened",
        ));
    }
    oa_sealed::Sealed::production_embedded().verify_open(&canonical, &directory)?;
    let anchor = PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()));
    oa_sealed::refuse_io_embedded(&anchor)?;
    // seal-allow: probes the type of the checked /proc/self/fd directory alias
    if !fs::metadata(&anchor)?.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "the checked directory descriptor is not reachable through /proc/self/fd",
        ));
    }
    Ok((anchor, directory))
}

impl Recorder {
    /// Prepares `dir` (created if missing) and counts the bytes of existing `.oaf` and
    /// `.oaf.gz` files toward the cap. Startup refuses directories with more than
    /// [`DEFAULT_MAX_DIRECTORY_ENTRIES`] total entries, including unrelated entries.
    /// The caller must provide one active writer per `(dir, feed)`; `create_new` prevents
    /// overwrites but does not coordinate concurrent recorders.
    pub fn open(
        dir: &Path,
        feed: &str,
        rotate_bytes: u64,
        max_bytes: u64,
        format: Format,
    ) -> io::Result<Recorder> {
        let opener: Opener = Box::new(|path: &Path| {
            // seal-allow: creates a recording file (create_new); reads nothing
            let f: File = OpenOptions::new().write(true).create_new(true).open(path)?;
            Ok(Box::new(f) as Box<dyn Write + Send>)
        });
        Recorder::open_with(dir, feed, rotate_bytes, max_bytes, format, opener)
    }

    /// Opens an existing recording directory and requests OS syncs at each closed segment.
    /// This mode is Linux-only and does not create the directory: its ancestors must already
    /// exist. File data is synced before the containing directory entry. An open segment is not
    /// power-loss durable until it closes; sync completion is an OS report, not hardware
    /// certification.
    #[cfg(target_os = "linux")]
    pub fn open_sync_closed_segments(
        dir: &Path,
        feed: &str,
        rotate_bytes: u64,
        max_bytes: u64,
        format: Format,
    ) -> io::Result<Recorder> {
        check_feed_label(feed).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        let (anchor_path, directory_anchor) = checked_directory_anchor(dir)?;
        use std::os::fd::AsRawFd;
        let fd = directory_anchor.as_raw_fd();
        let opener: SyncingOpener = Box::new(move |path| {
            let name = path.file_name().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "recording file has no basename",
                )
            })?;
            let anchored = PathBuf::from(format!("/proc/self/fd/{fd}")).join(name);
            oa_sealed::refuse_io_embedded(&anchored)?;
            // seal-allow: creates a recording beside the checked directory descriptor; verifies the open file below
            let file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&anchored)?;
            oa_sealed::Sealed::production_embedded().verify_open(&anchored, &file)?;
            Ok(Box::new(SyncingFile(file)) as Box<dyn SyncingWriter>)
        });
        let directory_sync_file = directory_anchor.try_clone()?;
        let directory_sync: DirectorySyncer = Box::new(move || directory_sync_file.sync_all());
        Self::open_inner(
            &anchor_path,
            feed,
            rotate_bytes,
            max_bytes,
            format,
            RecorderOpener::SyncClosedSegments {
                file: opener,
                directory: directory_sync,
            },
            Some(directory_anchor),
        )
    }

    /// Directory-entry sync is only provided on Linux; other platforms fail explicitly.
    #[cfg(not(target_os = "linux"))]
    pub fn open_sync_closed_segments(
        _dir: &Path,
        _feed: &str,
        _rotate_bytes: u64,
        _max_bytes: u64,
        _format: Format,
    ) -> io::Result<Recorder> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "closed-segment file and directory sync is currently supported on Linux only",
        ))
    }

    /// As [`Recorder::open`], creating each file through `opener` (for a writer that fails on
    /// demand in tests). The same startup directory-entry limit applies.
    pub fn open_with(
        dir: &Path,
        feed: &str,
        rotate_bytes: u64,
        max_bytes: u64,
        format: Format,
        opener: Opener,
    ) -> io::Result<Recorder> {
        check_feed_label(feed).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        fs::create_dir_all(dir)?;
        Self::open_inner(
            dir,
            feed,
            rotate_bytes,
            max_bytes,
            format,
            RecorderOpener::FlushOnly(opener),
            None,
        )
    }

    fn open_inner(
        dir: &Path,
        feed: &str,
        rotate_bytes: u64,
        max_bytes: u64,
        format: Format,
        opener: RecorderOpener,
        directory_anchor: Option<File>,
    ) -> io::Result<Recorder> {
        check_feed_label(feed).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        let existing = existing_recording_state(dir, feed, DEFAULT_MAX_DIRECTORY_ENTRIES)?;
        Ok(Recorder {
            dir: dir.to_path_buf(),
            feed: feed.to_string(),
            format,
            rotate_bytes: rotate_bytes.max(1),
            max_bytes,
            total: existing.bytes,
            current: None,
            scratch: Vec::new(),
            stopped: false,
            files_opened: 0,
            last_file_name: existing.last_file_name,
            rotations_after_clock_step_back: 0,
            implausible_stamps: 0,
            _directory_anchor: directory_anchor,
            opener,
            sync_started: 0,
            sync_completed: 0,
            sync_failures: 0,
            sync_failed_stage: None,
            persistence_unconfirmed_frames: 0,
            failed: None,
            lost_frames: 0,
            unflushed: 0,
        })
    }

    #[cfg(all(test, target_os = "linux"))]
    fn open_sync_closed_segments_with_hooks(
        dir: &Path,
        feed: &str,
        rotate_bytes: u64,
        max_bytes: u64,
        format: Format,
        file: SyncingOpener,
        directory: DirectorySyncer,
    ) -> io::Result<Recorder> {
        check_feed_label(feed).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        let (anchor, directory_anchor) = checked_directory_anchor(dir)?;
        Self::open_inner(
            &anchor,
            feed,
            rotate_bytes,
            max_bytes,
            format,
            RecorderOpener::SyncClosedSegments { file, directory },
            Some(directory_anchor),
        )
    }

    /// Appends one whole frame, rotating first if needed. `now_unix_secs` is the local clock,
    /// passed in by the caller: it names a new file and times the sync flush, so one frame with a
    /// wild stamp cannot stall flushing or name a file for the wrong time. `frame_unix_secs` is
    /// the frame's own receive stamp, used only to count stamps far from local time.
    pub fn write_frame(
        &mut self,
        raw: &[u8],
        frame_unix_secs: i64,
        now_unix_secs: i64,
    ) -> io::Result<Outcome> {
        if self.failed.is_some() {
            self.lost_frames = self.lost_frames.saturating_add(1);
            return Ok(Outcome::Failed);
        }
        match self.write_frame_inner(raw, frame_unix_secs, now_unix_secs) {
            Err(e) => {
                self.lost_frames = self.lost_frames.saturating_add(1);
                Err(self.fail(e))
            }
            ok => ok,
        }
    }

    fn write_frame_inner(
        &mut self,
        raw: &[u8],
        frame_unix_secs: i64,
        now_unix_secs: i64,
    ) -> io::Result<Outcome> {
        if now_unix_secs.abs_diff(frame_unix_secs) > MAX_STAMP_SKEW_SECS {
            self.implausible_stamps = self.implausible_stamps.saturating_add(1);
        }
        if self.stopped {
            return Ok(Outcome::Stopped);
        }
        let len = raw.len() as u64;
        let over_cap = match self.format {
            Format::Raw => self.total.saturating_add(len) > self.max_bytes,
            // The compressed size of this frame is not known yet; stop once the disk is full.
            Format::Gzip => self.total >= self.max_bytes,
        };
        if over_cap {
            self.finish()?;
            self.stopped = true;
            return Ok(Outcome::StoppedNow);
        }
        let rotate = match &self.current {
            Some(c) => match self.format {
                Format::Raw => c.disk > 0 && c.disk.saturating_add(len) > self.rotate_bytes,
                Format::Gzip => c.disk >= self.rotate_bytes,
            },
            None => true,
        };
        if rotate {
            self.finish()?;
            self.current = Some(self.create(now_unix_secs)?);
        }
        let Some(cur) = self.current.as_mut() else {
            return Ok(Outcome::Written);
        };
        self.scratch.clear();
        let mut flushed = false;
        let compressed = cur.gz.is_some();
        match cur.gz.as_mut() {
            Some(gz) => {
                let mut sink = |b: &[u8]| -> Result<(), Infallible> {
                    self.scratch.extend_from_slice(b);
                    Ok(())
                };
                let mut r = gz.write(raw, &mut sink);
                if r.is_ok() && now_unix_secs != cur.flushed_secs {
                    cur.flushed_secs = now_unix_secs;
                    r = gz.flush(&mut sink);
                    flushed = true;
                }
                if let Err(never) = r {
                    match never {}
                }
            }
            None => self.scratch.extend_from_slice(raw),
        }
        cur.file.write_all(&self.scratch)?;
        let n = self.scratch.len() as u64;
        cur.disk += n;
        self.total += n;
        if flushed {
            self.unflushed = 0;
        } else if compressed {
            self.unflushed = self.unflushed.saturating_add(1);
        }
        cur.frames = cur.frames.saturating_add(1);
        Ok(Outcome::Written)
    }

    /// Sync-flushes the compressor once the local whole second changes; for a feed
    /// that has gone quiet, so frames already written do not sit in the compressor. A flush with
    /// nothing new writes nothing. A no-op for raw recordings.
    pub fn idle_flush(&mut self, now_unix_secs: i64) -> io::Result<()> {
        if self.failed.is_some() {
            return Ok(());
        }
        self.idle_flush_inner(now_unix_secs)
            .map_err(|e| self.fail(e))
    }

    fn idle_flush_inner(&mut self, now_unix_secs: i64) -> io::Result<()> {
        let Some(cur) = self.current.as_mut() else {
            return Ok(());
        };
        let Some(gz) = cur.gz.as_mut() else {
            return Ok(());
        };
        if now_unix_secs == cur.flushed_secs {
            return Ok(());
        }
        cur.flushed_secs = now_unix_secs;
        self.scratch.clear();
        let mut sink = |b: &[u8]| -> Result<(), Infallible> {
            self.scratch.extend_from_slice(b);
            Ok(())
        };
        if let Err(never) = gz.flush(&mut sink) {
            match never {}
        }
        cur.file.write_all(&self.scratch)?;
        let n = self.scratch.len() as u64;
        cur.disk += n;
        self.total += n;
        self.unflushed = 0;
        Ok(())
    }

    /// Ends the open file: the compressed member gets its final block and trailer, and the writer
    /// is flushed. In sync-closed-segments mode the file and its containing directory are then
    /// synced. Call at exit; rotation and the cap do it themselves.
    pub fn finish(&mut self) -> io::Result<()> {
        if self.failed.is_some() {
            // Failed earlier: the open file is abandoned as it stands (it replays up to its last
            // flush) and the failure is already recorded.
            self.current = None;
            return Ok(());
        }
        self.finish_inner().map_err(|e| {
            if self.failed.is_some() {
                e
            } else {
                self.fail(e)
            }
        })
    }

    fn finish_inner(&mut self) -> io::Result<()> {
        let Some(cur) = self.current.as_mut() else {
            return Ok(());
        };
        if let Some(gz) = cur.gz.as_mut() {
            self.scratch.clear();
            let mut sink = |b: &[u8]| -> Result<(), Infallible> {
                self.scratch.extend_from_slice(b);
                Ok(())
            };
            if let Err(never) = gz.finish(&mut sink) {
                match never {}
            }
            cur.file.write_all(&self.scratch)?;
            self.total += self.scratch.len() as u64;
        }
        cur.file.flush()?;
        self.unflushed = 0;
        if cur.file.is_syncing() {
            let frames = cur.frames;
            self.sync_started = self.sync_started.saturating_add(1);
            eprintln!(
                "oa-feed: feed={} closed_segment_sync started frames={}",
                self.feed, frames
            );
            if let Err(error) = cur.file.sync_all() {
                return Err(self.fail_sync("file", error, frames));
            }
            eprintln!(
                "oa-feed: feed={} closed_segment_sync started stage=directory frames={}",
                self.feed, frames
            );
            let directory_result = match &mut self.opener {
                RecorderOpener::SyncClosedSegments { directory, .. } => directory(),
                RecorderOpener::FlushOnly(_) => Err(io::Error::other(
                    "syncing writer has no directory sync operation",
                )),
            };
            if let Err(error) = directory_result {
                return Err(self.fail_sync("directory", error, frames));
            }
            self.sync_completed = self.sync_completed.saturating_add(1);
            eprintln!(
                "oa-feed: feed={} closed_segment_sync completed frames={}",
                self.feed, frames
            );
        }
        self.current = None;
        Ok(())
    }

    /// Bytes counted for the cap (compressed for `.oaf.gz`), including files already in the
    /// directory. This count is bytes written, not a persistence confirmation.
    pub fn total_bytes(&self) -> u64 {
        self.total
    }

    /// The first error that failed the recording, or `None` while it is healthy. Once set it
    /// stays set.
    pub fn failure(&self) -> Option<&str> {
        self.failed.as_deref()
    }

    /// Frames handed to the recorder that were not saved because it had failed: the frame that
    /// hit the error, and the frames of the open file that the compressor held and had not yet
    /// flushed to it (a flush or finish that fails loses those too).
    pub fn lost_frames(&self) -> u64 {
        self.lost_frames
    }

    /// Why the recording means the run did not do what it was asked, if it did not: the failure,
    /// or the stop at the size cap (frames after it were not saved). The shell exits non-zero
    /// and prints this when it is `Some`.
    pub fn exit_reason(&self) -> Option<String> {
        if let Some(e) = &self.failed {
            return Some(format!("recording failed: {e}"));
        }
        if self.stopped {
            return Some(format!(
                "recording stopped at the size cap (--max-bytes {}, {} bytes recorded); \
                 later frames were not saved",
                self.max_bytes, self.total
            ));
        }
        None
    }

    /// Files created by this recorder.
    pub fn files_opened(&self) -> u32 {
        self.files_opened
    }

    /// Frames whose receive stamp was more than an hour from local time when written.
    pub fn implausible_stamps(&self) -> u64 {
        self.implausible_stamps
    }

    /// Records `e` as the failure (keeping the first), abandons the open file, passes `e` on. The
    /// frames the compressor still held, not yet flushed to the file, are lost with it.
    fn fail(&mut self, e: io::Error) -> io::Error {
        self.lost_frames = self.lost_frames.saturating_add(self.unflushed);
        if let Some(cur) = self.current.as_ref().filter(|cur| cur.file.is_syncing()) {
            let flushed_frames = cur.frames.saturating_sub(self.unflushed);
            self.persistence_unconfirmed_frames = self
                .persistence_unconfirmed_frames
                .saturating_add(flushed_frames);
        }
        self.unflushed = 0;
        if self.failed.is_none() {
            self.failed = Some(e.to_string());
        }
        self.current = None;
        e
    }

    fn fail_sync(&mut self, stage: &'static str, error: io::Error, frames: u64) -> io::Error {
        self.sync_failures = self.sync_failures.saturating_add(1);
        self.sync_failed_stage = Some(stage);
        self.persistence_unconfirmed_frames =
            self.persistence_unconfirmed_frames.saturating_add(frames);
        self.unflushed = 0;
        let message = format!(
            "closed-segment {stage} sync failed: {error}; persistence is unconfirmed for {frames} frames (not counted as lost)"
        );
        eprintln!("oa-feed: feed={} {message}", self.feed);
        if self.failed.is_none() {
            self.failed = Some(message.clone());
        }
        self.current = None;
        io::Error::new(error.kind(), message)
    }

    fn create(&mut self, now_unix_secs: i64) -> io::Result<Current> {
        let now_stamp = utc_stamp(now_unix_secs).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "local clock is outside the years 1 to 9999; no file name can be made",
            )
        })?;
        let (stamp, mut n) = match &self.last_file_name {
            Some((last_stamp, last_number)) if last_stamp >= &now_stamp => {
                (last_stamp.clone(), (*last_number).saturating_add(1))
            }
            _ => (now_stamp.clone(), 1),
        };
        let ext = match self.format {
            Format::Gzip => "oaf.gz",
            Format::Raw => "oaf",
        };
        loop {
            // Always numbered and zero-padded, so name order is creation order: a second file
            // in the same second must not sort before the first (a replay glob would then play
            // them out of order and show false sequence gaps).
            if n > MAX_SAME_SECOND {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!("more than {MAX_SAME_SECOND} recordings named for one second"),
                ));
            }
            let name = format!("{}-{stamp}-{n:04}.{ext}", self.feed);
            let path = self.dir.join(name);
            let file = match &mut self.opener {
                RecorderOpener::FlushOnly(opener) => opener(&path).map(RecordingFile::FlushOnly),
                RecorderOpener::SyncClosedSegments { file, .. } => {
                    file(&path).map(RecordingFile::Syncing)
                }
            };
            match file {
                Ok(file) => {
                    self.files_opened += 1;
                    self.last_file_name = Some((stamp.clone(), n));
                    if stamp > now_stamp {
                        self.rotations_after_clock_step_back =
                            self.rotations_after_clock_step_back.saturating_add(1);
                    }
                    return Ok(Current {
                        file,
                        disk: 0,
                        gz: match self.format {
                            Format::Gzip => Some(GzipWriter::new(DEFAULT_LEVEL)),
                            Format::Raw => None,
                        },
                        flushed_secs: now_unix_secs,
                        frames: 0,
                    });
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => n += 1,
                Err(e) => return Err(e),
            }
        }
    }
}

#[cfg(test)]
fn existing_recording_bytes(dir: &Path, entry_limit: usize) -> io::Result<u64> {
    existing_recording_state(dir, "", entry_limit).map(|state| state.bytes)
}

struct ExistingRecordingState {
    bytes: u64,
    last_file_name: Option<(String, u32)>,
}

// Keep the limit check before the counter increment as well as all entry processing.
// This also prevents counter overflow when the private helper receives usize::MAX.
#[allow(clippy::explicit_counter_loop)]
fn existing_recording_state(
    dir: &Path,
    feed: &str,
    entry_limit: usize,
) -> io::Result<ExistingRecordingState> {
    let mut total = 0u64;
    let mut last_file_name = None;
    let mut entries = 0usize;
    // The recorder is capture: it checks against the compiled-in list and never fails closed on
    // a changed list file (oa-sealed, `Sealed::production_embedded`).
    oa_sealed::refuse_io_embedded(dir)?;
    // seal-allow: the recorder's own output directory: sizes only, nothing is read
    for entry in fs::read_dir(dir)? {
        if entries >= entry_limit {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "recording directory entry limit exceeded: {found} entries found (counting stopped there) against a limit of {entry_limit}; unrelated files count too; no existing file was touched and nothing was created",
                    found = entries.saturating_add(1)
                ),
            ));
        }
        entries += 1;
        let entry = entry?;
        let path = entry.path();
        if !is_recording(&path) {
            continue;
        }
        let metadata = entry.metadata()?;
        total = total.saturating_add(metadata.len());
        if !metadata.is_file() {
            continue;
        }
        if let Some(candidate) = recording_name_watermark(&path, feed)
            .filter(|candidate| last_file_name.as_ref().is_none_or(|last| candidate > last))
        {
            last_file_name = Some(candidate);
        }
    }
    Ok(ExistingRecordingState {
        bytes: total,
        last_file_name,
    })
}

/// Earliest Unix time [`utc_stamp`] and [`utc_iso`] accept: 0001-01-01T00:00:00Z.
pub const UTC_MIN_SECS: i64 = -62_135_596_800;
/// Latest Unix time they accept: 9999-12-31T23:59:59Z.
pub const UTC_MAX_SECS: i64 = 253_402_300_799;

/// `YYYYMMDDTHHMMSSZ` for a Unix time in seconds (proleptic Gregorian, UTC); `None` outside
/// [`UTC_MIN_SECS`] to [`UTC_MAX_SECS`].
pub fn utc_stamp(unix_secs: i64) -> Option<String> {
    let (y, m, d, hh, mm, ss) = utc_fields(unix_secs)?;
    Some(format!("{y:04}{m:02}{d:02}T{hh:02}{mm:02}{ss:02}Z"))
}

/// `YYYY-MM-DDTHH:MM:SSZ` for a Unix time in seconds; `None` outside the range of [`utc_stamp`].
pub fn utc_iso(unix_secs: i64) -> Option<String> {
    let (y, m, d, hh, mm, ss) = utc_fields(unix_secs)?;
    Some(format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z"))
}

/// Civil date and time of a Unix time (Howard Hinnant's algorithm): year 1 to 9999, month 1 to
/// 12, day 1 to the month's length, hour 0 to 23, minute and second 0 to 59. `None` for a time
/// outside [`UTC_MIN_SECS`] to [`UTC_MAX_SECS`].
fn utc_fields(unix_secs: i64) -> Option<(i64, i64, i64, i64, i64, i64)> {
    if !(UTC_MIN_SECS..=UTC_MAX_SECS).contains(&unix_secs) {
        return None;
    }
    let days = unix_secs.div_euclid(86_400);
    let rem = unix_secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    Some((y, m, d, rem / 3600, rem % 3600 / 60, rem % 60))
}

/// The summary line for a recording: files and bytes, and, when it failed, the failure, the
/// frames lost since and the first error.
pub fn format_recording(feed: &str, r: &Recorder) -> String {
    let mut out = format!(
        "recorded feed={feed} files={} bytes={}",
        r.files_opened(),
        r.total_bytes()
    );
    if r.rotations_after_clock_step_back != 0 {
        out.push_str(&format!(
            " rotations_after_clock_step_back={}",
            r.rotations_after_clock_step_back
        ));
    }
    if matches!(&r.opener, RecorderOpener::SyncClosedSegments { .. }) {
        out.push_str(&format!(
            " sync_closed_segments_started={} completed={} failed={} failed_stage={} persistence_unconfirmed_frames={}",
            r.sync_started,
            r.sync_completed,
            r.sync_failures,
            r.sync_failed_stage.unwrap_or("none"),
            r.persistence_unconfirmed_frames
        ));
    }
    if let Some(e) = r.failure() {
        out.push_str(&format!(
            " FAILED lost_frames={} first_error=\"{e}\"",
            r.lost_frames()
        ));
    }
    out
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod directory_limit_tests {
    use super::{existing_recording_bytes, recording_name_watermark};
    use std::fs;
    use std::io::ErrorKind;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn temp_dir() -> PathBuf {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let serial = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "oa-feed-directory-limit-{}-{serial}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create temporary directory");
        path
    }

    // REQ: OA-FEED-189
    #[test]
    fn directory_entry_limit_counts_unrelated_and_zero_byte_entries() {
        let dir = temp_dir();
        fs::write(dir.join("existing.oaf"), b"abc").expect("write recording");
        fs::write(dir.join("empty.oaf.gz"), b"").expect("write zero-byte recording");
        fs::write(dir.join("notes.txt"), b"unrelated").expect("write unrelated entry");

        let refused = existing_recording_bytes(&dir, 2).expect_err("third entry exceeds limit");
        assert_eq!(refused.kind(), ErrorKind::InvalidData);
        let text = refused.to_string();
        assert!(text.contains("entry limit exceeded"));
        assert!(text.contains("3 entries found"), "{text}");
        assert!(text.contains("limit of 2"), "{text}");
        assert!(text.contains("no existing file was touched"), "{text}");
        assert_eq!(
            existing_recording_bytes(&dir, 3).expect("exact entry allowance"),
            3
        );
        let _ = fs::remove_dir_all(dir);
    }

    // REQ: OA-FEED-019
    #[test]
    fn recording_name_watermark_accepts_raw_and_gzip_and_rejects_noncanonical_names() {
        assert_eq!(
            recording_name_watermark(Path::new("clock-20231115T001320Z-0001.oaf"), "clock"),
            Some(("20231115T001320Z".to_owned(), 1))
        );
        assert_eq!(
            recording_name_watermark(Path::new("clock-20231115T001320Z-9999.oaf.gz"), "clock"),
            Some(("20231115T001320Z".to_owned(), 9_999))
        );
        for name in [
            "clock-20230229T001320Z-0001.oaf",
            "clock-20231115T241320Z-0001.oaf",
            "clock-20231115T001320Z-0000.oaf",
            "clock-20231115T001320Z-10000.oaf.gz",
            "other-20231115T001320Z-0001.oaf",
            "clock-20231115T001320Z-0001.txt",
        ] {
            assert_eq!(
                recording_name_watermark(Path::new(name), "clock"),
                None,
                "accepted noncanonical recording name {name}"
            );
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod closed_segment_sync_tests {
    use super::{
        format_recording, DirectorySyncer, Format, Outcome, Recorder, SyncingOpener, SyncingWriter,
    };
    use std::fs;
    use std::io::{self, Write};
    use std::os::unix::process::ExitStatusExt;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::{Duration, Instant};

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum FailAt {
        None,
        File,
        Directory,
        WriteAfterFirst,
        Flush,
    }

    struct FakeFile {
        events: Arc<Mutex<Vec<&'static str>>>,
        fail_at: FailAt,
        writes: u8,
    }

    impl Write for FakeFile {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.writes = self.writes.saturating_add(1);
            if self.fail_at == FailAt::WriteAfterFirst && self.writes > 1 {
                return Err(io::Error::other("write denied"));
            }
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            if self.fail_at == FailAt::Flush {
                return Err(io::Error::other("flush denied"));
            }
            Ok(())
        }
    }

    impl SyncingWriter for FakeFile {
        fn sync_all(&mut self) -> io::Result<()> {
            self.events.lock().unwrap().push("file");
            if self.fail_at == FailAt::File {
                return Err(io::Error::other("file sync denied"));
            }
            Ok(())
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let serial = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "oa-feed-sync-{name}-{}-{serial}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn recorder(
        dir: &std::path::Path,
        feed: &str,
        format: Format,
        rotate_bytes: u64,
        max_bytes: u64,
        fail_at: FailAt,
        events: Arc<Mutex<Vec<&'static str>>>,
    ) -> Recorder {
        let file_events = Arc::clone(&events);
        let file: SyncingOpener = Box::new(move |_| {
            Ok(Box::new(FakeFile {
                events: Arc::clone(&file_events),
                fail_at,
                writes: 0,
            }) as Box<dyn SyncingWriter>)
        });
        let directory_events = Arc::clone(&events);
        let directory: DirectorySyncer = Box::new(move || {
            directory_events.lock().unwrap().push("directory");
            if fail_at == FailAt::Directory {
                return Err(io::Error::other("directory sync denied"));
            }
            Ok(())
        });
        Recorder::open_sync_closed_segments_with_hooks(
            dir,
            feed,
            rotate_bytes,
            max_bytes,
            format,
            file,
            directory,
        )
        .unwrap()
    }

    // REQ: OA-FEED-170
    #[test]
    fn closed_segments_sync_file_then_directory_on_rotation_and_finish() {
        let dir = temp_dir("order");
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut rec = recorder(
            &dir,
            "order",
            Format::Raw,
            1,
            u64::MAX,
            FailAt::None,
            Arc::clone(&events),
        );
        let frame = b"frame";
        assert!(matches!(
            rec.write_frame(frame, 1_700_000_000, 1_700_000_000),
            Ok(Outcome::Written)
        ));
        assert!(matches!(
            rec.write_frame(frame, 1_700_000_001, 1_700_000_001),
            Ok(Outcome::Written)
        ));
        rec.finish().unwrap();
        assert_eq!(
            *events.lock().unwrap(),
            vec!["file", "directory", "file", "directory"]
        );
        assert_eq!(
            (rec.sync_started, rec.sync_completed, rec.sync_failures),
            (2, 2, 0)
        );
        assert!(format_recording("order", &rec)
            .contains("sync_closed_segments_started=2 completed=2 failed=0"));
        let _ = fs::remove_dir_all(dir);
    }

    // REQ: OA-FEED-170
    #[test]
    fn size_cap_closes_and_syncs_the_active_segment() {
        let dir = temp_dir("cap");
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut rec = recorder(
            &dir,
            "cap",
            Format::Raw,
            u64::MAX,
            5,
            FailAt::None,
            Arc::clone(&events),
        );
        assert!(matches!(
            rec.write_frame(b"frame", 1_700_000_000, 1_700_000_000),
            Ok(Outcome::Written)
        ));
        assert!(matches!(
            rec.write_frame(b"next", 1_700_000_001, 1_700_000_001),
            Ok(Outcome::StoppedNow)
        ));
        assert_eq!(*events.lock().unwrap(), vec!["file", "directory"]);
        assert_eq!((rec.sync_started, rec.sync_completed), (1, 1));
        let _ = fs::remove_dir_all(dir);
    }

    // REQ: OA-FEED-170
    #[test]
    fn file_and_directory_sync_failures_are_distinct_from_lost_frames() {
        for (name, fail_at, expected_events, expected_stage) in [
            ("file-failure", FailAt::File, vec!["file"], "file"),
            (
                "directory-failure",
                FailAt::Directory,
                vec!["file", "directory"],
                "directory",
            ),
        ] {
            let dir = temp_dir(name);
            let events = Arc::new(Mutex::new(Vec::new()));
            let mut rec = recorder(
                &dir,
                name,
                Format::Gzip,
                u64::MAX,
                u64::MAX,
                fail_at,
                Arc::clone(&events),
            );
            rec.write_frame(b"frame", 1_700_000_000, 1_700_000_000)
                .unwrap();
            let error = rec.finish().unwrap_err().to_string();
            assert!(error.contains(&format!("{expected_stage} sync failed")));
            assert_eq!(*events.lock().unwrap(), expected_events);
            assert_eq!(
                (rec.sync_started, rec.sync_completed, rec.sync_failures),
                (1, 0, 1)
            );
            assert_eq!(rec.sync_failed_stage, Some(expected_stage));
            assert_eq!(rec.persistence_unconfirmed_frames, 1);
            assert_eq!(rec.lost_frames(), 0);
            assert!(rec
                .exit_reason()
                .unwrap()
                .contains("persistence is unconfirmed"));
            let line = format_recording(name, &rec);
            assert!(line.contains("persistence_unconfirmed_frames=1"));
            assert!(line.contains("lost_frames=0"));
            let _ = fs::remove_dir_all(dir);
        }
    }

    // REQ: OA-FEED-170
    #[test]
    fn write_and_flush_failures_report_accepted_persistence_separately_from_lost_frames() {
        for (name, fail_at, finish, expected_lost) in [
            ("write-failure", FailAt::WriteAfterFirst, false, 1),
            ("flush-failure", FailAt::Flush, true, 0),
        ] {
            let dir = temp_dir(name);
            let events = Arc::new(Mutex::new(Vec::new()));
            let mut rec = recorder(
                &dir,
                name,
                Format::Raw,
                u64::MAX,
                u64::MAX,
                fail_at,
                Arc::clone(&events),
            );
            rec.write_frame(b"accepted", 1_700_000_000, 1_700_000_000)
                .unwrap();
            if finish {
                assert!(rec.finish().is_err());
            } else {
                assert!(rec
                    .write_frame(b"triggering", 1_700_000_001, 1_700_000_001)
                    .is_err());
            }
            assert_eq!(rec.persistence_unconfirmed_frames, 1);
            assert_eq!(rec.lost_frames(), expected_lost);
            assert!(format_recording(name, &rec).contains("persistence_unconfirmed_frames=1"));
            let _ = fs::remove_dir_all(dir);
        }
    }

    fn one_raw_frame() -> Vec<u8> {
        let payload = b"synthetic process-restart fixture";
        let mut frame =
            vec![0u8; crate::frame::HEADER_LEN + payload.len() + crate::frame::TRAILER_LEN];
        let n = crate::frame::encode_frame(
            &mut frame,
            1,
            crate::frame::FLAG_BYTES,
            1,
            1_700_000_000_000_000_000,
            1_700_000_000_000,
            payload,
        )
        .unwrap();
        frame.truncate(n);
        frame
    }

    const FILE_SYNC_RETURNED: &[u8] = b"file sync returned; directory sync not entered";

    fn pause_after_file_sync(dir: &Path) {
        use std::fs::OpenOptions;

        let checkpoint = dir.join("before-directory-sync.checkpoint");
        let completed = dir.join("directory-sync.completed");
        let file: SyncingOpener = Box::new(|path| {
            let file = OpenOptions::new().write(true).create_new(true).open(path)?;
            Ok(Box::new(super::SyncingFile(file)) as Box<dyn SyncingWriter>)
        });
        let directory: DirectorySyncer = Box::new(move || {
            fs::write(&checkpoint, FILE_SYNC_RETURNED)?;
            // Allow time for the parent to kill us. If it disappears, the child still exits.
            thread::sleep(Duration::from_secs(15));
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "parent did not interrupt the directory-sync boundary",
            ))
        });
        let mut recorder = Recorder::open_sync_closed_segments_with_hooks(
            dir,
            "restart",
            u64::MAX,
            u64::MAX,
            Format::Raw,
            file,
            directory,
        )
        .unwrap();
        recorder
            .write_frame(&one_raw_frame(), 1_700_000_000, 1_700_000_000)
            .unwrap();
        recorder.finish().unwrap();
        fs::write(completed, b"directory sync returned").unwrap();
    }

    // REQ: OA-FEED-170
    // Process-restart fixture only: visible files and successful sync calls do not certify
    // power-loss durability.
    #[test]
    fn process_restart_after_file_sync_replays_segment_and_respects_name_and_cap() {
        const CHILD_ENV: &str = "OA_FEED_TEST_SYNC_RESTART_DIR";
        if let Some(dir) = std::env::var_os(CHILD_ENV) {
            pause_after_file_sync(Path::new(&dir));
            return;
        }

        let dir = temp_dir("process-restart");
        let checkpoint = dir.join("before-directory-sync.checkpoint");
        let completed = dir.join("directory-sync.completed");
        let mut child = Command::new(std::env::current_exe().unwrap())
            .arg("process_restart_after_file_sync_replays_segment_and_respects_name_and_cap")
            .arg("--nocapture")
            .env(CHILD_ENV, &dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut checkpoint_seen = false;
        while Instant::now() < deadline {
            if fs::read(&checkpoint).is_ok_and(|bytes| bytes == FILE_SYNC_RETURNED) {
                checkpoint_seen = true;
                break;
            }
            if child.try_wait().unwrap().is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let _ = child.kill();
        let status = child.wait().unwrap();
        assert_eq!(
            status.signal(),
            Some(9),
            "child must be killed at the boundary"
        );
        assert!(
            checkpoint_seen,
            "child did not reach the post-file-sync hook"
        );
        assert_eq!(fs::read(&checkpoint).unwrap(), FILE_SYNC_RETURNED);
        assert!(
            !completed.exists(),
            "directory sync must not report completion after process kill"
        );

        let first = fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path.extension().is_some_and(|ext| ext == "oaf"))
            .unwrap();
        assert_eq!(fs::read(&first).unwrap(), one_raw_frame());
        let report = crate::replay::replay_sources(vec![fs::File::open(&first).unwrap()]).unwrap();
        assert_eq!(report.health.frames, 1);
        assert!(report.errors.is_empty());
        assert!(report.gzip.is_empty());

        let occupied_bytes = fs::metadata(&first).unwrap().len();
        let mut capped = Recorder::open_sync_closed_segments(
            &dir,
            "restart",
            u64::MAX,
            occupied_bytes,
            Format::Raw,
        )
        .unwrap();
        assert_eq!(capped.total_bytes(), occupied_bytes);
        assert_eq!(
            capped
                .write_frame(&one_raw_frame(), 1_700_000_000, 1_700_000_000)
                .unwrap(),
            Outcome::StoppedNow
        );
        assert_eq!(capped.files_opened(), 0);

        let mut continued =
            Recorder::open_sync_closed_segments(&dir, "restart", u64::MAX, u64::MAX, Format::Raw)
                .unwrap();
        assert_eq!(
            continued
                .write_frame(&one_raw_frame(), 1_700_000_000, 1_700_000_000)
                .unwrap(),
            Outcome::Written
        );
        continued.finish().unwrap();
        let mut names: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".oaf"))
            .collect();
        names.sort();
        let [first, second] = names.as_slice() else {
            panic!("expected the previous and restarted segments, got {names:?}");
        };
        assert!(first.ends_with("-0001.oaf"), "{names:?}");
        assert!(second.ends_with("-0002.oaf"), "{names:?}");
        let _ = fs::remove_dir_all(dir);
    }
}
