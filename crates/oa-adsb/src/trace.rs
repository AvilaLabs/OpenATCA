//! Decoding one trace file (JSON text) into reports. Core (pure).

use std::fmt;

use oa_json::{Doc, Kind, NodeIx};

use crate::{
    reports_push, Filter, Reports, Row, Stats, F_GROUND, MAX_OBJECT_MEMBERS, MAX_ROW_LEN,
    MAX_TRACE_NODES, MAX_TRACE_ROWS, NON_ICAO, REJ_ALTITUDE, REJ_FLAGS, REJ_OBJECT, REJ_POSITION,
    REJ_RATE, REJ_SHAPE, REJ_SOURCE, REJ_SPEED, REJ_TIME, REJ_TRACK, ROW_EMPTY, SIL_PER_HOUR,
    SIL_PER_SAMPLE, SIL_UNKNOWN, SOURCE_NAMES,
};

/// Why a whole trace file was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TraceError {
    /// The JSON is malformed or over an `oa-json` limit.
    Json(oa_json::JsonError),
    /// The top level is not an object.
    NotObject,
    /// A required key is missing or has the wrong type.
    Field(&'static str),
    /// The `icao` string is not six hex digits with an optional `~`.
    BadIcao,
    /// The base `timestamp` is outside 2000 to 2100.
    BadTimestamp,
    /// More than [`MAX_TRACE_ROWS`] rows.
    TooManyRows(usize),
}

impl fmt::Display for TraceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TraceError::Json(e) => write!(f, "trace: {e}"),
            TraceError::NotObject => write!(f, "trace: top level is not an object"),
            TraceError::Field(k) => write!(f, "trace: missing or malformed `{k}`"),
            TraceError::BadIcao => write!(f, "trace: `icao` is not a hex address"),
            TraceError::BadTimestamp => write!(f, "trace: `timestamp` out of range"),
            TraceError::TooManyRows(n) => {
                write!(f, "trace: {n} rows exceeds the limit of {MAX_TRACE_ROWS}")
            }
        }
    }
}

impl std::error::Error for TraceError {}

/// 2000-01-01 and 2100-01-01 in Unix seconds: the range a base timestamp must fall in.
const TS_MIN: f64 = 946_684_800.0;
const TS_MAX: f64 = 4_102_444_800.0;
/// Altitudes and rates are stored by readsb in 16 bits with factors 6.25 ft and 8 fpm
/// (`track.h`), so anything the writer prints is within these.
const ALT_LIMIT: i64 = 204_800;
const RATE_LIMIT: i64 = 262_144;
/// Rows are at most this many seconds from the base (the writer's day files span 24 h).
const DT_MAX: f64 = 4_000_000.0;

fn hex_val(b: u8) -> Option<u32> {
    match b {
        b'0'..=b'9' => Some(u32::from(b - b'0')),
        b'a'..=b'f' => Some(u32::from(b - b'a') + 10),
        b'A'..=b'F' => Some(u32::from(b - b'A') + 10),
        _ => None,
    }
}

fn parse_icao(s: &str) -> Option<u32> {
    let (non, digits) = match s.strip_prefix('~') {
        Some(rest) => (true, rest),
        None => (false, s),
    };
    if digits.len() != 6 {
        return None;
    }
    let mut v = 0u32;
    for b in digits.bytes() {
        v = v * 16 + hex_val(b)?;
    }
    Some(if non { v | NON_ICAO } else { v })
}

/// A value that is `null`, or a number: `Ok(None)` for null, `Err` for any other kind.
fn opt_f64(doc: &Doc<'_>, n: Option<NodeIx>) -> Result<Option<f64>, ()> {
    let Some(n) = n else { return Ok(None) };
    match doc.kind(n) {
        Some(Kind::Null) => Ok(None),
        Some(Kind::Number) => doc.as_f64(n).map(Some).ok_or(()),
        _ => Err(()),
    }
}

/// A value that is `null` or an integer within `[lo, hi]`.
fn opt_int(doc: &Doc<'_>, n: Option<NodeIx>, lo: i64, hi: i64) -> Result<Option<i64>, ()> {
    let Some(n) = n else { return Ok(None) };
    match doc.kind(n) {
        Some(Kind::Null) => Ok(None),
        Some(Kind::Number) => match doc.as_i64(n) {
            Some(v) if (lo..=hi).contains(&v) => Ok(Some(v)),
            _ => Err(()),
        },
        _ => Err(()),
    }
}

/// Fixed-point conversion: `v * scale` rounded to nearest. `v` is already range checked.
fn fixed(v: f64, scale: f64) -> i64 {
    (v * scale).round() as i64
}

/// Fields of the per-row aircraft object that we keep. Ranges are the widths of the ADS-B bit
/// fields readsb copies through (`nac_p` four bits, `nac_v` three, `sil`, `gva` and `sda` two);
/// `nic` is readsb's computed 0 to 11. Real data carries `nac_v` up to 7, so the defined 0 to 4
/// is not the bound.
fn read_object(doc: &Doc<'_>, obj: NodeIx, row: &mut Row) -> Result<(), ()> {
    if doc.len(obj) > MAX_OBJECT_MEMBERS {
        return Err(());
    }
    for (k, v) in doc.members(obj) {
        let Some(key) = doc.str(k) else {
            return Err(());
        };
        let small = |max: i64| -> Result<u8, ()> {
            match opt_int(doc, Some(v), 0, max)? {
                Some(x) => Ok(x as u8),
                None => Err(()),
            }
        };
        match key {
            "flight" => {
                let s = doc.str(v).ok_or(())?.trim_end_matches(' ');
                if s.len() > 8 || !s.bytes().all(|b| (0x20..=0x7e).contains(&b)) {
                    return Err(());
                }
                row.callsign = [0; 8];
                for (d, b) in row.callsign.iter_mut().zip(s.bytes()) {
                    *d = b;
                }
            }
            "squawk" => {
                let s = doc.str(v).ok_or(())?;
                if s.len() != 4 || !s.bytes().all(|b| (b'0'..=b'7').contains(&b)) {
                    return Err(());
                }
                row.squawk = s.parse().map_err(|_| ())?;
            }
            "category" => {
                let s = doc.str(v).ok_or(())?.as_bytes();
                let (Some(&a), Some(&b), 2) = (s.first(), s.get(1), s.len()) else {
                    return Err(());
                };
                let c = (hex_val(a).ok_or(())? << 4) | hex_val(b).ok_or(())?;
                row.category = c as u8;
            }
            "nic" => row.nic = small(11)?,
            "nac_p" => row.nac_p = small(15)?,
            "nac_v" => row.nac_v = small(7)?,
            "sil" => row.sil = small(3)?,
            "gva" => row.gva = small(3)?,
            "sda" => row.sda = small(3)?,
            "version" => row.version = small(7)?,
            "sil_type" => {
                row.sil_type = match doc.str(v).ok_or(())? {
                    "unknown" => SIL_UNKNOWN,
                    "perhour" => SIL_PER_HOUR,
                    "persample" => SIL_PER_SAMPLE,
                    _ => return Err(()),
                };
            }
            _ => {}
        }
    }
    Ok(())
}

/// Reasons a row fails after its position and time were accepted.
fn decode_rest(doc: &Doc<'_>, cols: &[NodeIx], row: &mut Row) -> Result<(), usize> {
    let col = |i: usize| cols.get(i).copied();
    // Flags first: they say what the altitude and rate columns mean.
    let flags = match opt_int(doc, col(6), 0, 15) {
        Ok(Some(f)) => f as u8,
        _ => return Err(REJ_FLAGS),
    };
    row.flags = flags;
    // Altitude: an integer, "ground", or null.
    let mut alt = None;
    match col(3) {
        Some(n) if doc.kind(n) == Some(Kind::String) || doc.kind(n) == Some(Kind::StringPooled) => {
            if doc.str(n) != Some("ground") {
                return Err(REJ_ALTITUDE);
            }
            row.flags |= F_GROUND;
        }
        n => match opt_int(doc, n, -ALT_LIMIT, ALT_LIMIT) {
            Ok(a) => alt = a,
            Err(()) => return Err(REJ_ALTITUDE),
        },
    }
    let geom = match opt_int(doc, col(10), -ALT_LIMIT, ALT_LIMIT) {
        Ok(g) => g,
        Err(()) => return Err(REJ_ALTITUDE),
    };
    if let Some(a) = alt {
        if flags & crate::F_ALT_GEOM != 0 {
            row.geom_alt_ft = a as i32;
        } else {
            row.baro_alt_ft = a as i32;
        }
    }
    if let Some(g) = geom {
        row.geom_alt_ft = g as i32;
    }
    match opt_f64(doc, col(4)) {
        Ok(Some(g)) if (0.0..=6000.0).contains(&g) => row.gs_dkt = fixed(g, 10.0) as u16,
        Ok(None) => {}
        _ => return Err(REJ_SPEED),
    }
    match opt_f64(doc, col(5)) {
        Ok(Some(t)) if (0.0..=360.0).contains(&t) => {
            row.track_ddeg = (fixed(t, 10.0) % 3600) as u16
        }
        Ok(None) => {}
        _ => return Err(REJ_TRACK),
    }
    match opt_int(doc, col(7), -RATE_LIMIT, RATE_LIMIT) {
        Ok(Some(r)) => row.vrate_fpm = r as i32,
        Ok(None) => {}
        Err(()) => return Err(REJ_RATE),
    }
    // Source type: absent in files before 2022.
    if let Some(n) = col(9) {
        match doc.kind(n) {
            Some(Kind::Null) => {}
            Some(Kind::String | Kind::StringPooled) => {
                let s = doc.str(n).unwrap_or("");
                match SOURCE_NAMES.iter().position(|&name| name == s) {
                    Some(i) => row.source = i as u8,
                    None => return Err(REJ_SOURCE),
                }
            }
            _ => return Err(REJ_SOURCE),
        }
    }
    if let Some(n) = col(8) {
        match doc.kind(n) {
            Some(Kind::Null) => {}
            Some(Kind::Object) => read_object(doc, n, row).map_err(|()| REJ_OBJECT)?,
            _ => return Err(REJ_OBJECT),
        }
    }
    Ok(())
}

/// Counts a refused row under `why` (an index into [`REJECT_NAMES`](crate::REJECT_NAMES)).
fn reject(st: &mut Stats, why: usize) {
    if let Some(c) = st.rejected.get_mut(why) {
        *c += 1;
    }
}

/// Decodes the trace file `json`, appending the reports inside `filter` to `out` and counting
/// every row in `stats`. A structural error refuses the whole trace (nothing is appended for
/// it, `stats` is untouched); a bad row is counted and skipped.
///
/// Rows outside the box or the time window are counted as such after their time and position
/// are validated; the rest of such a row is not examined.
pub fn decode_trace(
    json: &[u8],
    filter: &Filter,
    out: &mut Reports,
    stats: &mut Stats,
) -> Result<(), TraceError> {
    let limits = oa_json::Limits {
        max_nodes: MAX_TRACE_NODES,
        ..oa_json::DEFAULT_LIMITS
    };
    let doc = oa_json::parse(json, &limits).map_err(TraceError::Json)?;
    let root = NodeIx::ROOT;
    if doc.kind(root) != Some(Kind::Object) {
        return Err(TraceError::NotObject);
    }
    let icao = doc
        .get(root, "icao")
        .and_then(|n| doc.str(n))
        .ok_or(TraceError::Field("icao"))
        .and_then(|s| parse_icao(s).ok_or(TraceError::BadIcao))?;
    let base = doc
        .get(root, "timestamp")
        .and_then(|n| doc.as_f64(n))
        .ok_or(TraceError::Field("timestamp"))?;
    if !(TS_MIN..=TS_MAX).contains(&base) {
        return Err(TraceError::BadTimestamp);
    }
    let trace = doc
        .get(root, "trace")
        .filter(|&n| doc.kind(n) == Some(Kind::Array))
        .ok_or(TraceError::Field("trace"))?;
    let rows = doc.len(trace);
    if rows > MAX_TRACE_ROWS {
        return Err(TraceError::TooManyRows(rows));
    }
    let base_ms = fixed(base, 1000.0);
    let mut local = Stats::default();
    let mut cols = [NodeIx::NONE; MAX_ROW_LEN];
    for rown in doc.elements(trace) {
        local.rows_seen += 1;
        // Gather the row's columns.
        let ncol = if doc.kind(rown) == Some(Kind::Array) {
            doc.len(rown)
        } else {
            0
        };
        if !(7..=MAX_ROW_LEN).contains(&ncol) {
            reject(&mut local, REJ_SHAPE);
            continue;
        }
        for (slot, c) in cols.iter_mut().zip(doc.elements(rown)) {
            *slot = c;
        }
        let cols = cols.get(..ncol).unwrap_or(&[]);
        // Time, latitude, longitude: needed to filter.
        let dt = cols.first().and_then(|&n| doc.as_f64(n));
        let Some(dt) = dt.filter(|d| d.abs() <= DT_MAX) else {
            reject(&mut local, REJ_TIME);
            continue;
        };
        let time_ms = base_ms + fixed(dt, 1000.0);
        let lat = cols.get(1).and_then(|&n| doc.as_f64(n));
        let lon = cols.get(2).and_then(|&n| doc.as_f64(n));
        let (Some(lat), Some(lon)) = (lat, lon) else {
            reject(&mut local, REJ_POSITION);
            continue;
        };
        if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
            reject(&mut local, REJ_POSITION);
            continue;
        }
        let (lat_e7, lon_e7) = (fixed(lat, 1e7) as i32, fixed(lon, 1e7) as i32);
        if lat_e7 < filter.lat_min_e7
            || lat_e7 > filter.lat_max_e7
            || lon_e7 < filter.lon_min_e7
            || lon_e7 > filter.lon_max_e7
        {
            local.outside_box += 1;
            continue;
        }
        let time_ns = time_ms.saturating_mul(1_000_000);
        if time_ns < filter.from_ns || time_ns >= filter.to_ns {
            local.outside_time += 1;
            continue;
        }
        let mut row = ROW_EMPTY;
        row.time_ns = time_ns;
        row.icao = icao;
        row.lat_e7 = lat_e7;
        row.lon_e7 = lon_e7;
        if let Err(why) = decode_rest(&doc, cols, &mut row) {
            reject(&mut local, why);
            continue;
        }
        local.kept += 1;
        reports_push(out, &row);
    }
    crate::stats_add(stats, &local);
    Ok(())
}
