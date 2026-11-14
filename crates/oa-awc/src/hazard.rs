//! Hazard areas: international SIGMET (`/api/data/sigmet` and `isigmet`), US SIGMET/AIRMET
//! (`airsigmet`) and G-AIRMET (`gairmet`), in one table so a trajectory or a conflict check
//! asks one question: which hazards are active at time `t`, and does one cover this point and
//! altitude. These endpoints are snapshots of what is current at the fetch time (no history).
//!
//! * Geometry: one or more rings of points in degrees (stored as 1e-5 degrees). A ring is
//!   implicitly closed; a repeated last point is harmless. Longitudes may exceed 180 (an
//!   antimeridian polygon runs to, say, 180.667); [`hazard_covers`] tries the point at
//!   -360, 0 and +360 degrees. A `LINE` (G-AIRMET freezing-level contours, including closed
//!   ones; the API's `geom` is used, not `geometryType`) is stored but covers nothing.
//! * Altitude: feet above mean sea level, the lower bound `base_ft` and upper bound `top_ft`,
//!   `NO_ALT` where the product states none (an open end). `SFC` is 0. G-AIRMET altitudes are in
//!   hundreds of feet in the API and are converted; a G-AIRMET base of `FZL` ("the freezing
//!   level") sets [`FLAG_BASE_FZL`] and leaves `base_ft` at `NO_ALT`; the freezing-level band of
//!   an icing area is `fzl_base_ft` to `fzl_top_ft`, and `level_ft` is the level of a `FZLVL`
//!   contour. For a US SIGMET, `base_ft` is the smallest and `top_ft` the largest of the
//!   `altitudeLow*` and `altitudeHi*` values the API gives.
//! * Validity: `valid_from_s` to `valid_to_s`, Unix seconds, from the product's own
//!   `validTimeFrom`/`validTimeTo` (G-AIRMET: `validTime` to `expireTime`).
//! * `severity` is the product's own code and is not comparable across sources: for the US
//!   SIGMET it is the API's integer, for G-AIRMET 1 light, 2 moderate, 3 severe; 255 if absent.
//! * De-duplication key: (source, label, hazard, valid from, valid to, base, top, point count, first
//!   point; the issue time is left out because `sigmet` and `isigmet` give it differently); the copy kept has the greatest receipt time. Counted.

use crate::error::{count_refusal, refuse, ParseError, Reason, Refusal, RefusedCounts};
use crate::json::{opt_int, opt_str, req_array, req_int, req_str, require_object};
use crate::metar::deg_e5;
use crate::pool::{new_pool, pool_get, pool_push, pool_select, select, top_array, Pool, StrIx};
use crate::units::{
    pack_ident, parse_iso_ms, time_in_bounds, MAX_TIME_S, MIN_TIME_S, NO_ALT, NO_TIME,
};
use crate::{MAX_RECORDS_PER_FILE, MAX_TABLE_ROWS};
use oa_json::{parse, Doc, Kind, NodeIx, DEFAULT_LIMITS};

/// `source` value: international SIGMET.
pub const SRC_SIGMET: u8 = 0;
/// `source` value: US SIGMET / AIRMET (`airsigmet`).
pub const SRC_AIRSIGMET: u8 = 1;
/// `source` value: G-AIRMET.
pub const SRC_GAIRMET: u8 = 2;
/// `source` names, by code.
pub const SOURCE_NAMES: [&str; 3] = ["sigmet", "airsigmet", "gairmet"];
/// `geom` value: one closed area.
pub const GEOM_AREA: u8 = 0;
/// `geom` value: an open or closed line (covers nothing).
pub const GEOM_LINE: u8 = 1;
/// `geom` value: several areas (international SIGMET `AREAS`).
pub const GEOM_AREAS: u8 = 2;
/// Flag bit: the lower bound is the freezing level (G-AIRMET `FZL`).
pub const FLAG_BASE_FZL: u8 = 1;
/// Bound: points in one ring.
pub const MAX_RING_POINTS: usize = 2048;
/// Bound: rings in one record.
pub const MAX_RINGS: usize = 64;
/// Bound: distinct hazard words.
pub const MAX_HAZARD_NAMES: usize = 256;
/// Bound: lowest and highest accepted altitude, feet.
pub const ALT_RANGE_FT: (i64, i64) = (-2000, 100_000);
/// Bound: longest accepted label text, bytes.
pub const MAX_LABEL_BYTES: usize = 128;

/// Hazard rows, one `Vec` per field sharing an index, plus the ring and point columns.
// A7.2: cold struct (one per load; the columns are the hot data), size guard waived
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HazardTable {
    pub source: Vec<u8>,
    /// Index into `hazard_names`.
    pub hazard: Vec<u16>,
    pub issue_s: Vec<i64>,
    pub receipt_ms: Vec<i64>,
    pub valid_from_s: Vec<i64>,
    pub valid_to_s: Vec<i64>,
    pub base_ft: Vec<i32>,
    pub top_ft: Vec<i32>,
    pub level_ft: Vec<i32>,
    pub fzl_base_ft: Vec<i32>,
    pub fzl_top_ft: Vec<i32>,
    pub severity: Vec<u8>,
    pub geom: Vec<u8>,
    pub flags: Vec<u8>,
    /// Row `i` owns rings `ring_first[i]..ring_first[i + 1]`; one more entry than rows.
    pub ring_first: Vec<u32>,
    /// Ring `r` owns points `pt_first[r]..pt_first[r + 1]`; one more entry than rings.
    pub pt_first: Vec<u32>,
    pub lat_e5: Vec<i32>,
    pub lon_e5: Vec<i32>,
    /// Hazard words (`TS`, `TURB`, `ICE`, `IFR`, ...); sorted after [`finalize_hazard`].
    pub hazard_names: Vec<String>,
    /// Series or tag text of each row, as [`StrIx`] into `labels`.
    pub label: Vec<u32>,
    pub labels: Pool,
}

/// Counts for the files of one product parsed so far.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HazardStats {
    pub files: u32,
    pub records: u32,
    pub accepted: u32,
    pub refused: RefusedCounts,
}

/// What [`finalize_hazard`] removed.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HazardDedupe {
    pub rows_in: u32,
    pub rows_kept: u32,
    pub duplicates: u32,
}

/// An empty table.
pub fn new_hazard_table() -> HazardTable {
    HazardTable {
        source: Vec::new(),
        hazard: Vec::new(),
        issue_s: Vec::new(),
        receipt_ms: Vec::new(),
        valid_from_s: Vec::new(),
        valid_to_s: Vec::new(),
        base_ft: Vec::new(),
        top_ft: Vec::new(),
        level_ft: Vec::new(),
        fzl_base_ft: Vec::new(),
        fzl_top_ft: Vec::new(),
        severity: Vec::new(),
        geom: Vec::new(),
        flags: Vec::new(),
        ring_first: vec![0],
        pt_first: vec![0],
        lat_e5: Vec::new(),
        lon_e5: Vec::new(),
        hazard_names: Vec::new(),
        label: Vec::new(),
        labels: new_pool(),
    }
}

/// Number of rows.
pub fn hazard_len(t: &HazardTable) -> usize {
    t.source.len()
}

/// The hazard word of row `i`.
pub fn hazard_name(t: &HazardTable, i: usize) -> &str {
    t.hazard
        .get(i)
        .and_then(|&h| t.hazard_names.get(h as usize))
        .map_or("", String::as_str)
}

/// The label of row `i`.
pub fn hazard_label(t: &HazardTable, i: usize) -> &str {
    pool_get(
        &t.labels,
        StrIx(t.label.get(i).copied().unwrap_or(u32::MAX)),
    )
}

/// The rings of row `i` as a range of ring numbers.
pub fn hazard_rings(t: &HazardTable, i: usize) -> std::ops::Range<usize> {
    let a = t.ring_first.get(i).copied().unwrap_or(0) as usize;
    let b = t.ring_first.get(i + 1).copied().unwrap_or(0) as usize;
    a..b
}

/// The points of ring `r` as a range of point numbers.
pub fn ring_points(t: &HazardTable, r: usize) -> std::ops::Range<usize> {
    let a = t.pt_first.get(r).copied().unwrap_or(0) as usize;
    let b = t.pt_first.get(r + 1).copied().unwrap_or(0) as usize;
    a..b
}

/// One parsed record, before it is appended.
// A7.2: cold struct (a transient record, never stored in bulk), size guard waived
struct Rec {
    source: u8,
    hazard: String,
    label: String,
    issue_s: i64,
    receipt_ms: i64,
    valid_from_s: i64,
    valid_to_s: i64,
    base_ft: i32,
    top_ft: i32,
    level_ft: i32,
    fzl_base_ft: i32,
    fzl_top_ft: i32,
    severity: u8,
    geom: u8,
    flags: u8,
    rings: Vec<Vec<(i32, i32)>>,
}

fn alt_ft(v: i64, field: &'static str) -> Result<i32, Refusal> {
    if (ALT_RANGE_FT.0..=ALT_RANGE_FT.1).contains(&v) {
        Ok(v as i32)
    } else {
        refuse(Reason::OutOfRange, field)
    }
}

fn opt_alt_int(doc: &Doc<'_>, o: NodeIx, key: &'static str) -> Result<i32, Refusal> {
    match opt_int(doc, o, key, i64::MIN / 2, i64::MAX / 2)? {
        None => Ok(NO_ALT),
        Some(v) => alt_ft(v, key),
    }
}

/// A G-AIRMET altitude text: hundreds of feet (`"060"`), `SFC`, or `FZL`.
fn gairmet_alt(
    doc: &Doc<'_>,
    o: NodeIx,
    key: &'static str,
    flags: Option<&mut u8>,
) -> Result<i32, Refusal> {
    match opt_str(doc, o, key)? {
        None => Ok(NO_ALT),
        Some("SFC") => Ok(0),
        Some("FZL") => match flags {
            Some(f) => {
                *f |= FLAG_BASE_FZL;
                Ok(NO_ALT)
            }
            None => refuse(Reason::UnknownCode, key),
        },
        Some(s) => {
            let ok = !s.is_empty() && s.len() <= 3 && s.bytes().all(|b| b.is_ascii_digit());
            match (ok, s.parse::<i64>()) {
                (true, Ok(h)) => alt_ft(h * 100, key),
                _ => refuse(Reason::BadType, key),
            }
        }
    }
}

fn label_of(s: String) -> Result<String, Refusal> {
    if s.len() > MAX_LABEL_BYTES {
        refuse(Reason::OutOfRange, "label")
    } else {
        Ok(s)
    }
}

fn hazard_word(s: &str, key: &'static str) -> Result<String, Refusal> {
    let ok = !s.is_empty()
        && s.len() <= 16
        && s.bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_' || b == b'-');
    if ok {
        Ok(s.to_string())
    } else {
        refuse(Reason::BadType, key)
    }
}

/// A point node `{lat, lon}` with numbers or numeric strings.
fn point(doc: &Doc<'_>, p: NodeIx) -> Result<(i32, i32), Refusal> {
    let one = |key: &'static str, lim: f64| -> Result<i32, Refusal> {
        let Some(n) = doc.get(p, key) else {
            return refuse(Reason::MissingField, key);
        };
        let v = match doc.kind(n) {
            Some(Kind::Number) => doc.as_f64(n),
            Some(Kind::String) => doc.str(n).and_then(|s| s.parse::<f64>().ok()),
            _ => None,
        };
        match v {
            Some(v) if v.is_finite() => deg_e5(v, lim, key),
            _ => refuse(Reason::BadType, key),
        }
    };
    require_object(doc, p)?;
    Ok((one("lat", 90.0)?, one("lon", 360.0)?))
}

fn ring(doc: &Doc<'_>, arr: NodeIx, min_pts: usize) -> Result<Vec<(i32, i32)>, Refusal> {
    let n = doc.len(arr);
    if n < min_pts || n > MAX_RING_POINTS {
        return refuse(Reason::BadGeometry, "coords");
    }
    doc.elements(arr).map(|p| point(doc, p)).collect()
}

fn rings_of(doc: &Doc<'_>, o: NodeIx, geom: u8) -> Result<Vec<Vec<(i32, i32)>>, Refusal> {
    let coords = req_array(doc, o, "coords")?;
    let min_pts = if geom == GEOM_LINE { 2 } else { 3 };
    if geom == GEOM_AREAS {
        let n = doc.len(coords);
        if n == 0 || n > MAX_RINGS {
            return refuse(Reason::BadGeometry, "coords");
        }
        doc.elements(coords)
            .map(|r| {
                if doc.kind(r) == Some(Kind::Array) {
                    ring(doc, r, min_pts)
                } else {
                    refuse(Reason::BadGeometry, "coords")
                }
            })
            .collect()
    } else {
        Ok(vec![ring(doc, coords, min_pts)?])
    }
}

fn iso_ms(doc: &Doc<'_>, o: NodeIx, key: &'static str) -> Result<i64, Refusal> {
    match parse_iso_ms(req_str(doc, o, key)?) {
        Some(v) => Ok(v),
        None => refuse(Reason::BadTime, key),
    }
}

fn opt_iso_s(doc: &Doc<'_>, o: NodeIx, key: &'static str) -> Result<i64, Refusal> {
    match opt_str(doc, o, key)? {
        None => Ok(NO_TIME),
        Some(s) => match parse_iso_ms(s) {
            Some(ms) => Ok(ms.div_euclid(1000)),
            None => refuse(Reason::BadTime, key),
        },
    }
}

fn window(from: i64, to: i64) -> Result<(), Refusal> {
    if !time_in_bounds(from) || !time_in_bounds(to) {
        return refuse(Reason::OutOfRange, "validTime");
    }
    if to < from {
        return refuse(Reason::ReversedBand, "validTimeTo");
    }
    Ok(())
}

fn ordered(base: i32, top: i32) -> Result<(), Refusal> {
    if base != NO_ALT && top != NO_ALT && base > top {
        return refuse(Reason::ReversedBand, "base");
    }
    Ok(())
}

fn read_sigmet(doc: &Doc<'_>, o: NodeIx) -> Result<Rec, Refusal> {
    require_object(doc, o)?;
    let icao = req_str(doc, o, "icaoId")?;
    if pack_ident(icao, 3).is_none() {
        return refuse(Reason::BadIdent, "icaoId");
    }
    let geom = match req_str(doc, o, "geom")? {
        "AREA" => GEOM_AREA,
        "AREAS" => GEOM_AREAS,
        "LINE" => GEOM_LINE,
        _ => return refuse(Reason::UnknownCode, "geom"),
    };
    let from = req_int(doc, o, "validTimeFrom", MIN_TIME_S, MAX_TIME_S)?;
    let to = req_int(doc, o, "validTimeTo", MIN_TIME_S, MAX_TIME_S)?;
    window(from, to)?;
    let (base_ft, top_ft) = (opt_alt_int(doc, o, "base")?, opt_alt_int(doc, o, "top")?);
    ordered(base_ft, top_ft)?;
    Ok(Rec {
        source: SRC_SIGMET,
        hazard: hazard_word(req_str(doc, o, "hazard")?, "hazard")?,
        label: label_of(format!("{icao} {}", req_str(doc, o, "seriesId")?))?,
        issue_s: opt_iso_s(doc, o, "issueTime")?,
        receipt_ms: iso_ms(doc, o, "receiptTime")?,
        valid_from_s: from,
        valid_to_s: to,
        base_ft,
        top_ft,
        level_ft: NO_ALT,
        fzl_base_ft: NO_ALT,
        fzl_top_ft: NO_ALT,
        severity: 255,
        geom,
        flags: 0,
        rings: rings_of(doc, o, geom)?,
    })
}

fn read_airsigmet(doc: &Doc<'_>, o: NodeIx) -> Result<Rec, Refusal> {
    require_object(doc, o)?;
    let icao = req_str(doc, o, "icaoId")?;
    if pack_ident(icao, 3).is_none() {
        return refuse(Reason::BadIdent, "icaoId");
    }
    let from = req_int(doc, o, "validTimeFrom", MIN_TIME_S, MAX_TIME_S)?;
    let to = req_int(doc, o, "validTimeTo", MIN_TIME_S, MAX_TIME_S)?;
    window(from, to)?;
    let lows = [
        opt_alt_int(doc, o, "altitudeLow1")?,
        opt_alt_int(doc, o, "altitudeLow2")?,
    ];
    let highs = [
        opt_alt_int(doc, o, "altitudeHi1")?,
        opt_alt_int(doc, o, "altitudeHi2")?,
    ];
    let present = |v: [i32; 2]| v.into_iter().filter(|&x| x != NO_ALT);
    let base_ft = present(lows).min().unwrap_or(NO_ALT);
    let top_ft = present(highs).max().unwrap_or(NO_ALT);
    ordered(base_ft, top_ft)?;
    let kind = req_str(doc, o, "airSigmetType")?;
    let alpha = opt_str(doc, o, "alphaChar")?.unwrap_or("");
    let severity = opt_int(doc, o, "severity", 0, 9)?.map_or(255, |v| v as u8);
    Ok(Rec {
        source: SRC_AIRSIGMET,
        hazard: hazard_word(req_str(doc, o, "hazard")?, "hazard")?,
        label: label_of(format!(
            "{icao} {kind} {alpha} {}",
            req_str(doc, o, "seriesId")?
        ))?,
        issue_s: opt_iso_s(doc, o, "creationTime")?,
        receipt_ms: iso_ms(doc, o, "receiptTime")?,
        valid_from_s: from,
        valid_to_s: to,
        base_ft,
        top_ft,
        level_ft: NO_ALT,
        fzl_base_ft: NO_ALT,
        fzl_top_ft: NO_ALT,
        severity,
        geom: GEOM_AREA,
        flags: 0,
        rings: rings_of(doc, o, GEOM_AREA)?,
    })
}

fn read_gairmet(doc: &Doc<'_>, o: NodeIx) -> Result<Rec, Refusal> {
    require_object(doc, o)?;
    let geom = match req_str(doc, o, "geom")? {
        "AREA" => GEOM_AREA,
        "LINE" => GEOM_LINE,
        _ => return refuse(Reason::UnknownCode, "geom"),
    };
    let from = iso_ms(doc, o, "validTime")?.div_euclid(1000);
    let to = req_int(doc, o, "expireTime", MIN_TIME_S, MAX_TIME_S)?;
    window(from, to)?;
    let mut flags = 0u8;
    let base_ft = gairmet_alt(doc, o, "base", Some(&mut flags))?;
    let top_ft = gairmet_alt(doc, o, "top", None)?;
    ordered(base_ft, top_ft)?;
    let severity = match opt_str(doc, o, "severity")? {
        None => 255,
        Some("LGT") => 1,
        Some("MOD") => 2,
        Some("SEV") => 3,
        Some(_) => return refuse(Reason::UnknownCode, "severity"),
    };
    let receipt = req_int(doc, o, "receiptTime", MIN_TIME_S, MAX_TIME_S)?;
    Ok(Rec {
        source: SRC_GAIRMET,
        hazard: hazard_word(req_str(doc, o, "hazard")?, "hazard")?,
        label: label_of(format!(
            "{} {}",
            req_str(doc, o, "product")?,
            req_str(doc, o, "tag")?
        ))?,
        issue_s: req_int(doc, o, "issueTime", MIN_TIME_S, MAX_TIME_S)?,
        receipt_ms: receipt * 1000,
        valid_from_s: from,
        valid_to_s: to,
        base_ft,
        top_ft,
        level_ft: gairmet_alt(doc, o, "level", None)?,
        fzl_base_ft: gairmet_alt(doc, o, "fzlbase", None)?,
        fzl_top_ft: gairmet_alt(doc, o, "fzltop", None)?,
        severity,
        geom,
        flags,
        rings: rings_of(doc, o, geom)?,
    })
}

fn push_rec(t: &mut HazardTable, r: &Rec) -> Result<(), Refusal> {
    let h = match t.hazard_names.iter().position(|n| *n == r.hazard) {
        Some(i) => i,
        None => {
            if t.hazard_names.len() >= MAX_HAZARD_NAMES {
                return refuse(Reason::OutOfRange, "hazard");
            }
            t.hazard_names.push(r.hazard.clone());
            t.hazard_names.len() - 1
        }
    };
    let Some(StrIx(label)) = pool_push(&mut t.labels, &r.label) else {
        return refuse(Reason::OutOfRange, "label");
    };
    for rg in &r.rings {
        for &(lat, lon) in rg {
            t.lat_e5.push(lat);
            t.lon_e5.push(lon);
        }
        t.pt_first.push(t.lat_e5.len() as u32);
    }
    t.ring_first.push((t.pt_first.len() - 1) as u32);
    t.source.push(r.source);
    t.hazard.push(h as u16);
    t.issue_s.push(r.issue_s);
    t.receipt_ms.push(r.receipt_ms);
    t.valid_from_s.push(r.valid_from_s);
    t.valid_to_s.push(r.valid_to_s);
    t.base_ft.push(r.base_ft);
    t.top_ft.push(r.top_ft);
    t.level_ft.push(r.level_ft);
    t.fzl_base_ft.push(r.fzl_base_ft);
    t.fzl_top_ft.push(r.fzl_top_ft);
    t.severity.push(r.severity);
    t.geom.push(r.geom);
    t.flags.push(r.flags);
    t.label.push(label);
    Ok(())
}

fn parse_with(
    input: &[u8],
    table: &mut HazardTable,
    stats: &mut HazardStats,
    read: fn(&Doc<'_>, NodeIx) -> Result<Rec, Refusal>,
) -> Result<(), ParseError> {
    let doc = parse(input, &DEFAULT_LIMITS)?;
    let (arr, n) = top_array(
        &doc,
        hazard_len(table),
        MAX_RECORDS_PER_FILE,
        MAX_TABLE_ROWS,
    )?;
    stats.files += 1;
    stats.records = stats.records.saturating_add(n as u32);
    for o in doc.elements(arr) {
        match read(&doc, o).and_then(|r| push_rec(table, &r)) {
            Ok(()) => stats.accepted += 1,
            Err(e) => count_refusal(&mut stats.refused, e),
        }
    }
    Ok(())
}

/// Parses an international SIGMET file (`sigmet` or `isigmet`). All or nothing at file level; a
/// bad record is refused alone and counted by reason.
pub fn parse_sigmet(
    input: &[u8],
    table: &mut HazardTable,
    stats: &mut HazardStats,
) -> Result<(), ParseError> {
    parse_with(input, table, stats, read_sigmet)
}

/// Parses a US SIGMET/AIRMET file (`airsigmet`). Same rules as [`parse_sigmet`].
pub fn parse_airsigmet(
    input: &[u8],
    table: &mut HazardTable,
    stats: &mut HazardStats,
) -> Result<(), ParseError> {
    parse_with(input, table, stats, read_airsigmet)
}

/// Parses a G-AIRMET file (`gairmet`). Same rules as [`parse_sigmet`].
pub fn parse_gairmet(
    input: &[u8],
    table: &mut HazardTable,
    stats: &mut HazardStats,
) -> Result<(), ParseError> {
    parse_with(input, table, stats, read_gairmet)
}

/// Sorts by (source, label, hazard name, times, altitudes) and removes duplicates under the key
/// in the module comment, keeping the greatest receipt time; sorts the hazard dictionary
/// alphabetically and remaps the rows, so the result does not depend on the order of files.
pub fn finalize_hazard(t: &mut HazardTable) -> HazardDedupe {
    let n = hazard_len(t);
    // Sort the dictionary and remap.
    let mut names: Vec<(String, usize)> = t
        .hazard_names
        .iter()
        .cloned()
        .enumerate()
        .map(|(i, s)| (s, i))
        .collect();
    names.sort();
    let mut remap = vec![0u16; names.len()];
    for (new, (_, old)) in names.iter().enumerate() {
        if let Some(r) = remap.get_mut(*old) {
            *r = new as u16;
        }
    }
    for h in &mut t.hazard {
        *h = remap.get(*h as usize).copied().unwrap_or(0);
    }
    t.hazard_names = names.into_iter().map(|(s, _)| s).collect();

    let first_pt = |i: usize| {
        let r = hazard_rings(t, i).start;
        let p = t.pt_first.get(r).copied().unwrap_or(0) as usize;
        (
            t.lat_e5.get(p).copied().unwrap_or(0),
            t.lon_e5.get(p).copied().unwrap_or(0),
        )
    };
    let npts = |i: usize| {
        let r = hazard_rings(t, i);
        let a = t.pt_first.get(r.start).copied().unwrap_or(0);
        let b = t.pt_first.get(r.end).copied().unwrap_or(0);
        b - a
    };
    let key = |i: u32| {
        let i = i as usize;
        (
            t.source.get(i).copied().unwrap_or(0),
            hazard_label(t, i),
            t.hazard.get(i).copied().unwrap_or(0),
            t.valid_from_s.get(i).copied().unwrap_or(0),
            t.valid_to_s.get(i).copied().unwrap_or(0),
            t.base_ft.get(i).copied().unwrap_or(0),
            t.top_ft.get(i).copied().unwrap_or(0),
            npts(i),
            first_pt(i),
        )
    };
    let mut order: Vec<u32> = (0..n as u32).collect();
    order.sort_by(|&a, &b| {
        key(a).cmp(&key(b)).then_with(|| {
            t.receipt_ms
                .get(a as usize)
                .cmp(&t.receipt_ms.get(b as usize))
        })
    });
    let mut keep: Vec<u32> = Vec::with_capacity(n);
    let mut rep = HazardDedupe {
        rows_in: n as u32,
        ..HazardDedupe::default()
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

    // Rebuild the ring and point columns in the new row order.
    let mut ring_first = vec![0u32];
    let mut pt_first = vec![0u32];
    let (mut lat, mut lon) = (Vec::new(), Vec::new());
    for &k in &keep {
        for r in hazard_rings(t, k as usize) {
            for p in ring_points(t, r) {
                lat.push(t.lat_e5.get(p).copied().unwrap_or(0));
                lon.push(t.lon_e5.get(p).copied().unwrap_or(0));
            }
            pt_first.push(lat.len() as u32);
        }
        ring_first.push((pt_first.len() - 1) as u32);
    }
    let labels = {
        let idx: Vec<u32> = keep
            .iter()
            .map(|&k| t.label.get(k as usize).copied().unwrap_or(0))
            .collect();
        pool_select(&t.labels, &idx)
    };
    select(&mut t.source, &keep);
    select(&mut t.hazard, &keep);
    select(&mut t.issue_s, &keep);
    select(&mut t.receipt_ms, &keep);
    select(&mut t.valid_from_s, &keep);
    select(&mut t.valid_to_s, &keep);
    select(&mut t.base_ft, &keep);
    select(&mut t.top_ft, &keep);
    select(&mut t.level_ft, &keep);
    select(&mut t.fzl_base_ft, &keep);
    select(&mut t.fzl_top_ft, &keep);
    select(&mut t.severity, &keep);
    select(&mut t.geom, &keep);
    select(&mut t.flags, &keep);
    t.label = (0..keep.len() as u32).collect();
    t.labels = labels;
    t.ring_first = ring_first;
    t.pt_first = pt_first;
    t.lat_e5 = lat;
    t.lon_e5 = lon;
    rep
}

/// Appends to `out` the rows valid at `t_s` (`valid_from_s <= t_s <= valid_to_s`), in row order.
/// `out` is cleared first.
pub fn active_hazards_at(t: &HazardTable, t_s: i64, out: &mut Vec<u32>) {
    out.clear();
    for i in 0..hazard_len(t) {
        let (Some(&a), Some(&b)) = (t.valid_from_s.get(i), t.valid_to_s.get(i)) else {
            continue;
        };
        if a <= t_s && t_s <= b {
            out.push(i as u32);
        }
    }
}

fn inside(t: &HazardTable, ring: usize, lat: f64, lon: f64) -> bool {
    let pts = ring_points(t, ring);
    let get = |p: usize| -> (f64, f64) {
        (
            f64::from(t.lat_e5.get(p).copied().unwrap_or(0)) / 1e5,
            f64::from(t.lon_e5.get(p).copied().unwrap_or(0)) / 1e5,
        )
    };
    let mut c = false;
    let mut j = pts.end.saturating_sub(1);
    for i in pts.clone() {
        let ((yi, xi), (yj, xj)) = (get(i), get(j));
        if (yi > lat) != (yj > lat) && lon < (xj - xi) * (lat - yi) / (yj - yi) + xi {
            c = !c;
        }
        j = i;
    }
    c
}

/// Whether hazard row `i` covers the point: inside one of its rings (even-odd rule, planar in
/// degrees) and, when `alt_ft` is given, within `base_ft..=top_ft` (an absent bound is open;
/// a G-AIRMET base of `FZL` is treated as open because the freezing level varies). A `LINE`
/// covers nothing. Not time-aware; combine with [`active_hazards_at`].
pub fn hazard_covers(t: &HazardTable, i: usize, lat: f64, lon: f64, alt_ft: Option<i32>) -> bool {
    if t.geom.get(i).copied() == Some(GEOM_LINE) || i >= hazard_len(t) {
        return false;
    }
    if let Some(a) = alt_ft {
        let (b, tp) = (
            t.base_ft.get(i).copied().unwrap_or(NO_ALT),
            t.top_ft.get(i).copied().unwrap_or(NO_ALT),
        );
        if (b != NO_ALT && a < b) || (tp != NO_ALT && a > tp) {
            return false;
        }
    }
    hazard_rings(t, i).any(|r| {
        [lon, lon + 360.0, lon - 360.0]
            .iter()
            .any(|&x| inside(t, r, lat, x))
    })
}
