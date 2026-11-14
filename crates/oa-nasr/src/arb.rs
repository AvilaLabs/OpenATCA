//! ARTCC boundaries: `ARB_BASE` (facilities) and `ARB_SEG` (boundary points).

use crate::row::{parse_rows, Row, STD_POS};
use crate::{NasrError, Parsed, Position, RowError, RowErrorKind};
use std::collections::BTreeMap;
use std::io::BufRead;

/// A row of `ARB_BASE`: one air route traffic control centre (or CERAP).
#[derive(Clone, Debug, PartialEq)]
pub struct ArbBase {
    /// 1-based line in the file.
    pub line: u64,
    /// `EFF_DATE`, `YYYY/MM/DD`.
    pub eff_date: String,
    /// `LOCATION_ID`, e.g. `ZAU`.
    pub location_id: String,
    /// `LOCATION_NAME`.
    pub location_name: Option<String>,
    /// `COMPUTER_ID`.
    pub computer_id: String,
    /// `ICAO_ID`.
    pub icao_id: Option<String>,
    /// `LOCATION_TYPE`: `ARTCC` or `CERAP`.
    pub location_type: String,
    /// `CITY`.
    pub city: String,
    /// `STATE`.
    pub state: Option<String>,
    /// `COUNTRY_CODE`.
    pub country_code: String,
    /// Reference point of the facility.
    pub position: Position,
    /// `CROSS_REF`.
    pub cross_ref: Option<String>,
}

/// One point of a boundary (a row of `ARB_SEG`).
#[derive(Clone, Debug, PartialEq)]
pub struct BoundaryPoint {
    /// 1-based line in the file.
    pub line: u64,
    /// `REC_ID`.
    pub rec_id: String,
    /// `POINT_SEQ`, multiples of ten, in boundary order.
    pub point_seq: u32,
    /// The point.
    pub position: Position,
    /// `BNDRY_PT_DESCRIP`: how the line from this point to the next is drawn, for example `TO`,
    /// `/COMMON ZAB-ZDV/ TO`, `TO POINT OF BEGINNING`. Kept verbatim; the FAA does not say
    /// here whether such lines are great-circle or rhumb.
    pub description: Option<String>,
    /// `NAS_DESCRIP_FLAG` is `X`: the point is in the NAS description only, not the legal one.
    pub nas_description_only: bool,
}

/// An ordered point sequence for one facility, altitude structure and boundary type.
#[derive(Clone, Debug, PartialEq)]
pub struct Boundary {
    /// `LOCATION_ID`.
    pub location_id: String,
    /// `LOCATION_NAME` of the first point's row.
    pub location_name: Option<String>,
    /// `ALTITUDE`: `HIGH`, `LOW` or `UNLIMITED`.
    pub altitude: String,
    /// `TYPE`: `ARTCC`, `FIR`, `CTA`, `CTA/FIR` or `UTA`.
    pub boundary_type: String,
    /// Points sorted by `POINT_SEQ`. The last point's line returns to the first.
    pub points: Vec<BoundaryPoint>,
}

/// Parses `ARB_BASE`.
pub fn parse_base<R: BufRead>(source: &str, reader: R) -> Result<Parsed<ArbBase>, NasrError> {
    parse_rows(source, reader, |r: &Row| {
        Ok(ArbBase {
            line: r.line(),
            eff_date: r.req("EFF_DATE")?,
            location_id: r.req("LOCATION_ID")?,
            location_name: r.opt("LOCATION_NAME")?,
            computer_id: r.req("COMPUTER_ID")?,
            icao_id: r.opt("ICAO_ID")?,
            location_type: r.req("LOCATION_TYPE")?,
            city: r.req("CITY")?,
            state: r.opt("STATE")?,
            country_code: r.req("COUNTRY_CODE")?,
            position: r.position(&STD_POS)?,
            cross_ref: r.opt("CROSS_REF")?,
        })
    })
}

struct SegRow {
    location_id: String,
    location_name: Option<String>,
    altitude: String,
    boundary_type: String,
    point: BoundaryPoint,
}

/// Parses `ARB_SEG` into boundaries, one per (`LOCATION_ID`, `ALTITUDE`, `TYPE`), ordered by
/// that key, each with its points sorted by `POINT_SEQ`. A second row with the same key and
/// `POINT_SEQ` is rejected as a duplicate.
pub fn parse_boundaries<R: BufRead>(
    source: &str,
    reader: R,
) -> Result<Parsed<Boundary>, NasrError> {
    let rows = parse_rows(source, reader, |r: &Row| {
        Ok(SegRow {
            location_id: r.req("LOCATION_ID")?,
            location_name: r.opt("LOCATION_NAME")?,
            altitude: r.req("ALTITUDE")?,
            boundary_type: r.req("TYPE")?,
            point: BoundaryPoint {
                line: r.line(),
                rec_id: r.req("REC_ID")?,
                point_seq: r.req_u32("POINT_SEQ")?,
                position: r.position(&STD_POS)?,
                description: r.opt("BNDRY_PT_DESCRIP")?,
                nas_description_only: r.flag_of("NAS_DESCRIP_FLAG", "X", None)?,
            },
        })
    })?;
    let mut rejected = rows.rejected;
    type Key = (String, String, String);
    let mut groups: BTreeMap<Key, (Option<String>, Vec<BoundaryPoint>)> = BTreeMap::new();
    for s in rows.records {
        let key = (s.location_id, s.altitude, s.boundary_type);
        groups
            .entry(key)
            .or_insert_with(|| (s.location_name, Vec::new()))
            .1
            .push(s.point);
    }
    let mut records = Vec::with_capacity(groups.len());
    for ((location_id, altitude, boundary_type), (location_name, mut points)) in groups {
        points.sort_by_key(|p| p.point_seq);
        let mut kept: Vec<BoundaryPoint> = Vec::with_capacity(points.len());
        for p in points {
            if kept.last().is_some_and(|k| k.point_seq == p.point_seq) {
                rejected.push(RowError {
                    file: source.to_string(),
                    line: p.line,
                    column: "POINT_SEQ".to_string(),
                    kind: RowErrorKind::DuplicateKey(format!(
                        "{location_id}/{altitude}/{boundary_type}/{}",
                        p.point_seq
                    )),
                });
            } else {
                kept.push(p);
            }
        }
        records.push(Boundary {
            location_id,
            location_name,
            altitude,
            boundary_type,
            points: kept,
        });
    }
    rejected.sort_by_key(|e| e.line);
    Ok(Parsed { records, rejected })
}
