//! WGS-84 to screen projection (**core**, B1, A9).
//!
//! The oblique stereographic projection of the ellipsoid (the EPSG 9809 method, Gauss conformal
//! sphere), centred on a chosen point, like a radar scope: the centre is true, and scale grows
//! slowly outward. The ellipsoid is first mapped conformally onto a sphere (of radius
//! `sqrt(M * N)`, the Gaussian radius at the centre) and that sphere is projected stereographically.
//! Both steps are conformal, so a shape is not sheared: bearings are true and the scale is the same
//! in every direction at a point. The range is set like a scope's: the distance from the centre to
//! the nearest screen edge, so the circle of that radius is always whole. A pixel is 1/16 pixel
//! fixed point on the screen.
//!
//! # Error
//!
//! The scale is exactly 1 at the centre and grows as `1 + c^2 / 4` at angular distance `c` radians
//! (0.04 % at 250 NM, 0.16 % at 500 NM): the radial distance drawn is `2 R tan(c / 2)` for a true
//! `R c`, which is stretched `c^2 / 12`. On top of that the conformal sphere is not the ellipsoid
//! beyond the centre, a further error of the order of the square of the eccentricity times `c`
//! squared. The test `projection_distance_error_is_bounded` measures the distance from the centre
//! drawn on screen against the WGS-84 geodesic of oa-geo at five centres from the equator to
//! 65 N, out to 400 NM, and bounds it; the bound is printed there and is a small fraction of a
//! pixel at any real screen scale. Bearings are checked to a fraction of a degree. Fixed-point
//! rounding adds at most half of 1/16 pixel. This is a map projection for presenting positions,
//! not a measuring instrument: ranges and separations are always computed in oa-geo.
//!
//! The trigonometry uses `f64` inside the calculation only (A9); results are quantised to 1/16
//! pixel before they leave. The platform's libm may differ in the last bit of a sine, which can
//! move a point by one 1/16 pixel on rare inputs; the rasterizer, which is where bit-for-bit
//! reproducibility is claimed, is integer only and takes the display list as given.

use crate::list::DisplayError;
use oa_geo::wgs84;

/// Metres in a nautical mile.
pub const NM_M: f64 = 1852.0;
/// Fixed-point scale: units per pixel.
pub const SUBPIXEL: i32 = 16;
/// Largest projected coordinate magnitude, 1/16 pixel. Beyond it a point is saturated, which only
/// keeps later integer arithmetic inside `i64` and cannot be seen: it is 2^26 pixels.
pub const COORD_LIMIT: i32 = 1 << 30;
/// Range limits for the configured scale, nautical miles from the centre to the nearest edge.
pub const MIN_RANGE_NM: f64 = 0.05;
pub const MAX_RANGE_NM: f64 = 20_000.0;
/// Below this the point is within a hair of the antipode and cannot be projected.
const MIN_DENOMINATOR: f64 = 1e-6;

/// A configured projection. Precomputed from its configuration by [`projection_new`].
// A7.2: cold struct, size guard waived (one per display).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Projection {
    pub centre_lat_e7: i32,
    pub centre_lon_e7: i32,
    pub range_nm: f64,
    pub width: u32,
    pub height: u32,
    /// First eccentricity of WGS-84.
    pub ecc: f64,
    /// Gauss sphere constants: longitude scale `n`, latitude constant `c`.
    pub n: f64,
    pub c: f64,
    pub sin_chi0: f64,
    pub cos_chi0: f64,
    pub lon0_rad: f64,
    /// Radius of the conformal sphere, metres.
    pub radius_m: f64,
    /// 1/16 pixels per metre at the centre.
    pub scale: f64,
    /// Screen position of the centre, 1/16 pixel.
    pub cx16: f64,
    pub cy16: f64,
}

/// A projection centred on the given point, with `range_nm` nautical miles from the centre to the
/// nearest edge of a `width` by `height` pixel screen (half the shorter side).
pub fn projection_new(
    centre_lat_e7: i32,
    centre_lon_e7: i32,
    range_nm: f64,
    width: u32,
    height: u32,
) -> Result<Projection, DisplayError> {
    if !(-900_000_000..=900_000_000).contains(&centre_lat_e7)
        || !(-1_800_000_000..=1_800_000_000).contains(&centre_lon_e7)
    {
        return Err(DisplayError::BadView(
            "centre is not a position on the globe",
        ));
    }
    if !range_nm.is_finite() || !(MIN_RANGE_NM..=MAX_RANGE_NM).contains(&range_nm) {
        return Err(DisplayError::BadView("range is outside 0.05 to 20000 NM"));
    }
    if width == 0 || height == 0 || width > 1 << 16 || height > 1 << 16 {
        return Err(DisplayError::BadView("screen size is outside 1 to 65536"));
    }
    let lat0 = f64::from(centre_lat_e7) * 1e-7_f64.to_radians();
    let lon0 = f64::from(centre_lon_e7) * 1e-7_f64.to_radians();
    let ell = wgs84();
    let a = ell.equatorial_radius();
    let f = ell.flattening();
    let e2 = f * (2.0 - f);
    let ecc = e2.sqrt();
    let sin0 = lat0.sin();
    let cos0 = lat0.cos();
    let w = 1.0 - e2 * sin0 * sin0;
    // Meridian and prime-vertical radii of curvature; the sphere gets their geometric mean.
    let m = a * (1.0 - e2) / (w * w.sqrt());
    let nu = a / w.sqrt();
    let radius_m = (m * nu).sqrt();
    let n = (1.0 + e2 * cos0.powi(4) / (1.0 - e2)).sqrt();
    let w1 = iso_log(ecc, n, sin0).exp();
    let sin_chi_start = (w1 - 1.0) / (w1 + 1.0);
    let c = (n + sin0) * (1.0 - sin_chi_start) / ((n - sin0) * (1.0 + sin_chi_start));
    let (sin_chi0, cos_chi0) = chi_sin_cos(c, w1);
    let half_short = f64::from(width.min(height)) / 2.0;
    let scale = f64::from(SUBPIXEL) * half_short / (range_nm * NM_M);
    Ok(Projection {
        centre_lat_e7,
        centre_lon_e7,
        range_nm,
        width,
        height,
        ecc,
        n,
        c,
        sin_chi0,
        cos_chi0,
        lon0_rad: lon0,
        radius_m,
        scale,
        cx16: f64::from(width) * f64::from(SUBPIXEL) / 2.0,
        cy16: f64::from(height) * f64::from(SUBPIXEL) / 2.0,
    })
}

/// `ln(w / c)` for latitude with sine `sin_lat`: `n` times the log of `tan^2(pi/4 + phi/2) *
/// ((1 - e sin phi) / (1 + e sin phi))^e`, which is `2 n (atanh(sin) - e atanh(e sin))`. Infinite at
/// the north pole, minus infinity at the south pole.
fn iso_log(ecc: f64, n: f64, sin_lat: f64) -> f64 {
    2.0 * n * (sin_lat.atanh() - ecc * (ecc * sin_lat).atanh())
}

/// Sine and cosine of the conformal latitude from `w = c * exp(iso_log)`: with
/// `w = tan^2(pi/4 + chi/2)`, `sin chi = (w - 1) / (w + 1)` and `cos chi = 2 sqrt(w) / (w + 1)`.
fn chi_sin_cos(c: f64, w_over_c: f64) -> (f64, f64) {
    let w = c * w_over_c;
    if w.is_infinite() {
        return (1.0, 0.0);
    }
    if w <= 0.0 {
        return (-1.0, 0.0);
    }
    ((w - 1.0) / (w + 1.0), 2.0 * w.sqrt() / (w + 1.0))
}

fn saturate(v: f64) -> i32 {
    let lim = f64::from(COORD_LIMIT);
    // Finite by the callers' checks; a NaN would saturate to 0 and is excluded before this.
    v.round().clamp(-lim, lim) as i32
}

/// Screen position, 1/16 pixel, of a point given in radians. `None` if not finite or at the
/// antipode of the centre.
pub fn project_rad(p: &Projection, lat: f64, lon: f64) -> Option<(i32, i32)> {
    let (sc, cc) = chi_sin_cos(p.c, iso_log(p.ecc, p.n, lat.sin()).exp());
    let tau = std::f64::consts::TAU;
    let d =
        p.n * ((lon - p.lon0_rad + std::f64::consts::PI).rem_euclid(tau) - std::f64::consts::PI);
    let cd = d.cos();
    let denom = 1.0 + sc * p.sin_chi0 + cc * p.cos_chi0 * cd;
    if !denom.is_finite() || denom < MIN_DENOMINATOR {
        return None;
    }
    let k = 2.0 * p.radius_m / denom;
    let x = k * cc * d.sin();
    let y = k * (sc * p.cos_chi0 - cc * p.sin_chi0 * cd);
    let sx = p.cx16 + x * p.scale;
    let sy = p.cy16 - y * p.scale;
    if !sx.is_finite() || !sy.is_finite() {
        return None;
    }
    Some((saturate(sx), saturate(sy)))
}

/// Screen position, 1/16 pixel, of a model position in 1e-7 degree. `None` for the absent-position
/// sentinel, a coordinate outside the globe, or the antipode of the centre.
pub fn project(p: &Projection, lat_e7: i32, lon_e7: i32) -> Option<(i32, i32)> {
    if !(-900_000_000..=900_000_000).contains(&lat_e7)
        || !(-1_800_000_000..=1_800_000_000).contains(&lon_e7)
    {
        return None;
    }
    let deg = 1e-7_f64.to_radians();
    project_rad(p, f64::from(lat_e7) * deg, f64::from(lon_e7) * deg)
}

/// The model position, 1e-7 degree, under a screen position (1/16 pixel). `None` if it does not
/// land on the globe in range. The inverse of [`project`] up to quantisation.
pub fn unproject(p: &Projection, x16: i32, y16: i32) -> Option<(i32, i32)> {
    let x = (f64::from(x16) - p.cx16) / p.scale;
    let y = (p.cy16 - f64::from(y16)) / p.scale;
    let rho = x.hypot(y);
    let ang = 2.0 * (rho / (2.0 * p.radius_m)).atan();
    let (sa, ca) = (ang.sin(), ang.cos());
    let (chi, d) = if rho == 0.0 {
        (p.sin_chi0.asin(), 0.0)
    } else {
        let chi = (ca * p.sin_chi0 + y * sa * p.cos_chi0 / rho)
            .clamp(-1.0, 1.0)
            .asin();
        let d = (x * sa).atan2(rho * p.cos_chi0 * ca - y * p.sin_chi0 * sa);
        (chi, d)
    };
    let lon = p.lon0_rad + d / p.n;
    // Isometric latitude from the conformal latitude, then Newton's method for the geodetic one.
    let psi = 0.5 * ((1.0 + chi.sin()) / (p.c * (1.0 - chi.sin()))).ln() / p.n;
    let e2 = p.ecc * p.ecc;
    let mut lat = chi;
    for _ in 0..12 {
        let s = lat.sin();
        let psi_i = (std::f64::consts::FRAC_PI_4 + lat / 2.0).tan().ln()
            + p.ecc / 2.0 * ((1.0 - p.ecc * s) / (1.0 + p.ecc * s)).ln();
        lat -= (psi_i - psi) * lat.cos() * (1.0 - e2 * s * s) / (1.0 - e2);
    }
    let e7 = 1.0 / 1e-7_f64.to_radians();
    let mut lon_e7 = (lon * e7).round();
    // Wrap into [-180, 180] degrees.
    let full = 3_600_000_000.0;
    while lon_e7 > 1_800_000_000.0 {
        lon_e7 -= full;
    }
    while lon_e7 < -1_800_000_000.0 {
        lon_e7 += full;
    }
    let lat_e7 = (lat * e7).round();
    if !lat_e7.is_finite() || !lon_e7.is_finite() {
        return None;
    }
    Some((lat_e7 as i32, lon_e7 as i32))
}

/// Screen radius, 1/16 pixel, of the circle of true radius `metres` about the centre. `None` when
/// the radius is half the globe or more.
pub fn ring_radius16(p: &Projection, metres: f64) -> Option<i32> {
    let half = metres / (2.0 * p.radius_m);
    if !half.is_finite() || !(0.0..1.5).contains(&half) {
        return None;
    }
    Some(saturate(2.0 * p.radius_m * half.tan() * p.scale))
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
    use oa_geo::LatLon;

    const ORD: (i32, i32) = (419_786_000, -879_048_000);

    fn proj(range: f64) -> Projection {
        projection_new(ORD.0, ORD.1, range, 1920, 1080).unwrap()
    }

    /// Deterministic generator for sweeps.
    fn xorshift(state: &mut u64) -> u64 {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        *state
    }

    // REQ: OA-DSP-004
    #[test]
    fn the_centre_is_the_middle_of_the_screen() {
        let p = proj(250.0);
        assert_eq!(project(&p, ORD.0, ORD.1), Some((960 * 16, 540 * 16)));
        // North is up, east is right.
        let north = project(&p, ORD.0 + 10_000_000, ORD.1).unwrap();
        let east = project(&p, ORD.0, ORD.1 + 10_000_000).unwrap();
        assert!(north.1 < 540 * 16 && (north.0 - 960 * 16).abs() <= 1);
        assert!(east.0 > 960 * 16 && (east.1 - 540 * 16).abs() < 16 * 20);
    }

    // REQ: OA-DSP-004
    #[test]
    fn the_range_reaches_the_nearest_edge() {
        // 250 NM due north is 540 pixels up (half of the shorter side), to the stereographic scale.
        let p = proj(250.0);
        let ell = oa_geo::wgs84();
        let c = LatLon::new(41.9786, -87.9048).unwrap();
        let n = ell.direct(c, 0.0, 250.0 * NM_M).unwrap().position;
        let (lat, lon) = (
            (n.lat() * 1e7).round() as i32,
            (n.lon() * 1e7).round() as i32,
        );
        let (_, y) = project(&p, lat, lon).unwrap();
        let up = f64::from(540 * 16 - y) / 16.0;
        assert!((up - 540.0).abs() < 1.5, "250 NM north landed {up} px up");
    }

    // REQ: OA-DSP-004
    #[test]
    fn projection_round_trips_within_one_sixteenth_pixel() {
        let p = proj(250.0);
        let mut s = 0x9E37_79B9_7F4A_7C15_u64;
        for _ in 0..20_000 {
            let x = (xorshift(&mut s) % (1920 * 16)) as i32;
            let y = (xorshift(&mut s) % (1080 * 16)) as i32;
            let (lat, lon) = unproject(&p, x, y).unwrap();
            let (x2, y2) = project(&p, lat, lon).unwrap();
            // 1e-7 degree is 1.1 cm; 1/16 pixel here is 29 m, so the position grid is finer.
            assert!(
                (x2 - x).abs() <= 1 && (y2 - y).abs() <= 1,
                "({x}, {y}) came back as ({x2}, {y2})"
            );
        }
        // And the other way: a position survives to within half a screen unit (1/16 pixel is 53 m
        // here, so 27 m along each axis, 38 m on the diagonal) plus the 1e-7 degree grid.
        for _ in 0..20_000 {
            let lat = ORD.0 + (xorshift(&mut s) % 40_000_000) as i32 - 20_000_000;
            let lon = ORD.1 + (xorshift(&mut s) % 60_000_000) as i32 - 30_000_000;
            let (x, y) = project(&p, lat, lon).unwrap();
            let (lat2, lon2) = unproject(&p, x, y).unwrap();
            let a = LatLon::new(f64::from(lat) * 1e-7, f64::from(lon) * 1e-7).unwrap();
            let b = LatLon::new(f64::from(lat2) * 1e-7, f64::from(lon2) * 1e-7).unwrap();
            let d = oa_geo::wgs84().inverse(a, b).unwrap().distance_m;
            assert!(d < 38.0, "round trip moved a point {d} m");
        }
    }

    // REQ: OA-DSP-005
    #[test]
    fn projection_distance_error_is_bounded() {
        // Distance from the centre drawn on screen against the WGS-84 geodesic, over rings of
        // bearing, at several centres and ranges within 400 NM.
        let ell = oa_geo::wgs84();
        let mut worst = 0.0_f64;
        for (clat, clon) in [
            (41.9786, -87.9048),
            (25.8, -80.3),
            (47.6, -122.3),
            (64.8, -147.7),
            (0.0, 0.0),
        ] {
            let c = LatLon::new(clat, clon).unwrap();
            let (clat7, clon7) = ((clat * 1e7).round() as i32, (clon * 1e7).round() as i32);
            let p = projection_new(clat7, clon7, 400.0, 1920, 1080).unwrap();
            let scale_px_per_m = p.scale / 16.0;
            for nm in [1.0, 10.0, 50.0, 100.0, 200.0, 300.0, 400.0] {
                for bearing in (0..360).step_by(5) {
                    let q = ell
                        .direct(c, f64::from(bearing), nm * NM_M)
                        .unwrap()
                        .position;
                    let (x, y) = project(
                        &p,
                        (q.lat() * 1e7).round() as i32,
                        (q.lon() * 1e7).round() as i32,
                    )
                    .unwrap();
                    let dx = (f64::from(x) - p.cx16) / 16.0;
                    let dy = (f64::from(y) - p.cy16) / 16.0;
                    let drawn_m = dx.hypot(dy) / scale_px_per_m;
                    let true_m = ell.inverse(c, q).unwrap().distance_m;
                    // Distances below a few pixels are dominated by the 1/16 pixel grid.
                    let slack_m = 0.5 / 16.0 / scale_px_per_m;
                    let err = ((drawn_m - true_m).abs() - slack_m).max(0.0) / true_m;
                    worst = worst.max(err);
                }
            }
        }
        assert!(
            worst < 0.0015,
            "worst distance error {:.4} %",
            worst * 100.0
        );
        eprintln!("worst projected distance error: {:.4} %", worst * 100.0);
    }

    // REQ: OA-DSP-005
    #[test]
    fn bearings_are_true_to_a_fraction_of_a_degree() {
        let ell = oa_geo::wgs84();
        let c = LatLon::new(41.9786, -87.9048).unwrap();
        let p = proj(300.0);
        let mut worst = 0.0_f64;
        for nm in [50.0, 150.0, 250.0] {
            for bearing in (0..360).step_by(10) {
                let q = ell
                    .direct(c, f64::from(bearing), nm * NM_M)
                    .unwrap()
                    .position;
                let (x, y) = project(
                    &p,
                    (q.lat() * 1e7).round() as i32,
                    (q.lon() * 1e7).round() as i32,
                )
                .unwrap();
                let (dx, dy) = (f64::from(x) - p.cx16, p.cy16 - f64::from(y));
                let drawn = dx.atan2(dy).to_degrees().rem_euclid(360.0);
                let mut d = (drawn - f64::from(bearing)).abs();
                if d > 180.0 {
                    d = 360.0 - d;
                }
                worst = worst.max(d);
            }
        }
        assert!(worst < 0.3, "worst bearing error {worst} degrees");
    }

    // REQ: OA-DSP-006
    #[test]
    fn unusable_inputs_are_refused_not_drawn() {
        let p = proj(250.0);
        // Absent position, off the globe, and the point opposite the centre.
        assert_eq!(project(&p, i32::MIN, i32::MIN), None);
        assert_eq!(project(&p, 900_000_001, 0), None);
        assert_eq!(project(&p, 0, 1_800_000_001), None);
        // The antipode of the centre is far off the screen or refused, never drawn near it.
        if let Some((x, y)) = project(&p, -ORD.0, ORD.1 + 1_800_000_000) {
            assert!(x.abs() > 16 * 100_000 || y.abs() > 16 * 100_000);
        }
        assert!(projection_new(ORD.0, ORD.1, 0.0, 1920, 1080).is_err());
        assert!(projection_new(ORD.0, ORD.1, f64::NAN, 1920, 1080).is_err());
        assert!(projection_new(ORD.0, ORD.1, 1e9, 1920, 1080).is_err());
        assert!(projection_new(ORD.0, ORD.1, 100.0, 0, 1080).is_err());
        assert!(projection_new(i32::MIN, 0, 100.0, 1920, 1080).is_err());
        // Far points saturate instead of overflowing.
        let tiny = projection_new(ORD.0, ORD.1, 0.05, 1920, 1080).unwrap();
        let (x, y) = project(&tiny, ORD.0 + 400_000_000, ORD.1 + 500_000_000).unwrap();
        assert!(x.abs() <= COORD_LIMIT && y.abs() <= COORD_LIMIT);
    }

    // REQ: OA-DSP-006
    #[test]
    fn ring_radius_is_the_true_distance() {
        let p = proj(250.0);
        // At the centre scale is true: 50 NM of ring is 50 NM of range, to within the stretch.
        let r = f64::from(ring_radius16(&p, 50.0 * NM_M).unwrap()) / 16.0;
        let nominal = 50.0 * 540.0 / 250.0;
        assert!(
            (r - nominal).abs() / nominal < 0.002,
            "{r} against {nominal}"
        );
        assert_eq!(ring_radius16(&p, -1.0), None);
        assert_eq!(ring_radius16(&p, 1e12), None);
    }
}
