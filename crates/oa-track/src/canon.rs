//! The canonical bytes of a tracker output row, the per-second digest over them, and the rows
//! file that explains a difference between two runs. Core: [`canon_row`], [`Digester`] and
//! [`chain_next`] are pure. Shell: [`write_canon`] reads a spilled run back and writes the files.
//!
//! A row is [`ROW_BYTES`] bytes, every field little-endian at a fixed offset, floats as their
//! IEEE bits (a NaN keeps its payload and sign; nothing is zeroed or normalised). Rows are
//! grouped by the Unix second of their time (`t_ns` floored to a second). The digest of a second
//! is the SHA-256 of its rows in the order the tracker emitted them; the chain hash of a second
//! is the SHA-256 of the chain hash of the second before it (32 zero bytes for the first), the
//! second as `i64` little-endian, the second's digest and its row count as `u64` little-endian.
//! No floating-point value is decoded here: the rows are moved and hashed as bytes.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::Path;

use oa_sha256::{sha256, to_hex, Sha256};

use crate::spill::{check_columns, ColReader, SpilledRun, WIDTHS};
use crate::types::{OutRow, TrackOutput};

/// Bytes of one canonical row.
pub const ROW_BYTES: usize = 85;
/// Field layout of a canonical row: byte offsets and widths.
pub const OFF_ADDR: usize = 0;
pub const OFF_T_NS: usize = 4;
pub const OFF_LAT: usize = 12;
pub const OFF_LON: usize = 16;
pub const OFF_VE: usize = 20;
pub const OFF_VN: usize = 24;
pub const OFF_TURN: usize = 28;
pub const OFF_ALT: usize = 32;
pub const OFF_ALT_VAR: usize = 36;
pub const OFF_VRATE: usize = 40;
/// Position covariance `[ee, en, nn]`, three `f32`.
pub const OFF_POS_COV: usize = 44;
/// Velocity covariance `[ee, en, nn]`, three `f32`.
pub const OFF_VEL_COV: usize = 56;
/// Horizontal model probabilities, two `f32`.
pub const OFF_MU_H: usize = 68;
/// Vertical model probabilities, two `f32`.
pub const OFF_MU_V: usize = 76;
pub const OFF_FLAGS: usize = 84;

/// One canonical row.
pub type CanonRow = [u8; ROW_BYTES];

/// First line of a digest file.
pub const DIGEST_HEADER: &str = "# oa-track digest v1: second chain_hash second_hash rows";
/// Magic of a rows file; then `u32` version, `u32` row size, `u64` row count (little-endian).
pub const ROWS_MAGIC: &[u8; 8] = b"OATKROWS";
pub const ROWS_VERSION: u32 = 1;
pub const ROWS_HEADER_BYTES: usize = 24;
/// Rows held in memory at once when a rows file has to be written out of emission order.
const CHUNK_ROWS: u64 = 1 << 22;

/// `(spill column, offset in the canonical row)` of every field, in column order.
const FIELDS: [(usize, usize); 15] = [
    (0, OFF_T_NS),
    (1, OFF_ADDR),
    (2, OFF_LAT),
    (3, OFF_LON),
    (4, OFF_VE),
    (5, OFF_VN),
    (6, OFF_TURN),
    (7, OFF_ALT),
    (8, OFF_ALT_VAR),
    (9, OFF_VRATE),
    (10, OFF_POS_COV),
    (11, OFF_VEL_COV),
    (12, OFF_MU_H),
    (13, OFF_MU_V),
    (14, OFF_FLAGS),
];

fn put(row: &mut CanonRow, off: usize, bytes: &[u8]) {
    if let Some(dst) = row.get_mut(off..off + bytes.len()) {
        dst.copy_from_slice(bytes);
    }
}

fn put_f32s(row: &mut CanonRow, off: usize, v: &[f32]) {
    for (i, x) in v.iter().enumerate() {
        put(row, off + 4 * i, &x.to_bits().to_le_bytes());
    }
}

/// The canonical bytes of one row.
pub fn canon_row(r: &OutRow) -> CanonRow {
    let mut b = [0u8; ROW_BYTES];
    put(&mut b, OFF_ADDR, &r.addr.to_le_bytes());
    put(&mut b, OFF_T_NS, &r.t_ns.to_le_bytes());
    put(&mut b, OFF_LAT, &r.lat_e7.to_le_bytes());
    put(&mut b, OFF_LON, &r.lon_e7.to_le_bytes());
    put_f32s(&mut b, OFF_VE, &[r.vel_east_mps, r.vel_north_mps]);
    put_f32s(
        &mut b,
        OFF_TURN,
        &[r.turn_rate_rps, r.alt_ft, r.alt_var_ft2, r.vrate_fpm],
    );
    put_f32s(&mut b, OFF_POS_COV, &r.pos_cov_m2);
    put_f32s(&mut b, OFF_VEL_COV, &r.vel_cov_m2s2);
    put_f32s(&mut b, OFF_MU_H, &r.mu_h);
    put_f32s(&mut b, OFF_MU_V, &r.mu_v);
    put(&mut b, OFF_FLAGS, &[r.flags]);
    b
}

/// The canonical bytes of row `i` of `o` (None if a column is shorter than `t_ns`).
pub fn canon_row_at(o: &TrackOutput, i: usize) -> Option<CanonRow> {
    Some(canon_row(&OutRow {
        t_ns: *o.t_ns.get(i)?,
        addr: *o.addr.get(i)?,
        lat_e7: *o.lat_e7.get(i)?,
        lon_e7: *o.lon_e7.get(i)?,
        vel_east_mps: *o.vel_east_mps.get(i)?,
        vel_north_mps: *o.vel_north_mps.get(i)?,
        pos_cov_m2: *o.pos_cov_m2.get(i)?,
        vel_cov_m2s2: *o.vel_cov_m2s2.get(i)?,
        turn_rate_rps: *o.turn_rate_rps.get(i)?,
        alt_ft: *o.alt_ft.get(i)?,
        alt_var_ft2: *o.alt_var_ft2.get(i)?,
        vrate_fpm: *o.vrate_fpm.get(i)?,
        mu_h: *o.mu_h.get(i)?,
        mu_v: *o.mu_v.get(i)?,
        flags: *o.flags.get(i)?,
    }))
}

/// The Unix second of a canonical row's time (floored).
pub fn row_second(row: &CanonRow) -> i64 {
    let mut t = [0u8; 8];
    t.copy_from_slice(row.get(OFF_T_NS..OFF_T_NS + 8).unwrap_or(&[0; 8]));
    i64::from_le_bytes(t).div_euclid(1_000_000_000)
}

/// One output second of a digest.
#[derive(Debug, Clone, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct SecondEntry {
    pub second: i64,
    pub rows: u64,
    pub hash: [u8; 32],
    pub chain: [u8; 32],
}

impl SecondEntry {
    /// `second chain_hash second_hash rows`.
    pub fn line(&self) -> String {
        format!(
            "{} {} {} {}",
            self.second,
            to_hex(&self.chain),
            to_hex(&self.hash),
            self.rows
        )
    }
}

/// The chain hash after a second, given the one before it.
pub fn chain_next(prev: &[u8; 32], second: i64, hash: &[u8; 32], rows: u64) -> [u8; 32] {
    let mut m = Vec::with_capacity(80);
    m.extend_from_slice(prev);
    m.extend_from_slice(&second.to_le_bytes());
    m.extend_from_slice(hash);
    m.extend_from_slice(&rows.to_le_bytes());
    sha256(&m)
}

/// Accumulates rows (in emission order) into per-second digests.
#[derive(Default)]
// A7.2: cold struct, size guard waived
pub struct Digester {
    seconds: BTreeMap<i64, (Sha256, u64)>,
}

impl Digester {
    pub fn new() -> Digester {
        Digester::default()
    }

    pub fn push(&mut self, row: &CanonRow) {
        let e = self
            .seconds
            .entry(row_second(row))
            .or_insert_with(|| (Sha256::new(), 0));
        // A row is 85 bytes; a second's rows cannot reach the hasher's length limit.
        let _ = e.0.update(row);
        e.1 += 1;
    }

    /// The seconds in order, with their chain hashes.
    pub fn finish(self) -> Vec<SecondEntry> {
        let mut prev = [0u8; 32];
        let mut out = Vec::with_capacity(self.seconds.len());
        for (second, (h, rows)) in self.seconds {
            let hash = h.finalize();
            prev = chain_next(&prev, second, &hash, rows);
            out.push(SecondEntry {
                second,
                rows,
                hash,
                chain: prev,
            });
        }
        out
    }
}

/// Calls `f` with every row of a spilled run, canonical, in emission order.
pub fn spill_canon_rows(
    run: &SpilledRun,
    f: &mut dyn FnMut(&CanonRow) -> io::Result<()>,
) -> io::Result<()> {
    check_columns(&run.dir, run.rows)?;
    let mut cols = Vec::with_capacity(FIELDS.len());
    for (c, _) in FIELDS {
        let w = WIDTHS.get(c).copied().unwrap_or(1) as usize;
        cols.push((ColReader::open(&run.dir, c, w)?, w));
    }
    let mut seen = 0u64;
    loop {
        let mut blocks: Vec<Vec<u8>> = Vec::with_capacity(cols.len());
        for (r, _) in &mut cols {
            blocks.push(r.block()?.to_vec());
        }
        let n = blocks.first().map_or(0, |b| b.len() / 8);
        if n == 0 {
            break;
        }
        for i in 0..n {
            let mut row = [0u8; ROW_BYTES];
            for (k, ((_, w), (_, off))) in cols.iter().zip(FIELDS.iter()).enumerate() {
                let src = blocks.get(k).and_then(|b| b.get(i * w..(i + 1) * w));
                let Some(src) = src else {
                    return Err(io::Error::other(
                        "spill column files have different lengths",
                    ));
                };
                put(&mut row, *off, src);
            }
            f(&row)?;
            seen += 1;
        }
    }
    if seen != run.rows {
        return Err(io::Error::other("spill read back a different row count"));
    }
    Ok(())
}

/// The header of a rows file for `rows` rows.
pub fn rows_header(rows: u64) -> [u8; ROWS_HEADER_BYTES] {
    let mut h = [0u8; ROWS_HEADER_BYTES];
    put_hdr(&mut h, 0, ROWS_MAGIC);
    put_hdr(&mut h, 8, &ROWS_VERSION.to_le_bytes());
    put_hdr(&mut h, 12, &(ROW_BYTES as u32).to_le_bytes());
    put_hdr(&mut h, 16, &rows.to_le_bytes());
    h
}

fn put_hdr(h: &mut [u8; ROWS_HEADER_BYTES], off: usize, b: &[u8]) {
    if let Some(d) = h.get_mut(off..off + b.len()) {
        d.copy_from_slice(b);
    }
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
        d.push(r);
        Ok(())
    })?;
    let entries = d.finish();
    if let Some(p) = digest {
        let mut w = BufWriter::new(File::create(p)?);
        writeln!(w, "{DIGEST_HEADER}")?;
        for e in &entries {
            writeln!(w, "{}", e.line())?;
        }
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
                    if n > 0 && n + e.rows > CHUNK_ROWS {
                        break;
                    }
                    n += e.rows;
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
        seconds: entries.len() as u64,
        rows: run.rows,
        in_order,
    })
}
