//! Warning-grade (conservative) terrain maxima. Core (pure).
//!
//! The plain queries in [`crate::grid`] count only samples whose centre lies inside the area, so
//! a peak in a cell the area clips but whose centre is outside is missed, and an area smaller
//! than one cell can return nothing. The queries here are for the minimum safe altitude warning:
//! they never return less than the true maximum of the gridded surface inside the area.
//!
//! # Footprint and the never-low argument
//!
//! A [`Grid`] stores the position of the sample centres whatever the raster type (`PixelIsArea`
//! or `PixelIsPoint`, see the crate docs). The surface between samples is not defined by the
//! file; the crate's own interpolation is bilinear, and a warning must not depend on that choice.
//! Every interpolation in use (nearest, bilinear, bicubic without overshoot, any convex
//! combination of the surrounding samples) gives, at a point inside a lattice quad (the cell
//! between four adjacent sample centres), a value that is at most the largest of the quad's four
//! corner samples. So the true maximum inside an area is at most the largest corner sample of
//! any quad the area touches.
//!
//! The footprint of a sample is therefore the union of the four quads that share it: the square
//! of half-width one full step (in latitude and in longitude) around its centre. A sample counts
//! when this closed square touches the area. This is a superset of the sample's own pixel for
//! `PixelIsArea` (half a step each way), so a pixel-constant reading of the file is covered too,
//! and the same footprint serves both raster types. The price is over-inclusion: a sample counts
//! when the area comes within one step of its centre. At 1 arc-second that is about 30 m
//! in latitude and 24 m to 30 m in longitude at the latitudes of the study area.
//!
//! # Radius on the ellipsoid
//!
//! The radius is a WGS 84 geodesic distance. A path of length `R` from the centre stays within
//! `R / M0` radians of latitude (`M0`, the meridian radius of curvature at the equator, is the
//! smallest anywhere). Over that latitude band the line element satisfies
//! `ds^2 = M^2 dphi^2 + (N cos phi)^2 dlambda^2 >= (m dphi)^2 + (n dlambda)^2`, with `m` the
//! smallest `M` (at the band's lowest `|latitude|`, `M` grows poleward) and `n` the smallest
//! `N cos phi` (at the band's highest `|latitude|`). Straight lines are shortest in a flat
//! metric, so the geodesic distance from the centre to any point is at least
//! `sqrt((m dphi)^2 + (n dlambda)^2)`: a sample whose footprint contains a point within the
//! radius always passes the test, which never under-includes. The test is run with a relative
//! slack of 1e-9 against rounding.
//!
//! Over-inclusion is bounded: the straight line in the latitude/longitude plane is a path whose
//! length is at most `k` times the lower bound, `k = max(M_max / m, (N cos phi)_max / n)` over
//! the same band, so a sample counts only if its footprint comes within `k * R` of the centre.
//! `k` is below 1.001 for radii of 10 nm or less at latitudes up to 50 degrees (the tests state
//! the figure for 5 nm at 41 degrees). Together with the footprint this means a counted sample
//! is within `k * R` plus one step diagonal of the centre.
//!
//! # Completeness
//!
//! Every sample whose footprint touches the area is looked up in the tile set, on the lattice of
//! the first tile (all tiles must share it, otherwise [`QueryError::TilesNotAligned`]). A sample
//! no tile holds is missing, a sample every holding tile reports as NaN is nodata. Either makes
//! the answer [`TerrainMax::Incomplete`], which carries the counts and the highest valid sample
//! found so far as a lower bound only. The area reaching beyond the outermost sample centres of
//! the data counts as missing samples, exactly as [`crate::grid_elevation_at`] reports such a
//! point as outside. Where two tiles overlap, a valid sample beats a nodata one and the higher of
//! two valid samples is taken (the conservative choice).
//!
//! # Longitude
//!
//! The antimeridian and the poles are out of scope: a circle whose longitude band leaves
//! `[-180, 180]` or that reaches a pole is refused with [`QueryError::LongitudeWraps`]. This
//! refuses a circle that comes close to the antimeridian without crossing it (the band is an
//! upper bound). A polygon is a plane figure in longitude/latitude as in
//! [`crate::grid_max_in_polygon`] and a span over 180 degrees is refused as a bad polygon.

use crate::grid::{check_polygon, grid_sample, in_polygon, valid_coord};
use crate::METRES_PER_NM;
use crate::{Grid, Peak, QueryError, TileSet, MAX_QUERY_SAMPLES, MAX_RADIUS_NM, MAX_TILES};
use std::collections::BTreeSet;

/// WGS 84 semi-major axis, metres.
const WGS84_A: f64 = 6_378_137.0;
/// WGS 84 first eccentricity squared.
const WGS84_E2: f64 = 0.006_694_379_990_141_316;
/// Meridian radius of curvature at the equator, the smallest anywhere, metres.
const MERIDIAN_RADIUS_MIN_M: f64 = 6_335_439.0;
/// Relative slack on the radius and on footprints against rounding.
const SLACK: f64 = 1e-9;
/// Tolerance for two tile lattices to be the same, in steps.
const ALIGN_TOL: f64 = 1e-3;

/// A conservative terrain maximum. Never a bare number: it says whether the answer is complete.
/// Plain data (A1).
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, PartialEq)]
pub enum TerrainMax {
    /// Every sample whose footprint touches the area was found and valid; `peak` is at least the
    /// true maximum of the gridded surface in the area under any interpolation.
    Complete {
        /// The highest sample, metres and position.
        peak: Peak,
        /// Valid samples counted.
        nodes: u64,
    },
    /// Some sample was nodata or missing, so the true maximum may be higher than `peak`, which is
    /// a lower bound only (`None` when no valid sample was found).
    Incomplete {
        /// The highest valid sample found, a lower bound on the answer.
        peak: Option<Peak>,
        /// Valid samples counted.
        nodes: u64,
        /// Samples touched whose value is nodata in every tile that holds them.
        nodata_nodes: u64,
        /// Samples touched that no tile holds (the area leaves the loaded data).
        missing_nodes: u64,
        /// Distinct whole-degree squares (by floor of latitude and longitude) that contain
        /// missing samples; the tiles to load to complete the answer.
        missing_squares: u32,
    },
}

/// Running state of a conservative query.
struct Found {
    peak: Option<Peak>,
    nodes: u64,
    nodata: u64,
    missing: u64,
    squares: BTreeSet<(i32, i32)>,
    last_square: Option<(i32, i32)>,
}

/// One tile with its offset on the reference lattice.
struct Placed<'a> {
    g: &'a Grid,
    col_off: i64,
    row_off: i64,
}

/// Meridian radius of curvature at geodetic latitude `phi` (radians), metres.
fn meridian_radius(phi: f64) -> f64 {
    let s = phi.sin();
    WGS84_A * (1.0 - WGS84_E2) / (1.0 - WGS84_E2 * s * s).powf(1.5)
}

/// Prime vertical radius of curvature at geodetic latitude `phi` (radians), metres.
fn prime_vertical_radius(phi: f64) -> f64 {
    let s = phi.sin();
    WGS84_A / (1.0 - WGS84_E2 * s * s).sqrt()
}

/// Checks that every tile shares the lattice of the first and returns the tile offsets in steps.
fn place(tiles: &[Grid]) -> Result<Vec<Placed<'_>>, QueryError> {
    let r = tiles.first().ok_or(QueryError::NoTiles)?;
    let mut out = Vec::with_capacity(tiles.len());
    for g in tiles {
        if (g.dlon - r.dlon).abs() > SLACK * r.dlon || (g.dlat - r.dlat).abs() > SLACK * r.dlat {
            return Err(QueryError::TilesNotAligned);
        }
        let fc = (g.lon0 - r.lon0) / r.dlon;
        let fr = (r.lat0 - g.lat0) / r.dlat;
        if !(fc.is_finite() && fr.is_finite())
            || (fc - fc.round()).abs() > ALIGN_TOL
            || (fr - fr.round()).abs() > ALIGN_TOL
            || fc.abs() > 1e9
            || fr.abs() > 1e9
        {
            return Err(QueryError::TilesNotAligned);
        }
        out.push(Placed {
            g,
            col_off: fc.round() as i64,
            row_off: fr.round() as i64,
        });
    }
    Ok(out)
}

/// Visits every sample of the reference lattice inside the bounding box (widened by one step)
/// that `hit` accepts, and looks each up across the tiles.
fn collect(
    tiles: &[Grid],
    bbox: (f64, f64, f64, f64),
    hit: &dyn Fn(f64, f64) -> bool,
) -> Result<TerrainMax, QueryError> {
    let placed = place(tiles)?;
    let r = tiles.first().ok_or(QueryError::NoTiles)?;
    let (la0, la1, lo0, lo1) = bbox;
    let c_lo = ((lo0 - r.lon0) / r.dlon).floor() - 1.0;
    let c_hi = ((lo1 - r.lon0) / r.dlon).ceil() + 1.0;
    let r_lo = ((r.lat0 - la1) / r.dlat).floor() - 1.0;
    let r_hi = ((r.lat0 - la0) / r.dlat).ceil() + 1.0;
    let cells = (c_hi - c_lo + 1.0) * (r_hi - r_lo + 1.0);
    if !(cells.is_finite() && cells <= MAX_QUERY_SAMPLES as f64) {
        return Err(QueryError::TooManySamples);
    }
    let (c_lo, c_hi, r_lo, r_hi) = (c_lo as i64, c_hi as i64, r_lo as i64, r_hi as i64);
    // Only tiles that reach the node range are looked at.
    let cands: Vec<&Placed<'_>> = placed
        .iter()
        .filter(|p| {
            p.col_off <= c_hi
                && p.col_off + i64::from(p.g.width) > c_lo
                && p.row_off <= r_hi
                && p.row_off + i64::from(p.g.height) > r_lo
        })
        .collect();
    let mut f = Found {
        peak: None,
        nodes: 0,
        nodata: 0,
        missing: 0,
        squares: BTreeSet::new(),
        last_square: None,
    };
    for row in r_lo..=r_hi {
        let lat = r.lat0 - row as f64 * r.dlat;
        for col in c_lo..=c_hi {
            let lon = r.lon0 + col as f64 * r.dlon;
            if !hit(lat, lon) {
                continue;
            }
            let mut present = false;
            let mut best: Option<f32> = None;
            for p in &cands {
                let (Ok(c), Ok(rw)) = (
                    u32::try_from(col - p.col_off),
                    u32::try_from(row - p.row_off),
                ) else {
                    continue;
                };
                if let Some(v) = grid_sample(p.g, c, rw) {
                    present = true;
                    if !v.is_nan() && best.is_none_or(|b| v > b) {
                        best = Some(v);
                    }
                }
            }
            match (present, best) {
                (false, _) => {
                    f.missing += 1;
                    let sq = (lat.floor() as i32, lon.floor() as i32);
                    if f.last_square != Some(sq) {
                        f.squares.insert(sq);
                        f.last_square = Some(sq);
                    }
                }
                (true, None) => f.nodata += 1,
                (true, Some(v)) => {
                    f.nodes += 1;
                    if f.peak.is_none_or(|p| v > p.metres) {
                        f.peak = Some(Peak {
                            metres: v,
                            lat,
                            lon,
                        });
                    }
                }
            }
        }
    }
    Ok(match f.peak {
        Some(peak) if f.nodata == 0 && f.missing == 0 => TerrainMax::Complete {
            peak,
            nodes: f.nodes,
        },
        peak => TerrainMax::Incomplete {
            peak,
            nodes: f.nodes,
            nodata_nodes: f.nodata,
            missing_nodes: f.missing,
            missing_squares: u32::try_from(f.squares.len()).unwrap_or(u32::MAX),
        },
    })
}

/// The conservative maximum of one grid within `radius_nm` (WGS 84 geodesic) of a point. See the
/// module docs for the footprint, the never-low argument and the over-inclusion bound.
pub fn grid_max_in_radius_conservative(
    g: &Grid,
    lat: f64,
    lon: f64,
    radius_nm: f64,
) -> Result<TerrainMax, QueryError> {
    radius_conservative(std::slice::from_ref(g), lat, lon, radius_nm)
}

/// The conservative maximum of a tile set within `radius_nm` of a point; the tiles must share one
/// lattice. See [`grid_max_in_radius_conservative`].
pub fn max_elevation_in_radius_conservative(
    set: &TileSet,
    lat: f64,
    lon: f64,
    radius_nm: f64,
) -> Result<TerrainMax, QueryError> {
    if set.tiles.len() > MAX_TILES {
        return Err(QueryError::TooManyTiles);
    }
    radius_conservative(&set.tiles, lat, lon, radius_nm)
}

fn radius_conservative(
    tiles: &[Grid],
    lat: f64,
    lon: f64,
    radius_nm: f64,
) -> Result<TerrainMax, QueryError> {
    if !valid_coord(lat, lon) {
        return Err(QueryError::BadCoordinate);
    }
    if !(radius_nm.is_finite() && radius_nm > 0.0 && radius_nm <= MAX_RADIUS_NM) {
        return Err(QueryError::BadRadius);
    }
    let r_eff = radius_nm * METRES_PER_NM * (1.0 + SLACK);
    let g0 = tiles.first().ok_or(QueryError::NoTiles)?;
    // The latitude band any path of length r_eff stays in, then the smallest curvature radii in it.
    let band = (r_eff / MERIDIAN_RADIUS_MIN_M).to_degrees();
    let (lo, hi) = (lat - band, lat + band);
    if lo <= -90.0 || hi >= 90.0 {
        return Err(QueryError::LongitudeWraps);
    }
    let min_abs = if lo <= 0.0 && hi >= 0.0 {
        0.0
    } else {
        lo.abs().min(hi.abs())
    };
    let max_abs = lo.abs().max(hi.abs()).to_radians();
    let m = meridian_radius(min_abs.to_radians());
    let n = prime_vertical_radius(max_abs) * max_abs.cos();
    let dlat_box = (r_eff / m).to_degrees();
    let dlon_box = (r_eff / n).to_degrees();
    if !(dlon_box.is_finite() && lon - dlon_box >= -180.0 && lon + dlon_box <= 180.0) {
        return Err(QueryError::LongitudeWraps);
    }
    let (sdlat, sdlon) = (g0.dlat, g0.dlon);
    let hit = move |la: f64, lo: f64| {
        let dphi = ((la - lat).abs() - sdlat * (1.0 + SLACK))
            .max(0.0)
            .to_radians();
        let dlam = ((lo - lon).abs() - sdlon * (1.0 + SLACK))
            .max(0.0)
            .to_radians();
        let (x, y) = (m * dphi, n * dlam);
        x * x + y * y <= r_eff * r_eff
    };
    collect(
        tiles,
        (
            lat - dlat_box,
            lat + dlat_box,
            lon - dlon_box,
            lon + dlon_box,
        ),
        &hit,
    )
}

/// The conservative maximum of one grid inside a polygon of (latitude, longitude) vertices,
/// tested in the latitude/longitude plane. See the module docs.
pub fn grid_max_in_polygon_conservative(
    g: &Grid,
    poly: &[(f64, f64)],
) -> Result<TerrainMax, QueryError> {
    polygon_conservative(std::slice::from_ref(g), poly)
}

/// The conservative maximum of a tile set inside a polygon; the tiles must share one lattice.
pub fn max_elevation_in_polygon_conservative(
    set: &TileSet,
    poly: &[(f64, f64)],
) -> Result<TerrainMax, QueryError> {
    if set.tiles.len() > MAX_TILES {
        return Err(QueryError::TooManyTiles);
    }
    polygon_conservative(&set.tiles, poly)
}

fn polygon_conservative(tiles: &[Grid], poly: &[(f64, f64)]) -> Result<TerrainMax, QueryError> {
    let bbox = check_polygon(poly)?;
    let g0 = tiles.first().ok_or(QueryError::NoTiles)?;
    let (sdlat, sdlon) = (g0.dlat * (1.0 + SLACK), g0.dlon * (1.0 + SLACK));
    let hit = |la: f64, lo: f64| {
        rect_touches_polygon(poly, la - sdlat, la + sdlat, lo - sdlon, lo + sdlon)
    };
    collect(tiles, bbox, &hit)
}

/// True when the closed rectangle (latitude `y0..=y1`, longitude `x0..=x1`) shares a point with
/// the polygon region: an edge touches it, or it lies wholly inside the polygon.
fn rect_touches_polygon(poly: &[(f64, f64)], y0: f64, y1: f64, x0: f64, x1: f64) -> bool {
    let mut prev = poly.last().copied();
    for &q in poly {
        if let Some(p) = prev {
            if segment_touches_rect(p, q, y0, y1, x0, x1) {
                return true;
            }
        }
        prev = Some(q);
    }
    // No edge reaches the rectangle: it is wholly inside or wholly outside.
    in_polygon(poly, (y0 + y1) / 2.0, (x0 + x1) / 2.0)
}

/// Liang-Barsky: does the segment from `a` to `b` (latitude, longitude) touch the closed
/// rectangle?
fn segment_touches_rect(a: (f64, f64), b: (f64, f64), y0: f64, y1: f64, x0: f64, x1: f64) -> bool {
    let (ay, ax) = a;
    let (dy, dx) = (b.0 - ay, b.1 - ax);
    let (mut t0, mut t1) = (0.0_f64, 1.0_f64);
    for (p, q) in [(-dx, ax - x0), (dx, x1 - ax), (-dy, ay - y0), (dy, y1 - ay)] {
        if p == 0.0 {
            if q < 0.0 {
                return false;
            }
        } else {
            let t = q / p;
            if p < 0.0 {
                if t > t1 {
                    return false;
                }
                t0 = t0.max(t);
            } else {
                if t < t0 {
                    return false;
                }
                t1 = t1.min(t);
            }
        }
    }
    true
}
