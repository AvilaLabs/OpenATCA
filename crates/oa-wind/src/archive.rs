//! Hour files on disk and a bounded cache of hours. Shell: owns the files; the model and the
//! query are in [`crate::hour`] and [`crate::sample`].
//!
//! The NOAA files hold many messages of which an hour needs four parameters on isobaric levels.
//! When the `.idx` beside a file matches it, only the byte ranges of those messages are read
//! from disk; when it does not (the local copies are already subsets whose `.idx` is that of
//! the original file) the file is read whole, bounded by [`MAX_FILE_BYTES`], and its messages
//! are filtered by what they contain. Either way the same hour results.

use crate::error::{LoadError, QueryError};
use crate::hour::{hour_bytes, load_hour, needed_ranges_text, Hour};
use crate::isa::pressure_hpa_from_altitude_ft;
use crate::sample::{bracket, sample_blend, Query, Sample};
use crate::{MAX_ARCHIVE_FILES, MAX_FILE_BYTES, MAX_HOURS_HELD};
use oa_grib2::{forecast_seconds, inspect_stream, message_length, ref_unix_seconds};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// A file of one analysis hour and its valid time.
// A7.2: cold struct, size guard waived
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HourFile {
    pub path: PathBuf,
    pub valid_s: i64,
}

/// Hour files sorted by valid time and the hours currently held (least recently used first).
// A7.2: cold struct, size guard waived
#[derive(Debug)]
pub struct Archive {
    files: Vec<HourFile>,
    held: Vec<(usize, Hour)>,
}

fn io_err(path: &Path, e: &std::io::Error) -> LoadError {
    LoadError::Io {
        path: path.display().to_string(),
        kind: e.kind(),
    }
}

/// Valid time of a file from its first message only (a few tens of kilobytes are read).
fn peek_valid_time(path: &Path) -> Result<i64, LoadError> {
    let mut f = File::open(path).map_err(|e| io_err(path, &e))?;
    let mut head = [0u8; 16];
    f.read_exact(&mut head).map_err(|e| io_err(path, &e))?;
    // message_length needs the whole message, so take the length from the head and read on.
    let len = match head.get(8..16) {
        Some(b) => {
            let mut a = [0u8; 8];
            a.copy_from_slice(b);
            u64::from_be_bytes(a)
        }
        None => return Err(LoadError::BadTime),
    };
    if !(41..=oa_grib2::MAX_MESSAGE_BYTES).contains(&len) {
        return Err(LoadError::Decode(oa_grib2::Grib2Error::BadMessageLength(
            len,
        )));
    }
    let mut msg = head.to_vec();
    msg.resize(len as usize, 0);
    f.read_exact(msg.get_mut(16..).unwrap_or(&mut []))
        .map_err(|e| io_err(path, &e))?;
    message_length(&msg, 0)?;
    let info = inspect_stream(&msg);
    let first = info.fields.first().ok_or(LoadError::BadTime)?;
    let lead = forecast_seconds(first.time_unit, first.forecast_time).ok_or(LoadError::BadTime)?;
    ref_unix_seconds(&first.ref_time)
        .checked_add(lead)
        .ok_or(LoadError::BadTime)
}

/// Reads the first message of each file to learn its valid time. Refuses an unreadable file.
pub fn index_files(paths: &[PathBuf]) -> Result<Vec<HourFile>, LoadError> {
    if paths.len() > MAX_ARCHIVE_FILES {
        return Err(LoadError::TooManyFiles(paths.len()));
    }
    paths
        .iter()
        .map(|p| {
            Ok(HourFile {
                path: p.clone(),
                valid_s: peek_valid_time(p)?,
            })
        })
        .collect()
}

/// An archive over indexed files: sorts them by valid time and refuses two with the same time.
pub fn archive_files(mut files: Vec<HourFile>) -> Result<Archive, LoadError> {
    if files.len() > MAX_ARCHIVE_FILES {
        return Err(LoadError::TooManyFiles(files.len()));
    }
    files.sort_by_key(|f| f.valid_s);
    if let Some(w) = files
        .windows(2)
        .find(|w| matches!(w, [a, b] if a.valid_s == b.valid_s))
    {
        return Err(LoadError::DuplicateHour(w.first().map_or(0, |f| f.valid_s)));
    }
    Ok(Archive {
        files,
        held: Vec::new(),
    })
}

/// An archive with no files; every query is refused with `NoHours`.
pub fn new_archive() -> Archive {
    Archive {
        files: Vec::new(),
        held: Vec::new(),
    }
}

impl Archive {
    /// Valid times of the files, ascending.
    pub fn times(&self) -> Vec<i64> {
        self.files.iter().map(|f| f.valid_s).collect()
    }

    /// Hours currently held (never more than [`MAX_HOURS_HELD`]).
    pub fn held(&self) -> usize {
        self.held.len()
    }

    /// Bytes of arrays currently held.
    pub fn held_bytes(&self) -> u64 {
        self.held.iter().map(|(_, h)| hour_bytes(h)).sum()
    }

    /// The indexed files, sorted by valid time.
    pub fn files(&self) -> &[HourFile] {
        &self.files
    }
}

fn read_ranges(path: &Path, ranges: &[(u64, u64)]) -> std::io::Result<Vec<u8>> {
    let mut f = File::open(path)?;
    let mut out = Vec::new();
    for &(a, b) in ranges {
        let n = b - a + 1;
        if n > oa_grib2::MAX_MESSAGE_BYTES * 64 || out.len() as u64 + n > MAX_FILE_BYTES {
            return Err(std::io::ErrorKind::InvalidInput.into());
        }
        f.seek(SeekFrom::Start(a))?;
        let start = out.len();
        out.resize(start + n as usize, 0);
        f.read_exact(out.get_mut(start..).unwrap_or(&mut []))?;
    }
    Ok(out)
}

/// Loads one hour from a file, using the `.idx` beside it when it matches the file and the
/// whole file otherwise. A stale `.idx` is detected, never trusted: the ranges must read
/// and decode into a complete hour, else the file is read whole.
pub fn read_hour_file(path: &Path) -> Result<Hour, LoadError> {
    let meta = std::fs::metadata(path).map_err(|e| io_err(path, &e))?;
    if meta.len() > MAX_FILE_BYTES {
        return Err(LoadError::Io {
            path: path.display().to_string(),
            kind: std::io::ErrorKind::InvalidInput,
        });
    }
    let mut idx_path = path.as_os_str().to_owned();
    idx_path.push(".idx");
    if let Ok(text) = std::fs::read_to_string(&idx_path) {
        if let Ok(ranges) = needed_ranges_text(&text, meta.len()) {
            let fits = ranges.last().is_some_and(|r| r.1 < meta.len());
            if fits {
                if let Ok(data) = read_ranges(path, &ranges) {
                    if let Ok(h) = load_hour(&data) {
                        return Ok(h);
                    }
                }
            }
        }
    }
    let data = std::fs::read(path).map_err(|e| io_err(path, &e))?;
    load_hour(&data)
}

fn ensure_loaded(a: &mut Archive, k: usize) -> Result<(), LoadError> {
    if let Some(pos) = a.held.iter().position(|(i, _)| *i == k) {
        let entry = a.held.remove(pos);
        a.held.push(entry);
        return Ok(());
    }
    let file = a.files.get(k).ok_or(LoadError::BadTime)?;
    let hour = read_hour_file(&file.path)?;
    if hour.valid_s != file.valid_s {
        return Err(LoadError::Inconsistent("valid time differs from its index"));
    }
    while a.held.len() >= MAX_HOURS_HELD {
        a.held.remove(0);
    }
    a.held.push((k, hour));
    Ok(())
}

fn held_hour(a: &Archive, k: usize) -> Option<&Hour> {
    a.held.iter().find(|(i, _)| *i == k).map(|(_, h)| h)
}

/// The query over an archive: loads the (at most two) bracketing hours as needed, evicting the
/// least recently used beyond [`MAX_HOURS_HELD`].
pub fn archive_sample(a: &mut Archive, q: &Query) -> Result<Sample, QueryError> {
    let p = pressure_hpa_from_altitude_ft(q.alt_ft)?;
    let (i0, i1, w) = bracket(&a.times(), q.time_s)?;
    ensure_loaded(a, i0)?;
    ensure_loaded(a, i1)?;
    let (Some(h0), Some(h1)) = (held_hour(a, i0), held_hour(a, i1)) else {
        return Err(QueryError::Load(LoadError::BadTime));
    };
    let pt = sample_blend(h0, h1, w, q.lat, q.lon, p)?;
    Ok(Sample {
        u_ms: pt.u,
        v_ms: pt.v,
        temp_k: pt.t,
        height_gpm: pt.z,
        pressure_hpa: p,
        hour0_s: h0.valid_s,
        hour1_s: h1.valid_s,
        time_weight: w,
    })
}
