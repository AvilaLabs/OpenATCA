//! The local east-north frame: a spherical azimuthal equidistant projection. Core (pure).
//!
//! Moved here from oa-truth so the tracker (oa-track) and the truth builder share one frame.
//!
//! oa-geo has the geodesic inverse and direct problems but no local east-north frame, and a
//! geodesic call per report would cost far more than this needs, so the frame is a spherical
//! azimuthal equidistant projection about an origin: `x` east, `y` north, in
//! metres, the radius being the Gaussian mean radius of curvature at the origin. Forward and
//! inverse are closed form and exact inverses of each other, so the positions mapped back out
//! carry no projection error; the projection only has to be smooth for the smoother's motion
//! model to hold, and its scale error (about 0.3 % from the sphere, under 0.1 % across 400 km
//! tangentially) is far below the noise. Velocity is converted back to true east and north
//! components by mapping a one-second displacement out and measuring it on the ellipsoid.

const WGS84_A: f64 = 6_378_137.0;
const WGS84_E2: f64 = 0.006_694_379_990_141_316;

/// Metres of one radian of latitude and of longitude at `lat_rad` on WGS-84.
fn radii(lat_rad: f64) -> (f64, f64) {
    let s = lat_rad.sin();
    let w = (1.0 - WGS84_E2 * s * s).sqrt();
    let n = WGS84_A / w;
    let m = WGS84_A * (1.0 - WGS84_E2) / (w * w * w);
    (m, n * lat_rad.cos())
}

/// A projection about an origin. Plain data (A1, private fields: A1.1); A7.2: cold struct, size
/// guard waived.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    lat0: f64,
    lon0: f64,
    sin0: f64,
    cos0: f64,
    r: f64,
}

/// A frame about `(lat_deg, lon_deg)`.
pub fn frame_new(lat_deg: f64, lon_deg: f64) -> Frame {
    let lat0 = lat_deg.to_radians();
    let s = lat0.sin();
    let w2 = 1.0 - WGS84_E2 * s * s;
    // sqrt(M N) = a sqrt(1 - e2) / w2.
    let r = WGS84_A * (1.0 - WGS84_E2).sqrt() / w2;
    Frame {
        lat0,
        lon0: lon_deg.to_radians(),
        sin0: s,
        cos0: lat0.cos(),
        r,
    }
}

/// The angle at the sphere's centre between the origin and a point, radians.
pub fn frame_angle(f: &Frame, lat_deg: f64, lon_deg: f64) -> f64 {
    let (s_e, s_n, cosc) = parts(f, lat_deg.to_radians(), lon_deg.to_radians());
    (s_e * s_e + s_n * s_n).sqrt().atan2(cosc)
}

fn parts(f: &Frame, lat: f64, lon: f64) -> (f64, f64, f64) {
    let dl = lon - f.lon0;
    let (sl, cl) = (lat.sin(), lat.cos());
    let (sd, cd) = (dl.sin(), dl.cos());
    let s_e = cl * sd;
    let s_n = f.cos0 * sl - f.sin0 * cl * cd;
    let cosc = f.sin0 * sl + f.cos0 * cl * cd;
    (s_e, s_n, cosc)
}

/// Degrees to `(east_m, north_m)`. The origin maps to zero. The origin's antipode, where every
/// direction is the same distance away, maps to half the sphere's circumference due east: far
/// from the origin, as it is, and not onto it (a point there would pass every distance check).
pub fn project(f: &Frame, lat_deg: f64, lon_deg: f64) -> (f64, f64) {
    let (s_e, s_n, cosc) = parts(f, lat_deg.to_radians(), lon_deg.to_radians());
    let sinc = (s_e * s_e + s_n * s_n).sqrt();
    if sinc < 1e-12 && cosc < 0.0 {
        return (f.r * std::f64::consts::PI, 0.0);
    }
    let k = if sinc < 1e-12 {
        1.0
    } else {
        sinc.atan2(cosc) / sinc
    };
    (f.r * k * s_e, f.r * k * s_n)
}

/// `(east_m, north_m)` to `(lat_deg, lon_deg)`, longitude in [-180, 180].
pub fn unproject(f: &Frame, x: f64, y: f64) -> (f64, f64) {
    let rho = (x * x + y * y).sqrt();
    if rho < 1e-9 {
        return (f.lat0.to_degrees(), wrap_deg(f.lon0.to_degrees()));
    }
    let c = rho / f.r;
    let (sc, cc) = (c.sin(), c.cos());
    let lat = (cc * f.sin0 + y * sc * f.cos0 / rho)
        .clamp(-1.0, 1.0)
        .asin();
    let lon = f.lon0 + (x * sc).atan2(rho * f.cos0 * cc - y * f.sin0 * sc);
    (lat.to_degrees(), wrap_deg(lon.to_degrees()))
}

fn wrap_deg(d: f64) -> f64 {
    let m = (d + 180.0).rem_euclid(360.0) - 180.0;
    if m == -180.0 && d > 0.0 {
        180.0
    } else {
        m
    }
}

/// True `(east, north)` velocity in m/s at frame position `(x, y)` for a frame velocity
/// `(vx, vy)`, by mapping a one-second displacement out and measuring it on the ellipsoid.
pub fn frame_velocity(f: &Frame, x: f64, y: f64, vx: f64, vy: f64) -> (f64, f64) {
    let (la0, lo0) = unproject(f, x, y);
    let (la1, lo1) = unproject(f, x + vx, y + vy);
    let mut dlon = lo1 - lo0;
    if dlon > 180.0 {
        dlon -= 360.0;
    } else if dlon < -180.0 {
        dlon += 360.0;
    }
    let mid = ((la0 + la1) / 2.0).to_radians();
    let (m, e) = radii(mid);
    (dlon.to_radians() * e, (la1 - la0).to_radians() * m)
}

/// Frame velocity `(vx, vy)` in frame metres per second for a true `(east, north)` velocity in
/// metres per second at the frame origin: the inverse of [`frame_velocity`] there. The frame
/// metre is the sphere's, the true metre the ellipsoid's, so the two differ by up to 0.3 %
/// (east and north differently); a filter that works in the frame converts its velocity
/// measurements with this and its outputs back with [`frame_velocity`].
pub fn frame_from_true_velocity(f: &Frame, east_mps: f64, north_mps: f64) -> (f64, f64) {
    let w2 = 1.0 - WGS84_E2 * f.sin0 * f.sin0;
    let n = WGS84_A / w2.sqrt();
    let m = WGS84_A * (1.0 - WGS84_E2) / (w2 * w2.sqrt());
    (east_mps * f.r / n, north_mps * f.r / m)
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
    fn project_and_unproject_are_inverses() {
        let f = frame_new(41.9, -88.3);
        for (la, lo) in [(41.9, -88.3), (42.4, -87.1), (40.2, -90.0)] {
            let (x, y) = project(&f, la, lo);
            let (la2, lo2) = unproject(&f, x, y);
            assert!((la - la2).abs() < 1e-9 && (lo - lo2).abs() < 1e-9);
        }
    }

    // REQ: OA-TRK-083
    #[test]
    fn the_antipode_of_the_origin_is_far_from_it_not_on_it() {
        let f = frame_new(0.0, 0.0);
        let half = std::f64::consts::PI * f.r;
        for (la, lo) in [(0.0, 180.0), (0.0, -180.0)] {
            let (x, y) = project(&f, la, lo);
            assert!((x.hypot(y) - half).abs() < 1.0, "{la} {lo}: {x} {y}");
        }
        // Near it the distance is the right one too, and the origin itself is still zero.
        let (x, y) = project(&f, 0.0, 179.9);
        assert!(
            (x.hypot(y) - half).abs() < 0.2 * f.r.to_radians(),
            "{x} {y}"
        );
        assert_eq!(project(&f, 0.0, 0.0), (0.0, 0.0));
        // The same away from the equator.
        let g = frame_new(41.9, -88.3);
        let (x, y) = project(&g, -41.9, 91.7);
        assert!(
            (x.hypot(y) - std::f64::consts::PI * g.r).abs() < 1.0,
            "{x} {y}"
        );
    }

    #[test]
    fn true_velocity_round_trips_at_the_origin() {
        let f = frame_new(41.9, -88.3);
        let (vx, vy) = frame_from_true_velocity(&f, 130.0, -80.0);
        let (e, n) = frame_velocity(&f, 0.0, 0.0, vx, vy);
        assert!(
            (e - 130.0).abs() < 0.01 && (n + 80.0).abs() < 0.01,
            "{e} {n}"
        );
    }
}
