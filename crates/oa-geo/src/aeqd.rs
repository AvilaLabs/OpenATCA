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
//! tangentially) is far below the noise. Velocities and covariances are converted to and from
//! true east and north by the exact local differential of the projection (the radial direction
//! from the origin keeps its scale, the tangential one is scaled by `sin c / c`, and the axes
//! are turned by the meridian convergence, which is large near a pole).

const WGS84_A: f64 = 6_378_137.0;
const WGS84_E2: f64 = 0.006_694_379_990_141_316;

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
    let s = oa_math::sin(lat0);
    let w2 = 1.0 - WGS84_E2 * s * s;
    // sqrt(M N) = a sqrt(1 - e2) / w2.
    let r = WGS84_A * (1.0 - WGS84_E2).sqrt() / w2;
    Frame {
        lat0,
        lon0: lon_deg.to_radians(),
        sin0: s,
        cos0: oa_math::cos(lat0),
        r,
    }
}

/// The angle at the sphere's centre between the origin and a point, radians.
pub fn frame_angle(f: &Frame, lat_deg: f64, lon_deg: f64) -> f64 {
    let (s_e, s_n, cosc) = parts(f, lat_deg.to_radians(), lon_deg.to_radians());
    oa_math::atan2((s_e * s_e + s_n * s_n).sqrt(), cosc)
}

fn parts(f: &Frame, lat: f64, lon: f64) -> (f64, f64, f64) {
    let dl = lon - f.lon0;
    let (sl, cl) = (oa_math::sin(lat), oa_math::cos(lat));
    let (sd, cd) = (oa_math::sin(dl), oa_math::cos(dl));
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
        oa_math::atan2(sinc, cosc) / sinc
    };
    (f.r * k * s_e, f.r * k * s_n)
}

/// `(east_m, north_m)` to `(lat_deg, lon_deg)`, longitude in [-180, 180]. `None` for a point whose
/// distance from the origin is not finite or is past half the sphere's circumference: the chart
/// does not reach there, and a larger distance would wrap round onto some other place.
pub fn unproject(f: &Frame, x: f64, y: f64) -> Option<(f64, f64)> {
    // Each component is bounded before squaring, so the sum cannot overflow; `sqrt` is exactly
    // rounded on every platform, unlike a library `hypot`.
    let half = f.r * std::f64::consts::PI * (1.0 + 1e-12);
    if !(x.abs() <= half && y.abs() <= half) {
        return None;
    }
    let rho = (x * x + y * y).sqrt();
    if rho > half {
        return None;
    }
    if rho < 1e-9 {
        return Some((f.lat0.to_degrees(), wrap_deg(f.lon0.to_degrees())));
    }
    let c = rho / f.r;
    let (sc, cc) = (oa_math::sin(c), oa_math::cos(c));
    let lat = oa_math::asin((cc * f.sin0 + y * sc * f.cos0 / rho).clamp(-1.0, 1.0));
    let lon = f.lon0 + oa_math::atan2(x * sc, rho * f.cos0 * cc - y * f.sin0 * sc);
    Some((lat.to_degrees(), wrap_deg(lon.to_degrees())))
}

fn wrap_deg(d: f64) -> f64 {
    let m = (d + 180.0).rem_euclid(360.0) - 180.0;
    if m == -180.0 && d > 0.0 {
        180.0
    } else {
        m
    }
}

/// A 2 x 2 matrix, rows first.
pub type Mat2 = [[f64; 2]; 2];

/// A vector or a basis row in three dimensions.
type V3 = [f64; 3];

fn dot(a: &V3, b: &V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: &V3, b: &V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// The frame origin's east, north and up unit vectors in earth-fixed axes.
fn basis(f: &Frame) -> [V3; 3] {
    let (sl, cl) = oa_math::sin_cos(f.lon0);
    [
        [-sl, cl, 0.0],
        [-f.sin0 * cl, -f.sin0 * sl, f.cos0],
        [f.cos0 * cl, f.cos0 * sl, f.sin0],
    ]
}

/// The projection's local geometry at plane position `(x, y)`, in the frame's own
/// (east, north, up at the origin) axes: the unit position vector of the point on the sphere and
/// the two sphere displacements, in radians per plane metre, of a unit step along plane x and
/// along plane y. The radial direction is preserved by the projection and the tangential one is
/// scaled by `sin c / c` (`c` the angle from the origin), so the displacements are exact.
fn local(f: &Frame, x: f64, y: f64) -> (V3, [V3; 2], f64) {
    let rho = oa_math::hypot(x, y);
    let c = rho / f.r;
    let (sa, ca) = if rho < 1e-9 {
        (0.0, 1.0)
    } else {
        (x / rho, y / rho)
    };
    let (sc, cc) = oa_math::sin_cos(c);
    // Floored: the factor vanishes only at the origin's antipode, which the projection
    // does not map to one place.
    let k = if c < 1e-8 { 1.0 } else { (sc / c).max(1e-6) };
    let u = [cc * sa, cc * ca, -sc];
    let t = [ca, -sa, 0.0];
    let col = |p: f64, q: f64| {
        [
            (p * u[0] + q * k * t[0]) / f.r,
            (p * u[1] + q * k * t[1]) / f.r,
            (p * u[2] + q * k * t[2]) / f.r,
        ]
    };
    ([sc * sa, sc * ca, cc], [col(sa, ca), col(ca, -sa)], k)
}

/// The point's unit east and north vectors and the WGS-84 radii `(N, M)` there, in the frame's
/// axes. At a pole east and north are undefined; the convention is the limit along the frame
/// origin's own meridian: east is the origin's east, north the direction on from the point
/// along that meridian (past the north pole, towards the origin's longitude plus 180 degrees).
fn enu(f: &Frame, p: &V3) -> (V3, V3, f64, f64) {
    let z = [0.0, f.cos0, f.sin0];
    let e = cross(&z, p);
    let cos_lat = dot(&e, &e).sqrt();
    let east = if cos_lat < 1e-9 {
        [1.0, 0.0, 0.0]
    } else {
        [e[0] / cos_lat, e[1] / cos_lat, e[2] / cos_lat]
    };
    let north = cross(p, &east);
    let s = dot(&z, p);
    let w2 = 1.0 - WGS84_E2 * s * s;
    let w = w2.sqrt();
    (
        east,
        north,
        WGS84_A / w,
        WGS84_A * (1.0 - WGS84_E2) / (w2 * w),
    )
}

/// The local differential of the projection at plane position `(x, y)`: the matrix taking a
/// small plane displacement (or velocity) to the true `(east, north)` displacement in metres on
/// the ellipsoid at that point. Exact; the east and north at a pole follow [`enu`]'s convention.
pub fn frame_to_true(f: &Frame, x: f64, y: f64) -> Mat2 {
    let (p, cols, _) = local(f, x, y);
    let (east, north, n, m) = enu(f, &p);
    let row = |v: &V3, radius: f64| [radius * dot(v, &cols[0]), radius * dot(v, &cols[1])];
    [row(&east, n), row(&north, m)]
}

/// The inverse of [`frame_to_true`]: true `(east, north)` metres at `(x, y)` to plane metres.
pub fn frame_from_true(f: &Frame, x: f64, y: f64) -> Mat2 {
    let [[a, b], [c, d]] = frame_to_true(f, x, y);
    let det = a * d - b * c;
    [[d / det, -b / det], [-c / det, a / det]]
}

/// The differential of the change of frame at one physical point: the matrix taking a small
/// displacement (or velocity) in the plane of `from` at `(x1, y1)` to the same displacement in
/// the plane of `to` at `(x2, y2)`, where `(x2, y2)` is the same point in `to`'s plane. Needs
/// no east or north, so it is the same at a pole.
pub fn frame_change(from: &Frame, x1: f64, y1: f64, to: &Frame, x2: f64, y2: f64) -> Mat2 {
    let (_, cols, _) = local(from, x1, y1);
    let (_, cols2, _) = local(to, x2, y2);
    // `cols2` are the images of the plane steps; the plane components of a displacement `w`
    // are the solution of the 2 x 2 system through them (they are not orthonormal).
    let (b1, b2) = (basis(from), basis(to));
    let g = |a: &V3, b: &V3| dot(a, b);
    let (g00, g01, g11) = (
        g(&cols2[0], &cols2[0]),
        g(&cols2[0], &cols2[1]),
        g(&cols2[1], &cols2[1]),
    );
    let det = g00 * g11 - g01 * g01;
    let column = |c: &V3| {
        // The displacement in earth-fixed axes, then in the other frame's axes.
        let w_ecef = [
            c[0] * b1[0][0] + c[1] * b1[1][0] + c[2] * b1[2][0],
            c[0] * b1[0][1] + c[1] * b1[1][1] + c[2] * b1[2][1],
            c[0] * b1[0][2] + c[1] * b1[1][2] + c[2] * b1[2][2],
        ];
        let w = [
            dot(&w_ecef, &b2[0]),
            dot(&w_ecef, &b2[1]),
            dot(&w_ecef, &b2[2]),
        ];
        let (h0, h1) = (dot(&cols2[0], &w), dot(&cols2[1], &w));
        ((g11 * h0 - g01 * h1) / det, (g00 * h1 - g01 * h0) / det)
    };
    let (a, c) = (column(&cols[0]), column(&cols[1]));
    [[a.0, c.0], [a.1, c.1]]
}

/// `m * v`.
pub fn mat2_apply(m: &Mat2, v: (f64, f64)) -> (f64, f64) {
    (m[0][0] * v.0 + m[0][1] * v.1, m[1][0] * v.0 + m[1][1] * v.1)
}

/// `m * [a b; b c] * m^T` for the symmetric `[a, b, c]`: a covariance carried through the
/// linear map `m`.
pub fn mat2_congruence(m: &Mat2, s: &[f64; 3]) -> [f64; 3] {
    let [a, b, c] = *s;
    let (p, q) = (m[0], m[1]);
    [
        p[0] * (p[0] * a + p[1] * b) + p[1] * (p[0] * b + p[1] * c),
        q[0] * (p[0] * a + p[1] * b) + q[1] * (p[0] * b + p[1] * c),
        q[0] * (q[0] * a + q[1] * b) + q[1] * (q[0] * b + q[1] * c),
    ]
}

/// True `(east, north)` velocity in m/s at frame position `(x, y)` for a frame velocity
/// `(vx, vy)`: the exact local differential of the projection ([`frame_to_true`]), not a
/// finite displacement, so it holds up to the poles.
pub fn frame_velocity(f: &Frame, x: f64, y: f64, vx: f64, vy: f64) -> (f64, f64) {
    mat2_apply(&frame_to_true(f, x, y), (vx, vy))
}

/// Frame velocity `(vx, vy)` in frame metres per second for a true `(east, north)` velocity in
/// metres per second at frame position `(x, y)`: the inverse of [`frame_velocity`] there. The
/// frame metre is the sphere's, the true metre the ellipsoid's, so the two differ by up to 0.3 %
/// (east and north differently), and away from the origin the axes are turned by the meridian
/// convergence; a filter that works in the frame converts its velocity measurements with this
/// and its outputs back with [`frame_velocity`].
pub fn frame_from_true_velocity_at(
    f: &Frame,
    x: f64,
    y: f64,
    east_mps: f64,
    north_mps: f64,
) -> (f64, f64) {
    mat2_apply(&frame_from_true(f, x, y), (east_mps, north_mps))
}

/// [`frame_from_true_velocity_at`] at the frame origin.
pub fn frame_from_true_velocity(f: &Frame, east_mps: f64, north_mps: f64) -> (f64, f64) {
    frame_from_true_velocity_at(f, 0.0, 0.0, east_mps, north_mps)
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
            let (la2, lo2) = unproject(&f, x, y).unwrap();
            assert!((la - la2).abs() < 1e-9 && (lo - lo2).abs() < 1e-9);
        }
    }

    // REQ: OA-GEO-013
    #[test]
    fn unproject_refuses_distances_that_overflow_or_pass_half_the_circumference() {
        let f = frame_new(41.9, -88.3);
        let half = std::f64::consts::PI * f.r;
        assert_eq!(unproject(&f, 1e200, 1e200), None);
        assert_eq!(unproject(&f, f64::MAX, f64::MAX), None);
        assert_eq!(unproject(&f, f64::NAN, 0.0), None);
        assert_eq!(unproject(&f, 0.0, f64::INFINITY), None);
        assert_eq!(unproject(&f, 20_037_509.0 * 2.0, 0.0), None);
        assert_eq!(unproject(&f, half * 1.001, 0.0), None);
        // The far edge of the chart, and a point reached by project, still convert.
        assert!(unproject(&f, half, 0.0).is_some());
        let (x, y) = project(&f, -41.9, 91.7);
        assert!(unproject(&f, x, y).is_some());
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

    /// WGS-84 prime-vertical and meridian radii at a latitude, degrees.
    fn radii_at(lat_deg: f64) -> (f64, f64) {
        let s = lat_deg.to_radians().sin();
        let w2 = 1.0 - WGS84_E2 * s * s;
        (
            WGS84_A / w2.sqrt(),
            WGS84_A * (1.0 - WGS84_E2) / (w2 * w2.sqrt()),
        )
    }

    // REQ: OA-GEO-008
    #[test]
    fn velocity_at_the_origin_is_the_analytic_differential_up_to_the_poles() {
        for lat in [45.0, 89.0, 89.99, 89.9999, -45.0, -89.0, -89.99, -89.9999] {
            let f = frame_new(lat, 20.0);
            let (n, m) = radii_at(lat);
            for (e, nn) in [(100.0, 0.0), (0.0, 100.0), (70.0, -30.0)] {
                let (vx, vy) = frame_from_true_velocity(&f, e, nn);
                assert!(
                    (vx - e * f.r / n).abs() < 1e-9 && (vy - nn * f.r / m).abs() < 1e-9,
                    "{lat}: {vx} {vy}"
                );
                let (e2, n2) = frame_velocity(&f, 0.0, 0.0, vx, vy);
                assert!(
                    (e2 - e).abs() < 1e-6 && (n2 - nn).abs() < 1e-6,
                    "{lat}: ({e},{nn}) -> ({e2},{n2})"
                );
            }
        }
    }

    /// The differential by central differences of `unproject` over `h` metres, in true
    /// east-north metres, independent of the closed form.
    fn numeric_to_true(f: &Frame, x: f64, y: f64, h: f64) -> Mat2 {
        let mut j = [[0.0; 2]; 2];
        for (c, d) in [(1.0, 0.0), (0.0, 1.0)].iter().enumerate() {
            let (la1, lo1) = unproject(f, x + h * d.0, y + h * d.1).unwrap();
            let (la0, lo0) = unproject(f, x - h * d.0, y - h * d.1).unwrap();
            let mut dlon = lo1 - lo0;
            if dlon > 180.0 {
                dlon -= 360.0;
            } else if dlon < -180.0 {
                dlon += 360.0;
            }
            let mid = (la0 + la1) / 2.0;
            let (n, m) = radii_at(mid);
            j[0][c] = dlon.to_radians() * n * mid.to_radians().cos() / (2.0 * h);
            j[1][c] = (la1 - la0).to_radians() * m / (2.0 * h);
        }
        j
    }

    fn close(a: &Mat2, b: &Mat2, tol: f64) -> bool {
        let scale = b.iter().flatten().fold(0.0_f64, |m, v| m.max(v.abs()));
        (0..2).all(|i| (0..2).all(|k| (a[i][k] - b[i][k]).abs() <= tol * scale))
    }

    // REQ: OA-GEO-009
    #[test]
    fn the_differential_matches_a_fine_difference_of_the_inverse_at_every_latitude() {
        // (origin latitude, point offsets in metres, step, tolerance): the pole is within
        // 1.1 km of the 89.99 origin and 11 m of the 89.9999 one, so the points and steps shrink.
        let cases = [
            (45.0, [(3000.0, -2000.0), (-40_000.0, 25_000.0)], 0.5, 1e-5),
            (89.0, [(3000.0, -2000.0), (60_000.0, 90_000.0)], 0.5, 1e-5),
            (89.99, [(600.0, 400.0), (-300.0, 800.0)], 0.01, 1e-3),
            (89.9999, [(4.0, 3.0), (-2.0, 6.0)], 0.2, 1e-2),
            (-89.99, [(600.0, 400.0), (-300.0, 800.0)], 0.01, 1e-3),
            (-45.0, [(3000.0, -2000.0), (0.0, 0.0)], 0.5, 1e-5),
        ];
        for (lat, pts, h, tol) in cases {
            let f = frame_new(lat, 20.0);
            for (x, y) in pts {
                let got = frame_to_true(&f, x, y);
                let want = numeric_to_true(&f, x, y, h);
                assert!(
                    close(&got, &want, tol),
                    "{lat} ({x},{y}): {got:?} vs {want:?}"
                );
                // The frame velocity is that matrix, and its inverse undoes it.
                let (e, n) = frame_velocity(&f, x, y, 30.0, -70.0);
                assert!(
                    (e - (got[0][0] * 30.0 - got[0][1] * 70.0)).abs() < 1e-9
                        && (n - (got[1][0] * 30.0 - got[1][1] * 70.0)).abs() < 1e-9
                );
                let (vx, vy) = frame_from_true_velocity_at(&f, x, y, e, n);
                assert!(
                    (vx - 30.0).abs() < 1e-6 && (vy + 70.0).abs() < 1e-6,
                    "{vx} {vy}"
                );
            }
        }
    }

    // REQ: OA-GEO-010
    #[test]
    fn at_the_pole_east_and_north_are_those_of_the_frame_origins_meridian() {
        let pole = |lat: f64| frame_new(lat, 30.0);
        // The frame about the north pole: plane +x is the origin's east, +y continues the
        // origin's meridian through the pole.
        let f = pole(90.0);
        let (n, m) = radii_at(90.0);
        let j = frame_to_true(&f, 0.0, 0.0);
        assert!(close(&j, &[[n / f.r, 0.0], [0.0, m / f.r]], 1e-12), "{j:?}");
        // On the far side of the pole (plane +y), moving outward is moving south and plane +x
        // is moving west.
        let c = 1000.0 / f.r;
        let k = c.sin() / c;
        let j = frame_to_true(&f, 0.0, 1000.0);
        assert!(
            close(&j, &[[-n / f.r * k, 0.0], [0.0, -m / f.r]], 1e-9),
            "{j:?}"
        );
        // A frame near the pole and a point exactly on it: the same convention, finite.
        let g = pole(89.9);
        let (px, py) = project(&g, 90.0, 123.0);
        let j = frame_to_true(&g, px, py);
        let c = px.hypot(py) / g.r;
        let k = c.sin() / c;
        assert!(j.iter().flatten().all(|v| v.is_finite()));
        assert!(
            close(&j, &[[n / g.r * k, 0.0], [0.0, m / g.r]], 1e-3),
            "{j:?}"
        );
        // The south pole mirrors it.
        let s = pole(-90.0);
        let j = frame_to_true(&s, 0.0, 0.0);
        assert!(close(&j, &[[n / s.r, 0.0], [0.0, m / s.r]], 1e-12), "{j:?}");
    }

    // REQ: OA-GEO-011
    #[test]
    fn the_inverse_differential_undoes_it_and_the_change_of_frame_is_consistent() {
        for (lat, lon_t) in [(45.0, 0.05), (89.99, 90.0), (89.99, 45.0), (-89.99, 90.0)] {
            let f = frame_new(lat, 0.0);
            let (x, y) = project(&f, lat, lon_t);
            let a = frame_to_true(&f, x, y);
            let b = frame_from_true(&f, x, y);
            let i = [
                [
                    a[0][0] * b[0][0] + a[0][1] * b[1][0],
                    a[0][0] * b[0][1] + a[0][1] * b[1][1],
                ],
                [
                    a[1][0] * b[0][0] + a[1][1] * b[1][0],
                    a[1][0] * b[0][1] + a[1][1] * b[1][1],
                ],
            ];
            assert!(close(&i, &[[1.0, 0.0], [0.0, 1.0]], 1e-12), "{i:?}");
            // The same vector, seen from a frame centred on the point: the true east and north
            // of that physical vector do not depend on the frame.
            let (la, lo) = unproject(&f, x, y).unwrap();
            let g = frame_new(la, lo);
            let (x2, y2) = project(&g, la, lo);
            let d = frame_change(&f, x, y, &g, x2, y2);
            let c = frame_to_true(&g, x2, y2);
            let via = [
                [
                    c[0][0] * d[0][0] + c[0][1] * d[1][0],
                    c[0][0] * d[0][1] + c[0][1] * d[1][1],
                ],
                [
                    c[1][0] * d[0][0] + c[1][1] * d[1][0],
                    c[1][0] * d[0][1] + c[1][1] * d[1][1],
                ],
            ];
            assert!(close(&via, &a, 1e-9), "{lat} {lon_t}: {via:?} vs {a:?}");
            // Round trip back.
            let e = frame_change(&g, x2, y2, &f, x, y);
            let id = [
                [
                    d[0][0] * e[0][0] + d[0][1] * e[1][0],
                    d[0][0] * e[0][1] + d[0][1] * e[1][1],
                ],
                [
                    d[1][0] * e[0][0] + d[1][1] * e[1][0],
                    d[1][0] * e[0][1] + d[1][1] * e[1][1],
                ],
            ];
            assert!(close(&id, &[[1.0, 0.0], [0.0, 1.0]], 1e-9), "{id:?}");
        }
    }

    // REQ: OA-GEO-012
    #[test]
    fn a_covariance_carried_by_a_rotation_keeps_its_trace_and_turns_its_axes() {
        let r = std::f64::consts::FRAC_PI_2;
        let m = [[r.cos(), -r.sin()], [r.sin(), r.cos()]];
        let c = mat2_congruence(&m, &[10000.0, 0.0, 1.0]);
        assert!((c[0] - 1.0).abs() < 1e-9 && c[1].abs() < 1e-9 && (c[2] - 10000.0).abs() < 1e-9);
        let c = mat2_congruence(&[[2.0, 0.0], [0.0, 3.0]], &[1.0, 0.5, 4.0]);
        assert_eq!(c, [4.0, 3.0, 36.0]);
    }
}
