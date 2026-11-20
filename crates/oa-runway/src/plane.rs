//! The airport's local east-north plane. Core (pure).
//!
//! The plane is a tangent plane about the airport reference point with the ellipsoid's exact
//! local scales: `east = N cos(lat0) dlon (1 - tan(lat0) dlat)` and
//! `north = M dlat + tan(lat0) east^2 / (2 R)`, with the meridian radius `M`, the prime-vertical
//! radius `N` and the mean radius `R = sqrt(M N)` at the origin (WGS-84, from oa-geo). The
//! `(1 - tan(lat0) dlat)` factor is the first-order change of `cos(lat)` along the meridian, and
//! the `east^2` term is the poleward curvature of the parallel: a straight (geodesic) line due
//! east from the origin drifts toward the equator by that much, so without it the plane's
//! distances are wrong by tens of metres at 19 km and 64 degrees. Together they bring the error
//! to the third order in distance. The
//! conversion itself is a few multiplications and no library call, so a Kani proof can cover it
//! ([`crate::occupy`]); the scales are found once, in [`plane_new`], with `oa_math`.
//!
//! Extent and error: the plane is defined for points within [`EXTENT_M`] of the origin (a disc),
//! and [`to_local`] refuses (returns `None` for) any point past it. Within the extent and
//! for an origin up to [`MAX_ORIGIN_LAT_DEG`], the distance from the origin the plane gives
//! differs from the WGS-84 geodesic distance by at most [`PLANE_ERROR_BOUND_M`]; the test
//! `plane_error_against_geodesic_is_within_the_stated_bound` checks that over a grid of origins
//! and bearings and fails if the bound is exceeded.

use oa_geo::wgs84;

/// The radius of the disc the plane covers, metres. Our own choice: it holds a runway, its
/// approach and departure areas ([`crate::runway`]) and the airfield around them, and keeps
/// `dlat` under 0.2 degrees so the second-order error stays small.
pub const EXTENT_M: f64 = 20_000.0;

/// The highest origin latitude (north or south) the plane is built for, degrees. Our own choice:
/// at 80 degrees `tan(lat0)` is 5.7 and the east factor is still far from zero over the extent.
pub const MAX_ORIGIN_LAT_DEG: f64 = 80.0;

/// The largest difference, metres, between the plane's distance from the origin and the WGS-84
/// geodesic distance, anywhere in the extent, for an origin within [`MAX_ORIGIN_LAT_DEG`].
/// Measured by the test named in the module doc (worst case 1.05 m, at 80 degrees south and 20 km), then
/// rounded up; it is a stated bound, not a
/// derivation. A runway is at least 30 m wide, so this is small beside what the test decides.
pub const PLANE_ERROR_BOUND_M: f64 = 1.5;

/// A point on the earth, degrees. Plain data.
#[derive(Debug, Clone, Copy, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct Geo {
    pub lat_deg: f64,
    pub lon_deg: f64,
}

/// A position in an airport's plane, metres east and north of its origin. Plain data. Any value
/// is allowed: a non-finite one is an unknown position and counts as occupying.
#[derive(Debug, Clone, Copy, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct Local {
    pub east_m: f64,
    pub north_m: f64,
}

/// The plane about one airport reference point. Fields are private: [`plane_new`] is the only way
/// to make one, so its scales are always the ellipsoid's.
#[derive(Debug, Clone, Copy, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct Plane {
    lat0_deg: f64,
    lon0_deg: f64,
    /// Metres of north per degree of latitude (`M` times pi/180).
    m_per_deg_lat: f64,
    /// Metres of east per degree of longitude at the origin (`N cos(lat0)` times pi/180).
    m_per_deg_lon: f64,
    /// `tan(lat0)`.
    tan_lat0: f64,
    /// `tan(lat0) / (2 R)`, per metre: the curvature term of `north`.
    half_curv: f64,
}

/// A plane about `(lat0_deg, lon0_deg)`. `None` for a non-finite origin, a longitude outside
/// [-180, 180] or a latitude beyond [`MAX_ORIGIN_LAT_DEG`].
pub fn plane_new(lat0_deg: f64, lon0_deg: f64) -> Option<Plane> {
    if !(lat0_deg.is_finite() && lon0_deg.is_finite()) {
        return None;
    }
    if !(-MAX_ORIGIN_LAT_DEG..=MAX_ORIGIN_LAT_DEG).contains(&lat0_deg) {
        return None;
    }
    if !(-180.0..=180.0).contains(&lon0_deg) {
        return None;
    }
    let e = wgs84();
    let a = e.equatorial_radius();
    let f = e.flattening();
    let e2 = f * (2.0 - f);
    let (s, c) = oa_math::sin_cos(lat0_deg.to_radians());
    let w = oa_ieee::sqrt(1.0 - e2 * s * s);
    let n = a / w;
    let m = a * (1.0 - e2) / (w * w * w);
    let deg = core::f64::consts::PI / 180.0;
    let r = oa_ieee::sqrt(m * n);
    Some(Plane {
        lat0_deg,
        lon0_deg,
        m_per_deg_lat: m * deg,
        m_per_deg_lon: n * c * deg,
        tan_lat0: s / c,
        half_curv: s / c / (2.0 * r),
    })
}

/// The plane's origin, degrees.
pub fn plane_origin(p: &Plane) -> Geo {
    Geo {
        lat_deg: p.lat0_deg,
        lon_deg: p.lon0_deg,
    }
}

/// Degrees to the plane. `None` for a non-finite input or a point whose plane distance from the
/// origin, `sqrt(east^2 + north^2)`, exceeds [`EXTENT_M`] (a disc, so the stated error bound,
/// which is a distance from the origin, covers every point accepted). A longitude difference past 180 degrees is taken the short way
/// round.
pub fn to_local(p: &Plane, g: Geo) -> Option<Local> {
    if !(g.lat_deg.is_finite() && g.lon_deg.is_finite()) {
        return None;
    }
    let dlat = g.lat_deg - p.lat0_deg;
    let north0 = p.m_per_deg_lat * dlat;
    if !(-EXTENT_M..=EXTENT_M).contains(&north0) {
        return None;
    }
    let mut dlon = g.lon_deg - p.lon0_deg;
    if dlon > 180.0 {
        dlon -= 360.0;
    } else if dlon < -180.0 {
        dlon += 360.0;
    }
    let dlat_rad = dlat * (core::f64::consts::PI / 180.0);
    let east = p.m_per_deg_lon * dlon * (1.0 - p.tan_lat0 * dlat_rad);
    if !(-EXTENT_M..=EXTENT_M).contains(&east) {
        return None;
    }
    let north = north0 + p.half_curv * east * east;
    // The disc, compared so that a NaN fails it (the sum is finite here: each term is within
    // the per-axis guards above, and the guards are only there to bound the arithmetic).
    if !(-EXTENT_M..=EXTENT_M).contains(&north)
        || !(0.0..=EXTENT_M * EXTENT_M).contains(&(east * east + north * north))
    {
        return None;
    }
    Some(Local {
        east_m: east,
        north_m: north,
    })
}

/// A plane with the given scales, for the proofs only: the proof states which scales a
/// [`plane_new`] result can have and checks the conversion for every plane with them.
#[cfg(kani)]
pub(crate) fn plane_from_parts(
    lat0_deg: f64,
    lon0_deg: f64,
    m_per_deg_lat: f64,
    m_per_deg_lon: f64,
    tan_lat0: f64,
    half_curv: f64,
) -> Plane {
    Plane {
        lat0_deg,
        lon0_deg,
        m_per_deg_lat,
        m_per_deg_lon,
        tan_lat0,
        half_curv,
    }
}
