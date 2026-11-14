//! The ADSB payload carried in OAF1 MESSAGE frames (docs/ADSB_PAYLOAD.md). Core (pure).
//! Two layouts are read: `ADS1` (64-byte records, the first clips) and `ADS2` (80-byte records,
//! adds the measured wind, temperature, speeds, headings and roll); only `ADS2` is written.

use std::fmt;

use crate::{
    reports_len, reports_push, reports_row, Reports, Row, F_GROUND, NONE_I16, NONE_U16, NONE_U8,
    ROW_EMPTY, SOURCE_NAMES,
};

/// First four payload bytes of the layout written now.
pub const PAYLOAD_MAGIC: [u8; 4] = *b"ADS2";
/// First four payload bytes of the first layout, still read.
pub const LEGACY_PAYLOAD_MAGIC: [u8; 4] = *b"ADS1";
/// Bytes before the first record.
pub const PAYLOAD_HEADER_LEN: usize = 16;
/// Bytes of one record.
pub const RECORD_LEN: usize = 80;
/// Bytes of one record of the first layout.
pub const LEGACY_RECORD_LEN: usize = 64;
/// Most records in one frame: the largest count that keeps the payload (header plus records)
/// under the 4 MiB OAF1 limit, rounded down to a round number.
pub const MAX_ROWS_PER_FRAME: usize = 50_000;
/// Most records in one frame of the first layout, which has the shorter record.
pub const LEGACY_MAX_ROWS: usize = 60_000;
const _: () = assert!(PAYLOAD_HEADER_LEN + LEGACY_MAX_ROWS * LEGACY_RECORD_LEN <= 4 * 1024 * 1024);
const _: () = assert!(PAYLOAD_HEADER_LEN + MAX_ROWS_PER_FRAME * RECORD_LEN <= 4 * 1024 * 1024);

/// Frame time span of the clip tool when `--frame-ms` is not given, in milliseconds.
pub const DEFAULT_FRAME_MS: u32 = 1000;
/// Smallest and largest accepted `--frame-ms`.
pub const MIN_FRAME_MS: u32 = 1;
pub const MAX_FRAME_MS: u32 = 60_000;

/// Splits time-sorted `r` into frame row ranges `(from, to)`: a range ends when the next report
/// is at least `frame_ms` after the range's first report, or at [`MAX_ROWS_PER_FRAME`] rows.
/// The ranges are in order, non-empty and cover every row once. `frame_ms` is clamped to
/// [`MIN_FRAME_MS`]..=[`MAX_FRAME_MS`].
pub fn frame_ranges(r: &Reports, frame_ms: u32) -> Vec<(usize, usize)> {
    let span_ns = i64::from(frame_ms.clamp(MIN_FRAME_MS, MAX_FRAME_MS)) * 1_000_000;
    let n = reports_len(r);
    let mut out = Vec::new();
    let mut from = 0;
    while from < n {
        let t0 = r.time_ns.get(from).copied().unwrap_or(0);
        let mut to = from + 1;
        while to < n
            && to - from < MAX_ROWS_PER_FRAME
            && r.time_ns.get(to).copied().unwrap_or(0).saturating_sub(t0) < span_ns
        {
            to += 1;
        }
        out.push((from, to));
        from = to;
    }
    out
}

/// Why a payload was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayloadError {
    /// Shorter than the header, or the length is not header + count * record.
    Length,
    /// Neither `ADS2` nor `ADS1`.
    Magic,
    /// Record length field is not that of the layout (80, or 64 for `ADS1`), or a reserved field is not zero.
    Layout,
    /// More than [`MAX_ROWS_PER_FRAME`] records (or [`LEGACY_MAX_ROWS`] for `ADS1`).
    TooManyRows,
    /// Record `n` (0-based) has a value outside its range.
    BadRecord(usize),
    /// A row range outside the reports, or empty.
    Range,
}

impl fmt::Display for PayloadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PayloadError::Length => write!(f, "adsb payload: wrong length"),
            PayloadError::Magic => write!(f, "adsb payload: bad magic"),
            PayloadError::Layout => write!(f, "adsb payload: unsupported layout"),
            PayloadError::TooManyRows => write!(f, "adsb payload: too many records"),
            PayloadError::BadRecord(n) => write!(f, "adsb payload: record {n} out of range"),
            PayloadError::Range => write!(f, "adsb payload: bad row range"),
        }
    }
}

impl std::error::Error for PayloadError {}

fn put(dst: &mut [u8; RECORD_LEN], at: usize, bytes: &[u8]) {
    if let Some(d) = dst.get_mut(at..at + bytes.len()) {
        d.copy_from_slice(bytes);
    }
}

fn encode_row(r: &Row) -> [u8; RECORD_LEN] {
    let mut b = [0u8; RECORD_LEN];
    put(&mut b, 0, &r.icao.to_le_bytes());
    put(&mut b, 4, &r.lat_e7.to_le_bytes());
    put(&mut b, 8, &r.lon_e7.to_le_bytes());
    put(&mut b, 12, &r.baro_alt_ft.to_le_bytes());
    put(&mut b, 16, &r.geom_alt_ft.to_le_bytes());
    put(&mut b, 20, &r.vrate_fpm.to_le_bytes());
    put(&mut b, 24, &r.time_ns.to_le_bytes());
    put(&mut b, 32, &r.gs_dkt.to_le_bytes());
    put(&mut b, 34, &r.track_ddeg.to_le_bytes());
    put(&mut b, 36, &r.squawk.to_le_bytes());
    put(&mut b, 38, &[r.flags, r.source]);
    put(&mut b, 40, &r.callsign);
    put(
        &mut b,
        48,
        &[
            r.category, r.nic, r.nac_p, r.nac_v, r.sil, r.sil_type, r.version, r.gva, r.sda,
        ],
    );
    for (i, v) in [
        r.ws_kt,
        r.wd_deg,
        r.tas_kt,
        r.ias_kt,
        r.mach_milli,
        r.mag_heading_cdeg,
        r.true_heading_cdeg,
    ]
    .into_iter()
    .enumerate()
    {
        put(&mut b, 57 + 2 * i, &v.to_le_bytes());
    }
    for (i, v) in [r.oat_c, r.tat_c, r.roll_cdeg, r.track_rate_cdps]
        .into_iter()
        .enumerate()
    {
        put(&mut b, 71 + 2 * i, &v.to_le_bytes());
    }
    b
}

fn rd<const N: usize>(b: &[u8], at: usize) -> Option<[u8; N]> {
    b.get(at..at + N)?.try_into().ok()
}

/// The measurements of an `ADS2` record, or all absent for an `ADS1` one (whose bytes from 57 on
/// must be zero).
fn decode_measurements(b: &[u8], row: &mut Row, legacy: bool) -> Option<()> {
    if legacy {
        return b
            .get(57..LEGACY_RECORD_LEN)?
            .iter()
            .all(|&x| x == 0)
            .then_some(());
    }
    if *b.get(79)? != 0 {
        return None;
    }
    let u = |i: usize| -> Option<u16> { Some(u16::from_le_bytes(rd(b, 57 + 2 * i)?)) };
    let s = |i: usize| -> Option<i16> { Some(i16::from_le_bytes(rd(b, 71 + 2 * i)?)) };
    row.ws_kt = u(0)?;
    row.wd_deg = u(1)?;
    row.tas_kt = u(2)?;
    row.ias_kt = u(3)?;
    row.mach_milli = u(4)?;
    row.mag_heading_cdeg = u(5)?;
    row.true_heading_cdeg = u(6)?;
    row.oat_c = s(0)?;
    row.tat_c = s(1)?;
    row.roll_cdeg = s(2)?;
    row.track_rate_cdps = s(3)?;
    // Ranges the decoder guarantees: a value outside them is a corrupt record.
    let heading_ok = |v: u16| v == NONE_U16 || v < 36_000;
    let ok = (row.wd_deg == NONE_U16 || row.wd_deg < 360)
        && heading_ok(row.mag_heading_cdeg)
        && heading_ok(row.true_heading_cdeg)
        && (row.roll_cdeg == NONE_I16 || row.roll_cdeg.abs() <= 18_000);
    ok.then_some(())
}

fn decode_row(b: &[u8], legacy: bool) -> Option<Row> {
    let u8s: [u8; 9] = rd(b, 48)?;
    let [category, nic, nac_p, nac_v, sil, sil_type, version, gva, sda] = u8s;
    let mut row = Row {
        icao: u32::from_le_bytes(rd(b, 0)?),
        lat_e7: i32::from_le_bytes(rd(b, 4)?),
        lon_e7: i32::from_le_bytes(rd(b, 8)?),
        baro_alt_ft: i32::from_le_bytes(rd(b, 12)?),
        geom_alt_ft: i32::from_le_bytes(rd(b, 16)?),
        vrate_fpm: i32::from_le_bytes(rd(b, 20)?),
        time_ns: i64::from_le_bytes(rd(b, 24)?),
        gs_dkt: u16::from_le_bytes(rd(b, 32)?),
        track_ddeg: u16::from_le_bytes(rd(b, 34)?),
        squawk: u16::from_le_bytes(rd(b, 36)?),
        flags: *b.get(38)?,
        source: *b.get(39)?,
        callsign: rd(b, 40)?,
        category,
        nic,
        nac_p,
        nac_v,
        sil,
        sil_type,
        version,
        gva,
        sda,
        ..ROW_EMPTY
    };
    decode_measurements(b, &mut row, legacy)?;
    let ok = (-900_000_000..=900_000_000).contains(&row.lat_e7)
        && (-1_800_000_000..=1_800_000_000).contains(&row.lon_e7)
        && row.flags & !(F_GROUND | 15) == 0
        && (row.source == NONE_U8 || usize::from(row.source) < SOURCE_NAMES.len());
    ok.then_some(row)
}

/// Appends one payload holding reports `from..to` of `r` to `out`. The range must be non-empty
/// and inside the reports, with at most [`MAX_ROWS_PER_FRAME`] rows.
pub fn encode_payload(
    out: &mut Vec<u8>,
    r: &Reports,
    from: usize,
    to: usize,
) -> Result<(), PayloadError> {
    if from >= to || to > reports_len(r) {
        return Err(PayloadError::Range);
    }
    if to - from > MAX_ROWS_PER_FRAME {
        return Err(PayloadError::TooManyRows);
    }
    out.extend_from_slice(&PAYLOAD_MAGIC);
    out.extend_from_slice(&(RECORD_LEN as u16).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&((to - from) as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    for i in from..to {
        let row = reports_row(r, i).ok_or(PayloadError::Range)?;
        out.extend_from_slice(&encode_row(&row));
    }
    Ok(())
}

/// Parses one payload, appending its reports to `out`. Returns the count. A bad payload
/// appends nothing.
pub fn decode_payload(bytes: &[u8], out: &mut Reports) -> Result<usize, PayloadError> {
    if bytes.len() < PAYLOAD_HEADER_LEN {
        return Err(PayloadError::Length);
    }
    let legacy = match bytes.get(..4) {
        Some(m) if m == PAYLOAD_MAGIC => false,
        Some(m) if m == LEGACY_PAYLOAD_MAGIC => true,
        _ => return Err(PayloadError::Magic),
    };
    let (rec_len, max_rows) = if legacy {
        (LEGACY_RECORD_LEN, LEGACY_MAX_ROWS)
    } else {
        (RECORD_LEN, MAX_ROWS_PER_FRAME)
    };
    let rec = u16::from_le_bytes(rd(bytes, 4).ok_or(PayloadError::Length)?);
    let res1 = u16::from_le_bytes(rd(bytes, 6).ok_or(PayloadError::Length)?);
    let count = u32::from_le_bytes(rd(bytes, 8).ok_or(PayloadError::Length)?) as usize;
    let res2 = u32::from_le_bytes(rd(bytes, 12).ok_or(PayloadError::Length)?);
    if usize::from(rec) != rec_len || res1 != 0 || res2 != 0 {
        return Err(PayloadError::Layout);
    }
    if count > max_rows {
        return Err(PayloadError::TooManyRows);
    }
    if bytes.len() != PAYLOAD_HEADER_LEN + count * rec_len {
        return Err(PayloadError::Length);
    }
    let mut rows = Vec::with_capacity(count);
    for i in 0..count {
        let at = PAYLOAD_HEADER_LEN + i * rec_len;
        let rec = bytes.get(at..at + rec_len).ok_or(PayloadError::Length)?;
        rows.push(decode_row(rec, legacy).ok_or(PayloadError::BadRecord(i))?);
    }
    for row in &rows {
        reports_push(out, row);
    }
    Ok(count)
}
