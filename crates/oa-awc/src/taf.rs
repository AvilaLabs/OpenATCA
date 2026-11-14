//! Terminal forecasts (`/api/data/taf?format=json`): one header row per issued forecast and one
//! period row per forecast group. A snapshot of the forecasts current at the fetch time.
//!
//! * Wind direction is degrees, [`crate::WDIR_VRB`] for variable; speed and gust are knots.
//! * Visibility is hundredths of a statute mile; `"6+"` is 600 with [`FLAG_VISIB_PLUS`]; an
//!   empty text means "not given" (`NONE_U16`).
//! * The ceiling is the lowest `BKN` or `OVC` base in feet above ground (`NO_ALT` if none).
//! * The API's TAF `altim` and temperature fields are null throughout the captured data and are
//!   not read.
//! * De-duplication key: (station, issue time, valid from); the copy with the greatest `rawTAF`
//!   text wins. Counted.

use crate::error::{count_refusal, refuse, ParseError, Reason, Refusal, RefusedCounts};
use crate::json::{
    opt_array, opt_int, opt_str, req_array, req_f64, req_int, req_str, require_object, visib,
};
// The period flag bit for "visibility or more" is the METAR one.
pub use crate::metar::FLAG_VISIB_PLUS;
use crate::metar::{deg_e5, WDIR_VRB};
use crate::pool::{new_pool, pool_get, pool_push, pool_select, select, top_array, Pool, StrIx};
use crate::units::{pack_ident, parse_iso_ms, MAX_TIME_S, MIN_TIME_S, NONE_U16, NO_ALT};
use crate::{MAX_RAW_BYTES, MAX_RECORDS_PER_FILE, MAX_TABLE_ROWS};
use oa_json::{parse, Doc, Kind, NodeIx, DEFAULT_LIMITS};

/// `change` codes of a period: 0 base group, then these words.
pub const CHANGE_NAMES: [&str; 5] = ["", "FM", "TEMPO", "BECMG", "PROB"];
/// Bound: periods in one forecast.
pub const MAX_PERIODS: usize = 64;

/// Forecast headers and periods; period row ranges come from `period_first`.
// A7.2: cold struct (one per load; the columns are the hot data), size guard waived
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TafTable {
    pub station: Vec<u32>,
    pub issue_s: Vec<i64>,
    pub valid_from_s: Vec<i64>,
    pub valid_to_s: Vec<i64>,
    pub lat_e5: Vec<i32>,
    pub lon_e5: Vec<i32>,
    pub elev_m: Vec<i16>,
    /// Forecast `i` owns periods `period_first[i]..period_first[i + 1]`.
    pub period_first: Vec<u32>,
    pub raw: Pool,
    pub p_from_s: Vec<i64>,
    pub p_to_s: Vec<i64>,
    pub p_change: Vec<u8>,
    pub p_prob: Vec<u8>,
    pub p_wdir_deg: Vec<u16>,
    pub p_wspd_kt: Vec<u16>,
    pub p_wgst_kt: Vec<u16>,
    pub p_visib_sm100: Vec<u16>,
    pub p_ceiling_ft: Vec<i32>,
    pub p_flags: Vec<u8>,
}

/// Counts for the TAF files parsed so far.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TafStats {
    pub files: u32,
    pub records: u32,
    pub accepted: u32,
    pub refused: RefusedCounts,
}

/// What [`finalize_taf`] removed.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TafDedupe {
    pub rows_in: u32,
    pub rows_kept: u32,
    pub duplicates: u32,
}

/// An empty table.
pub fn new_taf_table() -> TafTable {
    TafTable {
        station: Vec::new(),
        issue_s: Vec::new(),
        valid_from_s: Vec::new(),
        valid_to_s: Vec::new(),
        lat_e5: Vec::new(),
        lon_e5: Vec::new(),
        elev_m: Vec::new(),
        period_first: vec![0],
        raw: new_pool(),
        p_from_s: Vec::new(),
        p_to_s: Vec::new(),
        p_change: Vec::new(),
        p_prob: Vec::new(),
        p_wdir_deg: Vec::new(),
        p_wspd_kt: Vec::new(),
        p_wgst_kt: Vec::new(),
        p_visib_sm100: Vec::new(),
        p_ceiling_ft: Vec::new(),
        p_flags: Vec::new(),
    }
}

/// Number of forecasts.
pub fn taf_len(t: &TafTable) -> usize {
    t.station.len()
}

/// The `rawTAF` text of forecast `i`.
pub fn taf_raw(t: &TafTable, i: usize) -> &str {
    pool_get(&t.raw, StrIx(i as u32))
}

/// The periods of forecast `i` as a range of period rows.
pub fn taf_periods(t: &TafTable, i: usize) -> std::ops::Range<usize> {
    let a = t.period_first.get(i).copied().unwrap_or(0) as usize;
    let b = t.period_first.get(i + 1).copied().unwrap_or(0) as usize;
    a..b
}

// A7.2: cold struct (a transient record), size guard waived
struct Period {
    from_s: i64,
    to_s: i64,
    change: u8,
    prob: u8,
    wdir: u16,
    wspd: u16,
    wgst: u16,
    visib: u16,
    ceiling: i32,
    flags: u8,
}

fn read_period(doc: &Doc<'_>, o: NodeIx) -> Result<Period, Refusal> {
    require_object(doc, o)?;
    let from_s = req_int(doc, o, "timeFrom", MIN_TIME_S, MAX_TIME_S)?;
    let to_s = req_int(doc, o, "timeTo", MIN_TIME_S, MAX_TIME_S)?;
    if to_s < from_s {
        return refuse(Reason::ReversedBand, "timeTo");
    }
    let change = match opt_str(doc, o, "fcstChange")? {
        None => 0,
        Some(s) => match CHANGE_NAMES.iter().position(|&n| n == s && !n.is_empty()) {
            Some(i) => i as u8,
            None => return refuse(Reason::UnknownCode, "fcstChange"),
        },
    };
    let prob = opt_int(doc, o, "probability", 0, 100)?.map_or(0, |v| v as u8);
    let wdir = match doc.get(o, "wdir").and_then(|n| Some((n, doc.kind(n)?))) {
        None | Some((_, Kind::Null)) => NONE_U16,
        Some((n, Kind::String)) => match doc.str(n) {
            Some("VRB") => WDIR_VRB,
            Some(s) => match s.parse::<u16>() {
                Ok(v) if v <= 360 => v,
                _ => return refuse(Reason::BadType, "wdir"),
            },
            None => return refuse(Reason::BadType, "wdir"),
        },
        Some((_, Kind::Number)) => opt_int(doc, o, "wdir", 0, 360)?.map_or(NONE_U16, |v| v as u16),
        Some(_) => return refuse(Reason::BadType, "wdir"),
    };
    let mut flags = 0u8;
    let visib = visib(doc, o, FLAG_VISIB_PLUS, &mut flags)?;
    let mut ceiling = NO_ALT;
    if let Some(arr) = opt_array(doc, o, "clouds")? {
        for c in doc.elements(arr) {
            require_object(doc, c)?;
            let covers = matches!(opt_str(doc, c, "cover")?, Some("BKN" | "OVC"));
            if let (true, Some(b)) = (covers, opt_int(doc, c, "base", 0, 100_000)?) {
                ceiling = if ceiling == NO_ALT {
                    b as i32
                } else {
                    ceiling.min(b as i32)
                };
            }
        }
    }
    Ok(Period {
        from_s,
        to_s,
        change,
        prob,
        wdir,
        wspd: opt_int(doc, o, "wspd", 0, 250)?.map_or(NONE_U16, |v| v as u16),
        wgst: opt_int(doc, o, "wgst", 0, 250)?.map_or(NONE_U16, |v| v as u16),
        visib,
        ceiling,
        flags,
    })
}

fn push_taf(t: &mut TafTable, doc: &Doc<'_>, o: NodeIx) -> Result<(), Refusal> {
    require_object(doc, o)?;
    let Some(station) = pack_ident(req_str(doc, o, "icaoId")?, 3) else {
        return refuse(Reason::BadIdent, "icaoId");
    };
    let Some(issue_ms) = parse_iso_ms(req_str(doc, o, "issueTime")?) else {
        return refuse(Reason::BadTime, "issueTime");
    };
    let from = req_int(doc, o, "validTimeFrom", MIN_TIME_S, MAX_TIME_S)?;
    let to = req_int(doc, o, "validTimeTo", MIN_TIME_S, MAX_TIME_S)?;
    if to < from {
        return refuse(Reason::ReversedBand, "validTimeTo");
    }
    let lat_e5 = deg_e5(req_f64(doc, o, "lat")?, 90.0, "lat")?;
    let lon_e5 = deg_e5(req_f64(doc, o, "lon")?, 180.0, "lon")?;
    let elev_m = req_int(doc, o, "elev", -500, 6000)? as i16;
    let raw = req_str(doc, o, "rawTAF")?;
    if raw.len() > MAX_RAW_BYTES {
        return refuse(Reason::OutOfRange, "rawTAF");
    }
    let fcsts = req_array(doc, o, "fcsts")?;
    let n = doc.len(fcsts);
    if n == 0 || n > MAX_PERIODS {
        return refuse(Reason::BadGeometry, "fcsts");
    }
    let periods: Vec<Period> = doc
        .elements(fcsts)
        .map(|p| read_period(doc, p))
        .collect::<Result<_, _>>()?;
    if pool_push(&mut t.raw, raw).is_none() {
        return refuse(Reason::OutOfRange, "rawTAF");
    }
    for p in &periods {
        t.p_from_s.push(p.from_s);
        t.p_to_s.push(p.to_s);
        t.p_change.push(p.change);
        t.p_prob.push(p.prob);
        t.p_wdir_deg.push(p.wdir);
        t.p_wspd_kt.push(p.wspd);
        t.p_wgst_kt.push(p.wgst);
        t.p_visib_sm100.push(p.visib);
        t.p_ceiling_ft.push(p.ceiling);
        t.p_flags.push(p.flags);
    }
    t.period_first.push(t.p_from_s.len() as u32);
    t.station.push(station);
    t.issue_s.push(issue_ms.div_euclid(1000));
    t.valid_from_s.push(from);
    t.valid_to_s.push(to);
    t.lat_e5.push(lat_e5);
    t.lon_e5.push(lon_e5);
    t.elev_m.push(elev_m);
    Ok(())
}

/// Parses one TAF JSON file and appends its accepted forecasts (unsorted). File-level and
/// per-record refusals work as in [`crate::parse_metar`].
pub fn parse_taf(
    input: &[u8],
    table: &mut TafTable,
    stats: &mut TafStats,
) -> Result<(), ParseError> {
    let doc = parse(input, &DEFAULT_LIMITS)?;
    let (arr, n) = top_array(&doc, taf_len(table), MAX_RECORDS_PER_FILE, MAX_TABLE_ROWS)?;
    stats.files += 1;
    stats.records = stats.records.saturating_add(n as u32);
    for o in doc.elements(arr) {
        match push_taf(table, &doc, o) {
            Ok(()) => stats.accepted += 1,
            Err(e) => count_refusal(&mut stats.refused, e),
        }
    }
    Ok(())
}

/// Sorts by (station, issue time, valid from, text) and removes duplicates under the key in the
/// module comment.
pub fn finalize_taf(t: &mut TafTable) -> TafDedupe {
    let n = taf_len(t);
    let key = |i: u32| {
        let i = i as usize;
        (
            t.station.get(i).copied().unwrap_or(0),
            t.issue_s.get(i).copied().unwrap_or(0),
            t.valid_from_s.get(i).copied().unwrap_or(0),
        )
    };
    let mut order: Vec<u32> = (0..n as u32).collect();
    order.sort_by(|&a, &b| {
        key(a)
            .cmp(&key(b))
            .then_with(|| taf_raw(t, a as usize).cmp(taf_raw(t, b as usize)))
    });
    let mut keep: Vec<u32> = Vec::with_capacity(n);
    let mut rep = TafDedupe {
        rows_in: n as u32,
        ..TafDedupe::default()
    };
    for &i in &order {
        if keep.last().is_some_and(|&p| key(p) == key(i)) {
            rep.duplicates += 1;
            if let Some(last) = keep.last_mut() {
                *last = i;
            }
        } else {
            keep.push(i);
        }
    }
    rep.rows_kept = keep.len() as u32;
    let mut period_first = vec![0u32];
    let mut prows: Vec<u32> = Vec::new();
    for &k in &keep {
        prows.extend(taf_periods(t, k as usize).map(|p| p as u32));
        period_first.push(prows.len() as u32);
    }
    let raw = pool_select(&t.raw, &keep);
    select(&mut t.station, &keep);
    select(&mut t.issue_s, &keep);
    select(&mut t.valid_from_s, &keep);
    select(&mut t.valid_to_s, &keep);
    select(&mut t.lat_e5, &keep);
    select(&mut t.lon_e5, &keep);
    select(&mut t.elev_m, &keep);
    select(&mut t.p_from_s, &prows);
    select(&mut t.p_to_s, &prows);
    select(&mut t.p_change, &prows);
    select(&mut t.p_prob, &prows);
    select(&mut t.p_wdir_deg, &prows);
    select(&mut t.p_wspd_kt, &prows);
    select(&mut t.p_wgst_kt, &prows);
    select(&mut t.p_visib_sm100, &prows);
    select(&mut t.p_ceiling_ft, &prows);
    select(&mut t.p_flags, &prows);
    t.period_first = period_first;
    t.raw = raw;
    rep
}
