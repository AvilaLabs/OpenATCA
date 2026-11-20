//! Final-approach corridors from the CIFP. Shell (B3): a pure function of a parsed file, kept with
//! the tool because it is the tool's reading of the CIFP and not a rule of the net.
//!
//! For each runway end of an airport that has an instrument approach with a published altitude at
//! its final approach fix, one corridor: the strip under the extended centreline from the landing
//! threshold outward to the fix (5 NM when the fix cannot be placed or is farther), a constant
//! half-width either side (legacy's capture box of 1 NM, FAA ARTS IIA A2.08), with the surface from the threshold elevation to
//! the published fix altitude (the core takes the allowance off it). The fix is the leg whose
//! fourth waypoint description code is `F`; its altitude is read as a floor (at, at or above,
//! the lower bound of a between; an at-or-below altitude is a ceiling and is not one). Where a
//! runway end has several approaches the one with the lowest fix altitude gives the corridor, so
//! that the surface never forbids what any published approach allows. A runway end with no such
//! approach has no corridor and the normal floor applies there: the net alerts, it does not
//! assume. The path of the approach is the business of approach path monitoring (build order step
//! 8), not of this net.
//!
//! Along the runway, from the landing threshold to the far end, a strip of the runway's half-width
//! plus 0.3 NM under a flat surface at the threshold elevation continues the approach corridor (it
//! begins 1 NM outside the threshold, so that a circle over the threshold is inside one of them).
//! At each runway end that has a SID (a runway transition of one) or is the reverse of an
//! approach end, a departure corridor runs 5 NM from the departure end along the runway heading,
//! 0.3 NM either side widening to 2.0 NM (A2.08's departure area reaches 2 NM), with no surface (the core gives it terrain and obstacle
//! clearance alone, and no chart altitude). A runway with a departure corridor and no approach
//! corridor gets the strip along it as well.

use oa_arinc424::{
    altitude_feet, procedures_at, runway_designator, runways_at, AltitudeDescription, Cifp, FixRef,
    Leg, Position, ProcedureKind,
};
use oa_msaw::{Corridor, CK_APPROACH, CK_DEPARTURE, CK_RUNWAY};
use std::collections::BTreeSet;

/// Metres per degree of latitude (mean radius), the core's.
const M_PER_DEG: f64 = 111_194.926_644_558_74;
const NM_M: f64 = 1852.0;
/// Half-width of a departure corridor at its start and at its far end, nautical miles. A2.08: the
/// departure area reaches 2 NM. Development values, to be fitted only on development days.
pub const NEAR_HALF_WIDTH_NM: f64 = 0.3;
pub const FAR_HALF_WIDTH_NM: f64 = 2.0;
/// The half-width of an approach corridor when the caller gives none, nautical miles (A2.08:
/// lateral monitor distance 1 NM either side of the centreline; a development value).
pub const APPROACH_HALF_WIDTH_NM: f64 = 1.0;
/// The length of an approach corridor when the fix cannot be placed or is farther than this
/// (A2.08: 5 NM with no final approach fix or one further out), nautical miles.
pub const DEFAULT_FAF_NM: f64 = 5.0;
/// A fix closer to the threshold than this is not taken for the final approach fix of a
/// straight-in approach, nautical miles.
const FAF_MIN_NM: f64 = 1.0;
/// Length of a departure corridor, nautical miles.
pub const DEPARTURE_NM: f64 = 5.0;
/// Half-width of the strip along a runway beyond the runway's own, nautical miles.
pub const RUNWAY_MARGIN_NM: f64 = 0.3;
/// How far outside the threshold the strip along the runway begins, metres.
const RUNWAY_OVERLAP_M: f64 = NM_M;
/// The runway width taken where the CIFP gives none, feet.
const DEFAULT_RUNWAY_WIDTH_FT: f64 = 200.0;
const FT_M: f64 = 0.3048;

/// One corridor of the CIFP and what it came from.
// A7.2: cold struct, size guard waived
#[derive(Debug, Clone, PartialEq)]
pub struct CifpCorridor {
    /// `CK_*` of the corridor.
    pub kind: u8,
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
    corridors_from_cifp_with(c, airport, APPROACH_HALF_WIDTH_NM)
}

/// As [`corridors_from_cifp`], with the half-width of an approach corridor, nautical miles.
pub fn corridors_from_cifp_with(
    c: &Cifp,
    airport: &str,
    approach_half_width_nm: f64,
) -> Result<CifpCorridors, String> {
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
            Some(nm) if (FAF_MIN_NM..=DEFAULT_FAF_NM).contains(&nm) => (nm, true),
            _ => (DEFAULT_FAF_NM, false),
        };
        out.corridors.push(CifpCorridor {
            kind: CK_APPROACH,
            label,
            corridor: Corridor {
                kind: CK_APPROACH,
                thr_lat_e7: (r.threshold.lat * 1e7).round() as i32,
                thr_lon_e7: (r.threshold.lon * 1e7).round() as i32,
                out_bearing_deg: (landing_course + 180.0).rem_euclid(360.0),
                length_m: faf_nm * NM_M,
                near_half_width_m: approach_half_width_nm * NM_M,
                far_half_width_m: approach_half_width_nm * NM_M,
                thr_elev_ft,
                faf_alt_ft,
            },
            faf_nm,
            faf_from_cifp,
            approaches: found.len(),
        });
    }
    add_runway_corridors(c, airport, &mut out);
    Ok(out)
}

/// The runway numbers a SID names by a runway transition (`RW27L` gives `27L`; `RW27B`, both
/// parallel runways, gives `27L`, `27R` and `27C`).
fn sid_runways(c: &Cifp, airport: &str) -> BTreeSet<String> {
    let mut v = BTreeSet::new();
    for p in procedures_at(c, airport, ProcedureKind::Sid) {
        for rt in &p.routes {
            let Some(rw) = rt.transition.strip_prefix("RW") else {
                continue;
            };
            let rw = rw.trim();
            if let Some(n) = rw.strip_suffix('B') {
                for side in ["L", "R", "C"] {
                    v.insert(format!("{n}{side}"));
                }
                v.insert(n.to_string());
            } else {
                v.insert(rw.to_string());
            }
        }
    }
    v
}

/// The point `dist_m` from (`lat`, `lon`) along `course` degrees, on the core's flat scale.
fn moved(lat: f64, lon: f64, course: f64, dist_m: f64) -> (f64, f64) {
    let (s, co) = course.to_radians().sin_cos();
    let la = lat + dist_m * co / M_PER_DEG;
    (la, lon + dist_m * s / (M_PER_DEG * lat.to_radians().cos()))
}

/// The strips along runways and the departure corridors of the airport (see the module text).
fn add_runway_corridors(c: &Cifp, airport: &str, out: &mut CifpCorridors) {
    let Some(apt) = c.airports.iter().find(|a| a.ident == airport) else {
        return;
    };
    let sids = sid_runways(c, airport);
    let with_approach: BTreeSet<String> = out
        .corridors
        .iter()
        .filter(|k| k.kind == CK_APPROACH)
        .filter_map(|k| k.label.rsplit_once(" RW").map(|(_, d)| d.to_string()))
        .collect();
    let any_approach: BTreeSet<String> = procedures_at(c, airport, ProcedureKind::Approach)
        .filter_map(|p| approach_runway(&p.ident))
        .collect();
    for r in runways_at(c, airport) {
        let designator = runway_designator(r);
        let reverse_of_approach =
            opposite_designator(designator).is_some_and(|o| any_approach.contains(&o));
        let departs = reverse_of_approach || sids.contains(designator);
        if !(departs || with_approach.contains(designator)) {
            continue;
        }
        let course = opposite_designator(designator)
            .and_then(|o| runways_at(c, airport).find(|x| runway_designator(x) == o))
            .map(|o| bearing_deg(&r.threshold, &o.threshold))
            .or_else(|| {
                r.magnetic_bearing
                    .map(|m| (m + apt.magnetic_variation.unwrap_or(0.0)).rem_euclid(360.0))
            });
        let label = format!("{airport} RW{designator}");
        let (Some(course), Some(elev)) = (course, r.threshold_elevation_ft.or(apt.elevation_ft))
        else {
            out.without_corridor.push(format!(
                "{label}: no runway strip, for want of a direction or an elevation"
            ));
            continue;
        };
        // From the landing threshold to the physical far end.
        let run_m = (f64::from(r.length_ft) - f64::from(r.displaced_threshold_ft)).max(1.0) * FT_M;
        let half_m = r.width_ft.map_or(DEFAULT_RUNWAY_WIDTH_FT, f64::from) * FT_M / 2.0
            + RUNWAY_MARGIN_NM * NM_M;
        let (lat, lon) = (r.threshold.lat, r.threshold.lon);
        let (slat, slon) = moved(lat, lon, course + 180.0, RUNWAY_OVERLAP_M);
        out.corridors.push(CifpCorridor {
            kind: CK_RUNWAY,
            label: format!("{label} strip"),
            corridor: Corridor {
                kind: CK_RUNWAY,
                thr_lat_e7: (slat * 1e7).round() as i32,
                thr_lon_e7: (slon * 1e7).round() as i32,
                out_bearing_deg: course.rem_euclid(360.0),
                length_m: run_m + RUNWAY_OVERLAP_M,
                near_half_width_m: half_m,
                far_half_width_m: half_m,
                thr_elev_ft: elev,
                faf_alt_ft: elev,
            },
            faf_nm: (run_m + RUNWAY_OVERLAP_M) / NM_M,
            faf_from_cifp: true,
            approaches: 0,
        });
        if !departs {
            continue;
        }
        let (dlat, dlon) = moved(lat, lon, course, run_m);
        out.corridors.push(CifpCorridor {
            kind: CK_DEPARTURE,
            label: format!("{label} departure"),
            corridor: Corridor {
                kind: CK_DEPARTURE,
                thr_lat_e7: (dlat * 1e7).round() as i32,
                thr_lon_e7: (dlon * 1e7).round() as i32,
                out_bearing_deg: course.rem_euclid(360.0),
                length_m: DEPARTURE_NM * NM_M,
                near_half_width_m: NEAR_HALF_WIDTH_NM * NM_M,
                far_half_width_m: FAR_HALF_WIDTH_NM * NM_M,
                thr_elev_ft: elev,
                faf_alt_ft: elev,
            },
            faf_nm: DEPARTURE_NM,
            faf_from_cifp: true,
            approaches: 0,
        });
    }
}

/// The airports of the CIFP that have an instrument approach (the file marks them): identifier and
/// reference point, for the count of alerts near an airport with an approach.
pub fn approach_airports(c: &Cifp) -> Vec<(String, f64, f64)> {
    airports_with_approach(c)
        .into_iter()
        .map(|(i, la, lo, _)| (i, la, lo))
        .collect()
}

/// The same with the airport elevation, feet (airports the file gives none for are left out: the
/// ceiling of their terminal area cannot be placed).
pub fn airports_with_approach(c: &Cifp) -> Vec<(String, f64, f64, i32)> {
    c.airports
        .iter()
        .filter(|a| a.ifr_capable)
        .filter_map(|a| {
            let e = a.elevation_ft?;
            Some((a.ident.clone(), a.position.lat, a.position.lon, e))
        })
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
