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
    canon_row_floats, digest_end_line, field_at, limit_text, row_second, rows_header, second_line,
    CanonRow, Digester, DIGEST_HEADER, FIELDS, MAX_ROWS_PER_SECOND, OFF_T_NS, ROW_BYTES,
};
use crate::spill::{check_columns, ColReader, SpilledRun, WIDTHS};

/// Rows held in memory at once when a rows file has to be written out of emission order.
pub const CHUNK_ROWS: u64 = 1 << 22;
// A chunk takes whole seconds; the largest second must fit in one.
const _: () = assert!(MAX_ROWS_PER_SECOND <= CHUNK_ROWS);

/// Calls `f` with every row of a spilled run, canonical (every NaN among the floats is the one
/// canonical NaN), in emission order.
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
            // A base that spilled raw bits may hold any NaN: the canonical row has the one NaN.
            canon_row_floats(&mut row);
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
///
/// Memory: the digest holds one entry per second (at most [`crate::canon::MAX_SECONDS`]); a rows
/// file written out of second order holds at most [`CHUNK_ROWS`] rows (about 356 MB of canonical
/// rows) at a time, and one in order holds none.
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

/// Writes a strict half-open timestamp slice of a spilled run in the ordinary canonical format.
/// Unlike [`write_canon`], only rows with `from_ns <= t_ns < to_ns` enter either the digest or
/// rows file. The tracker input/run itself is not clipped by this function.
pub fn write_canon_windowed(
    run: &SpilledRun,
    digest: Option<&Path>,
    rows: Option<&Path>,
    from_ns: i64,
    to_ns: i64,
) -> io::Result<CanonSummary> {
    if from_ns >= to_ns {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "empty or reversed canonical window",
        ));
    }
    let selected = |row: &CanonRow| {
        let t = i64::from_le_bytes(field_at::<OFF_T_NS, 8>(row));
        from_ns <= t && t < to_ns
    };
    let mut d = Digester::new();
    let mut in_order = true;
    let mut last = i64::MIN;
    let mut row_count = 0u64;
    spill_canon_rows(run, &mut |r| {
        if selected(r) {
            let second = row_second(r);
            in_order &= second >= last;
            last = last.max(second);
            d.push(r).map_err(|e| io::Error::other(limit_text(e)))?;
            row_count = row_count
                .checked_add(1)
                .ok_or_else(|| io::Error::other("canonical window row count overflow"))?;
        }
        Ok(())
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
        w.get_ref().sync_all()?;
    }
    if let Some(p) = rows {
        let mut w = BufWriter::with_capacity(1 << 20, File::create(p)?);
        w.write_all(&rows_header(row_count))?;
        if in_order {
            spill_canon_rows(run, &mut |r| {
                if selected(r) {
                    w.write_all(r)?;
                }
                Ok(())
            })?;
        } else {
            let mut at = 0usize;
            while let Some(first) = entries.get(at) {
                let (lo, mut n) = (first.second, 0u64);
                let mut hi = lo;
                while let Some(e) = entries.get(at) {
                    if n > 0 && n.saturating_add(e.rows) > CHUNK_ROWS {
                        break;
                    }
                    n = n.saturating_add(e.rows);
                    hi = e.second;
                    at += 1;
                }
                let mut held: Vec<(i64, CanonRow)> = Vec::new();
                spill_canon_rows(run, &mut |r| {
                    if selected(r) {
                        let second = row_second(r);
                        if (lo..=hi).contains(&second) {
                            held.push((second, *r));
                        }
                    }
                    Ok(())
                })?;
                held.sort_by_key(|(second, _)| *second);
                for (_, row) in &held {
                    w.write_all(row)?;
                }
            }
        }
        w.flush()?;
        w.get_ref().sync_all()?;
    }
    Ok(CanonSummary {
        seconds: u64::try_from(entries.len()).unwrap_or(u64::MAX),
        rows: row_count,
        in_order,
    })
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod public_window_canon_tests {
    use super::*;
    use crate::canon::{canon_row, rows_header, CanonRow, ROWS_HEADER_BYTES, ROW_BYTES};
    use crate::spill::{spill_append, spill_finish, spill_new};
    use crate::{log_new, output_new, OutRow, TrackOutput, TrackerStats, O_TICK};
    use std::fs;
    use std::path::PathBuf;

    fn synthetic_row(t_ns: i64, address: u32, value: f32) -> OutRow {
        OutRow {
            t_ns,
            addr: address,
            lat_e7: 420_000_000,
            lon_e7: -880_000_000,
            vel_east_mps: value,
            vel_north_mps: value + 1.0,
            pos_cov_m2: [20.0, 0.0, 20.0],
            vel_cov_m2s2: [0.0; 3],
            turn_rate_rps: 0.0,
            alt_ft: 30_000.0,
            alt_var_ft2: 25.0,
            vrate_fpm: 0.0,
            mu_h: [0.9, 0.1],
            mu_v: [0.9, 0.1],
            flags: O_TICK,
        }
    }

    fn push_output(out: &mut TrackOutput, row: &OutRow) {
        out.t_ns.push(row.t_ns);
        out.addr.push(row.addr);
        out.lat_e7.push(row.lat_e7);
        out.lon_e7.push(row.lon_e7);
        out.vel_east_mps.push(row.vel_east_mps);
        out.vel_north_mps.push(row.vel_north_mps);
        out.pos_cov_m2.push(row.pos_cov_m2);
        out.vel_cov_m2s2.push(row.vel_cov_m2s2);
        out.turn_rate_rps.push(row.turn_rate_rps);
        out.alt_ft.push(row.alt_ft);
        out.alt_var_ft2.push(row.alt_var_ft2);
        out.vrate_fpm.push(row.vrate_fpm);
        out.mu_h.push(row.mu_h);
        out.mu_v.push(row.mu_v);
        out.flags.push(row.flags);
    }

    fn scratch(name: &str) -> PathBuf {
        let base = std::env::var_os("CARGO_TARGET_TMPDIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let path = base.join(format!("canon_public_window_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn synthetic_run(dir: &std::path::Path, rows: &[OutRow]) -> SpilledRun {
        let mut output = output_new(rows.len().max(1));
        for row in rows {
            push_output(&mut output, row);
        }
        let mut writer = spill_new(&dir.join("spill")).unwrap();
        spill_append(&mut writer, &output).unwrap();
        spill_finish(
            writer,
            0,
            TrackerStats::default(),
            log_new(4),
            rows.len() as u64,
        )
        .unwrap()
    }

    fn row_bytes(path: &std::path::Path) -> Vec<CanonRow> {
        let bytes = fs::read(path).unwrap();
        assert!(bytes.len() >= ROWS_HEADER_BYTES);
        let mut rows = Vec::new();
        for chunk in bytes
            .get(ROWS_HEADER_BYTES..)
            .unwrap()
            .chunks_exact(ROW_BYTES)
        {
            let mut row = [0u8; ROW_BYTES];
            row.copy_from_slice(chunk);
            rows.push(row);
        }
        rows
    }

    // REQ: OA-TRK-409
    #[test]
    fn window_writer_is_half_open_preserves_repeated_rows_and_matches_full_run_subset() {
        let dir = scratch("subset");
        let t0 = 100_000_000_000;
        let repeated = synthetic_row(t0 + 1_000_000_000, 77, 3.0);
        let source_rows = [
            synthetic_row(t0 + 2_000_000_000, 77, 4.0),
            synthetic_row(t0, 77, 2.0),
            repeated,
            repeated,
            synthetic_row(t0 + 3_000_000_000, 77, 5.0),
        ];
        let run = synthetic_run(&dir, &source_rows);
        let full_digest = dir.join("full.digest");
        let full_rows = dir.join("full.rows");
        let window_digest = dir.join("window.digest");
        let window_rows = dir.join("window.rows");
        let full = write_canon(&run, Some(&full_digest), Some(&full_rows)).unwrap();
        let window = write_canon_windowed(
            &run,
            Some(&window_digest),
            Some(&window_rows),
            t0,
            t0 + 3_000_000_000,
        )
        .unwrap();
        assert_eq!((full.rows, window.rows), (5, 4));
        assert_eq!(
            fs::read(&window_rows).unwrap().get(..ROWS_HEADER_BYTES),
            Some(&rows_header(4)[..])
        );
        let expected: Vec<CanonRow> = row_bytes(&full_rows)
            .into_iter()
            .filter(|row| {
                let t = i64::from_le_bytes(field_at::<OFF_T_NS, 8>(row));
                t0 <= t && t < t0 + 3_000_000_000
            })
            .collect();
        assert_eq!(row_bytes(&window_rows), expected);
        assert_eq!(
            expected
                .iter()
                .filter(|row| **row == canon_row(&repeated))
                .count(),
            2
        );
        assert_eq!(window.seconds, 3);
        assert!(!window.in_order);
        assert!(fs::read_to_string(&window_digest)
            .unwrap()
            .contains("end 3 4 "));
        drop(run);
        let _ = fs::remove_dir_all(&dir);
    }

    // REQ: OA-TRK-409
    #[test]
    fn full_supported_window_preserves_existing_canonical_bytes_and_empty_is_refused() {
        let dir = scratch("compat");
        let input = [
            synthetic_row(10_000_000_000, 8, 1.0),
            synthetic_row(12_000_000_000, 8, 2.0),
        ];
        let run = synthetic_run(&dir, &input);
        let old_digest = dir.join("legacy.digest");
        let old_rows = dir.join("legacy.rows");
        let all_digest = dir.join("all.digest");
        let all_rows = dir.join("all.rows");
        let legacy = write_canon(&run, Some(&old_digest), Some(&old_rows)).unwrap();
        let all =
            write_canon_windowed(&run, Some(&all_digest), Some(&all_rows), i64::MIN, i64::MAX)
                .unwrap();
        assert_eq!(legacy, all);
        assert_eq!(
            fs::read(&old_digest).unwrap(),
            fs::read(&all_digest).unwrap()
        );
        assert_eq!(fs::read(&old_rows).unwrap(), fs::read(&all_rows).unwrap());
        assert!(write_canon_windowed(&run, None, None, 7, 7).is_err());
        assert!(write_canon_windowed(&run, None, None, 8, 7).is_err());
        drop(run);
        let _ = fs::remove_dir_all(&dir);
    }
}
