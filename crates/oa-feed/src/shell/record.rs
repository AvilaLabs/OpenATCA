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
//! compressor is sync-flushed when the local whole second changes after the last
//! flush, and by [`Recorder::idle_flush`] when the feed is quiet, so a crash loses at most about
//! one second of frames; each file is finished (final block and CRC-32 trailer) on rotation, at
//! the cap and in [`Recorder::finish`]. Rotation and the cap count the compressed bytes that are
//! on disk, not the frame bytes: the compressor holds a little back, so a file can pass the
//! rotation size, and the total the cap, by the output of one flush interval before the next
//! check sees it. A file cut by a crash is read back up to its last flush (oa-zip
//! `GzipReader`).
//!
//! A write, flush or finish that fails (a full disk) is a persistent failed state, not a
//! warning: the recorder keeps the first error, stops writing, and counts every frame handed to
//! it afterwards as lost ([`Recorder::failure`], [`Recorder::lost_frames`], [`format_recording`]),
//! so the shell can report it and exit non-zero. A stop at the size cap exits non-zero too
//! ([`Recorder::exit_reason`]): the run did not record what it was asked to.
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

/// The open file: bytes on disk so far, the compressor, and when it was last flushed.
// A7.2: cold struct, size guard waived
struct Current {
    file: Box<dyn Write + Send>,
    disk: u64,
    gz: Option<GzipWriter>,
    flushed_secs: i64,
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
    opener: Opener,
    /// The first error that failed the recording; once set, nothing more is written.
    failed: Option<String>,
    lost_frames: u64,
    /// Frames given to the compressor since its last successful flush: on disk only once the next
    /// flush or finish succeeds, and lost if that one fails.
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
    let prefix = format!("{feed}-");
    let body = stem.strip_prefix(prefix.as_str())?;
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
            opener,
            failed: None,
            lost_frames: 0,
            unflushed: 0,
        })
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

    /// Ends the open file: the compressed member gets its final block and trailer, and the file
    /// is synced to the operating system. Call at exit; rotation and the cap do it themselves.
    pub fn finish(&mut self) -> io::Result<()> {
        if self.failed.is_some() {
            // Failed earlier: the open file is abandoned as it stands (it replays up to its last
            // flush) and the failure is already recorded.
            self.current = None;
            return Ok(());
        }
        self.finish_inner().map_err(|e| self.fail(e))
    }

    fn finish_inner(&mut self) -> io::Result<()> {
        let Some(mut cur) = self.current.take() else {
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
        Ok(())
    }

    /// Bytes recorded in the directory so far (on disk, so compressed for `.oaf.gz`), including
    /// files that were already there.
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
        self.unflushed = 0;
        if self.failed.is_none() {
            self.failed = Some(e.to_string());
        }
        self.current = None;
        e
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
            match (self.opener)(&self.dir.join(name)) {
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
                    });
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => n += 1,
                Err(e) => return Err(e),
            }
        }
    }
}

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
        if is_recording(&entry.path()) {
            total = total.saturating_add(entry.metadata()?.len());
            if let Some(candidate) = recording_name_watermark(&entry.path(), feed) {
                if last_file_name.is_none_or(|last| candidate > last) {
                    last_file_name = Some(candidate);
                }
            }
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
