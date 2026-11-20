//! The spatial grid over one cycle's tracks and the scan for pairs near each other. Core (pure).
//!
//! Bands of latitude [`BAND_MARGIN`] times the record radius tall, each cut into as many cells of
//! longitude as keep a cell at least as wide as a band is tall at its edge nearest a pole (the
//! scheme of `oa-track`'s coarse grid, with a band sized to this radius). A pair inside the radius
//! is in the same or an adjacent band, and in the cells that overlap the longitude window of the
//! radius at the highest latitude of the three bands, so scanning those cells finds every pair
//! the flat-earth test would pass: a superset of the pairs within the radius below about 80
//! degrees. Each pair is found once, from its lower-numbered track.

use alloc::vec::Vec;

use crate::state::{Cand, EncounterState};
use crate::types::CycleStats;

/// Metres of a degree of latitude, a lower bound over the sphere the tool measures on.
pub(crate) const M_PER_DEG_LOW: f64 = 110_900.0;
/// A band is this much taller than the radius, so the radius never reaches past the next band.
pub(crate) const BAND_MARGIN: f64 = 1.05;
/// The flat-earth test accepts this multiple of the radius, covering its own error.
const CHEAP_MARGIN: f64 = 1.02;
const MAX_CELLS: u64 = u32::MAX as u64;

/// `(cells of longitude per band, first cell number per band, cosine of the band's edge nearest
/// a pole)`, or `None` if the cells would not fit a `u32`.
pub(crate) fn grid_tables(band_deg: f64) -> Option<(Vec<u32>, Vec<u32>, Vec<f64>)> {
    let n_bands = oa_ieee::ceil(180.0 / band_deg) as usize;
    let mut lon_cells = Vec::with_capacity(n_bands);
    let mut first_cell = Vec::with_capacity(n_bands);
    let mut cos_edge = Vec::with_capacity(n_bands);
    let mut next = 0u64;
    for b in 0..n_bands {
        let lo = -90.0 + band_deg * b as f64;
        let hi = lo + band_deg;
        let edge = if lo.abs() > hi.abs() {
            lo.abs()
        } else {
            hi.abs()
        };
        let edge = if edge > 90.0 { 90.0 } else { edge };
        let c = oa_math::cos(edge.to_radians());
        let c = if c > 0.0 { c } else { 0.0 };
        let n = (oa_ieee::floor(360.0 * c / band_deg) as u64).max(1);
        if next + n > MAX_CELLS {
            return None;
        }
        lon_cells.push(n as u32);
        first_cell.push(next as u32);
        cos_edge.push(c);
        next += n;
    }
    Some((lon_cells, first_cell, cos_edge))
}

fn lat_band(st: &EncounterState, lat: f64) -> usize {
    let n = st.lon_cells.len();
    let b = oa_ieee::floor((lat + 90.0) / st.band_deg);
    if b < 0.0 {
        0
    } else if b >= n as f64 {
        n.saturating_sub(1)
    } else {
        b as usize
    }
}

fn lon_cell(st: &EncounterState, band: usize, lon: f64) -> i64 {
    let n = i64::from(st.lon_cells.get(band).copied().unwrap_or(1));
    i64::rem_euclid(oa_ieee::floor((lon + 180.0) / 360.0 * n as f64) as i64, n)
}

/// Fills the per-track tables and the sorted keys from `st.obs`.
pub(crate) fn grid_build(st: &mut EncounterState) {
    st.cos_lat.clear();
    st.keys.clear();
    for i in 0..st.obs.len() {
        let Some(o) = st.obs.get(i).copied() else {
            break;
        };
        st.cos_lat.push(oa_math::cos(o.lat_deg.to_radians()));
        let band = lat_band(st, o.lat_deg);
        let lo = lon_cell(st, band, o.lon_deg);
        let first = st.first_cell.get(band).copied().unwrap_or(0);
        let cell = u64::from(first) + lo as u64;
        st.keys.push(cell << 32 | i as u64);
    }
    st.keys.sort_unstable();
}

/// Scans every track's neighbourhood, appending the pairs within the flat-earth radius to
/// `st.cands`. Each cell probe and each track met in a cell is one unit of work, checked against
/// the budget before it is done; when it runs out the scan stops and the tracks not reached are
/// counted.
pub(crate) fn grid_scan(st: &mut EncounterState, stats: &mut CycleStats) {
    let n = st.obs.len();
    let n_bands = st.lon_cells.len();
    let r = st.cfg.volume_h_m * CHEAP_MARGIN;
    let r2 = r * r;
    let budget = st.cfg.work_budget;
    let mut work = 0u64;
    'tracks: for i in 0..n {
        let (Some(oi), Some(cosl)) = (st.obs.get(i).copied(), st.cos_lat.get(i).copied()) else {
            break;
        };
        let la = lat_band(st, oi.lat_deg);
        let b0 = la.saturating_sub(1);
        let last = n_bands.saturating_sub(1);
        let b1 = if la + 1 > last { last } else { la + 1 };
        let mut cmin = 1.0f64;
        for b in b0..=b1 {
            let c = st.cos_edge.get(b).copied().unwrap_or(0.0);
            if c < cmin {
                cmin = c;
            }
        }
        // Half-width of the longitude window, degrees; 360 takes every cell of a band.
        let w = if cmin > 0.02 {
            st.band_deg / cmin
        } else {
            360.0
        };
        for lb in b0..=b1 {
            let nl = i64::from(st.lon_cells.get(lb).copied().unwrap_or(1));
            let first = u64::from(st.first_cell.get(lb).copied().unwrap_or(0));
            let lo = oa_ieee::floor((oi.lon_deg - w + 180.0) / 360.0 * nl as f64) as i64;
            let hi = oa_ieee::floor((oi.lon_deg + w + 180.0) / 360.0 * nl as f64) as i64;
            let (lo, hi) = if hi - lo + 1 >= nl {
                (0, nl - 1)
            } else {
                (lo, hi)
            };
            for k in lo..=hi {
                if work >= budget {
                    stats.tracks_unscanned = (n - i) as u64;
                    break 'tracks;
                }
                work += 1;
                let cell = first + i64::rem_euclid(k, nl) as u64;
                let mut p = st.keys.partition_point(|&x| x < cell << 32);
                while let Some(&key) = st.keys.get(p) {
                    if key >> 32 != cell {
                        break;
                    }
                    if work >= budget {
                        stats.tracks_unscanned = (n - i) as u64;
                        break 'tracks;
                    }
                    work += 1;
                    p += 1;
                    let j = (key & 0xFFFF_FFFF) as usize;
                    if j <= i {
                        continue;
                    }
                    let Some(oj) = st.obs.get(j) else { continue };
                    let dy = (oj.lat_deg - oi.lat_deg) * M_PER_DEG_LOW;
                    let mut dl = oj.lon_deg - oi.lon_deg;
                    if dl > 180.0 {
                        dl -= 360.0;
                    } else if dl < -180.0 {
                        dl += 360.0;
                    }
                    let dx = dl * cosl * M_PER_DEG_LOW;
                    let d2 = dx * dx + dy * dy;
                    if d2 <= r2 {
                        // Both altitudes known and a volume's height apart: never inside (the
                        // same difference and strict comparison as the evaluation's).
                        let dv = f64::from((oi.alt_ft - oj.alt_ft).abs());
                        if oi.alt_ft.is_finite()
                            && oj.alt_ft.is_finite()
                            && dv >= st.cfg.volume_v_ft
                        {
                            stats.pairs_skipped_altitude += 1;
                            continue;
                        }
                        stats.candidates += 1;
                        if st.cands.len() < st.cfg.max_candidates {
                            st.cands.push(Cand {
                                d2: d2 as u64,
                                i: i as u32,
                                j: j as u32,
                            });
                        } else {
                            stats.candidates_dropped += 1;
                        }
                    }
                }
            }
        }
    }
    stats.grid_work = work;
}
