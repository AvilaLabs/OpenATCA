//! Geometry the selection and the report need, on top of `oa-geo`. **Core** (B1): pure functions
//! of coordinates.
//!
//! * [`Area`]: the selection area, the union of the ARTCC's HIGH and LOW polygons and a buffer
//!   around their edges (OA-ASP-002). A point is in the area when it is inside either polygon
//!   or its geodesic distance to the nearest polygon edge is at most the buffer.
//! * The nearest-edge distance is found in two steps so the build stays fast: a spherical
//!   estimate per edge (cross-track on a sphere of the mean radius, off by at most about 0.4 %
//!   from the ellipsoid) decides every point that is not within 1 % of the buffer; only the
//!   rest are measured exactly (geodesic inverse and direct problems along the edge).
//! * [`edge_separations`]: per boundary, the largest geodesic-versus-rhumb separation over its
//!   edges (OA-ASP-005).
//! * Sampling helpers turn boundary descriptions (vertices, arcs, circles) into point lists
//!   used only for the "does it touch the area" tests; the model keeps the exact description.
//!
//! Coordinates enter as the model's 1e-7 degree integers, so selection is a function of the
//! values the snapshot stores.

use oa_geo::{wgs84, Ellipsoid, GeoError, LatLon, Polygon};

/// Degrees per model unit.
pub const DEG_PER_E7: f64 = 1e-7;
/// Mean earth radius used by the spherical estimate, metres.
const R_MEAN_M: f64 = 6_371_008.8;
/// Relative margin around the buffer inside which the exact distance decides: the spherical
/// estimate is within 0.4 % of the ellipsoidal distance, so 1 % is safe.
const GREY: f64 = 0.01;
/// Spacing of the points sampled along a long edge for intersection tests, metres.
pub const SAMPLE_STEP_M: f64 = 20_000.0;

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
struct Edge {
    a: LatLon,
    b: LatLon,
    a3: V3,
    b3: V3,
    /// Unit normal of the great-circle plane through the edge; zero for a degenerate edge.
    n3: V3,
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
#[derive(Clone, Copy, Debug)]
pub struct Bbox {
    pub min_lat: f64,
    pub max_lat: f64,
    pub min_lon: f64,
    pub max_lon: f64,
}

/// The selection area: polygons, the buffer, and prepared edges.
#[derive(Clone, Debug)]
pub struct Area {
    pub ell: Ellipsoid,
    polys: Vec<Polygon>,
    /// The rings the polygons were built from (their vertices, for containment tests).
    pub rings: Vec<Vec<LatLon>>,
    edges: Vec<Edge>,
    pub buffer_m: f64,
    /// Bounding box of the area including the buffer; `None` when the rings are too wide for a
    /// box to be a safe filter (antimeridian, poles).
    pub bbox: Option<Bbox>,
}

/// Builds the area from rings of vertices. Each ring must be a valid geodesic polygon for
/// `oa-geo`; the error says which limit it broke.
pub fn build_area(rings: Vec<Vec<LatLon>>, buffer_m: f64) -> Result<Area, GeoError> {
    let ell = wgs84();
    let mut polys = Vec::with_capacity(rings.len());
    let mut edges = Vec::new();
    let (mut min_lat, mut max_lat) = (f64::INFINITY, f64::NEG_INFINITY);
    let (mut min_lon, mut max_lon) = (f64::INFINITY, f64::NEG_INFINITY);
    for ring in &rings {
        polys.push(Polygon::new(&ell, ring)?);
        for (i, a) in ring.iter().enumerate() {
            let b = ring.get((i + 1) % ring.len()).copied().unwrap_or(*a);
            let (a3, b3) = (unit(*a), unit(b));
            let c = cross(a3, b3);
            let n = norm(c);
            let n3 = if n > 1e-12 {
                [c[0] / n, c[1] / n, c[2] / n]
            } else {
                [0.0; 3]
            };
            edges.push(Edge {
                a: *a,
                b,
                a3,
                b3,
                n3,
            });
            min_lat = min_lat.min(a.lat());
            max_lat = max_lat.max(a.lat());
            min_lon = min_lon.min(a.lon());
            max_lon = max_lon.max(a.lon());
        }
    }
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
        buffer_m,
        bbox,
    })
}

fn in_bbox(area: &Area, p: LatLon) -> bool {
    match area.bbox {
        Some(b) => {
            p.lat() >= b.min_lat
                && p.lat() <= b.max_lat
                && p.lon() >= b.min_lon
                && p.lon() <= b.max_lon
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
    let nearest = approx.iter().copied().fold(f64::INFINITY, f64::min);
    if nearest < area.buffer_m * (1.0 - GREY) {
        return Ok(true);
    }
    if nearest > area.buffer_m * (1.0 + GREY) {
        return Ok(false);
    }
    // Within 1 % of the buffer: measure the edges that could be that close exactly.
    for (e, est) in area.edges.iter().zip(&approx) {
        if *est <= area.buffer_m * (1.0 + 2.0 * GREY)
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
    let inv = ell.inverse(a, b)?;
    let n = ((inv.distance_m / step_m).ceil() as usize).clamp(1, 10_000);
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
    let mut out = Vec::new();
    for (i, a) in ring.iter().enumerate() {
        let b = ring.get((i + 1) % ring.len()).copied().unwrap_or(*a);
        out.extend(edge_samples(ell, *a, b, SAMPLE_STEP_M)?);
    }
    Ok(out)
}

/// A boundary record as the sampler needs it: the via code, the point (none for a circle), the
/// arc or circle centre and radius in metres.
#[derive(Clone, Copy, Debug)]
pub struct SegIn {
    pub via: u8,
    pub pos: Option<LatLon>,
    pub centre: Option<LatLon>,
    pub radius_m: f64,
}

fn arc_samples(
    ell: &Ellipsoid,
    centre: LatLon,
    radius_m: f64,
    from_deg: f64,
    sweep_deg: f64,
) -> Result<Vec<LatLon>, GeoError> {
    let n = ((sweep_deg.abs() / 5.0).ceil() as usize).clamp(1, 72);
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let brg = from_deg + sweep_deg * i as f64 / n as f64;
        out.push(ell.direct(centre, brg, radius_m)?.position);
    }
    Ok(out)
}

/// Samples of an ARINC 424 style boundary: geodesic and rhumb edges are sampled as geodesics
/// (the difference is far below the sample spacing), arcs and circles around their centres.
/// Only used to test whether the volume touches the area.
pub fn segment_samples(ell: &Ellipsoid, segs: &[SegIn]) -> Result<Vec<LatLon>, GeoError> {
    use crate::model::via;
    let mut out = Vec::new();
    for (i, s) in segs.iter().enumerate() {
        let next = segs.get((i + 1) % segs.len()).and_then(|n| n.pos);
        match s.via {
            via::CIRCLE => {
                if let Some(c) = s.centre {
                    out.extend(arc_samples(ell, c, s.radius_m, 0.0, 360.0)?);
                }
            }
            via::ARC_CW | via::ARC_CCW => {
                if let (Some(a), Some(b), Some(c)) = (s.pos, next, s.centre) {
                    let from = ell.inverse(c, a)?.azimuth1_deg;
                    let to = ell.inverse(c, b)?.azimuth1_deg;
                    let cw = (to - from).rem_euclid(360.0);
                    let sweep = if s.via == via::ARC_CW { cw } else { cw - 360.0 };
                    let r = ell.inverse(c, a)?.distance_m;
                    out.extend(arc_samples(ell, c, r, from, sweep)?);
                } else if let Some(a) = s.pos {
                    out.push(a);
                }
            }
            _ => {
                if let (Some(a), Some(b)) = (s.pos, next) {
                    out.extend(edge_samples(ell, a, b, SAMPLE_STEP_M)?);
                } else if let Some(a) = s.pos {
                    out.push(a);
                }
            }
        }
    }
    Ok(out)
}

/// True when a set of boundary samples touches the area: some sample is in the area, or some
/// vertex of the area lies inside the sampled outline (a volume that encloses the area).
pub fn samples_touch_area(area: &Area, samples: &[LatLon]) -> Result<bool, GeoError> {
    if samples.is_empty() {
        return Ok(false);
    }
    if let Some(b) = area.bbox {
        let (mut lo_la, mut hi_la, mut lo_lo, mut hi_lo) = (
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
        );
        for s in samples {
            lo_la = lo_la.min(s.lat());
            hi_la = hi_la.max(s.lat());
            lo_lo = lo_lo.min(s.lon());
            hi_lo = hi_lo.max(s.lon());
        }
        if hi_la < b.min_lat || lo_la > b.max_lat || hi_lo < b.min_lon || lo_lo > b.max_lon {
            return Ok(false);
        }
    }
    for s in samples {
        if area_contains(area, *s)? {
            return Ok(true);
        }
    }
    if let Ok(outline) = Polygon::new(&area.ell, samples) {
        for ring in &area.rings {
            if ring.iter().any(|v| outline.contains(*v)) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// True when the one-degree cell with southwest corner (`lat`, `lon`) overlaps the area: any
/// of 81 sample points of the cell is in the area, or a polygon vertex lies in the cell.
pub fn cell_overlaps_area(area: &Area, lat: i32, lon: i32) -> Result<bool, GeoError> {
    let (la, lo) = (f64::from(lat), f64::from(lon));
    if let Some(b) = area.bbox {
        if la + 1.0 < b.min_lat || la > b.max_lat || lo + 1.0 < b.min_lon || lo > b.max_lon {
            return Ok(false);
        }
    }
    for ring in &area.rings {
        if ring
            .iter()
            .any(|v| v.lat() >= la && v.lat() <= la + 1.0 && v.lon() >= lo && v.lon() <= lo + 1.0)
        {
            return Ok(true);
        }
    }
    for i in 0..=8 {
        for j in 0..=8 {
            let p = LatLon::new(
                (la + f64::from(i) / 8.0).min(90.0),
                (lo + f64::from(j) / 8.0).min(180.0),
            )?;
            if area_contains(area, p)? {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// The largest geodesic-versus-rhumb separation over a ring's edges (closing edge included).
#[derive(Clone, Copy, Debug, PartialEq)]
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
