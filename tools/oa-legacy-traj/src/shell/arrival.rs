//! Offline arrival experiment (shell, B3/C6). Binds recording rows at receipt time and passes
//! normalized issue inputs and separately sampled truth to the pure arrival core.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

use oa_adsb::{Reports, F_GROUND, F_RATE_GEOM, NONE_I32, NONE_U16};
use oa_bench::adsb_truth::{query, BuiltTruth};
use oa_bench::geo::{dist_m, enu, heading_deg, wrap180};
use oa_bench::legacy::{name, norm_id, tbfm_kind, Legacy, NONE, NO_COORD, NO_TIME};
use oa_bench::stats::nearest_rank;

use crate::core::arrival::{
    forecast, score, ArrivalScore, IssueSnapshot, LegacyEta, RouteFix, TrackState, TruthPoint,
};
use crate::core::types::Place;
use crate::shell::places::{place_of_e7, point_places, PlaceTable};

// C7: headroom over the two regional ADS-B days and the one day of route/TBFM recording in the
// development baseline. Check the volume before indexing and check keyed limits before inserting.
pub(crate) const MAX_REPORTS: usize = 12_000_000;
const MAX_CAPTURED_ROUTES: usize = 500_000;
const MAX_CAPTURED_TBFM: usize = 5_000_000;
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

// A7.2: one offline join record per received TBFM estimate for the selected fix.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct EtaRow {
    received_ms: i64,
    published_ms: i64,
    eta_ms: i64,
}

type Plan = [u8; 12];

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
    Some(RouteFix { name: fix, place })
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
            || !legacy
                .tbfm
                .fix
                .get(i)
                .is_some_and(|k| norm_id::<8>(k) == meter.name)
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
    eta: [Vec<f64>; 3],
    paired_oa_eta: [Vec<f64>; 2],
}

fn note(t: &mut Totals, s: ArrivalScore) {
    t.crossing += usize::from(s.actual_meter_ms.is_some());
    for ((h, v), result) in t.horizontal.iter_mut().zip(&mut t.vertical).zip(s.forecast) {
        h.extend(result.horizontal_error_m);
        v.extend(result.vertical_error_ft);
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

pub(crate) fn report(
    requested: &str,
    legacy: &Legacy,
    places: &PlaceTable,
    reports: &Reports,
    built: &BuiltTruth,
    header: &[(String, String)],
    sealed: u64,
) -> Result<String, String> {
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
    let tbfm = tbfm_index(legacy, meter)?;
    let mut totals = Totals::default();
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
    for routes in flights.values_mut() {
        routes.sort_by_key(|&(recv, row, _)| route_key(recv, row));
        let issue = routes
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
                    totals.ambiguous_routes += 1;
                    return None;
                }
                let addr = *legacy.routes.addr_at_receive.get(row.0 as usize)?;
                let Some((track, course)) =
                    adsb.get(&addr).and_then(|rows| track_at(reports, rows, at))
                else {
                    totals.no_track += 1;
                    return None;
                };
                if !(MIN_FROM_FIX_M..=MAX_FROM_FIX_M).contains(&distance(track.place, meter.place))
                {
                    totals.outside_issue_window += 1;
                    return None;
                }
                let Some(route) = route_ahead(legacy, places, row, meter_ix, track, course) else {
                    totals.unusable_route += 1;
                    return None;
                };
                Some((
                    IssueSnapshot {
                        cutoff_ms: at,
                        issued_ms: at,
                        track,
                        assigned_altitude_ft: None,
                        route,
                        meter_fix: Some(meter),
                    },
                    addr,
                    row,
                    meter_ix,
                ))
            });
        let Some((issue, addr, row, meter_ix)) = issue else {
            continue;
        };
        let at = issue.issued_ms;
        let predicted = match forecast(&issue) {
            Ok(predicted) => predicted,
            Err(refusal) => {
                *totals
                    .forecast_refused
                    .entry(format!("{refusal:?}"))
                    .or_default() += 1;
                continue;
            }
        };
        totals.issued += 1;
        let eram = legacy
            .route_points
            .est_ms
            .get(meter_ix)
            .copied()
            .filter(|&eta| eta > at)
            .map(|eta_ms| LegacyEta {
                issued_ms: at,
                eta_ms,
            });
        let tb = legacy
            .routes
            .plan_at_receive
            .get(row.0 as usize)
            .and_then(|p| tbfm.get(&norm_id(p)))
            .and_then(|entries| estimate_at(entries, at));
        match score(
            &predicted,
            &truth_path(built, addr, at),
            Some(meter),
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
        name(&meter.name)
    );
    for (label, value) in header {
        writeln!(text, "{label}: {value}").map_err(|e| e.to_string())?;
    }
    writeln!(text, "sealed-window rows dropped: {sealed}").map_err(|e| e.to_string())?;
    writeln!(text, "Selected flow: {requested}. AUTO ranks resolvable TBFM fixes by distinct ORD flight plans before scoring. One earliest eligible issue per flight, 40 to 200 NM from the fix, using the most recent unique ADS-B report within 30 s. Forecast uses received ERAM route geometry and ADS-B observation time. Legacy ETA values do not enter the model.").map_err(|e| e.to_string())?;
    let forecast_refused: usize = totals.forecast_refused.values().sum();
    let score_refused: usize = totals.score_refused.values().sum();
    writeln!(text, "offered flights with known address: {}; route messages without address: {no_addr}; issued: {}; scored: {}; without eligible state or route: {}; forecast refused: {forecast_refused}; score refused: {score_refused}; observed passages: {}", flights.len(), totals.issued, totals.issued.saturating_sub(score_refused), flights.len().saturating_sub(totals.issued + forecast_refused), totals.crossing).map_err(|e| e.to_string())?;
    for (kind, refusals) in [
        ("forecast", &totals.forecast_refused),
        ("score", &totals.score_refused),
    ] {
        for (reason, count) in refusals {
            writeln!(text, "{kind} refused: {reason} {count}").map_err(|e| e.to_string())?;
        }
    }
    writeln!(text, "route messages excluded: outside selected fix or repeated fix {no_meter}; ambiguous same-time routes {}; missing, stale or conflicting ADS-B state {}; outside issue distance {}; unresolved forward route {}", totals.ambiguous_routes, totals.no_track, totals.outside_issue_window, totals.unusable_route).map_err(|e| e.to_string())?;
    for ((mins, h), v) in [5, 10, 20]
        .into_iter()
        .zip(&mut totals.horizontal)
        .zip(&mut totals.vertical)
    {
        writeln!(
            text,
            "{}\n{}",
            summarize(&format!("{mins} min horizontal (m)"), h, totals.issued),
            summarize(&format!("{mins} min vertical (ft)"), v, totals.issued)
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
