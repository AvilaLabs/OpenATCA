//! The chart as a raster of 0.002-degree cells, each the highest minimum altitude of any FUS3,
//! FUS5 or TAV sector that touches the cell (so a cell on a sector's edge has the sector's
//! altitude: conservative), built once per one-degree block when a point first falls in it. A
//! cell no sector touches has no chart. Holes of a sector are ignored (a superset). Shell (B3).

use std::collections::HashMap;

use oa_mva::{ring_is_hole, ring_range, ring_vertices, sector_count, Model, SectorIx, SetKind};

use crate::coarse::CELLS_PER_DEG;

const BLOCK_CELLS: usize = 500;
/// A cell no sector touches.
pub const NO_CHART: i16 = i16::MIN;
/// Slack on a position, in cells.
const EPS: f64 = 1.0e-6;

/// The raster blocks by the south-west corner of their degree, over the chart they came from.
// A7.2: cold struct, size guard waived
pub struct ChartRaster {
    model: Model,
    blocks: HashMap<(i32, i32), Vec<i16>>,
}

fn set_of_interest(s: SetKind) -> bool {
    matches!(s, SetKind::Fus3 | SetKind::Fus5 | SetKind::Tav)
}

/// Marks the cells of row `row` between `xa` and `xb` (cell units, inclusive of the cells they
/// fall in) with `alt`.
fn mark_row(block: &mut [i16], row: usize, xa: f64, xb: f64, alt: i16) {
    let top = (BLOCK_CELLS - 1) as f64;
    let (a, b) = ((xa - EPS).floor().max(0.0), (xb + EPS).floor().min(top));
    if a > b {
        return;
    }
    for c in a as usize..=b as usize {
        if let Some(v) = block.get_mut(row * BLOCK_CELLS + c) {
            if alt > *v {
                *v = alt;
            }
        }
    }
}

/// Paints one outer ring (vertices in cell units of the block) with `alt`: every cell the ring
/// crosses or contains.
fn paint_ring(block: &mut [i16], xs: &[f64], ys: &[f64], alt: i16) {
    let n = xs.len().min(ys.len());
    if n < 2 {
        return;
    }
    let top = (BLOCK_CELLS - 1) as f64;
    let (ymin, ymax) = ys
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), &y| {
            (a.min(y), b.max(y))
        });
    let (r0, r1) = ((ymin - EPS).floor().max(0.0), (ymax + EPS).floor().min(top));
    if r0 > r1 {
        return;
    }
    let (r0, r1) = (r0 as usize, r1 as usize);
    // Crossings of each row's centre line, for the interior.
    let mut cross: Vec<Vec<f64>> = vec![Vec::new(); r1 - r0 + 1];
    for i in 0..n {
        let k = (i + 1) % n;
        let (Some((&x0, &y0)), Some((&x1, &y1))) =
            (xs.get(i).zip(ys.get(i)), xs.get(k).zip(ys.get(k)))
        else {
            continue;
        };
        let (ya, yb) = (y0.min(y1), y0.max(y1));
        let (e0, e1) = (
            (ya - EPS).floor().max(r0 as f64),
            (yb + EPS).floor().min(r1 as f64),
        );
        let mut row = e0;
        while row <= e1 {
            let r = row as usize;
            // The part of the edge inside the row band, for the cells it passes through.
            let (lo, hi) = (row - EPS, row + 1.0 + EPS);
            if yb > ya {
                let (ta, tb) = (((lo - y0) / (y1 - y0)), ((hi - y0) / (y1 - y0)));
                let (ta, tb) = (ta.min(tb).clamp(0.0, 1.0), ta.max(tb).clamp(0.0, 1.0));
                let (xa, xb) = (x0 + (x1 - x0) * ta, x0 + (x1 - x0) * tb);
                mark_row(block, r, xa.min(xb), xa.max(xb), alt);
                // The centre-line crossing, half-open so that a vertex counts once.
                let yc = row + 0.5;
                if (y0 <= yc) != (y1 <= yc) {
                    let x = x0 + (x1 - x0) * (yc - y0) / (y1 - y0);
                    if let Some(c) = cross.get_mut(r - r0) {
                        c.push(x);
                    }
                }
            } else {
                mark_row(block, r, x0.min(x1), x0.max(x1), alt);
            }
            row += 1.0;
        }
    }
    for (k, xs) in cross.iter_mut().enumerate() {
        xs.sort_by(|a, b| oa_math::cmp_canon_f64(*a, *b));
        for pair in xs.chunks_exact(2) {
            // Cells whose centre lies between the two crossings.
            if let [a, b] = *pair {
                mark_row(block, r0 + k, a - 0.5, b - 0.5, alt);
            }
        }
    }
}

impl ChartRaster {
    pub fn new(model: Model) -> ChartRaster {
        ChartRaster {
            model,
            blocks: HashMap::new(),
        }
    }

    pub fn blocks(&self) -> usize {
        self.blocks.len()
    }

    fn build(&self, lat0: i32, lon0: i32) -> Vec<i16> {
        let mut block = vec![NO_CHART; BLOCK_CELLS * BLOCK_CELLS];
        let m = &self.model;
        let e7 = |d: i32| i64::from(d) * 10_000_000;
        for i in 0..sector_count(m) {
            if !m.set.get(i).copied().is_some_and(set_of_interest) {
                continue;
            }
            let (Some(&a0), Some(&a1), Some(&b0), Some(&b1)) = (
                m.bbox_lat_min.get(i),
                m.bbox_lat_max.get(i),
                m.bbox_lon_min.get(i),
                m.bbox_lon_max.get(i),
            ) else {
                continue;
            };
            if i64::from(a1) < e7(lat0)
                || i64::from(a0) > e7(lat0 + 1)
                || i64::from(b1) < e7(lon0)
                || i64::from(b0) > e7(lon0 + 1)
            {
                continue;
            }
            let alt = m
                .min_alt_ft
                .get(i)
                .copied()
                .unwrap_or(i32::MAX)
                .clamp(i32::from(NO_CHART) + 1, i32::from(i16::MAX)) as i16;
            for r in ring_range(m, SectorIx(i as u32)) {
                if ring_is_hole(m, r) {
                    continue;
                }
                let (lats, lons) = ring_vertices(m, r);
                let ys: Vec<f64> = lats
                    .iter()
                    .map(|&v| (f64::from(v) * 1e-7 - f64::from(lat0)) * CELLS_PER_DEG)
                    .collect();
                let xs: Vec<f64> = lons
                    .iter()
                    .map(|&v| (f64::from(v) * 1e-7 - f64::from(lon0)) * CELLS_PER_DEG)
                    .collect();
                paint_ring(&mut block, &xs, &ys, alt);
            }
        }
        block
    }

    /// The highest minimum altitude of the sectors touching the cell of the point, feet; `None` if
    /// no sector touches it.
    pub fn at(&mut self, lat: f64, lon: f64) -> Option<i32> {
        if !(lat.is_finite() && lon.is_finite()) {
            return None;
        }
        let key = (lat.floor() as i32, lon.floor() as i32);
        if !self.blocks.contains_key(&key) {
            let b = self.build(key.0, key.1);
            self.blocks.insert(key, b);
        }
        let row =
            (((lat - f64::from(key.0)) * CELLS_PER_DEG).floor() as usize).min(BLOCK_CELLS - 1);
        let col =
            (((lon - f64::from(key.1)) * CELLS_PER_DEG).floor() as usize).min(BLOCK_CELLS - 1);
        let v = self
            .blocks
            .get(&key)?
            .get(row * BLOCK_CELLS + col)
            .copied()?;
        (v != NO_CHART).then_some(i32::from(v))
    }

    /// The model the raster was built from (the oracle of the tests asks it point by point).
    pub fn model(&self) -> &Model {
        &self.model
    }
}
