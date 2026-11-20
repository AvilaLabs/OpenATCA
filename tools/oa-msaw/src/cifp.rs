//! Final-approach corridors from the CIFP. Shell (B3): a pure function of a parsed file, kept with
//! the tool because it is the tool's reading of the CIFP and not a rule of the net.
//!
//! For each runway end of an airport that has an instrument approach with a published altitude at
//! its final approach fix, one corridor: the strip under the extended centreline from the landing
//! threshold outward to the fix (5 NM when the fix cannot be placed), 0.3 NM either side at the
//! threshold widening to 1.0 NM at the far end, with the surface from the threshold elevation to
//! the published fix altitude (the core takes the allowance off it). The fix is the leg whose
//! fourth waypoint description code is `F`; its altitude is read as a floor (at, at or above,
//! the lower bound of a between; an at-or-below altitude is a ceiling and is not one). Where a
//! runway end has several approaches the one with the lowest fix altitude gives the corridor, so
//! that the surface never forbids what any published approach allows. A runway end with no such
//! approach has no corridor and the normal floor applies there: the net alerts, it does not
//! assume. The path of the approach is the business of approach path monitoring (build order step
//! 8), not of this net. Departure procedures (SIDs) are not used yet.

use oa_arinc424::{
    altitude_feet, procedures_at, runway_designator, runways_at, AltitudeDescription, Cifp, FixRef,
    Leg, Position, ProcedureKind,
};
use oa_msaw::Corridor;

/// Metres per degree of latitude (mean radius), the core's.
const M_PER_DEG: f64 = 111_194.926_644_558_74;
const NM_M: f64 = 1852.0;
/// Half-width of a corridor at the threshold and at its far end, nautical miles.
pub const NEAR_HALF_WIDTH_NM: f64 = 0.3;
pub const FAR_HALF_WIDTH_NM: f64 = 1.0;
/// The length of a corridor when the fix cannot be placed, nautical miles.
pub const DEFAULT_FAF_NM: f64 = 5.0;
/// A fix closer to the threshold than this or farther than that is not taken for the final
/// approach fix of a straight-in approach, nautical miles.
const FAF_MIN_NM: f64 = 1.0;
const FAF_MAX_NM: f64 = 15.0;

/// One corridor of the CIFP and what it came from.
// A7.2: cold struct, size guard waived
#[derive(Debug, Clone, PartialEq)]
pub struct CifpCorridor {
    /// The airport and runway, for the summary.
    pub label: String,
    pub corridor: Corridor,
    /// The length of the corridor, nautical miles, and whether the CIFP gave it (else the default).
    pub faf_nm: f64,
    pub faf_from_cifp: bool,
    /// The approaches of the runway end that have a fix altitude.
    pub approaches: usize,
}

/// What [`corridors_from_cifp`] made and what it had to leave out. A7.2: cold struct.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CifpCorridors {
    pub corridors: Vec<CifpCorridor>,
    /// Runway ends with an approach but no corridor, for want of a fix altitude, a threshold
    /// elevation or a runway direction.
    pub without_corridor: Vec<String>,
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

/// The designator of the other end of the runway (`04L` gives `22R`), `None` if it has no number.
fn opposite_designator(d: &str) -> Option<String> {
    let n: u32 = d.get(..2)?.parse().ok()?;
    if !(1..=36).contains(&n) {
        return None;
    }
    let other = (n + 17) % 36 + 1;
    let side = match d.get(2..)? {
        "L" => "R",
        "R" => "L",
        "C" => "C",
        "" => "",
        _ => return None,
    };
    Some(format!("{other:02}{side}"))
}

/// The initial true bearing from one point to another, degrees in `0..360`.
fn bearing_deg(a: &Position, b: &Position) -> f64 {
    let (la1, la2) = (a.lat.to_radians(), b.lat.to_radians());
    let dlon = (b.lon - a.lon).to_radians();
    let y = dlon.sin() * la2.cos();
    let x = la1.cos() * la2.sin() - la1.sin() * la2.cos() * dlon.cos();
    y.atan2(x).to_degrees().rem_euclid(360.0)
}

/// The great-circle distance between two points, metres (mean radius, as the core's scale).
fn distance_m(a: &Position, b: &Position) -> f64 {
    let (la1, la2) = (a.lat.to_radians(), b.lat.to_radians());
    let (dla, dlo) = (la2 - la1, (b.lon - a.lon).to_radians());
    let h = (dla / 2.0).sin().powi(2) + la1.cos() * la2.cos() * (dlo / 2.0).sin().powi(2);
    2.0 * h.sqrt().min(1.0).asin() * M_PER_DEG * 180.0 / std::f64::consts::PI
}

/// The altitude of a leg as a floor, feet: at, at or above, the lower bound of a between, the
/// first altitude of a glide slope intercept (`G`, `H`, `I`, `J`: the second is the slope's own);
/// none for an at-or-below altitude (a ceiling).
fn leg_floor_ft(leg: &Leg) -> Option<i32> {
    let alt = match leg.altitude_description {
        None
        | Some(
            AltitudeDescription::At
            | AltitudeDescription::AtOrAbove
            | AltitudeDescription::Other('G' | 'H' | 'I' | 'J'),
        ) => leg.altitude1,
        Some(AltitudeDescription::Between) => leg.altitude2.or(leg.altitude1),
        Some(_) => None,
    };
    alt.and_then(altitude_feet)
}

/// The position of the fix a leg names, from the terminal waypoints of the airport, the enroute
/// waypoints, the NDBs and the VORs; `None` if the CIFP does not place it.
fn fix_position(c: &Cifp, airport: &str, fix: &FixRef) -> Option<Position> {
    match (fix.section, fix.subsection) {
        ('P', 'C') => c
            .terminal_waypoints
            .iter()
            .find(|w| w.area == airport && w.ident == fix.ident)
            .map(|w| w.position),
        ('E', 'A') => c
            .enroute_waypoints
            .iter()
            .find(|w| w.ident == fix.ident && w.icao_region == fix.icao_region)
            .map(|w| w.position),
        ('P', 'N') => c
            .terminal_ndbs
            .iter()
            .find(|n| n.airport_ident.as_deref() == Some(airport) && n.ident == fix.ident)
            .map(|n| n.position),
        ('D', 'B') => c
            .ndbs
            .iter()
            .find(|n| n.ident == fix.ident && n.icao_region == fix.icao_region)
            .map(|n| n.position),
        ('D', _) => c
            .vhf_navaids
            .iter()
            .find(|v| v.ident == fix.ident && v.icao_region == fix.icao_region)
            .and_then(|v| v.vor_position),
        _ => None,
    }
}

/// The corridors of the airport `airport` (an ICAO identifier as the CIFP codes it). An airport
/// the file does not hold is an error: a configuration naming it would otherwise run with no
/// corridors, silently.
pub fn corridors_from_cifp(c: &Cifp, airport: &str) -> Result<CifpCorridors, String> {
    let Some(apt) = c.airports.iter().find(|a| a.ident == airport) else {
        return Err(format!("the CIFP has no airport {airport}"));
    };
    let mut out = CifpCorridors::default();
    for r in runways_at(c, airport) {
        let designator = runway_designator(r);
        let label = format!("{airport} RW{designator}");
        // The approaches to this runway end that publish an altitude at their final approach fix:
        // (altitude, distance from the threshold in metres if the fix can be placed).
        let mut found: Vec<(i32, Option<f64>)> = Vec::new();
        let mut any_approach = false;
        for p in procedures_at(c, airport, ProcedureKind::Approach) {
            if approach_runway(&p.ident).as_deref() != Some(designator) {
                continue;
            }
            any_approach = true;
            let faf = p
                .routes
                .iter()
                .flat_map(|rt| rt.legs.iter())
                .filter(|l| l.description[3] == 'F')
                .find_map(|l| leg_floor_ft(l).map(|ft| (ft, l)));
            if let Some((ft, leg)) = faf {
                let dist = leg
                    .fix
                    .as_ref()
                    .and_then(|f| fix_position(c, airport, f))
                    .map(|pos| distance_m(&r.threshold, &pos));
                found.push((ft, dist));
            }
        }
        if !any_approach {
            continue;
        }
        let Some(&(faf_alt_ft, dist)) = found.iter().min_by(|a, b| a.0.cmp(&b.0)) else {
            out.without_corridor
                .push(format!("{label}: no altitude at a final approach fix"));
            continue;
        };
        // The runway direction: toward the other end's threshold when the CIFP has it, else the
        // magnetic course and the airport's variation.
        let landing_course = opposite_designator(designator)
            .and_then(|o| runways_at(c, airport).find(|x| runway_designator(x) == o))
            .map(|o| bearing_deg(&r.threshold, &o.threshold))
            .or_else(|| {
                r.magnetic_bearing
                    .map(|m| (m + apt.magnetic_variation.unwrap_or(0.0)).rem_euclid(360.0))
            });
        let Some(landing_course) = landing_course else {
            out.without_corridor
                .push(format!("{label}: no runway direction"));
            continue;
        };
        let Some(thr_elev_ft) = r.threshold_elevation_ft.or(apt.elevation_ft) else {
            out.without_corridor
                .push(format!("{label}: no threshold elevation"));
            continue;
        };
        let (faf_nm, faf_from_cifp) = match dist.map(|d| d / NM_M) {
            Some(nm) if (FAF_MIN_NM..=FAF_MAX_NM).contains(&nm) => (nm, true),
            _ => (DEFAULT_FAF_NM, false),
        };
        out.corridors.push(CifpCorridor {
            label,
            corridor: Corridor {
                thr_lat_e7: (r.threshold.lat * 1e7).round() as i32,
                thr_lon_e7: (r.threshold.lon * 1e7).round() as i32,
                out_bearing_deg: (landing_course + 180.0).rem_euclid(360.0),
                length_m: faf_nm * NM_M,
                near_half_width_m: NEAR_HALF_WIDTH_NM * NM_M,
                far_half_width_m: FAR_HALF_WIDTH_NM * NM_M,
                thr_elev_ft,
                faf_alt_ft,
            },
            faf_nm,
            faf_from_cifp,
            approaches: found.len(),
        });
    }
    Ok(out)
}

/// The airports of the CIFP that have an instrument approach (the file marks them): identifier and
/// reference point, for the count of alerts near an airport with an approach.
pub fn approach_airports(c: &Cifp) -> Vec<(String, f64, f64)> {
    c.airports
        .iter()
        .filter(|a| a.ifr_capable)
        .map(|a| (a.ident.clone(), a.position.lat, a.position.lon))
        .collect()
}

/// The distance from a point to the nearest of `airports`, nautical miles; `None` if there are none.
pub fn nearest_airport_nm(airports: &[(String, f64, f64)], lat: f64, lon: f64) -> Option<f64> {
    let here = Position { lat, lon };
    airports
        .iter()
        .map(|(_, la, lo)| distance_m(&here, &Position { lat: *la, lon: *lo }) / NM_M)
        .fold(None, |best: Option<f64>, d| {
            Some(best.map_or(d, |b| b.min(d)))
        })
}
