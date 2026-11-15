//! Truth construction: selection, legs, smoothing. Core (pure).

use oa_adsb::{
    category_observe, is_reserved_address, jump_verdict, reports_len, reports_row, CategoryState,
    JumpPair, JumpVerdict, Reports, F_NEW_LEG, F_STALE, NONE_U16, NONE_U8,
};

use crate::frame::{frame_angle, frame_new, project, Frame};
use crate::loo::{loo_finish, loo_leg, LooAcc};
use crate::quality::{nacp_epu_m, quality_check, QualityState};
use crate::smooth::{interp, smooth, LegView, SIGMA95};
use crate::{
    Fraction, Truth, TruthConfig, TruthError, TruthStats, MAX_FRAME_ARC_RAD, MAX_REPORTS,
    REM_DUPLICATE, REM_INVALID, REM_JUMP, REM_SHORT_LEG, SPLIT_FRAME, SPLIT_GAP, SPLIT_JUMP,
    SPLIT_NEW_LEG,
};

const NS_PER_MS: i64 = 1_000_000;
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
        if !v.is_finite() || v <= 0.0 {
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
    let a = ((p2 - p1) / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dl / 2.0).sin().powi(2);
    2.0 * EARTH_R_M * a.sqrt().min(1.0).asin()
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
    if n > MAX_REPORTS {
        return Err(TruthError::TooManyReports(n));
    }
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

/// An aircraft's rows with those equal in time put in a canonical order, and the rows of every
/// equal-time group that disagrees about the position.
struct Canon {
    rows: Vec<(u32, i64, u32)>,
    /// Input indexes of the rows to remove, sorted.
    quarantined: Vec<u32>,
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

/// Two reports of one aircraft at one instant that are further apart than the 95 % radius the
/// truth is allowed to have ([`crate::MAX_SIGMA95_H_M`], 50 m) cannot both be right: the whole
/// group is quarantined (counted as duplicate times). Closer ones are one position measured twice;
/// the first of them in the canonical order (lowest content key) is the one tried first, so the
/// same reports in any input order give the same truth.
fn canonical_ties(r: &Reports, rows: &[(u32, i64, u32)]) -> Canon {
    let mut rows: Vec<(u32, i64, u32)> = rows.to_vec();
    rows.sort_by_key(|&(_, t, i)| (t, tie_key(r, i), i));
    let mut quarantined = Vec::new();
    let mut at = 0usize;
    while at < rows.len() {
        let t = rows.get(at).map_or(0, |e| e.1);
        let end = at
            + rows
                .get(at..)
                .map_or(0, |s| s.iter().take_while(|e| e.1 == t).count());
        let group = rows.get(at..end).unwrap_or(&[]);
        // Only rows the selection could use count: a stale row or one from another source is
        // removed on its own account and is no evidence about the ADS-B position.
        let eligible: Vec<(u32, i64, u32)> = group
            .iter()
            .copied()
            .filter(|e| {
                let src = r.source.get(e.2 as usize).copied().unwrap_or(NONE_U8);
                let fl = r.flags.get(e.2 as usize).copied().unwrap_or(0);
                fl & F_STALE == 0 && (src == NONE_U8 || crate::ADSB_SOURCES.contains(&src))
            })
            .collect();
        let group = eligible.as_slice();
        if group.len() > 1 {
            let pos = |i: u32| {
                (
                    r.lat_e7.get(i as usize).copied().unwrap_or(0),
                    r.lon_e7.get(i as usize).copied().unwrap_or(0),
                )
            };
            let conflict = group.iter().any(|a| {
                group.iter().any(|b| {
                    let ((la, oa), (lb, ob)) = (pos(a.2), pos(b.2));
                    dist_m(la, oa, lb, ob) > crate::MAX_SIGMA95_H_M
                })
            });
            if conflict {
                quarantined.extend(group.iter().map(|e| e.2));
            }
        }
        at = end.max(at + 1);
    }
    quarantined.sort_unstable();
    Canon { rows, quarantined }
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
    let canon = canonical_ties(r, rows);
    let (rows, quarantined) = (canon.rows.as_slice(), canon.quarantined.as_slice());
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
    for &(_, t, i) in rows {
        let Some(row) = reports_row(r, i as usize) else {
            count_removed(b, REM_INVALID);
            continue;
        };
        if row.flags & F_NEW_LEG != 0 {
            pending_new_leg = true;
        }
        if quarantined.binary_search(&i).is_ok() {
            count_removed(b, REM_DUPLICATE);
            continue;
        }
        let verdict = quality_check(&mut q, &row, t, cfg);
        let (fail, carried, nacp) = (verdict.removal, verdict.carried, verdict.nacp);
        if let Some(kind) = fail {
            count_removed(b, kind);
            continue;
        }
        // Past this point the row has passed the quality rules. It is judged by the category as
        // it would leave the state, which is kept only if the row is.
        let mut trial = cat;
        let category = category_observe(&mut trial, row.category);
        let last = buf.t.last().copied();
        let last_pos = buf.lat_e7.last().copied().zip(buf.lon_e7.last().copied());
        if last.is_some_and(|lt| t <= lt) {
            count_removed(b, REM_DUPLICATE);
            continue;
        }
        let mut split: Option<usize> = None;
        if let (Some(lt), Some((la, lo))) = (last, last_pos) {
            if pending_new_leg {
                split = Some(SPLIT_NEW_LEG);
            } else if t - lt > gap_ns {
                split = Some(SPLIT_GAP);
            } else {
                let pair = JumpPair {
                    dist_m: dist_m(la, lo, row.lat_e7, row.lon_e7),
                    dt_s: (t - lt) as f64 / 1.0e9,
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
                if SIGMA95 * p[0].max(0.0).sqrt() <= gate {
                    at_reports.hit += 1;
                }
            }
            if let (Some(&ta), Some(&tb)) = (buf.t.get(k), buf.t.get(k + 1)) {
                if tb - ta <= bracket2 {
                    if let Some((_, p)) = interp(&view, k, ta + (tb - ta) / 2, cfg.accel_q) {
                        at_mid.n += 1;
                        if SIGMA95 * p[0].max(0.0).sqrt() <= gate {
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
