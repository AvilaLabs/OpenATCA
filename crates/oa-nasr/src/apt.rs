//! Airports: `APT_BASE`, `APT_RWY` (runways) and `APT_RWY_END` (runway ends).
//!
//! An airport is identified by `SITE_NO` (unique per facility record) and `ARPT_ID`. Runways and
//! runway ends carry the airport's `SITE_NO`, `ARPT_ID` and the runway's `RWY_ID` (`18/36`) so
//! they can be joined to their airport. Only the columns the airspace model needs are typed,
//! ordered as in the file.

use crate::row::{parse_rows, PosCols, Row, STD_POS};
use crate::{NasrError, Parsed, Position};
use std::io::BufRead;

const RWY_END_POS: PosCols = PosCols {
    lat_deg: "RWY_END_LAT_DEG",
    lat_min: "RWY_END_LAT_MIN",
    lat_sec: "RWY_END_LAT_SEC",
    lat_hemis: "RWY_END_LAT_HEMIS",
    lat_dec: "LAT_DECIMAL",
    lon_deg: "RWY_END_LONG_DEG",
    lon_min: "RWY_END_LONG_MIN",
    lon_sec: "RWY_END_LONG_SEC",
    lon_hemis: "RWY_END_LONG_HEMIS",
    lon_dec: "LONG_DECIMAL",
};

const DISPLACED_THR_POS: PosCols = PosCols {
    lat_deg: "DISPLACED_THR_LAT_DEG",
    lat_min: "DISPLACED_THR_LAT_MIN",
    lat_sec: "DISPLACED_THR_LAT_SEC",
    lat_hemis: "DISPLACED_THR_LAT_HEMIS",
    lat_dec: "LAT_DISPLACED_THR_DECIMAL",
    lon_deg: "DISPLACED_THR_LONG_DEG",
    lon_min: "DISPLACED_THR_LONG_MIN",
    lon_sec: "DISPLACED_THR_LONG_SEC",
    lon_hemis: "DISPLACED_THR_LONG_HEMIS",
    lon_dec: "LONG_DISPLACED_THR_DECIMAL",
};

/// A row of `APT_BASE`.
#[derive(Clone, Debug, PartialEq)]
pub struct Airport {
    /// 1-based line in the file.
    pub line: u64,
    /// `EFF_DATE`.
    pub eff_date: String,
    /// `SITE_NO`.
    pub site_no: String,
    /// `SITE_TYPE_CODE`: `A` airport, `H` heliport, `C` seaplane base, `U` ultralight,
    /// `G` gliderport, `B` balloonport.
    pub site_type_code: String,
    /// `STATE_CODE`.
    pub state_code: Option<String>,
    /// `ARPT_ID`, e.g. `ORD`.
    pub arpt_id: String,
    /// `CITY`.
    pub city: String,
    /// `COUNTRY_CODE`.
    pub country_code: String,
    /// `REGION_CODE`.
    pub region_code: Option<String>,
    /// `ARPT_NAME`.
    pub arpt_name: String,
    /// `FACILITY_USE_CODE`: `PU` public, `PR` private.
    pub facility_use_code: String,
    /// Airport reference point.
    pub position: Position,
    /// `ELEV`, feet MSL.
    pub elev: f64,
    /// `MAG_VARN`, degrees.
    pub mag_varn: Option<f64>,
    /// `MAG_HEMIS`: `E` or `W`.
    pub mag_hemis: Option<String>,
    /// `MAG_VARN_YEAR`.
    pub mag_varn_year: Option<i64>,
    /// `TPA`, traffic pattern altitude, feet.
    pub tpa: Option<i64>,
    /// `RESP_ARTCC_ID`.
    pub resp_artcc_id: String,
    /// `COMPUTER_ID`.
    pub computer_id: String,
    /// `ARPT_STATUS`.
    pub arpt_status: String,
    /// `TWR_TYPE_CODE`.
    pub twr_type_code: String,
    /// `ICAO_ID`.
    pub icao_id: Option<String>,
}

/// A row of `APT_RWY`.
#[derive(Clone, Debug, PartialEq)]
pub struct Runway {
    /// 1-based line in the file.
    pub line: u64,
    /// `EFF_DATE`.
    pub eff_date: String,
    /// `SITE_NO`.
    pub site_no: String,
    /// `SITE_TYPE_CODE`.
    pub site_type_code: String,
    /// `STATE_CODE`.
    pub state_code: Option<String>,
    /// `ARPT_ID`.
    pub arpt_id: String,
    /// `RWY_ID`, e.g. `10L/28R`.
    pub rwy_id: String,
    /// `RWY_LEN`, feet.
    pub rwy_len: f64,
    /// `RWY_WIDTH`, feet.
    pub rwy_width: f64,
    /// `SURFACE_TYPE_CODE`.
    pub surface_type_code: Option<String>,
    /// `COND`.
    pub cond: Option<String>,
    /// `RWY_LGT_CODE`.
    pub rwy_lgt_code: Option<String>,
}

/// A row of `APT_RWY_END`.
#[derive(Clone, Debug, PartialEq)]
pub struct RunwayEnd {
    /// 1-based line in the file.
    pub line: u64,
    /// `EFF_DATE`.
    pub eff_date: String,
    /// `SITE_NO`.
    pub site_no: String,
    /// `ARPT_ID`.
    pub arpt_id: String,
    /// `RWY_ID`, the runway this end belongs to.
    pub rwy_id: String,
    /// `RWY_END_ID`, e.g. `10L`.
    pub rwy_end_id: String,
    /// `TRUE_ALIGNMENT`, degrees true.
    pub true_alignment: Option<f64>,
    /// `ILS_TYPE`.
    pub ils_type: Option<String>,
    /// Runway end (threshold) position; not published for every end.
    pub position: Option<Position>,
    /// `RWY_END_ELEV`, feet.
    pub rwy_end_elev: Option<f64>,
    /// `THR_CROSSING_HGT`, feet.
    pub thr_crossing_hgt: Option<f64>,
    /// `VISUAL_GLIDE_PATH_ANGLE`, degrees.
    pub visual_glide_path_angle: Option<f64>,
    /// Displaced threshold position, when there is one.
    pub displaced_thr_position: Option<Position>,
    /// `DISPLACED_THR_ELEV`, feet.
    pub displaced_thr_elev: Option<f64>,
    /// `DISPLACED_THR_LEN`, feet.
    pub displaced_thr_len: Option<f64>,
    /// `TDZ_ELEV`, feet.
    pub tdz_elev: Option<f64>,
    /// `TKOF_RUN_AVBL`, feet (TORA).
    pub tkof_run_avbl: Option<f64>,
    /// `TKOF_DIST_AVBL`, feet (TODA).
    pub tkof_dist_avbl: Option<f64>,
    /// `ACLT_STOP_DIST_AVBL`, feet (ASDA).
    pub aclt_stop_dist_avbl: Option<f64>,
    /// `LNDG_DIST_AVBL`, feet (LDA).
    pub lndg_dist_avbl: Option<f64>,
}

/// Parses `APT_BASE`.
pub fn parse_airports<R: BufRead>(source: &str, reader: R) -> Result<Parsed<Airport>, NasrError> {
    parse_rows(source, reader, |r: &Row| {
        Ok(Airport {
            line: r.line(),
            eff_date: r.req("EFF_DATE")?,
            site_no: r.req("SITE_NO")?,
            site_type_code: r.req("SITE_TYPE_CODE")?,
            state_code: r.opt("STATE_CODE")?,
            arpt_id: r.req("ARPT_ID")?,
            city: r.req("CITY")?,
            country_code: r.req("COUNTRY_CODE")?,
            region_code: r.opt("REGION_CODE")?,
            arpt_name: r.req("ARPT_NAME")?,
            facility_use_code: r.req("FACILITY_USE_CODE")?,
            position: r.position(&STD_POS)?,
            elev: r.req_f64("ELEV")?,
            mag_varn: r.opt_f64("MAG_VARN")?,
            mag_hemis: r.opt("MAG_HEMIS")?,
            mag_varn_year: r.opt_i64("MAG_VARN_YEAR")?,
            tpa: r.opt_i64("TPA")?,
            resp_artcc_id: r.req("RESP_ARTCC_ID")?,
            computer_id: r.req("COMPUTER_ID")?,
            arpt_status: r.req("ARPT_STATUS")?,
            twr_type_code: r.req("TWR_TYPE_CODE")?,
            icao_id: r.opt("ICAO_ID")?,
        })
    })
}

/// Parses `APT_RWY`.
pub fn parse_runways<R: BufRead>(source: &str, reader: R) -> Result<Parsed<Runway>, NasrError> {
    parse_rows(source, reader, |r: &Row| {
        Ok(Runway {
            line: r.line(),
            eff_date: r.req("EFF_DATE")?,
            site_no: r.req("SITE_NO")?,
            site_type_code: r.req("SITE_TYPE_CODE")?,
            state_code: r.opt("STATE_CODE")?,
            arpt_id: r.req("ARPT_ID")?,
            rwy_id: r.req("RWY_ID")?,
            rwy_len: r.req_f64("RWY_LEN")?,
            rwy_width: r.req_f64("RWY_WIDTH")?,
            surface_type_code: r.opt("SURFACE_TYPE_CODE")?,
            cond: r.opt("COND")?,
            rwy_lgt_code: r.opt("RWY_LGT_CODE")?,
        })
    })
}

/// Parses `APT_RWY_END`.
pub fn parse_runway_ends<R: BufRead>(
    source: &str,
    reader: R,
) -> Result<Parsed<RunwayEnd>, NasrError> {
    parse_rows(source, reader, |r: &Row| {
        Ok(RunwayEnd {
            line: r.line(),
            eff_date: r.req("EFF_DATE")?,
            site_no: r.req("SITE_NO")?,
            arpt_id: r.req("ARPT_ID")?,
            rwy_id: r.req("RWY_ID")?,
            rwy_end_id: r.req("RWY_END_ID")?,
            true_alignment: r.opt_f64("TRUE_ALIGNMENT")?,
            ils_type: r.opt("ILS_TYPE")?,
            position: r.opt_position(&RWY_END_POS)?,
            rwy_end_elev: r.opt_f64("RWY_END_ELEV")?,
            thr_crossing_hgt: r.opt_f64("THR_CROSSING_HGT")?,
            visual_glide_path_angle: r.opt_f64("VISUAL_GLIDE_PATH_ANGLE")?,
            displaced_thr_position: r.opt_position(&DISPLACED_THR_POS)?,
            displaced_thr_elev: r.opt_f64("DISPLACED_THR_ELEV")?,
            displaced_thr_len: r.opt_f64("DISPLACED_THR_LEN")?,
            tdz_elev: r.opt_f64("TDZ_ELEV")?,
            tkof_run_avbl: r.opt_f64("TKOF_RUN_AVBL")?,
            tkof_dist_avbl: r.opt_f64("TKOF_DIST_AVBL")?,
            aclt_stop_dist_avbl: r.opt_f64("ACLT_STOP_DIST_AVBL")?,
            lndg_dist_avbl: r.opt_f64("LNDG_DIST_AVBL")?,
        })
    })
}
