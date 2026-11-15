//! Decoding one trace file (JSON text) into reports. Core (pure).

use std::fmt;

use oa_json::{Doc, Kind, NodeIx};

use crate::{
    is_reserved_address, reports_push, stats_note_unknown, Filter, Reports, Row, Stats,
    FLD_ALTITUDE, FLD_CATEGORY, FLD_FLAGS, FLD_FLIGHT, FLD_GEOM_ALT, FLD_GVA, FLD_NAC_P, FLD_NAC_V,
    FLD_NIC, FLD_OBJECT, FLD_RATE, FLD_SDA, FLD_SIL, FLD_SIL_TYPE, FLD_SOURCE, FLD_SPEED,
    FLD_SQUAWK, FLD_TRACK, FLD_VERSION, F_GROUND, MAX_OBJECT_MEMBERS, MAX_ROW_LEN, MAX_TRACE_NODES,
    MAX_TRACE_ROWS, NONE_I16, NONE_U16, NONE_U8, NON_ICAO, REJ_FLAGS, REJ_POSITION, REJ_SHAPE,
    REJ_TIME, ROW_EMPTY, SIL_PER_HOUR, SIL_PER_SAMPLE, SIL_UNKNOWN, SOURCE_NAMES, SOURCE_UNKNOWN,
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

/// An optional measurement: `Ok(None)` for null, `Ok(Some(x))` for a finite number in
/// `[lo, hi]`, `Err` for anything else.
fn measure(doc: &Doc<'_>, n: NodeIx, lo: f64, hi: f64) -> Result<Option<f64>, ()> {
    match doc.kind(n) {
        Some(Kind::Null) => Ok(None),
        Some(Kind::Number) => match doc.as_f64(n) {
            Some(x) if (lo..=hi).contains(&x) => Ok(Some(x)),
            _ => Err(()),
        },
        _ => Err(()),
    }
}

/// Stores the measurement `key` of an aircraft object (or an `ias`/`roll` row column) in the
/// row, converted to fixed point. A malformed or out-of-range value is left absent and counted
/// in `bad`: the position it came with is still good. Returns false for an unknown key.
fn read_measurement(doc: &Doc<'_>, key: &str, v: NodeIx, row: &mut Row, bad: &mut u64) -> bool {
    let u16_of = |lo: f64, hi: f64, scale: f64, bad: &mut u64| -> u16 {
        match measure(doc, v, lo, hi) {
            Ok(Some(x)) => fixed(x, scale) as u16,
            Ok(None) => NONE_U16,
            Err(()) => {
                *bad += 1;
                NONE_U16
            }
        }
    };
    let i16_of = |lo: f64, hi: f64, scale: f64, bad: &mut u64| -> i16 {
        match measure(doc, v, lo, hi) {
            Ok(Some(x)) => fixed(x, scale) as i16,
            Ok(None) => NONE_I16,
            Err(()) => {
                *bad += 1;
                NONE_I16
            }
        }
    };
    match key {
        "ws" => row.ws_kt = u16_of(0.0, 1000.0, 1.0, bad),
        // `%.0f` can print 360 for a direction just under north.
        "wd" => row.wd_deg = wrap(u16_of(0.0, 360.0, 1.0, bad), 360),
        "tas" => row.tas_kt = u16_of(0.0, 2000.0, 1.0, bad),
        "ias" => row.ias_kt = u16_of(0.0, 2000.0, 1.0, bad),
        "mach" => row.mach_milli = u16_of(0.0, 5.0, 1000.0, bad),
        "mag_heading" => row.mag_heading_cdeg = wrap(u16_of(0.0, 360.0, 100.0, bad), 36_000),
        "true_heading" => row.true_heading_cdeg = wrap(u16_of(0.0, 360.0, 100.0, bad), 36_000),
        "oat" => row.oat_c = i16_of(-200.0, 200.0, 1.0, bad),
        "tat" => row.tat_c = i16_of(-200.0, 200.0, 1.0, bad),
        "roll" => row.roll_cdeg = i16_of(-180.0, 180.0, 100.0, bad),
        "track_rate" => row.track_rate_cdps = i16_of(-300.0, 300.0, 100.0, bad),
        _ => return false,
    }
    true
}

/// An angle wrapped into `[0, modulus)` (360 becomes 0); absent stays absent.
fn wrap(v: u16, modulus: u16) -> u16 {
    if v == NONE_U16 {
        v
    } else {
        v % modulus
    }
}

/// The value of `n` as short text for the unknown-value counts: a string as it is, a number as
/// it prints, `null`, or the kind of anything else.
fn value_text(doc: &Doc<'_>, n: NodeIx) -> String {
    match doc.kind(n) {
        Some(Kind::Null) => "null".to_string(),
        Some(Kind::True) => "true".to_string(),
        Some(Kind::False) => "false".to_string(),
        Some(Kind::Number) => doc
            .as_f64(n)
            .map_or_else(|| "number".to_string(), |x| format!("{x}")),
        Some(Kind::String | Kind::StringPooled) => doc.str(n).unwrap_or("").to_string(),
        Some(Kind::Array) => "<array>".to_string(),
        Some(Kind::Object) => "<object>".to_string(),
        None => "<none>".to_string(),
    }
}

/// Counts the value at `n` as unknown for `field`.
fn note(st: &mut Stats, field: usize, doc: &Doc<'_>, n: NodeIx) {
    stats_note_unknown(st, field, &value_text(doc, n));
}

/// A small integer field of the aircraft object: the value if it is an integer in `0..=max`,
/// else [`NONE_U8`] with the value counted (a null is counted as `null`).
fn small_field(doc: &Doc<'_>, v: NodeIx, max: i64, field: usize, st: &mut Stats) -> u8 {
    match opt_int(doc, Some(v), 0, max) {
        Ok(Some(x)) => x as u8,
        _ => {
            note(st, field, doc, v);
            NONE_U8
        }
    }
}

/// Fields of the per-row aircraft object that we keep. Ranges are the widths of the ADS-B bit
/// fields readsb copies through (`nac_p` four bits, `nac_v` three, `sil`, `gva` and `sda` two);
/// `nic` is readsb's computed 0 to 11. Real data carries `nac_v` up to 7, so the defined 0 to 4
/// is not the bound. A field that is null, malformed or an unknown enumeration value is left
/// absent (`unknown` for `sil_type`) and counted in `st`: none of them is needed to know what the
/// position means, and the row's position is good. An object over the member limit is ignored
/// whole and counted.
fn read_object(doc: &Doc<'_>, obj: NodeIx, row: &mut Row, bad: &mut u64, st: &mut Stats) {
    if doc.len(obj) > MAX_OBJECT_MEMBERS {
        note(st, FLD_OBJECT, doc, obj);
        return;
    }
    for (k, v) in doc.members(obj) {
        let Some(key) = doc.str(k) else {
            continue;
        };
        if read_measurement(doc, key, v, row, bad) {
            continue;
        }
        match key {
            "flight" => {
                let s = doc.str(v).map(|s| s.trim_end_matches(' '));
                match s {
                    Some(s) if s.len() <= 8 && s.bytes().all(|b| (0x20..=0x7e).contains(&b)) => {
                        row.callsign = [0; 8];
                        for (d, b) in row.callsign.iter_mut().zip(s.bytes()) {
                            *d = b;
                        }
                    }
                    _ => note(st, FLD_FLIGHT, doc, v),
                }
            }
            "squawk" => match doc.str(v) {
                Some(s) if s.len() == 4 && s.bytes().all(|b| (b'0'..=b'7').contains(&b)) => {
                    row.squawk = s.parse().unwrap_or(NONE_U16);
                }
                _ => note(st, FLD_SQUAWK, doc, v),
            },
            "category" => {
                let c = doc.str(v).and_then(|s| {
                    let s = s.as_bytes();
                    match (s.first(), s.get(1), s.len()) {
                        (Some(&a), Some(&b), 2) => Some((hex_val(a)? << 4) | hex_val(b)?),
                        _ => None,
                    }
                });
                match c {
                    Some(c) => row.category = c as u8,
                    None => note(st, FLD_CATEGORY, doc, v),
                }
            }
            "nic" => row.nic = small_field(doc, v, 11, FLD_NIC, st),
            "nac_p" => row.nac_p = small_field(doc, v, 15, FLD_NAC_P, st),
            "nac_v" => row.nac_v = small_field(doc, v, 7, FLD_NAC_V, st),
            "sil" => row.sil = small_field(doc, v, 3, FLD_SIL, st),
            "gva" => row.gva = small_field(doc, v, 3, FLD_GVA, st),
            "sda" => row.sda = small_field(doc, v, 3, FLD_SDA, st),
            "version" => row.version = small_field(doc, v, 7, FLD_VERSION, st),
            "sil_type" => match doc.str(v) {
                Some("unknown") => row.sil_type = SIL_UNKNOWN,
                Some("perhour") => row.sil_type = SIL_PER_HOUR,
                Some("persample") => row.sil_type = SIL_PER_SAMPLE,
                // A null says nothing; any other value is a name this reader does not know.
                _ => {
                    if doc.kind(v) != Some(Kind::Null) {
                        row.sil_type = SIL_UNKNOWN;
                    }
                    note(st, FLD_SIL_TYPE, doc, v);
                }
            },
            _ => {}
        }
    }
}

/// Reads the columns after position and time. Only the flags can refuse the row ([`REJ_FLAGS`]);
/// every other malformed or unknown value is left absent (`unknown` for the source type) and
/// counted in `st`.
fn decode_rest(
    doc: &Doc<'_>,
    cols: &[NodeIx],
    row: &mut Row,
    bad: &mut u64,
    st: &mut Stats,
) -> Result<(), usize> {
    let col = |i: usize| cols.get(i).copied();
    // Flags first: they say what the altitude and rate columns mean, and whether the position is
    // current (stale) and continuous (new leg). Bits above the four readsb defines are cleared
    // and counted: they cannot change what the four mean.
    let flags = match (col(6), opt_int(doc, col(6), 0, 255)) {
        (Some(n), Ok(Some(f))) => {
            if f > 15 {
                note(st, FLD_FLAGS, doc, n);
            }
            (f & 15) as u8
        }
        _ => return Err(REJ_FLAGS),
    };
    row.flags = flags;
    // Altitude: an integer, "ground", or null.
    let mut alt = None;
    match col(3) {
        Some(n) if doc.kind(n) == Some(Kind::String) || doc.kind(n) == Some(Kind::StringPooled) => {
            if doc.str(n) == Some("ground") {
                row.flags |= F_GROUND;
            } else {
                note(st, FLD_ALTITUDE, doc, n);
            }
        }
        Some(n) => match opt_int(doc, Some(n), -ALT_LIMIT, ALT_LIMIT) {
            Ok(a) => alt = a,
            Err(()) => note(st, FLD_ALTITUDE, doc, n),
        },
        None => {}
    }
    let geom = match col(10) {
        Some(n) => match opt_int(doc, Some(n), -ALT_LIMIT, ALT_LIMIT) {
            Ok(g) => g,
            Err(()) => {
                note(st, FLD_GEOM_ALT, doc, n);
                None
            }
        },
        None => None,
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
    if let Some(n) = col(4) {
        match opt_f64(doc, Some(n)) {
            Ok(Some(g)) if (0.0..=6000.0).contains(&g) => row.gs_dkt = fixed(g, 10.0) as u16,
            Ok(None) => {}
            _ => note(st, FLD_SPEED, doc, n),
        }
    }
    if let Some(n) = col(5) {
        match opt_f64(doc, Some(n)) {
            Ok(Some(t)) if (0.0..=360.0).contains(&t) => {
                row.track_ddeg = (fixed(t, 10.0) % 3600) as u16
            }
            Ok(None) => {}
            _ => note(st, FLD_TRACK, doc, n),
        }
    }
    if let Some(n) = col(7) {
        match opt_int(doc, Some(n), -RATE_LIMIT, RATE_LIMIT) {
            Ok(Some(r)) => row.vrate_fpm = r as i32,
            Ok(None) => {}
            Err(()) => note(st, FLD_RATE, doc, n),
        }
    }
    // Source type: absent in files before 2022. A name this reader does not know is the
    // `unknown` source (which truth selection does not take as ADS-B).
    if let Some(n) = col(9) {
        match doc.kind(n) {
            Some(Kind::Null) => {}
            Some(Kind::String | Kind::StringPooled) => {
                let s = doc.str(n).unwrap_or("");
                match SOURCE_NAMES.iter().position(|&name| name == s) {
                    Some(i) => row.source = i as u8,
                    None => {
                        row.source = SOURCE_UNKNOWN;
                        note(st, FLD_SOURCE, doc, n);
                    }
                }
            }
            _ => {
                row.source = SOURCE_UNKNOWN;
                note(st, FLD_SOURCE, doc, n);
            }
        }
    }
    // Indicated airspeed and roll are columns of every row; the aircraft object, when present,
    // repeats them with more digits and overrides them below.
    if let Some(n) = col(12) {
        read_measurement(doc, "ias", n, row, bad);
    }
    if let Some(n) = col(13) {
        read_measurement(doc, "roll", n, row, bad);
    }
    if let Some(n) = col(8) {
        match doc.kind(n) {
            Some(Kind::Null) => {}
            Some(Kind::Object) => read_object(doc, n, row, bad, st),
            _ => note(st, FLD_OBJECT, doc, n),
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
/// it, `stats` is untouched); a row whose time, position, shape or flags cannot be read is
/// counted and skipped; any other unknown or malformed field is counted and the row is kept.
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
    // A reserved address (not flagged non-ICAO) is not an aircraft: count the rows, output none.
    if is_reserved_address(icao) {
        local.rows_seen = rows as u64;
        local.reserved_address = rows as u64;
        crate::stats_add(stats, &local);
        return Ok(());
    }
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
        let mut bad = 0;
        if let Err(why) = decode_rest(&doc, cols, &mut row, &mut bad, &mut local) {
            reject(&mut local, why);
            continue;
        }
        local.kept += 1;
        local.bad_measurements += u64::from(bad > 0);
        reports_push(out, &row);
    }
    crate::stats_add(stats, &local);
    Ok(())
}
