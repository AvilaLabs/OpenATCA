//! Airways: `AWY_BASE` (one row per airway) and `AWY_SEG_ALT` (segments with altitudes).
//!
//! An airway is identified by `AWY_ID` plus `AWY_LOCATION` (`C` contiguous US, `A` Alaska,
//! `H` Hawaii). [`parse_airways`] joins the two files into [`Airway`]s whose segments are
//! ordered by `POINT_SEQ`.

use crate::row::{parse_rows, Row};
use crate::{NasrError, Parsed, RowError, RowErrorKind};
use std::collections::HashMap;
use std::io::BufRead;

/// An altitude with the direction it applies to (`NORTH`, `EAST`, ...), as published.
#[derive(Clone, Debug, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct AltDir {
    /// Altitude, feet.
    pub alt: Option<i64>,
    /// Direction text.
    pub dir: Option<String>,
}

/// A row of `AWY_BASE`.
#[derive(Clone, Debug, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct AirwayBase {
    /// 1-based line in the file.
    pub line: u64,
    /// `EFF_DATE`.
    pub eff_date: String,
    /// `REGULATORY` is `Y`.
    pub regulatory: bool,
    /// `AWY_DESIGNATION`: `V`, `J`, `RN`, `AT`, `PA`, ...
    pub awy_designation: String,
    /// `AWY_LOCATION`.
    pub awy_location: String,
    /// `AWY_ID`, e.g. `V2`, `J80`.
    pub awy_id: String,
    /// `UPDATE_DATE`.
    pub update_date: String,
    /// `REMARK`.
    pub remark: Option<String>,
    /// `AIRWAY_STRING`: the airway's points in order, space separated.
    pub airway_string: String,
}

/// A row of `AWY_SEG_ALT`: one segment from `from_point` toward `to_point`.
///
/// `awy_seg_gap_flag` marks a gap in the airway after this segment; `signal_gap_flag` marks a
/// navaid signal gap; `dogleg` marks a change of direction at the point. The `*_alt` pairs are
/// the minimum enroute altitudes (MEA), their GPS and DME/IRU variants, minimum crossing and
/// reception altitudes, all in feet with their directions.
#[derive(Clone, Debug, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct AirwaySegment {
    /// 1-based line in the file.
    pub line: u64,
    /// `EFF_DATE`.
    pub eff_date: String,
    /// `REGULATORY` is `Y`.
    pub regulatory: bool,
    /// `AWY_LOCATION`.
    pub awy_location: String,
    /// `AWY_ID`.
    pub awy_id: String,
    /// `POINT_SEQ`.
    pub point_seq: u32,
    /// `FROM_POINT`.
    pub from_point: String,
    /// `FROM_PT_TYPE`, e.g. `WP`, `RP`, `VORTAC`.
    pub from_pt_type: Option<String>,
    /// `NAV_NAME`.
    pub nav_name: Option<String>,
    /// `NAV_CITY`.
    pub nav_city: Option<String>,
    /// `ARTCC`.
    pub artcc: Option<String>,
    /// `ICAO_REGION_CODE` of the from point.
    pub icao_region_code: Option<String>,
    /// `STATE_CODE`.
    pub state_code: Option<String>,
    /// `COUNTRY_CODE`.
    pub country_code: Option<String>,
    /// `TO_POINT`; empty on the last point of an airway.
    pub to_point: Option<String>,
    /// `MAG_COURSE`, degrees.
    pub mag_course: Option<f64>,
    /// `OPP_MAG_COURSE`, degrees.
    pub opp_mag_course: Option<f64>,
    /// `MAG_COURSE_DIST`, nautical miles.
    pub mag_course_dist: Option<f64>,
    /// `CHGOVR_PT`.
    pub chgovr_pt: Option<String>,
    /// `CHGOVR_PT_NAME`.
    pub chgovr_pt_name: Option<String>,
    /// `CHGOVR_PT_DIST`, nautical miles.
    pub chgovr_pt_dist: Option<i64>,
    /// `AWY_SEG_GAP_FLAG` is `Y`.
    pub awy_seg_gap_flag: bool,
    /// `SIGNAL_GAP_FLAG` is `Y`.
    pub signal_gap_flag: bool,
    /// `DOGLEG` is `Y`.
    pub dogleg: bool,
    /// `NEXT_MEA_PT`. The structure file says this is never empty, but 8,306 rows of the
    /// 2026-10-29 cycle have it empty, so it is optional here and flagged by schema validation.
    pub next_mea_pt: Option<String>,
    /// `MIN_ENROUTE_ALT` and `MIN_ENROUTE_ALT_DIR`.
    pub min_enroute_alt: AltDir,
    /// `MIN_ENROUTE_ALT_OPPOSITE` and `MIN_ENROUTE_ALT_OPPOSITE_DIR`.
    pub min_enroute_alt_opposite: AltDir,
    /// `GPS_MIN_ENROUTE_ALT` and `GPS_MIN_ENROUTE_ALT_DIR`.
    pub gps_min_enroute_alt: AltDir,
    /// `GPS_MIN_ENROUTE_ALT_OPPOSITE` and `GPS_MEA_OPPOSITE_DIR`.
    pub gps_min_enroute_alt_opposite: AltDir,
    /// `DD_IRU_MEA` and `DD_IRU_MEA_DIR`.
    pub dd_iru_mea: AltDir,
    /// `DD_I_MEA_OPPOSITE` and `DD_I_MEA_OPPOSITE_DIR`.
    pub dd_i_mea_opposite: AltDir,
    /// `MIN_OBSTN_CLNC_ALT`, feet.
    pub min_obstn_clnc_alt: Option<i64>,
    /// `MIN_CROSS_ALT` and `MIN_CROSS_ALT_DIR`.
    pub min_cross_alt: AltDir,
    /// `MIN_CROSS_ALT_NAV_PT`.
    pub min_cross_alt_nav_pt: Option<String>,
    /// `MIN_CROSS_ALT_OPPOSITE` and `MIN_CROSS_ALT_OPPOSITE_DIR`.
    pub min_cross_alt_opposite: AltDir,
    /// `MIN_RECEP_ALT`, feet.
    pub min_recep_alt: Option<i64>,
    /// `MAX_AUTH_ALT`, feet.
    pub max_auth_alt: Option<i64>,
    /// `MEA_GAP`.
    pub mea_gap: Option<String>,
    /// `REQD_NAV_PERFORMANCE`.
    pub reqd_nav_performance: Option<f64>,
    /// `REMARK`.
    pub remark: Option<String>,
}

/// An airway with its segments in `POINT_SEQ` order.
#[derive(Clone, Debug, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct Airway {
    /// The airway's `AWY_BASE` row.
    pub base: AirwayBase,
    /// Its `AWY_SEG_ALT` rows sorted by `POINT_SEQ`.
    pub segments: Vec<AirwaySegment>,
}

/// Parses `AWY_BASE`.
pub fn parse_airway_bases<R: BufRead>(
    source: &str,
    reader: R,
) -> Result<Parsed<AirwayBase>, NasrError> {
    parse_rows(source, reader, |r: &Row| {
        Ok(AirwayBase {
            line: r.line(),
            eff_date: r.req("EFF_DATE")?,
            regulatory: r.flag("REGULATORY")?,
            awy_designation: r.req("AWY_DESIGNATION")?,
            awy_location: r.req("AWY_LOCATION")?,
            awy_id: r.req("AWY_ID")?,
            update_date: r.req("UPDATE_DATE")?,
            remark: r.opt("REMARK")?,
            airway_string: r.req("AIRWAY_STRING")?,
        })
    })
}

/// Parses `AWY_SEG_ALT` rows in file order.
pub fn parse_airway_segments<R: BufRead>(
    source: &str,
    reader: R,
) -> Result<Parsed<AirwaySegment>, NasrError> {
    parse_rows(source, reader, |r: &Row| {
        Ok(AirwaySegment {
            line: r.line(),
            eff_date: r.req("EFF_DATE")?,
            regulatory: r.flag("REGULATORY")?,
            awy_location: r.req("AWY_LOCATION")?,
            awy_id: r.req("AWY_ID")?,
            point_seq: r.req_u32("POINT_SEQ")?,
            from_point: r.req("FROM_POINT")?,
            from_pt_type: r.opt("FROM_PT_TYPE")?,
            nav_name: r.opt("NAV_NAME")?,
            nav_city: r.opt("NAV_CITY")?,
            artcc: r.opt("ARTCC")?,
            icao_region_code: r.opt("ICAO_REGION_CODE")?,
            state_code: r.opt("STATE_CODE")?,
            country_code: r.opt("COUNTRY_CODE")?,
            to_point: r.opt("TO_POINT")?,
            mag_course: r.opt_f64("MAG_COURSE")?,
            opp_mag_course: r.opt_f64("OPP_MAG_COURSE")?,
            mag_course_dist: r.opt_f64("MAG_COURSE_DIST")?,
            chgovr_pt: r.opt("CHGOVR_PT")?,
            chgovr_pt_name: r.opt("CHGOVR_PT_NAME")?,
            chgovr_pt_dist: r.opt_i64("CHGOVR_PT_DIST")?,
            awy_seg_gap_flag: r.flag("AWY_SEG_GAP_FLAG")?,
            signal_gap_flag: r.flag("SIGNAL_GAP_FLAG")?,
            dogleg: r.flag("DOGLEG")?,
            next_mea_pt: r.opt("NEXT_MEA_PT")?,
            min_enroute_alt: r.alt_dir("MIN_ENROUTE_ALT", "MIN_ENROUTE_ALT_DIR")?,
            min_enroute_alt_opposite: r
                .alt_dir("MIN_ENROUTE_ALT_OPPOSITE", "MIN_ENROUTE_ALT_OPPOSITE_DIR")?,
            gps_min_enroute_alt: r.alt_dir("GPS_MIN_ENROUTE_ALT", "GPS_MIN_ENROUTE_ALT_DIR")?,
            gps_min_enroute_alt_opposite: r
                .alt_dir("GPS_MIN_ENROUTE_ALT_OPPOSITE", "GPS_MEA_OPPOSITE_DIR")?,
            dd_iru_mea: r.alt_dir("DD_IRU_MEA", "DD_IRU_MEA_DIR")?,
            dd_i_mea_opposite: r.alt_dir("DD_I_MEA_OPPOSITE", "DD_I_MEA_OPPOSITE_DIR")?,
            min_obstn_clnc_alt: r.opt_i64("MIN_OBSTN_CLNC_ALT")?,
            min_cross_alt: r.alt_dir("MIN_CROSS_ALT", "MIN_CROSS_ALT_DIR")?,
            min_cross_alt_nav_pt: r.opt("MIN_CROSS_ALT_NAV_PT")?,
            min_cross_alt_opposite: r
                .alt_dir("MIN_CROSS_ALT_OPPOSITE", "MIN_CROSS_ALT_OPPOSITE_DIR")?,
            min_recep_alt: r.opt_i64("MIN_RECEP_ALT")?,
            max_auth_alt: r.opt_i64("MAX_AUTH_ALT")?,
            mea_gap: r.opt("MEA_GAP")?,
            reqd_nav_performance: r.opt_f64("REQD_NAV_PERFORMANCE")?,
            remark: r.opt("REMARK")?,
        })
    })
}

/// Parses both airway files and joins them. Airways are in `AWY_BASE` file order; segments are
/// sorted by `POINT_SEQ`. A segment whose airway is not in `AWY_BASE` is rejected as an orphan,
/// a second base row with the same (`AWY_ID`, `AWY_LOCATION`) as a duplicate, and a second
/// segment with the same airway and `POINT_SEQ` as a duplicate. Rejections from both files are
/// returned together, base file first.
pub fn parse_airways<R1: BufRead, R2: BufRead>(
    base_source: &str,
    base_reader: R1,
    seg_source: &str,
    seg_reader: R2,
) -> Result<Parsed<Airway>, NasrError> {
    let bases = parse_airway_bases(base_source, base_reader)?;
    let segs = parse_airway_segments(seg_source, seg_reader)?;
    let mut rejected = bases.rejected;
    let mut dropped = bases.rejected_dropped.saturating_add(segs.rejected_dropped);
    let mut airways: Vec<Airway> = Vec::with_capacity(bases.records.len());
    let mut index: HashMap<(String, String), usize> = HashMap::new();
    for base in bases.records {
        let key = (base.awy_id.clone(), base.awy_location.clone());
        if index.contains_key(&key) {
            crate::reject(
                &mut rejected,
                &mut dropped,
                RowError {
                    file: base_source.to_string(),
                    line: base.line,
                    column: "AWY_ID".to_string(),
                    kind: RowErrorKind::DuplicateKey(format!("{}/{}", key.0, key.1)),
                },
            );
            continue;
        }
        index.insert(key, airways.len());
        airways.push(Airway {
            base,
            segments: Vec::new(),
        });
    }
    for e in segs.rejected {
        crate::reject(&mut rejected, &mut dropped, e);
    }
    for seg in segs.records {
        let key = (seg.awy_id.clone(), seg.awy_location.clone());
        match index.get(&key).and_then(|&i| airways.get_mut(i)) {
            Some(a) => a.segments.push(seg),
            None => crate::reject(
                &mut rejected,
                &mut dropped,
                RowError {
                    file: seg_source.to_string(),
                    line: seg.line,
                    column: "AWY_ID".to_string(),
                    kind: RowErrorKind::OrphanRecord(format!("{}/{}", key.0, key.1)),
                },
            ),
        }
    }
    for a in &mut airways {
        a.segments.sort_by_key(|s| s.point_seq);
        let mut kept: Vec<AirwaySegment> = Vec::with_capacity(a.segments.len());
        for s in std::mem::take(&mut a.segments) {
            if kept.last().is_some_and(|k| k.point_seq == s.point_seq) {
                crate::reject(
                    &mut rejected,
                    &mut dropped,
                    RowError {
                        file: seg_source.to_string(),
                        line: s.line,
                        column: "POINT_SEQ".to_string(),
                        kind: RowErrorKind::DuplicateKey(format!(
                            "{}/{}/{}",
                            s.awy_id, s.awy_location, s.point_seq
                        )),
                    },
                );
            } else {
                kept.push(s);
            }
        }
        a.segments = kept;
    }
    Ok(Parsed {
        records: airways,
        rejected,
        rejected_dropped: dropped,
    })
}
