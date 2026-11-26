//! Bounded resolution of a selected CIFP procedure transition into public fix geometry.

use oa_arinc424::{altitude_feet, Airway, Cifp, FixRef, Position, ProcedureKind as CifpKind};

use crate::core::{ProcedureKind, ProcedureRouteRef, ResolvedFix, ResolvedRoute, VerticalGuidance};

/// Why a CIFP altitude field could not be represented by the scenario's bounded MSL window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AltitudeValueKind {
    Missing,
    Ground,
    Unlimited,
    Unknown,
}

/// Why a selected public CIFP procedure could not be materialized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteResolveError {
    UnsupportedKind,
    ProcedureNotFound,
    TransitionNotFound,
    AmbiguousTransition,
    MissingFix(usize),
    FixNotFound(usize),
    AmbiguousFix(usize),
    UnknownAltitude(usize),
    UnsupportedAltitudeDescription(usize, char),
    InvalidAltitudeValue(usize, u8, AltitudeValueKind),
    UnexpectedAltitudeValue(usize),
    ContradictoryAltitudeWindow(usize),
    ConflictingVerticalGuidance(usize),
    BadSpeedLimit(usize),
    BadDirectConnector,
    MissingPublishedMap,
    MultiplePublishedMaps,
}

impl ::core::fmt::Display for RouteResolveError {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        match self {
            RouteResolveError::UnsupportedKind => {
                f.write_str("only CIFP SID, STAR and approach routes can be resolved")
            }
            RouteResolveError::ProcedureNotFound => {
                f.write_str("CIFP procedure identifier not found at selected airport")
            }
            RouteResolveError::TransitionNotFound => {
                f.write_str("exact CIFP transition identifier not found")
            }
            RouteResolveError::AmbiguousTransition => f.write_str(
                "CIFP transition identifier has multiple route types; select a unique transition",
            ),
            RouteResolveError::MissingFix(n) => write!(
                f,
                "CIFP leg {n} has no named fix and cannot be geometrically resolved"
            ),
            RouteResolveError::FixNotFound(n) => write!(
                f,
                "CIFP leg {n} fix is absent from supported public position records"
            ),
            RouteResolveError::AmbiguousFix(n) => write!(
                f,
                "CIFP leg {n} fix identifier resolves to conflicting positions"
            ),
            RouteResolveError::UnknownAltitude(n) => {
                write!(f, "CIFP leg {n} has an unsupported altitude constraint")
            }
            RouteResolveError::UnsupportedAltitudeDescription(n, code) => write!(
                f,
                "CIFP leg {n} has unsupported altitude description code {code:?}"
            ),
            RouteResolveError::InvalidAltitudeValue(n, field, kind) => write!(
                f,
                "CIFP leg {n} altitude field {field} is not a numeric feet/flight-level value ({kind:?})"
            ),
            RouteResolveError::UnexpectedAltitudeValue(n) => write!(
                f,
                "CIFP leg {n} has an altitude value without a supported description"
            ),
            RouteResolveError::ContradictoryAltitudeWindow(n) => {
                write!(f, "CIFP leg {n} altitude lower bound exceeds upper bound")
            }
            RouteResolveError::ConflictingVerticalGuidance(n) => write!(
                f,
                "CIFP route join has incompatible vertical guidance at leg {n}"
            ),
            RouteResolveError::BadSpeedLimit(n) => write!(
                f,
                "CIFP leg {n} has a speed limit outside the scenario representation"
            ),
            RouteResolveError::BadDirectConnector => f.write_str(
                "synthetic DCT must connect the preceding and following resolved CIFP fixes",
            ),
            RouteResolveError::MissingPublishedMap => {
                f.write_str("approach final has no explicit published MAP marker")
            }
            RouteResolveError::MultiplePublishedMaps => {
                f.write_str("approach final has more than one published MAP marker")
            }
        }
    }
}

impl std::error::Error for RouteResolveError {}

/// Resolves one exact SID, STAR, or approach transition using only the parsed CIFP object.
/// Unsupported path terminators/fix types fail closed; no direct connector is synthesized.
pub fn resolve_procedure_route(
    cifp: &Cifp,
    reference: &ProcedureRouteRef,
) -> Result<ResolvedRoute, RouteResolveError> {
    let fixes = resolve_single(cifp, reference)?;
    Ok(ResolvedRoute {
        reference: vec![reference.clone()],
        fixes,
    })
}

/// Resolves ordered public CIFP routes. Adjacent published routes must share the same fix
/// identity and position. An explicit `ProcedureKind::Direct` item inserts a straight synthetic
/// flight-plan segment between its named preceding/following CIFP fixes; it never claims a CIFP
/// leg. Repeated shared fixes at a published procedure boundary are retained only once.
pub fn resolve_route_chain(
    cifp: &Cifp,
    references: &[ProcedureRouteRef],
) -> Result<ResolvedRoute, RouteResolveError> {
    if references.is_empty() {
        return Err(RouteResolveError::ProcedureNotFound);
    }
    let mut fixes: Vec<ResolvedFix> = Vec::new();
    let mut reference_index = 0usize;
    while reference_index < references.len() {
        let reference = references
            .get(reference_index)
            .ok_or(RouteResolveError::TransitionNotFound)?;
        if reference.kind == ProcedureKind::Direct {
            if reference.airport != "SYN"
                || reference.procedure != "DCT"
                || reference.route_type.is_some()
                || fixes.is_empty()
            {
                return Err(RouteResolveError::BadDirectConnector);
            }
            let (from, to) = reference
                .transition
                .split_once('>')
                .ok_or(RouteResolveError::BadDirectConnector)?;
            if from.is_empty() || to.is_empty() || to.contains('>') {
                return Err(RouteResolveError::BadDirectConnector);
            }
            let next_reference = references
                .get(reference_index + 1)
                .filter(|next| next.kind != ProcedureKind::Direct)
                .ok_or(RouteResolveError::BadDirectConnector)?;
            let previous = fixes.last().ok_or(RouteResolveError::BadDirectConnector)?;
            let next_route = match next_reference.kind {
                ProcedureKind::Airway => resolve_airway(cifp, next_reference)?,
                ProcedureKind::Sid | ProcedureKind::Star | ProcedureKind::Approach => {
                    resolve_single(cifp, next_reference)?
                }
                ProcedureKind::Direct => return Err(RouteResolveError::BadDirectConnector),
            };
            let next = next_route
                .first()
                .ok_or(RouteResolveError::BadDirectConnector)?;
            if previous.ident != from
                || next.ident != to
                || (previous.lat_deg == next.lat_deg && previous.lon_deg == next.lon_deg)
            {
                return Err(RouteResolveError::BadDirectConnector);
            }
            fixes.extend(next_route);
            reference_index += 2;
            continue;
        }
        let route = match reference.kind {
            ProcedureKind::Airway => resolve_airway(cifp, reference)?,
            ProcedureKind::Sid | ProcedureKind::Star | ProcedureKind::Approach => {
                let synthetic_heading_exit = references
                    .get(reference_index + 1)
                    .is_some_and(|next| next.kind == ProcedureKind::Direct);
                resolve_single_with_heading_exit(cifp, reference, synthetic_heading_exit)?
            }
            ProcedureKind::Direct => return Err(RouteResolveError::BadDirectConnector),
        };
        if route.is_empty() {
            return Err(RouteResolveError::TransitionNotFound);
        }
        let boundary_index = fixes.len().saturating_sub(1);
        if !fixes.is_empty() && !route.is_empty() {
            let Some(last) = fixes.last_mut() else {
                return Err(RouteResolveError::TransitionNotFound);
            };
            let Some(first) = route.first() else {
                return Err(RouteResolveError::TransitionNotFound);
            };
            if last.ident != first.ident {
                return Err(RouteResolveError::TransitionNotFound);
            }
            {
                if last.lat_deg != first.lat_deg || last.lon_deg != first.lon_deg {
                    return Err(RouteResolveError::AmbiguousFix(boundary_index));
                }
                last.min_altitude_ft = match (last.min_altitude_ft, first.min_altitude_ft) {
                    (Some(a), Some(b)) => Some(a.max(b)),
                    (Some(a), None) | (None, Some(a)) => Some(a),
                    (None, None) => None,
                };
                last.max_altitude_ft = match (last.max_altitude_ft, first.max_altitude_ft) {
                    (Some(a), Some(b)) => Some(a.min(b)),
                    (Some(a), None) | (None, Some(a)) => Some(a),
                    (None, None) => None,
                };
                if last
                    .min_altitude_ft
                    .zip(last.max_altitude_ft)
                    .is_some_and(|(lo, hi)| lo > hi)
                {
                    return Err(RouteResolveError::UnknownAltitude(boundary_index));
                }
                last.vertical_guidance = match (last.vertical_guidance, first.vertical_guidance) {
                    (Some(a), Some(b)) if a != b => {
                        return Err(RouteResolveError::ConflictingVerticalGuidance(
                            boundary_index,
                        ));
                    }
                    (Some(a), _) | (_, Some(a)) => Some(a),
                    (None, None) => None,
                };
                last.max_speed_kt = match (last.max_speed_kt, first.max_speed_kt) {
                    (Some(a), Some(b)) => Some(a.min(b)),
                    (Some(a), None) | (None, Some(a)) => Some(a),
                    (None, None) => None,
                };
                fixes.extend(route.into_iter().skip(1));
                reference_index += 1;
                continue;
            }
        }
        fixes.extend(route);
        reference_index += 1;
    }
    Ok(ResolvedRoute {
        reference: references.to_vec(),
        fixes,
    })
}

fn resolve_single(
    cifp: &Cifp,
    reference: &ProcedureRouteRef,
) -> Result<Vec<ResolvedFix>, RouteResolveError> {
    resolve_single_with_heading_exit(cifp, reference, false)
}

fn resolve_single_with_heading_exit(
    cifp: &Cifp,
    reference: &ProcedureRouteRef,
    synthetic_heading_exit: bool,
) -> Result<Vec<ResolvedFix>, RouteResolveError> {
    let kind = match reference.kind {
        ProcedureKind::Sid => CifpKind::Sid,
        ProcedureKind::Star => CifpKind::Star,
        ProcedureKind::Approach => CifpKind::Approach,
        ProcedureKind::Airway => return Err(RouteResolveError::UnsupportedKind),
        ProcedureKind::Direct => return Err(RouteResolveError::BadDirectConnector),
    };
    let mut procedures = cifp.procedures.iter().filter(|p| {
        p.airport_ident == reference.airport && p.kind == kind && p.ident == reference.procedure
    });
    let procedure = procedures
        .next()
        .ok_or(RouteResolveError::ProcedureNotFound)?;
    if procedures.next().is_some() {
        return Err(RouteResolveError::ProcedureNotFound);
    }
    let mut routes = procedure.routes.iter().filter(|r| {
        r.transition == reference.transition
            && reference
                .route_type
                .is_none_or(|route_type| r.route_type == route_type)
    });
    let route = routes.next().ok_or(RouteResolveError::TransitionNotFound)?;
    if routes.next().is_some() {
        return Err(RouteResolveError::AmbiguousTransition);
    }
    // CIFP description slot 4 is column 43; only its explicit FAA MAP flag closes the
    // normal-landing portion. Later missed-approach legs remain in the source record but are
    // not treated as normal-arrival fixes.
    let normal_final_leg_count =
        if reference.kind == ProcedureKind::Approach && reference.transition.is_empty() {
            let mut map_markers = route
                .legs
                .iter()
                .enumerate()
                .filter(|(_, leg)| leg.description.get(3).copied() == Some('M'));
            let (map_index, _) = map_markers
                .next()
                .ok_or(RouteResolveError::MissingPublishedMap)?;
            if map_markers.next().is_some() {
                return Err(RouteResolveError::MultiplePublishedMaps);
            }
            map_index.saturating_add(1)
        } else {
            route.legs.len()
        };
    let mut fixes = Vec::with_capacity(normal_final_leg_count);
    for (index, leg) in route.legs.iter().take(normal_final_leg_count).enumerate() {
        let fix_ref = leg
            .fix
            .as_ref()
            .ok_or(RouteResolveError::MissingFix(index))?;
        let position =
            resolve_position(cifp, fix_ref, &reference.airport).map_err(|error| match error {
                PositionResolveError::NotFound => RouteResolveError::FixNotFound(index),
                PositionResolveError::Ambiguous => RouteResolveError::AmbiguousFix(index),
            })?;
        let (min_altitude_ft, max_altitude_ft, vertical_guidance) =
            leg_altitude_window(leg, index)?;
        let max_speed_kt = leg
            .speed_limit_kt
            .map(u16::try_from)
            .transpose()
            .map_err(|_| RouteResolveError::BadSpeedLimit(index))?;
        // A published FM leg starts at a fix and continues on a heading until manual
        // termination; it supplies no second geographic endpoint. Only an explicitly
        // requested synthetic DCT may replace that final heading portion. Retain its
        // crossing constraints at the identical source fix; never erase another leg type.
        let replaces_terminal_heading = synthetic_heading_exit
            && reference.kind == ProcedureKind::Star
            && reference.route_type == Some('6')
            && index.checked_add(1) == Some(normal_final_leg_count)
            && leg.path_terminator == oa_arinc424::PathTerminator::FM
            && index
                .checked_sub(1)
                .and_then(|i| route.legs.get(i))
                .is_some_and(|previous| previous.fix == leg.fix);
        if replaces_terminal_heading {
            let previous: &mut ResolvedFix = fixes
                .last_mut()
                .ok_or(RouteResolveError::MissingFix(index))?;
            if previous.lat_deg != position.lat || previous.lon_deg != position.lon {
                return Err(RouteResolveError::AmbiguousFix(index));
            }
            previous.min_altitude_ft = match (previous.min_altitude_ft, min_altitude_ft) {
                (Some(a), Some(b)) => Some(a.max(b)),
                (a, b) => a.or(b),
            };
            previous.max_altitude_ft = match (previous.max_altitude_ft, max_altitude_ft) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
            if previous
                .min_altitude_ft
                .zip(previous.max_altitude_ft)
                .is_some_and(|(a, b)| a > b)
            {
                return Err(RouteResolveError::ContradictoryAltitudeWindow(index));
            }
            previous.vertical_guidance = match (previous.vertical_guidance, vertical_guidance) {
                (Some(a), Some(b)) if a != b => {
                    return Err(RouteResolveError::ConflictingVerticalGuidance(index))
                }
                (a, b) => a.or(b),
            };
            previous.max_speed_kt = match (previous.max_speed_kt, max_speed_kt) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
            continue;
        }
        fixes.push(ResolvedFix {
            ident: fix_ref.ident.clone(),
            lat_deg: position.lat,
            lon_deg: position.lon,
            min_altitude_ft,
            max_altitude_ft,
            vertical_guidance,
            max_speed_kt,
        });
    }
    Ok(fixes)
}

fn resolve_airway(
    cifp: &Cifp,
    reference: &ProcedureRouteRef,
) -> Result<Vec<ResolvedFix>, RouteResolveError> {
    let (from, to) = reference
        .transition
        .split_once('>')
        .ok_or(RouteResolveError::TransitionNotFound)?;
    if from.is_empty() || to.is_empty() || to.contains('>') {
        return Err(RouteResolveError::TransitionNotFound);
    }
    let mut airways = cifp
        .airways
        .iter()
        .filter(|a| a.ident == reference.procedure);
    let airway = airways.next().ok_or(RouteResolveError::ProcedureNotFound)?;
    if airways.next().is_some() {
        return Err(RouteResolveError::ProcedureNotFound);
    }
    airway_segment(cifp, airway, from, to)
}

fn airway_segment(
    cifp: &Cifp,
    airway: &Airway,
    from: &str,
    to: &str,
) -> Result<Vec<ResolvedFix>, RouteResolveError> {
    let from_ix = airway
        .fixes
        .iter()
        .position(|fix| fix.fix.ident == from)
        .ok_or(RouteResolveError::TransitionNotFound)?;
    let to_ix = airway
        .fixes
        .iter()
        .position(|fix| fix.fix.ident == to)
        .ok_or(RouteResolveError::TransitionNotFound)?;
    if from_ix == to_ix {
        return Err(RouteResolveError::TransitionNotFound);
    }
    let (lo, hi, reverse) = if from_ix < to_ix {
        (from_ix, to_ix, false)
    } else {
        (to_ix, from_ix, true)
    };
    let Some(slice) = airway.fixes.get(lo..=hi) else {
        return Err(RouteResolveError::TransitionNotFound);
    };
    let ordered: Vec<_> = if reverse {
        slice.iter().rev().collect()
    } else {
        slice.iter().collect()
    };
    let mut out = Vec::with_capacity(ordered.len());
    for (index, fix) in ordered.into_iter().enumerate() {
        let position = resolve_position(cifp, &fix.fix, "ENRT").map_err(|error| match error {
            PositionResolveError::NotFound => RouteResolveError::FixNotFound(index),
            PositionResolveError::Ambiguous => RouteResolveError::AmbiguousFix(index),
        })?;
        let (min_altitude_ft, max_altitude_ft) = airway_altitude_window(fix, index, reverse)?;
        out.push(ResolvedFix {
            ident: fix.fix.ident.clone(),
            lat_deg: position.lat,
            lon_deg: position.lon,
            min_altitude_ft,
            max_altitude_ft,
            vertical_guidance: None,
            max_speed_kt: None,
        });
    }
    Ok(out)
}

fn airway_altitude_window(
    fix: &oa_arinc424::AirwayFix,
    index: usize,
    reverse: bool,
) -> Result<(Option<i32>, Option<i32>), RouteResolveError> {
    let min = if reverse {
        fix.min_altitude_2
            .or(fix.min_altitude)
            .and_then(altitude_feet)
    } else {
        fix.min_altitude.and_then(altitude_feet)
    };
    let max = fix.max_altitude.and_then(altitude_feet);
    if min.zip(max).is_some_and(|(lo, hi)| lo > hi) {
        return Err(RouteResolveError::UnknownAltitude(index));
    }
    Ok((min, max))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PositionResolveError {
    NotFound,
    Ambiguous,
}

fn resolve_position(
    cifp: &Cifp,
    fix: &FixRef,
    airport: &str,
) -> Result<Position, PositionResolveError> {
    let mut found: Option<Position> = None;
    let mut consider = |candidate: Option<Position>| -> bool {
        let Some(candidate) = candidate else {
            return true;
        };
        if found.is_some_and(|prior| prior != candidate) {
            return false;
        }
        found = Some(candidate);
        true
    };
    match (fix.section, fix.subsection) {
        ('E', 'A') => {
            for w in cifp
                .enroute_waypoints
                .iter()
                .filter(|w| w.ident == fix.ident && w.icao_region == fix.icao_region)
            {
                if !consider(Some(Position {
                    lat: w.position.lat,
                    lon: w.position.lon,
                })) {
                    return Err(PositionResolveError::Ambiguous);
                }
            }
        }
        ('P', 'C') => {
            for w in cifp.terminal_waypoints.iter().filter(|w| {
                w.ident == fix.ident && w.icao_region == fix.icao_region && w.area == airport
            }) {
                if !consider(Some(Position {
                    lat: w.position.lat,
                    lon: w.position.lon,
                })) {
                    return Err(PositionResolveError::Ambiguous);
                }
            }
        }
        ('D', ' ') => {
            for n in cifp
                .vhf_navaids
                .iter()
                .filter(|n| n.ident == fix.ident && n.icao_region == fix.icao_region)
            {
                if let (Some(vor), Some(dme)) = (n.vor_position, n.dme_position) {
                    if vor != dme {
                        return Err(PositionResolveError::Ambiguous);
                    }
                }
                if !consider(n.vor_position.or(n.dme_position)) {
                    return Err(PositionResolveError::Ambiguous);
                }
            }
        }
        ('D', 'B') => {
            for n in cifp
                .ndbs
                .iter()
                .filter(|n| n.ident == fix.ident && n.icao_region == fix.icao_region)
            {
                if !consider(Some(Position {
                    lat: n.position.lat,
                    lon: n.position.lon,
                })) {
                    return Err(PositionResolveError::Ambiguous);
                }
            }
        }
        ('P', 'N') => {
            for n in cifp.terminal_ndbs.iter().filter(|n| {
                n.ident == fix.ident
                    && n.icao_region == fix.icao_region
                    && n.airport_ident.as_deref() == Some(airport)
            }) {
                if !consider(Some(Position {
                    lat: n.position.lat,
                    lon: n.position.lon,
                })) {
                    return Err(PositionResolveError::Ambiguous);
                }
            }
        }
        ('P', 'G') => {
            for r in cifp
                .runways
                .iter()
                .filter(|r| r.ident == fix.ident && r.airport_ident == airport)
            {
                if !consider(Some(Position {
                    lat: r.threshold.lat,
                    lon: r.threshold.lon,
                })) {
                    return Err(PositionResolveError::Ambiguous);
                }
            }
        }
        ('P', 'I') => {
            for l in cifp
                .localizers
                .iter()
                .filter(|l| l.ident == fix.ident && l.airport_ident == airport)
            {
                if !consider(Some(Position {
                    lat: l.localizer_position.lat,
                    lon: l.localizer_position.lon,
                })) {
                    return Err(PositionResolveError::Ambiguous);
                }
            }
        }
        ('P', 'A') => {
            for a in cifp.airports.iter().filter(|a| a.ident == fix.ident) {
                if !consider(Some(Position {
                    lat: a.position.lat,
                    lon: a.position.lon,
                })) {
                    return Err(PositionResolveError::Ambiguous);
                }
            }
        }
        _ => return Err(PositionResolveError::NotFound),
    }
    found.ok_or(PositionResolveError::NotFound)
}

type LegAltitudeResolution = (Option<i32>, Option<i32>, Option<VerticalGuidance>);

fn leg_altitude_window(
    leg: &oa_arinc424::Leg,
    index: usize,
) -> Result<LegAltitudeResolution, RouteResolveError> {
    use oa_arinc424::{Altitude, AltitudeDescription};
    let required_value = |value: Option<Altitude>, field: u8| match value {
        Some(Altitude::Feet(feet)) => Ok(feet),
        Some(Altitude::FlightLevel(level)) => Ok(i32::from(level) * 100),
        None => Err(RouteResolveError::InvalidAltitudeValue(
            index,
            field,
            AltitudeValueKind::Missing,
        )),
        Some(Altitude::Ground) => Err(RouteResolveError::InvalidAltitudeValue(
            index,
            field,
            AltitudeValueKind::Ground,
        )),
        Some(Altitude::Unlimited) => Err(RouteResolveError::InvalidAltitudeValue(
            index,
            field,
            AltitudeValueKind::Unlimited,
        )),
        Some(Altitude::Unknown) => Err(RouteResolveError::InvalidAltitudeValue(
            index,
            field,
            AltitudeValueKind::Unknown,
        )),
    };
    let (window, vertical_guidance) = match leg.altitude_description {
        Some(AltitudeDescription::At) => {
            let at = required_value(leg.altitude1, 1)?;
            ((Some(at), Some(at)), None)
        }
        Some(AltitudeDescription::AtOrAbove) => {
            ((Some(required_value(leg.altitude1, 1)?), None), None)
        }
        Some(AltitudeDescription::AtOrBelow) => {
            ((None, Some(required_value(leg.altitude1, 1)?)), None)
        }
        Some(AltitudeDescription::Between) => {
            let high = required_value(leg.altitude1, 1)?;
            let low = required_value(leg.altitude2, 2)?;
            ((Some(low), Some(high)), None)
        }
        // ARINC 424-18 blank/@ mean an at-Altitude-1 constraint. The Navigraph DFD
        // v1 Appendix “Altitude Description” explicitly identifies its source as 424-18
        // and documents this mapping; 424-17 §5.29 independently describes the blank case.
        // Altitude 2 is never silently discarded for a blank descriptor.
        None if leg.altitude1.is_some() && leg.altitude2.is_none() => {
            let at = required_value(leg.altitude1, 1)?;
            ((Some(at), Some(at)), None)
        }
        None if leg.altitude1.is_none() && leg.altitude2.is_none() => ((None, None), None),
        None => return Err(RouteResolveError::UnexpectedAltitudeValue(index)),
        // ARINC 424-18 J: Altitude 1 is the at-or-above crossing constraint; Altitude 2
        // is the glide-slope intercept value. Preserve Altitude 2 (and its source vertical
        // angle, if present) as guidance metadata, not as an altitude window. This model
        // validates constraints at fixes but does not synthesize a glide-slope trajectory.
        // Source: https://developers.navigraph.com/docs/navigation-data/dfd-data-format
        // (v1 Introduction identifies ARINC 424-18; Appendix “Altitude Description”, J).
        Some(AltitudeDescription::Other('J')) if leg.kind == CifpKind::Approach => {
            let floor = required_value(leg.altitude1, 1)?;
            let intercept = required_value(leg.altitude2, 2)?;
            (
                (Some(floor), None),
                Some(VerticalGuidance::GlideSlopeIntercept {
                    altitude2_ft: intercept,
                    angle_deg: leg.vertical_angle_deg,
                }),
            )
        }
        // ARINC 424-18 H: preserve Altitude 2 as the glide-slope altitude at this fix,
        // while Altitude 1 remains the at-or-above crossing constraint. As with J, this
        // resolver does not synthesize a glide-slope trajectory. The v1 DFD altitude table
        // explicitly identifies ARINC 424-18 as its source and documents H semantics.
        Some(AltitudeDescription::Other('H')) if leg.kind == CifpKind::Approach => {
            let floor = required_value(leg.altitude1, 1)?;
            let path_altitude = required_value(leg.altitude2, 2)?;
            (
                (Some(floor), None),
                Some(VerticalGuidance::GlideSlopeAtFix {
                    altitude2_ft: path_altitude,
                    angle_deg: leg.vertical_angle_deg,
                }),
            )
        }
        Some(AltitudeDescription::Other(code)) => {
            return Err(RouteResolveError::UnsupportedAltitudeDescription(
                index, code,
            ));
        }
    };
    if window.0.zip(window.1).is_some_and(|(lo, hi)| lo > hi) {
        return Err(RouteResolveError::ContradictoryAltitudeWindow(index));
    }
    Ok((window.0, window.1, vertical_guidance))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

    use super::*;
    use oa_arinc424::{
        Altitude, AltitudeDescription, Cycle, Leg, Meta, PathTerminator, Procedure, Route,
        VhfNavaid, Waypoint, WaypointKind,
    };

    fn meta() -> Meta {
        Meta {
            line: 1,
            customer_area: "".into(),
            file_record_number: "".into(),
            cycle: Cycle {
                year: 26,
                release: 11,
            },
        }
    }

    fn leg(kind: CifpKind, ident: &str, route_type: char, transition: &str) -> Leg {
        Leg {
            meta: meta(),
            airport_ident: "KORD".into(),
            airport_region: "K5".into(),
            kind,
            ident: ident.into(),
            route_type,
            transition: transition.into(),
            sequence: 1,
            fix: Some(FixRef {
                ident: ident.into(),
                icao_region: "K5".into(),
                section: 'P',
                subsection: 'C',
            }),
            description: [' '; 4],
            turn_direction: None,
            rnp_code: None,
            path_terminator: PathTerminator::IF,
            recommended_navaid: None,
            arc_radius_nm: None,
            theta_deg: None,
            rho_nm: None,
            course_deg: None,
            distance_or_time: None,
            altitude_description: None,
            altitude1: None,
            altitude2: None,
            transition_altitude: None,
            speed_limit_kt: None,
            vertical_angle_deg: None,
            center_fix: None,
            gnss_fms_indication: None,
            speed_limit_description: None,
            route_qualifier_1: None,
            route_qualifier_2: None,
            continuations: Vec::new(),
        }
    }

    fn waypoint(ident: &str, lat: f64) -> Waypoint {
        Waypoint {
            meta: meta(),
            kind: WaypointKind::Terminal,
            area: "KORD".into(),
            area_region: "K5".into(),
            ident: ident.into(),
            icao_region: "K5".into(),
            waypoint_type: "W".into(),
            usage: "".into(),
            position: Position { lat, lon: -87.9 },
            magnetic_variation: None,
            datum: "NAR".into(),
            name: ident.into(),
        }
    }

    fn fixture() -> Cifp {
        Cifp {
            terminal_waypoints: vec![waypoint("FROM", 41.0), waypoint("TO", 41.1)],
            procedures: vec![
                Procedure {
                    customer_area: "".into(),
                    airport_ident: "KORD".into(),
                    airport_region: "K5".into(),
                    kind: CifpKind::Star,
                    ident: "STAR1".into(),
                    routes: vec![Route {
                        route_type: '4',
                        transition: "INBD".into(),
                        legs: vec![leg(CifpKind::Star, "FROM", '4', "INBD")],
                    }],
                },
                Procedure {
                    customer_area: "".into(),
                    airport_ident: "KORD".into(),
                    airport_region: "K5".into(),
                    kind: CifpKind::Approach,
                    ident: "I04R".into(),
                    routes: vec![Route {
                        route_type: 'A',
                        transition: "ENTRY".into(),
                        legs: vec![leg(CifpKind::Approach, "TO", 'A', "ENTRY")],
                    }],
                },
            ],
            ..Cifp::default()
        }
    }

    fn component_fixture() -> Cifp {
        let route = |kind, route_type, transition: &str, names: &[&str]| Route {
            route_type,
            transition: transition.into(),
            legs: names
                .iter()
                .enumerate()
                .map(|(index, name)| {
                    let mut item = leg(kind, name, route_type, transition);
                    item.sequence = index as u32 + 1;
                    item
                })
                .collect(),
        };
        let terminal_waypoints = [
            "FROM", "COMMON", "RUNWAY", "EXIT", "APP", "IAF", "MAP", "MISSED1", "MISSED2",
        ]
        .iter()
        .enumerate()
        .map(|(index, name)| waypoint(name, 41.0 + index as f64 * 0.01))
        .collect();
        let mut transition = route(CifpKind::Star, '4', "INBD", &["FROM", "COMMON"]);
        let mut common = route(CifpKind::Star, '5', "", &["COMMON", "RUNWAY"]);
        let runway = route(CifpKind::Star, '6', "RW04B", &["RUNWAY", "EXIT"]);
        transition.legs[1].altitude_description = Some(AltitudeDescription::AtOrAbove);
        transition.legs[1].altitude1 = Some(Altitude::Feet(10_000));
        common.legs[0].altitude_description = Some(AltitudeDescription::AtOrAbove);
        common.legs[0].altitude1 = Some(Altitude::Feet(12_000));
        let mut approach_final = route(
            CifpKind::Approach,
            'I',
            "",
            &["IAF", "MAP", "MISSED1", "MISSED2"],
        );
        approach_final
            .legs
            .get_mut(1)
            .expect("MAP fixture leg exists")
            .description[3] = 'M';
        Cifp {
            terminal_waypoints,
            procedures: vec![
                Procedure {
                    customer_area: "".into(),
                    airport_ident: "KORD".into(),
                    airport_region: "K5".into(),
                    kind: CifpKind::Star,
                    ident: "STAR1".into(),
                    routes: vec![transition, common, runway],
                },
                Procedure {
                    customer_area: "".into(),
                    airport_ident: "KORD".into(),
                    airport_region: "K5".into(),
                    kind: CifpKind::Approach,
                    ident: "I04R".into(),
                    routes: vec![
                        route(CifpKind::Approach, 'A', "ENTRY", &["APP", "IAF"]),
                        approach_final,
                    ],
                },
            ],
            ..Cifp::default()
        }
    }

    fn direct(from: &str, to: &str) -> ProcedureRouteRef {
        ProcedureRouteRef {
            airport: "SYN".into(),
            kind: ProcedureKind::Direct,
            procedure: "DCT".into(),
            transition: format!("{from}>{to}"),
            route_type: None,
        }
    }

    fn heading_exit_fixture() -> (Cifp, Vec<ProcedureRouteRef>) {
        let mut cifp = component_fixture();
        let runway = &mut cifp.procedures[0].routes[2];
        let mut heading = runway.legs[1].clone();
        heading.sequence += 1;
        heading.path_terminator = PathTerminator::FM;
        heading.altitude_description = Some(AltitudeDescription::AtOrAbove);
        heading.altitude1 = Some(Altitude::Feet(6_000));
        heading.speed_limit_kt = Some(220);
        runway.legs.push(heading);
        let refs = vec![
            ProcedureRouteRef {
                airport: "KORD".into(),
                kind: ProcedureKind::Star,
                procedure: "STAR1".into(),
                transition: "RW04B".into(),
                route_type: Some('6'),
            },
            direct("EXIT", "APP"),
            ProcedureRouteRef {
                airport: "KORD".into(),
                kind: ProcedureKind::Approach,
                procedure: "I04R".into(),
                transition: "ENTRY".into(),
                route_type: Some('A'),
            },
        ];
        (cifp, refs)
    }

    #[test]
    fn explicit_synthetic_exit_replaces_terminal_fm_geometry_and_keeps_constraints() {
        let (cifp, refs) = heading_exit_fixture();
        let route = resolve_route_chain(&cifp, &refs).unwrap();
        assert_eq!(
            route
                .fixes
                .iter()
                .map(|f| f.ident.as_str())
                .collect::<Vec<_>>(),
            ["RUNWAY", "EXIT", "APP", "IAF"]
        );
        assert_eq!(route.fixes[1].min_altitude_ft, Some(6_000));
        assert_eq!(route.fixes[1].max_speed_kt, Some(220));
        assert_eq!(
            resolve_procedure_route(&cifp, &refs[0])
                .unwrap()
                .fixes
                .len(),
            3
        );
    }

    #[test]
    fn synthetic_exit_does_not_collapse_a_repeated_non_fm_leg() {
        let (mut cifp, refs) = heading_exit_fixture();
        cifp.procedures[0].routes[2].legs[2].path_terminator = PathTerminator::RF;
        assert_eq!(resolve_route_chain(&cifp, &refs).unwrap().fixes.len(), 5);
    }

    #[test]
    fn synthetic_heading_exit_refuses_conflicting_crossing_constraints() {
        let (mut cifp, refs) = heading_exit_fixture();
        let previous = &mut cifp.procedures[0].routes[2].legs[1];
        previous.altitude_description = Some(AltitudeDescription::AtOrBelow);
        previous.altitude1 = Some(Altitude::Feet(5_000));
        assert_eq!(
            resolve_route_chain(&cifp, &refs),
            Err(RouteResolveError::ContradictoryAltitudeWindow(2))
        );
    }

    #[test]
    fn distinct_vor_and_dme_positions_refuse_route_resolution_as_ambiguous() {
        let mut cifp = fixture();
        let route = cifp.procedures.first_mut().expect("STAR fixture exists");
        let fix = route
            .routes
            .first_mut()
            .and_then(|item| item.legs.first_mut())
            .and_then(|item| item.fix.as_mut())
            .expect("STAR fixture fix exists");
        fix.ident = "VHF1".into();
        fix.section = 'D';
        fix.subsection = ' ';
        cifp.vhf_navaids.push(VhfNavaid {
            meta: meta(),
            airport_ident: None,
            airport_region: None,
            ident: "VHF1".into(),
            icao_region: "K5".into(),
            frequency_mhz: None,
            class: ['V', ' ', ' ', ' ', ' '],
            vor_position: Some(Position {
                lat: 41.0,
                lon: -88.0,
            }),
            dme_ident: None,
            dme_position: Some(Position {
                lat: 41.001,
                lon: -88.0,
            }),
            station_declination: None,
            dme_elevation_ft: None,
            figure_of_merit: None,
            datum: "NAR".into(),
            name: "SYNTHETIC TEST NAVAID".into(),
        });
        let reference = ProcedureRouteRef {
            airport: "KORD".into(),
            kind: ProcedureKind::Star,
            procedure: "STAR1".into(),
            transition: "INBD".into(),
            route_type: Some('4'),
        };
        assert_eq!(
            resolve_procedure_route(&cifp, &reference),
            Err(RouteResolveError::AmbiguousFix(0))
        );
        assert_eq!(
            RouteResolveError::AmbiguousFix(0).to_string(),
            "CIFP leg 0 fix identifier resolves to conflicting positions"
        );
    }

    #[test]
    fn explicit_synthetic_direct_joins_distinct_cifp_route_endpoints() {
        let cifp = fixture();
        let references = [
            ProcedureRouteRef {
                airport: "KORD".into(),
                kind: ProcedureKind::Star,
                procedure: "STAR1".into(),
                transition: "INBD".into(),
                route_type: Some('4'),
            },
            direct("FROM", "TO"),
            ProcedureRouteRef {
                airport: "KORD".into(),
                kind: ProcedureKind::Approach,
                procedure: "I04R".into(),
                transition: "ENTRY".into(),
                route_type: Some('A'),
            },
        ];
        let resolved = resolve_route_chain(&cifp, &references).expect("valid explicit DCT");
        assert_eq!(
            resolved
                .fixes
                .iter()
                .map(|fix| fix.ident.as_str())
                .collect::<Vec<_>>(),
            ["FROM", "TO"]
        );
        assert_eq!(resolved.reference[1].kind, ProcedureKind::Direct);
    }

    #[test]
    fn direct_connector_must_match_both_adjacent_resolved_fixes() {
        let cifp = fixture();
        let references = [
            ProcedureRouteRef {
                airport: "KORD".into(),
                kind: ProcedureKind::Star,
                procedure: "STAR1".into(),
                transition: "INBD".into(),
                route_type: Some('4'),
            },
            direct("WRONG", "TO"),
            ProcedureRouteRef {
                airport: "KORD".into(),
                kind: ProcedureKind::Approach,
                procedure: "I04R".into(),
                transition: "ENTRY".into(),
                route_type: Some('A'),
            },
        ];
        assert_eq!(
            resolve_route_chain(&cifp, &references),
            Err(RouteResolveError::BadDirectConnector)
        );
    }

    #[test]
    fn route_chain_composes_star_and_approach_components_in_order() {
        let cifp = component_fixture();
        let references = [
            ProcedureRouteRef {
                airport: "KORD".into(),
                kind: ProcedureKind::Star,
                procedure: "STAR1".into(),
                transition: "INBD".into(),
                route_type: Some('4'),
            },
            ProcedureRouteRef {
                airport: "KORD".into(),
                kind: ProcedureKind::Star,
                procedure: "STAR1".into(),
                transition: "".into(),
                route_type: Some('5'),
            },
            ProcedureRouteRef {
                airport: "KORD".into(),
                kind: ProcedureKind::Star,
                procedure: "STAR1".into(),
                transition: "RW04B".into(),
                route_type: Some('6'),
            },
            direct("EXIT", "APP"),
            ProcedureRouteRef {
                airport: "KORD".into(),
                kind: ProcedureKind::Approach,
                procedure: "I04R".into(),
                transition: "ENTRY".into(),
                route_type: Some('A'),
            },
            ProcedureRouteRef {
                airport: "KORD".into(),
                kind: ProcedureKind::Approach,
                procedure: "I04R".into(),
                transition: "".into(),
                route_type: Some('I'),
            },
        ];
        let resolved = resolve_route_chain(&cifp, &references).expect("all route parts join");
        assert_eq!(
            resolved
                .fixes
                .iter()
                .map(|fix| fix.ident.as_str())
                .collect::<Vec<_>>(),
            ["FROM", "COMMON", "RUNWAY", "EXIT", "APP", "IAF", "MAP"]
        );
        assert_eq!(resolved.fixes[1].min_altitude_ft, Some(12_000));
        assert_eq!(resolved.reference[3].kind, ProcedureKind::Direct);
    }

    // REQ: OA-DEMO-001
    #[test]
    fn approach_final_stops_at_explicit_map_and_requires_the_marker() {
        let mut cifp = component_fixture();
        let final_reference = ProcedureRouteRef {
            airport: "KORD".into(),
            kind: ProcedureKind::Approach,
            procedure: "I04R".into(),
            transition: String::new(),
            route_type: Some('I'),
        };
        let final_route = resolve_procedure_route(&cifp, &final_reference).unwrap();
        assert_eq!(
            final_route
                .fixes
                .iter()
                .map(|fix| fix.ident.as_str())
                .collect::<Vec<_>>(),
            ["IAF", "MAP"]
        );
        let marker = cifp
            .procedures
            .get_mut(1)
            .expect("approach fixture procedure exists")
            .routes
            .get_mut(1)
            .expect("approach final fixture exists")
            .legs
            .get_mut(1)
            .expect("MAP fixture leg exists")
            .description
            .get_mut(3)
            .expect("MAP description slot exists");
        *marker = ' ';
        assert_eq!(
            resolve_procedure_route(&cifp, &final_reference),
            Err(RouteResolveError::MissingPublishedMap)
        );

        let mut cifp = component_fixture();
        let final_route = cifp
            .procedures
            .get_mut(1)
            .expect("approach fixture procedure exists")
            .routes
            .get_mut(1)
            .expect("approach final fixture exists");
        final_route
            .legs
            .get_mut(2)
            .expect("missed approach fixture leg exists")
            .description[3] = 'M';
        assert_eq!(
            resolve_procedure_route(&cifp, &final_reference),
            Err(RouteResolveError::MultiplePublishedMaps)
        );
    }

    #[test]
    fn blank_altitude_descriptor_with_altitude1_means_at_and_altitude2_refuses() {
        let mut approach_leg = leg(CifpKind::Approach, "FIX", 'I', "");
        approach_leg.altitude1 = Some(Altitude::Feet(3_000));
        assert_eq!(
            leg_altitude_window(&approach_leg, 0),
            Ok((Some(3_000), Some(3_000), None))
        );

        approach_leg.altitude1 = None;
        approach_leg.altitude2 = Some(Altitude::Feet(2_800));
        assert_eq!(
            leg_altitude_window(&approach_leg, 0),
            Err(RouteResolveError::UnexpectedAltitudeValue(0))
        );

        approach_leg.altitude1 = Some(Altitude::Feet(3_000));
        assert_eq!(
            leg_altitude_window(&approach_leg, 0),
            Err(RouteResolveError::UnexpectedAltitudeValue(0))
        );
    }

    #[test]
    fn j_keeps_crossing_floor_and_glideslope_intercept_separate() {
        let mut item = leg(CifpKind::Approach, "FIX", 'I', "");
        item.altitude_description = Some(AltitudeDescription::Other('J'));
        item.altitude1 = Some(Altitude::Feet(3_000));
        item.altitude2 = Some(Altitude::Feet(2_800));
        item.vertical_angle_deg = Some(-3.0);
        assert_eq!(
            leg_altitude_window(&item, 0),
            Ok((
                Some(3_000),
                None,
                Some(VerticalGuidance::GlideSlopeIntercept {
                    altitude2_ft: 2_800,
                    angle_deg: Some(-3.0),
                }),
            ))
        );

        item.altitude2 = None;
        assert_eq!(
            leg_altitude_window(&item, 0),
            Err(RouteResolveError::InvalidAltitudeValue(
                0,
                2,
                AltitudeValueKind::Missing
            ))
        );

        let mut sid = leg(CifpKind::Sid, "FIX", '1', "");
        sid.altitude_description = Some(AltitudeDescription::Other('J'));
        sid.altitude1 = Some(Altitude::Feet(3_000));
        sid.altitude2 = Some(Altitude::Feet(2_800));
        assert_eq!(
            leg_altitude_window(&sid, 0),
            Err(RouteResolveError::UnsupportedAltitudeDescription(0, 'J'))
        );
    }

    #[test]
    fn h_keeps_crossing_floor_and_glideslope_fix_altitude_separate() {
        let mut item = leg(CifpKind::Approach, "FIX", 'I', "");
        item.altitude_description = Some(AltitudeDescription::Other('H'));
        item.altitude1 = Some(Altitude::Feet(2_200));
        item.altitude2 = Some(Altitude::Feet(2_128));
        item.vertical_angle_deg = Some(-3.0);
        assert_eq!(
            leg_altitude_window(&item, 0),
            Ok((
                Some(2_200),
                None,
                Some(VerticalGuidance::GlideSlopeAtFix {
                    altitude2_ft: 2_128,
                    angle_deg: Some(-3.0),
                }),
            ))
        );

        let mut star = leg(CifpKind::Star, "FIX", '1', "");
        star.altitude_description = Some(AltitudeDescription::Other('H'));
        star.altitude1 = Some(Altitude::Feet(2_200));
        star.altitude2 = Some(Altitude::Feet(2_128));
        assert_eq!(
            leg_altitude_window(&star, 0),
            Err(RouteResolveError::UnsupportedAltitudeDescription(0, 'H'))
        );

        item.altitude1 = None;
        assert_eq!(
            leg_altitude_window(&item, 0),
            Err(RouteResolveError::InvalidAltitudeValue(
                0,
                1,
                AltitudeValueKind::Missing
            ))
        );
    }

    #[test]
    fn unsupported_altitude_description_and_values_have_distinct_refusals() {
        let mut item = leg(CifpKind::Approach, "FIX", 'I', "");
        item.altitude_description = Some(AltitudeDescription::Other('V'));
        assert_eq!(
            leg_altitude_window(&item, 0),
            Err(RouteResolveError::UnsupportedAltitudeDescription(0, 'V'))
        );

        item.altitude_description = Some(AltitudeDescription::AtOrAbove);
        item.altitude1 = Some(Altitude::Ground);
        assert_eq!(
            leg_altitude_window(&item, 0),
            Err(RouteResolveError::InvalidAltitudeValue(
                0,
                1,
                AltitudeValueKind::Ground
            ))
        );

        item.altitude1 = Some(Altitude::Feet(3_000));
        item.altitude_description = Some(AltitudeDescription::Between);
        item.altitude2 = Some(Altitude::Feet(4_000));
        assert_eq!(
            leg_altitude_window(&item, 0),
            Err(RouteResolveError::ContradictoryAltitudeWindow(0))
        );
    }
}
