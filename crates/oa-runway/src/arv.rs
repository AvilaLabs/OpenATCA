//! Assigned-runway verification (ARV), pure geometry over caller-prepared predictions.
//!
//! This module does not discover an assignment, read runway/taxiway data, authenticate a source,
//! or issue a clearance. The shell supplies full flight identities, the assigned runway path,
//! candidate path geometry, predicted positions/headings, prior alert state, and the authenticated
//! go-around event bit. A go-around clears a latched ARV alert only when that bit is set by the
//! caller from its trusted event path. The geometry and the tolerances below are engineering
//! values, not facility adaptation or procedure authorization.

/// Maximum flights examined per call (C2/C3).
pub const MAX_ARV_FLIGHTS: usize = 256;
/// Maximum runway/taxiway candidate paths per call (C2/C3).
pub const MAX_ARV_PATHS: usize = 512;
/// Maximum conservative work units, including matching, identity validation, path ordering and
/// assigned-path searches (C7).
pub const MAX_ARV_WORK: usize = 200_000;
/// A path is considered only from its threshold outward to this range, metres (5 NM).
pub const MAX_THRESHOLD_RANGE_M: f64 = 9_260.0;
/// Extra lateral tolerance beyond half the supplied physical path width, metres. Development
/// value: 50 m gives a modest allowance for model and tracker error; not a facility setting.
pub const DEFAULT_LATERAL_TOLERANCE_M: f64 = 50.0;
/// Maximum heading difference for a path match, degrees. Development value: 5 degrees keeps the
/// candidate test distinct from a broad approach-sector test; not a facility setting.
pub const DEFAULT_HEADING_TOLERANCE_DEG: f64 = 5.0;
const MAX_PATH_WIDTH_M: f64 = 200.0;
const _: () = assert!(16 + core::mem::size_of::<u32>() + 4 + 12 == 36);
const _: () = assert!(core::mem::size_of::<[u8; MAX_ARV_FLIGHTS]>() == MAX_ARV_FLIGHTS);
const _: () = assert!(core::mem::size_of::<[u32; MAX_ARV_FLIGHTS]>() == 4 * MAX_ARV_FLIGHTS);

/// Full 36-byte flight identity is supplied as separate columns; no display address is used as a
/// substitute for identity. These columns match the regional flight identity components but do
/// not depend on a display or flight-table crate.
pub struct FlightIdentityColumns<'a> {
    pub gufi: &'a [[u8; 16]],
    pub aircraft_address: &'a [u32],
    pub centre: &'a [[u8; 4]],
    pub plan_id: &'a [[u8; 12]],
}

/// One predicted flight row. The position and unit track vector are predicted values at a common
/// caller-selected horizon. `go_around_authenticated` is only an assertion from the trusted
/// caller boundary; this core cannot authenticate the originating message.
pub struct PredictedFlights<'a> {
    pub identity: FlightIdentityColumns<'a>,
    pub assigned_path_id: &'a [u32],
    pub predicted_east_m: &'a [f64],
    pub predicted_north_m: &'a [f64],
    pub track_east: &'a [f64],
    pub track_north: &'a [f64],
    pub go_around_authenticated: &'a [u8],
    /// Previous status, one of `AlertKind` values. This makes alert latching explicit at the
    /// caller boundary; a prior alert remains active until an authenticated go-around event.
    pub prior_status: &'a [u8],
}

/// Candidate centerline geometry in one caller-local plane. The axis points toward the threshold.
/// `kind` is `PathKind::Runway` or `PathKind::Taxiway`; unknown values refuse the entire call.
pub struct ArvPaths<'a> {
    /// Strictly increasing, unique IDs. This enables bounded binary search for assignments.
    pub ids: &'a [u32],
    pub kinds: &'a [u8],
    pub threshold_east_m: &'a [f64],
    pub threshold_north_m: &'a [f64],
    pub inbound_east: &'a [f64],
    pub inbound_north: &'a [f64],
    /// Physical full width from adaptation, metres. No default width is fabricated here.
    pub width_m: &'a [f64],
}

/// Tolerances are caller-configurable. The five-NM threshold range is fixed by this function's
/// scope; the width and heading tolerances are engineering development values unless adapted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArvConfig {
    pub lateral_tolerance_m: f64,
    pub heading_tolerance_deg: f64,
}

pub const DEFAULT_CONFIG: ArvConfig = ArvConfig {
    lateral_tolerance_m: DEFAULT_LATERAL_TOLERANCE_M,
    heading_tolerance_deg: DEFAULT_HEADING_TOLERANCE_DEG,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum PathKind {
    Runway = 1,
    Taxiway = 2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum AlertKind {
    Clear = 0,
    WrongRunway = 1,
    Taxiway = 2,
    GoAroundCleared = 3,
    AmbiguousPath = 4,
}
const _: () = assert!(core::mem::size_of::<AlertKind>() == 1);

/// Caller-owned structure-of-arrays output, one row per flight. Output capacity is validated
/// before any column is changed.
pub struct ArvOutput<'a> {
    pub gufi: &'a mut [[u8; 16]],
    pub aircraft_address: &'a mut [u32],
    pub centre: &'a mut [[u8; 4]],
    pub plan_id: &'a mut [[u8; 12]],
    pub status: &'a mut [u8],
    /// Matched path ID, or zero where no candidate path matched.
    pub matched_path_id: &'a mut [u32],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ArvError {
    BadInput = 1,
    Capacity = 2,
    WorkLimit = 3,
    MissingAssignedRunway = 4,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArvResult {
    pub flights: u16,
    pub alerts: u16,
    pub go_around_clears: u16,
}

fn lengths_match(f: &PredictedFlights<'_>, n: usize) -> bool {
    f.identity.gufi.len() == n
        && f.identity.aircraft_address.len() == n
        && f.identity.centre.len() == n
        && f.identity.plan_id.len() == n
        && f.assigned_path_id.len() == n
        && f.predicted_east_m.len() == n
        && f.predicted_north_m.len() == n
        && f.track_east.len() == n
        && f.track_north.len() == n
        && f.go_around_authenticated.len() == n
        && f.prior_status.len() == n
}

fn output_lengths_match(o: &ArvOutput<'_>, n: usize) -> bool {
    o.gufi.len() == n
        && o.aircraft_address.len() == n
        && o.centre.len() == n
        && o.plan_id.len() == n
        && o.status.len() == n
        && o.matched_path_id.len() == n
}

fn path_kind(value: u8) -> Option<PathKind> {
    match value {
        1 => Some(PathKind::Runway),
        2 => Some(PathKind::Taxiway),
        _ => None,
    }
}

fn alert_kind(value: u8) -> Option<AlertKind> {
    match value {
        0 => Some(AlertKind::Clear),
        1 => Some(AlertKind::WrongRunway),
        2 => Some(AlertKind::Taxiway),
        3 => Some(AlertKind::GoAroundCleared),
        4 => Some(AlertKind::AmbiguousPath),
        _ => None,
    }
}

fn identity_equal(f: &PredictedFlights<'_>, a: usize, b: usize) -> bool {
    f.identity.gufi.get(a) == f.identity.gufi.get(b)
        && f.identity.aircraft_address.get(a) == f.identity.aircraft_address.get(b)
        && f.identity.centre.get(a) == f.identity.centre.get(b)
        && f.identity.plan_id.get(a) == f.identity.plan_id.get(b)
}

fn path_index(paths: &ArvPaths<'_>, id: u32) -> Option<usize> {
    paths.ids.binary_search(&id).ok()
}

fn assignment_search_bound(path_count: usize) -> usize {
    let mut span = 1_usize;
    let mut steps = 1_usize;
    while span < path_count {
        span = span.saturating_mul(2);
        steps = steps.saturating_add(1);
    }
    steps
}

fn work_bound(flight_count: usize, path_count: usize) -> usize {
    flight_count
        .saturating_mul(path_count)
        .saturating_add(flight_count.saturating_mul(flight_count))
        .saturating_add(path_count)
        .saturating_add(flight_count.saturating_mul(assignment_search_bound(path_count)))
}

fn validates(
    config: ArvConfig,
    flights: &PredictedFlights<'_>,
    paths: &ArvPaths<'_>,
    output: &ArvOutput<'_>,
) -> Result<(usize, usize, f64), ArvError> {
    let n = flights.assigned_path_id.len();
    let p = paths.ids.len();
    if n > MAX_ARV_FLIGHTS
        || p > MAX_ARV_PATHS
        || !lengths_match(flights, n)
        || paths.kinds.len() != p
        || paths.threshold_east_m.len() != p
        || paths.threshold_north_m.len() != p
        || paths.inbound_east.len() != p
        || paths.inbound_north.len() != p
        || paths.width_m.len() != p
    {
        return Err(ArvError::BadInput);
    }
    if !output_lengths_match(output, n) {
        return Err(ArvError::Capacity);
    }
    let work = work_bound(n, p);
    if work > MAX_ARV_WORK {
        return Err(ArvError::WorkLimit);
    }
    if !config.lateral_tolerance_m.is_finite()
        || !(0.0..=500.0).contains(&config.lateral_tolerance_m)
        || !config.heading_tolerance_deg.is_finite()
        || !(0.0..=45.0).contains(&config.heading_tolerance_deg)
    {
        return Err(ArvError::BadInput);
    }
    let (_, cosine) = oa_math::sin_cos(config.heading_tolerance_deg.to_radians());
    for i in 0..p {
        let (Some(id), Some(kind), Some(e), Some(north), Some(axis_e), Some(axis_n), Some(width)) = (
            paths.ids.get(i),
            paths.kinds.get(i),
            paths.threshold_east_m.get(i),
            paths.threshold_north_m.get(i),
            paths.inbound_east.get(i),
            paths.inbound_north.get(i),
            paths.width_m.get(i),
        ) else {
            return Err(ArvError::BadInput);
        };
        if *id == 0
            || path_kind(*kind).is_none()
            || !e.is_finite()
            || !north.is_finite()
            || e.abs() > 20_000.0
            || north.abs() > 20_000.0
            || !axis_e.is_finite()
            || !axis_n.is_finite()
            || !width.is_finite()
            || !(1.0..=MAX_PATH_WIDTH_M).contains(width)
        {
            return Err(ArvError::BadInput);
        }
        let norm = oa_math::hypot(*axis_e, *axis_n);
        if !norm.is_finite() || (norm - 1.0).abs() > 1.0e-6 {
            return Err(ArvError::BadInput);
        }
        if i > 0 && paths.ids.get(i - 1).is_some_and(|previous| previous >= id) {
            return Err(ArvError::BadInput);
        }
    }
    for i in 0..n {
        let (Some(e), Some(north), Some(te), Some(tn), Some(auth), Some(prior), Some(assigned)) = (
            flights.predicted_east_m.get(i),
            flights.predicted_north_m.get(i),
            flights.track_east.get(i),
            flights.track_north.get(i),
            flights.go_around_authenticated.get(i),
            flights.prior_status.get(i),
            flights.assigned_path_id.get(i),
        ) else {
            return Err(ArvError::BadInput);
        };
        if !e.is_finite()
            || !north.is_finite()
            || e.abs() > 20_000.0
            || north.abs() > 20_000.0
            || !te.is_finite()
            || !tn.is_finite()
            || *auth > 1
            || alert_kind(*prior).is_none()
            || *assigned == 0
        {
            return Err(ArvError::BadInput);
        }
        let norm = oa_math::hypot(*te, *tn);
        if !norm.is_finite() || (norm - 1.0).abs() > 1.0e-6 {
            return Err(ArvError::BadInput);
        }
        let Some(pi) = path_index(paths, *assigned) else {
            return Err(ArvError::MissingAssignedRunway);
        };
        if paths.kinds.get(pi) != Some(&(PathKind::Runway as u8)) {
            return Err(ArvError::MissingAssignedRunway);
        }
        for j in 0..i {
            if identity_equal(flights, i, j) {
                return Err(ArvError::BadInput);
            }
        }
    }
    Ok((n, p, cosine))
}

/// Evaluate predicted tracks against the assigned runway and candidate parallel paths. This is
/// all-or-nothing: all arrays, identities, assignments, paths, tolerances and work bounds are
/// checked before any output column is written.
pub fn evaluate(
    config: ArvConfig,
    flights: PredictedFlights<'_>,
    paths: ArvPaths<'_>,
    output: ArvOutput<'_>,
) -> Result<ArvResult, ArvError> {
    let (n, p, min_heading_dot) = validates(config, &flights, &paths, &output)?;
    let mut alerts: u16 = 0;
    let mut clears: u16 = 0;
    let mut computed_status = [AlertKind::Clear as u8; MAX_ARV_FLIGHTS];
    let mut computed_path = [0_u32; MAX_ARV_FLIGHTS];
    for i in 0..n {
        let Some(assigned_id) = flights.assigned_path_id.get(i).copied() else {
            return Err(ArvError::BadInput);
        };
        let Some(prior_raw) = flights.prior_status.get(i).copied() else {
            return Err(ArvError::BadInput);
        };
        let Some(auth) = flights.go_around_authenticated.get(i).copied() else {
            return Err(ArvError::BadInput);
        };
        let prior = alert_kind(prior_raw).ok_or(ArvError::BadInput)?;
        let (mut status, mut matched_id) = (AlertKind::Clear, 0_u32);
        if matches!(
            prior,
            AlertKind::WrongRunway | AlertKind::Taxiway | AlertKind::AmbiguousPath
        ) {
            if auth == 1 {
                status = AlertKind::GoAroundCleared;
                clears = clears.saturating_add(1);
            } else {
                status = prior;
            }
        } else {
            let east = *flights.predicted_east_m.get(i).ok_or(ArvError::BadInput)?;
            let north = *flights.predicted_north_m.get(i).ok_or(ArvError::BadInput)?;
            let track_east = *flights.track_east.get(i).ok_or(ArvError::BadInput)?;
            let track_north = *flights.track_north.get(i).ok_or(ArvError::BadInput)?;
            let mut assigned_match = false;
            let mut wrong_match: Option<(AlertKind, u32)> = None;
            let mut multiple_wrong = false;
            for pi in 0..p {
                let id = *paths.ids.get(pi).ok_or(ArvError::BadInput)?;
                let threshold_e = *paths.threshold_east_m.get(pi).ok_or(ArvError::BadInput)?;
                let threshold_n = *paths.threshold_north_m.get(pi).ok_or(ArvError::BadInput)?;
                let axis_e = *paths.inbound_east.get(pi).ok_or(ArvError::BadInput)?;
                let axis_n = *paths.inbound_north.get(pi).ok_or(ArvError::BadInput)?;
                let half_width = *paths.width_m.get(pi).ok_or(ArvError::BadInput)? * 0.5;
                let dx = east - threshold_e;
                let dy = north - threshold_n;
                let outward = -(dx * axis_e + dy * axis_n);
                let lateral = dx * axis_n - dy * axis_e;
                let aligned = track_east * axis_e + track_north * axis_n >= min_heading_dot;
                let in_capture = (0.0..=MAX_THRESHOLD_RANGE_M).contains(&outward)
                    && lateral.abs() <= half_width + config.lateral_tolerance_m
                    && aligned;
                if in_capture {
                    if id == assigned_id {
                        assigned_match = true;
                    } else {
                        let kind = match path_kind(*paths.kinds.get(pi).ok_or(ArvError::BadInput)?)
                        {
                            Some(PathKind::Runway) => AlertKind::WrongRunway,
                            Some(PathKind::Taxiway) => AlertKind::Taxiway,
                            None => return Err(ArvError::BadInput),
                        };
                        if wrong_match.is_some() {
                            multiple_wrong = true;
                        } else {
                            wrong_match = Some((kind, id));
                        }
                    }
                }
            }
            if assigned_match {
                status = AlertKind::Clear;
            } else if multiple_wrong {
                status = AlertKind::AmbiguousPath;
            } else if let Some((kind, id)) = wrong_match {
                status = kind;
                matched_id = id;
            }
        }
        if matches!(
            status,
            AlertKind::WrongRunway | AlertKind::Taxiway | AlertKind::AmbiguousPath
        ) {
            alerts = alerts.saturating_add(1);
        }
        *computed_status.get_mut(i).ok_or(ArvError::BadInput)? = status as u8;
        *computed_path.get_mut(i).ok_or(ArvError::BadInput)? = matched_id;
    }
    // Validation and all geometry complete before output mutation. All columns have equal lengths;
    // zip therefore writes exactly n records without an error path or partial failure.
    for (out, input) in output.gufi.iter_mut().zip(flights.identity.gufi.iter()) {
        *out = *input;
    }
    for (out, input) in output
        .aircraft_address
        .iter_mut()
        .zip(flights.identity.aircraft_address.iter())
    {
        *out = *input;
    }
    for (out, input) in output.centre.iter_mut().zip(flights.identity.centre.iter()) {
        *out = *input;
    }
    for (out, input) in output
        .plan_id
        .iter_mut()
        .zip(flights.identity.plan_id.iter())
    {
        *out = *input;
    }
    for (out, input) in output.status.iter_mut().zip(computed_status.iter()).take(n) {
        *out = *input;
    }
    for (out, input) in output
        .matched_path_id
        .iter_mut()
        .zip(computed_path.iter())
        .take(n)
    {
        *out = *input;
    }
    Ok(ArvResult {
        flights: n as u16,
        alerts,
        go_around_clears: clears,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn evaluate_one(
        east: f64,
        north: f64,
        track_east_component: f64,
        assigned: u32,
        prior: u8,
        go_around: u8,
    ) -> (ArvResult, u8, u32, [u8; 16], u32, [u8; 4], [u8; 12]) {
        let gufi = [[0xA5; 16]];
        let address = [0x123456];
        let centre = [*b"ORD "];
        let plan_id = [*b"PLAN-001    "];
        let assigned_id = [assigned];
        let e = [east];
        let north = [north];
        let track_norm = oa_math::hypot(track_east_component, 1.0);
        let track_east = [track_east_component / track_norm];
        let track_north = [1.0 / track_norm];
        let auth = [go_around];
        let prior_status = [prior];
        let ids = [10, 20, 30];
        let kinds = [
            PathKind::Runway as u8,
            PathKind::Runway as u8,
            PathKind::Taxiway as u8,
        ];
        let threshold_e = [0.0, 300.0, 600.0];
        let threshold_n = [0.0; 3];
        let axis_east = [0.0; 3];
        let axis_north = [1.0; 3];
        let widths = [30.0; 3];
        let mut out_gufi = [[0; 16]; 1];
        let mut out_address = [0; 1];
        let mut out_centre = [[0; 4]; 1];
        let mut out_plan_id = [[0; 12]; 1];
        let mut out_status = [255; 1];
        let mut out_path = [u32::MAX; 1];
        let result = evaluate(
            DEFAULT_CONFIG,
            PredictedFlights {
                identity: FlightIdentityColumns {
                    gufi: &gufi,
                    aircraft_address: &address,
                    centre: &centre,
                    plan_id: &plan_id,
                },
                assigned_path_id: &assigned_id,
                predicted_east_m: &e,
                predicted_north_m: &north,
                track_east: &track_east,
                track_north: &track_north,
                go_around_authenticated: &auth,
                prior_status: &prior_status,
            },
            ArvPaths {
                ids: &ids,
                kinds: &kinds,
                threshold_east_m: &threshold_e,
                threshold_north_m: &threshold_n,
                inbound_east: &axis_east,
                inbound_north: &axis_north,
                width_m: &widths,
            },
            ArvOutput {
                gufi: &mut out_gufi,
                aircraft_address: &mut out_address,
                centre: &mut out_centre,
                plan_id: &mut out_plan_id,
                status: &mut out_status,
                matched_path_id: &mut out_path,
            },
        );
        assert!(result.is_ok(), "well-formed ARV prediction was refused");
        let result = result.unwrap_or(ArvResult {
            flights: 0,
            alerts: 0,
            go_around_clears: 0,
        });
        (
            result,
            out_status.first().copied().unwrap_or_default(),
            out_path.first().copied().unwrap_or_default(),
            out_gufi.first().copied().unwrap_or_default(),
            out_address.first().copied().unwrap_or_default(),
            out_centre.first().copied().unwrap_or_default(),
            out_plan_id.first().copied().unwrap_or_default(),
        )
    }

    // REQ: OA-RWY-020, OA-RWY-021
    #[test]
    fn assigned_runway_match_is_clear_and_keeps_full_identity() {
        let (result, status, matched, gufi, address, centre, plan) =
            evaluate_one(0.0, -5_000.0, 0.0, 10, AlertKind::Clear as u8, 0);
        assert_eq!(result.alerts, 0);
        assert_eq!(status, AlertKind::Clear as u8);
        assert_eq!(matched, 0);
        assert_eq!(gufi, [0xA5; 16]);
        assert_eq!(address, 0x123456);
        assert_eq!(centre, *b"ORD ");
        assert_eq!(plan, *b"PLAN-001    ");
    }

    // REQ: OA-RWY-020
    #[test]
    fn parallel_runway_and_taxiway_mismatches_alert() {
        let (runway_result, runway_status, runway_id, ..) =
            evaluate_one(300.0, -5_000.0, 0.0, 10, AlertKind::Clear as u8, 0);
        assert_eq!(runway_result.alerts, 1);
        assert_eq!(runway_status, AlertKind::WrongRunway as u8);
        assert_eq!(runway_id, 20);

        let (taxi_result, taxi_status, taxi_id, ..) =
            evaluate_one(600.0, -5_000.0, 0.0, 10, AlertKind::Clear as u8, 0);
        assert_eq!(taxi_result.alerts, 1);
        assert_eq!(taxi_status, AlertKind::Taxiway as u8);
        assert_eq!(taxi_id, 30);
    }

    // REQ: OA-RWY-020
    #[test]
    fn only_authenticated_go_around_clears_latched_alert() {
        let (result, status, ..) =
            evaluate_one(300.0, -5_000.0, 0.0, 10, AlertKind::WrongRunway as u8, 1);
        assert_eq!(result.go_around_clears, 1);
        assert_eq!(result.alerts, 0);
        assert_eq!(status, AlertKind::GoAroundCleared as u8);

        let (still_latched, status, ..) =
            evaluate_one(0.0, -5_000.0, 0.0, 10, AlertKind::WrongRunway as u8, 0);
        assert_eq!(still_latched.alerts, 1);
        assert_eq!(status, AlertKind::WrongRunway as u8);
    }

    // REQ: OA-RWY-020
    #[test]
    fn threshold_range_and_heading_tolerances_bound_candidate_matches() {
        let (at_limit, status, ..) = evaluate_one(
            300.0,
            -MAX_THRESHOLD_RANGE_M,
            0.0,
            10,
            AlertKind::Clear as u8,
            0,
        );
        assert_eq!(at_limit.alerts, 1);
        assert_eq!(status, AlertKind::WrongRunway as u8);

        let (past_limit, status, ..) = evaluate_one(
            300.0,
            -(MAX_THRESHOLD_RANGE_M + 0.1),
            0.0,
            10,
            AlertKind::Clear as u8,
            0,
        );
        assert_eq!(past_limit.alerts, 0);
        assert_eq!(status, AlertKind::Clear as u8);

        let (off_heading, status, ..) =
            evaluate_one(300.0, -5_000.0, 0.139_173_1, 10, AlertKind::Clear as u8, 0);
        assert_eq!(off_heading.alerts, 0);
        assert_eq!(status, AlertKind::Clear as u8);
    }

    // REQ: OA-RWY-021
    #[test]
    fn invalid_late_row_and_duplicate_identity_leave_output_unchanged() {
        let gufi = [[1; 16], [2; 16]];
        let address = [1, 2];
        let centre = [*b"A   ", *b"B   "];
        let plan_id = [*b"P1          ", *b"P2          "];
        let assigned = [10, 10];
        let e = [0.0, f64::NAN];
        let north = [-1000.0; 2];
        let te = [0.0; 2];
        let tn = [1.0; 2];
        let auth = [0; 2];
        let prior = [0; 2];
        let ids = [10];
        let kinds = [PathKind::Runway as u8];
        let threshold_e = [0.0];
        let threshold_n = [0.0];
        let axis_e = [0.0];
        let axis_n = [1.0];
        let widths = [30.0];
        let mut out_gufi = [[9; 16]; 2];
        let mut out_address = [9; 2];
        let mut out_centre = [[9; 4]; 2];
        let mut out_plan = [[9; 12]; 2];
        let mut out_status = [9; 2];
        let mut out_path = [9; 2];
        let result = evaluate(
            DEFAULT_CONFIG,
            PredictedFlights {
                identity: FlightIdentityColumns {
                    gufi: &gufi,
                    aircraft_address: &address,
                    centre: &centre,
                    plan_id: &plan_id,
                },
                assigned_path_id: &assigned,
                predicted_east_m: &e,
                predicted_north_m: &north,
                track_east: &te,
                track_north: &tn,
                go_around_authenticated: &auth,
                prior_status: &prior,
            },
            ArvPaths {
                ids: &ids,
                kinds: &kinds,
                threshold_east_m: &threshold_e,
                threshold_north_m: &threshold_n,
                inbound_east: &axis_e,
                inbound_north: &axis_n,
                width_m: &widths,
            },
            ArvOutput {
                gufi: &mut out_gufi,
                aircraft_address: &mut out_address,
                centre: &mut out_centre,
                plan_id: &mut out_plan,
                status: &mut out_status,
                matched_path_id: &mut out_path,
            },
        );
        assert_eq!(result, Err(ArvError::BadInput));
        assert_eq!(out_gufi, [[9; 16]; 2]);
        assert_eq!(out_address, [9; 2]);
        assert_eq!(out_centre, [[9; 4]; 2]);
        assert_eq!(out_plan, [[9; 12]; 2]);
        assert_eq!(out_status, [9; 2]);
        assert_eq!(out_path, [9; 2]);

        let duplicated_address = [1, 1];
        let duplicated_gufi = [[1; 16], [1; 16]];
        let result = evaluate(
            DEFAULT_CONFIG,
            PredictedFlights {
                identity: FlightIdentityColumns {
                    gufi: &duplicated_gufi,
                    aircraft_address: &duplicated_address,
                    centre: &[*b"A   ", *b"A   "],
                    plan_id: &[*b"P1          ", *b"P1          "],
                },
                assigned_path_id: &assigned,
                predicted_east_m: &[0.0; 2],
                predicted_north_m: &north,
                track_east: &te,
                track_north: &tn,
                go_around_authenticated: &auth,
                prior_status: &prior,
            },
            ArvPaths {
                ids: &ids,
                kinds: &kinds,
                threshold_east_m: &threshold_e,
                threshold_north_m: &threshold_n,
                inbound_east: &axis_e,
                inbound_north: &axis_n,
                width_m: &widths,
            },
            ArvOutput {
                gufi: &mut out_gufi,
                aircraft_address: &mut out_address,
                centre: &mut out_centre,
                plan_id: &mut out_plan,
                status: &mut out_status,
                matched_path_id: &mut out_path,
            },
        );
        assert_eq!(result, Err(ArvError::BadInput));
    }

    // REQ: OA-RWY-021
    #[test]
    fn malformed_and_over_capacity_inputs_refuse_before_mutation() {
        let (_, status, ..) = evaluate_one(100.0, -5_000.0, 0.0, 10, AlertKind::Clear as u8, 0);
        assert_eq!(status, AlertKind::Clear as u8);
        let gufi = [[1; 16]];
        let addresses = [1];
        let centres = [*b"A   "];
        let plans = [*b"P1          "];
        let assigned = [99];
        let e = [0.0];
        let north = [-1000.0];
        let te = [0.0];
        let tn = [1.0];
        let auth = [0];
        let prior = [0];
        let ids = [10];
        let kinds = [PathKind::Runway as u8];
        let threshold = [0.0];
        let axis_e = [0.0];
        let axis_n = [1.0];
        let widths = [30.0];
        let mut out_gufi = [[7; 16]];
        let mut out_address = [7];
        let mut out_centre = [[7; 4]];
        let mut out_plan = [[7; 12]];
        let mut out_status = [7];
        let mut out_path = [7];
        let result = evaluate(
            DEFAULT_CONFIG,
            PredictedFlights {
                identity: FlightIdentityColumns {
                    gufi: &gufi,
                    aircraft_address: &addresses,
                    centre: &centres,
                    plan_id: &plans,
                },
                assigned_path_id: &assigned,
                predicted_east_m: &e,
                predicted_north_m: &north,
                track_east: &te,
                track_north: &tn,
                go_around_authenticated: &auth,
                prior_status: &prior,
            },
            ArvPaths {
                ids: &ids,
                kinds: &kinds,
                threshold_east_m: &threshold,
                threshold_north_m: &threshold,
                inbound_east: &axis_e,
                inbound_north: &axis_n,
                width_m: &widths,
            },
            ArvOutput {
                gufi: &mut out_gufi,
                aircraft_address: &mut out_address,
                centre: &mut out_centre,
                plan_id: &mut out_plan,
                status: &mut out_status,
                matched_path_id: &mut out_path,
            },
        );
        assert_eq!(result, Err(ArvError::MissingAssignedRunway));
        assert_eq!(out_status, [7]);
        assert_eq!(out_path, [7]);

        let too_many = [10_u32; MAX_ARV_FLIGHTS + 1];
        let result = evaluate(
            DEFAULT_CONFIG,
            PredictedFlights {
                identity: FlightIdentityColumns {
                    gufi: &[],
                    aircraft_address: &[],
                    centre: &[],
                    plan_id: &[],
                },
                assigned_path_id: &too_many,
                predicted_east_m: &[],
                predicted_north_m: &[],
                track_east: &[],
                track_north: &[],
                go_around_authenticated: &[],
                prior_status: &[],
            },
            ArvPaths {
                ids: &ids,
                kinds: &kinds,
                threshold_east_m: &threshold,
                threshold_north_m: &threshold,
                inbound_east: &axis_e,
                inbound_north: &axis_n,
                width_m: &widths,
            },
            ArvOutput {
                gufi: &mut [],
                aircraft_address: &mut [],
                centre: &mut [],
                plan_id: &mut [],
                status: &mut [],
                matched_path_id: &mut [],
            },
        );
        assert_eq!(result, Err(ArvError::BadInput));
    }

    // REQ: OA-RWY-021
    #[test]
    fn maximum_capacities_fit_checked_work_budget() {
        let n = MAX_ARV_FLIGHTS;
        let p = MAX_ARV_PATHS;
        assert_eq!(work_bound(n, p), 199_680);
        assert!(work_bound(n, p) <= MAX_ARV_WORK);
        assert!(work_bound(n, p + 1) > MAX_ARV_WORK);
        let gufi: Vec<[u8; 16]> = (0..n)
            .map(|value| {
                let mut identity = [0; 16];
                if let Some(last) = identity.last_mut() {
                    *last = value as u8;
                }
                identity
            })
            .collect();
        let addresses: Vec<u32> = (0..n).map(|value| value as u32 + 1).collect();
        let centres = vec![*b"TEST"; n];
        let plans = vec![*b"PLAN        "; n];
        let assigned = vec![1_u32; n];
        let positions = vec![0.0_f64; n];
        let track_east = vec![0.0_f64; n];
        let track_north = vec![1.0_f64; n];
        let auth = vec![0_u8; n];
        let prior = vec![0_u8; n];
        let ids: Vec<u32> = (1..=p as u32).collect();
        let kinds = vec![PathKind::Runway as u8; p];
        let thresholds = vec![0.0_f64; p];
        let axis_east = vec![0.0_f64; p];
        let axis_north = vec![1.0_f64; p];
        let widths = vec![30.0_f64; p];
        let mut out_gufi = vec![[9_u8; 16]; n];
        let mut out_addresses = vec![9_u32; n];
        let mut out_centres = vec![[9_u8; 4]; n];
        let mut out_plans = vec![[9_u8; 12]; n];
        let mut out_status = vec![9_u8; n];
        let mut out_paths = vec![9_u32; n];
        let result = evaluate(
            DEFAULT_CONFIG,
            PredictedFlights {
                identity: FlightIdentityColumns {
                    gufi: &gufi,
                    aircraft_address: &addresses,
                    centre: &centres,
                    plan_id: &plans,
                },
                assigned_path_id: &assigned,
                predicted_east_m: &positions,
                predicted_north_m: &positions,
                track_east: &track_east,
                track_north: &track_north,
                go_around_authenticated: &auth,
                prior_status: &prior,
            },
            ArvPaths {
                ids: &ids,
                kinds: &kinds,
                threshold_east_m: &thresholds,
                threshold_north_m: &thresholds,
                inbound_east: &axis_east,
                inbound_north: &axis_north,
                width_m: &widths,
            },
            ArvOutput {
                gufi: &mut out_gufi,
                aircraft_address: &mut out_addresses,
                centre: &mut out_centres,
                plan_id: &mut out_plans,
                status: &mut out_status,
                matched_path_id: &mut out_paths,
            },
        );
        assert_eq!(
            result,
            Ok(ArvResult {
                flights: MAX_ARV_FLIGHTS as u16,
                alerts: 0,
                go_around_clears: 0,
            })
        );
        assert!(out_gufi.iter().zip(gufi.iter()).all(|(a, b)| a == b));
        assert!(out_addresses
            .iter()
            .zip(addresses.iter())
            .all(|(a, b)| a == b));
        assert!(out_centres.iter().zip(centres.iter()).all(|(a, b)| a == b));
        assert!(out_plans.iter().zip(plans.iter()).all(|(a, b)| a == b));
        assert!(out_status
            .iter()
            .all(|status| *status == AlertKind::Clear as u8));
        assert!(out_paths.iter().all(|path| *path == 0));
    }

    // REQ: OA-RWY-021
    #[test]
    fn candidate_path_ids_must_be_strictly_increasing() {
        let ids = [20_u32, 10];
        let kinds = [PathKind::Runway as u8; 2];
        let thresholds = [0.0_f64; 2];
        let axis_east = [0.0_f64; 2];
        let axis_north = [1.0_f64; 2];
        let widths = [30.0_f64; 2];
        let mut out_gufi = [];
        let mut out_address = [];
        let mut out_centre = [];
        let mut out_plan = [];
        let mut out_status = [];
        let mut out_path = [];
        let result = evaluate(
            DEFAULT_CONFIG,
            PredictedFlights {
                identity: FlightIdentityColumns {
                    gufi: &[],
                    aircraft_address: &[],
                    centre: &[],
                    plan_id: &[],
                },
                assigned_path_id: &[],
                predicted_east_m: &[],
                predicted_north_m: &[],
                track_east: &[],
                track_north: &[],
                go_around_authenticated: &[],
                prior_status: &[],
            },
            ArvPaths {
                ids: &ids,
                kinds: &kinds,
                threshold_east_m: &thresholds,
                threshold_north_m: &thresholds,
                inbound_east: &axis_east,
                inbound_north: &axis_north,
                width_m: &widths,
            },
            ArvOutput {
                gufi: &mut out_gufi,
                aircraft_address: &mut out_address,
                centre: &mut out_centre,
                plan_id: &mut out_plan,
                status: &mut out_status,
                matched_path_id: &mut out_path,
            },
        );
        assert_eq!(result, Err(ArvError::BadInput));
    }
}
