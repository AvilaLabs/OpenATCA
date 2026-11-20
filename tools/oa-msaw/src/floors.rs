//! The floor lookups the core asks for: terrain, obstacles and charts at a sample point. Shell
//! (B3): it reads files. Every answer is conservative (a number that is too high, never too low)
//! or absent:
//!
//! * Terrain is the highest elevation of every cell that touches the circle of the point's radius
//!   (`oa-tiff`'s conservative query), rounded up to whole feet. A point is looked up in a cell of
//!   0.002 degrees and a radius bucket of 250 m: the query is made at the cell's centre with the
//!   bucket's radius plus the cell's half diagonal, which covers every circle in the cell and
//!   bucket. If any sample under the circle is nodata or in a tile that is not loaded, the answer
//!   is absent: the lower bound the query still holds is never used. Each query is made over a
//!   tile set of only the tiles of the one-degree cells its box touches, so that a tile whose
//!   lattice does not match its neighbours spoils only the queries that touch it; that is counted
//!   and reported on stderr, never silent.
//! * Obstacles are the highest top of every obstacle within the circle (distances taken with the
//!   smallest metre-per-degree scales, which only adds obstacles); none is not an absence of data.
//! * The chart is the highest minimum altitude of the FUS3, FUS5 and TAV sectors that contain the
//!   point, absent where no sector of any of them does.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::Instant;

use oa_dof::Dof;
use oa_msaw::{FloorSample, SamplePoint, FLOOR_NONE};
use oa_mva::{min_altitude_at, Model, SetKind};
use oa_sealed::Sealed;
use oa_tiff::{max_elevation_in_radius_conservative, Grid, TerrainMax, TileSet, METRES_PER_FOOT};

use crate::guard::refuse_sealed;

/// The cell of a lookup, degrees.
const CELL_DEG: f64 = 0.002;
/// Half the diagonal of a cell, metres, at the equator (the largest): 0.002 degrees is 222.4 m of
/// latitude and at most as much of longitude.
const CELL_HALF_DIAG_M: f64 = 158.0;
/// The most tiles held in memory at once (a 3DEP tile is some 50 MB, a facility's 99 tiles over
/// 5 GB); the least recently used is dropped and read again if it is needed. A smaller number
/// than the tiles in use thrashes: every track crosses a new lookup cell every second, so the
/// queries of one cycle are spread over the whole facility and each tile is wanted every cycle
/// (a tile read is 0.8 s).
const MAX_RESIDENT_TILES: usize = 100;
/// The most blocks whose misaligned tiles are described on stderr; the rest are only counted.
const MAX_MISALIGN_NOTES: usize = 40;
/// The radius bucket, metres.
const RADIUS_STEP_M: f64 = 250.0;
/// The obstacle index cell, degrees.
const OBST_CELL_DEG: f64 = 0.1;
/// Metres per degree of latitude, the smallest anywhere (at the equator).
const M_PER_DEG_LAT_MIN: f64 = 110_500.0;
const NM_M: f64 = 1852.0;

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
    /// The tiles of the query do not share one sample lattice.
    pub terrain_misaligned: u64,
    /// Any other refusal of the query.
    pub terrain_refused: u64,
    pub tiles_loaded: u64,
    pub tiles_absent: u64,
    /// Tiles read again after being dropped for room.
    pub tiles_reloaded: u64,
    /// Wall time, nanoseconds: in the terrain query itself, reading tiles from disk, in the
    /// obstacle index and in the chart.
    pub ns_terrain: u64,
    pub ns_tile_read: u64,
    pub ns_obstacle: u64,
    pub ns_chart: u64,
    /// Distinct blocks of tiles (the tiles one query touches) found not to share one lattice.
    pub blocks_misaligned: u64,
    pub chart_absent: u64,
}

/// One obstacle: latitude and longitude in 1e-7 degree, top in feet MSL.
type Obstacle = (i32, i32, i32);

/// The obstacles in a grid of cells for the circle query.
// A7.2: cold struct, size guard waived
#[derive(Debug, Default)]
pub struct ObstacleIndex {
    cells: HashMap<(i32, i32), Vec<Obstacle>>,
    pub count: usize,
}

fn cell_of(deg: f64, size: f64) -> i32 {
    (deg / size).floor() as i32
}

/// Indexes the obstacles of `d` (position 1e-7 degree, top feet above mean sea level).
pub fn obstacle_index(d: &Dof) -> ObstacleIndex {
    ObstacleIndex::from_points(
        d.lat_e7
            .iter()
            .zip(d.lon_e7.iter())
            .zip(d.amsl_ft.iter())
            .map(|((&la, &lo), &top)| (la, lo, top)),
    )
}

impl ObstacleIndex {
    /// An index of obstacles given as (latitude 1e-7 degree, longitude 1e-7 degree, top feet MSL).
    pub fn from_points(points: impl Iterator<Item = (i32, i32, i32)>) -> ObstacleIndex {
        let mut ix = ObstacleIndex::default();
        for (la, lo, top) in points {
            let key = (
                cell_of(f64::from(la) / 1e7, OBST_CELL_DEG),
                cell_of(f64::from(lo) / 1e7, OBST_CELL_DEG),
            );
            ix.cells.entry(key).or_default().push((la, lo, top));
            ix.count += 1;
        }
        ix
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
                for &(la, lo, top) in v {
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

/// The lookups with their data and cache. A7.2: cold struct, size guard waived.
pub struct Floors {
    tile_dir: Option<PathBuf>,
    /// The tiles were given whole (a test), none are read from `tile_dir`.
    preloaded: bool,
    set: TileSet,
    /// The tiles read, by the south-west corner of their cell, and when each was last used.
    resident: HashMap<(i32, i32), (Grid, u64)>,
    ever_loaded: BTreeSet<(i32, i32)>,
    absent: BTreeSet<(i32, i32)>,
    /// The blocks of cells found misaligned (reported once each).
    misaligned: BTreeSet<Vec<(i32, i32)>>,
    clock: u64,
    obstacles: Option<ObstacleIndex>,
    chart: Option<Model>,
    cache: HashMap<(i32, i32, u32), (i32, i32)>,
    pub stats: FloorStats,
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

fn read_tile(path: &Path) -> Result<Grid, String> {
    refuse_sealed(Sealed::production(), path)?;
    let bytes = oa_tiff::read_file(path).map_err(|e| format!("{}: {e}", path.display()))?;
    oa_tiff::read_geotiff(&bytes).map_err(|e| format!("{}: {e}", path.display()))
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
            set: TileSet::default(),
            resident: HashMap::new(),
            ever_loaded: BTreeSet::new(),
            absent: BTreeSet::new(),
            misaligned: BTreeSet::new(),
            clock: 0,
            obstacles,
            chart,
            cache: HashMap::new(),
            stats: FloorStats::default(),
        }
    }

    /// Lookups over tiles held in memory (a test hands them over); no file is read.
    pub fn with_tiles(
        set: TileSet,
        obstacles: Option<ObstacleIndex>,
        chart: Option<Model>,
    ) -> Floors {
        let mut f = Floors::new(Some(PathBuf::new()), obstacles, chart);
        f.preloaded = true;
        f.set = set;
        f
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

    /// The cells (south-west corner, whole degrees) the circle's box touches, in order.
    fn cells_of(lat: f64, lon: f64, radius_m: f64) -> Vec<(i32, i32)> {
        let dlat = radius_m / M_PER_DEG_LAT_MIN + 0.001;
        let dlon = radius_m / (M_PER_DEG_LAT_MIN * lat.to_radians().cos().max(0.01)) + 0.001;
        let mut cells = Vec::new();
        for la in (lat - dlat).floor() as i32..=(lat + dlat).floor() as i32 {
            for lo in (lon - dlon).floor() as i32..=(lon + dlon).floor() as i32 {
                cells.push((la, lo));
            }
        }
        cells
    }

    /// The grid of a cell, from memory or from its file; `None` if there is no file (noted, once).
    /// A tile that is there and cannot be read is an error, never a missing one.
    fn grid_of(&mut self, cell: (i32, i32)) -> Result<Option<Grid>, String> {
        if let Some((g, _)) = self.resident.remove(&cell) {
            return Ok(Some(g));
        }
        if self.absent.contains(&cell) {
            return Ok(None);
        }
        let Some(dir) = self.tile_dir.clone() else {
            return Ok(None);
        };
        let path = tile_name(cell.0, cell.1).map(|n| dir.join(n));
        match path {
            Some(p) if p.is_file() => {
                let t0 = Instant::now();
                let g = read_tile(&p)?;
                self.stats.ns_tile_read += t0.elapsed().as_nanos() as u64;
                if self.ever_loaded.insert(cell) {
                    self.stats.tiles_loaded += 1;
                } else {
                    self.stats.tiles_reloaded += 1;
                }
                Ok(Some(g))
            }
            _ => {
                self.absent.insert(cell);
                self.stats.tiles_absent += 1;
                Ok(None)
            }
        }
    }

    /// Says on stderr, once for the block, which tile does not match the first (spacing ratio and
    /// the offset from the first tile's lattice in steps), and counts the block.
    fn note_misaligned(&mut self, block: &[(i32, i32)], grids: &[Grid]) {
        if !self.misaligned.insert(block.to_vec()) {
            return;
        }
        self.stats.blocks_misaligned += 1;
        if self.misaligned.len() > MAX_MISALIGN_NOTES {
            return;
        }
        let Some(r) = grids.first() else {
            return;
        };
        for (cell, g) in block.iter().zip(grids.iter()).skip(1) {
            let (rl, rt) = (g.dlon / r.dlon - 1.0, g.dlat / r.dlat - 1.0);
            let fc = (g.lon0 - r.lon0) / r.dlon;
            let fr = (r.lat0 - g.lat0) / r.dlat;
            // Only a tile that fails the check against the first is named: the spacing more than
            // a part in 1e9 apart, or a position not a whole number of steps (to a thousandth).
            let spacing_bad = rl.abs() > 1.0e-9 || rt.abs() > 1.0e-9;
            let offset_bad = (fc - fc.round()).abs() > 1.0e-3 || (fr - fr.round()).abs() > 1.0e-3;
            if !(spacing_bad || offset_bad) {
                continue;
            }
            eprintln!(
                "oa-msaw: terrain tile of cell {},{} does not share the lattice of cell {},{}: \
                 spacing ratio - 1 = {:.3e} (lon) {:.3e} (lat), offset {:.6} columns {:.6} rows, \
                 {:.6} and {:.6} from a whole number of steps",
                cell.0,
                cell.1,
                block.first().map_or(0, |c| c.0),
                block.first().map_or(0, |c| c.1),
                rl,
                rt,
                fc,
                fr,
                fc - fc.round(),
                fr - fr.round()
            );
        }
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
        let nm = (radius_m / NM_M).max(1.0e-6);
        let result = if self.preloaded {
            max_elevation_in_radius_conservative(&self.set, lat, lon, nm)
        } else {
            // Only the tiles of the cells the box touches (at most a handful), in cell order so
            // that the reference lattice of the set is the same for the same block.
            let cells = Self::cells_of(lat, lon, radius_m);
            let mut set = TileSet::default();
            let mut present = Vec::new();
            let mut failed = None;
            for &c in &cells {
                match self.grid_of(c) {
                    Ok(Some(g)) => {
                        present.push(c);
                        set.tiles.push(g);
                    }
                    Ok(None) => {}
                    Err(e) => {
                        failed = Some(e);
                        break;
                    }
                }
            }
            let r = if failed.is_none() {
                max_elevation_in_radius_conservative(&set, lat, lon, nm)
            } else {
                Err(oa_tiff::QueryError::NoTiles)
            };
            if matches!(r, Err(oa_tiff::QueryError::TilesNotAligned)) {
                self.note_misaligned(&present, &set.tiles);
            }
            // The grids go back, marked as just used; the least recently used are dropped.
            self.clock += 1;
            for (c, g) in present.into_iter().zip(set.tiles) {
                self.resident.insert(c, (g, self.clock));
            }
            while self.resident.len() > MAX_RESIDENT_TILES {
                let oldest = self
                    .resident
                    .iter()
                    .min_by_key(|(c, (_, t))| (*t, **c))
                    .map(|(c, _)| *c);
                match oldest {
                    Some(c) => {
                        self.resident.remove(&c);
                    }
                    None => break,
                }
            }
            if let Some(e) = failed {
                return Err(e);
            }
            r
        };
        match result {
            Ok(TerrainMax::Complete { peak, .. }) => Ok(ceil_ft(f64::from(peak.metres))),
            Ok(TerrainMax::Incomplete {
                missing_nodes,
                nodata_nodes,
                ..
            }) => {
                self.stats.terrain_absent += 1;
                if missing_nodes > 0 {
                    self.stats.terrain_missing += 1;
                } else if nodata_nodes > 0 {
                    self.stats.terrain_nodata += 1;
                } else {
                    self.stats.terrain_refused += 1;
                }
                Ok(FLOOR_NONE)
            }
            Err(oa_tiff::QueryError::TilesNotAligned) => {
                self.stats.terrain_absent += 1;
                self.stats.terrain_misaligned += 1;
                Ok(FLOOR_NONE)
            }
            Err(_) => {
                self.stats.terrain_absent += 1;
                self.stats.terrain_refused += 1;
                Ok(FLOOR_NONE)
            }
        }
    }

    fn chart_ft(&mut self, lat: f64, lon: f64) -> i32 {
        let Some(m) = &self.chart else {
            return FLOOR_NONE;
        };
        let mut best = FLOOR_NONE;
        for set in [SetKind::Fus3, SetKind::Fus5, SetKind::Tav] {
            if let Ok(Some(a)) = min_altitude_at(m, lat, lon, set) {
                best = best.max(a.min_alt_ft);
            }
        }
        if best == FLOOR_NONE {
            self.stats.chart_absent += 1;
        }
        best
    }

    /// The cache key of a point and the query it stands for: the cell, the radius bucket, and the
    /// centre and radius the terrain and obstacle lookups are made at.
    fn key_of(p: &SamplePoint) -> ((i32, i32, u32), f64, f64, f64) {
        let (lat, lon) = (f64::from(p.lat_e7) / 1e7, f64::from(p.lon_e7) / 1e7);
        let bucket = (f64::from(p.radius_m) / RADIUS_STEP_M).ceil().max(1.0);
        let key = (
            cell_of(lat, CELL_DEG),
            cell_of(lon, CELL_DEG),
            bucket as u32,
        );
        let (clat, clon) = (
            (f64::from(key.0) + 0.5) * CELL_DEG,
            (f64::from(key.1) + 0.5) * CELL_DEG,
        );
        (key, clat, clon, bucket * RADIUS_STEP_M + CELL_HALF_DIAG_M)
    }

    /// Answers the terrain and obstacle lookups of `points` that are not cached yet, in the order
    /// of their cells (south to north, then west to east), so that a tile is read about once for
    /// the whole batch and not once for every time the points move on and back. The answers are
    /// the same as lookup by lookup (the cache is keyed by cell and radius bucket alone).
    pub fn prepare(&mut self, points: &[SamplePoint]) -> Result<(), String> {
        let mut todo: Vec<((i32, i32, u32), f64, f64, f64)> = points
            .iter()
            .map(Self::key_of)
            .filter(|k| !self.cache.contains_key(&k.0))
            .collect();
        todo.sort_by_key(|k| k.0);
        todo.dedup_by_key(|k| k.0);
        for (key, clat, clon, radius) in todo {
            let t = if self.tile_dir.is_some() {
                self.terrain_ft(clat, clon, radius)?
            } else {
                FLOOR_NONE
            };
            let t1 = Instant::now();
            let o = self
                .obstacles
                .as_ref()
                .map_or(FLOOR_NONE, |ix| ix.highest_within(clat, clon, radius));
            self.stats.ns_obstacle += t1.elapsed().as_nanos() as u64;
            self.cache.insert(key, (t, o));
        }
        Ok(())
    }

    /// The floor sample at a point. A terrain file that cannot be read is an error.
    pub fn floor_at(&mut self, p: &SamplePoint) -> Result<FloorSample, String> {
        self.stats.points += 1;
        let (lat, lon) = (f64::from(p.lat_e7) / 1e7, f64::from(p.lon_e7) / 1e7);
        let t0 = Instant::now();
        let mva_ft = self.chart_ft(lat, lon);
        self.stats.ns_chart += t0.elapsed().as_nanos() as u64;
        let bucket = (f64::from(p.radius_m) / RADIUS_STEP_M).ceil().max(1.0);
        let key = (
            cell_of(lat, CELL_DEG),
            cell_of(lon, CELL_DEG),
            bucket as u32,
        );
        let (terrain_ft, obstacle_ft) = if let Some(&v) = self.cache.get(&key) {
            self.stats.cache_hits += 1;
            v
        } else {
            let (clat, clon) = (
                (f64::from(key.0) + 0.5) * CELL_DEG,
                (f64::from(key.1) + 0.5) * CELL_DEG,
            );
            let radius = bucket * RADIUS_STEP_M + CELL_HALF_DIAG_M;
            let t = if self.tile_dir.is_some() {
                self.terrain_ft(clat, clon, radius)?
            } else {
                FLOOR_NONE
            };
            let t1 = Instant::now();
            let o = self
                .obstacles
                .as_ref()
                .map_or(FLOOR_NONE, |ix| ix.highest_within(clat, clon, radius));
            self.stats.ns_obstacle += t1.elapsed().as_nanos() as u64;
            self.cache.insert(key, (t, o));
            (t, o)
        };
        Ok(FloorSample {
            terrain_ft,
            obstacle_ft,
            obstacle_covered: self.obstacles.is_some(),
            mva_ft,
        })
    }
}
