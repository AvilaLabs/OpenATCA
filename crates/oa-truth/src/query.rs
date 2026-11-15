//! Queries on a built [`Truth`], its accessors and its digest. Core (pure).

use oa_adsb::NONE_I32;
use oa_sha256::Sha256;

use crate::frame::{frame_new, frame_velocity, unproject};
use crate::smooth::{interp, sigma95_h_m, LegView};
use crate::{LegIx, LegRange, Truth, TruthQuery, TruthSample, TRUTH_HASH_TAG};

const NS_PER_MS: i64 = 1_000_000;

/// The aircraft addresses of the truth, ascending.
pub fn aircraft_addrs(truth: &Truth) -> &[u32] {
    &truth.ac_addr
}

/// Number of legs in the truth.
pub fn leg_count(truth: &Truth) -> usize {
    truth.leg_addr.len()
}

/// The legs of `addr`: consecutive, in time order; `count` is 0 if the aircraft is unknown.
pub fn legs_of(truth: &Truth, addr: u32) -> LegRange {
    let none = LegRange {
        addr,
        first: LegIx(0),
        count: 0,
    };
    let Ok(a) = truth.ac_addr.binary_search(&addr) else {
        return none;
    };
    match (truth.ac_first_leg.get(a), truth.ac_first_leg.get(a + 1)) {
        (Some(&lo), Some(&hi)) => LegRange {
            addr,
            first: LegIx(lo),
            count: hi.saturating_sub(lo),
        },
        _ => none,
    }
}

/// The point range of a leg.
fn leg_points(truth: &Truth, leg: u32) -> Option<(usize, usize)> {
    let l = leg as usize;
    let lo = *truth.leg_first_pt.get(l)? as usize;
    let hi = *truth.leg_first_pt.get(l + 1)? as usize;
    (lo < hi).then_some((lo, hi))
}

/// Address word of a leg.
pub fn leg_addr(truth: &Truth, leg: LegIx) -> Option<u32> {
    truth.leg_addr.get(leg.0 as usize).copied()
}

/// Time of a leg's first report, Unix ns.
pub fn leg_start_ns(truth: &Truth, leg: LegIx) -> Option<i64> {
    let (lo, _) = leg_points(truth, leg.0)?;
    truth.pt_t_ns.get(lo).copied()
}

/// Time of a leg's last report, Unix ns.
pub fn leg_end_ns(truth: &Truth, leg: LegIx) -> Option<i64> {
    let (_, hi) = leg_points(truth, leg.0)?;
    truth.pt_t_ns.get(hi.checked_sub(1)?).copied()
}

/// First whole millisecond inside the leg (the start rounded up).
pub fn leg_start_ms(truth: &Truth, leg: LegIx) -> Option<i64> {
    let t = leg_start_ns(truth, leg)?;
    Some(t.div_euclid(NS_PER_MS) + i64::from(t.rem_euclid(NS_PER_MS) != 0))
}

/// Last whole millisecond inside the leg (the end rounded down).
pub fn leg_end_ms(truth: &Truth, leg: LegIx) -> Option<i64> {
    Some(leg_end_ns(truth, leg)?.div_euclid(NS_PER_MS))
}

/// Number of reports in a leg.
pub fn leg_reports(truth: &Truth, leg: LegIx) -> Option<u32> {
    let (lo, hi) = leg_points(truth, leg.0)?;
    u32::try_from(hi - lo).ok()
}

fn view_of(truth: &Truth, lo: usize, hi: usize) -> Option<LegView<'_>> {
    Some(LegView {
        t_ns: truth.pt_t_ns.get(lo..hi)?,
        alt_ft: truth.pt_alt_ft.get(lo..hi)?,
        vrate_fpm: truth.pt_vrate_fpm.get(lo..hi)?,
        xf: truth.pt_xf.get(lo..hi)?,
        pf: truth.pt_pf.get(lo..hi)?,
        xs: truth.pt_xs.get(lo..hi)?,
        ps: truth.pt_ps.get(lo..hi)?,
    })
}

fn lerp(a0: f64, a1: f64, t0: i64, t1: i64, t: i64) -> f64 {
    if t1 == t0 {
        return a0;
    }
    a0 + (a1 - a0) * ((t - t0) as f64 / (t1 - t0) as f64)
}

/// Truth for `addr` at `t_ms` (Unix milliseconds): position and velocity from the smoother,
/// altitude interpolated, only where real reports lie within the bracket on both sides.
/// See [`TruthQuery`] for the refusals. No extrapolation, ever.
pub fn truth_at(truth: &Truth, addr: u32, t_ms: i64) -> TruthQuery {
    let range = legs_of(truth, addr);
    if range.count == 0 {
        return TruthQuery::UnknownAircraft;
    }
    let Some(t) = t_ms.checked_mul(NS_PER_MS) else {
        return TruthQuery::OutsideLegs;
    };
    // The leg with the latest start at or before t.
    let (lo_leg, hi_leg) = (range.first.0, range.first.0 + range.count);
    let (mut a, mut b) = (lo_leg, hi_leg);
    while a < b {
        let m = a + (b - a) / 2;
        match leg_start_ns(truth, LegIx(m)) {
            Some(s) if s <= t => a = m + 1,
            _ => b = m,
        }
    }
    let Some(leg) = a.checked_sub(1).filter(|&l| l >= lo_leg) else {
        return TruthQuery::OutsideLegs;
    };
    let Some((lo, hi)) = leg_points(truth, leg) else {
        return TruthQuery::OutsideLegs;
    };
    let Some(v) = view_of(truth, lo, hi) else {
        return TruthQuery::OutsideLegs;
    };
    let Some(&end) = v.t_ns.last() else {
        return TruthQuery::OutsideLegs;
    };
    if t > end {
        return TruthQuery::OutsideLegs;
    }
    // k: last report at or before t. Right: the report at t itself, or the next one.
    let k = v.t_ns.partition_point(|&x| x <= t).saturating_sub(1);
    let (Some(&tl), Some(&tk1)) = (v.t_ns.get(k), v.t_ns.get(k + 1).or(v.t_ns.get(k))) else {
        return TruthQuery::OutsideLegs;
    };
    let (k_r, tr) = if tl == t { (k, tl) } else { (k + 1, tk1) };
    let bracket = truth.cfg.bracket_ms * NS_PER_MS;
    if t - tl > bracket || tr - t > bracket {
        return TruthQuery::NoNearbyReport;
    }
    let Some((x, p)) = interp(&v, k, t, truth.cfg.accel_q) else {
        return TruthQuery::OutsideLegs;
    };
    // A non-finite state or covariance is unusable, never precise.
    let Some(sigma95) = sigma95_h_m(p[0]).filter(|_| x.iter().all(|v| v.is_finite())) else {
        return TruthQuery::TooUncertain;
    };
    if sigma95 > truth.cfg.max_sigma95_h_m {
        return TruthQuery::TooUncertain;
    }
    let (Some(&lat0), Some(&lon0)) = (
        truth.leg_lat0.get(leg as usize),
        truth.leg_lon0.get(leg as usize),
    ) else {
        return TruthQuery::OutsideLegs;
    };
    let frame = frame_new(lat0, lon0);
    let [e, ve, _, n, vn, _] = x;
    let (lat, lon) = unproject(&frame, e, n);
    let (vel_e, vel_n) = frame_velocity(&frame, e, n, ve, vn);
    // Altitude and vertical rate from the bracketing reports.
    let (a_l, a_r) = (v.alt_ft.get(k).copied(), v.alt_ft.get(k_r).copied());
    let baro_alt_ft = match (a_l, a_r) {
        (Some(a0), Some(a1)) if a0 != NONE_I32 && a1 != NONE_I32 => {
            Some(lerp(f64::from(a0), f64::from(a1), tl, tr, t))
        }
        _ => None,
    };
    let (r_l, r_r) = (v.vrate_fpm.get(k).copied(), v.vrate_fpm.get(k_r).copied());
    let vert_rate_fpm = match (r_l, r_r) {
        (Some(r0), Some(r1)) if r0 != NONE_I32 && r1 != NONE_I32 => {
            Some(lerp(f64::from(r0), f64::from(r1), tl, tr, t))
        }
        _ => match (a_l, a_r) {
            (Some(a0), Some(a1))
                if a0 != NONE_I32 && a1 != NONE_I32 && tr - tl >= 2_000_000_000 =>
            {
                Some((f64::from(a1) - f64::from(a0)) / ((tr - tl) as f64 / 1.0e9) * 60.0)
            }
            _ => None,
        },
    };
    TruthQuery::Ok(TruthSample {
        lat_deg: lat,
        lon_deg: lon,
        baro_alt_ft,
        vel_east_mps: vel_e,
        vel_north_mps: vel_n,
        vert_rate_fpm,
        sigma95_h_m: sigma95,
        leg: LegIx(leg),
    })
}

fn put(h: &mut Sha256, b: &[u8]) {
    // The only failure is a message over 2^61 bytes.
    let _ = h.update(b);
}

/// Writes the truth's canonical serialisation to the hasher: the tag, the settings, the counts,
/// then every column, integers little-endian and floats as their IEEE bits. Equal digests mean
/// the models are bit-identical.
pub fn truth_hash_into(h: &mut Sha256, t: &Truth) {
    put(h, TRUTH_HASH_TAG);
    let c = &t.cfg;
    for v in [c.min_nacp, c.min_nic, c.required_sil, c.min_version] {
        put(h, &[v]);
    }
    for v in [c.quality_carry_ms, c.leg_gap_ms, c.bracket_ms] {
        put(h, &v.to_le_bytes());
    }
    for v in [
        c.max_sigma95_h_m,
        c.max_jump_kt,
        c.jump_slack_m,
        c.accel_q,
        c.sigma_scale,
    ] {
        put(h, &v.to_bits().to_le_bytes());
    }
    for v in [c.jump_resync, c.min_leg_points, c.loo_stride] {
        put(h, &v.to_le_bytes());
    }
    put(h, &[u8::from(c.loo)]);
    for n in [t.ac_addr.len(), t.leg_addr.len(), t.pt_t_ns.len()] {
        put(h, &(n as u64).to_le_bytes());
    }
    for v in t
        .ac_addr
        .iter()
        .chain(&t.ac_first_leg)
        .chain(&t.leg_addr)
        .chain(&t.leg_first_pt)
    {
        put(h, &v.to_le_bytes());
    }
    for v in t.leg_lat0.iter().chain(&t.leg_lon0) {
        put(h, &v.to_bits().to_le_bytes());
    }
    for v in &t.pt_t_ns {
        put(h, &v.to_le_bytes());
    }
    for v in t.pt_alt_ft.iter().chain(&t.pt_vrate_fpm) {
        put(h, &v.to_le_bytes());
    }
    for col in [&t.pt_xf, &t.pt_pf, &t.pt_xs, &t.pt_ps] {
        for row in col {
            for v in row {
                put(h, &v.to_bits().to_le_bytes());
            }
        }
    }
}
