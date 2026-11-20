//! The terrain a lookup reads: a grid of 0.002-degree cells, each the highest elevation of every
//! source sample whose area touches the cell (a sample's area reaches one sample step beyond its
//! position, the reach of `oa-tiff`'s conservative query). Built once from a tile when it is first
//! used and kept instead of the tile, so a lookup reads a few hundred cells and not the samples
//! under the circle. Shell (B3).
//!
//! A cell is only given a value by a grid that holds every sample that could touch it; a cell no
//! grid holds whole is missing, and a cell that touches a nodata sample is nodata. A lookup is the
//! highest value over the cells the circle's bounding box touches, widened by one cell, so it is
//! never below the fine query over the same circle (`oa-tiff`'s conservative maximum, the oracle
//! of the tests); it is also never given where the fine query is not.

use std::collections::HashMap;

use oa_tiff::Grid;

use crate::floors::ceil_ft;

/// Cells per degree (0.002 degree each).
pub const CELLS_PER_DEG: f64 = 500.0;
/// Cells along one side of a one-degree block.
const BLOCK_CELLS: i32 = 500;
/// A cell no grid holds whole.
pub const MISSING: i16 = i16::MIN;
/// A cell that touches a nodata sample (also the largest value, so that the highest wins).
pub const NODATA: i16 = i16::MAX;
/// The values a cell may hold, feet; the ends are the two markers above.
const FT_LOW: i32 = -32_000;
const FT_HIGH: i32 = 32_000;
/// Metres per degree of latitude, the smallest anywhere (at the equator).
const M_PER_DEG_LAT_MIN: f64 = 110_500.0;
/// The most cells one lookup reads; a larger circle is refused.
const MAX_LOOKUP_CELLS: i64 = 250_000;
/// Slack on a sample step and on a position, degrees (the lattice is not exact to the last bit).
const STEP_SLACK: f64 = 1.0e-6;
const POS_SLACK_DEG: f64 = 1.0e-9;

/// What a lookup found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Coarse {
    /// The highest terrain under the circle, feet, rounded up.
    Peak(i32),
    /// A cell of the circle is held by no grid.
    Missing,
    /// A cell of the circle touches a nodata sample (and none is missing).
    Nodata,
    /// The circle is too large for a lookup.
    TooLarge,
}

/// The cell grids by one-degree block (the block's south-west corner, whole degrees).
// A7.2: cold struct, size guard waived
#[derive(Debug, Default)]
pub struct CoarseTerrain {
    blocks: HashMap<(i32, i32), Vec<i16>>,
}

fn cell_of(deg: f64) -> i32 {
    (deg * CELLS_PER_DEG).floor() as i32
}

fn to_cell_ft(metres_max: f32, any_nodata: bool) -> i16 {
    if any_nodata {
        return NODATA;
    }
    ceil_ft(f64::from(metres_max)).clamp(FT_LOW, FT_HIGH) as i16
}

impl CoarseTerrain {
    pub fn blocks(&self) -> usize {
        self.blocks.len()
    }

    /// Bytes held by the cell grids.
    pub fn bytes(&self) -> usize {
        self.blocks.len() * (BLOCK_CELLS as usize).pow(2) * 2
    }

    /// Folds `g` in: every cell that `g` holds whole takes the highest sample touching it (and the
    /// highest of what it had). A cell reached past the grid's edge is left as it was.
    pub fn add_grid(&mut self, g: &Grid) {
        if g.width == 0 || g.height == 0 || !(g.dlat > 0.0 && g.dlon > 0.0) {
            return;
        }
        let (w, h) = (g.width as usize, g.height as usize);
        if g.elev.len() != w * h {
            return;
        }
        let pad_lat = g.dlat * (1.0 + STEP_SLACK) + POS_SLACK_DEG;
        let pad_lon = g.dlon * (1.0 + STEP_SLACK) + POS_SLACK_DEG;
        let lat_s = g.lat0 - (h as f64 - 1.0) * g.dlat;
        let lon_e = g.lon0 + (w as f64 - 1.0) * g.dlon;
        let step = 1.0 / CELLS_PER_DEG;
        for gi in cell_of(lat_s)..=cell_of(g.lat0) {
            let (lo, hi) = (
                f64::from(gi) * step - pad_lat,
                f64::from(gi + 1) * step + pad_lat,
            );
            if lo < lat_s || hi > g.lat0 {
                continue;
            }
            let r0 = (((g.lat0 - hi) / g.dlat).ceil().max(0.0)) as usize;
            let r1 = (((g.lat0 - lo) / g.dlat).floor().min(h as f64 - 1.0)) as usize;
            for gj in cell_of(g.lon0)..=cell_of(lon_e) {
                let (wl, wh) = (
                    f64::from(gj) * step - pad_lon,
                    f64::from(gj + 1) * step + pad_lon,
                );
                if wl < g.lon0 || wh > lon_e {
                    continue;
                }
                let c0 = (((wl - g.lon0) / g.dlon).ceil().max(0.0)) as usize;
                let c1 = (((wh - g.lon0) / g.dlon).floor().min(w as f64 - 1.0)) as usize;
                let mut best = f32::NEG_INFINITY;
                let mut nodata = false;
                for r in r0..=r1 {
                    for &v in g.elev.get(r * w + c0..=r * w + c1).unwrap_or(&[]) {
                        if v.is_nan() {
                            nodata = true;
                        } else if v > best {
                            best = v;
                        }
                    }
                }
                if best == f32::NEG_INFINITY && !nodata {
                    continue;
                }
                let v = to_cell_ft(best, nodata);
                let key = (gi.div_euclid(BLOCK_CELLS), gj.div_euclid(BLOCK_CELLS));
                let block = self
                    .blocks
                    .entry(key)
                    .or_insert_with(|| vec![MISSING; (BLOCK_CELLS * BLOCK_CELLS) as usize]);
                let at = (gi.rem_euclid(BLOCK_CELLS) * BLOCK_CELLS + gj.rem_euclid(BLOCK_CELLS))
                    as usize;
                if let Some(cell) = block.get_mut(at) {
                    if *cell == MISSING || v > *cell {
                        *cell = v;
                    }
                }
            }
        }
    }

    /// The cells (south-west to north-east, inclusive) a circle of `radius_m` at (`lat`, `lon`)
    /// touches by its bounding box (taken with the smallest metres per degree, which only widens
    /// it), widened by one cell.
    pub fn cell_box(lat: f64, lon: f64, radius_m: f64) -> (i32, i32, i32, i32) {
        let cos = lat.to_radians().cos().max(0.01);
        let dlat = radius_m / M_PER_DEG_LAT_MIN;
        let dlon = radius_m / (M_PER_DEG_LAT_MIN * cos);
        (
            cell_of(lat - dlat) - 1,
            cell_of(lat + dlat) + 1,
            cell_of(lon - dlon) - 1,
            cell_of(lon + dlon) + 1,
        )
    }

    /// The highest terrain over the cells the circle touches (see [`Self::cell_box`]).
    pub fn lookup(&self, lat: f64, lon: f64, radius_m: f64) -> Coarse {
        if !(lat.is_finite() && lon.is_finite() && radius_m.is_finite() && radius_m >= 0.0) {
            return Coarse::TooLarge;
        }
        let (i0, i1, j0, j1) = Self::cell_box(lat, lon, radius_m);
        if i64::from(i1 - i0 + 1) * i64::from(j1 - j0 + 1) > MAX_LOOKUP_CELLS {
            return Coarse::TooLarge;
        }
        let (mut best, mut missing, mut nodata) = (i16::MIN + 1, false, false);
        // The block of the cells of one row is looked up once for each run of cells in it.
        for i in i0..=i1 {
            let (bi, ri) = (i.div_euclid(BLOCK_CELLS), i.rem_euclid(BLOCK_CELLS));
            let mut j = j0;
            while j <= j1 {
                let bj = j.div_euclid(BLOCK_CELLS);
                let end = j1.min((bj + 1) * BLOCK_CELLS - 1);
                match self.blocks.get(&(bi, bj)) {
                    None => missing = true,
                    Some(b) => {
                        let at = (ri * BLOCK_CELLS + j.rem_euclid(BLOCK_CELLS)) as usize;
                        let run = b.get(at..at + (end - j + 1) as usize).unwrap_or(&[]);
                        if run.len() != (end - j + 1) as usize {
                            missing = true;
                        }
                        for &v in run {
                            match v {
                                MISSING => missing = true,
                                NODATA => nodata = true,
                                v => best = best.max(v),
                            }
                        }
                    }
                }
                j = end + 1;
            }
        }
        if missing {
            Coarse::Missing
        } else if nodata {
            Coarse::Nodata
        } else {
            Coarse::Peak(i32::from(best))
        }
    }
}
