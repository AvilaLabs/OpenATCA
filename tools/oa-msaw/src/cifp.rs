//! Procedure areas from the CIFP. Shell (B3): a pure function of a parsed file, kept with the tool
//! because it is the tool's reading of the CIFP and not a rule of the net.
//!
//! For each runway of an airport, one area: a circle of the configured radius round the landing
//! threshold, with the lowest altitude the airport's instrument approaches to that runway publish
//! (the lowest of the altitudes that are floors: at, at or above, and the lower bound of a
//! between; an at-or-below altitude is a ceiling and is not one). A runway with no approach that
//! publishes such an altitude has no area. This is a lower floor round the runway only: the path
//! of the approach is the business of approach path monitoring (build order step 8), not of this
//! net. Departure procedures (SIDs) are not used yet.

use oa_arinc424::{
    altitude_feet, procedures_at, runway_designator, runways_at, AltitudeDescription, Cifp,
    ProcedureKind,
};
use oa_msaw::ProcArea;
use oa_safety::geom::polygon_new;

/// The points of the circle, a sixteen-gon (a convex polygon the core's polygon test takes).
const CIRCLE_POINTS: usize = 16;
/// Metres per degree of latitude (mean radius).
const M_PER_DEG: f64 = 111_194.926_644_558_74;

/// One area of the CIFP and what it came from.
// A7.2: cold struct, size guard waived
#[derive(Debug, Clone, PartialEq)]
pub struct CifpArea {
    /// The airport and runway, for the summary.
    pub label: String,
    pub area: ProcArea,
    /// The approach procedures whose altitudes gave the floor.
    pub approaches: usize,
}

/// The runway an approach ident names, as the designator of its runway record: the type letter,
/// two digits and an optional L, R or C (`I04R` gives `04R`, `R22L-Z` gives `22L`); `None` for an
/// approach that names no runway (`VOR-A`).
pub fn approach_runway(ident: &str) -> Option<String> {
    let rest = ident.get(1..)?;
    let mut chars = rest.chars();
    let (a, b) = (chars.next()?, chars.next()?);
    if !(a.is_ascii_digit() && b.is_ascii_digit()) {
        return None;
    }
    let mut out = format!("{a}{b}");
    if let Some(c) = chars.next().filter(|c| matches!(c, 'L' | 'R' | 'C')) {
        out.push(c);
    }
    Some(out)
}

/// The areas of the airport `airport` (an ICAO identifier as the CIFP codes it) with circles of
/// `radius_nm`. An airport the file does not hold is an error: a configuration naming it would
/// otherwise run with no procedure areas, silently.
pub fn areas_from_cifp(c: &Cifp, airport: &str, radius_nm: f64) -> Result<Vec<CifpArea>, String> {
    if !c.airports.iter().any(|a| a.ident == airport) {
        return Err(format!("the CIFP has no airport {airport}"));
    }
    let mut out = Vec::new();
    for r in runways_at(c, airport) {
        let designator = runway_designator(r);
        let mut floor: Option<i32> = None;
        let mut approaches = 0;
        for p in procedures_at(c, airport, ProcedureKind::Approach) {
            if approach_runway(&p.ident).as_deref() != Some(designator) {
                continue;
            }
            let mut any = false;
            for leg in p.routes.iter().flat_map(|rt| rt.legs.iter()) {
                let alt = match leg.altitude_description {
                    None | Some(AltitudeDescription::At | AltitudeDescription::AtOrAbove) => {
                        leg.altitude1
                    }
                    Some(AltitudeDescription::Between) => leg.altitude2.or(leg.altitude1),
                    Some(_) => None,
                };
                if let Some(ft) = alt.and_then(altitude_feet) {
                    floor = Some(floor.map_or(ft, |f| f.min(ft)));
                    any = true;
                }
            }
            if any {
                approaches += 1;
            }
        }
        let Some(floor_ft) = floor else {
            continue;
        };
        let (lat, lon) = (r.threshold.lat, r.threshold.lon);
        let dlat = radius_nm * 1852.0 / M_PER_DEG;
        let dlon = dlat / lat.to_radians().cos().max(0.01);
        let mut pts = Vec::new();
        for k in 0..CIRCLE_POINTS {
            let a = std::f64::consts::TAU * k as f64 / CIRCLE_POINTS as f64;
            pts.push((
                ((lat + dlat * a.sin()) * 1e7).round() as i32,
                ((lon + dlon * a.cos()) * 1e7).round() as i32,
            ));
        }
        let area = polygon_new(&pts).map_err(|e| format!("{airport} {designator}: {e}"))?;
        out.push(CifpArea {
            label: format!("{airport} RW{designator}"),
            area: ProcArea { area, floor_ft },
            approaches,
        });
    }
    Ok(out)
}
