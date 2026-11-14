//! Radar-centred geometry and the line-of-sight test. Core (pure).
//!
//! Slant range and azimuth come from the WGS-84 ellipsoid (geodetic to earth-centred, then
//! rotated into the radar's east-north-up frame). The line-of-sight test uses a sphere of
//! effective radius `k` times the mean earth radius (the standard-refraction "4/3 earth", with
//! `k` an assumption parameter of the site, see [`crate::radar::RadarSite`]): the beam reaches
//! the target if the central angle between the two is no more than the sum of each end's
//! horizon angle. No terrain is modelled.

const WGS84_A: f64 = 6_378_137.0;
const WGS84_E2: f64 = 0.006_694_379_990_141_316;
/// Mean earth radius, metres (IUGG), the sphere the effective-radius model is scaled from.
pub const EARTH_MEAN_RADIUS_M: f64 = 6_371_000.0;
/// Metres per foot (exact).
pub const FT_M: f64 = 0.3048;
/// Metres per nautical mile (exact).
pub const NM_M: f64 = 1852.0;

/// Geodetic position to earth-centred earth-fixed metres (WGS-84).
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

/// A radar's frame: its ECEF position and the east, north and up unit vectors.
#[derive(Debug, Clone, Copy, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct Frame {
    pub origin: [f64; 3],
    east: [f64; 3],
    north: [f64; 3],
}

/// The frame at a geodetic position (east and north axes; up is not needed).
pub fn frame_at(lat_deg: f64, lon_deg: f64, h_m: f64) -> Frame {
    let (lat, lon) = (lat_deg.to_radians(), lon_deg.to_radians());
    let (sl, cl, so, co) = (lat.sin(), lat.cos(), lon.sin(), lon.cos());
    Frame {
        origin: ecef(lat_deg, lon_deg, h_m),
        east: [-so, co, 0.0],
        north: [-sl * co, -sl * so, cl],
    }
}

fn dot(a: &[f64; 3], b: &[f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Slant range (m) and azimuth (degrees clockwise from true north, in `[0, 360)`) of a target
/// seen from a radar.
pub fn polar_of(f: &Frame, lat_deg: f64, lon_deg: f64, h_m: f64) -> (f64, f64) {
    let p = ecef(lat_deg, lon_deg, h_m);
    let d = [p[0] - f.origin[0], p[1] - f.origin[1], p[2] - f.origin[2]];
    let (e, n) = (dot(&d, &f.east), dot(&d, &f.north));
    let range = dot(&d, &d).sqrt();
    (range, wrap360(e.atan2(n).to_degrees()))
}

/// Wraps degrees into `[0, 360)`.
pub fn wrap360(deg: f64) -> f64 {
    let r = deg.rem_euclid(360.0);
    if r >= 360.0 {
        0.0
    } else {
        r
    }
}

/// The signed difference `a - b` of two azimuths, wrapped into `[-180, 180)`.
pub fn az_diff(a: f64, b: f64) -> f64 {
    (a - b + 180.0).rem_euclid(360.0) - 180.0
}

/// The central angle in radians between two geodetic positions (haversine on the sphere).
pub fn central_angle(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
    let dl = (lon2 - lon1).to_radians();
    let a = ((p2 - p1) / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dl / 2.0).sin().powi(2);
    2.0 * a.sqrt().min(1.0).asin()
}

/// True if the beam reaches a target at height `h2_m` from an antenna at height `h1_m`, the
/// central angle between their ground points being `theta_rad`, on an earth of effective
/// radius `k * EARTH_MEAN_RADIUS_M`. Heights below zero count as zero.
pub fn line_of_sight(h1_m: f64, h2_m: f64, theta_rad: f64, k: f64) -> bool {
    let re = k * EARTH_MEAN_RADIUS_M;
    let horizon = |h: f64| (re / (re + h.max(0.0))).acos();
    // Ground distance is the same on both spheres: R theta = Re theta_eff, so theta_eff =
    // theta / k.
    theta_rad / k <= horizon(h1_m) + horizon(h2_m)
}

/// Offsets a position by `east_m` and `north_m` on a sphere of the mean earth radius (the flat
/// small-offset approximation, used for the sub-millisecond truth advance and the spoof
/// transforms, where an exact geodesic is not needed). Returns degrees.
pub fn offset_deg(lat_deg: f64, lon_deg: f64, east_m: f64, north_m: f64) -> (f64, f64) {
    let lat = lat_deg + (north_m / EARTH_MEAN_RADIUS_M).to_degrees();
    let c = lat_deg.to_radians().cos().max(1e-9);
    (
        lat,
        lon_deg + (east_m / (EARTH_MEAN_RADIUS_M * c)).to_degrees(),
    )
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

    #[test]
    fn polar_matches_simple_cases() {
        let f = frame_at(41.0, -88.0, 0.0);
        // Due north by 0.1 degree of latitude: about 11.1 km at azimuth 0.
        let (r, az) = polar_of(&f, 41.1, -88.0, 0.0);
        assert!((r - 11_100.0).abs() < 30.0, "{r}");
        assert!(az < 1e-6 || az > 360.0 - 1e-6, "{az}");
        let (_, az) = polar_of(&f, 41.0, -87.9, 0.0);
        assert!((az - 90.0).abs() < 0.1, "{az}");
        // Straight up 1000 m: range 1000.
        let (r, _) = polar_of(&f, 41.0, -88.0, 1000.0);
        assert!((r - 1000.0).abs() < 1e-6);
    }

    // REQ: OA-SIM-006
    #[test]
    fn line_of_sight_matches_the_horizon_formula() {
        // Radio horizon for 4/3 earth: d (km) = 4.12 (sqrt(h1 m) + sqrt(h2 m)).
        let (h1, h2) = (30.0_f64, 10_000.0_f64);
        let d_km = 4.12 * (h1.sqrt() + h2.sqrt());
        let theta = |km: f64| km * 1000.0 / EARTH_MEAN_RADIUS_M;
        assert!(line_of_sight(h1, h2, theta(d_km * 0.98), 4.0 / 3.0));
        assert!(!line_of_sight(h1, h2, theta(d_km * 1.02), 4.0 / 3.0));
        // With k = 1 the horizon is shorter.
        assert!(!line_of_sight(h1, h2, theta(d_km * 0.98), 1.0));
    }

    #[test]
    fn az_diff_wraps() {
        assert!((az_diff(359.0, 1.0) + 2.0).abs() < 1e-12);
        assert!((az_diff(1.0, 359.0) - 2.0).abs() < 1e-12);
    }
}
