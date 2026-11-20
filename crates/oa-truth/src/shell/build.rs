//! Truth construction: selection, legs, smoothing. Shell (B7). The output is a function of the
//! clip rows (distances use `oa_math`, so the same bits on every machine), but two things keep
//! the build out of core: a `thread_local!` counter of same-time distance evaluations (the test
//! hook `pair_evals_this_thread`), and the `oa_adsb` types it names (B8).

use oa_adsb::{
    category_observe, is_reserved_address, jump_verdict, reports_len, reports_row, CategoryState,
    JumpPair, JumpVerdict, Reports, F_NEW_LEG, NONE_I32, NONE_U16,
};

use crate::core::frame::{frame_angle, frame_new, project, Frame};
use crate::core::loo::{loo_finish, loo_leg, LooAcc};
use crate::core::quality::{nacp_epu_m, QualityState};
use crate::core::smooth::{interp, sigma95_h_m, smooth, LegView, SIGMA95};
use crate::shell::quality::quality_eval;
use crate::{
    Fraction, Truth, TruthConfig, TruthError, TruthStats, ALT_QUANTUM_FT, MAX_FRAME_ARC_RAD,
    MAX_REPORTS, MAX_SAME_TIME_DISTINCT, REM_AMBIGUOUS, REM_DUPLICATE, REM_INVALID, REM_JUMP,
    REM_NO_QUALITY, REM_OVERSIZED, REM_SHORT_LEG, SAME_TIME_CHI2, SPLIT_FRAME, SPLIT_GAP,
    SPLIT_JUMP, SPLIT_NEW_LEG,
};

const NS_PER_MS: i64 = 1_000_000;

thread_local! {
    static PAIR_EVALS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Distance evaluations made by same-time conflict checks on this thread so far (a test hook).
#[doc(hidden)]
pub fn pair_evals_this_thread() -> u64 {
    PAIR_EVALS.with(std::cell::Cell::get)
}
const EARTH_R_M: f64 = 6_371_008.8;
/// Checks a configuration; the error names the first bad value.
fn check_config(c: &TruthConfig) -> Result<(), TruthError> {
    let bad = |w: &'static str| Err(TruthError::BadConfig(w));
    if c.min_nacp == 0 || c.min_nacp > 11 {
        return bad("min_nacp");
    }
    if c.min_nic > 11 || c.required_sil > 3 || c.min_version > 7 {
        return bad("min_nic, required_sil or min_version");
    }
    if c.quality_carry_ms < 0 || c.leg_gap_ms <= 0 || c.bracket_ms <= 0 {
        return bad("a time gate");
    }
    // Times are compared in ns; keep the products far from overflow.
    if c.quality_carry_ms.max(c.leg_gap_ms).max(c.bracket_ms) > 86_400_000 {
        return bad("a time gate over one day");
    }
    for (v, w) in [
        (c.max_sigma95_h_m, "max_sigma95_h_m"),
        (c.max_jump_kt, "max_jump_kt"),
        (c.accel_q, "accel_q"),
        (c.sigma_scale, "sigma_scale"),
    ] {
        // The covariance squares these; past the caps it overflows to infinity and NaN.
        let cap = match w {
            "accel_q" => crate::MAX_ACCEL_Q,
            "sigma_scale" => crate::MAX_SIGMA_SCALE,
            _ => f64::MAX,
        };
        if !v.is_finite() || v <= 0.0 || v > cap {
            return bad(w);
        }
    }
    if !c.jump_slack_m.is_finite() || c.jump_slack_m < 0.0 {
        return bad("jump_slack_m");
    }
    if c.jump_resync == 0 || c.min_leg_points < 3 || c.loo_stride < 2 {
        return bad("jump_resync, min_leg_points or loo_stride");
    }
    Ok(())
}

/// Reports of the leg being collected. A7.2: cold, transient scratch.
#[derive(Default)]
struct LegBuf {
    t: Vec<i64>,
    lat_e7: Vec<i32>,
    lon_e7: Vec<i32>,
    alt: Vec<i32>,
    vrate: Vec<i32>,
    sigma: Vec<f64>,
}

fn buf_clear(b: &mut LegBuf) {
    b.t.clear();
    b.lat_e7.clear();
    b.lon_e7.clear();
    b.alt.clear();
    b.vrate.clear();
    b.sigma.clear();
}

struct Builder<'a> {
    cfg: &'a TruthConfig,
    truth: Truth,
    stats: TruthStats,
    loo: LooAcc,
    /// The aircraft currently being built has produced a leg.
    ac_open: bool,
}

/// The great-circle distance between two positions in 1e-7 degree, metres (haversine on a
/// sphere; the 0.3 % ellipsoid error is far below what the jump check needs).
fn dist_m(lat1: i32, lon1: i32, lat2: i32, lon2: i32) -> f64 {
    let (p1, p2) = (deg(lat1).to_radians(), deg(lat2).to_radians());
    let dl = (deg(lon2) - deg(lon1)).to_radians();
    // oa_math, not the standard library's methods: the distance decides which same-time rows are
    // kept, so it must be the same bits on every machine (B6, B8).
    let s1 = oa_math::sin((p2 - p1) / 2.0);
    let s2 = oa_math::sin(dl / 2.0);
    let a = oa_math::powi(s1, 2) + oa_math::cos(p1) * oa_math::cos(p2) * oa_math::powi(s2, 2);
    2.0 * EARTH_R_M * oa_math::asin(a.sqrt().min(1.0))
}

fn deg(e7: i32) -> f64 {
    f64::from(e7) / 1.0e7
}

/// Builds the reference trajectories of `reports`. Returns the truth and what was done; see the
/// crate documentation for the rules. Refuses columns of different lengths, more than
/// [`MAX_REPORTS`] reports and an invalid configuration (E5).
pub fn build_truth(
    reports: &Reports,
    cfg: &TruthConfig,
) -> Result<(Truth, TruthStats), TruthError> {
    check_config(cfg)?;
    let n = reports_len(reports);
    check_report_count(n)?;
    if !columns_match(reports, n) {
        return Err(TruthError::ColumnLength);
    }
    let mut order: Vec<(u32, i64, u32)> = Vec::with_capacity(n);
    for i in 0..n {
        let (Some(&a), Some(&t)) = (reports.icao.get(i), reports.time_ns.get(i)) else {
            return Err(TruthError::ColumnLength);
        };
        order.push((a, t, i as u32));
    }
    // (address, time, input index) is a total order. Reports equal in address and time are put in
    // a canonical order of their own, by content, in `process_aircraft`, so the result does not
    // depend on input order at all.
    order.sort_unstable();
    let mut b = Builder {
        cfg,
        truth: Truth {
            cfg: *cfg,
            ..Truth::default()
        },
        stats: TruthStats {
            rows_in: n as u64,
            ..TruthStats::default()
        },
        loo: LooAcc::default(),
        ac_open: false,
    };
    let mut at = 0usize;
    while at < order.len() {
        let Some(&(addr, _, _)) = order.get(at) else {
            break;
        };
        let end = at
            + order
                .get(at..)
                .map_or(0, |s| s.iter().take_while(|e| e.0 == addr).count());
        let rows = order.get(at..end).unwrap_or(&[]);
        b.stats.aircraft_in += 1;
        b.ac_open = false;
        process_aircraft(&mut b, reports, addr, rows)?;
        at = end;
    }
    // Closing sentinels of the prefix-offset columns.
    b.truth
        .ac_first_leg
        .push(u32::try_from(b.truth.leg_addr.len()).map_err(|_| TruthError::Capacity)?);
    b.truth
        .leg_first_pt
        .push(u32::try_from(b.truth.pt_t_ns.len()).map_err(|_| TruthError::Capacity)?);
    b.stats.rows_kept = b.truth.pt_t_ns.len() as u64;
    b.stats.legs = b.truth.leg_addr.len() as u64;
    b.stats.aircraft_out = b.truth.ac_addr.len() as u64;
    if cfg.loo {
        b.stats.loo = loo_finish(&mut b.loo, cfg);
    }
    Ok((b.truth, b.stats))
}

/// Refuses more than [`MAX_REPORTS`] reports; exactly the limit is accepted.
fn check_report_count(n: usize) -> Result<(), TruthError> {
    if n > MAX_REPORTS {
        return Err(TruthError::TooManyReports(n));
    }
    Ok(())
}

fn columns_match(r: &Reports, n: usize) -> bool {
    [
        r.time_ns.len(),
        r.lat_e7.len(),
        r.lon_e7.len(),
        r.baro_alt_ft.len(),
        r.geom_alt_ft.len(),
        r.gs_dkt.len(),
        r.track_ddeg.len(),
        r.vrate_fpm.len(),
        r.flags.len(),
        r.source.len(),
        r.callsign.len(),
        r.squawk.len(),
        r.category.len(),
        r.nic.len(),
        r.nac_p.len(),
        r.nac_v.len(),
        r.sil.len(),
        r.sil_type.len(),
        r.version.len(),
        r.gva.len(),
        r.sda.len(),
        r.nic_baro.len(),
    ]
    .iter()
    .all(|&l| l == n)
}

fn count_removed(b: &mut Builder<'_>, kind: usize) {
    if let Some(c) = b.stats.removed.get_mut(kind) {
        *c += 1;
    }
}

fn count_split(b: &mut Builder<'_>, kind: usize) {
    if let Some(c) = b.stats.splits.get_mut(kind) {
        *c += 1;
    }
}

/// Content of a row that the selection reads, as a sort key: the order between rows equal in
/// time, so that it does not depend on where they were in the input.
fn tie_key(r: &Reports, i: u32) -> [i64; 17] {
    let i = i as usize;
    let g = |v: Option<&i32>| v.map_or(0, |&x| i64::from(x));
    let u = |v: Option<&u8>| v.map_or(0, |&x| i64::from(x));
    [
        g(r.lat_e7.get(i)),
        g(r.lon_e7.get(i)),
        g(r.baro_alt_ft.get(i)),
        g(r.geom_alt_ft.get(i)),
        r.gs_dkt.get(i).map_or(0, |&x| i64::from(x)),
        r.track_ddeg.get(i).map_or(0, |&x| i64::from(x)),
        r.vrate_fpm.get(i).map_or(0, |&x| i64::from(x)),
        u(r.flags.get(i)),
        u(r.source.get(i)),
        u(r.category.get(i)),
        u(r.nic.get(i)),
        u(r.nac_p.get(i)),
        u(r.nac_v.get(i)),
        u(r.sil.get(i)),
        u(r.sil_type.get(i)),
        u(r.version.get(i)),
        0,
    ]
}

/// Which fields a row carries, in the preference order for rows equal in time: barometric
/// altitude, geometric altitude, vertical rate, ground speed, track. Compared element by element,
/// a row with a valid value earlier in the list is preferred over one without it, whatever the
/// later fields say; the raw value of an absent field never takes part.
fn preference(r: &Reports, i: u32) -> [bool; 5] {
    let i = i as usize;
    [
        r.baro_alt_ft.get(i).is_some_and(|&x| x != NONE_I32),
        r.geom_alt_ft.get(i).is_some_and(|&x| x != NONE_I32),
        r.vrate_fpm.get(i).is_some_and(|&x| x != NONE_I32),
        r.gs_dkt.get(i).is_some_and(|&x| x != NONE_U16),
        r.track_ddeg.get(i).is_some_and(|&x| x != NONE_U16),
    ]
}

/// An aircraft's rows in time order, those equal in time put in the order of their [`preference`]
/// (more valid fields first), then of their content, then input index, so that the first row of
/// an instant is the preferred one and the result does not depend on where they were in the input.
fn canonical_rows(r: &Reports, rows: &[(u32, i64, u32)]) -> Vec<(u32, i64, u32)> {
    let mut rows: Vec<(u32, i64, u32)> = rows.to_vec();
    rows.sort_by_key(|&(_, t, i)| (t, std::cmp::Reverse(preference(r, i)), tie_key(r, i), i));
    rows
}

/// What to do with the rows of one aircraft at one instant, as (input index, removal reason)
/// sorted by index; rows not listed go on to the ordinary walk.
///
/// A row is eligible if the full selection ([`quality_eval`]: coordinates, NACp, NIC, SIL,
/// version, stale flag, source) passes against `snap`, the carried quality before the group, so
/// the order of the group cannot change who is eligible. An ineligible row is removed for its own
/// reason. Eligible rows with the same position and NACp are one report and are collapsed; if more
/// than [`MAX_SAME_TIME_DISTINCT`] distinct ones remain the group is refused (counted as
/// oversized). Otherwise two reports conflict when their squared horizontal distance exceeds
/// [`SAME_TIME_CHI2`] times the sum of their squared per-axis sigmas (NACp 95 % radius over
/// [`SIGMA95`]; independent errors assumed) and then all eligible rows of the group are
/// quarantined (counted as duplicate times). Closer ones are one position measured twice. Eligible
/// rows with two valid barometric (or two valid geometric) altitudes more than [`ALT_QUANTUM_FT`]
/// apart also contradict each other: all are quarantined (counted as ambiguous). Otherwise the
/// first of the group in [`preference`] order is the one kept.
fn same_time_check(
    r: &Reports,
    group: &[(u32, i64, u32)],
    snap: &QualityState,
    cfg: &TruthConfig,
) -> Vec<(u32, usize)> {
    let mut out: Vec<(u32, usize)> = Vec::new();
    // (lat, lon, NACp, input index) of the eligible rows.
    let mut elig: Vec<(i32, i32, u8, u32)> = Vec::new();
    // The valid barometric and geometric altitudes of the eligible rows.
    let (mut baro, mut geom): (Vec<i32>, Vec<i32>) = (Vec::new(), Vec::new());
    for &(_, t, i) in group {
        let Some(row) = reports_row(r, i as usize) else {
            continue;
        };
        let (v, _) = quality_eval(snap, &row, t, cfg);
        match (v.removal, v.nacp) {
            (Some(kind), _) => out.push((i, kind)),
            (None, Some(n)) => {
                elig.push((row.lat_e7, row.lon_e7, n, i));
                if row.baro_alt_ft != NONE_I32 {
                    baro.push(row.baro_alt_ft);
                }
                if row.geom_alt_ft != NONE_I32 {
                    geom.push(row.geom_alt_ft);
                }
            }
            (None, None) => out.push((i, REM_NO_QUALITY)),
        }
    }
    elig.sort_unstable();
    let mut distinct: Vec<(i32, i32, u8)> = elig.iter().map(|e| (e.0, e.1, e.2)).collect();
    distinct.dedup();
    let refuse = if distinct.len() > MAX_SAME_TIME_DISTINCT {
        Some(REM_OVERSIZED)
    } else {
        let sig2 = |n: u8| (nacp_epu_m(n) / SIGMA95).powi(2);
        let mut conflict = false;
        for (k, a) in distinct.iter().enumerate() {
            for b in distinct.iter().skip(k + 1) {
                PAIR_EVALS.with(|c| c.set(c.get() + 1));
                let d = dist_m(a.0, a.1, b.0, b.1);
                if d * d > SAME_TIME_CHI2 * (sig2(a.2) + sig2(b.2)) {
                    conflict = true;
                }
            }
        }
        let spread = |v: &[i32]| {
            let (lo, hi) = (v.iter().min(), v.iter().max());
            lo.zip(hi)
                .is_some_and(|(&l, &h)| i64::from(h) - i64::from(l) > i64::from(ALT_QUANTUM_FT))
        };
        if conflict {
            Some(REM_DUPLICATE)
        } else if spread(&baro) || spread(&geom) {
            Some(REM_AMBIGUOUS)
        } else {
            None
        }
    };
    if let Some(kind) = refuse {
        out.extend(elig.iter().map(|e| (e.3, kind)));
    }
    out.sort_unstable();
    out
}

/// Selects, splits and builds the legs of one aircraft; `rows` are in time order.
fn process_aircraft(
    b: &mut Builder<'_>,
    r: &Reports,
    addr: u32,
    rows: &[(u32, i64, u32)],
) -> Result<(), TruthError> {
    let cfg = b.cfg;
    // The reserved addresses (all zeros, all ones) are what a misconfigured transponder sends:
    // many aircraft share them, so their reports are not one aircraft's track.
    if is_reserved_address(addr) {
        if let Some(c) = b.stats.removed.get_mut(REM_INVALID) {
            *c += rows.len() as u64;
        }
        return Ok(());
    }
    let rows = canonical_rows(r, rows);
    let gap_ns = cfg.leg_gap_ms * NS_PER_MS;
    let mut q = QualityState::default();
    // Ground speed of the last kept report, and the emitter category trusted for the aircraft
    // (readsb prints it on a minority of rows; only kept reports count towards it, and a
    // cap-raising one must persist, see `oa_adsb::category_observe`).
    let mut last_gs = NONE_U16;
    let mut cat = CategoryState::default();
    let mut buf = LegBuf::default();
    let mut frame: Option<Frame> = None;
    let mut pending_new_leg = false;
    let mut consecutive_jumps = 0u32;
    let mut at = 0usize;
    while at < rows.len() {
        let t_group = rows.get(at).map_or(0, |e| e.1);
        let end = at
            + rows.get(at..).map_or(1, |s| {
                s.iter().take_while(|e| e.1 == t_group).count().max(1)
            });
        let group = rows.get(at..end).unwrap_or(&[]);
        at = end;
        // Same-time rows are judged together against the carried quality as it was before them.
        let forced = if group.len() > 1 {
            same_time_check(r, group, &q, cfg)
        } else {
            Vec::new()
        };
        for &(_, t, i) in group {
            let Some(row) = reports_row(r, i as usize) else {
                count_removed(b, REM_INVALID);
                continue;
            };
            if row.flags & F_NEW_LEG != 0 {
                pending_new_leg = true;
            }
            if let Ok(k) = forced.binary_search_by_key(&i, |e| e.0) {
                count_removed(b, forced.get(k).map_or(REM_DUPLICATE, |e| e.1));
                continue;
            }
            let (verdict, q_next) = quality_eval(&q, &row, t, cfg);
            let (fail, carried, nacp) = (verdict.removal, verdict.carried, verdict.nacp);
            let last = buf.t.last().copied();
            let last_pos = buf.lat_e7.last().copied().zip(buf.lon_e7.last().copied());
            let judged = |cat: &CategoryState| -> bool {
                // Whether the row is where the last kept report allows; a row that would start a new
                // leg cannot be judged and counts as fine.
                let (Some(lt), Some((la, lo))) = (last, last_pos) else {
                    return true;
                };
                if t <= lt {
                    return false;
                }
                // A difference past the range of an i64 is a gap.
                let Some(dt) = t.checked_sub(lt).filter(|&d| d <= gap_ns) else {
                    return true;
                };
                if pending_new_leg {
                    return true;
                }
                let mut c = *cat;
                let pair = JumpPair {
                    dist_m: dist_m(la, lo, row.lat_e7, row.lon_e7),
                    dt_s: dt as f64 / 1.0e9,
                    slack_m: cfg.jump_slack_m,
                    category: category_observe(&mut c, row.category),
                    gs_a_dkt: last_gs,
                    gs_b_dkt: row.gs_dkt,
                };
                jump_verdict(&pair, cfg.max_jump_kt) == JumpVerdict::Plausible
            };
            if let Some(kind) = fail {
                count_removed(b, kind);
                // A row that fails on quality still says what the transponder prints (a NACp of 7
                // must not be forgotten by the reports after it), unless it is a jump or one of
                // several rows of the instant.
                if group.len() == 1 && judged(&cat) {
                    q = q_next;
                }
                continue;
            }
            // Past this point the row has passed the quality rules. It is judged by the category as
            // it would leave the state, which is kept only if the row is.
            let mut trial = cat;
            let category = category_observe(&mut trial, row.category);
            if last.is_some_and(|lt| t <= lt) {
                count_removed(b, REM_DUPLICATE);
                continue;
            }
            let mut split: Option<usize> = None;
            if let (Some(lt), Some((la, lo))) = (last, last_pos) {
                if pending_new_leg {
                    split = Some(SPLIT_NEW_LEG);
                } else if let Some(dt) = t.checked_sub(lt).filter(|&d| d <= gap_ns) {
                    let pair = JumpPair {
                        dist_m: dist_m(la, lo, row.lat_e7, row.lon_e7),
                        dt_s: dt as f64 / 1.0e9,
                        slack_m: cfg.jump_slack_m,
                        category,
                        gs_a_dkt: last_gs,
                        gs_b_dkt: row.gs_dkt,
                    };
                    if jump_verdict(&pair, cfg.max_jump_kt) != JumpVerdict::Plausible {
                        consecutive_jumps += 1;
                        if consecutive_jumps >= cfg.jump_resync {
                            split = Some(SPLIT_JUMP);
                        } else {
                            count_removed(b, REM_JUMP);
                            continue;
                        }
                    }
                } else {
                    split = Some(SPLIT_GAP);
                }
            }
            if split.is_none() {
                if let (Some(f), true) = (frame.as_ref(), last.is_some()) {
                    if frame_angle(f, deg(row.lat_e7), deg(row.lon_e7)) > MAX_FRAME_ARC_RAD {
                        split = Some(SPLIT_FRAME);
                    }
                }
            }
            if let Some(kind) = split {
                flush_leg(b, addr, &mut buf, frame.take())?;
                count_split(b, kind);
            }
            consecutive_jumps = 0;
            q = q_next;
            cat = trial;
            pending_new_leg = false;
            last_gs = row.gs_dkt;
            if buf.t.is_empty() {
                frame = Some(frame_new(deg(row.lat_e7), deg(row.lon_e7)));
            }
            if carried {
                b.stats.quality_carried += 1;
            }
            let epu = nacp.map_or(f64::INFINITY, nacp_epu_m);
            buf.t.push(t);
            buf.lat_e7.push(row.lat_e7);
            buf.lon_e7.push(row.lon_e7);
            buf.alt.push(row.baro_alt_ft);
            buf.vrate.push(row.vrate_fpm);
            buf.sigma.push(epu / SIGMA95 * cfg.sigma_scale);
        }
    }
    flush_leg(b, addr, &mut buf, frame.take())?;
    Ok(())
}

/// Smooths the collected leg and appends it to the truth (or removes it as too short). Empties
/// `buf`.
fn flush_leg(
    b: &mut Builder<'_>,
    addr: u32,
    buf: &mut LegBuf,
    frame: Option<Frame>,
) -> Result<(), TruthError> {
    let cfg = b.cfg;
    let n = buf.t.len();
    let Some(frame) = frame.filter(|_| n >= cfg.min_leg_points as usize) else {
        if n > 0 {
            if let Some(c) = b.stats.removed.get_mut(REM_SHORT_LEG) {
                *c += n as u64;
            }
        }
        buf_clear(buf);
        return Ok(());
    };
    let mut ze = Vec::with_capacity(n);
    let mut zn = Vec::with_capacity(n);
    for (&la, &lo) in buf.lat_e7.iter().zip(buf.lon_e7.iter()) {
        let (e, nn) = project(&frame, deg(la), deg(lo));
        ze.push(e);
        zn.push(nn);
    }
    let sm = smooth(&buf.t, &ze, &zn, &buf.sigma, cfg.accel_q);
    if cfg.loo {
        loo_leg(
            &mut b.loo, cfg, &buf.t, &ze, &zn, &buf.sigma, &buf.alt, &buf.vrate,
        );
    }
    // Uncertainty at report times and at interval midpoints.
    {
        let view = LegView {
            t_ns: &buf.t,
            alt_ft: &buf.alt,
            vrate_fpm: &buf.vrate,
            xf: &sm.xf,
            pf: &sm.pf,
            xs: &sm.xs,
            ps: &sm.ps,
        };
        let gate = cfg.max_sigma95_h_m;
        let bracket2 = 2 * cfg.bracket_ms * NS_PER_MS;
        let mut at_reports = Fraction::default();
        let mut at_mid = Fraction::default();
        for k in 0..n {
            if let Some(p) = sm.ps.get(k) {
                at_reports.n += 1;
                if sigma95_h_m(p[0]).is_some_and(|s| s <= gate) {
                    at_reports.hit += 1;
                }
            }
            if let (Some(&ta), Some(&tb)) = (buf.t.get(k), buf.t.get(k + 1)) {
                if let Some(d) = tb.checked_sub(ta).filter(|&d| d <= bracket2) {
                    if let Some((_, p)) = interp(&view, k, ta + d / 2, cfg.accel_q) {
                        at_mid.n += 1;
                        if sigma95_h_m(p[0]).is_some_and(|s| s <= gate) {
                            at_mid.hit += 1;
                        }
                    }
                }
            }
        }
        b.stats.unc_at_reports.n += at_reports.n;
        b.stats.unc_at_reports.hit += at_reports.hit;
        b.stats.unc_at_midpoints.n += at_mid.n;
        b.stats.unc_at_midpoints.hit += at_mid.hit;
    }
    // Append to the truth.
    let t = &mut b.truth;
    if !b.ac_open {
        t.ac_addr.push(addr);
        t.ac_first_leg
            .push(u32::try_from(t.leg_addr.len()).map_err(|_| TruthError::Capacity)?);
        b.ac_open = true;
    }
    t.leg_addr.push(addr);
    t.leg_first_pt
        .push(u32::try_from(t.pt_t_ns.len()).map_err(|_| TruthError::Capacity)?);
    t.leg_lat0
        .push(deg(buf.lat_e7.first().copied().unwrap_or(0)));
    t.leg_lon0
        .push(deg(buf.lon_e7.first().copied().unwrap_or(0)));
    t.pt_t_ns.extend_from_slice(&buf.t);
    t.pt_alt_ft.extend_from_slice(&buf.alt);
    t.pt_vrate_fpm.extend_from_slice(&buf.vrate);
    t.pt_xf.extend_from_slice(&sm.xf);
    t.pt_pf.extend_from_slice(&sm.pf);
    t.pt_xs.extend_from_slice(&sm.xs);
    t.pt_ps.extend_from_slice(&sm.ps);
    buf_clear(buf);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // REQ: OA-TRUTH-050
    #[test]
    fn exactly_the_report_limit_is_accepted_and_one_more_is_refused() {
        assert_eq!(check_report_count(0), Ok(()));
        assert_eq!(check_report_count(MAX_REPORTS), Ok(()));
        assert_eq!(
            check_report_count(MAX_REPORTS + 1),
            Err(TruthError::TooManyReports(MAX_REPORTS + 1))
        );
    }
}
