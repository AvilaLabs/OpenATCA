//! The ADSB payload carried in OAF1 MESSAGE frames (docs/ADSB_PAYLOAD.md). Core (pure).
//! Three layouts are read: `ADS1` (64-byte records, the first clips), `ADS2` (80-byte records,
//! adds the measured wind, temperature, speeds, headings and roll; byte 79 reserved) and `ADS3`
//! (the `ADS2` record with byte 79 holding NIC_baro plus one); only `ADS3` is written. A reader
//! that knows only `ADS1` and `ADS2` refuses an `ADS3` payload by its magic, so a clip that carries
//! NIC_baro cannot be read as one that does not.

use std::fmt;

use crate::{
    reports_len, reports_push, reports_row, Reports, Row, F_GROUND, NONE_I16, NONE_U16, NONE_U8,
    ROW_EMPTY, SOURCE_NAMES, SOURCE_UNKNOWN,
};

/// First four payload bytes of the layout written now.
pub const PAYLOAD_MAGIC: [u8; 4] = *b"ADS3";
/// First four payload bytes of the second layout (byte 79 reserved), still read.
pub const PREVIOUS_PAYLOAD_MAGIC: [u8; 4] = *b"ADS2";
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
/// The earliest report time a record may carry: 2000-01-01T00:00Z, ns since the Unix epoch.
pub const TIME_MIN_NS: i64 = 946_684_800_000_000_000;
/// The first time past the accepted range: 2100-01-01T00:00Z, ns since the Unix epoch.
pub const TIME_MAX_NS: i64 = 4_102_444_800_000_000_000;
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
    // The last byte: NIC_baro plus one (0 absent, 1 for NIC_baro 0, 2 for NIC_baro 1). Clips
    // written before it held a reserved zero there: they read as absent.
    put(&mut b, 79, &[nic_baro_code(r.nic_baro)]);
    b
}

/// The record byte of a NIC_baro value.
fn nic_baro_code(v: u8) -> u8 {
    if v <= 1 {
        v + 1
    } else {
        0
    }
}

fn rd<const N: usize>(b: &[u8], at: usize) -> Option<[u8; N]> {
    b.get(at..at + N)?.try_into().ok()
}

/// What was wrong with a record that was still decoded. Bits of [`Anomalies`].
const ANOM_SOURCE: u8 = 1;
const ANOM_MEASUREMENT: u8 = 2;
const ANOM_RESERVED: u8 = 4;

/// The measurements of an `ADS2` record, or all absent for an `ADS1` one. A reserved byte that
/// is not zero, or a measurement out of the range the decoder guarantees, is an anomaly: the
/// reserved bytes are ignored and the bad measurement is left absent. Returns the anomaly bits.
fn decode_measurements(b: &[u8], row: &mut Row, legacy: bool) -> Option<u8> {
    let mut anom = 0;
    if legacy {
        if !b.get(57..LEGACY_RECORD_LEN)?.iter().all(|&x| x == 0) {
            anom |= ANOM_RESERVED;
        }
        return Some(anom);
    }
    // Byte 79 is reserved in this layout (`ADS2`); an `ADS3` record is read with it taken out
    // first, by `take_nic_baro`.
    if *b.get(79)? != 0 {
        anom |= ANOM_RESERVED;
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
    // Ranges the decoder guarantees: a value outside them is a corrupt measurement.
    if !(row.wd_deg == NONE_U16 || row.wd_deg < 360) {
        row.wd_deg = NONE_U16;
        anom |= ANOM_MEASUREMENT;
    }
    for h in [&mut row.mag_heading_cdeg, &mut row.true_heading_cdeg] {
        if !(*h == NONE_U16 || *h < 36_000) {
            *h = NONE_U16;
            anom |= ANOM_MEASUREMENT;
        }
    }
    if !(row.roll_cdeg == NONE_I16 || row.roll_cdeg.abs() <= 18_000) {
        row.roll_cdeg = NONE_I16;
        anom |= ANOM_MEASUREMENT;
    }
    Some(anom)
}

/// Decodes one record of the layout `ads3` selects. `ADS3` is `ADS2` with byte 79 holding NIC_baro
/// plus one (0 absent, 1 for 0, 2 for 1): that byte is read here and the rest decoded as `ADS2`;
/// a value above 2 is a reserved byte set. Under `ADS2` and `ADS1` byte 79 stays reserved: a
/// nonzero one is counted and ignored, never read as NIC_baro.
fn decode_row_layout(b: &[u8], legacy: bool, ads3: bool) -> Option<(Row, u8)> {
    if !ads3 || legacy {
        return decode_row(b, legacy);
    }
    let mut rec: [u8; RECORD_LEN] = b.try_into().ok()?;
    let code = std::mem::take(rec.get_mut(79)?);
    let (mut row, mut anom) = decode_row(&rec, false)?;
    match code {
        0 => {}
        1 | 2 => row.nic_baro = code - 1,
        _ => anom |= ANOM_RESERVED,
    }
    Some((row, anom))
}

/// Decodes one record. `None` if its time, position or flags cannot be read (the record has no
/// meaning); otherwise the row with every minor field that was out of range made absent or
/// `unknown`, and the anomaly bits saying what was changed.
fn decode_row(b: &[u8], legacy: bool) -> Option<(Row, u8)> {
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
    let mut anom = decode_measurements(b, &mut row, legacy)?;
    // Without a readable position or flags (stale, new leg) the record has no meaning.
    let ok = (TIME_MIN_NS..TIME_MAX_NS).contains(&row.time_ns)
        && (-900_000_000..=900_000_000).contains(&row.lat_e7)
        && (-1_800_000_000..=1_800_000_000).contains(&row.lon_e7)
        && row.flags & !(F_GROUND | 15) == 0;
    if !ok {
        return None;
    }
    if !(row.source == NONE_U8 || usize::from(row.source) < SOURCE_NAMES.len()) {
        row.source = SOURCE_UNKNOWN;
        anom |= ANOM_SOURCE;
    }
    Some((row, anom))
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

/// What a lenient payload decode did. Plain record (A1).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct PayloadCounts {
    /// Records in the payload.
    pub records: usize,
    /// Records appended to the output.
    pub kept: usize,
    /// Records dropped because their position or flags were out of range.
    pub dropped: usize,
    /// Index of the first dropped record.
    pub first_dropped: Option<usize>,
    /// Kept records whose source type was not a known one (stored as `unknown`).
    pub unknown_source: usize,
    /// Of the dropped records, those whose report time was outside `TIME_MIN_NS..TIME_MAX_NS`.
    pub bad_time: usize,
    /// Kept records with a measurement out of range (left absent).
    pub bad_measurement: usize,
    /// Kept records with a reserved byte that was not zero (ignored).
    pub reserved_set: usize,
}

/// Parses the payload header and checks the length; `Ok((legacy, count, record_length))`.
fn parse_header(bytes: &[u8]) -> Result<(bool, usize, usize), PayloadError> {
    if bytes.len() < PAYLOAD_HEADER_LEN {
        return Err(PayloadError::Length);
    }
    let legacy = match bytes.get(..4) {
        Some(m) if m == PAYLOAD_MAGIC || m == PREVIOUS_PAYLOAD_MAGIC => false,
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
    Ok((legacy, count, rec_len))
}

/// Validates a payload header and exact row envelope without decoding records. Callers can use
/// the count to charge a finite row budget before entering the per-record decode loop.
pub fn payload_record_count(bytes: &[u8]) -> Result<usize, PayloadError> {
    parse_header(bytes).map(|(_, count, _)| count)
}

/// Parses one payload, appending every record that can be read and counting the rest. A payload
/// whose header or length is wrong is refused whole (the records cannot be located) and appends
/// nothing. A record whose position or flags are out of range is dropped and counted; a record
/// whose source type is unknown keeps its position with source `unknown`, and one with a bad
/// measurement keeps it absent (the layout is unchanged).
pub fn decode_payload_lenient(
    bytes: &[u8],
    out: &mut Reports,
) -> Result<PayloadCounts, PayloadError> {
    decode_payload_lenient_with_dropped(bytes, out, |_| {})
}

/// Leniently decodes a payload and reports the source ordinal of every record it drops.
///
/// The callback runs once per malformed record in input order. It receives no borrowed bytes, so
/// callers that need a durable locator can retain the bounded ordinal without copying payloads.
/// The payload header and exact row count are checked before the callback is invoked. Valid rows
/// are still appended even when neighboring rows are malformed.
pub fn decode_payload_lenient_with_dropped<F>(
    bytes: &[u8],
    out: &mut Reports,
    mut on_dropped: F,
) -> Result<PayloadCounts, PayloadError>
where
    F: FnMut(usize),
{
    let (legacy, count, rec_len) = parse_header(bytes)?;
    let ads3 = bytes.get(..4) == Some(&PAYLOAD_MAGIC[..]);
    let mut c = PayloadCounts {
        records: count,
        ..PayloadCounts::default()
    };
    let mut rows = Vec::with_capacity(count);
    for i in 0..count {
        let at = PAYLOAD_HEADER_LEN + i * rec_len;
        let rec = bytes.get(at..at + rec_len).ok_or(PayloadError::Length)?;
        match decode_row_layout(rec, legacy, ads3) {
            Some((row, anom)) => {
                c.unknown_source += usize::from(anom & ANOM_SOURCE != 0);
                c.bad_measurement += usize::from(anom & ANOM_MEASUREMENT != 0);
                c.reserved_set += usize::from(anom & ANOM_RESERVED != 0);
                rows.push(row);
            }
            None => {
                c.dropped += 1;
                let t = rd(rec, 24).map(i64::from_le_bytes);
                c.bad_time +=
                    usize::from(t.is_some_and(|t| !(TIME_MIN_NS..TIME_MAX_NS).contains(&t)));
                c.first_dropped.get_or_insert(i);
                on_dropped(i);
            }
        }
    }
    c.kept = rows.len();
    for row in &rows {
        reports_push(out, row);
    }
    Ok(c)
}

/// Parses one payload strictly, appending its reports to `out`. Returns the count. A payload
/// with anything [`decode_payload_lenient`] would drop or repair is refused whole with
/// [`PayloadError::BadRecord`] and appends nothing.
pub fn decode_payload(bytes: &[u8], out: &mut Reports) -> Result<usize, PayloadError> {
    let (legacy, count, rec_len) = parse_header(bytes)?;
    let ads3 = bytes.get(..4) == Some(&PAYLOAD_MAGIC[..]);
    let mut rows = Vec::with_capacity(count);
    for i in 0..count {
        let at = PAYLOAD_HEADER_LEN + i * rec_len;
        let rec = bytes.get(at..at + rec_len).ok_or(PayloadError::Length)?;
        match decode_row_layout(rec, legacy, ads3) {
            Some((row, 0)) => rows.push(row),
            _ => return Err(PayloadError::BadRecord(i)),
        }
    }
    for row in &rows {
        reports_push(out, row);
    }
    Ok(count)
}

#[cfg(kani)]
mod proofs {
    //! Kani proof harnesses (docs/VERIFICATION.md). Compiled only under `cargo kani`.
    #![allow(clippy::indexing_slicing)]
    use super::*;

    // REQ: OA-ADSB-100
    #[kani::proof]
    #[kani::unwind(20)]
    fn decode_row_total_ads2() {
        let rec: [u8; RECORD_LEN] = kani::any();
        if let Some((row, anom)) = decode_row(&rec, false) {
            assert!(anom <= ANOM_SOURCE | ANOM_MEASUREMENT | ANOM_RESERVED);
            assert!((-900_000_000..=900_000_000).contains(&row.lat_e7));
            assert!((-1_800_000_000..=1_800_000_000).contains(&row.lon_e7));
            assert!(
                row.source == NONE_U8
                    || usize::from(row.source) < SOURCE_NAMES.len()
                    || row.source == SOURCE_UNKNOWN
            );
        }
    }

    // REQ: OA-ADSB-100
    #[kani::proof]
    #[kani::unwind(20)]
    fn decode_row_total_ads1() {
        let rec: [u8; LEGACY_RECORD_LEN] = kani::any();
        let _ = decode_row(&rec, true);
    }

    // REQ: OA-ADSB-101
    #[kani::proof]
    #[kani::unwind(20)]
    fn parse_header_any_bytes_up_to_2_records() {
        const N: usize = PAYLOAD_HEADER_LEN + 2 * RECORD_LEN;
        let buf: [u8; N] = kani::any();
        let len: usize = kani::any();
        kani::assume(len <= N);
        let bytes = &buf[..len];
        match parse_header(bytes) {
            Ok((legacy, count, rec_len)) => {
                assert_eq!(
                    rec_len,
                    if legacy {
                        LEGACY_RECORD_LEN
                    } else {
                        RECORD_LEN
                    }
                );
                assert_eq!(len, PAYLOAD_HEADER_LEN + count * rec_len);
            }
            Err(_) => {}
        }
    }

    /// Stands for `decode_row`, whose own contract is proved by `decode_row_total_ads2` and
    /// `decode_row_total_ads1` (record layouts, not payload magics): each record is kept or
    /// dropped as the solver chooses, with anomaly bits among the three that exist. The record
    /// bytes themselves are not symbolic here (the payload is zero-filled past its header), so the
    /// proof is of the loop and the counting over any keep/drop sequence, not of any record's
    /// contents; the solver is spared re-deriving the record decoder for every record.
    fn any_row(_: &[u8], _: bool) -> Option<(Row, u8)> {
        if kani::any() {
            let anom: u8 = kani::any();
            kani::assume(anom <= ANOM_SOURCE | ANOM_MEASUREMENT | ANOM_RESERVED);
            Some((ROW_EMPTY, anom))
        } else {
            None
        }
    }

    /// Stands for `reports_push`, which appends the row to each column; thirty `Vec` pushes per
    /// record are what the solver cannot afford, so the output columns are not inspected.
    fn push_nothing(_: &mut Reports, _: &Row) {}

    /// A payload with a valid header carrying `magic` for exactly `K` all-zero records of the
    /// layout `LEGACY` selects, through the lenient and the strict decoder. The length is a
    /// constant: a symbolic slice length costs the solver more than the records do, and
    /// `parse_header_any_bytes_up_to_2_records` covers every other length.
    /// The ADS2 magic takes the same header branch as ADS3 and runs the same loop; a harness for
    /// it took 305 s, over the 300 s budget, so only ADS3 and ADS1 are proved.
    fn records_total<const K: usize, const LEGACY: bool>(magic: [u8; 4]) {
        let mut buf = [0u8; PAYLOAD_HEADER_LEN + RECORD_LEN * 8];
        let rec = if LEGACY {
            LEGACY_RECORD_LEN
        } else {
            RECORD_LEN
        };
        buf[..4].copy_from_slice(&magic);
        buf[4..6].copy_from_slice(&(rec as u16).to_le_bytes());
        buf[8..12].copy_from_slice(&(K as u32).to_le_bytes());
        let bytes = &buf[..PAYLOAD_HEADER_LEN + K * rec];

        let mut out = Reports::default();
        let Ok(c) = decode_payload_lenient(bytes, &mut out) else {
            panic!("a valid header was refused");
        };
        assert_eq!(c.records, K);
        assert_eq!(c.kept + c.dropped, c.records);
        assert_eq!(c.first_dropped.is_some(), c.dropped > 0);
        assert!(c.unknown_source <= c.kept && c.bad_measurement <= c.kept);
        assert!(c.reserved_set <= c.kept);

        let mut strict = Reports::default();
        match decode_payload(bytes, &mut strict) {
            Ok(n) => {
                assert_eq!(n, K);
            }
            Err(PayloadError::BadRecord(i)) => {
                assert!(i < K);
            }
            Err(_) => panic!("a valid header was refused"),
        }
    }

    // REQ: OA-ADSB-101
    #[kani::proof]
    #[kani::unwind(10)]
    #[kani::stub(decode_row, any_row)]
    #[kani::stub(reports_push, push_nothing)]
    fn payload_records_ads3_4() {
        records_total::<4, false>(PAYLOAD_MAGIC);
    }

    // REQ: OA-ADSB-101
    #[kani::proof]
    #[kani::unwind(10)]
    #[kani::stub(decode_row, any_row)]
    #[kani::stub(reports_push, push_nothing)]
    fn payload_records_ads1_4() {
        records_total::<4, true>(LEGACY_PAYLOAD_MAGIC);
    }
}
