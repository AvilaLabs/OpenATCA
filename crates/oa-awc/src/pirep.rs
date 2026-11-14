//! Pilot reports (`/api/data/pirep?format=json`) as a struct-of-arrays table, for checking
//! trajectory predictions against what aircraft actually flew and met.
//!
//! * Position is the API's `lat`/`lon` (the report's resolved location), degrees.
//! * `fltLvl` is in hundreds of feet. The API gives 0 with `fltLvlType` `DURD` (during descent)
//!   or `DURC` (during climb) when the altitude is not one level; those and any 0 are stored as
//!   `NONE_U16` with the type kept in `fl_type`. Bound: 0 to 600 (60 000 ft).
//! * An AIREP's `acType` carries the flight identification (`UAL378`), stored as no type. The
//!   API's intensity `NEGclr` is read as `NEG`.
//! * Icing and turbulence have two layers in the API; base, top and intensity are kept for both,
//!   type and frequency for layer 1 only. Bases and tops are hundreds of feet.
//! * De-duplication key: (`obsTime`, `lat`, `lon`, `acType`, `rawOb`); the copy kept has the
//!   greatest `receiptTime`. Counted, order-independent.

use crate::error::{count_refusal, refuse, ParseError, Reason, Refusal, RefusedCounts};
use crate::json::{opt_f64, opt_int, opt_str, req_f64, req_int, req_str, require_object};
use crate::metar::deg_e5;
use crate::pool::{new_pool, pool_get, pool_push, pool_select, select, top_array, Pool, StrIx};
use crate::units::{
    pack_ident, parse_iso_ms, MAX_TIME_S, MIN_TIME_S, NONE_I16, NONE_U16, NO_IDENT,
};
use crate::{MAX_RAW_BYTES, MAX_RECORDS_PER_FILE, MAX_TABLE_ROWS};
use oa_json::{parse, Doc, NodeIx, DEFAULT_LIMITS};

/// Intensity words in code order: code 0 is absent, then the listed words.
pub const INTENSITY_NAMES: [&str; 11] = [
    "",
    "NEG",
    "TRC",
    "TRC-LGT",
    "LGT",
    "LGT-MOD",
    "MOD",
    "MOD-SEV",
    "SEV",
    "SEV-EXTRM",
    "EXTRM",
];
/// Icing type words in code order (0 absent).
pub const ICE_TYPE_NAMES: [&str; 4] = ["", "RIME", "CLEAR", "MIXED"];
/// Turbulence type words in code order (0 absent).
pub const TURB_TYPE_NAMES: [&str; 4] = ["", "CHOP", "CAT", "LLWS"];
/// Turbulence frequency words in code order (0 absent).
pub const FREQ_NAMES: [&str; 5] = ["", "ISOL", "OCNL", "INT", "CONT"];
/// Flight-level type words (`fl_type` code order): `OTHER` is a stated level.
pub const FL_TYPE_NAMES: [&str; 4] = ["OTHER", "DURD", "DURC", "UNKN"];
/// Report type words (`ptype` code order).
pub const PIREP_TYPE_NAMES: [&str; 3] = ["PIREP", "AIREP", "Urgent PIREP"];
/// Bound: largest accepted flight level, hundreds of feet.
pub const MAX_FL: i64 = 600;

/// PIREP rows, one `Vec` per field sharing an index; sorted by (obs_s, lat, lon, ...) after
/// [`finalize_pirep`].
// A7.2: cold struct (one per load; the columns are the hot data), size guard waived
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PirepTable {
    pub obs_s: Vec<i64>,
    pub receipt_ms: Vec<i64>,
    pub lat_e5: Vec<i32>,
    pub lon_e5: Vec<i32>,
    /// Flight level, hundreds of feet; `NONE_U16` if the report states no single level.
    pub fl: Vec<u16>,
    pub fl_type: Vec<u8>,
    pub ptype: Vec<u8>,
    /// Packed aircraft type (`unpack_ident`), `NO_IDENT` if none.
    pub ac_type: Vec<u32>,
    /// Packed reporting station, `NO_IDENT` if none.
    pub station: Vec<u32>,
    /// Temperature, whole degrees C in tenths-free `i16`; `NONE_I16` if absent.
    pub temp_c: Vec<i16>,
    pub wdir_deg: Vec<u16>,
    pub wspd_kt: Vec<u16>,
    pub ice_base: Vec<u16>,
    pub ice_top: Vec<u16>,
    pub ice_int: Vec<u8>,
    pub ice_type: Vec<u8>,
    pub ice_base2: Vec<u16>,
    pub ice_top2: Vec<u16>,
    pub ice_int2: Vec<u8>,
    pub turb_base: Vec<u16>,
    pub turb_top: Vec<u16>,
    pub turb_int: Vec<u8>,
    pub turb_type: Vec<u8>,
    pub turb_freq: Vec<u8>,
    pub turb_base2: Vec<u16>,
    pub turb_top2: Vec<u16>,
    pub turb_int2: Vec<u8>,
    pub raw: Pool,
}

/// Counts for the PIREP files parsed so far.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PirepStats {
    pub files: u32,
    pub records: u32,
    pub accepted: u32,
    pub refused: RefusedCounts,
}

/// What [`finalize_pirep`] removed.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PirepDedupe {
    pub rows_in: u32,
    pub rows_kept: u32,
    pub duplicates: u32,
}

/// An empty table.
pub fn new_pirep_table() -> PirepTable {
    PirepTable {
        obs_s: Vec::new(),
        receipt_ms: Vec::new(),
        lat_e5: Vec::new(),
        lon_e5: Vec::new(),
        fl: Vec::new(),
        fl_type: Vec::new(),
        ptype: Vec::new(),
        ac_type: Vec::new(),
        station: Vec::new(),
        temp_c: Vec::new(),
        wdir_deg: Vec::new(),
        wspd_kt: Vec::new(),
        ice_base: Vec::new(),
        ice_top: Vec::new(),
        ice_int: Vec::new(),
        ice_type: Vec::new(),
        ice_base2: Vec::new(),
        ice_top2: Vec::new(),
        ice_int2: Vec::new(),
        turb_base: Vec::new(),
        turb_top: Vec::new(),
        turb_int: Vec::new(),
        turb_type: Vec::new(),
        turb_freq: Vec::new(),
        turb_base2: Vec::new(),
        turb_top2: Vec::new(),
        turb_int2: Vec::new(),
        raw: new_pool(),
    }
}

/// Number of rows.
pub fn pirep_len(t: &PirepTable) -> usize {
    t.obs_s.len()
}

/// The `rawOb` text of row `i`.
pub fn pirep_raw(t: &PirepTable, i: usize) -> &str {
    pool_get(&t.raw, StrIx(i as u32))
}

/// Flight level of row `i` in feet, if it states one level.
pub fn pirep_alt_ft(t: &PirepTable, i: usize) -> Option<i32> {
    match t.fl.get(i) {
        Some(&v) if v != NONE_U16 => Some(crate::units::fl_to_ft(v)),
        _ => None,
    }
}

fn code(doc: &Doc<'_>, o: NodeIx, key: &'static str, names: &[&str]) -> Result<u8, Refusal> {
    match opt_str(doc, o, key)? {
        None | Some("") => Ok(0),
        // The API writes "NEGclr" (negative, clear) for an intensity: read as NEG.
        Some(s) => match names
            .iter()
            .position(|&n| n == s || (n == "NEG" && s == "NEGclr"))
        {
            Some(i) => Ok(i as u8),
            None => refuse(Reason::UnknownCode, key),
        },
    }
}

fn level(doc: &Doc<'_>, o: NodeIx, key: &'static str) -> Result<u16, Refusal> {
    Ok(opt_int(doc, o, key, 0, MAX_FL)?.map_or(NONE_U16, |v| v as u16))
}

fn band(b: u16, t: u16, field: &'static str) -> Result<(), Refusal> {
    if b != NONE_U16 && t != NONE_U16 && b > t {
        return refuse(Reason::ReversedBand, field);
    }
    Ok(())
}

fn push_record(t: &mut PirepTable, doc: &Doc<'_>, o: NodeIx) -> Result<(), Refusal> {
    require_object(doc, o)?;
    let obs_s = req_int(doc, o, "obsTime", MIN_TIME_S, MAX_TIME_S)?;
    let Some(receipt_ms) = parse_iso_ms(req_str(doc, o, "receiptTime")?) else {
        return refuse(Reason::BadTime, "receiptTime");
    };
    let lat_e5 = deg_e5(req_f64(doc, o, "lat")?, 90.0, "lat")?;
    let lon_e5 = deg_e5(req_f64(doc, o, "lon")?, 180.0, "lon")?;
    let raw = req_str(doc, o, "rawOb")?;
    if raw.len() > MAX_RAW_BYTES {
        return refuse(Reason::OutOfRange, "rawOb");
    }
    let ac_type = match opt_str(doc, o, "acType")? {
        None | Some("") => NO_IDENT,
        Some(s) => match pack_ident(s, 2) {
            Some(v) => v,
            // An AIREP puts the flight identification here (`UAL378`): kept as "no type".
            None if s.len() <= 8
                && s.bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit()) =>
            {
                NO_IDENT
            }
            None => return refuse(Reason::BadIdent, "acType"),
        },
    };
    let station = match opt_str(doc, o, "icaoId")? {
        None | Some("") => NO_IDENT,
        Some(s) => match pack_ident(s, 3) {
            Some(v) => v,
            None => return refuse(Reason::BadIdent, "icaoId"),
        },
    };
    let fl = level(doc, o, "fltLvl")?;
    let fl = if fl == 0 { NONE_U16 } else { fl };
    let s = req_str(doc, o, "fltLvlType")?;
    let Some(fl_type) = FL_TYPE_NAMES.iter().position(|&n| n == s) else {
        return refuse(Reason::UnknownCode, "fltLvlType");
    };
    let s = req_str(doc, o, "pirepType")?;
    let Some(ptype) = PIREP_TYPE_NAMES.iter().position(|&n| n == s) else {
        return refuse(Reason::UnknownCode, "pirepType");
    };
    let (fl_type, ptype) = (fl_type as u8, ptype as u8);
    let temp_c = match opt_f64(doc, o, "temp")? {
        None => NONE_I16,
        Some(v) if (-90.0..=60.0).contains(&v) => v.round() as i16,
        Some(_) => return refuse(Reason::OutOfRange, "temp"),
    };
    let wdir_deg = opt_int(doc, o, "wdir", 0, 360)?.map_or(NONE_U16, |v| v as u16);
    let wspd_kt = opt_int(doc, o, "wspd", 0, 250)?.map_or(NONE_U16, |v| v as u16);
    let (ice_base, ice_top) = (level(doc, o, "icgBas1")?, level(doc, o, "icgTop1")?);
    let (ice_base2, ice_top2) = (level(doc, o, "icgBas2")?, level(doc, o, "icgTop2")?);
    let (turb_base, turb_top) = (level(doc, o, "tbBas1")?, level(doc, o, "tbTop1")?);
    let (turb_base2, turb_top2) = (level(doc, o, "tbBas2")?, level(doc, o, "tbTop2")?);
    band(ice_base, ice_top, "icgBas1")?;
    band(ice_base2, ice_top2, "icgBas2")?;
    band(turb_base, turb_top, "tbBas1")?;
    band(turb_base2, turb_top2, "tbBas2")?;
    let ice_int = code(doc, o, "icgInt1", &INTENSITY_NAMES)?;
    let ice_type = code(doc, o, "icgType1", &ICE_TYPE_NAMES)?;
    let ice_int2 = code(doc, o, "icgInt2", &INTENSITY_NAMES)?;
    let turb_int = code(doc, o, "tbInt1", &INTENSITY_NAMES)?;
    let turb_type = code(doc, o, "tbType1", &TURB_TYPE_NAMES)?;
    let turb_freq = code(doc, o, "tbFreq1", &FREQ_NAMES)?;
    let turb_int2 = code(doc, o, "tbInt2", &INTENSITY_NAMES)?;
    if pool_push(&mut t.raw, raw).is_none() {
        return refuse(Reason::OutOfRange, "rawOb");
    }
    t.obs_s.push(obs_s);
    t.receipt_ms.push(receipt_ms);
    t.lat_e5.push(lat_e5);
    t.lon_e5.push(lon_e5);
    t.fl.push(fl);
    t.fl_type.push(fl_type);
    t.ptype.push(ptype);
    t.ac_type.push(ac_type);
    t.station.push(station);
    t.temp_c.push(temp_c);
    t.wdir_deg.push(wdir_deg);
    t.wspd_kt.push(wspd_kt);
    t.ice_base.push(ice_base);
    t.ice_top.push(ice_top);
    t.ice_int.push(ice_int);
    t.ice_type.push(ice_type);
    t.ice_base2.push(ice_base2);
    t.ice_top2.push(ice_top2);
    t.ice_int2.push(ice_int2);
    t.turb_base.push(turb_base);
    t.turb_top.push(turb_top);
    t.turb_int.push(turb_int);
    t.turb_type.push(turb_type);
    t.turb_freq.push(turb_freq);
    t.turb_base2.push(turb_base2);
    t.turb_top2.push(turb_top2);
    t.turb_int2.push(turb_int2);
    Ok(())
}

/// Parses one PIREP JSON file and appends its accepted rows (unsorted). File-level refusals and
/// per-record refusals work as in [`crate::parse_metar`]. A refused record appends nothing.
pub fn parse_pirep(
    input: &[u8],
    table: &mut PirepTable,
    stats: &mut PirepStats,
) -> Result<(), ParseError> {
    let doc = parse(input, &DEFAULT_LIMITS)?;
    let (arr, n) = top_array(&doc, pirep_len(table), MAX_RECORDS_PER_FILE, MAX_TABLE_ROWS)?;
    stats.files += 1;
    stats.records = stats.records.saturating_add(n as u32);
    for o in doc.elements(arr) {
        match push_record(table, &doc, o) {
            Ok(()) => stats.accepted += 1,
            Err(e) => count_refusal(&mut stats.refused, e),
        }
    }
    Ok(())
}

/// Sorts by (obs_s, lat, lon, aircraft type, text, receipt) and removes duplicates under the
/// key in the module comment, keeping the greatest `receiptTime`.
pub fn finalize_pirep(t: &mut PirepTable) -> PirepDedupe {
    let n = pirep_len(t);
    let mut order: Vec<u32> = (0..n as u32).collect();
    let key = |i: u32| {
        let i = i as usize;
        (
            t.obs_s.get(i).copied().unwrap_or(0),
            t.lat_e5.get(i).copied().unwrap_or(0),
            t.lon_e5.get(i).copied().unwrap_or(0),
            t.ac_type.get(i).copied().unwrap_or(0),
        )
    };
    order.sort_by(|&a, &b| {
        key(a)
            .cmp(&key(b))
            .then_with(|| pirep_raw(t, a as usize).cmp(pirep_raw(t, b as usize)))
            .then_with(|| {
                t.receipt_ms
                    .get(a as usize)
                    .cmp(&t.receipt_ms.get(b as usize))
            })
    });
    let mut keep: Vec<u32> = Vec::with_capacity(n);
    let mut rep = PirepDedupe {
        rows_in: n as u32,
        ..PirepDedupe::default()
    };
    for &i in &order {
        let same = keep.last().is_some_and(|&p| {
            key(p) == key(i) && pirep_raw(t, p as usize) == pirep_raw(t, i as usize)
        });
        if same {
            rep.duplicates += 1;
            if let Some(last) = keep.last_mut() {
                *last = i;
            }
        } else {
            keep.push(i);
        }
    }
    rep.rows_kept = keep.len() as u32;
    let raw = pool_select(&t.raw, &keep);
    select(&mut t.obs_s, &keep);
    select(&mut t.receipt_ms, &keep);
    select(&mut t.lat_e5, &keep);
    select(&mut t.lon_e5, &keep);
    select(&mut t.fl, &keep);
    select(&mut t.fl_type, &keep);
    select(&mut t.ptype, &keep);
    select(&mut t.ac_type, &keep);
    select(&mut t.station, &keep);
    select(&mut t.temp_c, &keep);
    select(&mut t.wdir_deg, &keep);
    select(&mut t.wspd_kt, &keep);
    select(&mut t.ice_base, &keep);
    select(&mut t.ice_top, &keep);
    select(&mut t.ice_int, &keep);
    select(&mut t.ice_type, &keep);
    select(&mut t.ice_base2, &keep);
    select(&mut t.ice_top2, &keep);
    select(&mut t.ice_int2, &keep);
    select(&mut t.turb_base, &keep);
    select(&mut t.turb_top, &keep);
    select(&mut t.turb_int, &keep);
    select(&mut t.turb_type, &keep);
    select(&mut t.turb_freq, &keep);
    select(&mut t.turb_base2, &keep);
    select(&mut t.turb_top2, &keep);
    select(&mut t.turb_int2, &keep);
    t.raw = raw;
    rep
}
