//! Reads a spilled tracker run back as canonical rows and writes the digest file and the rows
//! file ([`crate::canon`] is the pure side). Shell (B1, B3): every item here reads or writes
//! files.
//!
//! | item | class |
//! |---|---|
//! | [`spill_canon_rows`] | shell: reads the column files of a [`SpilledRun`] |
//! | [`write_canon`] | shell: reads the run, writes the digest and rows files |
//! | [`CanonSummary`] | plain data |
//!
//! # Limits (C2, C7)
//!
//! [`write_canon`] feeds a [`Digester`], which refuses a run with more than
//! [`crate::canon::MAX_SECONDS`] seconds or a second with more than
//! [`crate::canon::MAX_ROWS_PER_SECOND`] rows; that is an error, not a second held whole. When
//! the rows come out of second order, the file is written by passes over the run, each holding at
//! most [`CHUNK_ROWS`] rows, which is above the largest allowed second, so a chunk always makes
//! progress.

use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::Path;

use crate::canon::{
    digest_end_line, limit_text, row_second, rows_header, second_line, CanonRow, Digester,
    DIGEST_HEADER, FIELDS, MAX_ROWS_PER_SECOND, ROW_BYTES,
};
use crate::spill::{check_columns, ColReader, SpilledRun, WIDTHS};

/// Rows held in memory at once when a rows file has to be written out of emission order.
pub const CHUNK_ROWS: u64 = 1 << 22;
// A chunk takes whole seconds; the largest second must fit in one.
const _: () = assert!(MAX_ROWS_PER_SECOND <= CHUNK_ROWS);

/// Calls `f` with every row of a spilled run, canonical, in emission order.
pub fn spill_canon_rows(
    run: &SpilledRun,
    f: &mut dyn FnMut(&CanonRow) -> io::Result<()>,
) -> io::Result<()> {
    check_columns(&run.dir, run.rows)?;
    let mut cols = Vec::with_capacity(FIELDS.len());
    for (c, _) in FIELDS {
        let w = WIDTHS
            .get(c)
            .and_then(|w| usize::try_from(*w).ok())
            .ok_or_else(|| io::Error::other("a canonical field has no spill column"))?;
        cols.push((ColReader::open(&run.dir, c, w)?, w));
    }
    let mut seen = 0u64;
    loop {
        let mut blocks: Vec<Vec<u8>> = Vec::with_capacity(cols.len());
        for (r, _) in &mut cols {
            blocks.push(r.block()?.to_vec());
        }
        // The first column is the time, eight bytes a row.
        let t_width = cols.first().map_or(8, |(_, w)| *w);
        let n = blocks.first().map_or(0, |b| b.len() / t_width.max(1));
        if n == 0 {
            break;
        }
        for i in 0..n {
            let mut row = [0u8; ROW_BYTES];
            for (k, ((_, w), (_, off))) in cols.iter().zip(FIELDS.iter()).enumerate() {
                let src = blocks.get(k).and_then(|b| b.get(i * w..(i + 1) * w));
                let (Some(src), Some(dst)) = (src, row.get_mut(*off..off + w)) else {
                    return Err(io::Error::other(
                        "spill column files have different lengths, or a field lies outside the row",
                    ));
                };
                dst.copy_from_slice(src);
            }
            f(&row)?;
            seen = seen
                .checked_add(1)
                .ok_or_else(|| io::Error::other("spill row count overflow"))?;
        }
    }
    if seen != run.rows {
        return Err(io::Error::other("spill read back a different row count"));
    }
    Ok(())
}

/// What [`write_canon`] wrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct CanonSummary {
    pub seconds: u64,
    pub rows: u64,
    /// Whether the rows came out of the tracker with non-decreasing seconds.
    pub in_order: bool,
}

/// Writes the digest file and/or the rows file of a spilled run. The rows file lists the rows
/// grouped by second in ascending order, each second's rows in emission order.
pub fn write_canon(
    run: &SpilledRun,
    digest: Option<&Path>,
    rows: Option<&Path>,
) -> io::Result<CanonSummary> {
    let mut d = Digester::new();
    let mut in_order = true;
    let mut last = i64::MIN;
    spill_canon_rows(run, &mut |r| {
        let s = row_second(r);
        in_order &= s >= last;
        last = last.max(s);
        d.push(r).map_err(|e| io::Error::other(limit_text(e)))
    })?;
    let entries = d.finish();
    if let Some(p) = digest {
        let mut w = BufWriter::new(File::create(p)?);
        writeln!(w, "{DIGEST_HEADER}")?;
        for e in &entries {
            writeln!(w, "{}", second_line(e))?;
        }
        writeln!(w, "{}", digest_end_line(&entries))?;
        w.flush()?;
    }
    if let Some(p) = rows {
        let mut w = BufWriter::with_capacity(1 << 20, File::create(p)?);
        w.write_all(&rows_header(run.rows))?;
        if in_order {
            spill_canon_rows(run, &mut |r| w.write_all(r))?;
        } else {
            // Out of order: pass over the run once per chunk of seconds, so memory stays bounded.
            let mut at = 0usize;
            while let Some(first) = entries.get(at) {
                let (lo, mut n) = (first.second, 0u64);
                let mut hi = lo;
                while let Some(e) = entries.get(at) {
                    // Every second is at most MAX_ROWS_PER_SECOND (the digester refused more),
                    // so a chunk of CHUNK_ROWS takes at least one whole second.
                    if n > 0 && n.saturating_add(e.rows) > CHUNK_ROWS {
                        break;
                    }
                    n = n.saturating_add(e.rows);
                    hi = e.second;
                    at += 1;
                }
                let mut held: Vec<(i64, CanonRow)> = Vec::new();
                spill_canon_rows(run, &mut |r| {
                    let s = row_second(r);
                    if (lo..=hi).contains(&s) {
                        held.push((s, *r));
                    }
                    Ok(())
                })?;
                held.sort_by_key(|(s, _)| *s);
                for (_, r) in &held {
                    w.write_all(r)?;
                }
            }
        }
        w.flush()?;
    }
    Ok(CanonSummary {
        seconds: u64::try_from(entries.len()).unwrap_or(u64::MAX),
        rows: run.rows,
        in_order,
    })
}
