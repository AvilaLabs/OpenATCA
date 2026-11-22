//! First scored 4D arrival-trajectory slice (M4). Pure deterministic projection and scoring.
//!
//! `IssueSnapshot` is an issue-time view: callers must apply the source availability/lag rules
//! before constructing it. Forecast code sees no future samples. Route geometry and the selected
//! meter fix are supplied by the caller after resolving names against NASR; unresolved names are
//! represented as absent and produce absent metrics.

use crate::core::cross::{approach, great_circle_m};
use crate::core::types::{PathPoint, Place};

const HORIZON_S: [i64; 3] = [300, 600, 1200];
/// C7: converted routes above this limit are refused before any route scan.
pub const MAX_ROUTE_FIXES: usize = 256;
/// C7: truth above this limit is refused before sorting or path construction.
pub const MAX_TRUTH_POINTS: usize = 100_000;
const MAX_TRACK_AGE_MS: i64 = 120_000;
const MAX_TRUTH_GAP_MS: i64 = 30_000;
const MIN_ALTITUDE_FT: f64 = -2_000.0;
const MAX_ALTITUDE_FT: f64 = 100_000.0;
const MAX_VERTICAL_RATE_FPM: f64 = 10_000.0;
const MAX_GROUND_SPEED_MPS: f64 = 1_000.0;
const MAX_METER_TOLERANCE_M: f64 = 500.0 * 1852.0;

/// Why an issue snapshot could not be forecast.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForecastRefusal {
    NonpositiveIssueTime,
    IssueBeforeCutoff,
    TrackAfterCutoff,
    TrackTooStale,
    InvalidTrackPlace,
    MissingSpeed,
    InvalidSpeed,
    InvalidTrackValue,
    TooManyRouteFixes,
    EmptyRoute,
    InvalidRoutePlace,
    InvalidMeterFix,
    EtaOverflow,
}

/// Why future evidence could not be scored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScoreRefusal {
    TooManyTruthPoints,
    InvalidMeterTolerance,
    InvalidTruthPoint,
    ConflictingTruthTimestamp,
    InvalidForecast,
    InvalidMeterFix,
    MeterFixMismatch,
}

/// A uniquely resolved route point. `name` is retained for diagnostics and fix selection.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RouteFix {
    pub name: [u8; 8],
    pub place: Place,
    /// Published altitude window at this point when present in the issue input.
    pub altitude_constraint: Option<AltitudeConstraint>,
}
// A7.2: cold struct, size guard waived; one forecast input value.

fn fix_identity(fix: RouteFix) -> ([u8; 8], Place) {
    (fix.name, fix.place)
}

/// Origin is carried with the value so predictions retain constraint provenance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConstraintProvenance {
    SfdpsAssignedAltitude,
    PublishedProcedure,
    Synthetic,
}

/// Inclusive altitude window in feet, available to the predictor at `available_ms`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AltitudeConstraint {
    pub minimum_ft: Option<f64>,
    pub maximum_ft: Option<f64>,
    pub available_ms: i64,
    pub provenance: ConstraintProvenance,
}

/// Track state available at issue time. Speed is ground speed in m/s. Vertical rate is ft/min.
/// Missing altitude/rate stay missing in forecasts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrackState {
    pub time_ms: i64,
    pub place: Place,
    pub altitude_ft: Option<f64>,
    pub speed_mps: Option<f64>,
    pub vertical_rate_fpm: Option<f64>,
}
// A7.2: cold struct, size guard waived; one forecast input value.

/// Issue-time information only. `cutoff_ms` prevents stale or future track states from being
/// accidentally used. Assigned altitude is optional and never inferred from the flown future.
#[derive(Clone, Debug, PartialEq)]
pub struct IssueSnapshot {
    pub cutoff_ms: i64,
    pub issued_ms: i64,
    pub track: TrackState,
    pub assigned_altitude_ft: Option<f64>,
    /// Source of assigned altitude; absent when its provenance is unknown.
    pub assigned_altitude_provenance: Option<ConstraintProvenance>,
    /// Ordered converted route points; the first point is the next fix after the track state.
    pub route: Vec<RouteFix>,
    /// The selected metering fix, if its name resolved unambiguously.
    pub meter_fix: Option<RouteFix>,
}
// A7.2: cold struct, size guard waived; assembled once per scored issue.

/// Forecast position and altitude at one horizon from the issue time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ForecastPoint {
    pub time_ms: i64,
    pub place: Option<Place>,
    pub altitude_ft: Option<f64>,
    pub vertical_status: VerticalStatus,
}
// A7.2: cold struct, size guard waived; one-shot forecast result.

/// Why vertical output is present or unavailable at a forecast horizon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerticalStatus {
    Available,
    MissingAltitude,
    MissingVerticalRate,
    OutsideModelRange,
    ConstraintConflict {
        constraint: ConstraintProvenance,
        assigned_altitude: Option<ConstraintProvenance>,
    },
}

/// Predicted metering-fix arrival; absent if the fix, speed, or usable route is missing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeterEta {
    pub fix: RouteFix,
    pub time_ms: i64,
}
// A7.2: cold struct, size guard waived; one-shot forecast result.

/// Deterministic forecast with 5/10/20 minute positions and selected-fix ETA.
#[derive(Clone, Debug, PartialEq)]
pub struct Forecast {
    pub issued_ms: i64,
    pub cutoff_ms: i64,
    /// The unique meter fix selected from this issue snapshot, if one was resolved.
    pub meter_fix: Option<RouteFix>,
    pub positions: [ForecastPoint; 3],
    pub meter_eta: Option<MeterEta>,
}
// A7.2: cold struct, size guard waived; one-shot forecast result.

/// Predict from a single issue-time snapshot. Horizontal motion follows the supplied route at
/// constant ground speed and is absent beyond its last point. Vertical motion uses current vertical
/// rate and captures at assigned altitude; without either altitude or rate it remains nil.
/// Invalid, future, stale and degenerate input returns a named refusal rather than a partial
/// forecast.
pub fn forecast(snapshot: &IssueSnapshot) -> Result<Forecast, ForecastRefusal> {
    let t = snapshot.track;
    if snapshot.issued_ms <= 0 || snapshot.cutoff_ms <= 0 {
        return Err(ForecastRefusal::NonpositiveIssueTime);
    }
    if snapshot.issued_ms < snapshot.cutoff_ms {
        return Err(ForecastRefusal::IssueBeforeCutoff);
    }
    if t.time_ms > snapshot.cutoff_ms {
        return Err(ForecastRefusal::TrackAfterCutoff);
    }
    if snapshot
        .issued_ms
        .checked_sub(t.time_ms)
        .is_none_or(|age| age > MAX_TRACK_AGE_MS)
    {
        return Err(ForecastRefusal::TrackTooStale);
    }
    if !valid_place(t.place) {
        return Err(ForecastRefusal::InvalidTrackPlace);
    }
    if [
        t.altitude_ft,
        t.speed_mps,
        t.vertical_rate_fpm,
        snapshot.assigned_altitude_ft,
    ]
    .into_iter()
    .flatten()
    .any(|v| !v.is_finite())
    {
        return Err(ForecastRefusal::InvalidTrackValue);
    }
    let speed = t.speed_mps.ok_or(ForecastRefusal::MissingSpeed)?;
    if speed <= 1.0 || speed > MAX_GROUND_SPEED_MPS {
        return Err(ForecastRefusal::InvalidSpeed);
    }
    if t.altitude_ft
        .is_some_and(|v| !(MIN_ALTITUDE_FT..=MAX_ALTITUDE_FT).contains(&v))
        || snapshot
            .assigned_altitude_ft
            .is_some_and(|v| !(MIN_ALTITUDE_FT..=MAX_ALTITUDE_FT).contains(&v))
        || t.vertical_rate_fpm
            .is_some_and(|v| v.abs() > MAX_VERTICAL_RATE_FPM)
    {
        return Err(ForecastRefusal::InvalidTrackValue);
    }
    if snapshot.route.len() > MAX_ROUTE_FIXES {
        return Err(ForecastRefusal::TooManyRouteFixes);
    }
    if snapshot.route.is_empty() {
        return Err(ForecastRefusal::EmptyRoute);
    }
    if snapshot.route.iter().any(|p| !valid_place(p.place)) {
        return Err(ForecastRefusal::InvalidRoutePlace);
    }
    if snapshot
        .route
        .iter()
        .filter_map(|p| p.altitude_constraint)
        .any(|c| {
            c.minimum_ft.is_some_and(|v| !v.is_finite())
                || c.maximum_ft.is_some_and(|v| !v.is_finite())
                || c.available_ms <= 0
                || c.available_ms > snapshot.cutoff_ms
                || c.minimum_ft
                    .is_some_and(|v| !(MIN_ALTITUDE_FT..=MAX_ALTITUDE_FT).contains(&v))
                || c.maximum_ft
                    .is_some_and(|v| !(MIN_ALTITUDE_FT..=MAX_ALTITUDE_FT).contains(&v))
                || matches!((c.minimum_ft, c.maximum_ft), (Some(lo), Some(hi)) if lo > hi)
        })
    {
        return Err(ForecastRefusal::InvalidTrackValue);
    }
    if snapshot.meter_fix.is_some_and(|p| !valid_place(p.place)) {
        return Err(ForecastRefusal::InvalidMeterFix);
    }
    let mut points = [ForecastPoint {
        time_ms: 0,
        place: None,
        altitude_ft: None,
        vertical_status: VerticalStatus::MissingAltitude,
    }; 3];
    for (i, seconds) in HORIZON_S.iter().copied().enumerate() {
        let delta_ms = seconds
            .checked_mul(1000)
            .ok_or(ForecastRefusal::EtaOverflow)?;
        let time_ms = snapshot
            .issued_ms
            .checked_add(delta_ms)
            .ok_or(ForecastRefusal::EtaOverflow)?;
        let elapsed_ms = time_ms
            .checked_sub(t.time_ms)
            .ok_or(ForecastRefusal::EtaOverflow)?;
        if let Some(point) = points.get_mut(i) {
            let vertical = vertical_at(
                t,
                snapshot.assigned_altitude_ft,
                snapshot.assigned_altitude_provenance,
                &snapshot.route,
                speed,
                elapsed_ms as f64 / 1000.0,
            );
            *point = ForecastPoint {
                time_ms,
                place: route_position(t.place, speed, &snapshot.route, elapsed_ms as f64 / 1000.0),
                altitude_ft: vertical.altitude_ft,
                vertical_status: vertical.status,
            };
        }
    }
    let meter_eta = if let Some(fix) = snapshot.meter_fix {
        route_distance_to(t.place, &snapshot.route, fix)
            .map(|distance| {
                let delta_ms_f = (distance / speed) * 1000.0;
                if !delta_ms_f.is_finite() || delta_ms_f < 0.0 || delta_ms_f >= i64::MAX as f64 {
                    return Err(ForecastRefusal::EtaOverflow);
                }
                let delta_ms = oa_ieee::round(delta_ms_f) as i64;
                let time_ms = t
                    .time_ms
                    .checked_add(delta_ms)
                    .ok_or(ForecastRefusal::EtaOverflow)?;
                if time_ms <= snapshot.issued_ms {
                    return Ok(None);
                }
                Ok(Some(MeterEta { fix, time_ms }))
            })
            .transpose()?
            .flatten()
    } else {
        None
    };
    Ok(Forecast {
        issued_ms: snapshot.issued_ms,
        cutoff_ms: snapshot.cutoff_ms,
        meter_fix: snapshot.meter_fix,
        positions: points,
        meter_eta,
    })
}

/// Actual flown state. Samples around future realization times may bracket an interpolation;
/// no sample changes the forecast that was issued from the cutoff snapshot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TruthPoint {
    pub time_ms: i64,
    pub place: Place,
    pub altitude_ft: Option<f64>,
    /// True when truth has a gap immediately before this sample; gaps over 30 seconds are also
    /// refused as interpolation segments even if this flag is false.
    pub gap_before: bool,
}
// A7.2: cold struct, size guard waived; one-shot scored truth input.

/// A legacy comparator estimate, explicitly stamped with when it became available.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LegacyEta {
    pub issued_ms: i64,
    pub eta_ms: i64,
}
// A7.2: cold struct, size guard waived; one comparator estimate.

/// Error and coverage for one horizon. `None` means unavailable, never zero error.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HorizonScore {
    pub horizontal_error_m: Option<f64>,
    pub vertical_error_ft: Option<f64>,
    pub prediction_available: bool,
    pub truth_available: bool,
    pub truth_altitude_available: bool,
    pub covered: bool,
}
// A7.2: cold struct, size guard waived; one-shot score output.

/// Scored issue. Coverage counts eligible truth samples; errors are nil if their evidence is
/// absent. Legacy columns are ETA errors against the actual meter-fix passage.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ArrivalScore {
    pub forecast: [HorizonScore; 3],
    pub forecast_covered: u8,
    pub forecast_possible: u8,
    pub actual_meter_ms: Option<i64>,
    pub oa_eta_error_s: Option<f64>,
    pub eram_eta_error_s: Option<f64>,
    pub tb_fm_eta_error_s: Option<f64>,
}
// A7.2: cold struct, size guard waived; one-shot score output.

/// Score a forecast against future truth plus issue-time ERAM/TBFM estimates. The comparator is
/// suppressed when its receive/availability stamp is later than this forecast's input cutoff.
pub fn score(
    forecast: &Forecast,
    truth: &[TruthPoint],
    meter_fix: Option<RouteFix>,
    eram: Option<LegacyEta>,
    tb_fm: Option<LegacyEta>,
    meter_tolerance_m: f64,
) -> Result<ArrivalScore, ScoreRefusal> {
    if truth.len() > MAX_TRUTH_POINTS {
        return Err(ScoreRefusal::TooManyTruthPoints);
    }
    if !meter_tolerance_m.is_finite()
        || meter_tolerance_m <= 0.0
        || meter_tolerance_m > MAX_METER_TOLERANCE_M
    {
        return Err(ScoreRefusal::InvalidMeterTolerance);
    }
    if forecast.issued_ms <= 0
        || forecast.cutoff_ms <= 0
        || forecast.issued_ms < forecast.cutoff_ms
        || forecast.positions.windows(2).any(|w| {
            w.first()
                .zip(w.get(1))
                .is_none_or(|(a, b)| a.time_ms >= b.time_ms)
        })
        || forecast.positions.iter().any(|p| {
            p.time_ms <= forecast.issued_ms
                || p.place.is_some_and(|v| !valid_place(v))
                || p.altitude_ft.is_some_and(|v| {
                    !v.is_finite() || !(MIN_ALTITUDE_FT..=MAX_ALTITUDE_FT).contains(&v)
                })
                || p.altitude_ft.is_some() != (p.vertical_status == VerticalStatus::Available)
        })
        || forecast
            .meter_eta
            .is_some_and(|eta| eta.time_ms <= 0 || !valid_place(eta.fix.place))
        || forecast
            .meter_fix
            .is_some_and(|fix| !valid_place(fix.place))
        || forecast
            .meter_eta
            .is_some_and(|eta| Some(fix_identity(eta.fix)) != forecast.meter_fix.map(fix_identity))
    {
        return Err(ScoreRefusal::InvalidForecast);
    }
    if meter_fix.is_some_and(|fix| !valid_place(fix.place)) {
        return Err(ScoreRefusal::InvalidMeterFix);
    }
    if forecast.meter_fix.map(fix_identity) != meter_fix.map(fix_identity) {
        return Err(ScoreRefusal::MeterFixMismatch);
    }
    let truth = prepare_truth(truth)?;
    let mut out = ArrivalScore {
        forecast_possible: 3,
        ..ArrivalScore::default()
    };
    for (slot, predicted) in out.forecast.iter_mut().zip(&forecast.positions) {
        if predicted.time_ms <= 0 {
            continue;
        }
        let actual = interpolate_truth(&truth, predicted.time_ms);
        slot.prediction_available = predicted.place.is_some();
        slot.truth_available = actual.is_some();
        slot.truth_altitude_available = actual.is_some_and(|point| point.altitude_ft.is_some());
        if let (Some(predicted_place), Some(actual)) = (predicted.place, actual) {
            slot.horizontal_error_m = Some(great_circle_m(predicted_place, actual.place));
            slot.vertical_error_ft = predicted
                .altitude_ft
                .zip(actual.altitude_ft)
                .map(|(a, b)| (a - b).abs());
            slot.covered = true;
            out.forecast_covered += 1;
        }
    }
    let actual_meter = meter_fix
        .and_then(|fix| actual_passage(&truth, forecast.issued_ms, fix, meter_tolerance_m));
    out.actual_meter_ms = actual_meter;
    if let Some(actual) = actual_meter {
        out.oa_eta_error_s = forecast
            .meter_eta
            .and_then(|eta| eta.time_ms.checked_sub(actual))
            .map(|delta| delta as f64 / 1000.0);
        out.eram_eta_error_s = comparator_error(eram, forecast.cutoff_ms, actual);
        out.tb_fm_eta_error_s = comparator_error(tb_fm, forecast.cutoff_ms, actual);
    }
    Ok(out)
}

fn comparator_error(estimate: Option<LegacyEta>, issue_ms: i64, actual_ms: i64) -> Option<f64> {
    estimate
        .filter(|e| {
            e.issued_ms > 0 && e.eta_ms > 0 && e.issued_ms <= issue_ms && e.eta_ms >= e.issued_ms
        })
        .and_then(|e| e.eta_ms.checked_sub(actual_ms))
        .map(|delta| delta as f64 / 1000.0)
}

fn prepare_truth(truth: &[TruthPoint]) -> Result<Vec<TruthPoint>, ScoreRefusal> {
    let mut ordered = truth.to_vec();
    if ordered.iter().any(|p| {
        p.time_ms <= 0
            || !valid_place(p.place)
            || p.altitude_ft.is_some_and(|v| {
                !v.is_finite() || !(MIN_ALTITUDE_FT..=MAX_ALTITUDE_FT).contains(&v)
            })
    }) {
        return Err(ScoreRefusal::InvalidTruthPoint);
    }
    ordered.sort_by_key(|p| p.time_ms);
    let mut unique: Vec<TruthPoint> = Vec::new();
    for p in ordered {
        if let Some(previous) = unique.last_mut() {
            if previous.time_ms == p.time_ms {
                if previous.place != p.place || previous.altitude_ft != p.altitude_ft {
                    return Err(ScoreRefusal::ConflictingTruthTimestamp);
                }
                previous.gap_before |= p.gap_before;
                continue;
            }
        }
        unique.push(p);
    }
    Ok(unique)
}

fn interpolate_truth(truth: &[TruthPoint], time_ms: i64) -> Option<TruthPoint> {
    if time_ms <= 0 {
        return None;
    }
    if let Some(exact) = truth.iter().find(|p| p.time_ms == time_ms) {
        return Some(*exact);
    }
    let (a, b) = truth.windows(2).find_map(|w| {
        let (a, b) = w.first().zip(w.get(1))?;
        (a.time_ms < time_ms
            && time_ms < b.time_ms
            && !b.gap_before
            && b.time_ms - a.time_ms <= MAX_TRUTH_GAP_MS)
            .then_some((*a, *b))
    })?;
    let fraction = (time_ms - a.time_ms) as f64 / (b.time_ms - a.time_ms) as f64;
    let place = Place {
        lat_deg: a.place.lat_deg + (b.place.lat_deg - a.place.lat_deg) * fraction,
        lon_deg: a.place.lon_deg + (b.place.lon_deg - a.place.lon_deg) * fraction,
    };
    let altitude_ft = a
        .altitude_ft
        .zip(b.altitude_ft)
        .map(|(x, y)| x + (y - x) * fraction);
    Some(TruthPoint {
        time_ms,
        place,
        altitude_ft,
        gap_before: false,
    })
}

fn actual_passage(
    truth: &[TruthPoint],
    issue_ms: i64,
    fix: RouteFix,
    tolerance_m: f64,
) -> Option<i64> {
    if issue_ms <= 0 || tolerance_m <= 0.0 {
        return None;
    }
    let mut path = Vec::with_capacity(truth.len());
    for (i, p) in truth.iter().enumerate() {
        let large_gap = i > 0
            && p.time_ms
                .checked_sub(truth.get(i - 1)?.time_ms)
                .is_none_or(|d| d > MAX_TRUTH_GAP_MS);
        path.push(PathPoint {
            t_ms: p.time_ms,
            lat_deg: p.place.lat_deg,
            lon_deg: p.place.lon_deg,
            gap_before: i > 0 && (p.gap_before || large_gap),
        });
    }
    let result = approach(&path, fix.place, tolerance_m);
    result
        .passages
        .iter()
        .find(|p| p.t_ms > issue_ms && p.dist_m <= tolerance_m && !p.at_edge)
        .map(|p| p.t_ms)
}

#[derive(Clone, Copy)]
struct VerticalForecast {
    altitude_ft: Option<f64>,
    status: VerticalStatus,
}
// A7.2: one-shot forecast helper, size guard waived.

fn vertical_at(
    track: TrackState,
    assigned: Option<f64>,
    assigned_provenance: Option<ConstraintProvenance>,
    route: &[RouteFix],
    speed: f64,
    seconds: f64,
) -> VerticalForecast {
    let Some(current) = track.altitude_ft.filter(|v| v.is_finite()) else {
        return VerticalForecast {
            altitude_ft: None,
            status: VerticalStatus::MissingAltitude,
        };
    };
    let Some(rate) = track.vertical_rate_fpm.filter(|v| v.is_finite()) else {
        return VerticalForecast {
            altitude_ft: None,
            status: VerticalStatus::MissingVerticalRate,
        };
    };
    let altitude_at_seconds = |elapsed: f64| {
        let raw = current + rate * elapsed / 60.0;
        assigned.filter(|v| v.is_finite()).map_or(raw, |target| {
            if rate > 0.0 && target >= current {
                oa_math::min_f64(raw, target)
            } else if rate < 0.0 && target <= current {
                oa_math::max_f64(raw, target)
            } else {
                raw
            }
        })
    };
    let mut distance = 0.0;
    let mut here = track.place;
    let traveled = speed * seconds;
    for fix in route {
        distance += great_circle_m(here, fix.place);
        if distance > traveled {
            break;
        }
        if let Some(constraint) = fix.altitude_constraint {
            let crossing_seconds = distance / speed;
            let altitude = altitude_at_seconds(crossing_seconds);
            let within_window = altitude >= constraint.minimum_ft.unwrap_or(MIN_ALTITUDE_FT)
                && altitude <= constraint.maximum_ft.unwrap_or(MAX_ALTITUDE_FT);
            if !within_window {
                return VerticalForecast {
                    altitude_ft: None,
                    status: VerticalStatus::ConstraintConflict {
                        constraint: constraint.provenance,
                        assigned_altitude: assigned
                            .filter(|_| assigned_provenance.is_some())
                            .and(assigned_provenance),
                    },
                };
            }
        }
        here = fix.place;
    }
    let altitude = altitude_at_seconds(seconds);
    if !(MIN_ALTITUDE_FT..=MAX_ALTITUDE_FT).contains(&altitude) {
        VerticalForecast {
            altitude_ft: None,
            status: VerticalStatus::OutsideModelRange,
        }
    } else {
        VerticalForecast {
            altitude_ft: Some(altitude),
            status: VerticalStatus::Available,
        }
    }
}

fn route_position(start: Place, speed: f64, route: &[RouteFix], seconds: f64) -> Option<Place> {
    let mut here = start;
    let mut remaining = speed * seconds;
    for (idx, fix) in route.iter().enumerate() {
        let distance = great_circle_m(here, fix.place);
        if remaining <= distance {
            return Some(interpolate_place(
                here,
                fix.place,
                remaining / if distance > 1.0 { distance } else { 1.0 },
            ));
        }
        remaining -= distance;
        here = fix.place;
        if idx + 1 < route.len() {
            continue;
        }
        return None;
    }
    Some(here)
}

fn route_distance_to(start: Place, route: &[RouteFix], fix: RouteFix) -> Option<f64> {
    let mut here = start;
    let mut sum = 0.0;
    let mut found = false;
    for p in route {
        sum += great_circle_m(here, p.place);
        here = p.place;
        if fix_identity(*p) == fix_identity(fix) {
            found = true;
            break;
        }
    }
    if !found {
        return None;
    }
    Some(sum)
}

fn valid_place(p: Place) -> bool {
    p.lat_deg.is_finite()
        && p.lon_deg.is_finite()
        && (-90.0..=90.0).contains(&p.lat_deg)
        && (-180.0..=180.0).contains(&p.lon_deg)
}
fn interpolate_place(a: Place, b: Place, f: f64) -> Place {
    Place {
        lat_deg: a.lat_deg + (b.lat_deg - a.lat_deg) * f,
        lon_deg: a.lon_deg + (b.lon_deg - a.lon_deg) * f,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (IssueSnapshot, RouteFix) {
        let fix = RouteFix {
            name: *b"METER   ",
            place: Place {
                lat_deg: 41.7,
                lon_deg: -87.9,
            },
            altitude_constraint: None,
        };
        let snapshot = IssueSnapshot {
            cutoff_ms: 1_000_000,
            issued_ms: 1_000_000,
            track: TrackState {
                time_ms: 1_000_000,
                place: Place {
                    lat_deg: 41.3,
                    lon_deg: -88.4,
                },
                altitude_ft: None,
                speed_mps: Some(180.0),
                vertical_rate_fpm: None,
            },
            assigned_altitude_ft: None,
            assigned_altitude_provenance: None,
            route: vec![fix],
            meter_fix: Some(fix),
        };
        (snapshot, fix)
    }

    // REQ: OA-LTJ-054
    #[test]
    fn cutoff_snapshot_and_absent_altitude_keep_metrics_absent() -> Result<(), String> {
        let (snapshot, fix) = fixture();
        let predicted = forecast(&snapshot).map_err(|error| format!("forecast: {error:?}"))?;
        assert_eq!(
            predicted.positions.map(|p| p.time_ms),
            [1_300_000, 1_600_000, 2_200_000]
        );
        let truth = [
            TruthPoint {
                time_ms: 1_290_000,
                place: Place {
                    lon_deg: fix.place.lon_deg - 0.10,
                    ..fix.place
                },
                altitude_ft: None,
                gap_before: false,
            },
            TruthPoint {
                time_ms: 1_300_000,
                place: predicted.positions[0].place.ok_or("missing 5-minute fix")?,
                altitude_ft: None,
                gap_before: false,
            },
            TruthPoint {
                time_ms: 1_330_000,
                place: Place {
                    lon_deg: fix.place.lon_deg - 0.02,
                    ..fix.place
                },
                altitude_ft: None,
                gap_before: false,
            },
            TruthPoint {
                time_ms: 1_340_000,
                place: fix.place,
                altitude_ft: None,
                gap_before: false,
            },
            TruthPoint {
                time_ms: 1_350_000,
                place: Place {
                    lon_deg: fix.place.lon_deg + 0.02,
                    ..fix.place
                },
                altitude_ft: None,
                gap_before: false,
            },
            TruthPoint {
                time_ms: 1_360_000,
                place: Place {
                    lon_deg: fix.place.lon_deg + 0.04,
                    ..fix.place
                },
                altitude_ft: None,
                gap_before: false,
            },
        ];
        let score = score(
            &predicted,
            &truth,
            Some(fix),
            Some(LegacyEta {
                issued_ms: 1_000_001,
                eta_ms: 1_200_000,
            }),
            Some(LegacyEta {
                issued_ms: 999_000,
                eta_ms: 1_350_000,
            }),
            500.0,
        )
        .map_err(|error| format!("score: {error:?}"))?;
        assert_eq!(predicted.issued_ms, snapshot.cutoff_ms);
        assert!(score.forecast[0].covered);
        assert_eq!(score.forecast[0].vertical_error_ft, None);
        assert_eq!(score.forecast[1].horizontal_error_m, None);
        assert_eq!(score.actual_meter_ms, Some(1_340_000));
        assert_eq!(score.eram_eta_error_s, None);
        assert_eq!(score.tb_fm_eta_error_s, Some(10.0));
        assert_eq!(score.forecast_covered, 1);
        Ok(())
    }

    // REQ: OA-LTJ-054
    #[test]
    fn future_or_stale_track_state_is_not_forecast() {
        let (mut snapshot, _) = fixture();
        snapshot.track.time_ms = snapshot.cutoff_ms + 1;
        assert_eq!(forecast(&snapshot), Err(ForecastRefusal::TrackAfterCutoff));
        snapshot.track.time_ms = snapshot.cutoff_ms - 120_001;
        assert_eq!(forecast(&snapshot), Err(ForecastRefusal::TrackTooStale));
    }

    // REQ: OA-LTJ-054
    #[test]
    fn first_valid_meter_passage_wins_and_a_truth_gap_is_never_bridged() {
        let fix = RouteFix {
            name: *b"METER   ",
            place: Place {
                lat_deg: 41.0,
                lon_deg: -87.0,
            },
            altitude_constraint: None,
        };
        let samples = [
            (-87.5, 1_000),
            (-87.4, 6_000),
            (-87.3, 11_000),
            (-87.2, 16_000),
            (-87.1, 21_000),
            (-87.0, 26_000),
            (-86.9, 31_000),
            (-86.5, 36_000),
            (-87.5, 41_000),
            (-87.0, 46_000),
            (-86.8, 51_000),
        ];
        let truth = samples
            .iter()
            .map(|(lon_deg, time_ms)| TruthPoint {
                time_ms: *time_ms,
                place: Place {
                    lat_deg: 41.0,
                    lon_deg: *lon_deg,
                },
                altitude_ft: None,
                gap_before: false,
            })
            .collect::<Vec<_>>();
        assert_eq!(actual_passage(&truth, 20_000, fix, 1_000.0), Some(26_000));

        let across_hole = [
            TruthPoint {
                time_ms: 10_000,
                place: Place {
                    lat_deg: 41.0,
                    lon_deg: -87.1,
                },
                altitude_ft: None,
                gap_before: false,
            },
            TruthPoint {
                time_ms: 20_000,
                place: Place {
                    lat_deg: 41.0,
                    lon_deg: -86.9,
                },
                altitude_ft: None,
                gap_before: true,
            },
        ];
        assert_eq!(actual_passage(&across_hole, 5_000, fix, 1_000.0), None);
    }

    // REQ: OA-LTJ-054
    #[test]
    fn duplicate_conflict_and_nonpositive_realization_are_refused() -> Result<(), ForecastRefusal> {
        let (snapshot, fix) = fixture();
        let mut predicted = forecast(&snapshot)?;
        predicted.positions[0].time_ms = 0;
        assert_eq!(
            score(&predicted, &[], None, None, None, 1_000.0),
            Err(ScoreRefusal::InvalidForecast)
        );
        predicted = forecast(&snapshot)?;
        let p = Place {
            lat_deg: 41.5,
            lon_deg: -88.0,
        };
        let duplicate = [
            TruthPoint {
                time_ms: 1_600_000,
                place: p,
                altitude_ft: Some(1_000.0),
                gap_before: false,
            },
            TruthPoint {
                time_ms: 1_600_000,
                place: Place {
                    lat_deg: 41.6,
                    lon_deg: -88.0,
                },
                altitude_ft: Some(1_000.0),
                gap_before: false,
            },
        ];
        assert_eq!(
            score(&predicted, &duplicate, Some(fix), None, None, 1_000.0),
            Err(ScoreRefusal::ConflictingTruthTimestamp)
        );
        Ok(())
    }

    // REQ: OA-LTJ-054
    #[test]
    fn route_end_and_empty_route_are_absent_or_refused() -> Result<(), ForecastRefusal> {
        let (mut snapshot, _) = fixture();
        let result = forecast(&snapshot)?;
        assert_eq!(result.positions[2].place, None);
        snapshot.route.clear();
        assert_eq!(forecast(&snapshot), Err(ForecastRefusal::EmptyRoute));
        Ok(())
    }

    // REQ: OA-LTJ-054
    #[test]
    fn horizon_and_eta_elapsed_time_start_at_track_observation() -> Result<(), String> {
        let (mut aged, _) = fixture();
        aged.track.time_ms = 995_000;
        aged.track.altitude_ft = Some(12_000.0);
        aged.track.vertical_rate_fpm = Some(-600.0);
        aged.assigned_altitude_ft = Some(5_000.0);
        let aged_forecast = forecast(&aged).map_err(|error| format!("aged forecast: {error:?}"))?;
        assert_eq!(aged_forecast.positions[0].altitude_ft, Some(8_950.0));

        let (baseline, _) = fixture();
        let baseline_forecast =
            forecast(&baseline).map_err(|error| format!("baseline forecast: {error:?}"))?;
        let horizontal_shift = great_circle_m(
            aged_forecast.positions[0].place.ok_or("missing aged fix")?,
            baseline_forecast.positions[0]
                .place
                .ok_or("missing baseline fix")?,
        );
        assert!((horizontal_shift - 900.0).abs() < 10.0);
        let elapsed_shift_ms = aged_forecast
            .meter_eta
            .ok_or("missing aged meter ETA")?
            .time_ms
            - baseline_forecast
                .meter_eta
                .ok_or("missing baseline meter ETA")?
                .time_ms;
        assert_eq!(elapsed_shift_ms, -5_000);
        Ok(())
    }

    // REQ: OA-LTJ-054
    #[test]
    fn elapsed_meter_crossing_before_issue_has_no_eta_and_score_rejects_other_fix(
    ) -> Result<(), ForecastRefusal> {
        let (mut snapshot, fix) = fixture();
        snapshot.track.time_ms = snapshot.issued_ms - MAX_TRACK_AGE_MS;
        snapshot.track.speed_mps = Some(180.0);
        let near = RouteFix {
            name: *b"NEAR    ",
            place: Place {
                lat_deg: 41.3,
                lon_deg: -88.39,
            },
            altitude_constraint: None,
        };
        snapshot.route = vec![near];
        snapshot.meter_fix = Some(near);
        let passed_fix = forecast(&snapshot)?;
        assert_eq!(passed_fix.meter_eta, None);

        let (snapshot, _) = fixture();
        let forecast = forecast(&snapshot)?;
        let other = RouteFix {
            name: *b"OTHER   ",
            place: Place {
                lat_deg: 41.6,
                lon_deg: -88.1,
            },
            altitude_constraint: None,
        };
        assert_eq!(
            score(&forecast, &[], Some(other), None, None, 1_000.0),
            Err(ScoreRefusal::MeterFixMismatch)
        );
        assert_eq!(forecast.meter_fix, Some(fix));
        let same_fix_with_constraint = RouteFix {
            altitude_constraint: Some(AltitudeConstraint {
                minimum_ft: Some(9_000.0),
                maximum_ft: None,
                available_ms: snapshot.cutoff_ms,
                provenance: ConstraintProvenance::PublishedProcedure,
            }),
            ..fix
        };
        assert!(score(
            &forecast,
            &[],
            Some(same_fix_with_constraint),
            None,
            None,
            1_000.0,
        )
        .is_ok());
        Ok(())
    }

    // REQ: OA-LTJ-054
    #[test]
    fn compatible_waypoint_window_checks_crossing_without_changing_the_profile(
    ) -> Result<(), ForecastRefusal> {
        let (mut snapshot, _) = fixture();
        snapshot.track.altitude_ft = Some(10_000.0);
        snapshot.track.speed_mps = Some(300.0);
        snapshot.track.vertical_rate_fpm = Some(-600.0);
        let constraint = AltitudeConstraint {
            minimum_ft: Some(7_000.0),
            maximum_ft: Some(8_500.0),
            available_ms: snapshot.cutoff_ms,
            provenance: ConstraintProvenance::PublishedProcedure,
        };
        snapshot.route[0].altitude_constraint = Some(constraint);
        let predicted = forecast(&snapshot)?;
        assert_eq!(predicted.positions[0].altitude_ft, Some(7_000.0));
        assert_eq!(
            predicted.positions[0].vertical_status,
            VerticalStatus::Available
        );
        snapshot.route[0].altitude_constraint = Some(AltitudeConstraint {
            available_ms: snapshot.cutoff_ms + 1,
            ..constraint
        });
        assert_eq!(forecast(&snapshot), Err(ForecastRefusal::InvalidTrackValue));
        Ok(())
    }

    // REQ: OA-LTJ-054
    #[test]
    fn infeasible_waypoint_window_marks_vertical_only_unavailable() -> Result<(), String> {
        let (mut snapshot, _) = fixture();
        snapshot.track.altitude_ft = Some(10_000.0);
        snapshot.track.speed_mps = Some(180.0);
        snapshot.track.vertical_rate_fpm = Some(-600.0);
        snapshot.route[0].altitude_constraint = Some(AltitudeConstraint {
            minimum_ft: Some(9_000.0),
            maximum_ft: Some(11_000.0),
            available_ms: snapshot.cutoff_ms,
            provenance: ConstraintProvenance::Synthetic,
        });
        snapshot.route.push(RouteFix {
            name: *b"AFTER   ",
            place: Place {
                lat_deg: 41.7,
                lon_deg: -86.8,
            },
            altitude_constraint: None,
        });
        let predicted = forecast(&snapshot).map_err(|e| format!("forecast: {e:?}"))?;
        assert!(predicted.positions[0].place.is_some());
        assert_eq!(predicted.positions[0].altitude_ft, Some(7_000.0));
        assert_eq!(
            predicted.positions[0].vertical_status,
            VerticalStatus::Available
        );
        assert!(predicted.positions[1].place.is_some());
        assert_eq!(predicted.positions[1].altitude_ft, None);
        assert_eq!(
            predicted.positions[1].vertical_status,
            VerticalStatus::ConstraintConflict {
                constraint: ConstraintProvenance::Synthetic,
                assigned_altitude: None,
            }
        );
        let actual = TruthPoint {
            time_ms: predicted.positions[1].time_ms,
            place: predicted.positions[1]
                .place
                .ok_or("missing 10-minute position")?,
            altitude_ft: None,
            gap_before: false,
        };
        let scored = score(
            &predicted,
            &[actual],
            predicted.meter_fix,
            None,
            None,
            2_000.0,
        )
        .map_err(|e| format!("score: {e:?}"))?;
        assert!(scored.forecast[1].horizontal_error_m.is_some());
        assert!(scored.forecast[1].truth_available);
        assert!(!scored.forecast[1].truth_altitude_available);
        assert_eq!(scored.forecast[1].vertical_error_ft, None);
        Ok(())
    }

    // REQ: OA-LTJ-054
    #[test]
    fn missing_vertical_rate_and_assigned_clearance_conflict_are_explicit(
    ) -> Result<(), ForecastRefusal> {
        let (mut missing_rate, _) = fixture();
        let missing_altitude = forecast(&missing_rate)?;
        assert_eq!(missing_altitude.positions[0].altitude_ft, None);
        assert_eq!(
            missing_altitude.positions[0].vertical_status,
            VerticalStatus::MissingAltitude
        );
        missing_rate.track.altitude_ft = Some(10_000.0);
        let missing = forecast(&missing_rate)?;
        assert!(missing.positions[0].place.is_some());
        assert_eq!(missing.positions[0].altitude_ft, None);
        assert_eq!(
            missing.positions[0].vertical_status,
            VerticalStatus::MissingVerticalRate
        );

        let (mut conflict, _) = fixture();
        conflict.track.altitude_ft = Some(10_000.0);
        conflict.track.speed_mps = Some(180.0);
        conflict.track.vertical_rate_fpm = Some(-600.0);
        conflict.assigned_altitude_ft = Some(9_000.0);
        conflict.assigned_altitude_provenance = Some(ConstraintProvenance::SfdpsAssignedAltitude);
        conflict.route[0].altitude_constraint = Some(AltitudeConstraint {
            minimum_ft: Some(9_500.0),
            maximum_ft: Some(11_000.0),
            available_ms: conflict.cutoff_ms,
            provenance: ConstraintProvenance::PublishedProcedure,
        });
        conflict.route.push(RouteFix {
            name: *b"AFTER   ",
            place: Place {
                lat_deg: 41.7,
                lon_deg: -86.8,
            },
            altitude_constraint: None,
        });
        let predicted = forecast(&conflict)?;
        assert!(predicted.positions[0].place.is_some());
        assert_eq!(predicted.positions[0].altitude_ft, Some(9_000.0));
        assert_eq!(
            predicted.positions[0].vertical_status,
            VerticalStatus::Available
        );
        assert!(predicted.positions[1].place.is_some());
        assert_eq!(predicted.positions[1].altitude_ft, None);
        assert_eq!(
            predicted.positions[1].vertical_status,
            VerticalStatus::ConstraintConflict {
                constraint: ConstraintProvenance::PublishedProcedure,
                assigned_altitude: Some(ConstraintProvenance::SfdpsAssignedAltitude),
            }
        );
        Ok(())
    }

    // REQ: OA-LTJ-054
    #[test]
    fn an_out_of_range_altitude_does_not_erase_horizontal_coverage() -> Result<(), ForecastRefusal>
    {
        let (mut snapshot, _) = fixture();
        snapshot.track.altitude_ft = Some(1_000.0);
        snapshot.track.vertical_rate_fpm = Some(-2_500.0);
        snapshot.assigned_altitude_ft = None;
        let forecast = forecast(&snapshot)?;
        assert!(forecast.positions[0].place.is_some());
        assert_eq!(forecast.positions[0].altitude_ft, None);
        assert_eq!(
            forecast.positions[0].vertical_status,
            VerticalStatus::OutsideModelRange
        );
        Ok(())
    }
}
