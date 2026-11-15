//! The radar simulation. Core (pure): a function of the truth, the site and the seed.
//!
//! For every truth leg and every antenna revolution the beam crosses the aircraft once. The
//! crossing time is solved so that the beam azimuth equals the aircraft's azimuth at that very
//! time, with the aircraft moving during the scan: with `t_k` the start of the revolution and
//! `P` its period, the beam is at `az0 + 360 (t - t_k) / P`, and the fixed-point iteration
//! `t <- t_k + P * ((az(t) - az0) mod 360) / 360` converges because the aircraft's angular rate
//! is far below the beam's (it is stopped after [`MAX_SOLVE_ITER`] steps or when the time moves
//! by under [`SOLVE_TOL_NS`], and a crossing whose azimuth residual is still over
//! [`SOLVE_TOL_DEG`] is dropped and counted, which only happens within metres of the antenna).
//! The antenna turns clockwise.
//!
//! Truth is queried with `oa_truth::truth_at`, which answers in whole milliseconds and refuses
//! to extrapolate; the sub-millisecond remainder is applied with the truth velocity. A plot is
//! therefore only produced where truth exists at the crossing time. Pressure altitude is taken
//! as height above the ellipsoid, and an aircraft with no altitude (on the ground) is placed at
//! zero height for geometry.
//!
//! Randomness: one stream per (aircraft, revolution) from [`crate::prng::Rng::from_stream`], so
//! an aircraft's plots do not depend on which other aircraft are in the truth. Draw order per
//! crossing is fixed: detection, Mode A presence, altitude presence, range noise, azimuth noise,
//! false-plot chance (then azimuth, range), split chance (then two signs).

use oa_adsb::NON_ICAO;
use oa_asterix::units::CAT048_RHO_NM;
use oa_truth::{
    aircraft_addrs, leg_end_ns, leg_start_ns, legs_of, truth_at, LegIx, Truth, TruthQuery,
};

use crate::error::SimError;
use crate::geom::{
    az_diff, central_angle, frame_at, line_of_sight, polar_of, wrap360, Frame, FT_M, NM_M,
};
use crate::plots::{
    plots_len, plots_push, Plot, Plots, KIND_FALSE, KIND_SPLIT, KIND_TRUE, MAX_PLOTS, NONE_ADDR,
    NONE_ALT, NONE_CODE,
};
use crate::prng::Rng;
use crate::profile::RadarProfile;

/// Limit: iterations of the crossing-time solve.
pub const MAX_SOLVE_ITER: u32 = 12;
/// The crossing time has converged when it moves by no more than this, nanoseconds.
pub const SOLVE_TOL_NS: i64 = 1_000;
/// A solved crossing is kept when the beam and target azimuths agree to this, degrees.
pub const SOLVE_TOL_DEG: f64 = 1.0e-4;
/// Limit: maximum range an ASTERIX CAT048 I048/040 can carry, metres (65 535 counts of 1/256 NM).
pub const MAX_CAT048_RANGE_M: f64 = 65_535.0 * CAT048_RHO_NM * NM_M;

/// A radar site and its run settings. Plain data (A1); A7.2: cold struct, size guard waived.
///
/// There is deliberately no `Default`: the two biases, the split geometry and the earth-radius
/// factor have no sourced value (docs/BENCHMARKS.md 3.1, "Gaps"), so every run states them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RadarSite {
    pub lat_deg: f64,
    pub lon_deg: f64,
    /// Antenna height above the ellipsoid, metres.
    pub height_m: f64,
    /// Rotation period, seconds (see `SCAN_TERMINAL_S`, `SCAN_EN_ROUTE_S`); converted once to
    /// whole nanoseconds.
    pub rotation_period_s: f64,
    /// Beam azimuth at `epoch_ns`, degrees clockwise from north.
    pub initial_az_deg: f64,
    /// The instant (Unix ns) at which the beam is at `initial_az_deg`; scan 0 starts here.
    pub epoch_ns: i64,
    /// Maximum slant range of coverage, metres.
    pub coverage_range_m: f64,
    /// Effective earth radius factor of the line-of-sight test (assumption; the standard value
    /// is 4/3, `EARTH_K_STANDARD`).
    pub earth_k: f64,
    pub profile: RadarProfile,
    /// Range bias, metres, added to every measured range. No default.
    pub bias_range_m: f64,
    /// Azimuth bias, degrees, added to every measured azimuth. No default.
    pub bias_az_deg: f64,
    /// Split plots: range offset of the second plot, metres (parameter, no sourced value).
    pub split_dr_m: f64,
    /// Split plots: azimuth offset of the second plot, degrees (parameter, no sourced value).
    pub split_daz_deg: f64,
    pub sac: u8,
    pub sic: u8,
}

/// What a run did. Plain record (A1); A7.2: cold struct.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SimStats {
    pub aircraft: u64,
    pub legs: u64,
    /// Revolution-aircraft pairs examined (a leg overlapping a revolution).
    pub crossings: u64,
    /// No truth at the crossing time (leg gap, bracket or uncertainty refusal, or past the leg).
    pub no_truth: u64,
    /// The crossing-time solve did not converge to [`SOLVE_TOL_DEG`].
    pub unsolved: u64,
    /// Outside the coverage range.
    pub out_of_range: u64,
    /// Inside the range but below the horizon.
    pub below_horizon: u64,
    /// In coverage (range and line of sight).
    pub in_coverage: u64,
    /// In coverage and not detected.
    pub missed: u64,
    /// True plots produced.
    pub detected: u64,
    pub false_plots: u64,
    pub split_plots: u64,
    /// Detections dropped because the measured range or azimuth was not finite.
    pub non_finite: u64,
}

fn check_site(s: &RadarSite) -> Result<(), SimError> {
    let fin = |v: f64| v.is_finite();
    if !(fin(s.lat_deg) && (-90.0..=90.0).contains(&s.lat_deg)) {
        return Err(SimError::BadSite("latitude"));
    }
    if !(fin(s.lon_deg) && (-180.0..=180.0).contains(&s.lon_deg)) {
        return Err(SimError::BadSite("longitude"));
    }
    if !(fin(s.height_m) && (-500.0..=9000.0).contains(&s.height_m)) {
        return Err(SimError::BadSite("height"));
    }
    // At least 0.1 s, so the period is a whole number of nanoseconds over 1e8.
    if !(fin(s.rotation_period_s) && (0.1..=600.0).contains(&s.rotation_period_s)) {
        return Err(SimError::BadSite("rotation period"));
    }
    if !(fin(s.initial_az_deg) && (0.0..360.0).contains(&s.initial_az_deg)) {
        return Err(SimError::BadSite("initial azimuth"));
    }
    if !(fin(s.coverage_range_m)
        && s.coverage_range_m > 0.0
        && s.coverage_range_m <= MAX_CAT048_RANGE_M)
    {
        return Err(SimError::BadSite("coverage range"));
    }
    if !(fin(s.earth_k) && (0.5..=4.0).contains(&s.earth_k)) {
        return Err(SimError::BadSite("earth radius factor"));
    }
    if !(fin(s.bias_range_m) && s.bias_range_m.abs() <= 10_000.0) {
        return Err(SimError::BadSite("range bias"));
    }
    if !(fin(s.bias_az_deg) && s.bias_az_deg.abs() <= 10.0) {
        return Err(SimError::BadSite("azimuth bias"));
    }
    if !(fin(s.split_dr_m) && s.split_dr_m.abs() <= 10_000.0) {
        return Err(SimError::BadSite("split range offset"));
    }
    if !(fin(s.split_daz_deg) && s.split_daz_deg.abs() <= 10.0) {
        return Err(SimError::BadSite("split azimuth offset"));
    }
    let p = &s.profile;
    if !(fin(p.range_sigma_m) && p.range_sigma_m >= 0.0) {
        return Err(SimError::BadSite("range sigma"));
    }
    if !(fin(p.az_sigma_deg) && p.az_sigma_deg >= 0.0) {
        return Err(SimError::BadSite("azimuth sigma"));
    }
    if !(fin(p.range_quantum_m) && p.range_quantum_m >= 0.0) {
        return Err(SimError::BadSite("range quantum"));
    }
    if p.alt_quantum_ft <= 0 {
        return Err(SimError::BadSite("altitude quantum"));
    }
    for (v, w) in [
        (p.pd, "detection probability"),
        (p.pd_mode_a, "Mode A probability"),
        (p.pd_alt, "altitude probability"),
        (p.false_rate, "false rate"),
        (p.split_rate, "split rate"),
    ] {
        if !(0.0..=1.0).contains(&v) {
            return Err(SimError::BadSite(w));
        }
    }
    Ok(())
}

/// The scan period in whole nanoseconds.
pub fn period_ns(site: &RadarSite) -> i64 {
    (site.rotation_period_s * 1.0e9).round() as i64
}

/// Start of revolution `k`, Unix ns.
pub fn scan_start_ns(site: &RadarSite, k: i64) -> i64 {
    site.epoch_ns
        .saturating_add(k.saturating_mul(period_ns(site)))
}

/// The beam azimuth at `t_ns`, degrees in `[0, 360)`.
pub fn beam_az_deg(site: &RadarSite, t_ns: i64) -> f64 {
    let p = period_ns(site);
    let into = (t_ns - site.epoch_ns).rem_euclid(p);
    wrap360(site.initial_az_deg + 360.0 * into as f64 / p as f64)
}

/// Where the truth puts an aircraft at a time (sub-millisecond remainder applied).
#[derive(Debug, Clone, Copy)]
struct Pos {
    lat: f64,
    lon: f64,
    alt_ft: Option<f64>,
}

fn truth_pos(truth: &Truth, addr: u32, t_ns: i64) -> Option<Pos> {
    let t_ms = t_ns.div_euclid(1_000_000);
    let rem_s = t_ns.rem_euclid(1_000_000) as f64 * 1.0e-9;
    match truth_at(truth, addr, t_ms) {
        TruthQuery::Ok(s) => {
            let (lat, lon) = crate::geom::offset_deg(
                s.lat_deg,
                s.lon_deg,
                s.vel_east_mps * rem_s,
                s.vel_north_mps * rem_s,
            )?;
            let alt_ft = s
                .baro_alt_ft
                .map(|a| a + s.vert_rate_fpm.unwrap_or(0.0) / 60.0 * rem_s);
            Some(Pos { lat, lon, alt_ft })
        }
        _ => None,
    }
}

fn height_m(p: &Pos) -> f64 {
    p.alt_ft.unwrap_or(0.0) * FT_M
}

/// Result of the crossing-time solve for one aircraft and revolution.
enum Solve {
    NoTruth,
    Unsolved,
    At(i64, Pos),
}

fn solve_crossing(
    truth: &Truth,
    addr: u32,
    site: &RadarSite,
    frame: &Frame,
    scan_start: i64,
    first_guess: i64,
) -> Solve {
    let p_ns = period_ns(site);
    let mut t = first_guess;
    for _ in 0..MAX_SOLVE_ITER {
        let Some(p) = truth_pos(truth, addr, t) else {
            return Solve::NoTruth;
        };
        let (_, az) = polar_of(frame, p.lat, p.lon, height_m(&p));
        let frac = wrap360(az - site.initial_az_deg) / 360.0;
        let t_new = (scan_start + (frac * p_ns as f64).round() as i64).min(scan_start + p_ns - 1);
        let moved = (t_new - t).abs();
        t = t_new;
        if moved <= SOLVE_TOL_NS {
            break;
        }
    }
    let Some(p) = truth_pos(truth, addr, t) else {
        return Solve::NoTruth;
    };
    let (_, az) = polar_of(frame, p.lat, p.lon, height_m(&p));
    if az_diff(beam_az_deg(site, t), az).abs() > SOLVE_TOL_DEG {
        return Solve::Unsolved;
    }
    Solve::At(t, p)
}

/// Mode A codes handed out to aircraft: every 12-bit code but 0000 and the emergency codes
/// 7500, 7600 and 7700, in ascending order. The truth carries no squawk, so the simulation
/// assigns each aircraft (by its rank in the truth's address order) the next code, wrapping
/// after 4092 aircraft.
fn mode_a_codes() -> Vec<u16> {
    (1u16..=0o7777)
        .filter(|c| !matches!(*c, 0o7500 | 0o7600 | 0o7700))
        .collect()
}

fn quantise_alt(alt_ft: f64, q: i32) -> i32 {
    let q = f64::from(q);
    ((alt_ft / q).round() * q) as i32
}

/// Simulates one radar over the truth, for revolutions overlapping `[from_ns, to_ns)`. Returns
/// the plots in time order (ties in generation order) and the counts. Refuses an invalid site
/// (E5) and a run that would pass [`MAX_PLOTS`].
pub fn simulate_radar(
    truth: &Truth,
    site: &RadarSite,
    seed: u64,
    from_ns: i64,
    to_ns: i64,
) -> Result<(Plots, SimStats), SimError> {
    check_site(site)?;
    let frame = frame_at(site.lat_deg, site.lon_deg, site.height_m);
    let prof = &site.profile;
    let p_ns = period_ns(site);
    let codes = mode_a_codes();
    let mut stats = SimStats::default();
    let mut rows: Vec<Plot> = Vec::new();
    let addrs = aircraft_addrs(truth);
    for (rank, &addr) in addrs.iter().enumerate() {
        let range = legs_of(truth, addr);
        stats.aircraft += 1;
        let mode_a = codes.get(rank % codes.len()).copied().unwrap_or(NONE_CODE);
        let rep_addr = if addr & NON_ICAO == 0 && prof.mode_s {
            addr
        } else {
            NONE_ADDR
        };
        for l in 0..range.count {
            let leg = LegIx(range.first.0 + l);
            let (Some(ls), Some(le)) = (leg_start_ns(truth, leg), leg_end_ns(truth, leg)) else {
                continue;
            };
            stats.legs += 1;
            let (lo, hi) = (ls.max(from_ns), le.min(to_ns.saturating_sub(1)));
            if lo > hi {
                continue;
            }
            let k0 = (lo - site.epoch_ns).div_euclid(p_ns);
            let k1 = (hi - site.epoch_ns).div_euclid(p_ns);
            for k in k0..=k1 {
                stats.crossings += 1;
                let start = scan_start_ns(site, k);
                let guess = (start + p_ns / 2).clamp(ls, le);
                let (t, pos) = match solve_crossing(truth, addr, site, &frame, start, guess) {
                    Solve::NoTruth => {
                        stats.no_truth += 1;
                        continue;
                    }
                    Solve::Unsolved => {
                        stats.unsolved += 1;
                        continue;
                    }
                    Solve::At(t, p) => (t, p),
                };
                if t < from_ns || t >= to_ns {
                    continue;
                }
                let h = height_m(&pos);
                let (r_true, az_true) = polar_of(&frame, pos.lat, pos.lon, h);
                if r_true > site.coverage_range_m {
                    stats.out_of_range += 1;
                    continue;
                }
                let theta = central_angle(site.lat_deg, site.lon_deg, pos.lat, pos.lon);
                if !line_of_sight(site.height_m, h, theta, site.earth_k) {
                    stats.below_horizon += 1;
                    continue;
                }
                stats.in_coverage += 1;
                let mut rng = Rng::from_stream(seed, u64::from(addr), k as u64);
                let det = rng.chance(prof.pd);
                let has_a = rng.chance(prof.pd_mode_a);
                let has_alt = rng.chance(prof.pd_alt);
                let (nr, na) = (rng.normal(), rng.normal());
                let false_hit = rng.chance(prof.false_rate);
                let (fa, fr) = (rng.unit(), rng.unit());
                let split_hit = rng.chance(prof.split_rate);
                let (s1, s2) = (rng.chance(0.5), rng.chance(0.5));
                if !det {
                    stats.missed += 1;
                    continue;
                }
                let mut range_m = r_true + site.bias_range_m + prof.range_sigma_m * nr;
                if prof.range_quantum_m > 0.0 {
                    range_m = (range_m / prof.range_quantum_m).round() * prof.range_quantum_m;
                }
                let az_deg = wrap360(az_true + site.bias_az_deg + prof.az_sigma_deg * na);
                let alt_ft = match pos.alt_ft {
                    Some(a) if has_alt => quantise_alt(a, prof.alt_quantum_ft),
                    _ => NONE_ALT,
                };
                if !(range_m.is_finite() && az_deg.is_finite()) {
                    stats.non_finite += 1;
                    continue;
                }
                let base = Plot {
                    time_ns: t,
                    scan: k,
                    kind: KIND_TRUE,
                    range_m,
                    az_deg,
                    alt_ft,
                    mode_a: if has_a { mode_a } else { NONE_CODE },
                    addr: rep_addr,
                    truth_addr: addr,
                    truth_range_m: r_true,
                    truth_az_deg: az_true,
                };
                rows.push(base);
                stats.detected += 1;
                if split_hit {
                    let sg = |b: bool| if b { 1.0 } else { -1.0 };
                    rows.push(Plot {
                        kind: KIND_SPLIT,
                        range_m: range_m + sg(s1) * site.split_dr_m,
                        az_deg: wrap360(az_deg + sg(s2) * site.split_daz_deg),
                        ..base
                    });
                    stats.split_plots += 1;
                }
                if false_hit {
                    // Uniform in area over the coverage disc, uniform in azimuth; the time is
                    // when the beam is at that azimuth in the same revolution.
                    let az = fa * 360.0;
                    let frac = wrap360(az - site.initial_az_deg) / 360.0;
                    let ft = (start + (frac * p_ns as f64).round() as i64).min(start + p_ns - 1);
                    rows.push(Plot {
                        time_ns: ft,
                        scan: k,
                        kind: KIND_FALSE,
                        range_m: site.coverage_range_m * fr.sqrt(),
                        az_deg: az,
                        alt_ft: NONE_ALT,
                        mode_a: NONE_CODE,
                        addr: NONE_ADDR,
                        truth_addr: NONE_ADDR,
                        truth_range_m: 0.0,
                        truth_az_deg: 0.0,
                    });
                    stats.false_plots += 1;
                }
                if rows.len() > MAX_PLOTS {
                    return Err(SimError::TooManyPlots);
                }
            }
        }
    }
    let mut order: Vec<usize> = (0..rows.len()).collect();
    order.sort_by_key(|&i| (rows.get(i).map_or(i64::MAX, |r| r.time_ns), i));
    let mut plots = Plots::default();
    for i in order {
        if let Some(r) = rows.get(i) {
            plots_push(&mut plots, r);
        }
    }
    debug_assert_eq!(plots_len(&plots), rows.len());
    Ok((plots, stats))
}
