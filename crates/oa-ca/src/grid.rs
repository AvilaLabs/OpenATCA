//! The spatial grid over one cycle's tracks and the scan for pairs near each other. Core (pure).
//!
//! The scheme of `oa-encounter`'s grid (and `oa-track`'s coarse grid): bands of latitude of the
//! configured height, each cut into as many cells of longitude as keep a cell at least as wide
//! as a band is tall at its edge nearest a pole. Unlike the record's, the radius is not fixed:
//! for track `i` it is `T (v_i + v_max) + H_max + r_i + r_max` (look-ahead times the closing
//! speed bound, the largest horizontal minimum, and both 95 % radii), so the scan visits as many
//! bands either side as the radius needs, and each pair is then accepted by its own radius
//! `T (v_i + v_j) + H_max + r_i + r_j`. Each pair is found once, from its lower-numbered track.

use alloc::vec::Vec;

use crate::solve::vertical_span;
use crate::state::{CaState, Cand};
use crate::types::{CycleStats, COAST_V_ALLOWANCE_FT, F_COAST, F_UNCERTAIN};

/// Metres of a degree of latitude, a lower bound over the sphere the tool measures on.
pub(crate) const M_PER_DEG_LOW: f64 = 110_900.0;
/// The flat-earth test accepts this multiple of the radius, covering its own error.
pub(crate) const CHEAP_MARGIN: f64 = 1.02;
const MAX_CELLS: u64 = u32::MAX as u64;
/// Metres of the smallest sphere radius the flat test is proved against.
pub(crate) const R_MIN_M: f64 = 6_356_752.0;

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

fn lat_band(st: &CaState, lat: f64) -> usize {
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

fn lon_cell(st: &CaState, band: usize, lon: f64) -> i64 {
    let n = i64::from(st.lon_cells.get(band).copied().unwrap_or(1));
    i64::rem_euclid(oa_ieee::floor((lon + 180.0) / 360.0 * n as f64) as i64, n)
}

/// Fills the per-track tables and the sorted keys from `st.obs`.
pub(crate) fn grid_build(st: &mut CaState) {
    st.cos_lat.clear();
    st.speed.clear();
    st.keys.clear();
    for i in 0..st.obs.len() {
        let Some(o) = st.obs.get(i).copied() else {
            break;
        };
        st.cos_lat.push(oa_math::cos(o.lat_deg.to_radians()));
        let (e, n) = (f64::from(o.vel_east_mps), f64::from(o.vel_north_mps));
        st.speed.push(oa_ieee::sqrt(e * e + n * n));
        let band = lat_band(st, o.lat_deg);
        let lo = lon_cell(st, band, o.lon_deg);
        let first = st.first_cell.get(band).copied().unwrap_or(0);
        let cell = u64::from(first) + lo as u64;
        st.keys.push(cell << 32 | i as u64);
    }
    st.keys.sort_unstable();
}

/// The radius of the pair `(a, b)` of speeds and 95 % radii: the closest they can start and
/// still be inside the largest horizontal minimum plus both radii within the look-ahead.
fn pair_radius(st: &CaState, va: f64, vb: f64, ra: f64, rb: f64) -> f64 {
    st.look_ahead_s * (va + vb) + st.h_max_m + ra + rb
}

/// The largest search radius of the cycle, metres (zero for no tracks).
pub(crate) fn max_radius(st: &CaState) -> f64 {
    let vmax = st.speed.iter().copied().fold(0.0f64, oa_math::max_f64);
    let rmax = st
        .obs
        .iter()
        .map(|o| f64::from(o.r95_m))
        .fold(0.0f64, oa_math::max_f64);
    if st.obs.is_empty() {
        0.0
    } else {
        pair_radius(st, vmax, vmax, rmax, rmax)
    }
}

/// True if `a` and `b`, with both altitudes and rates known, cannot come within the largest
/// vertical limit (plus the coast allowance of the pair) at any time in the look-ahead.
fn cannot_converge(st: &CaState, a: &crate::types::CaTrack, b: &crate::types::CaTrack) -> bool {
    if !(a.alt_ft.is_finite()
        && b.alt_ft.is_finite()
        && a.vrate_fpm.is_finite()
        && b.vrate_fpm.is_finite())
    {
        return false;
    }
    let mut v = st.v_max_ft;
    for t in [a, b] {
        if t.flags & (F_COAST | F_UNCERTAIN) != 0 {
            v += COAST_V_ALLOWANCE_FT;
        }
    }
    let dz = f64::from(b.alt_ft) - f64::from(a.alt_ft);
    let dvz = (f64::from(b.vrate_fpm) - f64::from(a.vrate_fpm)) / 60.0;
    vertical_span(dz, dvz, v, st.look_ahead_s).is_none()
}

/// Scans every track's neighbourhood, appending the pairs within the flat-earth radius to
/// `st.cands`. Each band step, cell probe and track met in a cell is one unit of work, checked
/// against the budget before it is done; when it runs out the scan stops and the tracks not
/// reached are counted.
pub(crate) fn grid_scan(st: &mut CaState, stats: &mut CycleStats) {
    let n = st.obs.len();
    let n_bands = st.lon_cells.len();
    let last = n_bands.saturating_sub(1);
    let vmax = st.speed.iter().copied().fold(0.0f64, oa_math::max_f64);
    let rmax = st
        .obs
        .iter()
        .map(|o| f64::from(o.r95_m))
        .fold(0.0f64, oa_math::max_f64);
    let budget = st.cfg.work_budget;
    let mut work = 0u64;
    'tracks: for i in 0..n {
        let (Some(oi), Some(cosl), Some(vi)) = (
            st.obs.get(i).copied(),
            st.cos_lat.get(i).copied(),
            st.speed.get(i).copied(),
        ) else {
            break;
        };
        let ri = f64::from(oi.r95_m);
        let search = pair_radius(st, vi, vmax, ri, rmax) * CHEAP_MARGIN;
        let rdeg = search / M_PER_DEG_LOW;
        let la = lat_band(st, oi.lat_deg);
        // Bands either side the radius can reach; at least one, at most all of them.
        let k = (oa_math::max_f64(oa_ieee::ceil(rdeg / st.band_deg), 1.0) as usize).min(n_bands);
        let b0 = la.saturating_sub(k);
        let b1 = if la + k > last { last } else { la + k };
        let mut cmin = 1.0f64;
        for b in b0..=b1 {
            if work >= budget {
                stats.tracks_unscanned = (n - i) as u64;
                break 'tracks;
            }
            work += 1;
            let c = st.cos_edge.get(b).copied().unwrap_or(0.0);
            if c < cmin {
                cmin = c;
            }
        }
        // Half-width of the longitude window, degrees; 360 takes every cell of a band.
        let w = if cmin > 0.02 { rdeg / cmin } else { 360.0 };
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
            for kk in lo..=hi {
                if work >= budget {
                    stats.tracks_unscanned = (n - i) as u64;
                    break 'tracks;
                }
                work += 1;
                let cell = first + i64::rem_euclid(kk, nl) as u64;
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
                    let (Some(oj), Some(vj)) = (st.obs.get(j), st.speed.get(j).copied()) else {
                        continue;
                    };
                    let dy = (oj.lat_deg - oi.lat_deg) * M_PER_DEG_LOW;
                    let mut dl = oj.lon_deg - oi.lon_deg;
                    if dl > 180.0 {
                        dl -= 360.0;
                    } else if dl < -180.0 {
                        dl += 360.0;
                    }
                    let dx = dl * cosl * M_PER_DEG_LOW;
                    let d2 = dx * dx + dy * dy;
                    let r = pair_radius(st, vi, vj, ri, f64::from(oj.r95_m)) * CHEAP_MARGIN;
                    if d2 <= r * r {
                        if cannot_converge(st, &oi, oj) {
                            stats.pairs_skipped_vertical += 1;
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

/// The exact pair radius, metres: beyond it two tracks cannot be inside the largest minimum and
/// both 95 % radii at any time in the look-ahead.
pub(crate) fn exact_radius(st: &CaState, i: usize, j: usize) -> Option<f64> {
    let (a, b) = (st.obs.get(i)?, st.obs.get(j)?);
    let (va, vb) = (st.speed.get(i).copied()?, st.speed.get(j).copied()?);
    Some(pair_radius(
        st,
        va,
        vb,
        f64::from(a.r95_m),
        f64::from(b.r95_m),
    ))
}
