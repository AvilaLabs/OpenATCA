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

use crate::core::list::DisplayError;
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
    /// Rows at the top of the screen kept out of the map (the legend strip).
    pub top_inset: u32,
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
/// nearest edge of the map area: the `width` by `height` pixel screen less its top `top_inset`
/// rows, so half the shorter side of that area, centred in it.
pub fn projection_new(
    centre_lat_e7: i32,
    centre_lon_e7: i32,
    range_nm: f64,
    width: u32,
    height: u32,
    top_inset: u32,
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
    if top_inset >= height || height - top_inset < 16 {
        return Err(DisplayError::BadView("the legend strip leaves no map area"));
    }
    let map_h = height - top_inset;
    let lat0 = f64::from(centre_lat_e7) * 1e-7_f64.to_radians();
    let lon0 = f64::from(centre_lon_e7) * 1e-7_f64.to_radians();
    let ell = wgs84();
    let a = ell.equatorial_radius();
    let f = ell.flattening();
    let e2 = f * (2.0 - f);
    let ecc = oa_ieee::sqrt(e2);
    let sin0 = oa_math::sin(lat0);
    let cos0 = oa_math::cos(lat0);
    let w = 1.0 - e2 * sin0 * sin0;
    // Meridian and prime-vertical radii of curvature; the sphere gets their geometric mean.
    let m = a * (1.0 - e2) / (w * oa_ieee::sqrt(w));
    let nu = a / oa_ieee::sqrt(w);
    let radius_m = oa_ieee::sqrt(m * nu);
    let n = oa_ieee::sqrt(1.0 + e2 * oa_math::powi(cos0, 4) / (1.0 - e2));
    let w1 = oa_math::exp(iso_log(ecc, n, sin0));
    let sin_chi_start = (w1 - 1.0) / (w1 + 1.0);
    let c = (n + sin0) * (1.0 - sin_chi_start) / ((n - sin0) * (1.0 + sin_chi_start));
    let (sin_chi0, cos_chi0) = chi_sin_cos(c, w1);
    // At a pole the isometric latitude is infinite and these come out NaN; such a view is
    // unusable, so it is refused here rather than accepted with non-finite constants.
    if ![radius_m, n, c, sin_chi0, cos_chi0]
        .iter()
        .all(|v| v.is_finite())
    {
        return Err(DisplayError::BadView(
            "centre has no finite projection (a pole)",
        ));
    }
    let half_short = f64::from(width.min(map_h)) / 2.0;
    let scale = f64::from(SUBPIXEL) * half_short / (range_nm * NM_M);
    Ok(Projection {
        centre_lat_e7,
        centre_lon_e7,
        range_nm,
        width,
        height,
        top_inset,
        ecc,
        n,
        c,
        sin_chi0,
        cos_chi0,
        lon0_rad: lon0,
        radius_m,
        scale,
        cx16: f64::from(width) * f64::from(SUBPIXEL) / 2.0,
        cy16: (f64::from(top_inset) + f64::from(map_h) / 2.0) * f64::from(SUBPIXEL),
    })
}

/// `ln(w / c)` for latitude with sine `sin_lat`: `n` times the log of `tan^2(pi/4 + phi/2) *
/// ((1 - e sin phi) / (1 + e sin phi))^e`, which is `2 n (atanh(sin) - e atanh(e sin))`. Infinite at
/// the north pole, minus infinity at the south pole.
fn iso_log(ecc: f64, n: f64, sin_lat: f64) -> f64 {
    2.0 * n * (atanh(sin_lat) - ecc * atanh(ecc * sin_lat))
}

/// `ln(1 + y)` accurate for small `y`, from `oa_math::ln` by the standard correction.
fn ln_1p(y: f64) -> f64 {
    let u = 1.0 + y;
    if u == 1.0 {
        y
    } else {
        oa_math::ln(u) * y / (u - 1.0)
    }
}

/// The inverse hyperbolic tangent, `0.5 ln((1 + x) / (1 - x))` written as `0.5 ln(1 + 2x / (1 - x))`
/// so that it keeps its accuracy near 0. Local because core may not call the platform's `atanh`
/// (B8: its result differs between machines) and `oa-math` has none. Measured against std's on
/// 10^6 inputs over `|x| <= sin 85 deg`: absolute error at most 9e-16 rad; the plain
/// `ln((1 + x) / (1 - x))` form reached 2.8e-14, which is why it is not used. Plus and minus 1
/// give the infinities, as std's does.
fn atanh(x: f64) -> f64 {
    if x == 1.0 {
        f64::INFINITY
    } else if x == -1.0 {
        f64::NEG_INFINITY
    } else {
        0.5 * ln_1p(2.0 * x / (1.0 - x))
    }
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
    ((w - 1.0) / (w + 1.0), 2.0 * oa_ieee::sqrt(w) / (w + 1.0))
}

fn saturate(v: f64) -> i32 {
    let lim = f64::from(COORD_LIMIT);
    // Finite by the callers' checks; a NaN would saturate to 0 and is excluded before this.
    oa_ieee::round(v).clamp(-lim, lim) as i32
}

/// Screen position, 1/16 pixel, of a point given in radians. `None` if not finite or at the
/// antipode of the centre.
pub fn project_rad(p: &Projection, lat: f64, lon: f64) -> Option<(i32, i32)> {
    let (sc, cc) = chi_sin_cos(p.c, oa_math::exp(iso_log(p.ecc, p.n, oa_math::sin(lat))));
    let tau = std::f64::consts::TAU;
    let d = p.n
        * (oa_math::rem_euclid(lon - p.lon0_rad + std::f64::consts::PI, tau)
            - std::f64::consts::PI);
    let cd = oa_math::cos(d);
    let denom = 1.0 + sc * p.sin_chi0 + cc * p.cos_chi0 * cd;
    if !denom.is_finite() || denom < MIN_DENOMINATOR {
        return None;
    }
    let k = 2.0 * p.radius_m / denom;
    let x = k * cc * oa_math::sin(d);
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
    let rho = oa_math::hypot(x, y);
    let ang = 2.0 * oa_math::atan(rho / (2.0 * p.radius_m));
    let (sa, ca) = (oa_math::sin(ang), oa_math::cos(ang));
    let (chi, d) = if rho == 0.0 {
        (oa_math::asin(p.sin_chi0), 0.0)
    } else {
        let chi = oa_math::asin((ca * p.sin_chi0 + y * sa * p.cos_chi0 / rho).clamp(-1.0, 1.0));
        let d = oa_math::atan2(x * sa, rho * p.cos_chi0 * ca - y * p.sin_chi0 * sa);
        (chi, d)
    };
    let lon = p.lon0_rad + d / p.n;
    // Isometric latitude from the conformal latitude, then Newton's method for the geodetic one.
    let psi =
        0.5 * oa_math::ln((1.0 + oa_math::sin(chi)) / (p.c * (1.0 - oa_math::sin(chi)))) / p.n;
    let e2 = p.ecc * p.ecc;
    let mut lat = chi;
    for _ in 0..12 {
        let s = oa_math::sin(lat);
        let psi_i = oa_math::ln(oa_math::tan(std::f64::consts::FRAC_PI_4 + lat / 2.0))
            + p.ecc / 2.0 * oa_math::ln((1.0 - p.ecc * s) / (1.0 + p.ecc * s));
        lat -= (psi_i - psi) * oa_math::cos(lat) * (1.0 - e2 * s * s) / (1.0 - e2);
    }
    let e7 = 1.0 / 1e-7_f64.to_radians();
    let mut lon_e7 = oa_ieee::round(lon * e7);
    // Wrap into [-180, 180] degrees.
    let full = 3_600_000_000.0;
    while lon_e7 > 1_800_000_000.0 {
        lon_e7 -= full;
    }
    while lon_e7 < -1_800_000_000.0 {
        lon_e7 += full;
    }
    let lat_e7 = oa_ieee::round(lat * e7);
    if !lat_e7.is_finite() || !lon_e7.is_finite() {
        return None;
    }
    Some((lat_e7 as i32, lon_e7 as i32))
}

/// A latitude and longitude box around the visible map, for rejecting a position before it is
/// projected. Longitude is held relative to the centre so the box needs no antimeridian case.
// A7.2: cold struct, size guard waived (one per view).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bbox {
    pub lat_min: i32,
    pub lat_max: i32,
    pub centre_lon: i32,
    /// Longitude offset from the centre, 1e-7 degree, in -1.8e9 to 1.8e9.
    pub dlon_min: i64,
    pub dlon_max: i64,
}

/// Longitude offset from `centre`, wrapped into -180 to 180 degrees, 1e-7 degree.
pub fn dlon_e7(lon: i32, centre: i32) -> i64 {
    i64::rem_euclid(
        i64::from(lon) - i64::from(centre) + 1_800_000_000,
        3_600_000_000,
    ) - 1_800_000_000
}

/// The box around the map area grown by `margin_px`, found by unprojecting its border (a position
/// is on the globe in the area only if it is between the border's extremes, as latitude and
/// longitude have no maximum inside the area when it holds no pole), then padded by 5 % and
/// 0.01 degree. `None` when the area reaches within 5 degrees of a pole or spans more than 180
/// degrees of longitude: then nothing is culled.
pub fn view_bbox(p: &Projection, margin_px: i32) -> Option<Bbox> {
    const STEPS: i32 = 32;
    let m = margin_px.max(0) * SUBPIXEL;
    let (x0, x1) = (
        -m,
        i32::try_from(i64::from(p.width) * 16)
            .ok()?
            .checked_add(m)?,
    );
    let top = i32::try_from(i64::from(p.top_inset) * 16).ok()? - m;
    let y1 = i32::try_from(i64::from(p.height) * 16)
        .ok()?
        .checked_add(m)?;
    let (mut lat_lo, mut lat_hi) = (i32::MAX, i32::MIN);
    let (mut dl_lo, mut dl_hi) = (i64::MAX, i64::MIN);
    for k in 0..=STEPS {
        let fx = x0 + ((i64::from(x1) - i64::from(x0)) * i64::from(k) / i64::from(STEPS)) as i32;
        let fy = top + ((i64::from(y1) - i64::from(top)) * i64::from(k) / i64::from(STEPS)) as i32;
        for (x, y) in [(fx, top), (fx, y1), (x0, fy), (x1, fy)] {
            let (lat, lon) = unproject(p, x, y)?;
            lat_lo = lat_lo.min(lat);
            lat_hi = lat_hi.max(lat);
            let d = dlon_e7(lon, p.centre_lon_e7);
            dl_lo = dl_lo.min(d);
            dl_hi = dl_hi.max(d);
        }
    }
    if lat_lo < -850_000_000 || lat_hi > 850_000_000 || dl_lo < -900_000_000 || dl_hi > 900_000_000
    {
        return None;
    }
    let pad_lat = i64::from(lat_hi - lat_lo) / 20 + 100_000;
    let pad_lon = (dl_hi - dl_lo) / 20 + 100_000;
    Some(Bbox {
        lat_min: i32::try_from(i64::from(lat_lo) - pad_lat).ok()?,
        lat_max: i32::try_from(i64::from(lat_hi) + pad_lat).ok()?,
        centre_lon: p.centre_lon_e7,
        dlon_min: dl_lo - pad_lon,
        dlon_max: dl_hi + pad_lon,
    })
}

/// False when the position is certainly outside the box.
pub fn in_bbox(b: &Bbox, lat: i32, lon: i32) -> bool {
    let d = dlon_e7(lon, b.centre_lon);
    lat >= b.lat_min && lat <= b.lat_max && d >= b.dlon_min && d <= b.dlon_max
}

/// Screen radius, 1/16 pixel, of the circle of true radius `metres` about the centre. `None` when
/// the radius is half the globe or more.
pub fn ring_radius16(p: &Projection, metres: f64) -> Option<i32> {
    let half = metres / (2.0 * p.radius_m);
    if !half.is_finite() || !(0.0..1.5).contains(&half) {
        return None;
    }
    Some(saturate(2.0 * p.radius_m * oa_math::tan(half) * p.scale))
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
        projection_new(ORD.0, ORD.1, range, 1920, 1080, 0).unwrap()
    }

    // REQ: OA-DSP-033
    #[test]
    fn a_pole_view_is_refused_and_the_nearest_ones_are_usable() {
        for lat in [900_000_000, -900_000_000] {
            for lon in [0, 1_800_000_000, -1_800_000_000] {
                for range in [0.05, 60.0, 20_000.0] {
                    assert!(
                        matches!(
                            projection_new(lat, lon, range, 1920, 1080, 0),
                            Err(DisplayError::BadView(_))
                        ),
                        "{lat},{lon},{range}"
                    );
                }
            }
        }
        // Next to a pole the sine of the latitude rounds to 1, so the nearest centres are as
        // unusable as the pole itself: each is either refused or fully usable, never in between.
        // Well clear of that, 0.001 degree off the pole, the view works.
        let mut worked = 0;
        for mag in [
            899_999_999,
            899_999_000,
            899_990_000,
            899_900_000,
            899_000_000,
        ] {
            for lat in [mag, -mag] {
                match projection_new(lat, 0, 60.0, 1920, 1080, 0) {
                    Err(e) => assert!(matches!(e, DisplayError::BadView(_)), "{lat}"),
                    Ok(p) => {
                        worked += 1;
                        for v in [p.n, p.c, p.sin_chi0, p.cos_chi0, p.radius_m, p.scale] {
                            assert!(v.is_finite(), "{lat}");
                        }
                        let (x, y) = project(&p, lat, 0).unwrap();
                        assert!(
                            (x - p.cx16 as i32).abs() < 16 && (y - p.cy16 as i32).abs() < 16,
                            "{lat}: {x},{y}"
                        );
                        let off = if lat > 0 { lat - 10_000 } else { lat + 10_000 };
                        let (x, y) = project(&p, off, 100_000_000).unwrap();
                        assert!(
                            x > 0 && y > 0 && x < 1920 * 16 && y < 1080 * 16,
                            "{lat}: {x},{y}"
                        );
                    }
                }
            }
        }
        assert!(worked >= 2);
        let p = projection_new(899_000_000, 0, 60.0, 1920, 1080, 0).unwrap();
        let below = f64::from_bits(90f64.to_bits() - 1).to_radians();
        // The nearest f64 latitude below 90 degrees, through the radian entry, is finite or
        // absent, and never a saturated corner.
        if let Some((x, y)) = project_rad(&p, below, 0.0) {
            assert!(x > 0 && y < 1080 * 16, "{x},{y}");
        }
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
            let p = projection_new(clat7, clon7, 400.0, 1920, 1080, 0).unwrap();
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
        assert!(projection_new(ORD.0, ORD.1, 0.0, 1920, 1080, 0).is_err());
        assert!(projection_new(ORD.0, ORD.1, f64::NAN, 1920, 1080, 0).is_err());
        assert!(projection_new(ORD.0, ORD.1, 1e9, 1920, 1080, 0).is_err());
        assert!(projection_new(ORD.0, ORD.1, 100.0, 0, 1080, 0).is_err());
        assert!(projection_new(i32::MIN, 0, 100.0, 1920, 1080, 0).is_err());
        // Far points saturate instead of overflowing.
        let tiny = projection_new(ORD.0, ORD.1, 0.05, 1920, 1080, 0).unwrap();
        let (x, y) = project(&tiny, ORD.0 + 400_000_000, ORD.1 + 500_000_000).unwrap();
        assert!(x.abs() <= COORD_LIMIT && y.abs() <= COORD_LIMIT);
    }

    // REQ: OA-DSP-028
    #[test]
    fn the_map_area_excludes_the_top_strip_and_the_range_circle_fits_in_it() {
        let p = projection_new(ORD.0, ORD.1, 250.0, 1920, 1080, 104).unwrap();
        // Centred in the area below the strip, not in the whole screen.
        assert_eq!(
            project(&p, ORD.0, ORD.1),
            Some((960 * 16, (104 + 488) * 16))
        );
        // The range reaches the top of the map area, 488 pixels above the centre, and no higher.
        let ell = oa_geo::wgs84();
        let c = LatLon::new(41.9786, -87.9048).unwrap();
        let n = ell.direct(c, 0.0, 250.0 * NM_M).unwrap().position;
        let (_, y) = project(
            &p,
            (n.lat() * 1e7).round() as i32,
            (n.lon() * 1e7).round() as i32,
        )
        .unwrap();
        let top = f64::from(y) / 16.0;
        assert!((top - 104.0).abs() < 1.5, "250 NM north is at row {top}");
        // A strip that leaves no map is refused.
        assert!(projection_new(ORD.0, ORD.1, 250.0, 1920, 100, 104).is_err());
        assert!(projection_new(ORD.0, ORD.1, 250.0, 1920, 110, 104).is_err());
    }

    // REQ: OA-DSP-030
    #[test]
    fn the_bounding_box_holds_every_visible_position() {
        let mut s = 77_u64;
        for (lat, lon, range) in [
            (ORD.0, ORD.1, 250.0),
            (ORD.0, ORD.1, 3.0),
            (0, 1_790_000_000, 400.0),
            (-340_000_000, 1_790_000_000, 900.0),
            (600_000_000, -1_790_000_000, 50.0),
        ] {
            let p = projection_new(lat, lon, range, 1920, 1080, 104).unwrap();
            let b = view_bbox(&p, 64).expect("a box for a mid-latitude view");
            for _ in 0..4000 {
                // Any position on the screen below the strip, with the margin.
                let x = range_i(&mut s, -64 * 16, 1984 * 16);
                let y = range_i(&mut s, (104 - 64) * 16, 1144 * 16);
                if let Some((la, lo)) = unproject(&p, x, y) {
                    assert!(
                        in_bbox(&b, la, lo),
                        "({x},{y}) -> ({la},{lo}) is outside {b:?}"
                    );
                }
            }
            // A far position is outside.
            assert!(!in_bbox(&b, lat + 300_000_000, lon));
        }
        // Near a pole, or spanning half the globe, nothing is culled.
        assert!(view_bbox(
            &projection_new(890_000_000, 0, 100.0, 800, 600, 104).unwrap(),
            64
        )
        .is_none());
        assert!(view_bbox(&projection_new(0, 0, 9000.0, 800, 600, 104).unwrap(), 64).is_none());
        // The longitude offset wraps at the antimeridian.
        assert_eq!(dlon_e7(-1_790_000_000, 1_790_000_000), 20_000_000);
        assert_eq!(dlon_e7(1_790_000_000, -1_790_000_000), -20_000_000);
    }

    fn range_i(s: &mut u64, lo: i32, hi: i32) -> i32 {
        lo + (xorshift(s) % (i64::from(hi) - i64::from(lo) + 1) as u64) as i32
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
