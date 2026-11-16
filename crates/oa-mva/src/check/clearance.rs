//! Minimum altitude against terrain and obstacles. A sector's minimum altitude should clear the
//! highest obstacle in it by 1,000 ft in non-mountainous terrain. This measures the margin; a
//! small or negative margin is a finding about the data (or about the datums), not a bug here.
//!
//! * Terrain: every 3DEP 1 arc-second sample whose centre is inside the sector (holes
//!   and non-finite samples excluded and counted as nodata), found by scanning each grid row
//!   between the sector's edges, one tile in memory at a time. Elevations are metres on NAVD88 and are taken as MSL (the difference is under
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

impl SectorClearance {
    /// The terrain scan met no gap: no missing tile and no nodata sample. Otherwise the terrain
    /// maximum is a lower bound and [`margin_ft`] an upper bound.
    pub fn terrain_complete(&self) -> bool {
        self.tiles_missing == 0 && self.terrain_nodata == 0
    }
}

/// Margin of a sector, feet: minimum altitude less the highest terrain or obstacle. `None` when
/// the sector had neither. An upper bound when [`SectorClearance::terrain_complete`] is false.
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
    if p.is_nan() {
        return f64::NAN;
    }
    // p is in 0 ..= 1, so the product is in 0 ..= len - 1.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let ix = (sorted.len().saturating_sub(1) as f64 * p.clamp(0.0, 1.0)).round() as usize;
    sorted.get(ix).copied().unwrap_or(f64::NAN)
}

/// Edges of a sector as (x0, y0, x1, y1) degrees with y0 < y1.
fn sector_edges(m: &Model, s: SectorIx) -> Vec<(f64, f64, f64, f64)> {
    let f = E7 as f64;
    let mut out = Vec::new();
    for r in crate::ring_range(m, s) {
        let (lats, lons) = crate::ring_vertices(m, r);
        for (ys, xs) in lats.windows(2).zip(lons.windows(2)) {
            if let ([ay, by], [ax, bx]) = (ys, xs) {
                let (ay, ax, by, bx) = (*ay, *ax, *by, *bx);
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

impl Acc {
    pub(crate) fn new() -> Acc {
        Acc {
            max_m: f32::MIN,
            peak: (0.0, 0.0),
            samples: 0,
            nodata: 0,
        }
    }
}

/// Work units one [`clearance`] call may do: rows of the row tables, rows crossed by edges, grid
/// cells scanned, sector-by-tile and sector-by-obstacle tests. A limit, not a measure of a typical
/// run: one sector over one 1-degree tile is at most 13 million cells.
pub const MAX_CLEARANCE_WORK: u64 = 1 << 36;

/// A finite `f64` as `i64`, saturating at the ends (a bound that far out is clamped to the grid
/// by the caller, never used as an index).
#[allow(clippy::cast_possible_truncation)]
fn sat_i64(v: f64) -> i64 {
    v as i64
}

/// Adds `n` to `work`; an error once it passes `limit`.
fn spend(work: &mut u64, limit: u64, n: u64, what: &str) -> Result<(), String> {
    *work = work.saturating_add(n);
    if *work > limit {
        return Err(format!(
            "terrain clearance needs more than {limit} units of work ({what}); not computed"
        ));
    }
    Ok(())
}

/// Scans the samples of `g` whose centres lie inside the polygon given by `edges`. The row table,
/// the rows each edge crosses and the cells of each span are added to `work` and held to `limit`
/// before they are built or visited. A grid without a valid lattice or shorter than its
/// dimensions, or a non-finite edge, is an error.
pub(crate) fn scan_grid(
    g: &oa_tiff::Grid,
    edges: &[(f64, f64, f64, f64)],
    acc: &mut Acc,
    work: &mut u64,
    limit: u64,
) -> Result<(), String> {
    if edges.is_empty() || g.width == 0 || g.height == 0 {
        return Ok(());
    }
    let lattice = [g.lat0, g.lon0, g.dlat, g.dlon];
    if !(lattice.iter().all(|v| v.is_finite()) && g.dlat > 0.0 && g.dlon > 0.0) {
        return Err("terrain grid has no valid lattice".to_string());
    }
    let cells = u64::from(g.width).saturating_mul(u64::from(g.height));
    if (g.elev.len() as u64) < cells {
        return Err("terrain grid holds fewer samples than its dimensions".to_string());
    }
    if edges
        .iter()
        .any(|e| !(e.0.is_finite() && e.1.is_finite() && e.2.is_finite() && e.3.is_finite()))
    {
        return Err("sector edge is not finite".to_string());
    }
    let (ymin, ymax) = edges.iter().fold((f64::MAX, f64::MIN), |(lo, hi), e| {
        (lo.min(e.1), hi.max(e.3))
    });
    let h = i64::from(g.height);
    let w = i64::from(g.width);
    let rmin = sat_i64(((g.lat0 - ymax) / g.dlat).ceil()).max(0);
    let rmax = sat_i64(((g.lat0 - ymin) / g.dlat).floor()).min(h.saturating_sub(1));
    if rmin > rmax {
        return Ok(());
    }
    // Rows whose latitude is in [y0, y1), for one edge.
    let span = |e: &(f64, f64, f64, f64)| {
        let lo = sat_i64(((g.lat0 - e.3) / g.dlat).floor())
            .saturating_add(1)
            .max(rmin);
        let hi = sat_i64(((g.lat0 - e.1) / g.dlat).floor()).min(rmax);
        (lo, hi)
    };
    let rows_n = rmax.saturating_sub(rmin).saturating_add(1);
    let edge_rows = edges.iter().fold(0u64, |a, e| {
        let (lo, hi) = span(e);
        a.saturating_add(u64::try_from(hi.saturating_sub(lo).saturating_add(1)).unwrap_or(0))
    });
    spend(
        work,
        limit,
        u64::try_from(rows_n).unwrap_or(u64::MAX),
        "row table",
    )?;
    spend(work, limit, edge_rows, "rows crossed by edges")?;
    let mut rows: Vec<Vec<f64>> =
        vec![Vec::new(); usize::try_from(rows_n).map_err(|e| e.to_string())?];
    for e in edges {
        let &(x0, y0, x1, y1) = e;
        let (lo, hi) = span(e);
        let mut r = lo;
        while r <= hi {
            let lat = g.lat0 - r as f64 * g.dlat;
            let x = x0 + (lat - y0) * (x1 - x0) / (y1 - y0);
            let slot = usize::try_from(r.saturating_sub(rmin))
                .ok()
                .and_then(|k| rows.get_mut(k));
            if let Some(row) = slot {
                row.push(x);
            }
            r = r.saturating_add(1);
        }
    }
    for (k, xs) in rows.iter_mut().enumerate() {
        xs.sort_by(f64::total_cmp);
        let r = rmin.saturating_add(k as i64);
        let lat = g.lat0 - r as f64 * g.dlat;
        for &[xa, xb] in xs.as_chunks::<2>().0 {
            let ca = sat_i64(((xa - g.lon0) / g.dlon).ceil()).max(0);
            let cb = sat_i64(((xb - g.lon0) / g.dlon).floor()).min(w.saturating_sub(1));
            if ca > cb {
                continue;
            }
            spend(
                work,
                limit,
                u64::try_from(cb.saturating_sub(ca).saturating_add(1)).unwrap_or(u64::MAX),
                "cells",
            )?;
            // r < height and c < width, and the sample table holds width * height samples.
            let base = r.saturating_mul(w);
            let mut c = ca;
            while c <= cb {
                let at = usize::try_from(base.saturating_add(c)).ok();
                match at.and_then(|i| g.elev.get(i)) {
                    Some(&v) if !v.is_finite() => acc.nodata = acc.nodata.saturating_add(1),
                    Some(&v) => {
                        acc.samples = acc.samples.saturating_add(1);
                        if v > acc.max_m {
                            acc.max_m = v;
                            acc.peak = (lat, g.lon0 + c as f64 * g.dlon);
                        }
                    }
                    None => return Err("terrain sample index outside the grid".to_string()),
                }
                c = c.saturating_add(1);
            }
        }
    }
    Ok(())
}

/// The 3DEP tile name of the 1-degree cell whose south-west corner is (`lat`, `lon`) (western
/// hemisphere longitudes are negative).
pub fn tile_name(lat: i32, lon: i32) -> String {
    format!(
        "USGS_1_n{:02}w{:03}.tif",
        lat.saturating_add(1),
        0i32.saturating_sub(lon)
    )
}

/// Scans `sectors` of `m` against the 3DEP tiles in `tile_dir` and the obstacles of `dof`.
/// Tiles are read one at a time; a tile that is absent is counted in
/// [`SectorClearance::tiles_missing`], and one that cannot be read is an error.
///
/// A sector that is not a row of `m`, obstacle columns of different lengths, or work over
/// [`MAX_CLEARANCE_WORK`] is an error, so no sector is reported as empty for want of its data.
pub fn clearance(
    m: &Model,
    sectors: &[SectorIx],
    tile_dir: &Path,
    dof: &oa_dof::Dof,
) -> Result<Vec<SectorClearance>, String> {
    clearance_budget(m, sectors, tile_dir, dof, MAX_CLEARANCE_WORK)
}

/// [`clearance`] with an explicit work limit.
pub fn clearance_budget(
    m: &Model,
    sectors: &[SectorIx],
    tile_dir: &Path,
    dof: &oa_dof::Dof,
    limit: u64,
) -> Result<Vec<SectorClearance>, String> {
    let n = oa_dof::len(dof);
    if [
        dof.lon_e7.len(),
        dof.amsl_ft.len(),
        dof.kind.len(),
        dof.oas_code.len(),
        dof.oas_number.len(),
    ]
    .iter()
    .any(|&l| l != n)
    {
        return Err("obstacle columns have different lengths".to_string());
    }
    for &s in sectors {
        let i = s.0 as usize;
        let rows = [
            m.bbox_lat_min.len(),
            m.bbox_lat_max.len(),
            m.bbox_lon_min.len(),
            m.bbox_lon_max.len(),
            m.min_alt_ft.len(),
            m.set.len(),
        ];
        if rows.iter().any(|&l| i >= l) || i >= crate::sector_count(m) {
            return Err(format!("sector {} is not in the model", s.0));
        }
    }
    let mut work = 0u64;
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
    let mut accs: Vec<Acc> = sectors.iter().map(|_| Acc::new()).collect();
    let mut missing = vec![0u32; sectors.len()];
    // The 1-degree cells any sector touches, counted from the boxes before any is listed.
    let whole = |v: i32| i32::try_from(i64::from(v).div_euclid(E7)).unwrap_or(i32::MAX);
    let mut listed = 0u64;
    for &s in sectors {
        let i = s.0 as usize;
        let b = |c: &[i32]| c.get(i).copied().map_or(0, whole);
        let (la, lo) = (
            i64::from(b(&m.bbox_lat_max))
                .saturating_sub(i64::from(b(&m.bbox_lat_min)))
                .saturating_add(1),
            i64::from(b(&m.bbox_lon_max))
                .saturating_sub(i64::from(b(&m.bbox_lon_min)))
                .saturating_add(1),
        );
        listed = listed.saturating_add(u64::try_from(la.saturating_mul(lo)).unwrap_or(0));
    }
    spend(
        &mut work,
        limit,
        listed,
        "1-degree cells of the sector boxes",
    )?;
    let mut cells: Vec<(i32, i32)> = Vec::new();
    for &s in sectors {
        let i = s.0 as usize;
        let b = |c: &[i32]| c.get(i).copied().map_or(0, whole);
        for la in b(&m.bbox_lat_min)..=b(&m.bbox_lat_max) {
            for lo in b(&m.bbox_lon_min)..=b(&m.bbox_lon_max) {
                cells.push((la, lo));
            }
        }
    }
    cells.sort_unstable();
    cells.dedup();
    let tests = (cells.len() as u64).saturating_mul(sectors.len() as u64);
    spend(&mut work, limit, tests, "sector and 1-degree cell pairs")?;
    for (la, lo) in cells {
        let touching: Vec<usize> = sectors
            .iter()
            .enumerate()
            .filter(|&(_, &s)| {
                let (a0, a1, o0, o1) = bbox(s);
                a1 > f64::from(la)
                    && a0 < f64::from(la.saturating_add(1))
                    && o1 > f64::from(lo)
                    && o0 < f64::from(lo.saturating_add(1))
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
                    *x = x.saturating_add(1);
                }
            }
            continue;
        }
        let bytes = oa_tiff::read_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let g = oa_tiff::read_geotiff(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        drop(bytes);
        for &k in &touching {
            if let (Some(acc), Some(es)) = (accs.get_mut(k), edges.get(k)) {
                scan_grid(&g, es, acc, &mut work, limit)
                    .map_err(|e| format!("{}: {e}", path.display()))?;
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
    spend(
        &mut work,
        limit,
        (sectors.len() as u64).saturating_mul(near.len() as u64),
        "sector and obstacle pairs",
    )?;
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
            count = count.saturating_add(1);
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
                    u32::try_from(o)
                        .ok()
                        .and_then(|i| oa_dof::oas_number(dof, oa_dof::ObstacleIx(i)))
                        .unwrap_or_default(),
                    dof.kind.get(o).map(|b| oa_dof::text(b)).unwrap_or_default(),
                )
            }),
            obstacles: count,
        });
    }
    Ok(out)
}
