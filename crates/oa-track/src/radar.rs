//! The secondary-radar measurement model: a plot in slant range and azimuth becomes a tracker
//! [`Measurement`] with a covariance in the local east-north plane. Core (pure).
//!
//! # The conversion, and its error
//!
//! A plot is (slant range `rho`, azimuth `theta` clockwise from true north, altitude). The radar's
//! frame is the WGS-84 east-north-up frame at its antenna, the same frame the simulator
//! (`oa-sensorsim`, `geom.rs`) builds a plot in, so the conversion is the exact inverse of that
//! forward model and not an approximation of a sphere:
//!
//! 1. The target is at `radar + rho * (cos(E) sin(theta), cos(E) cos(theta), sin(E))` in that
//!    frame, with the elevation `E` unknown. The height of the target above the ellipsoid is the
//!    Mode C altitude (the simulator, like this model, takes pressure altitude as ellipsoidal
//!    height; the real difference between the two is an unmodelled systematic term, stated in
//!    the report).
//! 2. `E` is the root of `height(E) - h_target = 0`. The start is the spherical closed form
//!    `sin(E) = ((R + h)^2 - (R + h_r)^2 - rho^2) / (2 (R + h_r) rho)` (law of cosines in the
//!    triangle earth centre, antenna, target, with `R` the Gaussian mean radius at the antenna),
//!    then Newton steps with `d height / dE = rho cos(E)` and the exact ECEF to geodetic
//!    conversion ([`geodetic_of_ecef`]) until the height is within [`HEIGHT_TOL_M`]. This takes 1
//!    or 2 steps; the method error is the tolerance (0.1 mm of height, well under a micrometre of
//!    position), checked against the simulator's forward model in the tests. What it leaves out is
//!    atmospheric refraction (the simulator does not model it either; a real radar's refraction
//!    correction belongs in front of this function) and the pressure-altitude to geometric-height
//!    difference.
//! 3. The covariance is first order. With `s` the ground arc, `a = R + h_r` and `b = R + h`, at
//!    fixed height `ds/drho = R / (a cos E)` and at fixed range
//!    `ds/dh = -(rho / a + sin E) R / (b cos E)`; the cross-range error is `s * sigma_theta`.
//!    The local covariance is `sigma_s^2 u u' + sigma_c^2 v v'` with `u` the radial and `v` the
//!    tangential unit vector. The neglected terms are second order in `sigma_theta` (a bias of
//!    `s sigma_theta^2 / 2`, 0.1 m at 100 km and 0.08 degree) and in the earth-curvature ratio
//!    `s / R` (a relative error of a few parts in 1e3 of the radial term at 250 km).
//!
//! The range quantum of the sensor adds `q^2 / 12` to the range variance and the altitude
//! quantum `q^2 / 12` to the height variance. A radar's registration estimate
//! (`registration.rs`) enters as a [`Bias`]: the correction is subtracted from range and azimuth
//! before the conversion and the estimate's variance is added to the plot's, so a radar with an
//! unknown bias is weighted as the uncertainty says.

use crate::types::{Measurement, NONE_I32, NO_ADDR, NO_IDENT};

const WGS84_A: f64 = 6_378_137.0;
const WGS84_E2: f64 = 0.006_694_379_990_141_316;
/// Metres per foot (exact).
pub const FT_M: f64 = 0.3048;
/// Newton stops when the computed height is within this of the plot's, metres.
pub const HEIGHT_TOL_M: f64 = 1.0e-4;
/// Limit: Newton steps in the conversion.
pub const MAX_NEWTON: u32 = 8;
/// Limit: plot range accepted, metres (the CAT048 range element reaches 256 NM; 600 km).
pub const MAX_PLOT_RANGE_M: f64 = 600_000.0;
/// Plots closer than this to the antenna are refused: the cone of silence, where the elevation
/// is steep and ground range is ill-conditioned, metres.
pub const MIN_PLOT_RANGE_M: f64 = 500.0;
/// The conversion's covariance uses `cos(E)` no smaller than this (E about 87 degrees).
pub const MIN_COS_ELEV: f64 = 0.05;
/// Absent altitude of a plot.
pub const NONE_ALT: i32 = i32::MIN;
/// Absent Mode A code of a plot.
pub const NONE_CODE: u16 = u16::MAX;
/// The absent address, for plots (the tracker's [`NO_ADDR`]).
pub const PLOT_NO_ADDR: u32 = NO_ADDR;

/// Why a site, plot or conversion was refused (E5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RadarError {
    /// A site value is out of range; the name says which.
    BadSite(&'static str),
    /// A plot value is out of range; the name says which.
    BadPlot(&'static str),
    /// The plot has no altitude and none was supplied.
    NoHeight,
    /// The conversion did not converge (cannot happen for a valid plot; counted if it does).
    NoConvergence,
}

impl std::fmt::Display for RadarError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RadarError::BadSite(w) => write!(f, "bad radar site: {w}"),
            RadarError::BadPlot(w) => write!(f, "bad radar plot: {w}"),
            RadarError::NoHeight => write!(f, "plot has no altitude and none was supplied"),
            RadarError::NoConvergence => write!(f, "polar conversion did not converge"),
        }
    }
}

impl std::error::Error for RadarError {}

/// A radar site as the tracker needs it. Plain data (A1); A7.2: cold struct (one per radar).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RadarSite {
    pub lat_deg: f64,
    pub lon_deg: f64,
    /// Antenna height above the ellipsoid, metres.
    pub height_m: f64,
    /// Random range error, one sigma, metres (the sensor's profile).
    pub sigma_range_m: f64,
    /// Random azimuth error, one sigma, degrees.
    pub sigma_az_deg: f64,
    /// Range quantum, metres; 0 for none.
    pub range_quantum_m: f64,
    /// Altitude resolution, feet (100 Mode C, 25 Mode S).
    pub alt_quantum_ft: u16,
    /// Antenna rotation period, ns.
    pub period_ns: i64,
    /// Sensor identifier written to the measurements.
    pub sensor: u16,
}

/// One radar plot as the tracker receives it. Plain data (A1).
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct RadarPlot {
    /// Time of the beam crossing, Unix ns.
    pub time_ns: i64,
    pub range_m: f64,
    /// Degrees clockwise from true north.
    pub az_deg: f64,
    /// Altitude, feet, or [`NONE_ALT`].
    pub alt_ft: i32,
    /// Mode S address (bit 24 set for non-ICAO), or [`NO_ADDR`]. A hard identity.
    pub addr: u32,
    /// Scoring label (the true aircraft behind the plot, or [`NO_ADDR`]). Carried through and
    /// logged for scoring; **never read by any decision** (tested: changing every label does
    /// not change the output).
    pub label: u32,
    /// Mode A code, or [`NONE_CODE`]. Never assigns a plot (LS-609); copied to the measurement.
    pub mode_a: u16,
}
// Budget: 8 + 8 + 8 + 4 + 4 + 4 + 2 = 38 packed; 40 with alignment.
const _: () = assert!(core::mem::size_of::<RadarPlot>() == 40);

/// A radar's registration correction and its uncertainty: the bias estimate to subtract and its
/// variance. Plain data (A1); A7.2: transient value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bias {
    pub range_m: f64,
    pub az_deg: f64,
    pub var_range_m2: f64,
    pub var_az_deg2: f64,
}

/// No correction and no extra uncertainty.
pub const NO_BIAS: Bias = Bias {
    range_m: 0.0,
    az_deg: 0.0,
    var_range_m2: 0.0,
    var_az_deg2: 0.0,
};

/// The radar's frame, computed once from its site. Plain data (A1); A7.2: cold, one per radar.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RadarGeom {
    pub origin: [f64; 3],
    pub east: [f64; 3],
    pub north: [f64; 3],
    pub up: [f64; 3],
    /// Gaussian mean radius of curvature at the antenna, metres.
    pub r_mean_m: f64,
    pub height_m: f64,
}

fn dot(a: &[f64; 3], b: &[f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Geodetic to earth-centred earth-fixed metres (WGS-84).
pub fn ecef(lat_deg: f64, lon_deg: f64, h_m: f64) -> [f64; 3] {
    let (lat, lon) = (lat_deg.to_radians(), lon_deg.to_radians());
    let (sl, cl) = (lat.sin(), lat.cos());
    let n = WGS84_A / (1.0 - WGS84_E2 * sl * sl).sqrt();
    [
        (n + h_m) * cl * lon.cos(),
        (n + h_m) * cl * lon.sin(),
        (n * (1.0 - WGS84_E2) + h_m) * sl,
    ]
}

/// Earth-centred earth-fixed metres to geodetic `(lat_deg, lon_deg, height_m)` by fixed-point
/// iteration on the latitude (converges to under 1e-9 rad in 5 steps; stopped at 1e-13).
pub fn geodetic_of_ecef(p: &[f64; 3]) -> (f64, f64, f64) {
    let [x, y, z] = *p;
    let r = x.hypot(y);
    let lon = y.atan2(x);
    let mut lat = z.atan2(r * (1.0 - WGS84_E2));
    let height = |lat: f64| -> (f64, f64) {
        let s = lat.sin();
        let n = WGS84_A / (1.0 - WGS84_E2 * s * s).sqrt();
        let h = if lat.cos().abs() > 0.1 {
            r / lat.cos() - n
        } else {
            z / s - n * (1.0 - WGS84_E2)
        };
        (n, h)
    };
    for _ in 0..8 {
        let (n, h) = height(lat);
        let next = z.atan2(r * (1.0 - WGS84_E2 * n / (n + h)));
        let done = (next - lat).abs() < 1.0e-13;
        lat = next;
        if done {
            break;
        }
    }
    let (_, h) = height(lat);
    (lat.to_degrees(), lon.to_degrees(), h)
}

/// Checks a site (E5).
pub fn site_check(s: &RadarSite) -> Result<(), RadarError> {
    let fin = |v: f64| v.is_finite();
    if !(fin(s.lat_deg) && (-85.0..=85.0).contains(&s.lat_deg)) {
        return Err(RadarError::BadSite("latitude"));
    }
    if !(fin(s.lon_deg) && (-180.0..=180.0).contains(&s.lon_deg)) {
        return Err(RadarError::BadSite("longitude"));
    }
    if !(fin(s.height_m) && (-500.0..=9000.0).contains(&s.height_m)) {
        return Err(RadarError::BadSite("height"));
    }
    if !(fin(s.sigma_range_m) && s.sigma_range_m > 0.0 && s.sigma_range_m < 10_000.0) {
        return Err(RadarError::BadSite("range sigma"));
    }
    if !(fin(s.sigma_az_deg) && s.sigma_az_deg > 0.0 && s.sigma_az_deg < 10.0) {
        return Err(RadarError::BadSite("azimuth sigma"));
    }
    if !(fin(s.range_quantum_m) && (0.0..=10_000.0).contains(&s.range_quantum_m)) {
        return Err(RadarError::BadSite("range quantum"));
    }
    if s.alt_quantum_ft == 0 || s.alt_quantum_ft > 1000 {
        return Err(RadarError::BadSite("altitude quantum"));
    }
    if !(100_000_000..=600_000_000_000).contains(&s.period_ns) {
        return Err(RadarError::BadSite("rotation period"));
    }
    Ok(())
}

/// The frame of a valid site.
pub fn radar_geom(s: &RadarSite) -> RadarGeom {
    let (lat, lon) = (s.lat_deg.to_radians(), s.lon_deg.to_radians());
    let (sl, cl, so, co) = (lat.sin(), lat.cos(), lon.sin(), lon.cos());
    let w2 = 1.0 - WGS84_E2 * sl * sl;
    RadarGeom {
        origin: ecef(s.lat_deg, s.lon_deg, s.height_m),
        east: [-so, co, 0.0],
        north: [-sl * co, -sl * so, cl],
        up: [cl * co, cl * so, sl],
        r_mean_m: WGS84_A * (1.0 - WGS84_E2).sqrt() / w2,
        height_m: s.height_m,
    }
}

/// Slant range (m) and azimuth (degrees clockwise from north, `[0, 360)`) of a geodetic point
/// seen from the radar: the exact forward model, the one the simulator uses.
pub fn polar_of(g: &RadarGeom, lat_deg: f64, lon_deg: f64, h_m: f64) -> (f64, f64) {
    let p = ecef(lat_deg, lon_deg, h_m);
    let d = [
        p[0] - g.origin[0],
        p[1] - g.origin[1],
        p[2] - g.origin[2],
    ];
    let az = dot(&d, &g.east).atan2(dot(&d, &g.north)).to_degrees();
    (dot(&d, &d).sqrt(), az.rem_euclid(360.0))
}

/// A converted plot position. Plain data (A1); A7.2: transient value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Geo {
    pub lat_deg: f64,
    pub lon_deg: f64,
    /// Elevation angle of the line of sight at the antenna, radians.
    pub elev_rad: f64,
    /// Ground arc from the antenna to the point under the target, metres.
    pub ground_m: f64,
}

/// The geodetic position of a target at slant range `rho_m`, azimuth `az_deg` and ellipsoidal
/// height `h_m` (the exact inverse of [`polar_of`]; see the module documentation).
pub fn polar_to_geodetic(
    g: &RadarGeom,
    rho_m: f64,
    az_deg: f64,
    h_m: f64,
) -> Result<Geo, RadarError> {
    if !(rho_m.is_finite() && az_deg.is_finite() && h_m.is_finite()) || rho_m < 1.0 {
        return Err(RadarError::BadPlot("range, azimuth or height"));
    }
    let (r, a) = (g.r_mean_m, g.r_mean_m + g.height_m);
    let b = r + h_m;
    let sin_e = ((b * b - a * a - rho_m * rho_m) / (2.0 * a * rho_m)).clamp(-1.0, 1.0);
    let mut e = sin_e.asin();
    let (st, ct) = az_deg.to_radians().sin_cos();
    let at = |e: f64| -> (f64, f64, f64) {
        let (se, ce) = e.sin_cos();
        let mut p = g.origin;
        for (i, c) in p.iter_mut().enumerate() {
            let dir = ce * st * g.east.get(i).copied().unwrap_or(0.0)
                + ce * ct * g.north.get(i).copied().unwrap_or(0.0)
                + se * g.up.get(i).copied().unwrap_or(0.0);
            *c += rho_m * dir;
        }
        geodetic_of_ecef(&p)
    };
    let mut res = at(e);
    for _ in 0..MAX_NEWTON {
        let err = res.2 - h_m;
        if err.abs() <= HEIGHT_TOL_M {
            let cos_e = e.cos();
            let phi = (rho_m * cos_e).atan2(a + rho_m * e.sin());
            return Ok(Geo {
                lat_deg: res.0,
                lon_deg: res.1,
                elev_rad: e,
                ground_m: r * phi,
            });
        }
        let d = rho_m * e.cos();
        if d.abs() < 1.0e-6 {
            break;
        }
        e -= err / d;
        res = at(e);
    }
    Err(RadarError::NoConvergence)
}

/// The local east-north covariance `[ee, en, nn]`, m^2, of a plot at slant range `rho_m`,
/// azimuth `az_deg`, elevation `elev_rad` and ground arc `ground_m`, with range variance
/// `var_rho_m2`, azimuth variance `var_az_rad2` and height variance `var_h_m2` (first order; see
/// the module documentation).
pub fn polar_cov_local(
    g: &RadarGeom,
    rho_m: f64,
    az_deg: f64,
    elev_rad: f64,
    ground_m: f64,
    var_rho_m2: f64,
    var_az_rad2: f64,
    var_h_m2: f64,
) -> [f64; 3] {
    let (a, r) = (g.r_mean_m + g.height_m, g.r_mean_m);
    let b = a + rho_m * elev_rad.sin();
    let cos_e = elev_rad.cos().max(MIN_COS_ELEV);
    let dsdrho = r / (a * cos_e);
    let dsdh = (rho_m / a + elev_rad.sin()) * r / (b * cos_e);
    let var_s = dsdrho * dsdrho * var_rho_m2 + dsdh * dsdh * var_h_m2;
    let var_c = ground_m * ground_m * var_az_rad2;
    let (st, ct) = az_deg.to_radians().sin_cos();
    [
        var_s * st * st + var_c * ct * ct,
        (var_s - var_c) * st * ct,
        var_s * ct * ct + var_c * st * st,
    ]
}

/// Checks a plot (E5).
pub fn plot_check(p: &RadarPlot) -> Result<(), RadarError> {
    if !(p.range_m.is_finite() && (MIN_PLOT_RANGE_M..=MAX_PLOT_RANGE_M).contains(&p.range_m)) {
        return Err(RadarError::BadPlot("range"));
    }
    if !(p.az_deg.is_finite() && (0.0..=360.0).contains(&p.az_deg)) {
        return Err(RadarError::BadPlot("azimuth"));
    }
    if p.alt_ft != NONE_ALT && !(-2_000..=100_000).contains(&p.alt_ft) {
        return Err(RadarError::BadPlot("altitude"));
    }
    Ok(())
}

/// A plot made into a measurement, with the pieces the association needs. A7.2: transient.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Converted {
    pub m: Measurement,
    pub geo: Geo,
}

/// The plot as a [`Measurement`] at its own time, after subtracting `bias` and adding its
/// variance. The height is the plot's Mode C altitude when it has one, else `height_m` (a
/// track's altitude, with `height_sigma_m` its standard deviation), else the plot is refused
/// with [`RadarError::NoHeight`]. The measurement's altitude fields are the plot's (the
/// supplied height is not a measurement). `addr` is the key the measurement is for.
pub fn plot_convert(
    g: &RadarGeom,
    s: &RadarSite,
    p: &RadarPlot,
    bias: &Bias,
    height_m: Option<(f64, f64)>,
    key: u32,
) -> Result<Converted, RadarError> {
    plot_check(p)?;
    let (h, sigma_h) = if p.alt_ft != NONE_ALT {
        (
            f64::from(p.alt_ft) * FT_M,
            f64::from(s.alt_quantum_ft) * FT_M / 12.0_f64.sqrt(),
        )
    } else {
        height_m.ok_or(RadarError::NoHeight)?
    };
    let rho = p.range_m - bias.range_m;
    let az = (p.az_deg - bias.az_deg).rem_euclid(360.0);
    let geo = polar_to_geodetic(g, rho, az, h)?;
    let var_rho = s.sigma_range_m * s.sigma_range_m
        + s.range_quantum_m * s.range_quantum_m / 12.0
        + bias.var_range_m2;
    let sa = s.sigma_az_deg.to_radians();
    let var_az = sa * sa + bias.var_az_deg2 * (core::f64::consts::PI / 180.0).powi(2);
    let cov = polar_cov_local(
        g,
        rho,
        az,
        geo.elev_rad,
        geo.ground_m,
        var_rho,
        var_az,
        sigma_h * sigma_h,
    );
    let m = Measurement {
        toa_ns: p.time_ns,
        pos_cov_m2: [cov[0] as f32, cov[1] as f32, cov[2] as f32],
        vel_cov_m2s2: [0.0; 3],
        vel_east_mps: 0.0,
        vel_north_mps: 0.0,
        lat_e7: (geo.lat_deg * 1.0e7).round() as i32,
        lon_e7: (geo.lon_deg * 1.0e7).round() as i32,
        alt_ft: if p.alt_ft == NONE_ALT {
            NONE_I32
        } else {
            p.alt_ft
        },
        vrate_fpm: NONE_I32,
        addr: key,
        alt_quant_ft: if p.alt_ft == NONE_ALT {
            0
        } else {
            s.alt_quantum_ft
        },
        sensor: s.sensor,
        ident: if p.mode_a == NONE_CODE {
            NO_IDENT
        } else {
            p.mode_a
        },
        flags: 0,
    };
    Ok(Converted { m, geo })
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

    const SITE: RadarSite = RadarSite {
        lat_deg: 41.976_940_27,
        lon_deg: -87.908_149_72,
        height_m: 207.0,
        sigma_range_m: 70.0,
        sigma_az_deg: 0.08,
        range_quantum_m: 0.0,
        alt_quantum_ft: 100,
        period_ns: 4_800_000_000,
        sensor: 1,
    };

    // REQ: OA-TRK-030
    #[test]
    fn conversion_inverts_the_exact_forward_model_to_a_millimetre() {
        let g = radar_geom(&SITE);
        let mut worst: f64 = 0.0;
        let mut n = 0;
        for &rho in &[800.0, 5_000.0, 30_000.0, 100_000.0, 250_000.0, 450_000.0] {
            for k in 0..24 {
                let az = f64::from(k) * 15.0 + 0.37;
                for &h in &[300.0, 3_000.0, 11_000.0] {
                    // Skip geometry below the horizon of the model (height below the antenna's
                    // reach at that range): the inverse is still defined, only unphysical.
                    let Ok(geo) = polar_to_geodetic(&g, rho, az, h) else {
                        continue;
                    };
                    let (r2, a2) = polar_of(&g, geo.lat_deg, geo.lon_deg, h);
                    let da = (a2 - az + 180.0).rem_euclid(360.0) - 180.0;
                    // Position error: range difference plus the cross-range from azimuth.
                    let cross = da.to_radians() * geo.ground_m;
                    worst = worst.max((r2 - rho).abs().max(cross.abs()));
                    n += 1;
                }
            }
        }
        assert!(n > 300, "{n}");
        assert!(worst < 1.0e-3, "worst inverse error {worst} m over {n} points");
    }

    // REQ: OA-TRK-030
    #[test]
    fn conversion_matches_the_simulators_forward_model() {
        // Independent code: oa-sensorsim's frame and polar_of, not this module's.
        let f = oa_sensorsim::geom::frame_at(SITE.lat_deg, SITE.lon_deg, SITE.height_m);
        let g = radar_geom(&SITE);
        let mut worst: f64 = 0.0;
        for &(lat, lon, h) in &[
            (42.5, -87.0, 10_000.0),
            (41.0, -89.0, 3_000.0),
            (43.2, -88.5, 11_500.0),
            (41.9, -87.8, 600.0),
            (40.0, -86.0, 9_000.0),
        ] {
            let (rho, az) = oa_sensorsim::geom::polar_of(&f, lat, lon, h);
            let geo = polar_to_geodetic(&g, rho, az, h).unwrap();
            let (dn, de) = (
                (geo.lat_deg - lat).to_radians() * 6.37e6,
                (geo.lon_deg - lon).to_radians() * 6.37e6 * lat.to_radians().cos(),
            );
            worst = worst.max(dn.hypot(de));
        }
        assert!(worst < 1.0e-3, "{worst}");
    }

    // REQ: OA-TRK-031
    #[test]
    fn covariance_matches_a_monte_carlo_of_the_conversion() {
        let g = radar_geom(&SITE);
        let mut rng = oa_sensorsim::prng::Rng::from_seed(7);
        for &(rho, az, h) in &[
            (20_000.0, 30.0, 3_000.0),
            (100_000.0, 200.0, 10_000.0),
            (200_000.0, 95.0, 11_000.0),
        ] {
            let base = polar_to_geodetic(&g, rho, az, h).unwrap();
            let (sr, saz, sh) = (70.0, 0.08_f64.to_radians(), 8.8);
            let (mut see, mut sen, mut snn) = (0.0, 0.0, 0.0);
            let n = 4000;
            for _ in 0..n {
                let (nr, na, nh) = (rng.normal(), rng.normal(), rng.normal());
                let p = polar_to_geodetic(
                    &g,
                    rho + sr * nr,
                    (az + (saz * na).to_degrees()).rem_euclid(360.0),
                    h + sh * nh,
                )
                .unwrap();
                let e = (p.lon_deg - base.lon_deg).to_radians()
                    * 6.378e6
                    * base.lat_deg.to_radians().cos();
                let nn = (p.lat_deg - base.lat_deg).to_radians() * 6.36e6;
                see += e * e;
                sen += e * nn;
                snn += nn * nn;
            }
            let cov = polar_cov_local(
                &g,
                rho,
                az,
                base.elev_rad,
                base.ground_m,
                sr * sr,
                saz * saz,
                sh * sh,
            );
            let nf = f64::from(n);
            let (mee, men, mnn) = (see / nf, sen / nf, snn / nf);
            // Monte Carlo of n = 4000 has about 2 % standard error per variance.
            let rel = |a: f64, b: f64| (a - b).abs() / b.abs().max(1.0);
            assert!(rel(mee, cov[0]) < 0.10, "ee {mee} vs {}", cov[0]);
            assert!(rel(mnn, cov[2]) < 0.10, "nn {mnn} vs {}", cov[2]);
            let scale = (cov[0] * cov[2]).sqrt();
            assert!((men - cov[1]).abs() < 0.10 * scale, "en {men} vs {}", cov[1]);
        }
    }

    // REQ: OA-TRK-032
    #[test]
    fn height_comes_from_mode_c_then_the_supplied_track_height_else_refused() {
        let g = radar_geom(&SITE);
        let mut p = RadarPlot {
            time_ns: 1_000_000_000,
            range_m: 60_000.0,
            az_deg: 100.0,
            alt_ft: 30_000,
            addr: NO_ADDR,
            label: NO_ADDR,
            mode_a: 0o1200,
        };
        let c = plot_convert(&g, &SITE, &p, &NO_BIAS, None, 5).unwrap();
        assert_eq!(c.m.alt_ft, 30_000);
        assert_eq!(c.m.alt_quant_ft, 100);
        assert_eq!(c.m.ident, 0o1200);
        assert_eq!(c.m.addr, 5);
        p.alt_ft = NONE_ALT;
        assert_eq!(
            plot_convert(&g, &SITE, &p, &NO_BIAS, None, 5),
            Err(RadarError::NoHeight)
        );
        let c2 = plot_convert(&g, &SITE, &p, &NO_BIAS, Some((9_144.0, 20.0)), 5).unwrap();
        assert_eq!(c2.m.alt_ft, NONE_I32);
        assert_eq!(c2.m.alt_quant_ft, 0);
        // Same height, so the same position as the Mode C plot (30,000 ft = 9,144 m).
        assert_eq!(c.m.lat_e7, c2.m.lat_e7);
        assert_eq!(c.m.lon_e7, c2.m.lon_e7);
        // The supplied height's uncertainty is in the covariance: larger than with Mode C.
        assert!(c2.m.pos_cov_m2[0] + c2.m.pos_cov_m2[2] > c.m.pos_cov_m2[0] + c.m.pos_cov_m2[2]);
    }

    // REQ: OA-TRK-033
    #[test]
    fn bias_is_subtracted_and_its_variance_added() {
        let g = radar_geom(&SITE);
        let p = RadarPlot {
            time_ns: 0,
            range_m: 80_000.0,
            az_deg: 45.0,
            alt_ft: 20_000,
            addr: NO_ADDR,
            label: NO_ADDR,
            mode_a: NONE_CODE,
        };
        let plain = plot_convert(&g, &SITE, &p, &NO_BIAS, None, 5).unwrap();
        let b = Bias {
            range_m: 100.0,
            az_deg: 0.1,
            var_range_m2: 30.0 * 30.0,
            var_az_deg2: 0.02 * 0.02,
        };
        let corrected = plot_convert(&g, &SITE, &p, &b, None, 5).unwrap();
        // Corrected position equals the plain conversion of the corrected polar values.
        let p2 = RadarPlot {
            range_m: 79_900.0,
            az_deg: 44.9,
            ..p
        };
        let direct = plot_convert(&g, &SITE, &p2, &NO_BIAS, None, 5).unwrap();
        assert_eq!(corrected.m.lat_e7, direct.m.lat_e7);
        assert_eq!(corrected.m.lon_e7, direct.m.lon_e7);
        assert_ne!(corrected.m.lat_e7, plain.m.lat_e7);
        let tr = |m: &Measurement| f64::from(m.pos_cov_m2[0]) + f64::from(m.pos_cov_m2[2]);
        assert!(tr(&corrected.m) > tr(&direct.m));
    }

    // REQ: OA-TRK-034
    #[test]
    fn invalid_sites_and_plots_are_refused() {
        let g = radar_geom(&SITE);
        let bad_sites = [
            RadarSite {
                lat_deg: 95.0,
                ..SITE
            },
            RadarSite {
                sigma_range_m: 0.0,
                ..SITE
            },
            RadarSite {
                period_ns: 0,
                ..SITE
            },
            RadarSite {
                alt_quantum_ft: 0,
                ..SITE
            },
            RadarSite {
                sigma_az_deg: f64::NAN,
                ..SITE
            },
        ];
        for s in &bad_sites {
            assert!(site_check(s).is_err(), "{s:?}");
        }
        assert!(site_check(&SITE).is_ok());
        let ok = RadarPlot {
            time_ns: 0,
            range_m: 50_000.0,
            az_deg: 10.0,
            alt_ft: 10_000,
            addr: NO_ADDR,
            label: NO_ADDR,
            mode_a: NONE_CODE,
        };
        for bad in [
            RadarPlot {
                range_m: f64::NAN,
                ..ok
            },
            RadarPlot {
                range_m: 100.0,
                ..ok
            },
            RadarPlot {
                range_m: 700_000.0,
                ..ok
            },
            RadarPlot {
                az_deg: -1.0,
                ..ok
            },
            RadarPlot {
                az_deg: 361.0,
                ..ok
            },
            RadarPlot {
                alt_ft: 200_000,
                ..ok
            },
        ] {
            assert!(
                matches!(
                    plot_convert(&g, &SITE, &bad, &NO_BIAS, None, 5),
                    Err(RadarError::BadPlot(_))
                ),
                "{bad:?}"
            );
        }
    }
}
