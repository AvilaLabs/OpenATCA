//! Offline arrival experiment (shell, B3/C6). Binds recording rows at receipt time and passes
//! normalized issue inputs and separately sampled truth to the pure arrival core.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

use oa_adsb::{Reports, F_GROUND, F_RATE_GEOM, NONE_I32, NONE_U16};
use oa_bench::adsb_truth::{query, BuiltTruth};
use oa_bench::assignment::{
    assignment_at_issue, AssignmentEvidence, AssignmentIssue, AssignmentQuery,
};
use oa_bench::geo::{dist_m, enu, heading_deg, wrap180};
#[cfg(test)]
use oa_bench::legacy::{assigned_altitude_history_overflowed, MAX_ASSIGNED_ALTITUDE_UPDATES};
use oa_bench::legacy::{
    name, norm_id, tbfm_kind, Legacy, ALT_FORM_AT_FIX, ALT_FORM_BLOCK, ALT_FORM_INVALID,
    ALT_FORM_SIMPLE, ALT_FORM_VFR, ALT_FORM_VFR_PLUS, NONE, NO_ALT, NO_COORD, NO_TIME,
};
use oa_bench::stats::nearest_rank;

use crate::core::arrival::{
    forecast, score, ArrivalScore, IssueSnapshot, LegacyEta, RouteFix, TrackState, TruthPoint,
    VerticalStatus,
};
use crate::core::types::Place;
use crate::shell::places::{place_of_e7, point_places, PlaceTable};

// C7: headroom over the two regional ADS-B days and the one day of route/TBFM recording in the
// development baseline. Check the volume before indexing and check keyed limits before inserting.
pub(crate) const MAX_REPORTS: usize = 12_000_000;
const MAX_CAPTURED_ROUTES: usize = 500_000;
const MAX_CAPTURED_TBFM: usize = 5_000_000;
#[cfg(test)]
const MAX_CAPTURED_ASSIGNED_ALTITUDES: usize = MAX_ASSIGNED_ALTITUDE_UPDATES;
const MAX_METER_PLAN_PAIRS: usize = 500_000;
const MAX_ESTIMATES_FOR_FIX: usize = 500_000;
const MAX_ISSUES: usize = 12_000;
const MAX_TRACK_AGE_MS: i64 = 30_000;
const TRUTH_STEP_MS: i64 = 5_000;
const TRUTH_STEPS: i64 = 720;
const MIN_FROM_FIX_M: f64 = 40.0 * 1852.0;
const MAX_FROM_FIX_M: f64 = 200.0 * 1852.0;

#[derive(Clone, Copy)]
struct RouteIx(u32);

#[derive(Clone, Copy)]
struct ReportIx(u32);

#[cfg(test)]
#[derive(Clone, Copy)]
struct AssignedAltitudeIx(u32);

#[cfg(test)]
#[derive(Clone, Copy)]
struct UnknownAssignedAltitudeIx(u32);

// A7.2: one offline join record per received TBFM estimate for the selected fix.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct EtaRow {
    received_ms: i64,
    published_ms: i64,
    eta_ms: i64,
}

type Plan = [u8; 12];

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AssignedAltitudeInput {
    Available { value_ft: i32, available_ms: i64 },
    Missing,
    UnsupportedForm { form: u8, available_ms: i64 },
    SameTimeConflict { available_ms: i64 },
    MalformedForm { available_ms: i64 },
    InvalidSimpleValue { available_ms: i64 },
    UnknownAvailability { source_ms: i64 },
    HistoryOverflow,
}

#[cfg(test)]
#[derive(Default)]
struct AssignedAltitudeIndices {
    known: BTreeMap<u32, Vec<AssignedAltitudeIx>>,
    unknown: BTreeMap<u32, Vec<UnknownAssignedAltitudeIx>>,
}

#[cfg(test)]
fn assigned_altitude_index(
    legacy: &Legacy,
    selected_flights: &BTreeSet<u32>,
) -> Result<AssignedAltitudeIndices, String> {
    let table = &legacy.assigned_altitudes;
    if table.ts_ms.len() != table.flight.len()
        || table.recv_ms.len() != table.flight.len()
        || table.form.len() != table.flight.len()
        || table.value_ft.len() != table.flight.len()
    {
        return Err("inconsistent assigned-altitude columns".into());
    }
    let unknown = &legacy.unknown_assigned_altitudes;
    if unknown.source_ms.len() != unknown.flight.len()
        || unknown.form.len() != unknown.flight.len()
        || unknown.value_ft.len() != unknown.flight.len()
    {
        return Err("inconsistent unknown assigned-altitude columns".into());
    }
    let mut index = AssignedAltitudeIndices::default();
    for (i, &flight) in table.flight.iter().enumerate() {
        if !selected_flights.contains(&flight) {
            continue;
        }
        let rows = index.known.entry(flight).or_default();
        if rows.len() == usize::from(oa_bench::legacy::MAX_ASSIGNED_ALTITUDE_UPDATES_PER_FLIGHT) {
            return Err("assigned-altitude history exceeds its per-flight bound".into());
        }
        rows.push(AssignedAltitudeIx(
            u32::try_from(i).map_err(|_| "assigned-altitude index exceeds u32")?,
        ));
    }
    for (i, &flight) in unknown.flight.iter().enumerate() {
        if !selected_flights.contains(&flight) {
            continue;
        }
        let rows = index.unknown.entry(flight).or_default();
        if rows.len() == usize::from(oa_bench::legacy::MAX_ASSIGNED_ALTITUDE_UPDATES_PER_FLIGHT) {
            return Err("unknown assigned-altitude history exceeds its per-flight bound".into());
        }
        rows.push(UnknownAssignedAltitudeIx(
            u32::try_from(i).map_err(|_| "unknown assigned-altitude index exceeds u32")?,
        ));
    }
    Ok(index)
}

#[cfg(test)]
fn assigned_altitude_at(
    legacy: &Legacy,
    index: &AssignedAltitudeIndices,
    flight: u32,
    cutoff_ms: i64,
) -> AssignedAltitudeInput {
    if assigned_altitude_history_overflowed(legacy, flight, cutoff_ms) {
        return AssignedAltitudeInput::HistoryOverflow;
    }
    let table = &legacy.assigned_altitudes;
    let rows = index.known.get(&flight).map_or(&[][..], Vec::as_slice);
    let unknown_rows = index.unknown.get(&flight).map_or(&[][..], Vec::as_slice);
    let mut latest: Option<(i64, i64, u8, i32)> = None;
    let mut conflicting = false;
    for &AssignedAltitudeIx(i) in rows {
        let i = i as usize;
        let (Some(&stamped_ms), Some(&available_ms), Some(&form), Some(&value_ft)) = (
            table.ts_ms.get(i),
            table.recv_ms.get(i),
            table.form.get(i),
            table.value_ft.get(i),
        ) else {
            continue;
        };
        if stamped_ms <= 0
            || stamped_ms > available_ms
            || available_ms <= 0
            || available_ms > cutoff_ms
        {
            continue;
        }
        match latest {
            None => latest = Some((stamped_ms, available_ms, form, value_ft)),
            Some((time, _, _, _)) if stamped_ms > time => {
                latest = Some((stamped_ms, available_ms, form, value_ft));
                conflicting = false;
            }
            Some((time, previous_available, previous_form, previous_value))
                if stamped_ms == time =>
            {
                conflicting |= form != previous_form || value_ft != previous_value;
                if !conflicting && available_ms < previous_available {
                    latest = Some((stamped_ms, available_ms, form, value_ft));
                }
            }
            _ => {}
        }
    }
    let Some((source_ms, available_ms, form, value_ft)) = latest else {
        if let Some(source_ms) =
            unknown_assignment_could_change(legacy, unknown_rows, cutoff_ms, None)
        {
            return AssignedAltitudeInput::UnknownAvailability { source_ms };
        }
        return AssignedAltitudeInput::Missing;
    };
    if let Some(source_ms) = unknown_assignment_could_change(
        legacy,
        unknown_rows,
        cutoff_ms,
        Some((source_ms, form, value_ft)),
    ) {
        return AssignedAltitudeInput::UnknownAvailability { source_ms };
    }
    if conflicting {
        return AssignedAltitudeInput::SameTimeConflict { available_ms };
    }
    if form == ALT_FORM_INVALID {
        return AssignedAltitudeInput::MalformedForm { available_ms };
    }
    if form != ALT_FORM_SIMPLE {
        return AssignedAltitudeInput::UnsupportedForm { form, available_ms };
    }
    if value_ft == NO_ALT || !(-2_000..=100_000).contains(&value_ft) {
        return AssignedAltitudeInput::InvalidSimpleValue { available_ms };
    }
    AssignedAltitudeInput::Available {
        value_ft,
        available_ms,
    }
}

#[cfg(test)]
fn unknown_assignment_could_change(
    legacy: &Legacy,
    rows: &[UnknownAssignedAltitudeIx],
    cutoff_ms: i64,
    selected: Option<(i64, u8, i32)>,
) -> Option<i64> {
    let table = &legacy.unknown_assigned_altitudes;
    let mut earliest = None;
    for &UnknownAssignedAltitudeIx(i) in rows {
        let i = i as usize;
        let (Some(&source_ms), Some(&form), Some(&value_ft)) = (
            table.source_ms.get(i),
            table.form.get(i),
            table.value_ft.get(i),
        ) else {
            continue;
        };
        if source_ms > 0 && source_ms > cutoff_ms {
            continue;
        }
        if let Some((selected_source, selected_form, selected_value)) = selected {
            if source_ms > 0 && selected_source > source_ms {
                continue;
            }
            if source_ms > 0
                && selected_source == source_ms
                && selected_form == form
                && selected_value == value_ft
            {
                continue;
            }
        }
        earliest = Some(earliest.map_or(source_ms, |previous: i64| previous.min(source_ms)));
    }
    earliest
}

fn route_range(legacy: &Legacy, ix: RouteIx) -> Option<std::ops::Range<usize>> {
    let row = ix.0 as usize;
    let begin = *legacy.routes.first.get(row)? as usize;
    let end = begin.checked_add(*legacy.routes.len.get(row)? as usize)?;
    legacy.route_points.fix.get(begin..end)?;
    Some(begin..end)
}

fn route_fix(legacy: &Legacy, places: &PlaceTable, i: usize) -> Option<RouteFix> {
    let fix = norm_id::<8>(legacy.route_points.fix.get(i)?);
    let lat = *legacy.route_points.lat_e7.get(i)?;
    let lon = *legacy.route_points.lon_e7.get(i)?;
    let place = match (lat, lon, point_places(places, name(&fix))) {
        (lat, lon, _) if lat != NO_COORD && lon != NO_COORD => place_of_e7(lat, lon),
        (_, _, [place]) => *place,
        _ => return None,
    };
    Some(RouteFix {
        name: fix,
        place,
        altitude_constraint: None,
    })
}

fn route_rows(legacy: &Legacy) -> impl Iterator<Item = RouteIx> + '_ {
    (0..legacy.routes.flight.len()).filter_map(|i| {
        let arrival = name(legacy.routes.arrival_at_receive.get(i)?);
        let recv = *legacy.routes.recv_ms.get(i)?;
        let issue = *legacy.routes.ts_ms.get(i)?;
        (matches!(arrival, "ORD" | "KORD") && issue > 0 && issue <= recv)
            .then_some(RouteIx(i as u32))
    })
}

fn select_meter(requested: &str, legacy: &Legacy, places: &PlaceTable) -> Result<RouteFix, String> {
    let selected = if requested == "AUTO" {
        // Before scoring, count distinct ORD plan/fix pairs present in both captured routes and
        // TBFM estimates. Do not select a fix based on forecast errors or truth availability.
        let mut candidates = BTreeSet::new();
        let mut counts = BTreeMap::new();
        for (i, &kind) in legacy.tbfm.kind.iter().enumerate() {
            if kind != tbfm_kind::ETA_MFX {
                continue;
            }
            let (Some(p), Some(f)) = (legacy.tbfm.plan_id.get(i), legacy.tbfm.fix.get(i)) else {
                continue;
            };
            let key = norm_id::<8>(f);
            let plan = norm_id::<12>(p);
            if plan != [0; 12] && matches!(point_places(places, name(&key)), [_]) {
                let pair = (plan, key);
                if candidates.len() == MAX_METER_PLAN_PAIRS && !candidates.contains(&pair) {
                    return Err("arrival selection exceeds the distinct plan/fix budget".into());
                }
                candidates.insert(pair);
            }
        }
        for row in route_rows(legacy) {
            let Some(p) = legacy.routes.plan_at_receive.get(row.0 as usize) else {
                continue;
            };
            let plan = norm_id::<12>(p);
            if legacy
                .routes
                .addr_at_receive
                .get(row.0 as usize)
                .is_none_or(|&a| a == NONE)
            {
                continue;
            }
            for fix in route_range(legacy, row)
                .into_iter()
                .flatten()
                .filter_map(|i| legacy.route_points.fix.get(i))
            {
                let key = norm_id::<8>(fix);
                if candidates.remove(&(plan, key)) {
                    *counts.entry(key).or_insert(0usize) += 1;
                }
            }
        }
        counts
            .into_iter()
            .max_by_key(|&(key, n)| (n, std::cmp::Reverse(key)))
            .ok_or("no resolvable ORD route fix with a matching TBFM estimate")?
            .0
    } else {
        norm_id::<8>(requested.as_bytes())
    };
    let [place] = point_places(places, name(&selected)) else {
        return Err("no unambiguous place for the selected ORD metering fix".into());
    };
    Ok(RouteFix {
        name: selected,
        place: *place,
        altitude_constraint: None,
    })
}

fn meter_index(legacy: &Legacy, row: RouteIx, meter: RouteFix) -> Option<usize> {
    let mut matches = route_range(legacy, row)?.filter(|&i| {
        legacy
            .route_points
            .fix
            .get(i)
            .is_some_and(|key| norm_id::<8>(key) == meter.name)
    });
    let index = matches.next()?;
    matches.next().is_none().then_some(index)
}

fn track_at(reports: &Reports, rows: &[ReportIx], cutoff: i64) -> Option<(TrackState, f64)> {
    let cutoff_ns = cutoff.checked_mul(1_000_000)?;
    let i = rows.partition_point(|ix| {
        reports
            .time_ns
            .get(ix.0 as usize)
            .is_some_and(|&t| t <= cutoff_ns)
    });
    let picked = rows.get(i.checked_sub(1)?)?.0 as usize;
    let time_ns = *reports.time_ns.get(picked)?;
    // Multiple reports with equal observation time give no unique as-of state.
    if i.checked_sub(2)
        .and_then(|j| rows.get(j))
        .and_then(|ix| reports.time_ns.get(ix.0 as usize))
        == Some(&time_ns)
    {
        return None;
    }
    let time_ms = time_ns / 1_000_000;
    let speed = *reports.gs_dkt.get(picked)?;
    let course = *reports.track_ddeg.get(picked)?;
    let flags = *reports.flags.get(picked)?;
    if cutoff.checked_sub(time_ms)? > MAX_TRACK_AGE_MS
        || flags & F_GROUND != 0
        || speed == NONE_U16
        || course >= 3600
    {
        return None;
    }
    let alt = *reports.baro_alt_ft.get(picked)?;
    let rate = *reports.vrate_fpm.get(picked)?;
    let track = TrackState {
        time_ms,
        place: place_of_e7(*reports.lat_e7.get(picked)?, *reports.lon_e7.get(picked)?),
        altitude_ft: (alt != NONE_I32).then_some(f64::from(alt)),
        speed_mps: Some(f64::from(speed) * (1852.0 / 36_000.0)),
        vertical_rate_fpm: (rate != NONE_I32 && flags & F_RATE_GEOM == 0)
            .then_some(f64::from(rate)),
    };
    Some((track, f64::from(course) / 10.0))
}

fn distance(a: Place, b: Place) -> f64 {
    dist_m(a.lat_deg, a.lon_deg, b.lat_deg, b.lon_deg)
}

fn route_ahead(
    legacy: &Legacy,
    places: &PlaceTable,
    row: RouteIx,
    meter_ix: usize,
    track: TrackState,
    course: f64,
) -> Option<Vec<RouteFix>> {
    let span = route_range(legacy, row)?;
    // Offline baseline: the closest resolved point in the forward half-plane is the next fix.
    // Unresolved points after it truncate coverage instead of bridging missing legs.
    let next = span
        .clone()
        .take_while(|&i| i <= meter_ix)
        .filter_map(|i| {
            let fix = route_fix(legacy, places, i)?;
            let (east, north) = enu(
                track.place.lat_deg,
                track.place.lon_deg,
                fix.place.lat_deg,
                fix.place.lon_deg,
            );
            (wrap180(heading_deg(east, north) - course).abs() <= 90.0)
                .then_some((i, distance(track.place, fix.place)))
        })
        .min_by(|a, b| oa_math::cmp_canon_f64(a.1, b.1))?
        .0;
    let route: Vec<RouteFix> = (next..span.end)
        .map(|i| route_fix(legacy, places, i))
        .map_while(|p| p)
        .collect();
    (meter_ix.checked_sub(next)? < route.len()).then_some(route)
}

fn tbfm_index(legacy: &Legacy, meter: RouteFix) -> Result<BTreeMap<Plan, Vec<EtaRow>>, String> {
    let mut map = BTreeMap::<Plan, Vec<EtaRow>>::new();
    let mut offered = 0usize;
    for (i, &kind) in legacy.tbfm.kind.iter().enumerate() {
        if kind != tbfm_kind::ETA_MFX
            || legacy
                .tbfm
                .fix
                .get(i)
                .is_none_or(|k| norm_id::<8>(k) != meter.name)
        {
            continue;
        }
        let (Some(plan), Some(&received_ms), Some(&published_ms), Some(&eta_ms)) = (
            legacy.tbfm.plan_id.get(i),
            legacy.tbfm.recv_ms.get(i),
            legacy.tbfm.issue_ms.get(i),
            legacy.tbfm.time_ms.get(i),
        ) else {
            continue;
        };
        if published_ms > 0
            && published_ms <= received_ms
            && received_ms < eta_ms
            && plan.iter().any(|&b| b != 0)
        {
            if offered == MAX_ESTIMATES_FOR_FIX {
                return Err("TBFM estimate rows exceed the selected fix budget".into());
            }
            offered += 1;
            map.entry(norm_id(plan)).or_default().push(EtaRow {
                received_ms,
                published_ms,
                eta_ms,
            });
        }
    }
    for estimates in map.values_mut() {
        estimates.sort_unstable();
        estimates.dedup();
    }
    Ok(map)
}

fn estimate_at(rows: &[EtaRow], cutoff: i64) -> Option<LegacyEta> {
    let before = rows.get(..rows.partition_point(|e| e.received_ms <= cutoff))?;
    let &last = before.last()?;
    // Conflicting values for the same receipt and publication are unavailable.
    if before.iter().rev().nth(1).is_some_and(|prev| {
        (prev.received_ms, prev.published_ms) == (last.received_ms, last.published_ms)
    }) {
        return None;
    }
    (cutoff < last.eta_ms).then_some(LegacyEta {
        issued_ms: last.received_ms,
        eta_ms: last.eta_ms,
    })
}

fn truth_path(built: &BuiltTruth, addr: u32, at: i64) -> Vec<TruthPoint> {
    let Ok(start) = query(&built.truth, addr, at) else {
        return Vec::new();
    };
    let mut gap = true;
    (0..=TRUTH_STEPS)
        .filter_map(|k| {
            let time_ms = at.checked_add(k * TRUTH_STEP_MS)?;
            let sample = query(&built.truth, addr, time_ms)
                .ok()
                .filter(|s| s.leg == start.leg);
            let Some(s) = sample else {
                gap = true;
                return None;
            };
            let gap_before = std::mem::replace(&mut gap, false);
            Some(TruthPoint {
                time_ms,
                place: Place {
                    lat_deg: s.lat,
                    lon_deg: s.lon,
                },
                altitude_ft: s.baro_alt_ft.map(f64::from),
                gap_before,
            })
        })
        .collect()
}

// A7.2: one aggregate accumulator per offline experiment, bounded by MAX_ISSUES.
#[derive(Default)]
struct Totals {
    issued: usize,
    crossing: usize,
    forecast_refused: BTreeMap<String, usize>,
    score_refused: BTreeMap<String, usize>,
    ambiguous_routes: usize,
    no_track: usize,
    outside_issue_window: usize,
    unusable_route: usize,
    horizontal: [Vec<f64>; 3],
    vertical: [Vec<f64>; 3],
    coverage: [HorizonCoverage; 3],
    assigned_altitude_states: BTreeMap<String, usize>,
    eta: [Vec<f64>; 3],
    paired_oa_eta: [Vec<f64>; 2],
}

#[derive(Default)]
struct HorizonCoverage {
    no_prediction: usize,
    no_truth_position: usize,
    no_truth_altitude: usize,
    vertical_unavailable: BTreeMap<String, usize>,
}

struct PlannedIssue {
    issue: IssueSnapshot,
    gufi: u128,
    addr: u32,
    route_row: RouteIx,
    meter_ix: usize,
}

/// The fixed no-assignment cohort selected before cutoff-specific assignment evidence is queried.
pub(crate) struct ArrivalPlan {
    requested: String,
    meter: Place,
    offered_flights: usize,
    no_addr: usize,
    no_meter: usize,
    ambiguous_routes: usize,
    no_track: usize,
    outside_issue_window: usize,
    unusable_route: usize,
    baseline_forecast_refused: BTreeMap<String, usize>,
    issues: Vec<PlannedIssue>,
}

impl ArrivalPlan {
    pub(crate) fn is_empty(&self) -> bool {
        self.issues.is_empty()
    }

    pub(crate) fn assignment_issues(&self) -> Vec<AssignmentIssue> {
        self.issues
            .iter()
            .map(|issue| AssignmentIssue {
                gufi: issue.gufi,
                cutoff_ms: issue.issue.cutoff_ms,
            })
            .collect()
    }
}

fn assigned_altitude_label(input: &AssignmentEvidence) -> String {
    match input {
        AssignmentEvidence::Available { .. } => "available simple SFDPS assignment".into(),
        AssignmentEvidence::Missing => "no available assignment".into(),
        AssignmentEvidence::UnsupportedForm { form, .. } => {
            let name = match form {
                ALT_FORM_VFR_PLUS => "vfrPlus",
                ALT_FORM_VFR => "vfr",
                ALT_FORM_BLOCK => "block",
                ALT_FORM_AT_FIX => "altFixAlt",
                _ => "unknown",
            };
            format!("unsupported SFDPS assignedAltitude form {name}")
        }
        AssignmentEvidence::SameTimeConflict { .. } => {
            "same-stamp conflicting SFDPS assignments".into()
        }
        AssignmentEvidence::MalformedForm { .. } => "malformed SFDPS assignment".into(),
        AssignmentEvidence::InvalidSimpleValue { .. } => {
            "invalid simple SFDPS assigned-altitude value".into()
        }
        AssignmentEvidence::UnknownAvailability { .. } => {
            "SFDPS assignment could affect snapshot but receive time is unknown".into()
        }
    }
}

fn apply_assignment(issue: &mut IssueSnapshot, evidence: &AssignmentEvidence) {
    if let AssignmentEvidence::Available {
        value_ft,
        available_ms,
    } = evidence
    {
        issue.assigned_altitude_ft = Some(f64::from(*value_ft));
        issue.assigned_altitude_available_ms = Some(*available_ms);
        issue.assigned_altitude_provenance =
            Some(crate::core::arrival::ConstraintProvenance::SfdpsAssignedAltitude);
    }
}

fn note(t: &mut Totals, s: ArrivalScore) {
    t.crossing += usize::from(s.actual_meter_ms.is_some());
    for (i, result) in s.forecast.into_iter().enumerate() {
        if let Some(values) = t.horizontal.get_mut(i) {
            values.extend(result.horizontal_error_m);
        }
        if let Some(values) = t.vertical.get_mut(i) {
            values.extend(result.vertical_error_ft);
        }
        if let Some(coverage) = t.coverage.get_mut(i) {
            coverage.no_prediction += usize::from(!result.prediction_available);
            coverage.no_truth_position += usize::from(!result.truth_available);
            coverage.no_truth_altitude +=
                usize::from(result.truth_available && !result.truth_altitude_available);
        }
    }
    for (all, error) in
        t.eta
            .iter_mut()
            .zip([s.oa_eta_error_s, s.eram_eta_error_s, s.tb_fm_eta_error_s])
    {
        all.extend(error.map(f64::abs));
    }
    for (paired, legacy) in t
        .paired_oa_eta
        .iter_mut()
        .zip([s.eram_eta_error_s, s.tb_fm_eta_error_s])
    {
        paired.extend(s.oa_eta_error_s.zip(legacy).map(|(oa, _)| oa.abs()));
    }
}

fn summarize(label: &str, values: &mut [f64], denom: usize) -> String {
    values.sort_by(|a, b| oa_math::cmp_canon_f64(*a, *b));
    if values.is_empty() {
        return format!("{label}: n=0/{denom}, median=n/a, p95=n/a");
    }
    let quantiles = (nearest_rank(values, 50), nearest_rank(values, 95));
    format!(
        "{label}: n={}/{denom}, median={:.1}, p95={:.1}",
        values.len(),
        quantiles.0,
        quantiles.1
    )
}

pub(crate) fn build_plan(
    requested: &str,
    legacy: &Legacy,
    places: &PlaceTable,
    reports: &Reports,
) -> Result<ArrivalPlan, String> {
    if reports.icao.len() > MAX_REPORTS
        || legacy.routes.flight.len() > MAX_CAPTURED_ROUTES
        || legacy.tbfm.kind.len() > MAX_CAPTURED_TBFM
    {
        return Err(
            "arrival experiment input exceeds the report, route, or estimate budget".into(),
        );
    }
    let meter = select_meter(requested, legacy, places)?;
    let mut flights = BTreeMap::<u32, Vec<(i64, RouteIx, usize)>>::new();
    let mut addresses = BTreeSet::new();
    let mut no_addr = 0usize;
    let mut no_meter = 0usize;
    for row in route_rows(legacy) {
        let Some(meter_ix) = meter_index(legacy, row, meter) else {
            no_meter += 1;
            continue;
        };
        let i = row.0 as usize;
        let (Some(&addr), Some(&flight), Some(&at)) = (
            legacy.routes.addr_at_receive.get(i),
            legacy.routes.flight.get(i),
            legacy.routes.recv_ms.get(i),
        ) else {
            return Err("inconsistent arrival route columns".into());
        };
        if addr == NONE {
            no_addr += 1;
        } else {
            if flights.len() == MAX_ISSUES && !flights.contains_key(&flight) {
                return Err("arrival flow exceeds the one-issue-per-flight budget".into());
            }
            if addresses.len() == MAX_ISSUES && !addresses.contains(&addr) {
                return Err("arrival flow exceeds the distinct ADS-B address budget".into());
            }
            addresses.insert(addr);
            flights.entry(flight).or_default().push((at, row, meter_ix));
        }
    }
    let offered_flights = flights.len();
    let mut adsb = BTreeMap::<u32, Vec<ReportIx>>::new();
    for (i, &addr) in reports.icao.iter().enumerate() {
        if addresses.contains(&addr) {
            adsb.entry(addr).or_default().push(ReportIx(i as u32));
        }
    }
    for rows in adsb.values_mut() {
        rows.sort_unstable_by_key(|ix| {
            reports
                .time_ns
                .get(ix.0 as usize)
                .copied()
                .unwrap_or(i64::MAX)
        });
    }

    let mut ambiguous_routes = 0usize;
    let mut no_track = 0usize;
    let mut outside_issue_window = 0usize;
    let mut unusable_route = 0usize;
    let mut baseline_forecast_refused = BTreeMap::new();
    let route_key = |recv, row: RouteIx| {
        (
            recv,
            legacy
                .routes
                .ts_ms
                .get(row.0 as usize)
                .copied()
                .unwrap_or(NO_TIME),
        )
    };
    let mut issues = Vec::new();
    for routes in flights.values_mut() {
        routes.sort_by_key(|&(recv, row, _)| route_key(recv, row));
        let candidate = routes
            .iter()
            .enumerate()
            .find_map(|(i, &(at, row, meter_ix))| {
                let key = route_key(at, row);
                let duplicates = routes
                    .get(i + 1)
                    .is_some_and(|&(t, r, _)| route_key(t, r) == key)
                    || i.checked_sub(1)
                        .and_then(|previous| routes.get(previous))
                        .is_some_and(|&(t, r, _)| route_key(t, r) == key);
                if duplicates {
                    ambiguous_routes += 1;
                    return None;
                }
                let addr = *legacy.routes.addr_at_receive.get(row.0 as usize)?;
                let Some((track, course)) =
                    adsb.get(&addr).and_then(|rows| track_at(reports, rows, at))
                else {
                    no_track += 1;
                    return None;
                };
                if !(MIN_FROM_FIX_M..=MAX_FROM_FIX_M).contains(&distance(track.place, meter.place))
                {
                    outside_issue_window += 1;
                    return None;
                }
                let Some(route) = route_ahead(legacy, places, row, meter_ix, track, course) else {
                    unusable_route += 1;
                    return None;
                };
                Some((
                    IssueSnapshot {
                        cutoff_ms: at,
                        issued_ms: at,
                        track,
                        assigned_altitude_ft: None,
                        assigned_altitude_available_ms: None,
                        assigned_altitude_provenance: None,
                        route,
                        meter_fix: Some(meter),
                    },
                    addr,
                    row,
                    meter_ix,
                ))
            });
        let Some((issue, addr, route_row, meter_ix)) = candidate else {
            continue;
        };
        if let Err(refusal) = forecast(&issue) {
            *baseline_forecast_refused
                .entry(format!("{refusal:?}"))
                .or_insert(0) += 1;
            continue;
        }
        let flight = *legacy
            .routes
            .flight
            .get(route_row.0 as usize)
            .ok_or("inconsistent arrival route flight index")?;
        let gufi = *legacy
            .flights
            .gufi
            .get(flight as usize)
            .ok_or("inconsistent arrival flight identity")?;
        if issues.len() == MAX_ISSUES {
            return Err("arrival issue plan exceeds its bounded flight budget".into());
        }
        issues.push(PlannedIssue {
            issue,
            gufi,
            addr,
            route_row,
            meter_ix,
        });
    }
    Ok(ArrivalPlan {
        requested: requested.to_string(),
        meter,
        offered_flights,
        no_addr,
        no_meter,
        ambiguous_routes,
        no_track,
        outside_issue_window,
        unusable_route,
        baseline_forecast_refused,
        issues,
    })
}

pub(crate) fn report(
    plan: &ArrivalPlan,
    assignments: Option<&AssignmentQuery>,
    legacy: &Legacy,
    built: &BuiltTruth,
    header: &[(String, String)],
    sealed: u64,
) -> Result<String, String> {
    if legacy.routes.flight.len() > MAX_CAPTURED_ROUTES
        || legacy.tbfm.kind.len() > MAX_CAPTURED_TBFM
    {
        return Err(
            "arrival experiment input exceeds the report, route, or estimate budget".into(),
        );
    }
    if !plan.issues.is_empty() && assignments.is_none() {
        return Err("arrival assignment query is required for the frozen issue plan".into());
    }
    let mut totals = Totals::default();
    totals.forecast_refused = plan.baseline_forecast_refused.clone();
    totals.issued = plan.issues.len();
    let tbfm = tbfm_index(legacy, plan.meter)?;
    let mut assigned_forecast_refused = 0usize;
    for planned in &plan.issues {
        let evidence = assignments
            .and_then(|query| assignment_at_issue(query, planned.gufi))
            .unwrap_or(AssignmentEvidence::Missing);
        *totals
            .assigned_altitude_states
            .entry(assigned_altitude_label(&evidence))
            .or_default() += 1;
        let mut issue = planned.issue.clone();
        apply_assignment(&mut issue, &evidence);
        let at = issue.issued_ms;
        let predicted = match forecast(&issue) {
            Ok(predicted) => predicted,
            Err(refusal) => {
                assigned_forecast_refused += 1;
                *totals
                    .forecast_refused
                    .entry(format!("{refusal:?}"))
                    .or_default() += 1;
                for coverage in &mut totals.coverage {
                    coverage.no_prediction += 1;
                }
                continue;
            }
        };
        for (coverage, point) in totals.coverage.iter_mut().zip(&predicted.positions) {
            if point.vertical_status != VerticalStatus::Available {
                *coverage
                    .vertical_unavailable
                    .entry(format!("{:?}", point.vertical_status))
                    .or_default() += 1;
            }
        }
        let eram = legacy
            .route_points
            .est_ms
            .get(planned.meter_ix)
            .copied()
            .filter(|&eta| eta > at)
            .map(|eta_ms| LegacyEta {
                issued_ms: at,
                eta_ms,
            });
        let tb = legacy
            .routes
            .plan_at_receive
            .get(planned.route_row.0 as usize)
            .and_then(|p| tbfm.get(&norm_id(p)))
            .and_then(|entries| estimate_at(entries, at));
        match score(
            &predicted,
            &truth_path(built, planned.addr, at),
            Some(plan.meter),
            eram,
            tb,
            2.0 * 1852.0,
        ) {
            Ok(s) => note(&mut totals, s),
            Err(refusal) => {
                *totals
                    .score_refused
                    .entry(format!("{refusal:?}"))
                    .or_default() += 1
            }
        }
    }
    let mut text = format!(
        "OpenATCA experimental ORD arrival forecast at {}\n",
        name(&plan.meter.name)
    );
    for (label, value) in header {
        writeln!(text, "{label}: {value}").map_err(|e| e.to_string())?;
    }
    writeln!(text, "sealed-window rows dropped: {sealed}").map_err(|e| e.to_string())?;
    writeln!(text, "Selected flow: {}. AUTO ranks resolvable TBFM fixes by distinct ORD flight plans before scoring. One earliest baseline-eligible issue per flight, 40 to 200 NM from the fix, using the most recent unique ADS-B report within 30 s. Issue GUFI and cutoff are frozen after route, track and assignment-free forecast eligibility, before the SFDPS assignment query. Forecast uses received ERAM route geometry and ADS-B observation time. Legacy ETA values do not enter the model.", plan.requested).map_err(|e| e.to_string())?;
    writeln!(text, "Procedure input: the legacy route tables contain no published procedure altitude windows; route constraint checks are unavailable for this run.").map_err(|e| e.to_string())?;
    writeln!(
        text,
        "Assigned altitude state at issue (sum denominator: {} frozen baseline issues): {:?}",
        plan.issues.len(),
        totals.assigned_altitude_states
    )
    .map_err(|e| e.to_string())?;
    writeln!(
        text,
        "Assignment query: {}",
        if assignments.is_some() {
            "cutoff-specific SFDPS scan for frozen issue GUFIs"
        } else {
            "no eligible issue; scan skipped"
        }
    )
    .map_err(|e| e.to_string())?;
    let forecast_refused: usize = totals.forecast_refused.values().sum();
    let baseline_forecast_refused: usize = plan.baseline_forecast_refused.values().sum();
    let score_refused: usize = totals.score_refused.values().sum();
    writeln!(text, "offered flights with known address: {}; route messages without address: {}; frozen baseline issues: {}; scored: {}; no eligible route/track: {}; assignment-free forecast refused: {}; forecast refused with assignment evidence: {}; score refused: {}; observed passages: {}", plan.offered_flights, plan.no_addr, totals.issued, totals.issued.saturating_sub(assigned_forecast_refused + score_refused), plan.offered_flights.saturating_sub(totals.issued + baseline_forecast_refused), baseline_forecast_refused, assigned_forecast_refused, score_refused, totals.crossing).map_err(|e| e.to_string())?;
    for (kind, refusals) in [
        ("forecast", &totals.forecast_refused),
        ("score", &totals.score_refused),
    ] {
        for (reason, count) in refusals {
            writeln!(text, "{kind} refused: {reason} {count}").map_err(|e| e.to_string())?;
        }
    }
    writeln!(text, "route messages excluded: outside selected fix or repeated fix {}; ambiguous same-time routes {}; missing, stale or conflicting ADS-B state {}; outside issue distance {}; unresolved forward route {}", plan.no_meter, plan.ambiguous_routes, plan.no_track, plan.outside_issue_window, plan.unusable_route).map_err(|e| e.to_string())?;
    for (((mins, h), v), coverage) in [5, 10, 20]
        .into_iter()
        .zip(&mut totals.horizontal)
        .zip(&mut totals.vertical)
        .zip(&totals.coverage)
    {
        writeln!(
            text,
            "{}\n{}",
            summarize(&format!("{mins} min horizontal (m)"), h, totals.issued),
            summarize(&format!("{mins} min vertical (ft)"), v, totals.issued)
        )
        .map_err(|e| e.to_string())?;
        writeln!(text, "{mins} min coverage: denominator is all {} issued forecasts; missing predicted position {}; no usable truth position {}; truth position without altitude {}", totals.issued, coverage.no_prediction, coverage.no_truth_position, coverage.no_truth_altitude).map_err(|e| e.to_string())?;
        let reasons = coverage
            .vertical_unavailable
            .iter()
            .map(|(reason, count)| format!("{reason}={count}"))
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(
            text,
            "{mins} min forecast vertical unavailable: {}/{} issued; {}",
            coverage.vertical_unavailable.values().sum::<usize>(),
            totals.issued,
            if reasons.is_empty() {
                "none"
            } else {
                reasons.as_str()
            }
        )
        .map_err(|e| e.to_string())?;
    }
    for (label, values) in ["OpenATCA", "ERAM", "TBFM"]
        .into_iter()
        .zip(&mut totals.eta)
    {
        writeln!(
            text,
            "{}",
            summarize(
                &format!("{label} meter absolute ETA error (s)"),
                values,
                totals.crossing
            )
        )
        .map_err(|e| e.to_string())?;
    }
    let [_, eram, tbfm] = &totals.eta;
    for ((label, values), available) in ["ERAM", "TBFM"]
        .into_iter()
        .zip(&mut totals.paired_oa_eta)
        .zip([eram.len(), tbfm.len()])
    {
        writeln!(
            text,
            "{}",
            summarize(
                &format!("OpenATCA meter absolute ETA error where {label} available (s)"),
                values,
                available
            )
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(text)
}

#[cfg(test)]
mod assignment_plan_tests {
    use super::*;

    #[test]
    fn frozen_plan_exports_one_typed_gufi_and_cutoff_per_issue() {
        let fix = RouteFix {
            name: *b"METER   ",
            place: Place {
                lat_deg: 41.7,
                lon_deg: -87.9,
            },
            altitude_constraint: None,
        };
        let cutoff_ms = 1_794_664_309_564;
        let plan = ArrivalPlan {
            requested: "ORD".into(),
            meter: fix.place,
            offered_flights: 1,
            no_addr: 0,
            no_meter: 0,
            ambiguous_routes: 0,
            no_track: 0,
            outside_issue_window: 0,
            unusable_route: 0,
            baseline_forecast_refused: BTreeMap::new(),
            issues: vec![PlannedIssue {
                issue: IssueSnapshot {
                    cutoff_ms,
                    issued_ms: cutoff_ms,
                    track: TrackState {
                        time_ms: cutoff_ms,
                        place: fix.place,
                        altitude_ft: None,
                        speed_mps: None,
                        vertical_rate_fpm: None,
                    },
                    assigned_altitude_ft: None,
                    assigned_altitude_available_ms: None,
                    assigned_altitude_provenance: None,
                    route: vec![fix],
                    meter_fix: Some(fix),
                },
                gufi: 0x1111,
                addr: 1,
                route_row: RouteIx(0),
                meter_ix: 0,
            }],
        };

        let issues = plan.assignment_issues();
        assert_eq!(issues.len(), 1);
        assert_eq!(
            issues.first().map(|issue| (issue.gufi, issue.cutoff_ms)),
            Some((0x1111, cutoff_ms))
        );
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod assigned_altitude_tests {
    use super::*;
    use oa_bench::ingest::{ingest_sfdps, new_reader};
    use oa_bench::legacy::{
        new_legacy, push_assigned_altitude, ALT_FORM_SIMPLE, ALT_FORM_VFR,
        MAX_ASSIGNED_ALTITUDE_UPDATES_PER_FLIGHT,
    };

    const GUFI: &str = "11111111-2222-4333-8444-555555555555";
    const BASE_RECEIVED_MS: i64 = 1_794_664_309_564;

    fn assignment_history() -> Legacy {
        let mut legacy = new_legacy();
        legacy.limits.assigned_altitude_updates = MAX_CAPTURED_ASSIGNED_ALTITUDES;
        legacy
    }

    fn message(timestamp: &str, assigned: &str) -> String {
        format!(
            r#"<?xml version="1.0"?><ns5:MessageCollection xmlns:ns5="http://www.faa.aero/nas/3.0" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"><message xsi:type="ns5:FlightMessageType"><flight xsi:type="ns5:NasFlightType" centre="ZXX" source="TH" system="SLC" timestamp="{timestamp}"><gufi codeSpace="urn:uuid">{GUFI}</gufi>{assigned}</flight></message></ns5:MessageCollection>"#
        )
    }

    fn ingest(
        legacy: &mut Legacy,
        reader: &mut oa_bench::ingest::Reader,
        timestamp: &str,
        recv_ms: i64,
        assigned: &str,
    ) {
        let xml = message(timestamp, assigned);
        ingest_sfdps(legacy, reader, xml.as_bytes(), recv_ms);
    }

    // REQ: OA-LTJ-055
    #[test]
    fn decoder_history_is_cutoff_safe_and_valid_unsupported_form_replaces_simple() {
        let mut legacy = assignment_history();
        let mut reader = new_reader().expect("synthetic decoder setup");
        ingest(
            &mut legacy,
            &mut reader,
            "2026-11-14T13:50:49.564Z",
            BASE_RECEIVED_MS,
            "<assignedAltitude><simple uom=\"FEET\">12000</simple></assignedAltitude>",
        );
        ingest(
            &mut legacy,
            &mut reader,
            "2026-11-14T13:50:50.564Z",
            BASE_RECEIVED_MS + 2_000,
            "<assignedAltitude><vfr/></assignedAltitude>",
        );
        ingest(
            &mut legacy,
            &mut reader,
            "2026-11-14T13:50:51.564Z",
            BASE_RECEIVED_MS + 4_000,
            "<assignedAltitude><simple uom=\"FEET\">14000</simple></assignedAltitude>",
        );
        let index = assigned_altitude_index(&legacy, &BTreeSet::from([0]))
            .expect("bounded synthetic index");
        let selected = assigned_altitude_at(&legacy, &index, 0, BASE_RECEIVED_MS + 1_000);
        assert_eq!(
            selected,
            AssignedAltitudeInput::Available {
                value_ft: 12_000,
                available_ms: BASE_RECEIVED_MS,
            }
        );
        let AssignedAltitudeInput::Available {
            value_ft,
            available_ms,
        } = selected
        else {
            panic!("simple assigned altitude is available at the cutoff");
        };
        let fix = RouteFix {
            name: *b"METER   ",
            place: Place {
                lat_deg: 41.7,
                lon_deg: -87.9,
            },
            altitude_constraint: None,
        };
        let snapshot = IssueSnapshot {
            cutoff_ms: BASE_RECEIVED_MS + 1_000,
            issued_ms: BASE_RECEIVED_MS + 1_000,
            track: TrackState {
                time_ms: BASE_RECEIVED_MS + 1_000,
                place: Place {
                    lat_deg: 41.3,
                    lon_deg: -88.4,
                },
                altitude_ft: Some(12_000.0),
                speed_mps: Some(180.0),
                vertical_rate_fpm: Some(-600.0),
            },
            assigned_altitude_ft: Some(f64::from(value_ft)),
            assigned_altitude_available_ms: Some(available_ms),
            assigned_altitude_provenance: Some(
                crate::core::arrival::ConstraintProvenance::SfdpsAssignedAltitude,
            ),
            route: vec![fix],
            meter_fix: Some(fix),
        };
        let forecast = forecast(&snapshot).expect("cutoff-safe decoded altitude forecast");
        assert_eq!(forecast.assigned_altitude_available_ms, Some(available_ms));
        assert_eq!(
            forecast.assigned_altitude_provenance,
            Some(crate::core::arrival::ConstraintProvenance::SfdpsAssignedAltitude,)
        );
        assert_eq!(
            assigned_altitude_at(&legacy, &index, 0, BASE_RECEIVED_MS + 3_000),
            AssignedAltitudeInput::UnsupportedForm {
                form: ALT_FORM_VFR,
                available_ms: BASE_RECEIVED_MS + 2_000,
            }
        );
        assert_eq!(
            assigned_altitude_at(&legacy, &index, 0, BASE_RECEIVED_MS + 1_000),
            AssignedAltitudeInput::Available {
                value_ft: 12_000,
                available_ms: BASE_RECEIVED_MS,
            }
        );
        assert_eq!(
            assigned_altitude_at(&legacy, &index, 0, BASE_RECEIVED_MS + 5_000),
            AssignedAltitudeInput::Available {
                value_ft: 14_000,
                available_ms: BASE_RECEIVED_MS + 4_000,
            }
        );
    }

    // REQ: OA-LTJ-055
    #[test]
    fn malformed_received_assignment_does_not_reuse_the_previous_simple_assignment() {
        let mut legacy = assignment_history();
        let mut reader = new_reader().expect("synthetic decoder setup");
        ingest(
            &mut legacy,
            &mut reader,
            "2026-11-14T13:50:49.564Z",
            BASE_RECEIVED_MS,
            "<assignedAltitude><simple uom=\"FEET\">12000</simple></assignedAltitude>",
        );
        ingest(
            &mut legacy,
            &mut reader,
            "2026-11-14T13:50:50.564Z",
            BASE_RECEIVED_MS + 2_000,
            "<assignedAltitude><simple uom=\"FEET\">14000</simple><vfr/></assignedAltitude>",
        );
        let index = assigned_altitude_index(&legacy, &BTreeSet::from([0]))
            .expect("bounded synthetic index");
        assert_eq!(
            assigned_altitude_at(&legacy, &index, 0, BASE_RECEIVED_MS + 1_000),
            AssignedAltitudeInput::Available {
                value_ft: 12_000,
                available_ms: BASE_RECEIVED_MS,
            }
        );
        assert_eq!(
            assigned_altitude_at(&legacy, &index, 0, BASE_RECEIVED_MS + 2_000),
            AssignedAltitudeInput::MalformedForm {
                available_ms: BASE_RECEIVED_MS + 2_000,
            }
        );
    }

    // REQ: OA-LTJ-055
    #[test]
    fn assigned_altitude_history_is_bounded_per_flight() {
        let mut legacy = assignment_history();
        for i in 0..=MAX_ASSIGNED_ALTITUDE_UPDATES_PER_FLIGHT {
            let recv_ms = BASE_RECEIVED_MS + i64::from(i);
            push_assigned_altitude(
                &mut legacy,
                0,
                BASE_RECEIVED_MS,
                recv_ms,
                ALT_FORM_SIMPLE,
                12_000,
            );
        }
        assert_eq!(
            legacy.assigned_altitudes.flight.len(),
            usize::from(MAX_ASSIGNED_ALTITUDE_UPDATES_PER_FLIGHT)
        );
        assert_eq!(legacy.counts.overflow_assigned_altitude_updates, 1);
        assert!(!assigned_altitude_history_overflowed(
            &legacy,
            0,
            BASE_RECEIVED_MS + 255,
        ));
        assert!(assigned_altitude_history_overflowed(
            &legacy,
            0,
            BASE_RECEIVED_MS + 256,
        ));
        let index = assigned_altitude_index(&legacy, &BTreeSet::from([0]))
            .expect("bounded synthetic index at exact capacity");
        assert_eq!(
            assigned_altitude_at(&legacy, &index, 0, BASE_RECEIVED_MS + 255),
            AssignedAltitudeInput::Available {
                value_ft: 12_000,
                available_ms: BASE_RECEIVED_MS,
            }
        );
        assert_eq!(
            assigned_altitude_at(&legacy, &index, 0, BASE_RECEIVED_MS + 300),
            AssignedAltitudeInput::HistoryOverflow
        );
    }

    // REQ: OA-LTJ-055
    #[test]
    fn same_stamped_time_conflict_refuses_scalar_and_unknown_receive_is_not_used() {
        let mut legacy = assignment_history();
        let mut reader = new_reader().expect("synthetic decoder setup");
        ingest(
            &mut legacy,
            &mut reader,
            "2026-11-14T13:50:49.564Z",
            BASE_RECEIVED_MS,
            "<assignedAltitude><simple uom=\"FEET\">12000</simple></assignedAltitude>",
        );
        ingest(
            &mut legacy,
            &mut reader,
            "2026-11-14T13:50:49.564Z",
            BASE_RECEIVED_MS,
            "<assignedAltitude><simple uom=\"FEET\">14000</simple></assignedAltitude>",
        );
        let index = assigned_altitude_index(&legacy, &BTreeSet::from([0]))
            .expect("bounded synthetic index");
        assert_eq!(
            assigned_altitude_at(&legacy, &index, 0, BASE_RECEIVED_MS),
            AssignedAltitudeInput::SameTimeConflict {
                available_ms: BASE_RECEIVED_MS,
            }
        );

        let mut unknown = assignment_history();
        let mut reader = new_reader().expect("synthetic decoder setup");
        ingest(
            &mut unknown,
            &mut reader,
            "2026-11-14T13:50:49.564Z",
            NO_TIME,
            "<assignedAltitude><simple uom=\"FEET\">18000</simple></assignedAltitude>",
        );
        assert_eq!(unknown.assigned_altitudes.flight.len(), 0);
        assert_eq!(unknown.unknown_assigned_altitudes.flight.len(), 1);
        assert_eq!(unknown.counts.assigned_altitude_unknown_availability, 1);
        let index = assigned_altitude_index(&unknown, &BTreeSet::from([0]))
            .expect("bounded synthetic index with an unknown receipt time");
        assert_eq!(
            assigned_altitude_at(&unknown, &index, 0, BASE_RECEIVED_MS),
            AssignedAltitudeInput::UnknownAvailability {
                source_ms: BASE_RECEIVED_MS - 60_000,
            }
        );
    }

    // REQ: OA-LTJ-055
    #[test]
    fn later_unknown_receipt_blocks_only_after_its_source_time_and_newer_definitive_event_supersedes(
    ) {
        let mut legacy = assignment_history();
        let mut reader = new_reader().expect("synthetic decoder setup");
        let source_base = BASE_RECEIVED_MS - 60_000;
        ingest(
            &mut legacy,
            &mut reader,
            "2026-11-14T13:50:49.564Z",
            source_base,
            "<assignedAltitude><simple uom=\"FEET\">12000</simple></assignedAltitude>",
        );
        ingest(
            &mut legacy,
            &mut reader,
            "2026-11-14T13:50:50.564Z",
            NO_TIME,
            "<assignedAltitude><simple uom=\"FEET\">18000</simple></assignedAltitude>",
        );
        let index = assigned_altitude_index(&legacy, &BTreeSet::from([0]))
            .expect("bounded synthetic index");
        assert_eq!(
            assigned_altitude_at(&legacy, &index, 0, source_base + 500),
            AssignedAltitudeInput::Available {
                value_ft: 12_000,
                available_ms: source_base,
            }
        );
        assert_eq!(
            assigned_altitude_at(&legacy, &index, 0, source_base + 1_000),
            AssignedAltitudeInput::UnknownAvailability {
                source_ms: source_base + 1_000,
            }
        );

        push_assigned_altitude(
            &mut legacy,
            0,
            source_base + 2_000,
            source_base + 3_000,
            ALT_FORM_SIMPLE,
            20_000,
        );
        let index = assigned_altitude_index(&legacy, &BTreeSet::from([0]))
            .expect("bounded synthetic index after definitive replacement");
        assert_eq!(
            assigned_altitude_at(&legacy, &index, 0, source_base + 3_000,),
            AssignedAltitudeInput::Available {
                value_ft: 20_000,
                available_ms: source_base + 3_000,
            }
        );
    }
}
