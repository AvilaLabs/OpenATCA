//! Point-in-polygon where every polygon edge is a geodesic on the ellipsoid.
//!
//! Method (OA-GEO-006): crossing-number test along the meridian through the test point. The
//! north pole is outside any polygon this type accepts, so a point is inside exactly when the
//! half meridian running north from it crosses the boundary an odd number of times. Longitude
//! is strictly monotone along a geodesic that is not a meridian, so for each edge the point
//! where it crosses the test longitude is unique; it is found on the arc length with the direct
//! problem by a bracketed secant (Illinois) iteration, about six evaluations, converged to a few
//! nanometres, and the
//! edge counts when that crossing lies north of the test point. Vertices that sit exactly on
//! the test meridian follow a half-open rule (a vertex belongs to the eastern side), so a
//! vertex is never counted twice or missed. Edges that cross the antimeridian are handled
//! because all longitudes are taken relative to the test point.
//!
//! Limits, all enforced when the polygon is built:
//! * at least 3 distinct vertices, at most 100 000, none at a pole;
//! * every edge shorter than 10 000 km and spanning less than 180 degrees of longitude;
//! * the boundary must not enclose a pole (detected: the longitude change summed round the
//!   ring would be +-360 degrees instead of 0);
//! * edges must not cross each other (not checked; behaviour for a self-intersecting ring is
//!   the even-odd rule, which is what this test computes).
//!
//! A point lying exactly on the boundary may be reported either way.

use crate::error::GeoError;
use crate::geodesic::{ang_normalize, Ellipsoid};
use crate::latlon::LatLon;
use alloc::vec::Vec;

const MAX_VERTICES: usize = 100_000;
const MAX_EDGE_M: f64 = 10_000_000.0;

/// Hot: one per polygon edge, scanned on every containment test (A7).
#[derive(Debug, Clone, Copy)]
#[repr(C)]
struct Edge {
    start: LatLon,
    azi1: f64,
    s12: f64,
    /// Signed longitude change along the edge, degrees, unreduced.
    span: f64,
}
const _: () = assert!(core::mem::size_of::<Edge>() == 40);

/// A polygon whose edges are geodesics. The ring is closed implicitly (a repeated first vertex
/// at the end is accepted and dropped).
#[derive(Debug, Clone)]
// A7.2: cold struct, size guard waived
pub struct Polygon {
    ell: Ellipsoid,
    edges: Vec<Edge>,
}

impl Polygon {
    /// Builds and validates a polygon; see the module documentation for the limits.
    pub fn new(ell: &Ellipsoid, vertices: &[LatLon]) -> Result<Polygon, GeoError> {
        if vertices.len() > MAX_VERTICES + 1 {
            return Err(GeoError::BadPolygon("more than 100000 vertices"));
        }
        let mut ring: Vec<LatLon> = Vec::with_capacity(vertices.len());
        for &v in vertices {
            if v.lat().abs() >= 90.0 {
                return Err(GeoError::BadPolygon("vertex at a pole"));
            }
            if ring.last() != Some(&v) {
                ring.push(v);
            }
        }
        while ring.len() > 1 && ring.first() == ring.last() {
            ring.pop();
        }
        if ring.len() < 3 {
            return Err(GeoError::BadPolygon("fewer than 3 distinct vertices"));
        }
        let mut edges = Vec::with_capacity(ring.len());
        let mut total_span = 0.0;
        for (i, &a) in ring.iter().enumerate() {
            // `i + 1` wraps to 0, and the ring has at least three vertices, so `get` succeeds.
            let b = ring.get((i + 1) % ring.len()).copied().unwrap_or(a);
            let inv = ell.inverse(a, b)?;
            if inv.distance_m >= MAX_EDGE_M {
                return Err(GeoError::BadPolygon("edge longer than 10000 km"));
            }
            let (_, span, _) = ell.direct_unrolled(a.lat(), inv.azimuth1_deg, inv.distance_m);
            if !span.is_finite() || span.abs() >= 180.0 {
                return Err(GeoError::BadPolygon("edge spans 180 degrees of longitude"));
            }
            total_span += span;
            edges.push(Edge {
                start: a,
                azi1: inv.azimuth1_deg,
                s12: inv.distance_m,
                span,
            });
        }
        if total_span.abs() > 180.0 {
            return Err(GeoError::BadPolygon("boundary encloses a pole"));
        }
        Ok(Polygon {
            ell: ell.clone(),
            edges,
        })
    }

    /// Number of edges.
    pub fn len(&self) -> usize {
        self.edges.len()
    }

    /// Always false: a valid polygon has at least three edges.
    pub fn is_empty(&self) -> bool {
        self.edges.is_empty()
    }

    /// True if `p` is inside the polygon (even-odd rule). A point on the boundary may be
    /// reported either way.
    pub fn contains(&self, p: LatLon) -> bool {
        let mut inside = false;
        for edge in &self.edges {
            let a = ang_normalize(edge.start.lon() - p.lon());
            let w = a + edge.span;
            if (a > 0.0) == (w > 0.0) {
                continue;
            }
            if self.crossing_latitude(edge, a) > p.lat() {
                inside = !inside;
            }
        }
        inside
    }

    /// Latitude at which `edge` crosses the test meridian, where `a` is the longitude of the
    /// edge start relative to that meridian.
    ///
    /// Solves `lon(s) + a = 0` for the arc length `s`, where `lon(s)` rises (or falls) steadily
    /// along the edge, by the Illinois form of regula falsi inside the bracket `[0, s12]`: the
    /// longitude is nearly linear in `s`, so about six direct problems reach 1e-14 degrees (a
    /// few nanometres) where bisection needed 64. Stops on a longitude residual under 1e-14
    /// degrees, a bracket narrower than 1e-9 m, or after 100 steps.
    fn crossing_latitude(&self, edge: &Edge, a: f64) -> f64 {
        let dir = if edge.span < 0.0 { -1.0 } else { 1.0 };
        let f = |s: f64| {
            let (_, lon12, _) = self.ell.direct_unrolled(edge.start.lat(), edge.azi1, s);
            dir * (lon12 + a)
        };
        let (mut lo, mut hi) = (0.0, edge.s12);
        // The bracket ends are known without a solve: lon(0) = 0 and lon(s12) = span.
        let (mut flo, mut fhi) = (dir * a, dir * (a + edge.span));
        let mut side = 0i8;
        let mut s = 0.5 * (lo + hi);
        for _ in 0..100 {
            s = if fhi != flo && flo <= 0.0 && fhi > 0.0 {
                (lo * fhi - hi * flo) / (fhi - flo)
            } else {
                0.5 * (lo + hi)
            };
            // Rounding can put the secant point on or outside the bracket; bisect then.
            if !(s > lo && s < hi) {
                s = 0.5 * (lo + hi);
            }
            let fs = f(s);
            if fs.abs() < 1e-14 || hi - lo < 1e-9 {
                break;
            }
            if fs <= 0.0 {
                lo = s;
                flo = fs;
                if side == -1 {
                    fhi *= 0.5;
                }
                side = -1;
            } else {
                hi = s;
                fhi = fs;
                if side == 1 {
                    flo *= 0.5;
                }
                side = 1;
            }
        }
        let (lat, _, _) = self.ell.direct_unrolled(edge.start.lat(), edge.azi1, s);
        lat
    }
}
