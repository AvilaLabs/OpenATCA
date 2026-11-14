//! Navaids: `NAV_BASE` (VOR, VORTAC, VOR/DME, NDB, TACAN, DME, ...).
//!
//! Only the columns the airspace model needs are typed, ordered as in the file. A navaid is
//! identified by `NAV_ID` plus `NAV_TYPE` plus `STATE_CODE`/`CITY` (the same ID recurs).

use crate::row::{parse_rows, PosCols, Row, STD_POS};
use crate::{NasrError, Parsed, Position};
use std::io::BufRead;

const TACAN_DME_POS: PosCols = PosCols {
    lat_deg: "TACAN_DME_LAT_DEG",
    lat_min: "TACAN_DME_LAT_MIN",
    lat_sec: "TACAN_DME_LAT_SEC",
    lat_hemis: "TACAN_DME_LAT_HEMIS",
    lat_dec: "TACAN_DME_LAT_DECIMAL",
    lon_deg: "TACAN_DME_LONG_DEG",
    lon_min: "TACAN_DME_LONG_MIN",
    lon_sec: "TACAN_DME_LONG_SEC",
    lon_hemis: "TACAN_DME_LONG_HEMIS",
    lon_dec: "TACAN_DME_LONG_DECIMAL",
};

/// A row of `NAV_BASE`.
#[derive(Clone, Debug, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct Navaid {
    /// 1-based line in the file.
    pub line: u64,
    /// `EFF_DATE`.
    pub eff_date: String,
    /// `NAV_ID`.
    pub nav_id: String,
    /// `NAV_TYPE`, e.g. `VORTAC`, `NDB`.
    pub nav_type: String,
    /// `STATE_CODE`.
    pub state_code: Option<String>,
    /// `CITY`.
    pub city: String,
    /// `COUNTRY_CODE`.
    pub country_code: String,
    /// `NAV_STATUS`, e.g. `OPERATIONAL IFR`.
    pub nav_status: String,
    /// `NAME`.
    pub name: String,
    /// `REGION_CODE`.
    pub region_code: Option<String>,
    /// `NAS_USE_FLAG`.
    pub nas_use_flag: bool,
    /// `PUBLIC_USE_FLAG`.
    pub public_use_flag: bool,
    /// `HIGH_ALT_ARTCC_ID`.
    pub high_alt_artcc_id: Option<String>,
    /// `LOW_ALT_ARTCC_ID`.
    pub low_alt_artcc_id: Option<String>,
    /// The navaid's position.
    pub position: Position,
    /// Separate TACAN/DME antenna position, when published.
    pub tacan_dme_position: Option<Position>,
    /// `ELEV`, feet.
    pub elev: Option<f64>,
    /// `MAG_VARN`, degrees.
    pub mag_varn: Option<f64>,
    /// `MAG_VARN_HEMIS`: `E` or `W`.
    pub mag_varn_hemis: Option<String>,
    /// `MAG_VARN_YEAR`.
    pub mag_varn_year: Option<i64>,
    /// `CHAN`, TACAN/DME channel.
    pub chan: Option<String>,
    /// `FREQ`, MHz (VOR) or kHz (NDB).
    pub freq: Option<f64>,
    /// `VOICE_CALL`.
    pub voice_call: Option<String>,
    /// `ALT_CODE`: `H`, `L`, `T`, ... service volume class.
    pub alt_code: Option<String>,
    /// `DME_SSV`: DME standard service volume.
    pub dme_ssv: Option<String>,
}

/// Parses `NAV_BASE`.
pub fn parse_navaids<R: BufRead>(source: &str, reader: R) -> Result<Parsed<Navaid>, NasrError> {
    parse_rows(source, reader, |r: &Row| {
        Ok(Navaid {
            line: r.line(),
            eff_date: r.req("EFF_DATE")?,
            nav_id: r.req("NAV_ID")?,
            nav_type: r.req("NAV_TYPE")?,
            state_code: r.opt("STATE_CODE")?,
            city: r.req("CITY")?,
            country_code: r.req("COUNTRY_CODE")?,
            nav_status: r.req("NAV_STATUS")?,
            name: r.req("NAME")?,
            region_code: r.opt("REGION_CODE")?,
            nas_use_flag: r.flag("NAS_USE_FLAG")?,
            public_use_flag: r.flag("PUBLIC_USE_FLAG")?,
            high_alt_artcc_id: r.opt("HIGH_ALT_ARTCC_ID")?,
            low_alt_artcc_id: r.opt("LOW_ALT_ARTCC_ID")?,
            position: r.position(&STD_POS)?,
            tacan_dme_position: r.opt_position(&TACAN_DME_POS)?,
            elev: r.opt_f64("ELEV")?,
            mag_varn: r.opt_f64("MAG_VARN")?,
            mag_varn_hemis: r.opt("MAG_VARN_HEMIS")?,
            mag_varn_year: r.opt_i64("MAG_VARN_YEAR")?,
            chan: r.opt("CHAN")?,
            freq: r.opt_f64("FREQ")?,
            voice_call: r.opt("VOICE_CALL")?,
            alt_code: r.opt("ALT_CODE")?,
            dme_ssv: r.opt("DME_SSV")?,
        })
    })
}
