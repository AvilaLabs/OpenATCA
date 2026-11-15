//! Elevation grids and the terrain queries. Core (pure).
//!
//! A [`Grid`] is a regular latitude/longitude lattice of elevation samples in metres; nodata is
//! stored as NaN. A [`TileSet`] is several grids (1 degree tiles) queried as one.
//!
//! Distances in the radius query are great-circle on a sphere of radius 6,371,008.8 m (haversine),
//! which is within 0.5% of the ellipsoidal distance; the radius is a terrain buffer, not a
//! measurement. Polygons are tested in the latitude/longitude plane (even-odd), which is exact
//! for the small buffers they are used for; a polygon spanning more than 180 degrees of
//! longitude is refused (it would cross the antimeridian).

use crate::{MAX_POLYGON_VERTICES, MAX_QUERY_SAMPLES, MAX_RADIUS_NM, MAX_TILES, METRES_PER_NM};

const EARTH_RADIUS_M: f64 = 6_371_008.8;
/// Tolerance for a point sitting on the edge of a grid, in fractions of a pixel.
const EDGE_EPS: f64 = 1e-6;

/// An elevation lattice. Plain data (A1); the sample array is one `Vec` because a grid has
/// exactly one field per node (A3 is met by construction).
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, PartialEq)]
pub struct Grid {
    /// Columns.
    pub width: u32,
    /// Rows.
    pub height: u32,
    /// Longitude of the centre of column 0, degrees.
    pub lon0: f64,
    /// Latitude of the centre of row 0 (the northern row), degrees.
    pub lat0: f64,
    /// Longitude step per column, degrees, positive (columns run east).
    pub dlon: f64,
    /// Latitude step per row, degrees, positive (rows run south).
    pub dlat: f64,
    /// Elevation in metres, row-major from the north-west; NaN is nodata.
    pub elev: Vec<f32>,
    /// Number of nodata samples.
    pub nodata_count: u64,
    /// The file was `PixelIsPoint`.
    pub point_registered: bool,
    /// EPSG geographic CRS code (4269 or 4326).
    pub crs: u16,
    /// EPSG vertical CS code if the file named one.
    pub vertical_cs: Option<u16>,
}

/// A set of grids queried as one. Plain data (A1).
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TileSet {
    /// The tiles; they may overlap (3DEP tiles carry a border shared with the neighbours).
    pub tiles: Vec<Grid>,
}

/// The result of an elevation lookup.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Elevation {
    /// Interpolated elevation, metres.
    Metres(f32),
    /// The point is covered but a sample it needs is nodata; no value is invented.
    NoData,
    /// No tile covers the point.
    Outside,
}

/// The highest sample found by an extreme query. Plain data (A1).
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Peak {
    /// Elevation, metres.
    pub metres: f32,
    /// Latitude of the sample, degrees.
    pub lat: f64,
    /// Longitude of the sample, degrees.
    pub lon: f64,
}

/// What a max-elevation query saw. Plain data (A1).
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MaxQuery {
    /// The highest valid sample inside the region, if any.
    pub peak: Option<Peak>,
    /// Valid samples visited (a sample in the overlap of two tiles is visited once per tile).
    pub samples: u64,
    /// Nodata samples visited inside the region.
    pub nodata: u64,
    /// Every probe point (centre, boundary samples, polygon vertices and edge midpoints) lies in
    /// some tile. False means part of the region is likely not covered, so the maximum may be low.
    pub probes_covered: bool,
}

/// A query was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueryError {
    /// Latitude or longitude is not finite or outside its range.
    BadCoordinate,
    /// The radius is not finite, not positive or over [`MAX_RADIUS_NM`].
    BadRadius,
    /// The polygon has fewer than 3 or more than [`MAX_POLYGON_VERTICES`] vertices, or spans more
    /// than 180 degrees of longitude.
    BadPolygon,
    /// More tiles than [`MAX_TILES`].
    TooManyTiles,
    /// The region would visit more than [`MAX_QUERY_SAMPLES`] samples.
    TooManySamples,
    /// The cells times the polygon vertices, plus the tiles, are over [`MAX_QUERY_WORK`].
    TooMuchWork,
    /// A conservative circle reaches a pole or would cross the antimeridian, where longitude is
    /// not a plane; the query is refused rather than answered from one side only.
    LongitudeWraps,
    /// A conservative query was given a tile set with no tiles.
    NoTiles,
    /// A conservative query was given tiles that do not share one lattice (spacing equal to a
    /// part in 1e9, positions a whole number of steps apart to a thousandth of a step).
    TilesNotAligned,
}

impl std::fmt::Display for QueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            QueryError::BadCoordinate => write!(f, "latitude/longitude not finite or out of range"),
            QueryError::BadRadius => write!(f, "radius not positive, finite and within the limit"),
            QueryError::BadPolygon => write!(f, "polygon needs 3 to {MAX_POLYGON_VERTICES} vertices within 180 degrees of longitude"),
            QueryError::TooManyTiles => write!(f, "more than {MAX_TILES} tiles"),
            QueryError::TooManySamples => write!(f, "query region too large"),
            QueryError::TooMuchWork => write!(f, "query would do too much work"),
            QueryError::LongitudeWraps => write!(f, "area reaches a pole or crosses the antimeridian"),
            QueryError::NoTiles => write!(f, "tile set is empty"),
            QueryError::TilesNotAligned => write!(f, "tiles do not share one sample lattice"),
        }
    }
}

impl std::error::Error for QueryError {}

/// The sample at (`col`, `row`); NaN is nodata, and a non-finite sample reads as NaN. `None`
/// outside the grid.
pub fn grid_sample(g: &Grid, col: u32, row: u32) -> Option<f32> {
    if col >= g.width || row >= g.height {
        return None;
    }
    g.elev
        .get(row as usize * g.width as usize + col as usize)
        .map(|&v| if v.is_finite() { v } else { f32::NAN })
}

pub(crate) fn valid_coord(lat: f64, lon: f64) -> bool {
    lat.is_finite()
        && lon.is_finite()
        && (-90.0..=90.0).contains(&lat)
        && (-180.0..=180.0).contains(&lon)
}

/// Fractional (column, row) of a point.
fn frac_index(g: &Grid, lat: f64, lon: f64) -> (f64, f64) {
    ((lon - g.lon0) / g.dlon, (g.lat0 - lat) / g.dlat)
}

/// True when the point lies within the sample lattice (edges inclusive, with a tiny tolerance).
fn in_lattice(g: &Grid, fx: f64, fy: f64) -> bool {
    fx >= -EDGE_EPS
        && fy >= -EDGE_EPS
        && fx <= f64::from(g.width - 1) + EDGE_EPS
        && fy <= f64::from(g.height - 1) + EDGE_EPS
}

/// Bilinear elevation at a point inside one grid. [`Elevation::Outside`] when the point is beyond
/// the outermost sample centres; [`Elevation::NoData`] when any of the four surrounding samples
/// is nodata (a partial average would hide the gap, RULESET E5).
pub fn grid_elevation_at(g: &Grid, lat: f64, lon: f64) -> Elevation {
    let e = grid_elevation_here(g, lat, lon);
    match other_side(e == Elevation::Outside, lon) {
        Some(alias) => grid_elevation_here(g, lat, alias),
        None => e,
    }
}

/// The other name of the meridian at exactly +180 or -180 degrees, when `outside` says the
/// first name found nothing.
fn other_side(outside: bool, lon: f64) -> Option<f64> {
    (outside && lon.abs() == 180.0).then_some(-lon)
}

fn grid_elevation_here(g: &Grid, lat: f64, lon: f64) -> Elevation {
    if !valid_coord(lat, lon) {
        return Elevation::Outside;
    }
    let (fx, fy) = frac_index(g, lat, lon);
    if !in_lattice(g, fx, fy) {
        return Elevation::Outside;
    }
    let fx = fx.clamp(0.0, f64::from(g.width - 1));
    let fy = fy.clamp(0.0, f64::from(g.height - 1));
    let c0 = (fx.floor() as u32).min(g.width - 2);
    let r0 = (fy.floor() as u32).min(g.height - 2);
    let tx = fx - f64::from(c0);
    let ty = fy - f64::from(r0);
    let corner = |dc: u32, dr: u32| grid_sample(g, c0 + dc, r0 + dr);
    match (corner(0, 0), corner(1, 0), corner(0, 1), corner(1, 1)) {
        (Some(a), Some(b), Some(c), Some(d))
            if a.is_finite() && b.is_finite() && c.is_finite() && d.is_finite() =>
        {
            let top = f64::from(a) * (1.0 - tx) + f64::from(b) * tx;
            let bot = f64::from(c) * (1.0 - tx) + f64::from(d) * tx;
            Elevation::Metres((top * (1.0 - ty) + bot * ty) as f32)
        }
        _ => Elevation::NoData,
    }
}

/// The node value at a latitude/longitude that must coincide with a sample centre to within
/// [`EDGE_EPS`] of a pixel; `None` if it does not coincide or is outside the grid.
fn node_value(g: &Grid, lat: f64, lon: f64) -> Option<f32> {
    let (fx, fy) = frac_index(g, lat, lon);
    if !in_lattice(g, fx, fy) {
        return None;
    }
    let (rx, ry) = (fx.round(), fy.round());
    if (fx - rx).abs() > 1e-3 || (fy - ry).abs() > 1e-3 {
        return None;
    }
    grid_sample(g, rx.max(0.0) as u32, ry.max(0.0) as u32)
}

/// True when some tile gives the point an answer, value or nodata (so a point in a one-pixel gap
/// between tiles that line up is covered).
pub fn tileset_covers(set: &TileSet, lat: f64, lon: f64) -> bool {
    covers(&set.tiles, lat, lon)
}

fn covers(tiles: &[Grid], lat: f64, lon: f64) -> bool {
    elevation_in(tiles, lat, lon) != Elevation::Outside
}

/// Bilinear elevation across a tile set. A tile that holds the whole surrounding cell answers
/// directly; for a point in the gap between tiles (tiles that abut without overlap) the four
/// surrounding nodes are taken from whichever tiles hold them, which requires the tiles' lattices
/// to line up. Nodata is reported only if no tile can give a value.
pub fn elevation_at(set: &TileSet, lat: f64, lon: f64) -> Elevation {
    elevation_in(&set.tiles, lat, lon)
}

fn elevation_in(tiles: &[Grid], lat: f64, lon: f64) -> Elevation {
    let e = elevation_here(tiles, lat, lon);
    match other_side(e == Elevation::Outside, lon) {
        Some(alias) => elevation_here(tiles, lat, alias),
        None => e,
    }
}

fn elevation_here(tiles: &[Grid], lat: f64, lon: f64) -> Elevation {
    if !valid_coord(lat, lon) {
        return Elevation::Outside;
    }
    let mut nodata = false;
    for g in tiles {
        match grid_elevation_at(g, lat, lon) {
            Elevation::Metres(v) => return Elevation::Metres(v),
            Elevation::NoData => nodata = true,
            Elevation::Outside => {}
        }
    }
    if nodata {
        return Elevation::NoData;
    }
    // Gap between tiles: use the lattice of the nearest tile that reaches within one pixel.
    let Some(base) = tiles.iter().find(|g| {
        let (fx, fy) = frac_index(g, lat, lon);
        fx >= -1.0 && fy >= -1.0 && fx <= f64::from(g.width) && fy <= f64::from(g.height)
    }) else {
        return Elevation::Outside;
    };
    let (fx, fy) = frac_index(base, lat, lon);
    let (c0, r0) = (fx.floor(), fy.floor());
    let (tx, ty) = (fx - c0, fy - r0);
    let at = |dc: f64, dr: f64| -> Option<f32> {
        let la = base.lat0 - (r0 + dr) * base.dlat;
        let lo = base.lon0 + (c0 + dc) * base.dlon;
        tiles.iter().find_map(|g| node_value(g, la, lo))
    };
    match (at(0.0, 0.0), at(1.0, 0.0), at(0.0, 1.0), at(1.0, 1.0)) {
        (Some(a), Some(b), Some(c), Some(d)) => {
            if !(a.is_finite() && b.is_finite() && c.is_finite() && d.is_finite()) {
                return Elevation::NoData;
            }
            let top = f64::from(a) * (1.0 - tx) + f64::from(b) * tx;
            let bot = f64::from(c) * (1.0 - tx) + f64::from(d) * tx;
            Elevation::Metres((top * (1.0 - ty) + bot * ty) as f32)
        }
        _ => Elevation::Outside,
    }
}

/// Great-circle (haversine) distance in metres.
fn haversine_m(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
    let dp = p2 - p1;
    let dl = (lon2 - lon1).to_radians();
    let a = (dp / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dl / 2.0).sin().powi(2);
    2.0 * EARTH_RADIUS_M * a.sqrt().min(1.0).asin()
}

/// Running state of a max query.
struct Acc {
    peak: Option<Peak>,
    samples: u64,
    nodata: u64,
}

/// Visits every node of `g` inside the bounding box that `inside` accepts.
fn scan(
    g: &Grid,
    lat_lo: f64,
    lat_hi: f64,
    lon_lo: f64,
    lon_hi: f64,
    inside: &dyn Fn(f64, f64) -> bool,
    acc: &mut Acc,
) -> Result<(), QueryError> {
    let col_lo = ((lon_lo - g.lon0) / g.dlon).ceil().max(0.0);
    let col_hi = ((lon_hi - g.lon0) / g.dlon)
        .floor()
        .min(f64::from(g.width - 1));
    let row_lo = ((g.lat0 - lat_hi) / g.dlat).ceil().max(0.0);
    let row_hi = ((g.lat0 - lat_lo) / g.dlat)
        .floor()
        .min(f64::from(g.height - 1));
    if col_lo > col_hi || row_lo > row_hi {
        return Ok(());
    }
    let cells = (col_hi - col_lo + 1.0) * (row_hi - row_lo + 1.0);
    if (acc.samples + acc.nodata) as f64 + cells > MAX_QUERY_SAMPLES as f64 {
        return Err(QueryError::TooManySamples);
    }
    for r in row_lo as u32..=row_hi as u32 {
        let lat = g.lat0 - f64::from(r) * g.dlat;
        for c in col_lo as u32..=col_hi as u32 {
            let lon = g.lon0 + f64::from(c) * g.dlon;
            if !inside(lat, lon) {
                continue;
            }
            match grid_sample(g, c, r) {
                Some(v) if v.is_finite() => {
                    acc.samples += 1;
                    if acc.peak.is_none_or(|p| v > p.metres) {
                        acc.peak = Some(Peak {
                            metres: v,
                            lat,
                            lon,
                        });
                    }
                }
                Some(_) => acc.nodata += 1,
                None => {}
            }
        }
    }
    Ok(())
}

/// Bounding box of a circle: (lat_lo, lat_hi, lon_lo, lon_hi), widened to every longitude when the
/// circle reaches a pole.
fn circle_box(lat: f64, lon: f64, radius_m: f64) -> (f64, f64, f64, f64) {
    let d = radius_m / EARTH_RADIUS_M;
    let dlat = d.to_degrees();
    let (lo, hi) = (lat - dlat, lat + dlat);
    if hi >= 90.0 || lo <= -90.0 {
        return (lo.max(-90.0), hi.min(90.0), -180.0, 180.0);
    }
    let dlon = (d.sin() / lat.to_radians().cos())
        .min(1.0)
        .asin()
        .to_degrees();
    (lo, hi, lon - dlon, lon + dlon)
}

fn check_radius(lat: f64, lon: f64, radius_nm: f64) -> Result<f64, QueryError> {
    if !valid_coord(lat, lon) {
        return Err(QueryError::BadCoordinate);
    }
    if !(radius_nm.is_finite() && radius_nm > 0.0 && radius_nm <= MAX_RADIUS_NM) {
        return Err(QueryError::BadRadius);
    }
    Ok(radius_nm * METRES_PER_NM)
}

/// The highest sample of one grid within `radius_nm` of a point. Samples exactly on the circle
/// count; the surface between samples is not searched (the bilinear maximum is always at a sample,
/// but one up to a pixel outside the circle is not seen).
pub fn grid_max_in_radius(
    g: &Grid,
    lat: f64,
    lon: f64,
    radius_nm: f64,
) -> Result<MaxQuery, QueryError> {
    max_in_radius(std::slice::from_ref(g), lat, lon, radius_nm)
}

/// The highest sample of a tile set within `radius_nm` of a point; see [`grid_max_in_radius`].
pub fn max_elevation_in_radius(
    set: &TileSet,
    lat: f64,
    lon: f64,
    radius_nm: f64,
) -> Result<MaxQuery, QueryError> {
    if set.tiles.len() > MAX_TILES {
        return Err(QueryError::TooManyTiles);
    }
    max_in_radius(&set.tiles, lat, lon, radius_nm)
}

fn max_in_radius(
    tiles: &[Grid],
    lat: f64,
    lon: f64,
    radius_nm: f64,
) -> Result<MaxQuery, QueryError> {
    let r_m = check_radius(lat, lon, radius_nm)?;
    let (la0, la1, lo0, lo1) = circle_box(lat, lon, r_m);
    let inside = move |la: f64, lo: f64| haversine_m(lat, lon, la, lo) <= r_m;
    let mut acc = Acc {
        peak: None,
        samples: 0,
        nodata: 0,
    };
    for g in tiles {
        scan(g, la0, la1, lo0, lo1, &inside, &mut acc)?;
    }
    // Probes: the centre and 72 points on the circle's edge; a point counts as covered when it
    // has an answer from some tile.
    let mut covered = covers(tiles, lat, lon);
    for k in 0..72 {
        let brg = f64::from(k) * 5.0_f64.to_radians();
        let d = r_m / EARTH_RADIUS_M;
        let p1 = lat.to_radians();
        let la = (p1.sin() * d.cos() + p1.cos() * d.sin() * brg.cos()).asin();
        let lo = lon.to_radians()
            + (brg.sin() * d.sin() * p1.cos()).atan2(d.cos() - p1.sin() * la.sin());
        covered &= covers(tiles, la.to_degrees(), lo.to_degrees());
    }
    Ok(MaxQuery {
        peak: acc.peak,
        samples: acc.samples,
        nodata: acc.nodata,
        probes_covered: covered,
    })
}

/// True when (`lat`, `lon`) is inside the polygon, even-odd rule in the latitude/longitude plane.
pub(crate) fn in_polygon(poly: &[(f64, f64)], lat: f64, lon: f64) -> bool {
    let mut inside = false;
    let mut j = poly.len().saturating_sub(1);
    for (i, &(yi, xi)) in poly.iter().enumerate() {
        let (yj, xj) = poly.get(j).copied().unwrap_or((yi, xi));
        if (yi > lat) != (yj > lat) && lon < (xj - xi) * (lat - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

pub(crate) fn check_polygon(poly: &[(f64, f64)]) -> Result<(f64, f64, f64, f64), QueryError> {
    if poly.len() < 3 || poly.len() > MAX_POLYGON_VERTICES {
        return Err(QueryError::BadPolygon);
    }
    let (mut la0, mut la1, mut lo0, mut lo1) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
    for &(la, lo) in poly {
        if !valid_coord(la, lo) {
            return Err(QueryError::BadCoordinate);
        }
        la0 = la0.min(la);
        la1 = la1.max(la);
        lo0 = lo0.min(lo);
        lo1 = lo1.max(lo);
    }
    if lo1 - lo0 > 180.0 {
        return Err(QueryError::BadPolygon);
    }
    Ok((la0, la1, lo0, lo1))
}

/// The highest sample of one grid inside a polygon of (latitude, longitude) vertices.
pub fn grid_max_in_polygon(g: &Grid, poly: &[(f64, f64)]) -> Result<MaxQuery, QueryError> {
    max_in_polygon(std::slice::from_ref(g), poly)
}

/// The highest sample of a tile set inside a polygon of (latitude, longitude) vertices.
pub fn max_elevation_in_polygon(
    set: &TileSet,
    poly: &[(f64, f64)],
) -> Result<MaxQuery, QueryError> {
    if set.tiles.len() > MAX_TILES {
        return Err(QueryError::TooManyTiles);
    }
    max_in_polygon(&set.tiles, poly)
}

fn max_in_polygon(tiles: &[Grid], poly: &[(f64, f64)]) -> Result<MaxQuery, QueryError> {
    let (la0, la1, lo0, lo1) = check_polygon(poly)?;
    let inside = |la: f64, lo: f64| in_polygon(poly, la, lo);
    let mut acc = Acc {
        peak: None,
        samples: 0,
        nodata: 0,
    };
    for g in tiles {
        scan(g, la0, la1, lo0, lo1, &inside, &mut acc)?;
    }
    let mut covered = true;
    let mut prev = poly.last().copied();
    for &(la, lo) in poly {
        covered &= covers(tiles, la, lo);
        if let Some((pa, po)) = prev {
            covered &= covers(tiles, (la + pa) / 2.0, (lo + po) / 2.0);
        }
        prev = Some((la, lo));
    }
    Ok(MaxQuery {
        peak: acc.peak,
        samples: acc.samples,
        nodata: acc.nodata,
        probes_covered: covered,
    })
}
