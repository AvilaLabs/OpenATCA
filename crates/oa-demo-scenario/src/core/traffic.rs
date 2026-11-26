//! Deterministic synthetic path materialization from resolved public CIFP fixes.
//!
//! All kinematic values are explicit scenario assumptions, not an aircraft flight model.
//! The path follows WGS-84 geodesics between published fixes with instantaneous heading changes
//! at fixes (no bank/turn model) and constant synthetic groundspeed. CIFP speed constraints are
//! checked against a separate indicated-airspeed proxy; no atmosphere or wind conversion exists.
//! Altitude limits validate the samples. Output has a fixed cap of [`MAX_SYNTHETIC_REPORTS`]
//! records and is reserved up front so growth never exceeds that ceiling. This module emits
//! scenario-owned positions and kinematics only; a private preparation adapter encodes wire bytes.

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use oa_geo::{wgs84, LatLon};

use super::scenario::{ProcedureRouteRef, SyntheticScenario, MAX_DURATION_MS};

/// Limit on how far before scenario T0 a generated route may begin.
pub const MAX_PRE_SCENARIO_MS: i64 = 6 * 60 * 60 * 1_000;
/// Maximum retained scenario samples (60 aircraft × 60 minutes at one-second minimum cadence).
pub const MAX_SYNTHETIC_REPORTS: usize = 360_000;
/// Maximum resolved fixes in one synthetic route. The prep chain is at most one 1,024-fix
/// airway plus four 256-leg CIFP route pieces (2,048 before join deduplication); 4,096 leaves
/// headroom and bounds planner windows, altitudes, and segments to under 400 KiB.
pub const MAX_RESOLVED_ROUTE_FIXES: usize = 4_096;

/// A decoded public CIFP fix plus its speed and altitude constraint for one synthetic route.
// A7.2: cold resolved-source record.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedFix {
    pub ident: String,
    pub lat_deg: f64,
    pub lon_deg: f64,
    /// The selected altitude window from the CIFP leg, in feet MSL; no bound means unrestricted.
    pub min_altitude_ft: Option<i32>,
    pub max_altitude_ft: Option<i32>,
    /// Preserved CIFP vertical guidance metadata. It is not a crossing bound and the
    /// synthetic fix-to-fix altitude model does not claim to follow this path.
    pub vertical_guidance: Option<VerticalGuidance>,
    /// Procedure-leg speed restriction, knots, when present.
    pub max_speed_kt: Option<u16>,
}

/// Supported guidance metadata retained from a CIFP procedure leg.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VerticalGuidance {
    /// ARINC 424-18 J: Altitude 2 is the glide-slope intercept value; Altitude 1 remains
    /// the at-or-above procedural crossing constraint. The current scenario model does not
    /// synthesize the glide-slope path.
    GlideSlopeIntercept {
        altitude2_ft: i32,
        angle_deg: Option<f64>,
    },
    /// ARINC 424-18 H: Altitude 2 is the glide-slope altitude at this fix; Altitude 1 is
    /// the at-or-above crossing constraint. The current synthetic model does not follow it.
    GlideSlopeAtFix {
        altitude2_ft: i32,
        angle_deg: Option<f64>,
    },
}

/// One ordered CIFP route geometry. The preparation stage must resolve every fix from the
/// verified public archive and preserve the same order as the `RouteSource.fix_sequence`.
// A7.2: cold bounded route setup.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedRoute {
    pub reference: Vec<ProcedureRouteRef>,
    pub fixes: Vec<ResolvedFix>,
}

/// Scenario-owned fixed-width synthetic sample. Time is signed virtual milliseconds from T0;
/// negative times represent route entry before the scenario window. It carries no address,
/// callsign, ADS-B quality claim, receiver metadata, or real-world identity.
/// A hot 32-byte record (A7.2 size guard satisfied): fixed-width fields and zero reserved flags.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyntheticReport {
    pub virtual_time_ms: i64,
    pub aircraft_index: u16,
    /// Bit 0 marks a new route leg. Every other bit is reserved and must remain zero.
    /// Bit0 starts a route; bit1 explicitly completes this arrival at the final endpoint.
    pub flags: u16,
    pub lat_e7: i32,
    pub lon_e7: i32,
    pub altitude_ft: i32,
    pub ground_speed_dkt: u16,
    pub track_ddeg: u16,
    pub vertical_rate_fpm: i32,
}

const _: () = assert!(::core::mem::size_of::<SyntheticReport>() == 32);

/// Non-operational timing/kinematic assumptions for the generated traffic.
// A7.2: cold per-materialization configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MotionAssumptions {
    /// Chosen constant groundspeed used only to time and position samples. It is an explicit
    /// synthetic scheduling assumption, not derived from IAS, TAS, atmosphere, or wind.
    pub nominal_ground_speed_kt: u16,
    /// Synthetic indicated-airspeed proxy checked against CIFP speed restrictions and the
    /// profile VMO. This value does not determine groundspeed; no atmosphere conversion exists.
    pub nominal_ias_proxy_kt: u16,
    /// Chosen initial altitude when the first fix has no altitude constraint.
    pub initial_altitude_ft: i32,
    /// Synthetic vertical-rate ceiling used to reject a route that cannot meet its fix windows.
    pub max_vertical_rate_fpm: u16,
    /// Report cadence. The simulator does not model the real ADS-B transmission schedule.
    pub sample_interval_ms: u32,
}

/// Why synthetic traffic could not be materialized.
// A7.2: cold error value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrafficError {
    RouteCount,
    RouteReference(usize),
    RouteFixCount(usize),
    RouteFixLimit(usize),
    RouteFixOrder(usize),
    InvalidLatLon(usize, usize),
    DegenerateSegment(usize, usize),
    BadAltitudeWindow(usize, usize),
    BadSpeedLimit(usize, usize),
    IasProxyExceedsProcedureLimit(usize, usize, u16, u16),
    Assumption(&'static str),
    Geodesic,
    VerticalRate(usize, usize),
    TimeRange,
    OutputLimit,
}

impl ::core::fmt::Display for TrafficError {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        match self {
            TrafficError::RouteCount => {
                f.write_str("resolved route count differs from scenario routes")
            }
            TrafficError::RouteReference(n) => write!(f, "route {n} reference mismatch"),
            TrafficError::RouteFixCount(n) => write!(f, "route {n} needs at least two fixes"),
            TrafficError::RouteFixLimit(n) => write!(
                f,
                "route {n} exceeds the {MAX_RESOLVED_ROUTE_FIXES}-fix materialization cap"
            ),
            TrafficError::RouteFixOrder(n) => write!(f, "route {n} fix sequence mismatch"),
            TrafficError::InvalidLatLon(r, i) => {
                write!(f, "route {r} fix {i} has an invalid coordinate")
            }
            TrafficError::DegenerateSegment(r, i) => {
                write!(f, "route {r} segment {i} has a degenerate WGS-84 length")
            }
            TrafficError::BadAltitudeWindow(r, i) => {
                write!(f, "route {r} fix {i} has an invalid altitude window")
            }
            TrafficError::BadSpeedLimit(r, i) => {
                write!(f, "route {r} fix {i} has an invalid speed limit")
            }
            TrafficError::IasProxyExceedsProcedureLimit(r, i, proxy, limit) => write!(
                f,
                "route {r} fix {i} CIFP speed limit {limit} kt is below synthetic IAS proxy {proxy} kt"
            ),
            TrafficError::Assumption(name) => {
                write!(f, "invalid synthetic motion assumption: {name}")
            }
            TrafficError::Geodesic => f.write_str("WGS-84 geodesic calculation failed"),
            TrafficError::VerticalRate(r, i) => write!(
                f,
                "route {r} leg {i} exceeds the selected synthetic vertical-rate limit"
            ),
            TrafficError::TimeRange => f.write_str(
                "synthetic report time is outside the scenario's bounded virtual-time range",
            ),
            TrafficError::OutputLimit => {
                f.write_str("synthetic report output exceeds the bounded scenario limit")
            }
        }
    }
}

/// Materializes a reproducible scenario-owned traffic feed from resolved public CIFP routes.
/// A private offline adapter may translate these samples to ADS-B-shaped rows for existing
/// truth/sensor tooling. The browser runtime does not depend on the ADS-B parser or wire format.
pub fn materialize_traffic(
    scenario: &SyntheticScenario,
    routes: &[ResolvedRoute],
    assumptions: MotionAssumptions,
) -> Result<Vec<SyntheticReport>, TrafficError> {
    materialize_with_preferences(scenario, routes, assumptions, None)
}

/// Materializes explicit scenario altitude clearances while preserving every published crossing
/// window and vertical-rate bound. Preferences are synthetic clearances, never CIFP facts.
pub fn materialize_traffic_with_altitude_preferences(
    scenario: &SyntheticScenario,
    routes: &[ResolvedRoute],
    assumptions: MotionAssumptions,
    preferred_ft: &[i32],
) -> Result<Vec<SyntheticReport>, TrafficError> {
    if preferred_ft.len() != routes.len() {
        return Err(TrafficError::RouteCount);
    }
    materialize_with_preferences(scenario, routes, assumptions, Some(preferred_ft))
}

fn materialize_with_preferences(
    scenario: &SyntheticScenario,
    routes: &[ResolvedRoute],
    assumptions: MotionAssumptions,
    preferred_ft: Option<&[i32]>,
) -> Result<Vec<SyntheticReport>, TrafficError> {
    if routes.len() != scenario.routes.len() {
        return Err(TrafficError::RouteCount);
    }
    validate_motion_assumptions(assumptions)?;
    if scenario.duration_ms == 0 || scenario.duration_ms > MAX_DURATION_MS {
        return Err(TrafficError::Assumption("scenario duration"));
    }

    let mut out = Vec::new();
    out.try_reserve_exact(MAX_SYNTHETIC_REPORTS)
        .map_err(|_| TrafficError::OutputLimit)?;
    let mut rows_written = 0usize;
    for (route_ix, route) in routes.iter().enumerate() {
        let Some(expected) = scenario.routes.get(route_ix) else {
            return Err(TrafficError::RouteReference(route_ix));
        };
        if route.reference != expected.source.route {
            return Err(TrafficError::RouteReference(route_ix));
        }
        if route.fixes.len() < 2 {
            return Err(TrafficError::RouteFixCount(route_ix));
        }
        if route.fixes.len() != expected.source.fix_sequence.len()
            || route
                .fixes
                .iter()
                .zip(&expected.source.fix_sequence)
                .any(|(fix, source_ident)| fix.ident != *source_ident)
        {
            return Err(TrafficError::RouteFixOrder(route_ix));
        }
    }

    // Precompute per-route segment lengths, selected speeds and altitude windows. This makes
    // every flight using the same route bit-for-bit identical apart from its time/address.
    let mut route_tables = Vec::with_capacity(routes.len());
    for (route_ix, route) in routes.iter().enumerate() {
        let Some(source) = scenario.routes.get(route_ix) else {
            return Err(TrafficError::RouteReference(route_ix));
        };
        if assumptions.nominal_ias_proxy_kt > source.performance.maximum_operating_ias_kcas {
            return Err(TrafficError::Assumption(
                "synthetic IAS proxy exceeds selected airframe VMO",
            ));
        }
        route_tables.push(build_route_table_with_preference(
            route_ix,
            route,
            assumptions,
            source.performance.maximum_operating_ias_kcas,
            preferred_ft.and_then(|p| p.get(route_ix)).copied(),
        )?);
    }

    for flight in &scenario.flights {
        let route_ix = usize::from(flight.route_variant);
        let Some(route) = routes.get(route_ix) else {
            return Err(TrafficError::RouteReference(route_ix));
        };
        let Some(table) = route_tables.get(route_ix) else {
            return Err(TrafficError::RouteReference(route_ix));
        };
        let route_duration_ms = table.total_duration_ms;
        let arrival_ms =
            i64::try_from(flight.arrival_time_ms).map_err(|_| TrafficError::TimeRange)?;
        let start_ms = arrival_ms
            .checked_sub(i64::try_from(route_duration_ms).map_err(|_| TrafficError::TimeRange)?)
            .ok_or(TrafficError::TimeRange)?;
        if start_ms < -MAX_PRE_SCENARIO_MS
            || arrival_ms
                > i64::try_from(scenario.duration_ms).map_err(|_| TrafficError::TimeRange)?
        {
            return Err(TrafficError::TimeRange);
        }
        let serial = synthetic_serial(&flight.aircraft_id)
            .ok_or(TrafficError::Assumption("synthetic aircraft id"))?;
        emit_route(
            &mut out,
            &mut rows_written,
            serial,
            start_ms,
            route_ix,
            route,
            table,
            assumptions,
        )?;
    }
    Ok(out)
}

/// Starts each source-timed arrival at its first sample inside the named public-airport area.
/// The flight clock and all subsequent route samples remain unchanged, including T0 pre-roll.
pub fn crop_arrival_area(
    reports: &mut Vec<SyntheticReport>,
    lat_deg: f64,
    lon_deg: f64,
    radius_nm: u16,
) -> Result<(), TrafficError> {
    let center = LatLon::new(lat_deg, lon_deg).map_err(|_| TrafficError::Geodesic)?;
    let mut starts = [i64::MAX; 60];
    for row in reports.iter() {
        let point = LatLon::new(f64::from(row.lat_e7) * 1e-7, f64::from(row.lon_e7) * 1e-7)
            .map_err(|_| TrafficError::Geodesic)?;
        let distance = wgs84()
            .inverse(center, point)
            .map_err(|_| TrafficError::Geodesic)?
            .distance_m;
        if distance <= f64::from(radius_nm) * 1852.0 {
            let start = starts
                .get_mut(usize::from(row.aircraft_index))
                .ok_or(TrafficError::OutputLimit)?;
            *start = (*start).min(row.virtual_time_ms);
        }
    }
    reports.retain(|row| {
        starts
            .get(usize::from(row.aircraft_index))
            .is_some_and(|&start| row.virtual_time_ms >= start)
    });
    Ok(())
}

/// Validates the same bounded per-route motion table used by materialization, without generating
/// any reports or allocating the report-output buffer. The fixed route-fix cap bounds temporary
/// planning storage to under 400 KiB; callers use this before selecting a route for later output.
pub fn validate_route_motion(
    route: &ResolvedRoute,
    assumptions: MotionAssumptions,
    maximum_operating_ias_kcas: u16,
) -> Result<(), TrafficError> {
    validate_motion_assumptions(assumptions)?;
    if assumptions.nominal_ias_proxy_kt > maximum_operating_ias_kcas {
        return Err(TrafficError::Assumption(
            "synthetic IAS proxy exceeds selected airframe VMO",
        ));
    }
    build_route_table(0, route, assumptions, maximum_operating_ias_kcas).map(|_| ())
}

fn validate_motion_assumptions(assumptions: MotionAssumptions) -> Result<(), TrafficError> {
    if assumptions.nominal_ground_speed_kt == 0
        || assumptions.nominal_ground_speed_kt > 400
        || assumptions.nominal_ias_proxy_kt == 0
        || assumptions.nominal_ias_proxy_kt > 250
        || assumptions.max_vertical_rate_fpm == 0
        || assumptions.sample_interval_ms == 0
        || assumptions.sample_interval_ms > 60_000
    {
        return Err(TrafficError::Assumption(
            "speed, vertical rate or sample interval",
        ));
    }
    Ok(())
}

// A7.2: cold, bounded route-planning records; not part of per-report hot state.
#[derive(Debug, PartialEq)]
struct Segment {
    distance_m: f64,
    initial_bearing_deg: f64,
    duration_ms: u64,
    /// Unrounded duration used by the rate validator and backward feasibility calculation.
    duration_ms_unrounded: f64,
    /// Conservative integer-foot change bound computed once from the segment duration.
    max_altitude_delta_ft: i64,
    speed_kt: u16,
    from_altitude_ft: i32,
    to_altitude_ft: i32,
}

// A7.2: fixed-width source-window interval for one resolved fix.
#[derive(Clone, Copy)]
struct AltitudeWindow {
    low_ft: i64,
    high_ft: i64,
}

// A7.2: cold, bounded route table reused across every synthetic flight on this route.
#[derive(Debug, PartialEq)]
struct RouteTable {
    segments: Vec<Segment>,
    total_duration_ms: u64,
}

fn build_route_table(
    route_ix: usize,
    route: &ResolvedRoute,
    assumptions: MotionAssumptions,
    maximum_operating_ias_kcas: u16,
) -> Result<RouteTable, TrafficError> {
    build_route_table_with_preference(
        route_ix,
        route,
        assumptions,
        maximum_operating_ias_kcas,
        None,
    )
}

fn build_route_table_with_preference(
    route_ix: usize,
    route: &ResolvedRoute,
    assumptions: MotionAssumptions,
    maximum_operating_ias_kcas: u16,
    preference_ft: Option<i32>,
) -> Result<RouteTable, TrafficError> {
    if route.fixes.len() < 2 {
        return Err(TrafficError::RouteFixCount(route_ix));
    }
    if route.fixes.len() > MAX_RESOLVED_ROUTE_FIXES {
        return Err(TrafficError::RouteFixLimit(route_ix));
    }
    if assumptions.nominal_ground_speed_kt == 0 || assumptions.max_vertical_rate_fpm == 0 {
        return Err(TrafficError::Assumption("speed or vertical rate"));
    }
    let mut windows = Vec::new();
    windows
        .try_reserve_exact(route.fixes.len())
        .map_err(|_| TrafficError::OutputLimit)?;
    for (fix_ix, fix) in route.fixes.iter().enumerate() {
        if !fix.lat_deg.is_finite()
            || !fix.lon_deg.is_finite()
            || !(-90.0..=90.0).contains(&fix.lat_deg)
            || !(-180.0..=180.0).contains(&fix.lon_deg)
        {
            return Err(TrafficError::InvalidLatLon(route_ix, fix_ix));
        }
        if fix
            .min_altitude_ft
            .zip(fix.max_altitude_ft)
            .is_some_and(|(lo, hi)| lo > hi)
        {
            return Err(TrafficError::BadAltitudeWindow(route_ix, fix_ix));
        }
        if fix.max_speed_kt == Some(0) {
            return Err(TrafficError::BadSpeedLimit(route_ix, fix_ix));
        }
        if let Some(limit) = fix.max_speed_kt {
            if assumptions.nominal_ias_proxy_kt > limit {
                return Err(TrafficError::IasProxyExceedsProcedureLimit(
                    route_ix,
                    fix_ix,
                    assumptions.nominal_ias_proxy_kt,
                    limit,
                ));
            }
        }
        windows.push(AltitudeWindow {
            low_ft: fix.min_altitude_ft.map_or(i64::from(i32::MIN), i64::from),
            high_ft: fix.max_altitude_ft.map_or(i64::from(i32::MAX), i64::from),
        });
    }

    let mut segments = Vec::new();
    segments
        .try_reserve_exact(route.fixes.len().saturating_sub(1))
        .map_err(|_| TrafficError::OutputLimit)?;
    let mut total_duration_ms = 0u64;
    for i in 0..route.fixes.len().saturating_sub(1) {
        let Some(from) = route.fixes.get(i) else {
            return Err(TrafficError::RouteFixCount(route_ix));
        };
        let Some(to) = route.fixes.get(i + 1) else {
            return Err(TrafficError::RouteFixCount(route_ix));
        };
        let p1 = LatLon::new(from.lat_deg, from.lon_deg)
            .map_err(|_| TrafficError::InvalidLatLon(route_ix, i))?;
        let p2 = LatLon::new(to.lat_deg, to.lon_deg)
            .map_err(|_| TrafficError::InvalidLatLon(route_ix, i + 1))?;
        let inverse = wgs84()
            .inverse(p1, p2)
            .map_err(|_| TrafficError::Geodesic)?;
        if !inverse.distance_m.is_finite() {
            return Err(TrafficError::Geodesic);
        }
        if inverse.distance_m <= 1.0 {
            return Err(TrafficError::DegenerateSegment(route_ix, i));
        }
        if assumptions.nominal_ias_proxy_kt > maximum_operating_ias_kcas {
            return Err(TrafficError::Assumption(
                "synthetic IAS proxy exceeds airframe limit",
            ));
        }
        let speed_kt = assumptions.nominal_ground_speed_kt;
        let duration_f = inverse.distance_m / (f64::from(speed_kt) * 0.514_444_444_444) * 1_000.0;
        if !duration_f.is_finite() || duration_f < 1.0 || duration_f > i64::MAX as f64 {
            return Err(TrafficError::TimeRange);
        }
        let duration_ceil_ms = oa_ieee::ceil(duration_f);
        if !duration_ceil_ms.is_finite() || duration_ceil_ms > i64::MAX as f64 {
            return Err(TrafficError::TimeRange);
        }
        let duration_ms = duration_ceil_ms as u64;
        let altitude_delta_cap_ft =
            max_altitude_delta_ft(duration_ms, assumptions.max_vertical_rate_fpm)?;
        total_duration_ms = total_duration_ms
            .checked_add(duration_ms)
            .ok_or(TrafficError::TimeRange)?;
        segments.push(Segment {
            distance_m: inverse.distance_m,
            initial_bearing_deg: inverse.azimuth1_deg,
            duration_ms,
            duration_ms_unrounded: duration_f,
            max_altitude_delta_ft: altitude_delta_cap_ft,
            speed_kt,
            from_altitude_ft: 0,
            to_altitude_ft: 0,
        });
    }

    let altitudes = plan_altitudes_with_preference(
        route_ix,
        &windows,
        &segments,
        assumptions.initial_altitude_ft,
        preference_ft,
    )?;
    for (index, (segment, pair)) in segments.iter_mut().zip(altitudes.windows(2)).enumerate() {
        let Some((&from_altitude_ft, &to_altitude_ft)) = pair.first().zip(pair.get(1)) else {
            return Err(TrafficError::RouteFixCount(route_ix));
        };
        segment.from_altitude_ft = from_altitude_ft;
        segment.to_altitude_ft = to_altitude_ft;
        let minutes = segment.duration_ms_unrounded / 60_000.0;
        let altitude_delta = i64::from(to_altitude_ft) - i64::from(from_altitude_ft);
        let rate = (altitude_delta as f64).abs() / minutes;
        if !rate.is_finite() || rate > f64::from(assumptions.max_vertical_rate_fpm) {
            return Err(TrafficError::VerticalRate(route_ix, index));
        }
    }
    Ok(RouteTable {
        segments,
        total_duration_ms,
    })
}

/// The public elapsed time is the ceiling of the geodesic duration. Using one millisecond less
/// than that ceiling is a conservative lower bound on the unrounded duration, including when
/// the duration was already integral. Integer `u128` arithmetic makes the feet bound exact;
/// `u32::MAX` is the largest possible difference between two representable `i32` altitudes.
fn max_altitude_delta_ft(
    duration_ceil_ms: u64,
    max_vertical_rate_fpm: u16,
) -> Result<i64, TrafficError> {
    let conservative_ms = duration_ceil_ms
        .checked_sub(1)
        .ok_or(TrafficError::TimeRange)?;
    let numerator = u128::from(max_vertical_rate_fpm)
        .checked_mul(u128::from(conservative_ms))
        .ok_or(TrafficError::TimeRange)?;
    let whole_feet = numerator / 60_000;
    let maximum_representable_delta = u128::from(u32::MAX);
    i64::try_from(whole_feet.min(maximum_representable_delta)).map_err(|_| TrafficError::TimeRange)
}

/// Selects integer foot altitudes inside every source window while respecting each segment's
/// rate envelope. Segment timestamps use `ceil(unrounded_duration)` milliseconds. Backward
/// intervals use `floor(rate × (ceil_ms - 1) / 60_000)` integer feet, a conservative lower bound
/// on unrounded duration; the final validator uses the unrounded duration itself. Forward
/// selection keeps the preferred
/// initial altitude, then retains the previous altitude whenever it remains feasible; otherwise
/// it descends/ascends only as far as needed to stay within the suffix-feasible interval.
#[cfg(test)]
fn plan_altitudes(
    route_ix: usize,
    windows: &[AltitudeWindow],
    segments: &[Segment],
    preferred_initial_ft: i32,
) -> Result<Vec<i32>, TrafficError> {
    plan_altitudes_with_preference(route_ix, windows, segments, preferred_initial_ft, None)
}

fn plan_altitudes_with_preference(
    route_ix: usize,
    windows: &[AltitudeWindow],
    segments: &[Segment],
    preferred_initial_ft: i32,
    preference_ft: Option<i32>,
) -> Result<Vec<i32>, TrafficError> {
    if windows.len() > MAX_RESOLVED_ROUTE_FIXES {
        return Err(TrafficError::RouteFixLimit(route_ix));
    }
    if windows.len() < 2 || segments.len().checked_add(1) != Some(windows.len()) {
        return Err(TrafficError::RouteFixCount(route_ix));
    }
    let mut feasible = Vec::new();
    feasible
        .try_reserve_exact(windows.len())
        .map_err(|_| TrafficError::OutputLimit)?;
    feasible.extend_from_slice(windows);
    for index in (0..segments.len()).rev() {
        let segment = segments.get(index).ok_or(TrafficError::TimeRange)?;
        if !segment.duration_ms_unrounded.is_finite() || segment.duration_ms_unrounded < 1.0 {
            return Err(TrafficError::TimeRange);
        }
        let capacity_ft = segment.max_altitude_delta_ft;
        let next = *feasible
            .get(index + 1)
            .ok_or(TrafficError::RouteFixCount(route_ix))?;
        let reachable_low = next.low_ft.checked_sub(capacity_ft).unwrap_or(i64::MIN);
        let reachable_high = next.high_ft.checked_add(capacity_ft).unwrap_or(i64::MAX);
        let here = feasible
            .get_mut(index)
            .ok_or(TrafficError::RouteFixCount(route_ix))?;
        here.low_ft = here.low_ft.max(reachable_low).max(i64::from(i32::MIN));
        if preference_ft.is_some() {
            // Arrival clearances cannot descend below a later published floor and climb back.
            here.low_ft = here.low_ft.max(next.low_ft);
        }
        here.high_ft = here.high_ft.min(reachable_high).min(i64::from(i32::MAX));
        if here.low_ft > here.high_ft {
            return Err(TrafficError::VerticalRate(route_ix, index));
        }
    }

    let first = *feasible
        .first()
        .ok_or(TrafficError::RouteFixCount(route_ix))?;
    let initial = i64::from(preferred_initial_ft).clamp(first.low_ft, first.high_ft);
    let mut altitudes = Vec::new();
    altitudes
        .try_reserve_exact(windows.len())
        .map_err(|_| TrafficError::OutputLimit)?;
    altitudes.push(i32::try_from(initial).map_err(|_| TrafficError::TimeRange)?);
    // Explicit synthetic descent preference:350ft/NM (about3.3degrees), constrained by
    // every published window and the1800ft/min rate envelope. Distance-to-go avoids holding
    // equal levels on folded STAR/vector paths while a following flight flies toward them.
    let final_floor = windows.last().map_or(0, |w| w.low_ft.max(0));
    let mut remaining_m: f64 = segments.iter().map(|segment| segment.distance_m).sum();
    for (index, segment) in segments.iter().enumerate() {
        if !segment.duration_ms_unrounded.is_finite() || segment.duration_ms_unrounded < 1.0 {
            return Err(TrafficError::TimeRange);
        }
        let capacity_ft = segment.max_altitude_delta_ft;
        let prior = i64::from(
            *altitudes
                .last()
                .ok_or(TrafficError::RouteFixCount(route_ix))?,
        );
        let reachable_low = prior.checked_sub(capacity_ft).unwrap_or(i64::MIN);
        let reachable_high = prior.checked_add(capacity_ft).unwrap_or(i64::MAX);
        let suffix = *feasible
            .get(index + 1)
            .ok_or(TrafficError::RouteFixCount(route_ix))?;
        let low = reachable_low.max(suffix.low_ft);
        let high = if preference_ft.is_some() {
            reachable_high.min(suffix.high_ft).min(prior)
        } else {
            reachable_high.min(suffix.high_ft)
        };
        if low > high {
            return Err(TrafficError::VerticalRate(route_ix, index));
        }
        remaining_m = oa_math::max_f64(remaining_m - segment.distance_m, 0.0);
        let preferred = preference_ft.map_or(prior, |ceiling| {
            let target = oa_math::min_f64(
                final_floor as f64 + remaining_m * 350.0 / 1852.0,
                f64::from(ceiling),
            );
            oa_ieee::round(target) as i64
        });
        altitudes.push(
            i32::try_from(i64::clamp(preferred, low, high)).map_err(|_| TrafficError::TimeRange)?,
        );
    }
    Ok(altitudes)
}

// Each argument is a prevalidated bounded route/source buffer; keeping these concrete inputs
// explicit avoids rebuilding an intermediate aggregate in the hot sample loop.
#[allow(clippy::too_many_arguments)]
fn emit_route(
    reports: &mut Vec<SyntheticReport>,
    rows_written: &mut usize,
    serial: u16,
    start_ms: i64,
    route_ix: usize,
    route: &ResolvedRoute,
    table: &RouteTable,
    assumptions: MotionAssumptions,
) -> Result<(), TrafficError> {
    let mut elapsed_ms = 0u64;
    for (segment_ix, segment) in table.segments.iter().enumerate() {
        let steps = segment
            .duration_ms
            .div_ceil(u64::from(assumptions.sample_interval_ms))
            .max(1);
        for step in 0..=steps {
            if segment_ix > 0 && step == 0 {
                continue;
            }
            if rows_written.saturating_add(1) > MAX_SYNTHETIC_REPORTS {
                return Err(TrafficError::OutputLimit);
            }
            let fraction = step as f64 / steps as f64;
            let distance = segment.distance_m * fraction;
            let Some(fix) = route.fixes.get(segment_ix) else {
                return Err(TrafficError::RouteFixCount(route_ix));
            };
            let origin = LatLon::new(fix.lat_deg, fix.lon_deg)
                .map_err(|_| TrafficError::InvalidLatLon(route_ix, segment_ix))?;
            let point = wgs84()
                .direct(origin, segment.initial_bearing_deg, distance)
                .map_err(|_| TrafficError::Geodesic)?
                .position;
            let time_offset_ms = elapsed_ms
                .saturating_add(oa_ieee::round(segment.duration_ms as f64 * fraction) as u64);
            let virtual_time_ms = start_ms
                .checked_add(i64::try_from(time_offset_ms).map_err(|_| TrafficError::TimeRange)?)
                .ok_or(TrafficError::TimeRange)?;
            let alt_delta = i64::from(segment.to_altitude_ft) - i64::from(segment.from_altitude_ft);
            let alt_f = f64::from(segment.from_altitude_ft) + (alt_delta as f64) * fraction;
            let altitude_ft = oa_ieee::round(alt_f) as i32;
            let track = wrap360(segment.initial_bearing_deg);
            let vertical_rate = if segment.duration_ms == 0 {
                0
            } else {
                oa_ieee::round(
                    (i64::from(segment.to_altitude_ft) - i64::from(segment.from_altitude_ft))
                        as f64
                        * 60_000.0
                        / segment.duration_ms as f64,
                ) as i32
            };
            let report = SyntheticReport {
                virtual_time_ms,
                aircraft_index: serial,
                flags: if segment_ix == 0 && step == 0 { 1 } else { 0 },
                lat_e7: oa_ieee::round(point.lat() * 1.0e7) as i32,
                lon_e7: oa_ieee::round(point.lon() * 1.0e7) as i32,
                altitude_ft,
                ground_speed_dkt: segment.speed_kt.saturating_mul(10),
                track_ddeg: (oa_ieee::round(track * 10.0) as u16) % 3600,
                vertical_rate_fpm: vertical_rate,
            };
            reports.push(report);
            *rows_written = rows_written.saturating_add(1);
        }
        elapsed_ms = elapsed_ms
            .checked_add(segment.duration_ms)
            .ok_or(TrafficError::TimeRange)?;
    }
    // The normal arrival leaves this airborne scenario at the published final's endpoint.
    // This explicit data event is distinct from a surveillance dropout or a missing update.
    if let Some(last) = reports.last_mut() {
        last.flags |= 2;
    }
    let _ = route;
    Ok(())
}

fn synthetic_serial(id: &str) -> Option<u16> {
    let digits = id.strip_prefix("SYN-")?;
    let n = digits.parse::<u16>().ok()?;
    if n >= 60 || format!("SYN-{n:04}") != id {
        return None;
    }
    Some(n)
}

fn wrap360(degrees: f64) -> f64 {
    let normalized = degrees % 360.0;
    if normalized < 0.0 {
        normalized + 360.0
    } else {
        normalized
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

    use super::*;

    fn route(limit: Option<u16>) -> ResolvedRoute {
        ResolvedRoute {
            reference: Vec::new(),
            fixes: vec![
                ResolvedFix {
                    ident: "SYN-A".into(),
                    lat_deg: 41.0,
                    lon_deg: -88.0,
                    min_altitude_ft: None,
                    max_altitude_ft: None,
                    vertical_guidance: None,
                    max_speed_kt: None,
                },
                ResolvedFix {
                    ident: "SYN-B".into(),
                    lat_deg: 41.1,
                    lon_deg: -88.0,
                    min_altitude_ft: None,
                    max_altitude_ft: None,
                    vertical_guidance: None,
                    max_speed_kt: limit,
                },
            ],
        }
    }

    fn fix(
        ident: &str,
        lat_deg: f64,
        min_altitude_ft: Option<i32>,
        max_altitude_ft: Option<i32>,
        max_speed_kt: Option<u16>,
    ) -> ResolvedFix {
        ResolvedFix {
            ident: ident.into(),
            lat_deg,
            lon_deg: -88.0,
            min_altitude_ft,
            max_altitude_ft,
            vertical_guidance: None,
            max_speed_kt,
        }
    }

    fn route_with_fixes(fixes: Vec<ResolvedFix>) -> ResolvedRoute {
        ResolvedRoute {
            reference: Vec::new(),
            fixes,
        }
    }

    fn assumptions() -> MotionAssumptions {
        MotionAssumptions {
            nominal_ground_speed_kt: 220,
            nominal_ias_proxy_kt: 220,
            initial_altitude_ft: 10_000,
            max_vertical_rate_fpm: 1_800,
            sample_interval_ms: 5_000,
        }
    }

    #[test]
    fn route_motion_validator_distinguishes_invalid_latlon_from_degenerate_segment() {
        let mut invalid = route(None);
        invalid.fixes[1].lat_deg = 91.0;
        assert_eq!(
            validate_route_motion(&invalid, assumptions(), 340),
            Err(TrafficError::InvalidLatLon(0, 1))
        );

        let mut degenerate = route(None);
        degenerate.fixes[1].lat_deg = degenerate.fixes[0].lat_deg;
        degenerate.fixes[1].lon_deg = degenerate.fixes[0].lon_deg;
        assert_eq!(
            validate_route_motion(&degenerate, assumptions(), 340),
            Err(TrafficError::DegenerateSegment(0, 0))
        );

        assert_eq!(
            validate_route_motion(&route(None), assumptions(), 340),
            Ok(())
        );
    }

    #[test]
    fn procedure_speed_below_fixed_ias_proxy_refuses_materialization() {
        assert!(matches!(
            build_route_table(0, &route(Some(210)), assumptions(), 340),
            Err(TrafficError::IasProxyExceedsProcedureLimit(0, 1, 220, 210))
        ));
        assert_eq!(
            TrafficError::IasProxyExceedsProcedureLimit(0, 1, 220, 210).to_string(),
            "route 0 fix 1 CIFP speed limit 210 kt is below synthetic IAS proxy 220 kt"
        );
        let mut first_limited = route(None);
        first_limited.fixes[0].max_speed_kt = Some(210);
        assert!(matches!(
            build_route_table(0, &first_limited, assumptions(), 340),
            Err(TrafficError::IasProxyExceedsProcedureLimit(0, 0, 220, 210))
        ));
    }

    #[test]
    fn compatible_limit_keeps_fixed_synthetic_groundspeed_unchanged() {
        let table = build_route_table(0, &route(Some(220)), assumptions(), 340)
            .expect("a limit equal to the disclosed IAS proxy is compatible");
        assert_eq!(table.segments.len(), 1);
        assert_eq!(table.segments[0].speed_kt, 220);
        assert!(table.total_duration_ms > 0);
    }

    #[test]
    fn zero_groundspeed_refuses_before_segment_timing() {
        let invalid = MotionAssumptions {
            nominal_ground_speed_kt: 0,
            ..assumptions()
        };
        assert_eq!(
            build_route_table(0, &route(None), invalid, 340),
            Err(TrafficError::Assumption("speed or vertical rate"))
        );
    }

    #[test]
    fn planner_descends_early_for_floor_only_approach_to_exact_runway() {
        let route = route_with_fixes(vec![
            fix("A", 41.0, None, None, None),
            fix("B", 41.25, Some(5_000), None, None),
            fix("C", 41.50, Some(2_200), None, None),
            fix("RWY", 41.75, Some(713), Some(713), None),
        ]);
        let selected = build_route_table(
            0,
            &route,
            MotionAssumptions {
                initial_altitude_ft: 18_000,
                ..assumptions()
            },
            340,
        )
        .expect("route has enough distance to meet each floor and runway altitude");
        let repeated = build_route_table(
            0,
            &route,
            MotionAssumptions {
                initial_altitude_ft: 18_000,
                ..assumptions()
            },
            340,
        )
        .expect("the planner is deterministic");
        let altitudes = |table: &RouteTable| {
            let first = table.segments.first().expect("route has segments");
            core::iter::once(first.from_altitude_ft)
                .chain(table.segments.iter().map(|segment| segment.to_altitude_ft))
                .collect::<Vec<_>>()
        };
        let chosen = altitudes(&selected);
        assert_eq!(selected, repeated);
        assert_eq!(chosen, altitudes(&repeated));
        assert_eq!(chosen.first(), Some(&18_000));
        assert!(chosen[1] >= 5_000);
        assert!(chosen[2] >= 2_200);
        assert_eq!(chosen.last(), Some(&713));
        for (index, segment) in selected.segments.iter().enumerate() {
            let minutes = segment.duration_ms_unrounded / 60_000.0;
            let delta = (i64::from(segment.to_altitude_ft) - i64::from(segment.from_altitude_ft))
                .unsigned_abs();
            assert!(delta as f64 / minutes <= 1_800.0, "segment {index}");
        }
    }

    #[test]
    fn planner_refuses_impossible_fixed_altitude_rate() {
        let route = route_with_fixes(vec![
            fix("A", 41.0, Some(10_000), Some(10_000), None),
            fix("RWY", 41.01, Some(713), Some(713), None),
        ]);
        assert!(matches!(
            build_route_table(0, &route, assumptions(), 340),
            Err(TrafficError::VerticalRate(0, 0))
        ));
    }

    #[test]
    fn planner_refuses_a_disjoint_source_altitude_window() {
        let route = route_with_fixes(vec![
            fix("A", 41.0, Some(5_000), Some(4_000), None),
            fix("B", 41.25, None, None, None),
        ]);
        assert_eq!(
            build_route_table(0, &route, assumptions(), 340),
            Err(TrafficError::BadAltitudeWindow(0, 0))
        );
    }

    #[test]
    fn planner_allows_a_legal_climb_between_fixed_altitudes() {
        let route = route_with_fixes(vec![
            fix("A", 41.0, Some(1_000), Some(1_000), None),
            fix("B", 41.25, Some(5_000), Some(5_000), None),
        ]);
        let table = build_route_table(0, &route, assumptions(), 340)
            .expect("segment has enough time for the bounded climb");
        assert_eq!(table.segments[0].from_altitude_ft, 1_000);
        assert_eq!(table.segments[0].to_altitude_ft, 5_000);
        assert!(table.segments[0].max_altitude_delta_ft >= 4_000);
    }

    #[test]
    fn planner_clamps_initial_preference_into_first_fixed_window() {
        let route = route_with_fixes(vec![
            fix("A", 41.0, Some(12_000), Some(13_000), None),
            fix("B", 41.25, None, None, None),
        ]);
        let table = build_route_table(
            0,
            &route,
            MotionAssumptions {
                initial_altitude_ft: 10_000,
                ..assumptions()
            },
            340,
        )
        .expect("initial preference can be clamped to the first window");
        assert_eq!(table.segments[0].from_altitude_ft, 12_000);
    }

    #[test]
    fn planner_refuses_zero_and_nonfinite_segment_times() {
        let windows = [
            AltitudeWindow {
                low_ft: 0,
                high_ft: 10_000,
            },
            AltitudeWindow {
                low_ft: 0,
                high_ft: 10_000,
            },
        ];
        for invalid_duration in [0.0, f64::NAN, f64::INFINITY] {
            let segments = [Segment {
                distance_m: 10.0,
                initial_bearing_deg: 0.0,
                duration_ms: 1,
                duration_ms_unrounded: invalid_duration,
                max_altitude_delta_ft: 0,
                speed_kt: 220,
                from_altitude_ft: 0,
                to_altitude_ft: 0,
            }];
            assert_eq!(
                plan_altitudes(0, &windows, &segments, 5_000),
                Err(TrafficError::TimeRange)
            );
        }
    }

    #[test]
    fn segment_time_capacity_is_exact_and_conservative_at_integer_boundaries() {
        assert_eq!(max_altitude_delta_ft(60_001, 1_800), Ok(1_800));
        assert_eq!(max_altitude_delta_ft(60_000, 1_800), Ok(1_799));
        assert_eq!(max_altitude_delta_ft(1, 1_800), Ok(0));
        assert_eq!(
            max_altitude_delta_ft(0, 1_800),
            Err(TrafficError::TimeRange)
        );
    }

    #[test]
    fn planner_refuses_routes_over_fixed_fix_cap_before_workspace_creation() {
        let fixes = (0..=MAX_RESOLVED_ROUTE_FIXES)
            .map(|index| {
                let index = u32::try_from(index).expect("test index stays within route cap");
                fix("FIX", 41.0 + f64::from(index) * 0.0001, None, None, None)
            })
            .collect();
        assert!(matches!(
            build_route_table(0, &route_with_fixes(fixes), assumptions(), 340),
            Err(TrafficError::RouteFixLimit(0))
        ));
    }
}
