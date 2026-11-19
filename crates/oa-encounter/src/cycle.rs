//! One cycle of the record: load, scan, evaluate in order of separation, end. Core (pure).

use oa_geo::aeqd::{frame_new, project};

use crate::geom::{inside_volume, pair_class};
use crate::grid::{grid_build, grid_scan};
use crate::state::{log_push, EncounterState, Open};
use crate::types::{
    CycleStats, Encounter, EncounterLog, Obs, CLASS_ALT_UNKNOWN, CLASS_VOLUME_ONLY, ON_GROUND,
};

fn add(t: &mut CycleStats, s: &CycleStats) {
    t.cycles += s.cycles;
    t.out_of_order += s.out_of_order;
    t.tracks += s.tracks;
    t.rows_bad += s.rows_bad;
    t.rows_duplicate += s.rows_duplicate;
    t.rows_over_capacity += s.rows_over_capacity;
    t.grid_work += s.grid_work;
    t.tracks_unscanned += s.tracks_unscanned;
    t.candidates += s.candidates;
    t.candidates_dropped += s.candidates_dropped;
    t.evaluated += s.evaluated;
    t.not_evaluated += s.not_evaluated;
    t.ground_excluded += s.ground_excluded;
    t.in_volume += s.in_volume;
    t.opened += s.opened;
    t.closed += s.closed;
    t.open_dropped += s.open_dropped;
}

fn good(o: &Obs) -> bool {
    o.lat_deg.is_finite()
        && o.lon_deg.is_finite()
        && (-90.0..=90.0).contains(&o.lat_deg)
        && (-180.0..=180.0).contains(&o.lon_deg)
}

/// Copies the usable rows into `st.obs` in address order. A row with an address another row of the
/// cycle also has is left out with its twins: which of them is right is not known.
fn load(st: &mut EncounterState, rows: &[Obs], stats: &mut CycleStats) {
    st.scratch.clear();
    st.obs.clear();
    for o in rows {
        if !good(o) {
            stats.rows_bad += 1;
        } else if st.scratch.len() >= st.cfg.max_tracks {
            stats.rows_over_capacity += 1;
        } else {
            st.scratch.push(*o);
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

/// Exact geometry of one candidate pair, and the encounter it opens or extends.
fn evaluate(st: &mut EncounterState, i: usize, j: usize, t_ns: i64, stats: &mut CycleStats) {
    let (Some(oi), Some(oj)) = (st.obs.get(i).copied(), st.obs.get(j).copied()) else {
        return;
    };
    let frame = frame_new(oi.lat_deg, oi.lon_deg);
    let (x, y) = project(&frame, oj.lat_deg, oj.lon_deg);
    let h = oa_math::hypot(x, y);
    let v = if oi.alt_ft.is_finite() && oj.alt_ft.is_finite() {
        f64::from((oi.alt_ft - oj.alt_ft).abs())
    } else {
        f64::NAN
    };
    let cfg = st.cfg;
    if !inside_volume(&cfg, h, v) {
        return;
    }
    if oi.ground == ON_GROUND && oj.ground == ON_GROUND {
        stats.ground_excluded += 1;
        return;
    }
    stats.in_volume += 1;
    let key = pair_key(oi.addr, oj.addr);
    let at = match st.open.binary_search_by_key(&key, |o| o.key) {
        Ok(p) => p,
        Err(p) => {
            if st.open.len() >= cfg.max_open {
                stats.open_dropped += 1;
                return;
            }
            let e = Encounter {
                addr_a: oi.addr,
                addr_b: oj.addr,
                start_ns: t_ns,
                end_ns: t_ns,
                closest_ns: t_ns,
                min_h_m: f32::INFINITY,
                v_at_closest_ft: f32::NAN,
                min_scaled_m: f32::NAN,
                alt_ft: f32::NAN,
                lat_e7: 0,
                lon_e7: 0,
                ticks_inside: 0,
                ticks_alt_unknown: 0,
                ticks_unevaluated: 0,
                flags_a: 0,
                flags_b: 0,
                ground_either: 0,
                class: CLASS_ALT_UNKNOWN,
            };
            st.open.insert(p, Open { key, e });
            stats.opened += 1;
            p
        }
    };
    let Some(open) = st.open.get_mut(at) else {
        return;
    };
    let e = &mut open.e;
    e.end_ns = t_ns;
    e.ticks_inside = e.ticks_inside.saturating_add(1);
    if v.is_nan() {
        e.ticks_alt_unknown = e.ticks_alt_unknown.saturating_add(1);
    } else {
        let c = pair_class(&cfg, h, v).unwrap_or(CLASS_VOLUME_ONLY);
        if c < e.class {
            e.class = c;
        }
        let s = oa_math::hypot(h, v / cfg.volume_v_ft * cfg.volume_h_m) as f32;
        if e.min_scaled_m.is_nan() || s < e.min_scaled_m {
            e.min_scaled_m = s;
        }
    }
    let h32 = h as f32;
    if h32 < e.min_h_m {
        e.min_h_m = h32;
        e.closest_ns = t_ns;
        e.v_at_closest_ft = v as f32;
        e.alt_ft = if v.is_nan() {
            f32::NAN
        } else {
            (oi.alt_ft + oj.alt_ft) / 2.0
        };
        e.lat_e7 = oa_ieee::round((oi.lat_deg + oj.lat_deg) / 2.0 * 1.0e7) as i32;
        e.lon_e7 = oa_ieee::round(mid_lon(oi.lon_deg, oj.lon_deg) * 1.0e7) as i32;
        e.flags_a = oi.flags;
        e.flags_b = oj.flags;
        e.ground_either = u8::from(oi.ground == ON_GROUND || oj.ground == ON_GROUND);
    }
}

/// Keeps the open encounter of a pair alive for a tick whose geometry was not evaluated.
fn keep_alive(st: &mut EncounterState, a: u32, b: u32, t_ns: i64) {
    if let Ok(p) = st.open.binary_search_by_key(&pair_key(a, b), |o| o.key) {
        if let Some(o) = st.open.get_mut(p) {
            if o.e.end_ns != t_ns {
                o.e.end_ns = t_ns;
                o.e.ticks_unevaluated = o.e.ticks_unevaluated.saturating_add(1);
            }
        }
    }
}

/// Gives one cycle of tracker rows (every live track at the tick `t_ns`) to the record. Finished
/// encounters are appended to `log`. A cycle whose time is not after the previous one is refused
/// and counted. Never allocates; work is bounded by the budgets of the configuration, and what
/// they leave undone is counted in the result (and in [`crate::encounter_totals`]).
pub fn encounter_cycle(
    st: &mut EncounterState,
    t_ns: i64,
    rows: &[Obs],
    log: &mut EncounterLog,
) -> CycleStats {
    let mut stats = CycleStats::default();
    if st.started && t_ns <= st.last_ns {
        stats.out_of_order = 1;
        add(&mut st.totals, &stats);
        return stats;
    }
    st.started = true;
    st.last_ns = t_ns;
    stats.cycles = 1;
    load(st, rows, &mut stats);
    grid_build(st);
    st.cands.clear();
    grid_scan(st, &mut stats);
    st.cands.sort_unstable();
    stats.candidates = st.cands.len() as u64;
    let n_eval = st.cands.len().min(st.cfg.eval_budget);
    for k in 0..n_eval {
        let Some(c) = st.cands.get(k).copied() else {
            break;
        };
        evaluate(st, c.i as usize, c.j as usize, t_ns, &mut stats);
        stats.evaluated += 1;
    }
    stats.not_evaluated = (st.cands.len() - n_eval) as u64 + stats.candidates_dropped;
    for k in n_eval..st.cands.len() {
        let Some(c) = st.cands.get(k).copied() else {
            break;
        };
        let (Some(a), Some(b)) = (st.obs.get(c.i as usize), st.obs.get(c.j as usize)) else {
            continue;
        };
        let (a, b) = (a.addr, b.addr);
        keep_alive(st, a, b, t_ns);
    }
    if stats.tracks_unscanned > 0 {
        let first = st.obs.len() - (stats.tracks_unscanned as usize).min(st.obs.len());
        let from = st.obs.get(first).map_or(u32::MAX, |o| o.addr);
        // Tracks are in address order: an unscanned track has an address at or above `from`.
        for p in 0..st.open.len() {
            let Some(e) = st.open.get(p).map(|o| (o.e.addr_a, o.e.addr_b)) else {
                break;
            };
            if e.0 >= from || e.1 >= from {
                keep_alive(st, e.0, e.1, t_ns);
            }
        }
    }
    let hyst = st.cfg.hysteresis_ns;
    st.open.retain(|o| {
        if t_ns.saturating_sub(o.e.end_ns) >= hyst {
            log_push(log, &o.e);
            stats.closed += 1;
            false
        } else {
            true
        }
    });
    add(&mut st.totals, &stats);
    stats
}

/// Ends every open encounter at its last tick inside (the run is over) and returns how many.
pub fn encounter_finish(st: &mut EncounterState, log: &mut EncounterLog) -> u64 {
    let mut n = 0u64;
    for o in &st.open {
        log_push(log, &o.e);
        n += 1;
    }
    st.open.clear();
    st.totals.closed += n;
    n
}
