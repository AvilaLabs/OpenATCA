//! Geodesic helpers for error vectors and distances. Core (pure). Distances come from the
//! WGS-84 inverse problem of `oa-geo`, so they hold near the poles and over long baselines;
//! `enu` is the local east-north frame at a point (geodesic azimuth and distance from it), and
//! `enu` and `from_enu` are inverses of each other, which the synthetic self-test relies on.

use std::sync::OnceLock;

use oa_geo::{wgs84, Ellipsoid, LatLon};

/// Metres per nautical mile.
pub const NM_M: f64 = 1852.0;
/// Metres per second per knot.
pub const KT_MPS: f64 = 1852.0 / 3600.0;
/// Metres per foot.
pub const FT_M: f64 = 0.3048;

/// Wraps degrees into (-180, 180].
pub fn wrap180(deg: f64) -> f64 {
    let mut d = deg % 360.0;
    if d > 180.0 {
        d -= 360.0;
    } else if d <= -180.0 {
        d += 360.0;
    }
    d
}

/// The WGS-84 ellipsoid with its series tables, built once.
fn ellipsoid() -> &'static Ellipsoid {
    static E: OnceLock<Ellipsoid> = OnceLock::new();
    E.get_or_init(wgs84)
}

/// East and north metres of `(lat, lon)` in the local frame at `(lat0, lon0)`: the WGS-84
/// geodesic distance and azimuth from the origin, split into east and north (an azimuthal
/// equidistant frame, so the length of `(e, n)` is the geodesic distance). `(NaN, NaN)` when
/// either position is not a valid latitude and longitude.
pub fn enu(lat0: f64, lon0: f64, lat: f64, lon: f64) -> (f64, f64) {
    let (Ok(a), Ok(b)) = (LatLon::new(lat0, lon0), LatLon::new(lat, lon)) else {
        return (f64::NAN, f64::NAN);
    };
    match ellipsoid().inverse(a, b) {
        Ok(r) => {
            let az = r.azimuth1_deg.to_radians();
            (r.distance_m * az.sin(), r.distance_m * az.cos())
        }
        Err(_) => (f64::NAN, f64::NAN),
    }
}

/// The inverse of [`enu`]: the position at east `e` and north `n` metres of `(lat0, lon0)`.
/// `(NaN, NaN)` when the origin or an offset is not valid.
pub fn from_enu(lat0: f64, lon0: f64, e: f64, n: f64) -> (f64, f64) {
    let Ok(a) = LatLon::new(lat0, lon0) else {
        return (f64::NAN, f64::NAN);
    };
    match ellipsoid().direct(a, e.atan2(n).to_degrees(), e.hypot(n)) {
        Ok(d) => (d.position.lat(), d.position.lon()),
        Err(_) => (f64::NAN, f64::NAN),
    }
}

/// Geodesic distance in metres on WGS-84; NaN when a position is not a valid latitude and
/// longitude.
pub fn dist_m(lat_a: f64, lon_a: f64, lat_b: f64, lon_b: f64) -> f64 {
    let (Ok(a), Ok(b)) = (LatLon::new(lat_a, lon_a), LatLon::new(lat_b, lon_b)) else {
        return f64::NAN;
    };
    ellipsoid().inverse(a, b).map_or(f64::NAN, |r| r.distance_m)
}

/// Heading in degrees clockwise from north of the vector (east, north), 0 to 360.
pub fn heading_deg(east: f64, north: f64) -> f64 {
    let h = east.atan2(north).to_degrees();
    if h < 0.0 {
        h + 360.0
    } else {
        h
    }
}
