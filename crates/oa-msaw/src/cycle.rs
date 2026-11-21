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
    alt_floor_ft, altitude_use, deg_to_e7, extrapolated_alt_ft, floor_of, grade_of, lowest_alt_ft,
    not_climbing, predicted_alt_ft, sample_points, sample_span_s, sample_time_s, track_deg_of,
    violates, violation_time_s, with_airport_area, AltUse, PathStart,
};
use crate::corridor::{corridor_hit_at, corridor_runway_elev_at};
use crate::state::{alert_push, skip_push, MsawState};
use crate::types::{
    Alert, AlertLog, CycleStats, FloorKind, FloorSample, Grade, MsawTrack, SamplePoint, SkipLog,
    SkipWhy, Skipped, AF_COAST, AF_COLD_CORRECTED, AF_COLD_HIGH, AF_COLD_UNKNOWN,
    AF_GROUND_UNKNOWN, AF_IDENTITY_UNCONFIRMED, AF_PARTIAL_COVERAGE, AF_QNH_MISSING, AF_RADAR_ONLY,
    AF_RATE_UNKNOWN, AF_STALE_EXTRAPOLATED, AF_UNCERTAIN, AF_WIDE_UNCERTAINTY, END_CLEAR,
    END_LOST_ALTITUDE_MISSING, END_LOST_ALTITUDE_STALE, FLOOR_NONE, F_COAST, F_COLD_CORRECTED,
    F_COLD_HIGH, F_COLD_UNKNOWN, F_IDENTITY_UNCONFIRMED, F_QNH_MISSING, F_RADAR_ONLY, F_UNCERTAIN,
    GROUND_UNKNOWN, KB_APPROACH, KB_DEPARTURE, KB_RUNWAY, MAX_ABS_ALT_FT, MAX_ABS_LAT_DEG,
    MAX_ABS_VRATE_FPM, MAX_R95_M, ON_GROUND, WIDE_R95_M,
};

/// Internal bit of a planned row's flags: the vertical rate was unknown and is taken as zero.
const IF_RATE_UNKNOWN: u16 = 0x100;
/// Internal bit of a planned row's flags: the altitude was stale within the hold and was extrapolated.
const IF_STALE_EXTRAPOLATED: u16 = 0x200;
const IF_WIDE: u16 = 0x400;

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
    a.alt_stale = a.alt_stale.saturating_add(b.alt_stale);
    a.alt_extrapolated = a.alt_extrapolated.saturating_add(b.alt_extrapolated);
    a.alt_age_unknown = a.alt_age_unknown.saturating_add(b.alt_age_unknown);
    a.alt_loss = a.alt_loss.saturating_add(b.alt_loss);
    a.landings = a.landings.saturating_add(b.landings);
    a.protection_lost = a.protection_lost.saturating_add(b.protection_lost);
    a.protection_lost_open = a
        .protection_lost_open
        .saturating_add(b.protection_lost_open);
    a.rate_unknown = a.rate_unknown.saturating_add(b.rate_unknown);
    a.rows_bad = a.rows_bad.saturating_add(b.rows_bad);
    a.rows_over_capacity = a.rows_over_capacity.saturating_add(b.rows_over_capacity);
    a.r95_held = a.r95_held.saturating_add(b.r95_held);
    a.wide = a.wide.saturating_add(b.wide);
    a.rollout_points = a.rollout_points.saturating_add(b.rollout_points);
    a.rollout_off_runway = a.rollout_off_runway.saturating_add(b.rollout_off_runway);
    a.wide_degrade = a.wide_degrade.saturating_add(b.wide_degrade);
    a.rows_lost_upstream = a.rows_lost_upstream.saturating_add(b.rows_lost_upstream);
    a.over_budget = a.over_budget.saturating_add(b.over_budget);
    a.evaluated = a.evaluated.saturating_add(b.evaluated);
    a.qnh_missing = a.qnh_missing.saturating_add(b.qnh_missing);
    a.below_mva = a.below_mva.saturating_add(b.below_mva);
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
/// every floor; degraded for a shortfall of the system only, with the reason of highest priority
/// (budget, then missing input) and the count of every shortfall of the cycle. A track with no
/// altitude or a stale one is a gap of that aircraft, not of the system: it is counted and listed
/// per track (`alt_missing`, `alt_stale`) and does not degrade the net, unless more than the
/// configured share of the airborne tracks lost their altitude in the cycle (`alt_loss`: a decoder
/// or a feed losing altitude wholesale), which reads as missing input.
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
        .saturating_add(s.rows_lost_upstream)
        .saturating_add(s.alt_loss)
        .saturating_add(s.wide_degrade);
    let all = work.saturating_add(input);
    let (reason, count) = if work > 0 {
        (Reason::BudgetExceeded, all)
    } else if input > 0 {
        (Reason::InputMissing, all)
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
            skip(skips, &mut stats, t_ns, r, SkipWhy::GroundExcluded);
            continue;
        }
        if r.alt_ft.is_nan() {
            stats.alt_missing = stats.alt_missing.saturating_add(1);
            skip(skips, &mut stats, t_ns, r, SkipWhy::AltitudeMissing);
            lose_protection(st, &mut stats, skips, t_ns, r, END_LOST_ALTITUDE_MISSING);
            continue;
        }
        let mut row = *r;
        if r.alt_age_s.is_nan() {
            stats.alt_age_unknown = stats.alt_age_unknown.saturating_add(1);
        }
        match altitude_use(r.alt_age_s, cfg.max_altitude_age_s, cfg.stale_hold_s) {
            AltUse::Fresh => {
                // Any age above nothing carries a known descent on: the measurement is of the
                // past. A rate that is not known carries it level here (it is fresh).
                let age = f64::from(r.alt_age_s);
                if age > 0.0 && age.is_finite() && !r.vrate_fpm.is_nan() {
                    row.alt_ft =
                        extrapolated_alt_ft(f64::from(r.alt_ft), f64::from(r.vrate_fpm), age)
                            as f32;
                }
            }
            AltUse::Lost => {
                stats.alt_stale = stats.alt_stale.saturating_add(1);
                skip(skips, &mut stats, t_ns, r, SkipWhy::AltitudeStale);
                lose_protection(st, &mut stats, skips, t_ns, r, END_LOST_ALTITUDE_STALE);
                continue;
            }
            AltUse::Extrapolated => {
                stats.alt_extrapolated = stats.alt_extrapolated.saturating_add(1);
                skip(skips, &mut stats, t_ns, r, SkipWhy::AltitudeExtrapolated);
                row.alt_ft = extrapolated_alt_ft(
                    f64::from(r.alt_ft),
                    f64::from(r.vrate_fpm),
                    f64::from(r.alt_age_s),
                ) as f32;
                row.flags |= IF_STALE_EXTRAPOLATED;
            }
        }
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
        // The radius is never shrunk: an unusable one takes the largest, one above it is held to it.
        if !(r95.is_finite() && (0.0..=MAX_R95_M).contains(&r95)) {
            stats.r95_held = stats.r95_held.saturating_add(1);
            row.r95_m = MAX_R95_M as f32;
        }
        if f64::from(row.r95_m) > WIDE_R95_M {
            stats.wide = stats.wide.saturating_add(1);
            row.flags |= IF_WIDE;
        }
        st.scratch.push(row);
    }
    // The net degrades when the tracks lost for altitude are more than the share of the airborne ones.
    let airborne = stats
        .rows
        .saturating_sub(stats.ground_excluded)
        .saturating_sub(stats.rows_bad)
        .saturating_sub(stats.rows_over_capacity);
    let lost = stats.alt_missing.saturating_add(stats.alt_stale);
    if airborne > 0 && (lost as f64) > cfg.altitude_loss_degrade_share * (airborne as f64) {
        stats.alt_loss = lost;
    }
    if airborne > 0 && (stats.wide as f64) > cfg.wide_degrade_share * (airborne as f64) {
        stats.wide_degrade = stats.wide;
    }
    // Over the budget the lowest predicted altitudes are kept (ties by address), the rest counted
    // and listed.
    if st.scratch.len() > cfg.track_budget {
        let la = cfg.look_ahead_s;
        // The lowest height above the ground first; a track whose ground is not given first of all.
        let key = |r: &MsawTrack| {
            let (alt, vr) = (f64::from(r.alt_ft), f64::from(r.vrate_fpm));
            let ground = f64::from(r.ground_ft);
            if ground.is_nan() {
                return f64::MIN;
            }
            alt - ground + oa_math::min_f64(0.0, predicted_alt_ft(0.0, vr, la))
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
            track_deg: -1,
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

/// A track whose altitude is lost (missing, or past the hold) has no protection: counted, whatever
/// the share of the tracks. If it has an open alert the alert is held open with the reason, not
/// closed as cleared, and the track says so; the alert ends when the altitude returns and the
/// track is clear, or when the track ends.
fn lose_protection(
    st: &mut MsawState,
    stats: &mut CycleStats,
    skips: &mut SkipLog,
    t_ns: i64,
    r: &MsawTrack,
    why: u8,
) {
    stats.protection_lost = stats.protection_lost.saturating_add(1);
    if let Ok(i) = st.open.binary_search_by_key(&r.addr, |a| a.addr) {
        if let Some(a) = st.open.get_mut(i) {
            a.end_reason = why;
            a.protect_ns = t_ns;
            stats.protection_lost_open = stats.protection_lost_open.saturating_add(1);
            skip(skips, stats, t_ns, r, SkipWhy::ProtectionLost);
        }
    }
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
        (IF_STALE_EXTRAPOLATED, AF_STALE_EXTRAPOLATED),
        (F_COLD_CORRECTED, AF_COLD_CORRECTED),
        (F_COLD_UNKNOWN, AF_COLD_UNKNOWN),
        (F_COLD_HIGH, AF_COLD_HIGH),
        (IF_WIDE, AF_WIDE_UNCERTAINTY),
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
    worst_at: (i32, i32, f32, u8),
    /// The kinds (`KB_*`) of the corridors that held the worst point for the track.
    worst_kinds: u8,
    partial: bool,
    /// Some point outside the corridors with a chart altitude is above the alert floor and the
    /// predicted altitude is below the chart altitude there.
    below_mva: bool,
    /// The path reached the runway elevation over the runway: a landing (the later points are
    /// the aircraft on the ground and are not judged).
    landed: bool,
    /// The points after a landing that were not judged (the roll-out), and those of them outside
    /// every runway strip (past the far end or off the runway).
    rollout: u32,
    rollout_off: u32,
}

fn judge(tr: &MsawTrack, pts: &[SamplePoint], floors: &[FloorSample], st: &MsawState) -> Verdict {
    let cfg = &st.cfg;
    let (alt, vr) = (f64::from(tr.alt_ft), f64::from(tr.vrate_fpm));
    let mut v = Verdict {
        ttv_s: None,
        worst_margin_ft: i32::MAX,
        worst_floor_ft: 0,
        worst_kind: FloorKind::Mva,
        worst_at: (0, 0, 0.0, 0),
        worst_kinds: 0,
        partial: false,
        below_mva: false,
        landed: false,
        rollout: 0,
        rollout_off: 0,
    };
    // The look-ahead is the terminal one where the track is now inside an approach capture box (A2.08)
    // and no departure corridor; runway strips, departure corridors, airport terminal areas and
    // `gtm_clearance` areas keep the general one, since a departure climbs toward terrain. A path
    // that leaves the box keeps that look-ahead.
    let limit = match pts.first().and_then(|p| corridor_hit_at(cfg, &st.cidx, p)) {
        Some(h) if h.kinds & KB_APPROACH != 0 && h.kinds & KB_DEPARTURE == 0 => {
            cfg.terminal_lookahead_s
        }
        _ => cfg.look_ahead_s,
    };
    // A point over the pavement is a landing only for a track that is not climbing (a known rate not
    // above zero and no rise over 10 s) and whose altitude is fresh, has a real setting (a cold
    // correction of a real setting is near zero at the runway): only then is the altitude as exact as
    // the runway elevation.
    let may_land = not_climbing(
        tr.flags & IF_RATE_UNKNOWN == 0,
        vr,
        f64::from(tr.alt_change_10s_ft),
    ) && tr.flags & (F_QNH_MISSING | IF_STALE_EXTRAPOLATED) == 0;
    for (k, (p, s)) in pts.iter().zip(floors.iter()).enumerate() {
        let hit = corridor_hit_at(cfg, &st.cidx, p);
        let (lo, hi) = sample_span_s(k, cfg.samples, cfg.look_ahead_s);
        if k > 0 && lo >= limit {
            continue;
        }
        let hi = oa_math::min_f64(hi, limit);
        let low = alt_floor_ft(lowest_alt_ft(alt, vr, lo, hi));
        // After a touchdown point the rest of the path is the roll-out, along the runway and off its
        // end: a straight line from the track's heading drifts off the pavement and below the
        // ground, and is no path the aircraft flies. A track that does not qualify (a climb, a
        // go-around, a low pass, no setting) never rolls out: every point of its path is judged.
        if v.landed {
            v.rollout += 1;
            if hit.is_none_or(|h| h.kinds & KB_RUNWAY == 0) {
                v.rollout_off += 1;
            }
            continue;
        }
        // The altitude at the point itself, not the lowest of its span.
        let at_point = alt_floor_ft(predicted_alt_ft(
            alt,
            vr,
            sample_time_s(k, cfg.samples, cfg.look_ahead_s),
        ));
        if may_land
            && corridor_runway_elev_at(cfg, &st.cidx, p, f64::from(tr.r95_m))
                .is_some_and(|e| at_point <= e)
        {
            // The path reaches the runway elevation over the pavement: a landing at this point
            // (and only this point: the points outside the runway are still judged).
            v.landed = true;
            continue;
        }
        let s = &with_airport_area(s, low);
        let Some((floor, kind)) = floor_of(s, hit, cfg) else {
            v.partial = true;
            continue;
        };
        if hit.is_none() && s.mva_ft != FLOOR_NONE && low < s.mva_ft {
            v.below_mva = true;
        }
        let margin = low.saturating_sub(floor);
        if margin < v.worst_margin_ft {
            v.worst_margin_ft = margin;
            v.worst_floor_ft = floor;
            v.worst_kind = kind;
            v.worst_at = (p.lat_e7, p.lon_e7, p.radius_m, k as u8);
            v.worst_kinds = hit.map_or(0, |h| h.kinds);
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
        worst_lat_e7: v.worst_at.0,
        worst_lon_e7: v.worst_at.1,
        worst_radius_m: v.worst_at.2,
        worst_k: v.worst_at.3,
        worst_ns: t_ns,
        worst_flags: flags,
        r95_m: tr.r95_m,
        vel_east_mps: tr.vel_east_mps,
        vel_north_mps: tr.vel_north_mps,
        ttv_min_s: ttv_s as f32,
        track_deg: track_deg_of(f64::from(tr.vel_east_mps), f64::from(tr.vel_north_mps)),
        worst_kinds: v.worst_kinds,
        cycles: 1,
        flags,
        end_reason: END_CLEAR,
        protect_ns: 0,
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
                    a.worst_lat_e7 = v.worst_at.0;
                    a.worst_lon_e7 = v.worst_at.1;
                    a.worst_radius_m = v.worst_at.2;
                    a.worst_k = v.worst_at.3;
                    a.worst_ns = t_ns;
                    a.worst_flags = flags;
                    a.worst_kinds = v.worst_kinds;
                }
                if (ttv_s as f32) < a.ttv_min_s {
                    a.ttv_min_s = ttv_s as f32;
                }
                a.cycles = a.cycles.saturating_add(1);
                a.flags |= flags;
                // The altitude is back and the track is in violation: the same alert goes on.
                a.end_reason = END_CLEAR;
                a.protect_ns = 0;
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
        // The altitude is back: a held alert is clear from now on (the hysteresis starts), or goes
        // on below if the track is in violation.
        if let Ok(i) = st.open.binary_search_by_key(&tr.addr, |a| a.addr) {
            if let Some(a) = st.open.get_mut(i) {
                if a.end_reason != END_CLEAR {
                    a.end_reason = END_CLEAR;
                    a.protect_ns = 0;
                    a.end_ns = t_ns;
                }
            }
        }
        let v = judge(tr, pts, fl, st);
        stats.evaluated = stats.evaluated.saturating_add(1);
        if v.partial {
            stats.no_coverage = stats.no_coverage.saturating_add(1);
            skip(skips, &mut stats, t_ns, tr, SkipWhy::NoCoverage);
        }
        if v.landed {
            stats.landings = stats.landings.saturating_add(1);
        }
        stats.rollout_points = stats.rollout_points.saturating_add(u64::from(v.rollout));
        stats.rollout_off_runway = stats
            .rollout_off_runway
            .saturating_add(u64::from(v.rollout_off));
        let Some(ttv) = v.ttv_s else {
            // An alert open for a track whose altitude is stale within the hold stays open: the
            // hysteresis does not close it on an altitude that is no longer measured.
            if tr.flags & IF_STALE_EXTRAPOLATED != 0 {
                if let Ok(i) = st.open.binary_search_by_key(&tr.addr, |a| a.addr) {
                    if let Some(a) = st.open.get_mut(i) {
                        a.end_ns = t_ns;
                    }
                }
            }
            if v.below_mva {
                stats.below_mva = stats.below_mva.saturating_add(1);
                skip(skips, &mut stats, t_ns, tr, SkipWhy::BelowMva);
            }
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
    // An alert of a track whose altitude is lost stays open while the track is seen so; when the track
    // is no longer seen it ends with the reason.
    let keeps = |a: &Alert| {
        if a.end_reason == END_CLEAR {
            a.end_ns.saturating_add(h) >= t_ns
        } else {
            a.protect_ns == t_ns
        }
    };
    for a in st.open.iter() {
        if !keeps(a) {
            stats.closed = stats.closed.saturating_add(1);
            if !alert_push(alerts, a) {
                stats.alerts_dropped = stats.alerts_dropped.saturating_add(1);
            }
        }
    }
    st.open.retain(keeps);
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
