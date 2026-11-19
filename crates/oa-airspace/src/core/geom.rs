//! Geometry the selection and the report need, on top of `oa-geo`. **Core** (B1, B7): pure functions
//! of coordinates.
//!
//! * [`Area`]: the selection area, the union of the ARTCC's HIGH and LOW polygons and a buffer
//!   around their edges (OA-ASP-002). A point is in the area when it is inside either polygon
//!   or its geodesic distance to the nearest polygon edge is at most the buffer.
//! * The nearest-edge distance is found in two steps so the build stays fast: a spherical
//!   estimate per edge (cross-track on a sphere of the mean radius, off by at most about 0.4 %
//!   from the ellipsoid, plus the bow of the great circle away from the geodesic, up to 80 m on
//!   a 600 NM edge) decides every point outside the grey band around the buffer (1 % of it, or
//!   the scale error plus the bow bound of the edge when that is larger); only the rest are
//!   measured exactly (geodesic inverse and direct problems along the edge).
//! * [`edge_separations`]: per boundary, the largest geodesic-versus-rhumb separation over its
//!   edges (OA-ASP-005).
//! * Sampling helpers turn boundary descriptions (vertices, arcs, circles) into dense point
//!   lists used only for the "does it touch the area" tests; the model keeps the exact
//!   description. A volume touches the area when a point of its outline is in the area (inside
//!   a polygon or within the buffer of an edge), when an outline edge crosses an edge of the
//!   area, when a vertex of the area is within the buffer of an outline edge, or when a vertex
//!   of the area is inside the outline (OA-ASP-065). The outline is dense enough that each
//!   chord lies within [`ARC_CHORD_M`] of the curve it stands for.
//!
//! Coordinates enter as the model's 1e-7 degree integers, so selection is a function of the
//! values the snapshot stores.

use oa_geo::{wgs84, Ellipsoid, GeoError, LatLon, Polygon};

/// Degrees per model unit.
pub const DEG_PER_E7: f64 = 1e-7;
/// Mean earth radius used by the spherical estimate, metres.
const R_MEAN_M: f64 = 6_371_008.8;
/// Smallest relative margin around the buffer inside which the exact distance decides (see
/// [`grey_m`] for the wider band of a long edge).
const GREY: f64 = 0.01;
/// Largest relative error of the spherical estimate from the scale difference between the
/// sphere and the ellipsoid (0.42 % at 24 degrees north), rounded up.
const SCALE_ERR: f64 = 0.005;
/// WGS 84 flattening, for the bow bound of [`bow_bound_m`].
const FLATTENING: f64 = 1.0 / 298.257_223_563;
/// Spacing of the points sampled along a long edge for intersection tests, metres.
pub const SAMPLE_STEP_M: f64 = 20_000.0;
/// Spacing of the points of a densified geodesic, rhumb or parallel edge, metres. Each point
/// is on the edge, and the great-circle chord between neighbours leaves the geodesic by far
/// less than a millimetre (the chord of a parallel by under a metre at 80 degrees).
pub const DENSE_STEP_M: f64 = 5_000.0;
/// Largest distance between a densified arc and the arc itself, metres.
pub const ARC_CHORD_M: f64 = 0.2;
/// Spacing along a parallel of latitude, where the chord leaves the parallel by about
/// `step^2 * tan(latitude) / (8 * R)`, metres.
const PARALLEL_STEP_M: f64 = 1_000.0;
/// Most points of one densified arc.
const MAX_ARC_POINTS: usize = 20_000;
/// Most sample points one boundary may produce (OA-ASP-074); a boundary that would need more is
/// refused before any point is made.
pub const MAX_BOUNDARY_SAMPLES: usize = 1 << 20;
/// Most edge tests one [`samples_touch_area`] call may make (samples times the area's edges,
/// dense edges and vertices); a call that would pass it is refused before any test is made.
pub const MAX_TOUCH_WORK: u64 = 1 << 34;
/// How far an arc's coded radius may differ from the distance between its centre and either
/// end before the build reports the volume, metres. The radius is coded in tenths of a
/// nautical mile (ARINC 424 cols 71-74), so rounding alone moves it by up to 0.05 NM
/// (92.6 m); one more metre covers the whole-metre storage and the 1e-7 degree positions.
pub const ARC_RADIUS_TOLERANCE_M: f64 = 92.6 + 1.0;

/// Degrees from a model unit value.
pub fn e7_to_deg(v: i32) -> f64 {
    f64::from(v) * DEG_PER_E7
}

/// Model units from degrees, rounded to nearest; `None` outside the `i32` range or non-finite.
pub fn deg_to_e7(d: f64) -> Option<i32> {
    let v = (d * 1e7).round();
    if v.is_finite() && v >= f64::from(i32::MIN) && v <= f64::from(i32::MAX) {
        Some(v as i32)
    } else {
        None
    }
}

/// A validated coordinate from model units.
pub fn latlon_e7(lat: i32, lon: i32) -> Result<LatLon, GeoError> {
    LatLon::new(e7_to_deg(lat), e7_to_deg(lon))
}

type V3 = [f64; 3];

fn unit(p: LatLon) -> V3 {
    let (la, lo) = (p.lat().to_radians(), p.lon().to_radians());
    [la.cos() * lo.cos(), la.cos() * lo.sin(), la.sin()]
}

fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn norm(a: V3) -> f64 {
    dot(a, a).sqrt()
}

/// Angle between two unit vectors, radians.
fn angle(a: V3, b: V3) -> f64 {
    norm(cross(a, b)).atan2(dot(a, b))
}

/// One polygon edge with what the spherical estimate needs.
#[derive(Clone, Debug)]
// A7.2: cold struct, size guard waived
struct Edge {
    a: LatLon,
    b: LatLon,
    a3: V3,
    b3: V3,
    /// Unit normal of the great-circle plane through the edge; zero for a degenerate edge.
    n3: V3,
    /// Unit vector to the middle of the edge, and half its angular length, radians.
    c3: V3,
    half: f64,
    /// Bound on how far the great circle through the end points lies from the geodesic, metres.
    bow_m: f64,
}

fn edge_of(a: LatLon, b: LatLon) -> Edge {
    let (a3, b3) = (unit(a), unit(b));
    let c = cross(a3, b3);
    let n = norm(c);
    let n3 = if n > 1e-12 {
        [c[0] / n, c[1] / n, c[2] / n]
    } else {
        [0.0; 3]
    };
    let m = [a3[0] + b3[0], a3[1] + b3[1], a3[2] + b3[2]];
    let mn = norm(m);
    let c3 = if mn > 1e-9 {
        [m[0] / mn, m[1] / mn, m[2] / mn]
    } else {
        a3
    };
    Edge {
        a,
        b,
        a3,
        b3,
        n3,
        c3,
        half: angle(a3, b3) / 2.0,
        bow_m: bow_bound_m(angle(a3, b3) * R_MEAN_M),
    }
}

/// Bound on the offset between the great circle through two geodetic coordinates and the
/// geodesic between them, for an edge of `len_m` metres: 1.25 times `f * L^2 / (8 R)`, the
/// measured worst case (80 m at 600 NM) being 0.99 times that term.
fn bow_bound_m(len_m: f64) -> f64 {
    1.25 * FLATTENING * len_m * len_m / (8.0 * R_MEAN_M)
}

/// Half-width of the band around the buffer inside which the spherical estimate for `e` does
/// not decide: the larger of [`GREY`] of the buffer and the scale error plus the bow bound.
fn grey_m(e: &Edge, buffer_m: f64) -> f64 {
    (GREY * buffer_m).max(SCALE_ERR * buffer_m + e.bow_m)
}

/// Spherical estimate of the distance from `p` to the edge, metres.
fn approx_edge_m(e: &Edge, p: V3) -> f64 {
    let mut best = angle(p, e.a3).min(angle(p, e.b3));
    if norm(e.n3) > 0.5 {
        let s = dot(e.n3, p);
        // Foot of the perpendicular on the great circle, then check it lies between a and b.
        let q = [p[0] - s * e.n3[0], p[1] - s * e.n3[1], p[2] - s * e.n3[2]];
        if norm(q) > 1e-12 && dot(cross(e.a3, q), e.n3) >= 0.0 && dot(cross(q, e.b3), e.n3) >= 0.0 {
            best = best.min(s.abs().min(1.0).asin());
        }
    }
    best * R_MEAN_M
}

/// Exact geodesic distance from `p` to the geodesic segment `a`-`b`, metres: a scan along the
/// segment brackets the nearest point, golden-section search refines it.
pub fn exact_edge_m(ell: &Ellipsoid, a: LatLon, b: LatLon, p: LatLon) -> Result<f64, GeoError> {
    const SCAN: usize = 16;
    const GOLDEN: f64 = 0.618_033_988_749_894_9;
    let ab = ell.inverse(a, b)?;
    let len = ab.distance_m;
    if len < 1e-6 {
        return Ok(ell.inverse(a, p)?.distance_m);
    }
    let f = |s: f64| -> Result<f64, GeoError> {
        let q = ell.direct(a, ab.azimuth1_deg, s.clamp(0.0, len))?.position;
        Ok(ell.inverse(q, p)?.distance_m)
    };
    let mut best = (0usize, f64::INFINITY);
    for i in 0..=SCAN {
        let d = f(len * i as f64 / SCAN as f64)?;
        if d < best.1 {
            best = (i, d);
        }
    }
    let step = len / SCAN as f64;
    let mut lo = (best.0 as f64 - 1.0).max(0.0) * step;
    let mut hi = (best.0 as f64 + 1.0).min(SCAN as f64) * step;
    let mut x1 = hi - GOLDEN * (hi - lo);
    let mut x2 = lo + GOLDEN * (hi - lo);
    let (mut f1, mut f2) = (f(x1)?, f(x2)?);
    let mut top = best.1.min(f1).min(f2);
    for _ in 0..40 {
        if f1 < f2 {
            hi = x2;
            x2 = x1;
            f2 = f1;
            x1 = hi - GOLDEN * (hi - lo);
            f1 = f(x1)?;
            top = top.min(f1);
        } else {
            lo = x1;
            x1 = x2;
            f1 = f2;
            x2 = lo + GOLDEN * (hi - lo);
            f2 = f(x2)?;
            top = top.min(f2);
        }
    }
    Ok(top)
}

/// Bounding box in degrees; used only to skip points that cannot be in the area.
///
/// The latitude range includes the poleward bulge of every geodesic edge (OA-ASP-008). The
/// longitude range is unwrapped: `max_lon - min_lon` is the true width, and either end may lie
/// outside [-180, 180] when the area crosses the antimeridian. Always test longitudes with
/// [`lon_overlaps`], which compares modulo 360 degrees.
// A7.2: cold struct, size guard waived (one per build).
#[derive(Clone, Copy, Debug)]
pub struct Bbox {
    pub min_lat: f64,
    pub max_lat: f64,
    pub min_lon: f64,
    pub max_lon: f64,
}

/// Wraps a longitude difference into [-180, 180).
fn wrap180(d: f64) -> f64 {
    d - 360.0 * ((d + 180.0) / 360.0).floor()
}

/// True when the longitude intervals [`a_lo`, `a_hi`] and [`b_lo`, `b_hi`] share a point
/// modulo 360 degrees. Each interval is unwrapped (its ends may lie outside [-180, 180]) and
/// at most 360 degrees wide, so shifts of up to two turns cover every case.
pub fn lon_overlaps(a_lo: f64, a_hi: f64, b_lo: f64, b_hi: f64) -> bool {
    [-720.0, -360.0, 0.0, 360.0, 720.0]
        .iter()
        .any(|k| a_lo <= b_hi + k && a_hi >= b_lo + k)
}

/// The longitude range of a sequence of longitudes, unwrapped around the first one: each value
/// is placed within 180 degrees of `reference`, so the range covers every value modulo 360.
fn lon_range(reference: f64, lons: impl Iterator<Item = f64>) -> Option<(f64, f64)> {
    let mut range: Option<(f64, f64)> = None;
    for l in lons {
        let u = reference + wrap180(l - reference);
        range = Some(match range {
            Some((lo, hi)) => (lo.min(u), hi.max(u)),
            None => (u, u),
        });
    }
    range
}

/// Latitude of the geodesic's northern or southern vertex between `a` and `b`, when it lies
/// inside the edge (the forward azimuth changes from north-going to south-going or the other
/// way), else `None`. Clairaut's relation on the ellipsoid: `cos(beta) sin(azimuth)` is constant
/// along the geodesic, where `beta` is the reduced latitude, so the vertex is where
/// `cos(beta_max) = |cos(beta_a) sin(azimuth_a)|`. Returns the signed latitude of the vertex
/// (positive for a northern one).
fn edge_vertex_lat(ell: &Ellipsoid, a: LatLon, b: LatLon) -> Result<Option<f64>, GeoError> {
    let inv = ell.inverse(a, b)?;
    let (c1, c2) = (
        inv.azimuth1_deg.to_radians().cos(),
        inv.azimuth2_deg.to_radians().cos(),
    );
    if c1 * c2 >= 0.0 {
        return Ok(None);
    }
    let f1 = 1.0 - ell.flattening();
    let beta_a = (f1 * a.lat().to_radians().tan()).atan();
    let k = (beta_a.cos() * inv.azimuth1_deg.to_radians().sin())
        .abs()
        .min(1.0);
    let beta_max = k.acos();
    let lat = (beta_max.tan() / f1).atan().to_degrees();
    Ok(Some(if c1 > 0.0 { lat } else { -lat }))
}

/// Bounding box of one ring of geodesic edges, for skipping points that cannot be inside it:
/// the latitude range includes the poleward bulge of every edge, the longitude range is
/// unwrapped as in [`Bbox`] (test with [`lon_overlaps`]), and both are padded by a millionth
/// of a degree. `None` when a box is not a safe filter: the ring is wider than 120 degrees of
/// longitude, reaches past 80 degrees of latitude, or an edge cannot be solved.
pub fn ring_bbox(ell: &Ellipsoid, ring: &[LatLon]) -> Option<Bbox> {
    const PAD: f64 = 1e-6;
    let first = ring.first()?;
    let (lo, hi) = lon_range(first.lon(), ring.iter().map(|v| v.lon()))?;
    let (mut min_lat, mut max_lat) = (f64::INFINITY, f64::NEG_INFINITY);
    for (i, a) in ring.iter().enumerate() {
        let b = ring.get((i + 1) % ring.len()).copied().unwrap_or(*a);
        min_lat = min_lat.min(a.lat());
        max_lat = max_lat.max(a.lat());
        if let Some(v) = edge_vertex_lat(ell, *a, b).ok()? {
            min_lat = min_lat.min(v);
            max_lat = max_lat.max(v);
        }
    }
    if hi - lo > 120.0 || min_lat.abs().max(max_lat.abs()) > 80.0 {
        return None;
    }
    Some(Bbox {
        min_lat: min_lat - PAD,
        max_lat: max_lat + PAD,
        min_lon: lo - PAD,
        max_lon: hi + PAD,
    })
}

/// The selection area: polygons, the buffer, and prepared edges.
#[derive(Clone, Debug)]
// A7.2: cold struct, size guard waived
pub struct Area {
    pub ell: Ellipsoid,
    polys: Vec<Polygon>,
    /// The rings the polygons were built from (their vertices, for containment tests).
    pub rings: Vec<Vec<LatLon>>,
    edges: Vec<Edge>,
    /// The same rings as dense chords (see [`DENSE_STEP_M`]), for the crossing tests.
    dense: Vec<Edge>,
    /// The longest half-length among `dense`, radians.
    dense_half: f64,
    pub buffer_m: f64,
    /// Bounding box of the area including the buffer; `None` when the rings are too wide for a
    /// box to be a safe filter (more than 120 degrees of longitude, or near a pole).
    pub bbox: Option<Bbox>,
}

/// Builds the area from rings of vertices. Each ring must be a valid geodesic polygon for
/// `oa-geo`; the error says which limit it broke.
pub fn build_area(rings: Vec<Vec<LatLon>>, buffer_m: f64) -> Result<Area, GeoError> {
    let ell = wgs84();
    let mut polys = Vec::with_capacity(rings.len());
    let mut edges = Vec::new();
    let (mut min_lat, mut max_lat) = (f64::INFINITY, f64::NEG_INFINITY);
    let reference = rings.iter().flatten().next().map_or(0.0, |v| v.lon());
    let (mut min_lon, mut max_lon) = (f64::INFINITY, f64::NEG_INFINITY);
    for ring in &rings {
        polys.push(Polygon::new(&ell, ring)?);
        if let Some((lo, hi)) = lon_range(reference, ring.iter().map(|v| v.lon())) {
            min_lon = min_lon.min(lo);
            max_lon = max_lon.max(hi);
        }
        for (i, a) in ring.iter().enumerate() {
            let b = ring.get((i + 1) % ring.len()).copied().unwrap_or(*a);
            edges.push(edge_of(*a, b));
            min_lat = min_lat.min(a.lat());
            max_lat = max_lat.max(a.lat());
            // A geodesic bulges poleward between its ends (OA-ASP-008).
            if let Some(v) = edge_vertex_lat(&ell, *a, b)? {
                min_lat = min_lat.min(v);
                max_lat = max_lat.max(v);
            }
        }
    }
    let mut dense = Vec::new();
    for ring in &rings {
        let pts = dense_ring_samples(&ell, ring)?;
        dense.extend(closed_edges(&pts));
    }
    let dense_half = dense.iter().fold(0.0_f64, |m, e| m.max(e.half));
    let dlat = buffer_m / 110_000.0 + 0.01;
    let worst_lat = (min_lat.abs().max(max_lat.abs()) + dlat).min(89.0);
    let dlon = buffer_m / (111_000.0 * worst_lat.to_radians().cos().max(0.05)) + 0.01;
    let bbox = if edges.is_empty() || max_lon - min_lon > 120.0 || worst_lat > 80.0 {
        None
    } else {
        Some(Bbox {
            min_lat: min_lat - dlat,
            max_lat: max_lat + dlat,
            min_lon: min_lon - dlon,
            max_lon: max_lon + dlon,
        })
    };
    Ok(Area {
        ell,
        polys,
        rings,
        edges,
        dense,
        dense_half,
        buffer_m,
        bbox,
    })
}

fn in_bbox(area: &Area, p: LatLon) -> bool {
    match area.bbox {
        Some(b) => {
            p.lat() >= b.min_lat
                && p.lat() <= b.max_lat
                && lon_overlaps(p.lon(), p.lon(), b.min_lon, b.max_lon)
        }
        None => true,
    }
}

/// True when `p` is inside any of the area's polygons (no buffer).
pub fn area_inside(area: &Area, p: LatLon) -> bool {
    in_bbox(area, p) && area.polys.iter().any(|poly| poly.contains(p))
}

/// Geodesic distance from `p` to the nearest edge of any polygon, metres (exact).
pub fn area_edge_distance_m(area: &Area, p: LatLon) -> Result<f64, GeoError> {
    let mut best = f64::INFINITY;
    for e in &area.edges {
        best = best.min(exact_edge_m(&area.ell, e.a, e.b, p)?);
    }
    Ok(best)
}

/// True when `p` is inside a polygon or within the buffer of an edge.
pub fn area_contains(area: &Area, p: LatLon) -> Result<bool, GeoError> {
    if !in_bbox(area, p) {
        return Ok(false);
    }
    if area.polys.iter().any(|poly| poly.contains(p)) {
        return Ok(true);
    }
    let p3 = unit(p);
    let approx: Vec<f64> = area.edges.iter().map(|e| approx_edge_m(e, p3)).collect();
    let mut undecided = false;
    for (e, est) in area.edges.iter().zip(&approx) {
        let g = grey_m(e, area.buffer_m);
        if *est < area.buffer_m - g {
            return Ok(true);
        }
        undecided |= *est <= area.buffer_m + g;
    }
    if !undecided {
        return Ok(false);
    }
    // Within the grey band: measure the edges that could be that close exactly.
    for (e, est) in area.edges.iter().zip(&approx) {
        if *est <= area.buffer_m + 2.0 * grey_m(e, area.buffer_m)
            && exact_edge_m(&area.ell, e.a, e.b, p)? <= area.buffer_m
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Points along the geodesic from `a` to `b`, from `a` up to but not including `b`, at most
/// `step_m` apart.
pub fn edge_samples(
    ell: &Ellipsoid,
    a: LatLon,
    b: LatLon,
    step_m: f64,
) -> Result<Vec<LatLon>, GeoError> {
    let mut remaining = MAX_BOUNDARY_SAMPLES;
    edge_samples_within(ell, a, b, step_m, &mut remaining)
}

/// The error of a boundary that needs more than [`MAX_BOUNDARY_SAMPLES`] points.
fn over_sample_budget() -> GeoError {
    GeoError::BadParameter("boundary sample count exceeds the work budget")
}

/// Takes `n` points from `remaining`, or refuses.
fn take_samples(remaining: &mut usize, n: usize) -> Result<(), GeoError> {
    *remaining = remaining.checked_sub(n).ok_or_else(over_sample_budget)?;
    Ok(())
}

/// [`edge_samples`], drawing its point count from `remaining` before making any point.
fn edge_samples_within(
    ell: &Ellipsoid,
    a: LatLon,
    b: LatLon,
    step_m: f64,
    remaining: &mut usize,
) -> Result<Vec<LatLon>, GeoError> {
    let inv = ell.inverse(a, b)?;
    let n = ((inv.distance_m / step_m).ceil() as usize).clamp(1, 10_000);
    take_samples(remaining, n)?;
    let mut out = Vec::with_capacity(n);
    out.push(a);
    for i in 1..n {
        out.push(
            ell.direct(a, inv.azimuth1_deg, inv.distance_m * i as f64 / n as f64)?
                .position,
        );
    }
    Ok(out)
}

/// Samples of a ring joined by geodesics (closing edge included).
pub fn ring_samples(ell: &Ellipsoid, ring: &[LatLon]) -> Result<Vec<LatLon>, GeoError> {
    ring_samples_at(ell, ring, SAMPLE_STEP_M)
}

/// The points of a ring's geodesic edges `step_m` apart, within the sample budget.
fn ring_samples_at(ell: &Ellipsoid, ring: &[LatLon], step_m: f64) -> Result<Vec<LatLon>, GeoError> {
    let mut out = Vec::new();
    let mut remaining = MAX_BOUNDARY_SAMPLES;
    for (i, a) in ring.iter().enumerate() {
        let b = ring.get((i + 1) % ring.len()).copied().unwrap_or(*a);
        out.extend(edge_samples_within(ell, *a, b, step_m, &mut remaining)?);
    }
    Ok(out)
}

/// Samples of a ring joined by geodesics, dense enough for exact edge tests (closing edge
/// included): the points are [`DENSE_STEP_M`] apart.
pub fn dense_ring_samples(ell: &Ellipsoid, ring: &[LatLon]) -> Result<Vec<LatLon>, GeoError> {
    ring_samples_at(ell, ring, DENSE_STEP_M)
}

/// Points along the rhumb line from `a` to `b`, from `a` up to but not including `b`, at most
/// `step_m` apart.
fn rhumb_samples(
    ell: &Ellipsoid,
    a: LatLon,
    b: LatLon,
    step_m: f64,
    remaining: &mut usize,
) -> Result<Vec<LatLon>, GeoError> {
    let inv = ell.rhumb_inverse(a, b)?;
    let n = ((inv.distance_m / step_m).ceil() as usize).clamp(1, 10_000);
    take_samples(remaining, n)?;
    let mut out = Vec::with_capacity(n);
    out.push(a);
    for i in 1..n {
        out.push(ell.rhumb_point_at_fraction(a, b, i as f64 / n as f64)?);
    }
    Ok(out)
}

/// A boundary record as the sampler needs it: the via code, the point (none for a circle), the
/// arc or circle centre and radius in metres.
#[derive(Clone, Copy, Debug)]
// A7.2: cold struct, size guard waived
pub struct SegIn {
    pub via: u8,
    pub pos: Option<LatLon>,
    pub centre: Option<LatLon>,
    pub radius_m: f64,
}

/// Points of an arc of the circle of radius `radius_m` around `centre`, from bearing
/// `from_deg` through `sweep_deg`, up to but not including the far end. The spacing keeps each
/// chord within [`ARC_CHORD_M`] of the arc.
fn arc_samples(
    ell: &Ellipsoid,
    centre: LatLon,
    radius_m: f64,
    from_deg: f64,
    sweep_deg: f64,
    remaining: &mut usize,
) -> Result<Vec<LatLon>, GeoError> {
    let r = radius_m.max(2.0 * ARC_CHORD_M);
    let max_step_deg = (2.0 * (1.0 - ARC_CHORD_M / r).acos()).to_degrees();
    let n = ((sweep_deg.abs() / max_step_deg).ceil() as usize).clamp(1, MAX_ARC_POINTS);
    take_samples(remaining, n)?;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let brg = from_deg + sweep_deg * i as f64 / n as f64;
        out.push(ell.direct(centre, brg, radius_m)?.position);
    }
    Ok(out)
}

/// The sweep of an arc segment from its start to the next point: bearings from the centre to
/// each end, positive clockwise, negative for a counter-clockwise arc.
fn arc_sweep(
    ell: &Ellipsoid,
    via: u8,
    centre: LatLon,
    a: LatLon,
    b: LatLon,
) -> Result<(f64, f64), GeoError> {
    use crate::model::via as v;
    let from = ell.inverse(centre, a)?.azimuth1_deg;
    let to = ell.inverse(centre, b)?.azimuth1_deg;
    let cw = (to - from).rem_euclid(360.0);
    Ok((from, if via == v::ARC_CW { cw } else { cw - 360.0 }))
}

/// Dense points of an ARINC 424 style boundary: geodesic edges as geodesics and rhumb edges as
/// rhumb lines, [`DENSE_STEP_M`] apart; arcs and circles at their coded radius around their
/// centres, with the arc's ends at the stored points. Only used to test whether the volume
/// touches the area; the model keeps the exact description.
pub fn segment_samples(ell: &Ellipsoid, segs: &[SegIn]) -> Result<Vec<LatLon>, GeoError> {
    use crate::model::via;
    let mut out = Vec::new();
    let mut remaining = MAX_BOUNDARY_SAMPLES;
    for (i, s) in segs.iter().enumerate() {
        let next = segs.get((i + 1) % segs.len()).and_then(|n| n.pos);
        match s.via {
            via::CIRCLE => {
                if let Some(c) = s.centre {
                    out.extend(arc_samples(ell, c, s.radius_m, 0.0, 360.0, &mut remaining)?);
                }
            }
            via::ARC_CW | via::ARC_CCW => {
                if let (Some(a), Some(b), Some(c)) = (s.pos, next, s.centre) {
                    let (from, sweep) = arc_sweep(ell, s.via, c, a, b)?;
                    let mut arc = arc_samples(ell, c, s.radius_m, from, sweep, &mut remaining)?;
                    // The stored start point, not the point at the coded radius on its bearing.
                    if let Some(first) = arc.first_mut() {
                        *first = a;
                    }
                    out.extend(arc);
                } else if let Some(a) = s.pos {
                    out.push(a);
                }
            }
            via::RHUMB => {
                if let (Some(a), Some(b)) = (s.pos, next) {
                    out.extend(rhumb_samples(ell, a, b, DENSE_STEP_M, &mut remaining)?);
                } else if let Some(a) = s.pos {
                    out.push(a);
                }
            }
            _ => {
                if let (Some(a), Some(b)) = (s.pos, next) {
                    out.extend(edge_samples_within(
                        ell,
                        a,
                        b,
                        DENSE_STEP_M,
                        &mut remaining,
                    )?);
                } else if let Some(a) = s.pos {
                    out.push(a);
                }
            }
        }
    }
    Ok(out)
}

/// The largest amount, metres, by which an arc's coded radius differs from the distance between
/// its centre and either of its two stored end points; 0 when the boundary has no arc with both
/// ends. The sampler uses the coded radius (what the model stores), so a gap here is a volume
/// whose stored description disagrees with itself.
pub fn arc_radius_gap_m(ell: &Ellipsoid, segs: &[SegIn]) -> Result<f64, GeoError> {
    use crate::model::via;
    let mut worst = 0.0_f64;
    for (i, s) in segs.iter().enumerate() {
        if !matches!(s.via, via::ARC_CW | via::ARC_CCW) {
            continue;
        }
        let next = segs.get((i + 1) % segs.len()).and_then(|n| n.pos);
        if let (Some(a), Some(b), Some(c)) = (s.pos, next, s.centre) {
            for end in [a, b] {
                worst = worst.max((ell.inverse(c, end)?.distance_m - s.radius_m).abs());
            }
        }
    }
    Ok(worst)
}

/// The edges of a closed polyline: each point to the next, and the last back to the first.
/// A single point has one degenerate edge.
fn closed_edges(pts: &[LatLon]) -> Vec<Edge> {
    pts.iter()
        .enumerate()
        .map(|(i, a)| edge_of(*a, pts.get((i + 1) % pts.len()).copied().unwrap_or(*a)))
        .collect()
}

/// True when the short great-circle arcs `p` and `q` share a point. Arcs are far shorter than a
/// quarter turn here: each lies in one plane, and they cross when each one's ends fall on
/// opposite sides of the other's plane and the plane-intersection point nearer to both is
/// between the ends of both. Two arcs on one circle meet when one holds an end of the other.
fn arcs_cross(p: &Edge, q: &Edge) -> bool {
    if norm(p.n3) < 0.5 || norm(q.n3) < 0.5 {
        return false;
    }
    let side = |n: V3, v: V3| dot(n, v);
    let (s1, s2) = (side(q.n3, p.a3), side(q.n3, p.b3));
    let (t1, t2) = (side(p.n3, q.a3), side(p.n3, q.b3));
    if (s1 > 0.0 && s2 > 0.0) || (s1 < 0.0 && s2 < 0.0) {
        return false;
    }
    if (t1 > 0.0 && t2 > 0.0) || (t1 < 0.0 && t2 < 0.0) {
        return false;
    }
    let x = cross(p.n3, q.n3);
    let xn = norm(x);
    if xn < 1e-9 {
        // One circle (or nearly): overlap when an end of either arc is within the other.
        let within = |e: &Edge, v: V3| angle(e.a3, v) + angle(v, e.b3) <= 2.0 * e.half + 1e-10;
        return within(p, q.a3) || within(p, q.b3) || within(q, p.a3) || within(q, p.b3);
    }
    let x = [x[0] / xn, x[1] / xn, x[2] / xn];
    let sign = if dot(x, p.c3) >= 0.0 { 1.0 } else { -1.0 };
    let x = [sign * x[0], sign * x[1], sign * x[2]];
    dot(x, q.c3) > 0.0
}

/// True when `p` is within the area's buffer of the short edge `e`.
fn within_buffer_of_edge(area: &Area, e: &Edge, p: LatLon) -> Result<bool, GeoError> {
    let est = approx_edge_m(e, unit(p));
    let g = grey_m(e, area.buffer_m);
    if est < area.buffer_m - g {
        return Ok(true);
    }
    if est > area.buffer_m + g {
        return Ok(false);
    }
    Ok(exact_edge_m(&area.ell, e.a, e.b, p)? <= area.buffer_m)
}

/// True when the closed polyline `pts` has an edge crossing a (dense) edge of the area, or an
/// edge within the buffer of a vertex of the area. Together with "a point of the polyline is in
/// the area" and "a vertex of the area is inside the polyline" these are all the ways the
/// polyline can touch the buffered area: two edges that do not cross are nearest at an end of
/// one of them.
fn polyline_meets_area(area: &Area, pts: &[LatLon]) -> Result<bool, GeoError> {
    let edges = closed_edges(pts);
    for e in &edges {
        let reach = (e.half + area.dense_half + 1e-7).cos();
        if area
            .dense
            .iter()
            .any(|d| dot(e.c3, d.c3) >= reach && arcs_cross(e, d))
        {
            return Ok(true);
        }
    }
    if area.buffer_m > 0.0 {
        for v in area.rings.iter().flatten() {
            for e in &edges {
                if within_buffer_of_edge(area, e, *v)? {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

/// True when a set of boundary samples touches the area. The samples are the points of a closed
/// outline in order, dense enough that each chord stands for the edge it replaces (see
/// [`segment_samples`] and [`dense_ring_samples`]). The outline touches the area when some
/// sample is in the area, some edge of the outline crosses an edge of the area or comes within
/// the buffer of a vertex of the area, or some vertex of the area lies inside the outline (a
/// volume that encloses the area).
pub fn samples_touch_area(area: &Area, samples: &[LatLon]) -> Result<bool, GeoError> {
    samples_touch_area_within(area, samples, MAX_TOUCH_WORK)
}

/// [`samples_touch_area`] with its work budget given: the edge tests it would make are counted
/// from the sizes first, and a call over `max_work` is refused before any test.
pub fn samples_touch_area_within(
    area: &Area,
    samples: &[LatLon],
    max_work: u64,
) -> Result<bool, GeoError> {
    if samples.is_empty() {
        return Ok(false);
    }
    let per_sample =
        area.edges.len() + area.dense.len() + area.rings.iter().map(Vec::len).sum::<usize>();
    let work = (samples.len() as u64).saturating_mul(per_sample as u64);
    if work > max_work {
        return Err(GeoError::BadParameter(
            "boundary against area tests exceed the work budget",
        ));
    }
    if let Some(b) = area.bbox {
        let (mut lo_la, mut hi_la) = (f64::INFINITY, f64::NEG_INFINITY);
        for s in samples {
            lo_la = lo_la.min(s.lat());
            hi_la = hi_la.max(s.lat());
        }
        // The samples are dense, so their extent stands for the outline's.
        let reference = samples.first().map_or(0.0, |s| s.lon());
        let (lo_lo, hi_lo) =
            lon_range(reference, samples.iter().map(|s| s.lon())).unwrap_or((reference, reference));
        if hi_la < b.min_lat
            || lo_la > b.max_lat
            || !lon_overlaps(lo_lo, hi_lo, b.min_lon, b.max_lon)
        {
            return Ok(false);
        }
    }
    for s in samples {
        if area_contains(area, *s)? {
            return Ok(true);
        }
    }
    // Fewer than three distinct samples enclose nothing; any other failure to form the outline
    // is a fault in the input and goes to the caller (never read as "not touching").
    let outline = match Polygon::new(&area.ell, samples) {
        Ok(o) => Some(o),
        Err(GeoError::BadPolygon("fewer than 3 distinct vertices")) => None,
        Err(e) => return Err(e),
    };
    if polyline_meets_area(area, samples)? {
        return Ok(true);
    }
    if let Some(outline) = outline {
        for ring in &area.rings {
            if ring.iter().any(|v| outline.contains(*v)) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// The closed outline of the one-degree cell with southwest corner (`lat`, `lon`): meridians
/// are geodesics (two points), parallels are sampled [`PARALLEL_STEP_M`] apart.
fn cell_outline(lat: i32, lon: i32) -> Result<Vec<LatLon>, GeoError> {
    let (la, lo) = (f64::from(lat), f64::from(lon));
    let mut out = Vec::new();
    for (row, from, to) in [(la, lo, lo + 1.0), (la + 1.0, lo + 1.0, lo)] {
        let row = row.min(90.0);
        let len_m = 111_000.0 * row.to_radians().cos().abs();
        let n = ((len_m / PARALLEL_STEP_M).ceil() as usize).clamp(1, 400);
        for i in 0..n {
            let t = i as f64 / n as f64;
            out.push(LatLon::new(
                row,
                (from + (to - from) * t).clamp(-180.0, 180.0),
            )?);
        }
    }
    Ok(out)
}

/// True when the one-degree cell with southwest corner (`lat`, `lon`) overlaps the area: a
/// polygon vertex lies in the cell, or the cell's outline (meridians and parallels) has a point
/// in the area, crosses an edge of the area, or comes within the buffer of a vertex of the area.
pub fn cell_overlaps_area(area: &Area, lat: i32, lon: i32) -> Result<bool, GeoError> {
    let (la, lo) = (f64::from(lat), f64::from(lon));
    if let Some(b) = area.bbox {
        if la + 1.0 < b.min_lat
            || la > b.max_lat
            || !lon_overlaps(lo, lo + 1.0, b.min_lon, b.max_lon)
        {
            return Ok(false);
        }
    }
    for ring in &area.rings {
        if ring.iter().any(|v| {
            v.lat() >= la && v.lat() <= la + 1.0 && lon_overlaps(v.lon(), v.lon(), lo, lo + 1.0)
        }) {
            return Ok(true);
        }
    }
    let outline = cell_outline(lat, lon)?;
    for p in &outline {
        if area_contains(area, *p)? {
            return Ok(true);
        }
    }
    polyline_meets_area(area, &outline)
}

/// The largest geodesic-versus-rhumb separation over a ring's edges (closing edge included).
#[derive(Clone, Copy, Debug, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct RingSeparation {
    /// Largest separation, metres.
    pub max_m: f64,
    /// 0-based edge where it occurs: edge `i` joins vertex `i` to vertex `i + 1` (the last edge
    /// joins the last vertex back to the first).
    pub edge: usize,
    /// Fraction along that edge's rhumb line at which it occurs.
    pub at_fraction: f64,
    /// Length of that edge, metres (geodesic).
    pub edge_len_m: f64,
}

/// Maximum geodesic-versus-rhumb separation over the edges of a ring (OA-ASP-005). Edges with
/// coincident endpoints contribute nothing. An edge `oa-geo` cannot evaluate (a pole) is an
/// error.
pub fn edge_separations(ell: &Ellipsoid, ring: &[LatLon]) -> Result<RingSeparation, GeoError> {
    let mut best: Option<RingSeparation> = None;
    for (i, a) in ring.iter().enumerate() {
        let b = ring.get((i + 1) % ring.len()).copied().unwrap_or(*a);
        if *a == b {
            continue;
        }
        let sep = ell.geodesic_rhumb_separation(*a, b, 64)?;
        if best.is_none_or(|x| sep.max_m > x.max_m) {
            best = Some(RingSeparation {
                max_m: sep.max_m,
                edge: i,
                at_fraction: sep.at_fraction,
                edge_len_m: sep.geodesic_m,
            });
        }
    }
    Ok(best.unwrap_or(RingSeparation {
        max_m: 0.0,
        edge: 0,
        at_fraction: 0.0,
        edge_len_m: 0.0,
    }))
}

/// Geodesic distance between two positions, metres.
pub fn distance_m(ell: &Ellipsoid, a: LatLon, b: LatLon) -> Result<f64, GeoError> {
    Ok(ell.inverse(a, b)?.distance_m)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    // REQ: OA-ASP-002
    #[test]
    fn the_spherical_estimate_stays_within_the_scale_error_plus_the_bow_bound_of_the_exact_distance(
    ) {
        let ell = wgs84();
        let (mut worst_ratio, mut worst_bow): (f64, f64) = (0.0, 0.0);
        for lat in [24.0, 35.0, 50.0, 60.0] {
            for az0 in [90.0, 60.0, 30.0, 0.0] {
                for len_nm in [50.0, 100.0, 200.0, 300.0, 450.0, 600.0] {
                    let a = LatLon::new(lat, -100.0).unwrap();
                    let b = ell.direct(a, az0, len_nm * 1852.0).unwrap().position;
                    let e = edge_of(a, b);
                    let inv = ell.inverse(a, b).unwrap();
                    for ti in 1..30 {
                        let t = inv.distance_m * f64::from(ti) / 30.0;
                        let q = ell.direct(a, inv.azimuth1_deg, t).unwrap();
                        for side in [90.0, -90.0] {
                            for d in [200.0, 1000.0, 10_000.0, 30_000.0] {
                                let p = ell
                                    .direct(q.position, q.azimuth2_deg + side, d)
                                    .unwrap()
                                    .position;
                                let ap = approx_edge_m(&e, unit(p));
                                let ex = exact_edge_m(&ell, a, b, p).unwrap();
                                let allowed = SCALE_ERR * ex + e.bow_m;
                                assert!(
                                    (ap - ex).abs() <= allowed,
                                    "lat {lat} az {az0} len {len_nm}: {ap} vs {ex}"
                                );
                                worst_ratio = worst_ratio.max((ap - ex).abs() / allowed);
                                if d < 300.0 {
                                    worst_bow = worst_bow.max((ap - ex).abs() / e.bow_m);
                                }
                            }
                        }
                    }
                }
            }
        }
        // The bound is not slack by more than the margin built into it.
        assert!(
            worst_ratio > 0.5 && worst_bow > 0.5,
            "{worst_ratio} {worst_bow}"
        );
    }
}
