//! Class B/C/D controlled airspace (UC) and special use airspace (UR), with boundaries.

use crate::error::ErrorKind;
use crate::field::Rec;
use crate::types::{AltitudeLimit, AltitudeUnit, Meta, Position};

/// Which section an airspace came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AirspaceKind {
    /// Section U subsection C.
    Controlled,
    /// Section U subsection R.
    SpecialUse,
}

/// How a boundary point connects to the next (cols 31).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoundaryVia {
    /// `C`: a circle around the arc origin; the radius is the arc distance.
    Circle,
    /// `G`: great-circle line to the next point.
    GreatCircle,
    /// `H`: rhumb line to the next point.
    RhumbLine,
    /// `L`: counter-clockwise arc around the arc origin to the next point.
    CounterClockwiseArc,
    /// `R`: clockwise arc around the arc origin to the next point.
    ClockwiseArc,
}

/// One boundary record.
#[derive(Debug, Clone, PartialEq)]
pub struct BoundaryPoint {
    /// Cols 21-24.
    pub sequence: u32,
    /// Col 31. Verified: `R` arcs at ORD with origin and radius, `G` lines, `C` circles on UR.
    pub via: BoundaryVia,
    /// Col 32 `E`: this record ends the boundary (the path returns to the first point).
    /// Verified: whole file, the last record of every one of the 2,867 volumes carries it.
    pub end: bool,
    /// Cols 33-51. Blank for a circle.
    pub position: Option<Position>,
    /// Cols 52-70. Arc or circle centre. Verified: the ORD Class B arc point is 5.0 NM from its
    /// centre at bearing 349.4, computed independently from the coordinates; whole file: the
    /// coded distance matches for 2,239 of 2,246 arc points and the bearing for all 2,246.
    pub arc_origin: Option<Position>,
    /// Cols 71-74, tenths of a nautical mile: radius from the arc origin.
    pub arc_distance_nm: Option<f64>,
    /// Cols 75-78, tenths of a degree: bearing from the arc origin to `position`.
    pub arc_bearing_deg: Option<f64>,
}

/// A controlling agency or other continuation of a UR volume (cols 100-123 for application type `C`).
#[derive(Debug, Clone, PartialEq)]
pub struct AirspaceContinuation {
    pub meta: Meta,
    /// Col 25: continuation record number.
    pub number: char,
    /// Col 26: application type (`C` controlling agency).
    pub application_type: char,
    /// Cols 100-123 for type `C` (controlling agency name); empty otherwise.
    pub controlling_agency: String,
}

/// One airspace volume: all records sharing designation and multiple code, in sequence order.
#[derive(Debug, Clone, PartialEq)]
pub struct Airspace {
    pub meta: Meta,
    pub kind: AirspaceKind,
    /// Cols 7-8. ICAO region code. Verified: `K5` for ORD.
    pub icao_region: String,
    /// Col 9. UC: airspace type (`T` terminal control area, `A`, `Z`; as coded). UR: restrictive
    /// type (`R` restricted, `M` MOA, `W` warning, `A` alert, `P` prohibited, `U` unspecified,
    /// `D`, `T`...). Verified: `T` for the ORD Class B, `M` for MOAs, `R` for R-3401A.
    pub airspace_type: char,
    /// UC cols 10-14: airspace center identifier (`KORD`); UR cols 10-19: restrictive
    /// airspace designation (`R-3401A` is coded `3401A`, `BOARDMAN`). Verified: both.
    pub designation: String,
    /// UC cols 15-16 (section and subsection of the center, `PA` airport) and col 17 (classification `B`, `C`, `D`).
    /// Verified: `B` for the ORD Class B. `None` for UR.
    pub classification: Option<char>,
    /// Col 20. Distinguishes separate volumes with the same designation. Verified: A, B... on ORD.
    pub multiple_code: char,
    /// UR col 26: level (`B` both, `H`, `L`). `None` for UC.
    pub level: Option<char>,
    /// UR col 27: time code, `C` continuous (FAA readme), blank part-time.
    pub time_code: Option<char>,
    /// Lower limit, cols 82-86 and unit col 87. Only on the first record (FAA readme). Verified: `GND` `A` at ORD.
    pub lower: Option<AltitudeLimit>,
    /// Upper limit, cols 88-92 and unit col 93. Verified: 10000 `M` at ORD.
    pub upper: Option<AltitudeLimit>,
    /// Cols 94-123, from the first record that has one. Verified: `CHICAGO AREA A`.
    pub name: String,
    pub boundary: Vec<BoundaryPoint>,
    /// Continuation records (UR controlling agencies).
    pub continuations: Vec<AirspaceContinuation>,
}

/// True when the boundary is closed: a single circle, or the last point carries the end
/// marker, or the first and last positions are the same point.
pub fn is_closed(a: &Airspace) -> bool {
    let (Some(first), Some(last)) = (a.boundary.first(), a.boundary.last()) else {
        return false;
    };
    if a.boundary.len() == 1 {
        return first.via == BoundaryVia::Circle;
    }
    last.end || (first.position.is_some() && first.position == last.position)
}

/// The controlling agency from the first `C` continuation, if any.
pub fn controlling_agency(a: &Airspace) -> Option<&str> {
    a.continuations
        .iter()
        .find(|c| c.application_type == 'C' && !c.controlling_agency.is_empty())
        .map(|c| c.controlling_agency.as_str())
}

fn limit(
    r: &Rec,
    alt_col: usize,
    unit_col: usize,
    name: &'static str,
) -> Result<Option<AltitudeLimit>, ErrorKind> {
    let Some(altitude) = r.altitude(name, alt_col, alt_col + 4)? else {
        return Ok(None);
    };
    let unit = match r.ch(unit_col) {
        'A' => AltitudeUnit::Agl,
        'M' => AltitudeUnit::Msl,
        c => AltitudeUnit::Other(c),
    };
    Ok(Some(AltitudeLimit { altitude, unit }))
}

pub(crate) fn parse_boundary_point(r: &Rec) -> Result<BoundaryPoint, ErrorKind> {
    let via = match r.ch(31) {
        'C' => BoundaryVia::Circle,
        'G' => BoundaryVia::GreatCircle,
        'H' => BoundaryVia::RhumbLine,
        'L' => BoundaryVia::CounterClockwiseArc,
        'R' => BoundaryVia::ClockwiseArc,
        _ => return Err(r.bad("boundary via", 31, 32, "expected C, G, H, L or R")),
    };
    let end = match r.ch(32) {
        ' ' => false,
        'E' => true,
        _ => {
            return Err(r.bad(
                "boundary via",
                31,
                32,
                "second character must be blank or E",
            ))
        }
    };
    let position = r.position("boundary point", 33)?;
    let arc_origin = r.position("arc origin", 52)?;
    let needs_origin = !matches!(via, BoundaryVia::GreatCircle | BoundaryVia::RhumbLine);
    if needs_origin && arc_origin.is_none() {
        return Err(r.bad("arc origin", 52, 70, "required for circles and arcs"));
    }
    if via != BoundaryVia::Circle && position.is_none() {
        return Err(r.bad("boundary point", 33, 51, "required except for circles"));
    }
    Ok(BoundaryPoint {
        sequence: r.req_uint("sequence number", 21, 24)?,
        via,
        end,
        position,
        arc_origin,
        arc_distance_nm: r.scaled("arc distance", 71, 74, 10.0)?,
        arc_bearing_deg: r.scaled("arc bearing", 75, 78, 10.0)?,
    })
}

pub(crate) fn parse_airspace(
    r: &Rec,
    meta: Meta,
    kind: AirspaceKind,
) -> Result<Airspace, ErrorKind> {
    let point = parse_boundary_point(r)?;
    let (designation, classification, level, time_code) = match kind {
        AirspaceKind::Controlled => (r.text(10, 14), Some(r.ch(17)), None, None),
        AirspaceKind::SpecialUse => (r.text(10, 19), None, r.opt_ch(26), r.opt_ch(27)),
    };
    Ok(Airspace {
        meta,
        kind,
        icao_region: r.text(7, 8),
        airspace_type: r.ch(9),
        designation,
        classification,
        multiple_code: r.ch(20),
        level,
        time_code,
        lower: limit(r, 82, 87, "lower limit")?,
        upper: limit(r, 88, 93, "upper limit")?,
        name: r.text(94, 123),
        boundary: vec![point],
        continuations: Vec::new(),
    })
}

/// Cols 1-20 identify the volume (everything before the sequence number).
pub(crate) fn volume_key(r: &Rec) -> String {
    r.raw(1, 20).to_string()
}

pub(crate) fn parse_airspace_continuation(r: &Rec, meta: Meta) -> AirspaceContinuation {
    AirspaceContinuation {
        meta,
        number: r.ch(25),
        application_type: r.ch(26),
        controlling_agency: if r.ch(26) == 'C' {
            r.text(100, 123)
        } else {
            String::new()
        },
    }
}
