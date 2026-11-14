//! Streaming a day archive: tar entries, gzip, trace decode. Shell (it reads streams).

use std::fmt;
use std::io::Read;

use oa_tar::{Next, TarError, TarReader, Truncated};
use oa_zip::{GzipReader, InflateLimits};

use crate::{
    decode_trace, reports_clear, stats_add, Filter, Reports, Stats, MAX_GZIP_RATIO, MAX_TRACE_BYTES,
};

/// How many refusal messages a scan keeps (the counts in [`Stats`] are complete).
const MAX_ERROR_SAMPLES: usize = 16;

/// How the archive ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanEnd {
    /// The end-of-archive marker was reached.
    Complete,
    /// The parts ended early (only the first part of a split archive, a cut download); every
    /// entry before the cut was processed.
    Truncated(Truncated),
}

/// What a scan did. Plain record (A1).
#[derive(Debug, Clone, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct ScanResult {
    pub end: ScanEnd,
    pub stats: Stats,
    /// Entries that were trace files.
    pub trace_entries: u64,
    /// Entries that were not (directories, other files); skipped unread.
    pub other_entries: u64,
    /// The first refusals, as `name: reason`, at most 16.
    pub error_samples: Vec<String>,
}

/// Why a scan stopped without a result.
#[derive(Debug)]
pub enum ScanError<E> {
    /// The tar layer refused the archive (bad header, limit, read failure).
    Tar(TarError),
    /// The caller's sink failed.
    Sink(E),
}

impl<E: fmt::Display> fmt::Display for ScanError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScanError::Tar(e) => write!(f, "{e}"),
            ScanError::Sink(e) => write!(f, "output failed: {e}"),
        }
    }
}

impl<E: fmt::Debug + fmt::Display> std::error::Error for ScanError<E> {}

fn is_trace_name(name: &str) -> bool {
    let base = name.rsplit('/').next().unwrap_or(name);
    base.starts_with("trace_full_") && base.ends_with(".json")
}

/// Plain JSON, or gzip (magic 1f 8b) inflated under the size and ratio limits.
fn unpack(raw: &[u8]) -> Result<Vec<u8>, String> {
    if raw.get(..2) != Some(&[0x1f, 0x8b]) {
        return Ok(raw.to_vec());
    }
    let limits = InflateLimits {
        max_output: MAX_TRACE_BYTES as u64,
        max_ratio: MAX_GZIP_RATIO,
        ratio_exempt_bytes: 1024 * 1024,
    };
    let mut gz = GzipReader::new(limits);
    let mut out: Vec<u8> = Vec::new();
    gz.push(raw, &mut |b: &[u8]| -> Result<(), String> {
        out.extend_from_slice(b);
        Ok(())
    })
    .map_err(|e| e.to_string())?;
    let sum = gz.finish();
    if sum.truncated.is_some() {
        return Err("gzip: file is cut short".to_string());
    }
    Ok(out)
}

/// Reads the day archive made of `parts` (one reader, or the split pieces in order). For each
/// trace file the reports inside `filter` are decoded into a buffer and handed to `sink` (one
/// call per trace with at least one report; the buffer is reused, so copy what you keep). A
/// bad trace is counted and skipped; the scan goes on (E2).
pub fn scan_archive<R: Read, E>(
    parts: Vec<R>,
    filter: &Filter,
    sink: &mut dyn FnMut(&Reports) -> Result<(), E>,
) -> Result<ScanResult, ScanError<E>> {
    let mut tar = TarReader::new(parts, oa_tar::DEFAULT_LIMITS);
    let mut stats = Stats::default();
    let mut res = ScanResult {
        end: ScanEnd::Complete,
        stats: Stats::default(),
        trace_entries: 0,
        other_entries: 0,
        error_samples: Vec::new(),
    };
    let mut buf = Reports::default();
    let mut raw: Vec<u8> = Vec::new();
    loop {
        let h = match tar.next_entry().map_err(ScanError::Tar)? {
            Next::Entry(h) => h,
            Next::End => break,
            Next::Truncated(t) => {
                res.end = ScanEnd::Truncated(t);
                break;
            }
        };
        if h.typeflag != b'0' || !is_trace_name(&h.name) {
            res.other_entries += 1;
            continue;
        }
        res.trace_entries += 1;
        if h.size > MAX_TRACE_BYTES as u64 {
            stats.traces_refused += 1;
            note(&mut res, &h.name, "entry larger than the trace size limit");
            continue;
        }
        raw.clear();
        if let Some(t) = tar.read_entry_to_vec(&mut raw).map_err(ScanError::Tar)? {
            res.end = ScanEnd::Truncated(t);
            res.trace_entries -= 1; // the cut entry was not read
            break;
        }
        let json = match unpack(&raw) {
            Ok(j) => j,
            Err(why) => {
                stats.traces_refused += 1;
                note(&mut res, &h.name, &why);
                continue;
            }
        };
        reports_clear(&mut buf);
        let mut one = Stats::default();
        match decode_trace(&json, filter, &mut buf, &mut one) {
            Ok(()) => {
                stats_add(&mut stats, &one);
                stats.traces_read += 1;
                if !buf.icao.is_empty() {
                    sink(&buf).map_err(ScanError::Sink)?;
                }
            }
            Err(e) => {
                stats.traces_refused += 1;
                note(&mut res, &h.name, &e.to_string());
            }
        }
    }
    res.stats = stats;
    Ok(res)
}

fn note(res: &mut ScanResult, name: &str, why: &str) {
    if res.error_samples.len() < MAX_ERROR_SAMPLES {
        res.error_samples.push(format!("{name}: {why}"));
    }
}
