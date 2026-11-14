//! Metrics F1 to F3, R1 and H1: time estimates, routes and handoffs against truth. Core (pure).
//!
//! **Truth crossing of a fix** (M2_DESIGN section 3): the time of closest approach of the truth
//! path to the fix, found on a coarse grid then refined, accepted only when the refined distance
//! is within the fix radius (2 NM) and the closest point is not the first or last point of a
//! truth leg (an aircraft that appears or disappears near a fix did not pass it). A flight that
//! never passes within the radius is excluded and counted.
//!
//! **Look-ahead** is the truth event time minus the time the estimate was issued, in 5-minute
//! bins to 60 minutes; the error is the estimate minus the truth time, signed seconds.

use std::collections::{BTreeMap, HashMap};

use crate::assoc::{Assoc, TruthIndex};
use crate::geo::{dist_m, enu, NM_M};
use crate::legacy::{hev, name, pad, tbfm_kind, tfms_kind, Legacy, NONE, NO_COORD, NO_TIME};
use crate::pair::PairTable;
use crate::stats::{exclude, group_rows, new_result, Exclusions, MetricResult, MAX_CASES};
use crate::truth::{AirportRef, Env, Truth};

/// Look-ahead bin names, 5 minutes each to 60.
pub const LOOKAHEAD_NAMES: [&str; 12] = [
    "0 to 5 min",
    "5 to 10 min",
    "10 to 15 min",
    "15 to 20 min",
    "20 to 25 min",
    "25 to 30 min",
    "30 to 35 min",
    "35 to 40 min",
    "40 to 45 min",
    "45 to 50 min",
    "50 to 55 min",
    "55 to 60 min",
];

/// Event metric settings (cold: A7.2).
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EventConfig {
    /// A truth path must pass within this of a meter fix, metres.
    pub fix_radius_m: f64,
    /// Coarse search step along a truth leg, ms.
    pub coarse_ms: i64,
    /// Refinement step, ms.
    pub fine_ms: i64,
    /// Width of a look-ahead bin, ms.
    pub bin_ms: i64,
    /// Landing or takeoff must be within this of a runway threshold, metres (1 NM).
    pub runway_radius_m: f64,
    /// ... and within this above the field elevation, feet.
    pub ground_margin_ft: i32,
    /// Handoff: how far either side of the event time to look for the boundary crossing, ms.
    pub handoff_window_ms: i64,
    /// Handoff: search step, ms.
    pub handoff_step_ms: i64,
    /// Route conformance: distance beyond which time counts as off-route, metres (2 NM).
    pub route_limit_m: f64,
    /// Route conformance: longest time one sample stands for, ms.
    pub route_max_dt_ms: i64,
}

impl Default for EventConfig {
    fn default() -> EventConfig {
        EventConfig {
            fix_radius_m: 2.0 * NM_M,
            coarse_ms: 5_000,
            fine_ms: 250,
            bin_ms: 300_000,
            runway_radius_m: NM_M,
            ground_margin_ft: 200,
            handoff_window_ms: 900_000,
            handoff_step_ms: 10_000,
            route_limit_m: 2.0 * NM_M,
            route_max_dt_ms: 60_000,
        }
    }
}

/// What the time-estimate metrics need besides the legacy tables (cold: A7.2).
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy)]
pub struct EventCtx<'a> {
    pub assoc: &'a Assoc,
    pub truth: &'a Truth<'a>,
    pub tix: &'a TruthIndex,
    pub env: &'a Env<'a>,
    pub cfg: &'a EventConfig,
}

fn legs_of<'a>(
    truth: &'a Truth<'_>,
    tix: &TruthIndex,
    addr: u32,
) -> Vec<&'a crate::truth::TruthLeg> {
    tix.by_addr
        .get(&addr)
        .map(|v| {
            v.iter()
                .filter_map(|&i| truth.legs.get(i as usize))
                .collect()
        })
        .unwrap_or_default()
}

/// The truth crossing time of a fix by aircraft `addr`, Unix ms, or the reason there is none.
pub fn crossing_time(
    truth: &Truth<'_>,
    tix: &TruthIndex,
    addr: u32,
    lat: f64,
    lon: f64,
    cfg: &EventConfig,
) -> Option<i64> {
    let mut best: Option<(f64, i64, i64, i64)> = None;
    for l in legs_of(truth, tix, addr) {
        let mut t = l.start_ms;
        while t <= l.end_ms {
            if let Ok(s) = (truth.query)(addr, t) {
                let d = dist_m(lat, lon, s.lat, s.lon);
                if best.is_none_or(|b| d < b.0) {
                    best = Some((d, t, l.start_ms, l.end_ms));
                }
            }
            t += cfg.coarse_ms.max(1);
        }
    }
    let (d0, t0, ls, le) = best?;
    if d0 > cfg.fix_radius_m + 2_000.0 {
        return None;
    }
    let (mut bd, mut bt) = (f64::MAX, t0);
    let mut t = t0 - cfg.coarse_ms;
    while t <= t0 + cfg.coarse_ms {
        if t >= ls && t <= le {
            if let Ok(s) = (truth.query)(addr, t) {
                let d = dist_m(lat, lon, s.lat, s.lon);
                if d < bd {
                    bd = d;
                    bt = t;
                }
            }
        }
        t += cfg.fine_ms.max(1);
    }
    if bd > cfg.fix_radius_m || bt - ls < cfg.coarse_ms || le - bt < cfg.coarse_ms {
        return None;
    }
    Some(bt)
}

fn lookahead_bin(cfg: &EventConfig, ahead_ms: i64) -> Option<u8> {
    if ahead_ms < 0 {
        return None;
    }
    let b = ahead_ms / cfg.bin_ms.max(1);
    (b < LOOKAHEAD_NAMES.len() as i64).then_some(b as u8)
}

fn bin_result(id: &'static str, title: &'static str, vals: &[f64], bins: &[u8]) -> MetricResult {
    let mut r = new_result(id, title, "s");
    r.rows = group_rows(vals, &[("lookahead", bins, &LOOKAHEAD_NAMES)]);
    r
}

/// F1 (meter-fix ETA) and F2 (scheduled time) of TBFM against the truth crossing of the meter
/// fix. F2 uses `sta_sfx` against the crossing of the section's meter fix: the feed has no
/// `sta_mfx`, and whether the scheduling fix is the meter fix is unconfirmed
/// (docs/TBFM_SURVEY.md), so F2 carries that note.
pub fn f1_f2_tbfm(
    leg: &Legacy,
    plan: &HashMap<[u8; 12], u32>,
    ctx: &EventCtx<'_>,
    ex: &mut Exclusions,
) -> (MetricResult, MetricResult) {
    let EventCtx {
        assoc,
        truth,
        tix,
        env,
        cfg,
    } = *ctx;
    let (mut v1, mut b1, mut v2, mut b2) = (vec![], vec![], vec![], vec![]);
    let mut cache: BTreeMap<(u32, [u8; 8]), Option<i64>> = BTreeMap::new();
    let t = &leg.tbfm;
    for i in 0..t.kind.len() {
        let kind = t.kind.get(i).copied().unwrap_or(0);
        let scope = match kind {
            tbfm_kind::ETA_MFX => "F1",
            tbfm_kind::STA_SFX => "F2",
            _ => continue,
        };
        let Some(&f) = t.plan_id.get(i).and_then(|p| plan.get(p)) else {
            exclude(ex, scope, "not matched to an SFDPS flight");
            continue;
        };
        let addr = assoc.addr.get(f as usize).copied().unwrap_or(NONE);
        if addr == NONE {
            exclude(ex, scope, "flight not associated with truth");
            continue;
        }
        let fix = t.fix.get(i).copied().unwrap_or([0; 8]);
        let Some((lat, lon)) = (env.fix)(name(&fix)) else {
            exclude(ex, scope, "meter fix not in the model");
            continue;
        };
        let cross = *cache
            .entry((addr, fix))
            .or_insert_with(|| crossing_time(truth, tix, addr, lat, lon, cfg));
        let Some(cross) = cross else {
            exclude(ex, scope, "truth never within 2 NM of the meter fix");
            continue;
        };
        let issue = t.issue_ms.get(i).copied().unwrap_or(NO_TIME);
        if issue == NO_TIME {
            exclude(ex, scope, "no issue time");
            continue;
        }
        let Some(bin) = lookahead_bin(cfg, cross - issue) else {
            exclude(
                ex,
                scope,
                "issued after the crossing or more than 60 min before it",
            );
            continue;
        };
        let err = (t.time_ms.get(i).copied().unwrap_or(0) - cross) as f64 / 1000.0;
        if kind == tbfm_kind::ETA_MFX {
            v1.push(err);
            b1.push(bin);
        } else {
            v2.push(err);
            b2.push(bin);
        }
    }
    let r1 = bin_result(
        "F1",
        "TBFM meter-fix ETA error (estimate minus truth crossing)",
        &v1,
        &b1,
    );
    let mut r2 = bin_result(
        "F2",
        "TBFM scheduled time error (sta_sfx minus truth crossing)",
        &v2,
        &b2,
    );
    r2.notes.push(
        "the feed has no STA at the meter fix; sta_sfx is compared with the crossing of the section's meter fix (unconfirmed, docs/TBFM_SURVEY.md)".into(),
    );
    (r1, r2)
}

fn near_threshold(ap: &AirportRef, lat: f64, lon: f64, radius: f64) -> bool {
    ap.thresholds
        .iter()
        .any(|&(a, b)| dist_m(a, b, lat, lon) <= radius)
}

fn on_ground_at_airport(s: &crate::truth::TruthSample, ap: &AirportRef, cfg: &EventConfig) -> bool {
    near_threshold(ap, s.lat, s.lon, cfg.runway_radius_m)
        && s.baro_alt_ft
            .is_none_or(|a| a <= ap.elev_ft + cfg.ground_margin_ft)
}

/// Truth landing time at `ap`: the last truth point of a leg that ends on the ground within the
/// radius of a threshold; looks back up to 30 s from the leg end for the last bracketed time.
pub fn landing_time(
    truth: &Truth<'_>,
    tix: &TruthIndex,
    addr: u32,
    ap: &AirportRef,
    cfg: &EventConfig,
) -> Option<i64> {
    let mut best: Option<i64> = None;
    for l in legs_of(truth, tix, addr) {
        for back in 0..=30 {
            let t = l.end_ms - back * 1000;
            if t < l.start_ms {
                break;
            }
            if let Ok(s) = (truth.query)(addr, t) {
                if on_ground_at_airport(&s, ap, cfg) {
                    best = best.max(Some(t));
                }
                break;
            }
        }
    }
    best
}

/// Truth takeoff time at `ap`: the first truth point of a leg that begins on the ground within
/// the radius of a threshold; looks forward up to 30 s from the leg start.
pub fn takeoff_time(
    truth: &Truth<'_>,
    tix: &TruthIndex,
    addr: u32,
    ap: &AirportRef,
    cfg: &EventConfig,
) -> Option<i64> {
    let mut best: Option<i64> = None;
    for l in legs_of(truth, tix, addr) {
        for fwd in 0..=30 {
            let t = l.start_ms + fwd * 1000;
            if t > l.end_ms {
                break;
            }
            if let Ok(s) = (truth.query)(addr, t) {
                if on_ground_at_airport(&s, ap, cfg) {
                    best = Some(best.map_or(t, |b| b.min(t)));
                }
                break;
            }
        }
    }
    best
}

/// F3: TFMS estimated arrival time against the truth landing, and estimated departure time
/// against the truth takeoff, by look-ahead. Matching a TFMS entry to an SFDPS flight is by the
/// shared flight plan identifier (`gufi`).
pub fn f3_tfms(
    leg: &Legacy,
    plan: &HashMap<[u8; 12], u32>,
    ctx: &EventCtx<'_>,
    ex: &mut Exclusions,
) -> (MetricResult, MetricResult) {
    let EventCtx {
        assoc,
        truth,
        tix,
        env,
        cfg,
    } = *ctx;
    let (mut va, mut ba, mut vd, mut bd) = (vec![], vec![], vec![], vec![]);
    let mut cache: BTreeMap<(u32, [u8; 8], u8), Option<i64>> = BTreeMap::new();
    let t = &leg.tfms;
    for i in 0..t.kind.len() {
        let kind = t.kind.get(i).copied().unwrap_or(0);
        let (scope, arr) = if kind == tfms_kind::ETA {
            ("F3-arrival", true)
        } else {
            ("F3-departure", false)
        };
        let key: [u8; 12] = t
            .plan_id
            .get(i)
            .map(|p| pad(name(p).as_bytes()))
            .unwrap_or([0; 12]);
        let Some(&f) = plan.get(&key) else {
            exclude(ex, scope, "not matched to an SFDPS flight");
            continue;
        };
        let addr = assoc.addr.get(f as usize).copied().unwrap_or(NONE);
        if addr == NONE {
            exclude(ex, scope, "flight not associated with truth");
            continue;
        }
        let apt_raw = if arr { t.arr.get(i) } else { t.dep.get(i) };
        let apt: [u8; 8] = apt_raw.copied().unwrap_or([0; 8]);
        let Some(ap) = (env.airport)(name(&apt)) else {
            exclude(ex, scope, "airport not in the model");
            continue;
        };
        let truth_t = *cache.entry((addr, apt, u8::from(arr))).or_insert_with(|| {
            if arr {
                landing_time(truth, tix, addr, &ap, cfg)
            } else {
                takeoff_time(truth, tix, addr, &ap, cfg)
            }
        });
        let Some(truth_t) = truth_t else {
            exclude(
                ex,
                scope,
                if arr {
                    "no truth landing at the destination"
                } else {
                    "no truth takeoff at the origin"
                },
            );
            continue;
        };
        let issue = t.issue_ms.get(i).copied().unwrap_or(NO_TIME);
        if issue == NO_TIME {
            exclude(ex, scope, "no issue time");
            continue;
        }
        let Some(bin) = lookahead_bin(cfg, truth_t - issue) else {
            exclude(
                ex,
                scope,
                "issued after the event or more than 60 min before it",
            );
            continue;
        };
        let err = (t.time_ms.get(i).copied().unwrap_or(0) - truth_t) as f64 / 1000.0;
        if arr {
            va.push(err);
            ba.push(bin);
        } else {
            vd.push(err);
            bd.push(bin);
        }
    }
    let mut ra = bin_result(
        "F3-arrival",
        "TFMS arrival estimate error (estimate minus truth landing)",
        &va,
        &ba,
    );
    ra.notes.push(
        "truth ends at the last bracketed ADS-B report, which can be up to 10 s before touchdown"
            .into(),
    );
    let rd = bin_result(
        "F3-departure",
        "TFMS departure estimate error (estimate minus truth takeoff)",
        &vd,
        &bd,
    );
    (ra, rd)
}

/// A route as a polyline in degrees.
fn polyline(leg: &Legacy, route: usize, env: &Env<'_>) -> Vec<(f64, f64)> {
    let first = leg.routes.first.get(route).copied().unwrap_or(0) as usize;
    let len = leg.routes.len.get(route).copied().unwrap_or(0) as usize;
    let mut out = Vec::new();
    for k in first..first + len {
        let (lat, lon) = (
            leg.route_points.lat_e7.get(k).copied().unwrap_or(NO_COORD),
            leg.route_points.lon_e7.get(k).copied().unwrap_or(NO_COORD),
        );
        if lat != NO_COORD && lon != NO_COORD {
            out.push((f64::from(lat) * 1e-7, f64::from(lon) * 1e-7));
        } else if let Some(fix) = leg.route_points.fix.get(k) {
            if let Some(p) = (env.fix)(name(fix)) {
                out.push(p);
            }
        }
    }
    out
}

/// Distance in metres from a point to a polyline (the nearest segment; one point is a point).
pub fn dist_to_polyline(lat: f64, lon: f64, line: &[(f64, f64)]) -> f64 {
    let mut best = f64::MAX;
    if let [only] = line {
        return dist_m(lat, lon, only.0, only.1);
    }
    for w in line.windows(2) {
        let (Some(&(la, lo)), Some(&(lb, ob))) = (w.first(), w.get(1)) else {
            continue;
        };
        let (bx, by) = enu(la, lo, lb, ob);
        let (px, py) = enu(la, lo, lat, lon);
        let l2 = bx * bx + by * by;
        let u = if l2 > 0.0 {
            ((px * bx + py * by) / l2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let d = (px - u * bx).hypot(py - u * by);
        best = best.min(d);
    }
    best
}

/// R1: route conformance. Over the paired samples of each flight that has an expanded route (the
/// latest one issued at or before the sample), the truth distance from the route, metres, and the
/// share of flight time beyond the limit. Each flight with any time beyond it is listed.
pub fn r1_route_conformance(
    leg: &Legacy,
    p: &PairTable,
    truth: &Truth<'_>,
    env: &Env<'_>,
    cfg: &EventConfig,
    ex: &mut Exclusions,
) -> MetricResult {
    let mut routes_of: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for r in 0..leg.routes.flight.len() {
        if let Some(&f) = leg.routes.flight.get(r) {
            routes_of.entry(f).or_default().push(r as u32);
        }
    }
    for v in routes_of.values_mut() {
        v.sort_by_key(|&r| (leg.routes.ts_ms.get(r as usize).copied().unwrap_or(0), r));
    }
    let mut lines: BTreeMap<u32, Vec<(f64, f64)>> = BTreeMap::new();
    let mut dists = Vec::new();
    let (mut t_all, mut t_off) = (0i64, 0i64);
    let (mut flights, mut off_flights) = (0u64, 0u64);
    let mut max_share = 0.0f64;
    let mut cases: Vec<(i64, String)> = Vec::new();
    let n = p.t_ms.len();
    let mut i = 0usize;
    while i < n {
        let f = p.flight.get(i).copied().unwrap_or(0);
        let mut j = i;
        while j < n && p.flight.get(j).copied() == Some(f) {
            j += 1;
        }
        let Some(rs) = routes_of.get(&f) else {
            exclude(ex, "R1", "flight has no expanded route");
            i = j;
            continue;
        };
        let (mut f_all, mut f_off, mut f_max, mut first_off) = (0i64, 0i64, 0.0f64, None);
        let mut any = false;
        for k in i..j {
            let t = p.t_ms.get(k).copied().unwrap_or(0);
            let r = rs
                .iter()
                .rev()
                .find(|&&r| leg.routes.ts_ms.get(r as usize).is_some_and(|&ts| ts <= t));
            let Some(&r) = r else {
                exclude(ex, "R1", "no route issued yet at the sample");
                continue;
            };
            let line = lines
                .entry(r)
                .or_insert_with(|| polyline(leg, r as usize, env));
            if line.len() < 2 {
                exclude(ex, "R1", "route has fewer than two resolved points");
                continue;
            }
            let own = p.addr.get(k).copied().unwrap_or(0);
            let Ok(s) = (truth.query)(own, t) else {
                continue;
            };
            let d = dist_to_polyline(s.lat, s.lon, line);
            let dt = if k + 1 < j {
                (p.t_ms.get(k + 1).copied().unwrap_or(t) - t).clamp(0, cfg.route_max_dt_ms)
            } else {
                0
            };
            any = true;
            dists.push(d);
            f_all += dt;
            if d > cfg.route_limit_m {
                f_off += dt;
                f_max = f_max.max(d);
                first_off.get_or_insert(t);
            }
        }
        if any {
            flights += 1;
            t_all += f_all;
            t_off += f_off;
            if first_off.is_some() {
                off_flights += 1;
                let cs = leg.flights.callsign.get(f as usize).map_or("", |c| name(c));
                let share = if f_all > 0 {
                    f_off as f64 / f_all as f64
                } else {
                    0.0
                };
                max_share = max_share.max(share);
                cases.push((
                    -(f_max * 1000.0) as i64,
                    format!(
                        "flight {cs}: {:.1}% of time beyond limit, up to {:.2} NM, first at {} ms",
                        share * 100.0,
                        f_max / NM_M,
                        first_off.unwrap_or(0)
                    ),
                ));
            }
        }
        i = j;
    }
    let mut r = new_result(
        "R1",
        "route conformance (truth distance from ERAM expanded route)",
        "m",
    );
    r.rows = group_rows(&dists, &[]);
    r.scalars.push(("flights_evaluated".into(), flights as f64));
    r.scalars
        .push(("flights_with_time_beyond_limit".into(), off_flights as f64));
    r.scalars.push((
        "share_of_time_beyond_limit".into(),
        if t_all > 0 {
            t_off as f64 / t_all as f64
        } else {
            0.0
        },
    ));
    r.scalars.push(("max_flight_share".into(), max_share));
    cases.sort();
    r.scalars.push((
        "cases_not_listed".into(),
        cases.len().saturating_sub(MAX_CASES) as f64,
    ));
    r.cases = cases.into_iter().take(MAX_CASES).map(|c| c.1).collect();
    r.notes.push(format!(
        "limit {:.0} m; each sample stands for the time to the next, at most {} s",
        cfg.route_limit_m,
        cfg.route_max_dt_ms / 1000
    ));
    r
}

/// The truth crossing of the boundary from unit `from` to unit `to` near time `around`, Unix ms.
fn boundary_crossing(
    truth: &Truth<'_>,
    env: &Env<'_>,
    addr: u32,
    from: [u8; 8],
    to: [u8; 8],
    around: i64,
    cfg: &EventConfig,
) -> Option<i64> {
    let unit = |t: i64| -> Option<[u8; 8]> {
        let s = (truth.query)(addr, t).ok()?;
        (env.unit_at)(s.lat, s.lon, s.baro_alt_ft)
    };
    let step = cfg.handoff_step_ms.max(1);
    let mut t = around - cfg.handoff_window_ms;
    let mut prev: Option<(i64, [u8; 8])> = None;
    while t <= around + cfg.handoff_window_ms {
        if let Some(u) = unit(t) {
            if let Some((pt, pu)) = prev {
                if pu == from && u == to {
                    let (mut lo, mut hi) = (pt, t);
                    while hi - lo > 1000 {
                        let mid = lo + (hi - lo) / 2;
                        if unit(mid) == Some(from) {
                            lo = mid;
                        } else {
                            hi = mid;
                        }
                    }
                    return Some(hi);
                }
            }
            prev = Some((t, u));
        }
        t += step;
    }
    None
}

/// H1: handoff initiation and acceptance time against the truth crossing of the boundary between
/// the transferring and receiving unit, seconds, signed (event minus crossing).
pub fn h1_handoffs(
    leg: &Legacy,
    assoc: &Assoc,
    truth: &Truth<'_>,
    env: &Env<'_>,
    cfg: &EventConfig,
    ex: &mut Exclusions,
) -> MetricResult {
    let (mut vals, mut kinds) = (vec![], vec![]);
    let h = &leg.handoffs;
    for i in 0..h.flight.len() {
        let ev = h.event.get(i).copied().unwrap_or(0);
        let code = match ev {
            hev::INITIATION => 0u8,
            hev::ACCEPTANCE => 1u8,
            _ => continue,
        };
        let f = h.flight.get(i).copied().unwrap_or(0);
        let addr = assoc.addr.get(f as usize).copied().unwrap_or(NONE);
        if addr == NONE {
            exclude(ex, "H1", "flight not associated with truth");
            continue;
        }
        let (from, to) = (
            h.from.get(i).copied().unwrap_or([0; 8]),
            h.to.get(i).copied().unwrap_or([0; 8]),
        );
        if from.iter().all(|&b| b == 0) || to.iter().all(|&b| b == 0) {
            exclude(ex, "H1", "handoff lacks a transferring or receiving unit");
            continue;
        }
        let ts = h.ts_ms.get(i).copied().unwrap_or(NO_TIME);
        if ts == NO_TIME {
            exclude(ex, "H1", "no message time");
            continue;
        }
        match boundary_crossing(truth, env, addr, from, to, ts, cfg) {
            Some(c) => {
                vals.push((ts - c) as f64 / 1000.0);
                kinds.push(code);
            }
            None => exclude(ex, "H1", "no boundary crossing in truth near the event"),
        }
    }
    let mut r = new_result(
        "H1",
        "handoff time error (event minus truth boundary crossing)",
        "s",
    );
    r.rows = group_rows(&vals, &[("event", &kinds, &["initiation", "acceptance"])]);
    r.notes.push(
        "units are compared exactly as ERAM names them (unit and sector); the boundary function is supplied by the airspace model".into(),
    );
    r
}
