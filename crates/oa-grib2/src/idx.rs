//! The `.idx` inventory that NOAA publishes beside each GRIB2 file (wgrib2 `-s` style), used to
//! find the byte range of one field for an HTTP range request. Core (pure).
//!
//! A line is `msg:offset:d=YYYYMMDDHH:VAR:level:forecast:` with optional further fields, for
//! example `12:345678:d=2026111418:UGRD:250 mb:anl:`. A message that holds several fields
//! (RAP puts u and v in one) is listed once per field as `12.1`, `12.2` with the same offset;
//! a byte range always covers the whole message, so fetching one such field fetches its
//! companions too. Message numbers and byte offsets must increase from message to message. A
//! message runs from its offset to the next message's offset minus one; the last has no stated
//! end.

/// Limit: most lines in one index. A full HRRR pressure file has about 700 messages.
pub const MAX_IDX_LINES: usize = 100_000;
/// Limit: longest line in bytes.
pub const MAX_IDX_LINE_BYTES: usize = 1024;

use crate::error::Grib2Error;

/// One inventory, one row per message, struct of arrays.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct Idx {
    pub msg: Vec<u32>,
    /// Field number inside the message (`12.2` is 2), 0 when the line has none.
    pub sub: Vec<u32>,
    pub offset: Vec<u64>,
    /// Reference time as `YYYYMMDDHH`.
    pub date: Vec<u64>,
    pub var: Vec<String>,
    pub level: Vec<String>,
    pub forecast: Vec<String>,
}

/// Number of rows.
pub fn idx_len(idx: &Idx) -> usize {
    idx.msg.len()
}

/// Parses an index. Blank lines are skipped; any other malformed line refuses the whole index.
pub fn parse_idx(text: &str) -> Result<Idx, Grib2Error> {
    let mut idx = Idx::default();
    for (n, line) in text.lines().enumerate() {
        let line_no = n + 1;
        let bad = |why| Grib2Error::BadIdx { line: line_no, why };
        let line = line.trim_end_matches('\r');
        if line.trim().is_empty() {
            continue;
        }
        if line.len() > MAX_IDX_LINE_BYTES {
            return Err(bad("line too long"));
        }
        if idx.msg.len() >= MAX_IDX_LINES {
            return Err(bad("too many lines"));
        }
        let mut it = line.split(':');
        let (msg, sub): (u32, u32) = it
            .next()
            .and_then(|s| match s.split_once('.') {
                Some((m, f)) => Some((m.parse().ok()?, f.parse().ok()?)),
                None => Some((s.parse().ok()?, 0)),
            })
            .ok_or_else(|| bad("message number"))?;
        let offset: u64 = it
            .next()
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| bad("byte offset"))?;
        let d = it
            .next()
            .and_then(|s| s.strip_prefix("d="))
            .ok_or_else(|| bad("missing d="))?;
        if d.len() != 10 || !d.bytes().all(|b| b.is_ascii_digit()) {
            return Err(bad("date is not YYYYMMDDHH"));
        }
        let date: u64 = d.parse().map_err(|_| bad("date"))?;
        let var = it
            .next()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| bad("missing variable"))?;
        let level = it.next().ok_or_else(|| bad("missing level"))?;
        let forecast = it.next().ok_or_else(|| bad("missing forecast"))?;
        if let (Some(&pm), Some(&ps), Some(&po)) =
            (idx.msg.last(), idx.sub.last(), idx.offset.last())
        {
            let same = msg == pm && sub > ps && offset == po;
            let next = msg > pm && offset > po;
            if !same && !next {
                return Err(bad("message number or offset does not increase"));
            }
        }
        idx.msg.push(msg);
        idx.sub.push(sub);
        idx.offset.push(offset);
        idx.date.push(date);
        idx.var.push(var.to_string());
        idx.level.push(level.to_string());
        idx.forecast.push(forecast.to_string());
    }
    Ok(idx)
}

/// Rows whose variable and level match exactly (`level` as written in the index, for example
/// `"250 mb"` or `"2 m above ground"`), in file order.
pub fn find_rows(idx: &Idx, var: &str, level: &str) -> Vec<usize> {
    (0..idx_len(idx))
        .filter(|&k| {
            idx.var.get(k).is_some_and(|v| v == var) && idx.level.get(k).is_some_and(|l| l == level)
        })
        .collect()
}

/// Inclusive byte range `(first, last)` of row `k`; `last` is `None` for the final message
/// unless `file_len` is given.
pub fn byte_range(idx: &Idx, k: usize, file_len: Option<u64>) -> Option<(u64, Option<u64>)> {
    let first = *idx.offset.get(k)?;
    // The next row at a larger offset starts the next message (rows of one message share theirs).
    let last = match idx.offset.iter().skip(k + 1).find(|&&o| o > first) {
        Some(&next) => Some(next - 1),
        None => file_len.and_then(|n| n.checked_sub(1)),
    };
    Some((first, last))
}

/// The label of an isobaric level as the index writes it.
pub fn isobaric_label(hpa: u32) -> String {
    format!("{hpa} mb")
}
