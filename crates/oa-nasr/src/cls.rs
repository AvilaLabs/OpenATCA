//! Class airspace designations per airport: `CLS_ARSP`.

use crate::row::{parse_rows, Row};
use crate::{NasrError, Parsed};
use std::io::BufRead;

/// A row of `CLS_ARSP`: which classes of airspace an airport has.
#[derive(Clone, Debug, PartialEq)]
pub struct ClassAirspace {
    /// 1-based line in the file.
    pub line: u64,
    /// `EFF_DATE`.
    pub eff_date: String,
    /// `SITE_NO`, joins to the airport's `SITE_NO`.
    pub site_no: String,
    /// `SITE_TYPE_CODE`.
    pub site_type_code: String,
    /// `STATE_CODE`.
    pub state_code: Option<String>,
    /// `ARPT_ID`.
    pub arpt_id: String,
    /// `CITY`.
    pub city: String,
    /// `COUNTRY_CODE`.
    pub country_code: String,
    /// `CLASS_B_AIRSPACE` is `Y`.
    pub class_b: bool,
    /// `CLASS_C_AIRSPACE` is `Y`.
    pub class_c: bool,
    /// `CLASS_D_AIRSPACE` is `Y`.
    pub class_d: bool,
    /// `CLASS_E_AIRSPACE` is `Y`.
    pub class_e: bool,
    /// `AIRSPACE_HRS`, free text such as part-time hours.
    pub airspace_hrs: Option<String>,
    /// `REMARK`.
    pub remark: Option<String>,
}

/// Parses `CLS_ARSP`. The class columns are `Y` or empty.
pub fn parse_class_airspace<R: BufRead>(
    source: &str,
    reader: R,
) -> Result<Parsed<ClassAirspace>, NasrError> {
    parse_rows(source, reader, |r: &Row| {
        Ok(ClassAirspace {
            line: r.line(),
            eff_date: r.req("EFF_DATE")?,
            site_no: r.req("SITE_NO")?,
            site_type_code: r.req("SITE_TYPE_CODE")?,
            state_code: r.opt("STATE_CODE")?,
            arpt_id: r.req("ARPT_ID")?,
            city: r.req("CITY")?,
            country_code: r.req("COUNTRY_CODE")?,
            class_b: r.flag_of("CLASS_B_AIRSPACE", "Y", None)?,
            class_c: r.flag_of("CLASS_C_AIRSPACE", "Y", None)?,
            class_d: r.flag_of("CLASS_D_AIRSPACE", "Y", None)?,
            class_e: r.flag_of("CLASS_E_AIRSPACE", "Y", None)?,
            airspace_hrs: r.opt("AIRSPACE_HRS")?,
            remark: r.opt("REMARK")?,
        })
    })
}
