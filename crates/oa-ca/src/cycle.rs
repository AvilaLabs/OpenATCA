//! One cycle of conflict alert: load, scan, evaluate in order of separation, keep the alerts,
//! publish the net's state. Core (pure).

use oa_geo::aeqd::{frame_new, project, try_frame_from_true_velocity_at};
use oa_safety::net::{state_report, Area, NetId, NetReport, Reason, StateTable};

use crate::grid::{exact_radius, grid_build, grid_scan, MAX_LAT_DEG};
use crate::solve::{predict, Prediction, Vertical};
use crate::state::{alert_push, AltRec, CaState, Cand};
use crate::types::{
    Alert, AlertLog, AltLost, CaConfig, CaTrack, CycleStats, EventLog, AF_ALT_UNKNOWN, AF_COAST,
    AF_IDENTITY_UNCONFIRMED, AF_ONE_GROUND, AF_RADAR_ONLY, AF_RATE_UNKNOWN, AF_UNCERTAIN,
    COAST_V_ALLOWANCE_FT, F_COAST, F_IDENTITY_UNCONFIRMED, F_RADAR_ONLY, F_UNCERTAIN,
    MAX_ABS_ALT_FT, MAX_ABS_VRATE_FPM, ON_GROUND,
};
use crate::volume::volume_minima;

/// What one cycle did and the state it published.
// A7.2: cold struct, size guard waived
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CycleOut {
    pub stats: CycleStats,
    /// The report given to the state table (M5 rule 1).
    pub report: NetReport,
    /// Everything left undone by the budgets and capacities of the cycle (pairs and tracks not
    /// evaluated, rows over capacity, alerts and events lost to a full table), whether or not
    /// an input cause was present too.
    pub work_short: u64,
    /// Rows or values left out or altered: bad, duplicate, above 85 degrees of latitude, with a
    /// radius held to the cap, or lost before reaching the cycle.
    pub input_short: u64,
}

fn add(t: &mut CycleStats, s: &CycleStats) {
    t.cycles += s.cycles;
    t.out_of_order += s.out_of_order;
    t.tracks += s.tracks;
    t.rows_bad += s.rows_bad;
    t.rows_duplicate += s.rows_duplicate;
    t.rows_over_capacity += s.rows_over_capacity;
    t.rows_high_lat += s.rows_high_lat;
    t.r95_clamped += s.r95_clamped;
    t.grid_work += s.grid_work;
    t.tracks_unscanned += s.tracks_unscanned;
    t.pairs_chord += s.pairs_chord;
    t.candidates += s.candidates;
    t.pairs_skipped_vertical += s.pairs_skipped_vertical;
    t.candidates_dropped += s.candidates_dropped;
    t.evaluated += s.evaluated;
    t.not_evaluated += s.not_evaluated;
    t.eval_failed += s.eval_failed;
    t.ground_excluded += s.ground_excluded;
    t.beyond_reach += s.beyond_reach;
    t.alt_unknown_pairs += s.alt_unknown_pairs;
    t.in_conflict += s.in_conflict;
    t.opened += s.opened;
    t.closed += s.closed;
    t.open_dropped += s.open_dropped;
    t.alt_lost += s.alt_lost;
    t.alt_lost_dropped += s.alt_lost_dropped;
    t.alerts_dropped += s.alerts_dropped;
    t.net_refused += s.net_refused;
    t.rate_unknown_pairs += s.rate_unknown_pairs;
    t.rows_lost_upstream = t.rows_lost_upstream.saturating_add(s.rows_lost_upstream);
    t.finish_dropped += s.finish_dropped;
}

fn row_ok(c: &CaConfig, o: &CaTrack) -> bool {
    let (e, n) = (f64::from(o.vel_east_mps), f64::from(o.vel_north_mps));
    let alt_ok =
        o.alt_ft.is_nan() || (o.alt_ft.is_finite() && f64::from(o.alt_ft.abs()) <= MAX_ABS_ALT_FT);
    let vr_ok = o.vrate_fpm.is_nan()
        || (o.vrate_fpm.is_finite() && f64::from(o.vrate_fpm.abs()) <= MAX_ABS_VRATE_FPM);
    o.lat_deg.is_finite()
        && o.lon_deg.is_finite()
        && (-90.0..=90.0).contains(&o.lat_deg)
        && (-180.0..=180.0).contains(&o.lon_deg)
        && e.is_finite()
        && n.is_finite()
        && e * e + n * n <= c.max_speed_mps * c.max_speed_mps
        && alt_ok
        && vr_ok
}

/// Copies the usable rows into `st.obs` in address order. A row with an address another row of the
/// cycle also has is left out with its twins: which of them is right is not known. A row's 95 %
/// radius that is not finite, is negative or is above the cap is held to the cap and counted. The
/// return value counts only finite radii above the cap, so the report can distinguish them from an
/// unusable radius without adding a field to the public cycle statistics.
fn load(st: &mut CaState, rows: &[CaTrack], stats: &mut CycleStats) -> u64 {
    st.scratch.clear();
    st.obs.clear();
    let cap = st.cfg.r95_cap_m;
    let mut above_cap = 0;
    for o in rows {
        if !row_ok(&st.cfg, o) {
            stats.rows_bad += 1;
        } else if st.scratch.len() >= st.cfg.max_tracks {
            stats.rows_over_capacity += 1;
        } else {
            let mut o = *o;
            let r = f64::from(o.r95_m);
            if !(r.is_finite() && (0.0..=cap).contains(&r)) {
                if r.is_finite() && r > cap {
                    above_cap += 1;
                }
                o.r95_m = cap as f32;
                stats.r95_clamped += 1;
            }
            st.scratch.push(o);
        }
    }
    st.scratch.sort_unstable_by_key(|o| o.addr);
    let n = st.scratch.len();
    let mut a = 0;
    while a < n {
        let addr = st.scratch.get(a).map_or(0, |o| o.addr);
        let mut b = a;
        while st.scratch.get(b).is_some_and(|o| o.addr == addr) {
            b += 1;
        }
        if b - a == 1 {
            if let Some(o) = st.scratch.get(a) {
                st.obs.push(*o);
            }
        } else {
            stats.rows_duplicate += (b - a) as u64;
        }
        a = b;
    }
    stats.tracks = st.obs.len() as u64;
    above_cap
}

/// True if a row with this address is in the cycle's loaded rows (they are in address order).
fn present(st: &CaState, addr: u32) -> bool {
    st.obs.binary_search_by_key(&addr, |o| o.addr).is_ok()
}

fn pair_key(a: u32, b: u32) -> u64 {
    u64::from(a) << 32 | u64::from(b)
}

/// Midpoint of two longitudes, degrees, across the antimeridian.
fn mid_lon(a: f64, b: f64) -> f64 {
    let mut d = b - a;
    if d > 180.0 {
        d -= 360.0;
    } else if d < -180.0 {
        d += 360.0;
    }
    let m = a + d / 2.0;
    if m > 180.0 {
        m - 360.0
    } else if m < -180.0 {
        m + 360.0
    } else {
        m
    }
}

fn track_flags(o: &CaTrack) -> u16 {
    let mut f = 0u16;
    for (bit, af) in [
        (F_COAST, AF_COAST),
        (F_UNCERTAIN, AF_UNCERTAIN),
        (F_RADAR_ONLY, AF_RADAR_ONLY),
        (F_IDENTITY_UNCONFIRMED, AF_IDENTITY_UNCONFIRMED),
    ] {
        if o.flags & bit != 0 {
            f |= af;
        }
    }
    if o.ground == ON_GROUND {
        f |= AF_ONE_GROUND;
    }
    f
}

/// The alert of a pair first seen in conflict at `t_ns`.
fn new_alert(a: &CaTrack, b: &CaTrack, t_ns: i64, p: &Prediction) -> Alert {
    Alert {
        addr_a: a.addr,
        addr_b: b.addr,
        start_ns: t_ns,
        end_ns: t_ns,
        peak_ns: t_ns,
        ttl_first_s: p.ttl_s as f32,
        ttl_peak_s: f32::INFINITY,
        h_now_peak_m: 0.0,
        h_miss_peak_m: f32::INFINITY,
        min_h_miss_m: f32::INFINITY,
        min_v_miss_ft: f32::NAN,
        h_eff_m: 0.0,
        v_eff_ft: f32::NAN,
        alt_ft: f32::NAN,
        lat_e7: 0,
        lon_e7: 0,
        ticks_conflict: 0,
        ticks_current: 0,
        ticks_unevaluated: 0,
        flags: 0,
        ground_either: 0,
    }
}

/// What the evaluation of a pair saw, for the alert it opens or extends.
// A7.2: transient, size guard waived
struct Seen {
    t_ns: i64,
    p: Prediction,
    h_now_m: f64,
    h_eff_m: f64,
    v_eff_ft: f64,
    alt_ft: f64,
    lat_e7: i32,
    lon_e7: i32,
    flags: u16,
    ground_either: bool,
}

/// Exact geometry of one candidate pair, and the alert it opens or extends. False if the pair
/// could not be evaluated (a projection failed).
fn evaluate(st: &mut CaState, i: usize, j: usize, t_ns: i64, stats: &mut CycleStats) -> bool {
    let (Some(oi), Some(oj), Some(fi)) = (
        st.obs.get(i).copied(),
        st.obs.get(j).copied(),
        st.frames.get(i).copied(),
    ) else {
        return false;
    };
    if oi.ground == ON_GROUND && oj.ground == ON_GROUND {
        stats.ground_excluded += 1;
        return true;
    }
    let (x, y) = project(&fi, oj.lat_deg, oj.lon_deg);
    let h_now = oa_ieee::sqrt(x * x + y * y);
    let Some(reach) = exact_radius(st, i, j) else {
        return false;
    };
    if !h_now.is_finite() {
        return false;
    }
    if h_now > reach {
        stats.beyond_reach += 1;
        return true;
    }
    let (Some((vix, viy)), Some((vjx, vjy))) = (
        try_frame_from_true_velocity_at(
            &fi,
            0.0,
            0.0,
            f64::from(oi.vel_east_mps),
            f64::from(oi.vel_north_mps),
        ),
        try_frame_from_true_velocity_at(
            &fi,
            x,
            y,
            f64::from(oj.vel_east_mps),
            f64::from(oj.vel_north_mps),
        ),
    ) else {
        return false;
    };
    let cfg = st.cfg;
    let alt_known = oi.alt_ft.is_finite() && oj.alt_ft.is_finite();
    let mid_alt = (f64::from(oi.alt_ft) + f64::from(oj.alt_ft)) / 2.0;
    let lat_e7 = oa_ieee::round((oi.lat_deg + oj.lat_deg) / 2.0 * 1.0e7) as i32;
    let lon_e7 = oa_ieee::round(mid_lon(oi.lon_deg, oj.lon_deg) * 1.0e7) as i32;
    let (h_min, v_min) = volume_minima(&cfg, lat_e7, lon_e7, alt_known.then_some(mid_alt));
    let h_eff = h_min + f64::from(oi.r95_m) + f64::from(oj.r95_m);
    let mut v_eff = v_min;
    for o in [&oi, &oj] {
        if o.flags & (F_COAST | F_UNCERTAIN) != 0 {
            v_eff += COAST_V_ALLOWANCE_FT;
        }
    }
    let mut flags = track_flags(&oi) | track_flags(&oj);
    // A rate is as needed as an altitude: an unknown one (the track has none, or the aircraft is
    // maybe not level) makes the vertical part of the test unsafe to assume, so the pair is
    // tested horizontally only and flagged, as with an unknown altitude.
    let rate_known = oi.vrate_fpm.is_finite() && oj.vrate_fpm.is_finite();
    let vert = if alt_known && rate_known {
        Some(Vertical {
            dz_ft: f64::from(oj.alt_ft) - f64::from(oi.alt_ft),
            dvz_fps: (f64::from(oj.vrate_fpm) - f64::from(oi.vrate_fpm)) / 60.0,
            v_eff_ft: v_eff,
        })
    } else {
        if alt_known {
            flags |= AF_RATE_UNKNOWN;
            stats.rate_unknown_pairs += 1;
        } else {
            flags |= AF_ALT_UNKNOWN;
            stats.alt_unknown_pairs += 1;
        }
        None
    };
    let Some(p) = predict(
        x,
        y,
        vjx - vix,
        vjy - viy,
        h_eff,
        vert.as_ref(),
        st.look_ahead_s,
    ) else {
        return true;
    };
    stats.in_conflict += 1;
    let seen = Seen {
        t_ns,
        p,
        h_now_m: h_now,
        h_eff_m: h_eff,
        v_eff_ft: if vert.is_some() { v_eff } else { f64::NAN },
        alt_ft: if alt_known { mid_alt } else { f64::NAN },
        lat_e7,
        lon_e7,
        flags,
        ground_either: oi.ground == ON_GROUND || oj.ground == ON_GROUND,
    };
    record(st, &oi, &oj, &seen, stats);
    true
}

/// Opens the alert of a pair in conflict or extends the one open.
fn record(st: &mut CaState, oi: &CaTrack, oj: &CaTrack, s: &Seen, stats: &mut CycleStats) {
    let key = pair_key(oi.addr, oj.addr);
    let at = match st
        .open
        .binary_search_by_key(&key, |a| pair_key(a.addr_a, a.addr_b))
    {
        Ok(p) => p,
        Err(p) => {
            if st.open.len() >= st.cfg.max_open {
                stats.open_dropped += 1;
                return;
            }
            st.open.insert(p, new_alert(oi, oj, s.t_ns, &s.p));
            stats.opened += 1;
            p
        }
    };
    let Some(a) = st.open.get_mut(at) else {
        return;
    };
    a.end_ns = s.t_ns;
    a.ticks_conflict = a.ticks_conflict.saturating_add(1);
    if s.p.now {
        a.ticks_current = a.ticks_current.saturating_add(1);
    }
    a.flags |= s.flags;
    let hm = s.p.h_miss_m as f32;
    if hm < a.min_h_miss_m {
        a.min_h_miss_m = hm;
    }
    if s.p.v_miss_ft.is_finite() {
        let vm = s.p.v_miss_ft as f32;
        if a.min_v_miss_ft.is_nan() || vm < a.min_v_miss_ft {
            a.min_v_miss_ft = vm;
        }
    }
    let ttl = s.p.ttl_s as f32;
    if ttl < a.ttl_peak_s || (ttl == a.ttl_peak_s && hm < a.h_miss_peak_m) {
        a.peak_ns = s.t_ns;
        a.ttl_peak_s = ttl;
        a.h_miss_peak_m = hm;
        a.h_now_peak_m = s.h_now_m as f32;
        a.h_eff_m = s.h_eff_m as f32;
        a.v_eff_ft = s.v_eff_ft as f32;
        a.alt_ft = s.alt_ft as f32;
        a.lat_e7 = s.lat_e7;
        a.lon_e7 = s.lon_e7;
        a.ground_either = u8::from(s.ground_either);
    }
}

/// Keeps the open alert of a pair alive for a cycle whose geometry was not evaluated.
fn keep_alive(st: &mut CaState, a: u32, b: u32, t_ns: i64) {
    if let Ok(p) = st
        .open
        .binary_search_by_key(&pair_key(a, b), |x| pair_key(x.addr_a, x.addr_b))
    {
        if let Some(o) = st.open.get_mut(p) {
            if o.end_ns != t_ns {
                o.end_ns = t_ns;
                o.ticks_unevaluated = o.ticks_unevaluated.saturating_add(1);
            }
        }
    }
}

/// Raises the loss of a known altitude (M5 rule 4), once per loss: a track whose previous cycle
/// had an altitude and whose row now has none. The memory is replaced by this cycle's rows, so
/// the event is raised again only after the altitude has come back and gone again.
fn alt_lost(st: &mut CaState, t_ns: i64, events: &mut EventLog, stats: &mut CycleStats) {
    st.alt_next.clear();
    for k in 0..st.obs.len() {
        let Some(o) = st.obs.get(k).copied() else {
            break;
        };
        if let Ok(p) = st.alt_prev.binary_search_by_key(&o.addr, |r| r.addr) {
            let before = st.alt_prev.get(p).map_or(f32::NAN, |r| r.alt_ft);
            if before.is_finite() && !o.alt_ft.is_finite() {
                stats.alt_lost += 1;
                if events.items.len() >= events.capacity {
                    events.overflow = events.overflow.saturating_add(1);
                    stats.alt_lost_dropped += 1;
                } else {
                    events.items.push(AltLost {
                        t_ns,
                        addr: o.addr,
                        last_alt_ft: before,
                        lat_e7: oa_ieee::round(o.lat_deg * 1.0e7) as i32,
                        lon_e7: oa_ieee::round(o.lon_deg * 1.0e7) as i32,
                        flags: o.flags,
                        ground: o.ground,
                    });
                }
            }
        }
        st.alt_next.push(AltRec {
            addr: o.addr,
            alt_ft: o.alt_ft,
        });
    }
    core::mem::swap(&mut st.alt_prev, &mut st.alt_next);
}

fn sat32(x: u64) -> u32 {
    u32::try_from(x).unwrap_or(u32::MAX)
}

/// The work a cycle left undone and the input it left out or altered, as counts: `(work, input)`.
fn shortfalls(s: &CycleStats) -> (u64, u64) {
    let work = s
        .not_evaluated
        .saturating_add(s.tracks_unscanned)
        .saturating_add(s.rows_over_capacity)
        .saturating_add(s.open_dropped)
        .saturating_add(s.alerts_dropped)
        .saturating_add(s.alt_lost_dropped);
    let input = s
        .rows_bad
        .saturating_add(s.rows_duplicate)
        .saturating_add(s.rows_high_lat)
        .saturating_add(s.r95_clamped)
        .saturating_add(s.rows_lost_upstream);
    (work, input)
}

/// True when finite radii above the cap are the only input shortfall for this cycle.
fn radius_capping_only(s: &CycleStats, above_cap: u64) -> bool {
    above_cap > 0
        && s.r95_clamped == above_cap
        && s.rows_bad == 0
        && s.rows_duplicate == 0
        && s.rows_high_lat == 0
        && s.rows_lost_upstream == 0
}

/// The net's report for a cycle (M5 rules 1 and 5): running when everything was evaluated;
/// degraded otherwise. The count is every shortfall of the cycle, work and input together (the
/// state table holds one reason, so it is the work's when there is any work left undone, else the
/// input's; [`CycleOut`] carries the two counts apart). Aggregate statistics alone cannot separate
/// finite over-cap radii from unusable radii, so this helper uses the generic input reason for
/// either; [`ca_cycle`] retains that distinction for its own report.
pub fn net_report_of(s: &CycleStats) -> NetReport {
    net_report_with_radius_cap(s, 0)
}

/// The cycle also knows how many of its held radii were finite and exceeded the configured cap;
/// that detail is kept out of the public aggregate statistics, so only `ca_cycle` passes it here.
fn net_report_with_radius_cap(s: &CycleStats, above_cap: u64) -> NetReport {
    let (work, input) = shortfalls(s);
    if work > 0 {
        NetReport::Degraded {
            reason: Reason::BudgetExceeded,
            area: Area::Whole,
            count: sat32(work.saturating_add(input)),
        }
    } else if input > 0 {
        NetReport::Degraded {
            reason: if radius_capping_only(s, above_cap) {
                Reason::RadiusCapped
            } else {
                Reason::InputMissing
            },
            area: Area::Whole,
            count: sat32(input),
        }
    } else {
        NetReport::Running
    }
}

fn publish(net: &mut StateTable, report: NetReport, t_ns: i64, stats: &mut CycleStats) {
    if state_report(net, NetId::ConflictAlert, report, t_ns, t_ns).is_err() {
        stats.net_refused += 1;
    }
}

/// Reports conflict alert unavailable because its configuration was refused (it cannot run).
/// Returns false if the state table refused the report.
pub fn ca_publish_refused(net: &mut StateTable, t_ns: i64) -> bool {
    let r = NetReport::Unavailable {
        reason: Reason::InputMissing,
    };
    state_report(net, NetId::ConflictAlert, r, t_ns, t_ns).is_ok()
}

/// Gives one cycle of tracker rows (every live track at the time `t_ns`) to conflict alert and
/// publishes the net's state to `net` (rule 1). Finished alerts are appended to `alerts` and
/// altitude losses to `events`. A cycle whose time is not after the previous one is refused,
/// counted and published as unavailable. Never allocates; work is bounded by the budgets of the
/// configuration, and what they leave undone is counted in the result and in [`crate::ca_totals`],
/// and published as degraded.
pub fn ca_cycle(
    st: &mut CaState,
    t_ns: i64,
    rows: &[CaTrack],
    alerts: &mut AlertLog,
    events: &mut EventLog,
    net: &mut StateTable,
) -> CycleOut {
    let mut stats = CycleStats::default();
    if st.started && t_ns <= st.last_ns {
        stats.out_of_order = 1;
        let report = NetReport::Unavailable {
            reason: Reason::SurveillanceLost,
        };
        publish(net, report, t_ns, &mut stats);
        add(&mut st.totals, &stats);
        return CycleOut {
            stats,
            report,
            work_short: 0,
            input_short: 0,
        };
    }
    st.started = true;
    st.last_ns = t_ns;
    stats.cycles = 1;
    stats.rows_lost_upstream = core::mem::take(&mut st.lost_input);
    let above_cap = load(st, rows, &mut stats);
    grid_build(st);
    // Rows above the latitude the cell search is shown for are used and counted.
    stats.rows_high_lat = st
        .obs
        .iter()
        .filter(|o| o.lat_deg.abs() > MAX_LAT_DEG)
        .count() as u64;
    st.frames.clear();
    for k in 0..st.obs.len() {
        let Some(o) = st.obs.get(k) else { break };
        let f = frame_new(o.lat_deg, o.lon_deg);
        st.frames.push(f);
    }
    st.cands.clear();
    grid_scan(st, &mut stats);
    sort_for_evaluation(st);
    let n_eval = st.cands.len().min(st.cfg.eval_budget);
    for k in 0..n_eval {
        let Some(c) = st.cands.get(k).copied() else {
            break;
        };
        if evaluate(st, c.i as usize, c.j as usize, t_ns, &mut stats) {
            stats.evaluated += 1;
        } else {
            stats.eval_failed += 1;
            keep_pair(st, &c, t_ns);
        }
    }
    stats.not_evaluated = stats.candidates - stats.evaluated;
    for k in n_eval..st.cands.len() {
        let Some(c) = st.cands.get(k).copied() else {
            break;
        };
        keep_pair(st, &c, t_ns);
    }
    if stats.tracks_unscanned > 0 {
        let first = st.obs.len() - (stats.tracks_unscanned as usize).min(st.obs.len());
        let from = st.obs.get(first).map_or(u32::MAX, |o| o.addr);
        // Tracks are in address order: an unscanned track has an address at or above `from`.
        for p in 0..st.open.len() {
            let Some(e) = st.open.get(p).map(|o| (o.addr_a, o.addr_b)) else {
                break;
            };
            if e.0 >= from || e.1 >= from {
                keep_alive(st, e.0, e.1, t_ns);
            }
        }
    }
    // A shortfall never ends or splits an alert: an open alert not evaluated this cycle is kept
    // alive, and marked, when its pair could have been lost to it. A dropped candidate may be any
    // pair of tracks that are here; a removed row may be any track that is not.
    let removed = stats.rows_duplicate + stats.rows_bad + stats.rows_over_capacity > 0;
    if removed || stats.candidates_dropped > 0 {
        for p in 0..st.open.len() {
            let Some(e) = st.open.get(p).map(|o| (o.addr_a, o.addr_b)) else {
                break;
            };
            let both = present(st, e.0) && present(st, e.1);
            if (stats.candidates_dropped > 0 && both) || (removed && !both) {
                keep_alive(st, e.0, e.1, t_ns);
            }
        }
    }
    alt_lost(st, t_ns, events, &mut stats);
    let hyst = st.cfg.hysteresis_ns;
    st.open.retain(|o| {
        if t_ns.saturating_sub(o.end_ns) >= hyst {
            if !alert_push(alerts, o) {
                stats.alerts_dropped += 1;
            }
            stats.closed += 1;
            false
        } else {
            true
        }
    });
    let report = net_report_with_radius_cap(&stats, above_cap);
    let (work_short, input_short) = shortfalls(&stats);
    publish(net, report, t_ns, &mut stats);
    add(&mut st.totals, &stats);
    CycleOut {
        stats,
        report,
        work_short,
        input_short,
    }
}

/// A lower bound on the time, seconds, before a candidate pair can be inside its horizontal
/// limit: the flat separation less the widest limit it can have (the largest of the volumes plus
/// both 95 % radii), over the sum of the two ground speeds, the speeds an unknown speed taken as
/// the configured maximum. Zero for a pair inside the limit now; infinite for a pair with no
/// speed that is outside it. The flat separation is itself approximate (the exact test is the
/// evaluation's), so this only orders the pairs: it never decides one.
fn time_to_reach(st: &CaState, c: &Cand) -> f64 {
    let (i, j) = (c.i as usize, c.j as usize);
    let speed = |k: usize| st.speed.get(k).copied().unwrap_or(st.cfg.max_speed_mps);
    let radius = |k: usize| {
        st.obs
            .get(k)
            .map_or(st.cfg.r95_cap_m, |o| f64::from(o.r95_m))
    };
    let wide = st.h_max_m + radius(i) + radius(j);
    let gap = oa_ieee::sqrt(c.d2 as f64) - wide;
    let closing = speed(i) + speed(j);
    if gap <= 0.0 {
        0.0
    } else if closing > 0.0 {
        gap / closing
    } else {
        f64::INFINITY
    }
}

/// Orders `st.cands` for the evaluation budget: by the least time before the pair can be inside
/// its limit ([`time_to_reach`]), then nearest first, then by track numbers (addresses). The
/// order is total, so it is the same on every run. A distant pair closing fast comes before a
/// near pair moving apart slowly, which is what a budget that stops early must not get wrong.
fn sort_for_evaluation(st: &mut CaState) {
    let mut cands = core::mem::take(&mut st.cands);
    cands.sort_unstable_by(|a, b| {
        oa_math::cmp_canon_f64(time_to_reach(st, a), time_to_reach(st, b)).then_with(|| a.cmp(b))
    });
    st.cands = cands;
}

fn keep_pair(st: &mut CaState, c: &Cand, t_ns: i64) {
    let (Some(a), Some(b)) = (st.obs.get(c.i as usize), st.obs.get(c.j as usize)) else {
        return;
    };
    let (a, b) = (a.addr, b.addr);
    keep_alive(st, a, b, t_ns);
}

/// Ends every open alert at its last cycle in conflict (the run is over) and returns how many.
/// Alerts the log cannot hold are counted ([`CycleStats::finish_dropped`], also in
/// `alerts_dropped`), and a final report of degraded is given to `net` at the last cycle's time:
/// they come after the last cycle's own report, so nothing else would show them.
pub fn ca_finish(st: &mut CaState, alerts: &mut AlertLog, net: &mut StateTable) -> u64 {
    let mut n = 0u64;
    let mut dropped = 0u64;
    for o in &st.open {
        if !alert_push(alerts, o) {
            dropped += 1;
        }
        n += 1;
    }
    st.open.clear();
    st.totals.closed += n;
    st.totals.alerts_dropped += dropped;
    st.totals.finish_dropped += dropped;
    if dropped > 0 && st.started {
        let r = NetReport::Degraded {
            reason: Reason::BudgetExceeded,
            area: Area::Whole,
            count: sat32(dropped),
        };
        if state_report(net, NetId::ConflictAlert, r, st.last_ns, st.last_ns).is_err() {
            st.totals.net_refused += 1;
        }
    }
    n
}
