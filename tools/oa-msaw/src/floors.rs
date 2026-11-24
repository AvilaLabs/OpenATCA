//! The floor lookups the core asks for: terrain, obstacles and charts at a sample point. Shell
//! (B3): it reads files. Every answer is conservative (a number that is too high, never too low)
//! or absent:
//!
//! * Terrain is the highest elevation of every sample that touches the circle of the point's
//!   radius, rounded up to whole feet, read from a grid of 0.002-degree cells per tile (`coarse`)
//!   that holds, for each cell, the highest sample touching it; the lookup widens the circle's
//!   cells by one. A point is looked up in a cell of 0.002 degrees and a radius bucket of 250 m:
//!   the lookup is made at the cell's centre with the bucket's radius plus the cell's half
//!   diagonal, which covers every circle in the cell and bucket. If any cell under the circle is
//!   nodata or in a tile that is not loaded, the answer is absent: no lower bound is used. A tile
//!   is read and reduced once, the first time a lookup touches it, and its samples are dropped;
//!   tiles do not need to share a lattice. The query over the samples themselves (`oa-tiff`'s
//!   conservative query) is the oracle of the tests.
//! * Obstacles are the highest top of every obstacle within the circle (distances taken with the
//!   smallest metre-per-degree scales, which only adds obstacles); none is not an absence of data.
//! * The chart is the highest minimum altitude of the FUS3, FUS5 and TAV sectors that contain the
//!   point, absent where no sector of any of them does.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::Instant;

use oa_dof::Dof;
use oa_msaw::{
    corridor_hit_for, corridor_hit_ft, Corridor, FloorSample, MsawConfig, SamplePoint, CK_APPROACH,
    CK_DEPARTURE, FLOOR_NONE,
};
use oa_mva::Model;
use oa_sealed::Sealed;
use oa_sha256::sha256;
use oa_tiff::{Grid, TileSet, METRES_PER_FOOT};

use crate::chart_raster::ChartRaster;
use crate::coarse::{Coarse, CoarseTerrain, CELLS_PER_DEG};
use crate::guard::refuse_sealed;
use crate::guard::{open_guarded, OpenFail};

/// The obstacle index cell, degrees.
const OBST_CELL_DEG: f64 = 0.1;
/// Metres per degree of latitude, the smallest anywhere (at the equator).
const M_PER_DEG_LAT_MIN: f64 = 110_500.0;

/// What the lookups did. A7.2: cold struct, size guard waived.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FloorStats {
    pub points: u64,
    pub cache_hits: u64,
    pub terrain_queries: u64,
    /// Terrain queries that gave no complete answer: the sum of the four that follow.
    pub terrain_absent: u64,
    /// The circle reaches an area no tile holds (a tile absent, or the edge of the data).
    pub terrain_missing: u64,
    /// The circle holds a sample that is nodata in every tile (and none is missing).
    pub terrain_nodata: u64,
    /// Always zero: each tile is reduced alone, so tiles need not share a lattice.
    pub terrain_misaligned: u64,
    /// Any other refusal of the query.
    pub terrain_refused: u64,
    pub tiles_loaded: u64,
    pub tiles_absent: u64,
    /// Tiles read again after being dropped (none: a reduced tile stays).
    pub tiles_reloaded: u64,
    /// Wall time, nanoseconds: in the terrain query itself, reading tiles from disk, in the
    /// obstacle index and in the chart.
    pub ns_terrain: u64,
    pub ns_tile_read: u64,
    /// Of `ns_tile_read`, the part spent reducing a tile to its grid.
    pub ns_tile_reduce: u64,
    pub ns_obstacle: u64,
    pub ns_chart: u64,
    /// Always zero (see `terrain_misaligned`).
    pub blocks_misaligned: u64,
    pub chart_absent: u64,
    /// Bytes the reduced terrain grids hold, and the one-degree blocks of the chart raster built.
    pub terrain_grid_bytes: u64,
    /// Terrain lookups refused by the configured retained-tile, resident-byte or cell-visit caps.
    pub terrain_budget_refused: u64,
    pub tiles_refused: u64,
    pub chart_blocks: u64,
}

/// One obstacle: latitude and longitude in 1e-7 degree, top in feet MSL.
type Obstacle = (i32, i32, i32, u32);

/// The obstacles in a grid of cells for the circle query.
// A7.2: cold struct, size guard waived
#[derive(Debug, Default)]
pub struct ObstacleIndex {
    cells: HashMap<(i32, i32), Vec<Obstacle>>,
    pub count: usize,
    /// The extent of the DOF file the obstacles came from, 1e-7 degree: south, north, west, east
    /// (the bounding area of its obstacles). `None`: no extent was given (the data is taken as
    /// covering everywhere, as a test hands it over).
    pub extent: Option<(i32, i32, i32, i32)>,
    /// How many states and countries the file names.
    pub regions: usize,
    /// The coverage of the files loaded: the union of each file's own covered cells of
    /// [`COVER_CELL_DEG`] (see [`cover_cells`]; a bounding box of a state would cover its
    /// neighbours). `None`: no extent was given.
    coverage: Option<std::collections::HashSet<(i32, i32)>>,
    /// The files the set is made of.
    pub files: usize,
}

/// The cells of the coverage of obstacle data, degrees.
const COVER_CELL_DEG: f64 = 0.5;

fn cell_of(deg: f64, size: f64) -> i32 {
    (deg / size).floor() as i32
}

/// How far, in cells, an obstacle cell must lie on each side of an empty cell for it to count as an
/// interior gap.
const GAP_CELLS: i32 = 4;

/// The covered cells of one file's positions (1e-7 degree): the cells that hold an obstacle, and an
/// empty cell with obstacle cells on both sides of it within [`GAP_CELLS`] along its row or column.
/// Empty cells connected to a file's bounding edge remain uncovered. Fill each file's interior
/// gaps before taking the union of the files.
fn cover_cells(points: impl Iterator<Item = (i32, i32)>) -> std::collections::HashSet<(i32, i32)> {
    const NEIGHBOURS: [(i32, i32); 8] = [
        (-1, -1),
        (-1, 0),
        (-1, 1),
        (0, -1),
        (0, 1),
        (1, -1),
        (1, 0),
        (1, 1),
    ];
    let mut have = std::collections::HashSet::new();
    for (la, lo) in points {
        have.insert((
            cell_of(f64::from(la) / 1e7, COVER_CELL_DEG),
            cell_of(f64::from(lo) / 1e7, COVER_CELL_DEG),
        ));
    }
    let (Some(a0), Some(a1), Some(b0), Some(b1)) = (
        have.iter().map(|c| c.0).min(),
        have.iter().map(|c| c.0).max(),
        have.iter().map(|c| c.1).min(),
        have.iter().map(|c| c.1).max(),
    ) else {
        return have;
    };
    let side = |a: i32, b: i32, da: i32, db: i32| {
        (1..=GAP_CELLS).any(|k| have.contains(&(a + da * k, b + db * k)))
    };
    let mut exterior = std::collections::HashSet::new();
    let mut pending: Vec<_> = (a0..=a1)
        .flat_map(|a| [(a, b0), (a, b1)])
        .chain((b0..=b1).flat_map(|b| [(a0, b), (a1, b)]))
        .collect();
    while let Some((a, b)) = pending.pop() {
        if !(a0..=a1).contains(&a)
            || !(b0..=b1).contains(&b)
            || have.contains(&(a, b))
            || !exterior.insert((a, b))
        {
            continue;
        }
        pending.extend(NEIGHBOURS.iter().map(|(da, db)| (a + da, b + db)));
    }
    let mut gaps = Vec::new();
    for a in a0..=a1 {
        for b in b0..=b1 {
            if !have.contains(&(a, b))
                && !exterior.contains(&(a, b))
                && ((side(a, b, 0, -1) && side(a, b, 0, 1))
                    || (side(a, b, -1, 0) && side(a, b, 1, 0)))
            {
                gaps.push((a, b));
            }
        }
    }
    have.extend(gaps);
    have
}

/// Indexes the obstacles of `d` (position 1e-7 degree, top feet above mean sea level).
pub fn obstacle_index(d: &Dof) -> ObstacleIndex {
    let mut ix = ObstacleIndex::from_points(
        d.lat_e7
            .iter()
            .zip(d.lon_e7.iter())
            .zip(d.amsl_ft.iter())
            .map(|((&la, &lo), &top)| (la, lo, top)),
    );
    let bounds = |v: &[i32]| v.iter().copied().min().zip(v.iter().copied().max());
    if let (Some((s, n)), Some((w, e))) = (bounds(&d.lat_e7), bounds(&d.lon_e7)) {
        ix.extent = Some((s, n, w, e));
        ix.coverage = Some(cover_cells(
            d.lat_e7.iter().zip(d.lon_e7.iter()).map(|(&a, &b)| (a, b)),
        ));
    }
    ix.files = 1;
    let mut r: Vec<[u8; 2]> = d.oas_code.clone();
    r.sort_unstable();
    r.dedup();
    ix.regions = r.len();
    ix
}

impl ObstacleIndex {
    /// An index of obstacles given as (latitude 1e-7 degree, longitude 1e-7 degree, top feet MSL).
    pub fn from_points(points: impl Iterator<Item = (i32, i32, i32)>) -> ObstacleIndex {
        let mut ix = ObstacleIndex::default();
        for (la, lo, top) in points {
            let rec = ix.count as u32;
            let key = (
                cell_of(f64::from(la) / 1e7, OBST_CELL_DEG),
                cell_of(f64::from(lo) / 1e7, OBST_CELL_DEG),
            );
            ix.cells.entry(key).or_default().push((la, lo, top, rec));
            ix.count += 1;
        }
        ix
    }

    /// The number of covered cells; `None` where no coverage was given.
    pub fn covered_cells(&self) -> Option<usize> {
        self.coverage.as_ref().map(|c| c.len())
    }

    /// True if the circle of `radius_m` around the point lies inside the coverage of the files the
    /// obstacles came from (or no extent was given): obstacle data covers it. A point outside has
    /// no obstacle data, which is no coverage, never "no obstacle".
    pub fn covers(&self, lat: f64, lon: f64, radius_m: f64) -> bool {
        let Some(cells) = &self.coverage else {
            return true;
        };
        let cos = lat.to_radians().cos().max(0.01);
        let (dlat, dlon) = (
            radius_m / M_PER_DEG_LAT_MIN,
            radius_m / (M_PER_DEG_LAT_MIN * cos),
        );
        [
            (-1.0, -1.0),
            (-1.0, 1.0),
            (1.0, -1.0),
            (1.0, 1.0),
            (0.0, 0.0),
        ]
        .iter()
        .all(|&(a, b)| {
            cells.contains(&(
                cell_of(lat + a * dlat, COVER_CELL_DEG),
                cell_of(lon + b * dlon, COVER_CELL_DEG),
            ))
        })
    }

    /// An index of the obstacles (latitude, longitude 1e-7 degree, top feet) of one file, with the
    /// coverage the file's own obstacles give, as a file read from disk gives it. For a test.
    pub fn from_file_points(points: Vec<(i32, i32, i32)>) -> ObstacleIndex {
        let cover = cover_cells(points.iter().map(|&(a, b, _)| (a, b)));
        let lat = points.iter().map(|p| p.0);
        let lon = points.iter().map(|p| p.1);
        let extent = lat
            .clone()
            .min()
            .zip(lat.max())
            .zip(lon.clone().min().zip(lon.max()))
            .map(|((s, n), (w, e))| (s, n, w, e));
        let mut ix = ObstacleIndex::from_points(points.into_iter());
        ix.extent = extent;
        ix.coverage = Some(cover);
        ix.files = 1;
        ix
    }

    /// Gives the set the extent of one file as the box (south, north, west, east, 1e-7 degree): the
    /// cells it holds, as a file read from disk gives its own. For a test.
    pub fn set_extent_box(&mut self, s: i32, n: i32, w: i32, e: i32) {
        self.extent = Some((s, n, w, e));
        let cell = |x: i32| cell_of(f64::from(x) / 1e7, COVER_CELL_DEG);
        let mut cells = std::collections::HashSet::new();
        for a in cell(s)..=cell(n) {
            for b in cell(w)..=cell(e) {
                cells.insert((a, b));
            }
        }
        self.coverage = Some(cells);
        self.files = self.files.max(1);
    }

    /// Adds the obstacles and the coverage of `o`: the set of several files. The coverage is the
    /// union of each file's own, the extent the box around both.
    pub fn merge(&mut self, o: ObstacleIndex) {
        let base = self.count;
        for (k, v) in o.cells {
            let e = self.cells.entry(k).or_default();
            e.extend(
                v.into_iter()
                    .map(|(la, lo, t, r)| (la, lo, t, r + base as u32)),
            );
        }
        self.count += o.count;
        self.regions += o.regions;
        self.files += o.files;
        self.extent = match (self.extent, o.extent) {
            (Some(a), Some(b)) => Some((a.0.min(b.0), a.1.max(b.1), a.2.min(b.2), a.3.max(b.3))),
            (a, b) => a.or(b),
        };
        self.coverage = match (self.coverage.take(), o.coverage) {
            (Some(mut a), Some(b)) => {
                a.extend(b);
                Some(a)
            }
            (a, b) => a.or(b),
        };
    }

    /// The highest obstacle within `radius_m` of the point: its top (feet), its record number in
    /// the order the points were given, and its distance (metres).
    pub fn highest_record_within(
        &self,
        lat: f64,
        lon: f64,
        radius_m: f64,
    ) -> Option<(i32, u32, f64)> {
        let cos = lat.to_radians().cos().max(0.01);
        let (dlat, dlon) = (
            radius_m / M_PER_DEG_LAT_MIN,
            radius_m / (M_PER_DEG_LAT_MIN * cos),
        );
        let mut best: Option<(i32, u32, f64)> = None;
        for a in cell_of(lat - dlat, OBST_CELL_DEG)..=cell_of(lat + dlat, OBST_CELL_DEG) {
            for b in cell_of(lon - dlon, OBST_CELL_DEG)..=cell_of(lon + dlon, OBST_CELL_DEG) {
                for &(la, lo, top, rec) in self.cells.get(&(a, b)).map_or(&[][..], |v| &v[..]) {
                    let dy = (f64::from(la) / 1e7 - lat) * M_PER_DEG_LAT_MIN;
                    let dx = (f64::from(lo) / 1e7 - lon) * M_PER_DEG_LAT_MIN * cos;
                    let d = (dx * dx + dy * dy).sqrt();
                    if d <= radius_m && best.is_none_or(|(t, _, _)| top > t) {
                        best = Some((top, rec, d));
                    }
                }
            }
        }
        best
    }

    /// The highest top within `radius_m` of the point among the obstacles `keep` accepts (it is
    /// given the obstacle's latitude and longitude, 1e-7 degree, and its top), feet;
    /// [`FLOOR_NONE`] if there is none.
    pub fn highest_where(
        &self,
        lat: f64,
        lon: f64,
        radius_m: f64,
        keep: &dyn Fn(i32, i32, i32) -> bool,
    ) -> i32 {
        let cos = lat.to_radians().cos().max(0.01);
        let (dlat, dlon) = (
            radius_m / M_PER_DEG_LAT_MIN,
            radius_m / (M_PER_DEG_LAT_MIN * cos),
        );
        let mut best = FLOOR_NONE;
        for a in cell_of(lat - dlat, OBST_CELL_DEG)..=cell_of(lat + dlat, OBST_CELL_DEG) {
            for b in cell_of(lon - dlon, OBST_CELL_DEG)..=cell_of(lon + dlon, OBST_CELL_DEG) {
                for &(la, lo, top, _) in self.cells.get(&(a, b)).map_or(&[][..], |v| &v[..]) {
                    let dy = (f64::from(la) / 1e7 - lat) * M_PER_DEG_LAT_MIN;
                    let dx = (f64::from(lo) / 1e7 - lon) * M_PER_DEG_LAT_MIN * cos;
                    if dx * dx + dy * dy <= radius_m * radius_m && top > best && keep(la, lo, top) {
                        best = top;
                    }
                }
            }
        }
        best
    }

    /// The highest top within `radius_m` of the point, feet; [`FLOOR_NONE`] if there is none.
    pub fn highest_within(&self, lat: f64, lon: f64, radius_m: f64) -> i32 {
        let cos = lat.to_radians().cos().max(0.01);
        let dlat = radius_m / M_PER_DEG_LAT_MIN;
        let dlon = radius_m / (M_PER_DEG_LAT_MIN * cos);
        let (a0, a1) = (
            cell_of(lat - dlat, OBST_CELL_DEG),
            cell_of(lat + dlat, OBST_CELL_DEG),
        );
        let (b0, b1) = (
            cell_of(lon - dlon, OBST_CELL_DEG),
            cell_of(lon + dlon, OBST_CELL_DEG),
        );
        let mut best = FLOOR_NONE;
        for a in a0..=a1 {
            for b in b0..=b1 {
                let Some(v) = self.cells.get(&(a, b)) else {
                    continue;
                };
                for &(la, lo, top, _) in v {
                    let dy = (f64::from(la) / 1e7 - lat) * M_PER_DEG_LAT_MIN;
                    let dx = (f64::from(lo) / 1e7 - lon) * M_PER_DEG_LAT_MIN * cos;
                    if dx * dx + dy * dy <= radius_m * radius_m && top > best {
                        best = top;
                    }
                }
            }
        }
        best
    }
}

/// An area with its own clearance above terrain (degrees).
#[derive(Debug, Clone, PartialEq)]
pub enum Area {
    /// South, north, west, east.
    Box(f64, f64, f64, f64),
    /// Vertices as (latitude, longitude).
    Polygon(Vec<(f64, f64)>),
    /// Centre latitude, longitude and radius in metres.
    Circle(f64, f64, f64),
}

impl Area {
    pub fn holds(&self, lat: f64, lon: f64) -> bool {
        match self {
            Area::Box(s, n, w, e) => lat >= *s && lat <= *n && lon >= *w && lon <= *e,
            Area::Circle(la, lo, r) => {
                let dy = (lat - la) * 111_194.926_644_558_74;
                let dx = (lon - lo) * 111_194.926_644_558_74 * la.to_radians().cos();
                dx * dx + dy * dy <= r * r
            }
            Area::Polygon(v) => {
                let mut inside = false;
                let mut j = v.len().saturating_sub(1);
                for (i, &(yi, xi)) in v.iter().enumerate() {
                    let (yj, xj) = v.get(j).copied().unwrap_or((yi, xi));
                    if (yi > lat) != (yj > lat) && lon < (xj - xi) * (lat - yi) / (yj - yi) + xi {
                        inside = !inside;
                    }
                    j = i;
                }
                inside
            }
        }
    }
}

impl Area {
    /// True if any part of the circle of `radius_m` around (`lat`, `lon`) lies in the area: the
    /// conservative direction for an area that raises the floor. A polygon is tested by its
    /// vertices, the circle's centre and 16 points of the circle's edge.
    pub fn overlaps(&self, lat: f64, lon: f64, radius_m: f64) -> bool {
        const M: f64 = 111_194.926_644_558_74;
        let cos = lat.to_radians().cos().max(0.01);
        match self {
            Area::Box(s, n, w, e) => {
                let (ny, nx) = (lat.clamp(*s, *n), lon.clamp(*w, *e));
                let (dy, dx) = ((lat - ny) * M, (lon - nx) * M * cos);
                dx * dx + dy * dy <= radius_m * radius_m
            }
            Area::Circle(la, lo, r) => {
                let dy = (lat - la) * M;
                let dx = (lon - lo) * M * la.to_radians().cos();
                (dx * dx + dy * dy).sqrt() <= r + radius_m
            }
            Area::Polygon(v) => {
                if self.holds(lat, lon) {
                    return true;
                }
                if v.iter().any(|&(vy, vx)| {
                    let (dy, dx) = ((vy - lat) * M, (vx - lon) * M * cos);
                    dx * dx + dy * dy <= radius_m * radius_m
                }) {
                    return true;
                }
                (0..16).any(|k| {
                    let a = f64::from(k) * core::f64::consts::TAU / 16.0;
                    self.holds(
                        lat + radius_m * a.cos() / M,
                        lon + radius_m * a.sin() / (M * cos),
                    )
                })
            }
        }
    }
}

/// An airport terminal area: a circle around the airport reference point, below a ceiling, with a
/// clearance above terrain in place of the general one (A2.08 gives legacy's terrain inhibit
/// areas around airports for practice, visual and circling approaches; this lowers the clearance
/// there and inhibits nothing). Development values, set by the configuration.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AirportArea {
    pub lat: f64,
    pub lon: f64,
    pub radius_m: f64,
    /// The airport elevation plus the ceiling above ground, feet MSL.
    pub ceiling_ft: i32,
    pub clearance_ft: i32,
}

/// The lookups with their data and cache. A7.2: cold struct, size guard waived.
pub struct Floors {
    tile_dir: Option<PathBuf>,
    /// The terrain was given whole (a test), none is read from `tile_dir`.
    preloaded: bool,
    coarse: CoarseTerrain,
    /// The tiles reduced, by the south-west corner of their cell.
    loaded: BTreeSet<(i32, i32)>,
    absent: BTreeSet<(i32, i32)>,
    refused: BTreeSet<(i32, i32)>,
    terrain_limits: Option<TerrainServiceLimits>,
    terrain_hashes: Vec<(String, [u8; 32])>,
    obstacles: Option<ObstacleIndex>,
    chart: Option<ChartRaster>,
    areas: Vec<(Area, i32)>,
    airports: Vec<AirportArea>,
    /// The corridors of the net, for the obstacles an approach surface stands for.
    corridors: Option<Box<MsawConfig>>,
    /// The grid index of those corridors.
    cidx: Option<oa_msaw::CorridorIndex>,
    pub stats: FloorStats,
}

/// Optional bounds for a reusable terrain service. Unbounded legacy constructors retain their
/// historical behavior; the regional startup adapter always installs these bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TerrainServiceLimits {
    pub max_tiles: usize,
    pub max_file_bytes: u64,
    pub max_resident_bytes: usize,
    pub max_query_cells: u64,
}

/// Whether a circle of `radius_m` at (`lat`, `lon`) can touch the corridor: its centre is within
/// the corridor widened by the radius (a bound that only adds).
fn reaches(c: &Corridor, lat: f64, lon: f64, radius_m: f64) -> bool {
    let (tlat, tlon) = (f64::from(c.thr_lat_e7) / 1e7, f64::from(c.thr_lon_e7) / 1e7);
    let dy = (lat - tlat) * 111_200.0;
    let dx = (lon - tlon) * 111_200.0 * tlat.to_radians().cos();
    let b = c.out_bearing_deg.to_radians();
    let along = dx * b.sin() + dy * b.cos();
    let across = (dx * b.cos() - dy * b.sin()).abs();
    let wide = c.near_half_width_m.max(c.far_half_width_m);
    along >= -radius_m && along <= c.length_m + radius_m && across <= wide + 2.0 * radius_m
}

/// Whole feet rounded up, held to the range of an `i32` with room for a clearance.
pub fn ceil_ft(m: f64) -> i32 {
    (m / METRES_PER_FOOT).ceil().clamp(-1.0e6, 1.0e6) as i32
}

/// The 3DEP tile name of the 1-degree cell whose south-west corner is (`lat`, `lon`), western
/// hemisphere only (`None` elsewhere: there is no such tile).
pub fn tile_name(lat: i32, lon: i32) -> Option<String> {
    if lon >= 0 {
        return None;
    }
    Some(format!("USGS_1_n{:02}w{:03}.tif", lat + 1, -lon))
}

pub fn read_tile(path: &Path) -> Result<Grid, String> {
    refuse_sealed(Sealed::production(), path)?;
    let bytes = oa_tiff::read_file(path).map_err(|e| format!("{}: {e}", path.display()))?;
    oa_tiff::read_geotiff(&bytes).map_err(|e| format!("{}: {e}", path.display()))
}

/// Reads one terrain source under the regional service's tighter byte cap, before TIFF parsing.
fn read_tile_bounded(path: &Path, limit: u64) -> Result<(Grid, [u8; 32]), String> {
    let file = open_guarded(&Sealed::production(), path).map_err(|e| match e {
        OpenFail::Refused(m) | OpenFail::Io(m) => m,
    })?;
    let mut bytes = Vec::new();
    let mut limited = std::io::Read::take(file, limit.saturating_add(1));
    std::io::Read::read_to_end(&mut limited, &mut bytes)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > limit {
        return Err(format!(
            "{}: terrain file exceeds {limit} bytes",
            path.display()
        ));
    }
    let hash = sha256(&bytes);
    let grid = oa_tiff::read_geotiff(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok((grid, hash))
}

impl Floors {
    pub fn new(
        tile_dir: Option<PathBuf>,
        obstacles: Option<ObstacleIndex>,
        chart: Option<Model>,
    ) -> Floors {
        Floors {
            tile_dir,
            preloaded: false,
            coarse: CoarseTerrain::default(),
            loaded: BTreeSet::new(),
            absent: BTreeSet::new(),
            refused: BTreeSet::new(),
            terrain_limits: None,
            terrain_hashes: Vec::new(),
            obstacles,
            chart: chart.map(ChartRaster::new),
            areas: Vec::new(),
            airports: Vec::new(),
            corridors: None,
            cidx: None,
            stats: FloorStats::default(),
        }
    }

    /// Installs explicit startup limits for future lazy terrain reads and per-query work.
    pub(crate) fn set_terrain_limits(&mut self, limits: TerrainServiceLimits) {
        self.terrain_limits = Some(limits);
    }

    /// SHA-256 hashes for the exact terrain bytes actually decoded by this service, in load order.
    pub fn terrain_hashes(&self) -> &[(String, [u8; 32])] {
        &self.terrain_hashes
    }

    /// Lookups over tiles held in memory (a test hands them over); no file is read.
    pub fn with_tiles(
        set: TileSet,
        obstacles: Option<ObstacleIndex>,
        chart: Option<Model>,
    ) -> Floors {
        let mut f = Floors::new(Some(PathBuf::new()), obstacles, chart);
        f.preloaded = true;
        for g in &set.tiles {
            f.coarse.add_grid(g);
        }
        f.stats.terrain_grid_bytes = f.coarse.bytes() as u64;
        f
    }

    /// Areas with their own clearance above terrain; where several hold a point the highest wins.
    /// Gives the lookups the corridors of the net. Where a lookup circle is wholly inside corridors
    /// that all have a surface (approach corridors and runway strips) and no departure corridor
    /// reaches it, an obstacle whose top is not above the highest surface of those corridors at
    /// the obstacle's own position, less the allowance, is left out: the surface stands for it.
    /// Elsewhere every obstacle counts.
    pub fn set_corridors(&mut self, cfg: &MsawConfig) {
        self.corridors = (!cfg.corridors.is_empty()).then(|| Box::new(cfg.clone()));
        self.cidx = if !cfg.corridors.is_empty() {
            oa_msaw::corridor_index_new(cfg).ok()
        } else {
            None
        };
    }

    /// The highest obstacle top for the lookup circle at (`lat`, `lon`) of `radius_m`, for a track
    /// with ground track `track_deg` (whole degrees, -1: none): the approach corridors and runway
    /// strips that count are those the track is aligned with.
    fn obstacle_top(&self, lat: f64, lon: f64, radius_m: f64, track_deg: i16) -> i32 {
        let Some(ix) = self.obstacles.as_ref() else {
            return FLOOR_NONE;
        };
        let (Some(cfg), Some(cidx)) = (self.corridors.as_deref(), self.cidx.as_ref()) else {
            return ix.highest_within(lat, lon, radius_m);
        };
        let (la, lo) = ((lat * 1e7).round() as i32, (lon * 1e7).round() as i32);
        let mut holding: Vec<&Corridor> = Vec::new();
        for k in cidx.near(la, lo, radius_m) {
            let Some(c) = cfg.corridors.get(usize::from(k)) else {
                continue;
            };
            if c.kind == CK_DEPARTURE {
                if reaches(c, lat, lon, radius_m) {
                    return ix.highest_within(lat, lon, radius_m);
                }
            } else if corridor_hit_for(c, la, lo, radius_m, track_deg, cfg).is_some() {
                holding.push(c);
            }
        }
        if holding.is_empty() {
            return ix.highest_within(lat, lon, radius_m);
        }
        // The surface falls toward the threshold: an obstacle at the edge of the circle can be above
        // the surface at the centre by the tangent of the angle times the radius. Only where the
        // allowance covers that over this lookup's whole radius may the surface stand for the
        // obstacles under it; otherwise every obstacle counts.
        let rise_ft = holding
            .iter()
            .filter(|c| c.kind == CK_APPROACH)
            .map(|c| oa_math::tan(c.path_angle_deg.to_radians()) * radius_m / 0.3048)
            .fold(0.0, f64::max);
        if rise_ft > f64::from(cfg.approach_allowance_ft) {
            return ix.highest_within(lat, lon, radius_m);
        }
        ix.highest_where(lat, lon, radius_m, &|ola, olo, top| {
            let surface = holding
                .iter()
                .filter_map(|c| corridor_hit_ft(c, ola, olo, 0.0, cfg).and_then(|h| h.surface_ft))
                .max();
            surface.is_none_or(|s| top > s)
        })
    }

    pub fn set_areas(&mut self, areas: Vec<(Area, i32)>) {
        self.areas = areas;
    }

    /// The airport terminal areas. Where several hold a whole cell the lowest clearance applies, with
    /// the lowest ceiling.
    pub fn set_airports(&mut self, mut a: Vec<AirportArea>) {
        a.sort_by(|x, y| oa_math::cmp_canon_f64(x.lat, y.lat));
        self.airports = a;
    }

    /// The airport area clearance and ceiling over a lookup circle (the cell's centre and the
    /// radius that covers the point's query circle): the areas that hold the whole circle, so that
    /// a point near an edge errs toward the general clearance; [`FLOOR_NONE`] twice if none.
    fn apt_at(&self, clat: f64, clon: f64, radius_m: f64) -> (i32, i32) {
        const M: f64 = 111_194.926_644_558_74;
        let mut best: Option<(i32, i32)> = None;
        // The areas are sorted by latitude: only those within the largest radius are looked at.
        let reach = self.airports.iter().map(|a| a.radius_m).fold(0.0, f64::max) / M;
        let from = self.airports.partition_point(|a| a.lat < clat - reach);
        let to = self.airports.partition_point(|a| a.lat <= clat + reach);
        for a in self.airports.get(from..to).unwrap_or(&[]) {
            let (dy, dx) = (
                (a.lat - clat) * M,
                (a.lon - clon) * M * a.lat.to_radians().cos(),
            );
            if (dx * dx + dy * dy).sqrt() + radius_m <= a.radius_m {
                best = Some(match best {
                    None => (a.clearance_ft, a.ceiling_ft),
                    Some((c0, k0)) => (c0.min(a.clearance_ft), k0.min(a.ceiling_ft)),
                });
            }
        }
        best.unwrap_or((FLOOR_NONE, FLOOR_NONE))
    }

    /// The clearance of the areas over a lookup circle: the highest of those it overlaps at all (an
    /// area that raises the floor errs to the high side); [`FLOOR_NONE`] if none does.
    fn gtm_at(&self, clat: f64, clon: f64, radius_m: f64) -> i32 {
        let mut best = FLOOR_NONE;
        for (a, ft) in &self.areas {
            if *ft > best && a.overlaps(clat, clon, radius_m) {
                best = *ft;
            }
        }
        best
    }

    /// How far from a point the cell that sets its floor is: the smallest radius, up to `radius_m`,
    /// at which the terrain (kind `Terrain`) or the obstacle top (`Obstacle`) reaches the value
    /// found over the whole radius, metres, to a metre. `None` for any other kind or no data.
    pub fn setter_distance_m(
        &mut self,
        lat_e7: i32,
        lon_e7: i32,
        radius_m: f64,
        kind: oa_msaw::FloorKind,
        track_deg: i16,
    ) -> Option<f64> {
        let (lat, lon) = (f64::from(lat_e7) / 1e7, f64::from(lon_e7) / 1e7);
        let at = |me: &mut Self, r: f64| -> i32 {
            match kind {
                oa_msaw::FloorKind::Terrain if me.tile_dir.is_some() => {
                    me.terrain_ft(lat, lon, r).unwrap_or(FLOOR_NONE)
                }
                oa_msaw::FloorKind::Obstacle => me.obstacle_top(lat, lon, r, track_deg),
                _ => FLOOR_NONE,
            }
        };
        let top = at(self, radius_m);
        if top == FLOOR_NONE {
            return None;
        }
        let (mut lo, mut hi) = (0.0, radius_m);
        while hi - lo > 1.0 {
            let mid = (lo + hi) / 2.0;
            if at(self, mid) >= top {
                hi = mid;
            } else {
                lo = mid;
            }
        }
        Some(hi)
    }

    /// The radius a lookup really uses for a point of radius `radius_m`: the radius itself, metres.
    pub fn effective_radius_m(radius_m: f64) -> f64 {
        radius_m
    }

    /// The highest terrain within `radius_m` of a point, feet; for the diagnosis.
    pub fn terrain_within_ft(&mut self, lat: f64, lon: f64, radius_m: f64) -> i32 {
        if self.tile_dir.is_none() {
            return FLOOR_NONE;
        }
        self.terrain_ft(lat, lon, radius_m).unwrap_or(FLOOR_NONE)
    }

    /// The highest obstacle record within `radius_m` of a point; for the diagnosis.
    pub fn obstacle_record_within(
        &self,
        lat: f64,
        lon: f64,
        radius_m: f64,
    ) -> Option<(i32, u32, f64)> {
        self.obstacles
            .as_ref()?
            .highest_record_within(lat, lon, radius_m)
    }

    pub fn has_terrain(&self) -> bool {
        self.tile_dir.is_some()
    }

    pub fn has_obstacles(&self) -> bool {
        self.obstacles.is_some()
    }

    pub fn has_chart(&self) -> bool {
        self.chart.is_some()
    }

    /// Bytes the reduced terrain holds.
    pub fn terrain_bytes(&self) -> usize {
        self.coarse.bytes()
    }

    /// Reduces the tile of a cell into the coarse grid if that is not done; a cell with no file is
    /// noted (once). A tile that is there and cannot be read is an error, never a missing one.
    fn ensure_tile(&mut self, cell: (i32, i32)) -> Result<(), String> {
        if self.preloaded
            || self.loaded.contains(&cell)
            || self.absent.contains(&cell)
            || self.refused.contains(&cell)
        {
            return Ok(());
        }
        let Some(dir) = self.tile_dir.clone() else {
            return Ok(());
        };
        match tile_name(cell.0, cell.1).map(|n| dir.join(n)) {
            Some(p) => {
                let sealed = Sealed::production();
                refuse_sealed(&sealed, &p)?;
                if !p.is_file() {
                    self.absent.insert(cell);
                    self.stats.tiles_absent += 1;
                    return Ok(());
                }
                let t0 = Instant::now();
                let (g, hash) = match self.terrain_limits {
                    Some(limits) => {
                        let (grid, hash) = read_tile_bounded(&p, limits.max_file_bytes)?;
                        (grid, Some(hash))
                    }
                    None => {
                        let bytes = oa_tiff::read_file(&p)
                            .map_err(|e| format!("{}: {e}", p.display()))?;
                        let grid = oa_tiff::read_geotiff(&bytes)
                            .map_err(|e| format!("{}: {e}", p.display()))?;
                        (grid, None)
                    }
                };
                if let Some(limits) = self.terrain_limits {
                    let span_lat = (f64::from(g.height.saturating_sub(1)) * g.dlat).abs();
                    let span_lon = (f64::from(g.width.saturating_sub(1)) * g.dlon).abs();
                    let south = g.lat0 - span_lat;
                    let west = g.lon0;
                    let block_rows = ((g.lat0 * 500.0).floor() as i32)
                        .div_euclid(500)
                        .abs_diff(((south * 500.0).floor() as i32).div_euclid(500))
                        as usize
                        + 1;
                    let block_cols = ((g.lon0 * 500.0).floor() as i32)
                        .div_euclid(500)
                        .abs_diff((((west + span_lon) * 500.0).floor() as i32).div_euclid(500))
                        as usize
                        + 1;
                    let potential_bytes = block_rows
                        .saturating_mul(block_cols)
                        .saturating_mul(500 * 500 * 2);
                    if span_lat > 1.1
                        || span_lon > 1.1
                        || self.coarse.bytes().saturating_add(potential_bytes)
                            > limits.max_resident_bytes
                    {
                        self.refused.insert(cell);
                        self.stats.tiles_refused += 1;
                        self.stats.terrain_budget_refused += 1;
                        return Ok(());
                    }
                }
                let t1 = Instant::now();
                self.coarse.add_grid(&g);
                if let Some(hash) = hash {
                    self.terrain_hashes.push((p.display().to_string(), hash));
                    self.terrain_hashes.sort_by(|a, b| a.0.cmp(&b.0));
                }
                self.stats.ns_tile_reduce += t1.elapsed().as_nanos() as u64;
                self.stats.terrain_grid_bytes = self.coarse.bytes() as u64;
                self.stats.ns_tile_read += t0.elapsed().as_nanos() as u64;
                self.loaded.insert(cell);
                self.stats.tiles_loaded += 1;
            }
            None => {
                self.absent.insert(cell);
                self.stats.tiles_absent += 1;
            }
        }
        Ok(())
    }

    /// The extent of the obstacle file (south, north, west, east, 1e-7 degree) and how many states
    /// and countries it names; `None` if there is none or no extent was given.
    /// Whether the obstacle data covers the point itself; `None` where there is no obstacle file.
    pub fn obstacle_covers_point(&self, lat: f64, lon: f64) -> Option<bool> {
        self.obstacles.as_ref().map(|o| o.covers(lat, lon, 0.0))
    }

    /// The covered cells of the obstacle data.
    pub fn obstacle_cells(&self) -> Option<usize> {
        self.obstacles.as_ref().and_then(|o| o.covered_cells())
    }

    pub fn obstacle_extent(&self) -> Option<((i32, i32, i32, i32), usize)> {
        self.obstacles
            .as_ref()
            .and_then(|o| o.extent.map(|e| (e, o.regions)))
    }

    /// The names of the tiles that a query needed and the directory did not hold.
    pub fn absent_tiles(&self) -> Vec<String> {
        self.absent
            .iter()
            .filter_map(|&(la, lo)| tile_name(la, lo))
            .collect()
    }

    fn terrain_ft(&mut self, lat: f64, lon: f64, radius_m: f64) -> Result<i32, String> {
        let t0 = Instant::now();
        let tile0 = self.stats.ns_tile_read;
        let r = self.terrain_query(lat, lon, radius_m);
        let spent = t0.elapsed().as_nanos() as u64;
        self.stats.ns_terrain += spent.saturating_sub(self.stats.ns_tile_read - tile0);
        r
    }

    fn terrain_query(&mut self, lat: f64, lon: f64, radius_m: f64) -> Result<i32, String> {
        self.stats.terrain_queries += 1;
        let (i0, i1, j0, j1) = CoarseTerrain::cell_box(lat, lon, radius_m);
        if let Some(limits) = self.terrain_limits {
            let rows = i64::from(i1) - i64::from(i0) + 1;
            let cols = i64::from(j1) - i64::from(j0) + 1;
            let visits = if rows <= 0 || cols <= 0 {
                u64::MAX
            } else {
                (rows as u64).saturating_mul(cols as u64)
            };
            if visits > limits.max_query_cells {
                self.stats.terrain_absent += 1;
                self.stats.terrain_refused += 1;
                self.stats.terrain_budget_refused += 1;
                return Ok(FLOOR_NONE);
            }
        }
        let per_deg = CELLS_PER_DEG as i32;
        if !self.preloaded && (i1 - i0 + 1).max(j1 - j0 + 1) < 10 * per_deg {
            if let Some(limits) = self.terrain_limits {
                let (la0, la1) = (i0.div_euclid(per_deg), i1.div_euclid(per_deg));
                let (lo0, lo1) = (j0.div_euclid(per_deg), j1.div_euclid(per_deg));
                let mut candidates = BTreeSet::new();
                for la in la0..=la1 {
                    for lo in lo0..=lo1 {
                        candidates.insert((la, lo));
                        if candidates.len() > limits.max_tiles.saturating_add(1) {
                            break;
                        }
                    }
                    if candidates.len() > limits.max_tiles.saturating_add(1) {
                        break;
                    }
                }
                let attempts = self.loaded.len() + self.absent.len() + self.refused.len();
                let new_needed = candidates
                    .iter()
                    .filter(|c| {
                        !self.loaded.contains(c)
                            && !self.absent.contains(c)
                            && !self.refused.contains(c)
                    })
                    .count();
                if attempts.saturating_add(new_needed) > limits.max_tiles {
                    self.stats.terrain_absent += 1;
                    self.stats.terrain_refused += 1;
                    self.stats.terrain_budget_refused += 1;
                    return Ok(FLOOR_NONE);
                }
            }
            for la in i0.div_euclid(per_deg)..=i1.div_euclid(per_deg) {
                for lo in j0.div_euclid(per_deg)..=j1.div_euclid(per_deg) {
                    self.ensure_tile((la, lo))?;
                }
            }
        }
        match self.coarse.lookup(lat, lon, radius_m) {
            Coarse::Peak(ft) => Ok(ft),
            Coarse::Missing => {
                self.stats.terrain_absent += 1;
                self.stats.terrain_missing += 1;
                Ok(FLOOR_NONE)
            }
            Coarse::Nodata => {
                self.stats.terrain_absent += 1;
                self.stats.terrain_nodata += 1;
                Ok(FLOOR_NONE)
            }
            Coarse::TooLarge => {
                self.stats.terrain_absent += 1;
                self.stats.terrain_refused += 1;
                Ok(FLOOR_NONE)
            }
        }
    }

    fn chart_ft(&mut self, lat: f64, lon: f64) -> i32 {
        let Some(r) = &mut self.chart else {
            return FLOOR_NONE;
        };
        let best = r.at(lat, lon).unwrap_or(FLOOR_NONE);
        self.stats.chart_blocks = r.blocks() as u64;
        if best == FLOOR_NONE {
            self.stats.chart_absent += 1;
        }
        best
    }

    /// Nothing to prepare: every lookup is made at the point itself with its own radius.
    pub fn prepare(&mut self, _points: &[SamplePoint]) -> Result<(), String> {
        Ok(())
    }

    /// The floor sample at a point, looked up at the point itself over its own radius (no cell and no
    /// radius bucket: the circle is exactly the point's). A terrain file that cannot be read is an
    /// error.
    pub fn floor_at(&mut self, p: &SamplePoint) -> Result<FloorSample, String> {
        self.stats.points += 1;
        let (lat, lon) = (f64::from(p.lat_e7) / 1e7, f64::from(p.lon_e7) / 1e7);
        let radius = f64::from(p.radius_m);
        let t0 = Instant::now();
        let mva_ft = self.chart_ft(lat, lon);
        self.stats.ns_chart += t0.elapsed().as_nanos() as u64;
        let terrain_ft = if self.tile_dir.is_some() {
            self.terrain_ft(lat, lon, radius)?
        } else {
            FLOOR_NONE
        };
        let t1 = Instant::now();
        let obstacle_ft = self.obstacle_top(lat, lon, radius, p.track_deg);
        self.stats.ns_obstacle += t1.elapsed().as_nanos() as u64;
        let gtm_clearance_ft = self.gtm_at(lat, lon, radius);
        let (apt_clearance_ft, apt_ceiling_ft) = self.apt_at(lat, lon, radius);
        Ok(FloorSample {
            terrain_ft,
            obstacle_ft,
            obstacle_covered: self
                .obstacles
                .as_ref()
                .is_some_and(|o| o.covers(lat, lon, radius)),
            mva_ft,
            gtm_clearance_ft,
            apt_clearance_ft,
            apt_ceiling_ft,
        })
    }
}
