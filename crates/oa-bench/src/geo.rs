//! Flat-earth helpers for error vectors. Core (pure). Errors between an ERAM position and the
//! truth position are at most a few kilometres, so a local east-north tangent plane at the truth
//! point is exact to better than 1e-4 of the error; `enu` and `from_enu` are exact inverses of
//! each other, which the synthetic self-test relies on.

/// Mean earth radius, metres.
pub const EARTH_R_M: f64 = 6_371_008.8;
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

/// East and north metres of `(lat, lon)` in the tangent plane at `(lat0, lon0)`.
pub fn enu(lat0: f64, lon0: f64, lat: f64, lon: f64) -> (f64, f64) {
    let e = EARTH_R_M * lat0.to_radians().cos() * wrap180(lon - lon0).to_radians();
    let n = EARTH_R_M * (lat - lat0).to_radians();
    (e, n)
}

/// The inverse of [`enu`].
pub fn from_enu(lat0: f64, lon0: f64, e: f64, n: f64) -> (f64, f64) {
    let lat = lat0 + (n / EARTH_R_M).to_degrees();
    let c = lat0.to_radians().cos();
    let lon = if c.abs() < 1e-9 {
        lon0
    } else {
        lon0 + (e / (EARTH_R_M * c)).to_degrees()
    };
    (lat, lon)
}

/// Distance in metres between two points that are not far apart (tangent plane at the first).
pub fn dist_m(lat_a: f64, lon_a: f64, lat_b: f64, lon_b: f64) -> f64 {
    let (e, n) = enu(lat_a, lon_a, lat_b, lon_b);
    e.hypot(n)
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
