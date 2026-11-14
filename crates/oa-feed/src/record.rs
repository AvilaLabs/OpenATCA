//! Frame recording: raw frames appended verbatim to size-rotated `.oaf` files. Shell (file
//! system); the clock is passed in as a value so naming is testable.
//!
//! Files are named `<feed>-<UTC start>.oaf` (UTC as `YYYYMMDDTHHMMSSZ`, with `-2`, `-3` if a name
//! is taken). A new file starts when the next frame would push the current one past the rotation
//! size. The total size of every `.oaf` file in the directory is capped; at the cap recording
//! stops, once, with a warning, and nothing is deleted (a recording is evidence, A8).

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// What [`write_frame`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The frame was appended.
    Written,
    /// The cap was reached by this frame: recording has just stopped (print the warning).
    StoppedNow,
    /// Recording had already stopped; the frame was not written.
    Stopped,
}

/// Recording state for one feed. A subsystem under D5: it owns the open file.
pub struct Recorder {
    dir: PathBuf,
    feed: String,
    rotate_bytes: u64,
    max_bytes: u64,
    total: u64,
    current: Option<(File, u64)>,
    stopped: bool,
    files_opened: u32,
}

impl Recorder {
    /// Prepares `dir` (created if missing) and counts the bytes of existing `.oaf` files toward
    /// the cap.
    pub fn open(dir: &Path, feed: &str, rotate_bytes: u64, max_bytes: u64) -> io::Result<Recorder> {
        fs::create_dir_all(dir)?;
        let mut total = 0u64;
        for e in fs::read_dir(dir)? {
            let e = e?;
            if e.path().extension().is_some_and(|x| x == "oaf") {
                total = total.saturating_add(e.metadata()?.len());
            }
        }
        Ok(Recorder {
            dir: dir.to_path_buf(),
            feed: feed.to_string(),
            rotate_bytes: rotate_bytes.max(1),
            max_bytes,
            total,
            current: None,
            stopped: false,
            files_opened: 0,
        })
    }

    /// Appends one whole frame, rotating first if needed. `now_unix_secs` names a new file.
    pub fn write_frame(&mut self, raw: &[u8], now_unix_secs: i64) -> io::Result<Outcome> {
        if self.stopped {
            return Ok(Outcome::Stopped);
        }
        let len = raw.len() as u64;
        if self.total.saturating_add(len) > self.max_bytes {
            self.stopped = true;
            return Ok(Outcome::StoppedNow);
        }
        let rotate = match &self.current {
            Some((_, n)) => *n > 0 && n.saturating_add(len) > self.rotate_bytes,
            None => true,
        };
        if rotate {
            self.current = Some((self.create(now_unix_secs)?, 0));
        }
        if let Some((f, n)) = self.current.as_mut() {
            f.write_all(raw)?;
            *n += len;
            self.total += len;
        }
        Ok(Outcome::Written)
    }

    /// Bytes recorded in the directory so far, including files that were already there.
    pub fn total_bytes(&self) -> u64 {
        self.total
    }

    /// Files created by this recorder.
    pub fn files_opened(&self) -> u32 {
        self.files_opened
    }

    fn create(&mut self, now_unix_secs: i64) -> io::Result<File> {
        let stamp = utc_stamp(now_unix_secs);
        let mut n = 1u32;
        loop {
            let name = if n == 1 {
                format!("{}-{stamp}.oaf", self.feed)
            } else {
                format!("{}-{stamp}-{n}.oaf", self.feed)
            };
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(self.dir.join(name))
            {
                Ok(f) => {
                    self.files_opened += 1;
                    return Ok(f);
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => n += 1,
                Err(e) => return Err(e),
            }
        }
    }
}

/// `YYYYMMDDTHHMMSSZ` for a Unix time in seconds (proleptic Gregorian, UTC).
pub fn utc_stamp(unix_secs: i64) -> String {
    let (y, m, d, hh, mm, ss) = utc_fields(unix_secs);
    format!("{y:04}{m:02}{d:02}T{hh:02}{mm:02}{ss:02}Z")
}

/// `YYYY-MM-DDTHH:MM:SSZ` for a Unix time in seconds.
pub fn utc_iso(unix_secs: i64) -> String {
    let (y, m, d, hh, mm, ss) = utc_fields(unix_secs);
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

/// Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
fn utc_fields(unix_secs: i64) -> (i64, i64, i64, i64, i64, i64) {
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
    (y, m, d, rem / 3600, rem % 3600 / 60, rem % 60)
}
