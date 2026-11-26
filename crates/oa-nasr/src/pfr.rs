//! FAA Preferred IFR Routes from NASR `PFR_BASE` and `PFR_SEG`.
//!
//! The description columns are retained verbatim. Their schema documents them as descriptions,
//! not as a machine-readable grammar; callers must not treat unknown text as an unrestricted or
//! mandatory route condition.

use crate::row::{parse_rows, Row};
use crate::{NasrError, Parsed, RowError, RowErrorKind};
use std::collections::{BTreeMap, BTreeSet};
use std::io::BufRead;

/// A row of `PFR_BASE` (one published preferred-route record).
#[derive(Clone, Debug, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct PreferredRoute {
    /// 1-based line in `PFR_BASE.csv`.
    pub line: u64,
    /// `EFF_DATE`.
    pub eff_date: String,
    /// `ORIGIN_ID` and `DSTN_ID`.
    pub origin: String,
    pub destination: String,
    /// `PFR_TYPE_CODE` and `ROUTE_NO`.
    pub route_type: String,
    pub route_no: u32,
    /// Description text from the published base row. Kept intact because FAA defines no
    /// machine grammar for these fields in the NASR schema.
    pub area_description: Option<String>,
    pub altitude_description: Option<String>,
    pub aircraft_description: Option<String>,
    pub hours_description: Option<String>,
    pub direction_description: Option<String>,
    /// Published route string; opaque source text, not rewritten by this parser.
    pub route_string: Option<String>,
    /// Ordered `PFR_SEG` rows keyed by `SEGMENT_SEQ`.
    pub segments: Vec<PreferredRouteSegment>,
}

/// One ordered row of `PFR_SEG`.
#[derive(Clone, Debug, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct PreferredRouteSegment {
    /// 1-based line in `PFR_SEG.csv`.
    pub line: u64,
    /// `EFF_DATE`.
    pub eff_date: String,
    /// `SEGMENT_SEQ`.
    pub sequence: u32,
    /// Published `SEG_VALUE` and `SEG_TYPE`.
    pub value: String,
    pub segment_type: String,
    /// Published qualifiers retained as text.
    pub state_code: Option<String>,
    pub country_code: Option<String>,
    pub icao_region_code: Option<String>,
    pub nav_type: Option<String>,
    pub next_segment: Option<String>,
}

type Key = (String, String, String, u32);

fn key(r: &Row<'_>) -> Result<Key, crate::row::Fail> {
    Ok((
        r.req("ORIGIN_ID")?,
        r.req("DSTN_ID")?,
        r.req("PFR_TYPE_CODE")?,
        r.req_u32("ROUTE_NO")?,
    ))
}

/// Parses the `PFR_BASE.csv` records.
pub fn parse_bases<R: BufRead>(
    source: &str,
    reader: R,
) -> Result<Parsed<PreferredRoute>, NasrError> {
    parse_rows(source, reader, |r: &Row| {
        Ok(PreferredRoute {
            line: r.line(),
            eff_date: r.req("EFF_DATE")?,
            origin: r.req("ORIGIN_ID")?,
            destination: r.req("DSTN_ID")?,
            route_type: r.req("PFR_TYPE_CODE")?,
            route_no: r.req_u32("ROUTE_NO")?,
            area_description: r.opt("SPECIAL_AREA_DESCRIP")?,
            altitude_description: r.opt("ALT_DESCRIP")?,
            aircraft_description: r.opt("AIRCRAFT")?,
            hours_description: r.opt("HOURS")?,
            direction_description: r.opt("ROUTE_DIR_DESCRIP")?,
            route_string: r.opt("ROUTE_STRING")?,
            segments: Vec::new(),
        })
    })
}

/// Parses `PFR_SEG.csv` in source order.
fn parse_segments<R: BufRead>(
    source: &str,
    reader: R,
) -> Result<Parsed<(Key, PreferredRouteSegment)>, NasrError> {
    parse_rows(source, reader, |r: &Row| {
        Ok((
            key(r)?,
            PreferredRouteSegment {
                line: r.line(),
                eff_date: r.req("EFF_DATE")?,
                sequence: r.req_u32("SEGMENT_SEQ")?,
                value: r.req("SEG_VALUE")?,
                segment_type: r.req("SEG_TYPE")?,
                state_code: r.opt("STATE_CODE")?,
                country_code: r.opt("COUNTRY_CODE")?,
                icao_region_code: r.opt("ICAO_REGION_CODE")?,
                nav_type: r.opt("NAV_TYPE")?,
                next_segment: r.opt("NEXT_SEG")?,
            },
        ))
    })
}

/// Joins NASR PFR base and segment rows by the FAA identity tuple and orders segments by
/// `SEGMENT_SEQ`. Duplicate bases, orphan segments, duplicate sequence numbers, and disagreeing
/// effective dates are reported as rejected rows; they are never silently merged.
pub fn parse_routes<R1: BufRead, R2: BufRead>(
    base_source: &str,
    base_reader: R1,
    segment_source: &str,
    segment_reader: R2,
) -> Result<Parsed<PreferredRoute>, NasrError> {
    let bases = parse_bases(base_source, base_reader)?;
    let segments = parse_segments(segment_source, segment_reader)?;
    let mut rejected = bases.rejected;
    let mut rejected_dropped = bases
        .rejected_dropped
        .saturating_add(segments.rejected_dropped);
    let mut routes = Vec::with_capacity(bases.records.len());
    let mut indexes = BTreeMap::<Key, usize>::new();
    for base in bases.records {
        let identity = (
            base.origin.clone(),
            base.destination.clone(),
            base.route_type.clone(),
            base.route_no,
        );
        if indexes.contains_key(&identity) {
            crate::reject(
                &mut rejected,
                &mut rejected_dropped,
                RowError {
                    file: base_source.to_string(),
                    line: base.line,
                    column: "ROUTE_NO".to_string(),
                    kind: RowErrorKind::DuplicateKey(format!(
                        "{}/{}/{}/{}",
                        identity.0, identity.1, identity.2, identity.3
                    )),
                },
            );
            continue;
        }
        indexes.insert(identity, routes.len());
        routes.push(base);
    }
    for error in segments.rejected {
        crate::reject(&mut rejected, &mut rejected_dropped, error);
    }
    for (identity, segment) in segments.records {
        let Some(&index) = indexes.get(&identity) else {
            crate::reject(
                &mut rejected,
                &mut rejected_dropped,
                RowError {
                    file: segment_source.to_string(),
                    line: segment.line,
                    column: "ROUTE_NO".to_string(),
                    kind: RowErrorKind::OrphanRecord(format!(
                        "{}/{}/{}/{}",
                        identity.0, identity.1, identity.2, identity.3
                    )),
                },
            );
            continue;
        };
        let Some(route) = routes.get_mut(index) else {
            crate::reject(
                &mut rejected,
                &mut rejected_dropped,
                RowError {
                    file: segment_source.to_string(),
                    line: segment.line,
                    column: "ROUTE_NO".to_string(),
                    kind: RowErrorKind::OrphanRecord(format!(
                        "{} base index {} is out of range",
                        identity.2, index
                    )),
                },
            );
            continue;
        };
        if route.eff_date != segment.eff_date {
            crate::reject(
                &mut rejected,
                &mut rejected_dropped,
                RowError {
                    file: segment_source.to_string(),
                    line: segment.line,
                    column: "EFF_DATE".to_string(),
                    kind: RowErrorKind::UnknownValue(format!(
                        "segment effective date {:?} does not match base {:?}",
                        segment.eff_date, route.eff_date
                    )),
                },
            );
            continue;
        }
        route.segments.push(segment);
    }
    for route in &mut routes {
        route.segments.sort_by_key(|segment| segment.sequence);
        let mut seen = BTreeSet::new();
        route.segments.retain(|segment| {
            if seen.insert(segment.sequence) {
                true
            } else {
                crate::reject(
                    &mut rejected,
                    &mut rejected_dropped,
                    RowError {
                        file: segment_source.to_string(),
                        line: segment.line,
                        column: "SEGMENT_SEQ".to_string(),
                        kind: RowErrorKind::DuplicateKey(format!(
                            "{}/{}/{}/{}/{}",
                            route.origin,
                            route.destination,
                            route.route_type,
                            route.route_no,
                            segment.sequence
                        )),
                    },
                );
                false
            }
        });
    }
    Ok(Parsed {
        records: routes,
        rejected,
        rejected_dropped,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn joins_and_orders_segments_without_interpreting_conditions() {
        let base = concat!(
            "EFF_DATE,ORIGIN_ID,DSTN_ID,PFR_TYPE_CODE,ROUTE_NO,SPECIAL_AREA_DESCRIP,ALT_DESCRIP,AIRCRAFT,HOURS,ROUTE_DIR_DESCRIP,ROUTE_STRING\n",
            "2026/10/29,ORD,JFK,H,1,AREA EAST,FL 240 AND ABOVE,2J/H,1200-2359,EASTBOUND,ORD J60 JFK\n",
        );
        let segments = concat!(
            "EFF_DATE,ORIGIN_ID,DSTN_ID,PFR_TYPE_CODE,ROUTE_NO,SEGMENT_SEQ,SEG_VALUE,SEG_TYPE,STATE_CODE,COUNTRY_CODE,ICAO_REGION_CODE,NAV_TYPE,NEXT_SEG\n",
            "2026/10/29,ORD,JFK,H,1,2,JFK,FIX,NY,US,K1,,\n",
            "2026/10/29,ORD,JFK,H,1,1,ORD,FIX,IL,US,K1,,J60\n",
        );
        let parsed = parse_routes(
            "PFR_BASE.csv",
            base.as_bytes(),
            "PFR_SEG.csv",
            segments.as_bytes(),
        )
        .unwrap();
        assert!(parsed.rejected.is_empty());
        let route = parsed.records.first().unwrap();
        assert_eq!(route.eff_date, "2026/10/29");
        assert_eq!(route.origin, "ORD");
        assert_eq!(route.destination, "JFK");
        assert_eq!(route.route_type, "H");
        assert_eq!(
            route.altitude_description.as_deref(),
            Some("FL 240 AND ABOVE")
        );
        assert_eq!(route.aircraft_description.as_deref(), Some("2J/H"));
        assert_eq!(route.direction_description.as_deref(), Some("EASTBOUND"));
        assert_eq!(
            route
                .segments
                .iter()
                .map(|s| s.sequence)
                .collect::<Vec<_>>(),
            [1, 2]
        );
    }

    #[test]
    fn orphan_duplicate_and_effective_date_conflicts_are_reported() {
        let base = concat!(
            "EFF_DATE,ORIGIN_ID,DSTN_ID,PFR_TYPE_CODE,ROUTE_NO,SPECIAL_AREA_DESCRIP,ALT_DESCRIP,AIRCRAFT,HOURS,ROUTE_DIR_DESCRIP,ROUTE_STRING\n",
            "2026/10/29,ORD,JFK,H,1,,,,,,ORD J60 JFK\n",
        );
        let segments = concat!(
            "EFF_DATE,ORIGIN_ID,DSTN_ID,PFR_TYPE_CODE,ROUTE_NO,SEGMENT_SEQ,SEG_VALUE,SEG_TYPE,STATE_CODE,COUNTRY_CODE,ICAO_REGION_CODE,NAV_TYPE,NEXT_SEG\n",
            "2026/10/29,ORD,JFK,H,1,1,ORD,FIX,,,,,\n",
            "2026/10/29,ORD,JFK,H,1,1,JFK,FIX,,,,,\n",
            "2026/10/29,LAX,JFK,H,2,1,LAX,FIX,,,,,\n",
            "2026/09/03,ORD,JFK,H,1,2,J60,AWY,,,,,\n",
        );
        let parsed = parse_routes(
            "PFR_BASE.csv",
            base.as_bytes(),
            "PFR_SEG.csv",
            segments.as_bytes(),
        )
        .unwrap();
        assert_eq!(parsed.records[0].segments.len(), 1);
        assert_eq!(parsed.rejected.len(), 3);
    }
}
