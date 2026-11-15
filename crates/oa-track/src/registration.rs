//! Radar registration: each radar's range and azimuth bias, estimated online against ADS-B
//! tracks of high integrity, with their uncertainty. Core (pure).
//!
//! # Estimator
//!
//! The two biases are independent scalars, each a constant (a random walk of small density,
//! [`crate::fusion_params::REG_Q_RANGE_M2_PER_S`]) observed directly: a Kalman filter per bias
//! with state `b`, variance `P`, prediction `P += q dt`, and measurement `z = plot - predicted`
//! where the prediction is the reference track's position seen from the radar by the exact
//! forward model (`radar::polar_of`). The measurement variance is the plot's random error
//! (range or azimuth sigma, range quantum) plus the reference track's own position variance
//! projected on the radial or tangential direction, plus the height term, so an imprecise
//! reference counts less. The starting variance is the assumed prior
//! ([`crate::fusion_params::REG_PRIOR_SIGMA_RANGE_M`]); no sourced bias exists, so the prior is a
//! stated assumption and the report sweeps the true bias instead.
//!
//! A residual more than [`REG_GATE_SIGMAS`] standard deviations from the estimate is rejected
//! and counted (a wrong association or an outlier must not pull the estimate). The estimate is
//! clamped to a limit and the clamp counted.
//!
//! The estimate is *subtracted* from the radar's plots and its variance added to theirs
//! (`radar::plot_convert`), so a radar that is not yet registered is weighted as the
//! uncertainty says. [`RegSample`] rows are the published history per radar over time.

use crate::fusion_params::{
    REG_GATE_SIGMAS, REG_LIMIT_AZ_DEG, REG_LIMIT_RANGE_M, REG_PRIOR_SIGMA_AZ_DEG,
    REG_PRIOR_SIGMA_RANGE_M, REG_Q_AZ_DEG2_PER_S, REG_Q_RANGE_M2_PER_S,
};
use crate::radar::{polar_of, Bias, RadarGeom, RadarPlot, RadarSite, FT_M};

/// One radar's bias estimate. Plain data (A1); A7.2: cold struct (one per radar).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RegState {
    pub range_m: f64,
    pub var_range_m2: f64,
    pub az_deg: f64,
    pub var_az_deg2: f64,
    /// Time of the last prediction or update, Unix ns; `i64::MIN` before the first.
    pub t_ns: i64,
    /// Residual pairs used.
    pub used: u64,
    /// Residual pairs rejected by the gate.
    pub rejected: u64,
    /// Updates that hit the limit.
    pub clamped: u64,
}

/// One published estimate. Plain data (A1); A7.2: cold struct (a log row).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RegSample {
    pub t_ns: i64,
    pub range_m: f64,
    pub range_sigma_m: f64,
    pub az_deg: f64,
    pub az_sigma_deg: f64,
    pub used: u64,
}

/// A residual of a plot against a reference position, with variances. Plain data (A1); A7.2:
/// transient.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Residual {
    pub range_m: f64,
    pub var_range_m2: f64,
    pub az_deg: f64,
    pub var_az_deg2: f64,
}

/// The estimator at the prior.
pub fn reg_new() -> RegState {
    RegState {
        range_m: 0.0,
        var_range_m2: REG_PRIOR_SIGMA_RANGE_M * REG_PRIOR_SIGMA_RANGE_M,
        az_deg: 0.0,
        var_az_deg2: REG_PRIOR_SIGMA_AZ_DEG * REG_PRIOR_SIGMA_AZ_DEG,
        t_ns: i64::MIN,
        used: 0,
        rejected: 0,
        clamped: 0,
    }
}

/// The correction to apply: the estimate and its variance.
pub fn reg_bias(r: &RegState) -> Bias {
    Bias {
        range_m: r.range_m,
        az_deg: r.az_deg,
        var_range_m2: r.var_range_m2,
        var_az_deg2: r.var_az_deg2,
    }
}

/// The estimate as a published row.
pub fn reg_sample(r: &RegState, t_ns: i64) -> RegSample {
    RegSample {
        t_ns,
        range_m: r.range_m,
        range_sigma_m: r.var_range_m2.max(0.0).sqrt(),
        az_deg: r.az_deg,
        az_sigma_deg: r.var_az_deg2.max(0.0).sqrt(),
        used: r.used,
    }
}

/// Wraps a difference of azimuths in degrees to `[-180, 180)`.
fn wrap180(d: f64) -> f64 {
    (d + 180.0).rem_euclid(360.0) - 180.0
}

/// The residual of `plot` against a reference at geodetic `(lat, lon, h_m)` whose position
/// covariance in the local plane is `cov = [ee, en, nn]` (m^2) and height variance `var_h_m2`.
// Eight plain inputs of one residual; a struct would only rename them.
#[allow(clippy::too_many_arguments)]
pub fn reg_residual(
    g: &RadarGeom,
    s: &RadarSite,
    plot: &RadarPlot,
    lat_deg: f64,
    lon_deg: f64,
    h_m: f64,
    cov: [f64; 3],
    var_h_m2: f64,
) -> Residual {
    let (rho, az) = polar_of(g, lat_deg, lon_deg, h_m);
    let dh = h_m - g.height_m;
    let ground = (rho * rho - dh * dh).max(1.0).sqrt();
    let cos_e = (ground / rho).clamp(0.05, 1.0);
    let sin_e = dh / rho;
    let (st, ct) = az.to_radians().sin_cos();
    // Radial (u) and tangential (v) horizontal unit vectors.
    let (u, v) = ([st, ct], [ct, -st]);
    let quad =
        |a: [f64; 2]| a[0] * a[0] * cov[0] + 2.0 * a[0] * a[1] * cov[1] + a[1] * a[1] * cov[2];
    let var_r_track = cos_e * cos_e * quad(u) + sin_e * sin_e * var_h_m2;
    let var_t_track = quad(v) / (ground * ground);
    let sa = s.sigma_az_deg.to_radians();
    let deg2 = (180.0 / core::f64::consts::PI).powi(2);
    Residual {
        range_m: plot.range_m - rho,
        var_range_m2: s.sigma_range_m * s.sigma_range_m
            + s.range_quantum_m * s.range_quantum_m / 12.0
            + var_r_track,
        az_deg: wrap180(plot.az_deg - az),
        var_az_deg2: (sa * sa + var_t_track) * deg2,
    }
}

/// Advances the estimate to `t_ns` (adds the random walk). Time never goes backwards.
pub fn reg_predict(r: &mut RegState, t_ns: i64) {
    if r.t_ns != i64::MIN && t_ns > r.t_ns {
        let dt = (t_ns - r.t_ns) as f64 / 1.0e9;
        r.var_range_m2 += REG_Q_RANGE_M2_PER_S * dt;
        r.var_az_deg2 += REG_Q_AZ_DEG2_PER_S * dt;
    }
    if r.t_ns == i64::MIN || t_ns > r.t_ns {
        r.t_ns = t_ns;
    }
}

/// Applies a residual at `t_ns`; returns whether it was used (false: rejected by the gate).
pub fn reg_update(r: &mut RegState, t_ns: i64, z: &Residual) -> bool {
    reg_predict(r, t_ns);
    let (sr, sa) = (
        (r.var_range_m2 + z.var_range_m2).sqrt(),
        (r.var_az_deg2 + z.var_az_deg2).sqrt(),
    );
    let (dr, da) = (z.range_m - r.range_m, z.az_deg - r.az_deg);
    if !(dr.is_finite() && da.is_finite())
        || dr.abs() > REG_GATE_SIGMAS * sr
        || da.abs() > REG_GATE_SIGMAS * sa
    {
        r.rejected += 1;
        return false;
    }
    let kr = r.var_range_m2 / (r.var_range_m2 + z.var_range_m2);
    let ka = r.var_az_deg2 / (r.var_az_deg2 + z.var_az_deg2);
    r.range_m += kr * dr;
    r.var_range_m2 *= 1.0 - kr;
    r.az_deg += ka * da;
    r.var_az_deg2 *= 1.0 - ka;
    let (cr, ca) = (
        r.range_m.clamp(-REG_LIMIT_RANGE_M, REG_LIMIT_RANGE_M),
        r.az_deg.clamp(-REG_LIMIT_AZ_DEG, REG_LIMIT_AZ_DEG),
    );
    if cr != r.range_m || ca != r.az_deg {
        r.clamped += 1;
    }
    r.range_m = cr;
    r.az_deg = ca;
    r.used += 1;
    true
}

/// The height of a track in metres from its altitude in feet (the model's pressure-altitude
/// convention, `radar` module documentation).
pub fn height_of_ft(alt_ft: f64) -> f64 {
    alt_ft * FT_M
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use crate::radar::{polar_to_geodetic, radar_geom, NONE_CODE};
    use oa_sensorsim::prng::Rng;

    const SITE: RadarSite = RadarSite {
        lat_deg: 41.976_940_27,
        lon_deg: -87.908_149_72,
        height_m: 207.0,
        sigma_range_m: 70.0,
        sigma_az_deg: 0.08,
        range_quantum_m: 0.0,
        alt_quantum_ft: 100,
        period_ns: 4_800_000_000,
        epoch_ns: 0,
        sensor: 1,
    };

    // REQ: OA-TRK-037
    #[test]
    fn the_estimate_converges_to_the_true_bias_with_an_honest_uncertainty() {
        let g = radar_geom(&SITE);
        let mut rng = Rng::from_seed(11);
        let (true_r, true_a) = (120.0, 0.12);
        let mut r = reg_new();
        let mut errs = Vec::new();
        for trial in 0..40 {
            let mut st = reg_new();
            let mut t = 0i64;
            for k in 0..150 {
                // A reference aircraft somewhere in the coverage, its position known to 5 m.
                let (lat, lon, h) = (
                    41.0 + 2.0 * rng.unit(),
                    -89.0 + 2.5 * rng.unit(),
                    3_000.0 + 8_000.0 * rng.unit(),
                );
                let (rho, az) = polar_of(&g, lat, lon, h);
                let plot = RadarPlot {
                    time_ns: t,
                    range_m: rho + true_r + 70.0 * rng.normal(),
                    az_deg: az + true_a + 0.08 * rng.normal(),
                    alt_ft: 0,
                    addr: u32::MAX,
                    label: u32::MAX,
                    mode_a: NONE_CODE,
                };
                let z = reg_residual(&g, &SITE, &plot, lat, lon, h, [25.0, 0.0, 25.0], 100.0);
                reg_update(&mut st, t, &z);
                t += 1_000_000_000 * (1 + i64::from(k % 3));
            }
            errs.push((
                (st.range_m - true_r) / st.var_range_m2.sqrt(),
                (st.az_deg - true_a) / st.var_az_deg2.sqrt(),
            ));
            if trial == 0 {
                r = st;
            }
        }
        // 150 residuals of 70 m: standard error about 5.7 m and 0.0065 degree.
        assert!(r.var_range_m2.sqrt() < 8.0, "{}", r.var_range_m2.sqrt());
        assert!(r.var_az_deg2.sqrt() < 0.01, "{}", r.var_az_deg2.sqrt());
        // Honest: the standardised error has unit-order spread across trials.
        let n = errs.len() as f64;
        let rms_r = (errs.iter().map(|e| e.0 * e.0).sum::<f64>() / n).sqrt();
        let rms_a = (errs.iter().map(|e| e.1 * e.1).sum::<f64>() / n).sqrt();
        assert!((0.6..1.5).contains(&rms_r), "range z rms {rms_r}");
        assert!((0.6..1.5).contains(&rms_a), "az z rms {rms_a}");
    }

    // REQ: OA-TRK-038
    #[test]
    fn outliers_are_rejected_and_the_estimate_is_clamped() {
        let mut r = reg_new();
        let ok = Residual {
            range_m: 10.0,
            var_range_m2: 5_000.0,
            az_deg: 0.01,
            var_az_deg2: 0.01,
        };
        for k in 0..60 {
            reg_update(&mut r, k * 1_000_000_000, &ok);
        }
        let before = r;
        // A residual 3 km away (a wrong association) is rejected, estimate unchanged.
        let wild = Residual {
            range_m: 3_000.0,
            ..ok
        };
        assert!(!reg_update(&mut r, 61_000_000_000, &wild));
        assert_eq!(r.rejected, before.rejected + 1);
        assert!((r.range_m - before.range_m).abs() < 1e-9);
        // An estimate near the limit, pulled further by a consistent residual, is clamped and
        // counted.
        let mut r2 = RegState {
            range_m: 1_990.0,
            var_range_m2: 400.0,
            ..reg_new()
        };
        let beyond = Residual {
            range_m: 2_050.0,
            var_range_m2: 400.0,
            az_deg: 0.0,
            var_az_deg2: 0.01,
        };
        assert!(reg_update(&mut r2, 0, &beyond));
        assert!((r2.range_m - REG_LIMIT_RANGE_M).abs() < 1e-9);
        assert_eq!(r2.clamped, 1);
    }

    // REQ: OA-TRK-039
    #[test]
    fn a_less_certain_reference_counts_less() {
        let g = radar_geom(&SITE);
        let (lat, lon, h) = (42.3, -87.5, 9_000.0);
        let (rho, az) = polar_of(&g, lat, lon, h);
        let plot = RadarPlot {
            time_ns: 0,
            range_m: rho + 50.0,
            az_deg: az,
            alt_ft: 0,
            addr: u32::MAX,
            label: u32::MAX,
            mode_a: NONE_CODE,
        };
        let tight = reg_residual(&g, &SITE, &plot, lat, lon, h, [16.0, 0.0, 16.0], 25.0);
        let loose = reg_residual(&g, &SITE, &plot, lat, lon, h, [2_500.0, 0.0, 2_500.0], 25.0);
        assert!((tight.range_m - 50.0).abs() < 1e-6);
        assert!(loose.var_range_m2 > tight.var_range_m2 + 1_000.0);
        assert!(loose.var_az_deg2 > tight.var_az_deg2);
        // The unpredictable part: variance never below the plot's own.
        assert!(tight.var_range_m2 >= 70.0 * 70.0);
        // The conversion round trip is consistent with the residual's geometry.
        let geo = polar_to_geodetic(&g, rho, az, h).unwrap();
        assert!((geo.lat_deg - lat).abs() < 1e-6);
    }
}
