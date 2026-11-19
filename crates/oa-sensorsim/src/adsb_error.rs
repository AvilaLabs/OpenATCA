//! ADS-B reports with per-aircraft persistent errors, generated from an exact truth. Core (pure).
//!
//! The truth is taken as the true trajectories (queried with `oa_truth::truth_at`, whole
//! milliseconds). One report is generated for every report of the base table, under the same
//! address and at the report's time rounded to the nearest millisecond (the generated row's
//! time is exactly that millisecond), with every field other than position, velocity, altitude
//! and vertical rate copied from the base row. Two errors, each persistent for the aircraft that
//! has it:
//!
//! * **Position bias.** A chosen fraction of the aircraft get a constant horizontal offset, in a
//!   random direction fixed per aircraft. The magnitude is `bias_mult` times the aircraft's one
//!   bound: the median of the aircraft's usable NACp 95 % bounds ([`oa_truth::nacp_epu_m`])
//!   over its rows (the upper middle for an even count), or `bias_fallback_bound_m` when no row
//!   has a usable NACp.
//! * **Latency.** A chosen fraction of the aircraft report the position, velocity, barometric
//!   altitude and vertical rate of time `t - L` stamped at `t`, `L` constant per aircraft and
//!   uniform in `[lag_min_s, lag_max_s]` (rounded to whole milliseconds). A lagged row's
//!   altitude or vertical rate the truth does not have is blanked, never left stale, and its
//!   geometric altitude (which the truth does not carry) is blanked.
//!
//! A lagged row whose time `t - L` the truth cannot answer (before the start of its leg) is
//! taken from the truth at `t` stepped back by `L` along its velocity, and counted as
//! extrapolated (by label and by reason); its barometric altitude and vertical rate, which the
//! truth at `t` does not give for `t - L`, are blanked. It is left out only when the truth cannot
//! answer at `t` either or its velocity is not finite.
//!
//! Optional per-report noise: each row gets an independent Gaussian offset (seeded per address
//! and time) of standard deviation `hypot(noise_sigma_m, noise_bound_frac * bound)` per axis,
//! where `bound` is the row's own NACp bound (the fallback where it has none). Both zero is no
//! noise.
//!
//! Everything else is exact: a row of an aircraft with neither error and no noise is the truth
//! at its time. A row the truth cannot answer (outside its legs, too uncertain, no nearby
//! report) is left out and counted, in total and by the label its aircraft would have carried. The fractions are honoured exactly: with `N` distinct addresses, round(`f` * `N`)
//! of them carry the error, chosen by a seeded shuffle; the two errors are chosen independently
//! and may share an aircraft. Every number is a parameter; there are no defaults. The output
//! depends only on the arguments.

use std::collections::BTreeMap;

use oa_adsb::{reports_len, reports_push, reports_row, Reports, NONE_I32};
use oa_math::{atan2, clamp_f64, hypot, rem_euclid};
use oa_truth::{nacp_epu_m, truth_at, Truth, TruthQuery};

use crate::error::SimError;
use crate::geom::direct_deg;
use crate::prng::Rng;

/// Label bit: the row carries the aircraft's position bias.
pub const ERR_BIAS: u8 = 1;
/// Label bit: the row carries the aircraft's latency.
pub const ERR_LAG: u8 = 2;

/// The largest latency accepted, seconds.
pub const MAX_LAG_S: f64 = 3600.0;

const STREAM_BIAS_SET: u64 = 1;
const STREAM_LAG_SET: u64 = 2;
const STREAM_DIRECTION: u64 = 3;
const STREAM_LAG: u64 = 4;
const STREAM_NOISE: u64 = 5;

/// What to apply. All fields explicit. Plain data (A1); A7.2: cold struct, size guard waived.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ErrorParams {
    pub seed: u64,
    /// Fraction of the aircraft given a position bias, 0 to 1.
    pub bias_fraction: f64,
    /// Bias magnitude as a multiple of the row's NACp 95 % bound, not negative.
    pub bias_mult: f64,
    /// The bound used where a row has no usable NACp, metres, not negative.
    pub bias_fallback_bound_m: f64,
    /// Fraction of the aircraft given a latency, 0 to 1.
    pub lag_fraction: f64,
    /// Latency range, seconds: `0 <= lag_min_s <= lag_max_s <= MAX_LAG_S`.
    pub lag_min_s: f64,
    pub lag_max_s: f64,
    /// Fixed part of the per-axis noise standard deviation, metres, not negative.
    pub noise_sigma_m: f64,
    /// Part of it as a multiple of the row's NACp bound, not negative.
    pub noise_bound_frac: f64,
}

/// The generated reports with their labels. Plain data (A1); A7.2: cold struct.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Errored {
    pub reports: Reports,
    /// `ERR_*` bits per row; 0 for a row with no error.
    pub label: Vec<u8>,
    /// Rows of the base table the truth could not answer, left out.
    pub dropped_rows: u64,
    /// Addresses given a bias, ascending.
    pub bias_addrs: Vec<u32>,
    /// Addresses given a latency, ascending, with the latency in whole milliseconds.
    pub lag_addrs: Vec<(u32, i64)>,
    /// Base rows by the label their aircraft carries (index = label bits 0 to 3), and of those
    /// the rows left out.
    pub rows_by_label: [u64; 4],
    pub dropped_by_label: [u64; 4],
    /// Lagged rows taken by stepping the truth at `t` back along its velocity, in total, by the
    /// label of their aircraft and by why the truth at `t - L` was refused (outside its legs, no
    /// nearby report, any other reason).
    pub extrapolated_rows: u64,
    pub extrapolated_by_label: [u64; 4],
    pub extrapolated_by_reason: [u64; 3],
}

fn unit_fraction(f: f64) -> bool {
    f.is_finite() && (0.0..=1.0).contains(&f)
}

/// Checks the parameters and returns the latency range in whole milliseconds.
fn check(p: &ErrorParams) -> Result<(i64, i64), SimError> {
    if !unit_fraction(p.bias_fraction) || !unit_fraction(p.lag_fraction) {
        return Err(SimError::BadSpoof("error fraction"));
    }
    let ok = |v: f64| v.is_finite() && v >= 0.0;
    if !ok(p.bias_mult) || !ok(p.bias_fallback_bound_m) {
        return Err(SimError::BadSpoof("bias multiple or fallback bound"));
    }
    if !ok(p.noise_sigma_m) || !ok(p.noise_bound_frac) {
        return Err(SimError::BadSpoof("noise sigma or bound fraction"));
    }
    if !(ok(p.lag_min_s) && ok(p.lag_max_s) && p.lag_min_s <= p.lag_max_s)
        || p.lag_max_s > MAX_LAG_S
    {
        return Err(SimError::BadSpoof("latency range"));
    }
    Ok((
        (p.lag_min_s * 1000.0).round() as i64,
        (p.lag_max_s * 1000.0).round() as i64,
    ))
}

/// `round(fraction * n)` aircraft of `addrs` (ascending), picked by a seeded shuffle, ascending.
fn pick(addrs: &[u32], fraction: f64, seed: u64, stream: u64) -> Vec<u32> {
    let k = ((fraction * addrs.len() as f64).round() as usize).min(addrs.len());
    let mut v = addrs.to_vec();
    let mut rng = Rng::from_stream(seed, stream, 0);
    for i in (1..v.len()).rev() {
        let j = ((rng.unit() * (i + 1) as f64) as usize).min(i);
        v.swap(i, j);
    }
    v.truncate(k);
    v.sort_unstable();
    v
}

/// Generates the erroneous reports of `base` from `truth` (see the module documentation).
/// Refuses a fraction outside 0 to 1, a negative or non-finite multiple, bound or latency, an
/// inverted latency range, a latency above [`MAX_LAG_S`] and base columns of different lengths.
pub fn adsb_error(truth: &Truth, base: &Reports, p: &ErrorParams) -> Result<Errored, SimError> {
    let (lag_lo_ms, lag_hi_ms) = check(p)?;
    let n = reports_len(base);
    if base.time_ns.len() != n
        || base.lat_e7.len() != n
        || base.lon_e7.len() != n
        || base.nac_p.len() != n
    {
        return Err(SimError::ColumnLength);
    }
    let mut addrs: Vec<u32> = base.icao.clone();
    addrs.sort_unstable();
    addrs.dedup();
    let bias_addrs = pick(&addrs, p.bias_fraction, p.seed, STREAM_BIAS_SET);
    let lag_set = pick(&addrs, p.lag_fraction, p.seed, STREAM_LAG_SET);
    let lag_addrs: Vec<(u32, i64)> = lag_set
        .iter()
        .map(|&a| {
            let u = Rng::from_stream(p.seed, STREAM_LAG, u64::from(a)).unit();
            let ms = lag_lo_ms as f64 + u * (lag_hi_ms - lag_lo_ms) as f64;
            (a, (ms.round() as i64).clamp(lag_lo_ms, lag_hi_ms))
        })
        .collect();
    // One bound per aircraft: the median of its usable NACp bounds. The bound falls as the NACp
    // rises, so the upper-middle bound is the NACp at index `len - 1 - len / 2` ascending.
    let mut nacps: BTreeMap<u32, Vec<u8>> = BTreeMap::new();
    for (&a, &q) in base.icao.iter().zip(base.nac_p.iter()) {
        if nacp_epu_m(q).is_finite() {
            nacps.entry(a).or_default().push(q);
        }
    }
    let bounds: BTreeMap<u32, f64> = nacps
        .into_iter()
        .filter_map(|(a, mut v)| {
            v.sort_unstable();
            let q = *v.get(v.len() - 1 - v.len() / 2)?;
            Some((a, nacp_epu_m(q)))
        })
        .collect();
    let mut out = Errored {
        bias_addrs,
        lag_addrs,
        ..Errored::default()
    };
    for i in 0..n {
        let Some(mut r) = reports_row(base, i) else {
            return Err(SimError::ColumnLength);
        };
        let lag_ms = out
            .lag_addrs
            .binary_search_by_key(&r.icao, |&(a, _)| a)
            .ok()
            .and_then(|k| out.lag_addrs.get(k))
            .map(|&(_, ms)| ms);
        let biased = out.bias_addrs.binary_search(&r.icao).is_ok();
        let lab =
            (if biased { ERR_BIAS } else { 0 }) | (if lag_ms.is_some() { ERR_LAG } else { 0 });
        let kind = usize::from(lab);
        let at_ms = r.time_ns.saturating_add(500_000).div_euclid(1_000_000);
        if let Some(c) = out.rows_by_label.get_mut(kind) {
            *c += 1;
        }
        let lag = lag_ms.unwrap_or(0);
        let mut back_m = 0.0;
        let mut extrapolated = false;
        let s = match truth_at(truth, r.icao, at_ms - lag) {
            TruthQuery::Ok(s) => s,
            refused => match truth_at(truth, r.icao, at_ms) {
                TruthQuery::Ok(s)
                    if lag > 0 && s.vel_east_mps.is_finite() && s.vel_north_mps.is_finite() =>
                {
                    // Before the leg began: the position it would have had, one step back.
                    back_m = hypot(s.vel_east_mps, s.vel_north_mps) * lag as f64 * 1.0e-3;
                    extrapolated = true;
                    out.extrapolated_rows += 1;
                    if let Some(c) = out.extrapolated_by_label.get_mut(kind) {
                        *c += 1;
                    }
                    let why = match refused {
                        TruthQuery::OutsideLegs => 0,
                        TruthQuery::NoNearbyReport => 1,
                        _ => 2,
                    };
                    if let Some(c) = out.extrapolated_by_reason.get_mut(why) {
                        *c += 1;
                    }
                    s
                }
                _ => {
                    if let Some(c) = out.dropped_by_label.get_mut(kind) {
                        *c += 1;
                    }
                    continue;
                }
            },
        };
        r.time_ns = at_ms.saturating_mul(1_000_000);
        let (mut lat, mut lon) = (s.lat_deg, s.lon_deg);
        if back_m > 0.0 {
            let heading = atan2(s.vel_east_mps, s.vel_north_mps).to_degrees() + 180.0;
            (lat, lon) = direct_deg(lat, lon, heading, back_m)
                .ok_or(SimError::BadSpoof("extrapolated position out of range"))?;
        }
        if biased {
            let bound = bounds
                .get(&r.icao)
                .copied()
                .unwrap_or(p.bias_fallback_bound_m);
            let bearing =
                Rng::from_stream(p.seed, STREAM_DIRECTION, u64::from(r.icao)).unit() * 360.0;
            (lat, lon) = direct_deg(lat, lon, bearing, p.bias_mult * bound)
                .ok_or(SimError::BadSpoof("bias position out of range"))?;
        }
        if p.noise_sigma_m > 0.0 || p.noise_bound_frac > 0.0 {
            let epu = nacp_epu_m(r.nac_p);
            let bound = if epu.is_finite() {
                epu
            } else {
                p.bias_fallback_bound_m
            };
            let sigma = hypot(p.noise_sigma_m, p.noise_bound_frac * bound);
            let key = (u64::from(r.icao) << 40) ^ (at_ms as u64);
            let mut rng = Rng::from_stream(p.seed, STREAM_NOISE, key);
            let (e, nn) = (rng.normal() * sigma, rng.normal() * sigma);
            (lat, lon) = direct_deg(
                lat,
                lon,
                rem_euclid(atan2(e, nn).to_degrees(), 360.0),
                hypot(e, nn),
            )
            .ok_or(SimError::BadSpoof("noise position out of range"))?;
        }
        r.lat_e7 = e7(lat, 900_000_000)?;
        r.lon_e7 = e7(lon, 1_800_000_000)?;
        let kt = hypot(s.vel_east_mps, s.vel_north_mps) * 3600.0 / 1852.0;
        r.gs_dkt = clamp_f64((kt * 10.0).round(), 0.0, f64::from(u16::MAX)) as u16;
        let trk = rem_euclid(atan2(s.vel_east_mps, s.vel_north_mps).to_degrees(), 360.0);
        r.track_ddeg = ((trk * 10.0).round() as u16) % 3600;
        let blank = lag_ms.is_some();
        r.baro_alt_ft = match s.baro_alt_ft {
            _ if extrapolated => NONE_I32,
            Some(a) => whole(a, r.baro_alt_ft),
            None if blank => NONE_I32,
            None => r.baro_alt_ft,
        };
        r.vrate_fpm = match s.vert_rate_fpm {
            _ if extrapolated => NONE_I32,
            Some(v) => whole(v, r.vrate_fpm),
            None if blank => NONE_I32,
            None => r.vrate_fpm,
        };
        if blank {
            r.geom_alt_ft = NONE_I32;
        }
        reports_push(&mut out.reports, &r);
        out.label.push(lab);
    }
    out.dropped_rows = out.dropped_by_label.iter().sum();
    Ok(out)
}

fn e7(deg: f64, limit_e7: i32) -> Result<i32, SimError> {
    let v = (deg * 1.0e7).round();
    if v.is_finite() && v.abs() <= f64::from(limit_e7) {
        Ok(v as i32)
    } else {
        Err(SimError::BadSpoof("position out of range"))
    }
}

/// `v` rounded to a whole number, or `keep` when that is not finite or not an `i32` (and never
/// the absent marker).
fn whole(v: f64, keep: i32) -> i32 {
    let r = v.round();
    if r.is_finite() && r >= f64::from(i32::MIN) && r <= f64::from(i32::MAX) && r as i32 != NONE_I32
    {
        r as i32
    } else {
        keep
    }
}
