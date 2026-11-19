//! The canonical bytes of a tracker output row, the per-second digest over them, and the header
//! of the rows file that explains a difference between two runs. Core (B1): every item here is
//! pure and does no I/O; the files are written by [`crate::canon_spill`] (shell).
//!
//! Classification of every public item:
//!
//! | item | class |
//! |---|---|
//! | [`ROW_BYTES`], `OFF_*`, [`DIGEST_HEADER`], [`ROWS_MAGIC`], [`ROWS_VERSION`], [`ROWS_HEADER_BYTES`], [`MAX_ROWS_PER_SECOND`], [`MAX_SECONDS`], [`CANON_FIELDS`], [`CANON_NAN_F32`] | core (constants) |
//! | [`canon_row`], [`canon_row_at`], [`canon_row_floats`], [`row_second`], [`field_at`] | core, pure |
//! | [`second_line`], [`digest_end_line`], [`chain_next`], [`hash_rows`], [`hex32`], [`from_hex32`] | core, pure |
//! | [`Digester`] (`new`, `push`, `finish`) | core, pure (a subsystem under D5: owns its sums) |
//! | [`rows_header`], [`parse_rows_header`], [`limit_text`] | core, pure |
//! | [`SecondEntry`], [`LimitError`], [`CanonRow`] | plain data |
//!
//! A row is [`ROW_BYTES`] bytes, every field little-endian at a fixed offset, floats as their
//! IEEE bits (every NaN becomes the one fixed NaN, [`CANON_NAN_F32`]; nothing else is zeroed or normalised). Rows are
//! grouped by the Unix second of their time (`t_ns` floored to a second). The digest of a second
//! is the SHA-256 of its rows in the order the tracker emitted them; the chain hash of a second
//! is the SHA-256 of the chain hash of the second before it (32 zero bytes for the first), the
//! second as `i64` little-endian, the second's digest and its row count as `u64` little-endian.
//! No floating-point value is decoded here: the rows are moved and hashed as bytes.
//!
//! # Limits (C2, C7)
//!
//! A digest holds at most [`MAX_SECONDS`] seconds and a second at most [`MAX_ROWS_PER_SECOND`]
//! rows (about 89 MB held whole by a reader). [`Digester::push`] refuses a row that would pass
//! either, as an error, before the row is counted; the readers of a digest and of a rows file
//! refuse a declared size above them before any loop runs.

use std::collections::BTreeMap;

use oa_sha256::{sha256, to_hex, Sha256};

use crate::spill::WIDTHS;
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

/// First line of a digest file. v2 ends the file with an `end` line ([`digest_end_line`]), so a
/// digest cut at a line boundary is not mistaken for a shorter run.
pub const DIGEST_HEADER: &str =
    "# oa-track digest v2: second chain_hash second_hash rows; last line: end seconds rows chain";
/// Magic of a rows file; then `u32` version, `u32` row size, `u64` row count (little-endian).
pub const ROWS_MAGIC: &[u8; 8] = b"OATKROWS";
pub const ROWS_VERSION: u32 = 1;
pub const ROWS_HEADER_BYTES: usize = 24;
/// Most rows one Unix second may hold (C2): about 89 MB of canonical rows, three orders of
/// magnitude above the busiest second of the national airspace. A reader holds one second whole.
pub const MAX_ROWS_PER_SECOND: u64 = 1 << 20;
/// Most distinct seconds one digest may list (C2): about twelve days.
pub const MAX_SECONDS: usize = 1 << 20;

/// Number of fields of the spill column table that make up a canonical row.
pub const CANON_FIELDS: usize = 15;

/// `(spill column, offset in the canonical row)` of every field, in column order.
pub(crate) const FIELDS: [(usize, usize); CANON_FIELDS] = [
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

// D4/F18: the layout is stated here and in the spill column table; these tie them together at
// compile time. The column of entry `i` is `i`, its width fits in the row, and the fields tile
// bytes 0..ROW_BYTES exactly once each, with no gap and no overlap. A failure is a compile error,
// so the indexing here cannot panic at run time.
#[allow(clippy::indexing_slicing)]
const _: () = {
    let mut seen = [false; ROW_BYTES];
    let mut i = 0;
    while i < CANON_FIELDS {
        assert!(FIELDS[i].0 == i);
        let (off, w) = (FIELDS[i].1, WIDTHS[i] as usize);
        assert!(off + w <= ROW_BYTES);
        let mut b = off;
        while b < off + w {
            assert!(!seen[b], "two canonical fields overlap");
            seen[b] = true;
            b += 1;
        }
        i += 1;
    }
    let mut b = 0;
    while b < ROW_BYTES {
        assert!(seen[b], "a byte of the canonical row belongs to no field");
        b += 1;
    }
    assert!(WIDTHS.len() == CANON_FIELDS);
};

// One second's rows hash in far fewer bytes than the hasher's message limit (u64::MAX / 8).
const _: () = assert!((MAX_ROWS_PER_SECOND as u128) * (ROW_BYTES as u128) < (u64::MAX / 8) as u128);

/// Writes `bytes` at `OFF`. The range is checked at compile time, so there is no runtime case
/// in which the bytes land nowhere.
fn put<const OFF: usize, const N: usize>(row: &mut CanonRow, bytes: [u8; N]) {
    const { assert!(OFF + N <= ROW_BYTES) };
    for (d, s) in row.iter_mut().skip(OFF).zip(bytes) {
        *d = s;
    }
}

/// The one NaN of a canonical row (positive, quiet, no payload). Local on purpose: this file is
/// also carried by the replay overlay onto old commits, which have no newer `oa-math` item; the
/// test `tests/canon_nan.rs` pins it to `oa_math::canon_nan_f32`.
pub const CANON_NAN_F32: u32 = 0x7fc0_0000;

/// The IEEE bits of `x`, every NaN as [`CANON_NAN_F32`].
fn canon_f32_bits(x: f32) -> u32 {
    if x.is_nan() {
        CANON_NAN_F32
    } else {
        x.to_bits()
    }
}

/// Writes `N` little-endian `f32` bit patterns at `OFF`.
fn put_f32s<const OFF: usize, const N: usize>(row: &mut CanonRow, v: [f32; N]) {
    const { assert!(OFF + 4 * N <= ROW_BYTES) };
    let bytes = v.into_iter().flat_map(|x| canon_f32_bits(x).to_le_bytes());
    for (d, s) in row.iter_mut().skip(OFF).zip(bytes) {
        *d = s;
    }
}

/// Rewrites every NaN among the `f32` fields of a row (bytes `OFF_VE` up to `OFF_FLAGS`) as
/// [`CANON_NAN_F32`]; the other bytes stay. A row read back from raw column bytes goes through
/// this, so a base that spilled raw bits still yields canonical rows.
pub fn canon_row_floats(row: &mut CanonRow) {
    const { assert!(OFF_FLAGS > OFF_VE && (OFF_FLAGS - OFF_VE) % 4 == 0) };
    let Some(floats) = row.get_mut(OFF_VE..OFF_FLAGS) else {
        return;
    };
    for c in floats.chunks_exact_mut(4) {
        if let Ok(b) = <[u8; 4]>::try_from(&*c) {
            let bits = canon_f32_bits(f32::from_le_bytes(b));
            c.copy_from_slice(&bits.to_le_bytes());
        }
    }
}

/// The `N` bytes at `OFF` of a row; the range is checked at compile time.
pub fn field_at<const OFF: usize, const N: usize>(row: &CanonRow) -> [u8; N] {
    const { assert!(OFF + N <= ROW_BYTES) };
    let mut out = [0u8; N];
    for (d, s) in out.iter_mut().zip(row.iter().skip(OFF)) {
        *d = *s;
    }
    out
}

/// The canonical bytes of one row.
pub fn canon_row(r: &OutRow) -> CanonRow {
    let mut b = [0u8; ROW_BYTES];
    put::<OFF_ADDR, 4>(&mut b, r.addr.to_le_bytes());
    put::<OFF_T_NS, 8>(&mut b, r.t_ns.to_le_bytes());
    put::<OFF_LAT, 4>(&mut b, r.lat_e7.to_le_bytes());
    put::<OFF_LON, 4>(&mut b, r.lon_e7.to_le_bytes());
    put_f32s::<OFF_VE, 2>(&mut b, [r.vel_east_mps, r.vel_north_mps]);
    put_f32s::<OFF_TURN, 4>(
        &mut b,
        [r.turn_rate_rps, r.alt_ft, r.alt_var_ft2, r.vrate_fpm],
    );
    put_f32s::<OFF_POS_COV, 3>(&mut b, r.pos_cov_m2);
    put_f32s::<OFF_VEL_COV, 3>(&mut b, r.vel_cov_m2s2);
    put_f32s::<OFF_MU_H, 2>(&mut b, r.mu_h);
    put_f32s::<OFF_MU_V, 2>(&mut b, r.mu_v);
    put::<OFF_FLAGS, 1>(&mut b, [r.flags]);
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
    i64::from_le_bytes(field_at::<OFF_T_NS, 8>(row)).div_euclid(1_000_000_000)
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

/// The digest-file line of a second: `second chain_hash second_hash rows`.
pub fn second_line(e: &SecondEntry) -> String {
    format!(
        "{} {} {} {}",
        e.second,
        to_hex(&e.chain),
        to_hex(&e.hash),
        e.rows
    )
}

/// The last line of a digest file: `end <seconds> <rows> <chain hash of the last second>` (32
/// zero bytes, in hex, for a run with no seconds). The reader recomputes all three from the lines
/// before it and requires equality.
pub fn digest_end_line(entries: &[SecondEntry]) -> String {
    let rows = entries.iter().fold(0u64, |a, e| a.saturating_add(e.rows));
    let chain = entries.last().map_or([0u8; 32], |e| e.chain);
    format!("end {} {} {}", entries.len(), rows, to_hex(&chain))
}

/// Lowercase hex of 32 bytes.
pub fn hex32(b: &[u8; 32]) -> String {
    to_hex(b)
}

/// The 32 bytes of 64 lowercase hex digits; None for anything else (uppercase included).
pub fn from_hex32(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 {
        return None;
    }
    let nib = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    };
    let mut out = [0u8; 32];
    for (o, pair) in out.iter_mut().zip(s.as_bytes().chunks_exact(2)) {
        let [hi, lo] = pair else { return None };
        *o = nib(*hi)? << 4 | nib(*lo)?;
    }
    Some(out)
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

/// The digest of one second given its rows in emission order: the SHA-256 of the rows back to
/// back, as [`Digester`] computes it. None when the rows are more than the hasher can take
/// (never, below [`MAX_ROWS_PER_SECOND`]).
pub fn hash_rows(rows: &[CanonRow]) -> Option<[u8; 32]> {
    let mut h = Sha256::new();
    for r in rows {
        h.update(r).ok()?;
    }
    Some(h.finalize())
}

/// A digest limit that a row would pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitError {
    /// More than [`MAX_SECONDS`] distinct seconds.
    TooManySeconds,
    /// More than [`MAX_ROWS_PER_SECOND`] rows in one second.
    SecondTooLarge { second: i64 },
    /// The hasher refused the message length (not reachable below the limits).
    Hash,
}

/// The sentence for a [`LimitError`].
pub fn limit_text(e: LimitError) -> String {
    match e {
        LimitError::TooManySeconds => {
            format!("more than {MAX_SECONDS} distinct seconds in one run (limit)")
        }
        LimitError::SecondTooLarge { second } => {
            format!("second {second} has more than {MAX_ROWS_PER_SECOND} rows (limit)")
        }
        LimitError::Hash => "the hasher refused a second's message length".to_string(),
    }
}

/// Accumulates rows (in emission order) into per-second digests.
// A7.2: cold struct, size guard waived
pub struct Digester {
    seconds: BTreeMap<i64, (Sha256, u64)>,
    max_seconds: usize,
    max_rows: u64,
}

impl Default for Digester {
    fn default() -> Digester {
        Digester::with_limits(MAX_SECONDS, MAX_ROWS_PER_SECOND)
    }
}

impl Digester {
    pub fn new() -> Digester {
        Digester::default()
    }

    /// A digester with smaller limits than the named ones (a test of the refusal).
    pub fn with_limits(max_seconds: usize, max_rows: u64) -> Digester {
        Digester {
            seconds: BTreeMap::new(),
            max_seconds: max_seconds.min(MAX_SECONDS),
            max_rows: max_rows.min(MAX_ROWS_PER_SECOND),
        }
    }

    /// Adds a row. A row that would pass a limit is refused and not counted.
    pub fn push(&mut self, row: &CanonRow) -> Result<(), LimitError> {
        let s = row_second(row);
        if !self.seconds.contains_key(&s) && self.seconds.len() >= self.max_seconds {
            return Err(LimitError::TooManySeconds);
        }
        let e = self.seconds.entry(s).or_insert_with(|| (Sha256::new(), 0));
        if e.1 >= self.max_rows {
            return Err(LimitError::SecondTooLarge { second: s });
        }
        e.0.update(row).map_err(|_| LimitError::Hash)?;
        e.1 = e.1.saturating_add(1);
        Ok(())
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

/// The header of a rows file for `rows` rows.
pub fn rows_header(rows: u64) -> [u8; ROWS_HEADER_BYTES] {
    let mut h = [0u8; ROWS_HEADER_BYTES];
    for (d, s) in h.iter_mut().zip(
        ROWS_MAGIC
            .iter()
            .copied()
            .chain(ROWS_VERSION.to_le_bytes())
            .chain((ROW_BYTES as u32).to_le_bytes())
            .chain(rows.to_le_bytes()),
    ) {
        *d = s;
    }
    h
}

const _: () = assert!(8 + 4 + 4 + 8 == ROWS_HEADER_BYTES);

/// Checks the header of a rows file in full (magic, version, row size, exact length) and returns
/// the row count it declares.
pub fn parse_rows_header(h: &[u8]) -> Result<u64, String> {
    let Ok(h) = <[u8; ROWS_HEADER_BYTES]>::try_from(h) else {
        return Err(format!(
            "rows file header is {} bytes, expected {ROWS_HEADER_BYTES}",
            h.len()
        ));
    };
    let mut at = 0usize;
    let mut take = |n: usize| {
        let s: Vec<u8> = h.iter().skip(at).take(n).copied().collect();
        at += n;
        s
    };
    if take(8) != ROWS_MAGIC {
        return Err("not a rows file (bad magic)".to_string());
    }
    let ver = le_u32(&take(4));
    if ver != ROWS_VERSION {
        return Err(format!("rows file version {ver}, expected {ROWS_VERSION}"));
    }
    let size = le_u32(&take(4));
    if usize::try_from(size).ok() != Some(ROW_BYTES) {
        return Err(format!("rows file row size {size}, expected {ROW_BYTES}"));
    }
    let mut c = [0u8; 8];
    for (d, s) in c.iter_mut().zip(take(8)) {
        *d = s;
    }
    Ok(u64::from_le_bytes(c))
}

fn le_u32(b: &[u8]) -> u32 {
    let mut x = [0u8; 4];
    for (d, s) in x.iter_mut().zip(b) {
        *d = *s;
    }
    u32::from_le_bytes(x)
}
