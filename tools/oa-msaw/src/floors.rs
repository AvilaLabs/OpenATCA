//! The floor lookups the core asks for: terrain, obstacles and charts at a sample point. Shell
//! (B3): it reads files. Every answer is conservative (a number that is too high, never too low)
//! or absent:
//!
//! * Terrain is the highest elevation of every cell that touches the circle of the point's radius
//!   (`oa-tiff`'s conservative query), rounded up to whole feet. A point is looked up in a cell of
//!   0.002 degrees and a radius bucket of 250 m: the query is made at the cell's centre with the
//!   bucket's radius plus the cell's half diagonal, which covers every circle in the cell and
//!   bucket. If any sample under the circle is nodata or in a tile that is not loaded, the answer
//!   is absent: the lower bound the query still holds is never used.
//! * Obstacles are the highest top of every obstacle within the circle (distances taken with the
//!   smallest metre-per-degree scales, which only adds obstacles); none is not an absence of data.
//! * The chart is the highest minimum altitude of the FUS3, FUS5 and TAV sectors that contain the
//!   point, absent where no sector of any of them does.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

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
    /// Terrain queries that gave no complete answer (a tile absent or nodata under the circle).
    pub terrain_absent: u64,
    pub tiles_loaded: u64,
    pub tiles_absent: u64,
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
    loaded: BTreeSet<(i32, i32)>,
    absent: BTreeSet<(i32, i32)>,
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
            loaded: BTreeSet::new(),
            absent: BTreeSet::new(),
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

    /// Loads the tiles of the cells the circle's box touches (a tile that is not there is noted,
    /// once). A tile that is there and cannot be read is an error, never a missing one.
    fn ensure_tiles(&mut self, lat: f64, lon: f64, radius_m: f64) -> Result<(), String> {
        let Some(dir) = self.tile_dir.clone() else {
            return Ok(());
        };
        if self.preloaded {
            return Ok(());
        }
        let dlat = radius_m / M_PER_DEG_LAT_MIN + 0.001;
        let dlon = radius_m / (M_PER_DEG_LAT_MIN * lat.to_radians().cos().max(0.01)) + 0.001;
        for la in (lat - dlat).floor() as i32..=(lat + dlat).floor() as i32 {
            for lo in (lon - dlon).floor() as i32..=(lon + dlon).floor() as i32 {
                if self.loaded.contains(&(la, lo)) || self.absent.contains(&(la, lo)) {
                    continue;
                }
                let name = tile_name(la, lo);
                let path = name.map(|n| dir.join(n));
                match path {
                    Some(p) if p.is_file() => {
                        self.set.tiles.push(read_tile(&p)?);
                        self.loaded.insert((la, lo));
                        self.stats.tiles_loaded += 1;
                    }
                    _ => {
                        self.absent.insert((la, lo));
                        self.stats.tiles_absent += 1;
                    }
                }
            }
        }
        Ok(())
    }

    fn terrain_ft(&mut self, lat: f64, lon: f64, radius_m: f64) -> Result<i32, String> {
        self.ensure_tiles(lat, lon, radius_m)?;
        self.stats.terrain_queries += 1;
        let nm = (radius_m / NM_M).max(1.0e-6);
        match max_elevation_in_radius_conservative(&self.set, lat, lon, nm) {
            Ok(TerrainMax::Complete { peak, .. }) => Ok(ceil_ft(f64::from(peak.metres))),
            _ => {
                self.stats.terrain_absent += 1;
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

    /// The floor sample at a point. A terrain file that cannot be read is an error.
    pub fn floor_at(&mut self, p: &SamplePoint) -> Result<FloorSample, String> {
        self.stats.points += 1;
        let (lat, lon) = (f64::from(p.lat_e7) / 1e7, f64::from(p.lon_e7) / 1e7);
        let mva_ft = self.chart_ft(lat, lon);
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
            let o = self
                .obstacles
                .as_ref()
                .map_or(FLOOR_NONE, |ix| ix.highest_within(clat, clon, radius));
            self.cache.insert(key, (t, o));
            (t, o)
        };
        Ok(FloorSample {
            terrain_ft,
            obstacle_ft,
            mva_ft,
        })
    }
}
