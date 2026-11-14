//! Fixes: `FIX_BASE` (named fixes) and `FIX_NAV` (navaid references of a fix).
//!
//! A fix is identified by `FIX_ID` plus `ICAO_REGION_CODE` (plus `STATE_CODE`, since the same
//! name can recur); all three are kept as published so sources can be matched later.

use crate::row::{parse_rows, Row, STD_POS};
use crate::{NasrError, Parsed, Position};
use std::io::BufRead;

/// A row of `FIX_BASE`.
#[derive(Clone, Debug, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct Fix {
    /// 1-based line in the file.
    pub line: u64,
    /// `EFF_DATE`.
    pub eff_date: String,
    /// `FIX_ID`.
    pub fix_id: String,
    /// `ICAO_REGION_CODE`, e.g. `K5`.
    pub icao_region_code: String,
    /// `STATE_CODE`.
    pub state_code: Option<String>,
    /// `COUNTRY_CODE`.
    pub country_code: String,
    /// The fix's position.
    pub position: Position,
    /// `FIX_ID_OLD`.
    pub fix_id_old: Option<String>,
    /// `CHARTING_REMARK`.
    pub charting_remark: Option<String>,
    /// `FIX_USE_CODE`, e.g. `WP`, `RP`, `CN`, `MR`, `VFR`.
    pub fix_use_code: String,
    /// `ARTCC_ID_HIGH`.
    pub artcc_id_high: Option<String>,
    /// `ARTCC_ID_LOW`. Not nullable in the structure file but empty for two fixes in the
    /// 2026-10-29 cycle, so optional here and flagged by schema validation.
    pub artcc_id_low: Option<String>,
    /// `PITCH_FLAG`.
    pub pitch_flag: bool,
    /// `CATCH_FLAG`.
    pub catch_flag: bool,
    /// `SUA_ATCAA_FLAG`.
    pub sua_atcaa_flag: bool,
    /// `MIN_RECEP_ALT`, feet.
    pub min_recep_alt: Option<i64>,
    /// `COMPULSORY`.
    pub compulsory: Option<String>,
    /// `CHARTS`.
    pub charts: Option<String>,
}

/// A row of `FIX_NAV`: a navaid-relative definition of a fix.
#[derive(Clone, Debug, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct FixNav {
    /// 1-based line in the file.
    pub line: u64,
    /// `EFF_DATE`.
    pub eff_date: String,
    /// `FIX_ID`.
    pub fix_id: String,
    /// `ICAO_REGION_CODE`.
    pub icao_region_code: String,
    /// `STATE_CODE`.
    pub state_code: Option<String>,
    /// `COUNTRY_CODE`.
    pub country_code: String,
    /// `NAV_ID`.
    pub nav_id: String,
    /// `NAV_TYPE`, e.g. `VORTAC`, `LOC`.
    pub nav_type: String,
    /// `BEARING`, degrees.
    pub bearing: Option<f64>,
    /// `DISTANCE`, nautical miles.
    pub distance: Option<f64>,
}

/// Parses `FIX_BASE`.
pub fn parse_fixes<R: BufRead>(source: &str, reader: R) -> Result<Parsed<Fix>, NasrError> {
    parse_rows(source, reader, |r: &Row| {
        Ok(Fix {
            line: r.line(),
            eff_date: r.req("EFF_DATE")?,
            fix_id: r.req("FIX_ID")?,
            icao_region_code: r.req("ICAO_REGION_CODE")?,
            state_code: r.opt("STATE_CODE")?,
            country_code: r.req("COUNTRY_CODE")?,
            position: r.position(&STD_POS)?,
            fix_id_old: r.opt("FIX_ID_OLD")?,
            charting_remark: r.opt("CHARTING_REMARK")?,
            fix_use_code: r.req("FIX_USE_CODE")?,
            artcc_id_high: r.opt("ARTCC_ID_HIGH")?,
            artcc_id_low: r.opt("ARTCC_ID_LOW")?,
            pitch_flag: r.flag("PITCH_FLAG")?,
            catch_flag: r.flag("CATCH_FLAG")?,
            sua_atcaa_flag: r.flag("SUA_ATCAA_FLAG")?,
            min_recep_alt: r.opt_i64("MIN_RECEP_ALT")?,
            compulsory: r.opt("COMPULSORY")?,
            charts: r.opt("CHARTS")?,
        })
    })
}

/// Parses `FIX_NAV`.
pub fn parse_fix_navs<R: BufRead>(source: &str, reader: R) -> Result<Parsed<FixNav>, NasrError> {
    parse_rows(source, reader, |r: &Row| {
        Ok(FixNav {
            line: r.line(),
            eff_date: r.req("EFF_DATE")?,
            fix_id: r.req("FIX_ID")?,
            icao_region_code: r.req("ICAO_REGION_CODE")?,
            state_code: r.opt("STATE_CODE")?,
            country_code: r.req("COUNTRY_CODE")?,
            nav_id: r.req("NAV_ID")?,
            nav_type: r.req("NAV_TYPE")?,
            bearing: r.opt_f64("BEARING")?,
            distance: r.opt_f64("DISTANCE")?,
        })
    })
}
