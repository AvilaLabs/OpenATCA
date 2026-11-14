//! Minimum altitude against terrain and obstacles. A sector's minimum altitude should clear the
//! highest obstacle in it by 1,000 ft in non-mountainous terrain. This measures the margin; a
//! small or negative margin is a finding about the data (or about the datums), not a bug here.
//!
//! * Terrain: every 3DEP 1 arc-second sample whose centre is inside the sector (holes
//!   excluded), found by scanning each grid row between the sector's edges, one tile in memory
//!   at a time. Elevations are metres on NAVD88 and are taken as MSL (the difference is under
//!   a metre at these latitudes). 3DEP is bare earth: structures come from the DOF.
//! * Obstacles: every DOF obstacle inside the sector (an edge counts as inside), by its AMSL
//!   height in feet, verified or not.
//! * Margin: minimum altitude minus the higher of the two maxima.

use crate::model::{facility_text, sector_name_text, Model, SectorIx};
use crate::query::sector_contains;
use crate::E7;
use std::path::Path;

/// Feet per metre.
pub const FEET_PER_METRE: f64 = 1.0 / oa_tiff::METRES_PER_FOOT;
/// The clearance FAA Order JO 7210.3 and JO 7110.65 require over obstacles outside mountainous
/// areas, feet (2,000 ft in designated mountainous areas).
pub const REQUIRED_CLEARANCE_FT: f64 = 1_000.0;

/// Margin below 1,000 ft by which a sector is still taken as rounded down to the hundred: chart
/// altitudes are whole hundreds of feet, so a minimum set from obstacle + 1,000 ft can sit up to
/// 49 ft below it (an obstacle at 2,049 ft gives 3,049 ft, charted as 3,000). My reading of the
/// charting practice, not a quoted rule: margins from 951 ft up are listed apart from the rest.
pub const ROUNDING_TOLERANCE_FT: f64 = 50.0;

/// What the terrain and obstacle scan found for one sector. Plain data (A1).
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, PartialEq)]
pub struct SectorClearance {
    pub sector: SectorIx,
    pub facility: String,
    pub set: &'static str,
    pub sector_id: String,
    pub min_alt_ft: i32,
    /// Highest terrain sample inside the sector, feet; `None` when no sample was found (no tile).
    pub terrain_max_ft: Option<f64>,
    /// Latitude and longitude of that sample.
    pub terrain_peak: Option<(f64, f64)>,
    pub terrain_samples: u64,
    pub terrain_nodata: u64,
    /// 1-degree tiles the sector's bounding box needs that are not in the directory (the terrain
    /// maximum may be low).
    pub tiles_missing: u32,
    /// Highest obstacle AMSL inside the sector, feet.
    pub obstacle_max_ft: Option<i32>,
    /// OAS number and type of that obstacle.
    pub obstacle_peak: Option<(String, String)>,
    pub obstacles: u32,
}

/// Margin of a sector, feet: minimum altitude less the highest terrain or obstacle. `None` when
/// the sector had neither.
pub fn margin_ft(c: &SectorClearance) -> Option<f64> {
    let t = c.terrain_max_ft;
    let o = c.obstacle_max_ft.map(f64::from);
    let top = match (t, o) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    };
    top.map(|h| f64::from(c.min_alt_ft) - h)
}

/// The value at fraction `p` (0 to 1) of an ascending sorted slice, nearest rank; NaN if empty.
pub fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let ix = ((sorted.len() - 1) as f64 * p.clamp(0.0, 1.0)).round() as usize;
    sorted.get(ix).copied().unwrap_or(f64::NAN)
}

/// Edges of a sector as (x0, y0, x1, y1) degrees with y0 < y1.
fn sector_edges(m: &Model, s: SectorIx) -> Vec<(f64, f64, f64, f64)> {
    let f = E7 as f64;
    let mut out = Vec::new();
    for r in crate::ring_range(m, s) {
        let (lats, lons) = crate::ring_vertices(m, r);
        let n = lats.len().min(lons.len());
        for i in 1..n {
            if let (Some(&ay), Some(&ax), Some(&by), Some(&bx)) =
                (lats.get(i - 1), lons.get(i - 1), lats.get(i), lons.get(i))
            {
                let (ax, ay, bx, by) = (
                    f64::from(ax) / f,
                    f64::from(ay) / f,
                    f64::from(bx) / f,
                    f64::from(by) / f,
                );
                if ay < by {
                    out.push((ax, ay, bx, by));
                } else if by < ay {
                    out.push((bx, by, ax, ay));
                }
            }
        }
    }
    out
}

/// Running maximum over the samples of a sector.
pub(crate) struct Acc {
    pub(crate) max_m: f32,
    pub(crate) peak: (f64, f64),
    pub(crate) samples: u64,
    pub(crate) nodata: u64,
}

/// Scans the samples of `g` whose centres lie inside the polygon given by `edges`.
pub(crate) fn scan_grid(g: &oa_tiff::Grid, edges: &[(f64, f64, f64, f64)], acc: &mut Acc) {
    if edges.is_empty() || g.width == 0 || g.height == 0 {
        return;
    }
    let (ymin, ymax) = edges.iter().fold((f64::MAX, f64::MIN), |(lo, hi), e| {
        (lo.min(e.1), hi.max(e.3))
    });
    let h = i64::from(g.height);
    let w = i64::from(g.width);
    let rmin = ((g.lat0 - ymax) / g.dlat).ceil().max(0.0) as i64;
    let rmax = (((g.lat0 - ymin) / g.dlat).floor() as i64).min(h - 1);
    if rmin > rmax {
        return;
    }
    let mut rows: Vec<Vec<f64>> = vec![Vec::new(); (rmax - rmin + 1) as usize];
    for &(x0, y0, x1, y1) in edges {
        // Rows whose latitude is in [y0, y1).
        let lo = (((g.lat0 - y1) / g.dlat).floor() as i64 + 1).max(rmin);
        let hi = (((g.lat0 - y0) / g.dlat).floor() as i64).min(rmax);
        let mut r = lo;
        while r <= hi {
            let lat = g.lat0 - r as f64 * g.dlat;
            let x = x0 + (lat - y0) * (x1 - x0) / (y1 - y0);
            if let Some(row) = rows.get_mut((r - rmin) as usize) {
                row.push(x);
            }
            r += 1;
        }
    }
    for (k, xs) in rows.iter_mut().enumerate() {
        xs.sort_by(f64::total_cmp);
        let r = rmin + k as i64;
        let lat = g.lat0 - r as f64 * g.dlat;
        for &[xa, xb] in xs.as_chunks::<2>().0 {
            let ca = (((xa - g.lon0) / g.dlon).ceil() as i64).max(0);
            let cb = (((xb - g.lon0) / g.dlon).floor() as i64).min(w - 1);
            let base = (r * w) as usize;
            let mut c = ca;
            while c <= cb {
                match g.elev.get(base + c as usize) {
                    Some(&v) if v.is_nan() => acc.nodata += 1,
                    Some(&v) => {
                        acc.samples += 1;
                        if v > acc.max_m {
                            acc.max_m = v;
                            acc.peak = (lat, g.lon0 + c as f64 * g.dlon);
                        }
                    }
                    None => {}
                }
                c += 1;
            }
        }
    }
}

/// The 3DEP tile name of the 1-degree cell whose south-west corner is (`lat`, `lon`) (western
/// hemisphere longitudes are negative).
pub fn tile_name(lat: i32, lon: i32) -> String {
    format!("USGS_1_n{:02}w{:03}.tif", lat + 1, -lon)
}

/// Scans `sectors` of `m` against the 3DEP tiles in `tile_dir` and the obstacles of `dof`.
/// Tiles are read one at a time; a tile that is absent is counted in
/// [`SectorClearance::tiles_missing`], and one that cannot be read is an error.
pub fn clearance(
    m: &Model,
    sectors: &[SectorIx],
    tile_dir: &Path,
    dof: &oa_dof::Dof,
) -> Result<Vec<SectorClearance>, String> {
    let edges: Vec<Vec<(f64, f64, f64, f64)>> =
        sectors.iter().map(|&s| sector_edges(m, s)).collect();
    let f = E7 as f64;
    let bbox = |s: SectorIx| -> (f64, f64, f64, f64) {
        let i = s.0 as usize;
        (
            f64::from(m.bbox_lat_min.get(i).copied().unwrap_or(0)) / f,
            f64::from(m.bbox_lat_max.get(i).copied().unwrap_or(0)) / f,
            f64::from(m.bbox_lon_min.get(i).copied().unwrap_or(0)) / f,
            f64::from(m.bbox_lon_max.get(i).copied().unwrap_or(0)) / f,
        )
    };
    let mut accs: Vec<Acc> = sectors
        .iter()
        .map(|_| Acc {
            max_m: f32::MIN,
            peak: (0.0, 0.0),
            samples: 0,
            nodata: 0,
        })
        .collect();
    let mut missing = vec![0u32; sectors.len()];
    // The 1-degree cells any sector touches.
    let mut cells: Vec<(i32, i32)> = Vec::new();
    for &s in sectors {
        let (la0, la1, lo0, lo1) = bbox(s);
        for la in la0.floor() as i32..=la1.floor() as i32 {
            for lo in lo0.floor() as i32..=lo1.floor() as i32 {
                if !cells.contains(&(la, lo)) {
                    cells.push((la, lo));
                }
            }
        }
    }
    cells.sort_unstable();
    for (la, lo) in cells {
        let touching: Vec<usize> = sectors
            .iter()
            .enumerate()
            .filter(|&(_, &s)| {
                let (a0, a1, o0, o1) = bbox(s);
                a1 > f64::from(la)
                    && a0 < f64::from(la + 1)
                    && o1 > f64::from(lo)
                    && o0 < f64::from(lo + 1)
            })
            .map(|(k, _)| k)
            .collect();
        if touching.is_empty() {
            continue;
        }
        let path = tile_dir.join(tile_name(la, lo));
        if !path.is_file() {
            for &k in &touching {
                if let Some(x) = missing.get_mut(k) {
                    *x += 1;
                }
            }
            continue;
        }
        let bytes = oa_tiff::read_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let g = oa_tiff::read_geotiff(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        drop(bytes);
        for &k in &touching {
            if let (Some(acc), Some(es)) = (accs.get_mut(k), edges.get(k)) {
                scan_grid(&g, es, acc);
            }
        }
    }
    // Obstacles, prefiltered to the union of the sector boxes.
    let (mut ula0, mut ula1, mut ulo0, mut ulo1) = (i32::MAX, i32::MIN, i32::MAX, i32::MIN);
    for &s in sectors {
        let i = s.0 as usize;
        ula0 = ula0.min(m.bbox_lat_min.get(i).copied().unwrap_or(ula0));
        ula1 = ula1.max(m.bbox_lat_max.get(i).copied().unwrap_or(ula1));
        ulo0 = ulo0.min(m.bbox_lon_min.get(i).copied().unwrap_or(ulo0));
        ulo1 = ulo1.max(m.bbox_lon_max.get(i).copied().unwrap_or(ulo1));
    }
    let near: Vec<usize> = (0..oa_dof::len(dof))
        .filter(|&i| {
            let (la, lo) = (
                dof.lat_e7.get(i).copied().unwrap_or(0),
                dof.lon_e7.get(i).copied().unwrap_or(0),
            );
            la >= ula0 && la <= ula1 && lo >= ulo0 && lo <= ulo1
        })
        .collect();
    let mut out = Vec::with_capacity(sectors.len());
    for (k, &s) in sectors.iter().enumerate() {
        let i = s.0 as usize;
        let (la0, la1, lo0, lo1) = (
            m.bbox_lat_min.get(i).copied().unwrap_or(0),
            m.bbox_lat_max.get(i).copied().unwrap_or(0),
            m.bbox_lon_min.get(i).copied().unwrap_or(0),
            m.bbox_lon_max.get(i).copied().unwrap_or(0),
        );
        let mut best: Option<(i32, usize)> = None;
        let mut count = 0u32;
        for &o in &near {
            let (la, lo) = (
                dof.lat_e7.get(o).copied().unwrap_or(0),
                dof.lon_e7.get(o).copied().unwrap_or(0),
            );
            if la < la0 || la > la1 || lo < lo0 || lo > lo1 || !sector_contains(m, s, la, lo) {
                continue;
            }
            count += 1;
            let a = dof.amsl_ft.get(o).copied().unwrap_or(i32::MIN);
            if best.is_none_or(|(b, _)| a > b) {
                best = Some((a, o));
            }
        }
        let acc = accs.get(k);
        let has_t = acc.is_some_and(|a| a.samples > 0);
        out.push(SectorClearance {
            sector: s,
            facility: facility_text(m, s),
            set: m.set.get(i).map_or("?", |x| x.name()),
            sector_id: sector_name_text(m, s),
            min_alt_ft: m.min_alt_ft.get(i).copied().unwrap_or(0),
            terrain_max_ft: acc
                .filter(|_| has_t)
                .map(|a| f64::from(a.max_m) * FEET_PER_METRE),
            terrain_peak: acc.filter(|_| has_t).map(|a| a.peak),
            terrain_samples: acc.map_or(0, |a| a.samples),
            terrain_nodata: acc.map_or(0, |a| a.nodata),
            tiles_missing: missing.get(k).copied().unwrap_or(0),
            obstacle_max_ft: best.map(|b| b.0),
            obstacle_peak: best.map(|(_, o)| {
                (
                    oa_dof::oas_number(dof, oa_dof::ObstacleIx(o as u32)).unwrap_or_default(),
                    dof.kind.get(o).map(|b| oa_dof::text(b)).unwrap_or_default(),
                )
            }),
            obstacles: count,
        });
    }
    Ok(out)
}
