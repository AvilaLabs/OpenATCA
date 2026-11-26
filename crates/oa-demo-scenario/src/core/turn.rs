//! Bounded synthetic fly-by geometry for one waypoint triple.
//!
//! This pure helper constructs a constant-radius planar fillet in an AEQD frame centered on B,
//! then unprojects its sampled arc. It is geometry only: callers must still validate procedure
//! constraints, neighboring fillet overlap, and route-leg semantics before materializing traffic.
//! It does not claim an aircraft-specific FMS or turn-performance model.

extern crate alloc;

use alloc::vec::Vec;
use oa_geo::aeqd::{frame_new, project, unproject};
use oa_geo::{wgs84, LatLon};
/// Geographic point in degrees.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GeoPoint {
    pub lat_deg: f64,
    pub lon_deg: f64,
}

/// A fly-by turn arc, including its planar audit geometry and unprojected samples.
#[derive(Debug, Clone, PartialEq)]
pub struct FlyByFillet {
    /// Signed course change in radians; positive is counterclockwise in east/north coordinates.
    pub signed_turn_rad: f64,
    /// Arc direction: +1 counterclockwise, -1 clockwise.
    pub direction: i8,
    pub radius_m: f64,
    /// Distance from B to each tangent point along its respective leg.
    pub tangent_distance_m: f64,
    /// Tangent point before B.
    pub entry: GeoPoint,
    /// Tangent point after B.
    pub exit: GeoPoint,
    /// Arc samples including entry and exit, ordered along the turn.
    pub arc: Vec<GeoPoint>,
}

/// Explicit source-leg interpretation needed by the synthetic route builder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaypointTurnMode {
    /// The route starts or ends here; no fillet is constructed.
    Endpoint,
    /// Caller has established this is a fly-by waypoint.
    FlyBy,
    /// This helper does not synthesize a fly-over turn.
    FlyOver,
    /// CIFP leg semantics were not mapped or are unsupported.
    Unsupported,
}

/// One source fix and the route semantics needed for safe synthetic geometry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RouteWaypoint {
    pub position: GeoPoint,
    pub turn_mode: WaypointTurnMode,
    /// True when an altitude or speed constraint must be evaluated at this exact fix.
    pub has_speed_or_altitude_constraint: bool,
}

/// Role of one point in the bounded route geometry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutePointKind {
    ExactFix,
    TurnArc,
}

/// A route geometry point, retaining its source-fix association.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RoutePathPoint {
    pub position: GeoPoint,
    pub source_fix_index: usize,
    pub kind: RoutePointKind,
}

/// Bounded geographic path built from exact source fixes and fly-by arcs.
#[derive(Debug, Clone, PartialEq)]
pub struct FlyByRoutePath {
    pub points: Vec<RoutePathPoint>,
    pub total_length_m: f64,
}

/// Fail-closed reasons for a turn that cannot be safely bounded by this helper.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlyByError {
    InvalidCoordinate,
    InvalidRadius,
    InvalidTurnRate,
    InvalidSampleCount,
    ProjectionFailure,
    DegenerateLeg,
    StraightCourse,
    CourseReversal,
    LegTooShort,
    RouteTooShort,
    EndpointMode,
    FlyOverUnsupported(usize),
    UnsupportedLeg(usize),
    ConstraintAtFlyBy(usize),
    AdjacentTurnsOverlap(usize),
    RoutePointLimit,
    RouteLengthLimit,
}

/// Convert an explicit synthetic groundspeed and turn-rate limit to a requested radius `v/ω`.
/// The result is bounded by the same 1 m..=25 km range accepted by [`build_fly_by_fillet`].
pub fn radius_for_turn_rate(speed_mps: f64, turn_rate_rad_s: f64) -> Result<f64, FlyByError> {
    if !speed_mps.is_finite()
        || speed_mps <= 0.0
        || !turn_rate_rad_s.is_finite()
        || turn_rate_rad_s <= 0.0
    {
        return Err(FlyByError::InvalidTurnRate);
    }
    let radius = speed_mps / turn_rate_rad_s;
    if !(1.0..=25_000.0).contains(&radius) {
        return Err(FlyByError::InvalidRadius);
    }
    Ok(radius)
}

/// Construct a fixed-radius tangent fillet for A→B→C.
///
/// `radius_m` is explicitly supplied by the caller (for example `ground_speed_mps / turn_rate_rad_s`).
/// The helper never shrinks it to fit. Both legs must be at most 100 km in the B-centered AEQD
/// frame; the arc uses 2..=512 intervals. The returned path runs tangent-in, around the arc, and
/// tangent-out. Neighboring turns and route constraints remain the caller's responsibility.
pub fn build_fly_by_fillet(
    a: GeoPoint,
    b: GeoPoint,
    c: GeoPoint,
    radius_m: f64,
    arc_intervals: usize,
) -> Result<FlyByFillet, FlyByError> {
    if !valid(a) || !valid(b) || !valid(c) {
        return Err(FlyByError::InvalidCoordinate);
    }
    if !radius_m.is_finite() || !(1.0..=25_000.0).contains(&radius_m) {
        return Err(FlyByError::InvalidRadius);
    }
    if !(2..=512).contains(&arc_intervals) {
        return Err(FlyByError::InvalidSampleCount);
    }

    let frame = frame_new(b.lat_deg, b.lon_deg);
    let (ax, ay) = project(&frame, a.lat_deg, a.lon_deg);
    let (cx, cy) = project(&frame, c.lat_deg, c.lon_deg);
    if ![ax, ay, cx, cy].into_iter().all(f64::is_finite) {
        return Err(FlyByError::ProjectionFailure);
    }
    let in_len = oa_math::hypot(ax, ay);
    let out_len = oa_math::hypot(cx, cy);
    if in_len <= 1.0 || out_len <= 1.0 || in_len > 100_000.0 || out_len > 100_000.0 {
        return Err(FlyByError::DegenerateLeg);
    }

    // Incoming course points from A to B; outgoing course points from B to C.
    let ix = -ax / in_len;
    let iy = -ay / in_len;
    let ox = cx / out_len;
    let oy = cy / out_len;
    let cross = ix * oy - iy * ox;
    let dot = oa_math::clamp_f64(ix * ox + iy * oy, -1.0, 1.0);
    let turn = oa_math::atan2(cross, dot);
    let abs_turn = turn.abs();
    if abs_turn < 1.0e-4 {
        return Err(FlyByError::StraightCourse);
    }
    if ::core::f64::consts::PI - abs_turn < 1.0e-3 {
        return Err(FlyByError::CourseReversal);
    }

    let tangent_distance = radius_m * oa_math::tan(abs_turn * 0.5);
    if !tangent_distance.is_finite() || tangent_distance >= in_len || tangent_distance >= out_len {
        return Err(FlyByError::LegTooShort);
    }
    let direction = if turn > 0.0 { 1_i8 } else { -1_i8 };
    let entry_xy = (-ix * tangent_distance, -iy * tangent_distance);
    let exit_xy = (ox * tangent_distance, oy * tangent_distance);
    let normal = (-iy * f64::from(direction), ix * f64::from(direction));
    let center = (
        entry_xy.0 + normal.0 * radius_m,
        entry_xy.1 + normal.1 * radius_m,
    );
    let entry_angle = oa_math::atan2(entry_xy.1 - center.1, entry_xy.0 - center.0);
    let sweep = turn;
    let unproject_xy = |x: f64, y: f64| -> Result<GeoPoint, FlyByError> {
        let (lat_deg, lon_deg) = unproject(&frame, x, y).ok_or(FlyByError::ProjectionFailure)?;
        Ok(GeoPoint { lat_deg, lon_deg })
    };
    let entry = unproject_xy(entry_xy.0, entry_xy.1)?;
    let exit = unproject_xy(exit_xy.0, exit_xy.1)?;
    let mut arc = Vec::with_capacity(arc_intervals + 1);
    for i in 0..=arc_intervals {
        let fraction = i as f64 / arc_intervals as f64;
        let theta = entry_angle + sweep * fraction;
        let (sine, cosine) = oa_math::sin_cos(theta);
        let x = center.0 + radius_m * cosine;
        let y = center.1 + radius_m * sine;
        arc.push(unproject_xy(x, y)?);
    }
    Ok(FlyByFillet {
        signed_turn_rad: turn,
        direction,
        radius_m,
        tangent_distance_m: tangent_distance,
        entry,
        exit,
        arc,
    })
}

/// Build a route using only explicitly classified fly-by fixes.
///
/// Fly-over and unmapped CIFP semantics fail closed. A constrained fix cannot be replaced by a
/// tangent arc because the current helper cannot prove the CIFP constraint at that offset point.
/// Adjacent turns may not consume the same source leg, and output is capped at 65,536 points and
/// 2,000 km. `arc_intervals` is bounded by [`build_fly_by_fillet`].
pub fn build_fly_by_route(
    waypoints: &[RouteWaypoint],
    radius_m: f64,
    arc_intervals: usize,
) -> Result<FlyByRoutePath, FlyByError> {
    const MAX_FIXES: usize = 256;
    const MAX_POINTS: usize = 65_536;
    const MAX_ROUTE_LENGTH_M: f64 = 2_000_000.0;
    if waypoints.len() < 2 {
        return Err(FlyByError::RouteTooShort);
    }
    if waypoints.len() > MAX_FIXES {
        return Err(FlyByError::RoutePointLimit);
    }
    if waypoints
        .first()
        .is_some_and(|w| w.turn_mode != WaypointTurnMode::Endpoint)
        || waypoints
            .last()
            .is_some_and(|w| w.turn_mode != WaypointTurnMode::Endpoint)
    {
        return Err(FlyByError::EndpointMode);
    }
    for (i, waypoint) in waypoints.iter().enumerate() {
        if !valid(waypoint.position) {
            return Err(FlyByError::InvalidCoordinate);
        }
        if i > 0 && i + 1 < waypoints.len() {
            match waypoint.turn_mode {
                WaypointTurnMode::FlyBy => {
                    if waypoint.has_speed_or_altitude_constraint {
                        return Err(FlyByError::ConstraintAtFlyBy(i));
                    }
                }
                WaypointTurnMode::FlyOver => return Err(FlyByError::FlyOverUnsupported(i)),
                WaypointTurnMode::Unsupported | WaypointTurnMode::Endpoint => {
                    return Err(FlyByError::UnsupportedLeg(i));
                }
            }
        }
    }

    let mut fillets: Vec<Option<FlyByFillet>> = Vec::with_capacity(waypoints.len());
    fillets.resize_with(waypoints.len(), || None);
    for (i, fillet_slot) in fillets
        .iter_mut()
        .enumerate()
        .take(waypoints.len().saturating_sub(1))
        .skip(1)
    {
        let a = waypoints
            .get(i - 1)
            .ok_or(FlyByError::RouteTooShort)?
            .position;
        let b = waypoints.get(i).ok_or(FlyByError::RouteTooShort)?.position;
        let c = waypoints
            .get(i + 1)
            .ok_or(FlyByError::RouteTooShort)?
            .position;
        match build_fly_by_fillet(a, b, c, radius_m, arc_intervals) {
            Ok(fillet) => {
                *fillet_slot = Some(fillet);
            }
            Err(FlyByError::StraightCourse) => {}
            Err(error) => return Err(error),
        }
    }

    for segment_ix in 0..waypoints.len().saturating_sub(1) {
        let a = waypoints
            .get(segment_ix)
            .ok_or(FlyByError::RouteTooShort)?
            .position;
        let b = waypoints
            .get(segment_ix + 1)
            .ok_or(FlyByError::RouteTooShort)?
            .position;
        let pa = LatLon::new(a.lat_deg, a.lon_deg).map_err(|_| FlyByError::InvalidCoordinate)?;
        let pb = LatLon::new(b.lat_deg, b.lon_deg).map_err(|_| FlyByError::InvalidCoordinate)?;
        let leg_len = wgs84()
            .inverse(pa, pb)
            .map_err(|_| FlyByError::ProjectionFailure)?
            .distance_m;
        let outgoing = fillets
            .get(segment_ix)
            .and_then(Option::as_ref)
            .map_or(0.0, |f| f.tangent_distance_m);
        let incoming = fillets
            .get(segment_ix + 1)
            .and_then(Option::as_ref)
            .map_or(0.0, |f| f.tangent_distance_m);
        if outgoing + incoming >= leg_len {
            return Err(FlyByError::AdjacentTurnsOverlap(segment_ix));
        }
    }

    let mut points = Vec::new();
    if let Some(first) = waypoints.first() {
        points.push(RoutePathPoint {
            position: first.position,
            source_fix_index: 0,
            kind: RoutePointKind::ExactFix,
        });
    }
    for fix_ix in 1..waypoints.len().saturating_sub(1) {
        if let Some(fillet) = fillets.get(fix_ix).and_then(Option::as_ref) {
            for position in &fillet.arc {
                points.push(RoutePathPoint {
                    position: *position,
                    source_fix_index: fix_ix,
                    kind: RoutePointKind::TurnArc,
                });
            }
        } else if let Some(waypoint) = waypoints.get(fix_ix) {
            points.push(RoutePathPoint {
                position: waypoint.position,
                source_fix_index: fix_ix,
                kind: RoutePointKind::ExactFix,
            });
        }
        if points.len() > MAX_POINTS {
            return Err(FlyByError::RoutePointLimit);
        }
    }
    if let Some(last) = waypoints.last() {
        points.push(RoutePathPoint {
            position: last.position,
            source_fix_index: waypoints.len() - 1,
            kind: RoutePointKind::ExactFix,
        });
    }
    if points.len() > MAX_POINTS {
        return Err(FlyByError::RoutePointLimit);
    }

    let mut total_length_m = 0.0;
    for pair in points.windows(2) {
        let from = pair.first().ok_or(FlyByError::RouteTooShort)?.position;
        let to = pair.get(1).ok_or(FlyByError::RouteTooShort)?.position;
        let from =
            LatLon::new(from.lat_deg, from.lon_deg).map_err(|_| FlyByError::InvalidCoordinate)?;
        let to = LatLon::new(to.lat_deg, to.lon_deg).map_err(|_| FlyByError::InvalidCoordinate)?;
        total_length_m += wgs84()
            .inverse(from, to)
            .map_err(|_| FlyByError::ProjectionFailure)?
            .distance_m;
        if !total_length_m.is_finite() || total_length_m > MAX_ROUTE_LENGTH_M {
            return Err(FlyByError::RouteLengthLimit);
        }
    }
    Ok(FlyByRoutePath {
        points,
        total_length_m,
    })
}

fn valid(p: GeoPoint) -> bool {
    p.lat_deg.is_finite()
        && p.lon_deg.is_finite()
        && (-90.0..=90.0).contains(&p.lat_deg)
        && (-180.0..=180.0).contains(&p.lon_deg)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use oa_geo::aeqd::{frame_new, project};

    fn point(frame: &oa_geo::aeqd::Frame, x: f64, y: f64) -> GeoPoint {
        let (lat_deg, lon_deg) = oa_geo::aeqd::unproject(frame, x, y).unwrap();
        GeoPoint { lat_deg, lon_deg }
    }

    #[test]
    fn left_right_quarter_turns_have_analytic_tangencies_and_arc_bounds() {
        let b = GeoPoint {
            lat_deg: 0.0,
            lon_deg: 0.0,
        };
        let frame = frame_new(b.lat_deg, b.lon_deg);
        let radius = 1_000.0;
        assert!((radius_for_turn_rate(100.0, 0.1).unwrap() - radius).abs() < 1e-9);

        // East-to-north left turn: tangent distance r, center (-r,+r), quarter-circle length.
        let left = build_fly_by_fillet(
            point(&frame, -10_000.0, 0.0),
            b,
            point(&frame, 0.0, 10_000.0),
            radius,
            16,
        )
        .unwrap();
        let (ex, ey) = project(&frame, left.entry.lat_deg, left.entry.lon_deg);
        let (xx, xy) = project(&frame, left.exit.lat_deg, left.exit.lon_deg);
        assert_eq!(left.direction, 1);
        assert!((left.tangent_distance_m - radius).abs() < 1e-6);
        assert!((ex + radius).abs() < 1e-3 && ey.abs() < 1e-3);
        assert!(xx.abs() < 1e-3 && (xy - radius).abs() < 1e-3);
        assert_eq!(left.arc.len(), 17);
        let (mx, my) = project(&frame, left.arc[8].lat_deg, left.arc[8].lon_deg);
        let diagonal = radius / ::core::f64::consts::SQRT_2;
        assert!((mx - (-radius + diagonal)).abs() < 1e-3);
        assert!((my - (radius - diagonal)).abs() < 1e-3);

        let right = build_fly_by_fillet(
            point(&frame, -10_000.0, 0.0),
            b,
            point(&frame, 0.0, -10_000.0),
            radius,
            16,
        )
        .unwrap();
        assert_eq!(right.direction, -1);
        assert_eq!(right.arc.len(), 17);
    }

    #[test]
    fn short_legs_never_shrink_requested_turn_radius() {
        let frame = frame_new(0.0, 0.0);
        let b = GeoPoint {
            lat_deg: 0.0,
            lon_deg: 0.0,
        };
        let result = build_fly_by_fillet(
            point(&frame, -1_000.0, 0.0),
            b,
            point(&frame, 0.0, 1_000.0),
            2_000.0,
            16,
        );
        assert_eq!(result, Err(FlyByError::LegTooShort));
    }

    #[test]
    fn straight_reversal_and_unbounded_sampling_are_refused() {
        let frame = frame_new(0.0, 0.0);
        let b = GeoPoint {
            lat_deg: 0.0,
            lon_deg: 0.0,
        };
        let a = point(&frame, -10_000.0, 0.0);
        assert_eq!(
            build_fly_by_fillet(a, b, point(&frame, 10_000.0, 0.0), 1_000.0, 16),
            Err(FlyByError::StraightCourse)
        );
        assert_eq!(
            build_fly_by_fillet(a, b, point(&frame, -10_000.0, 0.0), 1_000.0, 16),
            Err(FlyByError::CourseReversal)
        );
        assert_eq!(
            build_fly_by_fillet(a, b, point(&frame, 0.0, 10_000.0), 1_000.0, 513),
            Err(FlyByError::InvalidSampleCount)
        );
    }
}
