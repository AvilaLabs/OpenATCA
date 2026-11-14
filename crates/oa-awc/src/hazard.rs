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
//! * Points: an international SIGMET located at one spot (a volcano, an observed turbulence
//!   report) comes from the API as `geom` `AREA` with a single coordinate. It is stored as
//!   [`GEOM_POINT`] with that one vertex. ICAO gives such a report no extent, so a point never
//!   covers anything ([`hazard_covers`] is false); [`point_hazards_near`] returns the points
//!   within a distance the caller chooses. When `rawSigmet` states the position after `OBS AT` or
//!   `FCST AT` (`S4452 E16900`), it is compared with the coordinate and the agreement counted
//!   in [`HazardStats`]; a disagreement is counted, never a refusal. Two coordinates on an `AREA`
//!   are neither an area nor a point (the AWC API schema documents no such meaning) and are
//!   refused as `bad-geometry`.
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

use crate::error::{
    count_refusal, refuse, ParseError, Reason, Refusal, RefusedCounts, REASON_NAMES,
};
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
/// `geom` value: one position with no stated extent (an `AREA` of one coordinate).
pub const GEOM_POINT: u8 = 3;
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
    /// Point hazards accepted (`GEOM_POINT`).
    pub points: u32,
    /// Points whose `rawSigmet` states a position (`OBS AT`/`FCST AT`) that was compared.
    pub point_raw_checked: u32,
    /// Compared points whose stated position differs from the coordinate by more than the
    /// stated precision.
    pub point_raw_disagree: u32,
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
    /// For a point: [`RAW_NONE`], [`RAW_AGREE`] or [`RAW_DISAGREE`].
    raw_check: u8,
}

const RAW_NONE: u8 = 0;
const RAW_AGREE: u8 = 1;
const RAW_DISAGREE: u8 = 2;

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

type Rings = Vec<Vec<(i32, i32)>>;

/// The geometry of an international SIGMET: an `AREA` of exactly one coordinate is a point.
fn sigmet_geometry(doc: &Doc<'_>, o: NodeIx, geom: u8) -> Result<(u8, Rings), Refusal> {
    let coords = req_array(doc, o, "coords")?;
    if geom == GEOM_AREA && doc.len(coords) == 1 {
        return Ok((GEOM_POINT, vec![ring(doc, coords, 1)?]));
    }
    Ok((geom, rings_of(doc, o, geom)?))
}

/// A latitude or longitude token of an ICAO position (`S4452`, `E16900`, `N45`): hemisphere letter,
/// `deg_digits` of degrees and optional two digits of minutes. Returns signed degrees and the
/// stated precision in degrees (a minute or a whole degree).
fn icao_angle(tok: &str, pos: u8, neg: u8, deg_digits: usize) -> Option<(f64, f64)> {
    let b = tok.as_bytes();
    let hemi = *b.first()?;
    let digits = b.get(1..)?;
    if !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let num = |a: usize, z: usize| -> Option<f64> {
        std::str::from_utf8(digits.get(a..z)?)
            .ok()?
            .parse::<f64>()
            .ok()
    };
    let (v, prec) = if digits.len() == deg_digits {
        (num(0, deg_digits)?, 1.0)
    } else if digits.len() == deg_digits + 2 {
        (
            num(0, deg_digits)? + num(deg_digits, deg_digits + 2)? / 60.0,
            1.0 / 60.0,
        )
    } else {
        return None;
    };
    if hemi == pos {
        Some((v, prec))
    } else if hemi == neg {
        Some((-v, prec))
    } else {
        None
    }
}

/// The position stated after `OBS AT` or `FCST AT` in a SIGMET text (an optional `hhmmZ` first),
/// as (lat, lon, lat precision, lon precision) in degrees; `None` if the text has no such form.
fn raw_position(raw: &str) -> Option<(f64, f64, f64, f64)> {
    let toks: Vec<&str> = raw.split_ascii_whitespace().take(512).collect();
    for i in 1..toks.len() {
        if toks.get(i) != Some(&"AT") || !matches!(toks.get(i - 1), Some(&"OBS" | &"FCST")) {
            continue;
        }
        let mut j = i + 1;
        if toks
            .get(j)
            .is_some_and(|t| t.len() == 5 && t.ends_with('Z'))
        {
            j += 1;
        }
        let (la, lo) = (toks.get(j)?, toks.get(j + 1)?);
        let (lat, pa) = icao_angle(la, b'N', b'S', 2)?;
        let (lon, po) = icao_angle(lo, b'E', b'W', 3)?;
        return Some((lat, lon, pa, po));
    }
    None
}

/// Compares a point with the position `rawSigmet` states; the allowed difference is the stated
/// precision (a minute or a degree) plus the 1e-5 degree storage step.
fn raw_check(raw: Option<&str>, lat_e5: i32, lon_e5: i32) -> u8 {
    let Some((lat, lon, pa, po)) = raw.and_then(raw_position) else {
        return RAW_NONE;
    };
    let dlat = (f64::from(lat_e5) / 1e5 - lat).abs();
    let dlon = (f64::from(lon_e5) / 1e5 - lon).rem_euclid(360.0);
    let dlon = dlon.min(360.0 - dlon);
    if dlat <= pa + 1e-4 && dlon <= po + 1e-4 {
        RAW_AGREE
    } else {
        RAW_DISAGREE
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
    let (geom, rings) = sigmet_geometry(doc, o, geom)?;
    let raw_check = match (geom, rings.first().and_then(|r| r.first())) {
        (GEOM_POINT, Some(&(lat, lon))) => raw_check(opt_str(doc, o, "rawSigmet")?, lat, lon),
        _ => RAW_NONE,
    };
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
        rings,
        raw_check,
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
        raw_check: RAW_NONE,
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
        raw_check: RAW_NONE,
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
        match read(&doc, o).and_then(|r| push_rec(table, &r).map(|()| r)) {
            Ok(r) => {
                stats.accepted += 1;
                if r.geom == GEOM_POINT {
                    stats.points += 1;
                }
                if r.raw_check != RAW_NONE {
                    stats.point_raw_checked += 1;
                }
                if r.raw_check == RAW_DISAGREE {
                    stats.point_raw_disagree += 1;
                }
            }
            Err(e) => count_refusal(&mut stats.refused, e),
        }
    }
    Ok(())
}

/// Describes every record of a hazard file the parser refuses, one line each:
/// `record N <icao> <series>: <reason> (<field>)`, so a refusal can be read against the source
/// record. `src` is [`SRC_SIGMET`], [`SRC_AIRSIGMET`] or [`SRC_GAIRMET`]. Whole-file errors as
/// [`parse_sigmet`].
pub fn describe_hazard_refusals(input: &[u8], src: u8) -> Result<Vec<String>, ParseError> {
    let read: fn(&Doc<'_>, NodeIx) -> Result<Rec, Refusal> = match src {
        SRC_AIRSIGMET => read_airsigmet,
        SRC_GAIRMET => read_gairmet,
        _ => read_sigmet,
    };
    let doc = parse(input, &DEFAULT_LIMITS)?;
    let (arr, _) = top_array(&doc, 0, MAX_RECORDS_PER_FILE, MAX_TABLE_ROWS)?;
    let mut scratch = new_hazard_table();
    let mut out = Vec::new();
    for (n, o) in doc.elements(arr).enumerate() {
        if let Err(e) = read(&doc, o).and_then(|r| push_rec(&mut scratch, &r)) {
            let id = |k: &'static str| opt_str(&doc, o, k).ok().flatten().unwrap_or("?");
            out.push(format!(
                "record {n} {} {}: {} ({})",
                id("icaoId"),
                id("seriesId"),
                REASON_NAMES.get(e.reason as usize).copied().unwrap_or("?"),
                e.field
            ));
        }
    }
    Ok(out)
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

fn in_band(t: &HazardTable, i: usize, alt_ft: Option<i32>) -> bool {
    let Some(a) = alt_ft else { return true };
    let (b, tp) = (
        t.base_ft.get(i).copied().unwrap_or(NO_ALT),
        t.top_ft.get(i).copied().unwrap_or(NO_ALT),
    );
    !((b != NO_ALT && a < b) || (tp != NO_ALT && a > tp))
}

/// Great-circle distance between two positions in degrees, metres (spherical Earth, 6 371 km).
fn distance_m(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
    let dl = (lon2 - lon1).to_radians();
    let a = ((p2 - p1) / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dl / 2.0).sin().powi(2);
    2.0 * 6_371_000.0 * a.sqrt().min(1.0).asin()
}

/// Replaces `out` with the point hazards ([`GEOM_POINT`]) within `max_dist_m` metres
/// (great circle, inclusive) of the position and, when `alt_ft` is given, whose band contains it
/// (as [`hazard_covers`]). ICAO states no extent for a point report, so how far counts as near is
/// the caller's choice; a negative or non-finite distance returns nothing. Not time-aware;
/// combine with [`active_hazards_at`].
pub fn point_hazards_near(
    t: &HazardTable,
    lat: f64,
    lon: f64,
    max_dist_m: f64,
    alt_ft: Option<i32>,
    out: &mut Vec<u32>,
) {
    out.clear();
    if !max_dist_m.is_finite() || max_dist_m < 0.0 {
        return;
    }
    for i in 0..hazard_len(t) {
        if t.geom.get(i) != Some(&GEOM_POINT) || !in_band(t, i, alt_ft) {
            continue;
        }
        let Some(p) = hazard_rings(t, i)
            .next()
            .and_then(|r| ring_points(t, r).next())
        else {
            continue;
        };
        let (Some(&la), Some(&lo)) = (t.lat_e5.get(p), t.lon_e5.get(p)) else {
            continue;
        };
        if distance_m(lat, lon, f64::from(la) / 1e5, f64::from(lo) / 1e5) <= max_dist_m {
            out.push(i as u32);
        }
    }
}

/// Whether hazard row `i` covers the point: inside one of its rings (even-odd rule, planar in
/// degrees) and, when `alt_ft` is given, within `base_ft..=top_ft` (an absent bound is open;
/// a G-AIRMET base of `FZL` is treated as open because the freezing level varies). A `LINE`
/// covers nothing. Not time-aware; combine with [`active_hazards_at`].
pub fn hazard_covers(t: &HazardTable, i: usize, lat: f64, lon: f64, alt_ft: Option<i32>) -> bool {
    if matches!(t.geom.get(i), Some(&(GEOM_LINE | GEOM_POINT))) || i >= hazard_len(t) {
        return false;
    }
    if !in_band(t, i, alt_ft) {
        return false;
    }
    hazard_rings(t, i).any(|r| {
        [lon, lon + 360.0, lon - 360.0]
            .iter()
            .any(|&x| inside(t, r, lat, x))
    })
}
