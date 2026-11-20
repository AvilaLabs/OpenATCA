//! One cycle of the net, in two steps around the shell's lookups. Core (pure).
//!
//! 1. [`msaw_plan`] takes the cycle's tracks, leaves out the ones on the ground, with no altitude
//!    or with a bad value (each counted, and listed unless on the ground), keeps the tracks the
//!    budget allows, lowest predicted altitude first, and gives back the sample points of each predicted path.
//! 2. The shell looks up the terrain, obstacles and chart at every point (conservatively, within
//!    the point's radius) and hands the answers back in the same order.
//! 3. [`msaw_evaluate`] judges each path against the floors, hides what an inhibition covers
//!    (recording it), keeps one alert per track from first to last violating cycle and publishes
//!    the net's state.

use oa_safety::inhibit::{inhibit_suppress, InhibitTable};
use oa_safety::net::{state_report, Area, NetId, NetReport, Reason, StateTable};

use crate::calc::{
    alt_floor_ft, corridor_surface_at, deg_to_e7, floor_of, grade_of, lowest_alt_ft,
    predicted_alt_ft, sample_points, sample_span_s, violates, violation_time_s, PathStart,
};
use crate::state::{alert_push, skip_push, MsawState};
use crate::types::{
    Alert, AlertLog, CycleStats, FloorKind, FloorSample, Grade, MsawTrack, SamplePoint, SkipLog,
    SkipWhy, Skipped, AF_COAST, AF_GROUND_UNKNOWN, AF_IDENTITY_UNCONFIRMED, AF_PARTIAL_COVERAGE,
    AF_QNH_MISSING, AF_RADAR_ONLY, AF_RATE_UNKNOWN, AF_UNCERTAIN, F_COAST, F_IDENTITY_UNCONFIRMED,
    F_QNH_MISSING, F_RADAR_ONLY, F_UNCERTAIN, GROUND_UNKNOWN, MAX_ABS_ALT_FT, MAX_ABS_LAT_DEG,
    MAX_ABS_VRATE_FPM, ON_GROUND,
};

/// Internal bit of a planned row's flags: the vertical rate was unknown and is taken as zero.
const IF_RATE_UNKNOWN: u8 = 0x80;

/// What one cycle did.
// A7.2: cold struct, size guard waived.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CycleOut {
    pub stats: CycleStats,
    /// What was published to the net state table.
    pub report: NetReport,
}

fn add(a: &mut CycleStats, b: &CycleStats) {
    a.cycles = a.cycles.saturating_add(b.cycles);
    a.out_of_order = a.out_of_order.saturating_add(b.out_of_order);
    a.rows = a.rows.saturating_add(b.rows);
    a.ground_excluded = a.ground_excluded.saturating_add(b.ground_excluded);
    a.ground_unknown = a.ground_unknown.saturating_add(b.ground_unknown);
    a.alt_missing = a.alt_missing.saturating_add(b.alt_missing);
    a.rate_unknown = a.rate_unknown.saturating_add(b.rate_unknown);
    a.rows_bad = a.rows_bad.saturating_add(b.rows_bad);
    a.rows_over_capacity = a.rows_over_capacity.saturating_add(b.rows_over_capacity);
    a.r95_held = a.r95_held.saturating_add(b.r95_held);
    a.rows_lost_upstream = a.rows_lost_upstream.saturating_add(b.rows_lost_upstream);
    a.over_budget = a.over_budget.saturating_add(b.over_budget);
    a.evaluated = a.evaluated.saturating_add(b.evaluated);
    a.qnh_missing = a.qnh_missing.saturating_add(b.qnh_missing);
    a.no_coverage = a.no_coverage.saturating_add(b.no_coverage);
    a.violations = a.violations.saturating_add(b.violations);
    a.suppressed = a.suppressed.saturating_add(b.suppressed);
    a.opened = a.opened.saturating_add(b.opened);
    a.closed = a.closed.saturating_add(b.closed);
    a.open_full = a.open_full.saturating_add(b.open_full);
    a.alerts_dropped = a.alerts_dropped.saturating_add(b.alerts_dropped);
    a.skips_dropped = a.skips_dropped.saturating_add(b.skips_dropped);
    a.floor_mismatch = a.floor_mismatch.saturating_add(b.floor_mismatch);
    a.net_refused = a.net_refused.saturating_add(b.net_refused);
}

/// The net's report for a cycle (rules 1, 4 and 5): running when every track was evaluated with
/// every floor; degraded otherwise, with the reason of highest priority (budget, then missing
/// input, then missing altitude) and the count of every shortfall of the cycle.
pub fn net_report_of(s: &CycleStats) -> NetReport {
    let work = s
        .over_budget
        .saturating_add(s.rows_over_capacity)
        .saturating_add(s.alerts_dropped)
        .saturating_add(s.skips_dropped)
        .saturating_add(s.open_full);
    let input = s
        .no_coverage
        .saturating_add(s.rows_bad)
        .saturating_add(s.qnh_missing)
        .saturating_add(s.rows_lost_upstream);
    let all = work.saturating_add(input).saturating_add(s.alt_missing);
    let (reason, count) = if work > 0 {
        (Reason::BudgetExceeded, all)
    } else if input > 0 {
        (Reason::InputMissing, all)
    } else if s.alt_missing > 0 {
        (Reason::AltitudeMissing, s.alt_missing)
    } else {
        return NetReport::Running;
    };
    NetReport::Degraded {
        reason,
        area: Area::Whole,
        count: u32::try_from(count).unwrap_or(u32::MAX),
    }
}

fn publish(net: &mut StateTable, report: NetReport, t_ns: i64, stats: &mut CycleStats) {
    if state_report(net, NetId::Msaw, report, t_ns, t_ns).is_err() {
        stats.net_refused = stats.net_refused.saturating_add(1);
    }
}

/// Reports the net unavailable because its configuration was refused (it cannot run). Returns
/// false if the state table refused the report.
pub fn msaw_publish_refused(net: &mut StateTable, t_ns: i64) -> bool {
    let r = NetReport::Unavailable {
        reason: Reason::InputMissing,
    };
    state_report(net, NetId::Msaw, r, t_ns, t_ns).is_ok()
}

fn skip(skips: &mut SkipLog, stats: &mut CycleStats, t_ns: i64, tr: &MsawTrack, why: SkipWhy) {
    let s = Skipped {
        t_ns,
        addr: tr.addr,
        why,
        lat_e7: deg_to_e7(tr.lat_deg),
        lon_e7: deg_to_e7(tr.lon_deg),
    };
    if !skip_push(skips, &s) {
        stats.skips_dropped = stats.skips_dropped.saturating_add(1);
    }
}

fn row_usable(r: &MsawTrack, max_speed: f64) -> bool {
    let (ve, vn) = (f64::from(r.vel_east_mps), f64::from(r.vel_north_mps));
    if !(r.lat_deg.is_finite()
        && r.lon_deg.is_finite()
        && r.lat_deg.abs() <= MAX_ABS_LAT_DEG
        && r.lon_deg.abs() <= 180.0
        && ve.is_finite()
        && vn.is_finite()
        && oa_math::hypot(ve, vn) <= max_speed)
    {
        return false;
    }
    // NaN is "unknown" for altitude and rate; anything else must be a sane number.
    let alt = f64::from(r.alt_ft);
    let vr = f64::from(r.vrate_fpm);
    (alt.is_nan() || (alt.is_finite() && alt.abs() <= MAX_ABS_ALT_FT))
        && (vr.is_nan() || (vr.is_finite() && vr.abs() <= MAX_ABS_VRATE_FPM))
}

/// Plans a cycle at `t_ns` from the tracks `rows` (every live track): leaves out what cannot be
/// evaluated, listing it in `skips`, keeps within the track budget and fills the sample points
/// ([`crate::msaw_points`]). Returns the number of tracks planned. A cycle whose time is not after
/// the previous one is refused (and the evaluation publishes the net unavailable). Allocates
/// nothing; work is bounded by `max_tracks` rows and the sort of at most that many.
pub fn msaw_plan(st: &mut MsawState, t_ns: i64, rows: &[MsawTrack], skips: &mut SkipLog) -> usize {
    st.plan.clear();
    st.points.clear();
    st.scratch.clear();
    let mut stats = CycleStats::default();
    st.pending_t = t_ns;
    st.pending_refused = false;
    if st.started && t_ns <= st.last_ns {
        stats.out_of_order = 1;
        st.pending_refused = true;
        st.pending = stats;
        return 0;
    }
    st.started = true;
    st.last_ns = t_ns;
    stats.cycles = 1;
    stats.rows_lost_upstream = core::mem::take(&mut st.lost_input);
    let cfg = st.cfg;
    for (i, r) in rows.iter().enumerate() {
        stats.rows = stats.rows.saturating_add(1);
        if i >= cfg.max_tracks {
            stats.rows_over_capacity = stats.rows_over_capacity.saturating_add(1);
            skip(skips, &mut stats, t_ns, r, SkipWhy::OverCapacity);
            continue;
        }
        if !row_usable(r, cfg.max_speed_mps) {
            stats.rows_bad = stats.rows_bad.saturating_add(1);
            skip(skips, &mut stats, t_ns, r, SkipWhy::BadRow);
            continue;
        }
        if r.ground == ON_GROUND {
            stats.ground_excluded = stats.ground_excluded.saturating_add(1);
            continue;
        }
        if r.alt_ft.is_nan() {
            stats.alt_missing = stats.alt_missing.saturating_add(1);
            skip(skips, &mut stats, t_ns, r, SkipWhy::AltitudeMissing);
            continue;
        }
        let mut row = *r;
        if r.ground == GROUND_UNKNOWN {
            stats.ground_unknown = stats.ground_unknown.saturating_add(1);
        }
        if row.flags & F_QNH_MISSING != 0 {
            stats.qnh_missing = stats.qnh_missing.saturating_add(1);
            skip(skips, &mut stats, t_ns, r, SkipWhy::QnhMissing);
        }
        if row.vrate_fpm.is_nan() {
            stats.rate_unknown = stats.rate_unknown.saturating_add(1);
            row.vrate_fpm = 0.0;
            row.flags |= IF_RATE_UNKNOWN;
        }
        let r95 = f64::from(row.r95_m);
        if !(r95.is_finite() && r95 >= 0.0 && r95 <= cfg.r95_cap_m) {
            stats.r95_held = stats.r95_held.saturating_add(1);
            row.r95_m = cfg.r95_cap_m as f32;
        }
        st.scratch.push(row);
    }
    // Over the budget the lowest predicted altitudes are kept (ties by address), the rest counted
    // and listed.
    if st.scratch.len() > cfg.track_budget {
        let la = cfg.look_ahead_s;
        let key = |r: &MsawTrack| {
            let (alt, vr) = (f64::from(r.alt_ft), f64::from(r.vrate_fpm));
            alt + oa_math::min_f64(0.0, predicted_alt_ft(0.0, vr, la))
        };
        st.scratch.sort_unstable_by(|a, b| {
            oa_math::cmp_canon_f64(key(a), key(b)).then(a.addr.cmp(&b.addr))
        });
        for r in st.scratch.iter().skip(cfg.track_budget) {
            stats.over_budget = stats.over_budget.saturating_add(1);
            skip(skips, &mut stats, t_ns, r, SkipWhy::OverBudget);
        }
        st.scratch.truncate(cfg.track_budget);
    }
    st.scratch.sort_unstable_by_key(|a| a.addr);
    for r in st.scratch.iter() {
        let base = st.points.len();
        let start = PathStart {
            lat_deg: r.lat_deg,
            lon_deg: r.lon_deg,
            ve: f64::from(r.vel_east_mps),
            vn: f64::from(r.vel_north_mps),
            r95_m: f64::from(r.r95_m),
        };
        let blank = SamplePoint {
            lat_e7: 0,
            lon_e7: 0,
            radius_m: 0.0,
        };
        st.points.resize(base.saturating_add(cfg.samples), blank);
        let filled = st
            .points
            .get_mut(base..)
            .and_then(|out| sample_points(&start, &cfg, out));
        if filled == Some(cfg.samples) {
            st.plan.push(*r);
        } else {
            st.points.truncate(base);
            stats.rows_bad = stats.rows_bad.saturating_add(1);
            skip(skips, &mut stats, t_ns, r, SkipWhy::BadRow);
        }
    }
    st.pending = stats;
    st.plan.len()
}

fn flags_of(tr: &MsawTrack) -> u16 {
    let mut f = 0;
    for (bit, af) in [
        (F_COAST, AF_COAST),
        (F_UNCERTAIN, AF_UNCERTAIN),
        (F_RADAR_ONLY, AF_RADAR_ONLY),
        (F_IDENTITY_UNCONFIRMED, AF_IDENTITY_UNCONFIRMED),
        (IF_RATE_UNKNOWN, AF_RATE_UNKNOWN),
        (F_QNH_MISSING, AF_QNH_MISSING),
    ] {
        if tr.flags & bit != 0 {
            f |= af;
        }
    }
    if tr.ground == GROUND_UNKNOWN {
        f |= AF_GROUND_UNKNOWN;
    }
    f
}

/// What one track's path came to.
struct Verdict {
    /// Shortest time to violation, seconds; `None` if the path never goes below its floor.
    ttv_s: Option<f64>,
    worst_margin_ft: i32,
    worst_floor_ft: i32,
    worst_kind: FloorKind,
    partial: bool,
}

fn judge(tr: &MsawTrack, pts: &[SamplePoint], floors: &[FloorSample], st: &MsawState) -> Verdict {
    let cfg = &st.cfg;
    let (alt, vr) = (f64::from(tr.alt_ft), f64::from(tr.vrate_fpm));
    let mut v = Verdict {
        ttv_s: None,
        worst_margin_ft: i32::MAX,
        worst_floor_ft: 0,
        worst_kind: FloorKind::Mva,
        partial: false,
    };
    for (k, (p, s)) in pts.iter().zip(floors.iter()).enumerate() {
        let surface_ft = corridor_surface_at(cfg, p);
        let Some((floor, kind)) = floor_of(s, surface_ft, cfg) else {
            v.partial = true;
            continue;
        };
        let (lo, hi) = sample_span_s(k, cfg.samples, cfg.look_ahead_s);
        let low = alt_floor_ft(lowest_alt_ft(alt, vr, lo, hi));
        let margin = low.saturating_sub(floor);
        if margin < v.worst_margin_ft {
            v.worst_margin_ft = margin;
            v.worst_floor_ft = floor;
            v.worst_kind = kind;
        }
        if violates(low, floor) {
            let t = violation_time_s(alt, vr, floor, lo, hi);
            v.ttv_s = Some(v.ttv_s.map_or(t, |b| oa_math::min_f64(b, t)));
        }
    }
    v
}

/// Where and when a violation was found: the cycle time and the track's present position.
struct Found {
    t_ns: i64,
    lat_e7: i32,
    lon_e7: i32,
}

/// Opens or updates the alert of a track in violation, `ttv_s` seconds ahead.
fn raise(
    st: &mut MsawState,
    tr: &MsawTrack,
    v: &Verdict,
    ttv_s: f64,
    at: &Found,
    alerts: &mut AlertLog,
    stats: &mut CycleStats,
) {
    let Found {
        t_ns,
        lat_e7,
        lon_e7,
    } = *at;
    let grade = grade_of(ttv_s, st.cfg.warning_s);
    let flags = flags_of(tr) | if v.partial { AF_PARTIAL_COVERAGE } else { 0 };
    let fresh = Alert {
        addr: tr.addr,
        start_ns: t_ns,
        end_ns: t_ns,
        caution_ns: Some(t_ns),
        warning_ns: if grade == Grade::Warning {
            Some(t_ns)
        } else {
            None
        },
        peak: grade,
        lat_e7,
        lon_e7,
        alt_ft: tr.alt_ft,
        vrate_fpm: if tr.flags & IF_RATE_UNKNOWN != 0 {
            f32::NAN
        } else {
            tr.vrate_fpm
        },
        speed_mps: oa_math::hypot(f64::from(tr.vel_east_mps), f64::from(tr.vel_north_mps)) as f32,
        worst_margin_ft: v.worst_margin_ft as f32,
        worst_floor_ft: v.worst_floor_ft,
        worst_kind: v.worst_kind,
        ttv_min_s: ttv_s as f32,
        cycles: 1,
        flags,
    };
    match st.open.binary_search_by_key(&tr.addr, |a| a.addr) {
        Ok(i) => {
            if let Some(a) = st.open.get_mut(i) {
                a.end_ns = t_ns;
                if grade > a.peak {
                    a.peak = grade;
                }
                if grade == Grade::Warning && a.warning_ns.is_none() {
                    a.warning_ns = Some(t_ns);
                }
                if (v.worst_margin_ft as f32) < a.worst_margin_ft {
                    a.worst_margin_ft = v.worst_margin_ft as f32;
                    a.worst_floor_ft = v.worst_floor_ft;
                    a.worst_kind = v.worst_kind;
                }
                if (ttv_s as f32) < a.ttv_min_s {
                    a.ttv_min_s = ttv_s as f32;
                }
                a.cycles = a.cycles.saturating_add(1);
                a.flags |= flags;
            }
        }
        Err(i) => {
            if st.open.len() < st.cfg.max_open {
                st.open.insert(i, fresh);
                stats.opened = stats.opened.saturating_add(1);
            } else {
                // The open table is full: the alert is logged at once, never lost.
                stats.open_full = stats.open_full.saturating_add(1);
                if !alert_push(alerts, &fresh) {
                    stats.alerts_dropped = stats.alerts_dropped.saturating_add(1);
                }
            }
        }
    }
}

/// Judges the planned tracks against `floors` (one per point of [`crate::msaw_points`], in
/// order), raises and ends alerts, hides what an inhibition of the net covers (every hidden
/// violation is recorded in the inhibition table's suppressed-alert log) and publishes the net's
/// state to `net`. Finished alerts are appended to `alerts`, tracks not (fully) evaluated to
/// `skips`. Allocates nothing; work is the planned tracks times the samples.
pub fn msaw_evaluate(
    st: &mut MsawState,
    floors: &[FloorSample],
    alerts: &mut AlertLog,
    skips: &mut SkipLog,
    net: &mut StateTable,
    inh: &mut InhibitTable,
) -> CycleOut {
    let mut stats = core::mem::take(&mut st.pending);
    let t_ns = st.pending_t;
    if st.pending_refused {
        let report = NetReport::Unavailable {
            reason: Reason::SurveillanceLost,
        };
        publish(net, report, t_ns, &mut stats);
        add(&mut st.totals, &stats);
        return CycleOut { stats, report };
    }
    let n = st.cfg.samples;
    if floors.len() != st.points.len() {
        stats.floor_mismatch = 1;
        let report = NetReport::Unavailable {
            reason: Reason::InputMissing,
        };
        publish(net, report, t_ns, &mut stats);
        add(&mut st.totals, &stats);
        return CycleOut { stats, report };
    }
    let plan = core::mem::take(&mut st.plan);
    for (i, tr) in plan.iter().enumerate() {
        let (a, b) = (i.saturating_mul(n), i.saturating_add(1).saturating_mul(n));
        let (Some(pts), Some(fl)) = (st.points.get(a..b), floors.get(a..b)) else {
            continue;
        };
        let v = judge(tr, pts, fl, st);
        stats.evaluated = stats.evaluated.saturating_add(1);
        if v.partial {
            stats.no_coverage = stats.no_coverage.saturating_add(1);
            skip(skips, &mut stats, t_ns, tr, SkipWhy::NoCoverage);
        }
        let Some(ttv) = v.ttv_s else {
            continue;
        };
        stats.violations = stats.violations.saturating_add(1);
        let (lat_e7, lon_e7) = pts.first().map_or((0, 0), |p| (p.lat_e7, p.lon_e7));
        let s = inhibit_suppress(
            inh,
            NetId::Msaw,
            lat_e7,
            lon_e7,
            Some(alt_floor_ft(f64::from(tr.alt_ft))),
            t_ns,
        );
        if s.count > 0 {
            stats.suppressed = stats.suppressed.saturating_add(1);
            continue;
        }
        let at = Found {
            t_ns,
            lat_e7,
            lon_e7,
        };
        raise(st, tr, &v, ttv, &at, alerts, &mut stats);
    }
    st.plan = plan;
    // An alert whose track has been clear for the hysteresis ends.
    let h = st.cfg.hysteresis_ns;
    for a in st.open.iter() {
        if a.end_ns.saturating_add(h) < t_ns {
            stats.closed = stats.closed.saturating_add(1);
            if !alert_push(alerts, a) {
                stats.alerts_dropped = stats.alerts_dropped.saturating_add(1);
            }
        }
    }
    st.open.retain(|a| a.end_ns.saturating_add(h) >= t_ns);
    let report = net_report_of(&stats);
    publish(net, report, t_ns, &mut stats);
    add(&mut st.totals, &stats);
    CycleOut { stats, report }
}

/// Ends every open alert (after the last cycle), appending them to `alerts`. Returns how many the
/// log could not hold.
pub fn msaw_finish(st: &mut MsawState, alerts: &mut AlertLog) -> u64 {
    let mut dropped = 0u64;
    for a in st.open.iter() {
        st.totals.closed = st.totals.closed.saturating_add(1);
        if !alert_push(alerts, a) {
            dropped = dropped.saturating_add(1);
        }
    }
    st.open.clear();
    st.totals.alerts_dropped = st.totals.alerts_dropped.saturating_add(dropped);
    dropped
}
