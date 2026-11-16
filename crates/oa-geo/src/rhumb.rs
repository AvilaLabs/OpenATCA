//! Rhumb lines (loxodromes) on the ellipsoid and their separation from the geodesic.
//!
//! A rhumb line crosses every meridian at the same angle. With the isometric latitude psi
//! (d psi / d phi = M / p, where M is the meridional radius of curvature and p = N cos(phi) the
//! radius of the parallel) the course alpha satisfies tan(alpha) = d lambda / d psi, and the
//! length is ds = dmu / cos(alpha), where mu is meridian arc length. Both integrals are evaluated
//! by adaptive 8-point Gauss-Legendre quadrature (relative accuracy about 1e-15 on the
//! integrals; the integrands are analytic away from the poles, which are rejected), so there is
//! no cancellation for short lines. The length is computed as
//! `s = hypot(delta_mu, D * delta_lambda)` with `D = delta_mu / delta_psi` the mean parallel
//! radius, which stays finite for east-west lines.

use crate::error::GeoError;
use crate::geodesic::{ang_normalize, atan2d, sincosd, sq, Ellipsoid};
use crate::latlon::LatLon;

const HALF_PI: f64 = std::f64::consts::FRAC_PI_2;
/// Newton-type steps allowed before the scan is used, and the step size taken as converged.
const NEWTON_STEPS: usize = 40;
const NEWTON_TOL_M: f64 = 1e-8;
/// Golden-section steps on the separation: the 2/samples bracket shrinks to below 1e-10 of the
/// line, far finer than the separation's own rounding noise allows to resolve.
const GOLDEN_STEPS: usize = 40;
const GOLDEN: f64 = 0.618_033_988_749_894_9;

/// Distance and constant course of a rhumb line.
#[derive(Debug, Clone, Copy, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct RhumbInverse {
    /// Length along the rhumb line, metres.
    pub distance_m: f64,
    /// Constant course, degrees clockwise from north in (-180, 180].
    pub course_deg: f64,
}

/// Maximum separation between the geodesic and the rhumb line joining two points.
#[derive(Debug, Clone, Copy, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct Separation {
    /// Largest distance from a point on the rhumb line to the geodesic, metres.
    pub max_m: f64,
    /// Fraction of the rhumb length (0 to 1) at which the maximum occurs.
    pub at_fraction: f64,
    /// Geodesic length, metres.
    pub geodesic_m: f64,
    /// Rhumb length, metres.
    pub rhumb_m: f64,
}

impl Ellipsoid {
    fn gl_panel<F: Fn(f64) -> f64>(&self, f: &F, a: f64, b: f64) -> f64 {
        let mid = 0.5 * (a + b);
        let half = 0.5 * (b - a);
        let mut sum = 0.0;
        for (x, w) in self.gl_x.iter().zip(self.gl_w.iter()) {
            sum += w * f(mid + half * x);
        }
        sum * half
    }

    fn adapt<F: Fn(f64) -> f64>(&self, f: &F, a: f64, b: f64, whole: f64, depth: u32) -> f64 {
        let m = 0.5 * (a + b);
        let left = self.gl_panel(f, a, m);
        let right = self.gl_panel(f, m, b);
        let both = left + right;
        if depth == 0 || (both - whole).abs() <= 1e-14 * both.abs() {
            both
        } else {
            self.adapt(f, a, m, left, depth - 1) + self.adapt(f, m, b, right, depth - 1)
        }
    }

    /// Adaptive quadrature of `f` over [a, b] (a signed interval).
    fn integrate<F: Fn(f64) -> f64>(&self, f: F, a: f64, b: f64) -> f64 {
        if a == b {
            return 0.0;
        }
        let whole = self.gl_panel(&f, a, b);
        self.adapt(&f, a, b, whole, 14)
    }

    /// Meridional radius of curvature at latitude `phi` (radians).
    fn merid_radius(&self, phi: f64) -> f64 {
        self.a * (1.0 - self.e2) / oa_math::pow(1.0 - self.e2 * sq(oa_math::sin(phi)), 1.5)
    }

    /// Radius of the parallel at latitude `phi` (radians).
    fn parallel_radius(&self, phi: f64) -> f64 {
        self.a * oa_math::cos(phi) / (1.0 - self.e2 * sq(oa_math::sin(phi))).sqrt()
    }

    /// Meridian arc length from `phi_a` to `phi_b` (radians), metres, signed.
    fn meridian_arc(&self, phi_a: f64, phi_b: f64) -> f64 {
        self.integrate(|p| self.merid_radius(p), phi_a, phi_b)
    }

    /// Isometric latitude difference from `phi_a` to `phi_b` (radians), signed.
    fn isometric_arc(&self, phi_a: f64, phi_b: f64) -> f64 {
        self.integrate(
            |p| (1.0 - self.e2) / ((1.0 - self.e2 * sq(oa_math::sin(p))) * oa_math::cos(p)),
            phi_a,
            phi_b,
        )
    }

    /// Mean parallel radius D = delta_mu / delta_psi between two latitudes.
    fn mean_parallel_radius(&self, phi_a: f64, phi_b: f64) -> f64 {
        let dpsi = self.isometric_arc(phi_a, phi_b);
        if dpsi == 0.0 {
            self.parallel_radius(phi_a)
        } else {
            self.meridian_arc(phi_a, phi_b) / dpsi
        }
    }

    /// Distance and constant course of the rhumb line from `p1` to `p2`. The line runs the
    /// shorter way in longitude (eastward when the difference is exactly 180 degrees).
    /// Endpoints at a pole are rejected (the rhumb line spirals infinitely there).
    pub fn rhumb_inverse(&self, p1: LatLon, p2: LatLon) -> Result<RhumbInverse, GeoError> {
        if p1.lat().abs() >= 90.0 || p2.lat().abs() >= 90.0 {
            return Err(GeoError::RhumbPole);
        }
        let phi1 = p1.lat().to_radians();
        let phi2 = p2.lat().to_radians();
        let dlon = ang_normalize(p2.lon() - p1.lon());
        let dlam = dlon.to_radians();
        let dpsi = self.isometric_arc(phi1, phi2);
        let dmu = self.meridian_arc(phi1, phi2);
        let d = if dpsi == 0.0 {
            self.parallel_radius(phi1)
        } else {
            dmu / dpsi
        };
        Ok(RhumbInverse {
            distance_m: oa_math::hypot(dmu, d * dlam),
            course_deg: atan2d(dlam, dpsi),
        })
    }

    /// Destination of the rhumb line from `p1` with constant course `course_deg` and length
    /// `distance_m` (negative lengths run backwards). Errors if the line would reach a pole.
    pub fn rhumb_direct(
        &self,
        p1: LatLon,
        course_deg: f64,
        distance_m: f64,
    ) -> Result<LatLon, GeoError> {
        if !course_deg.is_finite() {
            return Err(GeoError::NonFinite("course"));
        }
        if !distance_m.is_finite() {
            return Err(GeoError::NonFinite("distance"));
        }
        if p1.lat().abs() >= 90.0 {
            return Err(GeoError::RhumbPole);
        }
        let phi1 = p1.lat().to_radians();
        let (sin_a, cos_a) = sincosd(course_deg);
        let t = distance_m * cos_a;
        let phi2 = if t == 0.0 {
            phi1
        } else {
            let pole = if t > 0.0 { HALF_PI } else { -HALF_PI };
            if t.abs() >= self.meridian_arc(phi1, pole).abs() {
                return Err(GeoError::RhumbPole);
            }
            let (mut lo, mut hi) = if t > 0.0 {
                (phi1, HALF_PI)
            } else {
                (-HALF_PI, phi1)
            };
            let mut phi = (phi1 + t / self.merid_radius(phi1)).clamp(lo, hi);
            let mut converged = false;
            for _ in 0..100 {
                let val = self.meridian_arc(phi1, phi) - t;
                if val < 0.0 {
                    lo = phi;
                } else {
                    hi = phi;
                }
                let step = val / self.merid_radius(phi);
                if val == 0.0 || step.abs() <= 2e-16 * phi.abs().max(1e-3) {
                    converged = true;
                    break;
                }
                let next = phi - step;
                phi = if next > lo && next < hi {
                    next
                } else {
                    0.5 * (lo + hi)
                };
            }
            if !converged && (hi - lo) > 1e-15 {
                return Err(GeoError::NoConvergence("rhumb direct problem"));
            }
            phi
        };
        let d = self.mean_parallel_radius(phi1, phi2);
        let dlon = (distance_m * sin_a / d).to_degrees();
        LatLon::new(
            phi2.to_degrees().clamp(-90.0, 90.0),
            ang_normalize(p1.lon() + ang_normalize(dlon)),
        )
    }

    /// The point a `fraction` (0 to 1) of the way along the rhumb line from `p1` to `p2`.
    pub fn rhumb_point_at_fraction(
        &self,
        p1: LatLon,
        p2: LatLon,
        fraction: f64,
    ) -> Result<LatLon, GeoError> {
        if !fraction.is_finite() || !(0.0..=1.0).contains(&fraction) {
            return Err(GeoError::BadParameter("fraction must be in [0, 1]"));
        }
        let inv = self.rhumb_inverse(p1, p2)?;
        if fraction == 0.0 {
            return Ok(p1);
        }
        if fraction == 1.0 {
            return Ok(p2);
        }
        self.rhumb_direct(p1, inv.course_deg, inv.distance_m * fraction)
    }

    /// Shortest distance from `p` to the geodesic segment that starts at `start` with azimuth
    /// `azi1` and length `s12`, and the distance along the segment at which it occurs.
    ///
    /// The nearest point is where the geodesic is perpendicular to the geodesic to `p`. From a
    /// guess `s` (the previous sample's answer, or the sample's own fraction) each step moves
    /// along the geodesic by `d cos(theta)`, with `d` the distance to `p` and `theta` the angle
    /// between the segment's direction there and the direction to `p`, both from the direct and
    /// inverse problems. The step converges quadratically for the short offsets of boundary
    /// edges; the result is stationary in `s`, so its error is second order in the position
    /// error. If the iteration does not settle (a very long offset) the 64-interval scan with
    /// golden-section refinement ([`Ellipsoid::distance_to_geodesic_scan`]) answers instead.
    fn distance_to_geodesic(
        &self,
        p: LatLon,
        start: LatLon,
        azi1: f64,
        s12: f64,
        guess: f64,
    ) -> Result<(f64, f64), GeoError> {
        let mut s = guess.clamp(0.0, s12);
        for _ in 0..NEWTON_STEPS {
            let q = self.direct(start, azi1, s)?;
            let inv = self.inverse(q.position, p)?;
            let theta = ang_normalize(inv.azimuth1_deg - q.azimuth2_deg).to_radians();
            let next = (s + inv.distance_m * oa_math::cos(theta)).clamp(0.0, s12);
            if (next - s).abs() < NEWTON_TOL_M {
                // Distance from the last evaluated point; it differs from the distance at
                // `next` by less than the tolerance, second order.
                return Ok((inv.distance_m, next));
            }
            s = next;
        }
        let d = self.distance_to_geodesic_scan(p, start, azi1, s12)?;
        Ok((d, s))
    }

    /// Reference method for [`Ellipsoid::distance_to_geodesic`]: a 64-interval scan along the
    /// segment brackets the nearest point and golden-section search refines it. Slow (hundreds
    /// of direct and inverse solutions) but needs no starting guess.
    fn distance_to_geodesic_scan(
        &self,
        p: LatLon,
        start: LatLon,
        azi1: f64,
        s12: f64,
    ) -> Result<f64, GeoError> {
        let dist_at = |s: f64| -> Result<f64, GeoError> {
            let q = self.direct(start, azi1, s)?;
            Ok(self.inverse(p, q.position)?.distance_m)
        };
        const K: usize = 64;
        let mut best = (0usize, f64::INFINITY);
        for i in 0..=K {
            let d = dist_at(s12 * i as f64 / K as f64)?;
            if d < best.1 {
                best = (i, d);
            }
        }
        let mut lo = s12 * best.0.saturating_sub(1) as f64 / K as f64;
        let mut hi = s12 * (best.0 + 1).min(K) as f64 / K as f64;
        let mut x1 = hi - GOLDEN * (hi - lo);
        let mut x2 = lo + GOLDEN * (hi - lo);
        let mut f1 = dist_at(x1)?;
        let mut f2 = dist_at(x2)?;
        let mut min = best.1.min(f1).min(f2);
        for _ in 0..80 {
            if f1 < f2 {
                hi = x2;
                x2 = x1;
                f2 = f1;
                x1 = hi - GOLDEN * (hi - lo);
                f1 = dist_at(x1)?;
            } else {
                lo = x1;
                x1 = x2;
                f1 = f2;
                x2 = lo + GOLDEN * (hi - lo);
                f2 = dist_at(x2)?;
            }
            min = min.min(f1).min(f2);
        }
        Ok(min)
    }

    /// OA-GEO-007: the maximum separation between the geodesic and the rhumb line joining `p1`
    /// and `p2`.
    ///
    /// Method: the rhumb line is sampled at `samples` equal fractions of its length. For each
    /// sample the true shortest distance to the geodesic segment is found (not the distance to
    /// the point at the same fraction) by Newton-type steps along the geodesic until the step
    /// is below 1e-8 m (the 64-interval scan with golden-section refinement is the fallback). The sample
    /// with the largest distance is then refined by golden-section search over the two
    /// neighbouring intervals (40 iterations), because the separation is a smooth single-hump
    /// function of the fraction. Accuracy: a few micrometres for lines up to several thousand
    /// kilometres (checked against the closed-form spherical answer in the tests); the result
    /// is a lower bound that converges as `samples` grows, and the refinement makes it
    /// insensitive to `samples` once the hump is bracketed.
    /// `samples` must be between 2 and 100000. Endpoints at a pole are rejected.
    pub fn geodesic_rhumb_separation(
        &self,
        p1: LatLon,
        p2: LatLon,
        samples: usize,
    ) -> Result<Separation, GeoError> {
        if !(2..=100_000).contains(&samples) {
            return Err(GeoError::BadParameter("samples must be in 2..=100000"));
        }
        let geo = self.inverse(p1, p2)?;
        let rhumb = self.rhumb_inverse(p1, p2)?;
        // The nearest geodesic point moves smoothly with the sample, so each solve starts from
        // the previous answer.
        let foot = std::cell::Cell::new(f64::NAN);
        let sep_at = |frac: f64| -> Result<f64, GeoError> {
            // The rhumb line was solved once above; only the direct problem varies per sample.
            let q = self.rhumb_direct(p1, rhumb.course_deg, rhumb.distance_m * frac)?;
            let guess = if foot.get().is_nan() {
                frac * geo.distance_m
            } else {
                foot.get()
            };
            let (d, s) =
                self.distance_to_geodesic(q, p1, geo.azimuth1_deg, geo.distance_m, guess)?;
            foot.set(s);
            Ok(d)
        };
        let mut best = (0usize, 0.0f64);
        for i in 1..samples {
            let d = sep_at(i as f64 / samples as f64)?;
            if d > best.1 {
                best = (i, d);
            }
        }
        let n = samples as f64;
        let mut lo = (best.0.saturating_sub(1)) as f64 / n;
        let mut hi = ((best.0 + 1).min(samples)) as f64 / n;
        let mut x1 = hi - GOLDEN * (hi - lo);
        let mut x2 = lo + GOLDEN * (hi - lo);
        let mut f1 = sep_at(x1)?;
        let mut f2 = sep_at(x2)?;
        let mut top = (best.0 as f64 / n, best.1);
        for _ in 0..GOLDEN_STEPS {
            if f1 > top.1 {
                top = (x1, f1);
            }
            if f2 > top.1 {
                top = (x2, f2);
            }
            if f1 > f2 {
                hi = x2;
                x2 = x1;
                f2 = f1;
                x1 = hi - GOLDEN * (hi - lo);
                f1 = sep_at(x1)?;
            } else {
                lo = x1;
                x1 = x2;
                f1 = f2;
                x2 = lo + GOLDEN * (hi - lo);
                f2 = sep_at(x2)?;
            }
        }
        Ok(Separation {
            max_m: top.1,
            at_fraction: top.0,
            geodesic_m: geo.distance_m,
            rhumb_m: rhumb.distance_m,
        })
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use crate::{Ellipsoid, LatLon};

    // REQ: OA-GEO-028
    #[test]
    fn fallback_distance_scan_matches_spherical_interior_and_endpoint_minima() {
        let radius = 6_371_000.0;
        let sphere = Ellipsoid::new(radius, 0.0).expect("sphere");
        let start = LatLon::new(0.0, -20.0).expect("start");
        let length = radius * 40.0f64.to_radians();
        for (lat, lon, expected) in [
            (0.0, 3.7, 0.0),
            (5.0, 3.7, radius * 5.0f64.to_radians()),
            // Outside the segment: distance to its western endpoint. Independent
            // 100-digit value of R acos(cos(5 degrees) cos(10 degrees)).
            (5.0, -30.0, 1_241_930.888_666_342_2),
        ] {
            let point = LatLon::new(lat, lon).expect("query");
            let got = sphere
                .distance_to_geodesic_scan(point, start, 90.0, length)
                .expect("fallback scan");
            assert!(
                (got - expected).abs() < 1e-6,
                "{lat} {lon}: {got} vs {expected}"
            );
        }
    }
}
