//! STAR and DP (departure procedure) route structure.
//!
//! STAR and DP files have the same shape: a `*_BASE` row per procedure, a `*_APT` row per
//! airport/runway the procedure serves, and `*_RTE` rows giving the route points. A procedure
//! is identified by its computer code (`AALAN.BLAID2`). Route points are grouped into
//! [`ProcedureRoute`]s: the `BODY` (common route) and each `TRANSITION`, by (portion, body
//! sequence, route name, transition code), each ordered by `POINT_SEQ`.

use crate::row::{parse_rows, Fail, Row};
use crate::{NasrError, Parsed, RowError, RowErrorKind};
use std::collections::{BTreeMap, HashMap};
use std::io::BufRead;

/// Which pair of file families a procedure comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ProcedureKind {
    /// Standard terminal arrival: `STAR_BASE`, `STAR_APT`, `STAR_RTE`.
    Star,
    /// Departure procedure: `DP_BASE`, `DP_APT`, `DP_RTE`.
    Dp,
}

fn name_col(kind: ProcedureKind) -> &'static str {
    match kind {
        ProcedureKind::Star => "ARRIVAL_NAME",
        ProcedureKind::Dp => "DP_NAME",
    }
}

fn code_col(kind: ProcedureKind) -> &'static str {
    match kind {
        ProcedureKind::Star => "STAR_COMPUTER_CODE",
        ProcedureKind::Dp => "DP_COMPUTER_CODE",
    }
}

fn amend_date_col(kind: ProcedureKind) -> &'static str {
    match kind {
        ProcedureKind::Star => "STAR_AMEND_EFF_DATE",
        ProcedureKind::Dp => "DP_AMEND_EFF_DATE",
    }
}

/// The placeholder `DP_COMPUTER_CODE` the FAA gives a departure procedure that has no computer
/// code (30 rows of `DP_BASE` in cycle 2026-10-29, for example `O'HARE NINE`). Several distinct
/// procedures share it, so they are joined to their airport and route rows by name and ARTCC
/// as well.
pub const UNASSIGNED_CODE: &str = "NOT ASSIGNED";

/// The key that joins `*_APT` and `*_RTE` rows to their base row: the computer code, extended
/// with the procedure name and ARTCC when a DP carries the [`UNASSIGNED_CODE`] placeholder.
fn join_key(kind: ProcedureKind, r: &Row<'_>) -> Result<String, Fail> {
    let code = r.req(code_col(kind))?;
    if kind == ProcedureKind::Dp && code == UNASSIGNED_CODE {
        let name = r.req(name_col(kind))?;
        let artcc = r.opt("ARTCC")?.unwrap_or_default();
        return Ok(format!("{code}|{name}|{artcc}"));
    }
    Ok(code)
}

/// A row of `STAR_BASE` or `DP_BASE`.
#[derive(Clone, Debug, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct ProcedureBase {
    /// 1-based line in the file.
    pub line: u64,
    /// `EFF_DATE`.
    pub eff_date: String,
    /// `ARRIVAL_NAME` or `DP_NAME`.
    pub name: String,
    /// `AMENDMENT_NO`.
    pub amendment_no: String,
    /// `ARTCC`.
    pub artcc: Option<String>,
    /// `STAR_AMEND_EFF_DATE` or `DP_AMEND_EFF_DATE`.
    pub amend_eff_date: String,
    /// `RNAV_FLAG` is `Y`.
    pub rnav_flag: bool,
    /// `STAR_COMPUTER_CODE` or `DP_COMPUTER_CODE`.
    pub computer_code: String,
    /// `GRAPHICAL_DP_TYPE` (`SID` or `OBSTACLE`); DP only.
    pub graphical_dp_type: Option<String>,
    /// `SERVED_ARPT`: space-separated airport identifiers, as published.
    pub served_arpt: String,
}

/// The served airport identifiers of a procedure, split on whitespace.
pub fn served_airports(base: &ProcedureBase) -> impl Iterator<Item = &str> {
    base.served_arpt.split_whitespace()
}

/// A row of `STAR_APT` or `DP_APT`: a body serves an airport, optionally one runway end.
#[derive(Clone, Debug, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct ProcedureAirport {
    /// 1-based line in the file.
    pub line: u64,
    /// `ARTCC`.
    pub artcc: Option<String>,
    /// `BODY_NAME`.
    pub body_name: String,
    /// `BODY_SEQ`.
    pub body_seq: u32,
    /// `ARPT_ID`.
    pub arpt_id: String,
    /// `RWY_END_ID`; `ALL` or empty when it applies to the whole airport.
    pub rwy_end_id: Option<String>,
}

/// Whether a route is the common body or a transition.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RoutePortion {
    /// `BODY`.
    Body,
    /// `TRANSITION`.
    Transition,
}

/// A row of `STAR_RTE` or `DP_RTE`.
#[derive(Clone, Debug, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct RoutePoint {
    /// 1-based line in the file.
    pub line: u64,
    /// `ARTCC`.
    pub artcc: Option<String>,
    /// `POINT_SEQ`.
    pub point_seq: u32,
    /// `POINT`.
    pub point: String,
    /// `ICAO_REGION_CODE`.
    pub icao_region_code: Option<String>,
    /// `POINT_TYPE`, e.g. `WP`, `RP`, `VORTAC`.
    pub point_type: String,
    /// `NEXT_POINT`.
    pub next_point: Option<String>,
    /// `ARPT_RWY_ASSOC`: comma-separated `AIRPORT` or `AIRPORT/RUNWAY` entries, as published.
    pub arpt_rwy_assoc: Option<String>,
}

/// The airport/runway associations of a route point as (airport, optional runway end) pairs.
pub fn associations(point: &RoutePoint) -> Vec<(&str, Option<&str>)> {
    let Some(text) = &point.arpt_rwy_assoc else {
        return Vec::new();
    };
    text.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| match s.split_once('/') {
            Some((a, r)) => (a.trim(), Some(r.trim())),
            None => (s, None),
        })
        .collect()
}

/// The route points of one body or transition, ordered by `POINT_SEQ`.
#[derive(Clone, Debug, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct ProcedureRoute {
    /// `ROUTE_PORTION_TYPE`.
    pub portion: RoutePortion,
    /// `ROUTE_NAME`.
    pub route_name: String,
    /// `BODY_SEQ`.
    pub body_seq: u32,
    /// `TRANSITION_COMPUTER_CODE`; empty for a body.
    pub transition_computer_code: Option<String>,
    /// Points in `POINT_SEQ` order.
    pub points: Vec<RoutePoint>,
}

/// A procedure with its airports and routes.
#[derive(Clone, Debug, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct Procedure {
    /// STAR or DP.
    pub kind: ProcedureKind,
    /// The `*_BASE` row.
    pub base: ProcedureBase,
    /// `*_APT` rows in file order.
    pub airports: Vec<ProcedureAirport>,
    /// Routes ordered by (portion, body sequence, route name, transition code).
    pub routes: Vec<ProcedureRoute>,
}

/// Parses `STAR_BASE` or `DP_BASE`.
pub fn parse_bases<R: BufRead>(
    kind: ProcedureKind,
    source: &str,
    reader: R,
) -> Result<Parsed<ProcedureBase>, NasrError> {
    let keyed = parse_keyed_bases(kind, source, reader)?;
    Ok(Parsed {
        records: keyed.records.into_iter().map(|(_, b)| b).collect(),
        rejected: keyed.rejected,
        rejected_dropped: keyed.rejected_dropped,
    })
}

fn parse_keyed_bases<R: BufRead>(
    kind: ProcedureKind,
    source: &str,
    reader: R,
) -> Result<Parsed<(String, ProcedureBase)>, NasrError> {
    parse_rows(source, reader, |r: &Row| {
        let key = join_key(kind, r)?;
        Ok((
            key,
            ProcedureBase {
                line: r.line(),
                eff_date: r.req("EFF_DATE")?,
                name: r.req(name_col(kind))?,
                amendment_no: r.req("AMENDMENT_NO")?,
                artcc: r.opt("ARTCC")?,
                amend_eff_date: r.req(amend_date_col(kind))?,
                rnav_flag: r.flag("RNAV_FLAG")?,
                computer_code: r.req(code_col(kind))?,
                graphical_dp_type: match kind {
                    ProcedureKind::Star => None,
                    ProcedureKind::Dp => Some(r.req("GRAPHICAL_DP_TYPE")?),
                },
                served_arpt: r.req("SERVED_ARPT")?,
            },
        ))
    })
}

/// Parses `STAR_APT` or `DP_APT` into (join key, airport row) pairs in file order. The join key is
/// the computer code, or the code with name and ARTCC for an [`UNASSIGNED_CODE`] departure.
pub fn parse_airports<R: BufRead>(
    kind: ProcedureKind,
    source: &str,
    reader: R,
) -> Result<Parsed<(String, ProcedureAirport)>, NasrError> {
    parse_rows(source, reader, |r: &Row| {
        Ok((
            join_key(kind, r)?,
            ProcedureAirport {
                line: r.line(),
                artcc: r.opt("ARTCC")?,
                body_name: r.req("BODY_NAME")?,
                body_seq: r.req_u32("BODY_SEQ")?,
                arpt_id: r.req("ARPT_ID")?,
                rwy_end_id: r.opt("RWY_END_ID")?,
            },
        ))
    })
}

// A7.2: cold struct, size guard waived
struct RteRow {
    code: String,
    portion: RoutePortion,
    route_name: String,
    body_seq: u32,
    transition: Option<String>,
    point: RoutePoint,
}

fn parse_route_rows<R: BufRead>(
    kind: ProcedureKind,
    source: &str,
    reader: R,
) -> Result<Parsed<RteRow>, NasrError> {
    parse_rows(source, reader, |r: &Row| {
        let portion = match r.req("ROUTE_PORTION_TYPE")?.as_str() {
            "BODY" => RoutePortion::Body,
            "TRANSITION" => RoutePortion::Transition,
            other => {
                return Err(r.problem(
                    "ROUTE_PORTION_TYPE",
                    RowErrorKind::UnknownValue(other.to_string()),
                ))
            }
        };
        Ok(RteRow {
            code: join_key(kind, r)?,
            portion,
            route_name: r.req("ROUTE_NAME")?,
            body_seq: r.req_u32("BODY_SEQ")?,
            transition: r.opt("TRANSITION_COMPUTER_CODE")?,
            point: RoutePoint {
                line: r.line(),
                artcc: r.opt("ARTCC")?,
                point_seq: r.req_u32("POINT_SEQ")?,
                point: r.req("POINT")?,
                icao_region_code: r.opt("ICAO_REGION_CODE")?,
                point_type: r.req("POINT_TYPE")?,
                next_point: r.opt("NEXT_POINT")?,
                arpt_rwy_assoc: r.opt("ARPT_RWY_ASSOC")?,
            },
        })
    })
}

type RouteKey = (RoutePortion, u32, String, Option<String>);

/// Parses the three files of one kind and joins them into [`Procedure`]s in `*_BASE` file order.
/// An airport or route row whose join key (see [`UNASSIGNED_CODE`]) is not in the base file is
/// rejected as an orphan; a repeated base join key, or a repeated `POINT_SEQ` within one route,
/// as a duplicate. Rejections from all three files are returned together (base, airport, route).
pub fn parse_procedures<R1: BufRead, R2: BufRead, R3: BufRead>(
    kind: ProcedureKind,
    base: (&str, R1),
    apt: (&str, R2),
    rte: (&str, R3),
) -> Result<Parsed<Procedure>, NasrError> {
    let bases = parse_keyed_bases(kind, base.0, base.1)?;
    let apts = parse_airports(kind, apt.0, apt.1)?;
    let rtes = parse_route_rows(kind, rte.0, rte.1)?;
    let mut rejected = bases.rejected;
    let mut dropped = bases
        .rejected_dropped
        .saturating_add(apts.rejected_dropped)
        .saturating_add(rtes.rejected_dropped);
    for e in apts.rejected.into_iter().chain(rtes.rejected) {
        crate::reject(&mut rejected, &mut dropped, e);
    }

    let mut procs: Vec<Procedure> = Vec::with_capacity(bases.records.len());
    let mut index: HashMap<String, usize> = HashMap::new();
    for (key, b) in bases.records {
        if index.contains_key(&key) {
            crate::reject(
                &mut rejected,
                &mut dropped,
                RowError {
                    file: base.0.to_string(),
                    line: b.line,
                    column: code_col(kind).to_string(),
                    kind: RowErrorKind::DuplicateKey(key),
                },
            );
            continue;
        }
        index.insert(key, procs.len());
        procs.push(Procedure {
            kind,
            base: b,
            airports: Vec::new(),
            routes: Vec::new(),
        });
    }
    for (code, a) in apts.records {
        match index.get(&code).and_then(|&i| procs.get_mut(i)) {
            Some(p) => p.airports.push(a),
            None => crate::reject(
                &mut rejected,
                &mut dropped,
                RowError {
                    file: apt.0.to_string(),
                    line: a.line,
                    column: code_col(kind).to_string(),
                    kind: RowErrorKind::OrphanRecord(code),
                },
            ),
        }
    }
    let mut grouped: Vec<BTreeMap<RouteKey, Vec<RoutePoint>>> =
        (0..procs.len()).map(|_| BTreeMap::new()).collect();
    for r in rtes.records {
        match index.get(&r.code).and_then(|&i| grouped.get_mut(i)) {
            Some(g) => g
                .entry((r.portion, r.body_seq, r.route_name, r.transition))
                .or_default()
                .push(r.point),
            None => crate::reject(
                &mut rejected,
                &mut dropped,
                RowError {
                    file: rte.0.to_string(),
                    line: r.point.line,
                    column: code_col(kind).to_string(),
                    kind: RowErrorKind::OrphanRecord(r.code),
                },
            ),
        }
    }
    for (p, g) in procs.iter_mut().zip(grouped) {
        for ((portion, body_seq, route_name, transition), mut points) in g {
            points.sort_by_key(|pt| pt.point_seq);
            let mut kept: Vec<RoutePoint> = Vec::with_capacity(points.len());
            for pt in points {
                if kept.last().is_some_and(|k| k.point_seq == pt.point_seq) {
                    crate::reject(
                        &mut rejected,
                        &mut dropped,
                        RowError {
                            file: rte.0.to_string(),
                            line: pt.line,
                            column: "POINT_SEQ".to_string(),
                            kind: RowErrorKind::DuplicateKey(format!(
                                "{}/{route_name}/{}",
                                p.base.computer_code, pt.point_seq
                            )),
                        },
                    );
                } else {
                    kept.push(pt);
                }
            }
            p.routes.push(ProcedureRoute {
                portion,
                route_name,
                body_seq,
                transition_computer_code: transition,
                points: kept,
            });
        }
    }
    Ok(Parsed {
        records: procs,
        rejected,
        rejected_dropped: dropped,
    })
}
