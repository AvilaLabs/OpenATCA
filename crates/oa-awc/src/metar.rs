//! METAR / SPECI observations (`/api/data/metar?format=json`) as a struct-of-arrays table, sorted
//! by (station, observation time) so a lookup is a binary search.
//!
//! Row key for de-duplication: (station, `obsTime`, `metarType`). Hourly files with overlapping
//! windows (both ends inclusive) repeat an observation; the copy kept is the one with the
//! greatest `receiptTime`, then the greatest `rawOb` text, so the result does not depend on the
//! order the files were read. Duplicates are counted, and counted again when the copies differ.
//!
//! The altimeter in the `altim` field is cross-checked against the group in `rawOb`. A
//! difference above [`ALTIM_AGREE_TOL_DHPA`] is counted and the row's altimeter is withheld
//! (set to `NONE_U16`, flag [`FLAG_ALTIM_WITHHELD`]): which of the two is right is unknown, and
//! an altitude correction must not guess. The other fields of the row are kept.

use crate::error::{count_refusal, refuse, ParseError, Reason, Refusal, RefusedCounts};
use crate::json::{opt_f64, opt_int, opt_str, req_f64, req_int, req_str, require_object, visib};
use crate::pool::{
    new_pool, pool_get, pool_len, pool_push, pool_select, select, top_array, Pool, StrIx,
};
use crate::units::{
    hpa_to_dhpa, pack_ident, parse_iso_ms, raw_altimeter_dhpa, MAX_TIME_S, MIN_TIME_S, NONE_I16,
    NONE_U16,
};
use crate::{AltimeterError, MAX_RAW_BYTES, MAX_RECORDS_PER_FILE, MAX_TABLE_ROWS};
use oa_json::{parse, Doc, Kind, NodeIx, DEFAULT_LIMITS};

/// Bound: largest accepted difference between `altim` and the `rawOb` group, tenths of hPa.
/// An `A`-format group has 0.01 inHg (0.34 hPa) resolution, so a correct pair can differ by
/// about 0.17 hPa plus the API's own rounding.
pub const ALTIM_AGREE_TOL_DHPA: u16 = 4;
/// `wdir_deg` value for variable wind (`"VRB"`).
pub const WDIR_VRB: u16 = 999;
/// Flag bit: visibility is "this value or more" (`"10+"`).
pub const FLAG_VISIB_PLUS: u8 = 1;
/// Flag bit: `altim` and `rawOb` disagreed; the altimeter was withheld.
pub const FLAG_ALTIM_WITHHELD: u8 = 2;
/// `kind` value of a routine METAR.
pub const KIND_METAR: u8 = 0;
/// `kind` value of a special observation.
pub const KIND_SPECI: u8 = 1;
/// `flt_cat` values: 0 absent, then VFR, MVFR, IFR, LIFR.
pub const FLT_CAT_NAMES: [&str; 5] = ["", "VFR", "MVFR", "IFR", "LIFR"];

/// METAR rows, one `Vec` per field sharing an index. Sorted by (station, obs_s, kind) after
/// [`finalize_metar`].
// A7.2: cold struct (one per load; the columns are the hot data), size guard waived
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MetarTable {
    pub station: Vec<u32>,
    pub obs_s: Vec<i64>,
    pub receipt_ms: Vec<i64>,
    pub kind: Vec<u8>,
    pub lat_e5: Vec<i32>,
    pub lon_e5: Vec<i32>,
    pub elev_m: Vec<i16>,
    /// Altimeter setting, tenths of hPa; `NONE_U16` if absent or withheld.
    pub altim_dhpa: Vec<u16>,
    /// The altimeter parsed from `rawOb`, tenths of hPa; `NONE_U16` if there is none.
    pub raw_altim_dhpa: Vec<u16>,
    pub temp_dc: Vec<i16>,
    pub dewp_dc: Vec<i16>,
    pub wdir_deg: Vec<u16>,
    pub wspd_kt: Vec<u16>,
    pub wgst_kt: Vec<u16>,
    pub visib_sm100: Vec<u16>,
    pub flt_cat: Vec<u8>,
    pub flags: Vec<u8>,
    pub raw: Pool,
}

/// Counts for the METAR files parsed so far.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MetarStats {
    pub files: u32,
    pub records: u32,
    pub accepted: u32,
    pub refused: RefusedCounts,
    /// Records where `altim` and a `rawOb` group were both present and compared.
    pub altim_checked: u32,
    /// Compared records that differ by more than [`ALTIM_AGREE_TOL_DHPA`].
    pub altim_disagree: u32,
    /// Largest compared difference, tenths of hPa.
    pub altim_max_diff_dhpa: u16,
    /// Records with no `altim` (the field absent or null).
    pub altim_missing: u32,
    /// Records with `altim` but no altimeter group in `rawOb`.
    pub altim_no_raw: u32,
}

/// What [`finalize_metar`] removed.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DedupeReport {
    pub rows_in: u32,
    pub rows_kept: u32,
    /// Rows dropped because another row has the same key.
    pub duplicates: u32,
    /// Keys whose copies differed in `rawOb` (a later correction, for example).
    pub conflicting_keys: u32,
}

/// An empty table.
pub fn new_metar_table() -> MetarTable {
    MetarTable {
        station: Vec::new(),
        obs_s: Vec::new(),
        receipt_ms: Vec::new(),
        kind: Vec::new(),
        lat_e5: Vec::new(),
        lon_e5: Vec::new(),
        elev_m: Vec::new(),
        altim_dhpa: Vec::new(),
        raw_altim_dhpa: Vec::new(),
        temp_dc: Vec::new(),
        dewp_dc: Vec::new(),
        wdir_deg: Vec::new(),
        wspd_kt: Vec::new(),
        wgst_kt: Vec::new(),
        visib_sm100: Vec::new(),
        flt_cat: Vec::new(),
        flags: Vec::new(),
        raw: new_pool(),
    }
}

/// Number of rows.
pub fn metar_len(t: &MetarTable) -> usize {
    t.station.len()
}

/// The `rawOb` text of row `i`.
pub fn metar_raw(t: &MetarTable, i: usize) -> &str {
    pool_get(&t.raw, StrIx(i as u32))
}

/// The altimeter of row `i` in hPa, if present and trusted.
pub fn metar_altim_hpa(t: &MetarTable, i: usize) -> Option<f64> {
    match t.altim_dhpa.get(i) {
        Some(&v) if v != NONE_U16 => Some(f64::from(v) / 10.0),
        _ => None,
    }
}

/// One parsed row, before it is appended.
// A7.2: cold struct (a transient record, never stored in bulk), size guard waived
struct Row<'a> {
    station: u32,
    obs_s: i64,
    receipt_ms: i64,
    kind: u8,
    lat_e5: i32,
    lon_e5: i32,
    elev_m: i16,
    altim_dhpa: u16,
    raw_altim_dhpa: u16,
    temp_dc: i16,
    dewp_dc: i16,
    wdir_deg: u16,
    wspd_kt: u16,
    wgst_kt: u16,
    visib_sm100: u16,
    flt_cat: u8,
    flags: u8,
    raw: &'a str,
}

fn tenths(v: Option<f64>, field: &'static str) -> Result<i16, Refusal> {
    match v {
        None => Ok(NONE_I16),
        Some(v) if (-90.0..=70.0).contains(&v) => Ok((v * 10.0).round() as i16),
        Some(_) => refuse(Reason::OutOfRange, field),
    }
}

/// Reads a degree value as hundred-thousandths (about 1 m), range-checked.
pub fn deg_e5(v: f64, lim: f64, field: &'static str) -> Result<i32, Refusal> {
    if !(-lim..=lim).contains(&v) {
        return refuse(Reason::OutOfRange, field);
    }
    Ok((v * 100_000.0).round() as i32)
}

fn read_row<'a>(doc: &'a Doc<'_>, o: NodeIx) -> Result<Row<'a>, Refusal> {
    require_object(doc, o)?;
    let Some(station) = pack_ident(req_str(doc, o, "icaoId")?, 3) else {
        return refuse(Reason::BadIdent, "icaoId");
    };
    let obs_s = req_int(doc, o, "obsTime", MIN_TIME_S, MAX_TIME_S)?;
    let Some(receipt_ms) = parse_iso_ms(req_str(doc, o, "receiptTime")?) else {
        return refuse(Reason::BadTime, "receiptTime");
    };
    let kind = match req_str(doc, o, "metarType")? {
        "METAR" => KIND_METAR,
        "SPECI" => KIND_SPECI,
        _ => return refuse(Reason::UnknownCode, "metarType"),
    };
    let lat_e5 = deg_e5(req_f64(doc, o, "lat")?, 90.0, "lat")?;
    let lon_e5 = deg_e5(req_f64(doc, o, "lon")?, 180.0, "lon")?;
    let elev_m = req_int(doc, o, "elev", -500, 6000)? as i16;
    let raw = req_str(doc, o, "rawOb")?;
    if raw.len() > MAX_RAW_BYTES {
        return refuse(Reason::OutOfRange, "rawOb");
    }
    let altim_dhpa = match opt_f64(doc, o, "altim")? {
        None => NONE_U16,
        Some(v) => match hpa_to_dhpa(v) {
            Some(d) => d,
            None => return refuse(Reason::OutOfRange, "altim"),
        },
    };
    let temp_dc = tenths(opt_f64(doc, o, "temp")?, "temp")?;
    let dewp_dc = tenths(opt_f64(doc, o, "dewp")?, "dewp")?;
    let mut flags = 0u8;
    let wdir_deg = match doc.get(o, "wdir").and_then(|n| Some((n, doc.kind(n)?))) {
        None | Some((_, Kind::Null)) => NONE_U16,
        Some((n, Kind::String)) if doc.str(n) == Some("VRB") => WDIR_VRB,
        Some((_, Kind::Number)) => opt_int(doc, o, "wdir", 0, 360)?.map_or(NONE_U16, |v| v as u16),
        Some(_) => return refuse(Reason::BadType, "wdir"),
    };
    let wspd_kt = opt_int(doc, o, "wspd", 0, 250)?.map_or(NONE_U16, |v| v as u16);
    let wgst_kt = opt_int(doc, o, "wgst", 0, 250)?.map_or(NONE_U16, |v| v as u16);
    let visib_sm100 = visib(doc, o, FLAG_VISIB_PLUS, &mut flags)?;
    let flt_cat = match opt_str(doc, o, "fltCat")? {
        None => 0,
        Some(s) => match FLT_CAT_NAMES.iter().position(|&n| n == s && !n.is_empty()) {
            Some(i) => i as u8,
            None => return refuse(Reason::UnknownCode, "fltCat"),
        },
    };
    Ok(Row {
        station,
        obs_s,
        receipt_ms,
        kind,
        lat_e5,
        lon_e5,
        elev_m,
        altim_dhpa,
        raw_altim_dhpa: raw_altimeter_dhpa(raw).unwrap_or(NONE_U16),
        temp_dc,
        dewp_dc,
        wdir_deg,
        wspd_kt,
        wgst_kt,
        visib_sm100,
        flt_cat,
        flags,
        raw,
    })
}

fn push_row(t: &mut MetarTable, r: &Row<'_>) -> bool {
    if pool_push(&mut t.raw, r.raw).is_none() {
        return false;
    }
    t.station.push(r.station);
    t.obs_s.push(r.obs_s);
    t.receipt_ms.push(r.receipt_ms);
    t.kind.push(r.kind);
    t.lat_e5.push(r.lat_e5);
    t.lon_e5.push(r.lon_e5);
    t.elev_m.push(r.elev_m);
    t.altim_dhpa.push(r.altim_dhpa);
    t.raw_altim_dhpa.push(r.raw_altim_dhpa);
    t.temp_dc.push(r.temp_dc);
    t.dewp_dc.push(r.dewp_dc);
    t.wdir_deg.push(r.wdir_deg);
    t.wspd_kt.push(r.wspd_kt);
    t.wgst_kt.push(r.wgst_kt);
    t.visib_sm100.push(r.visib_sm100);
    t.flt_cat.push(r.flt_cat);
    t.flags.push(r.flags);
    true
}

/// Parses one METAR JSON file and appends its accepted rows (unsorted) to `table`. All or
/// nothing at file level: a JSON error, a non-array, too many records or a full table refuses
/// the file and leaves `table` and `stats` untouched. A bad record is refused alone and counted
/// in `stats.refused` by reason. Call [`finalize_metar`] after the last file.
pub fn parse_metar(
    input: &[u8],
    table: &mut MetarTable,
    stats: &mut MetarStats,
) -> Result<(), ParseError> {
    let doc = parse(input, &DEFAULT_LIMITS)?;
    let (arr, n) = top_array(&doc, metar_len(table), MAX_RECORDS_PER_FILE, MAX_TABLE_ROWS)?;
    stats.files += 1;
    stats.records = stats.records.saturating_add(n as u32);
    for o in doc.elements(arr) {
        match read_row(&doc, o) {
            Ok(mut r) => {
                if r.altim_dhpa == NONE_U16 {
                    stats.altim_missing += 1;
                } else if r.raw_altim_dhpa == NONE_U16 {
                    stats.altim_no_raw += 1;
                } else {
                    stats.altim_checked += 1;
                    let d = r.altim_dhpa.abs_diff(r.raw_altim_dhpa);
                    stats.altim_max_diff_dhpa = stats.altim_max_diff_dhpa.max(d);
                    if d > ALTIM_AGREE_TOL_DHPA {
                        stats.altim_disagree += 1;
                        r.altim_dhpa = NONE_U16;
                        r.flags |= FLAG_ALTIM_WITHHELD;
                    }
                }
                if push_row(table, &r) {
                    stats.accepted += 1;
                } else {
                    count_refusal(
                        &mut stats.refused,
                        Refusal {
                            reason: Reason::OutOfRange,
                            field: "rawOb",
                        },
                    );
                }
            }
            Err(e) => count_refusal(&mut stats.refused, e),
        }
    }
    Ok(())
}

/// Sorts the table by (station, obs_s, kind) and removes duplicates under the key described in
/// the module comment. Deterministic: the result does not depend on row order. Safe to call
/// again after appending more files.
pub fn finalize_metar(t: &mut MetarTable) -> DedupeReport {
    let n = metar_len(t);
    let mut order: Vec<u32> = (0..n as u32).collect();
    let key = |i: u32| {
        let i = i as usize;
        (
            t.station.get(i).copied().unwrap_or(0),
            t.obs_s.get(i).copied().unwrap_or(0),
            t.kind.get(i).copied().unwrap_or(0),
            t.receipt_ms.get(i).copied().unwrap_or(0),
        )
    };
    order.sort_by(|&a, &b| {
        key(a)
            .cmp(&key(b))
            .then_with(|| metar_raw(t, a as usize).cmp(metar_raw(t, b as usize)))
    });
    let mut keep: Vec<u32> = Vec::with_capacity(n);
    let mut rep = DedupeReport {
        rows_in: n as u32,
        ..DedupeReport::default()
    };
    let mut group_conflict = false;
    for &i in &order {
        let same = keep.last().is_some_and(|&p| {
            let (a, b) = (key(p), key(i));
            (a.0, a.1, a.2) == (b.0, b.1, b.2)
        });
        if same {
            rep.duplicates += 1;
            if let Some(&p) = keep.last() {
                if metar_raw(t, p as usize) != metar_raw(t, i as usize) && !group_conflict {
                    group_conflict = true;
                    rep.conflicting_keys += 1;
                }
            }
            // The later row in sort order wins (greatest receipt, then greatest text).
            if let Some(last) = keep.last_mut() {
                *last = i;
            }
        } else {
            group_conflict = false;
            keep.push(i);
        }
    }
    rep.rows_kept = keep.len() as u32;
    let raw = pool_select(&t.raw, &keep);
    select(&mut t.station, &keep);
    select(&mut t.obs_s, &keep);
    select(&mut t.receipt_ms, &keep);
    select(&mut t.kind, &keep);
    select(&mut t.lat_e5, &keep);
    select(&mut t.lon_e5, &keep);
    select(&mut t.elev_m, &keep);
    select(&mut t.altim_dhpa, &keep);
    select(&mut t.raw_altim_dhpa, &keep);
    select(&mut t.temp_dc, &keep);
    select(&mut t.dewp_dc, &keep);
    select(&mut t.wdir_deg, &keep);
    select(&mut t.wspd_kt, &keep);
    select(&mut t.wgst_kt, &keep);
    select(&mut t.visib_sm100, &keep);
    select(&mut t.flt_cat, &keep);
    select(&mut t.flags, &keep);
    t.raw = raw;
    debug_assert_eq!(pool_len(&t.raw), metar_len(t));
    rep
}

/// A found altimeter setting.
// A7.2: cold struct (one per lookup), size guard waived
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AltimeterFix {
    /// Altimeter setting, hPa.
    pub hpa: f64,
    /// Observation time, Unix seconds.
    pub obs_s: i64,
    /// Seconds from the observation to the requested time (never negative).
    pub age_s: i64,
    /// Row in the table (valid until the table changes).
    pub row: u32,
}

/// The latest trusted altimeter setting of `station` observed at or before `t_s`, with its age.
/// Observations without a trusted altimeter are skipped. Refuses when the station is unknown,
/// nothing was observed yet, no observation has an altimeter, or the altimeter is older than
/// `max_age_s` (use [`crate::DEFAULT_MAX_ALTIMETER_AGE_S`] unless the caller states another).
/// The table must have been through [`finalize_metar`]. Binary search; no allocation.
pub fn altimeter_at(
    t: &MetarTable,
    station: &str,
    t_s: i64,
    max_age_s: i64,
) -> Result<AltimeterFix, AltimeterError> {
    let Some(id) = pack_ident(station, 3) else {
        return Err(AltimeterError::UnknownStation);
    };
    let lo = t.station.partition_point(|&s| s < id);
    let hi = t.station.partition_point(|&s| s <= id);
    if lo == hi {
        return Err(AltimeterError::UnknownStation);
    }
    let run = t.obs_s.get(lo..hi).unwrap_or(&[]);
    let upto = lo + run.partition_point(|&o| o <= t_s);
    if upto == lo {
        return Err(AltimeterError::NoObservationYet);
    }
    for i in (lo..upto).rev() {
        if let (Some(hpa), Some(&obs)) = (metar_altim_hpa(t, i), t.obs_s.get(i)) {
            let age_s = t_s - obs;
            if age_s > max_age_s {
                return Err(AltimeterError::Stale { age_s });
            }
            return Ok(AltimeterFix {
                hpa,
                obs_s: obs,
                age_s,
                row: i as u32,
            });
        }
    }
    Err(AltimeterError::NoAltimeter)
}

/// Number of distinct stations in a finalized table.
pub fn metar_station_count(t: &MetarTable) -> usize {
    let mut n = 0;
    let mut prev = None;
    for &s in &t.station {
        if prev != Some(s) {
            n += 1;
            prev = Some(s);
        }
    }
    n
}
