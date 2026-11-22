//! Offline arrival experiment (shell, B3/C6). Binds recording rows at receipt time and passes
//! normalized issue inputs and separately sampled truth to the pure arrival core.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

use oa_adsb::{
    Reports, F_ALT_GEOM, F_GROUND, F_RATE_GEOM, F_STALE, NONE_I32, NONE_U16, NONE_U8,
    SOURCE_UNKNOWN,
};
use oa_bench::adsb_truth::{query, BuiltTruth};
use oa_bench::assignment::{
    assignment_at_issue, AssignmentEvidence, AssignmentIssue, AssignmentQuery,
    MAX_ASSIGNMENT_ISSUES,
};
use oa_bench::geo::{dist_m, enu, heading_deg, wrap180};
#[cfg(test)]
use oa_bench::legacy::{
    assigned_altitude_history_overflowed, ALT_FORM_INVALID, ALT_FORM_SIMPLE,
    MAX_ASSIGNED_ALTITUDE_UPDATES, NO_ALT,
};
use oa_bench::legacy::{
    name, norm_id, tbfm_kind, Legacy, ALT_FORM_AT_FIX, ALT_FORM_BLOCK, ALT_FORM_VFR,
    ALT_FORM_VFR_PLUS, NONE, NO_COORD, NO_TIME,
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
const MAX_ISSUES: usize = MAX_ASSIGNMENT_ISSUES;
const MAX_TRACK_AGE_MS: i64 = 30_000;
const MAX_RATE_DIAGNOSTIC_ROWS: usize = 128;
const MAX_RATE_DIAGNOSTIC_GAP_NS: i64 = 10_000_000_000;
const RATE_DIAGNOSTIC_TOLERANCE_FT: f64 = 50.0;
const TRUTH_STEP_MS: i64 = 5_000;
const TRUTH_STEPS: i64 = 720;
const MIN_FROM_FIX_M: f64 = 40.0 * 1852.0;
const MAX_FROM_FIX_M: f64 = 200.0 * 1852.0;

enum TrackSelection {
    Available(TrackState, f64, usize),
    SourceFlagStale,
    Refused,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RawRateStatus {
    MissingRate,
    GeometricRate,
    MissingBarometricAltitude,
    UnknownSelectedSource,
    NonAdsbSelectedSource,
    NoComparablePrior,
    PriorSourceUnknown,
    PriorSourceNonAdsb,
    PriorSourceMismatch,
    PriorNotBarometricOrStale,
    PriorTimestampConflict,
    ScanLimit,
    Agrees,
    Contradicts,
}

#[derive(Default)]
// A7.2: cold aggregate over the frozen issue cohort, bounded by MAX_ISSUES; guard waived.
struct RawRateDiagnostic {
    missing_rate: usize,
    geometric_rate: usize,
    missing_baro_altitude: usize,
    unknown_selected_source: usize,
    non_adsb_selected_source: usize,
    no_comparable_prior: usize,
    prior_source_unknown: usize,
    prior_source_non_adsb: usize,
    prior_source_mismatch: usize,
    prior_not_baro_or_stale: usize,
    prior_timestamp_conflict: usize,
    scan_limit: usize,
    agrees: usize,
    contradicts: usize,
}

impl RawRateDiagnostic {
    fn add(&mut self, status: RawRateStatus) {
        let count = match status {
            RawRateStatus::MissingRate => &mut self.missing_rate,
            RawRateStatus::GeometricRate => &mut self.geometric_rate,
            RawRateStatus::MissingBarometricAltitude => &mut self.missing_baro_altitude,
            RawRateStatus::UnknownSelectedSource => &mut self.unknown_selected_source,
            RawRateStatus::NonAdsbSelectedSource => &mut self.non_adsb_selected_source,
            RawRateStatus::NoComparablePrior => &mut self.no_comparable_prior,
            RawRateStatus::PriorSourceUnknown => &mut self.prior_source_unknown,
            RawRateStatus::PriorSourceNonAdsb => &mut self.prior_source_non_adsb,
            RawRateStatus::PriorSourceMismatch => &mut self.prior_source_mismatch,
            RawRateStatus::PriorNotBarometricOrStale => &mut self.prior_not_baro_or_stale,
            RawRateStatus::PriorTimestampConflict => &mut self.prior_timestamp_conflict,
            RawRateStatus::ScanLimit => &mut self.scan_limit,
            RawRateStatus::Agrees => &mut self.agrees,
            RawRateStatus::Contradicts => &mut self.contradicts,
        };
        *count += 1;
    }
}

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

fn track_at(reports: &Reports, rows: &[ReportIx], cutoff: i64) -> TrackSelection {
    let Some(cutoff_ns) = cutoff.checked_mul(1_000_000) else {
        return TrackSelection::Refused;
    };
    let i = rows.partition_point(|ix| {
        reports
            .time_ns
            .get(ix.0 as usize)
            .is_some_and(|&t| t <= cutoff_ns)
    });
    let Some(selected_position) = i.checked_sub(1) else {
        return TrackSelection::Refused;
    };
    let Some(picked) = rows.get(selected_position).map(|ix| ix.0 as usize) else {
        return TrackSelection::Refused;
    };
    let Some(&time_ns) = reports.time_ns.get(picked) else {
        return TrackSelection::Refused;
    };
    // Multiple reports with equal observation time give no unique as-of state.
    if i.checked_sub(2)
        .and_then(|j| rows.get(j))
        .and_then(|ix| reports.time_ns.get(ix.0 as usize))
        == Some(&time_ns)
    {
        return TrackSelection::Refused;
    }
    let time_ms = time_ns / 1_000_000;
    let (Some(&speed), Some(&course), Some(&flags)) = (
        reports.gs_dkt.get(picked),
        reports.track_ddeg.get(picked),
        reports.flags.get(picked),
    ) else {
        return TrackSelection::Refused;
    };
    if flags & F_STALE != 0 {
        return TrackSelection::SourceFlagStale;
    }
    if cutoff
        .checked_sub(time_ms)
        .is_none_or(|age| age > MAX_TRACK_AGE_MS)
        || flags & F_GROUND != 0
        || speed == NONE_U16
        || course >= 3600
    {
        return TrackSelection::Refused;
    }
    let (Some(&alt), Some(&rate), Some(&lat), Some(&lon)) = (
        reports.baro_alt_ft.get(picked),
        reports.vrate_fpm.get(picked),
        reports.lat_e7.get(picked),
        reports.lon_e7.get(picked),
    ) else {
        return TrackSelection::Refused;
    };
    let track = TrackState {
        time_ms,
        place: place_of_e7(lat, lon),
        altitude_ft: (alt != NONE_I32).then_some(f64::from(alt)),
        speed_mps: Some(f64::from(speed) * (1852.0 / 36_000.0)),
        vertical_rate_fpm: (rate != NONE_I32 && flags & F_RATE_GEOM == 0)
            .then_some(f64::from(rate)),
    };
    TrackSelection::Available(track, f64::from(course) / 10.0, selected_position)
}

fn raw_rate_status(
    reports: &Reports,
    rows: &[ReportIx],
    selected_position: usize,
) -> RawRateStatus {
    let Some(&selected_ix) = rows.get(selected_position) else {
        return RawRateStatus::NoComparablePrior;
    };
    let selected_ix = selected_ix.0 as usize;
    let (
        Some(&selected_time),
        Some(&selected_source),
        Some(&selected_flags),
        Some(&selected_alt),
        Some(&selected_rate),
    ) = (
        reports.time_ns.get(selected_ix),
        reports.source.get(selected_ix),
        reports.flags.get(selected_ix),
        reports.baro_alt_ft.get(selected_ix),
        reports.vrate_fpm.get(selected_ix),
    )
    else {
        return RawRateStatus::NoComparablePrior;
    };
    if selected_rate == NONE_I32 {
        return RawRateStatus::MissingRate;
    }
    if selected_flags & F_RATE_GEOM != 0 {
        return RawRateStatus::GeometricRate;
    }
    if selected_alt == NONE_I32 || selected_flags & (F_ALT_GEOM | F_GROUND) != 0 {
        return RawRateStatus::MissingBarometricAltitude;
    }
    if selected_source == NONE_U8 || selected_source == SOURCE_UNKNOWN {
        return RawRateStatus::UnknownSelectedSource;
    }
    if !is_adsb_source(selected_source) {
        return RawRateStatus::NonAdsbSelectedSource;
    }
    if selected_position > 0
        && rows
            .get(selected_position - 1)
            .and_then(|ix| reports.time_ns.get(ix.0 as usize))
            == Some(&selected_time)
    {
        return RawRateStatus::PriorTimestampConflict;
    }
    if rows
        .get(selected_position + 1)
        .and_then(|ix| reports.time_ns.get(ix.0 as usize))
        == Some(&selected_time)
    {
        return RawRateStatus::PriorTimestampConflict;
    }

    let mut cursor = selected_position;
    let mut scanned = 0usize;
    let mut saw_unknown = false;
    let mut saw_non_adsb = false;
    let mut saw_source_mismatch = false;
    let mut saw_incompatible_baro = false;
    while cursor > 0 && scanned < MAX_RATE_DIAGNOSTIC_ROWS {
        let Some(&row_ix) = rows.get(cursor - 1) else {
            break;
        };
        let row_ix = row_ix.0 as usize;
        let Some(&row_time) = reports.time_ns.get(row_ix) else {
            break;
        };
        if row_time >= selected_time {
            return RawRateStatus::PriorTimestampConflict;
        }
        let Some(gap_ns) = selected_time.checked_sub(row_time) else {
            return RawRateStatus::PriorTimestampConflict;
        };
        if gap_ns > MAX_RATE_DIAGNOSTIC_GAP_NS {
            break;
        }
        let group_start = cursor - 1;
        if group_start > 0 {
            if scanned + 1 >= MAX_RATE_DIAGNOSTIC_ROWS {
                return RawRateStatus::ScanLimit;
            }
            if rows
                .get(group_start - 1)
                .and_then(|ix| reports.time_ns.get(ix.0 as usize))
                == Some(&row_time)
            {
                return RawRateStatus::PriorTimestampConflict;
            }
        }
        scanned += 1;
        let Some(&source) = reports.source.get(row_ix) else {
            saw_unknown = true;
            cursor = group_start;
            continue;
        };
        if source == NONE_U8 || source == SOURCE_UNKNOWN {
            saw_unknown = true;
        } else if !is_adsb_source(source) {
            saw_non_adsb = true;
        } else if source != selected_source {
            saw_source_mismatch = true;
        } else {
            let (Some(&flags), Some(&alt)) =
                (reports.flags.get(row_ix), reports.baro_alt_ft.get(row_ix))
            else {
                saw_incompatible_baro = true;
                cursor = group_start;
                continue;
            };
            if flags & (F_STALE | F_ALT_GEOM | F_GROUND) != 0 || alt == NONE_I32 {
                saw_incompatible_baro = true;
            } else {
                let rate_delta_ft = f64::from(selected_rate) * (gap_ns as f64 / 60_000_000_000.0);
                let observed_delta_ft = f64::from(selected_alt) - f64::from(alt);
                return if (rate_delta_ft - observed_delta_ft).abs() <= RATE_DIAGNOSTIC_TOLERANCE_FT
                {
                    RawRateStatus::Agrees
                } else {
                    RawRateStatus::Contradicts
                };
            }
        }
        cursor = group_start;
    }
    if scanned == MAX_RATE_DIAGNOSTIC_ROWS && cursor > 0 {
        return RawRateStatus::ScanLimit;
    }
    if saw_unknown {
        RawRateStatus::PriorSourceUnknown
    } else if saw_non_adsb {
        RawRateStatus::PriorSourceNonAdsb
    } else if saw_source_mismatch {
        RawRateStatus::PriorSourceMismatch
    } else if saw_incompatible_baro {
        RawRateStatus::PriorNotBarometricOrStale
    } else {
        RawRateStatus::NoComparablePrior
    }
}

fn is_adsb_source(source: u8) -> bool {
    // Known ADS-B source classes accepted by oa-truth; the diagnostic still refuses unknowns.
    matches!(source, 0 | 1 | 8)
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
    horizontal: [Vec<f64>; 3],
    vertical: [Vec<f64>; 3],
    coverage: [HorizonCoverage; 3],
    assigned_altitude_states: BTreeMap<String, usize>,
    assigned_altitude_pairs: [PairedVertical; 3],
    assigned_altitude_pair_eligible: usize,
    eta: [Vec<f64>; 3],
    paired_oa_eta: [Vec<f64>; 2],
}

#[derive(Default)]
struct PairedVertical {
    assignment_free_abs_error_ft: Vec<f64>,
    assigned_abs_error_ft: Vec<f64>,
    improved: usize,
    worsened: usize,
    tied: usize,
    missing_assignment_free: usize,
    missing_assigned: usize,
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
    report_position: usize,
}

/// The fixed no-assignment cohort selected before cutoff-specific assignment evidence is queried.
pub(crate) struct ArrivalPlan {
    requested: String,
    meter: RouteFix,
    offered_flights: usize,
    no_addr: usize,
    no_meter: usize,
    ambiguous_routes: usize,
    no_track: usize,
    stale_source_track: usize,
    outside_issue_window: usize,
    unusable_route: usize,
    baseline_forecast_refused: BTreeMap<String, usize>,
    issues: Vec<PlannedIssue>,
    raw_rate_diagnostic: Option<RawRateDiagnostic>,
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
            let name = match *form {
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

fn note_assignment_pair(
    totals: &mut Totals,
    assignment_free: Option<ArrivalScore>,
    assigned: Option<ArrivalScore>,
) {
    totals.assigned_altitude_pair_eligible += 1;
    for (i, pair) in totals.assigned_altitude_pairs.iter_mut().enumerate() {
        let before = assignment_free.and_then(|score| score.forecast.get(i)?.vertical_error_ft);
        let after = assigned.and_then(|score| score.forecast.get(i)?.vertical_error_ft);
        if before.is_none() {
            pair.missing_assignment_free += 1;
        }
        if after.is_none() {
            pair.missing_assigned += 1;
        }
        if let (Some(before), Some(after)) = (before, after) {
            pair.assignment_free_abs_error_ft.push(before);
            pair.assigned_abs_error_ft.push(after);
            if after < before {
                pair.improved += 1;
            } else if after > before {
                pair.worsened += 1;
            } else {
                pair.tied += 1;
            }
        }
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
    diagnose_raw_rate: bool,
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
    let mut stale_source_track = 0usize;
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
                let selected = adsb
                    .get(&addr)
                    .map_or(TrackSelection::Refused, |rows| track_at(reports, rows, at));
                let (track, course, report_position) = match selected {
                    TrackSelection::Available(track, course, position) => (track, course, position),
                    TrackSelection::SourceFlagStale => {
                        stale_source_track += 1;
                        return None;
                    }
                    TrackSelection::Refused => {
                        no_track += 1;
                        return None;
                    }
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
                    report_position,
                ))
            });
        let Some((issue, addr, route_row, meter_ix, report_position)) = candidate else {
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
            report_position,
        });
    }
    let raw_rate_diagnostic = diagnose_raw_rate.then(|| {
        let mut diagnostic = RawRateDiagnostic::default();
        for planned in &issues {
            let status = adsb
                .get(&planned.addr)
                .map_or(RawRateStatus::NoComparablePrior, |rows| {
                    raw_rate_status(reports, rows, planned.report_position)
                });
            diagnostic.add(status);
        }
        diagnostic
    });
    Ok(ArrivalPlan {
        requested: requested.to_string(),
        meter,
        offered_flights,
        no_addr,
        no_meter,
        ambiguous_routes,
        no_track,
        stale_source_track,
        outside_issue_window,
        unusable_route,
        baseline_forecast_refused,
        issues,
        raw_rate_diagnostic,
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
    let mut totals = Totals {
        forecast_refused: plan.baseline_forecast_refused.clone(),
        issued: plan.issues.len(),
        ..Totals::default()
    };
    let tbfm = tbfm_index(legacy, plan.meter)?;
    let mut assigned_forecast_refused = 0usize;
    for planned in &plan.issues {
        let evidence = assignments
            .and_then(|query| assignment_at_issue(query, planned.gufi))
            .ok_or("arrival assignment query does not cover its frozen issue plan")?;
        *totals
            .assigned_altitude_states
            .entry(assigned_altitude_label(&evidence))
            .or_default() += 1;
        let pair_eligible = matches!(&evidence, AssignmentEvidence::Available { .. });
        let assignment_free_forecast = pair_eligible
            .then(|| forecast(&planned.issue).ok())
            .flatten();
        let mut issue = planned.issue.clone();
        apply_assignment(&mut issue, &evidence);
        let at = issue.issued_ms;
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
                if pair_eligible {
                    let truth = truth_path(built, planned.addr, at);
                    let assignment_free_score = assignment_free_forecast.as_ref().and_then(|f| {
                        score(f, &truth, Some(plan.meter), eram, tb, 2.0 * 1852.0).ok()
                    });
                    note_assignment_pair(&mut totals, assignment_free_score, None);
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
        let truth = truth_path(built, planned.addr, at);
        let assigned_score =
            match score(&predicted, &truth, Some(plan.meter), eram, tb, 2.0 * 1852.0) {
                Ok(s) => {
                    note(&mut totals, s);
                    Some(s)
                }
                Err(refusal) => {
                    *totals
                        .score_refused
                        .entry(format!("{refusal:?}"))
                        .or_default() += 1;
                    None
                }
            };
        if pair_eligible {
            let assignment_free_score = assignment_free_forecast
                .as_ref()
                .and_then(|f| score(f, &truth, Some(plan.meter), eram, tb, 2.0 * 1852.0).ok());
            note_assignment_pair(&mut totals, assignment_free_score, assigned_score);
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
    writeln!(text, "Selected flow: {}. AUTO ranks resolvable TBFM fixes by distinct ORD flight plans before scoring. The experiment takes the first eligible route and track snapshot per flight, 40 to 200 NM from the fix, using the most recent unique ADS-B report within 30 s. It freezes each issue GUFI and cutoff after an assignment-free forecast succeeds, before the SFDPS assignment query. Forecast uses received ERAM route geometry and ADS-B observation time. Legacy ETA values do not enter the model.", plan.requested).map_err(|e| e.to_string())?;
    writeln!(text, "Procedure input: the legacy route tables contain no published procedure altitude windows; route constraint checks are unavailable for this run.").map_err(|e| e.to_string())?;
    if let Some(diagnostic) = &plan.raw_rate_diagnostic {
        writeln!(text, "Raw vertical-rate diagnostic: aggregate over {} frozen issues; compares the selected raw rate with a unique preceding same-source barometric altitude within 10 s, scanning at most {MAX_RATE_DIAGNOSTIC_ROWS} rows per issue. Agreement uses an illustrative 50 ft tolerance (about two 25 ft altitude quanta). This is a raw-pair proxy, not the tracker decision or receipt-time evidence.", plan.issues.len()).map_err(|e| e.to_string())?;
        writeln!(text, "raw-rate outcomes: missing rate {}; geometric rate {}; missing barometric altitude {}; selected source unknown {}; selected source non-ADS-B {}; no comparable prior {}; prior source unknown {}; prior source non-ADS-B {}; prior source mismatch {}; prior barometric altitude missing, geometric, or stale {}; prior timestamp conflict {}; preceding-row scan limit {}; agrees {}; contradicts {}", diagnostic.missing_rate, diagnostic.geometric_rate, diagnostic.missing_baro_altitude, diagnostic.unknown_selected_source, diagnostic.non_adsb_selected_source, diagnostic.no_comparable_prior, diagnostic.prior_source_unknown, diagnostic.prior_source_non_adsb, diagnostic.prior_source_mismatch, diagnostic.prior_not_baro_or_stale, diagnostic.prior_timestamp_conflict, diagnostic.scan_limit, diagnostic.agrees, diagnostic.contradicts).map_err(|e| e.to_string())?;
    }
    writeln!(
        text,
        "Assigned altitude state at issue (sum denominator: {} frozen baseline issues): {:?}",
        plan.issues.len(),
        totals.assigned_altitude_states
    )
    .map_err(|e| e.to_string())?;
    writeln!(
        text,
        "Paired vertical scoring uses {} issues with an admitted simple assignment. Each horizon pairs issues with vertical scores from both forecasts and counts missing scores for each forecast.",
        totals.assigned_altitude_pair_eligible
    )
    .map_err(|e| e.to_string())?;
    for (mins, pair) in [5, 10, 20]
        .into_iter()
        .zip(&mut totals.assigned_altitude_pairs)
    {
        let paired = pair.assignment_free_abs_error_ft.len();
        let free = summarize(
            "assignment-free absolute vertical error (ft)",
            &mut pair.assignment_free_abs_error_ft,
            totals.assigned_altitude_pair_eligible,
        );
        let assigned = summarize(
            "assigned absolute vertical error (ft)",
            &mut pair.assigned_abs_error_ft,
            totals.assigned_altitude_pair_eligible,
        );
        writeln!(
            text,
            "{mins} min paired ({paired} matched)\n  {free}\n  {assigned}\n  improved {}, worsened {}, tied {}, missing assignment-free score {}, missing assigned score {}",
            pair.improved,
            pair.worsened,
            pair.tied,
            pair.missing_assignment_free,
            pair.missing_assigned
        )
        .map_err(|e| e.to_string())?;
    }
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
    writeln!(text, "route messages excluded: outside selected fix or repeated fix {}; ambiguous same-time routes {}; missing, old, on-ground or conflicting ADS-B state {}; source-flag stale ADS-B state {}; outside issue distance {}; unresolved forward route {}", plan.no_meter, plan.ambiguous_routes, plan.no_track, plan.stale_source_track, plan.outside_issue_window, plan.unusable_route).map_err(|e| e.to_string())?;
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

    fn vertical_score(errors: [Option<f64>; 3]) -> ArrivalScore {
        let mut score = ArrivalScore::default();
        for (horizon, error) in score.forecast.iter_mut().zip(errors) {
            horizon.vertical_error_ft = error;
        }
        score
    }

    #[test]
    fn paired_vertical_diagnostic_counts_direction_ties_and_one_sided_missing() {
        let mut totals = Totals::default();
        note_assignment_pair(
            &mut totals,
            Some(vertical_score([Some(100.0), Some(100.0), Some(100.0)])),
            Some(vertical_score([Some(90.0), Some(110.0), Some(100.0)])),
        );
        note_assignment_pair(
            &mut totals,
            None,
            Some(vertical_score([Some(10.0), None, Some(20.0)])),
        );

        assert_eq!(totals.assigned_altitude_pair_eligible, 2);
        let [five, ten, twenty] = &totals.assigned_altitude_pairs;
        assert_eq!((five.improved, five.worsened, five.tied), (1, 0, 0));
        assert_eq!(
            (five.missing_assignment_free, five.missing_assigned),
            (1, 0)
        );
        assert_eq!((ten.improved, ten.worsened, ten.tied), (0, 1, 0));
        assert_eq!((ten.missing_assignment_free, ten.missing_assigned), (1, 1));
        assert_eq!((twenty.improved, twenty.worsened, twenty.tied), (0, 0, 1));
        assert_eq!(
            (twenty.missing_assignment_free, twenty.missing_assigned),
            (1, 0)
        );
    }

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
            meter: fix,
            offered_flights: 1,
            no_addr: 0,
            no_meter: 0,
            ambiguous_routes: 0,
            no_track: 0,
            stale_source_track: 0,
            outside_issue_window: 0,
            unusable_route: 0,
            baseline_forecast_refused: BTreeMap::new(),
            raw_rate_diagnostic: None,
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
                report_position: 0,
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
mod issue_track_quality_tests {
    use super::*;

    fn reports(times_ms: &[i64], flags: &[u8]) -> Reports {
        Reports {
            time_ns: times_ms.iter().map(|&t| t * 1_000_000).collect(),
            lat_e7: vec![417_000_000; times_ms.len()],
            lon_e7: vec![-879_000_000; times_ms.len()],
            baro_alt_ft: vec![12_000; times_ms.len()],
            gs_dkt: vec![2_500; times_ms.len()],
            track_ddeg: vec![900; times_ms.len()],
            vrate_fpm: vec![NONE_I32; times_ms.len()],
            flags: flags.to_vec(),
            ..Reports::default()
        }
    }

    fn rows(count: usize) -> Vec<ReportIx> {
        (0..count).map(|i| ReportIx(i as u32)).collect()
    }

    #[test]
    fn selected_stale_source_report_is_refused_without_falling_back() {
        let cutoff = 1_000_000;
        let reports = reports(&[cutoff - 5_000, cutoff - 1_000], &[0, F_STALE]);
        assert!(matches!(
            track_at(&reports, &rows(2), cutoff),
            TrackSelection::SourceFlagStale
        ));
    }

    #[test]
    fn healthy_report_is_available_and_future_stale_report_is_after_cutoff() {
        let cutoff = 1_000_000;
        let healthy = reports(&[cutoff - 1_000], &[0]);
        assert!(matches!(
            track_at(&healthy, &rows(1), cutoff),
            TrackSelection::Available(_, _, _)
        ));

        let with_future = reports(&[cutoff - 1_000, cutoff + 1_000], &[0, F_STALE]);
        assert!(matches!(
            track_at(&with_future, &rows(2), cutoff),
            TrackSelection::Available(_, _, _)
        ));
    }
}

#[cfg(test)]
mod raw_rate_diagnostic_tests {
    use super::*;

    fn reports(
        times_ms: &[i64],
        altitudes_ft: &[i32],
        rates_fpm: &[i32],
        sources: &[u8],
    ) -> Reports {
        Reports {
            time_ns: times_ms.iter().map(|&t| t * 1_000_000).collect(),
            baro_alt_ft: altitudes_ft.to_vec(),
            vrate_fpm: rates_fpm.to_vec(),
            flags: vec![0; times_ms.len()],
            source: sources.to_vec(),
            ..Reports::default()
        }
    }

    fn rows(count: usize) -> Vec<ReportIx> {
        (0..count).map(|i| ReportIx(i as u32)).collect()
    }

    #[test]
    fn raw_pair_proxy_distinguishes_contradiction_and_agreement() {
        let base = 1_000_000;
        let contradictory = reports(
            &[base - 5_000, base],
            &[10_000, 10_000],
            &[NONE_I32, 6_144],
            &[0, 0],
        );
        assert_eq!(
            raw_rate_status(&contradictory, &rows(2), 1),
            RawRateStatus::Contradicts
        );

        let matching = reports(
            &[base - 5_000, base],
            &[10_000, 10_500],
            &[NONE_I32, 6_000],
            &[0, 0],
        );
        assert_eq!(
            raw_rate_status(&matching, &rows(2), 1),
            RawRateStatus::Agrees
        );
    }

    #[test]
    fn raw_pair_proxy_refuses_wrong_source_and_equal_time_history() {
        let base = 1_000_000;
        let wrong_source = reports(
            &[base - 5_000, base],
            &[10_000, 10_000],
            &[NONE_I32, 0],
            &[5, 0],
        );
        assert_eq!(
            raw_rate_status(&wrong_source, &rows(2), 1),
            RawRateStatus::PriorSourceNonAdsb
        );

        let ambiguous = reports(
            &[base - 5_000, base - 5_000, base],
            &[10_000, 10_000, 10_000],
            &[NONE_I32, NONE_I32, 0],
            &[0, 0, 0],
        );
        assert_eq!(
            raw_rate_status(&ambiguous, &rows(3), 2),
            RawRateStatus::PriorTimestampConflict
        );
    }

    #[test]
    fn raw_pair_proxy_names_geometric_and_unknown_source_inputs() {
        let base = 1_000_000;
        let mut geometric = reports(
            &[base - 5_000, base],
            &[10_000, 10_500],
            &[NONE_I32, 6_000],
            &[0, 0],
        );
        geometric.flags[1] = F_RATE_GEOM;
        assert_eq!(
            raw_rate_status(&geometric, &rows(2), 1),
            RawRateStatus::GeometricRate
        );

        let unknown = reports(
            &[base - 5_000, base],
            &[10_000, 10_500],
            &[NONE_I32, 6_000],
            &[0, SOURCE_UNKNOWN],
        );
        assert_eq!(
            raw_rate_status(&unknown, &rows(2), 1),
            RawRateStatus::UnknownSelectedSource
        );
    }

    #[test]
    fn raw_pair_proxy_does_not_read_future_rows() {
        let base = 1_000_000;
        let with_future = reports(
            &[base - 5_000, base, base + 1_000],
            &[10_000, 10_500, 20_000],
            &[NONE_I32, 6_000, -6_000],
            &[0, 0, 0],
        );
        assert_eq!(
            raw_rate_status(&with_future, &rows(3), 1),
            RawRateStatus::Agrees
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
