//! Hour files on disk and a bounded cache of hours. Shell (B7): owns the files; the model and the
//! query are in [`crate::hour`] and [`crate::sample`].
//!
//! The NOAA files hold many messages of which an hour needs four parameters on isobaric levels.
//! When the `.idx` beside a file matches it, only the byte ranges of those messages are read
//! from disk; when it does not (the local copies are already subsets whose `.idx` is that of
//! the original file) the file is read whole, bounded by [`MAX_FILE_BYTES`], and its messages
//! are filtered by what they contain. Either way the same hour results: the `.idx` is checked
//! against the file (its offsets are the file's message starts, every message's own header says
//! what the index says, and the decoded ranges are the messages the index claimed), and any
//! disagreement is counted and the whole file read instead.

use crate::error::{LoadError, QueryError};
use crate::hour::{
    head_claim, hour_bytes, index_claims, load_hour, load_hour_claimed, needed_ranges, Hour,
};
use crate::isa::pressure_hpa_from_altitude_ft;
use crate::sample::{bracket, sample_blend, Query, Sample};
use crate::{MAX_ARCHIVE_FILES, MAX_FILE_BYTES, MAX_HOURS_HELD, MAX_IDX_BYTES};
use oa_grib2::{
    forecast_seconds, inspect_stream, message_length, parse_idx, peek_info, ref_unix_seconds, Idx,
};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// A file of one analysis hour and its valid time.
// A7.2: cold struct, size guard waived
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HourFile {
    pub path: PathBuf,
    /// GRIB forecast reference/issue time, Unix seconds.
    pub issue_s: i64,
    /// GRIB forecast lead, seconds.
    pub lead_s: i64,
    /// GRIB valid time (`issue_s + lead_s`), Unix seconds.
    pub valid_s: i64,
}

/// Why issue-cutoff selection could not return one complete forecast cycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CausalRefusal {
    TooManyFiles,
    InvalidMetadata,
    NoCycleBeforeCutoff,
    ReceiptBeforeIssue,
    ReceiptUnknown,
    ReceiptAfterCutoff,
    ConflictingCycle,
    NoValidBracket,
    ValidGapTooLarge,
}

/// One verified reference-time cycle selected using separately supplied receipt evidence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CycleSelection {
    pub issue_s: i64,
    pub first_valid_s: i64,
    pub last_valid_s: i64,
    /// Owned metadata rows selected from one issue cycle.
    pub files: Vec<HourFile>,
}

/// Selects the latest verified forecast cycle available by `cutoff_s` that brackets the complete
/// requested valid-time interval. `receipt_s[i]` is separate evidence supplied by the caller for
/// `files[i]`; `None` is unknown receipt time, never inferred from a file path, reference time or
/// file metadata. This selector does not load grid data. Existing [`archive_sample`] remains a
/// valid-time archive query and does not apply this causal rule. The returned rows are metadata;
/// before loading, the caller must bind each supplied receipt to the identical immutable file
/// bytes. The archive rechecks issue and lead metadata, but does not hash file contents.
/// Every returned hour must have receipt evidence, including hours between the two brackets.
/// The caller supplies a positive maximum gap for interpolation; exact-time requests can select
/// one exact hour and therefore do not require an adjacent hour.
pub fn select_cycle_as_of(
    files: &[HourFile],
    receipt_s: &[Option<i64>],
    cutoff_s: i64,
    valid_from_s: i64,
    valid_to_s: i64,
    max_gap_s: i64,
) -> Result<CycleSelection, CausalRefusal> {
    if files.len() > MAX_ARCHIVE_FILES {
        return Err(CausalRefusal::TooManyFiles);
    }
    if receipt_s.len() != files.len()
        || cutoff_s <= 0
        || valid_from_s <= 0
        || valid_to_s < valid_from_s
        || max_gap_s <= 0
    {
        return Err(CausalRefusal::InvalidMetadata);
    }
    let mut cycles = BTreeMap::<i64, Vec<usize>>::new();
    for (i, file) in files.iter().enumerate() {
        if file.issue_s <= 0
            || file.lead_s < 0
            || file.issue_s.checked_add(file.lead_s) != Some(file.valid_s)
            || file.valid_s <= 0
            || receipt_s.get(i).is_some_and(|r| r.is_some_and(|v| v <= 0))
        {
            return Err(CausalRefusal::InvalidMetadata);
        }
        if receipt_s
            .get(i)
            .and_then(|r| *r)
            .is_some_and(|received| received < file.issue_s)
        {
            return Err(CausalRefusal::ReceiptBeforeIssue);
        }
        if file.issue_s <= cutoff_s {
            cycles.entry(file.issue_s).or_default().push(i);
        }
    }
    if cycles.is_empty() {
        return Err(CausalRefusal::NoCycleBeforeCutoff);
    }

    let mut saw_unknown_receipt = false;
    let mut saw_late_receipt = false;
    let mut saw_conflict = false;
    let mut saw_large_gap = false;
    for (issue_s, mut indices) in cycles.into_iter().rev() {
        indices.sort_unstable_by_key(|&i| files.get(i).map_or(i64::MAX, |f| f.valid_s));
        if indices.windows(2).any(|w| {
            matches!(w, [a, b] if files.get(*a).map(|f| f.valid_s) == files.get(*b).map(|f| f.valid_s))
        }) {
            saw_conflict = true;
            continue;
        }
        let lower = indices
            .iter()
            .copied()
            .filter(|&i| files.get(i).is_some_and(|f| f.valid_s <= valid_from_s))
            .max_by_key(|&i| files.get(i).map_or(i64::MIN, |f| f.valid_s));
        let upper = if valid_from_s == valid_to_s
            && indices
                .iter()
                .any(|&i| files.get(i).is_some_and(|f| f.valid_s == valid_from_s))
        {
            indices
                .iter()
                .copied()
                .find(|&i| files.get(i).is_some_and(|f| f.valid_s == valid_from_s))
        } else {
            indices
                .iter()
                .copied()
                .filter(|&i| files.get(i).is_some_and(|f| f.valid_s >= valid_to_s))
                .min_by_key(|&i| files.get(i).map_or(i64::MAX, |f| f.valid_s))
        };
        let (Some(lower), Some(upper)) = (lower, upper) else {
            continue;
        };
        let eligible = |i: usize| {
            receipt_s
                .get(i)
                .and_then(|r| *r)
                .is_some_and(|received| received <= cutoff_s)
        };
        let lower_pos = indices.iter().position(|&i| i == lower).unwrap_or(0);
        let upper_pos = indices
            .iter()
            .position(|&i| i == upper)
            .unwrap_or(lower_pos);
        let selected = indices.get(lower_pos..=upper_pos).unwrap_or(&[]);
        if let Some(unavailable) = selected.iter().copied().find(|&i| !eligible(i)) {
            match receipt_s.get(unavailable).copied().flatten() {
                None => saw_unknown_receipt = true,
                Some(received) if received > cutoff_s => saw_late_receipt = true,
                Some(_) => {}
            }
            continue;
        }
        let selected = selected.to_vec();
        if selected.windows(2).any(|pair| match pair {
            [a, b] => files
                .get(*b)
                .and_then(|fb| {
                    files
                        .get(*a)
                        .and_then(|fa| fb.valid_s.checked_sub(fa.valid_s))
                })
                .map_or(true, |gap| gap > max_gap_s),
            _ => false,
        }) {
            saw_large_gap = true;
            continue;
        }
        let selected_files = selected
            .iter()
            .filter_map(|&i| files.get(i).cloned())
            .collect::<Vec<_>>();
        let first_valid_s = files.get(lower).map_or(valid_from_s, |file| file.valid_s);
        let last_valid_s = files.get(upper).map_or(valid_to_s, |file| file.valid_s);
        return Ok(CycleSelection {
            issue_s,
            first_valid_s,
            last_valid_s,
            files: selected_files,
        });
    }
    if saw_late_receipt {
        Err(CausalRefusal::ReceiptAfterCutoff)
    } else if saw_unknown_receipt {
        Err(CausalRefusal::ReceiptUnknown)
    } else if saw_conflict {
        Err(CausalRefusal::ConflictingCycle)
    } else if saw_large_gap {
        Err(CausalRefusal::ValidGapTooLarge)
    } else {
        Err(CausalRefusal::NoValidBracket)
    }
}

/// Hour files sorted by valid time and the hours currently held (least recently used first).
// A7.2: cold struct, size guard waived
#[derive(Debug)]
pub struct Archive {
    files: Vec<HourFile>,
    held: Vec<(usize, Hour)>,
    index_fallbacks: u64,
}

fn io_err(path: &Path, e: &std::io::Error) -> LoadError {
    LoadError::Io {
        path: path.display().to_string(),
        kind: e.kind(),
    }
}

/// Valid time of a file from its first message only (a few tens of kilobytes are read).
fn peek_cycle_time(path: &Path) -> Result<(i64, i64, i64), LoadError> {
    let mut f = oa_sealed::open(path).map_err(|e| io_err(path, &e))?;
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
    let issue_s = ref_unix_seconds(&first.ref_time);
    let valid_s = issue_s.checked_add(lead).ok_or(LoadError::BadTime)?;
    Ok((issue_s, lead, valid_s))
}

/// Reads the first message of each file to learn its valid time. Refuses an unreadable file.
pub fn index_files(paths: &[PathBuf]) -> Result<Vec<HourFile>, LoadError> {
    if paths.len() > MAX_ARCHIVE_FILES {
        return Err(LoadError::TooManyFiles(paths.len()));
    }
    paths
        .iter()
        .map(|p| {
            let (issue_s, lead_s, valid_s) = peek_cycle_time(p)?;
            Ok(HourFile {
                path: p.clone(),
                issue_s,
                lead_s,
                valid_s,
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
        index_fallbacks: 0,
    })
}

/// An archive with no files; every query is refused with `NoHours`.
pub fn new_archive() -> Archive {
    Archive {
        files: Vec::new(),
        held: Vec::new(),
        index_fallbacks: 0,
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

    /// Hours loaded from a whole file because their `.idx` disagreed with the file.
    pub fn index_fallbacks(&self) -> u64 {
        self.index_fallbacks
    }

    /// The indexed files, sorted by valid time.
    pub fn files(&self) -> &[HourFile] {
        &self.files
    }
}

/// Reads a whole file of at most `limit` bytes: its length is checked first, and the read
/// itself stops one byte past the limit, so a file that grows meanwhile is refused too.
pub fn read_file_capped(path: &Path, limit: u64) -> std::io::Result<Vec<u8>> {
    let f = oa_sealed::open(path)?;
    if f.metadata()?.len() > limit {
        return Err(std::io::ErrorKind::InvalidInput.into());
    }
    let mut out = Vec::new();
    f.take(limit.saturating_add(1)).read_to_end(&mut out)?;
    if out.len() as u64 > limit {
        return Err(std::io::ErrorKind::InvalidInput.into());
    }
    Ok(out)
}

fn read_ranges(path: &Path, ranges: &[(u64, u64)]) -> std::io::Result<Vec<u8>> {
    let mut f = oa_sealed::open(path)?;
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

/// How an hour's `.idx` sidecar was used.
// A7.2: cold struct, size guard waived
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexUse {
    /// No readable `.idx` beside the file, or it names no needed message.
    Absent,
    /// The hour was read from the index's byte ranges.
    Used,
    /// The index disagreed with the file (the reason is named); the whole file was read.
    Refused(LoadError),
}

/// Bytes read from the start of each message to learn what its first field is.
const HEAD_BYTES: usize = 2048;

/// Walks the framing of the file at `path`, 16 bytes a message, and returns each message's
/// start and length. Refuses a file that is not whole messages back to back or has more than
/// [`oa_grib2::idx::MAX_IDX_LINES`].
fn message_starts(f: &mut File, file_len: u64) -> Result<Vec<(u64, u64)>, LoadError> {
    let bad = LoadError::IndexMismatch("file framing");
    let mut out = Vec::new();
    let mut pos = 0u64;
    while pos < file_len {
        if out.len() >= oa_grib2::idx::MAX_IDX_LINES {
            return Err(bad);
        }
        let mut head = [0u8; 16];
        f.seek(SeekFrom::Start(pos)).map_err(|_| bad.clone())?;
        f.read_exact(&mut head).map_err(|_| bad.clone())?;
        let mut a = [0u8; 8];
        a.copy_from_slice(head.get(8..16).ok_or_else(|| bad.clone())?);
        let len = u64::from_be_bytes(a);
        if head.get(0..4) != Some(b"GRIB".as_slice())
            || !(41..=oa_grib2::MAX_MESSAGE_BYTES).contains(&len)
            || pos + len > file_len
        {
            return Err(bad);
        }
        out.push((pos, len));
        pos += len;
    }
    Ok(out)
}

/// Checks an index against the file itself, reading only the first bytes of each message: the
/// index's message offsets are exactly the file's message starts, and for every message the
/// variable and level of its first field agree with the index's first row for it whenever
/// either one is a field an hour wants.
fn check_index_against_file(path: &Path, idx: &Idx, file_len: u64) -> Result<(), LoadError> {
    let mut f = oa_sealed::open(path).map_err(|e| io_err(path, &e))?;
    let starts = message_starts(&mut f, file_len)?;
    let mut offsets: Vec<u64> = idx.offset.clone();
    offsets.dedup();
    if offsets.len() != starts.len() || offsets.iter().zip(&starts).any(|(o, (s, _))| o != s) {
        return Err(LoadError::IndexMismatch("message offsets"));
    }
    for &(start, len) in &starts {
        let n = len.min(HEAD_BYTES as u64) as usize;
        let mut head = vec![0u8; n];
        f.seek(SeekFrom::Start(start))
            .map_err(|e| io_err(path, &e))?;
        f.read_exact(&mut head).map_err(|e| io_err(path, &e))?;
        let info = peek_info(&head).map_err(LoadError::Decode)?;
        // The first index row of this message.
        let row = idx
            .offset
            .iter()
            .position(|&o| o == start)
            .ok_or(LoadError::IndexMismatch("message offsets"))?;
        let one = Idx {
            msg: vec![0],
            sub: vec![0],
            offset: vec![start],
            date: idx.date.get(row).copied().into_iter().collect(),
            var: idx.var.get(row).cloned().into_iter().collect(),
            level: idx.level.get(row).cloned().into_iter().collect(),
            forecast: idx.forecast.get(row).cloned().into_iter().collect(),
        };
        let said = index_claims(&one).into_iter().next();
        let is = head_claim(&info);
        if said.is_some() != is.is_some() {
            return Err(LoadError::IndexMismatch("message variable or level"));
        }
        if let (Some(a), Some(b)) = (said, is) {
            if a.var != b.var || a.level_pa != b.level_pa || a.date != b.date {
                return Err(LoadError::IndexMismatch("message variable, level or date"));
            }
        }
    }
    Ok(())
}

/// Loads one hour from a file, using the `.idx` beside it when it matches the file and the
/// whole file otherwise. A stale `.idx` is detected, never trusted: it must agree with the
/// file's messages (see the module comment), and the ranges must read and decode into the
/// messages it claimed, else the file is read whole.
pub fn read_hour_file(path: &Path) -> Result<Hour, LoadError> {
    read_hour_file_report(path).map(|(h, _)| h)
}

/// [`read_hour_file`] that also says how the `.idx` was used, so a caller can count refusals.
pub fn read_hour_file_report(path: &Path) -> Result<(Hour, IndexUse), LoadError> {
    // The guard first: the file is opened through it, and its size is read from the opened file.
    let file = oa_sealed::open(path).map_err(|e| io_err(path, &e))?;
    let meta = file.metadata().map_err(|e| io_err(path, &e))?;
    drop(file);
    if meta.len() > MAX_FILE_BYTES {
        return Err(LoadError::Io {
            path: path.display().to_string(),
            kind: std::io::ErrorKind::InvalidInput,
        });
    }
    let mut idx_path = path.as_os_str().to_owned();
    idx_path.push(".idx");
    let idx = read_file_capped(Path::new(&idx_path), MAX_IDX_BYTES)
        .ok()
        .and_then(|b| String::from_utf8(b).ok());
    let mut used = IndexUse::Absent;
    if let Some(idx) = idx.as_deref().and_then(|t| parse_idx(t).ok()) {
        match try_index(path, &idx, meta.len()) {
            Ok(Some(h)) => return Ok((h, IndexUse::Used)),
            Ok(None) => {}
            Err(e) => used = IndexUse::Refused(e),
        }
    }
    let data = read_file_capped(path, MAX_FILE_BYTES).map_err(|e| io_err(path, &e))?;
    Ok((load_hour(&data)?, used))
}

/// The indexed read: `Ok(None)` when the index names nothing to read, an error when it
/// disagrees with the file.
fn try_index(path: &Path, idx: &Idx, file_len: u64) -> Result<Option<Hour>, LoadError> {
    let ranges = needed_ranges(idx, file_len)?;
    let Some(last) = ranges.last() else {
        return Ok(None);
    };
    if last.1 >= file_len {
        return Err(LoadError::IndexMismatch("ranges run past the file"));
    }
    check_index_against_file(path, idx, file_len)?;
    let data = read_ranges(path, &ranges).map_err(|e| io_err(path, &e))?;
    load_hour_claimed(&data, &index_claims(idx)).map(Some)
}

type Loader<'a> = &'a mut dyn FnMut(&Path) -> Result<Hour, LoadError>;

fn ensure_loaded(a: &mut Archive, k: usize, load: Loader<'_>) -> Result<(), LoadError> {
    if let Some(pos) = a.held.iter().position(|(i, _)| *i == k) {
        let entry = a.held.remove(pos);
        a.held.push(entry);
        return Ok(());
    }
    let file = a.files.get(k).ok_or(LoadError::BadTime)?;
    let hour = load(&file.path)?;
    if hour.valid_s != file.valid_s {
        return Err(LoadError::Inconsistent("valid time differs from its index"));
    }
    if hour.issue_s != file.issue_s || hour.lead_s != file.lead_s {
        return Err(LoadError::Inconsistent(
            "forecast cycle differs from its index",
        ));
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

/// The valid-time query over an archive: loads the (at most two) bracketing hours as needed,
/// evicting the least recently used beyond [`MAX_HOURS_HELD`]. This preserves the archive's
/// valid-time behavior and does not enforce issue-cutoff or receipt-time availability. Callers
/// needing a causal query can first use [`select_cycle_as_of`] with explicit receipt evidence.
pub fn archive_sample(a: &mut Archive, q: &Query) -> Result<Sample, QueryError> {
    let mut refused = 0u64;
    let mut load = |p: &Path| {
        let (h, used) = read_hour_file_report(p)?;
        if matches!(used, IndexUse::Refused(_)) {
            refused += 1;
        }
        Ok(h)
    };
    let r = archive_sample_with(a, q, &mut load);
    a.index_fallbacks += refused;
    r
}

/// [`archive_sample`] with the file reader supplied (the tests give it synthetic hours).
pub(crate) fn archive_sample_with(
    a: &mut Archive,
    q: &Query,
    load: Loader<'_>,
) -> Result<Sample, QueryError> {
    let p = pressure_hpa_from_altitude_ft(q.alt_ft)?;
    let (i0, i1, w) = bracket(&a.times(), q.time_s)?;
    // Only an hour with weight is read: at a stated hour the neighbour may be unreadable.
    if w != 1.0 {
        ensure_loaded(a, i0, &mut *load)?;
    }
    if w != 0.0 {
        ensure_loaded(a, i1, &mut *load)?;
    }
    let (Some(f0), Some(f1)) = (a.files.get(i0), a.files.get(i1)) else {
        return Err(QueryError::Load(LoadError::BadTime));
    };
    let (t0, t1) = (f0.valid_s, f1.valid_s);
    let (Some(h0), Some(h1)) = (
        held_hour(a, i0).or(held_hour(a, i1)),
        held_hour(a, i1).or(held_hour(a, i0)),
    ) else {
        return Err(QueryError::Load(LoadError::BadTime));
    };
    let pt = sample_blend(h0, h1, w, q.lat, q.lon, p)?;
    Ok(Sample {
        u_ms: pt.u,
        v_ms: pt.v,
        temp_k: pt.t,
        height_gpm: pt.z,
        pressure_hpa: p,
        hour0_s: t0,
        hour1_s: t1,
        time_weight: w,
    })
}
