//! ADS-B reports with per-aircraft persistent errors, generated from an exact truth. Core (pure).
//!
//! The truth is taken as the true trajectories (queried with `oa_truth::truth_at`, whole
//! milliseconds). One report is generated for every report of the base table, at the same time and
//! under the same address, with every field other than position, velocity, altitude and vertical
//! rate copied from the base row. Two errors, each persistent for the aircraft that has it:
//!
//! * **Position bias.** A chosen fraction of the aircraft get a constant horizontal offset, in a
//!   random direction fixed per aircraft. The offset of a report is `bias_mult` times that
//!   report's NACp 95 % bound ([`oa_truth::nacp_epu_m`]), or `bias_fallback_bound_m` when the
//!   row has no usable NACp.
//! * **Latency.** A chosen fraction of the aircraft report the position, velocity, altitude and
//!   vertical rate of time `t - L` stamped at `t`, `L` constant per aircraft and uniform in
//!   `[lag_min_s, lag_max_s]` (rounded to whole milliseconds).
//!
//! Everything else is exact: a row of an aircraft with neither error is the truth at its time.
//! A row the truth cannot answer (outside its legs, too uncertain, no nearby report) is left out
//! and counted. The fractions are honoured exactly: with `N` distinct addresses, round(`f` * `N`)
//! of them carry the error, chosen by a seeded shuffle; the two errors are chosen independently
//! and may share an aircraft. Every number is a parameter; there are no defaults. The output
//! depends only on the arguments.

use oa_adsb::{reports_len, reports_push, reports_row, Reports, NONE_I32};
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
    if base.time_ns.len() != n || base.lat_e7.len() != n || base.lon_e7.len() != n {
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
        let t_ms = r.time_ns.div_euclid(1_000_000) - lag_ms.unwrap_or(0);
        let TruthQuery::Ok(s) = truth_at(truth, r.icao, t_ms) else {
            out.dropped_rows += 1;
            continue;
        };
        let (mut lat, mut lon) = (s.lat_deg, s.lon_deg);
        if biased {
            let epu = nacp_epu_m(r.nac_p);
            let bound = if epu.is_finite() {
                epu
            } else {
                p.bias_fallback_bound_m
            };
            let bearing =
                Rng::from_stream(p.seed, STREAM_DIRECTION, u64::from(r.icao)).unit() * 360.0;
            (lat, lon) = direct_deg(lat, lon, bearing, p.bias_mult * bound)
                .ok_or(SimError::BadSpoof("bias position out of range"))?;
        }
        r.lat_e7 = e7(lat, 900_000_000)?;
        r.lon_e7 = e7(lon, 1_800_000_000)?;
        let kt = s.vel_east_mps.hypot(s.vel_north_mps) * 3600.0 / 1852.0;
        r.gs_dkt = (kt * 10.0).round().clamp(0.0, f64::from(u16::MAX)) as u16;
        let trk = s
            .vel_east_mps
            .atan2(s.vel_north_mps)
            .to_degrees()
            .rem_euclid(360.0);
        r.track_ddeg = ((trk * 10.0).round() as u16) % 3600;
        if let Some(a) = s.baro_alt_ft {
            r.baro_alt_ft = whole(a, r.baro_alt_ft);
        }
        if let Some(v) = s.vert_rate_fpm {
            r.vrate_fpm = whole(v, r.vrate_fpm);
        }
        reports_push(&mut out.reports, &r);
        out.label
            .push(if biased { ERR_BIAS } else { 0 } | if lag_ms.is_some() { ERR_LAG } else { 0 });
    }
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
